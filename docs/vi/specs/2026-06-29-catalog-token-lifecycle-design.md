# Catalog — vòng đời token (edit + rotate) + scope tới table — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-29-catalog-token-lifecycle-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-29 · **Branch:** `feat/catalog-token-lifecycle` (off main). Sub-project B của Catalog access (sau Browse redesign). Lấp các lỗ hổng quản lý token: sửa, rotate, scope mịn tới table.

## Mục tiêu
Token catalog quản lý đầy đủ: **sửa** label/permissions/scope tại chỗ, **rotate** secret, và scope xuống **table cụ thể** (không chỉ namespace).

## Hiện trạng
`catalog_tokens(id, workspace_id, warehouse_id, token_hash, label, permissions[], namespace_scope[], created_at, revoked_at)` — chỉ lưu **SHA-256 hash** của raw `egcat_…` (raw hiện 1 lần khi mint). `tokens.rs`: create/list/revoke + hash_token + generate_token. Chuỗi auth: front-auth hash token → `TokenAuth{workspace_id, warehouse_id, permissions, namespace_scope}` → claim `{egret:{perms, ns}}` → `EgretAuthorizer.decide_scope` gate theo **namespace-prefix**. Gate table-action có sẵn `TableIdent` qua `action_on_table.info.table_ident()`. Routes token ở `browse.rs`: GET/POST/DELETE (gated `workspace:manage_members`). FE `CatalogAccess.tsx`: mint + list + revoke.

## Quyết định
1. **Edit (không re-issue).** `PUT …/tokens/{id}` (gated manage_members, scope theo `(id, workspace_id, warehouse_id)` → 404 cross-tenant): UPDATE `label/permissions/namespace_scope/table_scope`; **token_hash giữ nguyên** → secret cũ vẫn chạy với scope mới, hiệu lực ở request kế tiếp. `tokens.rs::update_token`. FE: dialog Edit pre-fill từ row.
2. **Rotate.** `POST …/tokens/{id}/rotate`: sinh raw mới, ghi đè `token_hash` cùng id (giữ label/scope/created_at, set `rotated_at`), trả `{token, raw}` **1 lần**; secret cũ chết ngay. DB thêm `rotated_at`. `tokens.rs::rotate_token`. FE: action Rotate dùng lại UI reveal của mint.
3. **Scope tới table.** DB: thêm `table_scope text[] NOT NULL DEFAULT '{}'` (migration `0035`). Thread `table_scope` qua `CatalogToken`/`TokenAuth`/create+update; claim thành `{egret:{perms, ns, tables}}`. **Enforcement:** `egret_claim` parse thêm `tables`; gate **table-action** khi `table_scope` khác rỗng yêu cầu FQN của table đích — `ns_levels.join(".")+"."+table` từ `info.table_ident()` — nằm trong `table_scope`, **cộng thêm** perm + namespace_scope. Rỗng = không hạn chế (như hiện tại). Namespace/view/generic-table gate giữ nguyên. **Giới hạn (đã kiểm chứng live):** `list_tables` của lakekeeper KHÔNG chạy authz từng table → token table-scoped **vẫn thấy tên table khác** khi list. Ranh giới enforce là **truy cập**: load/read/write/commit table ngoài `table_scope` bị từ chối (404/403). Lọc danh sách REST là future. UI browse admin vốn đã gate `workspace:manage_members`. **Ngữ nghĩa:** namespace_scope và table_scope độc lập; table action cần cả hai pass nếu cả hai được set; table_scope **chỉ áp cho table** (view theo namespace_scope). **UI:** mint + edit thêm field `table_scope` — **free-text `namespace.table`** (mỗi dòng/phẩy), validate dotted ≥2 đoạn. Picker là future.
4. **Components:** migration `0035`; `tokens.rs` (table_scope, update_token, rotate_token); `TokenAuth`; `front_auth.rs` (claim tables); `authorizer.rs` (parse tables + gate table FQN); `browse.rs` (PUT + rotate, mint nhận table_scope); `api/catalog.ts` + `CatalogAccess.tsx` (edit dialog, rotate, field table_scope).

## Test
BE integration (mirror `catalog_tokens_api.rs`): mint kèm table_scope; edit đổi scope → enforce ngay request sau; rotate → raw cũ 401, raw mới chạy, id/label giữ; token table-scoped đọc được table của nó nhưng **bị từ chối** table anh em cùng namespace (load/read/write → 404/403); token read-only ghi → 403; (lưu ý: tên table anh em VẪN có thể xuất hiện trong `list_tables` — list không lọc authz, xem QĐ3); token namespace-only không bị ảnh hưởng; cross-ws/wh `{id}` ở PUT/rotate → 404; non-admin → 403. Unit authorizer: thêm case match/deny theo table FQN. FE Vitest: edit pre-fill + submit; rotate reveal; parse free-text table_scope; loại entry sai (1 đoạn). **Manual real-Trino integration test** trước khi merge (quy tắc cho thay đổi authz).

## Rủi ro
Đụng đường enforcement authz (lỗi logic → cấp sai quyền) — giảm thiểu: table_scope rỗng giữ nguyên hành vi cũ + test allow/deny + list filter + chạy Trino test. Claim phình khi nhiều table (soft limit). Edit có hiệu lực ngay → thu hẹp scope có thể làm gãy client đang chạy (đúng ý đồ, như revoke).

## Ngoài phạm vi
Scope theo tên view; wildcard `ns.*`; UI picker table; token expiry/TTL; last-used/audit.
