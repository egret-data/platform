//! Postgres `queries` store + JSONL result I/O over an ObjectStore.

use std::sync::Arc;

use egret_metastore::Db;
use egret_storage::ObjectStore;
use uuid::Uuid;

use egret_core::TabularResult;

use crate::execution::error::EngineError;
use crate::execution::model::QueryStatus;

#[derive(Clone)]
pub struct QueryStore {
    db: Db,
}

#[derive(sqlx::FromRow)]
pub struct QueryRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub database_id: Uuid,
    pub user_id: Option<Uuid>,
    pub sql: String,
    pub status: String,
    pub columns_jsonb: Option<serde_json::Value>,
    pub preview_jsonb: Option<serde_json::Value>,
    pub row_count: Option<i64>,
    pub bytes_scanned: Option<i64>,
    pub truncated: bool,
    pub result_ref: Option<String>,
    pub error: Option<String>,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

/// A typed view used by the routes.
pub struct Query {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub database_id: Uuid,
    pub user_id: Option<Uuid>,
    pub sql: String,
    pub status: QueryStatus,
    pub columns: Option<serde_json::Value>,
    pub preview: Option<serde_json::Value>,
    pub row_count: Option<i64>,
    pub bytes_scanned: Option<i64>,
    pub truncated: bool,
    pub result_ref: Option<String>,
    pub error: Option<String>,
    pub submitted_at: chrono::DateTime<chrono::Utc>,
}

impl QueryRow {
    fn into_query(self) -> Query {
        let status = QueryStatus::parse(&self.status).unwrap_or(QueryStatus::Failed);
        Query {
            id: self.id,
            workspace_id: self.workspace_id,
            database_id: self.database_id,
            user_id: self.user_id,
            sql: self.sql,
            status,
            columns: self.columns_jsonb,
            preview: self.preview_jsonb,
            row_count: self.row_count,
            bytes_scanned: self.bytes_scanned,
            truncated: self.truncated,
            result_ref: self.result_ref,
            error: self.error,
            submitted_at: self.submitted_at,
        }
    }
}

const COLS: &str = "id, workspace_id, database_id, user_id, sql, status, columns_jsonb, \
    preview_jsonb, row_count, bytes_scanned, truncated, result_ref, error, submitted_at";

impl QueryStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn insert(
        &self,
        ws: Uuid,
        db_id: Uuid,
        user: Option<Uuid>,
        sql: &str,
    ) -> Result<Uuid, EngineError> {
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO query.queries (workspace_id, database_id, user_id, sql) \
             VALUES ($1,$2,$3,$4) RETURNING id",
        )
        .bind(ws)
        .bind(db_id)
        .bind(user)
        .bind(sql)
        .fetch_one(&self.db.pool)
        .await?;
        Ok(id)
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<Query>, EngineError> {
        let row = sqlx::query_as::<_, QueryRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM query.queries WHERE id=$1"
        )))
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?;
        Ok(row.map(QueryRow::into_query))
    }

    pub async fn list_for_user(
        &self,
        ws: Uuid,
        user: Uuid,
        limit: i64,
    ) -> Result<Vec<Query>, EngineError> {
        let rows = sqlx::query_as::<_, QueryRow>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM query.queries WHERE workspace_id=$1 AND user_id=$2 \
             AND NOT hidden_from_history ORDER BY submitted_at DESC LIMIT $3"
        )))
        .bind(ws)
        .bind(user)
        .bind(limit)
        .fetch_all(&self.db.pool)
        .await?;
        Ok(rows.into_iter().map(QueryRow::into_query).collect())
    }

    /// Clear `user`'s query history in `ws` (KI-092): their queries leave the
    /// History list. Only rows whose `user_id` is `user` are touched, so a
    /// caller can clear nobody's history but their own. The rows are kept:
    /// status and results by id still answer.
    pub async fn clear_history_for_user(&self, ws: Uuid, user: Uuid) -> Result<u64, EngineError> {
        let r = sqlx::query(
            "UPDATE query.queries SET hidden_from_history = true \
             WHERE workspace_id=$1 AND user_id=$2 AND NOT hidden_from_history",
        )
        .bind(ws)
        .bind(user)
        .execute(&self.db.pool)
        .await?;
        Ok(r.rows_affected())
    }

    pub async fn set_running(&self, id: Uuid) -> Result<(), EngineError> {
        sqlx::query("UPDATE query.queries SET status='running', started_at=now() WHERE id=$1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn set_cancelled(&self, id: Uuid) -> Result<(), EngineError> {
        sqlx::query("UPDATE query.queries SET status='cancelled', finished_at=now() WHERE id=$1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn fail(&self, id: Uuid, error: &str) -> Result<(), EngineError> {
        sqlx::query(
            "UPDATE query.queries SET status='failed', error=$2, finished_at=now() WHERE id=$1",
        )
        .bind(id)
        .bind(error)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    pub async fn finish(
        &self,
        id: Uuid,
        result: &TabularResult,
        result_ref: &str,
        preview_rows: usize,
    ) -> Result<(), EngineError> {
        let columns = serde_json::to_value(&result.columns).unwrap_or(serde_json::json!([]));
        let preview =
            serde_json::to_value(result.rows.iter().take(preview_rows).collect::<Vec<_>>())
                .unwrap_or(serde_json::json!([]));
        sqlx::query(
            "UPDATE query.queries SET status='finished', columns_jsonb=$2, preview_jsonb=$3, \
             row_count=$4, bytes_scanned=$5, truncated=$6, result_ref=$7, finished_at=now() WHERE id=$1")
            .bind(id).bind(columns).bind(preview)
            .bind(result.row_count as i64)
            .bind(result.bytes_scanned.map(|b| b as i64))
            .bind(result.truncated).bind(result_ref)
            .execute(&self.db.pool).await?;
        Ok(())
    }

    // ── JSONL result I/O ─────────────────────────────────────────────────────

    pub fn result_to_jsonl(result: &TabularResult) -> Vec<u8> {
        let mut buf = String::new();
        for row in &result.rows {
            buf.push_str(&serde_json::to_string(row).unwrap_or_else(|_| "[]".into()));
            buf.push('\n');
        }
        buf.into_bytes()
    }

    pub fn jsonl_slice(bytes: &[u8], offset: usize, limit: usize) -> Vec<serde_json::Value> {
        String::from_utf8_lossy(bytes)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .skip(offset)
            .take(limit)
            .map(|l| serde_json::from_str(l).unwrap_or(serde_json::Value::Null))
            .collect()
    }

    pub async fn write_result(
        &self,
        storage: &Arc<dyn ObjectStore>,
        uri: &str,
        result: &TabularResult,
    ) -> Result<(), EngineError> {
        storage.put(uri, Self::result_to_jsonl(result)).await?;
        Ok(())
    }

    pub async fn read_results(
        &self,
        storage: &Arc<dyn ObjectStore>,
        uri: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, EngineError> {
        let bytes = storage.get(uri).await?;
        Ok(Self::jsonl_slice(&bytes, offset, limit))
    }
}
