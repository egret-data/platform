# Notebook schema-aware autocomplete — Sprint 2 — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox syntax.

**Goal:** Smart, schema-aware autocomplete in notebook cells for ad-hoc analysis:
- **Python** — runtime-accurate completions from the kernel (`df.<column>`, variables, imports, function args) via the Jupyter `complete_request` protocol.
- **SQL** — table/column completions from the catalog, mirroring SQL Lab's existing pattern (`SqlEditor.tsx` suggests keywords + object names reported by the schema browser).

**Tech Stack:** React 18 + TS, Monaco (`@monaco-editor/react`), Vitest. Frontend-only. Reuses the notebook's `KernelConnection` (WS) and the Sprint-1 `SchemaBrowserPanel` + SQL Lab hooks.

**Branch:** `feat/notebook-autocomplete` (off main).

**Key constraints (verified):**
- `KernelConnection` currently only does `execute_request` + iopub (`src/lib/kernel/KernelConnection.ts`). Needs a `complete()` method.
- Monaco completion providers are **global per language**. The notebook registers python + sql providers; they must (a) route Python completion to the *current* notebook's kernel and (b) NOT pollute SQL Lab's SQL editor. Solution: a module-level **notebook completion context** set by `NotebookPage` on mount and cleared on unmount; providers return `{suggestions: []}` when no context is set.
- `registerStaticCompletions` (`src/components/notebook/monaco/registerStatic.ts`) currently registers static python helpers + sql keywords. This sprint replaces it with a richer `registerNotebookCompletions` that keeps the static items AND adds the dynamic ones.

**Conventions:** FE cmds from `egret/frontend`. Commit per task. Verify `npm run typecheck/test/lint/build`. Live-deploy to kind at the end.

---

## Task S2-1: KernelConnection.complete() (Jupyter complete_request)

**Files:** `src/lib/kernel/KernelConnection.ts`, `src/lib/kernel/protocol.ts` (if a helper is useful). Test: `src/lib/kernel/__tests__/KernelConnection.complete.test.ts`.

The Jupyter protocol: send a `complete_request` `{code, cursor_pos}` on the shell channel; the kernel replies with `complete_reply` `{matches: string[], cursor_start, cursor_end, status}` carrying the same `parent_header.msg_id`.

- [ ] **Step 1: Write the failing test** — drive a fake WS: `connect()` (stub `kernelConnect` + `WebSocket`), call `complete("df.", 3)`, feed a `complete_reply` message with matching parent msg_id, assert the returned `{matches, cursorStart, cursorEnd}`. Also assert it times out/rejects gracefully if no reply (short timeout). (Model the WS stubbing on `KernelConnection.status.test.ts` if it stubs WebSocket; otherwise test the reply-correlation via a lighter seam.)

- [ ] **Step 2: Implement.** Add to `KernelConnection`:
```ts
  private pendingCompletes = new Map<string, (r: CompleteResult) => void>();
  // in onMessage(): handle complete_reply BEFORE the pending-execute routing:
  //   if (m.header?.msg_type === "complete_reply") {
  //     const cb = this.pendingCompletes.get(m.parent_header?.msg_id);
  //     if (cb) { this.pendingCompletes.delete(id); cb({ matches: content.matches ?? [], cursorStart: content.cursor_start ?? 0, cursorEnd: content.cursor_end ?? 0 }); }
  //     return;
  //   }

  /** Ask the kernel to complete `code` at byte offset `cursorPos`. Resolves with
   *  matches; resolves empty on timeout / no connection (never throws to the UI). */
  async complete(code: string, cursorPos: number): Promise<CompleteResult> {
    if (!this.ws) return { matches: [], cursorStart: cursorPos, cursorEnd: cursorPos };
    const msg = newMsg("complete_request", this.session, { code, cursor_pos: cursorPos });
    return new Promise((resolve) => {
      const id = msg.header.msg_id;
      const timer = setTimeout(() => { this.pendingCompletes.delete(id); resolve({ matches: [], cursorStart: cursorPos, cursorEnd: cursorPos }); }, 3000);
      this.pendingCompletes.set(id, (r) => { clearTimeout(timer); resolve(r); });
      this.ws!.send(JSON.stringify(msg));
    });
  }
```
Add `export interface CompleteResult { matches: string[]; cursorStart: number; cursorEnd: number }` (in protocol.ts or KernelConnection.ts). Clear `pendingCompletes` on `disconnect()`/`onclose` (resolve them empty so no leak). NOTE: `cursor_pos` in Jupyter is a **codepoint** offset; Monaco `model.getOffsetAt(position)` is a UTF-16 offset — for ASCII code they match; acceptable for v1 (document the caveat).

- [ ] **Step 3:** `npm run test -- KernelConnection.complete`, `npm run typecheck`. Commit `feat(notebook): KernelConnection.complete() via Jupyter complete_request`.

---

## Task S2-2: Notebook completion providers (python via kernel, sql via words) + context

**Files:** Replace `src/components/notebook/monaco/registerStatic.ts` → `src/components/notebook/monaco/notebookCompletion.ts` (keep a re-export or update imports). Modify `CellEditor.tsx` (call the new register), `NotebookPage.tsx` (set/clear context). Test: `monaco/__tests__/notebookCompletion.test.ts`.

- [ ] **Step 1:** Create `notebookCompletion.ts` with a module-level context + a single idempotent registrar:
```ts
import type { Monaco } from "@monaco-editor/react";
import type { KernelConnection } from "@/lib/kernel/KernelConnection";

export interface NotebookCompletionContext {
  kernel: KernelConnection;
  /** Loaded catalog object names (catalogs/schemas/tables/columns) for SQL. */
  sqlWords: () => string[];
}
let ctx: NotebookCompletionContext | null = null;
export function setNotebookCompletionContext(c: NotebookCompletionContext | null) { ctx = c; }

const PYTHON_ITEMS = [ /* the existing static helper items: trino/starrocks/write_table/display/runtime/me */ ];
const SQL_KEYWORDS = [ /* the existing keyword list */ ];

let registered = false;
export function registerNotebookCompletions(monaco: Monaco): void {
  if (registered) return; registered = true;

  monaco.languages.registerCompletionItemProvider("python", {
    triggerCharacters: ["."],
    async provideCompletionItems(model, position) {
      const statics = PYTHON_ITEMS.map((it) => ({ label: it.label, kind: monaco.languages.CompletionItemKind.Function, insertText: it.insert, insertTextRules: monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet, detail: it.detail, range: undefined as never }));
      if (!ctx) return { suggestions: statics };
      const offset = model.getOffsetAt(position);
      const res = await ctx.kernel.complete(model.getValue(), offset).catch(() => null);
      if (!res || res.matches.length === 0) return { suggestions: statics };
      const start = model.getPositionAt(res.cursorStart);
      const end = model.getPositionAt(res.cursorEnd);
      const range = { startLineNumber: start.lineNumber, startColumn: start.column, endLineNumber: end.lineNumber, endColumn: end.column };
      const dynamic = res.matches.map((m) => ({ label: m, kind: monaco.languages.CompletionItemKind.Variable, insertText: m, range }));
      return { suggestions: [...dynamic, ...statics] };
    },
  });

  monaco.languages.registerCompletionItemProvider("sql", {
    provideCompletionItems(model, position) {
      const word = model.getWordUntilPosition(position);
      const range = { startLineNumber: position.lineNumber, endLineNumber: position.lineNumber, startColumn: word.startColumn, endColumn: word.endColumn };
      const kw = SQL_KEYWORDS.map((k) => ({ label: k, kind: monaco.languages.CompletionItemKind.Keyword, insertText: k, range }));
      const words = ctx ? ctx.sqlWords() : []; // empty when not in a notebook (so SQL Lab is unaffected)
      const objs = Array.from(new Set(words)).map((w) => ({ label: w, kind: monaco.languages.CompletionItemKind.Field, insertText: w, range }));
      return { suggestions: [...objs, ...kw] };
    },
  });
}
```
Port the exact `PYTHON_ITEMS` + `SQL_KEYWORDS` from the old `registerStatic.ts`.

- [ ] **Step 2:** In `CellEditor.tsx`, replace the `registerStaticCompletions(monaco)` call in `handleMount` with `registerNotebookCompletions(monaco)` (update the import). Keep everything else (Shift+Enter etc.).

- [ ] **Step 3:** In `NotebookPage.tsx`, set the completion context on mount and clear on unmount. The `sqlWords` reads a ref the schema browser fills (see S2-3):
```tsx
  const sqlWordsRef = useRef<string[]>([]);
  useEffect(() => {
    setNotebookCompletionContext({ kernel, sqlWords: () => sqlWordsRef.current });
    return () => setNotebookCompletionContext(null);
  }, [kernel]);
```

- [ ] **Step 4:** Test — `notebookCompletion.test.ts`: with no context, python provider returns only statics and sql returns only keywords (SQL-Lab-safe); with a context whose `kernel.complete` returns `["df.a","df.b"]`, python suggestions include them; with `sqlWords` returning `["orders","user_id"]`, sql suggestions include them. Use a fake monaco (capture the registered providers, invoke `provideCompletionItems` with a fake model/position) like the existing `registerStatic.test.ts`.

- [ ] **Step 5:** `npm run test -- notebookCompletion CellEditor`, typecheck, lint. Commit `feat(notebook): kernel-powered python + catalog-aware sql autocomplete`.

---

## Task S2-3: Schema browser reports loaded object names (feeds SQL completion)

**Files:** `src/components/notebook/panels/SchemaBrowserPanel.tsx`, `src/routes/explore/NotebookPage.tsx`.

Mirror SQL Lab's `DataPanel` → `SqlEditor` word-reporting: as the schema tree loads catalogs/schemas/tables/columns, accumulate their names so the SQL completion provider can suggest them.

- [ ] **Step 1:** Add an `onObjectsLoaded?: (names: string[]) => void` prop to `SchemaBrowserPanel`. Each level's fetched names (catalog/schema/table names + column names, and ideally fully-qualified `catalog.schema.table`) get reported up as they load (call `onObjectsLoaded` in an effect when a level's hook data arrives). Keep it simple: report the flat set of identifiers you have (table names, column names, and FQNs).
- [ ] **Step 2:** In `NotebookPage.tsx`, pass `onObjectsLoaded={(names) => { sqlWordsRef.current = Array.from(new Set([...sqlWordsRef.current, ...names])); }}` so browsing the tree progressively enriches SQL autocomplete. (The words persist for the session; that's fine.)
- [ ] **Step 3:** Test — SchemaBrowserPanel test asserts `onObjectsLoaded` is called with table/column names after expanding. typecheck, test. Commit `feat(notebook): schema browser feeds sql autocomplete words`.

---

## Task S2-4: Verify + live-deploy on kind

- [ ] **Step 1:** `npm run lint` (0 err) · `typecheck` · `test` (all pass) · `build`.
- [ ] **Step 2:** Deploy: build+load+rollout `egret-frontend:dev` on kind.
- [ ] **Step 3:** Manual live check on `egret.localtest.me`: in a Python cell, run `df = trino("SELECT 1 AS a, 2 AS b")` (or any df), then type `df.` → completions show `a`, `b` (kernel-driven). Type a partial variable/import → completes. In a SQL cell, browse the Schema panel to load a table, then type its prefix → the table/column names suggest. Confirm SQL Lab's own autocomplete is unaffected (open SQL Lab, no notebook words leak).

---

## Self-Review Notes
- Frontend-only; Python completion uses the existing kernel WS (`complete_request`), SQL reuses SQL Lab's proven word-list pattern via the Sprint-1 schema browser.
- Global-provider safety: providers return statics/keywords-only when no notebook context is set → SQL Lab's SQL editor is unaffected (it has its own provider).
- Caveat documented: `cursor_pos` codepoint vs UTF-16 offset (fine for ASCII code); df completions require the cell that created the df to have RUN (runtime introspection).
- Out of scope: signature help, hover docs, multi-line SQL FROM-parsing for scoped column suggestions — future.
