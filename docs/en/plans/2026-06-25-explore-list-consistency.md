# Explore List Consistency & Polish — Implementation Plan (Project A)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the Charts and Dashboards Explore lists up to the rich Datasets pattern and lift all three with shared building blocks: relative timestamps, a loading skeleton, viz-type icons, kebab row-actions, and confirm-on-irreversible-delete.

**Architecture:** Frontend-only (React 18 + TS). Extract small shared presentational/util components under `components/explore/` (+ a `ui/skeleton` primitive), then apply them across the three list pages. No backend/API changes.

**Tech Stack:** React 18, TypeScript, Vite, Vitest + Testing Library + `@testing-library/user-event`, `@tanstack/react-query`, shadcn primitives (`DropdownMenu`, `AlertDialog`, `Select`, `Input`), `lucide-react`, `sonner`, `dayjs` (via existing `lib/relativeTime.ts`).

**Conventions:**
- All commands run from `egret/frontend/`.
- Full sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`.
- Git repo root is `egret/`, so `git add` paths need the `frontend/` prefix. Docs use `docs/`.
- Branch `feat/explore-list-consistency` (already checked out). Git identity configured — never override the author.
- End every commit message with: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

**Spec:** `docs/en/specs/2026-06-25-explore-list-consistency-design.md`

---

## File Structure

**New:**
- `frontend/src/components/ui/skeleton.tsx` — `Skeleton` primitive.
- `frontend/src/components/explore/RelativeTime.tsx` — `<RelativeTime iso/>`.
- `frontend/src/components/explore/SortableHeader.tsx` — generic sortable `<th>` + `SortState`.
- `frontend/src/components/explore/RowActionsMenu.tsx` — kebab shell.
- `frontend/src/components/explore/ListSkeleton.tsx` — table loading skeleton.
- `frontend/src/components/chart/ChartActionsMenu.tsx` — charts row kebab.
- `frontend/src/components/chart/dashboard/DashboardActionsMenu.tsx` — dashboards row kebab.

**Modified:**
- `frontend/src/routes/explore/Charts.tsx` — rich rewrite.
- `frontend/src/routes/explore/Dashboards.tsx` — rich rewrite (keep trash/bookmark/create).
- `frontend/src/routes/explore/Datasets.tsx` — relative time + skeleton + use shared `SortableHeader`.
- `frontend/src/components/dataset/DatasetActionsMenu.tsx` — compose `RowActionsMenu` (light DRY).
- Existing dashboards tests (`routes/explore/__tests__/Dashboards*.test.tsx`) — update mocks + open kebab.

**Tests (new):**
- `frontend/src/components/ui/__tests__/skeleton.test.tsx`
- `frontend/src/components/explore/__tests__/RelativeTime.test.tsx`
- `frontend/src/components/explore/__tests__/SortableHeader.test.tsx`
- `frontend/src/components/explore/__tests__/RowActionsMenu.test.tsx`
- `frontend/src/components/explore/__tests__/ListSkeleton.test.tsx`
- `frontend/src/routes/explore/__tests__/Charts.test.tsx`

---

## Task 1: `Skeleton` primitive

**Files:**
- Create: `frontend/src/components/ui/skeleton.tsx`
- Test: `frontend/src/components/ui/__tests__/skeleton.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/ui/__tests__/skeleton.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { Skeleton } from "@/components/ui/skeleton";

test("renders an animated placeholder with passed classes", () => {
  render(<Skeleton data-testid="sk" className="h-4 w-10" />);
  const el = screen.getByTestId("sk");
  expect(el.className).toContain("animate-pulse");
  expect(el.className).toContain("h-4");
});
```

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/components/ui/__tests__/skeleton.test.tsx`
Expected: FAIL — cannot resolve `@/components/ui/skeleton`.

- [ ] **Step 3: Create the primitive**

Create `frontend/src/components/ui/skeleton.tsx`:
```tsx
import { cn } from "@/lib/utils";

export function Skeleton({ className, ...props }: React.ComponentProps<"div">) {
  return <div data-slot="skeleton" className={cn("animate-pulse rounded-md bg-muted", className)} {...props} />;
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/components/ui/__tests__/skeleton.test.tsx`
Expected: PASS (1 test).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/ui/skeleton.tsx frontend/src/components/ui/__tests__/skeleton.test.tsx
git commit -m "feat(ui): Skeleton primitive

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `RelativeTime`

**Files:**
- Create: `frontend/src/components/explore/RelativeTime.tsx`
- Test: `frontend/src/components/explore/__tests__/RelativeTime.test.tsx`

Context: `lib/relativeTime.ts` exports `timeAgo(iso)` and `fullTimestamp(iso)`.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/explore/__tests__/RelativeTime.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { fullTimestamp } from "@/lib/relativeTime";

test("shows relative text with a full-timestamp tooltip", () => {
  const iso = new Date(Date.now() - 5 * 60_000).toISOString();
  render(<RelativeTime iso={iso} />);
  const el = screen.getByText(/ago$/);
  expect(el).toHaveAttribute("title", fullTimestamp(iso));
});
```

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/components/explore/__tests__/RelativeTime.test.tsx`
Expected: FAIL — cannot resolve `RelativeTime`.

- [ ] **Step 3: Create the component**

Create `frontend/src/components/explore/RelativeTime.tsx`:
```tsx
import { timeAgo, fullTimestamp } from "@/lib/relativeTime";

export function RelativeTime({ iso, className }: { iso: string; className?: string }) {
  return <span className={className} title={fullTimestamp(iso)}>{timeAgo(iso)}</span>;
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/components/explore/__tests__/RelativeTime.test.tsx`
Expected: PASS (1 test).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/explore/RelativeTime.tsx frontend/src/components/explore/__tests__/RelativeTime.test.tsx
git commit -m "feat(explore): RelativeTime cell (timeAgo + full tooltip)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `SortableHeader`

**Files:**
- Create: `frontend/src/components/explore/SortableHeader.tsx`
- Test: `frontend/src/components/explore/__tests__/SortableHeader.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/explore/__tests__/SortableHeader.test.tsx`:
```tsx
import { render, screen, fireEvent } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { SortableHeader } from "@/components/explore/SortableHeader";

test("calls onSort with its key when clicked", () => {
  const onSort = vi.fn();
  render(<table><thead><tr>
    <SortableHeader sortKey="name" label="Name" current={{ key: "updated", dir: "desc" }} onSort={onSort} />
  </tr></thead></table>);
  fireEvent.click(screen.getByText("Name"));
  expect(onSort).toHaveBeenCalledWith("name");
});

test("shows a direction arrow only on the active key", () => {
  const { container, rerender } = render(<table><thead><tr>
    <SortableHeader sortKey="name" label="Name" current={{ key: "name", dir: "asc" }} onSort={() => {}} />
  </tr></thead></table>);
  expect(container.querySelector("svg")).toBeTruthy();
  rerender(<table><thead><tr>
    <SortableHeader sortKey="name" label="Name" current={{ key: "updated", dir: "asc" }} onSort={() => {}} />
  </tr></thead></table>);
  expect(container.querySelector("svg")).toBeNull();
});
```

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/components/explore/__tests__/SortableHeader.test.tsx`
Expected: FAIL — cannot resolve `SortableHeader`.

- [ ] **Step 3: Create the component**

Create `frontend/src/components/explore/SortableHeader.tsx`:
```tsx
import { ArrowDown, ArrowUp } from "lucide-react";

export interface SortState<K extends string> { key: K; dir: "asc" | "desc" }

export function SortableHeader<K extends string>({ sortKey, label, current, onSort, className }: {
  sortKey: K;
  label: string;
  current: SortState<K>;
  onSort: (key: K) => void;
  className?: string;
}) {
  const active = current.key === sortKey;
  return (
    <th className={`cursor-pointer select-none py-1 hover:text-foreground ${className ?? ""}`} onClick={() => onSort(sortKey)}>
      <span className="inline-flex items-center gap-1">
        {label}
        {active && (current.dir === "asc" ? <ArrowUp className="h-3 w-3" /> : <ArrowDown className="h-3 w-3" />)}
      </span>
    </th>
  );
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/components/explore/__tests__/SortableHeader.test.tsx`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/explore/SortableHeader.tsx frontend/src/components/explore/__tests__/SortableHeader.test.tsx
git commit -m "feat(explore): generic SortableHeader

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `RowActionsMenu`

**Files:**
- Create: `frontend/src/components/explore/RowActionsMenu.tsx`
- Test: `frontend/src/components/explore/__tests__/RowActionsMenu.test.tsx`

Context: `@/components/ui/dropdown-menu` exports `DropdownMenu`, `DropdownMenuContent`, `DropdownMenuTrigger`, `DropdownMenuItem`, `DropdownMenuSeparator`. Radix menus open in jsdom via `@testing-library/user-event` (see `components/chart/__tests__/ChartExportMenu.test.tsx`).

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/explore/__tests__/RowActionsMenu.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { DropdownMenuItem } from "@/components/ui/dropdown-menu";

test("opens to reveal its items", async () => {
  const user = userEvent.setup();
  render(<RowActionsMenu><DropdownMenuItem>Delete</DropdownMenuItem></RowActionsMenu>);
  await user.click(screen.getByRole("button", { name: "Actions" }));
  expect(await screen.findByText("Delete")).toBeTruthy();
});
```

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/components/explore/__tests__/RowActionsMenu.test.tsx`
Expected: FAIL — cannot resolve `RowActionsMenu`.

- [ ] **Step 3: Create the component**

Create `frontend/src/components/explore/RowActionsMenu.tsx`:
```tsx
import type { ReactNode } from "react";
import { MoreHorizontal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";

export function RowActionsMenu({ children, label = "Actions" }: { children: ReactNode; label?: string }) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="sm" className="h-7 w-7 p-0" title={label} aria-label={label}>
          <MoreHorizontal className="h-4 w-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">{children}</DropdownMenuContent>
    </DropdownMenu>
  );
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/components/explore/__tests__/RowActionsMenu.test.tsx`
Expected: PASS (1 test).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/explore/RowActionsMenu.tsx frontend/src/components/explore/__tests__/RowActionsMenu.test.tsx
git commit -m "feat(explore): RowActionsMenu kebab shell

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: `ListSkeleton`

**Files:**
- Create: `frontend/src/components/explore/ListSkeleton.tsx`
- Test: `frontend/src/components/explore/__tests__/ListSkeleton.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/explore/__tests__/ListSkeleton.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { ListSkeleton } from "@/components/explore/ListSkeleton";

test("renders the requested number of skeleton rows", () => {
  render(<ListSkeleton rows={3} cols={2} />);
  expect(screen.getByTestId("list-skeleton").children.length).toBe(3);
});
```

- [ ] **Step 2: Run the test, verify it FAILS**

Run: `npx vitest run src/components/explore/__tests__/ListSkeleton.test.tsx`
Expected: FAIL — cannot resolve `ListSkeleton`.

- [ ] **Step 3: Create the component**

Create `frontend/src/components/explore/ListSkeleton.tsx`:
```tsx
import { Skeleton } from "@/components/ui/skeleton";

export function ListSkeleton({ rows = 8, cols = 4 }: { rows?: number; cols?: number }) {
  return (
    <div data-testid="list-skeleton" className="space-y-2">
      {Array.from({ length: rows }).map((_, r) => (
        <div key={r} className="flex gap-3">
          {Array.from({ length: cols }).map((_, c) => (
            <Skeleton key={c} className="h-5 flex-1" />
          ))}
        </div>
      ))}
    </div>
  );
}
```

- [ ] **Step 4: Run the test, verify it PASSES**

Run: `npx vitest run src/components/explore/__tests__/ListSkeleton.test.tsx`
Expected: PASS (1 test).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/explore/ListSkeleton.tsx frontend/src/components/explore/__tests__/ListSkeleton.test.tsx
git commit -m "feat(explore): ListSkeleton loading state

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 6: Charts list — rich rewrite + `ChartActionsMenu`

**Files:**
- Create: `frontend/src/components/chart/ChartActionsMenu.tsx`
- Modify: `frontend/src/routes/explore/Charts.tsx` (full rewrite)
- Test: `frontend/src/routes/explore/__tests__/Charts.test.tsx` (new)

Context: `Chart { id, dataset_id, name, description, viz_type, params, created_by, updated_at }`; `useCharts(ws) → { data, isLoading }`; `useChartMutations(ws) → { create, update, remove }` (remove is hard delete; no duplicate). `CHART_TYPES: { key, name, description, icon }[]`.

- [ ] **Step 1: Create `ChartActionsMenu`**

Create `frontend/src/components/chart/ChartActionsMenu.tsx`:
```tsx
import { useNavigate } from "react-router-dom";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import type { Chart } from "@/api/charts";

export function ChartActionsMenu({ ws, chart, onDelete }: {
  ws: string; chart: Chart; onDelete: (c: Chart) => void;
}) {
  const nav = useNavigate();
  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/charts/${chart.id}`)}>Open</DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDelete(chart)}>Delete</DropdownMenuItem>
    </RowActionsMenu>
  );
}
```

- [ ] **Step 2: Rewrite `Charts.tsx`**

Replace the ENTIRE `frontend/src/routes/explore/Charts.tsx` with:
```tsx
import { useMemo, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { Search } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import {
  AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent,
  AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { useCharts, useChartMutations, type Chart } from "@/api/charts";
import { CHART_TYPES } from "@/components/chart/chartTypes";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { ChartActionsMenu } from "@/components/chart/ChartActionsMenu";

type SortKey = "name" | "viz_type" | "updated_at";
const vizMeta = (v: string) => CHART_TYPES.find((t) => t.key === v);
const vizLabel = (v: string) => vizMeta(v)?.name ?? v;

export default function Charts() {
  const ws = useParams().workspace!;
  const { data: charts, isLoading } = useCharts(ws);
  const { remove } = useChartMutations(ws);

  const [query, setQuery] = useState("");
  const [typeFilter, setTypeFilter] = useState("all");
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [deleteTarget, setDeleteTarget] = useState<Chart | null>(null);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (charts ?? []).filter((c) => {
      if (typeFilter !== "all" && c.viz_type !== typeFilter) return false;
      if (!q) return true;
      return [c.name, vizLabel(c.viz_type)].join(" ").toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      let cmp = 0;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "viz_type") cmp = vizLabel(a.viz_type).localeCompare(vizLabel(b.viz_type));
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [charts, query, typeFilter, sort]);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  async function doDelete(c: Chart) {
    try { await remove.mutateAsync(c.id); toast.success("Deleted"); }
    catch (e) { toast.error(String(e)); }
  }

  const hasData = !!charts && charts.length > 0;
  const presentTypes = CHART_TYPES.filter((t) => (charts ?? []).some((c) => c.viz_type === t.key));

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">Charts</h1>
        <Button asChild><Link to={`/explore/${ws}/charts/new`}>+ Chart</Link></Button>
      </div>

      {isLoading ? (
        <ListSkeleton cols={4} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search charts…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
            <Select value={typeFilter} onValueChange={setTypeFilter}>
              <SelectTrigger className="h-9 w-44"><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All types</SelectItem>
                {presentTypes.map((t) => <SelectItem key={t.key} value={t.key}>{t.name}</SelectItem>)}
              </SelectContent>
            </Select>
          </div>

          {rows.length > 0 ? (
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="viz_type" label="Type" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="updated_at" label="Updated" current={sort} onSort={onSort} />
                  <th className="w-8" />
                </tr>
              </thead>
              <tbody>
                {rows.map((c) => {
                  const Icon = vizMeta(c.viz_type)?.icon;
                  return (
                    <tr key={c.id} className="border-t hover:bg-accent">
                      <td className="py-1">
                        <Link className="inline-flex items-center gap-2 font-medium hover:underline" to={`/explore/${ws}/charts/${c.id}`}>
                          {Icon && <Icon className="h-4 w-4 shrink-0 text-muted-foreground" />}
                          {c.name}
                        </Link>
                      </td>
                      <td className="text-muted-foreground">{vizLabel(c.viz_type)}</td>
                      <td className="text-muted-foreground"><RelativeTime iso={c.updated_at} /></td>
                      <td><ChartActionsMenu ws={ws} chart={c} onDelete={setDeleteTarget} /></td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          ) : (
            <p className="text-muted-foreground">No charts match the current filters.</p>
          )}
        </>
      ) : (
        <div className="rounded border border-dashed p-8 text-center text-muted-foreground">
          <p>No charts yet. Build a chart from a dataset.</p>
          <Button asChild className="mt-3"><Link to={`/explore/${ws}/charts/new`}>Create your first chart</Link></Button>
        </div>
      )}

      <AlertDialog open={!!deleteTarget} onOpenChange={(o) => { if (!o) setDeleteTarget(null); }}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete chart “{deleteTarget?.name}”?</AlertDialogTitle>
            <AlertDialogDescription>This permanently deletes the chart. This cannot be undone.</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
              onClick={() => { if (deleteTarget) doDelete(deleteTarget); setDeleteTarget(null); }}>Delete</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
```

- [ ] **Step 3: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS.

- [ ] **Step 4: Write the Charts test**

Create `frontend/src/routes/explore/__tests__/Charts.test.tsx`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

const removeMock = vi.fn().mockResolvedValue(undefined);
// c1 is newest → first row under the default "updated_at desc" sort.
const charts = [
  { id: "c1", dataset_id: "d1", name: "Revenue bar", description: "", viz_type: "bar", params: {}, created_by: null, updated_at: "2026-06-21T00:00:00Z" },
  { id: "c2", dataset_id: "d1", name: "Orders line", description: "", viz_type: "line", params: {}, created_by: null, updated_at: "2026-06-20T00:00:00Z" },
];
vi.mock("@/api/charts", () => ({
  useCharts: () => ({ data: charts, isLoading: false }),
  useChartMutations: () => ({ remove: { mutateAsync: removeMock } }),
}));

import Charts from "../Charts";

function renderAt() {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/explore/eng/charts"]}>
        <Routes><Route path="/explore/:workspace/charts" element={<Charts />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe("Charts list", () => {
  it("renders chart rows with their type labels", () => {
    renderAt();
    expect(screen.getByText("Revenue bar")).toBeInTheDocument();
    expect(screen.getByText("Orders line")).toBeInTheDocument();
    expect(screen.getByText("Bar")).toBeInTheDocument();
  });
  it("search filters by name", () => {
    renderAt();
    fireEvent.change(screen.getByPlaceholderText("Search charts…"), { target: { value: "orders" } });
    expect(screen.queryByText("Revenue bar")).toBeNull();
    expect(screen.getByText("Orders line")).toBeInTheDocument();
  });
  it("Delete asks to confirm, then deletes the first row", async () => {
    removeMock.mockClear();
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);
    await user.click(await screen.findByText("Delete"));
    const dialog = await screen.findByRole("alertdialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(removeMock).toHaveBeenCalledWith("c1");
  });
});
```

- [ ] **Step 5: Run the Charts test + full suite**

Run: `npx vitest run src/routes/explore/__tests__/Charts.test.tsx` → expect 3 pass.
Then `npx vitest run` → all green.

- [ ] **Step 6: Full sweep + commit**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run` → all PASS, 0 lint errors.
```bash
git add frontend/src/components/chart/ChartActionsMenu.tsx frontend/src/routes/explore/Charts.tsx frontend/src/routes/explore/__tests__/Charts.test.tsx
git commit -m "feat(charts): rich list (search, type filter+icons, sort, confirm-delete, relative time)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 7: Dashboards list — rich rewrite + `DashboardActionsMenu`

**Files:**
- Create: `frontend/src/components/chart/dashboard/DashboardActionsMenu.tsx`
- Modify: `frontend/src/routes/explore/Dashboards.tsx` (full rewrite)
- Modify (tests): `frontend/src/routes/explore/__tests__/Dashboards.test.tsx`, `Dashboards.bookmark.test.tsx`, `Dashboards.trash.test.tsx`

Context: `DashboardSummary { id, name, card_count, updated_at, bookmarked?, … }`; `useDashboards(ws, { trashed })`; `useDashboardMutations → { create, remove, duplicate, trash, restore, addBookmark, removeBookmark }`. Move-to-trash is reversible (no confirm); permanent delete (trash view) keeps `ConfirmDeleteDialog`.

- [ ] **Step 1: Create `DashboardActionsMenu`**

Create `frontend/src/components/chart/dashboard/DashboardActionsMenu.tsx`:
```tsx
import { useNavigate } from "react-router-dom";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import type { DashboardSummary } from "@/api/dashboards";

export function DashboardActionsMenu({ ws, dash, trashed, onDuplicate, onTrash, onRestore, onDeletePermanent }: {
  ws: string;
  dash: DashboardSummary;
  trashed: boolean;
  onDuplicate: (d: DashboardSummary) => void;
  onTrash: (d: DashboardSummary) => void;
  onRestore: (d: DashboardSummary) => void;
  onDeletePermanent: (d: DashboardSummary) => void;
}) {
  const nav = useNavigate();
  if (trashed) {
    return (
      <RowActionsMenu>
        <DropdownMenuItem onClick={() => onRestore(dash)}>Restore</DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDeletePermanent(dash)}>Delete permanently</DropdownMenuItem>
      </RowActionsMenu>
    );
  }
  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/dashboards/${dash.id}`)}>Open</DropdownMenuItem>
      <DropdownMenuItem onClick={() => onDuplicate(dash)}>Duplicate</DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={() => onTrash(dash)}>Move to Trash</DropdownMenuItem>
    </RowActionsMenu>
  );
}
```

- [ ] **Step 2: Rewrite `Dashboards.tsx`**

Replace the ENTIRE `frontend/src/routes/explore/Dashboards.tsx` with:
```tsx
import { useMemo, useState } from "react";
import { Search, Star } from "lucide-react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { toast } from "sonner";
import { useDashboards, useDashboardMutations, type DashboardSummary } from "@/api/dashboards";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogTrigger } from "@/components/ui/dialog";
import { ConfirmDeleteDialog } from "@/components/ui/confirm-delete-dialog";
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader, type SortState } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { DashboardActionsMenu } from "@/components/chart/dashboard/DashboardActionsMenu";

type SortKey = "name" | "card_count" | "updated_at";

export default function Dashboards() {
  const ws = useParams().workspace!;
  const nav = useNavigate();
  const [showTrash, setShowTrash] = useState(false);
  const { data, isLoading } = useDashboards(ws, { trashed: showTrash });
  const { create, remove, duplicate, trash, restore, addBookmark, removeBookmark } = useDashboardMutations(ws);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [onlyBookmarked, setOnlyBookmarked] = useState(false);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState<SortKey>>({ key: "updated_at", dir: "desc" });
  const [pendingDelete, setPendingDelete] = useState<DashboardSummary | null>(null);

  const rows = useMemo(() => {
    const q = query.trim().toLowerCase();
    let r = (data ?? []).filter((d) => {
      if (onlyBookmarked && !d.bookmarked) return false;
      if (!q) return true;
      return d.name.toLowerCase().includes(q);
    });
    r = [...r].sort((a, b) => {
      let cmp = 0;
      if (sort.key === "name") cmp = a.name.localeCompare(b.name);
      else if (sort.key === "card_count") cmp = a.card_count - b.card_count;
      else cmp = a.updated_at.localeCompare(b.updated_at);
      return sort.dir === "asc" ? cmp : -cmp;
    });
    return r;
  }, [data, onlyBookmarked, query, sort]);

  function onSort(key: SortKey) {
    setSort((s) => (s.key === key ? { key, dir: s.dir === "asc" ? "desc" : "asc" } : { key, dir: "asc" }));
  }
  async function onCreate() {
    if (!name.trim()) { toast.error("Name required"); return; }
    try { const d = await create.mutateAsync({ name: name.trim() }); setOpen(false); setName(""); nav(`/explore/${ws}/dashboards/${d.id}`); }
    catch (e) { toast.error(String(e)); }
  }
  const guard = (p: Promise<unknown>) => p.catch((e) => toast.error(String(e)));

  const hasData = !!data && data.length > 0;

  return (
    <div className="p-4">
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-lg font-semibold">{showTrash ? "Trash" : "Dashboards"}</h1>
        <div className="flex gap-2">
          {!showTrash && (
            <Button variant={onlyBookmarked ? "default" : "outline"} onClick={() => setOnlyBookmarked((v) => !v)}>★ Bookmarks</Button>
          )}
          <Button variant={showTrash ? "default" : "outline"} onClick={() => { const next = !showTrash; setShowTrash(next); if (next) setOnlyBookmarked(false); }}>
            {showTrash ? "← Dashboards" : "Trash"}
          </Button>
          <Dialog open={open} onOpenChange={setOpen}>
            <DialogTrigger asChild><Button>+ Dashboard</Button></DialogTrigger>
            <DialogContent>
              <DialogHeader><DialogTitle>New dashboard</DialogTitle></DialogHeader>
              <div className="flex gap-2 py-2">
                <Input autoFocus placeholder="Name" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") onCreate(); }} />
                <Button onClick={onCreate} disabled={create.isPending}>Create</Button>
              </div>
            </DialogContent>
          </Dialog>
        </div>
      </div>

      {isLoading ? (
        <ListSkeleton cols={4} />
      ) : hasData ? (
        <>
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="relative max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
              <Input className="pl-8" placeholder="Search dashboards…" value={query} onChange={(e) => setQuery(e.target.value)} />
            </div>
          </div>

          {rows.length > 0 ? (
            <table className="w-full text-sm">
              <thead className="text-left text-muted-foreground">
                <tr>
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="card_count" label="Cards" current={sort} onSort={onSort} />
                  <SortableHeader sortKey="updated_at" label="Updated" current={sort} onSort={onSort} />
                  <th className="w-8" />
                </tr>
              </thead>
              <tbody>
                {rows.map((d) => (
                  <tr key={d.id} className="border-t hover:bg-accent">
                    <td className="py-1">
                      <span className="inline-flex items-center gap-1">
                        {!showTrash && (
                          <Button size="sm" variant="ghost" className="h-7 w-7 p-0" aria-label={`${d.bookmarked ? "unbookmark" : "bookmark"} ${d.name}`}
                            onClick={() => guard((d.bookmarked ? removeBookmark : addBookmark).mutateAsync(d.id))}>
                            <Star className={`h-4 w-4 ${d.bookmarked ? "fill-amber-400 text-amber-400" : "text-muted-foreground"}`} />
                          </Button>
                        )}
                        <Link className="font-medium hover:underline" to={`/explore/${ws}/dashboards/${d.id}`}>{d.name}</Link>
                      </span>
                    </td>
                    <td className="text-muted-foreground">{d.card_count}</td>
                    <td className="text-muted-foreground"><RelativeTime iso={d.updated_at} /></td>
                    <td>
                      <DashboardActionsMenu
                        ws={ws} dash={d} trashed={showTrash}
                        onDuplicate={(x) => guard(duplicate.mutateAsync(x.id).then((c) => nav(`/explore/${ws}/dashboards/${c.id}`)))}
                        onTrash={(x) => guard(trash.mutateAsync(x.id).then(() => toast.success("Moved to Trash")))}
                        onRestore={(x) => guard(restore.mutateAsync(x.id))}
                        onDeletePermanent={setPendingDelete}
                      />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <p className="text-muted-foreground">{onlyBookmarked ? "No bookmarked dashboards." : "No dashboards match the current filters."}</p>
          )}
        </>
      ) : (
        <p className="text-muted-foreground">{showTrash ? "Trash is empty." : "No dashboards yet."}</p>
      )}

      <ConfirmDeleteDialog
        open={pendingDelete !== null}
        onOpenChange={(o) => { if (!o) setPendingDelete(null); }}
        title="Delete permanently"
        description={`Permanently delete "${pendingDelete?.name ?? ""}"? This cannot be undone.`}
        onConfirm={async () => { if (!pendingDelete) return; await guard(remove.mutateAsync(pendingDelete.id)); setPendingDelete(null); }}
      />
    </div>
  );
}
```

- [ ] **Step 3: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS. If `duplicate.mutateAsync(...).then((c) => …c.id)` errors on `c`'s type, annotate as `.then((c: { id: string }) => …)` — but try inference first.

- [ ] **Step 4: Update the existing Dashboards tests**

Read `routes/explore/__tests__/Dashboards.test.tsx`, `Dashboards.bookmark.test.tsx`, `Dashboards.trash.test.tsx`. For EACH:
1. Ensure the `useDashboardMutations` mock returns ALL of `{ create, remove, duplicate, trash, restore, addBookmark, removeBookmark }` (each `{ mutateAsync: vi.fn().mockResolvedValue(...) }`; `create`/`duplicate` resolve to `{ id: "x" }`). Add any missing keys.
2. Row actions (Duplicate / Delete→"Move to Trash" / Restore / "Delete permanently") are now INSIDE the kebab. Where a test clicked those buttons directly, first open the kebab: `const user = userEvent.setup(); await user.click(screen.getAllByRole("button", { name: "Actions" })[0]);` then `await user.click(await screen.findByText("Move to Trash"))` (or "Duplicate"/"Restore"/"Delete permanently"). Import `userEvent from "@testing-library/user-event"` and make the test `async`.
3. The non-trash destructive action label changed from "Delete" to **"Move to Trash"**; the trash-view permanent one is **"Delete permanently"** (then the `ConfirmDeleteDialog` requires typing "delete" — preserve any existing confirm interaction).
4. Bookmark star button `aria-label` is unchanged (`bookmark <name>` / `unbookmark <name>`).
Keep every existing assertion's intent; do NOT weaken. Report exactly what you changed per file.

- [ ] **Step 5: Run dashboards tests + full suite**

Run: `npx vitest run src/routes/explore/__tests__/` → all green.
Then `npx vitest run` → all green. If a dashboards test still fails, fix the test setup (mock/kebab-open) — not the component — unless the component genuinely regressed; if so, STOP and report.

- [ ] **Step 6: Full sweep + commit**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run` → all PASS, 0 lint errors.
```bash
git add frontend/src/components/chart/dashboard/DashboardActionsMenu.tsx frontend/src/routes/explore/Dashboards.tsx frontend/src/routes/explore/__tests__/Dashboards.test.tsx frontend/src/routes/explore/__tests__/Dashboards.bookmark.test.tsx frontend/src/routes/explore/__tests__/Dashboards.trash.test.tsx
git commit -m "feat(dashboards): rich list (search, sort, kebab actions, relative time, skeleton)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 8: Datasets — relative time, skeleton, shared SortableHeader + RowActionsMenu

**Files:**
- Modify: `frontend/src/routes/explore/Datasets.tsx`
- Modify: `frontend/src/components/dataset/DatasetActionsMenu.tsx`

- [ ] **Step 1: Datasets — relative time + skeleton + shared header**

In `frontend/src/routes/explore/Datasets.tsx`:

(a) Add imports near the other component imports:
```tsx
import { RelativeTime } from "@/components/explore/RelativeTime";
import { SortableHeader } from "@/components/explore/SortableHeader";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
```
(b) DELETE the local `Th` component definition:
```tsx
  const Th = ({ k, label }: { k: SortKey; label: string }) => (
    <th className="cursor-pointer select-none py-1 hover:text-foreground" onClick={() => setSortKey(k)}>
      <span className="inline-flex items-center gap-1">{label}
        {sort.key === k && (sort.dir === "asc" ? <ArrowUp className="h-3 w-3" /> : <ArrowDown className="h-3 w-3" />)}
      </span>
    </th>
  );
```
(c) Replace the three `<Th .../>` usages in the `<thead>` with `SortableHeader`:
```tsx
                  <SortableHeader sortKey="name" label="Name" current={sort} onSort={setSortKey} />
                  <th>Kind</th>
                  <th>Database</th>
                  <th>Source</th>
                  <SortableHeader sortKey="column_count" label="Columns" current={sort} onSort={setSortKey} />
                  <SortableHeader sortKey="updated_at" label="Updated" current={sort} onSort={setSortKey} />
```
(d) Replace the Updated cell `{new Date(d.updated_at).toLocaleString()}` with:
```tsx
                    <td className="text-muted-foreground"><RelativeTime iso={d.updated_at} /></td>
```
(e) Replace the loading branch `<p className="text-muted-foreground">Loading…</p>` (the one gated by `isLoading`) with:
```tsx
        <ListSkeleton cols={6} />
```
(f) Remove the now-unused `ArrowDown, ArrowUp` from the `lucide-react` import (keep `Search`). If `ArrowDown`/`ArrowUp` are no longer referenced, drop them to satisfy lint.

- [ ] **Step 2: DatasetActionsMenu — compose RowActionsMenu**

Replace the ENTIRE `frontend/src/components/dataset/DatasetActionsMenu.tsx` with:
```tsx
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { useRefreshDataset, type DatasetSummary } from "@/api/datasets";

/** Per-row actions: Edit (→ detail edit), Refresh, Duplicate (virtual), Delete. */
export function DatasetActionsMenu({ ws, ds, onDuplicate, onDelete }: {
  ws: string;
  ds: DatasetSummary;
  onDuplicate: (ds: DatasetSummary) => void;
  onDelete: (ds: DatasetSummary) => void;
}) {
  const nav = useNavigate();
  const refresh = useRefreshDataset(ws, ds.id);

  return (
    <RowActionsMenu>
      <DropdownMenuItem onClick={() => nav(`/explore/${ws}/datasets/${ds.id}?edit=1`)}>Edit</DropdownMenuItem>
      <DropdownMenuItem
        disabled={refresh.isPending}
        onClick={() => refresh.mutateAsync().then(() => toast.success("Refreshed")).catch((e) => toast.error(String(e)))}>
        {refresh.isPending ? "Refreshing…" : "Refresh"}
      </DropdownMenuItem>
      {ds.kind === "virtual" && <DropdownMenuItem onClick={() => onDuplicate(ds)}>Duplicate</DropdownMenuItem>}
      <DropdownMenuSeparator />
      <DropdownMenuItem className="text-destructive focus:text-destructive" onClick={() => onDelete(ds)}>Delete</DropdownMenuItem>
    </RowActionsMenu>
  );
}
```

- [ ] **Step 3: Typecheck + build**

Run: `npm run typecheck && npm run build`
Expected: PASS (no unused imports — confirm `ArrowUp`/`ArrowDown` were removed from Datasets and `MoreHorizontal`/`Button`/`DropdownMenu*` trigger imports were removed from DatasetActionsMenu).

- [ ] **Step 4: Run the suite (no regressions)**

Run: `npx vitest run`
Expected: all green. If a Datasets test asserts on the kebab actions, they still render the same item text ("Edit"/"Refresh"/"Duplicate"/"Delete") via the same trigger (`aria-label="Actions"`) — update the open interaction only if needed, without weakening assertions.

- [ ] **Step 5: Full sweep + commit**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run` → all PASS, 0 lint errors.
```bash
git add frontend/src/routes/explore/Datasets.tsx frontend/src/components/dataset/DatasetActionsMenu.tsx
git commit -m "refactor(datasets): relative time + skeleton + shared SortableHeader/RowActionsMenu

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

Dev stack is up (frontend `http://localhost:5173`, backend `:8080`). Log in `admin@egret.local` / `Admin12345!`.
1. **Charts** (`/explore/eng/charts`): rows show a **viz-type icon** next to the name + the type label; **Search** filters; the **type filter** narrows by viz; clicking a **Sortable** header toggles order; the kebab **⋯ → Delete** opens an **AlertDialog**, Cancel keeps it, Delete removes it; Updated shows **relative time** with a full-timestamp tooltip.
2. **Dashboards** (`/explore/eng/dashboards`): **Search** + **sort** work; kebab **⋯** has Open / Duplicate / Move to Trash; **Move to Trash** removes without a heavy confirm (toast "Moved to Trash"); **Trash** view shows Restore + Delete permanently (typed confirm); bookmark star still toggles; relative time.
3. **Datasets** (`/explore/eng/datasets`): Updated now shows **relative time**; the kebab still has Edit/Refresh/Duplicate/Delete; sort + search + bulk select unchanged.
4. Reload each page once and confirm a **skeleton** flashes while loading (or appears on a throttled reload).
5. `browser_console_messages` (errors, all): no NEW errors beyond the known pre-existing favicon-404 / Trino-warmup schema-502s.

Screenshots to `/Users/1pro-l4r8/Documents/github/data-portal/` (parent dir, NOT the repo); delete after.

- [ ] **Step 3: Cleanup + final review**

Remove scratch screenshots. Confirm `git status` clean. Dispatch a final code reviewer over `git diff main...HEAD`, then proceed to `superpowers:finishing-a-development-branch`.

---

## Self-Review notes

- **Spec coverage:** Skeleton → T1. RelativeTime → T2. SortableHeader → T3. RowActionsMenu → T4. ListSkeleton → T5. Charts rich (search/type-filter/icons/sort/confirm-delete/relative/skeleton/empty) → T6. Dashboards rich (search/sort/kebab/trash/bookmark/relative/skeleton) → T7. Datasets lift (relative/skeleton/shared header+menu) → T8. Tests + live verify → each task + T9. ✓
- **Type consistency:** `SortState<K>`/`SortableHeader` (T3) consumed by Charts (T6), Dashboards (T7), Datasets (T8) with each page's `SortKey`. `RowActionsMenu` (T4) composed by `ChartActionsMenu` (T6), `DashboardActionsMenu` (T7), `DatasetActionsMenu` (T8). `RelativeTime` (T2) + `ListSkeleton` (T5, built on `Skeleton` T1) used in T6/T7/T8. `Chart`/`DashboardSummary`/`DatasetSummary` fields match `api/*`. ✓
- **Green-per-commit:** T1–T5 add isolated, tested units. T6 adds Charts (new test). T7 rewrites Dashboards and updates its existing tests in the same commit. T8 refactors Datasets/menu (existing tests stay green). Each task ends with a full sweep.
- **Untestable-in-jsdom:** skeleton flashes + visual polish covered by the Task 9 live verify; Radix menu opening IS covered via `user-event` (proven by `ChartExportMenu.test.tsx`).
