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
            r#"INSERT INTO authz_service_defs (id, name, def_jsonb) VALUES ($1,$2,$3)
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
            r#"SELECT id, name, def_jsonb FROM authz_service_defs WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| AuthzError::NotFound(format!("service-def {name}")))
    }

    pub async fn list_service_defs(&self) -> Result<Vec<ServiceDefRow>, AuthzError> {
        Ok(sqlx::query_as::<_, ServiceDefRow>(
            r#"SELECT id, name, def_jsonb FROM authz_service_defs ORDER BY name"#,
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
            r#"INSERT INTO authz_services (name, type, config_jsonb)
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
            r#"SELECT id, name, type, config_jsonb, policy_version FROM authz_services WHERE name = $1"#,
        )
        .bind(name)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| AuthzError::NotFound(format!("service {name}")))
    }

    pub async fn list_services(&self) -> Result<Vec<ServiceRow>, AuthzError> {
        Ok(sqlx::query_as::<_, ServiceRow>(
            r#"SELECT id, name, type, config_jsonb, policy_version FROM authz_services ORDER BY name"#,
        )
        .fetch_all(&self.db.pool)
        .await?)
    }

    pub async fn delete_service(&self, id: i64) -> Result<(), AuthzError> {
        let n = sqlx::query(r#"DELETE FROM authz_services WHERE id = $1"#)
            .bind(id)
            .execute(&self.db.pool)
            .await?
            .rows_affected();
        if n == 0 {
            return Err(AuthzError::NotFound(format!("service id {id}")));
        }
        Ok(())
    }
}
