# SQL Lab Run-Selection + Keyboard Shortcut Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Run only the selected SQL (Run button shows "Run selection") and run via `Cmd/Ctrl+Enter`, selection-aware, in SQL Lab.

**Architecture:** A pure `pickRunSql(full, selected)` helper decides what to execute. `SqlEditor` (Monaco) gains `onRun`/`onSelectionChange` props: a `CtrlCmd+Enter` keybinding calls `onRun(pickRunSql(...))` and a selection listener reports the selected text. `SqlTab` tracks the selection, makes `run()` accept a SQL override, and renders a selection-aware Run button.

**Tech Stack:** React 18, TypeScript, Vite, Vitest + Testing Library, `@monaco-editor/react` (monaco), `react-resizable-panels`.

**Conventions:**
- All commands run from `egret/frontend/`.
- Full sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Git identity already configured — never override the author.
- End every commit message with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Branch `feat/sqllab-run-selection` (already checked out). Git repo root is `egret/`, so `git add` paths need the `frontend/` prefix.

**Spec:** `docs/en/specs/2026-06-25-sqllab-run-selection-design.md`

---

## File Structure

**New:**
- `frontend/src/components/sqllab/pickRunSql.ts` — pure helper choosing selection-vs-full.
- `frontend/src/components/sqllab/__tests__/pickRunSql.test.ts` — its unit tests.

**Modified:**
- `frontend/src/components/sqllab/SqlEditor.tsx` — `onRun`/`onSelectionChange` props + Monaco keybinding + selection listener.
- `frontend/src/components/sqllab/SqlTab.tsx` — `selectedSql` state, `run(sqlOverride?)`, selection-aware Run button.
- `frontend/src/components/sqllab/__tests__/SqlTab.runselection.test.tsx` (new) — SqlTab selection path via a `SqlEditor` stub.

**Backend:** none.

---

## Task 1: `pickRunSql` helper

**Files:**
- Create: `frontend/src/components/sqllab/pickRunSql.ts`
- Test: `frontend/src/components/sqllab/__tests__/pickRunSql.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/sqllab/__tests__/pickRunSql.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { pickRunSql } from "../pickRunSql";

describe("pickRunSql", () => {
  const full = "SELECT 1;\nSELECT 2;";
  it("returns the selection when it has non-whitespace content", () => {
    expect(pickRunSql(full, "SELECT 2;")).toBe("SELECT 2;");
  });
  it("returns the full text when the selection is empty/whitespace/nullish", () => {
    expect(pickRunSql(full, "")).toBe(full);
    expect(pickRunSql(full, "   \n ")).toBe(full);
    expect(pickRunSql(full, null)).toBe(full);
    expect(pickRunSql(full, undefined)).toBe(full);
  });
  it("returns the selection verbatim (preserves surrounding whitespace)", () => {
    expect(pickRunSql(full, "  SELECT 2  ")).toBe("  SELECT 2  ");
  });
});
```

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/components/sqllab/__tests__/pickRunSql.test.ts`
Expected: FAIL — cannot resolve `../pickRunSql`.

- [ ] **Step 3: Create the helper**

Create `frontend/src/components/sqllab/pickRunSql.ts`:
```ts
/** The SQL to execute: the selected text when it has non-whitespace content, else the full editor text. */
export function pickRunSql(fullText: string, selectedText: string | null | undefined): string {
  const sel = selectedText ?? "";
  return sel.trim() ? sel : fullText;
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/components/sqllab/__tests__/pickRunSql.test.ts`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/sqllab/pickRunSql.ts frontend/src/components/sqllab/__tests__/pickRunSql.test.ts
git commit -m "feat(sqllab): pickRunSql helper (selection vs full)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: SqlEditor — onRun / onSelectionChange + keybinding

**Files:**
- Modify: `frontend/src/components/sqllab/SqlEditor.tsx`

The current file is:
```tsx
import Editor, { type Monaco } from "@monaco-editor/react";
import type * as MonacoNS from "monaco-editor";
import { useEffect, useRef } from "react";

const KEYWORDS = [
  "SELECT","FROM","WHERE","GROUP BY","ORDER BY","HAVING","LIMIT","JOIN","LEFT JOIN","RIGHT JOIN",
  "INNER JOIN","ON","AS","AND","OR","NOT","IN","IS NULL","IS NOT NULL","DISTINCT","COUNT","SUM",
  "AVG","MIN","MAX","CASE","WHEN","THEN","ELSE","END","UNION","UNION ALL","WITH","INSERT","UPDATE","DELETE","CREATE","TABLE",
];

export function SqlEditor({ value, onChange, schemaWords }: { value: string; onChange: (v: string) => void; schemaWords: string[] }) {
  const wordsRef = useRef(schemaWords);
  wordsRef.current = schemaWords;
  const disposableRef = useRef<{ dispose: () => void } | null>(null);
  useEffect(() => () => disposableRef.current?.dispose(), []);

  function handleMount(_editor: unknown, monaco: Monaco) {
    disposableRef.current = monaco.languages.registerCompletionItemProvider("sql", {
      provideCompletionItems: (model: MonacoNS.editor.ITextModel, position: MonacoNS.Position) => {
        const word = model.getWordUntilPosition(position);
        const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
        const kw = KEYWORDS.map((k) => ({ label: k, kind: monaco.languages.CompletionItemKind.Keyword, insertText: k, range }));
        const objs = Array.from(new Set(wordsRef.current)).map((w) => ({ label: w, kind: monaco.languages.CompletionItemKind.Field, insertText: w, range }));
        return { suggestions: [...objs, ...kw] };
      },
    });
  }

  return <Editor height="100%" defaultLanguage="sql" value={value} onChange={(v) => onChange(v ?? "")} onMount={handleMount}
    options={{ minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false }} />;
}
```

- [ ] **Step 1: Replace the file with the version that adds run + selection**

Replace the ENTIRE `frontend/src/components/sqllab/SqlEditor.tsx` with:
```tsx
import Editor, { type Monaco } from "@monaco-editor/react";
import type * as MonacoNS from "monaco-editor";
import { useEffect, useRef } from "react";
import { pickRunSql } from "./pickRunSql";

const KEYWORDS = [
  "SELECT","FROM","WHERE","GROUP BY","ORDER BY","HAVING","LIMIT","JOIN","LEFT JOIN","RIGHT JOIN",
  "INNER JOIN","ON","AS","AND","OR","NOT","IN","IS NULL","IS NOT NULL","DISTINCT","COUNT","SUM",
  "AVG","MIN","MAX","CASE","WHEN","THEN","ELSE","END","UNION","UNION ALL","WITH","INSERT","UPDATE","DELETE","CREATE","TABLE",
];

export function SqlEditor({ value, onChange, schemaWords, onRun, onSelectionChange }: {
  value: string;
  onChange: (v: string) => void;
  schemaWords: string[];
  onRun?: (sql: string) => void;
  onSelectionChange?: (selectedText: string) => void;
}) {
  const wordsRef = useRef(schemaWords);
  wordsRef.current = schemaWords;
  const onRunRef = useRef(onRun);
  onRunRef.current = onRun;
  const onSelRef = useRef(onSelectionChange);
  onSelRef.current = onSelectionChange;
  const disposables = useRef<{ dispose: () => void }[]>([]);
  useEffect(() => () => { disposables.current.forEach((d) => d.dispose()); disposables.current = []; }, []);

  function selectedText(editor: MonacoNS.editor.IStandaloneCodeEditor): string {
    const sel = editor.getSelection();
    return sel && !sel.isEmpty() ? (editor.getModel()?.getValueInRange(sel) ?? "") : "";
  }

  function handleMount(editor: MonacoNS.editor.IStandaloneCodeEditor, monaco: Monaco) {
    disposables.current.push(
      monaco.languages.registerCompletionItemProvider("sql", {
        provideCompletionItems: (model: MonacoNS.editor.ITextModel, position: MonacoNS.Position) => {
          const word = model.getWordUntilPosition(position);
          const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
          const kw = KEYWORDS.map((k) => ({ label: k, kind: monaco.languages.CompletionItemKind.Keyword, insertText: k, range }));
          const objs = Array.from(new Set(wordsRef.current)).map((w) => ({ label: w, kind: monaco.languages.CompletionItemKind.Field, insertText: w, range }));
          return { suggestions: [...objs, ...kw] };
        },
      }),
    );
    // Cmd/Ctrl+Enter → run the selection (if any) else the whole editor.
    editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
      onRunRef.current?.(pickRunSql(editor.getValue(), selectedText(editor)));
    });
    // Report the current selection text so the toolbar can show "Run selection".
    disposables.current.push(
      editor.onDidChangeCursorSelection(() => onSelRef.current?.(selectedText(editor))),
    );
  }

  return <Editor height="100%" defaultLanguage="sql" value={value} onChange={(v) => onChange(v ?? "")} onMount={handleMount}
    options={{ minimap: { enabled: false }, fontSize: 13, scrollBeyondLastLine: false }} />;
}
```

- [ ] **Step 2: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS. (`SqlTab` doesn't pass the new optional props yet — wired in Task 3. The component still works.)

- [ ] **Step 3: Run the suite**

Run: `npx vitest run`
Expected: PASS — the existing SqlTab/SqlLabTabs tests mock `@monaco-editor/react` with a `<textarea>`, so `handleMount` never runs there; the new props are optional. If a test fails for an unrelated reason, STOP and report.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/sqllab/SqlEditor.tsx
git commit -m "feat(sqllab): SqlEditor exposes onRun (Cmd/Ctrl+Enter) + onSelectionChange

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: SqlTab — selection-aware Run button + wiring + test

**Files:**
- Modify: `frontend/src/components/sqllab/SqlTab.tsx`
- Test: `frontend/src/components/sqllab/__tests__/SqlTab.runselection.test.tsx` (new)

- [ ] **Step 1: Add `selectedSql` state**

In `frontend/src/components/sqllab/SqlTab.tsx`, near the other `useState` hooks (e.g. after `const [panelCollapsed, setPanelCollapsed] = useState(false);`), add:
```tsx
  const [selectedSql, setSelectedSql] = useState("");
```

- [ ] **Step 2: Make `run` accept an override + guard while running**

The current `run` is:
```tsx
  async function run() {
    if (!tab.dbId) { toast.error("Pick a database"); return; }
    setQueryId(null); setPage(0);
    try { const { id } = await submit.mutateAsync({ database_id: tab.dbId, sql: tab.sql, limit: tab.limit }); setQueryId(id); }
    catch (e) { toast.error(String(e)); }
  }
```
Replace it with:
```tsx
  async function run(sqlOverride?: string) {
    if (running) return;
    if (!tab.dbId) { toast.error("Pick a database"); return; }
    const sql = sqlOverride ?? tab.sql;
    setQueryId(null); setPage(0);
    try { const { id } = await submit.mutateAsync({ database_id: tab.dbId, sql, limit: tab.limit }); setQueryId(id); }
    catch (e) { toast.error(String(e)); }
  }
```

- [ ] **Step 3: Selection-aware Run button**

The current Run button is:
```tsx
            <Button size="sm" onClick={run} disabled={running}>Run</Button>
```
Replace it with:
```tsx
            <Button size="sm" title="⌘/Ctrl+↵" onClick={() => run(selectedSql.trim() ? selectedSql : undefined)} disabled={running}>
              {selectedSql.trim() ? "Run selection" : "Run"}
            </Button>
```

- [ ] **Step 4: Pass the new props to `SqlEditor`**

The current editor render is:
```tsx
          <div className="flex-1"><SqlEditor value={tab.sql} onChange={(v) => patch({ sql: v })} schemaWords={words} /></div>
```
Replace with:
```tsx
          <div className="flex-1"><SqlEditor value={tab.sql} onChange={(v) => patch({ sql: v })} schemaWords={words}
            onRun={(sql) => run(sql)} onSelectionChange={setSelectedSql} /></div>
```

- [ ] **Step 5: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS.

- [ ] **Step 6: Write the SqlTab selection-path test**

Create `frontend/src/components/sqllab/__tests__/SqlTab.runselection.test.tsx`. It stubs `SqlEditor` so the `onRun`/`onSelectionChange` callbacks are drivable, and mocks the sqllab API to capture the submitted SQL:
```tsx
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, test, vi } from "vitest";

const submitMock = vi.fn().mockResolvedValue({ id: "q1" });

// Stub SqlEditor with two buttons that drive the selection/run callbacks.
vi.mock("@/components/sqllab/SqlEditor", () => ({
  SqlEditor: ({ onRun, onSelectionChange }: { onRun?: (s: string) => void; onSelectionChange?: (s: string) => void }) => (
    <div>
      <button onClick={() => onSelectionChange?.("SELECT 2")}>select-some</button>
      <button onClick={() => onSelectionChange?.("")}>select-none</button>
      <button onClick={() => onRun?.("SELECT 2")}>kbd-run</button>
    </div>
  ),
}));

vi.mock("@/api/sqllab", () => ({
  useDatabases: () => ({ data: [{ id: "db1", name: "DB", engine: "trino" }] }),
  useSubmitQuery: () => ({ mutateAsync: submitMock }),
  useCancelQuery: () => ({ mutate: vi.fn() }),
  useQueryStatus: () => ({ data: undefined }),
  useResults: () => ({ data: undefined }),
  useSaveQuery: () => ({ mutateAsync: vi.fn() }),
  useHistory: () => ({ data: [] }),
  useSavedQueries: () => ({ data: [] }),
  useDeleteSavedQuery: () => ({ mutate: vi.fn() }),
  useCatalogs: () => ({ data: [] }),
  useSchemas: () => ({ data: [] }),
  useTables: () => ({ data: [] }),
  useColumns: () => ({ data: [] }),
  csvUrl: () => "",
}));

vi.mock("@/api/datasets", () => ({ useCreateDataset: () => ({ mutateAsync: vi.fn(), isPending: false }) }));

import { SqlTab } from "@/components/sqllab/SqlTab";

function renderTab() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <SqlTab ws="eng" tab={{ id: "t1", title: "Q", sql: "SELECT 1", dbId: "db1", limit: 100 }} patch={vi.fn()} onOpenSaved={vi.fn()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

test("Run button runs the full SQL by default", () => {
  submitMock.mockClear();
  renderTab();
  fireEvent.click(screen.getByRole("button", { name: "Run" }));
  expect(submitMock).toHaveBeenCalledWith(expect.objectContaining({ sql: "SELECT 1" }));
});

test("with a selection, the button becomes 'Run selection' and runs the selection", () => {
  submitMock.mockClear();
  renderTab();
  fireEvent.click(screen.getByRole("button", { name: "select-some" }));
  const runBtn = screen.getByRole("button", { name: "Run selection" });
  fireEvent.click(runBtn);
  expect(submitMock).toHaveBeenCalledWith(expect.objectContaining({ sql: "SELECT 2" }));
});

test("the keyboard run path (onRun) submits the resolved SQL", () => {
  submitMock.mockClear();
  renderTab();
  fireEvent.click(screen.getByRole("button", { name: "kbd-run" }));
  expect(submitMock).toHaveBeenCalledWith(expect.objectContaining({ sql: "SELECT 2" }));
});
```

IMPORTANT: confirm the `SqlTabState` shape used in `renderTab` matches the real `tab` type (`@/hooks/useTabs` `SqlTabState`). Read `frontend/src/hooks/useTabs.ts` for the exact fields (`id`, `title`, `sql`, `dbId`, `limit`) and adjust the literal if a field name differs. The `useDatabases` mock returns a db whose `id` matches `tab.dbId` (`"db1"`) so `run` passes the `!tab.dbId` guard.

- [ ] **Step 7: Run the new test + full suite**

Run: `npx vitest run src/components/sqllab/__tests__/SqlTab.runselection.test.tsx`
Expected: PASS (3 tests). Then `npx vitest run` — all green.

If the test can't find the Run button by name because the existing toolbar also has other buttons, the queries above use exact accessible names ("Run", "Run selection") which are unique. If `SqlTabState` requires more fields, add them to the literal.

- [ ] **Step 8: Full sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all PASS, 0 lint errors.

- [ ] **Step 9: Commit**

```bash
git add frontend/src/components/sqllab/SqlTab.tsx frontend/src/components/sqllab/__tests__/SqlTab.runselection.test.tsx
git commit -m "feat(sqllab): selection-aware Run button + Cmd/Ctrl+Enter wiring

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Full sweep + live verification + cleanup

**Files:** none (verification only).

- [ ] **Step 1: Full sweep**

Run from `egret/frontend/`:
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
```
Expected: all PASS, 0 lint errors.

- [ ] **Step 2: Live verification (Playwright, 0 console errors)**

Dev stack is up (frontend `http://localhost:5173`, backend `:8080`). Log in `admin@egret.local` / `Admin12345!`. Go to SQL Lab (`/explore/eng/sql-lab`). Pick the database. Type a two-statement query, e.g.:
```
SELECT 1;
SELECT 2;
```
1. **Select** the second statement (`SELECT 2;`) → the Run button label reads **"Run selection"**; click it → only `SELECT 2` runs (results show a single `2`).
2. **Clear** the selection (click into the editor without selecting) → the button reads **"Run"**; click it → the whole editor runs.
3. **`Cmd/Ctrl+Enter`** with a selection runs the selection; with no selection runs all.
4. While a query is running, the Run button is disabled and `Cmd+Enter` does nothing.
5. `browser_console_messages` (per-navigation) shows 0 errors.

Capture a screenshot to `/Users/1pro-l4r8/Documents/github/data-portal/` (parent dir, NOT the repo) if useful; delete it after.

- [ ] **Step 3: Cleanup + final review**

Remove any scratch screenshots. Confirm `git status` shows only intended changes. Dispatch a final code reviewer over `git diff main...HEAD`, then proceed to `superpowers:finishing-a-development-branch`.

---

## Self-Review notes

- **Spec coverage:** A (pickRunSql) → Task 1. B (SqlEditor onRun/onSelectionChange + keybinding) → Task 2. C (SqlTab selection-aware run + button) → Task 3. Tests → Tasks 1,3 + Task 4 live. ✓
- **Type consistency:** `pickRunSql(full, selected)` (Task 1) used in `SqlEditor` (Task 2). `SqlEditor` props `onRun?: (sql:string)=>void` + `onSelectionChange?: (selectedText:string)=>void` (Task 2) passed by `SqlTab` (Task 3). `run(sqlOverride?: string)` (Task 3) called by the button and `onRun`. `selectedSql` state consistent. ✓
- **Green-per-commit:** Task 2's new props are optional, so after Task 2 the app/tests are unchanged-green; Task 3 wires them + adds the test. ✓
- **Monaco untestable in jsdom:** the keybinding/selection logic lives in `SqlEditor.onMount` (not exercised by the textarea mock); coverage is via the pure `pickRunSql` unit test + the SqlTab test (which stubs `SqlEditor`) + the Task 4 live verify.
