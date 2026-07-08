# E3 — Catalog Seam Extraction (decomposition)

> **Status:** Decomposition · **Date:** 2026-07-08 · **Epic:** E3 of the [service-ready redesign](../architecture/2026-07-08-service-ready-architecture-design.md) · **Precedents:** E1 (engine-mgmt), E2 (policy/ranger). Executed **subagent-driven** on branch `feat/e3-catalog-seam`.

## 1. The tangle (what E3 untangles)
`egret-catalog`'s single `CatalogStore` fuses **four unrelated concerns**:
- **(a) warehouse registry** — `warehouses` table
- **(b) catalog RBAC** — `catalog_roles` + `catalog_grants`
- **(c) catalog client registry** — `catalog_clients` + `impl machine_idp::ClientVerifier`
- **(d) storage-connection registry** — `storage_connections` (`storage_conn.rs`) — *unrelated to catalog*

Plus: `egret-catalog-engine` (embeds lakekeeper) is **anyhow-only** → opaque 500s (the folded E0·P1c work); `warehouses.rs` (1111 LOC) + `storage.rs` (328) live in `egret-controlplane`; 4 crates (notebook, kernel, workspace, server) carry **dead** `egret-catalog` deps. Highest migration = 0063 → E3 uses **0064**.

## 2. Locked decisions
1. **Crate topology → MERGE into one seam crate.** The catalog store (a/b/c) folds into **`egret-catalog-engine`** so the seam is one crate. Constraint: `egret-catalog-engine` uses **sqlx 0.9** (lakekeeper) while the store uses **sqlx 0.8** (shared `egret-metastore::Db` pool). Resolution: the merged crate depends on both via a rename — `sqlx08 = { package = "sqlx", version = "0.8", features = [...] }` for the moved store code (which uses runtime `sqlx::query`, not compile-checked macros, so aliasing is safe), keeping `sqlx` = 0.9 for lakekeeper. The store keeps the shared 0.8 `Db` pool; lakekeeper keeps its isolated 0.9 pool. **This merge is isolated to E3d** — E3a–c keep `egret-catalog` as-is so the risky merge lands last, on a green base.
2. **Storage split depth → registry + admin router.** Move both the `storage_connections` registry (`storage_conn.rs`) **and** `controlplane/storage.rs`'s HTTP router into `egret-storage`; give storage its own `storage` schema + `admin_router()`.
3. **`oidc_signing_keys` → `catalog` schema.** machine-idp is part of seam #1; its table moves into `catalog` (machine-idp reads it schema-qualified).

Effort-B boundary: `CatalogGuard`/`EgretCatalogGuard` authz **relocates as-is** (no semantic redesign).

## 3. Parts (strangler order within E3)
| Part | Content | Risk |
|---|---|---|
| **E3a** | Leaf contract crate **`egret-catalog-api`**: `CatalogPort` (the provisioning slice engine-mgmt needs) + pure-serde DTOs (`Warehouse`, `CatalogRole`, `CatalogGrant`, `GrantInput`, `NewClient`, `CreatedClient`, `CatalogClient`); host `ClientVerifier`/`VerifiedClient` (moved from machine-idp). Impl `CatalogPort` for `CatalogStore`; rewire engine-mgmt auto-wiring onto `Arc<dyn CatalogPort>`. | Low (E1a/E2a shape) |
| **E3b** | Move `storage_conn.rs` (StorageConn CRUD) `egret-catalog` → `egret-storage` (add `sqlx`+`Db`); move `controlplane/storage.rs` → `egret-storage` `admin_router()`; migration `0064? storage schema` (`SET SCHEMA storage` for `storage_connections`); cross-schema FKs `warehouses.storage_conn_id`→`storage.storage_connections`, `storage_connections.secret_ref`→`workspace_secrets` annotated SEAM-CROSSING. FE URLs stable. | Medium |
| **E3c** | Migration (catalog schema): `CREATE SCHEMA catalog`; `SET SCHEMA catalog` for `warehouses`, `catalog_roles`, `catalog_grants`, `catalog_clients`, `oidc_signing_keys`; schema-qualify the store SQL (`egret-catalog` embedded.rs) + machine-idp's `oidc_signing_keys` SQL. Force metastore re-embed. | Medium |
| **E3d** | **The merge + big move.** Fold `egret-catalog`'s store into `egret-catalog-engine` (sqlx08 alias); move `controlplane/warehouses.rs` (1111) into the seam as `admin_router()` behind `CatalogPort` + relocated `WarehouseProvisioner`/`WarehouseBrowser`; give `egret-catalog-engine` a real domain error + `IntoResponse` (E0·P1c); strip `CpState.storage`/`provisioner`/`browser`; drop the 4 dead deps; `EgretCatalogGuard` relocated as-is. FE URLs stable. | High |

(Migration numbering: E3b takes 0064, E3c takes 0065 — confirm actual next number at implementation time.)

## 4. Execution
Subagent-driven (superpowers): per part → implementer subagent → spec-compliance review → code-quality review → commit. Each part: fmt + clippy(ws, all-targets) + affected tests green (DB-gated tests against `egret_test`; known `last_admin_guard` isolation flake excluded). Stacked on one branch → one squash PR at the end (`--admin`, per project practice — GitHub CI out of quota). Zero frontend change intended (route strings preserved).

## 5. Deferred
OpenAPI emit + FE codegen (E0·P4); `catalog.changed`/`warehouse.*` events (first consumer); authz-semantics unification (Effort B, plugs into the E3 catalog contract).
