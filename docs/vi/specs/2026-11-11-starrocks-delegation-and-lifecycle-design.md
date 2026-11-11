# Uỷ quyền StarRocks và vòng đời engine (gói D): thiết kế

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/specs/2026-11-11-starrocks-delegation-and-lifecycle-design.md). Mọi giá trị chính xác, đoạn mã và bằng chứng file:line nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Trạng thái:** người dùng đã quyết D1a, D1b, D2 và D3 ngày 2026-11-11. Những gì thiết kế không tự quyết được từ các quyết định đó nằm ở mục "Câu hỏi mở", mỗi câu có khuyến nghị, và chưa được xây cho tới khi người dùng trả lời.

**Gói:** D trong các follow-up StarRocks. Dựng trên nhánh của gói A (`feat/starrocks-query-log`, PR #849, migration 0218), chưa merge. D dùng migration **0219**.

## Vấn đề

1. **Engine StarRocks ở chế độ impersonate dưới Ranger không chạy được query qua egret.** Có hai lỗ hổng độc lập:
   - **1a.** Không có gì cấp quyền `impersonate` cho `egret_delegator` trong Ranger. Dưới Ranger, StarRocks kiểm tra `EXECUTE AS` với Ranger và bỏ qua lệnh `GRANT IMPERSONATE ON ALL USERS` gốc. IR của egret chỉ sinh dòng `DelegatorImpersonate` cho Trino.
   - **1b.** User đích phải tồn tại (`checkUserExist` chạy trước mọi authorizer), mà egret không tạo user StarRocks cho người dùng.
2. **CN crash-loop sau khi Job initpwd đặt mật khẩu root.** Entrypoint của CN đăng ký bằng `mysql -u root`, chỉ đọc mật khẩu từ `MYSQL_PWD`, mà CR của egret không có biến này (KI-335). Từ #847, **Upgrade security** trên engine cũ chắc chắn gây lỗi này.
3. **Xoá engine để lại pod của Job initpwd và PVC meta của FE.** Job bị xoá với propagation mặc định (orphan); PVC từ `volumeClaimTemplates` không bị xoá. Engine mới cùng tên sẽ dùng lại PVC cũ, với user, catalog và mật khẩu root cũ, nên mọi lần đăng nhập của bootstrap đều thất bại.

## Đã quyết (người dùng, 2026-11-11)

- **D1a:** egret tạo sẵn user StarRocks cho mỗi người có thể query engine impersonate, mật khẩu ngẫu nhiên không dùng được. Đồng bộ khi gán engine cho workspace, khi thành viên thay đổi, và đối soát lại ở mỗi lần apply và lúc khởi động. egret DROP user khi người đó mất quyền (rời mọi workspace mà engine phục vụ, hoặc engine bị gỡ gán). Root không bao giờ vào đường query; việc đồng bộ chạy trong engine-mgmt, nơi giữ root.
- **D1b:** với StarRocks impersonate, egret sinh dòng infra `DelegatorImpersonate` cho `egret_delegator` quyền `impersonate` trên mọi user trừ `root`, `egret_delegator` và `egret_query_log`.
- **D2:** khi engine có mật khẩu root, thêm `MYSQL_PWD` từ `<name>-initpwd-secret` vào env của cả FE và CN, sau `apply_pod_fields`, không ghi đè `MYSQL_PWD` admin đã đặt.
- **D3:** xoá Job initpwd bằng propagation Background; đặt `persistentVolumeClaimRetentionPolicy: {whenDeleted: Delete}` khi fe-meta bền vững; xoá tường minh các PVC `fe-meta-<name>-fe-<i>`; ghi tài liệu rằng metadata FE của engine đã xoá là mất, dữ liệu bảng vẫn ở object storage.

## Thiết kế, tóm tắt

1. **Dòng Ranger (D1b).**
   - Một danh sách user StarRocks dành riêng duy nhất, `STARROCKS_RESERVED_USERS = [egret_delegator, egret_query_log, root]` trong `egret_core::engine_principals`, dùng cho cả dòng Ranger lẫn việc đồng bộ.
   - `delegator_impersonates` mở rộng cho StarRocks ở chế độ impersonate.
   - IR có object mới `InfraObject::AnyUserExcept(Vec<String>)`. Body StarRocks dùng resource `user` với `isExcludes: true`, cùng tên `egret-delegator-impersonate`. Trino giữ nguyên từng byte.
   - Chọn `isExcludes` thay vì dòng deny: service def cho phép (`excludesSupported: true`, `ignoreCase: true`), plugin đánh giá cờ này, và chỉ cần một dòng. Hành vi trên plugin thật của fork (Ranger 2.9.0) được đo ở Task 1 trước khi code dựa vào.
   - Evaluator OPA (`decide`) có nhánh tương ứng.
2. **User StarRocks cho người dùng (D1a).**
   - Tập mong muốn: user đang active có grant workspace `allow` còn hiệu lực (trực tiếp hoặc qua group) trên workspace được gán engine; rỗng nếu engine không ở chế độ impersonate. Tên là `users.username` nguyên văn, đúng tên mà `EXECUTE AS` gửi. User bị vô hiệu hoá thì bị drop.
   - Bảng mới `engine_mgmt.starrocks_users` ghi những user egret đã tạo. egret chỉ drop user mình đã ghi, không bao giờ đụng user người khác tạo (user "lạ" được để yên và ghi cảnh báo).
   - Một hàm đối soát idempotent, có khoá advisory theo engine, các bước: đọc, lập kế hoạch (hàm thuần), thực thi. `CREATE USER` không có `IF NOT EXISTS`: ghi bản ghi trước, nếu tạo lỗi thì xoá bản ghi chính lần này đã ghi. Mọi literal qua `quote_string`, mọi lỗi FE qua `FeError::scrubbed`.
   - **Điểm kích hoạt:** migration 0219 thêm trigger cấp statement trên `access_grants`, `user_groups`, `users` (status, username, delete) và `engine_mgmt.engine_workspaces`, gửi `pg_notify` lúc commit. Một listener trong engine-mgmt nghe kênh đó, kèm vòng quét 10 phút (lần đầu chạy ngay, tức là đối soát lúc khởi động). Ngoài ra còn đồng bộ sau mỗi lần apply StarRocks, và trong bootstrap sau các lệnh delegation (checkpoint thứ tám).
   - **Sổ username:** dựa vào bảo đảm "tên không bao giờ được cấp lại" của nó. Migration 0219 giữ chỗ thêm `root`.
3. **`MYSQL_PWD` (D2):** thêm vào `feEnvVars` và `cnEnvVars` dạng `secretKeyRef`, trừ khi admin đã đặt. FE và CN roll một lần ở lần apply kế tiếp.
4. **Xoá engine (D3):** Job xoá bằng `delete_background`; chính sách giữ PVC trên FE khi fe-meta bền vững; dò và xoá `fe-meta-<name>-fe-0`, `-1`, … đến ordinal đầu tiên không tồn tại (tối đa 16).

## Chưa đo (Task 1 của plan đo trước)

- M1: plugin của fork từ chối `EXECUTE AS` tới `root`, `ROOT`, `egret_query_log`, `egret_delegator` và cho phép user khác.
- M2: dạng trả về của `SHOW USERS`.
- M3: lỗi của `CREATE USER` khi user đã có, và của `DROP USER` khi user không có.
- M4 (kind): CN có `MYSQL_PWD` trước khi Job initpwd chạy sẽ khởi động lại rồi tham gia được.
- M5 (kind): sau khi xoá, PVC mất, và engine tạo lại cùng tên có FE mới với mật khẩu root của egret.

## Câu hỏi mở (có khuyến nghị)

1. **Platform admin không phải thành viên** vẫn qua được cổng workspace nhưng không được đồng bộ. Khuyến nghị: thêm người giữ `platform_admin` vào tập mong muốn của mọi engine impersonate đã được gán ít nhất một workspace.
2. **Access control gốc vẫn cho delegator thành root** (engine không gắn profile, `GRANT IMPERSONATE ON ALL USERS`). Khuyến nghị: follow-up cấp `GRANT IMPERSONATE ON USER` theo từng user do việc đồng bộ tạo, rồi thu hồi `ON ALL USERS`.
3. **User cùng tên không do egret tạo** được để yên. Khuyến nghị: giữ như vậy và ghi tài liệu cách bàn giao (root drop user đó, lần đồng bộ sau sẽ tạo user của egret).
4. **Tạo lại cùng tên khi PVC cũ còn.** Khuyến nghị: `POST /engines` trả 409 khi PVC `fe-meta-<name>-fe-0` còn tồn tại.
