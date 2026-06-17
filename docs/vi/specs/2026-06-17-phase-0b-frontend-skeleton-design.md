# Phase 0b — Frontend Skeleton — Spec thiết kế

> **Trạng thái:** Bản tham khảo tiếng Việt · **Ngày:** 2026-06-17 · **Phiên bản:** 0.1
> ⚠️ Bản gốc (canonical) là tiếng Anh: [`docs/en/specs/2026-06-17-phase-0b-frontend-skeleton-design.md`](../../en/specs/2026-06-17-phase-0b-frontend-skeleton-design.md). Khác biệt thì ưu tiên bản tiếng Anh.
> Hiện thực phần frontend skeleton của [Tài liệu kiến trúc gốc](../architecture/2026-06-17-egret-architecture-design.md) §6, trên nền backend identity đã ship (1a-1/1a-2/1a-3).

---

## 1. Mục tiêu & phạm vi

Dựng skeleton SPA của Egret: app shell dual-portal, theme, xác thực (local + SSO), workspace switcher, và routing cho Admin/Explorer với **trang placeholder** — nối backend đã có. Chưa có feature thật (các phase sau).

**Trong phạm vi**
- Một app Vite React ở `frontend/`, kế thừa design system **shadcn/ui + Radix + Tailwind v4** của sparklabx (copy vào).
- App shell — **workspace-first** (Layout C): header có workspace switcher + nút Admin + theme/user menu, sidebar theo ngữ cảnh.
- Auth: trang login (local + nút SSO), bootstrap session qua `/api/auth/me`, logout, header CSRF cho mutation.
- Routing + guard: `/login`, `/explore/:workspace/*`, `/admin/*`, có guard role/membership + trang placeholder.
- API client typed viết tay + hooks TanStack Query cho endpoint auth/workspace.
- Một **bổ sung backend nhỏ**: `GET /api/me/workspaces` (workspace của principal) — cần cho switcher workspace-first.
- Vite proxy dev tới backend; build tĩnh (nginx).

**Ngoài phạm vi (hoãn)**
- Mọi feature thật (database, catalog, SQL Lab, dataset, chart, dashboard, notebook, data API, workflow) → phase sau.
- OpenAPI codegen (backend chưa có utoipa) → khi backend expose OpenAPI.
- pnpm monorepo / shared package → khi có consumer thứ hai.
- Playwright e2e, i18n → tương lai.

## 2. Quyết định

| Quyết định | Lựa chọn |
|---|---|
| Cấu trúc project | **Single Vite app** `frontend/` (chưa monorepo) |
| Design system | **Copy** `components/ui` + theme + tailwind config + `ThemeProvider` của sparklabx |
| API client | **Viết tay** typed (`fetch`, credentials + CSRF) + hooks TanStack Query |
| Bố cục shell | **Workspace-first (Layout C)** — workspace selector nổi bật ở header; Admin là nút ⚙️ riêng (scope platform) |
| Phạm vi skeleton | App shell + auth + routing với **trang placeholder**; không feature thật |

## 3. Kiến trúc / cấu trúc thư mục

```text
frontend/
├── package.json · vite.config.ts · tsconfig.json · index.html · Dockerfile · nginx-prod.conf
├── src/
│   ├── main.tsx · App.tsx · index.css
│   ├── components/
│   │   ├── ui/                  # copy shadcn (button, card, dialog, table, tabs, select, dropdown-menu, input, tooltip, toaster, sonner, …)
│   │   ├── theme-provider.tsx   # copy (light/dark/system, localStorage "egret-theme")
│   │   └── layout/
│   │       ├── AppShell.tsx     # header + sidebar + main + footer (Layout C)
│   │       ├── Header.tsx       # workspace switcher + nút Admin + theme toggle + user menu
│   │       ├── Sidebar.tsx      # nav theo ngữ cảnh (collapsible)
│   │       └── WorkspaceSwitcher.tsx
│   ├── routes/
│   │   ├── admin/              # placeholder: Databases, Catalog, Policies, Users, Workspaces, Monitoring
│   │   └── explore/           # placeholder: SqlLab, Datasets, Charts, Dashboards, Notebooks
│   ├── auth/
│   │   ├── AuthProvider.tsx · useAuth.ts · LoginPage.tsx · guards.tsx (RequireAuth, RequirePlatform, RequireWorkspace)
│   ├── api/
│   │   ├── client.ts          # apiFetch (credentials, header CSRF, xử lý 401)
│   │   └── hooks.ts           # useMe, useLogin, useLogout, useMyWorkspaces
│   └── theme/                 # theme tokens copy
└── tests/                     # Vitest + Testing Library
```

## 4. App shell (Layout C — workspace-first)

- **Header:** brand `🐦 Egret` · **`WorkspaceSwitcher`** (dropdown workspace của user; chọn → điều hướng `/explore/:slug/...`) · spacer · nút **⚙️ Admin** (hiện với user có ≥1 platform permission; vào `/admin`) · **theme toggle** · **user menu** (email, Logout).
- **Sidebar (theo ngữ cảnh, collapsible — kế thừa sparklabx):**
  - Explorer (`/explore/:workspace/*`): SQL Lab · Datasets · Charts · Dashboards · Notebooks.
  - Admin (`/admin/*`): Databases · Catalog · Policies · Users · Workspaces · Monitoring.
- **Footer:** nhẹ, `© năm Egret`.
- Shell điều khiển theo route: tập item sidebar và "scope hiện tại" của header suy từ path đang ở `/admin` hay `/explore/:workspace`.

## 5. Routing + guard

| Path | Guard | Nội dung |
|---|---|---|
| `/login` | không (đã auth → vào app) | `LoginPage` |
| `/` | RequireAuth | redirect workspace đầu (`/explore/:slug`) hoặc màn "chọn workspace" nếu chưa có |
| `/explore/:workspace/*` | RequireAuth + RequireWorkspace | Explorer placeholder |
| `/admin/*` | RequireAuth + RequirePlatform | Admin placeholder |
| `*` | — | NotFound |

- **RequireAuth:** `useMe` trả chưa-auth → redirect `/login`.
- **RequirePlatform:** principal có ≥1 platform role/permission (từ `platform_roles` của `/api/auth/me`) → ngược lại trang 403.
- **RequireWorkspace:** slug `:workspace` nằm trong workspace của user (từ `useMyWorkspaces`) → ngược lại 403. (Backend cũng enforce ở mỗi API call.)
- Trang placeholder hiển thị tiêu đề + panel "Sẽ có ở phase sau", dùng `Card` của design system.

## 6. Luồng auth + bổ sung backend

**Frontend**
- **Bootstrap:** khi load, `AuthProvider` gọi `GET /api/auth/me`. 200 → authenticated (lưu `{ user_id, email, platform_roles }`); 401 → chưa auth.
- **Local login:** `LoginPage` post `POST /api/auth/login {email,password}` → 200 thì refetch `me`, điều hướng `/`.
- **SSO:** nút "Đăng nhập SSO" điều hướng full-page tới `GET /api/auth/oidc/login` (backend 302 tới IdP và quay lại). Nút chỉ hiện khi SSO bật — qua `GET /api/auth/config` trả `{ oidc_enabled: bool }` (bổ sung backend nhỏ). **Quyết định: thêm `GET /api/auth/config`.**
- **Logout:** `POST /api/auth/logout` (kèm header CSRF) → xoá state → `/login`.
- **CSRF:** `apiFetch` đọc cookie `egret_csrf`, set `X-Egret-CSRF` cho POST/PUT/PATCH/DELETE.

**Bổ sung backend (nhỏ, phase này)**
1. `GET /api/me/workspaces` → workspace principal truy cập được: hợp direct + group `workspace_members`, hoặc **tất cả workspace active nếu `platform_admin`**. Trả `[{ id, slug, name }]`. Hiện thực ở `egret-workspace` (dùng `WorkspaceRepo`/`Authorizer`), mount trên router `/api/me` (chỉ cần `Auth`, không cần workspace context).
2. `GET /api/auth/config` → `{ oidc_enabled: bool }` (không cần auth) để trang login ẩn/hiện nút SSO. Hiện thực ở `egret-auth` (đọc `config.oidc.enabled`).

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
Hooks (TanStack Query): `useMe()`, `useAuthConfig()`, `useLogin()`, `useLogout()`, `useMyWorkspaces()`. `QueryClient` cấu hình như sparklabx (no refetch-on-focus, retry: 1). 401 từ query bất kỳ → invalidate auth → redirect `/login`.

## 8. Dev / build

- **Vite dev proxy:** `vite.config.ts` proxy `/api` → `http://localhost:8080` (backend egret) để cookie/CSRF same-origin khi dev.
- **Build:** asset tĩnh phục vụ bởi nginx (copy/chỉnh `nginx-prod.conf` + `Dockerfile` của sparklabx), proxy `/api` tới backend.
- Tùy chọn thêm service `frontend` vào `deploy/docker-compose.dev.yml` (dev). CI frontend (lint + typecheck + vitest) là job workflow riêng; ráp là việc của plan.

## 9. Test

- **Vitest + Testing Library** (jsdom). Test trọng yếu:
  - `AuthProvider`: `me` 200 → authenticated; 401 → chưa auth.
  - Guard: `RequireAuth` redirect `/login` khi chưa auth; `RequirePlatform` chặn khi thiếu platform role; `RequireWorkspace` chặn slug không phải member.
  - `WorkspaceSwitcher`: chọn workspace → điều hướng `/explore/:slug`.
  - `apiFetch`: gắn `X-Egret-CSRF` từ cookie cho mutation; ném `UnauthorizedError` khi 401.
  - Mock `fetch`/handler (vd MSW hoặc fetch mock) — không cần backend thật; test offline.
- Lint (eslint, config kế thừa) + typecheck `tsc --noEmit` ở CI.

## 10. Phân rã cho plan

Task gợi ý: (1) backend `GET /api/me/workspaces` + `GET /api/auth/config` (Rust, DB-gated test); (2) scaffold app Vite + Tailwind + tsconfig + index.css; (3) copy design system (components/ui + theme-provider + theme), verify build; (4) client `apiFetch` + auth hooks + setup Vitest; (5) `AuthProvider` + `LoginPage` (local + nút SSO) + `useAuthConfig`; (6) `AppShell` + `Header` + `WorkspaceSwitcher` + `Sidebar` (Layout C); (7) routing + guard + trang placeholder; (8) Vite dev proxy + Dockerfile/nginx; (9) job CI frontend (lint + typecheck + vitest); (10) verify cuối + e2e tay với backend đang chạy.

## 11. Tham khảo

- Frontend sparklabx: `sparklabx/frontend/src/` — `App.tsx` (shell + routing + auth gating), `components/ui/*` (shadcn), `components/theme-provider.tsx`, `components/layout/{Header,Sidebar}.tsx`, `vite.config.ts`, `nginx-prod.conf`.
- Backend Egret (tiêu thụ): `/api/auth/{login,logout,me,oidc/login}`, `/api/admin/workspaces`, kèm hai cái mới `/api/me/workspaces` + `/api/auth/config`.
- Kiến trúc: [Tài liệu gốc](../architecture/2026-06-17-egret-architecture-design.md) §6.
