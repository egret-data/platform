# W7c — UI Workflows, phần 3: topology (đồ thị flow và đồ thị execution)

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-13-workflow-w7c-topology-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-13.

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
| Một node cho mỗi iteration trong đồ thị execution (backend), được gộp thành một trạng thái bởi node task (frontend) | Một node cho mỗi task, trạng thái được gộp qua các iteration, một số đếm, và danh sách iteration khi chọn | egret chạy mỗi iteration của `Loop` như một execution con; phán quyết của người dùng |
| Xuất đồ thị ra PNG/JPEG | Không xây | Không được yêu cầu |
| `GET /flows/{ns}/{id}/dependencies` (đồ thị flow-với-flow) | Không thuộc W7c | Một tính năng khác; nó đi cùng namespace hoặc trigger |

Phần so sánh ở §9 bổ sung những khác biệt đo được khi đối chiếu với một Kestra đang chạy.

## 3. Mô hình đồ thị (backend)

Một module mới `crates/egret-workflow/src/graph/` chứa một hàm thuần:

```rust
pub fn flow_graph(flow: &models::Flow, opts: &GraphOptions) -> Result<FlowGraph, GraphError>;
```

Nó đi qua model đã parse (`models::Flow`, `models::TaskKind`). Nó không đi qua plan của executor, vì plan bỏ đi những gì không chạy, như task bị disable và cấu trúc bị từ chối, mà đồ thị phải hiển thị chúng.

### 3.1 Node

Mỗi node có một `uid`: path của task theo cùng dạng mà các finding của validation dùng (`tasks.seq.child`, `errors.alert`), để một node và một finding về nó chỉ cùng một chỗ. Bên trong một `Subflow` đã mở rộng (§3.4), các uid của flow con được đặt tiền tố là uid của node `Subflow`.

| Loại node | Trường | Khi nào |
|---|---|---|
| `task` | `uid`, `task_id`, `type` (như đã viết), `canonical_type`, `kind` (`leaf`, `executable`, `flowable`), `branch` (`tasks`, `errors`, `finally`, `after_execution`), `disabled`, `refusal` (một mã lỗi validation, hoặc null) | mọi task, kể cả flowable, vốn cũng là cluster |
| `trigger` | `uid` (`triggers.{id}`), `trigger_id`, `type`, `disabled` | mọi trigger |
| `start` / `end` | `uid` | một cặp cho flow, một cặp cho mỗi cluster |

`refusal` là mã của lỗi `validate_for_run` đầu tiên có path trùng với path của task. Nó bao các cấu trúc mà no-silent-stall từ chối, để đồ thị có thể đánh dấu một task egret sẽ không chạy.

### 3.2 Cạnh

`{source, target, relation, label}`, trong đó `relation` là một trong bảy loại của Kestra: `sequential`, `choice`, `parallel`, `dynamic`, `error`, `finally`, `after_execution`.

| Cấu trúc | Cạnh |
|---|---|
| Một danh sách task (`tasks`, `Sequential`, `AllowFailure`, `WorkingDirectory`, `LoopUntil`, một `Pause` có `tasks:`) | `sequential`, theo thứ tự, từ `start` của cluster tới `end` của nó |
| `Parallel` | `parallel` từ `start` tới từng con và từ từng con tới `end` |
| `If` | `choice`, gắn nhãn `then` và `else` |
| `Switch` | `choice`, gắn nhãn bằng khoá của case, và `defaults` cho danh sách mặc định |
| `Dag` | `dynamic` từ `start` tới mỗi node không có `dependsOn`, một cạnh cho mỗi mục `dependsOn`, và từ mỗi node mà không ai phụ thuộc tới `end` |
| `Loop` | `dynamic` từ `start` qua thân tới `end` |
| `errors:` của flow hoặc của một flowable | `error`, từ `start` của chủ sở hữu tới danh sách lỗi |
| `finally:` và `afterExecution:` của flow | `finally` và `after_execution`, sau danh sách chính |
| Trigger | `sequential` từ mỗi trigger tới `start` của flow, được client vẽ nét đứt |

### 3.3 Cluster

Mỗi flowable là một cluster: `{uid, task_id, kind, nodes, parent, start, end}`. `kind` là tên flowable canonical: `sequential`, `parallel`, `if`, `switch`, `dag`, `loop`, `loop_until`, `allow_failure`, `working_directory`, hoặc `pause` khi một `Pause` có `tasks:`. Các trigger tạo thành một cluster, `triggers`, như ở Kestra.

`AllowFailure` và `WorkingDirectory` được vẽ như cluster, như Kestra vẽ, và mang `refusal` của chúng.

### 3.4 Mở rộng Subflow

`GraphOptions.subflows` là một tập uid của các node `Subflow` cần mở rộng.

- Với mỗi cái, server nạp bản draft hiện tại của flow con theo `namespace` và `flowId` trong cùng workspace, dựng đồ thị của nó với cùng các option, sâu hơn một cấp, và ghép vào như một cluster kiểu `subflow` dưới node `Subflow`.
- Một lần mở rộng không thể thực hiện sẽ thành một cluster không có node và có một `reason`:
  - `not_found`: không có flow như vậy trong workspace này;
  - `dynamic`: `namespace` hoặc `flowId` là một expression;
  - `depth`: việc mở rộng bị giới hạn ở hai cấp.
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
  "iterations": { "total": 3, "by_state": { "SUCCESS": 2, "FAILED": 1 } }
}
```

- **Một task có một lần chạy** lấy trạng thái, số lần thử và mốc thời gian của lần chạy đó.
- **Một task trong thân `Loop`** có các lần chạy ở nhiều execution con, là những execution có `parent_execution_id` là execution này và có `loop_index`. `state` của nó là phép gộp các lần chạy đó theo thứ tự ưu tiên của Kestra, và `iterations` đếm chúng theo trạng thái. `run_id` là null; danh sách iteration lấy từ §6.3.
- **Một task không có lần chạy nào** không có mục. Client vẽ nó mờ đi.
- **Một cluster flowable** lấy trạng thái của chính lần chạy của nó khi có.

Trạng thái theo từ vựng của W7b và `StateChip`.

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
- Node không kéo được và không nối được.

**Một node task** hiện icon của loại, `task_id` và loại rút gọn. Trên đồ thị execution nó còn hiện `StateChip`, thời lượng và, với thân `Loop`, số iteration.

**Đánh dấu:**
- Màu nhánh dùng token OKLCH: `errors` bằng token danger, `finally` và `afterExecution` bằng token warning.
- Cạnh `choice` mang nhãn của chúng. Cạnh trigger là nét đứt.
- Một task có `refusal` có viền gạch chéo và ghi chú "egret does not run this task" kèm mã của nó. Một task bị disable bị làm mờ và nói rõ điều đó.

**Điều khiển:**
- Zoom in, zoom out và fit.
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

- **Đăng ký.** `registerTab({ id: "topology", … })` đăng ký tab trong chunk Topology. Khung editor không đổi (W7a §5.1).
- **Cập nhật.** Tab gửi `POST flows/graph` với YAML hiện tại của editor, 1 s sau lần sửa cuối.
- **Lỗi.** Khi YAML không parse được, nó giữ đồ thị cuối cùng và hiện một banner, "The graph is out of date: the YAML has an error".
- **Chọn.** Chọn một node sẽ chuyển con trỏ của tab Code tới task đó, qua cùng phép ánh xạ path sang range mà tab Problems dùng.

### 6.3 Tab trên execution

- **Tab.** Nó được thêm vào các tab theo URL của W7b theo thứ tự của Kestra: Overview, Topology, Gantt, Logs, Outputs.
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

## 10. Rule gói này phải đáp ứng

- Rulebook frontend, mọi rule; đặc biệt FE-ROUTE-2 (một khu vực lazy có bundle test), FE-STATE-2 (skeleton), FE-A11Y (node focus được, list view) và FE-DATA-2 (client được sinh).
- Hai dependency mới được pin chính xác, và `package-lock.json` chỉ đổi vì chúng.

## 11. Giới hạn đã biết

- Một `Subflow` mở rộng thành draft hiện tại của flow con trong editor, không phải version sẽ chạy. Mỗi execution chạy draft đã lưu, nên hai thứ này giống nhau khi flow con đã được lưu.
- Đồ thị execution đọc lại mọi lần chạy ở mỗi lần poll. Một flow có hàng nghìn iteration của loop làm lần đọc đó nặng hơn; giới hạn 5.000 node không chặn số iteration, vì các iteration được gộp vào một node.
