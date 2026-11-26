# Các loại trigger tùy chọn của workflow (gói 11e) — thiết kế

**Trạng thái:** đã duyệt (brainstorm 2026-11-26). Gói #11 phần 5.
**Roadmap:** egret = engine Kestra OSS; gói #11 (triggers) phần 5 — hai loại trigger tùy chọn
mà roadmap đã hoãn. Tiền nhiệm: 11a–11d. Đo trên source Kestra local `96893b589`.

## 1. Mục tiêu

Gỡ hai loại trigger mà egret hiện đang từ chối kèm lời hứa "roadmap package #11 lifts
ScheduleOnDates and http.Trigger" (`spec/validate.rs:189`, `:3921`):

- **`io.kestra.plugin.core.trigger.ScheduleOnDates`** — trigger Schedulable chạy theo một danh
  sách ngày tường minh (không phải cron). Nhẹ: tái dùng scheduler hiện có.
- **`io.kestra.plugin.core.http.Trigger`** — trigger PollingTriggerInterface, poll một HTTP
  endpoint theo interval và chạy khi `responseCondition` đúng. Nặng: egret chưa có hạ tầng
  polling-trigger; gói này xây nó (claim→commit→poll-ngoài-lock→fire).

Cả hai trở thành `TriggerKind` variant mới (hiện rơi vào `TriggerKind::Leaf` → bị từ chối).

## 2. Đo đạc (Kestra `96893b589`)

- **ScheduleOnDates** (`core/.../trigger/ScheduleOnDates.java`): `extends AbstractTrigger
  implements Schedulable, TriggerOutput<VoidOutput>`. `interval = null` (ẩn — KHÔNG phải
  polling). Field: `dates: Property<List<ZonedDateTime>>`, `timezone` (mặc định system zone),
  `inputs`, `recoverMissedSchedules`. `eval`/`nextEvaluationDate` tính ngày kế tiếp từ danh sách
  `dates` đã sort, ≥ lần evaluate cuối — scheduler evaluate nó y hệt Schedule.
- **http.Trigger** (`core/.../http/Trigger.java`): `extends AbstractTrigger implements
  PollingTriggerInterface, HttpInterface, TriggerOutput<Request.Output>`. `interval =
  Duration.ofSeconds(60)`. Field: `uri`, `method` (GET), `body`, `params`, `formData`,
  `contentType` (application/json), `headers`, `options` (HttpConfiguration), `responseCondition`
  (`{{ response.statusCode < 400 }}`), `encryptBody`. Mỗi interval, worker gửi HTTP request,
  render `responseCondition` theo response, và fire (bind `Request.Output` làm trigger output)
  khi và chỉ khi kết quả truthy.
- **failOnTriggerError / allowConcurrent**: Kestra honour hai key này cho trigger WORKER/POLLING
  (`WorkerTriggerProcessor.java:181`). ScheduleOnDates là Schedulable (scheduler evaluate, nên
  allowConcurrent áp dụng như Schedule; failOnTriggerError KHÔNG — giống Schedule, 11d D6).
  http.Trigger là polling trigger nên CẢ HAI áp dụng (đây chính là polling trigger mà verdict
  Inert của `failOnTriggerError` ở 11d đã chờ).
- **egret hiện tại**: `TriggerKind` (models/trigger.rs:75) = Schedule/Webhook/Flow/Leaf;
  `kind_for` (:480) đẩy type lạ vào Leaf; validate từ chối chúng. Scheduler (`sync.rs`,
  `fire.rs`) keyed theo `TriggerKind::Schedule(_)` ở ~6 chỗ. `egret-egress` (client.rs +
  guard.rs) cung cấp HTTP egress + allowlist host mà http.Request của #4 đang dùng.

## 3. Kiến trúc

### 3.1 Hai TriggerKind variant mới
`TriggerKind::ScheduleOnDates(ScheduleOnDates)` và `TriggerKind::HttpTrigger(HttpTrigger)`,
với `kind_for` route `"scheduleOnDates"` / `"http.Trigger"` (và tên canonical đầy đủ) vào chúng.
Struct model mới nằm dưới `models::trigger` + `trigger/`.

### 3.2 ScheduleOnDates (nhẹ — tái dùng scheduler)
- `scheduler/sync.rs`: filter hiện chỉ cho `TriggerKind::Schedule(_)` vào bảng
  `workflow.triggers` nay cũng nhận `ScheduleOnDates`. `spec_key` của nó là digest của
  `dates|timezone` (đổi dates thì init lại, như đổi cron).
- `scheduler/fire.rs`: `next_evaluation_date` = ngày đầu tiên trong danh sách đã sort, nằm
  sau lần evaluate cuối (theo timezone), hoặc NULL khi danh sách cạn (ra khỏi queue, như cron
  không còn tick tương lai). Fire tái dùng đường Schedule: render inputs, labels, when
  skip-and-advance (11b), stopAfter (11b/11d), allowConcurrent hold (11d),
  recoverMissedSchedules (11b). `execution_trigger.type` = tên canonical của ScheduleOnDates.

### 3.3 http.Trigger (nặng — đường poll mới; D-poll)
- Được lên lịch trong `workflow.triggers` với `next_evaluation_date = last + interval`. Khi đến
  hạn, scheduler **claim row, commit (nhả lock), thực hiện HTTP request NGOÀI mọi transaction,
  rồi fire + advance trong transaction mới** — theo mẫu egress của webhook/#4, để I/O mạng
  không bao giờ giữ lock DB hay chặn lock của lượt fire.
- Request đi qua `egret-egress` (client + guard), bị chặn bởi allowlist
  `workflow.http.allow_hosts` hiện có (#4). Các field (`uri`/`method`/`body`/`params`/
  `headers`/`contentType`) render theo trigger context; `secret()` resolve trên chúng
  (#10a). `responseCondition` render theo response (mặc định `{{ response.statusCode < 400 }}`);
  kết quả truthy thì fire execution với `Request.Output` bind làm trigger output
  (`{{ trigger.* }}` — binding 11a), kết quả falsy chỉ advance interval.
- **failOnTriggerError** (D-poll-honour): lỗi poll/eval với `true` → execution born-failed
  (cơ chế fire.rs born_failed, 11d Task 2), ngược lại ghi vào `last_error`.
  **allowConcurrent**: áp dụng hold dẫn xuất của 11d (không poll-fire khi execution trước
  còn sống). Cả hai đều được honour cho http.Trigger trong census.

### 3.4 Reconcile
Bỏ ScheduleOnDates + http.Trigger khỏi lời từ chối unsupported-trigger (validate.rs:189,
:3921). Thêm cả hai vào census; `failOnTriggerError` chuyển từ `Lift::Inert` phẳng sang
honoured-trên-http.Trigger (vẫn Inert/warned trên Schedule/Webhook/Flow). Sinh lại
property-treatment.json + README.

## 4. Quyết định

- **D1** 11e = cả hai loại trong một gói (user đã duyệt).
- **D2 (thực thi poll)** claim→commit→poll-ngoài-lock→fire, tái dùng scheduler + bảng
  `workflow.triggers`; KHÔNG có vòng lặp polling worker riêng.
- **D3 (lifecycle trên http.Trigger)** honour `failOnTriggerError` + `allowConcurrent` cho
  http.Trigger (nó là polling trigger mà hai key này áp dụng); cập nhật treatment của
  `failOnTriggerError` trong census tương ứng.
- **D4 (state/migration)** tái dùng bảng `workflow.triggers`; `fire.rs` vốn đã đọc source flow
  để dispatch theo `TriggerKind`, nên không cần cột `trigger_type`. Dự kiến không cần
  migration; Task 0 xác nhận (dự phòng: một cột ở mig 0244, hoặc số nào còn trống lúc đó).
- **D5 (egress)** request ra ngoài của http.Trigger tái dùng `egret-egress` +
  `workflow.http.allow_hosts` (guard của #4); `secret()` resolve trên các field của request (#10a).
- **D6 (ngoài scope)** trigger realtime (`*.RealtimeTrigger`), fs.http.Trigger và mọi trigger
  plugin bên thứ ba vẫn bị từ chối (roadmap #18 plugin host). Chỉ hai loại core được nêu trong
  thông báo từ chối được gỡ.

## 5. Task 0 — đo đạc load-bearing (source pin + egret)

1. ScheduleOnDates: ngữ nghĩa chính xác của `nextDate` (có sort? có bao gồm lastEvaluation?
   áp timezone thế nào), tương tác với recoverMissedSchedules, inputs/labels/when áp dụng thế nào.
2. http.Trigger: render context chính xác của `responseCondition` (`response.*` lộ ra gì:
   statusCode, body, headers), shape `Request.Output` bind làm trigger output, LỖI poll khác
   điều kiện falsy ra sao, interval mặc định + khả năng override, xử lý encryptBody/options
   (nhiều khả năng từ chối các sub-key encryptBody/options mà egret không model — ghi lại cái nào).
3. Scheduler egret: mọi điểm dispatch `TriggerKind::Schedule(_)` trong `sync.rs`/`fire.rs` cần
   nhận thêm hai kind mới; xác nhận bảng triggers chứa được chúng mà không cần cột mới.
4. egret-egress: entry point chính xác + cách http.Request của #4 gọi nó, để tái dùng cho poll.
5. Baseline census (sau 11d: 1228/3/354/117/1702) + các điểm gỡ từ chối + grep lift-refusal
   (code + message + fixture) cho lời từ chối loại trigger và mọi test khẳng định
   ScheduleOnDates/http.Trigger bị từ chối. Kết luận migration.

## 6. Kiểm thử

- Các lượt chạy scoped gồm `--test routes --test execution_timestamps --test scheduler_fire
  --test scheduler_sync`; test điều khiển tick giữ mutex tuần tự hóa cục bộ theo file.
- ScheduleOnDates: fire ở từng ngày trong danh sách theo thứ tự; danh sách cạn → ra khỏi queue;
  honour when/labels/stopAfter/allowConcurrent.
- http.Trigger: một HTTP endpoint giả (kiểu wiremock hoặc test server local) → `responseCondition`
  truthy thì fire kèm response được bind; falsy thì advance mà không fire; lỗi poll với
  `failOnTriggerError:true` → execution born-failed; allowConcurrent giữ poll kế tiếp khi
  execution trước còn sống; allowlist egress chặn host không được phép.
- Census golden được sinh lại; cả hai loại honoured; tổng điều chỉnh (Task 0 chốt số chính xác).

## 7. Khác biệt (cần ghi tài liệu)

- Poll của http.Trigger chạy trong scheduler duy nhất (claim→commit→poll→fire), không phải
  worker-trigger processor riêng của Kestra; interval là best-effort quanh nhịp của lượt fire.
- Allowlist egress chặn http.Trigger (khác biệt bảo mật của egret, như http.Request của #4).
- Mọi sub-key của http.Trigger mà egret không model (vd. `options`/`encryptBody`) vẫn bị từ chối
  kèm gói sẽ gỡ nó (ghi lại từ Task 0).
