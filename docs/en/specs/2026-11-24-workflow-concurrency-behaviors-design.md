# Workflow package 8b — concurrency behaviors + unqueue/force-run (design)

**Status:** design (brainstormed 2026-11-24; split confirmed). **Package:** #8 concurrency, sub-package **8b** (after 8a core). Replay/restart/subflow-child gating = SEPARATE follow-up (8c), NOT in 8b.
**Branch (future):** `feat/workflow-concurrency-behaviors` off main AFTER 8a merges.
**Migration:** none (CANCELLED/FAILED states exist; no new state).

## Goal
Finish Kestra flow concurrency on top of 8a: enforce the non-QUEUE behaviors (`CANCEL`/`FAIL`), and add the queue-management ops `unqueue` + `force-run` (single + by-ids/by-query bulk), plus the FE actions + the flow concurrency.limit display.

## Scope (8b)
1. **Enforce CANCEL / FAIL at the admission gate.** 8a's `concurrency_admit` returns `Run|Queue` for `behavior: QUEUE`. Extend it to honor the behavior: when a fresh start is over the limit —
   - `QUEUE` → QUEUED (8a, unchanged).
   - `CANCEL` → the new execution is created **CANCELLED** (terminal immediately; history `["CREATED","CANCELLED"]`, `ended_at` set, no task run).
   - `FAIL` → created **FAILED** (same shape, state FAILED).
   The gate already runs under the per-flow advisory lock; CANCEL/FAIL just create a terminal row instead of a QUEUED one (they do NOT consume a slot — they terminate). **Remove the `NOT_YET_ENFORCED` warning for CANCEL/FAIL** (now enforced) and update `the_not_yet_enforced_list_is_pinned_in_both_directions` + `property-treatment.json`.
2. **unqueue** — `POST /api/ws/{ws}/workflows/executions/{id}/unqueue?state=` : only a **QUEUED** execution (409 `not_queued` otherwise); move it to a **terminal** state (`?state=`, default **CANCELLED**; a non-terminal `state` → 400). Gate `workflow:manage`. + `POST .../unqueue/by-ids` + `.../unqueue/by-query` (bulk, all-or-nothing via the 7f `bulk_apply`, cap `MAX_BULK`, `check` = is-QUEUED).
3. **force-run** — `POST /api/ws/{ws}/workflows/executions/{id}/force-run` : only a **QUEUED** execution (409 `not_queued` otherwise — egret's only held state is QUEUED; a diverge-from-Kestra note: Kestra allows any non-terminated, but egret has nothing else to force); promote it **now, bypassing the limit** (via 8a's `promote_queued`/`settle_new`, WITHOUT the under-limit check — it runs even if the flow is at/over its limit). Gate `workflow:manage`. + `.../force-run/by-ids` + `.../force-run/by-query` (bulk).
4. **Flow concurrency.limit display (FE).** 8a found `concurrency` is not in the flow HTTP response. Add `concurrency` (limit + behavior) to the flow GET DTO (small backend addition, no migration), regen openapi + client, and show it on the flow detail + beside a QUEUED execution ("queued under limit N").
5. **FE actions.** On a QUEUED execution's detail: **Unqueue** (confirm; optional target-state picker, default Cancel) + **Force run** buttons. On the executions list bulk toolbar: **Unqueue** + **Force run** entries for a selection of QUEUED executions (by-ids + select-all-matching-filter → by-query), mirroring the 7f bulk UI + confirm/400-surfacing.

## Architecture
- `worker::concurrency::concurrency_admit` → return `enum Admit { Run, Queue, Cancel, Fail }` (from the flow's behavior when over limit); the create path inserts QUEUED / CANCELLED / FAILED accordingly (CANCEL/FAIL = terminal insert, no settle, no task).
- `unqueue` / `force-run` single handlers in routes.rs; factor `_check` (is-QUEUED) + `_apply` (unqueue = set terminal state + audit; force-run = `promote_queued` ignoring the limit) so the single-op and the bulk share one core (the 7f pattern). Bulk via `bulk::bulk_apply` + `resolve_ids_by_query`.
- force-run reuses 8a's `promote_queued` but SKIPS the `live < limit` guard (it is the explicit "run regardless" path); still under the per-flow advisory lock to serialize with drains.
- Flow DTO gains `concurrency`; regen.

## Kestra divergences (record)
1. unqueue sets a TERMINAL state (default CANCELLED); Kestra's unqueue accepts any state incl. RUNNING — egret keeps unqueue=terminal and force-run=promote (two clear ops). Note.
2. force-run is QUEUED-only in egret (Kestra: any non-terminated) — egret has no other held state.
3. Sync (200), not Kestra's 202 + command queue; bulk all-or-nothing + MAX_BULK cap (as 7f).
4. Replay/restart/subflow-child gating still deferred (8c).

## Security
- unqueue/force-run gate `workflow:manage`; bulk 404-not-403 for foreign/denied ids; per-object PDP. force-run bypassing the limit is an explicit privileged action (manage).

## Tests
- gate: over-limit + CANCEL → new execution CANCELLED (terminal, no task, no slot consumed); + FAIL → FAILED; QUEUE unchanged. Warning no longer fires for CANCEL/FAIL.
- unqueue: QUEUED → CANCELLED (default) / a given terminal state; 409 if not QUEUED; non-terminal state → 400; bulk by-ids/by-query all-or-nothing + cap + 403.
- force-run: QUEUED → runs even when the flow is at limit (assert it leaves QUEUED → running despite live==limit); 409 if not QUEUED; bulk.
- FE: unqueue/force-run buttons on a QUEUED execution; bulk toolbar entries; limit shown on the flow.

## Open decisions (defaults; confirm)
- Q1 split: one 8b package (behaviors + unqueue/force-run + FE); replay/restart/subflow-gating = follow-up 8c. ✅ (user)
- Q2 unqueue default state = CANCELLED; allowed = terminal only (non-terminal → 400). *(default)*
- Q3 force-run = QUEUED-only, bypasses the limit. *(default)*
- Q4 add `concurrency` to the flow DTO for the FE limit display. *(default)*
