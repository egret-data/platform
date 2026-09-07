-- Workflow W3b Task 5: what a `{{ … }}` renders against, and the logical clock
-- the happens-before relation is read from.
--
-- Three things this table could not say, each of which stopped the real driver
-- reproducing something the differential oracle compares.
--
-- 1. **A task's OUTPUT was not stored anywhere.** `worker::Outcome::outputs`
--    existed from Task 3 and died at the end of the tick, so
--    `{{ outputs.<id> }}` — 14 of the comparable corpus entries read one —
--    could never resolve. Task 3's own concern names this: "nothing persists
--    `Outcome::outputs`, so no *durable* thing compares it yet".
--
-- 2. **An execution's INPUTS were not stored either.** `harvest.py` POSTed
--    inputs when it captured the golden and `oracle-exec/corpus.json` records
--    them; a driver that cannot carry them renders `{{ inputs.param }}`
--    against nothing and takes the other branch. Kestra's own execution
--    carries its inputs for the same reason.
--
-- 3. **Nothing ordered two writes inside one transaction.** The oracle
--    compares a happens-before relation over `(created, ended)` endpoints, and
--    a timestamp cannot supply it: every row a `settle` writes shares one
--    `now()`, because `now()` in Postgres is TRANSACTION time. That is the
--    same shape W3b Task 4 measured on Kestra's own side — all 670 loop
--    sub-executions carry their parent's `startDate` and all 220 sibling
--    groups share one instant, so the instant does not order siblings badly,
--    it does not order them at all. `seq` is therefore a LOGICAL clock: one
--    tick per round of `advance()` and one per render pass, which is exactly
--    the serialisation the relation is a claim about.
ALTER TABLE workflow.executions
  -- The inputs this execution was started with, as posted. `{}` for a flow
  -- with no `inputs:`; the flow's own declared `defaults:` are applied at
  -- render time rather than frozen here, because a default is a property of
  -- the FLOW and re-reading it keeps one copy of it.
  ADD COLUMN inputs jsonb NOT NULL DEFAULT '{}'::jsonb,
  -- The execution's logical clock. Monotone, never reset, and advanced by the
  -- worker rather than by the database: a `DEFAULT` or a trigger would tie it
  -- to the row's writes instead of to `advance()`'s rounds.
  ADD COLUMN seq bigint NOT NULL DEFAULT 0;

ALTER TABLE workflow.task_runs
  -- `worker::Outcome::outputs`, i.e. what `{{ outputs.<task id> }}` reads.
  --
  -- **NULL and `{}` are not the same thing here and the difference is
  -- Kestra's, measured on the artifact.** NULL means the handler returned no
  -- `Output` object at all — Kestra's `Log`, `Sleep` and a falsy `Fail` all
  -- `return null` — but an EXPRESSION cannot see that distinction: a flow
  -- whose second task renders `{{ outputs.a }}` printed `{}` for all three,
  -- and `{{ outputs }}` over two such tasks printed `{"a":{},"b":{}}`. So the
  -- column keeps the handler's answer verbatim and the renderer maps NULL to
  -- `{}` (see `worker::render`), rather than the column losing the difference.
  ADD COLUMN outputs jsonb,
  -- The `executions.seq` at which this run was CREATED and at which it reached
  -- a terminal state — the two endpoints of the happens-before relation the
  -- oracle compares, and the reason `seq` exists.
  --
  -- **Creation, not start.** W3a measured that taking `StartRun` as the left
  -- endpoint made egret's interval a strict sub-interval of Kestra's, so
  -- `Kestra-before` implied `egret-before` and never the converse — an
  -- over-acceptance. Kestra's own left endpoint is creation: `state.startDate`
  -- equals `histories[0].date` in 4215 of 4215 captured runs and
  -- `histories[0].state` is `CREATED` in all 4215.
  ADD COLUMN created_seq bigint,
  ADD COLUMN ended_seq bigint;
