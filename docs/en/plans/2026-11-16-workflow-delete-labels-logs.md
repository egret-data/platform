# Workflow package 7d: delete an execution, edit its labels, download and delete its logs — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a person with `workflow:manage` can delete an ended root execution (with its loop iterations, retry replacements, logs, index rows and, by default, its stored files), replace its visible labels (a flow label can be removed, `system.*` labels are kept), and delete its log lines by filter; a person with `workflow:read` can download its log as one `text/plain` file from the server. The semantics are Kestra's (`EC:478-494`, `ES:747-767`, `EC:2352-2402`, `LC:114-137`, `LC:192-201`), except where spec §5 and §"Spec corrections" below record a divergence.

**Architecture:**
- Migration `0231_execution_labels_replaced.sql` adds `workflow.executions.labels_replaced boolean NOT NULL DEFAULT false` (S1: 0230 is taken).
- `labels.rs` gains three pure helpers: `visible_labels` (the one list every reader shows and renders), `edit_labels_refusal` (D5's rules) and `merge_system_labels` (Kestra's `mergeSystemLabels`).
- Every reader of `workflow.executions.labels` reads `labels_replaced` beside it and calls `visible_labels`: the detail DTO, the render surface, the Flow-trigger source. Every INSERT that copies `labels` copies `labels_replaced` too (S6).
- Two worker modules, both one Postgres transaction holding the execution's row lock:
  - `worker/delete.rs` `delete_execution`: root check, family walk (every `parent_execution_id` descendant), family lock, idle, Flow-trigger outbox (S9), Flow-trigger fires detached (S8), dependents, objects (each member's `own_prefix` and `_wd/` root, S4), then one `DELETE` whose cascade takes the rest. A store failure rolls back.
  - `worker/labels.rs` `set_execution_labels`: idle, validation against the visible list, the merged list written with `labels_replaced = true`.
- `logs/repo.rs` gains `LogFilter`, `delete_for_execution` and `download_batch`; `logs::download_line` formats one line. The route streams the download in id-ordered batches (S11).
- Routes: `DELETE …/executions/{id}`, `POST …/executions/{id}/labels`, `GET …/executions/{id}/logs/download`, `DELETE …/executions/{id}/logs`; codes; a detailed error body for `has_dependents`/`not_a_root` (S3); audit; OpenAPI; `ExecutionDto.idle` (S7).
- Frontend: regenerated client; header **Edit labels** and **Delete** with `EditLabelsDialog` and `DeleteExecutionDialog`; the Logs tab's Download becomes a link to the server route and gains **Delete logs**; the task-run row menu gains **Delete logs**.

**Tech Stack:** Rust (axum, sqlx 0.9, Postgres, utoipa) in `crates/egret-workflow`; React 18 + TypeScript + orval client + vitest in `frontend/`.

Spec: `docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md`. Its Q1–Q8 and D1–D13 are binding. Where this plan finds a spec premise wrong, it says so under "Spec corrections" and does **not** change the decision silently. S8 and S9 changed user-visible behaviour and were put to the user, who **decided them on 2026-11-16** (S8: detach Flow-trigger fires; S9: refuse while the outbox holds an event). Every correction S1–S15, with those answers, is **folded into the spec** as §2.4 "Plan corrections, 2026-11-16" and into the Q2/D1–D9 text it changes (English and Vietnamese copies; to be committed by the controller before T1 as `docs(workflow): fold the plan's spec corrections into package 7d`). Branch `feat/workflow-delete-labels-logs`, one PR, base `origin/main` `c605a2b05` (spec commits `8bbc1f101`, `4b7cfd097`).

Tasks: T0 (baseline), T1–T6 (engine and API), T7 (client), T8 (frontend), T9 (docs in this repo and the gates). The user docs repo and the live check are the controller's (§"After the tasks").

**Review is not a task of this plan.** The controller reviews each task between tasks and runs one whole-branch review after T9. Fixes are recorded in spec §7 "As built".

---

## Spec corrections

Each item says where the spec's premise is wrong or incomplete, gives the evidence, and names the narrowest fix. The plan implements the line marked **Plan**. Items marked **DECIDED** were put to the user on 2026-11-16; the others inform. All are folded into spec §2.4.

- **S1 (§3: "Migration `0230_execution_labels_replaced.sql`") — inform.**
  - `0230` is taken: `migrations/0230_catalog_client_service_account.sql` is on `origin/feat/svc-accounts-sa2` (worktree `egret-sa2`). Main's top is `0229_service_account_principals.sql` (#867). Migration numbers are one namespace across main and open branches.
  - **Plan:** `migrations/0231_execution_labels_replaced.sql`. T0 re-checks every local and remote branch and every worktree for `023[1-9]_*` and moves to the next free number if needed.
- **S2 (D3: "the root and every `parent_execution_id` descendant" with `own_prefix`, "as the pruner does") — inform.**
  - The pruner's family is `storage::index::loop_family` (`storage/index.rs:91-110`, macro `family!` `:60-66`), which walks **only rows with a `loop_index`** (iterations). A `CREATE_NEW_EXECUTION` retry replacement has `parent_execution_id` set and `loop_index` NULL (`worker/replay.rs:244-262`), so `loop_family` leaves it out, yet the `ON DELETE CASCADE` on `parent_execution_id` (0138) deletes its row, and its iterations', with the root.
  - **Plan:** `worker/delete.rs` walks its own family: every row reached downward through `parent_execution_id`, whatever `loop_index` says. Objects, idle, dependents and the audit counts all use that family. `loop_family` is not changed.
- **S3 (D9: `has_dependents` with `details.executions`, `not_a_root` with `details.root`) — inform.**
  - No error body carries `details`. `egret_core::http::ErrorBody` is `{error, code}` (`crates/egret-core/src/http.rs:36-46`), built by struct literal in ten files across five crates. `ExecOpError` is `Core(Error)` or `Coded(StatusCode, &'static str, String)` (`routes.rs:4489-4494`), rendered at `routes.rs:4659-4680`.
  - The frontend already reads extra fields: `ApiError.body` holds the whole parsed body (`frontend/src/api/client.ts:4-29`).
  - **Plan:** keep `egret_core` unchanged. `ExecOpError` gains `Detailed(StatusCode, &'static str, String, serde_json::Value)`, rendered as a new `DetailedErrorBody { error, code, details }` (`ToSchema`), which the two 409s name in OpenAPI. The message names the ids too, so a client reading only `error` loses nothing. `has_dependents` also carries `details.total`, since at most 20 are named.
- **S4 (D3: stored files are "each family member's prefix with `own_prefix`") — inform.**
  - `WorkingDirectory` snapshots live under `FileStore::working_dir_root(ws, exec)` = `s3://{bucket}/{prefix}{ws}/executions/{exec}/_wd/` (`storage/mod.rs:243-251`), **outside** `execution_prefix` (`:255-270`). `own_prefix` (`storage/prune.rs:519-543`) covers only `execution_prefix`.
  - The `_wd` sweeper finds snapshots through `task_runs.working_dir_snapshot`/`working_dir_swept_at` (`storage/wd_sweep.rs:151`, migration 0197). Those rows cascade with the execution, so after a delete nothing would ever find those objects.
  - **Plan:** with `deleteStorage`, each member's `working_dir_root` is deleted too, after its `own_prefix`. The key is `{ws}/executions/{uuid}/_wd/`: a namespace path never has a UUID segment followed by `_wd`, and `execution_prefix` puts `executions/` after the flow slug, so the range is the execution's alone. `delete_prefix` already refuses a key that does not end with `/` (`egret-storage/src/lib.rs:106-…`).
- **S5 (D3/D5: "under the execution lock") — inform.**
  - The worker's lock helper is `claim::load` (`worker/claim.rs:4800-4806`): `SELECT … FOR NO KEY UPDATE`, then it **plans the stored source** (`plan_stored`, `:4836-4837`) and fails `Unplannable` when that does not plan. 7c's `worker/pause.rs:35-57` and 7a's `restart::lock` (`worker/restart.rs:331-357`) both go through it, inside `transaction!`, because they settle afterwards.
  - Delete and set-labels settle nothing and must work on an execution whose frozen source no longer plans (an old execution is exactly what a delete cleans up).
  - **Plan:** both use a plain `pool.begin()` transaction and lock the row themselves: delete with `FOR UPDATE` (the lock `DELETE` needs, and the one that blocks a concurrent `INSERT` that would reference the row: an FK check takes `KEY SHARE`, which `FOR UPDATE` conflicts with); set-labels with `FOR NO KEY UPDATE` (the same mode as `claim::load`, so it serialises with a restart or a settle). Delete is retried on a deadlock as the kill is (`kill::is_deadlock`, `DEADLOCK_RETRIES`, `worker/kill.rs:160`, `:534-545`).
  - 7c's "route reads, worker re-checks, `Moved`" exists only for resume (`ResumeRequest.expected`, `worker/pause.rs:60-75`), where the route stores files before the worker runs. Here the route stores nothing before the worker, so the worker makes **every** decision under the lock and the route reads nothing but the permission and the query. No `Moved` is needed.
- **S6 (D6: "Replay and restart on a new source copy `labels_replaced` with `labels`") — inform.**
  - Four INSERTs copy `labels`, not two:
    - `worker/copy.rs:249-257` + `claim::insert_execution` (replay, restart on a source);
    - `worker/replay.rs:244-280` (a `CREATE_NEW_EXECUTION` retry replacement, "`Execution.childExecution` copies the labels");
    - `worker/spawn.rs:417-428` (a `Loop` iteration, `(SELECT labels FROM workflow.executions WHERE id = $6)`);
    - `worker/restart.rs:576-590` writes the restart label in place, onto the same row, so the flag stays: no change.
  - A restart in place of an edited execution can create new iterations and retry replacements, so all three copies need the flag, or an iteration of an edited root would merge the removed flow label back in.
  - **Plan:** the three INSERT sites copy `labels_replaced`. `copy.rs` sets it with one `UPDATE` right after `insert_execution` (the settle comes later, `copy.rs:300-343`), so the 27 `NewExecution { … }` literals stay untouched.
- **S7 (D10: "the detail's `idle` field, added if the detail does not carry one yet") — inform.** It does not. `ExecutionDto` (`routes.rs:1798-1838`) has `paused_by_hand` but no idle flag; `log_status` computes idle in SQL only for `complete` (`repo.rs:1546-1566`). **Plan:** `ExecutionDto.idle`, computed in `ExecutionDto::new` from the detail it already holds: the execution's state is terminal and every run in `detail.runs` is terminal. `detail.runs` already drops the rows a restart emptied (`repo.rs:1517-1523`, `history <> '[]'`), which is exactly what `workflow.execution_is_idle` ignores since migration 0225 (`workflow.run_is_live`). The server re-checks with the SQL function under the lock.
- **S8 (Q2: dependents are "a subflow child (`trigger_execution_id`) or a replay or restart (`replayed_from_execution_id`)") — DECIDED 2026-11-16: (b) detach.**
  - A **Flow-trigger** fire also sets `trigger_execution_id` to its source (`flow_trigger/process.rs:1035-1045`, `trigger_execution_id: Some(fired.trigger_execution_id)`), with `trigger_run_ix` NULL (`NewExecution` has no run ordinal; `claim::insert_execution`, `claim.rs:4438-4466`, never writes it) and `flow_trigger_event_id` set. Refusing it would make every execution an alerting flow listened to undeletable until each fire is deleted first.
  - **Discriminator: `flow_trigger_event_id IS NOT NULL`.** Migration 0157's CHECK `workflow_executions_flow_trigger_event_iff_flow` (`0157:63-66`) makes it true exactly for an execution whose `execution_trigger` is a `Flow` record. `trigger_run_ix IS NULL` is not one: it is also NULL on any row without a trigger link.
  - **NULL is accepted (verified against 0138:149-190):** `workflow_executions_one_link_kind` is `parent_execution_id IS NULL OR trigger_execution_id IS NULL` (a NULL passes); `workflow_executions_trigger_run_needs_its_execution` is `trigger_run_ix IS NULL OR trigger_execution_id IS NOT NULL` (a fire's `trigger_run_ix` is NULL, so it passes); `workflow_executions_no_self_link` uses `IS DISTINCT FROM` (NULL passes); `workflow_executions_loop_run_whole_or_absent` does not read the column. The composite FK (`0138:136-138`) is not checked for a NULL member. 0157's CHECK reads `execution_trigger`, not the link.
  - **Plan:** in the delete's transaction, after the idle and outbox checks and before the dependents query, `UPDATE workflow.executions SET trigger_execution_id = NULL WHERE workspace_id = $1 AND trigger_execution_id = ANY(family) AND flow_trigger_event_id IS NOT NULL`. The fired execution keeps its `execution_trigger` record, which names the source id as data (`variables.executionId`). A later refusal (a subflow child, a replay) rolls the detach back with everything else. Only `Subflow` children and replays/restarts into a new execution answer `has_dependents`. `Deleted` and the audit gain `detached` (the count).
  - What the fired execution loses: its "Child of" link in the UI, and read access to the source's files (`storage::index::readable_executions` walks `trigger_execution_id`), which the delete removes anyway with `deleteStorage`. Spec §5 gains divergence 9.
  - T3's test `s8_a_flow_trigger_fire_is_detached_not_a_dependent` pins it.
- **S9 (Q1: idle = "the execution and its `afterExecution:` runs must have ended") — DECIDED 2026-11-16: (a) refuse.**
  - `workflow.flow_trigger_events.source_execution_id` is `ON DELETE CASCADE` (migration 0157:40). An execution that has just ended still holds its outbox row until every listener has heard it (`done`), retried with backoff and given up after `MAX_ATTEMPTS = 5` (`flow_trigger/process.rs:48`, `:478`). A delete in that window drops the event, and the listeners never fire for that end.
  - **Plan (user decision):** refuse with 409 `not_terminated` while a family member has an outbox row; the message says its Flow triggers have not yet heard it ended and to try again in a minute. Bounded: 5 attempts with backoff.
  - T3's test `s9_a_pending_flow_trigger_event_refuses_the_delete` pins it.
- **S10 (D6: "The list DTO's `labels` (own only) shows the whole list when `labels_replaced`") — inform.** That needs no code: when `labels_replaced` is true the column **is** the whole list, and `From<ExecutionRow> for ExecutionListItemDto` (`routes.rs:1621-1653`) serves the column. **Plan:** only its doc changes (`routes.rs:1595-1600`). The `Labels` task's write (`claim::write_render_side`, `claim.rs:5241-5248`) also needs no code: it writes `Surface::current_labels()` (`render.rs:733-737`), which becomes the visible list in T2, and its `UPDATE` does not touch `labels_replaced`, so the flag is kept (D6). T2 tests both.
- **S11 (§3: `download_lines(filters, cap)`) — inform.** A line can be 15 360 characters (`logs/mod.rs` piece size), so 100 000 lines can be 1.5 GB; collecting them would hold that in one response buffer. **Plan:** `LogRepo::download_batch(ws, exec, &filter, after, limit)` reads id-ordered batches of 2 000, and the route streams them (`axum::body::Body::from_stream`, as `storage/routes.rs:287` does). A database error mid-stream aborts the body, so the client sees a failed download, never a short file that looks whole.
- **S12 (D7: `Cache-Control: no-cache` "while the execution is not idle") — inform.** `ExecutionLogStatus` (`repo.rs:1677-1683`) carries `state` and `complete` only. **Plan:** it gains `idle: bool`, one more column of the same statement.
- **S13 (D5, D7, D8: the flow's trash) — inform.** D1 says a trashed or purged flow does not block a delete; the spec does not say it for labels or logs. **Plan:** none of the four routes looks at the flow (`live_flow_of`, `routes.rs:2486-2496`, is not called), as `GET …/logs` does not. Editing labels or logs of a past run is safe whatever became of its flow.
- **S14 (D10: "navigate to the executions list with a toast") — inform.** FE-ACTION-2 says no toast when a mutation navigates to its result. The executions list is not the result of a delete (the result is that the execution is gone), so the toast stays, as Kestra's. Recorded so the review does not flag it.
- **S15 (D8: `attempt`) — inform.** `workflow.task_logs.attempt` is 0-based (`LogLineDto` doc, `routes.rs:2156-2159`), as served by `GET …/logs`. The delete's `attempt` filter is the same number. Kestra's `attemptNumber` is also the attempt's index.

---

## Investigation record (facts the implementers must not re-derive)

### Kestra (`/Users/1pro-l4r8/Documents/github/data-portal/kestra`, read-only)

Cited in spec §1 and the survey `7d-kestra-survey.md` (session scratchpad). `LogEntry.toPrettyString` is `<timestamp.toString()> <LEVEL> <message>` (`core/…/models/executions/LogEntry.java:136-138`), lines joined with `\n`, no trailing newline. `minLevel` deletes that level and above (`LogEntry.java:73-81`). `mergeSystemLabels` is `EC:2383-2402`.

### egret

- **Labels.**
  - `labels.rs` (178 lines): `execution_labels(flow, own)` (Kestra merge, `:12-24`), `own_labels` (`:27-36`), `backfill_labels_refusal` (`:39-80`), caps `MAX_BACKFILL_LABELS = 50`, `MAX_LABEL_KEY_CHARS = 128`, `MAX_LABEL_VALUE_CHARS = 1024` (`:6-8`). All `pub(crate)`; `mod labels` is `pub(crate)` (`lib.rs:61`). `regex` is a dependency (`Cargo.toml:162`).
  - Column `workflow.executions.labels jsonb` CHECK array (migration 0192:100-102). `ExecutionRow.labels` (`repo.rs:1090-1092`); the projection is the macro `execution_cols!` (`repo.rs:1016-1023`), used by the list (`:1268`) and `get` (`:1452`). No code builds an `ExecutionRow` by literal.
  - Readers: `ExecutionDto::new` (`routes.rs:1840-1869`, `effective_labels`), `Surface::load_with` (`worker/render.rs:210-224` SELECT, `:305-309` merge), `flow_trigger::process::load_source` (`flow_trigger/process.rs:787-811`, `SourceRow` `:181-195`). `worker/subflow.rs:715-724` reads the raw column only for `system.restarted`, which a replaced list keeps.
  - Writers: start (`claim::insert_execution`, `claim.rs:4438-4466`), the `Labels` task (`claim.rs:5241-5248`), restart in place (`restart.rs:576-590`), and the three copies of S6.
  - `system.*` labels the engine writes: `system.restarted`, `system.replay` (`worker/restart.rs:52-61`).
- **Idle.** `workflow.execution_is_idle(uuid)` as redefined by migration 0225 (`:48-60`): terminal AND no `run_is_live(state, history)`. Used by `replay`'s `OLD_ROW` (`copy.rs:150`), `restart::lock` (`restart.rs:343`), the pruner (`prune.rs:403`, `:619`), `log_status` (`repo.rs:1549`).
- **FKs onto `workflow.executions`:** `task_runs`, `task_logs`, `loop_file_values`, `flow_trigger_events` (`ON DELETE CASCADE`), `execution_files` (composite, CASCADE), `parent_execution_id` (composite, CASCADE, 0138:131), `trigger_execution_id` (composite, **NO ACTION**, 0138:137), `replayed_from_execution_id` (composite, **NO ACTION**, 0166:60-62; indexed 0166:72). Index on `trigger_execution_id` 0138:245.
- **Storage.** `FileStore { store, bucket, prefix, caps, retention_days }`, all `pub` (`storage/mod.rs:95-104`); `execution_prefix` `:255-270`; `working_dir_root` `:243-251`; `storage::current()` `:325`; `install_mem_for_tests()` `:332` (feature `test-util`). `prune::own_prefix` is `pub(crate)` (`prune.rs:519-543`). `ObjectStore::delete_prefix(uri)` (`egret-storage/src/lib.rs:98`). The log writer survives a deleted execution: SQLSTATE 23503 on a batch is `InsertError::ExecutionDeleted` and the batch is retried without it (`logs/repo.rs:20-48`).
- **Logs.** `workflow.task_logs` (0159:67-78): `level smallint 0..4`, `attempt integer`, `ts`, `inserted_at DEFAULT now()`. `LogQuery` `logs/repo.rs:80-89`, `StoredLine` `:92-102`, `LogRepo` (`Clone`, `{pool}`) `:149-152`, `page` `:246-279`, private row mapper `stored` `:468`. `LogLevel` `logs/mod.rs:93-140` (`as_str`, `as_i16`, `parse`). The route `get_execution_logs` (`routes.rs:2254-2290`), `ExecutionLogsQuery` (`:2131-2149`), `log_status` (`repo.rs:1546-1566`).
- **Routes.**
  - Permission constants `WF_READ`/`WF_MANAGE` (`routes.rs:119-122`). The module doc's route list is `routes.rs:30-40`.
  - The model handler is `pause_execution` (`routes.rs:3430-3510`): `require`, scoped `get`, worker call, `st.audit.emit(egret_audit::mutation_for(&ctx.principal, Some(ws), action, "workflow_execution", Some(id.to_string()), detail))` (`crates/egret-audit/src/lib.rs:36-43`), read back, `ExecutionDto::new(detail, true, st.trigger_flags.worker_enabled)`.
  - A purge's audit names what is gone (`purge_flow`, `routes.rs:957-978`).
  - JSON body with a coded refusal: `set_execution_status` (`routes.rs:4969-4996`) takes `Result<Json<T>, JsonRejection>` and maps the rejection (`state_change_body_refusal`, `:4743-4748`).
  - Codes block `routes.rs:4336-4362`; `ExecOpError` `:4489-4525`; `IntoResponse` `:4659-4680`.
  - Router: `management_router` `routes.rs:5314`; `/executions/{id}` `:5434-5437`, `/executions/{id}/logs` `:5442-5445`; 7c's routes `:5534-5546`. OpenAPI `paths(` `:5574`, `components(schemas(` `:5637`.
  - OpenAPI file: `crates/egret-server/src/lib.rs:855` `committed_openapi_json_is_current` ("run `egret openapi > openapi.json`", i.e. `cargo run -p egret-bin -- openapi > openapi.json`); `:961` `generated_client_was_emitted_from_the_committed_spec` reads `frontend/src/api/generated.digest.json`, which `npm run gen:api` writes (`frontend/package.json:20`: `orval && node scripts/encode-path-params.mjs && node scripts/write-api-digest.mjs`). Operation ids are handler names; orval prefixes the tag: `useWorkflowDeleteExecution`, `getWorkflowDownloadExecutionLogsUrl`.
- **Tests.** Route harness: `tests/pause_resume_routes.rs:97-260` (`setup`, `flow_of`, `start`, `exec_state`, `settle_everything`, `expect_error`, `listed_under`, `LOOPY`). Worker harness: `tests/pause_resume.rs:40-196`. `common::{seed_workspace, grant, app, post, post_no_body, delete, get, json, body_text, audit_mutations}` (`tests/common/mod.rs`). A store fake: `tests/file_inputs.rs:43-90` (`Faulty`). A render of labels: `tests/worker.rs:1033-1100`. A retry replacement's labels: `tests/execution_timestamps.rs:469-498`.
- **Frontend.**
  - `routes/workflows/ExecutionDetail.tsx`: `ExecutionActions` `:374-560` (`ReasonButton`, `DisabledReasons`, `ConfirmAction`, `useRefreshExecution`), `ConfirmAction` `:568-640` (stays open with the error, FE-ACTION-3), `RunLogs` `:239-254`.
  - `routes/workflows/ExecutionOverview.tsx`: `LabelChips` `:91-104` (renders `effective_labels`), `TaskRuns` `:371-480` (the row `DropdownMenu`).
  - `components/workflows/executions/actions.ts`: `Enablement` from `components/workflows/draftState`, `isLoopIteration` `:48-50`, `iterationReason` `:52-53`, `pauseRefusal` `:76-78`. `useExecutionAction.ts`: `useRefreshExecution`, `runExecutionAction` (`:19-48`).
  - Logs: `components/workflows/executions/logs/LogView.tsx` (`saveText` `:77-84`, props `:88-121`, `runDownload` `:165-182`, toolbar `:274-307`); `useLogFollow.ts` (`LOG_DOWNLOAD_MAX_LINES` `:335`, `downloadLogs` `:356-…`); `ExecutionLogsTab.tsx` (filters in the URL, `LogView` at the end).
  - A plain download link: `components/workflows/executions/files/FilePreviewDrawer.tsx:92-101` (`DownloadLink`, an `<a href download>`; the session cookie authenticates it).
  - Test harnesses: `components/workflows/executions/__tests__/ReplayDialog.test.tsx:1-80` (a `Server` stubbing `fetch`), `routes/workflows/__tests__/ExecutionDetail.test.tsx`.

---

## Global Constraints

- Spec Q1–Q8 and D1–D13 are binding, with the dated corrections the controller folds in before T1. When the spec and egret's code disagree in a way not listed above, **stop and report**; do not improvise. Kestra's source is read-only.
- **Code search:** CodeGraph first: `codegraph_explore` with `projectPath: /Users/1pro-l4r8/Documents/github/data-portal/egret-execops`. Use grep only for literal strings. `rg` is not installed.
- **Worktree:** `/Users/1pro-l4r8/Documents/github/data-portal/egret-execops`, branch `feat/workflow-delete-labels-logs`.
  - Never switch branches. Never use `git stash`.
  - No network: no `git push`/`fetch`/`pull`, no `gh`, no `npm ci`/`npm install`.
- **Environment (every cargo command):**
  ```bash
  export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-execops/target
  export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test
  export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6465
  ```
  - **Run one cargo command at a time**; never two test suites at once.
  - Run `df -h` before a full build. Clean `target/debug/incremental` rather than starting a second target dir.
  - Never touch port 5432 or the `egret-dev-*` / `egret-demo-*` containers.
- **Frontend `node_modules`:** if `frontend/node_modules` is absent, symlink it, never install:
  ```bash
  ln -s ../../egret-sp2b/frontend/node_modules /Users/1pro-l4r8/Documents/github/data-portal/egret-execops/frontend/node_modules
  ```
  Remove the symlink at the end of T8 (it is gitignored; never commit it). `npm run gen:api`, never bare `orval`.
- **Gates** (each task runs the ones it touches; T9 runs them all, one at a time):
  - `cargo fmt --check`
  - `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings`
  - `cargo check -p egret-bin`
  - `cargo test -p egret-workflow --no-fail-fast` (7 `sql_starrocks_live` failures are expected on this host; anything else is a failure)
  - `cargo test -p egret-config -p egret-server -p egret-metastore`
  - `npm run lint`, `npm run typecheck`, `npm run test`, `npm run build`, `npm run test:bundle:workflows` (in `frontend/`)
- **Commits:** Conventional Commits, each message ending with
  ```text
  Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
  ```
  No `Closes #` in commits. Never commit scratch files or `.superpowers/`. Scratch output goes to the session scratchpad (`$SCRATCH`), never into the repo.
- **Test binaries:** every new `#[sqlx::test]` binary calls `egret_test_db::assert_database_url_is_exported()` in its setup **and** has
  ```rust
  #[test]
  fn database_url_is_exported_rather_than_read_from_dotenv() {
      egret_test_db::assert_database_url_is_exported();
  }
  ```
  This plan's new binaries are `tests/labels_replaced.rs`, `tests/delete_labels.rs` and `tests/delete_labels_logs_routes.rs`.
- **Red first:** each task writes its failing tests before the code, and runs them to see them fail.
- **Destructive tests touch only their own stores:** every delete test builds its own `MemStore` `FileStore` (or a failing fake), never the installed global and never a shared bucket (memory: a red test of a destructive call wiped the dev bucket).
- **Ratchet and census:** **301 / 25 / 25 / 0** and census **295 / 25 / 21** (7c's as-built), re-measured at T0. Nothing in this package should move them; any change is a regression to report.
- No new `std::env::var`. If one becomes necessary, add it to `egret-config`'s `ENV_OUTSIDE_CONFIG` and run `cargo test -p egret-config`.
- One migration (`0231`, S1). Any other: stop and report.
- **Words in code and messages:**
  - New refusal codes, exactly: `not_terminated` (409), `has_dependents` (409, `details.executions`, `details.total`), `not_a_root` (409, `details.root`), `invalid_labels` (400), `delete_logs_required` (400).
  - Audit actions: `workflow_execution:delete` `{namespace, flow_key, state, delete_storage, executions, files, detached}`; `workflow_execution:labels` `{keys_before, keys_after}`; `workflow_execution:logs_delete` `{task_id, task_run_id, attempt, min_level, deleted}`. Never a label value, an input or an output.
  - The truncation line: `… truncated at 100000 lines` (U+2026, then a space).
- Code, comments and docs in English.

---

## File structure

| File | Change | Task |
|---|---|---|
| `migrations/0231_execution_labels_replaced.sql` | **new**: the column | T1 |
| `crates/egret-workflow/src/labels.rs` | `SYSTEM_LABEL_PREFIX`, `visible_labels`, `edit_labels_refusal`, `merge_system_labels`; tests | T1 |
| `crates/egret-workflow/src/repo.rs` | `execution_cols!` + `ExecutionRow.labels_replaced`; `ExecutionLogStatus.idle` | T1, T5 |
| `crates/egret-workflow/src/routes.rs` | detail reader; list-DTO doc; four routes, codes, `ExecOpError::Detailed`, `DetailedErrorBody`, `ExecutionDto.idle`, OpenAPI, router | T2, T6 |
| `crates/egret-workflow/src/worker/render.rs` | `Surface::load_with` reads the flag | T2 |
| `crates/egret-workflow/src/flow_trigger/process.rs` | `load_source` reads the flag | T2 |
| `crates/egret-workflow/src/worker/copy.rs`, `worker/replay.rs`, `worker/spawn.rs` | copy the flag (S6) | T2 |
| `crates/egret-workflow/tests/labels_replaced.rs` | **new**: readers and copies | T2 |
| `crates/egret-workflow/tests/execution_timestamps.rs` | one test beside `a_create_new_execution_retry_copies_the_labels` | T2 |
| `crates/egret-workflow/src/worker/delete.rs` | **new**: `DeleteRequest`, `Deleted`, `DeleteRefused`, `delete_execution` | T3 |
| `crates/egret-workflow/src/worker/labels.rs` | **new**: `LabelsSet`, `LabelsRefused`, `set_execution_labels` | T4 |
| `crates/egret-workflow/src/worker/mod.rs` | `mod delete; mod labels;` and re-exports | T3, T4 |
| `crates/egret-workflow/tests/delete_labels.rs` | **new** worker-level tests | T3, T4 |
| `crates/egret-workflow/src/logs/repo.rs`, `logs/mod.rs` | `LogFilter`, `delete_for_execution`, `download_batch`; `download_line`, `LOG_DOWNLOAD_MAX_LINES`, `TRUNCATED_LINE`; tests | T5 |
| `crates/egret-workflow/tests/task_logs.rs` | repo tests for delete and download | T5 |
| `crates/egret-workflow/tests/delete_labels_logs_routes.rs` | **new** route tests | T6 |
| `openapi.json` | regenerated | T6 |
| `frontend/src/api/generated/**`, `frontend/src/api/generated.digest.json` | `npm run gen:api` | T7 |
| `frontend/src/components/workflows/executions/actions.ts` (+ `__tests__/actions.delete.test.ts`) | `canDelete`, `canEditLabels`, `deleteRefusal`, `labelsRefusal`, `dependentsOf` | T8 |
| `frontend/src/components/workflows/executions/DeleteExecutionDialog.tsx` (+ test) | **new** | T8 |
| `frontend/src/components/workflows/executions/EditLabelsDialog.tsx` (+ test), `labelsModel.ts` (+ test) | **new** | T8 |
| `frontend/src/components/workflows/executions/logs/DeleteLogsDialog.tsx` (+ test) | **new** | T8 |
| `frontend/src/components/workflows/executions/logs/LogView.tsx`, `useLogFollow.ts`, `ExecutionLogsTab.tsx` (+ tests) | Download is a link; `toolbar` slot; Delete logs; `downloadLogs` removed | T8 |
| `frontend/src/routes/workflows/ExecutionDetail.tsx`, `ExecutionOverview.tsx` (+ tests) | header buttons; task-run row Delete logs | T8 |
| `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`, this spec, `docs/vi/...` | dated notes, §7 placeholder | T9 |

---

## T0. Environment and baseline

- [ ] Check the containers: `docker ps --format '{{.Names}} {{.Ports}}' | grep egret-execops` lists `egret-execops-postgres` on 5465 and `egret-execops-valkey` on 6465. If either is missing, start it as 7b's T0 did (image `postgres:16`, `egret`/`egret`/`egret_test`, host port 5465; Valkey on 6465), and report it.
- [ ] Export the three variables (Global Constraints).
- [ ] Ratchet and census: `cargo test -p egret-workflow --test executor -- --nocapture 2>&1 | tee "$SCRATCH/t0-executor.txt"`. Read the tuple and the census line from the file, not from a `tail` (memory: `tail` truncates the report).
- [ ] Migration number (S1):
  ```bash
  ls migrations | tail -2
  for b in $(git for-each-ref --format='%(refname:short)' refs/heads refs/remotes); do git ls-tree --name-only "$b" migrations/ | grep -E '^migrations/023[0-9]' | sed "s|^|$b: |"; done | sort -u
  ls ../*/migrations/023* 2>/dev/null
  ```
  Expect `0230_catalog_client_service_account.sql` on `feat/svc-accounts-sa2` and nothing at `0231`. If `0231` is taken, use the next free number everywhere this plan says `0231`, and report it.
- **Accept:** ratchet 301 / 25 / 25 / 0; census 295 / 25 / 21; the free migration number. Report all three as measured. No commit.

---

## T1. The column and the pure label rules (spec Q3, Q8, D5, D6; S1)

**Files:**
- Create: `migrations/0231_execution_labels_replaced.sql`.
- Modify: `crates/egret-workflow/src/labels.rs` (module doc, three functions, tests), `src/repo.rs:1016-1023` (`execution_cols!`) and `:1048-1095` (`ExecutionRow`).

**Interfaces.**

Produces:
```rust
// labels.rs, all pub(crate)
pub(crate) const SYSTEM_LABEL_PREFIX: &str = "system.";
/// D6: the labels an execution shows and renders.
pub(crate) fn visible_labels(flow: &[Label], own: &[Label], replaced: bool) -> Vec<Label>;
/// D5: `Some(sentence naming the label)` on the first breach of `body`.
pub(crate) fn edit_labels_refusal(body: &[Label], visible: &[Label]) -> Option<String>;
/// D5, Kestra `mergeSystemLabels` (`EC:2383-2402`): `body`, then every
/// `system.*` label of `visible` whose key `body` does not hold.
pub(crate) fn merge_system_labels(body: &[Label], visible: &[Label]) -> Vec<Label>;

// repo.rs
pub struct ExecutionRow { /* … */ pub labels_replaced: bool, /* … */ }
```

**Steps.**

- [ ] **Write the failing unit tests** at the end of `labels.rs`'s `mod tests` (below). Run `cargo test -p egret-workflow --lib labels::`: it does not compile.
- [ ] **Migration** `migrations/0231_execution_labels_replaced.sql`:
  ```sql
  -- Workflow package 7d (spec docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md
  -- Q8, D6). `labels` (0192) holds the labels beyond the flow's own `labels:`,
  -- which every reader merges in from the frozen `source`. Once a person has
  -- replaced an execution's labels (`POST …/executions/{id}/labels`), `labels`
  -- holds the WHOLE visible list and the flow's are not merged in any more, so a
  -- flow label can be removed. Every reader goes through `labels::visible_labels`.
  ALTER TABLE workflow.executions
    ADD COLUMN labels_replaced boolean NOT NULL DEFAULT false;
  ```
  Adding a `NOT NULL DEFAULT false` column is a metadata-only change on Postgres 11+; no table rewrite.
- [ ] **`labels.rs`.** Replace the module doc's first line with "Execution labels (W7f spec §3.4; package 7d D5, D6): Kestra's merge, the visible list, and the checks a backfill's and an edit's labels pass before they are stored." Add after `backfill_labels_refusal`:
  ```rust
  /// The prefix of the labels only the engine writes (`system.restarted`,
  /// `system.replay`; Kestra `Label.SYSTEM_PREFIX`). Case-sensitive, as
  /// Kestra's `startsWith`.
  pub(crate) const SYSTEM_LABEL_PREFIX: &str = "system.";

  /// Kestra's label key pattern (`Label.java`, `@Pattern`): a lower-case
  /// letter, then letters, digits, `.`, `_` and `-`.
  static LABEL_KEY: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
      regex::Regex::new(r"^\p{Ll}[\p{L}0-9._-]*$").expect("the label key pattern compiles")
  });

  /// **The labels an execution shows and renders** (package 7d D6). Every
  /// reader of `workflow.executions.labels` calls this with
  /// `labels_replaced` beside it. Not replaced: the flow's `labels:` then
  /// the execution's own ([`execution_labels`]). Replaced (a person set the
  /// whole list): the execution's own only, deduplicated the same way, so a
  /// flow label that was removed stays removed.
  pub(crate) fn visible_labels(flow: &[Label], own: &[Label], replaced: bool) -> Vec<Label> {
      if replaced {
          execution_labels(&[], own)
      } else {
          execution_labels(flow, own)
      }
  }

  /// **D5's rules for a replacement list**, checked against the execution's
  /// visible list; `Some(sentence naming the label)` on the first breach.
  /// The backfill's rules without its template rule (a backfill renders
  /// nothing, an edit renders nothing either, but `{{` is a legal value
  /// here as in Kestra), plus:
  /// - Kestra's key pattern, **only for a key the visible list does not
  ///   already hold**: a flow may carry older keys that do not match, and
  ///   sending the list back unchanged must not fail;
  /// - a `system.*` label passes only if the same key AND value are visible
  ///   (Kestra: "System labels can only be set by Kestra itself").
  pub(crate) fn edit_labels_refusal(body: &[Label], visible: &[Label]) -> Option<String> {
      if body.len() > MAX_BACKFILL_LABELS {
          return Some(format!(
              "an execution takes at most {MAX_BACKFILL_LABELS} labels, got {}",
              body.len()
          ));
      }
      for (i, Label { key, value }) in body.iter().enumerate() {
          if key.trim().is_empty() {
              return Some("a label key is empty or only whitespace".to_string());
          }
          if body[..i].iter().any(|l| l.key == *key) {
              return Some(format!("label `{key}` is given more than once"));
          }
          if value.is_empty() {
              return Some(format!("label `{key}` has an empty value"));
          }
          if key.chars().count() > MAX_LABEL_KEY_CHARS {
              return Some(format!(
                  "label key `{key}` is longer than {MAX_LABEL_KEY_CHARS} characters"
              ));
          }
          if value.chars().count() > MAX_LABEL_VALUE_CHARS {
              return Some(format!(
                  "the value of label `{key}` is longer than {MAX_LABEL_VALUE_CHARS} characters"
              ));
          }
          if key.starts_with(SYSTEM_LABEL_PREFIX) {
              if !visible.iter().any(|l| l.key == *key && l.value == *value) {
                  return Some(format!(
                      "label `{key}` is a system label: only the engine sets `system.*` labels, \
                       and one the execution already has can only be sent back unchanged"
                  ));
              }
              continue;
          }
          if !visible.iter().any(|l| l.key == *key) && !LABEL_KEY.is_match(key) {
              return Some(format!(
                  "label key `{key}` must start with a lower-case letter and hold only \
                   letters, digits, `.`, `_` and `-`"
              ));
          }
      }
      None
  }

  /// **Kestra `mergeSystemLabels`** (`EC:2383-2402`, package 7d D5): the
  /// body, then each `system.*` label of `visible` whose key the body does
  /// not hold, in `visible`'s order. Call after [`edit_labels_refusal`],
  /// which has already refused a `system.*` label the body changed.
  pub(crate) fn merge_system_labels(body: &[Label], visible: &[Label]) -> Vec<Label> {
      let mut out = body.to_vec();
      for l in visible.iter().filter(|l| l.key.starts_with(SYSTEM_LABEL_PREFIX)) {
          if !out.iter().any(|o| o.key == l.key) {
              out.push(l.clone());
          }
      }
      out
  }
  ```
- [ ] **`repo.rs`.** Append `, labels_replaced` to `execution_cols!` after `labels` (keep the string one literal: `"…, updated_at, labels, labels_replaced, backfill_id"`), and add to `ExecutionRow` after `labels`:
  ```rust
  /// Package 7d (migration 0231): `labels` holds the whole visible list --
  /// a person replaced it -- and the flow's `labels:` are not merged in.
  /// `labels::visible_labels` is the one place that reads the two together.
  pub labels_replaced: bool,
  ```
- [ ] Run, one at a time:
  - `cargo test -p egret-workflow --lib labels::`
  - `cargo test -p egret-workflow --test executions_routes` (the list and detail still read; green)
  - `cargo clippy -p egret-workflow --all-targets -- -D warnings`
- [ ] Commit: `feat(workflow): labels_replaced column and the visible-label rules (package 7d)`.

**Unit tests** (`labels.rs` `mod tests`, which already has `fn l(k, v) -> Label`):

```rust
#[test]
fn visible_labels_merge_the_flow_until_replaced() {
    let flow = [l("team", "data"), l("env", "prod")];
    let own = [l("env", "dev"), l("reason", "x")];
    assert_eq!(
        visible_labels(&flow, &own, false),
        vec![l("team", "data"), l("env", "dev"), l("reason", "x")]
    );
    assert_eq!(
        visible_labels(&flow, &own, true),
        vec![l("env", "dev"), l("reason", "x")],
        "replaced: the flow's `team` is gone"
    );
}

#[test]
fn a_replaced_list_is_deduplicated_like_the_merge() {
    let own = [l("a", "1"), l("", "v"), l("b", ""), l("a", "2")];
    assert_eq!(visible_labels(&[l("x", "y")], &own, true), vec![l("a", "2")]);
    assert_eq!(visible_labels(&[l("x", "y")], &[], true), vec![]);
}

#[test]
fn an_edit_may_resend_the_visible_list_unchanged() {
    // `Team` and `9lives` do not match Kestra's pattern, but the flow has them.
    let visible = [
        l("Team", "data"),
        l("9lives", "yes"),
        l("system.restarted", "true"),
    ];
    assert_eq!(edit_labels_refusal(&visible, &visible), None);
    assert_eq!(edit_labels_refusal(&[], &visible), None, "an empty list is a valid edit");
}

#[test]
fn edit_refusals_name_the_label() {
    let visible = [l("team", "data"), l("system.restarted", "true")];
    let cases: Vec<(Vec<Label>, &str)> = vec![
        (vec![l("", "v")], "empty"),
        (vec![l("  ", "v")], "whitespace"),
        (vec![l("dup", "1"), l("dup", "2")], "dup"),
        (vec![l("k1", "")], "k1"),
        (vec![l("Upper", "v")], "Upper"),
        (vec![l("1st", "v")], "1st"),
        (vec![l("sp ace", "v")], "sp ace"),
        (vec![l("system.restarted", "false")], "system.restarted"),
        (vec![l("system.replay", "true")], "system.replay"),
        ((0..51).map(|i| l(&format!("k{i}"), "v")).collect(), "50"),
        (vec![l(&format!("k{}", "a".repeat(128)), "v")], "128"),
        (vec![l("k4", &"v".repeat(1025))], "k4"),
    ];
    for (body, needle) in cases {
        let msg = edit_labels_refusal(&body, &visible)
            .unwrap_or_else(|| panic!("no refusal for {body:?}"));
        assert!(msg.contains(needle), "{msg:?} should mention {needle:?}");
    }
}

#[test]
fn kestra_keys_and_template_values_pass() {
    let body = [
        l("team", "{{ not rendered }}"),
        l("é.x_y-1", "v"),
        l("a", " "),
    ];
    assert_eq!(edit_labels_refusal(&body, &[]), None);
}

#[test]
fn an_unchanged_system_label_passes_and_is_kept_once() {
    let visible = [l("team", "data"), l("system.restarted", "true")];
    let body = [l("system.restarted", "true"), l("env", "dev")];
    assert_eq!(edit_labels_refusal(&body, &visible), None);
    assert_eq!(merge_system_labels(&body, &visible), body.to_vec());
}

#[test]
fn merge_system_labels_appends_what_the_body_left_out() {
    let visible = [
        l("system.restarted", "true"),
        l("team", "data"),
        l("system.replay", "true"),
    ];
    assert_eq!(
        merge_system_labels(&[l("env", "dev")], &visible),
        vec![
            l("env", "dev"),
            l("system.restarted", "true"),
            l("system.replay", "true")
        ]
    );
    assert_eq!(
        merge_system_labels(&[], &visible),
        vec![l("system.restarted", "true"), l("system.replay", "true")]
    );
}

#[test]
fn the_system_prefix_is_case_sensitive_as_kestras() {
    // `System.x` is not a system label; it fails the key pattern instead.
    let msg = edit_labels_refusal(&[l("System.x", "v")], &[]).expect("refused");
    assert!(msg.contains("lower-case"), "{msg}");
    assert_eq!(merge_system_labels(&[], &[l("System.x", "v")]), vec![]);
}
```

---

## T2. Every reader and every copy goes through the flag (spec D6; S6, S10)

**Files:**
- Modify:
  - `src/routes.rs:1840-1869` (`ExecutionDto::new`), `:1595-1600` (the list DTO's `labels` doc), `:1826-1828` (`effective_labels` doc);
  - `src/worker/render.rs:216-224` (the SELECT) and `:305-309`;
  - `src/flow_trigger/process.rs:181-195` (`SourceRow`), `:792-811`;
  - `src/worker/copy.rs:116-127` (`Old`), `:140-152` (`OLD_ROW`), `:173-185` (the read), after the `insert_execution` call (`:262-298`);
  - `src/worker/replay.rs:180-185` (SELECT), `:244-280` (INSERT);
  - `src/worker/spawn.rs:417-428` (INSERT).
- Create: `crates/egret-workflow/tests/labels_replaced.rs`. Modify: `tests/execution_timestamps.rs` (one test).

**Steps.**

- [ ] **Write the failing tests** (list below). Run `cargo test -p egret-workflow --test labels_replaced`: red (a replaced list still renders the flow's `team`).
- [ ] **Detail** (`ExecutionDto::new`): replace the `execution_labels` call with
  ```rust
  let effective_labels = crate::labels::visible_labels(
      &flow_labels,
      &own,
      d.execution.labels_replaced,
  )
  ```
  `effective_labels` doc: "What the run sees as `labels`: the flow's, then `labels`, the later key winning; or, once a person replaced them (`POST …/labels`), exactly `labels`. Empty when the frozen source does not parse and nothing was replaced." List DTO `labels` doc: append "Once a person replaced the labels (`POST …/labels`, package 7d), it holds the whole visible list, the flow's labels included or removed."
- [ ] **Render** (`Surface::load_with`): add `labels_replaced` to the SELECT after `labels`, and
  ```rust
  let replaced: bool = row.get("labels_replaced");
  let labels_list = crate::labels::visible_labels(flow_labels, &own, replaced);
  ```
  Update the comment above (`:305-306`): "The flow's labels first, then the execution's own (a backfill's, a replay's copy), the later value winning (W7f spec §3.3); or, once replaced, the execution's own only (package 7d D6)."
- [ ] **Flow-trigger source** (`load_source`): `SourceRow` gains `labels_replaced: bool` (doc: "Package 7d D6: `labels` is the whole visible list."); the SELECT gains `e.labels_replaced` after `e.labels`; the merge becomes `crate::labels::visible_labels(&flow_labels, &crate::labels::own_labels(row.labels.as_ref()), row.labels_replaced)`.
- [ ] **Replay and restart on a source** (`copy.rs`): `Old` gains `labels_replaced: bool`; `OLD_ROW` selects `x.labels_replaced` after `x.labels`; the read fills it. After `let ctx = claim::insert_execution(…).await?;` and before step 10:
  ```rust
  // Package 7d D6 (S6): the copy carries the whole visible list when the
  // old execution's labels were replaced, so its flag goes with it. Before
  // the first settle, which is the first render.
  if old.labels_replaced {
      sqlx::query("UPDATE workflow.executions SET labels_replaced = true WHERE id = $1")
          .bind(ctx.id)
          .execute(&mut *conn)
          .await
          .map_err(db_err)?;
  }
  ```
- [ ] **Retry replacement** (`replay.rs::spawn_due`): the SELECT gains `labels_replaced`; the INSERT's column list gains `labels_replaced` after `labels` and its `VALUES` one more placeholder (renumber after `$11`), bound right after the labels bind with `row.get::<bool, _>("labels_replaced")` and the comment "Package 7d D6: the flag travels with the list."
- [ ] **Loop iteration** (`spawn.rs`): the column list gains `labels_replaced` after `labels`, and the `VALUES` gains `(SELECT labels_replaced FROM workflow.executions WHERE id = $6),` after the labels subquery.
- [ ] Run, one at a time:
  - `cargo test -p egret-workflow --test labels_replaced`
  - `cargo test -p egret-workflow --test execution_timestamps --test worker --test replay --test replay_routes --test restart_routes --test flow_trigger_fire`
  - `cargo clippy -p egret-workflow --all-targets -- -D warnings`
- [ ] Commit: `feat(workflow): every label reader and copy honours labels_replaced (package 7d)`.

**Tests** (`tests/labels_replaced.rs`; module doc names the package, the spec and "Run with: export DATABASE_URL=… ; cargo test -p egret-workflow --test labels_replaced"). Fixtures: `setup`, `flow_of`, `start`, `exec_state`, `run_state`, `outputs_of`, `handlers`, `settle_everything` copied from `tests/pause_resume_routes.rs:97-215`. Each test is `#[sqlx::test(migrations = "../../migrations")]`.

```rust
const LABELLED: &str = "id: labelled\nnamespace: t\nlabels:\n  team: data\n  env: prod\ntasks:\n  \
     - id: r\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{ labels.env }}\"\n";

/// `team` is the flow's; after a replace that dropped it, reading it fails the run.
const READS_TEAM: &str = "id: reads-team\nnamespace: t\nlabels:\n  team: data\ntasks:\n  \
     - id: r\n    type: io.kestra.plugin.core.debug.Return\n    format: \"{{ labels.team }}\"\n";

/// What `POST …/labels` writes, written by hand before the first tick.
async fn replace(pool: &sqlx::PgPool, ex: Uuid, labels: Value) {
    sqlx::query("UPDATE workflow.executions SET labels = $2, labels_replaced = true WHERE id = $1")
        .bind(ex)
        .bind(labels)
        .execute(pool)
        .await
        .expect("replace the labels");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replaced_list_reaches_the_render_and_a_removed_flow_label_is_gone(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, _user, _app) = setup(&pool).await;
    let env = start(&pool, ws, LABELLED).await;
    let team = start(&pool, ws, READS_TEAM).await;
    let control = start(&pool, ws, READS_TEAM).await;
    replace(&pool, env, json!([{"key": "env", "value": "dev"}])).await;
    replace(&pool, team, json!([{"key": "other", "value": "x"}])).await;

    // Act
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(outputs_of(&pool, env, "r").await, Some(json!({"value": "dev"})));
    assert_eq!(run_state(&pool, team, "r").await.as_deref(), Some("FAILED"));
    assert_eq!(outputs_of(&pool, control, "r").await, Some(json!({"value": "data"})));
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_detail_shows_the_replaced_list(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, LABELLED).await;
    replace(&pool, ex, json!([{"key": "env", "value": "dev"}])).await;
    let body = common::json(
        common::get(&app, &format!("/api/ws/{ws}/workflows/executions/{ex}"), user).await,
    )
    .await;
    assert_eq!(body["effective_labels"], json!([{"key": "env", "value": "dev"}]));
    assert_eq!(body["labels"], json!([{"key": "env", "value": "dev"}]));
}

/// A `Labels` task after an edit merges into the visible list and keeps
/// the removal (D6, S10): `team` does not come back, the flag stays.
#[sqlx::test(migrations = "../../migrations")]
async fn a_labels_task_after_an_edit_keeps_the_removal(pool: sqlx::PgPool) {
    let (_db, ws, _user, _app) = setup(&pool).await;
    let src = "id: lt\nnamespace: t\nlabels:\n  team: data\ntasks:\n  \
               - id: labels\n    type: io.kestra.plugin.core.execution.Labels\n    labels:\n      x: \"1\"\n";
    let ex = start(&pool, ws, src).await;
    replace(&pool, ex, json!([{"key": "env", "value": "dev"}])).await;
    settle_everything(&pool, &handlers()).await;
    let (labels, replaced): (Value, bool) = sqlx::query_as(
        "SELECT labels, labels_replaced FROM workflow.executions WHERE id = $1",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    assert_eq!(labels, json!([{"key": "env", "value": "dev"}, {"key": "x", "value": "1"}]));
    assert!(replaced);
}

/// Control for the test above: without an edit the task's write keeps the
/// flag false and the flow's label is in the stored list, as before 7d.
#[sqlx::test(migrations = "../../migrations")]
async fn a_labels_task_without_an_edit_is_unchanged(pool: sqlx::PgPool) {
    let (_db, ws, _user, _app) = setup(&pool).await;
    let src = "id: lt\nnamespace: t\nlabels:\n  team: data\ntasks:\n  \
               - id: labels\n    type: io.kestra.plugin.core.execution.Labels\n    labels:\n      x: \"1\"\n";
    let ex = start(&pool, ws, src).await;
    settle_everything(&pool, &handlers()).await;
    let (labels, replaced): (Value, bool) = sqlx::query_as(
        "SELECT labels, labels_replaced FROM workflow.executions WHERE id = $1",
    )
    .bind(ex)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(exec_state(&pool, ex).await, "SUCCESS");
    assert_eq!(labels, json!([{"key": "team", "value": "data"}, {"key": "x", "value": "1"}]));
    assert!(!replaced);
}

/// S6: a loop iteration of an edited root copies the flag.
#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_iteration_copies_the_flag(pool: sqlx::PgPool) {
    // Arrange: a loop whose body reads `labels.env`, which only the edit has.
    let (_db, ws, _user, _app) = setup(&pool).await;
    let src = "id: loopy-l\nnamespace: t\nlabels:\n  team: data\ntasks:\n  \
               - id: each\n    type: io.kestra.plugin.core.flow.Loop\n    values: [\"1\", \"2\"]\n    \
               concurrencyLimit: 1\n    tasks:\n      \
               - id: a\n        type: io.kestra.plugin.core.log.Log\n        message: \"{{ labels.env }}\"\n";
    let root = start(&pool, ws, src).await;
    replace(&pool, root, json!([{"key": "env", "value": "dev"}])).await;

    // Act
    settle_everything(&pool, &handlers()).await;

    // Assert
    assert_eq!(exec_state(&pool, root).await, "SUCCESS", "the body rendered `labels.env`");
    let iterations: Vec<(Value, bool)> = sqlx::query_as(
        "SELECT labels, labels_replaced FROM workflow.executions WHERE parent_execution_id = $1",
    )
    .bind(root)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(iterations.len(), 2);
    for it in iterations {
        assert_eq!(it, (json!([{"key": "env", "value": "dev"}]), true));
    }
}

/// S6: a replay of an edited execution copies the flag (worker API).
#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_copies_the_flag(pool: sqlx::PgPool) {
    let (_db, ws, user, _app) = setup(&pool).await;
    let ex = start(&pool, ws, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    replace(&pool, ex, json!([{"key": "env", "value": "dev"}])).await;
    let new = Uuid::new_v4();
    let replayed = egret_workflow::worker::replay_execution(
        &pool,
        &egret_workflow::worker::ReplayRequest {
            workspace_id: ws,
            from: ex,
            source: LABELLED,
            kind: egret_workflow::worker::ReplayKind::Whole,
            inputs: egret_workflow::worker::ReplayInputs::Reuse,
            started_by: user.0,
            id: new,
            input_files: Vec::new(),
        },
    )
    .await
    .expect("replay")
    .expect("not refused");
    settle_everything(&pool, &handlers()).await;
    let replaced: bool =
        sqlx::query_scalar("SELECT labels_replaced FROM workflow.executions WHERE id = $1")
            .bind(replayed)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(replaced);
    assert_eq!(outputs_of(&pool, replayed, "r").await, Some(json!({"value": "dev"})));
}
```
Plus `database_url_is_exported_rather_than_read_from_dotenv`.

In `tests/execution_timestamps.rs`, beside `a_create_new_execution_retry_copies_the_labels` (`:469-498`), add `a_create_new_execution_retry_copies_labels_replaced`: the same flow and steps, with `UPDATE workflow.executions SET labels = $2, labels_replaced = true`; assert the replacement's `labels_replaced` is `true` (red before the `replay.rs` change).

---

## T3. Worker: delete an execution under its lock (spec Q1, Q2, Q5, D1–D3; S2, S4, S5, S8, S9)

**Files:**
- Create: `crates/egret-workflow/src/worker/delete.rs`, `crates/egret-workflow/tests/delete_labels.rs`.
- Modify: `src/worker/mod.rs` (`mod delete;` after `mod copy;`; `pub use delete::{delete_execution, DeleteRefused, DeleteRequest, Deleted};` beside the `copy` re-export at `:152`).

**Interfaces.**

Consumes: `kill::{is_deadlock, DEADLOCK_RETRIES}` (`pub(super)`, `worker/kill.rs:160`, `:534`), `crate::storage::{FileStore, prune::own_prefix}`.

Produces:
```rust
// worker/delete.rs, re-exported from `worker`
#[derive(Clone, Copy, Debug)]
pub struct DeleteRequest { pub workspace: Uuid, pub id: Uuid, pub delete_storage: bool }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deleted {
    pub namespace: String,
    pub flow_key: String,
    /// The root's state when it was deleted.
    pub state: String,
    /// The family's size: the root and every `parent_execution_id` descendant.
    pub executions: u64,
    /// `workflow.execution_files` rows of the family (D4's `files`).
    pub files: i64,
    /// S8: Flow-trigger fires whose `trigger_execution_id` was set to NULL.
    pub detached: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeleteRefused {
    /// D2: a loop iteration or retry replacement; its family's root.
    NotARoot { root: Uuid },
    /// Q1: this family member is not idle; its state.
    NotIdle { execution: Uuid, state: String },
    /// S9 (decided (a)): a Flow-trigger event of the family is still unprocessed.
    ListenersPending,
    /// Q2: `Subflow` children, replays and restarts into a new execution
    /// outside the family that point at it (at most `MAX_NAMED_DEPENDENTS`,
    /// oldest first), and how many there are. Flow-trigger fires are
    /// detached instead (S8).
    HasDependents { executions: Vec<Uuid>, total: i64 },
}

/// At most this many dependents are named in a refusal.
pub const MAX_NAMED_DEPENDENTS: i64 = 20;

/// `fs`: the store the objects are deleted from (`storage::current()` in
/// production; a test's own store). `None`, or `delete_storage: false`,
/// deletes no object. `Err(NotFound)` for a missing or foreign id.
pub async fn delete_execution(
    pool: &PgPool,
    fs: Option<&FileStore>,
    req: DeleteRequest,
) -> Result<std::result::Result<Deleted, DeleteRefused>>;
```

**Steps.**

- [ ] **Write the failing tests** in `tests/delete_labels.rs` (below). Run `cargo test -p egret-workflow --test delete_labels`: it does not compile.
- [ ] **Implement `worker/delete.rs`:**
  ```rust
  //! **Delete an execution** -- workflow package 7d (spec
  //! `docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md` Q1,
  //! Q2, Q5, D1-D3; Kestra `ExecutionService.delete`, `ES:747-767`).
  //!
  //! One transaction, the root's row locked `FOR UPDATE` first, then every
  //! family member's (the root and every `parent_execution_id` descendant:
  //! loop iterations and retry replacements, which `ON DELETE CASCADE` takes
  //! with the root). `FOR UPDATE` conflicts with the `KEY SHARE` an FK check
  //! takes, so no execution can start to point at a locked member while this
  //! runs: a replay, a subflow child or an iteration created now waits, then
  //! fails its own insert. Every refusal comes before the first write but
  //! one: the S8 detach, which a later refusal rolls back with the rest.
  //!
  //! Not `claim::load`: nothing is settled, and an execution whose frozen
  //! source no longer plans must still be deletable (plan S5).
  //!
  //! Objects first, then rows (the pruner's order, `storage/prune.rs`): a
  //! store failure is an `Err`, the transaction rolls back, every row stays,
  //! and the call can be repeated. A member whose prefix is not provably its
  //! own (`own_prefix` is `None`) keeps its objects, logged, as the pruner
  //! does. Each member's `_wd/` snapshots are deleted too (plan S4).

  use egret_core::{Error, Result};
  use sqlx::{PgConnection, PgPool, Row};
  use uuid::Uuid;

  use super::kill::{is_deadlock, DEADLOCK_RETRIES};
  use crate::storage::prune::own_prefix;
  use crate::storage::FileStore;

  // (the types and `MAX_NAMED_DEPENDENTS` of the Interfaces block, with their docs)

  pub async fn delete_execution(
      pool: &PgPool,
      fs: Option<&FileStore>,
      req: DeleteRequest,
  ) -> Result<std::result::Result<Deleted, DeleteRefused>> {
      let mut retries = 0;
      loop {
          match delete_once(pool, fs, req).await {
              Err(e) if is_deadlock(&e) && retries < DEADLOCK_RETRIES => {
                  retries += 1;
                  tracing::info!(execution = %req.id, "a delete was a deadlock's victim; running it again");
              }
              other => return other,
          }
      }
  }

  /// One family member: what its prefix is made of.
  struct Member {
      id: Uuid,
      namespace: String,
      flow_key: String,
  }

  async fn delete_once(
      pool: &PgPool,
      fs: Option<&FileStore>,
      req: DeleteRequest,
  ) -> Result<std::result::Result<Deleted, DeleteRefused>> {
      let ws = req.workspace;
      let mut tx = pool.begin().await.map_err(db_err)?;

      // 1. The root's row, locked; scoped by the workspace.
      let row = sqlx::query(
          "SELECT namespace, flow_key, state, parent_execution_id \
             FROM workflow.executions WHERE id = $1 AND workspace_id = $2 FOR UPDATE",
      )
      .bind(req.id)
      .bind(ws)
      .fetch_optional(&mut *tx)
      .await
      .map_err(db_err)?
      .ok_or_else(|| Error::NotFound("execution".into()))?;
      // 2. D2: only a root.
      if row.get::<Option<Uuid>, _>("parent_execution_id").is_some() {
          let root = root_of(&mut tx, ws, req.id).await?;
          return Ok(Err(DeleteRefused::NotARoot { root }));
      }
      // 3. The family, locked in id order.
      let family = family(&mut tx, ws, req.id).await?;
      let ids: Vec<Uuid> = family.iter().map(|m| m.id).collect();
      // 4. Q1: every member idle (the root first when it is the busy one).
      let busy: Option<(Uuid, String)> = sqlx::query_as(
          "SELECT id, state FROM workflow.executions \
            WHERE id = ANY($1) AND NOT COALESCE(workflow.execution_is_idle(id), false) \
            ORDER BY (id = $2) DESC, id LIMIT 1",
      )
      .bind(&ids)
      .bind(req.id)
      .fetch_optional(&mut *tx)
      .await
      .map_err(db_err)?;
      if let Some((execution, state)) = busy {
          return Ok(Err(DeleteRefused::NotIdle { execution, state }));
      }
      // 5. S9 (user decision (a)): its Flow triggers have not all heard its end.
      let pending: bool = sqlx::query_scalar(
          "SELECT EXISTS (SELECT 1 FROM workflow.flow_trigger_events \
                           WHERE source_execution_id = ANY($1))",
      )
      .bind(&ids)
      .fetch_one(&mut *tx)
      .await
      .map_err(db_err)?;
      if pending {
          return Ok(Err(DeleteRefused::ListenersPending));
      }
      // 6. S8 (user decision (b)): a Flow-trigger fire is detached, not a
      // dependent. `flow_trigger_event_id` is set exactly on a fire
      // (migration 0157's CHECK); 0138's CHECKs accept the NULL. The fire
      // keeps its `execution_trigger` record, which names the source as data.
      // A refusal below rolls this back.
      let detached = sqlx::query(
          "UPDATE workflow.executions SET trigger_execution_id = NULL \
            WHERE workspace_id = $1 AND trigger_execution_id = ANY($2) \
              AND flow_trigger_event_id IS NOT NULL",
      )
      .bind(ws)
      .bind(&ids)
      .execute(&mut *tx)
      .await
      .map_err(db_err)?
      .rows_affected();
      // 7. Q2: what still points at the family from outside it: `Subflow`
      // children and replays/restarts into a new execution (the fires no
      // longer do, after step 6).
      let (total, executions) = dependents(&mut tx, ws, &ids).await?;
      if total > 0 {
          return Ok(Err(DeleteRefused::HasDependents { executions, total }));
      }
      // 8. D4's counts, before the rows go.
      let files: i64 = sqlx::query_scalar(
          "SELECT count(*) FROM workflow.execution_files \
            WHERE workspace_id = $1 AND execution_id = ANY($2)",
      )
      .bind(ws)
      .bind(&ids)
      .fetch_one(&mut *tx)
      .await
      .map_err(db_err)?;
      // 9. D3: the objects, member by member.
      if let (true, Some(fs)) = (req.delete_storage, fs) {
          for m in &family {
              match own_prefix(fs, ws, &m.namespace, &m.flow_key, m.id) {
                  Some((store_prefix, _)) => delete_prefix(fs, m.id, &store_prefix).await?,
                  None => tracing::warn!(
                      workspace = %ws,
                      execution = %m.id,
                      "workflow delete: this execution's prefix is not provably its own; \
                       its stored files are kept"
                  ),
              }
              delete_prefix(fs, m.id, &fs.working_dir_root(ws, m.id)).await?;
          }
      }
      // 10. The rows: the cascade takes the family, its runs, logs, index
      // rows, loop values and outbox rows.
      sqlx::query("DELETE FROM workflow.executions WHERE id = $1 AND workspace_id = $2")
          .bind(req.id)
          .bind(ws)
          .execute(&mut *tx)
          .await
          .map_err(|e| {
              // Unreachable while step 3's locks hold (see the module doc);
              // a 409, not a 500, if a future writer breaks that.
              if e.as_database_error().is_some_and(|d| d.is_foreign_key_violation()) {
                  Error::Conflict(format!(
                      "execution {} gained a dependent while it was being deleted; try again",
                      req.id
                  ))
              } else {
                  db_err(e)
              }
          })?;
      tx.commit().await.map_err(db_err)?;
      Ok(Ok(Deleted {
          namespace: row.get("namespace"),
          flow_key: row.get("flow_key"),
          state: row.get("state"),
          executions: family.len() as u64,
          files,
          detached,
      }))
  }

  /// The top of `id`'s `parent_execution_id` chain, whatever `loop_index`
  /// says (a retry replacement's parent may itself be an iteration).
  async fn root_of(conn: &mut PgConnection, ws: Uuid, id: Uuid) -> Result<Uuid> {
      sqlx::query_scalar(
          "WITH RECURSIVE up(id, parent) AS ( \
             SELECT id, parent_execution_id FROM workflow.executions \
              WHERE id = $1 AND workspace_id = $2 \
             UNION \
             SELECT e.id, e.parent_execution_id FROM up \
               JOIN workflow.executions e ON e.id = up.parent AND e.workspace_id = $2) \
           SELECT id FROM up WHERE parent IS NULL",
      )
      .bind(id)
      .bind(ws)
      .fetch_optional(&mut *conn)
      .await
      .map_err(db_err)?
      .ok_or_else(|| Error::NotFound(format!("the root of execution {id}")))
  }

  /// The root and every `parent_execution_id` descendant (plan S2: not
  /// `storage::index::loop_family`, which stops at rows without a
  /// `loop_index`), each row locked `FOR UPDATE` in id order.
  async fn family(conn: &mut PgConnection, ws: Uuid, root: Uuid) -> Result<Vec<Member>> {
      let ids: Vec<Uuid> = sqlx::query_scalar(
          "WITH RECURSIVE fam(id) AS ( \
             SELECT $1::uuid \
             UNION \
             SELECT e.id FROM workflow.executions e JOIN fam f ON e.parent_execution_id = f.id \
              WHERE e.workspace_id = $2) \
           SELECT id FROM fam",
      )
      .bind(root)
      .bind(ws)
      .fetch_all(&mut *conn)
      .await
      .map_err(db_err)?;
      let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
          "SELECT id, namespace, flow_key FROM workflow.executions \
            WHERE workspace_id = $1 AND id = ANY($2) ORDER BY id FOR UPDATE",
      )
      .bind(ws)
      .bind(&ids)
      .fetch_all(&mut *conn)
      .await
      .map_err(db_err)?;
      Ok(rows
          .into_iter()
          .map(|(id, namespace, flow_key)| Member { id, namespace, flow_key })
          .collect())
  }

  /// How many executions outside `family` point at a member, and the first
  /// `MAX_NAMED_DEPENDENTS` of them, oldest first. Both columns are indexed
  /// (0138:245, 0166:72). Called after the S8 detach, so a Flow-trigger fire
  /// is not among them.
  async fn dependents(conn: &mut PgConnection, ws: Uuid, family: &[Uuid]) -> Result<(i64, Vec<Uuid>)> {
      let rows: Vec<(Uuid, i64)> = sqlx::query_as(
          "SELECT id, count(*) OVER () FROM workflow.executions \
            WHERE workspace_id = $1 AND NOT (id = ANY($2)) \
              AND (trigger_execution_id = ANY($2) OR replayed_from_execution_id = ANY($2)) \
            ORDER BY created_at, id LIMIT $3",
      )
      .bind(ws)
      .bind(family)
      .bind(MAX_NAMED_DEPENDENTS)
      .fetch_all(&mut *conn)
      .await
      .map_err(db_err)?;
      let total = rows.first().map_or(0, |(_, n)| *n);
      Ok((total, rows.into_iter().map(|(id, _)| id).collect()))
  }

  async fn delete_prefix(fs: &FileStore, execution: Uuid, prefix: &str) -> Result<()> {
      fs.store.delete_prefix(prefix).await.map(drop).map_err(|e| {
          Error::Internal(anyhow::anyhow!(
              "deleting the stored files of execution {execution} failed, so nothing was deleted: {e}"
          ))
      })
  }

  fn db_err(e: sqlx::Error) -> Error {
      Error::Internal(anyhow::Error::new(e))
  }
  ```
  `&mut tx` derefs to `&mut PgConnection` for the helpers (sqlx 0.9 `Transaction: DerefMut`); if the compiler wants it, pass `&mut *tx`. If `own_prefix`'s path is not reachable from `worker` (`pub(crate)` in `pub mod prune`), it is: `crate::storage::prune::own_prefix`.
- [ ] Run, one at a time:
  - `cargo test -p egret-workflow --test delete_labels`
  - `cargo test -p egret-workflow --test storage_prune --test task_logs`
  - `cargo clippy -p egret-workflow --all-targets -- -D warnings`
- [ ] Commit: `feat(workflow): delete an ended execution and its family under the lock (package 7d)`.

**Tests** (`tests/delete_labels.rs`). Module doc: the package, the spec path, "Every delete test owns its store: a `MemStore` built here, never the installed global" and "Run with: export DATABASE_URL=… ; cargo test -p egret-workflow --test delete_labels". Fixtures: `setup`, `file_flow`, `settle_everything`, `handlers`, `exec_state` from `tests/pause_resume.rs:40-196`; a test that makes an ended run live again also clears its `ended_at` (migration 0183's CHECK `ended_at IS NULL OR state_is_terminal(state)`); `listed_under` from `tests/pause_resume_routes.rs:194-206`; `one_task`, `subflow_parent`, `LOOPY` from `tests/pause_resume.rs:30-95` and `tests/pause_resume_routes.rs:83-96`. Then:

```rust
use std::sync::Arc;

use egret_storage::{MemStore, ObjectStore, StorageError};
use egret_workflow::storage::{Caps, FileStore};
use egret_workflow::worker::{delete_execution, DeleteRefused, DeleteRequest};

async fn start(pool: &sqlx::PgPool, db: &Db, ws: Uuid, user: egret_core::UserId, src: &str) -> Uuid {
    let flow = file_flow(db, ws, user, src).await;
    egret_workflow::worker::start_execution(pool, ws, Some(flow), src, serde_json::json!({}))
        .await
        .expect("start")
}

/// This test's own store (never the installed one).
fn mem_fs(store: Arc<dyn ObjectStore>) -> FileStore {
    FileStore {
        store,
        bucket: "egret-7d".to_string(),
        prefix: "workflow/".to_string(),
        caps: Caps::default(),
        retention_days: 30,
    }
}

/// A store whose `delete_prefix` is down; everything else is a `MemStore`.
struct NoDelete(MemStore);

#[async_trait::async_trait]
impl ObjectStore for NoDelete {
    async fn put(&self, uri: &str, bytes: Vec<u8>) -> Result<(), StorageError> {
        self.0.put(uri, bytes).await
    }
    async fn get(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.0.get(uri).await
    }
    async fn delete(&self, uri: &str) -> Result<(), StorageError> {
        self.0.delete(uri).await
    }
    async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError> {
        self.0.copy(src, dst).await
    }
    async fn list(&self, prefix: &str, delimiter: Option<&str>) -> Result<egret_storage::Listing, StorageError> {
        self.0.list(prefix, delimiter).await
    }
    async fn delete_prefix(&self, uri: &str) -> Result<u64, StorageError> {
        Err(StorageError::S3(format!("{uri}: dispatch failure: connection refused")))
    }
}

fn req(ws: Uuid, id: Uuid, delete_storage: bool) -> DeleteRequest {
    DeleteRequest { workspace: ws, id, delete_storage }
}

async fn exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM workflow.executions WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Each member's (id, flow_key): the root and its iterations.
async fn members(pool: &sqlx::PgPool, root: Uuid) -> Vec<(Uuid, String)> {
    sqlx::query_as(
        "SELECT id, flow_key FROM workflow.executions WHERE id = $1 OR parent_execution_id = $1 ORDER BY id",
    )
    .bind(root)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// One object under the member's own prefix and one under its `_wd/` root.
async fn seed_objects(fs: &FileStore, ws: Uuid, id: Uuid, flow_key: &str) -> (String, String) {
    let own = fs.execution_prefix(ws, "t", flow_key, id);
    let wd = fs.working_dir_root(ws, id);
    fs.store.put(&format!("{own}tasks/a/f.txt"), b"x".to_vec()).await.unwrap();
    fs.store.put(&format!("{wd}blk/f.txt"), b"x".to_vec()).await.unwrap();
    (own, wd)
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_ended_loop_root_is_deleted_with_its_family_rows_and_objects(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let fs = mem_fs(Arc::new(MemStore::new()));
    let root = start(&pool, &db, ws, user, LOOPY).await;
    let other = start(&pool, &db, ws, user, &one_task("other")).await;
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, root).await, "SUCCESS");
    let family = members(&pool, root).await;
    assert_eq!(family.len(), 3, "the root and two iterations");
    let mut prefixes = Vec::new();
    for (id, flow_key) in &family {
        prefixes.push(seed_objects(&fs, ws, *id, flow_key).await);
    }
    let (kept, _) = seed_objects(&fs, ws, other, "other").await;

    // Act
    let done = delete_execution(&pool, Some(&fs), req(ws, root, true))
        .await
        .expect("delete")
        .expect("not refused");

    // Assert
    assert_eq!((done.executions, done.state.as_str(), done.flow_key.as_str()), (3, "SUCCESS", "loopy"));
    for (id, _) in &family {
        assert!(!exists(&pool, *id).await, "{id} is gone");
    }
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.task_runs WHERE execution_id = ANY($1)")
        .bind(family.iter().map(|(id, _)| *id).collect::<Vec<_>>())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 0);
    for (own, wd) in &prefixes {
        assert!(listed_under(&fs, own).await.objects.is_empty(), "{own}");
        assert!(listed_under(&fs, wd).await.objects.is_empty(), "{wd}");
    }
    assert_eq!(listed_under(&fs, &kept).await.objects.len(), 1, "another execution's files stay");
    assert!(exists(&pool, other).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_storage_false_keeps_the_objects(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let fs = mem_fs(Arc::new(MemStore::new()));
    let ex = start(&pool, &db, ws, user, &one_task("one")).await;
    settle_everything(&pool, &handlers()).await;
    let (own, wd) = seed_objects(&fs, ws, ex, "one").await;

    delete_execution(&pool, Some(&fs), req(ws, ex, false)).await.unwrap().unwrap();

    assert!(!exists(&pool, ex).await);
    assert_eq!(listed_under(&fs, &own).await.objects.len(), 1);
    assert_eq!(listed_under(&fs, &wd).await.objects.len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_store_failure_deletes_no_row_and_can_be_repeated(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let down = mem_fs(Arc::new(NoDelete(MemStore::new())));
    let ex = start(&pool, &db, ws, user, &one_task("one")).await;
    settle_everything(&pool, &handlers()).await;
    seed_objects(&down, ws, ex, "one").await;

    // Act
    let err = delete_execution(&pool, Some(&down), req(ws, ex, true)).await.unwrap_err();

    // Assert
    assert!(matches!(err, egret_core::Error::Internal(_)), "{err}");
    assert!(exists(&pool, ex).await, "the row stays");
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.task_runs WHERE execution_id = $1")
        .bind(ex)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 1, "and its runs");
    // Repeated against a working store, it goes.
    let up = mem_fs(Arc::new(MemStore::new()));
    delete_execution(&pool, Some(&up), req(ws, ex, true)).await.unwrap().unwrap();
    assert!(!exists(&pool, ex).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_running_execution_is_not_idle_and_is_kept(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &one_task("one")).await; // not ticked: RUNNING
    let got = delete_execution(&pool, None, req(ws, ex, true)).await.unwrap();
    assert_eq!(got, Err(DeleteRefused::NotIdle { execution: ex, state: "RUNNING".into() }));
    assert!(exists(&pool, ex).await);
}

/// Idle, not terminal (Q1): an ended execution with a live run (an
/// `afterExecution:` run, an `Exit` sibling) is refused, naming its state.
#[sqlx::test(migrations = "../../migrations")]
async fn an_ended_execution_with_a_live_run_is_not_idle(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &one_task("one")).await;
    settle_everything(&pool, &handlers()).await;
    sqlx::query("UPDATE workflow.task_runs SET state = 'RUNNING', ended_at = NULL WHERE execution_id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    let got = delete_execution(&pool, None, req(ws, ex, true)).await.unwrap();
    assert_eq!(got, Err(DeleteRefused::NotIdle { execution: ex, state: "SUCCESS".into() }));
}

/// The family is checked, not the root alone (S2).
#[sqlx::test(migrations = "../../migrations")]
async fn a_live_iteration_keeps_its_root(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let root = start(&pool, &db, ws, user, LOOPY).await;
    settle_everything(&pool, &handlers()).await;
    let iteration: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 ORDER BY loop_index LIMIT 1",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE workflow.task_runs SET state = 'RUNNING', ended_at = NULL WHERE execution_id = $1")
        .bind(iteration)
        .execute(&pool)
        .await
        .unwrap();
    let got = delete_execution(&pool, None, req(ws, root, true)).await.unwrap();
    assert_eq!(got, Err(DeleteRefused::NotIdle { execution: iteration, state: "SUCCESS".into() }));
    assert!(exists(&pool, root).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_loop_iteration_is_not_a_root(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let root = start(&pool, &db, ws, user, LOOPY).await;
    settle_everything(&pool, &handlers()).await;
    let iteration: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 LIMIT 1",
    )
    .bind(root)
    .fetch_one(&pool)
    .await
    .unwrap();
    let got = delete_execution(&pool, None, req(ws, iteration, true)).await.unwrap();
    assert_eq!(got, Err(DeleteRefused::NotARoot { root }));
    assert!(exists(&pool, iteration).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_parent_has_its_child_as_a_dependent_until_the_child_goes(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    file_flow(&db, ws, user, &one_task("child")).await;
    let parent = start(&pool, &db, ws, user, &subflow_parent("parent", "child", "")).await;
    settle_everything(&pool, &handlers()).await;
    assert_eq!(exec_state(&pool, parent).await, "SUCCESS");
    let child: Uuid = sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
        .bind(parent)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Act + Assert: refused, naming the child; the child first; then the parent.
    let got = delete_execution(&pool, None, req(ws, parent, true)).await.unwrap();
    assert_eq!(got, Err(DeleteRefused::HasDependents { executions: vec![child], total: 1 }));
    assert!(exists(&pool, parent).await);
    delete_execution(&pool, None, req(ws, child, true)).await.unwrap().unwrap();
    delete_execution(&pool, None, req(ws, parent, true)).await.unwrap().unwrap();
    assert!(!exists(&pool, parent).await);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replay_is_a_dependent_of_what_it_replayed(pool: sqlx::PgPool) {
    // Arrange
    let (db, ws, user) = setup(&pool).await;
    let src = one_task("one");
    let original = start(&pool, &db, ws, user, &src).await;
    settle_everything(&pool, &handlers()).await;
    let replay = egret_workflow::worker::replay_execution(
        &pool,
        &egret_workflow::worker::ReplayRequest {
            workspace_id: ws,
            from: original,
            source: &src,
            kind: egret_workflow::worker::ReplayKind::Whole,
            inputs: egret_workflow::worker::ReplayInputs::Reuse,
            started_by: user.0,
            id: Uuid::new_v4(),
            input_files: Vec::new(),
        },
    )
    .await
    .expect("replay")
    .expect("not refused");
    settle_everything(&pool, &handlers()).await;

    // Act + Assert
    assert_eq!(
        delete_execution(&pool, None, req(ws, original, true)).await.unwrap(),
        Err(DeleteRefused::HasDependents { executions: vec![replay], total: 1 })
    );
    assert!(exists(&pool, original).await);
    delete_execution(&pool, None, req(ws, replay, true)).await.unwrap().unwrap();
    delete_execution(&pool, None, req(ws, original, true)).await.unwrap().unwrap();
    assert!(!exists(&pool, original).await);
}

/// S9 default (a).
#[sqlx::test(migrations = "../../migrations")]
async fn s9_a_pending_flow_trigger_event_refuses_the_delete(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, &one_task("one")).await;
    settle_everything(&pool, &handlers()).await;
    sqlx::query("INSERT INTO workflow.flow_trigger_events (source_execution_id, workspace_id, state) VALUES ($1, $2, 'SUCCESS')")
        .bind(ex)
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(delete_execution(&pool, None, req(ws, ex, true)).await.unwrap(), Err(DeleteRefused::ListenersPending));
    sqlx::query("DELETE FROM workflow.flow_trigger_events WHERE source_execution_id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    delete_execution(&pool, None, req(ws, ex, true)).await.unwrap().unwrap();
}

/// S8 (user decision (b)): a Flow-trigger fire is not a dependent. The
/// source is deleted; the fire is kept, its `trigger_execution_id` cleared,
/// and its trigger record still names the source as data.
#[sqlx::test(migrations = "../../migrations")]
async fn s8_a_flow_trigger_fire_is_detached_not_a_dependent(pool: sqlx::PgPool) {
    // Arrange: a listener that fires on `producer`'s end (the fixtures of
    // tests/flow_trigger_fire.rs:47-73; `seed_flow` registers the listener).
    let (db, ws, user) = setup(&pool).await;
    let producer_src = "id: producer\nnamespace: w13\ntasks:\n  \
                        - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let listener_src = "id: listener\nnamespace: w13\ntriggers:\n  \
                        - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    \
                        when: \"{{ flow.id == 'producer' }}\"\ntasks:\n  \
                        - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    let producer = common::seed_flow(&db, ws, user, producer_src, "draft").await;
    common::seed_flow(&db, ws, user, listener_src, "draft").await;
    let source = egret_workflow::worker::start_execution(&pool, ws, Some(producer), producer_src, serde_json::json!({}))
        .await
        .expect("start the source");
    settle_everything(&pool, &handlers()).await;
    let report = egret_workflow::flow_trigger::process_events(&pool).await.expect("a pass");
    assert_eq!(report.fired, 1, "control: the listener fired");
    let fired: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE trigger_execution_id = $1 AND flow_trigger_event_id IS NOT NULL",
    )
    .bind(source)
    .fetch_one(&pool)
    .await
    .expect("the fire");
    settle_everything(&pool, &handlers()).await;
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM workflow.flow_trigger_events WHERE source_execution_id = $1")
        .bind(source)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "control: the fire consumed the source's event (else S9 would refuse)");

    // Act
    let done = delete_execution(&pool, None, req(ws, source, true))
        .await
        .unwrap()
        .expect("a fire does not block the delete");

    // Assert
    assert_eq!(done.detached, 1);
    assert!(!exists(&pool, source).await);
    assert!(exists(&pool, fired).await, "the fire is kept");
    let (link, record): (Option<Uuid>, serde_json::Value) = sqlx::query_as(
        "SELECT trigger_execution_id, execution_trigger FROM workflow.executions WHERE id = $1",
    )
    .bind(fired)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(link, None);
    assert!(record.to_string().contains(&source.to_string()), "the record still names the source: {record}");
}

/// Control for S8: the detach is rolled back when another dependent refuses
/// the delete (a subflow child here), so nothing is half-done.
#[sqlx::test(migrations = "../../migrations")]
async fn s8_a_refused_delete_keeps_the_fires_link(pool: sqlx::PgPool) {
    // Arrange: the S8 test's listener, listening to a subflow parent.
    let (db, ws, user) = setup(&pool).await;
    file_flow(&db, ws, user, &one_task("child")).await;
    let parent_src = subflow_parent("parent", "child", "");
    let parent_flow = file_flow(&db, ws, user, &parent_src).await;
    let listener_src = "id: listener\nnamespace: t\ntriggers:\n  \
                        - id: on_done\n    type: io.kestra.plugin.core.trigger.Flow\n    \
                        when: \"{{ flow.id == 'parent' }}\"\ntasks:\n  \
                        - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";
    common::seed_flow(&db, ws, user, listener_src, "draft").await;
    let parent = egret_workflow::worker::start_execution(&pool, ws, Some(parent_flow), &parent_src, serde_json::json!({}))
        .await
        .unwrap();
    settle_everything(&pool, &handlers()).await;
    egret_workflow::flow_trigger::process_events(&pool).await.unwrap();
    settle_everything(&pool, &handlers()).await;
    let child: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE trigger_execution_id = $1 AND flow_trigger_event_id IS NULL",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .unwrap();

    // Act
    let got = delete_execution(&pool, None, req(ws, parent, true)).await.unwrap();

    // Assert: refused for the child alone; the fire still points at the parent.
    assert_eq!(got, Err(DeleteRefused::HasDependents { executions: vec![child], total: 1 }));
    let still: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.executions WHERE trigger_execution_id = $1 AND flow_trigger_event_id IS NOT NULL",
    )
    .bind(parent)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(still, 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_foreign_workspace_cannot_delete(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let (other_ws, _) = common::seed_workspace(&db).await;
    let ex = start(&pool, &db, ws, user, &one_task("one")).await;
    settle_everything(&pool, &handlers()).await;
    let err = delete_execution(&pool, None, req(other_ws, ex, true)).await.unwrap_err();
    assert!(matches!(err, egret_core::Error::NotFound(_)), "{err}");
    assert!(exists(&pool, ex).await);
}

/// The family includes a `CREATE_NEW_EXECUTION` retry replacement (S2):
/// it is not a root, it cascades with the original, and its objects go too.
#[sqlx::test(migrations = "../../migrations")]
async fn a_retry_replacement_goes_with_its_root(pool: sqlx::PgPool) {
    // Arrange: tests/execution_timestamps.rs:469-498's flow and its `Fails`
    // registry (copied below), so the original fails and is replaced once.
    let (db, ws, user) = setup(&pool).await;
    let fs = mem_fs(Arc::new(MemStore::new()));
    let src = "id: cne\nnamespace: t\ntasks:\n  - id: a\n    type: io.kestra.plugin.core.log.Log\n\
               \x20   message: hi\n    retry:\n      behavior: CREATE_NEW_EXECUTION\n\
               \x20     type: constant\n      interval: PT1S\n      maxAttempts: 2\n";
    let original = start(&pool, &db, ws, user, src).await;
    settle_everything(&pool, &Fails).await;
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    egret_workflow::worker::wake(&pool, 16).await.expect("wake");
    settle_everything(&pool, &Fails).await;
    let replacement: Uuid = sqlx::query_scalar(
        "SELECT id FROM workflow.executions WHERE parent_execution_id = $1 AND parent_run_ix IS NULL",
    )
    .bind(original)
    .fetch_one(&pool)
    .await
    .expect("the replacement exists");
    let (own_o, _) = seed_objects(&fs, ws, original, "cne").await;
    let (own_r, _) = seed_objects(&fs, ws, replacement, "cne").await;

    // Act + Assert: the replacement alone is refused, naming the original.
    assert_eq!(
        delete_execution(&pool, Some(&fs), req(ws, replacement, true)).await.unwrap(),
        Err(DeleteRefused::NotARoot { root: original })
    );
    let done = delete_execution(&pool, Some(&fs), req(ws, original, true))
        .await
        .unwrap()
        .expect("deleted");
    assert_eq!(done.executions, 2);
    assert!(!exists(&pool, original).await && !exists(&pool, replacement).await);
    assert!(listed_under(&fs, &own_o).await.objects.is_empty());
    assert!(listed_under(&fs, &own_r).await.objects.is_empty());
}

/// tests/execution_timestamps.rs:56-70, without the sleep.
struct Fails;

#[async_trait::async_trait]
impl egret_workflow::worker::Registry for Fails {
    async fn run(&self, _task: &egret_workflow::worker::TaskInput) -> egret_core::Result<egret_workflow::worker::Outcome> {
        Ok(egret_workflow::worker::Outcome::failed_with("this fixture always fails"))
    }
    async fn cancel(&self, task: &egret_workflow::worker::TaskInput) -> egret_core::Result<()> {
        panic!("nothing here has a timeout, so {} is never killed", task.task_id);
    }
}
```
`settle_everything` takes `&dyn Registry` (`tests/pause_resume.rs:63`), so it drives `Fails` too. Plus `database_url_is_exported_rather_than_read_from_dotenv`.

---

## T4. Worker: replace an execution's labels under its lock (spec Q3, Q8, D5)

**Files:**
- Create: `crates/egret-workflow/src/worker/labels.rs`.
- Modify: `src/worker/mod.rs` (`mod labels;` after `mod kill;`; `pub use labels::{set_execution_labels, LabelsRefused, LabelsSet};`), `tests/delete_labels.rs` (tests below).

**Interfaces.**

Produces:
```rust
// worker/labels.rs, re-exported from `worker`
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelsSet {
    /// The visible keys before, in order.
    pub keys_before: Vec<String>,
    /// The stored keys after (the body, then the kept `system.*`).
    pub keys_after: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LabelsRefused {
    /// D5: not idle; the execution's state.
    NotIdle(String),
    /// D5: the sentence of `labels::edit_labels_refusal`.
    Invalid(String),
}

/// `Err(NotFound)` for a missing or foreign id.
pub async fn set_execution_labels(
    pool: &PgPool,
    workspace: Uuid,
    id: Uuid,
    body: &[crate::models::Label],
) -> Result<std::result::Result<LabelsSet, LabelsRefused>>;
```

**Steps.**

- [ ] **Write the failing tests** (below). Run `cargo test -p egret-workflow --test delete_labels`: it does not compile.
- [ ] **Implement `worker/labels.rs`:**
  ```rust
  //! **Replace an execution's labels** -- workflow package 7d (spec
  //! `docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md` Q3, Q8,
  //! D5; Kestra `EC:2352-2402`). Under the execution's row lock in the mode
  //! `claim::load` takes (`FOR NO KEY UPDATE`), so it serialises with a
  //! settle and a restart; not `claim::load` itself, which plans the source
  //! (plan S5). The body replaces the visible list; the visible `system.*`
  //! labels are kept; `labels_replaced` becomes true (migration 0231).

  use egret_core::{Error, Result};
  use sqlx::{PgPool, Row};
  use uuid::Uuid;

  use crate::labels::{edit_labels_refusal, merge_system_labels, own_labels, visible_labels};
  use crate::models::Label;

  // (the types of the Interfaces block, with their docs)

  pub async fn set_execution_labels(
      pool: &PgPool,
      workspace: Uuid,
      id: Uuid,
      body: &[Label],
  ) -> Result<std::result::Result<LabelsSet, LabelsRefused>> {
      let mut tx = pool.begin().await.map_err(db_err)?;
      let row = sqlx::query(
          "SELECT state, source, labels, labels_replaced, \
                  COALESCE(workflow.execution_is_idle(id), false) AS idle \
             FROM workflow.executions WHERE id = $1 AND workspace_id = $2 FOR NO KEY UPDATE",
      )
      .bind(id)
      .bind(workspace)
      .fetch_optional(&mut *tx)
      .await
      .map_err(db_err)?
      .ok_or_else(|| Error::NotFound("execution".into()))?;
      if !row.get::<bool, _>("idle") {
          return Ok(Err(LabelsRefused::NotIdle(row.get("state"))));
      }
      let flow_labels = crate::spec::parse(row.get::<&str, _>("source"))
          .map(|f| f.labels)
          .unwrap_or_default();
      let own = own_labels(row.get::<Option<serde_json::Value>, _>("labels").as_ref());
      let visible = visible_labels(&flow_labels, &own, row.get("labels_replaced"));
      if let Some(why) = edit_labels_refusal(body, &visible) {
          return Ok(Err(LabelsRefused::Invalid(why)));
      }
      let stored = merge_system_labels(body, &visible);
      sqlx::query(
          "UPDATE workflow.executions SET labels = $2, labels_replaced = true WHERE id = $1",
      )
      .bind(id)
      .bind(serde_json::to_value(&stored).expect("labels serialise"))
      .execute(&mut *tx)
      .await
      .map_err(db_err)?;
      tx.commit().await.map_err(db_err)?;
      Ok(Ok(LabelsSet {
          keys_before: visible.into_iter().map(|l| l.key).collect(),
          keys_after: stored.into_iter().map(|l| l.key).collect(),
      }))
  }

  fn db_err(e: sqlx::Error) -> Error {
      Error::Internal(anyhow::Error::new(e))
  }
  ```
  `models::Label` is `Serialize` (`labels.rs` and `claim.rs:5244` serialise it). An empty body stores `[]` with the flag true: every non-system label is removed, as in Kestra.
- [ ] Run `cargo test -p egret-workflow --test delete_labels`, then `cargo clippy -p egret-workflow --all-targets -- -D warnings`.
- [ ] Commit: `feat(workflow): replace an ended execution's labels under its lock (package 7d)`.

**Tests** (in `tests/delete_labels.rs`):

```rust
use egret_workflow::worker::{set_execution_labels, LabelsRefused, LabelsSet};

const LABELLED: &str = "id: labelled\nnamespace: t\nlabels:\n  team: data\n  Team2: x\ntasks:\n  \
     - id: a\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";

fn label(k: &str, v: &str) -> egret_workflow::models::Label {
    egret_workflow::models::Label { key: k.into(), value: v.into() }
}

async fn stored(pool: &sqlx::PgPool, ex: Uuid) -> (Option<serde_json::Value>, bool) {
    sqlx::query_as("SELECT labels, labels_replaced FROM workflow.executions WHERE id = $1")
        .bind(ex)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_replace_removes_a_flow_label_and_keeps_the_system_ones(pool: sqlx::PgPool) {
    // Arrange: ended, and restarted once (the engine's own label).
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    sqlx::query("UPDATE workflow.executions SET labels = $2 WHERE id = $1")
        .bind(ex)
        .bind(serde_json::json!([{"key": "system.restarted", "value": "true"}]))
        .execute(&pool)
        .await
        .unwrap();

    // Act: `team` is left out, `Team2` (a flow key outside Kestra's pattern) resent.
    let got = set_execution_labels(&pool, ws, ex, &[label("Team2", "x"), label("env", "dev")])
        .await
        .unwrap();

    // Assert
    assert_eq!(
        got,
        Ok(LabelsSet {
            keys_before: vec!["team".into(), "Team2".into(), "system.restarted".into()],
            keys_after: vec!["Team2".into(), "env".into(), "system.restarted".into()],
        })
    );
    assert_eq!(
        stored(&pool, ex).await,
        (
            Some(serde_json::json!([
                {"key": "Team2", "value": "x"},
                {"key": "env", "value": "dev"},
                {"key": "system.restarted", "value": "true"}
            ])),
            true
        )
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_second_replace_works_on_the_replaced_list(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    set_execution_labels(&pool, ws, ex, &[label("env", "dev")]).await.unwrap().unwrap();
    // `team` is no longer visible, so a new key `Team` must match the pattern.
    let got = set_execution_labels(&pool, ws, ex, &[label("Team", "x")]).await.unwrap();
    assert!(matches!(got, Err(LabelsRefused::Invalid(ref m)) if m.contains("Team")), "{got:?}");
    let got = set_execution_labels(&pool, ws, ex, &[]).await.unwrap().unwrap();
    assert_eq!(got.keys_before, vec!["env".to_string()]);
    assert_eq!(stored(&pool, ex).await, (Some(serde_json::json!([])), true));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_running_execution_cannot_be_relabelled(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await; // RUNNING
    let got = set_execution_labels(&pool, ws, ex, &[label("env", "dev")]).await.unwrap();
    assert_eq!(got, Err(LabelsRefused::NotIdle("RUNNING".into())));
    assert_eq!(stored(&pool, ex).await, (None, false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_changed_or_new_system_label_is_refused_and_nothing_is_written(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    for body in [vec![label("system.restarted", "true")], vec![label("system.replay", "x")]] {
        let got = set_execution_labels(&pool, ws, ex, &body).await.unwrap();
        assert!(matches!(got, Err(LabelsRefused::Invalid(ref m)) if m.contains(&body[0].key)), "{got:?}");
    }
    assert_eq!(stored(&pool, ex).await, (None, false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_foreign_workspace_cannot_relabel(pool: sqlx::PgPool) {
    let (db, ws, user) = setup(&pool).await;
    let (other_ws, _) = common::seed_workspace(&db).await;
    let ex = start(&pool, &db, ws, user, LABELLED).await;
    settle_everything(&pool, &handlers()).await;
    let err = set_execution_labels(&pool, other_ws, ex, &[]).await.unwrap_err();
    assert!(matches!(err, egret_core::Error::NotFound(_)), "{err}");
}
```

`egret_workflow::models::Label` is reachable from the test crate (`pub mod models`, `lib.rs:65`; `Label` is `models/flow.rs:176-180`). The flow's YAML `labels:` map must accept the key `Team2`; if `spec::parse` refuses an upper-case key, use `9lives` instead.

---

## T5. Logs: delete by filter, the download's line and batch, `idle` on the log status (spec Q4, Q6, D7, D8; S11, S12)

**Files:**
- Modify: `crates/egret-workflow/src/logs/repo.rs` (after `LogQuery`, `:80-89`; after `page`, `:246-279`), `src/logs/mod.rs` (re-exports `:22`; the new line helper and constants), `src/repo.rs:1546-1566` (`log_status`) and `:1677-1683` (`ExecutionLogStatus`), `tests/task_logs.rs`.

**Interfaces.**

Produces:
```rust
// logs/repo.rs, re-exported from `logs`
/// The filters of `DELETE …/logs` (D8) and `GET …/logs/download` (D7), ANDed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogFilter {
    pub task_id: Option<String>,
    pub task_run_id: Option<Uuid>,
    /// 0-based, as `StoredLine::attempt` (plan S15). The download has none.
    pub attempt: Option<i32>,
    /// This level and above.
    pub min_level: Option<LogLevel>,
}
impl LogRepo {
    /// D8: deletes the matching lines of `execution` in `ws`; how many.
    pub async fn delete_for_execution(&self, ws: Uuid, execution: Uuid, f: &LogFilter) -> Result<u64>;
    /// D7, S11: the matching lines after `after`, in id order, at most
    /// `limit`. No settle watermark: a download reads what is committed.
    pub async fn download_batch(&self, ws: Uuid, execution: Uuid, f: &LogFilter,
        after: Option<i64>, limit: i64) -> Result<Vec<StoredLine>>;
}

// logs/mod.rs
pub const LOG_DOWNLOAD_MAX_LINES: i64 = 100_000;
pub const LOG_DOWNLOAD_BATCH: i64 = 2_000;
/// The download's last line when it stops at the cap.
pub fn truncated_line() -> String; // "… truncated at 100000 lines"
/// Kestra `LogEntry.toPrettyString`: `<ts RFC 3339 UTC, millis> <LEVEL> <message>`.
pub fn download_line(line: &StoredLine) -> String;

// repo.rs
pub struct ExecutionLogStatus {
    pub state: String,
    pub complete: bool,
    /// S12.
    pub idle: bool,
}
```

**Steps.**

- [ ] **Write the failing tests** (below). Run `cargo test -p egret-workflow --lib logs::` and `cargo test -p egret-workflow --test task_logs`: they do not compile.
- [ ] **`logs/mod.rs`:**
  ```rust
  /// Package 7d D7: the most lines `GET …/executions/{id}/logs/download`
  /// writes (the frontend's former client-side cap).
  pub const LOG_DOWNLOAD_MAX_LINES: i64 = 100_000;
  /// How many lines the download reads per statement (plan S11): it streams,
  /// so 100 000 lines of up to 15 360 characters are never held at once.
  pub const LOG_DOWNLOAD_BATCH: i64 = 2_000;

  /// The download's last line when there were more lines than the cap.
  pub fn truncated_line() -> String {
      format!("… truncated at {LOG_DOWNLOAD_MAX_LINES} lines")
  }

  /// One line of the download (D7; Kestra `LogEntry.toPrettyString`,
  /// `LogEntry.java:136-138`): the instant in RFC 3339 UTC with
  /// milliseconds, the level, the message. Lines are joined with `\n` and
  /// the file has no trailing newline, as Kestra's.
  pub fn download_line(line: &StoredLine) -> String {
      format!(
          "{} {} {}",
          line.ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
          line.level.as_str(),
          line.message
      )
  }
  ```
  Re-export `LogFilter` beside `LogQuery` (`:22`).
- [ ] **`logs/repo.rs`:** `LogFilter` after `LogQuery`, then in `impl LogRepo` after `page`:
  ```rust
  /// **`DELETE …/executions/{id}/logs`** (package 7d D8; Kestra
  /// `deleteByQuery`, `AbstractJdbcLogDataStore.java:318-349`): every line
  /// of `execution` in `ws` that matches every given filter. Both ids are in
  /// the predicate, as in [`Self::page`]. No state check (Q6): a line
  /// written after the statement stays.
  pub async fn delete_for_execution(&self, ws: Uuid, execution: Uuid, f: &LogFilter) -> Result<u64> {
      sqlx::query(
          "DELETE FROM workflow.task_logs \
            WHERE workspace_id = $1 AND execution_id = $2 \
              AND ($3::text IS NULL OR task_id = $3) \
              AND ($4::uuid IS NULL OR task_run_id = $4) \
              AND ($5::int4 IS NULL OR attempt = $5) \
              AND ($6::int2 IS NULL OR level >= $6)",
      )
      .bind(ws)
      .bind(execution)
      .bind(f.task_id.as_deref())
      .bind(f.task_run_id)
      .bind(f.attempt)
      .bind(f.min_level.map(LogLevel::as_i16))
      .execute(&self.pool)
      .await
      .map(|r| r.rows_affected())
      .map_err(db_err)
  }

  /// **One batch of `GET …/logs/download`** (package 7d D7, S11): the
  /// matching lines after `after`, in id order, at most `limit`. Unlike
  /// [`Self::page`] there is no settle watermark: a download is one read of
  /// what is committed now, not a cursor a client keeps.
  pub async fn download_batch(
      &self,
      ws: Uuid,
      execution: Uuid,
      f: &LogFilter,
      after: Option<i64>,
      limit: i64,
  ) -> Result<Vec<StoredLine>> {
      let rows = sqlx::query(
          "SELECT id, task_run_id, task_id, attempt, level, ts, message \
             FROM workflow.task_logs \
            WHERE workspace_id = $1 AND execution_id = $2 AND id > COALESCE($3, 0) \
              AND ($4::text IS NULL OR task_id = $4) \
              AND ($5::uuid IS NULL OR task_run_id = $5) \
              AND ($6::int4 IS NULL OR attempt = $6) \
              AND ($7::int2 IS NULL OR level >= $7) \
            ORDER BY id \
            LIMIT $8",
      )
      .bind(ws)
      .bind(execution)
      .bind(after)
      .bind(f.task_id.as_deref())
      .bind(f.task_run_id)
      .bind(f.attempt)
      .bind(f.min_level.map(LogLevel::as_i16))
      .bind(limit.max(0))
      .fetch_all(&self.pool)
      .await
      .map_err(db_err)?;
      rows.iter().map(stored).collect()
  }
  ```
- [ ] **`repo.rs` (S12):** `log_status`'s SELECT becomes `SELECT e.state, COALESCE(workflow.execution_is_idle(e.id), false) AS idle, <the existing complete expression> AS complete …`, read as `(String, bool, bool)`, and `ExecutionLogStatus` gains
  ```rust
  /// Package 7d D7: terminal, and no task run live (`workflow.execution_is_idle`);
  /// `complete` without the settle lag.
  pub idle: bool,
  ```
  Fix every construction the compiler names (the struct is built only in `log_status`).
- [ ] Run, one at a time:
  - `cargo test -p egret-workflow --lib logs::`
  - `cargo test -p egret-workflow --test task_logs`
  - `cargo clippy -p egret-workflow --all-targets -- -D warnings`
- [ ] Commit: `feat(workflow): delete an execution's log lines by filter; the download's line and batch (package 7d)`.

**Unit tests** (`logs/mod.rs`, a `#[cfg(test)] mod download_tests` at the end):

```rust
#[cfg(test)]
mod download_tests {
    use super::*;
    use chrono::TimeZone;

    fn line(level: LogLevel, ms: u32, message: &str) -> StoredLine {
        StoredLine {
            id: 1,
            task_run_id: None,
            task_id: None,
            attempt: None,
            level,
            ts: chrono::Utc
                .with_ymd_and_hms(2026, 11, 16, 10, 0, 0)
                .unwrap()
                + chrono::Duration::milliseconds(i64::from(ms)),
            message: message.to_string(),
        }
    }

    #[test]
    fn a_download_line_is_kestras_pretty_string_with_millis() {
        assert_eq!(
            download_line(&line(LogLevel::Info, 123, "hello")),
            "2026-11-16T10:00:00.123Z INFO hello"
        );
        // Whole seconds keep the three digits; a multi-line message is kept as it is.
        assert_eq!(
            download_line(&line(LogLevel::Error, 0, "a\nb")),
            "2026-11-16T10:00:00.000Z ERROR a\nb"
        );
    }

    #[test]
    fn the_truncation_line_names_the_cap() {
        assert_eq!(truncated_line(), "… truncated at 100000 lines");
    }
}
```

**Repo tests** (`tests/task_logs.rs`, in its style: `#[tokio::test]`, `let Some(fx) = fx().await else { return };`, `fx.line(..)`, `fx.repo.insert_batch(..)`):

```rust
/// Package 7d D8: the filters AND together; `min_level` is that level and above.
#[tokio::test]
async fn delete_for_execution_ands_its_filters() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let other = fx.execution().await;
    let (r1, r2) = (Uuid::new_v4(), Uuid::new_v4());
    let mk = |task: Option<&str>, run: Option<Uuid>, attempt: Option<i32>, level: LogLevel, m: &str| {
        let mut l = fx.line(exec, level, m);
        l.task_id = task.map(str::to_string);
        l.task_run_id = run;
        l.attempt = attempt;
        l
    };
    fx.repo
        .insert_batch(&[
            mk(None, None, None, LogLevel::Error, "exec-error"),
            mk(Some("a"), Some(r1), Some(0), LogLevel::Warn, "a0-warn"),
            mk(Some("a"), Some(r1), Some(1), LogLevel::Warn, "a1-warn"),
            mk(Some("a"), Some(r1), Some(1), LogLevel::Info, "a1-info"),
            mk(Some("a"), Some(r2), Some(0), LogLevel::Error, "a2-error"),
            fx.line(other, LogLevel::Error, "other"),
        ])
        .await
        .unwrap();
    fx.settle(exec).await;
    let f = LogFilter {
        task_id: Some("a".into()),
        task_run_id: Some(r1),
        attempt: Some(1),
        min_level: Some(LogLevel::Warn),
    };
    assert_eq!(fx.repo.delete_for_execution(fx.ws, exec, &f).await.unwrap(), 1);
    let left = fx.page(exec, &q(None, 100)).await;
    assert_eq!(messages(&left), vec!["exec-error", "a0-warn", "a1-info", "a2-error"]);
    // min_level alone: WARN and ERROR go.
    let f = LogFilter { min_level: Some(LogLevel::Warn), ..LogFilter::default() };
    assert_eq!(fx.repo.delete_for_execution(fx.ws, exec, &f).await.unwrap(), 3);
    assert_eq!(messages(&fx.page(exec, &q(None, 100)).await), vec!["a1-info"]);
    // No filter: the rest; another execution's line stays.
    assert_eq!(fx.repo.delete_for_execution(fx.ws, exec, &LogFilter::default()).await.unwrap(), 1);
    assert_eq!(fx.count(other).await, 1);
}

/// Both ids are in the predicate: another workspace's id deletes nothing.
#[tokio::test]
async fn delete_for_execution_never_reaches_another_workspace() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    fx.repo.insert_batch(&[fx.line(exec, LogLevel::Info, "kept")]).await.unwrap();
    let (other_ws, _) = common::seed_workspace(&fx.db).await;
    assert_eq!(
        fx.repo.delete_for_execution(other_ws, exec, &LogFilter::default()).await.unwrap(),
        0
    );
    assert_eq!(fx.count(exec).await, 1);
}

/// D7, S11: id order, the cursor, the limit and the filters; no settle lag.
#[tokio::test]
async fn download_batch_reads_what_is_committed_in_id_order() {
    let Some(fx) = fx().await else { return };
    let exec = fx.execution().await;
    let mut lines = Vec::new();
    for i in 0..5 {
        let mut l = fx.line(exec, if i % 2 == 0 { LogLevel::Info } else { LogLevel::Error }, &format!("m{i}"));
        l.task_id = Some("t".into());
        lines.push(l);
    }
    fx.repo.insert_batch(&lines).await.unwrap();
    // Not settled: `page` would hold these back for 2 s; the download does not.
    let first = fx.repo.download_batch(fx.ws, exec, &LogFilter::default(), None, 2).await.unwrap();
    assert_eq!(messages(&first), vec!["m0", "m1"]);
    let rest = fx.repo
        .download_batch(fx.ws, exec, &LogFilter::default(), Some(first[1].id), 10)
        .await
        .unwrap();
    assert_eq!(messages(&rest), vec!["m2", "m3", "m4"]);
    let errors = LogFilter { min_level: Some(LogLevel::Error), ..LogFilter::default() };
    assert_eq!(
        messages(&fx.repo.download_batch(fx.ws, exec, &errors, None, 10).await.unwrap()),
        vec!["m1", "m3"]
    );
}
```
`LogFilter` is imported beside `LogQuery` at the top of `tests/task_logs.rs`.

---

## T6. Routes, codes, audit, OpenAPI, `ExecutionDto.idle` (spec D1, D4, D5, D7–D9, D12; S3, S7, S13)

**Files:**
- Modify: `crates/egret-workflow/src/routes.rs` (sections named below), `openapi.json` (regenerated).
- Create: `crates/egret-workflow/tests/delete_labels_logs_routes.rs`.

**Interfaces.**

Consumes: T3–T5.

Produces:
```rust
// routes.rs, in the codes block after RESUME_INPUTS_UNEXPECTED_CODE (:4362)
/// Package 7d D9: a delete or a labels edit of an execution that is not idle.
pub const NOT_TERMINATED_CODE: &str = "not_terminated";
/// Package 7d D9: a delete of an execution others point at; `details.executions`.
pub const HAS_DEPENDENTS_CODE: &str = "has_dependents";
/// Package 7d D9: a delete of a loop iteration or retry replacement; `details.root`.
pub const NOT_A_ROOT_CODE: &str = "not_a_root";
/// Package 7d D9: a labels edit that breaks D5's rules.
pub const INVALID_LABELS_CODE: &str = "invalid_labels";
/// Package 7d D9, Q5: `deleteLogs=false`.
pub const DELETE_LOGS_REQUIRED_CODE: &str = "delete_logs_required";

/// An `ErrorBody` that also carries `details` (package 7d D9, plan S3).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DetailedErrorBody {
    pub error: String,
    pub code: String,
    /// `has_dependents`: `{"executions": [uuid, …], "total": n}` (at most 20
    /// named, oldest first); `not_a_root`: `{"root": uuid}`.
    #[schema(value_type = Object)]
    pub details: serde_json::Value,
}

pub enum ExecOpError {
    Core(Error),
    Coded(StatusCode, &'static str, String),
    /// A coded refusal with `details` (plan S3).
    Detailed(StatusCode, &'static str, String, serde_json::Value),
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct DeleteExecutionQuery { pub delete_storage: Option<bool>, pub delete_logs: Option<bool>, pub delete_metrics: Option<bool> }

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct LogDownloadQuery { pub task_id: Option<String>, pub task_run_id: Option<Uuid>, pub min_level: Option<LogLevel> }

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct DeleteLogsQuery { pub task_id: Option<String>, pub task_run_id: Option<Uuid>, pub attempt: Option<i32>, pub min_level: Option<LogLevel> }

async fn delete_execution(State<WorkflowState>, WsContext, Path<(String, Uuid)>,
    Result<Query<DeleteExecutionQuery>, QueryRejection>) -> Result<StatusCode, ExecOpError>;
async fn set_execution_labels(State<WorkflowState>, WsContext, Path<(String, Uuid)>,
    Result<Json<Vec<LabelDto>>, JsonRejection>) -> Result<Json<ExecutionDto>, ExecOpError>;
async fn download_execution_logs(State<WorkflowState>, WsContext, Path<(String, Uuid)>,
    Query<LogDownloadQuery>) -> EgretResult<axum::response::Response>;
async fn delete_execution_logs(State<WorkflowState>, WsContext, Path<(String, Uuid)>,
    Query<DeleteLogsQuery>) -> EgretResult<StatusCode>;

// ExecutionDto, after paused_by_hand
/// `true` when the execution has ended and no task run of it is still
/// running, `afterExecution:` runs included (`workflow.execution_is_idle`).
/// Delete and Edit labels need it; the routes check again.
// Package 7d D10 (plan S7).
pub idle: bool,
```
Each field of the three query structs has a doc comment (they become OpenAPI parameter descriptions): `deleteStorage` "Default true: also delete the stored files of the execution, its loop iterations and its retries"; `deleteLogs` "Default true. `false` is refused (400 `delete_logs_required`): an execution's logs always go with it"; `deleteMetrics` "Accepted for Kestra compatibility; egret keeps no metrics"; `task_id`, `task_run_id`, `min_level` as `ExecutionLogsQuery`'s; `attempt` "Only lines of this attempt, 0-based as `LogLineDto.attempt`".

**Handler shapes.**

- **`ExecOpError::Detailed`.** `IntoResponse` (`:4659-4680`) gains the arm
  ```rust
  ExecOpError::Detailed(status, code, message, details) => {
      let error = match status {
          StatusCode::CONFLICT => Error::Conflict(message).to_string(),
          _ => Error::InvalidInput(message).to_string(),
      };
      (status, Json(DetailedErrorBody { error, code: code.to_string(), details })).into_response()
  }
  ```
  The enum's doc gains "and the coded refusals that carry `details` (package 7d)".
- **`ExecutionDto::new`**, before `d.execution` moves:
  ```rust
  let terminal = |s: &str| crate::worker::from_wire(s).is_some_and(RunState::is_terminal);
  // `d.runs` already leaves out the rows a restart emptied (`ExecutionRepo::task_runs`),
  // which is what `workflow.execution_is_idle` ignores (migration 0225).
  let idle = terminal(&d.execution.state) && d.runs.iter().all(|r| terminal(&r.state));
  ```
- **`delete_execution`** (D1–D4, D9; Q5):
  ```rust
  async fn delete_execution(
      State(st): State<WorkflowState>,
      WsContext(ctx): WsContext,
      Path((_ws, id)): Path<(String, Uuid)>,
      query: std::result::Result<Query<DeleteExecutionQuery>, axum::extract::rejection::QueryRejection>,
  ) -> std::result::Result<StatusCode, ExecOpError> {
      egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
      let Query(q) = query.map_err(|r| {
          ExecOpError::Core(Error::InvalidInput(format!(
              "deleteStorage, deleteLogs and deleteMetrics are true or false: {}",
              r.body_text()
          )))
      })?;
      // Q5: logs cascade with the row; keeping them is not possible.
      if q.delete_logs == Some(false) {
          return Err(ExecOpError::Coded(
              StatusCode::BAD_REQUEST,
              DELETE_LOGS_REQUIRED_CODE,
              "an execution's logs are always deleted with it in egret; omit deleteLogs or pass true"
                  .to_string(),
          ));
      }
      let ws = ctx.workspace_id.0;
      let delete_storage = q.delete_storage.unwrap_or(true);
      let fs = crate::storage::current();
      let req = crate::worker::DeleteRequest { workspace: ws, id, delete_storage };
      // Every check is the worker's, under the lock (plan S5); a foreign or
      // missing id is its `NotFound`, a 404.
      let done = match crate::worker::delete_execution(&st.db.pool, fs.as_deref(), req).await? {
          Ok(done) => done,
          Err(refused) => return Err(ExecOpError::from_delete_refusal(id, refused)),
      };
      // The names go in the detail: the row is gone, and the trail is the
      // only record left of what this id was (as `purge_flow`). No value.
      st.audit.emit(egret_audit::mutation_for(
          &ctx.principal,
          Some(ws),
          "workflow_execution:delete",
          "workflow_execution",
          Some(id.to_string()),
          serde_json::json!({
              "namespace": done.namespace,
              "flow_key": done.flow_key,
              "state": done.state,
              "delete_storage": delete_storage,
              "executions": done.executions,
              "files": done.files,
              "detached": done.detached,
          }),
      ));
      Ok(StatusCode::NO_CONTENT)
  }
  ```
  and in `impl ExecOpError`:
  ```rust
  fn from_delete_refusal(id: Uuid, r: crate::worker::DeleteRefused) -> Self {
      use crate::worker::DeleteRefused as R;
      match r {
          R::NotARoot { root } => Self::Detailed(
              StatusCode::CONFLICT,
              NOT_A_ROOT_CODE,
              format!(
                  "execution {id} is part of the run of execution {root} (a loop iteration or a \
                   retry) and is deleted with it; delete execution {root} instead"
              ),
              serde_json::json!({ "root": root }),
          ),
          R::NotIdle { execution, state } if execution == id => Self::conflict(
              NOT_TERMINATED_CODE,
              format!(
                  "execution {id} is {state} or still has running tasks (afterExecution \
                   included); kill it first, then delete it"
              ),
          ),
          R::NotIdle { execution, .. } => Self::conflict(
              NOT_TERMINATED_CODE,
              format!(
                  "execution {execution}, part of the run of execution {id}, is still running; \
                   kill execution {id} first, then delete it"
              ),
          ),
          R::ListenersPending => Self::conflict(
              NOT_TERMINATED_CODE,
              format!(
                  "execution {id} has just ended and its Flow triggers have not all heard it \
                   yet; try again in a minute"
              ),
          ),
          R::HasDependents { executions, total } => Self::Detailed(
              StatusCode::CONFLICT,
              HAS_DEPENDENTS_CODE,
              format!(
                  "{total} other execution(s) point at execution {id} (subflow children, \
                   replays or restarts into a new execution), among them {}; delete those first",
                  executions.iter().map(Uuid::to_string).collect::<Vec<_>>().join(", ")
              ),
              serde_json::json!({ "executions": executions, "total": total }),
          ),
      }
  }
  ```
- **`set_execution_labels`** (D5):
  ```rust
  async fn set_execution_labels(
      State(st): State<WorkflowState>,
      WsContext(ctx): WsContext,
      Path((_ws, id)): Path<(String, Uuid)>,
      body: std::result::Result<Json<Vec<LabelDto>>, axum::extract::rejection::JsonRejection>,
  ) -> std::result::Result<Json<ExecutionDto>, ExecOpError> {
      egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
      let Json(body) = body.map_err(|r| {
          ExecOpError::Core(Error::InvalidInput(format!(
              "the body is not a JSON list of `{{\"key\": …, \"value\": …}}`: {}",
              r.body_text()
          )))
      })?;
      let ws = ctx.workspace_id.0;
      let labels: Vec<crate::models::Label> = body
          .into_iter()
          .map(|l| crate::models::Label { key: l.key, value: l.value })
          .collect();
      let set = match crate::worker::set_execution_labels(&st.db.pool, ws, id, &labels).await? {
          Ok(set) => set,
          Err(crate::worker::LabelsRefused::NotIdle(state)) => {
              return Err(ExecOpError::conflict(
                  NOT_TERMINATED_CODE,
                  format!(
                      "execution {id} is {state} or still has running tasks (afterExecution \
                       included); its labels can be edited once it has ended"
                  ),
              ))
          }
          Err(crate::worker::LabelsRefused::Invalid(why)) => {
              return Err(ExecOpError::Coded(StatusCode::BAD_REQUEST, INVALID_LABELS_CODE, why))
          }
      };
      // Keys only: a label value can carry anything a person typed.
      st.audit.emit(egret_audit::mutation_for(
          &ctx.principal,
          Some(ws),
          "workflow_execution:labels",
          "workflow_execution",
          Some(id.to_string()),
          serde_json::json!({ "keys_before": set.keys_before, "keys_after": set.keys_after }),
      ));
      let detail = st.executions.get(ws, id).await?;
      Ok(Json(ExecutionDto::new(detail, true, st.trigger_flags.worker_enabled)))
  }
  ```
- **`download_execution_logs`** (D7, S11, S12). Authorisation exactly `get_execution_logs`'s: `require(WF_READ)`, then `st.executions.log_status(ws, id)` (404 for a foreign id).
  ```rust
  let status = st.executions.log_status(ws, id).await?;
  let filter = crate::logs::LogFilter {
      task_id: q.task_id,
      task_run_id: q.task_run_id,
      attempt: None,
      min_level: q.min_level,
  };
  let body = axum::body::Body::from_stream(log_download(st.logs.clone(), ws, id, filter));
  let mut res = axum::response::Response::new(body);
  let h = res.headers_mut();
  h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
  h.insert(
      header::CONTENT_DISPOSITION,
      HeaderValue::from_str(&format!("attachment; filename=\"{id}.log\""))
          .expect("a uuid is a valid header value"),
  );
  h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
  // D7: an execution still writing lines must not be served from a cache.
  if !status.idle {
      h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
  }
  Ok(res)
  ```
  (`use axum::http::{header, HeaderValue};`.) The stream:
  ```rust
  /// The download's body (plan S11): batches of `LOG_DOWNLOAD_BATCH` lines in
  /// id order, joined with `\n`, no trailing newline; at most
  /// `LOG_DOWNLOAD_MAX_LINES` lines, then `truncated_line()` when there were
  /// more. A database error ends the stream with an error, so the client's
  /// download fails rather than ending short.
  fn log_download(
      repo: LogRepo,
      ws: Uuid,
      id: Uuid,
      filter: crate::logs::LogFilter,
  ) -> impl futures_util::Stream<Item = std::io::Result<bytes::Bytes>> + Send + 'static {
      use crate::logs::{download_line, truncated_line, LOG_DOWNLOAD_BATCH, LOG_DOWNLOAD_MAX_LINES};
      struct Cursor { repo: LogRepo, ws: Uuid, id: Uuid, filter: crate::logs::LogFilter,
                      after: Option<i64>, served: i64, done: bool }
      let start = Cursor { repo, ws, id, filter, after: None, served: 0, done: false };
      futures_util::stream::unfold(start, |mut c| async move {
          if c.done {
              return None;
          }
          // One more than the cap is read, to know whether to say "truncated".
          let want = (LOG_DOWNLOAD_MAX_LINES + 1 - c.served).min(LOG_DOWNLOAD_BATCH);
          let batch = match c.repo.download_batch(c.ws, c.id, &c.filter, c.after, want).await {
              Ok(batch) => batch,
              Err(e) => {
                  c.done = true;
                  return Some((Err(std::io::Error::other(e.to_string())), c));
              }
          };
          let mut chunk = String::new();
          for line in &batch {
              if c.served == LOG_DOWNLOAD_MAX_LINES {
                  chunk.push('\n');
                  chunk.push_str(&truncated_line());
                  c.done = true;
                  break;
              }
              if c.served > 0 {
                  chunk.push('\n');
              }
              chunk.push_str(&download_line(line));
              c.served += 1;
              c.after = Some(line.id);
          }
          if (batch.len() as i64) < want {
              c.done = true;
          }
          (!chunk.is_empty()).then(|| (Ok(bytes::Bytes::from(chunk)), c))
      })
  }
  ```
- **`delete_execution_logs`** (D8, Q6):
  ```rust
  egret_core::authz::require(&*st.pdp, &ctx, WF_MANAGE).await?;
  let ws = ctx.workspace_id.0;
  // Existence and ownership, as the read: a foreign id is a 404 (S13: the flow is not looked at).
  st.executions.log_status(ws, id).await?;
  let filter = crate::logs::LogFilter {
      task_id: q.task_id.clone(),
      task_run_id: q.task_run_id,
      attempt: q.attempt,
      min_level: q.min_level,
  };
  let deleted = st.logs.delete_for_execution(ws, id, &filter).await?;
  st.audit.emit(egret_audit::mutation_for(
      &ctx.principal,
      Some(ws),
      "workflow_execution:logs_delete",
      "workflow_execution",
      Some(id.to_string()),
      serde_json::json!({
          "task_id": q.task_id,
          "task_run_id": q.task_run_id,
          "attempt": q.attempt,
          "min_level": q.min_level.map(LogLevel::as_str),
          "deleted": deleted,
      }),
  ));
  Ok(StatusCode::NO_CONTENT)
  ```
- **Router** (`routes.rs:5434-5445`):
  ```rust
  .route(
      "/ws/{workspace}/workflows/executions/{id}",
      get(get_execution).delete(delete_execution),
  )
  …
  .route(
      "/ws/{workspace}/workflows/executions/{id}/logs",
      get(get_execution_logs).delete(delete_execution_logs),
  )
  .route(
      "/ws/{workspace}/workflows/executions/{id}/logs/download",
      get(download_execution_logs),
  )
  ```
  and beside the 7c routes (`:5534`):
  ```rust
  .route(
      "/ws/{workspace}/workflows/executions/{id}/labels",
      post(set_execution_labels),
  )
  ```
  The module doc's route list (`routes.rs:30-40`) gains the four lines.
- **OpenAPI.** Add `delete_execution`, `set_execution_labels`, `download_execution_logs`, `delete_execution_logs` to `paths(` after `get_execution_logs`; `DetailedErrorBody` to `components(schemas(`.
  - **Delete** (`delete`, params `WorkspacePath`, `id`, `DeleteExecutionQuery`): 204 "Deleted, with its loop iterations and retries, their task runs, logs and file index; with `deleteStorage` (the default) their stored files too. Executions its Flow triggers fired are kept, their link to it cleared. Nothing is killed: only an ended execution can be deleted."; 400 (`delete_logs_required`; a malformed flag); 401; 403 ("Missing workflow:manage permission, or the workflows module is disabled for this workspace. **workflow:read is not enough.**"); 404 ("No such execution in this workspace, indistinguishably from one that never existed. A flow in the trash or deleted does not matter."); 409 `body = DetailedErrorBody` ("`not_terminated`: the execution, or a loop iteration or retry of it, has not ended or still runs `afterExecution:` tasks; or its Flow triggers have not all heard it ended (try again in a minute). `has_dependents`: subflow children, replays or restarts into a new execution point at it; `details.executions` names up to 20, `details.total` counts them; delete them first. An execution a Flow trigger fired from it does not block: it is kept and its link to this execution is cleared. `not_a_root`: a loop iteration or retry; `details.root` is the execution to delete. Nothing is deleted."); 500 ("Internal error, including the file store failing: nothing was deleted, and the call can be repeated").
  - **Labels** (`post`, `request_body = Vec<LabelDto>` "The whole list the execution shows afterwards: it replaces the visible labels, the flow's included. Each key is a lower-case letter then letters, digits, `.`, `_`, `-` (a key the execution already shows may be sent back as it is); values are non-empty; at most 50 labels, keys of 128 and values of 1024 characters. `system.*` labels the execution has are kept whether sent or not; one sent must be unchanged."): 200 `ExecutionDto` ("Its detail afterwards; `effective_labels` is the new list."); 400 ("`invalid_labels`, naming the label; or a body that is not a list of `{key, value}`"); 401; 403; 404; 409 ("`not_terminated`: it has not ended, or still runs `afterExecution:` tasks. Nothing is written."); 500.
  - **Download** (`get`, params `WorkspacePath`, `id`, `LogDownloadQuery`): 200 `content_type = "text/plain"`, `body = String` ("One line per log line in id order, `<instant RFC 3339 UTC, ms> <LEVEL> <message>`, joined with `\n`, as `<id>.log`. At most 100 000 lines; then the last line is `… truncated at 100000 lines`. `Cache-Control: no-cache` while the execution has not ended. No settle lag: the lines committed now."); 400 (`min_level`, a malformed id); 401; 403 ("Missing workflow:read …"); 404; 500.
  - **Delete logs** (`delete`, params `WorkspacePath`, `id`, `DeleteLogsQuery`): 204 ("The matching lines are deleted; a line written after this call stays. Allowed while the execution runs."); 400; 401; 403 ("workflow:manage …"); 404; 500.
  - After the regeneration, check that `openapi.json` names the delete's parameters `deleteStorage`, `deleteLogs`, `deleteMetrics` (utoipa reads `#[serde(rename_all)]`); if not, add `#[into_params(rename_all = "camelCase")]`.
- [ ] **Failing route tests** (below) in `tests/delete_labels_logs_routes.rs`. Run `cargo test -p egret-workflow --test delete_labels_logs_routes`: red.
- [ ] Implement.
- [ ] Run, one at a time:
  - `cargo test -p egret-workflow --test delete_labels_logs_routes --test delete_labels --test labels_replaced --test executions_routes --test pause_resume_routes --test restart_routes --test replay_routes --test task_logs`
  - `cargo clippy -p egret-workflow --all-targets -- -D warnings`
- [ ] **OpenAPI:**
  ```bash
  cargo run -p egret-bin -- openapi > openapi.json
  cargo test -p egret-server committed_openapi_json_is_current
  ```
  `generated_client_was_emitted_from_the_committed_spec` now fails until T7; that is expected, and the controller runs T7 next.
- [ ] Commit: `feat(workflow): delete, labels, log download and log delete routes (package 7d)`.

**Route tests** (`tests/delete_labels_logs_routes.rs`). Module doc lists the four routes, the spec and "Run with: …". Copy `setup`, `flow_of`, `start`, `exec_state`, `settle_everything`, `handlers`, `expect_error`, `LOOPY` from `tests/pause_resume_routes.rs:30-220`. Every refusal is paired with an accepted control. Each test is `#[sqlx::test(migrations = "../../migrations")]`.

```rust
fn one_task(id: &str) -> String {
    format!("id: {id}\nnamespace: t\nlabels:\n  team: data\ntasks:\n  - id: a\n    type: {LOG}\n    message: hi\n")
}
fn exec_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}")
}
fn labels_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/labels")
}
fn logs_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/logs")
}
fn download_path(ws: Uuid, id: Uuid) -> String {
    format!("/api/ws/{ws}/workflows/executions/{id}/logs/download")
}
/// Ended, and with no log line of its own: a test's lines are the only ones.
async fn ended(pool: &sqlx::PgPool, ws: Uuid, src: &str) -> Uuid {
    let ex = start(pool, ws, src).await;
    settle_everything(pool, &handlers()).await;
    assert_eq!(exec_state(pool, ex).await, "SUCCESS");
    sqlx::query("DELETE FROM workflow.task_logs WHERE execution_id = $1")
        .bind(ex)
        .execute(pool)
        .await
        .unwrap();
    ex
}
async fn exists(pool: &sqlx::PgPool, id: Uuid) -> bool {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM workflow.executions WHERE id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}
/// A line at a fixed instant, so the download's text is exact.
async fn log_line(pool: &sqlx::PgPool, ws: Uuid, ex: Uuid, task: Option<&str>, run: Option<Uuid>, level: i16, msg: &str) {
    sqlx::query(
        "INSERT INTO workflow.task_logs (workspace_id, execution_id, task_id, task_run_id, attempt, level, ts, message) \
         VALUES ($1, $2, $3, $4, CASE WHEN $4 IS NULL THEN NULL ELSE 0 END, $5, '2026-11-16T10:00:00.123Z', $6)",
    )
    .bind(ws)
    .bind(ex)
    .bind(task)
    .bind(run)
    .bind(level)
    .bind(msg)
    .execute(pool)
    .await
    .unwrap();
}
async fn lines_of(pool: &sqlx::PgPool, ex: Uuid) -> Vec<String> {
    sqlx::query_scalar("SELECT message FROM workflow.task_logs WHERE execution_id = $1 ORDER BY id")
        .bind(ex)
        .fetch_all(pool)
        .await
        .unwrap()
}
async fn text(res: axum::response::Response) -> String {
    let bytes = axum::body::to_bytes(res.into_body(), 16 * 1024 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_answers_204_and_is_audited_with_names_and_no_value(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;

    // Act
    let res = common::delete(&app, &exec_path(ws, ex), user).await;

    // Assert
    assert_eq!(res.status(), 204);
    assert!(!exists(&pool, ex).await);
    assert_eq!(common::get(&app, &exec_path(ws, ex), user).await.status(), 404);
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:delete".to_string(),
            Some(ex.to_string()),
            json!({
                "namespace": "t", "flow_key": "one", "state": "SUCCESS",
                "delete_storage": true, "executions": 1, "files": 0, "detached": 0
            })
        )]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_flags_follow_kestra_and_delete_logs_false_is_refused(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    let msg = expect_error(
        common::delete(&app, &format!("{}?deleteLogs=false", exec_path(ws, ex)), user).await,
        400,
        Some("delete_logs_required"),
    )
    .await;
    assert!(msg.contains("deleteLogs"), "{msg}");
    assert_eq!(
        common::delete(&app, &format!("{}?deleteStorage=maybe", exec_path(ws, ex)), user).await.status(),
        400
    );
    assert!(exists(&pool, ex).await);
    let res = common::delete(
        &app,
        &format!("{}?deleteStorage=false&deleteLogs=true&deleteMetrics=false", exec_path(ws, ex)),
        user,
    )
    .await;
    assert_eq!(res.status(), 204);
    let audit = common::audit_mutations(&app);
    assert_eq!(audit.last().unwrap().2["delete_storage"], json!(false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn delete_refusals_are_coded_with_details(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    // not_terminated: RUNNING.
    let running = start(&pool, ws, &one_task("run")).await;
    let msg = expect_error(common::delete(&app, &exec_path(ws, running), user).await, 409, Some("not_terminated")).await;
    assert!(msg.contains("RUNNING") && msg.contains("kill"), "{msg}");
    // not_a_root: a loop iteration, `details.root`.
    let root = ended(&pool, ws, LOOPY).await;
    let iteration: Uuid = sqlx::query_scalar("SELECT id FROM workflow.executions WHERE parent_execution_id = $1 LIMIT 1")
        .bind(root)
        .fetch_one(&pool)
        .await
        .unwrap();
    let res = common::delete(&app, &exec_path(ws, iteration), user).await;
    assert_eq!(res.status(), 409);
    let body = common::json(res).await;
    assert_eq!(body["code"], "not_a_root");
    assert_eq!(body["details"]["root"], json!(root));
    // has_dependents: a whole replay of `one`, `details.executions`.
    let one = ended(&pool, ws, &one_task("one")).await;
    let res = common::post_no_body(&app, &format!("{}/replay", exec_path(ws, one)), user).await;
    assert!(res.status().is_success(), "{}", res.status());
    let replay: Uuid = sqlx::query_scalar("SELECT id FROM workflow.executions WHERE replayed_from_execution_id = $1")
        .bind(one)
        .fetch_one(&pool)
        .await
        .unwrap();
    settle_everything(&pool, &handlers()).await;
    let res = common::delete(&app, &exec_path(ws, one), user).await;
    assert_eq!(res.status(), 409);
    let body = common::json(res).await;
    assert_eq!(body["code"], "has_dependents");
    assert_eq!(body["details"], json!({"executions": [replay], "total": 1}));
    assert!(body["error"].as_str().unwrap().contains(&replay.to_string()));
    // Nothing deleted, nothing audited; then the controls.
    assert!(exists(&pool, running).await && exists(&pool, iteration).await && exists(&pool, one).await);
    assert!(common::audit_mutations(&app).is_empty());
    assert_eq!(common::delete(&app, &exec_path(ws, replay), user).await.status(), 204);
    assert_eq!(common::delete(&app, &exec_path(ws, one), user).await.status(), 204);
    assert_eq!(common::delete(&app, &exec_path(ws, root), user).await.status(), 204);
    assert!(!exists(&pool, iteration).await, "the iteration went with its root");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_trashed_flow_does_not_block_a_delete(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    sqlx::query("UPDATE workflow.flows SET deleted_at = now() WHERE workspace_id = $1")
        .bind(ws)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(common::delete(&app, &exec_path(ws, ex), user).await.status(), 204);
}

#[sqlx::test(migrations = "../../migrations")]
async fn workflow_read_cannot_delete_relabel_or_delete_logs_but_can_download(pool: sqlx::PgPool) {
    // Arrange
    egret_test_db::assert_database_url_is_exported();
    let db = Db { pool: pool.clone() };
    let (ws, user) = common::seed_workspace(&db).await;
    common::grant(&db, ws, user, &["workflow:read"]).await;
    let app = common::app(&db).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    log_line(&pool, ws, ex, None, None, 2, "kept").await;

    // Act + Assert
    assert_eq!(common::delete(&app, &exec_path(ws, ex), user).await.status(), 403);
    assert_eq!(common::post(&app, &labels_path(ws, ex), user, json!([])).await.status(), 403);
    assert_eq!(common::delete(&app, &logs_path(ws, ex), user).await.status(), 403);
    assert_eq!(common::get(&app, &download_path(ws, ex), user).await.status(), 200);
    assert!(exists(&pool, ex).await);
    assert_eq!(lines_of(&pool, ex).await, vec!["kept"]);
    assert!(common::audit_mutations(&app).is_empty());
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_foreign_workspace_gets_404_on_all_four(pool: sqlx::PgPool) {
    let (db, ws, _user, app) = setup(&pool).await;
    let (other_ws, other) = common::seed_workspace(&db).await;
    common::grant(&db, other_ws, other, &["workflow:manage", "workflow:read"]).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    log_line(&pool, ws, ex, None, None, 2, "kept").await;
    assert_eq!(common::delete(&app, &exec_path(other_ws, ex), other).await.status(), 404);
    assert_eq!(common::post(&app, &labels_path(other_ws, ex), other, json!([])).await.status(), 404);
    assert_eq!(common::delete(&app, &logs_path(other_ws, ex), other).await.status(), 404);
    assert_eq!(common::get(&app, &download_path(other_ws, ex), other).await.status(), 404);
    assert!(exists(&pool, ex).await);
    assert_eq!(lines_of(&pool, ex).await, vec!["kept"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn labels_replace_the_visible_list_and_are_audited_by_key(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;

    // Act: the flow's `team` removed, `env` added.
    let res = common::post(&app, &labels_path(ws, ex), user, json!([{"key": "env", "value": "dev"}])).await;

    // Assert
    assert_eq!(res.status(), 200);
    let body = common::json(res).await;
    assert_eq!(body["effective_labels"], json!([{"key": "env", "value": "dev"}]));
    assert_eq!(body["idle"], json!(true));
    let audit = common::audit_mutations(&app);
    assert_eq!(
        audit,
        vec![(
            "workflow_execution:labels".to_string(),
            Some(ex.to_string()),
            json!({"keys_before": ["team"], "keys_after": ["env"]})
        )]
    );
    assert!(!audit[0].2.to_string().contains("dev"), "no value in the trail");
}

#[sqlx::test(migrations = "../../migrations")]
async fn labels_refusals_are_coded(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let running = start(&pool, ws, &one_task("run")).await;
    expect_error(common::post(&app, &labels_path(ws, running), user, json!([])).await, 409, Some("not_terminated")).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    for (body, needle) in [
        (json!([{"key": "Env", "value": "dev"}]), "Env"),
        (json!([{"key": "env", "value": ""}]), "env"),
        (json!([{"key": "system.replay", "value": "true"}]), "system.replay"),
        (json!([{"key": "a", "value": "1"}, {"key": "a", "value": "2"}]), "`a`"),
    ] {
        let msg = expect_error(common::post(&app, &labels_path(ws, ex), user, body).await, 400, Some("invalid_labels")).await;
        assert!(msg.contains(needle), "{msg} should name {needle}");
    }
    // A body that is not a list: 400, no code.
    expect_error(common::post(&app, &labels_path(ws, ex), user, json!({"key": "a"})).await, 400, None).await;
    assert!(common::audit_mutations(&app).is_empty());
    // Control: the unchanged list passes.
    assert_eq!(
        common::post(&app, &labels_path(ws, ex), user, json!([{"key": "team", "value": "data"}])).await.status(),
        200
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_detail_carries_idle(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let running = start(&pool, ws, &one_task("run")).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    let idle = |v: Value| v["idle"].clone();
    assert_eq!(idle(common::json(common::get(&app, &exec_path(ws, running), user).await).await), json!(false));
    assert_eq!(idle(common::json(common::get(&app, &exec_path(ws, ex), user).await).await), json!(true));
    // Ended with a live run: not idle.
    sqlx::query("UPDATE workflow.task_runs SET state = 'RUNNING', ended_at = NULL WHERE execution_id = $1")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(idle(common::json(common::get(&app, &exec_path(ws, ex), user).await).await), json!(false));
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_download_is_kestras_text_with_filters_and_headers(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    let run = Uuid::new_v4();
    log_line(&pool, ws, ex, None, None, 2, "one").await;
    log_line(&pool, ws, ex, Some("a"), Some(run), 4, "two").await;
    log_line(&pool, ws, ex, Some("a"), Some(run), 1, "three").await;

    // Act
    let res = common::get(&app, &download_path(ws, ex), user).await;

    // Assert
    assert_eq!(res.status(), 200);
    let h = res.headers().clone();
    assert_eq!(h["content-type"], "text/plain; charset=utf-8");
    assert_eq!(h["content-disposition"], format!("attachment; filename=\"{ex}.log\""));
    assert!(h.get("cache-control").is_none(), "an ended execution may be cached");
    assert_eq!(
        text(res).await,
        "2026-11-16T10:00:00.123Z INFO one\n2026-11-16T10:00:00.123Z ERROR two\n2026-11-16T10:00:00.123Z DEBUG three"
    );
    let res = common::get(&app, &format!("{}?task_run_id={run}&min_level=WARN", download_path(ws, ex)), user).await;
    assert_eq!(text(res).await, "2026-11-16T10:00:00.123Z ERROR two");
    // Not idle: no-cache. An empty log: an empty body.
    let running = start(&pool, ws, &one_task("run")).await;
    let res = common::get(&app, &download_path(ws, running), user).await;
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert_eq!(text(res).await, "");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_download_stops_at_100000_lines_and_says_so(pool: sqlx::PgPool) {
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    sqlx::query(
        "INSERT INTO workflow.task_logs (workspace_id, execution_id, level, ts, message) \
         SELECT $1, $2, 2, '2026-11-16T10:00:00Z', 'l' || g FROM generate_series(1, 100001) g",
    )
    .bind(ws)
    .bind(ex)
    .execute(&pool)
    .await
    .unwrap();
    let body = text(common::get(&app, &download_path(ws, ex), user).await).await;
    let lines: Vec<&str> = body.split('\n').collect();
    assert_eq!(lines.len(), 100_001);
    assert_eq!(lines[99_999], "2026-11-16T10:00:00.000Z INFO l100000");
    assert_eq!(lines[100_000], "… truncated at 100000 lines");
    // Exactly at the cap: no truncation line.
    sqlx::query("DELETE FROM workflow.task_logs WHERE execution_id = $1 AND message = 'l100001'")
        .bind(ex)
        .execute(&pool)
        .await
        .unwrap();
    let body = text(common::get(&app, &download_path(ws, ex), user).await).await;
    assert_eq!(body.split('\n').count(), 100_000);
    assert!(!body.contains("truncated"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn log_delete_ands_its_filters_works_while_running_and_is_audited(pool: sqlx::PgPool) {
    // Arrange: a running execution (Q6).
    let (_db, ws, user, app) = setup(&pool).await;
    let ex = start(&pool, ws, &one_task("run")).await;
    let run = Uuid::new_v4();
    log_line(&pool, ws, ex, None, None, 4, "exec-error").await;
    log_line(&pool, ws, ex, Some("a"), Some(run), 3, "a-warn").await;
    log_line(&pool, ws, ex, Some("a"), Some(run), 2, "a-info").await;

    // Act
    let res = common::delete(&app, &format!("{}?task_run_id={run}&min_level=WARN", logs_path(ws, ex)), user).await;

    // Assert
    assert_eq!(res.status(), 204);
    assert_eq!(lines_of(&pool, ex).await, vec!["exec-error", "a-info"]);
    assert_eq!(
        common::audit_mutations(&app),
        vec![(
            "workflow_execution:logs_delete".to_string(),
            Some(ex.to_string()),
            json!({"task_id": null, "task_run_id": run, "attempt": null, "min_level": "WARN", "deleted": 1})
        )]
    );
    assert_eq!(common::delete(&app, &logs_path(ws, ex), user).await.status(), 204);
    assert!(lines_of(&pool, ex).await.is_empty());
    assert_eq!(common::delete(&app, &format!("{}?min_level=LOUD", logs_path(ws, ex)), user).await.status(), 400);
}

/// tests/pause_resume.rs:75-82.
fn subflow_parent(id: &str, target: &str) -> String {
    format!(
        "id: {id}\nnamespace: t\ntasks:\n  - id: sub\n    \
         type: io.kestra.plugin.core.flow.Subflow\n    namespace: t\n    \
         flowId: {target}\n  - id: after\n    \
         type: io.kestra.plugin.core.log.Log\n    message: after\n"
    )
}

/// `has_dependents` for a subflow parent, through the route (D12).
#[sqlx::test(migrations = "../../migrations")]
async fn a_subflow_parent_answers_has_dependents(pool: sqlx::PgPool) {
    // Arrange
    let (_db, ws, user, app) = setup(&pool).await;
    flow_of(&pool, ws, &one_task("child")).await;
    let parent = ended(&pool, ws, &subflow_parent("parent", "child")).await;
    let child: Uuid = sqlx::query_scalar("SELECT id FROM workflow.executions WHERE trigger_execution_id = $1")
        .bind(parent)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Act
    let res = common::delete(&app, &exec_path(ws, parent), user).await;

    // Assert
    assert_eq!(res.status(), 409);
    let body = common::json(res).await;
    assert_eq!(body["code"], "has_dependents");
    assert_eq!(body["details"], json!({"executions": [child], "total": 1}));
    assert!(exists(&pool, parent).await && exists(&pool, child).await);
    // Control: the child first, then the parent.
    assert_eq!(common::delete(&app, &exec_path(ws, child), user).await.status(), 204);
    assert_eq!(common::delete(&app, &exec_path(ws, parent), user).await.status(), 204);
}

/// The workflows entitlement gates the four routes (as `restart_routes.rs:221-231`).
#[sqlx::test(migrations = "../../migrations")]
async fn the_workflows_entitlement_gates_the_four_routes(pool: sqlx::PgPool) {
    let (db, ws, user, app) = setup(&pool).await;
    let ex = ended(&pool, ws, &one_task("one")).await;
    log_line(&pool, ws, ex, None, None, 2, "kept").await;
    common::disable_feature(&db, ws, "workflows").await;

    assert_eq!(common::delete(&app, &exec_path(ws, ex), user).await.status(), 403);
    assert_eq!(common::post(&app, &labels_path(ws, ex), user, json!([])).await.status(), 403);
    assert_eq!(common::delete(&app, &logs_path(ws, ex), user).await.status(), 403);
    assert_eq!(common::get(&app, &download_path(ws, ex), user).await.status(), 403);
    assert!(exists(&pool, ex).await);
    assert_eq!(lines_of(&pool, ex).await, vec!["kept"]);
}
```
Plus `database_url_is_exported_rather_than_read_from_dotenv`.

---

## T7. Generated client (controller)

- [ ] In `frontend/`, after the `node_modules` check of the Global Constraints, run `npm run gen:api` (never bare `orval`).
- [ ] Check `frontend/src/api/generated/workflow/workflow.ts` for `useWorkflowDeleteExecution` (params `{deleteStorage?, deleteLogs?, deleteMetrics?}`), `useWorkflowSetExecutionLabels` (`data: LabelDto[]`), `getWorkflowDownloadExecutionLogsUrl` (params `{task_id?, task_run_id?, min_level?}`), `useWorkflowDeleteExecutionLogs`, and `ExecutionDto.idle: boolean`, `DetailedErrorBody`. Record the exact names for T8.
- [ ] Run `cargo test -p egret-server generated_client_was_emitted_from_the_committed_spec` (green).
- [ ] Commit: `chore(frontend): regenerate the API client (package 7d)`.

---

## T8. Frontend: Delete, Edit labels, server download, Delete logs (spec Q7, D10, D11)

Read `docs/en/frontend-rules.md` first; every MUST rule applies. The ones this task meets:
- FE-ACTION-1 (both deletes confirm in `ui/alert-dialog`);
- FE-ACTION-2 (invalidate; a toast when the page stays, and after the delete's navigation per S14);
- FE-ACTION-3 (a failed call keeps the dialog open with the server's words and clears nothing typed);
- FE-ACTION-4 (Delete and Edit labels disabled with a reason, through `ReasonButton`/`DisabledReasons`);
- FE-ACTION-5 (`useUnsavedChangesWarning` in `EditLabelsDialog` while dirty);
- FE-FORM-2 (each key and value `Input` has a `Label` tied by `htmlFor`/`id`), FE-FORM-4 (a row's error under its field);
- FE-DATA-1/2 (the generated hooks; keys from `workflowsKeys`);
- FE-COPY-1/2/3 (sentence case; errors say what happened and what to do; copy that names a control is tested).

**Files:**
- Modify: `components/workflows/executions/actions.ts` (+ `__tests__/actions.delete.test.ts`, new); `components/workflows/executions/logs/LogView.tsx`, `useLogFollow.ts`, `ExecutionLogsTab.tsx` (+ their tests); `routes/workflows/ExecutionDetail.tsx`, `routes/workflows/ExecutionOverview.tsx` (+ their tests); every test fixture that builds an `ExecutionDto` (add `idle`; `npm run typecheck` names them).
- Create: `components/workflows/executions/labelsModel.ts` (+ test), `EditLabelsDialog.tsx` (+ test), `DeleteExecutionDialog.tsx` (+ test), `components/workflows/executions/logs/DeleteLogsDialog.tsx` (+ test).

**Interfaces.**

```ts
// actions.ts
export const DELETE_NOT_IDLE_REASON: string;   // "The execution is still running, or its afterExecution tasks are. Kill it first, then delete it"
export const LABELS_NOT_IDLE_REASON: string;   // "Labels can be edited once the execution has ended and its afterExecution tasks have finished"
export function canDelete(execution: Pick<ExecutionDto, "idle" | "parent_execution_id">): Enablement;
export function canEditLabels(execution: Pick<ExecutionDto, "idle">): Enablement;
export function deleteRefusal(code: string | undefined): string | null;
export function labelsRefusal(code: string | undefined): string | null;
/** `has_dependents`' `details` from an `ApiError` body; `null` for anything else. */
export function dependentsOf(e: unknown): { executions: string[]; total: number } | null;

// labelsModel.ts (pure)
export interface LabelRow { key: string; value: string; system: boolean }
export const isSystemLabel: (key: string) => boolean;            // key.startsWith("system.")
export function rowsFromLabels(labels: LabelDto[]): LabelRow[];
export function rowErrors(rows: LabelRow[]): Map<number, { key?: string; value?: string }>;
export function labelsFromRows(rows: LabelRow[]): LabelDto[];   // every row, system ones included, in order
export function rowsChanged(before: LabelDto[], rows: LabelRow[]): boolean;

// DeleteExecutionDialog.tsx
export function DeleteExecutionDialog(props: { ws: string; execution: ExecutionDto; open: boolean; onOpenChange: (o: boolean) => void }): JSX.Element;
// EditLabelsDialog.tsx
export function EditLabelsDialog(props: { ws: string; execution: ExecutionDto; open: boolean; onOpenChange: (o: boolean) => void }): JSX.Element;
// logs/DeleteLogsDialog.tsx
export function DeleteLogsDialog(props: { ws: string; executionId: string; filters: LogFilters; scope: string;
  open: boolean; onOpenChange: (o: boolean) => void; onDeleted: () => void }): JSX.Element;
// LogView.tsx: new optional prop
toolbar?: ReactNode;   // rendered before Download
```

**Steps.**

- [ ] **Node modules** (Global Constraints), then the failing tests below. Run `npx vitest run src/components/workflows/executions src/routes/workflows`: red.
- [ ] **`actions.ts`.** Add after the pause block:
  ```ts
  /** Delete and Edit labels (package 7d D10): the server needs the execution idle -- ended, afterExecution tasks included. */
  export const DELETE_NOT_IDLE_REASON = "The execution is still running, or its afterExecution tasks are. Kill it first, then delete it";
  export const LABELS_NOT_IDLE_REASON = "Labels can be edited once the execution has ended and its afterExecution tasks have finished";

  /** A loop iteration or retry is deleted with the execution it belongs to (D2), never alone. */
  export function canDelete(execution: Pick<ExecutionDto, "idle" | "parent_execution_id">): Enablement {
    if (execution.parent_execution_id != null) {
      return { ok: false, reason: `This execution is part of the run of execution ${execution.parent_execution_id}. Delete that execution instead` };
    }
    return execution.idle ? { ok: true } : { ok: false, reason: DELETE_NOT_IDLE_REASON };
  }

  export function canEditLabels(execution: Pick<ExecutionDto, "idle">): Enablement {
    return execution.idle ? { ok: true } : { ok: false, reason: LABELS_NOT_IDLE_REASON };
  }

  /** One sentence for each coded refusal of the delete route; `null` otherwise (the server's message is shown). */
  export function deleteRefusal(code: string | undefined): string | null {
    switch (code) {
      case "not_terminated":
        return "The execution is running again, or its Flow triggers are still handling its end. Refresh, then try again.";
      case "not_a_root":
        return "This execution is part of another execution's run. Delete that execution instead.";
      case "has_dependents":
        return "Subflow executions, replays or restarts point at this one. Delete them first:";
      default:
        return null;
    }
  }

  export function labelsRefusal(code: string | undefined): string | null {
    return code === "not_terminated" ? "The execution is running again, so its labels cannot be edited now. Refresh to see its state." : null;
  }

  export function dependentsOf(e: unknown): { executions: string[]; total: number } | null {
    if (!(e instanceof ApiError) || e.code !== "has_dependents") return null;
    const details = (e.body as { details?: { executions?: unknown; total?: unknown } } | undefined)?.details;
    const executions = Array.isArray(details?.executions) ? details.executions.filter((x): x is string => typeof x === "string") : [];
    return { executions, total: typeof details?.total === "number" ? details.total : executions.length };
  }
  ```
  (`import { ApiError } from "@/api/client";`.)
- [ ] **`labelsModel.ts`** (pure; FE-FORM-1, no form library). `rowErrors`: an empty key → `key: "Enter a key"`; a key already used by an earlier row → `key: "This key is already used"`; an empty value → `value: "Enter a value"`. The server checks the rest (the key pattern, the caps) and its `invalid_labels` message is shown in the dialog. `rowsChanged` compares `labelsFromRows(rows)` with `before` by key and value, in order.
- [ ] **`EditLabelsDialog.tsx`** (`ui/dialog`):
  - Opens with `rowsFromLabels(execution.effective_labels)`, reset each time it opens.
  - One row per label: key `Input` and value `Input`, each with a visually hidden `Label` (`Key of label 2`, `Value of label 2`) tied by `useId`; a Remove button (`aria-label="Remove label {key}"`). A `system.*` row is read only: both inputs `readOnly`, no Remove, and a note "Set by egret; kept as it is".
  - **Add label** appends an empty row and focuses its key input (FE-A11Y-6: after the user's own action).
  - **Save**: if `rowErrors` is not empty, the errors show under their fields and nothing is sent. Else `useWorkflowSetExecutionLabels().mutateAsync({ workspace: ws, id: execution.id, data: labelsFromRows(rows) })`, through `runExecutionAction` with `expected: 200`, `refusal: labelsRefusal`, `onSuccess: () => toast.success("Saved the labels")`, then close.
  - A failure keeps the rows and shows the server's message in an `Alert` (FE-ACTION-3); a 400 `invalid_labels` message names the label.
  - `useUnsavedChangesWarning(open && rowsChanged(execution.effective_labels, rows))`.
  - Description: "These labels replace the execution's labels, the flow's included. Labels set by egret (`system.*`) are kept."
- [ ] **`DeleteExecutionDialog.tsx`** (`ui/alert-dialog`, staying open on failure as `ConfirmAction` does, `ExecutionDetail.tsx:568-640`):
  - Title "Delete this execution?"; description "Deletes execution {id} with its task runs, logs, loop iterations and retries. This cannot be undone."
  - A `Checkbox` "Delete stored files", checked each time it opens, help "The files its tasks and inputs stored. Unchecked, they stay in storage."
  - Confirm (destructive, "Delete"): `useWorkflowDeleteExecution().mutateAsync({ workspace: ws, id: execution.id, params: { deleteStorage } })`; on 204: `queryClient.removeQueries({ queryKey: workflowsKeys.execution(execution.id) })`, `invalidateQueries({ queryKey: workflowsKeys.executions(ws) })`, `navigate(`/explore/${ws}/workflows/executions`)`, `toast.success(`Deleted execution ${execution.id}`)` (S14).
  - On `has_dependents`: the sentence, then a list of links to each dependent's overview (`/explore/${ws}/workflows/executions/${id}/overview`), and "and {total − shown} more" when `total` is larger. Other codes: `deleteRefusal`'s sentence, else the server's message. The dialog stays open.
- [ ] **`ExecutionDetail.tsx` `ExecutionActions`.** `Pending` gains `"labels" | "delete"`. After the Kill button:
  ```tsx
  <ReasonButton enablement={labelsState} reasonId={`${reasonsId}-labels`} variant="outline" onClick={() => setOpen("labels")}>
    <Tag aria-hidden />
    Edit labels
  </ReasonButton>
  <ReasonButton enablement={deleteState} reasonId={`${reasonsId}-delete`} variant="outline" onClick={() => setOpen("delete")}>
    <Trash2 aria-hidden />
    Delete
  </ReasonButton>
  ```
  with `const labelsState = canEditLabels(execution); const deleteState = canDelete(execution);`, the two `DisabledReasons` entries (`Edit labels`, `Delete`), and the two dialogs. Both are always shown to a `can_manage` user (Q7: the header).
- [ ] **Logs.**
  - `LogView.tsx`: Download becomes a link to the server route:
    ```tsx
    <Button asChild variant="outline" size="sm" title="Saves every line the filters select, up to 100,000 lines.">
      <a href={getWorkflowDownloadExecutionLogsUrl(ws, executionId, downloadParams(filters))} download={downloadName ?? `${executionId}.log`}>
        <Download aria-hidden />
        Download
      </a>
    </Button>
    ```
    `downloadParams(f)` = `{ task_id: f.taskId, task_run_id: f.taskRunId, min_level: f.minLevel }` without the `undefined` keys. Remove `saveText`, the `download` state, `runDownload`, the abort ref, the capped and error notices, and the `downloadPageSize`/`downloadMaxLines` props: the server writes the truncation line itself. Add the `toolbar?: ReactNode` prop, rendered first in the toolbar row.
  - `useLogFollow.ts`: remove `LOG_DOWNLOAD_MAX_LINES`, `LogDownload` and `downloadLogs` (no other user: `LogView.tsx` and the two test files are the only references), and their tests in `__tests__/useLogFollow.test.tsx`. `LogView.test.tsx`'s download tests become: the link's `href` carries the filters, its `download` is `<id>.log` or `downloadName`.
  - `DeleteLogsDialog.tsx` (`ui/alert-dialog`, staying open on failure): title "Delete these logs?"; description "Deletes {scope}. Lines written after this stay. This cannot be undone."; confirm (destructive) "Delete logs": `useWorkflowDeleteExecutionLogs().mutateAsync({ workspace: ws, id: executionId, params: { task_id, task_run_id, min_level } })`, 204 → `onDeleted()` and close.
  - `ExecutionLogsTab.tsx`: when `execution.can_manage`, `toolbar` is a `DropdownMenu` (trigger `aria-label="More log actions"`, `MoreHorizontal`) with **Delete logs…**. Its `scope` sentence: "every log line of this execution", narrowed by the active filters: " at {level} and above", " of task {task}", " of run #{run_ix}". `onDeleted`: `toast.success("Deleted the logs")` and bump a `generation` state used as `LogView`'s `key`, so the view reads from the start.
  - `ExecutionOverview.tsx` `TaskRuns`: `const logsDeletable = canManage;` joins `hasActions`; the row menu gains **Delete logs** (`Trash2`) opening `DeleteLogsDialog` with `filters={{ taskRunId: run.id }}`, `scope={`the logs of ${run.task_id} #${run.run_ix}`}`, and `onDeleted={() => toast.success(`Deleted the logs of ${run.task_id} #${run.run_ix}`)}`.
- [ ] Run, one at a time, in `frontend/`: `npm run lint`, `npm run typecheck`, `npm run test`, `npm run build`, `npm run test:bundle:workflows`. Remove the `node_modules` symlink if T8 created it.
- [ ] Commit: `feat(frontend): delete an execution, edit its labels, server log download and delete (package 7d)`.

**Tests.**

`components/workflows/executions/__tests__/actions.delete.test.ts`:
```ts
import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import {
  DELETE_NOT_IDLE_REASON,
  LABELS_NOT_IDLE_REASON,
  canDelete,
  canEditLabels,
  deleteRefusal,
  dependentsOf,
  labelsRefusal,
} from "@/components/workflows/executions/actions";

describe("canDelete", () => {
  it("allows an idle root", () => {
    expect(canDelete({ idle: true, parent_execution_id: null })).toEqual({ ok: true });
  });
  it("says why while it runs", () => {
    expect(canDelete({ idle: false, parent_execution_id: null })).toEqual({ ok: false, reason: DELETE_NOT_IDLE_REASON });
  });
  it("names the execution an iteration or retry belongs to", () => {
    const got = canDelete({ idle: true, parent_execution_id: "root-1" });
    expect(got.ok).toBe(false);
    expect(!got.ok && got.reason).toContain("root-1");
  });
});

describe("canEditLabels", () => {
  it("follows idle", () => {
    expect(canEditLabels({ idle: true })).toEqual({ ok: true });
    expect(canEditLabels({ idle: false })).toEqual({ ok: false, reason: LABELS_NOT_IDLE_REASON });
  });
});

describe("refusals", () => {
  it("has a sentence for each delete code and none for others", () => {
    for (const code of ["not_terminated", "not_a_root", "has_dependents"]) expect(deleteRefusal(code)).toEqual(expect.any(String));
    expect(deleteRefusal("delete_logs_required")).toBeNull();
    expect(deleteRefusal(undefined)).toBeNull();
    expect(labelsRefusal("not_terminated")).toEqual(expect.any(String));
    expect(labelsRefusal("invalid_labels")).toBeNull();
  });
  it("reads has_dependents' details", () => {
    const e = new ApiError(409, "2 other execution(s)…", "has_dependents", {
      error: "…",
      code: "has_dependents",
      details: { executions: ["a", "b"], total: 3 },
    });
    expect(dependentsOf(e)).toEqual({ executions: ["a", "b"], total: 3 });
    expect(dependentsOf(new ApiError(409, "x", "not_terminated"))).toBeNull();
    expect(dependentsOf(new Error("x"))).toBeNull();
  });
});
```

`components/workflows/executions/__tests__/labelsModel.test.ts`:
```ts
import { describe, expect, it } from "vitest";
import { labelsFromRows, rowErrors, rowsChanged, rowsFromLabels } from "@/components/workflows/executions/labelsModel";

describe("labelsModel", () => {
  const labels = [
    { key: "team", value: "data" },
    { key: "system.restarted", value: "true" },
  ];
  it("marks system rows and round-trips", () => {
    const rows = rowsFromLabels(labels);
    expect(rows.map((r) => r.system)).toEqual([false, true]);
    expect(labelsFromRows(rows)).toEqual(labels);
    expect(rowsChanged(labels, rows)).toBe(false);
  });
  it("finds empty keys, empty values and repeated keys, by row", () => {
    const rows = [
      { key: "", value: "x", system: false },
      { key: "a", value: "", system: false },
      { key: "a", value: "1", system: false },
    ];
    const errors = rowErrors(rows);
    expect(errors.get(0)).toEqual({ key: "Enter a key" });
    expect(errors.get(1)).toEqual({ value: "Enter a value" });
    expect(errors.get(2)).toEqual({ key: "This key is already used" });
  });
  it("sees a removal as a change", () => {
    expect(rowsChanged(labels, rowsFromLabels(labels.slice(1)))).toBe(true);
  });
});
```

Component tests (harness: the `Server` `fetch` stub of `ReplayDialog.test.tsx:1-80`; `MemoryRouter` for navigation):
- `DeleteExecutionDialog.test.tsx`:
  1. confirm sends `DELETE …/executions/{id}?deleteStorage=true`, then the location is `/explore/{ws}/workflows/executions` and a toast says `Deleted execution {id}`;
  2. unchecking "Delete stored files" sends `deleteStorage=false`;
  3. a 409 `has_dependents` with `details.executions: [a, b], total: 3` keeps the dialog open with the sentence, links to `a` and `b`, and "and 1 more";
  4. a 409 `not_terminated` shows `deleteRefusal`'s sentence; a 500 shows the server's message.
- `EditLabelsDialog.test.tsx`:
  1. opens with the effective labels; the `system.restarted` row is read only and has no Remove;
  2. remove `team`, add `env=dev`, Save: the body is `[{"key":"env","value":"dev"},{"key":"system.restarted","value":"true"}]`; a toast "Saved the labels"; the dialog closes;
  3. an empty value shows "Enter a value" under that field and sends nothing;
  4. a 400 `invalid_labels` "label key `Env` must start with a lower-case letter…" keeps the rows and shows the message.
- `DeleteLogsDialog.test.tsx`: confirm sends `DELETE …/logs?task_run_id=…&min_level=WARN` and calls `onDeleted`; a failure keeps it open.
- `ExecutionDetail.test.tsx`: for a `can_manage` user, Edit labels and Delete are enabled on an idle execution and disabled with their reasons on a running one (`idle: false`) and, for Delete, on a loop iteration; a user without `can_manage` sees neither.
- `ExecutionLogsTab.test.tsx` (or the tab's existing test file): with filters `?level=WARN&task=a`, the Download link's `href` carries `min_level=WARN&task_id=a`; the More menu's Delete logs opens a dialog whose description names "at WARN and above" and "of task a"; it is absent without `can_manage`.
- `ExecutionOverview` test: a task-run row's menu has Delete logs for a `can_manage` user and opens the dialog naming `{task} #{run_ix}`.

---

## T9. Docs in this repo, and the gates (spec D13)

- [ ] **Roadmap** `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`. The rows are not edited; add a dated note block after 7c's (`:156-158`), numbered after the highest note (17):
  ```markdown
  **Notes, 2026-11-16 (package 7d, `docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md`; the rows above are not edited).**
  18. **#7:** 7d builds delete (`DELETE …/executions/{id}`, ended roots only, with dependents refused), the labels API (`POST …/executions/{id}/labels`, replacing the visible list, migration 0231 `labels_replaced`), the server log download (`GET …/executions/{id}/logs/download`) and log delete (`DELETE …/executions/{id}/logs`). Row #7's "delete (with logs and storage), labels API", "download and delete" are 7d's; logs follow (SSE) is still open.
  ```
  Appendix B rows `:558` (logs download/delete), `:566` (`POST /{id}/actions/labels`) and `:567` (`DELETE /{id}`) each get a trailing *Corrected 2026-11-16 (7d)* sentence naming the egret route and the recorded divergences (spec §5).
- [ ] **This package's spec:** the corrections and the user's answers to S8 and S9 were folded in before T1 by the controller. Check them against what was built and add a dated note for any deviation. Add `## 7. As built` with the line `(Filled in at the end of the implementation.)`; the controller fills it after the review.
- [ ] **Vietnamese copies:** mirror every paragraph changed above in `docs/vi/plans/2026-10-18-workflow-kestra-parity-roadmap.md` and `docs/vi/specs/2026-11-16-workflow-delete-labels-logs-design.md` (code, identifiers and commands untranslated).
- [ ] **Gates**, one at a time, each read from a file in `$SCRATCH`:
  - `cargo fmt --check`;
  - `cargo clippy -p egret-workflow -p egret-server -p egret-config -p egret-metastore --all-targets -- -D warnings`;
  - `cargo check -p egret-bin`;
  - `cargo test -p egret-workflow --no-fail-fast 2>&1 | tee "$SCRATCH/t9-workflow.txt"`: report passed, failed and ignored. Only the 7 `sql_starrocks_live` may fail;
  - `cargo test -p egret-config -p egret-server -p egret-metastore` (includes the OpenAPI and client-digest tests);
  - the ratchet and census, which must equal T0's;
  - in `frontend/` (symlink as in Global Constraints, removed after): `npm run lint`, `npm run typecheck`, `npm run test`, `npm run build`, `npm run test:bundle:workflows`.
- [ ] Commit: `docs(workflow): record package 7d in the roadmap and its spec`.

---

## After the tasks (controller, not implementers)

- **User docs** (`egret-data/docs`, Platform → Workflows; D13): Executions → Run actions (Delete: ended roots only, `deleteStorage`, the codes `not_terminated` (also while its Flow triggers have not heard its end), `has_dependents` (subflow children, replays, restarts into a new execution; Flow-trigger fires no longer block: they are kept and unlinked), `not_a_root`, `delete_logs_required`; Edit labels: replaces the list, `system.*` kept, `invalid_labels`), Logs (the server download and its 100 000-line cap, Delete logs by filter and per task run), the REST reference, and screenshots of both dialogs and the Logs menu. A separate PR; docs PRs merge at once.
- **Live check** on a native `egret serve` against the worktree's own containers and a **throwaway** database and bucket (never the shared demo stack or a shared bucket: a delete is destructive). Before it, grep the binary for `has_dependents` (memory: the build cache ships a stale binary). In a browser as `admin@egret.local`, light and dark:
  1. Edit labels on an ended execution: remove a flow label, add one; the Overview's chips show the new list; a restart's `system.restarted` stays;
  2. Delete a running execution: disabled with the reason; kill it, then delete: back on the list with a toast; its files are gone from the bucket;
  3. Delete a subflow parent: the dialog lists the child as a link; delete the child, then the parent. Delete the source of a Flow-trigger fire: it goes, the fire stays without a "Child of" link;
  4. Logs tab: Download with a level filter gives `<id>.log` in Kestra's line shape; Delete logs with the task filter removes only those lines; a task run's Delete logs from the Overview.

  Take the docs screenshots.
- **Whole-branch review**, then spec §7 "As built" (commits per task, measured counts, deviations, the ratchet at T0 and at the end), its Vietnamese copy, and the PR (body ends with the attribution line).

---

## Self-review against the spec

| Spec item | Where |
|---|---|
| Q1 idle only, kill first | T3 `delete_execution` step 4 + tests `a_running_execution_is_not_idle_and_is_kept`, `an_ended_execution_with_a_live_run_is_not_idle`, `a_live_iteration_keeps_its_root`; T6 `delete_refusals_are_coded_with_details`; T8 `canDelete` |
| Q2 dependents refused, named; Flow-trigger fires detached (S8) | T3 steps 6–7 + `a_subflow_parent_has_its_child_as_a_dependent_until_the_child_goes`, `a_replay_is_a_dependent_of_what_it_replayed`, `s8_a_flow_trigger_fire_is_detached_not_a_dependent`; T6 `details.executions`; T8 dependents links |
| Q3 replace, `system.*` kept, ended only | T1 `edit_labels_refusal`/`merge_system_labels`; T4; T6 labels tests |
| Q4 per-execution logs, Kestra filters, server download | T5; T6 download/delete tests |
| Q5 `deleteLogs=false` 400, `deleteMetrics` no-op, `deleteStorage` | T6 `delete_flags_follow_kestra_and_delete_logs_false_is_refused`; T3 `delete_storage_false_keeps_the_objects` |
| Q6 log delete while running | T6 `log_delete_ands_its_filters_works_while_running_and_is_audited` |
| Q7 UI on the execution page only | T8 |
| Q8 / D6 `labels_replaced`, one helper, every reader and copy | T1 migration + `visible_labels`; T2 (S6, S10) |
| D1 route, permission, 204, 404, trash ignored | T6 `delete_answers_204…`, `a_trashed_flow_does_not_block_a_delete`, `a_foreign_workspace_gets_404_on_all_four`, read-only test |
| D2 only a root | T3 `a_loop_iteration_is_not_a_root`, retry test; T6 `not_a_root` + `details.root` |
| D3 order, lock, family prefixes, skip, store failure | T3 (S2, S4, S5) + `an_ended_loop_root_is_deleted…`, `a_store_failure_deletes_no_row_and_can_be_repeated` |
| D4 audit with names, no value | T6 `delete_answers_204_and_is_audited_with_names_and_no_value` |
| D5 labels route, rules, audit by key | T1, T4, T6 |
| D7 download route, format, cap, no-cache | T5 `download_line`, `download_batch`; T6 two download tests (S11, S12) |
| D8 log delete route, ANDed, audit | T5, T6 |
| D9 codes | Global Constraints; T6 (S3 `Detailed`) |
| D10 header buttons, dialogs, `idle` | T6 `ExecutionDto.idle` (S7); T8 |
| D11 Logs tab download/delete, task-run row | T8 |
| D12 tests | T1–T6, T8 |
| D13 docs | T9; "After the tasks" |
| §5 divergences 1–9 | 1: T3/T6 `not_terminated`; 2: `has_dependents`; 9: T3 S8 test; 3: T6 flags test; 4: `not_a_root`; 5: T6 read-only test; 6: T4 `a_running_execution_cannot_be_relabelled`; 7: T6 cap test; 8: T1 caps cases |

**Decided 2026-11-16:** S8 (b) detach (T3 step 6, `s8_a_flow_trigger_fire_is_detached_not_a_dependent`); S9 (a) refuse (T3 step 5, `s9_a_pending_flow_trigger_event_refuses_the_delete`). No open point remains.
