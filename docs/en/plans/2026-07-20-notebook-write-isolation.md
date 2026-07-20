# Notebook write isolation (Sub-project C) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Each workspace gets a private Iceberg namespace (in an admin-configured notebook warehouse); the kernel gets a namespace-scoped catalog JWT via the runtime manifest, and new `write_iceberg`/`read_iceberg` helpers read+write that namespace DIRECTLY (confined by the existing `EgretCatalogGuard`). Governed reads stay on `trino()`/`starrocks()`.

**Architecture:** Reuse the built authz machinery (CatalogGuard namespace grants + machine-IdP `sign_claims`). Net-new: a per-workspace notebook-warehouse setting, catalog role/grant provisioning, an in-process catalog-JWT mint, manifest extension, and the kernel helper (pyiceberg + daft already installed).

**Tech Stack:** Rust (axum, sqlx, serde), Python (pyiceberg/daft) kernel, Playwright.

**Design doc:** `docs/en/specs/2026-07-20-notebook-write-isolation-design.md`.

**Branch:** `feat/notebook-write-isolation` (created off main; design committed).

**Conventions:** Commit per task. Backend tests need `DATABASE_URL=postgres://egret:egret@localhost:5432/egret`. Before merge: fmt/clippy/workspace test; FE lint/typecheck/build; kernel pytest.

**Key grounded facts:**
- `NotebookState` (`crates/egret-notebook/src/routes.rs:41`) already has `catalog_base_url: String`. It is constructed in `crates/egret-bin/src/main.rs:1227`.
- `CatalogStore` methods (`crates/egret-catalog-engine/src/store/embedded.rs`): `create_role(warehouse_id, name, consumer_type, description)` (:235), `add_grant(role_id, &GrantInput)` (:286) with `GrantInput{ securable_type, namespace: Option<Vec<String>>, table_name, privilege }` (:50), `list_warehouses()` (:200), `get_warehouse(id)` (:210) → `Warehouse{ workspace_id, lakekeeper_warehouse_id, … }` (:18).
- Catalog JWT mint = `egret_machine_idp::sign_claims(&km, &json!({...}))` (`token.rs:9`, re-exported `lib.rs:12`) with `km = PgSigningKeyStore::new(db).active().await?` (`keys.rs:11`, `lib.rs:24`). Claims mirror `machine-idp/src/lib.rs:189-198`.
- Guard: a `securable_type:"namespace"` write grant = full rw on exactly that namespace (`catalog_grant_covers`, `egret-bin/src/main.rs:407-440`); warehouse-prefix must equal the token's `warehouse` claim (`:343`). Catalog REST base = `{catalog_base_url}/catalog` (mount `/catalog/v1`, `egret-server/src/lib.rs:293`).
- Notebook-settings precedent: `analytics.notebook_engine_access` + `get_settings`/`set_engine_access` (`crates/egret-notebook/src/store.rs:474-551`) + `GET/PUT /api/ws/{workspace}/notebook-settings` (`routes.rs:93`, PUT gated `workspace:manage_members`).
- Kernel image already ships `daft` + `pyiceberg[s3fs]` (`kernel-image/Dockerfile:14`); `runtime.py:14-26` currently reads only `username`+`engines`.

---

## Task 1: Migration + notebook-warehouse setting store

**Files:** Create `migrations/00XX_notebook_warehouse_settings.sql` (next free number — check `ls migrations | tail`); modify `crates/egret-notebook/src/store.rs`. Test: inline `#[sqlx::test]` in store.rs (or a tests file, mirror existing store tests).

- [ ] **Step 1: Migration.**
```sql
CREATE TABLE analytics.notebook_warehouse_settings (
    workspace_id uuid PRIMARY KEY,
    warehouse_id uuid NOT NULL,
    namespace    text[] NOT NULL DEFAULT ARRAY['notebook'],
    updated_at   timestamptz NOT NULL DEFAULT now()
);
```
Touch `crates/egret-metastore/src/lib.rs` (or wherever migrations embed) to force re-embed if needed (see the sqlx-migration-staleness convention).

- [ ] **Step 2: Store methods** in `store.rs` (near the notebook-settings block ~474):
```rust
pub async fn get_notebook_warehouse(
    &self, workspace_id: Uuid,
) -> Result<Option<(Uuid, Vec<String>)>, sqlx::Error> {
    let row = sqlx::query_as::<_, (Uuid, Vec<String>)>(
        "SELECT warehouse_id, namespace FROM analytics.notebook_warehouse_settings WHERE workspace_id = $1",
    ).bind(workspace_id).fetch_optional(self.pool()).await?;
    Ok(row)
}

pub async fn set_notebook_warehouse(
    &self, workspace_id: Uuid, warehouse_id: Uuid, namespace: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO analytics.notebook_warehouse_settings (workspace_id, warehouse_id, namespace)
         VALUES ($1,$2,$3)
         ON CONFLICT (workspace_id) DO UPDATE SET warehouse_id = EXCLUDED.warehouse_id,
             namespace = EXCLUDED.namespace, updated_at = now()",
    ).bind(workspace_id).bind(warehouse_id).bind(namespace).execute(self.pool()).await?;
    Ok(())
}
```
Match the crate's exact pool accessor + error type (read a neighbouring store method).

- [ ] **Step 3: Test** — sqlx round-trip: set then get returns `(warehouse_id, ["notebook"])`; upsert changes it. Run: `DATABASE_URL=… cargo test -p egret-notebook notebook_warehouse`. Commit `feat(notebook): notebook_warehouse_settings table + store`.

---

## Task 2: Notebook-warehouse route (GET include + PUT set) + provision catalog role/grant

**Files:** `crates/egret-notebook/src/{model,routes}.rs`; thread `CatalogStore` into `NotebookState`. Test: route test.

- [ ] **Step 1: Thread `CatalogStore` into `NotebookState`** (`routes.rs:41`): add `pub catalog: egret_catalog_engine::CatalogStore,` and set it in `crates/egret-bin/src/main.rs:1227` (`catalog: egret_catalog_engine::CatalogStore::new(db.clone())` — the exact pattern at main.rs:1371). Update any other `NotebookState` construction (tests) to add the field so the workspace compiles.

- [ ] **Step 2: GET includes the warehouse setting.** Extend `NotebookSettings` (`model.rs:104`) with `warehouse_id: Option<Uuid>` + `namespace: Vec<String>` (default empty). In `get_settings`/the GET handler, populate them from `get_notebook_warehouse(ws)`.

- [ ] **Step 3: New PUT to set the notebook warehouse + provision role/grant.** Add route `PUT /api/ws/{workspace}/notebook-settings/warehouse` (gated `workspace:manage_members` like the sibling PUT), body `SetNotebookWarehouse{ warehouse_id: Uuid, namespace: Option<Vec<String>> }`:
```rust
async fn set_warehouse(State(st): State<NotebookState>, WsContext(ctx): WsContext, Json(b): Json<SetNotebookWarehouse>)
  -> Result<Json<NotebookSettings>, NotebookApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, MANAGE).await?;
    // validate the warehouse belongs to this workspace
    let wh = st.catalog.get_warehouse(b.warehouse_id).await.map_err(…)?
        .ok_or(NotebookApiError::bad_request("warehouse not found"))?;
    if wh.workspace_id != ctx.workspace_id.0 { return Err(NotebookApiError::bad_request("warehouse not in this workspace")); }
    let ns = b.namespace.unwrap_or_else(|| vec!["notebook".to_string()]);
    st.store.set_notebook_warehouse(ctx.workspace_id.0, b.warehouse_id, &ns).await?;
    // provision the notebook catalog role + a namespace-scoped write grant (idempotent)
    let role = st.catalog.create_role(b.warehouse_id, "notebook", "notebook", Some("notebook scratch")).await.map_err(…)?;
    st.catalog.add_grant(role.id, &egret_catalog_engine::GrantInput{
        securable_type: "namespace".into(), namespace: Some(ns.clone()), table_name: None, privilege: "write".into(),
    }).await.map_err(…)?;
    // return the updated settings
    Ok(Json(st.store.get_settings(ctx.workspace_id.0).await?))
}
```
Adjust to the real `create_role` return (role or role_id), `NotebookApiError` constructors, and `MANAGE` const. Confirm `create_role` is create-or-get (idempotent); if it errors on duplicate, catch/lookup the existing role.

- [ ] **Step 4: Test** — a manage-permitted PUT sets the warehouse (validates ws ownership; rejects a foreign warehouse with 400) and the GET reflects it; assert a `notebook` role + namespace grant exist (query CatalogStore). Run `DATABASE_URL=… cargo test -p egret-notebook`. Commit `feat(notebook): set-notebook-warehouse API + provision catalog role/grant`.

---

## Task 3: In-process catalog-JWT mint + signing keys in NotebookState

**Files:** `crates/egret-notebook/src/runtime.rs` (mint helper); `NotebookState` (add keys); `egret-bin/src/main.rs`. Test: unit.

- [ ] **Step 1: Add `keys` to `NotebookState`** (`routes.rs:41`): `pub keys: egret_machine_idp::PgSigningKeyStore,` set in main.rs:1227 (`keys: egret_machine_idp::PgSigningKeyStore::new(db.clone())`). Update other constructions.

- [ ] **Step 2: Mint helper** in `runtime.rs`:
```rust
/// Mint an `aud:"catalog"` JWT scoped to `warehouse` + `role`, mirroring the
/// machine-IdP OAuth path (crates/egret-machine-idp/src/lib.rs:189-198) — a local
/// RS256 sign, no Keycloak/refresh-token dependency.
async fn mint_catalog_jwt(
    keys: &egret_machine_idp::PgSigningKeyStore,
    issuer: &str, warehouse_lakekeeper_id: &str, role: &str,
) -> Result<(String, chrono::DateTime<chrono::Utc>), NotebookApiError> {
    use egret_machine_idp::SigningKeyStore as _;
    let km = keys.active().await.map_err(…)?;
    let exp_ts = chrono::Utc::now().timestamp() as u64 + 3600;
    let claims = serde_json::json!({
        "iss": issuer.trim_end_matches('/'), "sub": "notebook", "aud": "catalog",
        "warehouse": warehouse_lakekeeper_id, "roles": [role], "exp": exp_ts,
    });
    let jwt = egret_machine_idp::sign_claims(&km, &claims).map_err(…)?;
    Ok((jwt, chrono::DateTime::from_timestamp(exp_ts as i64, 0).unwrap()))
}
```
The `issuer` is the machine-IdP issuer — find how the guard/idp gets it (the `st.issuer` at lib.rs:181; NotebookState may need it, or reuse `catalog_base_url`-derived issuer — confirm the issuer the guard's `verify` expects; the guard `verify_with_jwk` checks the signature, and `warehouse`/`roles` claims, so `iss` mainly needs to be consistent — match what OAuth-minted catalog tokens use).

- [ ] **Step 3: Test** — the minted JWT decodes (verify_with_jwk against the active key) to `aud:"catalog"`, the given warehouse + role, future exp. Run `cargo test -p egret-notebook mint_catalog`. Commit `feat(notebook): in-process catalog-JWT mint for kernels`.

---

## Task 4: Extend NotebookRuntime manifest + build_notebook_runtime + runtime.py

**Files:** `crates/egret-analytics-api/src/dto.rs`; `crates/egret-notebook/src/runtime.rs`; `kernel-image/egret_kernel_helpers/runtime.py`. Test: runtime builder test + kernel pytest.

- [ ] **Step 1: Extend `NotebookRuntime`** (`dto.rs:41-55`):
```rust
    pub catalog_jwt: Option<String>,
    pub catalog_jwt_expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub catalog_uri: Option<String>,
    pub catalog_warehouse: Option<String>,
    pub namespace: Vec<String>,
```
(Keep serde defaults so older consumers/tests don't break — `#[serde(default)]`.)

- [ ] **Step 2: Populate in `build_notebook_runtime`** (`runtime.rs:26-66`): after building engines/engine_jwt, if `st.store.get_notebook_warehouse(ws)` is `Some((wh_id, ns))`:
  - `wh = st.catalog.get_warehouse(wh_id)` → `lakekeeper_warehouse_id`;
  - `(catalog_jwt, exp) = mint_catalog_jwt(&st.keys, issuer, &wh.lakekeeper_warehouse_id, "notebook")`;
  - set `catalog_uri = Some(format!("{}/catalog", st.catalog_base_url))`, `catalog_warehouse = Some(wh.lakekeeper_warehouse_id)`, `namespace = ns`.
  Else leave the catalog_* fields `None` / `namespace = vec![]`.

- [ ] **Step 3: `runtime.py`** — extend `Runtime` (`kernel-image/egret_kernel_helpers/runtime.py:14-26`) to parse `catalog_jwt`, `catalog_uri`, `catalog_warehouse`, `namespace` (default None/[]). Expose them as attributes for the write helper.

- [ ] **Step 4: Test** — Rust: with a notebook-warehouse set, `build_notebook_runtime` returns `catalog_jwt=Some`, correct uri/warehouse/namespace; without, `None`. Kernel: `test_runtime` parses the new fields. Run `cargo test -p egret-notebook` + `kernel-image/.venv/bin/python -m pytest kernel-image/tests/test_runtime.py`. Commit `feat(notebook): carry catalog token + namespace in the runtime manifest`.

---

## Task 5: Kernel helpers `write_iceberg` / `read_iceberg`

**Files:** Create `kernel-image/egret_kernel_helpers/iceberg.py`; export in `__init__.py`; inject in `startup/00-egret.py`. Test: `kernel-image/tests/test_iceberg.py`.

- [ ] **Step 1: Write the failing test** (mock pyiceberg + daft like the existing `test_display.py`/`test_engines.py` mock daft): assert `write_iceberg(df, "t")` (a) builds a `RestCatalog` with `uri=<catalog_uri>`, `token=<catalog_jwt>`, `warehouse=<catalog_warehouse>`, (b) targets the namespace `runtime.namespace` + table `t` (never a caller-supplied catalog/schema), (c) calls `df.write_iceberg(<table>, mode=...)`. And `read_iceberg("t")` calls `daft.read_iceberg`. And that a missing `catalog_jwt` (warehouse not configured) raises a clear error.

- [ ] **Step 2: Implement `iceberg.py`:**
```python
"""Direct-Iceberg scratch plane — write/read the workspace's private namespace
via a namespace-scoped catalog token. Governed data still goes through trino()/
starrocks(). The target namespace is FORCED to the workspace's reserved one, so
this helper cannot address anything outside it (the catalog guard also rejects)."""

def _catalog(runtime):
    if not getattr(runtime, "catalog_jwt", None):
        raise RuntimeError("egret: notebook warehouse not configured — set it in workspace settings")
    from pyiceberg.catalog.rest import RestCatalog
    return RestCatalog("egret", uri=runtime.catalog_uri, token=runtime.catalog_jwt,
                       warehouse=runtime.catalog_warehouse), tuple(runtime.namespace)

def write_iceberg(df, name, mode="append", *, runtime=None):
    import daft  # noqa
    from egret_kernel_helpers.runtime import load_runtime
    rt = runtime or load_runtime()
    cat, ns = _catalog(rt)
    cat.create_namespace_if_not_exists(ns)
    ident = ns + (name,)
    try:
        tbl = cat.load_table(ident)
    except Exception:
        tbl = cat.create_table(ident, schema=df.to_arrow().schema)
    return df.write_iceberg(tbl, mode=mode)

def read_iceberg(name, *, runtime=None):
    import daft
    from egret_kernel_helpers.runtime import load_runtime
    rt = runtime or load_runtime()
    cat, ns = _catalog(rt)
    return daft.read_iceberg(cat.load_table(ns + (name,)))
```
Verify the exact pyiceberg `create_table`/`create_namespace_if_not_exists` signatures + daft `write_iceberg`/`read_iceberg` API against the installed versions during implementation; adjust the schema construction (daft df → pyiceberg schema) as needed.

- [ ] **Step 3: Export + inject** — add `write_iceberg, read_iceberg` to `__init__.py` `__all__` + imports, and to the `from egret_kernel_helpers import …` line in `startup/00-egret.py`.

- [ ] **Step 4:** `kernel-image/.venv/bin/python -m pytest kernel-image/tests/test_iceberg.py`. Commit `feat(notebook): write_iceberg/read_iceberg direct-scratch helpers (confined namespace)`.

---

## Task 6: e2e + just recipe

**Files:** Create `frontend/tests/e2e/notebook-write-isolation-kind.spec.ts`; `justfile`. Reuse `helpers.ts`.

- [ ] **Step 1: Spec** (opt-in `EGRET_E2E_KIND=1`, `test.setTimeout(300_000)`, `loginLocalAdmin`):
  1. `provisionStorageAndWarehouse` → get the warehouse id; `PUT /api/ws/{ws}/notebook-settings/warehouse { warehouse_id, namespace:["notebook"] }`.
  2. In a notebook (drive Monaco like `notebook-ecommerce.spec.ts`), run: `import daft; df = daft.from_pydict({"x":[1,2,3]}); write_iceberg(df, "scratch"); back = read_iceberg("scratch"); print("SCRATCH_ROWS", back.count_rows())` → assert `SCRATCH_ROWS 3` (direct rw round-trip).
  3. **Confinement:** run a cell that tries to write OUTSIDE the namespace via raw pyiceberg (e.g. `from pyiceberg.catalog.rest import RestCatalog; RestCatalog(...).create_table(("other","t"), ...)` using the runtime's token) → assert it raises a forbidden/403 (the guard denies the non-granted namespace). Print a marker on the caught error.
  4. Governed read still works (if an engine is available): `trino("SELECT 1")` — else note optional.

- [ ] **Step 2: `just` recipe** `e2e-kind-notebook-write` mirroring `e2e-kind` (kernel deploy) — this needs the notebook kernel stack, not Ranger. Use `deploy-kind-oidc`.

- [ ] **Step 3:** `npm run typecheck` + `eslint` clean. Commit `test(notebook): e2e — write isolation (direct rw scratch + out-of-namespace denied)`.

---

## Task 7: Verify + live-deploy + run

- [ ] **Step 1:** Full local CI: `cargo fmt --all --check`; `cargo clippy --workspace`; `DATABASE_URL=… cargo test --workspace`; FE `npm run lint && typecheck && test && build`; kernel pytest.
- [ ] **Step 2:** Rebuild + kind-load `egret-backend:dev` + `egret-kernel:dev`; `kubectl rollout restart deploy/egret` (backend has new manifest code) + delete stale kernel pods (new image). Deploy is OIDC (kernel) — no Ranger needed.
- [ ] **Step 3:** Live check: set a notebook warehouse; `write_iceberg`/`read_iceberg` round-trips; an out-of-namespace write is denied; governed read via `trino()` unaffected.

---

## Self-Review Notes
- Reuses the built authz (CatalogGuard namespace grants + `sign_claims`); net-new is wiring, one migration, and the kernel helper.
- Catalog JWT is a local RS256 mint (no refresh-token dependency, unlike the engine JWT) — works for any user.
- Confinement is two-layer: the helper only ever targets the reserved namespace, AND the token's namespace grant makes the guard reject anything else. The e2e proves the guard layer directly (raw out-of-namespace write denied).
- Known nuance (spec §6.1): the token can CREATE other namespaces (guard allows namespace creation for any write grant), but cannot write TABLES outside the reserved namespace; the helper never exposes arbitrary namespaces. Documented.
- Warehouse setting is a separate PUT (`/notebook-settings/warehouse`) to avoid conflating with the engine-enable PUT.
- Out of scope: engine reading the scratch under governance (needs an engine-role read grant), per-user namespaces, auto-provisioning a warehouse.
