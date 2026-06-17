# Phase 2a — Iceberg REST Catalog — Spec thiết kế

> **Trạng thái:** Bản tham khảo tiếng Việt · **Ngày:** 2026-06-17 · **Phiên bản:** 0.1
> ⚠️ Bản gốc (canonical) là tiếng Anh: [`docs/en/specs/2026-06-17-phase-2a-iceberg-catalog-design.md`](../../en/specs/2026-06-17-phase-2a-iceberg-catalog-design.md). Khác biệt thì ưu tiên bản tiếng Anh.
> Hiện thực nửa **Iceberg REST Catalog** của [Kiến trúc gốc](../architecture/2026-06-17-egret-architecture-design.md) §10.3 / Phase 2. Nửa Apache Ranger (`egret-authz` + `egret-ranger`) là spec riêng (Phase 2b).
> Đã đối chiếu với bản tham chiếu `lakekeeper` (`data-portal/lakekeeper`).

---

## 1. Mục tiêu & phạm vi

Dựng một **Iceberg REST Catalog** đúng chuẩn để engine ngoài (Trino, StarRocks, Spark, pyiceberg) dùng Egret làm catalog: tạo namespace/table, commit snapshot, load & query — data file ở S3/MinIO truy cập qua **remote signing** do catalog cấp. Đây là nửa *data-plane* của Phase 2 (lõi governance); nửa policy Ranger là Phase 2b.

**Trong phạm vi**
- Hai crate mới: `egret-storage` (abstraction object-store + ký SigV4) và `egret-catalog` (HTTP Iceberg REST + `CatalogStore` Postgres). Một crate nhỏ `egret-iceberg-ext` giữ REST request/response types (bắt chước `iceberg-ext` của lakekeeper).
- Catalog **read-write đầy đủ**: namespace CRUD, table create/load/commit/drop/rename/register, view CRUD/rename. Commit dùng optimistic concurrency (`requirements` của Iceberg).
- **Postgres là nguồn sự thật** cho table metadata, lưu 1 cột `jsonb` `TableMetadata`; `metadata.json` cũng được ghi ra storage kèm pointer `metadata_location`.
- **Remote signing** (`POST /v1/aws/s3/sign`): catalog ký request S3 bằng creds của nó; engine không thấy creds thật.
- **Catalog global**, xác thực bằng bearer token tĩnh. Cách ly đa-workspace hoãn lại.
- Tái dùng crate apache `iceberg` (iceberg-rust) cho mô hình spec + áp metadata-update.
- Test bằng **pyiceberg client thật** (create → table → append → commit → load) trên MinIO qua testcontainers, kèm Rust integration (gate DB + MinIO) và unit.

**Ngoài phạm vi (hoãn)**
- Đa-workspace qua cơ chế `prefix`; enforce `workspace_catalog_grant` (tạo bảng nhưng chưa dùng).
- OAuth token endpoint (`/v1/oauth/tokens`) — engine dùng bearer token tĩnh.
- Multi-table transaction (`POST /v1/transactions/commit`).
- Vended STS credentials (`aws-sdk-sts` chỉ wire sẵn cho sau; Phase 2a dùng remote signing).
- Bảng metadata normalized (kiểu lakekeeper ~11 bảng) — tối ưu sau, thay cho JSONB blob.
- Admin UI catalog browser — phase này chỉ backend; UI ở phase governance/BI sau.
- Tạo bảng `stage-create` (trường được chấp nhận nhưng chưa xử lý ở phase này) — luồng staged/atomic-replace hoãn lại.
- Đổi tên view (`POST /v1/views/rename`) và đường commit/replace view — view lưu dạng metadata opaque ở phase này; vòng đời view đầy đủ hoãn lại.

## 2. Quyết định

| # | Quyết định | Lựa chọn | Căn cứ |
|---|---|---|---|
| 1 | Chẻ phase | Catalog trước (spec này); Ranger là Phase 2b | Policy tham chiếu resource catalog; storage là hạ tầng tái dùng |
| 2 | Năng lực | Read-write đầy đủ (namespace + table create/load/commit/drop/rename + views) | Khớp flow §11.4 |
| 3 | Storage creds | Vending qua **remote signing** (`/v1/aws/s3/sign`) | lakekeeper `io` hỗ trợ; engine không thấy creds |
| 4 | Lưu metadata | **Postgres-as-truth, 1 cột `jsonb` `TableMetadata`** + artifact `metadata.json` | lakekeeper normalize (~11 bảng); JSONB là MVP thực dụng cùng nguyên lý |
| 5 | Tenancy | **Global**, auth bearer-token tĩnh; cách ly workspace hoãn | Bề mặt nhỏ cho 2a |
| 6 | Metadata engine | Tái dùng apache `iceberg` + crate `egret-iceberg-ext` cho REST types | lakekeeper phụ thuộc `iceberg` + `iceberg-ext` |
| 7 | Storage SDK | `aws-sdk-s3` + `aws-sigv4` (+ `aws-sdk-sts` wire sẵn cho vended creds) | crate `io` lakekeeper dùng đúng bộ này |

## 3. Kiến trúc / cấu trúc crate

```text
crates/
├── egret-storage/
│   └── src/
│       ├── lib.rs              # trait ObjectStore: put/get/delete/list/head
│       ├── s3.rs              # S3Store (aws-sdk-s3, override endpoint MinIO, path-style)
│       ├── signer.rs          # sign_s3_request() qua aws-sigv4 (remote signing)
│       ├── location.rs        # layout warehouse: <warehouse>/<ns>/<table>-<uuid>/...
│       └── config.rs          # StorageConfig (endpoint, region, bucket, creds qua secrets)
│
├── egret-iceberg-ext/
│   └── src/
│       ├── lib.rs              # re-export iceberg::{TableUpdate, TableRequirement, ...}
│       ├── catalog.rs         # REST DTO: CreateNamespaceRequest, CreateTableRequest,
│       │                      #   CommitTableRequest, LoadTableResult, ConfigResponse,
│       │                      #   RenameTableRequest, ListTablesResponse, View DTO, ErrorModel
│       ├── sign.rs            # DTO S3SignRequest / S3SignResponse
│       └── validation.rs      # helper kiểm namespace/identifier
│
└── egret-catalog/
    └── src/
        ├── lib.rs
        ├── api/iceberg/        # handler axum, mỗi nhóm resource 1 module
        │   ├── config.rs      # GET /v1/config
        │   ├── namespaces.rs  # namespace CRUD + properties
        │   ├── tables.rs      # create/load/commit/drop/rename/register/metrics
        │   ├── views.rs       # view CRUD + rename
        │   └── sign.rs        # POST /v1/aws/s3/sign
        ├── service/
        │   ├── store.rs       # CatalogStore (Postgres): namespaces/tables/views
        │   ├── commit.rs      # load → kiểm requirements → apply updates → CAS write
        │   └── metadata.rs    # (de)serialize TableMetadata, ghi artifact metadata.json
        ├── auth.rs            # middleware bearer-token
        ├── error.rs          # CatalogError → JSON ErrorModel của Iceberg
        └── routes.rs         # iceberg_router() mount tại /catalog
```

**Layering** theo lakekeeper: `api/iceberg` (HTTP) → `service::CatalogStore` (Postgres) → `egret-storage` (file data/metadata). `egret-catalog` phụ thuộc `egret-iceberg-ext`, `egret-storage`, `egret-metastore`, `egret-config`, `egret-core`.

## 4. REST API surface (mount tại `/catalog`)

Tập con Iceberg REST Catalog OpenAPI. Mọi route cần bearer token hợp lệ (§7), kể cả config.

| Nhóm | Method + path | Ghi chú |
|---|---|---|
| Config | `GET /v1/config` | Trả `{ defaults, overrides }`. `overrides` mang `warehouse` + `s3.signer.uri` (+ `s3.remote-signing-enabled=true`) để client bật remote signing. |
| Namespace | `GET /v1/namespaces` | List; query `parent` cho lồng nhau. |
| | `POST /v1/namespaces` | Tạo kèm properties. |
| | `GET /v1/namespaces/{ns}` | Load properties. |
| | `HEAD /v1/namespaces/{ns}` | Tồn tại (200/404). |
| | `DELETE /v1/namespaces/{ns}` | Drop (404 nếu thiếu, 409 nếu còn bảng). |
| | `POST /v1/namespaces/{ns}/properties` | Cập nhật/xóa properties. |
| Table | `GET /v1/namespaces/{ns}/tables` | List. |
| | `POST /v1/namespaces/{ns}/tables` | Tạo (lưu ý: `stage-create` được chấp nhận nhưng hoãn — xem §1). Ghi metadata.json đầu + row. |
| | `GET /v1/namespaces/{ns}/tables/{t}` | Load → `LoadTableResult` (metadata + `config` có signer uri). |
| | `HEAD /v1/namespaces/{ns}/tables/{t}` | Tồn tại. |
| | `POST /v1/namespaces/{ns}/tables/{t}` | **Commit** — `{requirements, updates}`; CAS optimistic; 409 khi conflict. |
| | `DELETE /v1/namespaces/{ns}/tables/{t}` | Drop (`purgeRequested` best-effort). |
| | `POST /v1/tables/rename` | Rename/di chuyển giữa namespace. |
| | `POST /v1/namespaces/{ns}/register` | Đăng ký bảng có sẵn theo `metadata_location`. |
| | `POST /v1/namespaces/{ns}/tables/{t}/metrics` | Nhận & bỏ qua (200). |
| View | `GET/POST /v1/namespaces/{ns}/views`, `GET/HEAD/POST/DELETE /v1/namespaces/{ns}/views/{v}`, `POST /v1/views/rename` (rename hoãn — xem §1) | Dạng giống table; metadata view ở `catalog_views.metadata` jsonb. |
| Sign | `POST /v1/aws/s3/sign` | Body `S3SignRequest{ region, uri, method, headers, body? }` → `S3SignResponse{ uri, headers }`. |

Lỗi dùng JSON `ErrorModel` của Iceberg (`{ error: { message, type, code } }`) kèm HTTP status đúng (400/401/403/404/409/419/500).

## 5. Commit flow & optimistic concurrency

`POST /v1/namespaces/{ns}/tables/{t}` với `CommitTableRequest{ requirements, updates }`:

```text
1. BEGIN tx
2. SELECT metadata, current_version FROM catalog_tables
   WHERE namespace_id=? AND name=? FOR UPDATE          -- khóa row
3. Deserialize TableMetadata từ cột jsonb (crate iceberg).
4. Với mỗi requirement: assert với metadata hiện tại
   (assert-create / assert-ref-snapshot-id / assert-current-schema-id /
    assert-default-spec-id / assert-default-sort-order-id / assert-table-uuid /
    assert-last-assigned-field-id / assert-last-assigned-partition-id).
   Bất kỳ fail → ROLLBACK → 409 CommitFailedException.
5. Apply updates tuần tự qua TableMetadataBuilder
   (add-schema, set-current-schema, add-snapshot, set-snapshot-ref,
    set-properties, remove-properties, add-spec, add-sort-order, set-location, ...).
6. new_version = current_version + 1
7. metadata_location = <location>/metadata/<new_version>-<uuid>.metadata.json
8. egret-storage.put(metadata_location, serialize TableMetadata mới)  -- artifact
9. UPDATE catalog_tables SET metadata=?, metadata_location=?,
          current_snapshot_id=?, current_version=new_version, updated_at=now()
   WHERE id=? AND current_version=current_version     -- chốt CAS
10. INSERT catalog_table_metadata_log(table_id, metadata_location)
11. COMMIT  → trả LoadTableResult(metadata mới)
```

Khóa row `FOR UPDATE` + chốt CAS `current_version` chống commit đồng thời. Data & manifest file do **client** (engine/pyiceberg) ghi bằng request S3 đã remote-sign trước khi gửi commit; catalog chỉ ghi **metadata.json** của bảng và sở hữu pointer.

## 6. Remote signing (`egret-storage`)

Catalog quảng bá `s3.signer.uri = <egret>/catalog/v1/aws/s3/sign` và `s3.remote-signing-enabled=true` trong `/v1/config` và `LoadTableResult.config` từng bảng. Khi client cần đọc/ghi data file, nó gửi hình dạng request chưa ký tới endpoint sign:

```text
POST /catalog/v1/aws/s3/sign
{ "region": "us-east-1", "uri": "https://minio/warehouse/ns/t-uuid/data/f.parquet",
  "method": "PUT", "headers": { "x-amz-content-sha256": [...], ... } }
→ 200 { "uri": "...", "headers": { "Authorization": "AWS4-HMAC-SHA256 ...",
                                    "x-amz-date": [...], ... } }
```

`egret-storage::signer::sign_s3_request` dùng `aws-sigv4` tính chữ ký SigV4 trên request bằng creds S3 lưu của catalog, trả headers cần thêm. Client gửi request đã ký **thẳng** lên MinIO/S3 — Egret không nằm trong đường data, chỉ trong đường ký.

## 7. Auth & config

**Auth.** Middleware bearer-token trên `iceberg_router` kiểm `Authorization: Bearer <token>` với `config.catalog.auth_token` (so sánh constant-time). `GET /v1/config` cũng yêu cầu token để token sai fail sớm ngay call đầu. pyiceberg/engine cấu hình `token=<...>`. Follow-up: chuyển sang `api_keys` + OAuth `/v1/oauth/tokens` khi có tenancy.

**Thêm config (`egret-config`).**
```toml
[catalog]
enabled    = true
warehouse  = "s3://warehouse"      # base location đưa cho client
auth_token = "..."                 # bearer tĩnh (env/secret ở prod)

[storage]
endpoint   = "http://localhost:9000"   # MinIO; bỏ trống cho AWS thật
region     = "us-east-1"
bucket     = "warehouse"
access_key = "..."                 # từ env/secret
secret_key = "..."                 # từ env/secret
path_style = true                  # cần cho MinIO
```
`validate()` từ chối `catalog.enabled && (warehouse rỗng || auth_token rỗng || storage thiếu)`.

**Wiring server (`egret-server`).** `AppState` thêm `catalog: CatalogState`. `iceberg_router()` mount tại `/catalog` (anh em với `/api`, vì engine gọi trực tiếp, không qua stack CSRF/session của trình duyệt). Khi `catalog.enabled = false`, không mount router.

## 8. Data model (`migrations/0006_catalog.sql`)

```sql
create table catalog_namespaces (
  id          uuid primary key default gen_random_uuid(),
  levels      text[] not null,                 -- namespace Iceberg đa cấp
  properties  jsonb  not null default '{}',
  created_at  timestamptz not null default now(),
  unique (levels)
);

create table catalog_tables (
  id                        uuid primary key default gen_random_uuid(),
  namespace_id              uuid not null references catalog_namespaces(id) on delete restrict,
  name                      text not null,
  table_uuid                uuid not null,
  metadata                  jsonb not null,     -- toàn bộ TableMetadata (nguồn sự thật)
  metadata_location         text  not null,
  previous_metadata_location text,
  location                  text  not null,
  current_snapshot_id       bigint,
  current_version           int   not null default 0,   -- counter CAS optimistic
  created_at                timestamptz not null default now(),
  updated_at                timestamptz not null default now(),
  unique (namespace_id, name)
);

create table catalog_table_metadata_log (
  id                uuid primary key default gen_random_uuid(),
  table_id          uuid not null references catalog_tables(id) on delete cascade,
  metadata_location text not null,
  created_at        timestamptz not null default now()
);

create table catalog_views (
  id                uuid primary key default gen_random_uuid(),
  namespace_id      uuid not null references catalog_namespaces(id) on delete restrict,
  name              text not null,
  metadata          jsonb not null,
  metadata_location text not null,
  location          text not null,
  current_version   int  not null default 0,
  created_at        timestamptz not null default now(),
  updated_at        timestamptz not null default now(),
  unique (namespace_id, name)
);

-- tạo cho forward-compat; CHƯA enforce phase này (tenancy hoãn)
create table workspace_catalog_grant (
  workspace_id uuid not null references workspaces(id) on delete cascade,
  namespace_id uuid not null references catalog_namespaces(id) on delete cascade,
  granted_by   uuid references users(id),
  granted_at   timestamptz not null default now(),
  primary key (workspace_id, namespace_id)
);
```

## 8.1 Cân nhắc performance (JSONB-as-truth)

Lưu full `TableMetadata` thành 1 cột `jsonb` là đánh đổi có chủ đích. Theo access pattern:

- **`loadTable` (đường nóng nhất):** JSONB *thắng* normalized — response REST gần như chính là toàn bộ `TableMetadata`, nên đọc 1 blob rồi deserialize là tối ưu. Mô hình normalized của lakekeeper phải JOIN ~11 bảng rồi ráp lại, nhiều việc hơn cho đường đọc chủ đạo này.
- **`listTables` / `HEAD` / drop / rename:** không chạm blob. Các trường nóng (`name`, `metadata_location`, `current_snapshot_id`, `current_version`) là **cột thật**, nên Postgres chỉ de-TOAST `metadata` khi thực sự SELECT cột đó.
- **`commit` (điểm yếu thật):** đọc full blob → deserialize → apply → **serialize & ghi lại nguyên blob**, dù chỉ thêm 1 snapshot. Bảng có lịch sử snapshot dài → **write amplification O(history)**; normalized chỉ `INSERT` 1 row snapshot (O(1)).

**Blob phình** do mảng `snapshots` (~vài trăm byte mỗi commit). Bảng thường ở mức **KB** (không đáng kể). Bảng ghi-dày (streaming/hourly) giữ snapshot lâu có thể lên **MB**, lúc đó parse cost (loadTable QPS cao) + write amplification (commit) mới đáng kể. Postgres lưu `jsonb` > 2KB ở **TOAST** (nén lz4/pglz, out-of-line), nên blob to tốn de-TOAST + decompress + serde parse.

**Kết luận:** ổn ở scale Phase 2a / MVP. Bốn van an toàn giữ blob bị chặn:
1. **Tách cột nóng** (đã làm) — list/exists/pointer không đọc blob.
2. **Cache loadTable theo version** — cache metadata deserialize theo `(table_id, current_version)`; commit bump version nên cache tự invalidate, triệt tiêu parse lặp. (follow-up)
3. **Snapshot retention / `expireSnapshots`** — cơ chế Iceberg chuẩn, chặn kích thước blob.
4. **Escape hatch** — nếu xuất hiện workload ghi-dày, migrate ruột `CatalogStore` sang normalized snapshot table (kiểu lakekeeper). **REST layer không đổi**; chỉ store đổi — JSONB-as-truth không phải ngõ cụt.

## 9. Test

- **Gate pyiceberg client thật** (crate kiểu `tests/` hoặc `lakekeeper-integration-tests`): dựng MinIO + Egret qua testcontainers; chạy script pyiceberg `create_namespace → create_table(schema) → append(pyarrow table) → load_table → scan().to_arrow()` và assert lại data round-trip. Chứng minh protocol REST + remote signing end-to-end. Self-skip khi không có Docker (giống gate Keycloak).
- **Rust integration (gate DB + MinIO):** round-trip từng endpoint; namespace lồng; create + load; commit happy path; commit conflict → 409; rename giữa namespace; drop (có/không purge); view CRUD; register bảng có sẵn.
- **Unit:** `signer::sign_s3_request` sinh Authorization SigV4 hợp lệ cho cặp request/credential đã biết; `commit` apply update `add-snapshot` và bump snapshot ref; `CatalogStore` CAS từ chối `current_version` cũ.
- Test theo pattern gate-DB hiện có (self-skip nếu thiếu `DATABASE_URL`); test phụ thuộc MinIO gate theo endpoint `[storage]` truy cập được.

## 10. Phân rã cho plan

1. `egret-storage`: trait `ObjectStore` + `S3Store` (aws-sdk-s3, MinIO) + helper `location`; unit test put/get/list với MinIO (gate).
2. `egret-storage::signer`: `sign_s3_request` (aws-sigv4); unit test chữ ký với vector đã biết.
3. `egret-iceberg-ext`: REST DTO + re-export + validation; unit serde round-trip.
4. `migrations/0006_catalog.sql` + khung `CatalogStore` (namespace CRUD); test gate DB.
5. `CatalogStore` tables: create/load/list/drop/rename + CAS `current_version`; test gate DB.
6. `service::commit` + `service::metadata`: kiểm requirements + apply `TableMetadataBuilder` + ghi artifact; unit + test gate DB.
7. View trong store của `egret-catalog`; test gate DB.
8. Handler `api/iceberg` (config/namespaces/tables/views/sign) + map `error` + middleware `auth`; `iceberg_router()`.
9. `egret-config` `[catalog]` + `[storage]` + `validate()`; `egret-server` mount tại `/catalog`.
10. Gate pyiceberg client thật + verify cuối.

## 11. Tham khảo

- **lakekeeper** (`data-portal/lakekeeper`): crate `io` (aws-sdk-s3 + aws-sigv4 + aws-sdk-sts; s3 remote signing), `iceberg-ext` (REST DTO + spec + validation), `lakekeeper-storage-postgres` (metadata normalized — mô hình ta đơn giản hóa về JSONB), phụ thuộc `iceberg` (fork iceberg-rust).
- **apache iceberg-rust** (crate `iceberg`): `TableMetadata`, `TableMetadataBuilder`, `TableUpdate`, `TableRequirement`, spec types.
- **Iceberg REST Catalog OpenAPI**: hợp đồng endpoint ở §4.
- Kiến trúc: [Bản gốc](../architecture/2026-06-17-egret-architecture-design.md) §8.x (governance), §9.4 (data model catalog), §10.3 (catalog), §11.4 (commit flow).
