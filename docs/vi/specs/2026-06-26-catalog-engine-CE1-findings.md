# CE-1 Spike — Kết quả khảo sát (tham chiếu VN)

> Bản chuẩn (canonical) là tiếng Anh: `docs/en/specs/2026-06-26-catalog-engine-CE1-findings.md`.
> File này là bản tham chiếu rút gọn; khi lệch, lấy bản tiếng Anh làm chuẩn.

**Ngày:** 2026-06-26 · **Branch:** `feat/catalog-engine-lakekeeper`
**Kết quả: ✅ THIN-EMBED CHỨNG MINH KHẢ THI END-TO-END.** Router Iceberg REST của lakekeeper chạy in-process, round-trip một catalog op thật (config → tạo namespace → tạo table → load) qua store lakekeeper, lưu vào schema Postgres của nó, warehouse S3 validate với MinIO. Commit: `add deps` · `migrator` · `router` · `round-trip`.

## 1. Khả thi
- **Một `iceberg` 0.9.0 duy nhất** (cả hai pin rev `bb70173`). Không xung đột type.
- Build + round-trip pass (idempotent).

## 2. Coupling khi build (ghi nhận; dọn trước khi merge)
- **rustflags global** `tracing_unstable`/`tokio_unstable` (lakekeeper cần) — áp cho cả egret build; committed nên CI tự kế thừa.
- **sqlx 2 version:** egret 0.8, lakekeeper 0.9; `egret-catalog-engine` dùng 0.9 cho pool lakekeeper → **không share pool với egret được** (2 pool riêng). Hết khi egret nâng workspace lên 0.9.
- **SQLX_OFFLINE** + `SQLX_OFFLINE_DIR=../lakekeeper/.sqlx` (lakekeeper dùng macro `sqlx::query!`) — an toàn vì egret không dùng macro compile-time; nhưng **mong manh** (đường dẫn sibling).
- **Path deps → git pin** (trước merge): hiện path tới checkout sibling, không reproducible.
- **`test-utils` đang ở `[dependencies]`** (cần cho bootstrap principal) → CE-2 chuyển sang `[dev-dependencies]` + dùng admin principal thật.

## 3. Migrator coexistence — ĐÃ GIẢI (search_path)
Pool sqlx-0.9 với `SET search_path = lakekeeper, public`; tạo schema trước. 63 migration + `_sqlx_migrations` của lakekeeper vào schema `lakekeeper`; 24 của egret ở `public`. Không rò. **Không cần DB riêng.** API: `lakekeeper_storage_postgres::migrations::migrate_core_only(&pool)`.

## 4. Bề mặt nhúng — state + router (gọn, headless)
Router: `new_v1_full_router::<CatalogServer<…>, State<AllowAllAuthorizer, PostgresBackend, SecretsState>>().with_state(state)`. `State` 8 field dựng headless được; **`registered_task_queues` để RỖNG** (router chỉ đọc config, KHÔNG spawn worker). → **CE-6 phải đăng ký task queue + spawn worker** thì GC/expire/soft-delete mới chạy.

## 5. Định tuyến (CE-2/CE-3)
`{prefix}` = **UUID của warehouse** (không phải tên); lấy qua `GET /config?warehouse=<tên>` → `defaults.prefix`. Server thật nest dưới `/catalog/v1`.

## 6. Provisioning API (CE-2)
Các method trait management `bootstrap`/`create_project`/`create_warehouse`, chữ ký `(request, ApiContext, RequestMetadata)` — **luôn cần actor; authorizer được hỏi**. `bootstrap` seed default project. **`create_warehouse` round-trip S3 thật** (bucket phải tồn tại + ghi được). **Bootstrap cần principal CÓ user_id** (internal actor `user_id()=None` → lỗi `MissingUserId`) → CE-2 dùng admin principal thật. **Idempotency:** tra warehouse theo tên, đừng đoán theo lỗi create (`StorageProfileOverlap` là 400, không phải 409).

## 7. Tiêm principal = bridge CE-3
Router trần không có middleware authn → thêm layer `axum::from_fn` chèn `RequestMetadata` vào extension. Spike dùng `new_lakekeeper_internal(uuid)` (public, bypass authz). **CE-3:** thay bằng metadata dựng từ **token principal của egret** (+ workspace→project/warehouse, + scope). Layer này nằm trước → egret chặn được trước cả S3 sign.

## 8. Authz (CE-4)
Spike dùng `AllowAllAuthorizer`. Trait `Authorizer` là điểm thay: impl egret = RBAC workspace + token scope. Internal actor có `bypasses_control_plane_authz` = mẫu cho system/provisioning ops của egret.

## 9. Storage + secrets (CE-5)
`StorageProfile::S3` + `S3Credential::AccessKey` qua lakekeeper-io; MinIO chạy (S3Compat, path-style). Secrets dùng postgres `SecretsState` cho spike.

## 10. Chưa mount vào egret-server
Spike drive router qua `tower::oneshot` (không socket). Mount vào egret-server kéo cả cây dep lakekeeper vào crate đó → làm trong CE-2/CE-7 sau cờ config.

## Ảnh hưởng tới CE-2..8
CE-2: git-pin, admin bootstrap, idempotency theo tên, slug→warehouse(UUID), test-utils→dev, mount sau cờ. CE-3: layer principal từ token (+scope). CE-4: `Authorizer` = RBAC+scope, chặn trước sign. CE-5: storage/secrets. CE-6: đăng ký task queue + worker (registry rỗng chỉ phục vụ REST). CE-7/CE-8: UI/internal API + bỏ store cũ.
