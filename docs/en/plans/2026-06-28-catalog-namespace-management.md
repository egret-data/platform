# Catalog — namespace management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a workspace admin create (incl. nested) and drop (empty-only) namespaces in a warehouse, from the warehouse-detail Browse tab.

**Architecture:** Two new admin-gated routes under `/ws/{ws}/catalog/warehouses/{wid}/namespaces` call lakekeeper `NamespaceService::create_namespace` / `drop_namespace` (the browse handlers already use `list_namespaces` the same way). The FE Browse panel gets a "New namespace" dialog + a per-row Delete action.

**Tech Stack:** Rust (axum, embedded lakekeeper iceberg v1), egret-catalog-engine; React 18 + TanStack Query + shadcn; Vitest.

**Spec:** `docs/en/specs/2026-06-28-catalog-namespace-management-design.md`. **Branch:** `feat/catalog-namespaces` (off main).

**Key facts (verified):**
- `crates/egret-catalog-engine/src/browse.rs`: routes under `/ws/{workspace}/catalog/warehouses/{wid}/namespaces` (currently only `GET list_namespaces`). Helper `authorized_warehouse(st, ctx, wid) -> Resolved { prefix: Prefix, request_metadata: RequestMetadata }` checks `catalog:read` (`PERM`) then resolves `store.lakekeeper_id(ws, wid)` (cross-ws → NotFound) → `prefix = Prefix::from(lk_id.to_string())`. Browse handlers take `Path((_ws, wid)): Path<(String, Uuid)>` + `WsContext(ctx)`. `list_namespaces_one_level` shows the lakekeeper call pattern: `<EngineServer as NamespaceService<EngineState>>::list_namespaces(Some(prefix.clone()), query, st.engine.clone(), request_metadata)`. Helpers `parse_ns(ns) -> Vec<String>` (split '.'), `NamespaceIdent::from_vec(levels)`, `system_request_metadata(&st.system_user)`. `BrowseApiError`: NotFound→404, BadRequest→400, Conflict→409 (confirm a `Conflict` variant exists; if not, add one mapping to 409), Forbidden→403. The token routes use `require_workspace_admin`-style gating via `st.authz.require_workspace(ctx, "workspace:manage_members")` — find the exact admin gate the token handlers use and reuse it.
- Imports already present: `lakekeeper::api::iceberg::v1::namespace::{ListNamespacesQuery, NamespaceParameters, NamespaceService}`, `lakekeeper::iceberg::NamespaceIdent`, `lakekeeper::api::iceberg::types::{PageToken, Prefix}`. For create you also need the request type (`CreateNamespaceRequest`) — read the `NamespaceService` trait (same module) for the exact `create_namespace`/`drop_namespace` signatures + request/param types and import what's needed.
- FE `frontend/src/routes/explore/catalog/CatalogBrowse.tsx` (props `{ ws, wid }`): left **Namespaces** panel (maps `useCatalogNamespaces(ws, wid).data` → dotted rows via `nsDotted(n.levels)`). `frontend/src/api/catalog.ts`: `useCatalogNamespaces(ws, wid)`, `catalogKeys` (includes wid), `nsDotted`, `NamespaceDto { levels: string[] }`. Mirror an existing create/delete dialog (`StoragePage.tsx` Add dialog + `ConfirmDeleteDialog`).
- Test DB `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; MinIO + lakekeeper; `--test-threads=1`. Mirror `crates/egret-catalog-engine/tests/warehouse_api.rs` (seeds a storage connection + creates a warehouse → gives a `{wid}`).

---

## Task 1: Backend — create + drop namespace routes

**Files:**
- Modify: `crates/egret-catalog-engine/src/browse.rs` (routes + 2 handlers + an admin-gated resolution helper)
- Test: `crates/egret-catalog-engine/tests/namespace_api.rs` (new; DB+MinIO+lakekeeper-gated)

- [ ] **Step 1: Admin-gated warehouse resolution.** Add `authorized_warehouse_admin(st, ctx, wid) -> Resolved` — identical to `authorized_warehouse` but gating on the workspace-admin permission (`st.authz.require_workspace(ctx, "workspace:manage_members").await?`, matching the token handlers) instead of `catalog:read`. (Or parameterize `authorized_warehouse` with the permission string and keep one helper.)

- [ ] **Step 2: Register routes** (next to the existing `…/{wid}/namespaces` GET):
```rust
.route(
    "/ws/{workspace}/catalog/warehouses/{wid}/namespaces",
    get(list_namespaces).post(create_namespace_route),
)
.route(
    "/ws/{workspace}/catalog/warehouses/{wid}/namespaces/{ns}",
    delete(drop_namespace_route),
)
```
(Name the handlers `create_namespace_route`/`drop_namespace_route` to avoid clashing with the lakekeeper trait methods. Ensure `delete` is imported from `axum::routing`.)

- [ ] **Step 3: `create_namespace_route` handler.**
```rust
#[derive(Deserialize)]
struct CreateNamespaceBody { parent: Option<String>, name: String }

async fn create_namespace_route(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid)): Path<(String, Uuid)>,
    Json(body): Json<CreateNamespaceBody>,
) -> Result<(StatusCode, Json<NamespaceDto>), BrowseApiError> {
    let r = authorized_warehouse_admin(&st, &ctx, wid).await?;
    let name = body.name.trim();
    if name.is_empty() || name.contains('.') {
        return Err(BrowseApiError::BadRequest(
            "namespace name must be non-empty and contain no '.'".into(),
        ));
    }
    // Build the full ident: parent levels (if any) + the new leaf level.
    let mut levels: Vec<String> = match body.parent.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => parse_ns(p),
        None => Vec::new(),
    };
    levels.push(name.to_string());
    let ident = NamespaceIdent::from_vec(levels.clone()).map_err(|_| BrowseApiError::BadRequest("invalid namespace".into()))?;
    // Read the NamespaceService trait for the exact create_namespace signature +
    // CreateNamespaceRequest shape; pass empty properties.
    let request = /* CreateNamespaceRequest { namespace: ident, properties: None } */;
    let _ = <EngineServer as NamespaceService<EngineState>>::create_namespace(
        Some(r.prefix.clone()),
        request,
        st.engine.clone(),
        r.request_metadata,
    )
    .await
    .map_err(map_namespace_err)?;   // already-exists → 409; bad parent → 400/409
    Ok((StatusCode::CREATED, Json(NamespaceDto { levels })))
}
```
Add a small `map_namespace_err(e) -> BrowseApiError` that inspects the lakekeeper error: an already-exists / conflict → `Conflict`, a not-found parent → `BadRequest` (or `Conflict`), else `Internal`. (Mirror how the existing handlers turn `?`/lakekeeper errors into `BrowseApiError` — they currently use `.await?` via a `From<…> for BrowseApiError`; if that mapping already yields the right codes, just use `?` and only special-case the "not empty"/"exists" text if needed.)

- [ ] **Step 4: `drop_namespace_route` handler.**
```rust
async fn drop_namespace_route(
    State(st): State<BrowseState>,
    WsContext(ctx): WsContext,
    Path((_ws, wid, ns)): Path<(String, Uuid, String)>,
) -> Result<StatusCode, BrowseApiError> {
    let r = authorized_warehouse_admin(&st, &ctx, wid).await?;
    let ident = NamespaceIdent::from_vec(parse_ns(&ns)).map_err(|_| BrowseApiError::BadRequest("invalid namespace".into()))?;
    let params = NamespaceParameters { prefix: Some(r.prefix.clone()), namespace: ident };
    <EngineServer as NamespaceService<EngineState>>::drop_namespace(
        params,
        /* the drop flags/query arg the trait requires — read the signature; default = non-recursive */,
        st.engine.clone(),
        r.request_metadata,
    )
    .await
    .map_err(map_namespace_err)?;   // not-empty → 409
    Ok(StatusCode::NO_CONTENT)
}
```
> Confirm the exact `drop_namespace` arity (some iceberg versions take a `NamespaceDropFlags`/recursive query). A non-empty namespace must surface as **409** — verify lakekeeper's error for that case and map it in `map_namespace_err` (match on the error kind or its message containing "not empty"/"not allowed to be dropped").

- [ ] **Step 5: Test `crates/egret-catalog-engine/tests/namespace_api.rs`** (mirror `warehouse_api.rs` setup: seed a `workspace_storages` conn + named secret, create a warehouse to get `{wid}`, add a `ws_admin` token + a non-admin member). Assert:
```
// POST {wid}/namespaces {parent:null, name:"sales"}            -> 201; GET namespaces contains "sales"
// POST {wid}/namespaces {parent:"sales", name:"y2024"}         -> 201; GET contains "sales.y2024"
// POST {wid}/namespaces {parent:null, name:"sales"} (dup)      -> 409
// POST {wid}/namespaces {parent:null, name:"a.b"}              -> 400 (dot in name)
// DELETE {wid}/namespaces/sales.y2024 (empty)                  -> 204; GET no longer contains it
// DELETE {wid}/namespaces/sales (non-empty: still has child OR seed a table) -> 409
// cross-workspace {wid} POST/DELETE                            -> 404
// non-admin member POST/DELETE                                 -> 403
```
For the non-empty case, the easiest is to delete the PARENT while a child still exists (create sales + sales.y2024, then DELETE sales → 409).

- [ ] **Step 6: Run + checks**
```
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-catalog-engine --test namespace_api -- --test-threads=1
cargo build --workspace
cargo fmt --all -- --check
cargo clippy -p egret-catalog-engine --all-targets
```
(retry once on a MinIO flake; do NOT weaken). All green.

- [ ] **Step 7: Commit** `feat(catalog): create + drop namespace routes`.

---

## Task 2: Frontend — New-namespace dialog + Delete action

**Files:**
- Modify: `frontend/src/api/catalog.ts` (2 hooks)
- Modify: `frontend/src/routes/explore/catalog/CatalogBrowse.tsx` (panel header button + row delete)
- Test: `frontend/src/routes/explore/catalog/__tests__/CatalogBrowse.test.tsx` (extend)

- [ ] **Step 1: Hooks in `api/catalog.ts`** (mirror the existing mutation hooks):
```ts
export function useCreateNamespace(ws: string, wid: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { parent: string | null; name: string }) =>
      apiJson(`/ws/${ws}/catalog/warehouses/${wid}/namespaces`, { method: "POST", body: JSON.stringify(body) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: catalogKeys.namespaces(ws, wid) }),
  });
}
export function useDeleteNamespace(ws: string, wid: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (ns: string) =>
      apiJson(`/ws/${ws}/catalog/warehouses/${wid}/namespaces/${encodeURIComponent(ns)}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: catalogKeys.namespaces(ws, wid) }),
  });
}
```
(Match the exact `apiJson`/`catalogKeys.namespaces(ws, wid)` shapes already in the file.)

- [ ] **Step 2: Panel header "New namespace" button + dialog** in `CatalogBrowse.tsx`. Add a `+` button in the "Namespaces" header. On click → a `Dialog` with a **Name** `Input` and a **Parent** `Select` (options: "(root)" + each `nsDotted(n.levels)` from `namespaces.data`). Submit → `useCreateNamespace(ws, wid).mutate({ parent: selected==="(root)" ? null : selected, name })`; on error show the server message (`ApiError.message`, e.g. a 409 "already exists"/parent issue) inline.

- [ ] **Step 3: Per-row Delete action.** On each namespace `<li>`, add a small trash icon button (kept out of the navigate button so it doesn't trigger selection) → `ConfirmDeleteDialog` (title `Delete «{dotted}»?`, description "Only an empty namespace can be deleted; if it still has tables or child namespaces the server will reject it.") → `useDeleteNamespace(ws, wid).mutate(dotted)`. Surface a 409 "not empty" message (toast or inline alert). Reset the right-panel selection if the deleted namespace was selected.

- [ ] **Step 4: Tests** (extend `CatalogBrowse.test.tsx`, mock `@/api/catalog`): the New-namespace dialog calls `useCreateNamespace` with `{parent: null, name}` (root) and `{parent: "sales", name}` (nested); a row Delete → confirm → `useDeleteNamespace` with the dotted name; a 409 error message is shown. Keep the existing browse tests green.

- [ ] **Step 5: Gate** — `cd frontend && npm run lint && npm run typecheck && npm test && npm run build` — all green.

- [ ] **Step 6: Commit** `feat(web): create + drop namespaces in the catalog Browse tab`.

---

## Self-review (controller, before final review)
- [ ] Create builds the ident from `parent` + a dotless `name`; dup → 409; dot-in-name → 400; admin-gated; `{wid}` ownership-checked (cross-ws → 404).
- [ ] Drop is empty-only; non-empty → 409; admin-gated; IDOR-safe.
- [ ] FE: New-namespace dialog (root + parent), per-row Delete with the not-empty warning + 409 surfaced; list refetches.
- [ ] **Before merge:** full local CI (fmt/clippy/`cargo test --workspace`, FE lint/typecheck/test/build) + live smoke (open a warehouse, create a namespace + a nested one, drop the empty leaf, confirm dropping a non-empty parent is rejected). NOTE: 2 pre-existing `egret-engine` job tests fail on a fresh `egret_test` (unrelated; see the memory note) — don't treat them as a regression.

## Out of scope (later)
Namespace properties edit; rename; table/view DDL via UI; a hierarchical tree view.
