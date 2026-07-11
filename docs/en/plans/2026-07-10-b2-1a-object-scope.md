# B2-1a — Object-level authorization (backend) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Backend object-level authorization for BI content — folder/item grants resolved along Item→Folder→Workspace→Platform, reusing workspace roles at object scope. Delivers the PDP object-scope model, the object-grant store + share API, and the BI per-object handler migration.

**Architecture:** Non-admin Cedar templates key their resource operator on the role's scope (`workspace` → `in` for cascade, `platform` → `==` for anti-leak). A generic Cedar `Object` entity carries the request resource's ancestor chain. `Resource::Object` + `require_object()` are the new PEP; object grants are net-new rows in `access_grants`.

**Tech Stack:** Rust, `cedar-policy` 4.11.2, axum, `sqlx`/Postgres.

**Reference:** [B2-1a design](../specs/2026-07-10-b2-1a-object-scope-design.md). Reuse the migration/test patterns from the merged B1b-* work.

---

## Task 1: Template operator by role scope (unblocks inheritance; matrix stays 60/60)

**Files:** `crates/egret-auth/src/pdp/templates.rs` (+ its unit tests)

- [ ] **Step 1: Make the non-admin operator depend on scope.** In `template_for_role(role_name, scope, perms)`, the non-admin branch currently always emits `resource == ?resource`. Change it to:
```rust
    // platform_admin branch unchanged (returns the unrestricted `resource in ?resource`).
    let op = if scope == "workspace" { "in" } else { "==" };
    // workspace roles cascade to objects (folder→item, ws→object, object→itself via `in`);
    // platform roles keep `==` so a platform grant never matches a workspace/object resource
    // (anti-leak from B1b-2). Object grants use workspace roles, so they cascade correctly.
    format!("permit(principal in ?principal, action in [{actions}], resource {op} ?resource);")
```

- [ ] **Step 2: Update the template unit tests.** The B1b-2 tests assert non-admin uses `==`. Split them: a `platform`-scope role → `resource == ?resource`; a `workspace`-scope role → `resource in ?resource`; `platform_admin` → unrestricted `action, resource in ?resource`. Keep the "workspace role named platform_admin is not unrestricted" test (it should now be `resource in ?resource` but WITH its own perm list, not the bare `action`).

- [ ] **Step 3: Run.** `rtk proxy cargo test -p egret-auth pdp::templates` → PASS. Then `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test -p egret-auth --test pdp_regression` → **still 60/60** (the matrix has only Platform/Workspace requests; for a direct Workspace(W) request `resource in Workspace::"W"` reflexively equals `==`). If it changed, STOP and report BLOCKED. Also run `admin_graph_forbid` → still passes.

- [ ] **Step 4: Commit.**
```bash
git add crates/egret-auth/src/pdp/templates.rs
git commit -m "feat(authz): template resource operator keys on role scope (in for workspace, == for platform)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `Resource::Object` + `ObjectRef` + Cedar object hierarchy + `require_object`

**Files:** `crates/egret-core/src/authz.rs`, `crates/egret-auth/src/pdp/entities.rs`

- [ ] **Step 1: Add the resource variant.** In `crates/egret-core/src/authz.rs`:
```rust
#[derive(Debug, Clone)]
pub struct ObjectRef {
    pub kind: String,             // "dashboard" | "chart" | "dataset" | "folder"
    pub id: uuid::Uuid,
    pub folder_id: Option<uuid::Uuid>,
    pub workspace_id: crate::ids::WorkspaceId,
}

#[derive(Debug, Clone)]
pub enum Resource {
    Platform,
    Workspace(WorkspaceId),
    Object(ObjectRef),
}
```

- [ ] **Step 2: Add the `require_object` PEP.** In the same file:
```rust
/// Enforce `action` on a specific object. Subject comes from `ctx`; the resource
/// is the object + its ancestry. `Ok(())` if allowed, `Error::Forbidden` otherwise.
pub async fn require_object(
    pdp: &dyn Pdp,
    ctx: &crate::WorkspaceContext,
    action: &str,
    object: ObjectRef,
) -> crate::Result<()> {
    debug_assert_eq!(object.workspace_id, ctx.workspace_id);
    let decision = pdp
        .authorize(&ctx.principal.subject(), action, &Resource::Object(object))
        .await?;
    if decision.allow { Ok(()) } else { Err(crate::Error::Forbidden) }
}
```
Re-export `require_object`, `ObjectRef` from `crates/egret-core/src/lib.rs`. Add unit tests for `require_object` with a stub `Pdp` (allow/deny → Ok/Forbidden), mirroring the existing `require` tests.

- [ ] **Step 3: Build the object hierarchy in `entities.rs`.** In `crates/egret-auth/src/pdp/entities.rs`:
  - `resource_uid(&Resource)` gains an `Object(o)` arm → `format!("Egret::Object::\"{}:{}\"", o.kind, o.id).parse()`.
  - `build_entities(subject, resource)` gains the `Object` case: create the object entity with parent = its folder (`Egret::Object::"folder:<fid>"`) if `folder_id`, else its workspace; create the folder entity (parent = workspace) when present; create the workspace entity (parent = `Platform::"root"`); create Platform. (For `Platform`/`Workspace` cases, keep the existing logic.)
  - `scope_uid(scope_type, scope_id, object_type)` — extend to handle `scope_type == "object"` → `Egret::Object::"<object_type>:<scope_id>"`. (Add the `object_type` argument; thread it through `link_grants`.)

- [ ] **Step 4: Unit test the Cedar object path** (in `entities.rs` tests): a `ws_viewer` template (now `in`) linked with `?resource = Object::"folder:<f1>"` (a folder grant) → authorizes `dashboard:read` on `Object::"dashboard:<d1>"` whose parent chain is `folder:<f1>` → `Workspace` → `Platform`; and DENIES a dashboard in a different folder. And an item grant (`?resource = Object::"dashboard:<d1>"`) authorizes only `d1`.

- [ ] **Step 5: Run + commit.** `rtk proxy cargo test -p egret-core -p egret-auth pdp::entities authz` → PASS. Note: `CedarPdp::authorize` won't compile yet against the new `grants_for`/`scope_uid` signature — that's Task 3; if the crate doesn't build, complete the entities/link_grants signature changes here and leave `authorize`'s call updated in Task 3 (or make this + Task 3 one commit if the compiler forces it). Commit:
```bash
git add crates/egret-core/ crates/egret-auth/src/pdp/entities.rs
git commit -m "feat(authz): Resource::Object + require_object + Cedar object hierarchy

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `grants_for` object grants + `CedarPdp::authorize` wiring

**Files:** `crates/egret-auth/src/grants.rs`, `crates/egret-auth/src/pdp/mod.rs`

- [ ] **Step 1: Extend `Grant` + `grants_for`.** In `crates/egret-auth/src/grants.rs`:
  - `Grant` gains `pub object_type: Option<String>`.
  - `grants_for` signature → `grants_for(&self, user_id: UserId, ws: Option<WorkspaceId>, object_scope_ids: &[uuid::Uuid]) -> Result<Vec<Grant>>`. SQL: select `role_id, scope_type, scope_id, object_type`, and the WHERE gains `OR (ag.scope_type = 'object' AND ag.scope_id = ANY($3))`. Bind `$3` = `object_scope_ids`.

- [ ] **Step 2: Wire `authorize`.** In `crates/egret-auth/src/pdp/mod.rs::authorize`: compute the object scope ids and workspace from the resource:
```rust
    let (ws, object_ids) = match resource {
        Resource::Platform => (None, vec![]),
        Resource::Workspace(w) => (Some(*w), vec![]),
        Resource::Object(o) => (
            Some(o.workspace_id),
            o.folder_id.map_or_else(|| vec![o.id], |f| vec![o.id, f]),
        ),
    };
    let grants = self.grants.grants_for(subject.user_id, ws, &object_ids).await?;
```
Then `link_grants(..., &grants, ...)` passes each grant's `object_type` into `scope_uid` so object grants link `?resource = Object::"<type>:<scope_id>"`. Keep the `is_platform_admin` context computation (unchanged — platform grants only).

- [ ] **Step 3: Update the other `grants_for` call sites** (grep `grants_for(`): the `AccessGrantStore` test + anywhere else — pass `&[]` for the object ids where object-scope isn't relevant.

- [ ] **Step 4: End-to-end DB test** (`crates/egret-auth/tests/object_scope.rs`, new): seed a user U (not a workspace member), a workspace W, a folder F, a dashboard D in F; write an `access_grants` object grant of `ws_viewer` to U on folder F (`scope_type='object', object_type='folder', scope_id=F`); build a `CedarPdp`; assert `pdp.authorize(U, "dashboard:read", Resource::Object{kind:"dashboard", id:D, folder_id:Some(F), workspace_id:W})` → **allow**; a dashboard D2 in a different folder → **deny**; an item grant on D2 for U → U can read D2 only. Also assert a workspace grant still covers all objects, and a platform (non-admin) grant covers none.

- [ ] **Step 5: Run + commit.** `export DATABASE_URL=... && rtk proxy cargo test -p egret-auth --test object_scope --test pdp_regression -- --nocapture` → object_scope PASS, pdp_regression still 60/60. `cargo build --workspace`. Commit `feat(authz): grants_for object grants + CedarPdp object-scope authorize`.

---

## Task 4: Object-grant share API (write / revoke / list)

**Files:** `crates/egret-bi/src/` (new `shares.rs` module + route registration), `crates/egret-bin/src/main.rs` (if new state is needed — BiState already has `pdp`)

- [ ] **Step 1: `ObjectRef` resolver helper.** Add a helper (in `egret-bi`) `async fn object_ref(db, kind: &str, id: Uuid) -> Result<ObjectRef>` that does `SELECT folder_id, workspace_id FROM <table-for-kind> WHERE id = $1` (table = dashboards|charts|datasets|folders — for `folder`, `folder_id` is NULL and `workspace_id` comes from the folders table). Returns `Error::NotFound` if absent.

- [ ] **Step 2: The three endpoints** (workspace-scoped, in the BI router):
  - `PUT /ws/{workspace}/objects/{kind}/{id}/grants` — body `{ principal_type, principal_id, role }` (role ∈ `ws_viewer`|`ws_editor`). Resolve `object_ref`; gate `require_object(&*st.pdp, &ctx, "<kind>:manage", object_ref.clone()).await?`; look up the `role_id` for the role name; `INSERT INTO access_grants (principal_type, principal_id, role_id, scope_type, object_type, scope_id) VALUES (..., 'object', $kind, $id) ON CONFLICT DO NOTHING`.
  - `DELETE /ws/{workspace}/objects/{kind}/{id}/grants/{grant_id}` — same manage gate; `DELETE FROM access_grants WHERE id = $grant_id AND scope_type='object' AND scope_id = $id` (scope by the object so you can't delete an unrelated grant).
  - `GET /ws/{workspace}/objects/{kind}/{id}/grants` — same object, but gate on `"<kind>:read"`; return the object's direct grants (join `roles` for the role name, resolve principal display). Add `#[derive(ToSchema)]` DTOs so the OpenAPI picks them up.
  - `<kind>:manage`/`<kind>:read` = `dashboard:manage`/`dashboard:read` etc. — the object's own action namespace.

- [ ] **Step 3: Tests** (`crates/egret-bi/tests/shares.rs` or extend `http.rs`, real `CedarPdp`): a workspace `ws_editor` (manage) grants `ws_viewer` to an outside user U on a dashboard → 2xx; U (via a fresh session) can then `GET` that dashboard (200) but not a sibling (403); a `ws_viewer` caller cannot grant (403 on PUT); revoke removes access; list returns the grant.

- [ ] **Step 4: Run + commit.** `export DATABASE_URL=... && rtk proxy cargo test -p egret-bi -- --nocapture` → PASS. `cargo build --workspace`. Commit `feat(authz): object-grant share API (grant/revoke/list) for BI content`.

---

## Task 5: Migrate `datasets/{id}` handlers to object scope

**Files:** `crates/egret-bi/src/routes.rs`, tests

- [ ] **Step 1: Enumerate + migrate.** `rtk proxy grep -n "datasets/{id}" crates/egret-bi/src/routes.rs` and find each handler that operates on a specific dataset id (get, update, delete, refresh, metrics CRUD, columns CRUD, set_folder, trash, restore, bookmark). For each, replace `require(&*st.pdp, &ctx, DS_READ|DS_MANAGE).await?` with `require_object(&*st.pdp, &ctx, DS_READ|DS_MANAGE, st.object_ref("dataset", id).await?).await?` (read for GETs, manage for mutations). CREATE (`POST /datasets`) and LIST (`GET /datasets`) stay workspace-scope `require(&ctx, ...)`. Keep each PERM constant unchanged (only read vs manage per handler as today).

- [ ] **Step 2: Test.** Extend the datasets tests: an outside user granted `ws_viewer` on a dataset can GET it but not update it; a workspace `ws_editor` still does everything (member behavior unchanged).

- [ ] **Step 3: Run + commit.** `cargo test -p egret-bi`, `cargo build --workspace`. Commit `feat(authz): migrate dataset per-object handlers to object scope`.

---

## Task 6: Migrate `charts/{id}` handlers to object scope

Same as Task 5, for `charts/{id}` (`CH_READ`/`CH_MANAGE`): get, update, delete, set_folder, duplicate, trash, restore, bookmark. `st.object_ref("chart", id)`. LIST/CREATE stay workspace-scope. Test + commit `feat(authz): migrate chart per-object handlers to object scope`.

---

## Task 7: Migrate `dashboards/{id}` handlers to object scope

Same as Task 5, for `dashboards/{id}` (`DB_READ`/`DB_MANAGE`) and its sub-resources (cards, tabs, filters, set_folder, trash, restore, bookmark, etc.). `st.object_ref("dashboard", id)`. LIST/CREATE stay workspace-scope. Test + commit `feat(authz): migrate dashboard per-object handlers to object scope`.

---

## Task 8: Verify + FE client regen + full gate

- [ ] **Step 1: Verify object-scope coverage.** Grep the BI routes: every `{id}`-bearing dashboard/chart/dataset handler now uses `require_object` (not `require(&ctx, ...)`); LIST/CREATE handlers still use `require(&ctx, ...)`. Report any remaining per-object `require(&ctx, ...)`.
- [ ] **Step 2: Regenerate the FE api client** for the new share endpoints/DTOs: `cargo run -p egret-bin -- openapi > openapi.json`, then `cd frontend && npm run gen:api` (orval). Commit the regenerated client + openapi.json. (No FE UI in this increment — B2-1b — but the client + `committed_openapi_json_is_current` test must be current.)
- [ ] **Step 3: Full local CI gate (sequential):**
  - `rtk proxy cargo fmt --all --check`
  - `rtk proxy cargo clippy --workspace --all-targets -- -D warnings`
  - `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test --workspace` → 0 failures; confirm `pdp_regression` 60/60 in the run.
  - `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` → green.
- [ ] **Step 4: Commit** any fmt/openapi/client changes: `feat(authz): B2-1a object-scope — verify + regen FE client + gate green`.

---

## Self-Review Notes

- **Spec coverage:** §2 operator fix → Task 1; §3 entity hierarchy + §4 Resource::Object/require_object → Task 2; §5 grants_for → Task 3; §6 share API → Task 4; §7 handler migration → Tasks 5–7; §8 tests distributed + Task 8.
- **Behavior preservation:** the operator change keeps `pdp_regression` 60/60 (Task 1 Step 3, re-verified Task 8); workspace members keep full access via the cascading `in`; object grants only ADD access for non-members.
- **Security:** platform roles keep `==` (no platform→object leak); folder/item grants require `<kind>:manage` to create (Metabase curate semantics); object-grant deletes are scoped by the object id.
- **No placeholders:** exact signatures (`grants_for` 3-arg, `ObjectRef`, `require_object`), exact SQL fragments, and the per-resource handler lists are grep-driven in-task.

---

## Next: B2-1b (separate plan)

The sharing **UI** — a Metabase-style permissions/sharing modal on dashboards/charts/datasets/folders (add people/groups with Viewer/Editor, show inherited-vs-direct), driven by the Task 4 share API. Then B2-2 (object-scope for notebooks/saved-queries/warehouses), B2-3 (role hierarchy + explicit deny / allow-exceptions), and the deferred data-model flip.
