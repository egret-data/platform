# Catalog — quản lý namespace (create / drop) — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-28-catalog-namespace-management-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-28 · **Branch:** `feat/catalog-namespaces` (off main). **Sub-epic 2** của chương trình Catalog-console-parity (warehouse ✅ → **namespace** → table/view detail → tasks/stats → permissions). Tham chiếu: console lakekeeper.

## Mục tiêu
Cho workspace admin **tạo / xoá namespace** (gồm nested) trong một warehouse, ngay trong tab **Browse** của warehouse detail. Catalog đã duyệt cây namespace; thêm 2 thao tác DDL này.

## Hiện trạng
Browse đã warehouse-scoped (sub-epic 1): `GET …/warehouses/{wid}/namespaces` flatten cây (BFS `list_namespaces`) → list dotted; `…/{wid}/namespaces/{ns}/tables`. lakekeeper có `create_namespace`/`drop_namespace`/`list_namespaces` (iceberg v1); namespace = `NamespaceIdent` (vector levels), egret render/parse dạng dotted (`parse_ns`/`from_vec`). `{wid}`→lakekeeper id qua `lakekeeper_id(ws,wid)` (ownership 404). FE `CatalogBrowse.tsx` (props {ws,wid}): panel trái Namespaces (dotted) → tables.

## Quyết định
1. **Backend** (browse.rs, dưới `/ws/{ws}/catalog/warehouses/{wid}`, gate `require_workspace_admin`, resolve {wid} ownership):
   - `POST …/{wid}/namespaces` `{parent: Option<String>, name: String}` → build NamespaceIdent = parent levels + name (name không được chứa dấu chấm → 400) → `create_namespace` (properties rỗng) → 201; trùng → 409; parent không tồn tại → 400/409.
   - `DELETE …/{wid}/namespaces/{ns}` (ns dotted, URL-encoded) → `drop_namespace`; iceberg chỉ xoá namespace **rỗng** → lakekeeper lỗi nếu còn table/child → map **409** "not empty"; 204 nếu OK.
2. **FE** (api/catalog.ts + CatalogBrowse.tsx): hooks `useCreateNamespace(ws,wid)` / `useDeleteNamespace(ws,wid)` (invalidate `useCatalogNamespaces`). Panel: nút **＋ New namespace** → dialog (Name + Parent Select các namespace hiện có, default "(root)"); mỗi row có action **Delete** → ConfirmDeleteDialog (cảnh báo chỉ xoá rỗng) → 409 not-empty surface inline. Refetch sau create/drop.

## Test
BE (DB+MinIO+lakekeeper, mirror warehouse_api fixture): create root + nested (xuất hiện); trùng → 409; name có dấu chấm → 400; drop empty → 204 (biến mất); drop non-empty → 409; cross-ws {wid} → 404; non-admin → 403. FE (mock): New-namespace dialog gọi create (root+nested); Delete gọi delete; 409 hiển thị; refetch. Manual live smoke + full local CI trước merge.

## Rủi ro
drop chỉ rỗng (không cascade — đúng iceberg, báo lỗi rõ); nested create cần parent tồn tại; {wid} ownership IDOR; list phẳng → nested hiện dạng dotted (tree view để sau).

## Ngoài phạm vi
namespace **properties** edit; rename namespace (iceberg không hỗ trợ); table/view create/drop (qua iceberg REST bằng token write, không phải UI); tree view phân cấp.
