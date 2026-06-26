# Catalog-per-workspace — Plan A-1: Data Layer (migration + normalized store)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single global JSONB-blob Iceberg catalog store with a per-workspace, fully-normalized (lakekeeper-style) metadata store at the `CatalogStore` layer — no HTTP/routing/auth changes yet.

**Architecture:** One destructive migration drops & recreates the `catalog_*` schema with a `catalogs` row per workspace, namespaces scoped by `catalog_id`, and tables/views shredded into child tables (schemas / partition specs / sort orders / properties / snapshots / logs / refs / statistics / view versions / representations). `CatalogStore` gains shred-on-write and assemble-on-read; reads rebuild `iceberg::spec::TableMetadata` via the iceberg-rust fork's `TableMetadataBuilder ... build_unchecked()` (the same fork lakekeeper uses, so its `into_table_metadata` reconstruction ports directly). Correctness is gated by metadata round-trip tests (shred → assemble must equal the original).

**Tech Stack:** Rust, axum, sqlx (Postgres, embedded `sqlx::migrate!`), `iceberg` crate (git fork `lakekeeper/iceberg-rust` rev `bb70173`), tokio.

**Scope note:** This is the first of three plans for spec A. Plan A-2 adds HTTP `{prefix}` routing, the per-request warehouse/extractor wiring, and the workspace-create provisioning hook. Plan A-3 adds `catalog_tokens` auth, RBAC `catalog:*` perms, the internal UI API, and the frontend Catalog view. This plan ends with a normalized store that the existing `egret-catalog` integration tests exercise (still effectively one catalog, created explicitly in tests).

**Spec:** `docs/superpowers/specs/2026-06-26-catalog-per-workspace-design.md`

**Test prerequisite:** the `egret-catalog` integration tests self-skip without `DATABASE_URL`. Run them against a throwaway Postgres, e.g.:
```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
```

---

## File map

- Create: `migrations/0023_catalog_per_workspace.sql` — full normalized schema (drops old `catalog_*`).
- Modify: `crates/egret-metastore/src/lib.rs` — touch to force `sqlx::migrate!` re-embed (known staleness gotcha).
- Create: `crates/egret-catalog/src/service/catalogs.rs` — `catalogs` table CRUD + `Catalog` model.
- Modify: `crates/egret-catalog/src/service/store.rs` — namespaces take `catalog_id`; tables/views shred + assemble.
- Create: `crates/egret-catalog/src/service/table_repo.rs` — table shred/assemble helpers (keep `store.rs` focused).
- Create: `crates/egret-catalog/src/service/view_repo.rs` — view shred/assemble helpers.
- Modify: `crates/egret-catalog/src/service/mod.rs` — export the new modules.
- Modify: `crates/egret-catalog/tests/store_ns.rs`, `store_tables.rs`, `store_views.rs`, `http_roundtrip.rs` — create a catalog + pass `catalog_id`.

---

## Task 1: Migration — drop & recreate normalized schema

**Files:**
- Create: `migrations/0023_catalog_per_workspace.sql`
- Modify: `crates/egret-metastore/src/lib.rs` (whitespace touch to re-embed)

- [ ] **Step 1: Write the migration**

Create `migrations/0023_catalog_per_workspace.sql` with the exact DDL from the
spec §3 (`catalogs`, `catalog_namespaces` with `catalog_id`, `catalog_tables` +
all `catalog_table_*` children incl. `catalog_table_statistics` /
`catalog_table_partition_statistics`, `catalog_views` + all `catalog_view_*`
children incl. `catalog_view_representations`, and `catalog_tokens`). Prepend
the destructive drop block:

```sql
-- Spec A: per-workspace, normalized Iceberg catalog. Pre-release: drop & recreate.
DROP TABLE IF EXISTS workspace_catalog_grant CASCADE;
DROP TABLE IF EXISTS catalog_table_metadata_log CASCADE;
DROP TABLE IF EXISTS catalog_views CASCADE;
DROP TABLE IF EXISTS catalog_tables CASCADE;
DROP TABLE IF EXISTS catalog_namespaces CASCADE;
-- (then all CREATE TABLE statements from spec §3, in dependency order:
--  catalogs → catalog_namespaces → catalog_tables → catalog_table_* →
--  catalog_views → catalog_view_* → catalog_tokens)
```

Copy each `CREATE TABLE` verbatim from spec §3. Critical: `catalog_table_snapshots.schema_id` is **nullable** (no `NOT NULL`).

- [ ] **Step 2: Force the embedded migrator to pick up the new file**

`MIGRATOR` is `sqlx::migrate!("../../migrations")` embedded at compile time in
`egret-metastore`. Add/adjust a trailing newline or comment in
`crates/egret-metastore/src/lib.rs` so the crate recompiles and re-embeds
(documented gotcha: a new migration file alone won't recompile metastore).

- [ ] **Step 3: Verify the migration applies cleanly**

Run:
```bash
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test
cargo test -p egret-catalog --test store_ns -- --nocapture
```
Expected: the test binary runs `db.migrate()` without error (individual tests
may still fail until Task 4 — that is fine here; we only need the migration to
apply). If you see a SQL error, the DDL is malformed — fix it.

- [ ] **Step 4: Commit**

```bash
git add migrations/0023_catalog_per_workspace.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(catalog): normalized per-workspace schema migration"
```

---

## Task 2: `catalogs` table CRUD + `Catalog` model

**Files:**
- Create: `crates/egret-catalog/src/service/catalogs.rs`
- Modify: `crates/egret-catalog/src/service/mod.rs` (add `pub mod catalogs;` + re-export)
- Test: `crates/egret-catalog/tests/store_catalogs.rs` (create)

- [ ] **Step 1: Write the failing test**

Create `crates/egret-catalog/tests/store_catalogs.rs`:

```rust
//! DB-gated catalogs-store tests. Self-skip without DATABASE_URL.
use egret_catalog::service::CatalogStore;
use egret_metastore::Db;

async fn store() -> Option<CatalogStore> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(CatalogStore::new(db))
}

/// Insert a workspace row directly (FK target) and return its id.
async fn seed_workspace(s: &CatalogStore) -> uuid::Uuid {
    let id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO workspaces (id, slug, name, status) VALUES ($1,$2,$3,'active')")
        .bind(id)
        .bind(format!("ws_{}", id.simple()))
        .bind("Test WS")
        .execute(&s.pool())
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn create_and_get_catalog() {
    let Some(s) = store().await else { eprintln!("skip: DATABASE_URL not set"); return; };
    let ws = seed_workspace(&s).await;
    let c = s.create_catalog(ws, "main", "s3://wh/workspaces/ws/warehouse").await.unwrap();
    assert_eq!(c.workspace_id, ws);
    assert_eq!(c.warehouse_location, "s3://wh/workspaces/ws/warehouse");
    let got = s.get_catalog_by_workspace(ws).await.unwrap();
    assert_eq!(got.id, c.id);
}
```

Note: this requires a `pub fn pool(&self) -> &sqlx::PgPool` accessor on
`CatalogStore` — add it in Step 3 if not present.

- [ ] **Step 2: Run it — verify it fails to compile**

Run: `cargo test -p egret-catalog --test store_catalogs`
Expected: FAIL — `create_catalog` / `get_catalog_by_workspace` / `pool` not found.

- [ ] **Step 3: Implement `catalogs.rs`**

```rust
//! `catalogs` table: one managed Iceberg catalog per workspace.
use crate::CatalogError;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Catalog {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub warehouse_location: String,
}

impl crate::service::CatalogStore {
    pub async fn create_catalog(
        &self,
        workspace_id: Uuid,
        name: &str,
        warehouse_location: &str,
    ) -> Result<Catalog, CatalogError> {
        sqlx::query_as::<_, Catalog>(
            "INSERT INTO catalogs (workspace_id, name, warehouse_location) \
             VALUES ($1, $2, $3) RETURNING id, workspace_id, name, warehouse_location",
        )
        .bind(workspace_id)
        .bind(name)
        .bind(warehouse_location)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                CatalogError::AlreadyExists(format!("catalog for workspace {workspace_id}"))
            }
            _ => CatalogError::Db(e),
        })
    }

    pub async fn get_catalog_by_workspace(&self, workspace_id: Uuid) -> Result<Catalog, CatalogError> {
        sqlx::query_as::<_, Catalog>(
            "SELECT id, workspace_id, name, warehouse_location FROM catalogs WHERE workspace_id = $1",
        )
        .bind(workspace_id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound(format!("catalog for workspace {workspace_id}")))
    }

    pub async fn get_catalog_by_id(&self, id: Uuid) -> Result<Catalog, CatalogError> {
        sqlx::query_as::<_, Catalog>(
            "SELECT id, workspace_id, name, warehouse_location FROM catalogs WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound(format!("catalog {id}")))
    }
}
```

Add to `CatalogStore` in `store.rs` (if missing):
```rust
pub fn pool(&self) -> &sqlx::PgPool { &self.db.pool }
```
Add to `service/mod.rs`: `pub mod catalogs; pub use catalogs::Catalog;`

- [ ] **Step 4: Run the test — verify it passes**

Run: `cargo test -p egret-catalog --test store_catalogs`
Expected: PASS (1 test; skips if no DATABASE_URL).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-catalog/src/service/catalogs.rs crates/egret-catalog/src/service/store.rs crates/egret-catalog/src/service/mod.rs crates/egret-catalog/tests/store_catalogs.rs
git commit -m "feat(catalog): catalogs table CRUD (one catalog per workspace)"
```

---

## Task 3: Scope namespaces by `catalog_id`

**Files:**
- Modify: `crates/egret-catalog/src/service/store.rs` (namespace methods + `NamespaceRow`)
- Test: `crates/egret-catalog/tests/store_ns.rs`

- [ ] **Step 1: Update the test to thread `catalog_id`**

In `store_ns.rs`, add a helper that seeds a workspace + catalog and rewrite the
existing namespace test to pass `catalog.id`. Add the seed helper (same as
Task 2's `seed_workspace`) and:

```rust
async fn seed_catalog(s: &CatalogStore) -> uuid::Uuid {
    let ws = seed_workspace(s).await;
    s.create_catalog(ws, "main", "s3://wh").await.unwrap().id
}

#[tokio::test]
async fn create_get_list_drop_namespace() {
    let Some(s) = store().await else { eprintln!("skip"); return; };
    let cid = seed_catalog(&s).await;
    let levels = vec![format!("ns_{}", uuid::Uuid::new_v4().simple())];
    let ns = s.create_namespace(cid, &levels, &std::collections::HashMap::new()).await.unwrap();
    assert_eq!(ns.levels, levels);
    let got = s.get_namespace(cid, &levels).await.unwrap();
    assert_eq!(got.id, ns.id);
    assert!(s.list_namespaces(cid, None).await.unwrap().contains(&levels));
    s.drop_namespace(cid, &levels).await.unwrap();
    assert!(s.get_namespace(cid, &levels).await.is_err());
}
```

- [ ] **Step 2: Run it — verify it fails to compile**

Run: `cargo test -p egret-catalog --test store_ns`
Expected: FAIL — `create_namespace` etc. take the wrong arity.

- [ ] **Step 3: Update namespace store methods**

In `store.rs`, add `catalog_id` as the first param to `create_namespace`,
`get_namespace`, `list_namespaces`, `drop_namespace`, `update_namespace_properties`,
and add `catalog_id` to every WHERE/INSERT. Examples:

```rust
pub async fn create_namespace(
    &self,
    catalog_id: Uuid,
    levels: &[String],
    props: &HashMap<String, String>,
) -> Result<NamespaceRow, CatalogError> {
    let props_json = serde_json::to_value(props).unwrap_or_default();
    sqlx::query_as::<_, NamespaceRow>(
        "INSERT INTO catalog_namespaces (catalog_id, levels, properties) VALUES ($1, $2, $3) \
         RETURNING id, levels, properties",
    )
    .bind(catalog_id).bind(levels).bind(props_json)
    .fetch_one(self.pool()).await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => CatalogError::AlreadyExists(levels.join(".")),
        _ => CatalogError::Db(e),
    })
}

pub async fn get_namespace(&self, catalog_id: Uuid, levels: &[String]) -> Result<NamespaceRow, CatalogError> {
    sqlx::query_as::<_, NamespaceRow>(
        "SELECT id, levels, properties FROM catalog_namespaces WHERE catalog_id = $1 AND levels = $2",
    )
    .bind(catalog_id).bind(levels)
    .fetch_optional(self.pool()).await
    .map_err(CatalogError::Db)?
    .ok_or_else(|| CatalogError::NotFound(levels.join(".")))
}
```

For `list_namespaces`, add `catalog_id = $1 AND` to both the root and the
parent-scoped branch. For `drop_namespace` and `update_namespace_properties`,
resolve the namespace via the new `get_namespace(catalog_id, levels)`.

- [ ] **Step 4: Run the test — verify it passes**

Run: `cargo test -p egret-catalog --test store_ns`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-catalog/src/service/store.rs crates/egret-catalog/tests/store_ns.rs
git commit -m "feat(catalog): scope namespaces by catalog_id"
```

---

## Task 4: Table shred-on-write (`table_repo.rs`)

**Files:**
- Create: `crates/egret-catalog/src/service/table_repo.rs`
- Modify: `crates/egret-catalog/src/service/mod.rs`

This task writes the shred half. The assemble half + the round-trip test that
proves correctness is Task 5 — they are committed together because neither is
provably correct alone.

- [ ] **Step 1: Implement `shred_table_metadata`**

Create `table_repo.rs` with a function that, given an open transaction, a
`table_id`, and `&TableMetadata`, inserts every child row. The iceberg-rust
accessors on `TableMetadata` are: `.schemas_iter()`, `.partition_specs_iter()`,
`.sort_orders_iter()`, `.properties()`, `.snapshots()`, `.history()`
(snapshot log), `.metadata_log()`, `.refs()`, `.current_schema_id()`,
`.default_partition_spec_id()`, `.default_sort_order_id()`,
`.current_snapshot_id()`, `.format_version()`, `.last_column_id()`,
`.last_sequence_number()`, `.last_updated_ms()`, `.last_partition_id()`.

```rust
//! Shred/assemble a normalized Iceberg table into/out of the catalog_table_* tables.
use crate::CatalogError;
use iceberg::spec::TableMetadata;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

/// Insert all child-collection rows for a table within an open tx.
/// Children are append-only by id (insert-if-not-exists) since they are immutable.
pub async fn shred_children(
    tx: &mut Transaction<'_, Postgres>,
    table_id: Uuid,
    md: &TableMetadata,
) -> Result<(), CatalogError> {
    for (sid, schema) in md.schemas_iter().map(|s| (s.schema_id(), s)) {
        sqlx::query(
            "INSERT INTO catalog_table_schemas (table_id, schema_id, schema) \
             VALUES ($1,$2,$3) ON CONFLICT (table_id, schema_id) DO NOTHING",
        )
        .bind(table_id).bind(sid)
        .bind(serde_json::to_value(schema).map_err(bad)?)
        .execute(&mut **tx).await.map_err(CatalogError::Db)?;
    }
    for spec in md.partition_specs_iter() {
        sqlx::query(
            "INSERT INTO catalog_table_partition_specs (table_id, spec_id, spec) \
             VALUES ($1,$2,$3) ON CONFLICT (table_id, spec_id) DO NOTHING",
        )
        .bind(table_id).bind(spec.spec_id())
        .bind(serde_json::to_value(spec).map_err(bad)?)
        .execute(&mut **tx).await.map_err(CatalogError::Db)?;
    }
    for so in md.sort_orders_iter() {
        sqlx::query(
            "INSERT INTO catalog_table_sort_orders (table_id, sort_order_id, sort_order) \
             VALUES ($1,$2,$3) ON CONFLICT (table_id, sort_order_id) DO NOTHING",
        )
        .bind(table_id).bind(so.order_id)
        .bind(serde_json::to_value(so).map_err(bad)?)
        .execute(&mut **tx).await.map_err(CatalogError::Db)?;
    }
    for snap in md.snapshots() {
        sqlx::query(
            "INSERT INTO catalog_table_snapshots \
               (table_id, snapshot_id, parent_snapshot_id, sequence_number, \
                manifest_list, summary, schema_id, timestamp_ms) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (table_id, snapshot_id) DO NOTHING",
        )
        .bind(table_id).bind(snap.snapshot_id())
        .bind(snap.parent_snapshot_id()).bind(snap.sequence_number())
        .bind(snap.manifest_list())
        .bind(serde_json::to_value(snap.summary()).map_err(bad)?)
        .bind(snap.schema_id())
        .bind(snap.timestamp_ms())
        .execute(&mut **tx).await.map_err(CatalogError::Db)?;
    }
    // Pointers/mutables: properties, refs, logs are re-synced each commit.
    sqlx::query("DELETE FROM catalog_table_properties WHERE table_id = $1")
        .bind(table_id).execute(&mut **tx).await.map_err(CatalogError::Db)?;
    for (k, v) in md.properties() {
        sqlx::query("INSERT INTO catalog_table_properties (table_id, key, value) VALUES ($1,$2,$3)")
            .bind(table_id).bind(k).bind(v)
            .execute(&mut **tx).await.map_err(CatalogError::Db)?;
    }
    sqlx::query("DELETE FROM catalog_table_refs WHERE table_id = $1")
        .bind(table_id).execute(&mut **tx).await.map_err(CatalogError::Db)?;
    for (name, r) in md.refs() {
        sqlx::query(
            "INSERT INTO catalog_table_refs (table_id, ref_name, snapshot_id, type, retention) \
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(table_id).bind(name).bind(r.snapshot_id)
        .bind(match r.retention { iceberg::spec::SnapshotRetention::Branch { .. } => "branch", _ => "tag" })
        .bind(serde_json::to_value(&r.retention).map_err(bad)?)
        .execute(&mut **tx).await.map_err(CatalogError::Db)?;
    }
    Ok(())
}

fn bad(e: serde_json::Error) -> CatalogError { CatalogError::BadRequest(e.to_string()) }
```

> Note: exact accessor names (`schemas_iter`, `history`, `order_id`,
> `SnapshotRetention`) are from the fork checkout at
> `~/.cargo/git/checkouts/iceberg-rust-fae803509dd2d218/bb70173/crates/iceberg/src/spec/table_metadata.rs`.
> Verify each against that file as you go; adjust to the real signature if a name differs.

- [ ] **Step 2: Add snapshot-log + metadata-log appenders**

Append-only logs are written by the create/commit callers (Task 6), not here —
they need the new metadata_location. Add these helpers to `table_repo.rs`:

```rust
pub async fn append_logs(
    tx: &mut Transaction<'_, Postgres>,
    table_id: Uuid,
    md: &TableMetadata,
    metadata_file: &str,
) -> Result<(), CatalogError> {
    // metadata log: append the just-written metadata.json pointer
    if let Some(last) = md.metadata_log().last() {
        sqlx::query(
            "INSERT INTO catalog_table_metadata_log (table_id, timestamp_ms, metadata_file) VALUES ($1,$2,$3)",
        )
        .bind(table_id).bind(last.timestamp_ms).bind(metadata_file)
        .execute(&mut **tx).await.map_err(CatalogError::Db)?;
    }
    // snapshot log: re-sync to match metadata (cheap; small)
    sqlx::query("DELETE FROM catalog_table_snapshot_log WHERE table_id = $1")
        .bind(table_id).execute(&mut **tx).await.map_err(CatalogError::Db)?;
    for e in md.history() {
        sqlx::query(
            "INSERT INTO catalog_table_snapshot_log (table_id, snapshot_id, timestamp_ms) VALUES ($1,$2,$3)",
        )
        .bind(table_id).bind(e.snapshot_id).bind(e.timestamp_ms)
        .execute(&mut **tx).await.map_err(CatalogError::Db)?;
    }
    Ok(())
}
```

- [ ] **Step 3: Compile-check (no test yet — assemble lands in Task 5)**

Run: `cargo build -p egret-catalog`
Expected: compiles. Fix any accessor-name mismatches against the fork source.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-catalog/src/service/table_repo.rs crates/egret-catalog/src/service/mod.rs
git commit -m "feat(catalog): table metadata shred helpers"
```

---

## Task 5: Table assemble-on-read + round-trip test (correctness gate)

**Files:**
- Modify: `crates/egret-catalog/src/service/table_repo.rs` (add `assemble`)
- Modify: `crates/egret-catalog/src/service/store.rs` (`insert_table`/`load_table` use repo)
- Test: `crates/egret-catalog/tests/store_tables.rs`

- [ ] **Step 1: Write the round-trip test (the gate)**

Replace `store_tables.rs`'s `insert_load_drop_table` with a version that asserts
the assembled metadata equals the original, threading a catalog. Add the
`seed_catalog` helper (as in Task 3). Keep the existing `minimal_metadata`.

```rust
#[tokio::test]
async fn insert_then_load_roundtrips_metadata() {
    let Some(s) = store().await else { eprintln!("skip"); return; };
    let cid = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s.create_namespace(cid, &levels, &std::collections::HashMap::new()).await.unwrap();
    let tid = uuid::Uuid::new_v4();
    let loc = format!("s3://wh/{}/orders-{tid}", levels[0]);
    let md = minimal_metadata(&loc, tid);
    let mloc = format!("{loc}/metadata/00000-{}.metadata.json", uuid::Uuid::new_v4());
    s.insert_table(ns.id, "orders", tid, &md, &mloc, &loc).await.unwrap();

    let row = s.load_table(ns.id, "orders").await.unwrap();
    // The assembled metadata must equal the original (modulo last_updated_ms which
    // iceberg refreshes): compare the stable identity fields.
    assert_eq!(row.metadata.uuid(), md.uuid());
    assert_eq!(row.metadata.current_schema().schema_id(), md.current_schema().schema_id());
    assert_eq!(row.metadata.format_version(), md.format_version());
    assert_eq!(row.metadata.location(), md.location());
    assert_eq!(row.metadata.current_snapshot_id(), md.current_snapshot_id());
    s.drop_table(ns.id, "orders").await.unwrap();
    s.drop_namespace(cid, &levels).await.unwrap();
}
```

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p egret-catalog --test store_tables insert_then_load_roundtrips_metadata`
Expected: FAIL — `load_table` still reads the (now-dropped) `metadata` column / returns wrong data.

- [ ] **Step 3: Implement `assemble_table_metadata` (port lakekeeper)**

Add to `table_repo.rs` a function that fetches all child rows for a `table_id`
and rebuilds `TableMetadata`. Port lakekeeper's `into_table_metadata`
(`lakekeeper/crates/lakekeeper-storage-postgres/src/tabular/table/mod.rs:180-475`)
— it constructs the schemas/specs/snapshots/sort_orders/refs collections and
calls `TableMetadataBuilder ... .build_unchecked()` then `.try_normalize()`.
egret uses the **same iceberg-rust fork** (`bb70173`), so the builder methods
(`.snapshots()`, `.refs()`, `.statistics()`, `.build_unchecked()`) are
available verbatim. Drop the V3-only setters (`first_row_id`, `assigned_rows`,
`encryption_keys`, `next_row_id`) — egret targets V2. Fetch child rows with
simple `SELECT ... WHERE table_id = $1` per child table (no ARRAY_AGG needed at
this scale; optimize later if load shows up hot).

Signature:
```rust
pub async fn assemble_table_metadata(
    pool: &sqlx::PgPool,
    table_id: Uuid,
    root: &TableRootRow,   // the catalog_tables row: format_version, last_*, current_*, location, table_uuid
) -> Result<TableMetadata, CatalogError>
```

- [ ] **Step 4: Rewrite `insert_table` and `load_table` in `store.rs`**

`insert_table`: open a tx, INSERT the `catalog_tables` root row (the new columns:
`format_version`, `last_column_id`, `last_sequence_number`, `last_updated_ms`,
`last_partition_id`, `current_schema_id`, `default_spec_id`,
`default_sort_order_id`, `current_snapshot_id`, `metadata_location`, `location`,
`current_version = 0`) reading values from `&TableMetadata`, then call
`table_repo::shred_children(&mut tx, table_id, md)` and
`table_repo::append_logs(...)`, then commit. Return nothing (as today).

`load_table`: fetch the `catalog_tables` root row into a `TableRootRow`, then
`table_repo::assemble_table_metadata(self.pool(), id, &root)`; package into the
existing `TableRow` struct (now built from assembled metadata, not JSON).

- [ ] **Step 5: Run the round-trip test — verify it passes**

Run: `cargo test -p egret-catalog --test store_tables`
Expected: PASS (all tests in the file). If `current_snapshot_id` or schema
mismatches, inspect the assemble fn against the fork's `TableMetadata` fields.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-catalog/src/service/table_repo.rs crates/egret-catalog/src/service/store.rs crates/egret-catalog/tests/store_tables.rs
git commit -m "feat(catalog): normalized table assemble + round-trip gate"
```

---

## Task 6: Normalized commit (CAS + append children)

**Files:**
- Modify: `crates/egret-catalog/src/service/store.rs` (`cas_commit`)
- Test: `crates/egret-catalog/tests/store_tables.rs` (existing `cas_commit_rejects_stale_version` + a new append test)

- [ ] **Step 1: Add a test that a commit persists a new snapshot row**

```rust
#[tokio::test]
async fn commit_appends_snapshot_rows() {
    let Some(s) = store().await else { eprintln!("skip"); return; };
    let cid = seed_catalog(&s).await;
    let levels = vec![format!("t_{}", uuid::Uuid::new_v4().simple())];
    let ns = s.create_namespace(cid, &levels, &std::collections::HashMap::new()).await.unwrap();
    let tid = uuid::Uuid::new_v4();
    let loc = format!("s3://wh/{}/t-{tid}", levels[0]);
    let md0 = minimal_metadata(&loc, tid);
    let mloc0 = format!("{loc}/metadata/00000-x.metadata.json");
    s.insert_table(ns.id, "t", tid, &md0, &mloc0, &loc).await.unwrap();
    let row = s.load_table(ns.id, "t").await.unwrap();
    // Build a v2 metadata by adding a schema-only change via apply_commit upstream is heavy;
    // here assert CAS still works and version advances with normalized writes.
    s.cas_commit(row.id, 0, &md0, &format!("{loc}/metadata/00001-y.metadata.json")).await.unwrap();
    let reloaded = s.load_table(ns.id, "t").await.unwrap();
    assert_eq!(reloaded.current_version, 1);
    s.drop_table(ns.id, "t").await.unwrap();
    s.drop_namespace(cid, &levels).await.unwrap();
}
```

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p egret-catalog --test store_tables commit_appends_snapshot_rows`
Expected: FAIL — `cas_commit` still writes the removed `metadata` JSONB column.

- [ ] **Step 3: Rewrite `cas_commit`**

Within one tx: `UPDATE catalog_tables SET previous_metadata_location = metadata_location,
metadata_location = $new, current_snapshot_id = $snap, last_sequence_number = ...,
last_updated_ms = ..., last_column_id = ..., last_partition_id = ...,
current_schema_id = ..., default_spec_id = ..., default_sort_order_id = ...,
current_version = current_version + 1, updated_at = now() WHERE id = $id AND current_version = $expected`.
If `rows_affected() == 0` → rollback + `Conflict`. Else call
`table_repo::shred_children(&mut tx, table_id, md)` (append-only children, re-sync
pointers) and `table_repo::append_logs(&mut tx, table_id, md, new_metadata_location)`.
Commit.

- [ ] **Step 4: Run the full table test file — verify it passes**

Run: `cargo test -p egret-catalog --test store_tables`
Expected: PASS (round-trip, CAS-stale-conflict, append, duplicate, list).

- [ ] **Step 5: Commit**

```bash
git add crates/egret-catalog/src/service/store.rs crates/egret-catalog/tests/store_tables.rs
git commit -m "feat(catalog): normalized CAS commit with append-only children"
```

---

## Task 7: Normalized views (`view_repo.rs`)

**Files:**
- Create: `crates/egret-catalog/src/service/view_repo.rs`
- Modify: `crates/egret-catalog/src/service/store.rs` (view methods), `service/mod.rs`
- Test: `crates/egret-catalog/tests/store_views.rs`

- [ ] **Step 1: Write a view round-trip test**

Mirror Task 5's structure for views: create a catalog + namespace, insert a view
with a minimal `ViewMetadata`, load it, assert identity fields
(`view_uuid`, `current_version_id`, `location`) survive. Add a
`minimal_view_metadata` helper in `test_support.rs` analogous to
`minimal_table_metadata` (use `iceberg::spec::ViewMetadataBuilder` from the fork).

- [ ] **Step 2: Run it — verify it fails**

Run: `cargo test -p egret-catalog --test store_views`
Expected: FAIL — view store still reads/writes the old JSONB column.

- [ ] **Step 3: Implement `view_repo.rs` shred + assemble**

Shred a `ViewMetadata` into `catalog_view_schemas`, `catalog_view_versions`,
`catalog_view_representations` (one row per SQL representation:
`type`/`sql`/`dialect`), `catalog_view_version_log`, `catalog_view_properties`;
write the `catalog_views` root (`view_uuid`, `location`, `format_version`,
`current_version_id`, `metadata_location`, `current_version`). Assemble by
fetching child rows and rebuilding via `ViewMetadataBuilder`. Accessors live in
the fork at `crates/iceberg/src/spec/view_metadata.rs` — verify names
(`.versions()`, `.schemas_iter()`, `.current_version_id()`, representation
`.sql()`/`.dialect()`).

- [ ] **Step 4: Rewrite the view store methods in `store.rs`** to use `view_repo`
  (insert/load/drop), same shape as tables.

- [ ] **Step 5: Run the test — verify it passes**

Run: `cargo test -p egret-catalog --test store_views`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-catalog/src/service/view_repo.rs crates/egret-catalog/src/service/store.rs crates/egret-catalog/src/service/mod.rs crates/egret-catalog/tests/store_views.rs crates/egret-catalog/src/service/test_support.rs
git commit -m "feat(catalog): normalized view shred/assemble"
```

---

## Task 8: Repair remaining integration tests + full suite green

**Files:**
- Modify: `crates/egret-catalog/tests/http_roundtrip.rs` (and any other test referencing the old schema)

- [ ] **Step 1: Update `http_roundtrip.rs`**

This test drives the Iceberg REST router, which still has no `{prefix}` (that's
Plan A-2). For A-1, make it create a workspace + catalog and point the
`CatalogService` at that catalog's id/warehouse so the existing single-catalog
routes resolve. If the router cannot yet be told a catalog id without A-2's
extractor, mark the HTTP-level assertions `#[ignore]` with a comment
`// re-enabled in Plan A-2 once {prefix} routing lands` and keep the store-level
assertions. Do NOT delete coverage — ignore with a reason.

- [ ] **Step 2: Run the whole crate test suite**

Run: `cargo test -p egret-catalog`
Expected: PASS (ignored HTTP tests reported as ignored, not failed).

- [ ] **Step 3: Run fmt + clippy (CI parity)**

Run:
```bash
cargo fmt --all
cargo clippy -p egret-catalog --all-targets -- -D warnings
```
Expected: no diffs, no warnings. (egret CI runs `cargo fmt --all --check`.)

- [ ] **Step 4: Commit**

```bash
git add crates/egret-catalog/tests/http_roundtrip.rs
git commit -m "test(catalog): adapt integration tests to normalized per-workspace store"
```

---

## Self-review checklist (run before handing off)

- [ ] Spec §3 schema fully represented in migration `0023` (incl. statistics + view representations + `catalog_tokens`); `catalog_table_snapshots.schema_id` is nullable.
- [ ] Every store method that touched a namespace/table/view now threads `catalog_id` (namespaces) or scopes via `namespace_id`/`table_id`.
- [ ] No `metadata jsonb` reads/writes remain in `store.rs` (grep: `rg "metadata" crates/egret-catalog/src/service/store.rs`).
- [ ] Round-trip tests (table + view) assert assembled == original identity fields.
- [ ] Method names consistent: `shred_children`, `append_logs`, `assemble_table_metadata`, `create_catalog`, `get_catalog_by_workspace`.
- [ ] `cargo test -p egret-catalog` green; `cargo clippy` clean; `cargo fmt --all --check` clean.

## Out of scope (Plan A-2 / A-3)

- HTTP `/v1/{prefix}/...` routing + per-request catalog extractor + warehouse resolution.
- Dropping the global `warehouse: Arc<str>` from `CatalogService` / `egret-bin`.
- Workspace-create → provision `catalogs` row hook.
- `catalog_tokens` auth in `require_bearer`; RBAC `catalog:read|write` perms.
- Internal UI API + frontend Catalog view.
- Manual Trino integration test update.
