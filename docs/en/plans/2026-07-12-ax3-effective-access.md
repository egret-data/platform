# AX-3 Effective-Access Viewer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A read-only "Access explorer" that, for any user or group, resolves and explains their effective access — roles (direct/via-group), permissions (with role + parent provenance), workspace memberships, shared objects, and explicit denies.

**Architecture:** A new `AccessResolver` in `egret-auth` computes `EffectiveAccess` directly from `access_grants` + `roles` + `role_permissions` + `user_groups` + workspaces + `analytics.*` object tables (enumerable, deterministic, with provenance — Cedar can't enumerate). A `GET /api/control-plane/access/effective` endpoint (gated `role:read`) exposes it; a new frontend page renders the sections.

**Tech Stack:** Rust (axum, sqlx, Postgres), React 18/TS/orval/Vitest.

**Spec:** `docs/en/specs/2026-07-12-ax3-effective-access-design.md`

**Verified facts:**
- `access_grants(principal_type ∈ user|group|service, principal_id uuid, role_id, scope_type ∈ platform|workspace|object, scope_id uuid, object_type text, effect text default 'allow', granted_by, created_at)` (migs 0068 + 0072).
- `roles(id, scope ∈ platform|workspace, name, description, parent_role_id)` (0003 + 0071); `role_permissions(role_id, permission)`; `user_groups(user_id, group_id)`; `groups(id, name)`; `users(id, email citext, name)`; `workspaces(id, slug, name)`.
- Effective-perms closure precedent: `egret-auth/src/session.rs::permissions()` (recursive CTE over `roles.parent_role_id`).
- Access router: `crates/egret-controlplane/src/access/mod.rs` — `router()`; handlers `State<CpState>` + `Auth(p)` + `egret_core::authz::require(&*st.pdp, &p, "<perm>")` → `Result<Json<T>, CpError>`; `#[utoipa::path]` OpenAPI; existing `/access/roles`, `/permissions`, `/users`.
- Object kind→table map: hard-coded in `egret-bi/src/shares.rs` (`dashboard→dashboards`, `chart→charts`, `dataset→datasets`, `folder→folders`, `notebook→notebooks`, `saved_query→saved_queries`), all under the `analytics` schema, each with a `name` + `workspace_id` column (folders: no name column? verify — folders have `name`).
- FE nav: `frontend/src/components/layout/Sidebar.tsx` "Access control" group (`{ label, icon, perm, to? }`); routes in `App.tsx` under `/control-plane` gated `<RequirePermission perm="role:read">`; access API in `frontend/src/api/access.ts` (orval-generated `controlPlaneAccess*` under the hood). `FilterableTable` from AX-1 at `frontend/src/components/explore/FilterableTable.tsx`. Principal picker: `listPrincipals(q, kind)` (`frontend/src/api/policies.ts`).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/egret-auth/src/effective.rs` | `EffectiveAccess` types + `AccessResolver` | **create** |
| `crates/egret-auth/src/lib.rs` | export `effective` module | modify |
| `crates/egret-auth/tests/effective_db.rs` | resolver integration tests | **create** |
| `crates/egret-controlplane/src/access/mod.rs` | `/access/effective` endpoint + DTO + OpenAPI | modify |
| `crates/egret-controlplane/src/lib.rs` (or where `CpState` is built) | wire `AccessResolver` onto state | modify |
| `frontend/src/api/access.ts` | `getEffectiveAccess` + types | modify |
| `frontend/src/routes/control-plane/access/AccessExplorer.tsx` | the viewer page | **create** |
| `frontend/src/App.tsx` | route | modify |
| `frontend/src/components/layout/Sidebar.tsx` | nav entry | modify |
| `frontend/src/routes/control-plane/access/UsersPage.tsx` + `GroupsPage.tsx` | "View access" row action | modify |
| `frontend/src/routes/control-plane/access/__tests__/AccessExplorer.test.tsx` | FE tests | **create** |

---

## Task 1: Resolver types + roles resolution (`egret-auth`)

**Files:** Create `crates/egret-auth/src/effective.rs`; modify `crates/egret-auth/src/lib.rs`. Test in Task 4.

- [ ] **Step 1: Define the types + resolver skeleton**

Create `crates/egret-auth/src/effective.rs`:

```rust
//! Effective-access resolver (AX-3): for a principal, compute what they can do,
//! where, and why — enumerable from the data (Cedar can't enumerate). Read-only.

use egret_metastore::Db;
use serde::Serialize;
use uuid::Uuid;

use crate::roles::db_err; // reuse the existing error mapper

#[derive(Debug, Serialize)]
pub struct EffectiveAccess {
    pub principal: PrincipalInfo,
    pub platform_roles: Vec<RoleSource>,
    pub permissions: Vec<EffectivePermission>,
    pub workspaces: Vec<WorkspaceMembership>,
    pub objects: Vec<ObjectGrant>,
    pub denies: Vec<DenyEntry>,
}

#[derive(Debug, Serialize)]
pub struct PrincipalInfo {
    pub r#type: String, // "user" | "group"
    pub id: String,
    pub name: Option<String>,
    pub is_platform_admin: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct RoleSource {
    pub name: String,
    pub source: String, // "direct" | "group:{groupName}"
}

#[derive(Debug, Serialize)]
pub struct PermVia {
    pub role: String,
    pub inherited_from: Option<String>, // parent role name, or null if directly on `role`
}

#[derive(Debug, Serialize)]
pub struct EffectivePermission {
    pub key: String,
    pub via: Vec<PermVia>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceMembership {
    pub workspace_id: String,
    pub workspace_slug: String,
    pub workspace_name: String,
    pub role: String,
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct ObjectGrant {
    pub kind: String,
    pub id: String,
    pub name: Option<String>,
    pub workspace_slug: Option<String>,
    pub role: String,
    pub effect: String, // "allow" | "deny"
    pub source: String,
}

#[derive(Debug, Serialize)]
pub struct DenyEntry {
    pub scope: String, // "object" | "role"
    pub target: String, // object "kind:name" or role name
    pub role: String,
    pub source: String,
}

#[derive(Clone)]
pub struct AccessResolver {
    db: Db,
}

impl AccessResolver {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}
```

Add `pub mod effective;` to `crates/egret-auth/src/lib.rs` (match the existing `pub mod …` list). If `roles::db_err` isn't `pub`, either make it `pub(crate)` or define a local `db_err` in `effective.rs` identical to the one in `roles.rs`.

- [ ] **Step 2: Resolve principal grant rows (the shared base query)**

Add a private helper that returns every grant row feeding the principal (its own + its groups' if a user), tagged with source. Add to `impl AccessResolver`:

```rust
    /// Every grant feeding this principal, with a `source` label. For a user, this
    /// is their own grants plus grants held by any group they belong to; for a
    /// group, only its own grants (egret groups don't nest).
    async fn grant_rows(&self, ptype: &str, pid: Uuid) -> Result<Vec<GrantRow>, egret_core::Error> {
        // "direct" for the principal's own grants; "group:{name}" for grants that
        // come via a group the user belongs to.
        let sql = "
            SELECT ag.scope_type, ag.scope_id, ag.object_type, ag.effect,
                   r.name AS role, r.id AS role_id, r.scope AS role_scope,
                   'direct' AS source
              FROM access_grants ag JOIN roles r ON r.id = ag.role_id
             WHERE ag.principal_type = $1 AND ag.principal_id = $2
            UNION ALL
            SELECT ag.scope_type, ag.scope_id, ag.object_type, ag.effect,
                   r.name AS role, r.id AS role_id, r.scope AS role_scope,
                   'group:' || g.name AS source
              FROM access_grants ag
              JOIN roles r  ON r.id = ag.role_id
              JOIN groups g ON g.id = ag.principal_id AND ag.principal_type = 'group'
             WHERE $1 = 'user'
               AND ag.principal_id IN (SELECT group_id FROM user_groups WHERE user_id = $2)
        ";
        sqlx::query_as::<_, GrantRow>(sql)
            .bind(ptype)
            .bind(pid)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)
    }
```

Add the row struct at the bottom of the file:

```rust
#[derive(sqlx::FromRow, Clone)]
struct GrantRow {
    scope_type: String,
    scope_id: Option<Uuid>,
    object_type: Option<String>,
    effect: String,
    role: String,
    role_id: Uuid,
    role_scope: String,
    source: String,
}
```

- [ ] **Step 3: Resolve principal info + platform roles**

Add:

```rust
    async fn principal_info(&self, ptype: &str, pid: Uuid, grants: &[GrantRow]) -> Result<PrincipalInfo, egret_core::Error> {
        let name: Option<String> = match ptype {
            "user" => sqlx::query_scalar("SELECT email::text FROM users WHERE id = $1"),
            "group" => sqlx::query_scalar("SELECT name FROM groups WHERE id = $1"),
            _ => return Err(egret_core::Error::InvalidInput(format!("bad principal_type: {ptype}"))),
        }
        .bind(pid)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        let is_platform_admin = grants
            .iter()
            .any(|g| g.scope_type == "platform" && g.role == "platform_admin" && g.effect == "allow");
        Ok(PrincipalInfo { r#type: ptype.to_string(), id: pid.to_string(), name, is_platform_admin })
    }

    fn platform_roles(grants: &[GrantRow]) -> Vec<RoleSource> {
        grants.iter()
            .filter(|g| g.scope_type == "platform" && g.effect == "allow")
            .map(|g| RoleSource { name: g.role.clone(), source: g.source.clone() })
            .collect()
    }
```

- [ ] **Step 4: Compile check**

Run: `cargo build -p egret-auth`
Expected: compiles (the public `resolve` method that assembles everything comes in Task 3; for now the module compiles with these helpers even if unused — add `#[allow(dead_code)]` on the helpers if the build warns, to be removed when `resolve` uses them in Task 3).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/effective.rs crates/egret-auth/src/lib.rs
git commit -m "feat(auth): EffectiveAccess types + AccessResolver grant/roles resolution"
```

---

## Task 2: Resolver — effective permissions with provenance

**Files:** Modify `crates/egret-auth/src/effective.rs`. Test in Task 4.

- [ ] **Step 1: Add the permission-provenance resolver**

For each granted **platform** role (direct or via group), walk its parent chain and collect permissions, tagging each permission with `{ role: <granted role>, inherited_from: <parent name or null> }`. Add to `impl AccessResolver`:

```rust
    /// Effective permissions for the granted platform roles, with provenance.
    /// For each granted role, walks its parent chain (role hierarchy) and records,
    /// per permission, which granted role conferred it and (if it came from a
    /// parent) which parent. Skipped entirely for platform_admin (superuser).
    async fn permissions(&self, platform_roles: &[RoleSource]) -> Result<Vec<EffectivePermission>, egret_core::Error> {
        use std::collections::BTreeMap;
        let mut acc: BTreeMap<String, Vec<PermVia>> = BTreeMap::new();
        for rs in platform_roles {
            if rs.name == "platform_admin" {
                continue; // superuser — enumerated permissions are meaningless
            }
            // (permission, owning_role_name) for this granted role's whole closure.
            let rows: Vec<(String, String)> = sqlx::query_as(
                "WITH RECURSIVE closure AS ( \
                   SELECT id, name, parent_role_id FROM roles WHERE name = $1 AND scope = 'platform' \
                   UNION \
                   SELECT r.id, r.name, r.parent_role_id FROM roles r \
                     JOIN closure c ON r.id = c.parent_role_id \
                 ) \
                 SELECT rp.permission, c.name AS owner FROM closure c \
                   JOIN role_permissions rp ON rp.role_id = c.id",
            )
            .bind(&rs.name)
            .fetch_all(&self.db.pool)
            .await
            .map_err(db_err)?;
            for (perm, owner) in rows {
                let inherited_from = if owner == rs.name { None } else { Some(owner) };
                acc.entry(perm).or_default().push(PermVia { role: rs.name.clone(), inherited_from });
            }
        }
        Ok(acc.into_iter().map(|(key, via)| EffectivePermission { key, via }).collect())
    }
```

Note: dedupe `via` if the same (role, inherited_from) pair appears twice (a role reachable by two paths) — before pushing, skip if an identical `PermVia` is already present for that key. Implement that dedupe inline.

- [ ] **Step 2: Compile check**

Run: `cargo build -p egret-auth`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/egret-auth/src/effective.rs
git commit -m "feat(auth): resolve effective permissions with role/parent provenance"
```

---

## Task 3: Resolver — workspaces, objects, denies + public `resolve`

**Files:** Modify `crates/egret-auth/src/effective.rs`. Test in Task 4.

- [ ] **Step 1: Workspace memberships**

```rust
    async fn workspaces(&self, grants: &[GrantRow]) -> Result<Vec<WorkspaceMembership>, egret_core::Error> {
        let mut out = Vec::new();
        for g in grants.iter().filter(|g| g.scope_type == "workspace" && g.effect == "allow") {
            let Some(ws_id) = g.scope_id else { continue };
            let row: Option<(Uuid, String, String)> =
                sqlx::query_as("SELECT id, slug, name FROM workspaces WHERE id = $1")
                    .bind(ws_id).fetch_optional(&self.db.pool).await.map_err(db_err)?;
            if let Some((id, slug, name)) = row {
                out.push(WorkspaceMembership {
                    workspace_id: id.to_string(), workspace_slug: slug, workspace_name: name,
                    role: g.role.clone(), source: g.source.clone(),
                });
            }
        }
        Ok(out)
    }
```

- [ ] **Step 2: Object grants (name via a hard-coded kind→table map)**

`analytics.{table}` all have `name` + `workspace_id`. Resolve safely (kind is mapped to a `'static` literal — never interpolate the raw kind).

```rust
    fn object_table(kind: &str) -> Option<&'static str> {
        match kind {
            "dashboard" => Some("dashboards"),
            "chart" => Some("charts"),
            "dataset" => Some("datasets"),
            "notebook" => Some("notebooks"),
            "saved_query" => Some("saved_queries"),
            "folder" => Some("folders"),
            _ => None,
        }
    }

    async fn objects(&self, grants: &[GrantRow]) -> Result<Vec<ObjectGrant>, egret_core::Error> {
        let mut out = Vec::new();
        for g in grants.iter().filter(|g| g.scope_type == "object") {
            let (Some(kind), Some(id)) = (g.object_type.as_deref(), g.scope_id) else { continue };
            let (name, ws_slug) = if let Some(table) = Self::object_table(kind) {
                let sql = format!(
                    "SELECT o.name::text, w.slug FROM analytics.{table} o \
                     JOIN workspaces w ON w.id = o.workspace_id WHERE o.id = $1"
                );
                sqlx::query_as::<_, (Option<String>, Option<String>)>(&sql)
                    .bind(id).fetch_optional(&self.db.pool).await.map_err(db_err)?
                    .unwrap_or((None, None))
            } else {
                (None, None)
            };
            out.push(ObjectGrant {
                kind: kind.to_string(), id: id.to_string(), name, workspace_slug: ws_slug,
                role: g.role.clone(), effect: g.effect.clone(), source: g.source.clone(),
            });
        }
        Ok(out)
    }
```

(If `folders` has no `name` column, the query errors at runtime — verify: folders DO have `name`. If a table lacks `name`, special-case it. Confirm before running.)

- [ ] **Step 3: Denies + the public `resolve`**

```rust
    fn denies(objects: &[ObjectGrant], grants: &[GrantRow]) -> Vec<DenyEntry> {
        let mut out: Vec<DenyEntry> = objects.iter().filter(|o| o.effect == "deny")
            .map(|o| DenyEntry {
                scope: "object".into(),
                target: format!("{}:{}", o.kind, o.name.clone().unwrap_or_else(|| o.id.clone())),
                role: o.role.clone(), source: o.source.clone(),
            }).collect();
        // Role-scoped denies (effect='deny' on a platform/workspace grant), if any.
        for g in grants.iter().filter(|g| g.effect == "deny" && g.scope_type != "object") {
            out.push(DenyEntry {
                scope: "role".into(), target: g.role.clone(),
                role: g.role.clone(), source: g.source.clone(),
            });
        }
        out
    }

    /// Resolve a principal's full effective access.
    pub async fn resolve(&self, ptype: &str, pid: Uuid) -> Result<EffectiveAccess, egret_core::Error> {
        let grants = self.grant_rows(ptype, pid).await?;
        let principal = self.principal_info(ptype, pid, &grants).await?;
        let platform_roles = Self::platform_roles(&grants);
        let permissions = if principal.is_platform_admin { Vec::new() } else { self.permissions(&platform_roles).await? };
        let workspaces = self.workspaces(&grants).await?;
        let objects = self.objects(&grants).await?;
        let denies = Self::denies(&objects, &grants);
        Ok(EffectiveAccess { principal, platform_roles, permissions, workspaces, objects, denies })
    }
```

Remove any `#[allow(dead_code)]` added in Task 1 now that `resolve` uses the helpers.

- [ ] **Step 4: Compile + clippy**

Run: `cargo build -p egret-auth && cargo clippy -p egret-auth --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-auth/src/effective.rs
git commit -m "feat(auth): resolve workspaces/objects/denies + public AccessResolver::resolve"
```

---

## Task 4: Resolver integration tests

**Files:** Create `crates/egret-auth/tests/effective_db.rs`.

Read an existing `egret-auth` DB test (e.g. `crates/egret-auth/tests/session_db.rs`) for the harness (DB pool, `DATABASE_URL` self-skip, seeding users/groups/roles/grants). Reuse it.

- [ ] **Step 1: Write the tests**

```rust
// 1. Direct role + role via group both resolve with correct source; perms union.
#[tokio::test]
async fn resolves_direct_and_group_roles_with_source() { /* … */ }

// 2. Hierarchy provenance: a role whose PARENT grants perm P -> P has inherited_from=parent;
//    a permission directly on the granted role has inherited_from=None.
#[tokio::test]
async fn permission_provenance_marks_inherited() { /* … */ }

// 3. Workspace membership (direct + via group) resolves with slug/name + source.
#[tokio::test]
async fn resolves_workspace_memberships() { /* … */ }

// 4. Object allow grant appears in objects; a deny grant appears in denies.
#[tokio::test]
async fn resolves_objects_and_denies() { /* … */ }

// 5. Group principal resolves its own grants (no via-group layer);
//    a platform_admin principal -> is_platform_admin true and permissions empty.
#[tokio::test]
async fn group_principal_and_platform_admin() { /* … */ }
```

Fill each body using the harness helpers (seed a role hierarchy via `roles.parent_role_id`, grants via `access_grants`, a group + `user_groups`, a workspace, and an object row in `analytics.charts`). Assert concrete values (role names, `source` strings, `inherited_from`, counts, `is_platform_admin`).

- [ ] **Step 2: Run (ephemeral DB — dev Postgres `egret-dev-postgres-1` egret/egret; never the main `egret` DB, never two workspace suites at once)**

```bash
DB=ax3_$$; docker exec egret-dev-postgres-1 psql -U egret -d egret -c "CREATE DATABASE $DB;"
DATABASE_URL="postgres://egret:egret@localhost:5432/$DB" cargo test -p egret-auth --test effective_db 2>&1 | tail -20
docker exec egret-dev-postgres-1 psql -U egret -d egret -c "DROP DATABASE $DB;"
```
Expected: 5 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/egret-auth/tests/effective_db.rs
git commit -m "test(auth): AccessResolver — sources, provenance, workspaces, objects/denies, group/superuser"
```

---

## Task 5: Endpoint `/access/effective` (`egret-controlplane`)

**Files:** Modify `crates/egret-controlplane/src/access/mod.rs`; wire the resolver onto `CpState` (find where `CpState` is constructed — grep `CpState {` — and add an `AccessResolver` field, or construct it in the handler from the existing db handle on `CpState`).

- [ ] **Step 1: Add the route + handler + DTO**

In `access/mod.rs` `router()`, add alongside `/permissions`:
```rust
        .route("/access/effective", get(get_effective_access))
```

Add the handler (mirror `list_users`'s state/auth/require pattern; return the resolver's `EffectiveAccess` — re-export or `#[derive(ToSchema)]` on it, see Step 2). Accept the principal via query params; resolve an email/group-name to a UUID like AX-1's member add (or accept a UUID directly):

```rust
#[derive(Debug, serde::Deserialize)]
struct EffectiveQuery { principal_type: String, principal_id: String }

#[utoipa::path(
    get,
    path = "/api/control-plane/access/effective",
    tag = "control-plane-access",
    params(
        ("principal_type" = String, Query, description = "user | group"),
        ("principal_id" = String, Query, description = "UUID, or a user email / group name"),
    ),
    responses(
        (status = 200, description = "Effective access for the principal", body = EffectiveAccess),
        (status = 403, description = "Missing role:read permission"),
        (status = 404, description = "No such principal"),
    ),
)]
async fn get_effective_access(
    State(st): State<CpState>,
    Auth(p): Auth,
    axum::extract::Query(q): axum::extract::Query<EffectiveQuery>,
) -> Result<Json<egret_auth::effective::EffectiveAccess>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "role:read").await?;
    let pid = resolve_principal_id(&st, &q.principal_type, &q.principal_id).await?;
    let ea = st.access_resolver.resolve(&q.principal_type, pid).await.map_err(CpError::from)?;
    Ok(Json(ea))
}
```

Add `resolve_principal_id` (UUID-or-email/name → UUID, 404 if unknown) — mirror AX-1's `MemberRepo::resolve_principal`, or query `users`/`groups` directly here. Confirm `CpError` has a `From<egret_core::Error>` (other handlers map errors — match the real conversion; if it's `?`-able, use that).

- [ ] **Step 2: Make `EffectiveAccess` an OpenAPI schema**

Add `#[derive(utoipa::ToSchema)]` to `EffectiveAccess` and every nested type in `egret-auth/src/effective.rs` (utoipa is already a dep of egret-auth if other types there derive it; if not, gate the derive so egret-auth still builds — check whether `utoipa` is in egret-auth's Cargo.toml; if absent, either add it or define thin DTO mirrors in `access/mod.rs` and map. PREFER deriving `ToSchema` directly if utoipa is available). Register the handler in the crate's `#[openapi(paths(...))]` doc struct + the component schemas list (find it in `access/mod.rs` or the crate's openapi()).

- [ ] **Step 3: Wire `AccessResolver` onto `CpState`**

Find `CpState` construction; add `pub access_resolver: egret_auth::effective::AccessResolver` and build it with the existing db handle (`AccessResolver::new(db.clone())`). Match how other repos are attached to `CpState`.

- [ ] **Step 4: Build + clippy**

Run: `cargo build -p egret-controlplane && cargo clippy -p egret-controlplane --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-controlplane/src/access/mod.rs crates/egret-controlplane/src/lib.rs
git commit -m "feat(cp): GET /access/effective endpoint (role:read) exposing AccessResolver"
```

---

## Task 6: Regenerate client + FE access API

**Files:** Modify `frontend/src/api/access.ts` (+ regenerate the generated client).

- [ ] **Step 1: Regenerate the OpenAPI + orval client**

Run the repo's generation (as prior increments did): dump the spec (`cargo run -p egret-bin -- openapi > openapi.json`) then `cd frontend && npm run gen:api`. Confirm a `controlPlaneAccess…Effective` (or similarly named) fetch fn + an `EffectiveAccess` model appear under `frontend/src/api/generated/`.

- [ ] **Step 2: Add a typed wrapper in `access.ts`**

Mirror the existing `listPermissions()` style in `frontend/src/api/access.ts`:
```ts
import type { EffectiveAccess } from "./generated/model"; // exact name from Step 1
export type { EffectiveAccess };
export async function getEffectiveAccess(principalType: "user" | "group", principalId: string): Promise<EffectiveAccess> {
  return (await controlPlaneAccessGetEffectiveAccess({ principal_type: principalType, principal_id: principalId })).data as EffectiveAccess;
}
```
Use the EXACT generated fn name + its param shape (query object) from Step 1.

- [ ] **Step 3: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add frontend/openapi.json frontend/src/api/generated frontend/src/api/access.ts
git commit -m "feat(fe): regen client + getEffectiveAccess wrapper"
```

---

## Task 7: Access Explorer page

**Files:** Create `frontend/src/routes/control-plane/access/AccessExplorer.tsx`.

Mirror the structure/patterns of `frontend/src/routes/control-plane/access/RolesPage.tsx` (Card + section layout, `useQuery`, `useAuth`) and reuse: `FilterableTable` (`@/components/explore/FilterableTable`), `listPrincipals` (`@/api/policies`), `getEffectiveAccess` (Task 6), shadcn `Badge`/`Card`/`Select`.

- [ ] **Step 1: Write the page**

Implement `AccessExplorer` with:
- **Principal state from URL:** read `?user=` / `?group=` via `useSearchParams`; a picker (User/Group toggle + `listPrincipals` search) sets it and updates the URL (`setSearchParams`). Default: nothing selected → prompt "Pick a user or group".
- **Query:** `const q = useQuery({ queryKey: ["cp","effective",ptype,pid], queryFn: () => getEffectiveAccess(ptype, pid), enabled: !!pid, retry: false });`
- **Header:** principal name + type badge; if `data.principal.is_platform_admin`, a "Superuser" badge (styled with `--highlight`) + note "Bypasses all permission checks — has every permission."
- **Platform roles:** `data.platform_roles` as chips, each with its `source` (muted suffix "· direct" / "· via {group}").
- **Effective permissions:** when NOT superuser — a `FilterableTable` over `data.permissions`, columns: Permission (`key`, mono), and "Granted via" rendering each `via` entry as "{role}" + (if `inherited_from`) " (inherited from {inherited_from})". `value` accessors for search/sort on `key`.
- **Workspaces:** a small table/list of `data.workspaces` (name, role, source).
- **Shared objects:** `data.objects` (kind, name, workspace, role, effect badge).
- **Denies / Blocked:** `data.denies` rendered in a `--destructive`-tinted section (only when non-empty).
- Loading / forbidden(403 → "You need role:read")/ empty states, like the other access pages.

Keep it focused; extract a `<Section title>` in-file helper to avoid repetition. If it exceeds ~250 lines, extract the permissions table into a small in-file component.

- [ ] **Step 2: Typecheck + lint**

Run: `cd frontend && npm run typecheck && npm run lint`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/routes/control-plane/access/AccessExplorer.tsx
git commit -m "feat(fe): Access Explorer page (effective access with provenance)"
```

---

## Task 8: Nav entry + route + "View access" deep-links

**Files:** Modify `frontend/src/App.tsx`, `frontend/src/components/layout/Sidebar.tsx`, `UsersPage.tsx`, `GroupsPage.tsx`.

- [ ] **Step 1: Route (`App.tsx`)**

Add the import + a route under `/control-plane` next to the `permissions` route:
```tsx
import AccessExplorer from "@/routes/control-plane/access/AccessExplorer";
// …
          <Route path="access-explorer" element={<RequirePermission perm="role:read"><AccessExplorer /></RequirePermission>} />
```

- [ ] **Step 2: Nav entry (`Sidebar.tsx`)**

In the "Access control" group items, add after "Permissions":
```tsx
      { label: "Access explorer", icon: Compass, perm: "role:read", to: "access-explorer" },
```
Import `Compass` from lucide-react. (The `to` override was added in AX-1; if the label slug already equals the path you may omit `to` — here "Access explorer" slugs to `access-explorer` anyway, but pass `to` explicitly for safety.)

- [ ] **Step 3: "View access" row action (`UsersPage.tsx`, `GroupsPage.tsx`)**

In each page's row actions `DropdownMenu`, add an item that navigates to the explorer deep-link:
```tsx
// UsersPage row menu:
<DropdownMenuItem onClick={() => navigate(`/control-plane/access-explorer?user=${u.id}`)}>
  <Compass className="mr-2 h-4 w-4" /> View access
</DropdownMenuItem>
```
(`GroupsPage`: `?group=${g.id}`.) Add `useNavigate` + the `Compass` import where missing. Use the real id field names from those pages' row types.

- [ ] **Step 4: Typecheck + build**

Run: `cd frontend && npm run typecheck && npm run build`
Expected: clean; the nav shows "Access explorer"; Users/Groups rows expose "View access".

- [ ] **Step 5: Commit**

```bash
git add frontend/src/App.tsx frontend/src/components/layout/Sidebar.tsx frontend/src/routes/control-plane/access/UsersPage.tsx frontend/src/routes/control-plane/access/GroupsPage.tsx
git commit -m "feat(fe): Access explorer nav + route + View-access deep-links from Users/Groups"
```

---

## Task 9: FE tests

**Files:** Create `frontend/src/routes/control-plane/access/__tests__/AccessExplorer.test.tsx`.

Copy the harness from a sibling access test (`frontend/src/routes/control-plane/access/__tests__/*.test.tsx` — e.g. `GroupsPage.test.tsx`): QueryClientProvider + MemoryRouter (with an initial entry carrying `?user=<id>`), and `vi.mock` of `@/api/access` (`getEffectiveAccess`) + `@/api/policies` (`listPrincipals`) + `@/auth/useAuth`.

- [ ] **Step 1: Write the tests**

```tsx
it("renders roles, permissions with provenance, workspaces, and a highlighted deny", async () => { /* mock a full EffectiveAccess; assert a role chip, a perm row showing "inherited from …", a workspace, and the deny section */ });
it("preselects the principal from ?user= and fetches", async () => { /* MemoryRouter initialEntries with ?user=; assert getEffectiveAccess called with ("user", id) */ });
it("shows the Superuser note for a platform-admin principal", async () => { /* is_platform_admin:true -> superuser badge, no perm table */ });
```

Fill bodies against the mocked module; assert concrete rendered text.

- [ ] **Step 2: Run**

Run: `cd frontend && npm run test -- AccessExplorer 2>&1 | tail -20`
Expected: all pass.

- [ ] **Step 3: Commit**

```bash
git add frontend/src/routes/control-plane/access/__tests__/AccessExplorer.test.tsx
git commit -m "test(fe): Access Explorer — sections, deep-link preselect, superuser"
```

---

## Task 10: Full CI + review + finish

**Files:** none.

- [ ] **Step 1: Backend sweep** — `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`, then `cargo test -p egret-auth` on an ephemeral DB (as in Task 4). Expect green.
- [ ] **Step 2: Frontend sweep** — `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`. Expect green.
- [ ] **Step 3: Adversarial security/UX review** over the branch diff. Focus: the endpoint is gated `role:read` and returns nothing beyond the authz graph a role-reader may see (no data-plane/Ranger, no secrets); `kind`/table names are never interpolated from user input (hard-coded map); principal-id resolution is server-side; the SQL correctly unions direct + group grants and the hierarchy closure produces correct provenance (no missing/extra perms); platform_admin path doesn't leak an empty-but-misleading permission set (superuser note is shown). Fix any HIGH/MEDIUM in-branch.
- [ ] **Step 4: Optional live smoke** — with the stack running, open `/control-plane/access-explorer?user=<alice id>`; alice (data_steward) shows her platform role + data_steward's permissions with provenance; a `?user=<admin id>` shows the Superuser note.
- [ ] **Step 5: Hand off to `superpowers:finishing-a-development-branch`** → Push + PR → squash-merge `--admin`; update memory.

---

## Self-Review

**Spec coverage:** resolver+types → Tasks 1-3 ✓; platform_admin superuser → Task 3 (`permissions` empty when admin) ✓; group-no-nesting → Task 1 `grant_rows` (group path only unions groups for `$1='user'`) ✓; endpoint+gating+id-resolution → Task 5 ✓; client → Task 6 ✓; page with all 6 sections + provenance + FilterableTable → Task 7 ✓; nav+route+deep-link → Task 8 ✓; tests → Tasks 4, 9, 10 ✓; out-of-scope (simulator, Ranger, nesting) not built ✓.

**Placeholder scan:** backend code is complete SQL/Rust. Task 4/9 test bodies are specified as named scenarios + assertions against the sibling harness the implementer must read (`session_db.rs` / access test) — not fully coded, because they depend on those harnesses; each names the exact arrange/assert. Task 7's presentational markup defers to `RolesPage` + `FilterableTable` (concrete sources) while all data wiring (query, sections, provenance rendering, superuser branch) is spelled out. No `TBD`.

**Type consistency:** `EffectiveAccess` and nested types (`RoleSource{name,source}`, `PermVia{role,inherited_from}`, `EffectivePermission{key,via}`, `WorkspaceMembership`, `ObjectGrant{effect}`, `DenyEntry`) are defined once in Task 1 and consumed unchanged by Tasks 3/5/7. `AccessResolver::resolve(ptype, pid) -> EffectiveAccess` is the single entry used by the endpoint. `role:read` gating is consistent (endpoint + route + nav). `object_table` kind map matches `shares.rs`. FE `getEffectiveAccess(ptype, id)` matches the endpoint's query params.
