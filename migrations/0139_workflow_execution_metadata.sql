-- Workflow W6 Task 4: Kestra's `ExecutionMetadata`, which is the only thing
-- that can stop a `CREATE_NEW_EXECUTION` retry chain.
--
-- Migration 0138 made the replay's ROW representable -- a child with a parent
-- and no loop index. It did not make the replay DECIDABLE, and the two facts
-- that decide it are the two fields Kestra keeps in
-- `core/src/main/java/io/kestra/core/models/executions/ExecutionMetadata.java`:
--
--     Integer attemptNumber = 1;
--     Instant originalCreatedDate;
--
-- Both are read by the same two methods, one per place a `CREATE_NEW_EXECUTION`
-- retry can be declared:
--
--   * a TASK's `retry:` -- `TaskRun.nextRetryDate(retry, execution)`
--     (`TaskRun.java:310`);
--   * the FLOW's `retry:` -- `ExecutionService.nextRetryDate(retry, execution)`
--     (`ExecutionService.java:1272`).
--
-- and each reads them the same way:
--
--     if (retry.getMaxAttempts() != null
--         && execution.getMetadata().getAttemptNumber() >= retry.getMaxAttempts())
--         return null;
--     ...
--     if (retry.getMaxDuration() != null
--         && nextDate.isAfter(execution.getMetadata()
--                                      .getOriginalCreatedDate()
--                                      .plus(retry.getMaxDuration())))
--         return null;
--
-- **Without these two columns the chain cannot terminate**, and that is a
-- measurement rather than a worry. `executor::retry::next_retry_at` reads
-- `(1, Tick(0))` for this behaviour today -- "this execution IS the first
-- attempt, and `Tick`'s origin is its own creation" -- which is true of the
-- FIRST execution and false of every replay. With `attemptNumber` pinned at 1,
-- `1 >= 3` is never true and `retry-new-execution-{flow,task}-attempts` retry
-- for ever; with the origin pinned at each execution's own creation, no
-- elapsed time ever accumulates and `-{flow,task}-duration` do the same. All
-- four golden entries end their chain at the THIRD execution, which is
-- `FAILED` and spawns nothing.
--
-- # `origin_offset_ms` is a SNAPSHOT, and it cannot be derived
--
-- Kestra stores an absolute instant. This stores the same fact relatively:
-- **how many milliseconds of the ORIGINAL execution's clock had already
-- elapsed when this execution was created.** `0` for every execution that is
-- not a replay, which is all of them until W6 Task 4.
--
-- It is a stored snapshot rather than `own.created_at - original.created_at`
-- computed on read, and the reason is the differential oracle's own clock.
-- `oracle-exec/driver.rs`'s `pass_time` makes time pass by moving an
-- execution's `created_at` BACKWARDS by the shortfall to its next deadline --
-- that is the whole of its intervention. A chain of replays therefore
-- accumulates simulated time *inside* each link, and a subtraction taken at
-- read time between the last link and the first cannot see any of it: it would
-- read ~2 s of a chain that has simulated ~4 s, and the third execution would
-- schedule a fourth. Adding the parent's elapsed AT THE INSTANT THE CHILD IS
-- CREATED puts the whole accumulation in the row, where a later rewind of an
-- ancestor's `created_at` cannot reach it.
--
-- `bigint`, and no upper bound: it is `Tick`'s own type
-- (`executor::state::Tick(pub i64)`), so a value this column can hold is a
-- value `advance()` can read.
ALTER TABLE workflow.executions
  -- `execution.getMetadata().getAttemptNumber()`: 1 for an execution nobody
  -- replayed, N for the (N-1)th replay of one. It counts EXECUTIONS of a
  -- `CREATE_NEW_EXECUTION` chain and is unrelated to `task_runs.attempts`,
  -- which counts a `RETRY_FAILED_TASK`'s attempts INSIDE one execution -- the
  -- two clocks `executor/retry.rs`'s header table separates.
  ADD COLUMN attempt_number   integer NOT NULL DEFAULT 1,
  -- `execution.getMetadata().getOriginalCreatedDate()`, as an offset. See
  -- above for why it is not derived.
  ADD COLUMN origin_offset_ms bigint  NOT NULL DEFAULT 0;

ALTER TABLE workflow.executions
  -- A count of attempts starts at one. `@Min(1)` on Kestra's own
  -- `maxAttempts`, and `ExecutionMetadata`'s `@Builder.Default` of 1.
  ADD CONSTRAINT workflow_executions_attempt_number_positive
    CHECK (attempt_number >= 1),
  -- Time does not run backwards from the original: a replay is created after
  -- the execution it replaces, never before it.
  ADD CONSTRAINT workflow_executions_origin_offset_non_negative
    CHECK (origin_offset_ms >= 0),
  -- **The metadata belongs to a replay and to nothing else.**
  --
  -- Only `worker::replay::spawn_one` writes a value other than the default,
  -- and the row it writes is exactly 0138's newly-representable one: a parent
  -- with no loop columns. Stating that as a CHECK rather than as a rule keeps
  -- the two halves of "this is a replay" from drifting apart -- a loop
  -- iteration or a subflow child that claimed `attempt_number = 2` would make
  -- `next_retry_at` refuse a retry for an execution that has never been
  -- retried, silently, and no test asserting the SHAPE of a replay would see
  -- it. `parent_run_ix IS NULL` is what distinguishes a replay from a loop
  -- iteration under 0138's `workflow_executions_loop_run_whole_or_absent`; a
  -- subflow child has no `parent_execution_id` at all.
  --
  -- `origin_offset_ms` is deliberately NOT constrained to be non-zero in the
  -- replay arm: `interval: PT0S` is a legal (if useless) retry, and a replay
  -- created with no elapsed time is then the honest record rather than a
  -- constraint violation.
  ADD CONSTRAINT workflow_executions_metadata_belongs_to_a_replay CHECK (
    (attempt_number = 1 AND origin_offset_ms = 0)
    OR (attempt_number > 1
        AND parent_execution_id IS NOT NULL
        AND parent_run_ix IS NULL)
  );
