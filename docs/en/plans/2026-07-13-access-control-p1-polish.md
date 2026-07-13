# Access Control P1 — Surface polish + role impact-preview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** The P1 roadmap items — (U3/U3b) standardize the access surfaces by de-duplicating copied primitives and using `ListSkeleton` consistently, and (U5) show an **impact preview** ("held by N assignees · M roles inherit") before editing or deleting a role.

**Architecture:** Pure frontend, behavior-preserving except the additive impact notice. Extract the primitives copied across the access pages into one home each; swap ad-hoc `<p>Loading…</p>` for the existing `ListSkeleton`; add a small `RoleImpactNotice` computed from data already on the page (`RoleDto.assigneeCount` + client-side descendant walk over the flat roles list — no new endpoint, no new fetch).

**Tech Stack:** React 18 + TanStack Query + shadcn/Tailwind + Vitest.

---

## Context (verified duplications)

- **`errorStatus(err): number | undefined`** — identical copy in 6 files: `routes/explore/settings/Members.tsx`, `SharedObjects.tsx`, `RolesPermissions.tsx`, `routes/control-plane/access/WhoCanAccess.tsx`, `AccessExplorer.tsx`, `components/sharing/ShareDialog.tsx`.
- **`allowStyle`/`denyStyle` + `EffectBadge`** — identical copy in 3 files: `SharedObjects.tsx`, `WhoCanAccess.tsx`, `AccessExplorer.tsx`. NB `AccessExplorer.tsx` also applies `denyStyle` directly to the "Blocked" list (`style={denyStyle}`), so the styles must be exported, not just the component.
- **Plain `Loading…` full-panel text**: `AccessExplorer.tsx`, `WhoCanAccess.tsx`, `PermissionsPage.tsx`, `GroupsPage.tsx` (RolesPage already uses `ListSkeleton` for its table).
- **`ConfirmDeleteDialog`** (`components/ui/confirm-delete-dialog.tsx`) — props `{ open, onOpenChange, title, description: string, confirmText?, onConfirm }`; renders `description` in an `AlertDialogDescription`. No `children` slot yet.
- **RolesPage** (`routes/control-plane/access/RolesPage.tsx`): `AccessRole`/`RoleDto` fields are camelCase on the FE — `id, name, scope, description, permissionCount, assigneeCount, parentRoleId`. Delete uses `<ConfirmDeleteDialog title description confirmText="delete" onConfirm={…} />`; permission editing uses `EditRoleDialog({ role, roles, onOpenChange })`; `RoleAssigneesDialog` already lists assignees via `getRoleAssignees(id)`. `cyclicParentIds` already demonstrates the parent-chain walk pattern over the flat list.

---

## Non-Goals

- No full re-layout of `AccessExplorer` into the `FilterableTable` shell (high churn, low ROI) — surface standardization here = shared primitives + consistent skeletons only.
- No backend change; no new endpoint; no new fetch for the impact preview (uses `assigneeCount` already on the row + a client-side descendant count).
- No change to `SourceSuffix`/`sourceLabel` (the label vocabularies differ between AccessExplorer `group:` and WhoCanAccess `direct|workspace|platform` — not a clean shared unit; leave per-page).

---

## File Structure

- Create `frontend/src/lib/httpStatus.ts` — `errorStatus`.
- Create `frontend/src/components/access/EffectBadge.tsx` — `EffectBadge`, `allowStyle`, `denyStyle`.
- Create `frontend/src/components/access/RoleImpactNotice.tsx` — the impact summary.
- Modify the 6 `errorStatus` consumers + 3 `EffectBadge` consumers to import from the new homes and drop their local copies.
- Modify `AccessExplorer.tsx`, `WhoCanAccess.tsx`, `PermissionsPage.tsx`, `GroupsPage.tsx` — `ListSkeleton` for full-panel loading.
- Modify `components/ui/confirm-delete-dialog.tsx` — optional `children` slot.
- Modify `RolesPage.tsx` — render `RoleImpactNotice` in the delete dialog + `EditRoleDialog`.
- Tests: update any that assert the literal "Loading…" string; add impact-preview assertions to `RolesPage.test.tsx`.

---

## Tasks

### Task 1: Extract shared primitives + consistent skeletons (behavior-preserving)

**Files:** create `lib/httpStatus.ts`, `components/access/EffectBadge.tsx`; modify the consumers listed above.

- [ ] **Step 1 — Create the shared modules.**
  `lib/httpStatus.ts`:
  ```ts
  /** Narrow a react-query error into an HTTP status when one is present. */
  export function errorStatus(err: unknown): number | undefined {
    if (err && typeof err === "object" && "status" in err) {
      const s = (err as { status?: unknown }).status;
      if (typeof s === "number") return s;
    }
    return undefined;
  }
  ```
  `components/access/EffectBadge.tsx` — move the exact `allowStyle`/`denyStyle` `CSSProperties` (the `color-mix(in oklab, var(--success)/var(--destructive) …)` values) + `EffectBadge` component verbatim from `AccessExplorer.tsx`; export all three.
- [ ] **Step 2 — Repoint consumers, delete local copies.** In all 6 `errorStatus` files: remove the local `function errorStatus` and `import { errorStatus } from "@/lib/httpStatus"`. In `SharedObjects.tsx`, `WhoCanAccess.tsx`, `AccessExplorer.tsx`: remove local `allowStyle`/`denyStyle`/`EffectBadge`, import from `@/components/access/EffectBadge`; keep `AccessExplorer`'s `style={denyStyle}` on the Blocked list working via the import.
- [ ] **Step 3 — Skeletons.** In `AccessExplorer.tsx`, `WhoCanAccess.tsx`, `PermissionsPage.tsx`, `GroupsPage.tsx`: replace the full-panel `<p …>Loading…</p>` with `<ListSkeleton rows={…} cols={…} />` (import from `@/components/explore/ListSkeleton`; pick row/col counts matching each page's table width — e.g. AccessExplorer report is list-shaped, use a modest `rows={4} cols={2}`). Do NOT change loading semantics elsewhere; only the placeholder.
- [ ] **Step 4 — Fix affected tests.** Run `npm run test`. If any test asserted the literal "Loading…" text on a changed page, update that assertion to check for the skeleton (`getByTestId("list-skeleton")`) — change ONLY those assertions, nothing else.
- [ ] **Step 5 — Verify.** `npm run typecheck` + `npm run lint` clean; `npm run test` green (report counts). No behavior change beyond the loading placeholder.
- [ ] **Step 6 — Commit.** `refactor(fe): dedupe errorStatus + EffectBadge, consistent access skeletons`.

### Task 2: Role impact preview (U5)

**Files:** create `components/access/RoleImpactNotice.tsx`; modify `components/ui/confirm-delete-dialog.tsx`, `RolesPage.tsx`; test `routes/control-plane/access/__tests__/RolesPage.test.tsx`.

- [ ] **Step 1 — Failing test.** In `RolesPage.test.tsx` (mirror its existing harness/mocks): render the roles table with a role `analyst` that has `assigneeCount: 5` and a child role whose `parentRoleId` = analyst's id. Open its Delete action; assert the confirm dialog shows the impact — the assignee count (e.g. text matching /5 assignee/) AND that 1 role inherits (e.g. /1 role.*inherit/i). Also assert opening **Edit permissions** shows the same impact summary.
- [ ] **Step 2 — Run, confirm fail.** `npm run test -- RolesPage`.
- [ ] **Step 3 — Add the `children` slot to `ConfirmDeleteDialog`.**
  ```tsx
  interface ConfirmDeleteDialogProps { /* …existing… */ children?: React.ReactNode }
  // …render below <AlertDialogDescription>: {children && <div className="text-sm">{children}</div>}
  ```
  (Additive; existing callers unaffected.)
- [ ] **Step 4 — Create `RoleImpactNotice`.**
  ```tsx
  // props: { role: AccessRole; roles: AccessRole[] }
  // descendants = transitive children via parentRoleId over the flat list (guard against cycles, cap depth like cyclicParentIds).
  // Render a muted block:
  //   "Held by {role.assigneeCount} assignee{s}."  (omit/soften when 0)
  //   when descendants>0: "{n} role{s} inherit from this and will lose the inherited permissions."
  ```
  Use semantic tokens (muted/`--highlight` for emphasis when `assigneeCount>0`); no fetch.
- [ ] **Step 5 — Wire it in `RolesPage.tsx`.** Pass `<RoleImpactNotice role={deleteTarget} roles={rows} />` as `children` to the `ConfirmDeleteDialog` (guard on `deleteTarget`). In `EditRoleDialog`, render `<RoleImpactNotice role={role} roles={roles} />` near the top of the dialog body so admins see the blast radius before saving permissions.
- [ ] **Step 6 — Run, confirm pass**; `npm run typecheck` + `npm run lint` clean; `npm run test` green.
- [ ] **Step 7 — Commit.** `feat(fe): role edit/delete impact preview (assignees + inheriting roles)`.

### Task 3: Full verification

- [ ] `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` — all green; report the full-suite counts. (No backend touched, so no cargo run needed — but if any `.rs` changed unexpectedly, stop and report.)

---

## Self-Review Checklist

1. **Spec coverage:** dedupe errorStatus (Task 1) + EffectBadge (Task 1) = U3; ListSkeleton consistency (Task 1) = U3b; impact preview on edit + delete (Task 2) = U5.
2. **Behavior-preserving:** Task 1 changes only imports + the loading placeholder; every prior test still asserts the same behavior (except literal-"Loading…" assertions, intentionally updated). No prop/DOM changes to shared components except the additive `children`.
3. **No fetch/BE:** impact preview reads `assigneeCount` + computes descendants client-side. Confirm no new query added.
4. **Cycle safety:** the descendant walk caps depth / tracks visited (DB prevents cycles, but the walk must not infinite-loop).

## Follow-ups (not this plan)

- Full `AccessExplorer` re-layout into the standard `FilterableTable` shell.
- Impact preview could also fetch the assignee breakdown (users vs groups) if desired — currently uses the aggregate count to avoid a fetch.
- The two B1 nits (repoint Members `useWorkspaceRoles` off CP `listRoles`; widen FE `principal_type` union with `service`) — bundle into a cleanup PR.
