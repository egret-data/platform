> **Bản dịch tham khảo.** Bản tiếng Anh `docs/en/plans/2026-10-26-workflow-end-hooks.md` là bản chuẩn; khi hai bản khác nhau, bản tiếng Anh thắng. Bản này đi theo bản tiếng Anh từng mục một. Mã, tên khoá, mã lỗi, đường dẫn, cờ lệnh, thông báo người dùng thấy được và các khối mã giữ nguyên tiếng Anh.

# Gói workflow #2 "End hooks" — kế hoạch triển khai

> **Dành cho agent thực thi:** SUB-SKILL BẮT BUỘC: dùng superpowers:subagent-driven-development (khuyến nghị) hoặc superpowers:executing-plans để triển khai kế hoạch này từng task một. Các bước dùng cú pháp checkbox (`- [ ]`) để theo dõi.

**Spec:** `docs/en/specs/2026-10-26-workflow-end-hooks-design.md`. Đọc trước §0 (ngữ nghĩa của Kestra), các Quyết định D1–D16 và danh sách "Tiền đề sai". Kế hoạch này không thêm quyết định nào. §12 của spec có năm câu hỏi cho controller. Kế hoạch này xây câu trả lời mặc định cho từng câu; chỗ nào một câu trả lời làm đổi một task thì task đó nói rõ.

**Mục tiêu:** egret chạy mọi danh sách mà Kestra chạy ở cuối một khối hay một execution:
- `finally:` của flow;
- `finally:` trên mọi flowable trừ `Pause` (D14 chuyển `Pause` sang #9);
- `errors:` trên `Loop`, `LoopUntil` và `WorkingDirectory` (Phần A, PR #2a);
- `afterExecution:` của flow, trong một pha sau terminal, cùng với mọi quy tắc SQL đọc "terminal" là "không còn việc đang chạy" được chuyển sang `workflow.execution_is_idle` (Phần B, PR #2b).

**Kiến trúc:**
- **Model.** Các flowable của `models::Task` được thêm `finally` (và ba loại được thêm `errors`). `children()` và `error_children()` mang chúng, nên mọi phép duyệt sẵn có đều thấy.
- **Plan.** `executor::plan::flatten_flow` giữ bố cục hiện nay làm tiền tố và nối thêm các danh sách mới (D3). Nên `task_runs.task_ix` đã lưu vẫn hợp lệ cho các execution đang chạy.
- **Quy tắc danh sách.** Một hàm, `flowable::current_list`, port `findTaskDependingFlowState` của Kestra, gồm cả `finally` (D2). Mọi flowable và lời gọi gốc của flow đều dùng nó.
- **Pha sau terminal.** `advance` được thêm `after_execution::drive` cho một execution terminal (D8). Nó không bao giờ đổi state của execution. `executor::terminated` quyết khi nào cha nghe về con (D10).
- **Quy tắc 2.** Migration 0210 định nghĩa `workflow.execution_is_idle(uuid)` (D13). Quy tắc superseded của orphan sweep, `_wd` sweep và pruner đọc nó.

**Tech stack:** Rust (serde, serde_norway, sqlx, tokio), Postgres cho worker, oracle và các test sweep, và React cùng TypeScript (vitest) cho Task B6.

**Nhánh và worktree:**
- `feat/workflow-end-hooks` trong `data-portal/egret-endhooks`, xếp chồng lên `feat/workflow-no-silent-drops` (PR nháp #756) tại `68c9f4de`.
- Phần A là PR #2a. Phần B là PR #2b, xếp chồng lên #2a.
- Một migration, `0210`, ở Phần B.

## Các sự kiện đo cho kế hoạch này (2026-10-26, tại `68c9f4de`)

Đường dẫn tương đối với `crates/egret-workflow/src/` trừ khi ghi gốc khác. Đường dẫn Kestra dùng dạng viết tắt ở phần đầu spec (`ES`, `DE`, `FU`, `ExS`, `k/`).

- **Model.**
  - Danh sách của flow: `finally_` (`models/flow.rs:81-82`, `rename = "finally"`) và `after_execution` (`:91-92`).
  - Flowable: `Sequential` (`models/task.rs:309-316`), `Parallel` (`:330-337`), `If` (`:358-374`), `Switch` (`:382-391`), `Dag` (`:404-409`), `Loop` (`:463-472`, không có `errors`), `LoopUntil` (`:488-508`, không có `errors`), `Pause` (`:598-609`), `AllowFailure` (`:621-632`, loại duy nhất có `finally`), `WorkingDirectory` (`:646-674`, không có `errors`).
  - Các phép duyệt: `children()` (`:122-160`, `finally` của AllowFailure ở `:144`), `error_children()` (`:182-204`), và các bản song sinh `_mut` (`:212`, `:235`).
  - Khoá lạ bị `serde_json::from_value` của `kind_for` bỏ (`:959-989`).
- **Plan.**
  - Struct: `Plan` (`executor/plan.rs:20-37`), `PlanTask` (`:153-322`, `errors` ở `:307`), `TaskKind::AllowFailure { tasks, finally }` (`:384-387`), `children_of`/`direct_task_children` (`:108-149`).
  - Điểm vào: `build_plan` (`:1095-1124`), `plan_stored` (`:1149-1151`), `flatten_flow` (`:1173-1183`), `flatten_list` (`:1231-1241`), `flatten_one` (`:1243`, pre-order reserve ở `:1255-1258`), ba danh sách của AllowFailure (`:1292-1300`), `pause_gap` (`:862-919`, `errors` ở `:866`).
  - `LoopUntil` không làm phẳng `errors` (`:1470-1475`).
- **Executor.**
  - `advance` (`executor/mod.rs:178`), lệnh return sớm khi terminal (`:179-189`), lời gọi gốc (`:225`), `drives_forward` (`:326-331`).
  - `current_list` (`executor/flowable/mod.rs:199-233`), `aggregate` (`:244-282`), `run_of` (`:284`), `descend` (`:488-587`).
  - Các drive của flowable: `sequential::drive` (`sequential.rs:44-62`), `walk` (`:85`), `walk_errors` (`:163`); `parallel::drive` (`parallel.rs:33-66`); `dag::drive` (`dag.rs:76`); `branch::drive_if` (`branch.rs:130`) và `drive_switch` (`:191`); `allow_failure::drive` (`allow_failure.rs:23-48`); `working_directory::drive` (`working_directory.rs:27-49`); `loop_until::drive` (`loop_until.rs:224-279`).
  - Các test AllowFailure: `allow_failure_drive_tests` (`executor/mod.rs:10033-10170`, helper `run(src, fail)` ở `:10042`).
  - Các helper corpus trong `tests` của `executor/mod.rs`: `plan_of` (`:407`), `corpus_source` (`:413`), `every_corpus_flow` (`:428`), và `every_corpus_flow_graphs_and_every_planned_task_is_one_node` (`:458`).
- **Lưu trữ các run.** `task_runs.task_ix` được ghi ở `worker/claim.rs:538` và đọc thành `TaskRun::task` ở `:4393`. Mỗi lần nạp lập plan lại bằng `plan_stored(&source)` (`:4344-4349`). Các lần start lấy `flows.source` của dòng draft (`routes.rs:2621-2625`). Không gì đọc `"plan"` của snapshot lúc chạy (`publish.rs:347-351`).
- **Các lần lặp Loop.** `sub_flow_source` (`worker/spawn.rs:823-883`; xoá `errors`/`finally_`/`after_execution` ở `:836-839`; serialize lại ở `:878`); `find_task` (`:891-902`).
- **Worker.**
  - Claim và settle: SQL claim (`claim.rs:519-560`, không lọc theo state execution), due retries (`:799-807`), `notifies_parent` (`:2930`), `terminal:` của đuôi settle (`:3487-3497`), outbox (`:3608-3672`), `sla_terminal` (`:4652-4691`).
  - Cổng `render_updates` (`worker/render.rs:1504`).
  - Câu trả lời terminal của route kill (`worker/kill.rs:186`); cascade kill (`:358`).
- **Các sweep.** Truy vấn superseded của `run_liveness` (`worker/commands.rs:2591-2597`), `sweep_decision` (`:2672`, doc tiền đề `:2669-2671`), `pod_sweep_deletes` (`worker/k8s/runner.rs:1592`), phép chọn của `_wd` sweep (`storage/wd_sweep.rs:163-169`), pruner (`storage/prune.rs:10-30`, `terminal()` `:375-381`).
- **Bộ duyệt khoá.** `spec/keys.rs`: `ROOT_LISTS` (`:432`), `child_lists` (`:507-522`), `silent` (`:561-569`), phép khẳng định `lift_of(…Sequential, "finally") == None` (`:1280-1286`), `TWO` (`:1351`), `finally_and_the_errors_two_takes_draw_nothing` (`:1636-1650`).
- **Đồ thị.** Các danh sách của flow được vẽ ở `graph/mod.rs:489-553`. `finally` của flowable chỉ được vẽ cho `AllowFailure` (`:831-841`).
- **Frontend.** `executionFinished` (`frontend/src/components/workflows/executions/polling.ts:43-45`), `ExecutionTopologyTab.tsx:67-79`, `ExecutionGanttTab.tsx:91`, `timing.ts:50`, `actions.ts:53,74`.
- **Oracle.**
  - Ratchet 301 golden / 72 mismatching / 72 baselined / 0 broken. `(301, 229, 72)` được khẳng định ở `tests/executor.rs:813`.
  - Census: `LIVE` 295, `VALIDATE_REFUSED` 25, `UNKNOWN_TYPE_REFUSED` 21.
  - Các hằng sẽ dịch: `EX:6999` (finally 14), `EX:7044` (afterExecution 10), `MISMATCH_ON_RUNS` 55 (`EX:4402`), `MISMATCH_ON_ORDER` 27 (`EX:4426`), `MISMATCH_ON_EXECUTION` 32 (`EX:4377`), `SPAWNED_ENTRIES_DIFFERING_ONLY_INSIDE_SPAWNED` 4 (`EX:4826`), `RESTRICTION_REMOVES*` (`EX:3098-3150`), tên kill (`EX:7527`).
  - Các dòng baseline của 21 mục nằm ở spec §7.1.
- **Corpus** (quét Ruby cả 351 tệp, 2026-10-26):
  - `finally` của flowable: 7 flow (Sequential 2, Parallel 1, Dag 1, Loop 2, Pause 1);
  - `errors` trên Loop/LoopUntil/WorkingDirectory: 6 (`loop-switch-failed`, `all-flowable`, `finally-loop`, `finally-loop-parallel`, `working-directory`);
  - bên trong `afterExecution:` chỉ có `OutputValues`, `Log` và `If`; không có `retry:` hay `timeout:` bên trong `finally:`/`afterExecution:` nào; không có `Pause`/`Exit`/`Subflow`/`Loop` trong `afterExecution:`.
- **Migration:** số cao nhất là `migrations/0209_username_ledger.sql`; 0210 còn trống trên nhánh này.

## Ràng buộc chung

- **Chỉ dùng stack riêng.** Mọi lệnh cargo chạy với đúng:

  ```bash
  export DATABASE_URL=postgres://egret:egret@localhost:5458/egret_test; export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6458; export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-endhooks/target; unset EGRET_ALLOW_DB_SKIP EGRET_ALLOW_STORE_SKIP
  ```

  Các container `egret-endhooks-postgres` và `egret-endhooks-valkey` đã chạy sẵn.
  - Không bao giờ dùng cổng 5432, và không bao giờ dùng container `egret-dev-*` hay `egret-demo-*`.
  - Không bao giờ đặt `EGRET_ALLOW_*_SKIP` nào: một test cần DB mà return sớm là chưa khẳng định gì.
- **Một lệnh cargo mỗi lúc.** Không bao giờ `pkill` cargo. `rg` chưa cài: dùng `grep -rn`. `cp` được alias thành `-i` ở đây: trong script dùng `command cp -f`.
- **Ratchet oracle giữ xanh sau mỗi task:** `cargo test -p egret-workflow --test executor -- --nocapture`.
  - Ghi trong báo cáo task, trước và sau: golden, mismatching, baselined, broken, `LIVE`, `VALIDATE_REFUSED`, `UNKNOWN_TYPE_REFUSED`. Baseline trên nhánh này là 301/72/72/0, LIVE 295, VALIDATE_REFUSED 25, UNKNOWN_TYPE_REFUSED 21.
  - Một hằng chỉ đổi ở task làm nó dịch, kèm comment có ngày nêu các mục.
  - `oracle-exec/golden.json` không bao giờ bị sửa.
  - Một mục baselined bắt đầu khớp làm ratchet fail với "banked". Dòng của nó khi đó bị gỡ khỏi `oracle-exec/baseline.txt` trong cùng task, không bao giờ sớm hơn.
- **Cổng full-suite BẮT BUỘC ở cuối mỗi task**, sau test riêng của task và ratchet:

  ```bash
  cargo test -p egret-workflow --no-fail-fast > /private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/3525fc70-d007-4b4e-b99e-ef27f3a3a1f8/scratchpad/endhooks-<task>.txt 2>&1
  grep -n "FAILED\|panicked\|failed," /private/tmp/claude-501/-Users-1pro-l4r8-Documents-github-data-portal/3525fc70-d007-4b4e-b99e-ef27f3a3a1f8/scratchpad/endhooks-<task>.txt
  ```

  - Chỉ 7 test `sql_starrocks_live` được phép fail: chúng cần một engine StarRocks.
  - `runtime::a_cancelled_token_stops_both_tickers` là một flake do thời gian khi tải. Nếu nó fail, chạy lại riêng và ghi rằng nó pass.
  - Mọi lỗi khác chặn commit. Các test con theo từng task của các gói trước đã bỏ sót các lời từ chối sai mà chỉ full suite bắt được.
- **Kỷ luật mutation.** Với mỗi quy tắc mới, phá nó một lần bằng tay, thấy một test có tên chuyển đỏ, và khôi phục. Báo cáo task ghi mutation, lệnh và dòng đỏ. Một mutation mà mọi thứ vẫn xanh nghĩa là thiếu một test: thêm nó.
- **Thông báo giữ nguyên chữ** như spec cho. Mỗi thông báo mới nói trong doc comment của nó là của Kestra (kèm file:line) hay của riêng egret.
- **Ngữ nghĩa Kestra lấy từ §0 của spec.** Nếu code nói khác, dừng và báo cáo. Không bao giờ "sửa" spec trong code.
- **Không lệnh mạng trong các task** (không `git push/fetch/pull`, không `gh`, không `npm install` từ mạng). Controller push.
- **Commit** theo Conventional Commits và kết thúc bằng dòng `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`. Không bao giờ commit `.superpowers/` hay gì dưới scratchpad.
- **Số dòng lấy từ `68c9f4de`.** Grep lại mọi neo trước khi sửa.

## Bản đồ tệp

| Tệp | Trách nhiệm | Task |
|---|---|---|
| `src/executor/plan_layout_pre_end_hooks.json` (mới, dữ liệu test) | bố cục plan hiện nay cho mỗi flow corpus | A1 |
| `src/models/task.rs` | `finally` trên 9 flowable, `errors` trên 3; `children()`/`error_children()` | A2 |
| `src/executor/plan.rs` | `Plan::finally`/`after_execution`, `PlanTask::finally`, làm phẳng hoãn, `pause_gap` từ chối `finally` | A2, A3, B1 |
| `src/graph/mod.rs`, `src/graph/tests.rs` | `finally` của flowable được vẽ cho mọi loại | A3 |
| `src/executor/flowable/{mod,sequential,parallel,dag,branch,allow_failure}.rs` | `current_list` với `Finally`; các bên gọi | A4 |
| `src/executor/mod.rs` | lời gọi gốc truyền `plan.finally`; test; nhánh `after_execution`; `terminated`, `drives_after` | A4, B1 |
| `src/executor/flowable/{loop_until,working_directory}.rs` | các danh sách theo từng lần lặp và sau khi kết thúc | A5 |
| `src/worker/spawn.rs` | `sub_flow_source` giữ các danh sách của `Loop`; `find_task` duyệt `after_execution` | A6, B1 |
| `src/spec/keys.rs`, `src/spec/schema/property-treatment.json` | bộ duyệt đi xuống; các dòng sinh lại | A7 |
| `oracle-exec/baseline.txt`, `tests/executor.rs`, `oracle-exec/README.md`, `oracle-exec/handler_surface.py`, `oracle-exec/handler-surface.txt` | các bank và hằng (mỗi bank trong task gây ra nó) | A4, A6, A8, B3, B7 |
| `src/executor/after_execution.rs` (mới) | pha sau terminal | B1 |
| `src/spec/validate.rs`, `src/spec/keys.rs` | lời từ chối D12, cảnh báo D11 | B2 |
| `src/worker/claim.rs`, `src/worker/render.rs` | báo cha ở `terminated`; cổng render | B3 |
| `tests/end_hooks.rs` (mới, DB) | hành vi của pha ở mức worker | B3, B4, B5 |
| `migrations/0210_workflow_execution_is_idle.sql` (mới) | vị từ | B5 |
| `src/worker/commands.rs`, `src/storage/wd_sweep.rs`, `src/storage/prune.rs` | đọc `execution_is_idle` | B5 |
| `frontend/src/components/workflows/executionState.ts`, `executions/polling.ts`, `executions/topology/ExecutionTopologyTab.tsx`, `executions/gantt/ExecutionGanttTab.tsx`, `executions/timing.ts` (+ test) | "finished" = terminal và không run nào còn sống | B6 |
| docs (EN) | ghi chú như-đã-xây của spec, ghi chú roadmap, §10 của spec #1 được đóng, ghi chú spec nợ, §9 của programme, README schema, README oracle | A8, B7 |
| docs (VI), docs Platform của `egret-data` | bản dịch | B8 |

---

## Phần A — PR #2a: `finally:` và `errors:` bên trong một execution đang sống

### Task A1: đóng băng bố cục plan hiện nay (công cụ của D3)

**Tệp:**
- Tạo: `src/executor/plan_layout_pre_end_hooks.json`.
- Sửa: `src/executor/mod.rs` (`tests`): bộ sinh và test tiền tố.

**Interfaces:** không có trong production. Fixture là một object JSON `{ "<corpus id>": [[ix, "<task id>", "<kind tag>", [direct children…]], …] }` cho mọi flow corpus mà `plan_stored` lập plan được. `<kind tag>` là tên biến thể `TaskKind`. Các con lấy từ `Plan::children_of(ix)` như hiện nay.

- [ ] **Bước 1: viết bộ sinh và test, trước mọi thay đổi production.**

```rust
/// One flow's layout as D3 freezes it: index, id, kind, and the children
/// `Plan::children_of` returned before package #2.
fn layout(plan: &Plan) -> Vec<(usize, String, String, Vec<usize>)> {
    (0..plan.tasks.len())
        .map(|ix| {
            let kind = format!("{:?}", plan.tasks[ix].kind);
            let tag = kind.split([' ', '{', '(']).next().unwrap_or("").to_string();
            (ix, plan.tasks[ix].id.clone(), tag, plan.children_of(ix))
        })
        .collect()
}

/// **D3: a plan built after package #2 starts with the layout every stored
/// `task_runs.task_ix` was written against** (`worker/claim.rs:538`, read
/// back at `:4393`). A list #2 adds may only be APPENDED: an execution that
/// was running at deploy time is re-planned from its own source at its next
/// claim, and a shifted index would hand its runs another task's spec.
/// `EGRET_WRITE_PLAN_LAYOUT=1` rewrites the fixture; that is allowed only in
/// Task A1, at `68c9f4de`, before any planner change.
#[test]
fn the_plan_before_end_hooks_is_a_prefix_of_every_plan_after() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/executor/plan_layout_pre_end_hooks.json");
    let mut now = serde_json::Map::new();
    for (id, source) in every_corpus_flow() {
        if let Ok(plan) = plan::plan_stored(&source) {
            now.insert(id, serde_json::to_value(layout(&plan)).unwrap());
        }
    }
    if std::env::var("EGRET_WRITE_PLAN_LAYOUT").as_deref() == Ok("1") {
        std::fs::write(path, serde_json::to_string_pretty(&now).unwrap()).unwrap();
    }
    let frozen: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert!(frozen.len() >= 300, "the fixture covers the corpus ({})", frozen.len());
    let mut moved = Vec::new();
    for (id, old) in &frozen {
        let old: Vec<(usize, String, String, Vec<usize>)> = serde_json::from_value(old.clone()).unwrap();
        let Some(new) = now.get(id) else {
            moved.push(format!("{id}: planned before #2, not now"));
            continue;
        };
        let new: Vec<(usize, String, String, Vec<usize>)> = serde_json::from_value(new.clone()).unwrap();
        for o in &old {
            // A node keeps its index, id and kind, and every old child is still
            // its child (a new `finally` or a loop's new `errors` only adds some).
            match new.get(o.0) {
                Some(n) if n.1 == o.1 && n.2 == o.2 && o.3.iter().all(|c| n.3.contains(c)) => {}
                other => moved.push(format!("{id}: ix {} was {o:?}, is {other:?}", o.0)),
            }
        }
    }
    assert!(moved.is_empty(), "{moved:#?}");
}
```

  Tag `AllowFailure` đổi ở A3 (`AllowFailure { tasks }`), nhưng `format!("{:?}")` tới khoảng trắng hay ngoặc nhọn đầu tiên vẫn là `AllowFailure` cả hai lần.
- [ ] **Bước 2: sinh.** `EGRET_WRITE_PLAN_LAYOUT=1 cargo test -p egret-workflow --lib -- executor::tests::the_plan_before_end_hooks`. Rồi chạy lại không có biến: PASS. Ghi số flow trong fixture.
- [ ] **Bước 3: mutation.** Hoán đổi id của hai mục của một flow trong fixture bằng tay, thấy test đỏ với id của flow đó, và khôi phục tệp bằng `git checkout`.
- [ ] **Bước 4: ratchet và cổng full-suite.** Không số đếm nào dịch.
- [ ] **Bước 5: commit** `test(workflow): freeze the plan layout every stored task_ix was written against`.

---

### Task A2: model đọc `finally:` và `errors:` của các vòng lặp; `finally:` của `Pause` bị từ chối (D14)

**Tệp:**
- Sửa: `src/models/task.rs`: các trường, `children()`, `error_children()` và các bản song sinh `_mut`.
- Sửa: `src/executor/plan.rs` `pause_gap`: từ chối `finally`.
- Sửa: các test của `src/spec/validate.rs` (bảng fixture REFUSED, nếu nó liệt kê các ca `pause_gap`).

**Interfaces:**

```rust
// On Sequential, Parallel, If, Switch, Dag, Loop, LoopUntil, WorkingDirectory, Pause:
/// Kestra `AbstractBranch._finally` (`k/plugin/core/flow/AbstractBranch.java:30-35`),
/// `If.java:114`, `Switch.java:115`, `Dag.java:109`, `Pause.java:199`.
#[serde(rename = "finally", default, skip_serializing_if = "Vec::is_empty")]
pub finally: Vec<Task>,
// On Loop, LoopUntil, WorkingDirectory (AbstractBranch.errors):
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub errors: Vec<Task>,
```

`children()` trả, với mỗi loại, các con control-flow của nó rồi `finally`, theo thứ tự mà `AllowFailure` dùng hiện nay. `error_children()` trả `errors` cho cả `Loop`, `LoopUntil` và `WorkingDirectory`.

Thông báo của `pause_gap`, của riêng egret, cùng dạng với dòng `errors`: ``task '<id>' is a pause with 'finally', which egret does not run yet: … Roadmap package #9 lifts it``. Chép đúng chữ của dòng `errors` và chỉ đổi tên thuộc tính.

- [ ] **Bước 1: test fail trước** (trong tests của `models/task.rs`):

```rust
#[test]
fn every_flowable_reads_finally_and_lists_it_last_among_its_children() {
    for (ty, body) in [
        ("io.kestra.plugin.core.flow.Sequential", "tasks: [{id: a, type: core.log, message: m}]"),
        ("io.kestra.plugin.core.flow.Parallel", "tasks: [{id: a, type: core.log, message: m}]"),
        ("io.kestra.plugin.core.flow.If", "condition: 'true'\nthen: [{id: a, type: core.log, message: m}]"),
        ("io.kestra.plugin.core.flow.Switch", "value: x\ncases: {x: [{id: a, type: core.log, message: m}]}"),
        ("io.kestra.plugin.core.flow.Dag", "tasks: [{task: {id: a, type: core.log, message: m}}]"),
        ("io.kestra.plugin.core.flow.Loop", "values: [1]\ntasks: [{id: a, type: core.log, message: m}]"),
        ("io.kestra.plugin.core.flow.LoopUntil", "condition: 'true'\ntasks: [{id: a, type: core.log, message: m}]"),
        ("io.kestra.plugin.core.flow.WorkingDirectory", "tasks: [{id: a, type: io.kestra.plugin.scripts.shell.Commands, commands: [x]}]"),
    ] {
        let src = format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: b\n    type: {ty}\n    {}\n    finally: [{{id: z, type: core.log, message: m}}]\n",
            body.replace('\n', "\n    ")
        );
        let flow = crate::spec::parse(&src).unwrap_or_else(|e| panic!("{ty}: {e:?}"));
        let ids: Vec<&str> = flow.tasks[0].children().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids.last(), Some(&"z"), "{ty}: {ids:?}");
    }
}

#[test]
fn loop_loop_until_and_working_directory_read_errors() {
    for (ty, body) in [
        ("io.kestra.plugin.core.flow.Loop", "values: [1]"),
        ("io.kestra.plugin.core.flow.LoopUntil", "condition: 'true'"),
        ("io.kestra.plugin.core.flow.WorkingDirectory", ""),
    ] {
        let src = format!(
            "id: f\nnamespace: a.b\ntasks:\n  - id: b\n    type: {ty}\n    {body}\n    tasks: [{{id: a, type: core.log, message: m}}]\n    errors: [{{id: e, type: core.log, message: m}}]\n"
        );
        let flow = crate::spec::parse(&src).unwrap();
        let ids: Vec<&str> = flow.tasks[0].error_children().iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["e"], "{ty}");
    }
}
```

  Trong tests của `executor/plan.rs`:

```rust
#[test]
fn a_pause_finally_is_refused_and_names_package_nine() {
    let src = "id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    pauseDuration: PT1S\n    finally: [{id: z, type: core.log, message: m}]\n";
    let e = build_plan(src).unwrap_err().message;
    assert!(e.starts_with("unsupported_pause_property"), "{e}");
    assert!(e.contains("'finally'") && e.contains("Roadmap package #9 lifts it"), "{e}");
}
```

- [ ] **Bước 2: chạy, kỳ vọng FAIL:** `cargo test -p egret-workflow --lib -- models::task executor::plan::tests::a_pause_finally`.
- [ ] **Bước 3: triển khai.** Bộ làm phẳng chưa đổi: `flatten_one` đọc trường theo tên, nên các trường mới bị bỏ qua tới A3. Chạy test tiền tố A1: nó vẫn xanh, vì chưa có gì được lập plan.
- [ ] **Bước 4: census id trùng.** `children()` nay chạm các danh sách mới, nên `duplicate_task_id` có thể mới bắn. Chạy `cargo test -p egret-workflow --lib -- spec` và ratchet, ghi `LIVE`/`VALIDATE_REFUSED`. Kỳ vọng: không đổi. Một dịch chuyển là một phát hiện: nêu tên flow và dừng.
- [ ] **Bước 5: mutation.** Bỏ `finally` khỏi nhánh `Sequential` của `children()`: test đầu chuyển đỏ ở `Sequential`. Khôi phục. Bỏ nhánh `finally` của `pause_gap`: test Pause chuyển đỏ. Khôi phục.
- [ ] **Bước 6: ratchet và cổng full-suite.** Kỳ vọng: không số đếm nào dịch (bộ duyệt khoá vẫn bỏ qua các danh sách mới tới A7).
- [ ] **Bước 7: commit** `feat(workflow): flowables read finally and the loops read errors; Pause finally is refused for #9`.

---

### Task A3: plan nối thêm các danh sách mới (D3); đồ thị vẽ chúng

**Tệp:**
- Sửa: `src/executor/plan.rs`: `Plan::finally`, `PlanTask::finally`, `TaskKind::AllowFailure { tasks }`, `children_of`, `flatten_flow`, `flatten_one`.
- Sửa: mọi nơi đọc `TaskKind::AllowFailure { tasks, finally }`: `grep -rn "AllowFailure {" src`.
- Sửa: `src/graph/mod.rs:831-841` (vẽ `finally` cho mọi loại có trường đó), `src/graph/tests.rs`.

**Interfaces:**

```rust
pub struct Plan {
    pub tasks: Vec<PlanTask>,
    pub roots: Vec<TaskIx>,
    pub errors: Vec<TaskIx>,
    /// The flow's `finally:` (package #2a). Appended after every list the
    /// plan held before #2 (D3).
    pub finally: Vec<TaskIx>,
    pub retry: Option<RetryPolicy>,
}
pub struct PlanTask {
    // … unchanged …
    /// The block's `finally:` (Kestra `AbstractBranch._finally`). Inline for
    /// `AllowFailure` (where it always was), appended for every other kind (D3).
    pub finally: Vec<TaskIx>,
}
```

`flatten_flow` giữ `flatten_list(roots)` và `flatten_list(errors)` đứng đầu. `flatten_one` đẩy `(ix, &model_task)` vào một `deferred: VecDeque` cho mỗi loại có danh sách mới. Các loại là mọi flowable trừ `AllowFailure` (inline) và `Loop` (D5: không có gì ở cha). Rồi `flatten_flow` xả hàng đợi. Mỗi mục được xả đặt `out[ix].finally` và, với `LoopUntil`/`WorkingDirectory`, `out[ix].errors`, làm phẳng chúng bằng `flatten_list`, hàm này lại hoãn các danh sách mới lồng nhau. Rồi nó làm phẳng `flow.finally_` vào `Plan::finally` và xả lần nữa. Luồn hàng đợi như tham số `&mut` qua `flatten_list`/`flatten_one`.

- [ ] **Bước 1: test fail trước** (tests của `executor/plan.rs`):

```rust
#[test]
fn a_sequential_finally_is_planned_after_everything_that_was_planned_before() {
    let src = "id: f\nnamespace: a.b\ntasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks: [{id: a, type: core.log, message: m}]\n    finally: [{id: z, type: core.log, message: m}]\n  - id: after\n    type: core.log\n    message: m\nfinally:\n  - id: ff\n    type: core.log\n    message: m\n";
    let plan = plan_stored(src).unwrap();
    let ids: Vec<&str> = plan.tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["seq", "a", "after", "z", "ff"], "today's layout first, then the new lists");
    let seq = 0;
    assert_eq!(plan.tasks[seq].finally, vec![3]);
    assert_eq!(plan.finally, vec![4]);
    assert_eq!(plan.children_of(seq), vec![1, 3]);
}

#[test]
fn an_allow_failure_finally_stays_inline() {
    let src = "id: f\nnamespace: a.b\ntasks:\n  - id: af\n    type: io.kestra.plugin.core.flow.AllowFailure\n    tasks: [{id: a, type: core.log, message: m}]\n    finally: [{id: z, type: core.log, message: m}]\n  - id: after\n    type: core.log\n    message: m\n";
    let plan = plan_stored(src).unwrap();
    let ids: Vec<&str> = plan.tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["af", "a", "z", "after"]);
    assert_eq!(plan.tasks[0].finally, vec![2]);
}

#[test]
fn a_loops_errors_and_finally_are_not_planned_in_the_parent() {
    let src = "id: f\nnamespace: a.b\ntasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks: [{id: a, type: core.log, message: m}]\n    errors: [{id: e, type: core.log, message: m}]\n    finally: [{id: z, type: core.log, message: m}]\n";
    let plan = plan_stored(src).unwrap();
    assert!(plan.tasks.iter().all(|t| t.id != "e" && t.id != "z"));
}
```

  Trong `graph/tests.rs`: `a_sequential_finally_is_drawn_like_an_allow_failures`. Chép test AllowFailure sẵn có (`grep -n "AllowFailure" src/graph/tests.rs`) và khẳng định một node `z` trên `Branch::Finally`. Test corpus `every_corpus_flow_graphs_and_every_planned_task_is_one_node` (`executor/mod.rs:458`) phải giữ xanh; nó fail nếu đồ thị không vẽ một `finally` đã lập plan.
- [ ] **Bước 2: chạy, kỳ vọng FAIL:** `cargo test -p egret-workflow --lib -- executor::plan graph`.
- [ ] **Bước 3: triển khai.** Chạy test tiền tố A1: nó phải giữ xanh. Rồi chạy `cargo test -p egret-workflow --lib -- executor graph`. Kỳ vọng vài test executor trên các flow corpus có `finally:` đổi: executor chưa chạy các danh sách mới tới A4, nên các flow kết thúc mà không chạy chúng thì không đổi. Ghi mọi test dịch.
- [ ] **Bước 4: mutation.** (a) Làm phẳng `finally` của một `Sequential` inline (trong `flatten_one`) thay vì hoãn: test tiền tố A1 chuyển đỏ ở `valids/finally-sequential`. Khôi phục. (b) Bỏ `finally` khỏi `children_of`: `the_plan_before…` vẫn xanh, nhưng phép khẳng định cuối của `a_sequential_finally…` chuyển đỏ. Khôi phục.
- [ ] **Bước 5: ratchet và cổng full-suite.** Kỳ vọng: ratchet không đổi (chưa có gì chạy các danh sách).
- [ ] **Bước 6: commit** `feat(workflow): the plan appends finally lists after today's layout`.

---

### Task A4: một quy tắc danh sách, của Kestra (D2)

**Tệp:**
- Sửa: `src/executor/flowable/mod.rs` (`current_list`, `Current::Finally`), `sequential.rs`, `parallel.rs`, `dag.rs`, `branch.rs`, `allow_failure.rs`, `src/executor/mod.rs:225`.
- Test: một `mod end_hooks_drive_tests` mới trong `src/executor/mod.rs`, mô phỏng `allow_failure_drive_tests` (chép các helper `run` và `last_of` của nó, và thêm một helper `order` trả id các run theo thứ tự tạo).

**Interfaces:**

```rust
pub(crate) enum Current<'a> {
    Tasks(&'a [TaskIx]),
    Errors(&'a [TaskIx]),
    /// Kestra `findTaskDependingFlowState`'s `finally` answer (`Execution.java:589-593,615-620`).
    Finally(&'a [TaskIx]),
    RetryPending,
    Ended,
}
pub(crate) fn current_list<'a>(
    plan: &Plan, state: &ExecutionState,
    tasks: &'a [TaskIx], errors: &'a [TaskIx], finally: &'a [TaskIx],
    parent: Option<RunIx>, now: Tick,
) -> Current<'a>;
pub(crate) fn drive(  // sequential
    plan: &Plan, state: &ExecutionState,
    children: &[TaskIx], errors: &[TaskIx], finally: &[TaskIx],
    parent: Option<RunIx>, now: Tick,
) -> Step;
```

Thân `current_list` theo danh sách trường hợp §4.1 của spec từng dòng một, với một comment dẫn các dòng `Execution.java` ở mỗi nhánh. "Hết" = mọi task trong danh sách có một run mà state mới nhất là terminal.

- [ ] **Bước 1: test fail trước.** Mỗi dòng một test. Mỗi source là một dạng corpus cắt tối thiểu, dùng lá `core.log` và `io.kestra.plugin.core.execution.Fail` cho lỗi (như `BLOCK` làm).

```rust
/// spec §0.1, golden `finally-sequential`: tasks, then finally; the block ends
/// after its finally and keeps its tasks' state.
#[test]
fn a_sequential_runs_finally_after_a_failure_and_stays_failed() {
    let (plan, state, flow) = run(SEQ_FINALLY, &["ko"]);
    assert_eq!(order(&plan, &state), ["seq", "ko", "a1", "a2"]);
    assert_eq!(last_of(&plan, &state, "seq"), Some(RunState::Failed));
    assert_eq!(last_of(&plan, &state, "a1"), Some(RunState::Success));
    assert_eq!(flow, RunState::Failed);
}

/// `finally` runs on success too.
#[test]
fn a_sequential_runs_finally_after_success() { /* run(SEQ_FINALLY, &[]) -> seq SUCCESS, a1 a2 SUCCESS */ }

/// spec §0.2: a failing finally child changes nothing and does not stop the next one.
#[test]
fn a_failing_finally_child_neither_fails_the_block_nor_stops_the_next() {
    let (plan, state, flow) = run(SEQ_FINALLY, &["a1"]);
    assert_eq!(last_of(&plan, &state, "a1"), Some(RunState::Failed));
    assert_eq!(last_of(&plan, &state, "a2"), Some(RunState::Success));
    assert_eq!(last_of(&plan, &state, "seq"), Some(RunState::Success));
    assert_eq!(flow, RunState::Success);
}

/// golden `finally-sequential-error`: errors strictly before finally.
#[test]
fn errors_run_before_finally() { /* order: seq, ko, e1, e2, a1, a2 */ }

/// golden `finally-flow-error`: the flow's own lists in the same order.
#[test]
fn the_flows_errors_then_finally() { /* root: ko, e1, e2, a1, a2; flow FAILED */ }

/// golden `finally-flow-error-first`: the sibling after the failure is never created.
#[test]
fn the_flows_finally_skips_the_rest_of_tasks() { /* ko, a1; "ok" absent */ }

/// golden `finally-parallel`: finally is sequential under a Parallel.
#[test]
fn a_parallels_finally_runs_one_at_a_time() { /* a1 terminal before a2 is created */ }

/// `If.java:188-198`: an If whose branch is empty ends without its finally.
#[test]
fn an_if_with_nothing_to_run_skips_its_finally() { /* condition false, no else: z absent */ }

/// `FU:163-170`: a block whose children are all disabled is SUCCESS without finally.
#[test]
fn a_block_of_disabled_children_skips_its_finally() { /* all disabled: z absent, seq SUCCESS */ }

/// `Execution.java:602-606`: a failure that will be retried starts neither list.
#[test]
fn a_retry_pending_failure_starts_no_finally() { /* ko with retry maxAttempts 2: z created only after the last attempt */ }

/// allow_failure_drive_tests keep passing unchanged: AllowFailure is on the same rule.
```

  Viết mỗi `/* … */` thành code đầy đủ theo dạng của test đầu tiên. `SEQ_FINALLY` là một const như `BLOCK`: một `Sequential` `seq` chứa `ko` (`Fail`), với `finally: [a1, a2]` (`core.log`).
- [ ] **Bước 2: chạy, kỳ vọng FAIL:** `cargo test -p egret-workflow --lib -- executor::end_hooks_drive_tests executor::allow_failure_drive_tests`.
- [ ] **Bước 3: triển khai**, rồi chạy `cargo test -p egret-workflow --lib -- executor`.
- [ ] **Bước 4: mutation**, mỗi cái được khôi phục sau khi đỏ:
  - (a) Trong `current_list`, trả `Errors` trước khi kiểm "finally started": `errors_run_before_finally` vẫn xanh, nhưng một flow mà `finally` đã bắt đầu khi `errors` tồn tại phải đỏ. Nếu không gì đỏ, thêm `once_finally_has_started_errors_are_not_re_entered`.
  - (b) Cho `Done` aggregate cả `finally`: `a_failing_finally_child…` chuyển đỏ.
  - (c) Bỏ phép bỏ qua nhánh rỗng của `If`: `an_if_with_nothing_to_run…` chuyển đỏ.
  - (d) Chạy `Finally` bằng `parallel::drive_concurrent`: `a_parallels_finally_runs_one_at_a_time` chuyển đỏ.
- [ ] **Bước 5: ratchet, và bank trong task này.** Kỳ vọng: mười mục báo "banked": `finally-flow`, `finally-flow-error`, `finally-flow-error-first`, `finally-sequential`, `finally-sequential-error`, `finally-sequential-error-first`, `finally-dag`, `finally-parallel`, `restart-with-finally` và `task-runif-taskswithstate`. Danh sách thật là cái bản in nói.
  - Gỡ đúng các dòng đó khỏi `oracle-exec/baseline.txt`, không hơn.
  - Dịch mọi hằng mà chúng làm dịch, mỗi cái kèm comment có ngày nêu tên chúng: `(301, 229, 72)` (`EX:813`), các dòng attribution (`EX:6999`, `EX:7044`), `MISMATCH_ON_*`, `RESTRICTION_REMOVES*`, guard handler-surface và tệp sinh lại của nó, và các số đếm README.
  - Với một mục kỳ vọng mà không bank, ghi trường khác biệt đầu tiên từ bản in và để nguyên dòng của nó. A8 viết lại nguyên nhân.
  - Ratchet xanh lại ở cuối bước này. Mọi thay đổi ngoài "banked" là một điểm dừng.
- [ ] **Bước 6: cổng full-suite.**
- [ ] **Bước 7: commit** `feat(workflow): finally runs on every flowable and on the flow, by Kestra's list rule`.

---

### Task A5: `LoopUntil` và `WorkingDirectory` chạy `errors:` và `finally:` của chúng (D6, D7)

**Tệp:** `src/executor/flowable/loop_until.rs`, `src/executor/flowable/working_directory.rs`, và các test trong `end_hooks_drive_tests`.

- [ ] **Bước 1: đo trước (spec §4.1).** Đọc `worker/working_dir.rs` và đường `NotALeaf`/kết thúc khối của `worker/claim.rs`. Trả lời trong báo cáo: sau `Command::FinishWorkingDirectory` và `CompleteWorkingDirectoryClaim`, liệu run của khối có thể giữ `RUNNING` trong khi các con `errors`/`finally` của nó chạy, mà worker không coi nó là một khối còn sống (lập lại chuẩn bị nó, hay `_wd` sweep xoá thứ nó cần)? Nếu không, **dừng**. Báo controller và đề xuất phương án lùi của spec §4.1: từ chối `errors:`/`finally:` của `WorkingDirectory` bằng `unsupported_working_directory_property`, nêu #17.
- [ ] **Bước 2: test fail trước:**
  - `a_loop_until_runs_errors_then_finally_in_each_iteration_before_the_condition`: hai lần lặp, thân fail ở lần 1. Khẳng định thứ tự `body, e, z` trong mỗi lần lặp, và điều kiện được render sau `z`. Dùng harness test sẵn có của `loop_until`: `grep -n "fn .*loop_until" src/executor/mod.rs`.
  - `a_working_directory_runs_errors_and_finally_after_it_finishes` và `a_working_directory_runs_finally_after_success` (D7: egret chạy nó; code Kestra không, Q4).
  - `a_restarted_loop_until_discards_its_finally_runs` (`RestartLoop` chạm `finally` qua `children_of`).
- [ ] **Bước 3: chạy, kỳ vọng FAIL**, rồi triển khai. Chạy `cargo test -p egret-workflow --lib -- executor` và `--test working_dir_claims`.
- [ ] **Bước 4: mutation.** Hỏi điều kiện trước `finally`: test đầu chuyển đỏ. Khôi phục.
- [ ] **Bước 5: ratchet** (không kỳ vọng bank mới; ghi lại) **và cổng full-suite.**
- [ ] **Bước 6: commit** `feat(workflow): LoopUntil and WorkingDirectory run their errors and finally`.

---

### Task A6: một lần lặp `Loop` chạy `errors:` và `finally:` của `Loop` (D5)

**Tệp:** `src/worker/spawn.rs` (`sub_flow_source`, doc module của nó `:50-62`, `find_task`).

- [ ] **Bước 1: test fail trước** (tests của `spawn.rs`):

```rust
#[test]
fn an_iteration_runs_the_loops_errors_and_finally_as_its_own() {
    let src = "id: f\nnamespace: a.b\nerrors: [{id: fe, type: core.log, message: m}]\nfinally: [{id: ff, type: core.log, message: m}]\nafterExecution: [{id: fa, type: core.log, message: m}]\ntasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks: [{id: a, type: core.log, message: m}]\n    errors: [{id: e, type: core.log, message: m}]\n    finally: [{id: z, type: core.log, message: m}]\n";
    let sub = crate::spec::parse(&sub_flow_source(src, "each", uuid::Uuid::nil()).unwrap()).unwrap();
    let ids = |l: &[crate::models::Task]| l.iter().map(|t| t.id.clone()).collect::<Vec<_>>();
    assert_eq!(ids(&sub.tasks), ["a"]);
    assert_eq!(ids(&sub.errors), ["e"], "the Loop's errors, not the flow's (ES:1037-1043)");
    assert_eq!(ids(&sub.finally_), ["z"]);
    assert!(sub.after_execution.is_empty(), "a LOOP iteration has no afterExecution (ExS:1331)");
}
```

- [ ] **Bước 2: chạy, kỳ vọng FAIL**, rồi triển khai. Viết lại đoạn "Where this is knowingly narrower than Kestra" của doc module (`spawn.rs:50-62`) thành một đính chính có ngày (nó là lịch sử; không xoá).
- [ ] **Bước 3: ratchet, và bank trong task này như ở A4.** Các bank mới kỳ vọng: `valids/finally-loop`, `valids/finally-loop-parallel` và `invalids/loop-switch-failed`. `SPAWNED_ENTRIES_DIFFERING_ONLY_INSIDE_SPAWNED` (`EX:4826`) dịch cùng chúng.
- [ ] **Bước 4: mutation.** Tiếp tục xoá `errors`: test chuyển đỏ, và bank của `loop-switch-failed` trong ratchet cũng đỏ. Khôi phục.
- [ ] **Bước 5: cổng full-suite.**
- [ ] **Bước 6: commit** `feat(workflow): a Loop iteration runs the Loop's errors and finally`.

---

### Task A7: bộ duyệt khoá đi xuống các danh sách mới; property treatment sinh lại

**Tệp:** `src/spec/keys.rs`, `src/spec/schema/property-treatment.json`, `src/spec/schema/README.md` (số đếm).

- [ ] **Bước 1: test fail trước** (tests của `keys.rs`; thay `finally_and_the_errors_two_takes_draw_nothing` ở `:1636-1650`):

```rust
#[test]
fn a_typo_inside_a_flowable_finally_is_refused_at_its_path() {
    only(
        &format!("{HEAD}tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks: [{{id: a, type: core.log, message: m}}]\n    finally:\n      - id: z\n        type: core.log\n        message: m\n        mesage: typo\n"),
        "tasks.seq.z.mesage",
        "unknown_property",
    );
}

#[test]
fn a_typo_inside_a_loop_errors_is_refused_at_its_path() {
    only(
        &format!("{HEAD}tasks:\n  - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks: [{{id: a, type: core.log, message: m}}]\n    errors:\n      - id: e\n        type: core.log\n        message: m\n        mesage: typo\n"),
        "tasks.each.e.mesage",
        "unknown_property",
    );
}

#[test]
fn finally_and_the_loops_errors_are_model_keys_now() {
    silent_census(&format!("{HEAD}tasks:\n  - id: seq\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks: [{{id: a, type: core.log, message: m}}]\n    finally: [{{id: z, type: core.log, message: m}}]\n"));
}
```

  Kiểm dạng path với `spec::path::task_path` (`{parent path}.{id}`) trước khi khẳng định.
- [ ] **Bước 2: chạy, kỳ vọng FAIL**, rồi triển khai:
  - `silent()` mất `"finally" | "errors"` cho flowable (`finally` của Pause vẫn chỉ được `pause_gap` báo, nên thêm `|| (t == Pause && key == "finally")` vào `silent`);
  - `child_lists` thêm chúng theo thứ tự `children()`/`error_children()`;
  - xoá `TWO` (`:1351`) và các chỗ dùng nó trong phép đối chiếu.
- [ ] **Bước 3: sinh lại** `EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow --lib -- spec::schema`. Rồi `git diff --stat` và `git diff` trên tệp. Kỳ vọng: `finally` trên 8 flowable và `errors` trên 3 đi từ `ignored` sang `honoured`, và `finally` của Pause đi từ `ignored` sang `refused`. Không gì khác được dịch. Cập nhật số đếm trong `spec/schema/README.md` từ tệp. Chạy `--test flow_schema`.
- [ ] **Bước 4: mutation.** Bỏ `finally` khỏi nhánh `Sequential` của `child_lists`: test đầu chuyển đỏ. Khôi phục.
- [ ] **Bước 5: ratchet** (`LIVE`/`VALIDATE_REFUSED` không đổi; ghi lại) **và cổng full-suite.**
- [ ] **Bước 6: commit** `feat(workflow): the key walker reads into finally and the loops' errors`.

---

### Task A8: kiểm các bank và hằng của Phần A, viết docs #2a (EN). Ranh giới PR #2a.

**Tệp:** `oracle-exec/baseline.txt`, `tests/executor.rs`, `oracle-exec/README.md`, `oracle-exec/handler_surface.py`, `oracle-exec/handler-surface.txt`, docs.

- [ ] **Bước 1:** Kiểm tổng bank với spec §7.1. Kỳ vọng: 13 mục được bank qua A4 và A6, nên `(301, 242, 59)`. Ghi con số đo được và mọi mục kỳ vọng mà không bank.
- [ ] **Bước 2:** Đọc lại mọi hằng đã dịch ở A4 và A6 với bản in. Sửa cái nào lệch, kèm comment có ngày.
- [ ] **Bước 3:** Viết lại comment baseline của mọi mục không bank bằng nguyên nhân còn lại.
- [ ] **Bước 4: docs (EN), dưới dạng ghi chú có ngày; không xoá gì:**
  - spec này: một dòng Status cho #2a, và một ghi chú "như đã xây" có ngày dưới mỗi mục mà bản build đã đổi;
  - roadmap `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md` §2:
    - việc tách, kèm các bank đo được của dòng #2;
    - `finally:` của `Pause` chuyển sang #9 (D14);
    - một ghi chú dưới ghi chú 3 ngày 2026-10-26 rằng tiền đề `snapshot.source` của nó là sai (spec "Tiền đề sai" 1);
    - đính chính của Quy tắc 2 (tiền đề 2);
    - #6 được thêm việc tính `outputs:` cấp flow (spec §7.1);
    - #7 phải gỡ các run `errors`/`finally`/`afterExecution` khi restart (spec §9);
  - `docs/en/specs/2026-10-19-workflow-no-silent-drops-design.md` §10: một ghi chú có ngày rằng #2a đã đóng phần theo sau `finally`/`errors`, và chỉ dẫn snapshot là một tiền đề sai;
  - `docs/en/specs/2026-09-01-workflow-programme-design.md` §9: một ghi chú có ngày cho #2a.
- [ ] **Bước 5: ratchet và cổng full-suite**, cả hai xanh.
- [ ] **Bước 6: commit** `docs(workflow): package #2a as built` (cộng `test(workflow): …` nếu Bước 2 đã sửa một hằng).
- [ ] **Bước 7: giao lại cho controller để review toàn nhánh và mở PR #2a.** Phần B bắt đầu trên nó.

---

## Phần B — PR #2b: `afterExecution:` và Quy tắc 2

### Task B1: pha sau terminal trong `advance` (D8, D10)

**Tệp:**
- Tạo: `src/executor/after_execution.rs`.
- Sửa: `src/executor/mod.rs` (lệnh return sớm của `advance`; `pub use`), `src/executor/plan.rs` (`Plan::after_execution`, làm phẳng cuối cùng, rồi xả), `src/worker/spawn.rs` (`find_task` cũng duyệt `after_execution`).

**Interfaces:**

```rust
/// Kestra `ExecutorService.handleAfterExecution` (`ES:1005-1023`): once the
/// execution is terminal, run `afterExecution:` one task at a time. The
/// execution's state is never touched (`ES:1026`): no `SetExecutionState`, no
/// retry (spec D11), no pause (D12).
pub(crate) fn drive(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command>;
/// Kestra `ExecutionService.isTerminated(flow, execution)` (`ExS:1325-1338`).
pub fn terminated(plan: &Plan, state: &ExecutionState) -> bool;
/// Terminal and not [`terminated`]: the phase is running.
pub fn drives_after(plan: &Plan, state: &ExecutionState) -> bool;
```

- [ ] **Bước 1: test fail trước** (`end_hooks_drive_tests`; mở rộng `run` để nó tiếp tục gọi `advance` sau khi execution terminal tới khi không còn gì làm, và trả history của execution):

```rust
/// golden `after-execution-error`: CREATED RUNNING FAILED, and `end` runs after.
#[test]
fn after_execution_runs_after_the_execution_is_terminal_and_changes_nothing() {
    let (plan, state, flow) = run(AFTER_ERROR, &["fail"]);
    assert_eq!(state.history, [RunState::Created, RunState::Running, RunState::Failed]);
    assert_eq!(order(&plan, &state), ["fail", "hello", "end"]);
    assert_eq!(last_of(&plan, &state, "end"), Some(RunState::Success));
    assert_eq!(flow, RunState::Failed);
    assert!(terminated(&plan, &state));
}

/// A failing after-task does not change the execution (ES:1026), and is not retried (D11).
#[test]
fn a_failing_after_task_changes_nothing_and_is_not_retried() { /* retry on the task; one attempt, FAILED; history unchanged */ }

/// D10: terminal is not terminated while the list runs.
#[test]
fn terminated_waits_for_the_after_list() { /* step until `end` is RUNNING: last state terminal, terminated == false, drives_after == true */ }

/// D8: KILLING runs nothing; KILLED runs the list (ES:192-193, ES:1015).
#[test]
fn after_execution_runs_once_killed_and_not_while_killing() { /* apply kill_leaves; assert no after run while KILLING; after KILLED, `end` created */ }

/// No round that makes the execution terminal also starts the phase, and a RETRYING round never does.
#[test]
fn a_retrying_execution_does_not_start_after_execution() { /* flow retry RETRY_FAILED_TASK: no `end` run before the final FAILED */ }

/// No command of the phase writes the execution.
#[test]
fn the_phase_emits_no_execution_state_retry_or_pause() { /* drive over every corpus flow with afterExecution: (plan_of), with a terminal history, asserting the command kinds */ }
```

- [ ] **Bước 2: chạy, kỳ vọng FAIL**, rồi triển khai. Chạy test tiền tố A1 (`after_execution` được nối cuối cùng, nên nó vẫn xanh) và `cargo test -p egret-workflow --lib -- executor`.
- [ ] **Bước 3: mutation:**
  - (a) Bắt đầu pha trong cùng vòng với terminal: `a_retrying_execution…` hoặc phép khẳng định history chuyển đỏ.
  - (b) Gọi `retry::scan` trong pha: `a_failing_after_task…` chuyển đỏ.
  - (c) Chạy pha dưới `KILLING`: test kill chuyển đỏ.

  Khôi phục từng cái.
- [ ] **Bước 4: ratchet.** Oracle chạy qua worker, nên không mục Phần B nào được kỳ vọng bank trước B3. Nếu có, bank nó ở đây như ở A4. **Cổng full-suite.**
- [ ] **Bước 5: commit** `feat(workflow): afterExecution runs once the execution is terminal`.

---

### Task B2: cái gì bị từ chối hay cảnh báo bên trong `afterExecution:` (D11, D12)

**Tệp:** `src/spec/validate.rs` (`validate_for_run` được thêm lời từ chối), `src/spec/keys.rs` (cảnh báo).

**Thông báo (của riêng egret):**
- `unsupported_after_execution_task` (lỗi), path `afterExecution.<id>` (lồng: path của task): ``task '<id>' is a <Pause|Exit> inside afterExecution, which egret does not run: the execution has already ended, and a <Pause> would pause it again (<an Exit> would set its state). Roadmap package <#9|#20> lifts it``.
- `not_honoured` (cảnh báo) trên một `retry:` chạm tới một task `afterExecution:`, path `<task path>.retry` (hoặc `retry` cho của flow): ``an afterExecution task is not retried on egret: the execution has already ended. Roadmap package #13 lifts it``.

- [ ] **Bước 1: test fail trước:** `a_pause_inside_after_execution_is_refused_at_start` (qua `build_plan`), `an_exit_nested_in_an_if_inside_after_execution_is_refused`, `a_retry_inside_after_execution_warns_thirteen`, `a_flow_retry_with_after_execution_warns_thirteen`, `a_pause_in_tasks_is_not_touched_by_this_rule`.
- [ ] **Bước 2: chạy, kỳ vọng FAIL**, rồi triển khai. Duyệt `flow.after_execution` bằng `Task::children()`/`error_children()` đệ quy.
- [ ] **Bước 3: census.** Ratchet: `LIVE`/`VALIDATE_REFUSED` không đổi (spec §7.2). Ghi lại.
- [ ] **Bước 4: mutation.** Chỉ duyệt cấp trên cùng: test `Exit` lồng chuyển đỏ. Khôi phục.
- [ ] **Bước 5: cổng full-suite. Commit** `feat(workflow): Pause and Exit inside afterExecution are refused; retry there is warned`.

---

### Task B3: worker chạy pha; cha nghe ở "terminated" (D10)

*Ghi chú, 2026-10-28:* ở bất cứ chỗ nào task này hay kế hoạch nói theo Kestra về D9 (kill một execution terminal vẫn 409), việc Q3 thay thế D9 trong spec chi phối điều đã được xây: một lệnh kill có run `afterExecution:` còn sống được chấp nhận và dừng nó (409 `execution_terminated` khi không có, 409 `kill_not_applied` khi phase kill chưa áp dụng được).

**Tệp:** `src/worker/claim.rs` (`terminal:` của đuôi settle ở `:3487-3497`), `src/worker/render.rs:1504`, `tests/end_hooks.rs` (mới; chép phần setup của một tệp test DB sẵn có như `tests/kill_route.rs`).

- [ ] **Bước 1: đo.** Liệt kê mọi cổng trong `worker/render.rs` và `worker/claim.rs` đọc `drives_forward` hoặc `is_terminal()` trên execution: `grep -n "drives_forward\|history.last().*is_terminal\|is_terminal()" src/worker/render.rs src/worker/claim.rs`. Phân loại từng cái: mở rộng thành `drives_forward || drives_after`, hoặc giữ, kèm lý do. Ghi bảng vào báo cáo.
- [ ] **Bước 2: test DB fail trước** (`tests/end_hooks.rs`):
  - `an_after_execution_leaf_is_claimed_and_run_after_the_execution_ended`. Khẳng định `state`, `history` và `ended_at` của execution không đổi bởi settle của after-run, và dòng của after-run đạt `SUCCESS`.
  - `the_flow_trigger_event_is_written_once_at_the_terminal_state`: một dòng `flow_trigger_events` cho state terminal, không dòng nào cho after-run.
  - `a_subflow_parent_hears_the_child_only_after_its_after_execution`. Con có `afterExecution: [Sleep PT0S]`; run `Subflow` của cha vẫn `RUNNING` tới khi after-run của con terminal (`DE:752-759`).
  - `a_set_variables_inside_after_execution_runs` (cổng `render_updates` đã mở rộng).
  - `an_if_inside_after_execution_renders_and_runs` (golden `after-execution-flowable`).
- [ ] **Bước 3: chạy, kỳ vọng FAIL**, rồi triển khai. `Settled.terminal` thành `Some(last)` khi `executor::terminated(plan, after)` và không `terminated(plan, before)`. Một settle bị ép (`forced: true`) giữ quy tắc hiện nay, vì nó không có plan.
- [ ] **Bước 4: mutation.** Báo cha ở state terminal như cũ: test subflow chuyển đỏ. Khôi phục.
- [ ] **Bước 5: ratchet, và bank trong task này như ở A4** (kỳ vọng: 8 mục Phần B của spec §7.1, với danh sách tên kill của `EX:7527` bị làm rỗng và một comment có ngày: `flow-concurrency-queue-killed-after-execution` được bank, và không có gì bị kill trong lần thu của nó). Đo rằng vòng tick của `oracle-exec/driver.rs` (`:1160-1241`) chạy các after-run: một mục như `after-execution-error` giờ phải báo "banked". Nếu không, đọc điều kiện dừng của driver và báo cáo trước khi đổi nó. **Cổng full-suite.**
- [ ] **Bước 6: commit** `feat(workflow): the worker runs afterExecution, and a parent hears its child once it is terminated`.

---

### Task B4: các loại còn lại bên trong `afterExecution:` (phép đo của D12)

**Tệp:** `tests/end_hooks.rs`.

- [ ] **Bước 1:** Mỗi loại một test DB: một `Subflow`, một `Loop` (values `[1, 2]`), một `LoopUntil` (hai lần lặp) và một `WorkingDirectory` (một con `core.log`; không cần script) bên trong `afterExecution:`. Mỗi cái chạy tới cuối: mọi after-run terminal, và history của execution không đổi. Dùng vòng worker mà các test DB khác dùng, với số tick bị chặn, để một lần treo fail thay vì đứng.
- [ ] **Bước 2:** Với loại nào treo hay lỗi, không vá vòng quanh. Thêm loại đó vào lời từ chối của B2 (`unsupported_after_execution_task`, nêu dòng theo sau của #2: hỏi controller số nào), giữ test fail đó dưới dạng `#[ignore = "refused since package #2b: …"]` kèm lý do, và báo cáo.
- [ ] **Bước 3: ratchet và cổng full-suite. Commit** `test(workflow): Subflow, Loop, LoopUntil and WorkingDirectory inside afterExecution`.

---

### Task B5: Quy tắc 2 — `workflow.execution_is_idle` (D13, spec §6)

**Tệp:**
- Tạo: `migrations/0210_workflow_execution_is_idle.sql` (SQL của spec §6, nguyên văn).
- Sửa: `src/worker/commands.rs` (truy vấn thứ hai của `run_liveness` `:2591-2597`; doc ở `:2669-2671`), `src/storage/wd_sweep.rs:163-169`, `src/storage/prune.rs` (tập "has not ended").
- Test: `tests/migration_0210_execution_is_idle.rs` (mới, mô phỏng `tests/migration_0197_wd_swept.rs`), cộng các test sweep.

**Interfaces:** truy vấn superseded thành:

```sql
SELECT e.id,
       GREATEST(COALESCE(e.ended_at, e.updated_at),
                COALESCE((SELECT max(r.ended_at) FROM workflow.task_runs r WHERE r.execution_id = e.id),
                         COALESCE(e.ended_at, e.updated_at))),
       now()
  FROM workflow.executions e
 WHERE e.id = ANY($1) AND workflow.execution_is_idle(e.id)
```

Phép chọn `_wd` được thêm `AND workflow.state_is_terminal(t.state)`. "An execution that has not ended" của pruner thành `NOT workflow.execution_is_idle(e.id)`. Đọc lại `prune.rs` trước và chỉ đổi vị từ quyết "ended".

- [ ] **Bước 0: kiểm số.** `ls migrations | tail -3`. Nếu 0210 đã bị chiếm trên main hoặc bởi một PR đang mở mà controller nêu, đánh số lại trên cả hai (memory: số migration là một không gian tên dùng chung).
- [ ] **Bước 1: test fail trước:**
  - `execution_is_idle_is_false_while_a_run_is_live_after_the_end`, `…_true_when_terminal_and_every_run_terminal`, `…_false_while_running` (DB; chèn dòng trực tiếp như `migration_0197_wd_swept.rs` làm).
  - `a_superseded_kept_container_is_not_judged_while_after_execution_runs`: một lời gọi `run_liveness` với một execution terminal có một run còn sống. Run thiếu dòng của nó không nằm trong `superseded`.
  - `the_superseded_retention_runs_from_the_last_runs_end`.
  - `the_wd_sweep_leaves_a_live_block_of_a_terminal_execution` (một khối `RUNNING`, `updated_at` cũ 25 giờ). Giữ `the_wd_sweep_removes_a_force_closed_blocks_snapshots` xanh.
  - `the_pruner_keeps_an_execution_whose_after_execution_runs`.
- [ ] **Bước 2: chạy, kỳ vọng FAIL**, rồi triển khai. Viết lại đoạn tiền đề của `sweep_decision` (`commands.rs:2669-2671`). Tiền đề giờ là "idle", vị từ được nêu tên, và đoạn nói rằng các `Exit` anh em đã phá tiền đề cũ (Tiền đề sai 2 của spec).
- [ ] **Bước 3: mutation.** Lần lượt hoàn nguyên từng nơi đọc về `state_is_terminal(e.state)`; test của nó chuyển đỏ. Khôi phục.
- [ ] **Bước 4:** `cargo test -p egret-workflow --lib -- worker::commands worker::k8s storage`, `--test migration_0210_execution_is_idle`, các ca sweep Docker thật chỉ khi Docker chạy (ghi chúng có chạy không), **ratchet và cổng full-suite.**
- [ ] **Bước 5: commit** `feat(workflow): sweeps and the pruner read execution_is_idle, not terminal (roadmap Rule 2)`.

---

### Task B6: các trang execution theo dõi run tới khi nó idle (D15)

**Tệp:**
- `frontend/src/components/workflows/executionState.ts`: `executionLive(execution, runs)`.
- `frontend/src/components/workflows/executions/polling.ts`: `executionFinished`.
- `frontend/src/components/workflows/executions/topology/ExecutionTopologyTab.tsx:67-79`, `gantt/ExecutionGanttTab.tsx:91`, `timing.ts:50`.
- Các `__tests__` của chúng.

Đọc `docs/en/frontend-rules.md` trước. Task này chỉ đổi quy tắc dữ liệu: không component mới, không chữ mới, không sinh lại API.

- [ ] **Bước 1: vitest fail trước:**
  - `executionFinished` là false với `{state: "FAILED", runs: [{state: "RUNNING"}]}` và true với `{state: "FAILED", runs: [{state: "SUCCESS"}]}`;
  - tab topology giữ `refetchInterval` của nó khi một after-run còn sống;
  - thanh Gantt của một after-run còn sống tiếp tục dài ra (`timing.ts`).
- [ ] **Bước 2: chạy, kỳ vọng FAIL** (`npm --prefix frontend run test -- executions`), rồi triển khai.
- [ ] **Bước 3: mutation.** Bỏ qua `runs` trong `executionLive`: test polling chuyển đỏ. Khôi phục.
- [ ] **Bước 4:** `npm --prefix frontend run lint` và `npm --prefix frontend run typecheck` (hoặc các target `just` mà repo dùng: `grep -n "frontend" justfile`). Rồi cổng full-suite Rust (code không đổi, vẫn bắt buộc).
- [ ] **Bước 5: commit** `feat(workflows-ui): execution pages keep following while afterExecution runs`.

---

### Task B7: kiểm các bank và hằng của Phần B, viết docs #2b (EN). Ranh giới PR #2b.

- [ ] **Bước 1:** Kiểm tổng bank Phần B với spec §7.1. Kỳ vọng: `(301, 250, 51)`. Ghi con số đo được và mọi mục kỳ vọng mà không bank, và viết lại comment baseline của nó bằng nguyên nhân còn lại.
- [ ] **Bước 2:** Đọc lại mọi hằng đã dịch ở B3 với bản in, như ở A8.
- [ ] **Bước 3: docs (EN):**
  - các ghi chú như-đã-xây của spec này;
  - dòng #2 của roadmap: xong, kèm các bank đo được;
  - spec nợ §4.7: một ghi chú có ngày rằng `sweep_decision` nay đọc `execution_is_idle`;
  - programme §9: một ghi chú có ngày cho #2b;
  - `oracle-exec/README.md`: mục end-hooks, kèm các mục không bank và vì sao.
- [ ] **Bước 4: ratchet và cổng full-suite**, cả hai xanh. **Commit** `docs(workflow): package #2b as built`.

---

### Task B8: docs tiếng Việt, và docs của platform

- [ ] **Bước 1:** `docs/vi/specs/2026-10-26-workflow-end-hooks-design.md` và `docs/vi/plans/2026-10-26-workflow-end-hooks.md` dịch theo EN từng mục một. Header của chúng nói EN thắng. Dịch các ghi chú đã thêm vào roadmap VI, §10 của spec no-silent-drops VI, spec nợ VI và spec programme VI.
- [ ] **Bước 2:** Docs Platform của `egret-data` (quy tắc docs của `CLAUDE.md` của repo), trên trang workflow cho các flow, giải thích:
  - `finally:` trên mọi khối;
  - `afterExecution:` chạy sau khi execution đã kết thúc, không kill được, không được retry, và từ chối `Pause`/`Exit`;
  - trang execution tiếp tục cập nhật tới khi các after-task xong.

  *Đính chính, 2026-10-28 (Task B7, Q3 như đã xây):* "không kill được" ở trên bị thay thế. Docs Platform phải nói thế này: nút Kill vẫn có trên một execution đã kết thúc trong khi các task `afterExecution:` của nó chạy, và một lệnh kill dừng chúng mà không đổi state của chính execution; một lệnh kill chưa áp dụng được trả `kill_not_applied` (retry); kill của cha cũng dừng `afterExecution:` đang chạy của một con. `Pause`, `Exit` và `LoopUntil`/`WaitFor` bị từ chối bên trong `afterExecution:`, và một `retry:` ở đó bị cảnh báo. Trang theo dõi các after-task tối đa 30 phút sau khi kết thúc.

  Thay đổi này đi vào PR docs của repo đó; controller mở nó.
- [ ] **Bước 3: commit** `docs(workflow): package #2 in Vietnamese`.

## Cổng cuối (controller, sau review toàn nhánh của mỗi PR)

Trên nhánh được rebase lên `origin/main` mới, với môi trường của Ràng buộc chung, một lệnh mỗi lúc:
1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test -p egret-workflow --no-fail-fast`. Ratchet: sau #2a, kỳ vọng 301 / 59 / 59 / 0 (con số đo được chi phối); sau #2b, 301 / 51 / 51 / 0. `LIVE` 295, `VALIDATE_REFUSED` 25 và `UNKNOWN_TYPE_REFUSED` 21, trừ khi một task đã đo và ghi khác đi.
4. `cargo test -p egret-server` (OpenAPI: không đổi DTO, nên kỳ vọng không có diff).
5. Chỉ #2b: `npm --prefix frontend run test`, `lint` và `typecheck`.
6. Chỉ #2b: số migration được kiểm lại với main và các PR đang mở.

*Ghi chú, 2026-10-28 (sau #2b):* ratchet đo được là 301 / 52 / 52 / 0, không phải 51 (mục 3). "Không có diff" của mục 4 không còn đúng: `openapi.json` đã đổi (chỉ phần chữ, mô tả của route kill và của trang log), nên các test drift của `egret-server` cần sinh lại `openapi.json` và client sinh ra.

Đã biết và chấp nhận: 7 test `sql_starrocks_live` fail khi không có engine, và `runtime::a_cancelled_token_stops_both_tickers` có thể flake khi tải (chạy riêng thì pass).
