-- Package #6 (spec D9, D11): the values of `type: SECRET` outputs, sealed with
-- AES-256-GCM under `secrets.master_key` exactly as migration 0203 seals
-- SECRET inputs (`egret_workflow::secret_inputs`): the 12-byte nonce followed
-- by the ciphertext of one JSON object. The readable column beside each holds
-- `******` for every SECRET value. NULL when there is no SECRET value. No read
-- of an execution or a task run selects these columns; only the worker's
-- render surface opens them.
--
-- * `workflow.task_runs.secret_outputs`: a `Loop` run's list
--   (`{list position: {output id: value}}`, Task A4) and a `Subflow` run's
--   copy of its child's outputs (Task B3/B4).
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
ALTER TABLE workflow.executions ADD COLUMN outputs jsonb;
ALTER TABLE workflow.executions ADD COLUMN secret_outputs bytea;
ALTER TABLE workflow.task_runs ADD COLUMN secret_outputs bytea;
