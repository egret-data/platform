# Bind `trigger.*` trong task render (gói 11a) — Kế hoạch triển khai

> **Cho worker agent:** SUB-SKILL BẮT BUỘC: superpowers:subagent-driven-development. Bước dùng checkbox (`- [ ]`).
> **Lưu ý:** code/chữ ký hàm/code block giữ nguyên như bản EN (`docs/en/plans/2026-11-25-workflow-trigger-binding.md`). Bản VI dịch phần diễn giải; khi triển khai đọc kèm bản EN để lấy code chính xác.

**Mục tiêu:** Khi flow được khởi bởi trigger, bind biến trigger vào render context task của flow được khởi để `{{ trigger.body }}`/`{{ trigger.date }}`/`{{ trigger.outputs.x }}` render.

**Kiến trúc:** Persist output trigger của mỗi execution vào cột `executions.trigger` jsonb mới (migration 0243); mỗi trong 3 fire path có kiểu ghi Output đã tính (+`_context`) vào đó lúc tạo execution; `build_context` bind key render top-level `trigger` từ đó. Chỉ bind — không gỡ refusal.

**Tech Stack:** Rust, `crates/egret-workflow`; sqlx/Postgres; migration dưới `migrations/`.

## Ràng buộc toàn cục
- Chat VI; code/commit/docs EN. Conventional Commits; `Closes #N` chỉ body PR.
- Trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; body PR kết dòng Claude Code.
- Không commit secret/scratch. Stage theo path; không `git add -A`/`target*/`.
- MỘT build Rust nặng/lần, foreground subagent. Merge CI xanh, không `--admin`, `cargo fmt --all --check` trước.
- **Scoped `egret-workflow` PHẢI kèm `--test routes --test execution_timestamps`.**
- **Thêm cột vào `workflow.executions` ⇒ cập nhật MỌI SELECT nạp nó vào `Execution`/render context** — Task 0 liệt kê (exhaustiveness: bỏ sót một SELECT âm thầm mất giá trị). Đổi chart/golden phải `cargo test -p egret-config` (bài học 10b: golden exact-RBAC).
- Migration free kế = **0243** (mới nhất 0242). Xác nhận lại lúc build, renumber nếu bị chiếm.
- Tương tác `secret()`: KHÔNG — output trigger egret không secret-typed; không đụng 10a.

## Cấu trúc file
- `migrations/0243_execution_trigger.sql` (mới) — `ALTER TABLE workflow.executions ADD COLUMN trigger jsonb`.
- `src/worker/render.rs` — `Surface` thêm `trigger: Value`; `load_with` SELECT + build; `build_context` chèn key `trigger`.
- `src/worker/spawn.rs`, `replay.rs`, seam tạo execution (`start_execution_in_with`/`NewExecution`) — luồn payload trigger vào INSERT (None cho subflow/manual; tính cho 3 trigger; copy cho replay).
- `src/trigger/{webhook,schedule,flow}.rs` + `flow_trigger/process.rs` + `scheduler/` — tạo payload trigger per-type lúc fire.
- Tests: `tests/trigger_binding.rs` (mới); unit trong render.rs.

---

### Task 0: Đo (census + wiring capture) — load-bearing, hầu như không production code
**Vì sao:** chốt sự thật các task sau mã hoá, trên cây post-10b.
- [ ] **Bước 1 — Census/Banks.** Từ `crates/egret-workflow`: grep corpus/oracle xem flow nào đọc `{{ trigger.* }}` trong TASK (không phải `inputs:` của trigger) và đang ở bucket fail/refused. Ghi kèm file:line: **(A) census ổn định** (kỳ vọng) hoặc **(B) census dịch** (liệt kê + quyết định). (B) → báo controller trước Task 1. (Lệnh grep: xem bản EN.)
- [ ] **Bước 2 — Liệt kê MỌI SELECT executions nạp render/Execution** (`grep -rn "FROM workflow.executions" src/`): liệt kê site phải thêm `trigger` vào column list (tối thiểu loader render `worker/render.rs load_with`; ghi loader API/DTO nếu muốn expose — 11a chỉ cần đường render).
- [ ] **Bước 3 — Chốt shape Output per-type + seam tạo execution** (file:line): Webhook `Output` fields (`{body?,uri?,headers?,parameters?}`); Flow Output + nơi `flow_trigger::process` tạo execution (`start_execution_in_with`/`NewExecution`); Schedule fire site + `schedule_date` + API cron tính `next`/`previous`; struct `NewExecution` + chữ ký `start_execution_in_with` + column list INSERT ở `spawn.rs`(~417) & `replay.rs`(~244).
- [ ] **Bước 4 — Ghi finding vào task report.** Không đổi source. Report là nguồn tên field/chữ ký cho Task 1-6.

---

### Task 1: Migration + luồn cột `trigger` qua tạo execution (chưa bind)
**File:** tạo `migrations/0243_execution_trigger.sql`; sửa seam tạo execution (`NewExecution`/`start_execution_in_with`) + INSERT ở `spawn.rs`, `replay.rs`.
**Produces:** row executions mang `trigger jsonb NULL`; `NewExecution` thêm field `trigger: Option<serde_json::Value>` (None cho subflow/manual).
- [ ] **Bước 1: Migration** (SQL: xem bản EN) — `ALTER TABLE workflow.executions ADD COLUMN trigger jsonb` + COMMENT.
- [ ] **Bước 2: Thêm `trigger: Option<Value>` vào `NewExecution`**, default None; luồn vào column list + bind value (NULL khi None) của `start_execution_in_with`.
- [ ] **Bước 3: spawn.rs (subflow child) — `trigger` NULL** (thêm cột vào INSERT với literal NULL cho tường minh).
- [ ] **Bước 4: replay.rs — copy `trigger` của execution gốc** (`(SELECT trigger FROM workflow.executions WHERE id = <gốc>)`).
- [ ] **Bước 5: Test DB-gated** mỗi path: subflow NULL; replay copy. Chạy với DB env worktree.
- [ ] **Bước 6: Commit** `feat(workflow): executions.trigger column + thread through execution creation`.

---

### Task 2: Bind `trigger` trong `build_context`
**File:** `src/worker/render.rs` (`Surface` ~93/111-129; `load_with` ~212; khối chèn ~807-812).
**Consumes:** cột `trigger`. **Produces:** `{{ trigger.* }}` render trong task khi cột non-null.
- [ ] **Bước 1: Unit test fail** (render.rs): `Surface` có `trigger` = `json!({"date":...,"_context":{...}})` → `build_context` cho context mà `{{ trigger.date }}` render; `Surface` với `trigger: Value::Null` → KHÔNG chèn key `trigger`.
- [ ] **Bước 2: Chạy — FAIL.**
- [ ] **Bước 3: Thêm field + SELECT + chèn** — `Surface` thêm `trigger: Value`; `load_with` thêm `trigger` vào SELECT + `row.get::<Option<Value>,_>("trigger").unwrap_or(Value::Null)`; sau render.rs:812 chèn `if !self.trigger.is_null() { cx.insert("trigger", self.trigger.clone()); }` (code: xem EN).
- [ ] **Bước 4: Chạy — PASS** (+ scoped binaries vì loader SELECT đổi).
- [ ] **Bước 5: Commit** `feat(workflow): bind trigger.* in task render context`.

---

### Task 3: Capture Schedule — `{date,next,previous}` + `_context`
**File:** scheduler fire path (Task 0 chốt) + `trigger/schedule.rs`.
- [ ] **Bước 1: Integration test** (`tests/trigger_binding.rs`): execution do schedule khởi, task đọc `{{ trigger.date }}` render ngày fire; `{{ trigger._context.type }}` = FQCN Schedule.
- [ ] **Bước 2-3:** build payload `{date,next,previous,_context}` (next/previous từ cron qua API sẵn có — Task 0 chốt), truyền `NewExecution.trigger = Some(..)`.
- [ ] **Bước 4: PASS** (+ scoped). **Bước 5: Commit** `feat(workflow): bind Schedule trigger.date/next/previous`.

---

### Task 4: Capture Webhook — `{body?,headers,parameters}` + `_context`
**File:** webhook route/fire path (`trigger/webhook*`).
- [ ] **Bước 1: Integration test**: webhook khởi, task đọc `{{ trigger.body }}` render body JSON đã POST; `{{ trigger.headers.x }}` render header; GET không body → `{{ trigger.body }}` fail (không key `body`), Kestra parity.
- [ ] **Bước 2-3:** lưu `Output` webhook (đã build để render `inputs:`) + `_context` làm `NewExecution.trigger`; tái dùng struct `Output` (`skip_serializing_if="Option::is_none"` giữ field vắng).
- [ ] **Bước 4: PASS** (+ scoped). **Bước 5: Commit** `feat(workflow): bind Webhook trigger.body/headers/parameters`.

---

### Task 5: Capture Flow-trigger — Flow Output + `_context`
**File:** `flow_trigger/process.rs` (+ `trigger/flow.rs` Output).
- [ ] **Bước 1: Integration test**: flow-trigger khởi, task đọc `{{ trigger.outputs.x }}` và `{{ trigger.executionId }}` render output/id của execution nguồn.
- [ ] **Bước 2-3:** lưu Flow Output (`{executionId,namespace,flowId,flowRevision,state,startDate,endDate,firstFailedTaskId,lastTaskId,executionLabels,outputs}`) + `_context` làm `NewExecution.trigger` tại caller `start_execution_in_with` trong `process.rs`.
- [ ] **Bước 4: PASS** (+ scoped). **Bước 5: Commit** `feat(workflow): bind Flow trigger.outputs/executionId/...`.

---

### Task 6: Replay/restart copy + subflow/manual NULL — verify end-to-end
**File:** chỉ tests (Task 1 đã wiring replay-copy + subflow-NULL); task này chứng minh toàn đường.
- [ ] **Bước 1: Tests**: (a) replay execution webhook vẫn render `{{ trigger.body }}` (cột copy); (b) task subflow child đọc `{{ trigger.x }}` fail (NULL); (c) execution khởi thủ công (API) đọc `{{ trigger.x }}` fail (NULL), không key `trigger`.
- [ ] **Bước 2: Chạy.** (a) fail → sửa replay-copy (Task 1 B4); (b)/(c) fail → có path tạo execution set nhầm `trigger`.
- [ ] **Bước 3: Commit** `test(workflow): trigger.* survives replay, absent on subflow/manual`.

---

### Task 7: Docs (repo egret-data/docs, nhánh mới docs/workflow-trigger-binding)
- [ ] **Bước 1:** `platform/workflows/flows/expressions.md` (hoặc doc triggers) ghi `{{ trigger.* }}` khả dụng per-type (Schedule `date`/`next`/`previous`; Webhook `body`/`headers`/`parameters`; Flow `outputs`/`executionId`/…; `_context.id`/`_context.type`), và vắng cho execution không-trigger; nêu replay giữ nó.
- [ ] **Bước 2:** Build docs (hoặc dựa gate `build` của docs PR). **Bước 3: Commit** `docs(workflow): {{ trigger.* }} in task expressions`.

---

## Self-Review
- **Phủ spec:** persist (cột mới) → Task 1; capture 3 loại → Task 3/4/5; bind → Task 2; replay-copy/subflow-NULL → Task 1+6; Leaf NULL → Task 1 (default None); không gỡ refusal → không gì gỡ; census Task-0 → Task 0; tests → mỗi task; docs → Task 7. Không gap.
- **Placeholder:** code render-side chính xác; capture-side nêu đúng struct/site, chỉ hoãn wiring field chính xác cho Task 0 (hợp lệ — task đo).
- **Nhất quán type:** `NewExecution.trigger: Option<Value>` + `Surface.trigger: Value` nhất quán; payload per-type là `serde_json::Value` object có `_context`.
