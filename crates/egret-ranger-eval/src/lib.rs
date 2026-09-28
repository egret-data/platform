//! `egret-ranger-eval`: a Rust evaluator reproducing Apache Ranger's access
//! decision (SP4d).
//!
//! Pure crate: no sqlx, no tokio, no network, no filesystem access in
//! library code — mirrors the discipline documented at
//! `crates/egret-perm/src/eval.rs:1-64`. Test code may read fixture files.
//!
//! This first module, [`model`], is a typed **reader** for Ranger policy
//! bodies — not the evaluator itself. `policy.authz_policies.policy_jsonb`
//! stores each policy body exactly as the client (Ranger, or egret's own
//! editor) wrote it; nothing in the repo modeled that body for reading
//! before this crate, and the only existing typed shapes
//! (`egret_policy::admin::ranger_json`) are the *writer*'s, documented
//! lossy for policies egret did not author. `model` is read for field
//! names only — never depended on directly — because reusing the writer's
//! types would just relocate the same lossiness into this evaluator.
//!
//! The point of `model`, and of this whole crate: a construct it does not
//! model must be detected and reported (see [`model::Unmodelled`] and
//! [`servicedef::ServiceDefIssue`]), never silently dropped, because a
//! silently dropped construct produces a confident wrong decision
//! downstream.
//!
//! [`servicedef`] and [`resource`] add the second layer: the service-def's
//! resource hierarchy and `matcherOptions`, and Ranger's resource
//! **matching** — `RangerDefaultPolicyResourceMatcher.getMatchType` and the
//! two matchers it drives. That layer classifies a match; it does not
//! decide access. In particular [`resource::MatchType::Ancestor`] — a
//! policy naming fewer hierarchy levels than the request — is NOT an allow:
//! Ranger refuses to record one for an ancestor match on a resource
//! request, and a live Trino measurement confirmed it (see the
//! [`resource`] module doc). Precedence and the final decision are a later
//! task's; mask and row-filter evaluation a later one still.

pub mod model;
pub mod resource;
pub mod servicedef;
