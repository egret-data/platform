# B1b-final — Dọn dẹp hợp nhất enforcement

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-10-b1b-final-cleanup-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-10 · **Phiên bản:** 1.0
>
> **Tiền nhiệm:** [B1b-3 workspace migration](./2026-07-10-b1b-3-workspace-migration-design.md).
>
> Increment cuối của PEP cutover B1b. Xoá primitive gate legacy đã chết + fold inline admin check cuối (`set_platform_roles`) vào admin-graph `forbid`. Sau increment này, **mọi enforcement authorization đi qua PDP `require()`**. Cleanup rủi ro thấp.

---

## 1. Vì sao & scope

B1b-1/2/3 đã migrate mọi handler sang `egret_core::authz::require()`. Hai primitive gate legacy + helper của chúng giờ không handler nào tham chiếu (chỉ sống dạng `pub` re-export → không dead-code warning). Một mutation admin-only — `set_platform_roles` trong `egret-workspace` — vẫn dùng inline `platform_roles.contains("platform_admin")` (review follow-up của B1b-2). Increment này hoàn tất hợp nhất.

**Trong scope:** fold `set_platform_roles` vào admin-graph `forbid`; xoá `egret_auth::{require_permission, require_platform_admin}` chết, các gate method + helper permission đã chết của workspace `Authorizer`, và wrapper admin của `egret_controlplane::auth`.
**Ngoài scope (defer sang grant-model của B2):** **flip data-model** expand→contract — trỏ grant writers sang `access_grants`, suy `Principal.platform_roles`/`workspace_role` từ `access_grants`, drop trigger `0068`/`0069` + 2 bảng legacy (`platform_role_assignments`, `workspace_members`). Rủi ro cao hơn (đụng identity layer), độc lập (trigger giữ `access_grants` đúng trong lúc chờ); B2 dù sao cũng rework grant model nên flip thuộc về đó.

## 2. Giữ nguyên (nói rõ — KHÔNG xoá)

- **`Principal.permissions`** — KHÔNG chết. Nó serialize ra frontend qua `/me` (`egret-auth/src/routes.rs:196` → `MeResponse.permissions`) và điều khiển FE gating UI permission-aware. Không còn là input enforcement (PDP enforce) nhưng vẫn là hint cho FE. Giữ field, `SessionService::permissions()`, và field `/me`.
- **`Principal.platform_roles`** + `WorkspaceContext::is_platform_admin` — còn dùng bởi `WsContext` extractor (admin thấy display role `platform_admin`) và `list_my_workspaces` (admin thấy mọi workspace — filter display, không phải gate). Giữ.
- **`Authorizer::{new, workspace_roles_for, replace_platform_roles}`** — `workspace_roles_for` gọi bởi `WsContext` (`context.rs:57`); `replace_platform_roles` là logic ghi sau `set_platform_roles` (`routes.rs:400`). Giữ struct + 3 cái này; chỉ xoá gate/permission method chết (§3).
- Bảng legacy, trigger, và `access_grants` read-model — giữ tất (flip là B2).

## 3. Xoá (đều verified chết — không handler tham chiếu)

- `egret_auth::authz::require_permission` + `require_platform_admin` (+ test `#[cfg(test)]`). `egret-auth/src/authz.rs` chỉ chứa hai cái này — xoá file, gỡ `pub mod authz;` + re-export `pub use authz::{...}` trong `egret-auth/src/lib.rs`. (Thân `require_permission` đọc `principal.permissions`, nhưng field đó giữ cho `/me`; chỉ hàm đi.)
- `egret_workspace::rbac::Authorizer::{require_workspace, require_platform, workspace_permissions, platform_permissions}` — `require_workspace`/`require_platform` không caller ngoài `rbac.rs`; `workspace_permissions`/`platform_permissions` chỉ gọi bởi hai cái đó. Xoá cả bốn; giữ `new`/`workspace_roles_for`/`replace_platform_roles`.
- Wrapper `egret_controlplane::auth::require_platform_admin` + test. (Wrapper `require_permission` đã xoá ở B1b-2.) Nếu `auth.rs` còn lại vô nghĩa, xoá file + gỡ khai báo `mod`.

## 4. `set_platform_roles` → admin-graph `forbid`

`set_platform_roles` (`egret-workspace/src/routes.rs:384`) gán platform role (kể cả `platform_admin`) — mutation authz-graph nhạy cảm nhất. Hôm nay gate inline `if !p.platform_roles.contains("platform_admin") { Forbidden }`. Fold vào mô hình hợp nhất:
- Thêm `"user:assign_platform_role"` vào `egret_core::rbac::ADMIN_GRAPH_ACTIONS` (`forbid` sinh từ list này nên tự động phủ — chỉ cho `platform_admin`, non-delegatable by construction). KHÔNG thêm vào grantable catalog (nhất quán với admin-graph action khác).
- Thay check inline bằng `egret_core::authz::require(&*st.pdp, &p, "user:assign_platform_role").await?` (`WorkspaceState` đã mang `pdp` từ B1b-3).
- **Behavior-preserving:** `forbid`-unless-`platform_admin` cho đúng allow/deny như check inline. Thêm test: `platform_admin` set platform roles (2xx); non-admin (kể cả giữ mọi catalog permission) bị deny (403).
- Để nguyên filter `is_admin` của `list_my_workspaces` — nó là quyết định display, không phải gate.

## 5. Testing

- Test forbid `set_platform_roles` (§4).
- Test admin-graph forbid (từ B1b-2) vẫn pass; mở rộng gồm `user:assign_platform_role` nếu tiện.
- Removals: `cargo build --workspace` + `cargo clippy --workspace --all-targets -- -D warnings` chứng minh không gì tham chiếu item đã xoá.
- Full local gate: fmt, clippy, `cargo test --workspace`, FE `lint/typecheck/test/build` (field `/me` `permissions` không đổi nên không tác động FE — verify).

## 6. Câu hỏi mở (cho plan)

- Xác nhận `egret-auth/src/authz.rs` CHỈ chứa hai hàm (để xoá được file) — nếu có gì khác, chỉ xoá hai hàm.
- Xác nhận không test nào import `egret_auth::require_permission`/`require_platform_admin` (sửa/xoá test đó).
- Chuỗi action chính xác cho `set_platform_roles` — `user:assign_platform_role` (đề xuất); đảm bảo thêm vào `ADMIN_GRAPH_ACTIONS` để `forbid` phủ, và không route nào dùng nó làm bare permission gate.

---

*Design v1.0 (2026-07-10). Hoàn tất hợp nhất enforcement B1b: xoá primitive gate legacy chết + fold `set_platform_roles` vào admin-graph `forbid`. Flip data-model (drop bảng grant legacy) defer sang B2. `Principal.permissions` giữ — nó feed frontend, không phải enforcement.*
