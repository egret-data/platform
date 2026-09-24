//! egret's first data-permission model (SP4a). Grants and named rules over
//! `registry.catalogs`, stored in the `perm` schema (`migrations/0154_perm_model.sql`).
//! `policy.*` keeps serving raw Ranger JSON for hand-written profiles; `perm.*`
//! is the model a later task COMPILES into it. One direction only — nothing in
//! this crate reads Ranger JSON back into `perm`.
pub mod error;
pub mod eval;
pub mod model;
pub mod store;

pub use error::PermError;
pub use eval::{evaluate, Decision, Reason, Subject};
pub use model::{
    Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Rule, RuleKind,
    Scope, Selector,
};
pub use store::PermStore;
