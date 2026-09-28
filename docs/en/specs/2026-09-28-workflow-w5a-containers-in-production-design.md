# W5a — Script tasks and task runners: containers that run in production

**Status:** design, awaiting review
**Depends on:** W4 (container handler, `TaskRunner`/`DockerRunner`), W10 (runtime), W11 (`RunLog`, logs API) — all merged. Base `8f35f85a`.
**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §3.1, §6.4, §9 (row W5), §11 open question 3
**Decided with the user, 2026-09-28:**
- **W5 is split.** W5a (this spec) makes container tasks run in production. W5b (the Kestra plugin host image, the Java shim, `kestra.<FQCN>` dispatch and the plugin-jar allowlist) follows and rides on W5a. Storage- and secret-backed plugins wait for W9.
- **Follow Kestra's shape.** Kestra's script tasks carry a `taskRunner:` property; W5a ports those task types and that property rather than growing egret's own `egret.container` type.
- **Governance:** a per-workspace feature `workflow_containers`, **off by default**, plus a global image allowlist set by the platform admin. Both are stricter than Kestra OSS, which has neither; that is deliberate and written down.
- **Where containers run:** a Kubernetes pod per task in production; Docker stays for local development and tests.
- **Time:** the worker renews its lease while a handler runs (Kestra's heartbeat), and a container task with no `timeout:` gets a configured default (1 h).
- **Isolation:** standard Kubernetes hardening, egress open, except to egret's metadata Postgres and the Kubernetes API.

**Delivery:** one PR (docs and code), tasks in §8.

## 1. What exists and what does not

| piece | state on `origin/main` (`8f35f85a`) |
|---|---|
| container handler | `egret.container` and `egret.script.python` (W4). Contract: `image`, `commands`, `beforeCommands`, `env`, `interpreter` (default `/bin/sh -c`), `outputFiles`, `failFast`; outputs via `::{json}::` lines; output files inline, capped at 1 MiB |
| runners | `TaskRunner` trait (`worker/commands.rs:286`); **one** implementation, `DockerRunner`, driving the Docker CLI (`create`, `cp`, `start --attach`, `cp`, `rm`), container named `egret-run-<run_id>-<claimed_at_micros>` |
| production registry | `Handlers::with_core_tasks()` = `Log`, `Return`, `Fail`, `Sleep`. **Container and python are not registered**; `egret-bin`'s own comment says 9 of 13 publishable types hang in production |
| governance | none. Feature keys (`egret-workspace/src/features.rs`) have **"absent row = enabled"** semantics, so no existing key can be off by default |
| time | no default time budget for container/python; the reaper reclaims a run after `working_timeout_s` (default **3600 s**), so a container task running over an hour is re-run while its first run is still going (W4's recorded hazard) |
| Kubernetes precedent | the notebook kernel launcher (`egret-bin/src/kernel_launcher_k8s.rs`, `kube` crate) with `namespace-kernels.yaml`, `rbac-kernels.yaml` (`pods` get/list/watch/create/delete, `pods/log` get), `networkpolicy-kernels.yaml` |

## 2. What Kestra does, and where egret differs

Kestra (checkout `96893b589`, image `kestra/kestra:v2.0.0-rc13`):
- **Script tasks** (`io.kestra.plugin.scripts.shell.Commands`, `shell.Script`, `python.Commands`, `python.Script`) extend `AbstractExecScript`. They separate *what* runs (the task) from *where* (the `taskRunner:` property). The default runner is **Docker** (`AbstractExecScript.java:52-58`, `pullPolicy: IF_NOT_PRESENT`).
- **`TaskRunner`** (`core/.../tasks/runners/TaskRunner.java`) runs a `TaskCommands` (image, interpreter, before-commands, commands, env, working/output directory, timeout); `{{ workingDir }}` and the `WORKING_DIR` env var point at the working directory.
- **Runners:** `Process` (a local subprocess, in core); `io.kestra.plugin.scripts.runner.docker.Docker` (labels `kestra.io/*`, tar archives in and out, `resume: true` re-attaches by label after a worker restart, `delete: true`); **Kubernetes as a task runner is Enterprise-only** (`io.kestra.plugin.ee.kubernetes.runner.Kubernetes`). OSS ships `io.kestra.plugin.kubernetes.core.PodCreate`, which moves files with an **init container and a files sidecar**, upload and download by exec + tar with ready/ended markers, and re-attaches by label.
- **Double-run protection:** worker heartbeats (`kestra.server.liveness`, 3 s, 45 s timeout); a task is resubmitted only once its worker is confirmed dead, and container/pod runners re-attach rather than recreate.
- **Governance:** OSS has **no** plugin, image or runner allowlist. `kestra.plugins.configurations` (`{type, values}` per plugin type, read by `RunContext.pluginConfiguration`) is the only knob; Docker's `volume-enabled` defaults to `false` through it.

| Kestra | egret W5a | why |
|---|---|---|
| default `taskRunner` is Docker | the default is the configured `container_runner` (`kubernetes` in production, `docker` locally) | egret runs on Kubernetes; a Docker socket in the server is root on the node |
| `Process` runner | **refused at save** | it would run arbitrary code inside `egret-server` |
| Kubernetes runner is Enterprise | egret implements it under the same FQCN | a flow written for Kestra EE runs unchanged |
| no image allowlist, no per-tenant switch | feature `workflow_containers` (default off) + `allowed-images` in `plugin_configurations` | user decision: stricter than Kestra OSS |
| containers identified by labels only | pods also get a **deterministic name** | cancel and re-attach from any worker, the reason egret's Docker runner already names containers |
| heartbeat + liveness coordinator | the worker renews its lease (`claimed_at`) every `working_timeout / 3`; the reaper already reclaims only expired leases | the same guarantee on egret's existing claim model |
| output files → internal-storage URIs | inline contents, capped (as W4) | internal storage is W9 |

## 3. Task types and properties

Ported, with Kestra's FQCNs: `io.kestra.plugin.scripts.shell.Commands`, `io.kestra.plugin.scripts.shell.Script`, `io.kestra.plugin.scripts.python.Commands`, `io.kestra.plugin.scripts.python.Script`. `egret.container` and `egret.script.python` remain as **aliases** of `shell.Commands` and `python.Script` (existing flows keep working).

Properties implemented: `containerImage`, `commands` (Commands) / `script` (Script), `beforeCommands`, `interpreter`, `env`, `inputFiles` (inline contents only), `outputFiles`, `failFast`, `warningOnStdErr`, `timeout`, `taskRunner`.

- **Default images** per task type are Kestra's, **measured in Task 1** (`oracle-exec/w5a-scripts.json`): `ubuntu` for `shell.*` and `python:3.13-slim` for `python.*`. They must pass the allowlist like any other image.
- **Refused at save**, with a named error, never silently ignored: `namespaceFiles`, `outputDirectory`, `inputFiles` given as a storage URI, `targetOS`, and any other property Task 1 finds the image accepts that W5a does not implement (measured: the deprecated `runner`, and on `python.*` `dependencies`, `dependencyCacheEnabled`, `packageManager`, `pythonVersion`, `uvAutoInstallEnabled`, `uvInstallerSha256`, `uvInstallerVersion`; the per-type lists are `egret_expected.task_types` in the fixture). Kestra itself refuses an unknown property at save (`Unrecognized field …`, measured).
- **Base-task properties on script types.** egret's `Task` model reads `id`, `type`, `description`, `retry`, `timeout`, `disabled`, `runIf`, `allowFailure` and `allowWarning`. The other seven that Kestra accepts on every task (`assets`, `logLevel`, `logToFile`, `policyRefs`, `taskCache`, `version`, `workerSelector`) are dropped by egret without effect, so on the four script types (and their aliases) they are **refused at save** like the properties above. `version` is refused on the Docker runner as well.
- **Required at save:** `commands` (Commands) or `script` (Script), measured. `taskRunner` is not: the schema marks it `$required`, but a flow without it (or with `taskRunner: null`) saves and runs on the default runner.
- `warningOnStdErr` is accepted and has **no effect**, as on the artifact. *Amended by Task 1's measurement:* with `true`, `false` or unset, a script that writes to stderr and exits 0 ends **SUCCESS**, and the stderr line is logged at ERROR (as W11 measured); Kestra's field reads `"Not used anymore, will be removed soon"`. An earlier draft said it set WARNING; that was not measured.

`taskRunner:` accepts:
- `io.kestra.plugin.scripts.runner.docker.Docker`: `pullPolicy`, `cpu` (only `cpu.cpus`), `memory` (only `memory.memory`; `kernelMemory`, `memoryReservation`, `memorySwap`, `memorySwappiness` and `oomKillDisable` are refused), `delete` (default `true`). Other Kestra Docker properties (volumes, privileged, networkMode, …, and the schema-hidden `image` that Kestra still accepts) are refused at save.
- `io.kestra.plugin.ee.kubernetes.runner.Kubernetes`: `pullPolicy`, `resources` (`request`/`limit` for cpu and memory, capped by config), `delete` (default `true`). `resources` accepts exactly `request.cpu`, `request.memory`, `limit.cpu` and `limit.memory`; any other key, at any level, is refused. Namespace, service account, node selector and similar are **not** author-settable; they come from config.
- `io.kestra.plugin.core.runner.Process`: **refused at save**.
- Absent: the configured default runner.

## 4. Governance

- **Feature `workflow_containers`**, requiring `workflows`. It is the first feature that is **off by default**: the feature catalog gains a `default_enabled: bool`, and for this key an absent row means *disabled*. Existing keys keep `true`, so their behaviour does not change.
- **Image allowlist:** `workflow.plugin_configurations`, a list of `{type, values}` keyed by runner FQCN (Kestra's `kestra.plugins.configurations` shape). `values.allowed-images` is a list of glob patterns matched against the image reference as written, after defaulting the tag to `latest` (measured in Task 1: `containerImage: python` makes Docker pull `python:latest`). **No entry, or an empty list, allows no image.** Normalisation before matching, exactly:
  - nothing is added or stripped: no `docker.io/` and no `library/` (`docker.io/library/python:3.13-slim` does not match `python:*`);
  - a digest reference (`name@sha256:…`) is matched as written, with no tag added (measured: the artifact ran `python@sha256:…` as written);
  - otherwise the tag is the part after a `:` that follows the last `/`, so the port in `host:5000/img` is not a tag; a reference with no tag gets `:latest` (`host:5000/img` → `host:5000/img:latest`);
  - the default images are matched the same way: `ubuntu` → `ubuntu:latest`, `python:3.13-slim` unchanged.
  - *Glob semantics (added in Task 2's fix round 1):* `*` matches any run of characters **except `/`**, `?` one character other than `/`, and there is no `**`. A star therefore cannot reach another registry host: `python:3*` does not match `python:3000/x/img` (host `python`, port 3000), and `registry.company.com/*` does not match `registry.company.com.evil.io/x`. Allowing a deeper path under a registry takes one `*` per level (`registry.company.com/*/*`).
- **One gate function, two call sites** (the W13b lesson: drafts save despite validation errors and still run):
  - **at publish**, a literal image outside the allowlist, a disabled feature or a refused runner/property is a named 400;
  - **at run**, after rendering (the image may be a `{{ }}` expression), the handler calls the same function. On refusal the task is FAILED with the reason, and **no pod or container is created**.
- Turning the feature off stops new runs at the run-time gate; runs already in a pod finish.

## 5. The Kubernetes runner

**Pod, one per task-run attempt:**
- name `egret-run-<run_id>-<attempt>`; labels `egret.io/workspace-id`, `egret.io/execution-id`, `egret.io/taskrun-id`, `egret.io/attempt`;
- namespace `workflow.kubernetes.namespace` (pod security `restricted`);
- `automountServiceAccountToken: false`, `runAsNonRoot: true`, `runAsUser: workflow.kubernetes.run_as_user`, dropped capabilities, `seccompProfile: RuntimeDefault`;
- resources: config defaults, author requests capped by `max_resources`;
- `imagePullPolicy` from `pullPolicy`; `imagePullSecrets` from config;
- a shared `emptyDir` as the working directory; `WORKING_DIR` env var set to it.

**Containers**, after Kestra's `PodCreate`:
1. init `files-in` (the helper image, only when there are input files): waits for egret to upload the files by exec + tar, verified by a marker, then exits;
2. `main`: the flow's image, running `interpreter [beforeCommands + commands]` in the working directory;
3. sidecar `files-out` (the helper image): keeps the pod alive after `main` exits so egret can download `outputFiles` by exec + tar.

**Lifecycle:** create; upload input files; follow `main`'s log (`pods/log?follow`) into `ingest_line` → `RunLog` (W11); wait for `main` to terminate and read its exit code from the pod status; download output files; **delete the pod** (unless `delete: false`), on failure too.

**Timeout and kill:** delete the pod with grace 0; on a timeout the task is FAILED with W11's `Timeout after …` line.

**Re-attach:** on claim, look the pod up by name. Running: re-attach and follow the log from `sinceTime` = the run attempt's last stored line (W11), so few lines repeat. Terminated: download outputs and delete. Absent: create.

**Orphans:** the reaper, on its existing tick, deletes pods carrying egret's labels whose run is terminal (egret died between downloading outputs and deleting).

**RBAC:** a Role in the task namespace: `pods` get/list/watch/create/delete, `pods/log` get, `pods/exec` create; bound to egret's ServiceAccount.

**NetworkPolicy:** task pods may not reach egret's metadata Postgres or the Kubernetes API server; all other egress is open.

The Docker runner gets the same re-attach-by-name and delete-on-finish behaviour; it already names containers.

## 6. Lease renewal and the default timeout

- While a handler runs, the worker renews its lease every `working_timeout_s / 3`: `UPDATE … SET claimed_at = now() WHERE id = $run AND claimed_at = $held` (a fence). The reaper, unchanged, reclaims only leases older than `working_timeout_s`, so a live worker's run is never reclaimed and a dead worker's run is reclaimed within `working_timeout_s`.
- If a renewal finds the fence moved (another worker holds the run), the worker stops following the task and records no outcome; the pod is left for the new holder to re-attach.
- Applies to every handler, not only containers, so a long `egret.sql` is also safe.
- A container or script task with no `timeout:` runs under `workflow.container_default_timeout` (default **1 h**).

## 7. Configuration and deployment

`egret_config::WorkflowConfig` (env `EGRET_WORKFLOW__…`):
- `container_runner`: `none` | `docker` | `kubernetes`, default `none`. With `none` the production registry is unchanged; otherwise the four script types and the two aliases are registered.
- `container_default_timeout_s` (3600).
- `plugin_configurations`: `[{type, values}]`.
- `kubernetes`: `namespace`, `default_resources`, `max_resources`, `image_pull_secrets`, `run_as_user`, `helper_image`.

Helm chart: `workflow.containerRunner`; a task namespace, Role, RoleBinding and NetworkPolicy, modelled on `namespace-kernels.yaml`, `rbac-kernels.yaml` and `networkpolicy-kernels.yaml`. The helper image is built from a Dockerfile in the repo.

## 8. Tasks

1. **Measure on the artifact**: the four script task types' default images; accepted and refused properties; `outputFiles` result shape; inline `inputFiles`; `warningOnStdErr`; `failFast`; a tagless image. Fixture `oracle-exec/w5a-scripts.json`.
2. **Model and governance:** the four types plus aliases; `taskRunner`; save-time refusals; `default_enabled` in the feature catalog and the `workflow_containers` key; `plugin_configurations`; the one gate function, called at publish.
3. **Lease renewal and the default timeout** in the worker.
4. **`KubernetesRunner`:** pure pod-spec builder; lifecycle with exec+tar file transfer, log following, re-attach, delete; the orphan sweep; the helper image.
5. **Wiring:** config; the registry in `egret-bin`; the Docker runner as `taskRunner` Docker; the run-time gate; the Helm chart.
6. **End to end on kind**; coverage statement; EN/VI sweep; full local CI.

## 9. Testing

| test | mutation it kills |
|---|---|
| a `shell.Commands` flow runs in a pod on kind, its lines readable through W11's API, its `outputFiles` returned, the pod deleted | the pod not deleted / logs not followed |
| a worker killed mid-run: the next claim re-attaches to the same pod; one pod ever exists, one outcome recorded | create instead of re-attach |
| a task longer than `working_timeout` is not reaped while its worker lives; reaped after the worker dies | no lease renewal |
| timeout deletes the pod, FAILED with `Timeout after …` | timeout without pod deletion |
| feature off → refused at publish and at run, no pod created | the run-time gate skipped |
| image outside the allowlist refused; empty allowlist refuses all | allowlist check dropped |
| `Process` runner refused at save | Process accepted |
| from a task pod, egret's Postgres is unreachable | NetworkPolicy missing |
| pod spec: `restricted` fields, no SA token, resources capped | a field dropped from the builder |

Live Kubernetes tests run only with a kind cluster available, gated by an environment variable, as `container_docker_live` is gated on Docker. **Ratchet:** expected **109 → 109**, measured; the corpus has no script task.

## 10. Out of scope

W5b (plugin host image, shim, `kestra.<FQCN>`, jar allowlist); internal storage, `namespaceFiles`, storage-backed `inputFiles`/`outputFiles` URIs, secrets (W9); per-workspace image allowlists; `volumes`, `privileged`, custom service accounts; Windows `targetOS`; Kestra's other script languages (node, R, …), addable later as the same shape.

## 11. Known limits

- A pod that outlives egret entirely (egret uninstalled) is not cleaned up by egret.
- Re-attach may repeat a few log lines around the reclaim instant.
- `pods/exec` in the task namespace lets egret exec into task pods; the namespace holds nothing else.
