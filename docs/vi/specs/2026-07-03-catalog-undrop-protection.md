# Catalog Undrop + Protection + View detail — thiết kế (2026-07-03)

Bản tiếng Anh (canonical): `docs/en/specs/2026-07-03-catalog-undrop-protection.md`.

**Mục tiêu:** Expose 3 khả năng của management API lakekeeper mà embedded catalog
đã hỗ trợ sẵn nhưng egret chưa dùng:

1. **Undrop / khôi phục** — bảng/view bị drop là soft-delete (GC queue purge sau
   TTL); cho platform admin **xem danh sách bảng đã xoá và khôi phục** trước khi purge.
2. **Protection** — bật/tắt cờ **protected** trên table/view/namespace để chống drop nhầm.
3. **View detail** — cây Browse liệt kê view nhưng click vào thì lỗi (`load_table`
   chỉ cho table); thay bằng load **schema của view**.

## Kiến trúc
Gọi management API của lakekeeper in-process trên `EngineApiServer` với system
metadata (giống browse/provisioning). Warehouse provision với **delete-profile
Soft (TTL 7 ngày)** để undrop có ý nghĩa. Browse list bật `return_uuids` +
`return_protection_status` để mỗi namespace/tabular mang sẵn `id` + `protected`.

- Backend: `egret-catalog-engine::recovery` (list/undrop/set_protection) +
  `browse::load_view_detail`; `egret-controlplane` routes (deleted-tabulars,
  undrop, protection, view schema); `egret-bin` impl `WarehouseBrowser` mở rộng.
- Frontend (Browse tab): shield toggle protect trên namespace + table/view detail;
  view load schema; nút "Recycle bin" mở dialog list bảng đã xoá + nút Undrop.

Chi tiết đầy đủ xem bản EN.
