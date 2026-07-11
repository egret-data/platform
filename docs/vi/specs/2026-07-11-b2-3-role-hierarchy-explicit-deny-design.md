# B2-3 — Role hierarchy + explicit deny (deny-wins)

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-11-b2-3-role-hierarchy-explicit-deny-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-11 · **Phiên bản:** 1.0
>
> **Tiền nhiệm:** B2-1a (object-scope backend), B2-1b (sharing UI + data path), B2-2 (notebooks + dataset preview).
>
> Increment B2 thứ tư. Thêm 2 khả năng authz độc lập nhưng chung một seam (Cedar template generation): **role hierarchy** (role kế thừa perm của role cha) + **explicit deny** (grant `effect=deny` override allow — carve-out kiểu Ranger).

---

## 1. Mục tiêu & scope

Hai khả năng, làm chung:
1. **Role hierarchy** — role kế thừa perm của một role khác qua một `parent_role_id` duy nhất, **cùng scope**. `ws_viewer ← ws_editor ← ws_admin` thành chuỗi khai báo thay vì liệt kê tay. DRY khi soạn role + role tùy biến ghép được.
2. **Explicit deny (deny-wins)** — một dòng `access_grants` mang `effect=deny`; deny luôn thắng allow. Đây là carve-out mà cascade allow-only hiện tại không biểu đạt được: share cả folder Viewer rồi **chặn** 1 dashboard nhạy cảm cho 1 user.

Hai cái vào cùng 1 increment. Độc lập ở data-model + UI nhưng hội tụ ở `CedarPdp::load_inner` (sinh template) → làm chung để không đụng seam đó 2 lần.

**Trong scope:** `roles.parent_role_id` + enforce same-scope/acyclic + mở rộng transitive-closure khi sinh template; `access_grants.effect` + 1 `forbid` template mỗi role + link grant theo effect; admission object-guest biết deny; option "Blocked" ở share-modal; editor role-inheritance ở RBAC console.

**Ngoài scope:** chiều deny ngược (**deny rộng + allow hẹp / deny-exception** — Cedar `forbid` luôn thắng `permit`, re-allow trong vùng deny không biểu đạt được nếu không có forbid có điều kiện; hoãn, có thể không cần); multi-parent DAG (chỉ single-parent); cross-scope inherit; data-model flip (B2-flip); per-user data-plane delegation (B3); audit_log (B4).

**Quyết định chốt khi brainstorm:** làm cả hai; deny **deny-wins only**; hierarchy **single-parent, same-scope**; deny ở share-modal là một option **"Blocked (No access)"** dạng deny grant key theo role **`ws_editor`** (Editor ⊇ Viewer qua hierarchy → deny read+manage = block toàn bộ).

## 2. Role hierarchy

### 2.1 Data model
`ALTER TABLE roles ADD COLUMN parent_role_id uuid REFERENCES roles(id) ON DELETE SET NULL;` (migration mới; repo đã qua 0070 — dùng số kế tiếp). `ON DELETE SET NULL` để xóa role cha thì con về không-parent, không cascade delete.

**Same-scope + acyclic** không ép được bằng `CHECK` (không tham chiếu dòng khác). `RoleRepo` (sole writer của `roles`/`role_permissions`, đã xác nhận ở B1b-0) enforce cả hai khi set-parent:
- reject nếu `parent.scope != role.scope` (→ lỗi validation, HTTP 400);
- reject nếu đi ngược chain từ `parent_role_id` quay về `role.id` (cycle) (→ 400).

Thêm trigger defense-in-depth `roles_parent_same_scope_and_acyclic` (BEFORE INSERT/UPDATE) kiểm lại scope + đi chain, raise exception nếu vi phạm — nên raw SQL cũng không phá được graph.

### 2.2 Mở rộng transitive-closure (seam)
Hiện `load_inner` dựng body template từ `role_permissions` của chính role. Mới: perm **hiệu lực** của role = union theo chain parent — `own ∪ parent's effective ∪ …`. Tính trong `load_inner`:
1. Load tất cả roles (`id, name, scope, parent_role_id`) + tất cả `role_permissions` bằng 2 query bulk (tránh N+1 khi tính closure).
2. Mỗi role đi ngược `parent_role_id` (chain acyclic theo §2.1), union tập perm. Cap số hop phòng vệ (vd 32) + log nếu vượt (không bao giờ xảy ra nếu acyclic — belt-and-suspenders chống graph hỏng).
3. Emit template `permit` với action-list **hiệu lực**.

Hạ nguồn không đổi: `link_grants`, `build_entities`, forbid admin-graph, body bypass `platform_admin`. Hierarchy vô hình với request path — chỉ làm body template dày thêm lúc build.

**Hot-reload:** đổi parent là đổi role. `RoleRepo` phải phát `RolesChanged` cả khi set-parent (đã phát ở create/delete/set_permissions) → PDP rebuild + tính lại closure.

### 2.3 API + UI
- `RoleRepo` create/update nhận `parent_role_id` optional; role DTO expose `parent_role_id` + `inherited_permissions` (effective trừ own) để hiển thị.
- RBAC-console role editor: picker **"Inherits from"** liệt kê role cùng scope, loại self + descendants (client lọc theo danh sách roles để không mời cycle; backend re-validate). Editor perm hiển thị own (sửa được) + inherited (read-only, badge role nguồn).

## 3. Explicit deny (deny-wins)

### 3.1 Data model
`ALTER TABLE access_grants ADD COLUMN effect text NOT NULL DEFAULT 'allow' CHECK (effect IN ('allow','deny'));` + đưa effect vào UNIQUE: drop `UNIQUE (principal_type, principal_id, role_id, scope_type, scope_id)` cũ và thêm lại với `effect` — allow & deny cùng (principal,role,scope) coexist (deny thắng; coexist vô hại + upsert per-effect). Default `allow` → mọi dòng cũ + backfill/sync trigger B1a vẫn sinh allow → **parity không đổi**.

### 3.2 Grant loading
`Grant` thêm `effect: String`; `grants_for` thêm `ag.effect` vào SELECT. Không lọc — trả cả allow lẫn deny; effect quyết định link template nào.

### 3.3 Cedar emission (seam phần 2)
`load_inner` emit **2 template mỗi role**:
- `role:{id}` — `permit(...)` với action-list hiệu lực (như cũ + closure §2.2).
- `role-deny:{id}` — `forbid(...)` **cùng** action-list hiệu lực + **cùng** operator scope (`in` cho ws/object role, `==` cho platform role). Template deny của `platform_admin` mirror body bypass thành deny-all (`forbid(principal in ?principal, action, resource in ?resource)`).

`link_grants` chọn template theo effect: `allow → role:{id}`, `deny → role-deny:{id}`, cùng slot `?principal`/`?resource`. Cedar `forbid` override mọi `permit` → deny grant thắng allow **trong phạm vi action + scope-cascade nó phủ**. Deny cascade cùng chiều allow: deny folder (`forbid resource in folder:{id}`) chặn mọi item; deny item chặn đúng item.

`is_platform_admin` computation không đổi — đếm **allow** grant platform-scope trên `platform_admin`. (Deny trên role platform_admin là edge-case hiếm; Cedar deny-all `forbid` vẫn chặn request thực nên authz đúng dù cờ ADMIN_GRAPH exemption bỏ qua deny. Ghi chú, không xử lý riêng.)

### 3.4 Share API + object-guest admission
- `grant_put` (`egret-bi/src/shares.rs`) nhận `effect` (default `allow`). **"Blocked"** ở modal gửi `effect=deny` + `role=ws_editor` (Editor, action hiệu lực sau §2.2 = read+manage). Tạo deny vẫn cần `require_object(manage_action(kind), obj)` — cùng gate allow.
- **`has_object_grant_in_workspace` phải lọc `effect='allow'`.** Nó admit non-member có object grant; một deny grant đơn độc KHÔNG được admit ai (admit chỉ để deny là vô nghĩa, tệ hơn là biến dòng deny thành side-channel admission). Thêm `AND ag.effect = 'allow'` vào mọi nhánh query đó.
- `grant_list` trả cả deny (manager thấy block), gắn `effect`.

### 3.5 UI (share modal)
Dropdown role thêm option thứ 3 **"Blocked (No access)"** cạnh Viewer/Editor. Chọn → PUT `{role: "ws_editor", effect: "deny"}`. Principal bị block render badge "No access" (không phải nhãn role). Inherited deny (deny trên folder chứa) hiện ở section inherited read-only như inherited allow, để manager hiểu vì sao bị block. Remove (`DELETE`) hoạt động y hệt cho deny grant.

## 4. Bảo mật

- **Deny-wins fail-safe**: thêm deny chỉ bớt quyền, không cấp gì. Deny sai → over-block (an toàn), không bao giờ over-permit.
- **Admission loại deny** (§3.4): admission chỉ join `effect='allow'` → deny không nới rộng tập admit `WsContext`.
- **Cấm cross-scope inherit** (§2.1): ws role không kế thừa platform role → chống ws role hút perm platform qua link parent.
- **Chống cycle** (§2.1): chain acyclic bằng app + trigger; closure walk còn cap hop → graph hỏng suy biến thành tính có bound, không loop vô hạn/DoS.
- **Deny cần manage** (§3.4): chỉ ai đã manage được object (role, object-Editor share, hoặc admin) mới cắm được deny — Viewer không block người khác.
- **Admin-graph forbid không ảnh hưởng**: template deny per-role trực giao với `forbid` ADMIN_GRAPH tĩnh; cả hai đều `forbid`, đều chỉ deny — không tương tác escalate.
- **Giữ parity**: `effect` default `allow`; matrix regression B1a (allow-only) phải pass y nguyên.

## 5. Testing

- **Role hierarchy:** `ws_editor` kế thừa `ws_viewer` cho union (request action read của ws_viewer allow qua grant ws_editor); set-parent tạo cycle bị reject (400); parent khác scope reject (400); đổi parent hot-reload PDP (test app-wiring chứng minh closure đổi không cần restart); closure walk kết thúc trên chain sâu (test-forced).
- **Explicit deny:** allow folder + deny 1 item trong đó → item denied, sibling vẫn allow (carve-out deny-wins); deny folder → mọi item denied; non-member chỉ có deny grant KHÔNG được admit (regression §3.4); Viewer không tạo được deny (403 PUT); Editor/manager tạo được; `grant_list` hiện deny.
- **PDP parity:** matrix regression B1a (allow-only) pass y nguyên; matrix object-scope mới phủ tổ hợp allow+deny.
- **Frontend:** role editor render picker inherits-from (loại self/descendants) + hiện inherited perm read-only; share modal có Blocked + PUT deny grant; principal bị block hiện badge No-access; inherited deny read-only. Vitest.
- **Full gate:** BE `cargo test --workspace`, FE `lint/typecheck/test/build`, openapi + client current, adversarial security review.

## 6. Câu hỏi mở (cho plan)

- **Chi phí closure:** 2 query bulk + closure in-memory ở `load_inner` (rebuild mỗi `RolesChanged`). Roles ít (đơn/thấp-hai-chữ-số) nên không đáng kể — nhưng xác nhận plan dùng 2 query bulk (không N+1 per-role) để tránh 1 query/role mỗi reload.
- **Deny template cho platform role:** platform (non-admin) role dùng `==`; deny trên platform role không cascade vào workspace (mirror anti-leak allow). Xác nhận đúng ý (đúng — platform deny nên exact-match như platform allow).
- **Chọn role "Blocked":** key theo `ws_editor` để deny read+manage. Nếu tương lai một object kind có action ngoài closure ws_editor, block sẽ sót. Chấp nhận giờ (object action là read/manage mỗi kind); xem lại nếu kind thêm tầng action thứ 3.
- **Số migration:** repo đã qua 0070; plan chọn số kế tiếp (0071/0072) + touch `egret-metastore/src/lib.rs` để force re-embed (gotcha sqlx staleness đã biết).

---

*Design v1.0 (2026-07-11). Thêm role hierarchy (single-parent, same-scope, mở rộng transitive-closure lúc build template) + explicit deny (deny-wins, `forbid` template mỗi role, admission biết deny), cả hai hội tụ ở `CedarPdp::load_inner`. Deny-exception (chiều ngược) + multi-parent DAG cố ý ngoài scope.*
