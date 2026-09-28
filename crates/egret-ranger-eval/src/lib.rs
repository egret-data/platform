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
//! model must be detected and reported (see [`model::Unmodelled`]), never
//! silently dropped, because a silently dropped construct produces a
//! confident wrong decision downstream. Later tasks build resource
//! matching, precedence and mask/row-filter evaluation on top of these
//! types; no evaluation logic lives here yet.

pub mod model;
