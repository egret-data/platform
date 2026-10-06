# Data Apps — thao tác trên danh sách

> Bản tham khảo tiếng Việt. Bản chuẩn là
> [`docs/en/specs/2026-10-06-apps-list-actions-design.md`](../../en/specs/2026-10-06-apps-list-actions-design.md);
> khi hai bản lệch nhau, bản tiếng Anh đúng.

**Trạng thái:** đề xuất, 2026-10-06. Quyết định A1–A3 đã chốt với người dùng; phần còn lại là
đề xuất của tài liệu này và được đánh dấu như vậy.

## 1. Vấn đề

Danh sách Apps (`frontend/src/routes/explore/Apps.tsx`) chỉ tạo, mở và sửa được app. Backend đã
phục vụ đổi tên, thùng rác, khôi phục, bookmark, thư mục và chia sẻ cho app, nhưng trang không
đưa ra thao tác nào trong số đó. Mọi danh sách Explore khác (Dashboards, Charts, Datasets) đều
có, trên cùng một khung.

Hai thao tác mà các danh sách khác có thì backend của app chưa có: **xoá vĩnh viễn** và **nhân
bản**. Ngoài ra, response của danh sách không cho biết người gọi được làm gì trên từng hàng, nên
trang không thể giấu một thao tác mà server sẽ từ chối.

## 2. Quyết định

| # | Quyết định | Ai chốt |
|---|---|---|
| A1 | **Xoá vĩnh viễn** nằm trong phạm vi: route backend mới và thao tác trong Thùng rác. | người dùng |
| A2 | **Nhân bản** nằm trong phạm vi: route backend mới và thao tác trên hàng. | người dùng |
| A3 | Backend trả `can_write` và `can_manage` **theo từng hàng**; trang chỉ hiện thứ người gọi làm được. | người dùng |
| A4 | Chỉ xoá vĩnh viễn được **app đã nằm trong thùng rác** (ngược lại 409). Hai bước có chủ đích trước khi nội dung bị huỷ. | đề xuất |
| A5 | Xoá vĩnh viễn cần **`app:manage`** trên app, như chia sẻ — cùng mức Dashboards đặt. | đề xuất |
| A6 | Nhân bản cần **`app:write` trên app nguồn** và `app:write` ở workspace. Không phải `app:read`: bản sao lấy từ **draft**, mà người chỉ đọc hiện không được xem draft chưa publish. | đề xuất |
| A7 | Bản sao chỉ chép source và layout của draft. Không chép version, chia sẻ, bookmark; người gọi là chủ; nằm cùng thư mục với app nguồn; tên `Copy of <tên>`. App trong thùng rác không nhân bản được (409). | đề xuất |
| A8 | Trang chuyển sang khung danh sách dùng chung (`ListPageShell`); nút **Open** / **Edit** trên hàng chuyển vào menu hàng như các danh sách khác. Nút đầu trang thành `+ App`. | đề xuất |

## 3. Backend

Tất cả trong `crates/egret-apps` (`routes.rs`, `store.rs`), sinh lại OpenAPI.

### 3.1 Quyền theo hàng (A3)

`AppSummary` và `AppDetail` thêm:

- `can_write: bool` — người gọi có `app:write` trên app này.
- `can_manage: bool` — người gọi có `app:manage` trên app này.

Cả hai được đánh giá theo từng object, với đúng `ObjectRef` mà các handler ghi dùng, để cờ và
lần từ chối sau đó không thể mâu thuẫn. Khác `DashboardSummary.can_manage` (theo workspace,
giống nhau ở mọi hàng): quyền của app được cấp theo từng app qua chia sẻ, nên một câu trả lời
chung cho cả danh sách sẽ sai với cộng tác viên chỉ được chia sẻ.

Chi phí: hai lần hỏi PDP cho mỗi hàng. Chấp nhận được với cỡ danh sách này; nếu hiện lên trong
profile thì gộp lại sau.

### 3.2 `POST /api/ws/{workspace}/apps/{id}/duplicate` (A2, A6, A7)

1. Tìm app trong workspace của người gọi (không có thì 404, cùng một body).
2. Yêu cầu `app:write` trên app nguồn và `app:write` ở workspace.
3. Từ chối app nguồn đang trong thùng rác: 409.
4. Đọc draft nguồn (source + layout) từ object store ở revision hiện tại.
5. Tạo app mới qua đúng đường `create` dùng — tên `Copy of <tên>`, mô tả và thư mục của app
   nguồn, chủ là người gọi — với source và layout đó làm draft revision 1.
6. Phát sự kiện `app:duplicate`. Trả 201 `AppDetail`.

### 3.3 `DELETE /api/ws/{workspace}/apps/{id}` (A1, A4, A5)

1. Tìm app trong workspace của người gọi (không có thì 404).
2. Yêu cầu `app:manage` trên object.
3. App phải đang trong thùng rác; ngược lại 409 kèm lời nhắc chuyển vào Thùng rác trước.
4. Trong một transaction: xoá các dòng `analytics.bookmarks`, `access_grants` và `tag_assignments`
   của nó (đa hình, không có FK), rồi xoá dòng `analytics.apps`. Draft, version, session, grant và pod token xoá
   theo qua cascade.
5. Sau khi commit, xoá prefix `…/{workspace}/{app}/` trong object store.
6. Phát sự kiện `app:delete`. Trả 204.

**Thứ tự có ý nghĩa.** Xoá dòng trước, object sau: lỗi giữa chừng để lại object mồ côi không ai
tham chiếu, chứ không để lại dòng trỏ vào object đã mất. Việc xoá object là best-effort — lỗi
được ghi log và không làm hỏng request mà các dòng đã bị xoá.

**Pod đang chạy.** Xoá app làm pod token và grant của nó mất theo, nên pod run của nó không gọi
được pod API nữa và request của viewer bị proxy từ chối; reaper sẽ dọn pod. Việc xoá không tự
gọi launcher để dừng pod.

**Phải kiểm, không được giả định** (mỗi mục là một test trong plan):

- trigger chỉ-thêm của `app_versions` cho cascade đi qua, kể cả tham chiếu `restored_from` giữa
  các version của cùng một app;
- id app của workspace khác trả 404, dòng và object của nó nguyên vẹn;
- prefix đưa cho `delete_prefix` đúng là prefix của một app. Object của app bên cạnh phải còn.

Test phá huỷ chỉ chạy trên `MemStore` và database test theo tiến trình — không bao giờ trên
bucket dùng chung.

## 4. Frontend

### 4.1 Trang

`Apps.tsx` dựng lại trên khung mà `Dashboards.tsx` dùng: tiêu đề `Apps` (`Trash` khi xem thùng
rác) và nút `+ App`; thanh công cụ gồm lọc Bookmarks, Bulk select, Trash, New folder; tìm theo
tên; dải tóm tắt; bảng Name · Status · Updated có sắp xếp; mục thư mục có thu gọn, kéo-thả và
đổi tên/xoá/chia sẻ thư mục, y như Dashboards.

Hàng: sao bookmark · tên · badge trạng thái. Tên dẫn tới app đã publish nếu có, nếu không thì
tới trình soạn thảo khi người gọi có quyền ghi, nếu không thì là chữ thường.

### 4.2 Menu hàng (`AppActionsMenu`)

| Mục | Hiện khi | Ghi chú |
|---|---|---|
| Open | luôn | vô hiệu kèm lý do khi chưa publish |
| Edit | `can_write` | |
| Rename… | `can_write` | hộp thoại tên + mô tả |
| Duplicate | `can_write` | chuyển tới trình soạn thảo của bản sao |
| Move to folder | `can_write` | |
| Share… | `can_manage` | `ShareDialog` sẵn có, `kind="app"` |
| Move to Trash | `can_write` | |

Trong Thùng rác: **Restore** (`can_write`) và **Delete permanently** (`can_manage`, qua hộp
xác nhận). Hàng không có mục nào thì không vẽ nút menu.

Thao tác bị **giấu, không phải vô hiệu**: một nút phá huỷ bị làm mờ vẫn quảng cáo một khả năng
người đọc không có. Open là ngoại lệ — nó không thay đổi gì, và trạng thái vô hiệu tự giải thích.

### 4.3 Chọn hàng loạt

Chỉ bật khi có ít nhất một hàng đang hiện có `can_write`. Ô chọn chỉ xuất hiện trên hàng ghi
được, để "Move n to Trash" không bao giờ đếm một hàng mà server sẽ từ chối.

### 4.4 Lớp API

`frontend/src/api/apps.ts` thêm `useApps(ws, { trashed })` và `useAppMutations(ws)`. Client
sinh lại bằng `npm run gen:api` — không dùng `orval` trần.

## 5. Ngoài phạm vi

Lịch sử version từ danh sách; chứng nhận, đăng ký báo cáo; dừng pod đồng bộ khi xoá.

## 6. Vấn đề có sẵn mà phần này thừa hưởng

Chia sẻ một **thư mục** cần `folder:manage`, quyền mà không vai trò nào có — lỗi có từ trước và
đã ảnh hưởng các danh sách khác. Nút Share của thư mục vẫn được nối ở đây cho đồng bộ và sẽ lỗi
y như vậy cho tới khi được sửa riêng.

## 7. Kiểm chứng

- Rust: test đơn vị và test có DB cho ba thay đổi backend, gồm các mục ở §3.3 và ma trận quyền.
- Frontend: test component cho việc giấu mục menu ở mọi tổ hợp quyền, chế độ thùng rác, chọn
  hàng loạt trên các hàng lẫn quyền, và ba đích của liên kết tên.
- Kiểm sống trong trình duyệt trên stack đang chạy: mỗi thao tác một lần, với admin và với
  cộng tác viên chỉ được chia sẻ; một lần xoá vĩnh viễn được xác nhận bằng cách liệt kê bucket.
