> **Bản dịch tham khảo.** Bản tiếng Anh `docs/en/plans/2026-10-29-workflow-loop-outputs.md` là bản chuẩn; khi hai bản khác nhau, bản tiếng Anh thắng. Bản này đi theo bản tiếng Anh từng mục một. Mã, tên khoá, mã lỗi, đường dẫn, cờ lệnh, thông báo người dùng thấy được và các khối mã giữ nguyên tiếng Anh.

# Gói workflow #6 "`outputs` của `Loop`, `retry` của `Loop` và outputs cấp flow" — kế hoạch triển khai

Spec: `docs/en/specs/2026-10-29-workflow-loop-outputs-design.md` (Q1–Q5, D1–D13). Khảo sát: `docs/en/specs/2026-10-29-workflow-loop-outputs-survey.md`. Nhánh `feat/workflow-loop-outputs`. **Một PR** (Q1): T0, A1–A6 (`Loop`), B1–B4 (outputs của flow), Z (tài liệu và cổng kiểm tra).

**Trạng thái, 2026-10-29:** đã thiết kế, chưa xây.

**T0 đã đo, 2026-10-29.** Main (`f623f65f`, #5b, PR #780) đã được merge vào nhánh (`271b1c17`); merge không cần giải xung đột trong `OE baseline.txt`. `cargo test -p egret-workflow --test executor` trên `271b1c17`: ratchet **301 / 35 / 35 / 0** (golden / mismatching / baselined / broken), census `LIVE` **295** / `VALIDATE_REFUSED` **25** / `UNKNOWN_TYPE_REFUSED` **21** (`[publish_corpus] 351 corpus flows: 295 live, 46 draft-only (21 unported task type, 25 egret's own validate), 6 unparseable, 4 name-contested`). Cả hai đúng như dự kiến, nên phần "Accept" của mọi task sau giữ nguyên như đã viết. Migration: file cao nhất trên mọi nhánh cục bộ và ref remote-tracking (kể cả `origin/main`) là `0210_workflow_execution_is_idle.sql`, và không nhánh nào thêm file trên số đó, nên `0211` còn trống (spec D11; kiểm tra lại có mạng trước khi merge, D12). Các trường khác nhau, đọc từ trace của chính oracle (golden so với egret):

| entry | khác ở | golden | egret |
|---|---|---|---|
| `loop-outputs` | `runs`, `execution` | `end` SUCCESS, execution SUCCESS | `end` FAILED, execution FAILED; `loop` SUCCESS và 3 sub-execution khớp |
| `loop-outputs-store` | `runs`, `execution` | như trên | như trên |
| `loop-outputs-auto` | `runs`, `execution` | `write`, `loop`, `end` SUCCESS | chỉ `end` FAILED; `write` và `loop` SUCCESS và 3 sub-execution (values đọc từ URI của file) khớp, nên chỉ có `{{outputs.loop.outputs}}` của `end` giữ entry này |
| `loop-outputs-failed-render` | `spawned`, `runs`, `execution` | 1 sub-execution `CREATED RUNNING SUCCESS FAILED`; `loop` FAILED; execution FAILED | 3 sub-execution `CREATED RUNNING SUCCESS`; `loop` SUCCESS; execution SUCCESS |
| `if-in-parallel` | `runs`, `execution` | `assert` SUCCESS | `assert` FAILED, execution FAILED; `for_each` SUCCESS |
| `retry-flowable-parallel` | chỉ `spawned` | mỗi sub-execution trong 2 cái là `CREATED RUNNING FAILED RETRYING RUNNING FAILED RETRYING RUNNING FAILED`, `child` 3 attempt | mỗi cái `CREATED RUNNING FAILED`, `child` 1 attempt |
| `after-execution` | chỉ `runs` | `end` (task của `afterExecution:`) SUCCESS | `end` FAILED; execution SUCCESS ở cả hai phía |
| `flow-with-outputs-failed` | chỉ `execution` | `CREATED RUNNING SUCCESS FAILED` | `CREATED RUNNING SUCCESS`; `return` khớp |

**Review không phải là một task của kế hoạch này.** Controller chạy một lần review toàn nhánh sau Z; các sửa chữa của nó được ghi vào spec §7 như một đợt sửa.

## Ràng buộc chung

Các số Q và D của spec là ràng buộc. Mã nguồn Kestra: `/Users/1pro-l4r8/Documents/github/data-portal/kestra`. Conventional Commits; trailer `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`; subagent không dùng mạng (không `git push/fetch/pull`, không `gh`); không bao giờ chạm cổng 5432 hay container `egret-dev-*`/`egret-demo-*`; scratch và `.superpowers/` không bao giờ vào git. Build với `CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-loopout/target`; DB test `DATABASE_URL=postgres://egret:egret@localhost:5464/egret_test`, shared store `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6464`; không bao giờ chạy hai lệnh cargo cùng lúc; đĩa đang chật (khoảng 5 GiB trống lúc thiết kế): chạy `df -h` trước một lần build đầy đủ và dọn `target/debug/incremental` thay vì mở một target dir thứ hai.

**Đo đạc.** Ratchet executor (golden / mismatching / baselined / broken) từ `cargo test -p egret-workflow --test executor`: **301 / 40 / 40 / 0** trên gốc của nhánh này (`6dc1dcf9`), dự kiến **301 / 35 / 35 / 0** sau T0 (#5b, PR #780, đã merge vào main). Census `LIVE` 295 / `VALIDATE_REFUSED` 25 / `UNKNOWN_TYPE_REFUSED` 21. Mọi "Chấp nhận" dưới đây tính tương đối so với **số đo của T0**: nếu T0 không ra 35, mọi số sau dịch đi đúng khoảng chênh đó, và báo cáo nói rõ. Một task bank một entry thì gỡ dòng `OE baseline.txt` của nó trong cùng commit, kèm comment có ngày nêu nguyên nhân; một bank dự đoán mà không dịch chuyển được báo cáo kèm nguyên nhân đo được và dòng của nó được quy lại nguyên nhân, không xoá. Mọi task báo cáo số đo, không phải số dự đoán.

**Không oracle nào thấy outputs** (spec §4). Mọi task ghi outputs đều ghim nội dung của chúng bằng unit test hoặc integration test theo khảo sát §2; chỉ một lần bank trạng thái thì không đủ để chấp nhận về nội dung.

**Thứ tự trong một task.** Đỏ trước: test cho thấy khoảng trống tồn tại có trước bản sửa.

## T0. Merge main và đo

- `git merge main` (`main` cục bộ phải ở commit có #780 trước; không có mạng thì merge ref cục bộ mà controller chỉ định, và báo SHA của nó). Giải xung đột trong `OE baseline.txt` bằng cách giữ các dòng bị gỡ của cả hai phía.
- Chạy ratchet executor và census. Với `loop-outputs`, `loop-outputs-store`, `loop-outputs-auto`, `if-in-parallel`, `retry-flowable-parallel`, `after-execution`, `flow-with-outputs-failed`, ghi trường khác biệt chính xác từ bản in của test (task nào, trạng thái nào), để A3–B2 phân biệt được một lần bank với một sự trùng hợp. Với `loop-outputs-auto`, chỉ ra rằng không có gì ngoài outputs giữ nó (khảo sát §4, độ tin cậy trung bình).
- Đọc lại migration trên mọi ref cục bộ (`for b in $(git for-each-ref --format='%(refname:short)' refs/heads refs/remotes); do git ls-tree --name-only $b migrations/ | tail -1; done | sort | tail -1`) và xác nhận `0211` vẫn trống (spec D11).
- **Chấp nhận:** một merge commit; ratchet **301 / 35 / 35 / 0** (hoặc bộ số đo được, bộ này trở thành gốc của mọi task sau); census 295 / 25 / 21; bảy trường khác biệt trong báo cáo.

## A. Outputs và retry của `Loop`

- **A1. Mô hình và lưu (spec D8).** `models::Loop` thêm `outputs: Vec<FlowOutput>` (dùng lại kiểu của flow) và `fetch_type` (mặc định `AUTO`, `FETCH`, `STORE`); sửa comment cũ `models/task.rs:518-521`. Lưu: các kiểm tra output của flow (`validate.rs:1088-1096`) cũng chạy trên `outputs:` của `Loop`; một output SECRET dưới `STORE` hoặc dưới `AUTO` với `values:` là chuỗi bị từ chối với thông báo nêu tên output và `fetchType`. Gỡ nhánh `keys.rs:167` và viết lại test của nó (`:1769`) để khẳng định không có cảnh báo. `property-treatment.json`: `Loop.outputs`/`fetchType` là "honoured"; đoạn trong `README.md` của schema về điều "honoured" đo, và hai khoá mà gói #1 đã ghi trước khi chúng chạy; test hàng rào của D8. Kiểm tra xem `retry:` của `Loop` (hoặc tổ tiên) với `behavior: CREATE_NEW_EXECUTION` có được lưu im lặng không; nếu có, thêm một cảnh báo kiểu `not_honoured` nêu việc từ chối lúc chạy (D7).
  - **Test:** một `Loop` có `outputs:` parse được và round-trip; thiếu `type` bị từ chối; id trùng bị từ chối; SECRET + STORE bị từ chối; SECRET + FETCH được chấp nhận; không có cảnh báo cho `outputs`/`fetchType`; test hàng rào thất bại nếu `Flow.outputs` bị gỡ khỏi danh sách của nó.
  - **Chấp nhận:** ratchet không đổi (của T0, dự kiến 35 / 35); census không đổi; `--lib` xanh.
- **A2. Bộ tính dùng chung (spec D2).** Mới `EW src/outputs.rs`: `evaluate(surface, &[FlowOutput]) -> Result<Typed { values, secret_ids }, OutputError>`. Bắt buộc thì cùng nhau, tuỳ chọn thì từng cái (lỗi ghi log, `null`), rồi bộ ép kiểu của `inputs_check` theo `type`.
  - **Test (theo khảo sát §2 mục 3–4):** hai output bắt buộc, một lỗi → lỗi nêu tên nó, không trả gì; một output tuỳ chọn lỗi → `null` và một dòng log, các cái khác có kiểu; INT từ `"3"` ép được; INT từ `"x"` thất bại với văn bản của bộ ép kiểu; JSON, BOOLEAN, DATETIME, FILE (chỉ hình dạng ở đây), id SECRET được báo; danh sách rỗng trả map rỗng mà không render.
  - **Chấp nhận:** ratchet không đổi; unit test xanh.
- **A3. Tính ở trạng thái kết thúc của vòng lặp (spec D1).** Trong pha outbox của `claim::settle` cho một sub-execution LOOP đã đạt trạng thái kết thúc và có `Loop` khai báo `outputs:`: tính (A2) trên surface của sub-execution sau `errors:`/`finally:` của nó; khi lỗi thì nối `FAILED` sau trạng thái kết thúc, lưu, ghi log, và báo `FAILED`; mang map tới `on_execution_terminal` (một trường mới trên báo cáo kết thúc, `None` khi không có outputs). Trần 1 MiB mỗi vòng lặp ở đây (D4). Sửa `render.rs:8-11`.
  - **Test:** lịch sử trạng thái `CREATED RUNNING SUCCESS FAILED` khi render lỗi; `transmitFailed: true` chặn các vòng lặp sau (1 sub-execution); `transmitFailed: false` để các vòng khác chạy; outputs của `errors:`/`finally:` nhìn thấy được khi tính; trần 1 MiB thất bại với thông báo nêu `fetchType: STORE`.
  - **Chấp nhận:** bank `valids/loop-outputs-failed-render`: **301 / 34 / 34 / 0**.
- **A4. Danh sách, bộ đếm và trần tổng (spec D3, D4).** Trong `on_execution_terminal`, dưới khoá cha: cập nhật `iterationCount`, `runningIterations`, `terminatedIterations` trên mọi run `Loop`; nối `{item: {value, iteration, key?}, outputs}` khi map không rỗng và vòng lặp không phải một thất bại dưới `transmitFailed`; kiểm tra tổng 16 MiB coi một vòng lặp vượt trần là thất bại.
  - **Test:** hình dạng một entry cho giá trị list và cho giá trị map (`key` chỉ có ở đó); thứ tự kết thúc với `concurrencyLimit: 1` bằng thứ tự chỉ số; hai vòng lặp đồng thời đều được nối (không mất cập nhật); `outputs` vắng khi chưa có gì được nối; bộ đếm sau 3 vòng lặp với một FAILED dưới `transmitFailed: false`; kiểm tra 16 MiB (với giới hạn hạ xuống qua một hằng chỉ dùng cho test, không phải 16 MiB dữ liệu); `working-directory-loop` render được ba danh sách lồng nhau của nó.
  - **Chấp nhận:** bank `valids/loop-outputs`, `valids/if-in-parallel`: **301 / 32 / 32 / 0**.
- **A5. STORE và AUTO (spec D5, D10 cho URI đã ghi).** STORE ghi một file ION mỗi vòng lặp dưới prefix của sub-execution bằng `ionfmt::write_ion_element`, được đánh chỉ mục cho quy tắc đọc được và pruner; `outputs` của entry là `{uri}`. AUTO quyết định một lần cho mỗi run `Loop` từ `values:` đã render.
  - **Test:** URI nằm dưới prefix của sub-execution và `read()` nó trong một task sau trả về map đã có kiểu dưới dạng ION; AUTO với `values:` là chuỗi URI → STORE, với list → FETCH, với chuỗi không phải URI → FETCH; pruner bảo vệ file khi execution đang chạy.
  - **Chấp nhận:** bank `valids/loop-outputs-store` (và `valids/loop-outputs-auto` nếu T0 cho thấy không có gì khác giữ nó): **301 / 30 / 30 / 0**, chắc chắn **31 / 31**.
- **A6. Thừa kế retry (spec D7, Q2).** `sub_flow_source` gắn `retry:` gần nhất của `Loop` hoặc tổ tiên của nó trong flow cha làm chính sách thừa kế của vòng lặp; `retry::policy_for` tra nó sau chuỗi run, trước `plan.retry`. Đầu tiên, đỏ: một test rằng một sub-execution có task đi `FAILED` rồi `RETRYING` trong một settle không báo gì cho `Loop` cha (khảo sát R3); nếu nó có báo, sửa điều đó trước khi làm phần thừa kế.
  - **Test:** test R3; thứ tự ưu tiên (của chính task > tổ tiên trong vòng lặp > thừa kế > flow); một `Loop` bên trong một `Sequential` có `retry:` thừa kế của `Sequential`; `maxAttempts: 3` cho ba lần thử và lịch sử `… FAILED RETRYING RUNNING FAILED RETRYING RUNNING FAILED`; một `CREATE_NEW_EXECUTION` thừa kế vẫn bị `worker::replay` từ chối.
  - **Chấp nhận:** bank `valids/retry-flowable-parallel`: **301 / 29 / 29 / 0** (chắc chắn 31 / 31 nếu AUTO của A5 và entry này không dịch chuyển; báo cái nào).

## B. Outputs cấp flow

- **B1. Migration và tính ở trạng thái kết thúc (spec D6, D11).** `migrations/0211_workflow_execution_outputs.sql` (kiểm tra lại số trước, D12; khối comment như trong `0203`): `workflow.executions.outputs jsonb`, `workflow.executions.secret_outputs bytea`, `workflow.task_runs.secret_outputs bytea`. Tính (A2) khi một execution đạt trạng thái kết thúc, trước khi `afterExecution:` được lên lịch; lỗi thì nối `FAILED` và ghi log. `sub_flow_source` xoá `outputs` trên flow của vòng lặp (D6). Chỉ chạy `cargo sqlx prepare` nếu repo dùng dữ liệu query offline cho các query mới.
  - **Test:** lịch sử `CREATED RUNNING SUCCESS FAILED` khi một output lỗi; một execution FAILED có outputs vẫn tính chúng (Kestra tính ở mọi trạng thái kết thúc; đọc `ExecutorService.java:441-477` và ghim điều nó làm với FAILED và KILLED); một vòng lặp không tính outputs của cha; `no_two_migration_files_share_a_version` xanh.
  - **Chấp nhận:** bank `valids/flow-with-outputs-failed`: **301 / 28 / 28 / 0** (chắc chắn 30 / 30).
- **B2. `execution.outputs` trong render (spec D6).** `worker/render.rs:279-284` đọc cột; Surface mở `secret_outputs` (B4 điền nó).
  - **Test:** `afterExecution:` render được `{{ execution.outputs.final }}`; một execution không có `outputs:` render `execution.outputs` thành `{}` như hôm nay.
  - **Chấp nhận:** bank `valids/after-execution`: **301 / 27 / 27 / 0** (chắc chắn 29 / 29).
- **B3. `Subflow`, Flow trigger, API (spec D6).** `subflow.rs:774-779` chép outputs của con (và object niêm phong) lên task run `Subflow` khi con kết thúc (`wait: true`); `trigger.outputs` của một Flow trigger; `outputs` trên API execution (get và list), OpenAPI, và client frontend qua `npm run gen:api` (không bao giờ orval trần) nếu frontend dùng schema.
  - **Test:** flow cha đọc `{{ outputs.sub.outputs.x }}`; flow của một Flow trigger đọc `{{ trigger.outputs.x }}`; API trả `outputs`, và output SECRET dưới dạng mặt nạ; test route cho danh sách.
  - **Chấp nhận:** ratchet không đổi (27 / 27); `handler_surface.py --check` xanh.
- **B4. SECRET và phạm vi FILE (spec D9, D10).** Niêm phong giá trị SECRET bằng cipher của `secret_inputs` vào các cột `secret_outputs` (danh sách Loop đánh khoá theo vị trí, outputs của flow, bản chép `Subflow`); che trong `outputs`, log và thông báo lỗi; không có master key → output thất bại nêu tên setting. Output FILE (flow và `Loop`) phải nằm trong họ loop của execution và đọc được; nếu không thì `egret refuses output <id>: <uri> is not a file of this execution`.
  - **Test:** JSON được lưu chứa `******`, cột niêm phong giải mã ra giá trị, một lần render sau mở ra nó, một dòng log in nó thì hiện mặt nạ; API không bao giờ trả nó; không có khoá → bị từ chối; output FILE của task trong chính execution được chấp nhận; file của một vòng lặp anh em được chấp nhận; file của một execution thuộc chuỗi replay hoặc trigger bị từ chối; giá trị sai dạng hoặc không phải `kestra://` bị từ chối; `subflow-secret-output-child`/`-parent` vẫn trong baseline, comment của chúng quy lại nguyên nhân là khoá của harness (Q3).
  - **Chấp nhận:** ratchet không đổi (27 / 27); census không đổi.

## Z. Tài liệu, sổ sách và cổng kiểm tra

- **Sổ sách oracle (khảo sát D9):** comment có ngày của mỗi dòng được bank trong `OE baseline.txt`; văn bản README về `retry-flowable-parallel` (`OE baseline.txt:84`, `:343-350`) nêu tên thuộc tính; bộ ba `sixteen_banked` trong `tests/executor.rs` nếu entry nào trong tám entry nằm trong đó; bảng construct; `OE README.md`.
- **Lộ trình (EN + VI):** dòng #6 đánh dấu xong kèm số bank đo được và PR; đóng ghi chú 6; danh sách nguyên nhân (`:277-301`) và bảng nguyên nhân (`:334-340`) sửa theo spec §2.2; một ghi chú có ngày liệt kê các chỗ sửa.
- **Spec §7 "Như đã xây" và trạng thái của kế hoạch này (EN + VI):** số bank đo được, gốc của T0, số migration đã dùng và ngày kiểm tra lại, các quyết định đã sửa.
- **Ghi chú phát hành:** spec D13 dưới dạng bảng trước/sau (như của #5a), cùng các giới hạn riêng của egret và các khác biệt ở spec §6, nối vào `/private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/3525fc70-d007-4b4e-b99e-ef27f3a3a1f8/scratchpad/inputs-docs-repo-notes.md` dưới tiêu đề **"Package #6 Loop outputs"** (cho repo tài liệu `egret-data/docs`, mục Platform: `outputs:`/`fetchType:` của `Loop`, `outputs:` của flow, output SECRET và FILE, trần, thừa kế retry của `Loop`, trường `outputs` mới trên API execution), và chép vào mô tả PR.
- **Kiểm tra lại migration cho merge (spec D12):** mô tả PR mang các lệnh chính xác của D12 như một mục checklist trước merge.
- **Cổng kiểm tra:** toàn bộ `cargo test -p egret-workflow` (chỉ 7 test `sql_starrocks_live` được phép thất bại), các crate có API hoặc migration thay đổi (`egret-server`, test migration của `egret-metastore`), `cargo fmt --check`, clippy `-D warnings --all-targets` trên các crate bị chạm, `cargo check -p egret-bin`, `handler_surface.py --check`/`--self-test`, `npm run typecheck` của frontend nếu B3 sinh lại client.
- **Chấp nhận:** executor **301 / 27 / 27 / 0** (chắc chắn 29 / 29; bộ số đo được tương đối so với T0), census 295 / 25 / 21, mọi cổng xanh, tài liệu EN và VI trong cùng commit.
