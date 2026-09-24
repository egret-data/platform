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

pub use actions::{engine_accesses, CompileError, EngineType};
pub use assignment::{sync_assignment_grants, sync_assignment_grants_in_tx};
pub use error::PermError;
pub use eval::{evaluate, Decision, Reason, Subject};
pub use model::{
    Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Rule, RuleKind,
    Scope, Selector,
};
pub use store::{list_grants_via, list_rules_via, PermStore};
