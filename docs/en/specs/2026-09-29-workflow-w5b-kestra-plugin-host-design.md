# W5b — The Kestra plugin host: real Kestra plugins, run in W5a's containers

**Status:** design, awaiting review
**Depends on:** W5a (task runners, `workflow_containers`, `plugin_configurations`, lease renewal), W11 (`RunLog`) — both merged. Base `6acb1d68`.
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3 (the spike), §6.4 (row `kestra.<FQCN>`), §7 rule 1, §9 (row W5), §11 open question 3
**Decided with the user, 2026-09-29:**
- **Jars are baked into one image from a lockfile in the repo.** `plugin-host/plugins.lock` names each jar by Maven coordinate and sha256; adding a plugin is a reviewed PR. Nobody adds a jar at run time. This answers the jar half of §11 open question 3.
- **`type:` is `kestra.<FQCN>`**, the only spelling (§6.4 as written). A bare plugin FQCN is refused at publish with the prefixed spelling as a hint. This departs from §7 rule 4 on purpose: a Kestra flow pasted in needs its plugin `type:` lines prefixed.
- **`secret()` arrives in W5b**, resolved at run time from the execution's workspace's `workspace_secrets`, on `kestra.*` tasks only.
- **The first lockfile holds three plugins:** `plugin-jdbc-postgres`, `plugin-jdbc-trino`, `plugin-jdbc-mysql` (for StarRocks: Maven Central has no `plugin-jdbc-starrocks`, checked 2026-09-29).
- **Governance:** a feature `workflow_kestra_plugins`, **off by default**, requiring `workflow_containers`; the platform admin narrows the catalog with `allowed-types` globs; no entry allows the whole catalog.
- **One JVM per task-run attempt**, in a W5a pod. No long-lived JVM, no JVM in `egret-server`.
- **A timeout cancels the query on the engine** (added after comparing with Kestra): the host is stopped with SIGTERM and a grace period and calls the plugin's `kill()` — for JDBC, `Statement.cancel()` — before the pod is removed (§7.1). Measured in Task 1: `kill()` ends the query on Postgres; on Trino the query ended through Kestra's kill, which also interrupts the thread (`kill()` alone is tested in §10); on StarRocks it does not (§7.1, §12). *(W5b Task 7, 2026-09-30: the Trino test was not built — see §10 — so `kill()` alone on Trino is still unmeasured.)*
- **`pluginDefaults:` is refused at publish, on every flow**, as Kestra 2.0 refuses it (§4.1). Today egret drops it silently.
- **Decided 2026-09-30, after the whole-branch review:** writing a flow that calls `secret()` needs its own workspace permission, `workflow:use_secrets` (§6); and the host masks secrets on its own stdout, with egret masking its copy too (§6).

**Delivery:** one PR (docs and code), tasks in §9.

## 1. What exists and what does not

| piece | state on `origin/main` (`6acb1d68`) |
|---|---|
| the spike | proven 2026-09-01 (§3 of the programme spec): a ~120-line `ShimRunContext` ran the unmodified `io.kestra.plugin.jdbc.postgresql.Query` with no Micronaut in 1.61–1.68 s. The shim's source is **not in the repo** |
| Java in the repo | `crates/egret-workflow/oracle/` — a Maven project linking `io.kestra:core:2.0.0-rc13` (the Pebble oracle); `tests/ranger/` |
| task types | `spec::registry::TASK_TYPES` is a closed table of fifteen rows; every other `type:` is refused at publish, so no plugin FQCN publishes |
| runners | W5a's `TaskRunner`: Docker locally, one Kubernetes pod per task-run attempt in production; input files uploaded by exec+tar; outputs by `::{json}::` lines; re-attach by name; pod deleted after the outcome commits |
| governance | feature `workflow_containers` (off by default); `plugin_configurations[type].values.allowed-images`; one gate at publish and at run |
| `secret()` | refused by `runners::functions::REFUSED` (`"the secret backend", "W9"`); owned by no package (programme spec §9, note after W10) |
| workspace secrets | `egret_secrets::WorkspaceSecretStore`: named, typed (`Opaque` `{value}`, `kubernetes.io/basic-auth` `{username,password}`, `kubernetes.io/tls`), sealed with AES-256-GCM, with a console |

## 2. What Kestra does, and where egret differs

Kestra (checkout `96893b589`, artifact `2.0.0-rc13`):
- A plugin task is a Java class; its worker calls `run(RunContext)`. Properties are `Property<T>` rendered **inside** the plugin through `RunContext.render`, with Pebble and the flow's variables, secrets included.
- Plugins are fat jars on the worker's classpath; `kestra plugins install` fetches them from Maven Central. OSS has no allowlist.
- `secret(key, namespace, subkey, full)` (`pebble/functions/SecretFunction.java`): `namespace` defaults to the flow's and may name another one the flow is allowed to read; `subkey` reads one key of a JSON-shaped secret; `full` returns `{value, metadata}`.
- Every secret a run used is replaced by `******` in its log lines (`RunContextLogger.BaseAppender.replaceSecret`, a substring replace). Outputs are **not** masked.

| Kestra | egret W5b | why |
|---|---|---|
| `type: io.kestra.plugin.jdbc.postgresql.Query` | `type: kestra.io.kestra.plugin.jdbc.postgresql.Query` | user decision: the prefix says "runs in the JVM host" |
| plugins installed at will | three jars fixed by `plugins.lock`, sha256-checked at image build | governance (§11 question 3) |
| properties rendered inside the plugin | rendered by egret (minijinja) before dispatch; the shim's `render*` return their input | the spike's finding 3; no Pebble in the pod |
| unknown property refused at save by the JSON deserializer | refused at publish from `catalog.json`, generated from the jars at image build | publish runs without a JVM |
| `secret()` backend per namespace | `workspace_secrets` of the execution's workspace; `namespace` and `full` refused by name | a flow cannot name another workspace |
| outputs and files to internal storage | outputs inline (1 MiB cap, W5a's); anything needing storage fails by name | internal storage is W9 |
| Kestra runs the plugin in its worker JVM | one JVM per task-run attempt, in W5a's container | isolation: a plugin's OOM or `System.exit()` takes down one pod |
| flow-level `pluginDefaults` removed in 2.0 (commit `ca6cd11fa`, in every tag from `v2.0.0-rc6`): refused at save, ignored when an old flow is read | refused at publish on every flow; egret drops it silently until W5b | follow the pinned artifact; the feature moved to Enterprise Policies |
| on kill the worker calls the task's `kill()` (`WorkerJobLifecycle.kill`, `WorkerTaskCallable.java:48`); on timeout it interrupts the thread and calls `kill()` only once `run()` has returned (`AbstractWorkerCallable.callWithTimeout`), so a timed-out `pg_sleep(600)` ran its full 600 s (measured, Task 1) | the same `kill()`, reached at the timeout through SIGTERM and a grace period (§7.1) | a JDBC query must not outlive its task on the engine |

## 3. The host (`plugin-host/`)

A Maven project at the repo root, beside `crates/egret-workflow/oracle/`:
- `plugins.lock`: one line per jar, `groupId:artifactId:version sha256`. The first contents: `io.kestra.plugin:plugin-jdbc-postgres:2.0.0`, `…plugin-jdbc-trino:2.0.0`, `…plugin-jdbc-mysql:2.0.0`. All three are built against `io.kestra:platform:2.0.0-rc12` (their POMs, checked 2026-09-29), so they link against the pinned artifact `io.kestra:core:2.0.0-rc13`; the host does not move §7 rule 1's pin. Later plugin versions (2.0.4 is current) are built against `platform 2.0.2` and wait for a deliberate bump of both pins.
- `io.kestra.core.runners.ShimRunContext` extends `DefaultRunContext` in Kestra's own package (spike traps 2 and 3). `render*` returns its input; `logger()` writes to stdout; `metric()` writes one DEBUG line; `workingDir()` is Kestra's `LocalWorkingDir` over the pod's working directory; `storage()`, `namespaceKv()`, `namespace…()`, `secret…()` and every other collaborator egret does not provide throw `EgretUnsupported("… needs internal storage (W9)")`. *(W5b Task 7, 2026-09-30: one collaborator is a deliberate no-op rather than a refusal: `assets()`, Kestra's lineage metadata (an Enterprise feature). `AbstractJdbcBaseQuery` emits an asset for every `CREATE TABLE`, so a refusal would fail a plain DDL statement; the shim records nothing and writes one DEBUG line per asset.)*
- `io.kestra.core.runners.EgretPluginHost` (main): reads `task.json` (`{"type": FQCN, "properties": {…}}`), refuses a `type` absent from its own `catalog.json` (exit 2), deserializes with Kestra's `JacksonMapper` and the plugin registry fallback (spike trap 1), runs `validate`, then `run(shim)`, prints the `Output` as one `::{"outputs": …}::` line and exits.
- **Exit codes**, each a named FAILED reason on the egret side: `0` success; `2` refused or invalid (`ConstraintViolationException`, unknown type); `3` the plugin threw (class and message in the reason, stack trace in the log at ERROR); `4` the plugin asked for something W9 owns. Any other code (a JVM crash, an OOM kill) is FAILED with the code.
- **Log lines:** `LEVEL message` on stdout, parsed into W11 levels. A line with no level prefix (a stack trace's continuation) takes the level of the line before it, INFO if there is none. stderr stays ERROR, as for every container.
- **Failure reason:** on a non-zero exit the host prints one `::{"egretFailure": {"code": n, "message": "…"}}::` line before exiting; the handler puts the message (at most 4 KiB) in the FAILED reason. The runner harvests that key only for plugin tasks, so a script's own markers are unchanged.
- **`catalog.json`** is generated by `EgretPluginHost describe` at image build: for each task class in the three jars, its properties with Kestra's JSON schema (type, required, default, deprecated) and its output properties. It carries the sha256 of `plugins.lock`. It is committed under `crates/egret-workflow/src/plugins/catalog.json` and embedded with `include_str!`. A Rust test fails when the committed catalog's lockfile hash differs from `plugins.lock`.
- The image: `deploy/docker/plugin-host.Dockerfile`, Java 25 (the artifact's class-file 69), the Kestra core classpath, the three jars verified against `plugins.lock`, running as a non-root uid. Its reference comes from config, never from the flow.

## 4. The task model and publish

- A task whose `type` starts with `kestra.` is a plugin task. The rest must be a FQCN in `catalog.json`; the rows of `TASK_TYPES` that egret already ports natively (`io.kestra.plugin.core.log.Log`, …) are removed from the catalog at generation and a test asserts the two sets are disjoint, so a native type never runs in the JVM.
- **Refused at publish, by name:** a `kestra.` FQCN not in the catalog; a bare FQCN that is in the catalog (hint: the prefixed spelling); a property the catalog does not list for the class; a missing required property; a property value that needs internal storage — measured per plugin in Task 1, at least `fetchType: STORE`; `taskRunner` (plugin tasks run on the configured runner); the seven base-task properties egret does not read (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`, `workerSelector`), as on script tasks. *(W5b Task 7, 2026-09-30: four classes need internal storage as a whole, not through one property — `postgresql.Batch`, `mysql.Batch` and `postgresql.CopyIn` (their required `from` is a `kestra://` URI) and `postgresql.CopyOut` (it always writes its result to storage, measured in Task 1) — so the catalog leaves them out at generation, and they are refused at publish as types the catalog does not hold. The catalog holds five classes: `Query` and `Queries` for Postgres and MySQL, and Trino's `Query`.)*
- A literal property value is checked against the catalog's JSON type where the schema is a plain `string`, `integer`, `number`, `boolean`, `array` or `object`; a schema egret cannot read (`anyOf`, `$ref`, …) and a `{{ }}` value are accepted and checked by the host after rendering (exit 2).
- `retry`, `timeout`, `disabled`, `runIf`, `allowFailure`, `allowWarning` go through egret's `Task` model as for every task. A plugin task with no `timeout:` runs under `workflow.container_default_timeout_s`.

### 4.1 `pluginDefaults`

Kestra 2.0 removed flow-level `pluginDefaults` (commit `ca6cd11fa`, *"OSS enablers for EE Policies"*, 2026-07-17; in every tag from `v2.0.0-rc6`, so in the pinned `rc13`). Its `FlowService.validate` returns a constraint violation naming `pluginDefaults` (`FlowServiceTest.shouldReturnConstraintWhenFlowUsesRemovedPluginDefaults`); its lenient read path parses such a flow and applies nothing (`FlowParsingServiceTest.shouldIgnorePluginDefaultsWhenParsingLeniently`).

egret's `Flow` has no such field and no `deny_unknown_fields`, so today a flow with `pluginDefaults:` publishes and the values are silently not applied. W5b makes `spec::validate` refuse a top-level `pluginDefaults` key, whatever its value (an empty list included), on **every** flow, not only flows with plugin tasks: *"`pluginDefaults` was removed in Kestra 2.0; set the properties on each task"*. The refusal shows on a draft and blocks publish. A draft that runs anyway applies nothing, as Kestra's read path does. Measured in Task 1: the artifact refuses all three shapes (a list on a flow with a plugin task, `[]` on a flow with only a `Log`, a list on a flow with only a `Log`) at save with 422 from its JSON deserializer, `Unrecognized field "pluginDefaults" (class io.kestra.core.models.flows.FlowWithSource), not marked as ignorable`, not with a message of its own.

Other top-level keys egret drops silently are not audited here (§11).

## 5. Governance

- **Feature `workflow_kestra_plugins`**, off by default (W5a's `default_enabled: false`), requiring `workflow_containers`: a workspace may run scripts without plugins, never plugins without scripts.
- **`allowed-types`:** `plugin_configurations` gains an entry `{type: kestra, values: {allowed-types: [globs]}}`, matched against the FQCN (without `kestra.`) with W5a's `glob_match`. A FQCN holds no `/`, so `*` spans package segments: `io.kestra.plugin.jdbc.*` allows every JDBC class. **No `kestra` entry allows every catalog type** (the catalog was reviewed as a PR); an entry with an empty list allows none. `allowed-types` on any other entry, and `allowed-images` on the `kestra` entry, are refused at boot.
- **The host image** is `workflow.plugin_host.image` and is not matched against `allowed-images`: the author cannot choose it.
- **One gate function, two call sites** (W5a §4): at publish (feature, `allowed-types`, a configured runner) and at run after rendering, before anything is created. A refusal at run creates no pod, and adopts an existing pod for the same (run, attempt) exactly as W5a does.

## 6. `secret()`

- **Syntax is Kestra's:** `{{ secret('NAME') }}` and `{{ secret('NAME', subkey='password') }}`. `namespace=` and `full=` are refused at publish when literal and at run otherwise, by name. *(W5b fix round 1, 2026-09-30: "literal" is a `namespace=` or `full=` keyword, or a second positional argument (Kestra's signature reads it as `namespace`), written in a `secret(` call on a `kestra.*` task; code `secret_argument_refused`, with the run-time sentence. A call the source scan cannot see — `secret` bound to another name, a nested call in the argument list — is refused at run.)*
- **Who may write it** *(W5b fix round 1, 2026-09-30, user decision)*. `secret()` reads any secret of the workspace, including the engine and database credentials a workspace admin registers, and the flow's author decides where the value goes (a query's outputs, a JDBC url of their choosing). So a flow whose source can call it is **saved (create, update) or published** only by a principal holding the workspace permission **`workflow:use_secrets`** ("Save and publish flows that call secret()"), checked through the PDP on top of `workflow:manage`. Migration `0160` seeds it to `ws_admin` only; the platform admin holds it through its unrestricted template; `ws_editor` does not. The refusal is a 403 naming the permission. "Can call" is the identifier `secret` (not an attribute such as `inputs.secret`, not part of a longer name) in the code of any `{{ }}` or `{% %}` region (outside its string literals; `{# #}` comments do not count) of any YAML string or key, read as YAML decodes it, so `{% set s = secret %}` and a `\u0073ecret` escape count. **Saving is gated, not only publishing,** because every execution path runs the stored draft (`workflow.flows.source`), not a published version: the API start, the webhook, `Schedule`, `Flow` triggers and subflows read the draft, and replay and a retry's replacement run the source frozen on an execution, which came from a draft. Any save of such a source needs the permission, not only one that adds the call: an edit elsewhere in a flow that already reads a secret can redirect the value. Running, replaying or validating one does not: those write no source. A draft stored before migration `0160` is not re-checked at run (§12).
- **Resolution** at run time, in the execution's workspace, through `WorkspaceSecretStore`: with no `subkey`, the key `value` of an `Opaque` secret (a secret with no `value` key is an error naming its keys); with `subkey`, that key of any type. A missing secret or key fails the task, naming the secret and never a value.
- **Render once.** A plugin task's properties render in one pass, Kestra's default (`recursiveRendering = false`), not in the fixpoint loop other renders use: neither render-context data (inputs, trigger bodies, task outputs) nor a secret's value is evaluated as a template. Secret names written as string literals are collected from the source text, so a call the collecting render does not reach still resolves; a name computed from a secret's value is refused without being named.
- **Only on `kestra.*` tasks.** On a script task `secret()` stays refused, with a reason: the rendered commands and env of a script task are fields of the pod spec, readable by anyone with `get pods` in the task namespace. Everywhere else (inputs, `runIf`, `Log`, triggers) it stays refused as today. *(W5b fix round 1, 2026-09-30: refused at save and publish too, by path, code `secret_outside_plugin_task` — every string and key of the flow outside a `kestra.*` task's properties, the base-task keys of a plugin task (`runIf`, `timeout`, …) and tasks under `errors:` included. A save-only finding: a stored draft still plans, and the render refuses the call at run as before.)*
- **Where a value may exist:** the rendered `task.json`, written as an input file (exec+tar on Kubernetes, `docker cp` on Docker), and the JVM's memory. **Nowhere else:** not in `flow_versions`, not in anything stored on the task run (rendered spec, outputs egret writes, error messages), not in the pod spec or its env. *(W5b fix round 1, 2026-09-30: the whole-branch review found this false — the host printed what a plugin logged, secrets included, on the container's stdout, which `kubectl logs`, a node's log shipper and Docker's json-file driver keep; the Task 7 end to end stored `/* ****** */` only because egret masked its own copy. Now `task.json` also lists the values in `mask`, and the host masks its own stdout and stderr (next bullet). What remains: outputs, which are not masked, reach the stored task run and the container's stdout when the task returns a value (§12).)*
- **Masking:** every value a run resolved is replaced by `******` in its log lines before they reach `RunLog` (Kestra's substring replace), on the first holder and on a holder that re-attached (it re-renders, so it knows the values). Outputs are not masked, as in Kestra: a query that selects a password returns it (§12). *(W5b fix round 1, 2026-09-30: **the host masks too, before a byte leaves the JVM.** It reads `task.json`'s `mask` first, replaces stdout and stderr with line-buffered masking streams (whole lines only, so a value written in two pieces is still masked), and formats logback events with a masking layout (`%msg` and the stack trace); the `egretFailure` message is masked before it is JSON-encoded. Same algorithm as egret's: one scan, longest value first. The one line it leaves as it is is the `::{"outputs": …}::` marker, because outputs are not masked: masking them was measured to corrupt a real result — in the live test the username secret `w5b` turned the returned `'w5b-t6'` into `'******-t6'`. egret keeps masking its own copy.)*

## 7. Running a plugin task

The `kestra` handler (`worker::handlers::kestra`):
1. renders the task's properties with minijinja, resolving `secret()`;
2. calls the gate;
3. builds a `TaskCommands`: image `workflow.plugin_host.image`, command `java … EgretPluginHost /work/task.json` *(as built: `exec java -Xmx<heap> -cp … io.kestra.core.runners.EgretPluginHost run "$WORKING_DIR/task.json"`, with the one env var `EGRET_KILL_WAIT_MS` = `(kill_grace_s − 2) × 1000`)*, input file `task.json`, no output files, the timeout;
4. hands it to the configured runner (W5a) — same pod naming, hardening, NetworkPolicy, lease renewal, re-attach and delete-after-commit;
5. maps the exit code to an outcome; takes outputs from the `::{json}::` line (capped at 1 MiB by W5a's cap; over it the task is FAILED by name).

### 7.1 Timeout: cancel on the engine, then remove

W5a cancels a container with grace 0 (`KubernetesRunner::cancel`, `DockerRunner::cancel` = `docker rm --force`). Cutting the TCP connection does not stop a query: Postgres notices only when it next writes to the socket, and Trino and StarRocks keep the query until the client's heartbeat expires. Kestra's kill instead calls the task's `kill()`, which `AbstractJdbcQuery` implements as `Statement.isClosed()`, `cancel()`, `close()`; Kestra's timeout reaches `kill()` only after `run()` returns (§2).

- `TaskRunner::cancel` gains a `grace: Duration` argument: `0` from the script handler (W5a unchanged) and `workflow.plugin_host.kill_grace_s` (default **15 s**, minimum **3 s**: egret refuses to start below it, because the host waits `grace − 2 s` for `kill()` and anything less leaves it no time) from the plugin handler.
- **`cancel` returns only once the container has stopped** (or `grace` + 5 s has passed): the outcome commits after `cancel`, and `release` then removes the pod with grace 0, which would otherwise cut the plugin's `kill()` short.
- The command is `exec java …`, so the JVM is PID 1 and receives the SIGTERM itself; a `/bin/sh -c` parent would not forward it.
- **Kubernetes:** the pod is deleted with `gracePeriodSeconds = cancel_grace`; the kubelet sends SIGTERM to `main`, waits, then SIGKILL. **Docker:** `docker stop -t <grace>`, then W5a's `docker rm --force`.
- **The host** registers a JVM shutdown hook: on SIGTERM it calls `kill()` on the running task from the hook's thread (Kestra's own contract: *"may be invoked from a different thread than the one running the job"*), waits for `run` to return up to `cancel_grace − 2 s`, logs `killed: <reason>` and exits. *(W5b Task 7, 2026-09-30: that line, and the exit-3 `egretFailure` marker the host prints after it when `kill()` made `run()` throw, are not stored: the handler's run is dropped when the timeout fires, before `cancel` is called, so nothing reads the host during the grace and W11's logs end at the last line before the timeout.)*
- **Connection pooling is off for every JDBC plugin** *(W5b Task 6, recorded 2026-09-30)*: the host sets `connectionPooling: false` whatever the flow says, and writes one DEBUG line when the flow set it. The plugins' default Hikari pool registers a JVM shutdown hook of its own which, on the same SIGTERM, closes the connection without a cancel; when it beat `kill()`, the query outlived its task (measured in Task 6: 1 of 11 runs before the override; after it, 16 of 16 runs in which the query had reached Postgres ended with Postgres's `canceling statement due to user request`). A one-shot JVM gains nothing from a pool, so the override changes performance only, never what the task does.
- The outcome is egret's, not the host's: the task is FAILED with W11's `Timeout after …` line whatever the host prints. The kill runs before the outcome is committed and the pod removal after it, W5a's order.
- **Measured in Task 1** (`oracle-exec/w5b-plugins.json`, through Kestra's kill): `kill()` ends the query on Postgres (`idle` in `pg_stat_activity` within about 1 s, `canceling statement due to user request`). On Trino the query ended (`USER_CANCELED` in `system.runtime.queries`), but Kestra's kill also interrupts the thread, so `kill()` alone on Trino is unmeasured and is tested by the host's own live test (§10). *(W5b Task 7, 2026-09-30: that test was not built — the plan's live tests time out on Postgres only — so this is still unmeasured.)* On StarRocks through `plugin-jdbc-mysql` it does **not**: MySQL Connector/J's `Statement.isClosed()`, which `kill()` calls first, waits for the connection lock the running statement holds, so `kill()` blocks until the query ends. The host's hook then outlives the grace, and the query runs until it finishes or StarRocks's `query_timeout` (300 s by default) ends it (§12).
- egret has no user-facing kill yet (W10's finding); when it arrives it takes the same path.

`workflow.plugin_host`: `image` (required when `container_runner` is not `none`), `jvm_max_heap` (default `384m`, passed as `-Xmx`, below the pod's default memory limit), `kill_grace_s` (default 15, minimum 3; a lower value is a boot error). With `container_runner: none` no plugin task can run and publish refuses them, as it does script tasks.

**Release and upgrade note** *(W5b fix round 1, 2026-09-30)*:
- **Every deployment with `container_runner` other than `none` must now set `workflow.plugin_host.image`, or egret does not start** — also a deployment that runs no plugin task. The Helm chart defaults `workflow.pluginHost.image` to `egret-plugin-host:dev`, an image no registry publishes: a production install sets a pullable reference, built from `deploy/docker/plugin-host.Dockerfile`.
- Migration `0160` adds `workflow:use_secrets` to `ws_admin`. A custom workspace role that should save flows calling `secret()` must be granted it.
- Before turning `workflow_kestra_plugins` on for a workspace, review its drafts for `secret(`: a draft saved before `0160` was never checked against `workflow:use_secrets`, and executions run drafts.

## 8. Security

- The host deserializes only the root class named by `type`, and only if it is in its catalog; the classpath is Kestra core and the three locked jars.
- Pod hardening, NetworkPolicy (egret's Postgres and the Kubernetes API unreachable) and resource caps are W5a's, unchanged.
- A JDBC plugin reaches Trino or StarRocks with the credentials the author supplies; the engine's Ranger policies apply to that user. egret does not add or remove any access through this path.
- A live test searches the metadata database (every `workflow.*` table) and `kubectl get pod -o yaml` for a secret's value after a run and expects zero matches.

## 9. Tasks

1. **Measure on the artifact** (Kestra `2.0.0-rc13` with the three plugins): each plugin's `Query` against Postgres, Trino and StarRocks — output shape per `fetchType`, default `fetchType`, log lines and levels, masking of a secret in a log line, the properties that need storage, the refusal of a flow with `pluginDefaults`, whether `kill()` ends the query on each engine. `plugin-jdbc-mysql` against StarRocks is measured, not assumed. Fixture `oracle-exec/w5b-plugins.json`.
2. **`plugin-host/`:** Maven project, shim, main, `describe`, `plugins.lock`, Dockerfile, `catalog.json`; Java tests of the shim (W9 calls throw `EgretUnsupported`, exit codes); the Rust lockfile-hash test.
3. **Model, publish, governance:** `kestra.` types from the catalog; save-time refusals; `pluginDefaults` refused on every flow; `workflow_kestra_plugins`; `allowed-types`; the gate at publish.
4. **`secret()`:** resolution, `namespace`/`full` refusals, script-task refusal with its reason, masking in `RunLog`'s ingest; tests that the value is absent from stored rows.
5. **Handler and wiring:** the `kestra` handler; exit-code mapping; `workflow.plugin_host` config; registry in `egret-bin`; `cancel_grace` in both runners and the host's shutdown hook; the gate at run; the Helm chart (image value) and CI's image build.
6. **End to end on a temporary kind cluster:** `jdbc-postgres` against a Postgres in the cluster; the secret absent from the database and the pod spec; feature off refused with no pod; coverage statement; EN/VI sweep; full local CI.

## 10. Testing

| test | mutation it kills |
|---|---|
| a `kestra.…postgresql.Query` flow runs in a pod on kind, its rows in `outputs`, its lines in W11's API | handler not registered / outputs not parsed |
| the same flow on Docker against Postgres, Trino and StarRocks (live, gated like `container_docker_live`) | a plugin missing from the image |
| a secret used in `password` appears in no `workflow.*` row and not in `kubectl get pod -o yaml` (live, kind) | secret rendered into a stored spec or into env |
| a log line containing the secret is stored as `******` | masking dropped |
| `secret()` on a script task, and `secret(namespace=…)`, refused by name | the refusal dropped |
| feature off → refused at publish and at run, no pod | the run-time gate skipped |
| `allowed-types: []` refuses every plugin; no entry allows the catalog | inverted default |
| bare FQCN refused with the prefixed hint; a native FQCN never reaches the catalog | the disjointness test dropped |
| an unknown property and `fetchType: STORE` refused at publish | catalog check dropped |
| a plugin calling `storage()` fails with exit 4 and a reason naming W9 | W9 calls not trapped |
| catalog hash ≠ `plugins.lock` hash fails the Rust test | a lockfile change without a regenerated catalog |
| a flow with `pluginDefaults:` (also `[]`, also with no plugin task) refused at publish by name | the key accepted and dropped |
| a task timing out on `SELECT pg_sleep(600)` leaves no query in `pg_stat_activity` after the grace (live, Docker and kind); the same on Trino (live, Docker) *(W5b Task 7, 2026-09-30: built and measured for Postgres on Docker (`tests/plugin_host_live.rs`) and on kind (the Task 7 end to end); the Trino half was not built)* | grace 0 / no shutdown hook / `kill()` not called |

**Ratchet:** expected **109 → 109**, measured; the corpus has no plugin task.

## 11. Out of scope

Internal storage, KV, namespace files, storage-backed outputs (W9); **plugin triggers** (`io.kestra.plugin.jdbc.postgresql.Trigger` and every polling trigger); flowable and dynamic plugins (`dynamicWorkerResult`); plugins beyond the three jars; adding jars at run time; per-workspace allowlists; `secret()` on script tasks, which needs a Kubernetes Secret per attempt and more RBAC; bumping Kestra's pins; an audit of the other top-level flow keys egret drops without refusing.

## 12. Known limits

- **Outputs are not masked**, as in Kestra. A query that returns a secret stores it in the task run's outputs, readable by anyone who can read the execution. *(W5b fix round 1: the same value is on the container's stdout, in the outputs marker, the one line the host does not mask — §6.)*
- **`fetchType: FETCH` is bounded by W5a's 1 MiB output cap**; a larger result fails the task by name. `STORE`, which Kestra uses for large results, waits for W9.
- **Identity rendering:** a plugin that defers rendering of a property per row or per iteration sees the value egret rendered once (programme spec §3.2).
- **Cost:** about 1.6–2 s of JVM start and plugin scan per task-run attempt, measured in the spike, before the query.
- **A StarRocks query outlives its timed-out task** (§7.1, measured in Task 1): the pod is removed after the grace, and the query runs until it finishes or StarRocks's `query_timeout` ends it.
- **Masking is a substring replace**, as Kestra's: a secret split across two log lines, or re-encoded (base64, URL-escaped) by the plugin, is not masked.
- **A `timeout:` shorter than the JVM's start means the plugin never runs** *(W5b Task 6, recorded 2026-09-30)*. The budget starts before the pod does, and the host needs about 3.6 s to start on an idle machine and more than 5 s on a loaded one (measured in Task 6) before the plugin is deserialized. A shorter timeout ends the task FAILED `Timeout after …` with nothing sent to the engine. On Kubernetes the pod's start comes first: in Task 7's end to end on kind, `main` started about 6 s after the task did, and the query reached Postgres at about 11 s; two runs with `timeout: PT10S` never connected to Postgres at all (its connection log holds no connection from the pod), while `PT30S` did and was cancelled. Give plugin tasks a timeout of tens of seconds at least.
- **A cancel can still be lost in one upstream window** *(W5b Task 6, recorded 2026-09-30)*. Kestra's `AbstractJdbcQuery.kill()` reads its running statement and, when that is still `null`, closes the connection instead of cancelling. A SIGTERM that lands in the microseconds between the plugin opening the connection and publishing the statement therefore closes the socket with no cancel, and the query lives on as it would after a cut connection (§7.1). egret cannot close this without wrapping the plugin's class; it is a known residual, not measured to occur.
- **A secret rotated between two holders of one attempt** *(W5b fix round 1, 2026-09-30, review M3; not fixed)*. A holder that re-attaches re-renders, so it masks with the values current at re-attach; a line the old pod printed with the old value, replayed after the re-attach, is stored unmasked by egret. The host masked it on the container's stdout with the value it was given, so it is unmasked only in egret's copy.
- **Release engineering is not in CI** *(W5b fix round 1, review M4; not fixed)*. CI builds no plugin-host image and runs no `mvn verify`; the chart's default image `egret-plugin-host:dev` is published nowhere; nothing checks that the catalog baked into an image equals the one egret embeds (the Rust test compares only the lockfile hash); the Kestra core jars in `lib/` are pinned by version, not by hash.
- **A plugin timeout widens W5a's re-run window** *(W5b fix round 1, review M7; not fixed)*. The graceful cancel (up to `kill_grace_s` + 5 s) runs before the outcome commits, so a worker that loses its lease in that window leaves an attempt that a new holder may run again: about 20 s by default, where W5a's grace-0 cancel left milliseconds.
- **A draft saved before migration `0160` is not re-checked at run** *(W5b fix round 1, I3)*. `workflow:use_secrets` is checked when a source is written; executions run the stored draft without asking who wrote it. Such a draft needs the permission to be saved or published again, but runs as it is (§7's release note).
