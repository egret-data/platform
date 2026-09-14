-- W8 Task 5: a `Pause` whose `timeout:` fires can now end `WARNING`, so the
-- `resume_with` vocabulary migration 0141 wrote has a third terminal in it.
--
-- 0141's CHECK named `TERMINAL:SUCCESS` and `TERMINAL:FAILED` and said why:
-- those were "the only two terminals `flowable::pause::deadline` produces --
-- `pauseDuration:` and `timeout:` respectively". That was exactly true when it
-- was written and is what makes this migration necessary rather than optional:
-- the vocabulary is enforced at WRITE time, so a widening of the executor's
-- answer that did not come here would be refused by Postgres rather than
-- silently accepted -- which is the CHECK doing its job.
--
-- **What widened it is one line of `ExecutorService`, read off the pinned
-- artifact** (`kestra/kestra@sha256:28f59bf1…`, `javap -p -c`):
--
--     .state(duration != null ? behavior.mapToState() : State.Type.fail(pauseTask))
--
-- A `pauseDuration:` bound takes `behavior.mapToState()` and is untouched by
-- the two flags; a `timeout:` bound takes `State.Type.fail(pauseTask)`, which
-- is `FAILED` / `WARNING` / `SUCCESS` as `allowFailure:` and `allowWarning:`
-- read. `valids/pause-timeout-allow-failure` is the corpus flow that writes
-- the middle one, and its golden pause run is
-- `["CREATED","RUNNING","PAUSED","WARNING"]`.
--
-- `TERMINAL:SUCCESS` already covers the third combination (both flags on), so
-- the row added here is the only one missing. The two `allow*` flags cannot
-- reach `ITERATE` or `NEVER`: `Resume::Iterate` is a `LoopUntil`'s and
-- `Resume::Never` is an unbounded pause, neither of which has a terminal at
-- all.
--
-- The `not_before_ms` pairing CHECK is untouched -- a `TERMINAL:WARNING` is a
-- bounded wait exactly as the other two terminals are, so it falls under the
-- existing `resume_with <> 'NEVER'` arm without naming anything.
ALTER TABLE workflow.task_runs
  DROP CONSTRAINT workflow_task_runs_resume_with_known;

ALTER TABLE workflow.task_runs
  ADD CONSTRAINT workflow_task_runs_resume_with_known CHECK (
    resume_with IS NULL
    OR resume_with IN (
      'NEVER',
      'ITERATE',
      'TERMINAL:SUCCESS',
      'TERMINAL:FAILED',
      'TERMINAL:WARNING'
    )
  );
