# E1b — Engine-mgmt storage & schema split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the engine-provisioning seam ownership of its data: move `engines`/`engine_catalogs` into a new `engine_mgmt` Postgres schema and their CRUD out of `egret-catalog`'s `CatalogStore` into a new `EngineMgmtStore` in a new crate `egret-engine-mgmt`.

**Architecture:** New crate `egret-engine-mgmt` holds `EngineMgmtStore` (mirrors `CatalogStore`'s `{ db: Db }` + `pool()` shape) + an `EngineMgmtError`. The 12 engine/engine-catalog methods move verbatim (only the error type name and the schema-qualified table names change — the SQL is runtime `sqlx::query`/`query_as`, so no `.sqlx` offline regen). The auto-wiring in `create_engine` (control-plane) keeps calling `CatalogStore` for `create_client`/`add_grant`/`get_warehouse` — a cross-seam call. Design: [E1b design](../specs/2026-07-08-e1b-engine-mgmt-storage-split-design.md).

**Tech Stack:** Rust 2021, sqlx (runtime queries — NOT the `query!` macro), Postgres. `egret-metastore` provides `Db`/pool + the embedded MIGRATOR.

**Scope:** storage/data only. OUT: HTTP router/handlers, `build_provision_req`/`resolve_*`/`parse_*`, the provisioning Port, exec/log (all E1c); `create_engine` auto-wiring *logic* (unchanged — it just calls two stores now).

---

## File structure

- `crates/egret-engine-mgmt/{Cargo.toml, src/lib.rs, src/store.rs, src/error.rs}` — **new crate**. `store.rs`: `EngineMgmtStore` + the `Engine`/`EngineCatalog` structs + the 12 moved methods. `error.rs`: `EngineMgmtError`.
- `Cargo.toml` (root) — register `egret-engine-mgmt`.
- `migrations/0062_engine_mgmt_schema.sql` — **new**. Schema move.
- `crates/egret-metastore/src/lib.rs` — touch (whitespace) to force the embedded migrator to re-embed 0062.
- `crates/egret-catalog/src/service/embedded.rs` — delete the moved methods + `Engine`/`EngineCatalog` structs.
- `crates/egret-catalog/src/lib.rs` — drop `Engine`/`EngineCatalog` from the re-export.
- `crates/egret-controlplane/{Cargo.toml, src/lib.rs, src/error.rs, src/engines.rs, tests/routes_api.rs}` — dep the new crate; add the `engine_mgmt` `CpState` field + `From<EngineMgmtError>`; re-point `engines.rs`; fix the test-harness `CpState` build.
- `crates/egret-bin/{Cargo.toml, src/main.rs}` — dep + wire `engine_mgmt` into the `CpState` construction.

---

## Task 1: New crate `egret-engine-mgmt` with the store

**Files:** Create `crates/egret-engine-mgmt/Cargo.toml`, `src/lib.rs`, `src/error.rs`, `src/store.rs`; modify root `Cargo.toml`.

- [ ] **Step 1: Manifest**

`crates/egret-engine-mgmt/Cargo.toml`:
```toml
[package]
name = "egret-engine-mgmt"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
egret-metastore.workspace = true
sqlx.workspace = true
uuid.workspace = true
chrono.workspace = true
serde_json.workspace = true
thiserror.workspace = true
```
Root `Cargo.toml` `[workspace.dependencies]`, add:
```toml
egret-engine-mgmt = { path = "crates/egret-engine-mgmt" }
```

- [ ] **Step 2: Error type**

`crates/egret-engine-mgmt/src/error.rs` — mirror exactly the two `CatalogError` variants the engine methods produce, so their bodies move with a pure `CatalogError → EngineMgmtError` rename:
```rust
//! Engine-mgmt store errors.

#[derive(Debug, thiserror::Error)]
pub enum EngineMgmtError {
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}
```

- [ ] **Step 3: Store + moved methods**

`crates/egret-engine-mgmt/src/store.rs`. Create `EngineMgmtStore` (same shape as `CatalogStore`), the `Engine`/`EngineCatalog` structs (copied verbatim from `egret-catalog/src/service/embedded.rs` lines 34–60), and MOVE the 12 methods from `embedded.rs` (`create_engine`, `list_engines`, `get_engine`, `update_engine`, `set_engine_status`, `set_engine_authz_service`, `delete_engine`, `create_engine_catalog`, `list_engine_catalogs`, `get_engine_catalog`, `update_engine_catalog`, `delete_engine_catalog`) applying TWO mechanical substitutions to each: (a) `CatalogError` → `EngineMgmtError`; (b) the SQL table names `engines`/`engine_catalogs` → `engine_mgmt.engines`/`engine_mgmt.engine_catalogs`. Do NOT change constraint-name checks (`engines_name_key`, `engine_catalogs_engine_id_name_key` survive `SET SCHEMA` unchanged). Skeleton + the first method as the worked example (do the rest identically):
```rust
//! Engine-mgmt store: CRUD for the `engine_mgmt.engines` / `engine_mgmt.engine_catalogs`
//! tables (this seam owns them). Runtime sqlx queries; schema-qualified.

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::EngineMgmtError;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Engine {
    pub id: Uuid,
    pub name: String,
    pub r#type: String,
    pub namespace: String,
    pub warehouse_id: Uuid,
    pub spec: serde_json::Value,
    pub status: String,
    pub authz_service_id: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct EngineCatalog {
    pub id: Uuid,
    pub engine_id: Uuid,
    pub name: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub secret_ref: Option<Uuid>,
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct EngineMgmtStore {
    db: Db,
}

impl EngineMgmtStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
    fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    pub async fn create_engine(
        &self,
        name: &str,
        r#type: &str,
        namespace: &str,
        warehouse_id: Uuid,
        spec: &serde_json::Value,
    ) -> Result<Engine, EngineMgmtError> {
        sqlx::query_as::<_, Engine>(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) VALUES ($1,$2,$3,$4,$5) \
             RETURNING id, name, type, namespace, warehouse_id, spec, status, authz_service_id, created_at",
        )
        .bind(name).bind(r#type).bind(namespace).bind(warehouse_id).bind(spec)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.constraint() == Some("engines_name_key") => {
                EngineMgmtError::AlreadyExists(name.to_string())
            }
            _ => EngineMgmtError::Db(e),
        })
    }

    // … move the remaining 11 methods here verbatim, applying substitutions (a)+(b) …
}
```
`crates/egret-engine-mgmt/src/lib.rs`:
```rust
//! Engine-provisioning seam storage: owns the `engine_mgmt` schema.

pub mod error;
pub mod store;

pub use error::EngineMgmtError;
pub use store::{Engine, EngineCatalog, EngineMgmtStore};
```

- [ ] **Step 4: Build**

Run: `cargo build -p egret-engine-mgmt`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt Cargo.toml
git commit -m "feat(engine-mgmt): new crate with EngineMgmtStore (engines/engine_catalogs CRUD)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Migration — move the tables to schema `engine_mgmt`

**Files:** Create `migrations/0062_engine_mgmt_schema.sql`; touch `crates/egret-metastore/src/lib.rs`.

- [ ] **Step 1: Write the migration**

`migrations/0062_engine_mgmt_schema.sql`:
```sql
-- E1b: the engine-provisioning seam owns its tables. Move engines +
-- engine_catalogs from `public` into their own `engine_mgmt` schema.
-- SET SCHEMA preserves data, indexes, constraints, and FKs.
CREATE SCHEMA IF NOT EXISTS engine_mgmt;

ALTER TABLE engines SET SCHEMA engine_mgmt;
ALTER TABLE engine_catalogs SET SCHEMA engine_mgmt;

-- Cross-seam FKs remain (Postgres allows cross-schema FKs within one database);
-- they are dropped only at a real service split.
--   engine_mgmt.engines.warehouse_id      -> public.warehouses      -- SEAM-CROSSING (catalog seam)
--   engine_mgmt.engines.authz_service_id  -> public.authz_services  -- SEAM-CROSSING (policy seam)
-- In-seam FK unchanged:
--   engine_mgmt.engine_catalogs.engine_id -> engine_mgmt.engines
```

- [ ] **Step 2: Force the embedded migrator to re-embed**

The MIGRATOR embeds `migrations/` at compile time; a new file alone may not trigger a rebuild of `egret-metastore`. Add/remove a trailing newline (or a comment) in `crates/egret-metastore/src/lib.rs` to force recompilation.

- [ ] **Step 3: Verify the migration applies**

Run: `just reset-testdata` (or the project's migrate command against the dev DB).
Then verify: `psql "$DATABASE_URL" -c "\dt engine_mgmt.*"` shows `engine_mgmt.engines` and `engine_mgmt.engine_catalogs`; `psql "$DATABASE_URL" -c "\dt public.engines"` shows nothing.
Report the output.

- [ ] **Step 4: Commit**

```bash
git add migrations/0062_engine_mgmt_schema.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(db): migration 0062 — move engines/engine_catalogs to engine_mgmt schema

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Remove the moved code from `egret-catalog`

**Files:** Modify `crates/egret-catalog/src/service/embedded.rs`, `crates/egret-catalog/src/lib.rs`.

- [ ] **Step 1: Delete the moved structs + methods**

In `crates/egret-catalog/src/service/embedded.rs`: delete the `Engine` struct (lines ~34–45) and `EngineCatalog` struct (~50–60), and the 12 engine/engine-catalog methods inside `impl CatalogStore` (`create_engine` … `delete_engine_catalog`, the `// engines` + `// engine catalogs` sections, ~lines 200–377). Leave everything else (warehouses, roles, grants, clients, `ClientVerifier` impl) untouched.

- [ ] **Step 2: Drop the re-exports**

In `crates/egret-catalog/src/lib.rs`, remove `Engine, EngineCatalog` from the `pub use service::{...}` list (line 11) — leaving `CatalogStore, StorageConn, StorageConnInput`. Also check `crates/egret-catalog/src/service/mod.rs` re-exports `Engine`/`EngineCatalog` and drop them there if present.

- [ ] **Step 3: Build**

Run: `cargo build -p egret-catalog`
Expected: clean (nothing inside egret-catalog used the deleted engine methods/structs — they were called only from control-plane).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-catalog/src/service/embedded.rs crates/egret-catalog/src/lib.rs crates/egret-catalog/src/service/mod.rs
git commit -m "refactor(catalog): drop engine CRUD — moved to egret-engine-mgmt

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Re-point `egret-controlplane`

**Files:** Modify `crates/egret-controlplane/Cargo.toml`, `src/lib.rs`, `src/error.rs`, `src/engines.rs`, `tests/routes_api.rs`.

- [ ] **Step 1: Dependency + `From<EngineMgmtError>`**

`crates/egret-controlplane/Cargo.toml` `[dependencies]`, add:
```toml
egret-engine-mgmt.workspace = true
```
In `crates/egret-controlplane/src/error.rs`, add a `From` mirroring the existing `From<CatalogError>` mapping so behavior is identical (dup name → `AlreadyExists` → 409; db → 500):
```rust
impl From<egret_engine_mgmt::EngineMgmtError> for CpError {
    fn from(e: egret_engine_mgmt::EngineMgmtError) -> Self {
        match e {
            egret_engine_mgmt::EngineMgmtError::AlreadyExists(s) => CpError::AlreadyExists(s),
            egret_engine_mgmt::EngineMgmtError::Db(_) => CpError::Internal,
        }
    }
}
```

- [ ] **Step 2: `CpState` field**

In `crates/egret-controlplane/src/lib.rs`, add to `CpState` (after `pub storage: CatalogStore`):
```rust
    /// Engine-mgmt store (`engine_mgmt` schema) — owns engines/engine_catalogs CRUD.
    pub engine_mgmt: egret_engine_mgmt::EngineMgmtStore,
```

- [ ] **Step 3: Re-point `engines.rs`**

In `crates/egret-controlplane/src/engines.rs`:
- Change the `Engine`/`EngineCatalog` import from `egret_catalog::{… Engine, EngineCatalog …}` to `egret_engine_mgmt::{Engine, EngineCatalog}` (keep other `egret_catalog` imports like `GrantInput`, `NewClient`).
- Replace every engine-CRUD call `st.storage.<m>(…)` / `state.storage.<m>(…)` — for `<m>` in the 12 moved method names — with `st.engine_mgmt.<m>(…)`. Leave the catalog auto-wiring calls (`create_client`, `add_grant`, `get_warehouse`, `create_role`, `delete_engine`? no — `delete_engine` moved; `revoke_client`, `get_storage_conn`) on `st.storage`. Use the compiler to find every site: after the edit, `cargo build -p egret-controlplane` will flag any missed `storage.<engine_method>` as "no method" — fix each by switching to `engine_mgmt`.
- The `?` on these calls now yields `EngineMgmtError` → `CpError` via Step 1's `From`. If any site explicitly `match`es the old `CatalogError` from an engine call, swap it to `EngineMgmtError` variants (compiler-flagged).

- [ ] **Step 4: Fix the test harness `CpState` build**

In `crates/egret-controlplane/tests/routes_api.rs`, the test(s) construct `CpState { … }`. Add the new field, e.g. `engine_mgmt: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),` using whatever `Db`/pool the harness already has (the same one `storage: CatalogStore::new(db…)` uses). The compiler names the missing field.

- [ ] **Step 5: Build + test**

Run: `cargo build -p egret-controlplane`
Expected: clean.
Run: `cargo test -p egret-controlplane`
Expected: PASS — the engine-CRUD and `create_engine` auto-wiring integration tests exercise the new store against the `engine_mgmt` schema (migration applied); behavior unchanged.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-controlplane/Cargo.toml crates/egret-controlplane/src/lib.rs crates/egret-controlplane/src/error.rs crates/egret-controlplane/src/engines.rs crates/egret-controlplane/tests/routes_api.rs
git commit -m "refactor(controlplane): engine CRUD via EngineMgmtStore (engine_mgmt seam)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Wire `egret-bin`

**Files:** Modify `crates/egret-bin/Cargo.toml`, `crates/egret-bin/src/main.rs`.

- [ ] **Step 1: Dependency**

`crates/egret-bin/Cargo.toml` `[dependencies]`, add:
```toml
egret-engine-mgmt.workspace = true
```

- [ ] **Step 2: Construct the store in `CpState`**

In `crates/egret-bin/src/main.rs`, in the `egret_controlplane::CpState { … }` construction (around line 1193, right after `storage: egret_catalog::CatalogStore::new(db.clone()),`), add:
```rust
        engine_mgmt: egret_engine_mgmt::EngineMgmtStore::new(db.clone()),
```

- [ ] **Step 3: Build**

Run: `cargo build -p egret-bin`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-bin/Cargo.toml crates/egret-bin/src/main.rs
git commit -m "feat(bin): wire EngineMgmtStore into control-plane state

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Workspace gate + seam-ownership proof

**Files:** none (verification only).

- [ ] **Step 1: Format + clippy**

Run: `cargo fmt --all --check` (fmt + `style:` commit if needed).
Run: `cargo clippy -p egret-engine-mgmt -p egret-catalog -p egret-controlplane -p egret-bin --all-targets -- -D warnings`
Expected: clean (ignore the pre-existing vendored `mysql_async` lint).

- [ ] **Step 2: Workspace build + tests**

Run: `cargo build --workspace`
Expected: clean.
Run: `cargo test -p egret-controlplane -p egret-catalog -p egret-engine-mgmt`
Expected: PASS.

- [ ] **Step 3: Seam-ownership proof — no stray raw SQL against the moved tables**

Run: `grep -rn "FROM engines\b\|JOIN engines\b\|INTO engines\b\|FROM engine_catalogs\|INTO engine_catalogs\|UPDATE engines\b\|DELETE FROM engines\b" crates --include=*.rs | grep -v "engine_mgmt\."`
Expected: **no output** — every raw reference to these tables is now schema-qualified inside `EngineMgmtStore`. Any hit is a straggler query elsewhere that must be schema-qualified too; report it.

- [ ] **Step 4: No cross-seam JOIN in the new store**

Run: `grep -n "JOIN" crates/egret-engine-mgmt/src/store.rs`
Expected: no output (the engine methods are single-table; the auto-wiring's cross-seam reads go through `CatalogStore` in control-plane, not a JOIN here).

- [ ] **Step 5: Final `style:` commit if fmt changed anything**

```bash
git add -A && git commit -m "style: cargo fmt after engine-mgmt storage split" || echo "nothing to format"
```

---

## Self-review notes

- **Spec coverage:** new crate + store (Task 1) → design §3.1; migration + SEAM-CROSSING (Task 2) → §3.2; delete from CatalogStore (Task 3) → §3.4; re-point control-plane incl. `CpState` field + test harness (Task 4) → §3.4; egret-bin wiring (Task 5) → §3.4; verification incl. no-cross-seam-JOIN + ownership proof (Task 6) → §5.
- **Behavior preservation:** the 12 methods move verbatim (only error-type name + schema-qualified table names change); `From<EngineMgmtError> for CpError` mirrors the old `CatalogError` mapping exactly (dup → 409, db → 500); `create_engine` auto-wiring logic in `engines.rs` is untouched.
- **Placeholder scan:** the one "do the rest identically" is a verbatim relocate of a fully-enumerated method list with two explicit mechanical substitutions — the source is the committed `embedded.rs`; not undefined logic.
- **Risk spots (from grounding):** (1) migration embed staleness → Task 2 Step 2 forces re-embed; (2) constraint-name checks survive `SET SCHEMA` → left unchanged, called out; (3) an explicit `CatalogError` match on an engine call in `engines.rs` → compiler-flagged in Task 4 Step 3; (4) stray raw SQL against the moved tables anywhere → Task 6 Step 3 grep.
