# Kế hoạch triển khai — Data Governance Policies P3

> Bản chuẩn (code đầy đủ từng bước): `docs/en/plans/2026-07-06-data-governance-policies-p3.md`. File này là tổng quan tiếng Việt.

**Mục tiêu:** Hoàn tất editor Policies đạt parity Ranger đủ 3 loại — thêm editor **Column-masking** (policyType 1) và **Row-filter** (policyType 2) bên cạnh Access của P2 — và giao **bundle "Download Trino config" (Lớp B)** để coordinator Trino enforce policy qua plugin `trino-ranger` native.

**Kiến trúc:** `ranger_json.rs` thêm `masking_to_ranger_json` + `rowfilter_to_ranger_json` + dispatcher `editor_to_ranger_json` theo field `kind` (thiếu → mặc định access, giữ tương thích P2). Handler create/update nhận `Json<Value>` và dispatch. Endpoint mới `GET /policies/services/{service}/trino-config` sinh 2 file config từ `CpState.engine_egret_url` + tên service. Hai dialog React mới (Masking, Row-filter) tái dùng `PrincipalPicker`; list thêm New/Edit/Delete cho cả 3 tab + nút "Download Trino config".

**Phụ thuộc P1+P2** (nhánh `feat/data-governance-policies`).

**Sự thật Ranger (từ `service-defs/trino.json`):**
- Masking = policyType 1; resource catalog/schema/table/column; item trong `dataMaskPolicyItems:[{users,groups,accesses:[{type:"select",isAllowed:true}],dataMaskInfo:{dataMaskType,valueExpr?}}]`. maskTypes: MASK, MASK_SHOW_LAST_4/FIRST_4, MASK_HASH, MASK_NULL, MASK_NONE, MASK_DATE_SHOW_YEAR, CUSTOM (valueExpr).
- Row-filter = policyType 2; resource catalog/schema/table (KHÔNG column); item trong `rowFilterPolicyItems:[{...,rowFilterInfo:{filterExpr}}]`.
- Plugin `trino-ranger` native (có sẵn trong Trino chuẩn): `access-control.properties` (`access-control.name=ranger` + `ranger.service.name` + `ranger.plugin.config.resource`) + `ranger-trino-security.xml` (`ranger.plugin.trino.policy.rest.url` → base URL egret phục vụ `/service`). Token mặc định rỗng → `/service` anonymous, plugin đọc được.

**Quy ước:** DB test tự-skip; `fmt --check` + `clippy --workspace -D warnings`; frontend `lint/typecheck/test/build`; KHÔNG `cargo build -p egret-controlplane` đơn lẻ.

## Danh sách task (TDD, commit từng task)

1. **Serializer masking + rowfilter + dispatcher** (`ranger_json.rs`): `MaskingPolicyInput`/`RowFilterPolicyInput` + `masking_to_ranger_json`/`rowfilter_to_ranger_json` + `editor_to_ranger_json` (dispatch theo `kind`, thiếu→access). Round-trip test qua `summarize_policy`, test dispatcher + CUSTOM valueExpr.
2. **Dispatch create/update theo kind**: handler nhận `Json<Value>` → `editor_to_ranger_json`. Route test masking + rowfilter (list lọc theo type). P2 access (không có `kind`) vẫn chạy nhờ mặc định.
3. **Endpoint Lớp B** `GET /policies/services/{service}/trino-config` → 2 file (`access-control.properties` + `ranger-trino-security.xml`) từ `engine_egret_url` + service. Route test.
4. **Frontend api**: types `MaskingPolicyInput`/`RowFilterPolicyInput` + `PolicyInput` (kind) + `MASK_TYPES` + `getTrinoConfig`; `createPolicy`/`updatePolicy` nhận `PolicyInput`; Access editor thêm `kind:"access"`.
5. **`MaskingPolicyDialog`**: như Access nhưng per-item là `Select` maskType (+ `Input` valueExpr khi CUSTOM), resource 4 level. Vitest.
6. **`RowFilterPolicyDialog`**: resource 3 level (không column), per-item `Input` filterExpr. Vitest.
7. **Nối vào list**: New/Edit/Delete chọn dialog theo tab; `maskingToInput`/`rowfilterToInput` reverse-map; nút **Download Trino config** + `TrinoConfigDialog` (hiển thị + tải 2 file). Mở rộng test list.
8. **CI + harness Java + smoke**: fmt/clippy/test workspace + frontend gate; harness `tests/ranger` (đã có sẵn test `emailMasked`/`rowFilterApplied` — xác nhận policy tạo qua serializer mới vẫn enforce, chạy `mvn -f tests/ranger/pom.xml test`); smoke create/edit masking + rowfilter + tải config.

## Điểm implementer xác nhận tại chỗ
- Helper `req`/`session_for`/`into_json` trong `routes_api.rs` (task 2/3).
- `EgretRangerEnforcementTest` nạp policy kiểu fixture hay từ egret `/service` live (task 8).
- `AccessPolicyInput` có cần `#[derive(Serialize)]` cho test dispatcher không (task 1).

## Known-minor mang từ P2 (không xử lý ở đây)
Serializer lossy với policy Ranger ngoài (đã doc trong `ranger_json.rs`); principals cap gộp 50.
