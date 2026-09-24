//! `compile_grant`: turns one `perm.grants` row (plus its principal
//! subtract) into a Ranger `AccessPolicyInput`.
//!
//! Two things a reader can't check by eye, pinned by
//! `tests/compile_grants.rs`:
//!
//! - **The principal subtract compiles to `allowExceptions`** (or
//!   `denyExceptions` for a deny grant). That four-way Ranger structure
//!   (items / denyItems / allowExceptions / denyExceptions) is a
//!   COMPILATION TARGET ONLY — it appears nowhere in `perm`'s model types,
//!   egret's API, or any UI-facing name (global constraint). `PrincipalSet`
//!   (`include` minus `exclude`) is the only shape a human or the OpenAPI
//!   schema ever sees; this function is where that subtract is realized as
//!   Ranger's exception block, and only here.
//! - **A `User` principal's `r#ref` is emitted verbatim, never rewritten.**
//!   Round 1 of this file split on `@` to turn an email into a guessed
//!   username. That is not egret's username rule. The real one —
//!   `ensure_username` (`crates/egret-auth/src/users.rs`) plus
//!   `migrations/0040_user_username.sql` — lowercases, replaces
//!   `[^a-z0-9_-]+` with `-`, and resolves collisions with a `-2`, `-3`, …
//!   suffix. A compiler with no store handle cannot reproduce that: it
//!   cannot see the collision suffix, and a naive split-on-`@` can compile
//!   two distinct humans (`bob` and `bob-2`, both slugging from the same
//!   local part) onto the SAME engine principal — an `exclude` of the
//!   second lands on the first, and the one meant to be excluded stays
//!   included. `model::Principal`'s doc comment already says refs are "a
//!   username, never an email … written through unchanged" — canonicalising
//!   here contradicted the store's own contract. So: this compiler performs
//!   NO canonicalisation. It emits `r#ref` byte-for-byte and refuses
//!   (`CompileError::UnusableUserRef`) a `User` ref that's empty or
//!   contains `@`, since either one is a sign the value reaching this
//!   function was never resolved to `users.username` in the first place. A
//!   later, I/O-capable task (username resolution belongs at the authoring
//!   boundary, where `users.username` is readable) owns turning an email
//!   into the real, collision-resolved username.
//!
//! **Groups still compile through**, unchanged, even though D7
//! (`docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`) found
//! groups never reach the engine (no group provider on Trino, no usersync in
//! egret) — egret is not the only possible Ranger consumer, and the
//! situation may change. No claim of enforcement is implied or tested here;
//! see D7 §2-3 and `model::PrincipalKind`'s doc comment.
//!
//! **`WorkspaceMembers` principals are refused, not dropped.** Round 1
//! silently contributed nothing for a `WorkspaceMembers` principal, on the
//! theory that Task 6 (which expands it into the member list enumerated at
//! compile time, needing workspace-membership I/O this compiler does not
//! have) hadn't landed yet. That reasoning holds for `include` — an empty
//! item matches nobody, so the grant fails closed — but not for `exclude`:
//! dropping the only principal in an `exclude` set still produces a
//! non-empty exception list containing one EMPTY item, which Ranger matches
//! against nobody, so the subtract evaporates and the compiled policy grants
//! MORE than the model states. There is no position-dependent way to get
//! this right without the I/O this compiler doesn't have, so both positions
//! refuse via `CompileError::UnexpandedPrincipalKind`.

use std::collections::BTreeMap;

// `egret-policy` keeps `admin::ranger_json` module-private (D2: egret must
// never emit a tag *access* policy, and that module's tag builders sit next
// to the ones used here) and re-exports just these three struct types from
// `admin` itself.
use egret_policy::admin::{AccessItemInput, AccessPolicyInput, ResourceLevel};
use egret_policy::COMPILER_GRANT_PREFIX;

use egret_perm::{
    engine_accesses, CompileError, Effect, EngineType, Grant, Principal, PrincipalKind, Scope,
};

/// The one check every `User` ref gets before it is emitted, shared by
/// `principal_item` here and `crate::rules::exempt_principals` — the two
/// places this compiler ever buckets a `User` principal. A `User` ref is
/// emitted verbatim (see module doc comment) after being checked for the two
/// shapes that prove it was never resolved to a bare `users.username`: empty,
/// or containing `@`.
pub(crate) fn validate_user_ref(r#ref: &str) -> Result<(), CompileError> {
    if r#ref.is_empty() || r#ref.contains('@') {
        Err(CompileError::UnusableUserRef {
            r#ref: r#ref.to_string(),
        })
    } else {
        Ok(())
    }
}

/// Sort principals in `refs` into a Ranger item's `users`/`groups`/`roles`
/// buckets and set `effect`/`accesses` on it. Any `WorkspaceMembers`
/// principal refuses the whole compile rather than being silently dropped
/// (module doc comment).
fn principal_item(
    refs: &[Principal],
    effect: Effect,
    accesses: &[String],
) -> Result<AccessItemInput, CompileError> {
    let mut users = Vec::new();
    let mut groups = Vec::new();
    let mut roles = Vec::new();
    for p in refs {
        match p.kind {
            PrincipalKind::User => {
                validate_user_ref(&p.r#ref)?;
                users.push(p.r#ref.clone());
            }
            PrincipalKind::Group => groups.push(p.r#ref.clone()),
            PrincipalKind::Role => roles.push(p.r#ref.clone()),
            PrincipalKind::WorkspaceMembers => {
                return Err(CompileError::UnexpandedPrincipalKind { kind: p.kind });
            }
        }
    }
    Ok(AccessItemInput {
        users,
        groups,
        roles,
        accesses: accesses.to_vec(),
        effect: effect.as_str().to_string(),
    })
}

/// The Ranger resource-level names the target engine's service-def declares
/// for a catalog/schema/table/column path, in that order. Trino calls the
/// second level `schema`; StarRocks calls it `database`
/// (`crates/egret-policy/src/service-defs/{trino,starrocks}.json`) — same
/// position in the hierarchy, different declared name, so the compiler must
/// pick the name per engine rather than hard-coding Trino's.
/// `tests/compile_grants.rs`'s
/// `the_scope_becomes_the_ranger_resource_levels_the_service_def_declares`
/// re-reads both files' `resources` arrays and checks these four names
/// against each one's declared name + parent chain, so this can't drift
/// from the service-defs silently the way the write-up in round 1 could
/// have (its Trino-only test never opened a service-def).
// `pub(crate)`: `crate::rules` (Task 5) reuses this and `level`/
// `scope_to_resources` rather than duplicating the per-engine hierarchy
// naming a second time.
pub(crate) fn resource_level_names(e: EngineType) -> [&'static str; 4] {
    match e {
        EngineType::Trino => ["catalog", "schema", "table", "column"],
        EngineType::StarRocks => ["catalog", "database", "table", "column"],
    }
}

pub(crate) fn level(value: String) -> ResourceLevel {
    ResourceLevel {
        values: vec![value],
        is_excludes: false,
        is_recursive: false,
    }
}

/// Render a `Scope` into the engine's `resources` map. `catalog_name` is the
/// name the caller already resolved from `scope.catalog_id` (the compiler
/// stays free of registry I/O, and does not itself check that `catalog_name`
/// actually names `scope.catalog_id` — that binding is the caller's
/// responsibility, same as the rest of the registry lookup it already owns).
/// A level the scope leaves unnamed is left OUT of the map entirely, never
/// set to a wildcard `"*"`.
///
/// This is deliberate, not merely asserted: Apache Ranger's
/// `RangerDefaultPolicyResourceMatcher.getMatchType()` (`agents-common/…/
/// policyresourcematcher/RangerDefaultPolicyResourceMatcher.java`, upstream
/// `apache/ranger` master) walks the resource hierarchy level by level, and
/// once it runs out of policy-side resource entries it treats any further,
/// deeper levels present on the *request* as `MatchType.ANCESTOR` —
/// unconditionally, with no `isRecursive` or `mandatory` gate on that
/// branch. A policy resources map that stops at `table` therefore matches a
/// request that goes on to name a `column`, the same way a Hive/Trino
/// Ranger policy scoped to a table without ever opening the column picker
/// covers every column beneath it. Emitting `"*"` at an unnamed level
/// instead would still match today, but asserts a participation in the
/// hierarchy the model never claims, and — unlike the ANCESTOR match above —
/// its correctness for every case here has not been checked against the
/// live plugin. **Not yet independently verified**: whether Ranger Admin's
/// own policy-create REST validation enforces `mandatory: true` resource
/// levels at write time (a distinct question from the runtime matcher cited
/// above, and one egret's `default_policies.rs` sidesteps by always filling
/// every level) — flagged for a live check against `demo-trino`'s now-active
/// plugin (kind cluster, ranger enabled) rather than guessed here.
pub(crate) fn scope_to_resources(
    scope: &Scope,
    catalog_name: &str,
    e: EngineType,
) -> BTreeMap<String, ResourceLevel> {
    let names = resource_level_names(e);
    let mut map = BTreeMap::new();
    map.insert(names[0].to_string(), level(catalog_name.to_string()));
    if let Some(schema) = &scope.schema {
        map.insert(names[1].to_string(), level(schema.clone()));
    }
    if let Some(table) = &scope.table {
        map.insert(names[2].to_string(), level(table.clone()));
    }
    if let Some(column) = &scope.column {
        map.insert(names[3].to_string(), level(column.clone()));
    }
    map
}

/// The part of compiling a grant that needs **no service and no engine** —
/// run at the authoring boundary, before the grant is stored (SP4a Task 9
/// review, I-1).
///
/// **Why a write-time check exists at all.** The HTTP create path recompiles
/// every profile the grant affects in the same transaction, so a grant that
/// cannot compile is normally refused by that recompile. But when the affected
/// set is EMPTY — a catalog no engine attaches yet —
/// `recompile_services_in_tx(&[])` validates nothing and the grant commits. It
/// then sits there until someone attaches an engine to that catalog, at which
/// point EVERY recompile of that profile fails: `POST /perm/compile` 400s, and
/// `egret-bi::catalog_tags` rolls back every tag write on that warehouse. A
/// grant nobody can see breaks tagging. Compilation has two failure classes,
/// and this is the one that does not need a service to detect.
///
/// **What is in and what is out.** In: the checks that would fail against
/// every service alike — a `User` ref that was never resolved to a bare
/// `users.username` (`UnusableUserRef`), and a principal kind this compiler
/// cannot expand (`UnexpandedPrincipalKind`). Out: everything that depends on
/// which engine or profile is being compiled for (`UnsupportedMaskType` reads
/// a service-def; `MissingTagService` reads a profile's config), because a
/// grant can be perfectly valid for one profile and not another and only the
/// recompile can tell.
///
/// This calls `principal_item`, the very function `compile_grant` uses, and
/// throws the item away. Reusing it rather than restating the rules is the
/// point: a permission model with two copies of "which principals can be
/// compiled" gets a drift between them, and the drift is a grant that passes
/// the gate and fails the compile.
pub fn validate_grant_shape(g: &Grant) -> Result<(), CompileError> {
    principal_item(&g.principals.include, g.effect, &[])?;
    principal_item(&g.principals.exclude, g.effect, &[])?;
    Ok(())
}

/// Compile one `Grant` into a Ranger `AccessPolicyInput` for engine `e`.
///
/// `catalog_name` is the caller-resolved name for `g.scope.catalog_id` —
/// nothing here checks that the two actually correspond; that binding is the
/// caller's, made when it resolved the id in the first place. `service` is
/// the target Ranger service name (e.g. `"trino-1"`) stamped onto the policy
/// body.
///
/// Returns `CompileError::UnsupportedAction` if any of `g.actions` has no
/// access type on `e` (unreachable for SP4a's two actions — see
/// `actions::CompileError`); `CompileError::UnusableUserRef` if a `User`
/// principal's ref is empty or contains `@`; `CompileError::
/// UnexpandedPrincipalKind` if any principal (include or exclude) is a
/// `WorkspaceMembers` reference this compiler cannot yet expand.
pub fn compile_grant(
    g: &Grant,
    catalog_name: &str,
    service: &str,
    e: EngineType,
) -> Result<AccessPolicyInput, CompileError> {
    let mut accesses: Vec<String> = Vec::new();
    for action in &g.actions {
        for acc in engine_accesses(*action, e)? {
            if !accesses.iter().any(|a| a == acc) {
                accesses.push((*acc).to_string());
            }
        }
    }

    let item = principal_item(&g.principals.include, g.effect, &accesses)?;

    let mut allow_exceptions = Vec::new();
    let mut deny_exceptions = Vec::new();
    if !g.principals.exclude.is_empty() {
        let exception = principal_item(&g.principals.exclude, g.effect, &accesses)?;
        match g.effect {
            Effect::Allow => allow_exceptions.push(exception),
            Effect::Deny => deny_exceptions.push(exception),
        }
    }

    Ok(AccessPolicyInput {
        name: format!("{COMPILER_GRANT_PREFIX}{}", g.id),
        service: service.to_string(),
        is_enabled: true,
        resources: scope_to_resources(&g.scope, catalog_name, e),
        items: vec![item],
        allow_exceptions,
        deny_exceptions,
        is_deny_all_else: false,
        description: String::new(),
        labels: Vec::new(),
        is_override: false,
        additional: Vec::new(),
    })
}
