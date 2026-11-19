//! EO4: the global, single-row switch selecting which table the PDP reads
//! per-request grants from (`perm.cedar_grant_source`). Read on every reload;
//! written by the operator endpoints. Default (no row) is `Legacy` —
//! `access_grants`, the proven, pre-EO4 path.
use chrono::{DateTime, Utc};
use egret_metastore::Db;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum GrantSource {
    /// Read `access_grants` (pre-EO4 behaviour). The default.
    Legacy,
    /// Read the unified store mirror `perm.cedar_grants`.
    Unified,
    /// Serve `access_grants`, but run the equivalence report each reload (observe).
    Shadow,
}

impl GrantSource {
    pub fn as_db(self) -> &'static str {
        match self {
            GrantSource::Legacy => "legacy",
            GrantSource::Unified => "unified",
            GrantSource::Shadow => "shadow",
        }
    }
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "legacy" => Some(Self::Legacy),
            "unified" => Some(Self::Unified),
            "shadow" => Some(Self::Shadow),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct GrantSourceState {
    pub source: GrantSource,
    pub switched_at: Option<DateTime<Utc>>,
    pub switched_by: Option<String>,
}

/// Infallible, fail-safe to `Legacy` on any error or absent row (hot path).
pub async fn read_source(db: &Db) -> GrantSource {
    match sqlx::query_scalar::<_, String>(
        "SELECT source FROM perm.cedar_grant_source WHERE id = true",
    )
    .fetch_optional(&db.pool)
    .await
    {
        Ok(Some(s)) => GrantSource::from_db(&s).unwrap_or_else(|| {
            tracing::error!(source = %s, "unknown cedar_grant_source value; reading legacy");
            GrantSource::Legacy
        }),
        Ok(None) => GrantSource::Legacy,
        Err(e) => {
            tracing::warn!(error = %e, "cedar_grant_source unreadable; reading legacy");
            GrantSource::Legacy
        }
    }
}

pub async fn read_state(db: &Db) -> egret_core::Result<GrantSourceState> {
    let row: Option<(String, Option<DateTime<Utc>>, Option<String>)> = sqlx::query_as(
        "SELECT source, switched_at, switched_by FROM perm.cedar_grant_source WHERE id = true",
    )
    .fetch_optional(&db.pool)
    .await
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
    Ok(match row {
        Some((s, at, by)) => GrantSourceState {
            source: GrantSource::from_db(&s).unwrap_or(GrantSource::Legacy),
            switched_at: at,
            switched_by: by,
        },
        None => GrantSourceState {
            source: GrantSource::Legacy,
            switched_at: None,
            switched_by: None,
        },
    })
}

pub async fn write_source(db: &Db, source: GrantSource, actor: &str) -> egret_core::Result<()> {
    sqlx::query(
        "INSERT INTO perm.cedar_grant_source (id, source, switched_at, switched_by) \
         VALUES (true, $1, now(), $2) \
         ON CONFLICT (id) DO UPDATE SET source = EXCLUDED.source, \
           switched_at = EXCLUDED.switched_at, switched_by = EXCLUDED.switched_by",
    )
    .bind(source.as_db())
    .bind(actor)
    .execute(&db.pool)
    .await
    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
    Ok(())
}
