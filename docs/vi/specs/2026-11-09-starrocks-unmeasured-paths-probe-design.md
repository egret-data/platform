# Các đường StarRocks chưa đo: một gói thăm dò (thiết kế)

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/specs/2026-11-09-starrocks-unmeasured-paths-probe-design.md). Mọi giá trị chính xác, bảng check và các câu hỏi mở nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Trạng thái:** bản nháp chờ người dùng duyệt, 2026-11-09. Phạm vi do người dùng quyết định.

**Gói:** C của các việc tiếp theo cho StarRocks, làm sau B và A.

## Mục tiêu

Đo xem StarRocks 4.1.7 có thực thi mô hình bảo mật của egret trên bốn đường chưa ai đo hay không:
1. UDF Java và Python;
2. catalog Hive, JDBC và Paimon;
3. stream load và routine load (Kafka);
4. các cổng CN/BE, nhìn từ một pod trong namespace của engine.

Gói này **chỉ đo**. Sản phẩm của nó gồm:
- các check trong e2e của fork;
- một báo cáo, `docs/en/reports/2026-11-09-starrocks-unmeasured-paths-probe.md`;
- một mục việc tiếp theo cho mỗi lỗ hổng đã xác nhận. Mỗi mục có spec riêng.

Gói này không viết patch và không viết code sản phẩm.

Mỗi kết quả thuộc một trong ba loại:
- **enforced**: đường đó từ chối đúng những gì policy từ chối;
- **leak**: ứng viên cho patch fork `0010` trở đi, hoặc cho một biện pháp phía egret;
- **ngoài mô hình của egret**: ghi thành known issue.

## Ý chính

- **Giả thuyết H1–H8.** Đây là giả thuyết đọc từ mã nguồn 4.1.7, chưa phải kết quả:
  - UDF chạy với danh tính của node, vì không có SecurityManager;
  - `enable_http_auth` mặc định tắt;
  - `enable_token_check` sửa được qua HTTP;
  - thrift của FE tin danh tính do client gửi;
  - bảng hệ thống Paimon nằm ngoài tầm của `0001`;
  - stream load hỏi FE khi bắt đầu giao dịch.
- **Mô hình kẻ tấn công.**
  - M1: user không có quyền gì (`eve`);
  - M2: user bị mask và lọc dòng (`bob`, `mallory`);
  - M3: user có một quyền cao hẹp (`udfdev`, `loader`);
  - M4: đường ủy quyền (`svc` với `EXECUTE AS`);
  - M5: pod trong namespace engine, không có thông tin đăng nhập.
- **Mô hình kết quả.** Check thăm dò in ra một kết quả thay vì PASS/FAIL: `ENFORCED`, `LEAK`, `BLIND`, `N/A`, `SKIP` hoặc `ERROR`.
  - Mỗi check có một **kiểm soát tầm nhìn**. Nếu kiểm soát này hỏng, check là `BLIND`, và lần chạy thất bại.
  - Mỗi check chạy trên image chính thức và image egret, để có một cặp kết quả.
- **Harness.** Hình dạng giữ như `run.sh`:
  - `stack.sh` và `sqlpaths.py` là phần dùng chung;
  - `probe.sh` dùng tên và cổng riêng;
  - `probe_lib.py` chứa enum `Verdict` và enum `Probe`;
  - chỉ được bỏ qua một probe bằng `PROBE_SKIP` có kiểu; tên lạ thì dừng với exit 2;
  - `canary.py` đo lưu lượng đi ra;
  - fake Ranger đọc lại file policy ở mỗi request, để thu hồi được quyền giữa lúc chạy;
  - các dịch vụ tùy chọn là Postgres, Hive Metastore và Kafka.
- **Biến thể giảm thiểu.**
  - `BE_HTTP_AUTH=1`: bật `enable_http_auth` trong `be.conf`;
  - `UDF_JAVA_POLICY=1`: thêm một Java security policy không cấp quyền gì.
- **Bốn probe** có bảng check U, P, C, S, R và N, với kiểm soát tầm nhìn cho từng check. Xem bản tiếng Anh.

## Câu hỏi mở (chi tiết ở bản tiếng Anh)

1. **Công bố.** Fork là repo công khai. Đề xuất: chỉ push harness và các check enforced hoặc ngoài mô hình. Check của lỗ hổng đã xác nhận giữ ở nhánh local cho tới khi báo riêng cho StarRocks và có patch.
2. **Kafka, Hive và Postgres trong CI của fork?** Đề xuất: chỉ chạy local.
3. **UDF Python khi image không có Python.** Đề xuất: đo việc tạo hàm, còn việc chạy ghi là `N/A` kèm lý do đã đo.
4. **Nạp dữ liệu Paimon khi `pypaimon` không ghi được.** Đề xuất: ghi thành ranh giới đã chọn, không thêm Spark hay Flink.
5. **Chạy probe 4 trên kind.** Đề xuất: có chạy, nhưng không chạy N11 trên kind.
6. **Chạy biến thể giảm thiểu ngay trong gói này.** Đề xuất: có.

## Quyết định (2026-11-12)

Người dùng đã trả lời các câu hỏi mở: 1 (b), 2 (b), 3 (a), 4 (b), 5 (a), 6 (a). Chi tiết ở mục
"Rulings" của bản tiếng Anh.

**Bỏ probe 1 (2026-11-12).** Probe UDF ra khỏi gói này theo lựa chọn của người dùng; báo cáo ghi là
ranh giới đã chọn, kèm follow-up: FE phân tích câu lệnh trước khi kiểm tra quyền, nên có thể tải file
jar của UDF cả khi người gọi bị từ chối (chưa đo). Chi tiết ở bản tiếng Anh.
