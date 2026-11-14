# Workflow package #7a "Restart, task-run state và execution status" — kế hoạch triển khai

> Bản tiếng Việt để tham khảo. Bản gốc là [tiếng Anh](../../en/plans/2026-11-13-workflow-execution-ops-restart.md). Mọi giá trị chính xác, tên hàm/lớp, đường dẫn file và chi tiết test nằm ở bản tiếng Anh; khi hai bản khác nhau thì bản tiếng Anh đúng.

> **Dành cho agentic worker:** SUB-SKILL BẮT BUỘC: dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để thực hiện plan này từng task một. Các bước dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Mục tiêu:** một người có `workflow:manage` có thể restart tại chỗ một execution `FAILED`/`PAUSED`, đặt state của một task run, và đổi status của một execution đã xong, từ API và từ trang execution, với ngữ nghĩa của Kestra (spec D1–D14).

**Kiến trúc:** hai hàm thuần của executor, `executor::restart::restart` và `executor::restart::mark_as`. Mỗi hàm trả về các `Command` reset run trong bộ nhớ: một run mà Kestra gỡ bỏ thì được làm rỗng, không bao giờ bị xoá. Ba điểm vào của worker áp dụng các command đó dưới lock của execution rồi `settle`, theo cách `resume_by_hand` làm. Các iteration của `Loop` và child của `Subflow` được restart tại chỗ bằng cùng một điểm vào. Ba route, một migration nới lỏng một CHECK, ba action ở frontend, và docs.

**Tech stack:** Rust (axum, sqlx, Postgres), `crates/egret-workflow`; React + client orval trong `frontend/`.

Spec: `docs/en/specs/2026-11-13-workflow-execution-ops-restart-design.md`. Q1–Q2 và D1–D14 của nó có tính ràng buộc, và các trích dẫn file:line của spec là bằng chứng của nó. Nhánh `feat/workflow-execution-ops`, một PR. Các task: T0, A1–A5 (engine), B1 (route), C1 (frontend), Z (docs, cổng, live check).

**Review không phải là một task của plan này.** Controller review mỗi task giữa các task, và chạy một review toàn nhánh sau Z. Các sửa lỗi được ghi trong spec §7 dưới dạng các đợt sửa.

## Ràng buộc chung

- Q1–Q2 và D1–D14 của spec có tính ràng buộc. Mã nguồn Kestra: `/Users/1pro-l4r8/Documents/github/data-portal/kestra` tại `96893b58`. Khi spec và Kestra khác nhau, spec thắng. Khi spec và code của egret khác nhau, dừng và báo cáo; không tự ứng biến.
- **Tìm code:** dùng CodeGraph trước, `codegraph_explore` với `projectPath: /Users/1pro-l4r8/Documents/github/data-portal/egret-execops`. Chỉ grep cho chuỗi literal. `rg` chưa được cài.
- Conventional Commits. Trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- **Không có mạng** cho subagent: không `git push`/`fetch`/`pull`, không `gh`, không `npm ci`. Controller chạy `npm ci` / `npm run gen:api` khi C1 cần.
- Không bao giờ đụng cổng 5432 hoặc các container `egret-dev-*`/`egret-demo-*`. File nháp và `.superpowers/` không bao giờ vào git.
- **Môi trường build và test:**
  - `CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-execops/target`
  - DB test: `DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test`, container `egret-execops-postgres`.
  - Shared store: `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6465`, container `egret-execops-valkey`.
  - Không bao giờ chạy hai lệnh cargo cùng lúc.
  - Chạy `df -h` trước một lần build đầy đủ (còn trống 84 GiB lúc thiết kế). Dọn `target/debug/incremental` thay vì tạo target dir thứ hai.
- **Test cho mỗi task:** mỗi task chỉ chạy các test cho phần nó đụng đến. Các suite đầy đủ chạy một lần, ở Z.
- **Red trước:** test cho thấy khoảng trống tồn tại trước khi sửa.
- Mọi `std::env::var` mới phải được thêm vào allow-list `ENV_OUTSIDE_CONFIG` của `egret-config`. Chạy `cargo test -p egret-config` khi thêm một cái. Dự kiến không có cái nào.
- **Ratchet của executor không được dịch chuyển:** **301 / 25 / 25 / 0**, census `LIVE` 295 / `VALIDATE_REFUSED` 25 / `UNKNOWN_TYPE_REFUSED` 21 (spec §4). Bất kỳ thay đổi nào là một regression cần báo cáo, không phải một khoản ghi nhận.
- **Từ ngữ trong code và thông báo:** các mã từ chối hiển thị cho người dùng chính xác là
  - `nothing_to_restart`
  - `not_restartable`
  - `after_execution_running`
  - `files_pruned`
  - `state_unchanged`
  - `restart_revision_unsupported`

  Các audit action chính xác là `workflow_execution:restart`, `workflow_execution:set_task_run_state` và `workflow_execution:set_status`. Label của engine là `system.restarted` với giá trị `"true"`.

## T0. Môi trường và baseline

- [ ] Khởi động các container test. Theo các dòng `docker run` mà các gói trước dùng, có thể tìm bằng grep `egret-loopout-postgres` trong ghi chú memory hoặc lịch sử shell. Dùng image `postgres:16` trên cổng host **5465**, user/password/db `egret`/`egret`/`egret_test`, và một container Valkey trên cổng host **6465**.
- [ ] Chạy ratchet và census của executor: `cargo test -p egret-workflow --test executor -- --nocapture`. Ghi lại bộ số.
- [ ] Đọc lại số migration: `git ls-tree --name-only origin/main migrations/ | tail -1` và mọi nhánh local. `0225` phải còn trống (spec D12).
- **Chấp nhận:** ratchet 301 / 25 / 25 / 0; census 295 / 25 / 21; migration cao nhất là `0219`. Báo cáo cả ba, đúng như đo được.

## A. Engine

### A1. Các hàm reset thuần (spec D2, D3, D4, D6, D11)

**File:**
- Tạo: `crates/egret-workflow/src/executor/restart.rs` (kèm `#[cfg(test)] mod tests`).
- Sửa:
  - `crates/egret-workflow/src/executor/command.rs`: các command mới.
  - `crates/egret-workflow/src/executor/state.rs`: các nhánh `apply`.
  - `crates/egret-workflow/src/executor/mod.rs`: `pub mod restart;` và re-export.

**Interface được tạo ra:**
```rust
pub enum RestartRefusal { NothingToRestart, NotRestartable(RunState) }
/// Spec D2–D4, D6. `now` is the restart's tick.
pub fn restart(plan: &Plan, state: &ExecutionState, now: Tick)
    -> Result<RestartCommands, RestartRefusal>;
/// Spec D11 (Kestra `markAs`). `target` is one of SUCCESS/WARNING/FAILED/CANCELLED.
pub fn mark_as(plan: &Plan, state: &ExecutionState, run: RunIx, target: RunState, now: Tick)
    -> Result<RestartCommands, RestartRefusal>;
pub struct RestartCommands {
    pub commands: Vec<Command>,
    /// Runs emptied by D4/D3 (WorkingDirectory descendants): the worker renews their ids.
    pub emptied: Vec<RunIx>,
    /// Runs reset to CREATED/RUNNING: the worker clears their outputs/message/secret_outputs.
    pub reset: Vec<RunIx>,
    /// `Loop` runs set back to RUNNING whose last failing iteration the worker restarts (D8).
    pub loops: Vec<RunIx>,
    /// `Subflow` runs reset to CREATED: the worker looks up the earlier child (D9).
    pub subflows: Vec<RunIx>,
}
// command.rs
Command::EmptyRun { run: RunIx }              // as RestartLoop empties a descendant, for one run
Command::ReopenFlowable { run: RunIx, at: Tick } // push RUNNING and open a new Attempt at `at` (D6)
Command::OverwriteLastAttempt { run: RunIx, state: RunState } // mark_as: set the run's state AND its last attempt's, no new attempt
```
- **Dùng lại, không nhân đôi.** `EmptyRun` phải xoá đúng những gì `RestartLoop` xoá trên một descendant (`state.rs:998-1041`): history, attempts, renders, `not_before`, `resume_with`, `working_dir`. Tách phần thân xoá-descendant của `RestartLoop` thành một hàm private mà cả hai nhánh cùng gọi.
- **Các quy tắc (trích từ spec):**
  - **Reset set.** Mọi run có state hiện tại là `FAILED` hoặc `PAUSED`, cộng với mọi tổ tiên của chúng. Các state khác được giữ trừ khi là tổ tiên.
  - **State theo từng loại (bảng D3):**
    - một runnable leaf hoặc `Subflow` → `SetRunState{Created}`;
    - một flowable tổ tiên và `Pause` → `ReopenFlowable` (D6: một attempt mới tại `now`);
    - `WorkingDirectory` → `SetRunState{Created}`, mọi descendant `EmptyRun`;
    - `LoopUntil` → `ReopenFlowable`;
    - `Loop` → `ReopenFlowable` và được liệt kê trong `loops`.
  - **End-hook set (D4).** Mọi run mà task của nó nằm trong `errors:`/`finally:` của flow, hoặc trong `errors:`/`finally:` của bất kỳ flowable nào (cùng children), hoặc trong `afterExecution:` → `EmptyRun` và được liệt kê trong `emptied`. Một run đã rỗng thì bỏ qua. Dùng tư cách thành viên pha của chính plan: `after_execution::in_phase` và các danh sách `errors`/`finally` của flowable mà `current_list` đọc. Không khớp task id bằng chuỗi.
  - **Execution.** `SetExecutionState{Restarted}` đặt cuối cùng. `restart` từ chối `NotRestartable` trừ khi state hiện tại của execution là `FAILED` hoặc `PAUSED`, và từ chối `NothingToRestart` khi reset set rỗng.
  - **`mark_as`.** Run đích nhận `SetRunState{target}`, và state của attempt cuối cùng của nó bị ghi đè mà không có attempt mới. Việc này cần một command thứ ba, `Command::OverwriteLastAttempt { run, state }`; thêm nó cạnh các command khác. Các tổ tiên nhận `ReopenFlowable`, end-hook set bị làm rỗng, và sau đó:
    - đích `CANCELLED` → `SetExecutionState{Cancelled}`;
    - ngược lại nếu còn run nào `PAUSED` → không đặt state cho execution;
    - ngược lại → `SetExecutionState{Restarted}`.

    `mark_as` từ chối `NotRestartable` trừ khi execution đã terminal và không phải `KILLED`.
- [ ] **Test (thuần, state dựng tay như trong test của `state.rs` và `render.rs:1474-1498`).** Với mỗi trường hợp: dựng plan bằng `build_plan`, đưa nó đến hình dạng lỗi bằng `apply`, gọi hàm, `apply` các command của nó, rồi assert. Sau đó assert rằng một vòng `advance` trên kết quả làm đúng việc tiếp theo (các leaf CREATED được mở, không phải `Waiting`).
  1. Một sequence `a, b(fails), c`: chỉ `b` bị reset về CREATED. `a` không đổi. `c` không có run. History của execution kết thúc bằng `FAILED, RESTARTED`, và một `advance` nối thêm `RUNNING`.
  2. Các run `errors:` và `finally:` cấp flow bị làm rỗng và được liệt kê trong `emptied`. Sau một `advance`, walk đang ở pha Tasks, không phải Errors (đây là regression của roadmap-note-7).
  3. Một `errors:` cục bộ dưới một `Sequential` bị làm rỗng. `Sequential` được mở lại với một attempt mới có `started_at == now`.
  4. Các run `afterExecution:` bị làm rỗng.
  5. Một `Parallel` với `a` FAILED và `b` bị fail bởi rule 1: cả hai bị reset, và `Parallel` được mở lại.
  6. Một `WorkingDirectory` có child lỗi: block là CREATED, mọi descendant rỗng, và `working_dir` là `None`.
  7. Một `LoopUntil` có iteration child lỗi: `LoopUntil` được mở lại với `attempt_started_at() == Some(now)` (D6).
  8. Một execution `PAUSED` có run `Pause`: `Pause` được mở lại, history của nó vẫn chứa `PAUSED`, và một `advance` kết thúc nó `SUCCESS` (Kestra `Pause.java:269-272`).
  9. Một run `Loop` FAILED: nó được mở lại, được liệt kê trong `loops`, và không phải CREATED.
  10. Một run `Subflow` FAILED: nó là CREATED và được liệt kê trong `subflows`.
  11. Từ chối: một execution `SUCCESS` cho `NotRestartable`; một execution `FAILED` không có run FAILED/PAUSED nào cho `NothingToRestart`.
  12. `mark_as` SUCCESS trên task đầu FAILED của `a(fails), b`: `a` là SUCCESS, attempt cuối của nó là SUCCESS, số attempt không đổi, execution là RESTARTED, và một `advance` tạo `b`.
  13. `mark_as` với một nhánh error: các run error bị làm rỗng (Kestra `changeStateWithErrorBranch`).
  14. `mark_as` CANCELLED: execution là CANCELLED.
  15. `mark_as` trên một execution KILLED cho `NotRestartable`.
- [ ] Chạy: `cargo test -p egret-workflow --lib executor::restart`.
- [ ] Commit: `feat(workflow): pure restart and mark-as commands (package 7a)`.
- **Chấp nhận:** 15 test pass; `--lib` pass; ratchet không đổi (`--test executor`).

### A2. Restart tại chỗ: điểm vào của worker và migration (spec D1, D5, D7, D10, D12)

**File:**
- Tạo:
  - `migrations/0225_workflow_restart_attempt_number.sql`
  - `crates/egret-workflow/src/worker/restart.rs`
  - `crates/egret-workflow/tests/restart.rs`
  - `crates/egret-workflow/tests/migration_0225_restart_attempt.rs`
- Sửa:
  - `crates/egret-workflow/src/worker/mod.rs`: re-export.
  - `crates/egret-workflow/src/worker/claim.rs`: chỉ khi một helper phải thành `pub(crate)`.

**Interface:**
- Dùng: `restart`, `RestartCommands`, `RestartRefusal` của A1.
- Tạo ra:
```rust
pub enum Restarted { Done, Refused(RestartRefused) }
pub enum RestartRefused { NotRestartable(String /*state*/), NothingToRestart,
                          AfterExecutionRunning, FilesPruned, LoopIteration { root: Uuid } }
/// Unscoped by workspace, like every worker entry point; the route scopes first.
pub async fn restart_execution(pool: &PgPool, id: Uuid) -> egret_core::Result<Restarted>;
```
- **Migration 0225.** Drop và thêm lại `workflow_executions_metadata_belongs_to_a_replay` (0139:141-146) thành
  ```sql
  CHECK (attempt_number >= 1 AND (origin_offset_ms = 0 OR (attempt_number > 1 AND parent_execution_id IS NOT NULL AND parent_run_ix IS NULL)))
  ```
  Thêm một comment giải thích rằng restart làm tăng `attempt_number` trên mọi execution (spec D7) trong khi `origin_offset_ms` vẫn chỉ thuộc về replay.
- **Hình dạng.** Sao chép `resume_by_hand` (`claim.rs:4469-4485`): `transaction!`, `load` (lấy lock), các kiểm tra dưới lock, `before = state.clone()`, `apply` các command của A1, các side write, rồi `settle`. Lock các row run bằng `FOR UPDATE` theo thứ tự `run_ix` như `kill_loaded`. Retry khi deadlock như `kill_execution_reporting` (`DEADLOCK_RETRIES`, `worker/kill.rs`).
- **Các kiểm tra dưới lock, theo thứ tự này:**
  1. một iteration của loop (`parent_execution_id` có `loop_index`) → `LoopIteration{root}`;
  2. `files_pruned_at IS NOT NULL` → `FilesPruned`;
  3. execution đã terminal nhưng không idle (`workflow.execution_is_idle`, 0210, trả false) → `AfterExecutionRunning`;
  4. các từ chối của A1.

  Một từ chối không ghi gì.
- **Side write, trong cùng transaction, sau `apply` và trước `settle`:**
  - `UPDATE workflow.executions SET attempt_number = attempt_number + 1`;
  - thêm label `system.restarted: "true"` nếu chưa có, qua cùng dạng JSON mà `labels` đã có (đọc `labels.rs` và writer của cột `labels` trước);
  - với mỗi run `reset` và `emptied`: `outputs = NULL, message = NULL, secret_outputs = NULL, outputs_bytes = NULL`;
  - cấp cho mỗi run `emptied` một id mới theo cách `renew_restarted_ids` làm (`claim.rs:5266-5279`). Dùng lại nó; chỉ mở rộng nếu chữ ký của nó gắn chặt với `RestartLoop`.
- **`loops` và `subflows`** ở đây rỗng. A2 từ chối (kiểu `NothingToRestart`, một lỗi nội bộ tạm thời) khi một trong hai không rỗng, và A3/A4 gỡ từ chối đó. Nêu điều này trong một comment code có nhắc tên A3/A4.
- [ ] **Test** (`tests/restart.rs`, `#[sqlx::test]`, helper như trong `tests/kill_route.rs` và `tests/end_hooks.rs`: `start`, `tick`, `finish`, `runs_of`, `exec_history`). Dùng một registry làm fail một task có tên một lần rồi thành công, để một test có thể làm fail một run rồi cho nó pass sau restart.
  1. `restartSimple`: cùng execution id; cùng `task_runs.id` cho run bị reset; history `… FAILED RESTARTED RUNNING`; label có mặt; `attempt_number` 2.
  2. `restartFailedThenSuccess`: run 3 → 4 sau khi tick đến cuối; run lỗi có 2 attempt; execution SUCCESS.
  3. `errors:` cấp global bị làm rỗng và chạy lại trên một lỗi mới; `errors:` cục bộ; `finally:`; `afterExecution:`. Các run bị làm rỗng có id mới, và id cũ của chúng vẫn sở hữu các row `task_logs` cũ.
  4. `restartMultiple`: restart, fail, restart, succeed.
  5. Một `Parallel` có hai nhánh lỗi kết thúc SUCCESS sau restart.
  6. Một `WorkingDirectory` chạy lại trong một thư mục mới: pha `working_dir` của nó bắt đầu lại (bỏ qua nếu Docker không có, cùng cổng như `working_dir_claims.rs`).
  7. Một tên container được giữ không bị dùng lại: chỉ số của attempt mới là `attempts.len()`, lớn hơn cái cũ. Assert điều này trên attempt của `TaskInput` được claim, không phải trên Docker.
  8. Các row outbox của flow-trigger: mỗi cái cho RESTARTED, RUNNING và state terminal mới.
  9. Các từ chối không ghi gì: một execution SUCCESS, một KILLED, một có `files_pruned_at` được đặt, một có run `afterExecution:` đang sống, và một iteration của loop. Assert `history`, `attempt_number` và `labels` của row không đổi.
  10. Một retry: một run có `retry: maxAttempts: 2` đã dùng hết fail lại sau restart mà không retry (hệ quả của D5).
  11. `restartOrReplayLoopUntil` (Kestra `RestartCaseTest:463-505`): một `LoopUntil` có child lỗi chờ lại từ lúc restart; `maxDuration` của nó được đo từ tick của restart, không phải lần start đầu (D6).
  12. Các side effect D7: một SLA `EXECUTION_ASSERTION` được đánh giá lại khi execution được restart đang chạy; flow `outputs:` được đánh giá lại ở state terminal mới (giá trị cũ vẫn hiển thị cho đến lúc đó).
- [ ] `tests/migration_0225_restart_attempt.rs`:
  - một row top-level giờ có thể giữ `attempt_number = 3, origin_offset_ms = 0`;
  - `origin_offset_ms > 0` mà không có dạng replay vẫn bị từ chối.
- [ ] Commit: `feat(workflow): restart an execution in place (package 7a)`.
- **Chấp nhận:** các test mới pass; `--test kill_route --test end_hooks --test worker` pass; ratchet không đổi.

### A3. `Loop`: restart iteration lỗi cuối cùng tại chỗ (spec D8)

**File:** sửa `crates/egret-workflow/src/worker/restart.rs` và `crates/egret-workflow/src/worker/spawn.rs` (chỉ khi bộ đếm cần); mở rộng `tests/restart.rs`.

- Với mỗi run trong `loops`: tìm các row iteration `parent_execution_id = id AND parent_run_ix = run`, chọn `loop_index` cao nhất có state `FAILED` hoặc `PAUSED`, và restart nó bằng cùng hàm bên trong. Tách thân của `restart_execution` thành `restart_locked(conn, id, as_iteration: bool)`, hàm này bỏ qua từ chối loop-iteration cho lời gọi bên trong. Thứ tự lock là parent rồi iteration.
- Nếu không iteration nào đủ điều kiện: nối thêm `SetRunState{Failed}` vào run loop và ghi dòng task log `No restartable loop sub-execution found` trên run đó (tìm writer hiện có cho các dòng log của executor; không thêm cái mới).
- **Bộ đếm và danh sách outputs (#6).** Trước hết, red: một test mà một iteration kết thúc lần thứ hai bị đếm đôi trong `terminatedIterations`, hoặc xuất hiện hai lần trong `outputs.loop.outputs`. Rồi sửa `on_execution_terminal` (`spawn.rs:539-728`) để một iteration kết thúc lại thay thế entry của index nó và được đếm một lần. Bộ đếm được suy ra theo từng index, không bao giờ tăng một cách mù quáng.
- [ ] **Test:**
  1. `restartLoop` (Kestra `RestartCaseTest:224-311`): ba lần restart, mỗi lần tiếp tục từ iteration lỗi. Run id của `Loop` giống nhau mỗi lần; tồn tại đúng N row iteration (không spawn lại).
  2. Bộ đếm và danh sách outputs sau khi một iteration được restart thành công.
  3. Không iteration nào restart được → Loop là FAILED với dòng log.
  4. Các iteration đã SUCCESS không bị đụng đến.
- [ ] Commit: `feat(workflow): restart a Loop from its failing iteration (package 7a)`.
- **Chấp nhận:** các test pass; `--test restart --test worker` pass; ratchet không đổi.

### A4. `Subflow`: restart child lỗi tại chỗ (spec D9, Q1)

**File:** sửa `crates/egret-workflow/src/worker/subflow.rs` (nơi `SpawnSubflow` của walk được thực hiện) và `crates/egret-workflow/src/worker/restart.rs`; mở rộng `tests/restart.rs`.

- Khi một run `Subflow` được mở trong một execution có label `system.restarted`, tra cứu các child trước đó bằng `trigger_execution_id = parent AND trigger_run_ix = run` với cùng `value` và iteration.
  - **Khớp đúng một:**
    - child `FAILED` hoặc `PAUSED` → `restart_locked(child)`; liên kết nó làm child sống của run, để run chờ nó như chờ một spawn mới;
    - ngược lại → run kết thúc `SUCCESS`.
  - **Không khớp hoặc khớp nhiều** → spawn như hiện nay.
- Thứ tự lock là parent rồi child (`worker/kill.rs:82-104`); dùng retry deadlock của kill.
- `on_child_terminal` (`subflow.rs:661+`) phải chấp nhận báo cáo của child được restart, vì run parent lại không còn terminal. Thêm một test red trước.
- [ ] **Test:**
  1. `restartSubflow`: child lỗi được restart, không bao giờ bị thay; đúng một row child; parent SUCCESS.
  2. `restartSubflowWithLoop` (Kestra `:359-395`): ba lần restart và đúng ba child.
  3. Một child SUCCESS trong khi parent FAILED ở chỗ khác: run `Subflow` kết thúc SUCCESS, và không child nào được restart hay spawn.
  4. Hai child khớp: một child mới được spawn.
  5. Một deadlock giữa một restart và settle của chính child được retry và cả hai kết thúc. Mô hình hoá theo `a_worker_that_loses_a_deadlock_to_a_kill_still_finishes_it`.
- [ ] Commit: `feat(workflow): restart a failed Subflow child in place (package 7a)`.
- **Chấp nhận:** các test pass; `--test restart --test kill_route --test worker` pass; ratchet không đổi.

### A5. Task-run state và execution status (spec D11)

**File:** sửa `crates/egret-workflow/src/worker/restart.rs`; tạo `crates/egret-workflow/tests/mark_as.rs`.

**Interface được tạo ra:**
```rust
pub enum StateChangeRefused { NotChangeable(String), StateUnchanged, LoopIteration { root: Uuid },
                              NoSuchTaskRun, AfterExecutionRunning, FilesPruned }
pub async fn set_task_run_state(pool: &PgPool, id: Uuid, task_run_id: Uuid, target: RunState)
    -> egret_core::Result<Result<(), StateChangeRefused>>;
pub async fn set_execution_status(pool: &PgPool, id: Uuid, target: RunState)
    -> egret_core::Result<Result<(), StateChangeRefused>>;
```
- `set_task_run_state`:
  - phân giải `task_run_id` thành một `RunIx` chỉ trong execution này; một id không biết hoặc thuộc execution khác cho `NoSuchTaskRun`;
  - state đích bằng state hiện tại của run cho `StateUnchanged`;
  - rồi `mark_as` của A1, và các side write của A2: attempt+1, label, outputs bị xoá trên các run `reset`/`emptied` (**không** trên run được mark: Kestra giữ chúng), và id được cấp mới;
  - `loops`/`subflows` được xử lý như ở A3/A4.
- `set_execution_status`:
  - terminal và không phải KILLED, nếu không thì `NotChangeable`;
  - cùng state cho `StateUnchanged`;
  - nếu không thì `SetExecutionState{target}`, rồi `settle`. Không run nào bị đụng đến, và không đổi label hay attempt (Kestra `ES:1385-1392`).
- Cả hai hàm chỉ nhận bốn state đích. Kiểu được route (B1) kiểm tra; assert ở đây nữa, bằng `debug_assert!` cộng một `Err`.
- [ ] **Test:**
  1. `ChangeStateTestCase:28-40`: task đầu FAILED được mark SUCCESS → execution SUCCESS với 2 run.
  2. `changeStateWithErrorBranch`: các run error bị làm rỗng; `attempt_number` 2.
  3. Mark CANCELLED → execution CANCELLED, không chạy gì.
  4. Status SUCCESS → WARNING: history +1; các run không đổi; một row outbox.
  5. Các từ chối không ghi gì: KILLED, RUNNING, cùng state, một task-run id ngoài execution, một iteration của loop.
  6. Status của child Subflow được đổi trong khi run `Subflow` của parent đã terminal: parent không đổi (spec D11; FIXME của Kestra).
- [ ] Commit: `feat(workflow): set a task run's state and an execution's status (package 7a)`.
- **Chấp nhận:** các test pass; ratchet không đổi.

## B1. Route, OpenAPI và audit (spec D13)

**File:** sửa `crates/egret-workflow/src/routes.rs` (các handler cạnh `kill_execution`, các dòng router cạnh `:4062-4070`, danh sách route trong doc của module ở `:1-37`, đăng ký OpenAPI ở bất cứ đâu `kill_execution` được đăng ký); tạo `crates/egret-workflow/tests/restart_routes.rs`.

- **Route:**
  - `POST /ws/{workspace}/workflows/executions/{id}/restart`, query `revision` (giá trị bất kỳ) → 400 với mã `restart_revision_unsupported` và một thông báo nêu tên package 7b;
  - `POST /ws/{workspace}/workflows/executions/{id}/task-runs/{task_run_id}/state`, body `{"state": "SUCCESS"|"WARNING"|"FAILED"|"CANCELLED"}`;
  - `POST /ws/{workspace}/workflows/executions/{id}/status`, cùng body.

  State khác, hoặc một field không biết, cho 400 (`#[serde(deny_unknown_fields)]`).
- **Mỗi handler:** `require(WF_MANAGE)`, rồi `executions.get` theo phạm vi (404 cho id ngoài workspace), rồi kiểm tra state rẻ, rồi gọi worker. Các kết quả ánh xạ thành:
  - 409 `not_restartable` / `nothing_to_restart` / `after_execution_running` / `files_pruned` / `state_unchanged`;
  - 400 cho một iteration của loop, nêu tên root của nó (như thông báo của replay);
  - 404 cho `NoSuchTaskRun`.

  Rồi audit event với `{from_state}` (cộng `task_run_id` và `to_state` khi áp dụng), và 200 với `ExecutionDto::new(detail, true, st.trigger_flags.worker_enabled)`.
- **409 có mã:** dùng cùng dạng `ErrorBody{error, code}` như `KillError`. Định nghĩa một enum `ExecOpError` cho cả ba route thay vì ba bản sao.
- Doc `utoipa::path` cho mọi status, viết như của route kill (body là gì, mỗi 409 nghĩa là gì).
- [ ] **Test:**
  - các đường 200;
  - 403 khi chỉ có `workflow:read`;
  - 404 cho workspace khác và cho task run ngoài workspace;
  - từng 409 có mã;
  - 400 cho `revision`, cho `RUNNING` làm đích, và cho một iteration của loop;
  - một bản ghi audit cho mỗi action (`common::audit_mutations`);
  - cổng entitlement (`the_workflows_entitlement_gates_these_routes` tự động phủ các route mới; kiểm tra rằng nó phủ).
- [ ] Chạy `python3 scripts/handler_surface.py --check` (trần 290) nếu nó tồn tại trên nhánh này, và giữ dưới trần.
- [ ] Commit: `feat(workflow): restart, task-run state and status routes (package 7a)`.
- **Chấp nhận:** `--test restart_routes --test executions_routes --test routes` pass; `cargo clippy -p egret-workflow --all-targets -- -D warnings` sạch.

## C1. Frontend (spec D14)

**File:**
- Sửa:
  - `frontend/src/components/workflows/executions/actions.ts` (+ test của nó);
  - `frontend/src/routes/workflows/ExecutionDetail.tsx` (`ExecutionActions` `:358-524`, và các hàng task run);
  - `frontend/src/api/generated/*`, được controller sinh lại bằng `npm run gen:api`.
- Tạo: một `ChangeStateDialog.tsx` cạnh `actions.ts` nếu dialog vượt quá khoảng 80 dòng.

Đọc `docs/en/frontend-rules.md` (RULEBOOK frontend) trước; mọi quy tắc đều áp dụng.

- **Predicate của action:**
  - `showsRestart`: state `FAILED` hoặc `PAUSED`; `canRestart`: không phải một iteration của loop;
  - `showsChangeStatus` và `showsChangeTaskRunState`: terminal và không phải `KILLED`.
- **Restart:** `ConfirmAction` → `mutateAsync` → `invalidateQueries(workflowsKeys.execution(id))` và `executions(ws)` → toast. Polling bắt đầu lại vì state lại đang sống (`polling.ts`).
- **Change status:** một dialog với một select gồm bốn state trừ state hiện tại.
- **Change state:** một mục menu cho mỗi hàng task run. Nó sửa attempt cuối cùng của run; dialog nói rõ điều đó.
- **Các 409 có mã** ánh xạ thành câu chữ qua `ApiError.code`, như `kill_not_applied` của route kill được ánh xạ.
- [ ] **Test (vitest):**
  - các predicate cho mọi state trong bảng của `StateChip`;
  - mỗi mutation invalidate cả hai key;
  - một 409 có mã hiển thị câu chữ của nó;
  - select không bao giờ đưa ra state hiện tại.
- [ ] Chạy `npm run typecheck`, `npm run lint` và các file vitest bị đụng đến. Controller cung cấp `node_modules`; mượn, không bao giờ cài.
- [ ] Commit: `feat(workflows): restart, change status and change task-run state on the execution page (package 7a)`.
- **Chấp nhận:** typecheck, lint và vitest pass.

## Z. Docs, ghi sổ, cổng và live check

- [ ] **Spec §7 "As built":** các con số đo được, số migration, mọi sai khác so với plan này kèm lý do.
- [ ] **Roadmap** `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`:
  - một ghi chú có ngày dưới dòng #7 về việc tách gói và 7a xong;
  - các dòng bảng của Appendix §5 cho restart, change-status và state được đánh dấu supported, kèm bằng chứng route;
  - note 7 được đánh dấu đã đóng.
- [ ] **Tiếng Việt** các bản sao `docs/vi/specs/…` và `docs/vi/plans/…` (quy ước song ngữ). Một subagent Sonnet có thể dịch.
- [ ] **`egret-data/docs`** (Platform → Workflows → executions): ba action, permission, các từ chối, và ảnh chụp màn hình mới. Controller mở PR đó.
- [ ] **Các cổng** (lần lượt từng cái):
  - `cargo fmt --check`;
  - clippy `-D warnings --all-targets` trên egret-workflow, egret-server, egret-config, egret-metastore;
  - `cargo check -p egret-bin`;
  - `cargo test -p egret-workflow` (báo cáo passed, failed, ignored; chỉ `sql_starrocks_live` được phép fail, vì thiếu engine);
  - `cargo test -p egret-config -p egret-server -p egret-metastore`;
  - typecheck, lint và vitest của frontend.
- [ ] **Live check** trên một compose project riêng (không bao giờ dùng DB của demo stack dùng chung; memory "demo stack shared state"). Trong trình duyệt, với `admin@egret.local`:
  1. Chạy một flow fail một lần, bấm Restart, và xem nó chuyển SUCCESS trên cùng execution id.
  2. Change status SUCCESS → WARNING.
  3. Mark một task FAILED là SUCCESS và xem task kế tiếp chạy.

  Chụp ảnh màn hình cho docs. Trước live check, grep binary của image để tìm một chuỗi route mới ("docker build cache ships a stale binary").
- **Chấp nhận:** mọi cổng pass; ratchet là 301 / 25 / 25 / 0; live check pass kèm ảnh chụp màn hình.
