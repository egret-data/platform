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
   `Storage` surface. *(2026-10-04, W9a Task 14: the bridge exists.
   `plugin-host`'s `EgretStorage` implements `StorageInterface` under Kestra's
   own `InternalStorage`; it reads the files egret staged into the working
   directory and writes under the task run's prefix, and egret uploads them
   after the task. The pod holds no storage credential. Namespace files (W9b)
   and the KV store (W9c) still exit 4. W9a spec §9.1.)* *(2026-10-07, W9b
   Task 11: namespace files no longer exit 4. `storage().namespace(ns)`
   answers, read-only, over the namespace files egret staged for the task,
   those a whole property value names; a write throws
   `UnsupportedOperationException` with the reason. Only the KV store (W9c)
   still exits 4. W9b spec §10.)* *(2026-10-08, W9c Task 8: W9c gives flows
   the KV store through the `io.kestra.plugin.core.kv` tasks and `kv()`, with
   **no** plugin bridge, because no catalog plugin calls `namespaceKv()`. A
   plugin's `namespaceKv()` still exits 4, and its message now points at the
   `core.kv` tasks instead of naming W9c. W9c spec §10.)*
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
workflow.triggers       one row per Schedule trigger (a webhook needs none);
                        the claim queue of the trigger loop -- built by W12
                        (migration 0156, `scheduler::sync`/`fire_due`), designed in
                        docs/{en,vi}/specs/2026-09-24-workflow-w12-trigger-loop-design.md
workflow.task_logs      log lines, one global retention window (not per schedule)
                        -- built by W11 (migration 0159, `logs::{LogRepo, PgLogSink}`),
                        designed in docs/{en,vi}/specs/2026-09-27-workflow-w11-execution-logs-design.md
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
world they are in**. The second reason this sentence used to carry — *"a
version-snapshot resolver would reach almost nothing (W6 Task 1 … measured 16 of
345 flows reaching a live version)"* — **was retired at W10 Task 6 fix round
1**: §8 rule 4 says `type:` accepts Kestra's FQCN as the canonical form, and
`spec::registry::TASK_TYPES` did not implement that rule, so publish refused
every pasted Kestra flow. With the four core FQCNs in the table, **248 of 345**
corpus flows reach a live version and 18 of the 20 literal `Subflow` targets are
live, so a snapshot resolver would now reach most of them. The decision stands
on the binding reason alone; `driver::target_census` asserts the new figures on
every run. The cost is the one that paragraph already states: an author editing
a draft changes what a live webhook URL — or a start button — runs, with no
publish step in between.

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
  rewritten id. ~~and it closes the same two doors in SQL
  (`AND deleted_at IS NULL AND state <> 'disabled'`) that
  `routes::startable_flow` closes in Rust.~~

  **That last clause was false and is corrected here (W10 Task 7 fix round 1,
  2026-09-15), after `24855ba7` corrected it in the code and not in this file.**
  `routes::startable_flow` closes **three** doors — the trash, the `state`
  column, and the YAML's own `disabled: true`, which it reads by calling
  `parse`. `subflow::resolve` closes **two**, and cannot close the third:
  `disabled:` is not a column, so no SQL predicate can reach it. The gap was
  live — an operator wrote `disabled: true`, `POST …/flows/{id}/executions`
  answered **409**, and a sibling flow ran that same flow as a `Subflow` target.
  The third door now lives in `worker::subflow::spawn_one`, immediately after
  `spec::parse`, and answers `resolve`'s miss message **verbatim** so that both
  spellings of "off" give an operator the same answer. **Neither of `resolve`'s
  two original doors had a test**: deleting each left `--test worker` at 59/0,
  against a workspace control that failed 58/1. All three have one now.

  **The rule this cost, stated where the next such sentence will be written:**
  a comment that asserts what a *different* function does needs its own test, in
  that function. A claim about your own code is checked by the tests around it;
  a claim about somebody else's is checked by nothing, and reads as a
  cross-reference rather than as an assertion.

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

*(W5b, 2026-09-30: `secret` is the one reference resolved at run, not at
publish. Freezing its value into the version snapshot would store it in
`flow_versions`, which W5b forbids. The rule's purpose holds by construction
instead: `secret('NAME')` is looked up only in the execution's own workspace,
no argument can name another (`namespace=` and `full=` are refused by name),
it resolves only on `kestra.*` tasks, and a missing secret fails the task
naming the secret, never a value. See the W5b design spec §6. W5b fix round 1
added who may write it: a flow whose source can call `secret()` is saved or
published only by a holder of `workflow:use_secrets`, seeded to `ws_admin`.)*

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
  **Shipped by W12** (`scheduler::fire_due`), as designed in
  `2026-09-24-workflow-w12-trigger-loop-design.md`: one trigger per transaction
  under `FOR UPDATE SKIP LOCKED`, the execution created in the same transaction
  that advances the schedule, no lease, and every due trigger evaluated once per
  pass -- behind its own config gate, `scheduler_enabled`, default `false`;
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
| `egret.container` | same launcher; image + command + input/output files — the `TaskCommands` contract. *(W5a, 2026-09-28: now an alias of Kestra's `io.kestra.plugin.scripts.shell.Commands`, beside `shell.Script`, `python.Commands`, `python.Script` and the alias `egret.script.python`. Where it runs is the `taskRunner:` property or `workflow.container_runner`: Docker locally, one Kubernetes pod per task-run attempt in production, never the notebook `KernelLauncher`. See the W5a design spec.)* |
| `kestra.<FQCN>` | `egret.container` running the `kestra-plugin-host` image *(W5b, built 2026-09-30. It has its own `kestra` handler rather than going through `egret.container`'s: the handler builds the same `TaskCommands` — image `workflow.plugin_host.image`, `exec java … EgretPluginHost run task.json` — and hands it to W5a's configured runner. `<FQCN>` must be a class in the catalog generated from `plugin-host/plugins.lock`; a bare FQCN is refused with the prefixed hint. See the W5b design spec.)* |
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
| **W5** Kestra plugin host — **split on 2026-09-28 (user decision)** into **W5a** *Containers in production*: Kestra's four script tasks (`io.kestra.plugin.scripts.{shell,python}.{Commands,Script}`) with `taskRunner:` (Docker, and Kubernetes under Kestra EE's FQCN), the `workflow_containers` feature (off by default), the `plugin_configurations` image allowlist, lease renewal, the Kubernetes runner and the chart's task namespace — `docs/{en,vi}/specs/2026-09-28-workflow-w5a-containers-in-production-design.md`; and **W5b** *Kestra plugin host*, riding on W5a: the JVM image, `ShimRunContext`, `StorageInterface` bridge, `kestra.<FQCN>` dispatch, plugin jar resolution and the plugin-jar allowlist. Storage- and secret-backed plugins wait for W9. *(W5b, 2026-09-30: shipped without the `StorageInterface` bridge — every storage call throws and the task fails naming W9 — and with `secret()`, which W5b brought forward: secret-backed plugins run now; storage-backed ones still wait for W9. See the W5b correction below.)* *(W9a, 2026-10-04: the bridge shipped and storage-backed plugins run — `Query`/`Queries` with `fetchType: STORE`, `postgresql.Batch`, `CopyIn`, `CopyOut`, `mysql.Batch`; see the W9a correction below.)* | W4 (W5a also W10, W11) |
| **W6** Triggers & subflow | `subflow` + `CREATE_NEW_EXECUTION` replay (**13–15 baselined entries, one migration**); then cron, webhook, flow trigger and backfill — which reach **0** baselined entries and carry their own tests. Written before the corpus was measured: four of those five words are worth **zero**, and backfill has **no live declaration in 345 flows**. See the W6 spec §1. **Outcome, measured: the bankable half banked 16** — 12 `Subflow` + 4 replay, so the 13–15 estimate was low by one at the top end — **and the four trigger words banked 0, as predicted.** The mechanism of each zero is in `oracle-exec/README.md`, *The coverage statement* → the W6 section | W3b |
| **W7** Frontend | Workflows nav group; YAML editor; read-only topology; execution gantt and logs. **Blocked on W10**: the gantt and the topology need a process that advances executions and an execution REST surface, and neither existed when this row was written. **The logs panel needs W11** and is out of W7's first cut, stated rather than discovered. *(2026-09-27: W11 has shipped the API the panel reads, `GET …/executions/{id}/logs`; the panel itself is still W7's and is not built.)* *(2026-10-10: **W7 is split into W7a–W7e**: W7a flows (the nav group, the flow list, the multi-panel flow editor, Revisions and Run), W7b executions (with the logs panel), W7c topology, W7d namespaces, W7e triggers. **W7a is done, 2026-10-10**: `docs/{en,vi}/specs/2026-10-10-workflow-w7a-flows-design.md`.)* *(2026-10-12: **W7b done**: the executions list, the execution detail with Overview, Gantt, Logs and Outputs, Resume and Replay, with the logs panel this row deferred to W11; `docs/{en,vi}/specs/2026-10-11-workflow-w7b-executions-design.md`, compared against a running Kestra in its §2 and §11. W7c–W7e remain.)* | W1, W3b, W10 |
| **W8** Advanced semantics | `pause`/`resume` **with** `loopUntil` (one state, must not be split), `allowFailure`, and `sla:`'s two `EXECUTION_ASSERTION` entries — **18 baselined entries strict**, measured. **Concurrency limits reach 0 and `labels` reaches 0**; `WorkingDirectory` reaches 1, not the 4 its declarations suggest. The residual table's `concurrency:` → `CANCELLED` is a **misattribution** (`CANCELLED` is 2 `sla:` + 1 `Exit`, never a limit's) and the row is deleted, not lowered. Written before the corpus was measured. See the W8 spec §1 | W3b, W6 |
| **W9** Remaining Kestra infrastructure — **split on 2026-10-01 (user decision)** into **W9a** *Internal storage* (`kestra://` files for executions, tasks, plugins and the file functions; `docs/{en,vi}/specs/2026-10-01-workflow-w9a-internal-storage-design.md`), **W9b** *Namespace files* and **W9c** *the KV store*. W9a shipped (see its correction below); W9b and W9c have not started *(2026-10-07: W9b shipped, `docs/{en,vi}/specs/2026-10-05-workflow-w9b-namespace-files-design.md`; see its correction below. W9c has not started)* *(2026-10-08: W9c shipped, `docs/{en,vi}/specs/2026-10-07-workflow-w9c-kv-store-design.md`; see its correction below)* | KV store, namespace files, internal storage, outputs | W4 (W9a also W5a, W5b, W10, W11) |
| **W10** Runtime & execution surface | the worker ticker in `egret-bin` — config-gated, `MissedTickBehavior::Skip`, cancellation-aware, with a written per-call safety statement for `tick`, `wake`, `due_retries` and `reap`; the reaper on its own interval with `working_timeout_s` stated; an execution REST surface (list, get with task runs, start, resume, kill, replay) — workspace-scoped structurally, feature-gated, utoipa-documented, regenerated into the frontend client with `npm run gen:api`; **one end-to-end test that publishes, starts and observes a flow to a terminal state with no harness driving the tick**; and **this table's own correction**, so the **four** deliverables W7 needs have an owner — two of them W10's own, and two named as W11 and W12. **The ratchet cannot judge this package in either direction**: `oracle-exec/driver.rs:884` calls `worker::tick` directly with its own registry, so the oracle already exercises the loop's body and is blind to the binary's wiring by construction — expect **109 before and 109 after**. See the W10 spec §1 | W3b, W4 |
| **W11** Execution logs | `workflow.task_logs`, a `LogSink` that persists, a retention window (§11 open question 1) and a paginated read API. **Deferred out of W10 with the measurement rather than by omission:** no `task_logs` table exists in any migration — schema `workflow` holds exactly four tables, `flows`, `flow_versions`, `executions` and `task_runs`; `LogSink` has **three** implementations under `crates/egret-workflow/src/` — `TracingSink` (writes to `tracing`), a `#[cfg(test)]` `Recorder` (an in-memory `Vec`) and `NameHintSink` (annotates and forwards) — and **none of the three persists a line**; and the container handler records `stdOutLineCount` and `stdErrLineCount`, **counts, not content**. Until this lands, **W7 ships without a logs panel** | W10 |
| **W12** Trigger loop | `workflow.triggers` — the claimable cron/webhook rows §6.1 already designs — the loop that claims and fires them, and **its own concurrency safety argument** rather than one inherited from the worker's claim. **Deferred out of W10 with the measurement, exactly as W11 was:** **no migration anywhere creates `workflow.triggers`** (control: `workflow.executions` appears in **three** migration files, so the search reaches migrations, and schema `workflow` holds exactly four tables); `trigger::tick` has **no caller outside `crates/egret-workflow/src/trigger/`**; `TriggerState::initialise` has **exactly one** caller and it is a `#[cfg(test)]` helper; and `crates/egret-bin/src/main.rs` contains the string `trigger` **zero** times (control: `egret_workflow` appears **5** times in that file). **So a `cron:` trigger could not fire in production at the time this row was written, and W10 did not change that**: W10 wired the *worker* loop, which has a table to claim from, and the trigger loop had none. Until W12 landed a flow ran by **manual API start or by webhook**, both of which work — stated rather than discovered **Designed 2026-09-24** in `docs/{en,vi}/specs/2026-09-24-workflow-w12-trigger-loop-design.md`, scope narrowed with the user to **schedule triggers only**, missed ticks recovered as Kestra's default `ALL`. The row said *"cron/webhook rows"*: a webhook is pushed by HTTP and needs no row, so the table holds schedules alone.

**Correction, 2026-09-25 (W12 Task 6): shipped, and every measurement above is retired rather than deleted, because it is the record of why the row existed.** Migration 0156 creates `workflow.triggers`; `scheduler::sync` (called from `FlowRepo`'s three writers that can change what fires — `create`, `update`, `soft_delete`; `set_live` is exempt, since it changes neither `source` nor `deleted_at` and `state` is read at fire time) keeps it in step with `flow.source`; `scheduler::fire_due` claims and fires due `Schedule` triggers, one per transaction under `FOR UPDATE OF t SKIP LOCKED`, the execution created in the same transaction that advances the schedule via a savepoint, with `workflow_executions_one_per_schedule_tick` (migration 0156) as the backstop a stale read cannot get past. It runs on its own ticker in `runtime::spawn`, gated by `EGRET_WORKFLOW__SCHEDULER_ENABLED` — config-gated and **default `false`**, exactly as `worker_enabled` was at W10, so a deployment that has not typed the flag behaves exactly as it did before W12. **A `cron:` trigger now CAN fire in production, once that flag is set**; the three call-site measurements above (`trigger::tick`'s caller count, `TriggerState::initialise`'s caller count, the string `trigger` in `main.rs`) are all superseded — `scheduler::sync` and `scheduler::fire_due` are new, non-test callers of both, and `runtime.rs` now reads `trigger` a nonzero number of times. The oracle's ratchet cannot see any of it, for the reason `oracle-exec/README.md`'s coverage statement gives in its own W12 section: **109 before, 109 after, and 109 with `fire_due` deleted.** | W6, W10 |
| **W13** Flow triggers | `trigger/flow.rs`'s window-free arm wired into the worker's settle transaction — **event-driven, so it needs no claim table** — and later the `dependsOn` window arm, which 20 of the corpus's 21 declarations take. **Named out of W12's scope on 2026-09-24** so it has an owner rather than an absence. Backfill endpoints, `lateMaximumDelay`, `stopAfter` and publishing `trigger.*` into the render context are listed in the W12 spec §7 and remain **unowned** **Designed 2026-09-25** in `docs/{en,vi}/specs/2026-09-25-workflow-w13-flow-triggers-design.md`, BOTH arms by the user's decision, delivered as **W13a** (listener index kept by `scheduler::sync`, an outbox written in `settle`, a processor on the scheduler ticker, the window-free arm) and **W13b** (`window` model, `TimeWindow` port, `flow_trigger_windows`, `dependsOn`). Asynchronous, not nested in `settle`: nesting is capped at `MAX_SPAWN_CHAIN = 8` by worker stack, where Kestra allows depth 100. | W12 |

**Correction, 2026-09-25 (W13a Task 5): W13a shipped; W13b is still pending.** Migration 0157 adds `workflow.flow_listeners` (kept in step by `scheduler::sync`, called from the same three `FlowRepo` writers W12's row names) and `workflow.flow_trigger_events`, the outbox `settle` writes to — one conditional `INSERT`, the only statement W13a adds to that hot path (spec §4.3). `flow_trigger::process_events` drains it on the scheduler ticker (spec §4.4), calling `trigger::flow::evaluate` for the first time in production: it renders `when` and `inputs` against the source's own context, fires within a per-listener savepoint so one broken listener does not lose the event for its siblings, retries a transient failure up to `MAX_ATTEMPTS` and gives up loudly past it, and — measured against the pinned artifact, not assumed — creates a **born-FAILED** execution when `inputs` fails to render, exactly as Kestra does, rather than refusing. The two bridges the pre-W13a code left unbuilt are both built now: a `PAUSED` source's outbox event carries a real `models::State::Paused` into `evaluate` (`settle`'s hook does not gate on terminality, because a Flow trigger's default `states` includes `PAUSED`), and a fired child's `trigger_execution_id` lets `worker::render::Surface::load`'s existing causal walk (fixed at W6 Task 8) derive `labels.system.correlationId` as the chain's root id instead of the child's own — closing the gap `trigger::flow`'s module doc used to describe as "owned by the wiring seam." The oracle's ratchet cannot see any of this, for the same structural reason W10 and W12 gave: `oracle-exec/driver.rs` never calls `flow_trigger::process_events`, so it is **109 mismatching, 109 baselined, 0 broken of 294 golden entries** before this task, at this commit, and with `process_events` given an unconditional early return — measured on this tree, all three, not carried over. **W13b — the `dependsOn`/window arm, 20 of the corpus's 21 declarations — has not started**: no `window` model, no `TimeWindow` port, no migration 0158, and the W13 spec's §7.2 table is still the plan for it, not a record of it. See `oracle-exec/README.md`'s W13a section for the sweep and the full derivation. | W6, W10, W12 |

**Correction, 2026-09-26 (W13b Task 5): W13b shipped, and the W13a correction's "has not started" is retired rather than deleted.** Migration 0158 adds `workflow.flow_trigger_windows`, one row per `dependsOn` listener, deleted by cascade with its listener (a deliberate divergence: Kestra keeps it). `models::Window` models the seven fields the pinned artifact accepts, `timezone` included, and refuses `fireOnce` at parse as the artifact does. The pure `trigger::window` ports the four `TimeWindow` kinds' bounds from W13b Task 1's measurements, DST and midnight included. The pure `trigger::depends_on` ports the condition keys (murmur3, reproducing the artifact's stored keys), `merge` and `satisfied` under ALL / ANY / AT_LEAST, and one refusal list that save and run time share. `flow_trigger::process` routes a `dependsOn` listener to its own arm, which takes the window row `FOR UPDATE` at REPEATABLE READ, replaces an expired window on the next matching event (user decision D1, a divergence from Kestra's drop), fires once the mode is met and deletes the window under every mode; a race lost with `40001` is retried without spending an attempt. The scheduler ticker sweeps expired windows once per pass. Of the corpus's 20 `dependsOn` declarations, **18 resolve**, 1 is refused at save (W1's namespace-only over-refusal, already in `executor::PUBLISHED_BUT_REFUSED`) and 1 does not parse (`fireOnce`); none declares a `window:`. The ratchet is **109 mismatching, 109 baselined, 0 broken of 294** before W13b, at its tip, and with the `dependsOn` branch deleted, all three measured on this branch. Every row of the W13 spec's §7.2 table has had its mutation run and killed. See `oracle-exec/README.md`'s W13b section.

**Correction, 2026-09-27 (W11 Task 6): W11 shipped, and its row's measurement is retired rather than deleted.** The row above records why logs left W10: no `task_logs` table, `LogSink`'s three implementations of which *"none of the three persists a line"*, the container handler storing *"counts, not content"*, and *"W7 ships without a logs panel"*. All four were true on `6749a37b` and are the record of the deferral; none is current. §6.1's `task_logs` line read *"NOT BUILT: W11"* until this correction. Migration 0159 adds `workflow.task_logs`. `LogSink` is async and `logs::PgLogSink` persists: a bounded channel of 10,000 lines that makes a writing handler **wait** rather than drop, a writer that inserts batches of 500 lines or every 1 s, retries a failing batch for 30 s and names the lost count in `tracing::error!` if it still fails, and drains on shutdown within 10 s. Every handler writes through a per-attempt `RunLog`, which splits a line longer than 15,360 characters as Kestra does, caps an attempt at 10,000 lines or 10 MiB and writes one WARN summary outside the cap. Container and python stdout/stderr are stored **line by line as content** (stdout INFO, stderr ERROR, measured on the pinned artifact). `RunLog::finish` is a flush barrier bounded at 30 s, so a run's lines are inserted before its outcome commits. A pruner deletes lines older than `workflow.log_retention_days` (default 30, `0` keeps them forever) hourly in batches of 10,000. `GET /api/ws/{workspace}/workflows/executions/{id}/logs` reads them by cursor, 2 s behind the writer, and reports `complete`. **What is still not true:** production's registry has no container or python handler, so no production run stores stdout or stderr today; there is no SSE; and the logs panel is W7's. The ratchet is **109 mismatching, 109 baselined, 0 broken of 294** before W11 and at its tip, both measured: the oracle compares traces, not log lines. See `oracle-exec/README.md`'s W11 section and the W11 design spec's §11 for what `complete` does and does not guarantee.

**Correction, 2026-09-28 (W5a Task 6): W5 is split, W5a shipped, and the W11 correction's *"production's registry has no container or python handler"* is retired for containers rather than deleted.** W5a follows Kestra's shape instead of growing `egret.container`: the four script task types under Kestra's FQCNs, `egret.container` and `egret.script.python` kept as aliases, and `taskRunner:` choosing Docker (`io.kestra.plugin.scripts.runner.docker.Docker`) or a Kubernetes pod per task-run attempt (`io.kestra.plugin.ee.kubernetes.runner.Kubernetes`, Enterprise-only in Kestra and implemented here under the same FQCN); `Process` is refused at save. `workflow.container_runner` (`none` | `docker` | `kubernetes`, default **`none`**) decides whether production registers them: with `none` the registry is exactly W11's, so a deployment that sets nothing still stores no script output. Running one also needs the workspace feature `workflow_containers`, the first feature that is **off by default**, and an image allowed by `plugin_configurations[type].values.allowed-images` (no entry or an empty list allows **no** image); one gate function checks both at publish and again at run, after rendering, and a refusal at run creates no pod or container. The worker renews its lease every `working_timeout / 3` under a fence, which closes the second cause of `oracle-exec/README.md` item 19 for a live worker, and a script task without `timeout:` runs under `container_default_timeout_s` (3600). Seven of fifteen publishable types still have no production handler. The ratchet is **109 mismatching, 109 baselined, 0 broken of 294** before W5a and at its tip, both measured: the corpus declares no script task. See `oracle-exec/README.md`'s W5a section. **W5b has not started.**

**Correction, 2026-09-30 (W5b Task 7): W5b shipped, and the W5a correction's *"W5b has not started"* is retired rather than deleted.** A task whose `type:` is `kestra.<FQCN>` runs a real, unmodified Kestra plugin: one JVM per task-run attempt, in the image `egret-plugin-host` (Java 25, `io.kestra:core:2.0.0-rc13`, a `ShimRunContext` in Kestra's own package), on W5a's configured runner — one pod per attempt on Kubernetes. The jars are exactly three, `io.kestra.plugin:plugin-jdbc-postgres`, `plugin-jdbc-trino` and `plugin-jdbc-mysql` at 2.0.0, named with their sha256 in `plugin-host/plugins.lock` and verified at image build; `catalog.json` is generated from them (five task classes; the natively ported types and the four classes that need internal storage are left out) and publish checks types and properties against it with no JVM. A bare plugin FQCN is refused with the prefixed spelling as a hint. The workspace feature `workflow_kestra_plugins` is off by default and requires `workflow_containers`; `plugin_configurations`' `kestra` entry narrows the catalog with `allowed-types` globs (no entry allows the whole catalog, an empty list allows none); one gate runs at publish and again at run after rendering, and a refusal at run creates no pod. `secret('NAME')` and `secret('NAME', subkey=…)` resolve at run, in the execution's workspace, on `kestra.*` tasks only (§6.2's note); a plugin task's properties render once, as Kestra's default does; every resolved value is replaced by `******` in the run's log lines, and outputs are not masked, as in Kestra. `pluginDefaults:` is refused at publish on every flow, as Kestra 2.0 refuses it. A timeout stops the host with SIGTERM and a grace of `workflow.plugin_host.kill_grace_s` (default 15 s, minimum 3), in which the host calls the plugin's `kill()`: a Postgres query is cancelled on the server (measured live on Docker and on a kind cluster), a StarRocks query is not (W5b spec §12). **What is still not true:** a plugin that needs internal storage, KV or namespace files fails by name and waits for W9 *(retired for internal storage by the W9a correction below, 2026-10-04: KV still waits for W9c and namespace files for W9b; retired for namespace files by the W9b correction below, 2026-10-07: only KV still waits, for W9c; retired for KV by the W9c correction below, 2026-10-08: W9c shipped the KV store with **no** plugin bridge, so a plugin that calls `namespaceKv()` still fails by name, now pointing at the `core.kv` tasks, and waits for no package)*; plugin triggers and jars beyond the three are out of scope. The ratchet is **109 mismatching, 109 baselined, 0 broken of 294** before W5b and at its tip, both measured: the corpus declares no plugin task. See `oracle-exec/README.md`'s W5b section.

**Correction, 2026-10-04 (W9a Task 14): W9 is split, W9a shipped, and the W5b correction's *"a plugin that needs internal storage … waits for W9"* is retired for internal storage rather than deleted.** The user split W9 on 2026-10-01 into **W9a** internal storage, **W9b** namespace files and **W9c** the KV store. W9a gives executions Kestra's `kestra:///{namespace}/{flow}/executions/{exec}/…` files: `egret-storage` streams (multipart above 8 MiB; no file is held whole in egret's memory), the object store is the `[storage]` section's bucket under `workflow.storage.prefix` (`workflow/`) and a workspace's id (the URI carries none), and `workflow.execution_files` (migration 0166) indexes every committed file. **One function, `storage::resolve_readable_many`, decides whether a URI is readable**: a running flow reads its own execution's files, its loop family's, its trigger execution's and that trigger's loop family's, and a replay its source chain's — never another workspace's and never an unrelated execution's. Files are looked up in a snapshot taken before the render (the user's ruling): a URI assembled at render is an error naming it. On top of that: `FILE` inputs, uploaded by a multipart start; the file functions `read`, `fileSize`, `fileExists` and `isFileEmpty`; the execution file API (download, metadata, a preview of CSV/JSONL/Ion capped at 5000 rows); script tasks' `inputFiles` take URIs, staged by egret into the working directory; `outputFiles` and `outputDirectory` are collected into URIs; **the plugin storage bridge** — **egret moves files for the pod, which holds no storage credential and opens no connection to the store**; `egret.sql` with `fetch: STORE`; nine core storage tasks (`Write`, `Size`, `Delete`, `PurgeCurrentExecutionFiles`, `Concat`, `Split`, `Reverse`, `DeduplicateItems`, `FilterItems`); `Loop` (the programme's `ForEach`, `io.kestra.plugin.core.flow.Loop`) over a file's values (migration 0167); and a retention pruner of its own, `workflow.storage_retention_days`, 30 days by default, `0` keeps forever, which keeps a source while a dependent runs (migration 0168). Caps: `max_file_bytes` 1 GiB, `max_task_bytes` 5 GiB, `max_read_bytes` 10 MiB.

**Breaking, and the release note for it.** (1) **`outputs.<task>.outputFiles` changed from contents to URIs**: W5a stored each declared file's contents (UTF-8, at most 1 MiB) in the outputs; W9a stores `{relative path → kestra:// URI}`, as Kestra does. A flow that read `{{ outputs.t.outputFiles['out.txt'] }}` as text now reads `{{ read(outputs.t.outputFiles['out.txt']) }}`, or passes the URI on to a later task's `inputFiles`. It affects only script tasks, which run behind the workspace feature `workflow_containers`, **off by default**. (2) A container runner (`workflow.container_runner` `docker` or `kubernetes`) needs `[storage]`: the server refuses to start with one and without the other. (3) The task-pod helper image (`egret-task-helper`) needs `cat` as well as `sh`, `tar`, `touch`, `wc`, `sleep` and `tail`; Alpine's base has it, and the Dockerfile checks. (4) W5a's other behaviours that changed: the 1 MiB-per-file and UTF-8 rules on output files are gone (the caps above apply instead); a symlink, hard link or directory in the collected outputs fails the task by name; `outputDirectory: true` is implemented (`$OUTPUT_DIR`, `{{ outputDir }}`); staged files carry the upload's modification time and are owned by uid 0 in the archive (W5a's were dated 1970). (5) W5b's that changed: `storage()` works instead of exiting 4 naming W9; the catalog gains the four classes that needed storage (`postgresql.Batch`, `CopyIn`, `CopyOut`, `mysql.Batch`) and no property value is refused at publish for needing storage; namespace files exit 4 naming W9b *(retired by the W9b correction below, 2026-10-07)* and `namespaceKv()` W9c *(retired by the W9c correction below, 2026-10-08: it still exits 4, now pointing at the `core.kv` tasks)*. (6) **What an operator sees that is new**: `workflow.storage_retention_days` defaults to 30 (`0` keeps files for ever), and a pruned file answers 404 from the file API and fails `read()` and the other file functions naming the window; the file API (`file`, `file/metas`, `file/preview`) needs `workflow:read` and adds no permission; `[workflow.storage]` `prefix`, `max_file_bytes`, `max_task_bytes` and `max_read_bytes` are new config keys; a manual replay is refused (400) once a `FILE` input it copies was pruned; the plugin pod's default 512Mi memory limit sits over the JVM's `-Xmx384m` and 128Mi is OOM-killed; migrations 0166-0168 (a dev database that applied the old 0161/0169/0170, or the new 166-168 before commit 40455dc4 added comments to their headers — sqlx hashes comments, so `VersionMismatch` — must reset with `just reset-testdata`, or delete `(166,'workflow execution files')`, `(167,'workflow loop file values')`, `(168,'workflow execution files pruned')` from `_sqlx_migrations` and drop what they created, and drop `egret_test_template`; recipe in `oracle-exec/README.md`; no migration file is touched after merge).

**What is still not true:** namespace files (W9b) *(shipped 2026-10-07; see the W9b correction below)* and the KV store (W9c) *(shipped 2026-10-08; see the W9c correction below)*; a `Loop`'s own `outputs:` in STORE mode (`valids/loop-outputs-auto` stays baselined, the user's ruling of 2026-10-03); object storage for **log lines**, which stay in Postgres (§11 question 1; the W9a spec §14); and `http`, refused on purpose since 2026-10-17 because it would run in the control plane. The ratchet is **109 mismatching before W9a and 108 after**, both measured: `valids/loop-values-from-uri` banked (a `Loop` over a `Write` URI), measured at W9a Task 11. See `oracle-exec/README.md`'s W9a section, which also records the end to end on a temporary kind cluster.

**Correction, 2026-10-07 (W9b Task 11): W9b shipped, and the W9a correction's *"What is still not true: namespace files (W9b)"* and its release note's *"namespace files exit 4 naming W9b"* are retired rather than deleted.** W9b gives a workspace Kestra's namespace files (`docs/{en,vi}/specs/2026-10-05-workflow-w9b-namespace-files-design.md`): authored files under a namespace, kept **with revisions, as Kestra keeps them** (revision 1 at `path`, revision N at `path.vN`; a delete is soft; `PurgeFiles` hard-purges), indexed by `workflow.namespace_files` (migration 0174) and stored in W9a's bucket at `{workflow.storage.prefix}{workspace}/{ns-as-path}/_files{path}[.vN]`. A flow names them `kestra:///{ns-as-path}/_files{path}[.vN]` (no workspace in it, as W9a's URIs), `nsfile:///path`, `nsfile://other.ns/path` or a bare path. **One function, `storage::nsfiles::access::authorize`, decides every read, write and purge**, and every store function takes the `Grant` it returns, so an unauthorised call does not type-check: a running flow reads the files of any namespace of its workspace, writes only its own namespace, and purges its own and its child namespaces (on a dot boundary); the workspace is always the execution's (or the API route's `{ws}` after the permission check), never parsed out of a URI. On top of that: `fileURI` (it leaves the refused list), `read`, `fileSize`, `fileExists` and `isFileEmpty` over namespace files, looked up at render through the pool, since namespace files commit outside `settle`; `namespaceFiles` on the four script tasks (`include`/`exclude` as Kestra's `PathMatcherPredicate`, `namespaces`, `folderPerNamespace`, `ifExists`), staged before `inputFiles`, namespaces in list order; `inputFiles` values that are namespace-file URIs; the four `io.kestra.plugin.core.namespace` tasks `UploadFiles`, `DownloadFiles`, `DeleteFiles` and `PurgeFiles`, as native handlers in the worker; the namespace file API under `/api/ws/{ws}/workflows/namespaces/{ns}/files`; and the plugin bridge's `storage().namespace(ns)`, read-only over the files egret staged for the task, so **the pod still holds no storage credential** (measured on a kind cluster, the bridge's pod included). Revisions are taken under a Postgres advisory lock per path, and a body is **staged** first: it streams to a staging object while holding no connection and no lock, and one short transaction copies it onto its revision's key server-side.

**The user's rulings** (each dated in the W9b spec's header): revisions as Kestra's; a flow reads any namespace of its workspace and writes only its own; reading needs `workflow:read`, writing `workflow:manage`, no new permission; **`WorkingDirectory` stays out** (its `namespaceFiles` is validated at publish and kept in the model, never run); `inputFiles` always overwrite a same-named namespace file, whatever `ifExists` says; a namespace with a `_files` segment is refused everywhere; a namespace-file URI is accepted as an `inputFiles` value; **R1** soft-deleted history stays visible (`revisions` lists it flagged, an explicit-revision read succeeds, only a latest read is not-found); **R2** `PurgeFiles.namespacePattern` is Kestra's wildcard; **R3** `fileURI` of a directory is an error, search is Kestra's, no GET route creates anything; `_files` marks a namespace file only before the first `executions/{uuid}`, and a flow id `_files` is refused; a path is at most 512 bytes of UTF-8; a move onto any history is refused and a write never purges history implicitly; names holding glob characters stage as Kestra's, the four list/enum properties of `namespaceFiles` take expressions, a task's own script file wins over a namespace file; `PurgeFiles` with neither `keepAmount` nor `before` is refused and `deleteParentFolder` is Kestra's; a body is staged before any lock; the glob budget is 256 KiB per list; the export reads each file twice (a CRC pre-pass) so that Java's `ZipInputStream`, which Kestra's import uses, reads it, with an exact `Content-Length`.

**The release note.** (1) **A new config key**, `workflow.storage.max_namespace_files` (env `EGRET_WORKFLOW__STORAGE__MAX_NAMESPACE_FILES`), default **10 000**, at least 1: the most rows one listing, search, staging set, upload or purge page sees, counted on what egret fetched; crossing it fails the task or answers 413, naming the cap. And `workflow.storage.prefix` longer than **200 bytes** is now refused at boot, so a namespace file's key stays under S3's 1024 bytes. (2) **Migration 0174** creates `workflow.namespace_files` and `workflow.namespace_file_staging`. It was written as 0169 and renumbered before merge, at the rebase onto `edccf6c4`, whose main owns 0169–0173; a dev database that applied the branch's `(169, 'workflow namespace files')` resets with `just reset-testdata`, or deletes that row from `_sqlx_migrations`, drops the two tables and drops `egret_test_template` (recipe in `oracle-exec/README.md`). No migration file is touched after merge. (3) **The staging table and its sweep.** Every write's body is first uploaded to `{prefix}{ws}/_nsfiles_staging/{uuid}`, with a row in `workflow.namespace_file_staging`; the write deletes both on every way out, and an **hourly sweep**, run by every egret process that has a store, deletes each staging object whose row is older than **24 hours** (the database's clock), then the row. **Bounds:** an API upload whose body stalls for 30 seconds is refused (408), and so is one that takes more than **12 hours** in all; an export's response body ends with an error after **12 hours**, short of its `Content-Length`; an export reads at most `max_task_bytes` in total (413 before any byte). A namespace file is capped at `max_file_bytes` and, whatever that is configured to, at **5 GiB**, one S3 CopyObject's limit: refused, not copied in parts. *(2026-10-07, the final review's I-1.)* A **move** copies each revision server-side, never through egret, and copies at most `max_task_bytes` and `max_namespace_files` rows (413 before any copy, naming the cap and the count). An API upload, create directory, move or delete whose path a move or another write holds waits at most **5 s**, then answers **409** "the path is busy … try again"; a flow's `UploadFiles`, `DeleteFiles` and `PurgeFiles` keep waiting. (4) **Publish now refuses a namespace with a `_files` segment**, and a flow id `_files`, naming it. W9b Task 1 counted 0 of the corpus's 345 flows and 0 of the 5 published flows it could find on the dev Postgres (38 databases), so no known flow is affected; a namespace with an empty segment (`a..b`) still publishes but cannot use namespace files. (5) **`namespaceFiles` publishes on script tasks** (W5a refused it at save), and an `inputFiles` value may be a namespace-file URI. (6) **`WorkingDirectory.namespaceFiles` is validated at publish**: a malformed block, which egret silently dropped before, now fails to publish naming the property; nothing changes at run. (7) **Nine API routes plus the export**, under `/api/ws/{ws}/workflows/namespaces/{ns}/files`: download, `stats`, `revisions`, `search`, `directory` and `export` need **`workflow:read`**; upload, create directory, move and delete need **`workflow:manage`**; no new permission. (8) **The plugin host no longer exits 4 on `storage().namespace(..)`**; a plugin's namespace write throws `UnsupportedOperationException` with the reason, and `namespaceKv()` still exits 4 naming W9c *(2026-10-08, W9c: it still exits 4, now pointing at the `core.kv` tasks; see the W9c correction below)*. (9) **Known limits** (W9b spec §15): a flow writes only its own namespace, and `PurgeFiles` defaults to it, where Kestra's flows write anywhere; a missing object behind a row is an error, not a fallback to an older revision; `read` of a binary file is an error; `UploadFiles.files` globs the execution's files, not a working directory; no retention for namespace files; `kestra://` URIs are not percent-encoded, so a name holding a space breaks a plugin that parses it with `new URI` (`CopyIn`), a follow-up; `filePattern` is matched against the path without its leading `/`, as Kestra's, so `"/up/*"` purges nothing; a plugin reads only namespace files a whole property value names; namespace files outlive their flows (there is no namespace table).

**What is still not true:** ~~running `WorkingDirectory` (and so its `namespaceFiles`, `inputFiles`, `outputFiles`, `cache`)~~ *(retired 2026-10-17; see the WorkingDirectory correction below)*; plugin writes to namespace files; the KV store (W9c) *(shipped 2026-10-08; see the W9c correction below)*; `.zip` expansion and `_flows` import on upload; the UI (W7). The ratchet is **108 mismatching before W9b and 108 after** (294 golden entries, 0 broken), both measured: no golden entry touches namespace files, and the two `WorkingDirectory` corpus flows that declare `namespaceFiles` stand outside the ratchet, because the artifact refused both at publish. The end to end on a temporary kind cluster passed 7 of 7 scenarios. See `oracle-exec/README.md`'s W9b section.

**Correction, 2026-10-08 (W9c Task 8): W9c shipped, and the *"the KV store (W9c)"* items of the W9a and W9b corrections' *"What is still not true"*, the W9b release note's *"`namespaceKv()` still exits 4 naming W9c"* and §3 finding 4's *"Only the KV store (W9c) still exits 4"* are retired rather than deleted.** W9c gives a workspace Kestra's KV store (`docs/{en,vi}/specs/2026-10-07-workflow-w9c-kv-store-design.md`): typed values (STRING, NUMBER, BOOLEAN, DATETIME, DATE, DURATION, JSON) under a key per namespace, **with revisions, as Kestra keeps them** (each revision an Ion text object at `{workflow.storage.prefix}{workspace}/{ns-as-path}/_kv/{key}.ion`, or `….ion.vN` for revision N > 1; a delete is soft), indexed by `workflow.kv_entries` (migration 0175, one row per revision, the type **stored**), with a TTL under which **expired means missing** and **reads never write**. A flow reads it through the `kv()` function (it leaves the refused list, which is six names now) and the six `io.kestra.plugin.core.kv` tasks, `Set`, `Get`, `GetKeys`, `Delete`, the deep-merge `Put` and `PurgeKV`, as native handlers in the worker; `kv()` with no namespace and a `Get` of the flow's own namespace walk up the parent namespaces on a dot boundary, as Kestra's do. **The same access function as W9b, `storage::nsfiles::access::authorize`, decides every KV read, write and purge**, now with two resources, namespace files and the KV store (`Resource::Kv`); every KV store function takes the `Grant` it returns and requires its resource, so a namespace-file grant cannot reach KV and a read grant cannot write: a running flow reads the KV of any namespace of its workspace, writes and deletes only its own namespace's, and `PurgeKV` acts on its own namespace and its children (on a dot boundary); the workspace is always the execution's, or the API route's `{ws}` after the permission check. A write reuses W9b's staging: the value is staged while no connection or lock is held, then one short transaction under the key's advisory lock checks `overwrite`, picks the revision, copies server-side and inserts the row; `Put` reads, merges and writes under that lock. The KV API lives under `/api/ws/{ws}/workflows/namespaces/{ns}/kv`. The plugin host gets **no** KV bridge.

**The user's rulings** (each dated in the W9c spec's header, 2026-10-07 and 2026-10-08): the scope above, with no plugin bridge; W9b's write scope; storage as Kestra's, the type stored; expired means missing and honours `errorOnMissing`, and an hourly sweep hard-deletes expired keys; inheritance as Kestra's; `secret()` in a `Set`/`Put` value refused at publish; **a value's writes capped at `max_read_bytes`**, not `max_file_bytes`, so anything writable is readable; versions and soft delete as W9b's R1; the bulk delete capped at 1,000 keys; the server-paced store calls under a key's lock accepted; `Put` over an expired key starts fresh; another workspace's namespace answers as an empty one; `Put` a **real deep merge** with the artifact's `Put.mergeValues` semantics, although on the artifact a flow's value reaches `Put` as JSON text and is stored as a STRING; the API's `PUT` body is **exactly one Ion value** stored as its type, anything else the raw text, typed STRING (R-b: an empty body is 400; R-c: a year- or month-precision timestamp is the raw text); `PurgeKV`'s `before` as Kestra's (a revision is purged once its successor was created at or before it); R-a: `Set` stores a YAML map or list typed; **decimals preserved exactly**; the dense-value limit; every number at most 1,000 digits; conflicting dotted keys in a `Put` refused; both `Put` limits; **comparisons on exact numbers exact**, through option A (the comparison operators compiled into calls), a float compared by its exact binary value; egret's renderer keeps float division (`7 / 2` is `3.5`), a renderer-wide follow-up.

**The release note.** (1) **Migration 0175** creates `workflow.kv_entries`. No new config key, no new permission. (2) **`workflow.storage.max_read_bytes` must now be at least 4096** (`egret_config::MIN_MAX_READ_BYTES`), **checked at boot**: a smaller value refuses to start, naming the minimum, because it is the least the Ion reader of a KV value needs. No chart value or deploy env sets it (0 occurrences under `deploy/`, beside 18 `EGRET_WORKFLOW__` lines). (3) **The hourly expiry sweep.** Every egret process that has a store runs it, first an hour after start (`interval_at`), over every workspace: it hard-deletes each key whose latest revision has expired, rows and objects, by row id under the key's session lock, skipping a busy key, so it is safe on every replica. Until it runs, an expired key keeps its rows and objects, and every reader already treats it as missing. (4) **A KV value is at most `max_read_bytes`** (10 MiB by default) encoded, on `Set`, `Put` (its merged result) and the API's `PUT`; `max_file_bytes` does not apply. **The dense-value limit**: a value holds at most `max_read_bytes`/64 Ion values (163,840 at the default, about 320 KB of small numbers), counting itself and every nested value. **The 1,000-digit limit**: every number in a value (an int, a decimal's coefficient in plain notation, a float, an exponent) has at most 1,000 digits, counted before the Ion parser sees it, on every write and every read. Each refusal names its limit (413 on the API), and nothing is truncated. (5) **Publish refuses `secret()` in a `Set` or `Put` value** (`secret_in_kv_value`), with a message saying the secret's value would be exposed through the KV store, and **a namespace with a `_kv` segment** (0 of the corpus's 345 flows and 0 published flows on the dev Postgres, W9c Task 1). (6) **Exact decimals and comparisons.** An Ion decimal (only the API's `PUT` makes one) stays a decimal through every read, merge and write, its digits exact; it is written with the trailing zeros after its point dropped (`1.50` reads back `1.5`). A decimal or big integer that `kv()` returns compares exactly under `==`, `!=`, `<`, `<=`, `>`, `>=`, `in` and `not in`. To do that, **a new comparator rewrite pass** compiles every comparison operator in every template into a call (`a > b` → `kestra_gt(a, b)`), which answers as minijinja did for every other operand pair. Two visible consequences for every template, not only KV ones: **names starting with `kestra_` are reserved**, and a `set`, `for` or `with` assigning one fails at compile time; and **a chained comparison (`a < b < c`) now evaluates every operand**, so an operand whose evaluation fails (or a `kv()` of a missing key) fails the render even after an earlier comparison was false. Arithmetic on an exact decimal is a named refusal. (7) **`Put`'s limits**: a keyed entry's dotted key has at most 100 segments, a patch at most `max_read_bytes`/64 dotted segments in all, and a patch nests at most 100 deep; conflicting dotted keys (`q.r` with `q`) are refused naming both. (8) **Seven API routes** under `/api/ws/{ws}/workflows/namespaces/{ns}/kv`: the listing, `inheritance`, a key's value (`?revision`) and its `revisions` need **`workflow:read`**; `PUT` of a key, `DELETE` of a key and the bulk `DELETE` need **`workflow:manage`**. A writer waits at most 5 s for a key's lock, then answers 409; the `PUT` body keeps W9b's timeouts (408); the bulk `DELETE` takes **at most 1,000 keys** and a body of at most **1 MiB**, one all-or-nothing transaction, and more answers 413 naming the cap; no store answers 503. (9) **The plugin host's `namespaceKv()` message** is now *"namespaceKv(): egret gives flows the KV store through the `io.kestra.plugin.core.kv` tasks and the `kv()` function, not through a plugin's RunContext; use a `core.kv` task before or after this one."*, and the worker's exit-4 prefix *"the plugin needs a Kestra service egret does not provide to plugins"*. (10) **Known limits** (W9c spec §15): a flow writes and deletes only its own namespace's KV, and `PurgeKV` defaults to its own namespace and children, where Kestra's flows reach any namespace; `expiredOnly: false` is refused in favour of `version` with `keepAmount: 0`; a key named `inheritance` is not read by `GET …/kv/inheritance` (the listing), only with a letter percent-encoded; a ttl is at most 1,000 years; an exact number against a float compares against the float's exact binary value, where Pebble compares `doubleValue()`s; Pebble's `is null` test is unbound (write `== null`); `Get`'s `outputs.value` reaches a later render as an `f64`; DATETIME, DATE and DURATION read back as strings; the read-increment-`Set` pattern is racy, as in Kestra; a failed object delete of the sweep orphans the object; deleting a workspace leaves its `_kv` objects; about seven concurrent 1,000-key bulk deletes on different replicas could fill a stock Postgres's lock table; and the server-paced windows under a key's lock (`Put`'s bounded read and write, the copy and `head` of a write, a purge's and the sweep's object deletes) are accepted. (Task 9, the user's ruling on the final review's F2, 2026-10-08) **A render inside a transaction no longer reads.** `kv()`, the namespace-file functions, `read()` and a `Loop`'s values file in `settle` read through a per-call cache filled with no transaction open: a miss renders a placeholder so that one attempt finds every read it can reach, the attempt rolls back with everything it computed, the reads are fetched and the transaction runs again; per call at most 256 fetched entries, 64 rounds and 4 x `max_read_bytes` cached, each limit an author's error naming it and the read, recorded as the failed run's message (the user's ruling on Task 9's review C1, 2026-10-08). The committed values are a live render's; a render that reads costs its transaction one more attempt, plus one per level of real control dependency, which arises only where a placeholder takes a branch the real value does not (e.g. a `runIf` that the placeholder makes true and the real value false, so the next task's read is reached only in the next attempt). **Two pre-existing `Loop` defects are fixed with it** (Task 9 fix round 2): a `Loop` whose iterations end inside their first settle (an `If` with no `else` whose condition is false) no longer nests one settle per iteration on the stack -- 66 such iterations aborted the process with a stack overflow, and a placeholder could steer a flow onto that path; the tail of a settle now runs from an explicit work stack -- and a `concurrencyLimit: 0` (or any batch) `Loop` whose early iterations end there no longer fails its whole transaction with a duplicate `workflow_executions_loop_run_idx`: a batch's rows are all created before any of them settles. *(Amended 2026-10-09, Task 9 fix round 3: round 2's walk fixed only the single-level shape. A sequential `Loop` whose `Subflow` is refused (aborting from about 10 iterations) and a sequential `Loop` inside another (from 50 outer iterations) still nested a settle per iteration, and a placeholder could steer a working flow onto either. Since round 3 every settle of a transaction -- a batch's first settles, a `Subflow` child's, a refused or `wait: false` `Subflow`'s completion, a replay -- runs from that one walk, so the Rust stack no longer grows with iterations or nesting.)*

**What is still not true:** a KV bridge for plugins (a plugin's `namespaceKv()` still exits 4); `AllowFailure`, so `valids/retry-with-flowable-errors` publishes now but stays baselined; masking KV values in logs or outputs; the UI (W7). The ratchet is **108 mismatching before W9c and 108 after** (294 golden entries, 0 broken), both measured: no golden entry reads the KV store except `retry-with-flowable-errors`. The end to end on a temporary kind cluster passed 6 of 6 scenarios, the hourly sweep observed live; the plugin host under a container runner, and the live 409, 413, 408 and 503, were not driven there. See `oracle-exec/README.md`'s W9c section.

**Correction, 2026-10-16 (AllowFailure flowable): built on `feat/workflow-allow-failure-flowable` and in review, not yet merged; the W9c correction's *"`AllowFailure`, so `valids/retry-with-flowable-errors` publishes now but stays baselined"* is retired rather than deleted.** A block `AllowFailure` runs now: a failed child makes the block `WARNING`, its `errors:` and `finally:` run, and the task after the block runs. The table is measured on the pinned image, `kestra/kestra@sha256:28f59bf175e1…` (`docs/en/specs/2026-10-16-workflow-allow-failure-flowable-design.md` §5.1). Banked: `valids/allow-failure`, `valids/allow-failure-with-retry` and `valids/finally-allowfailure`, which leave `oracle-exec/baseline.txt` (107 entries before, 104 after). `valids/retry-with-flowable-errors` stays baselined: its `retry:` on the block itself is refused as `unsupported_allow_failure_retry`, which is the non-goal of §2. The image retries the failing child in that flow, so the retry on a flowable is a later package's question, not a missing measurement. The `allowFailure:` flag (W8) is unchanged.

**Correction, 2026-10-17 (WorkingDirectory): built on `feat/workflow-working-directory` and not yet merged; the W9b correction's *"running `WorkingDirectory` (and so its `namespaceFiles`, `inputFiles`, `outputFiles`, `cache`)"* item is retired rather than deleted.** `WorkingDirectory` runs now, on the Docker runner. Its children share one directory: the executor drives the block as a flowable, and between children the directory is carried as a snapshot, copied out of the finished child's container and into the next one's (`docs/en/specs/2026-10-16-workflow-working-directory-design.md` §4.2 and §5.2). The block's `namespaceFiles`, `inputFiles`, `cache` and `outputFiles` run in its prepare and finish claims (§4.4). A child whose runner is not Docker fails by name before it runs, and the block stops, so nothing runs on an empty directory. **Still not true:** a `WorkingDirectory` on Kubernetes. Its copy-in path is not built, and the spec's open point 1 defers it; until then the Kubernetes child is refused by name, not run. Flowable children of a block stay refused (§9 of that spec).

The oracle, measured on the pinned image: six `working-directory-real-*` flows are live corpus entries (WorkingDirectory Task 8; corpus 345 to 351, golden 294 to 301). `valids/working-directory-taskrun` banks and `valids/retry-failed-task-duration` is baselined with its cause (Kestra's per-retry scheduler latency of about 0.6 s stops it at three attempts; egret makes four in 5 of 5 rounds), so `oracle-exec/baseline.txt` holds **104** entries, with the census re-derived to match. The executor suite passes with that baseline. Of the four WorkingDirectory valids, `working-directory-taskrun` matches and three stay baselined by cause (`task-runif-workingdirectory`, `working-directory-invalid-runif`, `working-directory-taskrun-nested`). **Live run, 2026-10-17:** the corpus flow `working-directory-real-shared-file` ran once through the Docker runner on the private stack (a private Postgres and this process's in-memory object store, not the shared dev stack or RustFS), with the real snapshot path. Block `wdir`, child `writer` and child `reader` all ended `SUCCESS`, and the reader's log line read back is `READER_SAW=shared-content`, the content `writer` wrote to `shared.txt`. See `docs/en/specs/2026-10-16-workflow-working-directory-design.md` §7.

**Correction, 2026-10-18 (kill and `Exit`): the execution-level kill and the `Exit` task shipped**, on `feat/workflow-kill-exit` (`docs/{en,vi}/specs/2026-10-17-workflow-kill-and-exit-design.md`, plan `docs/en/plans/2026-10-17-workflow-kill-and-exit.md`). W7's "kill/restart", left out of W7, and W8's execution-level kill, which never shipped, are retired for the kill rather than deleted. `POST /api/ws/{workspace}/workflows/executions/{id}/kill?cascade=true|false` (`workflow:manage`, audited) moves the execution to `KILLING`, and to `KILLED` once its work has stopped: a worker finds its run `KILLING` by polling every `workflow.kill_poll_ms` (env `EGRET_WORKFLOW__KILL_POLL_MS`, default 5000, at least 100), cancels the handler and records `KILLED`, and a run whose worker died is finished by the next claim. `cascade` (default `true`) kills subflow and loop children. The execution page has a Kill button. `io.kestra.plugin.core.execution.Exit` runs executor-level; the four `valids/exit*` entries bank, so `oracle-exec/baseline.txt` holds **100** entries (104 before; refused 45 to 41). No migration. **Still not true:** kill of one task run, bulk kill, restart, a live measurement of the kill on Kubernetes, and keeping a killed `taskRunner.delete: false` container (the cancel removes it, as W5a's always has; see that spec's §4.2 note).

**2026-10-18, group C1: `OutputValues`, `Assert`, `PurgeLogs` (#691).** Three worker tasks ported (`docs/{en,vi}/specs/2026-10-18-workflow-core-tasks-design.md` §2). The ratchet went from 100 to **84** mismatching / baselined, 16 entries banked, and the census LIVE count 254 to 281. `PurgeLogs` is limited by default to the flow's own namespace and its child namespaces, and a `namespace` it names must pass the purge rule (`Access::Purge`, as `PurgeFiles` and `PurgeKV`), by the user's ruling (C1 spec §2.3); `valids/purge_logs_full_arguments` is a permanent baseline line for that reason. Group C2 (the executor tasks) is next.

**2026-10-19, workflow debt package, PR-1 (kill and WorkingDirectory).** Built on branch `feat/workflow-debt-kill-wd` (PR-1; `docs/{en,vi}/specs/2026-10-19-workflow-debt-design.md`). What shipped: a killed or timed-out `taskRunner.delete: false` container on Docker is stopped and kept (D1; Kubernetes still deletes the pod, D2); a kept container is removed by the orphan sweeps once its run is terminal and older than the new setting `workflow.kept_container_retention_days` (env `EGRET_WORKFLOW__KEPT_CONTAINER_RETENTION_DAYS`, default 7, `0` keeps for ever; D3); a kill that cannot load an execution closes it by force as `KILLED` instead of looping (D4), and the Docker sweep stops (`docker stop -t 0`, kept) a kept container still running for such a run, while a kept Kubernetes pod of such a run is cleaned up only by the retention, a follow-up (debt spec §2.2, final review I3 note); `commit_kill`'s deadlock retry has its test (D5); **migration 0197** adds `task_runs.working_dir_swept_at` for the hourly `_wd` sweep of the snapshots a crashed WorkingDirectory block left behind, which also sweeps `KILLED` force-closed blocks (D6); and the executor suite's oracle setup reports its own cause instead of "flows must be empty" (D7, the "flake 335"). **D14:** the oracle driver now resumes a run whose short deadline passed under load (a flake fix in the test harness, not in production). The "mode 2" limitation remains: under heavy load `valids/waitfor-child-task-warning` can still match by accident and fail the ratchet with "PROGRESS NOT BANKED", because wall time leaks into the Tick clock; the fix is a virtual clock in the driver, and the entry stays baselined. The ratchet is **301 golden entries, 73 mismatching, 73 baselined, 0 broken, with the census LIVE 293 (351 corpus flows: 293 live, 48 draft-only, 6 unparseable, 4 name-contested)** (no change from before PR-1). Source: `cargo test -p egret-workflow --test executor`. Its test `executor_matches_golden` prints `oracle-exec ratchet: 301 golden entries, 73 mismatching, 73 baselined, 0 broken`, and its census asserts `LIVE = 293` (`oracle-exec/driver.rs`). The figures are identical to the baseline before PR-1. The D14 pin test is `the_driver_resumes_a_run_whose_short_deadline_passed_before_pass_time`. PR-2 (C2 follow-ups, C1 debt) is next.

**W10, W11 and W12 were added on 2026-09-14, after W7 turned out to be blocked, and the reason is worth keeping.** **Four** things W7's row needs — a process that advances executions, an execution REST surface, `workflow.task_logs`, and a loop that fires `cron:` triggers — are designed in §6.1 and §6.3 of this document and were owned by **no row of this table**. The fourth was found while measuring the schema for W11's row, not by reading the table, which is why it is stated separately below. Machine-checked rather than read: grepping this table **as it stood before the W10, W11 and W12 rows below** for `task_logs|execution api|ExecutionController|wire|binary|daemon` returned **0** while `reaper` returned 1 and `W3b` returned 6 in the same pass, and `ExecutionController` appears **exactly once** in this spec — in §8, *"Not ported, but required reading before W7"*, where it is named as a **shape to study**, never as a deliverable. A row that names a thing in the architecture and never in the plan reads, to anyone checking coverage, exactly like a row that is done. The full derivation is in `docs/{en,vi}/specs/2026-09-14-workflow-w10-runtime-and-execution-surface-design.md` §1.

**The trigger loop, the fourth, is W12 above.** That leaves **two names owned by no row of this table**, recorded here rather than left to be rediscovered: the `secret` and `http` expression functions, which `crates/egret-workflow/src/runners/functions.rs`'s `REFUSED` table assigns to W9 **only so its refusal message can name an owner**, by that file's own admission — *"`secret` and `http` are in no package's contents at all"*. **No owner is assigned to them here**, deliberately: reassigning a name a second time makes the message churn without making it true. They are flagged for whoever writes the next programme-level review, and they are the same shape as the four W10 §1 records. *(Corrected 2026-09-30, W5b Task 7: `secret` has an owner now. W5b resolves `secret()` on `kestra.*` plugin tasks, from the execution's workspace's secrets (§6.2's note), and it is still refused everywhere else — script tasks, inputs, `runIf`, `Log`, triggers — with a reason naming where it does resolve. `http` is still owned by no row.)* *(Decided 2026-10-17: `http` is refused permanently, `Refusal::Unsupported`; an expression renders in the egret server, so a request from it is a server-side request forgery. See `docs/en/specs/2026-10-17-workflow-starrocks-timeout-and-http-design.md` §2.)*

Each package gets its own spec and plan, as the DBAPI-parity programme did.

## 10. Deliberately out of v1

`pause`/`resume`, backfill, `allowFailure`, `loopUntil`, concurrency limits, and
dynamic tasks are W8, not W3. They are where Kestra's test mass lives, and every
one is additive in the YAML, so deferring them does not strand a flow anyone has
already written.

## 11. Open questions

1. Log lines live in Postgres with a retention window in v1; object storage is
   W9. Confirm that is enough. **The Postgres half is W11** — §9's row carries
   the measurement showing nothing persists a log line today. *(Answered
   2026-09-27: W11 built the Postgres half — `workflow.task_logs`, one global
   `workflow.log_retention_days`, default 30 days, `0` keeps forever, pruned
   hourly. "Nothing persists a log line today" was true on `6749a37b` and is
   retired by §9's W11 correction. Object storage is still W9's.)* *(2026-10-04, W9a Task 14: still open. W9 was split and W9a built internal storage for execution files only; object storage for **log lines** is not built and is in no package (the W9a spec §14). Log lines stay in Postgres under `workflow.log_retention_days`.)*
2. Publish-time reference resolution makes a published flow rigid: renaming a
   dataset does not re-point it. This is believed correct but is a real
   trade-off.
3. `kestra-plugin-host` needs a policy for which plugin jars are allowed and who
   may add them. Running arbitrary third-party JVM code in a workspace's name is
   a governance decision, not only an engineering one. *(Answered for
   containers 2026-09-28, by the user, in W5a: a per-workspace feature
   `workflow_containers`, off by default, plus one global image allowlist
   the platform admin sets in `workflow.plugin_configurations`, where no entry
   allows no image; both stricter than Kestra OSS, which has neither. The
   `Process` runner is refused. **Still open for plugin jars**: which jars
   the W5b host may load, and who may add them, is W5b's.)* *(Answered for
   plugin jars 2026-09-29, by the user, in W5b: the jars are baked into one
   image from `plugin-host/plugins.lock` in the repo, each named by Maven
   coordinate and checked by sha256 at image build; adding one is a reviewed
   PR, and nothing is added at run time. Three jars today:
   `plugin-jdbc-postgres`, `plugin-jdbc-trino` and `plugin-jdbc-mysql`
   2.0.0. What runs is narrowed further by the platform admin's
   `allowed-types` globs and by the workspace feature
   `workflow_kestra_plugins`, off by default.)*
