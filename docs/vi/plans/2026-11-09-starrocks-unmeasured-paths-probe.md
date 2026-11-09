# Thăm dò các đường StarRocks chưa đo (gói C): plan triển khai

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-09-starrocks-unmeasured-paths-probe.md). Mọi giá trị chính xác, chữ ký hàm, lệnh và các bước nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Mục tiêu:** đo bốn đường chưa ai đo, trên image chính thức `starrocks/allin1-ubuntu:4.1.7` và trên image egret `4.1.7-egret.4`. Bốn đường là:
- UDF;
- catalog Hive, JDBC và Paimon;
- stream load và routine load;
- cổng CN/BE nhìn từ trong namespace.

Mỗi kết quả được xếp loại, và mỗi lỗ hổng có một mã việc tiếp theo. Gói này không viết patch và không viết code sản phẩm.

**Ràng buộc chính:**
- `run.sh` và `checks.py` giữ nguyên: mọi check, tên, tag và giá trị mong đợi.
- Chỉ chạy container `egs-probe-*`, và không bao giờ chạy `probe.sh` cùng lúc với `run.sh`, vì cả hai dùng cổng 18777.
- Check phá hủy chỉ chạy trong container allin1 dùng một lần.
- Chưa push gì cho tới khi người dùng trả lời câu hỏi công bố.

## Các task

- **Task 1.** Harness:
  - `stack.sh` và `sqlpaths.py`, đã tách ra mà output của `run.sh` không đổi;
  - `probe.sh`, `probe_lib.py` (`Verdict`, `Probe`, `PROBE_SKIP`) và `canary.py`;
  - fake Ranger đọc lại policy;
  - `policies.py --probe` và `--version 2 --revoke loader`;
  - ghim tag image.

  Có unit test và mutant.
- **Task 2.** Probe UDF:
  - jar Java gồm `Echo`, `ReadFile` và `Fetch`;
  - các check U0–U10;
  - P0–P4 cho Python;
  - biến thể `UDF_JAVA_POLICY=1`.
- **Task 3.** Catalog JDBC (Postgres) và Hive (Metastore), các check C1–C16.
- **Task 4.** Catalog Paimon. Dữ liệu nạp bằng `pypaimon`; nếu không được thì dừng lại và theo câu hỏi mở 4. Check C14 đọc `orders$files`.
- **Task 5.** Stream load (S1–S11) và routine load trên Kafka (R1–R8), gồm cả bước thu hồi quyền ở policy phiên bản 2.
- **Task 6.** Probe trong mạng trên allin1:
  - các check N1–N11;
  - thrift của FE được mã hóa tay;
  - biến thể `BE_HTTP_AUTH=1`.
- **Task 7.** Chạy trên kind, do controller làm, tùy chọn. Pod ở `egret-engines` là nơi đo, còn pod ở `default` là kiểm soát cho NetworkPolicy. Thử thêm `enable_http_auth` trong `cn_config`.
- **Task 8.** Ba lượt chạy đầy đủ, rồi:
  - báo cáo `docs/en/reports/2026-11-09-starrocks-unmeasured-paths-probe.md`;
  - mục "Probe" trong README của fork;
  - known issues trong `egret-data/docs`.
