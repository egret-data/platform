-- Package #6 (spec D9, D11): the values of `type: SECRET` outputs, sealed with
-- AES-256-GCM under `secrets.master_key` exactly as migration 0203 seals
-- SECRET inputs (`egret_workflow::secret_inputs`): the 12-byte nonce followed
-- by the ciphertext of one JSON object. The readable column beside each holds
-- `******` for every SECRET value. NULL when there is no SECRET value. No read
-- of an execution or a task run selects these columns; only the worker's
-- render surface opens them.
--
-- * `workflow.task_runs.secret_outputs`: a `Subflow` run's copy of its
--   child's outputs (Task B3/B4), one sealed object; and a `Loop` run's list
--   (Task A4), since fix wave 6 (final review I1) append-only FRAMES, one per
--   entry that has a SECRET value: a 4-byte big-endian list position, a
--   4-byte big-endian length, then that entry's `{output id: value}` sealed
--   as above (`secret_inputs::seal_loop_entry`). An iteration end appends
--   one frame in SQL and never re-seals the list.
-- * `workflow.task_runs.outputs_bytes` (fix wave 6, final review I1 and I3):
--   a `Loop` run's running total for its 16 MiB cap -- the bytes of its
--   `outputs` list as serialised, plus the clear size of every SECRET value
--   in it -- so an iteration end checks the cap from this number and its
--   own entry, without reading or re-serialising the list. NULL = nothing
--   appended yet; NULL on every other run.
-- * `workflow.executions.outputs`: the flow-level `outputs:` evaluated when
--   the execution ends (Task B1). NULL = no outputs evaluated; executions
--   that ended before this migration had none, so there is no backfill.
-- * `workflow.executions.secret_outputs`: their SECRET values, `{output id:
--   value}` (Task B4).
--
-- Written by Task A4, the first task that stores a SECRET output value, so
-- that no task of the package ever stores one in clear; the two
-- `workflow.executions` columns are the same migration's (D11: one migration),
-- used from Task B1.
--
-- `IF NOT EXISTS` (fix wave 6): the file was edited before it merged, so a
-- database that applied an earlier draft by hand re-applies it cleanly.
-- No `lock_timeout`: no migration of this repository sets one (final review
-- m11, recorded in the spec); each statement is a metadata-only `ADD COLUMN`.
ALTER TABLE workflow.executions ADD COLUMN IF NOT EXISTS outputs jsonb;
ALTER TABLE workflow.executions ADD COLUMN IF NOT EXISTS secret_outputs bytea;
ALTER TABLE workflow.task_runs ADD COLUMN IF NOT EXISTS secret_outputs bytea;
ALTER TABLE workflow.task_runs ADD COLUMN IF NOT EXISTS outputs_bytes bigint;
