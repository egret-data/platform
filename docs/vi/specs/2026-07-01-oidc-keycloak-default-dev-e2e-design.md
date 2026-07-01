# OIDC (Keycloak) làm auth mặc định cho dev + e2e — Thiết kế (bản tham chiếu)

> Bản chuẩn: `docs/en/specs/2026-07-01-oidc-keycloak-default-dev-e2e-design.md`. File này là bản tham chiếu rút gọn.
> Trạng thái: đã duyệt (brainstorming) · 2026-07-01.

## Mục tiêu
Biến OIDC thật (Keycloak local) thành đường đăng nhập **mặc định** cho dev stack và e2e Playwright. Local email/password giữ lại làm **break-glass** cho admin. Định hướng enterprise: chạy đúng luồng SSO thật (redirect → Keycloak → callback → JWT delegation per-user) thay vì local login khi test hằng ngày.

## Hiện trạng (đã có sẵn — việc này chỉ là *default hoá*, không build OIDC)
- **Keycloak trong dev stack** — `deploy/docker-compose.dev.yml` service `keycloak` (image 26.0, host `:8081`), import realm `deploy/keycloak/egret-realm.json`.
- **Realm `egret`** — client `egret`/`egret-secret`, `redirectUris` gồm `*` + `localhost/*`, mapper `groups`, audiences catalog/trino/starrocks, group `data-team`, user `tester@egret.local`/`pw`.
- **Backend OIDC** (`egret-auth`) — discover/login/callback/cookie/refresh; JIT provision theo `sub`, sync group→local group; F2 delegate JWT per-user xuống Trino/StarRocks/catalog.
- **Config** — `OidcConfig{enabled, issuer_url, client_id, client_secret, redirect_url, groups_claim}` (prefix `EGRET_OIDC__`), gate bởi `security.secret_key`.
- **Login page** — đã hiện nút "Sign in with SSO" khi `/api/auth/config` trả `oidc_enabled:true`.
- **RBAC** — `platform_role_assignments` (nâng quyền qua `replace_platform_roles`) + `workspace_members` (principal là **user hoặc group**). `IdpRoleMapping` chỉ map platform-role; map workspace-membership là "later phase".
- **Dev serving** — Vite `:5173` proxy `/api` → `:8080` (`changeOrigin`, `ws:true`); Playwright `baseURL` = `:5173`.

**Khoảng trống:** `.env` dev chưa có `EGRET_OIDC__*` → `oidc.enabled=false` → local login; notebook e2e login form local.

## Quyết định (brainstorming)
1. **Login UX:** SSO là chính; form local ẩn sau link nhỏ "Admin sign-in (break-glass)".
2. **E2e:** viết lại notebook e2e đăng nhập qua **UI Keycloak thật** (redirect → form KC → callback).
3. **Admin model:** user OIDC JIT provision là **member**; không thêm admin group vào realm, không map group→`platform_admin`. Local admin là super-admin bootstrap/break-glass, dùng để nâng quyền user OIDC sau.
4. **Workspace cho user SSO test — Approach A (chọn):** seed workspace + bind **group `data-team`** làm member role notebook (`ws_editor`). Vì OIDC sync `tester` vào group `data-team` và `workspace_members` nhận principal group, `tester` thừa hưởng quyền workspace ngay khi login SSO — e2e single-actor, tất định. (B = e2e 2-actor admin gán live: hoãn; C = nâng tester thành platform_admin: loại vì over-privileged.)
5. **Redirect/port:** browser ở origin Vite `:5173` nên callback phải về đó để cookie session rơi đúng origin → `EGRET_OIDC__REDIRECT_URL=http://localhost:5173/api/auth/oidc/callback` (realm cho phép qua `*`/`localhost/*`).

## Các cấu phần
1. **Config & dev env** — `.env`/`.env.example` thêm `EGRET_OIDC__*` (issuer `:8081/realms/egret`, client `egret`/`egret-secret`, redirect `:5173`, `GROUPS_CLAIM=groups`) + `EGRET_SECURITY__SECRET_KEY` (base64 ≥64B, dev-only). Giữ local admin.
2. **Login UX** (`LoginPage.tsx`) — khi `oidc_enabled`: SSO chính, form local ẩn sau disclosure "Admin sign-in (break-glass)".
3. **Realm/seed** — giữ `tester`/`data-team`; seed workspace + bind group `data-team`. Không thêm admin group.
4. **E2e** — helper `loginViaOidc()` drive Keycloak thật; `notebook.spec` dùng tester SSO; giữ helper local-admin cho break-glass.
5. **Verify** — Manual: `just up` → serve env OIDC → /login → SSO → tester → workspace → chạy notebook; kiểm break-glass local admin. Tự động: `npm run test:e2e`; backend OIDC tests còn xanh; full CI local trước merge.

## Ngoài phạm vi
- Rule map group→workspace theo IdP (later phase; ở đây seed 1 binding tĩnh).
- IdP enterprise thật (Okta/Azure AD) hardening + multi-workspace provisioning.
- E2e 2-actor admin-elevation (Approach B) — có thể là spec follow-up.

## Rủi ro
- **Cookie/port:** sai `REDIRECT_URL` (trỏ `:8080` khi app ở `:5173`) → session hỏng âm thầm. Quyết định 5 ghim `:5173`.
- **E2e giòn:** drive login page Keycloak phụ thuộc markup KC → selector bền (`name=username/password`, submit) + timeout rộng (KC cold start).
- **Realm `redirectUris` có `*`** — ổn cho dev; production phải ghim URI chính xác (spec prod-hardening).
