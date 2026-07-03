# Warehouse Browse + grant picker — thiết kế (2026-07-03)

Bản tiếng Anh (canonical): `docs/en/specs/2026-07-03-warehouse-browse-design.md`.

**Mục tiêu:** Platform admin browse cấu trúc catalog của một warehouse embedded —
namespaces → tables → schema (cột + kiểu) — ngay trong trang Warehouse detail
(control-plane), và **chọn namespace/table từ browse** khi Add grant thay vì gõ tay.

Khôi phục lại browse + namespace/table grant picker mà PR #90 từng có
(`browse.rs` + `CatalogAccess.tsx`), đã bị gỡ ở EB-1 — làm lại trên kiến trúc
embedded-catalog + control-plane (Architecture B).

**Ngoài phạm vi:** browse lọc theo RBAC của từng client (admin thấy tất cả); ghi từ
browse (chỉ xem); browse phía workspace.

## Kiến trúc (khớp bản EN)

Gọi thẳng service Iceberg v1 của lakekeeper **in-process** trên `ApiContext` với
system metadata (admin thấy hết) — đúng cách `browse.rs` #90 cũ + đúng pattern
`WarehouseProvisioner`. Ba lớp:

1. `egret-catalog-engine::browse`: `list_namespaces` / `list_tabulars` /
   `load_table_schema` (nhận `engine_state` + `lakekeeper_warehouse_id`).
2. `egret-controlplane`: trait `WarehouseBrowser` trong `CpState.browser` + 3 route
   platform-admin (`GET /warehouses/{id}/namespaces`, `.../tables`, `.../tables/{table}`).
   Warehouse chưa provision → trả `[]` (namespaces/tables) / `404` (schema).
3. `egret-bin`: impl `WarehouseBrowser` giữ `engine_state` (như `LakekeeperProvisioner`).

## Frontend

- API client: `listNamespaces` / `listTabulars` / `loadSchema`.
- **Tab "Browse"** trong `WarehouseDetail`: namespace list → tables/views (badge) →
  panel schema (Name / Type / Required + partition/sort chips). Có empty states.
- **Grant picker** (`AddGrantDialog`): thay 2 ô text bằng 2 `Select` browse-fed —
  Namespace (+ "All namespaces") → Table (+ "All tables"). `securableType` **suy ra**
  từ độ sâu chọn (table > namespace > catalog); bỏ ô Securable. Giống `CatalogAccess.tsx`.

## Test
Route test (namespaces/tables/schema, 403 non-admin, warehouse chưa provision → `[]`);
frontend test (Browse tab + grant picker); manual browse `test-wh` (đã có namespace
`az_eng` + table từ pyiceberg) + add grant table-level qua picker.

Chi tiết đầy đủ (DTO, data flow, error handling) xem bản EN.
