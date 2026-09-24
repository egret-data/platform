//! Compiles the `perm` model into Ranger access-policy JSON — the security
//! boundary: this output is what Trino/StarRocks actually enforce, so a
//! wrong grant here is not a wrong screen, it is data reaching someone it
//! should not with nothing in egret showing it.
//!
//! This module builds typed `egret_policy::admin::ranger_json` values
//! (`AccessPolicyInput`/`AccessItemInput`/`ResourceLevel`) and leaves
//! rendering those to JSON to that existing code — nothing here hand-writes
//! Ranger JSON. See `grants.rs` for `compile_grant`, and
//! `crate::actions` for the `Action` → Ranger access-type map it consumes.

pub mod grants;

pub use grants::{compile_grant, CANONICAL_ADMIN_PRINCIPAL};
