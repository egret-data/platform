# Plan triển khai: gia cố cổng 8080

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-07-port-8080-hardening.md); mọi giá trị chính xác, đoạn code và các bước nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Mục tiêu:** PR A chặn việc giả danh user qua HTTP thường trên engine Trino. PR B chuyển `/opa/` sang một listener riêng mà chỉ engine gọi tới được. Cả hai là điều kiện trước của Task 26 (SP2b).

**Spec:** `docs/en/specs/2026-11-07-port-8080-hardening-design.md`.

## PR A: identity của Trino (nhánh `feat/port-8080-hardening`, migration 0215)

- **A1.** Đo trên một Trino 483 chạy độc lập, cấu hình giống bản egret sinh ra, không cần build image egret. Mỗi ca chạy hai lần, cờ `true` rồi `false`. Kết quả ghi vào tài liệu live-result. Nếu một request chỉ kèm `X-Forwarded-Proto: https` mà không có credential vẫn chạy được, thì dừng lại và báo BLOCKED.
- **A2.** Mọi engine có auth đều được ghi `allow-insecure-over-http=false`. Sửa comment sai. Probe và `preStop` chỉ được sửa khi A1 cho thấy chúng hỏng.
- **A3.** Thêm hàm `normalise_trino_auth`: spec không có auth thì nhận `{"password":{}}`. Hàm được gọi ở create, update và lúc apply.
- **A4.** Migration 0215 thêm cột `applied_security_rev`. Thêm `TRINO_SECURITY_REV = 1`, trường `security_reapply_needed` trong `EngineDto`, và badge "Re-apply needed" trên danh sách engine.
- **A5.** Kịch bản e2e chứng minh request không có credential bị từ chối trên cả hai cơ chế. Thêm ghi chú đính chính vào spec 2026-07-15.
- **A6.** Docs: trang bảo mật engine, upgrade, known issues.

## PR B: listener OPA (nhánh `feat/opa-listener`, migration 0216)

- **B1.** Thêm khoá `[governance.opa] listen` (mặc định `0.0.0.0:8082`) và `engine_url`, cùng hàm `opa_engine_url`. Cấu hình trùng cổng bị từ chối.
- **B2.** Tách `opa_router` khỏi `build_router`, nên cổng 8080 trả 404 cho `/opa/`. Mở listener thứ hai trong `main.rs`.
- **B3.** Thêm `engine_opa_url` vào `CpState` và state của policy admin. `ranger_binding` chọn URL theo cơ chế; file plugin của engine ngoài cũng dùng URL mới.
- **B4.** Migration 0216 thêm cột `applied_opa_url`. Khi khởi động, `reapply_stale_opa_engines` chỉ re-apply những engine có URL cũ.
- **B5.** Helm: thêm block giá trị `opa`, cổng `opa`, và `networkpolicy-egret.yaml`. Policy này để 8080/8081 mở cho mọi nơi, còn 8082 chỉ nhận từ `engineNamespace` và `opa.extraFrom`.
- **B6.** Compose và `opa-e2e`: engine trỏ tới cổng 8082, và `egret:8080/opa` trả 404.
- **B7.** Docs: Trino on OPA, khoá cấu hình, giá trị Helm, upgrade.

## Ràng buộc chung

Xem mục "Global Constraints" ở bản tiếng Anh. Các điểm chính:
- dùng target riêng `target-p8080`;
- test DB chạy với `env -u EGRET_ALLOW_DB_SKIP`, và không chạy hai bộ test cùng lúc;
- không prune Docker và không đụng container của người khác;
- tạo lại client frontend chỉ bằng `npm run gen:api`;
- mỗi thay đổi hành vi có một test fail khi chưa sửa, và một mutant bị test đó bắt.
