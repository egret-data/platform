-- 0159: origin/main ends at 0158 (W13b) and no remote branch holds a 0159
-- (checked on every `origin/*` ref before naming this file).
--
-- Workflow W11: the durable log of an execution's task runs -- what Kestra
-- keeps in its `logs` index and egret, until now, sent only to `tracing`. See
-- docs/en/specs/2026-09-27-workflow-w11-execution-logs-design.md §3 (storage),
-- §6 (the cursor and its settle lag) and §7 (retention).
--
-- The spec's table, with each shape it asked to verify checked against the
-- code:
--
-- * `execution_id` is `uuid`: `workflow.executions.id` is
--   `uuid PRIMARY KEY DEFAULT gen_random_uuid()` (0134). Deleting an execution
--   cascades its lines, as `task_runs` and 0157's outbox already do.
--
-- * `workspace_id` carries NO foreign key. The house pattern: a top-level
--   table references `workspaces(id) ON DELETE CASCADE` (`flows`, 0132;
--   `executions`, 0134), while a table hanging off an execution carries
--   `workspace_id` as a plain column for its own filters (0157's
--   `flow_trigger_events` and `flow_listeners`). Deleting a workspace still
--   removes every line: workspaces -> executions -> task_logs, both cascades.
--   The column is here so the read API can filter on BOTH ids (spec §6: another
--   workspace's execution is a 404) without a join.
--
-- * `task_run_id` carries NO foreign key, but NOT for the reason the plan gave.
--   The plan's argument: the worker holds `task_runs` rows `FOR UPDATE` while
--   the writer inserts, an FK check takes `FOR KEY SHARE`, and W13a measured a
--   `40001` from that shape. Measured on this branch's Postgres (16.15), in a
--   scratch database with a parent/child FK, a READ COMMITTED inserter against
--   a holder that:
--     1. took `FOR NO KEY UPDATE` and did a non-key `UPDATE`  -> insert at once;
--     2. took `FOR UPDATE`                                   -> insert WAITS;
--     3. ran claim's `UPDATE ... WHERE id IN (SELECT ... FOR UPDATE)` -> WAITS;
--     4. updated the row after a REPEATABLE READ inserter's snapshot -> 40001;
--     5. the same interleaving as 4, inserter at READ COMMITTED -> insert ok.
--   And in the code: the only `FOR UPDATE` on `task_runs` is inside
--   `worker::claim::claim`'s single autocommit statement (called with the
--   pool), not held across a handler; every other `task_runs` write
--   (`settle`, the reaper, outputs) is a plain `UPDATE` of non-key columns --
--   the table's only unique keys are `id` and `(execution_id, run_ix)` --
--   which takes `FOR NO KEY UPDATE` and does not conflict with `KEY SHARE`.
--   `40001` needs REPEATABLE READ (case 4 vs 5; W13a's processor runs there,
--   the log writer does not). So an FK would cost a wait of one claim
--   statement at most, never a `40001`: the lock argument is REFUTED.
--   The column stays unconstrained because an FK buys nothing here:
--     - `task_runs` rows are never deleted except by the execution cascade,
--       which `execution_id`'s FK already applies to these lines;
--     - a single-column FK would not even say the run belongs to THIS
--       execution (that needs a composite FK and a new unique key on 0134's
--       table);
--     - it would add a `KEY SHARE` row lock, per inserted line, on the rows
--       the worker updates most.
--   NULL means a line about the execution, not about one run.
--
-- * No `truncated` column (user decision D6): a long line is SPLIT into
--   15,360-character pieces, as Kestra's `RunContextLogger` splits it, and
--   each piece is its own row.
--
-- * `attempt` is 0-based (controller ruling): `TaskInput.attempt - 1`,
--   matching Kestra and indexing the execution API's per-run `attempts`.
--
-- `ts` is when the line was produced (the worker's clock). `inserted_at` is
-- the database's clock and only the read cursor looks at it: ids are
-- allocated at insert and committed later, so the API returns only rows with
-- `inserted_at < now() - interval '2 seconds'`, and a reader's cursor cannot
-- move past a row that is still to commit (spec §6).
CREATE TABLE workflow.task_logs (
  id            bigserial   PRIMARY KEY,
  workspace_id  uuid        NOT NULL,
  execution_id  uuid        NOT NULL REFERENCES workflow.executions (id) ON DELETE CASCADE,
  task_run_id   uuid,
  task_id       text,
  attempt       integer,
  level         smallint    NOT NULL CHECK (level BETWEEN 0 AND 4),  -- TRACE..ERROR
  ts            timestamptz NOT NULL,
  inserted_at   timestamptz NOT NULL DEFAULT now(),
  message       text        NOT NULL
);

-- The read cursor: `WHERE execution_id = $1 AND id > $2 ORDER BY id`.
CREATE INDEX workflow_task_logs_execution_id ON workflow.task_logs (execution_id, id);
-- Retention: `DELETE ... WHERE ts < now() - <days>`, in batches.
CREATE INDEX workflow_task_logs_ts ON workflow.task_logs (ts);
