# Kế hoạch triển khai — Data Governance Policies P1

> Bản chuẩn (code đầy đủ từng bước): `docs/en/plans/2026-07-06-data-governance-policies-p1.md`. File này là tổng quan tiếng Việt.

**Mục tiêu:** Ship phase 1 trang **Policies** ở Control Plane — gắn profile↔engine, danh sách policy Ranger **read-only** (tab Access/Masking/Row-filter) lái theo service-def Trino, và chuyển mục menu.

**Kiến trúc:** Module `policies` mới trong `egret-controlplane` (session-auth, `require_platform_admin`) đọc chung `egret_authz::AuthzStore` (cùng store router máy `/service` ghi). Profile = 1 Ranger service, vòng đời độc lập; engine link qua cột nullable `authz_service_id`, tạo-hoặc-link lúc provision. Trang React mô phỏng `ControlPlaneSecrets`.

**Spec:** `docs/vi/specs/2026-07-06-data-governance-policies-design.md`.

**Quy ước (theo memory):** thêm migration → **chạm `crates/egret-metastore/src/lib.rs`** để re-embed; backend qua `cargo fmt --all --check` + clippy; frontend qua `npm run lint && typecheck && test && build`; test DB **tự skip** khi thiếu `DATABASE_URL`; không chạy 2 `cargo test --workspace` đồng thời.

## Danh sách task (TDD, commit từng task)

1. **Migration `0058_engine_authz_service.sql` + `authz_service_id` trên engines** (`egret-catalog/src/service/embedded.rs`): thêm cột `BIGINT NULL REFERENCES authz_services(id) ON DELETE SET NULL`; thêm field vào struct `Engine`; thêm cột vào 3 query (`create_engine`/`list_engines`/`get_engine`); thêm `set_engine_authz_service`; chạm metastore.
2. **`AuthzStore::list_policies(service_id)`** (`egret-authz/src/store.rs`): trả **tất cả** policy (kể cả disabled) dạng `PolicyRow`, newest-first. Test DB-gated.
3. **Gắn `AuthzStore` vào `CpState`**: thêm dep `egret-authz`; field `authz` vào `CpState`; dựng ở `egret-bin/src/main.rs:1581` và test `routes_api.rs` `setup_full`.
4. **`policies.rs` — DTO `PolicySummary` + `summarize_policy`** (parser Ranger JSON, cả 3 loại: policyType 0/1/2 → access/masking/rowfilter; resource_path = catalog.schema.table.column; principals = users+groups+roles; permissions = accesses.type). Unit test không cần DB.
5. **Endpoint read-only + router**: `GET /policies` (list + lọc service/type/q), `GET /policies/{id}`, `GET /policies/services` (profile + engine(s) link), `GET /policies/service-def/{name}`. Thêm `From<AuthzError> for CpError`. Merge router. Route test DB-gated.
6. **Auto-bind lúc provision** (`engines.rs` `create_engine` ~1707): thêm field `authz_service_id: Option<i64>` vào `CreateEngineBody`; sau khi tạo engine → link service có sẵn (validate) hoặc `create_service` mới theo tên engine + `set_engine_authz_service`. Best-effort (lỗi bind không fail provision).
7. **Frontend API + menu + route**: `api/policies.ts` (`listProfiles`, `listPolicies`); chuyển **Policies** từ `EXPLORE_GROUPS` sang nhóm **Data Governance** trong `CONTROL_PLANE_GROUPS` (giữ **Data Lineage** ở Explore); thêm route `policies` trong `ControlPlaneRoutes` (`App.tsx`).
8. **Trang `Policies.tsx` + Vitest**: profile selector + tab loại + bảng (Name·Resource·Principals·Permissions·Enabled) + empty state; test mock `@/api/policies`.
9. **Full local CI + smoke thủ công**: fmt/clippy/test workspace + frontend gate; đăng nhập `admin@egret.local`/`Admin12345!`, mở Control Plane → Data Governance → Policies, kiểm selector + tab + policy seed hiện ra. Nói đúng cái đã chạy.

## Ngoài P1 (để P2/P3)
Create/update/delete; endpoint principals + picker; editor Masking/Row-filter; bundle config Trino (Lớp B) + test enforce harness Java.

## Điểm implementer cần xác nhận tại chỗ
- Tên biến thể `AuthzError` (task 5) — đọc `egret-authz/src/error.rs`, khớp arm.
- Tên helper session/`json_body` trong `routes_api.rs` (task 5, 6) — đọc đầu file, tái dùng đúng.
