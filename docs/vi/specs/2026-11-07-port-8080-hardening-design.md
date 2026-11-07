# Gia cố cổng 8080: identity của Trino qua HTTP thường, và listener riêng cho OPA

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/specs/2026-11-07-port-8080-hardening-design.md); khi hai bản khác nhau, bản tiếng Anh đúng.

**Trạng thái:** thiết kế ngày 2026-11-07, người dùng đã duyệt từng phần. Đây là điều kiện trước của Task 26 trong governance SP2b, tức bật `[governance.opa] enabled` mặc định. Thiết kế giao thành hai PR code (A trước, B sau), mỗi PR kèm một PR docs trong `egret-data/docs`.

## 1. Vấn đề

Hai lỗi chung một gốc: cổng 8080 cho quá nhiều bên gọi tới, nhiều hơn mức nó nên tin.

**A. Bên nào gọi tới được cổng 8080 của coordinator Trino cũng có thể làm bất kỳ user nào.**
- egret ghi `allow-insecure-over-http=true` cho mọi engine, trừ engine jwt chạy TLS (`trino.rs:147`). Với cờ này, Trino không xác thực request đến qua HTTP thường; user ghi trong request được nhận nguyên như vậy.
- Lỗi đã được đo trong Task 21 trên engine impersonate có `"auth":{"password":{}}`, trên cả Ranger lẫn OPA: `trino --user ann --session-user bob` không kèm password vẫn chạy với quyền bob, và không có kiểm tra impersonation nào được gọi.
- Comment ở `trino.rs:144-146` sai.
- Engine không có khối auth thì không có bộ xác thực nào, nên bị lỗi này bất kể cờ đặt thế nào.
- Những bên gọi tới được cổng 8080: mọi pod trong namespace engine, mọi pod egret, `ingressFrom` (gồm ingress controller) và `extraFrom`.

**B. `/opa/` dùng chung cổng API 8080 của egret với pod app và pod task.**
- Bề mặt này phải đọc body rồi mới xác thực được, vì credential nằm trong body.
- Ngân sách của nó là chung: 64 đơn vị in-flight và 16 đơn vị parse.
- Chưa có NetworkPolicy nào chặn đường vào pod egret: pod app được mở rõ ràng tới `egret:8080`, còn pod task gọi tới được cả release namespace.
- Cách tấn công: mở 64 kết nối, khai báo `Content-Length` lớn rồi đứng im. Mọi engine khác sẽ nhận 503.
- Giới hạn theo từng peer bị loại, vì mỗi pod có địa chỉ riêng, đi qua ingress thì chỉ còn một địa chỉ, và server chưa có `ConnectInfo`.

## 2. Quyết định

**Người dùng quyết, 2026-11-07:**
- **Q1.** Mọi engine Trino do egret provision đều có xác thực.
- **Q2.** `/opa/` chuyển sang listener riêng, kèm một NetworkPolicy chỉ cho engine gọi tới.
- **Q3.** Chuyển hẳn sang listener mới, và egret tự re-apply những engine còn trỏ URL cũ. Engine ngoài phải cài lại file plugin.
- **Ngoài phạm vi:** StarRocks cổng 9030 cho `root` đăng nhập không password. Việc này là follow-up riêng.

**Phần A:**
- **A1.** Spec không có `trinoConfig.auth` được coi như có `"auth": {"password": {}}`. Delegator và `egret-monitor` được thêm vào như hiện nay. Spec lưu cũng được chuẩn hoá theo; request xoá khối auth sẽ bị chuẩn hoá lại chứ không bị từ chối. Engine cũ được bổ sung auth ở lần apply tới, và danh sách engine đánh dấu chúng cho tới khi được re-apply.
- **A2.** Ghi `allow-insecure-over-http=false` cho mọi engine.
  - Request HTTP thường không kèm `X-Forwarded-Proto: https` bị từ chối, trừ các resource public của Trino.
  - Request kèm header đó bị coi là bảo mật, nên phải có password hoặc JWT hợp lệ.
  - Worker vẫn dùng shared secret.
- **A3.** Các đường của egret vẫn chạy: monitor, truy vấn ủy quyền qua 8443, client qua ingress, readiness probe, `preStop`. Probe và `preStop` sẽ được kiểm trên engine thật; nếu bị từ chối thì chúng dùng credential của monitor.
- **A4.** Kiểm trên engine thật trước khi viết code, với một engine Ranger và một engine OPA. Kết quả ghi vào `docs/en/plans/2026-11-07-port-8080-hardening-live-result.md`.
- **A5.** Sửa comment ở `trino.rs`, và ghi chú đính chính vào spec 2026-07-15.

**Phần B:**
- **B1.** Thêm khoá `[governance.opa] listen`, mặc định `0.0.0.0:8082`. Listener này chỉ phục vụ router OPA, theo mẫu listener 8081 của apps. Cổng 8080 trả 404 cho `/opa/`. `/ranger/`, các route machine-IdP và Iceberg catalog ở lại 8080. Cấu hình trùng cổng bị từ chối.
- **B2.** Thêm khoá `[governance.opa] engine_url`. Nếu không đặt, egret lấy `engine_egret_url` và thay cổng bằng cổng của listener. `opa_plugin_files` và route plugin-config dùng URL này.
- **B3.** Helm:
  - Deployment và Service thêm cổng `opa` (8082).
  - Thêm `networkpolicy-egret.yaml` chọn pod `app: egret`. Cổng 8080 và 8081 nhận từ mọi nơi; cổng 8082 chỉ nhận từ `engineNamespace` và `opa.extraFrom`.
  - Policy này bật cùng với policy của engine.
- **B4.** Compose: engine trỏ tới `http://egret:8082`.
- **B5.** Migration **0215** thêm cột `engine_mgmt.engines.applied_opa_url`.
  - Mỗi lần apply sinh file plugin OPA, egret ghi lại URL đã dùng.
  - Khi khởi động, nếu OPA đang bật, egret re-apply những engine provisioned trên profile OPA có URL cũ. Engine đã có URL đúng thì không bị đụng tới.
  - Lỗi được ghi theo từng engine và không làm dừng server.

## 3–5. Hình dạng thay đổi, test và follow-up

Xem bản tiếng Anh §3–§5. Tóm tắt:
- **PR A:** render, chuẩn hoá spec, kết quả đo trên engine thật, dấu cần re-apply trong danh sách engine, docs.
- **PR B:** khoá cấu hình, listener, tách router, URL plugin, migration 0215 và lượt re-apply khi khởi động, Helm, compose, `opa-e2e`, docs.
- **Test:**
  - unit test cho render, kèm mutant;
  - kịch bản `opa-e2e` chứng minh `--session-user` bị từ chối trên cả hai engine;
  - test router: 404 trên 8080, có trả lời trên listener mới;
  - `helm template`;
  - DB test cho lượt re-apply khi khởi động.
- **Follow-up:** StarRocks 9030, và phần còn lại của Task 26.
