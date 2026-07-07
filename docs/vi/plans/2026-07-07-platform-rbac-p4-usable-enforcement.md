# Kế hoạch — Platform RBAC P4 (Enforcement dùng được)

> Bản chuẩn: `docs/en/plans/2026-07-07-platform-rbac-p4-usable-enforcement.md`. File này là tổng quan.

**Mục tiêu:** Vá 3 gap để RBAC chi tiết thực sự dùng được + trung thực. Dựa trên nghiên cứu Superset (lấy ý tưởng "visibility menu = tái dùng chính permission", bỏ mô hình per-object ViewMenu đắt đỏ, không copy silent-delete).

- **A 🔴 Frontend gate theo permission** — non-`platform_admin` mới dùng được console.
- **B 🟠 Reconcile catalog chết** — `database:*`/`workspace:*` (không route enforce) + `database:manage` (seed dùng, catalog không có) → drift.
- **C 🟡 Fix `monitor:kill`** — đang gate `engine:read`.

## Task (subagent-driven)
1. **Backend**: bỏ `database:*`/`workspace:*` khỏi `rbac.rs` (0 call-site); thêm `engine:kill`, đổi `kill_query` sang `engine:kill`; thêm `permissions` vào `MeResponse`; migration `0060` DELETE role_permissions `database:%`/`workspace:%` + sửa `0059` bỏ database:read/workspace:read của data_steward (touch metastore lib.rs); **drift-lint test** (mọi catalog key phải có ≥1 `require_permission` call-site — check drift ngược); startup warn→**panic ở debug**.
2. **Frontend**: `me.permissions`; guard **`RequirePermission(perm)`** (bypass platform_admin); nới outer control-plane gate = platform_admin HOẶC bất kỳ quyền control-plane; wrap mỗi route theo `*:read`; **lọc sidebar** theo `me.permissions` (item có `perm`, ẩn nếu không có; group rỗng thì ẩn heading). Test: role chỉ `policy:read` thấy Policies, không thấy Engines/Users.
3. **Gate + rebuild + smoke** — backend+frontend gate trên egret_test; rebuild serve; smoke: admin thấy đủ; tạo user chỉ role data_steward → login → chỉ thấy section được phép, route cấm hiện Forbidden. 0 console error.

## Ghi chú
Bỏ database:*/workspace:* là cố ý (dead = drift); workspace-scope epic sau sẽ thêm lại KÈM enforce (+ `scope_id` per-object). KHÔNG thêm loại perm `menu_access` — visibility tái dùng `*:read`.
