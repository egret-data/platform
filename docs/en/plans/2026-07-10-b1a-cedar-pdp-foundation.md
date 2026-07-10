# B1a — Cedar PDP Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up a single Cedar-based Policy Decision Point (PDP) in `egret-auth`, backed by a unified `access_grants` grant store, that reproduces today's authorization decisions exactly — validated by a regression matrix — without yet cutting over any call-sites.

**Architecture:** Cedar (`cedar-policy` 4.11.2, in-process) behind a `Pdp` trait in `egret-core`. RBAC is modelled the canonical Cedar way: one **policy template per role** (body lists the role's permitted actions, regenerated from `role_permissions`), and one **template-linked policy per grant** binding `?principal` (user/group) and `?resource` (the grant's scope). A new `access_grants` table is the read-model for grants (kept in sync from the two legacy tables via triggers during the expand phase). The PEP cutover and deletion of the legacy authorizers/tables happen in the follow-up plan **B1b**.

**Tech Stack:** Rust, `cedar-policy` 4.11.2, `sqlx`/Postgres (`egret-metastore::Db`), `async-trait`.

**Reference:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) §3 (B1).

---

## File Structure

- `crates/egret-core/src/authz.rs` (new) — `Subject`, `Resource`, `Decision`, `Pdp` trait. Engine-agnostic.
- `crates/egret-core/src/rbac.rs` (modify) — add `scope` to `Permission`; fold in workspace-feature actions.
- `crates/egret-auth/src/pdp/mod.rs` (new) — `CedarPdp` (impl `Pdp`), the composition point.
- `crates/egret-auth/src/pdp/schema.cedarschema` (new) — Cedar schema (embedded).
- `crates/egret-auth/src/pdp/templates.rs` (new) — generate one Cedar template per role from `role_permissions`.
- `crates/egret-auth/src/pdp/entities.rs` (new) — build Cedar `Entities` + linked policies for a `(subject, resource)`.
- `crates/egret-auth/src/grants.rs` (new) — `AccessGrantStore` read API over `access_grants`.
- `migrations/0062_access_grants.sql` (new) — create + backfill `access_grants`, sync triggers.
- `crates/egret-auth/tests/pdp_regression.rs` (new) — golden decision matrix (Cedar PDP == legacy logic).

---

## Task 1: Add cedar-policy dependency + prove the API (spike)

**Files:**
- Modify: `Cargo.toml` (workspace deps), `crates/egret-auth/Cargo.toml`
- Test: `crates/egret-auth/tests/cedar_spike.rs` (new, deleted at end of task)

- [ ] **Step 1: Pin the dependency**

In the workspace root `Cargo.toml` under `[workspace.dependencies]` add:

```toml
cedar-policy = "4.11.2"
```

In `crates/egret-auth/Cargo.toml` under `[dependencies]` add:

```toml
cedar-policy = { workspace = true }
```

And under `[dev-dependencies]` ensure `tokio` with `macros`/`rt` is present (it already is for other tests — verify).

- [ ] **Step 2: Write a spike test proving the real 4.11.2 API compiles and evaluates**

Create `crates/egret-auth/tests/cedar_spike.rs`:

```rust
use cedar_policy::{Authorizer, Context, Decision, Entities, Request};

#[test]
fn cedar_allows_a_matching_permit() {
    let policy: cedar_policy::PolicySet =
        r#"permit(principal == User::"alice", action == Action::"view", resource == File::"93");"#
            .parse()
            .expect("policy parses");
    let request = Request::new(
        r#"User::"alice""#.parse().unwrap(),
        r#"Action::"view""#.parse().unwrap(),
        r#"File::"93""#.parse().unwrap(),
        Context::empty(),
        None,
    )
    .expect("request builds");
    let response = Authorizer::new().is_authorized(&request, &policy, &Entities::empty());
    assert_eq!(response.decision(), Decision::Allow);
    assert!(!response.diagnostics().reason().count().eq(&0));
}
```

- [ ] **Step 3: Run the spike**

Run: `cargo test -p egret-auth --test cedar_spike -- --nocapture`
Expected: PASS. If any signature differs from the above, this is where you learn the real 4.11.2 surface — record any deltas in a comment at the top of `crates/egret-auth/src/pdp/mod.rs` when you create it in Task 5. (`diagnostics().reason()` returns an iterator of `&PolicyId`; adjust the assertion to whatever the compiler requires.)

- [ ] **Step 4: Delete the spike, keep the dependency**

```bash
rm crates/egret-auth/tests/cedar_spike.rs
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/egret-auth/Cargo.toml
git commit -m "chore(authz): add cedar-policy 4.11.2 dependency"
```

---

## Task 2: Core authz primitives (`egret-core::authz`)

**Files:**
- Create: `crates/egret-core/src/authz.rs`
- Modify: `crates/egret-core/src/lib.rs` (add `pub mod authz;` + re-exports)
- Test: inline `#[cfg(test)]` in `authz.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/egret-core/src/authz.rs`:

```rust
//! Engine-agnostic authorization primitives. The decision engine (Cedar) lives
//! in `egret-auth`; nothing here depends on it.

use crate::ids::{UserId, WorkspaceId};

/// The authenticated caller, as seen by the PDP.
#[derive(Debug, Clone)]
pub struct Subject {
    pub user_id: UserId,
    /// Group ids the user belongs to (direct memberships).
    pub group_ids: Vec<crate::ids::GroupId>,
}

/// What the action is being taken on. Object variants arrive in B2.
#[derive(Debug, Clone)]
pub enum Resource {
    Platform,
    Workspace(WorkspaceId),
}

/// The outcome of an authorization check.
#[derive(Debug, Clone)]
pub struct Decision {
    pub allow: bool,
    /// Ids of the policies that produced the decision (audit reason, B4).
    pub reasons: Vec<String>,
}

/// The single decision point. One implementation (Cedar) lives in `egret-auth`.
#[async_trait::async_trait]
pub trait Pdp: Send + Sync {
    async fn authorize(
        &self,
        subject: &Subject,
        action: &str,
        resource: &Resource,
    ) -> crate::Result<Decision>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_is_constructible() {
        let _ = Resource::Platform;
        let _ = Resource::Workspace(WorkspaceId::new());
    }
}
```

If `GroupId` does not exist in `crates/egret-core/src/ids.rs`, add it following the exact pattern of the existing `UserId`/`WorkspaceId` newtypes in that file (a `Uuid` newtype with `new()`), and re-export it. Ensure `async-trait` is a dependency of `egret-core` (add `async-trait = { workspace = true }` to `crates/egret-core/Cargo.toml` if missing).

- [ ] **Step 2: Wire the module**

In `crates/egret-core/src/lib.rs` add `pub mod authz;` and `pub use authz::{Decision, Pdp, Resource, Subject};` next to the existing `rbac`/`context` re-exports.

- [ ] **Step 3: Run the test**

Run: `cargo test -p egret-core authz`
Expected: PASS (compiles + `resource_is_constructible` passes).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-core/src/authz.rs crates/egret-core/src/lib.rs crates/egret-core/src/ids.rs crates/egret-core/Cargo.toml
git commit -m "feat(authz): Subject/Resource/Decision/Pdp primitives in egret-core"
```

---

## Task 3: Unify the permission catalog (add scope, fold in workspace actions)

**Files:**
- Modify: `crates/egret-core/src/rbac.rs`
- Test: inline `#[cfg(test)]` in `rbac.rs`

- [ ] **Step 1: Add a `scope` field to `Permission` and the macro**

Modify the struct and macro in `crates/egret-core/src/rbac.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Scope {
    Platform,
    Workspace,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Permission {
    pub key: &'static str,
    pub group: &'static str,
    pub description: &'static str,
    pub scope: Scope,
}

macro_rules! perm {
    ($key:literal, $group:literal, $desc:literal, $scope:expr) => {
        Permission { key: $key, group: $group, description: $desc, scope: $scope }
    };
}
```

Update every existing `perm!(...)` row to append the scope: control-plane rows (`engine:*`, `warehouse:*`, `storage:*`, `secret:*`, `policy:*`, `user:*`, `group:*`, `role:*`) get `Scope::Platform`; the `workspace:*` rows also get `Scope::Platform` (they gate platform-level workspace admin, per the module doc-comment).

- [ ] **Step 2: Fold in the workspace-feature actions**

Append to `PERMISSIONS` (keys taken verbatim from the `require_workspace(&ctx, "...")` call-sites in `egret-bi/src/routes.rs`, `egret-notebook/src/routes.rs`, `egret-query/src/{execution,registry}/routes.rs`, `egret-kernel/src/routes.rs`, and the `workspace:manage_members` site in `egret-workspace/src/routes.rs`):

```rust
    // Workspace features (scope = workspace)
    perm!("dataset:read", "Datasets", "View datasets", Scope::Workspace),
    perm!("dataset:manage", "Datasets", "Create/edit/delete datasets", Scope::Workspace),
    perm!("chart:read", "Charts", "View charts", Scope::Workspace),
    perm!("chart:manage", "Charts", "Create/edit/delete charts", Scope::Workspace),
    perm!("dashboard:read", "Dashboards", "View dashboards", Scope::Workspace),
    perm!("dashboard:manage", "Dashboards", "Create/edit/delete dashboards", Scope::Workspace),
    perm!("database:read", "Databases", "View workspace database connections", Scope::Workspace),
    perm!("database:manage", "Databases", "Manage workspace database connections", Scope::Workspace),
    perm!("query:run", "SQL Lab", "Run queries", Scope::Workspace),
    perm!("notebook:read", "Notebooks", "View notebooks", Scope::Workspace),
    perm!("notebook:write", "Notebooks", "Edit notebooks and run cells", Scope::Workspace),
    perm!("notebook:manage", "Notebooks", "Manage notebook access", Scope::Workspace),
    perm!("workspace:manage_members", "Workspaces", "Add/remove workspace members", Scope::Workspace),
```

Before writing these, grep each source file for the exact constant strings (e.g. `const DS_READ`, `const CH_MANAGE`, `const READ`, `const WRITE`, `PERM`) and use the **exact** literal each resolves to. Do not invent keys — every key here must match a live `require_workspace` argument. Add a row for each distinct literal you find; remove any speculative row above that has no matching call-site.

- [ ] **Step 3: Add a scope accessor + test**

Add:

```rust
/// Permissions at a given scope, catalog order preserved.
pub fn at_scope(scope: Scope) -> Vec<&'static Permission> {
    PERMISSIONS.iter().filter(|p| p.scope == scope).collect()
}
```

Extend the test module:

```rust
    #[test]
    fn workspace_actions_present_and_scoped() {
        assert!(is_known("dashboard:read"));
        assert!(PERMISSIONS.iter().any(|p| p.key == "dashboard:read" && p.scope == Scope::Workspace));
        assert!(!at_scope(Scope::Workspace).is_empty());
        assert!(at_scope(Scope::Platform).iter().any(|p| p.key == "engine:create"));
    }
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p egret-core rbac`
Expected: PASS.

- [ ] **Step 5: Fix downstream compile breaks from the new field**

The `PermissionDto`/catalog endpoint in `crates/egret-controlplane/src/access/mod.rs` constructs or maps `Permission`. Run `cargo build -p egret-controlplane`; wherever it fails to compile because of the new `scope` field, thread the field through (add `scope` to the DTO as a serialized string, or ignore it in the mapping). Keep the existing JSON shape additive — only add a `scope` field, never remove one.

- [ ] **Step 6: Run the workspace to confirm nothing else broke**

Run: `cargo build --workspace`
Expected: SUCCESS.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-core/src/rbac.rs crates/egret-controlplane/src/access/mod.rs
git commit -m "feat(authz): unify permission catalog — add scope + workspace-feature actions"
```

---

## Task 4: `access_grants` migration (expand phase)

**Files:**
- Create: `migrations/0062_access_grants.sql`
- Test: `crates/egret-auth/tests/access_grants_migration.rs` (new)

- [ ] **Step 1: Write the migration**

Create `migrations/0062_access_grants.sql`:

```sql
-- Unified grant store (Effort B / B1). Expand phase: access_grants is the
-- read-model for the PDP; the two legacy tables remain the write path and are
-- mirrored into access_grants by triggers until B1b flips writers and drops them.

CREATE TABLE access_grants (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    principal_type text NOT NULL CHECK (principal_type IN ('user','group','service')),
    principal_id   uuid NOT NULL,
    role_id        uuid NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    scope_type     text NOT NULL CHECK (scope_type IN ('platform','workspace','object')),
    scope_id       uuid,
    object_type    text,
    granted_by     uuid,
    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (principal_type, principal_id, role_id, scope_type, scope_id)
);
CREATE INDEX access_grants_principal_idx ON access_grants (principal_type, principal_id);
CREATE INDEX access_grants_scope_idx ON access_grants (scope_type, scope_id);

-- Backfill: platform assignments.
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
SELECT principal_type, principal_id, role_id, 'platform', NULL
FROM platform_role_assignments
ON CONFLICT DO NOTHING;

-- Backfill: workspace memberships (map role NAME -> workspace-scoped role id).
INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
SELECT wm.principal_type, wm.principal_id, r.id, 'workspace', wm.workspace_id
FROM workspace_members wm
JOIN roles r ON r.scope = 'workspace' AND r.name = wm.role
ON CONFLICT DO NOTHING;

-- Sync triggers: mirror legacy writes into access_grants during the expand phase.
CREATE FUNCTION sync_platform_grant() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF (TG_OP = 'INSERT') THEN
        INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
        VALUES (NEW.principal_type, NEW.principal_id, NEW.role_id, 'platform', NULL)
        ON CONFLICT DO NOTHING;
    ELSIF (TG_OP = 'DELETE') THEN
        DELETE FROM access_grants
        WHERE principal_type = OLD.principal_type AND principal_id = OLD.principal_id
          AND role_id = OLD.role_id AND scope_type = 'platform';
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_platform_grant_trg
AFTER INSERT OR DELETE ON platform_role_assignments
FOR EACH ROW EXECUTE FUNCTION sync_platform_grant();

CREATE FUNCTION sync_workspace_grant() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE rid uuid;
BEGIN
    IF (TG_OP = 'INSERT') THEN
        SELECT id INTO rid FROM roles WHERE scope = 'workspace' AND name = NEW.role;
        INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id)
        VALUES (NEW.principal_type, NEW.principal_id, rid, 'workspace', NEW.workspace_id)
        ON CONFLICT DO NOTHING;
    ELSIF (TG_OP = 'DELETE') THEN
        SELECT id INTO rid FROM roles WHERE scope = 'workspace' AND name = OLD.role;
        DELETE FROM access_grants
        WHERE principal_type = OLD.principal_type AND principal_id = OLD.principal_id
          AND role_id = rid AND scope_type = 'workspace' AND scope_id = OLD.workspace_id;
    END IF;
    RETURN NULL;
END $$;
CREATE TRIGGER sync_workspace_grant_trg
AFTER INSERT OR DELETE ON workspace_members
FOR EACH ROW EXECUTE FUNCTION sync_workspace_grant();
```

- [ ] **Step 2: Force migrator re-embed (known gotcha)**

The `serve`/test binary embeds migrations at compile time. Touch the metastore lib so a new migration is picked up:

```bash
touch crates/egret-metastore/src/lib.rs
```

- [ ] **Step 3: Write a test that the backfill is correct**

Create `crates/egret-auth/tests/access_grants_migration.rs`. Follow the existing DB-test harness in `crates/egret-workspace/tests/workspace_db.rs` for how a test `Db` is built (same helper/env). The test:

```rust
// Arrange: seed a platform_role_assignments row and a workspace_members row for
// a known user, then assert access_grants contains the equivalent rows.
// (Use the same test-db setup helper as egret-workspace/tests/workspace_db.rs.)
```

Write it concretely against that harness: insert one platform assignment (user + a platform role id) and one workspace membership (user + workspace + role name), then:

```rust
    let plat: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_id = $1 AND scope_type = 'platform'")
        .bind(user_id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(plat, 1);
    let ws: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM access_grants WHERE principal_id = $1 AND scope_type = 'workspace' AND scope_id = $2")
        .bind(user_id).bind(ws_id).fetch_one(&db.pool).await.unwrap();
    assert_eq!(ws, 1);
```

- [ ] **Step 4: Run the test**

Run: `cargo test -p egret-auth --test access_grants_migration`
Expected: PASS (migration applies, backfill + a fresh insert both land in `access_grants`).

- [ ] **Step 5: Commit**

```bash
git add migrations/0062_access_grants.sql crates/egret-metastore/src/lib.rs crates/egret-auth/tests/access_grants_migration.rs
git commit -m "feat(authz): access_grants table with backfill + expand-phase sync triggers"
```

---

## Task 5: `AccessGrantStore` read API

**Files:**
- Create: `crates/egret-auth/src/grants.rs`
- Modify: `crates/egret-auth/src/lib.rs` (add `pub mod grants;`)
- Test: `crates/egret-auth/tests/grant_store.rs` (new)

- [ ] **Step 1: Define the row + store**

Create `crates/egret-auth/src/grants.rs`:

```rust
//! Read API over the unified `access_grants` table.

use egret_core::ids::{UserId, WorkspaceId};
use egret_core::{Error, Result};
use egret_metastore::Db;

/// A single grant relevant to a decision.
#[derive(Debug, Clone)]
pub struct Grant {
    pub role_id: uuid::Uuid,
    pub scope_type: String,       // 'platform' | 'workspace'
    pub scope_id: Option<uuid::Uuid>,
}

#[derive(Clone)]
pub struct AccessGrantStore {
    db: Db,
}

impl AccessGrantStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Every grant held by the user directly or via a group, at platform scope
    /// or for the given workspace. (Object scope arrives in B2.)
    pub async fn grants_for(
        &self,
        user_id: UserId,
        ws: Option<WorkspaceId>,
    ) -> Result<Vec<Grant>> {
        let rows: Vec<(uuid::Uuid, String, Option<uuid::Uuid>)> = sqlx::query_as(
            "SELECT ag.role_id, ag.scope_type, ag.scope_id \
             FROM access_grants ag \
             WHERE ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                  OR (ag.principal_type = 'group' AND ag.principal_id IN \
                        (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
               AND ( ag.scope_type = 'platform' \
                  OR (ag.scope_type = 'workspace' AND ag.scope_id = $2) )",
        )
        .bind(user_id.0)
        .bind(ws.map(|w| w.0))
        .fetch_all(&self.db.pool)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!(e)))?;
        Ok(rows
            .into_iter()
            .map(|(role_id, scope_type, scope_id)| Grant { role_id, scope_type, scope_id })
            .collect())
    }
}
```

Confirm the `Db` import path and `.pool` field against an existing store (e.g. `egret-workspace/src/rbac.rs`), and match its `db_err` style.

- [ ] **Step 2: Write the test**

Create `crates/egret-auth/tests/grant_store.rs` using the same DB harness. Seed a user with a platform role and a workspace role; assert:

```rust
    let store = AccessGrantStore::new(db.clone());
    let g = store.grants_for(user_id, Some(ws_id)).await.unwrap();
    assert!(g.iter().any(|x| x.scope_type == "platform"));
    assert!(g.iter().any(|x| x.scope_type == "workspace" && x.scope_id == Some(ws_id.0)));
    // A workspace the user is not in returns no workspace grants.
    let g2 = store.grants_for(user_id, Some(other_ws)).await.unwrap();
    assert!(g2.iter().all(|x| x.scope_type != "workspace"));
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p egret-auth --test grant_store`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-auth/src/grants.rs crates/egret-auth/src/lib.rs crates/egret-auth/tests/grant_store.rs
git commit -m "feat(authz): AccessGrantStore read API over access_grants"
```

---

## Task 6: Cedar schema + per-role template generator

**Files:**
- Create: `crates/egret-auth/src/pdp/schema.cedarschema`
- Create: `crates/egret-auth/src/pdp/templates.rs`
- Modify: `crates/egret-auth/src/lib.rs` (add `pub mod pdp;`) and create `crates/egret-auth/src/pdp/mod.rs` with `pub mod templates;`
- Test: inline `#[cfg(test)]` in `templates.rs`

- [ ] **Step 1: Write the Cedar schema**

Create `crates/egret-auth/src/pdp/schema.cedarschema`:

```cedarschema
namespace Egret {
  entity User in [Group, Role];
  entity Group;
  entity Role;
  entity Platform;
  entity Workspace in [Platform];
  // Actions are validated dynamically; the full set is generated from the
  // permission catalog at build time (Task 8 loads action names from egret-core::rbac).
  action "placeholder" appliesTo { principal: [User], resource: [Platform, Workspace] };
}
```

Note: Cedar requires actions declared in the schema for *validation*, but authorization does **not** require a schema (we pass `None` to `Request::new`). For B1a we skip schema-based validation to avoid enumerating every action here; the schema file documents the entity model and is a hook for a future validation task. If the spike (Task 1) showed schema is needed, generate the action list from `egret_core::rbac::PERMISSIONS` instead of the placeholder.

- [ ] **Step 2: Write the template generator (failing test first)**

Create `crates/egret-auth/src/pdp/templates.rs`:

```rust
//! One Cedar policy template per role, body = the role's permitted actions.
//! Regenerated whenever `role_permissions` changes.

/// Build a Cedar template string for a role.
/// `platform_admin` gets an unrestricted body; every other role lists its actions.
pub fn template_for_role(role_name: &str, perms: &[String]) -> String {
    if role_name == "platform_admin" {
        return format!(
            "@id(\"role:{role_name}\")\npermit(principal in ?principal, action, resource in ?resource);"
        );
    }
    let actions = perms
        .iter()
        .map(|p| format!("Egret::Action::\"{p}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "@id(\"role:{role_name}\")\npermit(principal in ?principal, action in [{actions}], resource in ?resource);"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cedar_policy::PolicySet;

    #[test]
    fn admin_template_parses_as_a_template() {
        let t = template_for_role("platform_admin", &[]);
        let mut ps = PolicySet::new();
        let tmpl: cedar_policy::Template = t.parse().expect("admin template parses");
        ps.add_template(tmpl).expect("template added");
    }

    #[test]
    fn role_template_lists_actions_and_parses() {
        let t = template_for_role("ws_editor", &["dashboard:read".into(), "dashboard:manage".into()]);
        assert!(t.contains("Egret::Action::\"dashboard:read\""));
        let tmpl: cedar_policy::Template = t.parse().expect("role template parses");
        let mut ps = PolicySet::new();
        ps.add_template(tmpl).unwrap();
    }
}
```

If `PolicySet::new`, `Template::from_str`, or `add_template` differ in 4.11.2 (learned in Task 1), adjust to the real names; the intent is "the generated string is a valid Cedar **template** with `?principal` and `?resource` slots."

- [ ] **Step 3: Run the test**

Run: `cargo test -p egret-auth pdp::templates`
Expected: PASS (both templates parse).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-auth/src/pdp/schema.cedarschema crates/egret-auth/src/pdp/mod.rs crates/egret-auth/src/pdp/templates.rs crates/egret-auth/src/lib.rs
git commit -m "feat(authz): Cedar schema + per-role template generator"
```

---

## Task 7: Cedar entities + linked policies for a request

**Files:**
- Create: `crates/egret-auth/src/pdp/entities.rs`
- Modify: `crates/egret-auth/src/pdp/mod.rs` (`pub mod entities;`)
- Test: inline `#[cfg(test)]` in `entities.rs`

- [ ] **Step 1: Define the request-shaping helpers (failing test first)**

Create `crates/egret-auth/src/pdp/entities.rs`. It converts a `Subject`, its `Grant`s, and a `Resource` into: (a) the Cedar `Entities` (group memberships + resource hierarchy), (b) the request principal/action/resource `EntityUid`s, and (c) the per-grant template-linked policies.

```rust
use cedar_policy::{Entities, Entity, EntityUid, PolicyId, PolicySet, SlotId};
use egret_core::authz::{Resource, Subject};
use std::collections::HashMap;

use crate::grants::Grant;

/// Map a role_id -> role name (resolved once per request from the DB by the caller).
pub type RoleNames = HashMap<uuid::Uuid, String>;

fn user_uid(s: &Subject) -> EntityUid {
    format!("Egret::User::\"{}\"", s.user_id.0).parse().expect("user uid")
}
fn resource_uid(r: &Resource) -> EntityUid {
    match r {
        Resource::Platform => "Egret::Platform::\"root\"".parse().unwrap(),
        Resource::Workspace(w) => format!("Egret::Workspace::\"{}\"", w.0).parse().unwrap(),
    }
}
fn scope_uid(scope_type: &str, scope_id: Option<uuid::Uuid>) -> EntityUid {
    match scope_type {
        "platform" => "Egret::Platform::\"root\"".parse().unwrap(),
        "workspace" => format!("Egret::Workspace::\"{}\"", scope_id.expect("ws scope has id")).parse().unwrap(),
        other => panic!("unsupported scope_type in B1a: {other}"),
    }
}

/// Build entities: the user, its group memberships, and the resource hierarchy
/// (Workspace in Platform). Grants' scope entities are included so `resource in
/// ?resource` resolves.
pub fn build_entities(subject: &Subject, resource: &Resource) -> Entities {
    let mut es: Vec<Entity> = Vec::new();
    let groups: std::collections::HashSet<EntityUid> = subject
        .group_ids
        .iter()
        .map(|g| format!("Egret::Group::\"{}\"", g.0).parse().unwrap())
        .collect();
    es.push(Entity::new_no_attrs(user_uid(subject), groups));
    // Platform is the root of the resource hierarchy.
    let platform: EntityUid = "Egret::Platform::\"root\"".parse().unwrap();
    es.push(Entity::new_no_attrs(platform.clone(), Default::default()));
    if let Resource::Workspace(w) = resource {
        let ws: EntityUid = format!("Egret::Workspace::\"{}\"", w.0).parse().unwrap();
        es.push(Entity::new_no_attrs(ws, std::iter::once(platform).collect()));
    }
    Entities::from_entities(es, None).expect("entities build")
}

/// One template-linked policy per grant, binding ?principal (the granted
/// principal) and ?resource (the grant's scope). Returns the augmented policy set.
pub fn link_grants(
    mut policies: PolicySet,
    subject: &Subject,
    grants: &[Grant],
    role_names: &RoleNames,
) -> PolicySet {
    for (i, g) in grants.iter().enumerate() {
        let Some(role) = role_names.get(&g.role_id) else { continue };
        let template_id: PolicyId = format!("role:{role}").parse().unwrap();
        let link_id: PolicyId = format!("grant-{i}").parse().unwrap();
        let mut vals = HashMap::new();
        vals.insert(SlotId::principal(), user_uid(subject));
        vals.insert(SlotId::resource(), scope_uid(&g.scope_type, g.scope_id));
        policies.link(template_id, link_id, vals).expect("link grant");
    }
    policies
}

pub fn request_uids(subject: &Subject, action: &str, resource: &Resource) -> (EntityUid, EntityUid, EntityUid) {
    (
        user_uid(subject),
        format!("Egret::Action::\"{action}\"").parse().expect("action uid"),
        resource_uid(resource),
    )
}
```

Adjust `Entity::new_no_attrs`, `Entities::from_entities`, `SlotId::principal()/resource()`, and `PolicySet::link` to the real 4.11.2 names from Task 1 (these are the documented shapes; the compiler is the source of truth). Note the linked-grant principal is always the subject user (grants held via a group are already resolved to this user by `grants_for`), so `principal in ?principal` with `?principal = User` is correct and reflexive.

- [ ] **Step 2: Write a unit test that a linked policy authorizes**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdp::templates::template_for_role;
    use cedar_policy::{Authorizer, Context, Decision, Request};
    use egret_core::ids::{UserId, WorkspaceId};

    #[test]
    fn workspace_grant_allows_scoped_action() {
        let uid = UserId::new();
        let ws = WorkspaceId::new();
        let subject = Subject { user_id: uid, group_ids: vec![] };
        let role_id = uuid::Uuid::new_v4();

        let mut ps = PolicySet::new();
        ps.add_template(template_for_role("ws_editor", &["dashboard:read".into()]).parse().unwrap()).unwrap();
        let names: RoleNames = std::iter::once((role_id, "ws_editor".to_string())).collect();
        let grants = vec![Grant { role_id, scope_type: "workspace".into(), scope_id: Some(ws.0) }];
        let ps = link_grants(ps, &subject, &grants, &names);

        let resource = Resource::Workspace(ws);
        let entities = build_entities(&subject, &resource);
        let (p, a, r) = request_uids(&subject, "dashboard:read", &resource);
        let req = Request::new(p, a, r, Context::empty(), None).unwrap();
        let resp = Authorizer::new().is_authorized(&req, &ps, &entities);
        assert_eq!(resp.decision(), Decision::Allow);

        // A different workspace is denied.
        let other = Resource::Workspace(WorkspaceId::new());
        let entities2 = build_entities(&subject, &other);
        let (p2, a2, r2) = request_uids(&subject, "dashboard:read", &other);
        let req2 = Request::new(p2, a2, r2, Context::empty(), None).unwrap();
        assert_eq!(Authorizer::new().is_authorized(&req2, &ps, &entities2).decision(), Decision::Deny);
    }
}
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p egret-auth pdp::entities`
Expected: PASS — the grant allows the action in its workspace and denies it in another.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-auth/src/pdp/entities.rs crates/egret-auth/src/pdp/mod.rs
git commit -m "feat(authz): Cedar entities + per-grant template linking"
```

---

## Task 8: `CedarPdp` — assemble the decision point

**Files:**
- Modify: `crates/egret-auth/src/pdp/mod.rs`
- Test: `crates/egret-auth/tests/cedar_pdp.rs` (new)

- [ ] **Step 1: Implement `CedarPdp`**

In `crates/egret-auth/src/pdp/mod.rs`:

```rust
pub mod entities;
pub mod templates;

use std::collections::HashMap;

use cedar_policy::{Authorizer, Context, Decision as CedarDecision, PolicySet, Request};
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_core::{rbac, Result};
use egret_metastore::Db;

use crate::grants::AccessGrantStore;

/// The Cedar-backed decision point. Templates are built from `role_permissions`
/// once at construction (B1a); hot-reload on `role_permissions` changes is B2.
#[derive(Clone)]
pub struct CedarPdp {
    grants: AccessGrantStore,
    db: Db,
    templates: PolicySet,
    role_names: HashMap<uuid::Uuid, String>,
}

impl CedarPdp {
    pub async fn build(db: Db) -> Result<Self> {
        // Load roles + their permissions, build one template each.
        let roles: Vec<(uuid::Uuid, String)> =
            sqlx::query_as("SELECT id, name FROM roles")
                .fetch_all(&db.pool).await
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
        let mut templates = PolicySet::new();
        let mut role_names = HashMap::new();
        for (id, name) in roles {
            let perms: Vec<(String,)> =
                sqlx::query_as("SELECT permission FROM role_permissions WHERE role_id = $1")
                    .bind(id).fetch_all(&db.pool).await
                    .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
            let perms: Vec<String> = perms.into_iter().map(|(p,)| p).collect();
            let t = templates::template_for_role(&name, &perms);
            templates.add_template(t.parse().map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?)
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            role_names.insert(id, name);
        }
        // Assert every catalog action appears in at least one template? No — a
        // permission with no role is legal. (Drift is covered by the catalog lint.)
        let _ = rbac::PERMISSIONS;
        Ok(Self { grants: AccessGrantStore::new(db.clone()), db, templates, role_names })
    }
}

#[async_trait::async_trait]
impl Pdp for CedarPdp {
    async fn authorize(&self, subject: &Subject, action: &str, resource: &Resource) -> Result<Decision> {
        let ws = match resource { Resource::Workspace(w) => Some(*w), Resource::Platform => None };
        let grants = self.grants.grants_for(subject.user_id, ws).await?;
        let policies = entities::link_grants(self.templates.clone(), subject, &grants, &self.role_names);
        let ents = entities::build_entities(subject, resource);
        let (p, a, r) = entities::request_uids(subject, action, resource);
        let req = Request::new(p, a, r, Context::empty(), None)
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
        let resp = Authorizer::new().is_authorized(&req, &policies, &ents);
        Ok(Decision {
            allow: resp.decision() == CedarDecision::Allow,
            reasons: resp.diagnostics().reason().map(|id| id.to_string()).collect(),
        })
    }
}
```

The unused `db` field is retained for the future hot-reload path; if clippy complains, prefix with `_db` or add `#[allow(dead_code)]` with a `// B2: reload templates` comment.

- [ ] **Step 2: Write an end-to-end PDP test**

Create `crates/egret-auth/tests/cedar_pdp.rs` using the DB harness. Seed: a user with the seeded `ws_editor` role in workspace W (via `workspace_members`, which the trigger mirrors into `access_grants`) and no platform role. Assert:

```rust
    let pdp = CedarPdp::build(db.clone()).await.unwrap();
    let subject = Subject { user_id, group_ids: vec![] };
    // Allowed: a ws_editor action in W.
    assert!(pdp.authorize(&subject, "dashboard:read", &Resource::Workspace(ws)).await.unwrap().allow);
    // Denied: a platform action.
    assert!(!pdp.authorize(&subject, "engine:create", &Resource::Platform).await.unwrap().allow);
    // platform_admin bypass: grant platform_admin, everything allowed.
    // (insert platform_role_assignments row for platform_admin, rebuild pdp)
    let admin = CedarPdp::build(db.clone()).await.unwrap();
    // ...assign platform_admin to another user, assert engine:create allowed on Platform.
```

Fill the platform_admin arm concretely: insert a `platform_role_assignments` row binding the user to the `platform_admin` role id, then assert `authorize(&subject, "engine:create", &Resource::Platform)` is allowed and `authorize(&subject, "dashboard:manage", &Resource::Workspace(any))` is allowed (admin bypass covers all scopes because all resources are `in Platform`).

- [ ] **Step 3: Run the test**

Run: `cargo test -p egret-auth --test cedar_pdp`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-auth/src/pdp/mod.rs crates/egret-auth/tests/cedar_pdp.rs
git commit -m "feat(authz): CedarPdp — assemble the unified decision point"
```

---

## Task 9: Regression matrix — Cedar PDP == legacy decisions

**Files:**
- Create: `crates/egret-auth/tests/pdp_regression.rs`
- Test: itself

- [ ] **Step 1: Encode the legacy decision as an oracle**

The legacy semantics are: (platform) allow if the principal holds the permission via a platform role OR is `platform_admin`; (workspace) allow if `is_platform_admin` OR the principal holds the permission via a workspace role for that workspace. Write a pure oracle over seeded data and compare to `CedarPdp`.

Create `crates/egret-auth/tests/pdp_regression.rs`:

```rust
// For each (principal, action, resource) in a seeded matrix, assert
// CedarPdp.authorize().allow == legacy_oracle(...).
//
// Seed via the existing helpers: create roles (reuse the migration-seeded
// ws_admin/ws_editor/ws_viewer + platform roles), assign a mix to two users
// (one platform_admin, one ws_editor in W1 only), and enumerate actions across
// {engine:create, dashboard:read, dashboard:manage, notebook:write} x
// {Platform, Workspace(W1), Workspace(W2)}.
```

Implement `legacy_oracle` by querying the SAME tables the old authorizers used (`platform_role_assignments`/`role_permissions` for platform; `workspace_members`/`role_permissions` for workspace) — i.e. replicate `egret_workspace::rbac::Authorizer::require_platform`/`require_workspace` and `egret_auth::require_permission` logic exactly — then loop the matrix:

```rust
    for (uid, action, resource) in matrix {
        let expected = legacy_oracle(&db, uid, action, &resource).await;
        let got = pdp.authorize(&Subject { user_id: uid, group_ids: groups_of(uid) }, action, &resource)
            .await.unwrap().allow;
        assert_eq!(got, expected, "mismatch: user={uid} action={action} resource={resource:?}");
    }
```

- [ ] **Step 2: Run the regression**

Run: `cargo test -p egret-auth --test pdp_regression`
Expected: PASS — every cell matches. Any mismatch is a behavior regression to fix in the PDP (not the oracle).

- [ ] **Step 3: Full local CI gate**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all green. (Do not run two `cargo test --workspace` at once — DB contention causes false failures.)

- [ ] **Step 4: Commit**

```bash
git add crates/egret-auth/tests/pdp_regression.rs
git commit -m "test(authz): regression matrix — CedarPdp reproduces legacy decisions"
```

---

## Self-Review Notes

- **Spec coverage (design §3):** §3.1 grant table → Task 4; §3.2 PDP/Port → Tasks 2, 8; §3.3 entity provider → Tasks 5, 7; §3.5 unified catalog → Task 3; §3.6 behavior preservation → Task 9. §3.4 PEP (`require()` + `AuthContext`) and dropping the legacy authorizers/tables are **B1b** (the cutover plan) — deliberately not here; B1a leaves both authorizers live and the PDP un-wired but proven.
- **Out of scope confirmed:** object grants, role hierarchy, audit content, data-plane delegation — none appear as tasks.
- **Cedar API caveat:** exact 4.11.2 method names (`Entity::new_no_attrs`, `Entities::from_entities` arity, `SlotId`, `PolicySet::link`, `add_template`) are pinned by the Task 1 spike; the executing agent adjusts to the compiler where the documented shape differs. This is intentional, not a placeholder — the behavior each step must achieve is fully specified and testable.
- **Known gotchas encoded:** migrator re-embed via `touch` (Task 4), no concurrent `cargo test --workspace` (Task 9), `cargo fmt` in CI (Task 9).

---

## Next: B1b (separate plan)

After B1a merges, B1b performs the PEP cutover: introduce `AuthContext` + `require(ctx, action[, resource])`, migrate all ~200 call-sites off `require_permission`/`require_workspace`, flip grant writers to `access_grants`, drop the sync triggers + the two legacy tables + the two legacy authorizers, and extend the `catalog_drift` lint to the workspace-feature sites.
