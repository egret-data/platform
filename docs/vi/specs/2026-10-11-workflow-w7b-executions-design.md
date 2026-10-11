# W7b — UI Workflows, phần 2: danh sách execution, chi tiết execution (overview, gantt, logs, outputs), resume và replay

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-10-11-workflow-w7b-executions-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, 2026-10-11; chưa cài.

**Phụ thuộc:**
- W7a (#659): khu vực Workflows, `StateChip`, `executionState.ts`, trang execution tối thiểu, `nextPollMs`.
- W10 (execution và các route execution).
- W11 (log).
- W9a (file của execution).

Bộ rule frontend `docs/en/frontend-rules.md` ràng buộc mọi dòng code frontend.

**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md`, §8 (UI của Kestra làm tham chiếu) và §9 dòng W7.

**Spec W7a:** `docs/{en,vi}/specs/2026-10-10-workflow-w7a-flows-design.md`. Các phán quyết trong đó vẫn còn hiệu lực, nhất là: mọi execution chạy bản draft đã lưu.

**Đã quyết với người dùng, 2026-10-11:**
- **Mốc thời gian thật.** Execution và task run có `started_at` và `ended_at` thật; attempt có thời điểm kết thúc. Tất cả được ghi ngay khi state đổi (migration 0180).
- **API danh sách.** Danh sách execution có thêm bộ lọc "chỉ execution gốc" và bộ lọc "con của execution X". `replayed_from_execution_id` được đưa ra API.
- **Ngoài phạm vi.** Kill, restart, labels, metrics và output cấp execution không làm, vì backend chưa có.
- **Trang chi tiết dùng tab như Kestra:** Overview, Gantt, Logs, Outputs; tab nằm trên URL.
  - Bấm một run trong Gantt thì mở log và output của riêng run đó.
  - Bỏ Metrics và Dependencies vì không có dữ liệu.
  - W7c sẽ thêm tab Topology.
- **Danh sách mặc định chỉ hiện execution gốc**, kèm công tắc "Show child executions". Không có thao tác hàng loạt, vì backend chỉ replay và resume từng execution một.

**Đã quyết với người dùng, 2026-10-11 (review Task 2):**
- **Bộ lọc tiền tố namespace phía server.** `GET …/executions` có thêm `namespace_prefix`: một namespace và mọi namespace bên dưới nó, neo theo dấu chấm. "Include child namespaces" gửi tham số này. Nó thay cho cách mở rộng phía client trên các namespace đã biết, vốn chỉ phân trang đúng trong 200 hàng đầu của mỗi namespace.
- **`root_only` giữ lại execution retry.** Chỉ ẩn iteration của loop và execution con của subflow hay flow trigger. Execution do retry `CREATE_NEW_EXECUTION` tạo ra là phần tiếp của một lần chạy gốc, nên vẫn hiện trong danh sách mặc định, với nhãn Retry.

**Đã quyết với người dùng, 2026-10-11 (review Task 3):**
- **Resume thật sự.** `POST …/executions/{id}/resume` giải phóng ngay mọi run `Pause` đang PAUSED của execution, bất kể `pauseDuration:` hay `timeout:`, và mỗi run kết thúc `SUCCESS`. Đó là `ExecutionService.resume` của Kestra, và `Pause.java` ghi rằng một pause được resume trước hạn "will always succeed". Trước quyết định này, route chỉ settle lại execution: nó chỉ kết thúc được pause đã quá hạn, việc mà bộ đánh thức (waker) của worker vẫn tự làm.

## 1. Vì sao

Từ W7a người dùng chạy được flow trên trình duyệt, nhưng chỉ theo dõi được qua một trang tối thiểu liệt kê state của các task run. W7b mang tới đúng những gì người dùng Kestra quen dùng cho một lần chạy:
- danh sách lọc được;
- dòng thời gian cho biết cái gì chạy lúc nào, kể cả retry;
- log trực tiếp;
- output và file của nó;
- hai hành động mà engine hỗ trợ.

Gantt cần thời điểm bắt đầu và kết thúc thật, nhưng engine chưa ghi lại. Thời điểm kết thúc duy nhất hiện có là `updated_at` của run, mà trường này đổi ở mọi lần ghi. W7b bổ sung các mốc thời gian đó.

## 2. Khác biệt với Kestra

| Kestra | egret | Vì sao |
|---|---|---|
| Cập nhật trực tiếp qua SSE (`/executions/{id}/follow`, `/logs/{id}/follow`) | Polling: execution theo nhịp backoff của W7a, log mỗi 2 giây theo cursor | API của egret chỉ hỗ trợ polling (W11) |
| Tab: overview, gantt, logs, outputs, metrics, dependencies, audit-logs, assets | Overview, Gantt, Logs, Outputs; Topology đến cùng W7c | Không có dữ liệu metrics hay dependencies; audit và assets chỉ có ở bản EE |
| Hành động: restart, replay (kèm inputs, từ một task run, theo revision), kill, pause, resume, change state, set labels, unqueue, force run, delete | Replay (cả execution, dùng source và inputs đã đóng băng) và resume | Chỉ hai lệnh này có trong engine (§5) |
| Resume giải phóng ngay run đang pause và run thành công; một `Pause` có thể khai báo input `onResume`, dialog resume sẽ hỏi các input đó | Resume giải phóng ngay mọi `Pause` đang pause và mỗi run thành công; không nhận input khi resume | egret bỏ qua `onResume` của `Pause` (`spec/schema/property-treatment.json`: `ignored`), và route không nhận body, nên không có gì để hỏi |
| Danh sách execution có thao tác hàng loạt | Không có thao tác hàng loạt | Chỉ có route theo từng execution; phán quyết của người dùng |
| Danh sách hiện mọi execution | Mặc định chỉ execution gốc, có công tắc "Show child executions" | Phán quyết của người dùng: iteration của loop và execution con của subflow sẽ làm ngập danh sách |
| Outputs có expression debugger | Không có expression debugger | Không có endpoint đánh giá biểu thức |
| Xuất CSV danh sách | Không có trong W7b | Không được yêu cầu |

Khác biệt nào tìm thấy khi đối chiếu với một Kestra đang chạy (§11) sẽ được thêm vào đây, kèm lý do.

## 3. Backend: mốc thời gian thật

### 3.1 Migration 0180

```sql
ALTER TABLE workflow.executions
  ADD COLUMN started_at timestamptz,
  ADD COLUMN ended_at   timestamptz;
ALTER TABLE workflow.task_runs
  ADD COLUMN started_at timestamptz,
  ADD COLUMN ended_at   timestamptz;
```

Các hàng đã có giữ `NULL`, không đoán. UI đánh dấu thời gian của chúng là ước tính (§6.3).

### 3.2 Khi nào ghi từng mốc

**Execution:**
- `started_at` được gán một lần, khi execution vào `RUNNING` lần đầu.
- `ended_at` được gán một lần, khi execution vào một state kết thúc (`is_terminated` trong `models/state.rs`).
- Một lần resume sau pause không làm dời `started_at`.
- Khi một lần chuyển state rời khỏi state kết thúc, `ended_at` bị xoá về null, và được gán lại khi hàng đó kết thúc lần nữa. *(Đính chính 2026-10-11, Task 1: `RETRY_FAILED_TASK` đang làm đúng như vậy — RUNNING, FAILED, RETRYING, RUNNING, SUCCESS — và có test giữ điều này.)*

**Task run:**
- `started_at` được gán một lần, khi run vào `RUNNING` lần đầu: với task do worker chạy là lúc run vào `RUNNING`, cũng là lúc attempt đầu tiên bắt đầu; việc worker nhận task chỉ đặt lease, nên thời gian chờ trong hàng đợi trước khi được nhận nằm trong thanh; với task flowable là lúc executor chạy nó lần đầu.
- `ended_at` được gán một lần, khi run vào một state kết thúc.

**Attempt:**
- Mỗi phần tử của `attempts` có thêm `ended_at`, tính bằng mili giây kể từ `created_at` của execution, cùng mốc gốc với `started_at` đang có.
- Trường này được gán khi attempt đó vào một state kết thúc, kể cả state nó kết thúc trước khi retry khởi động attempt kế tiếp.

**Cách ghi:**
- Mọi lần ghi nằm trong chính câu `UPDATE` đổi state: `SET state = $s, ended_at = CASE WHEN … THEN COALESCE(ended_at, now()) END …`. Không bao giờ đọc state rồi mới ghi.
- Khi cài, liệt kê mọi chỗ ghi state của execution và task run: executor, worker, subflow, loop, retry, resume, SLA cancel, replay. Sửa từng chỗ.
- Có test sẽ đỏ nếu một chỗ đổi state mà không gán mốc thời gian.

**Bất biến, có test:**
- `ended_at` là null, hoặc lớn hơn hay bằng `started_at`.
- Khi một hàng đã ở state kết thúc, các lần ghi sau đó (ví dụ cập nhật output hay `updated_at`) không làm `ended_at` thay đổi.

### 3.3 Route danh sách

`GET …/executions` có thêm ba bộ lọc:
- `root_only=true`: loại iteration của loop (`loop_index IS NOT NULL`) và execution con của subflow hay flow trigger (`trigger_execution_id IS NOT NULL`). *(Quyết 2026-10-11, review Task 2: execution do retry `CREATE_NEW_EXECUTION` tạo ra, có `parent_execution_id` mà không có `loop_index`, được giữ lại.)* `loop_index` phân biệt chính xác: CHECK `workflow_executions_loop_run_whole_or_absent` của migration 0138 buộc bộ ba của loop phải đủ hoặc vắng hết, nên có cha mà có index là iteration, có cha mà không có index là retry (`worker/replay.rs`).
- `parent=<uuid>`: trả các hàng có `parent_execution_id` hoặc `trigger_execution_id` bằng id đó.
  - Id phải thuộc workspace của người gọi.
  - Nếu id thuộc workspace khác, trả một trang rỗng, không bao giờ báo lỗi làm lộ việc id đó tồn tại.

- `namespace_prefix=<ns>`: trả các hàng có namespace bằng `<ns>` hoặc bắt đầu bằng `<ns>.`: `namespace = $n OR starts_with(namespace, $n || '.')`, SQL tĩnh, không dùng `LIKE`, nên `a.b` không bao giờ khớp `a.bc`, và `_` hay `%` là ký tự thường. *(Quyết 2026-10-11, review Task 2.)*

Gửi cùng lúc `root_only=true` và `parent`, hoặc cùng lúc `namespace` và `namespace_prefix`, hoặc gửi bất kỳ bộ lọc nào với giá trị sai định dạng, đều trả 400.

Cả hai bộ lọc dùng các index đã có, vốn bắt đầu bằng đúng các cột đó (`workflow_executions_loop_run_idx`, migration 0135, và `workflow_executions_trigger_idx`, migration 0138), như `EXPLAIN` cho thấy. Không thêm index mới. *(Đính chính 2026-10-11, Task 1: đã thử dựng index mới và đo, planner không dùng cái nào.)* Có test giữ query plan này.

### 3.4 DTO

- `ExecutionListItemDto` và `ExecutionDto` có thêm:
  - `started_at: Option<DateTime>`;
  - `ended_at: Option<DateTime>`;
  - `replayed_from_execution_id: Option<Uuid>`, lấy từ cột do migration 0166 thêm vào.
- `TaskRunDto` có thêm `started_at` và `ended_at`.
- Mỗi phần tử của `attempts` mang `ended_at` khi đã biết.
- `ExecutionDto` có thêm `can_manage: bool`, câu trả lời của PDP cho `workflow:manage`, giống `FlowDto` (W7a). Trường này nằm trên execution vì flow của nó có thể đã bị đưa vào thùng rác, khi đó không đọc được flow, trong khi execution vẫn đọc được. *(Bổ sung 2026-10-11, Task 3.)*
- `execution_trigger` có kiểu `object | null`: server gửi `null` cho một lần chạy qua API. *(Sửa 2026-10-11, Task 3.)*

`openapi.json` và client được sinh lại bằng `npm run gen:api` (FE-DATA-1).

## 4. Route và danh sách

Các route nằm dưới `/explore/:workspace/workflows/`, trong khu vực lazy của W7a:

| Route | Trang |
|---|---|
| `executions` | Danh sách execution (§4.1) |
| `flows/:id/executions` | Tab Executions trong trang chi tiết flow: cùng danh sách, cố định theo flow |
| `executions/:id` | Chuyển hướng sang `executions/:id/overview` |
| `executions/:id/:tab` | Chi tiết execution; `tab` là `overview`, `gantt`, `logs` hoặc `outputs`. Tab lạ thì chuyển hướng sang `overview` |

W7b thay `ExecutionsPlaceholder` và `ExecutionMinimal` của W7a. Danh sách tab trong trang chi tiết flow có thêm Executions.

### 4.1 Danh sách

Bố cục theo FE-PAGE-1: `ListPageShell` → toolbar → `SummaryStrip` (đếm theo state của trang hiện tại) → bảng.

**Cột:**
- id, là một link;
- flow, là một link (ẩn khi đang ở tab Executions của chính flow đó);
- namespace;
- chip state;
- thời điểm bắt đầu, dạng tương đối, hover thấy giờ chính xác;
- thời lượng, đếm tiếp khi execution đang chạy;
- trigger: API, schedule, webhook, flow trigger hoặc subflow, mỗi loại kèm icon và nhãn chữ.

**Bộ lọc,** tất cả nằm trên URL (FE-PAGE-2):
- state: chọn nhiều, gửi dưới dạng danh sách ngăn cách bằng dấu phẩy;
- namespace: có tuỳ chọn "Include child namespaces", khi bật thì gửi namespace dưới dạng `namespace_prefix` thay cho `namespace` (§3.3);
- flow;
- khoảng thời gian: các preset 1 giờ, 24 giờ, 7 ngày, 30 ngày và tuỳ chọn, gửi bằng `since` và `until`;
- công tắc "Show child executions": mặc định tắt; khi tắt thì gửi `root_only=true`.

**Phân trang:**
- 50 dòng mỗi trang.
- Số trang nằm trên URL; `total` dùng để dựng bộ phân trang.

**Refresh:**
- Poll mỗi 5 giây, chỉ khi trên trang có ít nhất một hàng chưa kết thúc và tab trình duyệt đang hiển thị.
- Toolbar có nhãn "Updated Ns ago" và nút Refresh.

**Trạng thái** (FE-STATE-1–3):
- Đang tải: skeleton.
- Rỗng khi chưa đặt bộ lọc nào: "No executions yet", kèm link tới danh sách flow.
- Rỗng vì bộ lọc: nói rõ không có kết quả khớp, kèm nút xoá bộ lọc.
- Lỗi: giữ các hàng cũ trên màn hình, hiện lỗi ngay tại chỗ kèm nút Retry.

## 5. Chi tiết execution

### 5.1 Header và polling

Header dùng chung cho mọi tab, gồm:
- id execution, flow (là một link), namespace, chip state;
- thời điểm bắt đầu, thời lượng;
- trigger;
- "Replayed from <id>" khi có `replayed_from_execution_id`;
- "Child of <id>" khi có execution cha hoặc execution trigger.

Execution được poll theo `nextPollMs` của W7a (2 giây; khi đọc lỗi thì giãn dần tới 30 giây) cho tới khi kết thúc. Một lần poll lỗi vẫn giữ dữ liệu cũ, và hiện lỗi ngay tại chỗ kèm nút Retry.

### 5.2 Hành động

Cả hai hành động cần quyền `workflow:manage`. Người dùng không có quyền này không thấy nút. Trang đọc quyền này từ `ExecutionDto.can_manage` (§3.4).

**Resume:**
- Hiện và bật đúng khi state là `PAUSED`, và phải qua dialog xác nhận. Dialog nói rằng mọi task đang pause sẽ kết thúc ngay như một lần thành công, bất kể duration hay timeout, và execution chạy tiếp.
- Route giải phóng ngay mọi run `Pause` đang `PAUSED` với `SUCCESS` (quyết định ở đầu tài liệu, review Task 3). Nó dời hạn của từng pause về thời điểm hiện tại; sau đó run rời `PAUSED` qua đúng con đường của một pause hết hạn tự nhiên, nên lịch sử (`… PAUSED SUCCESS`, `RESTARTED` trên execution) và mốc thời gian giống hệt một lần kết thúc tự nhiên. `LoopUntil` đang chờ vòng lặp kế tiếp không bị đụng tới.
- Cuộc đua với waker của worker được khoá của execution phân xử: bên nào lấy khoá trước thì thắng. Một pause có `timeout:` mà waker kết thúc trước thì là `FAILED`, và resume trả 409. Một pause vẫn `PAUSED` khi resume lấy được khoá thì kết thúc `SUCCESS`.
- Thành công thì refresh query của execution. Toast theo state mà route trả về: "Resumed the execution" chỉ khi execution không còn pause; "Resumed, but the execution then ended Failed." (hoặc Killed, Cancelled) khi kết thúc xấu; và một ghi chú phòng hờ nếu câu trả lời vẫn là `PAUSED`.
- 409 (không pause, hoặc waker thắng) thì hiện thông báo của server trong dialog.
- **Chỉ resume `Pause`** *(quyết định 2026-10-11, Task 3 fix round 2, theo review N-1)*. Execution cũng ở `PAUSED` khi một `LoopUntil` đang chờ vòng lặp kế tiếp, và resume không kết thúc việc chờ đó, giống Kestra. Vì vậy:
  - route trả 409 "Nothing to resume: no Pause task is waiting." khi không có run `Pause` nào đang chờ, và không ghi gì;
  - trang chỉ bật Resume khi có một run `PAUSED` mang `resume_with` là `NEVER` hoặc `TERMINAL:*` (bản ghi của chính engine về một `Pause` đang chờ). Nếu không, Resume bị vô hiệu hoá kèm lý do "Waiting on a loop condition, not a Pause task".

**Replay:**
- Chỉ bật khi execution đã kết thúc; còn lại thì vô hiệu hoá với lý do "Replay is available when the execution has finished".
- Dialog nói rõ replay sẽ bắt đầu một execution mới, dùng YAML và inputs đã đóng băng của execution này.
- Nhận 201 thì chuyển sang execution mới.
- Nhận 409 hoặc 400 thì hiện thông báo của server, ví dụ "replay the loop root".

Mọi hành động bị vô hiệu hoá đều hiện lý do thành chữ (FE-ACTION-4), dùng `DisabledReasons` của W7a.

### 5.3 Overview

- **Inputs:** bảng key/value. Giá trị dạng `kestra:///` được hiện thành một hàng file (§8).
- **Trigger:** loại trigger và các biến của nó. Body, headers và parameters của webhook hiện trong một khung JSON thu gọn được.
- **Execution con:** một danh sách gọn lấy qua `parent=<id>`, gồm id, state, `loop_index`/`loop_value` và thời điểm bắt đầu; mỗi dòng là link tới execution đó.
- **Task run:** một danh sách gọn gồm task id, value, state và thời lượng ("so far" khi đang chạy). Task id là một liên kết mở tab Gantt với run đó được chọn (`gantt?run=<run_ix>`). Dùng liên kết thay vì cả dòng bấm được để dùng được bằng bàn phím (FE-A11Y-3). *(Sửa 2026-10-11, Task 3.)*

## 6. Gantt

### 6.1 Dòng và cấp lồng nhau

- Mỗi task run là một dòng, theo thứ tự `run_ix`.
- Dòng lồng dưới dòng cha theo `parent_ix`, và thu gọn được.
- Các con của một run loop hoặc `ForEach` có nhãn theo `value` của chúng.
- Mỗi dòng hiện task id, value (nếu có), chip state và thời lượng.

### 6.2 Thanh thời gian

- **Trục:** đi từ lúc execution bắt đầu tới lúc kết thúc, hoặc tới thời điểm hiện tại khi còn đang chạy. Trục có vạch chia và nhãn dễ đọc.
- **Thanh của run:** đi từ `started_at` tới `ended_at`, tô màu theo token state của run.
- **Attempt:** mỗi attempt là một đoạn riêng trong dòng, từ `started_at` tới `ended_at` của attempt đó. Một attempt thất bại rồi được retry mang màu theo state kết thúc của chính nó, nên nhìn thấy được các lần retry.
- **Run chưa kết thúc:** thanh kéo dài tới hiện tại và được vẽ lại ở mỗi lần poll. Khi tab đang hiển thị, việc vẽ lại được giới hạn theo animation frame; khi tab bị ẩn thì dừng.
- **Run chưa từng bắt đầu** (`CREATED`, hoặc `SKIPPED` do `runIf`): hiện một điểm đánh dấu thay vì thanh.

### 6.3 Thời gian ước tính

Execution ghi trước migration 0180 có `started_at`/`ended_at` null. Khi đó gantt dùng lại cách tính cũ (`gantt_rows` trong `tests/executions_routes.rs`):
- bắt đầu là `created_at` của execution cộng `attempts[0].started_at`;
- kết thúc là `updated_at` của run, khi `ended_seq` đã có giá trị.

Các thanh này vẽ gạch chéo, có nhãn "estimated", và đầu tab có một ghi chú giải thích.

### 6.4 Tương tác

- Chọn một dòng thì mở một panel bên dưới biểu đồ, gồm:
  - log của run, dùng component Logs ở §7, lọc theo `task_run_id`;
  - output của run (§8);
  - `message` của run, nếu có.
- Run đang chọn nằm trên URL (`?run=<run_ix>`).
- Các dòng là một `grid`: điều hướng bằng phím mũi tên, Enter để mở panel, Escape để đóng (FE-A11Y-3).
- State thể hiện bằng chữ trên chip và icon, không chỉ bằng màu (FE-A11Y-4).
- Gantt được vẽ bằng phần tử DOM, không dùng thư viện chart. Kestra cũng làm vậy trong `Gantt.vue`; cách này giữ các thanh truy cập được bằng công cụ hỗ trợ.

## 7. Logs

Một component Logs duy nhất phục vụ cả tab Logs và panel run trong Gantt.

**Bộ lọc:**
- Level tối thiểu (TRACE, DEBUG, INFO, WARN, ERROR), task và task run. Ở tab Logs, các bộ lọc nằm trên URL.
- Gửi lên server bằng `min_level`, `task_id`, `task_run_id`.

**Hiển thị:**
- Log nhóm theo task run, mỗi nhóm thu gọn được. Các dòng cấp execution (không thuộc task nào) hiện trước tiên.
- Mỗi dòng gồm:
  - thời điểm;
  - badge level có chữ;
  - số attempt, khi có hơn một attempt;
  - nội dung, font monospace, giữ nguyên khoảng trắng.

**Theo dõi trực tiếp** (theo hợp đồng của W11):
- Đọc theo cursor: `after` là giá trị `next_after` gần nhất.
- Khi log còn có thể đến thêm, poll mỗi 2 giây.
- Chỉ dừng khi `complete && items.length === 0`. Không bao giờ dừng chỉ vì execution đã ở state kết thúc, vì phía đọc chậm hơn phía ghi 2 giây.

**Cuộn:**
- Nếu người dùng đang ở cuối danh sách thì tự cuộn theo log mới.
- Nếu họ đã cuộn lên thì ngừng tự cuộn, và hiện nút "Jump to latest".

**Khối lượng:**
- Mỗi trang 500 dòng.
- Danh sách được ảo hoá (chỉ render phần đang nhìn thấy), nên vẫn mượt với hàng chục nghìn dòng.
- Phần log cũ hơn những gì đã tải được mở bằng nút "Load earlier".

**Tải xuống:** nút "Download" duyệt hết các trang theo cursor với bộ lọc hiện tại, rồi lưu thành một file `.log`, mỗi dòng log một dòng.

**Trạng thái:**
- Đang tải: skeleton.
- Rỗng: "No logs yet" khi execution còn chạy; "No logs" khi đã xong.
- Một lần poll lỗi vẫn giữ các dòng đã hiện, kèm nút Retry.

## 8. Outputs và file

**Bố cục:**
- Bên trái: danh sách task run có output.
- Bên phải: output của run đang chọn, dưới dạng cây thu gọn được.

**Giá trị:**
- Mỗi giá trị copy được.
- Mỗi đường dẫn copy được dưới dạng biểu thức, ví dụ `{{ outputs.<taskId>.<key> }}`.
- Bên trong loop, biểu thức theo dạng `outputs.<taskId>[value]` của Kestra, và có ghi chú điều đó.

**File:** mọi chuỗi bắt đầu bằng `kestra:///` trong output hoặc inputs được hiện thành một hàng file, gồm:
- tên file;
- kích thước, lấy từ `file/metas`;
- **Preview**: mở một drawer hiện các dòng của `file/preview`;
- **Download**: một link tới `file`.

**Drawer preview:**
- Giá trị Ion hiện dạng JSON; file text hiện từng dòng.
- File không preview được thì ghi "This file cannot be previewed".
- Preview bị cắt bớt thì ghi rõ điều đó, kèm giới hạn số dòng.
- Nhận 404 (file đã bị xoá theo thời hạn lưu trữ, hoặc không tồn tại) thì ghi "This file is no longer available".

**Không có:** output cấp execution (§10).

## 9. Cấu trúc frontend

**Trang:** `frontend/src/routes/workflows/` chứa:
- `ExecutionsList.tsx`, dùng chung cho trang danh sách và tab trong flow;
- `ExecutionDetail.tsx`, gồm header, tab và hành động;
- mỗi tab một file riêng.

**Component:** `frontend/src/components/workflows/executions/` chứa:
- `gantt/`: `ganttModel.ts` (một model thuần, không phụ thuộc UI) và phần hiển thị;
- `logs/`: hook `useLogFollow.ts` và phần hiển thị;
- `outputs/`;
- `files/`.

**Query key:** `workflowsKeys` có thêm:
- `executions(ws, params)`;
- `execution(id)` (đã có);
- `executionLogs(id, params)`;
- `executionFile(id, path)`.

**Polling:** `nextPollMs` của W7a được tổng quát hoá cho mọi kết quả query; các test của W7a vẫn phải xanh.

## 10. Ngoài phạm vi

- Kill, restart, change state, labels, delete, thao tác hàng loạt.
- Metrics, Dependencies, expression debugger, output cấp execution.
- Topology (W7c), namespace (W7d), trigger (W7e).
- SSE.

## 11. Đối chiếu với Kestra

Trong Task 7, một Kestra tạm từ image `kestra/kestra` có sẵn trên máy chạy ở một cổng trống, và bị xoá sau khi xong. Các phần sau được so về cấu trúc và tương tác (FE-REF-2):
- bộ lọc và cột của danh sách;
- gantt: dòng, cấp lồng nhau, đoạn attempt, cách chọn run;
- log: bộ lọc, cách nhóm, theo dõi trực tiếp;
- output và preview file.

Khác biệt được đưa vào §2.

## 12. Kiểm thử

**Rust:**
- Mốc thời gian trên mọi đường đổi state: thành công, thất bại, retry (`RETRY_FAILED_TASK` và `CREATE_NEW_EXECUTION`), pause rồi resume, subflow (`wait` true và false), iteration của loop, SLA cancel, replay.
- Resume thủ công: mọi loại pause được giải phóng ngay thành `SUCCESS` kèm mốc thời gian, nhiều pause song song, cuộc đua với waker (chỉ chuyển state một lần), 409 khi không pause, 409 khi chỉ có `LoopUntil` đang chờ (không ghi gì), và 404 với workspace khác.
- Các bất biến ở §3.2.
- Thời điểm kết thúc của attempt khi retry.
- `root_only` và `parent`:
  - id cha thuộc workspace khác thì trả trang rỗng;
  - gửi cả hai bộ lọc thì trả 400;
  - các index thật sự được dùng, kiểm bằng `EXPLAIN`;
  - execution retry vẫn hiện khi `root_only`, còn iteration của loop và execution con của subflow thì không.
- `namespace_prefix`: khớp chính xác, khớp con, namespace anh em cùng tiền tố nhưng không có dấu chấm, workspace khác, và 400 khi gửi cùng `namespace`.
- Migration.

**Vitest** (render component thật, fixture có khả năng làm test đỏ):
- `ganttModel`: thanh, đoạn attempt, thanh đang chạy, điểm đánh dấu run chưa từng bắt đầu, cách tính ước tính, cấp lồng nhau; dữ liệu đầu vào không sắp sẵn.
- `useLogFollow`:
  - cursor tiến đúng;
  - chỉ dừng khi `complete && empty`;
  - một lần poll lỗi vẫn giữ các dòng đã có;
  - tự cuộn so với người dùng đã cuộn lên.
- Danh sách:
  - bộ lọc ghi lên URL và đọc lại được;
  - gửi `root_only` khi công tắc tắt;
  - ba trạng thái;
  - chỉ poll khi có hàng chưa kết thúc.
- Chi tiết:
  - hành động bật hoặc vô hiệu hoá kèm lý do;
  - Resume bật cho mọi pause, và thông báo sau resume theo đúng state được trả về;
  - replay chuyển trang, và hiện lỗi từ server;
  - tab lạ chuyển hướng về overview.
- Outputs: nhận ra `kestra:///`, copy dưới dạng biểu thức, các trạng thái của preview.

**Playwright,** trên một stack riêng, không bao giờ trên dev stack dùng chung:
1. Chạy một flow gồm: một task bị retry, một `Pause` có duration, một loop và một task sinh nhiều log.
2. Theo dõi log trực tiếp.
3. Mở gantt và chọn một run.
4. Preview một file.
5. Resume.
6. Replay.

Đo trên DOM ở cả chế độ sáng lẫn tối, và đo gantt ở bề rộng hẹp.

**Cổng kiểm:**
- Frontend theo FE-TEST-4: lint, tsc, test, build, mọi `test:bundle*`.
- Backend: `cargo test -p egret-workflow`, `egret-server`, fmt, và clippy với `-D warnings`.

## 13. Hình dạng plan

1. Backend: migration 0180, ghi mốc thời gian ở mọi chỗ đổi state, thời điểm kết thúc của attempt, `root_only`/`parent` kèm index, trường mới trong DTO, sinh lại API.
2. Danh sách execution và tab Executions trong trang flow.
3. Khung trang chi tiết: route, header, polling, tab, Overview, hành động.
4. Gantt.
5. Component và tab Logs, cùng việc dùng lại nó trong panel của Gantt.
6. Outputs và file.
7. End to end, đối chiếu Kestra, tài liệu.

## 14. Rule package này phải đáp ứng

Toàn bộ `docs/en/frontend-rules.md`. Những rule quan trọng nhất ở đây:
- FE-PAGE-1/2/4;
- FE-STATE-1–4;
- FE-ACTION-1–4;
- FE-VISUAL-1/4/6;
- FE-DATA-1–3;
- FE-ROUTE-2;
- FE-A11Y-3/4;
- FE-REF-1/2;
- FE-TEST-1/2/4.

## 15. Giới hạn đã biết

- Execution ghi trước migration 0180 có thời gian ước tính (§6.3).
- Một lần retry của một vòng lặp con hoặc của một execution con do subflow tạo ra hiện trong danh sách mặc định như một execution gốc. Execution thay thế do `CREATE_NEW_EXECUTION` tạo ra mang `parent_execution_id` (là execution con) nhưng không có `loop_index` và không có `trigger_execution_id`, đúng hình dạng của một lần retry của execution gốc, nên `root_only` giữ nó lại (§3.3). *(Phát hiện trong review Task 2.)*
