# Workflow Subflow scheduleDate (gói #12b) — Kế hoạch triển khai

> **Cho agent thực thi:** SUB-SKILL BẮT BUỘC: dùng superpowers:subagent-driven-development. Các bước dùng `- [ ]`.

**Mục tiêu:** Tôn trọng `scheduleDate` của Subflow — `scheduleDate` tương lai park child được spawn ở `CREATED` và một waker start nó khi đến thời điểm; parent `wait:true` chờ qua khoảng trễ.

**Kiến trúc:** Một cơ chế khởi động-trễ riêng. Cột mới `workflow.executions.scheduled_start timestamptz` (migration 0246) giữ thời điểm; `worker::subflow::spawn_one` park child (insert CREATED, KHÔNG chạy settle đầu tiên) thay vì start nó khi `scheduleDate` ở tương lai; một waker gắn vào nhịp poll worker hiện có (điểm nối `reopen_due`/`spawn_due`) chạy settle đầu tiên của các child có `scheduled_start` đã đến hạn, đúng một lần. Độc lập với `scheduled_date` (cột tick-identity của scheduler) và CHECK của nó.

**Tech Stack:** Rust, axum, sqlx/Postgres. Crate `crates/egret-workflow`.

## Ràng buộc chung

- **Đo trước:** Task 0 chốt ngữ nghĩa `scheduleDate` của Kestra trên source pinned local `/Users/1pro-l4r8/Documents/github/data-portal/kestra` @ `96893b589`, và các điểm nối spawn/waker chính xác của egret, trước khi viết code.
- **D1** cột `scheduled_start` riêng (migration **0246**), KHÔNG tái dùng `scheduled_date` (không đụng CHECK `workflow_executions_scheduled_date_iff_schedule` mà 0244 của 11e đã thu hẹp). **D2** waker tái dùng nhịp poll worker hiện có (vòng `executor::retry::reopen_due` / `worker::replay::spawn_due` / `worker::claim`), không thêm timer. **D3** `scheduleDate` tương lai → park CREATED; quá khứ/bây giờ → start ngay; không render được → Subflow run fail (như các lỗi render khác của Subflow). **D4** child parked nằm `CREATED`; parent `wait:true` chờ qua khoảng trễ không đổi. **D5** census: `scheduleDate` nâng từ `Lift::Deferred` → honoured (1261 → **1262**). **D6** migration 0246 (số trống kế tiếp sau 0245).
- **Census:** `property-treatment.json` có golden test; nó chỉ đổi một lần, ở task reconcile (Task 2). Hệ thống con (Task 1) chỉ gồm hành vi + migration.
- **Các lượt chạy có phạm vi** gồm bộ test subflow/executor/restart/routes/execution_timestamps + file test của waker; test điều khiển tick/waker giữ mutex tuần tự hoá cục bộ theo file (poll của worker claim toàn cục).
- Trailer commit `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; Conventional Commits; `Closes #N` chỉ trong body PR. Trước khi xong mỗi task: `cargo fmt --all --check`, **`cargo clippy --all-targets -- -D warnings`** (CI chạy clippy toàn workspace; riêng `cargo check` đã bỏ sót `large_enum_variant` ở 11e — hãy chạy clippy), `cargo check -p egret-workflow`. Build ở foreground, từng cái một. Env var mới → egret-config `ENV_OUTSIDE_CONFIG`.

---

### Task 0: Đo — ngữ nghĩa scheduleDate của Kestra + các điểm nối spawn/waker của egret (mang tính quyết định, chỉ báo cáo)

**Files:** không (báo cáo).

- [ ] **Bước 1 — Ngữ nghĩa Kestra.** Trên source pinned (`ExecutableUtils.java:228-250`, `Execution.newExecution`): xác nhận `scheduleDate` được render theo runContext của PARENT; STATE của execution con khi đang defer (CREATED); executor làm gì với nó (giữ tới thời điểm); xử lý `scheduleDate` QUÁ KHỨ (start ngay hay kẹp?) và giá trị không render được/không hợp lệ (fail hay bỏ qua); `wait:true` + `transmitFailed` tương tác ra sao với child bị defer.
- [ ] **Bước 2 — Điểm nối spawn của egret.** Trong `worker::subflow::spawn_one` (`worker/subflow.rs:284`), tìm ĐIỂM CHÍNH XÁC nơi `settle` đầu tiên của child được chạy bên trong transaction của caller, để implementer rẽ nhánh "park (insert CREATED, đặt `scheduled_start`, bỏ qua settle) vs settle ngay". Ghi lại cách dòng child được insert (INSERT quanh subflow.rs:~450-525) để thêm bind `scheduled_start`.
- [ ] **Bước 3 — Điểm nối waker của egret.** Xác định vòng poll worker chính xác cần gắn (`executor::retry::reopen_due` + `worker::claim::due_retries` / `worker::replay::spawn_due` — đọc `worker/claim.rs:596-666`, `worker/replay.rs:110,362`): nhịp của nó, cách nó claim việc đến hạn, và cách nó chạy reopen/settle của một run NGOÀI mọi transaction của caller. Xác định cách chạy settle ĐẦU TIÊN của child parked khi được đánh thức muộn (child chưa từng có settle đầu tiên — `spawn_one` thường làm nó trong txn của parent; waker phải làm tương đương bây giờ, độc lập). Ghi lại guard idempotency/row-lock khiến một child parked đến hạn start ĐÚNG MỘT LẦN dưới nhiều poller đồng thời.
- [ ] **Bước 4 — Migration + census.** Xác nhận `scheduled_start` chỉ cần một cột nullable thường + một partial index (`WHERE scheduled_start IS NOT NULL`) và KHÔNG có CHECK/trigger ràng buộc; số trống kế tiếp là 0246 (đĩa tới 0245). Ghi baseline census (sau #12: 1261/3/363/120/1747), vị trí lift của `scheduleDate` (`keys.rs` arm `(Subflow, scheduleDate) => Lift::Deferred(...)`) và các test assert cảnh báo ghim nó (grep `Lift::Deferred` / "package #12b" trong `src/ tests/`), và ghi chú docs #12 "coming later" cần cập nhật. Ghi một commit BASE.

---

### Task 1: Hệ thống khởi động-trễ — migration 0246 + park lúc spawn + waker

**Files:**
- Create: `crates/egret-workflow/migrations/0246_execution_scheduled_start.sql`
- Modify: `crates/egret-workflow/src/worker/subflow.rs` (nhánh park trong `spawn_one` + INSERT của child)
- Modify: `crates/egret-workflow/src/models/trigger.rs`/`models/task.rs` — parse `scheduleDate` trên struct `Subflow` nếu chưa có (grep; #12 để nó chưa parse vì bị hoãn)
- Modify: điểm nối waker mà Task 0 §3 nêu (nhiều khả năng `crates/egret-workflow/src/worker/claim.rs` hoặc `worker/deferred_start.rs` mới + hook trong vòng lặp worker)
- Test: `crates/egret-workflow/tests/subflow_schedule_date.rs`

**Interfaces:**
- Dùng: Task 0 §2 (điểm nối spawn) + §3 (điểm nối waker, guard idempotency). Nếu Task 0 thấy việc chạy settle đầu tiên của child độc lập là thay đổi lớn hơn việc hook waker hiện có, DỪNG và báo cáo (có thể cần tách park và waker thành hai task).
- Tạo ra: `workflow.executions.scheduled_start` (thường NULL); child parked là `CREATED` với `scheduled_start` được đặt và không chạy settle; waker chạy settle đầu tiên của nó khi đến hạn.

- [ ] **Bước 1 — Migration.** `migrations/0246_execution_scheduled_start.sql`:

```sql
-- Subflow scheduleDate (package #12b): the instant a deferred child execution
-- should start. NULL for every normal execution; set on a Subflow child whose
-- scheduleDate is in the future. Independent of scheduled_date (the scheduler
-- tick identity). The waker (worker loop) starts a CREATED child once this is due.
ALTER TABLE workflow.executions ADD COLUMN scheduled_start timestamptz;
CREATE INDEX workflow_executions_scheduled_start
  ON workflow.executions (scheduled_start)
  WHERE scheduled_start IS NOT NULL;
```

- [ ] **Bước 2 — Test fail (park).** Một flow parent có Subflow khai báo `scheduleDate` là một thời điểm tương lai gần → sau khi parent spawn, dòng child tồn tại ở `CREATED` với `scheduled_start` ≈ thời điểm đó, và KHÔNG có task run / không RUNNING. Ghim state defer theo Task 0 §1.
- [ ] **Bước 3 — Chạy → fail** (thiếu cột `scheduled_start`/parse/park).
- [ ] **Bước 4 — Parse + park.** Parse `schedule_date` trên model `Subflow`. Trong `spawn_one`, render `scheduleDate` theo ngữ cảnh parent (Task 0 §1): thời điểm TƯƠNG LAI → insert child với `scheduled_start = T`, `state = CREATED`, và KHÔNG chạy settle đầu tiên (return trước điểm chạy settle mà Task 0 §2 nêu); QUÁ KHỨ/bây giờ → không đổi (settle ngay); lỗi render → làm Subflow run fail qua đường lỗi render Subflow hiện có.
- [ ] **Bước 5 — Chạy → pass** test park.
- [ ] **Bước 6 — Test fail (waker).** Một child parked có `scheduled_start` đã ở quá khứ → một lần poll worker start nó (chạy settle đầu tiên), nó chạy tới terminal, và parent `wait:true` hoàn tất. Lần poll thứ hai KHÔNG start đôi. Dùng mutex tuần tự hoá cục bộ theo file.
- [ ] **Bước 7 — Waker.** Hook nhịp poll worker hiện có (Task 0 §3): claim các child CREATED có `scheduled_start <= now()` dưới row lock, chạy settle đầu tiên của từng child độc lập (tương đương settle đầu tiên của `spawn_one`, nay ngoài mọi txn của parent), và xoá `scheduled_start` (hoặc dựa vào `state != CREATED`) để không bị đánh thức lại. Single-start idempotent theo Task 0 §3.
- [ ] **Bước 8 — Chạy → pass** test waker + assert không start đôi.
- [ ] **Bước 9 — Phạm vi + fmt + clippy.** `cargo test -p egret-workflow --test subflow_schedule_date --test executor --test routes --test execution_timestamps` xanh; `cargo fmt --all --check`; `cargo clippy --all-targets -- -D warnings`; `cargo check -p egret-workflow`.
- [ ] **Bước 10 — Commit.** `feat(workflow): Subflow scheduleDate — defer the child start (migration 0246 + waker)`

---

### Task 2: Reconcile — nâng scheduleDate lên honoured, census, docs

**Files:**
- Modify: `crates/egret-workflow/src/spec/keys.rs` (bỏ arm `(Subflow, scheduleDate) => Lift::Deferred` → None), `crates/egret-workflow/src/spec/schema/property-treatment.json` (regen), `crates/egret-workflow/src/spec/schema/README.md`
- Create: docs trong egret-data/docs (PR riêng, do controller xử lý)

**Interfaces:** Dùng bản đồ lift của Task 0 §4.

- [ ] **Bước 1 — Lift.** Trong `keys.rs`, `(Subflow, scheduleDate)` không còn cảnh báo (bỏ arm `Lift::Deferred` → None). Nếu `Lift::Deferred` không còn user nào, gỡ variant + các arm `short()`/`sentence()` của nó; nếu còn thì để lại. Grep + cập nhật các test assert cảnh báo (test `flowable_keys…` kỳ vọng thông điệp #12b trên `scheduleDate`).
- [ ] **Bước 2 — Regen census.** `EGRET_WRITE_PROPERTY_TREATMENT=1` rồi chạy lại không có biến này. `Subflow.scheduleDate` lật ignored→honoured; **kỳ vọng 1262 honoured / 3 / 362 ignored / 120 / 1747**. Cập nhật dòng Subflow trong README + dòng Result.
- [ ] **Bước 3 — Chạy** `cargo test -p egret-workflow spec::keys` + golden census + `--test routes --test execution_timestamps` xanh; `cargo clippy --all-targets -- -D warnings`; `cargo fmt --all --check`.
- [ ] **Bước 4 — Docs (egret-data/docs, PR riêng).** Cập nhật doc Subflow: `scheduleDate` nay được honoured (bỏ ghi chú "#12 coming later"), tài liệu hoá hành vi khởi động-trễ + điểm khác biệt về nhịp poll. Song ngữ.
- [ ] **Bước 5 — Commit.** `feat(workflow): lift Subflow scheduleDate to honoured + reconcile census (package #12b)`

---

## Ghi chú tự rà soát

- **Phủ spec:** ngữ nghĩa scheduleDate/park/waker → Task 1; census/lift/docs → Task 2; đo đạc → Task 0. D1–D6 là Ràng buộc chung.
- **Rủi ro:** Task 1 là hệ thống con; ẩn số lớn nhất là chạy settle ĐẦU TIÊN của child parked độc lập từ waker (`spawn_one` thường làm nó trong txn của parent). Task 0 §3 ghim điều này; nếu nó lớn hơn một cái hook, Task 1 tách thành 1a (migration + park) và 1b (waker) — đã nêu trong Interfaces của Task 1.
- **Migration:** 0246, cột nullable thường + partial index, không CHECK — đó chính là mục đích của D1 (tránh nới lại CHECK scheduled_date).
- **Kỷ luật census:** Task 1 chỉ gồm hành vi + migration (không keys.rs/measure.rs/property-treatment.json); Task 2 là lần đổi census duy nhất (1261→1262).
