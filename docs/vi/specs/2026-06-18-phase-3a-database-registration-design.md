# Phase 3a — Database Registration — Spec thiết kế

> **Trạng thái:** Bản tham khảo tiếng Việt · **Ngày:** 2026-06-18 · **Phiên bản:** 0.1
> ⚠️ Bản gốc (canonical) là tiếng Anh: [`docs/en/specs/2026-06-18-phase-3a-database-registration-design.md`](../../en/specs/2026-06-18-phase-3a-database-registration-design.md). Khác biệt thì ưu tiên bản tiếng Anh.
> Hiện thực nền tảng database-registry của [Kiến trúc gốc](../architecture/2026-06-17-egret-architecture-design.md) §10.1 / §9.2. Đây là hạng mục Phase 1 đã bị bỏ qua (chỉ ship identity 1a); là tiền đề cho query engine Phase 3b và SQL Lab 3c.

---

## 1. Mục tiêu & phạm vi

Đăng ký các database (engine query) mà Egret sẽ chạy query: lưu connection config, mã hoá credentials, grant database cho workspace, và xác minh kết nối on-demand. Chưa chạy query (đó là Phase 3b).

**Trong phạm vi**
- Hai crate mới: `egret-secrets` (mã hoá credential AES-256-GCM + `SecretStore` Postgres) và `egret-database` (CRUD registry, grant workspace, test-connection on-demand).
- **Engines:** Trino (REST) và StarRocks (MySQL). Một enum `engine`; test-connection probe từng engine theo cách riêng.
- **Credentials:** AES-256-GCM, master key từ config; mỗi secret có nonce + ciphertext lưu trong bảng `secrets`.
- **Test connection:** chạy `SELECT 1` thật (kiểm reachability + auth + query), không chỉ ping. Trino qua statement protocol; StarRocks qua `mysql_async`.
- **Grants:** database đăng ký toàn cục (platform admin) rồi grant cho workspace; Explorer list database được grant của workspace.
- Admin REST CRUD + test + grant; một endpoint list cho Explorer.

**Ngoài phạm vi (hoãn)**
- Chạy query, `JobManager`, `ResultStore`, cache kết quả — Phase 3b (`egret-engine`).
- SQL Lab, dataset, saved queries — Phase 3c (`egret-bi`).
- Health monitor định kỳ + bảng `database_health` + scheduler — sau (phase này chỉ test on-demand).
- KMS/Vault ngoài và xoay khoá — chỉ local config key (cột `key_id` để dành xoay khoá).
- Admin UI quản lý database (frontend) — phase Explorer/Admin frontend sau.
- Phát hiện capability engine.

## 2. Quyết định

| # | Quyết định | Lựa chọn | Căn cứ |
|---|---|---|---|
| 1 | Chẻ phase | 3a registration trước; engine = 3b, SQL Lab = 3c | Registration là tiền đề bị bỏ qua cho mọi việc query |
| 2 | Mã hoá credential | AES-256-GCM, master key từ config | Không cần hạ tầng ngoài; theo pattern `security.secret_key` sẵn có |
| 3 | Connectivity | "Test connection" on-demand (chưa health định kỳ) | Verify khi đăng ký, không cần scheduler |
| 4 | Engines | Trino (REST) + StarRocks (MySQL) | Cả hai là engine MVP của kiến trúc |
| A | Mức test | `SELECT 1` thật (auth + query), không phải ping | Bắt lỗi auth/quyền ngay lúc đăng ký |
| B | Client StarRocks | `mysql_async` (crate riêng), không phải feature `mysql` của sqlx | Giữ feature-set sqlx (`postgres`) nguyên; cô lập dep trong `egret-database` |

## 3. Kiến trúc / cấu trúc crate

```text
crates/
├── egret-secrets/
│   └── src/
│       ├── lib.rs
│       ├── crypto.rs    # AES-256-GCM seal()/open(): (nonce[12], ciphertext)
│       ├── store.rs     # SecretStore (Postgres): put/get/delete
│       ├── config.rs    # SecretsConfig { master_key }
│       └── error.rs     # SecretsError
│
└── egret-database/
    └── src/
        ├── lib.rs
        ├── model.rs     # enum Engine, Database row, ConnConfig, TestOutcome
        ├── repo.rs      # DatabaseRepo (CRUD bảng `databases`, seal creds qua SecretStore)
        ├── grants.rs    # workspace_database_grant CRUD + list theo grant
        ├── tester.rs    # test_connection(engine, conn, secret) từng engine
        ├── routes.rs    # admin CRUD/test/grant + me/ws list; DatabaseState
        └── error.rs     # DatabaseError
```

**Layering:** `egret-database::repo` dùng `egret-secrets::SecretStore` để seal/open creds; `tester` dùng `reqwest` (Trino) và `mysql_async` (StarRocks). Phụ thuộc `egret-{core,config,metastore,auth,workspace}` + `egret-secrets`.

## 4. `egret-secrets`

- **Config:** `[secrets] master_key = "<base64, ≥32 bytes>"`.
- **Crypto** (crate `aes-gcm`, `Aes256Gcm`): `seal(plaintext: &str) -> (nonce: [u8;12], ciphertext: Vec<u8>)` với nonce ngẫu nhiên; `open(nonce, ciphertext) -> String`. Ciphertext bị sửa hoặc sai key → fail ở GCM auth tag.
- **Store** trên `secrets(id, kind, nonce, ciphertext, key_id, created_at)`:
  - `put(kind: &str, plaintext: &str) -> Uuid`
  - `get(id: Uuid) -> String`
  - `delete(id: Uuid)`
  - `key_id` mặc định `"cfg"`; nhận diện master key đã seal secret (để dành xoay khoá).
- **Deps:** `aes-gcm`, `rand`, `base64`, `uuid`, `sqlx`, `thiserror`.

## 5. Data model (`migrations/0008_database.sql`)

```sql
CREATE TABLE secrets (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind       text NOT NULL,                       -- 'db_password' | 'db_token'
    nonce      bytea NOT NULL,
    ciphertext bytea NOT NULL,
    key_id     text NOT NULL DEFAULT 'cfg',
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE databases (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name       text NOT NULL UNIQUE,
    engine     text NOT NULL CHECK (engine IN ('trino','starrocks')),
    host       text NOT NULL,
    port       int  NOT NULL,
    options    jsonb NOT NULL DEFAULT '{}',         -- user, http_scheme, catalog/database, ssl…
    secret_id  uuid REFERENCES secrets(id) ON DELETE SET NULL,
    status     text NOT NULL DEFAULT 'unknown',     -- unknown | ok | error (lần test cuối)
    created_by uuid REFERENCES users(id),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE workspace_database_grant (
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    database_id  uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    granted_by   uuid REFERENCES users(id),
    granted_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, database_id)
);
```

## 6. Connection model & test

Field connection non-secret ở `options` (jsonb); chỉ password/token vào `secrets`.

| Engine | host/port | `options` | secret |
|---|---|---|---|
| Trino | host, 8080/8443 | `user`, `http_scheme` (http/https), `catalog?` | password/token (tùy chọn) |
| StarRocks | host, 9030 | `user`, `database?` | password |

**`test_connection` (Quyết định A — `SELECT 1` thật)** trả `TestOutcome { ok, latency_ms, detail }` và cập nhật `databases.status`:
- **Trino:** `POST {scheme}://{host}:{port}/v1/statement` body `SELECT 1`, header `X-Trino-User: <user>` (+ Basic/JWT từ secret nếu có); follow `nextUri` tới state `FINISHED` hoặc `FAILED`. `error` của payload `FAILED` thành `detail`. (Lát cắt tối thiểu của statement connector đầy đủ ở 3b.)
- **StarRocks (Quyết định B — `mysql_async`):** mở kết nối `host:port` với `user`/password/`database`, chạy `SELECT 1`, đọc một row.
- Timeout có giới hạn (connect + tổng, vd 10s) để host chết fail nhanh.

## 7. REST surface + RBAC

**Admin** — cần quyền quản lý-database mức platform (`platform_admin` hoặc `data_steward`). Mount `/api/admin` sau stack session + CSRF (như admin workspace routes).

| Method + path | Mục đích |
|---|---|
| `POST /api/admin/databases` | Đăng ký: `{ name, engine, host, port, options, secret? }` → seal `secret`, insert row. |
| `GET /api/admin/databases` | List (không secret; mỗi row có `has_secret: bool`). |
| `GET /api/admin/databases/{id}` | Detail (không secret). |
| `PUT /api/admin/databases/{id}` | Sửa; nếu có `secret` thì re-seal & thay (xoá secret cũ). |
| `DELETE /api/admin/databases/{id}` | Xoá (kèm secret). |
| `POST /api/admin/databases/{id}/test` | Test connection của database đã lưu. |
| `POST /api/admin/databases/test` | Test connection chưa lưu (body = config đầy đủ + secret) trước khi đăng ký. |
| `POST /api/admin/databases/{id}/grants` `{ workspace_id }` | Grant cho workspace. |
| `DELETE /api/admin/databases/{id}/grants/{workspace_id}` | Revoke. |

**Explorer** — `GET /api/me/databases?workspace=<slug>`: các database được grant cho workspace đó (DB picker của SQL Lab dùng ở 3c). Member workspace thấy grant của mình; `platform_admin` thấy tất cả. Trả `{ id, name, engine }` (không lộ chi tiết connection).

**Vệ sinh credential:** password/token plaintext chỉ nhận ở register/update; không endpoint đọc nào trả lại. Detail chỉ lộ `has_secret` và `options` non-secret.

## 8. Config + wiring

- **`egret-config`:** thêm `[secrets] master_key: String`. `validate()`: nếu non-empty thì phải base64-decode ≥32 bytes; đăng ký database **có** secret fail rõ khi `master_key` trống.
- **`egret-server`:** `AppState` thêm `database: Option<DatabaseState>`; mount admin database router `/api/admin` (gộp vào nhóm admin đã layer CSRF) + listing `/api/me/databases`.
- **`egret-bin`:** build `DatabaseState { repo: DatabaseRepo, secrets: SecretStore }` từ config + DB.

## 9. Test

- **`egret-secrets`:** unit — `seal`→`open` round-trip; lật 1 byte ciphertext → fail; key khác → `open` fail. Gate-DB — `put`/`get`/`delete` round-trip.
- **`egret-database`:** gate-DB — register (seal secret) / list (không secret, `has_secret` true) / update (xoay secret) / delete; grant + revoke; `/api/me/databases` chỉ trả database được grant. HTTP test qua `oneshot` cho router admin + me (gate auth).
- **Tester (gate-engine):** self-skip trừ khi set `EGRET_TEST_TRINO_URL` / `EGRET_TEST_STARROCKS_URL`; khi set (vd Trino/StarRocks trong `docker compose` dev), đăng ký engine đó và assert `test_connection` trả `ok = true`. Giống gate MinIO/Keycloak của catalog.
- **Config validate tests** (figment::Jail): kiểm độ dài master_key; register-with-secret không có key thì lỗi.

## 10. Phân rã cho plan

Task gợi ý: (1) scaffold crate `egret-secrets` + `egret-database` + workspace deps; (2) `egret-secrets::crypto` (AES-GCM seal/open + unit test); (3) `migrations/0008_database.sql` + `SecretStore` (gate-DB); (4) `egret-config` `[secrets]` + validate; (5) model `egret-database` + `DatabaseRepo` CRUD (seal creds) (gate-DB); (6) `grants` + list `/api/me/databases` theo grant (gate-DB); (7) `tester` — Trino REST `SELECT 1` (gate-engine); (8) `tester` — StarRocks `mysql_async` `SELECT 1` (gate-engine); (9) router admin + me + RBAC + map lỗi; (10) wiring `egret-server`/`egret-bin`; (11) verify cuối.

## 11. Tham khảo

- Kiến trúc: [Bản gốc](../architecture/2026-06-17-egret-architecture-design.md) §9.2 (data model database/secrets), §10.1 (`egret-database`), §10.2 (engine — tiêu thụ ở 3b), §8.2 (hai tầng authz — RBAC gate registration).
- Trino statement protocol: `POST /v1/statement` + polling `nextUri` (tái dùng đầy đủ ở `egret-engine` 3b).
- Pattern sẵn có: `egret-catalog` (config + `AppState` Option sub-state + test gate-DB/gate-engine), `egret-workspace::routes` (router admin/me + CSRF layer + RBAC).
