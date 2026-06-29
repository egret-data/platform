# Catalog Browse Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Catalog Browse tab's flat namespace/table lists with a scalable, searchable namespace **tree + breadcrumb**, with selection synced to the URL.

**Architecture:** `CatalogBrowse.tsx` becomes an orchestrator (URL-synced selection + dialogs) rendering a new `NamespaceTree.tsx` (flat→nested fold, filter, recursive lazy-loading nodes) on the left and a breadcrumb + the existing `TableDetailView` on the right. No new dependencies; large per-namespace lists get a "Showing X of Y" note instead of virtualization.

**Tech Stack:** React 18 + Vite + Tailwind + shadcn (`Breadcrumb`, `Card`) + TanStack Query + react-router (`useSearchParams`) + Vitest.

**Spec:** `docs/en/specs/2026-06-29-catalog-browse-redesign-design.md`.

---

## Background facts (verified)

- `useCatalogNamespaces(ws, wid)` → `NamespaceDto[]` where `NamespaceDto = { levels: string[] }` (full tree, flat). `nsDotted(levels)` joins with `.`.
- `useNamespaceContents(ws, wid, ns: string | null)` → `{ tables: string[], views: string[] }`; **`enabled: !!ns`** (passing `null` disables the query — this is how a collapsed tree node avoids loading).
- `useTableDetail(ws, wid, ns, table)` → `TableDetail`; `enabled: !!ns && !!table`.
- `useCreateNamespace(ws, wid)` / `useDeleteNamespace(ws, wid)` exist and are already wired by the current dialogs.
- `TableDetailView`, `DetailSection`, `snapshotSummary`, `CopyButton`, `NewNamespaceDialog` already live in `CatalogBrowse.tsx` — **keep `TableDetailView` (+ its helpers) and `NewNamespaceDialog`**; only the browse/list scaffolding is replaced.
- shadcn breadcrumb exports: `Breadcrumb, BreadcrumbList, BreadcrumbItem, BreadcrumbLink, BreadcrumbPage, BreadcrumbSeparator` from `@/components/ui/breadcrumb`.
- `useSearchParams` from `react-router-dom` (same pattern as the Storage browser).
- Threshold constant: `MAX_VISIBLE = 200` (tables+views shown per namespace before the "Showing X of Y" note).

---

## Task 1: Tree data model + `buildNamespaceTree` helper

**Files:**
- Create: `frontend/src/routes/explore/catalog/namespaceTree.ts`
- Test: `frontend/src/routes/explore/catalog/__tests__/namespaceTree.test.ts`

### Step 1: Write the failing test

`namespaceTree.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { buildNamespaceTree, type NsNode } from "../namespaceTree";

describe("buildNamespaceTree", () => {
  it("folds a flat namespace list into a nested tree", () => {
    const tree = buildNamespaceTree([
      { levels: ["sale"] },
      { levels: ["sale", "2024"] },
      { levels: ["marketing"] },
    ]);
    expect(tree.map((n) => n.name)).toEqual(["marketing", "sale"]); // sorted
    const sale = tree.find((n) => n.name === "sale")!;
    expect(sale.dotted).toBe("sale");
    expect(sale.children.map((c) => c.name)).toEqual(["2024"]);
    expect(sale.children[0].dotted).toBe("sale.2024");
  });

  it("synthesizes intermediate parents not present in the input", () => {
    // A child whose parent namespace isn't itself listed must still nest.
    const tree = buildNamespaceTree([{ levels: ["a", "b", "c"] }]);
    expect(tree.map((n) => n.name)).toEqual(["a"]);
    expect(tree[0].children[0].name).toBe("b");
    expect(tree[0].children[0].children[0].dotted).toBe("a.b.c");
  });

  it("marks nodes that exist as real namespaces vs synthesized", () => {
    const tree = buildNamespaceTree([{ levels: ["a", "b"] }]);
    expect(tree[0].exists).toBe(false); // 'a' was only implied
    expect(tree[0].children[0].exists).toBe(true); // 'a.b' was listed
  });
});
```

### Step 2: Run to verify it fails

Run: `cd frontend && npx vitest run src/routes/explore/catalog/__tests__/namespaceTree.test.ts`
Expected: FAIL (module not found).

### Step 3: Implement `namespaceTree.ts`

```ts
import { type NamespaceDto } from "@/api/catalog";

export interface NsNode {
  /** Last path level, e.g. "2024". */
  name: string;
  /** Full dotted path, e.g. "sale.2024". */
  dotted: string;
  /** Path levels. */
  levels: string[];
  /** True if this namespace was in the input (vs an implied intermediate). */
  exists: boolean;
  children: NsNode[];
}

/**
 * Fold a flat list of namespaces (each as its `levels`) into a nested tree,
 * synthesizing any intermediate parents that weren't listed. Children and roots
 * are sorted by name.
 */
export function buildNamespaceTree(list: NamespaceDto[]): NsNode[] {
  const roots: NsNode[] = [];
  const byPath = new Map<string, NsNode>();

  const ensure = (levels: string[], exists: boolean): NsNode => {
    const dotted = levels.join(".");
    const found = byPath.get(dotted);
    if (found) {
      if (exists) found.exists = true;
      return found;
    }
    const node: NsNode = { name: levels[levels.length - 1], dotted, levels, exists, children: [] };
    byPath.set(dotted, node);
    if (levels.length === 1) {
      roots.push(node);
    } else {
      const parent = ensure(levels.slice(0, -1), false);
      parent.children.push(node);
    }
    return node;
  };

  for (const { levels } of list) {
    if (levels.length > 0) ensure(levels, true);
  }

  const sortRec = (nodes: NsNode[]) => {
    nodes.sort((a, b) => a.name.localeCompare(b.name));
    nodes.forEach((n) => sortRec(n.children));
  };
  sortRec(roots);
  return roots;
}

/** Does `dotted` or any descendant path contain `q` (case-insensitive)? */
export function nodeMatchesFilter(node: NsNode, q: string): boolean {
  if (!q) return true;
  const lc = q.toLowerCase();
  const self = node.dotted.toLowerCase().includes(lc);
  return self || node.children.some((c) => nodeMatchesFilter(c, lc));
}
```

### Step 4: Run to verify it passes

Run: `cd frontend && npx vitest run src/routes/explore/catalog/__tests__/namespaceTree.test.ts`
Expected: PASS (3 tests).

### Step 5: Commit

```bash
git add frontend/src/routes/explore/catalog/namespaceTree.ts frontend/src/routes/explore/catalog/__tests__/namespaceTree.test.ts
git commit -m "feat(catalog): namespace tree-builder for the Browse redesign"
```

---

## Task 2: `NamespaceTree` component (filter + recursive lazy nodes)

**Files:**
- Create: `frontend/src/routes/explore/catalog/NamespaceTree.tsx`
- Test: covered by the `CatalogBrowse` test in Task 3 (the tree renders only inside the browse page).

**Component contract:**
```tsx
export function NamespaceTree({
  ws, wid, tree, selectedNs, selectedTable, onSelectTable, onDeleteNamespace, onNewNamespace,
}: {
  ws: string;
  wid: string;
  tree: NsNode[];
  selectedNs: string | null;
  selectedTable: string | null;
  onSelectTable: (ns: string, table: string) => void;
  onDeleteNamespace: (dotted: string) => void;
  onNewNamespace: () => void;
}): JSX.Element
```

### Step 1: Implement `NamespaceTree.tsx`

Requirements (match the existing `CatalogBrowse` left-pane styling — bordered container, uppercase header, `Database`/`Table2`/`Eye`/`Trash2`/`Plus` lucide icons, `font-mono`, active = `bg-accent text-accent-foreground`):

- Local state: `filter: string`; `expanded: Set<string>` (dotted paths). A `<input>` (or shadcn `Input`) filter box in the header next to the ＋ (`onNewNamespace`) button.
- Header strip: "NAMESPACES" + ＋ button (`aria-label="New namespace"`, calls `onNewNamespace`).
- When `filter` is non-empty: compute the visible set with `nodeMatchesFilter`; **auto-expand** every node on a path to a match (so matches are revealed) — i.e. a node is effectively-expanded if it's in `expanded` OR (`filter` non-empty AND it has a descendant match).
- Render roots via a recursive `TreeNode`. Each `TreeNode` (own sub-component so it can call hooks):
  - Row: a chevron/disclosure (▸/▾) for expand toggle, `Database` icon, the `name` (mono); dim if `!node.exists` (synthesized intermediate). A hover `Trash2` → `onDeleteNamespace(node.dotted)` (only when `node.exists`).
  - `const open = isExpanded(node);`
  - `const contents = useNamespaceContents(ws, wid, open ? node.dotted : null);` (collapsed → `null` → query disabled).
  - When `open`: render child namespace nodes (recursive), then the namespace's tables/views as leaves once `contents.data` is present:
    - Filter the leaf names by the same `filter` (case-insensitive substring).
    - Combine `tables` (icon `Table2`, clickable → `onSelectTable(node.dotted, name)`, active highlight when `selectedNs===node.dotted && selectedTable===name`) and `views` (icon `Eye`, **not** clickable).
    - Apply the `MAX_VISIBLE = 200` cap to the *filtered* combined leaf list: render the first 200 and, if more remain, a muted line `Showing {shown} of {total} — refine search`.
  - Indentation by depth (e.g. `style={{ paddingLeft: depth * 12 + 8 }}` or nested padding).
- Empty tree → caller handles (CatalogBrowse shows "No namespaces").

Export `MAX_VISIBLE` for the test, or hard-code 200 and have the test build 201 items.

### Step 2: Typecheck

Run: `cd frontend && npm run typecheck`
Expected: clean. (Behavioural assertions are exercised via Task 3's CatalogBrowse test.)

### Step 3: Commit

```bash
git add frontend/src/routes/explore/catalog/NamespaceTree.tsx
git commit -m "feat(catalog): NamespaceTree (filter + recursive lazy-loading nodes)"
```

---

## Task 3: Rewrite `CatalogBrowse` orchestrator (URL sync + breadcrumb) + tests

**Files:**
- Modify: `frontend/src/routes/explore/catalog/CatalogBrowse.tsx`
- Modify: `frontend/src/routes/explore/catalog/__tests__/CatalogBrowse.test.tsx`

### Step 1: Rewrite the test first (drives the new structure)

Replace `CatalogBrowse.test.tsx`. Keep the existing `TableDetailView` assertions (they still apply once a table is opened) and add tree/URL ones. Mock `@/api/catalog` as today (`useCatalogNamespaces`, `useNamespaceContents`, `useTableDetail`, `useCreateNamespace`, `useDeleteNamespace`), and render inside a `MemoryRouter` (since the component now uses `useSearchParams`).

Required test cases (full code — the implementer writes these exactly):
```tsx
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";

const namespacesMock = vi.fn();
const contentsMock = vi.fn();
const tableDetailMock = vi.fn();
const createNamespaceMock = vi.fn();
const deleteNamespaceMock = vi.fn();

vi.mock("@/api/catalog", () => ({
  nsDotted: (levels: string[]) => levels.join("."),
  useCatalogNamespaces: (ws: string, wid: string) => namespacesMock(ws, wid),
  useNamespaceContents: (_ws: string, wid: string, ns: string | null) => contentsMock(wid, ns),
  useTableDetail: (_ws: string, wid: string, ns: string | null, table: string | null) =>
    tableDetailMock(wid, ns, table),
  useCreateNamespace: () => ({ mutateAsync: createNamespaceMock, isPending: false }),
  useDeleteNamespace: () => ({ mutate: deleteNamespaceMock, isPending: false }),
}));

import CatalogBrowse from "../CatalogBrowse";

function renderAt(initialEntries: string[] = ["/"]) {
  const qc = new QueryClient();
  return render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={initialEntries}>
        <CatalogBrowse ws="eng" wid="wh-1" />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  namespacesMock.mockReset();
  contentsMock.mockReset();
  tableDetailMock.mockReset();
  createNamespaceMock.mockReset();
  deleteNamespaceMock.mockReset();
  createNamespaceMock.mockResolvedValue(undefined);
  namespacesMock.mockReturnValue({
    data: [{ levels: ["sale"] }, { levels: ["sale", "2024"] }, { levels: ["marketing"] }],
    isLoading: false,
    isError: false,
  });
  // Contents only resolve for the expanded namespace; default empty.
  contentsMock.mockImplementation((_wid: string, ns: string | null) =>
    ns === "sale"
      ? { data: { tables: ["orders", "customers"], views: ["v_orders"] }, isLoading: false, isError: false }
      : { data: { tables: [], views: [] }, isLoading: false, isError: false },
  );
  tableDetailMock.mockReturnValue({
    data: {
      name: "orders",
      location: "s3://wh/orders",
      current_schema_id: 1,
      columns: [{ name: "id", type: "long", required: true }],
      current_snapshot_id: 42,
      snapshot_count: 1,
      properties: { "write.format.default": "parquet" },
      format_version: 2,
      last_updated_ms: 1750000000000,
      partition_fields: [],
      sort_fields: [],
      snapshots: [],
    },
    isLoading: false,
    isError: false,
  });
});

describe("Catalog browse tree", () => {
  it("renders top-level namespaces as a tree", () => {
    renderAt();
    expect(screen.getByText("marketing")).toBeInTheDocument();
    expect(screen.getByText("sale")).toBeInTheDocument();
    // Child not visible until 'sale' is expanded.
    expect(screen.queryByText("2024")).not.toBeInTheDocument();
  });

  it("expands a namespace, lazy-loads its tables/views, and drills into a table", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: /expand sale/i }));
    expect(screen.getByText("2024")).toBeInTheDocument(); // child namespace
    expect(screen.getByText("orders")).toBeInTheDocument(); // table leaf
    expect(screen.getByText("v_orders")).toBeInTheDocument(); // view leaf
    expect(contentsMock).toHaveBeenCalledWith("wh-1", "sale");

    await user.click(screen.getByText("orders"));
    expect(tableDetailMock).toHaveBeenCalledWith("wh-1", "sale", "orders");
    expect(screen.getByText("write.format.default")).toBeInTheDocument(); // detail shown
  });

  it("filters the tree by the search box", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.type(screen.getByPlaceholderText(/filter/i), "marketing");
    expect(screen.getByText("marketing")).toBeInTheDocument();
    expect(screen.queryByText("sale")).not.toBeInTheDocument();
  });

  it("restores selection from the URL (deep-link)", () => {
    renderAt(["/?ns=sale&table=orders"]);
    // Detail rendered without any clicks.
    expect(tableDetailMock).toHaveBeenCalledWith("wh-1", "sale", "orders");
    expect(screen.getByText("write.format.default")).toBeInTheDocument();
  });

  it("creates a root namespace from the New namespace dialog", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: "New namespace" }));
    await user.type(screen.getByLabelText("Name"), "ops");
    await user.click(screen.getByRole("button", { name: "Create" }));
    expect(createNamespaceMock).toHaveBeenCalledWith({ parent: null, name: "ops" });
  });

  it("deletes a namespace after confirmation", async () => {
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: "Delete sale" }));
    await user.type(screen.getByPlaceholderText("delete"), "delete");
    await user.click(screen.getByRole("button", { name: /^Delete$/ }));
    expect(deleteNamespaceMock).toHaveBeenCalledWith("sale", expect.anything());
  });

  it("shows a 'Showing X of Y' note for a large namespace", async () => {
    contentsMock.mockImplementation((_wid: string, ns: string | null) =>
      ns === "sale"
        ? {
            data: {
              tables: Array.from({ length: 250 }, (_, i) => `t${i}`),
              views: [],
            },
            isLoading: false,
            isError: false,
          }
        : { data: { tables: [], views: [] }, isLoading: false, isError: false },
    );
    const user = userEvent.setup();
    renderAt();
    await user.click(screen.getByRole("button", { name: /expand sale/i }));
    expect(screen.getByText(/Showing 200 of 250/i)).toBeInTheDocument();
  });
});
```

> Note: the disclosure control must have an accessible name like **"Expand sale"** / **"Collapse sale"** (e.g. `aria-label={\`${open ? "Collapse" : "Expand"} ${node.dotted}\`}`), and the per-node delete button keeps `aria-label={\`Delete ${node.dotted}\`}`, so the tests above resolve unambiguously.

### Step 2: Run the test to verify it fails

Run: `cd frontend && npx vitest run src/routes/explore/catalog/__tests__/CatalogBrowse.test.tsx`
Expected: FAIL (new structure not built yet).

### Step 3: Rewrite `CatalogBrowse.tsx`

Replace the orchestrator + remove `NamespaceContentsView` (superseded by tree leaves). **Keep** `TableDetailView`, `snapshotSummary`, `CopyButton`, `NewNamespaceDialog`, and the imports they need.

Orchestrator requirements:
```tsx
import { useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import { buildNamespaceTree } from "./namespaceTree";
import { NamespaceTree } from "./NamespaceTree";
import {
  Breadcrumb, BreadcrumbList, BreadcrumbItem, BreadcrumbLink, BreadcrumbPage, BreadcrumbSeparator,
} from "@/components/ui/breadcrumb";
```
- Read selection from URL: `const [sp, setSp] = useSearchParams(); const ns = sp.get("ns"); const table = sp.get("table");`
- `setSelection(ns, table)` updates the search params (drop `table` when null; drop both when ns null), preserving other params: build a new `URLSearchParams` from `sp`, set/delete `ns`/`table`, `setSp(next, { replace: false })`.
- `const tree = useMemo(() => buildNamespaceTree(namespaces.data ?? []), [namespaces.data]);`
- `const detail = useTableDetail(ws, wid, ns, table);`
- Layout: keep the `grid grid-cols-[16rem_1fr] gap-4` (or widen left to `18rem`). Left = `<NamespaceTree ... onSelectTable={(n,t)=>setSelection(n,t)} onDeleteNamespace={(d)=>{ setDeleteTarget(d); }} onNewNamespace={()=>setAddOpen(true)} />`. Right:
  - loading/error/empty for namespaces (reuse current messages: "Failed to load namespaces.", "No namespaces.").
  - else if `table` → a `Breadcrumb` (`sale › orders`, segments: each namespace level is a `BreadcrumbLink` calling `setSelection(<that dotted>, null)`, the table is `BreadcrumbPage`) followed by `<TableDetailView ns={ns} table={table} detail={detail} onBack={() => setSelection(ns, null)} />`.
  - else → a dashed placeholder "Select a table to view its detail." (and, if `ns` set, you may show the namespace name in a breadcrumb).
- Keep `NewNamespaceDialog` (keyed by open state) and the `ConfirmDeleteDialog` for delete, wired exactly as today (`deleteMutation.mutate(deleteTarget, { onSuccess/onError })`, clearing selection if the deleted ns was selected).

> `TableDetailView` already renders its own internal `ns / table` mini-header; you may keep it or rely on the new breadcrumb — either is fine, but ensure the new breadcrumb is present (tests don't assert the breadcrumb text specifically, but the spec calls for it). Do not duplicate confusingly; prefer the new `Breadcrumb` above and keep `TableDetailView`'s `onBack`.

### Step 4: Run the test to verify it passes

Run: `cd frontend && npx vitest run src/routes/explore/catalog/__tests__/CatalogBrowse.test.tsx`
Expected: PASS.

### Step 5: Typecheck + lint + full FE test + build

Run (from `frontend/`):
```bash
npm run typecheck
npx eslint src/routes/explore/catalog
npx vitest run
npm run build
```
Expected: all clean.

### Step 6: Commit

```bash
git add frontend/src/routes/explore/catalog/CatalogBrowse.tsx frontend/src/routes/explore/catalog/__tests__/CatalogBrowse.test.tsx
git commit -m "feat(catalog): scalable namespace tree + breadcrumb Browse, URL-synced"
```

---

## Final verification (before merge)

1. **Full FE CI** (GitHub out of quota — run locally): `npm run lint` · `npm run typecheck` · `npx vitest run` · `npm run build` — all clean.
2. **Live smoke** (FE hot-reloads; backend already serves the data): log in (`admin@egret.local` / `Admin12345!`), open warehouse "123" → Browse: expand `sale`, see `orders`/`customers`/`v_orders`, click `orders` → detail + breadcrumb; the URL shows `?ns=sale&table=orders`; refresh restores it; type in the filter to narrow the tree. Confirm no console errors. (Seed extra namespaces/tables via the iceberg REST path from `[[egret-seed-catalog-table]]` if you want to eyeball scale.)
3. **Admin squash-merge** with identity `hoangquocvietbk2.13@gmail.com`.

---

## Self-review

- **Spec coverage:** tree nav (§1) → Tasks 1–3; URL sync (§2) → Task 3; scale filter + "Showing X of Y" (§3) → Task 2 + Task 3 test; component structure (§4) → Tasks 1–3; testing (§Testing) → Task 1 + Task 3 tests; out-of-scope (global search, view detail, virtualization) → not built. ✅
- **Placeholders:** none — tree-builder + test code complete; component contracts + required behaviours + exact test assertions given; JSX styling explicitly defers to the existing `CatalogBrowse` patterns the implementer can read.
- **Type consistency:** `NsNode`/`buildNamespaceTree`/`nodeMatchesFilter` from Task 1 are consumed by Task 2/3; `useNamespaceContents(... null)` disabling matches the real hook; `MAX_VISIBLE = 200` used by both the component and the test.
