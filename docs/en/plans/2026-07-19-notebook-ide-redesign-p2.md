# Notebook IDE Redesign — P2 (SQL cell + DataFrame binding) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add a first-class SQL cell whose result binds to a real Daft DataFrame variable in the kernel namespace, reusing egret's existing DataFrame-first helpers (`trino()`/`starrocks()`) — so a following Python cell can reuse `sales_df.filter(...)`.

**Architecture:** A thin vertical slice. Backend re-adds `sql_target` + adds `sql_into` columns on `analytics.notebook_cells` and threads them through model/store/routes/DTO. Kernel gains a slim `%%egret_sql --target <engine> --into <var>` cell magic that calls the existing helper, assigns `user_ns[var]`, and displays. Frontend adds a SQL cell type (target dropdown + "→ variable" input) and, on Run, prepends the hidden magic line to the raw SQL.

**Tech Stack:** Rust (axum, sqlx, utoipa), Python (IPython magics, pytest), React 18 + TS + Vitest, orval codegen.

**Reference spec:** `docs/en/specs/2026-07-19-notebook-ide-redesign-design.md` §5.

**Branch:** cut `feat/notebook-ide-redesign-p2` off the P1 branch tip (P1 is unmerged; P2 depends on P1's `CellEditor`/`useNotebookRuntime` wiring). See memory `branch-off-unmerged-feature`.

**Conventions:**
- Backend commands from `egret/`; FE commands from `egret/frontend`.
- The cell table is **`analytics.notebook_cells`** (schema-qualified). The next free migration number is **0090** (0046 added `sql_target`, 0047 dropped it; unrelated migrations went up to 0089).
- Migrations are **append-only**; after adding one, touch `crates/egret-metastore/src/lib.rs` to force re-embed (known staleness gotcha).
- Backend test DB: `#[sqlx::test]` provisions a scratch DB (needs `DATABASE_URL` pointing at a reachable Postgres for `cargo test`, per repo norms).
- Regen after schema/DTO change: `cargo run -p egret-bin -- openapi > openapi.json` then (in frontend) `npm run gen:api`.
- Do not commit unless the human asks; branch is already isolated.

---

## File Structure

| File | New/Mod | Responsibility |
|---|---|---|
| `migrations/0090_notebook_cell_sql_binding.sql` | New | Re-add `sql_target`, add `sql_into` (both nullable) |
| `crates/egret-metastore/src/lib.rs` | Mod | Comment bump to re-embed migrations |
| `crates/egret-notebook/src/model.rs` | Mod | `sql_target`/`sql_into` on `Cell`, `CreateCell`, `UpdateCell` |
| `crates/egret-notebook/src/store.rs` | Mod | Persist + select the two columns; `add_cell` signature |
| `crates/egret-notebook/src/routes.rs` | Mod | Pass the two fields from `CreateCell` into `add_cell` |
| `openapi.json` + `frontend/src/api/generated/**` | Regen | DTOs carry the new fields |
| `kernel-image/egret_kernel_helpers/magic.py` | New | `%%egret_sql` magic + testable core |
| `kernel-image/startup/00-egret.py` | Mod | Register the magic |
| `kernel-image/tests/test_magic.py` | New | pytest the magic core |
| `frontend/src/api/notebooks.ts` | Mod | `Cell` fields + `useAddCell`/`useUpdateCell` + default-var helper |
| `frontend/src/components/notebook/CellEditor.tsx` | Mod | SQL cell chrome (type option, target dropdown, into input) |
| `frontend/src/routes/explore/NotebookPage.tsx` | Mod | Prepend magic on run of SQL cells; collision-free default `sql_into`; binding chip |

---

## Task 1: Backend — columns, model, store, routes (cohesive Rust slice)

This is one task because the Rust must compile as a unit (adding a struct field forces the store's row-mapping to change in the same commit).

**Files:**
- Create: `migrations/0090_notebook_cell_sql_binding.sql`
- Modify: `crates/egret-metastore/src/lib.rs`, `crates/egret-notebook/src/model.rs`, `crates/egret-notebook/src/store.rs`, `crates/egret-notebook/src/routes.rs`
- Test: add a `#[sqlx::test]` in `crates/egret-notebook/src/store.rs`'s test module.

- [ ] **Step 1: Write the migration**

Create `migrations/0090_notebook_cell_sql_binding.sql`:

```sql
-- Notebook P2: SQL cell + DataFrame binding. `sql_target` = the engine the SQL
-- runs on (e.g. 'trino'/'starrocks'); `sql_into` = the kernel variable the
-- result Daft DataFrame is bound to. Both NULL for code/markdown cells.
-- (Re-adds sql_target, dropped in 0047, alongside the new sql_into.)
ALTER TABLE analytics.notebook_cells ADD COLUMN sql_target text;
ALTER TABLE analytics.notebook_cells ADD COLUMN sql_into text;
```

- [ ] **Step 2: Force migration re-embed**

In `crates/egret-metastore/src/lib.rs`, find the top comment line that lists recent migrations (it begins `//! (touched to re-embed migrations: ...)`). Append to that list: `; 0090 re-adds notebook_cells.sql_target + adds sql_into`. (Any whitespace/comment change in this file re-triggers the embed; editing the existing note keeps it truthful.)

- [ ] **Step 3: Write the failing test** (in `crates/egret-notebook/src/store.rs`, inside the existing `#[cfg(test)] mod tests`)

Add:

```rust
    #[sqlx::test(migrations = "../../migrations")]
    async fn sql_binding_columns_roundtrip(pool: sqlx::PgPool) {
        let store = test_store(pool).await; // reuse the module's existing helper
        let nb = seed_notebook(&store).await; // reuse the module's existing helper

        // Add a SQL cell carrying target + into.
        let cid = store
            .add_cell(nb, "sql", "SELECT 1", 0, Some("trino"), Some("df"))
            .await
            .unwrap();

        let detail = store.get_with_cells(nb, owner_of(&store, nb).await).await.unwrap().unwrap();
        let cell = detail.cells.iter().find(|c| c.id == cid).unwrap();
        assert_eq!(cell.sql_target.as_deref(), Some("trino"));
        assert_eq!(cell.sql_into.as_deref(), Some("df"));

        // Update the target/into.
        let upd = UpdateCell {
            cell_type: None,
            source: None,
            sql_target: Some("starrocks".into()),
            sql_into: Some("sr_df".into()),
        };
        store.update_cell(nb, cid, &upd).await.unwrap();
        let detail = store.get_with_cells(nb, owner_of(&store, nb).await).await.unwrap().unwrap();
        let cell = detail.cells.iter().find(|c| c.id == cid).unwrap();
        assert_eq!(cell.sql_target.as_deref(), Some("starrocks"));
        assert_eq!(cell.sql_into.as_deref(), Some("sr_df"));
    }
```

NOTE: the store test module already has helpers for building a store + seeding a notebook (used by `update_cell_partial` at ~line 861). Read that test first and reuse its EXACT helper names (`test_store`/`seed_notebook`/owner lookup may be named differently — match what's there; if a helper doesn't exist, inline the minimal setup the neighboring tests use). Keep the assertions above.

- [ ] **Step 4: Run the test to verify it fails**

Run (from `egret/`): `cargo test -p egret-notebook sql_binding_columns_roundtrip`
Expected: FAIL to compile — `add_cell` takes 4 args, `UpdateCell` has no `sql_target`/`sql_into`, `Cell` has no `sql_target`/`sql_into`.

- [ ] **Step 5: Implement — model.rs**

In `crates/egret-notebook/src/model.rs`:

`Cell` (after `last_execution_time_ms`):
```rust
    pub sql_target: Option<String>,
    pub sql_into: Option<String>,
```
`CreateCell` (after `cell_order`):
```rust
    pub sql_target: Option<String>,
    pub sql_into: Option<String>,
```
`UpdateCell` (after `source`):
```rust
    pub sql_target: Option<String>,
    pub sql_into: Option<String>,
```

- [ ] **Step 6: Implement — store.rs**

`CellRow` struct (after `last_execution_time_ms`):
```rust
    sql_target: Option<String>,
    sql_into: Option<String>,
```
`CellRow::into_cell` (after `last_execution_time_ms: self.last_execution_time_ms,`):
```rust
            sql_target: self.sql_target,
            sql_into: self.sql_into,
```
The SELECT in `get_with_cells` — extend the column list:
```rust
        let cells = sqlx::query_as::<_, CellRow>(
            "SELECT id, cell_type, source, cell_order, execution_count, \
             last_output, last_execution_time_ms, sql_target, sql_into \
             FROM analytics.notebook_cells \
             WHERE notebook_id=$1 \
             ORDER BY cell_order ASC, created_at ASC",
        )
```
`add_cell` — extend signature + INSERT:
```rust
    pub async fn add_cell(
        &self,
        nb: Uuid,
        ty: &str,
        source: &str,
        order: i32,
        sql_target: Option<&str>,
        sql_into: Option<&str>,
    ) -> Result<Uuid, NotebookApiError> {
        let id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO analytics.notebook_cells \
             (notebook_id, cell_type, source, cell_order, sql_target, sql_into) \
             VALUES ($1,$2,$3,$4,$5,$6) RETURNING id",
        )
        .bind(nb)
        .bind(ty)
        .bind(source)
        .bind(order)
        .bind(sql_target)
        .bind(sql_into)
        .fetch_one(&self.db.pool)
        .await?;
        self.touch(nb).await?;
        Ok(id)
    }
```
`update_cell` — extend the UPDATE + binds:
```rust
        let result = sqlx::query(
            "UPDATE analytics.notebook_cells SET \
             cell_type   = COALESCE($3, cell_type), \
             source      = COALESCE($4, source), \
             sql_target  = COALESCE($5, sql_target), \
             sql_into    = COALESCE($6, sql_into), \
             updated_at  = now() \
             WHERE id=$1 AND notebook_id=$2",
        )
        .bind(cid)
        .bind(nb)
        .bind(p.cell_type.as_deref())
        .bind(p.source.as_deref())
        .bind(p.sql_target.as_deref())
        .bind(p.sql_into.as_deref())
        .execute(&self.db.pool)
        .await?;
```
NOTE: COALESCE means these fields can be set/changed but not cleared back to NULL via update — consistent with the existing partial-update semantics for `source`/`cell_type`. Acceptable for MVP.

If any OTHER call site of `add_cell` exists besides the route handler (grep `add_cell(` across the crate — e.g. a seed helper or the test module's `seed_notebook`), update it to pass `None, None`.

- [ ] **Step 7: Implement — routes.rs**

In the `add_cell` handler, pass the new fields:
```rust
    st.store
        .add_cell(
            id,
            &b.cell_type,
            b.source.as_deref().unwrap_or(""),
            b.cell_order.unwrap_or(0),
            b.sql_target.as_deref(),
            b.sql_into.as_deref(),
        )
        .await?;
```
`update_cell` already forwards the whole `UpdateCell` struct (`&b`) — no change needed there.

- [ ] **Step 8: Run the test to verify it passes**

Run (from `egret/`, with `DATABASE_URL` set to a reachable Postgres): `cargo test -p egret-notebook sql_binding_columns_roundtrip`
Expected: PASS. Then `cargo test -p egret-notebook` (whole crate) green.
Run `cargo fmt -p egret-notebook -p egret-metastore` and `cargo clippy -p egret-notebook -- -D warnings`.

- [ ] **Step 9: Commit**

```bash
git add migrations/0090_notebook_cell_sql_binding.sql crates/egret-metastore/src/lib.rs crates/egret-notebook/src/model.rs crates/egret-notebook/src/store.rs crates/egret-notebook/src/routes.rs
git commit -m "feat(notebook): persist sql_target + sql_into on cells (P2 backend)"
```

---

## Task 2: Regenerate the API contract

**Files:** `openapi.json`, `frontend/src/api/generated/**` (generated — do not hand-edit).

- [ ] **Step 1: Regenerate the OpenAPI spec**

Run from `egret/`: `cargo run -p egret-bin -- openapi > openapi.json`
Then confirm the new fields appear: the `CreateCell`, `UpdateCell`, and `Cell` schemas in `openapi.json` must now include `sql_target` and `sql_into`. (grep: `grep -n "sql_into" openapi.json` → expect ≥3 hits.)

- [ ] **Step 2: Regenerate the frontend client**

Run from `egret/frontend/`: `npm run gen:api`
Confirm the generated `Cell`/`CreateCell`/`UpdateCell` TS types now include `sql_target?` and `sql_into?` (grep under `src/api/generated/`).

- [ ] **Step 3: Typecheck the frontend**

Run: `npm run typecheck`
Expected: clean (the generated types changed but nothing consumes the new fields yet).

- [ ] **Step 4: Commit**

```bash
git add openapi.json frontend/src/api/generated
git commit -m "chore(notebook): regen OpenAPI + client for sql_target/sql_into"
```

---

## Task 3: Kernel — `%%egret_sql` magic

**Files:**
- Create: `kernel-image/egret_kernel_helpers/magic.py`
- Modify: `kernel-image/startup/00-egret.py`
- Test: `kernel-image/tests/test_magic.py`

Run pytest from `kernel-image/` via its venv (`.venv/bin/python -m pytest`), matching how `tests/test_engines.py` is run.

- [ ] **Step 1: Write the failing test**

Create `kernel-image/tests/test_magic.py`:

```python
import pytest
from egret_kernel_helpers import magic as M


def test_run_binds_dataframe_and_displays():
    calls = {}
    engines = {"trino": lambda sql: {"sql": sql, "engine": "trino"}}
    displayed = []
    ns = {}
    M.run_egret_sql(
        "SELECT 1", target="trino", into="df",
        user_ns=ns, engines=engines, display_fn=displayed.append,
    )
    assert ns["df"] == {"sql": "SELECT 1", "engine": "trino"}
    assert displayed == [{"sql": "SELECT 1", "engine": "trino"}]


def test_run_rejects_bad_variable_name():
    with pytest.raises(ValueError):
        M.run_egret_sql("SELECT 1", target="trino", into="9bad",
                        user_ns={}, engines={"trino": lambda s: s}, display_fn=lambda x: None)


def test_run_rejects_unknown_target():
    with pytest.raises(ValueError):
        M.run_egret_sql("SELECT 1", target="nope", into="df",
                        user_ns={}, engines={"trino": lambda s: s}, display_fn=lambda x: None)
```

- [ ] **Step 2: Run to verify it fails**

Run from `kernel-image/`: `.venv/bin/python -m pytest tests/test_magic.py -q`
Expected: FAIL (module `magic` missing).

- [ ] **Step 3: Implement the magic**

Create `kernel-image/egret_kernel_helpers/magic.py`:

```python
"""%%egret_sql cell magic — run governed SQL and bind the result as a Daft DataFrame.

    %%egret_sql --target trino --into sales_df
    SELECT * FROM egret.sales WHERE region = 'US'

Runs the SQL via the existing governed helper (trino()/starrocks()), binds the
resulting Daft DataFrame to `sales_df` in the user namespace (so later Python
cells can reuse it), and displays it. The magic line is injected by the
frontend; users only ever see clean SQL.
"""
import re

_IDENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def run_egret_sql(sql, *, target, into, user_ns, engines, display_fn):
    """Testable core: validate, dispatch to the engine helper, bind, display."""
    if not _IDENT.match(into or ""):
        raise ValueError(f"egret: invalid --into variable name {into!r} (must be a Python identifier)")
    fn = engines.get(target)
    if fn is None:
        raise ValueError(f"egret: unknown --target {target!r} (expected one of {sorted(engines)})")
    df = fn(sql)
    user_ns[into] = df
    display_fn(df)
    return df


def register(ipython):
    """Register the %%egret_sql cell magic on a live IPython shell."""
    from IPython.core.magic import Magics, magics_class, cell_magic
    from IPython.core.magic_arguments import argument, magic_arguments, parse_argstring
    from .engines import trino, starrocks
    from .display import display

    engines = {"trino": trino, "starrocks": starrocks}

    @magics_class
    class EgretSqlMagics(Magics):
        @cell_magic
        @magic_arguments()
        @argument("--target", default="trino", help="engine: trino | starrocks")
        @argument("--into", default="df", help="variable to bind the result DataFrame to")
        def egret_sql(self, line, cell):
            args = parse_argstring(self.egret_sql, line)
            run_egret_sql(
                cell, target=args.target, into=args.into,
                user_ns=self.shell.user_ns, engines=engines, display_fn=display,
            )

    ipython.register_magics(EgretSqlMagics)
```

- [ ] **Step 4: Run to verify it passes**

Run: `.venv/bin/python -m pytest tests/test_magic.py -q`
Expected: 3 passed. Then run the whole helper suite `.venv/bin/python -m pytest -q` → all pass.

- [ ] **Step 5: Register the magic at startup**

In `kernel-image/startup/00-egret.py`, after the existing import block, add registration guarded by try/except (so a bare kernel without IPython still imports cleanly):

```python
# Register the %%egret_sql cell magic (SQL cell → bound Daft DataFrame).
try:
    from egret_kernel_helpers.magic import register as _register_egret_sql
    _register_egret_sql(get_ipython())
except Exception as _e:  # noqa: BLE001
    print(f"egret sql magic not loaded: {_e}")
```

- [ ] **Step 6: Commit**

```bash
git add kernel-image/egret_kernel_helpers/magic.py kernel-image/startup/00-egret.py kernel-image/tests/test_magic.py
git commit -m "feat(notebook): %%egret_sql cell magic binds result as Daft DataFrame"
```

---

## Task 4: Frontend data layer

**Files:**
- Modify: `frontend/src/api/notebooks.ts`
- Test: `frontend/src/api/__tests__/notebooks.test.tsx` (extend) or a new small unit test for the default-var helper.

- [ ] **Step 1: Write the failing test** for the collision-free default helper.

Create `frontend/src/api/__tests__/defaultSqlInto.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { defaultSqlInto } from "@/api/notebooks";
import type { Cell } from "@/api/notebooks";

function cell(sql_into?: string): Cell {
  return {
    id: Math.random().toString(),
    cell_type: "sql",
    source: "",
    cell_order: 0,
    execution_count: null,
    last_output: null,
    last_execution_time_ms: null,
    sql_into,
  };
}

describe("defaultSqlInto", () => {
  it("returns df when none used", () => {
    expect(defaultSqlInto([])).toBe("df");
  });
  it("skips taken names", () => {
    expect(defaultSqlInto([cell("df"), cell("df_2")])).toBe("df_3");
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npm run test -- defaultSqlInto`
Expected: FAIL — `defaultSqlInto` not exported.

- [ ] **Step 3: Implement**

In `frontend/src/api/notebooks.ts`:

Extend the `Cell` interface (after `last_execution_time_ms`):
```ts
  sql_target?: string | null;
  sql_into?: string | null;
```

Extend the mutation arg types:
- `useAddCell` mutation variable type → add `sql_target?: string; sql_into?: string;`, and pass them into `notebooksAddCell(ws, id, b)` (the generated body now accepts them after Task 2).
- `useUpdateCell` variable type → add `sql_target?: string; sql_into?: string;`, and forward them in the `notebooksUpdateCell(ws, id, a.cid, { cell_type, source, sql_target, sql_into })` call.

Add the helper (exported):
```ts
/** Pick a collision-free default DataFrame variable name for a new SQL cell:
 *  `df`, then `df_2`, `df_3`, … skipping any `sql_into` already in use. */
export function defaultSqlInto(cells: Cell[]): string {
  const used = new Set(cells.map((c) => c.sql_into).filter(Boolean) as string[]);
  if (!used.has("df")) return "df";
  for (let i = 2; ; i++) {
    const candidate = `df_${i}`;
    if (!used.has(candidate)) return candidate;
  }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `npm run test -- defaultSqlInto` → PASS. Run `npm run typecheck` → clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/notebooks.ts frontend/src/api/__tests__/defaultSqlInto.test.ts
git commit -m "feat(notebook): sql_target/sql_into on cell API + default-var helper"
```

---

## Task 5: Frontend — SQL cell chrome in CellEditor

**Files:**
- Modify: `frontend/src/components/notebook/CellEditor.tsx`
- Test: `frontend/src/components/notebook/__tests__/CellEditor.test.tsx` (extend)

- [ ] **Step 1: Write the failing test**

Add to `CellEditor.test.tsx` (the file already mocks `@monaco-editor/react`):

```tsx
it("shows a target dropdown and variable input for SQL cells", () => {
  render(
    <CellEditor
      {...baseProps}
      engines={[{ id: "e1", name: "Trino", engine: "trino", enabled: true }]}
      onChangeSqlTarget={noop}
      onChangeSqlInto={noop}
      cell={cell({ id: "s", cell_type: "sql", source: "SELECT 1", cell_order: 0, sql_target: "e1", sql_into: "df" })}
    />,
  );
  expect(screen.getByLabelText(/result variable/i)).toHaveValue("df");
});
```
(Add `engines`, `onChangeSqlTarget`, `onChangeSqlInto` to the test's shared `baseProps` as no-ops / `[]` so existing tests still compile.)

- [ ] **Step 2: Run to verify it fails**

Run: `npm run test -- CellEditor`
Expected: FAIL — no result-variable input; props don't exist.

- [ ] **Step 3: Implement**

In `CellEditor.tsx`:

Add to `CellEditorProps`:
```ts
  /** Enabled engines for the SQL target dropdown (from useNotebookRuntime). */
  engines?: { id: string; name: string; engine: string; enabled: boolean }[];
  onChangeSqlTarget?: (target: string) => void;
  onChangeSqlInto?: (into: string) => void;
```

Add `"sql"` to the type `<Select>`:
```tsx
              <SelectItem value="sql">SQL</SelectItem>
```

When `cell.cell_type === "sql"`, render a second toolbar row (below the existing toolbar, above the editor) with the target dropdown + variable input:
```tsx
        {cell.cell_type === "sql" && (
          <div className="flex items-center gap-2 border-b px-2 py-1 text-xs">
            <span className="text-muted-foreground">Target</span>
            <Select
              value={cell.sql_target ?? ""}
              onValueChange={(v) => onChangeSqlTarget?.(v)}
            >
              <SelectTrigger className="h-7 w-40 text-xs" aria-label="SQL target">
                <SelectValue placeholder="Select engine" />
              </SelectTrigger>
              <SelectContent>
                {(engines ?? []).filter((e) => e.enabled).map((e) => (
                  <SelectItem key={e.id} value={e.id}>
                    {e.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <span className="ml-2 text-muted-foreground">→</span>
            <input
              aria-label="Result variable"
              value={cell.sql_into ?? ""}
              onChange={(e) => onChangeSqlInto?.(e.target.value)}
              className="h-7 w-32 rounded border bg-transparent px-2 font-mono text-xs focus:outline-none focus:ring-1 focus:ring-ring"
              placeholder="df"
            />
          </div>
        )}
```

Set the Monaco language by cell type (replace the hardcoded `python`):
```tsx
        const monacoLanguage = cell.cell_type === "sql" ? "sql" : "python";
        // ... in <Editor defaultLanguage={monacoLanguage} language={monacoLanguage} />
```
(The SQL Monaco completions were already registered in P1's `registerStaticCompletions`.)

- [ ] **Step 4: Run to verify it passes**

Run: `npm run test -- CellEditor` → PASS (all, incl. the new one). Run `npm run typecheck` → clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/notebook/CellEditor.tsx frontend/src/components/notebook/__tests__/CellEditor.test.tsx
git commit -m "feat(notebook): SQL cell chrome (target dropdown + result variable)"
```

---

## Task 6: Frontend — run wiring (magic prepend, default var, binding chip)

**Files:**
- Modify: `frontend/src/routes/explore/NotebookPage.tsx`
- Test: `frontend/src/routes/explore/__tests__/NotebookPage.test.tsx` (extend)

- [ ] **Step 1: Write the failing test**

The P1 suite has an anti-regression test asserting Run sends the RAW source with no `%%egret_sql` prefix (because P1 has no SQL cells). In P2 that becomes conditional. UPDATE that test to assert: a **code** cell still sends raw source; a **sql** cell sends source prefixed with `%%egret_sql --target … --into …`. Find the existing test that captures `kernel.execute`'s `code` argument and add a SQL-cell case:

```tsx
it("prepends the egret_sql magic when running a SQL cell", async () => {
  // Render with a notebook whose cells include one sql cell
  // (sql_target -> an engine id, sql_into: "df", source: "SELECT 1").
  // Trigger that cell's Run, then assert the kernel.execute mock was called
  // with a `code` arg starting with "%%egret_sql" and containing "--into df"
  // and ending with the raw "SELECT 1".
  // (Reconcile with how the existing suite mocks useKernel/kernel.execute.)
});
```
Reconcile the placeholder with the file's actual kernel mock (the suite already stubs a `KernelConnection` with an `execute` mock — reuse it; assert on its captured `code` argument).

- [ ] **Step 2: Run to verify it fails**

Run: `npm run test -- NotebookPage`
Expected: FAIL — SQL cells currently send raw source.

- [ ] **Step 3: Implement**

In `NotebookPage.tsx`:

(a) **Magic prepend on run.** In `ManagedCell.handleRun`, build the execute source per cell type:
```tsx
    // SQL cells run through the %%egret_sql magic so the result binds to a
    // Daft DataFrame variable; the editor shows clean SQL.
    const runSource =
      cell.cell_type === "sql"
        ? `%%egret_sql --target ${cell.sql_target ?? ""} --into ${cell.sql_into || "df"}\n${source}`
        : source;
    kernel.execute(cell.id, runSource, { /* same handlers as today */ });
```
(Keep everything else in `handleRun` identical — only the `code` argument changes.)

(b) **Pass SQL props to CellEditor.** `ManagedCell` renders `<CellEditor>`; thread the runtime engines + change handlers:
- Bring `useNotebookRuntime(ws)` into `NotebookPage` (or reuse if already present) and pass `engines={runtime.data?.engines ?? []}` down through `ManagedCell` to `CellEditor`.
- `onChangeSqlTarget={(t) => updateCellMutate({ cid: cell.id, sql_target: t })}` and `onChangeSqlInto={(v) => updateCellMutate({ cid: cell.id, sql_into: v })}` (debounce the `sql_into` text input the same way `source` is debounced, or commit on change — simplest is commit-on-change since it's low-frequency; match the file's existing mutate pattern).

(c) **Collision-free default on add.** In `handleAddCell`, when adding a SQL cell, seed `sql_into`:
```tsx
  const handleAddCell = (cellType: CellType = "code") => {
    const cells = notebook.data?.cells ?? [];
    const maxOrder = cells.reduce((m, c) => Math.max(m, c.cell_order), -1);
    addCell.mutate({
      cell_type: cellType,
      cell_order: maxOrder + 1,
      ...(cellType === "sql" ? { sql_into: defaultSqlInto(cells) } : {}),
    });
  };
```
Add an "Add SQL" button next to "Add cell"/"Add markdown":
```tsx
            <Button variant="outline" size="sm" onClick={() => handleAddCell("sql")} aria-label="Add SQL cell">
              <Plus className="h-4 w-4" />
              Add SQL
            </Button>
```

(d) **Binding chip.** Under a SQL cell's output, show the bound variable. Simplest: pass a small `bindingLabel` to `CellEditor` when `cell.cell_type === "sql" && cell.sql_into`, rendered under the outputs as `sales_df · Daft DataFrame`. (If threading a new prop is heavy, render it in `ManagedCell` right after `<CellEditor>` instead — a `<div className="px-3 pb-2 text-[11px] text-muted-foreground">{cell.sql_into} · Daft DataFrame</div>` shown only when the cell has outputs.)

Import `defaultSqlInto` from `@/api/notebooks`.

- [ ] **Step 4: Run to verify it passes**

Run: `npm run test -- NotebookPage` → PASS. Run `npm run typecheck` → clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/explore/NotebookPage.tsx frontend/src/routes/explore/__tests__/NotebookPage.test.tsx
git commit -m "feat(notebook): run SQL cells via %%egret_sql magic + binding chip"
```

---

## Task 7: Full verification

**Files:** none (verification only).

- [ ] **Step 1: Backend** (from `egret/`, `DATABASE_URL` set): `cargo fmt --all --check` (per repo CI Format step), `cargo clippy -p egret-notebook -- -D warnings`, `cargo test -p egret-notebook`. Expected: clean + green.
- [ ] **Step 2: Kernel** (from `kernel-image/`): `.venv/bin/python -m pytest -q`. Expected: all pass.
- [ ] **Step 3: Frontend** (from `egret/frontend`): `npm run lint`, `npm run typecheck`, `npm run test`, `npm run build`. Expected: 0 lint errors, clean types, all tests pass, build ok.
- [ ] **Step 4: Live smoke (manual, recommended)** — per the notebook live-e2e recipe: add a SQL cell, pick a target engine, set `into = sales_df`, Run → see the DataFrame table; in a following Python cell run `sales_df.count_rows()` (or `display(sales_df.limit(5))`) and confirm the variable is usable. Confirm the magic line never appears in the SQL editor.
- [ ] **Step 5: Commit** any lint/type fixups.

---

## Self-Review Notes (author)

- **Spec §5 coverage:** §5.1 backend → Task 1 (+regen Task 2). §5.2 kernel magic → Task 3. §5.3 FE (fields, target dropdown, into input, magic prepend, collision-free default, binding chip) → Tasks 4/5/6. §5.4 Daft-local target → **out of scope (P3)**, not in this plan.
- **Migration number:** 0090 (verified 0089 is the current latest; 0046/0047 are the old add/drop of `sql_target`).
- **Table name:** `analytics.notebook_cells` (schema-qualified, matches existing store queries) — NOT bare `notebook_cells`.
- **Type consistency:** `add_cell` grows to 6 params (Task 1); every call site (route handler + any test seed helper) updated in the same task. `UpdateCell`/`CreateCell`/`Cell` gain `sql_target`+`sql_into` consistently across Rust, OpenAPI (regen), and TS (`Cell` + both mutations). `defaultSqlInto` defined in Task 4, consumed in Task 6.
- **No placeholders** except two test bodies (Task 5's props wiring and Task 6's SQL-run assertion) that must be reconciled with the existing suite's mocks — the instruction says exactly what to assert; the implementer fills the render harness from the neighboring tests.
- **COALESCE caveat** documented: sql_target/sql_into can be set/changed but not nulled via partial update (matches existing source/cell_type semantics).
