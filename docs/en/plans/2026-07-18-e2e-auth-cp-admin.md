# E2E Auth + Control-Plane Admin on Kind — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two Playwright specs on the in-cluster kind deploy that assert the auth privilege gate (break-glass admin reaches the control plane; an OIDC member is denied) and the control-plane access console (loads seeded data; RBAC group CRUD round-trip; the audit log records a `role:assign`).

**Architecture:** Pure browser UI + `page.request` for setup — no async k8s provisioning, so it runs in seconds and is deterministic. Reuses slice-1 helpers (`loginLocalAdmin`, `loginViaOidc`, `csrf`) and the `just e2e-kind` runner (gate `EGRET_E2E_KIND=1`, target `egret.localtest.me`).

**Tech Stack:** Playwright, TypeScript.

---

## Design notes (grounded surface)

- Guards (`src/auth/guards.tsx`): a JIT OIDC member has `platform_roles.length === 0` → `/control-plane` renders `<Forbidden/>` = **"403 — Forbidden"** + "You don't have access to this page." (no workspace required). The Header **"Control Plane"** link shows only for `canAdmin` (platform_admin or a CP permission).
- Access console (`src/routes/control-plane/access/*`): `GroupsPage` — **"New group"** button → dialog title **"New group"** → `Input#new-group-name` (Label "Name") → **"Create"** submit; per-group actions via a `DropdownMenuTrigger` (`aria-label="Actions for {name}"`) → **"Delete"** item → `ConfirmDeleteDialog` (type **"delete"** → **"Delete"** button). `PermissionsPage` is a read-only catalog.
- Endpoints (all under `/api/control-plane`, camel/snake as noted): `GET/POST /groups` (create body `{name}`), `DELETE /groups/{id}`, `POST /groups/{id}/roles` (body `{role_id}` → emits audit **`role:assign`**), `GET /access/roles`, `GET /audit`. **Group create/delete are NOT audited**; `role:assign`/`role:unassign` are.
- Realm seeds `member-sso@egret.local` / `Member12345!` (plain member). Break-glass admin `admin@egret.local` / `Admin12345!` (from `egret init`).

## File Structure

- **Modify** `frontend/tests/e2e/helpers.ts` — add `loginViaOidcMember`.
- **Create** `frontend/tests/e2e/auth-gating.spec.ts` — 2 tests (admin in / member out).
- **Create** `frontend/tests/e2e/cp-admin.spec.ts` — 3 tests (console loads / group CRUD / audit).
- **Modify** `frontend/tests/e2e/README.md` — add the auth + CP-admin subsection.

---

### Task 1: Add the `loginViaOidcMember` helper

**Files:**
- Modify: `frontend/tests/e2e/helpers.ts`

- [ ] **Step 1: Append the helper**

Add to `frontend/tests/e2e/helpers.ts`:

```ts
const OIDC_MEMBER_USER = process.env.EGRET_E2E_OIDC_MEMBER_USER ?? "member-sso@egret.local";
const OIDC_MEMBER_PASSWORD = process.env.EGRET_E2E_OIDC_MEMBER_PASSWORD ?? "Member12345!";

/**
 * Sign in through the real Keycloak as a plain member. Unlike `loginViaOidc`, this
 * does NOT wait for `/explore/{ws}/` — a member may have no workspace; it only waits
 * for the OIDC callback to land back in the app (off the Keycloak `/realms/` pages),
 * which is enough to establish the JIT-provisioned session.
 */
export async function loginViaOidcMember(page: Page): Promise<void> {
  await page.goto("/login");
  await page.getByRole("link", { name: /sign in with sso/i }).click();
  await page.locator("#username, input[name='username']").first().fill(OIDC_MEMBER_USER);
  await page.locator("#password, input[name='password']").first().fill(OIDC_MEMBER_PASSWORD);
  await page.locator("#kc-login, button[type='submit'], input[type='submit']").first().click();
  await page.waitForURL((url) => !/\/realms\//.test(url.toString()), { timeout: 30_000 });
}
```

- [ ] **Step 2: Typecheck**

Run: `cd frontend && npm run typecheck`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add frontend/tests/e2e/helpers.ts
git commit -m "test(e2e): add loginViaOidcMember helper"
```

---

### Task 2: Auth-gating spec

**Files:**
- Create: `frontend/tests/e2e/auth-gating.spec.ts`

- [ ] **Step 1: Write the spec**

Create `frontend/tests/e2e/auth-gating.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { loginLocalAdmin, loginViaOidcMember } from "./helpers";

/**
 * Auth privilege gate against the in-cluster kind deploy (egret.localtest.me).
 * Opt-in: EGRET_E2E_KIND=1. NOT part of `npm test` / GitHub CI.
 *
 * Proves the control plane is admin-only: a break-glass platform admin reaches it,
 * a JIT-provisioned OIDC member is denied (RequireControlPlaneAccess → Forbidden).
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";

test.describe("auth gating", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  test.setTimeout(60_000);

  test("break-glass admin reaches the control plane", async ({ page }) => {
    await loginLocalAdmin(page);
    // The Header exposes the Control Plane entry to admins.
    await expect(page.getByRole("link", { name: "Control Plane" })).toBeVisible();
    // The access console renders (not Forbidden) — seeded admin user is listed.
    await page.goto("/control-plane/users");
    await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
    await expect(page.getByText("admin@egret.local")).toBeVisible();
  });

  test("an OIDC member is denied the control plane", async ({ page }) => {
    await loginViaOidcMember(page);
    await page.goto("/control-plane");
    await expect(page.getByText("403 — Forbidden")).toBeVisible();
  });
});
```

- [ ] **Step 2: Typecheck + gate-skip**

Run: `cd frontend && npm run typecheck && npx playwright test auth-gating`
Expected: typecheck PASS; Playwright reports **2 skipped** (the `EGRET_E2E_KIND` gate is unset). If Playwright needs browsers, run `npx playwright install chromium` first.

- [ ] **Step 3: Commit**

```bash
git add frontend/tests/e2e/auth-gating.spec.ts
git commit -m "test(e2e): auth privilege-gate spec (admin in, OIDC member out)"
```

---

### Task 3: Control-plane admin spec

**Files:**
- Create: `frontend/tests/e2e/cp-admin.spec.ts`

- [ ] **Step 1: Write the spec**

Create `frontend/tests/e2e/cp-admin.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { loginLocalAdmin, csrf, postJson } from "./helpers";

/**
 * Control-plane access console against the in-cluster kind deploy. Opt-in
 * (EGRET_E2E_KIND=1), NOT in `npm test` / GitHub CI. As the break-glass admin:
 * the console pages load seeded data, a group CRUD round-trip works through the UI,
 * and an audited mutation (role:assign) shows in the audit log.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";
const RUN_ID = process.env.EGRET_E2E_RUN_ID ?? `${Date.now()}`;

test.describe("control-plane admin", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  test.setTimeout(90_000);

  test.beforeEach(async ({ page }) => {
    await loginLocalAdmin(page);
  });

  test("access console pages load with seeded data", async ({ page }) => {
    await page.goto("/control-plane/users");
    await expect(page.getByText("admin@egret.local")).toBeVisible();

    await page.goto("/control-plane/roles");
    await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
    await expect(page.getByText(/platform_admin/i).first()).toBeVisible();

    await page.goto("/control-plane/permissions");
    await expect(page.getByText("403 — Forbidden")).toHaveCount(0);
    await expect(page.getByText("group:create").first()).toBeVisible();
  });

  test("RBAC group CRUD round-trip via the UI", async ({ page }) => {
    const name = `e2e-grp-${RUN_ID}`;
    await page.goto("/control-plane/groups");

    // Create.
    await page.getByRole("button", { name: "New group" }).click();
    await page.locator("#new-group-name").fill(name);
    await page.getByRole("button", { name: "Create" }).click();
    await expect(page.getByText(name)).toBeVisible();

    // Delete: open the row's actions menu → Delete → type "delete" → confirm.
    await page.getByRole("button", { name: `Actions for ${name}` }).click();
    await page.getByRole("menuitem", { name: /Delete/ }).click();
    await page.getByPlaceholder("delete").fill("delete");
    await page.getByRole("button", { name: "Delete", exact: true }).click();
    await expect(page.getByText(name)).toHaveCount(0);
  });

  test("audit log records an audited mutation (role:assign)", async ({ page, context }) => {
    // Trigger an audited mutation via the API (group create/delete are NOT audited;
    // role:assign is). page.request shares the logged-in admin session.
    const h = await csrf(context);
    const grp = await postJson(page.request, "/api/control-plane/groups", h, {
      name: `e2e-audit-${RUN_ID}`,
    });
    const roles = (await (await page.request.get("/api/control-plane/access/roles")).json()) as Array<{
      id: string;
    }>;
    expect(roles.length, "at least one role exists").toBeGreaterThan(0);
    const assign = await page.request.post(`/api/control-plane/groups/${grp.id}/roles`, {
      headers: h,
      data: { role_id: roles[0].id },
    });
    expect(assign.ok(), `assign role → ${assign.status()} ${await assign.text()}`).toBeTruthy();

    // The audit log (UI) surfaces the role:assign entry. Filter by action, allowing
    // a brief propagation delay via toPass.
    await page.goto("/control-plane/audit-log");
    await page.locator("#audit-action").fill("role:assign");
    await expect(async () => {
      await expect(page.getByText("role:assign").first()).toBeVisible({ timeout: 3_000 });
    }).toPass({ timeout: 30_000 });

    // Cleanup.
    await page.request.delete(`/api/control-plane/groups/${grp.id}`, { headers: h }).catch(() => {});
  });
});
```

- [ ] **Step 2: Typecheck + gate-skip**

Run: `cd frontend && npm run typecheck && npx playwright test cp-admin`
Expected: typecheck PASS; Playwright reports **3 skipped**.

- [ ] **Step 3: Commit**

```bash
git add frontend/tests/e2e/cp-admin.spec.ts
git commit -m "test(e2e): control-plane access console spec (load, group CRUD, audit)"
```

> Note: the seeded-entry strings (`platform_admin` role, `group:create` permission) and the group actions-menu / audit-filter selectors are best-effort and finalized against the live UI in Task 4 — expected for browser e2e. Keep such tweaks minimal and local.

---

### Task 4: Document + live acceptance

**Files:**
- Modify: `frontend/tests/e2e/README.md`

- [ ] **Step 1: Add the README subsection**

Append to `frontend/tests/e2e/README.md`:

```markdown
## Auth + control-plane admin e2e (`auth-gating.spec.ts`, `cp-admin.spec.ts`)

Opt-in (`EGRET_E2E_KIND=1`), same `just e2e-kind` run, target `egret.localtest.me`.
Pure browser UI — deterministic, no async k8s (runs in seconds).

- `auth-gating`: a break-glass admin reaches `/control-plane`; a JIT OIDC member
  (`member-sso@egret.local`) is denied it (403 — Forbidden).
- `cp-admin`: the access console pages load seeded data; a group CRUD round-trip
  works through the UI; an audited `role:assign` (triggered via the API) shows in
  the audit log.

Needs the seeded `member-sso` realm user (a fresh `deploy-kind-oidc` imports it).
Member creds override via `EGRET_E2E_OIDC_MEMBER_USER` / `EGRET_E2E_OIDC_MEMBER_PASSWORD`.
```

- [ ] **Step 2: Commit**

```bash
git add frontend/tests/e2e/README.md
git commit -m "docs(e2e): document auth + control-plane admin specs"
```

- [ ] **Step 3: Live acceptance**

Ensure the kind deploy is up (from slice 1: `kind get clusters` → `egret`; if not deployed, `just deploy-kind-oidc && ./deploy/k8s/wait-ready.sh`). Then run both specs against it:

Run:
```bash
cd frontend && EGRET_E2E_KIND=1 EGRET_E2E_BASE_URL=http://egret.localtest.me \
  npx playwright test auth-gating cp-admin --reporter=list
```
Expected: **5 passed** (2 auth-gating + 3 cp-admin).

- [ ] **Step 4: Fix any selector/seed drift**

If a seeded-entry assertion or a dialog/menu selector fails, adjust it against the live DOM (use Playwright's failure screenshot/trace) — e.g. the exact role/permission strings, the actions-menu role, or the audit filter — and re-run until green. Commit any fix:

```bash
git add frontend/tests/e2e/*.spec.ts frontend/tests/e2e/helpers.ts
git commit -m "test(e2e): finalize auth/cp-admin selectors against live UI"
```

- [ ] **Step 5: Confirm unit gates**

Run: `cd frontend && npm run lint && npm run typecheck && npm test`
Expected: all pass (the new specs are opt-in, excluded from Vitest `npm test`).

---

## Self-Review

- **Spec coverage:** auth gating admin-in (Task 2 Test 1) ✓ + member-out (Test 2) ✓; console loads (Task 3 Test 1) ✓; group CRUD (Test 2) ✓; audit via `role:assign` (Test 3) ✓; helper `loginViaOidcMember` (Task 1) ✓; docs (Task 4) ✓; live acceptance (Task 4) ✓.
- **No placeholders:** every selector/endpoint pinned from the source (guards, GroupsPage, access routes). Complete code in each task.
- **Type consistency:** `loginViaOidcMember` / `loginLocalAdmin` / `csrf` / `postJson` used in the specs match their helper signatures.
- **Design deviation (intentional):** the audit test triggers `role:assign` (an audited action) rather than group create/delete (not audited) — the spec's "audit records the mutation" goal, made concrete. The group-role-assign is done via API (reliable) since the test asserts the audit UI, not the assign UI.
