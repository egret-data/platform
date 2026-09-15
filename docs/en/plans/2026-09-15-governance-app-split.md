# SP2 — Governance app split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move Data Governance (Policies, Classifications, Data Lineage) out of Control Plane into a top-level `/governance/*` app, keep the audit log in Control Plane under a new System group, and replace the two drifting Control Plane permission lists with one module.

**Architecture:** A pure `frontend/src/auth/appAccess.ts` decides who may enter each app and which app a URL belongs to; `guards.tsx`, `Header.tsx` and `Sidebar.tsx` all read it. `App.tsx` gains `GovernanceRoutes` behind a new `RequireGovernanceAccess`; the old Control Plane URLs are served by a `LegacyGovernanceRedirect` mounted at the **top level** of the route table (not inside `ControlPlaneRoutes` — see "Spec corrections" below). The five page files move with `git mv` to `frontend/src/routes/governance/`, and every in-app link is rewritten.

**Tech Stack:** React 19, react-router-dom 7.18.2, TanStack Query, Vitest + Testing Library, TypeScript, Tailwind/shadcn.

**Spec:** `docs/en/specs/2026-09-15-governance-app-split-design.md` (on branch `docs/control-plane-dg-review`; not on `main`).

**Source baseline.** Every snippet below was checked against `main` at `48f91c03` (the `frontend/` tree on `docs/control-plane-dg-review` is identical). Line numbers are from that commit. If the file in front of you disagrees with this plan, the plan is wrong — re-read the file and adapt.

## Spec corrections and decisions (read before Task 1)

1. **Redirects cannot live inside `ControlPlaneRoutes` (spec §3.4).** That tree is wrapped in `RequireControlPlaneAccess`. Once `policy:read` leaves `CONTROL_PLANE_PERMS` (spec §3.1), a governance-only user (`policy:read` and/or `tag:read`, nothing else) fails that guard and gets a 403 *before* any redirect route inside it renders — the exact user the redirect exists for. **Decision:** mount the three legacy routes as siblings of `/control-plane/*` in `App`'s top-level `<Routes>`. react-router 7 ranks by specificity, so `/control-plane/policies/*` beats `/control-plane/*` regardless of order (pinned by a test in Task 3).
2. **"A governance-only user who opens `/control-plane` is redirected to `/governance`" (spec §3.4)** — same problem: `ControlPlaneIndex` is behind the guard. **Decision:** `RequireControlPlaneAccess` itself redirects to `/governance` when the user fails Control Plane, passes Governance, and the path is exactly the landing (`/control-plane` or `/control-plane/`). Deep Control Plane links (e.g. `/control-plane/audit-log`) still 403 for that user.
3. **`as const` + `.includes(p)` does not typecheck (spec §3.1).** `(readonly ["engine:read", …]).includes(p: string)` is TS2345. **Decision:** type both lists `readonly string[]`.
4. **WarehouseDetail / EngineDetail link to no profile or tag (spec §3.5).** Grepped every `to=` / `navigate(` in both files: only `/control-plane/warehouses`, `/control-plane/engines/{id}`, `/control-plane/engines`. Nothing to change.
5. **Call sites the spec's list missed** (all handled in Task 5): `components/access/TagEditor.tsx:147` and `components/access/CatalogTagEditor.tsx:135` say "create them in Control Plane → Classifications", and their tests assert that text; `routes/control-plane/__tests__/defaultResourcesFromDef.test.ts` and `enrichResourceDefs.test.ts` import `../PolicyEditorPage` so they must move too; `PolicyEditorPage.tsx:42` imports `./config/FieldLabel` (relative — breaks on move); `ProfileDetailPage.tsx` has four path sites, not one; page docstrings name the old location.
6. **Header behaviour the spec leaves open.** **Decision:** Explorer / Governance / Control Plane render as one switcher; Governance and Control Plane show per `canEnter*`; Explorer shows whenever at least one of the other two does (with neither there is nothing to switch to, which is today's behaviour for plain members). The active entry gets `variant="secondary"` and `aria-current="page"`; the others `variant="ghost"`. `WorkspaceSwitcher` shows only in Explorer. Accessible names stay `"Explorer"` (button) and `"Control Plane"` (link) because e2e specs (`auth-gating.spec.ts:20`, `impersonation.spec.ts:144,156`, `narrow-viewport-kind.spec.ts:95`) query them; Governance is a link named `"Governance"`.
7. **Label casing.** Spec table says "Data lineage"; the existing label, test assertions and `Placeholder` title are "Data Lineage". **Decision:** keep "Data Lineage".
8. **Group heading uniqueness.** Collapsed-group state is one `localStorage` key keyed by heading across all apps, so the new headings (`Policies`, `Classification`, `Lineage`, `System`) must not repeat an existing heading. They don't.

**Not changed, flagged for follow-up (outside SP2):**
- `ControlPlaneIndex` also renders for the Control Plane `*` catch-all with a **relative** `<Navigate to="workspaces">`; under react-router 7 that is the same growing-URL loop `src/__tests__/exploreCatchAllRedirect.test.tsx` documents for Explore. `GovernanceIndex` below uses absolute targets so it does not repeat it.
- ~~A user with no workspace membership lands on "No workspaces yet" with no header~~ — pulled INTO SP2 as Task 4a.
- `workspace:read` admits a user to Control Plane, but no section is gated on it; such a user's landing is `workspaces` → 403.

## Global Constraints

- Frontend-only PR. Backend API paths are unchanged: `/api/control-plane/policies*`, `/api/control-plane/tags*`, `/api/control-plane/audit`. Do not edit anything under `frontend/src/api/**` and do not run `npm run gen:api` / orval.
- No Cedar, permission, or migration change.
- `CONTROL_PLANE_PERMS = engine:read, warehouse:read, storage:read, secret:read, user:read, group:read, role:read, workspace:read`; `GOVERNANCE_PERMS = policy:read, tag:read`.
- Import alias is `@/…` → `frontend/src/…`. New code uses it; do not add relative `./` imports that cross directories.
- react-router-dom is 7.18.2: redirect targets rendered from a splat/catch-all route must be **absolute**.
- Never run prettier. Frontend gates, run in `frontend/`: `npm run lint`, `npm run typecheck`, `npm test` (full suite — widely-mocked modules break other files), `npm run build`.
- Never run two test suites at once (other agents share this machine); one `npm test` at a time.
- Every `cd frontend && …` command runs from the worktree root `/Users/1pro-l4r8/Documents/github/data-portal/egret-governance-split`.
- Commits: Conventional Commits, each ending with the line `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Author identity is the repo's configured one — never override it.
- Branch `feat/governance-app-split` off `origin/main`. Do not merge; open the PR and stop.

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `frontend/src/auth/appAccess.ts` | Create | Permission lists, `canEnterControlPlane`, `canEnterGovernance`, `appForPath` |
| `frontend/src/auth/__tests__/appAccess.test.ts` | Create | Unit tests for the above |
| `frontend/src/auth/guards.tsx` | Modify | Delete local list; `RequireControlPlaneAccess` uses `appAccess` + landing redirect; add `RequireGovernanceAccess` |
| `frontend/src/auth/__tests__/guards.test.tsx` | Modify | Guard allow/deny/redirect tests |
| `frontend/src/routes/governance/LegacyGovernanceRedirect.tsx` | Create | `/control-plane/<x>` → `/governance/<x>`, keeping search + hash, `replace` |
| `frontend/src/routes/governance/__tests__/LegacyGovernanceRedirect.test.tsx` | Create | Subpath/search/hash/replace/specificity tests |
| `frontend/src/App.tsx` | Modify | `GovernanceRoutes`, `GovernanceIndex`, legacy routes, trimmed `ControlPlaneRoutes` + `FIRST_SECTION_BY_PERM` |
| `frontend/src/__tests__/governanceRouting.test.tsx` | Create | Real `<App/>` route-table tests |
| `frontend/src/routes/governance/{Policies,ProfileDetailPage,PolicyEditorPage,Classifications,TagDetail}.tsx` | Move (`git mv`) + modify | Pages, links rewritten |
| `frontend/src/routes/governance/__tests__/*` (7 files) | Move + modify | Their tests |
| `frontend/src/routes/control-plane/access/AccessOverview.tsx` | Modify | "Data plane" card → `/governance/policies` |
| `frontend/src/routes/control-plane/AuditLog.tsx` | Modify | Docstring only |
| `frontend/src/components/access/{TagEditor,CatalogTagEditor}.tsx` + tests | Modify | Copy "Governance → Classifications" |
| `frontend/src/components/layout/Sidebar.tsx` + `__tests__/Sidebar.test.tsx` | Modify | Governance mode, System group |
| `frontend/src/components/layout/Header.tsx` + `__tests__/PortalToggle.test.tsx`, `__tests__/Header.responsive.test.tsx` | Modify | Three-entry app switcher |

---

### Task 1: `appAccess` module

**Files:**
- Create: `frontend/src/auth/appAccess.ts`
- Test: `frontend/src/auth/__tests__/appAccess.test.ts`

**Interfaces:**
- Consumes: `type Me` from `@/api/hooks` (`{ user_id: string; email: string; platform_roles: string[]; permissions: string[]; impersonating?: … }`).
- Produces:
  - `export const CONTROL_PLANE_PERMS: readonly string[]`
  - `export const GOVERNANCE_PERMS: readonly string[]`
  - `export function canEnterControlPlane(me: Me | null | undefined): boolean`
  - `export function canEnterGovernance(me: Me | null | undefined): boolean`
  - `export type AppArea = "explore" | "governance" | "control-plane"`
  - `export function appForPath(pathname: string): AppArea`

- [ ] **Step 1: Create the worktree and install**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
git fetch origin main
git worktree add ../egret-governance-split -b feat/governance-app-split origin/main
cd ../egret-governance-split/frontend
npm ci
git branch --show-current
```

Expected: last line prints `feat/governance-app-split`. All later paths are relative to `/Users/1pro-l4r8/Documents/github/data-portal/egret-governance-split`.

- [ ] **Step 2: Write the failing test**

Create `frontend/src/auth/__tests__/appAccess.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { Me } from "@/api/hooks";
import {
  appForPath,
  canEnterControlPlane,
  canEnterGovernance,
  CONTROL_PLANE_PERMS,
  GOVERNANCE_PERMS,
} from "@/auth/appAccess";

const user = (platform_roles: string[], permissions: string[]): Me => ({
  user_id: "u",
  email: "u@example.com",
  platform_roles,
  permissions,
});

// What `data_steward` holds after migrations 0003, 0059, 0080, 0108, 0109
// (0060 later dropped `database:read`). It spans both apps.
const STEWARD = user(
  ["data_steward"],
  [
    "workspace:read", "user:read", "policy:read", "policy:create", "policy:update",
    "policy:delete", "policy:seed", "warehouse:read", "storage:read", "engine:read",
    "group:read", "role:read", "tag:read", "tag:manage", "dashboard:certify", "dataset:certify",
  ],
);

describe("canEnterControlPlane / canEnterGovernance", () => {
  it("admits platform_admin to both with no permissions at all", () => {
    const admin = user(["platform_admin"], []);
    expect(canEnterControlPlane(admin)).toBe(true);
    expect(canEnterGovernance(admin)).toBe(true);
  });

  it("admits a data_steward to both", () => {
    expect(canEnterControlPlane(STEWARD)).toBe(true);
    expect(canEnterGovernance(STEWARD)).toBe(true);
  });

  it("admits an infra-only user to Control Plane and not to Governance", () => {
    const infra = user([], ["engine:read"]);
    expect(canEnterControlPlane(infra)).toBe(true);
    expect(canEnterGovernance(infra)).toBe(false);
  });

  it("admits a policy-only user to Governance and not to Control Plane", () => {
    const policyOnly = user([], ["policy:read"]);
    expect(canEnterGovernance(policyOnly)).toBe(true);
    expect(canEnterControlPlane(policyOnly)).toBe(false);
  });

  it("admits a tag-only user to Governance — the permission both old lists missed", () => {
    const tagOnly = user([], ["tag:read"]);
    expect(canEnterGovernance(tagOnly)).toBe(true);
    expect(canEnterControlPlane(tagOnly)).toBe(false);
  });

  it("does not count a platform role other than platform_admin", () => {
    const roleOnly = user(["data_steward"], []);
    expect(canEnterControlPlane(roleOnly)).toBe(false);
    expect(canEnterGovernance(roleOnly)).toBe(false);
  });

  it("admits nobody without a session or with unrelated permissions", () => {
    for (const me of [null, undefined, user([], ["dashboard:certify", "policy:create"])]) {
      expect(canEnterControlPlane(me)).toBe(false);
      expect(canEnterGovernance(me)).toBe(false);
    }
  });

  it("keeps governance permissions out of the Control Plane list", () => {
    for (const perm of GOVERNANCE_PERMS) expect(CONTROL_PLANE_PERMS).not.toContain(perm);
  });
});

describe("appForPath", () => {
  it.each([
    ["/governance", "governance"],
    ["/governance/", "governance"],
    ["/governance/policies/trino/access/new", "governance"],
    ["/control-plane", "control-plane"],
    ["/control-plane/audit-log", "control-plane"],
    ["/explore/eng/sql-lab", "explore"],
    ["/", "explore"],
    ["/governance-reports", "explore"],
  ])("%s belongs to %s", (pathname, app) => {
    expect(appForPath(pathname)).toBe(app);
  });
});
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/auth/__tests__/appAccess.test.ts`
Expected: FAIL — `Failed to resolve import "@/auth/appAccess"`.

- [ ] **Step 4: Write the implementation**

Create `frontend/src/auth/appAccess.ts`:

```ts
import type { Me } from "@/api/hooks";

/**
 * Who may enter each top-level app, and which app a URL belongs to.
 *
 * The single source for `Header` (which entries to show), `Sidebar` (which
 * nav to render) and the area guards in `guards.tsx`. There used to be two
 * copies of the Control Plane list (Header + guards); both missed `tag:read`,
 * so a tag-only user could not reach Classifications at all.
 *
 * Typed `readonly string[]` rather than `as const`: `includes(p)` on a tuple of
 * literals rejects a plain `string` argument.
 */
export const CONTROL_PLANE_PERMS: readonly string[] = [
  "engine:read",
  "warehouse:read",
  "storage:read",
  "secret:read",
  "user:read",
  "group:read",
  "role:read",
  "workspace:read",
];

export const GOVERNANCE_PERMS: readonly string[] = ["policy:read", "tag:read"];

function isAdminOrHoldsAny(me: Me | null | undefined, perms: readonly string[]): boolean {
  if (!me) return false;
  if (me.platform_roles.includes("platform_admin")) return true;
  return me.permissions.some((p) => perms.includes(p));
}

/** platform_admin, or any Control Plane read permission. */
export function canEnterControlPlane(me: Me | null | undefined): boolean {
  return isAdminOrHoldsAny(me, CONTROL_PLANE_PERMS);
}

/** platform_admin, or any Governance read permission. */
export function canEnterGovernance(me: Me | null | undefined): boolean {
  return isAdminOrHoldsAny(me, GOVERNANCE_PERMS);
}

export type AppArea = "explore" | "governance" | "control-plane";

/** Segment-exact: `/governance-reports` is not Governance. */
export function appForPath(pathname: string): AppArea {
  if (pathname === "/governance" || pathname.startsWith("/governance/")) return "governance";
  if (pathname === "/control-plane" || pathname.startsWith("/control-plane/")) return "control-plane";
  return "explore";
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/auth/__tests__/appAccess.test.ts`
Expected: PASS (16 tests).

- [ ] **Step 6: Commit**

```bash
git add frontend/src/auth/appAccess.ts frontend/src/auth/__tests__/appAccess.test.ts
git commit -m "feat(auth): add appAccess module for app entry permissions

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Area guards

**Files:**
- Modify: `frontend/src/auth/guards.tsx` (imports at 1-6; RequireAuth docstring 8-22; `CONTROL_PLANE_PERMS` + `RequireControlPlaneAccess` at 66-89)
- Test: `frontend/src/auth/__tests__/guards.test.tsx`

**Interfaces:**
- Consumes: `canEnterControlPlane`, `canEnterGovernance` from Task 1.
- Produces:
  - `export function RequireControlPlaneAccess({ children }: { children: ReactNode })` — unchanged signature; now needs to render inside a Router (uses `useLocation`).
  - `export function RequireGovernanceAccess({ children }: { children: ReactNode })`

- [ ] **Step 1: Write the failing tests**

In `frontend/src/auth/__tests__/guards.test.tsx`:

Replace the six import lines (1-6) with:

```tsx
import type { ReactNode } from "react";
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import { AuthContext } from "@/auth/useAuth";
import type { Me } from "@/api/hooks";
import {
  RequireAuth,
  RequireControlPlaneAccess,
  RequireGlobalAdmin,
  RequireGovernanceAccess,
  RequirePermission,
  RequirePlatform,
} from "@/auth/guards";
```

In the test `"RequireControlPlaneAccess allows a user holding any control-plane permission"` change `permissions: ["policy:read"]` to `permissions: ["engine:read"]`.

Append to the end of the file:

```tsx
const withPerms = (permissions: string[], platform_roles: string[] = []): Me => ({
  user_id: "1",
  email: "a@b.c",
  platform_roles,
  permissions,
});

/** Mount a guard under `/control-plane/*` and `/governance/*`, like App does. */
function renderArea(path: string, me: Me | null, guarded: ReactNode) {
  return render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me, isLoading: false }}>
        <MemoryRouter initialEntries={[path]}>
          <Routes>
            <Route path="/control-plane/*" element={guarded} />
            <Route path="/governance/*" element={guarded} />
            <Route path="/governance" element={<span>governance landing</span>} />
          </Routes>
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
}

test("RequireControlPlaneAccess no longer admits a policy-only user to a Control Plane page", () => {
  renderArea(
    "/control-plane/engines",
    withPerms(["policy:read"]),
    <RequireControlPlaneAccess><span>control plane</span></RequireControlPlaneAccess>,
  );
  expect(screen.queryByText("control plane")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});

test.each(["/control-plane", "/control-plane/"])(
  "RequireControlPlaneAccess sends a governance-only user on %s to /governance",
  (path) => {
    // Rendered at `/governance` by the route above only if the guard redirected.
    render(
      <QueryClientProvider client={qc()}>
        <AuthContext.Provider value={{ me: withPerms(["tag:read"]), isLoading: false }}>
          <MemoryRouter initialEntries={[path]}>
            <Routes>
              <Route
                path="/control-plane/*"
                element={<RequireControlPlaneAccess><span>control plane</span></RequireControlPlaneAccess>}
              />
              <Route path="/governance" element={<span>governance landing</span>} />
            </Routes>
          </MemoryRouter>
        </AuthContext.Provider>
      </QueryClientProvider>,
    );
    expect(screen.getByText("governance landing")).toBeInTheDocument();
    expect(screen.queryByText(/Forbidden/i)).not.toBeInTheDocument();
  },
);

test("RequireControlPlaneAccess 403s a governance-only user on a Control Plane deep link", () => {
  renderArea(
    "/control-plane/audit-log",
    withPerms(["policy:read", "tag:read"]),
    <RequireControlPlaneAccess><span>control plane</span></RequireControlPlaneAccess>,
  );
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
  expect(screen.queryByText("governance landing")).not.toBeInTheDocument();
});

test("RequireControlPlaneAccess 403s a user with neither app on the landing", () => {
  renderArea(
    "/control-plane",
    withPerms([]),
    <RequireControlPlaneAccess><span>control plane</span></RequireControlPlaneAccess>,
  );
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});

test.each([
  ["tag:read only", withPerms(["tag:read"])],
  ["policy:read only", withPerms(["policy:read"])],
  ["platform_admin", withPerms([], ["platform_admin"])],
])("RequireGovernanceAccess admits %s", (_label, me) => {
  renderArea("/governance/policies", me, <RequireGovernanceAccess><span>governance</span></RequireGovernanceAccess>);
  expect(screen.getByText("governance")).toBeInTheDocument();
});

test.each([
  ["an infra-only user", withPerms(["engine:read"])],
  ["no session", null],
])("RequireGovernanceAccess denies %s", (_label, me) => {
  renderArea("/governance/policies", me, <RequireGovernanceAccess><span>governance</span></RequireGovernanceAccess>);
  expect(screen.queryByText("governance")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/auth/__tests__/guards.test.tsx`
Expected: FAIL — TypeScript/transform error `RequireGovernanceAccess` is not exported (`undefined` element type), and the policy-only / landing tests fail.

- [ ] **Step 3: Implement**

In `frontend/src/auth/guards.tsx`:

Replace lines 1-6:

```tsx
import type { ReactNode } from "react";
import { Navigate, useLocation, useParams } from "react-router-dom";
import { useAuth } from "./useAuth";
import { canEnterControlPlane, canEnterGovernance } from "./appAccess";
import { useMyWorkspaces } from "@/api/hooks";
import { Forbidden } from "@/routes/Forbidden";
import { ImpersonationBanner } from "@/components/layout/ImpersonationBanner";
```

In the `RequireAuth` docstring, change the first line
`` * Outer gate for every authenticated route (`/`, `/explore/*`, `/control-plane/*` ``
to
`` * Outer gate for every authenticated route (`/`, `/explore/*`, `/governance/*`, `/control-plane/*` ``.

Replace everything from `/** The read permissions that grant entry to the Control Plane area at all. */` through the closing `}` of `RequireControlPlaneAccess` (lines 66-89) with:

```tsx
/**
 * Outer gate for the whole `/control-plane/*` area: `platform_admin` OR any
 * Control Plane read permission (`CONTROL_PLANE_PERMS`) may enter. Per-route
 * guards (`RequirePermission`, `RequireGlobalAdmin`) then apply within.
 *
 * One exception to the 403: a user who cannot enter Control Plane but can
 * enter Governance, standing on the bare landing, is sent to `/governance`.
 * Policies used to live here, so that is where such a user's muscle memory and
 * bookmarks point. Deep links still 403 — `/control-plane/audit-log` is not a
 * Governance page, and silently landing somewhere else would hide that. The
 * moved pages' own old URLs are redirected earlier, in `App`, outside this guard.
 */
export function RequireControlPlaneAccess({ children }: { children: ReactNode }) {
  const { me } = useAuth();
  const { pathname } = useLocation();
  if (canEnterControlPlane(me)) return <>{children}</>;
  const onLanding = pathname === "/control-plane" || pathname === "/control-plane/";
  if (onLanding && canEnterGovernance(me)) return <Navigate to="/governance" replace />;
  return <Forbidden />;
}

/**
 * Outer gate for the whole `/governance/*` area: `platform_admin` OR any
 * Governance read permission (`GOVERNANCE_PERMS`) may enter. Per-route guards
 * then apply within, exactly as in Control Plane.
 */
export function RequireGovernanceAccess({ children }: { children: ReactNode }) {
  const { me } = useAuth();
  if (!canEnterGovernance(me)) return <Forbidden />;
  return <>{children}</>;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/auth/__tests__/guards.test.tsx src/auth/__tests__/appAccess.test.ts`
Expected: PASS (all tests in both files).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/auth/guards.tsx frontend/src/auth/__tests__/guards.test.tsx
git commit -m "feat(auth): add RequireGovernanceAccess and move policy:read out of Control Plane entry

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `LegacyGovernanceRedirect`

**Files:**
- Create: `frontend/src/routes/governance/LegacyGovernanceRedirect.tsx`
- Test: `frontend/src/routes/governance/__tests__/LegacyGovernanceRedirect.test.tsx`

**Interfaces:**
- Produces: `export function LegacyGovernanceRedirect(): JSX.Element` — no props; reads `useLocation()`; must be mounted on a route under `/control-plane/…`.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/routes/governance/__tests__/LegacyGovernanceRedirect.test.tsx`:

```tsx
import { render, screen } from "@testing-library/react";
import {
  createMemoryRouter,
  MemoryRouter,
  Route,
  RouterProvider,
  Routes,
  useLocation,
} from "react-router-dom";
import { describe, expect, it } from "vitest";
import { LegacyGovernanceRedirect } from "@/routes/governance/LegacyGovernanceRedirect";

function Where({ app }: { app: string }) {
  const { pathname, search, hash } = useLocation();
  return <div data-testid="where">{`${app} ${pathname}${search}${hash}`}</div>;
}

/**
 * The same route shapes App mounts. `/control-plane/*` is declared FIRST on
 * purpose: App relies on react-router 7 ranking the legacy routes above that
 * splat by specificity, not by declaration order.
 */
function renderAt(url: string) {
  render(
    <MemoryRouter initialEntries={[url]}>
      <Routes>
        <Route path="/control-plane/*" element={<Where app="control-plane" />} />
        <Route path="/control-plane/policies/*" element={<LegacyGovernanceRedirect />} />
        <Route path="/control-plane/classifications/*" element={<LegacyGovernanceRedirect />} />
        <Route path="/control-plane/data-lineage" element={<LegacyGovernanceRedirect />} />
        <Route path="/governance/*" element={<Where app="governance" />} />
      </Routes>
    </MemoryRouter>,
  );
  return screen.getByTestId("where").textContent;
}

describe("LegacyGovernanceRedirect", () => {
  it.each([
    ["/control-plane/policies", "governance /governance/policies"],
    ["/control-plane/policies/", "governance /governance/policies/"],
    ["/control-plane/policies/trino-prod", "governance /governance/policies/trino-prod"],
    [
      "/control-plane/policies/trino-prod/access/new?scope=tag#deny",
      "governance /governance/policies/trino-prod/access/new?scope=tag#deny",
    ],
    ["/control-plane/policies/trino-prod/masking/42", "governance /governance/policies/trino-prod/masking/42"],
    ["/control-plane/classifications", "governance /governance/classifications"],
    ["/control-plane/classifications/t1?tab=clearance", "governance /governance/classifications/t1?tab=clearance"],
    ["/control-plane/data-lineage", "governance /governance/data-lineage"],
  ])("sends %s to Governance with subpath, search and hash", (from, to) => {
    expect(renderAt(from)).toBe(to);
  });

  it.each(["/control-plane/audit-log", "/control-plane/engines", "/control-plane"])(
    "leaves %s in Control Plane",
    (url) => {
      expect(renderAt(url)).toBe(`control-plane ${url}`);
    },
  );

  it("replaces the history entry so Back does not bounce through the old URL", async () => {
    const router = createMemoryRouter(
      [
        { path: "/control-plane/policies/*", element: <LegacyGovernanceRedirect /> },
        { path: "/governance/*", element: <Where app="governance" /> },
      ],
      { initialEntries: ["/control-plane/policies/trino-prod?scope=tag"] },
    );
    render(<RouterProvider router={router} />);
    expect(await screen.findByTestId("where")).toHaveTextContent("governance /governance/policies/trino-prod?scope=tag");
    expect(router.state.historyAction).toBe("REPLACE");
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/routes/governance/__tests__/LegacyGovernanceRedirect.test.tsx`
Expected: FAIL — `Failed to resolve import "@/routes/governance/LegacyGovernanceRedirect"`.

- [ ] **Step 3: Implement**

Create `frontend/src/routes/governance/LegacyGovernanceRedirect.tsx`:

```tsx
import { Navigate, useLocation } from "react-router-dom";

/**
 * Old Control Plane URLs of the pages that moved to Governance (SP2): swaps the
 * leading `/control-plane` segment for `/governance` and keeps everything after
 * it — subpath, `?search` (the policy editor's `?scope=tag` picks By-tag mode)
 * and `#hash` — with `replace`, so Back does not return to the dead URL.
 *
 * Mounted by `App` at the top level, beside `/control-plane/*`, not inside
 * `ControlPlaneRoutes`: that tree sits behind `RequireControlPlaneAccess`,
 * which a governance-only user fails, so a redirect in there would never render
 * for the user it exists for.
 */
export function LegacyGovernanceRedirect() {
  const { pathname, search, hash } = useLocation();
  const target = pathname.replace(/^\/control-plane(?=\/)/, "/governance");
  return <Navigate to={{ pathname: target, search, hash }} replace />;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/routes/governance/__tests__/LegacyGovernanceRedirect.test.tsx`
Expected: PASS (12 tests).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/routes/governance/LegacyGovernanceRedirect.tsx frontend/src/routes/governance/__tests__/LegacyGovernanceRedirect.test.tsx
git commit -m "feat(governance): add redirect from legacy Control Plane governance URLs

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Governance route table in `App.tsx`

**Files:**
- Modify: `frontend/src/App.tsx` (imports line 6; `FIRST_SECTION_BY_PERM` 257-267; `ControlPlaneRoutes` 277-318; top-level routes 335)
- Test: `frontend/src/__tests__/governanceRouting.test.tsx`

**Interfaces:**
- Consumes: `RequireGovernanceAccess` (Task 2), `LegacyGovernanceRedirect` (Task 3). Page imports stay at `@/routes/control-plane/...` in this task; Task 5 moves them.
- Produces: route table — `/governance` (index → first section), `/governance/policies`, `/governance/policies/:service`, `/governance/policies/:service/:kind/new`, `/governance/policies/:service/:kind/:id`, `/governance/classifications`, `/governance/classifications/:id`, `/governance/data-lineage`, `/governance/*` → first section. Control Plane no longer mounts any of those seven.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/__tests__/governanceRouting.test.tsx`:

```tsx
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { queryClient } from "@/api/queryClient";
import type { Me } from "@/api/hooks";

/**
 * The REAL route table, not a copy. The failure this guards is one a copy
 * cannot see: where a redirect is mounted relative to an area guard. A
 * governance-only user fails `RequireControlPlaneAccess`, so any redirect
 * inside `ControlPlaneRoutes` would 403 them instead of moving them.
 *
 * `/api/auth/me` answers with the user under test; every other request rejects
 * (pages render their error states, which is fine — only routing is asserted).
 * "Page mounted without a 403" is read off `app-sidebar`: `AppShell` renders
 * inside both area guards and around the per-route `RequirePermission`, so once
 * the sidebar is in the DOM the per-route guard has rendered too.
 */
const TIMEOUT = { timeout: 4000 };

const me = (permissions: string[], platform_roles: string[] = []): Me => ({
  user_id: "u1",
  email: "u1@example.com",
  platform_roles,
  permissions,
});
const GOVERNANCE_ONLY = me(["policy:read", "tag:read"]);
const TAG_ONLY = me(["tag:read"]);
const INFRA_ONLY = me(["engine:read"]);
const ADMIN = me([], ["platform_admin"]);

function openApp(url: string, as: Me) {
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) =>
      String(input) === "/api/auth/me"
        ? Promise.resolve(
            new Response(JSON.stringify(as), { status: 200, headers: { "content-type": "application/json" } }),
          )
        : Promise.reject(new Error("no network in test")),
    ),
  );
  window.history.pushState({}, "", url);
  render(<App />);
}

const currentUrl = () => `${window.location.pathname}${window.location.search}${window.location.hash}`;

afterEach(() => {
  cleanup();
  queryClient.clear();
  vi.unstubAllGlobals();
  window.history.replaceState({}, "", "/");
});

describe("Governance routing in App", () => {
  it("sends a governance-only user who opens /control-plane to Governance, not a 403", async () => {
    openApp("/control-plane", GOVERNANCE_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/policies"), TIMEOUT);
    await screen.findByTestId("app-sidebar", {}, TIMEOUT);
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
  }, 15_000);

  it("lands an old policy-editor bookmark on the Governance editor with its query and hash", async () => {
    openApp("/control-plane/policies/trino/access/new?scope=tag#deny", GOVERNANCE_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/policies/trino/access/new?scope=tag#deny"), TIMEOUT);
    await screen.findByTestId("app-sidebar", {}, TIMEOUT);
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
  }, 15_000);

  it("lands an old tag bookmark on the Governance tag page", async () => {
    openApp("/control-plane/classifications/t1?tab=clearance", TAG_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/classifications/t1?tab=clearance"), TIMEOUT);
    await screen.findByTestId("app-sidebar", {}, TIMEOUT);
    expect(screen.queryByText("403 — Forbidden")).not.toBeInTheDocument();
  }, 15_000);

  it("moves the Data Lineage placeholder for an admin", async () => {
    openApp("/control-plane/data-lineage", ADMIN);
    await waitFor(() => expect(currentUrl()).toBe("/governance/data-lineage"), TIMEOUT);
    expect(await screen.findByRole("heading", { name: "Data Lineage" }, TIMEOUT)).toBeInTheDocument();
  }, 15_000);

  it("opens a tag-only user's Governance landing on Classifications", async () => {
    openApp("/governance", TAG_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/classifications"), TIMEOUT);
  }, 15_000);

  it("sends an unknown Governance URL to the first section", async () => {
    openApp("/governance/nope", GOVERNANCE_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/policies"), TIMEOUT);
  }, 15_000);

  it("403s an infra-only user in Governance and keeps their Control Plane landing", async () => {
    openApp("/governance/policies", INFRA_ONLY);
    expect(await screen.findByText("403 — Forbidden", {}, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe("/governance/policies");
  }, 15_000);

  it("still lands an infra-only user on their first Control Plane section", async () => {
    openApp("/control-plane", INFRA_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/control-plane/engines"), TIMEOUT);
  }, 15_000);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/__tests__/governanceRouting.test.tsx`
Expected: FAIL — the Governance tests time out on `currentUrl()` (e.g. stays `/control-plane` with a 403, or `/governance/...` renders `NotFound`). The last test ("infra-only … first Control Plane section") already passes.

- [ ] **Step 3: Implement — imports**

In `frontend/src/App.tsx` line 6, replace:

```tsx
import { RequireAuth, RequireControlPlaneAccess, RequireGlobalAdmin, RequirePermission, RequireWorkspace } from "@/auth/guards";
```

with:

```tsx
import {
  RequireAuth,
  RequireControlPlaneAccess,
  RequireGlobalAdmin,
  RequireGovernanceAccess,
  RequirePermission,
  RequireWorkspace,
} from "@/auth/guards";
```

After the line `import TagDetail from "@/routes/control-plane/governance/TagDetail";` add:

```tsx
import { LegacyGovernanceRedirect } from "@/routes/governance/LegacyGovernanceRedirect";
```

- [ ] **Step 4: Implement — Control Plane trim + Governance routes**

Delete the `{ path: "policies", perm: "policy:read" },` entry from `FIRST_SECTION_BY_PERM`.

In `ControlPlaneRoutes`, delete these seven lines (currently 295-301):

```tsx
          <Route path="policies" element={<RequirePermission perm="policy:read"><Policies /></RequirePermission>} />
          <Route path="policies/:service" element={<RequirePermission perm="policy:read"><ProfileDetailPage /></RequirePermission>} />
          <Route path="policies/:service/:kind/new" element={<RequirePermission perm="policy:read"><PolicyEditorPage /></RequirePermission>} />
          <Route path="policies/:service/:kind/:id" element={<RequirePermission perm="policy:read"><PolicyEditorPage /></RequirePermission>} />
          <Route path="classifications" element={<RequirePermission perm="tag:read"><Classifications /></RequirePermission>} />
          <Route path="classifications/:id" element={<RequirePermission perm="tag:read"><TagDetail /></RequirePermission>} />
          <Route path="data-lineage" element={<RequireGlobalAdmin><Placeholder title="Data Lineage" /></RequireGlobalAdmin>} />
```

Immediately after the closing `}` of `ControlPlaneRoutes`, add:

```tsx
// Governance (SP2): the data-plane rules a data steward manages, split out of
// the system-admin Control Plane. Same shape as `ControlPlaneRoutes` — one area
// gate, then a per-route permission. First section (in preference order) each
// Governance read permission unlocks, for the landing below.
const GOVERNANCE_FIRST_SECTION: { path: string; perm: string }[] = [
  { path: "/governance/policies", perm: "policy:read" },
  { path: "/governance/classifications", perm: "tag:read" },
];

function GovernanceIndex() {
  const { me } = useAuth();
  if (me?.platform_roles.includes("platform_admin")) return <Navigate to="/governance/policies" replace />;
  const perms = me?.permissions ?? [];
  const first = GOVERNANCE_FIRST_SECTION.find((s) => perms.includes(s.perm));
  // Absolute targets: this also renders for the `*` catch-all, where react-router 7
  // resolves a relative target against the unmatched URL and loops (see
  // `exploreRedirectTo`). `RequireGovernanceAccess` already admitted this user,
  // so `first` exists; the fallback only satisfies the type.
  return <Navigate to={first ? first.path : "/governance/policies"} replace />;
}

function GovernanceRoutes() {
  return (
    <RequireGovernanceAccess>
      <AppShell>
        <Routes>
          <Route index element={<GovernanceIndex />} />
          <Route path="policies" element={<RequirePermission perm="policy:read"><Policies /></RequirePermission>} />
          <Route path="policies/:service" element={<RequirePermission perm="policy:read"><ProfileDetailPage /></RequirePermission>} />
          <Route path="policies/:service/:kind/new" element={<RequirePermission perm="policy:read"><PolicyEditorPage /></RequirePermission>} />
          <Route path="policies/:service/:kind/:id" element={<RequirePermission perm="policy:read"><PolicyEditorPage /></RequirePermission>} />
          <Route path="classifications" element={<RequirePermission perm="tag:read"><Classifications /></RequirePermission>} />
          <Route path="classifications/:id" element={<RequirePermission perm="tag:read"><TagDetail /></RequirePermission>} />
          {/* Platform-wide (cross-workspace) placeholder; admin-only until the real feature and its permission land. */}
          <Route path="data-lineage" element={<RequireGlobalAdmin><Placeholder title="Data Lineage" /></RequireGlobalAdmin>} />
          <Route path="*" element={<GovernanceIndex />} />
        </Routes>
      </AppShell>
    </RequireGovernanceAccess>
  );
}
```

- [ ] **Step 5: Implement — top-level routes**

In `App()`, replace:

```tsx
              <Route path="/control-plane/*" element={<RequireAuth><ControlPlaneRoutes /></RequireAuth>} />
```

with:

```tsx
              <Route path="/governance/*" element={<RequireAuth><GovernanceRoutes /></RequireAuth>} />
              <Route path="/control-plane/*" element={<RequireAuth><ControlPlaneRoutes /></RequireAuth>} />
              {/* Old Control Plane URLs of the pages that moved to Governance. Mounted
                  here, not inside ControlPlaneRoutes: that tree is behind
                  RequireControlPlaneAccess, which a governance-only user fails, so
                  they would get a 403 for their own bookmark. More static segments
                  outrank `/control-plane/*` in react-router 7, so order does not
                  matter. No RequireAuth: the destination carries it. */}
              <Route path="/control-plane/policies/*" element={<LegacyGovernanceRedirect />} />
              <Route path="/control-plane/classifications/*" element={<LegacyGovernanceRedirect />} />
              <Route path="/control-plane/data-lineage" element={<LegacyGovernanceRedirect />} />
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cd frontend && npx vitest run src/__tests__/governanceRouting.test.tsx src/__tests__/App.toaster.test.tsx src/__tests__/exploreCatchAllRedirect.test.tsx`
Expected: PASS (8 + 1 + 4).

Run: `cd frontend && npm run typecheck`
Expected: exit 0.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/App.tsx frontend/src/__tests__/governanceRouting.test.tsx
git commit -m "feat(governance): mount Governance app at /governance with legacy redirects

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4a: Home landing for a user with no workspace

A user whose only access is Governance or Control Plane (a fresh `data_steward`) has no workspace membership, so `HomeRedirect` (`frontend/src/App.tsx:86-92`) shows "No workspaces yet", a page with no header. They could reach Governance only by typing the URL, which defeats the point of SP2. Send them to the app they can enter instead.

**Files:**
- Modify: `frontend/src/App.tsx` (`HomeRedirect`, lines 86-92)
- Test: `frontend/src/__tests__/governanceRouting.test.tsx` (from Task 4)

**Interfaces:**
- Consumes: `canEnterGovernance`, `canEnterControlPlane` (Task 1).

- [ ] **Step 1: Write the failing test**

Append inside `describe("Governance routing in App", …)` in `frontend/src/__tests__/governanceRouting.test.tsx` (the `openApp` stub rejects `/api/workspaces/mine`, so `useMyWorkspaces` yields no data, which is the no-workspace branch):

```tsx
  it("sends a user with no workspace but Governance access from / to Governance", async () => {
    openApp("/", GOVERNANCE_ONLY);
    await waitFor(() => expect(currentUrl()).toBe("/governance/policies"), TIMEOUT);
  }, 15_000);

  it("sends a user with no workspace but only Control Plane access from / to Control Plane", async () => {
    openApp("/", INFRA_ONLY);
    await waitFor(() => expect(currentUrl().startsWith("/control-plane")).toBe(true), TIMEOUT);
  }, 15_000);

  it("still tells a plain user with no workspace to ask an admin", async () => {
    openApp("/", me([]));
    expect(await screen.findByText(/No workspaces yet/, {}, TIMEOUT)).toBeInTheDocument();
    expect(currentUrl()).toBe("/");
  }, 15_000);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd frontend && npx vitest run src/__tests__/governanceRouting.test.tsx`
Expected: FAIL — the first two stay on `/` showing "No workspaces yet".

- [ ] **Step 3: Implement**

In `frontend/src/App.tsx`, add `canEnterControlPlane, canEnterGovernance` to the `@/auth/appAccess` import (create the import line if Task 4 did not add one), then replace the last line of `HomeRedirect`:

```tsx
  if (data && data.length > 0) return <Navigate to={`/explore/${data[0].slug}`} replace />;
  // No workspace yet. A steward or infra operator still has somewhere to go —
  // this page renders no header, so without the redirect they would be stranded.
  if (canEnterGovernance(me)) return <Navigate to="/governance" replace />;
  if (canEnterControlPlane(me)) return <Navigate to="/control-plane" replace />;
  return <div className="p-10">No workspaces yet. Ask an admin to add you to one.</div>;
```

Check first whether `useMyWorkspaces` is still loading when `data` is `undefined` (read its hook); if it exposes `isLoading`, guard the two new redirects with `!wsLoading` so a member with workspaces is not bounced to Governance during the fetch, and add a test for that (user with `policy:read` whose `/api/workspaces/mine` resolves to one workspace lands on `/explore/<slug>`). Use the real endpoint path from the hook in the fetch stub.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd frontend && npx vitest run src/__tests__/governanceRouting.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/App.tsx frontend/src/__tests__/governanceRouting.test.tsx
git commit -m "feat(governance): land workspace-less stewards in the app they can enter

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Move the pages and rewrite their links

**Files:**
- Move: `frontend/src/routes/control-plane/{Policies,ProfileDetailPage,PolicyEditorPage}.tsx` → `frontend/src/routes/governance/`
- Move: `frontend/src/routes/control-plane/governance/{Classifications,TagDetail}.tsx` → `frontend/src/routes/governance/`
- Move: `frontend/src/routes/control-plane/__tests__/{Policies.test.tsx,ProfileDetailPage.test.tsx,PolicyEditorPage.test.tsx,defaultResourcesFromDef.test.ts,enrichResourceDefs.test.ts}` → `frontend/src/routes/governance/__tests__/`
- Move: `frontend/src/routes/control-plane/governance/__tests__/{Classifications,TagDetail}.test.tsx` → `frontend/src/routes/governance/__tests__/`
- Modify: `frontend/src/App.tsx` (5 import lines), `frontend/src/routes/control-plane/access/AccessOverview.tsx:61`, `frontend/src/routes/control-plane/AuditLog.tsx:43`, `frontend/src/components/access/TagEditor.tsx:147`, `frontend/src/components/access/CatalogTagEditor.tsx:135`, `frontend/src/components/access/__tests__/TagEditor.test.tsx:88`, `frontend/src/components/access/__tests__/CatalogTagEditor.test.tsx:165`

**Interfaces:**
- Consumes: routes from Task 4.
- Produces: default exports unchanged, now at `@/routes/governance/Policies`, `@/routes/governance/ProfileDetailPage`, `@/routes/governance/PolicyEditorPage` (also named `defaultResourcesFromDef`, `enrichResourceDefs`), `@/routes/governance/Classifications`, `@/routes/governance/TagDetail`; `profilesQueryKey` still exported from `Policies`.

**Every in-app hit of `/control-plane/(policies|classifications|data-lineage)` in `frontend/src` (grep at `48f91c03`) and its disposition:**

| Hit | Kind | Change |
|---|---|---|
| `routes/control-plane/Policies.tsx:107` | app link | → `/governance/policies/${…}` |
| `routes/control-plane/ProfileDetailPage.tsx:77` | back link | → `/governance/policies` |
| `routes/control-plane/ProfileDetailPage.tsx:191` | `newHref` (tag tab) | → `/governance/policies/${…}/access/new?scope=tag` |
| `routes/control-plane/ProfileDetailPage.tsx:276` | `navigate` New policy | → `/governance/policies/${…}/${type ?? "access"}/new` |
| `routes/control-plane/ProfileDetailPage.tsx:281` | `editPath` | → `/governance/policies/${…}/${rowType}/${id}` |
| `routes/control-plane/PolicyEditorPage.tsx:372` | `profileUrl` (back link + 3 navigates) | → `/governance/policies/${…}` |
| `routes/control-plane/governance/Classifications.tsx:106,181` | row link, Edit menu | → `/governance/classifications/${t.id}` |
| `routes/control-plane/governance/TagDetail.tsx:332,346` | navigate after delete, back link | → `/governance/classifications` |
| `routes/control-plane/access/AccessOverview.tsx:61` | "Data plane" card | → `/governance/policies` |
| `routes/control-plane/__tests__/Policies.test.tsx:50` | test | → `/governance/policies/trino` |
| `routes/control-plane/__tests__/ProfileDetailPage.test.tsx:76` | test | → `/governance/policies/trino-prod/access/new` |
| `routes/control-plane/__tests__/PolicyEditorPage.test.tsx:135` | test | → `/governance/policies/trino` |
| `components/layout/__tests__/Sidebar.test.tsx:319-342` | test | rewritten in Task 6 |
| `routes/control-plane/access/AccessOverview.tsx:446` `/control-plane/audit-log` | app link, stays in CP | none |
| `routes/control-plane/AuditLog.tsx:46` `GET /control-plane/audit` | API path in a docstring | none |
| `api/policies.ts:91`, `api/generated/policy-admin/*`, `api/generated/control-plane-audit/*` | backend API paths | **do not touch** |

The moved pages contain **no relative navigations** (no `navigate("..")`, no `to="policies"` without a leading `/`): every `Link`/`navigate` is an absolute literal, listed above. `Policies.tsx:232` links `/control-plane/engines` ("Provision an engine") and stays. No Playwright spec under `frontend/tests/e2e/` opens a moved URL (only `/api/control-plane/policies*` API calls, which are unchanged). All moved tests render with a bare `<MemoryRouter>` (no `initialEntries`) and mock `useParams`/`useNavigate` where needed, so the move does not change what they render.

- [ ] **Step 1: Move files with git**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-governance-split/frontend/src/routes
mkdir -p governance/__tests__
git mv control-plane/Policies.tsx governance/Policies.tsx
git mv control-plane/ProfileDetailPage.tsx governance/ProfileDetailPage.tsx
git mv control-plane/PolicyEditorPage.tsx governance/PolicyEditorPage.tsx
git mv control-plane/governance/Classifications.tsx governance/Classifications.tsx
git mv control-plane/governance/TagDetail.tsx governance/TagDetail.tsx
git mv control-plane/__tests__/Policies.test.tsx governance/__tests__/Policies.test.tsx
git mv control-plane/__tests__/ProfileDetailPage.test.tsx governance/__tests__/ProfileDetailPage.test.tsx
git mv control-plane/__tests__/PolicyEditorPage.test.tsx governance/__tests__/PolicyEditorPage.test.tsx
git mv control-plane/__tests__/defaultResourcesFromDef.test.ts governance/__tests__/defaultResourcesFromDef.test.ts
git mv control-plane/__tests__/enrichResourceDefs.test.ts governance/__tests__/enrichResourceDefs.test.ts
git mv control-plane/governance/__tests__/Classifications.test.tsx governance/__tests__/Classifications.test.tsx
git mv control-plane/governance/__tests__/TagDetail.test.tsx governance/__tests__/TagDetail.test.tsx
ls control-plane/governance 2>/dev/null || echo "old governance dir gone"
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-governance-split
```

Expected: last line prints `old governance dir gone` (git leaves no empty directory once both subtrees are moved; if `ls` lists an empty `__tests__`, `rmdir control-plane/governance/__tests__ control-plane/governance`).

- [ ] **Step 2: Fix imports broken by the move**

`frontend/src/routes/governance/PolicyEditorPage.tsx` line 42:

```tsx
import { FieldLabel } from "./config/FieldLabel";
```
→
```tsx
import { FieldLabel } from "@/routes/control-plane/config/FieldLabel";
```

`frontend/src/routes/governance/__tests__/Classifications.test.tsx` line 7:
`import Classifications from "@/routes/control-plane/governance/Classifications";` → `import Classifications from "@/routes/governance/Classifications";`

`frontend/src/routes/governance/__tests__/TagDetail.test.tsx` line 7:
`import TagDetail from "@/routes/control-plane/governance/TagDetail";` → `import TagDetail from "@/routes/governance/TagDetail";`

`frontend/src/App.tsx` — replace the five page imports:

```tsx
import Policies from "@/routes/control-plane/Policies";
import ProfileDetailPage from "@/routes/control-plane/ProfileDetailPage";
import PolicyEditorPage from "@/routes/control-plane/PolicyEditorPage";
```
→
```tsx
import Policies from "@/routes/governance/Policies";
import ProfileDetailPage from "@/routes/governance/ProfileDetailPage";
import PolicyEditorPage from "@/routes/governance/PolicyEditorPage";
```
and
```tsx
import Classifications from "@/routes/control-plane/governance/Classifications";
import TagDetail from "@/routes/control-plane/governance/TagDetail";
```
→
```tsx
import Classifications from "@/routes/governance/Classifications";
import TagDetail from "@/routes/governance/TagDetail";
```

(The relative `../Policies`, `../ProfileDetailPage`, `../PolicyEditorPage` imports in the moved tests still resolve — page and test moved together.)

- [ ] **Step 3: Confirm the pure move is green, then commit it alone**

Run: `cd frontend && npm run typecheck && npx vitest run src/routes/governance src/__tests__/governanceRouting.test.tsx`
Expected: typecheck exit 0; all tests PASS (links still point at `/control-plane/...`, which Task 4 redirects).

```bash
git add -A frontend/src/routes frontend/src/App.tsx
git status --short
git commit -m "refactor(governance): move governance pages to routes/governance

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

Expected from `git status --short` before committing: 12 `R` rows (the moved files, three of them with the import edits above) and `M  frontend/src/App.tsx`; nothing else.

- [ ] **Step 4: Write the failing link tests**

`frontend/src/routes/governance/__tests__/Policies.test.tsx` line 50:
`expect(link).toHaveAttribute("href", "/control-plane/policies/trino");` → `expect(link).toHaveAttribute("href", "/governance/policies/trino");`

`frontend/src/routes/governance/__tests__/ProfileDetailPage.test.tsx` line 76:
`expect(navigate).toHaveBeenCalledWith("/control-plane/policies/trino-prod/access/new");` → `expect(navigate).toHaveBeenCalledWith("/governance/policies/trino-prod/access/new");`

and add inside `describe("ProfileDetailPage", …)` after the first test:

```tsx
  it("links back to the Governance policies landing", async () => {
    wrap(<ProfileDetailPage />);
    await screen.findByText("allow-analysts");
    expect(screen.getByRole("link", { name: "Policies" })).toHaveAttribute("href", "/governance/policies");
  });
```

`frontend/src/routes/governance/__tests__/PolicyEditorPage.test.tsx` line 135:
`expect(navigate).toHaveBeenCalledWith("/control-plane/policies/trino"),` → `expect(navigate).toHaveBeenCalledWith("/governance/policies/trino"),`

`frontend/src/routes/governance/__tests__/Classifications.test.tsx` — add inside `describe("Classifications", …)` after the first test:

```tsx
  test("links each tag name to its Governance detail page", async () => {
    renderPage();
    expect(await screen.findByRole("link", { name: "PII" })).toHaveAttribute("href", "/governance/classifications/t1");
  });
```

`frontend/src/routes/governance/__tests__/TagDetail.test.tsx` — add inside `describe("TagDetail", …)` after the first test:

```tsx
  test("links back to Governance → Classifications", async () => {
    renderPage();
    await screen.findByRole("heading", { name: "PII" });
    expect(screen.getByRole("link", { name: "Classifications" })).toHaveAttribute("href", "/governance/classifications");
  });
```

`frontend/src/components/access/__tests__/TagEditor.test.tsx` line 88 and `frontend/src/components/access/__tests__/CatalogTagEditor.test.tsx` line 165:
`screen.getByText(/create them in Control Plane.*Classifications/i),` → `screen.getByText(/create them in Governance.*Classifications/i),`

- [ ] **Step 5: Run tests to verify they fail**

Run: `cd frontend && npx vitest run src/routes/governance src/components/access/__tests__/TagEditor.test.tsx src/components/access/__tests__/CatalogTagEditor.test.tsx`
Expected: FAIL — 8 failures: six link/navigate assertions receive `/control-plane/...` (Policies href, ProfileDetailPage navigate + back link, PolicyEditorPage navigate, Classifications link, TagDetail back link), and the two copy assertions cannot find "create them in Governance".

- [ ] **Step 6: Rewrite the links and copy**

Path literals (the only `/control-plane/policies` and `/control-plane/classifications` strings in these files are the app links listed in the table above):

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-governance-split/frontend
sed -i '' -e 's#/control-plane/policies#/governance/policies#g' -e 's#/control-plane/classifications#/governance/classifications#g' \
  src/routes/governance/Policies.tsx \
  src/routes/governance/ProfileDetailPage.tsx \
  src/routes/governance/PolicyEditorPage.tsx \
  src/routes/governance/Classifications.tsx \
  src/routes/governance/TagDetail.tsx \
  src/routes/control-plane/access/AccessOverview.tsx
git diff --stat
```

Expected `git diff --stat` for those six files: `Policies.tsx | 2`, `ProfileDetailPage.tsx | 8`, `PolicyEditorPage.tsx | 2`, `Classifications.tsx | 4`, `TagDetail.tsx | 4`, `AccessOverview.tsx | 2` (plus the test edits from Step 4).

Copy — `frontend/src/components/access/TagEditor.tsx:147` and `frontend/src/components/access/CatalogTagEditor.tsx:135`:
`          No tags defined — create them in Control Plane → Classifications.` → `          No tags defined — create them in Governance → Classifications.`

Docstrings:
- `routes/governance/Policies.tsx:51` ` * Control Plane → Data governance → Policies (drill-down L1: profiles landing).` → ` * Governance → Policies (drill-down L1: profiles landing).`
- `routes/governance/ProfileDetailPage.tsx:35` ` * Control Plane → Policy profile detail (drill-down L2).` → ` * Governance → Policies → a profile (drill-down L2).`
- `routes/governance/PolicyEditorPage.tsx:269` ` * Control Plane → full-page policy editor. Replaces the three modal dialogs` → ` * Governance → Policies → full-page policy editor. Replaces the three modal dialogs`
- `routes/governance/Classifications.tsx:62` ` * Control Plane → Data Governance → Classifications.` → ` * Governance → Classifications.`
- `routes/governance/TagDetail.tsx:44` ` * Control Plane → Data Governance → Classifications → a single tag.` → ` * Governance → Classifications → a single tag.`
- `routes/control-plane/AuditLog.tsx:43` ` * Control Plane → Data Governance → Audit log.` → ` * Control Plane → System → Audit log.`

- [ ] **Step 7: Run tests to verify they pass, and grep for leftovers**

Run: `cd frontend && npx vitest run src/routes/governance src/components/access src/routes/control-plane/access src/__tests__/governanceRouting.test.tsx`
Expected: PASS.

Run (no pipe — `rtk` rewrites piped greps):

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-governance-split
git grep -nE "/control-plane/(policies|classifications|data-lineage)" -- frontend/src ':!frontend/src/api'
git grep -nE "Control Plane → (Classifications|Data [Gg]overnance)" -- frontend/src
```

Expected first command — exactly these, all intentional: `frontend/src/App.tsx` (3 legacy `<Route path=…>`), `frontend/src/routes/governance/__tests__/LegacyGovernanceRedirect.test.tsx`, `frontend/src/__tests__/governanceRouting.test.tsx`, and `frontend/src/components/layout/__tests__/Sidebar.test.tsx` lines 319-342 (fixed in Task 6). Expected second command: no output.

- [ ] **Step 8: Commit**

```bash
git add -A frontend/src
git commit -m "feat(governance): point governance links and copy at /governance

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Sidebar Governance mode and System group

**Files:**
- Modify: `frontend/src/components/layout/Sidebar.tsx` (imports 1-17; `NavItem` docstring 19-22; comment + `CONTROL_PLANE_GROUPS` 57-103; `Sidebar` body 125-150)
- Test: `frontend/src/components/layout/__tests__/Sidebar.test.tsx`

**Interfaces:**
- Consumes: `appForPath` from Task 1.
- Produces: `GOVERNANCE_GROUPS` (module-private). Rendered links: `/governance/policies` ("Policies", `policy:read`), `/governance/classifications` ("Classifications", `tag:read`), `/governance/data-lineage` ("Data Lineage", admin-only); Control Plane "System" group with `/control-plane/audit-log` ("Audit log", admin-only).

- [ ] **Step 1: Write the failing tests**

In `frontend/src/components/layout/__tests__/Sidebar.test.tsx`:

Line 155 comment `// Data Lineage is no longer a workspace item (moved to the Control Plane).` → `// Data Lineage is not a workspace item (it lives in the Governance app).`

Replace the whole `describe("Sidebar – Control Plane Data Governance group", …)` block (lines 318-345) with:

```tsx
describe("Sidebar – Control Plane System group", () => {
  test("Audit log sits under System and the governance items are gone from the Control Plane", () => {
    renderSidebar("/control-plane/workspaces");
    const auditLog = screen.getByRole("link", { name: "Audit log" });
    // Audit log has no `perm` → platform_admin-only, slugified to `audit-log`.
    expect(auditLog).toHaveAttribute("href", "/control-plane/audit-log");
    expect(screen.getByRole("button", { name: /^System$/i }).parentElement).toContainElement(auditLog);

    // The link above proves this is the Control Plane sidebar, so these absences mean something.
    for (const moved of ["Policies", "Classifications", "Data Lineage"]) {
      expect(screen.queryByRole("link", { name: moved })).not.toBeInTheDocument();
    }
    expect(screen.queryByText(/data governance/i)).not.toBeInTheDocument();
  });
});

describe("Sidebar – Governance app", () => {
  const groupOf = (heading: string) =>
    screen.getByRole("button", { name: new RegExp(`^${heading}$`, "i") }).parentElement;

  test("renders Policies, Classification and Lineage groups at /governance routes", () => {
    renderSidebar("/governance/policies");
    const policies = screen.getByRole("link", { name: "Policies" });
    const classifications = screen.getByRole("link", { name: "Classifications" });
    const lineage = screen.getByRole("link", { name: "Data Lineage" });

    expect(policies).toHaveAttribute("href", "/governance/policies");
    expect(classifications).toHaveAttribute("href", "/governance/classifications");
    expect(lineage).toHaveAttribute("href", "/governance/data-lineage");

    expect(groupOf("Policies")).toContainElement(policies);
    expect(groupOf("Classification")).toContainElement(classifications);
    expect(groupOf("Lineage")).toContainElement(lineage);
  });

  test("renders neither Control Plane nor Explore items", () => {
    renderSidebar("/governance/classifications");
    expect(screen.getByRole("link", { name: "Classifications" })).toBeInTheDocument();
    for (const label of ["Engines", "Users", "Audit log", "Workspaces", "SQL Lab", "Workspace settings"]) {
      expect(screen.queryByRole("link", { name: label })).not.toBeInTheDocument();
    }
  });

  test("a tag-only user sees Classifications and nothing else", () => {
    renderSidebar("/governance/classifications", {
      user_id: "4",
      email: "tags@example.com",
      platform_roles: [],
      permissions: ["tag:read"],
    });
    expect(screen.getByRole("link", { name: "Classifications" })).toHaveAttribute(
      "href",
      "/governance/classifications",
    );
    expect(screen.queryByRole("link", { name: "Policies" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Data Lineage" })).not.toBeInTheDocument();
    // Groups left with no visible items are dropped, heading included.
    expect(screen.queryByRole("button", { name: /^Policies$/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Lineage$/i })).not.toBeInTheDocument();
  });

  test("a policy-only user sees Policies and not Classifications", () => {
    renderSidebar("/governance/policies", {
      user_id: "5",
      email: "policies@example.com",
      platform_roles: [],
      permissions: ["policy:read"],
    });
    expect(screen.getByRole("link", { name: "Policies" })).toHaveAttribute("href", "/governance/policies");
    expect(screen.queryByRole("link", { name: "Classifications" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /^Classification$/i })).not.toBeInTheDocument();
  });
});
```

In the docblock above `describe("Sidebar – no Access heading on an Explore route", …)` (line 245), change `` hard either/or on `isAdmin` — control-plane groups OR Explore groups, never `` to `` hard either/or on the app (`appForPath`) — permission groups OR Explore groups, never ``. (Task 7's leftover grep for `isAdmin` would otherwise hit this comment.)

In `describe("Sidebar – permission-based filtering", …)`, replace the first test (`"a scoped (non platform_admin) user with only policy:read …"`, lines 348-370) with:

```tsx
  test("a scoped (non platform_admin) user with only engine:read sees Engines and nothing gated elsewhere", () => {
    renderSidebar("/control-plane/engines", {
      user_id: "2",
      email: "scoped@example.com",
      platform_roles: [],
      permissions: ["engine:read"],
    });

    expect(screen.getByRole("link", { name: "Engines" })).toBeInTheDocument();
    for (const label of ["Users", "Roles", "Workspaces", "Secrets", "Audit log"]) {
      expect(screen.queryByRole("link", { name: label })).not.toBeInTheDocument();
    }

    // Groups with no visible items are dropped entirely, heading included.
    expect(screen.queryByText("Identity")).not.toBeInTheDocument();
    expect(screen.queryByText("Access")).not.toBeInTheDocument();
    // System holds only the admin-only Audit log.
    expect(screen.queryByText("System")).not.toBeInTheDocument();
    expect(screen.getByText("Infrastructure")).toBeInTheDocument();
  });
```

In the next test (`"a platform_admin sees every Control Plane item …"`), replace the label list with:

```tsx
    for (const label of [
      "Access overview",
      "Users",
      "Groups",
      "Roles",
      "Access Explorer",
      "Permissions",
      "Workspaces",
      "Engines",
      "Warehouses",
      "Storage",
      "Secrets",
      "Audit log",
    ]) {
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd frontend && npx vitest run src/components/layout/__tests__/Sidebar.test.tsx`
Expected: FAIL — the System test (Policies link still in Control Plane, no System heading), all four Governance tests (Explore sidebar renders on `/governance`), and the engine:read test (`System`/`Data governance` present).

- [ ] **Step 3: Implement**

In `frontend/src/components/layout/Sidebar.tsx`, after `import { useAuth } from "@/auth/useAuth";` add:

```tsx
import { appForPath } from "@/auth/appAccess";
```

Replace the `NavItem` docstring (lines 19-22):

```tsx
/** `perm` is the fine-grained read permission that unlocks this item in the
 * Control Plane or Governance; items with no `perm` (e.g. Workspaces, Audit log)
 * are platform_admin-only. `feature` is the workspace entitlement key (e.g.
 * `notebooks`) that unlocks an Explore item; items with no `feature` are
 * unaffected by entitlements. */
```

Replace the comment above `CONTROL_PLANE_GROUPS` and the whole array (lines 57-103) with:

```tsx
// Governance = the data-plane rules a data steward manages (SP2: split out of the
// Control Plane). Headings must stay distinct from every other app's headings —
// collapsed-group state is one localStorage key across all three apps.
const GOVERNANCE_GROUPS: NavGroup[] = [
  {
    heading: "Policies",
    items: [{ label: "Policies", icon: ShieldCheck, perm: "policy:read", to: "policies" }],
  },
  {
    heading: "Classification",
    items: [{ label: "Classifications", icon: Tags, perm: "tag:read", to: "classifications" }],
  },
  {
    heading: "Lineage",
    // Platform-wide (cross-workspace), so it lives here rather than inside a single
    // workspace. Admin-only until the real feature + its permission land.
    items: [{ label: "Data Lineage", icon: Share2, to: "data-lineage" }],
  },
];

// Control Plane = system administration. Grouped by a mental model, not a feature
// list: Overview (posture hub) → Identity (who) → Access (what they can do/see) →
// Infrastructure (the platform itself) → System (the platform's own records).
const CONTROL_PLANE_GROUPS: NavGroup[] = [
  {
    heading: "Overview",
    items: [{ label: "Access overview", icon: Gauge, perm: "user:read", to: "access-overview" }],
  },
  {
    heading: "Identity",
    items: [
      { label: "Users", icon: Users, perm: "user:read" },
      { label: "Groups", icon: UsersRound, perm: "group:read" },
      { label: "Roles", icon: UserCog, perm: "role:read" },
    ],
  },
  {
    heading: "Access",
    items: [
      { label: "Access Explorer", icon: Compass, perm: "role:read", to: "access-explorer" },
      { label: "Permissions", icon: ListChecks, perm: "role:read" },
    ],
  },
  {
    heading: "Infrastructure",
    items: [
      { label: "Workspaces", icon: FolderKanban },
      { label: "Engines", icon: Server, perm: "engine:read" },
      { label: "Warehouses", icon: Boxes, perm: "warehouse:read" },
      { label: "Storage", icon: HardDrive, perm: "storage:read" },
      { label: "Secrets", icon: KeyRound, perm: "secret:read" },
    ],
  },
  {
    heading: "System",
    // API-layer authorization decisions and admin mutations — a system log, not a
    // data access log, so it stays here and not in Governance.
    items: [{ label: "Audit log", icon: ScrollText }],
  },
];
```

In `Sidebar`, replace from `const isAdmin = useLocation().pathname.startsWith("/control-plane");` through `const base = isAdmin ? "/control-plane" : \`/explore/${workspace ?? ""}\`;` (lines 126-150) with:

```tsx
  const app = appForPath(useLocation().pathname);
  const { workspace } = useParams();
  const { me } = useAuth();
  const isPlatformAdmin = me?.platform_roles.includes("platform_admin") ?? false;
  const permissions = me?.permissions ?? [];
  const enabledFeatures = useWorkspaceFeatures();

  // Governance and Control Plane items are permission-gated: platform_admin sees
  // everything, everyone else sees only items whose `perm` they hold. Items with
  // no `perm` are platform_admin-only. Explore items are entitlement-gated: an item
  // with a `feature` only shows when the workspace has it switched on
  // (entitlements apply to everyone, admins included — no bypass). Either way,
  // groups left with no visible items are dropped entirely (heading included).
  const permissionGroups =
    app === "governance" ? GOVERNANCE_GROUPS : app === "control-plane" ? CONTROL_PLANE_GROUPS : null;
  const groups = permissionGroups
    ? permissionGroups.map((group) => ({
        ...group,
        items: group.items.filter(
          (item) => isPlatformAdmin || (item.perm !== undefined && permissions.includes(item.perm)),
        ),
      })).filter((group) => group.items.length > 0)
    : EXPLORE_GROUPS.map((group) => ({
        ...group,
        items: group.items.filter((item) => !item.feature || enabledFeatures.includes(item.feature)),
      })).filter((group) => group.items.length > 0);
  const base =
    app === "governance" ? "/governance" : app === "control-plane" ? "/control-plane" : `/explore/${workspace ?? ""}`;
```

(The lucide import list is unchanged — `ShieldCheck`, `Tags`, `Share2`, `ScrollText` are all still used.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd frontend && npx vitest run src/components/layout`
Expected: PASS (Sidebar, SidebarCollapse, AppShell.responsive and the Header files — Header is untouched so far).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/layout/Sidebar.tsx frontend/src/components/layout/__tests__/Sidebar.test.tsx
git commit -m "feat(layout): add Governance sidebar and Control Plane System group

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Header app switcher

**Files:**
- Modify: `frontend/src/components/layout/Header.tsx`
- Test: `frontend/src/components/layout/__tests__/PortalToggle.test.tsx`, `frontend/src/components/layout/__tests__/Header.responsive.test.tsx`

**Interfaces:**
- Consumes: `appForPath`, `canEnterControlPlane`, `canEnterGovernance` (Task 1).
- Produces: header entries — button `"Explorer"`, link `"Governance"` → `/governance`, link `"Control Plane"` → `/control-plane`; the active one has `aria-current="page"`.

- [ ] **Step 1: Write the failing tests**

In `frontend/src/components/layout/__tests__/PortalToggle.test.tsx`, replace the header comment line ` * Tests the two-way Admin↔Explorer toggle rendered inside Header.` with ` * Tests the Explorer / Governance / Control Plane app switcher rendered inside Header.`, then replace both `describe` blocks (from `describe("PortalToggle – Control Plane portal", …` to the end of the file) with:

```tsx
const withPerms = (permissions: string[]): Me => ({ ...ME_WITH_ROLE, platform_roles: [], permissions });

describe("App switcher – which entries show", () => {
  test("platform_admin sees Explorer, Governance and Control Plane", () => {
    renderHeader({ initialPath: "/explore/my-workspace" });
    expect(screen.getByRole("button", { name: "Explorer" })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Governance" })).toHaveAttribute("href", "/governance");
    expect(screen.getByRole("link", { name: "Control Plane" })).toHaveAttribute("href", "/control-plane");
  });

  test("a policy-only user sees Governance but not Control Plane", () => {
    renderHeader({ initialPath: "/explore/my-workspace", me: withPerms(["policy:read"]) });
    expect(screen.getByRole("link", { name: "Governance" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Control Plane" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Explorer" })).toBeInTheDocument();
  });

  test("a tag-only user sees Governance", () => {
    renderHeader({ initialPath: "/explore/my-workspace", me: withPerms(["tag:read"]) });
    expect(screen.getByRole("link", { name: "Governance" })).toBeInTheDocument();
  });

  test("an infra-only user sees Control Plane but not Governance", () => {
    renderHeader({ initialPath: "/explore/my-workspace", me: withPerms(["engine:read"]) });
    expect(screen.getByRole("link", { name: "Control Plane" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Governance" })).not.toBeInTheDocument();
  });

  test("a user with neither app sees no switcher at all", () => {
    renderHeader({ initialPath: "/explore/my-workspace", me: withPerms([]) });
    expect(screen.queryByRole("link", { name: "Control Plane" })).not.toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Governance" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Explorer" })).not.toBeInTheDocument();
  });
});

describe("App switcher – active entry follows the URL", () => {
  test.each([
    ["/explore/my-workspace", "Explorer"],
    ["/governance/policies", "Governance"],
    ["/control-plane/users", "Control Plane"],
  ])("on %s only %s is current", (initialPath, active) => {
    renderHeader({ initialPath });
    const entries = {
      Explorer: screen.getByRole("button", { name: "Explorer" }),
      Governance: screen.getByRole("link", { name: "Governance" }),
      "Control Plane": screen.getByRole("link", { name: "Control Plane" }),
    };
    for (const [name, el] of Object.entries(entries)) {
      if (name === active) expect(el).toHaveAttribute("aria-current", "page");
      else expect(el).not.toHaveAttribute("aria-current");
    }
  });

  test("the workspace switcher shows only in Explorer", () => {
    renderHeader({ initialPath: "/governance/policies" });
    expect(screen.queryByLabelText("Workspace")).not.toBeInTheDocument();
  });
});

describe("App switcher – Explorer navigation", () => {
  test("Explorer navigates to the stored workspace from Governance", async () => {
    localStorage.setItem("egret:last-workspace", "my-workspace");
    const user = userEvent.setup();
    renderHeader({ initialPath: "/governance/policies" });

    await user.click(screen.getByRole("button", { name: /explorer/i }));

    expect(screen.getByTestId("loc").textContent).toBe("/explore/my-workspace");
    localStorage.removeItem("egret:last-workspace");
  });

  test("Explorer navigates to the stored workspace from Control Plane", async () => {
    localStorage.setItem("egret:last-workspace", "my-workspace");
    const user = userEvent.setup();
    renderHeader({ initialPath: "/control-plane" });

    await user.click(screen.getByRole("button", { name: /explorer/i }));

    expect(screen.getByTestId("loc").textContent).toBe("/explore/my-workspace");
    localStorage.removeItem("egret:last-workspace");
  });

  test("Explorer falls back to / when no stored workspace", async () => {
    localStorage.removeItem("egret:last-workspace");
    const user = userEvent.setup();
    renderHeader({ initialPath: "/control-plane" });

    await user.click(screen.getByRole("button", { name: /explorer/i }));

    expect(screen.getByTestId("loc").textContent).toBe("/");
  });

  test("visiting /explore/:workspace persists workspace to localStorage", async () => {
    renderHeader({ initialPath: "/explore/sales-team" });
    // useEffect fires after render; give it a tick
    await new Promise((r) => setTimeout(r, 0));
    expect(localStorage.getItem("egret:last-workspace")).toBe("sales-team");
    localStorage.removeItem("egret:last-workspace");
  });
});
```

In `frontend/src/components/layout/__tests__/Header.responsive.test.tsx`, in `"drops the wordmark and the Control Plane label, keeping both reachable"`, after `expect(screen.getByRole("link", { name: "Control Plane" })).toBeInTheDocument();` add:

```tsx
    // Same for the other two switcher entries.
    expect(screen.queryByText("Governance")).toBeNull();
    expect(screen.getByRole("link", { name: "Governance" })).toBeInTheDocument();
    expect(screen.queryByText("Explorer")).toBeNull();
    expect(screen.getByRole("button", { name: "Explorer" })).toBeInTheDocument();
```

and in `"keeps the desktop row exactly as it was"`, after `expect(screen.getByText("Control Plane")).toBeInTheDocument();` add:

```tsx
    expect(screen.getByText("Governance")).toBeInTheDocument();
    expect(screen.getByText("Explorer")).toBeInTheDocument();
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd frontend && npx vitest run src/components/layout/__tests__/PortalToggle.test.tsx src/components/layout/__tests__/Header.responsive.test.tsx`
Expected: FAIL — no `Governance` link anywhere; `Explorer` absent on Explore routes; tag-only user sees nothing; no `aria-current`.

- [ ] **Step 3: Implement**

In `frontend/src/components/layout/Header.tsx`:

Line 3 imports: `import { Compass, Feather, LogOut, Moon, Settings, Sun } from "lucide-react";` → `import { Compass, Feather, LogOut, Moon, Settings, ShieldCheck, Sun } from "lucide-react";`

After `import { useAuth } from "@/auth/useAuth";` add:

```tsx
import { type AppArea, appForPath, canEnterControlPlane, canEnterGovernance } from "@/auth/appAccess";
```

Replace from `const isAdmin = location.pathname.startsWith("/control-plane");` through the end of the `canAdmin` expression (lines 37-51) with:

```tsx
  const app = appForPath(location.pathname);
  // Same source of truth as the sidebar rule — one viewport breakpoint for the whole shell.
  // At 390px the row below is 441px of fixed-width children, which is what puts a horizontal
  // scrollbar on the document no matter how narrow the dashboard grid itself gets.
  const narrow = useIsNarrowViewport();
  // Governance and Control Plane each show for platform_admin OR anyone holding one of that
  // app's read permissions — the same predicates as RequireGovernanceAccess /
  // RequireControlPlaneAccess. Explorer is shown beside them so the three read as one
  // switcher; with neither app reachable there is nothing to switch to, so none render.
  const showGovernance = canEnterGovernance(me);
  const showControlPlane = canEnterControlPlane(me);
  const showSwitcher = showGovernance || showControlPlane;
  const entrySize = narrow ? "icon" : "sm";
  const entryClass = narrow ? undefined : "gap-2";
  const current = (area: AppArea): "page" | undefined => (app === area ? "page" : undefined);
  const variantFor = (area: AppArea): "secondary" | "ghost" => (app === area ? "secondary" : "ghost");
```

Replace `{!isAdmin && <WorkspaceSwitcher className={narrow ? "w-auto min-w-0 flex-1" : undefined} />}` with:

```tsx
      {app === "explore" && <WorkspaceSwitcher className={narrow ? "w-auto min-w-0 flex-1" : undefined} />}
```

Replace both entry blocks — from `{canAdmin && !isAdmin && (` through the closing `)}` of the `{isAdmin && ( <Button … Explorer … )}` block (lines 92-112) — with:

```tsx
      {showSwitcher && (
        <>
          <Button
            variant={variantFor("explore")}
            size={entrySize}
            className={entryClass}
            aria-label="Explorer"
            aria-current={current("explore")}
            onClick={handleExplorer}
          >
            <Compass className="h-4 w-4" />
            {!narrow && "Explorer"}
          </Button>
          {showGovernance && (
            <Button asChild variant={variantFor("governance")} size={entrySize} className={entryClass}>
              <Link to="/governance" aria-label="Governance" aria-current={current("governance")}>
                <ShieldCheck className="h-4 w-4" />
                {!narrow && "Governance"}
              </Link>
            </Button>
          )}
          {showControlPlane && (
            <Button asChild variant={variantFor("control-plane")} size={entrySize} className={entryClass}>
              <Link to="/control-plane" aria-label="Control Plane" aria-current={current("control-plane")}>
                <Settings className="h-4 w-4" />
                {!narrow && "Control Plane"}
              </Link>
            </Button>
          )}
        </>
      )}
```

Delete the now-unused `isAdmin`/`canAdmin`/local `CONTROL_PLANE_PERMS` — they were entirely inside the replaced range; confirm with:

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-governance-split
git grep -nE "CONTROL_PLANE_PERMS|isAdmin|canAdmin" -- frontend/src/components frontend/src/auth/guards.tsx
```

Expected: no output.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd frontend && npx vitest run src/components/layout src/__tests__/governanceRouting.test.tsx`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/layout/Header.tsx frontend/src/components/layout/__tests__/PortalToggle.test.tsx frontend/src/components/layout/__tests__/Header.responsive.test.tsx
git commit -m "feat(layout): header switcher for Explorer, Governance and Control Plane

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 8: Full gate, live browser verification, PR

**Files:** none new (fixes only if a check fails — each fix gets its own failing test first, then a commit).

- [ ] **Step 1: Run the frontend gate (sequentially, nothing else running)**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-governance-split/frontend
npm run lint
npm run typecheck
npm test
npm run build
```

Expected: `lint` exits 0 with no new errors (existing warnings allowed — compare the warning count against `origin/main` if it looks higher); `typecheck` exit 0; `npm test` reports 0 failed files and 0 failed tests (if `ChartExplore.mount` "bridge viz (pie) v1 shape" times out, re-run that one file alone — known load flake, unrelated); `build` exit 0. Record the pass counts for the PR body.

- [ ] **Step 2: Start the local stack**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret
docker compose -f deploy/docker-compose.dev.yml up -d
docker stop egret-dev-frontend-1 2>/dev/null || true   # static nginx on :5173 does not proxy /api
curl -fsS http://localhost:8080/healthz || echo "backend not running"
```

If it prints `backend not running`, start it from the existing checkout (backend is unchanged by this PR, and that checkout already has `.env`, `egret.toml` and a built `target/`) in a separate terminal / background task:

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret && just serve
```

Frontend from the worktree, in another background task:

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-governance-split/frontend && npm run dev
```

Use the port Vite prints (`5173`, or `5174` if taken). Below, `B` is that origin.

- [ ] **Step 3: Create a `data_steward`-only user and confirm its permissions**

```bash
B=http://localhost:5173
J=$(mktemp -t planSP2-admin); J2=$(mktemp -t planSP2-steward)
curl -fsS -c "$J" -b "$J" -H 'content-type: application/json' \
  -d '{"email":"admin@egret.local","password":"Admin12345!"}' "$B/api/auth/login" >/dev/null
CSRF=$(awk '$6=="egret_csrf"{print $7}' "$J")
curl -sS -c "$J" -b "$J" -H 'content-type: application/json' -H "X-Egret-CSRF: $CSRF" \
  -d '{"email":"sp2-steward@egret.local","name":"SP2 steward","password":"Steward12345!"}' \
  "$B/api/control-plane/users"
USER_ID=$(curl -fsS -b "$J" "$B/api/control-plane/users" | jq -r '.[] | select(.email=="sp2-steward@egret.local") | .id')
curl -fsS -X PUT -b "$J" -H 'content-type: application/json' -H "X-Egret-CSRF: $CSRF" \
  -d '{"roles":["data_steward"]}' "$B/api/admin/users/$USER_ID/platform-roles"
curl -fsS -c "$J2" -b "$J2" -H 'content-type: application/json' \
  -d '{"email":"sp2-steward@egret.local","password":"Steward12345!"}' "$B/api/auth/login" >/dev/null
curl -fsS -b "$J2" "$B/api/auth/me" | jq '{platform_roles, permissions: (.permissions | sort)}'
curl -fsS -b "$J" "$B/api/control-plane/policies/services" | jq -r '.[].name'
```

Expected: the create call returns the user JSON (or a 409 if it already exists — fine, the lookup finds it); the PUT returns `{"ok":true}`; `/api/auth/me` shows `platform_roles: ["data_steward"]` and permissions including `engine:read`, `group:read`, `policy:read`, `role:read`, `storage:read`, `tag:manage`, `tag:read`, `user:read`, `warehouse:read`, `workspace:read` and **not** `secret:read`. If this set differs, stop and report — the browser expectations below are derived from it. The last command lists policy profile names; if it prints nothing, create one in the browser as admin (Governance → Policies → New profile, name `sp2-trino`, type `trino`). Call the name `SVC`.

(UI alternative for the role: Control Plane → Users → row actions → Assign roles → `data_steward`.) Local login is rate-limited to 5 attempts per 5 minutes per ip+email — log in once per account.

- [ ] **Step 4: Browser — platform_admin**

Log in at `$B/login` as `admin@egret.local` / `Admin12345!` (Playwright MCP `browser_navigate` / `browser_fill_form` / `browser_snapshot`, or by hand). Check each and record pass/fail:

1. On `/explore/<ws>/…` the header shows **Explorer** (current), **Governance**, **Control Plane**, and the workspace switcher.
2. Click **Governance** → URL `/governance/policies`; header Governance is current; no workspace switcher; sidebar groups **Policies / Classification / Lineage**.
3. Sidebar **Classifications** → open any tag → its **Classifications** back link returns to `/governance/classifications`.
4. **Policies** → click profile `SVC` → URL `/governance/policies/SVC` → **New policy** → URL `/governance/policies/SVC/access/new` → the back link returns to `/governance/policies/SVC`. On the profile's **Tag** tab (if the profile has one), its New button goes to `/governance/policies/SVC/access/new?scope=tag`.
5. Sidebar **Data Lineage** → `/governance/data-lineage` placeholder.
6. Click **Control Plane** → sidebar has **Overview, Identity, Access, Infrastructure, System**; **System → Audit log** opens `/control-plane/audit-log`; there is no Data governance group and no Policies/Classifications item.
7. **Access overview** → "Data plane" card → **Policies** → `/governance/policies`.
8. Old bookmark: navigate to `/control-plane/policies/SVC/access/new?scope=tag` → URL becomes `/governance/policies/SVC/access/new?scope=tag` and the editor opens in By-tag mode; browser Back does **not** land on the `/control-plane/policies/…` URL.
9. Resize to 390×844, open `/explore/<ws>/dashboards`, and evaluate `document.documentElement.scrollWidth <= window.innerWidth` → `true`; the three entries render as icons.

Log out via the avatar menu → **Log out**.

- [ ] **Step 5: Browser — data_steward only**

Log in as `sp2-steward@egret.local` / `Steward12345!`.

1. Landing is `/` → redirected to `/governance` (Task 4a), not "No workspaces yet".
2. URL becomes `/governance/policies`; header shows **Explorer**, **Governance** (current), **Control Plane**; sidebar shows **Policies** and **Classification** groups only (no Lineage).
3. `/governance/classifications` lists tags; `/governance/data-lineage` shows **403 — Forbidden**.
4. Click **Control Plane** → URL `/control-plane/engines`; sidebar shows exactly: Overview → Access overview; Identity → Users, Groups, Roles; Access → Access Explorer, Permissions; Infrastructure → Engines, Warehouses, Storage. **No** Workspaces, Secrets, or System/Audit log.
5. `/control-plane/audit-log` → **403 — Forbidden**.
6. Old bookmark `/control-plane/policies/SVC/access/new?scope=tag` → lands on `/governance/policies/SVC/access/new?scope=tag`, editor in By-tag mode, no 403.

If any check fails: stop, write a failing unit test reproducing it, fix, re-run Step 1, commit.

- [ ] **Step 6: Push and open the PR, then stop**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret-governance-split
git branch --show-current
git push -u origin feat/governance-app-split
git ls-remote --heads origin feat/governance-app-split
gh pr create --base main --head feat/governance-app-split \
  --title "feat(governance): split Data Governance into its own app" \
  --body "$(cat <<'EOF'
## Summary
- Data Governance (Policies, Classifications, Data Lineage) is a top-level app at `/governance/*` with its own header entry and sidebar.
- Audit log stays in Control Plane under a new **System** group (platform_admin only).
- One permission module (`auth/appAccess.ts`) replaces the two Control Plane lists; `tag:read` now admits a user to Governance.
- Old `/control-plane/{policies,classifications,data-lineage}` URLs redirect with subpath, query and hash preserved. The redirects are mounted outside `RequireControlPlaneAccess`, because a governance-only user fails that guard.
- Backend API paths unchanged; no client regeneration.

Spec: `docs/en/specs/2026-09-15-governance-app-split-design.md`.

## Verification
- `npm run lint`, `npm run typecheck`, `npm test` (<counts>), `npm run build` — all green locally.
- Live in the browser as platform_admin and as a `data_steward`-only user: see Task 8 checklist in `docs/en/plans/2026-09-15-governance-app-split.md`.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Expected: `git branch --show-current` prints `feat/governance-app-split`; `ls-remote` prints one ref; `gh` prints the PR URL. Replace `<counts>` with the Step 1 numbers before running. Do not merge.

---

## Self-review

- **Spec coverage.** §3.1 → Task 1 (+ Task 2 guards, Task 7 Header delete local copies). §3.2 → Task 7. §3.3 → Task 6. §3.4 routes/index/redirects/`FIRST_SECTION_BY_PERM`/landing → Tasks 2, 3, 4 (relocated per corrections 1–2). §3.5 moves + links + AuditLog docstring → Task 5. §4 unit tests: appAccess (T1), Header (T7), Sidebar governance filtering (T6), redirects preserve subpath/query (T3, T4), `RequireGovernanceAccess` allow/deny (T2), Control Plane landing for a governance-only user (T2, T4); existing page tests from new location (T5); gate + live two-account check (T8).
- **Placeholder scan.** Only `<counts>` in the PR body, filled from Step 1 output by instruction; `SVC` is defined by a command in Task 8 Step 3.
- **Name consistency.** `canEnterControlPlane`, `canEnterGovernance`, `appForPath`, `AppArea`, `RequireGovernanceAccess`, `LegacyGovernanceRedirect`, `GovernanceRoutes`, `GovernanceIndex`, `GOVERNANCE_FIRST_SECTION`, `GOVERNANCE_GROUPS` are spelled identically everywhere they appear.
