# Trigger lifecycle parity cho workflow (gói 11d) — Kế hoạch triển khai

> **Cho agentic workers:** REQUIRED SUB-SKILL: Dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để triển khai plan này theo từng task. Các bước dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Mục tiêu:** Honour `allowConcurrent` trên Schedule trigger (tuần tự hoá execution) và `stopAfter` trên Flow trigger; giữ `failOnTriggerError` warned (Task 0 đo: Kestra không honour nó cho bất kỳ trigger type nào egret có).

**Kiến trúc:** Cả 3 key đã là field trên `models::Trigger`. `allowConcurrent` giữ một tick Schedule đến hạn tại `scheduler/fire.rs` khi còn một execution sống của trigger đó (suy từ trạng thái execution, không lưu lock → unlock-on-restart ẩn). `stopAfter` trên Flow mở type-gate `worker/stop_after.rs::on_terminal` và disable listener qua `workflow.flow_listeners`. Behavior ở Task 1–2 không đụng treatment validate-time; Task 3 reconcile census/warning một lần.

**Tech Stack:** Rust, axum, sqlx/Postgres. Crate `crates/egret-workflow`.

## Global Constraints

- **Measurement-first (XONG, Task 0):** semantics ghim theo Kestra source local tại cross-ref checkout `96893b589` (= pinned artifact `kestra/kestra@sha256:28f59bf1…`). Findings trong `.superpowers/sdd/2026-11-26-workflow-trigger-lifecycle/task-0-report.md`.
- **D1** `allowConcurrent` = derived lock, migration-free (verdict Task 0: DERIVABLE). Không cột lock; egret suy hold từ "bất kỳ execution non-terminal của (flow, trigger)". Honour **chỉ Schedule** (Kestra: Schedule + polling; egret không có polling trigger).
- **D2** Chỉ honour mỗi key nơi Kestra honour. Ngoại lệ (user duyệt): `stopAfter` trên Flow là **divergence egret có chủ đích**, nhất quán với việc egret đã honour `stopAfter` trên Webhook (11c) nơi Kestra cũng no-op (`DefaultExecutor.sendTriggerExecutionTerminated` loại Webhook + Flow + Subflow).
- **D6 `failOnTriggerError` NGOÀI phạm vi (không honour).** Task 0: Kestra chỉ đọc nó ở `WorkerTriggerProcessor` (worker/polling trigger); egret không có → Schedule/Webhook/Flow không bao giờ consult. Giữ warning `not_honoured`, nhưng Task 3 sửa câu chữ + target `lift_of` — nó là Kestra-divergence không gói egret nào gỡ, KHÔNG phải "package 11 sẽ honour".
- **D4** `dependsOn`/listener conditions đã implement (W13a/W13b); 11d không đụng (Task 0: no gap).
- **D5 lift discipline:** trước khi gỡ warning nào, grep code + message + **fixture YAML + test assert-warning** khắp `src/ tests/ oracle-exec/`.
- **Census:** `property-treatment.json` probe-generated (`EGRET_WRITE_PROPERTY_TREATMENT=1`) và golden-tested. KHÔNG di chuyển ở Task 1–2; di chuyển đúng một lần ở Task 3. Baseline sau 11c: **1224 honoured / 3 warned / 358 ignored / 117 refused / 1702 total** (9 cell lifecycle hiện đều `ignored`).
- **Scoped test runs** phải gồm `--test routes --test execution_timestamps`; cho stopAfter thêm `--test disabled_triggers --test stop_after`; cho scheduler thêm `--test schedule`.
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Conventional Commits; `Closes #N` chỉ trong PR body.
- Một build Rust nặng một lúc, foreground. `cargo fmt --all --check` trước mọi push. Key `std::env::var` mới vào allow-list `ENV_OUTSIDE_CONFIG` của egret-config.

---

### Task 0: Đo — semantics Kestra + census + lift map (load-bearing, chỉ báo cáo) — XONG

Xong. Verdicts (controller xác minh trong Kestra source `96893b589`): **allowConcurrent** honour Schedule-only, default `false` = HOLD serial, **DERIVABLE / migration-free**; **failOnTriggerError** chỉ đọc tại `WorkerTriggerProcessor.java:181` → không type egret honour → giữ warned (D6); **stopAfter** loại Flow+Webhook tại `DefaultExecutor.java:861-868` → Flow stopAfter là divergence egret (D2), và disable của Flow listener đọc từ `workflow.flow_listeners.disabled` (nên `on_terminal` phải UPDATE bảng đó, không chỉ `workflow.triggers`); **dependsOn** no gap. Census 1224/3/358/117/1702; migration kế 0244. Chi tiết + lift map trong `task-0-report.md`.

---

### Task 1: `allowConcurrent` — giữ tick Schedule đến hạn khi execution của trigger còn sống

**Files:**
- Modify: `crates/egret-workflow/src/scheduler/fire.rs` (`Fate` enum ~:88; `fire_one` ~:220; `PassReport` ~:44)
- Test: `crates/egret-workflow/tests/schedule.rs` (hoặc file test scheduler repo dùng — xác nhận `git grep -l fire_due tests/`)

**Interfaces:**
- Consumes: verdict Task 0 — hold semantics DERIVABLE, key trên "bất kỳ execution sống của (flow, trigger)"; default `false` tuần tự hoá (giữ). Tick lỡ bị GIỮ và xử lý tuần tự (một tick mỗi lần hoàn tất).
- Produces: biến thể `Fate::Held` mới và counter `PassReport.held: usize`.

- [ ] **Step 1 — Viết test fail.** Drive hai tick đến hạn của một Schedule trigger khai báo `allowConcurrent: false`, với task ở RUNNING. Dùng mutex tuần-tự-tick file-local (worker `tick` claim toàn cục; giữ `tokio::Mutex` suốt test, như `TICK` của webhook_route.rs ở 11c). Assert: tick 1 fire; khi execution đó non-terminal, pass `fire_due` thứ hai KHÔNG fire và KHÔNG advance `next_evaluation_date` qua tick bị giữ; sau khi execution đầu terminal, `fire_due` kế fire tick bị giữ đúng một lần. Ghim held/advance theo Task 0.

```rust
#[tokio::test]
async fn allow_concurrent_false_holds_a_tick_while_the_execution_is_live() {
    let _g = TICK.lock().await; // file-local serialiser; worker tick claims globally
    let (pool, flow) = seed_schedule_flow(
        "triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"* * * * *\"\n    allowConcurrent: false\n",
    ).await;
    let r1 = fire_due(&pool).await.unwrap();
    assert_eq!(r1.fired, 1);
    let exec1 = latest_execution(&pool, flow).await;
    make_due_again(&pool, flow, "s").await; // set next_evaluation_date <= now()
    let date_before = next_eval_date(&pool, flow, "s").await;
    let r2 = fire_due(&pool).await.unwrap();
    assert_eq!(r2.fired, 0, "held while exec1 is live");
    assert_eq!(r2.held, 1);
    assert_eq!(next_eval_date(&pool, flow, "s").await, date_before, "tick not advanced");
    drive_to_terminal(&pool, exec1).await;
    let r3 = fire_due(&pool).await.unwrap();
    assert_eq!(r3.fired, 1);
}
```

- [ ] **Step 2 — Chạy để xác nhận fail.** `cargo test -p egret-workflow --test schedule allow_concurrent_false_holds -- --nocapture`. Kỳ vọng: FAIL (`Fate::Held`/`PassReport.held` chưa có).

- [ ] **Step 3 — Thêm fate `Held` + counter.** Trong `fire.rs`: thêm `Held` vào `enum Fate`, thêm `held: usize` vào `PassReport`, và trong pass accounting map `Fate::Held` để tăng `held` và để nguyên `next_evaluation_date` (không `last_error`, không advance) — bản im lặng của đường "dates để nguyên, pass sau retry".

- [ ] **Step 4 — Hold trong `fire_one`.** Sau khi resolve trigger spec và trước `start()`, đọc `allow_concurrent`. Khi `false` VÀ có execution sống cho trigger này, trả `Fate::Held`. Query (dưới trigger-row lock của claim):

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

- [ ] **Step 5 — Chạy test để pass.** `cargo test -p egret-workflow --test schedule allow_concurrent_false_holds`. Kỳ vọng: PASS. Thêm test `allowConcurrent: true` KHÔNG hold — cả hai tick fire.

- [ ] **Step 6 — Scoped suite + fmt.** `cargo test -p egret-workflow --test schedule --test routes --test execution_timestamps` xanh; `cargo fmt --all --check`.

- [ ] **Step 7 — Commit.**

```bash
git add crates/egret-workflow/src/scheduler/fire.rs crates/egret-workflow/tests/schedule.rs
git commit -m "feat(workflow): honour allowConcurrent — hold a due Schedule tick while its execution is live"
```

---

### Task 2: `stopAfter` trên Flow trigger (divergence egret có chủ đích)

**Files:**
- Modify: `crates/egret-workflow/src/worker/stop_after.rs` (thêm `FLOW_TYPE`, mở type-gate ~:40; thêm disable `workflow.flow_listeners`)
- Test: `crates/egret-workflow/tests/stop_after.rs` và/hoặc `crates/egret-workflow/tests/disabled_triggers.rs`

**Interfaces:**
- Consumes: Task 0 — Kestra no-op Flow stopAfter (`DefaultExecutor.java:861-868`); egret honour như divergence, nhất quán Webhook (11c). Disable của Flow listener đọc từ `workflow.flow_listeners.disabled` (derived từ `disabled_triggers` chỉ lúc sync), KHÔNG `workflow.triggers` (Flow trigger không có hàng ở đó, UPDATE sẵn là no-op). Nên type-gate một mình CHƯA đủ — `on_terminal` phải disable luôn hàng listener.
- Produces: không gì task khác dùng.

- [ ] **Step 1 — Viết test fail.** Một listening flow với Flow trigger khai báo `stopAfter: [SUCCESS]`. Fire listener (publish source execution terminal ở state được nghe), drive execution listener tới SUCCESS, chạy settle, rồi assert listener bị disable (có hàng `workflow.disabled_triggers` VÀ `workflow.flow_listeners.disabled = true` cho `(listener_flow, trigger_id)`) và source event khớp thứ hai KHÔNG fire listener lại.

```rust
#[tokio::test]
async fn flow_trigger_stop_after_disables_the_listener() {
    let _g = TICK.lock().await;
    let (pool, listener) = seed_listener_flow(
        "triggers:\n  - id: l\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n    stopAfter: [SUCCESS]\n",
    ).await;
    fire_source_and_settle_listener_to(&pool, listener, "SUCCESS").await;
    assert!(disabled_trigger_exists(&pool, listener, "l").await);
    assert!(flow_listener_disabled(&pool, listener, "l").await);
    let fired = fire_source_event(&pool, listener).await;
    assert_eq!(fired, 0, "disabled listener does not fire");
}
```

- [ ] **Step 2 — Chạy để xác nhận fail.** `cargo test -p egret-workflow --test stop_after flow_trigger_stop_after -- --nocapture`. Kỳ vọng: FAIL — type-gate loại Flow, không disable được ghi, event thứ hai fire.

- [ ] **Step 3 — Mở type-gate + disable listener.** Trong `stop_after.rs`, thêm `const FLOW_TYPE: &str = "io.kestra.plugin.core.trigger.Flow";` và đưa vào `matches!` gate:

```rust
if !matches!(
    trigger.get("type").and_then(|t| t.as_str()),
    Some(crate::scheduler::SCHEDULE_TYPE | WEBHOOK_TYPE | FLOW_TYPE)
) {
    return Ok(());
}
```

Rồi, cho type Flow, disable luôn hàng listener để firing dừng ngay (INSERT `disabled_triggers` sẵn có sống qua sync; UPDATE `workflow.triggers` giữ no-op cho Flow). Thêm, trong cùng settle transaction:

```rust
// A Flow trigger fires off `workflow.flow_listeners`, whose `disabled` is
// re-derived from `disabled_triggers` only at sync. Disable it now so the
// listener stops firing within this settle, as sync would on the next save.
sqlx::query(
    "UPDATE workflow.flow_listeners SET disabled = true \
       WHERE flow_id = $1 AND trigger_id = $2",
)
.bind(flow_id)
.bind(trigger_id)
.execute(&mut *conn)
.await
.map_err(db_err)?;
```

Xác nhận tên cột/bảng chính xác với `flow_trigger/process.rs` (listener query ~:584-590) và migration `workflow.flow_listeners` — dùng đúng cái đường listener-fire đọc cho `disabled`. Cập nhật module doc thành "Schedule, Webhook and Flow `stopAfter`" và ghi divergence Flow (Kestra no-op).

- [ ] **Step 4 — Chạy test để pass.** `cargo test -p egret-workflow --test stop_after flow_trigger_stop_after`. Kỳ vọng: PASS.

- [ ] **Step 5 — Scoped suite + fmt.** `cargo test -p egret-workflow --test stop_after --test disabled_triggers --test routes --test execution_timestamps` xanh; `cargo fmt --all --check`.

- [ ] **Step 6 — Commit.**

```bash
git add crates/egret-workflow/src/worker/stop_after.rs
git commit -m "feat(workflow): honour stopAfter on the Flow trigger (egret divergence, as Webhook)"
```

---

### Task 3: Reconcile — gỡ warning, sửa câu chữ failOnTriggerError, census golden, docs

**Files:**
- Modify: `crates/egret-workflow/src/spec/keys.rs` (`lift_of` ~:176; warn logic ~:714–730; test assert-warning ~:2071, ~:2170–2200)
- Modify: `crates/egret-workflow/src/spec/schema/measure.rs` (`trigger_base_read` ~:99–112)
- Modify: `crates/egret-workflow/src/spec/schema/property-treatment.json` (regenerate) và `crates/egret-workflow/src/spec/schema/README.md` (~:216, ~:261–263)
- Create: bổ sung docs trong repo egret-data/docs (docs PR riêng, theo docs rule CLAUDE.md), en + vi

**Interfaces:**
- Consumes: lift map Task 0. Honour giờ: `(Schedule, allowConcurrent)` và `(Flow, stopAfter)`. Giữ warned: `failOnTriggerError` (mọi type) câu chữ sửa; `allowConcurrent` trên Flow/Webhook; `stopAfter` trên Schedule/Webhook đã honour bởi 11b/11c.

- [ ] **Step 1 — Cập nhật read-set.** Trong `measure.rs` `trigger_base_read`, thêm `allowConcurrent` vào read-set Schedule và `stopAfter` vào read-set Flow (và xác nhận `stopAfter` đã trong read-set Schedule + Webhook từ 11b/11c). KHÔNG thêm `failOnTriggerError` đâu cả, và KHÔNG thêm `allowConcurrent` vào Flow/Webhook.

- [ ] **Step 2 — Gỡ hai warning; sửa failOnTriggerError.** Trong `keys.rs`: làm logic warn/`lift_of` type-aware để `(Schedule, allowConcurrent)` và `(Flow, stopAfter)` không còn warn. Giữ warning `not_honoured` cho `allowConcurrent` trên Flow/Webhook. Với `failOnTriggerError`, đổi `lift_of` để nó KHÔNG báo "package 11 sẽ honour": làm warning nói đây là Kestra-divergence egret không act vì Kestra chỉ consult cho worker/polling trigger mà egret không có. Bỏ skip đặc biệt `allowConcurrent != false` nếu logic type-aware làm nó thừa.

- [ ] **Step 3 — Cập nhật test assert-warning.** Sửa mọi test Task 0 §Step 1 liệt kê (keys.rs ~:2071, ~:2170–2200, gồm `stop_after_and_an_explicit_allow_concurrent_false_warn_and_true_does_not`) để assert treatment mới: `(Schedule, allowConcurrent)` + `(Flow, stopAfter)` không còn warn; `failOnTriggerError` + `(Flow/Webhook, allowConcurrent)` vẫn warn (câu chữ sửa).

- [ ] **Step 4 — Regenerate census golden.** `EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow <the property_treatment golden test>` (probe tại measure.rs ~:807), rồi chạy lại không env var để xác nhận golden khớp. Honoured tăng 2 cell gỡ (kỳ vọng **1226** honoured; total giữ 1702). Cập nhật hàng Schedule (bỏ `allowConcurrent`) và hàng Flow (bỏ `stopAfter`) của `README.md` khỏi cột not-honoured.

- [ ] **Step 5 — Chạy suite validate/census.** `cargo test -p egret-workflow --test routes --test execution_timestamps`, unit test spec/keys (`cargo test -p egret-workflow spec::keys`), và test census golden — đều xanh. `cargo fmt --all --check`.

- [ ] **Step 6 — Docs (egret-data/docs).** Trong docs worktree/PR riêng, ghi: `allowConcurrent` trên Schedule (derived hold, execution tuần tự, divergences §7 — derived vs stored lock, held-tick xử lý tuần tự); `stopAfter` trên Flow trigger (divergence egret — Kestra no-op, như Webhook); và note `failOnTriggerError` được chấp nhận nhưng inert (Kestra chỉ act cho polling trigger egret không có). Song ngữ (en + vi).

- [ ] **Step 7 — Commit.**

```bash
git add crates/egret-workflow/src/spec/keys.rs crates/egret-workflow/src/spec/schema/measure.rs \
        crates/egret-workflow/src/spec/schema/property-treatment.json crates/egret-workflow/src/spec/schema/README.md
git commit -m "feat(workflow): lift allowConcurrent/stopAfter warnings + reconcile census for package 11d"
```

---

## Ghi chú Self-Review

- **Spec coverage:** allowConcurrent (Schedule) → Task 1 (+ Task 3 gỡ warning); stopAfter-Flow → Task 2 (+ Task 3); failOnTriggerError giữ warned → Task 3 câu chữ; dependsOn no-gap → Task 0; census/lift map → Task 0 + Task 3; divergences/docs → Task 3 §Step 6.
- **Sửa phạm vi (Task 0):** tiền đề gốc của plan rằng cả 3 key được honour là sai. Sự thật đo: chỉ `allowConcurrent` (Schedule) là Kestra parity; `stopAfter`-Flow là divergence egret có chủ đích (user duyệt, nhất quán 11c Webhook); `failOnTriggerError` không type egret nào honour và giữ warned.
- **Migration:** migration-free (D1 derived lock; stopAfter dùng lại bảng sẵn; không stored lock).
- **Census discipline:** Task 1–2 chỉ đụng `fire.rs`/`stop_after.rs` + tests (behavior), không bao giờ `keys.rs`/`measure.rs`/`property-treatment.json`; golden không di chuyển giữa feature. Task 3 là lần census move duy nhất (honoured 1224 → 1226).
