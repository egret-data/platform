# Audit StarRocks phía platform: Query log (thiết kế)

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/specs/2026-11-09-starrocks-audit-platform-design.md). Mọi giá trị chính xác nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Trạng thái:** bản nháp chờ người dùng duyệt, 2026-11-09. Bốn câu hỏi mở ở cuối bản tiếng Anh do người dùng quyết.

**Gói:** A trong các follow-up StarRocks. Thứ tự là B (đang làm), A, rồi C. A dùng `egret_perm::is_patched_starrocks_fe` và nút **Upgrade security** của B.

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

   Giá trị request đều đi qua `quote_string`. Statement bị cắt ở 2.000 ký tự, và được che mật khẩu theo giá trị chính xác lẫn theo dạng. Các lần đọc của chính egret bị loại khỏi kết quả.
6. **Tab:** chỉ có ở engine StarRocks. Tab có bộ lọc trong URL, bảng ưu tiên cột User (`authorizedUser`), panel chi tiết, và nút **Load more**.
7. **Gia cố:** `plugin_enable` và `plugin_dir` bị cấm trong `feConfig`. Cần đo xem plugin còn sau khi FE khởi động lại hay không.

## Kiểm thử

- unit;
- IR và render;
- test DB;
- test với StarRocks thật (`EGRET_TEST_STARROCKS_URL`, image egret.4 và image chính thức);
- e2e của fork cho guard;
- vitest;
- kiểm tra live trên kind.

## Câu hỏi mở (chi tiết ở bản tiếng Anh)

1. Ai được đọc nội dung statement? Đề xuất: quyền mới `engine:audit`.
2. Engine admin có được truy cập trực tiếp database audit bằng SQL không? Đề xuất: không ai được, kể cả admin.
3. Có hiển thị thêm audit của Ranger không? Đề xuất: không.
4. Engine egret.4 đã có từ trước: có tự setup lúc egret khởi động không? Đề xuất: có, chạy một lần lúc boot.