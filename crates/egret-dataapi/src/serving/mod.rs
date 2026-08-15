//! P4 — the Data API **serving** surface: the public, token-authenticated path
//! that runs a published endpoint. Everything under `/api` is session- and
//! CSRF-protected and belongs to a human; this surface belongs to a machine and
//! is authenticated by a bearer token alone.
//!
//! Task 2 lands [`auth`]: bearer token → [`ServingPrincipal`]. Task 3 lands
//! [`router`]: URL → published snapshot → rows. Task 4 lands [`cache`] and
//! [`ratelimit`] — steps 6 and 5 of spec §6.2. The full error envelope and the
//! access log arrive in Task 5.

pub mod auth;
pub mod cache;
pub mod ratelimit;
pub mod router;

pub use auth::{verify_serving_token, ServingPrincipal, DATA_API_AUDIENCE};
pub use cache::{cache_key, shared_cache_allowed, CachedResponse, ResponseCache};
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
/// **Task 5 owns the wire mapping** (`serving/error.rs`): the fixed code table
/// from spec §7, the `{ "error": { code, message, request_id } }` envelope and
/// the remaining variants (`ParamInvalid`, `ParamMissing`, `EngineUnavailable`,
/// `Timeout`). [`Self::RateLimited`] and [`Self::CacheScopeStale`] arrived with
/// Task 4 because its two mechanisms cannot answer without them. What is here
/// is the minimum the handler needs to answer at all — deliberately a plain
/// status + fixed string, so that nothing about it reads as a finished envelope.
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
    /// A `cache.scope = shared` version whose frozen `delegated` no longer
    /// matches the live `query.databases.delegated`.
    ///
    /// Refused rather than downgraded to a per-principal key: the operator
    /// published a configuration that is no longer safe, and serving it a
    /// different way hides that from the person whose job it is to fix it (spec
    /// §6.3). 503 with code `cache_scope_stale` — the endpoint is not
    /// permanently broken, its configuration has drifted, and a re-publish
    /// clears it.
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
