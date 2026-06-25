# SQL Lab — History / Saved / Toolbar UX (Design)

**Status:** Approved
**Date:** 2026-06-25
**Scope:** Frontend only (React/TS). No backend / API / migration changes — the saved-query
`PUT` endpoint already exists (`get(load).put(update).delete(remove)` in
`crates/egret-bi/src/routes.rs`).

## Problem

SQL Lab's bottom panel and toolbar are under-built compared to the data the API already returns:

1. **History tab** is a flat list of `[status] sql` buttons. It ignores `submitted_at`,
   `row_count`, and `database_id` that `HistoryItem` already carries. Status is shown as the
   raw text `[finished]`/`[failed]`. The only action is "click to load SQL"; there is no copy,
   no re-run, no empty state.
2. **Saved tab** shows only `name` + a plain red `delete` text link. It ignores `description`,
   `updated_at`, and `database_id` that `SavedQuery` carries. Worse, `delete` fires
   **immediately with no confirmation** (easy to lose a saved query). There is no rename/edit
   and no empty state.
3. **Limit selector** is `w-28`, too narrow — "1,000 rows" is clipped by the chevron to
   "1,000 row⋮". There is no way to set a custom limit outside the presets.
4. **Toolbar / Results**: a failed query's error string is printed inline on the toolbar
   (`status.data.error`), so a long message breaks the toolbar layout. After a run there is no
   sense of how long it took.

## Goals

Make History and Saved read like a real query workspace (Superset-style rich rows), fix the
clipped limit selector and allow a custom limit, and move query errors off the toolbar into the
results area with a small post-run status line — all frontend-only.

## Non-Goals

- **No backend changes.** `update` already accepts `{ name, sql, description, database_id }`.
- No history **search / filter / pagination** (keep a simple capped list).
- No **tags / favorites / multi-select** on saved queries.
- No backend-measured timing. Elapsed time is **measured client-side** (the status API returns
  no timestamps), shown as an approximation.
- Saved-query delete uses a **lightweight confirm** (Cancel / Delete) — NOT the type-the-word
  `ConfirmDeleteDialog` (too heavy for a low-stakes saved SQL string).

## Current State (anchors)

- `frontend/src/components/sqllab/SqlTab.tsx` — `HistoryTab` and `SavedTab` are **inline
  functions at the bottom** (lines ~139–156). Toolbar at lines 61–75 holds Run/Cancel/Format,
  the `w-28` limit `Select` (67–70), Save / Save as dataset, and the inline error span (74).
- `frontend/src/api/sqllab.ts` — `HistoryItem { id, database_id, status, sql, row_count,
  submitted_at }`; `SavedQuery { id, database_id, name, sql, description, created_by,
  updated_at }`. Hooks: `useHistory`, `useSavedQueries`, `useSaveQuery`, `useDeleteSavedQuery`.
  **Missing:** `useUpdateSavedQuery` (PUT).
- `frontend/src/components/sqllab/ResultsGrid.tsx` — footer already shows `{row_count} rows`
  + Download CSV; a `truncated` banner at the top.
- `frontend/src/components/sqllab/SaveQueryDialog.tsx` — create-only (empty initial name/desc,
  title "Save query").
- Available primitives: `ui/badge`, `ui/alert-dialog`, `ui/tooltip`, `ui/dropdown-menu`,
  `ui/dialog`. `dayjs` is a dependency (used in `chart/format.ts`) but the `relativeTime`
  plugin is not yet wired anywhere.

## Design

### File structure

**New:**
- `frontend/src/lib/relativeTime.ts` — wires `dayjs.extend(relativeTime)` once; exports
  `timeAgo(iso: string): string` (e.g. "2 minutes ago") and `fullTimestamp(iso: string):
  string` (absolute, for tooltips).
- `frontend/src/components/sqllab/QueryStatusBadge.tsx` — `<QueryStatusBadge status>` mapping
  `QueryStatus` → coloured `Badge`: finished=green, failed=red(destructive),
  running/queued=amber/blue, cancelled=muted/grey.
- `frontend/src/components/sqllab/HistoryTab.tsx` — extracted + enriched (see below).
- `frontend/src/components/sqllab/SavedTab.tsx` — extracted + enriched (see below).

**Modified:**
- `frontend/src/api/sqllab.ts` — add `useUpdateSavedQuery(ws)` (PUT
  `/ws/{ws}/sqllab/queries/{id}`, body `{ name, sql, description, database_id }`, invalidates
  the `saved` key).
- `frontend/src/components/sqllab/SaveQueryDialog.tsx` — optional edit mode: `initialName`,
  `initialDescription`, and `mode?: "create" | "edit"` (title + button label swap). Resets to
  the initial values whenever it opens.
- `frontend/src/components/sqllab/SqlTab.tsx` — remove the inline tab functions; widen + extend
  the limit control; track client-side run timing; move the error off the toolbar; pass
  `databases`, `onLoad`, `onRun` to `HistoryTab`.
- `frontend/src/components/sqllab/ResultsGrid.tsx` — accept an optional `error: string | null`
  (render a red block instead of the grid when set) and an optional `statusLine` node in the
  footer (elapsed + rows).

### 1. relativeTime helper

`relativeTime.ts` calls `dayjs.extend(relativeTime)` at module load (idempotent). `timeAgo`
returns `dayjs(iso).fromNow()`; `fullTimestamp` returns `dayjs(iso).format("YYYY-MM-DD
HH:mm:ss")`. Pure functions → directly unit-testable.

### 2. QueryStatusBadge

A tiny presentational component. One `Badge` with a status-specific class and the capitalised
status text (and, for running/queued, the existing animated feel is not required — colour is
enough). No data fetching.

### 3. HistoryTab (rich list rows)

Props: `{ ws, databases, onLoad(sql), onRun(sql, dbId) }`. Uses `useHistory(ws)`.

Each item is a row group:
- **Line 1 (meta):** `<QueryStatusBadge>` · DB name (resolved from `databases` by
  `database_id`, fallback to a short id) · `timeAgo(submitted_at)` (tooltip =
  `fullTimestamp`) · `{row_count} rows` when non-null.
- **Line 2 (sql):** single-line, `font-mono`, truncated, `title` = full SQL.
- **Row click** → `onLoad(sql)` (load into the editor, current behaviour).
- **Hover actions** (right-aligned, appear on row hover/focus): **Copy** (writes `sql` to
  `navigator.clipboard`, toasts "Copied"), **Re-run** (`onRun(sql, database_id)` → sets the
  tab's DB + SQL and runs immediately).
- **Empty state:** centered muted "No queries run yet."

### 4. SavedTab (rich list rows)

Props: `{ ws, databases, onOpen(sql, dbId) }`. Uses `useSavedQueries`, `useUpdateSavedQuery`,
`useDeleteSavedQuery`. Owns its own edit + delete dialog state (self-contained).

Each item is a row group:
- **Line 1:** name (semibold) · DB name · "updated " + `timeAgo(updated_at)` (tooltip =
  `fullTimestamp`).
- **Line 2:** `description` (muted) when present, else a muted single-line SQL preview.
- **Row click** → `onOpen(sql, database_id)`.
- **Hover actions:** **Edit** (pencil) → opens `SaveQueryDialog` in `edit` mode prefilled with
  the row's name/description; on submit calls `useUpdateSavedQuery` with `{ name, sql,
  description, database_id }` (sql + database_id preserved from the saved row). **Delete**
  (trash) → opens a lightweight `AlertDialog` (title "Delete saved query?", Cancel / Delete);
  Delete calls `useDeleteSavedQuery`.
- **Empty state:** centered muted "No saved queries."

### 5. Limit selector

- Widen the trigger to `w-36` and prefix a small "Limit" affordance (label or leading text) so
  the value isn't clipped.
- Preset items stay `[10, 100, 1000, 10000, 100000]`, formatted `n.toLocaleString() + " rows"`.
- Add a trailing **"Custom…"** item. Selecting it switches the control into an **inline number
  `Input`** (replacing the trigger) focused and pre-filled with the current limit. **Enter** or
  **blur** commits a clamped positive integer via `patch({ limit })`; **Esc** cancels back to
  the dropdown. If `tab.limit` is not one of the presets, the dropdown trigger still shows
  `"{tab.limit} rows"` (custom values round-trip).

### 6. Toolbar / Results polish

- **Remove** the inline `status.data.error` span (and the `running…` text stays, it's short).
- On a successful submit, **auto-switch the bottom tab to "Results"** (controlled `Tabs value`
  in `SqlTab`).
- When `status.status === "failed"`, the **Results tab renders the error** in a red block
  (pass `error` into `ResultsGrid`, which shows the block in place of the grid).
- **Client-side timing:** when a submit resolves and sets `queryId`, record `startedAt`
  (`performance.now()`). When status transitions to `finished`/`failed`, compute
  `elapsedMs = now - startedAt` and freeze it. Show a footer status line in `ResultsGrid`:
  `"Done in {1.2s} · {N} rows"` (or `"Failed in {…}"`). Elapsed is approximate (round-trip +
  poll interval) and labelled as such only implicitly via wording.

### Data flow

```
SqlTab
 ├─ databases (useDatabases)            ──▶ HistoryTab (name lookup), SavedTab (name lookup)
 ├─ onLoad(sql)=patch({sql})            ──▶ HistoryTab row click
 ├─ onRun(sql,dbId)=patch+run(sql)      ──▶ HistoryTab Re-run
 ├─ onOpen(sql,dbId) (existing prop)    ──▶ SavedTab row click
 ├─ tabValue/setTabValue (Results auto) ──▶ Tabs
 └─ ResultsGrid(error, statusLine)
SavedTab owns: edit dialog (SaveQueryDialog edit-mode) + delete AlertDialog, via
 useUpdateSavedQuery / useDeleteSavedQuery.
```

`onRun(sql, dbId)` in `SqlTab`: `patch({ dbId, sql })` then `run(sql)` (run already accepts an
override and is guarded while running). Because `patch` is async-to-render, `run(sql)` is called
with the explicit override so it doesn't depend on the not-yet-applied state.

### Error handling

- Copy failure → toast error (clipboard can reject).
- Update/delete saved → existing mutation error path (toast); list invalidation on success.
- Custom limit: ignore non-numeric / ≤0 input (revert to previous), clamp to a sane max
  (reuse the largest preset `100000` as the ceiling) to avoid pathological values.
- DB name lookup tolerates a missing database (deleted/ungranted) → show a truncated id.

## Testing

Vitest (jsdom), following the existing `sqllab/__tests__` patterns (mock `@/api/sqllab`):

- `relativeTime`: `timeAgo` / `fullTimestamp` for known offsets (inject a fixed "now" via the
  ISO inputs).
- `QueryStatusBadge`: renders the right label/colour class per status.
- `HistoryTab`: renders badge + DB name + rows + time; row click calls `onLoad`; Re-run calls
  `onRun(sql, dbId)`; Copy writes to a mocked `navigator.clipboard`; empty state.
- `SavedTab`: renders name + description + DB + updated time; row click calls `onOpen`; Edit
  opens the dialog and submitting calls the update mutation with the right body; Delete shows
  the confirm and only deletes after confirming; empty state.
- Limit: choosing "Custom…" reveals the input; entering a value patches `limit`; a custom
  `tab.limit` shows in the trigger.
- `SqlTab`: a `failed` status renders the error inside the Results panel (not on the toolbar);
  a successful submit selects the Results tab.

Live Playwright verification at the end (stack already up): History rows show badge/DB/time/rows
with Copy + Re-run working; Saved rows show description + Edit/Delete-with-confirm; limit shows
full text + a custom value runs; a failing query shows the error in Results, not the toolbar;
0 new console errors.

## YAGNI cuts

History search/filter/pagination; saved tags/favorites/multi-select; any backend change;
backend-measured timing; type-to-confirm on saved delete.
