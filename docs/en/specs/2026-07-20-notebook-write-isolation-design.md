# Notebook write isolation (Security e2e — Sub-project C) — Design

> Part of the "full security flow" effort. Sub-project **C** of three:
> A (Ranger auto-admin policies — **merged** #293) → B (per-user enforcement —
> **merged** #294) → **C = notebook write isolation to a per-workspace namespace
> (this doc).**

## 1. Goal

Give each workspace a **private Iceberg namespace** (in an admin-configured
"notebook warehouse"). The notebook kernel gets a **catalog token scoped to that
namespace** and Daft **reads + writes it DIRECTLY** (a fast, confined scratch
plane) — the token permits only that namespace, so a write anywhere else is
rejected by the catalog guard. **Governed/shared data is read via
Trino/StarRocks** (already the case). This is the "2-plane" model: private
direct-Iceberg scratch + governed engine reads.

## 2. Current state (verified by exploration)

- **Reads are already governed.** `trino(sql)`/`starrocks(sql)` run under the
  per-user engine JWT (`kernel-image/egret_kernel_helpers/engines.py:106,186`);
  there is NO direct Iceberg read in the kernel. The "reads via the engine"
  requirement is already met.
- **Writes go THROUGH the engine and are UNCONFINED.** `write_table(df,
  "catalog.schema.table")` runs `df.write_sql(...)` over Trino/StarRocks
  (`engines.py:210-258`), accepts ANY target, and relies on the engine's Ranger
  authz. There is no direct-Iceberg write, no private namespace, no catalog token
  for the kernel.
- **The authorization machinery already exists.** The embedded catalog's
  `EgretCatalogGuard` (`crates/egret-bin/src/main.rs:275-389`) enforces
  warehouse-prefix isolation (URL prefix must equal the token's `warehouse`
  claim's `lakekeeper_warehouse_id`, main.rs:343) + per-namespace read/write
  grants (`catalog_grant_covers`, main.rs:407-440: a `securable_type:"namespace"`
  grant matches the EXACT namespace and covers every table in it; writes require
  `privilege ∈ {write, manage}`, reads accept any). The machine-IdP mints
  `aud:"catalog"` JWTs; the kernel image already ships `pyiceberg[s3fs]` + `daft`
  (`kernel-image/Dockerfile:14`). So C is mostly WIRING these to notebooks.

## 3. Requirements (decisions from brainstorming)

| Decision | Choice |
| --- | --- |
| Write model | **Direct-Iceberg, confined** to a per-workspace namespace (not via the engine) |
| Private-ns access | **Read + write direct** (rw scratch) via a namespace-scoped catalog token |
| Where it lives | **Admin-configured notebook warehouse** (a workspace setting) + a reserved namespace |
| Granularity | **Per-workspace** namespace (shared by workspace members) |
| Governed reads | unchanged — via `trino()`/`starrocks()` |

## 4. Architecture

Six components. Backend wires the confined catalog token into the runtime
manifest; the kernel gains a direct-Iceberg rw helper.

### 4.1 Workspace setting — notebook warehouse + namespace

**Files:** `crates/egret-notebook/src/{model,store,routes}.rs` (+ a migration).

- New per-workspace setting `{ warehouse_id: Uuid, namespace: Vec<String> }`
  (namespace default `["notebook"]`). Persist in a new table
  `analytics.notebook_warehouse_settings(workspace_id PK, warehouse_id uuid,
  namespace text[])`, mirroring the existing `analytics.notebook_engine_access`
  precedent (`store.rs:543-551`).
- Surface via the existing notebook-settings API: extend `NotebookSettings`
  (`model.rs:104`) + `UpdateNotebookSettings` (`model.rs:110`) with the warehouse
  id + namespace, and `GET/PUT /api/ws/{workspace}/notebook-settings`
  (`routes.rs:93`, PUT gated `workspace:manage_members`). Validate the warehouse
  belongs to the workspace (`catalog.warehouses.workspace_id` — add a
  `list_warehouses_by_workspace` filter, not present today).

### 4.2 Per-workspace notebook catalog role + namespace-scoped rw grant

**Files:** `crates/egret-notebook` (provisioning) using
`egret-catalog-engine` store methods in-process (the HTTP handlers are
platform-admin-gated, so call the store directly).

- When the notebook warehouse is set (idempotently), ensure a `catalog_role`
  exists for the workspace's notebook plane: `create_role(warehouse_id,
  "notebook", consumer_type="notebook", …)` (`embedded.rs:235`) + `add_grant(role_id,
  GrantInput{ securable_type:"namespace", namespace:Some(namespace),
  table_name:None, privilege:"write" })` (`embedded.rs:286`). One `write`
  namespace grant = full rw on exactly that namespace (§2).
- Idempotent (`add_grant` dedupes on the grant tuple; `create_role` is
  create-or-get).

### 4.3 In-process catalog-JWT mint for the kernel

**Files:** `crates/egret-notebook/src/runtime.rs` (+ a small mint helper reusing
`egret-machine-idp`).

- Add a helper that mints an `aud:"catalog"` JWT the same way
  `machine-idp/src/lib.rs:189-198` does — `sign_claims(&keys.active().await?,
  &json!({ "iss":issuer, "sub":<notebook-consumer>, "aud":"catalog",
  "warehouse":<lakekeeper_warehouse_id>, "roles":[<notebook role>], "exp":… }))`
  (`token.rs:9`, `keys.rs:11`, `lib.rs:12`). No Keycloak/refresh-token dependency
  (unlike the engine JWT). The state needs access to the `PgSigningKeyStore`
  (already used in-process by the catalog guard).
- Scope: per-workspace (the role/grant are per-workspace); minted per
  kernel/user×workspace, short TTL (mirror the engine JWT's ~1h + a refresh
  endpoint — see §6).

### 4.4 Extend the runtime manifest + kernel env

**Files:** `crates/egret-analytics-api/src/dto.rs` (`NotebookRuntime`),
`crates/egret-notebook/src/runtime.rs` (`build_notebook_runtime`),
`kernel-image/egret_kernel_helpers/runtime.py`.

- Add to `NotebookRuntime` (`dto.rs:41-55`): `catalog_jwt: Option<String>`,
  `catalog_jwt_expires_at: Option<DateTime<Utc>>`, `catalog_uri: String` (the REST
  base = `{catalog_base_url}/catalog`, i.e. the `/catalog/v1` mount —
  `egret-server/src/lib.rs:293`), `catalog_warehouse: String` (the lakekeeper
  warehouse id the guard compares the prefix to, main.rs:343), `namespace:
  Vec<String>`.
- Populate them in `build_notebook_runtime` (`runtime.rs:26-66`) — only when the
  workspace has a notebook-warehouse setting; otherwise leave `catalog_jwt=None`
  (the write helper then errors with a clear "notebook warehouse not configured").
- `manifest_to_env` (`env.rs:18-27`) already serializes the whole manifest to
  `EGRET_NOTEBOOK_RUNTIME`; the new fields ride along. Extend
  `kernel-image/egret_kernel_helpers/runtime.py:14-26` to read them (it currently
  reads only `username` + `engines`).

### 4.5 Kernel helpers — `write_iceberg` / `read_iceberg`

**Files:** `kernel-image/egret_kernel_helpers/` (new module, exported +
injected in `__init__.py` / `startup/00-egret.py`).

- `write_iceberg(df, name, mode="append")` and `read_iceberg(name)` operate on
  the workspace's reserved namespace ONLY — the target is always
  `{namespace}.{name}` (the caller supplies just the table `name`, never a
  catalog/schema), so the helper cannot address anything outside the private ns.
- Build a `pyiceberg.catalog.rest.RestCatalog("egret", uri=<catalog_uri>,
  token=<catalog_jwt>, warehouse=<catalog_warehouse>)`; `create_namespace_if_not_exists(tuple(namespace))`;
  `create_table_if_not_exists` / `load_table`; `df.write_iceberg(tbl, mode=…)` and
  `daft.read_iceberg(tbl)`. The `token=` sets the Bearer the guard validates; the
  guard rejects any write outside the granted namespace.
- Governed reads stay on `trino()`/`starrocks()`; `write_table` (engine INSERT)
  remains for governed writes. `write_iceberg` is the NEW private-scratch plane.

### 4.6 e2e test

**Files:** `frontend/tests/e2e/notebook-write-isolation-kind.spec.ts` (new) +
a `just` recipe. Opt-in `EGRET_E2E_KIND=1`.

1. Provision a warehouse for the demo ws; set it as the notebook warehouse (`PUT
   notebook-settings`).
2. In a notebook (real kernel), `df = daft.from_pydict({...}); write_iceberg(df,
   "scratch")` → succeeds; `back = read_iceberg("scratch")` → returns the rows
   (direct rw round-trip, no engine).
3. **Confinement:** an attempt to write to a table in a DIFFERENT namespace (e.g.
   via a raw pyiceberg call to `other_ns.t`, or a helper that bypasses the forced
   namespace) is **rejected** by the catalog guard (403/forbidden). This proves
   the token can't write outside the reserved namespace.
4. Governed read still works: `trino("SELECT …")` reads shared data via the engine.

## 5. Data flow

```
admin: PUT /notebook-settings { warehouse_id, namespace }
  └─ provision: create_role("notebook") + add_grant(namespace, "write")   §4.2

kernel start (per user×workspace):
  build_notebook_runtime
    ├─ engine_jwt  (existing, Keycloak refresh — governed engine reads)
    └─ catalog_jwt = sign_claims({aud:"catalog", warehouse, roles:[notebook]})  §4.3
       + catalog_uri, catalog_warehouse, namespace  → manifest → EGRET_NOTEBOOK_RUNTIME

notebook write (private scratch):
  write_iceberg(df, "t")
    └─ RestCatalog(uri, token=catalog_jwt, warehouse) → {namespace}.t
       → EgretCatalogGuard: prefix==warehouse ✓ + namespace grant (write) ✓  → OK
       (any other namespace/table → grant miss → 403)

notebook read of governed data:  trino(sql) → per-user JWT → Trino+Ranger  (unchanged)
```

## 6. Edge cases, risks & open questions (resolve in planning)

1. **Namespace-CREATION is broader than table-write** (catalog-guard subtlety):
   `POST /{prefix}/namespaces` (create) has `target_ns=None` → the catalog-root
   branch allows it for any `write`/`manage` grant holder (main.rs:368-375). So
   the notebook token could create OTHER namespaces (not just the reserved one),
   even though it can't write TABLES outside the reserved namespace. Mitigation:
   the kernel helper only ever targets the reserved namespace (never exposes
   arbitrary namespace creation). Note as a known guard nuance; the confinement of
   DATA (tables) is exact.
2. **Notebook warehouse must exist.** The setting requires the workspace to have a
   warehouse (workspaces own N user-created warehouses, possibly zero). If unset,
   `catalog_jwt=None` and `write_iceberg` errors clearly ("configure a notebook
   warehouse in workspace settings"). No auto-provisioning in this iteration.
3. **Catalog-JWT TTL / refresh.** The token has a ~1h TTL. The kernel may outlive
   it. Mirror the engine-JWT refresh precedent (`POST
   /api/ws/{workspace}/notebook-runtime/token`, routes.rs:~820) — add a catalog
   variant, or re-mint on demand. For v1, embed in the manifest + document the TTL;
   a refresh endpoint can follow.
4. **Engine visibility of scratch (Q6):** the namespace IS visible to the
   workspace's Trino/StarRocks engine (same warehouse/REST), but the ENGINE's
   catalog role needs its OWN read grant on the namespace to query it (the two
   tokens are authorized independently). Making scratch "later queryable governed
   via the engine" is a nice-to-have — OUT OF SCOPE for v1 (add an engine-role read
   grant on the notebook namespace as a follow-up).
5. **daft ↔ pyiceberg specifics** (schema inference, `create_table_if_not_exists`
   signature, write mode). pyiceberg + daft are already installed; verify the exact
   API + any extra extra (e.g. `pyiceberg[pyarrow]`) live during implementation.
6. **Per-workspace shared scratch** means workspace members can overwrite each
   other's scratch tables (one shared namespace). Acceptable per the per-workspace
   decision; per-user isolation (`notebook_{user}`) is a possible future refinement.

## 7. Testing strategy

- **Rust unit** — the catalog-JWT mint helper produces the expected claims
  (`aud:"catalog"`, correct warehouse + role, exp); the notebook-warehouse setting
  store round-trips + validates warehouse-belongs-to-workspace; the role/grant
  provisioning is idempotent.
- **Kernel pytest** — `write_iceberg`/`read_iceberg` build the RestCatalog with the
  token + force the reserved namespace (mock pyiceberg/daft as the existing kernel
  tests mock daft).
- **e2e (kind)** — §4.6: direct rw round-trip + out-of-namespace-write denied +
  governed read via engine.
- Full local CI before merge (fmt/clippy/workspace test; FE lint/typecheck/test/
  build; kernel pytest), per convention.

## 8. Out of scope

- Auto-provisioning a notebook warehouse (admin configures an existing one).
- Engine (Trino/StarRocks) reading the scratch namespace under governance (needs an
  engine-role read grant — follow-up).
- Per-user (vs per-workspace) scratch namespaces.
- Any change to governed reads or the existing `write_table` engine-write path.
