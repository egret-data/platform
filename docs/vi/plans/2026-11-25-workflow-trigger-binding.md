# Bind `trigger.*` trong task render (gói 11a) — Kế hoạch triển khai

> **Cho worker agent:** SUB-SKILL BẮT BUỘC: superpowers:subagent-driven-development. Bước dùng checkbox (`- [ ]`).
> **Lưu ý:** code/chữ ký/code block giữ như bản EN (`docs/en/plans/2026-11-25-workflow-trigger-binding.md`). Bản VI dịch diễn giải; triển khai đọc kèm EN.

**Mục tiêu:** Khi flow khởi bởi trigger, bind biến trigger vào render context task của flow được khởi để `{{ trigger.body }}`/`{{ trigger.date }}`/`{{ trigger.outputs.x }}` render.

**Kiến trúc (sửa sau Task 0):** egret ĐÃ persist trigger record trong cột `workflow.executions.execution_trigger` sẵn có (migration 0140), shape `{ "id":.., "type":.., "variables":{..} }`. Schedule (`fire.rs schedule_trigger_record` → `{date,next,previous}`) và Flow (`flow_trigger/process.rs flow_trigger_record` → `{state,flowId,namespace,executionId,startDate,endDate?,lastTaskId?,executionLabels,outputs?}`) đã populate; **replay copy verbatim** (replay.rs, parity Kestra `Execution.java:422`); subflow/manual/webhook để NULL. Chỉ thiếu: (1) `build_context` không đọc nó; (2) route Webhook set `trigger: None`. Nên 11a = **bind cột sẵn có trong build_context + thêm capture Webhook**. Không migration, không cột mới, không field `NewExecution` mới.

**Tech Stack:** Rust, `crates/egret-workflow`; sqlx/Postgres.

## Ràng buộc toàn cục
- Chat VI; code/commit/docs EN. Conventional Commits; `Closes #N` chỉ body PR.
- Trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; body PR kết dòng Claude Code.
- Không commit secret/scratch. Stage theo path; không `git add -A`/`target*/`.
- MỘT build nặng/lần, foreground. Merge CI xanh, không `--admin`, `cargo fmt --all --check` trước.
- **Scoped `egret-workflow` PHẢI kèm `--test routes --test execution_timestamps`.**
- Tương tác `secret()`: KHÔNG (record trigger không secret-typed; record Flow đã mask SECRET output theo #6 D9).
- **GUARD 1 (gate theo fire):** chỉ bind `trigger` khi `execution_trigger` non-null (execution thực sự do trigger khởi). KHÔNG bao giờ bind root `trigger` rỗng/vô điều kiện. Nghĩa vụ chứng minh: chạy STANDALONE (API POST) flow đọc `{{ trigger.* }}` PHẢI vẫn fail-render. Task 0 đo: oracle driver chạy mọi flow standalone, không fire trigger → `execution_trigger` NULL → task fail → match golden FAILED; dưới gate này KHÔNG tuple ratchet/census nào (LIVE 299, VALIDATE_REFUSED 21, executor_matches_golden) dịch. Bind rỗng vô điều kiện sẽ regress 5 flow FAILED→SUCCESS vs golden — cấm.
- **GUARD 2 (chỉ context b):** chỉ bind `trigger` trong render TASK của flow được khởi (`build_context` qua `load_with`). KHÔNG bind trong render `inputs:`/`when:` CỦA CHÍNH Flow-trigger (render.rs ~1086-1097) — giữ unbound (parity Kestra thật, w13).
- **Transform record→render:** Kestra bind `trigger` = map `variables` của record + thêm `_context={id,type}`. Nên `{{ trigger.date }}`→`variables.date`, `{{ trigger._context.type }}`→fqcn.

## Cấu trúc file
- `src/worker/render.rs` — `Surface` thêm `trigger: Value`; `load_with` SELECT thêm `execution_trigger` + build giá trị transform; `build_context` chèn key `trigger` khi non-null.
- `src/routes.rs` (site tạo execution webhook ~4587) + helper mới `webhook_trigger_record(...)` (cạnh webhook Output ở `trigger/webhook.rs`) — set `NewExecution.trigger` cho webhook.
- Tests: `tests/trigger_binding.rs` (mới); unit render.rs.

---

### Task 0: Đo — HOÀN TẤT (ghi ở ledger)
Outcome A — census ỔN ĐỊNH dưới gate non-null. Phát hiện cột `execution_trigger` + record sẵn có (bản plan sửa này). Không code. (Giữ như marker; không re-dispatch.)

---

### Task 1: Bind `trigger` trong `build_context` từ `execution_trigger`
**File:** `src/worker/render.rs` (`Surface` ~93/111-129; `load_with` SELECT ~220; khối chèn ~807-812). Test: `tests/trigger_binding.rs`.
**Produces:** `{{ trigger.* }}` render trong task flow được khởi (Schedule + Flow ngay vì cột đã populate; Webhook sau Task 2).
**Transform record→render** (helper nhỏ `fn trigger_binding(record: Value) -> Value`): input record `{"id":..,"type":..,"variables":{..}}` (hoặc Null) → output Null khi null/thiếu `variables`; else object `variables` thêm key `_context` = `{"id":record.id,"type":record.type}`.
- [ ] **Bước 1: Tests fail** (`tests/trigger_binding.rs`, DB-gated; mirror harness schedule-fire + flow-trigger — grep test `schedule_trigger_record`/`flow_trigger_record` + `w12-schedule-trigger.json`/`w13-flow-trigger.json`): Schedule→`{{ trigger.date }}` + `{{ trigger._context.type }}`; Flow→`{{ trigger.outputs.x }}`/`{{ trigger.executionId }}`; replay Schedule vẫn render `{{ trigger.date }}` (cột copy — chứng minh replay-survival); standalone (manual/API)→`{{ trigger.x }}` FAIL (execution_trigger NULL → không key) = chứng minh GUARD-1. Thêm unit render.rs cho `trigger_binding` (null→Null; `{id,type,variables:{date}}`→`{date,_context:{id,type}}`).
- [ ] **Bước 2: Chạy — FAIL.**
- [ ] **Bước 3: Implement** (code: xem EN) — `Surface` thêm `trigger: Value`; `load_with` thêm `execution_trigger` vào SELECT + `trigger_binding(row.get::<Option<Value>,_>("execution_trigger").unwrap_or(Null))`; sau render.rs:812 chèn `if !self.trigger.is_null() { cx.insert("trigger", ...) }`; thêm helper. KHÔNG đụng render inputs/when của Flow-trigger (render.rs ~1086-1097).
- [ ] **Bước 4: PASS** (+ `--test routes --test execution_timestamps`). **Bước 5: Commit** `feat(workflow): bind trigger.* in task render context from execution_trigger`.

---

### Task 2: Capture Webhook — populate `execution_trigger` cho webhook
**File:** `src/trigger/webhook.rs` (thêm `webhook_trigger_record`); `src/routes.rs` (tạo execution webhook ~4587, hiện `trigger: None`).
- [ ] **Bước 1: Integration test fail**: webhook khởi, task đọc `{{ trigger.body }}` render body JSON POST; `{{ trigger.headers.x }}` render header; GET không body → `{{ trigger.body }}` fail (không key `body`), Kestra parity.
- [ ] **Bước 2: Chạy — FAIL** (webhook set `trigger: None`).
- [ ] **Bước 3: Implement** `webhook_trigger_record(trigger_id, out) -> {"id":..,"type":WEBHOOK_TYPE,"variables":<Output>}` (Output serialize với `skip_serializing_if="Option::is_none"` giữ field vắng); ở routes.rs ~4587 thay `trigger: None` bằng `Some(webhook_trigger_record(<id>, <Output đã build cho webhook inputs render>))` — tái dùng Output, không tính lại.
- [ ] **Bước 4: PASS** (+ scoped). **Bước 5: Commit** `feat(workflow): populate execution_trigger for webhook fires (trigger.body/headers/parameters)`.

---

### Task 3: Docs (repo egret-data/docs, nhánh docs/workflow-trigger-binding)
- [ ] **Bước 1:** `platform/workflows/flows/expressions.md` ghi `{{ trigger.* }}` per-type (Schedule `date`/`next`/`previous`; Webhook `body`/`headers`/`parameters`; Flow `outputs`/`executionId`/`state`/…; `_context.id`/`_context.type`), vắng cho execution không-trigger, replay giữ nó.
- [ ] **Bước 2:** Build docs (hoặc gate `build` PR). **Bước 3: Commit** `docs(workflow): {{ trigger.* }} in task expressions`.

---

## Self-Review
- **Phủ spec:** persist → đã có (execution_trigger 0140); capture Schedule/Flow → đã có; capture Webhook → Task 2; bind → Task 1; replay-survival → free (replay copy cột; Task 1 test); subflow/manual NULL → free; Leaf NULL → free; census Task-0 → xong; docs → Task 3. Không gap.
- **Placeholder:** code render-side chính xác; Output webhook tái dùng giá trị route đã build (grep-located).
- **Nhất quán type:** `Surface.trigger: Value`; `trigger_binding(Value)->Value`; `webhook_trigger_record(&str,&Output)->Value`; record `{id,type,variables}` nhất quán với record schedule/flow sẵn có.
