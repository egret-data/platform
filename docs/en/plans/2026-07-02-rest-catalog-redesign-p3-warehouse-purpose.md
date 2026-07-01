# REST Catalog Redesign — Plan 3: explicit warehouse purpose Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> Spec: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md` (Workstream **E**). Plan 3 of 6 (A → B → D → **E** → C → F). Self-contained and testable.
> VI reference: `docs/vi/plans/2026-07-02-rest-catalog-redesign-p3-warehouse-purpose.md`.

**Goal:** Replace the fragile "warehouse named `notebook` (else first)" selection with an explicit `purpose` on each warehouse, so the notebook catalog is chosen deterministically and an admin designates it on creation.

**Architecture:** Add a `purpose` column (`shared` | `notebook`, default `shared`) with a partial-unique index (≤1 `notebook` per workspace) + backfill; add a `WarehousePurpose` enum and a `insert_warehouse_with_purpose` store method (existing `insert_warehouse` delegates with `Shared` — zero churn to callers); select the notebook warehouse by purpose with no first-fallback; accept `purpose` in the create-warehouse API + UI.

**Tech Stack:** Postgres migration (sqlx embedded migrator), Rust (`egret-catalog`, `egret-notebook`, `egret-catalog-engine`), React/TS (Catalog UI).

---

## Background the implementer needs

- Migrations live at repo-root `migrations/`, embedded by `egret-metastore`. Latest is `0047_drop_notebook_sql_target.sql` → next is `0048`. **After adding a migration, `touch crates/egret-metastore/src/lib.rs`** so `serve`'s embedded migrator re-embeds it (known staleness gotcha).
- `workspace_warehouses` (migration 0034): `id, workspace_id, warehouse_id, name, storage_conn_id, created_at`, `UNIQUE (workspace_id, name)`.
- `crates/egret-catalog/src/service/warehouse.rs`: `WorkspaceWarehouse { id, warehouse_id, name, storage_conn_id, created_at }`, `WarehouseRow` (sqlx::FromRow), `insert_warehouse(workspace_id, warehouse_id, name, storage_conn_id)`, `list_warehouses`, `get_warehouse`. The storage-connection code (`service/storage_conn.rs`) already has a `purpose: Option<String>` column — mirror its style.
- `insert_warehouse` has ~25 callers (mostly tests passing `..., None`); do NOT change its signature.
- `crates/egret-notebook/src/runtime.rs`: `const NOTEBOOK_WAREHOUSE_NAME = "notebook"`, `select_notebook_warehouse<W,F>(warehouses, name_of)` (matches the name, `.unwrap_or(0)` fallback-to-first), used in `build_notebook_runtime` as `select_notebook_warehouse(warehouses, |w| w.name.as_str())`. Three unit tests: picks `notebook`, falls back to first (`shared`), empty → None.
- `crates/egret-catalog-engine/src/browse.rs`: `CreateWarehouseRequest { name, storage_conn_id }`, handler `create_warehouse` (calls `create_warehouse_on` then `st.store.insert_warehouse(ws, lk_id, &req.name, Some(req.storage_conn_id))`).
- UI: `frontend/src/routes/explore/catalog/WarehousesList.tsx` (list + "Add warehouse" dialog).

---

## Task 1: Migration — add `purpose`, backfill, enforce ≤1 notebook

**Files:**
- Create: `migrations/0048_workspace_warehouse_purpose.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (touch to re-embed)

- [ ] **Step 1: Write the migration**

```sql
-- Catalog redesign (Workstream E): explicit warehouse purpose.
-- `notebook` = the Daft-direct read-write catalog (at most one per workspace);
-- `shared`   = engine-governed warehouses (Trino/StarRocks), zero or more.
ALTER TABLE workspace_warehouses
    ADD COLUMN purpose text NOT NULL DEFAULT 'shared'
    CHECK (purpose IN ('shared', 'notebook'));

-- Backfill: a warehouse literally named 'notebook' becomes the notebook catalog.
UPDATE workspace_warehouses SET purpose = 'notebook' WHERE name = 'notebook';

-- At most one notebook-purpose warehouse per workspace.
CREATE UNIQUE INDEX workspace_warehouses_one_notebook_per_ws
    ON workspace_warehouses (workspace_id)
    WHERE purpose = 'notebook';
```

- [ ] **Step 2: Force the embedded migrator to pick it up**

Run: `touch crates/egret-metastore/src/lib.rs`

- [ ] **Step 3: Apply + verify against a dev DB**

Run: `cargo run -q -p egret-bin -- migrate` (or the project's migrate entrypoint)
Then: `docker exec egret-dev-postgres-1 psql -U egret -d egret -c "\d workspace_warehouses"`
Expected: `purpose` column present with the check; the partial unique index listed.

- [ ] **Step 4: Commit**

```bash
git add migrations/0048_workspace_warehouse_purpose.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(catalog): add workspace_warehouses.purpose (shared|notebook, <=1 notebook/ws)"
```

---

## Task 2: `WarehousePurpose` enum + purpose in the store

**Files:**
- Modify: `crates/egret-catalog/src/service/warehouse.rs`
- Test: `crates/egret-catalog/tests/warehouse_store.rs`

- [ ] **Step 1: Write the failing store test**

In `warehouse_store.rs`:

```rust
#[tokio::test]
async fn insert_with_purpose_roundtrips_and_lists() {
    let (s, ws) = fixture().await; // existing helper that yields a CatalogStore + workspace id
    let wh = s
        .insert_warehouse_with_purpose(ws, Uuid::new_v4(), "nb", None, WarehousePurpose::Notebook)
        .await
        .unwrap();
    assert_eq!(wh.purpose, WarehousePurpose::Notebook);
    // the back-compat helper defaults to Shared
    let sh = s.insert_warehouse(ws, Uuid::new_v4(), "sh", None).await.unwrap();
    assert_eq!(sh.purpose, WarehousePurpose::Shared);
    let listed = s.list_warehouses(ws).await.unwrap();
    assert!(listed.iter().any(|w| w.name == "nb" && w.purpose == WarehousePurpose::Notebook));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-catalog insert_with_purpose_roundtrips_and_lists`
Expected: FAIL (no `WarehousePurpose`, no `_with_purpose`, no `.purpose` field).

- [ ] **Step 3: Add the enum**

In `warehouse.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WarehousePurpose {
    Shared,
    Notebook,
}

impl WarehousePurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            WarehousePurpose::Shared => "shared",
            WarehousePurpose::Notebook => "notebook",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "notebook" => WarehousePurpose::Notebook,
            _ => WarehousePurpose::Shared, // default-safe
        }
    }
}
```

- [ ] **Step 4: Add `purpose` to the model + row, and the store methods**

```rust
// WorkspaceWarehouse: add field
pub purpose: WarehousePurpose,

// WarehouseRow: add
purpose: String,

// From<WarehouseRow>: add
purpose: WarehousePurpose::parse(&r.purpose),
```

Add the real insert and make `insert_warehouse` delegate:

```rust
pub async fn insert_warehouse_with_purpose(
    &self,
    workspace_id: Uuid,
    warehouse_id: Uuid,
    name: &str,
    storage_conn_id: Option<Uuid>,
    purpose: WarehousePurpose,
) -> Result<WorkspaceWarehouse, CatalogError> {
    let row: WarehouseRow = sqlx::query_as::<_, WarehouseRow>(
        "INSERT INTO workspace_warehouses \
         (workspace_id, warehouse_id, name, storage_conn_id, purpose) \
         VALUES ($1, $2, $3, $4, $5) \
         RETURNING id, warehouse_id, name, storage_conn_id, created_at, purpose",
    )
    .bind(workspace_id)
    .bind(warehouse_id)
    .bind(name)
    .bind(storage_conn_id)
    .bind(purpose.as_str())
    .fetch_one(self.pool())
    .await
    .map_err(|e| map_unique_violation(e, name))?;
    Ok(row.into())
}

pub async fn insert_warehouse(
    &self,
    workspace_id: Uuid,
    warehouse_id: Uuid,
    name: &str,
    storage_conn_id: Option<Uuid>,
) -> Result<WorkspaceWarehouse, CatalogError> {
    self.insert_warehouse_with_purpose(workspace_id, warehouse_id, name, storage_conn_id, WarehousePurpose::Shared)
        .await
}
```

Add `, purpose` to the SELECT column lists in `list_warehouses` and `get_warehouse` (and any other `WarehouseRow` SELECT).

- [ ] **Step 5: Run to verify pass**

Run: `DATABASE_URL=... cargo test -p egret-catalog insert_with_purpose_roundtrips_and_lists`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-catalog/src/service/warehouse.rs crates/egret-catalog/tests/warehouse_store.rs
git commit -m "feat(catalog): WarehousePurpose enum + purpose-aware warehouse store"
```

---

## Task 3: Select the notebook warehouse by purpose (no fallback)

**Files:**
- Modify: `crates/egret-notebook/src/runtime.rs`

- [ ] **Step 1: Update the three unit tests to purpose semantics**

In `runtime.rs` tests, the accessor now returns a purpose; there is no fallback:

```rust
#[test]
fn selects_the_notebook_purpose_warehouse() {
    let whs = vec!["shared", "notebook", "shared"];
    assert_eq!(select_notebook_warehouse(whs, |s| *s), Some("notebook"));
}
#[test]
fn no_notebook_purpose_returns_none_not_first() {
    let whs = vec!["shared", "shared"];
    assert_eq!(select_notebook_warehouse(whs, |s| *s), None); // NO fallback-to-first
}
#[test]
fn empty_returns_none() {
    let whs: Vec<&str> = vec![];
    assert_eq!(select_notebook_warehouse(whs, |s| *s), None);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p egret-notebook --lib no_notebook_purpose_returns_none_not_first`
Expected: FAIL (current code falls back to first → returns `Some("shared")`).

- [ ] **Step 3: Rewrite the selector by purpose**

```rust
/// Purpose value marking the notebook read-write catalog.
const NOTEBOOK_WAREHOUSE_PURPOSE: &str = "notebook";

/// Select the workspace's notebook warehouse by purpose. Returns `None` when no
/// warehouse is designated `notebook` (NO fallback to the first warehouse).
pub(crate) fn select_notebook_warehouse<W, F>(warehouses: Vec<W>, purpose_of: F) -> Option<W>
where
    F: Fn(&W) -> &str,
{
    let idx = warehouses
        .iter()
        .position(|w| purpose_of(w) == NOTEBOOK_WAREHOUSE_PURPOSE)?;
    warehouses.into_iter().nth(idx)
}
```
Remove the old `NOTEBOOK_WAREHOUSE_NAME` const.

- [ ] **Step 4: Use purpose in `build_notebook_runtime` with a clear error**

```rust
let warehouse =
    select_notebook_warehouse(warehouses, |w| w.purpose.as_str()).ok_or_else(|| {
        NotebookApiError(egret_core::Error::NotFound(
            "this workspace has no notebook warehouse; an admin must create one \
             (Catalog → Add warehouse → purpose: notebook)".into(),
        ))
    })?;
```

- [ ] **Step 5: Run to verify pass**

Run: `cargo test -p egret-notebook --lib`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-notebook/src/runtime.rs
git commit -m "feat(notebook): select notebook warehouse by purpose, no first-fallback"
```

---

## Task 4: Accept `purpose` in the create-warehouse API + guard ≤1 notebook

**Files:**
- Modify: `crates/egret-catalog-engine/src/browse.rs`
- Test: `crates/egret-catalog-engine/tests/warehouse_api.rs`

- [ ] **Step 1: Write the failing API test**

In `warehouse_api.rs`, add a test that creates a `purpose: "notebook"` warehouse, then a second notebook create returns a 4xx conflict:

```rust
#[tokio::test]
async fn second_notebook_warehouse_is_rejected() {
    // ... set up app + a storage connection `conn_id` (reuse existing helpers) ...
    let ok = create_warehouse_req(&app, &ws, "nb", conn_id, "notebook").await;
    assert_eq!(ok.status(), 200);
    let dup = create_warehouse_req(&app, &ws, "nb2", conn_id, "notebook").await;
    assert!(dup.status().is_client_error(), "second notebook warehouse must be rejected");
}
```
(If the harness lacks a `create_warehouse_req(purpose)` helper, extend the existing warehouse-create test helper to send the `purpose` field.)

- [ ] **Step 2: Run to verify failure**

Run: `DATABASE_URL=... cargo test -p egret-catalog-engine --test warehouse_api second_notebook_warehouse_is_rejected`
Expected: FAIL (request rejects `purpose` field / no guard).

- [ ] **Step 3: Add `purpose` to the request + the create path**

```rust
#[derive(Deserialize)]
struct CreateWarehouseRequest {
    name: String,
    storage_conn_id: Uuid,
    #[serde(default)]
    purpose: Option<String>, // "shared" (default) | "notebook"
}
```
In `create_warehouse`, parse the purpose and, when `notebook`, pre-check uniqueness before touching lakekeeper:

```rust
let purpose = egret_catalog::WarehousePurpose::parse(req.purpose.as_deref().unwrap_or("shared"));
if purpose == egret_catalog::WarehousePurpose::Notebook {
    let existing = st.store.list_warehouses(ws).await.map_err(map_catalog_err)?;
    if existing.iter().any(|w| w.purpose == egret_catalog::WarehousePurpose::Notebook) {
        return Err(BrowseApiError::Conflict(
            "this workspace already has a notebook warehouse".to_string(),
        ));
    }
}
```
Then replace the persist call with the purpose-aware variant:

```rust
.insert_warehouse_with_purpose(ws, lk_id, &req.name, Some(req.storage_conn_id), purpose)
```
(The partial unique index is the hard backstop; this pre-check gives a friendly message and avoids orphaning a lakekeeper warehouse. `map_catalog_err` is the existing `CatalogError → BrowseApiError` mapper used elsewhere in the file; reuse it.)

- [ ] **Step 4: Expose `purpose` on the warehouse-list DTO**

Wherever `list_warehouses` results are serialized to the UI (the warehouse list handler in `browse.rs`), add `purpose: w.purpose.as_str()` to the response DTO.

- [ ] **Step 5: Run to verify pass**

Run: `DATABASE_URL=... cargo test -p egret-catalog-engine --test warehouse_api`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-catalog-engine/src/browse.rs crates/egret-catalog-engine/tests/warehouse_api.rs
git commit -m "feat(catalog): create-warehouse accepts purpose; reject a second notebook warehouse"
```

---

## Task 5: Catalog UI — purpose selector + column

**Files:**
- Modify: `frontend/src/routes/explore/catalog/WarehousesList.tsx`
- Modify: the warehouse API client + type (search `frontend/src/api` for the warehouse create/list types)

- [ ] **Step 1: Add `purpose` to the warehouse type + create payload**

In the warehouse API types, add `purpose: "shared" | "notebook"` to the list item type and the create request. Default the create form to `"shared"`.

- [ ] **Step 2: Add a purpose select to the "Add warehouse" dialog**

In `WarehousesList.tsx`, add a `<Select>` (Shared / Notebook) alongside Name + Storage connection; include `purpose` in the create mutation body. Show a hint: "Notebook = the read-write catalog for notebooks (one per workspace)."

- [ ] **Step 3: Show purpose in the warehouse list**

Add a "Purpose" column/badge to the table so admins see which warehouse is the notebook catalog.

- [ ] **Step 4: Typecheck + lint + build**

Run: `cd frontend && npm run typecheck && npm run lint && npm run build`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/catalog/WarehousesList.tsx frontend/src/api
git commit -m "feat(ui): warehouse purpose selector + column in Catalog"
```

---

## Task 6: Workspace verification

- [ ] **Step 1: Backend fmt + clippy + tests**

Run:
```bash
cargo fmt --all --check
cargo clippy -p egret-catalog -p egret-notebook -p egret-catalog-engine --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-catalog -p egret-notebook
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-catalog-engine --test warehouse_api
```
Expected: all pass. Existing `insert_warehouse` callers (tests) still compile (signature unchanged; delegates to Shared).

- [ ] **Step 2: Frontend checks** — `cd frontend && npm run typecheck && npm run lint && npm run build` (clean).

---

## Self-review notes (spec coverage for Plan 3)

- Spec **E** (purpose column + ≤1 notebook + backfill): Task 1. (enum + store: Task 2.)
- Spec **E** (select by purpose, no fallback, clear error): Task 3.
- Spec **E** (admin designates purpose at creation): Task 4 (API) + Task 5 (UI).
- Engine registration of the two catalogs in Trino/StarRocks is an ops step, out of scope per the spec (documented there).
- Zero-churn choice: `insert_warehouse` keeps its signature and delegates to `insert_warehouse_with_purpose(.., Shared)`, so the ~25 existing callers are untouched.
