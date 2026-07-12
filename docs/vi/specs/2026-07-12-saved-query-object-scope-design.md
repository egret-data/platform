# Saved-Query Object-Scope — Thiết kế

**Trạng thái:** Đã duyệt
**Ngày:** 2026-07-12
**Effort:** B (thống nhất phân quyền) — hạng mục còn lại sau B4
**Sao chép mẫu:** B2-2 (notebook object-scope), B2-1a (nền BI object-scope)

> Bản tiếng Việt là tài liệu tham chiếu; bản chuẩn (canonical) là
> `docs/en/specs/2026-07-12-saved-query-object-scope-design.md`.

## Mục tiêu

Đưa saved query của SQL Lab lên ngang hàng đầy đủ với chart / dashboard / notebook:
**cấp quyền theo object, chia sẻ được, tổ chức theo folder** — do Cedar PDP quản trị
như mọi object BI khác — thay cho mô hình phẳng, toàn-workspace, khóa-theo-chủ-sở-hữu
hiện tại.

## Hiện trạng

`analytics.saved_queries` hiện tại:

- Cột: `id, workspace_id, database_id, name, sql, description, created_by, created_at,
  updated_at`. **Không có `folder_id`** (chart/dataset/notebook đều có).
- Cả năm handler (`save`/`list`/`load`/`update`/`delete`) gate bằng `require(query:run)`
  ở **workspace scope**; `update`/`delete` thêm `require_owner`. Hệ quả: *mọi* thành
  viên ws có `query:run` thấy **tất cả** saved query của workspace; không share cho
  object-guest được; chỉ chủ sở hữu sửa được.
- **IDOR chéo-workspace tiềm ẩn:** `load`/`update`/`delete` fetch `WHERE id = $1` không
  kèm điều kiện `workspace_id`, nên thành viên workspace A (có `query:run`) load được
  saved query của workspace B theo id. Object-scope qua `resolve` vá luôn lỗ này.

`folders` đã là per-type qua cột text `entity_type`
(`'chart' | 'dashboard' | 'dataset' | 'notebook'`); thêm `'saved_query'` không cần đổi
schema. `shares.rs` (`object_table` / `manage_action` / `object_ref` + share API
`/objects/{kind}/{id}/grants`) đã generic theo `kind`; chỉ cần thêm một arm.

## Thiết kế

### 1. Mô hình dữ liệu — migration `0077_saved_query_objects.sql`

```sql
ALTER TABLE analytics.saved_queries
  ADD COLUMN folder_id uuid REFERENCES analytics.folders(id) ON DELETE SET NULL;
CREATE INDEX saved_queries_folder_idx ON analytics.saved_queries (folder_id);

-- Cấp 2 quyền mới cho các role workspace được seed (mirror mig 0070).
-- ws_viewer -> saved_query:read
-- ws_editor, ws_admin -> saved_query:read + saved_query:manage
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'saved_query:read'   FROM roles WHERE name IN ('ws_viewer','ws_editor','ws_admin')
ON CONFLICT DO NOTHING;
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'saved_query:manage' FROM roles WHERE name IN ('ws_editor','ws_admin')
ON CONFLICT DO NOTHING;
```

- `folders.entity_type` nhận thêm giá trị `'saved_query'` lúc runtime (không DDL — là
  cột text tự do; folder CRUD đã generic theo `entity_type`).
- `folder_id` nullable: saved query cũ nằm ở root folder ngầm, giống chart trước B2.

### 2. Catalog quyền — `crates/egret-core/src/rbac.rs`

Thêm 2 quyền grantable, object-scopable cạnh các mục `chart:*` / `notebook:*`:

- `saved_query:read`
- `saved_query:manage`

**Không** thêm vào `ADMIN_GRAPH_ACTIONS` — hành xử như `chart:*` (cấp cho bất kỳ role,
object-scope được), không phải hành động admin-only.

### 3. Nối authz — `crates/egret-bi/src/shares.rs`

- `object_table`: thêm `"saved_query" => Some("saved_queries")`. (Helper trả tên bảng
  **trần**; `object_ref` tự prefix `analytics.` nên không thêm schema ở đây.)
- `manage_action`: không cần đổi — nhánh mặc định đã ra `format!("{kind}:manage")` =
  `"saved_query:manage"`.
- `object_ref`: không cần đổi — câu generic
  `SELECT folder_id, workspace_id FROM analytics.{table} WHERE id = $1` chạy được ngay
  khi cột tồn tại.
- **Admission object-guest** (`has_object_grant_in_workspace`, arm admission của
  WsContext): thêm nhánh `"saved_query"` mirror nhánh `"notebook"`, để principal chỉ
  có object grant saved_query vẫn được admit vào workspace với object scope.

### 4. Handler — `crates/egret-bi/src/routes.rs` (migrate cả 5)

| Handler  | Từ                                | Sang                                                      |
|----------|-----------------------------------|-----------------------------------------------------------|
| `save`   | `require(query:run)`              | `require(saved_query:manage)` (tạo ở mức ws)              |
| `list`   | `require(query:run)`              | `require(saved_query:read)`, rồi lọc theo hàng nhìn-thấy-được (owned ∪ folder-granted ∪ object-granted), mirror chart `list` |
| `load`   | `require(query:run)`              | `require_object(saved_query:read, resolve("saved_query", id))` |
| `update` | `require(query:run)` + `require_owner` | `require_object(saved_query:manage, resolve(...))` — **bỏ owner-gate**, sửa theo role/grant (như B2-1a) |
| `delete` | như trên                          | `require_object(saved_query:manage, resolve(...))`        |

Thêm:

- Handler `set_folder` mới cho saved_query (mirror chart `set_folder`): `PUT
  /ws/{ws}/sqllab/queries/{id}/folder`, body `{ folder_id: Option<Uuid> }`, gate
  `require_object(saved_query:manage, ...)` + `require_folder(...)` khi gán folder
  (kiểm folder cùng workspace và `entity_type = 'saved_query'`).
- `SavedQueryDto` / `SavedQuery` / `SavedQueryRepo` thêm `folder_id: Option<Uuid>`
  (repo `COLS`, `create`, `update`, `list`, `get`, và mapper `dto()`).
- Bỏ helper `require_owner` cho saved query nếu không còn caller nào khác dùng.

`list` lọc theo đúng tiền lệ chart-list: caller có role workspace cấp `saved_query:read`
thấy toàn bộ saved query của ws; object-guest chỉ thấy các saved query được cấp cho họ.

### 5. Share API + folder — đã generic (không route mới)

- `PUT/DELETE/GET /ws/{ws}/objects/saved_query/{id}/grants` chạy được ngay khi
  `object_table` chấp nhận kind.
- `GET/POST/DELETE /ws/{ws}/folders?entity_type=saved_query` đã sẵn.

### 6. Frontend — `frontend/src/components/sqllab/SavedTab.tsx` + share/folder

- **Folder:** tái dùng `FolderHeaderRow` + gom nhóm theo folder-tree (copy pattern list
  của Charts/Notebooks), `useFolders('saved_query')`, và row action move-to-folder gọi
  endpoint `set_folder` mới.
- **Share:** thêm row action `Share…` mở `ShareDialog` sẵn có với `kind='saved_query'`;
  mở rộng union `ShareKind` và map `objectKindLabel` với `saved_query`.
- orval regen tự nhận quyền mới và trường DTO `folder_id`.

## Kiểm thử

**Backend**

- Unit `shares`: `object_table`/`object_ref`/`manage_action` resolve được `saved_query`.
- Test handler:
  - Object-guest có grant saved_query load (và chạy) được query được cấp nhưng 403/404
    với query không được cấp.
  - **Regression IDOR:** load saved query thuộc workspace khác giờ trả `NotFound`.
  - `update`/`delete` bởi `ws_editor` không phải chủ sở hữu giờ thành công (đã bỏ
    owner-gate → theo role/grant).
  - `list` chỉ trả hàng nhìn-thấy-được cho object-guest; toàn bộ hàng ws cho người có
    role workspace.
  - `set_folder` từ chối folder của workspace khác / sai `entity_type`.

**Frontend (Vitest)**

- `SavedTab` render folder header và gom query dưới folder.
- `SavedTab` có action `Share…` mở `ShareDialog` với kind `saved_query`.

**Security review**

- Subagent review bảo mật đối kháng trước khi merge (chuẩn từng increment): tập trung
  vào lỗ IDOR đã vá, admission object-guest, guard folder chéo-workspace, và không path
  SQL saved-query nào splice `kind` như identifier.

## Ngoài phạm vi

- Delegation data-plane per-user cho *thực thi* saved query — đã có ở B3a (SQL Lab
  `run_query` đã mint JWT engine per-user); epic này quản trị *object* saved query, path
  chạy không đổi.
- Versioning / lịch sử saved query.
- Chia sẻ saved query chéo-workspace (object grant nằm trong 1 workspace, như mọi object
  BI khác).

## Ghi chú migration / rollout

- Migration `0077` additive, không cần backfill (`folder_id` nullable, hàng cũ → root
  folder; hàng role_permissions mới idempotent qua `ON CONFLICT DO NOTHING`).
- Thay đổi hành vi cho user hiện tại: sửa saved query không còn khóa-theo-chủ — mọi
  ws_editor/ws_admin sửa được, khớp chart/dashboard/notebook. Đây là thay đổi parity
  chủ đích (ghi ở đây để review không bất ngờ).
- Nhớ `touch crates/egret-metastore/src/lib.rs` để migrator nhúng lại migration mới
  (gotcha sqlx staleness đã biết).
