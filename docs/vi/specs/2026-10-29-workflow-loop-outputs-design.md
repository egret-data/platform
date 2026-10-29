> **Bản dịch tham khảo.** Bản tiếng Anh `docs/en/specs/2026-10-29-workflow-loop-outputs-design.md` là bản chuẩn; khi hai bản khác nhau, bản tiếng Anh thắng. Bản này đi theo bản tiếng Anh từng mục một. Mã, tên khoá, mã lỗi, đường dẫn và các câu thông báo người dùng thấy được giữ nguyên tiếng Anh.

# Gói workflow #6 "`outputs` của `Loop`, `retry` của `Loop` và outputs cấp flow"

**Trạng thái:** thiết kế ngày 2026-10-29; chưa xây. **Một PR** (quyết định Q1 của người dùng), nhánh `feat/workflow-loop-outputs`, các Task T0, A1–A6, B1–B4 và task tài liệu Z của kế hoạch. Trạng thái như-đã-xây được ghi vào §7 khi PR vào main. Dòng #6 và ghi chú 6 của lộ trình `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`. Kế hoạch: `docs/en/plans/2026-10-29-workflow-loop-outputs.md`. Khảo sát (file:line cho mọi khẳng định, các tiền đề đã đo): `docs/en/specs/2026-10-29-workflow-loop-outputs-survey.md`. Kestra là bản checkout `data-portal/kestra`, `96893b58`; engine của oracle executor là image ghim ở `oracle-exec/docker-compose.yml:31` (`v2.0.0-rc13`), mà `javap` cho thấy có cùng API `Loop`.

Viết tắt như trong khảo sát: `EW` = `crates/egret-workflow/`, `OE` = `EW oracle-exec/`, `KX` = `kestra/executor/src/main/java/io/kestra/executor/`, `KL` = `kestra/core/src/main/java/io/kestra/plugin/core/flow/Loop.java`.

## 1. Vấn đề

Ba thứ một flow đã lưu khai báo được parse, được lưu và không bao giờ chạy:

1. **`outputs:` và `fetchType:` của một `Loop`.** Kestra tính các outputs một lần cho mỗi vòng lặp, khi sub-execution của vòng đó kết thúc, và gom chúng thành một danh sách trên run của `Loop` (`{{ outputs.loop.outputs }}`). egret không mô hình hoá khoá nào trong hai khoá (`EW src/models/task.rs:534-559`), cảnh báo `not_honoured` nêu tên #6 (`EW src/spec/keys.rs:167`), và không ghi output nào trên run của `Loop`, nên `{{ outputs.loop.outputs }}` là undefined và thất bại dưới renderer nghiêm ngặt.
2. **`retry:` của một `Loop`.** Kestra cho một task thất bại bên trong vòng lặp dùng `retry:` của tổ tiên gần nhất, đi ngược cây task của flow cha lên qua `Loop` (`KX ExecutorService.java:576-599`, `:841-856`). `spawn::sub_flow_source` của egret (`EW src/worker/spawn.rs:830-899`) bỏ `retry:` của `Loop` khi dựng flow của vòng lặp, và `retry::policy_for` (`EW src/executor/retry.rs:156-190`) dừng ở gốc của sub-execution. Không có cảnh báo nào nói điều đó.
3. **`outputs:` cấp flow.** Kestra tính chúng khi execution kết thúc (`KX ExecutorService.java:441-477`), trước khi `afterExecution:` chạy, và một lỗi biến execution thành `FAILED`. egret kiểm tra chúng (`EW src/spec/validate.rs:1088-1096`) và không bao giờ tính: `execution.outputs` là hằng `{}` (`EW src/worker/render.rs:279-284`), `outputs.<sub>.outputs` của một `Subflow` cũng vậy (`EW src/worker/subflow.rs:774-779`), và `workflow.executions` không có cột `outputs`. Không có cảnh báo nào nói điều đó.

Số bank (quy nguyên nhân, đo bởi T0 và từng task): **8** entry executor là cận trên, 6 chắc chắn, từ **301 / 35 / 35 / 0** sau merge xuống **301 / 27 / 27 / 0** (chắc chắn **29 / 29**). Trong các entry gói này chạm tới, chỉ `subflow-secret-output-child` và `-parent` ở lại.

## 2. Quyết định

### 2.1 Quyết định của người dùng, 2026-10-29 (đã hỏi, đã trả lời)

- **Q1. `outputs:` cấp flow thuộc #6, trong cùng PR.** Không tách thành #6a/#6b. PR mang theo migration (D11). Các chữ cái task #6a/#6b của khảo sát trở thành A (Loop) và B (flow) trong một kế hoạch.
- **Q2. `retry-flowable-parallel` thuộc #6.** `retry:` của chính một `Loop` (và của một tổ tiên) được các task của mỗi vòng lặp thừa kế (D7).
- **Q3. Outputs SECRET được mã hoá bằng khoá của egret và bị che ở mọi nơi**, như input SECRET (KI-112, `EW src/secret_inputs.rs`; D9). `subflow-secret-output-child` và `-parent` vẫn nằm trong baseline: Kestra làm chúng thất bại chỉ vì harness oracle của nó không có khoá mã hoá, một khác biệt của harness, không phải lỗ hổng của egret.
- **Q4. Một output kiểu FILE, hoặc một URI STORE, trỏ ra ngoài bộ lưu trữ của chính execution thì bị từ chối**, ghi nhận là một khác biệt bảo mật (Kestra OSS không kiểm tra gì; D10, §6).
- **Q5. Outputs tích luỹ của `Loop` có trần:** 1 MiB cho map của mỗi vòng lặp và 16 MiB cho cả danh sách. Vượt một trong hai, vòng lặp thất bại với thông báo chỉ tới `fetchType: STORE` (D4). Kestra không có trần; ghi nhận là giới hạn riêng của egret (§6).

### 2.2 Sửa các tiền đề của lộ trình

Đã đo trong khảo sát (§1, §2, §7) và ràng buộc thiết kế này:

1. **"`outputs:` của một `Loop` được tính khi nó kết thúc" là sai.** Chúng được tính **theo từng vòng lặp**, khi sub-execution của mỗi vòng kết thúc (sau `errors:`/`finally:` của nó), trong ngữ cảnh render của sub-execution đó, và mỗi kết quả được nối vào một danh sách trên run của `Loop` (khảo sát §2 mục 1, 5). Không có lần tính nào "ở cuối `Loop`".
2. **"STORE cần store của W9a" đã cũ.** Bộ lưu trữ nội bộ và file ION của W9a đã có; `loop-values-from-uri` đã khớp (`values:` của `Loop` đọc từ URI chạy được). STORE chỉ cần ghi một file ION cho mỗi vòng lặp (D5).
3. **"Banks: 3 (+ tối đa 4)" là sai. Số bank là 8:** 5 entry outputs của `Loop` (`loop-outputs`, `loop-outputs-failed-render`, `loop-outputs-store`, `loop-outputs-auto`, `if-in-parallel`; STORE và AUTO được mở lại ngày 2026-10-19), 1 entry retry (`retry-flowable-parallel`) và 2 entry outputs của flow (`after-execution`, `flow-with-outputs-failed`).
4. **"4 entry `Loop` chưa quy nguyên nhân" đã được gọi tên, và không entry nào do outputs của `Loop`:**
   - `if-in-flowable`: toán tử `equals`, đã bank bởi #5a (Task A6);
   - `loop-object` và `loop-object-in-list`: test `json`, đã bank bởi #5b (Task B2, PR #780);
   - `retry-flowable-parallel`: `retry:` của `Loop` không được các vòng lặp thừa kế, **gói này** (Q2, D7).
5. **Gói #1 bỏ sót hai chỗ bỏ qua im lặng.** "Mọi khoá parse-nhưng-bỏ-qua đều cảnh báo" của nó không để lại cảnh báo `not_honoured` nào cho `outputs:` cấp flow hay cho `retry:` của `Loop`. `EW src/spec/schema/property-treatment.json` đánh dấu cả hai là "honoured" (`Flow.outputs` ở `:17`, `Loop.retry` trong dòng `Loop` ở `:557-575`) vì file đó chỉ đo đường **lưu** (D8 sửa tính trung thực).
6. **"flow `outputs:` … có thể đã cũ một phần (suy luận)" trong danh sách nguyên nhân (lộ trình `:296`) không cũ.** Không chỗ nào tính outputs của flow.
7. **Comment mã đã cũ:** `EW src/worker/render.rs:8-11` (khoảng trống của surface không còn là lý do `loop-outputs-auto` không phân giải được `values:`; W9a Task 11 đã đóng nó) và `EW src/models/task.rs:518-521` ("cố ý bỏ ra … không dùng trước W3").

### 2.3 Quyết định thiết kế

Quyết định của controller theo các quy tắc tự chủ. Mỗi mục nêu mục khảo sát mà nó dựa vào.

- **D1. Nơi tính outputs của một `Loop` (khảo sát §2 mục 1–4).** Trong lần settle kết thúc sub-execution của một vòng lặp, sau khi `errors:`/`finally:` của nó đã chạy, trên render surface của chính sub-execution (outputs task của nó, `item`, outputs cha mà nó thừa kế; surface mà pha outbox của `claim::settle` đã nạp sẵn), và chỉ khi `Loop` khai báo `outputs:` không rỗng. Một lỗi render hay ép kiểu nối `FAILED` vào lịch sử trạng thái của sub-execution **sau** trạng thái kết thúc nó đã đạt (`CREATED RUNNING SUCCESS FAILED`, như golden `loop-outputs-failed-render`), lưu lại, ghi log lỗi trên sub-execution, và báo cáo gửi cho `Loop` cha là `FAILED`. Outputs đã tính đi cùng báo cáo kết thúc vào `spawn::on_execution_terminal` (`EW src/worker/spawn.rs:519-657`), nơi hôm nay chỉ nhận trạng thái.
- **D2. Một bộ tính outputs cho cả hai loại (khảo sát §2 mục 3).** Một hàm dùng chung, cho D1 và D6, port `renderOutputs` + `typedOutputs`: các output **bắt buộc** (`required` vắng mặt hoặc `true`) render cùng nhau và một lỗi làm hỏng tất cả; mỗi output **tuỳ chọn** render riêng, lỗi được ghi log và thành `null`; rồi mỗi giá trị được ép theo `type` bằng bộ ép kiểu input của gói #3 (`EW src/inputs_check.rs`, `parseData` của Kestra); lỗi ép kiểu thì thất bại. Thông báo lỗi nêu id của output. Hàm thuần trên một render surface và một danh sách `FlowOutput`, trả về map đã có kiểu cùng các id SECRET (D9).
- **D3. Danh sách trên run của `Loop` (khảo sát §2 mục 5, §6 D2).** Khi một vòng lặp kết thúc: nếu nó thất bại và `transmitFailed` (mặc định `true`) đúng, `Loop` kết thúc `FAILED` ngay và outputs của vòng đó **không** được nối; ngược lại, nếu map của nó không rỗng, `{item: {value, iteration, key?}, outputs: {…}}` được nối vào danh sách `outputs` của run `Loop`. `item.value` là giá trị của vòng lặp dưới dạng chuỗi, `iteration` là chỉ số từ 0, `key` chỉ có với giá trị là map. Thứ tự là **thứ tự kết thúc** (bằng thứ tự chỉ số khi `concurrencyLimit: 1`). Việc nối diễn ra dưới khoá dòng cha mà `on_execution_terminal` đã lấy (`claim::load(… Stack::Nested)`), nên các vòng lặp đồng thời không bao giờ làm mất một entry.
- **D4. Bộ đếm và trần (khảo sát §2 mục 6, Q5).** Mọi run `Loop`, có hay không có `outputs:`, mang `iterationCount`, `runningIterations` và `terminatedIterations` (map từ tên trạng thái sang số đếm) trong `task_runs.outputs`, cập nhật dưới cùng khoá; `outputs` chỉ có mặt khi đã có một entry được nối, như Kestra. `nextOffset` không được ghi (egret đọc `values:` từ file theo chỉ số). **Trần:** một vòng lặp có map FETCH tuần tự hoá ra hơn 1 MiB (`env::MAX_OUTPUT_BYTES`) thất bại ngay trong settle của chính nó: sub-execution nhận `FAILED` như D1. Một vòng lặp mà việc nối sẽ đẩy danh sách quá 16 MiB được quyết định lúc nối, dưới khoá cha (chỉ cha thấy tổng): vòng lặp được coi là một vòng thất bại (dưới `transmitFailed` thì `Loop` kết thúc `FAILED`; ngược lại nó không được nối và được đếm vào `FAILED` trong `terminatedIterations`), và thông báo được ghi log trên run `Loop`; lịch sử của chính sub-execution không bị viết lại sau khi nó đã kết thúc. Thông báo là `Loop outputs of iteration <i> are <n> bytes; egret keeps at most 1 MiB per iteration and 16 MiB per Loop in the execution. Use fetchType: STORE to keep them in internal storage.` (biến thể 16 MiB nêu tổng). Các giới hạn là hằng số đặt cạnh `MAX_OUTPUT_BYTES`.
- **D5. `fetchType` (khảo sát §2 mục 7).** `FETCH` giữ map. `STORE` ghi map đã có kiểu thành **một file ION cho mỗi vòng lặp** bằng `runners::ionfmt::write_ion_element`, dưới prefix lưu trữ của **sub-execution** (như `runContext.storage().putFile` của Kestra), được đánh chỉ mục như file output của một task để quy tắc đọc được (`storage::index::resolve_readable_many`) và bộ dọn (pruner) biết đến nó, và thay map bằng `{uri: "kestra:///…"}`. `AUTO` (mặc định) là STORE khi `values:` là một **chuỗi** render ra một URI `kestra://` mà bộ lưu trữ của egret đọc được (`guessOutputMode` của Kestra), FETCH trong trường hợp khác; quyết định được lấy một lần cho mỗi run `Loop` từ `values:` đã render, không theo từng vòng. Trần 1 MiB / 16 MiB của D4 cũng áp cho danh sách các entry `{uri}` (rất xa mới chạm). Output SECRET dưới STORE: xem D9.
- **D6. `outputs:` cấp flow (khảo sát §2 mục 8, ghi chú 6).** Tính bằng D2 khi một execution đạt trạng thái kết thúc, **trước** khi `afterExecution:` được lên lịch (để `{{ execution.outputs.x }}` render được trong `afterExecution:`), và chỉ khi flow khai báo `outputs:`. Lưu vào cột mới `workflow.executions.outputs` (D11). Một lỗi nối `FAILED` sau trạng thái kết thúc đã tính (`CREATED RUNNING SUCCESS FAILED`, như golden `flow-with-outputs-failed`) và lỗi được ghi log trên execution. Giá trị xuất hiện ở:
  - `{{ execution.outputs }}` trong mọi lần render sau đó của execution (`afterExecution:`; `worker/render.rs:279-284` đọc cột);
  - `{{ outputs.<subflow>.outputs }}` của flow cha (`worker/subflow.rs:774-779` chép outputs của con lên task run `Subflow` khi con kết thúc, chỉ khi `wait: true`, như Kestra);
  - `{{ trigger.outputs }}` của một Flow trigger (outputs của execution kích hoạt, như Kestra);
  - API execution (`GET` một execution và danh sách mang `outputs`; cập nhật OpenAPI).
  **Bên trong một vòng lặp `Loop`, `outputs:` của flow cha không được tính:** `sub_flow_source` xoá `outputs` trên flow của vòng lặp. Mã nguồn Kestra tính outputs của cha trong mọi vòng lặp (`onEnd` dùng flow cha); egret coi đó là bug của Kestra (quy tắc lộ trình #20, tiền lệ `finally:` của `WorkingDirectory`) và ghi nhận nó (§6). Không flow nào trong corpus có cả hai, nên không bank nào dịch chuyển theo chiều nào.
- **D7. Thừa kế retry của `Loop` (khảo sát §1, Q2).** Khi `sub_flow_source` dựng flow của một vòng lặp, nó tìm `retry:` gần nhất trên `Loop` hoặc trên một tổ tiên của nó trong cây task của flow **cha** (`searchForParentTaskWithRetry` trên `Flow.findParentTasksByTaskId` của Kestra) và gắn vào vòng lặp như một **chính sách thừa kế**, một trường mới khác với `retry` cấp flow để hai thứ giữ nghĩa riêng. `retry::policy_for` tra nó sau chuỗi run cha và trước `plan.retry`: `retry:` của chính task > tổ tiên bên trong vòng lặp > thừa kế từ flow cha > `retry:` của chính flow, đúng thứ tự ưu tiên của Kestra. Nó áp cho task chạy được và `Subflow`, như Kestra (run của chính `Loop` không bao giờ retry). Một chính sách `CREATE_NEW_EXECUTION` đạt tới bằng đường này vẫn bị `worker::replay` từ chối lúc chạy, vì Kestra cũng ném lỗi ở đó (`OE baseline.txt:352-356`). **Báo cáo kết thúc (khảo sát R3):** một `FAILED` theo ngay sau là `RETRYING` trong cùng một settle không được báo trạng thái kết thúc cho `Loop` cha; hôm nay chỉ trạng thái mà settle kết thúc ở đó được báo (`EW src/worker/claim.rs:3509-3516`), và A6 ghim điều đó bằng một test trước khi dựa vào nó.
- **D8. Cảnh báo lúc lưu và tính trung thực của property-treatment (khảo sát §1, §3; Q1, Q2).** Vì Q1 đặt mọi thứ trong một PR, không có cảnh báo nào được ship mà chính PR đó gỡ đi:
  - cảnh báo `not_honoured` cho `outputs`/`fetchType` của `Loop` (`keys.rs:167`, test của nó ở `:1769`) bị gỡ;
  - không thêm cảnh báo cho `outputs:` cấp flow hay `retry:` của `Loop`; cả hai đều chạy khi PR này vào;
  - một `Loop` khai báo `outputs:` nhận kiểm tra lúc lưu như outputs của flow đã có (mỗi output có `type`; id không trùng; dùng lại `validate.rs:1088-1096`);
  - một output SECRET trên `Loop` có `fetchType` là `STORE`, hoặc `AUTO` với `values:` là chuỗi, bị từ chối lúc lưu (D9);
  - `property-treatment.json`: `Loop.outputs` và `Loop.fetchType` đổi từ "ignored" sang "honoured"; `README.md` của schema nói rõ "honoured" ở đó nghĩa là *được lưu và chuyển cho lần chạy*, rằng gói #1 chỉ đo đường lưu, và rằng `Flow.outputs` và `Loop.retry` đã được ghi "honoured" trước khi có gì chạy chúng (sửa bởi #6). Một test hàng rào khẳng định mọi khoá mà file treatment gọi là "honoured" trên dòng `Loop` và trên `Flow` hoặc được egret chạy hoặc nằm trong bảng `not_honoured` của `keys.rs`, với ba khoá của gói này được nêu tên trong đó.
- **D9. Outputs SECRET (Q3).** Cơ chế `secret_inputs` được dùng lại, không sao chép:
  - giá trị được lưu (danh sách của run `Loop`, `workflow.executions.outputs`, bản chép của run `Subflow`) giữ `MASK` (`logs::MASK`) cho mỗi output SECRET, và các giá trị thật được niêm phong, một object JSON mỗi dòng, bằng `egret_secrets::Crypto` dưới `secrets.master_key` (`secret_inputs::CIPHER`) vào một cột `bytea` bên cạnh: `workflow.executions.secret_outputs` và `workflow.task_runs.secret_outputs` (D11). Với danh sách `Loop`, object niêm phong được đánh khoá theo vị trí trong danh sách;
  - chỉ render surface của worker (`worker::render::Surface::load`) mở chúng, nên một `{{ outputs.loop.outputs[0].outputs.s }}` hay `{{ execution.outputs.s }}` về sau render ra giá trị thật; mọi dòng log và thông báo lỗi của lần chạy đều che chúng (`RenderContext::redactions`);
  - không projection API nào chọn các cột niêm phong; `trigger.outputs` của một Flow trigger và flow cha của `Subflow` thấy mặt nạ trừ khi chúng cùng họ execution (cha của `Subflow` chép object niêm phong của con vào `secret_outputs` của task run `Subflow`, như input của con `Subflow` đi qua `split` hôm nay);
  - một triển khai không có master key từ chối lúc tính, nêu tên output và setting, như việc khởi chạy với input SECRET bị từ chối; vòng lặp hoặc execution thất bại;
  - một output SECRET dưới STORE bị từ chối lúc lưu (D8): file ION sẽ giữ giá trị dạng rõ trong bộ lưu trữ mà `read()` trả về dạng văn bản. Khác biệt được ghi nhận (§6): Kestra ghi dạng đã mã hoá vào file;
  - `subflow-secret-output-child` và `-parent` vẫn trong baseline, quy lại nguyên nhân trong `OE baseline.txt` là khác biệt harness (oracle của Kestra không có khoá nên Kestra FAILED; egret có khoá thì thành công).
- **D10. Output FILE và URI STORE ở trong bộ lưu trữ của chính execution (Q4).** Giá trị của một output `type: FILE` (flow hoặc `Loop`), sau khi render, phải là một URI `kestra:///` có `{exec}` là **chính execution đó hoặc một execution khác trong họ loop của nó** (gốc loop và các vòng lặp của nó, `loop_family` của `storage::index`), và phân giải là đọc được (`resolve_readable_many`). Mọi thứ khác (một execution khác của chuỗi replay hoặc tập trigger, namespace khác, URI sai dạng, đường dẫn không phải `kestra://`) làm output thất bại với `egret refuses output <id>: <uri> is not a file of this execution`. URI STORE do chính egret ghi (D5) theo cấu tạo nằm trong prefix của sub-execution. Một URI đọc lại từ danh sách STORE của `Loop` đi qua quyết định đọc được hiện có, không đổi. Quy tắc này hẹp hơn quy tắc đọc được, vốn cũng cho phép chuỗi replay và họ trigger: một output công bố một file, còn một lần đọc chỉ tiêu thụ nó.
- **D11. Một migration.** `workflow.executions` thêm `outputs jsonb` (NULL = chưa tính outputs) và `secret_outputs bytea`; `workflow.task_runs` thêm `secret_outputs bytea`. Comment như trong `0203`. Không backfill: các execution kết thúc trước migration chưa từng được tính outputs. **Số:** số trống tiếp theo trên số cao nhất của main **và** trên migration của mọi PR đang mở. Đo ngày 2026-10-29 trên ref cục bộ (không có mạng): số cao nhất trên `origin/main` và trên mọi nhánh cục bộ và remote-tracking là `0210_workflow_execution_is_idle.sql`, nên file là `migrations/0211_workflow_execution_outputs.sql`. Số này là tạm thời (D12). T0 kiểm tra lại sau khi merge main (2026-10-29, ref cục bộ, không có mạng): vẫn là `0210` ở mọi nơi, `0211` còn trống.
- **D12. Kiểm tra lại số migration lúc merge.** Không gian số dùng chung với mọi nhánh đang mở (`no_two_migration_files_share_a_version` chỉ bắt được va chạm khi cả hai file nằm trên một nhánh, và một va chạm trên database đã từng thấy main làm `migrate()` dừng với `VersionMismatch`, nên không cột nào của PR này tồn tại). Ngay trước khi merge PR (có mạng): `git fetch origin`, `git ls-tree --name-only origin/main migrations/ | tail -1`, và với mỗi PR đang mở (`gh pr list --state open --json number,headRefName`), `git ls-tree --name-only origin/<head> migrations/ | tail -1`. Nếu có số `0211` trở lên, đổi số bằng `git mv` thành một số trên số cao nhất, rồi quét ba quần thể: tên file, số trần (comment, tài liệu, spec này) và mọi identifier đặt theo số. Merge main vào nhánh sau khi đổi số và chạy lại các test migration.
- **D13. Thay đổi hành vi với flow đã lưu (ghi chú phát hành).** Các câu trả lời sau thay đổi: `{{ outputs.<loop>.outputs }}` và các bộ đếm tồn tại trên run `Loop`; một `Loop` có biểu thức output lỗi giờ làm vòng lặp thất bại (và `Loop`, dưới `transmitFailed`); `retry:` của một `Loop` (hoặc của tổ tiên) giờ retry các task của mỗi vòng lặp; một flow có output cấp flow lỗi giờ kết thúc `FAILED`; `{{ execution.outputs }}` và `outputs.<id>.outputs` của `Subflow` mang giá trị thay vì `{}`; API execution mang `outputs`; output FILE ngoài bộ lưu trữ của execution, output SECRET dưới STORE và outputs `Loop` vượt trần bị từ chối (riêng egret). Văn bản đi vào repo tài liệu và mô tả PR (kế hoạch Z).

## 3. Hình dạng thay đổi

- **Mô hình và lưu:** `models/task.rs` (`Loop.outputs: Vec<FlowOutput>`, `fetch_type: FetchType`), `spec/keys.rs`, `spec/validate.rs`, `spec/schema/property-treatment.json` và README của nó.
- **Bộ tính:** module mới `outputs.rs` (D2), trên bộ ép kiểu của `inputs_check` và niêm phong của `secret_inputs`.
- **Đường Loop:** `worker/claim.rs` (pha outbox của settle: D1), `worker/spawn.rs` (`sub_flow_source`: xoá theo D6, retry thừa kế theo D7; `on_execution_terminal`: D3, D4), `executor/retry.rs` (D7), ghi lưu trữ cho D5.
- **Đường flow:** chuyển trạng thái kết thúc trong executor/worker (D6), `worker/render.rs` (`execution.outputs`, Surface mở niêm phong), `worker/subflow.rs`, ngữ cảnh render của Flow trigger, `repo.rs` và `routes.rs` (API), OpenAPI và client sinh ra của frontend (`npm run gen:api`, không bao giờ orval trần) nếu trường được lộ ra ở đó.
- **Migration** `0211` (tạm thời, D12).

## 4. Ratchet và test

Đo bằng `cargo test -p egret-workflow --test executor` (golden / mismatching / baselined / broken). Trên gốc của nhánh này (`6dc1dcf9`, #5a) là **301 / 40 / 40 / 0**; sau khi T0 merge main có #5b (#780) dự kiến là **301 / 35 / 35 / 0** (T0 đo). **T0 đo ngày 2026-10-29 trên commit merge `271b1c17`: 301 / 35 / 35 / 0, census 295 / 25 / 21**, đúng như dự kiến; các trường khác nhau của từng entry nằm trong ghi chú trạng thái T0 của plan. Census `LIVE` 295 / `VALIDATE_REFUSED` 25 / `UNKNOWN_TYPE_REFUSED` 21, dự kiến không đổi (không task nào đổi việc kiểm tra một flow corpus; T0 và Z đo lại).

| Task | Entry được bank | Ratchet sau (cận trên / chắc chắn) |
|---|---|---|
| T0 | không (merge, đo) | 35 / 35 |
| A1, A2 | không | 35 / 35 |
| A3 | `loop-outputs-failed-render` | 34 / 34 |
| A4 | `loop-outputs`, `if-in-parallel` | 32 / 32 |
| A5 | `loop-outputs-store`; `loop-outputs-auto` có thể | 30 / 31 |
| A6 | `retry-flowable-parallel` có thể | 29 / 31 |
| B1 | `flow-with-outputs-failed` | 28 / 30 |
| B2 | `after-execution` | **27 / 29** |
| B3, B4 | không | 27 / 29 |

"Chắc chắn" chỉ đếm các entry có độ tin cậy cao trong khảo sát; một entry "có thể" không dịch chuyển giữ dòng baseline của nó, quy lại theo nguyên nhân task của nó đo được. Một lần bank gỡ dòng của nó trong `OE baseline.txt` trong cùng commit (test thất bại nếu còn dòng chưa gỡ).

**Không oracle nào thấy outputs** (khảo sát R1): `golden.json` và `traces.json` chỉ ghi trạng thái. Mọi khẳng định về nội dung (hình dạng item, thứ tự, kiểu, URI STORE, bộ đếm, che) được ghim bằng unit test và integration test viết theo khảo sát §2, liệt kê theo task trong kế hoạch. `working-directory-loop` (ba `outputs:` `Loop` lồng nhau, không có golden) được dùng làm fixture integration.

## 5. Việc tiếp theo được ghi nhận

- `nextOffset` nếu egret có lúc đọc `values:` từ file theo trang (D4).
- Hiển thị outputs trong UI execution (API mang chúng sau B3; không có màn hình trong PR này).
- Output SECRET dưới STORE (ION mã hoá), nếu được yêu cầu (D9).
- `subflow-secret-output-*`: một harness có khoá phía Kestra sẽ bank chúng.
- Kestra tính `outputs:` của flow cha trong mọi vòng lặp `Loop` (D6): xem lại nếu Kestra thay đổi.

## 6. Khác biệt được ghi nhận

1. **`outputs:` của flow cha không được tính bên trong một vòng lặp `Loop`** (D6). Mã nguồn Kestra có tính. Không có entry corpus.
2. **Output FILE và URI STORE ngoài họ loop của execution bị từ chối** (D10, Q4, bảo mật). Kestra OSS chấp nhận mọi URI.
3. **Outputs của `Loop` có trần** 1 MiB mỗi vòng lặp và 16 MiB mỗi `Loop` (D4, Q5). Kestra không có trần.
4. **Một output SECRET dưới STORE bị từ chối lúc lưu** (D9). Kestra ghi dạng đã mã hoá.
5. **Một triển khai không có master key làm output SECRET thất bại** (D9), như nó từ chối khởi chạy với input SECRET. Kestra cũng thất bại (lý do `subflow-secret-output-*` vẫn trong baseline).
6. **Thứ tự của các vòng lặp đồng thời** là thứ tự kết thúc của egret; của Kestra cũng không tất định (khảo sát R5). Không phải khác biệt ai đo được; ghi lại để người đọc không ghim nó.

## 7. Như đã xây

Do task tài liệu Z viết khi PR sẵn sàng: số bank đo được, gốc của T0, số migration thực dùng và thời điểm kiểm tra lại, và mọi quyết định mà một task đã sửa.
