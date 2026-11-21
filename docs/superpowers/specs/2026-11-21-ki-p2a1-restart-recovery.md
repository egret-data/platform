# KI-237: a graceful restart hands its runs back

## Row

Restarting egret while a long task runs (`Sleep`) leaves the run `RUNNING`
under a fresh lease until the reaper takes it, `working_timeout` (1 h) later.
The claim loop waits for every in-flight run after the stop signal, `main`
gives up after 10 s (Docker's grace), the process exits, and nothing releases
the leases.

## Decision: release what this process holds, at shutdown

Two mechanisms were weighed.

- **`claimed_by` node id + startup reclaim.** Needs a migration and a change to
  the claim and renew statements, and its safety rests on a restarted process
  keeping its identity. A Deployment's pods get new names, so the id would have
  to be invented; reclaiming rows of a guessed "previous incarnation" is the
  exact way to hand a *live* worker's run to a second handler.
- **Graceful-shutdown release (chosen).** The process tracks the runs it holds
  and the latest `claimed_at` it wrote for each (the claim's, then every
  renewal's). On stop the claim loop gives the runs in flight
  `shutdown_drain` (5 s) to commit, aborts what is left (as a lost lease
  already aborts a handler: no cancel, the container is re-attached by the next
  claim) and releases the held leases in one `UPDATE ... FROM unnest(ids, at)`.

Multi-replica safety: the release is fenced on the exact `claimed_at` this
process wrote. A run this process lost (reaped and re-claimed elsewhere) has a
newer `claimed_at`, so the fence matches nothing; a lease another worker holds
was never in this process's book. Nothing is released by identity guess.

Out of scope and unchanged: the reaper, its timeout and the hard-crash path
(SIGKILL still waits for the reaper).

## Mechanics

- `worker::HeldLeases` (new `worker/release.rs`): the book. `start_claimed`
  records each claim and forgets it when the run's task ends. `renew_lease`
  records the moved lease through a task-local, so no signature between the
  claim loop and the renewal changes.
- `runtime::RuntimeConfig::shutdown_drain` (default `DEFAULT_SHUTDOWN_DRAIN`,
  5 s, inside `egret-bin`'s 10 s `WORKFLOW_STOP_WAIT`).
- The claim loop's post-cancel wait is bounded by it.

## Tests

- `tests/runtime.rs::a_graceful_stop_releases_the_lease_it_held_and_only_that_one`:
  a stuck handler, a cancel, the lease cleared and the other worker's lease
  untouched.
- `tests/worker.rs`: the release fence both ways (held lease cleared; a lease
  that moved on is left alone; a renewal moves what is released).
