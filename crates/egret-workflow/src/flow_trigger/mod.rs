//! The Flow trigger's I/O half (W13a): the processor that drains
//! `workflow.flow_trigger_events` and fires the window-free listeners that
//! hear each event.
//!
//! [`trigger::flow`](crate::trigger::flow) holds the predicate and stays pure;
//! this module is its only production caller. `settle` writes the outbox
//! (`worker::claim`, spec §4.3) and this module reads it, from the W12
//! scheduler ticker, after `fire_due`, behind the same `scheduler_enabled`
//! switch (`runtime::spawn_scheduler`).
//!
//! See `docs/en/specs/2026-09-25-workflow-w13-flow-triggers-design.md` §4.4,
//! §4.5 and §6.
//!
//! # Safety with N replicas, per statement
//!
//! `scheduler`'s rule applies unchanged: the `SKIP LOCKED` sentence covers the
//! statement that carries it and nothing else, so every statement an event's
//! life issues gets its own entry and its own test. Tests that run tickers are
//! in `tests/runtime.rs` (a scratch database each, because a pass is global);
//! the rest are in `tests/flow_trigger_fire.rs`, serialised by a mutex.
//!
//! ## The transaction runs at `REPEATABLE READ`
//!
//! [`execution_render_context`](crate::worker::execution_render_context)
//! issues several statements (the execution, its surface, its task runs), and
//! the processor reads the source row and its last task run beside them.
//! Under READ COMMITTED a source commit landing between two of them would hand
//! the listener a context no single moment ever had. At `REPEATABLE READ`
//! every read shares the snapshot the claim took.
//!
//! What that could cost is a **serialization failure on a write**, so each
//! write was checked, and the three kinds measured on Postgres 16 (a scratch
//! database on this branch's server, two sessions):
//!
//! - **the event row** (`UPDATE`/`DELETE`, statement 5): locked by our own
//!   claim, and nothing writes an event but a processor holding that lock. No
//!   conflict is possible after the claim; the one before it is statement 2's.
//! - **the new execution's rows** and everything its first `settle` writes:
//!   `INSERT`s of rows no other transaction can see, plus `UPDATE`s of those
//!   same rows. A `Subflow` child it spawns reports to *its* parent -- the row
//!   this transaction just created -- and a flow-fired execution reports to
//!   nobody (`trigger_run_ix` is NULL).
//! - **the foreign-key checks those inserts make**, `FOR KEY SHARE` on rows
//!   that DO exist before the snapshot: the source execution
//!   (`trigger_execution_id`), the listener flow (`flow_id`), the workspace.
//!   **Measured:** a `FOR KEY SHARE` check against a parent row that another
//!   session **non-key-updated** after our snapshot succeeds at `REPEATABLE
//!   READ`; against a parent whose **key** column was updated it fails with
//!   `40001 could not serialize access due to concurrent update`, and rolling
//!   back to the savepoint leaves the transaction usable. A source execution
//!   moving on (`state`, `history`, `seq`, `updated_at`) is a non-key update
//!   **only because `claim::load_with` locks it `FOR NO KEY UPDATE`** (final
//!   review I2). The first version of this paragraph stopped at "non-key
//!   update" and was false: every `settle` took the row `FOR UPDATE` before
//!   updating it, and a `KEY SHARE` check against a row changed after the
//!   snapshot by a `FOR UPDATE` holder fails `40001` exactly as a key change
//!   does -- the reviewer measured it (PG16), and
//!   `a_source_settled_between_the_claim_and_the_fire_still_fires` reproduces
//!   it through the worker's own `resume`: under `FOR UPDATE` the fire is
//!   `errored`, under `FOR NO KEY UPDATE` it fires. So a busy source no longer
//!   costs its listeners their fires. `load_with`'s doc carries the check
//!   that the weaker lock keeps every serialisation the worker relies on. A
//!   flow save that edits the listener is a non-key update too (an `UPDATE`
//!   that changes no key column takes `FOR NO KEY UPDATE`, as `scheduler`'s
//!   module doc says); a save that **renames** the flow
//!   changes `(workspace_id, namespace, flow_id)`, a unique key, and would
//!   fail that listener's insert. `classify` answers `Failed` for it, the
//!   savepoint rolls back, and the listener is retried next pass -- the same
//!   outcome the rename already has under W12's deadlock note (M2), and one
//!   more reason nothing is lost.
//!
//! So `REPEATABLE READ` was kept, with the claim's own failure handled below.
//!
//! **A failed event transaction does not end the pass** (fix round 1, M2). A
//! savepoint, the event's last write or the `COMMIT` failing rolls back that
//! event's fires and leaves its row as it was; the pass logs it at `warn`,
//! counts it `errored`, moves its cursor past it to newer events, and
//! a later pass claims it again. **Its attempt is counted apart** (final
//! review F1): the write that counts it inside the transaction rolled back
//! too, so the failure path issues its own autocommit `UPDATE … SET attempts =
//! attempts + 1, next_attempt_at = …` on the row (`FOR UPDATE SKIP LOCKED`, so
//! it never waits on a replica that has claimed the event since), and at
//! [`MAX_ATTEMPTS`](process::MAX_ATTEMPTS) deletes it with the same "gave up"
//! `warn`. Before that, an event whose transaction ALWAYS fails was retried
//! every pass for ever. Proved by `fire_and_delete_are_atomic`, whose second
//! event fires in the same pass, and by
//! `an_event_whose_transaction_always_fails_is_given_up`.
//!
//! ## 1. The outbox `INSERT` -- **atomic with the transition**
//!
//! Written by `settle`, inside the source's own transaction (spec §4.3). A
//! transition that commits has its event; one that rolls back has none. No
//! lock is needed: it only inserts. Proved by `tests/worker.rs` (W13a Task 3).
//!
//! ## 2. The claim -- safe by **exclusion**
//!
//! `SELECT … FROM workflow.flow_trigger_events WHERE id > $cursor AND
//! next_attempt_at <= now() ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED`, one
//! event per transaction. Two replicas' passes **partition** the outbox: an
//! event another pass holds is skipped, not waited on, and the lock is held
//! until that pass commits. The cursor (the last id this pass claimed) and
//! `next_attempt_at` (a kept event's backoff) are not part of the exclusion
//! argument: they bound what ONE pass claims (final review I3, F2), and a
//! pass is capped at [`MAX_EVENTS_PER_PASS`](process::MAX_EVENTS_PER_PASS)
//! events or [`MAX_PASS_DURATION`](process::MAX_PASS_DURATION), checking the
//! shutdown token between events, so a backlog never holds `fire_due` back.
//!
//! **A `SELECT … FOR UPDATE`, not the spec's `DELETE … RETURNING`.** Per-
//! listener retry (spec §4.4 point 1) sometimes KEEPS the event, updated, and
//! a deleted row would have to be re-inserted. The delete moved to statement
//! 5, in the same transaction, so "fire and consume commit together" holds
//! exactly as the spec's shape had it.
//!
//! **Its one serialization failure, handled.** The snapshot is taken at the
//! start of this statement and the lock after it. An event another processor
//! deletes or updates -- and commits -- between the two cannot be locked at
//! `REPEATABLE READ` (`40001`, measured: `could not serialize access due to
//! concurrent update`). Nothing has been done yet, so the pass rolls back and
//! claims again with a fresh snapshot, which no longer sees that row.
//!
//! Proved by `tests/runtime.rs`'s `two_processors_never_fire_one_event_twice`,
//! asserted on rows **and** on insert attempts counted by a non-transactional
//! sequence. Unmutated: 3 of 3 green, `attempts == rows == 30`. Mutations, W13a
//! Task 4, three runs each:
//!
//! - **drop `SKIP LOCKED`, keep `FOR UPDATE`: survives, 3 of 3, and the lock
//!   still holds** -- `attempts == rows == 30`. A blocked claim waits, and when
//!   the holder commits it meets the row changed after its snapshot, fails
//!   `40001`, and re-claims with a fresh snapshot. W12 measured the same shape
//!   for its claim. What `SKIP LOCKED` buys is throughput; no test observes it.
//! - **drop the whole lock: dies, 3 of 3, on the attempts assertion only** --
//!   60, 60 and 60 fires attempted for 30 rows. The rows stayed right: the
//!   duplicate transaction is rolled back by the unique index or by its own
//!   `40001` on the event's `DELETE`, which is why a rows-only test would pass.
//! - **drop the lock and the unique index: dies, 3 of 3, on attempts only**
//!   (60, 60, 54 for 30 rows). With the index gone the rows STILL stayed right:
//!   `REPEATABLE READ` alone refuses the second `DELETE` of one event, and that
//!   rolls back the duplicate fires with it. The index is the backstop for a
//!   READ COMMITTED world this code no longer runs in.
//!
//! ## 3. The listener `SELECT` -- **no lock, and none needed**
//!
//! The listeners that are ON, in the claim's snapshot: same workspace, the
//! event's state in `states`, not `disabled` (the YAML's and the trigger's
//! switches, copied by `sync`), plus [`flow_is_on!`](crate::scheduler::start)
//! -- the four W12 switches, the very SQL `fire_due` uses. It reads, and what
//! it reads is only ever compared, never written back: a save that turns a
//! listener off while an event is being processed is the same as that save
//! landing a moment after the event was processed, and an event is not a
//! clock, so there is nothing to recover later either (spec §6). The listener
//! flow's source is read in the same snapshot, so the YAML parsed and the row
//! selected agree.
//!
//! Proved by `tests/flow_trigger_fire.rs`'s `every_off_switch`. Mutations, one
//! run each, every one killed by that test and by nothing else in its binary:
//! `NOT l.disabled` dropped (`refused` 0 -> 2: `evaluate`'s FlowDisabled and
//! TriggerDisabled caught both, so only the `refused: 0` half of the assertion
//! sees it); `f.deleted_at IS NULL`, `f.state <> 'disabled'`, the workspace's
//! `status = 'active'` and the feature's `NOT EXISTS` each dropped (`fired`
//! 2 -> 3). The last four live in `scheduler::start::flow_is_on!`, shared
//! with `fire_due`.
//!
//! ## 4. The fire -- the unique index is the **backstop**
//!
//! [`start_execution_in`](crate::worker::start_execution_in), in a savepoint
//! of its own per listener, with `flow_id = Some(listener)` and
//! `flow_trigger_event_id = event`. `workflow_executions_one_per_flow_trigger_event`
//! (0157, `(flow_trigger_event_id, flow_id, execution_trigger->>'id')`) makes
//! a second execution for one (event, listener) **unrepresentable**. A
//! violation rolls back the savepoint and counts as `refused`, done with. Under
//! statement 2 it never happens.
//!
//! The savepoint is what keeps one listener's failure from undoing another's
//! fire. Proved by `a_broken_listener_does_not_stop_its_siblings` and
//! `a_transient_failure_keeps_the_event_for_that_listener_only`. Mutation,
//! one run: **fire on the transaction, no savepoint** -- dies in those two and
//! in `a_listener_that_keeps_failing_is_given_up_after_five_attempts`, each
//! with "current transaction is aborted". The broken-listener test needs its
//! `b_already` listener for that: a YAML that does not parse fails before any
//! SQL, and on its own would not have seen the savepoint go.
//!
//! ## 5. The event's last write -- **under the claim's lock**
//!
//! `DELETE … WHERE id = $1` when every listener is done (or the event has had
//! [`MAX_ATTEMPTS`](process::MAX_ATTEMPTS)), otherwise `UPDATE … SET done,
//! attempts = attempts + 1`. The row the claim locked, by primary key, in the
//! claim's transaction: it commits with the fires or not at all.
//!
//! Proved by `fire_and_delete_are_atomic` (a fault at `COMMIT`, after the
//! fire, leaves no execution and the event in place),
//! `a_transient_failure_keeps_the_event_for_that_listener_only` and
//! `a_listener_that_keeps_failing_is_given_up_after_five_attempts`. Mutations,
//! one run each:
//!
//! - **delete the event in its own transaction before firing**: dies in
//!   `fire_and_delete_are_atomic` ("the event is untouched") and in the two
//!   retry tests (the event is gone after pass 1).
//! - **roll back the whole event on a retried error**: dies in the two retry
//!   tests (the pass returns `Err`, and the healthy sibling's fire is undone).
//! - **never give up**: dies in the give-up test only ("given up: deleted").

mod process;

pub use process::{
    process_events, process_events_with, EventReport, PassLimits, DEFAULT_RETRY_BACKOFF,
    DEPENDS_ON_IS_W13B, MAX_ATTEMPTS, MAX_EVENTS_PER_PASS, MAX_PASS_DURATION,
};
