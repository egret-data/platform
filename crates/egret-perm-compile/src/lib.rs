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
//! **The pipeline (governance sub-project 1).** A profile's compile is these
//! stages, driven by `trigger::compile_desired`:
//!
//! 1. **Load** (`ir_load`): every store read the lowerings need, on the
//!    caller's connection, as stored (`load_ep`, then the per-family loaders).
//!    `load_ep` does not read the bound engines; `ir_load::bound_engines` runs
//!    inside `fold_clearance_masks`, at the point the old code read them.
//! 2. **Lower** (crate `egret-perm-ir`, `lower::{grants, rules, withhold,
//!    engine_privileges}`): all decisions (scope, skip, refusal, StarRocks tag
//!    expansion) taken on plain data, with the capability matrix saying what
//!    each enforcement point can enforce. Pure: no database, no I/O.
//! 3. **Render** (`ranger::{grants, rules, withhold, engine_privileges}`, via
//!    `ranger::emit`): the neutral statements become typed
//!    `egret_policy::admin` inputs and Ranger JSON. Shape only; nothing is
//!    decided here. `ranger/rules.rs`'s `stamp_service_type` is the one place
//!    an already-rendered `Value` is corrected (a `serviceType` the renderer
//!    hardcodes to `"trino"`).
//!    3b. **Fold** (`trigger::fold_clearance_masks`): the StarRocks clearance
//!    masks are folded into the rendered mask policies, which needs the
//!    rendered Ranger masks and the bound engines.
//! 4. **Complete** (`ranger::completion`): the denies a profile needs because
//!    of what its whole served unit does (hidden columns on StarRocks,
//!    Iceberg metadata tables on Trino), run on the complete desired set.
//! 5. **Refuse overlaps** (`trigger::refuse_overlaps`): the compile is
//!    refused when its finished set would create ambiguous-priority overlaps.
//!
//! The D4 boundary: store-to-IR holds decisions, IR-to-Ranger renders shape
//! only. `egret-perm-ir` has no dependency on `egret-policy`,
//! `egret-policy-api` or `egret-ranger-eval` and no Ranger word in its public
//! type names (`egret-perm-ir/tests/boundary.rs`).
//!
//! **`trigger::recompile_service` (SP4a Task 7)** loads a profile's model,
//! resolves the registry/tag reads the lowerings stay free of, and diffs the
//! result against `perm.compiled` so a recompile writes only what changed.
//! `egret-bi::catalog_tags` calls it on every catalog-tag write, alongside
//! the `tag_version` bump already there. See `trigger`'s own module doc for
//! why a `WorkspaceMembers`-only refusal is skipped rather than aborting the
//! whole recompile. `egret_perm::actions` holds the `Action` to Ranger
//! access-type map.

// SP4d-2a Task 3: mapping a compiled Ranger policy NAME back to the `perm`
// model object that minted it — pure, no DB, no evaluator. See that module's
// own doc comment for the four live no-model-object shapes it must not choke
// on.
pub mod attribution;
pub mod db_error;
// SP4d-2a Task 4: hand-written wire DTOs mirroring `egret-ranger-eval`'s
// `Decision`/`DenyReason`/`UndecidableCause` and mask/row-filter results —
// see that module's own doc comment for why they are hand-written and not
// derived on the source types.
pub mod explain_wire;
// Verify a mask per user: explain's one sentence per person and engine.
pub mod explain_effect;
// View governance A2: the StarRocks priority-1 clearance MASK per classified
// column, its cleared items folded through `egret-ranger-eval`.
pub(crate) mod clearance_mask;
// Governance sub-project 1, Task 4: every read the lowering of one profile
// needs, in one loader.
pub(crate) mod ir_load;
// Governance SP2a, Task 4: a profile's IR-owned infrastructure policies,
// converged before every compile and on their own.
pub mod infra;
// Task 3 of view governance: the two StarRocks bodies the differential harness
// asks the real jars about, built by the compiler's own functions.
#[cfg(feature = "harness")]
#[doc(hidden)]
pub mod harness_support;
// Governance sub-project 1: every recompile compiles twice and asserts the two
// agree (a determinism check, and during a refactor the old path against the
// new). Off in every production build.
#[cfg(feature = "shadow-compile")]
mod shadow;
// SP4d-3 Task 5: classify every policy of a profile (owner, verdict, blocker
// sentences) for `GET /perm/policies/services/{id}/classify`.
pub mod classify;
// What a rule preview will do: reach, enforcing engines, exemptions and
// overlapping masks (classification UX spec C6, C7).
pub mod effect;
/// F11: the curated engine capability catalogue.
pub mod engine_capabilities;
/// F11: compile an engine grant into its engine's profile.
pub mod engine_grants;
pub mod expiry;
/// What a table or column rename carries of grants and path rules.
pub mod follow;
// DG #3: each profile's last compile (`perm.compile_runs`, migration 0198).
pub mod compile_runs;
mod governance_events;
// SP4d-3 Task 6: the equivalence gate — probes derived from a profile's
// before/after downloads, compared per plugin version over every affected
// profile. Pure; Task 7's migration transaction is its caller.
pub mod equivalence;
// SP4d-3 Task 3: the closed grammar from a hand-written Ranger access policy to
// the `Grant`s it is equivalent to, or every named reason it is not. Pure; the
// inverse of `compile_grant` — see `ranger::grants`' own doc comment.
pub mod grammar;
// J-F3 Task 1b: a group expands to its members at compile time, so every
// `user_groups` writer recompiles through this port implementation — see that
// module's own doc comment for why it is a port and `role_membership` is not.
pub mod group_membership;
// DG #4b (CL7): the one module that matches hand-written Ranger policies to
// Data objects, principals and classifications.
pub mod handwritten;
// View governance B1 (Task 7): a view's table-level lineage from Trino's
// `EXPLAIN (TYPE IO, FORMAT JSON)`, and the tags it carries to the view.
pub mod lineage;
#[cfg(test)]
mod lineage_compile_tests;
pub(crate) mod members;
// SP4d-3 Task 7: the migration transaction (re-classify, write, recompile,
// switch off, provenance, re-gate every affected profile) and its undo.
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
// Governance sub-project 1 Task 5: the Ranger target. Renders the neutral
// statements `egret_perm_ir::lower` produces; decides nothing.
pub(crate) mod ranger;
pub mod role_membership;
pub mod routes;
pub mod rules;
// D2 (StarRocks whole-table classification): a table's current columns, read
// from the embedded catalog.
pub mod table_columns;
pub mod trigger;
pub mod username_reuse;
pub mod view_audit;
pub mod view_exposure;
// View governance B3 (Task 9): a view's lineage-derived column tags follow
// its base tables' classifications.
pub mod view_tags;
// #789: the pause point a test sets between a PATCH's first read of the stored
// row and its lock wait; inert unless a test scopes one.
#[doc(hidden)]
pub mod write_seam;

pub use attribution::{attribute_policy_name, PolicyAttribution};
pub use egret_perm_ir::lower::grants::validate_grant_shape;
pub use egret_perm_ir::lower::rules::validate_rule_shape;
pub use group_membership::PermGroupRecompiler;
pub use infra::{converge_infra_policy, InfraSync};
pub use members::lock_compile_mints_in_tx;
pub use principal_refs::{mint_missing_usernames, UsernameRepair};
pub use ranger::grants::compile_grant;
pub use role_membership::{
    assign_platform_role_and_recompile, converge_infra_policies,
    delete_platform_role_and_recompile, set_user_status_and_recompile,
    set_user_status_and_recompile_in_tx, unassign_platform_role_and_recompile,
    unassign_platform_role_and_recompile_in_tx,
};
pub use routes::{openapi, perm_router, PermHttpError, PermState};
pub use rules::{compile_rule, ResolvedNames, TableTarget, TaggedColumn};
pub use trigger::{
    affected_services_for_classification, affected_services_for_grant,
    affected_services_for_group_membership, affected_services_for_role_membership,
    affected_services_for_rule, affected_services_for_tag_rename,
    affected_services_for_workspace_members, affected_starrocks_profiles_for_tables,
    group_references_via, lock_services_in_tx, merge_service_ids, recompile_path_mask_profiles,
    recompile_service, recompile_service_in_tx, recompile_services_in_tx,
    recompile_table_classified_profiles, CompileSummary, PermRecompiler, SkippedItem, SourceKind,
    DEFAULT_EXPANSION_CEILING, PROFILE_EXPANSION_CEILING,
};
pub use view_tags::{dependent_view_warehouses, retag_view_in_tx, RetagSummary, ViewRetag};
