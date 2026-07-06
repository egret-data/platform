# Kế hoạch triển khai — Data Governance Policies P2

> Bản chuẩn (code đầy đủ từng bước): `docs/en/plans/2026-07-06-data-governance-policies-p2.md`. File này là tổng quan tiếng Việt.

**Mục tiêu:** CRUD đầy đủ cho policy **Access** trên trang Control-Plane Policies — editor lái theo service-def (resource hierarchy + allow/deny items), principal picker từ users/groups của egret, và endpoint create/update/delete serialize form thành Ranger JSON hợp lệ.

**Kiến trúc:** submodule mới `policies/ranger_json.rs` giữ serialize editor-DTO↔Ranger-JSON (unit-test bằng round-trip qua `summarize_policy` của P1). 3 endpoint mutating mới trong `policies/mod.rs` gọi `AuthzStore::{create,update,delete}_policy` (đã có). Endpoint `/control-plane/principals` list users + groups. Editor React là Dialog (theo mẫu add-dialog của `ControlPlaneSecrets`), mở từ **New** và **Edit** mỗi dòng.

**Phụ thuộc P1** (nhánh `feat/data-governance-policies`). Masking + Row-filter + bundle Lớp B để **P3**.

**Quy ước:** DB test tự-skip khi thiếu `DATABASE_URL`; backend `fmt --check` + `clippy --workspace -D warnings`; frontend `lint/typecheck/test/build`; KHÔNG `cargo build -p egret-controlplane` đơn lẻ (dùng `egret-bin`/test profile). Ranger body camelCase (`policyType`/`isEnabled`/`policyItems`/`isExcludes`); DTO egret snake_case.

## Danh sách task (TDD, commit từng task)

1. **`ranger_json.rs`** — `AccessPolicyInput` DTO + `access_to_ranger_json` (allow→`policyItems`, deny→`denyPolicyItems`, `accesses`→`[{type,isAllowed}]`, bỏ level rỗng). Chuyển `policies.rs`→`policies/mod.rs`. Unit test round-trip qua `summarize_policy`.
2. **Endpoint create/update/delete** trong `policies/mod.rs` (POST/PUT/DELETE) gọi serializer + `AuthzStore`. Route test DB-gated (create→list→delete). DELETE handler đặt tên `delete_policy_handler` tránh trùng.
3. **`/control-plane/principals`** + `GroupRepo::list` + thêm `users`/`groups` vào `CpState` (dựng ở bin + test). Trả `{kind,name,label}` (user=email, group=name), lọc theo `q`. Lưu ý principal value phải khớp Trino group-mapping (admin free-type được).
4. **Frontend `api/policies.ts`**: thêm `createPolicy/updatePolicy/deletePolicy/getPolicy/listPrincipals/getServiceDef` + types (`AccessPolicyInput` camelCase).
5. **`PrincipalPicker.tsx`** — multi-select users/groups bằng Popover+Input+list (không có Command component), chip xóa được. Vitest.
6. **`AccessPolicyDialog.tsx`** — form: name, 4 resource inputs (giá trị đơn/`*`), list allow/deny items (checkbox access-type từ service-def + PrincipalPicker). Vitest submit.
7. **Nối editor vào `Policies.tsx`**: nút New + Edit/Delete mỗi dòng (chỉ tab Access), reverse-map `toInput` (đảo của serializer), ConfirmDeleteDialog. Mở rộng test list.
8. **Full CI + smoke**: fmt/clippy/test workspace (DB) + frontend gate; smoke create/edit/delete Access policy qua UI.

## Ngoài P2 (để P3)
Editor Masking + Row-filter; bundle "Download Trino config" (Lớp B) + test enforce harness Java; UI multi-value + isExcludes/isRecursive per level (DTO đã mang sẵn flag).

## Điểm implementer xác nhận tại chỗ
- Tên helper `req`/`session_for`/`into_json` trong `routes_api.rs` (task 2/3).
- `egret-auth` đã có struct `Group` chưa (task 3).
- ESLint có cấm `any` không (task 7).
