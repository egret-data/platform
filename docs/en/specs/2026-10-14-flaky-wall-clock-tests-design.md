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

Each fixed test (§3.1–§3.3) is shown failing against a deliberately regressed implementation, and passing 5 times out of 5 while the CPU is saturated.

## 3. The tests in this package

### 3.1 `merge.rs`: three complexity tests

The code, verified:

- `refuse_conflicts` walks a trie, with one `child` lookup per segment (`merge.rs:222-258`).
- `flatten_to_nested` takes one arena step per segment (`merge.rs:344-390`).
- The depth limit is checked in `merge_entries`'s entry loop, before either of them runs (`merge.rs:173-178`).

| Test | Bound today | Regression it guards | New measure |
|---|---|---|---|
| `a_dotted_key_deeper_than_max_depth_is_refused_at_once` (`merge.rs:847`) | `< 250 ms` | the refusal comes after something is built | (a) work: a test-only per-thread counter of path steps (one per `child` lookup in the trie, one per arena step) must read **0** after a refused merge |
| `many_keys_near_the_depth_limit_merge_fast` (`merge.rs:892`) | `< 2 s` | a merge quadratic in a path's depth (C1: every prefix hashed as a vector) | (a) exactly 2 steps per segment (bound: 3 times that), against an extra walk, **and** (c), against C1's dearer step, the two deep patches take at most 1.5 times a baseline: the same number of segments as one-segment keys, timed beside them |
| `a_large_merge_is_linear` (`merge.rs:943`) | `< 5 s` | a merge quadratic in a map's width or a list's length | (c) the three merges at size `n` take at most 30 times the same merges at `n / 10`. Linear, that ratio is about 10; quadratic, about 100 |

The counter is `#[cfg(test)]`. In other builds it compiles to an empty inline function.

A relative bound is measured with the `least_times` helper:

- It takes the least of 3 runs of the baseline and the least of 2 runs of the big case.
- It runs up to 5 rounds and stops at the first round within the bound. It also stops early when the big case is over a give-up ratio (60 for `a_large_merge_is_linear`), so a regressed merge fails after one slow round, not five.
- Load that stretches both measures cancels out. A burst of load in one round is outlasted by the next. Only a cost that grows faster than the baseline's fails every round.

Measured on 2026-10-14: the deep patches cost 0.5–0.7 times the baseline, loaded or not. With the C1 defect put back (every prefix hashed as a vector, in the check and in the nesting), they cost 2.1–2.3 times; with half of it, 1.6–1.8 times. Because the depth is capped at `MAX_DEPTH`, being quadratic in it costs a bounded factor, so this window is narrow. The absolute 2 s bound had a narrower one: 1.1 s alone and 2.8 s under load. `a_large_merge_is_linear` measured 10–15 times linear, and 108 times with a scanning `deep_merge`.

The two measures guard different regressions. The step count catches an **extra walk**: the `trie-rewalk` mutation took 16.5 million steps against a bound of about 1 million. It **cannot see C1**, because C1 kept the step count and made each step dearer: the `c1` and `prefix-vectors` mutations took the same 655 200 steps as the linear merge. The relative time is the **only** guard against C1. Its margin against half of C1 is about 1.1 times, so that half can now and then pass: a missed regression, never a red on correct code.

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
- **Dead pass.** Abort the worker, then poll `pg_stat_activity` until no other backend of the test's database is `active` (60 s failure deadline), so an in-flight renewal has landed. Read the lease, and wait until **the database clock** is past it by `working_timeout` (`until_db_clock_passes`). The lease must be unchanged, and the reap reclaims one.

It still catches:

- a worker that never renews (the live pass times out);
- a reaper that reclaims everything (the live reap returns 1);
- a reaper that reclaims nothing (the dead reap returns 0);
- a worker that renews after its abort (the lease moves).

### 3.3 Three more, from the audit (HIGH)

| Test | Race today | New measure | Still catches |
|---|---|---|---|
| `tests/runtime.rs:1398` `a_save_that_meets_a_claimed_trigger_waits_and_keeps_the_advance` | The hold is a `pg_sleep(1)` in a `BEFORE UPDATE` trigger. Two `pg_stat_activity` polls and a spawned save must fit inside that second. | **No time at all.** The trigger waits for `pg_advisory_xact_lock_shared(1386)`, while the test holds `pg_advisory_lock(1386)` on a connection of its own. The pass is seen waiting (`wait_event = 'advisory'`), then the save is seen waiting (`transactionid` / `tuple`). Only then does the test release the pass. | A save that rewinds the advance: it reads the date without the row lock and writes it back. |
| `tests/task_logs.rs:821` `lines_reach_postgres_in_order_within_a_flush` | `count == 0` must be read before a 300 ms flush timer fires. | (a) The writer runs over the counting `Rigged` inserter: exactly **one** call stores the three lines. Plus a **lower** bound, which load can only make truer: that call begins at least `flush_every` after the first write. | A flush per line (3 calls); a flush that does not wait for its timer (it begins about 2 ms after the write). |
| `tests/task_logs.rs:973` `an_execution_deleted_batch_is_rerun_once_not_retried` | `< 400 ms` over Postgres round trips, then `sleep(500)` before `calls == 4`. | (a) "At once" is measured as "no other task ran between the 23503 and the re-run". Each `Rigged` call spawns a probe task that sets a flag. On the test's single-threaded runtime the probe can only run if the writer yields, and a backoff `sleep` yields. The 4th call is waited for as a condition. "Dropped" is shown by the next batch being stored by the 5th call, and the dropped lines never being stored. | A backoff before the re-run; a second 23503 retried like an ordinary failure. |

The probe in the third test needs a current-thread runtime, so both `task_logs` tests set `#[tokio::test(flavor = "current_thread")]` explicitly. On a multi-threaded runtime the probe could run beside the writer and the test would fail on correct code (a false red); it would not silently weaken.

The three tests were rated HIGH by analysis. `main`'s versions passed 5 out of 5 under the same load, so no failure was reproduced.

## 4. Not fixed in this package

These timing-sensitive tests were judged MEDIUM-HIGH. They are left as known follow-ups.

- `tests/runtime.rs:2025` `a_run_longer_than_working_timeout_is_not_reaped_while_its_worker_lives`: a 2 s `working_timeout`, renewed every 667 ms, against a reaper ticking every 100 ms. One renewal stalled for more than about 1.3 s gets the run reaped and run twice.
- `src/worker/claim.rs:4101` `succeeding_renewals_keep_the_run_past_the_deadline`: runs on the real clock with `EVERY` 60 ms and `FENCE` 200 ms. A starved runtime self-fences. A paused clock would fit, because `under_lease_with` already takes an injectable renewal and uses the tokio clock.
- `tests/prefetch_hold.rs:1752` `the_lease_is_renewed_while_a_long_fetch_downloads`: a 1.5 s lease and `sleep(3s)`, then `reap == 0`. This is the pattern §3.2 replaced; `until_db_clock_passes` can be reused.
- `tests/kv_store.rs:1408` `a_large_decode_runs_off_the_async_runtime_and_a_ticker_keeps_ticking`: a 2 ms ticker must tick once per 10 ms of decode. At high load, OS wake-up latency alone exceeds that.
- `src/storage/nsfiles/stage.rs:1148` `matching_runs_off_the_async_runtime_and_a_ticker_keeps_ticking`: the same ticker floor, plus a check on a 1-connection pool.
- `tests/container_docker_live.rs` 795, 986 and 1853 (live tests, opt-in): each drops `run()` after 1.5–2.5 s and asserts the container is running, so a slow `docker run` has not started it yet.

Judged MEDIUM, also left as follow-ups:

- `tests/flow_trigger_fire.rs:1476` `a_pass_stops_at_its_time_cap_and_the_next_pass_goes_on`: the 1 s pass cap against `pg_sleep(0.6)` per fire.
- `tests/runtime.rs:567` `the_worker_ticks_on_a_fixed_interval_rather_than_draining_to_zero`: `sleep(600)`, then `ran == 1`.
- `tests/worker.rs:7000` `a_max_duration_chain_stops_when_the_original_clock_runs_out`: the first attempt must land within 1 s of a PT2S budget.
- `tests/worker.rs:211` `two_concurrent_claims_partition_the_runnable_set`: `timeout(500ms, claim)`.
- `tests/kv_sweep.rs:379` `the_spawned_sweeper_sweeps_after_its_interval_and_stops_on_cancel`: "not swept" at 500 ms against a 1 s interval.
- `tests/nsfiles_api.rs:1894` `an_export_whose_client_goes_away_stops_opening_files`: a 300 ms abort-propagation snapshot.
- `src/worker/commands.rs:4387` `a_dropped_run_future_leaves_no_staging_directory_behind`: two process spawns within 1.5 s.
- `src/worker/k8s/runner.rs:2416` `a_slow_but_progressing_upload_is_not_timed_out_and_a_stalled_one_is`: an unbiased `select!` with 200 ms of slack.
- `tests/task_logs.rs:1853` `a_refused_line_costs_only_itself_not_its_batch`: a barrier `< 2 s` over Postgres round trips.

**On §3.1's relative check.** In `many_keys_near_the_depth_limit_merge_fast` its margin is narrow: bound 1.5, linear 0.53–0.69, half of C1 1.64. It is nonetheless the only guard against C1's dearer step; the step counter guards only an extra walk. A narrow margin can give a missed regression, never a red on correct code.

## 5. Proof required

For each fixed test, the report records:

- the mutation and the failing output;
- a normal pass;
- 5 passes under saturation: 24 `yes > /dev/null` processes on 8 cores, started and killed by this package.
