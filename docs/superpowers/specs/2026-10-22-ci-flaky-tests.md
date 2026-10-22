# CI flakes: three tests made deterministic (2026-10-22)

Three tests failed in CI and passed on rerun. Each one's verdict depended on something other than
the behaviour it names. No production code changes: none of the three races is in shipped code.

## 1. `worker::claim::tests::a_discard_only_block_claim_survives_renewal_and_the_poll`

- **Cause.** The test ran 400 ms of work under `working_timeout = 150 ms` and `kill_poll = 20 ms`.
  Two latencies decided the verdict. (a) A lease renewal slower than 150 ms makes `under_lease`
  self-fence and return `Lost` ("a discard-only claim lost its lease"). (b) For the leaf contrast,
  the refused renewal is followed by one read that tells a kill from a lost lease, bounded by
  `kill_poll`: a read slower than 20 ms is `Lost`, not `Killed` ("the contrast: under a leaf's
  watch the same row is a kill"). This is the CI failure, 12 times in the last 150 failed runs.
- **Fix.** `working_timeout = 3 s` and `kill_poll = 3 s`. The block's work ends on an event, two
  renewals by the ticker seen in the row, instead of after a 400 ms sleep. The leaf contrast is
  decided by the row's state on the first renewal; the poll period plays no part.
- **Production.** The defaults are 1 h and 5 s, and a kill read that does not answer in time is
  treated as a lost lease by design (the reaper then hands the `KILLING` run to `finish_claimed_kill`).
  Not a bug.

## 2. `worker::commands::tests::a_store_failure_mid_copy_kills_docker_cp_before_its_stdin_closes`

- **Cause.** The store failed 500 ms after `copy_in` started, which races with the `docker cp`
  shim starting. A spawn slower than 500 ms gets `sh` killed before it writes its pid, and the test
  panics reading it ("the shim started: No such file or directory").
- **Fix.** The store fails once the shim has written its pid (written to a side file and renamed,
  so the file is whole when it appears). The failure now always lands on a running `docker cp`,
  which is what the test is about.

## 3. `declassifying_removes_the_derived_view_tags_and_keeps_manual_ones` (egret-bi)

- **Cause.** Every test in `view_retag_on_classification` seeds the same
  `policy.authz_service_defs` row at once with `ON CONFLICT (id) DO NOTHING`. Only the primary key
  is the arbiter, so a racer that misses the other's uncommitted `id` still collides on the `name`
  UNIQUE index and gets `23505 authz_service_defs_name_key`. Any of the five tests can lose (CI
  shows three of them).
- **Fix.** `ON CONFLICT DO NOTHING` with no target, so every unique index is an arbiter. The same
  seed in `catalog_tag_version_bump.rs` (two tests) gets the same change.
- **Production.** `PolicyStore::upsert_service_def` has the same shape (`ON CONFLICT (id) DO
  UPDATE`) and already retries once on a unique violation, with a comment naming this race. No
  change.
