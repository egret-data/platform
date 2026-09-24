//! Compiles the `perm` model into Ranger access-policy JSON — the security
//! boundary: this output is what Trino/StarRocks actually enforce, so a
//! wrong grant here is not a wrong screen, it is data reaching someone it
//! should not with nothing in egret showing it.
//!
//! This module builds typed `egret_policy::admin::{AccessPolicyInput,
//! AccessItemInput, ResourceLevel}` values and leaves rendering those to
//! JSON to that existing code — nothing here hand-writes Ranger JSON. See
//! `grants.rs` for `compile_grant` (grants → access policies) and
//! `rules.rs` for `compile_rule` (named rules → masking/row-filter
//! policies, with the StarRocks tag expansion), and `crate::actions` for the
//! `Action` → Ranger access-type map the former consumes.

pub mod grants;
pub mod rules;

pub use grants::compile_grant;
pub use rules::{compile_rule, TaggedColumn};
