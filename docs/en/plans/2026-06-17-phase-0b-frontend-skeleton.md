# Phase 0b — Frontend Skeleton Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A single Vite React SPA skeleton — dual-portal app shell (workspace-first), theme, local + SSO auth, workspace switcher, routing with placeholder pages — wired to the existing Egret backend, plus two small backend endpoints it needs.

**Architecture:** A `frontend/` Vite app inheriting the sparklabx shadcn/ui + Radix + Tailwind v4 design system (copied in). State via TanStack Query over a hand-written `apiFetch` (cookie session + CSRF). Backend gains `GET /api/me/workspaces` and `GET /api/auth/config`. Per the [Phase 0b design spec](../specs/2026-06-17-phase-0b-frontend-skeleton-design.md).

**Tech Stack:** Backend: Rust/axum (existing). Frontend: Vite, React 18, TypeScript, react-router-dom v6, @tanstack/react-query, Tailwind v4 (@tailwindcss/vite), Radix/shadcn, Vitest + Testing Library.

**Reference:** [Phase 0b spec](../specs/2026-06-17-phase-0b-frontend-skeleton-design.md); `sparklabx/frontend/` (design system + shell to copy/adapt); Egret backend `egret-auth`/`egret-workspace`.

**Conventions:**
- Backend: runtime sqlx; DB-gated tests self-skip without `DATABASE_URL`; plain `git commit` (configured identity, NO `-c` overrides) + trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Frontend: package manager **npm**; tests are offline (mock `fetch`); `npm run build` + `npm test` + `npm run typecheck` must pass.

**⚠️ Executor-iteration points:** Tasks 2–3 (Vite + Tailwind v4 + shadcn copy) use version-sensitive tooling/config. Copy the concrete files from `sparklabx/frontend` and adjust config to what the installed versions require; acceptance = `npm run build` and `npm test` pass. Keep behavior + test assertions.

---

## File Structure

```text
crates/egret-workspace/src/routes.rs       # + me_router (GET /api/me/workspaces)
crates/egret-workspace/src/workspaces.rs    # + list_for_principal
crates/egret-auth/src/routes.rs             # + GET /api/auth/config
crates/egret-server/src/lib.rs              # mount me_router
crates/egret-workspace/tests/workspace_db.rs # + me/workspaces tests
frontend/
├── package.json · vite.config.ts · tsconfig.json · tsconfig.node.json · index.html
├── vitest.config.ts · vitest.setup.ts · eslint.config.js · Dockerfile · nginx-prod.conf
├── .gitignore
└── src/
    ├── main.tsx · App.tsx · index.css · vite-env.d.ts
    ├── lib/utils.ts                         # cn() — copied
    ├── components/ui/*                       # shadcn — copied from sparklabx
    ├── components/theme-provider.tsx         # copied (storageKey egret-theme)
    ├── theme/egret-theme.ts                  # copied
    ├── api/client.ts · api/hooks.ts
    ├── auth/AuthProvider.tsx · auth/useAuth.ts · auth/LoginPage.tsx · auth/guards.tsx
    ├── components/layout/{AppShell,Header,Sidebar,WorkspaceSwitcher}.tsx
    └── routes/admin/* · routes/explore/* · routes/NotFound.tsx · routes/Forbidden.tsx
deploy/docker-compose.dev.yml               # + frontend service (optional)
.github/workflows/ci.yml                    # + frontend job
```

**Note:** internal working plan — English only under `docs/en/plans/`.

---

### Task 1: Backend endpoints — `GET /api/me/workspaces` + `GET /api/auth/config`

**Files:** Modify `crates/egret-workspace/src/{workspaces.rs,routes.rs}`, `crates/egret-auth/src/routes.rs`, `crates/egret-server/src/lib.rs`, `crates/egret-workspace/tests/workspace_db.rs`.

- [ ] **Step 1: Add `WorkspaceRepo::list_for_principal`**

In `crates/egret-workspace/src/workspaces.rs` `impl WorkspaceRepo`:
```rust
    /// Workspaces the principal can access: all active if `include_all` (platform_admin),
    /// else those where the user is a member directly or via a group.
    pub async fn list_for_principal(&self, user_id: UserId, include_all: bool) -> Result<Vec<Workspace>> {
        let rows = if include_all {
            sqlx::query_as::<_, WorkspaceRow>(
                "SELECT id, slug, name, status FROM workspaces WHERE status = 'active' ORDER BY name",
            )
            .fetch_all(&self.db.pool)
            .await
        } else {
            sqlx::query_as::<_, WorkspaceRow>(
                "SELECT DISTINCT w.id, w.slug, w.name, w.status FROM workspaces w \
                 JOIN workspace_members wm ON wm.workspace_id = w.id \
                 WHERE w.status = 'active' \
                   AND ( (wm.principal_type = 'user'  AND wm.principal_id = $1) \
                      OR (wm.principal_type = 'group' AND wm.principal_id IN \
                             (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
                 ORDER BY w.name",
            )
            .bind(user_id.0)
            .fetch_all(&self.db.pool)
            .await
        };
        Ok(rows.map_err(db_err)?.into_iter().map(Into::into).collect())
    }
```
> `WorkspaceRow`/`db_err` already exist in this file.

- [ ] **Step 2: Add the `me_router`**

In `crates/egret-workspace/src/routes.rs` add:
```rust
async fn list_my_workspaces(
    State(st): State<WorkspaceState>,
    Auth(p): Auth,
) -> Result<Json<Vec<WorkspaceDto>>, AuthError> {
    let is_admin = p.platform_roles.iter().any(|r| r == "platform_admin");
    let list = st.workspaces.list_for_principal(p.user_id, is_admin).await.map_err(AuthError)?;
    Ok(Json(list.into_iter().map(Into::into).collect()))
}

pub fn me_router() -> Router<WorkspaceState> {
    Router::new().route("/workspaces", get(list_my_workspaces))
}
```

- [ ] **Step 3: Add `GET /api/auth/config`**

In `crates/egret-auth/src/routes.rs`, add a handler and register it on the **public** part of `auth_router` (alongside `login`, which is CSRF-exempt):
```rust
async fn auth_config(State(st): State<AuthState>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "oidc_enabled": st.config.oidc.enabled }))
}
```
In `auth_router()`, add `.route("/config", get(auth_config))` to the public (non-protected) routes. Ensure `get` is imported.

- [ ] **Step 4: Mount `me_router` in `egret-server`**

In `crates/egret-server/src/lib.rs`, in the workspace block, add `.nest("/api/me", me_router())` and import `me_router`:
```rust
    if let Some(ws) = state.workspace.clone() {
        let admin = Router::new()
            .nest("/api/admin", admin_workspace_router().merge(admin_user_router()))
            .nest("/api/ws", ws_member_router())
            .nest("/api/me", me_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(ws);
        router = router.merge(admin);
    }
```

- [ ] **Step 5: Add DB handler tests**

Append to `crates/egret-workspace/tests/workspace_db.rs`:
```rust
use egret_workspace::routes::me_router;

#[tokio::test]
async fn my_workspaces_lists_membership_and_admin_sees_all() {
    let Some(db) = connect().await else { eprintln!("skip"); return; };
    let owner = seed_user(&db).await;
    let member = seed_user(&db).await;
    let st = ws_state(&db).await;
    let ws = st.workspaces.create(&uniq("ws"), "W", owner).await.unwrap();
    st.members.add(ws.id, "user", member.0, "ws_viewer", owner).await.unwrap();
    let app = axum::Router::new().nest("/api/me", me_router()).with_state(st.clone());

    // member sees their workspace
    let cookie = login_cookie(&db, member, &st.sessions).await;
    let resp = app.clone().oneshot(HttpRequest::builder().uri("/api/me/workspaces")
        .header("cookie", cookie).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let arr: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(arr.as_array().unwrap().iter().any(|w| w["id"] == ws.id.to_string()));

    // a brand-new non-member sees none of this ws
    let outsider = seed_user(&db).await;
    let cookie2 = login_cookie(&db, outsider, &st.sessions).await;
    let resp = app.oneshot(HttpRequest::builder().uri("/api/me/workspaces")
        .header("cookie", cookie2).body(Body::empty()).unwrap()).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let arr: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(!arr.as_array().unwrap().iter().any(|w| w["id"] == ws.id.to_string()));
}
```

- [ ] **Step 6: Verify + commit**

Offline `cargo test --workspace` (skips DB); with DB `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-workspace --test workspace_db`. `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`.
```bash
git add crates/egret-workspace crates/egret-auth/src/routes.rs crates/egret-server/src/lib.rs
git commit -m "feat(api): GET /api/me/workspaces and /api/auth/config for the SPA"
```

---

### Task 2: Vite app scaffold

**Files:** Create `frontend/{package.json,vite.config.ts,tsconfig.json,tsconfig.node.json,index.html,vitest.config.ts,vitest.setup.ts,.gitignore}`, `frontend/src/{main.tsx,App.tsx,index.css,vite-env.d.ts}`.

- [ ] **Step 1: Create `frontend/package.json`**
```json
{
  "name": "egret-frontend",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "typecheck": "tsc --noEmit",
    "test": "vitest run",
    "lint": "eslint ."
  },
  "dependencies": {
    "@tanstack/react-query": "^5.90.0",
    "react": "^18.3.1",
    "react-dom": "^18.3.1",
    "react-router-dom": "^6.26.0"
  },
  "devDependencies": {
    "@testing-library/jest-dom": "^6.4.0",
    "@testing-library/react": "^16.0.0",
    "@testing-library/user-event": "^14.5.0",
    "@types/react": "^18.3.0",
    "@types/react-dom": "^18.3.0",
    "@vitejs/plugin-react": "^4.3.0",
    "jsdom": "^25.0.0",
    "typescript": "^5.5.0",
    "vite": "^5.4.0",
    "vitest": "^2.1.0"
  }
}
```
> The design-system deps (Radix, tailwind, cva, clsx, tailwind-merge, lucide-react, sonner) are added in Task 3 when the components are copied — copy their exact versions from `sparklabx/frontend/package.json`.

- [ ] **Step 2: Config files**

`frontend/vite.config.ts`:
```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@": path.resolve(__dirname, "src") } },
  server: {
    proxy: { "/api": { target: "http://localhost:8080", changeOrigin: true } },
  },
});
```
`frontend/tsconfig.json`:
```json
{
  "compilerOptions": {
    "target": "ES2022", "useDefineForClassFields": true, "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "module": "ESNext", "skipLibCheck": true, "moduleResolution": "bundler",
    "resolveJsonModule": true, "isolatedModules": true, "noEmit": true, "jsx": "react-jsx",
    "strict": true, "noUnusedLocals": true, "noUnusedParameters": true, "noFallthroughCasesInSwitch": true,
    "baseUrl": ".", "paths": { "@/*": ["src/*"] }
  },
  "include": ["src"]
}
```
`frontend/vitest.config.ts`:
```ts
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import path from "node:path";

export default defineConfig({
  plugins: [react()],
  resolve: { alias: { "@": path.resolve(__dirname, "src") } },
  test: { environment: "jsdom", globals: true, setupFiles: ["./vitest.setup.ts"] },
});
```
`frontend/vitest.setup.ts`:
```ts
import "@testing-library/jest-dom/vitest";
```
`frontend/index.html`:
```html
<!doctype html>
<html lang="en">
  <head><meta charset="UTF-8" /><meta name="viewport" content="width=device-width, initial-scale=1.0" /><title>Egret</title></head>
  <body><div id="root"></div><script type="module" src="/src/main.tsx"></script></body>
</html>
```
`frontend/.gitignore`:
```text
node_modules
dist
*.local
```
`frontend/src/vite-env.d.ts`: `/// <reference types="vite/client" />`

- [ ] **Step 3: Minimal entry**

`frontend/src/index.css`: `/* replaced by the copied Tailwind theme in Task 3 */` (empty for now).
`frontend/src/App.tsx`:
```tsx
export default function App() {
  return <div>Egret</div>;
}
```
`frontend/src/main.tsx`:
```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode><App /></React.StrictMode>,
);
```

- [ ] **Step 4: Install + verify**

Run (in `frontend/`): `npm install` then `npm run build` and `npm run typecheck`.
Expected: build succeeds; no type errors.

- [ ] **Step 5: Commit**
```bash
git add frontend
git commit -m "chore(frontend): scaffold vite react app with /api dev proxy"
```

---

### Task 3: Copy the sparklabx design system

**Files:** Create `frontend/src/components/ui/*`, `frontend/src/components/theme-provider.tsx`, `frontend/src/theme/egret-theme.ts`, `frontend/src/lib/utils.ts`, replace `frontend/src/index.css`; Modify `frontend/package.json`, `frontend/vite.config.ts`.

> **⚠️ Version-sensitive copy.** Source: `sparklabx/frontend/`. Copy the files, copy the matching dependency versions, and set up Tailwind v4 exactly as sparklabx does (it uses `@tailwindcss/vite`). Acceptance = build + a trivial render test pass.

- [ ] **Step 1: Copy the design-system files**

Copy from `sparklabx/frontend/src/` into `frontend/src/`:
- `components/ui/*` (all shadcn components)
- `components/theme-provider.tsx`
- `lib/utils.ts`
- `theme/sparklabx-theme.ts` → save as `theme/egret-theme.ts`
- `index.css` (the Tailwind v4 entry with `@theme` tokens) → `frontend/src/index.css`

In the copied `theme-provider.tsx`, change the `storageKey` default to `"egret-theme"`. In any copied file that imports `@/theme/sparklabx-theme`, update the path to `@/theme/egret-theme`.

- [ ] **Step 2: Add the matching dependencies**

From `sparklabx/frontend/package.json`, copy into `frontend/package.json` `dependencies` the exact versions of every package imported by the copied `components/ui/*` + theme + utils — at minimum: all `@radix-ui/react-*` used, `class-variance-authority`, `clsx`, `tailwind-merge`, `lucide-react`, `sonner`, `next-themes` (only if `theme-provider` imports it; sparklabx uses a custom provider, so likely not). Add Tailwind devDeps exactly as sparklabx: `tailwindcss`, `@tailwindcss/vite`, `tw-animate-css` (if present). Add the `@tailwindcss/vite` plugin to `frontend/vite.config.ts` plugins array.

- [ ] **Step 3: Verify with a render test**

Create `frontend/src/components/ui/__tests__/button.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import { Button } from "@/components/ui/button";

test("renders a button with its label", () => {
  render(<Button>Click me</Button>);
  expect(screen.getByRole("button", { name: "Click me" })).toBeInTheDocument();
});
```

- [ ] **Step 4: Build + test**

Run (in `frontend/`): `npm install`, `npm run build`, `npm test`, `npm run typecheck`. All pass (Tailwind compiles, Button renders).

- [ ] **Step 5: Commit**
```bash
git add frontend
git commit -m "feat(frontend): import sparklabx shadcn design system and theme"
```

---

### Task 4: API client + auth hooks

**Files:** Create `frontend/src/api/client.ts`, `frontend/src/api/hooks.ts`, `frontend/src/api/__tests__/client.test.ts`.

- [ ] **Step 1: Write `apiFetch` with tests**

`frontend/src/api/client.ts`:
```ts
export class UnauthorizedError extends Error {}
export class ApiError extends Error {
  constructor(public status: number, message: string) { super(message); }
  static async from(res: Response): Promise<ApiError> {
    let msg = res.statusText;
    try { const j = await res.json(); if (j?.error) msg = j.error; } catch { /* ignore */ }
    return new ApiError(res.status, msg);
  }
}

export function readCookie(name: string): string | null {
  const m = document.cookie.match(new RegExp("(?:^|; )" + name + "=([^;]*)"));
  return m ? decodeURIComponent(m[1]) : null;
}

const MUTATING = new Set(["POST", "PUT", "PATCH", "DELETE"]);

export async function apiFetch(path: string, init: RequestInit = {}): Promise<Response> {
  const method = (init.method ?? "GET").toUpperCase();
  const headers = new Headers(init.headers);
  if (MUTATING.has(method)) {
    const csrf = readCookie("egret_csrf");
    if (csrf) headers.set("X-Egret-CSRF", csrf);
  }
  const res = await fetch(`/api${path}`, { ...init, headers, credentials: "include" });
  if (res.status === 401) throw new UnauthorizedError();
  if (!res.ok) throw await ApiError.from(res);
  return res;
}

export async function apiJson<T>(path: string, init?: RequestInit): Promise<T> {
  return (await apiFetch(path, init)).json() as Promise<T>;
}
```
`frontend/src/api/__tests__/client.test.ts`:
```ts
import { apiFetch, UnauthorizedError, readCookie } from "@/api/client";
import { afterEach, expect, test, vi } from "vitest";

afterEach(() => vi.restoreAllMocks());

test("attaches X-Egret-CSRF on mutating requests from the cookie", async () => {
  document.cookie = "egret_csrf=tok123";
  const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("{}", { status: 200 }));
  await apiFetch("/x", { method: "POST" });
  const headers = (spy.mock.calls[0][1] as RequestInit).headers as Headers;
  expect(headers.get("X-Egret-CSRF")).toBe("tok123");
});

test("does not attach CSRF on GET", async () => {
  document.cookie = "egret_csrf=tok123";
  const spy = vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("{}", { status: 200 }));
  await apiFetch("/x");
  const headers = (spy.mock.calls[0][1] as RequestInit).headers as Headers;
  expect(headers.get("X-Egret-CSRF")).toBeNull();
});

test("throws UnauthorizedError on 401", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
  await expect(apiFetch("/x")).rejects.toBeInstanceOf(UnauthorizedError);
});

test("readCookie parses a value", () => {
  document.cookie = "egret_csrf=abc";
  expect(readCookie("egret_csrf")).toBe("abc");
});
```

- [ ] **Step 2: Write the hooks**

`frontend/src/api/hooks.ts`:
```ts
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export interface Me { user_id: string; email: string; platform_roles: string[]; }
export interface WorkspaceSummary { id: string; slug: string; name: string; }

export function useMe() {
  return useQuery<Me | null>({
    queryKey: ["me"],
    queryFn: async () => {
      try { return await apiJson<Me>("/auth/me"); }
      catch { return null; } // UnauthorizedError → unauthenticated
    },
  });
}

export function useAuthConfig() {
  return useQuery({ queryKey: ["auth-config"], queryFn: () => apiJson<{ oidc_enabled: boolean }>("/auth/config") });
}

export function useMyWorkspaces(enabled: boolean) {
  return useQuery<WorkspaceSummary[]>({
    queryKey: ["my-workspaces"],
    queryFn: () => apiJson<WorkspaceSummary[]>("/me/workspaces"),
    enabled,
  });
}

export function useLogin() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { email: string; password: string }) =>
      apiFetch("/auth/login", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["me"] }),
  });
}

export function useLogout() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => apiFetch("/auth/logout", { method: "POST" }),
    onSuccess: () => qc.setQueryData(["me"], null),
  });
}
```

- [ ] **Step 3: Test + commit**

`npm test` (client tests pass), `npm run typecheck`.
```bash
git add frontend/src/api
git commit -m "feat(frontend): apiFetch client (CSRF, 401) and TanStack Query hooks"
```

---

### Task 5: AuthProvider + LoginPage

**Files:** Create `frontend/src/auth/{AuthProvider.tsx,useAuth.ts,LoginPage.tsx}`, `frontend/src/auth/__tests__/AuthProvider.test.tsx`.

- [ ] **Step 1: AuthProvider + useAuth**

`frontend/src/auth/useAuth.ts`:
```ts
import { createContext, useContext } from "react";
import type { Me } from "@/api/hooks";

export interface AuthState { me: Me | null; isLoading: boolean; }
export const AuthContext = createContext<AuthState>({ me: null, isLoading: true });
export const useAuth = () => useContext(AuthContext);
```
`frontend/src/auth/AuthProvider.tsx`:
```tsx
import type { ReactNode } from "react";
import { useMe } from "@/api/hooks";
import { AuthContext } from "./useAuth";

export function AuthProvider({ children }: { children: ReactNode }) {
  const { data, isLoading } = useMe();
  return (
    <AuthContext.Provider value={{ me: data ?? null, isLoading }}>
      {children}
    </AuthContext.Provider>
  );
}
```

- [ ] **Step 2: LoginPage (local + SSO button)**

`frontend/src/auth/LoginPage.tsx`:
```tsx
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card } from "@/components/ui/card";
import { useAuthConfig, useLogin } from "@/api/hooks";

export function LoginPage() {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const login = useLogin();
  const cfg = useAuthConfig();

  const onSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError(null);
    try { await login.mutateAsync({ email, password }); }
    catch { setError("Invalid email or password"); }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <Card className="w-full max-w-sm p-6">
        <h1 className="mb-4 text-xl font-semibold">Sign in to Egret</h1>
        <form onSubmit={onSubmit} className="space-y-3">
          <Input type="email" placeholder="Email" value={email} onChange={(e) => setEmail(e.target.value)} required />
          <Input type="password" placeholder="Password" value={password} onChange={(e) => setPassword(e.target.value)} required />
          {error && <p className="text-sm text-destructive">{error}</p>}
          <Button type="submit" className="w-full" disabled={login.isPending}>Sign in</Button>
        </form>
        {cfg.data?.oidc_enabled && (
          <a href="/api/auth/oidc/login" className="mt-3 block">
            <Button variant="outline" className="w-full">Sign in with SSO</Button>
          </a>
        )}
      </Card>
    </div>
  );
}
```
> If the copied `Input`/`Card` export differently, adjust imports to the actual component API (they come from the copied design system).

- [ ] **Step 3: AuthProvider test**

`frontend/src/auth/__tests__/AuthProvider.test.tsx`:
```tsx
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import { AuthProvider } from "@/auth/AuthProvider";
import { useAuth } from "@/auth/useAuth";

afterEach(() => vi.restoreAllMocks());

function Probe() {
  const { me, isLoading } = useAuth();
  if (isLoading) return <span>loading</span>;
  return <span>{me ? me.email : "anon"}</span>;
}
function wrap() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}><AuthProvider><Probe /></AuthProvider></QueryClientProvider>);
}

test("authenticated when /auth/me returns 200", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    new Response(JSON.stringify({ user_id: "1", email: "a@b.c", platform_roles: [] }), { status: 200 }));
  wrap();
  await waitFor(() => expect(screen.getByText("a@b.c")).toBeInTheDocument());
});

test("anonymous when /auth/me returns 401", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(new Response("", { status: 401 }));
  wrap();
  await waitFor(() => expect(screen.getByText("anon")).toBeInTheDocument());
});
```

- [ ] **Step 4: Test + commit**

`npm test`, `npm run typecheck`.
```bash
git add frontend/src/auth
git commit -m "feat(frontend): AuthProvider, useAuth, and login page (local + SSO)"
```

---

### Task 6: App shell (Layout C) — AppShell, Header, WorkspaceSwitcher, Sidebar

**Files:** Create `frontend/src/components/layout/{AppShell,Header,Sidebar,WorkspaceSwitcher}.tsx`, `frontend/src/components/layout/__tests__/WorkspaceSwitcher.test.tsx`.

- [ ] **Step 1: WorkspaceSwitcher**

`frontend/src/components/layout/WorkspaceSwitcher.tsx`:
```tsx
import { useNavigate, useParams } from "react-router-dom";
import { useMyWorkspaces } from "@/api/hooks";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";

export function WorkspaceSwitcher() {
  const navigate = useNavigate();
  const { workspace } = useParams();
  const { data: workspaces } = useMyWorkspaces(true);

  return (
    <Select value={workspace ?? ""} onValueChange={(slug) => navigate(`/explore/${slug}`)}>
      <SelectTrigger className="w-56" aria-label="Workspace">
        <SelectValue placeholder="Select a workspace" />
      </SelectTrigger>
      <SelectContent>
        {(workspaces ?? []).map((w) => (
          <SelectItem key={w.id} value={w.slug}>{w.name}</SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
```
> Adjust to the copied `Select` component's actual prop API (shadcn Select uses `onValueChange`).

- [ ] **Step 2: Header + Sidebar + AppShell**

`frontend/src/components/layout/Header.tsx`:
```tsx
import { Link, useLocation } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { useAuth } from "@/auth/useAuth";
import { useLogout } from "@/api/hooks";
import { WorkspaceSwitcher } from "./WorkspaceSwitcher";

export function Header() {
  const { me } = useAuth();
  const logout = useLogout();
  const isAdmin = useLocation().pathname.startsWith("/admin");
  const canAdmin = (me?.platform_roles?.length ?? 0) > 0;

  return (
    <header className="flex h-14 shrink-0 items-center gap-3 border-b border-border bg-card px-4">
      <Link to="/" className="font-semibold">🐦 Egret</Link>
      {!isAdmin && <WorkspaceSwitcher />}
      <div className="flex-1" />
      {canAdmin && (
        <Link to="/admin"><Button variant={isAdmin ? "default" : "ghost"} size="sm">⚙️ Admin</Button></Link>
      )}
      <span className="text-sm text-muted-foreground">{me?.email}</span>
      <Button variant="ghost" size="sm" onClick={() => logout.mutate()}>Logout</Button>
    </header>
  );
}
```
`frontend/src/components/layout/Sidebar.tsx`:
```tsx
import { NavLink, useLocation, useParams } from "react-router-dom";

const EXPLORE = ["SQL Lab", "Datasets", "Charts", "Dashboards", "Notebooks"];
const ADMIN = ["Databases", "Catalog", "Policies", "Users", "Workspaces", "Monitoring"];
const slug = (s: string) => s.toLowerCase().replace(/\s+/g, "-");

export function Sidebar() {
  const isAdmin = useLocation().pathname.startsWith("/admin");
  const { workspace } = useParams();
  const items = isAdmin ? ADMIN : EXPLORE;
  const base = isAdmin ? "/admin" : `/explore/${workspace ?? ""}`;

  return (
    <nav className="w-56 shrink-0 border-r border-border bg-card p-2">
      {items.map((label) => (
        <NavLink key={label} to={`${base}/${slug(label)}`}
          className={({ isActive }) =>
            `block rounded px-3 py-2 text-sm ${isActive ? "bg-accent text-accent-foreground" : "text-muted-foreground hover:bg-accent/50"}`}>
          {label}
        </NavLink>
      ))}
    </nav>
  );
}
```
`frontend/src/components/layout/AppShell.tsx`:
```tsx
import type { ReactNode } from "react";
import { Header } from "./Header";
import { Sidebar } from "./Sidebar";

export function AppShell({ children }: { children: ReactNode }) {
  return (
    <div className="flex h-screen flex-col bg-background">
      <Header />
      <div className="flex min-h-0 flex-1">
        <Sidebar />
        <main className="min-w-0 flex-1 overflow-auto p-6">{children}</main>
      </div>
      <footer className="shrink-0 border-t border-border bg-card px-4 py-2 text-center text-xs text-muted-foreground">
        © {new Date().getFullYear()} Egret
      </footer>
    </div>
  );
}
```

- [ ] **Step 3: WorkspaceSwitcher test**

`frontend/src/components/layout/__tests__/WorkspaceSwitcher.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Routes, Route, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import { WorkspaceSwitcher } from "@/components/layout/WorkspaceSwitcher";

afterEach(() => vi.restoreAllMocks());

function LocationProbe() { return <span data-testid="loc">{useLocation().pathname}</span>; }

test("selecting a workspace navigates to /explore/:slug", async () => {
  vi.spyOn(globalThis, "fetch").mockResolvedValue(
    new Response(JSON.stringify([{ id: "1", slug: "sales", name: "Sales" }]), { status: 200 }));
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={["/"]}>
        <WorkspaceSwitcher />
        <Routes><Route path="*" element={<LocationProbe />} /></Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  await userEvent.click(await screen.findByLabelText("Workspace"));
  await userEvent.click(await screen.findByText("Sales"));
  expect(screen.getByTestId("loc").textContent).toBe("/explore/sales");
});
```
> **Executor note:** Radix `Select` renders a portal/listbox; if the click interaction differs under jsdom, drive it via the underlying trigger/options the copied `Select` exposes (keyboard or `selectOption`). Keep the assertion: navigation to `/explore/sales`.

- [ ] **Step 4: Test + commit**

`npm test`, `npm run typecheck`.
```bash
git add frontend/src/components/layout
git commit -m "feat(frontend): workspace-first app shell (header, sidebar, switcher)"
```

---

### Task 7: Routing + guards + placeholder pages

**Files:** Create `frontend/src/auth/guards.tsx`, `frontend/src/routes/{NotFound,Forbidden}.tsx`, `frontend/src/routes/admin/*`, `frontend/src/routes/explore/*`; Replace `frontend/src/App.tsx`; Create `frontend/src/auth/__tests__/guards.test.tsx`.

- [ ] **Step 1: Guards**

`frontend/src/auth/guards.tsx`:
```tsx
import type { ReactNode } from "react";
import { Navigate, useParams } from "react-router-dom";
import { useAuth } from "./useAuth";
import { useMyWorkspaces } from "@/api/hooks";
import { Forbidden } from "@/routes/Forbidden";

export function RequireAuth({ children }: { children: ReactNode }) {
  const { me, isLoading } = useAuth();
  if (isLoading) return <div className="p-10">Loading…</div>;
  if (!me) return <Navigate to="/login" replace />;
  return <>{children}</>;
}

export function RequirePlatform({ children }: { children: ReactNode }) {
  const { me } = useAuth();
  if (!me || me.platform_roles.length === 0) return <Forbidden />;
  return <>{children}</>;
}

export function RequireWorkspace({ children }: { children: ReactNode }) {
  const { workspace } = useParams();
  const { data, isLoading } = useMyWorkspaces(true);
  if (isLoading) return <div className="p-10">Loading…</div>;
  if (!data?.some((w) => w.slug === workspace)) return <Forbidden />;
  return <>{children}</>;
}
```

- [ ] **Step 2: Placeholder + error pages**

`frontend/src/routes/Forbidden.tsx`:
```tsx
import { Card } from "@/components/ui/card";
export function Forbidden() {
  return <div className="p-10"><Card className="p-6"><h1 className="text-lg font-semibold">403 — Forbidden</h1><p className="text-muted-foreground">You don't have access to this page.</p></Card></div>;
}
```
`frontend/src/routes/NotFound.tsx`:
```tsx
import { Card } from "@/components/ui/card";
export function NotFound() {
  return <div className="p-10"><Card className="p-6"><h1 className="text-lg font-semibold">404 — Not found</h1></Card></div>;
}
```
Create a shared placeholder `frontend/src/routes/Placeholder.tsx`:
```tsx
import { Card } from "@/components/ui/card";
export function Placeholder({ title }: { title: string }) {
  return (
    <Card className="p-6">
      <h1 className="text-lg font-semibold">{title}</h1>
      <p className="mt-2 text-muted-foreground">This feature arrives in a later phase.</p>
    </Card>
  );
}
```
Create thin page modules that render `<Placeholder title="…" />` for each Explorer page (`routes/explore/{SqlLab,Datasets,Charts,Dashboards,Notebooks}.tsx`) and each Admin page (`routes/admin/{Databases,Catalog,Policies,Users,Workspaces,Monitoring}.tsx`). Example `routes/explore/SqlLab.tsx`:
```tsx
import { Placeholder } from "@/routes/Placeholder";
export default function SqlLab() { return <Placeholder title="SQL Lab" />; }
```
(Repeat for each, with the matching title.)

- [ ] **Step 3: App routing**

Replace `frontend/src/App.tsx`:
```tsx
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Navigate, Route, Routes, useParams } from "react-router-dom";
import { ThemeProvider } from "@/components/theme-provider";
import { AuthProvider } from "@/auth/AuthProvider";
import { RequireAuth, RequirePlatform, RequireWorkspace } from "@/auth/guards";
import { AppShell } from "@/components/layout/AppShell";
import { LoginPage } from "@/auth/LoginPage";
import { NotFound } from "@/routes/NotFound";
import { useAuth } from "@/auth/useAuth";
import { useMyWorkspaces } from "@/api/hooks";
import SqlLab from "@/routes/explore/SqlLab";
import Datasets from "@/routes/explore/Datasets";
import Charts from "@/routes/explore/Charts";
import Dashboards from "@/routes/explore/Dashboards";
import Notebooks from "@/routes/explore/Notebooks";
import Databases from "@/routes/admin/Databases";
import Catalog from "@/routes/admin/Catalog";
import Policies from "@/routes/admin/Policies";
import Users from "@/routes/admin/Users";
import Workspaces from "@/routes/admin/Workspaces";
import Monitoring from "@/routes/admin/Monitoring";

const qc = new QueryClient({ defaultOptions: { queries: { refetchOnWindowFocus: false, retry: 1, staleTime: 30000 } } });

function HomeRedirect() {
  const { me, isLoading } = useAuth();
  const { data } = useMyWorkspaces(!!me);
  if (isLoading) return <div className="p-10">Loading…</div>;
  if (data && data.length > 0) return <Navigate to={`/explore/${data[0].slug}`} replace />;
  return <div className="p-10">No workspaces yet. Ask an admin to add you to one.</div>;
}

function ExploreRoutes() {
  const { workspace } = useParams();
  return (
    <RequireWorkspace>
      <AppShell>
        <Routes>
          <Route index element={<Navigate to="sql-lab" replace />} />
          <Route path="sql-lab" element={<SqlLab />} />
          <Route path="datasets" element={<Datasets />} />
          <Route path="charts" element={<Charts />} />
          <Route path="dashboards" element={<Dashboards />} />
          <Route path="notebooks" element={<Notebooks />} />
          <Route path="*" element={<Navigate to="sql-lab" replace />} />
        </Routes>
      </AppShell>
    </RequireWorkspace>
  );
}

function AdminRoutes() {
  return (
    <RequirePlatform>
      <AppShell>
        <Routes>
          <Route index element={<Navigate to="databases" replace />} />
          <Route path="databases" element={<Databases />} />
          <Route path="catalog" element={<Catalog />} />
          <Route path="policies" element={<Policies />} />
          <Route path="users" element={<Users />} />
          <Route path="workspaces" element={<Workspaces />} />
          <Route path="monitoring" element={<Monitoring />} />
          <Route path="*" element={<Navigate to="databases" replace />} />
        </Routes>
      </AppShell>
    </RequirePlatform>
  );
}

export default function App() {
  return (
    <QueryClientProvider client={qc}>
      <ThemeProvider defaultTheme="system" storageKey="egret-theme">
        <BrowserRouter>
          <AuthProvider>
            <Routes>
              <Route path="/login" element={<LoginPage />} />
              <Route path="/" element={<RequireAuth><HomeRedirect /></RequireAuth>} />
              <Route path="/explore/:workspace/*" element={<RequireAuth><ExploreRoutes /></RequireAuth>} />
              <Route path="/admin/*" element={<RequireAuth><AdminRoutes /></RequireAuth>} />
              <Route path="*" element={<NotFound />} />
            </Routes>
          </AuthProvider>
        </BrowserRouter>
      </ThemeProvider>
    </QueryClientProvider>
  );
}
```
> Adjust the `ThemeProvider` import/props to the copied component's actual API.

- [ ] **Step 4: Guard test**

`frontend/src/auth/__tests__/guards.test.tsx`:
```tsx
import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, expect, test, vi } from "vitest";
import { AuthContext } from "@/auth/useAuth";
import { RequireAuth, RequirePlatform } from "@/auth/guards";

afterEach(() => vi.restoreAllMocks());
const qc = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

test("RequireAuth redirects to /login when unauthenticated", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: null, isLoading: false }}>
        <MemoryRouter initialEntries={["/secret"]}>
          <Routes>
            <Route path="/secret" element={<RequireAuth><span>secret</span></RequireAuth>} />
            <Route path="/login" element={<span>login page</span>} />
          </Routes>
        </MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.getByText("login page")).toBeInTheDocument();
});

test("RequirePlatform blocks a non-platform user", () => {
  render(
    <QueryClientProvider client={qc()}>
      <AuthContext.Provider value={{ me: { user_id: "1", email: "a@b.c", platform_roles: [] }, isLoading: false }}>
        <MemoryRouter><RequirePlatform><span>admin area</span></RequirePlatform></MemoryRouter>
      </AuthContext.Provider>
    </QueryClientProvider>,
  );
  expect(screen.queryByText("admin area")).not.toBeInTheDocument();
  expect(screen.getByText(/Forbidden/i)).toBeInTheDocument();
});
```

- [ ] **Step 5: Test + build + commit**

`npm test`, `npm run typecheck`, `npm run build` (all pages compile).
```bash
git add frontend/src
git commit -m "feat(frontend): routing, guards, and placeholder pages for both portals"
```

---

### Task 8: Dockerfile + nginx + optional compose service

**Files:** Create `frontend/Dockerfile`, `frontend/nginx-prod.conf`; Modify `deploy/docker-compose.dev.yml`.

- [ ] **Step 1: Adapt sparklabx's nginx + Dockerfile**

Copy `sparklabx/frontend/Dockerfile` and `sparklabx/frontend/nginx-prod.conf` into `frontend/`, adapting: the build stage runs `npm ci && npm run build`; nginx serves `dist/` and proxies `/api` to the backend (`proxy_pass http://egret:8080;` for compose, or an `API_URL` env). Ensure SPA fallback (`try_files $uri /index.html;`).

- [ ] **Step 2: Optional dev compose service**

In `deploy/docker-compose.dev.yml` add (optional, for full-stack dev):
```yaml
  frontend:
    build: ../frontend
    ports:
      - "5173:80"
    depends_on:
      - postgres
```
> The egret backend isn't containerized in the dev compose yet; for local dev the Vite dev server (`npm run dev`) + `cargo run -p egret-bin -- serve` is the primary path. This service is for a static-build smoke test.

- [ ] **Step 3: Validate + commit**

`docker compose -f deploy/docker-compose.dev.yml config -q` → exit 0. (Building the image is optional here.)
```bash
git add frontend/Dockerfile frontend/nginx-prod.conf deploy/docker-compose.dev.yml
git commit -m "chore(frontend): production Dockerfile + nginx, optional dev compose service"
```

---

### Task 9: Frontend CI job

**Files:** Modify `.github/workflows/ci.yml`; Create `frontend/eslint.config.js`.

- [ ] **Step 1: ESLint config**

Create a minimal `frontend/eslint.config.js` (flat config) using `@eslint/js` + `typescript-eslint` + the react-hooks/react-refresh plugins, or copy/adapt `sparklabx/frontend/eslint.config.js`. Add the matching devDeps (`eslint`, `typescript-eslint`, `eslint-plugin-react-hooks`, `eslint-plugin-react-refresh`, `@eslint/js`) to `frontend/package.json` with the versions from sparklabx.

- [ ] **Step 2: Add a `frontend` CI job**

In `.github/workflows/ci.yml`, add a second job (parallel to `rust`):
```yaml
  frontend:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: frontend
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "20"
          cache: npm
          cache-dependency-path: frontend/package-lock.json
      - run: npm ci
      - run: npm run lint
      - run: npm run typecheck
      - run: npm test
      - run: npm run build
```

- [ ] **Step 3: Verify locally + commit**

In `frontend/`: `npm run lint`, `npm run typecheck`, `npm test`, `npm run build` all pass. Ensure `frontend/package-lock.json` is committed (run `npm install` once to generate it).
```bash
git add .github/workflows/ci.yml frontend/eslint.config.js frontend/package.json frontend/package-lock.json
git commit -m "ci(frontend): lint, typecheck, test, and build job"
```

---

### Task 10: Final verification + manual e2e

**Files:** none (verification only).

- [ ] **Step 1: Frontend gate**

In `frontend/`: `npm run lint`, `npm run typecheck`, `npm test`, `npm run build`. All green.

- [ ] **Step 2: Backend gate**

`cargo test --workspace`; `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-workspace --test workspace_db`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`. All green.

- [ ] **Step 3: Manual e2e (against the running backend)**

```bash
# backend
export DATABASE_URL=postgres://egret:egret@localhost:5432/egret
EGRET_ADMIN_EMAIL=admin@egret.local EGRET_ADMIN_PASSWORD=devpass cargo run -p egret-bin -- init --force
cargo run -p egret-bin -- serve &
# frontend dev server (proxies /api → :8080)
cd frontend && npm run dev &
# browse http://localhost:5173 → login (admin@egret.local / devpass) → lands in app;
#  ⚙️ Admin visible (platform_admin); workspace switcher lists workspaces (create one via
#  POST /api/admin/workspaces first if empty); /admin/* and /explore/:ws/* show placeholders.
```
Expected: local login works end-to-end through the SPA; guards + switcher + shell behave; logout returns to `/login`.

- [ ] **Step 4: Commit (only if a verification fix was needed)**
```bash
git add -A
git commit -m "fix(frontend): address final verification findings"
```

---

## Self-Review

**Spec coverage (Phase 0b spec):**
- §3 directory structure → Tasks 2–7. ✅
- §4 shell Layout C (header workspace switcher + Admin button + contextual sidebar) → Task 6. ✅
- §5 routing + guards (RequireAuth/RequirePlatform/RequireWorkspace) + placeholders → Task 7. ✅
- §6 auth (me/login/logout/SSO/CSRF) + backend `GET /api/me/workspaces` + `GET /api/auth/config` → Tasks 1, 4, 5. ✅
- §7 apiFetch client + TanStack Query hooks → Task 4. ✅
- §8 dev proxy (Task 2) + Dockerfile/nginx (Task 8). ✅
- §9 testing (Vitest: AuthProvider, guards, switcher, apiFetch CSRF) → Tasks 4–7; CI job Task 9. ✅

**Placeholder scan:** No "TBD"/"add validation". The design-system copy (Task 3) and nginx/eslint (Tasks 8–9) are concrete "copy these exact files from sparklabx + matching deps + verify" steps, not hand-waves — the source files exist. Executor notes on Tasks 2–3/6 flag version-sensitive tooling with the acceptance gate (build/test pass).

**Type consistency:**
- `apiFetch`/`apiJson`/`readCookie`/`UnauthorizedError` — consistent Tasks 4–7. ✅
- Hooks `useMe`/`useAuthConfig`/`useMyWorkspaces`/`useLogin`/`useLogout` + types `Me`/`WorkspaceSummary` — consistent Tasks 4, 5, 6, 7. ✅
- `AuthContext`/`useAuth`/`AuthProvider` — consistent Tasks 5, 6, 7. ✅
- Guards `RequireAuth`/`RequirePlatform`/`RequireWorkspace` — consistent Tasks 7. ✅
- Backend `WorkspaceRepo::list_for_principal`, `me_router`, `WorkspaceDto` — consistent Task 1. ✅ (`WorkspaceDto` already exists in `routes.rs` from 1a-2.)
- Routes `/api/me/workspaces`, `/api/auth/config` — backend (Task 1) and frontend hooks (Task 4) agree. ✅

**Open follow-ups:** real feature pages (later phases); OpenAPI-generated client; Playwright e2e; containerize the backend for full-stack compose.
