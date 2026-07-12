# Access-Control UX Uplift — Roadmap

**Status:** Approved 2026-07-12
**Goal:** Bring egret's authorization *surface* up to the level of its authorization *model* — from "engineer-usable" to "admin-delightful". No new authz mechanisms (the Effort-B model is already complete); this is a UX/surfacing initiative, mostly frontend.

## Why

Egret's authz backend (Effort B) is genuinely full-featured — single Cedar PDP chokepoint, two-plane (platform/workspace) + object scope, role hierarchy, explicit deny, object sharing, groups, per-user data-plane delegation, audit log. But the UI under-delivers: the model is more capable than the console exposes. Live review of the Access-Control pages surfaced concrete gaps (see the "admin-delightful" definition below).

## "Admin-delightful" = six things an admin can do fast

1. Find anyone / anything instantly (search / filter / sort).
2. Run their own workspace without bugging a platform admin (a per-workspace Members home).
3. Answer "who can do what, and why" in one click (effective-access with provenance).
4. See the full picture at a glance — hierarchy, deny, grants — without manual reasoning.
5. Never get confused about platform vs workspace scope.
6. Act in bulk and undo mistakes safely.

## Increments (each = its own spec → plan → subagent-build → review → squash-merge, like Effort B)

Sequence (reordered per approval — AX-3 pulled up to #2):

**AX-1 → AX-3 → AX-2 → AX-4 → AX-5**

| # | Increment | What makes admin "sướng" | Backend | Frontend | Size | Depends on |
|---|---|---|---|---|---|---|
| **AX-1** | **Workspace Members page** | ws_admin gets a home; self-serve team management | enrich `MemberDto` (principal_name, added_by/at); last-admin guard; confirm `add_member` is upsert or add role-change path | `/explore/{ws}/settings/members` + workspace-settings shell; table + inline role select + add picker (reuse ShareDialog `AddGrantRow`) + group expand + guardrails + scope clarifier. Build `FilterableTable` v1 here. | M | — (mockup ready) |
| **AX-3** | **Effective-access viewer** ⭐ | pick a user/group → "what can they do, where, and why" (direct / via group / inherited / denied) | an **explain** endpoint that resolves effective access + provenance (PDP already computes closures; surface Cedar diagnostics / reason chain) | per-principal viewer (page or drawer): resolved roles, effective permissions with provenance, per-workspace membership + object grants, deny call-outs | L | AX-1 (`FilterableTable`, name-resolution) |
| **AX-2** | **Table tooling + density** | find/filter/sort every list; drop filler KPI cards | — | generalize `FilterableTable` (search + faceted filters + sortable headers) across Users / Roles / Permissions; replace oversized KPI `StatCardRow`s with compact summary strips | M | AX-1 (component) |
| **AX-4** | **Surface the hidden model** | see hierarchy / deny / grants directly; no guessing | — | Roles: inheritance tree + permission preview + deny flags; role editor: **scope-filter the permission catalog** (workspace role → only workspace perms), square + higher-contrast checkboxes, select-all-per-group, in-dialog search; in-product scope clarifiers | M | AX-2 |
| **AX-5** | **Bulk ops + safety polish** | disable / assign / remove in bulk; undo; clearer state | (optional) a bulk endpoint | bulk-select + actions; undo toasts on destructive/bulk; a semantic `--success` token for state badges (separate from the teal brand accent); consistent empty / skeleton states | S–M | AX-2 |

## Milestone

**AX-1 + AX-3 + AX-2 = the "delightful MVP"** (home + answer-tool + fast find). **AX-4 + AX-5 = polish.**

## Cross-cutting principles (same as Effort B)

- Per increment: spec (EN canonical + VI reference) → writing-plans → subagent-driven-development → adversarial security/UX review → squash-merge `--admin`.
- **Add no new authz mechanisms** — the model is complete; only lift the UX to match it.
- **Reuse** existing components: `SortableHeader`, `useBulkSelect`, the `ShareDialog` principal picker, `FolderHeaderRow`, `StatCard`.
- Each increment ships independently (admin sees progress from AX-1 onward).

## Reference

- Live design review + gap analysis: this session (2026-07-12).
- AX-1 mockup: `scratchpad/workspace-members-mockup.html` (published artifact).
- Authz model: `[[egret-effort-b-authz]]`, `[[egret-platform-rbac]]`, ADR-19 three-layer model `[[egret-three-layer-management-model]]`.
