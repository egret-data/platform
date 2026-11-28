# Workflow Subflow scheduleDate (gói #12b) — Thiết kế

**Trạng thái:** đã brainstorm + chốt quyết định 2026-11-27 (formalize trên worktree #12b tách từ
main sau #12 `4796619ea`). Đo đối chiếu với Kestra `96893b589`.
**Roadmap:** phần tách ra từ #12 (Subflow). Bốn key Subflow còn lại đã ship trong #12 (#920);
`scheduleDate` bị hoãn vì cần một hệ thống khởi động-trễ execution hoàn toàn mới.

## 1. Mục tiêu

Tôn trọng `scheduleDate` của task Subflow (một `Property<ZonedDateTime>`): khi đặt là một thời điểm
trong tương lai, execution con được spawn KHÔNG BẮT ĐẦU cho tới thời điểm đó; nó được tạo ra và
chờ. Parent `wait: true` chờ qua khoảng trễ như với mọi child. Hiện tại `scheduleDate` được chấp
nhận nhưng không có tác dụng, kèm cảnh báo qua `Lift::Deferred("roadmap package #12b lifts it")`.

## 2. Đo đạc (Kestra `96893b589` + egret)

- **Kestra** (`ExecutableUtils.java:228-250`): `scheduleDate` được render theo runContext của
  parent và truyền vào `Execution.newExecution(flow, inputs, labels, scheduleDate, kind)` — nó
  được đặt **ngay trên execution con** lúc tạo. Executor của Kestra không start một execution
  trước `scheduledDate` của nó; child nằm ở CREATED cho tới khi đến hạn. (KHÔNG qua
  `ExecutionDelay`, vốn là cơ chế RESUME_FLOW/pause-resume của Kestra.) Task 0 chốt cách xử lý
  ngày quá khứ/không hợp lệ.
- **egret không có điểm nối defer-start:** `worker::subflow::spawn_one` chạy `settle` đầu tiên của
  child ngay lập tức, bên trong transaction của caller (thiết kế nested-settle sâu). Không có gì
  tra cứu thời điểm bắt đầu.
- **Cột `scheduled_date` của egret là cột sai:** nó là khoá định danh tick / chống bắn đôi của
  scheduler, và CHECK `workflow_executions_scheduled_date_iff_schedule` (0156, thu hẹp bởi 0244
  trong 11e) CẤM child của Subflow (`execution_trigger` NULL) ghi vào nó.
- **Mẫu "đánh thức lúc T" tái dùng được của egret:** retry delay — `executor::retry::reopen_due`
  + `TaskRun::not_before` + các pass waker `worker::claim::due_retries` /
  `worker::replay::spawn_due` (poll các run đã đến hạn delay, mở lại chúng). #12b mô hình hoá việc
  khởi động-trễ child theo mẫu poll-đến-hạn-rồi-đánh-thức này.

## 3. Kiến trúc

Một **cơ chế khởi động-trễ riêng**, độc lập với `scheduled_date`:

### 3.1 Cột mới (migration 0246)
`ALTER TABLE workflow.executions ADD COLUMN scheduled_start timestamptz;` — NULL với mọi execution
thường; chỉ được đặt trên child Subflow có `scheduleDate` render ra một thời điểm tương lai.
Không ràng buộc CHECK với `execution_trigger` (đây là khái niệm khác `scheduled_date`). Một index
trên `scheduled_start WHERE scheduled_start IS NOT NULL` cho poll của waker.

### 3.2 Defer lúc spawn (`worker::subflow::spawn_one`)
Khi Subflow khai báo `scheduleDate`, render nó theo ngữ cảnh parent:
- **thời điểm tương lai** → insert dòng child với `scheduled_start = T` và `state = CREATED`, và
  **KHÔNG chạy settle đầu tiên**. Child được tạo nhưng bị "đậu" (parked).
- **quá khứ / bây giờ / render được nhưng không hợp lệ** → start ngay (hành vi hiện tại); Task 0
  chốt quy tắc chính xác của Kestra (nhiều khả năng "start ngay" với quá khứ; lỗi render làm
  Subflow run fail như các lỗi render khác của Subflow).

### 3.3 Waker (tái dùng nhịp poll của worker hiện có)
Một query mới gắn vào vòng poll worker/scheduler hiện có (cùng nhịp mà `reopen_due` /
`spawn_due` chạy — Task 0 chốt điểm nối chính xác): `SELECT các child đang parked WHERE
scheduled_start <= now() AND state = CREATED AND <chưa settle>`, và với mỗi child, chạy `settle`
đầu tiên của nó (đúng settle đầu tiên mà `spawn_one` sẽ chạy), rồi xoá/để nguyên `scheduled_start`
để không bị đánh thức lại. Idempotent: child đã qua CREATED thì bỏ qua; hai waker đua nhau trên
một child được chặn bằng row lock / tính idempotent của chính settle (Task 0 xác nhận guard).

### 3.4 Parent wait + topology
Parent `wait: true` chờ child đạt terminal đúng như hiện nay — defer chỉ làm trễ CREATED→RUNNING.
UI topology/executions hiện child parked là CREATED cùng `scheduled_start` của nó (thêm một DTO
nhỏ, tuỳ chọn cho gói này — Task 0 quyết định hiện hay hoãn phần UI).

### 3.5 Reconcile
Nâng `scheduleDate` từ `Lift::Deferred` → honoured; read-set của `measure.rs` đã có nó qua
`wire_fields_of::<Subflow>()` (field đã tồn tại), nên regen sẽ lật `Subflow.scheduleDate`
ignored→honoured. Census 1261 → **1262**. Cập nhật README + ghi chú docs của #12 ("coming later"
→ có tài liệu).

## 4. Quyết định

- **D1** cột `scheduled_start` riêng (migration 0246), KHÔNG tái dùng `scheduled_date` (làm vậy sẽ
  nới lại CHECK mà 11e vừa thu hẹp và trộn lẫn tick-identity với deferral).
- **D2** waker tái dùng nhịp poll của worker hiện có (điểm nối `reopen_due`/`spawn_due`), không
  thêm timer/vòng lặp mới.
- **D3** `scheduleDate` tương lai → park CREATED; quá khứ/bây giờ → start ngay; lỗi render làm
  Subflow run fail (Task 0 chốt quy tắc Kestra chính xác).
- **D4** child parked nằm ở `CREATED`; parent `wait: true` chờ qua khoảng trễ không đổi.
- **D5** census: `scheduleDate` nâng lên honoured (+1 → 1262); `Lift::Deferred` của nó bị gỡ.
- **D6** migration 0246 (số trống kế tiếp sau 0245).

## 5. Task 0 — đo đạc mang tính quyết định

1. Kestra: ngữ nghĩa `scheduleDate` chính xác — render theo ngữ cảnh parent (xác nhận); state của
   execution con khi đang defer; xử lý ngày quá khứ và ngày không hợp lệ/không render được;
   `wait:true` của parent và `transmitFailed` tương tác ra sao với child bị defer; `scheduleDate`
   ở quá khứ bị kẹp về now hay chạy ngay.
2. egret: điểm chạy settle đầu tiên chính xác của `spawn_one` (nơi rẽ nhánh "park vs settle");
   điểm nối poll worker chính xác mà waker gắn vào (vòng `reopen_due`/`spawn_due`/`claim`) và nhịp
   của nó; settle đầu tiên của child được chạy NGOÀI transaction spawn của parent ra sao khi được
   đánh thức muộn; guard idempotency/row-lock cần có để waker start child đúng một lần.
3. Hình dạng migration (cột + index) và có cần guard unique/start-đôi không; số trống kế tiếp
   (0246); xác nhận không có CHECK trên cột mới.
4. Baseline census (sau #12: 1261/3/363/120/1747); vị trí lift của `scheduleDate` (`keys.rs`
   `Lift::Deferred`) + các test assert cảnh báo cần lật; ghi chú docs #12 cần cập nhật.

## 6. Kiểm thử

- Subflow có `scheduleDate` là một thời điểm tương lai gần → child là CREATED và KHÔNG start cho
  tới thời điểm đó; sau khi waker chạy nó start, chạy, và parent `wait:true` hoàn tất.
- `scheduleDate` quá khứ → child start ngay.
- `scheduleDate` không render được → Subflow run fail (như các lỗi render khác của Subflow).
- Waker start một child parked đã đến hạn đúng một lần (không start đôi khi poll đua nhau).
- Golden census được regen: `Subflow.scheduleDate` ignored→honoured (1262).
- Các lượt chạy có phạm vi gồm bộ test subflow/executor/restart/routes/execution_timestamps + file
  test của waker; test điều khiển tick/waker giữ mutex tuần tự hoá cục bộ theo file.

## 7. Điểm khác biệt (cần tài liệu hoá)

- egret start child bị defer từ một nhịp poll (best-effort quanh chu kỳ vòng lặp worker), không
  phải scheduledDate do executor của Kestra giữ; việc start có thể trễ so với thời điểm tới một
  chu kỳ poll.
- `scheduleDate` chỉ được parse theo RFC3339. Cách render `ZonedDateTime` của Kestra dễ dãi hơn,
  nên một ngày trần như `2026-01-01` mà Kestra chấp nhận sẽ làm Subflow run fail trong egret.
- `created_at` trở nên có thể thay đổi với child parked được đánh thức: lần wake ghi lại nó thành
  `now()` để đồng hồ của child (`not_before`, thời điểm attempt) bắt đầu khi nó start, không phải
  khi nó bị park. Chỉ tie-break của graph sắp theo `created_at` mới nhận ra (một ghi chú hẹp);
  child đã start thì không bao giờ bị dời.
