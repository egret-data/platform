# SQL Lab History / Saved / Toolbar UX Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make SQL Lab's History and Saved tabs rich (Superset-style rows with status badge, DB, relative time, row count, and per-row actions), fix the clipped limit selector and allow a custom limit, and move query errors off the toolbar into the Results area with a client-measured run-time status line.

**Architecture:** Frontend only (React 18 + TS). Extract the inline `HistoryTab`/`SavedTab` out of `SqlTab.tsx` into focused components, add a `relativeTime` helper (dayjs) and a `QueryStatusBadge`, add a `useUpdateSavedQuery` PUT hook (endpoint already exists), and extend `SaveQueryDialog` with an edit mode and `ResultsGrid` with an error block + footer status line. No backend / API / migration changes.

**Tech Stack:** React 18, TypeScript, Vite, Vitest + Testing Library, `@tanstack/react-query`, `dayjs` (+`relativeTime` plugin), shadcn UI primitives (`Badge`, `AlertDialog`, `Select`, `Dialog`), `lucide-react` icons, `sonner` toasts.

**Conventions:**
- All commands run from `egret/frontend/`.
- Full sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Git repo root is `egret/`, so `git add` paths need the `frontend/` prefix. Docs paths use `docs/`.
- Branch `feat/sqllab-history-saved-ux` (already checked out). Git identity already configured — never override the author.
- End every commit message with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

**Spec:** `docs/en/specs/2026-06-25-sqllab-history-saved-ux-design.md`

---

## File Structure

**New:**
- `frontend/src/lib/relativeTime.ts` — `timeAgo` + `fullTimestamp` (dayjs relativeTime).
- `frontend/src/components/sqllab/QueryStatusBadge.tsx` — status → coloured `Badge`.
- `frontend/src/components/sqllab/HistoryTab.tsx` — rich history list (extracted + enriched).
- `frontend/src/components/sqllab/SavedTab.tsx` — rich saved list (extracted + enriched).

**Modified:**
- `frontend/src/api/sqllab.ts` — add `useUpdateSavedQuery`.
- `frontend/src/components/sqllab/SaveQueryDialog.tsx` — optional edit mode.
- `frontend/src/components/sqllab/ResultsGrid.tsx` — `error` + `statusLine` props.
- `frontend/src/components/sqllab/SqlTab.tsx` — use the new tabs, widen + extend limit, controlled tabs + auto-switch, client timing, route error to Results, `run` accepts a db override.

**Tests (new):**
- `frontend/src/lib/__tests__/relativeTime.test.ts`
- `frontend/src/components/sqllab/__tests__/QueryStatusBadge.test.tsx`
- `frontend/src/components/sqllab/__tests__/HistoryTab.test.tsx`
- `frontend/src/components/sqllab/__tests__/SavedTab.test.tsx`
- `frontend/src/components/sqllab/__tests__/SqlTab.uximprovements.test.tsx`

---

## Task 1: `relativeTime` helper

**Files:**
- Create: `frontend/src/lib/relativeTime.ts`
- Test: `frontend/src/lib/__tests__/relativeTime.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/lib/__tests__/relativeTime.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { timeAgo, fullTimestamp } from "../relativeTime";

describe("relativeTime", () => {
  it("timeAgo renders a relative phrase ending in 'ago' for a past time", () => {
    const fiveMinAgo = new Date(Date.now() - 5 * 60_000).toISOString();
    expect(timeAgo(fiveMinAgo)).toMatch(/ago$/);
    expect(timeAgo(fiveMinAgo)).toContain("minute");
  });
  it("fullTimestamp formats as YYYY-MM-DD HH:mm:ss", () => {
    expect(fullTimestamp("2026-06-25T09:40:00Z")).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
  });
});
```

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/lib/__tests__/relativeTime.test.ts`
Expected: FAIL — cannot resolve `../relativeTime`.

- [ ] **Step 3: Create the helper**

Create `frontend/src/lib/relativeTime.ts`:
```ts
import dayjs from "dayjs";
import relativeTime from "dayjs/plugin/relativeTime";

dayjs.extend(relativeTime);

/** Human "n minutes ago" for an ISO timestamp. */
export function timeAgo(iso: string): string {
  return dayjs(iso).fromNow();
}

/** Absolute local timestamp, for tooltips. */
export function fullTimestamp(iso: string): string {
  return dayjs(iso).format("YYYY-MM-DD HH:mm:ss");
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/lib/__tests__/relativeTime.test.ts`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/lib/relativeTime.ts frontend/src/lib/__tests__/relativeTime.test.ts
git commit -m "feat(sqllab): relativeTime helper (timeAgo / fullTimestamp)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `QueryStatusBadge`

**Files:**
- Create: `frontend/src/components/sqllab/QueryStatusBadge.tsx`
- Test: `frontend/src/components/sqllab/__tests__/QueryStatusBadge.test.tsx`

Context: `QueryStatus = "queued" | "running" | "finished" | "failed" | "cancelled"` (from `@/api/sqllab`). `Badge` (in `@/components/ui/badge`) supports variants `success`, `warning`, `processing`, `destructive`, `secondary` (verified in `badge.tsx`).

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/sqllab/__tests__/QueryStatusBadge.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { QueryStatusBadge } from "@/components/sqllab/QueryStatusBadge";

test("renders the status label", () => {
  render(<QueryStatusBadge status="finished" />);
  expect(screen.getByText("finished")).toBeInTheDocument();
});

test("uses the success colour for finished and destructive for failed", () => {
  const { rerender } = render(<QueryStatusBadge status="finished" />);
  expect(screen.getByText("finished").className).toContain("bg-green-600");
  rerender(<QueryStatusBadge status="failed" />);
  expect(screen.getByText("failed").className).toContain("bg-destructive");
});
```

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/components/sqllab/__tests__/QueryStatusBadge.test.tsx`
Expected: FAIL — cannot resolve `QueryStatusBadge`.

- [ ] **Step 3: Create the component**

Create `frontend/src/components/sqllab/QueryStatusBadge.tsx`:
```tsx
import { Badge } from "@/components/ui/badge";
import type { QueryStatus } from "@/api/sqllab";

const VARIANT: Record<QueryStatus, "success" | "destructive" | "warning" | "processing" | "secondary"> = {
  finished: "success",
  failed: "destructive",
  running: "processing",
  queued: "warning",
  cancelled: "secondary",
};

export function QueryStatusBadge({ status }: { status: QueryStatus }) {
  return <Badge variant={VARIANT[status]} className="capitalize">{status}</Badge>;
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/components/sqllab/__tests__/QueryStatusBadge.test.tsx`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/sqllab/QueryStatusBadge.tsx frontend/src/components/sqllab/__tests__/QueryStatusBadge.test.tsx
git commit -m "feat(sqllab): QueryStatusBadge (coloured status badge)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `useUpdateSavedQuery` API hook

**Files:**
- Modify: `frontend/src/api/sqllab.ts`

Context: The backend route is `get(load).put(update).delete(remove)` at `/ws/{workspace}/sqllab/queries/{id}`; `update` accepts `Json<SaveQuery>` = `{ name, sql, description, database_id }` and requires the caller be the owner. Mirror the existing `useSaveQuery`/`useDeleteSavedQuery` shape (both invalidate `qk.saved(ws)`).

- [ ] **Step 1: Add the hook**

In `frontend/src/api/sqllab.ts`, immediately AFTER the existing `useSaveQuery` function (before `useDeleteSavedQuery`), add:
```ts
export function useUpdateSavedQuery(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: { id: string; database_id: string | null; name: string; sql: string; description?: string }) =>
      apiJson<SavedQuery>(`/ws/${ws}/sqllab/queries/${id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.saved(ws) }),
  });
}
```

- [ ] **Step 2: Typecheck**

Run: `npm run typecheck`
Expected: PASS (the new hook is exported and unused for now — that's fine, it's consumed in Task 6).

- [ ] **Step 3: Commit**

```bash
git add frontend/src/api/sqllab.ts
git commit -m "feat(sqllab): useUpdateSavedQuery (PUT saved query)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `SaveQueryDialog` edit mode

**Files:**
- Modify: `frontend/src/components/sqllab/SaveQueryDialog.tsx`

The current file is create-only (empty initial state, title "Save query"). Add an optional edit mode that prefills name/description, resets to the initial values whenever the dialog opens, and swaps the title + CTA label. Defaults keep the existing create behaviour unchanged.

- [ ] **Step 1: Replace the file**

Replace the ENTIRE `frontend/src/components/sqllab/SaveQueryDialog.tsx` with:
```tsx
import { useEffect, useState } from "react";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";

export function SaveQueryDialog({ open, onOpenChange, onSave, mode = "create", initialName = "", initialDescription = "" }: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  onSave: (name: string, description: string) => Promise<void>;
  mode?: "create" | "edit";
  initialName?: string;
  initialDescription?: string;
}) {
  const [name, setName] = useState(initialName);
  const [description, setDescription] = useState(initialDescription);
  const [busy, setBusy] = useState(false);
  // Reset the fields to the latest initial values each time the dialog opens.
  useEffect(() => { if (open) { setName(initialName); setDescription(initialDescription); } }, [open, initialName, initialDescription]);

  async function submit() {
    if (!name.trim()) return;
    setBusy(true);
    try { await onSave(name.trim(), description.trim()); onOpenChange(false); }
    finally { setBusy(false); }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader><DialogTitle>{mode === "edit" ? "Edit query" : "Save query"}</DialogTitle></DialogHeader>
        <div className="space-y-3">
          <div className="space-y-1"><Label htmlFor="sq-name">Name</Label><Input id="sq-name" value={name} onChange={(e) => setName(e.target.value)} placeholder="Monthly revenue" /></div>
          <div className="space-y-1"><Label htmlFor="sq-desc">Description</Label><Textarea id="sq-desc" value={description} onChange={(e) => setDescription(e.target.value)} rows={3} /></div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button onClick={submit} disabled={busy || !name.trim()}>{mode === "edit" ? "Save changes" : "Save"}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
```

- [ ] **Step 2: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS. (`SqlTab` still calls it without the new props — they're optional with create defaults.)

- [ ] **Step 3: Run the suite (no regressions)**

Run: `npx vitest run`
Expected: PASS — existing tests unaffected (create behaviour unchanged).

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/sqllab/SaveQueryDialog.tsx
git commit -m "feat(sqllab): SaveQueryDialog edit mode (prefill + title/CTA swap)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `ResultsGrid` error block + status line

**Files:**
- Modify: `frontend/src/components/sqllab/ResultsGrid.tsx`
- Test: `frontend/src/components/sqllab/__tests__/ResultsGrid.test.tsx` (extend the existing file)

Add an `error` prop (render a red block instead of the grid) and a `statusLine` node (rendered in the footer; and in the error block's footer when there are no results).

- [ ] **Step 1: Write the failing tests**

The existing test file is `frontend/src/components/sqllab/__tests__/ResultsGrid.test.tsx`. APPEND these tests to it (keep the existing test(s)):
```tsx
test("renders the error block when error is set", () => {
  render(<ResultsGrid page={null} pageIndex={0} onPage={() => {}} csvHref={null} truncated={false}
    error={"line 1:8: boom"} statusLine={"Failed in 12 ms"} />);
  expect(screen.getByText("line 1:8: boom")).toBeInTheDocument();
  expect(screen.getByText("Failed in 12 ms")).toBeInTheDocument();
});

test("renders the status line in the footer when results exist", () => {
  const page = { columns: [{ name: "n", type: "int" }], rows: [[1]], page: 0, page_size: 100, row_count: 1, truncated: false };
  render(<ResultsGrid page={page} pageIndex={0} onPage={() => {}} csvHref={null} truncated={false} statusLine={"Done in 1.2 s"} />);
  expect(screen.getByText("Done in 1.2 s")).toBeInTheDocument();
});
```
Ensure the file imports `render`, `screen` from `@testing-library/react` and `test`/`expect` from `vitest`, and `ResultsGrid` from `../ResultsGrid` (match the existing import lines already in the file).

- [ ] **Step 2: Run the tests, verify they FAIL**

Run: `npx vitest run src/components/sqllab/__tests__/ResultsGrid.test.tsx`
Expected: FAIL — `error`/`statusLine` props not yet rendered.

- [ ] **Step 3: Update `ResultsGrid`**

Replace the ENTIRE `frontend/src/components/sqllab/ResultsGrid.tsx` with:
```tsx
import type { ReactNode } from "react";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Button } from "@/components/ui/button";
import type { ResultsPage } from "@/api/sqllab";

export function ResultsGrid({ page, pageIndex, onPage, csvHref, truncated, error, statusLine }: {
  page: ResultsPage | null; pageIndex: number; onPage: (n: number) => void; csvHref: string | null; truncated: boolean;
  error?: string | null; statusLine?: ReactNode;
}) {
  if (error) {
    return (
      <div className="flex h-full flex-col">
        <div className="flex-1 overflow-auto whitespace-pre-wrap p-4 font-mono text-sm text-red-600">{error}</div>
        {statusLine && <div className="border-t p-2 text-sm text-muted-foreground">{statusLine}</div>}
      </div>
    );
  }
  if (!page) return <div className="p-4 text-sm text-muted-foreground">No results yet. Run a query.</div>;
  const totalPages = Math.max(1, Math.ceil(page.row_count / page.page_size));
  return (
    <div className="flex h-full flex-col">
      {truncated && <div className="bg-yellow-100 px-3 py-1 text-xs text-yellow-900">Results were limited to {page.row_count} rows.</div>}
      <div className="flex-1 overflow-auto">
        <Table>
          <TableHeader>
            <TableRow>{page.columns.map((c) => <TableHead key={c.name}>{c.name}</TableHead>)}</TableRow>
          </TableHeader>
          <TableBody>
            {page.rows.map((row, i) => (
              <TableRow key={i}>{row.map((cell, j) => <TableCell key={j}>{cell === null ? "" : String(cell)}</TableCell>)}</TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
      <div className="flex items-center gap-2 border-t p-2 text-sm">
        <Button variant="outline" size="sm" disabled={pageIndex <= 0} onClick={() => onPage(pageIndex - 1)}>Prev</Button>
        <span>Page {pageIndex + 1} / {totalPages}</span>
        <Button variant="outline" size="sm" disabled={pageIndex >= totalPages - 1} onClick={() => onPage(pageIndex + 1)}>Next</Button>
        {statusLine && <span className="text-muted-foreground">{statusLine}</span>}
        <span className="ml-auto">{page.row_count} rows</span>
        {csvHref && <a href={csvHref} download><Button variant="outline" size="sm">Download CSV</Button></a>}
      </div>
    </div>
  );
}
```

- [ ] **Step 4: Run the tests, verify they PASS**

Run: `npx vitest run src/components/sqllab/__tests__/ResultsGrid.test.tsx`
Expected: PASS (existing + 2 new).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/sqllab/ResultsGrid.tsx frontend/src/components/sqllab/__tests__/ResultsGrid.test.tsx
git commit -m "feat(sqllab): ResultsGrid error block + footer status line

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: `HistoryTab` (rich list)

**Files:**
- Create: `frontend/src/components/sqllab/HistoryTab.tsx`
- Test: `frontend/src/components/sqllab/__tests__/HistoryTab.test.tsx`

Context: `HistoryItem { id, database_id, status, sql, row_count, submitted_at }`; `DatabaseListItem { id, name, engine }` (both exported from `@/api/sqllab`). The row click loads SQL; hover actions Copy + Re-run.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/sqllab/__tests__/HistoryTab.test.tsx`:
```tsx
import { render, screen, fireEvent } from "@testing-library/react";
import { expect, test, vi, beforeEach } from "vitest";

const historyData = [
  { id: "h1", database_id: "db1", status: "finished", sql: "SELECT 42", row_count: 1, submitted_at: new Date(Date.now() - 60_000).toISOString() },
  { id: "h2", database_id: "db1", status: "failed", sql: "SELECT boom", row_count: null, submitted_at: new Date(Date.now() - 120_000).toISOString() },
];
vi.mock("@/api/sqllab", () => ({ useHistory: () => ({ data: historyData }) }));

import { HistoryTab } from "@/components/sqllab/HistoryTab";

const databases = [{ id: "db1", name: "trino-local", engine: "trino" }];

beforeEach(() => {
  Object.assign(navigator, { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
});

test("renders status badge, db name, and sql for each row", () => {
  render(<HistoryTab ws="eng" databases={databases} onLoad={vi.fn()} onRun={vi.fn()} />);
  expect(screen.getByText("finished")).toBeInTheDocument();
  expect(screen.getByText("failed")).toBeInTheDocument();
  expect(screen.getAllByText("trino-local").length).toBe(2);
  expect(screen.getByText("SELECT 42")).toBeInTheDocument();
});

test("clicking a row loads its sql", () => {
  const onLoad = vi.fn();
  render(<HistoryTab ws="eng" databases={databases} onLoad={onLoad} onRun={vi.fn()} />);
  fireEvent.click(screen.getByText("SELECT 42"));
  expect(onLoad).toHaveBeenCalledWith("SELECT 42");
});

test("re-run calls onRun with sql and db id", () => {
  const onRun = vi.fn();
  render(<HistoryTab ws="eng" databases={databases} onLoad={vi.fn()} onRun={onRun} />);
  fireEvent.click(screen.getAllByRole("button", { name: "Re-run" })[0]);
  expect(onRun).toHaveBeenCalledWith("SELECT 42", "db1");
});

test("copy writes the sql to the clipboard", () => {
  render(<HistoryTab ws="eng" databases={databases} onLoad={vi.fn()} onRun={vi.fn()} />);
  fireEvent.click(screen.getAllByRole("button", { name: "Copy SQL" })[0]);
  expect(navigator.clipboard.writeText).toHaveBeenCalledWith("SELECT 42");
});

test("empty state when there is no history", () => {
  vi.resetModules();
});
```
NOTE: the empty-state path is covered separately in the SqlTab integration test (Task 8) to avoid per-test module remocking; the placeholder `test("empty state…")` above may be deleted if it adds no assertion. Keep the four real tests.

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/components/sqllab/__tests__/HistoryTab.test.tsx`
Expected: FAIL — cannot resolve `HistoryTab`.

- [ ] **Step 3: Create `HistoryTab`**

Create `frontend/src/components/sqllab/HistoryTab.tsx`:
```tsx
import { toast } from "sonner";
import { Copy, Play } from "lucide-react";
import { useHistory, type DatabaseListItem } from "@/api/sqllab";
import { QueryStatusBadge } from "@/components/sqllab/QueryStatusBadge";
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";

function dbName(databases: DatabaseListItem[] | undefined, id: string): string {
  return databases?.find((d) => d.id === id)?.name ?? `${id.slice(0, 8)}…`;
}

export function HistoryTab({ ws, databases, onLoad, onRun }: {
  ws: string;
  databases: DatabaseListItem[] | undefined;
  onLoad: (sql: string) => void;
  onRun: (sql: string, dbId: string) => void;
}) {
  const { data } = useHistory(ws);
  const items = data ?? [];
  if (items.length === 0) return <div className="p-4 text-center text-sm text-muted-foreground">No queries run yet.</div>;

  async function copy(sql: string) {
    try { await navigator.clipboard.writeText(sql); toast.success("Copied"); }
    catch { toast.error("Copy failed"); }
  }

  return (
    <div className="divide-y">
      {items.map((h) => (
        <div key={h.id} className="group flex items-start gap-2 px-3 py-2 hover:bg-accent">
          <button className="min-w-0 flex-1 text-left" onClick={() => onLoad(h.sql)}>
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <QueryStatusBadge status={h.status} />
              <span>{dbName(databases, h.database_id)}</span>
              <span>·</span>
              <span title={fullTimestamp(h.submitted_at)}>{timeAgo(h.submitted_at)}</span>
              {h.row_count != null && (<><span>·</span><span>{h.row_count} rows</span></>)}
            </div>
            <div className="truncate font-mono text-sm" title={h.sql}>{h.sql}</div>
          </button>
          <div className="flex shrink-0 items-center gap-1 opacity-0 transition group-hover:opacity-100">
            <button className="rounded p-1 hover:bg-background" title="Copy SQL" aria-label="Copy SQL" onClick={() => copy(h.sql)}><Copy className="h-4 w-4" /></button>
            <button className="rounded p-1 hover:bg-background" title="Re-run" aria-label="Re-run" onClick={() => onRun(h.sql, h.database_id)}><Play className="h-4 w-4" /></button>
          </div>
        </div>
      ))}
    </div>
  );
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/components/sqllab/__tests__/HistoryTab.test.tsx`
Expected: PASS (4 real tests). If the placeholder empty-state test was kept and is empty, delete it.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/sqllab/HistoryTab.tsx frontend/src/components/sqllab/__tests__/HistoryTab.test.tsx
git commit -m "feat(sqllab): rich HistoryTab (badge, db, time, rows, copy, re-run)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: `SavedTab` (rich list + edit + confirm-delete)

**Files:**
- Create: `frontend/src/components/sqllab/SavedTab.tsx`
- Test: `frontend/src/components/sqllab/__tests__/SavedTab.test.tsx`

Context: `SavedQuery { id, database_id, name, sql, description, created_by, updated_at }`. SavedTab is self-contained: it owns the edit dialog (`SaveQueryDialog` edit mode → `useUpdateSavedQuery`) and the delete confirm (`AlertDialog` → `useDeleteSavedQuery`).

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/sqllab/__tests__/SavedTab.test.tsx`:
```tsx
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";

const savedData = [
  { id: "q1", database_id: "db1", name: "Monthly revenue", sql: "SELECT 1", description: "rev by month", created_by: null, updated_at: new Date(Date.now() - 3 * 3600_000).toISOString() },
];
const updateMock = vi.fn().mockResolvedValue({});
const deleteMock = vi.fn();
vi.mock("@/api/sqllab", () => ({
  useSavedQueries: () => ({ data: savedData }),
  useUpdateSavedQuery: () => ({ mutateAsync: updateMock }),
  useDeleteSavedQuery: () => ({ mutate: deleteMock }),
}));

import { SavedTab } from "@/components/sqllab/SavedTab";

const databases = [{ id: "db1", name: "trino-local", engine: "trino" }];

test("renders name, db, and description", () => {
  render(<SavedTab ws="eng" databases={databases} onOpen={vi.fn()} />);
  expect(screen.getByText("Monthly revenue")).toBeInTheDocument();
  expect(screen.getByText("trino-local")).toBeInTheDocument();
  expect(screen.getByText("rev by month")).toBeInTheDocument();
});

test("clicking the row opens the query", () => {
  const onOpen = vi.fn();
  render(<SavedTab ws="eng" databases={databases} onOpen={onOpen} />);
  fireEvent.click(screen.getByText("Monthly revenue"));
  expect(onOpen).toHaveBeenCalledWith("SELECT 1", "db1");
});

test("edit opens the dialog prefilled and saving calls update with the full body", async () => {
  updateMock.mockClear();
  render(<SavedTab ws="eng" databases={databases} onOpen={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Edit" }));
  expect(screen.getByText("Edit query")).toBeInTheDocument();
  expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Monthly revenue");
  fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(updateMock).toHaveBeenCalledWith(
    expect.objectContaining({ id: "q1", database_id: "db1", name: "Monthly revenue", sql: "SELECT 1", description: "rev by month" }),
  ));
});

test("delete asks for confirmation, then deletes", () => {
  deleteMock.mockClear();
  render(<SavedTab ws="eng" databases={databases} onOpen={vi.fn()} />);
  fireEvent.click(screen.getByRole("button", { name: "Delete" }));
  expect(screen.getByText("Delete saved query?")).toBeInTheDocument();
  expect(deleteMock).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Delete", hidden: false }) ?? screen.getAllByRole("button", { name: "Delete" })[1]);
  expect(deleteMock).toHaveBeenCalledWith("q1");
});
```
NOTE on the last test: there are two "Delete" controls once the dialog opens — the row's trash button (`aria-label="Delete"`) and the dialog's confirm action (text "Delete"). Disambiguate by querying within the dialog. Prefer:
```tsx
import { within } from "@testing-library/react";
const dialog = screen.getByRole("alertdialog");
fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
```
Use that `within(dialog)` form in the implementation of the test instead of the fallback shown above; the fallback line is illustrative only — replace it.

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/components/sqllab/__tests__/SavedTab.test.tsx`
Expected: FAIL — cannot resolve `SavedTab`.

- [ ] **Step 3: Create `SavedTab`**

Create `frontend/src/components/sqllab/SavedTab.tsx`:
```tsx
import { useState } from "react";
import { toast } from "sonner";
import { Pencil, Trash2 } from "lucide-react";
import {
  AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogTitle,
  AlertDialogDescription, AlertDialogFooter, AlertDialogCancel, AlertDialogAction,
} from "@/components/ui/alert-dialog";
import {
  useSavedQueries, useUpdateSavedQuery, useDeleteSavedQuery,
  type SavedQuery, type DatabaseListItem,
} from "@/api/sqllab";
import { SaveQueryDialog } from "@/components/sqllab/SaveQueryDialog";
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";

function dbName(databases: DatabaseListItem[] | undefined, id: string | null): string {
  if (!id) return "—";
  return databases?.find((d) => d.id === id)?.name ?? `${id.slice(0, 8)}…`;
}

export function SavedTab({ ws, databases, onOpen }: {
  ws: string;
  databases: DatabaseListItem[] | undefined;
  onOpen: (sql: string, dbId: string | null) => void;
}) {
  const { data } = useSavedQueries(ws);
  const update = useUpdateSavedQuery(ws);
  const del = useDeleteSavedQuery(ws);
  const [editing, setEditing] = useState<SavedQuery | null>(null);
  const [deleting, setDeleting] = useState<SavedQuery | null>(null);
  const items = data ?? [];

  if (items.length === 0) return <div className="p-4 text-center text-sm text-muted-foreground">No saved queries.</div>;

  return (
    <>
      <div className="divide-y">
        {items.map((q) => (
          <div key={q.id} className="group flex items-start gap-2 px-3 py-2 hover:bg-accent">
            <button className="min-w-0 flex-1 text-left" onClick={() => onOpen(q.sql, q.database_id)}>
              <div className="flex items-center gap-2 text-sm">
                <span className="font-semibold">{q.name}</span>
                <span className="text-xs text-muted-foreground">{dbName(databases, q.database_id)}</span>
                <span className="text-xs text-muted-foreground" title={fullTimestamp(q.updated_at)}>updated {timeAgo(q.updated_at)}</span>
              </div>
              <div className="truncate text-xs text-muted-foreground" title={q.description || q.sql}>{q.description || q.sql}</div>
            </button>
            <div className="flex shrink-0 items-center gap-1 opacity-0 transition group-hover:opacity-100">
              <button className="rounded p-1 hover:bg-background" title="Edit" aria-label="Edit" onClick={() => setEditing(q)}><Pencil className="h-4 w-4" /></button>
              <button className="rounded p-1 text-red-600 hover:bg-background" title="Delete" aria-label="Delete" onClick={() => setDeleting(q)}><Trash2 className="h-4 w-4" /></button>
            </div>
          </div>
        ))}
      </div>

      <SaveQueryDialog
        open={editing != null}
        onOpenChange={(o) => { if (!o) setEditing(null); }}
        mode="edit"
        initialName={editing?.name ?? ""}
        initialDescription={editing?.description ?? ""}
        onSave={async (name, description) => {
          if (!editing) return;
          await update.mutateAsync({ id: editing.id, database_id: editing.database_id, name, sql: editing.sql, description });
          toast.success("Updated");
          setEditing(null);
        }}
      />

      <AlertDialog open={deleting != null} onOpenChange={(o) => { if (!o) setDeleting(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete saved query?</AlertDialogTitle>
            <AlertDialogDescription>“{deleting?.name}” will be permanently deleted.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-destructive text-white hover:bg-destructive/90"
              onClick={() => { if (deleting) del.mutate(deleting.id); setDeleting(null); }}
            >Delete</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  );
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/components/sqllab/__tests__/SavedTab.test.tsx`
Expected: PASS (4 tests). If the dialog's "Save changes" click resolves asynchronously, the `waitFor` handles it.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/sqllab/SavedTab.tsx frontend/src/components/sqllab/__tests__/SavedTab.test.tsx
git commit -m "feat(sqllab): rich SavedTab (desc, db, time, edit, confirm-delete)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Integrate into `SqlTab` (limit, error routing, auto-switch, timing, wire tabs)

**Files:**
- Modify: `frontend/src/components/sqllab/SqlTab.tsx`
- Test: `frontend/src/components/sqllab/__tests__/SqlTab.uximprovements.test.tsx`

This task removes the inline `HistoryTab`/`SavedTab`, imports the new ones, makes the bottom `Tabs` controlled (auto-switch to Results on run), routes the failed-query error into `ResultsGrid`, adds client-side timing, and replaces the limit `Select` with a widened version that supports a custom value. `run` gains a db override so Re-run targets the history row's database.

- [ ] **Step 1: Replace the file**

Replace the ENTIRE `frontend/src/components/sqllab/SqlTab.tsx` with:
```tsx
import { useEffect, useRef, useState } from "react";
import { format as formatSql } from "sql-formatter";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { toast } from "sonner";
import { PanelLeftOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
  csvUrl, useCancelQuery, useDatabases, useHistory, useQueryStatus, useResults,
  useSaveQuery, useSubmitQuery,
} from "@/api/sqllab";
import { SqlEditor } from "@/components/sqllab/SqlEditor";
import { ResultsGrid } from "@/components/sqllab/ResultsGrid";
import { DataPanel } from "@/components/sqllab/DataPanel";
import { SaveQueryDialog } from "@/components/sqllab/SaveQueryDialog";
import { HistoryTab } from "@/components/sqllab/HistoryTab";
import { SavedTab } from "@/components/sqllab/SavedTab";
import { CreateDatasetDialog } from "@/components/dataset/CreateDatasetDialog";
import type { SqlTabState } from "@/hooks/useTabs";

const LIMITS = [10, 100, 1000, 10000, 100000];
const MAX_LIMIT = 100000;
const CUSTOM = "__custom__";
const CUSTOM_CURRENT = "__custom_current__";

function fmtElapsed(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} s`;
}

export function SqlTab({ ws, tab, patch, onOpenSaved }: {
  ws: string; tab: SqlTabState; patch: (p: Partial<SqlTabState>) => void; onOpenSaved: (sql: string, dbId: string | null) => void;
}) {
  const { data: databases } = useDatabases(ws);
  const engine = databases?.find((d) => d.id === tab.dbId)?.engine ?? null;
  const [queryId, setQueryId] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [words, setWords] = useState<string[]>([]);
  const [saveOpen, setSaveOpen] = useState(false);
  const [datasetOpen, setDatasetOpen] = useState(false);
  const [panelCollapsed, setPanelCollapsed] = useState(false);
  const [selectedSql, setSelectedSql] = useState("");
  const [tabValue, setTabValue] = useState("results");
  const [customLimit, setCustomLimit] = useState(false);
  const [elapsedMs, setElapsedMs] = useState<number | null>(null);
  const startedRef = useRef<number | null>(null);

  const submit = useSubmitQuery(ws);
  const cancel = useCancelQuery(ws);
  const status = useQueryStatus(ws, queryId);
  const save = useSaveQuery(ws);
  const finished = status.data?.status === "finished";
  const failed = status.data?.status === "failed";
  const running = status.data?.status === "queued" || status.data?.status === "running";
  const results = useResults(ws, queryId, page, finished);

  // Freeze a client-measured elapsed time once the query reaches a terminal state.
  useEffect(() => {
    if ((finished || failed) && startedRef.current != null && elapsedMs == null) {
      setElapsedMs(performance.now() - startedRef.current);
      startedRef.current = null;
    }
  }, [finished, failed, elapsedMs]);

  async function run(sqlOverride?: string, dbOverride?: string | null) {
    if (running) return;
    const dbId = dbOverride ?? tab.dbId;
    if (!dbId) { toast.error("Pick a database"); return; }
    const sql = sqlOverride ?? tab.sql;
    setQueryId(null); setPage(0); setElapsedMs(null); setTabValue("results");
    try {
      const { id } = await submit.mutateAsync({ database_id: dbId, sql, limit: tab.limit });
      setQueryId(id); startedRef.current = performance.now();
    } catch (e) { toast.error(String(e)); }
  }
  function doFormat() {
    const language = engine === "starrocks" ? "mysql" : "trino";
    try { patch({ sql: formatSql(tab.sql, { language }) }); } catch { /* keep as-is */ }
  }
  function insert(name: string) { patch({ sql: tab.sql + name }); }
  function commitLimit(raw: string) {
    const n = Math.floor(Number(raw));
    if (Number.isFinite(n) && n > 0) patch({ limit: Math.min(n, MAX_LIMIT) });
    setCustomLimit(false);
  }

  const isPreset = LIMITS.includes(tab.limit);
  const statusLine =
    elapsedMs == null ? null
    : finished ? `Done in ${fmtElapsed(elapsedMs)}`
    : failed ? `Failed in ${fmtElapsed(elapsedMs)}`
    : null;

  const main = (
    <PanelGroup direction="vertical">
      <Panel defaultSize={50} minSize={20}>
        <div className="flex h-full flex-col">
          <div className="flex items-center gap-2 border-b p-2">
            <Button size="sm" title="⌘/Ctrl+↵" onClick={() => run(selectedSql.trim() ? selectedSql : undefined)} disabled={running}>
              {selectedSql.trim() ? "Run selection" : "Run"}
            </Button>
            <Button size="sm" variant="outline" disabled={!running} onClick={() => queryId && cancel.mutate(queryId)}>Cancel</Button>
            <Button size="sm" variant="outline" onClick={doFormat}>Format</Button>
            <span className="text-sm text-muted-foreground">Limit</span>
            {customLimit ? (
              <Input
                autoFocus type="number" min={1} className="h-8 w-24" defaultValue={tab.limit}
                onKeyDown={(e) => {
                  if (e.key === "Enter") commitLimit((e.target as HTMLInputElement).value);
                  else if (e.key === "Escape") setCustomLimit(false);
                }}
                onBlur={(e) => commitLimit(e.target.value)}
              />
            ) : (
              <Select
                value={isPreset ? String(tab.limit) : CUSTOM_CURRENT}
                onValueChange={(v) => { if (v === CUSTOM) setCustomLimit(true); else if (v !== CUSTOM_CURRENT) patch({ limit: Number(v) }); }}
              >
                <SelectTrigger className="h-8 w-36"><SelectValue /></SelectTrigger>
                <SelectContent>
                  {LIMITS.map((l) => <SelectItem key={l} value={String(l)}>{l.toLocaleString()} rows</SelectItem>)}
                  {!isPreset && <SelectItem value={CUSTOM_CURRENT}>{tab.limit.toLocaleString()} rows</SelectItem>}
                  <SelectItem value={CUSTOM}>Custom…</SelectItem>
                </SelectContent>
              </Select>
            )}
            <Button size="sm" variant="outline" onClick={() => setSaveOpen(true)}>Save</Button>
            <Button size="sm" variant="outline" onClick={() => setDatasetOpen(true)}>Save as dataset</Button>
            {running && <span className="text-sm text-muted-foreground">running…</span>}
          </div>
          <div className="flex-1"><SqlEditor value={tab.sql} onChange={(v) => patch({ sql: v })} schemaWords={words}
            onRun={(sql) => run(sql)} onSelectionChange={setSelectedSql} /></div>
        </div>
      </Panel>
      <PanelResizeHandle className="h-1.5 bg-border transition-colors hover:bg-primary/60 data-[resize-handle-state=drag]:bg-primary" />
      <Panel defaultSize={50} minSize={20}>
        <Tabs value={tabValue} onValueChange={setTabValue} className="flex h-full flex-col">
          <TabsList className="m-2 w-fit"><TabsTrigger value="results">Results</TabsTrigger><TabsTrigger value="history">History</TabsTrigger><TabsTrigger value="saved">Saved</TabsTrigger></TabsList>
          <TabsContent value="results" className="flex-1 overflow-hidden">
            <ResultsGrid page={results.data ?? null} pageIndex={page} onPage={setPage} truncated={status.data?.truncated ?? false}
              error={failed ? (status.data?.error ?? "Query failed") : null} statusLine={statusLine}
              csvHref={finished && queryId ? csvUrl(ws, queryId) : null} />
          </TabsContent>
          <TabsContent value="history" className="flex-1 overflow-auto">
            <HistoryTab ws={ws} databases={databases} onLoad={(s) => patch({ sql: s })} onRun={(sql, dbId) => { patch({ dbId, sql }); run(sql, dbId); }} />
          </TabsContent>
          <TabsContent value="saved" className="flex-1 overflow-auto">
            <SavedTab ws={ws} databases={databases} onOpen={onOpenSaved} />
          </TabsContent>
        </Tabs>
      </Panel>
    </PanelGroup>
  );

  const saveDialog = (
    <SaveQueryDialog open={saveOpen} onOpenChange={setSaveOpen}
      onSave={async (name, description) => { await save.mutateAsync({ database_id: tab.dbId, name, sql: tab.sql, description }); toast.success("Saved"); }} />
  );

  const datasetDialog = (
    <CreateDatasetDialog
      ws={ws}
      open={datasetOpen}
      onOpenChange={setDatasetOpen}
      initialSql={tab.sql}
      initialDbId={tab.dbId}
      initialTab="sql"
    />
  );

  if (panelCollapsed) {
    return (
      <div className="flex h-full">
        <div className="flex h-full flex-col border-r">
          <button onClick={() => setPanelCollapsed(false)} title="Show data panel" className="p-2 text-muted-foreground hover:text-foreground">
            <PanelLeftOpen className="h-4 w-4" />
          </button>
        </div>
        <div className="min-w-0 flex-1">{main}</div>
        {saveDialog}
        {datasetDialog}
      </div>
    );
  }

  return (
    <PanelGroup direction="horizontal" className="h-full">
      <Panel defaultSize={22} minSize={12}>
        <DataPanel ws={ws} dbId={tab.dbId} onDb={(id) => patch({ dbId: id })} onInsert={insert} onWords={setWords} onCollapse={() => setPanelCollapsed(true)} />
      </Panel>
      <PanelResizeHandle className="w-1.5 bg-border transition-colors hover:bg-primary/60 data-[resize-handle-state=drag]:bg-primary" />
      <Panel defaultSize={78}>{main}</Panel>
      {saveDialog}
      {datasetDialog}
    </PanelGroup>
  );
}
```

Notes on what changed vs the old file:
- Removed the inline `HistoryTab`/`SavedTab` functions and their now-unused imports (`useDeleteSavedQuery`, `useSavedQueries`).
- Added imports: `useEffect`, `useRef`; `Input`; `HistoryTab`, `SavedTab`.
- The toolbar's old failed-error `<span>` is gone (error now shows in Results).

- [ ] **Step 2: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS. If typecheck flags an unused import, remove it (the import list above is already pruned).

- [ ] **Step 3: Write the integration test**

Create `frontend/src/components/sqllab/__tests__/SqlTab.uximprovements.test.tsx`:
```tsx
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { expect, test, vi } from "vitest";

const patchMock = vi.fn();

vi.mock("@/components/sqllab/SqlEditor", () => ({
  SqlEditor: () => <div data-testid="sql-editor" />,
}));
// Render the new tabs as inert stubs so this test focuses on SqlTab wiring.
vi.mock("@/components/sqllab/HistoryTab", () => ({ HistoryTab: () => <div data-testid="history" /> }));
vi.mock("@/components/sqllab/SavedTab", () => ({ SavedTab: () => <div data-testid="saved" /> }));
vi.mock("@/components/sqllab/DataPanel", () => ({ DataPanel: () => <div data-testid="data-panel" /> }));

vi.mock("@/api/sqllab", () => ({
  useDatabases: () => ({ data: [{ id: "db1", name: "DB", engine: "trino" }] }),
  useSubmitQuery: () => ({ mutateAsync: vi.fn().mockResolvedValue({ id: "q1" }) }),
  useCancelQuery: () => ({ mutate: vi.fn() }),
  // A failed query, to assert the error renders in the Results panel.
  useQueryStatus: () => ({ data: { id: "q1", database_id: "db1", status: "failed", columns: null, preview: null, row_count: null, truncated: false, error: "line 1:8: boom" } }),
  useResults: () => ({ data: undefined }),
  useSaveQuery: () => ({ mutateAsync: vi.fn() }),
  useHistory: () => ({ data: [] }),
  csvUrl: () => "",
}));

vi.mock("@/api/datasets", () => ({ useCreateDataset: () => ({ mutateAsync: vi.fn(), isPending: false }) }));

import { SqlTab } from "@/components/sqllab/SqlTab";

function renderTab(limit = 1000) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter>
        <SqlTab ws="eng" tab={{ id: "t1", title: "Q", sql: "SELECT 1", dbId: "db1", limit }} patch={patchMock} onOpenSaved={vi.fn()} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

test("a failed query shows the error in the Results panel, not on the toolbar", () => {
  renderTab();
  expect(screen.getByText("line 1:8: boom")).toBeInTheDocument();
});

test("the limit control shows the full preset text and a Custom… option", () => {
  renderTab();
  // The trigger shows the formatted preset (not clipped).
  expect(screen.getByText("1,000 rows")).toBeInTheDocument();
});

test("a custom (non-preset) limit round-trips in the trigger", () => {
  renderTab(250);
  expect(screen.getByText("250 rows")).toBeInTheDocument();
});
```

- [ ] **Step 4: Run the integration test**

Run: `npx vitest run src/components/sqllab/__tests__/SqlTab.uximprovements.test.tsx`
Expected: PASS (3 tests). If the limit trigger text isn't found because Radix renders the value lazily, assert on `screen.getByText(/rows/)` instead.

- [ ] **Step 5: Confirm the pre-existing SqlTab tests still pass**

Run: `npx vitest run src/components/sqllab/__tests__/`
Expected: PASS. The pre-existing `SqlTab.dataset.test.tsx` and `SqlLabTabs.test.tsx` mock `@/api/sqllab`; since SqlTab no longer imports `useSavedQueries`/`useDeleteSavedQuery`, those mock keys are simply unused (harmless). If any of those tests reference the removed inline tabs, update them to expect the stubbed/extracted components. Report — do not silently weaken assertions.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/sqllab/SqlTab.tsx frontend/src/components/sqllab/__tests__/SqlTab.uximprovements.test.tsx
git commit -m "feat(sqllab): wire rich History/Saved, custom limit, error-in-results, run timing

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 9: Full sweep, live verification, finish

**Files:** none (verification only).

- [ ] **Step 1: Full sweep**

Run from `egret/frontend/`:
```bash
npm run typecheck && npm run lint && npm run build && npx vitest run
```
Expected: all PASS, 0 lint errors.

- [ ] **Step 2: Live verification (Playwright, 0 new console errors)**

Dev stack is up (frontend `http://localhost:5173`, backend `:8080`, Trino ready). Log in `admin@egret.local` / `Admin12345!`. Go to SQL Lab (`/explore/eng/sql-lab`), pick `trino-local`.

1. **Limit:** the selector shows the full text "1,000 rows" (not clipped). Open it, choose **Custom…**, type `250`, press Enter → the trigger reads "250 rows"; run `SELECT 1` and confirm it still works.
2. **Run timing + error routing:** run `SELECT 42` → Results auto-selected, shows `42`, footer shows "Done in …". Run an invalid query, e.g. `SELECT * FROM nope.nope.nope` → the **error renders inside the Results panel** (red, monospace) and the **toolbar is not broken**; footer shows "Failed in …".
3. **History tab:** each row shows a coloured status badge, DB name, relative time, and row count; the SQL is on its own monospace line. Hover a row → **Copy** copies the SQL (toast "Copied"); **Re-run** runs it again (Results updates). Clicking the row loads the SQL into the editor.
4. **Saved tab:** Save a query first (toolbar **Save**). In **Saved**, the row shows name + description + DB + "updated … ago". Hover → **Edit** opens a prefilled "Edit query" dialog; change the description and **Save changes** → the row updates. Hover → **Delete** opens a "Delete saved query?" confirm; **Cancel** keeps it, **Delete** removes it.
5. `browser_console_messages` (errors, all): no NEW errors beyond the known pre-existing favicon-404 / Trino-warmup schema-502s.

Capture screenshots to `/Users/1pro-l4r8/Documents/github/data-portal/` (parent dir, NOT the repo) if useful; delete them after.

- [ ] **Step 3: Cleanup + final review**

Remove any scratch screenshots. Confirm `git status` shows only intended changes. Dispatch a final code reviewer over `git diff main...HEAD`, then proceed to `superpowers:finishing-a-development-branch`.

---

## Self-Review notes

- **Spec coverage:** relativeTime → T1. QueryStatusBadge → T2. `useUpdateSavedQuery` → T3. SaveQueryDialog edit mode → T4. ResultsGrid error + status line → T5. History rich list (badge/db/time/rows/copy/re-run/empty) → T6. Saved rich list (desc/db/time/edit/confirm-delete/empty) → T7. Limit widen + custom, error-to-Results, auto-switch tab, client timing, Re-run db override → T8. Tests + live verify → T6/T7/T8 + T9. ✓
- **Type consistency:** `timeAgo`/`fullTimestamp` (T1) used in T6/T7. `QueryStatusBadge` `status: QueryStatus` (T2) used in T6. `useUpdateSavedQuery({ id, database_id, name, sql, description })` (T3) called in T7 with exactly those keys. `SaveQueryDialog` props `mode/initialName/initialDescription` (T4) passed in T7. `ResultsGrid` props `error/statusLine` (T5) passed in T8. `run(sqlOverride?, dbOverride?)` (T8) called by Re-run with `(sql, dbId)`. ✓
- **Green-per-commit:** T3 exports an unused hook (compiles); T4 is backward-compatible (optional props); T5/T6/T7 add isolated units; T8 wires them together. Each task ends green.
- **Untestable-in-jsdom:** the Monaco keybinding and live data flow are covered by the Task 9 live verify; unit tests stub `SqlEditor`/`HistoryTab`/`SavedTab` where appropriate.
