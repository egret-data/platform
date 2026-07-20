# Notebook ergonomics — Sprint 4 (final polish pass) — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Finish the ad-hoc notebook polish in one pass: (1) ⌘K command palette, (2) drag-reorder cells, (3) kernel-powered hover + signature help, (4) FROM-aware SQL column suggestions. All frontend (+ one `KernelConnection` method for hover).

**Tech Stack:** React 18 + TS, Monaco, `@dnd-kit/*` (already a dep), shadcn `dialog`/`popover` (already present — no cmdk needed), Vitest.

**Branch:** `feat/notebook-polish-sprint4` (off main).

**Conventions:** FE cmds from `egret/frontend`. Commit per task. Verify `npm run typecheck/test/lint/build`. Live-deploy to kind at the end.

---

## Task S4-1: ⌘K command palette

**Files:** Create `src/components/notebook/CommandPalette.tsx`; wire into `NotebookPage.tsx`. Test: CommandPalette.

Build on shadcn `dialog.tsx` (no cmdk dep): a Dialog opened by ⌘K / Ctrl+K, containing a search `<input>` + a filtered list of actions; ↑/↓ to move selection, Enter to run, Esc to close.

- [ ] **Step 1:** `CommandPalette` — props `{ open, onOpenChange, actions: { id: string; label: string; hint?: string; run: () => void }[] }`. Filter actions by the typed query (case-insensitive substring on label); keyboard nav (arrow up/down changes a `selected` index, Enter runs `actions[selected].run()` + closes). Compact, matches app styling.
- [ ] **Step 2:** In `NotebookPage.tsx`: `const [paletteOpen, setPaletteOpen] = useState(false);` + a window keydown listener for `(e.metaKey||e.ctrlKey) && e.key === "k"` → `e.preventDefault(); setPaletteOpen(true);` (add/remove on mount). Build the `actions` array from existing handlers: Add code/markdown/SQL cell, Run all, Interrupt, Restart, Clear outputs, Toggle Outline/Data sources/Schema/Variables, and one "Go to cell…" per cell (label = `In [n] · <first line>`) that calls `focusCell(cell.id)`. Render `<CommandPalette open={paletteOpen} onOpenChange={setPaletteOpen} actions={actions} />`.
- [ ] **Step 3:** Test — render palette open with a couple actions; typing filters; Enter runs the selected action's `run` + closes. typecheck, test. Commit `feat(notebook): ⌘K command palette`.

---

## Task S4-2: Drag-reorder cells

**Files:** `src/routes/explore/NotebookPage.tsx` (wrap cells in dnd-kit Sortable), `src/components/notebook/CellEditor.tsx` (a drag handle). Test: NotebookPage.

READ dnd-kit usage elsewhere in the repo first (`grep -rl "@dnd-kit" src`) to match the project's Sortable pattern.

- [ ] **Step 1:** Wrap the cell list in `<DndContext onDragEnd=...><SortableContext items={cellIds} strategy={verticalListSortingStrategy}>…</SortableContext></DndContext>`. Each cell wrapper uses `useSortable({ id: cell.id })` — apply `setNodeRef`, `transform`, `transition`; spread `attributes`/`listeners` onto a **drag handle** ONLY (a grip icon in the cell gutter/toolbar — `GripVertical` from lucide), NOT the whole cell (so Monaco text selection still works). `onDragEnd` computes the new id order and calls `reorderCells.mutate(newOrder)` (reuse the existing reorder mutation). Use `PointerSensor` with an activation distance so a click on the handle doesn't start a drag accidentally.
- [ ] **Step 2:** Add the drag-handle affordance to `CellEditor` (or the ManagedCell wrapper): a small `GripVertical` button, `cursor-grab`, shown on hover, receiving the sortable `listeners`/`attributes` (thread them as props, or make the sortable wrapper live in NotebookPage and pass the handle down). Keep it out of Monaco's way.
- [ ] **Step 3:** Test — simulate a reorder (dnd-kit testing is fiddly; a lighter test: assert the DndContext + handles render, and unit-test the `onDragEnd` reorder-computation helper directly with a `{active, over}` shape). typecheck, test. Commit `feat(notebook): drag-reorder cells`.

---

## Task S4-3: Kernel-powered hover + signature help

**Files:** `src/lib/kernel/KernelConnection.ts` (add `inspect()`), `src/components/notebook/monaco/notebookCompletion.ts` (register hover + signature-help providers). Test: KernelConnection.inspect + notebookCompletion.

Jupyter `inspect_request` `{code, cursor_pos, detail_level}` → `inspect_reply` `{found, data: {"text/plain": "<docstring/signature>"}}`. Mirror the `complete()`/`inspectVariables()` request/reply pattern (`pendingInspectReq` map + `inspect_reply` branch in `onMessage`).

- [ ] **Step 1 (KernelConnection):** add `async inspect(code, cursorPos, detailLevel = 0): Promise<string | null>` — sends `inspect_request`, resolves with `content.data?.["text/plain"]` when `content.found`, else `null`; timeout/no-socket → `null`; drain on close. Add the `inspect_reply` branch in `onMessage`. Test mirrors `KernelConnection.inspect.test.ts` (the variables one) but for `inspect_reply`.
- [ ] **Step 2 (providers):** in `registerNotebookCompletions` (already registers python/sql completion), also register for **python**:
  - `registerHoverProvider("python", { provideHover: async (model, position) => { if (!ctx) return null; const txt = await ctx.kernel.inspect(model.getValue(), model.getOffsetAt(position), 0); return txt ? { contents: [{ value: "```\n" + stripAnsi(txt).slice(0, 2000) + "\n```" }] } : null; } })` — strip ANSI from the kernel's text (reuse `ansiToHtml`'s `stripAnsi` or a tiny local strip).
  - `registerSignatureHelpProvider("python", { signatureHelpTriggerCharacters: ["(", ","], provideSignatureHelp: async (model, position) => { … call inspect with detail_level 0, parse the first line as a signature label … })` — keep it simple: show the inspect text's first meaningful line as one `SignatureInformation` label; if parsing is unreliable, a hover-only v1 is acceptable (note it).
  Extend `NotebookCompletionContext` if needed (it already has `kernel`). Guard all providers on `ctx` (silent outside a notebook).
- [ ] **Step 3:** Test — with a ctx whose `kernel.inspect` returns a docstring, the hover provider returns contents containing it; with no ctx, returns null. typecheck, test. Commit `feat(notebook): kernel-powered hover + signature help`.

---

## Task S4-4: FROM-aware SQL column suggestions

**Files:** `src/components/notebook/monaco/notebookCompletion.ts` (sql provider), `src/components/notebook/panels/SchemaBrowserPanel.tsx` + `NotebookPage.tsx` (feed a table→columns map).

Enhance the S2 SQL completion: when the cursor is in a column position, prioritize columns of the tables referenced in the query's FROM/JOIN.

- [ ] **Step 1:** Extend `NotebookCompletionContext` with `tableColumns: () => Record<string, string[]>` (table name — bare and/or FQN — → its columns). Have `SchemaBrowserPanel` report a `{table: columns}` map (as columns load) via a new `onColumnsLoaded?: (table: string, cols: string[]) => void`; `NotebookPage` accumulates into a `tableColumnsRef` and exposes it via the context. Keep the existing flat `sqlWords` too.
- [ ] **Step 2:** In the sql provider: parse the model text for `FROM <t>` / `JOIN <t>` table references (simple regex capturing identifiers/FQNs, handling optional aliases). Union their columns (from `ctx.tableColumns()`) and rank them ABOVE keywords/other words when the referenced tables are known. Fall back to the current flat `sqlWords` + keywords when no FROM tables match. Keep it robust (regex parse must never throw).
- [ ] **Step 3:** Test — with `tableColumns` = `{ orders: ["id","total"] }` and model text `SELECT  FROM orders`, the provider suggests `id`,`total` (as columns). With no FROM match, falls back to flat words + keywords. typecheck, test. Commit `feat(notebook): FROM-aware sql column suggestions`.

---

## Task S4-5: Verify + live-deploy on kind

- [ ] **Step 1:** `npm run lint` (0 err) · `typecheck` · `test` (all pass) · `build`.
- [ ] **Step 2:** Deploy: build+load+rollout `egret-frontend:dev` on kind.
- [ ] **Step 3:** Live check on `egret.localtest.me`: ⌘K opens the palette + runs an action; drag a cell handle to reorder; hover a Python symbol (after running an import) shows its docstring; in a SQL cell with a browsed table, typing after `SELECT … FROM <table>` suggests that table's columns.

---

## Self-Review Notes
- All frontend; S4-3 adds one `KernelConnection.inspect()` mirroring the existing `complete()`/`inspectVariables()` request/reply plumbing.
- Palette built on shadcn `dialog` (no cmdk dep); dnd via the already-present `@dnd-kit`.
- All Monaco providers stay ctx-gated → SQL Lab / other surfaces unaffected (Sprint-2 invariant).
- Drag handle is a dedicated grip (not the whole cell) so Monaco text selection is preserved.
- Signature-help may ship hover-only if reliable parsing of the kernel inspect text into a signature proves flaky — documented, hover is the main win.
- Out of scope: multi-cursor cross-cell, collaborative cursors, AI actions in the palette.
