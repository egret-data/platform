# Workflow — Programme Design

Status: **spec, awaiting approval**
Date: 2026-09-01
Supersedes: the `Workflows` and `Pipelines` placeholder nav items (`App.tsx:206-207`, `Sidebar.tsx:50,54`)

## 1. Goal

Workflow becomes a first-class egret area with its own nav group, the way Data API
is — not a page inside another module. It orchestrates work across egret's own
substrate (Trino/StarRocks SQL, Iceberg tables, notebooks/Daft pods, Data API
endpoints, reports) and, later, across third-party systems.

Three decisions were taken with the user before this spec:

1. **Data orchestration first** (ELT), business integration later, on one engine
   and one task model.
2. **YAML is the source of truth**, edited in a schema-aware editor; the graph is
   read-only. Not a drag-drop canvas.
3. **The engine is written in Rust inside egret**, deliberately modelled on
   Kestra, rather than integrating Kestra, Airflow, or n8n as a service.

## 2. Why not integrate an existing engine

Measured, not assumed. All figures come from the checkouts in `data-portal/`.

| | Flow language | Workspace isolation | Licence | Ecosystem |
|---|---|---|---|---|
| Kestra | YAML — matches | namespace + an egret proxy; RBAC/tenancy are Enterprise | Apache-2.0 | medium; plugins are external JARs |
| Airflow | **Python — requires codegen** | none; or one deployment per workspace | Apache-2.0 | largest (82 top-level `providers/`, nested further) |
| n8n | canvas JSON — does not match | permission model sits behind `.ee` | **Sustainable Use — blocks us** | large, integration-oriented |

**Airflow.** `airflow-core/src/airflow/utils/file.py:110` accepts only
`path.suffix == ".py"` or a zip of Python, and
`dag_processing/processor.py:578` (`DagFileProcessorProcess extends
WatchedSubprocess`) forks a child to execute it. A YAML-first product would have
to generate Python, and a workspace's flow would become arbitrary Python
executed in a shared DAG processor. Airflow has no tenant concept.

**n8n.** `LICENSE.md`: files with `.ee` in the path are *not* licensed without a
commercial Enterprise Licence. 122 such files exist, including
`packages/cli/src/permissions.ee` and `sso.ee` — precisely the multi-tenancy
pieces egret would need. The Sustainable Use Licence also forbids offering the
software as a service to third parties, which is what egret does.

**Kestra** was the best of the three, and the analysis below keeps its design
while declining its runtime.

## 3. Spike W5 — findings (executed 2026-09-01)

The riskiest claim in this programme was "egret can run existing Kestra plugins
without embedding Kestra". The spike settled it empirically.

Setup: JDK 25 (Kestra 2.x is compiled to class-file version 69; Kestra 1.3.35 is
version 65 / Java 21), `io.kestra:core:2.0.0-rc13` resolved to 207 jars on the classpath from
Maven Central, and a ~120-line `ShimRunContext`.

**Result: a real, unmodified Kestra plugin ran outside Kestra, with no Micronaut.**

```
OUTPUT={"row":null,"rows":[{"one":1,"tag":"from-kestra-plugin","workspaces":1}],"size":1}
[host] RunContext methods touched: [logger, render(Property), render(String,Map), validate, metric]
```

That is `io.kestra.plugin.jdbc.postgresql.Query` (from the published 12.7 MB fat
jar) querying egret's dev Postgres, driven by a JSON task description.

Findings, in the order they were discovered:

1. **No Micronaut is required to resolve plugin types.** `PluginDeserializer`
   falls back to `DefaultPluginRegistry.getOrCreate()` when no `KestraContext`
   exists — a path Kestra maintains for its own unit tests.
   `PluginScanner` registered **76 core plugins in ~450 ms** from the classpath.
2. **The shim must extend `DefaultRunContext`, not `RunContext`.**
   `RunContextProperty`'s constructor casts to `DefaultRunContext` to read
   `getTask()`/`getTrigger()`. `DefaultRunContext` has a public no-arg
   constructor, and `setTask` is package-private — so the shim must be declared
   in package `io.kestra.core.runners`.
3. **Identity rendering is sufficient.** egret renders `{{ }}` with minijinja
   before dispatch; every `render*` on the shim returns its input unchanged, and
   both plugins tested behaved correctly.
4. **`storage()` is not needed for inline results.** With `fetchType: FETCH`
   nothing beyond logger/render/validate/metric was touched. With
   `fetchType: STORE` the plugin calls `workingDir()` then `storage()`.
   `workingDir()` is one line — Kestra's own `LocalWorkingDir(Path)` is public.
   `storage()` can reuse the public `InternalStorage(StorageContext,
   StorageInterface, NamespaceFactory)`, so the host implements only
   `StorageInterface` — **16 methods of a plain blob store**, not the 19-method
   `Storage` surface.
5. **Plugins ship as self-contained fat jars.** `plugin-jdbc-postgres-2.0.0.jar`
   is 12.7 MB / 10,608 entries and bundles the shared `plugin-jdbc` base *and*
   the PostgreSQL driver. The Maven coordinate `io.kestra.plugin:plugin-jdbc:2.0.0`
   does not exist on Central — plugins are not consumable as ordinary Maven
   dependencies, they are dropped on a classpath. That makes the host simpler,
   not harder.
6. **Cost: 1.61 s / 1.64 s / 1.68 s** total wall clock per process — JVM boot,
   plugin scan, deserialize, query, exit. Acceptable when amortised over data
   tasks, and it is paid only by flows that use a Kestra plugin.

### 3.1 The shape this implies

`kestra-plugin-host` is a container image (JVM + Kestra core + plugin jars). It
is driven through the **same `egret.container` contract as any other container
task** — image, command, env, working dir, output dir. It is not a new concept in
the engine, and there is no JVM inside `egret-server` and no JNI.

Why not JNI, stated once so it is not revisited: a plugin's entry point is
`run(RunContext)`, and `RunContext` is 41 abstract methods backed by collaborator
interfaces totalling ~75 more. Satisfying that in-process means either
bidirectional JNI or booting Micronaut inside `egret-server`; either way a plugin
OOM or `System.exit()` would kill SQL Lab, Data API serving, and every in-flight
request, and every long-running task would pin a `spawn_blocking` thread. A pod
gives isolation, k8s-native concurrency, and reuses machinery egret already has.

### 3.2 Known limits of the host

- `render()` is identity, so a plugin that *deliberately defers* rendering per
  loop iteration will not behave correctly.
- Plugins that emit dynamic sub-tasks (`dynamicWorkerResult`, e.g. `ForEachItem`)
  are not supported.

Both classes are flowable/dynamic tasks, which egret ports natively. Leaf
plugins — JDBC, object storage, Slack, Kafka, Databricks — are the target and
they work.

## 4. Expressions, and the Pebble oracle

Kestra does not own its template engine: `platform/build.gradle:105` pins
`io.pebbletemplates:pebble:4.1.2`. Kestra adds 1,251 lines of glue plus 37
filters (1,684 lines) and 40 functions (2,968 lines).

There is no usable Rust Pebble/Twig engine. `pebble-rust` is a Pebble
*smartwatch* SDK binding; on crates.io `twig` is 0.1.0 "work in progress" and
`oak-twig` is 0.0.11. So egret implements Kestra's expression language **on
minijinja**, which is already in the tree and whose `set_formatter` mechanism is
what makes `egret-dataapi`'s SQL rendering injection-proof by construction.

Syntax divergence is narrower than expected. Counting real usage in the Kestra
checkout: `is` tests (12) and `~` concatenation (1) exist in Jinja2 already; only
`??` (38) and the ternary `? :` (8) are missing, and both are handled by an
expression-level rewrite.

**Semantics are the real risk, and they are now measurable.** The spike built a
standalone Pebble engine from `io.kestra.core.runners.pebble.Extension` (public
no-arg constructor), with no Micronaut:

```
{{ missing ?? 'fallback' }}                      => fallback
{{ flag ? 'yes' : 'no' }}                        => yes
{{ name | slugify }}                             => egret-data-portal
{{ d | date("yyyy-MM-dd'T'HH:mm:ss", timeZone='Asia/Ho_Chi_Minh') }} => 2026-09-01T09:30:00
{{ json | jq('.a.b') }}                          => [42]
{{ [1,2,2,3] | distinct }}                       => [1, 2, 3]
```

The `jq` line is the point: it returns **a list**, not `42`. A hand-written Rust
filter would plausibly return the scalar, render something plausible, and never
fail a test. `date` uses Java `DateTimeFormatter` patterns with quoted literals,
not strftime.

Therefore **W2 ships a differential test**: a golden corpus of expressions
rendered by real Pebble in CI and diffed against the Rust implementation.
Fidelity stops being a review opinion and becomes a measurement. The oracle needs
only a JVM, Kestra core, and Pebble — a strict subset of the W5 host.

## 5. Port surface

| Kestra | LOC (main) | Disposition |
|---|---|---|
| `core/models` less dashboards/collectors/policies/stats/ui/settings | 14,842 | port |
| `core/runners` less pebble | 8,942 | port |
| `runners/pebble/{filters,functions}` | 4,652 | port onto minijinja + oracle |
| `runners/pebble` engine glue | 1,529 | replaced by minijinja |
| `executor/` | 5,649 | port |
| `plugin/core` less dashboard | 12,336 | port — built-in task types |
| `worker/` | 5,972 | partial |
| `core/scheduler` + `scheduler/` | 5,583 | partial; the claim loop already exists |
| `core/{services,utils,validations,serializers}` | 16,110 | selective |
| `core/{storages,plugins,http,repositories,metrics,secret,encryption,namespace,queues,lock,topologies}` | ~12,700 | selective |
| `webserver/` + `ui/` | 21,794 + 731 files | **not ported — required UX/API reference, see §8** |
| `jdbc/` + `queue-jdbc/` | 11,890 | dropped — Postgres is the queue |
| `cli/`, `core/{ai,mcp,docs,collectors,reporter,migration,tenant,security}` | ~6,600 | dropped |

Port target ≈ **86k lines of Java** → estimated **35–40k lines of Rust**. Models
compress hard (no Lombok, no builders, no `@Schema`); logic is close to 1:1.

For scale: `egret-dataapi` is 45,783 lines today. This is a second Data API.

## 6. Architecture

### 6.1 Schema

Mirrors the draft/version split of `dataapi.endpoints` / `dataapi.endpoint_versions`,
for the reason recorded in migration 0121: if execution read the draft, editing a
flow would change the behaviour of tonight's run and versioning would be decoration.

```
workflow.flows          draft: yaml source + parsed plan, state draft/live/disabled
workflow.flow_versions  frozen snapshot: yaml + normalised plan + RESOLVED references
workflow.executions     one run, carrying its own frozen copy of the DRAFT's yaml
                        source -- there is no flow_version_id column (see below)
workflow.task_runs      one attempt of one node: attempt, state, not_before, deadline_at, output
workflow.triggers       cron/webhook rows, claimable -- NOT BUILT: W12 (see the
                        table in section 9). W6 shipped the trigger as a pure
                        function over injected state and built no table;
                        `TriggerState` is constructed only in tests
workflow.task_logs      log lines, with retention_days as automation.schedules has
                        -- NOT BUILT: W11 (see the table in section 9)
```

**Correction (W10 Task 5, fix round 1): executions run the DRAFT, and there is
no `flow_version_id`.** The line above used to read *"one run, pinned to a
`flow_version_id`; never reads the draft"*. Both halves were false, and the
column half is checkable in one command: `flow_version_id` occurs **zero** times
across `migrations/` (positive control: `flow_key`, at `0134:32`). What
`workflow.executions` actually holds is `source` — a frozen copy of
`workflow.flows.source`, taken at start.

There are **three** places that start an execution and all three read the draft
row:

| site | reads |
|---|---|
| `worker::subflow::resolve` | `SELECT id, source FROM workflow.flows …` |
| `webhook_routes::handle` | `SELECT id, source, state FROM workflow.flows …` |
| `routes::start_flow_execution` (W10 Task 5) | `FlowRepo::get` → `flows.source` |

Nothing in any execution path touches `workflow.flow_versions`. The binding
reason is recorded at `worker::subflow::resolve` and is not being revisited
here: **a parent and its child must not disagree about which version of the
world they are in**, and a version-snapshot resolver would reach almost nothing
(W6 Task 1 filed the whole corpus through the product's own create-then-publish
path and measured **16 of 345** flows reaching a live version;
`driver::target_census` asserts that on every run). The cost is the one that
paragraph already states: an author editing a draft changes what a live webhook
URL — or a start button — runs, with no publish step in between.

**§6.2's rigidity is untouched, but by a different mechanism than the sentence
above claimed.** Publish still resolves every reference and still freezes the
resolved ids into the snapshot; what keeps a *draft* start from running against
an unresolved reference is not that the executor never holds one — it does —
but that the handler refuses it. Measured, and narrower than "a start can only
run reference-free flows":

* **Exactly one handler of the eight** in `worker/handlers/` parses a spec field
  as a UUID and refuses otherwise. `handlers::sql` answers
  *"`database` is … which is not a UUID. `publish` rewrites this field to the
  resolved connection id; an execution whose source still holds the author's
  name was started from unresolved source"* (`handlers/sql.rs:294`). So a flow
  with an `egret.sql` task whose `database:` is written by name **starts**, and
  then **that task fails** — the refusal is at the task, not at the route.
* **A `Subflow` reference is the opposite case and works fine**, which is why
  the general sentence would have been wrong: `subflow::resolve` resolves by
  `(namespace, flowId)` against the draft table at run time and never reads a
  rewritten id — and it closes the same two doors in SQL
  (`AND deleted_at IS NULL AND state <> 'disabled'`) that
  `routes::startable_flow` closes in Rust.

`a_start_runs_the_draft_even_when_a_published_version_exists`
(`tests/executions_routes.rs`) is what stops this correction going stale: it
publishes a version, edits the draft underneath it, and asserts the start runs
the draft. Before it, no fixture anywhere had a published version at all, so
nothing discriminated a draft read from a `flow_versions` read.

### 6.2 References are resolved at publish, not at run

A flow references egret objects — `database`, `notebook`, `dataset`, `chart`,
`secret`, Data API endpoint. Every one of those is another instance of the
`chart_id` class of bug: migration 0128 shipped a plain FK with no workspace
predicate, and the Cedar check that followed was told the *caller's* workspace,
never the object's.

So the rule is structural, not procedural: **every reference is resolved and
workspace-checked at `publish`, then frozen into the version snapshot. The
executor never holds an unresolved id.** Publish refuses the whole flow if any
reference is foreign, answering `NotFound` — indistinguishable from "does not
exist", as `DataApiEndpointPort::workspace_of` already requires, so the call
cannot probe other workspaces.

A cross-workspace flow becomes unpublishable rather than something each handler
must remember to block.

### 6.3 The executor state machine is a pure function

```rust
/// No I/O, no DB, no clock. Snapshot in, commands out.
fn advance(plan: &Plan, state: &ExecutionState, now: Instant) -> Vec<Command>
```

Kestra needs 69,179 lines of core tests largely because these cases are only
reachable through a running system: a `parallel` branch failing while a sibling
retries, a subflow killed mid-run, a worker dying with `WorkerJobLifecycle`,
concurrency-slot release, `ExecutionDelay`. As a pure function over a snapshot,
each becomes a unit test on a struct — no Postgres, no Trino, no pod.

The durable loop is thin and reuses the statement already proven in
`egret-automation/src/scheduler.rs`:

- **trigger loop** — claim due triggers, create executions and root task runs.
  **Not built: W12.** W6 shipped the trigger predicate as a pure function over
  injected state and no `workflow.triggers` table, so nothing claims here yet;
- **worker loop** — claim `task_runs` where `state='pending' AND not_before <= now()`
  with `FOR UPDATE SKIP LOCKED`, run the handler, and in the *same transaction*
  record the result, call `advance()`, and insert the successors. No third loop,
  and no window where a task is finished but the execution has not moved;
- **reclaim** — `working_timeout_s` as automation does. For pod-backed tasks the
  reclaim must kill the pod *before* retrying, or two pods will write at once.

Retry is a new attempt with `not_before = now + backoff`; timeout is
`deadline_at` swept by a reaper. Both fall out of the same claim predicate.

### 6.4 Task handlers

Named `TaskHandler`, deliberately *not* `TaskRunner`: in Kestra that name means a
shell-command executor (`TaskRunner.run(RunContext, TaskCommands, filesToDownload)`,
whose javadoc says "will be called by the script plugin"), and reusing it would
recreate the exact confusion this programme had to clear up.

```rust
#[async_trait]
pub trait TaskHandler: Send + Sync {
    fn type_id(&self) -> &'static str;
    /// At PUBLISH: validate, resolve every reference within the workspace, freeze.
    async fn resolve(&self, spec: &Value, cx: &PublishCx<'_>) -> Result<ResolvedSpec>;
    async fn run(&self, cx: &TaskCx<'_>) -> Result<Outcome>;
    async fn cancel(&self, cx: &TaskCx<'_>) -> Result<()>;
}
```

| `type` | Backed by |
|---|---|
| `egret.sql` | `QueryExecPort` (already fail-closed on workspace scoping) + the `bind_formatter` path |
| `egret.python` | `KernelLauncher`; needs a new `egret-kernel-api` port |
| `egret.container` | same launcher; image + command + input/output files — the `TaskCommands` contract |
| `kestra.<FQCN>` | `egret.container` running the `kestra-plugin-host` image |
| `egret.dataapi.call` | `DataApiEndpointPort` |
| `egret.dataset.refresh`, `egret.report.send` | `egret-automation` renderer + delivery |
| `core.http`, `core.slack` | `automation/deliver/*`; must block SSRF (host allow-list, no internal IPs) |
| `core.{sequential,parallel,if,switch,dag,loop,loopUntil,subflow}` | no handler — the state machine |

Handlers depend on port traits, never on concrete crates, exactly as
`egret-dataapi` holds `Arc<dyn QueryExecPort>` and does not know `egret-query`.

### 6.5 Crates

```
egret-workflow-api/   WorkflowPort::workspace_of
egret-workflow/
  models/     flows, tasks, executions, triggers, property   (no I/O)
  runners/    expression rendering, run variables            (no I/O)
  executor/   advance() — the pure state machine             (no I/O)
  worker/     claim loops, handlers, reaper                  (I/O)
  plugins/    handler registry
  repo.rs / routes.rs
```

The first three do not touch the database or the network. That boundary holds all
the semantic difficulty and is testable without standing anything up. It also
exists so this does not become another `dataapi/src/lifecycle.rs` — 12,091 lines
in one file.

## 7. Fidelity rules

1. **Pin Kestra twice, and record both.** Without a pin, "like Kestra" is a
   moving target.
   - *Source reference* — the checkout the port is read against:
     `data-portal/kestra` at `96893b589` (2.0.0-SNAPSHOT).
   - *Artifact reference* — the published version the plugin host and the Pebble
     oracle link against: `2.0.0-rc13`.
   These are not the same artefact and must be bumped together, deliberately.

   **They have already drifted in content, and the drift is not cosmetic.** W2's oracle found that
   `2.0.0-rc13` carries a `range` function (`BoundedRangeFunction`) that does not exist anywhere in
   checkout `96893b589` — confirmed by decompiling `Extension.class`. So the published artifact was not
   built from the pinned commit. The rule that follows: **the artifact is authoritative for behaviour**,
   because it is what the oracle can measure and what a user's Kestra actually runs; the checkout is
   authoritative for *reading* an implementation. Where a surface exists only in the artifact, record it
   and port it from measured behaviour or skip it with a stated reason — never quietly assume the two
   agree.
   Note that Kestra 2.x is compiled for Java 25 and 1.3.x for Java 21; the host
   image's JDK follows the artifact reference.
2. **Rust module names mirror the Java package names**: `models/`, `runners/`,
   `executor/`, `worker/`, `plugins/`.
3. **Type names are kept**: `Execution`, `TaskRun`, `WorkerTask`,
   `WorkerTaskResult`, `FlowableUtils`. `State` keeps all **17** variants —
   `CREATED, SUBMITTED, RUNNING, PAUSED, RESTARTED, KILLING, SUCCESS, WARNING,
   FAILED, KILLED, CANCELLED, QUEUED, RETRYING, RETRIED, SKIPPED, BREAKPOINT,
   RESUBMITTED` — including those v1 never produces. A missing variant is where
   semantics drift quietly, and `RESUBMITTED` proves the point: it is the last
   entry in the Java enum and therefore ends with `;` rather than `,`, so a
   line-oriented reading of `State.java` misses it while
   `Type.isTerminated()` still depends on it.
4. **`type:` accepts Kestra's full FQCN as the canonical form**
   (`io.kestra.plugin.core.log.Log`); short aliases are secondary. A Kestra flow
   pasted into egret runs, and looking a task up in Kestra's docs lands on the
   right page.
5. **Every ported file names its source on line one**:
   `// kestra: core/src/main/java/io/kestra/core/runners/FlowableUtils.java`.
   Cheap, and it is what keeps rules 1–4 from rotting.

## 8. Kestra's UI and webserver as reference

Not ported, but required reading before W7.

- `ui/src/components/executions/` — `Topology.vue`, `Gantt.vue`, `Logs.vue`,
  `Outputs.vue`, `Metrics.vue`, `TaskRunLine.vue`, `ExecutionProgress.vue`,
  `ChangeExecutionStatus.vue`, `ReplayWithInputs.vue`, `FilePreviewDrawer.vue`.
  This is the part Kestra does best and egret has no equivalent of.
- `ui/src/components/flows/` — `MultiPanelFlowEditorView.vue` (the hardest UX
  question in W7), `FlowRevisions.vue`, `FlowTriggers.vue`, `FlowConcurrency.vue`,
  `BackfillBanner.vue`, `FlowRun.vue`, `Curl.vue`, and `no-code/` for later.
- `webserver/.../controllers/api/` — endpoint shape: `FlowController`,
  `ExecutionController`, `LogController`, `TriggerController`, `KVController`,
  `NamespaceFileController`.

**Rule:** take the information architecture and the interactions; do not take the
visual language. egret has `docs/en/ui-conventions.md`, the canonical Explore
list-page scaffold, and OKLCH tokens, and Explore and Control Plane already use
different header/button idioms. This is the same discipline used for Metabase
parity: copy how it *works*, not how it *looks*.

Kestra ships a `docker-compose.yml`. Stand it up and look at it before designing
W7 — a screen inventory is not a UX review.

## 9. Packages

| Package | Contents | Depends on |
|---|---|---|
| **W0** Foundation & renames | crate skeleton mirroring Kestra module names; delete the two placeholders; `workflows` entitlement; rename `egret-orchestrator` → `egret-provisioner` (it currently means k8s engine provisioning and will collide with a workflow orchestrator) as a separate mechanical PR | — |
| **W1** Model & language | `models/{flows,tasks,executions,triggers,property}`; YAML parse/validate; draft/version tables; publish + reference resolution | W0 |
| **W2** Expressions | minijinja + 37 filters + 40 functions + `??`/`?:` rewrite + **the Pebble differential oracle** | W0 |
| **W3a** Executor state machine | `advance()`; `sequential/parallel/if/switch/dag/loop`; retry/timeout/`errors:`; the Kestra execution-trace oracle. No I/O | W1, W2 |
| **W3b** Durable executor | claim loops; reaper; `SpawnExecution`; handlers `core.log`, `core.debug.Return`, `core.execution.Fail`, `core.flow.Sleep`, `egret.sql`; the oracle re-pointed at the real driver. **Correction, 2026-09-14 (see the W10 spec §1): the claim loop and the reaper shipped as functions with no production caller.** A grep for `tick(`, `reap(` and `start_execution(` over every `crates/*/src` finds no call site, and `egret-bin` builds a `WorkflowState` — the flow repo and its refs — and no loop, so the only things that drive the loop are `oracle-exec/driver.rs` and this crate's own tests. That is a correction of the record rather than a criticism of W3b — it is already item 15 of `oracle-exec/README.md`'s coverage statement — and **W10 is the row that closes it** | W3a |
| **W4** Runner & container | the `RETRYING` waker, task-level `timeout:`, the kill path — the three holes W3b handed forward, **worth ~0 baselined entries, not the 20 this row first claimed** (that figure was a union of three declaration greps; see the W4 spec §1.1); then `TaskCommands`, `egret.container`, `egret.script.python`, which reach ZERO corpus entries and carry their own tests. **`egret.python` is NOT W4's** — §6.4 gives that name to the notebook `KernelLauncher`, and `TASK_TYPES` has held it since W1 | W3b |
| **W5** Kestra plugin host | the JVM image, `ShimRunContext`, `StorageInterface` bridge, `kestra.<FQCN>` dispatch, plugin jar resolution | W4 |
| **W6** Triggers & subflow | `subflow` + `CREATE_NEW_EXECUTION` replay (**13–15 baselined entries, one migration**); then cron, webhook, flow trigger and backfill — which reach **0** baselined entries and carry their own tests. Written before the corpus was measured: four of those five words are worth **zero**, and backfill has **no live declaration in 345 flows**. See the W6 spec §1. **Outcome, measured: the bankable half banked 16** — 12 `Subflow` + 4 replay, so the 13–15 estimate was low by one at the top end — **and the four trigger words banked 0, as predicted.** The mechanism of each zero is in `oracle-exec/README.md`, *The coverage statement* → the W6 section | W3b |
| **W7** Frontend | Workflows nav group; YAML editor; read-only topology; execution gantt and logs. **Blocked on W10**: the gantt and the topology need a process that advances executions and an execution REST surface, and neither existed when this row was written. **The logs panel needs W11** and is out of W7's first cut, stated rather than discovered | W1, W3b, W10 |
| **W8** Advanced semantics | `pause`/`resume` **with** `loopUntil` (one state, must not be split), `allowFailure`, and `sla:`'s two `EXECUTION_ASSERTION` entries — **18 baselined entries strict**, measured. **Concurrency limits reach 0 and `labels` reaches 0**; `WorkingDirectory` reaches 1, not the 4 its declarations suggest. The residual table's `concurrency:` → `CANCELLED` is a **misattribution** (`CANCELLED` is 2 `sla:` + 1 `Exit`, never a limit's) and the row is deleted, not lowered. Written before the corpus was measured. See the W8 spec §1 | W3b, W6 |
| **W9** Remaining Kestra infrastructure | KV store, namespace files, internal storage, outputs | W4 |
| **W10** Runtime & execution surface | the worker ticker in `egret-bin` — config-gated, `MissedTickBehavior::Skip`, cancellation-aware, with a written per-call safety statement for `tick`, `wake`, `due_retries` and `reap`; the reaper on its own interval with `working_timeout_s` stated; an execution REST surface (list, get with task runs, start, resume, kill, replay) — workspace-scoped structurally, feature-gated, utoipa-documented, regenerated into the frontend client with `npm run gen:api`; **one end-to-end test that publishes, starts and observes a flow to a terminal state with no harness driving the tick**; and **this table's own correction**, so the **four** deliverables W7 needs have an owner — two of them W10's own, and two named as W11 and W12. **The ratchet cannot judge this package in either direction**: `oracle-exec/driver.rs:884` calls `worker::tick` directly with its own registry, so the oracle already exercises the loop's body and is blind to the binary's wiring by construction — expect **109 before and 109 after**. See the W10 spec §1 | W3b, W4 |
| **W11** Execution logs | `workflow.task_logs`, a `LogSink` that persists, a retention window (§11 open question 1) and a paginated read API. **Deferred out of W10 with the measurement rather than by omission:** no `task_logs` table exists in any migration — schema `workflow` holds exactly four tables, `flows`, `flow_versions`, `executions` and `task_runs`; `LogSink` has **three** implementations under `crates/egret-workflow/src/` — `TracingSink` (writes to `tracing`), a `#[cfg(test)]` `Recorder` (an in-memory `Vec`) and `NameHintSink` (annotates and forwards) — and **none of the three persists a line**; and the container handler records `stdOutLineCount` and `stdErrLineCount`, **counts, not content**. Until this lands, **W7 ships without a logs panel** | W10 |
| **W12** Trigger loop | `workflow.triggers` — the claimable cron/webhook rows §6.1 already designs — the loop that claims and fires them, and **its own concurrency safety argument** rather than one inherited from the worker's claim. **Deferred out of W10 with the measurement, exactly as W11 was:** **no migration anywhere creates `workflow.triggers`** (control: `workflow.executions` appears in **three** migration files, so the search reaches migrations, and schema `workflow` holds exactly four tables); `trigger::tick` has **no caller outside `crates/egret-workflow/src/trigger/`**; `TriggerState::initialise` has **exactly one** caller and it is a `#[cfg(test)]` helper; and `crates/egret-bin/src/main.rs` contains the string `trigger` **zero** times (control: `egret_workflow` appears **5** times in that file). **So a `cron:` trigger cannot fire in production today, and W10 does not change that**: W10 wires the *worker* loop, which has a table to claim from, and the trigger loop has none. Until W12 lands a flow runs by **manual API start or by webhook**, both of which work — stated rather than discovered | W6, W10 |

**W10, W11 and W12 were added on 2026-09-14, after W7 turned out to be blocked, and the reason is worth keeping.** **Four** things W7's row needs — a process that advances executions, an execution REST surface, `workflow.task_logs`, and a loop that fires `cron:` triggers — are designed in §6.1 and §6.3 of this document and were owned by **no row of this table**. The fourth was found while measuring the schema for W11's row, not by reading the table, which is why it is stated separately below. Machine-checked rather than read: grepping this table **as it stood before the W10, W11 and W12 rows below** for `task_logs|execution api|ExecutionController|wire|binary|daemon` returned **0** while `reaper` returned 1 and `W3b` returned 6 in the same pass, and `ExecutionController` appears **exactly once** in this spec — in §8, *"Not ported, but required reading before W7"*, where it is named as a **shape to study**, never as a deliverable. A row that names a thing in the architecture and never in the plan reads, to anyone checking coverage, exactly like a row that is done. The full derivation is in `docs/{en,vi}/specs/2026-09-14-workflow-w10-runtime-and-execution-surface-design.md` §1.

**The trigger loop, the fourth, is W12 above.** That leaves **two names owned by no row of this table**, recorded here rather than left to be rediscovered: the `secret` and `http` expression functions, which `crates/egret-workflow/src/runners/functions.rs`'s `REFUSED` table assigns to W9 **only so its refusal message can name an owner**, by that file's own admission — *"`secret` and `http` are in no package's contents at all"*. **No owner is assigned to them here**, deliberately: reassigning a name a second time makes the message churn without making it true. They are flagged for whoever writes the next programme-level review, and they are the same shape as the four W10 §1 records.

Each package gets its own spec and plan, as the DBAPI-parity programme did.

## 10. Deliberately out of v1

`pause`/`resume`, backfill, `allowFailure`, `loopUntil`, concurrency limits, and
dynamic tasks are W8, not W3. They are where Kestra's test mass lives, and every
one is additive in the YAML, so deferring them does not strand a flow anyone has
already written.

## 11. Open questions

1. Log lines live in Postgres with a retention window in v1; object storage is
   W9. Confirm that is enough. **The Postgres half is W11** — §9's row carries
   the measurement showing nothing persists a log line today.
2. Publish-time reference resolution makes a published flow rigid: renaming a
   dataset does not re-point it. This is believed correct but is a real
   trade-off.
3. `kestra-plugin-host` needs a policy for which plugin jars are allowed and who
   may add them. Running arbitrary third-party JVM code in a workspace's name is
   a governance decision, not only an engineering one.
