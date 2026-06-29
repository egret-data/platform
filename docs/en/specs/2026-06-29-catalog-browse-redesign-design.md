# Catalog — Browse tab redesign (scalable namespace tree) — Design

**Date:** 2026-06-29 · **Status:** Design — approved · **Branch:** `feat/catalog-browse-redesign` (off main)
**Program:** Catalog-console-parity, follow-up improvement. Reference: the lakekeeper console warehouse browser (`data-portal/console`, `WarehousesNavigationTree` + breadcrumb).

## Goal
Replace the current Catalog **Browse** tab — a flat 16rem list of every namespace + a flat per-namespace table/view list, with no search — with a **scalable namespace navigation tree + breadcrumb** that stays usable when a warehouse has many namespaces and tables.

## Background (current state)
- `CatalogBrowse.tsx` renders a two-pane grid: left = `useCatalogNamespaces` rendered as a **flat dotted list** (e.g. `sale`, `sale.2024`, `marketing`), right = either a namespace's tables/views list (`NamespaceContentsView`) or `TableDetailView`. No search, no tree, no virtualization; selection is component state (not URL).
- Backend already returns the data exhaustively: `GET …/warehouses/{wid}/namespaces` BFS-walks the whole tree and returns **all** namespaces flat (`{ levels: string[] }[]`, paginating each level internally); `GET …/namespaces/{ns}/tables` returns `{ tables: string[], views: string[] }` for one namespace. So the scale work is **frontend**.
- Available primitives: shadcn `Breadcrumb` (`Breadcrumb/BreadcrumbList/BreadcrumbItem/BreadcrumbLink/BreadcrumbPage/BreadcrumbSeparator`), `useSearchParams` (react-router, already used by the Storage browser), `DetailSection`/`Card`. **No** virtualization library is installed.

## Decisions

### 1. Navigation model — namespace tree + breadcrumb (lakekeeper-console style)
- **Left pane = `NamespaceTree`**: the flat `useCatalogNamespaces` list is folded into a **nested tree** (by `levels`), client-side. Each namespace node expands/collapses; on first expand its tables/views are **lazy-loaded** via `useNamespaceContents` (TanStack Query, cached) and rendered as leaf rows (table → clickable, view → shown, non-clickable for now — no view-detail page yet). A **filter input** at the top of the tree.
- **Right pane = content**: a `Breadcrumb` (`warehouse name › namespace levels › table`) above either `TableDetailView` (unchanged — the just-redesigned card view) or, when a namespace is selected but no table, a short overview ("Select a table…", plus its table/view counts).
- Keep the existing **New namespace** dialog (parent picker) and **Delete namespace** confirm; surface them as a header ＋ button and a per-node hover action.

### 2. Selection state synced to the URL
Selected namespace + table live in URL search params (`?ns=<dotted>&table=<name>`) via `useSearchParams` (mirroring the Storage browser). On load, the tree **auto-expands the ancestors** of `?ns` and opens `?table`'s detail — so refresh, deep-link, and the browser Back button all work. (Fixes the long-standing "browse URL doesn't change" gap for this view.)

### 3. Scale handling (no new dependency)
- Collapsed tree → only expanded nodes render children, keeping the DOM small.
- The filter box filters the namespace tree by dotted-path substring (auto-expanding matching branches); within an **expanded** namespace it also filters that namespace's loaded tables/views by the same query.
- For a namespace whose table+view count exceeds a threshold (**200**), render the filtered subset and show a clear **"Showing X of Y — refine search"** line (never a silent truncation). True row **virtualization is deferred** (would need a new dep like `@tanstack/react-virtual`); noted as a future step if real catalogs exceed the threshold routinely.

### 4. Component structure
- `CatalogBrowse.tsx` (orchestrator): URL-synced selection; renders `NamespaceTree` + the content pane; owns the New-namespace / Delete-namespace dialogs.
- `NamespaceTree.tsx`: flat→nested transform, filter input, recursive `TreeNode` (expand/collapse, lazy contents, leaves, active highlight, hover delete).
- `CatalogBreadcrumb` (small, inline or local): renders the path with clickable segments.
- Reuse `TableDetailView` and `DetailSection`/`Card` unchanged.

## Testing (Vitest, mock `@/api/catalog`)
- Folds a flat namespace list into a nested tree (parent `sale` with child `sale.2024`).
- Expanding a namespace lazy-loads + renders its tables and views; `useNamespaceContents` is called with the expanded namespace.
- The filter input narrows the visible tree (and an expanded namespace's contents).
- Selecting a table renders `TableDetailView`, updates the breadcrumb, and sets `?ns`/`?table`.
- Deep-link: rendering with `?ns=sale&table=orders` restores the expansion + opens the detail.
- A namespace over the 200 threshold shows the "Showing X of Y" line.
- New-namespace create + Delete-namespace (with 409 surfaced) still work.

## Risks
- **Lazy-load fan-out:** expanding many namespaces issues many `useNamespaceContents` queries; TanStack caches them and only expanded nodes load, so it's bounded by user action.
- **Filtering tables requires them loaded:** filter only matches a namespace's tables/views once that namespace is expanded; cross-namespace table search is explicitly out of scope (needs a backend search endpoint).
- **Large single namespace:** the 200-item note avoids a DOM blow-up without a new dependency; true virtualization is the documented next step.

## Out of scope (later)
Global cross-namespace table search (needs a backend search/filter endpoint); a VIEW detail page; true row virtualization; server-side pagination/search; generic-table support.
