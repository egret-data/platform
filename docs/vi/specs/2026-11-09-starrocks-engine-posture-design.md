# Tình trạng bảo mật của engine StarRocks: engine không gắn profile và engine cũ (thiết kế)

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/specs/2026-11-09-starrocks-engine-posture-design.md). Mọi giá trị chính xác nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Trạng thái:** người dùng duyệt ngày 2026-11-09, theo hai phần: KI-336, rồi engine cũ.

**Gói:** đây là gói B trong các follow-up StarRocks sau SP2. Thứ tự là B, rồi A (audit phía platform và tab Query log), rồi C (probe các đường chưa đo). A và C có spec riêng.

## Vấn đề

1. **Engine không gắn profile (KI-336).**
   - Engine StarRocks provision mà không có profile thì cứ không có profile, kể cả khi Ranger bật. Lý do: egret chỉ tự tạo profile cho Trino, và **Add engine** không có bộ chọn profile cho StarRocks.
   - Hệ quả: engine không áp grant, mask hay row filter nào của egret, và không có gì báo điều đó.
2. **Engine cũ.** Engine tạo trước #826 và #832 có thể còn một trong hai vấn đề:
   - **Chạy image FE chính thức, hoặc image fork trước `4.1.7-egret.4`.** Các image này còn những lỗ mà fork đã vá: KI-340, KI-345, HTTP SQL API, Arrow Flight SQL, FILES(), materialized view, EXPLAIN COSTS, BROKER LOAD.
   - **`root` không có mật khẩu.** Engine cũ không có `root_secret_id`, nên ai vào được cổng 9030 cũng đăng nhập được bằng `root`.

   Hiện nay lỗ do image chỉ hiện thành cảnh báo trên trang của từng engine. Không có danh sách các engine bị ảnh hưởng, và không có cách sửa nào ngoài sửa `feImage` bằng tay. Việc `root` không có mật khẩu thì không hiện ở đâu cả.

## Phần 1: KI-336, StarRocks có profile giống Trino

**Tạo engine.**
- `plan_trino_profile` đổi thành `plan_profile(st, name, engine_type, requested)` và được cả hai đường tạo engine gọi. Nó trả về một trong ba kế hoạch:
  - `Existing`: request đã chọn một profile có sẵn, thì gắn vào profile đó;
  - `Unbound`: Ranger tắt và không chọn profile, giữ như hiện nay;
  - `Create`: Ranger bật và chưa có profile tên `<name>`.
- Với StarRocks, `Create` tạo profile `<name>` kiểu `starrocks`, không có tag service, vì migration 0176 cấm `tagService` trên profile không phải Trino.
- Hai trường hợp bị từ chối bằng 409: có profile trùng tên nhưng khác kiểu, hoặc profile trùng tên đã gắn với engine khác. Câu thông báo nêu đúng kiểu cần có.
- Tạo engine thất bại thì hoàn tác qua `CreateUndo`, và chỉ xoá profile do chính lần tạo này sinh ra.

**Form Add engine:** nhánh StarRocks có bộ chọn profile giống Trino.
- Danh sách chỉ hiện profile kiểu `starrocks` chưa gắn với engine nào.
- Lựa chọn mặc định là tạo profile mới mang tên engine.

**Engine không gắn profile:** khi Ranger bật, engine hiện cảnh báo "Not governed" và có badge trong danh sách engine. Cách sửa là dùng `PUT …/engines/{id}` với `authzServiceId`.

## Phần 2: engine cũ, badge và nút nâng cấp

**Điều kiện, chỉ áp cho engine do egret provision.** DTO của engine có thêm `securityUpgradeNeeded` và `securityUpgradeReasons`, với hai lý do:
- **`unpatchedImage`:** image FE chưa vá đủ.
  - Quy tắc nhận diện chuyển vào `egret-perm::engine_images`.
  - Image tính là đã vá nếu khớp digest của một image fork đã biết (kể cả khi được mirror sang registry khác), hoặc tag là `4.1.7-egret.4` trở lên, hoặc một bản phát hành mới hơn.
- **`noRootPassword`:** engine không có `root_secret_id`.

**Giao diện:**
- Danh sách engine có badge "Security upgrade needed".
- Trang engine có cảnh báo nêu từng lý do, kèm nút **Upgrade security** (quyền `engine:update`).
- Khi cảnh báo này hiện thì cảnh báo cũ không hiện, để trang chỉ có một thông điệp.

**Hành động nâng cấp:** `POST /api/control-plane/engines/{id}/security-upgrade`. Engine ngoài hoặc không phải StarRocks bị trả 409.
- **Hộp xác nhận** nêu image FE và CN đổi từ đâu sang đâu, và rằng mật khẩu `root` sẽ được đặt. Hộp cảnh báo:
  - FE và CN sẽ restart;
  - client ngoài egret đang dùng `root` không mật khẩu sẽ bị từ chối.
- **Các bước,** bước nào cũng idempotent:
  1. tạo mật khẩu `root`;
  2. đổi image FE và CN sang mặc định, nếu không phải hạ cấp;
  3. apply engine. Job `{name}-initpwd` đặt mật khẩu trên FE đang chạy.
- **Chặn hạ cấp:** nếu bản phát hành của FE mới hơn bản fork mặc định thì không đổi image, vì metadata của FE không hạ phiên bản được. Mật khẩu `root` vẫn được đặt, và lý do `unpatchedImage` vẫn còn.
- **Không có gì tự động:** không có migration tự đổi image, vì FE sẽ restart.

## Kiểm thử

Mỗi thay đổi hành vi có một test, kèm một mutant bị test đó bắt. Gồm:
- **Unit:**
  - quy tắc nhận diện image: theo tag, theo digest, image không biết;
  - chặn hạ cấp;
  - thứ tự các lý do;
  - `plan_profile`.
- **Test DB:**
  - tạo engine StarRocks: Ranger bật, Ranger tắt, profile chọn sẵn, profile trùng tên trả 409, hoàn tác;
  - nâng cấp: đủ bước, gọi lần hai không đổi gì, chặn hạ cấp, engine ngoài trả 409, thiếu quyền trả 403.
- **Test render:** job `initpwd` xuất hiện sau khi nâng cấp.
- **Vitest:** bộ chọn profile, cảnh báo "Not governed", badge, hộp thoại, cảnh báo cũ bị ẩn.
- **Kiểm tra trực tiếp trên kind:** một engine 4.1.6 không mật khẩu. Sau khi nâng cấp:
  - `root` không mật khẩu bị từ chối;
  - query của egret vẫn chạy;
  - badge biến mất.

## Docs và phần ngoài phạm vi

- **Docs:**
  - trang Engine detail và Add engine;
  - ghi chú upgrade;
  - known issues: KI-336 đã sửa, KI-340 và KI-345 trỏ tới nút nâng cấp;
  - REST API;
  - screenshot.
- **Ngoài phạm vi:**
  - engine StarRocks ngoài;
  - gói A và gói C;
  - tag policy native cho StarRocks.
