-- Package #12 Task 3 (spec Task 0 §4). A `Subflow` child is linked to the run
-- that spawned it only by `(trigger_execution_id, trigger_run_ix)` (0138). A
-- `LoopUntil` reuses ONE run across its iterations (`Command::RestartLoop`
-- empties the body's runs in place), so two children of a looped `Subflow` land
-- under one `(execution, run_ix)` pair -- and a parent restart, which matches an
-- earlier child by that pair, cannot tell them apart. Kestra disambiguates by
-- `taskRunIteration` (`ExecutableUtils.java:113-117`); this column carries it on
-- the child row so the restart matches the right iteration's child by identity
-- rather than by list position.
--
-- `0` for a child not inside a `LoopUntil` (the common `Subflow`, where the pair
-- is already unique) and for every non-child execution; the iteration number,
-- `>= 1`, for a child of a looped `Subflow`.
ALTER TABLE workflow.executions
  ADD COLUMN trigger_run_iteration integer NOT NULL DEFAULT 0;
