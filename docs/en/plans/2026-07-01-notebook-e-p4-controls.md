# Notebook E-P4 — Full run controls + SQL-target dropdown Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Complete the notebook execution UX (the LAST notebook-epic phase): a page toolbar (Run all / Interrupt / Restart / Clear all), per-cell Run above/below, a SQL-target dropdown, and active-executions restore.

**Architecture:** Run-all uses the Jupyter **upfront-queue** model — fire every non-markdown cell's `execute_request` in order; the kernel runs them sequentially and honors `stop_on_error` (already set in E-P3's execute content). A page-level **runner registry** (a ref map cellId→`{run, clear}`) lets the toolbar orchestrate cells whose run/output state lives in `ManagedCell`. Interrupt/Restart POST to the E-P1 endpoints via new `KernelConnection` methods (restart drops the WS → the existing `onclose` flush clears stuck-running). The SQL-target dropdown edits `cell.sql_target` via `useUpdateCell`, options from the F1 `notebook-runtime` manifest engines. On mount, `active-executions` (E-P2) seeds which cells show as running.

**Tech Stack:** React 18 + TS, Vitest + RTL, TanStack Query, shadcn (Select/Button/DropdownMenu).

---

## File structure
- `frontend/src/api/notebooks.ts` — `sql_target` in `useUpdateCell`; new `useNotebookRuntime` + `useActiveExecutions` hooks + `NotebookRuntime`/`EngineOption` types.
- `frontend/src/lib/kernel/KernelConnection.ts` — `interrupt()` + `restart()`.
- `frontend/src/components/notebook/CellEditor.tsx` — SQL-target `<Select>` + per-cell run-above/below menu.
- `frontend/src/routes/explore/NotebookPage.tsx` — page toolbar + runner registry + active-executions restore.

---

## Task 1: API hooks — sql_target update + runtime + active-executions (TDD)

**Files:** `frontend/src/api/notebooks.ts`, `frontend/src/api/__tests__/notebooks.test.ts` (create if absent — else add to the notebooks test).

- [ ] **Step 1: Types + hooks.** Add to `notebooks.ts`:
```ts
export interface EngineOption { id: string; name: string; engine: string; enabled: boolean }
export interface NotebookRuntime {
  username: string;
  engines: EngineOption[];
  engine_jwt: string | null;
  engine_jwt_expires_at: string | null;
}

export function useNotebookRuntime(ws: string) {
  return useQuery({
    queryKey: ["notebook-runtime", ws],
    queryFn: () => apiJson<NotebookRuntime>(`/ws/${encodeURIComponent(ws)}/notebook-runtime`),
    staleTime: 60_000,
  });
}

export function useActiveExecutions(ws: string, id: string) {
  return useQuery({
    queryKey: ["active-executions", ws, id],
    queryFn: () => apiJson<{ running: string[] }>(`${base(ws)}/${id}/kernel/active-executions`),
  });
}
```
- [ ] **Step 2: Extend `useUpdateCell`** — widen the arg + PATCH body with `sql_target`:
```ts
    mutationFn: (a: { cid: string; cell_type?: CellType; source?: string; sql_target?: string }) =>
      apiFetch(`${base(ws)}/${id}/cells/${a.cid}`,
        jsonBody("PATCH", { cell_type: a.cell_type, source: a.source, sql_target: a.sql_target })),
```
(Backend already accepts `sql_target` — `UpdateCell` + `COALESCE($5, sql_target)`.)
- [ ] **Step 3: Failing test** `frontend/src/api/__tests__/notebooks.test.ts` — stub `apiFetch`/`apiJson` (`vi.mock("@/api/client")`), render `useUpdateCell` (via `renderHook` + a QueryClient wrapper) and assert calling `mutate({cid:"c1", sql_target:"trino"})` calls `apiFetch` with a PATCH whose body includes `sql_target:"trino"`; render `useNotebookRuntime("w")` and assert it calls `apiJson` with `/ws/w/notebook-runtime`. (Follow any existing api test; if none, use `@testing-library/react` `renderHook` + `QueryClientProvider`.)
- [ ] **Step 4: Run — fail then pass** `cd frontend && npx vitest run src/api`.
- [ ] **Step 5: Commit**
```bash
git add frontend/src/api/notebooks.ts frontend/src/api/__tests__/
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(frontend): notebook-runtime + active-executions hooks + sql_target update

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `KernelConnection.interrupt()` / `restart()` (TDD)

**Files:** `frontend/src/lib/kernel/KernelConnection.ts`, `frontend/src/lib/kernel/__tests__/KernelConnection.test.ts`.

- [ ] **Step 1: Failing test** — with the existing `MockWS` + stubbed `fetch`, assert `await kc.interrupt()` POSTs to a URL ending `/kernel/interrupt` and `await kc.restart()` POSTs to `/kernel/restart`:
```ts
it("interrupt and restart POST the kernel endpoints", async () => {
  const calls: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: any) => { calls.push(`${init?.method ?? "GET"} ${url}`); return new Response("{}", { status: 200 }); }));
  const kc = new KernelConnection("ws1", "nb1");
  await kc.interrupt();
  await kc.restart();
  expect(calls.some((c) => c.startsWith("POST") && c.includes("/kernel/interrupt"))).toBe(true);
  expect(calls.some((c) => c.startsWith("POST") && c.includes("/kernel/restart"))).toBe(true);
});
```
- [ ] **Step 2: Run — fails** `cd frontend && npx vitest run src/lib/kernel` → FAIL (methods missing).
- [ ] **Step 3: Implement** — add to `KernelConnection` (using the existing `apiFetch` + `path()`):
```ts
  /** Interrupt the running kernel (best-effort; requires a connected kernel). */
  async interrupt(): Promise<void> {
    await apiFetch(this.path("/interrupt"), { method: "POST" });
  }

  /** Restart the kernel. The channels WS drops → onclose flushes pending (clears running). */
  async restart(): Promise<void> {
    await apiFetch(this.path("/restart"), { method: "POST" });
  }
```
- [ ] **Step 4: Run — passes**; `npx tsc --noEmit` clean.
- [ ] **Step 5: Commit**
```bash
git add frontend/src/lib/kernel/
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(frontend): KernelConnection interrupt() + restart()

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: SQL-target dropdown (TDD)

**Files:** `frontend/src/components/notebook/CellEditor.tsx`, `frontend/src/routes/explore/NotebookPage.tsx`, tests.

- [ ] **Step 1: CellEditor gains the dropdown.** Add `onChangeSqlTarget?: (target: string) => void` and `engines?: EngineOption[]` to `CellEditorProps` (import `EngineOption` from `@/api/notebooks`). After the cell-type `<Select>` (~L47-59), gated on `cell.cell_type === "sql"`, add:
```tsx
{cell.cell_type === "sql" && onChangeSqlTarget && (
  <Select value={cell.sql_target || "daft"} onValueChange={onChangeSqlTarget}>
    <SelectTrigger className="h-7 w-32 text-xs" aria-label="SQL target"><SelectValue /></SelectTrigger>
    <SelectContent>
      <SelectItem value="daft">Daft (in-kernel)</SelectItem>
      {(engines ?? []).map((e) => <SelectItem key={e.id} value={e.name}>{e.name}</SelectItem>)}
    </SelectContent>
  </Select>
)}
```
- [ ] **Step 2: Wire in `NotebookPage`/`ManagedCell`.** Page: `const runtime = useNotebookRuntime(ws!);` pass `engines={runtime.data?.engines}` down to each `ManagedCell` → `CellEditor`. `ManagedCell`: add `onChangeSqlTarget={(t) => updateCellMutate({ cid: cell.id, sql_target: t })}` (widen `ManagedCellProps.updateCellMutate` to accept `sql_target?: string`). The magic prefix in `handleRun` already reads `cell.sql_target || "daft"`.
- [ ] **Step 3: Failing test** — in `NotebookPage.test.tsx`, mock `useNotebookRuntime` to return `{ data: { engines: [{id:"e1",name:"trino",engine:"trino",enabled:true}] } }`; render a SQL cell; assert a "SQL target" combobox exists with a "trino" option; changing it calls `updateCell.mutate` with `{cid, sql_target:"trino"}`. (RTL: the shadcn Select renders a `combobox` role; assert on the option text — or assert the `onValueChange` path via a direct call if the Radix portal is hard to drive in jsdom; follow how existing tests handle the cell-type Select.)
- [ ] **Step 4: Run — fail then implement then pass** `cd frontend && npx vitest run src/routes/explore src/components`.
- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/notebook/CellEditor.tsx frontend/src/routes/explore/NotebookPage.tsx frontend/src/routes/explore/__tests__/ frontend/src/components/notebook/__tests__/
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(frontend): per-cell SQL-target dropdown (Daft | engines)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Page toolbar + runner registry + active-executions restore (TDD)

**Files:** `frontend/src/routes/explore/NotebookPage.tsx`, tests.

The page must orchestrate cells whose run/output state lives in `ManagedCell`. Use a **runner registry**: a `useRef<Record<string, { run: () => Promise<void>; clear: () => void }>>({})` on the page; each `ManagedCell` registers itself on mount (and deregisters on unmount).

- [ ] **Step 1: Runner registry + ManagedCell registration.** In `NotebookPage`, add `const runners = useRef<Record<string, { run: () => Promise<void>; clear: () => void }>>({});` and pass `runners` (or a register callback) to each `ManagedCell`. In `ManagedCell`, register on mount:
```ts
useEffect(() => {
  runners.current[cell.id] = { run: handleRun, clear: () => setOutputs([]) };
  return () => { delete runners.current[cell.id]; };
}, [cell.id, source, cell.sql_target, cell.cell_type]); // re-register when the closure inputs change
```
Make `handleRun` return `Promise<void>` (it already `await kernel.connect()` then fires execute — return after firing; the kernel queues + runs sequentially with `stop_on_error`). Markdown cells register a no-op `run`.
- [ ] **Step 2: Page toolbar.** In the header row (~L227), add a `<Button>` group:
```tsx
<Button size="sm" variant="outline" onClick={runAll} disabled={busy}>Run all</Button>
<Button size="sm" variant="outline" onClick={() => kernel.interrupt()}>Interrupt</Button>
<Button size="sm" variant="outline" onClick={() => kernel.restart()}>Restart</Button>
<Button size="sm" variant="ghost" onClick={clearAll}>Clear outputs</Button>
```
with:
```ts
async function runAll() {
  const runnable = sortedCells.filter((c) => c.cell_type !== "markdown");
  for (const c of runnable) { await runners.current[c.id]?.run(); } // fires each execute upfront; kernel sequences + stop_on_error
}
function clearAll() { sortedCells.forEach((c) => runners.current[c.id]?.clear()); }
```
(`await` each `run()` only blocks on `kernel.connect()` (memoized) + the send — it does NOT wait for idle, so all execute_requests are queued upfront per the Jupyter model.)
- [ ] **Step 3: active-executions restore.** `const active = useActiveExecutions(ws!, id!);` build `const runningSet = new Set(active.data?.running ?? []);` and pass `initialRunning={runningSet.has(cell.id)}` to each `ManagedCell`; seed `const [running, setRunning] = useState(initialRunning ?? false);`. (On reopen, cells the recorder still lists as running show the spinner.)
- [ ] **Step 4: Failing test** — in `NotebookPage.test.tsx`: (a) clicking "Run all" calls the mocked kernel `execute` once per non-markdown cell; (b) "Interrupt"/"Restart" call the mocked `kernel.interrupt`/`restart`; (c) "Clear outputs" empties rendered outputs; (d) mock `useActiveExecutions` to return `{running:[codeCellId]}` and assert that cell shows a running indicator on load. Mock `KernelConnection`/`useKernel` with `interrupt`/`restart` spies + an `execute` that records calls.
- [ ] **Step 5: Run — fail then implement then pass** `cd frontend && npx vitest run src/routes/explore`.
- [ ] **Step 6: Commit**
```bash
git add frontend/src/routes/explore/
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(frontend): notebook toolbar — run all / interrupt / restart / clear + active-executions restore

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Per-cell Run above / Run below (TDD)

**Files:** `frontend/src/components/notebook/CellEditor.tsx`, `frontend/src/routes/explore/NotebookPage.tsx`, tests.

- [ ] **Step 1: CellEditor menu.** Add `onRunAbove?: () => void` and `onRunBelow?: () => void` to `CellEditorProps`. In the action group (~L61-105, near Run), add a shadcn `DropdownMenu` (imported from `@/components/ui/dropdown-menu`) triggered by a small chevron `Button`, with items "Run all above" (`onClick={onRunAbove}`) and "Run all below" (`onClick={onRunBelow}`), each disabled when the prop is undefined.
- [ ] **Step 2: Wire in `NotebookPage`.** Pass to each `ManagedCell` (and through to `CellEditor`):
```ts
onRunAbove: () => runSlice(0, index),        // cells before this one
onRunBelow: () => runSlice(index + 1, sortedCells.length),
```
where `index = sortedCells.findIndex((c) => c.id === cell.id)` and:
```ts
async function runSlice(from: number, to: number) {
  const slice = sortedCells.slice(from, to).filter((c) => c.cell_type !== "markdown");
  for (const c of slice) { await runners.current[c.id]?.run(); }
}
```
(`runSlice` lives at the page level next to `runAll`, sharing the runner registry. Pass `index`/`sortedCells` context so each `ManagedCell` computes its slice.)
- [ ] **Step 3: Failing test** — a cell's "Run all above"/"Run all below" fires `execute` only for the cells in the slice (assert the count + which cell ids). Follow the T4 mock setup.
- [ ] **Step 4: Run — fail then implement then pass** `cd frontend && npx vitest run src/routes/explore src/components`.
- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/notebook/CellEditor.tsx frontend/src/routes/explore/ frontend/src/components/notebook/__tests__/
git -c user.email=hoangquocvietbk2.13@gmail.com commit -m "feat(frontend): per-cell run above / run below

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Frontend CI

- [ ] **Step 1:** `cd frontend && npm run lint` → 0 errors (2 pre-existing chart warnings OK; any E-P4-file warning = fix).
- [ ] **Step 2:** `npm run typecheck` → clean.
- [ ] **Step 3:** `npm test` → all pass (new api/kernel/notebook tests + existing suite).
- [ ] **Step 4:** `npm run build` → succeeds.
- [ ] **Step 5:** PR note: Vitest covers controls/dropdown/restore with mocks; live-kernel run-all/interrupt/restart is verified in the E2E/manual step (next work item after E-P4).

---

## Self-review
- **Spec coverage (E §3.5 full controls + dropdown; E-P4 = run-all/above/below, interrupt, restart, clear + SQL-target dropdown + active-executions restore):** T1 hooks ✔; T2 interrupt/restart client ✔; T3 SQL dropdown ✔; T4 toolbar (run-all upfront-queue, interrupt, restart, clear) + active-executions restore ✔; T5 run above/below ✔.
- **Placeholder scan:** every task has concrete code + the exact endpoint shapes (from the anchors). Tests reference the existing `NotebookPage.test.tsx` mock idiom (real file). No logic placeholders.
- **Type consistency:** `EngineOption{id,name,engine,enabled}`; `useUpdateCell({cid,cell_type?,source?,sql_target?})`; `KernelConnection.interrupt()/restart(): Promise<void>`; runner registry `Record<string,{run:()=>Promise<void>;clear:()=>void}>`; `useActiveExecutions → {running:string[]}` — consistent across tasks.
- **Correctness:** run-all uses the Jupyter upfront-queue (fire all execute_requests; kernel sequences + `stop_on_error` already in E-P3's content); the dropdown value is the engine `name` (matches the `%%egret_sql --target` + the `/kernel/query` engine match); restart relies on the E-P3 `onclose` flush to clear running. active-executions restore seeds the spinner from the E-P2 registry.
- **Security:** interrupt/restart/active-executions all hit the session-CSRF routes via `apiFetch` (CSRF header auto); the backend re-checks `authorize_notebook` (owner/public + workspace membership) on each — no new client-trust surface.
- **Scope:** completes E; after this the Notebook epic is done. No S3 spill / rich MIME (out of scope per spec).
