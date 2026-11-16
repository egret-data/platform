# Workflow package 7d: delete an execution, edit its labels, download and delete its logs

**Status:** designed 2026-11-16. Branch `feat/workflow-delete-labels-logs`, one PR. Roadmap row #7 (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`), fourth of the six sub-packages 7a–7f. 7a (#860), 7b (#866) and 7c (#868) are merged; their specs are `2026-11-13-workflow-execution-ops-restart-design.md`, `2026-11-14-workflow-replay-from-task-design.md` and `2026-11-15-workflow-pause-resume-design.md`. Kestra is the checkout at `data-portal/kestra`. `EC` is `webserver/src/main/java/io/kestra/webserver/controllers/api/ExecutionController.java`, `ES` is `core/src/main/java/io/kestra/core/services/ExecutionService.java`, `LC` is `webserver/src/main/java/io/kestra/webserver/controllers/api/LogController.java`.

## 1. Problem

Kestra has four execution operations egret lacks:

1. **Delete an execution** (`DELETE /{id}`, `EC:478-494`; `ES.delete`, `ES:747`). It hard-deletes the execution and, by flags that all default to `true`, its logs (`deleteLogs`), metrics (`deleteMetrics`) and internal storage under `kestra:///{ns path}/{flow slug}/executions/{id}` (`deleteStorage`). 404 for a missing id, else 204. It checks no state, kills nothing, and leaves subflow children alone.
2. **Set labels** (`POST /{id}/actions/labels`, `EC:2352-2380`). The body, a list of `{key, value}`, REPLACES the execution's labels; the existing `system.*` labels are re-appended, and a `system.*` label in the body that differs from an existing one is refused (`mergeSystemLabels`). Only a terminated execution (400 otherwise). Keys match `^\p{Ll}[\p{L}0-9._-]*$`, values are non-empty. 200 with the execution.
3. **Download logs** (`GET /logs/{id}/download`, `LC:114-137`). `text/plain`, attachment `<id>.log`, one line per entry as `LogEntry.toPrettyString` (`<instant ISO> <LEVEL> <message>`), joined with `\n`. Same filters as the log list. `Cache-Control: no-cache` while the execution is not terminated.
4. **Delete logs** (`DELETE /logs/{id}`, `LC:192-201`; `logRepository.deleteByQuery`). Optional `taskId`, `taskRunId`, `attempt`, `minLevel`, ANDed; `minLevel` deletes that level and above. No state check.

egret today (survey, 2026-11-16):

- Nothing deletes an execution. Flow purge keeps executions (`flow_id` becomes NULL, `repo.rs:757`). `task_runs`, `task_logs`, `execution_files`, `loop_file_values`, `flow_trigger_events` and loop/retry children (`parent_execution_id`) cascade on delete; `trigger_execution_id` (migration 0138) and `replayed_from_execution_id` (0166) are `NO ACTION`. Stored objects never cascade: `storage/prune.rs` deletes a family's prefixes (`own_prefix`, `prune.rs:524`) before its rows.
- `workflow.executions.labels` holds only the labels beyond the flow's own (migration 0192, `labels.rs`). Three readers merge it with the frozen source's `labels:` through `labels::execution_labels` (`routes.rs:1854` detail, `worker/render.rs:309` render, `flow_trigger/process.rs:808`). A `Labels` task writes the whole merged list back (`worker/claim.rs:5242`). So a flow label cannot be removed: it comes back on every read.
- `GET …/executions/{id}/logs` pages `workflow.task_logs` (`routes.rs:2254`). The frontend's Download button pages that API in the browser (`LogView.tsx`, `LOG_DOWNLOAD_MAX_LINES = 100_000`). No route deletes logs; `LogRepo::purge` exists for retention (`logs/repo.rs:413`).

## 2. Decisions

### 2.1 User decisions, 2026-11-16 (asked, answered)

- **Q1. Only an idle execution can be deleted.** The execution and its `afterExecution:` runs must have ended (`workflow.execution_is_idle`, migration 0210). Otherwise 409 `not_terminated`. *Corrected 2026-11-16 (plan S2, S9):* every member of the family must be idle (the root and every `parent_execution_id` descendant), and the delete also answers `not_terminated` while a family member still has a `workflow.flow_trigger_events` row (its Flow triggers have not all heard its end; user decision). Kill first, then delete. Kestra deletes in any state; egret refuses because a live worker would write into rows that are gone. §5 divergence 1.
- **Q2. An execution with dependents is refused.** If a subflow child (`trigger_execution_id` with a run ordinal) or a replay or restart (`replayed_from_execution_id`) points at it, 409 `has_dependents`, naming their ids. Delete those first. *Corrected 2026-11-16 (plan S8, user decision):* an execution a `Flow` trigger fired from it (`flow_trigger_event_id IS NOT NULL`) is not a dependent: the delete keeps it and sets its `trigger_execution_id` to NULL. §5 divergence 9. Kestra leaves dangling references. §5 divergence 2.
- **Q3. Labels follow Kestra:** the body replaces the whole visible list, `system.*` labels already present are kept, a new or changed `system.*` is refused, and only an ended execution can be edited.
- **Q4. Logs: per execution with Kestra's filters, plus a server download.** Delete by flow is out of scope.
- **Q5. Delete flags: only `deleteStorage` changes anything.** Logs always go with the execution (`task_logs` cascades), so `deleteLogs=false` is refused with 400. egret has no metrics; `deleteMetrics` is accepted and does nothing. §5 divergence 3.
- **Q6. Deleting logs of a running execution is allowed**, as in Kestra. Lines written after the delete stay.
- **Q7. UI on the execution page only.** Header: Delete and Edit labels. Logs tab: Download from the server API, Delete logs for the execution and for one task run. No list row actions; bulk is 7f.
- **Q8. Removing a flow label is possible (migration).** A new column `workflow.executions.labels_replaced boolean NOT NULL DEFAULT false`. When it is true, `labels` holds the WHOLE list and the flow's `labels:` are not merged in. Every reader goes through one helper (D6).
- **Design approval, 2026-11-16:** the user approved the design (§2.2–§2.3) with "ok tiếp tục đi".

### 2.2 Design decisions

- **D1. Delete route.** `DELETE /api/ws/{ws}/workflows/executions/{id}?deleteStorage=&deleteLogs=&deleteMetrics=`, all optional booleans defaulting to `true`. `workflow:manage`. 204 on success. A foreign or missing id is 404. A flow in the trash or purged does NOT block the delete: deleting is how old executions get cleaned up.
- **D2. Only a root.** A loop iteration or retry child (`parent_execution_id IS NOT NULL`) is deleted with its root, not alone: 409 `not_a_root`, naming the root. §5 divergence 4.
- **D3. Order: objects, then rows, under the execution lock.** The worker takes the execution lock, re-checks idle and dependents (the route's read can be stale), then, with `deleteStorage`, deletes each family member's prefix (the root and every `parent_execution_id` descendant) with `own_prefix`. A member whose prefix is not provably its own (`own_prefix` → `None`) is skipped and logged, as the pruner does. A store failure answers 500 and deletes no row, so the call can be repeated. Then `DELETE FROM workflow.executions WHERE id = $root` in the same transaction as the re-check; the cascade takes the rest. *Corrected 2026-11-16 (plan S2, S4, S5, S8):* the family is walked by this package (the pruner's `loop_family` stops at rows without a `loop_index`, so it misses retry replacements); each member's `_wd/` snapshots (`FileStore::working_dir_root`, outside `own_prefix`) are deleted too; the lock is the row's own (`FOR UPDATE`, in a plain transaction, not `claim::load`, which plans the source); the worker makes every check, the route none; Flow-trigger fires are detached before the dependents check.
- **D4. Audit.** `workflow_execution:delete` with `{namespace, flow_key, state, delete_storage, executions, files, detached}` (`detached`: Flow-trigger fires unlinked, plan S8): the row is gone afterwards, so the detail names what it was. No input or output value.
- **D5. Labels route.** `POST /api/ws/{ws}/workflows/executions/{id}/labels`, body `[{"key","value"}]`, `workflow:manage`. Under the execution lock: not idle → 409 `not_terminated`. Validation, each 400 `invalid_labels` naming the label: the rules of `backfill_labels_refusal` (≤ 50 labels, non-blank key, no duplicate, non-empty value, key ≤ 128 and value ≤ 1024 characters) without its template-syntax rule, which is backfill-specific, plus Kestra's key pattern `^\p{Ll}[\p{L}0-9._-]*$` for a key that is not already in the visible list (a flow may carry older keys that do not match, and resubmitting them must not fail); a `system.*` key in the body passes only if the same key and value already exist. The stored list is the body plus every existing `system.*` label not in it; `labels_replaced` is set true. 200 with the execution detail. Audit `workflow_execution:labels` with `{keys_before, keys_after}`; no value.
- **D6. One helper for the visible list.** `labels::visible_labels(flow: &[Label], own: &[Label], replaced: bool) -> Vec<Label>`: `replaced` → `own` deduplicated (empty key or value dropped, last value in first position); else `execution_labels(flow, own)`. The three readers (detail, render, flow trigger) call it and read `labels_replaced` beside `labels`. A `Labels` task on a replaced execution merges into the visible list and writes it back with `labels_replaced` kept. Replay and restart on a new source copy `labels_replaced` with `labels` (Kestra copies the full list). The list DTO's `labels` (own only) shows the whole list when `labels_replaced`. *Corrected 2026-11-16 (plan S6, S10):* a `CREATE_NEW_EXECUTION` retry replacement and a `Loop` iteration copy `labels` too, so they copy the flag; the list DTO and the `Labels` task's write need no code (the column is the whole list, and the write leaves the flag alone).
- **D7. Log download route.** `GET /api/ws/{ws}/workflows/executions/{id}/logs/download?task_id=&task_run_id=&min_level=`, `workflow:read`, the same filters and authorisation as `GET …/logs`. `text/plain; charset=utf-8`, `Content-Disposition: attachment; filename="<id>.log"`, one line per entry `<ts RFC 3339 UTC, millis> <LEVEL> <message>`, joined with `\n`. At most 100 000 lines (the frontend's existing cap); past it the last line is `… truncated at 100000 lines`. `Cache-Control: no-cache` while the execution is not idle. No settle lag: the download reads what is committed. *Corrected 2026-11-16 (plan S11, S12):* the body is streamed in id-ordered batches of 2 000 lines (a line can be 15 360 characters), and the log status gains `idle` for the header.
- **D8. Log delete route.** `DELETE /api/ws/{ws}/workflows/executions/{id}/logs?task_id=&task_run_id=&attempt=&min_level=`, `workflow:manage`, 204. ANDed filters; `min_level` deletes that level and above. No state check. One `DELETE … WHERE execution_id = $1 AND …` (the execution already scoped to the workspace by the route's read). Audit `workflow_execution:logs_delete` with the filters and the row count.
- **D9. Error codes.** New: `not_terminated` (409), `has_dependents` (409, `details.executions`, and `details.total` since at most 20 are named), `not_a_root` (409, `details.root`), `invalid_labels` (400), `delete_logs_required` (400, for `deleteLogs=false`). *Corrected 2026-11-16 (plan S3):* no error body had `details`; these two 409s use a new `DetailedErrorBody { error, code, details }`, and `egret_core::http::ErrorBody` is unchanged.

### 2.3 UI, tests, docs

- **D10. Execution page header.** A **Delete** button opens a confirm dialog with a "Delete stored files" checkbox (checked) and the text that logs and loop iterations go with it; on success, navigate to the executions list with a toast. An **Edit labels** button opens a dialog with key/value rows (the flow's labels and the execution's, `system.*` read-only), add and remove. Both are enabled only when the execution is idle (the detail's new `idle` field, plan S7), with a tooltip otherwise. 409 messages are shown in the dialog.
- **D11. Logs tab.** Download calls the server route with the current filters (an anchor to the URL). A **Delete logs** menu item deletes with the current filters after a confirm; the task-run row menu gets **Delete logs** for that run. Both need `workflow:manage` and refresh the view.
- **D12. Tests.** Pure: `visible_labels`, label validation, the pretty line. Worker: delete under lock (idle, dependents, root, store failure keeps rows, family prefixes), labels replace with system kept. Routes: every code above, audit rows without values, permission (`workflow:read` cannot delete or edit), foreign workspace 404. Render: a replaced list reaches `{{ labels }}`; a `Labels` task after an edit keeps the removal.
- **D13. Docs.** `egret-data/docs`: Executions → Run actions (Delete, Edit labels), Logs (download, delete), REST reference, screenshots of both dialogs and the Logs menu.

### 2.4 Plan corrections, 2026-11-16

From the implementation plan `docs/en/plans/2026-11-16-workflow-delete-labels-logs.md` ("Spec corrections"). S8 and S9 were put to the user and decided on 2026-11-16; the others inform. The text above carries each where it changes a decision.

- **S1.** Migration `0231`, not `0230` (taken by `feat/svc-accounts-sa2`).
- **S2.** The delete's family is every `parent_execution_id` descendant, walked by the delete itself; `loop_family` misses retry replacements. Idle is checked on every member.
- **S3.** `has_dependents` and `not_a_root` answer a new `DetailedErrorBody { error, code, details }`; `has_dependents` also gives `details.total`.
- **S4.** With `deleteStorage`, each member's `_wd/` snapshots (`working_dir_root`) are deleted too; nothing could find them after the rows go.
- **S5.** Delete and set-labels lock the row in a plain transaction (`FOR UPDATE` / `FOR NO KEY UPDATE`), not through `claim::load`, so an execution whose source no longer plans can still be deleted or relabelled. The worker makes every check; delete retries a deadlock as the kill does.
- **S6.** The retry replacement (`worker/replay.rs`) and the `Loop` iteration (`worker/spawn.rs`) copy `labels_replaced` beside `labels`, as replay and restart on a source do.
- **S7.** The detail gains `idle` (terminal and no live run), computed from the runs it holds.
- **S8 (decided: detach).** A `Flow`-trigger fire (`flow_trigger_event_id IS NOT NULL`) is not a dependent: in the delete's transaction its `trigger_execution_id` is set to NULL (migration 0138's CHECKs accept it) and it is kept; a later refusal rolls that back. It loses its "Child of" link and read access to the deleted execution's files. Audit `detached`.
- **S9 (decided: refuse).** While a family member has a `flow_trigger_events` row, the delete answers 409 `not_terminated` ("try again in a minute"); the row would cascade and its listeners never fire. Bounded by the processor's 5 attempts.
- **S10.** The list DTO's `labels` and the `Labels` task's write need no code change.
- **S11.** The download streams id-ordered batches of 2 000 lines (`download_batch`) rather than collecting 100 000 lines; a database error aborts the body, never a short file.
- **S12.** `ExecutionLogStatus` gains `idle` for the download's `Cache-Control`.
- **S13.** None of the four routes looks at the flow's trash state; only D1 said so, and the labels and log routes follow it.
- **S14.** The delete's toast after navigating stays (FE-ACTION-2 forbids a toast only when navigating to the result).
- **S15.** The log delete's `attempt` is 0-based, as the log API serves it.

**As-built deviations, 2026-11-16 (T1-T8):**

- The migration is `0231_execution_labels_replaced.sql` (S1 confirmed).
- T3 locks the delete's family to a fixpoint: after locking it re-reads the descendants and repeats until no new member appears, so a concurrent insert under a member cannot slip in.
- T4 locks the row first and then checks idle in a second statement, not in the lock query.
- T6 made `DetailedErrorBody.details` optional (absent on `not_terminated`) and logs a download error that happens mid-body.
- T8 appends new label rows after the `system.*` rows in the label editor.

## 3. Shape of the change

- Migration `0231_execution_labels_replaced.sql`: the column (*corrected 2026-11-16, plan S1:* 0230 is taken by `feat/svc-accounts-sa2`).
- `labels.rs`: `visible_labels`, `edit_labels_refusal`, `merge_system_labels`.
- `worker/delete.rs`: `delete_execution` under the lock; `worker/labels.rs`: `set_execution_labels`.
- `logs/repo.rs`: `delete_for_execution(filters)`, `download_batch(filters, after, limit)` (*corrected, plan S11*).
- `routes.rs`: four routes, OpenAPI, codes; the three label readers switched to `visible_labels`; restart/replay copy the flag.
- Frontend: regenerated client (`npm run gen:api`), `DeleteExecutionDialog`, `EditLabelsDialog`, Logs tab menu, header buttons.

## 4. Tests

See D12. Gates as in 7c: `egret-workflow` all targets (the 7 `sql_starrocks_live` tests need StarRocks), srv/config/metastore, ratchet and census unchanged, frontend lint/typecheck/build/bundle test, live check on a throwaway DB and bucket.

## 5. Recorded divergences from Kestra

Security- or integrity-driven:

1. A running or paused execution cannot be deleted (409 `not_terminated`); Kestra deletes it and a live worker may keep writing.
2. An execution with subflow children or replays pointing at it cannot be deleted (409 `has_dependents`); Kestra leaves dangling references.
3. `deleteLogs=false` is refused (logs cascade with the row); `deleteMetrics` does nothing (egret has no metrics).
4. A loop iteration or retry child cannot be deleted alone (409 `not_a_root`).
5. `workflow:manage` for delete, labels and log delete; `workflow:read` for download. Kestra OSS checks nothing.

Other:

6. Labels editing waits for the execution to be idle (afterExecution runs included); Kestra checks `isTerminated` only.
7. Download is capped at 100 000 lines with a truncation line; Kestra streams everything.
8. Labels validation also caps count and lengths (egret's backfill rules).
9. Deleting an execution clears `trigger_execution_id` on the executions its `Flow` triggers fired (they are kept); Kestra leaves the reference dangling. A delete also waits for its Flow triggers to have heard its end (409 `not_terminated`), so no fire is lost.

## 6. Out of scope

Delete logs by flow (`LC:204`), bulk by ids or query (7f), a label filter on the executions list, metrics.

## 7. As built

(Filled in at the end of the implementation.)
