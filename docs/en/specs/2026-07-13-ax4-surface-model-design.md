# AX-4 — Surface the Hidden Model — Design

**Status:** Approved
**Date:** 2026-07-13
**Initiative:** Access-Control UX uplift (increment 4 of 5) — see
`docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`

## Goal

Make the Roles surface show the depth the authz model already has: the **role
hierarchy** (which role inherits from which), and a role editor that only offers the
permissions a role can actually hold. Today the Roles table hides the parent/inherits
relationship (a B2-3 feature) behind a single "Edit" dialog, and the permission matrix
shows the *entire* platform+workspace catalog for every role — so editing a **workspace**
role still lists platform-only groups (Engines/Warehouses/Storage/Secrets) it can never
hold, which is confusing clutter (flagged in the AX-1 review). Add in-product scope
clarifiers so platform-vs-workspace stops being ambiguous.

**Frontend-only.** The data is already present: `AccessRole` carries `parentRoleId` +
`scope`, and `PermissionDto` carries `scope` (`"platform"|"workspace"`). No backend
change.

## Design

### 1. Roles table — surface the hierarchy (two modes)

The Roles page uses the AX-2 `FilterableTable`. Add both a column and an optional tree,
per approval:

- **List mode (default):** add an **"Inherits from"** column. Resolve each role's
  `parentRoleId` to the parent's `name` via a `Map<id, name>` built from the loaded
  roles list; render it as a `Badge`/chip, or `—` when there is no parent. The column
  gets a `value` accessor (parent name) so it is searchable and sortable like the others.
  Everything else on the page (summary strip, search, Scope facet, dialogs, actions,
  built-in badge, assignee drill-in) is unchanged.
- **Tree view (toggle):** a "Tree view" toggle button in the page header (styled like the
  existing header toggles). When on, the table body is replaced by an **indented tree**:
  roles grouped by scope (a Platform section and a Workspace section, since a parent must
  share its child's scope), each root role listed with its children nested and indented
  one level per hierarchy depth. Since search + the Scope facet are list-mode tools, they
  are **hidden** while Tree view is active, with a small note ("Tree view shows the full
  hierarchy — switch to list to search or filter."). Each tree node shows the role name +
  built-in badge; clicking a node opens the same Edit dialog (admin) as the list row, so
  no action is lost. The tree is a pure structural view of all roles.

Build the indentation from the parent chain (roots = roles with no `parentRoleId` within
their scope; recurse over children). Guard against the (DB-prevented) cycle with a depth
cap.

### 2. Role editor — scope-filter the permission matrix + polish

In `RolesPage.tsx`'s `EditRoleDialog`:

- **Scope-filter (the fix):** render only the permission groups/permissions whose
  `scope === role.scope`. A workspace role's matrix shows only workspace-scoped groups
  (Charts/Dashboards/Datasets/Notebooks/SQL Lab/Workspaces…); platform-only groups are
  hidden entirely. `platform_admin` keeps its existing superuser treatment (matrix
  disabled + note). This uses the `scope` field already on each catalog permission.
- **In-dialog search:** a search `Input` at the top of the matrix that filters the shown
  permissions by `key` or `description` (case-insensitive); a group with no matches is
  hidden. Independent of the checked state.
- **Select-all-per-group:** each group header gets a checkbox (a shadcn `Checkbox`, or a
  tri-state look via `indeterminate` when partially checked) that toggles all of that
  group's **non-inherited** permissions on/off at once. Inherited permissions stay
  disabled and are unaffected.
- **Checkbox affordance:** verify the permission checkboxes render a clearly visible
  checked vs unchecked state in both themes (the AX-1 review noted low contrast). Use the
  standard shadcn `Checkbox` (square, filled-on-check); adjust only if contrast is
  genuinely weak. Do not invent a custom control.

### 3. Scope clarifiers (in-product)

- **Roles page:** a one-line clarifier under the header (reuse the AX-1 Members
  clarifier style): "Platform roles apply across all of egret; workspace roles apply
  inside a single workspace and are assigned per-workspace on that workspace's Members
  page."
- **Role editor:** a short line near the matrix: "This is a **{scope}** role — it can
  hold {scope}-scoped permissions." (explains why the matrix is scope-filtered).

## Testing (Vitest)

- **Roles table:**
  - The "Inherits from" column renders the parent role's **name** (not its id) for a role
    with a parent, and `—` for a root role.
  - Toggling "Tree view" replaces the table with an indented tree that nests a child role
    under its parent and groups by scope; the search box + Scope facet are hidden in tree
    mode.
  - List mode still sorts + filters (a regression check on AX-2 behavior).
- **Role editor:**
  - Opening the editor for a **workspace** role shows only workspace-scoped permission
    groups and NOT a platform-only group (e.g. no "Engines"); a **platform** role shows
    platform groups.
  - The in-dialog search filters the matrix; a group with no matches disappears.
  - The group select-all checkbox toggles all non-inherited perms in that group; inherited
    perms remain checked+disabled.
- **Clarifiers:** the Roles-page clarifier and the editor's scope line render.

## Out of scope

- Showing an inherited-permission **count** ("own +N inherited") on the Roles list — the
  list DTO carries only the own `permissionCount`; a count would need a small backend DTO
  field. Deferred as an optional follow-up (the "Inherits from" column + tree already
  convey inheritance).
- **AX-5** — bulk operations + undo.
- Any backend change (this increment is purely presentational; all data is already on the
  DTOs).
- Editing the hierarchy from the tree (parent changes still happen in the Edit dialog's
  existing "Inherits from" select).

## Notes

- Reuse: the AX-2 `FilterableTable` (list mode), the AX-1 Members clarifier style, the
  existing `EditRoleDialog` structure (groups + checkboxes + inherited badges + the
  "Inherits from" parent select), shadcn `Checkbox`/`Input`/`Button`.
- Keep `RolesPage.tsx` from ballooning: if the tree render or the editor changes push the
  file well past its current size, extract the tree body into a small in-file component
  (or a sibling file `RolesTree.tsx`) — split by responsibility, don't restructure the
  dialogs.
