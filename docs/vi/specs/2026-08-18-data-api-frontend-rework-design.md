# Làm lại frontend Data API, và quy ước trang list của egret

Hai sản phẩm nhưng là một việc: đưa các trang Data API lên ngang những gì phần còn lại của Explore đã làm được, và — vì làm điều đó bằng copy-paste sẽ là bản sao thứ năm — rút phần dùng chung ra trước rồi ghi kết quả thành chuẩn cho mọi trang Explore.

*(Bản tiếng Anh tại `docs/en/specs/2026-08-18-data-api-frontend-rework-design.md` là bản chuẩn; bản này để tham chiếu.)*

## 1. Vì sao

Các trang Data API được xây từng trang theo yêu cầu riêng, và phần còn lại của Explore trước đó cũng vậy. Kết quả là thứ **đo được** chứ không phải chuyện khẩu vị, nên nó đã được đo trước khi spec này được viết.

| Năng lực | Datasets / Charts / Dashboards / Alerts | Endpoints | Clients |
|---|---|---|---|
| Bookmark, và lọc chỉ bookmark | cả bốn | **không** | **không** |
| Bulk select + hành động hàng loạt | cả bốn | **không** | **không** |
| Trash / restore / xoá vĩnh viễn | cả bốn | **không**, chỉ xoá cứng | **không** |
| Folder: tạo, đổi tên, chuyển, kéo-thả | cả bốn | **nhóm theo folder nhưng không đặt được** | **không** |
| Menu hành động của hàng | mỗi domain một menu riêng | inline, thiếu chuyển/chia sẻ/nhân bản | **không có menu; nút rải trong hàng** |
| Facet filter | ba trên bốn | chỉ search | chỉ search |
| Share / Tags | ba trên bốn | **không** | **không** |
| Summary strip | cả bốn | có | có |
| Phân trang | không trang nào | không | **có — trang duy nhất** |

Hai phát hiện từ khảo sát đó làm đổi cách tiếp cận:

- **`components/explore/BulkBar.tsx` và `components/explore/EmptyState.tsx` đã tồn tại và không file nào import.** Cả bốn trang có bulk-select đều tự viết lại nút toggle và thanh hành động ngay trong trang, và cả sáu trang đều tự viết lại khối empty-state viền đứt y hệt nhau. Chép mẫu đó sang Data API sẽ là bản sao thứ năm và thứ sáu của thứ vốn đã tồn tại dưới dạng component.
- **Logic bookmark bị copy-paste bốn lần**, không có component dùng chung nào.

Nên công việc không phải là "làm cho Data API giống Datasets", mà là "rút phần bốn trang kia vốn đã dùng chung, chứng minh nó trên những trang chưa có gì, rồi ghi nó xuống".

### Phần không thể chỉ làm ở frontend

Yêu cầu là việc frontend. Ba trong bảy năng lực không thể giao theo cách đó, và điều này nói ngay từ đầu chứ không để phát hiện giữa chừng:

| Năng lực | Việc backend | Cần migration? |
|---|---|---|
| Bulk select, trang New, layout detail, facet, row menu | không | không |
| Bookmark | hai route `POST`/`DELETE /ws/{workspace}/data-api/endpoints/{id}/bookmark` | **không** — `bookmarks` vốn generic (`entity_type text`, `entity_id uuid`) |
| Chuyển vào folder | `folder_id` trong `UpdateEndpointInput` và store | **không** — cột `dataapi.endpoints.folder_id` đã có |
| Trash | `deleted_at`, store, route, **và đường serving phải loại endpoint đã trash** | **có** |

Trash là cái mang cạnh nguy hiểm, và §6 nói về cạnh đó chứ không nói về UI.

## 2. Primitive dùng chung, rút ra trước

Data API là consumer đầu tiên vì hôm nay nó **không có gì**: áp dụng ở đó không thể làm hỏng một trang đang chạy. Bốn trang cũ migrate lần lượt ở các PR sau, cố ý nằm ngoài phạm vi spec này.

Mỗi primitive dưới đây thay thế một thứ đang tồn tại dưới dạng 4–6 bản gần như giống hệt. Không cái nào là ý tưởng mới; mỗi cái chỉ là **cái tên** cho một hình dạng codebase đã lặp lại sẵn.

| Primitive | Thay thế | Ghi chú |
|---|---|---|
| `ListPageShell` | header `p-4` + `flex items-center justify-between`, viết tay ở cả sáu trang | Nhận `title`, `description` tuỳ chọn, một slot toolbar và một slot hành động chính. |
| `BookmarkToggle` | bốn bản sao của nút sao và state `onlyBookmarked` | Nhận mutation add/remove; không sở hữu gì khác. |
| `BulkBar` + `useBulkSelect` | bốn bản tự viết lại | **Component đã tồn tại và không ai dùng** — hãy dùng nó thay vì viết bản thứ năm. |
| `EmptyState` | sáu bản sao của khối viền đứt | **Đã tồn tại và không ai dùng.** Phải phủ **ba** trạng thái, không phải hai: rỗng, lọc-ra-rỗng, và **lỗi**. |
| `TrashToggle` | bốn bản sao của bộ ba trash/restore/xoá vĩnh viễn | |
| `FolderToolbar` | bốn bản sao của new-folder + move-to-folder + kéo-thả | |

**`EmptyState` phải giữ lỗi tách khỏi rỗng.** Ở phase trước, hai account picker đã gộp `isError` vào "rỗng" và bảo người vận hành đi tạo một service account **vốn đã tồn tại** — một ngõ cụt kèm lời giải thích sai. Một component dùng chung cho phép gộp như vậy sẽ **lan** lỗi đó thay vì chặn nó.

**Không primitive nào sở hữu việc fetch dữ liệu.** Mỗi cái nhận state và callback. Một primitive tự fetch sẽ biến query key của mọi trang thành việc của nó, và lỗi P5b — sáu mutation báo thành công trên một danh sách cũ — đến từ đúng kiểu ràng buộc đó khi nó ngầm định thay vì tường minh.

## 3. Hai trang list

`Endpoints` và `Clients` áp dụng mọi primitive trên, cộng thêm:

- **Facet filter.** Endpoints có thêm `state` (live / draft / disabled) và `engine`. Hôm nay bộ lọc state có tồn tại nhưng chỉ chạm tới được bằng cách bấm chip trên `SummaryStrip`, và như vậy nó không hiện diện như một bộ lọc.
- **Menu hành động của hàng.** Cả hai trang có `*ActionsMenu` riêng như bốn domain kia, thay vì nút rải trong hàng. Clients hiện không có menu nào.
- **Search nằm ở toolbar của trang, không nằm trong bảng.** `Clients` hiện dựa vào ô search nội bộ của `FilterableTable`, nên search của nó nằm ở chỗ khác mọi trang Explore còn lại.

**Không thêm certification.** Endpoint và client không có trường `certified`, và bịa ra một trường để giống Datasets là thay đổi mô hình dữ liệu khoác áo thay đổi UI.

## 4. Ba trang New riêng

`endpoints/new`, `clients/new`, và luồng cấp client credential trở thành route thay vì dialog.

Tiền lệ là `charts/new` và `alerts/new`: cả hai đăng ký literal `new` **trước** `:id` (nếu không literal sẽ bị bắt làm id) và cả hai render **cùng component với route detail**, chỉ khác là URL chưa có id. Data API theo đúng vậy chứ không phát minh hình dạng thứ ba.

**`App.tsx:124-127` hiện có comment lập luận NGƯỢC LẠI**, với lý do dialog tạo chỉ hỏi những gì một hàng không thể thiếu rồi bàn giao ngay cho editor thật. Lý lẽ đó đã bị thay thế, và comment phải được thay chứ không để đứng cạnh quyết định trái ngược — hai lý lẽ mâu thuẫn trong một file còn tệ hơn một trong hai.

Hệ quả phải thiết kế cho: route `/new` **không có id**, nên mọi thứ khoá theo id phải chịu được sự vắng mặt của nó. Shell detail hiện remount theo `id` bằng `key={id}`; khi không có id thì không có gì để khoá, và lần lưu đầu tiên phải điều hướng sang id vừa tạo mà không làm mất thứ tác giả đã gõ.

## 5. Nội dung detail: một bảng mỗi tab

**Vấn đề là card, không phải bảng.** Các mục detail hiện xếp chồng vài card `ConfigSection`, mỗi card đóng khung một bảng một hai dòng. Đọc dọc trang, đó là một cột những cái hộp gần như rỗng.

Mỗi tab thành **một bảng full width**. Các nhóm (`Endpoint`, `Cache`, `Limits`, `Rate limit`) thành **hàng tiêu đề trải ngang bảng**, không phải khung riêng:

```text
Setting          Details                              Value
-----------------------------------------------------------------
ENDPOINT
Name             Endpoint hiện trong danh sách thế nào [ … ]
Path             URL consumer gọi                      [ … ]
-----------------------------------------------------------------
CACHE
Enabled          Phục vụ lại từ cache thay vì chạy lại ( o)
```

- **Bỏ `max-w-3xl` trên `Outlet` của detail.** Nó là nguyên nhân trực tiếp của khoảng trống ngang: cột `Details` xuống ba dòng trong khi một phần ba bên phải trang bỏ không.
- **Cột `Details` có max width riêng**, để đoạn văn giải thích không thành một dòng dài không đọc nổi ở 2560px, còn cột `Value` lấy phần dư nó cần cho input dài (SQL, path).
- `ConfigSection` chỉ nghỉ **ở route detail của Data API**. Chín chỗ dùng ở Control Plane giữ nguyên; sửa chúng không phải việc của spec này.
- **`ConfigTableRow.details` vẫn là chuỗi thuần, không render markdown.** Backtick viết vào đó hiện ra nguyên ký tự — một lỗi thật tìm được trên trình duyệt ở phase trước. Bảng rộng hơn làm người ta muốn viết dài hơn; ràng buộc thì không đổi.

Tab `Query` là ngoại lệ và giữ nguyên hình dạng hiện tại: một editor SQL và một lưới kết quả không phải là *setting*, và ép chúng vào hàng `Setting | Details | Value` là đúng sai lầm của card, lộn ngược.

## 6. Trash, và cạnh thật sự quan trọng

Schema, store và route thì thẳng thớm. Một quy tắc thì không:

> **Endpoint đã vào trash phải NGỪNG phục vụ.** `serving/router.rs` resolve endpoint theo path và method rồi join `endpoint_versions` qua `live_version_id`. Nếu truy vấn đó không loại các hàng đã trash, một endpoint xoá mềm **vẫn trả lời lưu lượng consumer** — và việc xoá trở nên vô hình với chính người vừa xoá.

Đây không phải giả định. Chương trình Alerts đã ship đúng lỗi này: một schedule xoá mềm **vẫn tiếp tục bắn** cho tới khi thêm `deleted_at IS NULL` vào truy vấn claim. Cùng hình dạng, chỉ khác một subsystem.

**Trash chỉ nhận endpoint `draft` và `disabled`.** Endpoint `live` phải disable trước. Hai lý do:

1. Nó giữ trạng thái trung thực. Một hàng ghi `live` mà không phục vụ gì, hoặc ghi `trashed` mà vẫn phục vụ, đều là nói dối.
2. Nó tránh tạo **đường thứ hai** để "ngừng phục vụ" chạy song song với `disable`. Hai cơ chế cùng chặn lưu lượng là hai chỗ để quên một, và cái bị quên luôn là cái mới hơn.

Việc từ chối phải là từ chối thật ở API, không phải một nút bị disable: nút mà UI giấu đi không phải là quy tắc mà server thực thi.

Restore đưa endpoint về `disabled`, không bao giờ thẳng về `live`. Ra khỏi thùng rác không phải là publish, và việc phục vụ lại lưu lượng nên là một hành động thứ hai có chủ ý.

## 7. Tài liệu quy ước

`docs/en/ui-conventions.md`, kèm bản vi, viết **từ** các primitive ở §2 chứ không viết trước chúng — một quy ước chưa ai hiện thực hoá chỉ là một điều ước.

Nó bao gồm: giải phẫu trang list và thứ tự các nút trên toolbar; khi nào luồng tạo là một trang và khi nào dialog vẫn đúng; bảng `Setting | Details | Value` và khi nào card là xứng đáng; ba trạng thái mọi danh sách phải phân biệt (loading, rỗng, lỗi) và vì sao lỗi không bao giờ được gộp vào rỗng; và các quy tắc bề ngang.

Mỗi quy tắc dẫn chiếu tới code hiện thực nó, và nếu có, tới lỗi đã thúc đẩy nó. Một quy tắc không có cả hai là một ý kiến, mà ý kiến thì thuộc về comment review chứ không thuộc về một chuẩn.

## 8. Kiểm thử

- **Mỗi primitive có test riêng**, không chỉ test của một trang dùng nó. Một primitive chỉ được test qua một consumer thì cách "không được test" đúng một lần refactor.
- **`EmptyState` phải có test rằng lỗi không hiện ra như rỗng**, mutation-check bằng cách gộp hai trạng thái lại và xem nó fail.
- **Trash: test rằng endpoint đã trash ngừng phục vụ**, chạy qua serving router thật chứ không qua store. Mutation-check bằng cách bỏ predicate `deleted_at` — mutation đó **phải** làm test fail; nếu không, test đang kiểm sai tầng.
- **Trash: test rằng API từ chối trash một endpoint `live`**, ở tầng route chứ không trong component.
- **Test route `/new` phải mount đúng cây route mà ứng dụng thật sự có.** Ở phase trước, một test mount `/explore/:ws/data-api` — cây mà app không hề có — và xanh trên code chết.
- Assertion phải mutation-check trước khi tin. Mutation không build được thì không chứng minh gì — build trước, xác nhận 0 lỗi, rồi mới đọc kết quả. Và hãy assert **kết quả cụ thể** thay vì `is_err()`: một mutation gần đây gỡ hẳn một scan kiểm tra mà input **vẫn** bị từ chối, bởi một quy tắc khác, với một lỗi khác.

## 9. Phân rã

Sáu phase. Mỗi phase ship và review độc lập được; phase sau có thể bỏ mà không phải gỡ phase trước.

1. **Primitive** — sáu component ở §2, kèm test riêng. Không đụng trang nào.
2. **Hai trang list Data API** — áp dụng primitive; facet; row menu. Chỉ frontend.
3. **Bookmark + chuyển folder** — hai route, một trường input; rồi nối UI.
4. **Trang New** — ba route; cho dialog nghỉ; sửa comment ở `App.tsx`.
5. **Layout detail** — một bảng mỗi tab; bỏ giới hạn bề ngang; cho `ConfigSection` nghỉ ở đây.
6. **Trash** — migration, store, route, phần loại trừ ở serving, và UI sau cùng.

Tài liệu quy ước viết ở cuối phase 5 và cập nhật bởi phase 6, để nó mô tả thứ **đang tồn tại**.

**Phase 1 và 2 phải land gần nhau.** Một primitive không có consumer thì trên thực tế không review được: không có gì cho thấy interface của nó đúng hay sai.

## 10. Hoãn lại, kèm lý do

- **Migrate Datasets / Charts / Dashboards / Alerts sang primitive.** Điểm của việc làm Data API trước là nó không thể phá cái gì; migrate bốn trang đang chạy là rủi ro riêng với vòng review riêng.
- **Certification cho endpoint và client** — thay đổi mô hình dữ liệu, không phải UI.
- **Share và Tags cho đối tượng Data API.** Mô hình truy cập của endpoint là Cedar cộng grant invoke theo từng endpoint, còn `ShareDialog` nói ngôn ngữ mô hình đối tượng analytics. Cho hai thứ đó gặp nhau là một thiết kế riêng.
- **Phân trang cho năm trang list còn lại.** `Clients` có nó vì `FilterableTable` sẵn có; số còn lại render mọi hàng. Đáng làm, nhưng không liên quan việc này.
- **Cho `ConfigSection` nghỉ ở Control Plane.** Chín chỗ dùng, không chỗ nào là việc của lần này.
