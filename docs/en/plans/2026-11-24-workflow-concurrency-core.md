# Workflow package 8a — flow concurrency core — Implementation Plan

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Kestra flow concurrency core — `concurrency.limit` with `behavior: QUEUE`, a new `QUEUED` execution state, a race-safe admission gate at execution-create, drain-on-terminalize + a safety sweep, FE for the QUEUED state. CANCEL/FAIL + unqueue/force-run are 8b.

**Architecture:** A per-flow Postgres advisory lock serializes concurrent creates for a flow; under it, count live executions and admit RUN or QUEUE. On terminalize, under the same lock, promote the oldest QUEUED via the normal start path. A periodic safety sweep covers a missed drain. DB is the source of truth.

## Global Constraints
- Conventional Commits; `Closes #N` only in PR body; co-author trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. Stage by explicit path; never `git add -A`/`target*/`.
- Migration: next free number is **0242** (renumber above main + open PRs at merge — migration numbers are a shared namespace).
- **Run `cargo fmt --all --check` before every push** (CI's rust job fails fast on fmt — lesson from #904).
- Local test: use SCOPED `cargo test -p egret-workflow --test <X> --lib` — a full `cargo test -p egret-workflow --no-fail-fast` HANGS locally (a binary needs a service it doesn't self-skip); rely on CI for the full `--workspace` run. Env: `DATABASE_URL=postgres://egret:egret@localhost:5467/egret_test`, `EGRET_TEST_SHARED_STORE_URL=redis://127.0.0.1:6467`, `EGRET_ALLOW_DOCKER_SKIP=1 EGRET_ALLOW_ENGINE_SKIP=1`, unset `EGRET_ALLOW_DB_SKIP`.
- `behavior: CANCEL`/`FAIL` → **rejected at publish** in 8a ("supported in a later release").
- QUEUED is NOT terminal, NOT idle, NOT "running a task" — audit every state predicate.
- Regen openapi.json (`cargo run -p egret-bin -- openapi > openapi.json`) + FE client (`npm run gen:api`) when the state enum / flow DTO changes.
- Any new `std::env::var` must go in egret-config's `ENV_OUTSIDE_CONFIG` allow-list (run `cargo test -p egret-config`) — only if 8a adds an env var (e.g. the sweep interval; prefer a const).

## Task 0: baseline
- [ ] Worktree `egret-concurrency` on `feat/workflow-concurrency` (done, off main faaf51f15). Test DB egret-cc-postgres :5467 + egret-cc-valkey :6467 up + migrated (done). Green baseline: `cargo test -p egret-workflow --test executions_routes --lib` (with the env above). Stop/report if red.

## Task 1: QUEUED state + Concurrency flow model + validation
**Files:** `migrations/0242_execution_queued_state.sql`; flow model (`crates/egret-workflow/src/models/` or wherever `Flow` is parsed); `publish.rs`/validate; state predicates; tests.
- [ ] Migration 0242: add `QUEUED` to the execution state CHECK/enum. Confirm `workflow.execution_is_idle` + terminal predicates: QUEUED → not idle, not terminal. (Grep the SQL state lists; add QUEUED where "live/non-terminal" is meant, exclude from "terminal".)
- [ ] Flow model: `Concurrency { limit: u32, behavior: Behavior { Queue, Cancel, Fail } }` parsed from `concurrency:` YAML; `Option<Concurrency>` on `Flow`.
- [ ] Validation (publish): `limit ≥ 1` (else error); `behavior` CANCEL/FAIL → publish error "concurrency behavior CANCEL/FAIL is supported in a later release".
- [ ] Tests: parse a flow with `concurrency:{limit:2,behavior:QUEUE}`; reject limit 0; reject CANCEL and FAIL at publish; a QUEUED execution row reads as not-idle/not-terminal (unit or SQL test).
- [ ] fmt+clippy; commit (migration + model + publish + tests).

## Task 2: race-safe admission gate
**Files:** `crates/egret-workflow/src/worker/concurrency.rs` (new); `crates/egret-workflow/src/worker/claim.rs` (wire into create path); `worker/mod.rs` (re-export); tests.
- [ ] `pub async fn concurrency_admit(tx, namespace, flow_key, limit) -> Admit` — `pg_advisory_xact_lock(hashtext(namespace||'/'||flow_key))`; `SELECT count(*) FROM workflow.executions WHERE namespace=$ AND flow_key=$ AND state IN (<live non-terminal non-QUEUED>)`; `Admit::Run` if count < limit else `Admit::Queue`.
- [ ] Wire into `insert_execution`/`start_flow_execution`: when the flow has `concurrency`, call `concurrency_admit` inside the create transaction; on `Queue`, insert the execution in `QUEUED` state (do NOT hand it to the executor/claim); on `Run`, proceed as today. A flow without `concurrency` skips the gate.
- [ ] Tests (`tests/executions_routes.rs` or a new `tests/concurrency.rs`): limit N, N live → (N+1)th start → QUEUED (assert state, assert it runs no task); under N → RUNNING; no-concurrency flow unaffected; **race test** — spawn two concurrent starts at the boundary (limit N, N-1 live) and assert exactly one RUNNING + one QUEUED (`#[sqlx::test]` or two tasks against the shared DB).
- [ ] fmt+clippy; commit.

## Task 3: drain + safety sweep
**Files:** `worker/concurrency.rs`; the settle/terminalize path; the sweep; tests.
- [ ] `pub async fn concurrency_drain(tx, namespace, flow_key, limit)` — under the per-flow advisory lock, while live-count < limit and a QUEUED exists, promote the OLDEST QUEUED (`ORDER BY created_at, id`) to start via the normal start path. Promote one per freed slot (a single terminalize frees one; the sweep may promote several).
- [ ] Wire `concurrency_drain` into the settle that terminalizes an execution (after the terminal state is committed), for the terminalized execution's flow (only if that flow has `concurrency`).
- [ ] Safety sweep: a periodic job (reuse an existing sweeper cadence; a const interval, no new env var if avoidable) that, per flow with QUEUED waiting and live-count < limit, drains.
- [ ] Tests: terminalizing one live execution promotes the oldest QUEUED; oldest-first order; drain never exceeds limit; the sweep recovers a flow left under-limit with QUEUED (simulate a missed drain); a promoted execution runs identically to a fresh start.
- [ ] fmt+clippy; commit.

## Task 4: frontend (QUEUED state)
**Files:** `frontend/src/**` (executions list/detail state chip + filter; flow concurrency display); tests. Follow docs/en/frontend-rules.md.
- [ ] QUEUED state chip + add QUEUED to the state filter options; show a flow's `concurrency.limit` where the flow is shown.
- [ ] Tests: QUEUED chip renders; filter includes QUEUED; limit shown.
- [ ] `npm run lint && typecheck && npm test -- executions`; commit.

## Task 5: regen openapi + client
- [ ] `cargo run -p egret-bin -- openapi > openapi.json`; `cd frontend && npm run gen:api`; typecheck clean. Stage only openapi.json + frontend/src/api/generated. Commit.

## Task Z: docs, gates, live-check, merge
- [ ] Docs (egret-data/docs, bilingual): flow `concurrency`, the QUEUED state, QUEUE behavior, limit. Docs PR merges at once on approval. **Rebase the docs branch onto origin/main before merging** (the executions docs pages conflict-merge often — resolve by keeping all sections).
- [ ] Gates: `cargo fmt --all --check`; `cargo clippy -p egret-workflow --all-targets -- -D warnings`; scoped `cargo test -p egret-workflow --test executions_routes --test concurrency --lib` (NOT the full `-p egret-workflow` which hangs locally). Renumber the migration above main + open PRs at merge.
- [ ] Live-check: a flow `concurrency.limit: 1`; start 2 executions → 2nd is QUEUED; finish the 1st → 2nd promotes and runs (verify QUEUED chip in the browser).
- [ ] Push + PR; **CI green → merge (NO --admin; `cargo fmt --all --check` first)**. Clean up worktree/containers.

## Self-Review notes
- Spec coverage: limit+QUEUE+QUEUED+gate+drain+sweep+reject-CANCEL/FAIL+FE → Tasks 1-5; race-safety → Task 2 advisory lock + race test; CANCEL/FAIL + unqueue/force-run → 8b.
- The drain must not over-promote past the limit (advisory lock + re-count) and must not starve (oldest-first) — Task 3 tests both.
- Migration number assigned 0242; renumber at merge.
