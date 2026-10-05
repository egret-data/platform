//! Classification tag repository (Access P3.1) — the metadata foundation for
//! ABAC. A `tags` catalog + `tag_assignments` (which objects carry which tags).
//!
//! METADATA ONLY: no authorization decision reads these tables in this phase.
//! Enforcement (tags feeding access decisions) is a later phase — this repo just
//! stores and lists the classification metadata.

use egret_auth::events::RolesChanged;
use egret_events::EventBus;
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::BiError;

#[derive(Clone)]
pub struct TagRepo {
    db: Db,
}

/// A classification tag plus its live assignment count (0 unless `list`/`get`
/// computed it via the LEFT JOIN).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Tag {
    pub id: Uuid,
    pub key: String,
    pub name: String,
    pub description: String,
    pub color: String,
    /// Number of `tag_assignments` rows referencing this tag.
    pub assignment_count: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// The tag columns joined with a computed assignment count. `created_by` is
/// intentionally omitted from the domain type (audit-only column).
const SELECT_WITH_COUNT: &str = "SELECT t.id, t.key, t.name, t.description, t.color, \
     COUNT(ta.tag_id) AS assignment_count, t.created_at \
     FROM tags t LEFT JOIN tag_assignments ta ON ta.tag_id = t.id";

fn map_conflict_err(e: sqlx::Error) -> BiError {
    if let sqlx::Error::Database(ref dbe) = e {
        if dbe.is_unique_violation() {
            return BiError::Conflict("a tag with that key already exists".into());
        }
    }
    BiError::Db(e)
}

/// A tag key must be a stable, URL/identifier-safe slug: non-empty, lowercase
/// ascii alphanumerics plus `:`, `-`. Keeps keys usable as ABAC selector tokens
/// later without escaping.
fn validate_key(key: &str) -> Result<(), BiError> {
    if key.is_empty() {
        return Err(BiError::BadRequest("tag key must not be empty".into()));
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == ':' || c == '-')
    {
        return Err(BiError::BadRequest(format!(
            "tag key must be lowercase ascii alphanumerics, ':' or '-': {key}"
        )));
    }
    Ok(())
}

impl TagRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Create a tag. Validates `key`; maps the unique-key violation to `Conflict`.
    pub async fn create(
        &self,
        key: &str,
        name: &str,
        description: &str,
        color: &str,
        created_by: Option<Uuid>,
    ) -> Result<Tag, BiError> {
        validate_key(key)?;
        // RETURNING can't carry the join-computed count, so INSERT then re-select
        // (a freshly created tag always has assignment_count = 0).
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO tags (key, name, description, color, created_by) \
             VALUES ($1, $2, $3, $4, $5) RETURNING id",
        )
        .bind(key)
        .bind(name)
        .bind(description)
        .bind(color)
        .bind(created_by)
        .fetch_one(&self.db.pool)
        .await
        .map_err(map_conflict_err)?;
        self.get(id)
            .await?
            .ok_or_else(|| BiError::NotFound(format!("tag {id}")))
    }

    /// All tags, each with its assignment count, ordered by key.
    pub async fn list(&self) -> Result<Vec<Tag>, BiError> {
        let sql = format!("{SELECT_WITH_COUNT} GROUP BY t.id ORDER BY t.key");
        Ok(sqlx::query_as::<_, Tag>(sqlx::AssertSqlSafe(sql))
            .fetch_all(&self.db.pool)
            .await?)
    }

    /// One tag by id (with its assignment count), or `None`.
    pub async fn get(&self, id: Uuid) -> Result<Option<Tag>, BiError> {
        let sql = format!("{SELECT_WITH_COUNT} WHERE t.id = $1 GROUP BY t.id");
        Ok(sqlx::query_as::<_, Tag>(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?)
    }

    /// Update the mutable fields; `None` leaves a field unchanged (COALESCE). The
    /// key is immutable (it's the stable selector token). Returns the updated tag
    /// or `None` if no such id.
    ///
    /// **A rename is a recompile trigger, in the same transaction as the
    /// rename** (SP4a whole-branch review, Important 3 — D6 promised "a rename
    /// recompiles instead of breaking" and nothing implemented it). D6 resolves
    /// `tags.key` → `tags.name` at compile time because the Ranger tag plugin
    /// matches on the NAME, so a compiled Trino tag policy carries the name as
    /// it stood at compile time while [`Self::get`]'s sibling `download_tags`
    /// serves `t.name` live. After a rename the two agree only until the next
    /// plugin restart or `lastKnownVersion=-1` poll — at which point the tag
    /// map carries the NEW name and the policy still names the OLD one, **the
    /// mask stops applying, and the column returns real values**. Fail-open,
    /// with an unrelated restart as the trigger.
    ///
    /// Both halves are needed and neither is sufficient: the recompile moves
    /// the POLICIES to the new name, and the `tag_version` bump is what stops
    /// the tag plugin being answered `304` and serving the old map against
    /// them. The order and the shape are `CatalogTagRepo::assign_catalog`'s —
    /// write, bump, then lock the expanded service set ascending and recompile
    /// on this same transaction — so the two tag-write paths cannot drift.
    pub async fn update(
        &self,
        id: Uuid,
        name: Option<&str>,
        description: Option<&str>,
        color: Option<&str>,
    ) -> Result<Option<Tag>, BiError> {
        let mut tx = self.db.pool.begin().await?;
        // The key (for "is any rule compiled from this name?") and the current
        // name (for "is this actually a rename?"), read inside the transaction
        // that is about to change it.
        let before: Option<(String, String)> =
            sqlx::query_as("SELECT key, name FROM tags WHERE id = $1")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((key, old_name)) = before else {
            return Ok(None);
        };
        let affected = sqlx::query(
            "UPDATE tags SET \
                name = COALESCE($2, name), \
                description = COALESCE($3, description), \
                color = COALESCE($4, color) \
             WHERE id = $1",
        )
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(color)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if affected == 0 {
            return Ok(None);
        }
        if name.is_some_and(|n| n != old_name) {
            Self::recompile_for_rename_in_tx(&mut tx, id, &key).await?;
        }
        tx.commit().await?;
        self.get(id).await
    }

    /// The recompile half of [`Self::update`]'s rename. Runs on the caller's
    /// transaction so a crash cannot leave the new name committed with the old
    /// one still compiled — there is only one commit, the same property
    /// `CatalogTagRepo::recompile_bound_services_in_tx` exists for.
    ///
    /// A recompile FAILURE fails the rename, deliberately: the alternative is a
    /// committed rename with policies still naming the old tag, which is the
    /// fail-open state this whole function exists to prevent.
    async fn recompile_for_rename_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        tag_id: Uuid,
        key: &str,
    ) -> Result<(), BiError> {
        // The tag MAP the plugin downloads carries `t.name` too, so every
        // service serving a warehouse that carries this tag must stop 304-ing.
        let warehouses: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT warehouse_id FROM catalog_tag_assignments WHERE tag_id = $1 \
             ORDER BY warehouse_id",
        )
        .bind(tag_id)
        .fetch_all(&mut **tx)
        .await?;
        for warehouse_id in warehouses {
            egret_catalog_registry::bump_bound_tag_versions(tx, warehouse_id).await?;
        }
        let services = egret_perm_compile::affected_services_for_tag_rename(tx, key).await?;
        egret_perm_compile::lock_services_in_tx(tx, &services).await?;
        for service_id in services {
            let summary = egret_perm_compile::recompile_service_in_tx(tx, service_id).await?;
            if !summary.skipped.is_empty() {
                tracing::warn!(
                    tag = %key,
                    service_id,
                    skipped_count = summary.skipped.len(),
                    "a tag rename's recompile skipped some grants/rules for this service"
                );
            }
        }
        Ok(())
    }

    /// Delete a tag. `tag_assignments` and `catalog_tag_assignments` cascade
    /// (FK `ON DELETE CASCADE`, migrations 0080/0081).
    ///
    /// **A tag with no `perm.rules` row selecting it USED to feed no compiled
    /// policy, and SP4c ended that.** The pre-SP4c justification — that
    /// `compile_desired` only resolves a tag's columns for a key a LIVE rule
    /// names, so cascading the assignments away creates no staleness to
    /// recompile out of — is no longer true, and the body below says so three
    /// lines down: `classification_policies` cascades with the row too, so
    /// deleting a GATED tag removes a clearance that IS compiled, whether or not
    /// any rule selects the tag. The recompile in the body is therefore not
    /// optional, and this doc no longer argues the opposite of it.
    ///
    /// A tag a `perm.rules` row still SELECTS (`perm.rules.tag_key REFERENCES
    /// tags(key)`, migration 0154, no `ON DELETE` clause — the default
    /// RESTRICT) cannot be deleted at all: Postgres refuses the DELETE with a
    /// foreign-key violation rather than silently cascading the tag's own
    /// row away out from under a compiled StarRocks masking policy that
    /// still names it (SP4a Task 7 fix round 1 — verified against a live
    /// `egret_test_template` connection, not assumed: inserting a tag, a
    /// `perm.rules` row selecting its key, and then attempting `DELETE FROM
    /// tags` raises `rules_tag_key_fkey`). Mapped to a clear `Conflict`
    /// instead of the generic 500 a raw `sqlx::Error` would otherwise render
    /// as — the caller should be told to delete/retarget the rule first, not
    /// shown an opaque internal error.
    pub async fn delete(&self, id: Uuid) -> Result<(), BiError> {
        // **SP4c Task 3: deleting a tag is a write to input 1 AND to input 2.**
        // `classification_policies` and `catalog_tag_assignments` both cascade
        // away with the row (FKs, migrations 0081/0082), so a GATED tag's delete
        // removes its clearance — and before SP4c that reached nothing, because
        // a tag with no `perm.rules` row selecting it fed no compiled policy and
        // this function therefore recompiled nothing. A clearance changes that:
        // the compiled deny survives the tag, is served to every plugin, and
        // nothing would ever sweep it again. That is SP4a Task 7's "a deleted
        // rule's policy is enforced forever" Critical, one input over — and a
        // clearance that cannot be lifted by deleting the tag is the one-way
        // door SP4a had to reopen once already.
        //
        // In the SAME transaction, like every other recompile on this seam: the
        // alternative is a committed delete with a deny nobody can remove.
        let mut tx = self.db.pool.begin().await?;
        let affected = sqlx::query("DELETE FROM tags WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                if let sqlx::Error::Database(ref dbe) = e {
                    if dbe.is_foreign_key_violation() {
                        return BiError::Conflict(
                            "this tag is still selected by a permission rule (perm.rules) — \
                             delete or retarget that rule first"
                                .into(),
                        );
                    }
                }
                BiError::Db(e)
            })?
            .rows_affected();
        // No row deleted is no state change: recompiling would be a lock
        // acquisition and a full compile for an idempotent no-op.
        if affected > 0 {
            // The skips are logged inside; a 204 has no body to carry them.
            let _ = ClassificationPolicyRepo::recompile_for_clearance_in_tx(&mut tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Assign a tag to an object. Idempotent — a re-assign is a no-op (ON CONFLICT
    /// DO NOTHING on the composite primary key).
    pub async fn assign(
        &self,
        tag_id: Uuid,
        object_type: &str,
        object_id: Uuid,
        assigned_by: Option<Uuid>,
    ) -> Result<(), BiError> {
        sqlx::query(
            "INSERT INTO tag_assignments (tag_id, object_type, object_id, assigned_by) \
             VALUES ($1, $2, $3, $4) ON CONFLICT (tag_id, object_type, object_id) DO NOTHING",
        )
        .bind(tag_id)
        .bind(object_type)
        .bind(object_id)
        .bind(assigned_by)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Remove a tag from an object (idempotent — deletes zero or one row).
    pub async fn unassign(
        &self,
        tag_id: Uuid,
        object_type: &str,
        object_id: Uuid,
    ) -> Result<(), BiError> {
        sqlx::query(
            "DELETE FROM tag_assignments \
             WHERE tag_id = $1 AND object_type = $2 AND object_id = $3",
        )
        .bind(tag_id)
        .bind(object_type)
        .bind(object_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// The tags carried by one object, ordered by key. `assignment_count` here is
    /// the tag's global count (LEFT JOIN over all assignments), not per-object.
    pub async fn tags_for_object(
        &self,
        object_type: &str,
        object_id: Uuid,
    ) -> Result<Vec<Tag>, BiError> {
        let sql = format!(
            "{SELECT_WITH_COUNT} \
             JOIN tag_assignments this ON this.tag_id = t.id \
                 AND this.object_type = $1 AND this.object_id = $2 \
             GROUP BY t.id ORDER BY t.key"
        );
        Ok(sqlx::query_as::<_, Tag>(sqlx::AssertSqlSafe(sql))
            .bind(object_type)
            .bind(object_id)
            .fetch_all(&self.db.pool)
            .await?)
    }
}

/// Store for **classification access policies** (Access P3.3 + P3.4): one row per
/// tag binding it to a set of clearance role NAMES and/or clearance GROUP ids.
/// Semantics: a principal is CLEARED for objects carrying the tag iff they hold
/// `>= 1` of `required_roles` **OR** belong to `>= 1` of `required_groups` (in
/// egret OIDC groups model departments). BOTH lists **empty** is NO gate (the tag
/// is informational only) — enforcement applies only when at least one is set.
///
/// The Cedar PDP `load_inner` consumes `list_policies` to compile one `forbid` per
/// gated tag. Mutations publish [`RolesChanged`] on the shared bus — the SAME
/// reload event the PDP subscribes to — so the compiled policy set stays fresh
/// with no extra plumbing, exactly as the role/permission CRUD does.
#[derive(Clone)]
pub struct ClassificationPolicyRepo {
    db: Db,
    bus: EventBus,
}

/// A clearance role name is spliced verbatim into a Cedar `forbid` string literal
/// by the PDP loader, so it MUST be a safe role-name slug: non-empty, lowercase
/// ascii alphanumerics plus `:`, `_`, `-` (matching egret role names like
/// `ws_viewer`, `data_steward`). This rejects `"`, `\`, `]`, spaces and `|`, which
/// would otherwise let a `tag:manage` caller inject/break the Cedar policy
/// (policy-string injection → forbid-parse failure → PDP reload DoS, or a
/// `|| true` payload disabling the gate).
fn validate_role_name(role: &str) -> Result<(), BiError> {
    let ok = !role.is_empty()
        && role
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, ':' | '_' | '-'));
    if !ok {
        return Err(BiError::BadRequest(format!(
            "invalid clearance role name (must be non-empty lowercase ascii \
             alphanumerics, ':', '_' or '-'): {role:?}"
        )));
    }
    Ok(())
}

impl ClassificationPolicyRepo {
    pub fn new(db: Db, bus: EventBus) -> Self {
        Self { db, bus }
    }

    /// UPSERT the required-role and required-group lists for `tag_id` (INSERT ...
    /// ON CONFLICT (tag_id) DO UPDATE). Publishes `RolesChanged` so the PDP
    /// reloads its policy set.
    ///
    /// Every entry in `required_roles` is validated against a safe role-name
    /// charset BEFORE the INSERT (so the CP handler and any other caller are all
    /// covered) — a single bad entry rejects the whole call with `BadRequest`,
    /// preventing Cedar policy-string injection in the PDP loader. `required_groups`
    /// are already-typed `Uuid`s (a `uuid[]` column can only hold valid uuids), so
    /// the parse-validation of the string→uuid boundary happens in the CRUD layer.
    ///
    /// CLEARANCE (Access P3.4): cleared = >=1 required ROLE OR >=1 required GROUP.
    /// BOTH lists empty = NO gate (informational tag only).
    ///
    /// Returns whatever the accompanying recompile could NOT express — a
    /// group-only gate, or a Trino profile with no usable tag service — so the
    /// endpoint can tell the author, instead of answering `{"ok": true}` over a
    /// clearance no engine will ever enforce.
    pub async fn set_policy(
        &self,
        tag_id: Uuid,
        required_roles: &[String],
        required_groups: &[Uuid],
        by: Option<Uuid>,
    ) -> Result<Vec<egret_perm_compile::SkippedItem>, BiError> {
        for role in required_roles {
            validate_role_name(role)?;
        }
        let mut tx = self.db.pool.begin().await?;
        sqlx::query(
            "INSERT INTO classification_policies \
                 (tag_id, required_roles, required_groups, created_by, updated_at) \
             VALUES ($1, $2, $3, $4, now()) \
             ON CONFLICT (tag_id) DO UPDATE SET \
                 required_roles = EXCLUDED.required_roles, \
                 required_groups = EXCLUDED.required_groups, \
                 updated_at = now()",
        )
        .bind(tag_id)
        .bind(required_roles)
        .bind(required_groups)
        .bind(by)
        .execute(&mut *tx)
        .await?;
        let skipped = Self::recompile_for_clearance_in_tx(&mut tx).await?;
        tx.commit().await?;
        self.bus.publish(RolesChanged);
        Ok(skipped)
    }

    /// Remove a tag's classification policy (idempotent — deletes zero or one row).
    /// Publishes `RolesChanged` so the PDP reloads its policy set. Returns what
    /// the recompile could not express, for the same reason [`Self::set_policy`]
    /// does.
    pub async fn clear_policy(
        &self,
        tag_id: Uuid,
    ) -> Result<Vec<egret_perm_compile::SkippedItem>, BiError> {
        let mut tx = self.db.pool.begin().await?;
        sqlx::query("DELETE FROM classification_policies WHERE tag_id = $1")
            .bind(tag_id)
            .execute(&mut *tx)
            .await?;
        let skipped = Self::recompile_for_clearance_in_tx(&mut tx).await?;
        tx.commit().await?;
        self.bus.publish(RolesChanged);
        Ok(skipped)
    }

    /// **SP4c Task 3, input 1 of three: the tag's own GATE.** Recompile every
    /// profile a clearance can reach, on the CALLER's transaction — so the gate
    /// and the engine policy it compiles to commit together or not at all.
    ///
    /// Until SP4c a `classification_policies` write reached the data plane
    /// through nothing at all: the gate was enforced only by Cedar, on
    /// control-plane object actions, and the compiler had never read the table.
    /// Now that a gate compiles to an engine DENY, a write without its recompile
    /// is the SP4a defect class this programme shipped four separate times — and
    /// in both directions here. A missed `set_policy` leaves the classified
    /// column readable by everyone while the UI shows the clearance as active; a
    /// missed `clear_policy` leaves a deny nobody can lift, which is the one-way
    /// door SP4a already had to reopen once.
    ///
    /// **A recompile FAILURE fails the write**, deliberately, exactly as
    /// `CatalogTagRepo::recompile_bound_services_in_tx` does: a stored-but-
    /// uncompiled clearance is worse than a rejected one, because egret's UI
    /// shows it as enforced.
    ///
    /// `recompile_services_in_tx` takes the locks itself, expanded with each
    /// profile's TAG service and ASCENDING (`lock_services_in_tx`) — the
    /// ordering that turns two overlapping recompiles into a queue instead of a
    /// deadlock. Nothing is locked here by hand, so there is no second ordering
    /// to keep in step with it.
    /// **The skips are RETURNED, not only logged (SP4c review I5).** A clearance
    /// authored on a Trino profile with no usable `tagService` compiles nothing
    /// at all, and a group-only gate compiles nothing on any engine — and the
    /// endpoint used to answer `{"ok": true}` either way, so the UI showed the
    /// clearance as active while the data plane had never heard of it. That is
    /// the same failure the compiler already refuses elsewhere: `SkipReport` is
    /// on the perm mutation responses precisely because "a create that succeeded
    /// but compiled nothing is a fact the author needs at authoring time, not
    /// one a steward discovers from a query that returns no rows". The clearance
    /// author gets the same courtesy. Still logged, because a skip on the tag
    /// DELETE path has no response to ride on.
    pub(crate) async fn recompile_for_clearance_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<Vec<egret_perm_compile::SkippedItem>, BiError> {
        let services = egret_perm_compile::affected_services_for_classification(tx).await?;
        let mut skipped: Vec<egret_perm_compile::SkippedItem> = Vec::new();
        for summary in egret_perm_compile::recompile_services_in_tx(tx, &services).await? {
            if !summary.skipped.is_empty() {
                tracing::warn!(
                    skipped_count = summary.skipped.len(),
                    "a clearance write's recompile skipped some items — a group-only gate, or a \
                     Trino profile with no tag service"
                );
            }
            // **Deduplicated across profiles.** A clearance is
            // installation-wide, so the SAME unexpressible gate is reported
            // once per profile in the affected set — and an author reading "3
            // skipped" for one gate would reasonably think three different
            // things went wrong. The triple is the identity: two profiles that
            // skipped the same tag for DIFFERENT reasons (one group-only, one
            // with no tag service) stay two entries, because those are two
            // facts.
            for item in &summary.skipped {
                if !skipped.iter().any(|s: &egret_perm_compile::SkippedItem| {
                    s.id == item.id && s.source_kind == item.source_kind && s.reason == item.reason
                }) {
                    skipped.push(item.clone());
                }
            }
        }
        Ok(skipped)
    }

    /// The required-role and required-group lists for one tag, or `None` if no
    /// policy row exists. (An existing row with both arrays empty is
    /// `Some((vec![], vec![]))` — a NO-gate policy.)
    pub async fn get_policy(
        &self,
        tag_id: Uuid,
    ) -> Result<Option<(Vec<String>, Vec<Uuid>)>, BiError> {
        Ok(sqlx::query_as::<_, (Vec<String>, Vec<Uuid>)>(
            "SELECT required_roles, required_groups \
             FROM classification_policies WHERE tag_id = $1",
        )
        .bind(tag_id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// Every classification policy as `(tag_id, required_roles, required_groups)`,
    /// ordered by `tag_id`. Consumed by the PDP loader and by the tag-list UI (to
    /// show which tags are gated).
    pub async fn list_policies(&self) -> Result<Vec<(Uuid, Vec<String>, Vec<Uuid>)>, BiError> {
        Ok(sqlx::query_as::<_, (Uuid, Vec<String>, Vec<Uuid>)>(
            "SELECT tag_id, required_roles, required_groups \
             FROM classification_policies ORDER BY tag_id",
        )
        .fetch_all(&self.db.pool)
        .await?)
    }
}
