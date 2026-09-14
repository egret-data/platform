# W10 — Runtime and execution surface, and four deliverables no package owns

**Status:** design, awaiting review
**Depends on:** W3b (durable executor), W4 (runner), W8 (advanced semantics — merged)
**Followed by:** W7 (frontend), and it is blocked on this one
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md`

## 1. The measurement, first, and it is about the package table rather than the corpus

Every W package so far opened with a corpus measurement. This one opens with a
different kind, because the finding is not about flows — it is about **ownership**.

W7's row reads *"Workflows nav group; YAML editor; read-only topology; execution
gantt and logs"*. Two of those four cannot be built, and the reason is that four
things they need are **owned by no package in the table**. Three were in this
table when the spec was written; the fourth — the trigger loop — was found by W10
Task 1 while measuring the schema for the logs row, and it is listed here because
a finding about unowned deliverables that quietly omits one repeats the defect it
records:

| what W7 needs | where the programme spec designs it | which package owns building it |
|---|---|---|
| a process that advances executions | **§6.3**, *"worker loop … trigger loop … reclaim"* | **W3b** — *"claim loops; reaper"*. **Merged. The functions exist; nothing calls them.** |
| an execution REST surface | **nowhere** | **none** |
| `workflow.task_logs` + a sink that writes to it | **§6.1**'s schema list, and **§11** open question 1 | **none** |
| a loop that fires `cron:` and schedule triggers | **§6.3**, *"worker loop … **trigger loop** … reclaim"*, and **§6.1**'s `workflow.triggers` row | **none** — W6 shipped `trigger::tick` as a **pure function over injected state** and built no table for it |

**Machine-checked, not read:** grepping the whole package table (§9) for
`task_logs|execution api|ExecutionController|wire|binary|daemon` returns **0**.
`ExecutionController` appears exactly **once** in the programme spec, at line 352,
inside *"Not ported, but required reading before W7"* — it is named as a **shape to
study**, never as a deliverable.

**The fourth row is measured the same way, each zero beside a control.** No
migration anywhere creates `workflow.triggers` — schema `workflow` holds exactly
four tables, `flows`, `flow_versions`, `executions` and `task_runs` (control:
`workflow.executions` appears in **three** migration files, so the search reaches
migrations). `TriggerState::initialise` has **exactly one** caller and it is a
`#[cfg(test)]` helper in `trigger/mod.rs`; nothing outside
`crates/egret-workflow/src/trigger/` calls `trigger::tick`; and
`crates/egret-bin/src/main.rs` contains the string `trigger` **zero** times
(control: `egret_workflow` appears **5** times in the same file). **So a `cron:`
trigger cannot fire in production, and W10 as scoped does not change that** —
W10 wires the *worker* loop, which has a table to claim from; the trigger loop
has none.

**This is the mirror image of a defect W8 closed.** There, `allowFailure` was
assigned **twice** in two documents and the package had to say which owner was
wrong. Here four deliverables are assigned **zero** times. A row that names a
thing in the architecture and never in the plan reads, to anyone checking coverage,
exactly like a row that is done.

So W10 exists to **give all four an owner**. It builds the first two itself and
names the other two rather than absorbing them: **W11** is logs, **W12** is the
trigger loop, each recorded in §9 with its measurement as a decision rather than
an omission. W10's deliverable is *executions advance and are visible*; with it a
flow runs by manual API start or by webhook, both of which already work. Cron is
a separate capability needing its own table, its own claim and its own safety
argument, and folding it in now would duplicate the runtime's concurrency work
before that work exists. **The programme spec's §9 table is a deliverable of this
package, not a reference for it.**

## 2. What judges W10 — and the honest answer is *not the ratchet*

Every previous package reported a number: 143 → 127 → 109. **W10 will report no
ratchet movement at all, in either direction, and that is structural rather than
disappointing.**

`oracle-exec/driver.rs:884` calls `worker::tick(pool, BATCH, reg)` **directly**,
with **its own registry**. The oracle therefore already exercises the loop's
**body** on all 345 corpus flows — and is blind to the **binary's wiring** by
construction. Wiring the worker into `egret-bin` moves **zero** entries; deleting
that wiring afterwards would also move zero.

This is not a sixth blindness mechanism. It is the position `egret.sql`,
`egret.container` and W6's four triggers already occupy: **built, and judged by
their own tests, stated as such on page one** so a green ratchet is never read as
coverage it does not have. The discipline W3b's §3.1 established after the fact,
and W4 and W6 adopted before it, applies here to the **whole package**.

**So the instruments are:**

1. **The runtime's own tests**, plus one end-to-end proof that does not exist
   today: *a flow published through the API, started through the API, observed
   reaching a terminal state through the API, with no test harness driving the
   tick.* That single test is the package's headline, because **nothing in the
   repository has ever run a workflow without a test driving it**.
2. **The execution API's own tests**, in the shape `egret-dataapi`'s management
   router already uses.
3. **The correction in §1**, because a package table that names an owner for
   nothing is worse than one that names the wrong owner — the wrong owner gets
   argued with.

## 3. The runtime: everything is written, nothing is called

Measured, and this is the load-bearing paragraph:

- `worker::claim::tick(pool, n, &dyn Registry) -> Result<usize>` is **public**.
  So are `wake`, `due_retries`, `resume`, `start_execution`,
  `start_execution_with_trigger`, and `reaper::reap(pool, timeout) -> Result<usize>`.
- `Handlers::with_core_tasks()` takes **no arguments** and registers four
  handlers. `egret.sql` is in **neither** registry because it needs an
  `Arc<dyn QueryExecPort>`; the doc already records the one line that adds it.
- `main.rs` builds `egret_workflow::WorkflowState`, mounts `management_router()`
  and `webhook_router()`, and reads `config.workflow.webhooks_enabled`. **It
  spawns nothing.**

**The consequence, stated plainly because it is the reason this package exists:**
a webhook creates an execution row and **nothing ever advances it**. Until W10
ships, every durability property W3b, W4 and W8 proved is a property of code **no
deployment runs** — which is item 15 of `oracle-exec/README.md`'s coverage
statement, still true after three packages.

### 3.1 No leader election, and the argument is already written down

`claim::claim` uses **`FOR UPDATE SKIP LOCKED`** (`claim.rs`, the claim CTE). That
is the same property `egret-automation` relies on, and its `runner::spawn` doc
states the conclusion in one sentence: *"one periodic task per replica, no leader
election, with `SKIP LOCKED` in the claim making that safe."*

W10 copies that shape verbatim — `tokio::spawn`, `tokio::time::interval`,
**`MissedTickBehavior::Skip`** (so a paused replica does not fire a burst of
catch-up ticks), and a `CancellationToken` — gated by config the way
`config.automation.enabled` / `tick_interval_secs` gate the automation ticker.

**But `tick` is not the only thing the loop calls, and each one needs its own
sentence rather than inheriting the claim's.** `wake`, `due_retries` and `reap`
are separate statements against the same tables; **the package must state, per
call, why concurrent replicas are safe**, and prove it rather than assert it. A
guard nobody has seen fail is a guard nobody has seen.

### 3.2 `tick`'s return value is a trap for the obvious loop

`tick` returns **results committed**, not work done — its own doc says *"a tick
that only woke something returns 0"*, deliberately, because **a caller that looped
until zero would never stop on an execution whose retry it had just reopened.**

So the loop is a **fixed-interval ticker, not a drain-until-empty loop**, and the
spec says so where the next reader will look. This is exactly the class W8 spent
four rounds on: a value whose meaning is not what its name suggests.

### 3.3 Turning the worker on is the risk, and it is not a small one

This package makes the system **actually run for the first time**. Two specific
hazards, both measured rather than imagined:

- **`egret.container` and `egret.script.python` exist as handlers and are NOT in
  `with_core_tasks()`.** Registering them means running containers, in a
  workspace's name, on a schedule. That is the governance question W5's open
  question 3 already raises for plugin jars, arriving early. **W10 ships with the
  four core handlers and states the rest as a deliberate, separate decision.**
- **The dev database has no `workflow` schema at all** — measured on 2026-09-14:
  `pg_namespace` on the dev DB lists **fourteen** non-`pg_` schemas, and
  `workflow` is not among them (the same query returns `false` for its presence,
  which is the control on the count). **This feature has never run against the dev
  stack**, so *"it works locally"* is not yet a sentence anyone has been able to
  say.

## 4. The execution surface: the data is rich, the endpoints are zero

`workflow.executions` carries `state`, `history` (jsonb), the frozen `source`, and
**eleven** columns added across migrations 0135–0143 — `inputs`, `seq`,
`execution_trigger`, `trigger_execution_id`/`trigger_run_ix`,
`parent_execution_id`/`parent_run_ix`, `loop_index`/`loop_value`,
`attempt_number`, `origin_offset_ms`.

`workflow.task_runs` carries **seventeen** base columns — including `run_ix`,
`task_ix`, `task_id`, `type_id`, `parent_ix`, `rendered`, `state`, `history`,
`attempts`, `not_before_ms`, `claimed_at` — plus **five** added over the same
range: `outputs`, `message`, `created_seq`, `ended_seq`, `resume_with`.

**So a gantt needs no new column.** `created_seq`/`ended_seq` are on `task_runs`,
and together with the timestamps already in each run's `history` they are the
whole input. The reason there is no gantt
is not missing data — it is a missing **endpoint**.

W10 ships, in the shape `egret-dataapi::management_router` already uses for auth,
workspace scoping, feature gating and utoipa:

- **list executions**, filtered by flow, state and time window, paginated;
- **get one execution** with its task runs;
- **start an execution** — today the **only** way to start one in production is a
  webhook, which is why the UI has nothing to press;
- **resume** (`worker::claim::resume` already exists), **kill**, and **replay**.

**One thing to settle rather than assume:** `executions.source` is the frozen YAML,
and `build_plan` is pure over it, so a **topology** endpoint is a derivation, not a
new store. Whether the graph is served from an endpoint or re-parsed in TypeScript
is W7's question — but this spec records the measurement that makes it answerable:
re-parsing in TS **duplicates `spec::validate` and will drift**, and the two tables
that must agree (`KESTRA_FLOW_CONTROL` and `kind_for`) are guarded in Rust and
nowhere else.

## 5. Logs are the only greenfield, and they are a package-sized piece

Measured:

- **No `workflow.task_logs` table exists** — grepped every migration; the table is
  in the programme spec's §6.1 schema list and has never been created.
- **`LogSink` has three implementations in `src/` and none of them persists a line**
  (a fourth lives in a live-container test, `container_docker_live.rs:261` — the
  count is scoped on purpose, because an unscoped "three" is false across `crates/`):
  `TracingSink` (writes to `tracing`), a test `Recorder`, and `NameHintSink`.
- The container handler records **`stdOutLineCount` and `stdErrLineCount`** —
  counts, not content.

So a Logs panel needs a table, a persisting sink, a retention policy, and a
paginated read API. The programme spec's §11 open question 1 already fixes the v1
answer — *"log lines live in Postgres with a retention window in v1; object storage
is W9"* — and the retention shape to copy is the audit prune already in `main.rs`:
`scheduled_retention(days)` plus an hourly loop.

**This is the one place W10 should consider stopping.** Runtime and the execution
API unblock W7's gantt and topology; logs unblock only the logs panel, and cost a
table, a sink on every handler path, retention, and an API. **The spec's
recommendation is that logs are W10's third deliverable only if the package is
willing to be large; otherwise they are W11 and W7 ships without a logs panel,
stated rather than discovered.** That is a decision for the reviewer, and it is
written here as a decision rather than resolved silently.

## 6. Fidelity rules

Unchanged, with four that carry weight here:

- **Over-acceptance must not ship**, and here it has a new face: an execution API
  that starts a flow the caller may not read, or lists executions across
  workspaces. Every one of these endpoints is the `chart_id` class of bug waiting
  to happen — see the programme spec §6.2 and `egret-report-target-cross-workspace-leak`.
  **Workspace scoping is a structural requirement, not a handler's responsibility.**
- **A bank carries no "why", and neither does a green gate.** W10 has no ratchet to
  lean on at all, so every claim it makes must be pinned by a test that fails when
  the claim is false.
- **A probe cannot locate what it detects** — when a mutation of a predicate
  survives, enumerate every **caller** before concluding anything about the
  predicate. W8's third shipped defect was a caller skipping a gate, first
  diagnosed as a predicate missing a clause.
- **A zero is unfalsifiable alone.** This package will report several — zero
  ratchet movement, zero entries affected — and each must appear beside a number
  that could have disagreed.

## 7. Risks

1. **The ratchet cannot judge this package.** Three packages have trained everyone
   to read a number; W10 has none, and the risk is that its absence is read as
   "nothing changed" rather than "the instrument does not point here".
2. **Turning the worker on surfaces every interaction that has never run.** The
   properties are proved; the wiring is not. Expect the first real defects of the
   programme to be in startup, shutdown, config and back-pressure rather than in
   `advance()`.
3. **The execution API is a new authorization surface** on data that has never been
   exposed. It is also the first place a workspace boundary can leak in this
   subsystem.
4. **Logs will expand to fill the package** if they are not scoped or deferred
   explicitly (§5).
5. **W7 is blocked until at least §3 and §4 land**, so any schedule that has W7
   starting now is wrong by the length of this package.

## 8. Deliverables

- A worker ticker in `egret-bin`, config-gated, `MissedTickBehavior::Skip`,
  cancellation-aware, **with a written per-call safety statement for `tick`,
  `wake`, `due_retries` and `reap`** rather than one inherited from the claim.
- The reaper on its own interval, with `working_timeout_s` stated.
- An execution REST surface: list, get with task runs, start, resume, kill,
  replay — workspace-scoped structurally, feature-gated, utoipa-documented, and
  regenerated into the frontend client with `npm run gen:api` (**never bare
  `orval`**, which strips `encodeURIComponent`).
- **One end-to-end test that publishes, starts and observes a flow to a terminal
  state with no harness driving the tick** — the package's headline.
- `workflow.task_logs`, a persisting `LogSink`, retention and a read API —
  **or an explicit deferral to W11, recorded with the measurement** (§5).
- **The programme spec's §9 package table corrected**, so all four deliverables
  have an owner, and §1's finding recorded where the next reader of the table will
  see it.
- A coverage-statement entry saying, in the same voice as W6's trigger section,
  that **the oracle cannot speak for this package at all** and why.
