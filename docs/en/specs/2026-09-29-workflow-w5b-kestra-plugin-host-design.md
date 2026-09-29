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

## 3. The host (`plugin-host/`)

A Maven project at the repo root, beside `crates/egret-workflow/oracle/`:
- `plugins.lock`: one line per jar, `groupId:artifactId:version sha256`. The first contents: `io.kestra.plugin:plugin-jdbc-postgres:2.0.0`, `…plugin-jdbc-trino:2.0.0`, `…plugin-jdbc-mysql:2.0.0`. All three are built against `io.kestra:platform:2.0.0-rc12` (their POMs, checked 2026-09-29), so they link against the pinned artifact `io.kestra:core:2.0.0-rc13`; the host does not move §7 rule 1's pin. Later plugin versions (2.0.4 is current) are built against `platform 2.0.2` and wait for a deliberate bump of both pins.
- `io.kestra.core.runners.ShimRunContext` extends `DefaultRunContext` in Kestra's own package (spike traps 2 and 3). `render*` returns its input; `logger()` writes to stdout; `metric()` writes one DEBUG line; `workingDir()` is Kestra's `LocalWorkingDir` over the pod's working directory; `storage()`, `namespaceKv()`, `namespace…()`, `secret…()` and every other collaborator egret does not provide throw `EgretUnsupported("… needs internal storage (W9)")`.
- `io.kestra.core.runners.EgretPluginHost` (main): reads `task.json` (`{"type": FQCN, "properties": {…}}`), refuses a `type` absent from its own `catalog.json` (exit 2), deserializes with Kestra's `JacksonMapper` and the plugin registry fallback (spike trap 1), runs `validate`, then `run(shim)`, prints the `Output` as one `::{"outputs": …}::` line and exits.
- **Exit codes**, each a named FAILED reason on the egret side: `0` success; `2` refused or invalid (`ConstraintViolationException`, unknown type); `3` the plugin threw (class and message in the reason, stack trace in the log at ERROR); `4` the plugin asked for something W9 owns. Any other code (a JVM crash, an OOM kill) is FAILED with the code.
- **Log lines:** `LEVEL message`, parsed by the handler into W11 levels; a line that does not parse is INFO.
- **`catalog.json`** is generated by `EgretPluginHost describe` at image build: for each task class in the three jars, its properties with Kestra's JSON schema (type, required, default, deprecated) and its output properties. It carries the sha256 of `plugins.lock`. It is committed under `crates/egret-workflow/src/plugins/catalog.json` and embedded with `include_str!`. A Rust test fails when the committed catalog's lockfile hash differs from `plugins.lock`.
- The image: `deploy/docker/plugin-host.Dockerfile`, Java 25 (the artifact's class-file 69), the Kestra core classpath, the three jars verified against `plugins.lock`, running as a non-root uid. Its reference comes from config, never from the flow.

## 4. The task model and publish

- A task whose `type` starts with `kestra.` is a plugin task. The rest must be a FQCN in `catalog.json`; the rows of `TASK_TYPES` that egret already ports natively (`io.kestra.plugin.core.log.Log`, …) are removed from the catalog at generation and a test asserts the two sets are disjoint, so a native type never runs in the JVM.
- **Refused at publish, by name:** a `kestra.` FQCN not in the catalog; a bare FQCN that is in the catalog (hint: the prefixed spelling); a property the catalog does not list for the class; a missing required property; a property value that needs internal storage — measured per plugin in Task 1, at least `fetchType: STORE`; `taskRunner` (plugin tasks run on the configured runner); the seven base-task properties egret does not read (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`, `workerSelector`), as on script tasks.
- A literal property value is checked against the catalog's JSON type; a `{{ }}` value is accepted and checked by the host after rendering (exit 2).
- `retry`, `timeout`, `disabled`, `runIf`, `allowFailure`, `allowWarning` go through egret's `Task` model as for every task. A plugin task with no `timeout:` runs under `workflow.container_default_timeout_s`.

## 5. Governance

- **Feature `workflow_kestra_plugins`**, off by default (W5a's `default_enabled: false`), requiring `workflow_containers`: a workspace may run scripts without plugins, never plugins without scripts.
- **`allowed-types`:** `plugin_configurations` gains an entry `{type: kestra, values: {allowed-types: [globs]}}`, matched against the FQCN (without `kestra.`) with W5a's `glob_match`. A FQCN holds no `/`, so `*` spans package segments: `io.kestra.plugin.jdbc.*` allows every JDBC class. **No `kestra` entry allows every catalog type** (the catalog was reviewed as a PR); an entry with an empty list allows none. `allowed-types` on any other entry, and `allowed-images` on the `kestra` entry, are refused at boot.
- **The host image** is `workflow.plugin_host.image` and is not matched against `allowed-images`: the author cannot choose it.
- **One gate function, two call sites** (W5a §4): at publish (feature, `allowed-types`, a configured runner) and at run after rendering, before anything is created. A refusal at run creates no pod, and adopts an existing pod for the same (run, attempt) exactly as W5a does.

## 6. `secret()`

- **Syntax is Kestra's:** `{{ secret('NAME') }}` and `{{ secret('NAME', subkey='password') }}`. `namespace=` and `full=` are refused at publish when literal and at run otherwise, by name.
- **Resolution** at run time, in the execution's workspace, through `WorkspaceSecretStore`: with no `subkey`, the key `value` of an `Opaque` secret (a secret with no `value` key is an error naming its keys); with `subkey`, that key of any type. A missing secret or key fails the task, naming the secret and never a value.
- **Only on `kestra.*` tasks.** On a script task `secret()` stays refused, with a reason: the rendered commands and env of a script task are fields of the pod spec, readable by anyone with `get pods` in the task namespace. Everywhere else (inputs, `runIf`, `Log`, triggers) it stays refused as today.
- **Where a value may exist:** the rendered `task.json`, written as an input file (exec+tar on Kubernetes, `docker cp` on Docker), and the JVM's memory. **Nowhere else:** not in `flow_versions`, not in anything stored on the task run (rendered spec, outputs egret writes, error messages), not in the pod spec or its env.
- **Masking:** every value a run resolved is replaced by `******` in its log lines before they reach `RunLog` (Kestra's substring replace), on the first holder and on a holder that re-attached (it re-renders, so it knows the values). Outputs are not masked, as in Kestra: a query that selects a password returns it (§12).

## 7. Running a plugin task

The `kestra` handler (`worker::handlers::kestra`):
1. renders the task's properties with minijinja, resolving `secret()`;
2. calls the gate;
3. builds a `TaskCommands`: image `workflow.plugin_host.image`, command `java … EgretPluginHost /work/task.json`, input file `task.json`, no output files, the timeout;
4. hands it to the configured runner (W5a) — same pod naming, hardening, NetworkPolicy, lease renewal, re-attach and delete-after-commit;
5. maps the exit code to an outcome; takes outputs from the `::{json}::` line (capped at 1 MiB by W5a's cap; over it the task is FAILED by name).

`workflow.plugin_host`: `image` (required when `container_runner` is not `none`), `jvm_max_heap` (default `384m`, passed as `-Xmx`, below the pod's default memory limit). With `container_runner: none` no plugin task can run and publish refuses them, as it does script tasks.

## 8. Security

- The host deserializes only the root class named by `type`, and only if it is in its catalog; the classpath is Kestra core and the three locked jars.
- Pod hardening, NetworkPolicy (egret's Postgres and the Kubernetes API unreachable) and resource caps are W5a's, unchanged.
- A JDBC plugin reaches Trino or StarRocks with the credentials the author supplies; the engine's Ranger policies apply to that user. egret does not add or remove any access through this path.
- A live test searches the metadata database (every `workflow.*` table) and `kubectl get pod -o yaml` for a secret's value after a run and expects zero matches.

## 9. Tasks

1. **Measure on the artifact** (Kestra `2.0.0-rc13` with the three plugins): each plugin's `Query` against Postgres, Trino and StarRocks — output shape per `fetchType`, default `fetchType`, log lines and levels, masking of a secret in a log line, the properties that need storage. `plugin-jdbc-mysql` against StarRocks is measured, not assumed. Fixture `oracle-exec/w5b-plugins.json`.
2. **`plugin-host/`:** Maven project, shim, main, `describe`, `plugins.lock`, Dockerfile, `catalog.json`; Java tests of the shim (W9 calls throw `EgretUnsupported`, exit codes); the Rust lockfile-hash test.
3. **Model, publish, governance:** `kestra.` types from the catalog; save-time refusals; `workflow_kestra_plugins`; `allowed-types`; the gate at publish.
4. **`secret()`:** resolution, `namespace`/`full` refusals, script-task refusal with its reason, masking in `RunLog`'s ingest; tests that the value is absent from stored rows.
5. **Handler and wiring:** the `kestra` handler; exit-code mapping; `workflow.plugin_host` config; registry in `egret-bin`; the gate at run; the Helm chart (image value) and CI's image build.
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

**Ratchet:** expected **109 → 109**, measured; the corpus has no plugin task.

## 11. Out of scope

Internal storage, KV, namespace files, storage-backed outputs (W9); **plugin triggers** (`io.kestra.plugin.jdbc.postgresql.Trigger` and every polling trigger); flowable and dynamic plugins (`dynamicWorkerResult`); plugins beyond the three jars; adding jars at run time; per-workspace allowlists; `secret()` on script tasks, which needs a Kubernetes Secret per attempt and more RBAC; bumping Kestra's pins.

## 12. Known limits

- **Outputs are not masked**, as in Kestra. A query that returns a secret stores it in the task run's outputs, readable by anyone who can read the execution.
- **`fetchType: FETCH` is bounded by W5a's 1 MiB output cap**; a larger result fails the task by name. `STORE`, which Kestra uses for large results, waits for W9.
- **Identity rendering:** a plugin that defers rendering of a property per row or per iteration sees the value egret rendered once (programme spec §3.2).
- **Cost:** about 1.6–2 s of JVM start and plugin scan per task-run attempt, measured in the spike, before the query.
- **Masking is a substring replace**, as Kestra's: a secret split across two log lines, or re-encoded (base64, URL-escaped) by the plugin, is not masked.
