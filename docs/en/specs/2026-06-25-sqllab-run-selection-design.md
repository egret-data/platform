# SQL Lab — Run Selection + Keyboard Shortcut (Design)

**Status:** Approved
**Date:** 2026-06-25
**Scope:** Frontend only (React/TS). No backend / API changes.
**Sub-project 1 of 4** in the SQL Lab → Superset parity effort (the others: table preview + searchable tree; results polish; extras — each its own spec/plan).

## Problem

egret SQL Lab runs only the whole editor text, via the Run button, with no keyboard
shortcut. Superset's core interaction is **Run / Run selection** (execute the highlighted
SQL) plus **`Ctrl/Cmd+Enter`**. This is the single most-used SQL Lab ergonomic and egret
lacks it.

## Goals

- If text is **selected** in the editor, run **only the selection**; otherwise run the
  whole editor. The Run button label reflects this ("Run selection" vs "Run").
- **`Cmd/Ctrl + Enter`** runs (selection-aware, identical to the button).
- Both no-op while a query is running.

## Non-Goals

- No `Cmd+Shift+Enter` ("run all while a selection exists") — YAGNI for now.
- No full keyboard-shortcuts help modal (Superset has one) — a tooltip on Run suffices.
- No change to the query submission API or async polling (unchanged).

## Current State (anchors)

- `components/sqllab/SqlEditor.tsx` — wraps `@monaco-editor/react` `Editor`. `onMount` gets
  `(editor, monaco)` and registers a completion provider; today the `editor` arg is unused
  (`_editor`). Disposed via `disposableRef`.
- `components/sqllab/SqlTab.tsx`:
  - `run()` (no args) submits `{ database_id: tab.dbId, sql: tab.sql, limit: tab.limit }`,
    guarded by `if (!tab.dbId)`; `running` derives from the query status.
  - The Run button: `<Button size="sm" onClick={run} disabled={running}>Run</Button>`.
  - Renders `<SqlEditor value={tab.sql} onChange={(v) => patch({ sql: v })} schemaWords={words} />`.
- Tests mock `@monaco-editor/react` as a `<textarea data-testid="sql">`, so `SqlEditor`'s
  `onMount` (keybindings, selection listener) never fires in unit tests. The selection logic
  must therefore live in a **pure helper** and the SqlTab integration must be testable via a
  `SqlEditor` stub.

## Design

### A. Pure helper `pickRunSql`

New module `components/sqllab/pickRunSql.ts`:
```ts
/** The SQL to execute: the selected text when it has non-whitespace content, else the full editor text. */
export function pickRunSql(fullText: string, selectedText: string | null | undefined): string {
  const sel = selectedText ?? "";
  return sel.trim() ? sel : fullText;
}
```

### B. `SqlEditor` exposes run + selection

Add two optional props:
```ts
onRun?: (sql: string) => void;            // fired by Cmd/Ctrl+Enter, with the resolved SQL
onSelectionChange?: (selectedText: string) => void; // current selection text ("" when none)
```
In `onMount(editor, monaco)` (capture the real `editor`):
- Register the keybinding:
  ```ts
  editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
    const sel = editor.getSelection();
    const selected = sel && !sel.isEmpty() ? (editor.getModel()?.getValueInRange(sel) ?? "") : "";
    onRunRef.current?.(pickRunSql(editor.getValue(), selected));
  });
  ```
- Report selection changes (for the button label):
  ```ts
  const d = editor.onDidChangeCursorSelection(() => {
    const sel = editor.getSelection();
    const selected = sel && !sel.isEmpty() ? (editor.getModel()?.getValueInRange(sel) ?? "") : "";
    onSelRef.current?.(selected);
  });
  // push d to the disposables list
  ```
- Use refs (`onRunRef`, `onSelRef`) holding the latest callbacks (closure-safe, mirroring the
  existing `wordsRef` pattern), so the keybinding/listener always call the current props.
- Generalize the single `disposableRef` to a `disposables` array (completion provider +
  selection listener) disposed on unmount. (`addCommand` keybindings are owned by the editor
  and disposed with it — no manual cleanup needed.)

### C. `SqlTab` — selection-aware Run

- Add state `const [selectedSql, setSelectedSql] = useState("");`.
- `run` takes an optional override:
  ```ts
  async function run(sqlOverride?: string) {
    if (running) return;                       // keybinding/button no-op while running
    if (!tab.dbId) { toast.error("Pick a database"); return; }
    const sql = sqlOverride ?? tab.sql;
    setQueryId(null); setPage(0);
    try { const { id } = await submit.mutateAsync({ database_id: tab.dbId, sql, limit: tab.limit }); setQueryId(id); }
    catch (e) { toast.error(String(e)); }
  }
  ```
- The Run button becomes selection-aware (label + run target), with a shortcut tooltip:
  ```tsx
  const runSelection = selectedSql.trim().length > 0;
  <Button size="sm" title="⌘/Ctrl+↵" onClick={() => run(runSelection ? selectedSql : undefined)} disabled={running}>
    {runSelection ? "Run selection" : "Run"}
  </Button>
  ```
- Pass the new props to `SqlEditor`:
  ```tsx
  <SqlEditor value={tab.sql} onChange={(v) => patch({ sql: v })} schemaWords={words}
    onRun={(sql) => run(sql)} onSelectionChange={setSelectedSql} />
  ```
  (The keybinding path resolves the SQL inside `SqlEditor` via `pickRunSql` and passes it to
  `run`; the button path uses the last-reported `selectedSql`. Both run the selection.)

## Testing

**Unit (Vitest):**
- `components/sqllab/__tests__/pickRunSql.test.ts`:
  - selected non-empty → returns the selection; `"  "` (whitespace) → returns full; `null`/
    `undefined`/`""` → returns full; selection equal to a substring is returned verbatim.
- `SqlTab` selection path: in a SqlTab test, mock `@/components/sqllab/SqlEditor` with a stub
  that renders two buttons — one calling `onSelectionChange("SELECT 1")` and one calling
  `onRun("SELECT 1")` — plus the existing flow. Assert: after `onSelectionChange("SELECT 1")`
  the Run button reads "Run selection"; clicking it (or invoking `onRun("SELECT 1")`) calls
  `submit.mutateAsync` with `sql: "SELECT 1"`. With no selection, the button reads "Run" and
  submits `tab.sql`. (The existing `@monaco-editor/react` textarea mock is unchanged for the
  other tests; this new test additionally stubs `SqlEditor` so the callbacks are drivable.)
- Keep the existing SqlTab/SqlLabTabs tests green (the new `SqlEditor` props are optional).

**Live (Playwright, 0 console errors):** in SQL Lab, type a multi-statement query, select one
statement → the Run button reads "Run selection" and running executes only the selection;
clear the selection → "Run" runs the whole editor; `Cmd/Ctrl+Enter` runs (selection-aware);
both are inert while a query runs.

## Files

**New:**
- `frontend/src/components/sqllab/pickRunSql.ts`
- `frontend/src/components/sqllab/__tests__/pickRunSql.test.ts`

**Modified:**
- `frontend/src/components/sqllab/SqlEditor.tsx` — `onRun`/`onSelectionChange` props, keybinding, selection listener, disposables array.
- `frontend/src/components/sqllab/SqlTab.tsx` — `selectedSql` state, `run(sqlOverride?)`, selection-aware Run button, pass new props.
- `frontend/src/components/sqllab/__tests__/SqlTab.*.test.tsx` (or a new test) — selection-path test via a `SqlEditor` stub.

**Backend:** none.

## Risks / Notes

- Closure-safety: the Monaco keybinding/selection listener are registered once on mount, so
  they must read the latest `onRun`/`onSelectionChange` via refs — not stale closures.
- `addCommand(CtrlCmd|Enter)` overrides Monaco's default Enter only when chorded with Cmd/Ctrl;
  plain Enter still inserts a newline (correct).
- The button's `selectedSql` is state updated from `onSelectionChange`; there's a negligible
  lag vs the editor's live selection, but the keybinding uses the live selection, so a fast
  Cmd+Enter is always accurate.
