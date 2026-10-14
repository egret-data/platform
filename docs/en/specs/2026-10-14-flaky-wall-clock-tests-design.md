# Flaky wall-clock tests: surviving a loaded machine

**Status:** design, 2026-10-14. Line numbers are those of `main` at `8e34db0a`.

## 1. Problem

Several `egret-workflow` tests compare elapsed wall-clock time with a fixed bound, or race a real sleep against a fixed deadline. Each passes alone. Each has failed while other sessions' `cargo` ran on the same machine (load average 23–29 on 8 cores):

- `storage::kv::merge::tests::many_keys_near_the_depth_limit_merge_fast` asserts `took < 2 s` (`crates/egret-workflow/src/storage/kv/merge.rs:914-917`). It failed at 2.84 s ("1638 keys of 100 segments took 2.839154778s"), against about 1.1 s alone. Other `merge` timing tests failed at 3.5 s and 5.5 s.
- `tests/worker.rs::a_run_whose_worker_stopped_renewing_is_reaped` (`crates/egret-workflow/tests/worker.rs:9397`) failed once under load and passed 3 runs out of 3 alone.

A test that fails because the machine is slow trains people to re-run red tests. This package makes these tests depend on the code under test, not on the machine.

## 2. What "fixed" means

A fixed test still detects the regression it was written for, and does not fail because the machine is slow. The approaches, in order of preference:

- (a) measure work, not time;
- (b) use a virtual clock where the code already takes one;
- (c) use a bound relative to a baseline timed in the same run;
- (d) as a last resort, use a much larger absolute bound.

Each fixed test is shown failing against a deliberately regressed implementation, and passing 5 times out of 5 while the CPU is saturated.

## 3. The tests in this package

### 3.1 `merge.rs`: three complexity tests

The code, verified:

- `refuse_conflicts` walks a trie, with one `child` lookup per segment (`merge.rs:222-258`).
- `flatten_to_nested` takes one arena step per segment (`merge.rs:344-390`).
- The depth limit is checked in `merge_entries`'s entry loop, before either of them runs (`merge.rs:173-178`).

| Test | Bound today | Regression it guards | New measure |
|---|---|---|---|
| `a_dotted_key_deeper_than_max_depth_is_refused_at_once` (`merge.rs:847`) | `< 250 ms` | the refusal comes after something is built | (a) work: a test-only per-thread counter of path steps (one per `child` lookup in the trie, one per arena step) must read **0** after a refused merge |
| `many_keys_near_the_depth_limit_merge_fast` (`merge.rs:892`) | `< 2 s` | a merge quadratic in a path's depth (C1: every prefix hashed as a vector) | (a) at most 2 steps per segment, **and** (c) the two deep patches take at most 6 times a baseline: the same number of segments as one-segment keys, timed beside them |
| `a_large_merge_is_linear` (`merge.rs:943`) | `< 5 s` | a merge quadratic in a map's width or a list's length | (c) the three merges at size `n` take at most 30 times the same merges at `n / 10`. Linear, that ratio is about 10; quadratic, about 100 |

The counter is `#[cfg(test)]`. In other builds it compiles to an empty inline function.

A relative bound is measured with the `least_times` helper:

- It takes the least of 3 runs of the baseline and the least of the big case's runs.
- It runs up to 5 rounds and stops at the first round within the bound.
- Load that stretches both measures cancels out. A burst of load in one round is outlasted by the next. Only a cost that grows faster than the baseline's fails every round.

The counter alone would not suffice for the complexity tests. It sees only the loops it is placed in, and a rewrite (the C1 defect was a rewrite) can bypass it. The relative time sees any code.

### 3.2 `a_run_whose_worker_stopped_renewing_is_reaped`

The code, verified:

- `reap` reclaims a lease older than `timeout` by the database's clock: `claimed_at < now() - make_interval(secs => $1)` (`worker/reaper.rs:39-53`).
- The worker renews every `working_timeout / 3` (`worker/claim.rs:1147-1149`).
- The worker self-fences when no renewal has succeeded for `working_timeout` (`worker/claim.rs:1616-1618`, `under_lease_with`).
- A renewal in flight when the worker's future is dropped can still commit on the server (`claim.rs:1493-1495`).

The test today uses a 900 ms lease and fixed sleeps:

- It sleeps 1.5 s, then expects `reap(900 ms) == 0`. A renewal later than about 600 ms makes the run reapable, or the worker self-fences.
- After the abort, it reads the lease at once, sleeps 1.2 s, and expects the same lease. A renewal still in flight at the abort can land in between.

A virtual clock (b) does not apply here, because the lease and the reaper read Postgres' `now()`. The new test waits on conditions instead:

- **Live pass.** Poll the lease every 50 ms until a renewal lands more than `working_timeout` after the claim, then reap immediately: zero reclaimed. The deadline is 60 s, so a worker that never renews fails with a message. `working_timeout` becomes 2 s, so renewals happen every 667 ms and the slack is 1.33 s. That remaining dependence on time cannot be removed: a live worker that cannot renew within `working_timeout` is, correctly, reapable.
- **Dead pass.** Abort the worker, then wait one renewal period for an in-flight renewal to land. Read the lease, and wait until **the database clock** is past it by `working_timeout` (`until_db_clock_passes`). The lease must be unchanged, and the reap reclaims one.

It still catches:

- a worker that never renews (the live pass times out);
- a reaper that reclaims everything (the live reap returns 1);
- a reaper that reclaims nothing (the dead reap returns 0);
- a worker that renews after its abort (the lease moves).

## 4. Other tests found

Other tests in `egret-workflow` and `egret-server` that time or race the wall clock are listed in the report, each with a judgement. They are not changed in this package. The highest-risk ones:

- `tests/runtime.rs:1386` `a_save_that_meets_a_claimed_trigger_waits_and_keeps_the_advance`: several Postgres polls must fit inside one `pg_sleep(1)`.
- `tests/task_logs.rs:973` `an_execution_deleted_batch_is_rerun_once_not_retried`: `< 400 ms` over Postgres round trips.
- `tests/task_logs.rs:821` `lines_reach_postgres_in_order_within_a_flush`: "not yet flushed" must hold before a 300 ms timer fires.
- Lease margins of 1.3 s or less:
  - `tests/runtime.rs:2025` `a_run_longer_than_working_timeout_is_not_reaped_while_its_worker_lives`;
  - `worker/claim.rs:4089` `succeeding_renewals_keep_the_run_past_the_deadline`;
  - `tests/prefetch_hold.rs:1752` `the_lease_is_renewed_while_a_long_fetch_downloads`.
- Tests that need one 2 ms ticker tick per 10 ms:
  - `tests/kv_store.rs:1408` `a_large_decode_runs_off_the_async_runtime_and_a_ticker_keeps_ticking`;
  - `storage/nsfiles/stage.rs:1148` `matching_runs_off_the_async_runtime_and_a_ticker_keeps_ticking`.

## 5. Proof required

For each fixed test, the report records:

- the mutation and the failing output;
- a normal pass;
- 5 passes under saturation: 24 `yes > /dev/null` processes on 8 cores, started and killed by this package.
