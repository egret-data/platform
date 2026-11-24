# AllowFailure block retry — kế hoạch triển khai (gói 9b)

> **Cho agent:** SUB-SKILL BẮT BUỘC: superpowers:subagent-driven-development.

**Mục tiêu:** Gỡ `unsupported_allow_failure_retry`; để block `retry:` trên `AllowFailure` chạy như mọi flowable khác (child fail kế thừa nó, block ánh xạ terminal sau-retry qua `allow_failure::block_state`), hoà giải census corpus + golden trong cùng commit.

**Kiến trúc:** Kế thừa retry (`retry.rs` `inherited_retry`), `is_retried_kind` (flowable không retry như đơn vị), và `allow_failure::block_state` (ánh xạ WARNING) đã có sẵn. 9b gỡ từ chối validate và verify/tạo run khớp golden, rồi hoà giải census toàn-suite ghép với từ chối đó.

## Ràng buộc toàn cục

- Spec: `docs/en/specs/2026-11-25-workflow-allowfailure-retry-design.md` (bản VI). D1–D5 ràng buộc.
- **Không migration, không API mới, không field model.** Chat VI; code/docs EN.
- Conventional Commits; `Closes #N` chỉ trong mô tả PR; trailer commit `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; thân PR kết `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Stage theo đường dẫn rõ; không `git add -A`; không stage `target*/`. Không commit secret/scratch.
- `cargo fmt --all --check` trước push. MỘT build Rust nặng một lúc; chạy build FOREGROUND.
- **Scoped runs PHẢI gồm `--test routes --test execution_timestamps --test executor`**.
- **Oracle là golden đã vendored** (`oracle-exec/golden.json`), không sửa để khớp egret. **Gỡ từ chối CUỐI, trong cùng commit hành vi đã đúng** (§3 spec), hoà giải toàn blast radius.

## Cấu trúc file

- `src/spec/validate.rs` — gỡ arm `unsupported_allow_failure_retry` (~2307) + unit test (~6193-6235) + header (~58). (Chỉ commit lift.)
- `src/executor/plan.rs`, `src/executor/allow_failure.rs` — doc comment; verify planner giữ `TaskKind::AllowFailure`.
- `src/executor/mod.rs` — self-probe (~4346) + entry `PublishedButRefused` (~4614). (Chỉ commit lift.)
- `src/executor/retry.rs` / `allow_failure.rs` — có lẽ KHÔNG đổi hành vi; xác nhận đường inherited-retry chạm child AllowFailure và block ánh xạ terminal sau-retry.
- `oracle-exec/driver.rs` — đếm LIVE/refused. (Chỉ commit lift.)
- `tests/executor.rs` — census tuple `(301, 279, 22) → (301, 280, 21)` + danh sách `PublishedButRefused`. (Chỉ commit lift.)
- `tests/routes.rs` — lật `a_draft_with_an_allow_failure_saves_...` sang assert validate sạch + giữ control vẫn-bị-từ-chối. (Chỉ commit lift.)
- `tests/` — fixture test hành vi (child luôn fail → block WARNING) + warningOnRetry.

---

### Task 0: Baseline worktree + lập oracle từ trace đã vendored

- Worktree `egret-afretry` (off origin/main sau-9a, branch `feat/workflow-allowfailure-retry`) và container egret-afretry-postgres :5470 / valkey :6470 **đã được controller tạo** — chỉ verify đang chạy và build crate một lần.
- **Grep thẩm quyền (load-bearing):** `grep -rn "unsupported_allow_failure_retry" crates/egret-workflow/src crates/egret-workflow/tests crates/egret-workflow/oracle-exec` và ghi toàn bộ hit vào report — đây là blast radius commit lift phải phủ.
- **Đọc oracle:** trích entry `valids/retry-with-flowable-errors` từ `golden.json` và trace chi tiết từ `traces.json`. Ghi vào spec §6: shape run chính xác (set_kv 1 attempt; retry_block 1 attempt → SUCCESS; run_script 2 attempt FAILED→SUCCESS; execution …RETRYING…→SUCCESS) **và cơ chế attempt 2 success khi golden bỏ run `incr_counter` riêng**. Không test nào sau này được assert terminal mà bước này chưa đặt nền từ trace đã vendored.
- Baseline: chạy scoped suite (`--test executor --test routes --test execution_timestamps`) xác nhận xanh TRƯỚC mọi thay đổi (census `(301, 279, 22)`, từ chối còn hoạt động).

### Task 1: Tạo/verify run khớp golden khi từ chối VẪN hoạt động

- Từ chối chặn `build_plan`, nên drive hành vi qua `plan_stored` / fixture `TaskKind::AllowFailure { retry: Some(...), .. }` trực tiếp (cùng seam 9a dùng cho `pause_plan_gap`). Xác nhận:
  1. planner cho `TaskKind::AllowFailure` với block có `retry:` (sửa `plan.rs` chỉ khi nó flatten thành `Runnable` opaque — D4);
  2. executor, drive stored plan đó, tái hiện golden shape cho `retry-with-flowable-errors` (child 2 attempt FAILED→SUCCESS qua `inherited_retry`; block 1 attempt → SUCCESS qua `block_state`; execution RETRYING→SUCCESS).
- Nếu machinery đã làm được mà không đổi hành vi, task này chỉ là fixture + assertion (như kỳ vọng, rủi ro thấp). Nếu chưa, sửa **tối thiểu** trong `retry.rs`/`allow_failure.rs` và verify lại với golden.
- Census corpus KHÔNG đổi task này (từ chối còn) → scoped suite vẫn xanh. Commit.

### Task 2: Hành vi — cạn-retry → WARNING, và warningOnRetry

- Vẫn với từ chối còn hoạt động (drive qua `plan_stored`/fixture), thêm test executor cho 2 ca corpus flow KHÔNG phủ:
  1. child **luôn** fail cạn `maxAttempts` → child FAILED → block **WARNING** (ánh xạ `allow_failure::block_state`) → execution tiếp tục/terminal theo luật parity-Sequential;
  2. `warningOnRetry: true` log các attempt retry là warning mà không đổi terminal.
- Ghim mỗi ca vào terminal đo được/parity, không đoán. Giữ scoped suite xanh. Commit.

### Task 3: GỠ từ chối + hoà giải corpus trong MỘT commit

- Gỡ arm `unsupported_allow_failure_retry` khỏi `validate.rs` (+ unit test + header); để block validate như flowable thường (children/errors/finally recurse như hiện tại).
- Gỡ entry `PublishedButRefused { id: "valids/retry-with-flowable-errors", … }` khỏi `mod.rs`; cập nhật self-probe (~4346) để assert flow giờ plan/validate sạch (không bị từ chối).
- Cập nhật đếm LIVE/refused trong `oracle-exec/driver.rs` và census tuple `tests/executor.rs` về `(301, 280, 21)` (`21 + 280 = 301`), kèm comment dẫn xuất cùng phong cách các comment hiện có.
- Lật `tests/routes.rs::a_draft_with_an_allow_failure_saves_...` sang assert block validate sạch; giữ control vẫn-bị-từ-chối (một flow thực sự invalid).
- Cập nhật doc comment (`plan.rs`, `allow_failure.rs`).
- Chạy lại **grep** của Task 0 khắp `src/`, `tests/`, `oracle-exec/` — chỉ còn các dòng commit này cố ý viết lại (ví dụ comment kiểu changelog), không còn assertion từ chối sống. Thêm mọi test binary grep nêu vào scoped bar và chạy.
- Driver giờ tái hiện golden cho `retry-with-flowable-errors` → census đầy đủ xanh. Commit (đây là commit lift+hoà-giải duy nhất).

### Task 4: Docs + gate + PR

- `egret-data/docs` (Platform): ghi `retry:` trên block `AllowFailure` giờ được hỗ trợ — child fail kế thừa, block không retry, block ánh xạ child-còn-fail sang `WARNING`; cập nhật `flow-control.md` (mục AllowFailure) và `error-handling.md` (đoạn retry-on-flowable) bỏ câu "AllowFailure block retry bị từ chối".
- Gate: `cargo fmt --all --check`, clippy, scoped suite đầy đủ (+ mọi binary grep Task-0 nêu).
- Mở PR code (`Closes` issue 9b nếu có) + PR docs. Merge khi CI xanh (KHÔNG `--admin`). Hoàn tất gói roadmap #9.

## Checklist tự rà (chạy khi formalize — ĐÃ XONG)

- D1 (parity) → T1/T2; D2 (oracle vendored) → T0; D3 (không mig/API) → Ràng buộc; D4 (planner flowable) → T1; D5 (gỡ cuối, hoà giải) → T3.
- Coupling corpus (`valids/retry-with-flowable-errors`) xử lý bằng luật gỡ-cuối của 9a — T1/T2 phát triển sau từ chối còn sống, T3 gỡ và hoà giải trong một commit.
- Không test nào assert terminal chưa đặt nền từ golden/trace đã vendored (T0 load-bearing).
- Grep Task-0 (không phải liệt kê reviewer) là blast radius thẩm quyền cho T3.
