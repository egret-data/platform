# W11 — Log của execution: mọi dòng một task in ra đều được giữ và đọc lại được

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-09-27-workflow-w11-execution-logs-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, chờ duyệt
**Phụ thuộc:** W10 (runtime, bề mặt REST của execution), đã merge. Nền là W13b (#598).
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (dòng W11) và §11 câu hỏi mở 1
**Đã quyết với người dùng, 2026-09-27:**
- **Phạm vi:** lưu mọi dòng: `Log`/`Fail` và thông báo của handler, **và mọi dòng stdout/stderr của task container và python**. Đọc bằng **polling** một API có con trỏ. **Không SSE.**
- **Thời hạn giữ:** **một cấu hình chung**, mặc định 30 ngày, một ticker nền xoá.
- **Đường ghi:** **writer nền theo lô**. Khi kênh đầy, handler **chờ**; không dòng nào bị bỏ âm thầm.
- **Giới hạn:** một dòng **cắt ở 64 KiB**, một lần thử của một task run giữ tối đa **10.000 dòng hoặc 10 MiB**. Vượt trần thì ngừng lưu, task vẫn chạy tiếp, và **một dòng WARN tóm tắt** nói bao nhiêu không được lưu. Cả ba giới hạn là cấu hình chung.

**Bàn giao:** một PR (tài liệu + mã), các task ở §8.

## 1. Cái gì đã có, cái gì chưa

| phần | trạng thái trên `origin/main` (`6749a37b`) |
|---|---|
| `workflow.task_logs` | **chưa có**. Schema `workflow` không có bảng log nào |
| `LogSink` (`worker/handlers/mod.rs:64`) | trait **đồng bộ** với ba bản cài: `TracingSink` (mặc định đang ship, ghi ra `tracing`), `Recorder` chỉ trong `#[cfg(test)]`, và `NameHintSink` (chú thích rồi chuyển tiếp). **Không cái nào lưu một dòng** |
| nối sink | mỗi handler tự gọi `default_sink()` trong `::new()` của nó (`log.rs`, `fail.rs`, `container.rs`, `python.rs`). Không có gì trong `egret-bin` dựng một sink dùng chung |
| output container / python | `worker/commands.rs` đọc stdout và stderr **từng dòng ngay khi tới** và đưa mỗi dòng cho `ingest_line`. Hàm này **đếm** dòng (`stdOutLineCount`/`stdErrLineCount`), parse marker `::{"outputs":…}::`, và trả về một mức (stdout INFO, stderr WARN). Nội dung chỉ đi ra `tracing`, không đi đâu khác |
| transaction của handler | `claim::run_one` chạy handler **trước** `pool.begin()`; transaction chỉ bao `apply_outcome`. Handler container và python **không có ngân sách thời gian mặc định** |
| tiền lệ | `egret-audit`: `try_send` đồng bộ vào một `mpsc` có giới hạn, cộng một writer theo lô được spawn (nó **bỏ** sự kiện khi kênh đầy). `egret-automation`: `log_retention_days` theo từng schedule, xoá ngay trong tick của scheduler. Ticker W10/W12: `interval` + `MissedTickBehavior::Skip` + `CancellationToken` |
| bề mặt đọc | các route execution của W10 (`routes.rs`), phân trang offset (`DEFAULT_PAGE_LIMIT` 50, tối đa 200). **egret chưa có endpoint SSE nào** |
| migration mới nhất | `0158`; W11 lấy **`0159`** |

## 2. Kestra làm gì, và egret khác ở đâu

Kestra (checkout `96893b589`, image `kestra/kestra:v2.0.0-rc13`):
- **Model:** `LogEntry` gồm tenant, namespace, flow, task, execution, task run, attempt, trigger, timestamp, level, thread, message, execution kind. Trên Postgres nó nằm trong bảng `logs` dạng JSONB với các cột sinh sẵn, có index trên `(execution_id, task_id)`, `(execution_id, taskrun_id)` và thời gian/mức, cộng một index GIN toàn văn.
- **API đọc:** `LogController` phục vụ `GET /{executionId}` (mọi dòng, không phân trang), `/download` (văn bản thô), `/follow` (SSE, xem trực tiếp), `/search` (có phân trang) và các lệnh xoá.
- **Thời hạn giữ:** **không có** cơ chế tự xoá. Người dùng tự viết task `PurgeLogs` trong một flow.
- **Output của script:** các dòng stdout và stderr thành dòng log qua `AbstractLogConsumer`. Marker `##kestra:log:debug##` đóng khung phần DEBUG. Đọc mã nguồn thì stderr ra **ERROR**, Task 1 đo được **ERROR**, còn egret hiện nói **WARN** (§5).
- **Giới hạn:** **không có** trần theo tổng lượng. Một dòng dài hơn 15.360 ký tự bị **chia** thành nhiều dòng cùng một timestamp (`RunContextLogger`, Task 1 đã đo), và API trả chúng về không theo thứ tự.

Các chỗ khác có chủ đích, đều đã chọn cùng người dùng:

| Kestra | egret W11 | lý do |
|---|---|---|
| không tự xoá; người dùng tự viết `PurgeLogs` | giữ theo cấu hình chung, ticker xoá | một bảng chỉ lớn lên là một sự cố Postgres bị hoãn lại; egret dùng chung một database |
| không có trần tổng lượng; dòng quá 15.360 ký tự bị chia làm nhiều dòng | một dòng bị cắt ở 64 KiB (một hàng, `truncated`), 10.000 dòng / 10 MiB mỗi lần thử của task run | một script in vòng lặp không được làm đầy database dùng chung |
| SSE `follow` | polling bằng con trỏ (§6) | egret chưa có SSE ở đâu; polling đạt cùng mục tiêu "thấy log khi task đang chạy", trễ khoảng 2 giây |
| `thread`, `triggerId`, `executionKind` | không lưu | egret không có các khái niệm này |
| `GET /{executionId}` không phân trang | phân trang bằng con trỏ | một execution có thể chứa 10 MiB cho mỗi lần thử |

## 3. Lưu trữ

Migration `0159_workflow_task_logs.sql`:

```sql
CREATE TABLE workflow.task_logs (
    id            bigserial   PRIMARY KEY,
    workspace_id  uuid        NOT NULL,
    execution_id  uuid        NOT NULL REFERENCES workflow.executions(id) ON DELETE CASCADE,
    task_run_id   uuid        NULL,      -- NULL: dòng nói về execution, không thuộc một run
    task_id       text        NULL,
    attempt       integer     NULL,
    level         smallint    NOT NULL CHECK (level BETWEEN 0 AND 4),  -- TRACE..ERROR
    ts            timestamptz NOT NULL,  -- lúc dòng được sinh ra
    inserted_at   timestamptz NOT NULL DEFAULT now(),
    message       text        NOT NULL,
    truncated     boolean     NOT NULL DEFAULT false
);
CREATE INDEX task_logs_execution_id ON workflow.task_logs (execution_id, id);
CREATE INDEX task_logs_ts ON workflow.task_logs (ts);
```

*Đây là hình dạng cần kiểm lại, không phải văn bản để dán.* Plan sẽ kiểm:
- kiểu thật của `executions.id`;
- FK tới `workspace_id` có phải là mẫu chung của codebase không;
- `task_run_id` nên tham chiếu `task_runs` hay để không ràng buộc. Một FK làm mỗi lần insert phải lấy khoá trên `task_runs`, trong khi worker giữ các hàng `task_runs` bằng `FOR UPDATE`.

`ts` là lúc dòng được sinh ra (giờ của worker khi gọi `log`). `inserted_at` là giờ của database, và chỉ quy tắc con trỏ ở §6 đọc nó.

## 4. Đường ghi

### 4.1 `LogSink` chuyển sang async

```rust
#[async_trait]
pub trait LogSink: Send + Sync + std::fmt::Debug {
    async fn write(&self, line: LogLine);
}
```

Trait đổi từ `fn log(&self, task, level, message)` sang một method async nhận một dòng sở hữu. Đây là cách duy nhất để có cả hai điều người dùng chọn:
- handler **chờ** khi kênh đầy;
- không **chặn luồng tokio** nào.

Một `log` đồng bộ chỉ có thể bỏ dòng (`try_send`) hoặc chặn runtime (`blocking_send`). Mọi chỗ gọi hiện tại vốn đã nằm trong handler async.

### 4.2 `RunLog`: một handle cho mỗi lần thử của task run

Worker dựng một `RunLog` cho mỗi run được claim, trong `claim::run_one`, trước khi handler bắt đầu. Nó mang:
- danh tính của run: workspace, execution, task run, task id, attempt;
- sink dùng chung;
- các bộ đếm cho trần ở §5.

Handler ghi qua nó (`run_log.info(msg)`, `run_log.line(level, msg)`). `NameHintSink` trở thành một lớp bọc quanh `RunLog`. Sau khi handler trả về, `run_one` gọi `RunLog::finish()` trên mọi đường: thành công, thất bại, `Err`, hết giờ và bị huỷ. `finish()` ghi dòng tóm tắt của §5 nếu có dòng nào không được lưu. Handle tới tay handler bằng cách nào (qua `TaskInput` hay một tham số riêng) được quyết trong plan, đối chiếu với chữ ký của `cancel::guarded`.

### 4.3 `PgLogSink` và writer của nó

- Sink gửi vào một kênh `mpsc` **có giới hạn**, sức chứa 10.000 dòng, bằng `send().await`. Kênh đầy thì handler đang ghi chậm lại; không bao giờ bỏ dòng.
- Một task writer, spawn trong `egret-bin` cạnh các ticker của W10, rút dòng khỏi kênh. Nó insert một lô khi đã giữ **500 dòng** hoặc khi đã qua **1 giây** kể từ dòng chưa ghi đầu tiên, cái nào tới trước. Mỗi lô là một câu lệnh: `INSERT … SELECT * FROM unnest($1::uuid[], …)`.
- **Ghi lỗi:** thử lại kèm backoff (100 ms, gấp đôi, trần 5 giây) trong tối đa 30 giây cho mỗi lô. Kênh đầy dần trong lúc thử lại, nên handler chậm lại chứ dòng không biến mất. Nếu lô vẫn hỏng, `tracing::error!` ghi tên các execution id và số dòng mất, lô bị bỏ, và writer chạy tiếp. **Việc ghi log không bao giờ đổi kết quả của một run.**
- **Tắt máy:** khi bị huỷ, writer ngừng nhận, rút hết những gì còn trong kênh, ghi xong, rồi thoát trong thời gian có giới hạn (10 giây). Runtime đợi nó sau khi ticker của worker đã dừng.
- **Nối dây:** `Handlers` (registry) giữ sink. Constructor của handler nhận sink thay vì gọi `default_sink()`. `TracingSink` được giữ lại cho unit test và cho trường hợp worker chạy không có database.

Writer chạy khi `worker_enabled` bật, vì worker là nơi duy nhất sinh dòng log.

### 4.4 Nguồn của các dòng

| nguồn | mức |
|---|---|
| `core.log.Log` | `level:` được khai báo (đã đo ở W3) |
| `core.execution.Fail` | ERROR, chính `errorMessage` (đã đo; dòng stack trace DEBUG của Kestra không được tái tạo) |
| stdout của container / python | INFO; **DEBUG** giữa `##kestra:log:debug##` và `##kestra:log:info##` (đã đo; các dòng marker không được lưu) |
| stderr của container / python | **ERROR** (đã đo, §5), kể cả bên trong khung debug |
| marker `::{…}::` chiếm trọn dòng nhưng không hợp lệ | WARN `Invalid outputs '…'` (dạng đã đo, §5) |
| dòng bao quanh của worker ("started"/"ended") | không có (đã đo: Kestra không lưu dòng nào như vậy) |
| thông báo handler đang gửi qua sink | giữ nguyên |
| dòng tóm tắt vượt trần ở §5 | WARN |

Các dòng bị tiêu thụ làm marker `::{…}::` chiếm trọn dòng **không được lưu** (đã đo, §5).

## 5. Giới hạn, và mức log theo phép đo

**Cắt dòng.** Một dòng dài hơn `log_line_max_bytes` (mặc định 65.536) bị cắt tại ranh giới ký tự UTF-8 cuối cùng không vượt giới hạn, và được lưu với `truncated = true`.

**Trần mỗi lần thử.** `RunLog` đếm số dòng và số byte đã gửi. Dòng đầu tiên làm vượt `log_run_max_lines` (mặc định 10.000) hoặc `log_run_max_bytes` (mặc định 10 MiB) tắt việc lưu cho lần thử đó. Việc đếm vẫn tiếp tục. Khi `finish()`, nếu có dòng nào không được lưu, egret ghi đúng một dòng WARN, **nằm ngoài** trần:

> *N lines (M bytes) of this attempt were not stored: over the per-run log cap (10000 lines / 10485760 bytes).*

Task vẫn chạy tiếp và kết quả không đổi. Giới hạn tính theo từng lần thử, nên một lần retry bắt đầu lại từ đầu.

**Mức log, đã đo.** Task 1 đo các điểm sau trên artifact (fixture `crates/egret-workflow/oracle-exec/w11-logs.json`, plugin shell 1.9.8, runner Process và Docker). Comment của egret trong `commands.rs` hiểu `warningOnStdErr` của Kestra là núm chỉnh *mức log* của stderr, và lời tự bào chữa của nó, *"nothing here reads a level back"*, không còn đúng ở W11, vì `min_level` đọc lại mức log. Kết quả đo:
1. **stderr là ERROR**, không phải WARN, bất kể `warningOnStdErr` (mặc định, `false` và `true` đều ghi ERROR). egret đổi WARN thành ERROR và vẫn không cung cấp `warningOnStdErr`.
2. **Marker `::{json}::` chiếm trọn dòng không được lưu**, trên stdout lẫn stderr, và `outputs` của nó vẫn vào output của task. Điều này đúng với mọi object JSON, không riêng object có `outputs` (dòng `::{"metrics":[]}::` cũng không được lưu). Dòng chỉ chứa marker ở giữa (`x ::{…}::`, hay có dấu cách ở cuối) là dòng thường. Marker chiếm trọn dòng mà JSON không parse được thì không được lưu; Kestra ghi thay vào đó một dòng WARN `Invalid outputs '<lỗi>'`, còn egret ghi một dòng WARN bắt đầu bằng `Invalid outputs '` với nội dung lỗi của serde (đã chọn: không có dòng stack trace). Comment "consumed as an output AND logged" của egret nói sai về Kestra.
3. **Không có dòng bao quanh của worker** trong log của execution. `Type {} started` và `Flow completed with state {}` đi vào logger riêng của server. Runner script của Kestra tự thêm các dòng chẩn đoán DEBUG/TRACE (`Starting command with pid …`, `Command succeed with exit code 0`, id container và volume); egret **không** tái tạo chúng (đã chọn).
4. **`attemptNumber` đếm từ 0**: các dòng của lần thử đầu mang 0, lần retry mang 1, cùng một task run id. Nó là chỉ số trong `task_runs.attempts`. `TaskInput.attempt` của egret đếm từ 1, nên `task_logs.attempt = TaskInput.attempt - 1` (cách quy đổi là đã chọn, cách đánh số là đã đo).
5. **Khung `##kestra:log:debug##` có áp dụng**, trên cả hai runner: các dòng stdout sau nó là DEBUG cho tới `##kestra:log:info##`, và cả hai dòng marker đều không được lưu. Dòng stderr bên trong khung vẫn là ERROR. Một chế độ cho mỗi lệnh, bật/tắt bởi marker trên stream nào cũng được.
6. **Dòng dài bị chia, không bị cắt:** Kestra lưu một dòng 100.000 ký tự thành các mảnh 15.360 ký tự cùng một timestamp, và đọc lại chúng không theo thứ tự. egret thì cắt ở 65.536 byte (ở trên); đây là khác biệt có chủ đích.

Mã làm theo phép đo, và mọi chỗ bất đồng với mã Java đều được ghi lại.

## 6. API đọc

```
GET /api/ws/{workspace}/workflows/executions/{id}/logs
    ?after=<id>&limit=<n>&task_id=<s>&task_run_id=<uuid>&min_level=<TRACE..ERROR>
→ 200 { items: [ {id, task_id, task_run_id, attempt, level, ts, message, truncated} ],
        next_after: <id|null>, execution_state: <State> }
```

- **Phân quyền:** giống hệt `GET /executions/{id}` của W10: `WsContext`, `authz::require`, và cổng feature `workflows`. Câu query lọc **cả** `execution_id` lẫn `workspace_id`, nên execution của workspace khác là **404**. Có test chứng minh điều đó.
- **Phân trang:** `limit` mặc định 500, tối đa 2.000. Các hàng trả theo thứ tự `id`. `next_after` là `id` cuối cùng được trả, hoặc `null` khi không trả gì.
- **Hết dữ liệu:** khi `execution_state` đã kết thúc và một lần gọi không trả dòng nào, client đã có đủ và dừng polling.
- **Vì sao dùng con trỏ chứ không offset như W10:** log chỉ ghi thêm và lớn dần trong lúc đang được đọc. Phân trang offset trên một tập đang lớn sẽ lặp hoặc sót hàng.

**Cái bẫy commit lệch thứ tự, và độ trễ lắng.** `bigserial` cấp id lúc insert rồi commit sau. Với hai writer (hai bản egret, hoặc lần thử lại của một writer chồng lên lô kế tiếp), id 100 có thể commit sau id 101. Một client đã đọc 101 và dời con trỏ qua đó sẽ không bao giờ thấy 100. Nên câu query chỉ trả các hàng có `inserted_at < now() - interval '2 seconds'`.
- Một lô là một transaction ngắn, nên 2 giây là một khoảng rộng rãi.
- Cái giá là một dòng hiện ra khoảng 2 giây sau khi được ghi.
- Một test dựng hai transaction commit ngược thứ tự id và kiểm rằng client polling đọc được cả hai.
- Độ trễ và lý do được ghi vào doc comment của route.

Tài liệu hoá bằng utoipa; client frontend được sinh lại bằng `npm run gen:api`. **Panel log thuộc W7**; W11 chỉ giao API.

## 7. Thời hạn giữ

- Cấu hình: `workflow.log_retention_days`, mặc định **30**; `0` nghĩa là giữ mãi.
- Một ticker trong `egret_workflow::runtime`, khởi động khi `worker_enabled`:
  - chạy mỗi **giờ**, `MissedTickBehavior::Skip`, dừng được khi bị huỷ;
  - xoá theo lô **10.000** hàng (`DELETE … WHERE id IN (SELECT id … WHERE ts < now() - make_interval(days => $1) LIMIT 10000)`);
  - lặp tới khi một lô xoá ít hơn số đó hoặc hết ngân sách 30 giây của lượt.
- Không câu lệnh nào giữ khoá lâu. Nhiều bản chạy đồng thời cũng vô hại, vì xoá một hàng hai lần không làm gì.
- `ON DELETE CASCADE` xoá các dòng của một execution cùng với execution đó.

Cấu hình (`egret_config::WorkflowConfig`, biến môi trường `EGRET_WORKFLOW__…`): `log_retention_days` (30), `log_line_max_bytes` (65536), `log_run_max_lines` (10000), `log_run_max_bytes` (10485760).

## 8. Các task

1. **Đo trên artifact** (các câu 1–5 ở §5); fixture `oracle-exec/w11-logs.json`.
2. **Migration `0159` và `LogRepo`:** insert theo lô, đọc theo con trỏ có độ trễ lắng, xoá theo lô.
3. **Phần log thuần:**
   - `LogSink` async, `LogLine`, `RunLog` (cắt dòng, trần, tóm tắt khi `finish`);
   - bốn handler và `ingest_line` chuyển sang `RunLog`;
   - registry mang sink;
   - mức log theo Task 1.
4. **Writer `PgLogSink` và ticker giữ log:** chờ khi kênh đầy, ghi theo lô, thử lại, rút kênh khi tắt máy; nối vào `egret-bin`; các khoá cấu hình.
5. **API đọc:** route, utoipa, `gen:api`.
6. **Tuyên bố độ phủ, rà EN/VI, CI cục bộ đầy đủ.** Việc rà tìm những câu W11 làm cho sai, ví dụ *"none of the three persists a line"* và *"W7 ships without a logs panel"* trong spec chương trình §9, và comment về stderr trong `commands.rs`.

## 9. Kiểm thử

Chạy trên Postgres thật, mỗi test đi kèm một mutation phải bị giết:

| test | mutation nó giết |
|---|---|
| đầu-cuối: publish và chạy một flow gồm `Log` + một container in cả stdout lẫn stderr, không có gì điều khiển tick bằng tay; đọc lại qua API đúng thứ tự và đúng mức đo được | handler container không ghi qua `RunLog` |
| kênh đầy với writer bị làm chậm: handler chờ và **không mất dòng nào** | `try_send` bỏ dòng thay vì `send().await` |
| vượt trần: dòng lưu dừng ở trần, đúng **một** dòng WARN tóm tắt, task thành công | dòng tóm tắt ghi bên trong trần / không ghi trên đường `Err` |
| cắt dòng tại ranh giới ký tự nhiều byte | cắt theo số byte |
| commit lệch thứ tự: hai transaction commit ngược thứ tự id và client polling đọc được cả hai | bỏ độ trễ lắng |
| tắt máy thì kênh được rút hết | thoát mà không rút kênh |
| giữ log chỉ xoá dòng cũ hơn hạn; `0` không xoá gì | bỏ điều kiện `ts` |
| execution của workspace khác → 404 | bỏ `workspace_id` khỏi câu query |
| `finish()` chạy khi hết giờ và khi bị huỷ | chỉ gọi nó khi thành công |

**Ratchet:** kỳ vọng **109 → 109**, đo chứ không khẳng định. Oracle so trace, không so dòng log.

## 10. Ngoài phạm vi

SSE / `follow`; `/download`; tìm kiếm toàn văn; xoá log thủ công; `logLevel` theo flow; mức ghi đè thời hạn giữ theo workspace (cấu hình chung chừa chỗ cho nó mà không phải đổi schema); object storage (W9); `PurgeLogs`.

## 11. Giới hạn đã biết, nói rõ

- **Có thể mất dòng** khi process chết: những dòng còn trong kênh, và những dòng thuộc một lô đã hết lượt thử lại. Cả hai được ghi tên trong `tracing` ở chỗ ghi hỏng; còn một process bị kill thì không tự báo được mình mất gì.
- **Một run chạy hai lần** (bị reap trong khi handler vẫn chạy, mối nguy W4 đã ghi lại) ghi dòng của cả hai lần dưới cùng một attempt.
- Một dòng hiện ra khoảng **2 giây** sau khi được ghi (§6).
- **Production chưa chạy task container hay python nào.** `egret-bin` đăng ký `Handlers::with_core_tasks()` (`Log`, `Return`, `Fail`, `Sleep`), và chính comment của nó nói chín trên mười ba kiểu có thể publish bị treo trên production. W11 lưu và test stdout/stderr qua registry dựng trong test; trên production hiện nay, các dòng được lưu đến từ `Log`/`Fail` và thông báo của handler. Việc nối container, python và sql vào production là một gói riêng, vì cách server được phép chạy container là một quyết định bảo mật riêng (quyết định của người dùng, 2026-09-27).
