# AX-3 — Effective-Access Viewer — Thiết kế

**Trạng thái:** Đã duyệt
**Ngày:** 2026-07-12
**Sáng kiến:** Access-Control UX uplift (increment 2/5, đẩy lên từ #3) —
`docs/en/plans/2026-07-12-access-control-ux-uplift-roadmap.md`

> Bản tiếng Việt là tham chiếu; bản chuẩn là
> `docs/en/specs/2026-07-12-ax3-effective-access-design.md`.

## Mục tiêu

Biến console Access-Control từ "danh sách" thành **công cụ trả lời**: chọn 1 user/group
→ thấy ngay **họ làm được gì, ở đâu, vì sao** — roles (direct hay qua group), effective
permissions (kèm role/parent mỗi quyền đến từ đâu), membership từng workspace, object
được share, và deny tường minh. Hiện admin phải tự ghép roles+groups+hierarchy+object
grant+deny để trả lời "vì sao Alice thấy cái này?"; tính năng này tính + giải thích sẵn.

## Vì sao chọn hình dạng này

Cedar `authorize` là API *per-decision* (allow/deny cho 1 action/resource) → không liệt
kê được "tất cả". Nên effective access **tính trực tiếp từ dữ liệu** (`access_grants` +
`roles` + `role_permissions` + `user_groups` + object tables) — deterministic, enumerable,
kèm provenance. Đây là mở rộng đúng `session.rs::permissions()` (recursive hierarchy
closure) + theo dõi *nguồn* mỗi grant. **Access simulator** ("can X do A on R" dùng Cedar
`Decision.reasons`) cố ý **ngoài phạm vi**.

## Thiết kế

### 1. Backend — `AccessResolver` (trong `egret-auth`)

Resolver read-only: input principal `(type ∈ {user,group}, id)` → `EffectiveAccess`:
- **principal**: type, id, name, `is_platform_admin`.
- **platform_roles**: `[{ name, source }]` — source `"direct"` | `"group:{G}"`.
- **permissions**: `[{ key, group, via: [{ role, inherited_from }] }]` — perm hiệu lực +
  **vì sao** (role nào; `inherited_from` = tên parent nếu kế thừa, else null).
- **workspaces**: `[{ workspace{slug,name}, role, source }]`.
- **objects**: `[{ kind, id, name, workspace_slug, role, effect: allow|deny, source }]`.
- **denies**: các grant `effect='deny'` tách riêng cho nổi bật.

Quy tắc:
- **User**: grant của chính user **+** grant của mọi group trong `user_groups`; mỗi thứ
  gắn `source` = direct / `group:{G}`.
- **Group principal**: egret không nest group (chỉ user→group) → chỉ grant trực tiếp của
  group, không có lớp "qua group".
- **platform_roles**: `access_grants` scope=platform (+ groups) join `roles`.
- **permissions (phần mang provenance)**: mỗi role được grant → đi **chuỗi parent**
  (hierarchy closure như `session.rs`) gom `role_permissions`; mỗi perm ghi mọi path
  `{ role, inherited_from }`. Nhóm theo catalog group để hiển thị.
- **workspaces**: `access_grants` scope=workspace (+ groups) → role + source; resolve
  scope_id → `{id,slug,name}`.
- **objects**: `access_grants` scope=object (+ groups) → object_type(kind), scope_id→name
  + workspace (dùng map kind→table trong `egret-bi/src/shares.rs`), role, effect.
- **denies**: grant `effect='deny'` gom vào list riêng (deny-wins vốn là hành vi PDP; đây
  chỉ phơi ra).
- **platform_admin**: nếu giữ `platform_admin` platform-scope → `is_platform_admin=true`;
  UI hiện "Superuser — bypass mọi kiểm tra" thay vì liệt kê perm (giống role editor).

### 2. Backend — endpoint
`GET /api/control-plane/access/effective?principal_type={user|group}&principal_id={id}`
→ `200 EffectiveAccess`.
- **Gating**: `role:read` (perm đang gate trang Roles/Permissions; platform_admin bypass).
  Endpoint lộ toàn bộ access graph 1 người → không rộng hơn quyền đọc-graph của console.
- `principal_id` nhận UUID hoặc email/group-name (resolve server-side như AX-1). Không
  tìm thấy → 404.
- Nối vào access router control-plane; lộ qua orval client.

### 3. Frontend — trang "Access explorer"
- **Nav** mới "Access explorer" dưới Access control (sau Permissions), gate `role:read`.
- **Route** `/control-plane/access/effective`, đọc `?user={id}`/`?group={id}` preselect
  (deep-link, share URL được).
- **Principal picker** đầu trang (tái dùng `listPrincipals`); đổi → refetch.
- **Sections** (chỉ render khi có data): (1) header + badge Superuser nếu có; (2) Platform
  roles chip + source; (3) **Effective permissions** nhóm theo catalog, mỗi perm ghi "via
  {role}" + "inherited from {parent}", dùng `FilterableTable` (AX-1) cho search/sort; (4)
  Workspaces role+source; (5) Shared objects; (6) **Denies/Blocked** highlight `--destructive`.
- **Deep-link entry**: hàng Users/Groups thêm action **"View access"** →
  `/control-plane/access/effective?user={id}`.
- Read-only. Loading/empty/error/forbidden xử lý như trang access khác.

### 4. Data flow
Picker (hoặc URL param) chọn principal → 1 call `GET …/effective` → render sections. Không ghi.

## Kiểm thử

**Backend (`AccessResolver`, PG thật)**
- User có role direct + role qua group → cả hai xuất hiện đúng `source`; perm gồm cả hai.
- **Hierarchy provenance**: role có parent cấp perm P → P hiện `inherited_from=<parent>`;
  perm direct → `inherited_from=null`.
- **Workspaces**: ws_editor A (direct) + ws_viewer B (qua group) → cả hai đúng source.
- **Objects + deny**: object grant hiện ở `objects`; grant `effect='deny'` hiện ở `denies`.
- **Group principal**: resolve grant của group; không lớp "qua group".
- **platform_admin**: `is_platform_admin=true`, đi path superuser.
- **Resolve principal_id**: email/group-name resolve; principal lạ → 404.

**Frontend (Vitest)**
- Render các section từ `EffectiveAccess` mock, gồm provenance ("inherited from …") + deny highlight.
- `?user={id}` preselect (deep-link).
- Đổi principal → refetch.
- Badge superuser cho platform-admin.

**Security/UX review** trước merge: endpoint gate `role:read`, không lộ quá authz graph mà
role-reader vốn thấy; không data-plane/Ranger; resolve principal-id server-side.

## Ngoài phạm vi
- **Access simulator** ("can X do A on R" → Cedar reasons) — follow-up mạnh, để sau.
- **Data-plane / Ranger** (query data thật) — hệ governance khác.
- **Group nesting** — egret không có.
- Sửa từ viewer — read-only; sửa ở trang Users/Roles/Members.

## Ghi chú
- Không migration — thuần read/resolver + endpoint + trang mới.
- `AccessResolver` là core tái dùng: unit gọn trong `egret-auth`, return `EffectiveAccess`
  rõ ràng, test độc lập khỏi HTTP.
- Tái dùng: `FilterableTable` (AX-1), picker `listPrincipals`, map kind→table trong
  `egret-bi/src/shares.rs`, logic hierarchy closure trong `session.rs::permissions()`.
