# Phase 0b — Frontend Skeleton Design Spec

> **Status:** Approved design · **Date:** 2026-06-17 · **Version:** 0.1
> Canonical (English). Vietnamese reference: [`docs/vi/specs/2026-06-17-phase-0b-frontend-skeleton-design.md`](../../vi/specs/2026-06-17-phase-0b-frontend-skeleton-design.md).
> Implements the frontend skeleton of the [Master Architecture Design](../architecture/2026-06-17-egret-architecture-design.md) §6, sitting on the shipped identity backend (1a-1/1a-2/1a-3).

---

## 1. Goal & scope

Stand up the Egret single-page-app skeleton: the dual-portal app shell, theme, authentication (local + SSO), a workspace switcher, and routing for the Admin and Explorer portals with **placeholder pages** — wired to the existing backend. No real product features (those arrive in later phases).

**In scope**
- A single Vite React app under `frontend/`, inheriting the sparklabx **shadcn/ui + Radix + Tailwind v4** design system (copied in).
- App shell — **workspace-first** layout (Layout C): header workspace switcher + Admin button + theme/user menu, contextual sidebar.
- Auth: login page (local email/password + an SSO button), session bootstrap via `/api/auth/me`, logout, CSRF header on mutations.
- Routing + guards: `/login`, `/explore/:workspace/*`, `/admin/*`, with role/membership guards and placeholder pages.
- Hand-written typed API client + TanStack Query hooks for the auth/workspace endpoints.
- One small **backend addition**: `GET /api/me/workspaces` (the principal's workspaces) — required by the workspace-first switcher.
- Dev Vite proxy to the backend; static build (nginx).

**Out of scope (deferred)**
- All real product features (databases, catalog, SQL Lab, datasets, charts, dashboards, notebooks, data API, workflows) → later phases.
- OpenAPI codegen (backend has no utoipa yet) → when the backend exposes OpenAPI.
- pnpm monorepo / shared packages → when a second consumer appears.
- Playwright e2e, i18n → future.

## 2. Decisions

| Decision | Choice |
|---|---|
| Project structure | **Single Vite app** `frontend/` (not a monorepo yet) |
| Design system | **Copy** sparklabx's `components/ui` + theme + Tailwind config + `ThemeProvider` |
| API client | **Hand-written** typed client (`fetch`, credentials + CSRF) + TanStack Query hooks |
| Shell layout | **Workspace-first (Layout C)** — prominent workspace selector in the header; Admin is a separate ⚙️ button (platform scope) |
| Skeleton scope | App shell + auth + routing with **placeholder pages**; no real features |

## 3. Architecture / directory structure

```text
frontend/
├── package.json · vite.config.ts · tsconfig.json · index.html · Dockerfile · nginx-prod.conf
├── src/
│   ├── main.tsx · App.tsx · index.css
│   ├── components/
│   │   ├── ui/                  # copied shadcn components (button, card, dialog, table, tabs, select, dropdown-menu, input, tooltip, toaster, sonner, …)
│   │   ├── theme-provider.tsx   # copied (light/dark/system, localStorage "egret-theme")
│   │   └── layout/
│   │       ├── AppShell.tsx     # header + sidebar + main + footer (Layout C)
│   │       ├── Header.tsx       # workspace switcher + Admin button + theme toggle + user menu
│   │       ├── Sidebar.tsx      # contextual nav (collapsible)
│   │       └── WorkspaceSwitcher.tsx
│   ├── routes/
│   │   ├── admin/              # placeholder pages: Databases, Catalog, Policies, Users, Workspaces, Monitoring
│   │   └── explore/           # placeholder pages: SqlLab, Datasets, Charts, Dashboards, Notebooks
│   ├── auth/
│   │   ├── AuthProvider.tsx · useAuth.ts · LoginPage.tsx · guards.tsx (RequireAuth, RequirePlatform, RequireWorkspace)
│   ├── api/
│   │   ├── client.ts          # apiFetch (credentials, CSRF header, 401 handling)
│   │   └── hooks.ts           # useMe, useLogin, useLogout, useMyWorkspaces
│   └── theme/                 # copied theme tokens
└── tests/                     # Vitest + Testing Library
```

## 4. App shell (Layout C — workspace-first)

- **Header:** `🐦 Egret` brand · **`WorkspaceSwitcher`** (dropdown of the user's workspaces; selecting navigates to `/explore/:slug/...`) · spacer · **⚙️ Admin** button (visible to users with any platform permission; routes to `/admin`) · **theme toggle** · **user menu** (email, Logout).
- **Sidebar (contextual, collapsible — inherits sparklabx):**
  - Explorer (`/explore/:workspace/*`): SQL Lab · Datasets · Charts · Dashboards · Notebooks.
  - Admin (`/admin/*`): Databases · Catalog · Policies · Users · Workspaces · Monitoring.
- **Footer:** light, `© year Egret`.
- The shell is driven by route context: the sidebar item set and the header's "current scope" derive from whether the path is under `/admin` or `/explore/:workspace`.

## 5. Routing + guards

| Path | Guard | Content |
|---|---|---|
| `/login` | none (redirects to app if already authed) | `LoginPage` |
| `/` | RequireAuth | redirect to the first workspace (`/explore/:slug`) or a "pick a workspace" view if none |
| `/explore/:workspace/*` | RequireAuth + RequireWorkspace | Explorer placeholder pages |
| `/admin/*` | RequireAuth + RequirePlatform | Admin placeholder pages |
| `*` | — | NotFound |

- **RequireAuth:** if `useMe` returns unauthenticated → redirect `/login`.
- **RequirePlatform:** principal has ≥1 platform role/permission (from `/api/auth/me`'s `platform_roles`) → else a 403 page.
- **RequireWorkspace:** the `:workspace` slug is in the user's workspaces (from `useMyWorkspaces`) → else 403. (The backend also enforces this on every API call.)
- Placeholder pages render the page title + a "Coming in a later phase" panel, using the design-system `Card`.

## 6. Auth flow + backend addition

**Frontend**
- **Bootstrap:** on load, `AuthProvider` calls `GET /api/auth/me`. 200 → authenticated (store `{ user_id, email, platform_roles }`); 401 → unauthenticated.
- **Local login:** `LoginPage` posts `POST /api/auth/login {email,password}` → on 200, refetch `me`, navigate to `/`.
- **SSO:** a "Sign in with SSO" button that does a full-page navigation to `GET /api/auth/oidc/login` (the backend 302s to the IdP and back). The button is shown only when SSO is enabled — exposed via a tiny unauthenticated `GET /api/auth/config` returning `{ oidc_enabled: bool }` (small backend addition, see below) OR hidden behind a build flag for the skeleton. **Decision: add `GET /api/auth/config`.**
- **Logout:** `POST /api/auth/logout` (with CSRF header) → clear state → `/login`.
- **CSRF:** `apiFetch` reads the `egret_csrf` cookie and sets `X-Egret-CSRF` on POST/PUT/PATCH/DELETE.

**Backend additions (small, this phase)**
1. `GET /api/me/workspaces` → the workspaces the principal can access: union of direct + group `workspace_members`, or **all active workspaces if `platform_admin`**. Returns `[{ id, slug, name }]`. Implemented in `egret-workspace` (reuses `WorkspaceRepo`/`Authorizer`), mounted on an authed `/api/me` router (needs only `Auth`, no workspace context).
2. `GET /api/auth/config` → `{ oidc_enabled: bool }` (unauthenticated) so the login page can show/hide the SSO button. Implemented in `egret-auth` (reads `config.oidc.enabled`).

## 7. API client + TanStack Query

```ts
// client.ts
export async function apiFetch(path: string, init: RequestInit = {}) {
  const method = (init.method ?? "GET").toUpperCase();
  const headers = new Headers(init.headers);
  if (["POST","PUT","PATCH","DELETE"].includes(method)) {
    const csrf = readCookie("egret_csrf");
    if (csrf) headers.set("X-Egret-CSRF", csrf);
  }
  const res = await fetch(`/api${path}`, { ...init, headers, credentials: "include" });
  if (res.status === 401) throw new UnauthorizedError();
  if (!res.ok) throw await ApiError.from(res);
  return res;
}
```
Hooks (TanStack Query): `useMe()`, `useAuthConfig()`, `useLogin()`, `useLogout()`, `useMyWorkspaces()`. `QueryClient` configured like sparklabx (no refetch-on-focus, retry: 1). 401 from any query invalidates auth → redirect to `/login`.

## 8. Dev / build

- **Vite dev proxy:** `vite.config.ts` proxies `/api` → `http://localhost:8080` (the egret backend) so cookies/CSRF work same-origin in dev.
- **Build:** static assets served by nginx (copy/adapt sparklabx's `nginx-prod.conf` + `Dockerfile`), proxying `/api` to the backend.
- Optionally add a `frontend` service to `deploy/docker-compose.dev.yml` (dev). The CI for the frontend (lint + typecheck + vitest) is a separate workflow job; wiring it is part of the plan.

## 9. Testing

- **Vitest + Testing Library** (jsdom). Key tests:
  - `AuthProvider`: `me` 200 → authenticated; 401 → unauthenticated.
  - Guards: `RequireAuth` redirects to `/login` when unauthenticated; `RequirePlatform` blocks without platform role; `RequireWorkspace` blocks a non-member slug.
  - `WorkspaceSwitcher`: selecting a workspace navigates to `/explore/:slug`.
  - `apiFetch`: attaches `X-Egret-CSRF` from the cookie on mutations; throws `UnauthorizedError` on 401.
  - Mock `fetch`/handlers (e.g. MSW or a fetch mock) — no real backend needed; tests are offline.
- Lint (eslint, inherited config) + `tsc --noEmit` typecheck in CI.

## 10. Decomposition for the plan

Suggested tasks: (1) backend `GET /api/me/workspaces` + `GET /api/auth/config` (Rust, DB-gated tests); (2) Vite app scaffold + Tailwind + tsconfig + index.css; (3) copy design system (components/ui + theme-provider + theme) and verify it builds; (4) `apiFetch` client + auth hooks + Vitest setup; (5) `AuthProvider` + `LoginPage` (local + SSO button) + `useAuthConfig`; (6) `AppShell` + `Header` + `WorkspaceSwitcher` + `Sidebar` (Layout C); (7) routing + guards + placeholder pages; (8) Vite dev proxy + Dockerfile/nginx; (9) frontend CI job (lint + typecheck + vitest); (10) final verification + manual e2e against the running backend.

## 11. References

- sparklabx frontend: `sparklabx/frontend/src/` — `App.tsx` (shell + routing + auth gating), `components/ui/*` (shadcn), `components/theme-provider.tsx`, `components/layout/{Header,Sidebar}.tsx`, `vite.config.ts`, `nginx-prod.conf`.
- Egret backend (consumed): `/api/auth/{login,logout,me,oidc/login}`, `/api/admin/workspaces`, plus the two new `/api/me/workspaces` + `/api/auth/config`.
- Architecture: [Master design](../architecture/2026-06-17-egret-architecture-design.md) §6.
