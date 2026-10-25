# Hang in `a_reaped_run_is_still_lost_not_killed`

## Symptom

Under load (CI, or locally with many test threads and CPU hogs) the test
`worker::claim::tests::a_reaped_run_is_still_lost_not_killed` in `egret-workflow`
never finished: the test binary ran until it was killed (once for 54 minutes).

## Cause (measured)

The test joined two futures: the worker (`run_one`, bounded by a 20 s
`timeout`) and a second future that waited on the handler's `started`
notification, with no bound, before moving the lease fence.

`run_one` does not always reach the handler. `under_lease` makes its first
lease renewal before it polls the handler, and gives the run up (the
self-fence, `Leased::Lost`) when no renewal has succeeded within
`working_timeout`, counted from before `run_one` reads the task's input. The
test used `working_timeout = 600 ms`. On a loaded machine the input reads and
the first renewal took longer than that, so the worker returned
`Ok(LeaseLost)` without ever starting the handler. Nothing notified
`started`, and the `join!` waited for ever.

A diagnostic build (a 30 s bound on that wait that reports what the worker
returned) failed 2 of 30 stressed runs with: handler never started; worker
finished with `Ok(Ok(LeaseLost))`; handler not dropped (it never ran).

This is the documented product behaviour (a worker that cannot prove its lease
within `working_timeout` gives the run up), not a product bug.

## Fix (test only)

- `KillSpy::started_while` waits for `started` while it polls the worker. If
  the worker returns first, the test fails at once and prints what the worker
  returned. Every wait in the test that depends on another task is now bounded
  by the worker's own 20 s `timeout`.
- The test's `working_timeout` is 3 s (a renewal every second still finds the
  moved fence), so the self-fence before the handler no longer happens under
  load and the moved fence is what the test sees.
- The sibling helper `kill_a_sleeping_run` (used by
  `the_worker_cancels_and_writes_killed` and `a_cancel_that_errs_still_ends_killed`)
  had the same unbounded wait on `started` and uses the same helper now.
