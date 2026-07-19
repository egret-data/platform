# Notebook IDE Redesign — P3 (Daft-local SQL target) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox syntax.

**Goal:** Add a **"Daft (local)"** SQL target that runs `daft.sql()` over the DataFrames already bound in the kernel namespace (from prior SQL/Python cells) — a Hex-like "SQL over your dataframes" mode, no engine round-trip.

**Architecture:** Builds on P2. The `%%egret_sql` magic already dispatches `--target`. Add a special `"daft"` target that runs `daft.sql(sql, **frames)` where `frames` = the user-namespace variables that are Daft DataFrames (passed as kwargs — the current Daft API; `SQLCatalog` is deprecated). Frontend adds a "Daft (local)" option (sentinel value `"daft"`) to the SQL cell's target dropdown.

**Tech Stack:** Python (IPython magic, pytest), React + TS + Vitest. Daft API: `daft.sql("SELECT * FROM my_df", my_df=df)` (kwargs bind DataFrames by name).

**Reference spec:** `docs/en/specs/2026-07-19-notebook-ide-redesign-design.md` §5.4.

**Branch:** `feat/notebook-ide-redesign-p3` (off main, which now contains merged P1+P2).

**Note:** Daft isn't installed in `kernel-image/.venv`, so tests use a FAKE daft module (a stub with a `DataFrame` class + a `sql(query, **frames)` fn). Real `daft.sql` execution is verified only in live smoke.

---

## File Structure

| File | New/Mod | Responsibility |
|---|---|---|
| `kernel-image/egret_kernel_helpers/magic.py` | Mod | `"daft"` target branch + `run_daft_local` helper |
| `kernel-image/tests/test_magic.py` | Mod | Tests for daft dispatch + frame filtering |
| `frontend/src/components/notebook/CellEditor.tsx` | Mod | "Daft (local)" option in the target dropdown |
| `frontend/src/components/notebook/__tests__/CellEditor.test.tsx` | Mod | Assert the Daft option renders |

---

## Task 1: Kernel — Daft-local dispatch

**Files:** `kernel-image/egret_kernel_helpers/magic.py`, `kernel-image/tests/test_magic.py`. Run pytest via `kernel-image/.venv/bin/python -m pytest`.

- [ ] **Step 1: Write failing tests** — add to `tests/test_magic.py`:

```python
class _FakeDF:
    pass


class _FakeDaft:
    DataFrame = _FakeDF

    def __init__(self):
        self.calls = []

    def sql(self, query, **frames):
        self.calls.append((query, frames))
        return {"ran": query, "frames": sorted(frames)}


def test_run_daft_local_passes_only_dataframe_frames():
    daft = _FakeDaft()
    df1, df2 = _FakeDF(), _FakeDF()
    user_ns = {"a": df1, "b": df2, "n": 5, "sql": _FakeDF()}  # 'sql' is reserved → excluded
    out = M.run_daft_local("SELECT * FROM a", user_ns, daft)
    assert daft.calls == [("SELECT * FROM a", {"a": df1, "b": df2})]
    assert out == {"ran": "SELECT * FROM a", "frames": ["a", "b"]}


def test_dispatch_routes_daft_target_to_runner():
    seen = []
    dispatch = M.make_dispatch([], {"trino": lambda s, engine: None}, daft_runner=lambda sql: seen.append(sql) or "df")
    assert dispatch("daft", "SELECT 1") == "df"
    assert seen == ["SELECT 1"]


def test_dispatch_daft_without_runner_raises():
    dispatch = M.make_dispatch([], {"trino": lambda s, engine: None})  # no daft_runner
    with pytest.raises(ValueError):
        dispatch("daft", "SELECT 1")
```

- [ ] **Step 2: Run to confirm FAIL** — `.venv/bin/python -m pytest tests/test_magic.py -q` → the 3 new tests fail (`run_daft_local` missing; `make_dispatch` has no `daft_runner`).

- [ ] **Step 3: Implement** in `magic.py`:

Add the helper:
```python
# daft.sql kwargs that would collide with the query/positional params.
_DAFT_RESERVED = {"sql", "catalog", "register_globals"}


def run_daft_local(sql, user_ns, daft_module):
    """Run `daft.sql` over the Daft DataFrames currently bound in the user
    namespace, passing each as a keyword arg so the SQL can reference it by its
    variable name (`SELECT * FROM my_df`). Skips reserved kwarg names."""
    frames = {
        k: v
        for k, v in user_ns.items()
        if k not in _DAFT_RESERVED and isinstance(v, daft_module.DataFrame)
    }
    return daft_module.sql(sql, **frames)
```

Change `make_dispatch` to accept `daft_runner=None` and route `"daft"` first:
```python
def make_dispatch(engines, kind_fns, daft_runner=None):
    by_key = {}
    for e in engines or []:
        by_key[e.id] = e
        by_key[e.name] = e

    def dispatch(target, sql):
        if target == "daft":
            if daft_runner is None:
                raise ValueError("egret: Daft-local SQL is unavailable (daft not installed in this kernel)")
            return daft_runner(sql)
        e = by_key.get(target)
        if e is not None:
            fn = kind_fns.get(e.engine)
            if fn is None:
                raise ValueError(f"egret: engine kind {e.engine!r} is not runnable from notebooks")
            return fn(sql, engine=e.id)
        fn = kind_fns.get(target)
        if fn is None:
            raise ValueError(
                f"egret: unknown --target {target!r} — pick a data source from the cell's Target dropdown"
            )
        return fn(sql, engine=target)

    return dispatch
```

In `register()`, build a `daft_runner` closure and pass it into `make_dispatch`. Inside the `egret_sql` method, after computing `engines`:
```python
            def daft_runner(sql):
                import daft
                return run_daft_local(sql, self.shell.user_ns, daft)

            dispatch = make_dispatch(engines, kind_fns, daft_runner=daft_runner)
```
(Everything else in `egret_sql` — `parse_argstring`, `run_egret_sql(...)` — stays the same.)

- [ ] **Step 4: Run to confirm PASS** — `.venv/bin/python -m pytest tests/test_magic.py -q` → all pass (previous 5 + 3 new = 8). Then `.venv/bin/python -m pytest -q` (the pre-existing `sqlalchemy` failure in test_engines is unrelated — leave it).

- [ ] **Step 5: Commit**
```bash
git add kernel-image/egret_kernel_helpers/magic.py kernel-image/tests/test_magic.py
git commit -m "feat(notebook): Daft-local SQL target (daft.sql over bound DataFrames)"
```

---

## Task 2: Frontend — "Daft (local)" target option

**Files:** `frontend/src/components/notebook/CellEditor.tsx`, its test.

- [ ] **Step 1: Write failing test** — add to `CellEditor.test.tsx`:

```tsx
it("offers a Daft-local target option for SQL cells", async () => {
  render(
    <CellEditor
      {...baseProps}
      engines={[{ id: "e1", name: "Trino", engine: "trino", enabled: true }]}
      cell={cell({ id: "s", cell_type: "sql", source: "SELECT 1", cell_order: 0, sql_target: "daft", sql_into: "df" })}
    />,
  );
  // The trigger shows the selected option's label.
  expect(screen.getByLabelText("SQL target")).toHaveTextContent(/daft/i);
});
```
(Reconcile with how the shadcn `Select` renders its value — if `toHaveTextContent` on the trigger is flaky in jsdom for Radix Select, instead assert the option exists by opening the dropdown, or assert the component renders without error and the value maps. Keep the assertion meaningful: the "Daft (local)" item must be selectable.)

- [ ] **Step 2: Run to confirm FAIL** — `npm run test -- CellEditor`.

- [ ] **Step 3: Implement** — in `CellEditor.tsx`, in the SQL target `<SelectContent>`, add a fixed "Daft (local)" item BEFORE the engine list:
```tsx
              <SelectContent>
                <SelectItem value="daft">Daft (local)</SelectItem>
                {engines.filter((e) => e.enabled).map((e) => (
                  <SelectItem key={e.id} value={e.id}>
                    {e.name}
                  </SelectItem>
                ))}
              </SelectContent>
```
No other change needed: `NotebookPage.handleRun` already emits `--target ${cell.sql_target}` verbatim, so a `sql_target` of `"daft"` produces `%%egret_sql --target daft --into <var>`, which the kernel (Task 1) routes to `run_daft_local`.

- [ ] **Step 4: Run to confirm PASS** — `npm run test -- CellEditor` → all pass. `npm run typecheck` → clean.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/notebook/CellEditor.tsx frontend/src/components/notebook/__tests__/CellEditor.test.tsx
git commit -m "feat(notebook): Daft (local) target option in SQL cell dropdown"
```

---

## Task 3: Verification

- [ ] **Kernel** (`kernel-image/`): `.venv/bin/python -m pytest -q` → magic tests pass (pre-existing sqlalchemy failure noted).
- [ ] **Frontend** (`frontend/`): `npm run lint` (0 err), `npm run typecheck`, `npm run test`, `npm run build`.
- [ ] **Live smoke (manual):** bind a DataFrame in cell 1 (SQL cell `--into orders` on an engine, OR a Python cell `orders = trino("SELECT ...")`), then a second SQL cell with target **Daft (local)** running `SELECT count(*) FROM orders` → returns a result computed locally by Daft over the bound frame.
- [ ] Commit any fixups.

## Self-Review Notes
- Spec §5.4 (Daft-local stretch) → Tasks 1-2. No backend/DB/migration/OpenAPI change (target `"daft"` is a FE sentinel that never persists a new engine; `sql_target` already stores arbitrary text).
- Daft API: `daft.sql(sql, **frames)` (kwargs), NOT the deprecated `SQLCatalog`. Reserved kwarg names filtered.
- Tests use a fake daft module (daft not in the venv); real execution is live-smoke only — documented.
- `make_dispatch`'s new `daft_runner` param is optional/keyword → P2's existing call sites/tests still compile.
