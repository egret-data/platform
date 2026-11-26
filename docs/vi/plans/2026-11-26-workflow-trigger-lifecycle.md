# Trigger lifecycle parity cho workflow (gói 11d) — Kế hoạch triển khai

> **Cho agentic workers:** REQUIRED SUB-SKILL: Dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để triển khai plan này theo từng task. Các bước dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Mục tiêu:** Honour 3 lifecycle key trigger còn lại (`lift_of → Package(11)`) — `allowConcurrent`, `failOnTriggerError`, và `stopAfter` trên Flow trigger — trên các type Kestra honour.

**Kiến trúc:** Cả 3 key đã là field trên `models::Trigger`. Mỗi key honour tại seam có sẵn: `allowConcurrent` giữ một tick Schedule đến hạn tại `scheduler/fire.rs` khi còn một execution sống của trigger đó (suy từ trạng thái execution, không lưu lock → unlock-on-restart ẩn); `failOnTriggerError` phơi lỗi eval trigger thành một execution `born_failed`; `stopAfter` trên Flow mở type-gate `worker/stop_after.rs::on_terminal`. Behavior ở Task 1–3 không đụng treatment validate-time; Task 4 reconcile census/warning một lần.

**Tech Stack:** Rust, axum, sqlx/Postgres. Crate `crates/egret-workflow`.

## Global Constraints

- **Measurement-first:** không key nào honour cho tới khi Task 0 ghim semantics Kestra chính xác trên pinned artifact `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd` (Java cross-ref checkout `96893b589`). Code Task 1–3 ghim theo kết quả Task 0, không theo đường kỳ vọng của plan.
- **D1** `allowConcurrent` = derived lock, migration-free. Stored `locked_execution_id` + unlock-on-restart tường minh là dự phòng CHỈ khi Task 0 chứng minh semantic không suy được.
- **D2** Chỉ honour mỗi key trên các type Kestra honour. Type Kestra bỏ qua giữ cảnh báo `not_honoured` đúng (hoặc ghi inert). Không đoán.
- **D4** `dependsOn`/listener conditions đã implement (W13a/W13b); 11d không đụng.
- **D5 lift discipline:** trước khi gỡ warning nào, grep code + message + **fixture YAML + test assert-warning** khắp `src/ tests/ oracle-exec/`.
- **Census:** `property-treatment.json` probe-generated (`EGRET_WRITE_PROPERTY_TREATMENT=1`) và golden-tested. KHÔNG được di chuyển ở Task 1–3; di chuyển đúng một lần ở Task 4. Baseline sau 11c: **1224 honoured / 3 warned / 358 ignored / 117 refused / 1702 total**.
- **Scoped test runs** phải gồm `--test routes --test execution_timestamps`; cho stopAfter thêm `--test disabled_triggers --test stop_after`; cho scheduler thêm `--test schedule`.
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Conventional Commits; `Closes #N` chỉ trong PR body.
- Một build Rust nặng một lúc, foreground. `cargo fmt --all --check` trước mọi push. Key `std::env::var` mới vào allow-list `ENV_OUTSIDE_CONFIG` của egret-config.

---

### Task 0: Đo — semantics Kestra + census + lift map (load-bearing, chỉ báo cáo)

**Files:** none (findings ghi vào task report).

- [ ] **Step 1 — Census baseline + lift-refusal grep.** Ghi aggregate `property-treatment.json` hiện tại (kỳ vọng 1224/3/358/117/1702) và bucket của mỗi `(trigger-type, key)` của `allowConcurrent`/`failOnTriggerError`/`stopAfter` cho Flow, Schedule, Webhook. Grep khắp `src/ tests/ oracle-exec/`: `allowConcurrent`, `failOnTriggerError`, `stopAfter`, `not_honoured`, `lift_of`, và mọi test assert các warning này (vd `spec/keys.rs` `stop_after_and_an_explicit_allow_concurrent_false_warn_and_true_does_not` ~:2178, các test `ALL_LIFT`/base-key ~:2300). Liệt kê mọi file+line mà gỡ warning sẽ làm vỡ.

- [ ] **Step 2 — allowConcurrent trên pinned artifact.** Đọc Kestra scheduler source tại checkout `96893b589` (`AbstractScheduler`, `SchedulerExecutionWithTrigger`, `TriggerState`, `Schedule`) và/hoặc drive image. Ghim:
  - (a) **default** của `allowConcurrent` (comment keys.rs:733 ghi `false`; xác nhận — nếu `false`, egret over-fire ngay cả khi không khai báo).
  - (b) áp cho **trigger type** nào (chỉ Schedule/polling, hay cả Flow/Webhook).
  - (c) **hold semantics**: khi execution trước còn sống, tick đến hạn bị GIỮ (fire sau) hay BỎ (advance)? Khi unlock, tick bị giữ có fire không, hay chỉ tick kế? Tương tác `recoverMissedSchedules` và backfill.
  - (d) **lock key**: hold trên `executionId` cụ thể, hay trên "bất kỳ execution sống nào của (flow, trigger)"? Quyết định query suy (§Task 1) đủ hay cần fallback D1.
  - Ghi verdict: **suy được (mặc định)** hay **cần fallback stored-lock**, kèm lý do.

- [ ] **Step 3 — failOnTriggerError trên pinned artifact.** Ghim: **default**; áp cho **trigger type** nào; khi lỗi eval với `true`, Kestra tạo execution FAILED, chỉ log, hay cả hai, và execution failed mang gì (message, trigger context); thế nào là "trigger error" (eval throw / render fail / fire fail) vs no-fire bình thường.

- [ ] **Step 4 — stopAfter trên Flow trigger.** Xác nhận Kestra disable Flow trigger khi terminal state trong `stopAfter` y như Schedule, và disable per-trigger (không per-flow). Trong egret, xác nhận đường evaluate Flow-trigger đọc disable: trace `flow_trigger/process.rs` → `trigger/flow.rs` `evaluate` `Refusal::TriggerDisabled` và kiểm `trigger.disabled` suy từ `workflow.disabled_triggers` cho Flow trigger (như route webhook đọc). Ghi: stopAfter-Flow đã wired đủ bằng type-gate `stop_after.rs` chưa, hay cần confirm/add đường đọc disable.

- [ ] **Step 5 — dependsOn no-gap check.** Xác nhận máy móc W13a/W13b không để gap `dependsOn`/condition-type: census cho thấy các corpus flow Flow `dependsOn` được xử lý (fire/refuse đúng đo), không condition type Flow-trigger nào ngoài `states`/`when`/`dependsOn` bị âm thầm bỏ. Ghi "no gap" hoặc liệt kê gap (sẽ thành dòng roadmap mới, không phải 11d).

- [ ] **Step 6 — Viết lift map + verdict migration.** Với mỗi `(trigger-type, key)` honoured per §2–§4, ghi: site warn `keys.rs` cần gỡ, entry `measure.rs` `trigger_base_read` cần thêm, test assert-warning cần sửa, và key honoured hay giữ warned (kèm câu chữ). Ghi "migration-free" (kỳ vọng) hoặc cột stored-lock + số migration kế (kế tiếp là **0244**; 0243 là 11a).

- [ ] **Step 7 — Ghi BASE commit** (`git rev-parse HEAD`) cho review package, và viết mọi finding vào report. Không production code ở Task 0.

---

### Task 1: `allowConcurrent` — giữ tick đến hạn khi execution của trigger còn sống

**Files:**
- Modify: `crates/egret-workflow/src/scheduler/fire.rs` (`Fate` enum ~:88; `fire_one` ~:220; `claim_and_fire`/`PassReport` ~:44/:158)
- Test: `crates/egret-workflow/tests/schedule.rs` (hoặc file test scheduler Task 0 xác định)

**Interfaces:**
- Consumes: verdict Task 0 về hold semantics (§Step 2) và quyết định suy-được/fallback. Nếu Task 0 nói **fallback stored-lock**, DỪNG và escalate — task này giả định đường suy (mặc định D1).
- Produces: biến thể `Fate::Held` mới và counter `PassReport.held: usize`; predicate live-execution dùng ở Task 2 độc lập (không cần helper chung).

- [ ] **Step 1 — Viết test fail.** Trong file test scheduler, drive hai tick đến hạn của một Schedule trigger khai báo `allowConcurrent: false`, với task ở RUNNING. Dùng mẫu mutex tuần-tự-tick file-local (worker `tick` claim toàn cục; giữ `tokio::Mutex` suốt test, như `TICK` của webhook_route.rs ở 11c). Assert: tick 1 fire một execution; khi execution đó non-terminal, pass `fire_due` thứ hai KHÔNG fire execution thứ hai và KHÔNG advance `next_evaluation_date` qua tick bị giữ; sau khi execution đầu terminal, `fire_due` kế fire tick bị giữ đúng một lần. Ghim hành vi held/advance/retroactive theo Task 0 §Step 2(c).

```rust
#[tokio::test]
async fn allow_concurrent_false_holds_a_tick_while_the_execution_is_live() {
    let _g = TICK.lock().await; // file-local serialiser; worker tick claims globally
    let (pool, flow) = seed_schedule_flow(
        "triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"* * * * *\"\n    allowConcurrent: false\n",
    ).await;
    // tick 1 fires
    let r1 = fire_due(&pool).await.unwrap();
    assert_eq!(r1.fired, 1);
    let exec1 = latest_execution(&pool, flow).await;
    // while exec1 is live, a due tick 2 is HELD, not fired, not advanced
    make_due_again(&pool, flow, "s").await; // set next_evaluation_date <= now()
    let date_before = next_eval_date(&pool, flow, "s").await;
    let r2 = fire_due(&pool).await.unwrap();
    assert_eq!(r2.fired, 0, "held while exec1 is live");
    assert_eq!(r2.held, 1);
    assert_eq!(next_eval_date(&pool, flow, "s").await, date_before, "tick not advanced");
    // exec1 terminates; the held tick now fires once
    drive_to_terminal(&pool, exec1).await;
    let r3 = fire_due(&pool).await.unwrap();
    assert_eq!(r3.fired, 1);
}
```

- [ ] **Step 2 — Chạy để xác nhận fail.** `cargo test -p egret-workflow --test schedule allow_concurrent_false_holds -- --nocapture`. Kỳ vọng: FAIL (`Fate::Held`/`PassReport.held` chưa có; tick 2 fire).

- [ ] **Step 3 — Thêm fate `Held` + counter.** Trong `fire.rs`: thêm `Held` vào `enum Fate`, thêm `held: usize` vào `PassReport`, và map `Fate::Held` trong pass accounting (nơi `Fate::Frozen` v.v. được đếm) để tăng `held` và để nguyên `next_evaluation_date` (không `last_error`, không advance) — cùng đường "dates để nguyên, pass sau retry" mà `Errored` dùng, nhưng im lặng.

- [ ] **Step 4 — Hold trong `fire_one`.** Sau khi resolve trigger spec và trước `start()`, đọc `allow_concurrent` từ trigger đã resolve. Khi nó `false` (ghim default theo Task 0 §Step 2a) VÀ có execution sống cho trigger này, trả `Fate::Held`. Predicate live-execution (dưới trigger-row lock của claim):

```rust
// Any non-terminal execution this trigger already started. `execution_is_idle`
// (migration 0210) is the terminal test; its negation is "live".
let live: bool = sqlx::query_scalar(
    "SELECT EXISTS (SELECT 1 FROM workflow.executions \
       WHERE flow_id = $1 AND workspace_id = $2 \
         AND execution_trigger ->> 'id' = $3 \
         AND NOT workflow.execution_is_idle(id))",
)
.bind(c.flow_id)
.bind(c.workspace_id)
.bind(&c.trigger_id)
.fetch_one(&mut **at)
.await?;
if !allow_concurrent && live {
    return Ok(Fate::Held);
}
```

Ghim lock key chính xác (per-trigger vs per-prior-executionId) theo Task 0 §Step 2d; query trên key trên "bất kỳ execution sống của (flow, trigger)".

- [ ] **Step 5 — Chạy test để pass.** `cargo test -p egret-workflow --test schedule allow_concurrent_false_holds`. Kỳ vọng: PASS. Thêm/xác nhận test `allowConcurrent: true` (hoặc unset, theo default Task 0) KHÔNG hold — cả hai tick fire.

- [ ] **Step 6 — Scoped suite + fmt.** `cargo test -p egret-workflow --test schedule --test routes --test execution_timestamps` xanh; `cargo fmt --all --check`.

- [ ] **Step 7 — Commit.**

```bash
git add crates/egret-workflow/src/scheduler/fire.rs crates/egret-workflow/tests/schedule.rs
git commit -m "feat(workflow): honour allowConcurrent — hold a due tick while the trigger's execution is live"
```

---

### Task 2: `failOnTriggerError` — phơi lỗi trigger thành execution born-failed

**Files:**
- Modify: `crates/egret-workflow/src/scheduler/fire.rs` (`fire_one` error arm; `start`/`record_error` ~:573/:677)
- Test: `crates/egret-workflow/tests/schedule.rs`

**Interfaces:**
- Consumes: Task 0 §Step 3 (default, type-scope, "cả FAILED execution + log" vs chỉ log, thế nào là trigger error). `NewExecution.born_failed: bool` (worker/claim.rs) — "start execution đã FAILED, không task run."
- Produces: không gì task khác dùng.

- [ ] **Step 1 — Viết test fail.** Drive một Schedule trigger mà fire ném lỗi eval trigger (ghim lớp lỗi chính xác theo Task 0 §Step 3c — vd `when`/inputs render vào biến thiếu, hoặc gì Task 0 xác định là "trigger error" của Kestra). Assert: với `failOnTriggerError: false` (default), hành vi hôm nay — `last_error` được set, KHÔNG execution; với `failOnTriggerError: true`, có execution `born_failed` (state FAILED, `taskRunList` rỗng) VÀ `last_error` vẫn ghi.

```rust
#[tokio::test]
async fn fail_on_trigger_error_true_surfaces_a_failed_execution() {
    let _g = TICK.lock().await;
    let (pool, flow) = seed_schedule_flow(
        "triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"* * * * *\"\n    failOnTriggerError: true\n    inputs:\n      x: \"{{ missing.field }}\"\n",
    ).await; // pin the error shape to Task 0 §Step 3c
    let r = fire_due(&pool).await.unwrap();
    let exec = latest_execution(&pool, flow).await;
    assert_eq!(state_of(&pool, exec).await, "FAILED");
    assert!(task_runs_of(&pool, exec).await.is_empty(), "no task run");
    assert!(last_error_of(&pool, flow, "s").await.is_some());
}
```

- [ ] **Step 2 — Chạy để xác nhận fail.** `cargo test -p egret-workflow --test schedule fail_on_trigger_error_true -- --nocapture`. Kỳ vọng: FAIL (không execution; `fire_one` chỉ ghi `last_error`).

- [ ] **Step 3 — Phơi lỗi ở error arm.** Trong `fire_one`, nơi lỗi eval trigger hiện được classify thành `Fate::Errored`/`record_error` (arm Task 0 §Step 3c xác định), khi `fail_on_trigger_error` là `true` cho type Kestra honour (Task 0 §Step 3a/b), start một execution born-failed trước khi ghi `last_error`:

```rust
if fail_on_trigger_error {
    // Surface the trigger error as Kestra does: a CREATED -> FAILED execution,
    // no task run. `born_failed` is W13a's mechanism (worker/claim.rs).
    let _ = start_born_failed(at, c, &resolved /* inputs/trigger as measured */).await;
}
// existing behaviour, unchanged: record the reason on the trigger row
record_error(at, c, &why).await?;
```

Thêm một `start_born_failed` cạnh `start` gọi `start_execution_in_with` với `born_failed: true` và cùng các field `NewExecution` mà `start` dùng (trigger record, scheduled_date, inputs tới mức render được). Nó KHÔNG bao giờ được propagate lỗi làm rollback trigger advance: khi fail thì log và rơi xuống `record_error` (Global Constraint: key chỉ thêm phơi lỗi, không bỏ log sẵn có). Ghim việc execution failed có mang inputs/trigger render không theo Task 0 §Step 3b.

- [ ] **Step 4 — Chạy test để pass.** `cargo test -p egret-workflow --test schedule fail_on_trigger_error`. Kỳ vọng: PASS. Thêm test đối chứng default `false` (không execution, chỉ `last_error`).

- [ ] **Step 5 — Scoped suite + fmt.** `cargo test -p egret-workflow --test schedule --test routes --test execution_timestamps` xanh; `cargo fmt --all --check`.

- [ ] **Step 6 — Commit.**

```bash
git add crates/egret-workflow/src/scheduler/fire.rs crates/egret-workflow/tests/schedule.rs
git commit -m "feat(workflow): honour failOnTriggerError — surface a trigger error as a born-failed execution"
```

---

### Task 3: `stopAfter` trên Flow trigger

**Files:**
- Modify: `crates/egret-workflow/src/worker/stop_after.rs` (thêm `FLOW_TYPE`, mở type-gate ~:40)
- Modify (nếu Task 0 §Step 4 yêu cầu): `crates/egret-workflow/src/trigger/flow.rs` / `flow_trigger/process.rs` (xác nhận đường evaluate Flow-trigger đọc `workflow.disabled_triggers`)
- Test: `crates/egret-workflow/tests/stop_after.rs` và/hoặc `crates/egret-workflow/tests/disabled_triggers.rs`

**Interfaces:**
- Consumes: Task 0 §Step 4 (Flow-trigger disable per-trigger qua `disabled_triggers`; đường evaluate đã đọc nó chưa). `on_terminal` sẵn có đã làm INSERT `disabled_triggers` + UPDATE `triggers` (UPDATE là no-op cho Flow, không có hàng `workflow.triggers` — như Webhook).
- Produces: không gì task khác dùng.

- [ ] **Step 1 — Viết test fail.** Một listening flow với Flow trigger khai báo `stopAfter: [SUCCESS]`. Fire listener (publish một source execution terminal ở state được nghe), drive execution listener tới SUCCESS, chạy settle, rồi assert có hàng `workflow.disabled_triggers` cho `(listener_flow, trigger_id)` và một source event khớp thứ hai KHÔNG fire listener lại.

```rust
#[tokio::test]
async fn flow_trigger_stop_after_disables_the_listener() {
    let _g = TICK.lock().await;
    let (pool, listener) = seed_listener_flow(
        "triggers:\n  - id: l\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n    stopAfter: [SUCCESS]\n",
    ).await;
    fire_source_and_settle_listener_to(&pool, listener, "SUCCESS").await;
    assert!(disabled_trigger_exists(&pool, listener, "l").await);
    // a second source event does not fire the disabled listener
    let fired = fire_source_event(&pool, listener).await;
    assert_eq!(fired, 0);
}
```

- [ ] **Step 2 — Chạy để xác nhận fail.** `cargo test -p egret-workflow --test stop_after flow_trigger_stop_after -- --nocapture`. Kỳ vọng: FAIL — type-gate của `on_terminal` loại Flow, nên không hàng `disabled_triggers`.

- [ ] **Step 3 — Mở type-gate.** Trong `stop_after.rs`, thêm `const FLOW_TYPE: &str = "io.kestra.plugin.core.trigger.Flow";` và đưa vào `matches!` gate:

```rust
if !matches!(
    trigger.get("type").and_then(|t| t.as_str()),
    Some(crate::scheduler::SCHEDULE_TYPE | WEBHOOK_TYPE | FLOW_TYPE)
) {
    return Ok(());
}
```

Cập nhật module doc thành "Schedule, Webhook and Flow `stopAfter`". Nếu Task 0 §Step 4 thấy đường evaluate Flow-trigger CHƯA đọc `disabled_triggers`, wire đường đọc đó trong `flow_trigger/process.rs`/`trigger/flow.rs` để disable có hiệu lực (listener có hàng `disabled_triggers` bị skip với `Refusal::TriggerDisabled`).

- [ ] **Step 4 — Chạy test để pass.** `cargo test -p egret-workflow --test stop_after flow_trigger_stop_after`. Kỳ vọng: PASS.

- [ ] **Step 5 — Scoped suite + fmt.** `cargo test -p egret-workflow --test stop_after --test disabled_triggers --test routes --test execution_timestamps` xanh; `cargo fmt --all --check`.

- [ ] **Step 6 — Commit.**

```bash
git add crates/egret-workflow/src/worker/stop_after.rs
git commit -m "feat(workflow): honour stopAfter on the Flow trigger"
```

---

### Task 4: Reconcile — gỡ warning, census golden, docs

**Files:**
- Modify: `crates/egret-workflow/src/spec/keys.rs` (warn logic ~:725–760; `lift_of` ~:176; test assert-warning ~:2178/:2300)
- Modify: `crates/egret-workflow/src/spec/schema/measure.rs` (`trigger_base_read` ~:97)
- Modify: `crates/egret-workflow/src/spec/schema/property-treatment.json` (regenerate) và `crates/egret-workflow/src/spec/schema/README.md`
- Create: bổ sung `docs/en/platform/workflows/triggers/*.md` + `docs/vi/...` (repo egret-data/docs — docs PR riêng, theo docs rule CLAUDE.md)

**Interfaces:**
- Consumes: lift map Task 0 §Step 6 — các cặp `(trigger-type, key)` giờ honoured vs giữ warned, và test assert-warning cần sửa.

- [ ] **Step 1 — Cập nhật read-set.** Trong `measure.rs` `trigger_base_read`, thêm mỗi `(trigger-type, key)` honoured từ lift map để read-set đường fire classify nó honoured (vd `allowConcurrent`/`failOnTriggerError` trên Schedule, `stopAfter` trên Flow — đúng theo Task 0 §Step 6). KHÔNG thêm key cho type Task 0 thấy inert.

- [ ] **Step 2 — Gỡ warning.** Trong `keys.rs`, làm logic `lift_of`/warn type-aware cho các key này: bỏ warning `not_honoured` cho cặp `(type, key)` honoured; giữ nó (câu chữ đúng) cho cặp giữ warned (D2). Bỏ skip đặc biệt `allowConcurrent != false` chỉ khi default của Task 0 làm nó thừa.

- [ ] **Step 3 — Cập nhật test assert-warning.** Sửa mọi test Task 0 §Step 1 liệt kê (vd `stop_after_and_an_explicit_allow_concurrent_false_warn_and_true_does_not`, các test base-key/`ALL_LIFT`) để assert treatment mới: cặp honoured không còn warn; cặp giữ-warned vẫn warn.

- [ ] **Step 4 — Regenerate census golden.** `EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow --test <the golden test> property_treatment` (probe ghi `property-treatment.json`), rồi chạy lại không env var để xác nhận golden khớp. Ghi aggregate mới (honoured tăng bằng số cặp gỡ; total giữ 1702). Cập nhật các hàng Flow/Schedule/Webhook của `README.md` để chuyển các key gỡ ra khỏi cột not-honoured.

- [ ] **Step 5 — Chạy full suite validate/census.** `cargo test -p egret-workflow --test routes --test execution_timestamps` cộng unit test spec/keys (`cargo test -p egret-workflow spec::keys`) và test census golden — đều xanh. `cargo fmt --all --check`.

- [ ] **Step 6 — Docs (egret-data/docs).** Trong docs worktree/PR riêng, ghi tài liệu: `allowConcurrent` (derived hold + divergences §7 — derived vs stored lock, hành vi held-tick như đo), `failOnTriggerError` (born-failed surfacing, default, type-scope), `stopAfter` trên Flow trigger. Kèm divergences đo từ Task 0. Song ngữ (en + vi).

- [ ] **Step 7 — Commit.**

```bash
git add crates/egret-workflow/src/spec/keys.rs crates/egret-workflow/src/spec/schema/measure.rs \
        crates/egret-workflow/src/spec/schema/property-treatment.json crates/egret-workflow/src/spec/schema/README.md
git commit -m "feat(workflow): lift trigger lifecycle warnings + reconcile census for package 11d"
```

---

## Ghi chú Self-Review

- **Spec coverage:** allowConcurrent → Task 1 (+ Task 4 warning); failOnTriggerError → Task 2 (+ Task 4); stopAfter-Flow → Task 3 (+ Task 4); dependsOn no-gap → Task 0 §Step 5; census/lift map → Task 0 + Task 4; divergences/docs → Task 4 §Step 6. D1/D2/D4/D5 là Global Constraints. Mọi mục spec map tới một task.
- **Migration:** kỳ vọng migration-free (D1 derived lock). Nếu Task 0 §Step 2d buộc fallback stored-lock, Task 1 escalate và chèn một task migration (cột trên `workflow.triggers` + pass unlock-on-restart, số kế **0244**) trước Task 1 — không đi tiếp đường suy.
- **Census discipline:** Task 1–3 chỉ đụng `fire.rs`/`stop_after.rs` + tests (behavior), không bao giờ `keys.rs`/`measure.rs`/`property-treatment.json`; golden không di chuyển giữa feature. Task 4 là lần census move duy nhất.
