# Audit StarRocks phía platform: Query log (thiết kế)

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/specs/2026-11-09-starrocks-audit-platform-design.md). Mọi giá trị chính xác nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Trạng thái:** người dùng đã duyệt. Bốn câu hỏi mở được quyết ngày 2026-11-10, xem mục cuối.

> **Đính chính (2026-11-10, từ plan).** Bản tiếng Anh đã nêu ở mục 1: provisioner chỉ render
> object Kubernetes nên setup nằm trong `egret-engine-mgmt`; bootstrap không chạy sau mọi lần apply
> (nên có `spawn_query_log_setup`); engine không gắn profile chạy native access control nên egret
> phải `GRANT INSERT` cho loader.

**Gói:** A trong các follow-up StarRocks. Thứ tự là B (đã merge, platform #847), A, rồi C. A dùng `egret_perm::is_patched_starrocks_fe` và nút **Upgrade security** của B.

## Vấn đề

egret chưa trả lời được câu "ai đã chạy gì trên engine StarRocks này, và có được phép không":
- audit của plugin Ranger bị tắt;
- `fe.audit.log` nằm trên `emptyDir`, nên mất khi pod khởi động lại, và egret không hiển thị nó;
- query uỷ quyền đăng nhập bằng service user rồi `EXECUTE AS`, nên cột `user` không cho biết người thật.

Image fork egret.4 trở đi có sẵn plugin AuditLoader ở `/opt/egret/auditloader.zip`, nhưng egret chưa cài nó.

## Đã quyết

- Plugin AuditLoader ghi vào bảng trong engine, giữ 30 ngày. egret đọc bảng này và hiển thị ở tab **Query log** trên trang Engine detail, với quyền `engine:read`.
- Phạm vi:
  - tạo `starrocks_audit_db__.starrocks_audit_tbl__` theo DDL của fork;
  - tạo user nạp dữ liệu chỉ có quyền INSERT, cùng policy Ranger cho user đó;
  - `INSTALL PLUGIN`, rồi chờ plugin refresh;
  - chặn user thường trên database audit;
  - API, tab Query log và docs.
- Engine dùng image không có plugin thì được báo rõ, và apply không lỗi. Sau khi B nâng cấp image, setup chạy lại.

## Thiết kế, tóm tắt

1. **Setup:** hàm idempotent `ensure_query_log` trong `egret-engine-mgmt/src/query_log.rs`. Nó chạy ở ba chỗ:
   - bootstrap sau khi FE Ready;
   - sau mỗi lần apply StarRocks có thể khởi động lại FE (sửa image, và nâng cấp của B);
   - route `POST …/query-log/setup` (quyền `engine:update`).

   Các bước, theo thứ tự:
   - từ chối nếu tên user nạp đã thuộc về một người;
   - chờ có compute node hoặc backend đang sống (thêm ngày 2026-11-11): đọc `SHOW COMPUTE NODES` và `SHOW BACKENDS` (cột `Alive`, tìm theo header) mỗi 10 giây, tối đa khoảng 5 phút. Hết hạn thì setup trả `Failed` với thông báo rõ ràng, và cơ chế thử lại vẫn áp dụng. Route đồng bộ chỉ kiểm tra một lần. Lỗi "available CN nodes is 0" của FE cũng là `Failed`, không phải trạng thái cuối;
   - tạo database và bảng;
   - tạo user `egret_query_log`, rồi `ALTER USER` để mật khẩu khớp;
   - chỉ khi engine không gắn profile: `GRANT INSERT`;
   - `SHOW PLUGINS`, rồi `INSTALL PLUGIN` chỉ khi image đã vá;
   - chờ có event (chỉ trong chạy nền).
2. **User nạp dữ liệu:** `egret_query_log`, được migration 0218 giữ chỗ trong `username_ledger`. Mật khẩu là secret hệ thống `egret-query-log-<id>`; id lưu ở cột mới `query_log_secret_id`.
3. **Ranger:** hai loại infra mới trong IR:
   - `egret-query-log-loader` cho phép `insert`;
   - các dòng `egret-query-log-guard - …` cấm nhóm `public` mọi quyền trên database audit, trừ `insert` của user nạp.

   Lệnh cấm này chặn cả engine admin (câu hỏi mở 2). egret đọc bằng `root`, user mà Ranger không kiểm tra.
4. **Image không có plugin:** trạng thái `pluginNotShipped`. Setup vẫn tạo bảng và user, chỉ không cài plugin. Tab chỉ tới nút **Upgrade security**.
5. **Đọc log:** có ba route `engine:read`:
   - `status`: một trong `ready`, `notSetUp`, `pluginNotShipped`, `unreachable`, cùng `lastEventAt`, đều đọc trực tiếp từ FE;
   - danh sách: lọc theo `from`, `to` (tối đa 31 ngày), `user`, `state`, `limit` (tối đa 200), phân trang bằng cursor;
   - chi tiết một query.

   Giá trị request đều đi qua `quote_string`. Statement bị cắt ở 2.000 ký tự, và được che mật khẩu theo giá trị chính xác lẫn theo dạng. Các lần đọc của chính egret bị loại khỏi kết quả. Mọi kết nối MySQL mà egret mở tới FE StarRocks, gồm kết nối `root` của egret lẫn kết nối của user và kết nối uỷ quyền qua connector StarRocks của egret-query, đều tự đặt `max_allowed_packet` và `wait_timeout`, nên driver không gửi câu `SELECT @@max_allowed_packet,@@wait_timeout` khi kết nối, và log không có dòng đó dưới tên `root` hay tên user (thêm ngày 2026-11-11).
6. **Tab:** chỉ có ở engine StarRocks. Tab có bộ lọc trong URL, bảng ưu tiên cột User (`authorizedUser`), panel chi tiết, và nút **Load more**. Text của dòng trạng thái giữ nguyên tiếng Anh như trên giao diện:
   - `notSetUp`: "The query log is not set up on this engine: its audit plugin or its log table is missing.", kèm nút **Set up query log** (cần `engine:update`; khi bị tắt, nút ghi lý do "Setting it up needs the engine:update permission.");
   - `pluginNotShipped`, có hai biến thể (Task 6):
     - khi engine mang cờ `securityUpgradeNeeded` của B, text chỉ tới nút của B nằm trong cảnh báo bảo mật phía trên các tab: "This engine's image has no audit plugin. **Upgrade security**, in the security warning on this page, moves it to egret's image, and the Query log starts after that.";
     - khi không có cờ đó thì không có nút để nhắc tới: "This engine's image has no audit plugin. The Query log starts once the engine runs egret's image."
7. **Gia cố:** `plugin_enable` và `plugin_dir` bị cấm trong `feConfig`. Đã đo ngày 2026-11-11: plugin còn sau khi FE khởi động lại, nên không cần chuyển `plugin_dir` (xem mục "Đo trên kind, 2026-11-11").

**Đã đo ở Task 5** (2026-11-10, image egret.4; chi tiết và text lỗi chính xác ở bản tiếng Anh):
- StarRocks tự che mật khẩu trong `stmt` của audit (`IDENTIFIED BY '***'`, `"password" = ***`);
- lỗi khi thiếu zip, khi plugin đã cài, khi thiếu database (5501) và thiếu bảng (5502) đã được ghi lại và ghim trong test;
- múi giờ của FE là `Etc/UTC`, nên các lần đọc giữ UTC;
- vẫn chưa đo: plugin có còn sau khi FE khởi động lại hay không (đã đo ngày 2026-11-11, xem mục cuối).

## Kiểm thử

- unit;
- IR và render;
- test DB;
- test với StarRocks thật (`EGRET_TEST_STARROCKS_URL`, image egret.4 và image chính thức);
- e2e của fork cho guard;
- vitest;
- kiểm tra live trên kind.

## Đã quyết ngày 2026-11-10 (trước là câu hỏi mở)

1. Ai đọc được nội dung statement: quyền mới `engine:audit`, migration 0218 cấp cho các role có `engine:update`.
2. Truy cập SQL trực tiếp vào database audit: không ai được, kể cả engine admin.
3. Audit của Ranger: không hiển thị.
4. Engine đã có từ trước: egret tự chạy setup một lần lúc khởi động, và nút **Set up query log** vẫn giữ để khôi phục.

## Đã quyết ngày 2026-11-11 (review Task 4)

1. **Nội dung lỗi cũng cần `engine:audit`.** Thông báo lỗi của StarRocks trích lại statement bị từ chối (`Unexpected input 'xyz'`, `Cast '<literal>' to INT failed`), nên `errorMessage` cũng là nội dung statement. Không có `engine:audit` thì entry không có cả `statement` lẫn `errorMessage` (không có trong JSON, chứ không phải `null`), và `statementTruncated` là false. Các trường khác giữ nguyên, kể cả `state`, nên statement lỗi vẫn hiện là lỗi. Áp dụng cho cả route danh sách lẫn chi tiết.
2. **Bộ lọc `user` đọc `authorizedUser` giống như UI hiển thị.** Tên là phần giữa dấu `'` đầu tiên và `'@'<host>'` cuối cùng, `''` được đọc thành một dấu nháy, rồi chuyển chữ thường: `` lower(replace(regexp_extract(authorizedUser, '^''(.*)''@''[^'']*''$', 1), '''''', '''')) ``. Nhờ vậy `'o''brien'@'10.%'` được tìm thấy bằng user `o'brien`, đúng tên mà `authorizedUser` hiển thị.

## Đo trên kind, 2026-11-11

Từ đợt kiểm tra live của gói này trên kind (FE `4.1.7-egret.4`, 1 FE và 1 CN, bật Ranger):

1. **Fact 1: plugin còn sau khi pod FE bị tạo lại.** FE giải nén lại plugin từ file zip trong image (`/opt/egret/auditloader.zip`), và meta của FE nằm trên PVC. Khoảng 2 phút event quanh lúc khởi động lại bị mất. Không cần chuyển `plugin_dir`.
2. **Dòng audit không có text lỗi.** `errorCode` chỉ chứa mã lỗi. Đã đo: `INTERNAL_ERR` khi Ranger từ chối (`SELECT` trên cột bị cấm), và `ANALYSIS_ERR` khi `EXECUTE AS` một user không tồn tại. Không có cột chứa thông báo, nên UI hiển thị mã lỗi chứ không phải "Access denied". Dòng "Access denied" trong phần facts của bản tiếng Anh đã được đính chính.
3. **Giới hạn đã biết: `count(*)` trên bảng audit không bị chặn.** StarRocks kiểm tra `SELECT` theo từng cột được tham chiếu, và `SELECT count(*)` không tham chiếu cột nào, nên guard theo cột không áp dụng. Ai kết nối được cũng biết số dòng của bảng. Chưa đo trường hợp có `WHERE`; một `WHERE` nhắc tới cột thì tham chiếu cột đó và có lẽ bị chặn. Đây là việc follow-up.
4. **Bootstrap giờ chờ có compute node đang sống** trước khi tạo bảng (lần chạy live đầu tiên thất bại với "Number of available CN nodes is 0"). Bootstrap chạy query log sau các lệnh delegation, nên việc chờ này không làm chậm chúng.
