> Bản tham khảo tiếng Việt. Bản gốc (chuẩn) là [docs/en/specs/2026-10-19-workflow-debt-design.md](../../en/specs/2026-10-19-workflow-debt-design.md); khi hai bản khác nhau, bản tiếng Anh thắng.

# Gói nợ workflow: những gì nhóm B, C1, C2 và WorkingDirectory còn để mở

**Trạng thái:** thiết kế ngày 2026-10-19. PR-1 đã xây và kiểm chứng ngày 2026-10-19 trên nhánh `feat/workflow-debt-kill-wd` (PR-1); PR-2 đã xây và kiểm chứng ngày 2026-10-19. Thứ tự của user ngày 2026-10-18 đặt gói này sau khi C2 merge và trước roadmap parity với Kestra (dòng 1 của `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md` chờ gói này).

**Nguồn của các khẳng định:** mọi khẳng định về code egret đều đã kiểm tra ở worktree `egret-coretasks` tại `481d9101` (nhánh `feat/workflow-kestra-executor-tasks`, C2 ở HEAD, tức là hình dạng của main sau khi C2 merge). Code được trích dẫn dạng `path:line`, tính từ `crates/egret-workflow/src/` trừ khi ghi gốc khác. Kestra là bản checkout tại `data-portal/kestra`, đường dẫn của nó tính từ đó. Khẳng định nào không kiểm tra tĩnh được thì ghi "to measure in Task N" và nêu task của plan sẽ đo. Chỗ nào ghi chú nợ nói điều mà code không còn cho thấy, mục đó được báo là lỗi thời (stale), không đưa vào kế hoạch.

**Nợ được ghi lại ở đâu:**
- nợ kill và nợ C1: các đoạn nhóm B và nhóm C1 trong memory của chương trình, và ghi chú §4.2 của spec kill (`docs/en/specs/2026-10-17-workflow-kill-and-exit-design.md:101`);
- nợ WorkingDirectory: open point 2 của spec WorkingDirectory (`docs/en/specs/2026-10-16-workflow-working-directory-design.md:174`) và ghi chú merge của #683;
- các minor của C2: danh sách "can wait" trong final review C2 (M3–M8) và các dòng "minor (deferred)" trong ledger C2.

**Đã chốt với user, 2026-10-18:**
- **Gói này đi trước roadmap parity.**
- **Task bị kill mà container được khởi động với `taskRunner.delete: false` thì giữ container.** Hiện nay kill xoá nó. Orphan sweep sau đó xoá các container như vậy khi quá hạn; hiện nay nó bỏ qua chúng mãi mãi.

**Đã chốt với user, 2026-10-19** (hai câu hỏi mở của bản nháp đầu):
- **Hạn của container được giữ (D3) áp dụng cho mọi container được giữ mà run đã terminal**, bất kể run kết thúc ở trạng thái nào, nên các run kết thúc bình thường cũng mất container được giữ sau hạn. Kestra giữ chúng mãi mãi; egret đặt giới hạn, vì Docker runner không có giới hạn đĩa.
- **Publish chấp nhận target `Subflow`/`dependsOn` không có trong workspace của caller, kèm warning như Kestra, và chỉ phân giải nó trong workspace đó** (§5.1). Bốn test bảo mật được viết lại để giữ tính chất không-dò-được. §5.1 được xây trong PR-2.

**Quyết định mặc định (2026-10-19).** Nguyên tắc: theo Kestra, trừ khi egret có lý do làm khác; giữ an toàn chặt chẽ.
- **D1. Trên Docker, kill hoặc budget hết hạn thì dừng (stop) container `delete: false` chứ không xoá.** Lệnh dừng là `docker stop -t <grace>`, grace bằng 0 thì kill ngay. Đây là `Docker.cleanupContainer` của Kestra: gọi `kill()`, rồi `removeContainerCmd` chỉ khi `delete` là true (`script/src/main/java/io/kestra/plugin/scripts/runner/docker/Docker.java:662-690`, `kill` ở `:777-800`). Budget hết hạn và kill dùng chung một `cancel` (`worker/cancel.rs:208`), nên cả hai đổi cùng nhau, như Kestra. `delete: true` giữ `rm --force`.
- **D2. Kubernetes giữ hành vi hiện tại: pod bị kill thì bị xoá, dù `delete: false` hay không.** Không thể dừng container của pod mà không xoá pod; cách duy nhất là exec `kill` bên trong image của tác giả, rất mong manh. Kill chưa từng được đo trên cluster (spec kill §9), và WorkingDirectory trên Kubernetes cũng được hoãn. Sự khác biệt này được ghi vào spec kill.
- **D3. Hạn của container được giữ là một setting mới, `workflow.kept_container_retention_days`, mặc định 7, `0` nghĩa là giữ mãi.**
  - Setting theo `workflow.storage_retention_days` (`crates/egret-config/src/lib.rs:745`, mặc định 30, `0` = mãi mãi).
  - Đồng hồ bắt đầu từ `ended_at` của run (migration 0183), hoặc `updated_at` với dòng cũ hơn 0183.
  - Vì sao 7 ngày mà không phải 30 như log và file: container được giữ chiếm writable layer trên daemon, mà Docker runner không có giới hạn đĩa (`worker/commands.rs:2590-2597`, "No disk bound"). Một tuần vẫn đủ bao một cuối tuần giữa lúc kill và lúc có người kiểm tra.
  - Sweep chạy theo tick của reaper (`runtime.rs:638-659`, mỗi `reaper_interval_secs`, mặc định 60), nên container biến mất trong vòng một phút sau hạn.
  - Sweep của cả hai runner đều áp dụng hạn: của Docker (`worker/commands.rs:2514-2557`) và pod sweep (`worker/k8s/runner.rs:1485-1547`). Nó với tới mọi container được giữ mà run đã terminal (đã chốt với user, 2026-10-19).
- **D4. Kill không load được execution thì đóng execution đó cưỡng bức, thành `KILLED`.** Việc đóng cưỡng bức là §2.2. Executor của Kestra làm fail execution mà nó không xử lý được (`Execution.failedExecutionFromExecutor`, `core/src/main/java/io/kestra/core/models/executions/Execution.java:860-902`). Egret ghi `KILLED` thay vì `FAILED`, vì operator đã yêu cầu kill.
- **D5. `commit_kill` giữ lần retry duy nhất khi là nạn nhân deadlock.** Gói này thêm test mà retry chưa từng có (§2.3). Không đổi hành vi.
- **D6. Một sweep riêng chạy mỗi giờ xoá các snapshot `_wd/{block}/` mà block `WorkingDirectory` bị crash để lại**, 24 giờ sau khi run của block kết thúc (§3.1). Nó dùng migration **0197** (số trống tiếp theo: `ls migrations | tail -1` là `0196_workflow_execution_variables.sql`).
- **D7. Phần setup oracle của executor suite làm rỗng `workflow.flows` trước khi nạp corpus.** Setup đầu tiên hỏng thì từ đó không còn biến mọi test sau thành thông báo gây hiểu lầm "left 335" (§3.2).
- **D8. `executions.variables` bị chặn ở 1 MiB JSON đã serialize.** Đó là `runners::env::MAX_OUTPUT_BYTES` (`runners/env.rs:204`), giới hạn cho một lần render và cho các output marker của container (`worker/commands.rs:204`). Update vượt giới hạn thì làm run fail, với message riêng của egret.
- **D9. Label của task `Labels` phải qua các giới hạn theo-label của backfill**: key tối đa 128 ký tự và value tối đa 1.024 (`labels.rs:7-8`). Danh sách sau merge chứa tối đa 100 label: gấp đôi 50 của backfill (`labels.rs:6`), để label của flow và của backfill đều vừa.
- **D10. (M5) Các update được tạo trong cùng một lượt render thì render theo bề mặt (surface) như lúc lượt bắt đầu, và merge lên giá trị hiện tại.** Khớp với Kestra (§4.2). Câu "each one sees the result of the one before it" của spec C2 bị rút lại.
- **D11. (M6) Update lỗi vẫn chỉ là run fail thường.** Đường executor-exception của Kestra được ghi lại như một khác biệt đã biết, không sao chép (§4.3).
- **D12. (M7) Mỗi lần lặp `LoopUntil` cấp cho mỗi descendant được restart một task-run id mới**, vì Kestra tạo lại các task run (§4.4).
- **D13. (M8) `executionLabels` của Flow trigger được ghi lại như một khác biệt, không sửa** (§4.5).

- **D14. Driver của oracle chạy tiếp một run có hạn ngắn đã qua khi máy tải nặng.** Đây là một bản sửa trong driver của oracle (harness test), không phải code production, được thêm ngày 2026-10-19 sau cuộc điều tra ratchet flake (§3.2). Đó là PR-1 Task 1.6b.
- **D15. Container được giữ mà run id của nó không còn dòng nào, vì một lần lặp `LoopUntil` đã thay nó, được sweep khi execution của nó đã terminal.** Nó nhận thời hạn giữ của D3, tính từ lúc execution kết thúc, qua label `egret.io/execution-id` của container. Container `delete: true` không bị quy tắc này đụng tới. Một block `WorkingDirectory` bên trong `LoopUntil` được chuẩn bị lại ở mỗi lần lặp, và việc dọn `_wd` xoá các prefix của id cũ (§4.7). Thêm ngày 2026-10-19 trong lúc xây D12 (PR-2 Task 3b).

## 1. Mọi mục nợ, đã kiểm chứng

| # | Mục | Kiểm tại | Trạng thái |
|---|---|---|---|
| 1 | Kill xoá container `delete: false`; sweep không bao giờ xoá nó | `worker/commands.rs:1617-1643` (`cancel` kết thúc bằng `rm --force`), `:2563-2577` (`parse_docker_ps` bỏ `delete=false`), `worker/k8s/runner.rs:1503-1511` | mở, §2.1 |
| 2a | Retry deadlock của `commit_kill` không có test | retry ở `worker/claim.rs:2414-2427`; test deadlock duy nhất, `tests/kill_route.rs:735`, chạy retry của `run_one` (`claim.rs:1545-1600`), không phải của `commit_kill` | mở, §2.3 |
| 2b | Kill mà không load được execution thì lặp mãi | `claim::load` fail `Unplannable`/`Corrupt`/`UnknownState` (`worker/claim.rs:4078-4080`, `:4111-4116`); `commit_kill_with` gọi nó đầu tiên (`:2452-2453`); `finish_claimed_kill` chuyển lỗi do nội dung dòng sang `commit_kill` (`:1725-1736`), lại fail; run giữ lease đến khi `reap`, rồi lần claim kế tiếp lặp lại mỗi `working_timeout` (3600 s) mãi mãi. `kill_in` của route (`worker/kill.rs:147-172`) và `descendants` của cascade (`:191-215`) fail cùng cách, thành lỗi 500 | mở, §2.2 |
| 3 | VI §9 thiếu các ghi chú 2026-10-16/17 | VI có mọi ghi chú có ngày mà EN có: AllowFailure `docs/vi/...programme-design.md:521`, WorkingDirectory `:523-525`, kill `:527`, C1 `:529` | **lỗi thời (stale)**. Thay vào đó có hai khoảng trống thật: VI `:511` thiếu phần gạch ngang và "(retired 2026-10-17 …)" của EN `:523`; **cả** EN lẫn VI đều chưa có ghi chú C2 (EN `:541` vẫn ghi "Group C2 … is next") |
| 4 | Hai chỗ publish từ chối quá tay | `publish.rs:433-450` (Subflow) và `:532-558` (`dependsOn` của Flow trigger) từ chối `None` bằng `ref_not_found()` | mở, §5.1 (user: chấp nhận, 2026-10-19) |
| 5 | `{% %}` của `render_map` không có trong spec; schema README §1 | `models/property.rs:133-137,162-163` xử lý `{%`…`%}` như một biểu thức nguyên khối, mà build note của spec C1 (`docs/en/specs/2026-10-18-workflow-core-tasks-design.md:90-94`) không nói tới; `src/spec/schema/README.md:19,29-31` là một câu dài 90 từ; recipe vẫn nêu `w7a-kestra-t1` và `admin@w7a.local` (`:41-54`) | mở, §5.2 |
| 6a | Sweep `_wd` sau crash | §3.1 | mở |
| 6b | "flake 335" | §3.2 | đã tìm ra nguyên nhân bằng phân tích tĩnh; tái hiện trong PR-1 Task 6 |
| 6c | Checkbox plan WorkingDirectory chưa tick | `docs/en/plans/2026-10-16-workflow-working-directory.md`: 35 ô chưa tick; công việc đằng sau 32 ô đã có trong code (§3.3) | mở, chỉ tài liệu |
| 7a | Giới hạn kích thước | §4.1 | mở |
| 7b | M5 | §4.2 | mở, cho khớp |
| 7c | M6 | §4.3 | ghi lại |
| 7d | M7 | §4.4 | mở, cho khớp |
| 7e | Minor trong code | `executor/update.rs:94` clone variables cho mọi op, kể cả `Labels` (`:97` trong ledger đã dời chỗ); `executor/plan.rs:1365-1367` gọi `UpdateOp::of` hai lần kèm `expect`; test ở `spec/unrunnable.rs:144` tên là "publish" nhưng chạy `validate_for_run` + `unrunnable`; các khoảng trống test của `update.rs` nêu trong ledger C2 (Task 2) | mở, §4.6 |
| 7f | M8 | §4.5 | ghi lại |

## 2. Kill (PR-1)

### 2.1 Container `delete: false` bị kill thì được giữ, và sweep xoá sau

**Hiện nay:**
- `TaskRunner::cancel(task, grace)` (`worker/commands.rs:641`) không có cách nào biết `delete`. `cancel` của Docker kết thúc bằng `rm --force` (`:1631`). Kubernetes xoá pod (`worker/k8s/runner.rs:1417-1438`).
- `release` đọc `delete` (`worker/commands.rs:1648-1656`, `k8s/runner.rs:1442-1450`), nhưng đường kill gọi nó sau một cancel đã xoá sạch (`worker/claim.rs:2392`; ghi chú spec kill §4.2).
- Script handler parse lại spec của nó trong `release` (`worker/handlers/script.rs:786-797`) nhưng không làm vậy trong `cancel` (`:774-776`).
- Plugin handler luôn dùng `delete: true` (`worker/handlers/kestra.rs:802-822`; publish từ chối `taskRunner:` trên task plugin).

**Thay đổi:**
- `TaskRunner::cancel` nhận spec đã parse: `cancel(task, grace, spec: &RunnerSpec)`.
- `cancel` của script handler parse spec như `release` đang làm. Task không parse được, hoặc nêu runner khác, thì dùng mặc định của runner đã cấu hình (`delete: true`), như hiện nay.
- Plugin handler truyền `RunnerSpec::defaults(kind)`.
- Docker, `spec.delete == false`:
  - `docker stop -t <ceil(grace)>`, bị chặn bởi `grace + CANCEL_WAIT_MARGIN` như đường graceful (`worker/commands.rs:1617-1630`), và không `rm`.
  - Container không tồn tại là `Ok(())`.
  - Container đã dừng sẵn là `Ok(())`. Tin rằng `docker stop` thoát với 0 ở đó; đo trong PR-1 Task 1 trên daemon, như W5a đã đo `rm --force` (`:1636-1640`).
- Docker, `spec.delete == true`: không đổi.
- Kubernetes bỏ qua `spec.delete` trong `cancel` (D2), kèm doc comment nói rõ điều đó.
- `release` sau kill vốn đã giữ container `delete: false` (`worker/commands.rs:1649`), nên `release` của đường kill không cần đổi.

**Sweep:**
- `parse_docker_ps` trả về cờ keep thay vì bỏ các container được giữ.
- `run_liveness` (`worker/commands.rs:2456-2487`) cũng trả về `COALESCE(ended_at, updated_at)` của mỗi run đã terminal.
- Một hàm thuần, dùng chung cho cả hai sweep, quyết định:
  - chưa terminal: giữ;
  - terminal và không được giữ: xoá (quy tắc hiện tại);
  - terminal và được giữ: xoá khi `now - ended > retention`, trong đó `None` (setting `0`) nghĩa là không bao giờ.
- Pod sweep (`worker/k8s/runner.rs:1503-1511`) đọc annotation theo cùng cách. Pod mà run của nó không có dòng vẫn được để yên ở cả hai nửa (`:1530-1533`, `worker/commands.rs:2504-2511`).
- Retention tới các runner lúc khởi tạo, trong `runtime::handlers_for` (`runtime.rs:750-778`): `DockerRunner::with_kept_retention(Option<Duration>)` và tương tự trên `KubernetesRunner`.
- Config: `WorkflowConfig::kept_container_retention_days: u32`, mặc định 7, được tài liệu hoá như `storage_retention_days`, với env `EGRET_WORKFLOW__KEPT_CONTAINER_RETENTION_DAYS`. Nó không phải giá trị chart, theo quy tắc ở `deploy/helm/egret/templates/configmap-env.yaml:44-51`.

**Một hệ quả cần nêu:** run của body `LoopUntil` dùng lại task-run id và khởi động lại attempt từ 0 (§4.4), nên tên container `egret-run-{run}-{attempt}` (`worker/commands.rs:793-795`) lặp lại qua các lần lặp. `find` coi container đã thoát là `Started` (`:1717-1719`), nên với `delete: false`, lần lặp thứ hai có thể gắn lại vào container đã thoát của lần đầu thay vì chạy. Tin rằng điều này đang xảy ra hiện nay; đo trong PR-2 Task 3. D12 loại bỏ nó.

**(2026-10-19, đã đo và đã xây.)** Case Docker live `a_loop_until_body_with_delete_false_runs_a_new_container_each_iteration` (`cargo test -p egret-workflow --test container_docker_live`) cho thấy mỗi lần lặp có một container mới, đặt tên từ run id mới của lần lặp đó. D15 (§4.7) nói container được giữ của một lần lặp đã bị thay thì sau đó ra sao.

### 2.2 Đóng cưỡng bức (force-close)

**Khi nào:** `claim::load` fail với lỗi do nội dung dòng (`WorkerError::is_row_content`, `worker/mod.rs:822-830`: `Unplannable`, `NotALeaf`, `Corrupt`, `UnknownState`) tại một trong ba chỗ:
- `kill_in` của route (`worker/kill.rs:147-172`);
- `descendants` của cascade (`:191-215`), cho một descendant;
- `commit_kill_with` (`worker/claim.rs:2450-2453`).

Mọi lỗi khác vẫn là tạm thời, như `finish_claimed_kill` đã quy định (`claim.rs:1695-1699`).

**Nó ghi gì:** một hàm, `worker::force_close::close(conn, execution, reason, own: Option<(run_id, claimed_at)>)`. Đó là SQL thuần và không bao giờ lập plan từ source:
1. Nó khoá dòng execution (`FOR NO KEY UPDATE`, như `load`), rồi các dòng run của nó `FOR UPDATE` theo thứ tự `run_ix`, như `kill_loaded` (`worker/kill.rs:243-249`).
2. Run còn sống (`NOT workflow.state_is_terminal(state)`) mà một worker đang giữ thì được đặt thành `KILLING`, và giữ lease. "Đang giữ" nghĩa là `RUNNING`, `SUBMITTED` hoặc `KILLING` với `claimed_at IS NOT NULL`, và không phải run `own` của caller. Worker đó có input của handler, nên có thể cancel; `commit_kill` của nó sau đó lại tới hàm này với `own` được đặt.
3. Mọi run còn sống khác thành `KILLED`:
   - `KILLED` được nối vào `history`;
   - `ended_at = now()` và `claimed_at = NULL`;
   - `message = reason` khi chưa có message;
   - `history` của attempt cuối nhận `KILLED` khi trạng thái cuối của nó chưa terminal.

   Run đang chạy không có lease thì không bị cancel: không có gì dựng được handler của nó (quy tắc fix round 1, `claim.rs:1688-1694`). Orphan sweep sở hữu container của nó, nay kể cả container được giữ (§2.1). Cách đóng dấu `ended_at` của attempt (`stamps::stamp_attempt_ends` ghi một tick tương đối so với `created_at`) cần đo trong PR-1 Task 4.
4. Execution thành `KILLED`, kèm `ended_at`, khi không còn run nào `KILLING`. Ngược lại nó thành `KILLING`.
5. Execution `KILLED` báo cho cha qua cùng các lời gọi mà `settle` thực hiện (`spawn::on_execution_terminal`, `worker/spawn.rs:512`; `subflow::on_child_terminal`, `worker/subflow.rs:602`). Chúng load cha, không load execution này, nên một loop hoặc run `Subflow` đang chờ sẽ kết thúc.
6. Không ghi sự kiện outbox của Flow trigger, và không chạy `errors:` hay `finally:`: không có plan nào để chạy chúng. Message nói rõ điều này.

**Message**, lưu trên mỗi run mà nó đóng: `egret could not load this execution (<reason>), so it was killed without running its flow: no errors:, finally: or Flow trigger ran`. `<reason>` là text của `WorkerError`.

*Ghi chú, 2026-10-19 (như đã xây, PR-1 Task 4):*
- **Quy tắc 2 nay chỉ giữ leaf.** Một run chỉ được coi là đang giữ khi nó là leaf (một `Runnable` có handler) mà một worker đang lease. Một block `WorkingDirectory` có lease không được coi là đang giữ: không ai cancel nó (claim của nó không bao giờ bị kill), và để nó `KILLING` thì nó treo mãi, nên nó được đóng `KILLED` ngay và commit về sau của worker nó fail một lần ở bước load. Một block đã `KILLING` hoặc `FINISHING` mà claim discard-only thấy execution không load được thì bị chính claim đó force-close (`apply_block_claim_on`, test `a_discard_only_block_claim_of_an_unloadable_execution_closes_it`); lỗi ở mọi claim block khác vẫn được trả về như cũ. Test: `a_force_close_closes_a_leased_block_claim_at_once`, `a_force_close_leaves_a_held_leaf_to_its_worker`.
- **Quy tắc 5, báo cáo của task con bị force-close, được hoãn và có chốt.** Báo cáo là một job hoãn (`Job::Report`), nên cha chỉ được load khi job chạy; trong cascade, cha có thể là một hậu duệ mà kill chưa settle xong. Báo cáo **bỏ qua cha đã terminal** (đọc bằng SQL thường, không lấy lock của nó; test `a_report_to_a_terminal_parent_does_not_lock_it`), và **bị bỏ, kèm cảnh báo, khi việc load cha gặp lỗi nội dung row** (cha còn sống mà không load được sẽ bị chính kill của nó đóng; test `an_unloadable_iterations_report_to_an_unloadable_live_parent_is_dropped`). Mọi lỗi khác của báo cáo được trả về và làm kill fail (`a_report_that_cannot_lock_the_parent_still_fails_the_kill`). Chốt này bỏ lỗi nội dung row từ bất kỳ lời gọi nào của báo cáo, không chỉ từ việc load cha.
- **Chữ ký có thêm `caller` và kết quả có thêm `own`.** `close(conn, execution, reason, own, caller) -> Closed`, trong đó `Closed.own` cho biết run có fence của chính caller đã được đóng hay chưa. `commit_kill_with` trả `LeaseLost` khi chưa, nhưng transaction vẫn commit các run khác nó đã đóng, nên trên đường này `LeaseLost` không còn nghĩa là "chưa ghi gì".
- **`ended_at` của attempt đã được đo** (điểm mở của quy tắc 3): `stamps::stamp_attempt_ends` ghi một tick nguyên, tính bằng mili giây sau `created_at` của execution, như `started_at`; `close` ghi cùng khoá và cùng đơn vị. Khoá audit `"forced": true` chỉ được ghi khi transaction của route (hoặc một hậu duệ trong cascade) bị force-close.

*Ghi chú, 2026-10-19 (final review I3, sửa câu "Orphan sweep sở hữu container của nó, nay kể cả container được giữ" của quy tắc 3):* câu đó nói quá những gì sweep làm. Một container được giữ (`delete: false`) không được `cancel` trên đường force-close, cũng như khi `finish_claimed_kill` không dựng được handler, nên nó có thể vẫn chạy khi run của nó đã `KILLED`. Trước bản sửa này, sweep chỉ xoá nó sau thời hạn giữ, nên code của tác giả có thể chạy tới 7 ngày, hoặc mãi mãi với setting `0`. Như đã xây hiện nay:
- **Docker.** Danh sách của sweep đọc `{{.State}}`. Với một run đã terminal mà container được giữ vẫn đang chạy (`running`, `paused` hoặc `restarting`), sweep chạy `docker stop -t 0`, cùng kiểu dừng như D1, và giữ lại container. Việc xoá vẫn chờ thời hạn giữ (`commands::docker_sweep_action`). Grace là `0` vì ba lý do. Run đã terminal nên không có gì chờ một lần tắt êm. Đường giữ của D1 thực tế dùng grace 0, vì chỉ script task tôn trọng `delete`. Và sweep chạy trong tick của reaper, lần lượt từng container. Một lần dừng thất bại hoặc quá hạn được ghi log và thử lại ở tick sau. Test: `the_docker_sweep_stops_a_running_kept_container_of_a_terminal_run` (unit) và `a_force_closed_kept_container_is_stopped_by_the_sweep` (Docker thật).
- **Kubernetes: không đổi, và là một việc tiếp theo.** Pod không dừng được, chỉ xoá được, và kill thì xoá pod (D2). Vì vậy một pod được giữ của run bị force-close chỉ được dọn bởi pod sweep theo thời hạn giữ, và có thể chạy tới lúc đó. Việc tiếp theo là xoá pod đó, hoặc bằng cách khác chấm dứt tiến trình của nó, khi run đã terminal mà pod vẫn chạy.

**Audit:** bản ghi sẵn có của route (`routes.rs:3085-3096`) thêm `"forced": true` khi transaction của chính route đã đóng cưỡng bức. Force-close phía worker hoàn tất một kill mà route đã audit, nên chỉ ghi `tracing::warn!`.

**Kestra:** Kestra không có kill cho execution không load được. Executor của nó đáp ứng một exception bằng `failedExecutionFromExecutor` (ở trên), làm fail task run chưa xong cuối cùng hoặc execution. D4 giữ hình dạng đó và ghi `KILLED`.

### 2.3 Test deadlock của `commit_kill`

Test này sao chép `a_worker_that_loses_a_deadlock_to_a_kill_still_finishes_it` (`tests/kill_route.rs:735-830`), vốn bao `run_one`. Ở đây worker nằm trong claim chỉ-kill:
1. Run của một child subflow là `KILLING` và không có lease.
2. Transaction của test giữ dòng của cha, với `deadlock_timeout = 60s`.
3. `commit_kill` của worker khoá child rồi, qua `subflow::on_child_terminal`, chờ cha.
4. Transaction của test sau đó khoá child. Đó là chu trình, và worker là nạn nhân.
5. Retry (`worker/claim.rs:2419-2427`) chờ transaction của test, thấy fence vẫn là `KILLING` dưới lease của nó, và commit.

Kiểm tra mutation: bỏ nhánh retry thì tick trả `Err`.

## 3. WorkingDirectory (PR-1)

### 3.1 Sweep `_wd` sau crash

**Bố cục:**
- Snapshot của block run `b` nằm dưới `s3://{bucket}/{prefix}{ws}/executions/{exec}/_wd/{b}/` (`storage/mod.rs:232-242`).
- Các object là `prepare.tar` (`worker/working_dir.rs:43,280`) và một `{child run}.tar` cho mỗi child (`:133`).
- Thiết kế ghi `{seq}.tar` (`docs/en/specs/2026-10-16-workflow-working-directory-design.md:101`); đó là doc drift, sửa trong PR-1 Task 7.

**Xoá thông thường:** `working_dir::discard` (`worker/working_dir.rs:953-966`) là một `delete_prefix` best-effort. Nó chạy sau khi một finish claim commit (`worker/claim.rs:1829`), hoặc trước commit với finish chỉ-discard (`:1786`). Khi thất bại nó log "the pruner will sweep them".

**Dòng log đó là sai.** Pruner W9a chỉ xoá `execution_prefix`, tức `{ws}/{ns}/{flow}/executions/{exec}/` (`storage/prune.rs:512-531`, `storage/mod.rs:245-260`). `_wd` nằm dưới `{ws}/executions/`, không có segment namespace hay flow. Ứng viên của pruner cũng cần các dòng index mà object `_wd` không bao giờ tạo (`storage/prune.rs:411-438`). Vì vậy open point 2 của thiết kế ("removed by the pruner's retention") không đúng.

**Rò rỉ hiện nay:**
- crash giữa lúc ghi snapshot và lúc discard;
- mọi `delete_prefix` đã thất bại;
- block không bao giờ tới finish claim.

**Thiết kế (D6):**
- Migration 0197: `ALTER TABLE workflow.task_runs ADD COLUMN working_dir_swept_at timestamptz`, với `CHECK (working_dir_swept_at IS NULL OR working_dir IS NOT NULL)`, và một partial index trên `(ended_at) WHERE working_dir IS NOT NULL AND working_dir_swept_at IS NULL`.
  *Đổi số, 2026-10-19:* viết ban đầu là 0196; #694 lấy 0195 trước, nên C2 chuyển sang 0196 và migration này ship thành `0197_workflow_task_run_working_dir_swept.sql`.
- **Vì sao một cột:** không có dấu "xong", sweep phải liệt kê lại mọi block của 30 ngày gần nhất ở mỗi lượt. Có dấu, nó liệt kê mỗi block bị crash đúng một lần.
- `discard` đóng dấu `working_dir_swept_at = now()` sau một `delete_prefix` thành công. Lần thất bại để NULL.
- **Sweeper** (`storage::wd_sweep`, theo mẫu `storage/nsfiles/staging.rs:118-206`):
  - mỗi giờ, nó chọn các block run có `working_dir IS NOT NULL AND working_dir_swept_at IS NULL`, mà execution đã terminal (`workflow.state_is_terminal`) và `COALESCE(ended_at, updated_at)` cũ hơn 24 giờ (`orphan_age` của `StoragePruneConfig::new`, `storage/prune.rs:136-144`), 100 mỗi batch;
  - với mỗi block, nó gọi `delete_prefix(working_dir_prefix(ws, exec, run))`, rồi đóng dấu cột;
  - nó lấy một `pg_try_advisory_lock` riêng, để mỗi lúc chỉ một replica sweep.
- Nó được spawn trong `crates/egret-bin/src/main.rs:1928-1946` cạnh các sweeper staging và KV, trên mọi replica có store. Nó chạy bất kể `storage_retention_days` nói gì: đây là object tạm, không phải file của tác giả.
- Nó không phụ thuộc runner, nên bao cả Kubernetes khi WorkingDirectory tới đó.

*Ghi chú, 2026-10-19 (như đã xây, PR-1 Task 5):* câu chọn không đọc state của run, nên nó cũng quét snapshot của block mà force-close của kill đã đóng thành `KILLED` (§2.2): block như vậy giữ các object `_wd` của nó, và không có gì claim một block `KILLED` lần nữa. Test `the_wd_sweep_removes_a_force_closed_blocks_snapshots` khoá điều này, và thêm `t.state <> 'KILLED'` vào câu chọn thì test đỏ. Một lần discard xoá thành công tự đóng dấu `working_dir_swept_at` (`worker::working_dir::discard_snapshots`); xoá lỗi để nó NULL cho sweep. Chưa có restart tại chỗ của một execution (replay là một execution mới), nên dấu swept không có nơi đọc nào có thể làm sống lại một block đã quét; dòng 7 của roadmap (thao tác execution) phải xoá hoặc tôn trọng `working_dir_swept_at` khi thêm nó.

### 3.2 "Flake 335"

**Assertion:** `oracle-exec/driver.rs:341-348` khẳng định `workflow.flows` rỗng trước khi `publish_corpus` nạp corpus. `publish_corpus` là nơi duy nhất ghi `workflow.flows` trong binary `executor`.

**Caller:** `egret_side()` (`tests/executor.rs:1452-1477`) chạy nó bên trong `tokio::sync::OnceCell::get_or_init`, sau `TRUNCATE workflow.executions CASCADE`. Lệnh truncate đó không làm rỗng `workflow.flows`, vì `flows` không tham chiếu gì trong `executions`. Mười bốn test gọi `egret_side()`.

**Phép tính:**
- Một lượt qua corpus N file ra `N - PARSE_REFUSED (6) - CREATE_REFUSED (4)` dòng (`driver.rs:799,803`).
- Với 351 thì là 341. Với 345, corpus trước khi WorkingDirectory Task 8 thêm sáu flow, nó là **335**.
- Vậy "left 335" chính xác là một lượt hoàn tất qua corpus 345 flow.

**Nguyên nhân, theo phân tích tĩnh:**
- Nếu init đầu tiên nạp corpus rồi panic ở một census assert (`driver.rs:1135-1166`; `a7270435` nằm trong WorkingDirectory Task 8, nơi corpus và các hằng census đổi cùng nhau), `OnceCell` vẫn rỗng.
- `get_or_init` của tokio không lưu một init đã panic (cần xác nhận trong PR-1 Task 6).
- Nên mọi caller sau chạy lại init trên cùng bản clone theo-process (`crates/egret-test-db/src/lib.rs:352`), thấy 335 dòng và fail với message gây hiểu lầm.
- Cho ra 1 lỗi thật cộng tối đa 13 bản sao.
- Loại trừ chia sẻ giữa các process: mỗi process clone database `egret_t_{pid}` của riêng nó (`egret-test-db/src/lib.rs:105,209-235`).

**Sửa (D7):** bên trong guard `is_process_clone` sẵn có, `TRUNCATE workflow.flows CASCADE` trước `publish_corpus`. Init chạy lại sẽ fail ở cùng census assert như lần đầu, nên cả mười bốn lỗi đều mang nguyên nhân thật.

**Tái hiện (PR-1 Task 6):** một `LIVE - 1` tạm thời, không commit, trong `driver.rs`.
- Trước khi sửa: một lỗi census, và "left 341" ở mọi chỗ khác.
- Sau khi sửa: lỗi census ở mọi chỗ.

*Ghi chú, 2026-10-19 (như đã xây, PR-1 Task 6):* chốt là `TRUNCATE workflow.flows, workflow.executions CASCADE` trong `egret_side()`, trong một câu lệnh. Đo với `LIVE = 292` tạm thời: trước khi sửa 17 test fail, 1 ở census và 16 ở "workflow.flows must be empty" với `left: 341`; sau khi sửa 17 test fail, tất cả ở census. Ratchet sau đó đọc 301 golden entry, 73 mismatching, 73 baselined, 0 broken. Comment trong code giữ nguyên các con số lịch sử "335/345" (tại `a7270435`); hôm nay là 341 và 351.

#### D14: driver của oracle và hạn ngắn (PR-1 Task 1.6b)

**Nguyên nhân.** Cuộc điều tra flake (2026-10-19) thấy mismatch dư xuất hiện chập chờn không đến từ Task 1.1 đến 1.3 hay C2. Đó là một race trong driver của oracle, từ W3b (#576): khi hạn của một run ngắn, `pass_time` của driver có thể nhìn thấy nó sau khi hạn đã qua do máy tải, nên driver dừng ở một run đã đến hạn và để nó `PAUSED`.

**Bản sửa.** `driver::run` nay giao cho `run_with(.., before_pass_time: Duration)`, một seam có độ trễ (bằng không ở đường ratchet; không có env var). Khi `pass_time` trả `Ok(false)`, driver gọi `due_in_tree`, hàm duyệt cây của root qua `parent_execution_id` và hỏi `worker::due_retries(pool, i64::MAX)`, đúng câu chọn mà `wake` chạy. Có run đến hạn trong cây thì driver `continue`, để tick sau `wake` chạy tiếp nó; nếu không thì dừng. Ngưỡng bước sẵn có kết thúc một vòng quay vô ích bằng `Broken("did not settle within ..")`, nên một run đến hạn mà không bao giờ được chạy tiếp là một entry broken, không thể đưa vào baseline. Test ghim là `the_driver_resumes_a_run_whose_short_deadline_passed_before_pass_time` (độ trễ 300 ms trên `valids/waitfor-multiple-tasks`); đổi `Ok(true) => continue` thành `break` thì nó đỏ (`left: Some("PAUSED")`). Với độ trễ 300 ms trên mọi entry, cả ratchet đọc 73 mismatching và không có regression (trước khi sửa: 83 và 10 regression); dưới 12 tiến trình ngốn CPU, 4 lần chạy trong 4 đều xanh.

**Điều vẫn còn mở ("mode 2").** Khi máy tải rất nặng, `valids/waitfor-child-task-warning` vẫn có thể khớp một cách ngẫu nhiên, làm ratchet fail với "PROGRESS NOT BANKED". Thời gian thực rò vào đồng hồ Tick, nên `reached_maximums` (`executor/flowable/loop_until.rs`) kích hoạt `maxDuration` sớm một vòng lặp. Cách sửa đúng là một đồng hồ ảo trong driver. Entry vẫn nằm trong baseline, kèm comment trong `oracle-exec/baseline.txt`, và không được đưa vào từ một lần chạy như vậy. Việc duyệt cây chỉ đi theo `parent_execution_id` (như `pass_time`), nên một run đến hạn trong task con chỉ nối bằng cột riêng của W6 nằm ngoài cả hai.

**Số liệu ratchet của PR-1:** 301 golden entry, 73 mismatching, 73 baselined, 0 broken, với census LIVE 293 (351 flow corpus: 293 live, 48 draft-only, 6 unparseable, 4 name-contested). Nguồn: `cargo test -p egret-workflow --test executor`. Test `executor_matches_golden` của nó in ra `oracle-exec ratchet: 301 golden entries, 73 mismatching, 73 baselined, 0 broken`, và census của nó assert `LIVE = 293` (`oracle-exec/driver.rs`). Các số này giống hệt baseline trước PR-1. Test chốt của D14 là `the_driver_resumes_a_run_whose_short_deadline_passed_before_pass_time`.

### 3.3 Checkbox của plan

`docs/en/plans/2026-10-16-workflow-working-directory.md` có 35 ô chưa tick:
- Task 0: 3; Task 1 Step 5: 1; Task 3: 4; Task 4: 3; Task 5: 4; Task 6: 4; Task 7: 3; Task 8: 5; Task 9: 6; Task 10: 2.

Công việc đằng sau 32 ô đã có trong code:
- Task 3: `executor/plan.rs:393`.
- Task 4: `worker/snapshot.rs`.
- Task 5: `executor/flowable/working_directory.rs`.
- Task 6-7: `worker/working_dir.rs:1113-1353`.
- Task 8: sáu flow `oracle-exec/corpus/valids/working-directory-real-*`, census 351 tại `oracle-exec/driver.rs:1135`.
- Task 9: lần chạy thật được ghi trong spec WorkingDirectory §7.
- Task 10: programme EN `:535`, và các file VI đã có.

Ba ô vẫn chưa tick, kèm ghi chú có ngày "deferred, Kubernetes": câu hỏi Kubernetes của Task 0 và Task 1 Step 5. Plan VI (27 ô chưa tick) phản chiếu plan EN.

## 4. Follow-up của C2 (PR-2)

### 4.1 Giới hạn kích thước (D8, D9)

**Các giới hạn đã đo:**
- `MAX_OUTPUT_BYTES` 1 MiB cho mỗi lần render (`runners/env.rs:204`).
- `MAX_VARS_BYTES` 1 MiB cho output marker của container (`worker/commands.rs:204`).
- Body webhook 256 KiB (`webhook_routes.rs:254`).
- Body start JSON 2 MiB (`routes.rs:2360`).
- Label của backfill: 50 label, key 128 ký tự, value 1.024 ký tự (`labels.rs:6-8`), chỉ được kiểm bởi `backfill_labels_refusal` (`labels.rs:37-75`).

Không có gì chặn `executions.variables` sau merge hay các label mà task `Labels` thêm (C2 final review M3).

**Thay đổi:** trong `executor/update.rs::compute`:
- **`Set`:** kết quả đã serialize tối đa `MAX_VARIABLES_BYTES = runners::env::MAX_OUTPUT_BYTES`. Nếu không, run fail với `` `variables`: the execution's variables would be <n> bytes, over egret's limit of 1048576 ``. `Unset` chỉ làm nhỏ đi, nên không kiểm.
- **`Labels`:**
  - mỗi label mà task đặt phải qua giới hạn độ dài key và value, với message của backfill;
  - danh sách sau merge chứa tối đa `MAX_EXECUTION_LABELS = 100` label, với `` `labels`: the execution would carry <n> labels, over egret's limit of 100 ``;
  - hai kiểm tra của Kestra vẫn chạy trước, nên message golden không đổi.

Đây là giới hạn riêng của egret; Kestra không có. Không flow golden nào tới gần chúng; plan xác nhận điều này bằng ratchet.

### 4.2 M5: render theo lúc bắt đầu của lượt (D10)

**Kestra, đã đo:**
- `handleWorkerTasks` dựng một `RunContext` cho mọi task run `CREATED` từ execution như lúc chu kỳ bắt đầu (`executor/src/main/java/io/kestra/executor/ExecutorService.java:1098-1107`). `RunContextFactory` đóng băng variables ở thời điểm đó (`core/.../runners/RunContextFactory.java:152-168`).
- `handleExecutionUpdatingTasks` sau đó gọi từng `update(executor.getExecution(), runContext)` với execution sống, đã được cập nhật (`ExecutorService.java:1390-1406`).
- `SetVariables.update` render bằng context đã đóng băng, kiểm `overwrite` với variables sống, và merge lên chúng (`core/.../plugin/core/execution/SetVariables.java:69-88`).
- Vậy trong Kestra, update thứ hai render cái mà update thứ nhất đã thấy và merge lên cái mà update thứ nhất đã ghi.

**Egret hiện nay:** `render_updates` dựng từng context sau khi update trước đã đổi surface (`worker/render.rs:1505-1528`, doc `:1459-1460`). `tests/worker.rs:11538` ghim `y = "1"`.

**Thay đổi:** dựng context của mọi run đang chờ trước, rồi tính theo thứ tự run với `surface.vars()` và `current_labels()` sống.

**Test:**
- Flow thêm `variables: {x: 0}`, vì Kestra làm fail `two` nếu thiếu nó: `vars` là undefined ở đó (cần đo: hành vi của renderer egret trên `vars` undefined, PR-2 Task 2).
- Nó kỳ vọng `{"x": 2, "y": "0"}`.
- Không flow corpus nào đặt update trong `Parallel`, nên ratchet không thể dịch chuyển.

Gạch đầu dòng "Each one sees the result of the one before it" của spec C2 §3.1 được sửa.

**(2026-10-19, đã xây.)**
- Test là `two_updates_in_one_parallel_render_the_start_and_merge_in_run_order` (`cargo test -p egret-workflow --test worker`). Nó đỏ với code cũ (`y` là `"1"`) và nay xanh.
- **Đã đo (chỗ "cần đo" ở trên):** `{{ vars.x }}` khi không có `vars` thì fail trong egret, như trong Kestra. Renderer trả `undefined value` với key `vars` vắng, với `vars: null` và với `vars: {}`. Nó không bao giờ render ra rỗng, vì renderer chặt với giá trị undefined.
- Vậy **không có khác biệt M5** nào để liệt kê. Thay đổi duy nhất là quy tắc theo thứ tự run của D10.

### 4.3 M6: update lỗi (D11, ghi lại)

**Catch của Kestra:**
- Nó làm fail task run của update và lưu exception (`ExecutorService.java:1425-1432`).
- `DefaultExecutor` sau đó gọi `handleFailedExecutionFromExecutor` (`executor/src/main/java/io/kestra/executor/ExecutorService.java:247-252`), chạy `failedExecutionFromExecutor`.
- Hàm đó ép **task run chưa xong cuối cùng** thành `FAILED`, hoặc execution khi không có task run nào (`Execution.java:860-902`).

**Hệ quả trong Kestra:**
- Update lỗi ở cấp root làm fail thẳng execution, nên `errors:` của flow không chạy.
- Update lỗi lồng bên trong có thể ép fail một sibling vô tội, hoặc `AllowFailure` bao quanh.

M6 của C2 final review và phép đo này đồng ý rằng đây là đường dự phòng cho một exception, không phải hành vi được thiết kế.

**Egret:** `FailUnstarted` chỉ làm fail run của update (`worker/render.rs:1531-1541`). `errors:` và `AllowFailure` sau đó áp dụng như với mọi lỗi khác.

**Golden:** các hình dạng duy nhất được phủ là `valids/set-variables-duplicate` và `valids/labels-update-task-empty`, cả hai ở cấp cao nhất, không có `errors:`, và hai hệ thống khớp nhau ở đó. Không flow corpus nào kết hợp update với `errors:` hay `AllowFailure`.

**Quyết định:** ghi vào danh sách khác biệt của spec C2; không đổi.

### 4.4 M7: task-run id mới cho mỗi lần lặp `LoopUntil` (D12)

**Kestra:**
- `ExecutionService.retryWaitFor` xoá mọi task run descendant của loop (`core/src/main/java/io/kestra/core/services/ExecutionService.java:180-208`).
- `LoopUntil.resolveNexts` tạo lại chúng qua `TaskRun.of`, mỗi cái với `IdUtils.create()` (`core/.../models/executions/TaskRun.java:210-213`).

**Egret:**
- `Command::RestartLoop` làm rỗng chính các dòng đó tại chỗ (`executor/state.rs:986-1002`).
- `Surface::reserve_id` trả về id đã load (`worker/render.rs:678-685`).
- Nên `{{ taskrun.id }}` lặp lại qua các lần lặp (`render.rs:845-847`).
- Mọi khoá dẫn xuất từ nó cũng vậy:
  - `task_run_id` của các dòng log (migration 0159, `routes.rs:1979-2109` lọc theo nó);
  - URI file của execution `tasks/{task}/{run}/{file}` (`storage/uri.rs:296,345`), mà dòng index được upsert, nên lần lặp 2 ghi đè lần lặp 1 (`storage/mod.rs:469-472`);
  - và tên container ở §2.1.

**Thay đổi:**
- Khi `RestartLoop` của một settle làm rỗng một run, `persist` ghi cho nó một `id` mới. Không khoá ngoại nào tham chiếu `task_runs.id`: migration 0159 và 0166 nói vậy, `:25` và `:24`.
- Surface quên id cũ, nên `reserve_id` kế tiếp là id mới.
- Điểm bên trong `claim::settle`/`persist` nơi có cả trước và sau cần tìm trong PR-2 Task 3.

**Test:** một body `WaitFor` `SetVariables {ids: ["{{ taskrun.id }}"]}` qua ba lần lặp. `deep_merge` chỉ nối các phần tử list mới (`executor/update.rs:169-199`), nên kết quả chứa ~~3~~ **4** id sau thay đổi, và 1 trước đó.

**(2026-10-19, đính chính.)** Đoạn trên ghi 3 id; thế là sai. `maxIterations: 3` chạy body bốn lần: lượt đầu cộng ba lần restart. Golden của Kestra cho `valids/waitfor-max-iterations` (`maxIterations: 3`) cho run `WaitFor` đi qua `RUNNING` bốn lần với ba `PAUSED` ở giữa, và `LoopUntil.java:142` đổi id ở mỗi lần restart. Test đã xây `each_loop_until_iteration_has_its_own_task_run_id` (`cargo test -p egret-workflow --test worker`) đọc số lần lặp từ lịch sử của chính loop, assert nó là 4, rồi assert mỗi lần lặp một id riêng và dòng giữ id cuối.

**(2026-10-19, đã xây.)** Việc đổi id nằm trong `claim::drive`, không phải `persist`: `drive` áp dụng `RestartLoop` rồi `renew_restarted_ids` cấp id mới cho mọi run từng có lịch sử mà nay không còn (surface quên id cũ, `Surface::forget_id`). `write_render_side` ghi id mới vào các dòng. Không thể chờ `persist`, vì body `SetVariables` được dựng lại và render ngay trong settle đã restart nó, và lần render đó phải thấy id mới.

### 4.5 M8: label của Flow trigger (D13, ghi lại)

**Kestra:** trigger được đánh giá trên đối tượng `Execution` cuối chu kỳ, chỉ hoán đổi state theo từng transition (`executor/src/main/java/io/kestra/executor/DefaultExecutor.java:719-726`). Đó không phải đọc mới.

**Egret:** `flow_trigger/process.rs:779-805` đọc dòng hiện tại khi xử lý sự kiện. Do đó một task `Labels` ở settle sau có thể đổi cái mà trigger của sự kiện trước thấy.

**Một bản sửa** cần snapshot label trên `workflow.flow_trigger_events` (migration 0157 `:38-50`), được ghi trong settle (`worker/claim.rs:3398-3412`). Nhưng `vars` và `outputs` cũng cũ y như vậy, và snapshot chỉ-label sẽ làm quy tắc đúng một nửa.

**Quyết định:** ghi như một khác biệt trong spec C2, và nêu tên ở dòng renderer của roadmap parity. Không flow golden nào phủ nó.

### 4.6 Minor trong code

- `executor/update.rs:94`: chỉ tính `current` trong các nhánh `Set` và `Unset`.
- `executor/plan.rs:1365-1367`: một nhánh `let Some(op) = UpdateOp::of(..)`, không `expect`.
- `spec/unrunnable.rs:144`: đổi tên test thành `the_executor_level_types_pass_the_serve_gate_on_a_deployment_with_no_handlers`.
- Các test `update.rs` mới, mỗi test ghim một message hoặc một quy tắc:
  - biểu thức nguyên khối null của D8;
  - `deep_merge` với key vắng và value null;
  - `Unset` trên variables null;
  - các message `labels[i]` và form;
  - `set_variables_without_variables_fails` assert toàn bộ message.
- `tests/migration_0196_variables.rs` giữ seed riêng. Dùng chung `seed()` của `worker.rs` sẽ làm test migration phụ thuộc vào một file test của worker; điều này được ghi lại và không đổi.

### 4.7 Công việc được giữ của một lần lặp `LoopUntil` đã bị thay (D15)

D12 cấp cho run của body mỗi lần lặp id mới, nên run id của lần lặp trước không còn dòng nào. Hai thứ treo vào một id như vậy. Cả hai được phát hiện khi xây D12 (PR-2 Task 3b).

**Container và pod được giữ.**
- Mọi container và pod đã mang sẵn label `egret.io/execution-id` (Docker `labels()` trong `worker/commands.rs`, pod trong `pod_spec.rs`), nên không cần label hay migration mới. Docker sweep đọc nó như thêm một cột của `docker ps`, và pod sweep đọc label của pod. Label vắng hoặc không phải UUID coi như không có, và container được để yên như trước.
- `run_liveness` nhận các cặp `(run, execution)`. Với các run không còn dòng, nó chạy thêm một query chỉ-đọc, bị chặn bởi các id đã liệt kê: execution nào trong số đó đã terminal (`workflow.state_is_terminal`), và kết thúc lúc nào (`COALESCE(ended_at, updated_at)`, theo đồng hồ của database).
- **Quy tắc (D15):** container được giữ (`delete: false`) mà run của nó không còn dòng, và execution của nó đã terminal, được coi như container của một run terminal (D3). Nó bị xoá khi execution đã kết thúc quá `workflow.kept_container_retention_days` ngày, và không bao giờ khi đặt `0`. Docker sweep cũng dừng container như vậy nếu nó vẫn đang chạy, như với run terminal. Trường hợp đó bình thường không xảy ra, vì body của lần lặp đã kết thúc trước lúc restart.
- **Vẫn để yên, như trước:**
  - container `delete: true` của một run không còn dòng. Thực tế không có cái nào, vì release hoặc cancel đã xoá nó;
  - container mà run không còn dòng và execution của nó không có trong database này hoặc còn sống. Gồm cả container của egret khác trên cùng daemon dùng chung.
- **Test:**
  - unit test `a_superseded_runs_kept_work_gets_the_retention_deadline` (cũng ghim trường hợp `delete: true`) và `the_docker_sweep_parses_the_execution_label` (`cargo test -p egret-workflow --lib -- worker::commands`);
  - case Docker live `a_loop_until_body_with_delete_false_runs_a_new_container_each_iteration` (`--test container_docker_live`): sau một `LoopUntil` hai lần lặp, sweep với hạn một giờ xoá 0 container và còn 2, còn sweep với hạn không xoá 2.
- **Kubernetes chỉ được phủ bằng unit test.** Pod sweep đi qua cùng `sweep_decision`, nhưng không có cụm nào để chạy, và guard riêng của đường pod cần một `kube::Client` nên không có seam để unit test.

**Block `WorkingDirectory` bên trong `LoopUntil`.**
- Trước thay đổi này block giữ nguyên phase qua restart, nên nó không được chuẩn bị lại cho lần lặp kế tiếp. Nay `Command::RestartLoop` xoá phase và block được chuẩn bị lại ở mỗi lần lặp. `persist` cũng xoá `working_dir_snapshot` khi phase bị xoá, và `working_dir_swept_at` khi phase thành `PREPARING`, để block được restart không mang theo con trỏ snapshot hay dấu sweep của lần lặp trước (các CHECK của migration 0194 và 0197 áp dụng).
- **Các prefix cũ.** Run của block có id mới ở mỗi lần lặp, nên prefix `_wd/{id cũ}/` của lần lặp trước sẽ mồ côi. `wd_sweep::delete_superseded` liệt kê `executions/{ex}/_wd/` và xoá mọi prefix mà block id không có dòng `task_runs` nào của execution đó giữ. Nó chạy sau khi lệnh xoá của chính một lần discard thành công, và từ `_wd` sweep cho mỗi block mà sweep xoá. Nếu nó fail thì block không được đánh dấu, nên sweep sẽ quay lại. Vậy lần discard hoặc sweep cuối của một execution xoá mọi prefix của các lần lặp trước.
- **Vì sao an toàn:** prefix có id mà một dòng đang giữ thì thuộc dòng đó, sống hay không. Prefix không có dòng thì đã xong, vì `RestartLoop` chỉ đổi tên dòng của block sau khi block đã kết thúc.
- **Đã loại:** đặt khoá dấu sweep theo `(execution_id, run_ix)`, và prefix dùng chung theo `run_ix`. Khi đó một lần discard muộn của lần lặp 1 sẽ tác động lên dòng hay object của lần lặp 2 trong lúc lần lặp 2 đang chuẩn bị.
- **Test** (`cargo test -p egret-workflow --test working_dir_claims`): `a_block_inside_a_loop_until_is_prepared_again_each_iteration`, `a_block_inside_a_waiting_loop_is_unstamped_when_it_restarts`, `a_block_after_another_task_in_a_loop_restarts_with_no_snapshot`, `a_later_discard_removes_a_superseded_iterations_snapshots`, `the_wd_sweep_removes_a_superseded_iterations_snapshots`. Case live `a_block_inside_a_wait_for_runs_its_docker_child_each_iteration` (`--test working_dir_docker_live`) chạy một child sẽ fail nếu lần lặp 2 dùng lại thư mục của lần lặp 1.
- **Một lỗ hổng còn lại, được ghi và chưa đóng.** Nếu lệnh xoá của discard một lần lặp trước bị fail, và sau đó không còn discard hay `_wd` sweep nào chạy trong execution đó, `_wd/{id cũ}/` vẫn còn. Hai trường hợp dẫn tới đó: block không bao giờ được chạm tới nữa và execution không có block nào khác; hoặc lần restart đã xoá phase và snapshot, nên sweep không còn chọn dòng đó. Đóng nó cần một bản ghi bền của prefix đã bị thay, tức một migration. Gói này không thêm migration nào.

## 5. Follow-up của C1 và tài liệu (PR-2)

### 5.1 Hai chỗ publish từ chối quá tay

**Đã đo:**
- `FlowService.warnings` của Kestra biến target `Subflow` bị thiếu thành một **warning** (`core/src/main/java/io/kestra/core/services/FlowService.java:546-551,619-648`): "The subflow '<id>' not found in namespace '<ns>'.".
- Nó không nói gì về `dependsOn` của Flow trigger (`:556-568`).
- Cả hai flow corpus đều được publish với 200 (`oracle-exec/README.md:13453-13464`).

**Egret từ chối cả hai bằng `ref_not_found()`** (`publish.rs:94-105,433-450,532-558`). Việc từ chối tồn tại để publish không dò được những gì workspace khác đang giữ: đối tượng của workspace khác và đối tượng không tồn tại trả lời giống nhau.

Phép tra cứu đằng sau nó, `FlowRepo::find_by_name` (`repo.rs:892-946`), không bị giới hạn theo workspace. Doc của chính nó ghi rằng một tên mà hai workspace cùng có có thể phân giải sai và bị từ chối như thuộc workspace khác (`repo.rs:901-922`).

**Runtime không dùng id đã đóng băng:** `subflow::resolve` tra target theo tên trong workspace của execution (`worker/subflow.rs:1100-1117`).

**Đề xuất:**
- Publish phân giải các cặp `Subflow` và `dependsOn` **chỉ trong workspace của caller**, qua một phép tra `FlowRepo` có phạm vi workspace, không dùng port không-phạm-vi.
- Tìm thấy: id được đóng băng, như hiện nay.
- Vắng: flow vẫn được publish, `flow_ref` vẫn là `None`, và `Subflow` nhận warning của Kestra trong `warnings` của snapshot. `dependsOn` không nhận gì, như Kestra.
- Câu trả lời không còn phụ thuộc vào workspace nào khác, nên vẫn không có chuyện dò. Nó cũng đóng lỗi từ chối quá tay xuyên workspace của `find_by_name`.
- Run của một `Subflow` mà target vẫn vắng thì fail như hiện nay, tại `subflow::resolve`.
- Điều này viết lại bốn test bảo mật trong `tests/publish.rs` (`:62`, `:95`, `:279`, `:308`): "foreign" thành "absent from your workspace, published with the warning, and the run fails not found". User đồng ý ngày 2026-10-19.
- `VALIDATE_REFUSED` và `LIVE` của oracle census dịch chuyển theo hai entry. Cần đo trong PR-2 Task 5; `baseline.txt` không đổi, vì executor oracle chạy `build_plan`, không chạy publish (`oracle-exec/README.md:13459`).

**(2026-10-19, đã xây.)** Như đề xuất, với các chi tiết sau:
- `FlowRepo::find_in_workspace(ws, namespace, flow_id)` là phép tra có phạm vi. Publish dùng nó cho `Subflow` và cho `dependsOn` của Flow trigger, và chỉ trong workspace của caller. Flow mà chỉ workspace khác có thì được trả lời y như flow vắng.
- Target `Subflow` vắng thì publish kèm warning `subflow_not_found`, message `The subflow '<flowId>' not found in namespace '<namespace>'.`, tại path của task. Target `dependsOn` vắng thì publish không kèm warning, như Kestra.
- Publish luôn ghi đè `flow_ref` bằng câu trả lời của phép tra, nên target vắng xoá `flowRef` mà tác giả viết trong draft.
- Publish cũng đi qua các list `errors:` lồng nhau. Một `Subflow` ở đó nhận cùng phép tra, warning và `flowRef` bị xoá, còn `database:` ở đó được kiểm như `database:` ở cấp cao nhất. (Task type không biết hoặc không được phục vụ ở đó vốn đã bị validate từ chối.)
- Phép tra không-phạm-vi đã bị bỏ: `RefKind::Subflow` và `FlowRepo::find_by_name` đã bị xoá, nên lỗi từ chối quá tay xuyên workspace không thể quay lại.
- **Census, đã đo** (`cargo test -p egret-workflow --test executor`, trên corpus 351 flow): `LIVE` 293 lên **295** và `VALIDATE_REFUSED` 27 xuống **25**. Hai flow là `valids/task-runif` và `valids/trigger-flow-listener-no-inputs`. Số phép tra subflow không-phạm-vi mà publish thực hiện trong census giảm về 0, và nay chúng canh để publish không quay lại port không-phạm-vi.
- **Ratchet không đổi:** 301 golden entry, 73 mismatching, 73 baselined, 0 broken. `baseline.txt` và `golden.json` không bị sửa.
- **Test:** trong `tests/publish.rs`, `a_subflow_absent_from_the_callers_workspace_publishes_with_the_warning_whoever_has_it`, `a_trigger_dependency_absent_from_the_callers_workspace_publishes_whoever_has_it`, `an_authored_flow_ref_does_not_survive_an_absent_target`, và cho `errors:` lồng nhau `a_subflow_inside_a_nested_errors_list_is_resolved_warned_and_its_authored_flow_ref_cleared` và `a_foreign_database_inside_a_nested_errors_list_cannot_be_published` (`cargo test -p egret-workflow --test publish`); trong `tests/worker.rs`, `a_published_flow_whose_subflow_target_is_absent_runs_and_the_subflow_fails_not_found` (`--test worker -- subflow`), chạy flow đã publish và thấy child fail với `unable to find flow`.

### 5.2 Sửa tài liệu nhỏ

- **Build note của spec C1** (EN + VI):
  - `render_map` render một chuỗi như một biểu thức nguyên khối khi, sau khi trim, nó bắt đầu bằng `{{` hoặc `{%` và kết thúc bằng `}}` hoặc `%}` (`models/property.rs:162-163`; các kiểm tra delimiter `PebbleUtil` của Kestra). Nên `"{% if … %}{…}{% endif %}"` được đọc như một object JSON sau khi render.
  - Mọi chuỗi khác được parse như JSON trước, rồi duyệt.
- **`src/spec/schema/README.md` §1:**
  - Tách câu 90 từ ở `:29-31` thành ba câu.
  - Đổi tên container và user của recipe từ `w7a-kestra-t1`/`admin@w7a.local` thành `kestra-schema-export`/`admin@egret.local`. Không đổi gì khác, vì image ID và các lệnh là thứ đã được đo.
- **Spec chương trình §9:**
  - VI `:511` thêm phần gạch ngang của EN `:523` và ghi chú "(retired 2026-10-17 …)" của nó.
  - EN và VI thêm một ghi chú C2 có ngày (các số nằm trong Status của spec C2: ratchet 84 → 73, LIVE 281 → 293) và, khi mỗi PR merge, một ghi chú cho gói này.
- **Spec kill §4.2** (EN + VI): ghi chú "open product question" được đóng bằng phán quyết của user, và D1, D2 được viết vào đó.
- **Spec WorkingDirectory** (EN + VI): sửa open point 2 và `:101`/`:111` (pruner không với tới `_wd`; sweep ở §3.1 thì có); `{seq}.tar` thành `prepare.tar` và `{child run}.tar`.
- **Spec C2** (EN + VI): D10, M6, M7 và M8 vào danh sách khác biệt; câu ở §3.1 được sửa.

## 6. Đóng gói

Hai PR. Mỗi PR có thể review riêng, và chúng chạm các file khác nhau trừ tài liệu:
- **PR-1, kill và WorkingDirectory:** §2 và §3, với migration 0197 và config key mới.
  - Các case Docker live: `a_killed_container_is_stopped_by_the_cancel_even_when_delete_is_false` được lật lại, và một case mới giữ-rồi-sweep.
- **PR-2, C2, C1 và tài liệu:** §4 và §5. Không có migration.
  - §5.1 được xây (user trả lời có ngày 2026-10-19).

Chúng có thể chạy song song theo quy tắc autonomy: mỗi bên một worktree, Postgres và thư mục target. PR-2 rebase lên phần tài liệu của PR-1.

**Test và nghiệm thu, cả hai PR:**
- TDD theo từng task; unit test ở chỗ logic thuần (quyết định của sweep, các giới hạn, `compute`).
- Test DB trên Postgres riêng.
- Case Docker live trên daemon riêng, không bao giờ trên dev stack dùng chung.
- Ratchet oracle (`cargo test -p egret-workflow --test executor`, 301 golden, 73 mismatching / 73 baselined, 0 broken, LIVE 293) giữ xanh sau mỗi task. Chỉ §5.1 được kỳ vọng làm dịch chuyển số census (LIVE 293 lên 295 khi §5.1 được xây; đã đo ở §5.1).
- Gate cuối là của plan C2, cộng với bộ Docker live cho PR-1.

## 7. Câu hỏi mở

Không có. Cả hai câu hỏi của bản nháp đầu đã được user trả lời ngày 2026-10-19 (xem "Đã chốt với user").
