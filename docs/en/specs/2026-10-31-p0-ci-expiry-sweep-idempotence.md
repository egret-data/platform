# P0-ci: `the_sweep_is_idempotent` is flaky on main (#795 follow-up)

## Symptom

`crates/egret-perm-compile/tests/expiry_sweep.rs::the_sweep_is_idempotent`
(added by #796) fails intermittently on `main` and on PRs:
`an unchanged profile writes nothing on a repeat sweep; left: 1, right: 0`.

## Root cause: test isolation, not the compiler

- `egret-test-db` gives each test **process** one private database, so the
  two tests in `expiry_sweep.rs` share one database and run concurrently.
- `recompile_lapsed_grants` recompiles every profile with a lapse in its
  two-hour window, across the whole database. `SweepSummary::rewritten` was a
  **count** over all of those profiles.
- When `a_lapsed_role_grant_is_swept_out_of_its_profile` lapses its holder
  between the idempotence test's first and second sweep, the second sweep
  rewrites **that** test's profile, and the count is 1.

Evidence (local, `DATABASE_URL=postgres://egret:egret@localhost:5432/egret`):

- On main: 1/30 sequential runs of the binary failed with the CI message;
  0/15 runs of the idempotence test alone failed.
- A diagnostic build that reports service ids: 7/200 runs (4 parallel loops)
  failed, every one `own=1 services=[1, 2] rewritten=[2]`. The test's own
  profile was never rewritten, so a non-deterministic compile (hypothesis b)
  is not what fails.

## Change

1. `SweepSummary::rewritten` becomes `Vec<i64>`: the ascending ids of the
   profiles whose output changed. A count cannot tell one profile's
   convergence from another profile's first rewrite. The `egret-bin` log line
   keeps logging the count.
2. `the_sweep_is_idempotent` asserts on its own profile only, and makes the
   CI interleaving deterministic: a neighbour profile's holder lapses between
   the two sweeps. It also asserts the first sweep rewrites the profile and
   the second rewrites the neighbour, so the test checks more than before.

## Red / green

- Red: the new test with the old count assertion
  (`second.rewritten.len() == 0`) fails 5/5 with `left: 1`.
- Green: 30/30 runs of both tests, 15/15 of the idempotence test alone,
  200/200 under four parallel loops, and the full `egret-perm-compile` suite.
