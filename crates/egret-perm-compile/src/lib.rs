//! Compiles the `perm` model (`egret-perm`) into Ranger access-policy JSON —
//! the security boundary: this output is what Trino/StarRocks actually
//! enforce, so a wrong grant here is not a wrong screen, it is data reaching
//! someone it should not with nothing in egret showing it.
//!
//! **Split out of `egret-perm` itself (SP4a Task 6 fix round 1, Important 3).**
//! `egret-perm`'s model/store half (grants, rules, `sync_assignment_grants`)
//! touches only `perm.*` tables and no Ranger JSON at all; only this half
//! ever needed `egret_policy`. Before the split, `egret-perm` carried
//! `egret_policy` as a plain dependency, so anything that only wanted
//! `sync_assignment_grants` (`egret-engine-mgmt`, wired in this same fix
//! round) pulled `egret-policy`'s internals into its PRODUCTION dependency
//! graph — exactly what `egret-policy-api::PolicyPort` exists to prevent
//! (`egret-policy` is a *dev*-only dependency of `egret-engine-mgmt`
//! everywhere else). This crate is the seam: it depends on `egret-perm`
//! (for the model types and `CompileError`/`EngineType`/`engine_accesses`,
//! none of which need Ranger) and on `egret-policy`, and nothing depends on
//! this crate except its own tests. `egret-perm` itself now carries no
//! `egret-policy` dependency at all.
//!
//! This module builds typed `egret_policy::admin::{AccessPolicyInput,
//! AccessItemInput, ResourceLevel, MaskingPolicyInput, RowFilterPolicyInput,
//! ...}` values and leaves RENDERING those to JSON to that existing code —
//! nothing here hand-writes a Ranger policy body from scratch. `rules.rs`'s
//! `stamp_service_type` is the one place this module touches an already-
//! rendered `Value` after the fact, fixing a `serviceType` the renderer
//! itself hardcodes to `"trino"` — that is a targeted correction of one
//! field, not hand-written JSON. See `grants.rs` for `compile_grant` (grants
//! → access policies) and `rules.rs` for `compile_rule` (named rules →
//! masking/row-filter policies, with the StarRocks tag expansion), and
//! `egret_perm::actions` for the `Action` → Ranger access-type map the
//! former consumes.

pub mod grants;
pub mod rules;

pub use grants::compile_grant;
pub use rules::{compile_rule, ResolvedNames, TaggedColumn};
