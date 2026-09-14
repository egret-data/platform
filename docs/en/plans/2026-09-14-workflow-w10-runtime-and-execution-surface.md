# W10 — Runtime and execution surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make egret actually run workflows in production, and expose executions over HTTP, so W7 has something to render.

**Architecture:** Two config-gated tickers in `egret-bin` (`claim::tick`, `reaper::reap`) copying `egret_automation::runner::spawn`'s shape — one periodic task per replica, no leader election, because `claim::claim` uses `FOR UPDATE SKIP LOCKED`. Then an `ExecutionRepo` read layer and a REST surface in `egret-workflow/src/routes.rs`, in the same `WsContext` + `authz::require` + utoipa shape the nine flow endpoints already use.

**Tech Stack:** Rust, axum, sqlx (runtime queries, **no compile-time macros in this workspace**), tokio, utoipa, orval-generated TS client.

**Scope decision, resolved rather than left open.** The spec's §5 leaves logs to the reviewer. **This plan scopes W10 to the runtime and the execution surface, and defers `workflow.task_logs` to W11**, with the measurement: no table exists, `LogSink`'s three implementations persist nothing, and the container handler stores `stdOutLineCount`/`stdErrLineCount` — counts, not content. Logs unblock only the logs panel; runtime and the API unblock W7's gantt and topology. **Reversible:** if the reviewer wants logs in W10, they become Tasks 8–9 and the end-to-end proof grows one assertion.

**A warning about this document.** Code blocks below are **shapes to verify, not text to paste**. On the previous branch four separate errors shipped inside a plan's illustrative code, and prose in a plan was factually wrong six times out of twenty-four. **Read the file before editing it, and treat any disagreement between this plan and the code as the plan being wrong.**

---

## Global Constraints

Every task's requirements implicitly include this section.

- **The ratchet cannot judge this package, in either direction.** `oracle-exec/driver.rs:884` calls `worker::tick(pool, BATCH, reg)` **directly with its own registry**, so the oracle already exercises the loop's body and is blind to the binary's wiring by construction. **Expected ratchet: 109 before and after, md5 `c702545e…`, id list `3c290eeab650320a1a9d7b5fb5d92d75`, unchanged at every commit.** Report it beside a number that could disagree; a package that reports only "unchanged" is reporting an unfalsifiable zero.
- **Baseline gate at `92cfae9b` (main): 259 result lines / 3513 passed / 0 failed / 5 ignored**, exit 0, with **all three** service variables exported: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` (**never** the dev DB in `.env`), `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`, `EGRET_TEST_STARROCKS_URL`. Without the latter two you get exactly **20** service-gate failures.
- **Reconcile every gate by test-name-set diff, never arithmetic**, over **both** `crates/egret-workflow/tests/executor.rs` and `crates/egret-workflow/oracle-exec/driver.rs`. Extract **all three** attribute forms — `#[test]` + `#[tokio::test]` + `#[sqlx::test]` — because a bare `#[test]` grep undercounts. **Derive the shortfall at your own commit; do not quote one.** The figure this plan first carried was `16`, which was true when W8 Task 2 measured it and had decayed to **19** by `bf0782ea` as later tasks added async tests: **a count of what a wrong instrument misses is itself a moving figure, and a date is part of it.**
- **Never run two suites at once** — they share `egret_test`. Check with `ps -axo pid,ppid,args | awk -v me=$$ '$1!=me && $2!=me && /cargo/ && /test/' | wc -l`, and note **a `ps` pipeline containing its own search string matches itself**.
- **Never edit an already-applied migration.** A comment-only edit to `0142` gave every DB-gated binary `VersionMismatch(142)` and cost a task its canonical measurement. Migrations run to **0143**; the next free number is **0144**.
- **`advance()` is pure over `(plan, state, now)`** — no I/O, no clock. A change to its **contract** is a **finding**, not a step.
- **Workspace scoping is structural, not a handler's responsibility.** Every new endpoint takes `WsContext(ctx): WsContext` and calls `egret_core::authz::require(&*st.pdp, &ctx, WF_READ | WF_MANAGE)`, and every query filters on `ctx.workspace_id.0`. This is the `chart_id` class of bug: migration 0128 shipped a plain FK with no workspace predicate and the Cedar check that followed was told the **caller's** workspace, never the object's.
- **Over-acceptance must not ship**, and here it means: an endpoint that starts a flow the caller may not read, or lists executions across workspaces. **The oracle cannot catch any of it.**
- **A bank carries no "why", and neither does a green gate.** With no ratchet to lean on, every claim must be pinned by a test that **fails when the claim is false**. Build the failing test first and watch it fail.
- **A probe cannot locate what it detects.** When a mutation of a predicate survives, **enumerate every caller** before concluding anything about the predicate — W8's third shipped defect was a caller skipping a gate, first misdiagnosed as a predicate missing a clause.
- **Mutation discipline.** Every mutation **prints out-of-band evidence that it applied** (`git diff --stat`, an occurrence count of the mutated token) **before** its verdict is read — a mutation that silently fails to land prints `test result: ok. N passed; 0 failed`, **byte-identical to the control**, and one that fails to **compile** prints **nothing at all** where a result line goes, so grep for `^test result:`. Every verdict carries a **run count** and an **unmutated control**. A dying mutant is asked **which assertion killed it** and whether that assertion **predates** what it proves; the clean shape is a **dead-at-HEAD / alive-at-parent pair**.
- **A zero is unfalsifiable alone** — report every zero beside a number that can disagree, and every sweep as **N examined / M corrected**. A denominator equal to its numerator means the sweep only found what it went looking for.
- **A correction sweep has three populations:** the phrase **verbatim across the whole repository** (a companion once lived in a different package's plan file that never appeared in any diff); **ordinals and counts as their own population**, because an ordinal shares no letters with what it names; and **data that feeds a generator**. A phrase that **wraps a line break** is invisible to a line-oriented grep and fails **quietly**.
- **`rg -c` here returns `rtk: search failed … (os error 2)` with rc 1 for a present token *and* an absent one** — broken search and true zero agree in output **and** exit code. Use `/usr/bin/grep` with **file arguments** plus a positive control.
- **`cargo doc` link-checks public items only**; `executor/` is `pub(crate)`, so a dangling intra-doc link there needs `--document-private-items`.
- **Regenerate the frontend client with `npm run gen:api`, never bare `orval`** — bare `orval` strips `encodeURIComponent` from path params.
- **A wrong assertion is worse than none.** One shipped in W8 defending a divergence with a green test named for the wrong behaviour.
- Commits use `git -c user.email=hoangquocvietbk2.13@gmail.com`. **Commit before you deliberately break anything.** Never commit `.superpowers/`, a scratchpad, or `__pycache__`. **No PRs** — the controller opens them.

## File Structure

```
docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md
                                 §9 package table gains W10; §1's finding recorded
crates/egret-config/src/lib.rs   WorkflowConfig gains worker_enabled,
                                 tick_interval_secs, batch, working_timeout_s,
                                 reaper_interval_secs
crates/egret-workflow/src/
  runtime.rs                     NEW — spawn(pool, reg, cfg, cancel); the two
                                 tickers and the per-call safety statements
  repo.rs                        ExecutionRepo beside FlowRepo — reads only
  routes.rs                      execution endpoints + DTOs + utoipa paths
crates/egret-bin/src/main.rs     calls runtime::spawn, config-gated
crates/egret-workflow/tests/
  runtime.rs                     NEW — the loops' own tests
  executions_routes.rs           NEW — the API's own tests
  end_to_end.rs                  NEW — publish → start → terminal, no harness
frontend/src/api/generated/      regenerated by `npm run gen:api`
crates/egret-workflow/oracle-exec/README.md
                                 the coverage entry: the oracle cannot speak here
```

---

## Task 1: The corrections, before anything is built

**Files:** modify `docs/en/specs/2026-09-01-workflow-programme-design.md`; modify `docs/vi/specs/2026-09-01-workflow-programme-design.md`.

The package table (§9) assigns the runtime to **W3b** (merged, loop built as functions, nothing calling them) and assigns the execution API and `task_logs` to **nobody**.

- [ ] **Step 1: Re-derive the finding rather than quoting it.** Grep the §9 table for `task_logs|execution api|ExecutionController|wire|binary|daemon` and confirm **0**; confirm `ExecutionController` appears **exactly once** in the whole spec and that the occurrence is inside *"Not ported, but required reading before W7"*. **Report both counts beside a positive control** (a term that does appear in the table, e.g. `reaper`), or the zeros are unfalsifiable.
- [ ] **Step 2: Add a W10 row** to §9 in **both** languages, and amend W3b's row to say its claim loop shipped as **functions with no caller** — that is a correction of the record, not a criticism of W3b, and the coverage statement's item 15 already says it.
- [ ] **Step 3: Add a W11 row** for logs, carrying §5's measurement (no table; three `LogSink` impls, none persisting; container stores counts only) so the deferral is a decision with evidence rather than an omission.
- [ ] **Step 4: Sweep all three populations** — the phrase, the ordinals (`W0–W9`, "ten packages", "nine packages", any "the other N"), and any generated artefact that lists packages. Report **N examined / M corrected** per population.
- [ ] **Step 5: Confirm EN and VI carry the same sections** — compare numbered-heading sequences programmatically, not by eye. §1.1 once shipped in English only while the report said the spec was corrected.
- [ ] **Step 6: Commit.** No code, no gate change expected; state the gate and ratchet anyway with the figures above.

---

## Task 2: The runtime loops, and a safety statement per call

**Files:** create `crates/egret-workflow/src/runtime.rs`; modify `crates/egret-workflow/src/lib.rs`; modify `crates/egret-config/src/lib.rs`; modify `crates/egret-bin/src/main.rs`; create `crates/egret-workflow/tests/runtime.rs`.

**Interfaces:**
- Consumes: `worker::claim::tick(pool: &PgPool, n: usize, reg: &dyn Registry) -> Result<usize>`, `worker::reaper::reap(pool: &PgPool, timeout: Duration) -> Result<usize>`, `worker::registry::Handlers::with_core_tasks()`.
- Produces: `pub fn runtime::spawn(pool: PgPool, reg: Arc<dyn Registry>, cfg: RuntimeConfig, cancel: CancellationToken)`.

- [ ] **Step 1: Extend `WorkflowConfig`.** It currently has exactly one field, `webhooks_enabled: bool`, with `default_workflow()` returning `false`. Add `worker_enabled: bool` (**default `false`** — the same argument the webhook flag makes: a surface that has never run in production is opt-in), `tick_interval_secs: u64`, `batch: usize`, `reaper_interval_secs: u64`, `working_timeout_s: u64`. Mirror `AutomationConfig`'s serde defaults exactly; read it before writing.
- [ ] **Step 2: Write the failing tests first.** In `tests/runtime.rs`: that `spawn` with `worker_enabled: false` never calls the registry (inject a counting `Registry`); that a cancelled `CancellationToken` stops both tickers; and that a paused ticker does **not** fire catch-up ticks (`MissedTickBehavior::Skip`). Run them and watch each fail.
- [ ] **Step 3: Implement `runtime::spawn`,** copying `egret_automation::runner::spawn` — `tokio::spawn`, `tokio::time::interval`, `MissedTickBehavior::Skip`, `tokio::select!` on `cancel.cancelled()`. **Two independent tickers**, not one: the reaper's interval is not the worker's.
- [ ] **Step 4: Write the per-call safety statement, and prove it.** The claim's safety is already argued — `claim::claim` uses **`FOR UPDATE SKIP LOCKED`**, which is why `egret-automation` needs no leader election. **`wake`, `due_retries` and `reap` are separate statements and inherit nothing.** For each, state in the module doc why concurrent replicas are safe, and **write a test that runs two tickers against the same database and asserts the invariant each statement needs.** A guard nobody has seen fail is a guard nobody has seen.
- [ ] **Step 5: Do not loop until zero.** `tick` returns **results committed**, not work done — its doc says *"a tick that only woke something returns 0"*, deliberately, because a caller draining to zero would never stop on an execution whose retry it had just reopened. Assert this in a test with a name that says it, so the next person who "optimises" the loop gets a red.
- [ ] **Step 6: Wire it in `main.rs`,** gated by `config.workflow.worker_enabled`, beside the existing `if config.automation.enabled { egret_automation::runner::spawn(…) }` at ~line 1690. Register **`Handlers::with_core_tasks()` only** — `egret.container` and `egret.script.python` are a separate, deliberate decision (spec §3.3), and `egret.sql` needs an `Arc<dyn QueryExecPort>` neither constructor has.
- [ ] **Step 7: Mutation** — delete the `MissedTickBehavior::Skip` line, and separately flip the config gate's sense. Each with a run count, an unmutated control, and out-of-band evidence it applied.
- [ ] **Step 8: Full gate; ratchet before/after; commit.**

---

## Task 3: `ExecutionRepo` — the read layer

**Files:** modify `crates/egret-workflow/src/repo.rs`; create tests in the same file or `tests/execution_repo.rs`.

`repo.rs` has exactly one repo, `FlowRepo` (line 189), and **nothing reads `workflow.executions`**.

- [ ] **Step 1: Write the failing tests first** — list filters by workspace, by flow, by state and by time window; get-one returns its task runs ordered by `run_ix`; **and a cross-workspace test that inserts an execution in workspace B and asserts workspace A cannot see it by id.** That last is the `chart_id` class and must exist before the query does.
- [ ] **Step 2: Implement `ExecutionRepo`** with runtime `sqlx::query(...)` — **this workspace has zero compile-time macros and no `.sqlx` directory**; a `query!` would be the first and would need offline data. Every statement filters on `workspace_id`.
- [ ] **Step 3: Read, do not derive.** `executions.state` **is** `history.last()` — `claim::persist` computes both in one expression, one statement, one transaction (that is why migration 0142's CHECK on `state` is sound). So list pages read the column; do not open the jsonb to answer "what state is it in".
- [ ] **Step 4: Mutation** — drop the `workspace_id` predicate from each query in turn. Every one must die, and the killer must be the cross-workspace test, not a coincidence. Report which assertion killed each.
- [ ] **Step 5: Full gate; commit.**

---

## Task 4: The execution read API

**Files:** modify `crates/egret-workflow/src/routes.rs`; create `crates/egret-workflow/tests/executions_routes.rs`; regenerate the frontend client.

**Interfaces:** the nine flow endpoints already establish the shape — `State(st): State<WorkflowState>`, `WsContext(ctx): WsContext`, `egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?`, `EgretResult<Json<T>>`, and a `#[utoipa::path(...)]` block with `params(WorkspacePath)` and the five documented error statuses. `WF_READ` is `"workflow:read"` (routes.rs:73); `WF_MANAGE` is `"workflow:manage"` (:76).

- [ ] **Step 1: Write the failing tests first**, including **401 unauthenticated, 403 without `workflow:read`, and 404-not-403 for an execution in another workspace** — leaking existence is the same defect as leaking content.
- [ ] **Step 2: DTOs.** `ExecutionListItemDto` and `ExecutionDto` + `TaskRunDto`. `task_runs` has **seventeen** base columns plus **five** added (`outputs`, `message`, `created_seq`, `ended_seq`, `resume_with`); `executions` has **eleven** added. **Do not expose `source` in the list DTO** — it is the whole frozen YAML on every row.
- [ ] **Step 3: `GET /api/ws/{workspace}/workflows/executions`** with filters and pagination, and **`GET …/executions/{id}`** returning the execution with its task runs.
- [ ] **Step 4: Gantt data is already there — assert it rather than assuming.** `created_seq`/`ended_seq` are on `task_runs`, and with each run's `history` timestamps they are the whole input a gantt needs. **Write the test that proves a rendered gantt row can be computed from the DTO alone**, so W7 does not discover a missing field.
- [ ] **Step 5: Register the paths in the existing `utoipa::OpenApi` derive** in `routes.rs` — the list of `paths(...)` is right below `management_router`.
- [ ] **Step 6: `npm run gen:api`** — **never bare `orval`**, which strips `encodeURIComponent`. Confirm the new operations appear and that path params are encoded.
- [ ] **Step 7: Full gate; ratchet unchanged; commit.**

---

## Task 5: The execution write API

**Files:** modify `crates/egret-workflow/src/routes.rs`; extend `tests/executions_routes.rs`.

Today the **only** way to start an execution in production is the webhook, which is why the UI has nothing to press.

- [ ] **Step 1: Write the failing tests first** — start requires `WF_MANAGE`, not `WF_READ`; starting a flow in another workspace 404s; resume on a non-paused execution is a 409, not a silent success.
- [ ] **Step 2: `POST …/flows/{id}/executions`** — start, taking `inputs`. Reuse `worker::claim::start_execution`; do **not** write a second creation path. The webhook route uses `start_execution_with_trigger`; read `webhook_routes.rs:648` for the shape.
- [ ] **Step 3: `POST …/executions/{id}/resume`** — `worker::claim::resume(pool, execution_id)` already exists.
- [ ] **Step 4: kill and replay.** If either needs a capability `advance()` does not have, that is a **finding**: report it, do not widen `advance()`'s contract.
- [ ] **Step 5: Mutation per authorization arm** — swap `WF_MANAGE` for `WF_READ` on each write endpoint. Each must die on its own test.
- [ ] **Step 6: `npm run gen:api`; full gate; commit.**

---

## Task 6: The end-to-end proof — the package's headline

**Files:** create `crates/egret-workflow/tests/end_to_end.rs`.

**Nothing in this repository has ever run a workflow without a test driving the tick.** The oracle drives `worker::tick` itself; every other test calls the executor directly.

- [ ] **Step 1: Write the test that does not exist.** Start the runtime tickers with a short interval against a real database, publish a flow **through the management API**, start an execution **through the API**, and poll the read API until the execution reaches a terminal state. **No call to `tick` anywhere in the test body.**
- [ ] **Step 2: Give it a control that could disagree.** With `worker_enabled: false`, the same test must **time out with the execution still non-terminal**. Without that control the test passes on a harness that secretly drives the tick — which is exactly the failure `ADDED 0` names.
- [ ] **Step 3: Bound the flake.** The runtime is a clock and this branch has been bitten by wall-clock assertions before. Use a generous timeout with an explicit poll, and assert on **state**, never on elapsed time.
- [ ] **Step 4: Mutation** — delete the `runtime::spawn` call from the test's setup. It must fail, and the failure must name the non-terminal state rather than timing out anonymously.
- [ ] **Step 5: Full gate; commit.**

---

## Task 7: The coverage statement, and what the oracle cannot say

**Files:** modify `crates/egret-workflow/oracle-exec/README.md`.

- [ ] **Step 1: Add the W10 section** in the same voice as W6's trigger section: **the oracle cannot speak for this package at all**, because `driver.rs:884` calls `worker::tick` directly with its own registry, so it exercises the loop's body and is blind to the binary's wiring **by construction**. Say that wiring the worker moved **0** entries **and** that deleting the wiring would also move 0 — the second half is what makes the first falsifiable.
- [ ] **Step 2: Close item 15.** The coverage statement says *"nothing wires the worker into the binary … every durability property W3b and W4 prove is proved about code no deployment runs."* After Task 2 that is **false**, and it is the most consequential sentence in the file. Correct it, and **sweep for every other instance of the proposition** — the phrase, its ordinals, and `runnable.txt`, which is a pure function of `corpus.json` + `probe.json` and re-emits whatever those hold.
- [ ] **Step 3: Record what is still not covered** — `egret.container` and `egret.script.python` are registered nowhere; `egret.sql` is in neither registry; logs are W11 with the measurement.
- [ ] **Step 4: Sweep as N examined / M corrected per population**; full gate; ratchet **109**, md5 unchanged; commit.

---

## Final review

Dispatch the whole-branch review on the most capable model. Point it at **predicates and at the new authorization surface**, not at figures: W10 has no ratchet number to check, so the reviewer's instruments are the tests, the bytecode-free question *"what does this endpoint let a caller reach that it should not"*, and the concurrency statements in Task 2 Step 4. Assume this branch has one Critical of its own; the two likeliest seams are **a query that lost its workspace predicate** and **a ticker whose second call is not as safe as its first**.
