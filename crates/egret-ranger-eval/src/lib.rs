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
//! [`resource`] module doc).
//!
//! [`eval`] is the third layer and the one that answers: policy-item
//! evaluation, precedence and the final three-valued
//! [`eval::Decision`].
//! `Allowed | Denied | Undecidable { why }` is the whole point of the
//! crate: anything it cannot model names itself as an
//! [`eval::UndecidableCause`] instead of being guessed at, and a policy
//! carrying an [`model::Unmodelled`] construct makes the decision
//! undecidable even when another policy would have allowed.
//!
//! [`eval`] answers Ranger's other two questions too —
//! [`eval::evaluate_data_mask`] and [`eval::evaluate_row_filter`] — on the
//! same contract but with a different shape of answer, because a mask is
//! DATA (`maskType` / `maskedValue` / `maskCondition`) and not a verdict:
//! `RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult` reports
//! allow unconditionally, so the allow half carries no information.
//! Ranger keeps the three worlds in three disjoint evaluator lists and
//! neither can see the others.
//!
//! The engine is an explicit input ([`eval::EngineType`],
//! [`eval::PluginVersion`]), not a constant: Trino and StarRocks name their
//! middle resource level differently (`schema` vs `database`), their
//! access-type vocabularies are partly disjoint, and they load different
//! `ranger-plugins-common` versions (2.5.0 and 2.8.0, both measured on the
//! running pods).

pub mod eval;
pub mod model;
pub mod resource;
pub mod servicedef;

/// SP4d-2b Task 2's tag-phase tests, in-crate because the three functions they
/// exercise are `pub(crate)` — see the module doc for the boundary argument.
#[cfg(test)]
mod tag_eval_tests;
