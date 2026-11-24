# Workflow package 8b — concurrency behaviors + unqueue/force-run — Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development. `- [ ]` steps.

**Goal:** enforce `concurrency.behavior` CANCEL/FAIL; add `unqueue` + `force-run` (single + by-ids/by-query); FE actions + flow concurrency.limit display. Builds on 8a. No migration.

## Global Constraints
- Conventional Commits; `Closes #N` only in PR body; trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Stage explicit paths; never `git add -A`/`target*/`.
- **`cargo fmt --all --check` before every push** (#904). LOCAL TEST SCOPED (`--test X`/`--lib`); full `-p egret-workflow` HANGS, CI runs `--workspace`. Env: DATABASE_URL=…5468/egret_test (new worktree port), EGRET_TEST_SHARED_STORE_URL=…6468, EGRET_ALLOW_DOCKER_SKIP=1 EGRET_ALLOW_ENGINE_SKIP=1, unset EGRET_ALLOW_DB_SKIP.
- unqueue/force-run gate `workflow:manage`; bulk = 7f `bulk::{bulk_apply, resolve_ids_by_query, MAX_BULK}`, all-or-nothing, 404-not-403, distinct op_ids.
- No migration. Regen openapi+client when the flow DTO gains `concurrency`.

## Task 0: worktree + baseline
- [ ] `feat/workflow-concurrency-behaviors` off main (after 8a merged). Per-worktree Postgres :5468 + valkey :6468, migrate, green baseline (`cargo test -p egret-workflow --test concurrency --lib`).

## Task 1: enforce CANCEL / FAIL at the gate
**Files:** worker/concurrency.rs, claim.rs (create path), validate.rs (warning), tests.
- [ ] `concurrency_admit` → `enum Admit { Run, Queue, Cancel, Fail }` decided from `concurrency.behavior` when `live >= limit` (Queue for QUEUE, Cancel for CANCEL, Fail for FAIL). Under limit → Run.
- [ ] Create path: on `Cancel`/`Fail`, insert the execution as a TERMINAL row — state CANCELLED/FAILED, history `["CREATED","<state>"]`, `ended_at` set, NO settle, NO task run. (CANCEL/FAIL do NOT consume a slot — they terminate.)
- [ ] Remove the `NOT_YET_ENFORCED` "concurrency" warning for CANCEL/FAIL (now enforced); update `the_not_yet_enforced_list_is_pinned_in_both_directions` + `property-treatment.json` so concurrency is fully honoured (no behavior warns).
- [ ] Tests: over-limit + CANCEL → new execution CANCELLED (terminal, no task, slot not consumed — a 3rd CANCEL start doesn't block a later fresh start); + FAIL → FAILED; QUEUE unchanged (8a); no warning for any behavior now.
- [ ] fmt+clippy; commit.

## Task 2: unqueue (single + bulk)
**Files:** routes.rs, worker (unqueue apply), tests.
- [ ] Factor `unqueue_check` (read-only: exists + ownership + WF_MANAGE PDP + **is-QUEUED** else `not_queued`) + `unqueue_apply` (set the execution to the target terminal state + `ended_at` + audit).
- [ ] `POST .../executions/{id}/unqueue?state=` (op_id `unqueue_execution`): default state CANCELLED; a NON-terminal `state` → 400 `unqueue_state_not_terminal`; 409 `not_queued` if not QUEUED.
- [ ] `POST .../executions/unqueue/by-ids` + `.../unqueue/by-query` (op_ids `unqueue_executions_by_ids`/`_by_query`): `bulk_apply(ids, unqueue_check, |id| unqueue_apply(...,state))`; cap MAX_BULK; WF_MANAGE; the `state` param applies to the batch.
- [ ] Tests: QUEUED→CANCELLED (default) + →a given terminal; 409 not-QUEUED; 400 non-terminal state; bulk all-or-nothing (one non-QUEUED id → 400, nothing changed) + cap + 403.
- [ ] fmt+clippy; commit.

## Task 3: force-run (single + bulk)
**Files:** routes.rs, worker/concurrency.rs (force promote), tests.
- [ ] `force_run_check` (exists + ownership + WF_MANAGE + is-QUEUED else `not_queued`) + `force_run_apply` (promote via 8a `promote_queued` but SKIP the `live < limit` guard — run regardless of the limit; still under the per-flow advisory lock to serialize with drains).
- [ ] `POST .../executions/{id}/force-run` (op_id `force_run_execution`): QUEUED → runs now even if the flow is at/over its limit; 409 `not_queued` otherwise.
- [ ] `POST .../executions/force-run/by-ids` + `.../force-run/by-query` (op_ids `force_run_executions_by_ids`/`_by_query`): bulk_apply; cap; WF_MANAGE.
- [ ] Tests: QUEUED force-run → runs even when live==limit (assert it leaves QUEUED → running despite the flow being full); 409 not-QUEUED; bulk all-or-nothing + cap + 403.
- [ ] fmt+clippy; commit.

## Task 4: flow DTO concurrency + regen
**Files:** the flow GET DTO (routes.rs / models), openapi.json, frontend/src/api/generated.
- [ ] Add `concurrency` (limit + behavior) to the flow GET response DTO (no migration — it's already on the Flow model).
- [ ] `cargo run -p egret-bin -- openapi > openapi.json`; `cd frontend && npm run gen:api`; typecheck. Stage only openapi.json + generated (+ the DTO change in a separate commit).
- [ ] Commit (DTO change) + commit (regen).

## Task 5: frontend (unqueue/force-run actions + limit display)
**Files:** frontend/src/** (execution detail actions + bulk toolbar + flow detail). Follow docs/en/frontend-rules.md.
- [ ] On a QUEUED execution's detail: **Unqueue** (confirm; default Cancel; optional terminal-state picker) + **Force run** buttons → the single-op hooks.
- [ ] Executions list bulk toolbar: **Unqueue** + **Force run** entries, enabled only when the selection is all QUEUED; by-ids + select-all-matching-filter → by-query; confirm + 400 `BulkErrorResponse` surfacing (mirror 7f).
- [ ] Flow detail: show `concurrency.limit` (+ behavior) from the flow DTO.
- [ ] Tests (vitest): buttons appear for QUEUED only; bulk entries send the right by-ids/by-query requests; limit renders.
- [ ] lint+typecheck+test; commit.

## Task Z: docs, gates, live-check, merge
- [ ] Docs (egret-data/docs, bilingual; branch off LATEST origin/main to avoid conflicts): CANCEL/FAIL behaviors, unqueue, force-run, the limit display. Docs PR merges at once.
- [ ] Gates: `cargo fmt --all --check`; clippy `-p egret-workflow --all-targets -D warnings`; scoped `cargo test -p egret-workflow --test concurrency --test executions_routes --lib`. Renumber nothing (no migration).
- [ ] Live-check: limit 1 + behavior CANCEL → 2nd start CANCELLED; behavior QUEUE → unqueue a QUEUED (→ cancelled) and force-run another (→ runs despite full).
- [ ] Push + PR; CI green → merge (NO --admin; fmt --check first). Cleanup worktree/containers.

## Self-Review
- Coverage: CANCEL/FAIL → T1; unqueue → T2; force-run → T3; flow DTO+regen → T4; FE → T5. Bulk reuses 7f machinery. Replay/restart/subflow-gating intentionally OUT (8c follow-up).
- force-run must bypass the limit (the one place that skips the under-limit check) — T3 asserts it runs at/over limit. unqueue targets terminal only. CANCEL/FAIL insert terminal (no slot consumed) — T1 asserts a CANCEL start doesn't block later fresh starts.
