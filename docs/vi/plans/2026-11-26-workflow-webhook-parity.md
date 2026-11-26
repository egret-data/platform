# Webhook trigger parity (gói 11c) — Kế hoạch triển khai

> **Cho worker agent:** SUB-SKILL BẮT BUỘC: superpowers:subagent-driven-development. Bước dùng checkbox (`- [ ]`).
> **Lưu ý:** code/chữ ký/code block + file:line giữ như bản EN (`docs/en/plans/2026-11-26-workflow-webhook-parity.md`). Bản VI dịch diễn giải; triển khai đọc kèm EN.

**Mục tiêu:** Cho Webhook trigger egret khớp Kestra ở `labels`, `stopAfter`, `when`, và synchronous response (`wait`/`returnOutputs`/`responseContentType`/`responseCode`).

**Kiến trúc:** Cả 5 feature hook vào seam fire webhook duy nhất `webhook_routes.rs::handle()` + seam terminal 11b `worker/stop_after.rs::on_terminal`. labels/stopAfter/when tái dùng khuôn 11b; sync response thêm bounded poll `workflow.executions.state` sau khi execution start. Đo trước: Task 0 pin nhánh Kestra `when`, timeout/default sync, nguồn outputs, delta census trước mọi code feature.

## Ràng buộc toàn cục
- Chat VI; code/commit/docs EN. Conventional Commits; `Closes #N` chỉ body PR.
- Trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; body PR kết dòng Claude Code.
- Không commit secret/scratch. Stage theo path; không `git add -A`/`target*/`.
- MỘT build nặng/lần, foreground. Merge CI xanh, không `--admin`, `cargo fmt --all --check` trước.
- **Scoped `egret-workflow` PHẢI kèm `--test routes --test execution_timestamps --test webhook_route`** (test webhook ở `tests/webhook_route.rs`; stopAfter thêm `--test disabled_triggers --test stop_after` cho Task 2/3/4).
- **LIFT-REFUSAL discipline:** trước khi gỡ refuse sync key (+ warning `when`), grep code + message + fixture YAML across `src/ tests/ oracle-exec/` (`declared_unsupported`, `Unsupported::`, `WEBHOOK_REFUSED`, `keys.rs:185` `(WEBHOOK,"when")`, corpus webhook).
- Số migration chung — chỉ nếu cần cột (KHÔNG dự kiến). Số free kế lúc plan, renumber.
- `secret()`/10a không đụng.

## Cấu trúc file
Xem EN. Chính: `webhook_routes.rs` `handle()` (trigger-disabled check T2, label render T1, when gate T3, sync poll T4); `worker/stop_after.rs` `on_terminal` (mở gate type → Webhook, T2); `trigger/webhook.rs` `declared_unsupported` (gỡ sync key T4, label renderer webhook T1); `spec/keys.rs` `WEBHOOK_REFUSED`/`lift_of` (T3/T4); `property-treatment.json`+README (census). Test: `tests/webhook_route.rs` (chính), `disabled_triggers.rs`, `stop_after.rs`.

---

### Task 0: Đo — census + pin semantics Kestra chưa đo (load-bearing)
- [ ] **Bước 1 — Census + LIFT-REFUSAL grep.** Ghi flow Webhook corpus đang refused/warned cho sync key + `when`. Grep các site. Xác nhận `baseline.txt` không đổi (harness không drive route). Ghi aggregate `property-treatment.json` hiện tại (1218/3/360/121 sau 11b) + webhook key ở bucket nào.
- [ ] **Bước 2 — Nhánh `when` Kestra.** Trên artifact pin, xác định controller webhook Kestra CÓ eval `when` lúc request không. Drive request thật flow webhook `when:"false"` + templated falsy. Ghi: falsy có suppress execution? Nếu có, request nhận status/body gì? → chọn **nhánh (a)** (gate tạo execution) vs **nhánh (b)** (accept-but-inert no-op).
- [ ] **Bước 3 — Semantics sync `wait`.** Drive `wait:true`. Pin: timeout default + chỗ cấu hình; response khi terminal (nguồn status, body); `responseCode` default + có dẫn từ terminal state; shape body `returnOutputs:true`; response khi timeout trước terminal. Pin cách đọc chính xác outputs flow-level của execution terminal (cột/nguồn trong `workflow.executions` hay tính).
- [ ] **Bước 4 — Xác nhận migration-free.** labels/stopAfter/when/sync tái dùng cột/bảng sẵn. Ghi "migration-free" hoặc cột cần + số free kế.
- [ ] **Bước 5 — Ghi finding** vào task report. Không production code. Pin Task 1/2/3/4.

---

### Task 1: Webhook `labels` lên execution được fire
**File:** `handle()` trước `start_execution_with_trigger` (`:686`); label renderer ở `trigger/webhook.rs` (mô phỏng `render_trigger_labels` `fire.rs:627` nhưng bind **webhook** record `{body,headers,parameters,uri}`, KHÔNG phải `ScheduledExecution` → viết biến thể, không gọi thẳng). Test `webhook_route.rs`.
- [ ] **Bước 1: Test fail** — flow webhook `labels:[{env:prod},{src:"{{trigger.headers.x}}"}]` fire execution mang label đó (templated render với record), **prepend** trước flow labels. Assert thứ tự: trigger rồi flow.
- [ ] **Bước 2: FAIL** (label webhook bị bỏ). **Bước 3:** ở `handle()` sau `build_output` trước start, render `labels` trigger với record (tái dùng `render_str`+`RenderContext` shape `fire.rs:627`, bind record dưới `trigger`), prepend. Flow labels không đổi. Lỗi render → fail-safe như 11b.
- [ ] **Bước 4: PASS** (+scoped). **Bước 5: Commit** `feat(workflow): Webhook trigger labels on the fired execution`.

---

### Task 2: `stopAfter` — disable webhook trigger sau terminal khớp
**File:** `worker/stop_after.rs` `on_terminal` (`:40`, gate `type != SCHEDULE_TYPE`); `handle()` sau check flow-disabled (`:603`). Test `stop_after.rs`, `webhook_route.rs`, `disabled_triggers.rs`.
- [ ] **Bước 1a: Test fail (hook)** — execution do **Webhook** khởi đạt terminal ∈ `stopAfter` (vd `[FAILED]`) disable trigger (`triggers.disabled=true` + row `disabled_triggers`); terminal không-khớp giữ enabled; execution không-trigger không disable gì.
- [ ] **Bước 2a: FAIL** (gate loại non-Schedule). **Bước 3a:** mở gate `:40` từ `== Some(SCHEDULE_TYPE)` sang nhận Schedule HOẶC Webhook (thêm `WEBHOOK_TYPE`/dùng `keys::WEBHOOK`); phần còn lại type-agnostic (null `next_evaluation_date` = no-op cho webhook). Cập nhật doc `:1` thành Schedule VÀ Webhook.
- [ ] **Bước 4a: PASS.**
- [ ] **Bước 1b: Test fail (route gate)** — sau khi webhook trigger disabled (`triggers.disabled=true`), POST tới key trả **409** (không fire); trigger enabled vẫn fire.
- [ ] **Bước 2b: FAIL** (`handle()` nay chỉ check flow-disabled). **Bước 3b:** ở `handle()` sau check flow-disabled (`:603`), đọc `triggers.disabled` cho `(flow_id,trigger_id)`, trả `Error::Conflict` khi disabled. Message khác với flow-disabled.
- [ ] **Bước 4b: PASS** (+scoped kèm `--test stop_after --test disabled_triggers`). **Bước 5: Commit** `feat(workflow): Webhook stopAfter disables the trigger and the route rejects it`.

---

### Task 3: `when` — honor theo nhánh Task 0
**File:** `handle()` (sau key match, trước start); `spec/keys.rs` `lift_of` (`:185`); `validate.rs`+`property-treatment.json`+README (census). Test `webhook_route.rs`.
- [ ] **Bước 1: Test fail** — **nhánh (a):** flow webhook `when:"false"` (+ templated falsy) trả theo Task 0 **không** tạo execution; `when:"true"` fire; lỗi render `when` fail **safe** (không execution, 4xx). **nhánh (b):** `when:"false"` vẫn fire (no-op), assert theo hành vi Kestra Task 0 đo. Chọn nhánh Task 0 pin.
- [ ] **Bước 2: FAIL. Bước 3:** theo nhánh Task 0. (a): sau key match, render `when` với record `{body,headers,parameters,uri}` (confined, bind 11a); falsy→trả response Task 0, không start; lỗi render→fail-safe. (b): giữ runtime nguyên, vẫn parse. **Cả hai:** đổi `keys.rs lift_of` `(WEBHOOK,"when")` (`:185`) khỏi `Package(11)` (a: honoured→bỏ lift khỏi warn, như 11b bỏ Schedule when; b: theo census Task 0) + cập nhật `property-treatment.json` + README `### Result` + note drift-recount. LIFT-REFUSAL grep trước.
- [ ] **Bước 4: PASS** (+scoped kèm `--test flow_schema`). **Bước 5: Commit** `feat(workflow): honour Webhook when:` (a) hoặc `feat(workflow): accept Webhook when as an inert no-op (Kestra parity)` (b).

---

### Task 4: Synchronous response — `wait`/`returnOutputs`/`responseContentType`/`responseCode`
**File:** `trigger/webhook.rs` `declared_unsupported` (`:348`); `handle()` (`:618` gỡ refuse + thêm poll); `keys.rs` `WEBHOOK_REFUSED` (`:435`) + `lift_of` (`:186`); `property-treatment.json`+README. Test `webhook_route.rs`.
- [ ] **Bước 1: Test fail** — (a) `wait:true` flow terminal: body mang `outputs` terminal khi `returnOutputs:true`, status = `responseCode` render (hoặc default Task 0), Content-Type = `responseContentType` nếu set; (b) `wait:true` không terminal trong timeout → response timeout Task 0; (c) `wait:false` (default) trả `state:"CREATED"` ngay, fire-and-forget. (Drive executor để execution thật đạt terminal — mô phỏng harness `webhook_route.rs`+executor scheduler tests dùng.)
- [ ] **Bước 2: FAIL** (nay 409). **Bước 3:** (1) gỡ Wait/ReturnOutputs/ResponseContentType/ResponseCode khỏi `declared_unsupported` (`:348`, giữ StoreBody/Multipart); gỡ khỏi `WEBHOOK_REFUSED` (`:435`) + chỉnh `lift_of` (`:186`); LIFT-REFUSAL grep trước. (2) ở `handle()` sau start, branch `webhook.wait`. `false`→`WebhookResponse{state:"CREATED"}` không đổi. `true`→bounded poll: loop `SELECT state[,outputs] FROM workflow.executions WHERE id=$1` mỗi FIXED_INTERVAL tới `RunState::is_terminal`, bound bởi max-wait Task 0. Khi terminal: `responseCode` render→status (else default Task 0), body=outputs terminal khi `return_outputs` (nguồn Task 0) else body thường, Content-Type từ `response_content_type`. Timeout: response Task 0; execution vẫn chạy. (3) interval+max-wait là `const` doc rõ. Không LISTEN/NOTIFY.
- [ ] **Bước 4: PASS** (+scoped). **Bước 5: Commit** `feat(workflow): synchronous Webhook response (wait/returnOutputs/responseContentType/responseCode)`.

---

### Task 5: Docs (repo egret-data/docs, nhánh docs/workflow-webhook-parity)
- [ ] **Bước 1:** ghi 5 feature (webhook labels; stopAfter disable + route reject; `when` theo nhánh Task 0; sync response wait/returnOutputs/responseContentType/responseCode), nêu **divergence** §5 spec (poll vs reactive latency; stopAfter enforce ở route; nhánh (b) no-op nếu có).
- [ ] **Bước 2:** build docs (hoặc gate `build` PR). **Bước 3: Commit** `docs(workflow): Webhook trigger parity (labels/stopAfter/when/sync response)`.

---

## Self-Review
- Phủ spec: labels→T1; stopAfter→T2; when→T3; sync→T4; census→T0+T3+T4; divergence→T5; testing gấp vào mỗi task. Không gap.
- Placeholder: giá trị hoãn (nhánh `when`, timeout/default wait, nguồn outputs, delta census) là **deliverable đo Task 0**, không phải placeholder impl — mỗi bước impl nêu file:line + edit cụ thể. `render_trigger_labels` là Schedule-shaped → T1 viết biến thể webhook (đã ghi).
- Thứ tự: đo → dễ (labels) → stopAfter → when → sync (nặng nhất) → docs.
