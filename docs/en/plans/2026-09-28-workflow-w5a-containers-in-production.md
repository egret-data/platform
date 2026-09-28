# W5a — Script tasks and task runners, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Kestra's script tasks (`io.kestra.plugin.scripts.{shell,python}.{Commands,Script}`) with a `taskRunner:` property run in production. They run in a Kubernetes pod per task, or in Docker locally, behind a per-workspace feature and a global image allowlist. The worker renews its lease so that a long task is never run twice.

**Architecture:**
- `egret.container`'s handler becomes the handler for the four script types, and the two egret types become their aliases.
- The runner is chosen from `taskRunner:` or the configured default and dispatched through the existing `TaskRunner` trait. `DockerRunner` gains re-attach; a new `KubernetesRunner` is modelled on Kestra's `PodCreate`.
- One gate function (feature, runner, image, properties) is called at publish and again at run time after rendering.
- The worker renews `claimed_at` under a fence while a handler runs.

**Tech Stack:** Rust, tokio, sqlx, `kube` + `k8s-openapi` (already in `egret-bin`), Helm, kind.

**Spec:** `docs/{en,vi}/specs/2026-09-28-workflow-w5a-containers-in-production-design.md`. **§n refers to the spec.**

**A warning about this document.** Code blocks are shapes to verify, not text to paste. The plan is wrong wherever it disagrees with the code, and an implementer who finds that says so in the report.

## Global Constraints

- **Kestra fidelity:** task and runner `type:`s are Kestra's FQCNs. Property names are Kestra's.
  - Anything W5a does not implement is **refused at save with a named error**, never ignored.
  - Where egret deliberately differs (spec §2 table), the difference is written in the code's doc comment.
- **Governance (user decision):**
  - Feature `workflow_containers` is **off by default** and requires `workflows`.
  - `plugin_configurations[type].values.allowed-images`: no entry or an empty list allows **no** image.
  - The `Process` runner is refused at save.
  - **One gate function**, called at publish and at run after rendering. A refusal at run is a FAILED task with the reason, and **no pod or container is created**.
- **Defaults, verbatim from the spec:**
  - `container_runner` is `none` (with `none`, the production registry is unchanged);
  - `container_default_timeout_s` is 3600;
  - lease renewal every `working_timeout_s / 3`, under a fence;
  - pod name `egret-run-<run_id>-<attempt>`;
  - labels `egret.io/{workspace-id,execution-id,taskrun-id,attempt}`;
  - `delete` defaults to `true`; `pullPolicy` defaults to `IF_NOT_PRESENT`.
- **Pod hardening:**
  - `automountServiceAccountToken: false`, `runAsNonRoot: true`, `runAsUser` from config, all capabilities dropped, `seccompProfile: RuntimeDefault`;
  - resources capped by `max_resources`.
- **RBAC (exactly):** `pods` get/list/watch/create/delete; `pods/log` get; `pods/exec` get, create. Only in the task namespace. (`get` added 2026-09-28 by user decision: WebSocket exec on Kubernetes >= 1.30 is authorised as `get` (measured on kind 1.33 in W5a Task 6's e2e).)
- **NetworkPolicy:** task pods cannot reach egret's metadata Postgres or the Kubernetes API server. All other egress is open.
- **Logs:** every stdout/stderr line goes through W11's `ingest_line` → `RunLog`, unchanged.
- **The ratchet cannot judge this package.** Expected **109 → 109**, measured.
- **Mutation discipline:**
  - Every mutation shows evidence that it applied (`git diff --stat` plus a token count), a run count, an unmutated control, and the assertion that killed it.
  - A mutant that fails to compile proves nothing, and a surviving mutant is reported as unproved.
  - Never commit a mutant, and never mutate while a build or test runs.
- **A zero is reported beside a number that can disagree.** A sweep is reported as N examined / M corrected, per population, using `/usr/bin/grep` with file arguments.
- **Commits:**
  - Conventional Commits, made with `git -c user.email=hoangquocvietbk2.13@gmail.com`.
  - Trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
  - Check `git branch --show-current` before every commit.
  - Never commit `.superpowers/`. No PRs.
- **Environment:**
  - A dedicated Postgres on its own port (the controller sets it up), and `EGRET_TEST_SHARED_STORE_URL=redis://localhost:6399`.
  - `CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret/target` is shared: check `pgrep -lx cargo` first, and never run two cargo test processes at once.
  - Keep ≥ 10 G free disk.
  - Live Kubernetes tests use the kind cluster `egret` (`kind-egret` context) and are gated by an env var, following `container_docker_live`'s gating.
  - Never touch the dev stack's other namespaces.

## File Structure

```
crates/egret-workspace/src/features.rs                  MOD  Feature.default_enabled; `workflow_containers`
crates/egret-config/src/…                               MOD  container_runner, container_default_timeout_s,
                                                              plugin_configurations, kubernetes.{…}
crates/egret-workflow/src/
  spec/registry.rs                                      MOD  four script FQCNs; aliases egret.container / egret.script.python
  scripts/mod.rs                                        NEW  the script-task model: properties, refusals, taskRunner parsing
  scripts/gate.rs                                       NEW  the one gate: feature, runner, image allowlist, properties
  spec/validate.rs                                      MOD  publish calls scripts::gate
  worker/handlers/container.rs                          MOD  handles the four types; run-time gate; runner dispatch
  worker/commands.rs                                    MOD  TaskRunner gains re-attach; DockerRunner re-attach by name
  worker/k8s/{mod,pod_spec,runner,transfer}.rs          NEW  KubernetesRunner (pod_spec is pure)
  worker/claim.rs                                       MOD  lease renewal loop around the handler; fence-lost handling
  worker/reap.rs (or claim.rs)                          MOD  orphan-pod sweep hook
  runtime.rs                                            MOD  build the runner from config; pass to the registry
crates/egret-bin/src/main.rs                            MOD  register script types when container_runner != none
deploy/docker/task-helper.Dockerfile                    NEW  helper image for files-in / files-out
deploy/helm/egret/templates/{namespace,rbac,networkpolicy}-workflow-tasks.yaml   NEW
crates/egret-workflow/tests/{scripts_gate.rs,k8s_runner_live.rs}                 NEW
crates/egret-workflow/oracle-exec/w5a-scripts.json      NEW  Task 1's measurements
```

---

## Task 1: Measure script tasks on the artifact

**Files:**
- Create `crates/egret-workflow/oracle-exec/w5a-scripts.json`.
- Amend spec §3/§4 (EN and VI) only where a measurement contradicts them.

- [ ] **Step 1:** Bring up the pinned artifact (`crates/egret-workflow/oracle-exec`, `docker compose up -d`; the API and auth are in `harvest.py`).
  - **Hazard measured in W13b:** never `docker exec psql` as a child of the artifact's postmaster. Use the HTTP API, or a separate container on the compose network.
- [ ] **Step 2: Measure, recording the raw JSON of each execution and its logs:**
  1. For each of `shell.Commands`, `shell.Script`, `python.Commands` and `python.Script` with no `containerImage`: the image actually used (`docker ps`/events while it runs, or the task's outputs/logs) and the default `taskRunner`.
  2. The `outputFiles` output shape (`outputFiles: {name: kestra://…}`) for `["out.txt"]` and a glob. `inputFiles` given as an inline map: where the files land (`{{ workingDir }}`).
  3. `warningOnStdErr` true/false: the task state when stderr has a line.
  4. `failFast` (Commands): the effect on a failing first command. `exitCode` in outputs.
  5. The properties the image **accepts** on each type: list them from the JSON schema (`GET /api/v1/plugins/{type}` or the schema endpoint `harvest.py` knows), so that Task 2 can refuse precisely what W5a does not implement.
  6. `containerImage: python` (tagless): the image reference Docker pulls.
  7. `taskRunner: {type: io.kestra.plugin.core.runner.Process}` and the Docker runner with `delete: false`: the behaviour, for the record.
- [ ] **Step 3: Save** the observations verbatim with timestamps and the image digest. Add a `findings` array quoting its values, and `egret_expected` entries as **typed values** that Task 2's tests can deserialize and assert (W11's lesson: no prose where a value is expected). Where a question cannot be measured, say so, and mark the spec "chosen".
- [ ] **Step 4:** `docker compose down -v`. Commit `docs(workflow): W5a Task 1 — script tasks on the artifact`.
- [ ] **Step 5: Baseline gate** at the branch base (after the artifact is down): `cargo test -p egret-workflow`, with the name/result list saved to the SDD workspace as `baseline-gate.txt`.

---

## Task 2: The script-task model and the gate

**Files:**
- Create `crates/egret-workflow/src/scripts/{mod.rs,gate.rs}`.
- Modify `spec/registry.rs`, `spec/validate.rs`, `crates/egret-workspace/src/features.rs` and `egret-config` (`plugin_configurations` only; the rest of the config lands in Task 5).
- Tests go in `crates/egret-workflow/tests/scripts_gate.rs` and beside the code.

**Interfaces:**

```rust
// scripts/mod.rs
pub enum ScriptKind { ShellCommands, ShellScript, PythonCommands, PythonScript }
pub enum RunnerKind { Docker, Kubernetes }                        // Process is unrepresentable: refused while parsing
pub struct RunnerSpec { pub kind: RunnerKind, pub pull_policy: PullPolicy, pub delete: bool,
                        pub resources: Option<Resources> /* Kubernetes */, pub cpu: Option<..>, pub memory: Option<..> /* Docker */ }
pub struct ScriptTask { pub kind: ScriptKind, pub container_image: Option<String>, pub commands: Vec<String>,
                        pub script: Option<String>, pub before_commands: Vec<String>, pub interpreter: Option<Vec<String>>,
                        pub env: BTreeMap<String,String>, pub input_files: BTreeMap<String,String>,
                        pub output_files: Vec<String>, pub fail_fast: bool, pub warning_on_std_err: bool,
                        pub task_runner: Option<RunnerSpec> }
pub fn parse(type_id: &str, spec: &serde_json::Value) -> Result<ScriptTask, Vec<ScriptRefusal>>;  // named refusals
pub fn default_image(kind: ScriptKind) -> &'static str;             // Task 1's measurement

// scripts/gate.rs
pub struct GateInput<'a> { pub feature_enabled: bool, pub image: &'a str, pub runner: RunnerKind,
                           pub plugin_configurations: &'a PluginConfigurations }
pub fn check(input: &GateInput) -> Result<(), GateRefusal>;         // the ONE function, pure
```

- **`features.rs`:** add `default_enabled: bool` to `Feature`. Every existing key keeps `true`. `workflow_containers` has `false`, requires `workflows`, and has no route rules (it gates execution, not a URL prefix). Everywhere "absent row means enabled" is decided, it must read `default_enabled`. Find each such site (`absent_row_means_enabled` is a test name to start from).
- **Registry:** the four FQCNs become rows; `egret.container` → alias of `shell.Commands`; `egret.script.python` → alias of `python.Script`. Check that `add_publishable`'s both-directions guard still holds.
- **Publish** (`spec::validate`) calls `scripts::parse` and then `gate::check`, the latter only when the image is a literal. A `{{ }}` image is gated at run (Task 5).

- [ ] **Step 1: Failing tests**, fed from `w5a-scripts.json` where it applies:
  - every property Task 1 lists as accepted-but-unimplemented is refused, by name;
  - the `Process` runner is refused;
  - defaults match Task 1;
  - `gate::check` refuses when the feature is off, the image is outside the allowlist, the list is empty, or there is no entry for the runner type;
  - it accepts a matching glob, and a tagless image per Task 1's rule;
  - `workflow_containers` is off when its row is absent while every other key stays on;
  - publish is refused with each named error, through the route (`tests/routes.rs` style);
  - the alias `egret.container` still publishes.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Mutations:**

| mutation | must kill |
|---|---|
| allowlist check returns Ok | the outside-allowlist test |
| empty allowlist treated as allow-all | the empty-list test |
| `default_enabled` ignored (absent = on) | the feature-off-by-default test |
| Process parsed as a runner | the Process test |
| one unimplemented property silently dropped | its refusal test |

- [ ] **Step 4: Commit:** `feat(workflow): W5a — script task types, taskRunner, and the container gate`.

---

## Task 3: Lease renewal and the default timeout

**Files:**
- Modify `crates/egret-workflow/src/worker/claim.rs` (`run_one`) and the budget code (`input_for`, `Budget`).
- Tests go in `tests/worker.rs` and `tests/runtime.rs`.

**Interfaces:**

```rust
// claim.rs
async fn renew_lease(pool: &PgPool, run: Uuid, held: DateTime<Utc>) -> Result<Option<DateTime<Utc>>>; // None = fence lost
// run_one: spawn a renewal loop (every working_timeout / 3) alongside the handler future;
//   on fence lost -> cancel the handler future, do NOT apply an outcome, return Ok (the new holder owns the run).
```

- Renewal is `UPDATE workflow.task_runs SET claimed_at = now() WHERE id = $1 AND claimed_at = $2 RETURNING claimed_at` (verify the real lease column and table). Carry the new `claimed_at` into the next renewal and into `apply_outcome`'s existing fence. **`apply_outcome`'s fence must accept the renewed value**; find where it compares `claimed_at` and thread the latest one through.
- The default timeout: a script/container task with no `timeout:` gets `container_default_timeout_s`. Other handlers are unchanged.

- [ ] **Step 1: Failing tests:**
  - `a_run_longer_than_working_timeout_is_not_reaped_while_its_worker_lives`: `working_timeout` of 2 s, a handler sleeping 5 s, the reaper ticking; exactly one outcome, attempt count unchanged;
  - `a_run_whose_worker_stopped_renewing_is_reaped`;
  - `a_worker_that_loses_its_fence_records_no_outcome`;
  - `a_script_task_without_timeout_gets_the_default`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Mutations:** no renewal → the first test dies; outcome applied after a lost fence → the third test dies; `apply_outcome` still fencing on the original `claimed_at` → the first test dies (the outcome is refused).
- [ ] **Step 4:** Crate gate reconciled against `baseline-gate.txt`. Commit `feat(workflow): W5a — the worker renews its lease while a handler runs`.

---

## Task 4: `KubernetesRunner`

**Files:**
- Create `crates/egret-workflow/src/worker/k8s/{mod.rs,pod_spec.rs,runner.rs,transfer.rs}` and `deploy/docker/task-helper.Dockerfile`.
- Tests go in `tests/k8s_runner_live.rs` (gated) and unit tests in `pod_spec.rs`.
- Add `kube`/`k8s-openapi` to `egret-workflow` from the workspace; check how `egret-bin/src/kernel_launcher_k8s.rs` builds its client and copy that.

**Interfaces:**

```rust
// worker/commands.rs — the trait gains re-attach (Docker implements it too)
#[async_trait] pub trait TaskRunner { /* existing run/cancel */ 
    async fn run(&self, task: &TaskInput, cmds: &TaskCommands, spec: &RunnerSpec) -> Result<CommandResult>; // re-attaches if the named pod/container exists
}
// k8s/pod_spec.rs — pure
pub struct PodConfig { pub namespace: String, pub run_as_user: i64, pub helper_image: String,
                       pub default_resources: Resources, pub max_resources: Resources, pub image_pull_secrets: Vec<String> }
pub fn pod_name(run_id: Uuid, attempt: i32) -> String;              // egret-run-<run_id>-<attempt>
pub fn build(task: &TaskInput, cmds: &TaskCommands, spec: &RunnerSpec, cfg: &PodConfig) -> Pod;
// k8s/runner.rs
pub struct KubernetesRunner { client: kube::Client, cfg: PodConfig }
impl TaskRunner for KubernetesRunner { … }
pub async fn sweep_orphans(client: &kube::Client, cfg: &PodConfig, pool: &PgPool) -> Result<u64>;
```

**Lifecycle (spec §5):**
1. get-or-create the pod by name;
2. when there are input files and `files-in` is waiting, upload them by exec + tar (plus a ready marker) into `files-in` and verify;
3. follow `main`'s log — `sinceTime` = the attempt's last stored `task_logs.ts` when re-attaching (W11's `LogRepo`) — and feed each line to `ingest_line`/`RunLog`;
4. wait for `main` to terminate and read its exit code;
5. download `outputFiles` by exec + tar from `files-out` (the W4 caps apply);
6. delete the pod (grace 0) when `delete`;
7. `cancel` = delete with grace 0.

- [ ] **Step 1: Failing unit tests on `pod_spec::build`:**
  - name and labels;
  - `restricted` fields;
  - no SA token;
  - resources defaulted and capped;
  - pull policy and secrets;
  - an init container only when there are input files;
  - the sidecar always present;
  - `WORKING_DIR` env and the `emptyDir` mount;
  - the main args are `interpreter ++ [joined script]`, W4's rule.
- [ ] **Step 2: Failing live tests** (gated on the kind cluster):
  - `a_shell_command_runs_in_a_pod_and_the_pod_is_deleted`;
  - `input_and_output_files_round_trip`;
  - `a_nonzero_exit_fails_and_keeps_outputs`;
  - `cancel_deletes_the_pod`;
  - `a_second_run_call_for_the_same_attempt_reattaches_not_recreates` (count the pods created);
  - `the_orphan_sweep_deletes_pods_of_terminal_runs_only`.
- [ ] **Step 3: Implement.** Build the helper image and load it into kind (`kind load docker-image … --name egret`).
- [ ] **Step 4: Mutations:**

| mutation | must kill |
|---|---|
| create instead of get-or-create | the re-attach test |
| skip delete | the pod-deleted test |
| drop `runAsNonRoot` / the SA-token flag in the builder | the spec unit test |
| no resource cap | the cap unit test |
| sweep deletes pods of running runs | the orphan test |

- [ ] **Step 5: Commit** `feat(workflow): W5a — a Kubernetes task runner`.

---

## Task 5: Wiring, the run-time gate, and the chart

**Files:**
- Modify `worker/handlers/container.rs`, `runtime.rs`, `egret-config`, `crates/egret-bin/src/main.rs` and `worker/commands.rs` (DockerRunner re-attach and delete).
- Create the Helm templates.
- Tests go in `tests/scripts_gate.rs` and `tests/runtime.rs`.

- **Config (spec §7):** `container_runner`, `container_default_timeout_s`, and `kubernetes.{namespace, default_resources, max_resources, image_pull_secrets, run_as_user, helper_image}`, with doc comments in the house style of `worker_enabled`.
- **Handler:**
  - parse → render → `gate::check` (with the workspace's feature state read at run time and the rendered image) → choose the runner (`taskRunner` or the configured default) → run.
  - A refusal is `Outcome::failed_with(reason)`, and **no runner is called**.
  - `warningOnStdErr` sets WARNING (Task 1).
- **Registry:** `egret-bin` registers the four types and the aliases **only when** `container_runner != none`. The runner is built once.
- **Docker runner:** re-attach by name and delete on finish (`delete`), as the Kubernetes runner does.
- **Chart:** `workflow.containerRunner`; a namespace, Role (exactly the RBAC in Global Constraints), RoleBinding and NetworkPolicy, modelled on the `*-kernels.yaml` files.

- [ ] **Step 1: Failing tests:**
  - `a_disabled_feature_fails_the_run_and_creates_nothing` (a spy runner records calls);
  - `a_rendered_image_outside_the_allowlist_fails_the_run_and_creates_nothing`;
  - `the_default_runner_is_the_configured_one`;
  - `with_container_runner_none_the_registry_is_unchanged`;
  - `warning_on_std_err_sets_warning`;
  - Docker re-attach (`container_docker_live`, gated).
- [ ] **Step 2: Implement.** `helm template` the chart and check the Role's rules equal the constraint exactly.
- [ ] **Step 3: Mutations:** skip the run-time gate → the first two tests die; register at `none` → the registry test dies.
- [ ] **Step 4:** Crate gate. Commit `feat(workflow): W5a — script tasks wired into the runtime, and the task namespace in the chart`.

---

## Task 6: End to end on kind, coverage, sweep, full gate

- [ ] **Step 1: E2E on kind** (`just deploy-kind` or the repo's equivalent, with `workflow.containerRunner=kubernetes` and an allowlist):
  - publish and run a `shell.Commands` flow through the API;
  - read its lines through W11's logs route and its `outputFiles` from the execution;
  - check the pod was deleted;
  - kill the egret pod mid-run and show the next claim re-attaches (one pod, one outcome);
  - from a task pod, egret's Postgres is unreachable (NetworkPolicy);
  - a flow with the feature off is refused at publish.

  Record the commands and outputs.
- [ ] **Step 2: Coverage statement** in `oracle-exec/README.md`: ratchet 109 → 109 measured; what is covered live versus by unit test; the spec §11 limits.
- [ ] **Step 3: Sweep, EN and VI:**
  - the programme spec's §6.4 and §9 rows for W5;
  - §11 question 3 (now answered for containers, still open for plugin jars);
  - `egret-bin`'s "nine of thirteen types hang" comment;
  - `container.rs`/`commands.rs` doc claims that no Kubernetes runner exists.

  Report N/M per population.
- [ ] **Step 4: Full local CI** per `.github/workflows/ci.yml`:
  - fmt;
  - clippy (only denials that also exist on `origin/main`, established with evidence);
  - workspace tests;
  - the OpenAPI currency test and `npm run gen:api` if any API changed;
  - frontend checks if the client changed.

  Reconcile names against the base.
- [ ] **Step 5: Commit** `docs(workflow): W5a coverage statement and the sweep`.

---

## Final review

A whole-branch review on the most capable model. Point it at:
1. Any path that creates a pod or container without passing the gate: a draft, a rendered image, an alias type, a resumed run.
2. Double execution: lease renewal against the reaper, a lost fence, re-attach versus create, a pod of attempt N seen by attempt N+1.
3. Pod hardening and RBAC as built (`helm template`) against the constraints; NetworkPolicy measured on kind.
4. Orphan sweep deleting a live run's pod.
5. Refusals at save that silently accept an unimplemented property.
6. Kestra fidelity against `w5a-scripts.json`.
