# Catalog — table detail (enrich) — Thiết kế (tham chiếu VN)

> Bản chuẩn: `docs/en/specs/2026-06-28-catalog-table-detail-design.md`. Lệch thì theo bản tiếng Anh.

**Ngày:** 2026-06-28 · **Branch:** `feat/catalog-table-detail` (off main). **Sub-epic 3** của chương trình Catalog-console-parity (warehouse ✅ → namespace ✅ → **table detail** → tasks/stats → permissions). Tham chiếu: trang table "details" + "versioning" của console lakekeeper.

## Mục tiêu
Làm giàu trang table detail (đang hiện location/schema/snapshot-count/properties) để bám "details" + "versioning" của lakekeeper — **chỉ từ iceberg metadata egret đã load** (không chạy query): partition spec, sort order, snapshot history, format-version, last-updated.

## Hiện trạng
Browse warehouse-scoped: click table → `GET …/warehouses/{wid}/namespaces/{ns}/tables/{table}` → `table_detail` (load_table full metadata nhưng project thin DTO). `TableDetail` FE: `{name,location,current_schema_id,columns,current_snapshot_id,snapshot_count,properties}`. `TableMetadata` lakekeeper có đủ: schemas, partition_specs, sort_orders, snapshots, properties, format_version, location, last_updated_ms.

## Quyết định
1. **Backend** (`table_detail` + DTO) project thêm từ metadata: `format_version`, `last_updated_ms`; schema columns (type render nested); **partition spec** `[{name, source_column, transform}]`; **sort order** `[{column, transform, direction, null_order}]`; **snapshots** `[{snapshot_id, parent_snapshot_id, timestamp_ms, operation, summary:map}]` sort mới→cũ. Auth giữ `catalog:read`. snake_case.
2. **FE** (`TableDetail` + `TableDetailView`) single-scroll sectioned (KHÔNG nest tab): Header (ns/table + location · format v · last-updated); Schema (Column·Type·Nullable, type nested); Partition spec (Field·Source·Transform; "Unpartitioned" nếu rỗng); Sort order (ẩn nếu rỗng); Snapshots (Snapshot id·Time·Operation·summary; "No snapshots yet" nếu rỗng); Properties.

## Test
BE (DB+MinIO+lakekeeper): table có partition + snapshot → table_detail trả đủ; unpartitioned → rỗng; tạo snapshot thật nặng → fallback unit test trên TableMetadata dựng sẵn. FE (mock): render schema/partition/sort/snapshots/properties + empty-states + nested type. Manual + full local CI trước merge.

## Rủi ro
shape iceberg metadata (đọc kỹ accessors `TableMetadata`/`LoadTableResult`); nested type formatter (struct/list/map); summary keys khác nhau theo operation (show subset known); tạo snapshot thật trong test nặng → fallback unit test.

## Ngoài phạm vi
preview (sample rows — cần engine); files/manifests; health; view detail (path khác); permissions/tasks (sub-epic 4/5); edit properties.
