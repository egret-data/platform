//! The `Action` → Ranger access-type map (SP4a Task 4).
//!
//! Written against what the two service-defs actually declare, not against
//! memory or the plan's table — an access type Ranger's plugin does not know
//! is silently ignored: the policy compiles, deploys, and grants nothing.
//! Read with:
//!
//! ```text
//! python3 -c "
//! import json
//! for n in ('trino','starrocks'):
//!     d=json.load(open(f'crates/egret-policy/src/service-defs/{n}.json'))
//!     print(n, [a['name'] for a in d['accessTypes']])
//! "
//! ```
//!
//! `trino.json`'s `accessTypes` (2026-09-24): select, insert, create, drop,
//! delete, use, alter, grant, revoke, show, impersonate, execute,
//! read_sysinfo, write_sysinfo, all — **no `update`**.
//!
//! `starrocks.json`'s `accessTypes` (2026-09-24): grant, node, operate,
//! delete, drop, insert, select, alter, export, `update`, usage, plugin,
//! file, blacklist, repository, refresh, impersonate, create database,
//! create table, create view, create function, create global function,
//! create materialized view, create resource, create resource group, create
//! external catalog, create storage volume.
//!
//! `every_access_type_the_map_emits_is_declared_by_that_service_def`
//! (`tests/compile_grants.rs`) re-reads both files and checks every access
//! type this map emits against them, so the map can't drift from the
//! service-defs silently.

use crate::model::{Action, PrincipalKind};

/// The two engines SP4a's compiler targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineType {
    Trino,
    StarRocks,
}

/// A grant the compiler cannot express against the target engine, or cannot
/// express *correctly* — every variant here exists because a silent
/// alternative would compile a policy that grants something other than what
/// the model states, with nothing anywhere to show it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    /// `UnsupportedAction` is unreachable today — SP4a ships exactly `read`
    /// and `write`, and both map on both engines — but it stays in the
    /// error type because a later phase (SP4b/SP4c) may add an action, and
    /// a compiler that silently dropped one would grant less than the
    /// model says.
    #[error("action {action:?} has no access type declared on {engine:?}'s service-def")]
    UnsupportedAction { action: Action, engine: EngineType },
    #[error(
        "compiling rule {rule:?} would expand to {count} entries, over the ceiling of {ceiling}"
    )]
    ExpansionCeiling {
        rule: String,
        count: usize,
        ceiling: usize,
    },
    /// A `User` principal's `r#ref` is not the bare username the store is
    /// supposed to hold (`model::Principal`'s doc comment: "a username,
    /// never an email … written through unchanged"). The compiler does no
    /// I/O and so cannot resolve `users.username` itself (round 1's fix
    /// guessed a resolution by splitting on `@`, which can collide two
    /// distinct humans onto one engine principal — see
    /// `compile::grants`'s module doc). Refusing is the only safe move
    /// left to a function that cannot look the real username up.
    // The format string can't spell the field name directly — `r#ref` is a
    // raw identifier, and Rust's format-string captures don't support those
    // — so the placeholder is bound explicitly to the field below.
    #[error(
        "user principal ref {value:?} is not usable as a bare username (empty, or contains \
         '@') — canonicalise at the authoring boundary, where `users.username` is readable, \
         not in the compiler",
        value = r#ref
    )]
    UnusableUserRef { r#ref: String },
    /// A principal kind this compiler cannot yet turn into Ranger users/
    /// groups/roles (`WorkspaceMembers` today — Task 6 expands it into the
    /// member list enumerated at compile time). Dropping it silently on an
    /// `include` fails closed (an empty item matches nobody) but on an
    /// `exclude` it fails OPEN: the subtract evaporates and the compiled
    /// policy grants more than the model states. Refusing both is the only
    /// answer that can't silently do the wrong thing in either position.
    #[error(
        "{kind:?} principals are not expanded by this compiler (Task 6) — compiling one would \
         either grant nobody (as an include) or silently drop an exclude, both wrong"
    )]
    UnexpandedPrincipalKind { kind: PrincipalKind },
}

/// The Ranger access types `a` compiles to on `e`. Every engine declares
/// `select` for `read`; `write` differs by engine because Trino's
/// service-def declares no `update` access type.
pub fn engine_accesses(a: Action, e: EngineType) -> Result<Vec<&'static str>, CompileError> {
    match (a, e) {
        (Action::Read, EngineType::Trino) => Ok(vec!["select"]),
        (Action::Read, EngineType::StarRocks) => Ok(vec!["select"]),
        (Action::Write, EngineType::Trino) => Ok(vec!["insert", "delete"]),
        (Action::Write, EngineType::StarRocks) => Ok(vec!["insert", "update", "delete"]),
    }
}
