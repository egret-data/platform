# AX-4 — Surface the Hidden Model — Thiết kế

**Trạng thái:** Đã duyệt
**Ngày:** 2026-07-13
**Sáng kiến:** Access-Control UX uplift (increment 4/5) —
`docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`

> Bản tiếng Việt là tham chiếu; bản chuẩn là
> `docs/en/specs/2026-07-13-ax4-surface-model-design.md`.

## Mục tiêu

Cho phần Roles lộ đúng chiều sâu mà model authz đã có: **role hierarchy** (role nào kế
thừa role nào), và role editor chỉ hiện permission mà role thực sự giữ được. Hiện bảng
Roles giấu quan hệ parent/inherits (tính năng B2-3) trong 1 dialog Edit, và permission
matrix hiện **toàn bộ** catalog platform+workspace cho mọi role → sửa role **workspace**
vẫn liệt kê group platform-only (Engines/Warehouses/Storage/Secrets) không bao giờ giữ
được — gây rối (đã phát hiện lúc review AX-1). Thêm scope clarifiers để platform vs
workspace hết mơ hồ.

**FE-only.** Data đã sẵn: `AccessRole` có `parentRoleId` + `scope`, `PermissionDto` có
`scope` (`"platform"|"workspace"`). Không đổi backend.

## Thiết kế

### 1. Bảng Roles — lộ hierarchy (2 chế độ)

Trang Roles dùng `FilterableTable` (AX-2). Thêm cả cột lẫn tree (theo duyệt):

- **List mode (mặc định):** thêm cột **"Inherits from"**. Resolve `parentRoleId` → tên
  role cha qua `Map<id, name>` từ danh sách roles đã load; render dạng chip, hoặc `—` nếu
  không có cha. Cột có `value` (tên cha) → search/sort được như cột khác. Mọi thứ khác
  (summary strip, search, facet Scope, dialogs, actions, built-in badge, assignee
  drill-in) giữ nguyên.
- **Tree view (toggle):** nút "Tree view" ở header trang. Khi bật, body bảng thay bằng
  **cây thụt lề**: roles nhóm theo scope (mục Platform + mục Workspace, vì parent phải
  cùng scope con), mỗi root role liệt kê với con nested, thụt 1 tầng/độ sâu. Search +
  facet Scope là công cụ list-mode → **ẩn** khi tree bật, kèm note nhỏ ("Tree view hiện
  toàn cây — chuyển list để search/filter."). Mỗi node hiện tên role + built-in badge;
  click node mở đúng dialog Edit (admin) như hàng list → không mất action nào. Tree thuần
  là view cấu trúc của mọi role.

Dựng thụt lề từ chuỗi parent (root = role không có `parentRoleId` trong scope; đệ quy con).
Chặn cycle (DB đã cấm) bằng depth cap.

### 2. Role editor — scope-filter matrix + polish

Trong `EditRoleDialog` của `RolesPage.tsx`:

- **Scope-filter (bản fix):** chỉ render group/permission có `scope === role.scope`. Role
  workspace chỉ hiện group workspace (Charts/Dashboards/Datasets/Notebooks/SQL Lab/
  Workspaces…); group platform-only ẩn hẳn. `platform_admin` giữ xử lý superuser (matrix
  disabled + note). Dùng field `scope` đã có trên mỗi catalog permission.
- **Search trong dialog:** ô `Input` đầu matrix lọc permission theo `key`/`description`
  (không phân biệt hoa thường); group không match bị ẩn. Độc lập với trạng thái checked.
- **Select-all-per-group:** mỗi group header có checkbox (shadcn `Checkbox`, hoặc tri-state
  `indeterminate` khi lọc dở) bật/tắt toàn bộ perm **không-inherited** của group. Perm
  inherited vẫn disabled, không bị ảnh hưởng.
- **Checkbox affordance:** đảm bảo checkbox permission hiện rõ checked vs unchecked ở cả 2
  theme (review AX-1 chê tương phản thấp). Dùng shadcn `Checkbox` chuẩn (vuông, tô khi
  check); chỉ chỉnh nếu thực sự tương phản yếu. Không tự chế control.

### 3. Scope clarifiers (in-product)

- **Trang Roles:** 1 dòng clarifier dưới header (tái dùng style Members AX-1): "Platform
  roles áp dụng toàn egret; workspace roles áp dụng trong 1 workspace và gán per-workspace
  ở trang Members của workspace đó."
- **Role editor:** 1 dòng gần matrix: "This is a **{scope}** role — chỉ giữ được permission
  scope {scope}." (giải thích vì sao matrix bị lọc).

## Kiểm thử (Vitest)

- **Bảng Roles:**
  - Cột "Inherits from" hiện **tên** role cha (không phải id) cho role có cha, và `—` cho
    root.
  - Bật "Tree view" thay bảng bằng cây thụt lề nest con dưới cha + nhóm theo scope; search
    + facet Scope ẩn ở tree-mode.
  - List mode vẫn sort + filter (regression check AX-2).
- **Role editor:**
  - Mở editor cho role **workspace** chỉ hiện group workspace, KHÔNG hiện group platform
    (vd không có "Engines"); role **platform** hiện group platform.
  - Search trong dialog lọc matrix; group không match biến mất.
  - Checkbox select-all group bật/tắt toàn bộ perm không-inherited; perm inherited vẫn
    checked+disabled.
- **Clarifiers:** clarifier trang Roles + dòng scope của editor render.

## Ngoài phạm vi

- Hiện **số** inherited ("own +N inherited") trên cột Permissions của list — list DTO chỉ
  có `permissionCount` (own); cần thêm 1 field DTO backend nhỏ. Hoãn (follow-up tùy chọn;
  cột "Inherits from" + tree đã diễn đạt kế thừa).
- **AX-5** — bulk ops + undo.
- Đổi backend (increment này thuần trình bày; data đã trên DTO).
- Sửa hierarchy từ tree (đổi parent vẫn qua select "Inherits from" trong dialog Edit).

## Ghi chú

- Tái dùng: `FilterableTable` (AX-2, list mode), style clarifier Members (AX-1), cấu trúc
  `EditRoleDialog` hiện có (groups + checkbox + inherited badge + select "Inherits from"),
  shadcn `Checkbox`/`Input`/`Button`.
- Giữ `RolesPage.tsx` không phình: nếu tree render / editor đổi làm file vượt xa kích thước
  hiện tại, tách body tree thành component in-file (hoặc file `RolesTree.tsx`) — tách theo
  trách nhiệm, không tái cấu trúc dialog.
