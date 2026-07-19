# Notebook ergonomics — Sprint 1 (ad-hoc analysis ease-of-use) — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox syntax.

**Goal:** Make the notebook faster/frictionless for **ad-hoc analysis** (NOT a low-code/app builder). Add keyboard-driven editing, insert-cell-anywhere, auto-connect, and a schema browser to discover tables/columns — all frontend, reusing existing SQL Lab schema-browse hooks.

**Scope decision (user):** notebook = ad-hoc analysis surface; do NOT build reactive DAG / app-publish / collaboration. Just ergonomics.

**Tech Stack:** React 18 + TS, Monaco, Vitest. Reuse `src/api/sqllab.ts` hooks (`useDatabases/useCatalogs/useSchemas/useTables/useColumns`). Frontend-only (schema-browse backend already exists via SQL Lab).

**Branch:** `feat/notebook-ergonomics` (off main).

**Conventions:** FE cmds from `egret/frontend`. Commit per task. Verify with `npm run typecheck/test/lint/build`. Live-deploy to kind at the end (rebuild frontend image + `kind load` + `kubectl rollout restart deploy/egret-frontend`).

---

## Shared model (introduced in S1-3, used by S1-4/S1-5)

`NotebookPage` gains an **active-cell + mode** model:
- `activeCellId: string | null` — the selected cell.
- `mode: "edit" | "command"` — edit = the cell's editor is focused; command = keyboard drives the notebook.
- A **focus registry** `cellFocus: Ref<Record<string, () => void>>` — each cell registers `() => editor.focus()` so the page can focus a cell programmatically (parallel to the existing runner registry).
- `CellEditor` reports focus via `onFocusCell(cellId)` (Monaco `onDidFocusEditorText` / markdown focus / cell click) → sets activeCellId + mode="edit". Monaco `Esc` command → mode="command" + blur.

---

## Task S1-1: Auto-connect kernel on open

**Files:** `src/routes/explore/NotebookPage.tsx`. Test: extend `__tests__/NotebookPage.test.tsx`.

Removes the "Disconnected → click Run → wait" friction: connect the kernel when the notebook opens.

- [ ] **Step 1:** In `NotebookPage`, after the notebook data is available, add an effect that connects once on mount (best-effort, non-blocking):
```tsx
  // Connect the kernel eagerly so the first Run is instant (ad-hoc friction fix).
  useEffect(() => {
    kernel.connect().catch(() => {
      /* surfaced on the first real Run; don't toast on passive open */
    });
    // kernel is stable for the page's lifetime (keyed on ws/id remount).
  }, [kernel]);
```
Place it near the other hooks (after `kernel` is created). Do NOT block render on it.

- [ ] **Step 2:** Test — the existing suite mocks `useKernel` returning a stub with a `connect` mock. Add a test asserting `connect` is called on mount:
```tsx
it("auto-connects the kernel on open", () => {
  renderAt();
  expect(kernelMock.connect).toHaveBeenCalled(); // reconcile with the suite's kernel stub name
});
```
Reconcile `kernelMock.connect` with how the suite stubs the kernel (find the `useKernel` mock; ensure its `connect` is a `vi.fn()`).

- [ ] **Step 3:** `npm run test -- NotebookPage`, `npm run typecheck`. Commit `feat(notebook): auto-connect kernel on open`.

---

## Task S1-2: Insert cell anywhere (+ hover between cells)

**Files:** `src/routes/explore/NotebookPage.tsx`, `src/components/notebook/CellEditor.tsx` (or a small `InsertCellBar`). Test: NotebookPage test.

- [ ] **Step 1:** Add an `insertCell(type, atIndex)` helper in `NotebookPage` that inserts a cell at a position (append then reorder):
```tsx
  const insertCell = (cellType: CellType, atIndex: number) => {
    const cells = [...(notebook.data?.cells ?? [])].sort((a, b) => a.cell_order - b.cell_order);
    const maxOrder = cells.reduce((m, c) => Math.max(m, c.cell_order), -1);
    addCell.mutate(
      {
        cell_type: cellType,
        cell_order: maxOrder + 1,
        ...(cellType === "sql" ? { sql_into: defaultSqlInto(cells), sql_target: engines.find((e) => e.enabled)?.id } : {}),
      },
      {
        onSuccess: (detail) => {
          // The new cell is the highest-order one; move it to `atIndex`.
          const created = [...detail.cells].sort((a, b) => a.cell_order - b.cell_order).at(-1);
          if (!created) return;
          const order = cells.map((c) => c.id).filter((cid) => cid !== created.id);
          order.splice(Math.max(0, Math.min(atIndex, order.length)), 0, created.id);
          reorderCells.mutate(order);
        },
      },
    );
  };
```
Refactor `handleAddCell(type)` to `insertCell(type, cells.length)` (append) so the bottom buttons reuse it.

- [ ] **Step 2:** Add a hover **"+ insert"** affordance between cells. Wrap each cell with a thin insert bar shown on hover that inserts a code cell at that index:
```tsx
  // Between cells: a slim, hover-revealed insert row.
  function InsertBar({ index }: { index: number }) {
    return (
      <div className="group/insert relative h-2">
        <div className="pointer-events-none absolute inset-x-0 top-1/2 flex -translate-y-1/2 items-center justify-center opacity-0 transition-opacity group-hover/insert:opacity-100">
          <div className="h-px flex-1 bg-border" />
          <button
            type="button"
            aria-label="Insert cell here"
            onClick={() => insertCell("code", index)}
            className="pointer-events-auto mx-2 rounded-full border bg-background p-0.5 text-muted-foreground shadow-sm hover:text-foreground"
          >
            <Plus className="h-3.5 w-3.5" />
          </button>
          <div className="h-px flex-1 bg-border" />
        </div>
      </div>
    );
  }
```
Render an `<InsertBar index={i} />` before each cell (and after the last). Keep the existing bottom add buttons too.

- [ ] **Step 3:** Test — inserting via a bar calls `addCell` then `reorderCells`. Assert (with the suite's mutation mocks) that clicking an insert `+` triggers add. `npm run test -- NotebookPage`, typecheck. Commit `feat(notebook): insert cell anywhere via hover + affordance`.

---

## Task S1-3: In-editor run shortcuts + focus registry + select-next

**Files:** `src/components/notebook/CellEditor.tsx`, `src/routes/explore/NotebookPage.tsx`.

- [ ] **Step 1:** Add a **focus registry** + **active cell** to `NotebookPage`:
```tsx
  const [activeCellId, setActiveCellId] = useState<string | null>(null);
  const cellFocus = useRef<Record<string, () => void>>({});
  const focusCell = (cid: string) => {
    setActiveCellId(cid);
    cellFocus.current[cid]?.();
  };
  const selectAdjacent = (cid: string, dir: 1 | -1) => {
    const ids = sortedCells.map((c) => c.id);
    const i = ids.indexOf(cid);
    const next = ids[i + dir];
    if (next) focusCell(next);
  };
```
Pass to `ManagedCell` → `CellEditor`: `active={activeCellId === cell.id}`, `onFocusCell={() => setActiveCellId(cell.id)}`, `registerFocus={(fn) => (cellFocus.current[cell.id] = fn)}`, and `onRunAndNext={() => { handleRun(); selectAdjacent(cell.id, 1); }}`, `onRunAndBelow={() => { handleRun(); insertCell("code", index + 1); }}`.

- [ ] **Step 2:** In `CellEditor`, register focus + add Monaco run keybindings on mount:
```tsx
  // in handleMount(editor, monaco):
    registerFocus?.(() => editor.focus());
    editor.onDidFocusEditorText(() => onFocusCell?.());
    editor.addCommand(monaco.KeyMod.Shift | monaco.KeyCode.Enter, () => onRunAndNext?.());
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => runRef.current?.());
    editor.addCommand(monaco.KeyMod.Alt | monaco.KeyCode.Enter, () => onRunAndBelow?.());
    editor.addCommand(monaco.KeyCode.Escape, () => { editor.getDomNode()?.blur(); onEnterCommandMode?.(); });
```
Add the new props to `CellEditorProps`: `active?`, `onFocusCell?`, `registerFocus?`, `onRunAndNext?`, `onRunAndBelow?`, `onEnterCommandMode?`. Show an active ring: on the cell root add `${active ? "ring-2 ring-primary/40" : ""}`.

- [ ] **Step 3:** Tests — mock Monaco `onMount` to capture `addCommand` handlers OR test at the ManagedCell/page level that Shift+Enter path calls run + selectAdjacent. Keep meaningful. typecheck, `npm run test -- CellEditor NotebookPage`. Commit `feat(notebook): run shortcuts (Shift/Ctrl/Alt+Enter) + cell focus registry`.

---

## Task S1-4: Command/edit mode keyboard nav

**Files:** `src/routes/explore/NotebookPage.tsx`.

Full Jupyter-style command mode built on S1-3's activeCell + focusCell.

- [ ] **Step 1:** Add `mode` state + a page-level keydown handler that only acts in command mode (active element not an editable field):
```tsx
  const [mode, setMode] = useState<"edit" | "command">("command");
  // CellEditor.onFocusCell → setMode("edit"); Esc → setMode("command").
  const ddRef = useRef<number>(0); // for the "dd" double-press

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (mode !== "command" || !activeCellId) return;
    const cid = activeCellId;
    const idx = sortedCells.findIndex((c) => c.id === cid);
    switch (e.key) {
      case "Enter": e.preventDefault(); setMode("edit"); focusCell(cid); break;
      case "ArrowDown": case "j": e.preventDefault(); selectAdjacent(cid, 1); break;
      case "ArrowUp": case "k": e.preventDefault(); selectAdjacent(cid, -1); break;
      case "a": e.preventDefault(); insertCell("code", idx); break;      // above
      case "b": e.preventDefault(); insertCell("code", idx + 1); break;  // below
      case "d":
        if (Date.now() - ddRef.current < 400) { e.preventDefault(); handleDeleteCell(cid); }
        ddRef.current = Date.now();
        break;
    }
  };
```
Attach `onKeyDown` + `tabIndex={0}` to the scrollable `<section>`. Clicking a cell's non-editor chrome sets command mode + active. Selecting a cell in command mode should focus the section (so keys route here) — after `selectAdjacent`/select, call `sectionRef.current?.focus()` when in command mode (don't steal focus in edit mode).

- [ ] **Step 2:** Wire `CellEditor` blur/Esc → `setMode("command")` + focus the section; cell click (non-editor) → `setActiveCellId + setMode("command")`.

- [ ] **Step 3:** Tests — fireEvent.keyDown on the section with an active cell: `ArrowDown` selects next (assert activeCellId change via a visible ring or a data attr), `b` inserts below (addCell called), `dd` deletes. Keep to a couple of high-signal assertions. typecheck, test. Commit `feat(notebook): Jupyter-style command/edit keyboard nav`.

---

## Task S1-5: Schema browser panel (reuse SQL Lab hooks)

**Files:** Create `src/components/notebook/panels/SchemaBrowserPanel.tsx`; wire a third header toggle in `NotebookHeader` + `NotebookPage`. Test: SchemaBrowserPanel.

Reuse `src/api/sqllab.ts`: `useDatabases(ws)`, `useCatalogs(ws, db)`, `useSchemas(ws, db, catalog)`, `useTables(ws, db, catalog, schema)`, `useColumns(ws, db, catalog, schema, table, enabled)`. Read `src/components/sqllab/DataPanel.tsx` first to mirror its tree UX (expanders, lazy load per level).

- [ ] **Step 1:** Build a lazy tree: databases → catalogs → schemas → tables → (columns). Each node expands to fetch the next level via the hook. A **table** row has a click action "insert `SELECT * FROM <catalog>.<schema>.<table> LIMIT 100`"; a **column** row inserts its name. Insertion target = the active cell's editor (via a new `onInsertText(text)` that inserts at the cursor of the active cell), or if no active code/sql cell, create a new SQL cell seeded with the SELECT.
- [ ] **Step 2:** Insertion mechanism — extend the focus registry to an **edit registry**: `cellEdit: Ref<Record<string, (text: string) => void>>` where each CellEditor registers `(text) => editor.executeEdits(..insert at selection..)`. `SchemaBrowserPanel` calls `onInsert(text)` → NotebookPage inserts into `activeCellId`'s editor, or `insertCell("sql", …)` + set its source when none is active. (Simplest robust v1: clicking a table always **creates a new SQL cell** `SELECT * FROM … LIMIT 100` and focuses it; clicking a column inserts into the active editor if one is focused, else copies to clipboard with a toast. Pick the simpler path and note it.)
- [ ] **Step 3:** Add the header toggle: extend `NotebookPanel` to `"toc" | "data" | "schema"`; add a `TableProperties`/`Columns` lucide icon button in `NotebookHeader`; render `SchemaBrowserPanel` in the right aside for `activePanel === "schema"`.
- [ ] **Step 4:** Test — mock the sqllab hooks; assert the tree renders databases and expands to tables; clicking a table triggers the insert/create path. typecheck, `npm run test -- SchemaBrowser NotebookPage`. Commit `feat(notebook): schema browser panel with click-to-insert`.

---

## Task S1-6: Verify + live-deploy on kind

- [ ] **Step 1:** `npm run lint` (0 err) · `npm run typecheck` · `npm run test` (all pass) · `npm run build`.
- [ ] **Step 2:** Live-deploy: `docker build -f frontend/Dockerfile -t egret-frontend:dev frontend` → `kind load docker-image egret-frontend:dev --name egret` → `kubectl rollout restart deploy/egret-frontend -n egret-system && kubectl rollout status ...`.
- [ ] **Step 3:** Manual live check on `egret.localtest.me`: open a notebook — kernel auto-connects (badge → Connecting/Ready without clicking Run); Shift/Ctrl/Alt+Enter run variants; Esc→command, j/k nav, a/b insert, dd delete; hover `+` inserts between; Schema panel browses tables/columns and click-inserts a SELECT.

---

## Self-Review Notes
- All frontend; schema-browse backend already exists (SQL Lab `useDatabases/useCatalogs/...`). No migration/OpenAPI.
- Ordered by value/risk: auto-connect (S1-1) + insert (S1-2) + run-shortcuts (S1-3) ship the biggest wins first; command-mode (S1-4) + schema browser (S1-5) build on the active-cell/focus registry.
- Out of scope (deferred): schema-aware autocomplete (needs per-editor schema fetch), variable explorer, command palette ⌘K, drag-reorder — separate sprint.
- Keep it ad-hoc-focused: no app view, no reactivity.
