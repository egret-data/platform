# Uỷ quyền StarRocks và vòng đời engine (gói D): plan triển khai

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-11-starrocks-delegation-and-lifecycle.md). Mọi giá trị chính xác, chữ ký hàm, đoạn mã và các bước nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Mục tiêu:** engine StarRocks ở chế độ impersonate dưới Ranger chạy được query của người dùng qua egret, CN không còn crash-loop khi root có mật khẩu, và xoá engine không để lại pod của Job hay metadata FE.

**Nhánh:** `fix/starrocks-delegation-and-lifecycle` trong worktree `egret-srd`, dựng trên nhánh của gói A (PR #849, chưa merge). Commit, không push. Docs ở `egret-data/docs` sau khi PR platform merge.

**Ràng buộc chung (chi tiết ở bản tiếng Anh):** `CARGO_TARGET_DIR=$PWD/target-srd`; test DB chạy với `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; không chạy hai bộ test cùng lúc; chỉ dùng image fork; client frontend chỉ sinh lại bằng `npm run gen:api`; SQL qua `quote_string`, lỗi FE qua `FeError::scrubbed`; mỗi thay đổi hành vi có mutant bị test giết; cổng golden (bless, kiểm tra ngữ nghĩa, chạy lại không bless, chạy với `EGRET_GOLDEN_MECHANISM=opa`); đối chiếu CI trước khi xong (clippy toàn workspace và các bộ test policy `egret-perm-compile --features harness`, `egret-perm-eval`, `egret-policy`, `egret-ranger-eval`, kể cả các cổng `EGRET_TEST_RANGER_E2E=1`); migration 0219.

## Các task

- **Task 1 (controller).** Đo M1 đến M3 trước khi code dựa vào: plugin của fork có tôn trọng `isExcludes` trên resource `user` không (chạy e2e của fork với dòng viết tay); dạng trả về của `SHOW USERS`; text lỗi của `CREATE USER` khi user đã có và `DROP USER` khi user không có.
- **Task 2.** `STARROCKS_RESERVED_USERS` và `is_starrocks_reserved_user`; `delegator_impersonates` mở rộng cho StarRocks impersonate; `InfraObject::AnyUserExcept`; `lower_infra` sinh dòng cho StarRocks; body `starrocks_delegator_impersonate_policy`; nhánh render theo cặp (object, engine), từ chối danh sách rỗng; nhánh tương ứng trong `decide` của OPA.
- **Task 3.** Test convergence (thêm dòng, chuyển sang jwt thì gỡ dòng), sửa test StarRocks đang đếm thiếu, bless lại golden, kiểm tra ngữ nghĩa bằng script (chỉ thêm đúng dòng mới), chạy lại không bless và với OPA.
- **Task 4.** `MYSQL_PWD` dạng `secretKeyRef` vào `feEnvVars` và `cnEnvVars`, sau `apply_pod_fields`, không ghi đè giá trị admin đã đặt.
- **Task 5.** Xoá Job initpwd bằng `delete_background`; `persistentVolumeClaimRetentionPolicy: {whenDeleted: Delete}` khi fe-meta bền vững; dò và xoá `fe-meta-<name>-fe-<i>` đến ordinal đầu tiên không tồn tại (tối đa 16); gom mọi lỗi.
- **Task 6.** Migration 0219: bảng `engine_mgmt.starrocks_users`, giữ chỗ `root` trong `username_ledger`, trigger `pg_notify` trên `access_grants`, `user_groups`, `users` và `engine_mgmt.engine_workspaces`. Thêm các hàm store và khoá advisory theo engine. Có test trigger (thông báo lúc commit, không thông báo khi rollback).
- **Task 7.** Module `starrocks_users.rs`: kiểm tra tên theo `FeNameFormat`, SQL `CREATE USER`/`DROP USER`, đọc `SHOW USERS`, hàm lập kế hoạch thuần, và `sync_starrocks_users`. Fake FE biết quản lý user. Có test DB: tạo, drop, user lạ được để yên, user bị vô hiệu hoá và grant hết hạn không được tạo, thành viên qua group được tạo, tạo lỗi thì xoá bản ghi của chính nó và mật khẩu bị che, `NotNeeded`, `Busy`.
- **Task 8.** Nối dây: checkpoint 7 trong bootstrap (trước query log), mỗi lần apply StarRocks (`finish_apply`), listener `LISTEN` kèm vòng quét 10 phút (lần đầu là đối soát lúc khởi động), khởi chạy trong `egret-bin`.
- **Task 9.** Đối chiếu CI: fmt, clippy toàn workspace, các crate đã sửa, các bộ test policy và các cổng Ranger e2e.
- **Task 10 (controller).** Test với StarRocks thật (allin1 egret.4): đồng bộ hai lần, user lạ, drop; chạy lại e2e của fork với body egret render ra.
- **Task 11 (controller).** Kiểm tra live trên cluster kind `egret`, chứng minh bốn điều: query qua egret với tư cách `member-sso` và user được tự tạo; CN khoẻ mà không cần vá tay; xoá engine không để lại pod của Job hay PVC; tạo lại cùng tên có root mới.
- **Task 12.** Docs trong `egret-data/docs`: Engines and Ranger, Databases, Engine detail (xoá engine), Upgrade notes, Known issues (KI-335 đã sửa, thêm hai dòng mới).

Các câu hỏi mở của spec chưa được xây.
