-- Workflow W8 Task 4 fix round 2: **a paused run names who resumes it.**
--
-- `RunState::Paused` is a unit variant and `Command::PauseRun` recorded no
-- owner, so "which construct ends this wait" lived in two hand-written lists
-- of `TaskKind`s -- one in `executor::flowable::sequential::walk`, one in
-- `executor::flowable::parallel::drive_concurrent` -- each behind a catch-all
-- that answers "wait" for everything else.
--
-- **That shape is a silent stall, and it was measured rather than feared.** A
-- real `Command::PauseRun` applied to a run of any third kind leaves it at
-- `["CREATED","SUBMITTED","RUNNING","PAUSED"]` for ever -- 20 rounds past its
-- deadline, 0 commands emitted -- under `Sequential`, `Parallel` AND `Dag`,
-- while a `Pause` in the same fixture moves. Nothing failed: no exhaustive
-- match, no source-reading guard, and the two lists agreed with each other.
-- W8 Task 4 shipped exactly that defect once already (a `LoopUntil` under a
-- `Parallel`) and fix round 1 repaired the instance by adding a row to both
-- lists, which is the same shape again.
--
-- So the fix is structural: `Command::PauseRun` carries a `Resume`, and this
-- column is where it survives the settle that ends when the run is parked.
-- The resume side then reads the run and never the kind, the two lists
-- collapse to one site, and **a construct that pauses cannot fail to say who
-- resumes it** -- it is a field, not a list to remember.
--
-- # Why the deadline is NOT duplicated here
--
-- `Resume::Terminal { at, state }` and `Resume::Iterate { at }` both carry the
-- instant the wait ends, and that instant is already `not_before_ms` -- the one
-- column `worker::claim::reopen_due` and `oracle-exec/driver.rs::pass_time`
-- read, both with no state term at all. Storing it twice would be two clocks
-- that can disagree, so this column stores only what `not_before_ms` cannot:
-- the discriminant, and for a `Pause` the terminal its bound chose.
--
--     NULL                a run that has never been paused
--     'NEVER'             an unbounded `Pause` -- `not_before_ms` IS NULL
--     'ITERATE'           a `LoopUntil` -- `not_before_ms` is the next iteration
--     'TERMINAL:SUCCESS'  a `Pause` whose `pauseDuration:` elapses
--     'TERMINAL:FAILED'   a `Pause` whose `timeout:` elapses
--
-- The CHECK below makes the pairing with `not_before_ms` structural rather
-- than remembered: an unbounded pause with a deadline, or a bounded one
-- without, cannot be written. That is the invariant `flowable::pause`'s
-- `Bound` enum states in the executor, held at the other end of the round trip.
ALTER TABLE workflow.task_runs
  ADD COLUMN resume_with text;

ALTER TABLE workflow.task_runs
  -- The vocabulary, so an unknown discriminant is refused at write time rather
  -- than decoded into a `WorkerError::Corrupt` on some later read. `SUCCESS`
  -- and `FAILED` are the only two terminals `flowable::pause::deadline`
  -- produces -- `pauseDuration:` and `timeout:` respectively, measured on
  -- `valids/pause-delay` and `valids/pause-timeout`.
  ADD CONSTRAINT workflow_task_runs_resume_with_known CHECK (
    resume_with IS NULL
    OR resume_with IN ('NEVER', 'ITERATE', 'TERMINAL:SUCCESS', 'TERMINAL:FAILED')
  ),
  -- **A wait has a deadline exactly when something ends it.** `NEVER` is the
  -- unbounded `Pause` that on the pinned artifact ends only on a human resume,
  -- and it writes no `not_before_ms` at all -- which is what keeps the
  -- driver's clock from ever being asked to move for it. Every other
  -- discriminant carries an instant.
  ADD CONSTRAINT workflow_task_runs_resume_bound_matches CHECK (
    resume_with IS NULL
    OR (resume_with = 'NEVER' AND not_before_ms IS NULL)
    OR (resume_with <> 'NEVER' AND not_before_ms IS NOT NULL)
  );
