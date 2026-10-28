> **Bản dịch tham khảo.** Bản tiếng Anh `docs/en/specs/2026-10-28-workflow-inputs-design.md` là bản chuẩn; khi hai bản khác nhau, bản tiếng Anh thắng. Bản này đi theo bản tiếng Anh từng mục một. Mã, tên khoá, mã lỗi, đường dẫn và các câu thông báo người dùng thấy được giữ nguyên tiếng Anh.

# Gói workflow #3 "Kiểm inputs khi start"

**Trạng thái:** thiết kế ngày 2026-10-28; #3a đã xây (Task A1-A7, PR #3a đang review); #3b chưa xây. Dòng #3 của lộ trình `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`. Kế hoạch: `docs/en/plans/2026-10-28-workflow-inputs.md`. Nhánh `feat/workflow-inputs`. Hai PR: **#3a** (việc kiểm lúc start, các khoảng trống lúc lưu, seam của harness) và **#3b** (endpoint validate và form run).

**Các khẳng định lấy từ đâu.** Bản khảo sát là `docs/en/specs/2026-10-28-workflow-inputs-survey.md` (bản sao của khảo sát chỉ-đọc làm ngày 2026-10-28; mọi khẳng định về Kestra có file:line ở đó). Kestra là bản checkout `data-portal/kestra`, `96893b58`, **mới hơn golden image đã ghim**. FORM, `prefill`, `renderError` và quy tắc "default phải là required" có thể chưa có trong image. Mọi điều phụ thuộc vào một trong số đó đều nói rõ.

## 1. Vấn đề

egret lưu inputs đúng như được post và ép kiểu ở mỗi lần render. Không đường start nào kiểm `required`, kiểu, `values`, `validator`, `min`/`max` hay giới hạn ngày. Chỉ backfill kiểm `required`, và start kiểm các input FILE. Một input sai làm start một execution rồi hỏng ở render, hoặc chạy với giá trị sai (một BOOL `"yes"` vẫn là string). Kestra từ chối start (HTTP 422) và liệt kê các lỗi trong endpoint `validate` của nó.

## 2. Các quyết định (quy tắc tự chủ: controller quyết định ngày 2026-10-28; ghi lại ở đây)

- **D1. Một checker, một điểm nghẽn.** Một module thuần `inputs_check` nhận `inputs:` của flow, object được post và một đồng hồ, rồi trả về object đã resolve hoặc lỗi **đầu tiên** theo thứ tự khai báo. Nó được gọi từ `start_execution_in_on` (`worker/claim.rs:3896`), sau khi object được chuẩn hoá (≈ dòng 3936) và **trước** `secret_inputs::split` (≈ 3943), vì `validator` của SECRET phải thấy giá trị rõ. Kestra chỉ báo input sai đầu tiên khi start; egret cũng vậy (một tập lớn hơn sẽ làm hỏng golden). Endpoint validate (D14) liệt kê tất cả.
- **D2. Cái được lưu.** Giá trị **đã ép kiểu**, để replay và mọi lần render thấy giá trị có kiểu. Key không khai báo bị bỏ (Kestra bỏ chúng kèm cảnh báo); route multipart giữ lỗi 400 chặt hơn cho part lạ hoặc trùng. **Default vẫn được điền lúc render**, như hiện nay (`render.rs` `with_declared_defaults`); chúng được kiểm lúc start khi default là literal. Một default có template (`{{`) không được kiểm lúc start và được liệt kê ở §6 như một khoảng trống.
- **D3. Thông báo.** Dạng có tiền tố của Kestra được tái tạo: ``Invalid value for input `<id>`. Cause: <cause>``, với các cause của Kestra nguyên văn: ``it must match the pattern `<v>` ``, ``it must be more than `<min>` `` / ``it must be less than `<max>` ``, ``it must be after `<after>` `` / ``before``, ``It must be more than `<min>` `` cho DURATION (chữ I hoa, như Kestra), ``it must match the values `[a, b]` ``, ``value `<x>` doesn't match the values `[A, B, C]` `` cho MULTISELECT, ``Unable to parse array element as `<T>` on `<el>` ``, ``Cannot convert value '<v>' to <Class>`` (giữ tên class Java), `The input must be a valid email`. Một giá trị bắt buộc bị thiếu là ``Missing required input:<id>`` (không có dấu cách, như Kestra). Lớp bọc code-block của Kestra cho lỗi parse JSON/YAML/ION và câu "encryption is not configured" **không** được chép: lỗi parse dùng dạng có tiền tố. Status giữ **400** với body `{"error": …}` của egret (Kestra: 422); khác biệt đã ghi. Webhook trả 400 kèm thông báo (Kestra: 422, body rỗng).
- **D4. Các kiểu** (parse rồi validate, theo khảo sát §1.3):
  - STRING: mọi scalar thành string của nó; `validator` full-match. SECRET: như vậy, trên giá trị rõ, trước khi seal.
  - INT: một số nguyên JSON hoặc một string số nguyên vừa 32 bit; `1.0` dạng string bị từ chối, một số JSON không có phần thập phân thì được chấp nhận. FLOAT: mọi số hữu hạn hoặc string số (`NaN`/`Infinity` bị từ chối: **khác biệt**, chúng làm hỏng việc lưu JSON). `min`/`max` bao gồm biên.
  - **BOOL: `true`/`false` (mọi kiểu chữ) hoặc một boolean JSON; mọi giá trị khác bị từ chối.** Kestra biến mọi giá trị khác thành `false` một cách im lặng; đó là một lần chạy sai, nên đây là một **khác biệt** đã ghi.
  - DATETIME (ISO instant), DATE, TIME, DURATION (ISO 8601, `PT…`/`P…D`): parse chặt, `after`/`before`, `min`/`max`.
  - URI (phải có scheme), EMAIL (regex của Kestra, chấp nhận rỗng), JSON (parse; `jsonSchema` ở D12), YAML (parse), ION (parse), ARRAY và MULTISELECT (một mảng JSON hoặc một string mảng JSON, mỗi phần tử được parse với `itemType`).
  - SELECT/MULTISELECT: kiểm thuộc `values` trừ khi `allowCustomValue`. **SELECT chỉ kiểm `values` khi `required` là true (điểm lạ của Kestra, giữ lại cho parity)**; MULTISELECT luôn kiểm.
  - FILE: không đổi (`check_file_inputs`). FORM và REUSABLE_INPUTS: không đổi (chưa hỗ trợ; form run đã vô hiệu chúng). Khảo sát ghi FORM không phải `@EeOnly` trong source mới hơn; kiểm trên image đã ghim là việc theo sau (§6).
- **D5. String rỗng.** `""` là vắng mặt với mọi kiểu trừ STRING, SELECT, EMAIL và SECRET (Kestra).
- **D6. `required` và `dependsOn`.** Một giá trị null/vắng mặt không qua được `required` (mặc định `true`). `dependsOn.inputs`: một input bị **vô hiệu** trừ khi mọi input được liệt kê đều được bật và khác null; input bị vô hiệu bị bỏ và bỏ qua `required`. `dependsOn.condition` được render bằng renderer của workflow với `inputs` (các giá trị phụ thuộc); chỉ boolean `true` mới bật. Một id phụ thuộc không biết bị bỏ qua lúc chạy (Kestra) và bị từ chối lúc lưu (D11).
- **D7. Các đường start.** Mọi đường qua `start_execution_in_on` đều được kiểm, trừ:
  - **Replay** (`replayed_from_execution_id` được đặt) và **bản sao Loop/retry** không kiểm lại (Kestra sao chép inputs đã lưu). Vì D2 lưu giá trị đã ép kiểu, kiểm lại cũng idempotent.
  - **Flow trigger**: Kestra không validate các inputs này (nó render chúng và điền default lúc render). **egret theo Kestra: các lần fire của Flow-trigger không được kiểm.** Dòng lộ trình ghi "flow-trigger start"; phần đó vượt quá Kestra và bị bỏ. Một giá trị sai lộ ra ở render, như trước. Điều này cũng giữ cho lần fire `born_failed` (mang `{}`) tiếp tục chạy.
  - **Subflow con** có INSERT riêng (`subflow.rs:514`): cùng checker được gọi trước `secret_inputs::split` ở đó; một lần từ chối làm run Subflow của cha FAILED với `subflow inputs: <message>` (khớp Kestra).
  - **Schedule fire / tick backfill**: checker chạy; một lần từ chối giữ cách xử lý `Refused` hiện nay. Kiểm lúc lưu các inputs literal của schedule là việc theo sau (§6).
- **D8. Seam của harness.** `tests/executor.rs` có một seam **start**: một start bị từ chối với `InvalidInput` là `Refused`, không phải `Broken` (`oracle-exec/driver.rs:1175`). Điều này bank `invalids/inputs-with-multiple-constraint-violations` (thông báo golden được so theo tiền tố và theo tập các phần tử sai, vì thứ tự của Kestra là thứ tự hash).
- **D9. Thời gian.** `after`/`before` của date/time so với các instant đã parse; không đọc đồng hồ lúc start (default như `{{ now() }}` có template và thuộc khoảng trống của D2).
- **D10. Phương ngữ regex.** `validator` dùng `fancy-regex` (đã có trong `Cargo.lock`, nên không thêm crate bắc cầu mới; nó thành dependency trực tiếp của `egret-workflow`) với giới hạn backtrack thay cho timeout 10 giây của Kestra. Lookaround và backreference chạy được; possessive quantifier và một số cú pháp chỉ có ở Java thì không. Thông báo khi chạm giới hạn là của Kestra ``the validator pattern `<v>` timed out — it may be too complex``.
- **D11. Khoảng trống lúc lưu** (trong #3a): `dependsOn` nêu một input không tồn tại, vòng `dependsOn`, `defaults` cùng `prefill`, `defaults` của FILE có scheme không phải `file`/`nsfile`, và một `validator` không compile được (bank `invalids/inputs-bad-validator-syntax` **nếu** `fancy-regex` từ chối `\` như Java; đo ở Task 5). **Không thêm:** "`defaults` cần `required: true`" (chưa kiểm được trong image đã ghim; không flow nào trong corpus có `defaults` với `required: false`).
- **D12. `jsonSchema`.** Chỉ thêm nếu đã có một crate JSON Schema trong cây hoặc nó nhỏ và đã được chính sách dependency duyệt; nếu không nó vẫn là cảnh báo `not_honoured` và chuyển sang các việc theo sau. Quyết định ở Task 4 bằng phép đo.
- **D13. `expression` của SELECT/MULTISELECT.** Được render lúc start bằng renderer của workflow; một lỗi là lỗi ``Cannot render 'expression'`` của Kestra. `autoSelectFirst` điền default bằng giá trị đầu tiên. `isRadio` là gợi ý UI.
- **D14. Endpoint validate (#3b).** `POST /api/ws/{ws}/workflows/flows/{id}/executions/validate`, body **JSON** `{ "inputs": {…} }` (của Kestra là multipart; form run của egret đã nói JSON, và một part FILE được biểu diễn chỉ bằng tên file). Không lưu gì, không upload gì. **Luôn 200** với shape của Kestra `{ id, namespace, inputs: [{ input, value, enabled, isDefault, errors: [{ message, renderError }] }], checks: [] }` (`checks` để rỗng đến gói #16). 404 cho flow không có và 409 cho flow bị vô hiệu, như start.
- **D15. Form run (#3b).** Form giữ các kiểm tra phía client và thêm: `min`/`max`, `after`/`before`, `validator` (hiển thị, không đánh giá trong trình duyệt), và gọi endpoint validate khi submit để một nguồn sự thật quyết định. Lỗi từ server gắn vào input được nêu trong dấu backtick (đã như vậy). Comment của module nói "the server checks nothing" được viết lại. Lỗi chính tả `"BOOLEAN"` (`BOOL`) ở `BackfillDialog` được sửa trong cùng PR.
- **D16. Model.** `models::Input` có thêm `validator`, `min`, `max`, `after`, `before`, `values`, `expression`, `allow_custom_value`, `is_radio`, `auto_select_first`, `json_schema`, để phẳng như `item_type`. Các cảnh báo `not_honoured` cho những key nay được honoured bị gỡ; bảng xử lý key (`spec/schema/property-treatment.json`) được cập nhật; `inputs` của FORM và `ref` của REUSABLE vẫn bị cảnh báo.

## 3. Ngoài phạm vi (nêu tên, kèm gói gỡ nó)

Resume với inputs `onResume` và resume-validate (#9). Replay với inputs mới (#7). `checks:` (#16). Default có template được kiểm lúc start (việc theo sau, §6). FORM / REUSABLE_INPUTS. FILE với đường dẫn local (cố ý không port: chúng đọc đĩa của server).

## 4. Ratchet kỳ vọng

`invalids/inputs-with-multiple-constraint-violations` (D8) và, nếu D10 cho phép, `invalids/inputs-bad-validator-syntax`. Mọi thứ khác không được nhúc nhích: **301 / 52 / 52 / 0 → 301 / 50 / 50 / 0** tốt nhất, 301 / 51 / 51 / 0 nếu không.

**Đo ở Task A6 (2026-10-28): 301 / 50 / 50 / 0**, `(301, 251, 50)`; census 295 live, 25 bị validate từ chối, 21 kiểu không biết, không đổi. Cả hai mục đều được bank: mục multi-constraint ở `Seam::Start` mới (các dòng của egret bằng 422 Kestra đã ghi, xét như multiset), mục validator ở `Seam::Save` (`input_validator_invalid`). Chi tiết ở `crates/egret-workflow/oracle-exec/README.md`, mục "Workflow inputs #3 Task A6".

## 5. Kiểm thử

Một table test cho mỗi kiểu trong `inputs_check` (hợp lệ, từng câu lỗi, string rỗng, null). `dependsOn` bật/tắt với `required`. Một integration test cho mỗi đường start ở D7 (API JSON, multipart, webhook, schedule, subflow con, replay không kiểm lại, Flow-trigger không kiểm, bỏ qua born-failed). Endpoint validate: luôn 200, mọi input được liệt kê, lỗi theo input, không lưu gì (số dòng không đổi). Các drift test (`openapi.json`, client sinh ra) phải qua. Frontend: các kiểm tra mới của form và việc gắn lỗi server vào input.

## 6. Các việc theo sau đã ghi

Kiểm FORM trên image đã ghim. Validate default có template lúc start. Kiểm lúc lưu các inputs literal của schedule. `jsonSchema` nếu D12 hoãn nó. `defaults ⇒ required` nếu image đã ghim có quy tắc đó. Status 422 cho webhook/API nếu một client nào cần đúng status của Kestra.

## 7. Như đã xây (2026-10-28, #3a)

Những gì Task A1-A6 tìm thấy khác với, hoặc thêm vào, các quyết định ở trên. Các quyết định giữ nguyên như bản ghi; chỗ nào khác nhau, mục này thắng.

- **`jsonSchema` (D12, D16).** Vẫn không được kiểm. `Cargo.lock` không có validator JSON Schema (`jsonschema`, `boon` và `valico` đều vắng), nên không thêm gì. Nó giữ cảnh báo `not_honoured`, và `json_schema` được gỡ khỏi `models::Input` (D16 có liệt kê nó): một key được model hoá mà schema flow không phục vụ làm hỏng `flow_schema`. Một việc theo sau phải thêm lại field, mục `input_keys` và mục treatment cùng lúc.
- **Input tuỳ chọn vắng mặt (D2).** `null` được lưu cho một input tuỳ chọn mà không ai post (không `required`, không `defaults`, không `autoSelectFirst`), như Kestra. Không có nó, một Subflow đọc `{{ inputs.x }}` hỏng dưới renderer chặt (`valids/subflow-nullable-input-parent`). Input có default và input bị vô hiệu vẫn bị bỏ.
- **`dependsOn` và default (D6).** Một input bị `dependsOn` vô hiệu bị bỏ khỏi object đã lưu, nhưng render vẫn điền default đã khai báo của nó (Kestra bỏ nó). Khác biệt đã ghi; sửa cần một marker trong object đã lưu hoặc để render chạy lại `dependsOn`.
- **`autoSelectFirst` (D13).** Option đầu tiên được lưu làm giá trị, một ngoại lệ của D2 (default đã khai báo vẫn không được lưu), vì `render.rs` không biết gì về `autoSelectFirst`.
- **Start từ Flow-trigger (D7).** Được nhận diện bằng `flow_trigger_event_id` (migration 0157 đặt nó khi và chỉ khi trigger là một bản ghi Flow); không thêm field mới. `born_failed` cũng bị bỏ qua, thêm một lớp bảo hiểm.
- **Ngày, giờ, duration (D4).** DATETIME, DATE, TIME và DURATION được parse và kiểm biên, và **string đã post** được lưu (`P1D`, `10:00:00`, offset được giữ). INT, FLOAT và BOOL được lưu có kiểu.
- **Finding lúc lưu (D11).** Năm mã, trong `save_only_findings`, nên chúng tới save và publish nhưng **không** tới run gate (một version đã publish vẫn chạy): `input_depends_on_unknown`, `input_depends_on_cycle`, `input_defaults_with_prefill`, `input_file_defaults_invalid`, `input_validator_invalid`. `fancy-regex` từ chối `\` như Java, nên `invalids/inputs-bad-validator-syntax` được bank ở `Seam::Save`.
- **Seam của oracle (D8).** `Seam::Start` được thêm vào oracle: một start bị từ chối với `InvalidInput` là `Ran::StartRefused`, và 422 golden được so như multiset các dòng. Nó bank `invalids/inputs-with-multiple-constraint-violations`.
- **Ratchet.** Đo được **301 / 50 / 50 / 0**, `(301, 251, 50)` (§4).
- **Khác.** Một default literal được kiểm lúc start nhưng không lưu. Việc kiểm `values` của SELECT bị bỏ qua khi không có `values` và không có `expression`. Key không khai báo bị bỏ trên mọi đường đã kiểm, nên sáu test sẵn có dựa vào chúng đã được đổi. Client được sinh lại với một `node_modules` mượn; chỉ hash `spec` đổi.
