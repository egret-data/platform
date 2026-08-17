//! `POST …/endpoints/{id}/preview` — spec §2 of the P6 authoring-loop design.
//!
//! # What this is, in one line
//!
//! The author's on-screen draft, run as the AUTHOR, through the same validation
//! and render path publish and serving use, capped at 100 rows, with none of the
//! four consumer-facing side effects.
//!
//! # Why the body carries the draft rather than being read from the database
//!
//! Reading the saved draft would defeat the feature: the author would have to
//! Save before every preview, and the loop this route exists to close would
//! still be cut — by a button rather than by navigation. This opens no new
//! attack surface, because SQL Lab already executes arbitrary author-supplied
//! SQL at an equivalent permission, and the payload here passes the same gates
//! publish passes ([`crate::lifecycle::validate_preview`]).
//!
//! # The four things this must never do
//!
//! No `access_log` row (Activity measures real traffic; a preview click is not
//! traffic, and writing one would skew every percentile on that page). No
//! rate-limit consumption (an author iterating must not throttle a live
//! consumer). No cache read (it would show a stale answer for SQL just edited)
//! and no cache write (it would poison what consumers read with a result
//! produced under a different principal).
//!
//! Each absence is asserted in `tests/preview.rs`, because an absence is
//! exactly what a later refactor adds back without noticing.

use std::collections::BTreeMap;

use egret_core::UserId;
use egret_dataapi_api::ParamSpec;
use uuid::Uuid;

use crate::routes::{DataApiError, DataApiState};

/// Rows a preview returns, at most.
///
/// Fixed and not configurable, and the endpoint's own `limits.max_rows` does not
/// apply: an author wants the SHAPE of the data, not a million rows in a
/// browser. Named here rather than left to a call site so there is one number to
/// change and one number the UI's copy has to agree with.
pub(crate) const PREVIEW_MAX_ROWS: usize = 100;

/// Wall-clock budget for a preview.
///
/// Shorter than the serving ceiling (`serving/router.rs`'s 300 s) because the
/// person waiting is at a keyboard with an editor open, not a batch client. The
/// draft body carries no `limits`, so this is not clamping author input — it is
/// the whole budget.
pub(crate) const PREVIEW_TIMEOUT_SECS: u64 = 30;

/// The on-screen draft, as the request carries it.
///
/// No `cache`, `limits` or `rate_limit`: preview does not consult any of them,
/// and accepting fields it ignores would invite the reader to think it did.
#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewEndpointInput {
    /// What is in the editor right now — not what is stored.
    pub sql: String,
    /// The parameter declarations as they are on screen right now.
    #[serde(default)]
    pub params: Vec<ParamSpec>,
    /// The author's test values, one list per parameter name. A list, not a
    /// scalar, because a repeated parameter is a list on the serving path too
    /// and preview must resolve it the same way.
    #[serde(default)]
    pub values: BTreeMap<String, Vec<String>>,
    /// The database picker's current value. Workspace-scoped by
    /// `connection_per_principal_pair`, which answers `NotFound` for a database
    /// belonging to another workspace — this id comes from a request body, so it
    /// is exactly the shape that could name someone else's connection.
    pub database_id: Uuid,
}

/// What a preview answers with.
///
/// `columns` carries the ENGINE's own names — `output.naming` is deliberately not
/// applied, because an author debugging SQL needs the names they wrote, and the
/// published contract (naming transform included) is what `Response` is for. The
/// UI must say which is which.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct DataApiPreviewDto {
    pub columns: Vec<egret_core::ColumnMeta>,
    #[schema(value_type = Vec<Vec<Object>>)]
    pub rows: Vec<Vec<serde_json::Value>>,
    pub row_count: u64,
    /// True when the result was longer than [`PREVIEW_MAX_ROWS`] and this is a
    /// prefix of it. The connector fetches `cap + 1` and sets this rather than
    /// truncating silently, so `row_count == 100` alone cannot tell "exactly at"
    /// from "over".
    pub truncated: bool,
}

/// Run `body` as `principal` and return what the engine said.
///
/// Every error variant passes through unchanged, which is the deliberate
/// INVERSE of the serving path's `data_access_denied` sanitising: a consumer must
/// not learn the shape of the tables, and an author who owns the SQL cannot fix a
/// syntax error from a sanitised message. That inversion is why this expression
/// must never be reachable below `data_api_endpoint:manage`.
pub(crate) async fn run(
    st: &DataApiState,
    ws: Uuid,
    principal: UserId,
    body: &PreviewEndpointInput,
) -> Result<DataApiPreviewDto, DataApiError> {
    // Workspace containment for a body-supplied id, before anything is rendered
    // or executed. The pair itself is unused — preview consults no cache — but
    // this call is the scope check, so it is not replaceable with a cheaper one.
    let _ = st
        .endpoints
        .connection_per_principal_pair(ws, body.database_id)
        .await?;

    crate::lifecycle::validate_preview(&body.sql, &body.params)?;

    // The SAME function serving resolves through, so a value that renders one
    // way here cannot render another way for a consumer.
    let resolved = crate::serving::params::resolve_supplied(&body.values, &body.sql, &body.params)
        .map_err(|_| {
            egret_core::Error::InvalidInput(
                "a test value does not satisfy its parameter's declared type".into(),
            )
        })?;

    let (sql, binds) = crate::render::render(&body.sql, &resolved.values)
        .map_err(crate::lifecycle::ValidationError::Render)?;

    let out = st
        .query
        .execute_as_with_params(
            ws,
            body.database_id,
            principal,
            &sql,
            &binds,
            egret_query_api::Limits {
                max_rows: PREVIEW_MAX_ROWS,
                timeout: std::time::Duration::from_secs(PREVIEW_TIMEOUT_SECS),
            },
        )
        .await?;

    Ok(DataApiPreviewDto {
        columns: out.columns,
        rows: out.rows,
        row_count: out.row_count,
        truncated: out.truncated,
    })
}
