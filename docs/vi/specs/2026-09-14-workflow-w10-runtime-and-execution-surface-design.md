# W10 — Runtime và bề mặt execution, cùng bốn hạng mục không gói nào sở hữu

**Trạng thái:** thiết kế, chờ review
**Phụ thuộc:** W3b (executor bền), W4 (runner), W8 (ngữ nghĩa nâng cao — đã merge)
**Mở đường cho:** W7 (frontend), và W7 đang bị chặn bởi chính gói này
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md`

> Bản tiếng Anh là bản chuẩn:
> `docs/en/specs/2026-09-14-workflow-w10-runtime-and-execution-surface-design.md`.

## 1. Phép đo, trước hết — và lần này nó nói về BẢNG PACKAGE, không phải corpus

Mọi gói W trước đều mở đầu bằng một phép đo trên corpus. Gói này mở đầu bằng một
loại khác, vì phát hiện không nằm ở flow mà ở **quyền sở hữu**.

Dòng W7 viết: *"Workflows nav group; YAML editor; read-only topology; execution
gantt and logs"*. **Hai trong bốn hạng mục ấy không dựng được**, và lý do là
**bốn** thứ chúng cần **không thuộc gói nào trong bảng**. Ba thứ đã nằm sẵn trong
bảng này lúc viết spec; thứ tư — vòng trigger — do W10 Task 1 tìm ra trong lúc đo
schema cho dòng log, và nó được liệt ở đây vì một phát hiện về các hạng mục vô
chủ mà lại lặng lẽ bỏ sót một hạng mục thì chính là lặp lại cái khiếm khuyết nó
đang ghi:

| W7 cần gì | spec chương trình thiết kế ở đâu | gói nào sở hữu việc xây |
|---|---|---|
| một tiến trình đẩy execution đi tiếp | **§6.3**, *"worker loop … trigger loop … reclaim"* | **W3b** — *"claim loops; reaper"*. **Đã merge. Hàm thì có; không ai gọi.** |
| một bề mặt REST cho execution | **không đâu cả** | **không gói nào** |
| `workflow.task_logs` + một sink biết ghi vào đó | **§6.1** (danh sách bảng) và **§11** câu hỏi mở 1 | **không gói nào** |
| một vòng kích hoạt trigger `cron:` và lịch | **§6.3**, *"worker loop … **trigger loop** … reclaim"*, và dòng `workflow.triggers` ở **§6.1** | **không gói nào** — W6 ship `trigger::tick` dưới dạng **hàm thuần trên state được tiêm vào** và không xây bảng nào cho nó |

**Kiểm bằng máy, không phải đọc:** quét cả bảng package (§9) tìm
`task_logs|execution api|ExecutionController|wire|binary|daemon` → **0**.
`ExecutionController` xuất hiện **đúng một lần** trong spec chương trình, ở dòng
352, bên trong mục *"Not ported, but required reading before W7"* — nó được nêu
như một **hình dạng để nghiên cứu**, chưa bao giờ như một hạng mục phải giao.

**Dòng thứ tư được đo theo đúng cách ấy, mỗi số 0 đều kèm một control.** Không
migration nào tạo `workflow.triggers` — schema `workflow` có đúng bốn bảng:
`flows`, `flow_versions`, `executions`, `task_runs` (control: `workflow.executions`
xuất hiện trong **ba** file migration, nên phép tìm có chạm tới migration).
`TriggerState::initialise` có **đúng một** caller và đó là một helper `#[cfg(test)]`
trong `trigger/mod.rs`; ngoài `crates/egret-workflow/src/trigger/` không có gì gọi
`trigger::tick`; và `crates/egret-bin/src/main.rs` chứa chuỗi `trigger` **không lần
nào** (control: `egret_workflow` xuất hiện **5** lần trong cùng file). **Nên một
trigger `cron:` KHÔNG THỂ nổ trong production, và W10 với phạm vi hiện tại không
đổi điều đó** — W10 nối vòng *worker*, vốn có bảng để claim; vòng trigger thì
không có bảng nào.

**Đây là ảnh gương của một defect W8 đã đóng.** Ở đó `allowFailure` bị giao **hai
lần** trong hai tài liệu và gói phải nói chủ nào sai. Ở đây bốn hạng mục được giao
**không lần nào**. Một dòng gọi tên một thứ trong kiến trúc rồi không bao giờ gọi
lại trong kế hoạch thì, với người đi kiểm độ phủ, **trông y hệt một dòng đã xong**.

Nên W10 tồn tại để **đặt chủ cho cả bốn**. Hai thứ đầu W10 tự xây; hai thứ còn
lại W10 gọi tên chứ không nuốt vào: **W11** là log, **W12** là vòng trigger, mỗi
cái được ghi ở §9 kèm phép đo, như một quyết định chứ không phải một chỗ bỏ sót.
Hạng mục phải giao của W10 là *execution đi tiếp được và nhìn thấy được*; có nó
thì một flow chạy được bằng start thủ công qua API hoặc bằng webhook, cả hai đều
đã hoạt động. Cron là một năng lực riêng, cần bảng riêng, claim riêng và lập luận
an toàn riêng; nhét nó vào lúc này sẽ nhân đôi phần việc concurrency của runtime
trước khi phần việc ấy tồn tại. **Bảng §9 của spec chương trình là một hạng mục
phải giao của gói này**, không phải tài liệu tham chiếu cho nó.

## 2. Cái gì phán xét W10 — và câu trả lời trung thực là *không phải ratchet*

Mọi gói trước đều báo một con số: 143 → 127 → 109. **W10 sẽ không báo một dịch
chuyển ratchet nào, theo chiều nào cũng không — và đó là chuyện CẤU TRÚC chứ không
phải đáng thất vọng.**

`oracle-exec/driver.rs:884` gọi `worker::tick(pool, BATCH, reg)` **trực tiếp**, với
**registry của riêng nó**. Nghĩa là oracle **đã** chạy **thân** của vòng lặp trên
cả 345 flow — và **về cấu trúc không nhìn thấy phần WIRING trong binary**. Nối
worker vào `egret-bin` dịch **0** entry; xoá phần nối ấy đi sau cũng dịch **0**.

Đây **không** phải cơ chế mù thứ sáu. Đây đúng vị trí mà `egret.sql`,
`egret.container` và bốn trigger của W6 đang đứng: **được xây, và được phán xét
bằng test của chính chúng, nói thẳng ở trang một** để một ratchet xanh không bao
giờ bị đọc thành độ phủ mà nó không có. Kỷ luật W3b §3.1 lập ra sau khi việc đã
rồi, W4 và W6 áp dụng trước khi làm — ở đây áp cho **cả gói**.

**Nên các dụng cụ là:**

1. **Test của chính runtime**, cộng một bằng chứng end-to-end **hôm nay chưa tồn
   tại**: *một flow được publish qua API, được khởi động qua API, được quan sát đi
   tới trạng thái kết thúc qua API, mà KHÔNG có harness nào lái tick.* Một test ấy
   là tiêu đề của cả gói, vì **chưa có gì trong repo từng chạy một workflow mà
   không có test lái nó**.
2. **Test của chính API execution**, theo hình dạng router quản lý của
   `egret-dataapi` đã dùng.
3. **Phần đính chính ở §1**, vì một bảng package không đặt chủ cho thứ gì **tệ hơn**
   một bảng đặt sai chủ — đặt sai thì còn có người cãi lại.

## 3. Runtime: mọi thứ đã viết, không gì được gọi

Đo được, và đây là đoạn chịu lực:

- `worker::claim::tick(pool, n, &dyn Registry) -> Result<usize>` là **public**.
  `wake`, `due_retries`, `resume`, `start_execution`, `start_execution_with_trigger`
  và `reaper::reap(pool, timeout) -> Result<usize>` cũng vậy.
- `Handlers::with_core_tasks()` **không nhận tham số nào** và đăng ký bốn handler.
  `egret.sql` **không** nằm trong registry nào vì nó cần một
  `Arc<dyn QueryExecPort>`; doc đã ghi sẵn một dòng để thêm.
- `main.rs` dựng `egret_workflow::WorkflowState`, mount `management_router()` và
  `webhook_router()`, đọc `config.workflow.webhooks_enabled`. **Nó không spawn gì
  cả.**

**Hệ quả, nói thẳng vì nó chính là lý do gói này tồn tại:** một webhook tạo được
hàng execution và **không gì đẩy nó đi tiếp bao giờ**. Chừng nào W10 chưa ship,
mọi tính chất độ bền mà W3b, W4 và W8 chứng minh là tính chất của **mã không
deployment nào chạy** — tức item 15 trong tuyên bố độ phủ ở `oracle-exec/README.md`,
vẫn đúng sau ba gói.

### 3.1 Không cần leader election, và lập luận đã được viết sẵn

`claim::claim` dùng **`FOR UPDATE SKIP LOCKED`**. Đó đúng tính chất mà
`egret-automation` dựa vào, và doc `runner::spawn` của nó phát biểu kết luận trong
một câu: *"một periodic task mỗi replica, không leader election, `SKIP LOCKED`
trong claim khiến điều đó an toàn."*

W10 chép nguyên hình dạng ấy — `tokio::spawn`, `tokio::time::interval`,
**`MissedTickBehavior::Skip`** (để một replica bị tạm dừng không bắn một tràng tick
bù), và một `CancellationToken` — có cờ config theo kiểu
`config.automation.enabled` / `tick_interval_secs`.

**Nhưng `tick` không phải thứ duy nhất vòng lặp gọi, và mỗi thứ cần câu của riêng
nó chứ không thừa hưởng câu của claim.** `wake`, `due_retries` và `reap` là những
câu lệnh riêng trên cùng bảng; **gói phải phát biểu, theo từng lời gọi, vì sao
nhiều replica chạy song song là an toàn**, và **chứng minh** thay vì khẳng định.
Một guard chưa ai thấy nổ là một guard chưa ai thấy.

### 3.2 Giá trị trả về của `tick` là một cái bẫy cho vòng lặp hiển nhiên

`tick` trả về **số kết quả đã commit**, không phải số việc đã làm — doc của chính
nó nói *"một tick chỉ đánh thức thứ gì đó thì trả 0"*, và **cố ý** như vậy, vì
**một caller lặp cho tới khi bằng 0 sẽ không bao giờ dừng trên một execution mà nó
vừa mở lại retry.**

Nên vòng lặp là **ticker theo chu kỳ cố định, không phải vòng rút-cho-cạn**, và
spec nói điều đó ở đúng chỗ người đọc sau sẽ tìm. Đây đúng lớp mà W8 tốn bốn vòng:
**một giá trị có nghĩa khác với điều tên nó gợi ra.**

### 3.3 Bật worker lên mới là rủi ro, và không phải rủi ro nhỏ

Gói này khiến hệ thống **thực sự chạy lần đầu tiên**. Hai mối nguy cụ thể, cả hai
đều đo được chứ không phải tưởng tượng:

- **`egret.container` và `egret.script.python` tồn tại như handler và KHÔNG nằm
  trong `with_core_tasks()`.** Đăng ký chúng nghĩa là chạy container, nhân danh
  một workspace, theo lịch. Đó chính là câu hỏi quản trị mà W5 câu hỏi mở 3 đã nêu
  cho plugin jar, đến sớm hơn. **W10 ship với bốn handler lõi và ghi phần còn lại
  như một quyết định riêng, có chủ ý.**
- **Database dev không có schema `workflow` nào cả** — đo ngày 2026-09-14:
  `pg_namespace` trên DB dev liệt kê **mười bốn** schema không thuộc `pg_`, và
  `workflow` không nằm trong đó (cùng truy vấn trả `false` cho sự hiện diện của
  nó — đó là control cho con số). **Tính năng này chưa bao giờ chạy trên stack
  dev**, nên *"chạy được ở máy tôi"* vẫn chưa phải câu ai đó nói ra được.

## 4. Bề mặt execution: dữ liệu thì giàu, endpoint thì bằng không

`workflow.executions` mang `state`, `history` (jsonb), `source` đã đóng băng, cùng
**mười một** cột thêm qua migration 0135–0143 — `inputs`, `seq`,
`execution_trigger`, `trigger_execution_id`/`trigger_run_ix`,
`parent_execution_id`/`parent_run_ix`, `loop_index`/`loop_value`,
`attempt_number`, `origin_offset_ms`.

`workflow.task_runs` mang **mười bảy** cột gốc — gồm `run_ix`, `task_ix`,
`task_id`, `type_id`, `parent_ix`, `rendered`, `state`, `history`, `attempts`,
`not_before_ms`, `claimed_at` — cộng **năm** cột thêm cùng dải: `outputs`,
`message`, `created_seq`, `ended_seq`, `resume_with`.

**Nên một gantt KHÔNG cần cột mới nào.** `created_seq`/`ended_seq` nằm trên
`task_runs`, và cùng với `execution.created_at` và `attempts[].started_at` của từng
run thì đó là toàn bộ đầu vào. **`history` KHÔNG mang ngày tháng** —
`worker::claim::states_json` ghi tên trạng thái trần, và `state.rs:254` nói rõ vì sao
không thêm trường `started_at` thứ hai: nó sẽ là bản sao có thể bất đồng với
`attempts[0].started_at`. Lý do chưa có gantt không phải thiếu dữ liệu — mà là thiếu
một **endpoint**.

W10 ship, theo hình dạng `egret-dataapi::management_router` đã dùng cho auth, phạm
vi workspace, feature gate và utoipa:

- **list execution**, lọc theo flow, trạng thái và cửa sổ thời gian, có phân trang;
- **get một execution** kèm các task run của nó;
- **khởi động một execution** — hôm nay cách **duy nhất** để bắt đầu một lần chạy
  trong production là webhook, và đó là lý do UI không có gì để bấm;
- **resume** (`worker::claim::resume` đã có), **kill**, và **replay**.

**Một điều phải chốt chứ không được mặc định:** `executions.source` là YAML đã đóng
băng, và `build_plan` thuần trên nó, nên một endpoint **topology** là một phép suy
ra, không phải một kho mới. Chuyện phục vụ đồ thị từ endpoint hay parse lại bằng
TypeScript là câu hỏi của W7 — nhưng spec này ghi lại phép đo khiến nó trả lời
được: parse lại bằng TS **nhân bản `spec::validate` và sẽ trôi lệch**, và hai bảng
buộc phải khớp nhau (`KESTRA_FLOW_CONTROL` và `kind_for`) chỉ được guard **trong
Rust, không ở đâu khác**.

## 5. Log là mảnh đất trống duy nhất, và nó to bằng một gói

Đo được:

- **Không có bảng `workflow.task_logs`** — đã quét mọi migration; bảng ấy nằm trong
  danh sách §6.1 của spec chương trình và **chưa bao giờ được tạo**.
- **`LogSink` có ba bản cài TRONG `src/` và không bản nào lưu một dòng nào**
  (bản thứ tư nằm trong một test container sống, `container_docker_live.rs:261` —
  con số được giới hạn phạm vi có chủ ý, vì một chữ "ba" không kèm phạm vi là SAI
  trên cả `crates/`): `TracingSink`
  (ghi ra `tracing`), `Recorder` (test), và `NameHintSink`.
- Handler container ghi **`stdOutLineCount` và `stdErrLineCount`** — **đếm dòng,
  không phải nội dung**.

Nên một panel Logs cần: một bảng, một sink biết lưu, chính sách retention, và một
API đọc có phân trang. Câu hỏi mở 1 ở §11 của spec chương trình đã chốt câu trả lời
cho v1 — *"log lines sống trong Postgres với một cửa sổ retention ở v1; object
storage là W9"* — và hình dạng retention để chép là phần prune audit đã có trong
`main.rs`: `scheduled_retention(days)` cộng một vòng lặp mỗi giờ.

**Đây là chỗ duy nhất W10 nên cân nhắc DỪNG.** Runtime và API execution mở khoá
gantt và topology cho W7; log chỉ mở khoá đúng panel log, và tốn một bảng, một sink
trên mọi đường handler, retention, và một API. **Khuyến nghị của spec: log là hạng
mục thứ ba của W10 CHỈ KHI gói chấp nhận to; nếu không, chúng là W11 và W7 ship
không có panel log — nói ra chứ không để người sau phát hiện.** Đó là quyết định
của người review, và nó được viết ở đây **như một quyết định**, không được giải
quyết lặng lẽ.

## 6. Quy tắc trung thành

Không đổi, với bốn điều mang trọng lượng ở đây:

- **Over-acceptance không được ship**, và ở đây nó có bộ mặt mới: một API execution
  khởi động một flow mà người gọi không có quyền đọc, hoặc list execution xuyên
  workspace. Mỗi endpoint như thế là lớp lỗi `chart_id` đang chờ xảy ra — xem §6.2
  của spec chương trình và [[egret-report-target-cross-workspace-leak]].
  **Phạm vi workspace là yêu cầu CẤU TRÚC, không phải trách nhiệm của từng handler.**
- **Một cái bank không mang lý do, và một cái gate xanh cũng vậy.** W10 **không có
  ratchet nào để dựa vào**, nên mọi khẳng định nó đưa ra phải được ghim bằng một
  test **đỏ khi khẳng định ấy sai**.
- **Một probe không định vị được thứ nó phát hiện** — khi một mutation trên vị từ
  sống sót, hãy **liệt kê mọi caller** trước khi kết luận điều gì về vị từ. Defect
  ship thứ ba của W8 là một caller bỏ qua cổng, mà chẩn đoán đầu tiên là vị từ
  thiếu mệnh đề.
- **Một số 0 đứng một mình là bất khả phủ chứng.** Gói này sẽ báo vài số 0 — ratchet
  không dịch, không entry nào bị ảnh hưởng — và **mỗi cái phải xuất hiện cạnh một
  con số có thể bất đồng**.

## 7. Rủi ro

1. **Ratchet không phán xét được gói này.** Ba gói đã huấn luyện mọi người đọc một
   con số; W10 không có, và rủi ro là sự vắng mặt ấy bị đọc thành *"không có gì
   thay đổi"* thay vì *"dụng cụ không chĩa về hướng này"*.
2. **Bật worker lên làm lộ ra mọi tương tác chưa từng chạy.** Các tính chất đã được
   chứng minh; phần wiring thì chưa. Hãy chờ đợi những defect thật đầu tiên của cả
   chương trình nằm ở startup, shutdown, config và back-pressure, chứ không ở
   `advance()`.
3. **API execution là một bề mặt phân quyền MỚI** trên dữ liệu chưa từng được phơi
   ra. Nó cũng là chỗ đầu tiên một ranh giới workspace có thể rò trong hệ con này.
4. **Log sẽ phình ra chiếm cả gói** nếu không được giới hạn hoặc hoãn một cách
   tường minh (§5).
5. **W7 bị chặn cho tới khi ít nhất §3 và §4 vào**, nên bất kỳ lịch nào cho W7 bắt
   đầu ngay bây giờ đều sai đúng bằng độ dài của gói này.

## 8. Hạng mục phải giao

- Một ticker worker trong `egret-bin`, có cờ config, `MissedTickBehavior::Skip`,
  nhận huỷ, **kèm một phát biểu an toàn VIẾT RA cho từng lời gọi `tick`, `wake`,
  `due_retries` và `reap`** thay vì thừa hưởng từ claim.
- Reaper trên chu kỳ riêng, có nêu `working_timeout_s`.
- Một bề mặt REST cho execution: list, get kèm task run, start, resume, kill,
  replay — phạm vi workspace **theo cấu trúc**, có feature gate, có utoipa, và sinh
  lại client frontend bằng `npm run gen:api` (**không bao giờ `orval` trần**, vì nó
  bỏ mất `encodeURIComponent`).
- **Một test end-to-end publish → start → quan sát tới trạng thái kết thúc, KHÔNG
  harness nào lái tick** — tiêu đề của cả gói.
- `workflow.task_logs`, một `LogSink` biết lưu, retention và một API đọc — **hoặc
  một lời hoãn tường minh sang W11, ghi lại kèm phép đo** (§5).
- **Bảng package §9 của spec chương trình được sửa**, để cả bốn hạng mục trên có chủ,
  và phát hiện ở §1 được ghi ở đúng chỗ người đọc bảng sau này sẽ thấy.
- Một mục trong tuyên bố độ phủ, cùng giọng với phần trigger của W6, nói rằng
  **oracle không thể phát ngôn gì cho gói này** và vì sao.
