# The Catalogs UI — SP3c-1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `registry.catalogs` becomes something an operator can see and drive. Control Plane gains an Infrastructure → **Catalogs** section: a list that names every catalog's kind, engines, workspaces and failed applies; a create flow with a typed form per kind family and an Advanced block per engine; a detail page with Configuration, Engines (attach, detach, apply status, Retry, Test) and Workspaces, plus — when the catalog is an egret warehouse — the roles, clients, browse, connect and settings tabs that live on `WarehouseDetail.tsx` today, reused rather than rewritten. `/control-plane/warehouses/*` redirects onto the matching catalog and every literal link moves with it. Engine detail's Catalogs tab drops its inline `CatalogForm` for a registry picker, and the create-engine wizard attaches catalogs. One backend addition only: a registry-tagged detach route, so the generated client has a detach hook at all. **Nothing is removed** — that is SP3c-2.

**Architecture:** One backend change, in `crates/egret-engine-mgmt/src/catalogs.rs`: `DELETE /api/control-plane/catalogs/{id}/engines/{engine_id}` under the `control-plane-catalog-registry` tag, gated `catalog_registry:update`, calling the existing `catalog_apply::detach_from_engine` — the same function the legacy `DELETE /engines/{id}/catalogs/{cid}` calls, which stays exactly where it is (D5). Everything else is `frontend/src`. A new `routes/control-plane/catalogs/` directory holds the list, the create page, the detail shell and its three registry tabs, and a `kinds/` module with one typed form per kind family built from `ConfigSection` / `FieldLabel` / `SecretPicker` / the storage `<Select>`. A new `routes/control-plane/warehouse-tabs/` directory holds the five tab components lifted verbatim out of `WarehouseDetail.tsx` — they keep their `warehouseId: string` prop, so the catalog detail feeds them `catalog.warehouseId` and their bodies do not change. `WarehouseDetail.tsx` and `Warehouses.tsx` are then reduced to nothing and deleted, their routes replaced by two redirect components. `EngineDetail`'s `CatalogsCard` and `EngineCreate`'s warehouse `<Select>` are rewritten against the registry client.

**Tech Stack:** Rust (axum 0.8, utoipa 5, sqlx 0.9 / Postgres), orval via `npm run gen:api`; React 18, TanStack Query v5, react-router 6, shadcn/Radix + Tailwind v4 (OKLCH tokens), Vitest + Testing Library, Playwright.

**Spec:** `docs/en/specs/2026-09-22-catalog-ui-and-legacy-removal-sp3c-design.md` — **D1–D5 govern, and §3 is the scope.** §4 (the removal) is SP3c-2 and is not in this plan. Over `docs/en/specs/2026-09-19-catalog-registry-design.md` §5, which §3 corrects in the places the table below records.

## Global Constraints

- Branch: `feat/catalog-ui-sp3c-1`, cut from `origin/main` at or after `307cc006`. The plan document itself lands on `docs/sp3c-catalog-ui`; **do not implement on that branch.** Never commit on `main`; run `git branch --show-current` before every commit and stop if it does not print `feat/catalog-ui-sp3c-1`. Never `git stash`.
- **D5 — nothing is removed.** Every legacy endpoint stays: `POST /api/control-plane/warehouses`, `DELETE /api/control-plane/warehouses/{id}`, the whole `warehouse_admin` surface, `POST|PUT|DELETE /engines/{id}/catalogs*`, `engines.warehouse_id`, `warehouse:*`. This plan deletes exactly four **frontend** files (`Warehouses.tsx`, `WarehouseDetail.tsx`, `CatalogForm.tsx` and `Warehouses.test.tsx`) and no backend line. A user who upgrades to 3c-1 alone has a working system with two ways to reach catalogs.
- **D3's lock-out rule is a per-task invariant, not a final state.** `catalog_registry:read` enters `CONTROL_PLANE_PERMS` in **Task 2**, the same task that adds the section, the route guard and the sidebar entry. `warehouse:read` stays in that list for the whole of SP3c-1. At no commit in this branch is a user admitted to a section they cannot see or excluded from one they could see before.
- `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target` in every shell that runs cargo.
- DB tests: `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` and run cargo test under `env -u EGRET_ALLOW_DB_SKIP`. **Never** the dev database `egret`. A skipped DB test prints `ok`: read every `finished in` — a DB test binary that finishes in well under 0.3 s did no database work.
- SP3c-1 adds **no migration**. If a step seems to need one, stop: it belongs in SP3c-2.
- OpenAPI: regenerate with `rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json` (without `rtk proxy` the RTK hook filters stdout and corrupts the file); drift gate `cargo test -p egret-server --lib committed_openapi_json_is_current`.
- Frontend client: `cd frontend && npm run gen:api` — **never bare `orval`**, which drops `encodeURIComponent` from path parameters. CI's frontend job runs `npm run gen:api` and **fails on any drift**, so commit exactly what it produced (`git add -A frontend/src/api/generated`), including churn this branch did not cause. **Schema-property doc comments reach the generated client as JSDoc; response descriptions do not** — anything a UI author must read has to live on the DTO field, not in a `(status = …, description = …)`.
- Frontend gate per task: `cd frontend && npm run lint && npm run typecheck && npx vitest run <the files this task touched>`. The full `npm test` and `npm run build` run exactly once, in Task 9.
- Generated query hooks return a discriminated union, not a bare body. Read them as `q.data?.status === 200 ? q.data.data : <empty>` — the pattern `WarehouseDetail.tsx:1140-1142` already uses. Never `q.data.data` unguarded.
- **Two error types, two field names.** A generated hook's `TError` is `ErrorBody`, whose message is **`e.error`** (`model/errorBody.ts`). A hand-written `@/api/controlPlane` call rejects with `ApiError`, whose message is **`e.message`**. Both are objects with a string field, so writing `e.message` on an `ErrorBody` type-errors but writing `e.error` on neither does — read which client the call came from before reaching for the field.
- Never add a hand-written fetch wrapper for a registry route. Components import the generated hooks directly (`useControlPlaneCatalogRegistry*`), the way `WarehouseDetail.tsx` imports `useControlPlaneEnginesListEngines`. The only hand-written calls this plan uses are the **legacy warehouse** ones that already exist in `frontend/src/api/controlPlane.ts` (`listWarehouses`, `createWarehouse`, `deleteWarehouse`).
- Design tokens are OKLCH (Tailwind v4). Inline styles use `var(--token)`, never `hsl(var(--token))`. Reuse `ConfigSection`, `FieldLabel`, `SecretPicker`, `FilterableTable`, `SummaryStrip`, `EmptyState`, `ListSkeleton`, `ConfirmDeleteDialog` — this plan introduces no new primitive.
- RTK rewrites shell commands. Any command whose raw stdout is piped or redirected runs as `rtk proxy <cmd>`; `rtk grep` swallows piped stdin — never verify a diff with it.
- Commits: Conventional Commits; body ends with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Never override the author.

## Design statements checked against the tree, and the decisions they forced

Read on 2026-09-22 against `main` at `307cc006`. Where the design and the tree disagree, the row says which one this plan follows.

| # | Statement | Verdict | Decision in this plan |
|---|---|---|---|
| 1 | §3 "Backend, only what is missing: **a detach endpoint** for the registry (the generated client has no detach hook today; removal still goes through the legacy engine route)" | **Half true.** The *handler* exists: `catalog_apply::detach_from_engine` (`catalog_apply.rs:1190`), reached by `DELETE /api/control-plane/engines/{id}/catalogs/{cid}` → `engines.rs::remove_catalog` (`:5757`), gated `engine:delete`, tagged `control-plane-engines`. What is missing is a **registry-tagged, `catalog_registry:update`-gated** route, so orval emits a hook into `control-plane-catalog-registry`. Re-tagging `remove_catalog` would rename the legacy generated function and break `deleteEngineCatalog` — a removal, which D5 forbids here. Axum cannot mount a second `DELETE` on the same path. | The new route is **`DELETE /api/control-plane/catalogs/{id}/engines/{engine_id}`**, mounted in `catalogs.rs::routes()`, symmetric with `/catalogs/{id}/workspaces`, delegating to the same `detach_from_engine`. The legacy route is untouched and SP3c-2 deletes it. Task 1. |
| 2 | D4 "Without [an attached engine], a **TCP reachability check** of the configured host and port" | **No such endpoint exists anywhere.** `rtk grep -rn 'TcpStream\|tcp_probe\|reachab' crates/` returns only prose. The only test route is `POST /engines/{id}/catalogs/{cid}/test` (`engines.rs:5655`), which is **Trino-only**: it goes through `crate::monitor::coordinator_call`, whose fallback is the `{name}-trino` Service. Adding a TCP prober would be a second backend piece, which §3 says SP3c-1 does not have. | The Test control is **per attachment**, on the Engines tab, enabled only for a `trino` attachment. Its button reads `Test via <engine>` and its result reads **`SHOW SCHEMAS FROM <name> succeeded on <engine>`** — the statement it ran, so a green tick cannot be read as "the credentials work" for any other engine. A catalog with no Trino attachment shows the button **disabled** with the sentence "Testing needs a Trino engine attached — egret has no way to reach this catalog on its own yet." The TCP half and StarRocks testing are named as SP5's, per D4's own last line. Task 5. |
| 3 | D1 "an iceberg-egret catalog additionally shows **today's six** warehouse tabs, reusing those components" | **Five, not six.** The six are `RolesTab`, `ClientsTab`, `EnginesTab`, `BrowseTab`, `ConnectTab`, `SettingsTab` (`WarehouseDetail.tsx:195-218`). The warehouse `EnginesTab` (`:1139`) lists `engines.filter(e => e.warehouseId === warehouseId)` — engines whose **primary** catalog this is. The catalog detail's own Engines tab lists `catalog.engines`, the registry attachments, which since SP3a **include** each engine's primary attachment. Showing both would put two tabs named "Engines" on one page, the second a strict subset of the first. | Five tabs move (`RolesTab`, `ClientsTab`, `BrowseTab`, `ConnectTab`, `SettingsTab`); the warehouse `EnginesTab` is **deleted**, superseded by the registry Engines tab, and Task 6 Step 1 records that its rows are a subset of the rows that replace them. D2's `is_primary` (SP3c-2) is what makes "primary" visible again as a badge. |
| 4 | §3 "**Three places** link to that path by literal string — `WorkspaceDetail.tsx`, `StorageDetail.tsx`, `Sidebar.tsx`" | **Sidebar has no literal.** `Sidebar.tsx:107` is `{ label: "Warehouses", icon: Boxes, perm: "warehouse:read" }`; the path comes from `slug(label)` at `:171`. The literal `/control-plane/warehouses` route links outside the warehouse pages are exactly **three**: `WorkspaceDetail.tsx:377` (`?workspace=`), `WorkspaceDetail.tsx:403`, `StorageDetail.tsx:543`. Two more live inside the pages being deleted (`Warehouses.tsx:67,185`, `WarehouseDetail.tsx:162`). Two **tests** assert the string: `Sidebar.test.tsx:226`, `WorkspaceDetail.test.tsx:206,212`. `api/catalogTags.ts:42,53,70` uses `/control-plane/warehouses/{id}/resource-tags` — an **API** path, not a route; D3 re-gates it in SP3c-2 and this plan does not touch it. | Task 6 changes the three route links and the two tests, and states the API path is deliberately left alone. |
| 5 | Research note: "`useUnsavedChangesWarning.ts` and a governance redirect test mention the string" | **Neither does.** The file is `frontend/src/lib/useUnsavedChangesWarning.ts` (not `src/hooks/`) and `rtk grep -n warehouse` on it exits 1. No test under `src/routes/governance` matches `warehouses`. | The grep in Task 6 Step 4 is re-run as a gate, so the plan is falsified rather than trusted if a site is missed. The real extra site the note omitted is `src/auth/__tests__/appAccess.test.ts:24`, handled in Task 2. |
| 6 | §3 "**Control Plane → Infrastructure → Catalogs**" replacing Warehouses, with `/control-plane/warehouses/*` redirecting | **Deleting `Warehouses.tsx` deletes the only way to CREATE a warehouse.** `AddWarehouseDialog` (`Warehouses.tsx:245`) posts `createWarehouse`, which provisions the lakekeeper warehouse. The registry's own create **refuses** egret catalogs: `create_registry_catalog`'s 400 reads "…or an egret warehouse catalog (those are created with their warehouse)". `delete_registry_catalog` refuses them too, with `code = "warehouse_catalog"`. | The Catalogs **create** page offers `egret_warehouse` as its first kind and posts it to the legacy `createWarehouse(...)`, carrying `AddWarehouseDialog`'s workspace + storage fields verbatim (Task 3 Step 4). The Catalogs **list** delete calls `deleteWarehouse(c.warehouseId)` when `warehouseId != null` and `useControlPlaneCatalogRegistryDeleteRegistryCatalog` otherwise (Task 2 Step 5). Neither is a workaround: they are the endpoints D5 keeps. |
| 7 | §3 "**Create-engine wizard** attaches catalogs instead of choosing one warehouse" | **`warehouseId` is still mandatory.** `CreateEngineBody.warehouse_id: Uuid` (`engines.rs:152`) is not `Option`. D2's `is_primary` migration is SP3c-2, so until then an engine without a warehouse cannot be created at all. `EngineCreate.tsx:119-123` makes `warehouseId.length > 0` a create precondition and `:129-146` sends it. | The wizard's field becomes **"Primary catalog"**, a picker over registry catalogs with `warehouseId != null`, and the body sends **`warehouseId: picked.warehouseId`**. Beneath it, a **multi-select of additional catalogs** attached after the create returns, one `attachCatalog` call each, with per-catalog failures surfaced and the engine kept. The screen says what it does; the body is unchanged; SP3c-2's `is_primary` is what lets the first field become optional. Task 8. |
| 8 | §5 "the existing warehouse tabs move here (namespace browser, catalog roles, grants, clients, **tags**)" | Tags are not a warehouse tab. `CatalogTagDialog` from `components/access/CatalogTagEditor.tsx` is invoked from **inside** `BrowseTab`'s table detail, and its 168-line test mocks `api/catalogTags.ts` directly, not the page. | Tags move with `BrowseTab` because they are inside it. `CatalogTagEditor.tsx` and `CatalogTagEditor.test.tsx` are **not touched** — Task 6 Step 5 asserts that file is byte-identical at the end of the task. |
| 9 | §3 "Create: choose a kind, fill the typed form… Built from the existing config primitives (`ConfigSection`, `FieldLabel`, the secret picker, the storage-connection picker)" | Three of four exist as named; there is **no storage-connection picker component**. `WarehouseDetail`/`Warehouses`/`EngineCreate` each inline a shadcn `<Select>` over `listStorage()`. `SecretPicker` (`components/SecretPicker.tsx:41`) takes a **workspace id** (`ws: string`) and offers inline creation — registry secrets are platform-scoped. | The typed forms use `ConfigSection` (`level={2}`, since the create page puts them under its `h1`), `FieldLabel`, a local `<StorageSelect>` and `<SecretSelect>` defined once in `catalogs/kinds/pickers.tsx` over `listStorage()` / `listSecrets()`, both plain controlled `<Select>`s returning an id. `SecretPicker` is **not** reused: its workspace scope and inline-create path are wrong for a platform catalog, and bending it would change a component six other screens depend on. Task 3 Step 2. |
| 10 | §3 "List: name, kind, attached engines, assigned workspaces, and a warning when any attachment is `failed` or stuck `pending`" | `RegistryCatalogDto` carries all four directly (`engines: CatalogAttachmentDto[]`, `workspaceIds: string[]`), so no per-row fetch is needed. `applyStatus` is `applied \| failed \| pending`, documented on the DTO field, so the JSDoc reaches the client. **"Stuck" is not a value** — `pending` with a recent `attachment` is normal for 5–10 minutes after a StarRocks create (SP3b D1). | The warning badge is `failed` **or** `pending` **with no `appliedAt` and a catalog `updatedAt` older than 15 minutes**; the tooltip distinguishes the two. `workspaceIds` are resolved to names through `useMyWorkspaces(true)`, the same hook `Warehouses.tsx:127` uses. Task 2 Step 3. |
| 11 | §5 "Workspace detail (Control Plane): assigned catalogs" | `WorkspaceDetail.tsx:365-410` renders a Warehouses card filtered by `w.workspaceId === workspaceId` — warehouse ownership, which is **not** the registry's workspace *assignment* (`registry.catalog_workspaces`, many-to-many). They answer different questions and both are true. | §5's bullet is **out of scope for SP3c-1** and this plan says so rather than half-doing it: Task 6 only re-points that card's two links at `/control-plane/catalogs`. Replacing ownership with assignment is a behaviour change that belongs with D2/SP4. |
| 12 | §5 "`provisioning-ui-kind.spec.ts` is already stale" | **True, and worse than stated.** It expects `getByRole("dialog")` with the text "Add Trino engine" and fields `#cpe-name` / `#cpe-warehouse` / `#cpe-workers` / `#cpe-memory` (`:48-58`). The code navigates to a full page (`/control-plane/engines/new`, `EngineCreate.tsx`) whose fields are `#ec-name` / `#ec-warehouse` / … and whose button is "Create engine", not "Add engine". It is `EGRET_E2E_KIND`-opt-in, so it has been failing invisibly. | Task 9 rewrites it against the page the code actually renders **and** against Task 8's Primary-catalog field, so it can never again pass by not running. It also stops seeding a warehouse purely through the API: the rewritten spec creates the catalog through the Catalogs UI, which is the thing SP3c-1 adds. |

## File Structure

**`crates/egret-engine-mgmt`** — one route.

| File | Change |
|---|---|
| `src/catalogs.rs` | `detach_catalog` handler + its `#[utoipa::path]`; one `.route(…)` line in `routes()`; `detach_catalog` added to the `CatalogRegistryApi` `paths(…)` list |

**`frontend/src/routes/control-plane/catalogs/`** — new.

| File | Contents |
|---|---|
| `Catalogs.tsx` | the list page (default export `Catalogs`), `catalogsQueryKey`, `isEgretWarehouse`, `attachmentWarning` |
| `CatalogCreate.tsx` | the full-page create wizard (default export `CatalogCreate`) |
| `CatalogDetail.tsx` | the detail shell (default export `CatalogDetail`), kind-conditional `<TabsList>` (D1) |
| `ConfigurationTab.tsx` | read + edit of `config` / `advanced` / `description`, with the impact preview |
| `EnginesTab.tsx` | attach, detach, apply status, Retry, Test (D4) |
| `WorkspacesTab.tsx` | assignment, over `listCatalogWorkspaces` / `setCatalogWorkspaces` |
| `kinds/pickers.tsx` | `StorageSelect`, `SecretSelect`, `KIND_OPTIONS`, `kindLabel` |
| `kinds/forms.tsx` | `IcebergForm`, `HiveFamilyForm`, `JdbcForm`, `ElasticsearchForm`, `SampleForm`, `GenericForm`, `EgretWarehouseForm`, and `emptyConfigFor(kind)` |
| `kinds/AdvancedEditor.tsx` | the per-engine free-form `advanced` editor |
| `__tests__/Catalogs.test.tsx`, `__tests__/CatalogCreate.test.tsx`, `__tests__/CatalogDetail.test.tsx`, `__tests__/EnginesTab.test.tsx` | |

**`frontend/src/routes/control-plane/warehouse-tabs/`** — new, lifted out of `WarehouseDetail.tsx`.

| File | What moves in |
|---|---|
| `shared.tsx` | `CopyButton` (`:251`), `consumerDot` (`:244`), `rolesKey` (`:123`), `clientsKey` (`:126`) |
| `RolesTab.tsx` | `RolesTab` (`:390`), `consumerHint` (`:481`), `RoleRow` (`:488`), `AddRoleDialog` (`:607`), `AddGrantDialog` (`:713`), `grantsKey` (`:129`), `CONSUMER_TYPES` (`:121`) |
| `ClientsTab.tsx` | `ClientsTab` (`:864`), `AddClientDialog` (`:994`) |
| `BrowseTab.tsx` | `BrowseTab` (`:1205`), `ProtectToggle` (`:1364`), `NsNode` (`:1410`), `TableDetailView` (`:1493`), `DeletedTablesDialog` (`:1782`), `Fact2` (`:1873`), `BrowseEmpty` (`:1882`) |
| `ConnectTab.tsx` | `ConnectTab` (`:2188`), `Coord`, `CodeBlock`, `CredField` (the file tail) |
| `SettingsTab.tsx` | `SettingsTab` (`:1891`), `WarehouseTasksCard` (`:2131`) |
| `__tests__/warehouseTabs.test.tsx` | `WarehouseDetail.test.tsx`'s two cases, re-pointed at the tab components |

**Deleted (frontend only):** `routes/control-plane/Warehouses.tsx`, `routes/control-plane/WarehouseDetail.tsx`, `routes/control-plane/CatalogForm.tsx`, `routes/control-plane/__tests__/Warehouses.test.tsx`, `routes/control-plane/__tests__/WarehouseDetail.test.tsx`. `WarehouseSummary` and the warehouse `EnginesTab` die with their file (table rows 3 and 6 say why each is not a loss).

**Modified:** `frontend/src/App.tsx`, `frontend/src/auth/appAccess.ts`, `frontend/src/components/layout/Sidebar.tsx`, `frontend/src/routes/control-plane/EngineDetail.tsx`, `frontend/src/routes/control-plane/EngineCreate.tsx`, `frontend/src/routes/control-plane/WorkspaceDetail.tsx`, `frontend/src/routes/control-plane/StorageDetail.tsx`, and the tests `auth/__tests__/appAccess.test.ts`, `components/layout/__tests__/Sidebar.test.tsx`, `routes/control-plane/__tests__/WorkspaceDetail.test.tsx`, `routes/control-plane/__tests__/EngineDetail.test.tsx`, `routes/control-plane/__tests__/EngineCreate.test.tsx`, `frontend/tests/e2e/provisioning-ui-kind.spec.ts`.

**Generated:** `openapi.json`, `frontend/src/api/generated/**`.

---
### Task 1: The detach endpoint on the registry

The only backend change in SP3c-1, and it must land first: Task 5 cannot call a hook that does not exist.

**Files:**
- Modify: `crates/egret-engine-mgmt/src/catalogs.rs`
- Regenerate: `openapi.json`, `frontend/src/api/generated/**`

**Interfaces:**
- Consumes: `crate::catalog_apply::{detach_from_engine, CatalogOpError}`, `engine_or_404` (`catalogs.rs:557`), `load` (`catalogs.rs:244`), `egret_core::authz::require`.
- Produces:
  - `DELETE /api/control-plane/catalogs/{id}/engines/{engine_id}` → `204 No Content`.
  - Generated client: `controlPlaneCatalogRegistryDetachCatalog(id: string, engineId: string)` and `useControlPlaneCatalogRegistryDetachCatalog({ mutation })` taking `{ id: string; engineId: string }`, in `frontend/src/api/generated/control-plane-catalog-registry/control-plane-catalog-registry.ts`.

- [ ] **Step 0: Branch**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git fetch origin
git switch -c feat/catalog-ui-sp3c-1 origin/main
git branch --show-current
```
Expected: `feat/catalog-ui-sp3c-1`.

- [ ] **Step 1: The route** — in `catalogs.rs::routes()`, after the `/catalogs/{id}/workspaces` line:

```rust
        .route(
            "/catalogs/{id}/engines/{engine_id}",
            delete(detach_catalog),
        )
```

and widen the import at the top of the file:

```rust
use axum::routing::{delete, get, post};
```

- [ ] **Step 2: The handler**, next to `attach_catalog`:

```rust
#[utoipa::path(
    delete,
    path = "/api/control-plane/catalogs/{id}/engines/{engine_id}",
    tag = "control-plane-catalog-registry",
    params(
        ("id" = Uuid, Path, description = "Registry catalog id"),
        ("engine_id" = Uuid, Path, description = "Engine id"),
    ),
    responses(
        (status = 204, description = "Removed from the engine, then detached. The registry catalog itself is kept, and the credential egret minted for this attachment is revoked"),
        (status = 400, description = "This is the engine's primary catalog (detach it by deleting the engine), or Kubernetes is not configured", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing catalog_registry:update permission", body = ErrorBody),
        (status = 404, description = "No such catalog or engine, or the catalog is not attached to it", body = ErrorBody),
        (status = 409, description = "The engine is being deleted; retry", body = ErrorBody),
        (status = 502, description = "The engine rejected the removal; nothing was detached", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn detach_catalog(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, engine_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "catalog_registry:update").await?;
    let engine = engine_or_404(&st, engine_id).await?;
    let catalog = load(&st, id).await?;
    detach_from_engine(&st, &engine, &catalog)
        .await
        .map_err(CatalogOpError::into_registry)?;
    Ok(StatusCode::NO_CONTENT)
}
```

Add `detach_from_engine` to the `use crate::catalog_apply::{…}` list at the top, and `detach_catalog` to the `paths(…)` list of `CatalogRegistryApi` (`catalogs.rs:792-806`), after `attach_catalog` (`:800`).

**This handler adds no policy.** Every refusal — the primary-catalog rule, the `deleting` conflict, the missing attachment — is already inside `detach_from_engine` (`catalog_apply.rs:1190-1214`), which the legacy route reaches too. A second refusal point here would be a second place to get it wrong.

- [ ] **Step 3: Route tests** — in `crates/egret-engine-mgmt/tests/routes.rs`, beside the existing attach coverage:

```rust
#[tokio::test]
async fn registry_detach_removes_the_attachment_and_keeps_the_catalog() { /* … */ }

#[tokio::test]
async fn registry_detach_refuses_the_primary_catalog_with_400() { /* … */ }

#[tokio::test]
async fn registry_detach_is_404_when_the_catalog_is_not_attached() { /* … */ }

#[tokio::test]
async fn registry_detach_requires_catalog_registry_update_not_engine_delete() { /* … */ }
```

The fourth is the one that justifies the new route existing at all: a principal holding `catalog_registry:update` and **not** `engine:delete` gets `204` here and `403` on `DELETE /engines/{id}/catalogs/{cid}`. Follow the existing attach tests for fixture construction and the `FakeEnginePort` wiring.

- [ ] **Step 4: Gate and commit**

```bash
export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo fmt --all -- --check
cargo clippy -p egret-engine-mgmt --all-targets -- -D warnings
env -u EGRET_ALLOW_DB_SKIP cargo test -p egret-engine-mgmt --test routes registry_detach
rtk proxy cargo run -q -p egret-bin -- openapi > openapi.json
cargo test -p egret-server --lib committed_openapi_json_is_current
cd frontend && npm run gen:api && git status --short src/api/generated && cd ..
rtk proxy grep -n "DetachCatalog" frontend/src/api/generated/control-plane-catalog-registry/control-plane-catalog-registry.ts | head
```
Expected: the four tests pass; `gen:api` produces `useControlPlaneCatalogRegistryDetachCatalog`; the generated URL builder is `` `/api/control-plane/catalogs/${encodeURIComponent(id)}/engines/${encodeURIComponent(engineId)}` `` — confirm `encodeURIComponent` is on **both** parameters before committing.

```bash
git add -A && git commit -m "feat(catalogs): detach a catalog from an engine through the registry route"
```

---
### Task 2: The Catalogs section — list, route, sidebar, permission

Everything a user needs to *reach* catalogs, in one commit, so `catalog_registry:read` never lags the section it gates.

**Files:**
- Create: `frontend/src/routes/control-plane/catalogs/Catalogs.tsx`, `frontend/src/routes/control-plane/catalogs/__tests__/Catalogs.test.tsx`
- Create as placeholders (Step 2), replaced in full by later tasks: `catalogs/CatalogCreate.tsx` (Task 3), `catalogs/CatalogDetail.tsx` (Task 4)
- Modify: `frontend/src/App.tsx`, `frontend/src/auth/appAccess.ts`, `frontend/src/components/layout/Sidebar.tsx`, `frontend/src/auth/__tests__/appAccess.test.ts`, `frontend/src/components/layout/__tests__/Sidebar.test.tsx`

**Interfaces:**
- Consumes: `useControlPlaneCatalogRegistryListRegistryCatalogs(params?, options?)` → `{ status: 200, data: RegistryCatalogDto[] }`; `useControlPlaneCatalogRegistryDeleteRegistryCatalog`; `deleteWarehouse` from `@/api/controlPlane`; `useMyWorkspaces(true)` from `@/api/hooks`; `FilterableTable`, `Column`, `SummaryStrip`, `EmptyState`, `ListSkeleton`, `ConfirmDeleteDialog`.
- Produces:
  - `export const catalogsQueryKey = ["control-plane", "catalogs"] as const;`
  - `export function isEgretWarehouse(c: RegistryCatalogDto): boolean` — `c.warehouseId != null`. The registry's own CHECK constraint (`0147_catalog_registry.sql:30-33`) makes `warehouse_id IS NOT NULL` exactly equivalent to `kind = 'iceberg' AND config->metastore->>type = 'egret'`, so this single field is the whole predicate; do not also sniff `config`.
  - `export type CatalogWarning = { level: "failed" | "stalled"; text: string } | null;`
  - `export function attachmentWarning(c: RegistryCatalogDto, now: number): CatalogWarning`
  - `export default function Catalogs()`

- [ ] **Step 1: The permission, first** — `frontend/src/auth/appAccess.ts`, in `CONTROL_PLANE_PERMS`, immediately after `"warehouse:read"`:

```ts
  "warehouse:read",
  // D3: `catalog_registry:read` gates the Catalogs section. It joins this list
  // in the SAME change that adds the section, and `warehouse:read` only LEAVES
  // it in SP3c-2, once `warehouse_admin` has been re-gated. Between the two
  // releases a user may hold either key and is locked out of nothing.
  "catalog_registry:read",
```

`frontend/src/auth/__tests__/appAccess.test.ts:24` lists the permissions a fixture principal holds; add `"catalog_registry:read"` there and a case that is the lock-out this design exists to avoid:

```ts
test("a user holding only catalog_registry:read may enter Control Plane", () => {
  const me = { platform_roles: [], permissions: ["catalog_registry:read"] } as unknown as Me;
  expect(canEnterControlPlane(me)).toBe(true);
});
```

- [ ] **Step 2: Route, guard and landing** — `frontend/src/App.tsx`.

Add the lazy imports beside the existing control-plane ones, then in `FIRST_SECTION_BY_PERM` (`:275-285`) put Catalogs **immediately after** `engines` and **before** `warehouses`, so a user holding both lands on the new section:

```ts
const FIRST_SECTION_BY_PERM: { path: string; perm: string }[] = [
  { path: "engines", perm: "engine:read" },
  { path: "catalogs", perm: "catalog_registry:read" },
  { path: "warehouses", perm: "warehouse:read" },
```

and in `ControlPlaneRoutes`, directly above the two `warehouses` routes (`:312-313`):

```tsx
          <Route path="catalogs" element={<RequirePermission perm="catalog_registry:read"><Catalogs /></RequirePermission>} />
          <Route path="catalogs/new" element={<RequirePermission perm="catalog_registry:create"><CatalogCreate /></RequirePermission>} />
          <Route path="catalogs/:id" element={<RequirePermission perm="catalog_registry:read"><CatalogDetail /></RequirePermission>} />
```

`CatalogCreate` and `CatalogDetail` land in Tasks 3 and 4. To keep this task's tree compiling, create both files now as a one-line placeholder that Task 3 / Task 4 replace **in full**:

```tsx
// Replaced in full by Task 3 (CatalogCreate) / Task 4 (CatalogDetail).
export default function CatalogCreate() { return null; }
```

The `warehouses` routes are **not** touched here — Task 6 turns them into redirects, and until then both sections work (D5).

- [ ] **Step 3: The list page** — `frontend/src/routes/control-plane/catalogs/Catalogs.tsx`, following `Warehouses.tsx`'s scaffold exactly (h1 + description + primary action, `SummaryStrip`, optional `?workspace=` banner, `FilterableTable`, `ConfirmDeleteDialog`).

The warning rule (table row 10) is the only non-obvious logic, so it is a pure exported function with its own tests:

```tsx
/** 15 minutes: longer than a StarRocks FE takes to come up (SP3b D1 says 5-10). */
const PENDING_GRACE_MS = 15 * 60 * 1000;

/**
 * The list's warning column. `failed` is unambiguous. `pending` is NORMAL for
 * the first minutes of a StarRocks engine's life — SP3b writes it deliberately
 * — so it is only a warning once it has outlived the bootstrap that clears it.
 * `appliedAt` is the proof the attachment ever converged, and `updatedAt` is
 * the last time egret changed the catalog, which is when a re-apply would have
 * restarted the clock.
 */
export function attachmentWarning(c: RegistryCatalogDto, now: number): CatalogWarning {
  const failed = c.engines.filter((e) => e.applyStatus === "failed");
  if (failed.length > 0) {
    return {
      level: "failed",
      text: `Failed to apply on ${failed.map((e) => e.engineName).join(", ")}`,
    };
  }
  const stalled = c.engines.filter(
    (e) =>
      e.applyStatus === "pending" &&
      !e.appliedAt &&
      now - new Date(c.updatedAt).getTime() > PENDING_GRACE_MS,
  );
  if (stalled.length > 0) {
    return {
      level: "stalled",
      text: `Still pending on ${stalled.map((e) => e.engineName).join(", ")} — egret has recorded the attachment but has not been able to tell the engine`,
    };
  }
  return null;
}
```

Columns: **Name** (a `<Link to={`/control-plane/catalogs/${c.id}`}>`), **Kind** (`kindLabel(c)` — `"egret warehouse"` when `isEgretWarehouse(c)`, otherwise the raw `kind`, as a `<Badge variant="secondary">`), **Engines** (`c.engines.length`, `value:` the joined engine names so search matches what the row shows — the same reason `warehouseColumns` resolves ids to names), **Workspaces** (`c.workspaceIds.length`, `value:` the joined workspace names), **Status** (the `attachmentWarning` badge, or `—`), **Actions** (Delete).

`SummaryStrip`: `catalogs`, `attached to an engine`, `needing attention`.

- [ ] **Step 4: The `?workspace=` filter** — `WorkspaceDetail.tsx:377` links `/control-plane/warehouses?workspace=<id>` today and Task 6 re-points it here, so the filter must exist before the link moves. Unlike the warehouse list, this is a **server-side** filter — the route already takes it:

```tsx
const [searchParams] = useSearchParams();
const wsFilter = searchParams.get("workspace");
const catalogsQ = useControlPlaneCatalogRegistryListRegistryCatalogs(
  wsFilter ? { workspaceId: wsFilter } : undefined,
  { query: { retry: false } },
);
const rows = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];
```

Keep `Warehouses.tsx:180-189`'s banner verbatim, with `/control-plane/catalogs` as the clear link.

- [ ] **Step 5: Delete, which is two endpoints** (table row 6):

```tsx
const deleteRegistry = useControlPlaneCatalogRegistryDeleteRegistryCatalog({
  mutation: { onSuccess: invalidate, onError: (e) => toast.error(e.error ?? "Failed to delete the catalog.") },
});
const deleteWh = useMutation<void, ApiError, string>({
  mutationFn: deleteWarehouse,
  onSuccess: invalidate,
  onError: (e) => toast.error(e.message ?? "Failed to delete the warehouse."),
});

function confirmDelete(c: RegistryCatalogDto) {
  // The registry refuses an egret warehouse catalog unconditionally (409
  // `warehouse_catalog`) — it is deleted with its warehouse, through the
  // endpoint D5 keeps. Sending it to the registry would show the user a
  // conflict they cannot act on.
  if (isEgretWarehouse(c)) deleteWh.mutate(c.warehouseId!);
  else deleteRegistry.mutate({ id: c.id });
}
```

The `ConfirmDeleteDialog` `description` differs per branch: the warehouse one keeps `Warehouses.tsx:236`'s sentence about the lakekeeper warehouse not being dropped; the registry one says which engines and workspaces still reference it, read off the row.

- [ ] **Step 6: The sidebar entry** — `Sidebar.tsx`, in the Infrastructure group, between Engines and Warehouses:

```ts
      { label: "Engines", icon: Server, perm: "engine:read" },
      { label: "Catalogs", icon: Library, perm: "catalog_registry:read" },
      { label: "Warehouses", icon: Boxes, perm: "warehouse:read" },
```

Add `Library` to the `lucide-react` import. The path is `slug("Catalogs")` = `catalogs`, so no `to` is needed — do not add one. Both entries render through SP3c-1; Task 6 removes the Warehouses entry once its route is a redirect.

`Sidebar.test.tsx:226` asserts the rendered hrefs; add `"/control-plane/catalogs"` to that array in the position the group renders it.

- [ ] **Step 7: Tests** — `catalogs/__tests__/Catalogs.test.tsx`, mocking the generated module the way `Warehouses.test.tsx` mocks `@/api/controlPlane`:

```ts
vi.mock("@/api/generated/control-plane-catalog-registry/control-plane-catalog-registry");
vi.mock("@/api/hooks");
```

Cases:
1. renders a catalog with its kind badge, engine count and workspace name;
2. `attachmentWarning` returns `failed` for a `failed` attachment, `null` for a `pending` one on a catalog updated a minute ago, and `stalled` for a `pending` one updated an hour ago (three `expect`s, no rendering — the rule is pure);
3. `?workspace=ws1` passes `{ workspaceId: "ws1" }` to the list hook and shows the banner whose clear link is `/control-plane/catalogs`;
4. the empty state offers "New catalog";
5. **delete routes by kind**: deleting an egret-warehouse row calls `deleteWarehouse("wh-1")` and never the registry delete; deleting a `jdbc` row calls the registry delete with `{ id: "c2" }` and never `deleteWarehouse`. Assert both directions — a test that only checks the call that happened passes just as well when both fire.

- [ ] **Step 8: Gate and commit**

```bash
cd frontend
npm run lint && npm run typecheck
npx vitest run src/routes/control-plane/catalogs src/auth/__tests__/appAccess.test.ts src/components/layout/__tests__/Sidebar.test.tsx
cd ..
git add -A && git commit -m "feat(catalogs): Control Plane Catalogs list, route, sidebar entry and catalog_registry:read"
```

---
### Task 3: The create flow

**Files:**
- Create: `frontend/src/routes/control-plane/catalogs/kinds/pickers.tsx`, `kinds/forms.tsx`, `kinds/AdvancedEditor.tsx`, `frontend/src/routes/control-plane/catalogs/__tests__/CatalogCreate.test.tsx`
- Replace in full: `frontend/src/routes/control-plane/catalogs/CatalogCreate.tsx` (Task 2's placeholder)

**Interfaces:**
- Consumes: `useControlPlaneCatalogRegistryCreateRegistryCatalog`; `createWarehouse`, `listStorage`, `listSecrets`, `type CreateWarehouseInput` from `@/api/controlPlane`; `useMyWorkspaces(true)`; `ConfigSection`, `FieldLabel`.
- Produces:
  - `kinds/pickers.tsx`: `export function StorageSelect({ id, value, onChange }: { id: string; value: string; onChange: (v: string) => void })`; `export function SecretSelect({ id, value, onChange, placeholder }: { id: string; value: string; onChange: (v: string) => void; placeholder?: string })`; `export const KIND_OPTIONS: { kind: string; label: string; blurb: string }[]`; `export function kindLabel(c: { kind: string; warehouseId?: string | null }): string`.
  - `kinds/forms.tsx`: `export type KindFormProps = { config: Record<string, unknown>; onChange: (next: Record<string, unknown>) => void };` plus one component per family and `export function emptyConfigFor(kind: string): Record<string, unknown>`.
  - `kinds/AdvancedEditor.tsx`: `export function AdvancedEditor({ value, onChange }: { value: { trino?: Record<string,string>; starrocks?: Record<string,string> }; onChange: (v: …) => void })`.
  - `CatalogCreate.tsx`: `export default function CatalogCreate()`.

**The `config` shapes are not guesses.** Each is the serde shape of `crates/egret-catalog-registry/src/config.rs`, which is `#[serde(deny_unknown_fields)]` throughout — an extra key is a 400, not a silently ignored field. The tagged enums use `type` (`IcebergMetastore`, `RestAuth`, `HiveMetastore`) and `mode` (`JdbcSsl`).

| Kind | `config` | Source |
|---|---|---|
| `egret_warehouse` (UI-only) | not a registry kind — posts to `createWarehouse` | table row 6 |
| `iceberg` | `{ metastore: { type: "rest", uri, warehouse, auth: { type: "none" } \| { type: "oauth2", credential_secret_id, scope?, token_uri? } } \| { type: "hive", uri } \| { type: "glue", region, access_key?, secret_key_secret_id? }, storage_connection_id? }` | `config.rs:86-127` |
| `hive` / `delta_lake` / `hudi` | `{ metastore: { type: "hive", uri } \| { type: "glue", region, access_key?, secret_key_secret_id? }, storage_connection_id }` — **required**, unlike iceberg's | `config.rs:130-152` |
| `jdbc` | `{ dialect: "postgresql"\|"mysql"\|"mariadb"\|"oracle"\|"sqlserver"\|"clickhouse", host, port: u16, database?, user, password_secret_id, ssl: { mode: "disable" } \| { mode: "require" } \| { mode: "verify_full", ca_secret_id? } }` | `config.rs:155-211` |
| `elasticsearch` | `{ hosts: string[], port: u16, tls: bool, user?, password_secret_id? }` | `config.rs:213-221` |
| `tpch` / `tpcds` | `{ scale?: u32 }` | `config.rs:223-229` |
| `generic` | `{ engines: ("trino"\|"starrocks")[], secret_properties: { trino: {k:uuid}, starrocks: {k:uuid} } }` | `config.rs:231-249` |

- [ ] **Step 1: The page shell** — `/control-plane/catalogs/new`, a full page (not a dialog), mirroring `EngineCreate.tsx`'s layout: a back link to `/control-plane/catalogs`, an `h1`, then sections. Three states in one page, no wizard steps:

```tsx
const [kind, setKind] = useState("");            // "" until chosen
const [name, setName] = useState("");
const [description, setDescription] = useState("");
const [config, setConfig] = useState<Record<string, unknown>>({});
const [advanced, setAdvanced] = useState<{ trino?: Record<string,string>; starrocks?: Record<string,string> }>({});
```

Choosing a kind resets `config` to `emptyConfigFor(kind)` — never merges into the previous kind's shape, which `deny_unknown_fields` would reject.

The name field carries the backend's own rule as its help text and validates it client-side so the user is not round-tripped for a typo:

```tsx
const NAME_RE = /^[a-z][a-z0-9_]{0,62}$/;
// `CreateRegistryCatalogBody.name`: "SQL name on every engine:
// ^[a-z][a-z0-9_]{0,62}$, immutable." The name is the SQL identifier on every
// engine the catalog is ever attached to, which is why it cannot be edited later.
```

- [ ] **Step 2: The pickers** — `kinds/pickers.tsx`. Table row 9 says why these are local rather than `SecretPicker`:

```tsx
/**
 * A platform storage-connection / secret chooser.
 *
 * NOT `components/SecretPicker.tsx`: that one is workspace-scoped (`ws: string`)
 * and offers inline secret creation, both wrong for a platform catalog, and
 * six other screens depend on its current shape. These are plain controlled
 * `<Select>`s over the platform lists, the same pattern `Warehouses.tsx`,
 * `EngineCreate.tsx` and `WarehouseDetail.tsx` each already inline.
 */
export function StorageSelect({ id, value, onChange }: { id: string; value: string; onChange: (v: string) => void }) {
  const q = useQuery({ queryKey: ["control-plane", "storage"], queryFn: listStorage, retry: false });
  const items = q.data ?? [];
  return (
    <Select value={value} onValueChange={onChange}>
      <SelectTrigger id={id} className="w-full">
        <SelectValue placeholder="Select a storage connection" />
      </SelectTrigger>
      <SelectContent>
        {items.length === 0 ? (
          <SelectItem value="__none__" disabled>No storage connections</SelectItem>
        ) : (
          items.map((s) => <SelectItem key={s.id} value={s.id}>{s.name}</SelectItem>)
        )}
      </SelectContent>
    </Select>
  );
}
```

`SecretSelect` is the same over `listSecrets`. Both use `__none__` as the disabled-placeholder value because shadcn `Select` cannot take `""` as a `SelectItem` value — the constraint `CatalogForm.tsx:33-36` documents.

`KIND_OPTIONS` leads with the egret warehouse and carries one honest sentence each:

```tsx
export const KIND_OPTIONS = [
  { kind: "egret_warehouse", label: "egret warehouse", blurb: "An Iceberg warehouse egret manages itself, over a storage connection. Creates the warehouse as well as the catalog." },
  { kind: "iceberg", label: "Iceberg", blurb: "An existing Iceberg catalog — REST, Hive Metastore or AWS Glue." },
  { kind: "hive", label: "Hive", blurb: "Hive tables through a Hive Metastore or Glue." },
  { kind: "delta_lake", label: "Delta Lake", blurb: "Delta tables through a Hive Metastore or Glue." },
  { kind: "hudi", label: "Hudi", blurb: "Hudi tables through a Hive Metastore or Glue." },
  { kind: "jdbc", label: "JDBC", blurb: "A relational database: PostgreSQL, MySQL, MariaDB, Oracle, SQL Server or ClickHouse." },
  { kind: "elasticsearch", label: "Elasticsearch", blurb: "Elasticsearch indices as tables." },
  { kind: "tpch", label: "TPC-H", blurb: "Generated benchmark data. No external system." },
  { kind: "tpcds", label: "TPC-DS", blurb: "Generated benchmark data. No external system." },
  { kind: "generic", label: "Generic", blurb: "Engine properties written by hand, for a connector egret has no typed form for." },
];
```

- [ ] **Step 3: The typed forms** — `kinds/forms.tsx`. One `ConfigSection level={2}` per form, `FieldLabel` on every field, and the tagged-union discriminator as its own `<Select>` that **replaces** the dependent fields rather than hiding them, so no stale key survives into the body:

```tsx
export function JdbcForm({ config, onChange }: KindFormProps) {
  const c = config as {
    dialect?: string; host?: string; port?: number; database?: string;
    user?: string; password_secret_id?: string; ssl?: { mode?: string; ca_secret_id?: string };
  };
  const set = (patch: Partial<typeof c>) => onChange({ ...c, ...patch });
  return (
    <ConfigSection
      level={2}
      title="Database"
      description="egret connects with these credentials from every engine this catalog is attached to."
    >
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="space-y-1.5">
          <FieldLabel htmlFor="cc-jdbc-dialect" label="Dialect" help="Selects the driver and the URL vocabulary. StarRocks treats mysql and mariadb identically." />
          <Select value={c.dialect ?? ""} onValueChange={(v) => set({ dialect: v })}>
            <SelectTrigger id="cc-jdbc-dialect" className="w-full"><SelectValue placeholder="Select a dialect" /></SelectTrigger>
            <SelectContent>
              {["postgresql", "mysql", "mariadb", "oracle", "sqlserver", "clickhouse"].map((d) => (
                <SelectItem key={d} value={d}>{d}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        {/* host, port (number), database (optional), user */}
        <div className="space-y-1.5">
          <FieldLabel htmlFor="cc-jdbc-password" label="Password secret" help="A platform secret id. The catalog never stores a plaintext password." />
          <SecretSelect id="cc-jdbc-password" value={c.password_secret_id ?? ""} onChange={(v) => set({ password_secret_id: v })} />
        </div>
      </div>
      {/* ssl.mode Select; picking verify_full REPLACES ssl wholesale:
          onValueChange={(mode) => set({ ssl: mode === "verify_full" ? { mode } : { mode } })}
          — never spread the old ssl object, or `ca_secret_id` survives a switch
          to `disable` and `deny_unknown_fields` rejects the body. */}
    </ConfigSection>
  );
}
```

`IcebergForm` and `HiveFamilyForm` follow the same rule at their `metastore.type` and `auth.type` switches. `HiveFamilyForm` takes a `storageRequired: boolean` prop (`true` for hive/delta_lake/hudi, where `storage_connection_id` is not `Option`) and blocks submit without it. `SampleForm` is a single optional number. `GenericForm` is two engine checkboxes plus a key→secret-id list per engine.

`emptyConfigFor` returns the minimal skeleton per kind, one arm each, with no default arm (an unknown kind is a bug, not an empty object):

```ts
export function emptyConfigFor(kind: string): Record<string, unknown> {
  switch (kind) {
    case "iceberg":       return { metastore: { type: "rest", uri: "", warehouse: "", auth: { type: "none" } } };
    case "hive":
    case "delta_lake":
    case "hudi":          return { metastore: { type: "hive", uri: "" }, storage_connection_id: "" };
    case "jdbc":          return { dialect: "", host: "", port: 5432, user: "", password_secret_id: "", ssl: { mode: "require" } };
    case "elasticsearch": return { hosts: [], port: 9200, tls: true };
    case "tpch":
    case "tpcds":         return {};
    case "generic":       return { engines: [], secret_properties: { trino: {}, starrocks: {} } };
    default:              throw new Error(`no config skeleton for kind ${kind}`);
  }
}
```

Strip empty-string optionals at submit time; **never send `""` for a `*_secret_id`** or for a required `storage_connection_id`, which is not a UUID and returns a 400 the user cannot read.

- [ ] **Step 4: The egret-warehouse branch** (table row 6) — `EgretWarehouseForm` is not a registry form at all. It renders `AddWarehouseDialog`'s two fields (`Warehouses.tsx:244`): a workspace `<Select>` over `useMyWorkspaces(true)` and a `<StorageSelect>`. The page's submit branches once:

```tsx
async function handleCreate() {
  if (kind === "egret_warehouse") {
    // The registry refuses these (`create_registry_catalog`'s 400: "…or an
    // egret warehouse catalog (those are created with their warehouse)").
    // Creating the warehouse is what creates the registry catalog: SP3a's
    // warehouse-create path writes the `registry.catalogs` row with
    // `warehouse_id` set. D5 keeps this endpoint for exactly this reason.
    // `CreateWarehouseInput` is `{ name, workspaceId, storageConnectionId }`
    // (controlPlane.ts:219-223). Note the field is `storageConnectionId` on the
    // way IN and `storageConnId` on the `Warehouse` that comes back — they are
    // not the same spelling, and TypeScript will not catch the swap because
    // both are `string`.
    const wh = await createWarehouse({ name: name.trim(), workspaceId, storageConnectionId });
    navigate(`/control-plane/catalogs?created=${encodeURIComponent(wh.name)}`);
    return;
  }
  const created = await createMut.mutateAsync({
    data: { name: name.trim(), kind, config, advanced, ...(description.trim() ? { description: description.trim() } : {}) },
  });
  if (created.status === 201) navigate(`/control-plane/catalogs/${created.data.id}`);
}
```

- [ ] **Step 5: Advanced** — `kinds/AdvancedEditor.tsx`, a collapsible section below the typed form with one key/value list per engine (`trino`, `starrocks`), rendered only when a kind is chosen and never for `egret_warehouse`. Values are strings; `Advanced` is `BTreeMap<String,String>` per engine (`config.rs:252-258`). Carry the two refusals into the help text, because they are validation errors the user will otherwise meet as opaque 400s:

```tsx
<p className="text-xs text-muted-foreground">
  Written after the properties the form above sets. A key the form already sets
  is refused, and a value that looks like a credential is refused — store it as a
  secret and reference its id.
</p>
```

(`ValidationError::Collision` and `ValidationError::PlaintextSecret`, `config.rs:292-302`.)

- [ ] **Step 6: Tests** — `__tests__/CatalogCreate.test.tsx`:
1. choosing `jdbc` renders the dialect/host/port/user/password fields and **not** the iceberg metastore field;
2. switching kind from `jdbc` to `tpch` sends `config: {}` — assert the mutation payload, because a merged leftover key is exactly what `deny_unknown_fields` rejects and exactly what a re-render still looks fine with;
3. switching `ssl.mode` from `verify_full` to `disable` leaves no `ca_secret_id` in the payload;
4. a name of `1bad` blocks submit and shows the rule; `good_name` does not;
5. kind `egret_warehouse` calls `createWarehouse` and **never** the registry create mutation — and the converse for `jdbc` (assert both, per Task 2 Step 7's reasoning).

- [ ] **Step 7: Gate and commit**

```bash
cd frontend && npm run lint && npm run typecheck && npx vitest run src/routes/control-plane/catalogs && cd ..
git add -A && git commit -m "feat(catalogs): create flow with a typed form per kind and per-engine advanced properties"
```

---
### Task 4: Detail — Configuration and Workspaces

**Files:**
- Replace in full: `frontend/src/routes/control-plane/catalogs/CatalogDetail.tsx` (Task 2's placeholder)
- Create: `frontend/src/routes/control-plane/catalogs/ConfigurationTab.tsx`, `WorkspacesTab.tsx`, `__tests__/CatalogDetail.test.tsx`
- Create as a placeholder (Step 1), replaced in full by Task 5: `catalogs/EnginesTab.tsx`

**Interfaces:**
- Consumes: `useControlPlaneCatalogRegistryGetRegistryCatalog(id, options)` → `RegistryCatalogDto`; `useControlPlaneCatalogRegistryUpdateRegistryCatalog` → `CatalogUpdateResultDto`; `useControlPlaneCatalogRegistryCatalogImpact(id, options)` → `CatalogImpactDto`; `useControlPlaneCatalogRegistryListCatalogWorkspaces(id, options)` → `CatalogWorkspaceDto[]`; `useControlPlaneCatalogRegistrySetCatalogWorkspaces`; the Task 3 forms; `useMyWorkspaces(true)`.
- Produces:
  - `CatalogDetail.tsx`: `export default function CatalogDetail()`; `export const catalogKey = (id: string) => ["control-plane", "catalogs", id] as const;`
  - `ConfigurationTab.tsx`: `export function ConfigurationTab({ catalog }: { catalog: RegistryCatalogDto })`
  - `WorkspacesTab.tsx`: `export function WorkspacesTab({ catalogId }: { catalogId: string })`

- [ ] **Step 1: The shell** — same skeleton as `WarehouseDetail.tsx:139-241`: a back link, a summary band, then `<Tabs>` whose active tab is `?tab=`. The default tab is **`configuration`** (the warehouse page's default was `roles`, which is not a tab every catalog has).

```tsx
const [sp, setSp] = useSearchParams();
const tab = sp.get("tab") ?? "configuration";
const setTab = (v: string) =>
  setSp((prev) => { const n = new URLSearchParams(prev); n.set("tab", v); return n; }, { replace: true });
```

The `<TabsList>` in this task holds exactly three triggers — `configuration`, `engines` (Task 5), `workspaces`. Task 6 adds the kind-conditional five. Task 5's `EnginesTab` is a placeholder here, the same one-liner convention Task 2 used.

The summary band shows name (monospace — it is a SQL identifier), `kindLabel`, engine count, workspace count, `createdAt`/`updatedAt`, and for an egret warehouse a link to its storage connection.

- [ ] **Step 2: Configuration, read mode** — a `<DetailFact>` grid over the typed `config`, plus the `advanced` map per engine, plus `description`. Render the **same** `kinds/forms.tsx` components in a read-only pass rather than a second renderer: pass `onChange={() => {}}` and wrap in a `<fieldset disabled>`.

> A `fieldset[disabled]` is exactly what `toBeDisabled` walks up to. A test asserting "the edit control is disabled" therefore passes whether or not the control itself is disabled — so the read-mode test asserts the **absence of a Save button**, not the disabledness of an input.

- [ ] **Step 3: Configuration, edit mode** — an Edit button swaps in the live form. `name` and `kind` are rendered as static text and are not in the body: `UpdateRegistryCatalogBody` is `{ config, advanced?, description? }` and its own doc comment says "Replaces config/advanced/description; name and kind never change."

Before saving, show the blast radius from the impact route, because an edit re-applies to every attached engine:

```tsx
const impact = useControlPlaneCatalogRegistryCatalogImpact(catalog.id, {
  query: { enabled: editing && catalog.engines.length > 0, retry: false },
});
// action is `restart` (Trino rolls its pods) | `in_place` (StarRocks rebuilds
// the connector) — CatalogImpactEngineDto's own JSDoc. Say which, per engine:
// "trino-prod will restart", "sr-eu updates in place".
```

`update` returns `CatalogUpdateResultDto { catalog, outcomes: ApplyOutcomeDto[] }`. A **partial** success is the normal case and must not read as a failure: the mutation's `onSuccess` shows one toast per `outcomes[i].status === "failed"` naming `engineName` and `error`, and a single success toast otherwise. Spec §4.5.3 is explicit that a failure flags that attachment and moves on — the other engines already hold the new configuration and nothing reverts.

An egret-warehouse catalog's configuration is **read-only** — its `config` is `{ metastore: { type: "egret", warehouse_id } }`, system-managed, and the CHECK constraint ties `warehouse_id` to it. Render the Edit button only when `!isEgretWarehouse(catalog)`, with the reason beside it.

- [ ] **Step 4: Workspaces** — a checkbox list of every workspace from `useMyWorkspaces(true)`, checked from `listCatalogWorkspaces(id)`, saved through `setCatalogWorkspaces` with the **full set** (`SetCatalogWorkspacesBody { workspaceIds: string[] }` is a replace, not a delta — sending only the additions silently unassigns everything else). Show `assignedAt` per assigned row. Gate the Save button on `catalog_registry:update` via `useAuth()`.

- [ ] **Step 5: Tests** — `__tests__/CatalogDetail.test.tsx`:
1. the three tabs render and `?tab=workspaces` selects Workspaces on first paint;
2. read mode shows the JDBC host and **no** Save button; Edit reveals Save;
3. an egret-warehouse catalog shows no Edit button;
4. saving with two engines shows the impact sentence for each (`restart` / `in_place`) before the confirm;
5. an `outcomes` array with one `failed` and one `applied` produces one error toast naming the failed engine and does **not** roll the form back to read mode with an error banner;
6. Workspaces save sends the **whole** id array: check one extra box on a catalog already assigned to two workspaces and assert `workspaceIds` has length 3.

- [ ] **Step 6: Gate and commit**

```bash
cd frontend && npm run lint && npm run typecheck && npx vitest run src/routes/control-plane/catalogs && cd ..
git add -A && git commit -m "feat(catalogs): catalog detail with Configuration and Workspaces tabs"
```

---
### Task 5: Detail — the Engines tab

Depends on Task 1: `useControlPlaneCatalogRegistryDetachCatalog` does not exist before it.

**Files:**
- Replace in full: `frontend/src/routes/control-plane/catalogs/EnginesTab.tsx` (Task 4's placeholder)
- Create: `frontend/src/routes/control-plane/catalogs/__tests__/EnginesTab.test.tsx`

**Interfaces:**
- Consumes: `useControlPlaneEnginesListEngines` (for the attach picker's candidates); `useControlPlaneCatalogRegistryAttachCatalog` (`{ id: engineId, cid: catalogId }`); `useControlPlaneCatalogRegistryDetachCatalog` (`{ id: catalogId, engineId }` — **note the inverted argument order versus attach**, because the routes are mounted under different resources); `useControlPlaneCatalogRegistryRetryCatalogApply` (`{ id: engineId, cid: catalogId }`); `testEngineCatalog(engineId, catalogId)` from `@/api/controlPlane` (`:1030`) → `CatalogTestResult { ok: boolean; message: string }` (`:1025`, the hand-written alias of the generated `CatalogTestDto`).
- Produces: `export function EnginesTab({ catalog }: { catalog: RegistryCatalogDto })`.

- [ ] **Step 1: The attachment table** — one row per `catalog.engines`, columns Engine (a link to `/control-plane/engines/{engineId}`), Type (`<Badge>`), Status, Applied, Actions.

Status renders all three values with their meanings, taken from `CatalogAttachmentDto.applyStatus`'s own JSDoc — which is where they reach the client, since response descriptions do not:

```tsx
function StatusBadge({ a }: { a: CatalogAttachmentDto }) {
  if (a.applyStatus === "failed")
    return <Badge variant="destructive" title={a.applyError ?? undefined}>failed</Badge>;
  if (a.applyStatus === "pending")
    return <Badge variant="outline" title="egret has recorded this attachment and has not yet been able to tell the engine. A StarRocks FE takes 5-10 minutes to come up after create; the bootstrap or Retry converges it.">pending</Badge>;
  return <Badge variant="secondary">applied</Badge>;
}
```

A `failed` row shows `applyError` in full below the table, not only in a `title` — it is the one string that says what to fix.

- [ ] **Step 2: Attach** — a `<Select>` of engines not already in `catalog.engines`, plus an Attach button:

```tsx
const attach = useControlPlaneCatalogRegistryAttachCatalog({
  mutation: {
    onSuccess: (res) => {
      invalidate();
      // SP3b D1: a StarRocks attach returns 201 even when the FE refused, with
      // applyStatus `failed` and the reason in applyError. Reporting "attached"
      // and nothing else would hide the half of the outcome that matters.
      if (res.status === 201 && res.data.applyStatus === "failed")
        toast.error(`Attached, but the engine refused it: ${res.data.applyError ?? "no reason given"}`);
      else toast.success("Attached.");
    },
    onError: (e) => toast.error(e.error ?? "Could not attach."),
  },
});
```

- [ ] **Step 3: Detach** — a `ConfirmDeleteDialog` with `confirmLabel="Detach"` and `confirmText` the catalog name, calling `detach.mutate({ id: catalog.id, engineId })`. The 400 the primary catalog returns is surfaced verbatim; do not pre-disable the button on a guess about which attachment is primary — SP3c-1 has no `is_primary` field to read (that is D2, SP3c-2), and a client-side guess from `engines.warehouseId` would be a second copy of a rule the backend already owns.

- [ ] **Step 4: Retry** — shown on a `failed` or `pending` row:

```tsx
const retry = useControlPlaneCatalogRegistryRetryCatalogApply({
  mutation: { onSuccess: () => { invalidate(); toast.success("Re-applied."); },
              onError: (e) => toast.error(e.error ?? "The engine rejected the re-apply.") },
});
```

- [ ] **Step 5: Test, labelled as what it is (D4, table row 2)** — one button per **Trino** attachment:

```tsx
/**
 * D4. There is exactly one test egret can run today:
 * `POST /engines/{id}/catalogs/{cid}/test` submits `SHOW SCHEMAS FROM "<name>"`
 * to the engine's TRINO coordinator (`engines.rs:5655` → `monitor::coordinator_call`,
 * whose fallback Service is `{name}-trino`). There is no TCP prober in the tree
 * and SP3c-1 adds no second backend route, so:
 *   - a Trino attachment gets the button, and the result names the statement;
 *   - every other row gets it disabled, with the reason;
 *   - "the port answers" is never shown, because nothing measures it yet (SP5).
 */
const ok = `SHOW SCHEMAS FROM ${catalog.name} succeeded on ${a.engineName}`;
const notRun = "Testing needs a Trino engine attached — egret has no way to reach this catalog on its own yet.";
```

The success text must name the statement and the engine. A bare green tick is what D4 exists to forbid.

- [ ] **Step 6: Tests** — `__tests__/EnginesTab.test.tsx`:
1. the three statuses render their distinct badges, and a `failed` row shows `applyError` as text;
2. attach calls the hook with `{ id: <engineId>, cid: <catalogId> }` — assert the object, since both fields are strings and a swap type-checks;
3. detach calls the hook with `{ id: <catalogId>, engineId: <engineId> }` — same reason, opposite order;
4. Retry appears on `failed` and `pending` rows and not on `applied`;
5. a 201 whose `applyStatus` is `failed` produces the error toast, not the success one;
6. the Test button is enabled for a `trino` attachment and its success text contains `SHOW SCHEMAS FROM`; for a `starrocks`-only catalog it is disabled and the reason sentence is present.

- [ ] **Step 7: Gate and commit**

```bash
cd frontend && npm run lint && npm run typecheck && npx vitest run src/routes/control-plane/catalogs && cd ..
git add -A && git commit -m "feat(catalogs): Engines tab with attach, detach, apply status, Retry and a labelled Test"
```

---
### Task 6: Kind-conditional tabs (D1), the redirects, and the moved links

The largest task, because reusing five tabs means extracting them from a 2379-line file.

**Files:**
- Create: `frontend/src/routes/control-plane/warehouse-tabs/{shared,RolesTab,ClientsTab,BrowseTab,ConnectTab,SettingsTab}.tsx`, `warehouse-tabs/__tests__/warehouseTabs.test.tsx`
- Modify: `frontend/src/routes/control-plane/catalogs/CatalogDetail.tsx`, `frontend/src/App.tsx`, `frontend/src/components/layout/Sidebar.tsx`, `frontend/src/routes/control-plane/WorkspaceDetail.tsx`, `frontend/src/routes/control-plane/StorageDetail.tsx`, `frontend/src/routes/control-plane/__tests__/WorkspaceDetail.test.tsx`, `frontend/src/components/layout/__tests__/Sidebar.test.tsx`
- Delete: `frontend/src/routes/control-plane/Warehouses.tsx`, `WarehouseDetail.tsx`, `__tests__/Warehouses.test.tsx`, `__tests__/WarehouseDetail.test.tsx`

**Interfaces:**
- Produces, all with **unchanged** signatures — this is the point of the extraction:
  - `RolesTab({ warehouseId }: { warehouseId: string })`
  - `ClientsTab({ warehouseId }: { warehouseId: string })`
  - `BrowseTab({ warehouseId }: { warehouseId: string })`
  - `ConnectTab({ warehouse }: { warehouse: Warehouse | undefined })`
  - `SettingsTab({ warehouseId, storageConnId }: { warehouseId: string; storageConnId?: string })`
  - `shared.tsx`: `CopyButton`, `consumerDot`, `rolesKey`, `clientsKey`
- Consumes (new, in `CatalogDetail`): `getWarehouse` from `@/api/controlPlane`, for `ConnectTab`'s `Warehouse` argument.

- [ ] **Step 1: What moves, what stays, what dies.** Do this as a mechanical cut-and-paste — **no rewriting inside a moved body.** A behaviour change smuggled into a 1000-line move is invisible in review.

**Moves**, with its helpers, to the file named in the File Structure table: `RolesTab`, `ClientsTab`, `BrowseTab`, `ConnectTab`, `SettingsTab`, `CopyButton`, `consumerDot`, `consumerHint`, `RoleRow`, `AddRoleDialog`, `AddGrantDialog`, `AddClientDialog`, `ProtectToggle`, `NsNode`, `TableDetailView`, `DeletedTablesDialog`, `Fact2`, `BrowseEmpty`, `WarehouseTasksCard`, `Coord`, `CodeBlock`, `CredField`, `CONSUMER_TYPES`, `rolesKey`, `clientsKey`, `grantsKey`.

**Dies with the file:**
- `WarehouseDetail` itself (the shell) — replaced by `CatalogDetail`'s shell.
- `WarehouseSummary` (`:270`) — `CatalogDetail`'s own summary band replaces it, and it reads `Warehouse` fields the catalog DTO does not carry.
- the warehouse `EnginesTab` (`:1139`) — **superseded, not lost** (table row 3). It rendered `engines.filter(e => e.warehouseId === warehouseId)`: the engines whose *primary* catalog this warehouse is. `catalog.engines` is every attachment, which since SP3a includes each of those engines' primary attachment. Every row the old tab drew is a row the new tab draws, with an apply status the old one never had. Do not port it.
- `Warehouses` + `AddWarehouseDialog` (`Warehouses.tsx:244`) — the list page. Creating a warehouse moved to `CatalogCreate`'s `egret_warehouse` kind (Task 3 Step 4); deleting one moved to the Catalogs list (Task 2 Step 5). Both call the same legacy endpoints.

`rolesKey` and `clientsKey` are currently **exported** from `WarehouseDetail.tsx`; re-check their importers before moving and update them:

```bash
cd frontend && rtk proxy grep -rn "rolesKey\|clientsKey" src | grep -v warehouse-tabs
```

- [ ] **Step 2: Do the move**, one file at a time, running `npx tsc --noEmit` after each so an unresolved helper surfaces at the file that needed it rather than at the end. Each new file takes only the imports it uses — ESLint's unused-import rule is the check that the split is clean.

- [ ] **Step 3: Kind-conditional tabs in `CatalogDetail`** (D1):

```tsx
// D1: Configuration, Engines and Workspaces for EVERY catalog. An egret
// warehouse catalog additionally shows the tabs that were `WarehouseDetail`'s,
// REUSING those components — they keep their `warehouseId: string` prop and the
// registry DTO carries `warehouseId` for exactly this kind of catalog and no
// other (0147's CHECK constraint). A jdbc catalog simply has fewer tabs.
const wh = isEgretWarehouse(catalog) ? catalog.warehouseId! : null;

// ConnectTab needs the Warehouse row itself (name + lakekeeperWarehouseId),
// which the catalog DTO does not carry; fetch it only for this kind.
const warehouseQ = useQuery<Warehouse, ApiError>({
  queryKey: ["control-plane", "warehouses", wh],
  queryFn: () => getWarehouse(wh!),
  enabled: wh !== null,
  retry: false,
});
```

Render the five extra `<TabsTrigger>`/`<TabsContent>` pairs after the three, in `WarehouseDetail.tsx:196-218`'s original order minus Engines: Roles & grants, Catalog Clients, Browse, Connect, Settings — same icons, same labels, same `value`s (`roles`, `clients`, `browse`, `connect`, `settings`), so a bookmarked `?tab=browse` still lands where it did.

- [ ] **Step 4: Redirects and links.**

In `App.tsx`, replace the two warehouse routes with redirects. `/control-plane/warehouses` is static; `/control-plane/warehouses/:id` has to resolve a warehouse id to a catalog id, which needs a fetch:

```tsx
/**
 * §3: `/control-plane/warehouses/*` redirects to the matching catalog. The id in
 * the old URL is a WAREHOUSE id; the registry's `warehouseId` is the only field
 * that maps it, so the redirect has to wait for the list. A warehouse with no
 * registry row (impossible after SP3a's backfill, but a 404 is not a crash)
 * lands on the list.
 */
function WarehouseRedirect() {
  const { id = "" } = useParams();
  const q = useControlPlaneCatalogRegistryListRegistryCatalogs(undefined, { query: { retry: false } });
  const [sp] = useSearchParams();
  if (q.isLoading) return <div className="p-10">Loading…</div>;
  const rows = q.data?.status === 200 ? q.data.data : [];
  const match = rows.find((c) => c.warehouseId === id);
  const tab = sp.get("tab");
  return (
    <Navigate
      replace
      to={match ? `/control-plane/catalogs/${match.id}${tab ? `?tab=${encodeURIComponent(tab)}` : ""}` : "/control-plane/catalogs"}
    />
  );
}
```

```tsx
<Route path="warehouses" element={<Navigate to="/control-plane/catalogs" replace />} />
<Route path="warehouses/:id" element={<RequirePermission perm="catalog_registry:read"><WarehouseRedirect /></RequirePermission>} />
```

The guard becomes `catalog_registry:read`, not `warehouse:read`: the destination is the Catalogs page, and sending a `warehouse:read`-only user to a page they will 403 on is a worse landing than a clean 403 here. Both keys are in `CONTROL_PLANE_PERMS` for the whole of SP3c-1, so neither loses the app.

Then the three literal route links (table row 4):
- `WorkspaceDetail.tsx:377` → `/control-plane/catalogs?workspace=${workspaceId}`
- `WorkspaceDetail.tsx:403` → `` `/control-plane/catalogs/${catalogIdFor(w.id)}` `` — this card lists `Warehouse` rows, so link through the registry list the same way `WarehouseRedirect` does, or (simpler, and what this plan does) leave the card's data source alone and point the link at `/control-plane/warehouses/${w.id}`, which now redirects. **Choose the second**: the redirect exists precisely so that every remaining warehouse-id link keeps working, and rewiring this card's query is the §5 bullet table row 11 puts out of scope.
- `StorageDetail.tsx:543` → same treatment, same reason.

Remove the Warehouses entry from `Sidebar.tsx`'s Infrastructure group now that its route is a redirect.

- [ ] **Step 5: The grep that falsifies this step.** Table row 5 exists because the research list was wrong once:

```bash
cd frontend
rtk proxy grep -rn "control-plane/warehouses" src tests | grep -v "src/api/generated"
```
Expected, and nothing else: `src/api/catalogTags.ts:42,53,70` (an **API** path, re-gated in SP3c-2 under D3 — deliberately untouched), `src/App.tsx` (the two redirect routes), `src/routes/control-plane/WorkspaceDetail.tsx` and `StorageDetail.tsx` (links that now resolve through the redirect), `tests/e2e/helpers.ts:109` and `tests/e2e/embedded-catalog.spec.ts` (API paths). If anything else appears, fix it here rather than in Task 9.

```bash
git diff --stat HEAD -- src/components/access/CatalogTagEditor.tsx
```
Expected: empty (table row 8).

- [ ] **Step 6: The tests follow the components.** `WarehouseDetail.test.tsx`'s two cases move to `warehouse-tabs/__tests__/warehouseTabs.test.tsx`, keeping every mock and assertion. The only change is what is rendered:

```tsx
function renderTab(ui: React.ReactElement) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>{ui}</MemoryRouter>
    </QueryClientProvider>,
  );
}
// "browse namespaces -> tables -> schema": renderTab(<BrowseTab warehouseId="w1" />)
//   and DROP the `await user.click(await screen.findByRole("tab", { name: /Browse/ }))`
//   line — there is no tab strip around the component any more. Everything after it
//   is unchanged, which is what makes the move falsifiable.
// "add grant via browse picker": renderTab(<RolesTab warehouseId="r-wh" />), with
//   `getWarehouse` no longer needed in the beforeEach.
```

`Warehouses.test.tsx` is deleted with its page. Its three surviving assertions already have homes: the render/empty-state cases in Task 2 Step 7's cases 1 and 4, and the `?workspace=` case in Task 2 Step 7's case 3.

Add one case to `warehouseTabs.test.tsx` and two to `CatalogDetail.test.tsx`:

```tsx
test("an egret warehouse catalog shows the five warehouse tabs", /* … */);
test("a jdbc catalog shows only Configuration, Engines and Workspaces", /* … */);
```

and a redirect test per moved path, in `App.test.tsx` or a new `__tests__/warehouseRedirect.test.tsx`:

```tsx
test("/control-plane/warehouses redirects to /control-plane/catalogs", /* … */);
test("/control-plane/warehouses/wh-1 redirects to the catalog whose warehouseId is wh-1", /* … */);
test("/control-plane/warehouses/wh-1?tab=browse keeps the tab", /* … */);
test("a warehouse with no registry catalog lands on the list, not a blank page", /* … */);
```

`WorkspaceDetail.test.tsx:206,212` assert the two hrefs; `:212` becomes `/control-plane/catalogs?workspace=${WS.id}` and `:206` stays `/control-plane/warehouses/wh1` (the redirect target), with a comment saying the redirect is what makes it still correct.

- [ ] **Step 7: Gate and commit**

```bash
cd frontend
npm run lint && npm run typecheck
npx vitest run src/routes/control-plane src/components/layout src/components/access
cd ..
git add -A && git commit -m "feat(catalogs): reuse the warehouse tabs on egret catalogs; redirect /control-plane/warehouses"
```

---
### Task 7: Engine detail's Catalogs tab

**Files:**
- Modify: `frontend/src/routes/control-plane/EngineDetail.tsx`, `frontend/src/routes/control-plane/__tests__/EngineDetail.test.tsx`
- Delete: `frontend/src/routes/control-plane/CatalogForm.tsx`

**Interfaces:**
- Consumes: `useControlPlaneCatalogRegistryListRegistryCatalogs`, `useControlPlaneCatalogRegistryAttachCatalog`, `useControlPlaneCatalogRegistryDetachCatalog`, `useControlPlaneCatalogRegistryRetryCatalogApply`.
- Produces: `CatalogsCard({ engine })` keeps its name, its props and its place in the tab strip; only its body changes.

- [ ] **Step 1: Replace the body.** `CatalogsCard` (`EngineDetail.tsx:1298`) is a master/detail over `listEngineCatalogs` with `CatalogForm` in the right pane. It becomes a table of this engine's attachments plus a picker:

```tsx
const catalogsQ = useControlPlaneCatalogRegistryListRegistryCatalogs(undefined, { query: { retry: false } });
const all = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];
// An attachment of THIS engine, read off the catalog rows — no second endpoint.
const attached = all
  .map((c) => ({ catalog: c, a: c.engines.find((e) => e.engineId === engine.id) }))
  .filter((r): r is { catalog: RegistryCatalogDto; a: CatalogAttachmentDto } => r.a !== undefined);
const candidates = all.filter((c) => !c.engines.some((e) => e.engineId === engine.id));
```

Each row: catalog name (link to `/control-plane/catalogs/{id}`), kind, the same `StatusBadge` Task 5 defined, Retry on `failed`/`pending`, Detach. The picker attaches. The header prose stops promising a pod roll unconditionally — Trino restarts, StarRocks rebuilds in place — and says so per engine type.

Reuse Task 5's components rather than writing a second set: export `StatusBadge` and the attach/detach/retry action cells from `catalogs/EnginesTab.tsx` and import them here. Two copies of the "a 201 can be `failed`" rule is one copy too many.

- [ ] **Step 2: Delete `CatalogForm.tsx`** and its imports. Confirm nothing else referenced it:

```bash
cd frontend && rtk proxy grep -rn "CatalogForm" src
```
Expected: empty.

- [ ] **Step 3: Tests** — in `EngineDetail.test.tsx` (954 lines), replace the `CatalogForm`-driven cases. Rewrite rather than adapt: the cases asserted a form that no longer exists.
1. the tab lists the engine's attachments with their statuses and omits catalogs attached elsewhere;
2. the picker offers only unattached catalogs;
3. attach calls the hook with `{ id: engine.id, cid: catalog.id }`;
4. Detach calls the hook with `{ id: catalog.id, engineId: engine.id }`;
5. a 400 from a primary-catalog detach surfaces the server's message verbatim, not a generic one.

- [ ] **Step 4: Gate and commit**

```bash
cd frontend && npm run lint && npm run typecheck && npx vitest run src/routes/control-plane/__tests__/EngineDetail.test.tsx src/routes/control-plane/catalogs && cd ..
git add -A && git commit -m "feat(engines): pick catalogs from the registry instead of the inline catalog form"
```

---
### Task 8: The create-engine wizard

**Files:**
- Modify: `frontend/src/routes/control-plane/EngineCreate.tsx`, `frontend/src/routes/control-plane/__tests__/EngineCreate.test.tsx`

**Interfaces:**
- Consumes: `useControlPlaneCatalogRegistryListRegistryCatalogs`, `useControlPlaneCatalogRegistryAttachCatalog`, `createEngine` (unchanged).
- Produces: no new export; `CreateEngineInput.warehouseId` is still sent, now derived (table row 7).

- [ ] **Step 1: Primary catalog replaces the warehouse `<Select>`** (`:206-226`):

```tsx
/**
 * `CreateEngineBody.warehouse_id` is a `Uuid`, not an `Option` (engines.rs:152),
 * and D2's `is_primary` migration is SP3c-2 — so an engine still needs exactly
 * one primary catalog at create time. What changes here is the vocabulary and
 * the reach: the user picks a CATALOG (every egret warehouse catalog is one,
 * and `warehouseId` is the field that makes it one), and can attach more
 * catalogs in the same screen. The body is unchanged.
 */
const catalogsQ = useControlPlaneCatalogRegistryListRegistryCatalogs(undefined, { query: { retry: false } });
const allCatalogs = catalogsQ.data?.status === 200 ? catalogsQ.data.data : [];
const primaryCandidates = allCatalogs.filter((c) => c.warehouseId);
const [primaryCatalogId, setPrimaryCatalogId] = useState("");
const [extraCatalogIds, setExtraCatalogIds] = useState<string[]>([]);
const primary = primaryCandidates.find((c) => c.id === primaryCatalogId);
```

Keep the element id `ec-warehouse` — the e2e spec Task 9 rewrites targets it, and changing the id and the spec in the same pass loses the ability to tell a selector break from a behaviour break. Relabel it "Primary catalog". The `canCreate` precondition (`:119-123`) becomes `primaryCatalogId.length > 0`.

- [ ] **Step 2: Additional catalogs** — a checkbox list below it, over `allCatalogs.filter(c => c.id !== primaryCatalogId)`, each row showing kind and current engine count. Help text: "Attached after the engine is created. A catalog that fails to attach leaves the engine in place — attach it again from the engine's Catalogs tab."

- [ ] **Step 3: Submit** — `handleCreate` (`:125-152`) sends `warehouseId: primary!.warehouseId!` and then attaches:

```tsx
const engine = await createMutation.mutateAsync(input);
const failures: string[] = [];
for (const cid of extraCatalogIds) {
  const res = await attach.mutateAsync({ id: engine.id, cid }).catch(() => null);
  const c = allCatalogs.find((x) => x.id === cid);
  // A 201 whose applyStatus is `failed` is a failure the user must see (SP3b D1).
  if (!res || res.status !== 201 || res.data.applyStatus === "failed") failures.push(c?.name ?? cid);
}
if (failures.length > 0) toast.error(`Engine created. These catalogs did not attach: ${failures.join(", ")} — retry from the engine's Catalogs tab.`);
navigate(`/control-plane/engines/${engine.id}`);
```

Sequential, not `Promise.all`: each attach triggers a Trino config apply and, for Trino, a pod roll; firing them concurrently at one engine is how you get a 409 storm from your own UI.

- [ ] **Step 4: Tests** — `EngineCreate.test.tsx` (133 lines):
1. the Primary catalog select lists only catalogs with a `warehouseId`;
2. `createEngine` is called with `warehouseId` equal to the **picked catalog's** `warehouseId`, not its `id` — the two are both UUID strings, so this is the assertion that catches the confusion;
3. two extra catalogs produce two `attach` calls, in order, after the create resolves;
4. an attach that returns `applyStatus: "failed"` names that catalog in the error toast and still navigates to the engine.

- [ ] **Step 5: Gate and commit**

```bash
cd frontend && npm run lint && npm run typecheck && npx vitest run src/routes/control-plane/__tests__/EngineCreate.test.tsx && cd ..
git add -A && git commit -m "feat(engines): create wizard picks a primary catalog and attaches more"
```

---
### Task 9: The e2e rewrite, the full gate, and a live check (stop before the PR)

**Files:**
- Rewrite: `frontend/tests/e2e/provisioning-ui-kind.spec.ts`

- [ ] **Step 1: Rewrite the stale spec** (table row 12). Today it opens a dialog that became a page:

| It expects | The code renders |
|---|---|
| `getByRole("dialog")` containing "Add Trino engine" | a full page at `/control-plane/engines/new` (`EngineCreate.tsx`) |
| `#cpe-name`, `#cpe-warehouse`, `#cpe-workers`, `#cpe-memory` | `#ec-name`, `#ec-warehouse`, and the Trino editor's own fields |
| a dialog button "Add engine" | the page's "Create engine" |

Rewrite it to drive what SP3c-1 ships, and to create the catalog through the UI rather than seeding it over the API — an e2e that seeds everything it needs through `request` proves only that the last click works:

```ts
// 1. Catalogs → New catalog → kind "egret warehouse" → name/workspace/storage → Create.
await page.goto("/control-plane/catalogs");
await page.getByRole("link", { name: "New catalog" }).click();
await page.locator("#cc-kind").click();
await page.getByRole("option", { name: "egret warehouse" }).click();
await page.locator("#cc-name").fill(catalogName);
// … workspace + storage selects …
await page.getByRole("button", { name: "Create catalog" }).click();
await expect(page.getByRole("link", { name: catalogName })).toBeVisible({ timeout: 30_000 });

// 2. Engines → New engine → Primary catalog = that catalog → 1 worker / 1Gi → Create.
await page.goto("/control-plane/engines/new");
await page.locator("#ec-name").fill(engineName);
await page.locator("#ec-warehouse").click();
await page.getByRole("option", { name: catalogName }).click();
// … workers/memory via the Trino editor …
await page.getByRole("button", { name: "Create engine" }).click();

// 3. The list row reaches `running` (real k8s; the badge polls every 5s).
// 4. The catalog's Engines tab shows that engine with `applied`.
await page.goto(`/control-plane/catalogs/${catalogId}?tab=engines`);
await expect(page.getByRole("row").filter({ hasText: engineName }).getByText("applied")).toBeVisible({ timeout: 120_000 });
// 5. Delete the engine through the UI; 6. delete the catalog through the UI.
```

Keep `EGRET_E2E_KIND=1` opt-in, `test.describe.configure({ mode: "serial" })`, the 600 s timeout, and the `finally` cleanup. **Log in once** — the login route is rate-limited 5/5min.

```bash
cd frontend && npx tsc --noEmit -p tsconfig.json && npx playwright test --list tests/e2e/provisioning-ui-kind.spec.ts && cd ..
git add -A && git commit -m "test(e2e): rewrite the provisioning UI spec against the page and the Catalogs flow"
```
`--list` compiles the spec without running it, which is the only way to learn that a Playwright spec is broken without a kind cluster — and not knowing that is how the old one rotted.

- [ ] **Step 2: Branch and tree**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git branch --show-current
git status --short
git log --oneline origin/main..HEAD
```
Expected: `feat/catalog-ui-sp3c-1`, a clean tree, nine commits (Tasks 1–8 plus Step 1).

- [ ] **Step 3: Rust gates**

```bash
export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
env -u EGRET_ALLOW_DB_SKIP EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 cargo test --workspace --no-fail-fast
```
Expected: no failures; record any pre-existing `egret-workflow` clippy findings rather than fixing them. Read `egret-engine-mgmt`'s `routes` binary `finished in`: seconds, not milliseconds, or the DB tests skipped and Task 1 proved nothing. A failure outside this branch's crates is called "pre-existing" only after running it on `origin/main` in a separate worktree and seeing it fail there too.

- [ ] **Step 4: OpenAPI and the client have no drift**

```bash
rtk proxy cargo run -q -p egret-bin -- openapi > /tmp/planSP3c-openapi.json
cmp /tmp/planSP3c-openapi.json openapi.json
cargo test -p egret-server --lib committed_openapi_json_is_current
cd frontend && npm run gen:api && git status --short src/api/generated && cd ..
```
Expected: `cmp` silent; `git status` after `gen:api` shows nothing. (Use the session's scratchpad rather than `/tmp`, prefixed `planSP3c-`.)

- [ ] **Step 5: The frontend gate, once, whole**

```bash
cd frontend
npm run lint && npm run typecheck && npm test && npm run build
rtk proxy grep -rn "control-plane/warehouses" src tests | grep -v "src/api/generated"
cd ..
```
Expected: green, and the grep returns only the six sites Task 6 Step 5 enumerates.

- [ ] **Step 6: Kind cluster — manual, reported as manual.** Deploy with `just deploy-kind` (or the OIDC variant the cluster runs), sign in as `admin@egret.local` / `Admin12345!`, and walk §5's own list:
  - **Create** a catalog in the UI: a `jdbc`/PostgreSQL one pointed at the in-cluster `postgres`. It appears in the list with kind `jdbc`, no engines, no workspaces.
  - **Attach** it to the Trino engine from the catalog's Engines tab. The status settles to `applied`. Press **Test** → the result names `SHOW SCHEMAS FROM <name>` and the engine. Record the text verbatim.
  - **Retry** on a deliberately broken catalog (wrong password secret) → the row goes `failed` and `applyError` is readable on the page, not only in a tooltip.
  - **Detach** it → the row goes, the catalog stays in the list. Then attach it again and **detach the primary** → a 400 whose message is shown verbatim.
  - **Assign** it to a workspace in the Workspaces tab, then open that workspace in Explore and confirm the catalog is visible to it.
  - **Redirects**: open `/control-plane/warehouses` and `/control-plane/warehouses/<a real warehouse id>?tab=browse` and confirm both land on the catalog, with the Browse tab selected on the second.
  - **Kind-conditional tabs**: the egret warehouse catalog shows Roles & grants, Catalog Clients, Browse, Connect, Settings; the jdbc catalog shows only Configuration, Engines, Workspaces.
  - **The lock-out test, live**: create a role holding only `catalog_registry:read`, assign a user to it, sign in as them → Control Plane opens on Catalogs. This is the whole reason D3 exists and the one check no unit test can stand in for.
  - `EGRET_E2E_KIND=1 npx playwright test tests/e2e/provisioning-ui-kind.spec.ts` → green, in minutes of real work. A skip here means the rewritten spec is as unfalsifiable as the one it replaced.
  Report every result as "manual, on kind", never as a test.

- [ ] **Step 7: Stop.** Do not push and do not open a PR. Report: the nine commit hashes, the gate outputs (with any recorded pre-existing clippy findings), the kind results verbatim — especially the Test button's exact wording and the `catalog_registry:read`-only sign-in — and anything left open.

---

## Self-review (against the design)

**Coverage of §3, bullet by bullet.** "Control Plane → Infrastructure → Catalogs. List: name, kind, attached engines, assigned workspaces, and a warning when any attachment is `failed` or stuck `pending`" — Task 2 Steps 3 and 6, with the warning rule made falsifiable in Step 7 case 2 and the `pending` grace period justified in table row 10. "Create: choose a kind, fill the typed form, optionally an Advanced tab per engine. Built from the existing config primitives" — Task 3, with the missing fourth primitive recorded in table row 9. "Detail: Configuration, Engines (attach, detach, apply status, Retry), Workspaces (assign), plus the six existing warehouse tabs when the catalog is iceberg-egret (D1)" — Tasks 4, 5, 6; five tabs, not six, per table row 3. "`/control-plane/warehouses/*` redirects… Three places link to that path by literal string" — Task 6 Step 4, with the count corrected in table row 4 and a grep that fails the task if the list is wrong. "Engine detail → Catalogs replaces the inline `CatalogForm` with a picker over the registry, showing each attachment's status and offering Retry" — Task 7. "Create-engine wizard attaches catalogs instead of choosing one warehouse" — Task 8, as far as a non-nullable `warehouse_id` allows, per table row 7. "Permissions: `catalog_registry:read` gates the new section, in the route guard, the sidebar and `appAccess.ts`" — Task 2 Steps 1, 2, 6, all in one commit, which is the Global Constraint D3 turns into an invariant. "Backend, only what is missing: a detach endpoint" — Task 1, exactly one route, per table row 1.

**Coverage of D1–D5.** D1 — Task 6 Step 3, tested by two cases in Step 6. D2 — **not in this plan**; it is SP3c-2, and tables rows 3 and 7 name the two places where its absence shapes what SP3c-1 can do (no `is_primary` badge; the wizard still sends `warehouseId`). D3 — the Global Constraints and Task 2 Step 1, with the live check in Task 9 Step 6. D4 — Task 5 Step 5, honestly narrowed by table row 2. D5 — the Global Constraints; this plan deletes four frontend files and no backend line, and Task 6 Step 1 lists exactly what dies and why each is superseded rather than lost.

**Coverage of §5 (testing).** "Vitest for the new pages, including the kind-conditional tabs, the attach picker, the wizard's multi-select, and a redirect test per moved path" — Task 6 Step 6 (tabs, four redirect cases), Task 5 Step 6 and Task 7 Step 3 (the pickers), Task 8 Step 4 (the multi-select). "A permission test that a user holding only `catalog_registry:read` can enter Control Plane and see Catalogs" — Task 2 Step 1, and again live in Task 9 Step 6. "A migration test that every role holding `warehouse:*` ends with the matching `catalog_registry:*`" — **SP3c-2's**, since SP3c-1 adds no migration. "`provisioning-ui-kind.spec.ts` is already stale… SP3c-1 rewrites it" — Task 9 Step 1. "Live on kind, reported as manual" — Task 9 Step 6.

**Placeholder scan.** Every task names real files, real symbols and real commands. The two deliberate placeholders — `CatalogCreate`/`CatalogDetail` in Task 2 Step 2 and `EnginesTab` in Task 4 Step 1 — exist so each commit compiles, and each names the task that replaces it **in full**. No step says "add appropriate tests", "handle errors" or "update the relevant files": Task 6 Step 1 enumerates every symbol that moves and every one that dies; Task 6 Step 5 is a grep whose expected output is listed; Task 9 Step 5 re-runs it.

**Type consistency across tasks.** `RegistryCatalogDto` is the single currency: Task 2's list rows, Task 4's `ConfigurationTab({ catalog })`, Task 5's `EnginesTab({ catalog })`, Task 7's engine-side rows and Task 8's picker all read the same object, and none of them refetches a catalog another task already has. `isEgretWarehouse` is defined once (Task 2) and consumed by Tasks 3, 4 and 6 — the alternative, sniffing `config.metastore.type` at each site, is three chances to disagree with the CHECK constraint. `StatusBadge` and the attach/detach/retry cells are defined once (Task 5) and imported by Task 7, so the "a 201 can be `failed`" rule has one implementation. The five warehouse tabs keep their exact prop types through the move, so their tests move with a one-line change. The one asymmetry that will bite is called out twice, in Task 5's Interfaces and its test cases 2 and 3: **attach takes `{ id: engineId, cid: catalogId }` and detach takes `{ id: catalogId, engineId }`** — both are `{ [k: string]: string }`, so a swap compiles, runs, and 404s at the user.

**Three things a reviewer should push on.**

1. **The Test button is narrower than D4 promises, and the plan does not hide it.** D4 wants a TCP reachability check when no engine is attached. There is no TCP prober anywhere in the tree, and §3 says the backend delta is one detach route. Table row 2 resolves this by shipping only the check that exists — Trino's `SHOW SCHEMAS`, labelled with the statement it ran — and disabling the button elsewhere with the reason. A reviewer who wants D4 whole should ask for the prober as a second Task 1 route rather than let the UI imply a check nothing performs; that is a scope decision, not an oversight, and it is the one place this plan knowingly under-delivers against a decision.

2. **Deleting `Warehouses.tsx` nearly deleted warehouse creation.** The registry refuses egret catalogs on create and on delete, by design — they are created and destroyed with their warehouse. Table row 6 routes both operations back to the legacy endpoints D5 keeps, inside the new screens. It works, and it means the Catalogs create page has one branch that is not a registry call at all. The alternative — a registry create path for egret catalogs — is a backend change SP3c-1 does not have, and would duplicate warehouse provisioning. Worth a second opinion on whether the branch belongs in `CatalogCreate` or in a separate "New warehouse" affordance on the list.

3. **Five tabs, not six, and the sixth's disappearance is an argument, not a measurement.** Table row 3 claims the registry Engines tab is a superset of the warehouse `EnginesTab` because SP3a records a primary attachment for every engine. That follows from `list_engine_attachments(engine.id, Some(engine.warehouse_id))` and the SP3a backfill, but it is a claim about data already in the database, not about code. Task 9 Step 6's kind walk should check it directly: open an egret warehouse catalog's Engines tab and confirm every engine that the old Warehouses → warehouse → Engines tab would have listed is there. If one is missing, the backfill is incomplete and that is a finding worth more than this task.
