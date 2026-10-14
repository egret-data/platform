-- 0189, not 0186: origin/main ends at 0185 and the unmerged feat/view-governance branch holds 0186-0188, so 0189 collides with neither in either merge order.
--
-- Workflow W7e: the runtime switch of a trigger. See docs/en/specs/2026-10-14-workflow-w7e-triggers-design.md §3.1.
--
-- A row means OFF; enabling deletes it, so there is no `disabled` column (an
-- "enabled" row would mean the same as no row). It lives in a table of its own
-- because `sync` deletes every `workflow.triggers` and `workflow.flow_listeners`
-- row of a flow whose source does not parse, and a `Webhook` trigger has no row
-- at all; the switch must survive both. `sync` copies it into those rows'
-- `disabled` (the effective value, spec §3.2) and deletes the rows of ids the
-- source no longer declares, or all of them on a soft delete (spec §3.4).
CREATE TABLE workflow.disabled_triggers (
  flow_id      uuid        NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  -- The trigger's `id:` in the YAML, of any kind.
  trigger_id   text        NOT NULL,
  -- Copied from the flow for filtering; not an authority.
  workspace_id uuid        NOT NULL,
  disabled_by  uuid        REFERENCES users (id) ON DELETE SET NULL,
  disabled_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (flow_id, trigger_id)
);

-- The meaning of the two old columns changes. Stated here rather than by
-- editing the prose of 0156 and 0157, which stays as written.
COMMENT ON COLUMN workflow.triggers.disabled IS
  'Effective: flow YAML disabled OR trigger YAML disabled OR a row in workflow.disabled_triggers (W7e).';
COMMENT ON COLUMN workflow.flow_listeners.disabled IS
  'Effective: flow YAML disabled OR trigger YAML disabled OR a row in workflow.disabled_triggers (W7e).';

-- The last execution of each trigger (spec §3.6): one descent per
-- `(flow_id, trigger id)` probe. `workflow_executions_flow_idx` (0134) is on
-- `flow_id` only, and `workflow_executions_one_per_schedule_tick` (0156) covers
-- schedules only and orders by `scheduled_date`. Executions started from the
-- API or by replay carry no `execution_trigger` and are outside the index.
--
-- Measured, as 0183 measures: over 3,000 triggered executions of one flow,
-- `EXPLAIN` of the statement without this index is a Seq Scan of
-- `workflow.executions` plus a Sort under the LIMIT; with it, the planner
-- chooses this index and no scan.
-- `tests/disabled_triggers.rs::the_last_execution_per_trigger_is_served_by_its_index`
-- pins the plan, so a change that loses it fails there.
CREATE INDEX workflow_executions_last_by_trigger
  ON workflow.executions (flow_id, (execution_trigger->>'id'), created_at DESC)
  WHERE execution_trigger IS NOT NULL;
