//! StarRocks connector — MySQL protocol (mysql_async) with row-cap + cancellation.

use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use super::{ConnSpec, Connector};
use crate::error::EngineError;
use crate::model::{Column, Limits, QueryResult};

pub struct StarRocksConnector {
    opts: mysql_async::Opts,
}

impl StarRocksConnector {
    pub fn new(spec: &ConnSpec) -> Self {
        let user = spec.options.user.clone().unwrap_or_else(|| "root".into());
        let mut b = mysql_async::OptsBuilder::default()
            .ip_or_hostname(spec.host.clone())
            .tcp_port(u16::try_from(spec.port).unwrap_or(9030))
            .user(Some(user))
            .prefer_socket(false);
        if let Some(s) = &spec.secret {
            b = b.pass(Some(s.clone()));
        }
        if let Some(db) = &spec.options.database {
            b = b.db_name(Some(db.clone()));
        }
        StarRocksConnector { opts: b.into() }
    }
}

#[async_trait]
impl Connector for StarRocksConnector {
    async fn execute(
        &self,
        sql: &str,
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<QueryResult, EngineError> {
        let mut conn = mysql_async::Conn::new(self.opts.clone())
            .await
            .map_err(|e| EngineError::Connect(e.to_string()))?;

        let mut result = conn
            .query_iter(sql)
            .await
            .map_err(|e| EngineError::Engine(e.to_string()))?;

        // Read columns BEFORE consuming any rows — columns_ref() accesses the pending result set.
        let columns: Vec<Column> = result
            .columns_ref()
            .iter()
            .map(|c| Column {
                name: c.name_str().to_string(),
                ty: format!("{:?}", c.column_type()),
            })
            .collect();

        // Capture per-column MySQL types for type-aware value coercion.
        let col_types: Vec<mysql_async::consts::ColumnType> = result
            .columns_ref()
            .iter()
            .map(|c| c.column_type())
            .collect();

        let cap = limits.max_rows;
        let mut rows: Vec<Vec<serde_json::Value>> = Vec::new();

        loop {
            if cancel.is_cancelled() {
                drop(result);
                let _ = conn.disconnect().await;
                return Err(EngineError::Cancelled);
            }
            match result
                .next()
                .await
                .map_err(|e| EngineError::Engine(e.to_string()))?
            {
                None => break,
                Some(row) => {
                    rows.push(row_to_json(row, &col_types));
                    if rows.len() > cap {
                        break;
                    }
                }
            }
        }

        drop(result);
        let _ = conn.disconnect().await;

        let truncated = rows.len() > cap;
        if truncated {
            rows.truncate(cap);
        }
        Ok(QueryResult {
            columns,
            row_count: rows.len() as u64,
            rows,
            bytes_scanned: None,
            truncated,
        })
    }

    async fn describe(&self, sql: &str, timeout: Duration) -> Result<Vec<Column>, EngineError> {
        let wrapped = format!("SELECT * FROM (\n{sql}\n) AS _egret_ds LIMIT 0");
        let opts = self.opts.clone();
        // Bound the whole connect+query under `timeout` — unlike `execute`, `describe`
        // is called directly (no JobManager outer timeout), so a hung query would block.
        let fut = async move {
            let mut conn = mysql_async::Conn::new(opts)
                .await
                .map_err(|e| EngineError::Connect(e.to_string()))?;
            let result = conn
                .query_iter(wrapped)
                .await
                .map_err(|e| EngineError::Engine(e.to_string()))?;
            let columns: Vec<Column> = result
                .columns_ref()
                .iter()
                .map(|c| Column {
                    name: c.name_str().to_string(),
                    ty: coltype_name(c.column_type()).to_string(),
                })
                .collect();
            drop(result);
            let _ = conn.disconnect().await;
            Ok::<Vec<Column>, EngineError>(columns)
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| EngineError::Timeout)?
    }
}

fn row_to_json(
    row: mysql_async::Row,
    col_types: &[mysql_async::consts::ColumnType],
) -> Vec<serde_json::Value> {
    use mysql_async::Value as V;
    (0..row.len())
        .map(|i| match row.as_ref(i) {
            Some(V::NULL) | None => serde_json::Value::Null,
            Some(V::Int(n)) => serde_json::json!(n),
            Some(V::UInt(n)) => serde_json::json!(n),
            Some(V::Float(f)) => serde_json::json!(f),
            Some(V::Double(f)) => serde_json::json!(f),
            // Text protocol returns everything as Bytes; use the declared column type
            // to decide coercion — prevents silent string→number corruption (e.g. "007"→7).
            Some(V::Bytes(b)) => bytes_to_json(b, col_types.get(i).copied()),
            Some(other) => serde_json::Value::String(format!("{:?}", other)),
        })
        .collect()
}

/// Coerce a MySQL text-protocol byte string to a JSON scalar using the declared column type.
///
/// - Integer types  → parse as i64, fall back to u64, else string.
/// - Float types    → parse as f64, else string.
/// - Everything else (VARCHAR, DECIMAL, dates, …) → keep as string to preserve exact value.
/// - Non-UTF-8 binary → hex-encode.
fn bytes_to_json(b: &[u8], ty: Option<mysql_async::consts::ColumnType>) -> serde_json::Value {
    use mysql_async::consts::ColumnType::*;
    let Ok(s) = std::str::from_utf8(b) else {
        return serde_json::Value::String(hex::encode(b));
    };
    match ty {
        Some(
            MYSQL_TYPE_TINY | MYSQL_TYPE_SHORT | MYSQL_TYPE_LONG | MYSQL_TYPE_LONGLONG
            | MYSQL_TYPE_INT24 | MYSQL_TYPE_YEAR,
        ) => s
            .parse::<i64>()
            .map(|i| serde_json::json!(i))
            .or_else(|_| s.parse::<u64>().map(|u| serde_json::json!(u)))
            .unwrap_or_else(|_| serde_json::Value::String(s.to_owned())),
        Some(MYSQL_TYPE_FLOAT | MYSQL_TYPE_DOUBLE) => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(s.to_owned())),
        // DECIMAL/NEWDECIMAL, VARCHAR, STRING, dates, blobs, and everything else:
        // keep as string to preserve exact representation.
        _ => serde_json::Value::String(s.to_owned()),
    }
}

/// Map a MySQL/StarRocks protocol column type to a canonical lowercase SQL name,
/// matching the vocabulary `DESCRIBE` returns so `typegen` classifies both the
/// physical (DESCRIBE) and virtual (describe) paths identically.
fn coltype_name(ty: mysql_async::consts::ColumnType) -> &'static str {
    use mysql_async::consts::ColumnType::*;
    match ty {
        MYSQL_TYPE_TINY => "tinyint",
        MYSQL_TYPE_SHORT => "smallint",
        MYSQL_TYPE_LONG | MYSQL_TYPE_INT24 | MYSQL_TYPE_YEAR => "int",
        MYSQL_TYPE_LONGLONG => "bigint",
        MYSQL_TYPE_FLOAT => "float",
        MYSQL_TYPE_DOUBLE => "double",
        MYSQL_TYPE_DECIMAL | MYSQL_TYPE_NEWDECIMAL => "decimal",
        MYSQL_TYPE_DATE | MYSQL_TYPE_NEWDATE => "date",
        MYSQL_TYPE_DATETIME | MYSQL_TYPE_DATETIME2 | MYSQL_TYPE_TIMESTAMP
        | MYSQL_TYPE_TIMESTAMP2 => "datetime",
        MYSQL_TYPE_TIME | MYSQL_TYPE_TIME2 => "time",
        MYSQL_TYPE_JSON => "json",
        MYSQL_TYPE_VARCHAR | MYSQL_TYPE_VAR_STRING | MYSQL_TYPE_STRING => "varchar",
        _ => "string",
    }
}
