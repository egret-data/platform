# Workflow concurrency — gate every kind of start (package 8c)

**Status:** design, awaiting review
**Author:** egret workflow parity programme
**Date:** 2026-11-24
**Depends on:** 8a (concurrency core — #908), 8b (behaviors + unqueue/force-run — #909)

## 1. Goal

Make a flow's `concurrency: {limit, behavior}` apply to **every** way an
execution of that flow can start — not only a fresh start. Today (8a/8b) only
a fresh start (API, schedule, webhook, Flow trigger) passes the admission gate
(`concurrency::concurrency_admit`). Three other ways to start an execution
**bypass** the gate and run regardless of the limit:

- a **replay** (a new execution copied from an earlier one, whole or partial);
- a **restart-on-source** (a replay variant that runs the frozen/chosen source);
- a **restart-in-place** of a failed execution;
- a **subflow child** spawned by a `Subflow` task.

8c closes that gap: all of them go through the same gate and obey the same
behavior (QUEUE / CANCEL / FAIL) as a fresh start.

## 2. The corrected premise

The parity roadmap recorded this follow-up as *"count toward the limit but not
queued yet — bounded soft overshoot"*. **That premise is wrong**, measured
against Kestra, and 8c follows Kestra instead.

Kestra applies the concurrency gate in `ExecutionEventMessageHandler` to any
execution whose state is `CREATED` **or** `failedThenRestarted()`:

```java
// executor/.../handler/ExecutionEventMessageHandler.java
if ((execution.getState().getCurrent() == State.Type.CREATED
     || execution.getState().failedThenRestarted())) {
    // ... resolve concurrency limits, then QUEUE / CANCEL / FAIL
}
```

- `CREATED` covers a fresh start, a **replay** (a new execution is born
  `CREATED`), and a **subflow child** (also born `CREATED`).
- `failedThenRestarted()` is `current == RESTARTED && previous state isFailed()`
  — i.e. a **restart-in-place of a failed execution**. (A `RESTARTED` that
  follows `PAUSED` — a resume — is **not** `failedThenRestarted()`, so a resume
  is **never** re-gated.)

So Kestra queues/cancels/fails all four kinds of start, with **no soft
overshoot**. There is no special case that lets a replay or a subflow child
exceed the limit.

Two facts already true in egret and unchanged by 8c:

- The gate's live count —
  `count(*) WHERE flow_id = $1 AND NOT state_is_terminal(state) AND state <> 'QUEUED'`
  — is blind to **how** an execution started, so replays, restarts and subflow
  children **already count** toward the limit (they occupy slots). 8c only adds
  the missing half: those same starts are themselves gated when they would push
  the flow over the limit.
- The gate's scope is the flow's own concurrency. A subflow child is gated on
  the **child flow's** `concurrency:`, not the parent's.

## 3. Decisions

**D1 — Full parity, one package (user, 2026-11-24).** Gate all of replay,
restart-on-source, restart-in-place and subflow children in 8c, matching
Kestra's `CREATED | failedThenRestarted` rule. Do not split restart out to a
later package unless the implementation proves intractable (see §9 fallback).

**D2 — Deadlock footgun: match Kestra, do not guard (user, 2026-11-24).** A
flow that starts itself as a `Subflow` with `wait: true` under a low limit can
deadlock: the parent holds a slot while `RUNNING`, the child is `QUEUED`, and
neither can advance. Kestra has the same footgun and does not guard it. 8c does
not guard it either; §7 documents it for users.

**D3 — Uniform drain by history pop.** The drain restores a `QUEUED` execution
to the state it would have had if it had not been queued, then settles. 8a's
`promote_queued` hardcodes that restored state to `CREATED`; 8c generalises it
to **pop the trailing `QUEUED` from the stored history** and restore the prior
state (`CREATED` for fresh/replay/subflow, `RESTARTED` for a restart). This is
backward-compatible: a fresh start's history is `["CREATED","QUEUED"]`, and a
pop yields `["CREATED"]` with state `CREATED` — identical to today.

**D4 — Gating writes append, they do not overwrite.** When the gate decides
`QUEUE`, the `QUEUED` state is **appended** to the execution's current history.
For a new execution (fresh/replay/subflow) the current history is `["CREATED"]`
→ `["CREATED","QUEUED"]` (unchanged from 8a). For a restart-in-place the
current history is `[…,"FAILED","RESTARTED"]` → `[…,"FAILED","RESTARTED","QUEUED"]`,
which is exactly what D3's pop reverses.

**D5 — CANCEL / FAIL terminalize through settle, no task run.** As in 8b, an
over-limit start under `behavior: CANCEL`/`FAIL` becomes `CANCELLED`/`FAILED`
terminal immediately, with no task run and no slot consumed, but **through**
the first settle so terminal hooks (outbox, Flow-trigger listeners, alerts)
fire. For a replay/subflow child the new execution terminalizes before any run
is copied. For a restart-in-place the execution — already `FAILED` — is set
`CANCELLED`/`FAILED` through settle without driving.

**D6 — No migration, no new API, no FE change.** 8c reuses the `QUEUED` state,
the `unqueue`/`force-run` routes and the FE from 8a/8b. A queued replay /
restart / subflow child shows in the UI as an ordinary `QUEUED` execution and
can be unqueued or force-run exactly like a queued fresh start.

**D7 — Fold in the 8b residual minors.** See §8.

## 4. The four gating points

Each path reaches a point where it is about to run (drive) a new or reopened
execution. 8c inserts the gate at that point. The gate is always
`concurrency_admit(conn, flow_id, limit, behavior)` under the flow's advisory
lock, in the same transaction as the write that follows.

### 4.1 Fresh start (already done, 8a/8b)

`claim::start_execution_in_on` → `insert_execution_gated(…, gated=true)`.
Unchanged, except that it now shares the `Admit → row/settle` helper that 8c
factors out (§5) so every path treats the four outcomes identically.

### 4.2 Replay and restart-on-source (`worker/copy.rs`)

`copy::replay_locked` builds a **new** execution with `claim::insert_execution`
(today `gated=false`) and then settles it — `settle_new` for a whole replay
(no copied runs) or a copy-then-settle for a partial replay / restart-on-source.

8c change:

- Gate the insert (`gated=true`), capturing the `Admit`.
- `Admit::Run` → today's behaviour (copy runs if any, then settle).
- `Admit::Queue` → **still copy the runs** (a partial replay's copied runs must
  exist for the drain to resume), set the row `QUEUED` (history `["CREATED",
  "QUEUED"]`), and **skip the settle**. The drain resumes it.
- `Admit::Cancel`/`Fail` → do **not** copy runs; terminalize the new execution
  through settle (D5).

Because a replay is a new execution born `CREATED`, the drain's pop (D3) restores
`CREATED`, and `load` reads whatever runs were copied — a whole replay has none
(settles from empty), a partial replay has its copied prefix (settle resumes).

### 4.3 Restart-in-place (`worker/restart.rs`) — the hard path

`restart::restart_execution` locks the failed execution, `prepare` builds the
reset commands (clear each failed run's results/leases) and the `RESTARTED`
transition, and the settle walk applies + persists them and drives.

8c change: after the reset commands and `RESTARTED` are **persisted**, but
before the walk drives, check the gate.

- `Admit::Run` → drive (today's behaviour).
- `Admit::Queue` → append `QUEUED` (history `[…,"RESTARTED","QUEUED"]`), persist,
  and stop before driving. The reset runs are already persisted, so the drain
  (D3 pop → `RESTARTED`; `load` reads the reset runs; settle drives) resumes an
  identical restart.
- `Admit::Cancel`/`Fail` → set `CANCELLED`/`FAILED` through settle without
  driving (D5).

This is the one path where the gate falls **inside** the settle walk rather
than before an insert, so it is the riskiest task. The implementer finds the
cleanest seam that (a) persists the resets before the `QUEUED` stop and (b)
leaves a row the uniform drain can resume. §9 gives the fallback if the seam
proves intractable.

### 4.4 Subflow child (`worker/subflow.rs`)

The `Subflow` task spawns a child with its own INSERT (born `CREATED`) and then
settles the child inside the parent's settle (`wait: false`, reading the child
state at this instant).

8c change:

- Gate on the **child flow's** `flow_id` + `concurrency:`
  (`concurrency_admit`), under the child flow's advisory lock.
- `Admit::Run` → today's behaviour.
- `Admit::Queue` → insert the child `QUEUED`, **skip the child settle**. The
  parent's `Subflow` run sees a non-terminal (`QUEUED`) child: with `wait: true`
  it keeps waiting; the drain runs the child when a slot frees, and the child's
  terminal settle notifies the parent through the existing subflow-completion
  path.
- `Admit::Cancel`/`Fail` → insert the child, terminalize it through settle (D5).
  The parent's `Subflow` run sees a terminal `CANCELLED`/`FAILED` child and
  reacts as it does to any terminal child (`transmitFailed`, etc.).

A subflow child is born `CREATED` with no runs, so the uniform drain restores
`CREATED` and settles from empty — the existing `promote_queued` path, no
special case.

## 5. The shared admit helper

8c factors the `Admit → (row state, row history, whether to settle, which
terminal)` decision into one helper so all four paths treat the four outcomes
identically and cannot drift. Shape (illustrative, the implementer may adjust
names):

- input: the current history of the row about to be written/reopened and the
  `Admit`;
- output: the state + history to persist, and whether the caller should settle
  normally, terminalize (with which state), or stop (queue).

Fresh start, replay, subflow child write a new row (`["CREATED"]` base);
restart-in-place reopens an existing row (its live history base). The helper's
append/terminalize logic is the same for both; only the base history differs.

## 6. The drain (`worker/concurrency.rs`)

`concurrency_drain`, `on_terminal` and `concurrency_sweep` are **unchanged** in
shape: they still find the oldest `QUEUED` rows of a flow under its limit and
call `promote_queued`. The only change is inside `claim::promote_queued` (D3):
it pops the trailing `QUEUED` from the stored history and restores the prior
state before `load` + `settle`, instead of hardcoding `CREATED`.

This means a queued replay, restart or subflow child is drained by exactly the
same oldest-first machinery as a queued fresh start, honouring FIFO order
across all kinds of start of a flow.

## 7. Deadlock footgun (documented, not guarded — D2)

A flow that starts **itself** as a `Subflow` with `wait: true` under a
concurrency limit lower than its own recursion depth can deadlock: the parent
holds slot(s) while `RUNNING` and waiting, the child is `QUEUED` and cannot be
promoted until a slot frees, and no slot frees until the parent completes.
Kestra has the same behaviour. The docs (`concurrency.md`) warn that a
self-referential `Subflow` under a low limit with `wait: true` can stall, and
that `unqueue`/`force-run` (8b) are the manual escape hatch.

## 8. Residual 8b minors folded in

- **M2 — unqueue `?state=` parse.** An unparseable `?state=` on the single/
  by-ids unqueue route returns an uncoded 400; give it a coded, documented
  error consistent with the other input refusals.
- **Deadlock-race test.** A deterministic test of the ABBA that 8b's
  deadlock-retry absorbs (force-run/unqueue advisory→row vs settle-drain
  mutators row→advisory), rather than relying on the retry alone.
- **MAX_BULK cap test.** Assert the `MAX_BULK` (1000) cap is enforced for the
  unqueue and force-run by-ids/by-query routes.
- **Bulk-over-limit race test.** Assert that a bulk force-run that crosses the
  limit respects the behaviour (the drain's live count stays correct under
  concurrent promotion).

## 9. Risk and fallback

The restart-in-place gate (§4.3) is the only change that reaches inside the
settle walk and is the highest-risk task. If the seam proves intractable within
the package, the fallback is: keep restart-in-place at soft-overshoot under
`behavior: QUEUE` **only** (it still terminalizes under CANCEL/FAIL, which needs
no resume-drain), ship the other three paths, and record restart-in-place
QUEUE gating as package 8d. The default is to complete all four in 8c.

## 10. Testing

Per path, with `concurrency: {limit: 1, behavior: QUEUE|CANCEL|FAIL}`:

- **Replay** over the limit: QUEUE → new execution `QUEUED`, drained to a
  correct resume (whole replay runs from empty; partial replay resumes its
  copied prefix); CANCEL/FAIL → terminal, no runs, hooks fired.
- **Restart-on-source** over the limit: same three outcomes.
- **Restart-in-place** over the limit: QUEUE → `QUEUED` then drained to an
  identical restart (reset runs resume); CANCEL/FAIL → terminal.
- **Subflow child** over the limit: QUEUE → child `QUEUED`, parent waits, drain
  runs child, parent notified; CANCEL/FAIL → child terminal, parent reacts.
- **FIFO** across kinds: a queued fresh start and a queued replay of the same
  flow drain oldest-first.
- **Drain equivalence**: a drained queued fresh start is byte-for-byte what 8a
  produced (D3 backward-compatibility).
- The §8 minors' tests.

Scoped local runs for `egret-workflow` **must** include `--test routes --test
execution_timestamps` (both guards live in binaries the scoped run otherwise
skips; they broke CI in 8a and 8b).

## 11. Out of scope

- Namespace- and tenant-scoped concurrency limits (Kestra EE; egret resolves
  only the flow's own `concurrency:`).
- Any change to the `unqueue`/`force-run` API surface or the FE beyond the M2
  error code.
- Guarding the self-subflow deadlock (D2).
