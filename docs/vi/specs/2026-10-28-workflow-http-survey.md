> **Bản tóm tắt tham khảo.** Bản tiếng Anh `docs/en/specs/2026-10-28-workflow-http-survey.md` là bản chuẩn và đầy đủ; bản này chỉ tóm tắt và không thay thế nó.

# Khảo sát cho gói lộ trình #4, "HTTP và các task nền tảng chưa được phục vụ"

Ngày 2026-10-28. Khảo sát chỉ-đọc về `http.Request`, `http.Download` và `http.SseRequest` của Kestra (checkout `96893b58`, `2.0.0-SNAPSHOT`), đối chiếu với egret tại `1a72e827` (#3a đã merge).

- Mọi khẳng định về Kestra có file:line trong bản tiếng Anh; các quyết định dựa trên chúng nằm ở `docs/vi/specs/2026-10-28-workflow-http-design.md`.
- **Oracle:** corpus không có flow nào dùng ba task http; gói #4 bank **0** mục. Bằng chứng phải là test mới (server `axum` cục bộ và resolver được inject).
- **Vị trí chạy:** handler chạy trong chính process egret, nên SSRF quan trọng: process đó với tới Postgres, Kubernetes API, endpoint admin của engine và cloud metadata.
- **Kestra không chặn gì mặc định:** proxy do tác giả chọn, có trust-all, redirect được đi theo mà không vet, operator chỉ có danh sách tiền tố. Khảo sát đề xuất phương án A (chặn địa chỉ không public, ghim địa chỉ, vet lại mỗi redirect, `.no_proxy()`, cộng allowlist theo deployment); người dùng chọn A (Q1).
- **Sáu dòng chưa được phục vụ:** nối `egret.sql` (cần dựng query port trước `handlers_for` và một resolver `RefKind::Database`); gỡ `core.http`, `core.slack`, `egret.dataset.refresh`; hoãn `egret.dataapi.call` và `egret.report.send`. Gỡ một dòng chỉ đổi mã lỗi cho các flow vốn không chạy được.
- **Các lựa chọn hành vi** (giới hạn body 1 MiB, tên file Download ổn định, giới hạn Download và SSE, `DIGEST`, `encryptBody`, alias `fs.http`, `body` giữ là string, quy tắc `allowedResponseCodes`) được liệt kê ở §6 của bản tiếng Anh và được chốt ở spec D4–D10.
