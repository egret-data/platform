# OIDC làm auth mặc định cho dev + e2e — Kế hoạch triển khai (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-01-oidc-default-dev-e2e.md`. File này là bản tham chiếu rút gọn — code/step đầy đủ xem bản Anh.
> Spec: `docs/en/specs/2026-07-01-oidc-keycloak-default-dev-e2e-design.md`.

**Mục tiêu:** OIDC (Keycloak local) làm đường login mặc định cho dev + e2e; local email/password còn lại làm break-glass.

**Kiến trúc:** Backend OIDC + realm đã có. Plan: (1) bật OIDC trong `.env` dev, (2) login page ưu tiên SSO, ẩn form local sau break-glass, (3) thêm CLI helper idempotent bind IdP group → workspace (bước "admin phân quyền"), (4) viết lại notebook e2e đăng nhập qua Keycloak thật.

## Cấu trúc file
- `.env`, `.env.example` — thêm `EGRET_OIDC__*` + `EGRET_SECURITY__SECRET_KEY` (dev); giữ local admin. *(Task 1)*
- `crates/egret-config/src/lib.rs` — 1 unit test config OIDC dev validate. *(Task 1)*
- `crates/egret-bin/src/lib.rs` — helper `grant_group_to_workspace()`. *(Task 2)*
- `crates/egret-bin/tests/provisioning.rs` — integration test DB-gated. *(Task 2)*
- `crates/egret-bin/src/cli.rs`, `.../main.rs` — CLI `workspace add-group`. *(Task 3)*
- `frontend/src/auth/LoginPage.tsx` + `__tests__/LoginPage.test.tsx` — SSO chính + break-glass. *(Task 4)*
- `frontend/tests/e2e/helpers.ts`, `notebook.spec.ts`, `README.md` — login qua OIDC. *(Task 5)*

## Các task (TDD, mỗi bước 2–5 phút)
1. **Bật OIDC trong dev env** — thêm `EGRET_OIDC__*` (redirect `:5173`) + `SECRET_KEY` vào `.env*`; unit test `validate_ok_for_dev_oidc_config` (`cargo test -p egret-config`). Commit (chỉ `.env.example`, `.env` gitignored).
2. **Helper `grant_group_to_workspace(db, slug, group, role, admin_email)`** — upsert group + resolve workspace by slug + resolve admin by email + `MemberRepo::add(ws,"group",gid,role,admin.id)` (idempotent). Test: group `data-team` → `ws_editor` cho user trong group ⇒ `Authorizer::workspace_permissions` chứa `notebook:write`. TDD (fail → impl → pass).
3. **CLI `egret-bin workspace add-group --workspace --group --role`** — thêm `Command::Workspace{action}` + `WorkspaceAction::AddGroup` vào `cli.rs`, dispatch `workspace_cmd` trong `main.rs` gọi helper. Build + smoke.
4. **Login page** — `oidc_enabled`: nút SSO chính; form local ẩn, hiện sau nút "Admin sign-in (break-glass)". Vitest: password field không render tới khi bấm disclosure; SSO link có mặt. Giữ case OIDC-off (form luôn hiện).
5. **E2e qua Keycloak** — `helpers.ts`: `loginViaOidc()` (click SSO → điền form KC `name=username/password` → submit → chờ `/explore/{ws}/`) + `loginLocalAdmin()` break-glass. `notebook.spec` thay `login()` → `loginViaOidc()`. README: bước `workspace add-group` + creds `tester/pw`. `tsc --noEmit`.
6. **Verify** — `just up` (+Keycloak) → `init` admin → serve env OIDC → `npm run dev`; provision warehouse + `workspace add-group`; browser SSO tester + break-glass admin; `npm run test:e2e`; full CI local (fmt/clippy/test + FE lint/tsc/vitest/build). Kết thúc branch qua `finishing-a-development-branch`.

## Rủi ro
- `REDIRECT_URL` phải `:5173` (origin Vite) nếu không cookie rơi sai origin → login hỏng âm thầm.
- Selector Keycloak bền + timeout rộng (KC cold start).
- Xác nhận lại khi code: `User{id,email}`, `UserRepo::find_by_email/insert_oidc`, `MemberRepo::new`, `WorkspaceRepo::create(slug,name,created_by)`, `egret-bin` export `cli::WorkspaceAction`.
