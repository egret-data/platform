# AX-5 — Bulk Ops + Safety Polish — Design

**Status:** Approved
**Date:** 2026-07-13
**Initiative:** Access-Control UX uplift (increment 5 of 5 — the final one) — see
`docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`

## Goal

Let an admin act on many rows at once, safely: multi-select on the Users and Workspace
Members tables with bulk actions (disable/enable users, assign a role to many, remove
members, change many members' role), each reversible action offering an **Undo**.
Finish the polish: give the "active" state badge a semantic colour distinct from the
brand accent, and replace ad-hoc "Loading…" text with consistent skeletons across the
access lists. **Frontend-only** — bulk loops the existing single-item endpoints; no
backend change.

## Motivation / current state

- The access tables (`FilterableTable`, AX-2) have no selection — every action is
  one-row-at-a-time. Managing many users/members is tedious.
- `useBulkSelect<T extends {id}>(rows)` already exists
  (`frontend/src/hooks/useBulkSelect.ts`) → `{ bulkMode, setBulkMode, selected,
  selectedIds, toggle, toggleAll, clear, allSelected, someSelected }`, and `ListSkeleton`
  (`frontend/src/components/explore/ListSkeleton.tsx`) already exists — both from the BI
  lists. `FilterableTable` just needs an optional selection hook-in.
- The single-item endpoints all exist: `updateUser(id, {status})`, `assignUserRole(id,
  roleId)`, `unassignUserRole(id, roleId)` (`@/api/access`); `useAddMember` (upsert /
  role change) + `useRemoveMember` (`@/api/members`, AX-1). Bulk = loop these.
- The Users "active" badge uses the primary/teal token — the same colour as CTAs — so
  state reads like an action (flagged in the AX-1 review). The `--success` token exists
  (added in AX-1) but isn't used here.

## Design

### 1. `FilterableTable` — optional selection

Add one optional prop:

```ts
selection?: {
  selected: Set<string>;
  onToggle: (id: string) => void;
  allSelected: boolean;
  onToggleAll: () => void;
};
```

When present, `FilterableTable` prepends a leading checkbox column: the header cell is a
select-all `Checkbox` (`checked={allSelected}`, `onCheckedChange={onToggleAll}`), and each
row's leading cell is a per-row `Checkbox` (`checked={selected.has(getId(row))}`,
`onCheckedChange={() => onToggle(getId(row))}`). When absent, the table renders exactly as
today. Selection is driven by the page's `useBulkSelect`; the header select-all toggles
the **currently visible** (searched/faceted) rows only (that is what `useBulkSelect`'s
`toggleAll` already does, given the visible `rows`). Keep `FilterableTable` otherwise
unchanged (still single-responsibility: display + search + sort + optional selection column).

### 2. Bulk on Users + Workspace Members

Both pages already render `FilterableTable`. On each:

- A **"Bulk select"** toggle button (mirroring the Notebooks list's bulk toggle) flips
  `bulkMode`; entering bulk mode passes `selection` to `FilterableTable` (checkbox column
  appears) and hides the per-row `⋯` menu's destructive-only items where they'd be
  redundant (keep the menu; bulk is additive).
- A **bulk-action bar** appears above the table when `someSelected`: "N selected" + the
  actions + a "Clear" button.

| Page | Bulk actions |
|---|---|
| **Users** (`.../access/UsersPage.tsx`) | **Disable** / **Enable** the selected users (`updateUser(id, {status})`); **Assign role…** — pick a role, `assignUserRole(id, roleId)` for each (admin-only, like the single action) |
| **Workspace Members** (`.../explore/settings/Members.tsx`) | **Remove** the selected members (`useRemoveMember`); **Set role…** — pick a workspace role, add each (`useAddMember`, upsert) |

Each bulk op runs the single-item mutation per selected id (sequentially or with bounded
concurrency), then invalidates the list query and clears the selection. Respect existing
guards: the Members bulk **Remove** must not drop the workspace's last admin — the backend
already returns a `Conflict` per-item, so a bulk remove that would orphan the workspace
surfaces that item's error (collected into the partial-failure summary, below) rather than
succeeding.

### 3. Undo (sonner toast + inverse)

Before a **reversible** bulk op, snapshot the affected rows' prior state. After it
completes, show a toast: e.g. *"Disabled 4 users. [Undo]"*. Clicking **Undo** runs the
inverse per affected id: enable→disable / disable→enable (`updateUser`); assign→
`unassignUserRole`; remove-member→re-add with the snapshotted prior role (`useAddMember`);
set-role→re-set to the snapshotted prior role. The toast auto-dismisses (~6s). Undo is
offered **only** for reversible ops.

**Partial failure:** a bulk op runs each item independently and collects successes +
failures. If any fail, the result toast is *"Disabled 3 of 4 users; 1 failed."* (error
variant), and Undo (when offered) inverts **only the items that actually succeeded**
(snapshot keyed by success). Never silently swallow a failure.

### 4. Success-token state badge

Change the Users "active" `Badge` to use the semantic `--success` token (e.g. a
`bg-success text-success-foreground` variant or an inline `style` using `var(--success)`),
so an active row reads as a *state*, not a CTA. "disabled" stays the muted/secondary look.
This is a small, self-contained colour change; do not touch other badges.

### 5. Skeleton + empty-state consistency

Replace the ad-hoc `"Loading…"` text on Users / Groups / Roles / Members with the shared
`ListSkeleton` (sized to each table's column count) while the list query is loading. Keep
the existing empty ("No users." etc.) and error states; only the loading state changes, so
the access lists load consistently.

## Testing (Vitest)

- **`FilterableTable` selection:** with a `selection` prop, a leading checkbox column
  renders; toggling a row calls `onToggle(id)`; the header select-all calls `onToggleAll`;
  without the prop, no checkbox column appears (regression).
- **Users bulk:** entering bulk mode + selecting two users + clicking "Disable" calls
  `updateUser` for both selected ids with `status: "disabled"`; the Undo toast's action
  calls `updateUser` with `status: "active"` for those ids; "Assign role…" calls
  `assignUserRole` for each.
- **Members bulk:** selecting members + "Remove" calls the remove mutation per id; Undo
  re-adds with the prior role.
- **Partial failure:** when one item's mutation rejects, the summary reflects "N of M"
  and Undo inverts only the succeeded ids.
- **Badge:** the active badge uses the success token (assert the class/style).
- **Skeleton:** while loading, `ListSkeleton` (`data-testid="list-skeleton"`) renders
  instead of "Loading…".

## Out of scope

- Bulk **delete** of Roles or Groups — destructive with seeded/last-admin guards; the
  roadmap scoped bulk to users/members. Not built.
- Backend bulk endpoints — looping the existing single-item endpoints is sufficient for an
  admin tool at realistic N; no backend change.
- Bulk on the Permissions page (a read-only catalog — nothing to act on).

## Notes

- Reuse: `useBulkSelect` (`@/hooks/useBulkSelect`), `ListSkeleton`
  (`@/components/explore/ListSkeleton`), the Notebooks list's bulk-toggle + bulk-bar
  pattern (`frontend/src/routes/explore/Notebooks.tsx`) as the interaction reference,
  `FilterableTable` (AX-2), shadcn `Checkbox`/`Button`, `toast` from "sonner", the
  `--success` token (AX-1).
- Keep the bulk logic per page small: a `runBulk(ids, op)` helper that maps ids → the
  single mutation, collects results, toasts the summary + optional Undo. If a page's file
  grows unwieldy, extract the bulk-bar into a small in-file component — don't restructure
  the dialogs or data hooks.
