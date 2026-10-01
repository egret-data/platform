use crate::PolicyStoreError;
use egret_metastore::Db;
use egret_ranger_eval::resource::MatchType;
use serde_json::Value;

#[derive(Clone)]
pub struct PolicyStore {
    db: Db,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ServiceDefRow {
    pub id: i64,
    pub name: String,
    pub def_jsonb: Value,
}

#[derive(Debug, sqlx::FromRow)]
pub struct ServiceRow {
    pub id: i64,
    pub name: String,
    #[sqlx(rename = "type")]
    pub r#type: String,
    pub config_jsonb: Value,
    pub policy_version: i64,
    /// Monotonic tag-assignment version; the Ranger tag-download plugin polls this
    /// (parallel to `policy_version`) to know when to re-pull catalog tags.
    pub tag_version: i64,
    /// `"manual"` (hand-authored through the policy editor, the default — see
    /// migration 0155) or `"generated"` (owned by SP4a's permission-model
    /// compiler, `egret-perm-compile::trigger::apply_diff_in_tx`, which stamps
    /// this the moment it first compiles a policy for the service).
    ///
    /// Task 8 fix round 2 (I-1): the guard this drives is scoped PER POLICY,
    /// not per profile — `create_policy` and `seed_default_trino_policies`
    /// refuse outright once a profile is `"generated"` (see
    /// [`refuse_if_generated`]; adding brand-new content to a compiled
    /// profile is the confusion Task 8 exists to prevent), but
    /// `update_policy`/`delete_policy`/`set_policy_enabled` only refuse a
    /// SPECIFIC policy whose own name is compiler-owned (see
    /// [`refuse_if_compiler_owned`] and [`is_compiler_owned_policy_name`]) —
    /// a policy the compiler's sweep never touches is not protected by
    /// "read-only", since read-only-ness exists only to guard what the
    /// compiler would otherwise silently overwrite. `delete_service` mirrors
    /// that: refused only while a compiler-owned policy still exists under
    /// it (see [`refuse_if_compiler_owned_policies_remain`]), not merely
    /// because the profile is (or was ever) `"generated"`.
    pub origin: String,
}

/// One catalog resource → tag pairing for a service's warehouse(s), the row
/// feeding `ServiceTags::from_assignments`. Column order matches the tuple the
/// builder consumes: `(catalog, schema, table, column, tag_id, tag_name)`.
#[derive(Debug, sqlx::FromRow)]
pub struct TagAssignmentRow {
    pub catalog: String,
    pub schema_name: String,
    pub table_name: String,
    pub column_name: String,
    pub tag_id: String,
    pub tag_name: String,
}

/// Postgres' `23505 unique_violation`, whichever index raised it.
fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

/// Name prefix every ACCESS policy SP4a's permission-model compiler
/// (`egret-perm-compile::grants::compile_grant`) produces —
/// `format!("{COMPILER_GRANT_PREFIX}{}", grant.id)`.
///
/// Defined HERE, not in `egret-perm-compile`, even though that crate is the
/// one that actually mints names with it: `egret-perm-compile` depends on
/// `egret-policy` (for `PolicyStore` and the Ranger JSON renderers), never
/// the reverse, so a constant only `egret-perm-compile` owned could not be
/// consumed by this crate's own guards. `egret-perm-compile`'s
/// `trigger::apply_diff_in_tx` (the sweep that reaps any
/// `policy.authz_policies` row under this prefix that its current compile no
/// longer produces) and `grants.rs`'s name-builder both import THIS constant
/// rather than keeping their own copy — see that crate's `trigger.rs` module
/// doc. Two independent copies of this string would drift, and a drift makes
/// a policy the sweep reaps look editable to the guards below, or the
/// reverse.
pub const COMPILER_GRANT_PREFIX: &str = "perm-grant-";
/// Name prefix every MASKING/ROW-FILTER policy the compiler
/// (`egret-perm-compile::rules::compile_rule`) produces. Same rationale as
/// [`COMPILER_GRANT_PREFIX`].
pub const COMPILER_RULE_PREFIX: &str = "perm-rule-";
/// Name prefix every CLEARANCE policy the compiler
/// (`egret-perm-compile::clearance::compile_clearance`) produces —
/// `format!("{COMPILER_CLEARANCE_PREFIX}{tag_id}")`, one per gated tag. Same
/// rationale for living here as [`COMPILER_GRANT_PREFIX`].
///
/// **This one is a tag ACCESS policy, and that AMENDS SP4a's D2** ("tags drive
/// rules only, never access" — see `admin`'s re-export of
/// `tag_access_to_ranger_json`, which stayed unreachable until SP4c, and
/// `egret-perm-compile::clearance`'s module doc for the whole argument). A
/// classification exists to WITHHOLD data, and neither of D2's permitted tag
/// shapes can do that: a mask and a row filter both still answer the query. So
/// a clearance compiles to a tag-service DENY whose `denyExceptions` name the
/// cleared users. A future reader who finds a compiled tag access policy under
/// this prefix is looking at that amendment, not at a bug.
///
/// It needs a prefix of its OWN rather than riding on the grant prefix because
/// its name is keyed by `tags.id`, not by a `perm.grants` id: the editor's
/// reserved-name guard and `apply_diff_in_tx`'s existing-state sweep both key
/// on these prefixes, and a clearance filed under `perm-grant-` would be
/// indistinguishable from a grant whose row no longer exists.
pub const COMPILER_CLEARANCE_PREFIX: &str = "perm-clearance-";

/// Whether `name` identifies a policy the SP4a permission-model compiler
/// owns (produced by `compile_grant`/`compile_rule`/`compile_clearance` and
/// reachable by `apply_diff_in_tx`'s existing-state sweep) — see
/// [`COMPILER_GRANT_PREFIX`] / [`COMPILER_RULE_PREFIX`] /
/// [`COMPILER_CLEARANCE_PREFIX`].
pub fn is_compiler_owned_policy_name(name: &str) -> bool {
    name.starts_with(COMPILER_GRANT_PREFIX)
        || name.starts_with(COMPILER_RULE_PREFIX)
        || name.starts_with(COMPILER_CLEARANCE_PREFIX)
}

/// SP4a Task 8 fix round 3: refuse a human write that would put (or leave) a
/// policy NAME under the compiler's reserved prefix (`COMPILER_GRANT_PREFIX`/
/// `COMPILER_RULE_PREFIX`) — in ANY profile, `manual` or `generated`. Pure
/// (no DB access): the fact under test is the string alone.
///
/// This exists because the profile/policy-origin guards above cannot close
/// the round-2 report's finding: a human names a policy `perm-grant-foo` in
/// a still-`manual` profile (`create_policy`'s origin check does not yet
/// fire — the profile isn't `generated` yet), and the FIRST real recompile
/// that ever reaches this service silently deletes it in the same
/// transaction that stamps the profile `generated` (`apply_diff_in_tx`'s
/// existing-state sweep matches on name alone, with or without a
/// `perm.compiled` provenance row — Task 7 fix round 1's own Critical fix,
/// which this reservation deliberately does NOT try to re-litigate: making
/// the collision UNREPRESENTABLE at the only human entry point is simpler
/// and strictly stronger than teaching the sweep to defend against it).
///
/// Deliberately does not require `origin = 'generated'`: a `manual` profile
/// is exactly the case the round-2 report showed getting silently
/// steamrolled, so gating this on `origin` would reopen the same hole.
fn refuse_reserved_name(name: &str) -> Result<(), PolicyStoreError> {
    if is_compiler_owned_policy_name(name) {
        return Err(PolicyStoreError::CompilerOwned(format!(
            "policy name {name:?} starts with a prefix reserved for the SP4a permission model \
             ({COMPILER_GRANT_PREFIX:?} / {COMPILER_RULE_PREFIX:?} / \
             {COMPILER_CLEARANCE_PREFIX:?}) — only its compiler writes \
             policies there; rename this policy instead of retrying"
        )));
    }
    Ok(())
}

/// SP4a Task 8 (D4): refuse adding brand-new content to a profile the
/// permission-model compiler has claimed (`origin = 'generated'`, migration
/// 0155) — used only by `create_policy` and
/// `default_policies::seed_default_trino_policies` (via the pooled wrapper
/// below). Unlike [`refuse_if_compiler_owned`], this is deliberately
/// profile-wide: a NEW policy cannot yet be "compiler-owned or not" (it
/// doesn't exist), so the only question create-time can ask is whether this
/// profile is one a human should be adding hand-written policies to at all —
/// Task 8 fix round 2 (I-1) kept this refusal exactly where the original
/// Task 8 left it; only the EDIT/DELETE guards below moved from profile
/// scope to policy scope.
///
/// Connection-generic so every caller can run it on its OWN already-open
/// transaction. `egret-perm-compile`'s recompile driver
/// (`trigger::apply_diff_in_tx`) never calls this — it writes
/// `policy.authz_policies` with its own raw `sqlx::query` calls directly
/// against the transaction it owns — so this refusal can never block the
/// compiler itself, only a human (or anything else) going through this
/// store's public write API.
///
/// `None`/absent service (a nonexistent `service_id`) is deliberately NOT an
/// error here — the caller's own existence check (a `NotFound` from a prior
/// lookup, or the FK violation an insert against a missing service would
/// raise) is what surfaces that, unchanged from before this guard existed.
async fn refuse_if_generated(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<(), PolicyStoreError> {
    let origin: Option<String> = sqlx::query_scalar::<_, String>(
        r#"SELECT origin FROM policy.authz_services WHERE id = $1"#,
    )
    .bind(service_id)
    .fetch_optional(&mut *conn)
    .await?;
    if origin.as_deref() == Some("generated") {
        return Err(PolicyStoreError::CompilerOwned(format!(
            "profile {service_id} is compiled by the SP4a permission model — edit its grants \
             and rules instead of writing a policy directly; the next recompile would silently \
             overwrite this change"
        )));
    }
    Ok(())
}

/// SP4a Task 8 fix round 2 (I-1): refuse an edit/delete/enable-toggle on a
/// SPECIFIC policy only when it is itself compiler-owned (see
/// [`is_compiler_owned_policy_name`]) AND its profile is currently
/// `'generated'`. Read-only-ness protects what the compiler's sweep
/// (`apply_diff_in_tx`) would otherwise silently overwrite or reap on the
/// next recompile — a policy that sweep never looks at (a different name, no
/// `perm-grant-`/`perm-rule-` prefix) is not protected by that reasoning, so
/// it stays as editable/deletable as it would be in a `'manual'` profile.
/// This is the "moves from profile level to policy level" the user ruled on
/// for I-1: the old profile-wide refusal in `update_policy`/`delete_policy`/
/// `set_policy_enabled` is what built the one-way door (an operator could
/// never touch ANYTHING in a generated profile, including content the
/// compiler had no opinion about).
///
/// The `origin` check still matters even though the name-prefix check runs
/// first: a policy hand-named with this prefix in a still-`'manual'` profile
/// is not YET at risk from a sweep that has never run against this service —
/// see the fix-round-2 report's finding on whether that policy could later
/// be silently reaped once the profile becomes `'generated'` (a Task 7
/// property, tracked separately, not fixed by this guard).
async fn refuse_if_compiler_owned(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    policy_name: &str,
) -> Result<(), PolicyStoreError> {
    if !is_compiler_owned_policy_name(policy_name) {
        return Ok(());
    }
    let origin: Option<String> = sqlx::query_scalar::<_, String>(
        r#"SELECT origin FROM policy.authz_services WHERE id = $1"#,
    )
    .bind(service_id)
    .fetch_optional(&mut *conn)
    .await?;
    if origin.as_deref() == Some("generated") {
        return Err(PolicyStoreError::CompilerOwned(format!(
            "policy {policy_name:?} is compiled by the SP4a permission model — it is produced \
             by a grant or rule, not hand-authored; edit or delete that grant/rule instead of \
             writing this policy directly, since the next recompile would otherwise silently \
             overwrite this change"
        )));
    }
    Ok(())
}

/// SP4a Task 8 fix round 2 (I-1): `delete_service`'s own guard — refuse only
/// while at least one compiler-owned policy (see
/// [`is_compiler_owned_policy_name`]) still exists under `service_id`,
/// regardless of the profile's `origin`. A `'generated'` profile that the
/// compiler has since reaped down to zero owned policies (every grant/rule
/// reaching it was deleted) is deletable — `origin` is sticky (Task 8's
/// original design) precisely so the EDIT guard above keeps distinguishing
/// "this profile has been compiler-managed" from "this profile has never
/// been touched", but that stickiness has no bearing on whether deleting the
/// row right now would delete anything the compiler still owns.
async fn refuse_if_compiler_owned_policies_remain(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<(), PolicyStoreError> {
    let count: i64 = sqlx::query_scalar::<_, i64>(
        r#"SELECT count(*) FROM policy.authz_policies
           WHERE service_id = $1
             AND (name LIKE $2 || '%' OR name LIKE $3 || '%' OR name LIKE $4 || '%')"#,
    )
    .bind(service_id)
    .bind(COMPILER_GRANT_PREFIX)
    .bind(COMPILER_RULE_PREFIX)
    .bind(COMPILER_CLEARANCE_PREFIX)
    .fetch_one(&mut *conn)
    .await?;
    if count > 0 {
        let noun = if count == 1 { "policy" } else { "policies" };
        return Err(PolicyStoreError::CompilerOwned(format!(
            "profile {service_id} still holds {count} {noun} compiled by the SP4a permission \
             model (named with the perm-grant-/perm-rule-/perm-clearance- prefix) — delete the \
             grants/rules that produce them, or the classification clearance behind them, \
             first, or wait for the next recompile to reap them, before deleting the profile \
             itself"
        )));
    }
    Ok(())
}

impl PolicyStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Pool-based [`refuse_if_generated`], for a write path that has no
    /// transaction open yet at the point the check belongs
    /// (`default_policies::seed_default_trino_policies`, which must refuse
    /// BEFORE its own "already has policies" no-op check — a generated
    /// profile always has policies, so that check would otherwise silently
    /// swallow the refusal as a no-op `Ok(0)`).
    pub(crate) async fn refuse_if_generated_pooled(
        &self,
        service_id: i64,
    ) -> Result<(), PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        refuse_if_generated(&mut conn, service_id).await
    }

    /// Idempotent upsert of a service-def by name (used to seed Trino).
    pub async fn upsert_service_def(
        &self,
        id: i64,
        name: &str,
        def: &Value,
    ) -> Result<(), PolicyStoreError> {
        // `authz_service_defs` has TWO unique constraints — the `id` primary key and a unique
        // `name` — and `ON CONFLICT (id)` names only the first as its arbiter. A violation on the
        // OTHER index is not absorbed by the DO UPDATE; Postgres raises it.
        //
        // That is reachable whenever two callers seed the same def at once: each engine runs this
        // at startup, and three test binaries run it in parallel against one database. The loser
        // inserts a row whose `id` is not yet visible to it but whose `name` collides on the index,
        // and gets `23505 authz_service_defs_name_key`. Reproduced locally by running those three
        // binaries concurrently — it fails roughly one round in four.
        //
        // Retrying is the whole fix: this is an IDEMPOTENT SEED, so a concurrent caller having
        // already written the row is the desired end state, and by the second attempt that row has
        // committed and `ON CONFLICT (id)` absorbs it. One retry is enough — the race is with a
        // transaction that is already committing, not an unbounded stream of writers.
        for attempt in 0..2 {
            let result = sqlx::query(
                r#"INSERT INTO policy.authz_service_defs (id, name, def_jsonb) VALUES ($1,$2,$3)
                   ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, def_jsonb = EXCLUDED.def_jsonb"#,
            )
            .bind(id)
            .bind(name)
            .bind(def)
            .execute(&self.db.pool)
            .await;

            match result {
                Ok(_) => return Ok(()),
                Err(e) if attempt == 0 && is_unique_violation(&e) => continue,
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    pub async fn get_service_def(&self, name: &str) -> Result<ServiceDefRow, PolicyStoreError> {
        sqlx::query_as::<_, ServiceDefRow>(
            r#"SELECT id, name, def_jsonb FROM policy.authz_service_defs WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| PolicyStoreError::NotFound(format!("service-def {name}")))
    }

    pub async fn list_service_defs(&self) -> Result<Vec<ServiceDefRow>, PolicyStoreError> {
        Ok(sqlx::query_as::<_, ServiceDefRow>(
            r#"SELECT id, name, def_jsonb FROM policy.authz_service_defs ORDER BY name"#,
        )
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn create_service(
        &self,
        name: &str,
        typ: &str,
        config: &Value,
    ) -> Result<ServiceRow, PolicyStoreError> {
        sqlx::query_as::<_, ServiceRow>(
            r#"INSERT INTO policy.authz_services (name, type, config_jsonb)
               VALUES ($1,$2,$3) RETURNING id, name, type, config_jsonb, policy_version, tag_version, origin"#,
        )
        .bind(name)
        .bind(typ)
        .bind(config)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                PolicyStoreError::AlreadyExists(name.into())
            }
            _ => PolicyStoreError::Db(e),
        })
    }

    /// Like [`get_service`], keyed by id instead of name — SP4a Task 7's
    /// recompile driver is handed a `service_id` (the trigger's caller
    /// already resolved which services a tag/grant write reaches; see
    /// `egret_catalog_registry::services_bound_to_warehouse`) and has no
    /// service name to look it up by until it reads this row.
    pub async fn get_service_by_id(&self, id: i64) -> Result<ServiceRow, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        get_service_by_id_via(&mut conn, id).await
    }

    pub async fn get_service(&self, name: &str) -> Result<ServiceRow, PolicyStoreError> {
        sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version, tag_version, origin FROM policy.authz_services WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| PolicyStoreError::NotFound(format!("service {name}")))
    }

    /// Like [`get_service`], but returns `None` instead of `NotFound` when the
    /// named service is absent (used by seed paths that no-op when unbound).
    pub async fn get_service_opt(
        &self,
        name: &str,
    ) -> Result<Option<ServiceRow>, PolicyStoreError> {
        Ok(sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version, tag_version, origin FROM policy.authz_services WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// Whether a policy of the given name already exists under a service (used to
    /// make policy seeding idempotent).
    pub async fn policy_exists(
        &self,
        service_id: i64,
        name: &str,
    ) -> Result<bool, PolicyStoreError> {
        Ok(sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS(SELECT 1 FROM policy.authz_policies WHERE service_id = $1 AND name = $2)"#,
        )
        .bind(service_id)
        .bind(name)
        .fetch_one(&self.db.pool)
        .await?)
    }

    pub async fn list_services(&self) -> Result<Vec<ServiceRow>, PolicyStoreError> {
        Ok(sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version, tag_version, origin FROM policy.authz_services ORDER BY name"#,
        )
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// The name of the tag service `service_id` can actually USE, or `None`.
    ///
    /// Both halves must hold: the profile carries a `tagService` in its
    /// `config_jsonb`, AND that name resolves to a real
    /// `policy.authz_services` row. The join is the same one
    /// [`services_and_tag_services_via`] uses, so the two cannot disagree about
    /// what "has a tag service" means — which matters, because that function
    /// silently contributes nothing for a dangling `tagService`, and this is
    /// the predicate callers use to refuse BEFORE reaching that silence.
    ///
    /// A profile with no usable tag service compiles ZERO tag policies, so
    /// every mask and row filter vanishes and the columns are served raw.
    /// `PUT /engines/{id}` refuses to bind an engine to such a profile.
    pub async fn usable_tag_service(
        &self,
        service_id: i64,
    ) -> Result<Option<String>, PolicyStoreError> {
        Ok(sqlx::query_scalar::<_, String>(
            r#"SELECT t.name
                 FROM policy.authz_services s
                 JOIN policy.authz_services t
                   ON t.name = s.config_jsonb ->> 'tagService'
                WHERE s.id = $1"#,
        )
        .bind(service_id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// DELETE a profile outright — refused (Task 8) while it still holds any
    /// compiler-owned policy (see [`refuse_if_compiler_owned_policies_remain`]):
    /// a compiled profile's row (and the `perm.compiled` provenance `ON DELETE
    /// CASCADE`s from it, migration 0154) stays owned by the compiler until
    /// the perm model stops targeting it, not until an admin deletes it out
    /// from under a still-live recompile. Fix round 2 (I-1): this is scoped to
    /// the policies actually present, not the profile's `origin` — a
    /// `'generated'` profile the compiler has since reaped down to zero owned
    /// policies is deletable.
    ///
    /// Review fix (M-1): the check and the delete run on the SAME
    /// transaction, not a pooled check followed by a separately-connected
    /// delete (the shape Task 7's `recompile_service_in_tx` exists to avoid
    /// for exactly this reason — see that function's own doc comment). Two
    /// separate connections left a window: a compile writing a NEW
    /// compiler-owned policy between the check and the delete would let a
    /// profile be deleted anyway despite that write. One transaction closes
    /// it — Postgres holds the relevant rows from the `SELECT count(*)`
    /// through the `DELETE`, so a concurrent write from `apply_diff_in_tx`
    /// either commits first (this delete then sees the new row and refuses)
    /// or blocks until this transaction ends.
    pub async fn delete_service(&self, id: i64) -> Result<(), PolicyStoreError> {
        let mut tx = self.db.pool.begin().await?;
        if let Err(e) = refuse_if_compiler_owned_policies_remain(&mut tx, id).await {
            tx.rollback().await?;
            return Err(e);
        }
        let n = sqlx::query(r#"DELETE FROM policy.authz_services WHERE id = $1"#)
            .bind(id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if n == 0 {
            tx.rollback().await?;
            return Err(PolicyStoreError::NotFound(format!("service id {id}")));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Total policy count per service (service_id -> count), for the profiles list.
    pub async fn policy_counts(
        &self,
    ) -> Result<std::collections::HashMap<i64, i64>, PolicyStoreError> {
        let rows = sqlx::query_as::<_, (i64, i64)>(
            "SELECT service_id, count(*) FROM policy.authz_policies GROUP BY service_id",
        )
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows.into_iter().collect())
    }

    /// Catalog resource → tag rows for a service's warehouse(s), feeding the
    /// Ranger tag download (`ServiceTags::from_assignments`).
    ///
    /// SERVICE → CATALOG-NAME MAPPING: a policy profile (`policy.authz_services`)
    /// is bound to warehouses through the engine seam —
    /// `engine_mgmt.engines.authz_service_id`. Each such warehouse's tag
    /// assignments (keyed by `warehouse_id`) are served under every Trino
    /// catalog name the engine attaches, reached through
    /// `registry.engine_catalog_attachments` (SP3 D5).
    ///
    /// One branch only, since SP3c-2: the engine attaches this warehouse's
    /// registry catalog through `registry.engine_catalog_attachments`.
    ///
    /// Until SP3c-2 a second branch read `engine_mgmt.engine_catalogs` for two
    /// populations the registry could not see: engines whose backfill failed,
    /// and a catalog renamed but not yet re-applied. Both are gone — the backfill
    /// is deleted, so no engine can be in the first state, and
    /// `registry.catalogs.name` is UNIQUE and immutable, so nothing renames a
    /// catalog at all.
    pub async fn tag_assignments_for_service(
        &self,
        service_id: i64,
    ) -> Result<Vec<TagAssignmentRow>, PolicyStoreError> {
        // `$1` is the service the plugin polled. The Trino tag plugin polls the
        // TAG service (`<profile>_tag`), but engines bind to the TRINO profile —
        // so resolve the trino profile that owns this tag service (via its
        // `tagService` config) and match engines on THAT. `COALESCE(tp.id, s.id)`
        // also accepts the trino profile id directly, so callers that already hold
        // it (and the unit tests) keep working. Passing the tag service id straight
        // through — as the HTTP `download_tags` path did — matched no engine and
        // made tag masking silently no-op.
        Ok(sqlx::query_as::<_, TagAssignmentRow>(
            r#"SELECT
                   c.name           AS catalog,
                   a.schema_name    AS schema_name,
                   a.table_name     AS table_name,
                   a.column_name    AS column_name,
                   t.id::text       AS tag_id,
                   t.name           AS tag_name
               FROM catalog_tag_assignments a
               JOIN tags t                 ON t.id = a.tag_id
               JOIN policy.authz_services s ON s.id = $1
               LEFT JOIN policy.authz_services tp
                    ON tp.type = 'trino' AND tp.config_jsonb->>'tagService' = s.name
               JOIN registry.catalogs c
                    ON c.warehouse_id = a.warehouse_id
               JOIN registry.engine_catalog_attachments ea
                    ON ea.catalog_id = c.id
               JOIN engine_mgmt.engines e
                    ON e.id = ea.engine_id
                   AND e.authz_service_id = COALESCE(tp.id, s.id)
               ORDER BY catalog, schema_name, table_name, column_name, tag_name"#,
        )
        .bind(service_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Registry catalogs reachable by `service_id`'s engine(s), through
    /// `registry.engine_catalog_attachments` (SP3 D5) — the DIRECT engine
    /// bind (`e.authz_service_id = service_id`), not the tag-service
    /// `COALESCE` `tag_assignments_for_service` resolves: a recompile always
    /// targets the profile's own service id, never its paired tag service
    /// (StarRocks has no tag service at all — D5).
    pub async fn catalogs_for_service(
        &self,
        service_id: i64,
    ) -> Result<Vec<CatalogRow>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        catalogs_for_service_via(&mut conn, service_id).await
    }

    /// Columns carrying `tags.key = tag_key`, resolved to their Ranger-facing
    /// NAMES, restricted to catalogs `service_id`'s engine(s) reach.
    /// Table-level tags (`column_name = ''`, migration 0081) are excluded —
    /// this feeds the StarRocks per-column expansion (D5), which masks a
    /// COLUMN, never a whole table.
    pub async fn tagged_columns_for_service(
        &self,
        service_id: i64,
        tag_key: &str,
    ) -> Result<Vec<TaggedColumnRow>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        tagged_columns_for_service_via(&mut conn, service_id, tag_key).await
    }

    /// `tags.name` for a `tags.key` — D6: rules select by the stable `key`,
    /// but every engine (and the Ranger tag plugin) matches on the mutable
    /// `name`, so this is the resolution `ResolvedNames.tag_name` needs
    /// before a tag-selector rule can compile. `None` when no such key
    /// exists (a rule can outlive the tag it named, e.g. after a tag
    /// deletion — the caller decides what that means for the compile, not
    /// this lookup).
    pub async fn tag_name_for_key(&self, key: &str) -> Result<Option<String>, PolicyStoreError> {
        let mut conn = self.db.pool.acquire().await?;
        tag_name_for_key_via(&mut conn, key).await
    }
}

/// Connection-generic core of [`PolicyStore::get_service_by_id`] — SP4a
/// Task 7 fix round 1: `egret-perm-compile`'s `recompile_service_in_tx`
/// reads the service row from inside the caller's OWN open transaction
/// (`egret-bi::catalog_tags`'s tag write), not a second pool-acquired
/// connection — see `egret_perm::store::list_grants_via`'s doc comment for
/// why that distinction matters (READ COMMITTED visibility of the caller's
/// own uncommitted writes).
pub async fn get_service_by_id_via(
    conn: &mut sqlx::PgConnection,
    id: i64,
) -> Result<ServiceRow, PolicyStoreError> {
    sqlx::query_as::<_, ServiceRow>(
        r#"SELECT id, name, type, config_jsonb, policy_version, tag_version, origin FROM policy.authz_services WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| PolicyStoreError::NotFound(format!("service id {id}")))
}

/// The id of the profile NAMED `name`, or `None` when no such row exists.
///
/// **SP4a Task 11, defect 2.** A compiled policy's Ranger body carries the
/// name of the service it belongs to (`"service": "<name>"`), and for a
/// tag-selected rule on Trino that name is the profile's TAG service, not the
/// profile itself (`admin::ranger_json::tagify`). `egret-perm-compile`'s
/// recompile driver stored every policy it produced under the profile's own
/// `service_id` regardless, so the tag plugin's download
/// (`/service/plugins/policies/download/<profile>_tag`) served ZERO policies
/// while the resource profile carried a `serviceType: tag` policy it can never
/// match — a mask that deployed, listed as live, and returned the real values
/// (`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3, case
/// T3). This is the lookup that turns the body's own `service` name into the
/// row's `service_id`, so the two can no longer disagree.
///
/// Connection-generic for the same reason as [`get_service_by_id_via`]: the
/// driver runs inside the caller's own open transaction.
pub async fn service_id_by_name_via(
    conn: &mut sqlx::PgConnection,
    name: &str,
) -> Result<Option<i64>, PolicyStoreError> {
    Ok(
        sqlx::query_scalar::<_, i64>(r#"SELECT id FROM policy.authz_services WHERE name = $1"#)
            .bind(name)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// Connection-generic core of [`PolicyStore::catalogs_for_service`] — same
/// rationale as [`get_service_by_id_via`].
pub async fn catalogs_for_service_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<CatalogRow>, PolicyStoreError> {
    Ok(sqlx::query_as::<_, CatalogRow>(
        r#"SELECT DISTINCT c.id, c.name
           FROM registry.catalogs c
           JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id
           WHERE e.authz_service_id = $1"#,
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await?)
}

/// Every service in `service_ids`, **plus each one's tag service**, ascending.
///
/// SP4a Task 11, defect 2: a recompile of a Trino profile now reads and writes
/// `policy.authz_policies` rows under the profile's TAG service as well as
/// under the profile itself, and takes a `FOR UPDATE` row lock on each. The
/// lock ORDER is what keeps two overlapping recompiles queueing instead of
/// deadlocking (`egret_perm_compile::trigger::lock_services_in_tx`), and a
/// profile's tag service can perfectly well have a LOWER id than the profile —
/// the live cluster's `sp3c2-trino-live` is profile 52 with tag service 51. So
/// the set every caller locks has to be the EXPANDED one, sorted once, rather
/// than profiles first and their tag services discovered later.
///
/// A `tagService` naming no row contributes nothing (the join drops it), which
/// is the same treatment the compiler gives it.
pub async fn services_and_tag_services_via(
    conn: &mut sqlx::PgConnection,
    service_ids: &[i64],
) -> Result<Vec<i64>, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"SELECT DISTINCT q.id FROM (
               SELECT s.id
                 FROM policy.authz_services s
                WHERE s.id = ANY($1)
               UNION
               SELECT t.id
                 FROM policy.authz_services s
                 JOIN policy.authz_services t
                   ON t.name = s.config_jsonb ->> 'tagService'
                WHERE s.id = ANY($1)
           ) q
           ORDER BY 1"#,
    )
    .bind(service_ids)
    .fetch_all(&mut *conn)
    .await?)
}

/// **The reverse of [`catalogs_for_service_via`]**: which policy profiles does
/// a change to THIS catalog affect? SP4a Task 9 needs it — a grant names a
/// catalog, so a grant write has to recompile exactly the profiles whose
/// engines reach that catalog. Same join, walked the other way, and the same
/// `authz_service_id` binding `egret_catalog_registry::services_bound_to_warehouse`
/// uses for the tag-write trigger; this one is keyed by catalog rather than
/// by warehouse because that is the level a `perm.grants.catalog_id` names.
///
/// Profiles NO engine binds are deliberately absent: nothing downloads them,
/// so nothing enforces them. `POST /api/control-plane/perm/compile/{id}` is
/// the repair trigger for a profile that later acquires a binding.
pub async fn services_for_catalog_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: uuid::Uuid,
) -> Result<Vec<i64>, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"SELECT DISTINCT e.authz_service_id
           FROM registry.engine_catalog_attachments ea
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id
           WHERE ea.catalog_id = $1 AND e.authz_service_id IS NOT NULL
           ORDER BY 1"#,
    )
    .bind(catalog_id)
    .fetch_all(&mut *conn)
    .await?)
}

/// Every profile the SP4a compiler targets and an engine actually binds —
/// i.e. every `trino`/`starrocks` service some `engine_mgmt.engines` row
/// points at.
///
/// **This is deliberately installation-wide.** A tag-selected rule is
/// catalog-UNSCOPED on Trino by design (D5: one tag policy, resolved by the
/// Ranger tag plugin, covering whatever the tag is on), so creating or
/// deleting one rule changes the compiled output of EVERY Trino profile in
/// the installation — there is no narrower honest answer. StarRocks profiles
/// are included for the same mutation because the same rule compiles there
/// too, per tagged column. See `egret_perm_compile::trigger::affected_services_for_rule`.
pub async fn compiler_targeted_services_via(
    conn: &mut sqlx::PgConnection,
) -> Result<Vec<i64>, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, i64>(
        r#"SELECT DISTINCT s.id
           FROM policy.authz_services s
           JOIN engine_mgmt.engines e ON e.authz_service_id = s.id
           WHERE s.type IN ('trino', 'starrocks')
           ORDER BY s.id"#,
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// The `tags.key`s carried by ONE exact resource (`column = ""` means the
/// table itself), resolved through the catalog's warehouse.
///
/// # ⚠ EXACT, not hierarchy-complete — do not feed this to an evaluator
///
/// `schema_name` / `table_name` / `column_name` are matched by string
/// EQUALITY, so asked about `report.country` this does not return a tag
/// attached to the table `report`. Ranger's `RangerTagEnricher` does the
/// opposite: it walks the hierarchy and emits ancestor tags, which Ranger
/// admits for a tag request. Every live assignment today is column-level, so
/// the difference is invisible in current data and would ship green.
///
/// SP4a Task 9 added this so `explain` could hand the resolved set to
/// `egret_perm::eval::evaluate`, which does no I/O of its own. **SP4d-2a Task 5
/// stopped doing that, and SP4d-2b Task 7 settled where each query belongs.**
/// `explain` evaluates the artifact each engine downloads, and the tag set it
/// hands the evaluator comes from [`resolved_tags_for_resource_via`] — which
/// rolls the hierarchy up and returns `tags.name` — gated on
/// [`tag_assignment_coordinates_via`] and `egret_perm_compile`'s
/// `stored_value_problem`. What `explain` uses THIS for is
/// the response's informational `tags` list — "what has egret got recorded on
/// exactly this resource" — and nothing else. The two must not be merged: this
/// one returns `tags.key`, which is a different value (live they differ for one
/// of three tags) and the engine matches the name.
pub async fn tag_keys_for_resource_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: uuid::Uuid,
    schema: &str,
    table: &str,
    column: &str,
) -> Result<Vec<String>, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, String>(
        r#"SELECT DISTINCT t.key
           FROM registry.catalogs c
           JOIN catalog_tag_assignments a ON a.warehouse_id = c.warehouse_id
           JOIN tags t ON t.id = a.tag_id
           WHERE c.id = $1 AND a.schema_name = $2 AND a.table_name = $3
             AND a.column_name = $4
           ORDER BY t.key"#,
    )
    .bind(catalog_id)
    .bind(schema)
    .bind(table)
    .bind(column)
    .fetch_all(&mut *conn)
    .await?)
}

/// One tag attachment the enricher would admit for a request: the tag's
/// **name** and the [`MatchType`] that attachment produces.
///
/// `name` is `tags.name`, not `tags.key`, and the distinction is the whole
/// point — see [`resolved_tags_for_resource_via`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTag {
    /// `tags.name` — the identifier the wire carries as the tag's `type`
    /// (`ServiceTags::from_assignments`) and therefore the one the engine
    /// matches tag policies on.
    pub name: String,
    /// The `MatchType` this attachment produces against the request,
    /// classified exactly as `RangerTagEnricher.findMatchingTags` does.
    pub match_type: MatchType,
}

/// The tag set an engine's `RangerTagEnricher` would resolve for ONE resource —
/// one entry per admitted attachment, carrying `tags.name` and the attachment's
/// [`MatchType`]. **This is the resolution an evaluator may act on**, unlike
/// [`tag_keys_for_resource_via`], which is exact-match attribution only.
///
/// `column = ""` in the REQUEST position means the request names the table
/// itself, mirroring migration 0081's storage convention.
///
/// # The rule, measured
///
/// `RangerTagEnricher.findMatchingTags` builds a real
/// `RangerDefaultPolicyResourceMatcher` per tag service resource and calls
/// `getMatchType` with the **tag's** resource elements in the POLICY position
/// and the **query's** resource in the request position (measured from
/// `ranger-plugins-common` bytecode, byte-identical at 2.5.0 and 2.8.0). For a
/// request naming `catalog/schema/table/column`:
///
/// | tag attached at | MatchType | admitted |
/// |---|---|---|
/// | the exact resource the request names | `SELF` | yes |
/// | the table, request names a column | `ANCESTOR` | yes |
/// | deeper than the request | `DESCENDANT` | **no** |
///
/// # Why two levels and not the enricher's four
///
/// Schema- and catalog-level attachments also yield `ANCESTOR`, and this
/// function cannot produce one because **egret's storage cannot express one**:
/// `public.catalog_tag_assignments` (migration 0081) keys on
/// `(tag_id, warehouse_id, schema_name, table_name, column_name)` with
/// `column_name text NOT NULL DEFAULT ''` and the documented convention that
/// `''` means table-level, while `schema_name` and `table_name` are `NOT NULL`
/// with **no such convention**. So column- and table-level are the only levels
/// any row here can carry (SP4d-2b D2, the user's ruling: evaluate what egret
/// can express, and refuse other payload shapes by shape). Schema- and
/// catalog-level tagging is a separate feature — a migration, a writer and a UI.
///
/// # ⚠ The comparison is CASE-INSENSITIVE, because the matcher is
///
/// **SP4d-2b Task 7 fix round 1, and the review measured the fail-open.** This
/// predicate was an exact, case-SENSITIVE `=` on all three levels. The enricher's
/// comparison is not: all four of `trino.json`'s `catalog` / `schema` / `table` /
/// `column` resources declare `RangerDefaultResourceMatcher` with
/// `{"wildCard": true, "ignoreCase": true}`, and `ignoreCase` makes
/// `RangerDefaultResourceMatcher` compare with `StringUtils.equalsIgnoreCase`.
/// Measured on the real jar for the request `hive.sales.orders.email`: a row
/// stored `('SALES','ORDERS','')` is `ANCESTOR` and admitted, and
/// `('SALES','ORDERS','EMAIL')` is `SELF` and admitted — while this query
/// returned NEITHER. egret would have reported "no tags" for a resource the
/// engine is masking, and an empty set is the licence to skip the tag phase
/// entirely.
///
/// **And that class comes out of ordinary use, not out of malformed data.**
/// `ScopePicker` renders schema / table / column as free-text inputs and
/// `request_resource` inserts them verbatim, so a steward typing `SALES` /
/// `ORDERS` against rows stored `sales` / `orders` produced it with nothing
/// wrong anywhere. Folding here REPRODUCES the matcher rather than declining
/// around it, which is why this is a widened predicate and not another gate: a
/// gate would have answered "cannot decide" to a steward who typed in the wrong
/// case, for a question egret can answer.
///
/// `tests/ranger/src/test/java/EgretTagAttachmentMatchTypeTest.java` pins the
/// Ranger half — every row above, measured against the real
/// `RangerServiceResourceMatcher.getMatchType` — so the equivalence this
/// predicate claims is measured on one side rather than asserted on both.
///
/// # ⚠ WHERE the two folds diverge is measured, and it is 25 code points
///
/// `lower()` is collation-driven FULL-STRING Unicode case mapping; Java's
/// `equalsIgnoreCase` is a PER-CHARACTER `toUpperCase`/`toLowerCase` pair after a
/// length check. Two algorithms, so this predicate reproduces the matcher only
/// where they agree — and fix round 2 measured where that is, by sweeping every
/// defined code point in Unicode:
/// `tests/ranger/src/test/java/EgretCaseFoldAgreementTest.java` finds **25**
/// divergent code points (the Turkish `İ`/`ı`, the long `ſ`, Greek `Σ`/final sigma
/// and the Greek/Cyrillic variant letters, the micro sign, the ypogegrammeni
/// pair), and **no Vietnamese letter is among them** — nor German `ß`, nor
/// Cyrillic, nor the Kelvin sign.
///
/// `egret_perm_compile`'s `fold_divergent` declines the resolution for a value
/// carrying one of those 25, on EITHER side of the comparison (the pair
/// `("ſ","s")` disagrees, so an ASCII stored value with an odd request value is
/// the same fail-open). Everything else folds here. ⚠ Fix round 1 declined every
/// non-ASCII stored value instead, which in a Vietnamese-language deployment is
/// one tag on `người` refusing a whole catalog — an over-refusal resting on an
/// un-measured premise.
///
/// The Postgres half of that measurement is a STAND-IN (`String.toLowerCase(
/// Locale.ROOT)`) and is **unverified**, exactly as this predicate itself is: no
/// Postgres runs in that harness. A collation that tailors case (`tr_TR`) would
/// move `I`/`i` into the divergent set and is the one deployment where the set is
/// too narrow.
///
/// # Three things this gets right that are each a silent fail-open
///
/// 1. **It returns `tags.name`, never `tags.key`.** The engine matches tag
///    policies on the wire tag's `type`, which `ServiceTags::from_assignments`
///    fills from `tags.name`; `tag_keys_for_resource_via` returns `tags.key`,
///    and live the two differ for one of three tags (`sp4cgate` vs
///    `sp4cgate2`). Same shape, different identifier — which is why that
///    function stays on the attribution surface and one function must not
///    return both.
/// 2. **It does not dedupe by name.** `RangerTagForEval`'s `equals`/`hashCode`
///    include `matchType`, so one tag attached at BOTH levels is TWO entries in
///    Ranger's set — and MASK evaluators are ordered by
///    `MATCH_TYPE_COMPARATOR` (ascending `SELF < SELF_AND_ALL_DESCENDANTS <
///    ANCESTOR`), so collapsing them would silently reorder masks and change
///    the masking answer. The `DISTINCT` below is over `(name, is_self)` — the
///    LEVEL as a boolean, which is exactly `RangerTagForEval`'s own equality for
///    the two levels this storage can express — never over the name alone. ⚠ Fix
///    round 1 narrowed it from `(name, column_name)`: with the comparison folded,
///    two rows differing only in case are ONE member of Ranger's set, and keeping
///    the raw value in the key would have invented a second one.
/// 3. **A column attachment is EXCLUDED from a table-level request.** That is
///    `DESCENDANT`, which the enricher rejects; returning it would fail closed
///    on `explain` and still disagree with the engine.
///
/// Rows come back ordered by match type ascending then name — deterministic,
/// and in `MATCH_TYPE_COMPARATOR`'s direction. The evaluator still applies its
/// own ordering; this order exists so the answer is stable, not so a caller can
/// skip that.
pub async fn resolved_tags_for_resource_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: uuid::Uuid,
    schema: &str,
    table: &str,
    column: &str,
) -> Result<Vec<ResolvedTag>, PolicyStoreError> {
    // `lower(a.column_name) = lower($4)` is the SELF arm; `a.column_name = ''`
    // with a non-empty request column is the table-level ANCESTOR arm. A request
    // that names the table ($4 = '') collapses the two into the one SELF arm, and
    // a column attachment then matches NEITHER — that is the DESCENDANT
    // exclusion, and it is structural rather than a filter that could be
    // forgotten.
    //
    // **`is_self` is computed in SQL, not in Rust, and that is the point.** It
    // used to be `attached_column == column` out here, which was a SECOND
    // comparison of the same two values under a different rule — and the moment
    // the predicate folded case, the two disagreed: a row stored `EMAIL` matched
    // the SELF arm in SQL and was then labelled `Ancestor` in Rust, which is a
    // wrong match type and so a wrong mask ORDER. One comparison, one place.
    //
    // `DISTINCT` is over `(t.name, is_self)` — `RangerTagForEval`'s own equality
    // (see point 2 above) — and no longer over the raw `column_name`, because two
    // rows differing only in case are now ONE member of Ranger's set rather than
    // two.
    let rows = sqlx::query_as::<_, (String, bool)>(
        r#"SELECT DISTINCT t.name, (lower(a.column_name) = lower($4)) AS is_self
           FROM registry.catalogs c
           JOIN catalog_tag_assignments a ON a.warehouse_id = c.warehouse_id
           JOIN tags t ON t.id = a.tag_id
           WHERE c.id = $1
             AND lower(a.schema_name) = lower($2)
             AND lower(a.table_name) = lower($3)
             AND (lower(a.column_name) = lower($4) OR a.column_name = '')
           ORDER BY is_self DESC, t.name"#,
    )
    .bind(catalog_id)
    .bind(schema)
    .bind(table)
    .bind(column)
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(name, is_self)| ResolvedTag {
            name,
            match_type: if is_self {
                MatchType::Self_
            } else {
                // The only other row the query can return: `column_name = ''`
                // (table level) against a request naming a column.
                MatchType::Ancestor
            },
        })
        .collect())
}

/// Every distinct `(schema_name, table_name, column_name)` this catalog's
/// `catalog_tag_assignments` rows carry — the stored coordinates, with no ruling
/// applied.
///
/// **The ruling is `egret_perm_compile`'s `stored_value_problem`, in Rust, and
/// the split is deliberate.** Its one caller runs this BEFORE
/// [`resolved_tags_for_resource_via`] and declines the whole resolution while any
/// coordinate is outside the population that resolver reproduces, so the two
/// cannot come apart.
///
/// # ⚠ What this exists for: a MISSED row, not a returned one
///
/// **SP4d-2b Task 7.** [`resolved_tags_for_resource_via`] compares the three
/// levels for EQUALITY (case-folded since fix round 1). The enricher does not
/// compare at all — it builds a `RangerDefaultPolicyResourceMatcher` per tag
/// service resource and asks `getMatchType`, which walks the hierarchy and
/// answers `ANCESTOR` for a resource ABOVE the request. `ServiceTags::from_assignments`
/// omits an empty level, so a row stored `('sales','','')` ships as
/// `resourceElements = {catalog, schema}` — measured `ANCESTOR` of
/// `hive.sales.orders.email` on the real jar, and `findMatchingTags` admits
/// `ANCESTOR` at the default `ResourceMatchingScope.SELF`. That row can never come
/// back from an equality predicate on `table_name`. So egret would hand the
/// evaluator a set MISSING a tag the engine enforces, and an empty set is the
/// licence to skip the tag phase entirely: a wrong confident verdict.
///
/// **The missed row is invisible to the resolver's RESULT** — nothing in the
/// returned set says a broader row was skipped — so no inspection of that set can
/// detect it. It has to be detected on the stored data, which is what this reads.
///
/// Such rows are writable today: `egret_bi::catalog_tags`' `assign_resource_tag`
/// validates nothing, migration 0081 has no `CHECK`, and `NOT NULL` does not
/// exclude `''`. One `CHECK` would retire both this read and its ruling — see
/// open item B of the SP4d-2b plan. Unrepresentable beats unforgettable.
///
/// # ⚠ Why the coordinates and not a `count(*)` with the predicate in SQL
///
/// **Fix round 1.** It WAS a `count(*)` whose `WHERE` clause carried the rule,
/// and the rule was wrong: it covered the empty-level class and missed the
/// wildcard and differing-case classes, both of which the review then measured as
/// the same fail-open. A predicate in SQL cannot be unit-tested or mutated
/// anywhere this branch can run — the shared test template is ahead of this
/// branch's migrations — so the miss could not have been caught by any test
/// either. The rule is now one pure Rust function with the enumeration beside it
/// and a test per class; this query returns facts.
///
/// The cost is one row per distinct tagged `(schema, table, column)` in the
/// catalog's warehouse, once per `explain` request (not per profile). `explain` is
/// an interactive admin read, and `tagged_columns_for_service_via` already ships
/// per-column rows on the compile path.
///
/// Scoped exactly as the resolver is — `a.warehouse_id = c.warehouse_id` for
/// `c.id = $1` — so a coordinate this returns is one that query could have
/// matched, and a row in another warehouse (whose service resource names another
/// catalog, never an ancestor of this one) is neither returned here nor there.
pub async fn tag_assignment_coordinates_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: uuid::Uuid,
) -> Result<Vec<(String, String, String)>, PolicyStoreError> {
    Ok(sqlx::query_as::<_, (String, String, String)>(
        r#"SELECT DISTINCT a.schema_name, a.table_name, a.column_name
           FROM registry.catalogs c
           JOIN catalog_tag_assignments a ON a.warehouse_id = c.warehouse_id
           WHERE c.id = $1
           ORDER BY a.schema_name, a.table_name, a.column_name"#,
    )
    .bind(catalog_id)
    .fetch_all(&mut *conn)
    .await?)
}

/// `tags.key` for each of `tag_ids`, as `(id, key)` pairs — the reverse of the
/// lookup every other reader here needs.
///
/// **SP4d-2a Task 5.** A compiled clearance policy's NAME carries the
/// `tags.id` it was minted from (`perm-clearance-{tag_id}[-{locator}]`), and
/// `egret_perm_compile::attribute_policy_name` hands that bare uuid back
/// without touching the database. `explain` has to turn it into something a
/// steward recognises, and into the `required_roles` the gate declares — which
/// [`clearance_gates_for_tags_via`] answers by KEY, not by id. So this is the
/// one hop between them.
///
/// It returns `tags.key`, not `tags.name`: `key` is the stable identifier the
/// rest of this module's tag surface is addressed by
/// ([`tag_keys_for_resource_via`], [`clearance_gates_for_tags_via`]), and the
/// two columns are different values — `tag_name_for_key_via` exists precisely
/// because the display name has to be looked up separately.
pub async fn tag_keys_by_ids_via(
    conn: &mut sqlx::PgConnection,
    tag_ids: &[uuid::Uuid],
) -> Result<Vec<(uuid::Uuid, String)>, PolicyStoreError> {
    if tag_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(
        sqlx::query_as::<_, (uuid::Uuid, String)>(r#"SELECT id, key FROM tags WHERE id = ANY($1)"#)
            .bind(tag_ids)
            .fetch_all(&mut *conn)
            .await?,
    )
}

/// The clearance gate for each of `tag_keys` that `egret_perm::evaluate` can
/// actually agree with the compiled engine about (SP4c Task 4/D4): one
/// `(tag key, required_roles)` pair per gated tag whose `required_roles` is
/// non-empty. A `required_groups`-only gate is deliberately excluded — D2/D7
/// measured that a group name matches nobody in the engine, so
/// `egret_perm_compile::clearance` compiles it to NO deny at all, and
/// `explain` reporting a denial for it would disagree with the engine in the
/// direction opposite the bug SP4c Task 4 fixes.
pub async fn clearance_gates_for_tags_via(
    conn: &mut sqlx::PgConnection,
    tag_keys: &[String],
) -> Result<Vec<(String, Vec<String>)>, PolicyStoreError> {
    if tag_keys.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, (String, Vec<String>)>(
        r#"SELECT t.key, cp.required_roles
             FROM classification_policies cp
             JOIN tags t ON t.id = cp.tag_id
            WHERE t.key = ANY($1) AND cardinality(cp.required_roles) > 0"#,
    )
    .bind(tag_keys)
    .fetch_all(&mut *conn)
    .await?)
}

/// Connection-generic core of [`PolicyStore::tagged_columns_for_service`] —
/// same rationale as [`get_service_by_id_via`].
pub async fn tagged_columns_for_service_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    tag_key: &str,
) -> Result<Vec<TaggedColumnRow>, PolicyStoreError> {
    Ok(sqlx::query_as::<_, TaggedColumnRow>(
        r#"SELECT DISTINCT c.name AS catalog, a.schema_name, a.table_name, a.column_name
           FROM catalog_tag_assignments a
           JOIN tags t ON t.id = a.tag_id
           JOIN registry.catalogs c ON c.warehouse_id = a.warehouse_id
           JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id
           WHERE e.authz_service_id = $1 AND t.key = $2 AND a.column_name <> ''
           ORDER BY c.name, a.schema_name, a.table_name, a.column_name"#,
    )
    .bind(service_id)
    .bind(tag_key)
    .fetch_all(&mut *conn)
    .await?)
}

/// Connection-generic core of [`PolicyStore::tag_name_for_key`] — same
/// rationale as [`get_service_by_id_via`].
pub async fn tag_name_for_key_via(
    conn: &mut sqlx::PgConnection,
    key: &str,
) -> Result<Option<String>, PolicyStoreError> {
    Ok(
        sqlx::query_scalar::<_, String>(r#"SELECT name FROM tags WHERE key = $1"#)
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// The `(id, name)` of each catalog in `ids` that still exists, in one round
/// trip.
///
/// **Why a batch lookup and not `catalogs_for_service_via`.** That one answers
/// "which catalogs does this profile reach", which is a different question and
/// needs an engine binding; a grant or rule names a catalog whether or not any
/// engine is attached to it yet. The perm read surface uses this to put the
/// catalog's NAME on every grant and rule it returns (SP4b-2 finding 1): a
/// client holding `policy:read` alone cannot call the catalog registry, so a
/// client-side id→name lookup degraded the one column those pages exist to
/// show into a raw uuid. The names are resolved here, where no extra
/// permission is involved.
///
/// An id with no row is simply absent from the result (a catalog deleted
/// between the grant read and this call) — never an error, and never a
/// fabricated name.
pub async fn catalog_names_for_ids_via(
    conn: &mut sqlx::PgConnection,
    ids: &[uuid::Uuid],
) -> Result<Vec<CatalogRow>, PolicyStoreError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, CatalogRow>(
        r#"SELECT id, name FROM registry.catalogs WHERE id = ANY($1)"#,
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await?)
}

/// The `(key, name)` of each tag in `keys` that exists — the batch form of
/// [`tag_name_for_key_via`], for the same reason
/// [`catalog_names_for_ids_via`] exists. `tags.name` is the display name;
/// `tags.key` is what a rule stores (D6), and the two differ (`pii` / `PII`).
pub async fn tag_names_for_keys_via(
    conn: &mut sqlx::PgConnection,
    keys: &[String],
) -> Result<Vec<(String, String)>, PolicyStoreError> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    Ok(
        sqlx::query_as::<_, (String, String)>(r#"SELECT key, name FROM tags WHERE key = ANY($1)"#)
            .bind(keys)
            .fetch_all(&mut *conn)
            .await?,
    )
}

/// One registry catalog: `(id, name)`.
#[derive(Debug, sqlx::FromRow)]
pub struct CatalogRow {
    pub id: uuid::Uuid,
    pub name: String,
}

/// One tagged column, Ranger-facing names only (see `tagged_columns_for_service`).
#[derive(Debug, sqlx::FromRow)]
pub struct TaggedColumnRow {
    pub catalog: String,
    pub schema_name: String,
    pub table_name: String,
    pub column_name: String,
}

#[derive(Debug, sqlx::FromRow)]
pub struct PolicyRow {
    pub id: i64,
    pub service_id: i64,
    pub name: String,
    pub policy_jsonb: Value,
    pub enabled: bool,
    pub version: i64,
}

impl PolicyStore {
    /// Create a policy under a service and bump policy_version atomically. Returns the new policy id.
    pub async fn create_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<i64, PolicyStoreError> {
        // Fix round 3: reserved-namespace check first — pure, no DB needed,
        // and applies regardless of this service's `origin` (see
        // `refuse_reserved_name`'s doc comment for why `manual` is not
        // exempt).
        refuse_reserved_name(name)?;
        let mut tx = self.db.pool.begin().await?;
        if let Err(e) = refuse_if_generated(&mut tx, service_id).await {
            tx.rollback().await?;
            return Err(e);
        }
        let id: i64 = sqlx::query_scalar::<_, i64>(
            r#"INSERT INTO policy.authz_policies (service_id, name, policy_jsonb)
               VALUES ($1,$2,$3) RETURNING id"#,
        )
        .bind(service_id)
        .bind(name)
        .bind(policy)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                PolicyStoreError::AlreadyExists(name.into())
            }
            _ => PolicyStoreError::Db(e),
        })?;
        sqlx::query(
            r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
        )
        .bind(service_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Insert a policy, or update its body in place if `(service_id, name)`
    /// already exists, bumping the policy's `version` and the service's
    /// `policy_version` atomically. Returns the policy id.
    ///
    /// **Guarded by the reserved-name check and by nothing else, deliberately**
    /// (whole-branch review, Important 6). [`refuse_if_generated`] must NOT be
    /// added here: this method's only caller is
    /// `default_policies::seed_default_trino_policies`, `engines.rs` propagates
    /// a seed failure as a 400, and a profile the compiler has claimed is
    /// exactly the one an engine update would then be unable to save — the
    /// exemption is load-bearing, not an oversight. [`refuse_reserved_name`] is
    /// pure and name-only, so it cannot fail that seed (its
    /// `egret-admin-*` names collide with nothing), and it closes the one way
    /// this path could drift: a future second caller passing a
    /// `perm-grant-`/`perm-rule-` name would silently overwrite compiled
    /// output, which the next recompile restores — an edit that appears to
    /// work and then vanishes.
    pub async fn upsert_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<i64, PolicyStoreError> {
        refuse_reserved_name(name)?;
        let mut tx = self.db.pool.begin().await?;
        let id: i64 = sqlx::query_scalar::<_, i64>(
            r#"INSERT INTO policy.authz_policies (service_id, name, policy_jsonb)
               VALUES ($1,$2,$3)
               ON CONFLICT (service_id, name) DO UPDATE
                 SET policy_jsonb = EXCLUDED.policy_jsonb,
                     version = policy.authz_policies.version + 1,
                     updated_at = now()
               RETURNING id"#,
        )
        .bind(service_id)
        .bind(name)
        .bind(policy)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query(
            r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
        )
        .bind(service_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    pub async fn update_policy(
        &self,
        id: i64,
        name: &str,
        policy: &Value,
        enabled: bool,
    ) -> Result<(), PolicyStoreError> {
        let mut tx = self.db.pool.begin().await?;
        // Fetch the policy's CURRENT name — ownership (Task 8 fix round 2,
        // I-1) is decided by what this policy IS today, not by the new name
        // the caller is trying to write.
        let row: Option<(i64, String)> = sqlx::query_as::<_, (i64, String)>(
            r#"SELECT service_id, name FROM policy.authz_policies WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((service_id, current_name)) = row else {
            tx.rollback().await?;
            return Err(PolicyStoreError::NotFound(format!("policy {id}")));
        };
        if let Err(e) = refuse_if_compiler_owned(&mut tx, service_id, &current_name).await {
            tx.rollback().await?;
            return Err(e);
        }
        // Fix round 3: reserved-namespace check on the NEW/target name, AFTER
        // the ownership check above — an already-owned policy (caught above,
        // whatever `name` the caller resent) gets the ownership-specific
        // message ("this is compiler-owned, edit the grant/rule"), which is
        // both more accurate and less confusing than "rename this" for a
        // name the compiler itself is the one using legitimately. This check
        // is what catches the round-2 finding's OTHER vector instead: a
        // human renaming an ordinary (not-yet-owned) policy INTO the
        // compiler's namespace, in ANY profile including `manual` — where
        // the ownership check above does not fire at all (it requires
        // `origin = 'generated'`, and the whole point of the round-2 finding
        // was that a `manual` profile is not yet flagged as anything).
        if let Err(e) = refuse_reserved_name(name) {
            tx.rollback().await?;
            return Err(e);
        }
        sqlx::query(
            r#"UPDATE policy.authz_policies SET name=$2, policy_jsonb=$3, enabled=$4, version=version+1, updated_at=now() WHERE id=$1"#,
        )
        .bind(id)
        .bind(name)
        .bind(policy)
        .bind(enabled)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
        )
        .bind(service_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Flip a policy's enabled flag without rewriting its body; bumps versions.
    pub async fn set_policy_enabled(&self, id: i64, enabled: bool) -> Result<(), PolicyStoreError> {
        let mut tx = self.db.pool.begin().await?;
        let row: Option<(i64, String)> = sqlx::query_as::<_, (i64, String)>(
            r#"SELECT service_id, name FROM policy.authz_policies WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((service_id, current_name)) = row else {
            tx.rollback().await?;
            return Err(PolicyStoreError::NotFound(format!("policy id {id}")));
        };
        if let Err(e) = refuse_if_compiler_owned(&mut tx, service_id, &current_name).await {
            tx.rollback().await?;
            return Err(e);
        }
        sqlx::query(
            r#"UPDATE policy.authz_policies SET enabled=$2, version=version+1, updated_at=now() WHERE id=$1"#,
        )
        .bind(id)
        .bind(enabled)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
        )
        .bind(service_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_policy(&self, id: i64) -> Result<(), PolicyStoreError> {
        let mut tx = self.db.pool.begin().await?;
        // Look the policy up BEFORE deleting — Task 8's guard must refuse the
        // delete outright, not delete-then-refuse (which would leave the
        // policy gone despite the 409).
        let row: Option<(i64, String)> = sqlx::query_as::<_, (i64, String)>(
            r#"SELECT service_id, name FROM policy.authz_policies WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((service_id, current_name)) = row else {
            tx.rollback().await?;
            return Err(PolicyStoreError::NotFound(format!("policy {id}")));
        };
        if let Err(e) = refuse_if_compiler_owned(&mut tx, service_id, &current_name).await {
            tx.rollback().await?;
            return Err(e);
        }
        sqlx::query(r#"DELETE FROM policy.authz_policies WHERE id = $1"#)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            r#"UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() WHERE id = $1"#,
        )
        .bind(service_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn get_policy(&self, id: i64) -> Result<PolicyRow, PolicyStoreError> {
        sqlx::query_as::<_, PolicyRow>(
            r#"SELECT id, service_id, name, policy_jsonb, enabled, version FROM policy.authz_policies WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| PolicyStoreError::NotFound(format!("policy {id}")))
    }

    /// All policies for a service (enabled and disabled), newest first — the
    /// admin list view. Returns typed rows; unlike `enabled_policies_for_service`
    /// this does not inject id/version into the body (the caller/summary does that).
    pub async fn list_policies(&self, service_id: i64) -> Result<Vec<PolicyRow>, PolicyStoreError> {
        Ok(sqlx::query_as::<_, PolicyRow>(
            r#"SELECT id, service_id, name, policy_jsonb, enabled, version
               FROM policy.authz_policies WHERE service_id = $1 ORDER BY id DESC"#,
        )
        .bind(service_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Enabled policies of a service (for download assembly).
    pub async fn enabled_policies_for_service(
        &self,
        service_id: i64,
    ) -> Result<Vec<Value>, PolicyStoreError> {
        // Inject the DB-assigned numeric `id`, `version`, and `isEnabled` into each policy.
        // The Ranger policy engine NPEs on a policy whose `id` is null, and clients only send
        // these on the response/CRUD path — the stored body keeps the client's original JSON.
        let rows = sqlx::query_as::<_, (i64, i64, bool, Value)>(
            r#"SELECT id, version, enabled, policy_jsonb FROM policy.authz_policies
               WHERE service_id = $1 AND enabled = true ORDER BY id"#,
        )
        .bind(service_id)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, version, enabled, mut p)| {
                if let Value::Object(m) = &mut p {
                    m.insert("id".into(), Value::from(id));
                    m.insert("version".into(), Value::from(version));
                    m.insert("isEnabled".into(), Value::from(enabled));
                }
                p
            })
            .collect())
    }
}
