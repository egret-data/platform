# AX-1 — Trang Workspace Members — Thiết kế

**Trạng thái:** Đã duyệt (design qua review trực tiếp + mockup đã publish)
**Ngày:** 2026-07-12
**Sáng kiến:** Access-Control UX uplift (increment 1/5) —
`docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`
**Mockup:** `scratchpad/workspace-members-mockup.html`

> Bản tiếng Việt là tham chiếu; bản chuẩn là
> `docs/en/specs/2026-07-12-ax1-workspace-members-design.md`.

## Mục tiêu

Mỗi workspace có **trang Members riêng** để ws_admin tự quản ai thuộc workspace và ở
role nào — **không cần** platform admin. Đóng gap cấu trúc trong mô hình authz 2-plane:
plane platform (Control Plane → Access control) có UI đầy đủ, còn plane workspace thì
không → hiện mọi việc gán role workspace bị ép qua control-plane (platform-admin-only).

## Hiện trạng

- Backend **đã có** member API — `GET/POST/DELETE /api/ws/{ws}/members`, đều gate
  `workspace:manage_members` (ws_admin + platform_admin dùng được). **Chưa có FE.**
- `MemberRepo::add` là **DELETE-rồi-INSERT trong 1 tx** ("một workspace role mỗi
  member: xoá grant cũ rồi chèn role mới"). ⇒ **đổi role = POST lại member với role
  mới**, không cần endpoint mới.
- `MemberRepo::list` chỉ trả `{principal_type, principal_id, role}` — **không tên,
  không provenance** → UI hiện toàn UUID.
- `add` validate role theo const `WORKSPACE_ROLES = [ws_admin, ws_editor, ws_viewer]`
  → **chặn custom ws-role** (vd "Report Builder") dù INSERT đã scope `r.scope =
  'workspace'`.
- **Chưa có last-admin guard**: remove/demote có thể lấy đi ws_admin cuối cùng.
- FE **chưa có shell settings workspace** — chỉ có `notebook-settings` lẻ.

## Thiết kế

### 1. Backend — `crates/egret-workspace`

**1a. Enrich member DTO** (`routes.rs` + `members.rs`): thêm vào `MemberRow` /
`WorkspaceMember` / `MemberDto` và câu `list`:
- `principal_name` — email user / tên group qua `LEFT JOIN` + `COALESCE(u.email,
  g.name)` (đúng pattern `shares.rs::grant_list`).
- `granted_by_name` — ai đã thêm (`access_grants.granted_by` → email).
- `added_at` — `access_grants.created_at`.
- `is_platform_admin: bool` — principal có giữ `platform_admin` (platform scope)
  không (EXISTS sub-select) → cho badge "superuser bất kể membership".

**1b. Cho phép custom ws-role** (`members.rs::add`): thay guard
`WORKSPACE_ROLES.contains` bằng kiểm role tồn tại ở `scope='workspace'` (query bảng
`roles`). INSERT đã scope sẵn; chỉ cần nhận mọi role workspace (seeded **hoặc**
custom), từ chối tên lạ / platform-scope.

**1c. Last-admin guard** (`routes.rs` + `members.rs`): thêm
`count_ws_admins(ws)`. Trước **remove** hoặc **đổi role rời khỏi ws_admin**, nếu đây
là ws_admin cuối cùng → `Conflict("workspace phải giữ tối thiểu 1 admin")`. Gắn vào cả
`remove_member` và `add_member` (khi target đang là ws_admin và role mới ≠ ws_admin).
Mirror guard "last platform admin" trong `egret-auth/src/roles.rs`.

**1d. Không endpoint mới.** Đổi role dùng lại `POST /members`. `list_members` giữ
gate `workspace:manage_members` (chỉ admin xem) — nới cho mọi member xem = ngoài phạm vi.

### 2. Frontend

**2a. Shell settings workspace (mới):** khu `/explore/:workspace/settings` với sub-nav
trái, tab **Members** đầu tiên (làm bây giờ), chừa chỗ cho Connections/Catalog/Storage
sau. Route + shell gate `workspace:manage_members`. Entry: nút ⚙ "Workspace settings"
trong nav workspace, chỉ hiện cho người có `workspace:manage_members`.

**2b. Trang Members** (`frontend/src/routes/explore/settings/Members.tsx`) — bám mockup:
- Header: "Members" + subtitle tên workspace + nút **Add member**.
- **Scope clarifier**: "Workspace roles chỉ áp dụng trong {workspace}. Quyền toàn nền
  tảng cấp riêng ở Control Plane → Access control."
- **Add picker**: tái dùng `AddGrantRow` của `ShareDialog` (toggle User/Group + search
  `listPrincipals` + role select + Add). Role = `GET …/roles` lọc `scope='workspace'`.
- **Toolbar**: search + filter Role + filter Type.
- **Summary strip** 1 dòng ("N members · N admins · N people · N groups"), KHÔNG KPI to.
- **Table**: Member (tên + badge `platform admin` khi `is_platform_admin`), Type,
  **Role (Select inline)**, Added by, Added, menu `⋯` (Remove). Group row expand "N
  people via this group" (best-effort; nếu chưa có data thì hiện tên+type, hoãn expand).
- **Đổi role inline**: Select gọi `POST /members` role mới (replace-then-insert),
  optimistic + toast, lỗi thì revert.
- **Guardrails**: ws_admin duy nhất → Select khoá ("Last admin — role locked"); Remove
  disable cho admin cuối; remove có confirm; tự-đổi/tự-remove có cảnh báo.

**2c. `FilterableTable` v1 (gọn):** primitive filterable-table (search + sortable,
dùng lại `SortableHeader`) cho bảng Members. Tối thiểu; AX-2 tổng quát hoá sau.

**2d. API client:** `useWorkspaceMembers` / `useAddMember` (POST — cả add lẫn đổi role)
/ `useRemoveMember`. Type khớp `MemberDto` enriched.

### 3. Màu semantic
Thêm token `--success` (green tách khỏi teal brand) cho swatch role "admin" / state →
màu state tách khỏi màu CTA. (Token nhỏ P3 mà AX-5 cũng cần; thêm ở đây ok.)

## Kiểm thử

**Backend**
- `list` trả rows enriched: user→email, group→tên; `granted_by_name` + `added_at` có;
  `is_platform_admin` đúng.
- `add` nhận **custom** ws-role (tạo "Report Builder", gán → OK); từ chối tên lạ /
  platform-scope.
- Đổi role: add lại member với role mới → **replace** (vẫn 1 grant, role mới), không
  nhân đôi.
- **Last-admin guard**: remove ws_admin cuối → `Conflict`; demote ws_admin cuối →
  `Conflict`; có 2 admin, remove 1 → OK.
- Cô lập cross-workspace: list/add/remove chỉ chạm grant của workspace trên path.

**Frontend (Vitest)**
- Bảng render tên enriched (không UUID) + badge `platform admin`.
- Select đổi role gọi mutation add với role mới.
- Add picker mở + submit.
- Hàng admin-duy-nhất: Select disable + hint last-admin.
- Callout scope-clarifier render.

**Security/UX review** trước merge: gate `manage_members` mọi path; last-admin guard
không bypass được qua đổi role; validate custom-role phía server (không tin role string
client); cô lập tenant; shell/route gate quyền.

## Ngoài phạm vi
- Các tab settings khác (Connections/Catalog/Storage) — chỉ shell + Members.
- Effective-access viewer (**AX-3**).
- Tổng quát `FilterableTable` ra Users/Roles/Permissions (**AX-2**).
- Bulk member ops + undo (**AX-5**).
- Cho non-admin xem roster (nới `list_members` sau).
- Expand group-membership nếu data chưa sẵn (hiện tên/type, hoãn drill-in).

## Ghi chú migration / rollout
- Không migration — chỉ query/handler/DTO + frontend (`--success` là token CSS).
- Additive: trang mới + read enriched; thay đổi hành vi duy nhất = last-admin guard
  (từ chối an toàn mới) + nhận custom ws-role (nới, không phá).
