# Workflow package 8a — flow concurrency core (design)

**Status:** design (user pre-approved auto-implement 2026-11-24; decisions locked in brainstorm). **Package:** #8 concurrency, sub-package **8a** (core); **8b** follows (CANCEL/FAIL + unqueue/force-run + FE actions).
**Branch:** `feat/workflow-concurrency` (worktree `egret-concurrency`, off main `faaf51f15`).
**Migration:** yes — a new `QUEUED` execution state (next number 0242; renumber at merge).

## Goal

Port Kestra's flow-level concurrency core: a flow may declare `concurrency: { limit: <int ≥1>, behavior: QUEUE }`; when the flow already has `limit` non-terminal executions, a newly-created execution is held in a new **`QUEUED`** state instead of running, and is promoted to run when a slot frees. 8a ships the `QUEUE` behavior only.

## Scope (8a)
1. **Flow YAML** `concurrency: { limit, behavior }` — parse + validate (`limit ≥ 1`; `behavior` defaults `QUEUE`; `CANCEL`/`FAIL` are **ACCEPTED at publish** (Kestra accepts them; the oracle banks 3 such flows) but NOT yet enforced — a `NOT_YET_ENFORCED` warning fires for non-QUEUE behavior; 8b implements them. (Reversed from an earlier reject decision after the oracle conflict — user 2026-11-24.)
2. **`QUEUED` execution state** (migration) — not terminal, not idle, not "running a task". QUEUED executions do NOT count toward the limit; live non-terminal non-QUEUED executions do.
3. **Race-safe admission gate at execution-create** — inside the create transaction, for a flow with a limit, take a per-flow `pg_advisory_xact_lock(hashtext(namespace||'/'||flow_key))`, count the flow's live executions, and admit RUN (< limit) or QUEUE (≥ limit). The lock serializes concurrent creates for the same flow so two simultaneous starts cannot both pass the limit.
4. **Drain when a slot frees** — when an execution of the flow reaches a terminal state (in the settle that terminalizes it), under the same per-flow lock, promote the **oldest** QUEUED execution of that flow (`ORDER BY created_at, id`) to start via the normal start path. One slot per freed slot. Plus a **safety sweep** (periodic) that drains any flow left under-limit with QUEUED waiting (covers a missed drain, e.g. a crash between terminalize and drain).
5. **Children** count per their own flow (a subflow child against the subflow flow's limit; a `CREATE_NEW_EXECUTION` retry against its flow) — faithful to Kestra (concurrency is a property of a flow, enforced on every execution of it).
6. **FE** — a `QUEUED` state chip + filter value in the executions list/detail; show the flow's `concurrency.limit` where the flow is displayed. No queue-management actions (those are 8b).

**Deferred to 8b:** `behavior: CANCEL` (new execution → CANCELLED), `behavior: FAIL` (→ FAILED); `unqueue` (QUEUED → drop/cancel) and `force-run` (QUEUED → run now, bypass limit) single-op + bulk (the 7f-deferred ops) + their FE actions.

## Architecture
- **Flow model:** `Concurrency { limit: u32, behavior: Behavior }` on the parsed `Flow`; `Behavior` enum carries Queue/Cancel/Fail but publish rejects Cancel/Fail in 8a.
- **State:** add `QUEUED` to the execution-state representation + the migration; audit `workflow.execution_is_idle`, terminal predicates, the pruner, logs settle for correct QUEUED handling (QUEUED ≠ terminal, ≠ idle).
- **Admission gate:** `worker::concurrency::concurrency_admit(tx, flow_identity) -> Admit::Run | Admit::Queue` wired into the create path (`claim::insert_execution` / `start_flow_execution`); a flow with no `concurrency` skips the gate (always Run).
- **Drain:** `worker::concurrency::concurrency_drain(tx, flow_identity)` in the terminalize settle + a safety sweep; the promoted execution runs via the normal start path (identical to a fresh start).
- The advisory-lock key derives from the flow identity (namespace + flow_key), stable across executions of the flow.

## Kestra divergences (to record)
1. Only `QUEUE` behavior ENFORCED in 8a; CANCEL/FAIL accepted + warned (not enforced) until 8b.
2. Enforcement is a Postgres advisory lock + count, not Kestra's in-memory `ExecutionRunning` (egret's source of truth is the DB — matches its claim-based model).
3. unqueue/force-run deferred to 8b.

## Security / correctness
- Admission + drain run under the per-flow advisory lock within a transaction → no double-admit, no over-promote past the limit.
- QUEUED executions are subject to the same workspace scoping + per-object PDP as any execution.
- Drain is oldest-first (no starvation); the safety sweep recovers a missed drain.

## Tests
- parse/validate `concurrency` (limit ≥1; QUEUE; reject CANCEL/FAIL at publish).
- admission: limit N, N live → (N+1)th create QUEUED; under N runs; no-concurrency flow ignores the gate.
- **race:** two concurrent creates at the boundary (limit N, N-1 live) → exactly one runs, one queues.
- drain: terminalizing one live execution promotes the oldest QUEUED; order oldest-first; never over-promotes; the promoted one runs like a fresh start.
- QUEUED not terminal/idle; pruner/logs/`execution_is_idle` treat it right.
- safety sweep drains a flow left under-limit with QUEUED waiting.
- FE: QUEUED chip + filter; flow shows the limit.

## Decisions (locked 2026-11-24)
- **Split 8a/8b** ✅ — 8a core (limit + QUEUED + gate + drain + sweep + QUEUE + FE-state); 8b CANCEL/FAIL + unqueue/force-run + FE actions.
- **Accept CANCEL/FAIL at publish + NOT_YET_ENFORCED warning** (reversed reject after oracle conflict) ✅ user 2026-11-24.
- **Gate** = per-flow `pg_advisory_xact_lock` + count ✅.
- **Limit counts** live non-terminal non-QUEUED executions ✅.
- **Drain** oldest-first on terminalize + safety sweep ✅.
