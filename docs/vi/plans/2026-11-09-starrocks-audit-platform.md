# Audit StarRocks phía platform (gói A): plan triển khai

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-09-starrocks-audit-platform.md). Mọi giá trị chính xác, chữ ký hàm và các bước nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Mục tiêu:** engine StarRocks do egret provision trên image của egret ghi mọi statement vào bảng audit, giữ 30 ngày. egret hiển thị bảng này ở tab **Query log**, và query uỷ quyền hiện tên người thật.

**Nhánh:** `feat/starrocks-query-log`, tạo từ `main` sau khi gói B đã merge.

**Những chỗ sửa lại điểm xuất phát (mục 0):**
1. Crate provisioner chỉ render object Kubernetes, nên setup nằm trong `egret-engine-mgmt`.
2. Bootstrap không chạy sau mỗi lần apply, nên cần thêm `spawn_query_log_setup`.
3. `build_default_policies` là hàm private, nên guard cần một builder public mới.
4. Engine không gắn profile phải `GRANT INSERT` theo cơ chế native.
5. `lower_infra` dừng sớm khi không có engine nào uỷ quyền, nên query log cần một cờ riêng.
6. B đã dùng migration 0217, nên A dùng 0218.

## Các task

- **Task 1.** Thêm `QUERY_LOG_LOADER_PRINCIPAL`; migration 0218 (cột `query_log_secret_id`, và giữ chỗ tên trong `username_ledger`); thêm `set_query_log_secret` và `ensure_query_log_secret`. Task này cũng thêm ghi chú sửa spec.
- **Task 2.** Ranger: thêm `InfraKind::QueryLogLoader` và `QueryLogGuard`, cờ `InfraInputs.query_log`, các body, tên trong `infra_names`, và nhánh trong `render`. Có test DB cho convergence.
- **Task 3.** Viết `ensure_query_log` và gọi nó ở ba chỗ: bootstrap, sau apply hoặc nâng cấp của B, và route. Có `plugin_step`, tách `wait_for_fe_root_login` ra dùng chung, và thêm `plugin_enable`/`plugin_dir` vào danh sách key bị cấm. Pass lúc boot tuỳ câu hỏi mở 4.
- **Task 4.** Bốn route (`status`, danh sách, chi tiết, `setup`), SQL đọc, cursor, `authorized_name`, `redact_statement`, `statement_visible`, và `engine:audit` tuỳ câu hỏi mở 1.
- **Task 5 (controller).** Test với StarRocks thật trên image egret.4 và image chính thức, đo bốn điểm chưa đo, và chạy e2e của fork cho guard.
- **Task 6.** Frontend: tab Query log, bốn trạng thái, bộ lọc trong URL, bảng, panel chi tiết, **Load more**, và vitest.
- **Task 7 (controller).** Kiểm tra live trên kind: event có xuất hiện, uỷ quyền, bị từ chối, admin bị chặn, plugin còn sau khi pod FE khởi động lại, nâng cấp engine cũ, và chụp màn hình.
- **Task 8.** Docs trong `egret-data/docs`.