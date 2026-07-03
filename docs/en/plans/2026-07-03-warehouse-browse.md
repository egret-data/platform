# Warehouse Browse + grant picker — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-03-warehouse-browse-design.md`. Branch: `feat/warehouse-browse`.

**Goal:** Platform admins browse an embedded-catalog warehouse (namespaces → tables → schema) from the control-plane Warehouse detail page, and pick a namespace/table from that browse when adding a grant.

**Architecture:** In-process — a `browse` module in `egret-catalog-engine` calls lakekeeper's Iceberg v1 services on the `ApiContext` with a system principal; a `WarehouseBrowser` trait (in `egret-controlplane`, impl'd in `egret-bin` like `WarehouseProvisioner`) fronts three platform-admin routes; the React Warehouse detail page gains a Browse tab and a browse-fed grant picker.

**Tech Stack:** Rust (axum, lakekeeper git dep), React 18 + shadcn + TanStack Query, Vitest.

---

## Background the implementer needs

- **The old feature to adapt:** PR #90 shipped `crates/egret-catalog-engine/src/browse.rs` (145+ lines) + `frontend/src/routes/explore/catalog/CatalogAccess.tsx`; EB-1 stripped them. Read the old code for reference: `git show a49b464:crates/egret-catalog-engine/src/browse.rs` and `git show a49b464:frontend/src/routes/explore/catalog/CatalogAccess.tsx`. **Do NOT copy the workspace-scoping, create/drop, or task bits — only the lakekeeper read calls.**
- **The provisioning pattern to mirror** (same shape as this feature): `crates/egret-catalog-engine/src/warehouse.rs` (`create_lakekeeper_warehouse`), `crates/egret-controlplane/src/warehouses.rs` (`WarehouseProvisioner` trait + `provision_lakekeeper`), `crates/egret-bin/src/main.rs` (`LakekeeperProvisioner`).
- **Engine types:** `EngineApiServer = lakekeeper::api::management::v1::ApiServer<PostgresBackend, AllowAllAuthorizer, SecretsState>` (see `warehouse.rs`). The Iceberg v1 services are implemented for it: `NamespaceService`, `TablesService`, `ViewService` (from `lakekeeper::api::iceberg::v1::{namespace,tables,views}`). `EngineState` + `ApiContext` are re-exported from `egret_catalog_engine`.
- **System principal:** `crate::bootstrap::system_request_metadata(system_user)` builds a `RequestMetadata`; it is **consumed per call**, so build a fresh one for each service call.
- **Warehouse prefix:** the browser takes `lakekeeper_warehouse_id` (a string UUID). Wrap it as `Prefix` — `lakekeeper::api::iceberg::types::Prefix::from(...)` (see how the old browse built `r.prefix`; the value is the warehouse UUID string).
- **Provisioned check:** `egret_catalog::service::Warehouse.lakekeeper_warehouse_id: Option<String>`. `None` → not provisioned.
- **Run backend tests:** `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-controlplane`. **Run frontend tests:** `cd frontend && npx vitest run <file>`.

---

## Task 1: `WarehouseBrowser` trait + DTOs + `CpState.browser`

**Files:**
- Modify: `crates/egret-controlplane/src/warehouses.rs` (add near `WarehouseProvisioner`)
- Modify: `crates/egret-controlplane/src/lib.rs` (add `browser` field + re-export)
- Modify: `crates/egret-controlplane/tests/routes_api.rs` (fixture: `browser: None`)
- Modify: `crates/egret-bin/src/main.rs` (CpState construction: `browser: None` for now)

- [ ] **Step 1: Add the trait + DTOs** in `warehouses.rs` (after the `WarehouseProvisioner` block):

```rust
/// Catalog browse result rows (control-plane DTOs; camelCase on the wire).
pub struct BrowseNamespace {
    pub parts: Vec<String>,
}
pub struct BrowseTabular {
    pub name: String,
    pub kind: String, // "table" | "view"
}
pub struct BrowseColumn {
    pub name: String,
    pub type_: String,
    pub required: bool,
}
pub struct BrowseSchema {
    pub columns: Vec<BrowseColumn>,
    pub partition_by: Vec<String>,
    pub sort_by: Vec<String>,
}

/// Reads catalog structure (namespaces/tables/schema) from the embedded
/// lakekeeper. Implemented in egret-bin over the lakekeeper `ApiContext`
/// (like `WarehouseProvisioner`). All args key by the lakekeeper warehouse id.
#[async_trait::async_trait]
pub trait WarehouseBrowser: Send + Sync {
    async fn namespaces(&self, lakekeeper_warehouse_id: &str) -> Result<Vec<BrowseNamespace>, String>;
    async fn tabulars(
        &self,
        lakekeeper_warehouse_id: &str,
        namespace: &[String],
    ) -> Result<Vec<BrowseTabular>, String>;
    async fn schema(
        &self,
        lakekeeper_warehouse_id: &str,
        namespace: &[String],
        table: &str,
    ) -> Result<BrowseSchema, String>;
}
```

- [ ] **Step 2: Add the field to `CpState`** in `lib.rs` (next to `provisioner`), and re-export the trait + DTOs:

```rust
// in the `pub use warehouses::{...}` line, add:
pub use warehouses::{
    BrowseNamespace, BrowseSchema, BrowseTabular, ProvisionS3, WarehouseBrowser, WarehouseProvisioner,
};
// in `pub struct CpState { ... }`, after `provisioner`:
    /// Reads catalog structure from the embedded lakekeeper (wired in egret-bin);
    /// `None` in tests.
    pub browser: Option<Arc<dyn WarehouseBrowser>>,
```

- [ ] **Step 3: Fix the two `CpState` constructions** so the workspace still compiles:
  - `crates/egret-controlplane/tests/routes_api.rs` `setup()` — add `browser: None,` to the `CpState { ... }`.
  - `crates/egret-bin/src/main.rs` control-plane block — add `browser: None,` to `CpState { ... }` (Task 4 replaces it).

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p egret-controlplane -p egret-bin`
Expected: `Finished`.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-controlplane/src/warehouses.rs crates/egret-controlplane/src/lib.rs crates/egret-controlplane/tests/routes_api.rs crates/egret-bin/src/main.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(catalog): WarehouseBrowser trait + CpState.browser"
```

---

## Task 2: Browse routes + route tests (fake browser)

**Files:**
- Modify: `crates/egret-controlplane/src/warehouses.rs` (routes + handlers + DTO serialization)
- Modify: `crates/egret-controlplane/tests/routes_api.rs` (a `FakeBrowser` + tests)

- [ ] **Step 1: Write the failing test** in `tests/routes_api.rs`. Add a `FakeBrowser` and a test that seeds a provisioned warehouse, sets `browser: Some(Arc::new(FakeBrowser))`, and asserts the three routes:

```rust
struct FakeBrowser;
#[async_trait::async_trait]
impl egret_controlplane::WarehouseBrowser for FakeBrowser {
    async fn namespaces(&self, _wh: &str) -> Result<Vec<egret_controlplane::BrowseNamespace>, String> {
        Ok(vec![egret_controlplane::BrowseNamespace { parts: vec!["analytics".into()] }])
    }
    async fn tabulars(&self, _wh: &str, _ns: &[String]) -> Result<Vec<egret_controlplane::BrowseTabular>, String> {
        Ok(vec![egret_controlplane::BrowseTabular { name: "events".into(), kind: "table".into() }])
    }
    async fn schema(&self, _wh: &str, _ns: &[String], _t: &str) -> Result<egret_controlplane::BrowseSchema, String> {
        Ok(egret_controlplane::BrowseSchema {
            columns: vec![egret_controlplane::BrowseColumn { name: "id".into(), type_: "long".into(), required: true }],
            partition_by: vec![], sort_by: vec![],
        })
    }
}

#[tokio::test]
async fn browse_namespaces_tables_schema() {
    let Some(f) = setup_with_browser(Some(std::sync::Arc::new(FakeBrowser))).await else {
        eprintln!("skip: DATABASE_URL not set"); return;
    };
    let admin = session_for(&f, "admin", true).await;
    // Seed a provisioned warehouse (workspace + storage + warehouse row with lakekeeper id).
    let wh = seed_provisioned_warehouse(&f).await; // helper: returns the warehouse id (Uuid)

    let (s, j) = into_json(f.app.clone().oneshot(req("GET", &format!("/warehouses/{wh}/namespaces"), &admin, None)).await.unwrap()).await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j[0]["parts"][0], serde_json::json!("analytics"));

    let (s, j) = into_json(f.app.clone().oneshot(req("GET", &format!("/warehouses/{wh}/namespaces/analytics/tables"), &admin, None)).await.unwrap()).await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j[0]["name"], serde_json::json!("events"));
    assert_eq!(j[0]["kind"], serde_json::json!("table"));

    let (s, j) = into_json(f.app.clone().oneshot(req("GET", &format!("/warehouses/{wh}/namespaces/analytics/tables/events"), &admin, None)).await.unwrap()).await;
    assert_eq!(s, StatusCode::OK, "{j}");
    assert_eq!(j["columns"][0]["name"], serde_json::json!("id"));
    assert_eq!(j["columns"][0]["type"], serde_json::json!("long"));
}

#[tokio::test]
async fn browse_non_admin_forbidden() {
    let Some(f) = setup_with_browser(Some(std::sync::Arc::new(FakeBrowser))).await else { return };
    let plain = session_for(&f, "plain", false).await;
    let wh = seed_provisioned_warehouse(&f).await;
    let resp = f.app.clone().oneshot(req("GET", &format!("/warehouses/{wh}/namespaces"), &plain, None)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn browse_unprovisioned_warehouse_empty() {
    let Some(f) = setup_with_browser(Some(std::sync::Arc::new(FakeBrowser))).await else { return };
    let admin = session_for(&f, "admin", true).await;
    let wh = seed_unprovisioned_warehouse(&f).await; // warehouse row, lakekeeper_warehouse_id = NULL
    let (s, j) = into_json(f.app.clone().oneshot(req("GET", &format!("/warehouses/{wh}/namespaces"), &admin, None)).await.unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(j, serde_json::json!([]));
}
```

Refactor `setup()` into `setup_with_browser(browser)`; add DB helpers `seed_provisioned_warehouse` (insert workspace + storage_connection + a `warehouses` row and `set_lakekeeper_warehouse_id(id, "wh-123")`) and `seed_unprovisioned_warehouse` (same without the lakekeeper id). Use `egret_catalog::CatalogStore` methods (`create_warehouse`, `set_lakekeeper_warehouse_id`) against `f.db`.

- [ ] **Step 2: Run it — fails** (routes don't exist → 404): `DATABASE_URL=… cargo test -p egret-controlplane browse_ -- --nocapture` → FAIL.

- [ ] **Step 3: Implement the routes + serialization** in `warehouses.rs`. Add `#[derive(Serialize)] #[serde(rename_all = "camelCase")]` DTOs that serialize the trait DTOs (`type_` → `type`), the three handlers, and register the routes:

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NamespaceDto { parts: Vec<String> }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TabularDto { name: String, kind: String }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ColumnDto { name: String, r#type: String, required: bool }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SchemaDto { columns: Vec<ColumnDto>, partition_by: Vec<String>, sort_by: Vec<String> }

fn parse_ns(ns: &str) -> Vec<String> { ns.split('.').map(str::to_string).collect() }

/// Resolve the warehouse and its lakekeeper id; `Ok(None)` = not provisioned.
async fn lk_id(st: &CpState, id: Uuid) -> Result<Option<String>, CpError> {
    Ok(st.storage.get_warehouse(id).await?.and_then(|w| w.lakekeeper_warehouse_id))
}

async fn browse_namespaces(
    State(st): State<CpState>, Auth(p): Auth, Path(id): Path<Uuid>,
) -> Result<Json<Vec<NamespaceDto>>, CpError> {
    require_platform_admin(&p)?;
    let (Some(lk), Some(b)) = (lk_id(&st, id).await?, st.browser.as_ref()) else {
        return Ok(Json(vec![])); // unprovisioned or no browser
    };
    let ns = b.namespaces(&lk).await.map_err(|e| CpError::BadRequest(format!("browse: {e}")))?;
    Ok(Json(ns.into_iter().map(|n| NamespaceDto { parts: n.parts }).collect()))
}

async fn browse_tables(
    State(st): State<CpState>, Auth(p): Auth, Path((id, ns)): Path<(Uuid, String)>,
) -> Result<Json<Vec<TabularDto>>, CpError> {
    require_platform_admin(&p)?;
    let (Some(lk), Some(b)) = (lk_id(&st, id).await?, st.browser.as_ref()) else {
        return Ok(Json(vec![]));
    };
    let t = b.tabulars(&lk, &parse_ns(&ns)).await.map_err(|e| CpError::BadRequest(format!("browse: {e}")))?;
    Ok(Json(t.into_iter().map(|x| TabularDto { name: x.name, kind: x.kind }).collect()))
}

async fn browse_schema(
    State(st): State<CpState>, Auth(p): Auth, Path((id, ns, table)): Path<(Uuid, String, String)>,
) -> Result<Json<SchemaDto>, CpError> {
    require_platform_admin(&p)?;
    let lk = lk_id(&st, id).await?.ok_or(CpError::NotFound)?;
    let b = st.browser.as_ref().ok_or(CpError::NotFound)?;
    let s = b.schema(&lk, &parse_ns(&ns), &table).await.map_err(|e| CpError::BadRequest(format!("browse: {e}")))?;
    Ok(Json(SchemaDto {
        columns: s.columns.into_iter().map(|c| ColumnDto { name: c.name, r#type: c.type_, required: c.required }).collect(),
        partition_by: s.partition_by, sort_by: s.sort_by,
    }))
}
```

Register in `router()`:

```rust
        .route("/warehouses/{id}/namespaces", get(browse_namespaces))
        .route("/warehouses/{id}/namespaces/{ns}/tables", get(browse_tables))
        .route("/warehouses/{id}/namespaces/{ns}/tables/{table}", get(browse_schema))
```

- [ ] **Step 4: Run tests — pass**: `DATABASE_URL=… cargo test -p egret-controlplane browse_` → 3 passing.

- [ ] **Step 5: Commit** — `feat(catalog): control-plane warehouse browse routes`.

---

## Task 3: Engine browse module (real lakekeeper calls)

**Files:**
- Create: `crates/egret-catalog-engine/src/browse.rs`
- Modify: `crates/egret-catalog-engine/src/lib.rs` (add `pub mod browse;` + re-export)

Adapt the read helpers from `git show a49b464:crates/egret-catalog-engine/src/browse.rs` — **only** the lakekeeper service calls, dropping the axum routing, `BrowseState`, workspace-scoping, create/drop, and task code.

- [ ] **Step 1: Create `browse.rs`** with three `pub async fn`s (signatures from the spec). Structure:

```rust
//! In-process catalog browse: list namespaces/tables and load a table's schema
//! from the embedded lakekeeper, using a system principal (admin sees all).

use anyhow::Context as _;
use lakekeeper::api::iceberg::types::{PageToken, Prefix};
use lakekeeper::api::iceberg::v1::namespace::{ListNamespacesQuery, NamespaceService};
use lakekeeper::api::iceberg::v1::tables::{ListTablesQuery, LoadTableRequest, TableParameters, TablesService};
use lakekeeper::api::iceberg::v1::views::ViewService;
use lakekeeper::api::ApiContext;
use lakekeeper::iceberg::{NamespaceIdent, TableIdent};
use lakekeeper::service::authz::AllowAllAuthorizer;
use lakekeeper_storage_postgres::{PostgresBackend, SecretsState};

use crate::bootstrap::system_request_metadata;
use crate::mount::EngineState;

type EngineApiServer =
    lakekeeper::api::management::v1::ApiServer<PostgresBackend, AllowAllAuthorizer, SecretsState>;

pub struct NamespaceEntry { pub parts: Vec<String> }
pub enum TabularKind { Table, View }
pub struct TabularEntry { pub name: String, pub kind: TabularKind }
pub struct ColumnInfo { pub name: String, pub r#type: String, pub required: bool }
pub struct TableSchema {
    pub columns: Vec<ColumnInfo>,
    pub partition_by: Vec<String>,
    pub sort_by: Vec<String>,
}

fn prefix(warehouse_id: &str) -> Prefix { Prefix::from(warehouse_id.to_string()) }

/// BFS the whole namespace tree (lakekeeper returns one level per call).
pub async fn list_namespaces(
    engine_state: ApiContext<EngineState>, system_user: &str, warehouse_id: &str,
) -> anyhow::Result<Vec<NamespaceEntry>> {
    let px = prefix(warehouse_id);
    let mut out: Vec<NamespaceEntry> = Vec::new();
    let mut frontier: Vec<Option<NamespaceIdent>> = vec![None];
    while let Some(parent) = frontier.pop() {
        let mut page = PageToken::Empty;
        loop {
            let mut query = ListNamespacesQuery { parent: parent.clone(), page_token: page.clone(), page_size: None, return_uuids: false, return_protection_status: false };
            let rm = system_request_metadata(system_user).context("system metadata")?;
            let resp = <EngineApiServer as NamespaceService<EngineState>>::list_namespaces(
                Some(px.clone()), query, engine_state.clone(), rm,
            ).await.map_err(|e| anyhow::anyhow!("list_namespaces: {e:?}"))?;
            for ns in resp.namespaces.iter() {
                let levels = ns.clone().inner();
                frontier.push(Some(NamespaceIdent::from_vec(levels.clone()).map_err(|e| anyhow::anyhow!("{e:?}"))?));
                out.push(NamespaceEntry { parts: levels });
            }
            match resp.next_page_token {
                Some(t) if !t.is_empty() => page = PageToken::Present(t),
                _ => break,
            }
        }
    }
    out.sort_by(|a, b| a.parts.cmp(&b.parts));
    Ok(out)
}
```

> **Note:** `ListNamespacesQuery`'s exact fields may differ in the pinned rev — check the old `ns_query` helper (`git show a49b464:…/browse.rs` around line 495) and the type in `~/.cargo/git/checkouts/lakekeeper-*/7916505/…/api/iceberg/v1/namespace.rs`. Match the real fields; the pattern (paginate + recurse) is what matters.

- [ ] **Step 2: Add `list_tabulars`** — `TablesService::list_tables` + `ViewService::list_views` over the namespace, mapping each to `TabularEntry`. Paginate like the old `list_tables` helper (old browse ~line 1016). Tables get `TabularKind::Table`, views `TabularKind::View`.

- [ ] **Step 3: Add `load_table_schema`** — `TablesService::load_table` then map:

```rust
pub async fn load_table_schema(
    engine_state: ApiContext<EngineState>, system_user: &str, warehouse_id: &str, namespace: &[String], table: &str,
) -> anyhow::Result<TableSchema> {
    let rm = system_request_metadata(system_user).context("system metadata")?;
    let ident = TableIdent { namespace: NamespaceIdent::from_vec(namespace.to_vec()).map_err(|e| anyhow::anyhow!("{e:?}"))?, name: table.to_string() };
    let loaded = <EngineApiServer as TablesService<EngineState>>::load_table(
        TableParameters { prefix: Some(prefix(warehouse_id)), table: ident },
        LoadTableRequest { /* snapshots: all / as the old browse set it */ ..Default::default() },
        DataAccessMode::NotSpecified, // match the old call — check signature
        engine_state, rm,
    ).await.map_err(|e| anyhow::anyhow!("load_table: {e:?}"))?;
    let md = loaded.metadata; // the TableMetadata
    let schema = md.current_schema();
    let columns = schema.as_struct().fields().iter().map(|f| ColumnInfo {
        name: f.name.clone(), r#type: format!("{}", f.field_type), required: f.required,
    }).collect();
    let partition_by = md.default_partition_spec().fields().iter().map(|pf| {
        schema.field_by_id(pf.source_id).map(|c| c.name.clone()).unwrap_or_default()
    }).collect();
    let sort_by = md.default_sort_order().fields().iter().map(|sf| {
        schema.field_by_id(sf.source_id).map(|c| c.name.clone()).unwrap_or_default()
    }).collect();
    Ok(TableSchema { columns, partition_by, sort_by })
}
```

> **Note:** the exact `load_table` signature (`LoadTableRequest` fields, the `DataAccessMode` arg) and the `TableMetadata` accessors (`current_schema`, `default_partition_spec`, `default_sort_order`, `field_by_id`) are pinned-rev-specific. Cross-check the old browse (`git show a49b464:…/browse.rs` lines 1062-1170) and `iceberg` crate types. The mapping shape (columns name/type/required, partition/sort source column names) is fixed.

- [ ] **Step 4: Export** in `lib.rs`: `pub mod browse;` and `pub use browse::{list_namespaces, list_tabulars, load_table_schema, NamespaceEntry, TabularEntry, TabularKind, ColumnInfo, TableSchema};`.

- [ ] **Step 5: Verify it compiles**: `cargo build -p egret-catalog-engine`. Fix signatures against the pinned rev until `Finished`. (No unit test — this layer needs a live lakekeeper; it is verified live in Task 8.)

- [ ] **Step 6: Commit** — `feat(catalog): egret-catalog-engine browse (namespaces/tables/schema)`.

---

## Task 4: Wire `LakekeeperBrowser` in egret-bin

**Files:** Modify `crates/egret-bin/src/main.rs`

- [ ] **Step 1: Add the impl** near `LakekeeperProvisioner`:

```rust
struct LakekeeperBrowser {
    engine_state: egret_catalog_engine::ApiContext<egret_catalog_engine::EngineState>,
    system_user: String,
}

#[async_trait::async_trait]
impl egret_controlplane::WarehouseBrowser for LakekeeperBrowser {
    async fn namespaces(&self, wh: &str) -> Result<Vec<egret_controlplane::BrowseNamespace>, String> {
        egret_catalog_engine::list_namespaces(self.engine_state.clone(), &self.system_user, wh)
            .await.map(|v| v.into_iter().map(|n| egret_controlplane::BrowseNamespace { parts: n.parts }).collect())
            .map_err(|e| e.to_string())
    }
    async fn tabulars(&self, wh: &str, ns: &[String]) -> Result<Vec<egret_controlplane::BrowseTabular>, String> {
        egret_catalog_engine::list_tabulars(self.engine_state.clone(), &self.system_user, wh, ns)
            .await.map(|v| v.into_iter().map(|t| egret_controlplane::BrowseTabular {
                name: t.name,
                kind: match t.kind { egret_catalog_engine::TabularKind::View => "view", _ => "table" }.to_string(),
            }).collect())
            .map_err(|e| e.to_string())
    }
    async fn schema(&self, wh: &str, ns: &[String], table: &str) -> Result<egret_controlplane::BrowseSchema, String> {
        egret_catalog_engine::load_table_schema(self.engine_state.clone(), &self.system_user, wh, ns, table)
            .await.map(|s| egret_controlplane::BrowseSchema {
                columns: s.columns.into_iter().map(|c| egret_controlplane::BrowseColumn { name: c.name, type_: c.r#type, required: c.required }).collect(),
                partition_by: s.partition_by, sort_by: s.sort_by,
            }).map_err(|e| e.to_string())
    }
}
```

- [ ] **Step 2: Construct + wire** it in the catalog-engine block (where `catalog_provisioner` is set), then pass it to `CpState`:

```rust
// inside `if config.catalog.enabled { ... }`, after building the provisioner:
        catalog_browser = Some(std::sync::Arc::new(LakekeeperBrowser {
            engine_state: engine_state.clone(),
            system_user: config.catalog.system_user.clone(),
        }));
```

Declare `let mut catalog_browser: Option<std::sync::Arc<dyn egret_controlplane::WarehouseBrowser>> = None;` next to `catalog_provisioner`, and set `browser: catalog_browser,` in the `CpState { ... }` (replacing the Task 1 `browser: None`).

- [ ] **Step 3: Verify**: `cargo build --workspace` → `Finished`.

- [ ] **Step 4: Commit** — `feat(catalog): wire LakekeeperBrowser into the control plane`.

---

## Task 5: Frontend API client

**Files:** Modify `frontend/src/api/controlPlane.ts`

- [ ] **Step 1: Add types + fns** (after the Grant section):

```ts
// ── Warehouse browse (embedded catalog) ──────────────────────────────────────

export interface Namespace { parts: string[] }
export interface Tabular { name: string; kind: "table" | "view" }
export interface Column { name: string; type: string; required: boolean }
export interface Schema { columns: Column[]; partitionBy: string[]; sortBy: string[] }

export function listNamespaces(warehouseId: string): Promise<Namespace[]> {
  return apiJson<Namespace[]>(`/control-plane/warehouses/${warehouseId}/namespaces`);
}
export function listTabulars(warehouseId: string, namespace: string): Promise<Tabular[]> {
  return apiJson<Tabular[]>(
    `/control-plane/warehouses/${warehouseId}/namespaces/${encodeURIComponent(namespace)}/tables`,
  );
}
export function loadSchema(warehouseId: string, namespace: string, table: string): Promise<Schema> {
  return apiJson<Schema>(
    `/control-plane/warehouses/${warehouseId}/namespaces/${encodeURIComponent(namespace)}/tables/${encodeURIComponent(table)}`,
  );
}
```

- [ ] **Step 2: Typecheck**: `cd frontend && npx tsc --noEmit -p tsconfig.json` → 0 errors.
- [ ] **Step 3: Commit** — `feat(ui): warehouse-browse API client`.

---

## Task 6: Frontend Browse tab

**Files:**
- Modify: `frontend/src/routes/control-plane/WarehouseDetail.tsx`
- Create: `frontend/src/routes/control-plane/__tests__/WarehouseDetail.test.tsx`

- [ ] **Step 1: Failing test** — mock `@/api/controlPlane`, render `WarehouseDetail` in a `MemoryRouter` at `/control-plane/warehouses/w1`, click the "Browse" tab, and assert a namespace, then its table, then a schema column render:

```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { beforeEach, describe, expect, test, vi } from "vitest";
import WarehouseDetail from "@/routes/control-plane/WarehouseDetail";
import * as api from "@/api/controlPlane";
import * as hooks from "@/api/hooks";
vi.mock("@/api/controlPlane");
vi.mock("@/api/hooks");

function renderAt() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/control-plane/warehouses/w1"]}>
        <Routes><Route path="/control-plane/warehouses/:id" element={<WarehouseDetail />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}
describe("WarehouseDetail browse", () => {
  beforeEach(() => {
    vi.mocked(hooks.useMyWorkspaces).mockReturnValue({ data: [] } as ReturnType<typeof hooks.useMyWorkspaces>);
    vi.mocked(api.getWarehouse).mockResolvedValue({ id: "w1", name: "test-wh", workspaceId: "x", storageConnId: "y", lakekeeperWarehouseId: "lk" });
    vi.mocked(api.listRoles).mockResolvedValue([]);
    vi.mocked(api.listClients).mockResolvedValue([]);
    vi.mocked(api.listNamespaces).mockResolvedValue([{ parts: ["analytics"] }]);
    vi.mocked(api.listTabulars).mockResolvedValue([{ name: "events", kind: "table" }]);
    vi.mocked(api.loadSchema).mockResolvedValue({ columns: [{ name: "id", type: "long", required: true }], partitionBy: [], sortBy: [] });
  });
  test("browse namespaces -> tables -> schema", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(await screen.findByRole("tab", { name: /Browse/ }));
    await user.click(await screen.findByText("analytics"));
    await user.click(await screen.findByText("events"));
    expect(await screen.findByText("id")).toBeInTheDocument();
    expect(screen.getByText("long")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run — fails** (no Browse tab): `npx vitest run src/routes/control-plane/__tests__/WarehouseDetail.test.tsx` → FAIL.

- [ ] **Step 3: Implement the Browse tab.** Add a third `<TabsTrigger value="browse">Browse</TabsTrigger>` + `<TabsContent value="browse"><BrowseTab warehouseId={id} /></TabsContent>`. Implement `BrowseTab` as a 3-column master-detail:
  - `useQuery(["cp","browse",id,"ns"], () => listNamespaces(id))`; render each `namespace.parts.join(".")` as a clickable row; `useState` selected namespace.
  - When a namespace is selected: `useQuery([...,"tables", ns], () => listTabulars(id, ns))`; render each `tabular.name` (+ a `kind` `Badge`) clickable; `useState` selected table.
  - When a table is selected: `useQuery([...,"schema", ns, table], () => loadSchema(id, ns, table))`; render a `Table` (Name / Type / Required) + partition-by / sort-by `Badge`s if non-empty.
  - Empty states per the spec. Reuse the `Card`, `Table`, `Badge` imports already in the file.

- [ ] **Step 4: Run — pass**; also `npx tsc --noEmit` clean.
- [ ] **Step 5: Commit** — `feat(ui): warehouse Browse tab (namespaces/tables/schema)`.

---

## Task 7: Frontend grant picker (browse-fed selects)

**Files:**
- Modify: `frontend/src/routes/control-plane/WarehouseDetail.tsx` (`AddGrantDialog`)
- Modify: `frontend/src/routes/control-plane/__tests__/WarehouseDetail.test.tsx` (a picker test)

- [ ] **Step 1: Failing test** — add a test: open a role's Add grant, assert a Namespace select shows "analytics" and "All namespaces", pick namespace → Table select shows "events" and "All tables", pick table, submit, and assert `api.addGrant` was called with `securableType: "table", namespace: ["analytics"], tableName: "events"`. (Mock `api.addGrant`.)

- [ ] **Step 2: Run — fails.**

- [ ] **Step 3: Rewrite `AddGrantDialog`.** The dialog receives the `warehouseId` (thread it from `RoleGrants` → `AddGrantDialog`). Replace the Securable select + the two text inputs with:
  - `const ALL_NS = "__all_ns__"; const ALL_TABLES = "__all_tables__";`
  - Namespace `Select` from `listNamespaces(warehouseId)` values = `parts.join(".")`, plus a `<SelectItem value={ALL_NS}>All namespaces</SelectItem>`. Default `ALL_NS`.
  - Table `Select` from `listTabulars(warehouseId, namespace)` (enabled only when a concrete namespace is chosen) + `<SelectItem value={ALL_TABLES}>All tables</SelectItem>`. Default `ALL_TABLES`.
  - Keep the privilege `Select`.
  - On submit derive: `securableType = table !== ALL_TABLES ? "table" : namespace !== ALL_NS ? "namespace" : "catalog"`; `namespace = namespaceSel !== ALL_NS ? namespaceSel.split(".") : null`; `tableName = table !== ALL_TABLES ? table : null`. Call `addGrant(roleId, { securableType, namespace, tableName, privilege })`.

- [ ] **Step 4: Run — pass**; `npx tsc --noEmit` + `npm run lint` clean.
- [ ] **Step 5: Commit** — `feat(ui): browse-fed namespace/table grant picker`.

---

## Task 8: Live verify + full CI

- [ ] **Step 1: Bring up the stack** (per `docs/en/EMBEDDED-CATALOG-STATUS.md` dev notes): `set -a; . ./.env; set +a; cargo run -p egret-bin -- serve` + `cd frontend && npm run dev`. Log in as admin, open `test-wh` (it has namespace `az_eng` + a pyiceberg table under `demo`). Open the **Browse** tab: confirm namespaces list, a table lists, and clicking it shows the schema columns. Add a table-level grant via the picker; confirm the grant row shows `namespace.table`.

- [ ] **Step 2: Full local CI** (GitHub CI is out of quota — see memory): backend `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` (ignore the pre-existing `mysql_async` warning), `DATABASE_URL=… cargo test --workspace`; frontend `npx tsc --noEmit`, `npm run lint`, `npx vitest run`, `npm run build`.

- [ ] **Step 3: Commit** any fixups — `chore: warehouse-browse fixups`.

---

## Self-review notes (spec coverage)

- Spec **Backend** (browse module + trait + 3 routes + egret-bin impl) → Tasks 1-4. **Frontend** (api client + Browse tab + grant picker) → Tasks 5-7. **Testing** (route tests, frontend tests, manual) → Tasks 2, 6, 7, 8.
- **Auth**: `require_platform_admin` on every route (Task 2). **Not-provisioned** → `[]`/`404` (Task 2). **Admin-sees-all** via system metadata (Task 3).
- **Known pinned-rev risk:** the exact lakekeeper `ListNamespacesQuery`/`load_table` signatures + `TableMetadata` accessors (Task 3) must be matched against `git show a49b464:…/browse.rs` and the checkout — flagged inline in Task 3 as the one place to expect iteration.
