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
- Nếu về sau có lần chuyển state rời khỏi state kết thúc (hiện chưa đường nào làm vậy), `ended_at` bị xoá về null. Có test khẳng định hiện không đường nào làm thế.

**Task run:**
- `started_at` được gán một lần, khi run vào `RUNNING` lần đầu: với task do worker chạy là lúc worker nhận và bắt đầu chạy; với task flowable là lúc executor chạy nó lần đầu.
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

`GET …/executions` có thêm hai bộ lọc:
- `root_only=true`: loại các hàng có `parent_execution_id` hoặc `trigger_execution_id` khác null, tức iteration của loop, execution con của subflow và execution con do flow trigger tạo ra.
- `parent=<uuid>`: trả các hàng có `parent_execution_id` hoặc `trigger_execution_id` bằng id đó.
  - Id phải thuộc workspace của người gọi.
  - Nếu id thuộc workspace khác, trả một trang rỗng, không bao giờ báo lỗi làm lộ việc id đó tồn tại.

Gửi cả hai bộ lọc cùng lúc, hoặc gửi một bộ lọc với giá trị sai định dạng, đều trả 400.

Index partial trên `(workspace_id, parent_execution_id)` và `(workspace_id, trigger_execution_id)`, chỉ cho các hàng có cột đó khác null, giúp cả hai bộ lọc chạy nhanh.

### 3.4 DTO

- `ExecutionListItemDto` và `ExecutionDto` có thêm:
  - `started_at: Option<DateTime>`;
  - `ended_at: Option<DateTime>`;
  - `replayed_from_execution_id: Option<Uuid>`, lấy từ cột do migration 0166 thêm vào.
- `TaskRunDto` có thêm `started_at` và `ended_at`.
- Mỗi phần tử của `attempts` mang `ended_at` khi đã biết.

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
- namespace: có tuỳ chọn "include children", lọc phía client trên các namespace đã biết, vì API so khớp namespace chính xác;
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

Cả hai hành động cần quyền `workflow:manage`. Người dùng không có quyền này không thấy nút.

**Resume:**
- Chỉ hiện khi state là `PAUSED`, và phải qua dialog xác nhận.
- Giới hạn của backend (route resume trong `routes.rs`): task `Pause` không có `pauseDuration` lẫn `timeout` thì vẫn đứng ở PAUSED sau khi gọi resume. Với trường hợp đó, Resume bị vô hiệu hoá kèm lý do "This pause has no duration or timeout, so it cannot be resumed from here". Thông tin này đọc từ `source` đã đóng băng của execution.
- Thành công thì refresh query của execution.

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
- **Task run:** một danh sách gọn gồm task id, value, state và thời lượng. Bấm một dòng thì mở tab Gantt với run đó được chọn.

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
- Các bất biến ở §3.2.
- Thời điểm kết thúc của attempt khi retry.
- `root_only` và `parent`:
  - id cha thuộc workspace khác thì trả trang rỗng;
  - gửi cả hai bộ lọc thì trả 400;
  - các index thật sự được dùng, kiểm bằng `EXPLAIN`.
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
  - trường hợp resume khi pause không có timeout;
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
- Resume không giải phóng được một `Pause` không có duration hay timeout (§5.2).
- "Include children" của namespace chỉ phủ các namespace mà client đã biết (§4.1).
