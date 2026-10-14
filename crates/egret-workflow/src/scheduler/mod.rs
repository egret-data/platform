//! The trigger loop's I/O half (W12): `workflow.triggers`, and the code that
//! reads and writes it.
//!
//! [`trigger`](crate::trigger) stays pure -- its module doc promises that
//! nothing in that tree reaches a database, and its tests rely on it -- so the
//! database lives here, and this module is `trigger`'s only I/O caller. Every
//! "now" in this module comes from Postgres's `now()` in the current
//! transaction, handed to `trigger` as a [`FixedClock`](crate::trigger::FixedClock).
//!
//! See `docs/en/specs/2026-09-24-workflow-w12-trigger-loop-design.md` §3.
//!
//! # Safety with N replicas, per statement
//!
//! `runtime::spawn` starts [`fire_due`] on its own ticker, behind its own gate
//! (`RuntimeConfig::scheduler_enabled`), on **every** replica and with no
//! leader election. `runtime.rs`'s rule applies here unchanged: the
//! `SKIP LOCKED` sentence covers the statement that carries it and nothing
//! else, so each statement a pass issues -- and the one a flow save issues --
//! gets its own statement and its own test. The tests that run tickers are in
//! `tests/runtime.rs`; each has a scratch database of its own, because a pass
//! is global.
//!
//! ## 1. The claim -- safe by **exclusion**
//!
//! `SELECT … FROM workflow.triggers t JOIN workflow.flows f … LIMIT 1
//! FOR UPDATE OF t SKIP LOCKED` (`fire.rs`), one trigger per transaction. Two
//! replicas' passes **partition** the due set: a trigger another pass holds is
//! skipped, not waited on, and the lock is held until that pass's `COMMIT`, so
//! everything below runs with no second fire of this trigger in flight.
//!
//! The `WHERE` also carries every off switch that is not in the YAML --
//! `deleted_at`, `state`, the workspace's `workflows` feature (a `NOT
//! EXISTS` over `workspace_features`, the rows the route gate reads; final
//! review I3 -- since W5a a `COALESCE` of the row's `enabled` with the
//! catalog's `default_enabled`, the same answer for this key), and the workspace's own `status` (an `EXISTS` over
//! `workspaces` requiring `status = 'active'`, the same column and value
//! `WsContext` and the webhook route already refuse a non-active workspace
//! on; the user's decision of 2026-09-25) -- so an off trigger is never
//! claimed and never consumes a tick.
//!
//! **What the lock does NOT give: a fresh flow row.** Under READ COMMITTED a
//! claim that waited on a trigger row re-reads the NEWEST version of `t` but
//! keeps its snapshot's version of `f` (EvalPlanQual re-fetches only the locked
//! table; measured by the final review, I2). A save that committed while the
//! claim waited can therefore leave the pass holding the old `f.source` beside
//! the new row's dates and `spec_key`. `fire_one` compares the claimed
//! `spec_key` with the key of the cron it parsed, and on a mismatch fires
//! nothing, moves nothing and re-runs `sync`, whose own statement reads the
//! committed source. Proved by `tests/scheduler_fire.rs`'s
//! `a_spec_key_that_disagrees_with_the_source_does_not_fire_and_resyncs`,
//! which stages the mismatch by hand rather than the race.
//!
//! Proved by `two_scheduler_tickers_never_fire_one_tick_twice`: 10 flows × 3
//! due ticks under two tickers, asserted on rows (each `(flow, tick)` once,
//! exactly 30) **and** on insert *attempts*, counted by a non-transactional
//! sequence, so the test says which mechanism held. Mutations, W12 Task 5:
//!
//! - **drop `SKIP LOCKED`, keep `FOR UPDATE`: survives, 3 of 3, and the lock
//!   is still what holds** -- attempts == rows. A blocked claim waits, and
//!   READ COMMITTED re-checks the `WHERE` against the row the winner
//!   committed and locks *that* version, so the loser sees the advanced date
//!   and never repeats the tick. What `SKIP LOCKED` buys is throughput -- a
//!   pass never waits behind another pass's fire -- and no test observes it.
//!   (The brief predicted the index would be what holds; the measurement says
//!   otherwise.)
//! - **drop the whole lock: dies, 3 of 3, on the attempts assertion only** --
//!   12, 30 and 30 scheduled inserts were attempted and rolled back, and the
//!   rows were right. With the lock gone, the index is the only thing in the
//!   path that refuses such an insert, but the counter (`attempts - rows`)
//!   counts ANY rolled-back scheduled insert, not only an index refusal; the
//!   run did not record which constraint refused each one. A rows-only test
//!   would have passed.
//!
//! ## 2. The execution's `INSERT` -- the unique index is the **backstop**
//!
//! [`start_execution_in`](crate::worker::start_execution_in) runs in a
//! savepoint inside the claim's transaction. Its `workflow.executions` insert
//! carries `scheduled_date`, and `workflow_executions_one_per_schedule_tick`
//! (migration 0156, `(flow_id, execution_trigger->>'id', scheduled_date)`)
//! makes a second execution for one tick **unrepresentable** rather than
//! merely avoided. A violation of that index rolls back the savepoint and is a
//! consumed tick (`PassReport::skipped`), not an error. Under statement 1 it
//! never fires; it is there for the day statement 1 is wrong.
//!
//! Proved real by `tests/scheduler_fire.rs`'s
//! `the_unique_index_refuses_a_second_fire` (the constraint name, with a
//! one-tick-later control) and
//! `a_tick_that_already_fired_is_consumed_not_taken_off_the_queue` (the
//! classification); and by the lock-free mutation above, where the rows
//! stayed right although 12 to 30 scheduled inserts were rolled back.
//! Dropping the lock **and** the index dies on duplicate rows, 3 of 3.
//!
//! ## 3. The final `UPDATE` -- safe **under the claim's lock**
//!
//! `UPDATE workflow.triggers SET next_evaluation_date, evaluated_at,
//! last_error … WHERE flow_id = $1 AND trigger_id = $2` -- and its two
//! siblings on the other paths, `record_error` (reason only, dates kept) and
//! `take_off_queue` (`next_evaluation_date = NULL`). No lock of their own and
//! none needed: they write the row the claim locked, in the same transaction,
//! keyed by its primary key, so nothing else can have written it since the
//! claim read it. The values come from `tick` over what the claim read, and
//! they commit with the execution or not at all.
//!
//! Proved by `tests/scheduler_fire.rs`'s `fire_and_advance_are_atomic` (a
//! fault in this `UPDATE` rolls the execution back too). The two-replica half
//! is carried by statement 1's test's **attempt counter** (`attempts ==
//! rows`), and by nothing else in it. An advance written from a stale read
//! makes the next pass re-fire a tick it already fired, but the unique index
//! refuses that re-fire and `fire_due` records it as a consumed tick, so the
//! rows, the total and the per-flow instants all stay correct. Under the
//! lock-free mutation above, the per-flow `scheduled_dates` assertion PASSED
//! and only `attempts == rows` died.
//!
//! ## 4. `sync`'s upsert -- **serialised by the row lock**; it waits
//!
//! `INSERT INTO workflow.triggers … ON CONFLICT (flow_id, trigger_id) DO
//! UPDATE` and the `DELETE` of undeclared rows before it ([`sync`]), run in
//! the transaction of `FlowRepo::create`, `update` and `soft_delete` --
//! `set_live` is exempt, since it changes neither `source` nor `deleted_at` --
//! and in one transaction per flow when [`reconcile`] runs at ticker start,
//! after reading the flow `FOR SHARE`. They take no `SKIP LOCKED`: a
//! save that meets a trigger a pass has claimed **waits** for that pass to
//! commit, then updates the version the pass committed -- `ON CONFLICT DO
//! UPDATE` and `DELETE` both act on the latest row version -- so an unchanged
//! `spec_key` keeps the *advanced* dates rather than rewinding them. The
//! other order is the claim's business: a pass that meets a row a save holds
//! skips it and finds it on a later pass. **That direction is unproved**: it
//! is the `SKIP LOCKED` property statement 1 records as unobserved, and no
//! test holds a save open while a pass runs. The pass's own call, `resync` (a
//! source that disagrees with the row), runs inside the claim's transaction
//! and already holds the claimed trigger's lock. `sync` also reads
//! `workflow.disabled_triggers` (the runtime switch, W7e) and deletes its
//! undeclared rows, in the same transaction.
//!
//! **`resync` is serialised on the flow row, without waiting.** Every other
//! caller holds the flow row while `sync` runs (a save `FOR NO KEY UPDATE` or
//! `FOR UPDATE`, `reconcile` `FOR SHARE`), and so does the W7e switch route
//! while it writes a switch row and a trigger's copy. `resync` reads the flow
//! `FOR SHARE SKIP LOCKED`: when a writer holds it, `resync` does nothing --
//! the claimed trigger stays frozen, the writer's own write puts the copy
//! right, and a later pass resyncs a row that still disagrees. Without the
//! lock, `sync` read the switch rows before the route committed, waited on the
//! trigger row the route held, and then wrote the stale `disabled` over it (a
//! lost update; W7e review I1). A waiting `FOR SHARE` deadlocks when the route
//! switches the claimed trigger itself (route holds the flow, wants the
//! trigger; the pass holds the trigger, wants the flow), and `NOWAIT` would
//! abort the pass's transaction. Proved by `tests/disabled_triggers.rs`'s
//! `a_resync_does_not_undo_a_concurrent_switch_on_another_trigger` and
//! `a_resync_of_the_trigger_being_switched_does_not_deadlock`.
//!
//! Proved by `tests/runtime.rs`'s
//! `a_save_that_meets_a_claimed_trigger_waits_and_keeps_the_advance`, which
//! holds a real pass at its final `UPDATE`, observes the save **blocked on a
//! lock** in `pg_stat_activity`, and asserts the advance survives the save.
//!
//! **A known deadlock, left in place (final review M2).** An `update` that
//! renames the flow changes `namespace` or `flow_id`, columns of the unique
//! key `(workspace_id, namespace, flow_id)`, so its `UPDATE workflow.flows`
//! takes `FOR UPDATE` on the flow row -- not the `FOR NO KEY UPDATE` an edit
//! of the source takes -- and then its `sync` waits on the trigger row a pass
//! holds. That pass's execution `INSERT` needs `KEY SHARE` on the same flow row
//! for its foreign key, which `FOR UPDATE` blocks. Postgres detects the cycle
//! and aborts one side: the pass (its transaction rolls back and the next pass
//! retries the tick) or the save (a 500 the author can retry). Nothing is lost
//! or written twice, which is why it is recorded here and not engineered away.
//! A save that does not rename takes `FOR NO KEY UPDATE`, which `KEY SHARE`
//! does not conflict with. Unproved: no test stages it.

mod fire;
/// Shared with `flow_trigger` (W13a Task 4): the off switches' SQL and the
/// classification of a failed start.
pub(crate) mod start;
mod sync;

pub use fire::{fire_due, PassReport};
// The switch route (W7e Task 5) writes the same effective value `sync` does;
// until it lands nothing outside `sync` reads it.
#[allow(unused_imports)]
pub(crate) use sync::effective_disabled;
pub use sync::{reconcile, sync, Reconciled};
