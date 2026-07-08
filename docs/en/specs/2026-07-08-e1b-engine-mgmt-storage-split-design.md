# E1b — Engine-mgmt storage & schema split (design)

> **Status:** Design (approved to plan) · **Date:** 2026-07-08 · **Epic:** E1 (Engine-provisioning seam) of the [service-ready architecture](../architecture/2026-07-08-service-ready-architecture-design.md).
> **Follows:** [E1a](../plans/2026-07-08-e1a-engine-spec-model-collapse-plan.md) (spec-model collapse, merged #179).
> **One-line goal:** Give the engine-provisioning seam **ownership of its data** — move the `engines` + `engine_catalogs` tables into a dedicated `engine_mgmt` Postgres schema and their CRUD out of `egret-catalog`'s `CatalogStore` into a new `EngineMgmtStore` in a new seam crate `egret-engine-mgmt`.

---

## 1. Why

Per the seam recipe (design §3.2), each seam must **own its tables** so it can later become a separate service without a shared-DB coupling. Today `engines`/`engine_catalogs` live in the `public` schema and their CRUD is interleaved inside `egret-catalog`'s single `CatalogStore` impl (`crates/egret-catalog/src/service/embedded.rs`, ~633 LOC) alongside Warehouse / CatalogRole / CatalogGrant / CatalogClient CRUD. This E1b slice carves out that ownership. It touches **data + storage only** — the HTTP router/handlers and the provisioning `Port` are the next slice (E1c).

## 2. The three resolved E1 decisions this design builds on

1. **Secret-resolution** (`build_provision_req`/`resolve_*`) stays in the HTTP/control-plane layer, above the future Port — not moved here.
2. **exec/log-stream** stays out of the kube-free Port (uses orchestrator's `KubeClient` directly) — an E1c concern, not touched here.
3. **Storage split** = a dedicated `EngineMgmtStore` owning the two tables in schema `engine_mgmt`; the auto-wiring's catalog-seam calls (`create_client`/`add_grant`/`get_warehouse`) remain **cross-seam calls from the HTTP layer**, which holds both stores. ← this slice.

## 3. Design

### 3.1 New crate `egret-engine-mgmt`
The engine-mgmt seam's home. E1b puts only `EngineMgmtStore` in it; E1c grows it with the HTTP router (`engines.rs` + `monitor.rs`) and the provisioning `Port`.
- **Dependencies:** `egret-metastore` (the `Db`/sqlx pool), `egret-core` (ids/error), `sqlx`, plus whatever the moved queries need (`serde_json`, `chrono`, `uuid`). **No `egret-catalog` dependency** — the store touches only its own tables; the auto-wiring's catalog calls stay in the caller (control-plane), which depends on both stores.
- **`EngineMgmtStore`:** the engine + engine-catalog CRUD moved verbatim from `CatalogStore` — `list_engines`, `get_engine`, `create_engine`, `update_engine`, `delete_engine`, `set_engine_status`, `set_engine_authz_service`, `list_engine_catalogs`, `create_engine_catalog`, `get_engine_catalog`, `update_engine_catalog`, `delete_engine_catalog` (the exact set is enumerated from `embedded.rs` in the plan). Same signatures, same error type; only the SQL table names change to be schema-qualified.

### 3.2 Migration `0062_engine_mgmt_schema.sql`
```sql
CREATE SCHEMA IF NOT EXISTS engine_mgmt;
ALTER TABLE engines SET SCHEMA engine_mgmt;
ALTER TABLE engine_catalogs SET SCHEMA engine_mgmt;
```
- **Cross-seam FKs stay** (Postgres supports cross-schema FKs within one database) and get a `-- SEAM-CROSSING` annotation: `engine_mgmt.engines.warehouse_id → public.warehouses` (catalog seam) and `engine_mgmt.engines.authz_service_id → public.authz_services` (policy seam). They are dropped only at a real service split (design §3.2 decision).
- **In-seam FK unchanged:** `engine_mgmt.engine_catalogs.engine_id → engine_mgmt.engines` (both moved together).
- `SET SCHEMA` preserves data, indexes, constraints, and the FKs — no data migration, no downtime concern (pre-release).

### 3.3 Schema-qualified queries
`EngineMgmtStore`'s SQL uses fully-qualified names (`engine_mgmt.engines`, `engine_mgmt.engine_catalogs`) rather than a `search_path`-pinned pool. Rationale: explicit, compile-time-clear for sqlx's macro checks, and it reuses the shared `egret-metastore` pool (no second pool to manage) — the store only ever touches its own two tables. (Contrast: lakekeeper needs a `search_path`-pinned pool because it owns ~63 tables and can't qualify them all; two tables qualify cleanly.)

### 3.4 Re-point callers
- `egret-catalog`'s `CatalogStore` **loses** the engine/engine-catalog methods (they moved); it keeps Warehouse/Role/Grant/Client CRUD.
- `egret-controlplane`'s `engines.rs` uses `EngineMgmtStore` for engine CRUD and keeps `CatalogStore` for the auto-wiring catalog calls (`create_client`/`add_grant`/`get_warehouse`/`create_role`). `CpState` gains an `engine_mgmt: EngineMgmtStore` field; the engine handlers read it instead of `storage`.
- `egret-bin` constructs `EngineMgmtStore` (over the shared pool) and passes it into `CpState`.
- Any other caller of the moved methods (checked in the plan; expected: only `engines.rs` + the control-plane test harness) is re-pointed.

## 4. Scope boundaries

- **IN:** new `egret-engine-mgmt` crate with `EngineMgmtStore`; migration 0062 (schema move + SEAM-CROSSING annotations); move the ~12 engine/catalog CRUD methods out of `CatalogStore`; re-point `engines.rs`/`CpState`/`egret-bin`.
- **OUT:** the HTTP router/handlers, `build_provision_req`/`resolve_*`/`parse_*`, the provisioning `Port` trait, exec/log wiring (all E1c); the auto-wiring *logic* in `create_engine` (unchanged — it just calls the two stores); any behavior change; `egret-workspace`/authz.

## 5. Verification

- **Migration applies:** `engine_mgmt` schema created, both tables moved, FKs intact — verified by a fresh `just reset-testdata`/migrate + a `\dt engine_mgmt.*` check, or a repo integration test that provisions an engine.
- **Store parity:** the moved methods behave identically — covered by the existing control-plane integration tests that exercise engine CRUD and the 8-step `create_engine` auto-wiring (they hit the real DB; they must stay green after the schema move + store split).
- **No cross-seam JOIN introduced:** `EngineMgmtStore`'s queries reference only `engine_mgmt.*` (grep-checkable); the auto-wiring reads warehouses/clients via `CatalogStore`, not via a JOIN.
- **Gate:** `cargo fmt --all --check`, `cargo clippy` on the touched crates, `cargo build --workspace`, `cargo test -p egret-controlplane -p egret-catalog -p egret-engine-mgmt`.

## 6. Decisions locked

1. New seam crate **`egret-engine-mgmt`** holds `EngineMgmtStore`; no `egret-catalog` dep.
2. Tables move to schema **`engine_mgmt`** (migration 0062); cross-seam FKs **kept + `-- SEAM-CROSSING`** annotated.
3. Queries are **schema-qualified** over the shared pool (not a second `search_path`-pinned pool).
4. Auto-wiring catalog calls stay **cross-seam from the control-plane layer** (holds both stores); `create_engine` logic unchanged.
5. E1b is **storage/data only** — router/Port/exec are E1c.

---

*Design for E1b. Next: a bite-sized implementation plan, then subagent-driven execution.*
