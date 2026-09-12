-- Workflow W6 Task 7: the record of WHY an execution exists.
--
-- Kestra's `Execution.trigger` is an `ExecutionTrigger` -- `{id, type,
-- variables}` -- and for a webhook the `variables` half is the whole of what
-- the caller sent: the body, the headers and the query parameters. Measured on
-- the pinned artifact (`kestra/kestra@sha256:28f59bf1...`) by driving a real
-- request at a real server:
--
--     "trigger": {
--       "id": "hook",
--       "type": "io.kestra.plugin.core.trigger.Webhook",
--       "variables": {
--         "body": {"hello": "world", "n": 3},
--         "headers": {"Content-Type": ["application/json"], ...},
--         "parameters": {"validationToken": ["abc", "def"], "x": ["1"]}
--       }
--     }
--
-- Without this column a webhook execution has no place to put any of that, so
-- the request that started it is lost the moment the response is written. Every
-- one of the fifteen corpus flows that declares a `Webhook` reads it
-- (`{{ trigger | json }}`, `{{ trigger.body }}`,
-- `{{ trigger.parameters.validationToken[0] }}`), so an engine that does not
-- store it cannot ever serve them.
--
-- # It is STORED here and deliberately NOT RENDERED yet
--
-- `worker::render::Surface::context` publishes `inputs`, `vars`, `flow`,
-- `labels`, `execution`, `item`, `outputs`, `tasks` and `taskrun`. It does NOT
-- publish `trigger`, and W6 Task 7 did not add it -- `worker/render.rs` carries
-- two findings already assigned to Task 8, and publishing this root is that
-- task's, on top of this column.
--
-- **The reason is the 293 other entries, not the one this task was warned
-- about.** The plan predicted that publishing a `trigger` root would make
-- `valids/webhook-plaintext` "diverge further". It would not: that entry
-- renders `{{ trigger.parameters.validationToken[0] ?? 'hello-world' }}`, which
-- misses at `trigger.parameters` and then indexes `.validationToken` on the
-- result, so a defined-but-empty root errors exactly as an absent one does.
-- Measured directly rather than argued: the root was temporarily added to
-- `render.rs`, `executor_matches_golden` was run with `--nocapture`, and the
-- ratchet read **127 mismatching / 127 baselined, test result ok** -- the same
-- figure as without it, and `ok` rather than a count is the stronger statement,
-- because the ratchet fails on a bank as well as on a regression. Zero entries
-- moved. `render.rs` was then restored and verified byte-identical.
--
-- What stays true is the plan's own rule: every one of the 294 golden entries
-- was harvested through a plain execution POST, so an unconditional `trigger`
-- root changes the render context of 293 executions that have nothing to do
-- with webhooks. An entry that moves for a reason like that is a coincidence
-- rather than a bank, so the root arrives when a task owns the file it lives in.
ALTER TABLE workflow.executions
  -- `io.kestra.core.models.executions.ExecutionTrigger`, whole.
  --
  -- NULL for every execution nobody triggered, which is all of them before this
  -- migration and all but the webhook-started ones after it. NULL rather than a
  -- `'{}'` default on purpose: "no trigger" and "a trigger that carried
  -- nothing" are different facts, and `{}` cannot tell them apart.
  --
  -- One column rather than three (`trigger_id`, `trigger_type`,
  -- `trigger_variables`) because the three are written together, read together,
  -- and have exactly one shape -- the same call
  -- `catalog_metadata_jsonb_then_normalize` made for `TableMetadata`. The CHECK
  -- below is what stops that becoming an excuse for a half-written value.
  ADD COLUMN execution_trigger jsonb;

ALTER TABLE workflow.executions
  -- A trigger record is whole or absent.
  --
  -- `{id, type, variables}` are the three fields `ExecutionTrigger.of(trigger,
  -- output)` sets, and a row carrying two of them is not a weaker record, it is
  -- an unreadable one: `trigger.id` with no `trigger.type` cannot say which
  -- plugin produced the variables beside it, and `variables` with no `id`
  -- cannot be attributed to a trigger the flow still declares.
  --
  -- Added VALIDATING. Nothing can be violating it -- the column did not exist
  -- one statement ago -- so the only way this fails is a hand-written row, which
  -- is exactly the row it wants to hear about. `NOT VALID` would install a
  -- constraint nobody has ever seen evaluated, which W6 Task 4's own migration
  -- already refused for the same reason.
  ADD CONSTRAINT workflow_executions_trigger_record_is_whole CHECK (
    execution_trigger IS NULL
    OR (
      jsonb_typeof(execution_trigger) = 'object'
      AND execution_trigger ? 'id'
      AND execution_trigger ? 'type'
      AND execution_trigger ? 'variables'
      AND jsonb_typeof(execution_trigger -> 'id') = 'string'
      AND jsonb_typeof(execution_trigger -> 'type') = 'string'
      AND jsonb_typeof(execution_trigger -> 'variables') = 'object'
    )
  );
