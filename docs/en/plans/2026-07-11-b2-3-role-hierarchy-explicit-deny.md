# B2-3 Role hierarchy + explicit deny — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add role hierarchy (a role inherits a same-scope parent's permissions) and explicit deny (a deny grant that overrides allows) to the egret Cedar PDP, both converging on `CedarPdp::load_inner`.

**Architecture:** Role hierarchy expands each role's permitted-action list to the transitive closure over its `parent_role_id` chain when templates are built — the request path is untouched. Explicit deny adds an `access_grants.effect` column and emits a `forbid` template per role; a deny grant links the forbid template, and Cedar's `forbid`-beats-`permit` gives deny-wins for free. The share modal's "Blocked" is a deny grant keyed to `ws_editor`.

**Tech Stack:** Rust (axum, sqlx, Postgres), `cedar-policy` 4.11.2, React 18 + TS + Vite + orval + react-query.

**Spec:** `docs/en/specs/2026-07-11-b2-3-role-hierarchy-explicit-deny-design.md`.

**Conventions (read before starting):**
- Run cargo from repo root `/Users/1pro-l4r8/Documents/github/data-portal/egret`. DB tests need `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret`.
- After adding a migration, `touch crates/egret-metastore/src/lib.rs` so the embedded migrator re-embeds (known sqlx staleness gotcha) before running `serve`/tests that migrate.
- `cargo`/`grep`/`npm` output is filtered by RTK; prefix `rtk proxy` for raw output.
- Git identity is configured — do NOT override the author. End every commit body with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Never run two `cargo test --workspace` concurrently.

---

## File structure

**Role hierarchy**
- `migrations/0071_role_parent.sql` (create) — `roles.parent_role_id` + same-scope/acyclic trigger.
- `crates/egret-auth/src/roles.rs` (modify) — `RoleRow.parent_role_id`; `set_parent`; `create` accepts parent; `effective_permissions` helper query is NOT here (closure lives in the PDP).
- `crates/egret-auth/src/pdp/mod.rs` (modify) — `load_inner` computes the closure via two bulk queries.
- `crates/egret-controlplane/src/access/mod.rs` (modify) — `RoleDto.parent_role_id` + `inherited_permissions`; create/update accept `parent_role_id`.

**Explicit deny**
- `migrations/0072_access_grant_effect.sql` (create) — `access_grants.effect` + unique redo.
- `crates/egret-auth/src/grants.rs` (modify) — `Grant.effect` + SELECT.
- `crates/egret-auth/src/pdp/templates.rs` (modify) — `deny_template_for_role`.
- `crates/egret-auth/src/pdp/mod.rs` (modify) — emit `role-deny:{id}` forbid template.
- `crates/egret-auth/src/pdp/entities.rs` (modify) — `link_grants` picks template by effect.
- `crates/egret-workspace/src/rbac.rs` (modify) — `has_object_grant_in_workspace` filters `effect='allow'`.
- `crates/egret-bi/src/shares.rs` (modify) — `ObjectGrantBody.effect`, `ObjectGrantDto.effect`, write + re-read + list.

**Frontend**
- `frontend/src/components/sharing/ShareDialog.tsx` (modify) — Blocked option, No-access badge, inherited deny.
- The RBAC-console role editor component (locate under `frontend/src/routes/control-plane/` — search `RoleDto`/roles editor) — inherits-from picker + inherited perms.
- Regenerated orval client.

---

## Task 1: Role hierarchy — migration + `RoleRepo` parent

**Files:**
- Create: `migrations/0071_role_parent.sql`
- Modify: `crates/egret-auth/src/roles.rs`
- Modify: `crates/egret-metastore/src/lib.rs` (touch only)
- Test: `crates/egret-auth/tests/role_parent.rs` (create)

- [ ] **Step 1: Write the migration**

Create `migrations/0071_role_parent.sql`:

```sql
-- B2-3: role hierarchy. A role may inherit a same-scope parent's permissions.
ALTER TABLE roles ADD COLUMN parent_role_id uuid REFERENCES roles(id) ON DELETE SET NULL;

-- Same-scope + acyclic cannot be a CHECK (it must read other rows). A trigger
-- enforces both as defense-in-depth behind the app-layer validation in RoleRepo.
CREATE OR REPLACE FUNCTION roles_parent_guard() RETURNS trigger AS $$
DECLARE
    cur uuid;
    parent_scope text;
    hops int := 0;
BEGIN
    IF NEW.parent_role_id IS NULL THEN
        RETURN NEW;
    END IF;
    -- Same scope.
    SELECT scope INTO parent_scope FROM roles WHERE id = NEW.parent_role_id;
    IF parent_scope IS DISTINCT FROM NEW.scope THEN
        RAISE EXCEPTION 'role parent must share scope (role %, parent scope %)', NEW.scope, parent_scope;
    END IF;
    -- Acyclic: walk up from the parent; if we reach NEW.id, it is a cycle.
    cur := NEW.parent_role_id;
    WHILE cur IS NOT NULL LOOP
        IF cur = NEW.id THEN
            RAISE EXCEPTION 'role parent cycle detected on role %', NEW.id;
        END IF;
        hops := hops + 1;
        IF hops > 64 THEN
            RAISE EXCEPTION 'role parent chain too deep on role %', NEW.id;
        END IF;
        SELECT parent_role_id INTO cur FROM roles WHERE id = cur;
    END LOOP;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER roles_parent_guard_trg
    BEFORE INSERT OR UPDATE OF parent_role_id ON roles
    FOR EACH ROW EXECUTE FUNCTION roles_parent_guard();
```

- [ ] **Step 2: Touch the metastore lib so the migration re-embeds**

Run: `touch crates/egret-metastore/src/lib.rs`

- [ ] **Step 3: Write the failing test**

Create `crates/egret-auth/tests/role_parent.rs`:

```rust
//! Role hierarchy: parent set/validate on RoleRepo. Requires DATABASE_URL.
use egret_auth::roles::RoleRepo;
use egret_events::EventBus;
use egret_metastore::Db;

async fn db() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn set_parent_same_scope_ok_and_publishes() {
    let db = db().await;
    let bus = EventBus::new();
    let repo = RoleRepo::new(db.clone(), bus.clone());
    let mut rx = bus.subscribe::<egret_auth::events::RolesChanged>();
    let child = repo.create("workspace", "b2_3_child", "", None).await.unwrap();
    let parent = repo.create("workspace", "b2_3_parent", "", None).await.unwrap();
    repo.set_parent(child, Some(parent)).await.unwrap();
    // A RolesChanged fires on set_parent (drain: create also fired earlier).
    assert!(rx.try_recv().is_ok(), "set_parent must publish RolesChanged");
    let row = repo.get(child).await.unwrap().unwrap();
    assert_eq!(row.parent_role_id, Some(parent));
    // cleanup
    repo.delete(child).await.unwrap();
    repo.delete(parent).await.unwrap();
}

#[tokio::test]
async fn set_parent_cross_scope_rejected() {
    let db = db().await;
    let repo = RoleRepo::new(db.clone(), EventBus::new());
    let child = repo.create("workspace", "b2_3_xs_child", "", None).await.unwrap();
    let platform_parent = repo.create("platform", "b2_3_xs_parent", "", None).await.unwrap();
    let err = repo.set_parent(child, Some(platform_parent)).await;
    assert!(err.is_err(), "cross-scope parent must be rejected");
    repo.delete(child).await.unwrap();
    repo.delete(platform_parent).await.unwrap();
}

#[tokio::test]
async fn set_parent_cycle_rejected() {
    let db = db().await;
    let repo = RoleRepo::new(db.clone(), EventBus::new());
    let a = repo.create("workspace", "b2_3_cyc_a", "", None).await.unwrap();
    let b = repo.create("workspace", "b2_3_cyc_b", "", None).await.unwrap();
    repo.set_parent(b, Some(a)).await.unwrap(); // b -> a
    let err = repo.set_parent(a, Some(b)).await; // a -> b would cycle
    assert!(err.is_err(), "cycle must be rejected");
    repo.delete(b).await.unwrap();
    repo.delete(a).await.unwrap();
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test role_parent`
Expected: FAIL — `RoleRow` has no `parent_role_id`, `RoleRepo` has no `set_parent`.

- [ ] **Step 5: Add `parent_role_id` to `RoleRow` and the SELECTs**

In `crates/egret-auth/src/roles.rs`, add the field to `RoleRow`:

```rust
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct RoleRow {
    pub id: Uuid,
    pub scope: String,
    pub name: String,
    pub description: String,
    pub parent_role_id: Option<Uuid>,
}
```

Update the three `SELECT ... FROM roles` column lists in `list`, `get`, and `roles_of` to include `parent_role_id` (for `roles_of` alias it as `r.parent_role_id`):
- `list`: `"SELECT id, scope, name, description, parent_role_id FROM roles ORDER BY scope, name"`
- `get`: `"SELECT id, scope, name, description, parent_role_id FROM roles WHERE id = $1"`
- `roles_of`: `"SELECT r.id, r.scope, r.name, r.description, r.parent_role_id FROM platform_role_assignments pra JOIN roles r ON r.id = pra.role_id WHERE pra.principal_type = $1 AND pra.principal_id = $2 ORDER BY r.name"`

- [ ] **Step 6: Add `set_parent` and extend `create`**

In `crates/egret-auth/src/roles.rs`, add to `impl RoleRepo`:

```rust
    /// Set (or clear) a role's parent. Validates same-scope and acyclic in the
    /// app layer (the DB trigger re-checks). Publishes `RolesChanged` so the PDP
    /// recomputes permission closures.
    pub async fn set_parent(&self, id: Uuid, parent: Option<Uuid>) -> Result<()> {
        if let Some(pid) = parent {
            let child = self.get(id).await?.ok_or_else(|| {
                egret_core::Error::NotFound(format!("role {id}"))
            })?;
            let parent_row = self.get(pid).await?.ok_or_else(|| {
                egret_core::Error::InvalidInput(format!("parent role {pid} not found"))
            })?;
            if parent_row.scope != child.scope {
                return Err(egret_core::Error::InvalidInput(
                    "role parent must share scope".into(),
                ));
            }
            // Walk up from parent; a hit on `id` is a cycle.
            let mut cur = Some(pid);
            let mut hops = 0;
            while let Some(c) = cur {
                if c == id {
                    return Err(egret_core::Error::InvalidInput(
                        "role parent cycle".into(),
                    ));
                }
                hops += 1;
                if hops > 64 {
                    return Err(egret_core::Error::InvalidInput(
                        "role parent chain too deep".into(),
                    ));
                }
                cur = self.get(c).await?.and_then(|r| r.parent_role_id);
            }
        }
        sqlx::query("UPDATE roles SET parent_role_id = $1 WHERE id = $2")
            .bind(parent)
            .bind(id)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        self.bus.publish(RolesChanged);
        Ok(())
    }
```

Extend `create` to accept an optional parent (validated by the trigger; the app-level cross-scope/cycle checks apply on the later `set_parent`, but for `create` the trigger guards a bad parent). Change its signature and INSERT:

```rust
    pub async fn create(
        &self,
        scope: &str,
        name: &str,
        description: &str,
        parent_role_id: Option<Uuid>,
    ) -> Result<Uuid> {
        let res = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO roles (scope, name, description, parent_role_id) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(scope)
        .bind(name)
        .bind(description)
        .bind(parent_role_id)
        .fetch_one(&self.db.pool)
        .await;
        match res {
            Ok(id) => {
                self.bus.publish(RolesChanged);
                Ok(id)
            }
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(
                egret_core::Error::Conflict(format!("role already exists: {scope}/{name}")),
            ),
            Err(e) => Err(db_err(e)),
        }
    }
```

- [ ] **Step 7: Fix all `create(` call sites**

Search for `.create(` on a `RoleRepo` and add the trailing `None`:
Run: `rtk proxy grep -rn "roles.create(\|\.create(\"platform\|\.create(\"workspace" crates/ | grep -i role`
Add `, None` as the 4th argument at each `RoleRepo::create` call so the whole workspace compiles now. This includes the control-plane `create_role` handler in `crates/egret-controlplane/src/access/mod.rs` (pass `None` temporarily — Task 3 replaces it with `body.parent_role_id`) and any test/helper call sites in other crates. Confirm with `rtk proxy cargo build --workspace` before committing.

- [ ] **Step 8: Run the test to verify it passes**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test role_parent`
Expected: PASS (3 tests).

- [ ] **Step 9: Commit**

```bash
git add migrations/0071_role_parent.sql crates/egret-auth/src/roles.rs crates/egret-metastore/src/lib.rs crates/egret-auth/tests/role_parent.rs crates/egret-controlplane crates/egret-catalog-engine crates/egret-policy
git commit -m "feat(authz): role parent_role_id + RoleRepo.set_parent (same-scope, acyclic)"
```
(Include any crate whose test/helper `RoleRepo::create` call you touched in Step 7.)

---

## Task 2: Role hierarchy — transitive-closure in `load_inner`

**Files:**
- Modify: `crates/egret-auth/src/pdp/mod.rs`
- Test: `crates/egret-auth/tests/pdp_role_hierarchy.rs` (create)

- [ ] **Step 1: Write the failing test**

Create `crates/egret-auth/tests/pdp_role_hierarchy.rs`:

```rust
//! A child role's Cedar template must include its parent's permissions.
//! Requires DATABASE_URL.
use egret_auth::pdp::CedarPdp;
use egret_core::authz::{Pdp, Resource, Subject};
use egret_core::ids::{UserId, WorkspaceId};
use egret_events::EventBus;
use egret_metastore::Db;
use uuid::Uuid;

async fn db() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn child_role_inherits_parent_permissions() {
    let db = db().await;
    // parent has dashboard:read; child inherits parent, adds nothing of its own.
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();
    let pname = format!("b2_3_pdp_parent_{}", &parent.to_string()[..8]);
    let cname = format!("b2_3_pdp_child_{}", &child.to_string()[..8]);
    sqlx::query("INSERT INTO roles (id, scope, name, description) VALUES ($1,'workspace',$2,'')")
        .bind(parent).bind(&pname).execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO role_permissions (role_id, permission) VALUES ($1,'dashboard:read')")
        .bind(parent).execute(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO roles (id, scope, name, description, parent_role_id) VALUES ($1,'workspace',$2,'',$3)")
        .bind(child).bind(&cname).bind(parent).execute(&db.pool).await.unwrap();

    // Build the PDP AFTER seeding (raw SQL seeding emits no RolesChanged).
    let pdp = CedarPdp::build(db.clone(), &EventBus::new()).await.unwrap();

    // Grant a user the CHILD role at workspace scope, then request the
    // PARENT-only action dashboard:read on that workspace.
    let user = UserId::new();
    let ws = WorkspaceId::new();
    sqlx::query(
        "INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id) \
         VALUES ('user',$1,$2,'workspace',$3)",
    ).bind(user.0).bind(child).bind(ws.0).execute(&db.pool).await.unwrap();

    let subject = Subject { user_id: user, group_ids: vec![] };
    let d = pdp.authorize(&subject, "dashboard:read", &Resource::Workspace(ws)).await.unwrap();
    assert!(d.allow, "child role must inherit parent's dashboard:read");

    // cleanup
    sqlx::query("DELETE FROM access_grants WHERE principal_id=$1").bind(user.0).execute(&db.pool).await.unwrap();
    sqlx::query("DELETE FROM roles WHERE id IN ($1,$2)").bind(child).bind(parent).execute(&db.pool).await.unwrap();
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test pdp_role_hierarchy`
Expected: FAIL — `load_inner` uses only the role's own permissions, so `dashboard:read` (parent-only) is denied.

- [ ] **Step 3: Rewrite `load_inner` to compute the closure**

In `crates/egret-auth/src/pdp/mod.rs`, replace the body of `load_inner` (the roles + per-role-perms loop) with two bulk queries and an in-memory closure. Keep the `PolicySet` build + the admin-graph `forbid` block unchanged. New body:

```rust
    async fn load_inner(db: &Db) -> Result<PdpInner> {
        // Two bulk queries: all roles (with parent) and all role_permissions.
        let roles: Vec<(uuid::Uuid, String, String, Option<uuid::Uuid>)> =
            sqlx::query_as("SELECT id, name, scope, parent_role_id FROM roles")
                .fetch_all(&db.pool)
                .await
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;
        let perm_rows: Vec<(uuid::Uuid, String)> =
            sqlx::query_as("SELECT role_id, permission FROM role_permissions")
                .fetch_all(&db.pool)
                .await
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!(e)))?;

        // Own permissions and parent pointer per role.
        let mut own: HashMap<uuid::Uuid, Vec<String>> = HashMap::new();
        for (rid, perm) in perm_rows {
            own.entry(rid).or_default().push(perm);
        }
        let parent: HashMap<uuid::Uuid, Option<uuid::Uuid>> =
            roles.iter().map(|(id, _, _, p)| (*id, *p)).collect();

        // Effective permissions = own ∪ ancestors' own, walking parent_role_id.
        // The chain is acyclic (Task 1 app + trigger), but cap defensively.
        let effective = |start: uuid::Uuid| -> Vec<String> {
            let mut acc: std::collections::BTreeSet<String> = Default::default();
            let mut cur = Some(start);
            let mut hops = 0;
            while let Some(c) = cur {
                if let Some(ps) = own.get(&c) {
                    acc.extend(ps.iter().cloned());
                }
                hops += 1;
                if hops > 64 {
                    tracing::error!(role = %start, "role closure exceeded 64 hops; graph may be corrupt");
                    break;
                }
                cur = parent.get(&c).copied().flatten();
            }
            acc.into_iter().collect()
        };

        let mut templates = PolicySet::new();
        let mut role_names = HashMap::new();
        for (id, name, scope, _parent) in &roles {
            let perms = effective(*id);
            let tid: PolicyId = format!("role:{id}")
                .parse()
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            let tmpl = Template::parse(
                Some(tid),
                templates::template_for_role(name, scope, &perms),
            )
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            templates
                .add_template(tmpl)
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            role_names.insert(*id, name.clone());
        }
        // ... admin-graph forbid block (UNCHANGED) ...
```

Keep the existing admin-graph `forbid` block and `Ok(PdpInner { templates, role_names })` exactly as they are after this loop.

- [ ] **Step 4: Run the test to verify it passes**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test pdp_role_hierarchy`
Expected: PASS.

- [ ] **Step 5: Run the PDP parity + existing suites to confirm no regression**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth`
Expected: PASS (parity matrix unchanged — a role with no parent has closure == own perms).

- [ ] **Step 6: Commit**

```bash
git add crates/egret-auth/src/pdp/mod.rs crates/egret-auth/tests/pdp_role_hierarchy.rs
git commit -m "feat(authz): expand role permissions to parent-chain closure in load_inner"
```

---

## Task 3: Role hierarchy — control-plane API + DTO

**Files:**
- Modify: `crates/egret-controlplane/src/access/mod.rs`
- Test: `crates/egret-controlplane/tests/routes_api.rs` (extend)

- [ ] **Step 1: Write the failing test**

Add to `crates/egret-controlplane/tests/routes_api.rs` (follow the file's existing harness for authenticated control-plane requests as platform_admin — mirror an existing `create_role` test; search `create_role`/`/access/roles`):

```rust
#[tokio::test]
async fn create_role_with_parent_exposes_inherited_permissions() {
    // Arrange: a parent workspace role with dashboard:read.
    let app = /* existing test harness building the control-plane app as platform_admin */;
    // Create parent, give it dashboard:read.
    let parent = post_json(&app, "/api/control-plane/access/roles",
        json!({"scope":"workspace","name":"b2_3_api_parent","description":""})).await;
    let parent_id = parent["id"].as_str().unwrap();
    put_json(&app, &format!("/api/control-plane/access/roles/{parent_id}/permissions"),
        json!({"permissions":["dashboard:read"]})).await;
    // Create child inheriting parent.
    let child = post_json(&app, "/api/control-plane/access/roles",
        json!({"scope":"workspace","name":"b2_3_api_child","description":"","parentRoleId":parent_id})).await;
    let child_id = child["id"].as_str().unwrap();

    // Act: GET the role detail.
    let dto = get_json(&app, &format!("/api/control-plane/access/roles/{child_id}")).await;

    // Assert: parentRoleId set, inheritedPermissions includes dashboard:read.
    assert_eq!(dto["parentRoleId"].as_str(), Some(parent_id));
    let inherited: Vec<String> = serde_json::from_value(dto["inheritedPermissions"].clone()).unwrap();
    assert!(inherited.contains(&"dashboard:read".to_string()));
}
```

(If the file has no single-role GET, add one — see Step 4. Use the file's actual helper names for `post_json`/`get_json`/`put_json`; if they differ, match the existing style.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-controlplane --test routes_api create_role_with_parent`
Expected: FAIL — `RoleDto` has no `parentRoleId`/`inheritedPermissions`; `CreateRoleInput` has no `parentRoleId`.

- [ ] **Step 3: Extend `RoleDto` and inputs**

In `crates/egret-controlplane/src/access/mod.rs`:

```rust
#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleDto {
    pub id: Uuid,
    pub scope: String,
    pub name: String,
    pub description: String,
    pub permission_count: i64,
    pub assignee_count: i64,
    pub parent_role_id: Option<Uuid>,
    /// Effective-minus-own permissions, for read-only display. Populated on the
    /// single-role GET; empty on the list endpoint.
    pub inherited_permissions: Vec<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct CreateRoleInput {
    scope: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    parent_role_id: Option<Uuid>,
}
```

Add a set-parent input reused by update:

```rust
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
struct UpdateRoleInput {
    description: String,
    #[serde(default)]
    parent_role_id: Option<Uuid>,
}
```

- [ ] **Step 4: Populate the new fields + a single-role GET with inherited perms**

Add a helper to compute inherited perms (effective − own) by reading the chain via `RoleRepo`:

```rust
async fn inherited_perms(st: &CpState, role: &egret_auth::roles::RoleRow) -> Result<Vec<String>, CpError> {
    let own: std::collections::BTreeSet<String> =
        st.roles.permissions_of(role.id).await?.into_iter().collect();
    let mut acc: std::collections::BTreeSet<String> = Default::default();
    let mut cur = role.parent_role_id;
    let mut hops = 0;
    while let Some(c) = cur {
        for p in st.roles.permissions_of(c).await? { acc.insert(p); }
        hops += 1;
        if hops > 64 { break; }
        cur = st.roles.get(c).await?.and_then(|r| r.parent_role_id);
    }
    Ok(acc.difference(&own).cloned().collect())
}
```

Add a single-role GET route `GET /api/control-plane/access/roles/{id}` (if none exists) returning the full `RoleDto` with `inherited_permissions` populated; in `list_roles` set `parent_role_id: r.parent_role_id, inherited_permissions: vec![]`. In `create_role`, pass `body.parent_role_id` to `st.roles.create(...)` (now 4-arg) and, if `Some`, the trigger validates scope/cycle — map a DB error to `CpError::BadRequest`. In `update_role`, after `update_description`, call `st.roles.set_parent(id, body.parent_role_id).await?` (map its `InvalidInput` to `CpError::BadRequest`), then build the DTO with parent + `inherited_perms`. Register the new GET route + updated schemas in the router and the `#[openapi(...)]` list.

- [ ] **Step 5: Run the test to verify it passes**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-controlplane --test routes_api create_role_with_parent`
Expected: PASS.

- [ ] **Step 6: Regenerate OpenAPI**

Run: `rtk proxy cargo run -p egret-bin -- openapi > openapi.json`
Confirm `parentRoleId`/`inheritedPermissions` appear on `RoleDto` and `CreateRoleInput`.

- [ ] **Step 7: Run the control-plane suite + commit**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-controlplane`
Expected: PASS.

```bash
git add crates/egret-controlplane/src/access/mod.rs crates/egret-controlplane/tests/routes_api.rs openapi.json
git commit -m "feat(authz): role parent + inherited permissions in control-plane API"
```

---

## Task 4: Explicit deny — migration + `grants_for.effect`

**Files:**
- Create: `migrations/0072_access_grant_effect.sql`
- Modify: `crates/egret-auth/src/grants.rs`
- Modify: `crates/egret-metastore/src/lib.rs` (touch)
- Test: `crates/egret-auth/tests/grant_effect.rs` (create)

- [ ] **Step 1: Write the migration**

Create `migrations/0072_access_grant_effect.sql`:

```sql
-- B2-3: explicit deny. A grant may deny (forbid) instead of allow. Deny wins.
ALTER TABLE access_grants ADD COLUMN effect text NOT NULL DEFAULT 'allow'
    CHECK (effect IN ('allow','deny'));

-- Re-key uniqueness to include effect so an allow and a deny on the same
-- (principal, role, scope) can coexist. The old constraint name is the default
-- Postgres name for the 0068 UNIQUE(...) — drop by column set via a new one.
ALTER TABLE access_grants DROP CONSTRAINT access_grants_principal_type_principal_id_role_id_scope_ty_key;
ALTER TABLE access_grants ADD CONSTRAINT access_grants_principal_role_scope_effect_key
    UNIQUE (principal_type, principal_id, role_id, scope_type, scope_id, effect);
```

> Note: the dropped constraint's name is Postgres's autogenerated name for the 0068 `UNIQUE(principal_type, principal_id, role_id, scope_type, scope_id)`. If `migrate` errors that the constraint does not exist, find the real name with `\d access_grants` in psql and substitute it; Postgres truncates autogenerated names to 63 chars — the name above is the expected truncation. Confirm before running.

- [ ] **Step 2: Verify the constraint name against the live DB**

Run: `psql "$DATABASE_URL" -c "\d access_grants" | rtk proxy grep -i unique`
Confirm the existing unique constraint's exact name; if it differs from `access_grants_principal_type_principal_id_role_id_scope_ty_key`, update the `DROP CONSTRAINT` line to match. Then `touch crates/egret-metastore/src/lib.rs`.

- [ ] **Step 3: Write the failing test**

Create `crates/egret-auth/tests/grant_effect.rs`:

```rust
//! grants_for surfaces the effect column. Requires DATABASE_URL.
use egret_auth::grants::AccessGrantStore;
use egret_core::ids::{UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

async fn db() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn grants_for_returns_effect() {
    let db = db().await;
    let store = AccessGrantStore::new(db.clone());
    let user = UserId::new();
    let ws = WorkspaceId::new();
    let role: Uuid = sqlx::query_scalar("SELECT id FROM roles WHERE scope='workspace' AND name='ws_editor'")
        .fetch_one(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, scope_id, effect) VALUES ('user',$1,$2,'workspace',$3,'deny')")
        .bind(user.0).bind(role).bind(ws.0).execute(&db.pool).await.unwrap();
    let grants = store.grants_for(user, Some(ws), &[]).await.unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].effect, "deny");
    sqlx::query("DELETE FROM access_grants WHERE principal_id=$1").bind(user.0).execute(&db.pool).await.unwrap();
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test grant_effect`
Expected: FAIL — `Grant` has no `effect` field.

- [ ] **Step 5: Add `effect` to `Grant` + `grants_for`**

In `crates/egret-auth/src/grants.rs`, add the field:

```rust
#[derive(Debug, Clone)]
pub struct Grant {
    pub role_id: uuid::Uuid,
    pub scope_type: String,
    pub scope_id: Option<uuid::Uuid>,
    pub object_type: Option<String>,
    /// 'allow' | 'deny'. Deny links the `role-deny:{id}` forbid template.
    pub effect: String,
}
```

Update the query tuple + SELECT + mapping in `grants_for`:

```rust
        let rows: Vec<(uuid::Uuid, String, Option<uuid::Uuid>, Option<String>, String)> = sqlx::query_as(
            "SELECT ag.role_id, ag.scope_type, ag.scope_id, ag.object_type, ag.effect \
             FROM access_grants ag \
             WHERE ( (ag.principal_type = 'user'  AND ag.principal_id = $1) \
                  OR (ag.principal_type = 'group' AND ag.principal_id IN \
                        (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
               AND ( ag.scope_type = 'platform' \
                  OR (ag.scope_type = 'workspace' AND ag.scope_id = $2) \
                  OR (ag.scope_type = 'object' AND ag.scope_id = ANY($3)) )",
        )
        .bind(user_id.0)
        .bind(ws.map(|w| w.0))
        .bind(object_scope_ids)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|(role_id, scope_type, scope_id, object_type, effect)| Grant {
                role_id,
                scope_type,
                scope_id,
                object_type,
                effect,
            })
            .collect())
```

- [ ] **Step 6: Fix `Grant` construction sites**

Search for `Grant {` literals (tests in `entities.rs`, `mod.rs` if any) and add `effect: "allow".into(),`:
Run: `rtk proxy grep -rn "Grant {" crates/egret-auth/src/`
Add `effect: "allow".into(),` to each literal (the `entities.rs` test helpers construct `Grant` directly).

- [ ] **Step 7: Run the test to verify it passes**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test grant_effect`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add migrations/0072_access_grant_effect.sql crates/egret-auth/src/grants.rs crates/egret-metastore/src/lib.rs crates/egret-auth/tests/grant_effect.rs
git commit -m "feat(authz): access_grants.effect column + grants_for surfaces it"
```

---

## Task 5: Explicit deny — forbid template + effect-aware linking

**Files:**
- Modify: `crates/egret-auth/src/pdp/templates.rs`
- Modify: `crates/egret-auth/src/pdp/mod.rs`
- Modify: `crates/egret-auth/src/pdp/entities.rs`
- Test: `crates/egret-auth/tests/pdp_explicit_deny.rs` (create)

- [ ] **Step 1: Write the failing test**

Create `crates/egret-auth/tests/pdp_explicit_deny.rs`:

```rust
//! Deny-wins: an allow on a folder + a deny on one item inside it → the item is
//! denied, siblings still allowed. Requires DATABASE_URL.
use egret_auth::pdp::CedarPdp;
use egret_core::authz::{ObjectRef, Pdp, Resource, Subject};
use egret_core::ids::{UserId, WorkspaceId};
use egret_events::EventBus;
use egret_metastore::Db;
use uuid::Uuid;

async fn db() -> Db {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    db
}

#[tokio::test]
async fn deny_on_item_overrides_folder_allow() {
    let db = db().await;
    let pdp = CedarPdp::build(db.clone(), &EventBus::new()).await.unwrap();
    let user = UserId::new();
    let ws = WorkspaceId::new();
    let folder = Uuid::new_v4();
    let d1 = Uuid::new_v4(); // denied item
    let d2 = Uuid::new_v4(); // sibling, allowed
    let viewer: Uuid = sqlx::query_scalar("SELECT id FROM roles WHERE scope='workspace' AND name='ws_viewer'")
        .fetch_one(&db.pool).await.unwrap();
    let editor: Uuid = sqlx::query_scalar("SELECT id FROM roles WHERE scope='workspace' AND name='ws_editor'")
        .fetch_one(&db.pool).await.unwrap();
    // Allow ws_viewer on the folder (cascades to items).
    sqlx::query("INSERT INTO access_grants (principal_type,principal_id,role_id,scope_type,object_type,scope_id,effect) VALUES ('user',$1,$2,'object','folder',$3,'allow')")
        .bind(user.0).bind(viewer).bind(folder).execute(&db.pool).await.unwrap();
    // Deny ws_editor (read+manage) on item d1.
    sqlx::query("INSERT INTO access_grants (principal_type,principal_id,role_id,scope_type,object_type,scope_id,effect) VALUES ('user',$1,$2,'object','dashboard',$3,'deny')")
        .bind(user.0).bind(editor).bind(d1).execute(&db.pool).await.unwrap();

    let subject = Subject { user_id: user, group_ids: vec![] };
    let obj = |id: Uuid| Resource::Object(ObjectRef { kind: "dashboard".into(), id, folder_id: Some(folder), workspace_id: ws });
    // d1 denied (deny-wins over the folder allow).
    assert!(!pdp.authorize(&subject, "dashboard:read", &obj(d1)).await.unwrap().allow, "deny on d1 must win");
    // d2 still allowed via the folder grant.
    assert!(pdp.authorize(&subject, "dashboard:read", &obj(d2)).await.unwrap().allow, "sibling d2 stays allowed");

    sqlx::query("DELETE FROM access_grants WHERE principal_id=$1").bind(user.0).execute(&db.pool).await.unwrap();
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test pdp_explicit_deny`
Expected: FAIL — no forbid template links for the deny grant, so d1 stays allowed.

- [ ] **Step 3: Add `deny_template_for_role`**

In `crates/egret-auth/src/pdp/templates.rs`, add:

```rust
/// The `forbid` counterpart of `template_for_role`: same effective action list
/// and same scope operator, but forbids. A deny grant links this so Cedar's
/// forbid-beats-permit gives deny-wins. The platform-scoped `platform_admin`
/// gets an unrestricted deny-all (mirroring its unrestricted permit).
pub fn deny_template_for_role(role_name: &str, scope: &str, perms: &[String]) -> String {
    if scope == "platform" && role_name == "platform_admin" {
        return "forbid(principal in ?principal, action, resource in ?resource);".to_string();
    }
    let actions = perms
        .iter()
        .map(|p| format!("Egret::Action::\"{p}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let op = if scope == "workspace" { "in" } else { "==" };
    format!("forbid(principal in ?principal, action in [{actions}], resource {op} ?resource);")
}
```

Add a unit test in the same `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn deny_template_forbids_and_parses() {
        let t = deny_template_for_role("ws_editor", "workspace", &["dashboard:read".into()]);
        assert!(t.starts_with("forbid("));
        assert!(t.contains("resource in ?resource"));
        let tmpl: Template = t.parse().expect("deny template parses");
        let mut ps = PolicySet::new();
        ps.add_template(tmpl).unwrap();
    }
```

- [ ] **Step 4: Emit the deny template in `load_inner`**

In `crates/egret-auth/src/pdp/mod.rs`, inside the `for (id, name, scope, _parent) in &roles` loop (Task 2), after adding the `role:{id}` permit template, also add the `role-deny:{id}` forbid template using the SAME `perms` closure:

```rust
            let did: PolicyId = format!("role-deny:{id}")
                .parse()
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            let dtmpl = Template::parse(
                Some(did),
                templates::deny_template_for_role(name, scope, &perms),
            )
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
            templates
                .add_template(dtmpl)
                .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("{e:?}")))?;
```

- [ ] **Step 5: Pick the template by effect in `link_grants`**

In `crates/egret-auth/src/pdp/entities.rs`, change the `template_id` selection in `link_grants`:

```rust
        let template_id: PolicyId = if g.effect == "deny" {
            format!("role-deny:{}", g.role_id).parse().unwrap()
        } else {
            format!("role:{}", g.role_id).parse().unwrap()
        };
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test pdp_explicit_deny`
Expected: PASS.

- [ ] **Step 7: Run the whole egret-auth suite (parity + templates + entities)**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth`
Expected: PASS (allow-only parity unchanged; new deny + hierarchy tests green).

- [ ] **Step 8: Commit**

```bash
git add crates/egret-auth/src/pdp/templates.rs crates/egret-auth/src/pdp/mod.rs crates/egret-auth/src/pdp/entities.rs crates/egret-auth/tests/pdp_explicit_deny.rs
git commit -m "feat(authz): forbid template per role + effect-aware grant linking (deny-wins)"
```

---

## Task 6: Explicit deny — admission filter + share API effect

**Files:**
- Modify: `crates/egret-workspace/src/rbac.rs`
- Modify: `crates/egret-bi/src/shares.rs`
- Test: `crates/egret-bi/tests/share_deny.rs` (create)

- [ ] **Step 1: Write the failing test**

Create `crates/egret-bi/tests/share_deny.rs`. Mirror the existing object-scope share test harness in `crates/egret-bi/tests/` (search for a test that PUTs `/objects/{kind}/{id}/grants` with a real `CedarPdp`; reuse its app-builder + seed helpers). Assert:

```rust
// 1. An Editor/manager can PUT a deny grant (effect="deny").
// 2. grant_list returns it with effect="deny".
// 3. A non-member holding ONLY a deny grant on an object in the workspace is
//    NOT admitted by has_object_grant_in_workspace (call it directly, assert false).
```

Concretely for assertion 3, add a direct unit-style check using `egret_workspace::rbac::Authorizer`:

```rust
#[tokio::test]
async fn lone_deny_grant_does_not_admit() {
    let db = /* connect + migrate */;
    let authz = egret_workspace::rbac::Authorizer::new(db.clone());
    let user = egret_core::ids::UserId::new();
    let ws = /* seed a workspace */;
    let dash = /* seed a dashboard in ws */;
    let editor: uuid::Uuid = sqlx::query_scalar("SELECT id FROM roles WHERE scope='workspace' AND name='ws_editor'").fetch_one(&db.pool).await.unwrap();
    sqlx::query("INSERT INTO access_grants (principal_type,principal_id,role_id,scope_type,object_type,scope_id,effect) VALUES ('user',$1,$2,'object','dashboard',$3,'deny')")
        .bind(user.0).bind(editor).bind(dash).execute(&db.pool).await.unwrap();
    assert!(!authz.has_object_grant_in_workspace(user, ws).await.unwrap(),
        "a lone deny grant must not admit an object-guest");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-bi --test share_deny`
Expected: FAIL — admission does not filter effect; `ObjectGrantBody` has no `effect`.

- [ ] **Step 3: Filter admission to allow-only**

In `crates/egret-workspace/src/rbac.rs`, in `has_object_grant_in_workspace`, add `AND ag.effect = 'allow'` to the WHERE clause (after `AND ag.scope_type = 'object'`):

```rust
                 AND ag.scope_type = 'object' \
                 AND ag.effect = 'allow' \
                 AND ( (ag.object_type = 'folder'    AND ag.scope_id IN (SELECT id FROM analytics.folders    WHERE workspace_id = $2)) \
```

- [ ] **Step 4: Add `effect` to the share DTOs + write path**

In `crates/egret-bi/src/shares.rs`:

Add to `ObjectGrantBody`:

```rust
pub struct ObjectGrantBody {
    pub principal_type: String,
    pub principal_id: Uuid,
    /// `ws_viewer` (Viewer) | `ws_editor` (Editor). For a "Blocked" grant, send
    /// `ws_editor` with `effect="deny"`.
    pub role: String,
    /// `allow` (default) | `deny`. A deny blocks the principal on this object.
    #[serde(default = "default_effect")]
    pub effect: String,
}

fn default_effect() -> String { "allow".to_string() }
```

Add to `ObjectGrantDto`:

```rust
pub struct ObjectGrantDto {
    pub id: Uuid,
    pub principal_type: String,
    pub principal_id: Uuid,
    pub role: String,
    pub effect: String,
}
```

Add an effect validator:

```rust
fn valid_effect(e: &str) -> Result<(), BiApiError> {
    match e {
        "allow" | "deny" => Ok(()),
        _ => Err(Error::InvalidInput(format!("effect: {e}")).into()),
    }
}
```

In `grant_put`: call `valid_effect(&b.effect)?;` after `valid_role`. Add `effect` to the INSERT and the re-read SELECT + the returned DTO:

```rust
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect, granted_by) \
         VALUES ($1, $2, $3, 'object', $4, $5, $6, $7) ON CONFLICT DO NOTHING",
    )
    .bind(&b.principal_type)
    .bind(b.principal_id)
    .bind(role_id)
    .bind(&kind)
    .bind(id)
    .bind(&b.effect)
    .bind(ctx.principal.user_id.0)
    .execute(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;

    let grant_id: Uuid = sqlx::query_scalar(
        "SELECT id FROM access_grants \
         WHERE principal_type = $1 AND principal_id = $2 AND role_id = $3 \
           AND scope_type = 'object' AND scope_id = $4 AND effect = $5",
    )
    .bind(&b.principal_type)
    .bind(b.principal_id)
    .bind(role_id)
    .bind(id)
    .bind(&b.effect)
    .fetch_one(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;

    Ok(Json(ObjectGrantDto {
        id: grant_id,
        principal_type: b.principal_type,
        principal_id: b.principal_id,
        role: b.role,
        effect: b.effect,
    }))
```

In `grant_list`: add `ag.effect` to the SELECT + tuple + DTO:

```rust
    let rows: Vec<(Uuid, String, Uuid, String, String)> = sqlx::query_as(
        "SELECT ag.id, ag.principal_type, ag.principal_id, r.name AS role, ag.effect \
         FROM access_grants ag JOIN roles r ON r.id = ag.role_id \
         WHERE ag.scope_type = 'object' AND ag.scope_id = $1 \
         ORDER BY ag.created_at",
    )
    .bind(id)
    .fetch_all(&st.db.pool)
    .await
    .map_err(|e| BiApiError::from(crate::BiError::Db(e)))?;
    Ok(Json(
        rows.into_iter()
            .map(|(gid, pt, pid, role, effect)| ObjectGrantDto {
                id: gid, principal_type: pt, principal_id: pid, role, effect,
            })
            .collect(),
    ))
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-bi --test share_deny`
Expected: PASS.

- [ ] **Step 6: Regenerate OpenAPI + run egret-bi + egret-workspace suites**

Run: `rtk proxy cargo run -p egret-bin -- openapi > openapi.json` (confirm `effect` on `ObjectGrantBody`/`ObjectGrantDto`).
Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-bi -p egret-workspace`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-workspace/src/rbac.rs crates/egret-bi/src/shares.rs crates/egret-bi/tests/share_deny.rs openapi.json
git commit -m "feat(authz): deny-aware object-guest admission + effect on share API"
```

---

## Task 7: Frontend — role-inheritance editor + share-modal Blocked

**Files:**
- Modify: `frontend/src/components/sharing/ShareDialog.tsx`
- Modify: the RBAC-console role editor component (locate via search)
- Modify: generated orval client (regen)
- Test: extend the relevant `*.test.tsx`

- [ ] **Step 1: Regenerate the api client**

Run: `cd frontend && rtk proxy npm run gen:api`
Confirm the generated `RoleDto` has `parentRoleId`/`inheritedPermissions` and `ObjectGrantBody`/`ObjectGrantDto` have `effect`.

- [ ] **Step 2: Share modal — add the Blocked option (failing test first)**

Find the role dropdown in `frontend/src/components/sharing/ShareDialog.tsx` (Viewer/Editor). Add a test to its `*.test.tsx` (mirror the existing add-grant test) asserting that choosing "Blocked (No access)" PUTs `{role: "ws_editor", effect: "deny"}` and that a listed deny grant renders a "No access" badge. Run it, confirm it fails.

- [ ] **Step 3: Implement the Blocked option + badge**

In `ShareDialog.tsx`: extend the role select options to `[{value:"ws_viewer",label:"Viewer"},{value:"ws_editor",label:"Editor"},{value:"__blocked__",label:"Blocked (No access)"}]`. When the user picks `__blocked__`, send `{principal_type, principal_id, role: "ws_editor", effect: "deny"}`; otherwise send the selected role with `effect: "allow"`. In the grant list, render a grant with `effect === "deny"` as a red "No access" badge instead of the role label. Show inherited denies (from the folder grants query) in the read-only inherited section with the same badge. Keep OKLCH tokens (`var(--destructive)` for the red badge, per the design-token convention).

- [ ] **Step 4: Role editor — inherits-from picker (failing test first)**

Locate the RBAC-console role editor (search `frontend/src` for `useRolesList`/`RoleDto`/a roles detail/edit surface under `routes/control-plane`). Add a test asserting the editor renders an "Inherits from" select populated with same-scope roles excluding the role itself, and that inherited permissions render read-only. Run it, confirm it fails.

- [ ] **Step 5: Implement the inherits-from picker + inherited display**

In the role editor: add an "Inherits from" `<Select>` listing roles of the same `scope`, excluding the current role id (and, to avoid an obvious cycle, excluding roles whose `parentRoleId` chain already includes this role — compute from the roles list). On change, PATCH the role with `{description, parentRoleId}`. Render `inheritedPermissions` (from the single-role GET) as read-only checked+disabled rows, badged "inherited". Own permissions stay editable via the existing set-permissions call.

- [ ] **Step 6: Run the frontend gate**

Run (from `frontend`):
```bash
rtk proxy npm run lint
rtk proxy npm run typecheck
rtk proxy npm run test
rtk proxy npm run build
```
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add frontend/
git commit -m "feat(fe): role-inheritance editor + share-modal Blocked (deny) option"
```

---

## Task 8: Verify — full CI gate

**Files:** none (verification only; fix forward if a regression appears).

- [ ] **Step 1: Backend fmt + clippy**

Run: `rtk proxy cargo fmt --all --check` (if it fails, `cargo fmt --all` and note it).
Run: `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` (ignore the pre-existing third-party `mysql_async` warning).
Expected: clean.

- [ ] **Step 2: Backend tests**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test --workspace`
Expected: all pass (the 2 pre-existing engine `open_secret` NULL-ws failures are the only known-benign exceptions on a fresh DB; any other failure is a real regression — fix forward).

- [ ] **Step 3: PDP parity explicit check**

Run: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth pdp_regression object_scope`
Expected: the allow-only regression matrix + object-scope suites pass unchanged.

- [ ] **Step 4: OpenAPI currency**

Run: `rtk proxy cargo run -p egret-bin -- openapi > /tmp/openapi_b23.json && git diff --no-index openapi.json /tmp/openapi_b23.json`
Expected: no diff (Tasks 3 + 6 committed it). If a diff exists, overwrite `openapi.json`, `cd frontend && npm run gen:api`, and commit.

- [ ] **Step 5: Frontend gate**

Run (from `frontend`): `rtk proxy npm run lint && rtk proxy npm run typecheck && rtk proxy npm run test && rtk proxy npm run build`
Expected: all pass.

- [ ] **Step 6: Commit any fix-forward changes**

If Steps 1–5 required a fix, commit it:
```bash
git add -A
git commit -m "chore(authz): B2-3 CI gate fixes"
```
Otherwise report GREEN with no changes.

---

## Notes for the security review (after Task 8)

The controller dispatches an adversarial security review before finishing the branch. Focus areas: deny-wins cannot over-permit (fail-safe); admission excludes deny (§3.4 — a lone deny must not admit); cross-scope inheritance rejected (Task 1 app + trigger); cycle prevention bounds the closure (Task 2 hop cap); deny requires `manage` to set (Task 6); the per-role deny template does not interact with the ADMIN_GRAPH forbid to escalate; allow-only PDP parity unchanged (Task 8 Step 3).
