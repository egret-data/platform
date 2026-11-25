# Gói 11b — Schedule trigger parity — Thiết kế

**Trạng thái:** thiết kế đã duyệt (scope + decisions chốt), formalize trên worktree #11b. Migration free kế = 0244 (nếu recover guard cần cột; Task 0 quyết).
**Ngày:** 2026-11-25
**Roadmap:** egret-workflow parity Kestra, gói **#11 (triggers)**, phần **11b**. Chương trình #11: 11a (bind `trigger.*`, ĐÃ MERGE) → **11b** (Schedule parity, tài liệu này) → 11c (Webhook parity) → 11d (Flow-trigger listener conditions + lifecycle còn lại). User chọn **cả 5 feature Schedule trong 11b** (nên `stopAfter` ở đây, không phải 11d).

> Bản VI này dịch diễn giải; chi tiết kỹ thuật + file:line xem bản EN (`docs/en/specs/2026-11-25-workflow-schedule-parity-design.md`).

## 1. Mục tiêu
Cho Schedule trigger của egret khớp Kestra ở 5 feature đang refuse hoặc parse-nhưng-bỏ-qua:
1. **`when:`** — điều kiện Pebble truthiness (đang refuse, `schedule.rs:1353-1356`).
2. **`lateMaximumDelay`** — bỏ occurrence trễ quá duration (refuse, `schedule.rs:1367-1368`).
3. **`recoverMissedSchedules`** — ALL/LAST/NONE khi scheduler restart (parse; chỉ route re-enable đọc; path thường hardcode replay kiểu ALL).
4. **`labels`** — label riêng của trigger lên execution (đang bị bỏ).
5. **`stopAfter`** — disable trigger sau khi execution đạt một trong các state này (parse, không ai đọc).

Dựa trên **11a**: `when` và `labels` render với `{schedule,trigger}` vars 11a đã bind.

## 2. Sự thật đã đo (fire path egret)
Xem bản EN §2 (chi tiết fire.rs/tick/next_evaluation_date/workflow.triggers). Tóm: scheduler `fire.rs fire_due→claim_and_fire→fire_one(resolve: refuse when/lateDelay)→tick(pure, DB-free)→start(dựng NewExecution)`. `next_evaluation_date` precompute due-ness. `handle_max_delay` (port lateMaximumDelay) **dead** hôm nay. `recoverMissedSchedules` chỉ đọc ở switch/re-enable; path thường replay 1-tick-mỗi-lần kiểu ALL. `labels` chỉ lấy từ backfill. `stopAfter` không consumer. State table `workflow.triggers` (0156); backfill `workflow.trigger_backfills` (0192).

## 3. Semantics Kestra (mục tiêu parity)
Xem EN §3. Tóm: **when** = skip-and-advance, đánh giá với `{schedule,trigger}`=`{date,next,previous}` của candidate; date helpers isWeekend/isDayWeekInMonth/isLastWorkingDay. **lateMaximumDelay** = bỏ occurrence `date+delay<now` (bypass khi backfill chạy). **recoverMissedSchedules** default ALL; enforce ở scheduler onStart (NONE reseed-from-now / LAST most-recent-missed / ALL keep). **stopAfter** = disable trigger khi end state ∈ stopAfter. **labels** = trigger labels (render với `{{trigger.*}}`) + backfill labels, append order; flow labels riêng.

## 4. Thiết kế & quyết định
- **D1 — when (medium):** đánh giá ở **nửa fire/IO** (`fire.rs`, có render context), không ở pure tick. Với `when` khác "true": sau khi tính candidate tick, render `when` với `{schedule,trigger}`=record candidate `{date,next,previous}` (dùng confined engine, 3 date helper có sẵn); nếu falsy, tiến candidate kế, lặp (bound now+10y như Kestra); fire tick khớp đầu tiên, set `next_evaluation_date` qua nó. Bỏ refuse `resolve`. Không cột mới. `when` render lỗi → fail-safe (không fire, ghi `last_error`) như runIf.
- **D2 — lateMaximumDelay (small):** parse ISO Duration (`PT1H` — unmeasured, pin ở Task 0/dùng parser sẵn có); bỏ refuse `from_schedule`; `handle_max_delay` (đã port) drop occurrence trễ, bypass khi backfill.
- **D3 — recoverMissedSchedules (lớn nhất): onStart hook kiểu Kestra (USER CHỌN):** thêm pass startup scheduler, chạy MỘT lần trước/khi start: mỗi trigger enabled có `next_evaluation_date < now` (miss) → NONE reseed-from-now / LAST `last_tick_at_or_before(now)` / ALL keep. **Guard once-per-startup** tránh áp mỗi fire. Có cần cột `workflow.triggers` mới (vd `recover_applied_at`/scheduler-epoch) hay dùng `evaluated_at`+ranh giới startup = **quyết định Task 0** — ưu tiên không migration. Backfill + route re-enable giữ semantics hiện tại.
- **D4 — labels (small):** ở `start()` (`fire.rs:473`) render `labels` riêng của trigger với `{{trigger.*}}`, prepend trước backfill labels. Flow labels không đổi.
- **D5 — stopAfter (cross-seam):** khi execution do trigger khởi đạt terminal ∈ `stopAfter`, set `workflow.triggers.disabled`. **Seam:** hook settle/terminal, nhận diện trigger-started qua `execution_trigger._context.id` (+flow), tra `(flow_id,trigger_id)`, set disabled khi state khớp. Task 0 xác định hook settle chính xác.

## 5. Census / Banks (Task 0, đo trước)
Gỡ `when`/`lateMaximumDelay` dịch flow Schedule corpus từ `VALIDATE_REFUSED`/warn `schedule_not_resolvable` sang valid/LIVE. Task 0 grep corpus/oracle + ghi constants dịch (`LIVE`, `VALIDATE_REFUSED`, `DRAFT_ONLY`) + giá trị reconcile — chốt trước code. `labels`/`stopAfter`/`recover` là hành vi runtime/scheduler, không nên dịch census validate/publish (xác nhận). Oracle executor có fire Schedule (w12-schedule-trigger.json) — đo + reconcile nếu dịch.

## 6. Kiểm thử
Xem EN §6. Task 0 đo census + pin Duration format, cơ chế recover guard (cột-hay-không), settle hook stopAfter, w12 oracle. Test per-feature (when skip + date helper + render-error fail-safe; lateDelay skip/within/backfill-bypass; recover NONE/LAST/ALL on restart + once-per-startup; labels order; stopAfter disable/không-disable). Scoped kèm `--test routes --test execution_timestamps`. Live-verify scheduler trên kind nếu được.

## 7. Ràng buộc toàn cục
- Chat VI; code/commit/docs EN. Conventional Commits; `Closes #N` chỉ body PR.
- Trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; body PR kết dòng Claude Code.
- Không commit secret/scratch. Stage theo path; không `git add -A`/`target*/`.
- MỘT build nặng/lần, foreground. Merge CI xanh, không `--admin`, `cargo fmt --all --check` trước.
- **LIFT-REFUSAL discipline (bị 9a×2, 9b):** trước khi gỡ refuse `when`/`lateMaximumDelay`, grep code + message + fixture YAML across `src/ tests/ oracle-exec/`.
- Số migration chung — nếu cần cột (recover guard), lấy số free kế lúc plan (0244) + renumber trên main + PR mở.
- Scoped `egret-workflow` kèm `--test routes --test execution_timestamps`.
