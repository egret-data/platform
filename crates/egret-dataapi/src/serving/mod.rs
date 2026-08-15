//! P4 — the Data API **serving** surface: the public, token-authenticated path
//! that runs a published endpoint. Everything under `/api` is session- and
//! CSRF-protected and belongs to a human; this surface belongs to a machine and
//! is authenticated by a bearer token alone.
//!
//! Task 2 lands [`auth`]: bearer token → [`ServingPrincipal`]. Task 3 lands
//! [`router`]: URL → published snapshot → rows. Task 4 lands [`cache`] and
//! [`ratelimit`] — steps 6 and 5 of spec §6.2. Task 5 lands [`error`] (spec §7's
//! envelope and its fixed code table) and [`access_log`] (spec §8).

pub mod access_log;
pub mod auth;
pub mod cache;
pub mod error;
pub mod ratelimit;
pub mod router;

pub use access_log::{scheduled_retention, AccessLogEntry, AccessLogStore};
pub use auth::{verify_serving_token, ServingPrincipal, DATA_API_AUDIENCE};
pub use cache::{cache_key, shared_cache_allowed, CachedResponse, ResponseCache};
pub use error::{ServingErrorBody, ServingErrorDetail};
pub use ratelimit::{Rate, RateLimitStore, TokenBucket};
pub use router::{serving_openapi, serving_router, ServingMeta, ServingResponse};

/// A serving-path failure.
///
/// Every authentication failure — wrong audience, expired, missing `exp`, bad
/// signature, unparseable identity claim — is the SAME [`Self::Unauthorized`].
/// Distinguishing them would hand anyone probing the surface a free oracle:
/// "expired" tells them the signature was good, "wrong audience" tells them the
/// token is a real egret token. The caller learns only that authentication
/// failed; the reason goes to the log.
///
/// [`Self::NotFound`] carries the same discipline one layer down and for the
/// same reason: unknown endpoint, disabled endpoint, wrong workspace, archived
/// workspace and a path that is not even well-formed are one variant, so a
/// caller cannot learn that an endpoint exists in a workspace it cannot reach.
///
/// [`super::error::to_wire`] owns the wire mapping — spec §7's fixed code table
/// and the `{ "error": { code, message, request_id } }` envelope — and matches
/// this enum EXHAUSTIVELY. Adding a variant here is a compile error there, on
/// purpose: a wildcard arm would turn a new, specific, actionable condition into
/// a silent 500, and would inherit whatever the wildcard did with any engine
/// message the new variant carried.
///
/// # The variants spec §7 names that do NOT exist here
///
/// Recorded so their absence reads as a decision rather than as an oversight.
/// None of them is constructible on today's serving path, and a dead variant
/// would make the table look finished while nothing could ever produce the code:
///
/// * `param_invalid` / `param_missing` — spec §6.2 step 7 (rendering declared
///   `params` into the SQL) is not implemented, so no caller input is ever
///   validated. They arrive with step 7.
/// * `engine_unavailable` (502) and `timeout` (504) — **not decidable at this
///   seam.** `QueryExecPort` returns `egret_core::Error`, and `port_impl` folds
///   BOTH the engine's infrastructure failures (`Connect`/`Storage`/`Db`) AND
///   its own egret-side faults — resolving the principal's email, opening the
///   database secret (`egret-query/src/execution/port_impl.rs:120-137`) — into
///   the same `Error::Internal`. Mapping that to 502 would page an operator
///   about the engine when the metastore is what failed. `Timeout` is worse:
///   `engine_error` maps `EngineError::Timeout` to
///   `Error::InvalidInput("query timed out")`, deliberately, because for
///   `describe_as` a timeout means the query is too expensive to plan — so the
///   only way to recognise it here would be to match another crate's message
///   string, which breaks silently the day someone rewords it. Making these two
///   real means widening the port's error type, which is a cross-crate contract
///   change and not this task's.
///
/// Everything a caller CAN receive today is in the table, and everything in the
/// table is reachable.
#[derive(Debug, thiserror::Error)]
pub enum ServingError {
    #[error("unauthorized")]
    Unauthorized,
    /// Absent, disabled, in another workspace, in an archived workspace, or a
    /// path that could not be canonicalised. One variant on purpose — see the
    /// type comment.
    #[error("not found")]
    NotFound,
    /// The engine refused the DATA (Ranger), not the request. Kept apart from
    /// [`Self::Internal`] because the operator's fix is a grant, and apart from
    /// any "bad input" code because the author's SQL is fine — telling them
    /// otherwise sends them to debug the wrong thing.
    #[error("data access denied")]
    DataAccessDenied,
    /// **egret** refused the CALL: the service account holds no
    /// `data_api_endpoint:invoke` grant on this endpoint (spec §6.2 step 4).
    ///
    /// Kept distinct from [`Self::DataAccessDenied`] because they are two
    /// different people's problems with two different fixes, and collapsing them
    /// would send an operator to the wrong system:
    ///
    /// * `DataAccessDenied` — the ENGINE refused the DATA. egret let the call
    ///   through, the query ran as the service account, and Ranger filtered it
    ///   to nothing or refused the relation. The fix is a Ranger grant.
    /// * `NotGranted` — egret refused before the engine was ever contacted. The
    ///   fix is `POST …/endpoints/{id}/grants` naming this service account.
    ///
    /// **403 and not 404**, which is the opposite of every earlier step. By this
    /// point the caller has proved it holds a valid credential for this
    /// workspace, so "this endpoint exists and you may not call it" tells it
    /// nothing it could not already enumerate through the management API or by
    /// walking its own grants. The 404s above hide existence from a caller who
    /// has proved nothing; there is nothing left to hide here, and a 404 would
    /// instead send an operator hunting for a deleted endpoint.
    #[error("endpoint not granted to this service account")]
    NotGranted,
    /// The result did not fit under the snapshot's `limits.max_rows`.
    ///
    /// An error rather than a truncated 200: a client that believes it has every
    /// row produces wrong numbers with no signal. Driven by
    /// [`egret_core::TabularResult::truncated`], never by comparing `row_count`
    /// to the cap — a count equal to the cap cannot distinguish "exactly at"
    /// from "over".
    #[error("row limit exceeded")]
    RowLimitExceeded { cap: u32 },
    /// The engine's columns no longer match the ones the snapshot froze.
    ///
    /// Refused rather than reshaped. `ServingMeta::columns` advertises the
    /// frozen contract, so serving rows built from a different column set would
    /// return a body that contradicts its own metadata with no signal — the
    /// exact drift the frozen snapshot exists to make visible. The operator's
    /// fix is to re-publish, which is why this is not [`Self::Internal`].
    #[error("published output schema no longer matches the data")]
    SchemaDrift,
    /// `output.naming` maps two frozen columns onto one key.
    ///
    /// `total_revenue` and `totalRevenue` both become `totalRevenue` under
    /// `camel`, and a JSON object keeps only the last — so a column advertised
    /// in `meta.columns` would vanish from every row. Separate from
    /// [`Self::SchemaDrift`] because the fix is different: change `naming` or
    /// alias the column, not re-publish the same thing.
    ///
    /// **This should be refused at PUBLISH time, and this check kept only as a
    /// backstop.** Unlike [`Self::SchemaDrift`] the condition is deterministic
    /// and permanent for a given snapshot — nothing about the world has to
    /// change for it to fire — and it is fully determinable when the snapshot is
    /// built: `lifecycle::snapshot_from` holds the `describe`-resolved `columns`
    /// and the `output` spec in the same window (`lifecycle.rs:910`,
    /// `:962-967`). As it stands an endpoint publishes green and then fails
    /// every single call, which is the worst place to learn it. Recorded as P3
    /// territory rather than fixed here.
    #[error("output naming maps two columns onto one key")]
    OutputNamingCollision,
    /// The caller exhausted a published rate limit (spec §6.2 step 5).
    ///
    /// Carries the wait rather than a bare status, because that is the whole
    /// difference between this and the login-style lockout in
    /// `egret-auth`: a client refused here is retryable as soon as its bucket
    /// refills, and `Retry-After` is how it learns when.
    #[error("rate limited")]
    RateLimited { retry_after: std::time::Duration },
    /// A `cache.scope = shared` version whose frozen
    /// `(delegated, options.impersonate)` pair no longer matches the live
    /// `query.databases` row.
    ///
    /// **Both halves, not just `delegated`.** Either one on its own makes the
    /// engine run the query as the caller — see
    /// [`crate::lifecycle::runs_per_principal`] — so either drifting is enough
    /// to turn a published shared cache into a cross-identity leak. This doc
    /// said `delegated` alone for as long as the check did, and the check being
    /// fixed without the doc is how the next reader learns the old rule.
    ///
    /// Refused rather than downgraded to a per-principal key: the operator
    /// published a configuration that is no longer safe, and serving it a
    /// different way hides that from the person whose job it is to fix it (spec
    /// §6.3). 503 with code `cache_scope_stale` — the endpoint is not
    /// permanently broken, its configuration has drifted, and a re-publish (or
    /// putting the connection back) clears it. The wire message names both
    /// settings for the same reason this doc does: an operator sent to look at
    /// `delegated` after changing `impersonate` finds it unchanged and concludes
    /// the error is wrong.
    #[error("cache scope is stale")]
    CacheScopeStale,
    /// Something on egret's side failed — the IdP key store was unreachable, say.
    /// Deliberately NOT folded into [`Self::Unauthorized`]: a 401 tells an
    /// operator their credential is bad and sends them to rotate it, while the
    /// actual fault is ours. Task 5 maps this to a 500 with a fixed message.
    #[error("internal error")]
    Internal(#[source] anyhow::Error),
}

impl ServingError {
    /// Wrap any foreign failure as [`Self::Internal`], keeping its message for
    /// the log and out of the response.
    ///
    /// Generic over `Display` rather than one `From` impl per source type
    /// because the sources are unrelated crates (sqlx, serde_json,
    /// `egret_core::Error`) and a `From` for each would let a future caller
    /// convert with `?` at a site where the right answer is a 404 or a 403.
    /// Writing `map_err(ServingError::internal)` makes "this really is our
    /// fault" a decision at the call site.
    pub(crate) fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal(anyhow::anyhow!("{e}"))
    }
}
