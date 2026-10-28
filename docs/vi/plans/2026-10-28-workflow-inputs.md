> **Bản dịch tham khảo.** Bản tiếng Anh `docs/en/plans/2026-10-28-workflow-inputs.md` là bản chuẩn; khi hai bản khác nhau, bản tiếng Anh thắng. Bản này đi theo bản tiếng Anh từng mục một. Mã, tên khoá, mã lỗi, đường dẫn, cờ lệnh, thông báo người dùng thấy được và các khối mã giữ nguyên tiếng Anh.

# Gói workflow #3 "Kiểm inputs khi start" — kế hoạch triển khai

Spec: `docs/en/specs/2026-10-28-workflow-inputs-design.md` (các quyết định D1–D16). Khảo sát: `docs/en/specs/2026-10-28-workflow-inputs-survey.md`. Nhánh `feat/workflow-inputs`. Crate `crates/egret-workflow` trừ khi ghi khác; đường dẫn tương đối với `src/`.

## Ràng buộc chung

- Các số D của spec có tính ràng buộc; chỗ nào kế hoạch này và spec khác nhau, spec thắng. Source Kestra: `/Users/1pro-l4r8/Documents/github/data-portal/kestra` (khảo sát trích file:line).
- Câu lỗi là của Kestra nguyên văn (spec D3). Status 400, body `{"error": …}`.
- Conventional Commits; trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`; không dùng network (controller push); không bao giờ đụng cổng 5432 hay container `egret-dev-*`/`egret-demo-*`; scratch không vào git.
- Mọi `std::env::var` mới cần `cargo test -p egret-config`. Ratchet oracle: `cargo test -p egret-workflow --test executor` không được thụt lùi (301 / 52 / 52 / 0 lúc bắt đầu).
- Hạ tầng test: container Postgres + Valkey riêng tên `egret-inputs-postgres` (:5461) và `egret-inputs-valkey` (:6461); `CARGO_TARGET_DIR=<worktree>/target`.

## Phần A — PR #3a (backend)

**A1. Model.** `models/input.rs`: thêm các field phẳng `validator`, `min`, `max` (số JSON / string duration ISO giữ dạng `serde_json::Value` hoặc có kiểu theo D4 — chọn cách ít xâm lấn nhất mà vẫn round-trip), `after`, `before`, `values` (scalar hoặc `{label,value}`), `expression`, `allow_custom_value` (mặc định false), `is_radio`, `auto_select_first`, `json_schema`. serde camelCase; test round-trip theo kiểu các test sẵn có. Gỡ các cảnh báo `not_honoured` cho các key nay đã model (`spec/keys.rs:192-204`, `:881-915`) và cập nhật `spec/schema/property-treatment.json`; `inputs` của FORM / `ref` của REUSABLE vẫn bị cảnh báo. Kiểm schema README và JSON schema sinh ra vẫn nhất quán.

**A2. Checker scalar.** `inputs_check.rs` mới (thuần, `#![forbid(unsafe_code)]` theo kiểu của crate): `resolve(inputs: &[Input], posted: &Value) -> Result<Value, InputError>`; các kiểu scalar, required, quy tắc string rỗng (D5), `validator` của STRING/SECRET (`fancy-regex`, thêm làm dependency trực tiếp), parse + biên của INT/FLOAT/BOOL/DATETIME/DATE/TIME/DURATION, URI, EMAIL, `values` của SELECT (điểm lạ ở D4). Table test cho mỗi kiểu: hợp lệ, từng câu lỗi, string rỗng, null, số JSON so với string.

**A3. Collection, dependsOn, expression.** ARRAY/MULTISELECT (+ `itemType`), parse JSON/YAML/ION, `dependsOn.inputs` và `condition` (render bằng renderer của workflow với các giá trị phụ thuộc; tìm điểm vào của nó trước), `expression` của SELECT/MULTISELECT, `autoSelectFirst`. Quyết định `jsonSchema` (D12) bằng cách tìm crate JSON Schema có sẵn trong `Cargo.lock`; nếu không có cái nào chấp nhận được, giữ cảnh báo và ghi lại.

**A4. Nối vào.** Gọi `inputs_check::resolve` trong `worker/claim.rs` `start_execution_in_on` giữa bước chuẩn hoá object và `secret_inputs::split`; lưu object đã ép kiểu (D2); bỏ qua với `replayed_from_execution_id` và với start từ Flow-trigger (D7) — tìm xem một start Flow-trigger được nhận diện thế nào trong `NewExecution` và thêm cờ tường minh nếu chưa có. Subflow: gọi cùng hàm trong `worker/subflow.rs` trước `secret_inputs::split` (:497), làm run cha FAILED với `subflow inputs: …`. Map lỗi sang `Error::InvalidInput`. Cập nhật câu 400 trong tài liệu utoipa của `start_execution` (`routes.rs` ~2541) và sinh lại `openapi.json` (`cargo run -p egret-bin --bin egret -- openapi`) cùng client (`npm --prefix frontend run gen:api`). Integration test (`tests/inputs_start.rs`): API JSON, multipart, webhook, schedule, subflow con, replay không kiểm lại, Flow trigger không kiểm và fire born-FAILED vẫn chạy, validator của SECRET thấy giá trị rõ, không tạo gì khi bị từ chối.

**A5. Khoảng trống lúc lưu (D11).** Trong `spec/validate.rs` cạnh `input_item_type_invalid`: `dependsOn` không biết, vòng, `defaults` + `prefill`, scheme `defaults` của FILE, `validator` không compile được. Thông báo như của Kestra (khảo sát §1.1). Đo xem `fancy-regex` có từ chối `\` như Java không cho `inputs-bad-validator-syntax`; báo kết quả dù thế nào.

**A6. Seam harness + bank.** `tests/executor.rs` và `oracle-exec/driver.rs:1175`: thêm seam start (D8); bank `invalids/inputs-with-multiple-constraint-violations`; chỉ bank `inputs-bad-validator-syntax` nếu A5 đã đo. Cập nhật `oracle-exec/baseline.txt` và README. Ratchet đo được ghi vào spec.

**A7. Tài liệu và gate.** Bản VI của spec và kế hoạch này, dòng #3 của lộ trình (EN+VI), README của oracle, dòng trạng thái của spec. Gate: `cargo test -p egret-config -p egret-server`, toàn bộ suite `egret-workflow` (chỉ 7 test `sql_starrocks_live` được phép hỏng), `cargo fmt --check`, `cargo clippy` như CI.

## Phần B — PR #3b (endpoint validate và form run)

**B1. Endpoint (D14).** `POST …/flows/{id}/executions/validate`, body JSON, luôn 200 với shape của Kestra, lỗi theo từng input qua một biến thể `resolve_all` của checker thu thập lỗi của mọi input thay vì dừng. Không lưu hay upload gì. 404/409 như start. Tài liệu utoipa, `openapi.json`, sinh lại client, drift test. Route test (200 khi không hợp lệ, mọi input được liệt kê, không có dòng execution).

**B2. Form run (D15).** `frontend/src/components/workflows/run/inputModel.ts`, `RunFlow.tsx`: kiểm tra client mới, gọi endpoint validate khi submit, gắn lỗi theo input, viết lại comment module đã cũ, sửa `BackfillDialog.tsx` `"BOOLEAN"` → `"BOOL"`. Vitest cho từng cái. Cần `npm --prefix frontend ci` trong worktree.

**B3. Tài liệu.** Ghi chú cho `egret-data/docs` (hướng người dùng: cái gì được kiểm lúc start, thông báo, 400, endpoint validate, các khác biệt BOOL / NaN / status của webhook), lộ trình và trạng thái spec.
