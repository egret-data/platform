-- 0192, not 0190 or 0191: origin/main ends at 0189, feat/view-governance holds 0186-0188,
-- and the egret-tagdef worktree (fix/tag-policies-service-def) holds 0190 and 0191
-- (committed there, 2026-10-15), so 0192 collides with none of them in any merge order.
-- The W7f spec (D17, §3.1) names 0191; it was taken after the spec was written.
--
-- Workflow W7f: schedule-trigger backfill. See
-- docs/en/specs/2026-10-15-workflow-w7f-backfill-design.md §3.1-§3.3.

-- A backfill in progress, one per `Schedule` trigger. Its own table, not
-- columns of workflow.triggers: `sync` deletes every workflow.triggers row of
-- a flow whose source does not parse, and a running backfill must survive
-- that, as 0189's switch rows do. No foreign key to workflow.triggers for the
-- same reason. `ON DELETE CASCADE` fires on a hard delete of the flow only.
-- `end` and `current_date` are reserved words, hence `window_start`,
-- `window_end` and `current_tick`.
CREATE TABLE workflow.trigger_backfills (
  flow_id                       uuid        NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  -- The `Schedule` trigger's `id:` in the YAML.
  trigger_id                    text        NOT NULL,
  -- Copied from the flow for filtering; not an authority.
  workspace_id                  uuid        NOT NULL,
  -- This backfill's identity, written on every execution it fires
  -- (workflow.executions.backfill_id). A new backfill over the same window
  -- has a new id, so it re-runs those dates (user decision, 2026-10-15).
  id                            uuid        NOT NULL DEFAULT gen_random_uuid() UNIQUE,
  window_start                  timestamptz NOT NULL,
  window_end                    timestamptz NOT NULL,
  -- The next window tick to fire: equal to workflow.triggers.next_evaluation_date
  -- while the trigger row is on the queue. Not `current_date`, a reserved word.
  current_tick                  timestamptz NOT NULL,
  paused                        boolean     NOT NULL DEFAULT false,
  paused_by                     uuid        REFERENCES users (id) ON DELETE SET NULL,
  paused_at                     timestamptz,
  -- Overlaid on the schedule's `inputs:` key by key.
  inputs                        jsonb       NOT NULL DEFAULT '{}'::jsonb,
  -- [{key, value}], no `system.` key, in the order given.
  labels                        jsonb       NOT NULL DEFAULT '[]'::jsonb,
  -- The regular schedule's next date when the backfill was created. Written
  -- at create and re-snapped by `sync` on a cron change; never by pause/resume.
  previous_next_evaluation_date timestamptz NOT NULL,
  total_ticks                   integer     NOT NULL,
  ticks_done                    integer     NOT NULL DEFAULT 0,
  created_by                    uuid        REFERENCES users (id) ON DELETE SET NULL,
  created_at                    timestamptz NOT NULL DEFAULT now(),
  updated_at                    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (flow_id, trigger_id),
  CHECK (window_start < window_end),
  CHECK (current_tick >= window_start),
  CHECK (jsonb_typeof(inputs) = 'object'),
  CHECK (jsonb_typeof(labels) = 'array'),
  CHECK (paused = (paused_at IS NOT NULL)),
  CHECK (total_ticks BETWEEN 1 AND 10000 AND ticks_done BETWEEN 0 AND total_ticks)
);

-- The backfill that fired this execution (D2). No foreign key to
-- workflow.trigger_backfills: the row is deleted when the backfill ends, and
-- its executions stay. A column and not a key in `execution_trigger`, which
-- must stay the record Kestra stores.
ALTER TABLE workflow.executions ADD COLUMN backfill_id uuid;
-- A backfill execution is a schedule fire: 0156's CHECK ties scheduled_date to it.
ALTER TABLE workflow.executions ADD CONSTRAINT workflow_executions_backfill_is_a_schedule_fire
  CHECK (backfill_id IS NULL OR scheduled_date IS NOT NULL);

-- The per-tick index of 0156 now covers regular fires only (user decision, 2026-10-15:
-- a backfill re-runs dates that already ran). 0156's prose is not edited.
-- `workflow_executions_one_per_backfill_tick` keeps one execution per
-- (backfill, tick); `scheduler::start::classify` maps a violation of either to
-- `AlreadyFired`.
--
-- **Not `CONCURRENTLY`, so the rebuild blocks writes to `workflow.executions`
-- for its duration during the deploy** (as 0189's index does). sqlx runs this
-- file in one transaction, and `CREATE INDEX CONCURRENTLY` cannot run in a
-- transaction block; opting the file out would make the table and comments
-- above non-atomic with it, and would leave a moment with no per-tick index.
-- The window is one pass over the scheduled executions, the only rows the
-- partial indexes hold.
DROP INDEX workflow.workflow_executions_one_per_schedule_tick;
CREATE UNIQUE INDEX workflow_executions_one_per_schedule_tick
  ON workflow.executions (flow_id, (execution_trigger->>'id'), scheduled_date)
  WHERE scheduled_date IS NOT NULL AND backfill_id IS NULL;
CREATE UNIQUE INDEX workflow_executions_one_per_backfill_tick
  ON workflow.executions (backfill_id, scheduled_date) WHERE backfill_id IS NOT NULL;

-- The labels this execution was started with beyond its flow's own `labels:`
-- ([{key, value}], Kestra's list shape). NULL: none. The flow's labels are
-- read from `source`, which is the execution's frozen copy.
ALTER TABLE workflow.executions ADD COLUMN labels jsonb
  CHECK (labels IS NULL OR jsonb_typeof(labels) = 'array');
