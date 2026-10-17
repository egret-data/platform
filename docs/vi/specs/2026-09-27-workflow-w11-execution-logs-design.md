# W11 — Log của execution: mọi dòng một task in ra đều được giữ và đọc lại được

> Bản tham khảo tiếng Việt. Bản chính là `docs/en/specs/2026-09-27-workflow-w11-execution-logs-design.md`; khi hai bản khác nhau, bản tiếng Anh thắng.

**Trạng thái:** thiết kế, chờ duyệt
**Phụ thuộc:** W10 (runtime, bề mặt REST của execution), đã merge. Nền là W13b (#598).
**Spec chương trình:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9 (dòng W11) và §11 câu hỏi mở 1
**Đã quyết với người dùng, 2026-09-27:**
- **Phạm vi:** lưu mọi dòng: `Log`/`Fail` và thông báo của handler, **và mọi dòng stdout/stderr của task container và python**. Đọc bằng **polling** một API có con trỏ. **Không SSE.**
- **Thời hạn giữ:** **một cấu hình chung**, mặc định 30 ngày, một ticker nền xoá.
- **Đường ghi:** **writer nền theo lô**. Khi kênh đầy, handler **chờ**; không dòng nào bị bỏ âm thầm. **Đã sửa cùng người dùng, 2026-09-28:** trong khi writer đang hỏng (cầu dao đang bật), một lần ghi gặp kênh đầy thì bỏ dòng và đếm thay vì chờ, để việc ghi log không bao giờ đẩy một run vượt qua `timeout:` của nó hay làm treo worker; số dòng bị bỏ được báo qua `tracing::error!` khi writer hồi phục hoặc rút cạn lúc tắt máy. Khi writer khoẻ, handler vẫn chờ.
- **Giới hạn:** một lần thử của một task run giữ tối đa **10.000 dòng hoặc 10 MiB**. Vượt trần thì ngừng lưu, task vẫn chạy tiếp, và **một dòng WARN tóm tắt** nói bao nhiêu không được lưu. Cả hai trần là cấu hình chung.
- **Dòng dài (D6, quyết định sau khi Task 1 đo Kestra):** một dòng dài hơn **15.360 ký tự bị chia** thành nhiều dòng được lưu, đúng như Kestra chia; không mất chữ nào. Quyết định này thay cho quyết định đầu tiên là cắt ở 64 KiB. Độ dài mảnh là hằng số, không phải cấu hình.

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

Kestra (checkout Java `96893b589`; image đã đo là `kestra/kestra@sha256:28f59bf1…`, gắn tag `v2.0.0-rc13` nhưng nhãn ghi `version=nightly`, revision `169ca7f2`):
- **Model:** `LogEntry` gồm tenant, namespace, flow, task, execution, task run, attempt, trigger, timestamp, level, thread, message, execution kind. Trên Postgres nó nằm trong bảng `logs` dạng JSONB với các cột sinh sẵn, có index trên `(execution_id, task_id)`, `(execution_id, taskrun_id)` và thời gian/mức, cộng một index GIN toàn văn.
- **API đọc:** `LogController` phục vụ `GET /{executionId}` (mọi dòng, không phân trang), `/download` (văn bản thô), `/follow` (SSE, xem trực tiếp), `/search` (có phân trang) và các lệnh xoá.
- **Thời hạn giữ:** **không có** cơ chế tự xoá. Người dùng tự viết task `PurgeLogs` trong một flow.
- **Output của script:** các dòng stdout và stderr thành dòng log qua `AbstractLogConsumer`. Marker `##kestra:log:debug##` đóng khung phần DEBUG. Đọc mã nguồn thì stderr ra **ERROR**, Task 1 đo được **ERROR**, còn egret hiện nói **WARN** (§5).
- **Giới hạn:** **không có** trần theo tổng lượng. Một dòng dài hơn 15.360 ký tự bị **chia** thành nhiều dòng cùng một timestamp (`RunContextLogger`, Task 1 đã đo), và API có thể trả chúng về không theo thứ tự (đã xảy ra ở 2 trên 2 lần chạy).

Các chỗ khác có chủ đích, đều đã chọn cùng người dùng:

| Kestra | egret W11 | lý do |
|---|---|---|
| không tự xoá; người dùng tự viết `PurgeLogs` | giữ theo cấu hình chung, ticker xoá | một bảng chỉ lớn lên là một sự cố Postgres bị hoãn lại; egret dùng chung một database |
| không có trần tổng lượng | 10.000 dòng / 10 MiB mỗi lần thử của task run; dòng dài được chia đúng như Kestra chia (§5) | một script in vòng lặp không được làm đầy database dùng chung |
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
    message       text        NOT NULL
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

- Sink gửi vào một kênh `mpsc` **có giới hạn**, sức chứa 10.000 dòng. Khi writer khoẻ, kênh đầy thì handler đang ghi **chờ** có chỗ; không bao giờ bỏ dòng. Khi writer **đang hỏng** (cầu dao, bên dưới), kênh đầy thì dòng bị bỏ và được đếm.
- Một task writer, spawn trong `egret-bin` cạnh các ticker của W10, rút dòng khỏi kênh. Nó insert một lô khi đã giữ **500 dòng** hoặc khi đã qua **1 giây** kể từ dòng chưa ghi đầu tiên, cái nào tới trước. Mỗi lô là một câu lệnh: `INSERT … SELECT * FROM unnest($1::uuid[], …)`.
- **Ghi lỗi:** một lô bắt đầu khi writer còn khoẻ được thử lại kèm backoff (100 ms, gấp đôi, trần 5 giây) trong tối đa 30 giây. Nếu vẫn hỏng, `tracing::error!` ghi tên các execution id và số dòng mất, lô bị bỏ, và writer chạy tiếp. Một lô bắt đầu khi writer đang hỏng chỉ được thử **một** lần. SQLSTATE 23503 (một execution bị xoá giữa câu lệnh) được chạy lại ngay một lần, không bao giờ thử lại kèm backoff.
- **Một dòng Postgres từ chối** (SQLSTATE lớp 22, ví dụ một byte U+0000, 22021): không lần thử lại nào sửa được một giá trị, và thử lại sẽ giữ làm con tin các dòng khác của lô, thuộc bất kỳ execution và bất kỳ workspace nào. Nên lô được insert lại **từng dòng một**; chỉ (các) dòng bị từ chối bị bỏ, kèm một `tracing::error!` có đếm. `RunLog` xoá U+0000 khỏi mọi dòng nó dựng, giống Kestra (`JdbcJsonbUtils.java:13`), nên một dòng chỉ tới đường này nếu nó đi vòng qua `RunLog`.
- **Cầu dao.** Writer và mọi sink dùng chung một cờ **đang hỏng**. Cờ được bật bởi một lần insert hỏng vì lỗi thông thường và bởi một rào flush quá hạn; được tắt bởi lần insert thành công kế tiếp. Khi cờ bật: rào flush trả về ngay (có đếm, một `warn` cho mỗi đợt hỏng), và một rào đang chờ được thả ra; một `write` gặp kênh đầy thì bỏ dòng và đếm thay vì chờ (một `error!` khi bắt đầu bỏ, tổng số khi writer hồi phục hoặc dừng); mỗi lô mới chỉ được thử một lần, nên kênh vẫn được rút. Khi vận hành bình thường nó không đổi gì.
- **Việc ghi log có thể và không thể làm gì một run.** Nó không bao giờ đổi kết quả của run qua `Outcome`: một lần ghi hỏng được báo qua `tracing`. Nó có thể **làm chậm** kết quả tối đa bằng giới hạn của rào (5 giây), một lần cho mỗi đợt hỏng, cộng thời gian một lần insert cần để hỏng trước khi cầu dao bật. Nó không thể đẩy một task tới `timeout:` qua việc ghi log khi writer đang hỏng, vì lúc đó `write` không chờ. Một `write` **vẫn có thể** chờ khi writer khoẻ nhưng chậm hơn handler (backpressure của D3), và thời gian chờ đó tính vào `timeout:` của task như mọi việc khác task làm.
- **Tắt máy:** khi bị huỷ, writer ngừng nhận, rút hết những gì còn trong kênh, ghi xong, rồi thoát trong thời gian có giới hạn (10 giây). Runtime đợi nó sau khi ticker của worker đã dừng.
- **Rào flush:** `LogSink::flush()` mặc định không làm gì, nên `TracingSink` và các sink của test trả về ngay. `PgLogSink` gửi một dấu mốc vào kênh, đứng sau mọi dòng nó đã nhận. Kênh chở `Line(LogLine) | Flush(oneshot)`. Writer ghi lô hiện tại, với cách thử lại và cách chạy lại khi gặp 23503 như thường lệ, rồi mới xác nhận. `RunLog::finish()` chờ rào sau khi đã ghi dòng tóm tắt vượt trần. `claim::run_one` gọi `finish()` trước khi mở transaction ghi kết quả, nên **các dòng của một run đã được insert trước khi kết quả của nó, tức trạng thái kết thúc của execution, được commit**. Thời gian chờ bị chặn ở **5 giây** (`WriterConfig::flush_within`: một rào lúc khoẻ chờ mọi thứ đã xếp hàng trước nó, có thể tới cả kênh — khoảng 21 lần insert 500 dòng). Quá hạn đó, hoặc khi writer đã dừng, `finish()` ghi log rồi trả về, và lần quá hạn bật cầu dao. Khi cầu dao bật, rào không chờ chút nào. Vì vậy trong lúc ghi log gặp sự cố, kết quả của một run có thể được commit **trước khi các dòng cuối của nó được lưu, hoặc không có chúng**, và `complete` khi đó có thể đúng trong khi thiếu các dòng ấy (§11).
- **Tắt máy trong lúc ghi log gặp sự cố.** Tick của vòng claim không nhạy với huỷ, nên một tick đang chạy lúc SIGTERM sẽ chạy tới hết. Việc ghi log cộng thêm vào tick đó tối đa một giới hạn rào (5 giây) và một lần insert hỏng cho mỗi đợt hỏng, không phải 30 giây mỗi run, nên độ dài một tick là thời gian của các handler như trước W11.
- **Nối dây:** `Handlers` (registry) giữ sink. Constructor của handler nhận sink thay vì gọi `default_sink()`. `TracingSink` được giữ lại cho unit test và cho trường hợp worker chạy không có database.

Writer chạy khi `worker_enabled` bật, vì worker là nơi duy nhất sinh dòng log.

### 4.4 Nguồn của các dòng

| loại task / đường đi | ai ghi | các dòng (mức, nội dung) |
|---|---|---|
| `core.log.Log` | handler | mỗi message một dòng, ở `level:` được khai báo (đã đo ở W3 và Task 1) |
| `core.execution.Fail`, không có `condition` | chỉ `run_one` (handler không ghi gì) | một dòng: ERROR, chính `errorMessage`. Kestra ghi đúng một dòng ERROR như vậy (đã đo) |
| `core.execution.Fail`, `condition` đúng | handler, rồi `run_one` | hai dòng: ERROR `errorMessage` (handler), rồi ERROR `Fail on a condition` (`run_one`), đúng hai dòng của Kestra (`Fail.java:131-136` và `exceptionHandler`) |
| `core.execution.Fail`, `condition` sai | không ai | không có |
| stdout của container / python | handler (`ingest_line`) | INFO; **DEBUG** giữa `##kestra:log:debug##` và `##kestra:log:info##` (đã đo; các dòng marker không được lưu) |
| stderr của container / python | handler (`ingest_line`) | **ERROR** (đã đo, §5), kể cả bên trong khung debug |
| marker `::{…}::` chiếm trọn dòng nhưng không hợp lệ | handler (`ingest_line`) | WARN `Invalid outputs '…'` (dạng đã đo, §5) |
| mọi task có lần thử kết thúc `Failed` kèm thông điệp (handler trả `Ok(Failed)`, `Budget::Refused`, hết thời gian) | `run_one` (`RunLog::failure`) | **dòng lần-thử-thất-bại**: một dòng ERROR, thông điệp của `Outcome`. Dòng này **đi vòng qua trần**, nằm sau các dòng của handler và trước dòng tóm tắt. Không ghi khi handler trả `Err` (sự cố của egret, đã chọn), khi future bị drop (Kestra không ghi gì khi bị kill), hoặc khi `Outcome` `Failed` không có thông điệp |
| dòng bao quanh của worker ("started"/"ended") | không ai | không có (đã đo: Kestra không lưu dòng nào như vậy) |
| thông báo handler đang gửi qua sink | handler | giữ nguyên |
| dòng tóm tắt vượt trần ở §5 | `RunLog::finish` | WARN, sau dòng lần-thử-thất-bại, nằm ngoài trần |

**Dòng lần-thử-thất-bại so với Kestra.** `AbstractWorkerCallable.exceptionHandler` của Kestra ghi `e.getMessage()` ở ERROR cho mọi task ném exception, trừ khi task đã bị kill. Sau đó `RunContextLogger.logEntries` thêm một dòng ERROR thứ hai chứa thông điệp của chuỗi nguyên nhân khi exception có nguyên nhân, và một stack trace DEBUG. Bản ghi q4 (`Fail`) và q5 (script) cho thấy cả hai dạng. egret không có chuỗi nguyên nhân, nên chỉ ghi **dòng đầu tiên**; dòng nguyên nhân và stack trace là khác biệt đã nêu rõ. Nội dung trùng với Kestra cho `Fail` (`errorMessage`, `Fail on a condition`) và cho hết thời gian (`Timeout after HH:mm:ss.SSS`). Với mọi handler khác, đó là thông điệp thất bại riêng của egret (đã chọn). Với script, Kestra ghi `io.kestra…TaskException: Command failed with exit code N` rồi đến dòng nguyên nhân.

Các dòng bị tiêu thụ làm marker `::{…}::` chiếm trọn dòng **không được lưu** (đã đo, §5).

## 5. Giới hạn, và mức log theo phép đo

**Dòng dài bị chia (D6).** `RunLog` chia mọi dòng dài hơn **15.360 ký tự** thành các mảnh liên tiếp 15.360 ký tự, mảnh cuối ngắn hơn, đúng như `RunContextLogger` của Kestra (`MAX_MESSAGE_LENGTH = 1024 * 15`, `Splitter.fixedLength`; Task 1 đã đo: một dòng 100.000 ký tự thành 6 × 15.360 + 7.840). Mỗi mảnh là một dòng được lưu riêng, cùng `ts`, ghi theo thứ tự, nên các mảnh có id tăng dần và API trả chúng đúng thứ tự. Nối các mảnh lại được đúng dòng ban đầu. Mỗi mảnh tính vào trần mỗi lần thử như một dòng, với số byte của chính nó. Độ dài mảnh là hằng số `LOG_PIECE_CHARS = 15_360`, không phải cấu hình.

**Một ký tự là gì.** egret đếm **Unicode scalar value** (`char` của Rust), nên chỗ chia không bao giờ rơi vào giữa một ký tự và mảnh nào cũng là UTF-8 hợp lệ. Mã Java của Kestra đếm **UTF-16 code unit** (`String.length()`). Hai cách đếm trùng nhau với mọi ký tự trong Basic Multilingual Plane. Chúng khác nhau với ký tự nằm ngoài (emoji, một số chữ CJK): một ký tự như vậy là 1 với egret và 2 với Kestra, nên mảnh của egret có thể chứa nhiều ký tự như vậy hơn, và Kestra có thể chia đôi một cặp surrogate còn egret thì không. Đây là khác biệt có chủ đích và **chưa được đo**: dòng dài của Task 1 chỉ gồm ASCII.

**Trần mỗi lần thử.** `RunLog` đếm số dòng và số byte đã gửi. Dòng đầu tiên làm vượt `log_run_max_lines` (mặc định 10.000) hoặc `log_run_max_bytes` (mặc định 10 MiB) tắt việc lưu cho lần thử đó. Việc đếm vẫn tiếp tục. Khi `finish()`, nếu có dòng nào không được lưu, egret ghi đúng một dòng WARN, **nằm ngoài** trần:

> *N lines (M bytes) of this attempt were not stored: over the per-run log cap (10000 lines / 10485760 bytes).*

Task vẫn chạy tiếp và kết quả không đổi. Giới hạn tính theo từng lần thử, nên một lần retry bắt đầu lại từ đầu.

**Mức log, đã đo.** Task 1 đo các điểm sau trên artifact (fixture `crates/egret-workflow/oracle-exec/w11-logs.json`, plugin shell 1.9.8, runner Process và Docker). Comment của egret trong `commands.rs` hiểu `warningOnStdErr` của Kestra là núm chỉnh *mức log* của stderr, và lời tự bào chữa của nó, *"nothing here reads a level back"*, không còn đúng ở W11, vì `min_level` đọc lại mức log. Kết quả đo:
1. **stderr là ERROR**, không phải WARN, bất kể `warningOnStdErr` (mặc định, `false` và `true` đều ghi ERROR). egret đổi WARN thành ERROR và vẫn không cung cấp `warningOnStdErr`.
2. **Marker `::{json}::` chiếm trọn dòng không được lưu**, trên stdout lẫn stderr, và `outputs` của nó vẫn vào output của task. Điều này đúng với mọi object JSON, không riêng object có `outputs` (dòng `::{"metrics":[]}::` cũng không được lưu). Dòng có chứa marker nhưng không phải đúng một marker (`x ::{…}::`, hay marker có dấu cách theo sau) là dòng thường và được lưu. Marker chiếm trọn dòng mà JSON không parse được thì không được lưu; Kestra ghi thay vào đó một dòng WARN `Invalid outputs '<lỗi>'`, còn egret ghi một dòng WARN bắt đầu bằng `Invalid outputs '` với nội dung lỗi của serde (đã chọn: không có dòng stack trace). Comment "consumed as an output AND logged" của egret nói sai về Kestra.
3. **Không có dòng bao quanh của worker** trong log của execution. `Type {} started` và `Flow completed with state {}` đi vào logger riêng của server. Runner script của Kestra tự thêm các dòng chẩn đoán DEBUG/TRACE (`Starting command with pid …`, `Command succeed with exit code 0`, id container và volume); egret **không** tái tạo chúng (đã chọn).
4. **`attemptNumber` đếm từ 0**: các dòng của lần thử đầu mang 0, lần retry mang 1, cùng một task run id. Nó là chỉ số trong `task_runs.attempts`. `TaskInput.attempt` của egret đếm từ 1, nên `task_logs.attempt = TaskInput.attempt - 1` (cách quy đổi là đã chọn, cách đánh số là đã đo).
5. **Khung `##kestra:log:debug##` có áp dụng**, trên cả hai runner: các dòng stdout sau nó là DEBUG cho tới `##kestra:log:info##`, và cả hai dòng marker đều không được lưu. Dòng stderr bên trong khung vẫn là ERROR. Một chế độ cho mỗi lệnh, bật/tắt bởi marker trên stream nào cũng được.
6. **Dòng dài bị chia, không bị cắt:** Kestra lưu một dòng 100.000 ký tự thành các mảnh 15.360 ký tự cùng một timestamp, và đọc lại chúng không theo thứ tự. egret chia cùng cách đó (ở trên, D6) và giữ các mảnh đúng thứ tự.

Mã làm theo phép đo, và mọi chỗ bất đồng với mã Java đều được ghi lại.

## 6. API đọc

```
GET /api/ws/{workspace}/workflows/executions/{id}/logs
    ?after=<id>&limit=<n>&task_id=<s>&task_run_id=<uuid>&min_level=<TRACE..ERROR>
→ 200 { items: [ {id, task_id, task_run_id, attempt, level, ts, message} ],
        next_after: <id|null>, execution_state: <State>, complete: <bool> }
```

- **Phân quyền:** giống hệt `GET /executions/{id}` của W10: `WsContext`, `authz::require`, và cổng feature `workflows`. Câu query lọc **cả** `execution_id` lẫn `workspace_id`, nên execution của workspace khác là **404**. Có test chứng minh điều đó.
- **Phân trang:** `limit` mặc định 500, tối đa 2.000. Các hàng trả theo thứ tự `id`. `next_after` là `id` cuối cùng được trả, hoặc `null` khi không trả gì.
- **Hết dữ liệu:** `complete` đúng khi trạng thái execution đã kết thúc **và** `now() - executions.updated_at > 2 giây` (độ trễ lắng). Cả hai được tính bằng SQL trên cùng một hàng. **Client dừng polling khi `complete` đúng và `items` rỗng, không bao giờ chỉ dựa vào `execution_state` đã kết thúc.** Trạng thái kết thúc được commit sau khi các dòng của run đã được insert (rào flush ở §4.3). Vì vậy, khi độ trễ lắng cũng đã trôi qua kể từ lần commit đó, mọi dòng đều đã thấy được. Nếu không có rào, client có thể đọc được trạng thái kết thúc trong lúc các dòng cuối còn nằm trong kênh, trong một lô chưa flush (≤ 1 giây) hoặc đang được thử lại (≤ 30 giây). Client khi đó sẽ dừng và âm thầm mất phần đuôi. Rào chỉ cho bảo đảm này khi writer khoẻ; §11 liệt kê khi nào nó không đúng. Route đọc execution bằng một query một hàng (tồn tại, quyền sở hữu, trạng thái, `complete`). Nó không nạp source đã đóng băng hay các task run, vì panel poll nó liên tục.
- **Vì sao dùng con trỏ chứ không offset như W10:** log chỉ ghi thêm và lớn dần trong lúc đang được đọc. Phân trang offset trên một tập đang lớn sẽ lặp hoặc sót hàng.

**Cái bẫy commit lệch thứ tự, và độ trễ lắng.** `bigserial` cấp id lúc insert rồi commit sau. Với hai writer (hai bản egret; một writer chạy tuần tự nên không tự chồng lên mình), id 100 có thể commit sau id 101, và một câu lệnh bắt đầu sau có thể giữ id thấp hơn một số id của câu lệnh bắt đầu trước. Một client đã đọc 101 và dời con trỏ qua đó sẽ không bao giờ thấy 100. Nên trang là một **ngưỡng nước (watermark)**: nó trả các hàng theo thứ tự id và dừng trước hàng đầu tiên sau con trỏ có `inserted_at` chưa cũ hơn `now() - interval '2 seconds'`, bất kể bộ lọc. Lọc từng hàng là không đủ: nó có thể trả một id cao đã lắng trong khi một id thấp hơn còn đang lắng.
- **Giới hạn:** một transaction ghi phải ngắn hơn **nửa** độ trễ (1 giây). Một hàng chưa commit thì watermark không thấy; nó chỉ có thể giữ id thấp hơn một hàng đã trả nếu transaction của nó bắt đầu trước khi transaction của hàng đó kết thúc, nên nó commit trong vòng hai lần thời lượng ghi kể từ lúc hàng đó bắt đầu. Một lô là một câu `INSERT` ngắn.
- Cái giá là một dòng hiện ra khoảng 2 giây sau khi được ghi.
- Hai test: hai transaction commit ngược thứ tự id, đọc bởi một client polling phải nhận cả hai; và ba hàng đã commit mà chỉ hàng thấp nhất và cao nhất đã lắng, nơi client không được tiến qua hàng giữa.
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

Cấu hình (`egret_config::WorkflowConfig`, biến môi trường `EGRET_WORKFLOW__…`): `log_retention_days` (30), `log_run_max_lines` (10000), `log_run_max_bytes` (10485760).

## 8. Các task

1. **Đo trên artifact** (các câu 1–5 ở §5); fixture `oracle-exec/w11-logs.json`.
2. **Migration `0159` và `LogRepo`:** insert theo lô, đọc theo con trỏ có độ trễ lắng, xoá theo lô.
3. **Phần log thuần:**
   - `LogSink` async, `LogLine`, `RunLog` (chia dòng dài, trần, tóm tắt khi `finish`);
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
| đầu-cuối: publish và chạy một flow gồm `Log` + `Fail` qua runtime thật, không có gì điều khiển tick bằng tay, rồi đọc lại các dòng từ Postgres đúng thứ tự và đúng mức. **stdout/stderr của container và python được test riêng** (`tests/container_docker_live.rs`, trên Docker daemon thật): tới `RunLog` đúng mức đo được, ghi vào một sink của test; không test nào lưu một dòng container vào Postgres | đường `Log` / thất bại không ghi qua `RunLog` |
| kênh đầy với writer bị làm chậm: handler chờ và **không mất dòng nào** | `try_send` bỏ dòng thay vì `send().await` |
| vượt trần: dòng lưu dừng ở trần, đúng **một** dòng WARN tóm tắt, task thành công | dòng tóm tắt ghi bên trong trần / không ghi trên đường `Err` |
| một lần thử thất bại khi đã vượt trần: các dòng đã lưu của handler, rồi đúng một dòng ERROR lần-thử-thất-bại, rồi dòng tóm tắt; `Fail` không điều kiện lưu một dòng ERROR, `Fail` có điều kiện lưu hai | dòng thất bại bị tính vào trần / ghi sau dòng tóm tắt / ghi trên đường `Err` / `Fail` ghi `errorMessage` hai lần |
| một dòng 40.000 ký tự có các ký tự nhiều byte ở ngay và quanh ký tự thứ 15.360 → 3 dòng được lưu đúng thứ tự (15.360 / 15.360 / 9.280 ký tự), nối lại bằng đúng dòng ban đầu | chia theo số byte thay vì theo số `char` |
| commit lệch thứ tự: hai transaction commit ngược thứ tự id và client polling đọc được cả hai | bỏ độ trễ lắng |
| watermark: trong ba hàng đã commit chỉ hàng thấp nhất và cao nhất đã lắng; trang trả hàng thấp nhất rồi dừng | lọc đã-lắng từng hàng thay vì watermark |
| một dòng chứa U+0000 được lưu sau khi bỏ nó; một lô chứa một dòng bị từ chối vẫn lưu mọi dòng khác, của mọi workspace, không chờ cửa sổ thử lại | không xoá U+0000 / thử lại một lỗi dữ liệu |
| writer đang hỏng: `finish()` của năm run trả về ngay; một `write` trên kênh đầy không chờ; rào lại chờ khi một lần insert thành công | rào bỏ qua cầu dao |
| một rào đứng sau một lô bị bỏ, và một rào còn trong kênh lúc tắt máy, đều được xác nhận | không xác nhận sau một lô bị bỏ / trong lúc rút kênh |
| tắt máy thì kênh được rút hết | thoát mà không rút kênh |
| giữ log chỉ xoá dòng cũ hơn hạn; `0` không xoá gì | bỏ điều kiện `ts` |
| execution của workspace khác → 404 | bỏ `workspace_id` khỏi câu query |
| `finish()` chạy khi hết giờ và khi bị huỷ | chỉ gọi nó khi thành công |

**Ratchet:** kỳ vọng **109 → 109**, đo chứ không khẳng định. Oracle so trace, không so dòng log.

## 10. Ngoài phạm vi

SSE / `follow`; `/download`; tìm kiếm toàn văn; xoá log thủ công; `logLevel` theo flow; mức ghi đè thời hạn giữ theo workspace (cấu hình chung chừa chỗ cho nó mà không phải đổi schema); object storage (W9); `PurgeLogs`.

## 11. Giới hạn đã biết, nói rõ

- **Có thể mất dòng:** những dòng còn trong kênh khi process chết; một lô hết lượt thử lại, hoặc hỏng ở lần thử duy nhất khi writer đang hỏng; một dòng Postgres từ chối (chỉ dòng đó); một dòng bị bỏ ở kênh đầy khi writer đang hỏng. Mỗi trường hợp được đếm trong `tracing` ở nơi nó xảy ra, trừ process chết, vốn không tự báo được mình mất gì.
- **`complete` có thể đúng trong khi thiếu dòng** ở đúng các trường hợp sau: (1) process chết khi còn dòng chưa lưu; (2) một lô bị bỏ (ở trên) trước lần commit kết thúc; (3) một rào flush không chờ các dòng của run, vì nó quá hạn sau giới hạn **5 giây** hoặc vì writer đang hỏng (cầu dao); kết quả của run khi đó được commit trước khi các dòng cuối được lưu, hoặc không có chúng, và các dòng được lưu sau lần commit kết thúc có thể được trả sau khi `complete` đã báo đúng; (4) một run chạy hai lần (bị reap trong khi handler vẫn chạy): handler cũ có thể ghi sau lần commit kết thúc. `complete` còn giả định không có gì khác ghi sau trạng thái kết thúc, điều đúng chừng nào egret chưa có route kill. *(Ghi chú, 2026-10-18: egret nay đã có route kill (`docs/{en,vi}/specs/2026-10-17-workflow-kill-and-exit-design.md`). Trên đường kill, worker drop handler và chạy `finish()` của log của run trước khi commit `KILLED` (`worker/claim.rs`, nhánh `Leased::Killed` của `run_one`). Kill chưa được đo riêng đối với `complete`.)*
- **Giao ít nhất một lần:** một lô được thử lại sau một lỗi mà writer không thấy được lần commit của nó (mất kết nối sau khi commit) sẽ được lưu hai lần.
- **Một run chạy hai lần** (bị reap trong khi handler vẫn chạy, mối nguy W4 đã ghi lại) ghi dòng của cả hai lần dưới cùng một attempt.
- Một dòng hiện ra khoảng **2 giây** sau khi được ghi (§6).
- **Production chưa chạy task container hay python nào.** `egret-bin` đăng ký `Handlers::with_core_tasks()` (`Log`, `Return`, `Fail`, `Sleep`), và chính comment của nó nói chín trên mười ba kiểu có thể publish bị treo trên production. W11 lưu và test stdout/stderr qua registry dựng trong test; trên production hiện nay, các dòng được lưu đến từ `Log`/`Fail` và thông báo của handler. Việc nối container, python và sql vào production là một gói riêng, vì cách server được phép chạy container là một quyết định bảo mật riêng (quyết định của người dùng, 2026-09-27). *(W5a, 2026-09-28: gói đó là W5a. Khi `workflow.container_runner` là `docker` hoặc `kubernetes`, production chạy bốn script task của Kestra và hai alias, và lưu stdout ở INFO, stderr ở ERROR; mặc định `none` giữ nguyên mục này. Bảy trên mười lăm kiểu có thể publish vẫn bị treo, không phải chín trên mười ba; `egret.sql` và `egret.python` vẫn chưa có trên production.)*
