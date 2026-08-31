//! P4 — the Data API **serving** surface: the public, token-authenticated path
//! that runs a published endpoint. Everything under `/api` is session- and
//! CSRF-protected and belongs to a human; this surface belongs to a machine and
//! is authenticated by a bearer token alone.
//!
//! Task 2 lands [`auth`]: bearer token → [`ServingPrincipal`]. Task 3 lands
//! [`router`]: URL → published snapshot → rows. Task 4 lands [`cache`] and
//! [`ratelimit`] — steps 6 and 5 of spec §6.2. Task 5 lands [`error`] (spec §7's
//! envelope and its fixed code table) and [`access_log`] (spec §8).
//!
//! P4.5 lands [`params`] — step 7's first half, where a request and a snapshot's
//! `ParamSpec`s become the values [`crate::render::render`] binds and the bytes
//! the cache keys on. Read its module comment before touching either: the two
//! projections come from one pass on purpose, and the reason is a wrong-rows bug.

pub mod access_log;
pub mod auth;
pub mod cache;
pub mod error;
pub mod governance;
/// Prometheus instrumentation for this surface. `pub` because the composition
/// root calls [`metrics::describe`] once at startup; everything else in it is
/// crate-private.
pub mod metrics;
pub(crate) mod params;
pub mod ratelimit;
pub mod router;

pub use access_log::{scheduled_retention, AccessLogEntry, AccessLogStore, Recorded};
pub use auth::{verify_serving_token, ServingPrincipal, DATA_API_AUDIENCE};
pub use cache::{cache_key, shared_cache_allowed, CachedResponse, ResponseCache};
pub use error::{ServingErrorBody, ServingErrorDetail};
pub use governance::{governance_allows_serving, LiveConnection};
pub use ratelimit::{Rate, RateLimitStore, TokenBucket};
pub use router::{
    serving_openapi, serving_router, MetaContract, ServingMeta, ServingResponse, ServingStatus,
    StatementMeta,
};

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
/// # The two variants spec §7 named and P4 could not produce — now it can
///
/// `engine_unavailable` (502) and `timeout` (504) were recorded here for four
/// phases as "not decidable at this seam", and the reason was true: `port_impl`
/// folded the engine's infrastructure failures **and** egret's own faults into
/// one `Error::Internal`, so a 502 would have paged an operator about Trino when
/// the metastore was what failed; and `EngineError::Timeout` arrived as
/// `Error::InvalidInput("query timed out")`, so recognising it would have meant
/// matching another crate's message string. Package 3b widened the port —
/// additively, via `egret_query_api::ExecFailure` inside the existing
/// `Error::Internal` — so both causes now arrive as values. They are
/// [`Self::EngineUnavailable`] and [`Self::EngineTimeout`], and
/// [`Self::EndpointQueryFailed`] is the third code that widening required (spec
/// §7's requirement (b): an author-fault code that is not a 4xx).
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
    /// The engine refused the DATA, not the request. Kept apart from
    /// [`Self::Internal`] because the operator's fix is a permission, and apart
    /// from any "bad input" code because the author's SQL is fine — telling them
    /// otherwise sends them to debug the wrong thing.
    ///
    /// # One code for two recognitions, and for two causes
    ///
    /// `router::engine_error` reaches this variant two ways, and the wire code
    /// is the same for both because a caller can act on neither distinction:
    ///
    /// * **Typed.** Trino's connector (`connector/trino.rs`'s `send()`) returns
    ///   `egret_core::Error::Forbidden` when the COORDINATOR's own HTTP response
    ///   is 401 or 403.
    /// * **Untyped, by message.** Everything else — StarRocks entirely, and a
    ///   Trino denial that instead surfaces as a query-level failure inside a
    ///   200 — arrives as `Error::InvalidInput` carrying the engine's own words,
    ///   which [`super::error::is_permission_denied`] recognises. See that
    ///   function's doc for why a substring match is the only tool available and
    ///   why it is documented as a heuristic, not a certainty.
    ///
    /// **The code is deliberately NOT named for Ranger.** The typed arm cannot
    /// tell a Ranger/file-ACL deny from a bad or expired engine credential —
    /// Trino answers 401 and 403 through the same connector path — so a
    /// `ranger_denied` code would state the wrong cause in half the cases it
    /// fires. The wire message names both causes and the remedy for each
    /// instead; see [`super::error::to_wire`].
    ///
    /// Distinct from [`Self::NotGranted`], which is two different people's
    /// problem with a different fix. `NotGranted` means egret refused the CALL
    /// before the engine was ever contacted — the fix is
    /// `POST …/endpoints/{id}/grants`. This variant means the engine refused the
    /// DATA — the fix is in the engine's own permission system, or in the
    /// service account's engine credential. Conflating them would send an admin
    /// to write an egret grant that already exists.
    ///
    /// # `raw` — the only 4xx that carries anything, and why
    ///
    /// The wire body never renders it — [`super::error::to_wire`] ignores the
    /// field, same as [`Self::Internal`] ignores its own source — but a
    /// message-based heuristic can false-positive, and a false positive here
    /// sends an admin to edit engine policy for a failure that was actually,
    /// say, malformed SQL. Discarding the message at classification time would
    /// leave production with no way to tell that had happened; keeping it lets
    /// the log at `serve`'s failure branch (`error = ?e`) record what the engine
    /// actually said, next to the fixed code and message the caller received.
    ///
    /// **`None` on the typed arm, and that is a statement rather than an
    /// oversight.** `egret_core::Error::Forbidden` is a unit variant: there is
    /// no engine text to keep. `Option` makes that absence explicit at every
    /// construction site instead of letting a future edit invent a placeholder
    /// string that reads like something the engine said.
    #[error("data access denied")]
    DataAccessDenied {
        #[source]
        raw: Option<anyhow::Error>,
    },
    /// **egret** refused the CALL: the service account holds no
    /// `data_api_endpoint:invoke` grant on this endpoint (spec §6.2 step 4).
    ///
    /// Kept distinct from [`Self::DataAccessDenied`] because they are two
    /// different people's problems with two different fixes, and collapsing them
    /// would send an operator to the wrong system:
    ///
    /// * `DataAccessDenied` — the ENGINE refused the DATA. egret let the call
    ///   through, the query ran as the service account, and the engine refused
    ///   the relation (or would not authenticate the credential at all). The fix
    ///   is a data permission — a Ranger grant, say — or a working credential.
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
    /// A `required` parameter the caller did not supply and the author gave no
    /// `default` for (spec §5, serve-time rules).
    ///
    /// **Carries nothing, deliberately.** The parameter's name is the endpoint
    /// author's, and putting it in a 400 turns the error surface into a
    /// directory of which parameters an endpoint declares — the same free
    /// oracle the single [`Self::Unauthorized`] and the single [`Self::NotFound`]
    /// exist to close. A unit variant leaves nothing for a later edit to
    /// interpolate out of habit; making it name a parameter would have to be a
    /// deliberate change to this type.
    #[error("a required parameter is missing")]
    ParamMissing,
    /// A supplied value that does not satisfy its `ParamSpec.ty` — a `Date` that
    /// is not a date, a `Number` that is not one, an `Enum` value outside
    /// `allowed`, a repeat of an `Enum`, or input past the size caps in
    /// [`super::params`] — **or** one that satisfies its `ty` but fails a
    /// declared `minimum`/`maximum`/`min_length`/`max_length` constraint
    /// (`CoerceError::Constraint`, mapped here with its `constraint`/`bound`
    /// detail dropped for the same reason as everything else on this line).
    ///
    /// Unit for the same reason as [`Self::ParamMissing`], and additionally
    /// because echoing the VALUE back would reflect caller-controlled bytes into
    /// a response body.
    #[error("a parameter value is invalid")]
    ParamInvalid,
    /// The result did not fit under the snapshot's `limits.max_rows`.
    ///
    /// An error rather than a truncated 200: a client that believes it has every
    /// row produces wrong numbers with no signal. Driven by
    /// [`egret_core::TabularResult::truncated`], never by comparing `row_count`
    /// to the cap — a count equal to the cap cannot distinguish "exactly at"
    /// from "over".
    #[error("row limit exceeded")]
    RowLimitExceeded { cap: u32 },
    /// The engine's columns are no longer WITHIN the ones the snapshot froze.
    ///
    /// Refused rather than reshaped. `meta` advertises the frozen contract —
    /// since M1 the union of every branch described at publish, and since M2
    /// under `columns` for a one-statement endpoint and
    /// `statements.<name>.columns` for several — so a body may legitimately carry a SUBSET of those columns: a
    /// branch that returns fewer is the endpoint's own documented behaviour, and
    /// refusing it was the defect M1 fixed. What it may never carry is a name
    /// that is not in the union, a declared column under a different type, fewer
    /// than the columns flagged `required`, two columns of one name, or no
    /// columns at all. Each of those is a body contradicting its own metadata
    /// with no signal — the exact drift the frozen snapshot exists to make
    /// visible. The operator's fix is to re-publish, which is why this is not
    /// [`Self::Internal`].
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
    /// The endpoint's database is marked `governed` right now, and the
    /// connection does not run queries as the caller right now.
    ///
    /// `execute_as` would run the SQL as the SERVICE ACCOUNT, so the engine's
    /// row filters and column masks would be evaluated for the machine identity
    /// and every consumer of this endpoint would read whatever that identity may
    /// read. [`crate::lifecycle::check_governed_connection`] makes that
    /// unpublishable; this is what happens when it becomes true AFTER a publish,
    /// which it can, because both columns are mutable.
    ///
    /// **Refused, not warned.** The three options were refuse, serve with a
    /// recorded warning, or downgrade somehow, and only the first is
    /// defensible: a warning means governed rows keep flowing under a machine
    /// identity while a log line accumulates somewhere nobody reads, which is
    /// precisely the defect this package exists to close, now with a paper trail
    /// saying it was known. There is nothing to downgrade to — the query has one
    /// identity and it is not the caller's. 503 rather than 500 for the same
    /// reason as its three neighbours: nothing failed, the configuration drifted,
    /// and a human clears it by turning delegation on, moving the endpoint to a
    /// delegated connection, or clearing `governed`.
    ///
    /// Distinct from [`Self::CacheScopeStale`] because the fixes are disjoint.
    /// `cache_scope_stale` is cleared by RE-PUBLISHING; re-publishing does not
    /// clear this one — [`crate::lifecycle::check_governed_connection`] would
    /// refuse the re-publish, telling the operator the same thing again. Sending
    /// them to do something that cannot work is the one failure a drift message
    /// must not have.
    #[error("this endpoint's database is governed and its connection is not per-principal")]
    GovernanceStale,
    /// A version that declares parameters but was frozen before the
    /// publish-time parameter gate existed, or before its current revision —
    /// `EndpointSnapshot::param_gate < PARAM_GATE`.
    ///
    /// **The alternative was an instruction.** Spec §7 said every endpoint with
    /// parameters must be re-published after P4.5, because publish-time
    /// validation is where the new rules live and an older snapshot bypasses all
    /// of them: a `location: body` parameter that silently drops a filter, a
    /// `default` that makes every defaulted call a 400, a `| ident` that is a
    /// 500 whenever the caller omits its parameter. An instruction is not an
    /// enforcement, and the endpoint that nobody remembers to re-publish is
    /// exactly the one that carries the rule nobody checked.
    ///
    /// Refused rather than rendered-and-hoped, and refused rather than
    /// re-validated on the fly: validating here would mean running a
    /// sixty-four-render gate on a request path, and passing it would still
    /// leave the snapshot's own frozen columns resolved by the pre-gate describe.
    ///
    /// 503 with `param_gate_stale`, alongside the other two config-drift 503s:
    /// the endpoint is not broken, its published version predates a rule, and one
    /// re-publish clears it. Not a 500 — nothing failed — and not a 404, which
    /// would tell an operator to look for a deleted endpoint.
    #[error("published before the parameter validation gate")]
    ParamGateStale,
    /// The engine ran the endpoint's published statement and refused it: the SQL
    /// no longer parses, a relation it names has been dropped, a column changed
    /// type, a resource group rejected it.
    ///
    /// # The finding this closes
    ///
    /// This used to be [`Self::Internal`], and it is the largest share of the
    /// six unrelated causes that shared the `internal` code. The cost is not
    /// abstract: an operator reading `dataapi.access_log.error_code` to answer
    /// *"which endpoints are failing?"* could not separate **an author's broken
    /// endpoint** from **egret itself being broken**, and those are two
    /// different people's problems. The design doc has argued this since P4
    /// (`docs/vi/specs/2026-08-14-data-api-design.md` §7, requirement (b)); PR
    /// #552 put the cause in the *log*, which helps someone already reading the
    /// log for one request and does nothing for the column an alert aggregates.
    ///
    /// # 500, and specifically NOT a 4xx
    ///
    /// Requirement (c) of that same paragraph, and it is the whole reason this
    /// variant is not simply `param_invalid`'s neighbour: **the caller did
    /// nothing wrong.** It sent a well-formed request to an endpoint someone
    /// published; the endpoint is what broke. A 4xx would tell a machine client
    /// to fix its own request, which it cannot do, and would make the failure
    /// invisible in every "5xx rate" an operator alerts on.
    ///
    /// Not a 503 either, unlike the four config-drift codes: those are cleared
    /// by an action the message can name (re-publish, restore the connection),
    /// and they carry a `Retry-After` saying so. This one may be permanent (the
    /// table is gone) or transient (the cluster was out of resources), and egret
    /// cannot tell which — so it neither invites a retry nor forbids one.
    ///
    /// # `raw`
    ///
    /// Kept for the same reason [`Self::DataAccessDenied`] keeps its own, and
    /// with the same guarantee: [`super::error::to_wire`] never renders it, so
    /// the engine's words — which name tables and columns — reach the server log
    /// and nothing else.
    #[error("the endpoint's published statement failed at the engine")]
    EndpointQueryFailed {
        #[source]
        raw: Option<anyhow::Error>,
    },
    /// The statement outran the endpoint's own published `limits.timeout`.
    ///
    /// Spec §7 named this `timeout` (504) and recorded why P4 could not produce
    /// it: `QueryExecPort` mapped `EngineError::Timeout` to
    /// `InvalidInput("query timed out")` — a fixed string egret itself wrote —
    /// so recognising it here would have meant matching another crate's message
    /// text, "a check that keeps compiling and silently stops working the day
    /// someone rewords it". `egret_query_api::ExecFailure` closed that: the
    /// cause now arrives as a value.
    ///
    /// **504 and not 500**, because the distinction is real and actionable: the
    /// endpoint works, its budget was too small (or its data grew). The fix is
    /// the author's `limits.timeout`, not a bug report.
    ///
    /// No `raw`: the port's timeout carries no engine text, only egret's own
    /// note of which budget expired.
    #[error("the endpoint's statement exceeded its published time budget")]
    EngineTimeout,
    /// The engine could not be reached at all — connection refused, TLS failure,
    /// a dropped socket.
    ///
    /// Spec §7 named this `engine_unavailable` (502) and recorded why P4 could
    /// not produce it: `port_impl` folded the engine's `Connect` failures and
    /// **egret's own** faults (resolving the principal's email, opening the
    /// database secret, reaching egret's object store) into one
    /// `Error::Internal`, so answering 502
    /// "would page an operator about Trino when the metastore is what failed".
    /// That is now decidable — only the engine's own transport failures carry
    /// `ExecFailureKind::EngineUnavailable`, and egret's faults deliberately
    /// carry nothing and still land on [`Self::Internal`].
    ///
    /// Kept apart from [`Self::EndpointQueryFailed`] because the two send
    /// different people to different systems: this one is an operator's problem
    /// with the engine or the network, that one is the endpoint author's problem
    /// with their statement.
    #[error("the engine could not be reached")]
    EngineUnavailable {
        #[source]
        raw: Option<anyhow::Error>,
    },
    /// The request outran **egret's** serving deadline — not the endpoint's
    /// published budget, which is [`Self::EngineTimeout`].
    ///
    /// # Why this is a variant here and not a string in the middleware
    ///
    /// The request-bounds layer (`serving/guard.rs`, Package 3a) answers a
    /// deadline, a shed and an over-size body from *outside* the handler, and it
    /// built its envelopes directly — bypassing [`super::error::to_wire`]'s
    /// exhaustive match, which is the one place this crate forces a status/code
    /// decision to be made deliberately. Three codes that no `ServingError`
    /// produces are three codes the taxonomy's own tests cannot see, and the
    /// `code` label's bound in `serving/metrics.rs` stops being "that match" and
    /// becomes "that match plus whatever else someone wrote".
    ///
    /// # And its code is NOT `timeout`
    ///
    /// This is the part that would have been expensive later. Both this and
    /// [`Self::EngineTimeout`] are 504s, and the bounds layer called this one
    /// `timeout` too — the same code, the same status, for **two different
    /// actors**: there, egret's own serving clock; here, the budget the endpoint's
    /// author published. `dataapi.access_log.error_code` records one string, so
    /// an operator asking "are we cutting callers off, or are their queries too
    /// slow?" would have had no way to tell — which is the exact question this
    /// package split `internal` to make answerable. So this one is
    /// `deadline_exceeded` and `timeout` stays spec §7's name for the author's
    /// budget.
    #[error("the request exceeded egret's serving deadline")]
    ServingDeadline,
    /// egret shed the request: every concurrency permit on the serving surface
    /// was in use.
    ///
    /// A refusal by egret about egret, and the only 503 here that is not
    /// configuration drift — it clears by itself, so its `Retry-After` is a
    /// second rather than the config-drift floor.
    #[error("the serving surface is at its concurrency limit")]
    Overloaded,
    /// The request declared a body larger than the serving surface accepts.
    ///
    /// The one 413 in the taxonomy, and the one refusal here that really is the
    /// caller's own doing.
    #[error("the request body is larger than the serving surface accepts")]
    PayloadTooLarge,
    /// Something on egret's side failed — the IdP key store was unreachable, say.
    /// Deliberately NOT folded into [`Self::Unauthorized`]: a 401 tells an
    /// operator their credential is bad and sends them to rotate it, while the
    /// actual fault is ours. Task 5 maps this to a 500 with a fixed message.
    ///
    /// **Narrower than it was.** Until Package 3b this was also every engine
    /// failure the seam could not type — see [`Self::EndpointQueryFailed`],
    /// [`Self::EngineTimeout`] and [`Self::EngineUnavailable`], which took three
    /// of those causes away. What remains really is egret: the metastore, the
    /// secret store, a corrupt snapshot, the cache. That narrowing is the point
    /// — `internal` is now a statement about egret rather than a shrug.
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

/// One constructed value per [`ServingError`] variant, and the check that the
/// list is COMPLETE.
///
/// # Why this exists
///
/// Three tests describe the taxonomy as a whole — the spec §7 code table in
/// [`super::error`], the `code` label set and the status label set in
/// [`super::metrics`] — and all three were hand-written lists of variants. Two
/// of them had already gone stale on `main` (`governance_stale` was missing from
/// the metrics list while its assertion still read `codes.len() == 14`, and
/// passed; the status-label list had neither `502` nor `504`, both live since
/// Package 3b). And the comment above the code table claimed a new variant
/// "shows up as a missing line here", which was false: only `to_wire`'s
/// exhaustive match fails to compile, and a `to_wire` arm can be added without
/// touching any list.
///
/// So the lists are one list, and the list is checked against the enum itself.
/// A new variant now fails [`every_declared_variant_has_a_sample`] by name; once
/// sampled, it fails the two tables that have no row for it. That is three
/// failures a new variant cannot land without, in place of three tests that
/// silently kept passing.
///
/// Sampled by NAME, and the name is what the census compares — a sample that
/// constructs the wrong variant under a right name would pass the census and
/// then fail the code table, which reads its expectations by the same name.
#[cfg(test)]
pub(crate) mod census {
    use super::ServingError;

    /// Every variant, each with the name it is declared under. More than one
    /// row per variant is allowed (and used: the two `raw` shapes are two
    /// construction sites), so the census compares the SET of names.
    pub(crate) fn every_variant() -> Vec<(&'static str, ServingError)> {
        vec![
            ("Unauthorized", ServingError::Unauthorized),
            ("NotFound", ServingError::NotFound),
            (
                "DataAccessDenied",
                ServingError::DataAccessDenied { raw: None },
            ),
            (
                "DataAccessDenied",
                ServingError::DataAccessDenied {
                    raw: Some(anyhow::anyhow!(
                        "Access Denied: Cannot select from table sales"
                    )),
                },
            ),
            ("NotGranted", ServingError::NotGranted),
            ("ParamMissing", ServingError::ParamMissing),
            ("ParamInvalid", ServingError::ParamInvalid),
            (
                "RowLimitExceeded",
                ServingError::RowLimitExceeded { cap: 100 },
            ),
            (
                "RateLimited",
                ServingError::RateLimited {
                    retry_after: std::time::Duration::from_secs(2),
                },
            ),
            ("SchemaDrift", ServingError::SchemaDrift),
            ("OutputNamingCollision", ServingError::OutputNamingCollision),
            ("CacheScopeStale", ServingError::CacheScopeStale),
            ("GovernanceStale", ServingError::GovernanceStale),
            ("ParamGateStale", ServingError::ParamGateStale),
            (
                "EndpointQueryFailed",
                ServingError::EndpointQueryFailed { raw: None },
            ),
            (
                "EndpointQueryFailed",
                ServingError::EndpointQueryFailed {
                    raw: Some(anyhow::anyhow!(
                        "line 1:15: Table 'hive.a.b' does not exist"
                    )),
                },
            ),
            ("EngineTimeout", ServingError::EngineTimeout),
            (
                "EngineUnavailable",
                ServingError::EngineUnavailable { raw: None },
            ),
            ("ServingDeadline", ServingError::ServingDeadline),
            ("Overloaded", ServingError::Overloaded),
            ("PayloadTooLarge", ServingError::PayloadTooLarge),
            ("Internal", ServingError::internal("boom")),
        ]
    }

    /// The variant names DECLARED by the enum, read out of this file.
    ///
    /// Reading the source is the only mechanism available that a new variant
    /// cannot pass: `match` is exhaustive but cannot enumerate, and nothing in
    /// this workspace derives an iterator over variants. It is not clever and it
    /// is not fragile in the way that matters — it parses `rustfmt`'s output of
    /// one enum in one file, and if it ever failed to find the enum it fails
    /// LOUD rather than reporting an empty set (see the assertion below).
    fn declared_variants() -> Vec<String> {
        let src =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/serving/mod.rs"))
                .expect("serving/mod.rs must be readable from its own crate");
        let body = src
            .split_once("pub enum ServingError {")
            .expect("the enum this census is about must exist")
            .1;
        let body = body.split_once("\n}").expect("the enum must end").0;
        body.lines()
            .filter_map(|line| {
                // A variant is a line at one level of indentation whose first
                // token starts with a capital: `Unauthorized,`,
                // `RowLimitExceeded { cap: u32 },`, `DataAccessDenied {`,
                // `Internal(#[source] anyhow::Error),`. Field lines are indented
                // deeper and attribute/doc lines start with `#` or `/`.
                let rest = line.strip_prefix("    ")?;
                if rest.starts_with(' ') || rest.starts_with('#') || rest.starts_with('/') {
                    return None;
                }
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                let first = name.chars().next()?;
                first.is_ascii_uppercase().then_some(name)
            })
            .collect()
    }

    /// The census itself: no variant may exist without a sample.
    ///
    /// Mutation: delete any row from `every_variant` and this names it.
    #[test]
    fn every_declared_variant_has_a_sample() {
        use std::collections::BTreeSet;

        let declared: BTreeSet<String> = declared_variants().into_iter().collect();
        // A floor, not a count of today's enum: its only job is to fail loudly
        // if the parser ever stops matching the source, because a census that
        // finds nothing agrees with every sample list there is.
        assert!(
            declared.len() >= 10,
            "the parser found only {} variants, which means it stopped matching \
             the source rather than that the enum shrank: {declared:?}",
            declared.len()
        );
        let sampled: BTreeSet<String> = every_variant()
            .into_iter()
            .map(|(name, _)| name.to_string())
            .collect();

        let missing: Vec<_> = declared.difference(&sampled).collect();
        assert!(
            missing.is_empty(),
            "ServingError variants with no sample in `census::every_variant`: \
             {missing:?}. Every test that describes the taxonomy as a whole \
             iterates that list, so an unsampled variant is a variant those \
             tests cannot see."
        );
        let unknown: Vec<_> = sampled.difference(&declared).collect();
        assert!(
            unknown.is_empty(),
            "`census::every_variant` names variants the enum does not declare \
             (a rename?): {unknown:?}"
        );
    }
}
