# Workspace Admin — Catalog access + Storage (lớp 2) — Thiết kế (tham chiếu VN)

> Bản chuẩn (canonical) là tiếng Anh: `docs/en/specs/2026-06-27-workspace-admin-catalog-design.md`.
> Bản tham chiếu rút gọn; khi lệch lấy bản tiếng Anh làm chuẩn.

**Ngày:** 2026-06-27 · **Branch:** `feat/workspace-admin-catalog` (off main) · **Parent:** ADR-19 (mô hình quản lý 3 lớp).

## Mục tiêu
Dựng khu **Workspace Admin** (ADR-19 lớp 2 — cấu hình per-workspace, khu FE MỚI) và lấp 2 mục đầu: **Catalog access** (quản lý token catalog: mint/list/revoke + read/write + namespace_scope) và **Storage** (xem config storage). Hiện các tính năng này chỉ có CLI/backend. Trang **browse** catalog giữ nguyên (lớp 3, `explore/Catalog.tsx`).

## Quyết định (chốt)
- **Vị trí:** route mới `/explore/:workspace/settings/*` (mục "Settings" ở sidebar workspace) + sub-nav (Catalog access · Storage). KHÔNG nhét tab vào trang browse; KHÔNG vào `/admin/*` platform.
- **Token routes ở `egret-catalog-engine/src/browse.rs`** — `BrowseState` đã có `CatalogStore` + handle auth `WsContext` → không cần seam mới.
- Mọi endpoint quản lý gate `workspace:manage_members`.
- **Raw token hiện đúng 1 lần** (lúc mint); không lưu plaintext / không trả lại (chỉ lưu hash SHA-256).
- **403-graceful:** FE chưa có hook role per-ws (`useMe()` chỉ có platform roles) → mục Settings luôn vào được; API 403 → hiện notice "cần workspace admin". Ẩn theo role để sau.

## Backend
1. **tokens.rs:** thêm `revoke_token_for_workspace(id, workspace_id) -> bool` (`UPDATE … WHERE id=$1 AND workspace_id=$2 AND revoked_at IS NULL`) chống IDOR ([[api-nested-resource-ownership-scoping]]).
2. **browse.rs routes** (gate `workspace:manage_members`, `WsContext`):
   - `GET /ws/{ws}/catalog/tokens` → `[CatalogTokenDto{id,label,permissions,namespace_scope,created_at}]` (không hash).
   - `POST .../tokens` (`{label, permissions:["read"|"write"], namespace_scope}`) → map read→catalog:read/write→catalog:write (sai → 400; rỗng → 400) → `create_token` → `{token, raw}` (raw 1 lần).
   - `DELETE .../tokens/{id}` → `revoke_token_for_workspace`: true→204, false→404 (đóng IDOR).
3. **Storage:** `GET /api/ws/{ws}/storage` đã có (gate sẵn), FE chỉ tiêu thụ.

## Frontend
- **Routing/nav:** thêm `{label:"Settings", icon:Settings}` vào mảng `EXPLORE` (Sidebar.tsx); route lồng `settings` → `WorkspaceSettings` (index→catalog) + `settings/catalog`→`CatalogAccess`, `settings/storage`→`StorageSettings`. `WorkspaceSettings` = header + sub-nav (NavLink) + `<Outlet/>` (sub-nav bằng link list, không thêm Tabs primitive đang thiếu).
- **api/catalog.ts:** thêm type + hook `useCatalogTokens` / `useMintToken` / `useRevokeToken` / `useWorkspaceStorage` (TanStack Query, `apiJson`/`apiFetch`; lộ `ApiError.status===403`).
- **CatalogAccess.tsx:** bảng token (Label · Permissions badge · Namespaces `*` nếu rỗng · Created · Revoke) + nút Mint (dialog: label + checkbox Read/Write + Namespaces optional → submit → hiện raw 1 lần + copy + cảnh báo "không xem lại được"); Revoke qua confirm-delete-dialog; 403 → notice admin.
- **StorageSettings.tsx:** read-only `useWorkspaceStorage` (provider/bucket/endpoint/region/key_prefix/access_key/has_secret — KHÔNG secret); 404 → "dùng storage mặc định platform"; 403 → notice; ghi chú storage set lúc tạo workspace, chưa sửa được (CE-5).

## Test
- BE (DB+MinIO): mint→200 có raw + xuất hiện trong list (không hash); revoke→204 + biến mất; revoke id của ws khác→404 (IDOR); non-admin→403; permission sai→400.
- FE (Vitest mock `@/api/catalog`): WorkspaceSettings render sub-nav + chuyển trang; CatalogAccess render list, Mint dialog hiện raw 1 lần, revoke gọi mutation, 403 → notice; StorageSettings render không lộ secret, 404 → message default; Catalog.test.tsx (browse) vẫn xanh.

## Rủi ro
IDOR revoke (đóng bằng revoke_token_for_workspace + 404); raw-token leak (chỉ POST trả, render 1 lần, không lưu/log); FE chưa có role hook (API 403 là gate); auth parity (token handler phải dùng đúng `WsContext`+`require_workspace("workspace:manage_members")` như get_storage — test 403 + IDOR).

## Ngoài phạm vi
Members / DB-connections / external-catalog / workspace secrets (đợt sau cùng khu); ẩn nav theo role; sửa storage đã provisioned; trang admin/Catalog overview platform.
