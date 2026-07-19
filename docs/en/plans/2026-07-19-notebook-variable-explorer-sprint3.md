# Notebook variable explorer — Sprint 3 — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox syntax.

**Goal:** A **Variables** panel listing the kernel namespace's variables/DataFrames (name, type, and columns for DataFrames), auto-refreshing after cell runs, with click-to-`display()`. Frontend + a `KernelConnection` introspection call — no kernel-image change.

**Mechanism:** Introspect the kernel via a **silent `execute_request` + `user_expressions`** that returns a base64-encoded JSON of the namespace (base64 so the `text/plain` repr parses cleanly). Mirrors the S2 `complete()` request/reply pattern (add `execute_reply` handling to `KernelConnection`).

**Tech Stack:** React 18 + TS, Monaco, Vitest. Reuses `KernelConnection` (WS), the Sprint-1 right tools panel, and `insertCell(..., { initialSource })`.

**Branch:** `feat/notebook-variable-explorer` (off main).

**Conventions:** FE cmds from `egret/frontend`. Commit per task. Verify `npm run typecheck/test/lint/build`. Live-deploy to kind at the end.

---

## Task S3-1: KernelConnection.inspectVariables()

**Files:** `src/lib/kernel/KernelConnection.ts` (+ `protocol.ts` for the type if useful). Test: `src/lib/kernel/__tests__/KernelConnection.inspect.test.ts`.

READ FIRST: the S2 `complete()` + `pendingCompletes` + the `complete_reply` handling in `onMessage` (lines ~37, 115, 128) — mirror this pattern exactly.

- [ ] **Step 1: Write the failing test** — mirror `KernelConnection.complete.test.ts`'s WS-stub style. After `connect()`, call `inspectVariables()`; assert a `execute_request` was sent with `silent: true`, `store_history: false`, and a `user_expressions.v` string. Then deliver an `execute_reply` with the same `parent_header.msg_id` and `content.user_expressions.v.data["text/plain"]` = the Python repr of a base64 string that decodes to `[{"name":"df","type":"DataFrame","columns":["a","b"]},{"name":"x","type":"int","columns":null}]` (compute the base64 in the test and wrap in single quotes). Assert `inspectVariables()` resolves to that parsed array. Also: no socket → resolves `[]`; drain-on-close resolves `[]`.

- [ ] **Step 2: Implement.**
```ts
export interface VarInfo { name: string; type: string; columns: string[] | null }

// field:
private pendingInspects = new Map<string, (u: Record<string, any> | undefined) => void>();

// The introspection is a SINGLE Python expression (evaluated via user_expressions,
// so no def is needed). base64 so the text/plain repr is quote-clean.
private static readonly VARS_EXPR =
  "__import__('base64').b64encode(__import__('json').dumps([" +
  "{'name': k, 'type': type(v).__name__, 'columns': (list(v.column_names) " +
  "if hasattr(v, 'column_names') and getattr(type(v), '__module__', '').split('.')[0] == 'daft' else None)} " +
  "for k, v in list(get_ipython().user_ns.items()) " +
  "if not k.startswith('_') " +
  "and k not in ('In','Out','exit','quit','get_ipython','open','runtime','me','trino','starrocks','display','write_table') " +
  "and type(v).__name__ not in ('module','function','builtin_function_or_method','method','type','ZMQExitAutocall','MagicsManager')" +
  "], default=str).encode()).decode()";

async inspectVariables(): Promise<VarInfo[]> {
  if (!this.ws) return [];
  const msg = newMsg("execute_request", this.session, {
    code: "", silent: true, store_history: false,
    user_expressions: { v: KernelConnection.VARS_EXPR },
  });
  return new Promise((resolve) => {
    const id = msg.header.msg_id;
    const timer = setTimeout(() => { this.pendingInspects.delete(id); resolve([]); }, 3000);
    this.pendingInspects.set(id, (u) => {
      clearTimeout(timer);
      try {
        const raw = u?.v?.data?.["text/plain"] as string | undefined;
        if (!raw) return resolve([]);
        const b64 = raw.replace(/^['"]|['"]$/g, "");
        const json = decodeUtf8Base64(b64);       // atob + UTF-8 decode helper (below)
        resolve(JSON.parse(json) as VarInfo[]);
      } catch { resolve([]); }
    });
    this.ws!.send(JSON.stringify(msg));
  });
}
```
Add a small helper:
```ts
function decodeUtf8Base64(b64: string): string {
  const bin = atob(b64);
  const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}
```
In `onMessage`, add (BEFORE the normal execute/iopub routing, alongside the `complete_reply` branch):
```ts
    if (m.header?.msg_type === "execute_reply") {
      const cb = this.pendingInspects.get(parent);
      if (cb) { this.pendingInspects.delete(parent); cb(m.content?.user_expressions); return; }
      // else: a normal cell run's execute_reply — currently unused; fall through.
    }
```
Drain `pendingInspects` on `disconnect()` + the `onclose` flush (resolve each `[]`, clear), like `pendingCompletes`.

- [ ] **Step 3:** `npm run test -- KernelConnection`, `npm run typecheck`. Commit `feat(notebook): KernelConnection.inspectVariables() via silent user_expressions`.

---

## Task S3-2: Variables panel + 4th toggle + refresh + click-to-display

**Files:** Create `src/components/notebook/panels/VariablesPanel.tsx`; modify `NotebookHeader.tsx` (4th toggle), `NotebookPage.tsx` (state + wiring + refresh). Test: VariablesPanel.

- [ ] **Step 1:** `VariablesPanel` — props `{ vars: VarInfo[]; loading: boolean; onRefresh: () => void; onDisplay: (name: string) => void }`. Render a header row with a refresh button; then the list: each var → `<name>` (mono) + a type badge; if `columns`, an expander revealing the column names. Clicking the name calls `onDisplay(name)`. Empty state: "No variables yet — run a cell." Match `SchemaBrowserPanel`/`DataSourcesPanel` styling (text-xs, compact).

- [ ] **Step 2:** `NotebookHeader.tsx` — extend `NotebookPanel` to `"toc" | "data" | "schema" | "vars" | null`; add a 4th toggle button (lucide `Variable` or `Braces`) `aria-label="Variables"`, pressed when `activePanel === "vars"`.

- [ ] **Step 3:** `NotebookPage.tsx`:
  - State `const [vars, setVars] = useState<VarInfo[]>([]); const [varsLoading, setVarsLoading] = useState(false);`
  - `const refreshVars = useCallback(async () => { setVarsLoading(true); try { setVars(await kernel.inspectVariables()); } finally { setVarsLoading(false); } }, [kernel]);`
  - Refresh when the panel opens, and after runs while it's open (kernel goes idle). An effect:
    ```tsx
    useEffect(() => {
      if (activePanel === "vars" && (kernelStatus === "idle" || kernelStatus === "connected")) {
        void refreshVars();
      }
    }, [activePanel, kernelStatus, refreshVars]);
    ```
    (Fires on open + on each busy→idle transition after a run — cheap silent call; the `kernelStatus` idle edge is the natural refresh trigger.)
  - `onDisplay = (name) => insertCell("code", <cellsLength>, { focus: true, initialSource: \`display(${name})\` })` — reconcile with the real `insertCell` signature (it already supports `{ focus, initialSource }` from Sprint 1's schema browser). 
  - Render `<VariablesPanel vars={vars} loading={varsLoading} onRefresh={refreshVars} onDisplay={onDisplay} />` in the aside when `activePanel === "vars"`; add "Variables" to the `panelTitle` map.

- [ ] **Step 4:** Test — `VariablesPanel.test.tsx`: renders a list from vars (name + type), expands columns, clicking a name calls `onDisplay`, empty-state, refresh button calls `onRefresh`. typecheck, `npm run test -- Variables NotebookPage NotebookHeader`. Commit `feat(notebook): variables panel (kernel namespace explorer) with click-to-display`.

---

## Task S3-3: Verify + live-deploy on kind

- [ ] **Step 1:** `npm run lint` (0 err) · `typecheck` · `test` (all pass) · `build`.
- [ ] **Step 2:** Deploy: build+load+rollout `egret-frontend:dev` on kind.
- [ ] **Step 3:** Live check on `egret.localtest.me`: run a cell `x = 42` and (if an engine is available) `df = trino("SELECT 1 a, 2 b")` — else `import daft; df = daft.from_pydict({"a":[1],"b":[2]})`; open the Variables panel → see `x: int`, `df: DataFrame` with columns `a,b`; click `df` → a new cell `display(df)` appears + runs; verify the panel refreshes after running another cell.

---

## Self-Review Notes
- No kernel-image change: introspection is a single Python expression via `user_expressions` (silent, no execution_count pollution). base64 keeps the `text/plain` repr parse-clean.
- `KernelConnection` gains `execute_reply` handling (only intercepts inspect requests; normal runs fall through as before).
- Refresh is driven by the `kernelStatus` idle edge while the panel is open — cheap, no manual polling.
- Columns only for Daft DataFrames (via `column_names`); scalars show `type` only (no value materialization → no compute). Rows/shape deferred (would trigger compute).
- Out of scope: value previews, mutable watch expressions, non-Daft frame types (pandas), memory size.
