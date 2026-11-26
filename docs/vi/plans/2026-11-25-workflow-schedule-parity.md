# Schedule trigger parity (gói 11b) — Kế hoạch triển khai

> **Cho worker agent:** SUB-SKILL BẮT BUỘC: superpowers:subagent-driven-development. Bước dùng checkbox (`- [ ]`).
> **Lưu ý:** code/chữ ký/code block + file:line giữ như bản EN (`docs/en/plans/2026-11-25-workflow-schedule-parity.md`). Bản VI dịch diễn giải; triển khai đọc kèm EN.

**Mục tiêu:** Cho Schedule trigger egret khớp Kestra ở `when`, `lateMaximumDelay`, `recoverMissedSchedules`, `labels`, `stopAfter`.

**Kiến trúc:** Gỡ refuse fire-time (`when`, `lateMaximumDelay`) + honor các field parse-nhưng-bỏ, hook mỗi cái ở seam đã đo: `when` ở nửa fire/IO (`fire.rs`), `lateMaximumDelay` qua `handle_max_delay` đã port, `labels` ở `start()`, `recoverMissedSchedules` qua pass startup scheduler, `stopAfter` qua seam execution-terminal. Dựa trên bind `trigger.*` của 11a.

## Ràng buộc toàn cục
- Chat VI; code/commit/docs EN. Conventional Commits; `Closes #N` chỉ body PR.
- Trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; body PR kết dòng Claude Code.
- Không commit secret/scratch. Stage theo path; không `git add -A`/`target*/`.
- MỘT build nặng/lần, foreground. Merge CI xanh, không `--admin`, `cargo fmt --all --check` trước.
- **Scoped `egret-workflow` PHẢI kèm `--test routes --test execution_timestamps`.**
- **LIFT-REFUSAL DISCIPLINE (bị 9a×2, 9b):** trước khi gỡ refuse `when`/`lateMaximumDelay`, grep code + message + fixture YAML across `src/ tests/ oracle-exec/` (`UnsupportedWhenCondition`/`UnsupportedLateMaximumDelay`, message `schedule.rs:1294-1301`, `schedule_not_resolvable`, flow corpus keyed refused Schedule).
- Số migration chung — nếu cần cột (recover guard), số free kế = 0244 (11a đã lấy 0243), renumber nếu bị chiếm.
- `secret()`/10a không đụng.

## Cấu trúc file
Xem EN. Chính: `trigger/schedule.rs` (gỡ refuse when ~1353-1356 + lateMaximumDelay ~1367-1368; `handle_max_delay` ~1500 đã port); `scheduler/fire.rs` (when ở fire_one/resolve ~245, labels ở start() ~473-494); seam startup scheduler (recover, Task 0 pin); `trigger/switch.rs`+`trigger/mod.rs` (tái dùng RecoverMissed + resume_after_backfill/last_tick_at_or_before); seam settle (stopAfter→triggers.disabled); tests.

---

### Task 0: Đo — census + pin 2 seam chưa đo (load-bearing)
- [ ] **Bước 1 — Census/Banks + lift-refusal grep** (lệnh: xem EN). Ghi flow Schedule corpus ở VALIDATE_REFUSED/warn `schedule_not_resolvable` vì when/lateMaximumDelay + constants driver.rs dịch. Lưu ý w12-schedule-trigger.json (driver CÓ fire Schedule) → when/lateMaximumDelay có thể dịch oracle executor. Chốt reconcile trước code.
- [ ] **Bước 2 — Pin seam recover onStart.** Tìm nơi scheduler start/chạy lần đầu (trước vòng fire_due). Quyết guard once-per-startup dùng state sẵn (`evaluated_at`+ranh giới) hay cần cột mới. Ghi cơ chế + số migration nếu cần.
- [ ] **Bước 3 — Pin seam settle stopAfter.** Tìm nơi execution đạt terminal + commit; xác nhận thấy được trigger id (`execution_trigger->>'id'`) + flow để tra `(flow_id,trigger_id)` set disabled; xác nhận `stopAfter` reach được (re-parse flow trigger spec).
- [ ] **Bước 4 — Pin parser Duration** cho lateMaximumDelay (parser ISO sẵn có hay quyết parse). Ghi hàm chính xác.
- [ ] **Bước 5 — Ghi finding** vào task report. Không production code. Pin Task 2/4/5.

---

### Task 1: Schedule `labels` lên execution được fire
**File:** `scheduler/fire.rs` `start()` (~473-494).
- [ ] **Bước 1: Test fail** — Schedule có `labels: [{k:v}]` fire execution mang label đó (render với `{{trigger.*}}` nếu template) + backfill labels, đúng thứ tự.
- [ ] **Bước 2: FAIL** (label trigger bị bỏ). **Bước 3:** ở `start()` render `labels` riêng của trigger (`models/trigger.rs:51`) với schedule record, PREPEND trước backfill labels (`fire.rs:473-494`). Thứ tự = trigger rồi backfill. Flow labels không đổi.
- [ ] **Bước 4: PASS** (+scoped). **Bước 5: Commit** `feat(workflow): Schedule trigger labels on the fired execution`.

---

### Task 2: `lateMaximumDelay` — parse + gỡ refuse
**File:** `trigger/schedule.rs` `from_schedule` (~1367-1368) + `handle_max_delay` (~1500, đã port).
- [ ] **Bước 1: Test fail** — tick trễ hơn `now - lateMaximumDelay` bị SKIP (fire tick kế trong hạn); tick trong hạn fire; skip bypass khi backfill chạy (ví dụ đo `schedule.rs:1272-1274`).
- [ ] **Bước 2: FAIL. Bước 3:** parse Duration (parser Task 0 pin); bỏ `return Err(UnsupportedLateMaximumDelay)` (~1367-1368) + mang `Option<Duration>` vào resolved để `next_evaluation_date` chạm `handle_max_delay`. Giữ bypass `backfill==null`. LIFT-REFUSAL grep trước.
- [ ] **Bước 4: PASS** (+scoped). **Bước 5: Commit** `feat(workflow): honor Schedule lateMaximumDelay (skip over-late occurrences)`.

---

### Task 3: `when` — skip-and-advance, đánh giá ở nửa fire/IO
**File:** `trigger/schedule.rs` `resolve` (~1353-1356); `scheduler/fire.rs` `fire_one` (~209-279).
- [ ] **Bước 1: Test fail** — (a) Schedule có `when` fire CHỈ tick khớp, tiến qua không-khớp; (b) ca date-helper `isLastWorkingDay(trigger.date)`; (c) `when` render lỗi → fail SAFE (không fire, `last_error`).
- [ ] **Bước 2: FAIL. Bước 3:** bỏ refuse `UnsupportedWhenCondition` (`schedule.rs:1353-1356`). Ở nửa fire/IO (`fire_one`, có render context), khi trigger có `when` khác "true": sau candidate tick, render `when` với `{schedule,trigger}`=record candidate `{date,next,previous}` (confined engine; 3 date helper có sẵn); falsy→tiến candidate kế, lặp bound; fire tick khớp đầu, set `next_evaluation_date` qua. Lỗi render→không fire, `last_error`. Giữ pure tick cho ca không-when. LIFT-REFUSAL grep trước.
- [ ] **Bước 4: PASS** (+scoped). **Bước 5: Commit** `feat(workflow): honor Schedule when: (skip non-matching ticks)`.

---

### Task 4: `recoverMissedSchedules` — onStart hook kiểu Kestra
**File:** seam startup scheduler (Task 0); tái dùng `RecoverMissed` (`schedule.rs:1312`) + `last_tick_at_or_before` + `resume_after_backfill` (`trigger/mod.rs:328`). Migration chỉ nếu Task 0 thấy cần cột.
- [ ] **Bước 1: Test fail** — mô phỏng restart với trigger `next_evaluation_date` quá khứ (miss): NONE→reseed tick đầu sau now; LAST→most-recent-missed; ALL→giữ (replay). Assert guard once-per-startup: fire_due pass thứ 2 KHÔNG áp lại reseed.
- [ ] **Bước 2: FAIL. Bước 3:** thêm pass startup (seam Task 0) chạy một lần: mỗi trigger enabled có `next_evaluation_date < now` → resolve recover (absent=ALL) → NONE reseed-from-now / LAST last_tick_at_or_before(now) / ALL giữ. Dùng guard Task 0 chọn. Không đụng backfill/route re-enable.
- [ ] **Bước 4: PASS** (+scoped). **Bước 5: Commit** `feat(workflow): recoverMissedSchedules NONE/LAST on scheduler start`.

---

### Task 5: `stopAfter` — disable trigger khi terminal khớp
**File:** seam execution-terminal/settle (Task 0); `workflow.triggers` (`disabled`).
- [ ] **Bước 1: Test fail** — execution do Schedule khởi đạt state ∈ `stopAfter` (vd `[FAILED]`) set trigger `disabled` (không fire nữa); state không khớp giữ enabled; execution không-trigger không disable gì.
- [ ] **Bước 2: FAIL. Bước 3:** ở seam terminal/settle Task 0 pin, khi execution đạt terminal VÀ trigger-started (`execution_trigger` non-null), re-parse flow trigger spec cho `execution_trigger._context.id`, nếu terminal ∈ `stopAfter` → `UPDATE workflow.triggers SET disabled=true WHERE flow_id=$ AND trigger_id=$`. Idempotent.
- [ ] **Bước 4: PASS** (+scoped). **Bước 5: Commit** `feat(workflow): stopAfter disables the trigger after a matching terminal`.

---

### Task 6: Docs (repo egret-data/docs, nhánh docs/workflow-schedule-parity)
- [ ] **Bước 1:** ghi 5 feature Schedule (when + date helpers skip-and-advance; lateMaximumDelay skip; recoverMissedSchedules ALL default/LAST/NONE; labels; stopAfter), nêu divergence nếu Task 0 ghi.
- [ ] **Bước 2:** build docs (hoặc gate `build` PR). **Bước 3: Commit** `docs(workflow): Schedule trigger parity (when/lateMaximumDelay/recover/labels/stopAfter)`.

---

## Self-Review
- Phủ spec: D1 when→T3; D2 lateMaximumDelay→T2; D3 recover→T4; D4 labels→T1; D5 stopAfter→T5; census→T0; docs→T6. Không gap.
- Placeholder: seam fire-path có file:line; 2 seam chưa đo (recover startup, stopAfter settle) + Duration parser + quyết cột recover-guard là measurement Task 0, không "TBD" trong bước impl.
- Thứ tự: dễ→rủi ro (labels, lateDelay, when, rồi recover + stopAfter cần seam Task-0-pin), docs cuối.
