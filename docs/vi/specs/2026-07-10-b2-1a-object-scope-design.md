# B2-1a — Authorization mức object (backend) cho BI content

> **Bản tham chiếu tiếng Việt.** Bản chuẩn là [tiếng Anh](../../en/specs/2026-07-10-b2-1a-object-scope-design.md) — khác biệt lấy bản EN làm gốc.
>
> **Trạng thái:** Design (đã duyệt để lên plan) · **Ngày:** 2026-07-10 · **Phiên bản:** 1.0
>
> **Doc cha:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) §2 (B2). Increment ĐẦU của **B2 (object-level scoping)**.
>
> **Tiền đề:** B1b xong — mọi enforcement qua Cedar PDP; `access_grants` có sẵn cột reserve `scope_type='object'` / `object_type` / `scope_id`.

---

## 1. Mục tiêu & scope

Giao **authorization mức object** cho BI content — collection sharing kiểu Metabase: một user/group được grant role trên **folder** (collection) hoặc **item** riêng (dashboard/chart/dataset), quyền resolve dọc hierarchy **Item → Folder → Workspace → Platform**. Increment này là **backend**: mô hình object-scope trong PDP, grant store + share API, migrate BI per-object handler. **UI** share là B2-1b.

**Trong scope:** `Resource::Object`; Cedar entity hierarchy + fix template operator theo role-scope; `AccessGrantStore` đọc object-grant; API grant/revoke/list object-grant; migrate BI per-object handler (dashboard/chart/dataset) từ workspace-scope sang object-scope; test.
**Ngoài scope:** UI share (B2-1b); object-scope cho type khác — notebook, saved-query, warehouse (B2-2); role hierarchy + explicit deny / allow-exception (B2-3); data-model flip đã defer. Folder hiện **phẳng** (không nest) — nesting ngoài scope.

**Role share:** tái dùng workspace role — `ws_viewer` (→ `*:read`) và `ws_editor` (→ `*:manage`). Share API hiển thị "Viewer"/"Editor". Không role type mới.

## 2. Fix mô hình lõi: operator template theo role-scope

B1b-2 đổi mọi non-admin template thành `resource == ?resource` để chống platform-grant cascade vào workspace. Nhưng inheritance object (grant **folder** phải phủ **item trong folder**) cần `resource in ?resource`. Giải: cho operator phụ thuộc **scope của ROLE** (`template_for_role(role_name, scope, perms)` đã nhận `scope`):

| Role scope | Thân template | Hiệu ứng |
|---|---|---|
| `platform` (non-admin) | `permit(principal in ?principal, action in [perms], resource == ?resource)` | **không cascade** — platform grant chỉ khớp `Platform` (như B1b-2; anti-leak giữ) |
| `workspace` (non-admin) | `permit(principal in ?principal, action in [perms], resource in ?resource)` | **cascade xuống** — ws grant phủ object trong ws; folder grant phủ item trong folder; object grant phủ object đó (reflexive `in`) |
| `platform_admin` | `permit(principal in ?principal, action, resource in ?resource)` | unrestricted (không đổi) |

Vì sao an toàn:
- **Không leak cross-workspace:** ws grant link `?resource = Workspace::"W1"`; `resource in Workspace::"W1"` chỉ đúng cho resource có ancestor chain chứa W1. Resource ở W2 không `in` W1. ✓
- **Không leak platform→object:** platform role giữ `==`, nên platform grant không bao giờ khớp resource workspace/object. ✓ (Độc lập: namespace permission platform và workspace rời nhau — review B1b-2.)
- **Object grant dùng workspace role** (`ws_viewer`/`ws_editor`) → được `in` → cascade đúng.
- **Regression matrix 60-cell không đổi:** nó chỉ phát request `Platform` và `Workspace(id)`; với request `Workspace(W1)` trực tiếp, `resource in Workspace::"W1"` (reflexive) = `resource == Workspace::"W1"`. Khác biệt `in`/`==` chỉ hiện ở object request (matrix không có). Re-run — phải giữ 60/60.

## 3. Cedar entity model

Thêm một entity type generic **`Object`** (tránh khai N Cedar type theo kind). Id namespace theo kind: `Egret::Object::"dashboard:<uuid>"`, `Egret::Object::"folder:<uuid>"`… Chuỗi ancestor của request resource dựng trong `build_entities`:

```
Egret::Object::"dashboard:<id>"   parent → Egret::Object::"folder:<fid>"   (nếu có folder_id)
                                  parent → Egret::Workspace::"<wsid>"      (nếu folder_id NULL)
Egret::Object::"folder:<fid>"     parent → Egret::Workspace::"<wsid>"
Egret::Workspace::"<wsid>"        parent → Egret::Platform::"root"
```
Nên `resource in Egret::Object::"folder:<fid>"` đúng cho folder + item của nó; `resource in Egret::Workspace::"<wsid>"` đúng cho mọi thứ trong ws; `resource in Egret::Platform::"root"` khớp tất (đường admin). Grant link `?resource` = scope entity của grant: `Object::"<kind>:<id>"` cho object grant, `Workspace::"<id>"` cho ws grant, `Platform::"root"` cho platform grant.

## 4. `Resource::Object` + PEP `require_object`

`egret_core::authz::Resource` thêm variant mang ancestry PDP cần (để PDP không query object store):
```rust
pub enum Resource { Platform, Workspace(WorkspaceId), Object(ObjectRef) }
pub struct ObjectRef {
    pub kind: String,              // "dashboard" | "chart" | "dataset" | "folder"
    pub id: Uuid,
    pub folder_id: Option<Uuid>,   // folder của item (None = Uncategorized)
    pub workspace_id: WorkspaceId,
}
```
PEP mới cạnh `require`:
```rust
pub async fn require_object(pdp: &dyn Pdp, ctx: &WorkspaceContext, action: &str, object: ObjectRef) -> Result<()>;
```
Handler resolve `ObjectRef` từ object nó thao tác (đã load row, hoặc `SELECT folder_id, workspace_id FROM <table> WHERE id=$1` trước khi authorize). `build_entities` dùng field của `ObjectRef` dựng chuỗi §3; `authorize` truyền `Resource::Object`. `AuthTarget` không thêm impl — object request đi qua `require_object`, không phải `require` chung.

## 5. Grant store: object grants

- **Ghi** — object grant là row mới trong `access_grants`: `principal_*, role_id, scope_type='object', object_type=<kind>, scope_id=<object id>`. (Không bảng legacy, không trigger — write mới trực tiếp vào `access_grants`, độc lập data-model flip.) UNIQUE constraint sẵn nhận.
- **Đọc** — `AccessGrantStore::grants_for` mở rộng để trả thêm **object grant có scope là ancestor của request resource**:
  ```rust
  pub async fn grants_for(&self, user_id, ws: Option<WorkspaceId>, object_scope_ids: &[Uuid]) -> Result<Vec<Grant>>;
  ```
  với `object_scope_ids = [object.id, folder_id?]`. SQL thêm `OR (ag.scope_type='object' AND ag.scope_id = ANY($3))`. `Grant` thêm `object_type`. `link_grants` bind `?resource` = `Object::"<object_type>:<scope_id>"` cho object grant.

## 6. Share API (write/revoke/list)

Endpoint workspace-scoped mới (trong BI seam) — backend cho UI B2-1b:
- `PUT /ws/{ws}/objects/{kind}/{id}/grants` — grant `(principal, role)` trên object (folder HOẶC item). Gate bởi `require_object(pdp, ctx, "<kind>:manage", object)` — chỉ người manage được object mới share ("curate" kiểu Metabase).
- `DELETE /ws/{ws}/objects/{kind}/{id}/grants/{grant_id}` — revoke.
- `GET /ws/{ws}/objects/{kind}/{id}/grants` — list ai có quyền (grant trực tiếp; UI show inherited riêng).
Ghi/xoá object grant có publish `RolesChanged` không? KHÔNG — `RolesChanged` cho *định nghĩa* role/permission (rebuild template). Object grant là *assignment*, đọc live per-request bởi `grants_for` (như workspace_members), nên **không cần event** — request kế tiếp thấy ngay.

## 7. Migrate BI per-object handler

Hôm nay BI handler check `require(&ctx, "dashboard:read")` ở **workspace** scope. Migrate handler thao tác trên object **cụ thể** sang object scope:
- Handler có path `{id}` cho dashboard/chart/dataset → `require_object(&*st.pdp, &ctx, "dashboard:read"|"dashboard:manage"|…, object_ref).await?`, resolve `object_ref` (folder_id + workspace_id) của id đó trước.
- Handler **workspace-level** (list all, create mới không có parent object) giữ `require(&ctx, PERM)` ở workspace scope — ws grant vẫn authorize (và qua §2 cũng phủ object read cho member).
- **Behavior-preserving:** ws member với `ws_editor` vẫn manage mọi dashboard trong ws (ws grant cascade xuống object qua `in`). Object grant *thêm* quyền cho principal **không** phải ws member (hoặc role thấp hơn) — năng lực mới. Hành vi member cũ không đổi.

## 8. Testing

- **Regression matrix:** `pdp_regression` giữ 60/60 (§2).
- **Object-scope test mới:**
  - Folder grant `ws_viewer` cho U (không phải ws member) → U `dashboard:read` item **trong folder đó** (inherited), **không** đọc được item folder khác / Uncategorized.
  - Item grant `ws_editor` cho U → U `dashboard:manage` **đúng** dashboard đó, không phải sibling.
  - Ws grant vẫn phủ mọi object (hành vi member không đổi).
  - Platform (non-admin) grant **không** authorize object action (anti-leak).
  - `platform_admin` manage mọi object.
  - Share API: người manage-được grant/revoke; viewer không grant được (403).
- Full local gate: fmt, clippy, `cargo test --workspace`, FE `lint/typecheck/test/build` (share API đổi OpenAPI → regen FE client; chưa có UI).

## 9. Câu hỏi mở (cho plan)

- Tập chính xác BI per-object handler cần migrate (liệt kê route mang `{id}` dashboard/chart/dataset trong `egret-bi/src/routes.rs`); xác nhận read vs manage mỗi cái.
- Resolve `ObjectRef`: tái dùng row handler đã load, hay query ancestry riêng — chọn theo từng handler (tránh fetch đôi).
- Item "Uncategorized" (folder_id NULL) chỉ được ws grant phủ (đúng — không folder để grant); xác nhận messaging UI cho B2-1b.
- Endpoint share ở BI seam (`egret-bi`) hay surface mới `egret-sharing` — mặc định BI seam.
- List role share: chỉ `ws_viewer`/`ws_editor`, hay thêm `ws_admin`? Mặc định Viewer/Editor; xét "Manager" ở B2-3.

---

*Design v1.0 (2026-07-10). Backend object-level authorization cho BI content (folder + item, per-item + inherited), tái dùng workspace role tại object scope. Căng thẳng `in`/`==` của B1b-2 giải bằng cách cho operator template theo scope của role. UI share là B2-1b; type khác B2-2; role-hierarchy/deny B2-3; flip bảng legacy vẫn defer.*
