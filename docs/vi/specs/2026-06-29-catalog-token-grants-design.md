# Catalog — token scope v2: resource grant (per-grant read/write) + picker — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-29-catalog-token-grants-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-29 · **Branch:** `feat/catalog-token-grants` (off main). Thay model `namespace_scope`+`table_scope` (PR #89, migration 0035) bằng model **resource-grant** đánh giá OR + UI builder dùng selectbox.

## Vấn đề
Model đã ship AND hai mảng phẳng → 2 lỗ hổng: (1) không có picker (free-text); (2) **không trộn được resource** ("toàn bộ A" + "chỉ B.table2") vì `table_scope` là bộ lọc toàn cục + AND với namespace_scope. Đã kiểm chứng live.

## Mục tiêu
Token scope = danh sách **resource grant** đánh giá OR, mỗi grant có read/write riêng, dựng bằng cách chọn namespace/table có sẵn.

## Model
- **Grant** = `{ namespace, table, permissions }`: `namespace` = dotted prefix (subtree) hoặc sentinel `"*"` = tất cả; `table` = `null` (tất cả table trong ns) hoặc tên table cụ thể; `permissions` = tập con read/write **theo grant**.
- **Token scope = `Grant[]`**, OR: cho phép nếu khớp BẤT KỲ grant nào + đủ perm. **Rỗng = không quyền** (least-privilege). "Đọc tất cả" = `[{"*", null, ["read"]}]`. Giải case: `[{sale,null,[read]}, {marketing,campaigns,[read,write]}]`.

## Enforcement (`EgretAuthorizer`, OR)
`covers_ns(g, target)` = `g.namespace=="*"` ∨ `ns_prefix_matches(g.namespace, target)`.
- **Table** (ns, name, write?): ∃ grant đủ perm, `covers_ns` ∧ (`table None` ∨ `==name`).
- **Namespace READ** (list/metadata): ∃ grant read "liên quan" — `covers_ns(g,target)` ∨ `ns_prefix_matches(target, g.namespace)` (target là tổ tiên của grant → duyệt xuống tới table được cấp). Nhờ vậy grant chỉ-table (B.table2) vẫn dùng được (list B + root).
- **Namespace WRITE** (create/delete): chỉ khi ∃ grant write whole-namespace (`table None`) phủ target.
- **View / generic**: ∃ grant whole-namespace phủ ns đó.
- System principal bypass giữ nguyên. front-auth coarse gate dùng **union** perms.

## Lưu trữ / wire
- **DB (migration 0036)**: thêm `scope jsonb NOT NULL DEFAULT '[]'` (list grant, perm full `catalog:*`); backfill từ cột cũ (cả-rỗng→`*` all với perms cũ; mỗi ns→whole-ns grant; mỗi FQN→table grant) rồi **DROP** `permissions/namespace_scope/table_scope`. AND→OR chỉ ảnh hưởng token có CẢ hai list (thực tế chưa có).
- **Claim**: `{egret:{grants:[{ns,table,perms}]}}`.
- **API**: grant dùng perm **ngắn** `("read"|"write")[]` cả 2 chiều (backend map ngắn↔full ở biên). Request `{label, scope:[{namespace,table?,permissions}]}`; DTO `{id,label,scope:[...],created_at,rotated_at?}`.

## Components
Migration 0036; `tokens.rs` (`Grant`, `CatalogToken.scope`, `TokenAuth{grants, permissions=union}`, create/update nhận `&[Grant]`, authenticate parse `scope`, `validate_grants`); `front_auth.rs` (claim grants + coarse union); `authorizer.rs` (parse grants + quyết định OR cho table/namespace/view/generic); `browse.rs` (DTO.scope, Mint/Update request.scope, map ngắn↔full, validate ≥1 grant); `api/catalog.ts` + `CatalogAccess.tsx` (grant builder: dòng namespace-select + table-select + Read/Write + xoá, "＋ Add resource", ≥1; picker namespace từ `useCatalogNamespaces`, table từ `useNamespaceContents(ns)`; list tóm tắt grant).

## Test
Authorizer unit (ma trận OR allow/deny: mixed A.* read + B.t2 read/write, deny A write, deny C, `*`, navigation list B cho grant chỉ-table, deny create-table B cho grant chỉ-table, rỗng=deny); store round-trip + migration backfill (3 case); integration mint/edit multi-grant + edit giữ secret + validate ≥1 (rỗng→400) + cross-wh 404; FE grant builder (add/remove, table-select theo namespace, ≥1, submit scope, list summary); manual Trino + focused live REST e2e mixed-grant.

## Rủi ro
Rewrite authz (thay gate, không mở rộng) → test ma trận OR + live e2e + Trino test + helper thuần nhỏ. Migration AND→OR chỉ ảnh hưởng token có cả 2 list (chưa có). Rule navigation (target là tổ tiên của grant) cần thiết để grant chỉ-table dùng được — có unit test.

## Ngoài phạm vi
Wildcard ngoài `*`-all; column-level; lọc list theo grant (giới hạn đã biết còn đó); expiry/TTL.
