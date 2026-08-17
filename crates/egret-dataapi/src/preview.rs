//! `POST …/endpoints/{id}/preview` — spec §2 of the P6 authoring-loop design.
//!
//! # What this is, in one line
//!
//! The author's on-screen draft, run as the AUTHOR, through the same validation
//! and render path publish and serving use, capped at 100 rows, with none of the
//! four consumer-facing side effects.
//!
//! # Not quite EVERY publish rule — two are about things a preview does not do
//!
//! [`crate::lifecycle::validate_preview`] omits the cache-scope rule (preview
//! touches no cache) and the unreferenced-parameter rule (it protects the OpenAPI
//! contract a PUBLISHED endpoint offers, not the statement about to run). The
//! second one is a browser finding: with it in place, SQL edited before the
//! parameter declarations caught up — the commonest state of the loop this route
//! exists to close — could not be previewed at all. Every other publish scan
//! still applies, including the statement check that keeps `:manage` from being
//! engine write access and the placeholder-inside-quoted-text rule. That
//! function's own comment is the authority on the split.
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
//! # What actually asserts those four absences
//!
//! Two of the four are asserted end-to-end in `tests/preview.rs`:
//! `preview_writes_no_access_log_row` counts `dataapi.access_log` before and
//! after, and `preview_does_not_populate_the_cache` publishes a cache-enabled
//! `shared`-scope endpoint, previews it, and requires a consumer's next call to be
//! a MISS. **There is no cache-READ test and no rate-limit test** — do not read the
//! four as equally covered.
//!
//! The absence is nonetheless STRUCTURAL, which is the stronger guarantee and the
//! reason the two missing tests were not written: this module contains no cache,
//! rate-limit, `access_log` or audit reference of any kind. There is nothing to
//! disable and no flag to get wrong — [`run`] goes straight from validation to
//! `execute_as_with_params`. So the failure mode to watch for is a refactor that
//! ADDS one of the four (routing preview through a serving helper that does all of
//! them), not a switch flipping; and grep for those four names in this file is the
//! check that catches it.
//!
//! # Preview bypasses `decode`, so `values` is not bounded by `MAX_PARAM_BYTES`
//!
//! The serving path reaches `resolve` through `decode`, which enforces
//! `MAX_PARAM_BYTES` and `MAX_PARAM_REPEATS` on a query string. [`run`] calls
//! [`crate::serving::params::resolve_supplied`] with the body's already-parsed map
//! instead, so neither cap applies here. Accepted, deliberately:
//!
//! * The body is bounded by axum's request body limit, and what reaches the engine
//!   is bounded by `render`'s `CappedSink` — so neither this process nor the
//!   coordinator can be made to hold an unbounded statement.
//! * The route is `data_api_endpoint:manage`-gated. An author who wanted to send
//!   the engine a large statement can already do so with SQL Lab, at equivalent
//!   permission; the caps exist to stop an anonymous-ish CONSUMER, which is not
//!   who is here.
//! * `resolve_supplied` exists precisely because re-encoding the body's map into a
//!   query string and pushing it back through `decode` is lossy — a value
//!   containing `&`, `=` or `%` would not survive the round trip intact, so
//!   "reuse `decode` for the caps" would trade a bound for a correctness bug.

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

/// One refusal from [`crate::serving::params::resolve_supplied`], as the sentence
/// the AUTHOR reads.
///
/// # Why this is not one message
///
/// Serving answers **both** parameter failures with a fixed, payload-free
/// sentence, and both reasons for that are about a CONSUMER: naming the parameter
/// turns the error surface into a directory of what an endpoint declares, and
/// echoing the value reflects caller bytes into a response. Neither applies to a
/// preview. The author owns the declarations, is looking at them on screen, and
/// typed the value themselves — so a message that will not say which input was
/// wrong, or that calls a blank required field a type error, sends them to edit
/// the wrong thing. That is the same defect this route exists to invert; see the
/// module comment.
///
/// # This pass decides nothing
///
/// It runs only after `resolve_supplied` has ALREADY refused, and it re-walks the
/// declarations for one purpose: to recover which one. It reaches that answer
/// through [`crate::serving::params::coerce`] — the same function that made the
/// decision, never a second copy of the rules — and when it cannot reproduce the
/// refusal it falls back to an unnamed sentence for the variant rather than
/// guessing. So a disagreement between this pass and `resolve_supplied` can only
/// ever cost precision in a message; it can never change the verdict.
fn param_error(
    e: crate::serving::ServingError,
    specs: &[ParamSpec],
    values: &BTreeMap<String, Vec<String>>,
) -> DataApiError {
    use crate::serving::ServingError as E;
    let supplied = |spec: &ParamSpec| values.get(&spec.name).filter(|v| !v.is_empty());
    match e {
        // A `required` declaration with no `default` and nothing typed in. The
        // fix is to fill the field in, which the old blanket message actively
        // pointed away from.
        E::ParamMissing => {
            let name = specs
                .iter()
                .find(|s| s.required && s.default.is_none() && supplied(s).is_none())
                .map(|s| s.name.clone());
            egret_core::Error::InvalidInput(match name {
                Some(name) => format!("`{name}` is declared required and has no test value"),
                None => "a parameter declared required has no test value".into(),
            })
        }
        // Two different mistakes share this variant, and they have two different
        // fixes: a value the declared type cannot read, and more than one value
        // for an enum (which is a single choice — `serving::params` refuses a
        // repeat so `| ident` can only ever see one).
        E::ParamInvalid => {
            let repeated = specs
                .iter()
                .find(|s| {
                    s.ty == egret_dataapi_api::ParamType::Enum
                        && supplied(s).is_some_and(|v| v.len() > 1)
                })
                .map(|s| {
                    format!(
                        "`{}` is an enum and takes a single test value",
                        s.name.clone()
                    )
                });
            let uncoercible = || {
                specs.iter().find_map(|s| {
                    let v = supplied(s)?;
                    v.iter()
                        .any(|t| crate::serving::params::coerce(s, t).is_err())
                        .then(|| {
                            format!(
                                "the test value for `{}` is not a valid {}",
                                s.name,
                                crate::lifecycle::type_name(&s.ty)
                            )
                        })
                })
            };
            egret_core::Error::InvalidInput(
                repeated
                    .or_else(uncoercible)
                    .unwrap_or_else(|| "a test value is not valid for its parameter".into()),
            )
        }
        // Not reachable: both `Internal` arms in `resolve_supplied` are about a
        // published definition that `validate_preview` has already refused above
        // (a `default` that fails its own type, an enum with no `allowed`). A 500
        // rather than a 400 if it ever happens anyway — it would be egret's fault,
        // not the author's, and `Error::Internal` is what redacts the detail into
        // the log instead of the body.
        other => egret_core::Error::Internal(anyhow::anyhow!("{other}")),
    }
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
        .map_err(|e| param_error(e, &body.params, &body.values))?;

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
