//! EO3: the global, single-row switch selecting which assembler the PDP serves
//! (`perm.pdp_policy_source`). Read on every reload; written by the operator
//! endpoints. Default (no row) is `Legacy` — the proven, pre-EO3 path.

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use serde::Serialize;

/// Which assembler the live PDP serves. Stored as text in the DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PolicySource {
    /// `load_inner`'s bespoke SQL (pre-EO3 behaviour). The default.
    Legacy,
    /// Serve legacy, but also build + compare the compiled set (canary).
    Shadow,
    /// Serve the egret-cedar-compiled set; fail-safe to legacy on error.
    Compiled,
}

impl PolicySource {
    pub fn as_db(self) -> &'static str {
        match self {
            PolicySource::Legacy => "legacy",
            PolicySource::Shadow => "shadow",
            PolicySource::Compiled => "compiled",
        }
    }
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "legacy" => Some(PolicySource::Legacy),
            "shadow" => Some(PolicySource::Shadow),
            "compiled" => Some(PolicySource::Compiled),
            _ => None,
        }
    }
}

/// The switch row with its audit columns, for the GET endpoint.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SourceState {
    pub source: PolicySource,
    pub switched_at: Option<DateTime<Utc>>,
    pub switched_by: Option<String>,
}

/// Read the current source, fail-safe to `Legacy` on any error or absent row.
/// Called on the reload hot path, so it never returns an error — an unreadable
/// switch must not break a reload (EO-D7).
pub async fn read_source(db: &Db) -> PolicySource {
    match sqlx::query_scalar::<_, String>(
        "SELECT source FROM perm.pdp_policy_source WHERE id = true",
    )
    .fetch_optional(&db.pool)
    .await
    {
        Ok(Some(s)) => PolicySource::from_db(&s).unwrap_or_else(|| {
            tracing::error!(source = %s, "unknown pdp_policy_source value; serving legacy");
            PolicySource::Legacy
        }),
        Ok(None) => PolicySource::Legacy,
        Err(e) => {
            tracing::warn!(error = %e, "pdp_policy_source unreadable; serving legacy");
            PolicySource::Legacy
        }
    }
}

/// Read the full switch row (source + audit) for the GET endpoint.
pub async fn read_state(db: &Db) -> egret_core::Result<SourceState> {
    let row: Option<(String, Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
        "SELECT source, switched_at, switched_by FROM perm.pdp_policy_source WHERE id = true",
    )
    .fetch_optional(&db.pool)
    .await
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
    Ok(match row {
        Some((s, at, by)) => SourceState {
            source: PolicySource::from_db(&s).unwrap_or(PolicySource::Legacy),
            switched_at: at,
            switched_by: by,
        },
        None => SourceState {
            source: PolicySource::Legacy,
            switched_at: None,
            switched_by: None,
        },
    })
}

/// Upsert the singleton switch row, stamping the audit columns.
pub async fn write_source(db: &Db, source: PolicySource, actor: &str) -> egret_core::Result<()> {
    sqlx::query(
        "INSERT INTO perm.pdp_policy_source (id, source, switched_at, switched_by) \
         VALUES (true, $1, now(), $2) \
         ON CONFLICT (id) DO UPDATE SET \
           source = EXCLUDED.source, \
           switched_at = EXCLUDED.switched_at, \
           switched_by = EXCLUDED.switched_by",
    )
    .bind(source.as_db())
    .bind(actor)
    .execute(&db.pool)
    .await
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
    Ok(())
}
