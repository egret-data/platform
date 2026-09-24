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
//! - **Principals are emitted in one canonical form: a bare username.**
//!   Measured live on kind (D7 — `docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`,
//!   2026-09-24): `current_user` inside Trino reports `admin`, not
//!   `admin@egret.local`, for a query run as `admin@egret.local` through
//!   egret's query API. A policy naming an email matches nobody, and
//!   nothing in Ranger or egret would show it. `CANONICAL_ADMIN_PRINCIPAL`
//!   below is that measured literal.
//!
//! **Groups still compile through**, unchanged, even though the same D7
//! measurement found groups never reach the engine (no group provider on
//! Trino, no usersync in egret) — egret is not the only possible Ranger
//! consumer, and the situation may change. No claim of enforcement is
//! implied or tested here; see D7 §2-3 and `model::PrincipalKind`'s doc
//! comment.
//!
//! `WorkspaceMembers` principals are not expanded here: doing so needs
//! workspace-membership I/O this compiler does not have (`compile_grant`
//! takes no store handle, by design — the caller resolves the catalog id to
//! a name and stays the only I/O boundary). Per `model::PrincipalKind`'s doc
//! comment, "nothing compiles it yet"; Task 6 expands it into the member
//! list enumerated at compile time (D7 §3). A `WorkspaceMembers` principal
//! reaching this function today contributes nothing to the compiled item
//! rather than being mis-emitted as a Ranger group.

use std::collections::BTreeMap;

use egret_policy::admin::ranger_json::{AccessItemInput, AccessPolicyInput, ResourceLevel};

use crate::actions::{engine_accesses, CompileError, EngineType};
use crate::model::{Effect, Grant, Principal, PrincipalKind, Scope};

/// The bare-username form the engine reports for the seeded admin
/// principal, per D7 (`docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`,
/// measured 2026-09-24 on `kind-egret` / `demo-trino`): `SELECT current_user`
/// run as `admin@egret.local` returns `admin`. The compiler canonicalises
/// every `User` principal to this form (stripping an email's domain, the
/// general case this literal instantiates) and emits nothing else.
pub const CANONICAL_ADMIN_PRINCIPAL: &str = "admin";

/// Canonicalise a `User` principal's reference to the bare-username form the
/// engine reports (D7). `Group`/`Role`/`WorkspaceMembers` refs pass through
/// unchanged — D7 measured what a *user* principal looks like to the
/// engine, not a group or role, and groups don't reach the engine at all
/// today (D7 §2).
fn canonical_ref(p: &Principal) -> String {
    match p.kind {
        PrincipalKind::User => p.r#ref.split('@').next().unwrap_or(&p.r#ref).to_string(),
        PrincipalKind::Group | PrincipalKind::Role | PrincipalKind::WorkspaceMembers => {
            p.r#ref.clone()
        }
    }
}

/// Sort principals in `refs` into a Ranger item's `users`/`groups`/`roles`
/// buckets, canonicalising user refs, and set `effect`/`accesses` on it.
fn principal_item(refs: &[Principal], effect: Effect, accesses: &[String]) -> AccessItemInput {
    let mut users = Vec::new();
    let mut groups = Vec::new();
    let mut roles = Vec::new();
    for p in refs {
        match p.kind {
            PrincipalKind::User => users.push(canonical_ref(p)),
            PrincipalKind::Group => groups.push(p.r#ref.clone()),
            PrincipalKind::Role => roles.push(p.r#ref.clone()),
            // See module doc comment: expansion needs I/O this compiler
            // doesn't have, and is Task 6's job.
            PrincipalKind::WorkspaceMembers => {}
        }
    }
    AccessItemInput {
        users,
        groups,
        roles,
        accesses: accesses.to_vec(),
        effect: effect.as_str().to_string(),
    }
}

/// The Ranger resource-level names the target engine's service-def declares
/// for a catalog/schema/table/column path, in that order. Trino calls the
/// second level `schema`; StarRocks calls it `database`
/// (`crates/egret-policy/src/service-defs/{trino,starrocks}.json`) — same
/// position in the hierarchy, different declared name, so the compiler must
/// pick the name per engine rather than hard-coding Trino's.
fn resource_level_names(e: EngineType) -> [&'static str; 4] {
    match e {
        EngineType::Trino => ["catalog", "schema", "table", "column"],
        EngineType::StarRocks => ["catalog", "database", "table", "column"],
    }
}

fn level(value: String) -> ResourceLevel {
    ResourceLevel {
        values: vec![value],
        is_excludes: false,
        is_recursive: false,
    }
}

/// Render a `Scope` into the engine's `resources` map. `catalog_name` is the
/// name the caller already resolved from `scope.catalog_id` (the compiler
/// stays free of registry I/O). A level the scope leaves unnamed is left
/// OUT of the map entirely, never set to a wildcard `"*"` — Ranger treats a
/// missing resource level and a `"*"`-valued one differently (a missing
/// level does not constrain the hierarchy the same way a recorded wildcard
/// does), and the model only ever narrows explicitly (`Scope`'s doc
/// comment: "a column implies a table implies a schema"), so an absent
/// level is the only faithful compilation of "the grant doesn't name this
/// level".
fn scope_to_resources(
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

/// Compile one `Grant` into a Ranger `AccessPolicyInput` for engine `e`.
///
/// `catalog_name` is the caller-resolved name for `g.scope.catalog_id`;
/// `service` is the target Ranger service name (e.g. `"trino-1"`) stamped
/// onto the policy body. Returns `CompileError::UnsupportedAction` if any of
/// `g.actions` has no access type on `e` (unreachable for SP4a's two
/// actions — see `actions::CompileError`).
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

    let item = principal_item(&g.principals.include, g.effect, &accesses);

    let mut allow_exceptions = Vec::new();
    let mut deny_exceptions = Vec::new();
    if !g.principals.exclude.is_empty() {
        let exception = principal_item(&g.principals.exclude, g.effect, &accesses);
        match g.effect {
            Effect::Allow => allow_exceptions.push(exception),
            Effect::Deny => deny_exceptions.push(exception),
        }
    }

    Ok(AccessPolicyInput {
        name: format!("perm-grant-{}", g.id),
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
