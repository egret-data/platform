# CI flakes, batch 3: seven tests (2026-10-25)

Seven tests were listed as known flakes. Batch 1 (#730) and batch 2 (#732) already fixed some of
them. For each test this records the cause, the fix, and the same stress run before and after.
Machine: 8 cores, shared with other sessions (load average 30 to 130 during the runs). "Load" means
4 extra busy-loop CPU hogs on top of that.

CI history: the failed CI runs from 2026-10-05 to 2026-10-24 (194 runs), plus the first attempts
of the runs that were re-run.

## Fixed here

### `egret-dataapi` `serving::router::tests::a_drift_is_logged_against_the_statement_that_drifted`

- **Seen:** failed on 2026-10-24 and on 2026-10-25, both times at its first `contains` with
  **empty** logs.
- **Cause (confirmed):** tracing-core caches a callsite's interest the first time any thread
  reaches it. When only one dispatcher is registered (this test's scoped capture, after the other
  captures were dropped), it does not ask the registered dispatchers. It asks the default of the
  thread that reached the callsite. A parallel test with no subscriber that reaches the drift
  `warn!` first in that window caches `never`, and the capture then records nothing.
- **Fix (test code):** the capture helper sets a global subscriber once per process. That
  subscriber answers `sometimes` for every callsite and enables nothing. With it, the registry
  always holds two dispatchers, and no callsite is cached as `never`. The event still goes only to
  the emitting thread's own subscriber.
- **Evidence:** a scratch test, never committed. A second thread with no subscriber reaches the
  drift `warn!` while the capture is the only dispatcher. Before the fix it failed **5/5** with
  empty logs. After the fix it passed **10/10**. Without the scratch test, the router tests at
  `--test-threads 32` passed 40/40 before the fix, so the natural rate is too low to measure on
  this machine.

### `egret-workflow` `triggers_routes::off_then_on_for_each_kind` (and `bulk_switching_is_per_entry_and_bounded`)

- **Seen:** on 2026-10-24 the enable audit carried `next_evaluation_date: "…14:05:00Z"` where the
  test asserts `null`.
- **Cause (confirmed):** the seed dates the schedule at the next minute. If a minute boundary
  passes before the switch back on, the date has passed. The re-enable then skips to the next
  tick, which is correct product behaviour, and audits that date. The bulk test has the same
  window between seeding `mix` and the bulk switch (0.15 to 0.55 s).
- **Fix (test code):** each test sets the schedule's date an hour ahead before switching. The
  sibling tests already do this.
- **Evidence:** the test binary was started at a chosen second of the minute.
  - `off_then_on`, before: **failed 3/3** when started at :56, :57 and :58 (the setup takes about
    2 s before the seed), and passed at :52, :54 and :58.5 to :59.8. After: **0/7** failures when
    started between :55 and :59.
  - Bulk test, before: **1/8** failed when started at :52.2 to :52.9, with the same audit
    message. After: **0/10** at the same offsets.

### Frontend `WhoCanReadDialog.test.tsx` "Tab from the field with the list open lands on Explain"

- **Seen:** failed on `main` on 2026-10-23 and on a branch:
  `expected <div role="dialog"> to be <body>`, at the check that `<body>` is focused between
  focusout and focusin.
- **Cause (confirmed):** the test waited for its two microtasks inside `await act(async …)`. In
  React 19 an async act ends by queueing a macrotask (`setImmediate`). If any React update lands
  in that task (a query result, the 200 ms debounce), act flushes it and waits for one more task.
  In that task the list's close timer (`setTimeout(0)`) runs and removes the list while `<body>`
  holds focus, and the dialog's FocusScope then takes focus. A Tab in the browser has no task
  between focusout and focusin.
- **Fix (test code):** wait for the microtasks with plain `await`, not inside an act.
- **Evidence:** a scratch test, never committed. It schedules an unrelated React state update with
  `setImmediate` just before the step. Before the fix: **3/3** failures with the CI message. After
  the fix: **5/5** passes. The fixed test still catches the defect it guards: when the hook closes
  the list on blur at once, the test fails. Plain loops of the whole file under load passed
  20/20 both before and after.

### Frontend ChartExplore mount family (`ChartExplore.mount`, `ChartExplore.dirty`, …)

- **Seen:** not in CI within the scanned window. Locally, under parallel load, the first test of
  `ChartExplore.mount` and of `ChartExplore.dirty` timed out. Each had been given a 15 s budget to
  hide this.
- **Cause (confirmed):** each file imported the route with `await import("../ChartExplore")`
  inside its mount helper. That cold import of the whole route ran inside whichever test mounted
  first.
- **Fix (test code):** all seven ChartExplore test files import the route at the top, so the
  import is paid while the file loads. The two cold-import 15 s budgets are removed; the first
  tests run with the default 5 s.
- **Evidence:** the two files were run in turns, before and after, 6 rounds each, under load. The
  first pie test took **5.3 to 7.6 s** before (over the 5 s default every time) and **0.6 to
  1.0 s** after. The first dirty test took 5.5 to 7.7 s before and 0.7 to 1.5 s after.
- **Not fixed:** at a load average around 130, other ChartExplore tests also passed 5 s once. That
  is CPU starvation of tests that mount a whole route, not ordering. The two remaining 15 s budgets
  (KI-051, cartesian Change) are for tests that mount twice, and are left alone.

## Already fixed on main by #730: confirmed, no change

None of these three has failed in CI since #730 merged on 2026-10-22 at 09:53Z. Before that they
failed often: the claim test 11 times and declassifying 4 times in the scanned runs.

- `worker::claim::tests::a_discard_only_block_claim_survives_renewal_and_the_poll`: **15/15**
  passes in the whole `worker::` lib suite at `--test-threads 64` under load.
- `worker::commands::tests::a_store_failure_mid_copy_kills_docker_cp_before_its_stdin_closes`:
  **15/15** passes in the same runs.
- `declassifying_removes_the_derived_view_tags_and_keeps_manual_ones`: after, **0/30** failures of
  the binary under load. With #730's change reverted in a scratch build: **1/30** runs failed, with
  the `authz_service_defs_name_key` 23505 race that #730 names. It hit a sibling test, which #730
  says any of the five can be.

## Load-only worker flakes (#732): measured again, not changed

These are from the same 15 runs of the `worker::` suite under load:

- `a_budget_cancel_in_flight_when_the_kill_lands_is_asked_again`: failed 5/15.
- `a_budget_that_expired_on_a_killed_run_cancels_once`: failed 5/15.
- `a_cancel_that_errs_still_ends_killed`: failed 1/15.
- `the_worker_cancels_and_writes_killed`: failed 1/15.
- `commands::tests::the_glob_worst_case_is_bounded_by_the_step_budget`: failed 1/15.
- New: `a_reaped_run_is_still_lost_not_killed` **hung** in 2/15 runs, killed after 300 s. One
  earlier run hung for 54 minutes. The 20 s `timeout` bounds only the worker future. The `other`
  branch of the `join!` waits on `started` with no bound. So if the worker gives the run up before
  the handler starts, the test may wait forever instead of failing. This reading is not confirmed.
