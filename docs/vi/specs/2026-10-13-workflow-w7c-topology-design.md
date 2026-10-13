# W7c — UI Workflows, phần 3: topology (đồ thị flow và đồ thị execution)

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-13-workflow-w7c-topology-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** đã triển khai, 2026-10-13. Bài end to end (`frontend/tests/e2e/workflows-topology.spec.ts`) chạy xanh trên một stack riêng. Đồ thị của editor đi theo một lần sửa 1 s sau đó và giữ đồ thị cuối dưới banner khi YAML hỏng; chọn một node chuyển con trỏ Code; đồ thị của execution hiện `Sleep` ở RUNNING rồi SUCCESS trong lúc polling, không làm dịch khung nhìn, rồi các trạng thái cuối, và không đọc lần nào trong 32 s sau khi kết thúc; thân `Loop` liệt kê ba iteration, một cái FAILED; `Subflow` mở rộng kèm trạng thái của flow con. Đo trên flow 15 task của bài: khung nhìn đầu tiên mở ở zoom 0.8, mức sàn đọc được (§6.1), với điểm bắt đầu của flow trong khung và id của task đầu tiên được vẽ ở 10,4 px, theo chiều ngang và chiều dọc, trong editor và trên execution. Fit, vốn hiện toàn bộ đồ thị, xuống tới 0.12–0.20 ở đó; ở mức zoom ấy mọi card, cluster và marker vẫn nằm trong canvas và không nhãn nào bị cắt, nhưng chữ quá nhỏ để đọc. Vòng focus bàn phím đo được 3,5:1 (sáng) và 3,6:1 (tối) so với card của nó. Phần so sánh với Kestra 2.0 nightly ở §2.

**Phụ thuộc:**
- W7a (#659): khu vực Workflows, editor nhiều panel và sổ đăng ký tab của nó (`registerTab`), phép ánh xạ từ path sang YAML của tab Problems.
- W7b (#662): chi tiết execution, các tab theo URL của nó, backoff polling, `StateChip`, và run panel mà Gantt mở ra.
- no-silent-stall (#663): các mã validation từ chối những cấu trúc egret không chạy (`unsupported_allow_failure`, `unsupported_working_directory`, …).

Bộ rule frontend `docs/en/frontend-rules.md` ràng buộc mọi dòng code frontend.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §8 (UI của Kestra làm tham chiếu: `ui/src/components/executions/Topology.vue`) và §9 dòng W7. **Spec W7a** §5.1 đã dành tab Topology cho gói này.

**Chốt với người dùng, 2026-10-13:**
- **Toàn bộ topology chỉ đọc, như Kestra có.**
  - Một tab Topology trong editor của flow, vẽ từ YAML chưa lưu.
  - Một tab Topology trên chi tiết execution, với màu trạng thái trực tiếp.
  - Cluster thu gọn được, nhánh `errors`/`finally` có màu riêng, một cluster trigger, bố cục ngang và dọc, zoom và fit.
  - Một `Subflow` mở rộng tại chỗ thành đồ thị của flow con.
  - Không sửa từ đồ thị: không thêm, sửa hay xoá task. Việc sửa topology của Kestra thuộc về no-code editor của nó, thứ W7a đã loại.
- **Thân của `Loop` là một node cho mỗi task, với trạng thái được gộp qua mọi iteration.** Áp dụng thứ tự ưu tiên của Kestra: FAILED, KILLED, WARNING, SKIPPED, KILLING, RUNNING, SUCCESS. Node hiện một số đếm như "2 of 3 iterations done, 1 failed", và chọn node sẽ liệt kê các iteration kèm liên kết tới execution con của chúng. egret chạy mỗi iteration như một execution con, và một node cho mỗi iteration sẽ làm đồ thị lớn theo dữ liệu.
- **Server dựng đồ thị, còn trình duyệt vẽ bằng `@xyflow/react` và `@dagrejs/dagre`.** Một nguồn sự thật duy nhất cho cấu trúc: alias, canonical type, phụ thuộc của `Dag`, các cấu trúc bị từ chối. Đây là bản đối ứng React của `vue-flow` và `dagre` ở Kestra.

## 1. Vì sao

W7a cho người dùng viết một flow và W7b cho họ theo dõi các lần chạy của nó, nhưng không cái nào cho thấy hình dạng của flow. Một flow có `Dag`, một `Switch` bên trong `Parallel` và một nhánh `errors:` đọc ra như một cây YAML, không phải thứ tự mọi việc xảy ra. Người dùng Kestra mong có một đồ thị cạnh code và trên mỗi execution, tô màu theo trạng thái.

## 2. Khác biệt so với Kestra

| Kestra | egret | Vì sao |
|---|---|---|
| Sửa từ đồ thị: thêm task trên một cạnh, sửa, xoá, thêm nhánh lỗi, chạy trong playground, replay một task | Chỉ đọc | Sửa no-code nằm ngoài W7 (phán quyết của W7a); replay một task không tồn tại trong egret |
| Đồ thị execution theo dõi execution qua SSE | Polling với backoff của W7b | API của egret chỉ có poll |
| Một node task gộp nhiều lần chạy của nó thành một trạng thái ở client (`TaskNode.vue`, `SORT_STATUS`), không có số đếm iteration và không có danh sách của chúng | Một node cho mỗi task, trạng thái được gộp qua các iteration ở server, một số đếm, và danh sách iteration khi chọn | egret chạy mỗi iteration của `Loop` như một execution con; phán quyết của người dùng. Thứ Kestra 2.0 vẽ cho thân `Loop` được đo ở bảng dưới |
| Xuất đồ thị ra PNG/JPEG | Không xây | Không được yêu cầu |
| `GET /flows/{ns}/{id}/dependencies` (đồ thị flow-với-flow) | Không thuộc W7c | Một tính năng khác; nó đi cùng namespace hoặc trigger |

Phần so sánh ở §9 bổ sung những khác biệt đo được khi đối chiếu với một Kestra đang chạy.

**Đo trên một Kestra đang chạy, 2026-10-13 (Task 7, §9).** Image cục bộ `kestra/kestra` `28f59bf175e1` (Kestra 2.0 nightly, `server local`, trong một container dùng một lần) chạy flow của bài end to end (§8), chỉ đổi id: một trigger `Schedule` bị disable, một `Sequential` chứa một `Sleep` và một `If`, một `Parallel`, một `Subflow`, một `Loop` ba giá trị có giá trị giữa fail, và một nhánh `errors:`. Hai server dựng cùng một hình: 26 node, 28 cạnh và 5 cluster, và 29, 31 và 6 khi mở rộng `Subflow`; mọi node và mọi cạnh đều ghép đôi được. Đã tìm thấy những khác biệt sau. Mỗi khác biệt được quan sát trên image đang chạy, qua API hoặc UI của nó, trừ khi dòng đó nói nó được đọc trong source của Kestra:

| Kestra 2.0 nightly | egret | Vì sao |
|---|---|---|
| Không có tab Topology trên execution: đồ thị nằm trong tab Overview, dưới phần tóm tắt (`executionTabs.ts`: Overview, Gantt, Logs, Input/Output, Metrics, Dependencies, …) | Một tab Topology, theo thứ tự của Kestra 1.x (§6.3) | Phán quyết của người dùng: một tab riêng, có URL chia sẻ được |
| Một iteration của `Loop` là một sub-execution kiểu `LOOP` (`kind` của nó trong API), và node thân trên đồ thị của execution cha không hiện trạng thái nào: card `item` chỉ hiện id của nó, không có thời lượng và không có nút "Show the state history", thứ mà mọi task đã chạy đều có, trong khi card `each` hiện thời lượng của nó và API cho `each` một lần chạy FAILED | Node thân gộp các lần chạy của nó: FAILED, "2 of 3 iterations done, 1 failed", và danh sách iteration | Phán quyết của người dùng (§4) |
| Uid: `root.prepare.check`, marker `root.prepare.root-{random}` và `root.prepare.end-{random}`, cluster `cluster_root.prepare`; phần ngẫu nhiên đổi ở mỗi request | `tasks.prepare.check`, `start:tasks.prepare`, `end:tasks.prepare`, `cluster:tasks.prepare`, giống nhau ở mọi request | Uid là path validation (§3.1), và một uid ổn định có thể làm `?node=` của URL |
| Các cạnh đi vào và đi ra khỏi marker, và chuỗi trigger, không mang relation (`relationType` null); cạnh của một nhánh tới end của `If` không có nhãn | Mọi cạnh có relation: của chính danh sách (`sequential`, `parallel`, `dynamic`, `error`), và một nhánh giữ nhãn của nó tới cuối (§3.2) | Client tô màu và gắn nhãn một cạnh từ chính các trường của nó, không cần nhìn hàng xóm |
| Mọi cạnh được vẽ chấm (`stroke-dasharray: 1.5px 3px`); cạnh `errors` màu đỏ | Cạnh liền; chỉ chuỗi trigger là nét đứt; `errors` bằng token danger, `finally` và `afterExecution` bằng token warning, mỗi cái kèm chữ của nó | §6.1: nét đứt nghĩa là "một trigger", và màu không bao giờ là tín hiệu duy nhất (FE-A11Y-4) |
| Card gọn, 137 × 28 px: icon, id, thời lượng, một menu | Card rộng 240 px: icon, id, loại rút gọn, state chip, thời lượng, số iteration, số lần thử | Các thông tin của execution được đọc trên card; canvas fit và zoom được |
| Mỗi node là một điểm dừng tab riêng (`tabindex=0`, `role=group`, không có tên truy cập được); không có list view | Một điểm dừng tab di chuyển bằng phím mũi tên, node là button có tên, và một treegrid List view mang cùng các node và trạng thái | FE-A11Y; list view là bản tương đương truy cập được của canvas |
| Click một node trong editor mở form no-code của task, và trên execution, log và output lấy từ menu của node (đọc trong `LowCodeEditor.vue`: `@edit`, `@show-logs`, `@show-outputs`; UI hiện một menu "More actions" trên mọi node) | Chọn một node chuyển con trỏ Code (§6.2); trên execution nó mở run panel của Gantt hoặc danh sách iteration (§6.3) | Chỉ đọc (phán quyết của W7a) |
| Một lần mở rộng không thực hiện được làm hỏng cả request đồ thị. Đã thử trên image với một flow thứ hai: `?subflows=root.dyn` (một `flowId` là expression) trả 422 "Can't expand subflow task 'dyn' because namespace and/or flowId contains dynamic values", và `?subflows=root.gone` (không có flow đó) trả 404 "Unable to find subflow"; đồ thị của cùng flow đó khi không có `subflows` trả 200 | Đồ thị vẫn được vẽ, và cluster của `Subflow` mang một `reason` (`not_found`, `dynamic`, `depth`) | Một lần mở rộng hỏng không nên làm mất phần còn lại của đồ thị |
| `?subflows=` nhận uid của Kestra (`root.call`) | `?subflows=` nhận uid của egret (`tasks.call`) | Theo cách đặt uid |
| Hướng lưu trong `localStorage` `topology-orientation`, một boolean (đọc trong `LowCodeEditor.vue`); dọc khi chưa lưu gì, như panel của editor đã hiện, trong khi overview của execution vẽ theo chiều ngang | `egret.workflows.topology.orientation`, `LR` hoặc `TB`; ngang khi chưa lưu gì | egret giữ một mặc định cho cả hai tab |
| Canvas có zoom in, zoom out, fit, nút đổi hướng và nút tải ảnh | Zoom in, zoom out, fit, hướng, Expand all, Collapse all và List view; không có tải ảnh | Xuất ảnh không được xây (dòng ở trên) |

## 3. Mô hình đồ thị (backend)

Một module mới `crates/egret-workflow/src/graph/` chứa một hàm thuần:

```rust
pub fn flow_graph(flow: &models::Flow, opts: &GraphOptions) -> Result<FlowGraph, GraphError>;
```

Nó đi qua model đã parse (`models::Flow`, `models::TaskKind`). Nó không đi qua plan của executor, vì plan bỏ đi những gì không chạy, như task bị disable và cấu trúc bị từ chối, mà đồ thị phải hiển thị chúng.

### 3.1 Node

Mỗi node có một `uid`: path của task theo cùng dạng mà các finding của validation dùng (`tasks.seq.child`, `errors.alert`), để một node và một finding về nó chỉ cùng một chỗ. Bên trong một `Subflow` đã mở rộng (§3.4), các uid của flow con được đặt tiền tố là uid của node `Subflow`.

**Uid, theo phán quyết 2026-10-13 (Task 1, Q6 và Q7):**
- Một task là path validation của nó; một trigger là `triggers.{id}`.
- Marker là `start:{owner}` và `end:{owner}`, còn một cluster là `cluster:{owner}`. Owner là `flow`, `triggers`, hoặc uid của chính flowable. Không có cluster cho flow, chỉ có cặp `start:flow`/`end:flow`. Không uid task hay trigger nào bắt đầu bằng `start:`, `end:` hay `cluster:` được, vì mọi uid đều bắt đầu bằng `tasks.`, `errors.`, `finally.`, `afterExecution.` hoặc `triggers.`.
- Một id lặp lại giữ path trơn ở lần đầu; lần sau lấy hậu tố còn trống đầu tiên trong `~2`, `~3`, …, theo thứ tự duyệt, và các con của nó được dựng dưới uid có hậu tố. `refusal` của nó được tra theo path trơn.
- Bên trong một lần mở rộng, uid của một task con là `{subflow}/{child}`, ví dụ `tasks.call/tasks.child_log`.

| Loại node | Trường | Khi nào |
|---|---|---|
| `task` | `uid`, `task_id`, `type` (như đã viết), `canonical_type`, `kind` (`leaf`, `executable`, `flowable`), `branch` (`tasks`, `errors`, `finally`, `after_execution`), `disabled`, `refusal` (một mã lỗi validation, hoặc null) | mọi task, kể cả flowable, vốn cũng là cluster |
| `trigger` | `uid` (`triggers.{id}`), `trigger_id`, `type`, `disabled` | mọi trigger |
| `start` / `end` | `uid` | một cặp cho flow, một cặp cho mỗi cluster |

`refusal` bao các cấu trúc mà no-silent-stall từ chối, để đồ thị có thể đánh dấu một task egret sẽ không chạy. **Phán quyết 2026-10-13 (Task 1):**
- Một lỗi của `validate_for_run` thuộc về node task có uid là tiền tố dài nhất của path lỗi: chính path đó, hoặc một phần của nó mà theo sau là `.`. Vì vậy một lỗi ở path của thuộc tính (`tasks.p.tasks` với một `Pause`, `tasks.l.checkFrequency.maxIterations` với một `LoopUntil`) thuộc về task của nó.
- `refusal` của một node là mã của lỗi đầu tiên như vậy, theo thứ tự validation.
- Chỉ node task sở hữu lỗi. Một lỗi mà không path task nào là tiền tố (`namespace`, `inputs.x`) không thuộc về node nào.
- Chấp nhận một chỗ còn nhập nhằng: một task con có id đúng bằng tên một thuộc tính của cha (một con tên `tasks` dưới một flowable) nhận lỗi ở path thuộc tính của cha.

### 3.2 Cạnh

`{source, target, relation, label}`, trong đó `relation` là một trong bảy loại của Kestra: `sequential`, `choice`, `parallel`, `dynamic`, `error`, `finally`, `after_execution`.

**Phán quyết 2026-10-13 (Task 1, Q1 tới Q5), như Kestra làm:** node của chính một flowable `X` nằm bên trong cluster của nó. `start:X → X` là `sequential`, các cạnh của cấu trúc đi ra từ `X`, và `errors:` của flowable đi ra từ `start:X`. Một flowable được đi vào ở `start:X` và đi ra ở `end:X`.

| Cấu trúc | Cạnh |
|---|---|
| Một danh sách task (`Sequential`, `AllowFailure`, `WorkingDirectory`, `LoopUntil`, một `Pause` có `tasks:`) | `sequential`: `X` → đầu → … → cuối → `end:X` |
| `Parallel` | `parallel` từ `X` tới từng con và từ từng con tới `end:X` |
| `If` | cạnh đầu của một nhánh là `choice`, gắn nhãn `then` hoặc `else`; các cạnh sau của nhánh, và cạnh của nó tới `end:X`, là `sequential` với cùng nhãn. Một nhánh rỗng không vẽ cạnh nào |
| `Switch` | như `If`, gắn nhãn bằng khoá của case (các case theo thứ tự khoá), rồi `defaults` |
| `Dag` | `dynamic` từ `X` tới mỗi node không có `dependsOn`, một cạnh cho mỗi mục `dependsOn` gọi tên một node được vẽ, và từ mỗi node mà không node được vẽ nào phụ thuộc tới `end:X` |
| `Loop` | `dynamic` trên mọi cạnh từ `X` qua thân tới `end:X` |
| `errors:` của một flowable | `error` trên mọi cạnh, từ `start:X`; lỗi cuối đi tới `end:X` |
| Flow | `start:flow` → tasks → `finally` → `afterExecution` → `end:flow`. `errors:` của flow đi ra từ `start:flow`, và lỗi cuối đi tới task `finally` đầu tiên, nếu không có thì task `afterExecution` đầu tiên, nếu không có thì `end:flow`. Cạnh đi vào task `finally` hoặc `afterExecution` đầu tiên mang relation của danh sách đó; mọi cạnh khác mang relation của danh sách nó rời đi; một chuỗi lỗi luôn là `error` |
| Trigger | chuỗi của Kestra: `start:triggers` → mỗi trigger → `end:triggers` → `start:flow`, tất cả là `sequential`. Client vẽ một cạnh nét đứt khi một đầu của nó nằm trong `cluster:triggers` |

Một danh sách rỗng không vẽ cạnh nào, ở mọi cấp.

### 3.3 Cluster

Mỗi flowable là một cluster: `{uid, task_id, kind, nodes, parent, start, end}`, với `nodes` chứa node của chính flowable (§3.2) và các marker của nó. `kind` là tên flowable canonical: `sequential`, `parallel`, `if`, `switch`, `dag`, `loop`, `loop_until`, `allow_failure`, `working_directory`, hoặc `pause` khi một `Pause` có `tasks:`. Các trigger tạo thành một cluster, `triggers`, như ở Kestra.

`AllowFailure` và `WorkingDirectory` được vẽ như cluster, như Kestra vẽ, và mang `refusal` của chúng.

### 3.4 Mở rộng Subflow

`GraphOptions.subflows` là một tập uid của các node `Subflow` cần mở rộng.

- Với mỗi cái, server nạp bản draft hiện tại của flow con theo `namespace` và `flowId` trong cùng workspace, dựng đồ thị của nó với cùng các option, sâu hơn một cấp, và ghép vào như một cluster kiểu `subflow` dưới node `Subflow`: `cluster:S` chứa `start:S`, node `Subflow` `S`, các node cấp cao nhất của flow con (uid `{S}/{child}`) và `end:S`, và cha của nó là cluster từng chứa `S`. Một task con nằm dưới `errors` hoặc `finally` của flow cha nhận nhánh đó, như ở Kestra.
- Một lần mở rộng không thể thực hiện sẽ thành một cluster không có node và có một `reason`:
  - `not_found`: không có flow như vậy trong workspace này;
  - `dynamic`: `namespace` hoặc `flowId` là một expression;
  - `depth`: việc mở rộng bị giới hạn ở hai cấp.

  Một cluster như vậy không vẽ marker: `start` và `end` của nó là uid của chính node `Subflow`, và node đó ở nguyên chỗ cũ.
- Không có reason `forbidden`. Một `Subflow` chỉ phân giải trong chính workspace của nó, và `workflow:read` là quyền ở mức workspace, nên người xem đọc được flow cha thì đọc được flow con.

Trên đồ thị execution (§4), một node `Subflow` mà lần chạy của nó đã khởi tạo một execution con sẽ mở rộng từ source đã đóng băng của execution con đó và mang các trạng thái của nó.

### 3.5 Giới hạn

Một đồ thị trên 5.000 node, đếm sau khi mở rộng, bị từ chối bằng 422 kèm thông báo nêu số đếm. Không flow nào trong corpus gần mức đó; giới hạn chỉ ngăn một flow bệnh lý dựng ra response khổng lồ.

## 4. Trạng thái execution

`GET …/executions/{id}/graph` dựng đồ thị từ `source` đã đóng băng của execution, tức source mà nó đã chạy, và thêm `states`, một map từ `uid` của node tới:

```json
{
  "state": "RUNNING",
  "attempts": 2,
  "run_id": "…",
  "started_at": "…",
  "ended_at": null,
  "iterations": {
    "total": 3,
    "by_state": { "SUCCESS": 2, "FAILED": 1 },
    "runs": [{ "loop_index": 0, "state": "SUCCESS", "child_execution_id": "…", "run_id": "…" }]
  }
}
```

- **Một task có một lần chạy** lấy trạng thái, số lần thử và mốc thời gian của lần chạy đó.
- **Một task trong thân `Loop`** có các lần chạy ở nhiều execution con, là những execution có `parent_execution_id` là execution này và có `loop_index`. `state` của nó là phép gộp các lần chạy đó theo thứ tự ưu tiên của Kestra, và `iterations` đếm chúng theo trạng thái. `run_id` là null.
  - **`iterations.runs`** (phán quyết 2026-10-13, Task 6) liệt kê trạng thái CỦA CHÍNH node trong mỗi iteration, một dòng cho mỗi `loop_index` mà nó đã chạy: các lần chạy của nó ở đó được gộp lại, execution con của iteration và lần chạy. Đó không phải trạng thái của iteration, thứ mà một task khác trong thân có thể quyết định. Một iteration mà node chưa từng chạy thì không có dòng, và `run_id` của một dòng là null khi một `Loop` lồng nhau chạy node nhiều lần trong đó. Danh sách iteration của §6.3 được vẽ từ đây, không giới hạn số dòng.
  - **`attempts`** của một phép gộp là số lần thử nhiều nhất mà một lần chạy cần, không phải tổng, nên client ghi "up to n attempts".
  - **`Loop` lồng nhau** đếm các iteration của loop ngoài: một iteration chạy thân ngoài cùng như `tasks:` của chính nó.
- **Một `Subflow` có một lần chạy** nêu execution con mà nó khởi tạo trong `child_execution_id` (cái mới nhất, khi một lần retry khởi tạo nhiều cái), để panel liên kết tới đó. Trường này vắng mặt ở mọi node khác và ở một phép gộp.
- **Một task không có lần chạy nào** không có mục. Client vẽ nó mờ đi.
- **Một cluster flowable** lấy trạng thái của chính lần chạy của nó khi có.

Trạng thái theo từ vựng của W7b và `StateChip`.

**Thứ tự của phép gộp, theo phán quyết 2026-10-13 (Task 3).** `SORT_STATUS` của Kestra trong `ui/packages/topology/src/nodes/TaskNode.vue` là FAILED, KILLED, WARNING, SKIPPED, KILLING, RUNNING, SUCCESS, RESTARTED, CREATED, nhưng code của nó không áp dụng thứ tự này: nó lấy ký tự thứ hai của chuỗi trạng thái làm chỉ số (`TaskNode.vue:262-268`), nên mọi chỉ số là -1 và phép sắp xếp rốt cuộc theo bảng chữ cái. egret theo thứ tự ưu tiên đã nêu và đặt các trạng thái riêng của nó theo nghĩa: FAILED, KILLED, CANCELLED (trạng thái kết thúc đã dừng lần chạy, cạnh KILLED), WARNING, SKIPPED, RETRIED (trạng thái kết thúc do `CREATE_NEW_EXECUTION` để lại), PAUSED (còn sống và chờ một người, ở chỗ của KILLING, thứ egret không có), RETRYING, RUNNING, SUBMITTED, SUCCESS, RESTARTED, CREATED.

## 5. Route

Tất cả nằm dưới `/api/ws/{workspace}/workflows/`, được chặn bởi tính năng workflows, và được ghi trong `openapi.json`. Client được sinh lại bằng `npm run gen:api`.

| Route | Quyền | Trả lời |
|---|---|---|
| `GET flows/{id}/graph?version=&subflows=` | `workflow:read` | đồ thị của draft, hoặc của một version |
| `POST flows/graph?subflows=`, body `{ "source": "…" }` | `workflow:manage`, như route validate | đồ thị của YAML chưa lưu |
| `GET executions/{id}/graph?subflows=` | `workflow:read` | đồ thị của execution kèm `states` |

**Lỗi:**
- YAML không parse được: 400, kèm dòng và cột của nó.
- Một flow, version hoặc execution của workspace khác: 404.
- Hơn 5.000 node: 422.

Một YAML parse được nhưng không qua validation vẫn có đồ thị: các task bị từ chối mang `refusal`.

## 6. Frontend

### 6.1 `TopologyView`

Một component dưới `frontend/src/components/workflows/topology/`, dùng chung cho cả hai tab.

**Vẽ và bố cục:**
- Nó được vẽ bằng `@xyflow/react` và bố cục bằng `@dagrejs/dagre`. Cả hai được pin chính xác và chỉ nạp trong chunk Topology; `test:bundle:workflows` được thêm một marker để không cái nào lọt vào chunk của danh sách flow.
- Một hàm thuần `layout(graph, { orientation, collapsed })` trả về vị trí và kích thước node và được unit test riêng.
- **Phán quyết 2026-10-13 (Task 4):** mỗi container một lần bố cục dagre, trong cùng trước, để một cluster giữ chỗ của nó trong bố cục của cha. Trên 300 thành viên trong một container, hoặc khi dagre ném lỗi (một chuỗi 2.000 task làm tràn stack của nó), container đó lùi về một bố cục phân lớp không đệ quy. Một bố cục vẫn thất bại thì hiện một alert, và list view vẫn dùng được mà không cần bố cục.
- Node không kéo được và không nối được.

**Một node task** hiện icon của loại, `task_id` và loại rút gọn. Trên đồ thị execution nó còn hiện `StateChip`, thời lượng và, với thân `Loop`, số iteration.

**Đánh dấu:**
- Màu nhánh dùng token OKLCH: `errors` bằng token danger, `finally` và `afterExecution` bằng token warning.
- Cạnh `choice` mang nhãn của chúng. Cạnh trigger là nét đứt.
- Một task có `refusal` có viền gạch chéo và ghi chú "egret does not run this task" kèm mã của nó. Một task bị disable bị làm mờ và nói rõ điều đó.

**Điều khiển:**
- Zoom in, zoom out và fit. Fit hiện toàn bộ đồ thị, xuống tới zoom tối thiểu 0.1 của canvas.
- **Khung nhìn đầu tiên** (sửa đổi 2026-10-13, Task 7). Một lần fit dưới 0.8 không đọc được: id task chữ mono cao 13 px, được vẽ ở 10,4 px khi zoom 0.8, và flow 15 task của bài end to end fit ở 0.12 tới 0.20. Vì vậy khi fit sẽ xuống dưới 0.8, khung nhìn đầu tiên mở ở 0.8 trên `start:flow` của flow và task đầu tiên của nó, card đầu tiên của danh sách `tasks:` (sửa đổi lần nữa, fix round 2). Theo hướng của bố cục, nó bắt đầu ở cạnh đầu của đồ thị khi task đầu tiên vẫn vừa canvas tính từ đó, để chuỗi trigger ở lại trên màn hình; còn không thì nó bắt đầu ngay trước `start:flow`, để các trigger nằm ngoài màn hình bên trái (hoặc phía trên). Theo chiều ngang của hướng đó, nó căn giữa điểm bắt đầu và task đầu tiên. Còn không thì khung nhìn đầu tiên là phép fit. Nó được đặt khi đồ thị đầu tiên tới và lại được đặt cho một flow hay execution khác, một hướng khác hoặc một tập cluster thu gọn khác, không bao giờ cho trạng thái của một lần poll, đồ thị của một lần sửa cùng flow hay một lần mở rộng `Subflow`, nên việc pan và zoom của người dùng được giữ.
- Một nút chuyển hướng, ngang hoặc dọc, được nhớ trong `localStorage`.
- "Expand all".
- Cluster thu gọn và mở rộng được.
- Một node `Subflow` có hành động Expand, fetch lại với `subflows`.

**Bàn phím và trình đọc màn hình:**
- Mỗi node là một button focus được. Phím mũi tên di chuyển theo thứ tự của đồ thị, và Enter mở node.
- Một nút "List view" thay canvas bằng một treegrid gồm cùng các node, lồng theo cluster, kèm trạng thái của chúng. Đây là bản tương đương truy cập được của canvas.

**Trạng thái giao diện:**
- Đang tải hiện skeleton (FE-STATE-2).
- Một flow rỗng báo rằng nó không có task.
- Khi lỗi, giữ đồ thị cuối cùng và hiện lý do.

### 6.2 Tab trong editor

- **Đăng ký** (sửa đổi 2026-10-13, Task 5). `registerTab({ id: "topology", … })` đăng ký tab cùng các tab khác của editor, như một stub có phần thân được nạp lazy trong chunk Topology. Một layout đã lưu có gọi tên `topology` luôn tìm thấy nó đã được đăng ký, và xyflow không bị nạp mỗi lần mở editor. Khung editor không đổi (W7a §5.1).
- **Người xem chỉ đọc.** Một người xem không có `workflow:manage` không dùng được `POST flows/graph` và không sửa được, nên tab của họ vẽ draft đã lưu qua `GET flows/{id}/graph`.
- **Cập nhật.** Tab gửi `POST flows/graph` với YAML hiện tại của editor, 1 s sau lần sửa cuối.
- **Lỗi.** Khi YAML không parse được, nó giữ đồ thị cuối cùng và hiện một banner, "The graph is out of date: the YAML has an error".
- **Chọn.** Chọn một node sẽ chuyển con trỏ của tab Code tới task đó, qua cùng phép ánh xạ path sang range mà tab Problems dùng.

### 6.3 Tab trên execution

- **Tab.** Nó được thêm vào các tab theo URL của W7b theo thứ tự của Kestra: Overview, Topology, Gantt, Logs, Outputs. Nó là một đoạn path, `…/executions/{id}/topology`, như các tab của W7b (sửa đổi 2026-10-13, Task 6); lựa chọn là `?node=` và các Subflow đã mở rộng là `?expand=`.
- **Cập nhật.** Nó fetch lại đồ thị execution theo backoff polling của W7b và dừng khi execution đã ở trạng thái kết thúc.
- **Chọn.** Chọn một node task mở cùng run panel mà Gantt mở, kèm log và output của lần chạy. Chọn một node thân `Loop` mở panel với danh sách các iteration, mỗi cái kèm trạng thái và một liên kết tới execution con của nó.

## 7. W7c không làm gì

- Sửa từ đồ thị (§2).
- Đồ thị phụ thuộc flow-với-flow.
- Minimap và xuất ảnh.
- Cập nhật trực tiếp vượt quá polling của W7b.

## 8. Test

**Rust:**
- `flow_graph` cho mọi flowable: số node và cạnh, relation, nhãn `choice`, cạnh phụ thuộc của `Dag`, các nhánh `errors`, `finally` và `afterExecution`, cluster trigger, `refusal` trên các cấu trúc bị từ chối, và task bị disable.
- Một đợt quét 345 flow của `oracle-exec/corpus`: mọi flow parse được đều dựng được đồ thị mà không panic, và mọi task của plan của executor đều có một node.
- Các route:
  - quyền;
  - 404 với workspace khác;
  - 400 kèm dòng với YAML không parse được;
  - giới hạn 5.000 node.
- Mở rộng Subflow: thành công, `not_found` (kể cả một flow cùng tên ở workspace khác), `dynamic`, `depth`.
- Trạng thái execution: một task được retry, một `Pause`, một `Loop` được gộp qua các execution con, một task chưa từng chạy, và thứ tự ưu tiên của phép gộp.

**Vitest**, trên component thật với fixture có thể fail:
- `layout`: cả hai hướng, cluster thu gọn, đồ thị rỗng.
- Node: đánh dấu trạng thái, `refusal` và disable.
- Tab editor: debounce, giữ đồ thị cuối khi lỗi parse, việc chọn di chuyển con trỏ Code.
- Tab execution: polling dừng khi kết thúc; việc chọn mở run panel và danh sách iteration.
- Điều hướng bằng bàn phím và list view.

**Playwright,** trên stack riêng, không bao giờ dùng stack dev dùng chung:
1. Tạo một flow có `Sequential`, `If`, `Parallel`, `Loop`, một nhánh `errors:` và một `Subflow`.
2. Mở Topology và sửa YAML: đồ thị đi theo.
3. Chạy flow: trạng thái của đồ thị đổi trong lúc chạy.
4. Chọn một node thân `Loop` và xem các iteration của nó; mở rộng `Subflow`.

Các phép đo DOM bao gồm chế độ sáng và tối và một panel hẹp.

## 9. So sánh với Kestra

Cùng một flow chạy trên image Kestra cục bộ. Topology của nó, trong editor và trên execution, được so với của egret, và mọi khác biệt có chủ đích đều được ghi ở §2.

**Đã làm, 2026-10-13 (Task 7).** Phép so sánh chạy trên image cục bộ `28f59bf175e1` (Kestra 2.0 nightly) trong một container dùng một lần `w7c-kestra` với storage nhúng của riêng nó, đã xoá sau đó. Phía Kestra được đọc qua API của nó (`GET /api/v1/main/flows/{ns}/{id}/graph`, có và không có `subflows`, và đồ thị của execution) và UI của nó, điều khiển bằng một script Playwright. Mọi khác biệt tìm được nằm ở §2.

## 10. Rule gói này phải đáp ứng

- Rulebook frontend, mọi rule; đặc biệt FE-ROUTE-2 (một khu vực lazy có bundle test), FE-STATE-2 (skeleton), FE-A11Y (node focus được, list view) và FE-DATA-2 (client được sinh).
- Hai dependency mới được pin chính xác, và `package-lock.json` chỉ đổi vì chúng.

## 11. Giới hạn đã biết

- Một `Subflow` mở rộng thành draft hiện tại của flow con trong editor, không phải version sẽ chạy. Mỗi execution chạy draft đã lưu, nên hai thứ này giống nhau khi flow con đã được lưu.
- Đồ thị execution đọc lại mọi lần chạy ở mỗi lần poll, và payload của nó là O(số lần chạy trong thân): `iterations.runs` liệt kê mọi iteration của mọi node trong thân và được gửi lại ở mỗi lần poll. Một flow có hàng nghìn iteration của loop làm lần đọc đó và câu trả lời đó nặng hơn; giới hạn 5.000 node không chặn số iteration, vì các iteration được gộp vào một node.
- Một `Subflow` bên trong thân `Loop` được mở rộng, trên đồ thị execution, từ draft hiện tại của flow con và không có trạng thái: các lần chạy của nó nằm trong execution con riêng của từng iteration.
- Fit hiện toàn bộ đồ thị và có thể vẽ nó quá nhỏ để đọc (0.12 tới 0.20 với một flow 15 task theo phép đo của Task 7). Thay vào đó, khung nhìn đầu tiên đọc được (§6.1), và list view đọc được ở mọi kích thước.
- Một lần sửa trong editor và một lần mở rộng `Subflow` giữ khung nhìn của người dùng và không fit lại, nên một node mới, hoặc một flow con vừa mở rộng, có thể nằm ngoài canvas cho tới khi người dùng bấm Fit. Thu gọn hoặc mở rộng một cluster, hoặc đổi hướng, đặt lại khung nhìn đầu tiên: những thao tác đó vẽ lại toàn bộ bố cục theo yêu cầu của người dùng.
- Các bậc (rank) không được căn thẳng giữa các cluster anh em: mỗi container được bố cục riêng (§6.1), nên hai cluster song song có thể đặt thành viên của chúng ở độ sâu khác nhau.
