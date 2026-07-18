# E2E: auth + control-plane admin trên kind — Thiết kế

**Ngày:** 2026-07-18 · **Trạng thái:** Thiết kế — đã duyệt · **Nhánh:** `feat/e2e-auth-cp-admin` (off `main`)

> Bản chuẩn tiếng Anh: `docs/en/specs/2026-07-18-e2e-auth-cp-admin-design.md`.

## Vấn đề

Project 2 (full automated e2e) slice 1 — data-plane money-path — đã xong ([[egret-e2e-dataplane-kind]], PR #273). Slice tiếp = **auth + control-plane admin**: cửa vào (login) + bề mặt quản trị (access console: users / groups / roles / permissions / audit), kèm guard chống privilege-escalation giữ non-admin khỏi control plane. Chưa có e2e tự động nào phủ.

Bề mặt này thuần UI form/table — tất định, không async k8s provisioning — nên chạy vài giây và ổn định (ngược với độ rung engine-warmup của slice 1).

## Mục tiêu

Spec Playwright chạy qua runner `just e2e-kind` sẵn có, nhắm in-cluster kind deploy (`egret.localtest.me`), assert:

1. **Auth gating** (money-path bảo mật): break-glass admin vào được control plane; OIDC member **bị chặn**.
2. **Control-plane admin**: các trang access console load đúng seed; RBAC group CRUD round-trip; mutation hiện trong audit log.

Mô hình chạy như slice 1: kind là target canonical, opt-in `EGRET_E2E_KIND=1`, **không** trong GitHub CI (chỉ unit ở đó).

## Ngoài phạm vi

- Governance (policies / masking / row-filter / classifications / Cedar) — slice riêng.
- OIDC-admin-via-group-seed (đường `deploy/seed-sso-test-rbac.sql`) — test member-denied không cần RBAC seed.
- Workspace/engine provisioning (chỗ khác phủ), gán role cho user, sửa permission catalog (code-defined, read-only).
- Không đổi code app/backend — chỉ harness test + docs.

## Bối cảnh — bề mặt đã grounding (verify)

- **Guards** (`src/auth/guards.tsx`): `RequireControlPlaneAccess` render `<Forbidden/>` khi `me.permissions`/roles không cho CP; `RequireGlobalAdmin` cần `platform_roles.includes("platform_admin")`; `RequirePermission perm="X:read"` gate từng mục. `<Forbidden/>` render **"403 — Forbidden"** + "You don't have access to this page." Member OIDC JIT có `platform_roles.length === 0` → Forbidden ở `/control-plane` (không cần workspace).
- **Header** (`src/components/layout/Header.tsx`): link **"Control Plane"** chỉ hiện khi `canAdmin` = `platform_roles.includes("platform_admin")` HOẶC `permissions` giao `CONTROL_PLANE_PERMS`. Member thường không thấy.
- **CP routes** (`src/App.tsx:132-161`): `/control-plane/{users,groups,roles,permissions,access-overview,access-explorer,who-can-access,audit-log,workspaces}`.
- **Access console** (`src/routes/control-plane/access/*`): `GroupsPage` có nút **Create** → dialog field tên → `createGroup({name})` (`src/api/access.ts`), + actions mỗi group (manage roles, members, rename, **Delete** qua `ConfirmDeleteDialog`). `PermissionsPage` là catalog read-only. `UsersPage`/`RolesPage` list entry seed.
- **Login helper** (`frontend/tests/e2e/helpers.ts`): `loginLocalAdmin` (break-glass) và `loginViaOidc` (Keycloak SSO, mặc định `tester@egret.local`, chờ `/explore/{ws}/`). Realm seed `member-sso@egret.local` / `Member12345!` (group `data-team`, member thường) + `admin-sso` / `tester`.
- **Runner**: `just e2e-kind` (slice 1). Spec slice này chạy cùng lệnh (khớp `EGRET_E2E_KIND`).

## Kiến trúc

Hai spec focused + một helper nhỏ; thuần browser-UI (không `page.request` provisioning, không chờ k8s).

### A. `frontend/tests/e2e/auth-gating.spec.ts` — money-path bảo mật

Gate: skip trừ khi `EGRET_E2E_KIND=1`.

- **Test 1 — break-glass admin vào CP.** `loginLocalAdmin(page)` → assert Header có link **"Control Plane"** → `goto /control-plane` → assert CP render (thấy element CP đã biết) và **KHÔNG** có "403 — Forbidden".
- **Test 2 — OIDC member bị CHẶN CP.** `loginViaOidcMember(page)` (helper mới: fill form Keycloak cho `member-sso@egret.local` / `Member12345!`, chờ callback về app — KHÔNG cần landing `/explore` nên không cần bind workspace) → `goto /control-plane` → assert **"403 — Forbidden"** hiện VÀ link Header **"Control Plane"** vắng.

### B. `frontend/tests/e2e/cp-admin.spec.ts` — access console (as admin)

Gate: skip trừ khi `EGRET_E2E_KIND=1`. `loginLocalAdmin` một lần (`beforeEach` hoặc mỗi test).

- **Test 1 — access console load đúng seed.** Vào `/control-plane/users`, `/groups`, `/roles`, `/permissions`; assert mỗi trang render bảng + entry seed đã biết: `admin@egret.local` trong Users, ≥1 role trong Roles, permission catalog non-empty trong Permissions.
- **Test 2 — RBAC group CRUD round-trip.** Ở `/control-plane/groups`: click **Create** → nhập tên unique `e2e-grp-${RUN_ID}` → submit → assert group hiện trong list. Rồi actions của group → **Delete** → confirm trong `ConfirmDeleteDialog` → assert group biến mất. (Gán role cho group là assertion **stretch** — chỉ thêm nếu flow UI gọn 2–3 bước; nếu không để slice sau.)
- **Test 3 — audit log ghi mutation.** `goto /control-plane/audit-log`; assert có entry tạo/xoá group (khớp tên `e2e-grp-${RUN_ID}` hoặc loại action).

### C. Helper — `loginViaOidcMember(page)` trong `helpers.ts`

Giống `loginViaOidc` nhưng cho identity member và không chờ `/explore`: vào `/login`, click "Sign in with SSO", fill form Keycloak (`EGRET_E2E_OIDC_MEMBER_USER` / `..._PASSWORD`, mặc định `member-sso@egret.local` / `Member12345!`), submit, chờ URL về app origin (callback OIDC resolve). Cred override qua env.

### D. Docs

Thêm mục "Auth + control-plane admin e2e" vào `frontend/tests/e2e/README.md` (2 spec assert gì; cùng lệnh `just e2e-kind`; test member-denied cần realm user `member-sso`, không cần RBAC SQL seed).

## Kiểm thử / acceptance

Không phải "đã viết test" — **chạy thật** qua `just e2e-kind` (hoặc `npx playwright test auth-gating cp-admin` vào deploy đang chạy) và xác nhận cả 2 spec PASS. Unit gates (`npm run lint`/`typecheck`/`test`/`build`) giữ xanh; spec opt-in, loại khỏi `npm test`.

## Rủi ro

1. **Timing login OIDC member** — form Keycloak + callback có thể chậm; helper chờ URL app-origin, không delay cứng. Giảm thiểu bằng `waitForURL` timeout rộng.
2. **Selector drift** ở bảng/dialog access console — gom flow Create/Delete trong spec; ưu tiên role/text selector ("Control Plane", "Create", "403 — Forbidden", tên group).
3. **Trễ propagation audit-log** — mutation có thể mất chút mới hiện; poll/reload trang audit với cap ngắn.
4. **Thiếu realm user member-sso** — nếu realm import của deploy cũ hơn seeded SSO users, Test 2 không login được; ghi làm prereq (fresh `deploy-kind-oidc` import chúng).

## Project 2 còn lại sau slice này

governance (policies/masking/Cedar) · provisioning-as-test · notebooks (Daft) · BI/SQL-Lab consumer UI. Mỗi cái spec → plan → impl riêng.
