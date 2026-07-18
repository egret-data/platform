# E2E: auth + control-plane admin on kind — Design

**Date:** 2026-07-18 · **Status:** Design — approved · **Branch:** `feat/e2e-auth-cp-admin` (off `main`)

> Vietnamese reference: `docs/vi/specs/2026-07-18-e2e-auth-cp-admin-design.md`.

## Problem

Project 2 (full automated e2e) slice 1 — the data-plane money-path — is done ([[egret-e2e-dataplane-kind]], PR #273). The next slice is **auth + control-plane admin**: the platform's entry (login) and management surface (access console: users / groups / roles / permissions / audit), plus the privilege-escalation guard that keeps non-admins out of the control plane. None of this is covered by an automated e2e yet.

This surface is UI form/table driven — deterministic, no async k8s provisioning — so it runs in seconds and is reliable (the opposite of slice 1's engine-warmup flakiness).

## Goal

Automated Playwright specs, run via the existing `just e2e-kind` runner against the in-cluster kind deploy (`egret.localtest.me`), that assert:

1. **Auth gating** (security money-path): a break-glass admin reaches the control plane; an OIDC member is **denied** it.
2. **Control-plane admin**: the access console pages load with seeded data; an RBAC group CRUD round-trip works; the mutation shows in the audit log.

Run model unchanged from slice 1: kind is the canonical target, opt-in via `EGRET_E2E_KIND=1`, **not** in GitHub CI (unit-only there).

## Out of scope

- Governance (policies / masking / row-filter / classifications / Cedar) — its own slice.
- OIDC-admin-via-group-seed (the `deploy/seed-sso-test-rbac.sql` path) — the member-denied test needs no RBAC seed.
- Workspace/engine provisioning (covered elsewhere), role-assignment to users, permission-catalog editing (code-defined, read-only).
- No app/backend change — test harness + docs only.

## Context — grounded surface (verified)

- **Guards** (`src/auth/guards.tsx`): `RequireControlPlaneAccess` renders `<Forbidden/>` when `me.permissions`/roles don't grant CP access; `RequireGlobalAdmin` needs `platform_roles.includes("platform_admin")`; `RequirePermission perm="X:read"` gates each section. `<Forbidden/>` renders **"403 — Forbidden"** + "You don't have access to this page." A JIT OIDC member has `platform_roles.length === 0` → Forbidden on `/control-plane` (no workspace required).
- **Header** (`src/components/layout/Header.tsx`): the **"Control Plane"** link shows only when `canAdmin` = `platform_roles.includes("platform_admin")` OR `permissions` intersects `CONTROL_PLANE_PERMS`. A plain member doesn't see it.
- **CP routes** (`src/App.tsx:132-161`): `/control-plane/{users,groups,roles,permissions,access-overview,access-explorer,who-can-access,audit-log,workspaces}`.
- **Access console** (`src/routes/control-plane/access/*`): `GroupsPage` has a **Create** button → dialog with a name field → `createGroup({name})` (`src/api/access.ts`), plus per-group actions (manage roles, members, rename, **Delete** via `ConfirmDeleteDialog`). `PermissionsPage` is a read-only catalog. `UsersPage`/`RolesPage` list seeded entries.
- **Login helpers** (`frontend/tests/e2e/helpers.ts`): `loginLocalAdmin` (break-glass) and `loginViaOidc` (Keycloak SSO, defaults to `tester@egret.local`, waits for `/explore/{ws}/`). Realm seeds `member-sso@egret.local` / `Member12345!` (group `data-team`, plain member) and `admin-sso` / `tester`.
- **Runner**: `just e2e-kind` (slice 1) — `deploy-kind-oidc` + `wait-ready.sh` + `npx playwright test`. This slice's specs run under the same command (all `dataplane-kind`/new specs match `EGRET_E2E_KIND`).

## Architecture

Two focused specs + one small helper; pure browser-UI (no `page.request` provisioning, no k8s waits).

### A. `frontend/tests/e2e/auth-gating.spec.ts` — the security money-path

Gate: skip unless `EGRET_E2E_KIND=1`.

- **Test 1 — break-glass admin reaches the control plane.** `loginLocalAdmin(page)` → assert the Header **"Control Plane"** link is visible → `goto /control-plane` → assert the control plane renders (a known CP element visible) and **"403 — Forbidden"** is **not** present.
- **Test 2 — an OIDC member is denied the control plane.** `loginViaOidcMember(page)` (new helper: fills the Keycloak form for `member-sso@egret.local` / `Member12345!` and waits for the callback back into the app — does NOT require landing in `/explore`, so no workspace binding is needed) → `goto /control-plane` → assert **"403 — Forbidden"** is visible AND the Header **"Control Plane"** link is absent.

### B. `frontend/tests/e2e/cp-admin.spec.ts` — the access console (as admin)

Gate: skip unless `EGRET_E2E_KIND=1`. `loginLocalAdmin` once (a `beforeEach` or per-test).

- **Test 1 — access console loads with seeded data.** Visit `/control-plane/users`, `/groups`, `/roles`, `/permissions`; assert each renders its table and a known seeded entry is present: `admin@egret.local` in Users, ≥1 role in Roles, a non-empty permission catalog in Permissions.
- **Test 2 — RBAC group CRUD round-trip.** On `/control-plane/groups`: click **Create** → enter a unique name `e2e-grp-${RUN_ID}` → submit → assert the group appears in the list. Then open the group's actions → **Delete** → confirm in `ConfirmDeleteDialog` → assert the group is gone. (Assigning a role to the group is a **stretch** assertion — include only if the UI flow is a clean 2–3 steps; otherwise leave it to a later slice.)
- **Test 3 — audit log records the mutation.** `goto /control-plane/audit-log`; assert an entry for the group create/delete appears (match on the group name `e2e-grp-${RUN_ID}` or the action type).

### C. Helper — `loginViaOidcMember(page)` in `helpers.ts`

Like `loginViaOidc` but for the member identity and without the `/explore` wait: go to `/login`, click "Sign in with SSO", fill the Keycloak form (`EGRET_E2E_OIDC_MEMBER_USER` / `..._PASSWORD`, defaulting to `member-sso@egret.local` / `Member12345!`), submit, and wait for the URL to return to the app origin (the OIDC callback resolves). Credentials overridable via env.

### D. Docs

Add an "Auth + control-plane admin e2e" subsection to `frontend/tests/e2e/README.md` (what the two specs assert; same `just e2e-kind` run; the member-denied test needs the seeded `member-sso` realm user, no RBAC SQL seed).

## Testing / acceptance

Not "wrote tests" — **run them for real** via `just e2e-kind` (or `npx playwright test auth-gating cp-admin` against a running deploy) and confirm both specs PASS. Unit gates (`npm run lint`/`typecheck`/`test`/`build`) stay green; the specs are opt-in and excluded from `npm test`.

## Risks

1. **OIDC member login timing** — the Keycloak form + callback can be slow; the helper waits for the app-origin URL, not a fixed delay. Mitigate with a generous `waitForURL` timeout.
2. **Selector drift** in the access console tables/dialogs — centralize the Create/Delete flow in the spec; prefer role/text selectors ("Control Plane", "Create", "403 — Forbidden", the group name).
3. **Audit-log propagation lag** — a mutation may take a moment to appear; poll/reload the audit page with a short cap.
4. **member-sso realm user missing** — if the deploy's realm import predates the seeded SSO users, Test 2 can't log in; documented as a prereq (fresh `deploy-kind-oidc` imports them).

## Project 2 remaining after this slice

governance (policies/masking/Cedar) · provisioning-as-test · notebooks (Daft) · BI/SQL-Lab consumer UI. Each its own spec → plan → implementation.
