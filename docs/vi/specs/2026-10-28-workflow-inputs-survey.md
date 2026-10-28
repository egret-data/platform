> **Bản tóm tắt tham khảo.** Bản tiếng Anh `docs/en/specs/2026-10-28-workflow-inputs-survey.md` là bản chuẩn và đầy đủ (260 dòng); bản này chỉ tóm tắt và không thay thế nó.

# Khảo sát cho gói lộ trình #3, "Kiểm inputs khi start"

Ngày 2026-10-28. Khảo sát chỉ-đọc về cách Kestra (checkout `96893b58`, `2.0.0-SNAPSHOT`, mới hơn golden image đã ghim) kiểm inputs, đối chiếu với egret tại `ff25210b`.

- Mọi khẳng định về Kestra có file:line trong bản tiếng Anh; các quyết định dựa trên chúng nằm ở `docs/vi/specs/2026-10-28-workflow-inputs-design.md`.
- Lưu ý phiên bản: FORM, `ValueOption {label,value}`, `prefill` và `renderError` có thể chưa có trong image đã ghim; chưa kiểm.
- Kết luận chính: Kestra kiểm `required`, kiểu, `values`, `validator`, `min`/`max` và biên ngày ở start (HTTP 422) và liệt kê lỗi trong endpoint `validate`; egret trước #3 không kiểm gì trong số đó ở start.
