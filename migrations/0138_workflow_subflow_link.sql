-- Workflow W6 Task 2: the two child executions `parent_execution_id` cannot
-- express, and the CHECK that keeps the third one out of the oracle's
-- `spawned`.
--
-- Migration 0135 gave `workflow.executions` one way to be a child --
-- `(parent_execution_id, parent_run_ix, loop_index, loop_value)`, a `Loop`
-- iteration -- and a CHECK that says those four are all set or all null:
--
--     (parent_execution_id IS NULL
--      AND parent_run_ix IS NULL AND loop_index IS NULL AND loop_value IS NULL)
--     OR
--     (parent_execution_id IS NOT NULL
--      AND parent_run_ix IS NOT NULL AND loop_index IS NOT NULL
--      AND loop_value IS NOT NULL)
--
-- That is one sentence with two consequences, and W6 needs the first without
-- the second:
--
-- 1. it forbids a HALF-written loop link, which is what it was for -- a child
--    that knows its parent but not its index is not orderable, and the
--    ordering is the whole of what the oracle compares `spawned` on;
-- 2. it also forbids **a child with a parent and no loop index at all**, which
--    is exactly the shape a `CREATE_NEW_EXECUTION` replay has.
--
-- Kestra's own distinction, measured on the pinned artifact
-- (`kestra/kestra@sha256:28f59bf1...`, `oracle-exec/traces.json`) rather than
-- read off a class:
--
--   | kind             | parentId | loopRun | link Kestra uses     |
--   |------------------|----------|---------|----------------------|
--   | loop iteration   | set      | set     | parentId             |
--   | replay           | set      | ABSENT  | parentId             |
--   | subflow child    | **NULL** | absent  | trigger_execution_id |
--
-- The replay row is not inferred: over the four `valids/retry-new-execution-*`
-- flows, every one of the eight descendants carries a `parentId`, a NULL
-- `loopIndex` and a NULL `loopValue`, and `harvest.py`'s `spawnedBy`
-- (Kestra's `loopRun.taskRunId`) is NULL for all eight. Their golden entries
-- record it on the other side of the comparison too: each `spawned` element is
-- `{"parent_run": null, "value": null, ...}`.
--
-- The subflow row is measured by an ABSENCE that is visible: `harvest.py`
-- discovers descendants by following `parentId`, and `valids/subflow-parent`
-- -- a flow whose only task is a `Subflow` that Kestra ran to SUCCESS -- has
-- `"descendants": []` in all five rounds. The child exists; nothing keyed on
-- `parentId` can see it.
--
-- # Why this is a second column and not a widening
--
-- `oracle-exec/driver.rs`'s `read_trace_inner` builds the oracle's `spawned`
-- field with `WHERE parent_execution_id = $1`. Linking a `Subflow` child that
-- way would put it in egret's trace, and **all 41 golden `spawned`-carrying
-- entries have zero `Subflow` children** -- the entries W6 exists to bank
-- would become `spawned` mismatches instead. So the constraint that matters
-- most here is not that the new column exists; it is
-- `workflow_executions_one_link_kind` below, which makes "linked both ways"
-- unrepresentable rather than merely discouraged.
ALTER TABLE workflow.executions
  -- The execution that CAUSED this one without owning it: the parent of a
  -- `Subflow` child (W6 Task 3), and later the execution a `Flow` trigger
  -- fired on (W6 Task 6). Kestra's `Execution.trigger.variables.executionId`.
  --
  -- **No `ON DELETE` action, i.e. NO ACTION, and that is the difference from
  -- `parent_execution_id`.** 0135 chose CASCADE because a loop
  -- sub-execution *is a part of* its parent's record. A subflow child is not:
  -- it is a full execution of a DIFFERENT flow, with its own row in the
  -- executions list, and deleting the flow that called it must not delete it.
  -- SET NULL is not available either -- it would null this column while
  -- leaving `trigger_run_ix` set, which the CHECK below forbids, so the delete
  -- would fail with a confusing constraint error instead of a foreign-key one.
  -- Nothing in `src/` deletes an execution row today (grepped: there is no
  -- `DELETE FROM workflow.executions` outside one test), so NO ACTION refuses
  -- a case that does not arise rather than guessing at it; a workspace
  -- deletion still cascades, because it removes parent and child in one
  -- statement and the referential check runs after it.
  --
  -- **The reference itself is declared below, as a composite foreign key that
  -- carries `workspace_id`** -- see *Both links are workspace-scoped*. A plain
  -- `REFERENCES workflow.executions(id)` would let a child in one workspace
  -- name a trigger in another.
  ADD COLUMN trigger_execution_id uuid,
  -- WHICH taskrun of `trigger_execution_id` caused this execution -- the
  -- `RunIx`, addressed positionally exactly as `parent_run_ix` is, and with no
  -- foreign key for the same reason that column has none.
  --
  -- Set for a `Subflow` child; NULL for a `Flow`-trigger-fired execution,
  -- which is caused by an execution reaching a state and by no particular
  -- taskrun. Without it a flow with two `Subflow` tasks could not tell which
  -- child belonged to which run -- `valids/subflow-parent-retry` is that flow,
  -- with `subflow1` and `subflow2` targeting the same `(namespace, flowId)`.
  ADD COLUMN trigger_run_ix       integer;

-- # Both links are workspace-scoped, and neither was before
--
-- `parent_execution_id` (0135) and `trigger_execution_id` are id columns that
-- point at a row in another workspace just as happily as at one in this
-- workspace: `executions_parent_execution_id_fkey` checks that the row EXISTS
-- and says nothing about whose it is. **This repository has already paid for
-- that shape once** -- five columns of exactly this kind, one of which
-- (`chart_id`) allowed a cross-workspace read, patched only after it shipped.
--
-- The fix is the standard one: make `workspace_id` part of the reference, so
-- the child can only name a row that carries the SAME workspace. Postgres
-- needs a unique key on the referenced pair, and `id` is already the primary
-- key, so `(id, workspace_id)` is unique by construction -- the added index is
-- the whole cost.
--
-- **Both columns, not just the new one.** Closing it for `trigger_execution_id`
-- alone would leave a guard that reads as complete and is not -- the same
-- reason `workflow_executions_no_self_link` below closes 0135's hole as well
-- as this migration's. `worker::spawn::spawn_one` is the only writer of
-- `parent_execution_id` and it binds the parent's own `workspace_id`, so no
-- row can be violating this today; a database that somehow holds one fails
-- this migration loudly, which is the right direction for data that is
-- already wrong.
--
-- 0135's single-column foreign key is REPLACED rather than kept beside the new
-- one, so there is exactly one referential statement per column and one error
-- a reader has to interpret. `ON DELETE CASCADE` is carried over unchanged:
-- 0135's reasoning for it -- a loop sub-execution is a part of its parent's
-- record -- is untouched by this.
ALTER TABLE workflow.executions
  ADD CONSTRAINT workflow_executions_id_workspace_key UNIQUE (id, workspace_id);

ALTER TABLE workflow.executions
  DROP CONSTRAINT executions_parent_execution_id_fkey;

ALTER TABLE workflow.executions
  ADD CONSTRAINT workflow_executions_parent_same_workspace
    FOREIGN KEY (parent_execution_id, workspace_id)
    REFERENCES workflow.executions (id, workspace_id) ON DELETE CASCADE,
  -- No `ON DELETE` action, for the reason given on the column above. MATCH
  -- SIMPLE is what makes a top-level execution legal under both of these: with
  -- the link NULL and `workspace_id` NOT NULL the key is partially null, and
  -- the reference is not checked at all.
  ADD CONSTRAINT workflow_executions_trigger_same_workspace
    FOREIGN KEY (trigger_execution_id, workspace_id)
    REFERENCES workflow.executions (id, workspace_id);

-- 0135's CHECK, re-stated so that it still forbids a half-written loop link
-- but no longer forbids a parent without one.
--
-- Renamed rather than kept: "all or nothing" would now be a false description
-- of what it enforces, and a constraint whose name says something else is a
-- constraint a reader trusts for the wrong reason.
ALTER TABLE workflow.executions
  DROP CONSTRAINT workflow_executions_loop_run_all_or_nothing;

ALTER TABLE workflow.executions
  -- The loop triple is whole or absent, and it needs a parent when present.
  -- Still unrepresentable: a loop index with no parent, a parent run with no
  -- index, an index with no value. Newly representable: a parent alone -- the
  -- replay.
  ADD CONSTRAINT workflow_executions_loop_run_whole_or_absent CHECK (
    (parent_run_ix IS NULL AND loop_index IS NULL AND loop_value IS NULL)
    OR
    (parent_execution_id IS NOT NULL
     AND parent_run_ix IS NOT NULL AND loop_index IS NOT NULL
     AND loop_value IS NOT NULL)
  ),
  -- **The one the package's banks depend on.** A child is linked one way or
  -- the other, never both, so a `Subflow` child cannot be reached by the
  -- `parent_execution_id` query that builds `spawned`. A writer that forgets
  -- this returns 200 and moves 41 golden entries; the database refusing it is
  -- the difference between a rule and a guarantee.
  ADD CONSTRAINT workflow_executions_one_link_kind CHECK (
    parent_execution_id IS NULL OR trigger_execution_id IS NULL
  ),
  -- A run ordinal into an execution that is not named is not an address.
  ADD CONSTRAINT workflow_executions_trigger_run_needs_its_execution CHECK (
    trigger_run_ix IS NULL OR trigger_execution_id IS NOT NULL
  ),
  -- Neither link may name the row itself. The foreign keys do not stop this:
  -- an INSERT that supplies its own `id` for `parent_execution_id` satisfies
  -- the reference, because the row exists by the time the constraint is
  -- checked. 0135 left that hole for `parent_execution_id`; closing it for the
  -- new column only would leave a guard that reads as complete and is not, so
  -- this closes both.
  --
  -- **It is added VALIDATING, so a database already holding a self-parented
  -- row fails this migration** rather than carrying the row forward. That is
  -- deliberate: `NOT VALID` would install a constraint nobody has ever seen
  -- evaluated AND keep the corrupt row, which is strictly worse than a loud
  -- stop. Nothing in `src/` can write such a row -- `worker::spawn::spawn_one`
  -- lets `id` default to `gen_random_uuid()` and binds a parent that already
  -- existed -- so the failure mode is a hand-written row, and a hand-written
  -- row that names itself is exactly what this wants to hear about.
  ADD CONSTRAINT workflow_executions_no_self_link CHECK (
    parent_execution_id IS DISTINCT FROM id
    AND trigger_execution_id IS DISTINCT FROM id
  );

-- 0135's unique index, with `NULLS NOT DISTINCT`.
--
-- Its argument is unchanged -- two children of one parent that sort equal make
-- the `spawned` order undetermined, which is what `normalise.py` has to BAIL
-- on, because the creation instant carries no information at all. What changes
-- is that the key can now contain NULLs: a replay child's `parent_run_ix` and
-- `loop_index` are both NULL, and under the default NULLS DISTINCT rule any
-- number of them could hang off one parent, tie under
-- `ORDER BY parent_run_ix, loop_index`, and be ordered by nothing.
--
-- One replay child per parent is the artifact's own behaviour, measured rather
-- than assumed: over the 710 descendants in `traces.json`, 40 carry a NULL
-- `loopIndex` and **no parent has more than one of them** -- Kestra chains a
-- `CREATE_NEW_EXECUTION` retry (root -> replay -> replay) instead of fanning
-- it out. The same sweep found **no parent that mixes** a NULL-index child
-- with loop children, which is why nothing here has to decide where a replay
-- sorts among iterations.
--
-- Recreated under the same NAME so that `spawn.rs` and `oracle-exec/README.md`
-- keep referring to something that exists. It changes nothing for a row that
-- was already legal: before this migration the key could not contain a NULL at
-- all.
--
-- **The drop and rebuild are not `CONCURRENTLY`, so they take a write lock,
-- and that is forced rather than chosen.** sqlx runs each migration file
-- inside one transaction unless the file opts out, and `CREATE INDEX
-- CONCURRENTLY` cannot run inside a transaction block at all -- opting out
-- would mean the ALTERs above stop being atomic with this, which is a worse
-- trade than a brief outage. The window is bounded by `workflow.executions`,
-- one row per execution, and the ALTERs above already take ACCESS EXCLUSIVE on
-- the same table for the duration, so the index rebuild does not add a lock
-- that was not already held.
DROP INDEX workflow.workflow_executions_loop_run_idx;

CREATE UNIQUE INDEX workflow_executions_loop_run_idx
  ON workflow.executions (parent_execution_id, parent_run_ix, loop_index)
  NULLS NOT DISTINCT
  WHERE parent_execution_id IS NOT NULL;

-- The lookup the other direction: "what did this execution's run N spawn".
--
-- **Not unique, and that is measured on what HAPPENED rather than on what was
-- declared.** A `Subflow` task may carry its own `retry:`, and each attempt
-- starts a NEW child execution under the SAME `(execution, run_ix)`. Two
-- corpus flows declare such a retry -- but the golden shows the retries
-- actually occurring, which is the stronger form: in
-- `valids/subflow-parent-retry` the runs `subflow1` and `subflow2` each carry
-- **2 attempts** (`… FAILED, RETRYING, CREATED, RUNNING, SUCCESS`), and in
-- `valids/retry-subflow` the single `subflow` run carries **3**. So several
-- children really do hang off one `(execution, run_ix)`, and a unique index
-- here would make the second attempt unrepresentable.
CREATE INDEX workflow_executions_trigger_idx
  ON workflow.executions (trigger_execution_id, trigger_run_ix)
  WHERE trigger_execution_id IS NOT NULL;
