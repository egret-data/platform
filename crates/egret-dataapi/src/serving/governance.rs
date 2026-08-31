//! The serving half of Package 1's governance gate: what happens when an
//! endpoint that was safe at publish stops being safe afterwards.
//!
//! # The rule, and where its other half lives
//!
//! A `governed` database carries data the ENGINE filters per principal — Ranger
//! row filters, column masks. A served request runs through `execute_as`, which
//! runs the SQL as the SERVICE ACCOUNT unless the connection is delegated or
//! impersonating (`egret-query/src/execution/mod.rs:44-88`), so on a connection
//! that is neither, those filters are evaluated for the machine identity and
//! every consumer reads exactly what the machine may read.
//!
//! [`crate::lifecycle::check_governed_connection`] refuses that configuration at
//! publish, which is the strong half: the unsafe endpoint never enters
//! `endpoint_versions`. This module exists because publish-time checking cannot
//! be the whole guard.
//!
//! # Why a publish-time check is not enough — the drift case
//!
//! `governed` and `delegated` are both mutable columns on `query.databases`, and
//! nothing re-validates a published version when either moves. Two edits, each
//! entirely reasonable on its own, turn a legitimately published endpoint into
//! the defect:
//!
//! * an admin marks an existing database `governed` — the endpoints already
//!   published onto it were never asked the question;
//! * someone turns `delegated` off, or drops `options.impersonate`, on a
//!   database that stayed governed.
//!
//! In both, the endpoint keeps answering `200` with rows the consumer's own
//! policy would not have returned, and nothing anywhere says so.
//!
//! # Why this does NOT use the frozen-witness shape, and the departure is
//! deliberate
//!
//! The precedent in this crate is [`crate::serving::shared_cache_allowed`]: it
//! freezes `(delegated, impersonate)` into the snapshot at publish and, at serve
//! time, compares the live pair against the frozen one, refusing on
//! disagreement. That is exactly right *for that rule*, because the unsafe half
//! of it — `cache.scope = shared` — lives IN the snapshot. The live row is the
//! only thing that can move, so "has it moved?" is the whole question.
//!
//! Governance is not that question, and asking it that way would fail in the
//! most common case. Both halves of `governed && !runs_per_principal(…)` are
//! live columns. An endpoint published against an ungoverned, non-delegated
//! database would freeze `governed: false, delegated: false`; when an admin
//! marks the database governed, `delegated` has not moved, so a frozen-pair
//! comparison **agrees** and stays silent — at the precise moment the endpoint
//! became unsafe. A witness for `governed` itself would not save it either:
//! `frozen governed == false, live governed == true` is a disagreement, but so
//! is the harmless reverse (a database DE-governed after publish), and a check
//! that refuses on inequality would take a safe endpoint offline for becoming
//! safer.
//!
//! So this evaluates the SAFETY PREDICATE against the live row rather than
//! comparing the live row to a frozen one. Nothing is frozen for it, and
//! `EndpointSnapshot` gains no field. A pleasant consequence, and the reason
//! this direction needs no `param_gate`-style stamp: every version ever
//! published — including every one frozen before this module existed — is
//! checked, because the check reads nothing the old snapshots lack.
//!
//! # Refuse, rather than warn
//!
//! The three candidates were refuse, serve-with-a-recorded-warning, and some
//! form of downgrade. There is no downgrade: the query has one identity and it
//! is not the caller's. A warning means governed rows keep flowing under the
//! machine identity while a log line accumulates somewhere nobody is reading —
//! the defect this package exists to close, with a paper trail attesting it was
//! known. So: refuse, `503 governance_stale`, and say which three edits clear it
//! (see [`ServingError::GovernanceStale`], which explains why "re-publish" is
//! NOT among them).
//!
//! # Cost — ZERO extra round trips, which took a second attempt
//!
//! The first version read `query.databases` in its own `SELECT`, adding one
//! metastore round trip to **every** served request — where a `per_principal`
//! endpoint, the common case, previously paid none. The metastore pool is the
//! first ceiling the scale audit predicted (~13 round trips per request against
//! a default pool of 10, shared with all of egret), so a 14th on the hot path is
//! not a rounding error; the memo that would have relieved it (`database_id →
//! LiveConnection`, short TTL) was written down and, mercifully, not needed.
//!
//! The three columns now arrive on `router::live_snapshot`'s existing routing
//! query, through a `LEFT JOIN query.databases ON id = snapshot->>'database_id'`.
//! One statement, no extra trip, and the snapshot and the connection are read at
//! ONE moment instead of two — which also removes the (harmless but real)
//! possibility of the two live-connection rules seeing different rows.
//!
//! **What moved is the READ, not the CHECK.** The read now happens at step 2,
//! ahead of the invoke grant and the rate limiter, because that is where the
//! routing query is; it costs those refused requests nothing they were not
//! already paying, since it is the same statement. Both checks still run exactly
//! where they ran: after the limiter and **before the cache lookup**, which is
//! not negotiable for the same reason it is not negotiable in `cache` — checking
//! after a hit means serving the governed rows first and asking afterwards.

use super::ServingError;

/// The three `query.databases` columns the serving path's safety decisions are
/// made from, as they are RIGHT NOW.
///
/// Deliberately not [`crate::lifecycle::ConnectionSafety`], though it carries
/// the same three booleans. That type is the publish-time input and is named for
/// a decision that is made once; this one is named for the fact that matters
/// here — it was read on this request and is already potentially out of date. A
/// reader who sees them as interchangeable is one refactor away from caching the
/// publish-time value on a request path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveConnection {
    pub governed: bool,
    pub delegated: bool,
    pub impersonate: bool,
}

impl LiveConnection {
    /// Build one from the three `query.databases` columns as `router`'s routing
    /// query returned them: `None` for any of them means the `LEFT JOIN` matched
    /// nothing, i.e. the row is gone.
    ///
    /// **There is no `load` any more, and there must not be a second one.** The
    /// first version of this module read the row in its own `SELECT`, which cost
    /// every served request an extra metastore round trip at the exact place the
    /// scale audit named as the first ceiling. The columns now ride in on
    /// `live_snapshot`'s `LEFT JOIN` — zero extra round trips, and the snapshot
    /// and the connection are read at ONE moment rather than two. Adding a
    /// separate reader back would restore both costs and reintroduce the thing
    /// this type's own comment warns about: two answers to one question.
    ///
    /// `governed` and `delegated` are `NOT NULL` in the schema, so a `None` in
    /// either can only mean an unmatched outer join; `governed` is the one
    /// discriminated on and the others follow it.
    pub fn from_row(
        governed: Option<bool>,
        delegated: Option<bool>,
        options: Option<serde_json::Value>,
    ) -> Option<Self> {
        Some(Self {
            governed: governed?,
            delegated: delegated.unwrap_or(false),
            impersonate: options
                .as_ref()
                .is_some_and(crate::lifecycle::impersonate_flag),
        })
    }

    /// Whether the engine runs this connection's queries as the caller.
    ///
    /// Routed through [`crate::lifecycle::runs_per_principal`] rather than
    /// re-spelled as `delegated || impersonate`, so there stays exactly one
    /// expression of "runs as the caller" in the crate — the thing whose absence
    /// was a live cross-identity leak the last time this question was asked in
    /// two places.
    pub fn runs_per_principal(&self) -> bool {
        crate::lifecycle::runs_per_principal(self.delegated, self.impersonate)
    }
}

/// May this request be served, given the connection as it is right now?
///
/// `Err(GovernanceStale)` when the database is governed and the connection does
/// not run queries as the caller — see the module comment for why that predicate
/// is evaluated live rather than compared against a frozen witness, and why the
/// answer is a refusal rather than a warning.
///
/// # `None` — the database row is gone — is NOT refused here
///
/// The shared-cache check refuses on a missing row, and this one does not. The
/// asymmetry is intended and it is not a fail-open: a row that does not exist
/// cannot be marked `governed`, and the request is already doomed — the very
/// next thing this path does is ask the query port to execute against that
/// `database_id`, which fails on its own. Answering `governance_stale` here
/// would name a flag on a row the operator cannot go and look at, and would hide
/// the one fact that explains everything they are seeing: the connection was
/// deleted. Refusal is reserved for the state it can actually describe.
///
/// **"Doomed at execution" is load-bearing, and it had to be MADE true.** As
/// first written it was false on one path: a `per_principal` snapshot returns
/// from [`crate::serving::shared_cache_allowed`] before that function's
/// missing-row refusal, so a warm cache entry answered `200` for a deleted
/// database without any execution to be doomed by. [`crate::serving::cache_ttl`]
/// now takes the live connection and answers `None` when there is no row, so
/// nothing is served from — or written to — the cache for a connection that no
/// longer exists, and the sentence above describes every request rather than
/// most of them. That function's doc carries the reasoning and the two
/// alternatives it was chosen over.
pub fn governance_allows_serving(live: Option<LiveConnection>) -> Result<(), ServingError> {
    let Some(live) = live else {
        return Ok(());
    };
    if live.governed && !live.runs_per_principal() {
        tracing::warn!(
            governed = live.governed,
            delegated = live.delegated,
            impersonate = live.impersonate,
            "a published endpoint's database is governed while its connection runs queries as \
             the service account; refusing to serve rather than reading governed data under a \
             machine identity"
        );
        return Err(ServingError::GovernanceStale);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(governed: bool, delegated: bool, impersonate: bool) -> LiveConnection {
        LiveConnection {
            governed,
            delegated,
            impersonate,
        }
    }

    /// The one combination that must be refused, and the seven that must not.
    ///
    /// Enumerated rather than sampled because the predicate has exactly eight
    /// inputs and the interesting property is which ONE of them refuses: a
    /// mutation that widens the rule (`live.governed`, dropping the
    /// `runs_per_principal` half) turns three more of these green-side rows red,
    /// and a mutation that narrows it (`false &&`) turns the refusing row green.
    #[test]
    fn only_governed_and_not_per_principal_is_refused() {
        for governed in [false, true] {
            for delegated in [false, true] {
                for impersonate in [false, true] {
                    let live = conn(governed, delegated, impersonate);
                    let refused = matches!(
                        governance_allows_serving(Some(live)),
                        Err(ServingError::GovernanceStale)
                    );
                    let should_refuse = governed && !delegated && !impersonate;
                    assert_eq!(
                        refused, should_refuse,
                        "governed={governed} delegated={delegated} impersonate={impersonate}"
                    );
                }
            }
        }
    }

    /// `impersonate` alone is enough, which is the half a reader who only knows
    /// `delegated` will get wrong — it is what made the shared-cache check a
    /// live leak once already.
    #[test]
    fn impersonate_alone_satisfies_a_governed_database() {
        assert!(governance_allows_serving(Some(conn(true, false, true))).is_ok());
    }

    /// A deleted connection is not a governance refusal. See the doc comment:
    /// the request fails on its own at execution, and naming a flag on a row
    /// that does not exist sends the operator to the wrong place.
    #[test]
    fn a_missing_database_row_is_not_a_governance_refusal() {
        assert!(governance_allows_serving(None).is_ok());
    }
}
