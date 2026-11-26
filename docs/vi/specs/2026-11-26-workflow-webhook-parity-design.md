# Gói 11c — Webhook trigger parity — Thiết kế

**Trạng thái:** thiết kế đã duyệt (scope + quyết định chốt), formalize trên worktree #11c.
**Ngày:** 2026-11-26
**Roadmap:** egret-workflow parity Kestra, gói **#11 (triggers)**, phần **11c**. Chương trình #11: 11a (bind `trigger.*`, MERGED #915) → 11b (Schedule parity, MERGED #916) → **11c** (Webhook parity, tài liệu này) → 11d (Flow-trigger listener conditions + lifecycle còn lại). User chọn **tất cả feature Webhook trong 11c** (gồm cả sync response, không tách).

> Bản VI dịch diễn giải; chi tiết kỹ thuật + file:line xem bản EN (`docs/en/specs/2026-11-26-workflow-webhook-parity-design.md`).

## 1. Mục tiêu
Cho Webhook trigger egret khớp Kestra ở 5 feature đang refuse hoặc bỏ qua:
1. **`labels`** — label riêng của trigger lên execution được fire (parse, bỏ qua; nay `not_honoured`).
2. **`stopAfter`** — disable webhook trigger sau khi execution đạt terminal khớp (parse, bỏ qua).
3. **`when`** — điều kiện truthiness đánh giá lúc request (nay parse, `not_honoured` warning qua `lift_of` Package 11, bỏ qua runtime — **không** hard-refused; **hành vi Kestra chính xác là đo Task 0**, §4 D1).
4. **Synchronous response** — `wait`/`returnOutputs`/`responseContentType`/`responseCode`: giữ HTTP response mở tới khi execution terminal, rồi trả outputs/content-type/status (nay cả 4 refused 409 qua `declared_unsupported`).

Ngoài scope (vẫn refused — cần object store, gói riêng): `fetchType: STORE` (`StoreBody`) và `multipart/form-data` (`Multipart`).

Dựa trên **11a** (bind `trigger.*`: webhook record `{body,headers,parameters,uri}` đã bind) + khuôn mẫu **11b** (`render_trigger_labels`, hook `worker/stop_after.rs on_terminal`, hai cơ chế disable).

## 2. Sự thật đã đo (fire path webhook egret)
Seam fire webhook duy nhất = `webhook_routes.rs::handle()`. Luồng hiện tại: rate-limit → resolve workspace + tìm flow/trigger + render & so `key:` → **reject nếu FLOW disabled** (`:603`; **KHÔNG check cột `disabled` của trigger**) → **refuse sync key 409** (`declared_unsupported`, `:618`) → `build_output` → render `inputs:` → **start fire-and-forget** `start_execution_with_trigger` (`claim.rs:4085`) → trả `WebhookResponse { state:"CREATED" }`. **when/labels/stopAfter không bao giờ được đọc.**

- `models::Trigger`: `when` (`:49`, default `"true"`), `labels` (`:51`), `stop_after` (`:60`) — đúng field 11b đã honor cho Schedule.
- `models::Webhook`: `wait` (`:164`), `return_outputs` (`:166`), `response_content_type` (`:168`), `response_code` (`:170`).
- `declared_unsupported(&Webhook)` trả first của Wait/ReturnOutputs/ResponseContentType/ResponseCode (`webhook.rs:348`); `spec::schema` cũng gọi → editor không gợi ý các key này.
- `workflow.executions.state` (`text NOT NULL`) giữ state cuối (wire spelling Kestra). **Không có HTTP-blocking await nào**; subflow join qua `settle` recursion nội bộ, không phải chờ chặn request → sync handler PHẢI poll.
- Hai cơ chế disable (11b): cột `workflow.triggers.disabled` + bảng `workflow.disabled_triggers` (sync tái dẫn cột từ bảng). Disable phải ghi **cả hai** để sống qua sync. `worker/stop_after.rs on_terminal` đã làm, nhưng **gate chỉ `type=Schedule`**.

## 3. Semantics Kestra (mục tiêu parity)
- **labels** — label webhook trigger (render với record) thêm vào execution, cạnh flow labels; thứ tự: trigger trước, rồi còn lại (11b).
- **stopAfter** — execution do trigger khởi đạt end state ∈ stopAfter → disable trigger (webhook: POST tiếp bị reject).
- **when** — **Task 0 đo artifact:** controller webhook Kestra CÓ eval `when` lúc request không, và trả gì khi falsy? `webhook.rs` (đo kỹ artifact path key/inputs) **không nhắc** `when` → tín hiệu thật là controller có thể không eval. Hai nhánh faithful (§4 D1).
- **sync response** — `wait:true`: Kestra giữ response (Mono reactive subscribe queue) tới khi execution terminal hoặc timeout, rồi trả. `returnOutputs:true` → body = outputs terminal; `responseCode` → status; `responseContentType` → Content-Type. Default `wait:false` trả ngay (hiện tại). **Task 0 đo** timeout default, response khi timeout, `responseCode` default & có dẫn từ state không, nguồn outputs chính xác.

## 4. Thiết kế & quyết định
- **D4 — labels (nhỏ):** ở `handle()` trước `start_execution_with_trigger`, render `labels` trigger với webhook record (bind 11a) qua `render_trigger_labels` (tái dùng 11b) + prepend trước flow labels. Flow labels không đổi. Fail-safe lỗi render (11b).
- **D5 — stopAfter (nhỏ-vừa):** (a) mở `worker/stop_after.rs on_terminal` nhận **`type=Webhook`** (đã ghi cả hai cơ chế disable + null `next_evaluation_date` = no-op cho webhook). (b) ở `handle()`, sau check flow-disabled, reject khi **TRIGGER** disabled — đọc `triggers.disabled` cho `(flow_id,trigger_id)`, trả 409. Check phía route = analogue của filter claim `AND NOT t.disabled` scheduler (webhook không qua claim → gate ở route). Divergence: disable keyed theo trigger id, enforce ở route.
- **D1 — when (vừa): Task 0 chọn nhánh.**
  - *Nhánh (a) — Kestra eval `when` lúc request:* sau khi key khớp, render `when` với record; falsy → **không** tạo execution, trả đúng cái Kestra trả (Task 0 pin status/body). Gỡ refuse. Lỗi render → fail-safe (không execution, 4xx), như `runIf`.
  - *Nhánh (b) — Kestra không eval webhook `when`:* honor = accept-but-inert. Gỡ refuse, vẫn parse, doc là no-op, bỏ khỏi tập refused/warned để census đúng.
  - Webhook `when` nay **không** hard-refused (không ở `WEBHOOK_REFUSED`/`declared_unsupported`): là `not_honoured` warning qua `keys.rs lift_of` `(WEBHOOK,"when")=>Package(11)` (`keys.rs:185`) + bỏ qua runtime. Nhánh (a) = bỏ warning + honor; nhánh (b) = bỏ warning + doc no-op. Task 0 pin treatment `lift_of`/census cần đổi. (Sync key thì CÓ hard-refused → cần LIFT-REFUSAL grep, xem D-SYNC.)
- **D-SYNC — sync response (lớn, cross-seam):**
  - Gỡ 4 sync key khỏi `declared_unsupported` (`webhook.rs:348`) + refuse route (`:618`) + `WEBHOOK_REFUSED`/schema.
  - `wait:false` (default): fire-and-forget không đổi.
  - `wait:true`: sau start, **poll** `executions.state` theo execution id (loop bound: interval cố định + max-wait) tới khi terminal (`RunState::is_terminal`). Khi terminal: `responseCode` render→status (default Task 0 pin); `returnOutputs:true`→body = outputs flow-level (Task 0 pin nguồn đọc); `responseContentType`→Content-Type. Timeout trước terminal → Task 0 pin câu trả lời Kestra (execution as-is hay 4xx); execution vẫn chạy.
  - **Không LISTEN/NOTIFY** (egret chưa có) → bounded poll; interval & max-wait là hằng impl, doc rõ, chọn sao cho execution chậm không giữ connection vô hạn. Divergence latency, không phải semantic (§5).

## 5. Divergence (ghi docs PR)
- **Poll vs reactive:** egret poll state theo interval thay vì subscribe queue → response chờ có thể trễ terminal ≤ 1 interval. Semantics (outputs/code/content-type) khớp.
- **Webhook stopAfter enforce ở route:** disable check ở `webhook_routes.rs`, không scheduler (webhook không qua claim).
- Nếu Task 0 thấy Kestra không eval webhook `when` → kết quả nhánh (b) (accepted no-op).

## 6. Census / Banks (Task 0, đo trước)
Gỡ sync key + (nhánh a) `when` dịch flow Webhook corpus từ refused/warned sang valid. Task 0 đo lại `property-treatment.json` (probe-gen + golden-test như 11b), ghi delta honoured/warned/ignored/refused + aggregate mới (nay sau 11b: 1218/3/360/121, tổng 1702). `labels`/`stopAfter` là runtime/scheduler, không nên dịch census validate/publish (xác nhận). Oracle executor không drive route webhook (`harvest.py` không gọi POST webhook) → `baseline.txt` không đổi; mọi claim judge bằng test của crate này.

## 7. Kiểm thử
Task 0 pin: nhánh `when` (a/b) + response falsy; `wait` timeout default + response timeout; `responseCode` default + nguồn; `returnOutputs` body/outputs source; delta census; site treatment `when` (`keys.rs lift_of` + census). Test per-feature: labels order; stopAfter disable + route reject webhook disabled + terminal không-khớp giữ enabled; `when` gate theo nhánh Task 0; `wait` terminal→outputs/code/content-type; `wait` timeout; `wait:false` fire-and-forget không đổi. Scoped **phải** kèm `--test routes --test execution_timestamps`. Migration-free dự kiến; Task 0 xác nhận.

## 8. Ràng buộc toàn cục
- Chat VI; code/commit/docs EN. Conventional Commits; `Closes #N` chỉ body PR.
- Trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; body PR kết dòng Claude Code.
- Không commit secret/scratch. Stage theo path; không `git add -A`/`target*/`.
- MỘT build nặng/lần, foreground. Merge CI xanh, không `--admin`, `cargo fmt --all --check` trước.
- **LIFT-REFUSAL discipline:** trước khi gỡ refuse `when`/sync, grep code + message + fixture YAML across `src/ tests/ oracle-exec/`.
- Số migration chung — chỉ nếu cần cột (không dự kiến); lấy số free kế lúc plan + renumber.
- Scoped `egret-workflow` kèm `--test routes --test execution_timestamps`.
