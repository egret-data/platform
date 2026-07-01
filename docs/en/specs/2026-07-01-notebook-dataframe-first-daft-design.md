# Notebook: dataframe-first with Daft as the engine — Design

> Bilingual: English canonical; VI reference at `docs/vi/specs/2026-07-01-notebook-dataframe-first-daft-design.md`.

- **Date:** 2026-07-01
- **Status:** Approved (brainstorming) — ready for implementation plan
- **Scope:** Reorient the egret notebook around **Daft dataframes**. Daft connects
  **directly** to Iceberg, Trino, and StarRocks as the logged-in user (per-user
  short-lived JWT wired into the kernel), returning lazy Daft DataFrames the user
  composes in Python. Remove the in-notebook SQL cell type and the
  `%%egret_sql` magic. The BI plane (SQL Lab, dataset/chart/dashboard, the Rust
  patched-`mysql_async` governed path) is untouched.

## Background / current state

- The notebook kernel is Python/Daft, one Jupyter Kernel Gateway per
  (user × workspace) (`crates/egret-notebook/`, `kernel-image/`).
- Today SQL in the notebook is a **cell type** + a target dropdown
  ("SQL / Daft (in-kernel)") + the `%%egret_sql --target daft|<engine>` magic
  (`kernel-image/egret_kernel_helpers/magic.py`):
  - `--target daft` → `daft.sql(cell)` — but **no Iceberg catalog is attached** in
    `startup/00-egret.py`, so `daft.sql` only resolves DataFrames already in the
    namespace (the "over the attached Iceberg catalog" docstring is aspirational).
  - `--target trino|starrocks` → `query(engine, sql)` → `POST /api/kernel/query`
    (backend runs the SQL as the user via an F2 JWT) → `{columns, rows}` →
    `daft.from_pydict(...)`. This is **eager, materialized, capped** (10k / max
    100k rows) — no pushdown.
- This "pick engine, type SQL, see a grid" flow duplicates SQL Lab and does not
  play to the notebook's strength (dataframe composition).
- F2 already provides per-user OIDC-delegated JWTs (RFC 8693 token exchange via
  `EngineTokenMinter`, `crates/egret-notebook/src/engine_token.rs`) and a runtime
  manifest (`GET /ws/{ws}/notebook-runtime`, `crates/egret-notebook/src/runtime.rs`)
  carrying `username`, a private Iceberg namespace `notebook_{username}` with a
  scoped catalog token, the enabled `engines`/`catalogs`, and a short-lived
  `engine_jwt`. `catalog()` (`kernel-image/.../catalog.py`) returns a pyiceberg
  `RestCatalog` bound to the workspace warehouse with the user's token.
- The kernel authenticates callbacks to egret with a bearer `EGRET_KERNEL_TOKEN`
  (`KernelIdentity` extractor); identity comes only from the token, never the path.

## Feasibility (researched)

- **Iceberg — native.** `daft.read_iceberg(pyiceberg_table)` reads directly from a
  pyiceberg `Table`; the per-user catalog token already flows through `catalog()`.
  Lazy, with predicate/projection pushdown.
- **Trino — feasible.** `daft.read_sql(sql, conn)` accepts a **connection
  factory** (a callable returning a SQLAlchemy connection). A `trino://` engine
  with `connect_args={"auth": trino.auth.JWTAuthentication(jwt), "http_scheme":
  "https"}` authenticates as the user and pushes the SQL down.
- **StarRocks — unproven (Phase 0 spike).** JWT over the MySQL wire protocol needs
  a client supporting the `authentication_openid-connect_client` plugin (MySQL
  client 9.2+ / token-file handshake). Python MySQL drivers (PyMySQL, connectorx)
  do **not** support it out of the box — this is why the Rust side patched
  `mysql_async` (F2-P4b). Phase 0 must prove a Python/Daft path before the
  `starrocks()` helper commits to a transport.

Sources: Daft SQL connectors (connection factory / SQLAlchemy fallback), Trino
Python client `JWTAuthentication`, StarRocks JWT authentication docs.

## Decisions (from brainstorming)

1. **API shape — thin pre-loaded helpers.** The kernel auto-loads three helpers,
   each returning a **lazy Daft DataFrame** authenticated as the user:
   `iceberg(table)`, `trino(sql)`, `starrocks(sql)`. (A unified `read(engine, …)`
   may alias these later; not required.)
2. **StarRocks — spike first (Phase 0).** Prove Python→StarRocks with a per-user
   JWT before designing its transport. If proven → direct `read_sql`; if not →
   `starrocks()` keeps the same signature but routes through the existing governed
   callback (hybrid) until a driver lands.
3. **Remove SQL from the notebook.** Delete the SQL cell type, the target
   dropdown, and the `%%egret_sql` magic. Notebook cells are **Code + Markdown**.
4. **JWT lives in the kernel.** Each helper fetches a fresh, short-lived, per-user
   JWT so Daft connects directly. Informed trade-off vs F2's "kernel holds no
   privileged credential": the token is the user's own, downscoped, minutes-lived,
   and the user can already run SQL as themselves — so this grants no new
   authority, only a brief in-process exposure of their own token.
5. **BI plane untouched.** SQL Lab, dataset/chart/dashboard, and the Rust
   patched-`mysql_async` governed SQL keep serving BI. This design touches only
   the notebook kernel + notebook frontend.

## Architecture / components

### A. Engine-token + connection endpoint (backend)

- New callback `POST /api/kernel/engine-token` (kernel-authenticated via
  `EGRET_KERNEL_TOKEN`), body `{ engine }` (id or name). Response:
  `{ jwt, expires_at, kind: "trino"|"starrocks", host, port, catalog, scheme }`.
  - Re-validates the engine is enabled for notebooks in the caller's workspace
    (`notebook_engine_access`), mints the user's JWT via `EngineTokenMinter`
    (reusing the F2 path), and returns the engine's connection coordinates from
    the workspace engine/database registry the governed query path already uses.
- Extend the runtime manifest (`GET …/notebook-runtime`) so `engines[]` also
  carries `{ kind, host, port, catalog, scheme }` for enabled engines (so helpers
  can build connections without a round-trip for coordinates; the JWT itself is
  always fetched fresh from `engine-token`).
- Iceberg needs no new endpoint — it reuses the existing catalog token in the
  manifest via `catalog()`.

### B. Kernel helpers (`kernel-image/egret_kernel_helpers/`)

New module(s), auto-loaded in `startup/00-egret.py` alongside the existing
`query`/`catalog`/`display`/`runtime`/`me`:

- `iceberg(table, *, catalog=None) -> daft.DataFrame`
  - `daft.read_iceberg(catalog(catalog).load_table(table))`. `table` is
    `"namespace.name"`; defaults resolve within the workspace warehouse / the
    user's private `notebook_{username}` namespace. Lazy.
- `trino(sql, *, engine="trino") -> daft.DataFrame`
  - `daft.read_sql(sql, conn_factory)` where `conn_factory` builds a SQLAlchemy
    `trino://{user}@{host}:{port}/{catalog}` engine with
    `connect_args={"auth": JWTAuthentication(fresh_jwt()), "http_scheme": scheme}`.
- `starrocks(sql, *, engine="starrocks") -> daft.DataFrame`
  - Phase-0-dependent: direct `daft.read_sql` via the proven JWT MySQL connection,
    **or** hybrid fallback delegating to the existing `query()` callback with the
    identical signature/return type.
- A shared `_engine_token(engine)` helper calls `POST /kernel/engine-token`,
  caches per engine, and refreshes when within a safety margin of `expires_at`.
- Errors surface clearly (expired token → "restart/refresh"; engine not enabled →
  403 message), never silent-empty (match the existing `query.py` style).

### C. Frontend (`frontend/src/…/notebook`)

- Remove the **SQL** cell type and the **target dropdown** (the
  "SQL / Daft (in-kernel)" control). Cell type selector = **Code | Markdown**.
- Remove SQL-target state/handlers in `NotebookPage.tsx` and the cell editor.
- Existing SQL cells (dev data only): migrate/downgrade to Code cells, or drop
  (dev DB — acceptable to drop; note in the plan).

### D. Kernel image + data model cleanup

- Remove `magic.py` (`%%egret_sql`) and its registration in `startup/00-egret.py`;
  drop its tests.
- `notebook_cells.sql_target` becomes unused → drop in a follow-up migration
  (keep the column until the FE stops writing it, then remove).
- **Keep** `notebook_engine_access` / `notebook_catalog_access` — they still gate
  which engines/catalogs a user may reach through the helpers.
- Decide `POST /kernel/query`'s fate: superseded by the direct helpers; **remove
  once StarRocks-direct is proven**, otherwise keep it as the StarRocks hybrid
  transport.

## Phasing

- **Phase 0 — StarRocks-JWT spike.** In a scratch script (not shipped), stand up
  the dev StarRocks with JWT security integration + a Keycloak-minted per-user
  JWT and prove a Python/Daft `read_sql` returns rows authenticated as the user.
  Exit criteria: a documented working connection recipe **or** a clear negative
  with the blocking reason. Output decides the `starrocks()` transport.
- **Phase 1 — Backend tokens + coordinates.** `POST /kernel/engine-token` +
  manifest `engines[]` connection coordinates + `EngineTokenMinter` reuse. Tests
  (DB-gated) for enable-gating and minting.
- **Phase 2 — Helpers.** `iceberg()`, `trino()`, `starrocks()` + `_engine_token`
  cache/refresh; unit tests (mock HTTP + fake daft) mirroring the existing
  `kernel-image/tests` style.
- **Phase 3 — Remove SQL surface.** Frontend cell-type/dropdown removal, magic
  removal, data-model cleanup, Vitest/tsc/lint.
- **Phase 4 — Live verification.** Real kernel: `iceberg()` read+compose+write-back;
  `trino()` pushdown as-user; `starrocks()` per Phase 0. Full local CI.

## Testing / verification

- **Kernel helpers:** pytest in `kernel-image/tests/` — mock the HTTP token
  endpoint and the daft/SQLAlchemy calls; assert the right URL/auth/DataFrame
  wiring and error messages (no silent-empty).
- **Backend:** DB-gated tests for `/kernel/engine-token` (enable-gating, minting,
  coordinates), following `crates/egret-*/tests` patterns.
- **Frontend:** Vitest for the cell-type selector (no SQL option); tsc/lint.
- **Live (manual/opt-in):** the e2e stack + real Keycloak — a notebook cell doing
  `iceberg(...).where(...).show()`, `trino("SELECT …")`, and (Phase-0-permitting)
  `starrocks("SELECT …")`, each as the logged-in user.

## Out of scope

- BI plane (SQL Lab, dataset/chart/dashboard, Rust governed SQL) — untouched.
- Completion/hover, `.ipynb` export, multi-kernel (separate roadmap items).
- Per-user Iceberg **data-file** isolation (F2-P6) — unchanged here.

## Risks / notes

- **StarRocks-direct may fail (Phase 0).** Mitigated: `starrocks()` keeps a stable
  signature and falls back to the governed callback; the epic still ships Iceberg +
  Trino direct.
- **JWT in-kernel exposure.** Accepted trade-off (§Decisions 4): user's own,
  short-lived, downscoped token; refreshed per safety margin.
- **Trino/StarRocks reachability from the kernel.** The kernel (subprocess/pod)
  must reach the engines' host:port; true in the current dev/prod topology.
- **Long-lived kernel vs short-lived JWT.** Handled by `_engine_token`
  cache+refresh; a query that outlives a token must re-mint (Daft opens the
  connection at execution time, so the factory always uses a fresh token).
- **Iceberg predicate pushdown** depends on Daft's Iceberg reader; large scans
  without filters still pull data — document the dataframe idiom (filter early).
