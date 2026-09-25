-- 0157, not 0154/0155: origin/main ends at 0156 and the unmerged SP4a branch
-- holds 0154 and 0155, so 0157 collides with neither in either merge order
-- (globals.md, "Migration numbers").
--
-- Workflow W13a: the plumbing for a Flow trigger (no `dependsOn`) that starts
-- a flow because another execution in its workspace reached a state. See
-- docs/en/specs/2026-09-25-workflow-w13-flow-triggers-design.md §4.1.
--
-- Reproduced verbatim from the spec, including `flow_trigger_events.done` and
-- `.attempts`, which W13a Task 2 does not yet read (the processor is Task 4)
-- but which belong on the table from its first row rather than as a later
-- ALTER.
--
-- The unique index keys directly on
-- `execution_trigger->'variables'->>'state'` (no typed `trigger_source_state`
-- column) because W13a Task 1 measured, on the pinned artifact, that
-- `trigger.variables.state` is the bare enum name (`"SUCCESS"`), not an
-- object -- so the spec's original expression stands and the typed-column
-- fallback the spec allows for is not needed.

-- One row per Flow trigger a flow declares, kept by scheduler::sync.
CREATE TABLE workflow.flow_listeners (
  flow_id      uuid    NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  trigger_id   text    NOT NULL,
  workspace_id uuid    NOT NULL,
  states       text[]  NOT NULL,  -- Flow.states after defaults
  depends_on   boolean NOT NULL,  -- which arm
  disabled     boolean NOT NULL,  -- flow YAML disabled OR trigger disabled
  PRIMARY KEY (flow_id, trigger_id)
);
CREATE INDEX workflow_flow_listeners_ws ON workflow.flow_listeners (workspace_id) WHERE NOT disabled;

-- The outbox: one row per source transition that some listener could hear.
CREATE TABLE workflow.flow_trigger_events (
  id                  bigserial   PRIMARY KEY,
  source_execution_id uuid        NOT NULL REFERENCES workflow.executions (id) ON DELETE CASCADE,
  workspace_id        uuid        NOT NULL,
  state               text        NOT NULL,
  done                text[]      NOT NULL DEFAULT '{}', -- listeners already handled
  attempts            integer     NOT NULL DEFAULT 0,
  created_at          timestamptz NOT NULL DEFAULT now()
);

ALTER TABLE workflow.executions ADD COLUMN execution_depth integer NOT NULL DEFAULT 0;
-- A backstop, as W12's one-per-tick index: a listener fires at most once per
-- (source, source state).
CREATE UNIQUE INDEX workflow_executions_one_per_flow_trigger_event
  ON workflow.executions (trigger_execution_id, flow_id, (execution_trigger->>'id'),
                          (execution_trigger->'variables'->>'state'))
  WHERE trigger_execution_id IS NOT NULL
    AND execution_trigger->>'type' = 'io.kestra.plugin.core.trigger.Flow';
