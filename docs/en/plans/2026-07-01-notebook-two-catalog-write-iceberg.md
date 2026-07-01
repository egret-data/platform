# Notebook two-catalog + Iceberg write + dataframe e2e — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
> Bilingual: English canonical; VI reference at `docs/vi/plans/2026-07-01-notebook-two-catalog-write-iceberg.md`.

**Goal:** Give the notebook its own read-write Iceberg catalog (the workspace warehouse named `notebook`), add a `write_iceberg()` Daft helper, and add an opt-in Playwright e2e for the dataframe-first path (engine read + Iceberg write→read roundtrip).

**Architecture:** Backend: `build_notebook_runtime` selects the warehouse named `notebook` (else the first — back-compat) as the notebook catalog B; everything downstream (private namespace, rw token, manifest) is unchanged. Kernel: a `write_iceberg(df, table, mode)` helper resolves the table in the private `notebook_{user}` namespace via the existing `catalog()` rw RestCatalog, auto-creates it from the Daft DataFrame's Arrow schema if missing, then `df.write_iceberg`. The shared catalog (Trino/StarRocks) is read only via the existing `trino()`/`starrocks()` helpers.

**Tech Stack:** Rust (egret-notebook), Python 3.11 (Daft 0.7.x, pyiceberg 0.11.x, pytest), Playwright.

**Spec:** `docs/en/specs/2026-07-01-notebook-two-catalog-write-iceberg-design.md`

**Verified APIs (Daft 0.7.16 / pyiceberg 0.11.1 in the kernel env):**
- `df.write_iceberg(table: pyiceberg.table.Table, mode: str = "append")` — modes `append` / `overwrite`.
- `pyiceberg Catalog.create_table(identifier, schema)` accepts a **PyArrow schema directly** → `cat.create_table(ident, df.to_arrow().schema)`.
- Missing table raises `pyiceberg.exceptions.NoSuchTableError`.

---

## File Structure
- `crates/egret-notebook/src/runtime.rs` — add `NOTEBOOK_WAREHOUSE_NAME` + a pure `select_notebook_warehouse()` helper; use it in `build_notebook_runtime`. *(T1)*
- `kernel-image/egret_kernel_helpers/engines.py` — add `write_iceberg()` + `_resolve_identifier()`. *(T2)*
- `kernel-image/egret_kernel_helpers/__init__.py` + `startup/00-egret.py` + `tests/test_init.py` — export/auto-load `write_iceberg`. *(T3)*
- `kernel-image/tests/test_engines.py` — `write_iceberg` unit tests. *(T2)*
- `frontend/tests/e2e/notebook-dataframe.spec.ts` (new) + `frontend/tests/e2e/README.md` — opt-in e2e. *(T4)*

---

## Task 1: Backend — pick the `notebook` warehouse as catalog B

**Files:**
- Modify: `crates/egret-notebook/src/runtime.rs`

Context: `build_notebook_runtime` currently does `warehouses.into_iter().next()` (~line 81) to pick the warehouse, then provisions the private namespace + token there. `list_warehouses(ws)` returns `Vec<Warehouse>` where `Warehouse` has `id: WorkspaceId`-ish + `name: String` (the code already reads `warehouse.name` and `warehouse.id`). Extract the selection into a pure, unit-testable function.

- [ ] **Step 1: Write the failing test** — add to the `#[cfg(test)] mod tests` in `runtime.rs` (create the module if absent). `Warehouse` has no public test constructor, so `select_notebook_warehouse` is **generic over the name accessor** and the test uses plain `&str` items (no `Warehouse` construction):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_notebook_named_warehouse() {
        let whs = vec!["shared", "notebook", "other"];
        assert_eq!(select_notebook_warehouse(whs, |s| *s), Some("notebook"));
    }

    #[test]
    fn falls_back_to_first_when_no_notebook_warehouse() {
        let whs = vec!["shared", "other"];
        assert_eq!(select_notebook_warehouse(whs, |s| *s), Some("shared"));
    }

    #[test]
    fn none_when_empty() {
        let whs: Vec<&str> = Vec::new();
        assert_eq!(select_notebook_warehouse(whs, |s| *s), None);
    }
}
```

- [ ] **Step 2: Run — confirm FAIL:** `cargo test -p egret-notebook select_notebook_warehouse -- --nocapture` (function missing).

- [ ] **Step 3: Implement** in `runtime.rs` (above `build_notebook_runtime`):
```rust
/// The reserved warehouse name that designates a workspace's notebook (read-write)
/// Iceberg catalog. Other warehouses are "shared" (Trino/StarRocks).
const NOTEBOOK_WAREHOUSE_NAME: &str = "notebook";

/// Pick the notebook catalog warehouse: prefer the one named `notebook`, else the
/// first (back-compat). Generic over the name accessor so it is unit-testable
/// without constructing a full `Warehouse`.
pub(crate) fn select_notebook_warehouse<W, F>(warehouses: Vec<W>, name_of: F) -> Option<W>
where
    F: Fn(&W) -> &str,
{
    let idx = warehouses
        .iter()
        .position(|w| name_of(w) == NOTEBOOK_WAREHOUSE_NAME)
        .unwrap_or(0);
    warehouses.into_iter().nth(idx)
}
```
Then in `build_notebook_runtime`, replace `let warehouse = warehouses.into_iter().next().ok_or_else(...)` with:
```rust
    let warehouse = select_notebook_warehouse(warehouses, |w| w.name.as_str()).ok_or_else(|| {
        NotebookApiError(egret_core::Error::NotFound(
            "no warehouse configured for this workspace".into(),
        ))
    })?;
```
(Confirm `Warehouse.name` is a `String` → `.as_str()`; adjust if it's already `&str`.)

- [ ] **Step 4: Run — confirm PASS:** `cargo test -p egret-notebook select_notebook_warehouse` (3 tests pass).

- [ ] **Step 5: fmt/clippy + build the workspace** (`build_notebook_runtime`'s call site changed):
```
cargo fmt -p egret-notebook
cargo clippy -p egret-notebook --all-targets -- -D warnings
cargo build --workspace
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-notebook
```
Expected: green.

- [ ] **Step 6: Commit:**
```
git add crates/egret-notebook/src/runtime.rs
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(notebook): select the 'notebook'-named warehouse as the notebook catalog"
```

---

## Task 2: Kernel helper — `write_iceberg(df, table, mode)`

**Files:**
- Modify: `kernel-image/egret_kernel_helpers/engines.py`
- Test: `kernel-image/tests/test_engines.py`

Context: mirror the other helpers. `catalog()` (`kernel-image/egret_kernel_helpers/catalog.py`, signature `catalog(id=None, *, env=None)`) returns a pyiceberg `RestCatalog` (rw token) for the notebook warehouse B. `load_runtime(env)` (`runtime.py`) parses `EGRET_NOTEBOOK_RUNTIME`; the private namespace is `rt.catalog.namespace` (= `notebook_{username}`). Tests mock `daft`, `E.catalog`, and `E.load_runtime` (import `load_runtime` into `engines.py` at module level so the test can `monkeypatch.setattr(E, "load_runtime", ...)`). `pyiceberg` IS installed in the kernel venv, so `from pyiceberg.exceptions import NoSuchTableError` is importable — but import it lazily inside the function so the MODULE still imports without pyiceberg.

- [ ] **Step 1: Write the failing tests** — append to `tests/test_engines.py`:
```python
def _fake_df(schema="ARROW_SCHEMA"):
    class _Arrow:
        schema = schema
    class _DF:
        def to_arrow(self):
            return _Arrow()
        def write_iceberg(self, tbl, mode="append"):
            _DF.last = {"tbl": tbl, "mode": mode}
            return "WRITE_RESULT"
    return _DF()


def _install_fake_pyiceberg_exc(monkeypatch):
    # Provide a real exception class for the NoSuchTableError import path.
    import sys, types
    exc_mod = types.ModuleType("pyiceberg.exceptions")
    class NoSuchTableError(Exception):
        pass
    exc_mod.NoSuchTableError = NoSuchTableError
    pkg = types.ModuleType("pyiceberg")
    monkeypatch.setitem(sys.modules, "pyiceberg", pkg)
    monkeypatch.setitem(sys.modules, "pyiceberg.exceptions", exc_mod)
    return NoSuchTableError


def test_write_iceberg_creates_missing_table_then_writes(monkeypatch):
    NoSuchTableError = _install_fake_pyiceberg_exc(monkeypatch)
    calls = {}
    class FakeCat:
        def load_table(self, ident):
            calls["load"] = ident
            raise NoSuchTableError("nope")
        def create_table(self, ident, schema):
            calls["create"] = (ident, schema)
            return "NEW_TBL"
    monkeypatch.setattr(E, "catalog", lambda *a, **k: FakeCat())
    monkeypatch.setattr(E, "load_runtime", lambda env=None: type("R", (), {
        "catalog": type("C", (), {"namespace": "notebook_alice"})()})())
    df = _fake_df()
    out = E.write_iceberg(df, "sales", mode="overwrite")
    assert out == "WRITE_RESULT"
    assert calls["load"] == "notebook_alice.sales"          # default namespace resolution
    assert calls["create"] == ("notebook_alice.sales", "ARROW_SCHEMA")
    assert df.__class__.last == {"tbl": "NEW_TBL", "mode": "overwrite"}


def test_write_iceberg_uses_existing_table(monkeypatch):
    _install_fake_pyiceberg_exc(monkeypatch)
    class FakeCat:
        def load_table(self, ident):
            return "EXISTING_TBL"
        def create_table(self, *a):
            raise AssertionError("should not create when table exists")
    monkeypatch.setattr(E, "catalog", lambda *a, **k: FakeCat())
    df = _fake_df()
    out = E.write_iceberg(df, "notebook_bob.orders")        # explicit namespace, no default lookup
    assert out == "WRITE_RESULT"
    assert df.__class__.last == {"tbl": "EXISTING_TBL", "mode": "append"}  # default mode
```

- [ ] **Step 2: Run — confirm FAIL:** `.venv/bin/python -m pytest tests/test_engines.py -k write_iceberg -q` (function missing).

- [ ] **Step 3: Implement** in `engines.py` — add `from .runtime import load_runtime` at module top (next to `from .catalog import catalog`), then:
```python
def _resolve_identifier(table, *, env=None):
    """Resolve a table ref to "namespace.name". A bare name defaults to your
    private notebook namespace (`notebook_{username}`)."""
    if "." in table:
        return table
    rt = load_runtime(os.environ if env is None else env)
    ns = rt.catalog.namespace if rt is not None else "default"
    return f"{ns}.{table}"


def write_iceberg(df, table, *, mode="append", env=None):
    """Write a Daft DataFrame to your notebook Iceberg catalog (read-write).
    `table` is "namespace.name" (a bare name defaults to your private
    `notebook_{username}` namespace). Auto-creates the table from df's schema if
    it doesn't exist. mode="append" (default) | "overwrite". Returns the Daft
    write result."""
    from pyiceberg.exceptions import NoSuchTableError
    cat = catalog()
    ident = _resolve_identifier(table, env=env)
    try:
        tbl = cat.load_table(ident)
    except NoSuchTableError:
        tbl = cat.create_table(ident, df.to_arrow().schema)
    return df.write_iceberg(tbl, mode=mode)
```

- [ ] **Step 4: Run — confirm PASS:** `.venv/bin/python -m pytest tests/test_engines.py -q` then `.venv/bin/python -m pytest -q` (all pass).

- [ ] **Step 5: Commit:**
```
git add kernel-image/egret_kernel_helpers/engines.py kernel-image/tests/test_engines.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-helpers): write_iceberg(df, table) -> write Daft DF to notebook catalog"
```

---

## Task 3: Export + auto-load `write_iceberg`

**Files:**
- Modify: `kernel-image/egret_kernel_helpers/__init__.py`, `startup/00-egret.py`, `tests/test_init.py`

- [ ] **Step 1:** In `__init__.py`, add `write_iceberg` to `from .engines import ...` and to `__all__` (next to `iceberg`, `trino`, `starrocks`).
- [ ] **Step 2:** In `startup/00-egret.py`, add `write_iceberg` to the `from egret_kernel_helpers import ...` auto-load line.
- [ ] **Step 3:** In `tests/test_init.py`, add `"write_iceberg"` to the presence-check tuple.
- [ ] **Step 4: Run:** `.venv/bin/python -m pytest tests/test_init.py -q` and `.venv/bin/python -m pytest -q` → pass. Also `.venv/bin/python -c "import ast; ast.parse(open('startup/00-egret.py').read()); print('ok')"`.
- [ ] **Step 5: Commit:**
```
git add kernel-image/egret_kernel_helpers/__init__.py kernel-image/startup/00-egret.py kernel-image/tests/test_init.py
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(kernel-helpers): auto-load write_iceberg into the kernel namespace"
```

---

## Task 4: Opt-in dataframe-first e2e

**Files:**
- Create: `frontend/tests/e2e/notebook-dataframe.spec.ts`
- Modify: `frontend/tests/e2e/README.md`

Context: opt-in Playwright (not in CI), mirroring `notebook.spec.ts` (SSO login via `helpers.ts` `loginViaOidc`, create a notebook, set a cell's Monaco source via `setFirstCellSource`, click Run, assert output text). Read `notebook.spec.ts` + `helpers.ts` first and reuse their helpers verbatim.

- [ ] **Step 1: Write the spec** `frontend/tests/e2e/notebook-dataframe.spec.ts`:
```ts
import { test, expect, type Page } from "@playwright/test";
import { loginViaOidc } from "./helpers";

/**
 * Dataframe-first notebook e2e (opt-in — needs the live stack + a `notebook`
 * warehouse + at least one registered engine; NOT in CI). Verifies the money
 * path: read via a governed engine, and an Iceberg write->read roundtrip.
 */

async function setFirstCellSource(page: Page, code: string) {
  await page.waitForFunction(() => (window as any).monaco?.editor?.getEditors?.().length > 0, null, { timeout: 20_000 });
  await page.evaluate((src) => {
    (window as any).monaco.editor.getEditors()[0].setValue(src);
  }, code);
}

async function newNotebook(page: Page, name: string) {
  await page.getByRole("link", { name: "Notebooks" }).click();
  await page.getByRole("button", { name: "+ Notebook" }).click();
  await page.getByRole("textbox", { name: "Name" }).fill(name);
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await page.waitForURL(/\/notebooks\/[0-9a-f-]{36}$/, { timeout: 20_000 });
}

test("iceberg write -> read roundtrip via Daft", async ({ page }) => {
  await loginViaOidc(page);
  await newNotebook(page, `df-e2e ${Date.now()}`);
  const marker = `egret-df-${Date.now()}`;
  await setFirstCellSource(page, [
    "import daft",
    `df = daft.from_pydict({"who": ["${marker}"], "n": [42]})`,
    'write_iceberg(df, "e2e_roundtrip", mode="overwrite")',
    'iceberg("e2e_roundtrip").show()',
  ].join("\n"));
  await page.getByRole("button", { name: "Run", exact: true }).first().click();
  await expect(page.getByText(marker)).toBeVisible({ timeout: 60_000 });
});
```
(If the workspace also has a registered engine, a second test can run `trino("SELECT 1 AS n").show()` and assert `1`; keep it in the same spec but note in the README it needs an enabled engine.)

- [ ] **Step 2: README** — add a "Dataframe-first spec" section to `frontend/tests/e2e/README.md`: the target workspace needs a warehouse **named `notebook`** (catalog B) with a provisioned storage connection (so `write_iceberg`/`iceberg` have a rw catalog), and — for the optional engine-read test — at least one registered+enabled engine. Note it's opt-in like `notebook.spec.ts`.

- [ ] **Step 3: Type-check** (don't run the live e2e): `cd frontend && npx tsc --noEmit` and `npx eslint tests/e2e/notebook-dataframe.spec.ts` → clean. (Playwright specs aren't type-checked by the main tsconfig; at minimum eslint + a syntax check. Mirror how `notebook.spec.ts` is treated.)

- [ ] **Step 4: Commit:**
```
git add frontend/tests/e2e/notebook-dataframe.spec.ts frontend/tests/e2e/README.md
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "test(e2e): opt-in dataframe-first spec (engine read + iceberg write->read)"
```

---

## Task 5: Verify + finish

**Files:** none.

- [ ] **Step 1: Backend + helper unit tests:**
```
cargo fmt --all -- --check
cargo clippy -p egret-notebook --all-targets -- -D warnings
DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-notebook
cd kernel-image && .venv/bin/python -m pytest -q
```
Expected: green.

- [ ] **Step 2: (Optional) live write_iceberg smoke** — in the kernel venv, with a real catalog token (seed as in the dataframe-first live-verify recipe): `write_iceberg(daft.from_pydict({"n":[1]}), "smoke", mode="overwrite")` then `iceberg("smoke").show()`. Only if a `notebook` warehouse + catalog token are available; otherwise rely on unit tests + the opt-in e2e.

- [ ] **Step 3: Full CI + finish** — run the full local CI (backend fmt/clippy/test + frontend lint/tsc/vitest/build + kernel pytest), then a final review, then `superpowers:finishing-a-development-branch`.

---

## Notes / risks
- **`Warehouse.name` type** — Step 3 of T1 assumes `String` (`.as_str()`); if it's `&str`, drop `.as_str()`. The generic `select_notebook_warehouse` sidesteps this in the test.
- **Daft write API is pinned** (0.7.16): `df.write_iceberg(pyiceberg_table, mode)`; pyiceberg `create_table(ident, pa.Schema)`. If the kernel image bumps Daft, re-verify the signature.
- **Back-compat:** a workspace without a `notebook` warehouse still works (first warehouse) — writes just land there; the README recommends creating the `notebook` warehouse to isolate notebook rw from shared BI.
- **e2e is opt-in** (needs live stack + `notebook` warehouse + engine) — same posture as `notebook.spec.ts`, not in CI.
