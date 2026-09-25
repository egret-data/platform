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
-- The backstop unique index keys on the outbox EVENT that fired the
-- execution (`flow_trigger_event_id`), not on the source's state. An earlier
-- draft keyed on `(trigger_execution_id, flow_id, trigger id,
-- variables.state)`, and that made a legitimate second fire unrepresentable:
-- a source that re-enters a state (PAUSED -> RUNNING -> PAUSED) is a NEW
-- transition, Kestra fires its listeners on every transition, and the old key
-- let a listener hear only the first. Event ids are bigserial and never
-- reused, so "one fire per (event, listener)" is exactly the property.

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

-- The outbox event a Flow-trigger fire consumed. Not a foreign key: the event
-- row is deleted in the very transaction that fires (spec §4.4), so the id
-- outlives its row by design.
ALTER TABLE workflow.executions ADD COLUMN flow_trigger_event_id bigint;
-- Set iff the execution was fired by a `Flow` trigger, mirroring 0156's
-- `workflow_executions_scheduled_date_iff_schedule`. 0140's
-- `workflow_executions_trigger_record_is_whole` guarantees a non-NULL
-- `execution_trigger` carries a string `type`, so the right-hand side is never
-- NULL and this CHECK cannot pass on an unknown.
ALTER TABLE workflow.executions ADD CONSTRAINT workflow_executions_flow_trigger_event_iff_flow
  CHECK ((flow_trigger_event_id IS NOT NULL)
         = (execution_trigger IS NOT NULL
            AND execution_trigger->>'type' = 'io.kestra.plugin.core.trigger.Flow'));
-- A backstop, as W12's one-per-tick index: a listener fires at most once per
-- outbox event. Re-entering a state is a new event, so it fires again.
CREATE UNIQUE INDEX workflow_executions_one_per_flow_trigger_event
  ON workflow.executions (flow_trigger_event_id, flow_id, (execution_trigger->>'id'))
  WHERE flow_trigger_event_id IS NOT NULL;
