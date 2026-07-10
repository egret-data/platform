# B1b-3 — Migrate workspace seams sang `require()`

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-10-b1b-3-workspace-migration-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-10 · **Phiên bản:** 1.0
>
> **Tiền nhiệm:** [B1b-2 platform migration + forbid](./2026-07-10-b1b-2-platform-migration-forbid-design.md).
>
> Increment thứ tư của B1b. Migrate mọi **workspace** seam khỏi legacy `Authorizer::require_workspace` (và `Authorizer::require_platform` còn sót ở workspace seam) sang PDP `require()`. Thuần cơ học — không model authorization mới. Sau increment này, không handler nào trong codebase còn gate qua hai authorizer cũ.

---

## 1. Vì sao & scope

B1b-2 xong tầng platform. B1b-3 xong tầng workspace: ~86 site `Authorizer::require_workspace(&ctx, action)` trên bi (46), notebook (17), query (13), kernel (7), workspace (3), cộng 7 site `Authorizer::require_platform(&p, action)` trong `egret-workspace/src/routes.rs` (workspace CRUD + `user:read`). Dùng overload `&WorkspaceContext` của `AuthTarget` đã implement + unit-test ở B1b-1.

**Trong scope:** inject `Arc<dyn Pdp>` chung vào `BiState`, `NotebookState`, `KernelState`, `EngineState`, `DatabaseState`, `WorkspaceState`; swap mọi call gate `require_workspace`/`require_platform` sang `require()`; sửa test mỗi seam.
**Ngoài scope:** xoá field `authz: Authorizer` (còn cần — xem §3); xoá authorizer/primitive legacy, `Principal.permissions`, hay flip grant-writer (B1b-final); gom `set_platform_roles` vào forbid (B1b-final).

## 2. Migration (cơ học, pattern đã chốt)

Giống hệt B1b-2, dùng overload workspace:
- **Workspace gate:** `st.authz.require_workspace(&ctx, PERM).await?` → `egret_core::authz::require(&*st.pdp, &ctx, PERM).await?` (`&WorkspaceContext` → `Resource::Workspace(ctx.workspace_id)`).
- **Platform gate (chỉ workspace seam):** `st.authz.require_platform(&p, PERM).await?` → `egret_core::authz::require(&*st.pdp, &p, PERM).await?` (`&Principal` → `Resource::Platform`).
- Mỗi seam state thêm `pub pdp: std::sync::Arc<dyn egret_core::authz::Pdp>`, inject từ `pdp` var chung trong `egret-bin` (build ở B1b-1).

**Behavior-preserving:** quyết định Workspace của PDP == quyết định `require_workspace` legacy, và quyết định Platform == `require_platform` (đều chứng minh bởi regression matrix 60-cell B1a — cross cả platform lẫn workspace scope). Workspace-feature action (`dashboard:read`, `notebook:write`…) không phải admin-graph nên `forbid` B1b-2 không đụng. Deny vẫn map `Error::Forbidden` → 403.

## 3. Vì sao `authz: Authorizer` giữ nguyên

Mỗi workspace seam state giữ field `authz: Authorizer` dù method `require_workspace`/`require_platform` không còn được gọi: `WsContext` extractor (`egret-workspace/src/context.rs`) dùng `Authorizer::workspace_roles_for(user, ws)` để tính `workspace_role` của request (role display/precedence trên `WorkspaceContext`), qua `Authorizer: FromRef<S>`. Extractor đó không đổi ở đây. Nên `Authorizer` struct vẫn sống (cho `workspace_roles_for`); chỉ method gate thành unused (method `pub` → không dead-code warning). Xoá `Authorizer` hẳn — và viết lại cách suy ra `workspace_role` — là **B1b-final**.

## 4. Testing

- Mỗi seam: route test cũ xanh (cùng allow/deny), với real `CedarPdp` wire vào bất kỳ test assert 403 (seed role **trước** khi build PDP — snapshot lúc build; raw-SQL seed không phát `RolesChanged`), và inline `AllowAllPdp` stub chỉ nơi authz incidental. Thêm một assertion real-PDP mỗi seam: một member với read role được read (200), non-member / role không đủ bị deny (403).
- `catalog_drift` lint vẫn xanh (đã nhận `authz::require(`).
- Full local gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, FE `lint/typecheck/test/build`.

## 5. Câu hỏi mở (cho plan)

- Xác nhận tên biến state của handler mỗi seam + `WorkspaceContext` trong scope là `ctx` tại mỗi site `require_workspace` (chỗ dùng `&ctx`, chỗ `ctx`).
- `egret-bi` có 46 site — swap single-file lớn nhất; đảm bảo grep bắt hết (const như `DS_READ`, `CH_MANAGE`, `PERM` resolve ra literal — swap giữ nguyên tham số const, chỉ đổi call).
- Xác nhận workspace seam states phụ thuộc `egret-core` (cho `Arc<dyn Pdp>`) — có transitively; confirm dep trực tiếp để gọi tên trait.

---

*Design v1.0 (2026-07-10). Migration cơ học tầng workspace sang PDP `require()` qua overload `&WorkspaceContext`; field `Authorizer` giữ cho `WsContext` extractor tới B1b-final. Không model authorization mới.*
