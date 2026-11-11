# Uỷ quyền StarRocks và vòng đời engine (gói D): plan triển khai

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-11-starrocks-delegation-and-lifecycle.md). Mọi giá trị chính xác, chữ ký hàm, đoạn mã và các bước nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Mục tiêu:** engine StarRocks ở chế độ impersonate dưới Ranger chạy được query của người dùng qua egret, CN không còn crash-loop khi root có mật khẩu, và xoá engine không để lại pod của Job hay metadata FE.

**Nhánh:** `fix/starrocks-delegation-and-lifecycle` trong worktree `egret-srd`, dựng trên nhánh của gói A (PR #849, chưa merge). Commit, không push. Docs ở `egret-data/docs` sau khi PR platform merge.

**Ràng buộc chung (chi tiết ở bản tiếng Anh):** `CARGO_TARGET_DIR=$PWD/target-srd`; test DB chạy với `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; không chạy hai bộ test cùng lúc; chỉ dùng image fork; client frontend chỉ sinh lại bằng `npm run gen:api`; SQL qua `quote_string`, lỗi FE qua `FeError::scrubbed`; mỗi thay đổi hành vi có mutant bị test giết; cổng golden (bless, kiểm tra ngữ nghĩa, chạy lại không bless, chạy với `EGRET_GOLDEN_MECHANISM=opa`); đối chiếu CI trước khi xong (clippy toàn workspace và các bộ test policy `egret-perm-compile --features harness`, `egret-perm-eval`, `egret-policy`, `egret-ranger-eval`, kể cả các cổng `EGRET_TEST_RANGER_E2E=1`); migration 0219.

## Đã quyết ngày 2026-11-11

1. **Đồng bộ cả platform admin** trên mọi engine impersonate được gán ít nhất một workspace; trigger phủ grant/revoke role platform (`access_grants`) và bảng `roles`. Task 7, 8, 13.
2. **Sửa access control gốc ngay trong gói D:** đồng bộ cấp `IMPERSONATE ON USER` theo từng user egret quản lý và thu hồi `ON ALL USERS`; không bao giờ cấp cho user dành riêng; bootstrap thôi cấp `ON ALL USERS`; engine có sẵn hội tụ ở lần apply hoặc quét kế tiếp. Task 1 (M6, M7), 10, 12, 13.
3. **User không do egret tạo được để yên**, ghi log, và có tài liệu cách bàn giao. Task 8, 10, 14.
4. **`POST /engines` (StarRocks) trả 409 `engine_metadata_left_over`** khi `fe-meta-<name>-fe-0` còn, thông báo kèm lệnh `kubectl`; ghi chú nâng cấp vẫn giữ. Task 6, 13, 14.

## Các task

- **Task 1 (controller).** Đo M1 đến M3 (e2e của fork: `isExcludes` trên `user`; `SHOW USERS`; lỗi `CREATE USER`/`DROP USER`), và M6, M7 trên FE access control gốc (`SHOW GRANTS FOR` của delegator; thu hồi `ON ALL USERS` khi đã có grant theo user; thu hồi khi chưa cấp; cấp lặp lại).
- **Task 2.** `STARROCKS_RESERVED_USERS`, `delegator_impersonates` mở rộng, `InfraObject::AnyUserExcept`, `lower_infra`, body `starrocks_delegator_impersonate_policy`, nhánh render, nhánh `decide` của OPA.
- **Task 3.** Test convergence, bless lại golden, kiểm tra ngữ nghĩa bằng script, chạy lại không bless và với OPA.
- **Task 4.** `MYSQL_PWD` cho FE và CN (D2).
- **Task 5.** Xoá engine dọn pod của Job và PVC meta của FE (D3).
- **Task 6.** `POST /engines` (StarRocks) trả 409 `engine_metadata_left_over` khi PVC cũ còn, kèm lệnh `kubectl`; đọc PVC lỗi thì trả 502; có test và mutant; sinh lại `openapi.json` và client bằng `npm run gen:api`.
- **Task 7.** Migration 0219: bảng `engine_mgmt.starrocks_users`, giữ chỗ `root`, trigger `pg_notify` trên `access_grants`, `user_groups`, `users`, `engine_mgmt.engine_workspaces` và `roles`; các hàm store và khoá advisory; test trigger (kể cả grant `platform_admin` và ghi vào `roles`).
- **Task 8.** Module `starrocks_users.rs`: tập mong muốn gồm thành viên và platform admin (khi engine phục vụ ít nhất một workspace), kế hoạch thuần (có `kept`), `sync_starrocks_users`, fake FE quản lý user; test DB chỉ khẳng định về user của chính test vì platform admin của DB test cũng được đồng bộ.
- **Task 9.** Nối dây: checkpoint 7 trong bootstrap, mỗi lần apply StarRocks, listener và vòng quét 10 phút, khởi chạy trong `egret-bin`.
- **Task 10.** Access control gốc: bootstrap thôi cấp `ON ALL USERS`; đồng bộ đọc `SHOW GRANTS FOR` một lần (theo header), cấp `IMPERSONATE ON USER` cho user egret quản lý trước, rồi thu hồi `ON ALL USERS` khi mọi lệnh cấp thành công (đổi thứ tự ở fix round 1, 2026-11-12); thu hồi grant theo user trước mỗi `DROP USER`, kể cả trên engine gắn profile, vì `DROP USER` không xoá grant; engine gắn profile không nhận lệnh cấp hay thu hồi `ON ALL USERS`; grant mồ côi là known issue (quyết 2026-11-12); sửa các test bootstrap cũ; có test và mutant.
- **Task 11.** Đối chiếu CI: fmt, clippy toàn workspace, các crate đã sửa, các bộ test policy và các cổng Ranger e2e.
- **Task 12 (controller).** Test với StarRocks thật: đồng bộ, user lạ, drop, và grant native (thu hồi `ON ALL USERS`, delegator `EXECUTE AS` thành viên được, `root` và user lạ bị từ chối); chạy lại e2e của fork với body egret render ra.
- **Task 13 (controller).** Kiểm tra live trên kind `egret`: bốn chứng minh (query với tư cách `member-sso` và user tự tạo; CN khoẻ không cần vá tay; xoá engine không để lại pod của Job hay PVC; tạo lại cùng tên có root mới), access control gốc trên engine không gắn profile, và 409 khi PVC cũ còn.
- **Task 14.** Docs trong `egret-data/docs`: Engines and Ranger (kể cả platform admin, grant native, bàn giao user lạ), Databases, Engine detail, REST API và Provision (409), Upgrade notes, Known issues.
