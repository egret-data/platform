# Governance SP2b Task 26: bật OPA mặc định (plan triển khai)

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-08-gov-sp2b-task26-opa-default-on.md); mọi giá trị chính xác và các bước nằm ở bản tiếng Anh, và khi hai bản khác nhau thì bản tiếng Anh đúng.

**Mục tiêu:** đổi mặc định của `[governance.opa] enabled` thành `true`. Trước đó phải đóng nốt các mục readiness còn mở sau khi gói cổng 8080 (#842, #843) đã merge.

**Quyết định:**
- Người dùng, 2026-11-07:
  - profile không có tag service thì cảnh báo trong câu trả lời switch và trong explain, không chặn;
  - KI-352, KI-353 và KI-354 chỉ ghi vào docs.
- Controller, 2026-11-08: giá trị Helm `opa.enabled` cũng mặc định là `true`.

## Các task

- **Task 1.** Test qua `opa_router` với body `/batch` khoảng 5 MiB, tức trên 4 MiB:
  - dưới giới hạn thì không bị 413;
  - vượt `batch_body_limit_bytes` thì 413;
  - body `/allow` trên 4 MiB thì 413.
- **Task 2.** Cảnh báo `no_tag_service_warning` khi một profile Trino không có tag service:
  - xuất hiện trong câu trả lời switch (200 và 502);
  - xuất hiện trong `ExplainProfile`, trên cả Ranger và OPA;
  - frontend hiện cảnh báo; có test cho mọi phần trên.
- **Task 3.** Đo độ trễ refresh khi N profile dùng chung một tag service, với N = 1, 5, 10, 20:
  - nếu ở N = 10, profile cuối cùng có snapshot mới trong vòng 1 s thì không sửa code;
  - nếu chậm hơn thì sửa, rồi đo lại;
  - nêu rõ cửa sổ thống kê cũ;
  - kết quả ghi bổ sung vào báo cáo latency.
- **Task 4.** Bật mặc định:
  - `#[serde(default = "default_true")]`, và đổi tên test thành `governance_opa_is_on_by_default`;
  - sửa các test và file compose bị ảnh hưởng;
  - Helm: `opa.enabled: true` và render env một cách tường minh.
- **Task 5.** Docs:
  - phần điều kiện cần của "Trino on OPA", khoá cấu hình, Kubernetes, release notes và upgrade;
  - KI-352, KI-353, KI-354;
  - cảnh báo tag service;
  - câu chữ CARRY về listener 8082;
  - ghi chú độ trễ khi tag service dùng chung.

  Chụp lại screenshot `trino-config` là việc của controller.

## Ràng buộc chung

Xem bản tiếng Anh:
- dùng target riêng `target-t26`;
- không chạy hai bộ test cùng lúc;
- không prune Docker;
- tạo lại client frontend chỉ bằng `npm run gen:api`;
- mỗi thay đổi hành vi có một test, kèm một mutant bị test đó bắt.
