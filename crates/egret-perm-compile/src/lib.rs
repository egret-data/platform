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
//! none of which need Ranger) and on `egret-policy`. `egret-perm` itself
//! carries no `egret-policy` dependency at all.
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
//!
//! **`trigger::recompile_service` (SP4a Task 7)** is this crate's first real
//! caller of `compile_grant`/`compile_rule` outside their own tests — nothing
//! else in the codebase called either before this. It loads a profile's
//! model, resolves the registry/tag reads `compile_grant`/`compile_rule`
//! themselves stay free of (see those modules' doc comments), and diffs the
//! result against `perm.compiled` so a recompile writes only what changed.
//! `egret-bi::catalog_tags` calls it on every catalog-tag write, alongside
//! the `tag_version` bump already there — see `trigger`'s own module doc for
//! why a `WorkspaceMembers`-only refusal is skipped rather than aborting the
//! whole recompile.

// SP4d-2a Task 3: mapping a compiled Ranger policy NAME back to the `perm`
// model object that minted it — pure, no DB, no evaluator. See that module's
// own doc comment for the four live no-model-object shapes it must not choke
// on.
pub mod attribution;
// SP4d-2a Task 4: hand-written wire DTOs mirroring `egret-ranger-eval`'s
// `Decision`/`DenyReason`/`UndecidableCause` and mask/row-filter results —
// see that module's own doc comment for why they are hand-written and not
// derived on the source types.
pub mod explain_wire;
// SP4c Task 2: compiling a classification's clearance into a tag-service DENY
// — the one place egret emits a tag ACCESS policy, which AMENDS SP4a's D2. The
// argument is in that module's own doc comment, at the site.
pub(crate) mod clearance;
// View governance A2: the StarRocks priority-1 clearance MASK per classified
// column, its cleared items folded through `egret-ranger-eval`.
pub(crate) mod clearance_mask;
// Task 3 of view governance: the two StarRocks bodies the differential harness
// asks the real jars about, built by the compiler's own functions.
#[cfg(feature = "harness")]
#[doc(hidden)]
pub mod harness_support;
// SP4d-3 Task 5: classify every policy of a profile (owner, verdict, blocker
// sentences) for `GET /perm/policies/services/{id}/classify`.
pub mod classify;
/// F11: the curated engine capability catalogue.
pub mod engine_capabilities;
/// F11: compile an engine grant into its engine's profile.
pub mod engine_grants;
// DG #3: each profile's last compile (`perm.compile_runs`, migration 0196).
pub mod compile_runs;
mod governance_events;
// SP4d-3 Task 6: the equivalence gate — probes derived from a profile's
// before/after downloads, compared per plugin version over every affected
// profile. Pure; Task 7's migration transaction is its caller.
pub mod equivalence;
// SP4d-3 Task 3: the closed grammar from a hand-written Ranger access policy to
// the `Grant`s it is equivalent to, or every named reason it is not. Pure; the
// inverse of `grants::compile_grant` — see that module's own doc comment.
pub mod grammar;
// SP4d-3 Task 7: the migration transaction (re-classify, write, recompile,
// switch off, provenance, re-gate every affected profile) and its undo.
pub mod grants;
// J-F3 Task 1b: a group expands to its members at compile time, so every
// `user_groups` writer recompiles through this port implementation — see that
// module's own doc comment for why it is a port and `role_membership` is not.
pub mod group_membership;
// View governance B1 (Task 7): a view's table-level lineage from Trino's
// `EXPLAIN (TYPE IO, FORMAT JSON)`, and the tags it carries to the view.
pub mod lineage;
#[cfg(test)]
mod lineage_compile_tests;
pub(crate) mod members;
pub mod migrate;
pub(crate) mod people;
// People & groups Task 3: `standsFor` agrees with the compiler's expansion
// (spec §4.1). In-crate because the expansion is crate-private.
#[cfg(test)]
mod people_agreement_tests;
// SP4d-4 Task 3: overlapping mask/row-filter rules across the tag/resource
// boundary, named by rule, answered 409 — see that module's own doc comment.
pub mod overlap;
// J-F3: every principal ref a grant or rule names must name someone egret
// knows, or a deny/exclude on it fails open — see that module's doc comment.
pub mod principal_refs;
pub mod role_membership;
pub mod routes;
pub mod rules;
pub mod trigger;
pub mod view_audit;
pub mod view_exposure;
// View governance B3 (Task 9): a view's lineage-derived column tags follow
// its base tables' classifications.
pub mod view_tags;

pub use attribution::{attribute_policy_name, PolicyAttribution};
pub use grants::{compile_grant, validate_grant_shape};
pub use group_membership::PermGroupRecompiler;
pub use members::lock_compile_mints_in_tx;
pub use role_membership::{
    assign_platform_role_and_recompile, delete_platform_role_and_recompile,
    unassign_platform_role_and_recompile,
};
pub use routes::{openapi, perm_router, PermHttpError, PermState};
pub use rules::{compile_rule, validate_rule_shape, ResolvedNames, TaggedColumn};
pub use trigger::{
    affected_services_for_classification, affected_services_for_grant,
    affected_services_for_group_membership, affected_services_for_role_membership,
    affected_services_for_rule, affected_services_for_tag_rename,
    affected_services_for_workspace_members, group_references_via, lock_services_in_tx,
    merge_service_ids, recompile_service, recompile_service_in_tx, recompile_services_in_tx,
    CompileSummary, PermRecompiler, SkippedItem, SourceKind, DEFAULT_EXPANSION_CEILING,
    PROFILE_EXPANSION_CEILING,
};
pub use view_tags::{dependent_view_warehouses, retag_view_in_tx, RetagSummary, ViewRetag};
