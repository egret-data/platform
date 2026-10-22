# CI flakes, batch 2 (2026-10-23)

Follows batch 1 (`2026-10-22-ci-flaky-tests.md`, #730). Same rule: a test's verdict must depend on
the behaviour it names, not on timing or on what other tests do at the same moment. No retries, no
longer sleeps, no `#[ignore]`.

## 1. `stale_rows_are_pruned_on_upsert_and_fresh_ones_kept` (egret-policy)

- **Cause.** `record_plugin_download` prunes on every 64th upsert, counted by a process-global
  `static PRUNE_TICK`. The test's 64 upserts in a row hit a multiple of 64 only if no other test in
  the binary upserts at the same time. When one does, the 64th tick is taken by that test and the
  prune runs for ITS service, and the `old` row of this test's service stays (CI: `left: [fresh, old,
  old-elsewhere]`, twice in the last 195 failed runs, once on `main`).
- **Fix (production code).** The counter moves into `PolicyStore` (`download_ticks`, an
  `Arc<AtomicU64>` shared by clones). Behaviour is unchanged in production: the one store behind the
  `/ranger` surface (`egret-bin`, `RangerState`) is the only caller of `record_plugin_download`, so
  every poll of the process still counts on one counter, and the first upsert still prunes. Not a
  production bug: which service a tick pruned for never mattered beyond "eventually".

## 2. `delete_resumes_the_regular_schedule_where_it_stood` (egret-workflow)

- **Cause.** Trigger `s` gets its date from the save that seeds the flow: the next minute after the
  SEED. `make_due` then waits out a minute's last ten seconds. A seed in seconds 50-59 has that wait
  cross the minute, so `prev` is already past and `assert!(prev > fx.now())` fails (3 times on main on
  2026-10-21).
- **Fix.** `s`'s date is set after `make_due` (`date_trunc('minute', now()) + 1 minute`), as the
  test already does for `bug`. What the test checks, that delete resumes from the stored position,
  is unchanged.

## 3. `failed_provision_rolls_back_row_client_and_cluster` (egret-engine-mgmt)

- **Cause.** The final check counted every orphaned `catalog-client-*` credential in the process
  database. Other tests in the binary retire clients concurrently (client first, its secret just
  after), so a credential mid-retire elsewhere shows up as an orphan (CI: one extra
  `catalog-client-6e25…`).
- **Fix.** The check names this test's two clients. A rollback deletes the client row, so a trigger
  on `catalog.catalog_clients` (created by this test in its process database) records each client as
  it is minted; the test asserts exactly one client per create and no secret left for it. Stronger
  than before: a create that minted nothing would have passed the old check.

## 4. Test-template migration race (`egret-test-db`): not a race

The one CI occurrence (run 39560895684, branch `fix/f2-storage-browse-secrets`) is a branch with two
migrations numbered 0200 (`0200_engine_ranger_credential.sql` from main and
`0200_secret_system_managed.sql`). The first test to migrate the template gets
`duplicate key … _sqlx_migrations_pkey`, the next ones `migration 200 was previously applied but has
been modified`. The panic text already names this cause, and
`egret-metastore::no_two_migration_files_share_a_version` guards it. Nothing to fix.

## 5-7 and others, seen under local load only: documented, not changed

None of these failed in the 195 failed CI runs scanned. Load here is 8-12 busy-loop CPU hogs on 8
cores plus the `worker::` lib suite at `--test-threads 64`.

- `worker::claim::tests::a_budget_that_expired_on_a_killed_run_cancels_once`: `FAILED` vs
  `KILLED`, 2 of 44 suite runs. The handler plants the kill and then sleeps; the task's budget is
  0.3 s from the handler's start. A plant slower than 0.3 s is cut off by the budget (its
  transaction is dropped), so the budget's `FAILED` commits onto a `RUNNING` run (a scratch 400 ms
  delay before the plant gives the same `FAILED` every run). The kill can only
  land between the immediate first lease renewal and the budget's expiry, so the only test-side fix
  is a longer budget, which is a longer wait; left as is.
- `worker::claim::tests::a_budget_cancel_in_flight_when_the_kill_lands_is_asked_again`: not in the
  batch-1 list. 2 of the first 5 suite runs ended in the test's own 20 s timeout ("the kill poll
  ends the hung cancel: Elapsed"), then 0 of 31 with the kill poll instrumented. Under that load a
  kill-check read takes up to the 100 ms `kill_poll` that bounds it (3 reads in 14 runs timed out);
  a read that does not answer within one period is "not killed this tick". Mechanism not confirmed.
- `worker::claim::tests::a_killed_working_directory_runs_a_discard_only_finish_and_ends_killed`:
  1 of 39 suite runs, at its first check (`the prepare wrote under the block's prefix`: no object
  after the prepare). Cause not found.
- `tests/worker.rs::a_retry_whose_delay_has_elapsed_is_woken_and_one_still_waiting_is_left_alone`:
  0 of 30 alone under load; not reproduced.
- Also seen under that load: `the_worker_cancels_and_writes_killed` ("the kill took 2.4 s") and
  `a_cache_is_restored_and_saved_again_only_when_a_matched_file_changed` (a lease lost at commit),
  once each. Runs that exhausted the shared Postgres's 100 connections are not counted.
