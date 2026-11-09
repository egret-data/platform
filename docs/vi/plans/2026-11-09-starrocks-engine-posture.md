# Tình trạng bảo mật của engine StarRocks (gói B): plan triển khai

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-09-starrocks-engine-posture.md). Mọi giá trị chính xác, chữ ký hàm và các bước nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Mục tiêu:**
- engine StarRocks có profile giống Trino, và engine không có profile thì hiện cảnh báo;
- engine cũ (image chưa vá, hoặc `root` không mật khẩu) được đánh dấu, và admin sửa được bằng một nút.

**Ba chỗ sửa lại spec (mục 0):**
1. Form Add engine không có bộ chọn profile cho Trino, nên bộ chọn cho StarRocks là thành phần mới.
2. Frontend không biết Ranger đang bật hay tắt, nên backend tính `notGoverned`.
3. Hộp thoại cần biết image đích trước khi gọi route. DTO vì thế có thêm `securityUpgradeFeImage` và `securityUpgradeBlockedRelease`.

## Các task

- **Task 1.** Chuyển quy tắc nhận diện image đã vá vào `egret-perm`:
  - `is_patched_starrocks_fe`: nhận diện theo tag, và theo digest cho image được mirror;
  - `starrocks_fe_upgrade_target`: chặn hạ cấp.

  Có unit test và mutant. Task này cũng sửa spec theo mục 0.
- **Task 2.** KI-336 phía backend:
  - `PolicyPort::ensure_starrocks_def`;
  - `plan_profile` và `bind_profile` dùng chung cho Trino và StarRocks; StarRocks tạo profile mà không có tag service;
  - DTO có thêm `notGoverned`.

  Có test DB cho các trường hợp tạo engine và cho việc hoàn tác.
- **Task 3.** DTO có thêm `securityUpgradeNeeded`, `securityUpgradeReasons` (`unpatchedImage`, rồi `noRootPassword`), `securityUpgradeFeImage` và `securityUpgradeBlockedRelease`. Có unit test.
- **Task 4.** Route `POST /api/control-plane/engines/{id}/security-upgrade` (quyền `engine:update`):
  1. tạo mật khẩu `root`;
  2. đổi image FE và CN, trừ khi bị chặn hạ cấp;
  3. apply engine, qua đó job `initpwd` đặt mật khẩu.

  Có test DB: đủ bước, gọi lần hai không đổi gì, chặn hạ cấp, 409 và 403. Có thêm test render.
- **Task 5.** Frontend:
  - bộ chọn "Policy profile" cho StarRocks;
  - cảnh báo và badge "Not governed";
  - badge "Security upgrade needed";
  - cảnh báo nâng cấp và hộp thoại;
  - ẩn cảnh báo cũ, bỏ `isPatchedStarRocksImage`.

  Có vitest.
- **Task 6.** Kiểm tra trực tiếp trên kind, do controller làm: engine 4.1.6 không mật khẩu, nâng cấp xong thì `root` không mật khẩu bị từ chối và egret vẫn chạy query được.
- **Task 7.** Docs: Engine detail, Add engine, ghi chú upgrade, KI-336/340/345, REST API và screenshot.

## Ràng buộc chung

Xem bản tiếng Anh. Tóm tắt:
- **build:** dùng target riêng `target-srb`;
- **test:** không chạy hai bộ test cùng lúc; mỗi thay đổi hành vi có một test kèm một mutant;
- **Docker và stack:** không prune Docker; không đụng container không phải của mình;
- **frontend:** chỉ tạo lại client bằng `npm run gen:api`;
- **quyền:** route dùng quyền `engine:update`.
