//! egret's first data-permission model (SP4a). Grants and named rules over
//! `registry.catalogs`, stored in the `perm` schema (`migrations/0154_perm_model.sql`).
//! `policy.*` keeps serving raw Ranger JSON for hand-written profiles; `perm.*`
//! is the model a later task COMPILES into it. One direction only — nothing in
//! this crate reads Ranger JSON back into `perm`.
//!
//! **This crate carries no `egret-policy` dependency** (SP4a Task 6 fix
//! round 1, Important 3). The compiler that turns a `Grant`/`Rule` into
//! Ranger JSON lives in the sibling crate `egret-perm-compile`, which
//! depends on both this crate and `egret-policy`; this crate's model/store
//! half (`sync_assignment_grants` included) needs neither Ranger types nor
//! Ranger JSON rendering, and staying free of that dependency is what lets
//! `egret-engine-mgmt` depend on this crate in production without pulling
//! `egret-policy`'s internals along — the same boundary
//! `egret-policy-api::PolicyPort` draws for every other production
//! consumer of `egret-policy`.
pub mod actions;
pub mod assignment;
pub mod error;
pub mod eval;
pub mod model;
pub mod store;

pub use actions::{
    engine_accesses, modelled_ranger_plugins_sentence, ranger_plugin_is_modelled, CompileError,
    EngineType, OverlapKind, OverlapRule, EXTERNAL_PLUGIN_ASSUMPTIONS, MODELLED_RANGER_PLUGINS,
};
pub use assignment::{sync_assignment_grants, sync_assignment_grants_in_tx};
pub use error::PermError;
// **`evaluate` is deliberately NOT re-exported here (SP4d-2a Task 5).** It was
// the function `/perm/explain` answered with, and `explain` now evaluates the
// compiled artifact each engine downloads (`egret-ranger-eval`) instead — two
// implementations of one question had already been measured disagreeing in
// production. `eval::evaluate` survives as an ORACLE: tests still compare the
// model's own answer against the artifact's
// (`egret-perm-compile/tests/routes.rs`'s
// `the_artifact_answer_and_the_model_oracle_agree_on_the_happy_path`), and
// `crates/egret-perm/tests/eval.rs` still pins its semantics. Dropping it from
// the root re-export is what stops a new production caller reaching for it by
// reflex; `egret_perm::eval::evaluate` remains the path for a test that wants
// the oracle on purpose.
//
// `PUBLIC_GROUP` stays exported: it is genuine COMPILER code
// (`egret_perm_compile::clearance` emits it as the one principal a Ranger deny
// can name that matches everybody), not part of the retired evaluator.
pub use eval::{ClearanceGate, Decision, Reason, Subject, PUBLIC_GROUP};
pub use model::{
    Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Rule, RuleKind,
    Scope, Selector,
};
pub use store::{
    compiled_service_ids_for_catalog_via, create_grant_in_tx, create_rule_in_tx,
    delete_grant_in_tx, delete_rule_in_tx, get_grant_via, get_rule_via, list_grants_via,
    list_rules_via, update_grant_in_tx, update_rule_in_tx, PermStore,
};
