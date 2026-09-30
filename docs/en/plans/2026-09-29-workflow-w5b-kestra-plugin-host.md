# W5b — The Kestra plugin host, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A flow task `type: kestra.<FQCN>` runs a real, unmodified Kestra plugin (the three JDBC plugins) in one JVM per task-run attempt, inside W5a's container runner. Secrets come from the workspace's `workspace_secrets` and never land in egret's database or the pod spec. A timeout cancels the query on the engine.

**Architecture:**
- `plugin-host/` is a Maven project and a container image. It holds a shim `RunContext`, a CLI main, and a `describe` command that writes `catalog.json`. The three plugin jars are fixed by `plugins.lock` (sha256).
- `egret-workflow` embeds `catalog.json`. Publish checks `kestra.*` tasks against it, and one gate (feature, `allowed-types`, runner) runs at publish and at run.
- A new `kestra` handler does four things in order: render (resolving `secret()`), gate, build a `TaskCommands` that runs the host image with `task.json` as an input file, then hand it to the configured W5a runner.
- Runners gain two things: a host protocol (leveled stdout, an `egretFailure` marker) and a graceful `cancel(task, grace)` that waits for the container to stop.

**Tech Stack:** Rust (tokio, sqlx, minijinja, kube), Java 25 + Maven (Kestra `io.kestra:core:2.0.0-rc13`), Docker, Helm, kind.

**Spec:** `docs/{en,vi}/specs/2026-09-29-workflow-w5b-kestra-plugin-host-design.md`. **§n refers to the spec.**

**A warning about this document.** Code blocks show shapes to verify, not text to paste. Wherever the plan disagrees with the code, the plan is wrong, and the implementer says so in the report. Past plans in this programme were wrong in 4–6 places each; see memory `plan-illustrative-code-is-a-guess`.

## Global Constraints

- **Type spelling (user decision):**
  - `kestra.<FQCN>` is the only spelling for a plugin task.
  - A bare catalog FQCN is refused at publish, with the prefixed spelling as a hint.
  - A FQCN egret ports natively is never in the catalog.
- **Jars:** exactly `io.kestra.plugin:plugin-jdbc-postgres:2.0.0`, `plugin-jdbc-trino:2.0.0` and `plugin-jdbc-mysql:2.0.0`, each checked by sha256 against `plugin-host/plugins.lock`.
  - The Kestra core is `io.kestra:core:2.0.0-rc13` (§7 rule 1 pin, unchanged). No jar is added at run time.
- **Governance:**
  - Feature `workflow_kestra_plugins` is **off by default** and requires `workflow_containers`.
  - `plugin_configurations` entry `{type: kestra, values: {allowed-types: [globs]}}`. **No `kestra` entry allows the whole catalog; an empty list allows none.**
  - `allowed-types` on any other entry, and `allowed-images` on the `kestra` entry, are refused at boot.
  - The host image comes from `workflow.plugin_host.image` and is **not** matched against `allowed-images`.
  - **One gate function**, called at publish and at run after rendering. A refusal at run creates nothing, and adopts an existing pod for the same (run, attempt), as W5a does.
- **Secrets:**
  - `{{ secret('NAME') }}` and `{{ secret('NAME', subkey='k') }}`. `namespace=` and `full=` are refused by name.
  - Resolved at run, in the execution's workspace only, and only on `kestra.*` tasks. On a script task `secret()` is still refused, with the reason (pod spec).
  - **A resolved value is never written to:** `flow_versions`, `task_runs` (spec, outputs egret writes, message), `task_logs` unmasked, the pod spec or env, or any `tracing` event.
  - Masking is a substring replace to `******` before `RunLog`.
- **`pluginDefaults:`** is refused at publish on every flow.
- **Timeout:**
  - Plugin tasks cancel with grace `workflow.plugin_host.kill_grace_s` (default **15**); script tasks keep grace **0**.
  - `cancel` returns only when the container stopped or `grace + 5 s` passed.
  - The command is `exec java …`.
- **Defaults, verbatim:** `workflow.plugin_host.jvm_max_heap` `384m`; outputs are capped by W5a's 1 MiB; the `egretFailure` message is at most 4 KiB; host exit codes are `0`/`2`/`3`/`4`, and any other code is FAILED with the code.
- **The ratchet cannot judge this package.** Expected **109 → 109**, measured.
- **Mutation discipline** (as W5a):
  - Every mutation shows evidence that it applied (`git diff --stat` plus a token count), a run count, an unmutated control, and the assertion that killed it.
  - A mutant that fails to compile proves nothing. A mutant in Java, YAML or a Dockerfile is proven by running that artefact, not by `cargo build`.
  - Never commit a mutant. Never mutate while a build or test runs.
- **A zero is reported beside a number that can disagree.** Sweeps are reported as N examined / M corrected per population, using `/usr/bin/grep` with file arguments.
- **Commits:**
  - Conventional Commits, `git -c user.email=hoangquocvietbk2.13@gmail.com`, trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
  - Check `git branch --show-current` == `feat/workflow-w5b-plugin-host` before every commit.
  - Never commit `.superpowers/`, jars, or `plugin-host/target/`. No PRs.
- **Environment:**
  - Worktree `data-portal/egret-w5b`. Dedicated Postgres `egret-w5b-postgres` on **:5437**. `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`.
  - `CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target` is shared: `pgrep -lx cargo` first, never two cargo test processes at once. Keep ≥ 10 G free disk.
  - Never `docker image prune -a`: it removes images other suites need (`python:3.13-slim`, the Trino/StarRocks images).
  - Live tests are gated by env vars, following `container_docker_live`.
  - The kind cluster `egret` is the shared dev stack. The end-to-end run uses a **temporary** kind cluster, `egret-w5b-e2e`.

## File Structure

```
plugin-host/                                             NEW  Maven project (sibling of crates/, like tests/ranger/)
  pom.xml                                                     io.kestra:core:2.0.0-rc13 + deps, Java 25, shade/assembly to lib/
  plugins.lock                                                3 lines: groupId:artifactId:version sha256
  fetch-plugins.sh                                            downloads the 3 jars from Maven Central, verifies sha256
  src/main/java/io/kestra/core/runners/ShimRunContext.java    extends DefaultRunContext (spike traps 2, 3)
  src/main/java/io/kestra/core/runners/EgretPluginHost.java   main: run | describe
  src/main/java/io/kestra/core/runners/EgretUnsupported.java  thrown by every W9 collaborator -> exit 4
  src/main/resources/logback.xml                              `%level %msg%n` to stdout
  src/test/java/…                                             shim + exit-code tests
deploy/docker/plugin-host.Dockerfile                     NEW  Java 25 JRE, lib/, plugins/, non-root uid
crates/egret-workflow/src/plugins/
  mod.rs                                                 NEW  PREFIX, catalog(), PluginTask parse + refusals, plugin_tasks(flow)
  catalog.json                                           NEW  generated by `EgretPluginHost describe`, committed
  gate.rs                                                NEW  the one gate: feature, allowed-types, runner
crates/egret-workflow/src/runners/secrets.rs             NEW  secret(): collect pass, resolve, render pass
crates/egret-workflow/src/worker/handlers/kestra.rs      NEW  the plugin handler
crates/egret-workflow/src/worker/commands.rs             MOD  TaskCommands.host_protocol; CommandsResult.host_failure;
                                                              ingest_line leveled mode; TaskRunner::cancel(task, grace);
                                                              DockerRunner stop -t
crates/egret-workflow/src/worker/k8s/runner.rs           MOD  cancel with grace, waits for termination
crates/egret-workflow/src/worker/handlers/script.rs      MOD  cancel passes Duration::ZERO; secret() refusal reason
crates/egret-workflow/src/logs/run_log.rs                MOD  RunLog::masked(values) -> RunLog
crates/egret-workflow/src/models/flow.rs                 MOD  plugin_defaults captured so validate can refuse it
crates/egret-workflow/src/spec/validate.rs               MOD  kestra.* tasks; pluginDefaults; bare-FQCN hint
crates/egret-workflow/src/publish.rs                     MOD  step 3c plugin gate; step 4 accepts catalog types
crates/egret-workflow/src/routes.rs                      MOD  reads workflow_kestra_plugins into the publish gate
crates/egret-workflow/src/runtime.rs                     MOD  handlers_for registers catalog types when a runner exists
crates/egret-workspace/src/features.rs                   MOD  workflow_kestra_plugins (off, requires workflow_containers)
crates/egret-secrets/src/workspace_secrets.rs            MOD  resolve_typed_by_name(workspace_id: Uuid, name)
crates/egret-config/src/lib.rs                           MOD  plugin_host.{image,jvm_max_heap,kill_grace_s}; allowed-types; boot check
deploy/helm/egret/{values.yaml,templates/…}              MOD  workflow.pluginHost.image -> EGRET_WORKFLOW__PLUGIN_HOST__IMAGE
crates/egret-workflow/tests/{plugins_gate.rs,plugin_host_live.rs}   NEW
crates/egret-workflow/oracle-exec/{w5b-plugins.json,docker-compose.w5b.yml}  NEW  Task 1
```

---

## Task 1: Measure the three plugins on the artifact

**Files:**
- Create `crates/egret-workflow/oracle-exec/docker-compose.w5b.yml`: an override that mounts the three jars into the Kestra service's `/app/plugins` and adds `postgres:18`, `trinodb/trino:<pin>` and a StarRocks all-in-one service on the compose network.
- Create `crates/egret-workflow/oracle-exec/w5b-plugins.json`.
- Amend spec §3/§4/§6/§7.1 (EN and VI) only where a measurement contradicts them.

- [ ] **Step 1: Download the jars** with a throwaway script that `plugin-host/fetch-plugins.sh` in Task 2 will reuse. Source: `https://repo1.maven.org/maven2/io/kestra/plugin/<a>/2.0.0/<a>-2.0.0.jar`. Record each sha256; these become `plugins.lock`.
- [ ] **Step 2: Bring up** the pinned artifact with the override (`docker compose -f docker-compose.yml -f docker-compose.w5b.yml up -d`). Confirm `GET /api/v1/plugins` lists the three JDBC groups.
  - **Hazard (W13b):** never `docker exec psql` as a child of the artifact's postmaster. Use the API, or a separate container on the network.
  - If the nightly image refuses the 2.0.0 jars, stop and report. Do not switch images silently.
- [ ] **Step 3: Measure, saving the raw execution JSON and logs of each run:**
  1. For each plugin's `Query`, against its engine (`postgresql` → Postgres, `trino` → Trino, `mysql` → StarRocks): outputs for `fetchType` `FETCH_ONE`, `FETCH` and `NONE`, and the default `fetchType`; types as rendered in outputs (timestamp, decimal, null).
  2. The **list of task classes** in each jar, with each class's JSON schema, from the artifact's plugin API (`harvest.py` knows the endpoints). Also mark which classes and properties need internal storage: `fetchType: STORE`, `Batch`/`CopyIn`/`CopyOut` `from`/`to`, and anything taking a `kestra://` URI.
  3. The log lines of a `Query` (levels, wording) and of a failing one (bad SQL): the task state and the error message.
  4. `secret()`: set `SECRET_PGPASS` (base64) on the Kestra service and use `{{ secret('PGPASS') }}` as `password`. Record whether the value appears masked in logs when the plugin logs the URL or the error echoes it.
  5. `pluginDefaults:` on a flow: the save response (status, body naming `pluginDefaults`).
  6. **Kill on timeout:** a `Query` of `SELECT pg_sleep(600)` (Postgres), a long Trino query (`SELECT count(*) FROM tpch.sf100.lineitem`) and a StarRocks `SELECT SLEEP(600)`, each with `timeout: PT5S`. Record the task state and message. Then record, **10 s after the timeout**, whether the query is still running on the engine: `pg_stat_activity`, `system.runtime.queries`, `SHOW PROCESSLIST`. Also record whether `AbstractJdbcQuery` in the 2.0.0 jar has a `kill()` (decompile with `javap -p`).
  7. `plugin-jdbc-mysql` against StarRocks at all: connection URL form and a simple `SELECT 1`. If it does not work, stop and report. The spec's third plugin depends on it.
- [ ] **Step 4: Save** the observations verbatim, with timestamps and the image digest. Add a `findings` array quoting values, and `egret_expected` as **typed values**:
  - `classes` (FQCN → properties, required, and `needs_storage`);
  - `refused_values` (FQCN → property → values);
  - `default_fetch_type`;
  - `plugin_defaults_refusal`;
  - `kill_ends_query` (engine → bool);
  - `log_levels`.
  
  Where a question cannot be measured, say so and mark the spec "chosen".
- [ ] **Step 5:** `docker compose … down -v`. Commit `docs(workflow): W5b Task 1 — the JDBC plugins on the artifact`.
- [ ] **Step 6: Baseline gate** at the branch base, after the artifact is down: `cargo test -p egret-workflow`, with the name/result list saved to the SDD workspace as `baseline-gate.txt`.

---

## Task 2: `plugin-host/` — the shim, the CLI, the image, the catalog

**Files:** everything under `plugin-host/`, `deploy/docker/plugin-host.Dockerfile`, `crates/egret-workflow/src/plugins/catalog.json` (generated), and a lockfile-hash test in `crates/egret-workflow/src/plugins/mod.rs` (the module skeleton only: `CATALOG_JSON` and the hash test).

**Interfaces:**
- Produces the CLI contract that Task 5 and Task 6 depend on:

```text
java -Xmx<heap> -cp '/opt/egret-plugin-host/lib/*:/opt/egret-plugin-host/plugins/*' \
     io.kestra.core.runners.EgretPluginHost run <path-to-task.json>
task.json   = {"type": "<FQCN without kestra.>", "properties": {…rendered…}}
stdout      = "<LEVEL> <message>" lines; one "::{\"outputs\": {…}}::" on success;
              one "::{\"egretFailure\": {\"code\": n, \"message\": \"…\"}}::" before a non-zero exit
exit        = 0 ok | 2 refused/invalid | 3 plugin threw | 4 needs W9
SIGTERM     = shutdown hook calls task.kill(), waits for run() up to KILL_WAIT (env EGRET_KILL_WAIT_MS), exits

java … EgretPluginHost describe <plugins-dir>   -> catalog.json on stdout
catalog.json = {"lockSha256": "<sha256 of plugins.lock bytes>", "kestra": "2.0.0-rc13",
                "classes": {"<FQCN>": {"properties": {"<name>": {"schema": {…}, "required": bool}},
                                        "outputs": ["<name>", …]}}}
```

- [ ] **Step 1: `pom.xml`.** Model it on `crates/egret-workflow/oracle/pom.xml` (the same `io.kestra:core:2.0.0-rc13` resolution the spike used; 207 jars). Java release 25. `maven-dependency-plugin:copy-dependencies` into `target/lib/`. JUnit 5 for tests.
- [ ] **Step 2: `plugins.lock` and `fetch-plugins.sh`,** with the three sha256 from Task 1. The script downloads into a directory given as `$1` and fails, naming the jar, on a sha mismatch. It uses `shasum -a 256` on macOS and `sha256sum` on Linux.
- [ ] **Step 3: Write the failing Java tests** (`src/test/java/io/kestra/core/runners/EgretPluginHostTest.java`):

```java
@Test void a_type_absent_from_the_catalog_exits_2() {
    var r = Host.run(json("{\"type\":\"io.kestra.plugin.core.log.Log\",\"properties\":{\"message\":\"x\"}}"));
    assertEquals(2, r.exit());
    assertTrue(r.failureMessage().contains("io.kestra.plugin.core.log.Log"));
}
@Test void storage_on_the_shim_is_exit_4_naming_w9() {
    var shim = new ShimRunContext(Path.of(tmp));
    var e = assertThrows(EgretUnsupported.class, shim::storage);
    assertTrue(e.getMessage().contains("W9"));
}
@Test void render_is_identity() throws Exception {
    assertEquals("{{ not rendered }}", new ShimRunContext(Path.of(tmp)).render("{{ not rendered }}"));
}
@Test void a_constraint_violation_exits_2() { /* postgresql.Query without `sql` */ }
@Test void a_plugin_exception_exits_3_with_class_and_message() { /* Query with url to a closed port */ }
```

`Host.run` is a test seam: `EgretPluginHost.execute(Path taskJson, PrintStream out) -> int`, so the tests never call `System.exit`.
- [ ] **Step 4: Run them to watch them fail** (`mvn -q -f plugin-host/pom.xml test` with the jars fetched into `plugin-host/target/plugins`).
- [ ] **Step 5: Implement** the shim, `EgretUnsupported` and the main, **in package `io.kestra.core.runners`** (spike trap 3):
  - Rebuild the shim from the spike's description (programme spec §3, memory `kestra-plugin-host-spike`).
  - `workingDir()` returns `new LocalWorkingDir(Path.of(System.getenv("WORKING_DIR")))`.
  - Every storage, KV, namespace and secret collaborator throws `EgretUnsupported`.
  - `metric()` logs one DEBUG line.
  - Deserialization: `JacksonMapper.ofJson().convertValue(Map.of("type", fqcn, …properties, "id", "egret"), Task.class)`, which relies on the plugin-registry fallback (spike trap 1).
  - The root class must equal the requested FQCN, and that FQCN must be in the catalog loaded from the classpath resource `catalog.json`.
  - Output: `JacksonMapper.toMap(output)` printed as the `outputs` marker.
  - **The shutdown hook:** register it before `run`. On SIGTERM it calls `task.kill()` (the task is a `WorkerJobLifecycle`), then `runThread.join(KILL_WAIT)`, prints `WARN killed: timeout` and returns.
- [ ] **Step 6: `describe`.** Try Kestra's `io.kestra.core.docs.JsonSchemaGenerator` without Micronaut first. **If it cannot be constructed without a context, stop and report:** the fallback is to generate `catalog.json` from Task 1's artifact schemas, which changes spec §3's "generated at image build". That is the user's call.
  - `describe` excludes the following, and the report lists each exclusion with its reason:
    - triggers and abstract classes;
    - classes whose required property needs storage (Task 1's `needs_storage`);
    - every FQCN in `spec::registry::TASK_TYPES` (a hard-coded list, checked against the Rust table by Task 3's test).
- [ ] **Step 7: Run the Java tests to see them pass.** Then compare `describe`'s properties and required flags with Task 1's `egret_expected.classes`, and report every difference.
- [ ] **Step 8: Dockerfile.** Base `eclipse-temurin:25-jre` (pin its digest). `COPY target/lib/`, the fetched `plugins/` (re-verified by `fetch-plugins.sh` in the build), and the host jar. `USER 65532`. No `ENTRYPOINT` that wraps in a shell: the handler supplies `exec java …`.
  - Build it: `docker build -f deploy/docker/plugin-host.Dockerfile -t egret-plugin-host:dev plugin-host`.
- [ ] **Step 9: Smoke-test the image alone:** start `postgres:18` on a user network, then `docker run --rm -e WORKING_DIR=/work -v $PWD/t:/work egret-plugin-host:dev sh -c 'exec java … run /work/task.json'` with a `postgresql.Query` `SELECT 1 AS one`. Expect exit 0 and an `outputs` marker whose `row` or `rows` match Task 1's shape.
  - Also send `docker stop -t 15` during a `pg_sleep(600)`. Expect `WARN killed: timeout` in the output, and 2 s later no `pg_sleep` in `pg_stat_activity` if Task 1 found `kill_ends_query.postgres = true`.
  - Paste both transcripts into the report.
- [ ] **Step 10: Generate and commit `catalog.json`,** plus the Rust test:

```rust
// crates/egret-workflow/src/plugins/mod.rs
pub(crate) const CATALOG_JSON: &str = include_str!("catalog.json");

#[test]
fn the_committed_catalog_was_generated_from_the_committed_lockfile() {
    let lock = include_bytes!("../../../../plugin-host/plugins.lock");
    let want = sha256_hex(lock);
    let got: serde_json::Value = serde_json::from_str(CATALOG_JSON).unwrap();
    assert_eq!(got["lockSha256"], want, "plugins.lock changed; regenerate catalog.json");
}
```

- [ ] **Step 11:** Add `plugin-host/target/` to `.gitignore`. Commit `feat(workflow): W5b Task 2 — the Kestra plugin host image and its catalog`.

---

## Task 3: The plugin-task model, publish, governance, and `pluginDefaults`

**Files:**
- `crates/egret-workflow/src/plugins/{mod.rs,gate.rs}`, `spec/validate.rs`, `publish.rs`, `routes.rs`, `models/flow.rs`.
- `crates/egret-workspace/src/features.rs`, `crates/egret-config/src/lib.rs` (`allowed-types` and the boot check only).
- Tests in `crates/egret-workflow/tests/plugins_gate.rs` and beside the code.

**Interfaces:**

```rust
// plugins/mod.rs
pub const PREFIX: &str = "kestra.";
pub const FEATURE_KEY: &str = "workflow_kestra_plugins";
pub const CONFIG_TYPE: &str = "kestra";                     // plugin_configurations entry type
pub struct Catalog { pub lock_sha256: String, pub classes: BTreeMap<String, PluginClass> }
pub struct PluginClass { pub properties: BTreeMap<String, PropertySpec>, pub outputs: Vec<String> }
pub struct PropertySpec { pub schema: serde_json::Value, pub required: bool }
pub fn catalog() -> &'static Catalog;                        // LazyLock over CATALOG_JSON; panics on a bad file (tested)
pub fn lookup(type_id: &str) -> Option<(&'static str /*fqcn*/, &'static PluginClass)>; // only "kestra."-prefixed
pub fn bare_hint(type_id: &str) -> Option<String>;           // "kestra.<type_id>" when type_id is a catalog FQCN
pub struct PluginTask { pub fqcn: &'static str, pub properties: serde_json::Map<String, Value> }
pub struct PluginRefusal { pub code: &'static str, pub path: String, pub message: String }
pub fn parse(type_id: &str, spec: &Value) -> Result<PluginTask, Vec<PluginRefusal>>;
pub(crate) fn plugin_tasks(flow: &Flow) -> Vec<(String, &Task, &Value)>; // children() AND error_children()

// plugins/gate.rs
pub struct GateInput<'a> { pub feature_enabled: bool, pub containers_enabled: bool,
                           pub plugin_configurations: &'a PluginConfigurations,
                           pub runner: Option<RunnerKind>, pub fqcn: &'a str }
pub enum GateRefusal { FeatureDisabled, ContainersDisabled, NoRunner, TypeNotAllowed { fqcn: String } }
pub fn check(input: &GateInput<'_>) -> Result<(), GateRefusal>;   // Display names the key/setting to change

// egret-config
impl PluginConfigurations { pub fn allowed_types(&self) -> Option<Vec<&str>>; } // None = no `kestra` entry
impl WorkflowConfig { pub fn validate_plugin_configurations(&self) -> Result<(), String>; }

// publish.rs: ScriptGate gains `kestra_plugins_enabled: bool`; `closed()` sets it false.
pub fn check_plugin_tasks(flow: &Flow, gate: &ScriptGate<'_>) -> Result<()>;   // step 3c
```

- [ ] **Step 1: Write the failing tests** in `tests/plugins_gate.rs`. One test per row of spec §10 that this task owns, and each asserts the named code and a substring of the message:

```rust
#[test] fn a_catalog_fqcn_with_the_prefix_parses() {
    let t = plugins::parse("kestra.io.kestra.plugin.jdbc.postgresql.Query",
        &json!({"url": "jdbc:postgresql://h/db", "sql": "select 1"})).unwrap();
    assert_eq!(t.fqcn, "io.kestra.plugin.jdbc.postgresql.Query");
}
#[test] fn a_bare_catalog_fqcn_is_refused_with_the_prefixed_hint() { /* validate() error message contains
    "kestra.io.kestra.plugin.jdbc.postgresql.Query" */ }
#[test] fn an_unknown_property_is_refused_by_name() { /* "sqll" */ }
#[test] fn a_missing_required_property_is_refused_by_name() {}
#[test] fn fetch_type_store_is_refused_naming_w9() {}
#[test] fn task_runner_and_the_seven_base_properties_are_refused() {}
#[test] fn a_literal_of_the_wrong_plain_type_is_refused_and_an_expression_is_not() {}
#[test] fn no_native_fqcn_is_in_the_catalog() {
    for row in spec::registry::TASK_TYPES { assert!(plugins::catalog().classes.get(row.r#type).is_none()); }
}
#[test] fn plugin_defaults_is_refused_on_every_flow_even_empty() { /* `pluginDefaults: []`, flow with only Log */ }
#[test] fn a_plugin_task_under_errors_is_parsed_too() {}
#[test] fn no_kestra_entry_allows_the_catalog_and_an_empty_list_allows_none() {}
#[test] fn allowed_types_glob_spans_package_segments() { /* io.kestra.plugin.jdbc.* */ }
#[test] fn the_feature_requires_workflow_containers() {}
#[tokio::test] async fn publish_refuses_a_plugin_task_with_the_feature_off() {}
#[test] fn allowed_types_on_a_runner_entry_is_refused_at_boot() {}
```

- [ ] **Step 2: Run them to watch them fail** (`cargo test -p egret-workflow --test plugins_gate`).
- [ ] **Step 3: Implement.**
  - **Walker:** `plugin_tasks` walks `children()` **and** `error_children()`, as `scripts::script_tasks` does (`scripts/mod.rs:932`). `walk_task`'s pre-existing blindness to `errors:` must not extend to plugins.
  - **Validation:** `validate` gains `validate_plugin_tasks`, placed beside `validate_script_tasks` (`spec/validate.rs:380`).
  - **The `Leaf` arm:** at `validate.rs:1326`, `known_type(..).is_none()` becomes `known_type(..).is_none() && plugins::lookup(..).is_none()`. A bare catalog FQCN gets an error carrying `bare_hint`, not the generic warning.
  - **Publish step 4:** accepts `plugins::lookup(..).is_some()`. A plugin task has no `RefField`s.
  - **`models/flow.rs`:** add

    ```rust
    #[serde(default, rename = "pluginDefaults", skip_serializing)]
    pub plugin_defaults: Option<serde_json::Value>,
    ```

    `validate` refuses `Some(_)` with code `removed_plugin_defaults` and the spec §4.1 message. Check first that nothing re-serialises a `Flow` into a stored snapshot in a way that would depend on this field. Report what you find.
  - **Features:** `workflow_kestra_plugins` goes in the feature catalog with `default_enabled: false`, plus a `(workflow_kestra_plugins, workflow_containers, "…")` dependency row.
  - **Routes:** at `routes.rs:717`, read the new feature and fill `ScriptGate::kestra_plugins_enabled`.
  - **Glob:** reuse `scripts::gate::glob_match`. Do not write a second glob.
- [ ] **Step 4: Run the tests to see them pass.** Then the crate's existing validate and publish tests.
- [ ] **Step 5: Mutations.** For each, record evidence per the Global Constraints:
  - (a) `plugin_tasks` without `error_children()`;
  - (b) the no-entry default inverted;
  - (c) the `pluginDefaults` check removed;
  - (d) the step-4 catalog arm removed;
  - (e) the feature dependency row removed.
- [ ] **Step 6: Commit** `feat(workflow): W5b Task 3 — kestra.* tasks publish against the catalog, behind their own gate`.

---

## Task 4: `secret()` and masking

**Files:** `runners/secrets.rs` (new), `runners/functions.rs` (the `REFUSED` row for `secret` gets a reason that names the script-task rule), `logs/run_log.rs`, `crates/egret-secrets/src/workspace_secrets.rs`, and tests beside each file plus a DB-gated test in `tests/plugins_gate.rs`.

**Interfaces:**

```rust
// egret-secrets
impl WorkspaceSecretStore {
    /// Workspace-scoped ONLY: `workspace_id = $1 AND name = $2` -- never the global fallback.
    pub async fn resolve_typed_by_name(&self, workspace_id: Uuid, name: &str)
        -> Result<TypedSecret, SecretsError>;
}

// runners/secrets.rs
#[async_trait] pub trait SecretSource: Send + Sync {
    async fn get(&self, workspace: Uuid, name: &str) -> Result<BTreeMap<String, String>, SecretError>;
}
pub struct Resolved { pub values: Vec<String> }          // every value handed out, for masking
/// Render every string in `props` (recursively), resolving secret():
/// pass 1 renders with a collecting `secret` (records (name, subkey), returns ""),
/// then fetches each distinct name once, then pass 2 renders with the fetched map.
/// A name first seen in pass 2 is an error ("secret names must not depend on secrets").
pub async fn render_with_secrets(props: &serde_json::Map<String, Value>, cx: &RenderContext,
    workspace: Uuid, source: &dyn SecretSource) -> Result<(serde_json::Map<String, Value>, Resolved), String>;

// logs/run_log.rs
impl RunLog { pub fn masked(&self, values: &[String]) -> RunLog; } // shares counters; `line` replaces each value by "******"
```

- [ ] **Step 1: Write the failing tests:**

```rust
#[tokio::test] async fn secret_resolves_value_and_subkey_in_the_executions_workspace() {}
#[tokio::test] async fn a_secret_of_another_workspace_is_not_found_and_the_message_names_no_value() {}
#[tokio::test] async fn namespace_and_full_are_refused_by_name() {}
#[tokio::test] async fn an_opaque_secret_without_value_names_its_keys() {}
#[tokio::test] async fn secret_under_a_filter_sees_the_real_value() { /* {{ secret('p') | length }} == "6" */ }
#[tokio::test] async fn a_secret_name_computed_from_a_secret_is_refused() {}
#[tokio::test] async fn masked_run_log_replaces_every_value_and_counts_the_masked_line() {}
#[test] fn secret_on_a_script_task_is_still_refused_with_the_pod_spec_reason() {}
```

  The first two run against the dedicated Postgres (`:5437`), with two workspaces and one secret of the same name in each.
- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - **The functions:** the collecting and the resolving `secret` are per-render functions, registered over `REFUSED`'s `secret` on a copy of the environment. Add a `render_str_with(template, cx, extend: &dyn Fn(&mut Environment))` beside `render_str` (`runners/env.rs:1870`), keeping its render thread and fixpoint loop. The global `environment()` keeps refusing `secret`.
  - **Mask before the split:** `masked` masks the whole message *before* `pieces()` splits it, so a split cannot cut a secret in two inside one line.
  - **Never log a value:** a `SecretError`'s `Display` names the secret and never a value. Check every `tracing::` call in the new code.
- [ ] **Step 4: Run the tests to see them pass.**
- [ ] **Step 5: Mutations:**
  - (a) `resolve_typed_by_name` with the global fallback (the second test must die);
  - (b) `masked` masks after the split;
  - (c) the script-task refusal removed.
- [ ] **Step 6: Commit** `feat(workflow): W5b Task 4 — secret() from workspace secrets, masked in logs`.

---

## Task 5: Runner changes — the host protocol and a graceful cancel

**Files:** `worker/commands.rs`, `worker/k8s/runner.rs`, `worker/handlers/script.rs` (the `cancel` call site only), and every other `TaskRunner` impl or fake in tests (find them with `rg "impl TaskRunner for"`).

**Interfaces:**

```rust
pub struct TaskCommands { /* … */ pub host_protocol: bool }   // default false: W5a behaviour, byte for byte
pub struct CommandsResult { /* … */ pub host_failure: Option<HostFailure> }
pub struct HostFailure { pub code: i32, pub message: String }   // message truncated to 4096 bytes at a char boundary
async fn cancel(&self, task: &TaskInput, grace: std::time::Duration) -> Result<()>;
```

- [ ] **Step 1: Write the failing tests:**

```rust
#[test] fn host_protocol_levels_stdout_and_a_continuation_keeps_the_previous_level() {
    // "WARN a", "\tat x.y(Z.java:1)", "INFO b" -> Warn, Warn, Info
}
#[test] fn without_host_protocol_a_level_prefix_is_ordinary_text() { /* "WARN a" stays Info, text unchanged */ }
#[test] fn the_egret_failure_marker_is_harvested_only_under_host_protocol() {}
#[test] fn an_egret_failure_message_over_4_kib_is_truncated_on_a_char_boundary() {}
// live, gated like container_docker_live:
#[tokio::test] async fn docker_cancel_with_grace_lets_a_sigterm_trap_run() {
    // alpine: sh -c 'trap "echo got-term; exit 0" TERM; while :; do sleep 1; done' — note: exec'd trap, not a child
    // cancel(task, 10s) -> the attempt's log holds "got-term"; with grace 0 it does not
}
#[tokio::test] async fn docker_cancel_returns_only_after_the_container_stopped() {}
// live on kind (k8s_runner_live's gating):
#[tokio::test] async fn k8s_cancel_with_grace_waits_for_termination_then_release_deletes() {}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.**
  - **`ingest_line`:** gains a `host_protocol: bool` parameter and a `StreamState::last_level`. With it on, stdout lines starting with one of `TRACE|DEBUG|INFO|WARN|ERROR` plus a space are leveled and the prefix is stripped. The debug bracket and marker rules are unchanged.
  - **`classify_marker`:** under `host_protocol`, harvest `egretFailure` into `host_failure`.
  - **`DockerRunner::cancel(grace)`:** for `grace > 0`, `docker stop -t <secs>` then the existing `rm --force`. For `0`, exactly W5a.
  - **`KubernetesRunner::cancel(grace)`:** delete with `grace_period(grace)`, then poll the pod (the existing watch or get helper) until it is gone or `main` is terminated, bounded by `grace + 5 s`. A still-running pod after the bound is logged at WARN and `Ok(())`, since the outcome is the timeout's either way (`cancel.rs` fact 2).
  - **The script handler** passes `Duration::ZERO`.
- [ ] **Step 4: Run the tests to see them pass.** Run the whole `egret-workflow` suite: W5a's tests must not change.
- [ ] **Step 5: Mutations:**
  - (a) the continuation line gets INFO;
  - (b) `host_protocol` ignored (levels always on);
  - (c) Docker `stop` with `-t 0`;
  - (d) the k8s cancel without the wait. The kind test must die, or report it as unproved.
- [ ] **Step 6: Commit** `feat(workflow): W5b Task 5 — runners speak the host protocol and cancel with grace`.

---

## Task 6: The `kestra` handler and wiring

**Files:**
- `worker/handlers/kestra.rs` (new), `worker/handlers/mod.rs`, `runtime.rs`.
- `crates/egret-config/src/lib.rs` (`plugin_host`), `crates/egret-bin/src/main.rs` (only if `handlers_for` needs new inputs).
- The Helm chart (`values.yaml`, the egret deployment's env).
- `crates/egret-workflow/tests/plugin_host_live.rs` (new).

**Interfaces:**

```rust
// egret-config
pub struct PluginHostConfig { pub image: Option<String>, pub jvm_max_heap: String /*384m*/, pub kill_grace_s: u64 /*15*/ }
// WorkflowConfig.plugin_host: PluginHostConfig; boot error when container_runner != none && image is None

// handlers/kestra.rs
// handlers/script.rs's `FeatureState` gains `async fn kestra_plugins_enabled(&self, ws: Uuid) -> Result<bool>`
// (PgFeatureState reads `workflow_kestra_plugins` with the same default-off rule as `workflow_containers`).
pub struct KestraRuntime { pub runner: Arc<dyn TaskRunner>, pub runner_kind: RunnerKind,
    pub features: Arc<dyn FeatureState>, pub plugin_configurations: PluginConfigurations,
    pub host: PluginHostConfig, pub secrets: Arc<dyn SecretSource> }
pub fn register(handlers: &mut Handlers, rt: Arc<KestraRuntime>);   // one key per catalog FQCN: "kestra.<fqcn>"
```

- [ ] **Step 1: Write the failing tests.** These are unit tests with a spy runner, modelled on `handlers/script.rs`'s tests (`script.rs:438-1217`):

```rust
#[tokio::test] async fn a_disabled_feature_fails_the_run_and_calls_no_runner() {}
#[tokio::test] async fn a_refused_run_adopts_its_existing_container_and_never_creates_one() {}
#[tokio::test] async fn the_commands_run_the_host_image_with_exec_java_and_task_json_as_the_only_input() {
    // container_image == cfg.image; argv joined contains "exec java -Xmx384m"; input_files keys == ["task.json"];
    // env contains no rendered property value; host_protocol == true
}
#[tokio::test] async fn a_secret_is_in_task_json_and_nowhere_in_the_commands_or_env() {}
#[tokio::test] async fn exit_codes_map_to_named_reasons() { /* 2, 3, 4, 137 */ }
#[tokio::test] async fn outputs_are_the_markers_outputs_as_they_are() {}
#[tokio::test] async fn cancel_passes_the_configured_grace() {}
#[tokio::test] async fn the_host_image_is_not_matched_against_allowed_images() { /* allowed-images empty everywhere: runs */ }
#[tokio::test] async fn a_plugin_task_without_timeout_runs_under_container_default_timeout() {}
#[test] fn register_serves_every_catalog_type_with_the_prefix_and_nothing_else() {}
```

- [ ] **Step 2: Run them to watch them fail.**
- [ ] **Step 3: Implement.** The handler runs these steps in order:
  1. `plugins::parse`: the same function publish uses;
  2. `render_with_secrets`;
  3. `plugins::gate::check` with the feature state **read now**;
  4. build `TaskCommands { container_image: host.image, interpreter: ["/bin/sh","-c"], commands: ["exec java -Xmx<heap> -cp '…' io.kestra.core.runners.EgretPluginHost run \"$WORKING_DIR/task.json\""], input_files: {"task.json": …}, output_files: [], host_protocol: true, .. }`;
  5. build `task.masked(resolved.values)` as the `TaskInput` given to the runner;
  6. `run` (or `adopt` on refusal);
  7. map the outcome: exit `0` → SUCCESS with `vars` as outputs; `2`/`3`/`4` → FAILED with `host_failure.message` prefixed by the code's name; any other code → FAILED `the plugin host exited with <n>`.

  **Verify before relying on it:** how each runner stages `input_files`. `DockerRunner` stages to a temporary directory on egret's disk before `docker cp`. Confirm it is removed on every path, including a panic, and has mode 0700, and state it in the report. For Kubernetes, confirm the tar is built in memory. If either writes the rendered `task.json` anywhere that outlives the attempt, stop and report.

  **Default timeout:** find where W5a applies `container_default_timeout_s` to script types (`rg container_default_timeout crates/egret-workflow/src`) and extend that predicate to `kestra.*` types, with the test above. It must not become a second, separate rule.

  **Wiring:**
  - `runtime::handlers_for` registers the kestra handler when a runner exists, sharing the script runner's `Arc`.
  - `egret-bin` builds `SecretSource` from its `WorkspaceSecretStore`.
  - Helm: `workflow.pluginHost.image` renders to `EGRET_WORKFLOW__PLUGIN_HOST__IMAGE`, and `kill_grace_s` likewise.
- [ ] **Step 4: Run the tests to see them pass.**
- [ ] **Step 5: Live tests** (`tests/plugin_host_live.rs`, gated by `EGRET_PLUGIN_HOST_LIVE=1`, Docker runner, image `egret-plugin-host:dev`):
  - a `kestra.…postgresql.Query` flow runs end to end through the worker: rows are in `outputs`, lines come back through the W11 API, the password comes from a workspace secret, and a log line that echoes it is stored as `******`;
  - the same for Trino and StarRocks (skipped with a printed reason when their containers are absent, **and the report says how many were skipped**);
  - `SELECT pg_sleep(600)` with `timeout: PT5S`: FAILED `Timeout after …`, and `pg_stat_activity` holds no `pg_sleep` 20 s later (or the Task 1 finding, if the plugin's `kill()` does not cancel);
  - **after all runs,** a scan of every text column of every `workflow.*` table for the secret's value, expecting zero rows. Next to that zero, report the count of rows scanned.
- [ ] **Step 6: Mutations:**
  - (a) the secret rendered into `env` (the scan or the spy must die);
  - (b) `masked` not applied;
  - (c) `exec` dropped from the command. The live timeout test must die: `sh` as PID 1 does not forward SIGTERM.
- [ ] **Step 7: Commit** `feat(workflow): W5b Task 6 — the kestra handler runs plugins in the host image`.

---

## Task 7: End to end on a temporary kind cluster, coverage, docs, full CI

- [ ] **Step 1: Set up the cluster.**
  - `kind create cluster --name egret-w5b-e2e`, then build and `kind load` `egret-plugin-host:dev` and `egret-task-helper:dev`.
  - Install the chart with `workflow.containerRunner=kubernetes` and `workflow.pluginHost.image=egret-plugin-host:dev`.
  - Add a Postgres in a namespace the task NetworkPolicy lets through.
  - **Do not touch the shared `egret` cluster.**
- [ ] **Step 2: Drive the scenario through the API.**
  - Enable `workflow_containers` and `workflow_kestra_plugins`. Create a workspace secret. Publish and run a flow with a `kestra.…postgresql.Query` using `{{ secret('pg', subkey='password') }}`.
  - Check: the rows are in the execution's outputs; the pod ran and was deleted; `kubectl get pod -o yaml` taken **during** the run (hold it with a `pg_sleep(20)` first task) does not contain the password; the database scan finds zero rows.
  - A `pg_sleep(600)` with `timeout: PT10S` leaves no query after the grace.
  - Turning the feature off leads to a refusal at publish and at run, and no pod.
  - `pluginDefaults:` is refused.
  - Save the transcript under the SDD workspace.
- [ ] **Step 3:** `kind delete cluster --name egret-w5b-e2e`.
- [ ] **Step 4: Coverage and docs.**
  - `oracle-exec/README.md` gets a W5b section: what the ratchet can and cannot see, the measured **109 → 109**, and every live test with its gating variable.
  - Programme spec §9 gets a W5b correction paragraph, like W5a's. §11 question 3 is answered for jars. §6.4's `kestra.<FQCN>` row says "W5b", no longer "not built". The note on `secret` being owned by no package is corrected: `secret()` now resolves on `kestra.*` tasks, and is still refused elsewhere.
  - EN/VI sweep of both specs and the programme spec, reported as N examined / M corrected per population.
- [ ] **Step 5: Full local CI:**
  - `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`;
  - `cargo test --workspace` on `:5437`;
  - the frontend `npm run lint && npm test` if any generated API changed (config and features could change the OpenAPI: run `cargo run -p egret-bin -- openapi > openapi.json` and `npm run gen:api`, never bare orval);
  - `mvn -f plugin-host/pom.xml verify`;
  - `helm lint deploy/helm/egret`.
  
  Compare against `baseline-gate.txt`. Every failure absent from the baseline is this branch's until measured otherwise (memory `pre-existing-is-a-hypothesis`).
- [ ] **Step 6: Commit** `docs(workflow): W5b Task 7 — end to end on kind, coverage and the programme spec`. **Stop:** no PR until the user asks.
