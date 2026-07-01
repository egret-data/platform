# Redesign REST Catalog — Plan 2: vended STS + xoá signer (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-rest-catalog-redesign-p2-vended-sts.md` (code/step đầy đủ ở đó).
> Spec: `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md` (Workstream **D**). Plan 2/6. Thực thi qua subagent-driven-development.

**Mục tiêu:** catalog **vend STS creds ngắn hạn, scoped theo bảng** cho warehouse `notebook` (kernel IO S3 trực tiếp mà không giữ creds gốc); **xoá remote-signing** egret đang proxy.

## Bối cảnh
- Tạo warehouse: `provision_ws.rs::create_warehouse_on` → `s3_profile_at(cfg, prefix)` (~L204) hiện `.sts_enabled(false)`.
- lakekeeper `S3Profile`: `sts_enabled`, `sts_role_arn` (bắt buộc khi STS on), `sts_endpoint: Option<Url>`, `remote_signing_enabled`. STS on + role → vend creds tạm trong `config`.
- `front_auth`: `warehouse_prefix` có nhánh `"signer"`; `required_permission` có case `/aws/s3/sign`.
- Kernel hiện kế thừa `AWS_*` của serve → pyiceberg dùng creds gốc.

## Task
- **T1 (STS on storage profile):** thêm `sts_role_arn: Option<String>` + `sts_endpoint: Option<String>` vào `StorageConfig` (opt-in); trong `s3_profile_at`, nếu có role → `.sts_enabled(true).sts_role_arn(..).sts_endpoint(sts||endpoint).remote_signing_enabled(false)`; không có → giữ `sts_enabled(false)`. 2 unit test (có role → STS on + remote-signing off + endpoint fallback; không role → off).
- **T2 (xoá signer):** bỏ nhánh `"signer"` trong `warehouse_prefix` (chỉ `config`→None, còn lại first-segment); bỏ case `/aws/s3/sign` trong `required_permission`; sửa 2 test tương ứng.
- **T3 (kernel dùng vended):** lọc bỏ `AWS_*` khỏi env kernel subprocess (`egret-kernel`, `rg` để định vị chỗ dựng env); verify `catalog.py` không inject `s3.*` từ env (pyiceberg tự đọc vended từ `config`).
- **T4 (MinIO STS + live):** cấu hình MinIO AssumeRole cho key `egret` + `EGRET_STORAGE__STS_ROLE_ARN`; boot + roundtrip `write_iceberg`/`iceberg`; assert có `s3.session-token` trong config, không request `/aws/s3/sign`, kernel không có `AWS_*`. Document vào README e2e.
- **T5 (verify):** fmt + clippy + unit + integration (`front_auth`, `gc_e2e`).

## Ghi chú
- STS opt-in theo `sts_role_arn`; store không hỗ trợ STS → cần signer (fallback, không làm ở plan này — xem spec risks).
- Cần định vị lúc làm: định nghĩa + chỗ dựng `StorageConfig`; chỗ dựng env kernel.
- Kế tiếp: Plan 3 (E — warehouse purpose).
