# EB-4: control-plane management + revert egret-polaris Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-egret-embedded-catalog-design.md` (Component 4). **EB-4.** Depends on EB-3 (stores). VI: `docs/vi/plans/2026-07-02-eb4-controlplane-mgmt-revert-polaris.md`.

**Goal:** Platform-admin control-plane routes to manage the embedded catalog (warehouses, roles/grants, clients) over EB-3's stores — and **remove the Polaris integration** merged in PR #115 (`egret-polaris` crate + the Polaris control-plane routes + Polaris DB tables).

**Architecture:** New router in `egret-controlplane` over EB-3's `WarehouseStore`/`RoleStore`/`CatalogClientStore`, gated by `require_platform_admin`. Delete the Polaris-era modules (`polaris_conn.rs`, `catalogs.rs`, `rbac.rs`, `assignment.rs`) + the `egret-polaris` crate + drop `polaris_connection`/`workspace_catalog_assignments`.

**Tech Stack:** Rust (axum, sqlx).

---

## Background the implementer needs

- `egret-controlplane` (P2/P4) has `CpState`, `require_platform_admin`, `CpError`, plus the Polaris modules to delete: `polaris_conn.rs`, `catalogs.rs`, `rbac.rs`, `assignment.rs` (all merged in #115). `storage.rs`/`secrets.rs` (P2) are kept.
- `egret-polaris` crate (client over Polaris management API) is no longer used — delete it.
- Migration `0050`/`0051` created `polaris_connection` + `workspace_catalog_assignments` — drop them (EB-4 adds `0054`).

---

## Task 1: Control-plane routes over the embedded stores

**Files:** Create `crates/egret-controlplane/src/embedded_catalog/routes.rs`; modify `src/lib.rs`, `CpState`

- [ ] **Step 1: `CpState` gains the stores** — add `warehouses: WarehouseStore`, `roles: RoleStore`, `clients: CatalogClientStore` (constructed in `egret-bin`).

- [ ] **Step 2: Failing test — 403 gate + warehouse create**

```rust
// non-admin -> 403 on GET /api/control-plane/warehouses
// platform_admin -> POST /warehouses {name, workspaceId, storageConnectionId} -> 201
```

- [ ] **Step 3: Routes** (all `require_platform_admin`, merged into `control_plane_router()`):
- Warehouses: `GET/POST /warehouses`, `GET/DELETE /warehouses/{id}`.
- Roles: `GET/POST /warehouses/{id}/roles` (`{name, consumerType}`); grants: `GET/POST /roles/{roleId}/grants` (`{securableType, namespace?, table?, privilege}`), `DELETE /grants/{grantId}`.
- Clients: `GET /warehouses/{id}/clients`, `POST /warehouses/{id}/clients` (`{name, consumerType, workspaceId, catalogRoleId}` → 201 with `clientId`/`clientSecret` **once**), `DELETE /clients/{id}`.

- [ ] **Step 4: Run + commit** — tests pass; `feat(control-plane): embedded-catalog mgmt (warehouses/roles/grants/clients)`.

---

## Task 2: Delete the Polaris integration

**Files:** `crates/egret-controlplane/src/*`, `crates/egret-polaris/`, `crates/egret-bin/*`, root `Cargo.toml`

- [ ] **Step 1: Remove the Polaris control-plane modules**

`git rm crates/egret-controlplane/src/{polaris_conn.rs,catalogs.rs,rbac.rs,assignment.rs}` (+ their tests). In `src/lib.rs`: remove their `mod`/router-merges + the `polaris_conn`/`assignments` fields from `CpState`. In `egret-bin/main.rs`: remove their construction. Keep `storage.rs`/`secrets.rs`.

- [ ] **Step 2: Delete the `egret-polaris` crate**

`git rm -r crates/egret-polaris`; remove it from `[workspace.dependencies]` + from `egret-controlplane` deps. `rg egret_polaris crates` → no live refs.

- [ ] **Step 3: Build** — `cargo build --workspace` 0 errors (compiler-guided: remove any leftover `egret_polaris::` import).

- [ ] **Step 4: Commit** — `chore(catalog): remove egret-polaris + Polaris control-plane routes`.

---

## Task 3: Migration — drop Polaris tables

**Files:** Create `migrations/0054_drop_polaris_tables.sql`; touch `crates/egret-metastore/src/lib.rs`

- [ ] **Step 1:**
```sql
DROP TABLE IF EXISTS workspace_catalog_assignments CASCADE;
DROP TABLE IF EXISTS polaris_connection CASCADE;
```
`touch … && cargo run -q -p egret-bin -- migrate`; verify both `to_regclass` null.

- [ ] **Step 2: Commit** — `feat(db): drop polaris_connection + workspace_catalog_assignments`.

---

## Task 4: Verify

- [ ] `cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `DATABASE_URL=… cargo test --workspace`. Authz: every `/api/control-plane/warehouses|roles|grants|clients` endpoint → 403 for non-admin.

---

## Self-review notes (spec coverage for EB-4)

- Spec **Component 4 + revert**: warehouse/role/grant/client mgmt routes (Task 1), `egret-polaris` + Polaris control-plane routes removed (Task 2), Polaris tables dropped (Task 3), all platform-admin-gated.
- **Kept:** control-plane crate + Storage/Secrets (P2).
- **Out of EB-4:** UI (EB-5), e2e (EB-6).
