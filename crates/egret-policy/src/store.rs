use crate::PolicyStoreError;
use egret_metastore::Db;
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

impl PolicyStore {
    pub fn new(db: Db) -> Self {
        Self { db }
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
               VALUES ($1,$2,$3) RETURNING id, name, type, config_jsonb, policy_version, tag_version"#,
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

    pub async fn get_service(&self, name: &str) -> Result<ServiceRow, PolicyStoreError> {
        sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version, tag_version FROM policy.authz_services WHERE name = $1"#,
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
            r#"SELECT id, name, type, config_jsonb, policy_version, tag_version FROM policy.authz_services WHERE name = $1"#,
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
            r#"SELECT id, name, type, config_jsonb, policy_version, tag_version FROM policy.authz_services ORDER BY name"#,
        )
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn delete_service(&self, id: i64) -> Result<(), PolicyStoreError> {
        let n = sqlx::query(r#"DELETE FROM policy.authz_services WHERE id = $1"#)
            .bind(id)
            .execute(&self.db.pool)
            .await?
            .rows_affected();
        if n == 0 {
            return Err(PolicyStoreError::NotFound(format!("service id {id}")));
        }
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
    /// assignments (keyed by `warehouse_id`) are served under EVERY Trino catalog
    /// name the engine might actually be serving them under right now: the
    /// UNION of
    ///   (a) the registry branch — every catalog this engine attaches, reached
    ///       through `registry.engine_catalog_attachments` (SP3 D5), and
    ///   (b) the legacy branch — the engine's `engine_mgmt.engine_catalogs`
    ///       primary row name, reached through `engines.warehouse_id` (the old
    ///       behaviour).
    /// The legacy branch stays until `engine_catalogs` is retired (SP3c): an
    /// engine whose backfill failed (`backfill_failed`, or the whole run errored
    /// and boot proceeded anyway) never gets an attachment and keeps serving its
    /// legacy catalog name, and an engine whose registry catalog was renamed but
    /// not yet re-applied keeps serving the OLD name until its next apply — both
    /// populations are invisible to branch (a) alone. A name that ends up
    /// matching no live catalog costs nothing (Ranger just never sees a resource
    /// under it); a name that IS live but gets dropped here is the fail-open bug
    /// this union exists to close, so over-emitting is the safe direction.
    /// Distinct `(catalog, schema, table, column, tag)` rows only (multiple
    /// engines on one warehouse, or the same catalog name reached by both
    /// branches, are de-duplicated).
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
        // made tag masking silently no-op. Both branches repeat this resolution
        // (a CTE would need to be materialized per-branch anyway); UNION (not
        // UNION ALL) does the cross-branch de-duplication.
        Ok(sqlx::query_as::<_, TagAssignmentRow>(
            r#"(SELECT
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
                   AND e.authz_service_id = COALESCE(tp.id, s.id))
               UNION
               (SELECT
                   lc.name          AS catalog,
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
               JOIN engine_mgmt.engines e
                    ON e.warehouse_id = a.warehouse_id
                   AND e.authz_service_id = COALESCE(tp.id, s.id)
               JOIN engine_mgmt.engine_catalogs lc
                    ON lc.engine_id = e.id AND lc.is_primary AND lc.kind = 'egret-warehouse')
               ORDER BY catalog, schema_name, table_name, column_name, tag_name"#,
        )
        .bind(service_id)
        .fetch_all(&self.db.pool)
        .await?)
    }
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
        let mut tx = self.db.pool.begin().await?;
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
    pub async fn upsert_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<i64, PolicyStoreError> {
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
        let svc: Option<i64> = sqlx::query_scalar::<_, i64>(
            r#"SELECT service_id FROM policy.authz_policies WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(service_id) = svc else {
            tx.rollback().await?;
            return Err(PolicyStoreError::NotFound(format!("policy {id}")));
        };
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
        let svc: Option<i64> = sqlx::query_scalar::<_, i64>(
            r#"SELECT service_id FROM policy.authz_policies WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(service_id) = svc else {
            tx.rollback().await?;
            return Err(PolicyStoreError::NotFound(format!("policy id {id}")));
        };
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
        let svc: Option<i64> = sqlx::query_scalar::<_, i64>(
            r#"DELETE FROM policy.authz_policies WHERE id = $1 RETURNING service_id"#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(service_id) = svc else {
            tx.rollback().await?;
            return Err(PolicyStoreError::NotFound(format!("policy {id}")));
        };
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
