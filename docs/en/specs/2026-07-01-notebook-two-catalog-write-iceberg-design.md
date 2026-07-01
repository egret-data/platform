# Notebook: two-catalog model + Iceberg write + dataframe e2e — Design

> Bilingual: English canonical; VI reference at `docs/vi/specs/2026-07-01-notebook-two-catalog-write-iceberg-design.md`.

- **Date:** 2026-07-01
- **Status:** Approved (brainstorming) — ready for implementation plan
- **Scope:** Give the notebook its own read-write Iceberg catalog (separate from the shared catalog Trino/StarRocks query), add a `write_iceberg()` Daft helper, and write an opt-in Playwright e2e that exercises the dataframe-first path (engine reads + an Iceberg write→read roundtrip).

## Background / current state

The dataframe-first notebook shipped (PRs #110/#111/#112): kernel helpers `iceberg(table)` / `trino(sql)` / `starrocks(sql)` return lazy Daft DataFrames as the user; `POST /kernel/engine-token` mints per-user JWTs; the old SQL surface is gone. See `docs/en/specs/2026-07-01-notebook-dataframe-first-daft-design.md`.

Catalog facts today:
- A workspace can already have **multiple warehouses** (`workspace_warehouses`).
- `build_notebook_runtime` (`crates/egret-notebook/src/runtime.rs`) picks the **first** warehouse by name, provisions the private namespace `notebook_{username}` + a fresh **1-grant rw catalog token** there (`provision_notebook_access`, `provisioning.rs`), and returns it as `RuntimeCatalog {url, warehouse, namespace, token}`.
- `catalog()` (`kernel-image/egret_kernel_helpers/catalog.py`) → a pyiceberg `RestCatalog` bound to that warehouse with the rw token. `iceberg(table)` reads via `catalog().load_table(table)`.
- There is **no write helper** today.
- Trino/StarRocks read whatever Iceberg catalog their connectors are configured against.

## Decisions (from brainstorming)

1. **Read/write routing:** the notebook reads **shared** data ONLY via `trino()`/`starrocks()` (governed SQL). `iceberg()` (read) and the new `write_iceberg()` (write) operate ONLY on the notebook's own catalog **B**. The notebook does not iceberg-direct-read the shared catalog.
2. **Catalog B is designated by name convention:** the workspace warehouse literally named **`notebook`** is the notebook catalog. Other warehouses are "shared" (for Trino/StarRocks). If no `notebook`-named warehouse exists, fall back to the current behavior (first warehouse) for back-compat.
3. **`write_iceberg(df, table, mode="append")`** — auto-creates the table from the DataFrame schema if missing, then writes; modes `append` (default) and `overwrite`. Writes into the private `notebook_{username}` namespace (rw) in B.
4. **e2e is opt-in** (like the existing `notebook.spec.ts`) — needs the live stack + registered engines + a `notebook` warehouse; NOT in CI.

## Architecture / components

### A. Backend — pick catalog B by name (`crates/egret-notebook/src/runtime.rs`)

`build_notebook_runtime` currently does `warehouses.into_iter().next()` (first). Change the selection to: **prefer the warehouse whose `name == "notebook"`; otherwise the first** (unchanged fallback). Everything downstream (`provision_notebook_access`, the returned `RuntimeCatalog`, the rw token, the `notebook_{username}` namespace) is unchanged — it just targets warehouse B. Add a small named constant `NOTEBOOK_WAREHOUSE_NAME = "notebook"`.

- This is the ONLY backend change. `catalog()`/`iceberg()`/`write_iceberg()` all consume the manifest's `RuntimeCatalog`, so they automatically target B.
- Doc note: a workspace intending to use notebook Iceberg rw should have a warehouse named `notebook` (the shared BI warehouse(s) keep their own names and back Trino/StarRocks).

### B. Kernel helper — `write_iceberg()` (`kernel-image/egret_kernel_helpers/engines.py` or a new `iceberg_io.py`)

```python
def write_iceberg(df, table, *, mode="append"):
    """Write a Daft DataFrame to your notebook Iceberg catalog (read-write).
    `table` is "namespace.name" (defaults to your private notebook_{username}
    namespace). Auto-creates the table from df's schema if it does not exist.
    mode="append" (default) | "overwrite"."""
```
Behavior:
- Resolve the catalog via the existing `catalog()` (rw RestCatalog for B).
- Resolve/normalize `table` to `(namespace, name)`; default namespace = the runtime's private `notebook_{username}` (available from the runtime manifest / `catalog()` context).
- If the table does not exist: create it from the Daft DataFrame's schema (derive the pyiceberg schema from the df's Arrow schema — `df.to_arrow().schema` → pyiceberg schema, or Daft's Iceberg integration if it exposes create).
- Write: `df.write_iceberg(pyiceberg_table, mode=mode)` (Daft's Iceberg writer; `append`/`overwrite`).
- Return the write result / a small summary (rows written) if Daft returns one; never silent-fail (clear `egret:`-prefixed errors, matching the other helpers).
- Export + auto-load `write_iceberg` alongside `iceberg`/`trino`/`starrocks` in `__init__.py` + `startup/00-egret.py`.

### C. e2e — dataframe-first Playwright spec (`frontend/tests/e2e/notebook-dataframe.spec.ts`)

Opt-in (manual), reusing `helpers.ts` `loginViaOidc`. Steps: SSO login → create a notebook → run cells and assert outputs:
1. `trino("SELECT 1 AS n").show()` (and/or `starrocks(...)`) → assert the value renders. (At least one governed engine; gated on the workspace having that engine registered.)
2. **Write→read roundtrip:** a cell that builds a small Daft df, `write_iceberg(df, "e2e_roundtrip", mode="overwrite")`, then `iceberg("e2e_roundtrip").show()` → assert the written rows render.
Update `frontend/tests/e2e/README.md`: the target workspace needs (a) a warehouse named `notebook` (catalog B) with a provisioned storage connection, and (b) at least one registered+enabled engine for the engine-read cell.

## Testing

- **Backend (Rust, DB-gated):** a `build_notebook_runtime`/warehouse-selection test — with two warehouses (`shared` + `notebook`), the runtime picks `notebook`; with only one, it picks that one (back-compat).
- **Kernel helper (pytest):** `write_iceberg` unit tests (mock `daft` + `catalog()`): asserts create-if-missing path (table absent → create called from schema, then write), append vs overwrite mode passed to `df.write_iceberg`, and the default-namespace resolution. Mirror the existing `test_engines.py` mocking style. `test_init` gains `write_iceberg`.
- **e2e:** opt-in spec above (not CI).

## Out of scope

- Iceberg-direct read of the shared catalog from the notebook (decided: shared via SQL only).
- Changing how Trino/StarRocks are configured against catalog A.
- Auto-provisioning the `notebook` warehouse (admin creates it via the existing Storage→Catalog flow; we only document it).
- Schema evolution / partition specs on write (append/overwrite of a flat table only; MVP).

## Risks / notes

- **Daft→pyiceberg schema derivation** for auto-create is the trickiest bit — confirm the exact Daft API (`df.write_iceberg` may create-on-write, or we create via pyiceberg `catalog.create_table(identifier, schema)` from the Arrow schema). The plan must pin the concrete calls against the installed Daft/pyiceberg versions (Daft 0.7.x, pyiceberg).
- **Name-convention fallback:** a workspace without a `notebook` warehouse keeps working (uses the first warehouse) — but then writes land in whatever the first warehouse is; document that the `notebook` warehouse is recommended to keep notebook rw isolated from shared BI data.
- **e2e write cleanup:** the roundtrip uses `overwrite` on a fixed table name so reruns are idempotent; no residual-state assertions.
