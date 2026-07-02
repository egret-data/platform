# REST Catalog Redesign — Plan 4: complete the pivot (remove Daft-direct + cleanup) Implementation Plan

> ⛔ **SUPERSEDED (2026-07-02):** replaced by the control-plane Polaris direction — see `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. The embedded-lakekeeper approach here is dropped (Plans 1 & 4 void; the `write_table` helper is kept). Retained for history.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> Spec: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md` (revised single-catalog pivot). This is the consolidated plan for the remaining code work — Workstreams **R** (remove Daft-direct), **C** (single-catalog cleanup), **F** (legacy removal). **A + B (base_url + config split) are in Plan 1** (`…-p1-base-url-config.md`); the write helper (**W**) is already done + committed. Plans 2 & 3 are SUPERSEDED.
> VI reference: `docs/vi/plans/2026-07-02-rest-catalog-redesign-p4-remove-daft-direct-cleanup.md`.

**Goal:** Make the notebook engine-only: remove every Daft-direct Iceberg path (`iceberg`/`write_iceberg`/`catalog`) and the per-user catalog provisioning that fed them, rewrite the e2e to `write_table`, and delete the legacy hand-rolled catalog — leaving one shared, engine-facing catalog.

**Architecture:** Delete the kernel `catalog()`/`iceberg()`/`write_iceberg()` helpers + their manifest (`RuntimeCatalog`); drop `build_notebook_runtime`'s warehouse selection + `provision_notebook_access` (+ now-dead `select_notebook_warehouse`, `NamespaceProvisioner`); rewrite `notebook-dataframe.spec.ts` to `trino()` + `write_table`; verify-then-remove the dead hand-rolled catalog.

**Tech Stack:** Python (kernel helpers, pytest), Rust (`egret-notebook`, `egret-catalog`, `egret-iceberg-ext`), Playwright (e2e), codegraph.

---

## Background the implementer needs

- Kernel helpers (`kernel-image/egret_kernel_helpers/`): `engines.py` has `iceberg()` (line ~267, `catalog().load_table(_resolve_identifier(...))`), `write_iceberg()` (~line 288, uses `catalog()`), and `_resolve_identifier()`. `catalog()` lives in `catalog.py` and is used ONLY by those two. `runtime.py` has a `RuntimeCatalog` class (`url/warehouse/namespace/token`) and `Runtime.catalog = RuntimeCatalog(d["catalog"])`. Exports in `__init__.py` (imports + `__all__`) and `startup/00-egret.py`. Tests: `tests/test_catalog.py`, iceberg/write_iceberg tests in `tests/test_engines.py`, `tests/test_runtime.py`, `tests/test_init.py`.
- Backend (`egret-notebook`): `build_notebook_runtime` (`runtime.rs:75+`) calls `select_notebook_warehouse` (`runtime.rs:100`) then `provision_notebook_access` (`runtime.rs:107`) and returns a `NotebookRuntime` whose manifest carries `catalog: RuntimeCatalog {url, warehouse, namespace, token}` plus `username`, `engines`, `catalogs`, `engine_jwt`, `engine_jwt_expires_at`. `provisioning.rs` defines `provision_notebook_access` + `Provisioned` + the `NamespaceProvisioner` trait (mint-and-revoke + `ensure_namespace`). `lib.rs:11` re-exports `provision_notebook_access, NamespaceProvisioner, Provisioned`. `NotebookState.provisioner` (`routes.rs`) is the injected impl.
- After the pivot the notebook needs **no** catalog credential and **no** private namespace (it writes to the shared catalog via Trino, and `write_table` `CREATE SCHEMA`s as needed). So the whole catalog-provisioning path is dead.
- e2e: `frontend/tests/e2e/notebook-dataframe.spec.ts` currently runs `write_iceberg(df, …)` + `iceberg(…)`.
- Legacy: an earlier hand-rolled Iceberg REST surface (`egret-catalog::api` / `egret-iceberg-ext` types) was reported by codegraph but `ls crates/egret-catalog/src/api/` found nothing — its exact location is **uncertain**, so Task 5 is verify-then-remove.

---

## Task 1: Remove the Daft-direct kernel helpers

**Files:**
- Modify: `kernel-image/egret_kernel_helpers/engines.py`
- Delete: `kernel-image/egret_kernel_helpers/catalog.py`, `kernel-image/tests/test_catalog.py`
- Modify: `kernel-image/egret_kernel_helpers/__init__.py`, `kernel-image/startup/00-egret.py`, `kernel-image/tests/test_engines.py`, `kernel-image/tests/test_init.py`

- [ ] **Step 1: Delete the iceberg helpers from `engines.py`**

Remove the functions `iceberg(table, *, env=None)`, `write_iceberg(df, table, *, mode=..., env=None)`, and `_resolve_identifier(table, *, env=None)` entirely. Keep `trino`/`starrocks`/`write_table` + the `_trino_conn`/`_starrocks_conn` factories + token helpers.

- [ ] **Step 2: Delete `catalog.py` and its test**

Run: `git rm kernel-image/egret_kernel_helpers/catalog.py kernel-image/tests/test_catalog.py`

- [ ] **Step 3: Fix the exports**

In `__init__.py`: change the import line to `from .engines import starrocks, trino, write_table  # noqa: F401` (drop `iceberg`, `write_iceberg`); remove the `from .catalog import catalog` line; update the module docstring; set `__all__ = ["display", "load_runtime", "runtime", "me", "trino", "starrocks", "write_table"]`.
In `startup/00-egret.py`: change the import to `from egret_kernel_helpers import display, runtime, me, trino, starrocks, write_table  # noqa: F401`.

- [ ] **Step 4: Remove the dead tests**

In `test_engines.py`: delete the `iceberg`/`write_iceberg` tests (the `_fake_df` / `FakeCat` helpers and every `test_*iceberg*`). In `test_init.py`: drop `catalog`/`iceberg`/`write_iceberg` from the expected-exports assertion, add nothing new (write_table already asserted).

- [ ] **Step 5: Run the kernel tests**

Run: `cd kernel-image && python -m pytest tests/ -q`
Expected: PASS with no import errors (no references to `catalog`/`iceberg`/`write_iceberg`).

- [ ] **Step 6: Commit**

```bash
git add kernel-image/egret_kernel_helpers/engines.py kernel-image/egret_kernel_helpers/__init__.py kernel-image/startup/00-egret.py kernel-image/tests/
git commit -m "refactor(kernel): remove Daft-direct iceberg/write_iceberg/catalog helpers"
```

---

## Task 2: Drop `RuntimeCatalog` from the kernel manifest reader

**Files:**
- Modify: `kernel-image/egret_kernel_helpers/runtime.py`, `kernel-image/tests/test_runtime.py`

- [ ] **Step 1: Update the failing test**

In `test_runtime.py`, remove any assertion on `runtime.catalog` / the `catalog` manifest key; keep `username`/`engines` assertions.

- [ ] **Step 2: Remove `RuntimeCatalog`**

In `runtime.py`: delete the `RuntimeCatalog` class and the `self.catalog = RuntimeCatalog(d["catalog"])` line in `Runtime.__init__`. Keep `username`, `engines`, `catalogs`.

- [ ] **Step 3: Run**

Run: `cd kernel-image && python -m pytest tests/test_runtime.py tests/test_init.py -q`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add kernel-image/egret_kernel_helpers/runtime.py kernel-image/tests/test_runtime.py
git commit -m "refactor(kernel): drop RuntimeCatalog from the runtime manifest (no direct catalog)"
```

---

## Task 3: Remove the notebook catalog provisioning in the backend

**Files:**
- Modify: `crates/egret-notebook/src/runtime.rs`, `crates/egret-notebook/src/lib.rs`, `crates/egret-notebook/src/routes.rs`
- Delete/gut: `crates/egret-notebook/src/provisioning.rs`
- Test: `crates/egret-notebook/src/runtime.rs`

- [ ] **Step 1: Remove the `catalog` field from the `NotebookRuntime` manifest**

Locate the manifest struct (`rg "struct NotebookRuntime" crates/egret-notebook/src`) and delete its `catalog: RuntimeCatalog` field; delete the `RuntimeCatalog` struct definition. Keep `username`, `engines`, `catalogs`, `engine_jwt`, `engine_jwt_expires_at`.

- [ ] **Step 2: Simplify `build_notebook_runtime`**

In `runtime.rs`, remove: the `list_warehouses` + `select_notebook_warehouse` block (lines ~88–104), the `provision_notebook_access(...)` call (lines ~107+), and the `catalog: RuntimeCatalog { … prov … }` in the returned struct. Keep resolving `username` (via `ensure_username`), the enabled `engines`/`catalogs`, and the `engine_jwt` mint. Also delete the `select_notebook_warehouse` fn + its three unit tests + the `NOTEBOOK_WAREHOUSE_*` const (all now unused).

- [ ] **Step 3: Delete the dead provisioning module**

Remove `provision_notebook_access`, `Provisioned`, and — if no other caller remains (`rg "NamespaceProvisioner|ensure_namespace|\.provisioner" crates`) — the `NamespaceProvisioner` trait and `NotebookState.provisioner` field (+ its wiring in `egret-bin`). If `NamespaceProvisioner` is still used elsewhere, keep only that; otherwise `git rm crates/egret-notebook/src/provisioning.rs`. Update `lib.rs:11` re-exports accordingly.

- [ ] **Step 4: Build + test**

Run: `cargo build -p egret-notebook && DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-notebook`
Expected: builds (no unused-import/dead-code warnings for the removed items) and tests pass. Fix any `egret-bin` wiring the removed `provisioner`/manifest field leaves dangling.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-notebook crates/egret-bin
git commit -m "refactor(notebook): drop catalog provisioning + warehouse selection (engine-only notebook)"
```

---

## Task 4: Rewrite the dataframe e2e to `write_table` + `trino`

**Files:**
- Modify: `frontend/tests/e2e/notebook-dataframe.spec.ts`, `frontend/tests/e2e/README.md`

- [ ] **Step 1: Rewrite the roundtrip cell**

Replace the `write_iceberg`/`iceberg` cell with an engine roundtrip. Cell source (marker `egret-df-<ts>`):

```python
import daft
df = daft.from_pydict({"who": ["${marker}"], "n": [6 * 7]})
write_table(df, "iceberg.e2e.roundtrip", engine="trino", mode="overwrite")
row = trino("SELECT who, n FROM iceberg.e2e.roundtrip").to_pydict()
print(row["who"][0], row["n"][0])
```

Assert `getByText(`${marker} 42`)` (output-only, mirrors the existing pattern). Keep the reopen-and-re-run guard (now trivially green — no per-kernel catalog token to revoke).

- [ ] **Step 2: Update the e2e README with the required setup**

Document that this opt-in spec needs: (a) a Trino **iceberg** catalog registered against egret's lakekeeper (REST + an opaque service token + MinIO S3 — the spike recipe in memory `egret-catalog-single-catalog-pivot`), and (b) a Trino group provider (or relaxed ACL) so the SSO user can write.

- [ ] **Step 3: Typecheck the spec**

Run: `cd frontend && npx tsc --noEmit -p tests/e2e/tsconfig.json` (or the project's e2e typecheck)
Expected: clean. (The spec is opt-in; a full live run needs the setup from Step 2.)

- [ ] **Step 4: Commit**

```bash
git add frontend/tests/e2e/notebook-dataframe.spec.ts frontend/tests/e2e/README.md
git commit -m "test(e2e): dataframe roundtrip via write_table + trino (engine-only)"
```

---

## Task 5: Remove the legacy hand-rolled catalog (verify-then-remove)

**Files:**
- Modify/Delete: `crates/egret-catalog` (any hand-rolled REST module), `crates/egret-iceberg-ext`

- [ ] **Step 1: Establish what is actually live**

Confirm the live catalog path is only the lakekeeper engine + `egret-catalog::CatalogStore` (tokens + warehouse). For each candidate-dead symbol (the `CatalogConfig`/`CreateTableRequest`/`LoadTableResult`/… in `egret-iceberg-ext`, and any `get_config`/table handlers), run `codegraph_impact <symbol>` and `rg <symbol> crates` to confirm **no live caller** (only tests / each other).

- [ ] **Step 2: Remove confirmed-dead code**

Delete only the symbols/modules with zero live callers (and their tests). Do NOT remove `CatalogStore` (tokens + warehouse mapping — used by `front_auth`). If `egret-iceberg-ext` becomes entirely unused, remove the crate from the workspace `Cargo.toml` members + any `dep:` references; otherwise trim the unused types only.

- [ ] **Step 3: Build the workspace**

Run: `cargo build --workspace`
Expected: builds. Anything that fails to compile was a live dependency — restore it and narrow the removal.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore(catalog): remove dead hand-rolled Iceberg REST (superseded by lakekeeper)"
```

---

## Task 6: Workspace verification

- [ ] **Step 1: Backend fmt + clippy + tests**

Run:
```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-notebook -p egret-catalog -p egret-catalog-engine
```
Expected: all clean/pass (no dead-code warnings from the removals).

- [ ] **Step 2: Kernel + frontend**

Run: `cd kernel-image && python -m pytest tests/ -q` and `cd frontend && npm run typecheck && npm run lint && npm run build`
Expected: clean.

---

## Self-review notes (spec coverage for Plan 4)

- **R** (remove Daft-direct): Tasks 1, 2, 3.
- **C** (single-catalog cleanup): Task 3 (drop warehouse selection + provisioning) — no per-user catalog credential remains.
- **F** (legacy removal): Task 5.
- e2e rewrite: Task 4.
- Out of this plan: **A + B** (Plan 1 — base_url fork + config split), and the **ops prerequisites** (Trino iceberg-catalog registration + JWT→group provider), documented in the spec.
