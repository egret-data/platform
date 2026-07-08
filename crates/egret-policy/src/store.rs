use crate::AuthzError;
use egret_metastore::Db;
use serde_json::Value;

#[derive(Clone)]
pub struct AuthzStore {
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
}

impl AuthzStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Idempotent upsert of a service-def by name (used to seed Trino).
    pub async fn upsert_service_def(
        &self,
        id: i64,
        name: &str,
        def: &Value,
    ) -> Result<(), AuthzError> {
        sqlx::query(
            r#"INSERT INTO policy.authz_service_defs (id, name, def_jsonb) VALUES ($1,$2,$3)
               ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, def_jsonb = EXCLUDED.def_jsonb"#,
        )
        .bind(id)
        .bind(name)
        .bind(def)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn get_service_def(&self, name: &str) -> Result<ServiceDefRow, AuthzError> {
        sqlx::query_as::<_, ServiceDefRow>(
            r#"SELECT id, name, def_jsonb FROM policy.authz_service_defs WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| AuthzError::NotFound(format!("service-def {name}")))
    }

    pub async fn list_service_defs(&self) -> Result<Vec<ServiceDefRow>, AuthzError> {
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
    ) -> Result<ServiceRow, AuthzError> {
        sqlx::query_as::<_, ServiceRow>(
            r#"INSERT INTO policy.authz_services (name, type, config_jsonb)
               VALUES ($1,$2,$3) RETURNING id, name, type, config_jsonb, policy_version"#,
        )
        .bind(name)
        .bind(typ)
        .bind(config)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                AuthzError::AlreadyExists(name.into())
            }
            _ => AuthzError::Db(e),
        })
    }

    pub async fn get_service(&self, name: &str) -> Result<ServiceRow, AuthzError> {
        sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version FROM policy.authz_services WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| AuthzError::NotFound(format!("service {name}")))
    }

    pub async fn list_services(&self) -> Result<Vec<ServiceRow>, AuthzError> {
        Ok(sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version FROM policy.authz_services ORDER BY name"#,
        )
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn delete_service(&self, id: i64) -> Result<(), AuthzError> {
        let n = sqlx::query(r#"DELETE FROM policy.authz_services WHERE id = $1"#)
            .bind(id)
            .execute(&self.db.pool)
            .await?
            .rows_affected();
        if n == 0 {
            return Err(AuthzError::NotFound(format!("service id {id}")));
        }
        Ok(())
    }

    /// Total policy count per service (service_id -> count), for the profiles list.
    pub async fn policy_counts(&self) -> Result<std::collections::HashMap<i64, i64>, AuthzError> {
        let rows = sqlx::query_as::<_, (i64, i64)>(
            "SELECT service_id, count(*) FROM policy.authz_policies GROUP BY service_id",
        )
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows.into_iter().collect())
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

impl AuthzStore {
    /// Create a policy under a service and bump policy_version atomically. Returns the new policy id.
    pub async fn create_policy(
        &self,
        service_id: i64,
        name: &str,
        policy: &Value,
    ) -> Result<i64, AuthzError> {
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
                AuthzError::AlreadyExists(name.into())
            }
            _ => AuthzError::Db(e),
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

    pub async fn update_policy(
        &self,
        id: i64,
        name: &str,
        policy: &Value,
        enabled: bool,
    ) -> Result<(), AuthzError> {
        let mut tx = self.db.pool.begin().await?;
        let svc: Option<i64> = sqlx::query_scalar::<_, i64>(
            r#"SELECT service_id FROM policy.authz_policies WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(service_id) = svc else {
            tx.rollback().await?;
            return Err(AuthzError::NotFound(format!("policy {id}")));
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
    pub async fn set_policy_enabled(&self, id: i64, enabled: bool) -> Result<(), AuthzError> {
        let mut tx = self.db.pool.begin().await?;
        let svc: Option<i64> = sqlx::query_scalar::<_, i64>(
            r#"SELECT service_id FROM policy.authz_policies WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(service_id) = svc else {
            tx.rollback().await?;
            return Err(AuthzError::NotFound(format!("policy id {id}")));
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

    pub async fn delete_policy(&self, id: i64) -> Result<(), AuthzError> {
        let mut tx = self.db.pool.begin().await?;
        let svc: Option<i64> = sqlx::query_scalar::<_, i64>(
            r#"DELETE FROM policy.authz_policies WHERE id = $1 RETURNING service_id"#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(service_id) = svc else {
            tx.rollback().await?;
            return Err(AuthzError::NotFound(format!("policy {id}")));
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

    pub async fn get_policy(&self, id: i64) -> Result<PolicyRow, AuthzError> {
        sqlx::query_as::<_, PolicyRow>(
            r#"SELECT id, service_id, name, policy_jsonb, enabled, version FROM policy.authz_policies WHERE id = $1"#,
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| AuthzError::NotFound(format!("policy {id}")))
    }

    /// All policies for a service (enabled and disabled), newest first — the
    /// admin list view. Returns typed rows; unlike `enabled_policies_for_service`
    /// this does not inject id/version into the body (the caller/summary does that).
    pub async fn list_policies(&self, service_id: i64) -> Result<Vec<PolicyRow>, AuthzError> {
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
    ) -> Result<Vec<Value>, AuthzError> {
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
