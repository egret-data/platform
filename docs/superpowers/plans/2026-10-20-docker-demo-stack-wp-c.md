# Docker demo stack WP-C: `DockerEngineProvisioner` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** *Add engine* (Trino) works in the Docker demo stack: egret starts, reports, updates, restarts and deletes Trino containers through the Docker Engine API, and the Control Plane monitor can read their queries. No Kubernetes.

**Architecture:** A new `egret_provisioner::docker` module implements the existing `egret_engine_mgmt_api::EngineProvisioningPort`. It **reuses the Kubernetes renderers**: it calls `trino_config_map`, `trino_catalogs_config_map`, `trino_env_secret`, `trino_tls_secret` and `trino_deployment` (all kube-free, they return `serde_json::Value`) and turns their output into one `ContainerPlan` per role. Config files reach the container by **tar upload** (`PUT /containers/{id}/archive`) before start, so egret needs no shared filesystem and no host path. All Docker calls go through a small `DockerApi` trait (fake in unit tests, `bollard` in production). The provisioner is chosen by a new `[engines] provisioner` key; Control Plane coordinator requests get a plain-HTTP fallback when no kube client exists.

**Tech Stack:** Rust (egret workspace), `bollard` 0.21 (already in `Cargo.lock` through `testcontainers`), `tar`, `tecnativa/docker-socket-proxy`, Docker Compose, Playwright (Node library, as in WP-B).

**Spec:** `docs/superpowers/specs/2026-10-20-docker-demo-stack-design.md` (WP-C and "Notes from WP-A for WP-C").

## Decisions that differ from the spec (made while reading the code)

1. **Tar upload instead of bind-mounted work dir.** The spec has egret write `<work_dir>/<name>/` and bind-mount it. egret runs in a container, so that path would be a path *inside egret*, not on the Docker host, and would need a shared named volume with `subpath`. Uploading the files to the created container removes the host-path problem and the "bind-mount only its own dir" rule: engine containers get **no mounts at all**. `engines.docker.work_dir` is dropped.
2. **Address aliases instead of changing engine addressing.** `Engine::coordinator_host()` is `{name}-trino.{namespace}.svc` (`crates/egret-engine-mgmt/src/store.rs:100`), also the SAN of the generated TLS cert (`engines.rs:2248`). The coordinator container joins the compose network with the aliases `{name}-trino`, `{name}-trino.{namespace}` and `{name}-trino.{namespace}.svc`, so query, TLS and monitor addressing work unchanged.
3. **No frontend change.** The spec asked the UI to say "Trino only". There is no capabilities endpoint, so the server answers a StarRocks create under the Docker provisioner with a clear 400 and the guide says Trino only. Adding an endpoint plus UI for it is out of scope.
4. **`catalog.public_url` stays as it is.** The demo already sets it to `http://egret:8080` (WP-A). `engines.docker.callback_url` is added as an optional override of `engine_egret_url` (the value baked into engine catalog properties) so a deployment can separate the two; the JWKS URI is still derived from `catalog.public_url`. The live task checks that JWT delegation works with the demo values.
5. **Pod logs, follow and exec** stay Kubernetes-only. Under Docker they answer 400 "not available with the Docker engine provisioner". A failed engine's `message` carries the last log lines instead.

## Global Constraints

- Provisioner key: `[engines] provisioner = "auto" | "kubernetes" | "docker" | "none"`, env `EGRET_ENGINES__PROVISIONER`. `auto` (default) keeps today's behaviour exactly: try in-cluster kube, then kubeconfig, else disabled. `docker` is never auto-selected.
- Docker config: `[engines.docker]` keys `host` (default `unix:///var/run/docker.sock`; `tcp://…` allowed), `network` (required with `docker`), `project` (label value, default empty), `callback_url` (optional), `allowed_images` (default `["trinodb/trino"]`). List values in env are JSON arrays.
- Labels on every provisioned container: `egret.managed=true`, `egret.project=<engines.docker.project>`, `egret.engine=<name>`, `egret.namespace=<namespace>`, `egret.role=coordinator|worker`. egret **never touches a container without `egret.managed=true`**, and never one whose `egret.project` differs from its own.
- Container name: `egret-engine-{namespace}-{name}-coordinator` and `…-worker-{i}` (i from 0). Coordinator aliases: `{name}-trino`, `{name}-trino.{namespace}`, `{name}-trino.{namespace}.svc`.
- Security options on every container: `no-new-privileges`, `cap_drop = ["ALL"]`, **no published ports, no mounts, no privileged, no host network**. Image must match `allowed_images` (repository part, before the tag or digest); otherwise provision/update fail before anything is created.
- Unsupported options are **rejected with an error naming them**, never silently ignored: `ingress.enabled`, `autoscaling`, extra volume mounts, init containers, extra containers, `image_pull_secret_json`, `engine.kind == starrocks`.
- Status mapping (`EngineStatus.phase`): no containers → `provisioning`; any `exited`/`dead` → `failed` with the last 20 log lines as `message`; all `running` and healthy → `running`; otherwise `provisioning`. `PodStatus` is one per container: `name` = container name, `phase` = `Running|Pending|Failed|Succeeded`, `ready` = running and healthy.
- The Kubernetes path must be byte-for-byte unchanged: no edit to `trino.rs` renderers except making `Role` and helpers `pub` if the Docker module needs them. Existing tests keep passing.
- Docker is needed only by the gated live tests (visible "ignored" report, not a silent pass; same hand-rolled `main` convention as `crates/egret-workflow/tests/container_docker_live.rs`).
- Never commit secrets. Conventional Commits. `Closes #N` only in PR descriptions. Scratch files in the scratchpad.
- Docs in the same PR: `docs/en/guides/demo-stack.md` (Add engine row, Docker socket safety, Trino only, `engines.*` keys). The `egret-data/docs` follow-up is a separate PR.
- Do not touch containers named `egret-dev-*` or `egretdocs-*`. Use only the `demo-*` just recipes. Test with `DEMO_PORT=18080 DEMO_URL=http://egret.localtest.me:18080 DEMO_APPS_URL=http://apps.localtest.me:18080` (port 8080 is taken on the dev machine); never commit those values.

## File Structure

| File | Responsibility |
|---|---|
| `crates/egret-config/src/lib.rs` | `EnginesConfig`, `EnginesDockerConfig`, `Provisioner` enum, validation |
| `crates/egret-provisioner/src/docker/mod.rs` | module root, re-exports |
| `crates/egret-provisioner/src/docker/plan.rs` | pure: `EngineSpec` → `Vec<ContainerPlan>` (reuses Trino renderers), quantity parsing, tar building |
| `crates/egret-provisioner/src/docker/api.rs` | `DockerApi` trait, DTOs, in-memory `FakeDocker` |
| `crates/egret-provisioner/src/docker/provisioner.rs` | `DockerEngineProvisioner` (`EngineProvisioningPort`), guards, status mapping |
| `crates/egret-provisioner/src/docker/bollard_api.rs` | `BollardDocker: DockerApi` |
| `crates/egret-provisioner/tests/docker_live.rs` | gated live lifecycle test |
| `crates/egret-engine-mgmt-api/src/lib.rs` | `CoordinatorTransport` trait |
| `crates/egret-engine-mgmt/src/{state,monitor}.rs` | state field + fallback in `coordinator_call` |
| `crates/egret-bin/src/main.rs` | provisioner selection, transport wiring, `engine_egret_url` override |
| `deploy/docker-compose.demo.yml`, `deploy/demo/*`, `justfile` | socket proxy, env, label filters, verify recipe |
| `deploy/demo/verify-engine.mjs` | Playwright *Add engine* check |
| `docs/en/guides/demo-stack.md` | guide updates |

---

### Task 1: `[engines]` configuration

**Files:**
- Modify: `crates/egret-config/src/lib.rs` (add structs, `Config.engines` with `#[serde(default)]`, validation, tests)
- Modify: `.env.example` only if it lists sections (it should not gain secrets)

**Interfaces:**
- Produces: `egret_config::{EnginesConfig, EnginesDockerConfig, Provisioner}`; `Config.engines: EnginesConfig`.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provisioner {
    #[default]
    Auto,
    Kubernetes,
    Docker,
    None,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnginesDockerConfig {
    #[serde(default = "default_docker_host")]
    pub host: String,                 // "unix:///var/run/docker.sock"
    #[serde(default)]
    pub network: String,              // required when provisioner = docker
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub callback_url: Option<String>,
    #[serde(default = "default_allowed_images")]
    pub allowed_images: Vec<String>,  // ["trinodb/trino"]
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EnginesConfig {
    #[serde(default)]
    pub provisioner: Provisioner,
    #[serde(default)]
    pub docker: EnginesDockerConfig,  // needs a Default impl using the two default fns
}
```

- [ ] **Step 1: Write failing tests** next to the existing `engine_defaults_present_and_validate` test, using the same `figment::Jail` pattern the file already uses for env tests:
  - default config: `provisioner == Auto`, `docker.host == "unix:///var/run/docker.sock"`, `allowed_images == ["trinodb/trino"]`, `validate()` ok.
  - `EGRET_ENGINES__PROVISIONER=docker` with empty `network` → `validate()` is `Err` containing `engines.docker.network`.
  - `docker` with `network = "egret-demo_default"` → ok.
  - `EGRET_ENGINES__DOCKER__ALLOWED_IMAGES='["trinodb/trino","acme/trino"]'` parses to two entries.
  - `EGRET_ENGINES__PROVISIONER=bogus` → load error.
  - `docker` with `host = "ftp://x"` → `validate()` Err (only `unix://` and `tcp://` accepted).
  - `docker` with an empty `allowed_images` → Err.
- [ ] **Step 2: Run** `cargo test -p egret-config engines` → FAIL (types missing).
- [ ] **Step 3: Implement** the structs, the `Default` impl for `EnginesDockerConfig`, add `#[serde(default)] pub engines: EnginesConfig` to `Config` (and to the struct literal in the existing `Config` constructor near line 1692), and the three validation rules above inside `Config::validate`. Doc-comment each key the way neighbouring keys are documented (what it does, default, env name).
- [ ] **Step 4: Run** `cargo test -p egret-config` → PASS; `cargo clippy -p egret-config -- -D warnings`.
- [ ] **Step 5: Commit** `feat(config): [engines] provisioner selection and Docker settings`

---

### Task 2: Pure plan renderer (`EngineSpec` → `ContainerPlan`)

**Files:**
- Create: `crates/egret-provisioner/src/docker/{mod.rs,plan.rs}`
- Modify: `crates/egret-provisioner/src/lib.rs` (`pub mod docker;`), `crates/egret-provisioner/src/trino.rs` (visibility only: `pub fn trino_deployment` already pub; make `Role` constructors usable, nothing else), `crates/egret-provisioner/Cargo.toml` (add `tar = "0.4"`; it is not in `Cargo.lock` yet, so this is a new crate, state it in the PR)

**Interfaces:**
- Consumes: `trino_config_map`, `trino_catalogs_config_map`, `trino_env_secret`, `trino_tls_secret`, `trino_deployment(spec, Role)` (existing).
- Produces:

```rust
pub struct PlanFile { pub path: String, pub content: Vec<u8>, pub mode: u32 }

pub struct ContainerPlan {
    pub name: String,
    pub role: PlanRole,                       // Coordinator | Worker(u32)
    pub image: String,
    pub env: Vec<(String, String)>,           // sorted by key
    pub labels: std::collections::BTreeMap<String, String>,
    pub aliases: Vec<String>,                 // coordinator only
    pub files: Vec<PlanFile>,                 // absolute container paths, sorted by path
    pub nano_cpus: Option<i64>,
    pub memory_bytes: Option<i64>,
}

pub struct PlanOptions<'a> { pub project: &'a str, pub allowed_images: &'a [String] }

pub fn plan_trino(spec: &EngineSpec, opts: &PlanOptions) -> Result<Vec<ContainerPlan>, String>;
pub fn parse_cpu(q: &str) -> Result<i64, String>;      // "500m" -> 500_000_000, "2" -> 2_000_000_000
pub fn parse_memory(q: &str) -> Result<i64, String>;   // "2Gi" -> 2147483648, "512Mi", "1G", plain bytes
pub fn files_tar(files: &[PlanFile]) -> Vec<u8>;       // uid/gid 1000, dir entries first
pub fn image_allowed(image: &str, allowed: &[String]) -> bool;
pub fn container_name(namespace: &str, name: &str, role: &PlanRole) -> String;
```

How files are derived (this is the heart of the task, no forking of renderers): render `trino_deployment(spec, role)`. For each `volumeMounts` entry:
- volume `config` with `subPath` S → file content is `trino_config_map(spec)["data"][S]`, path = `mountPath`;
- volume `catalogs` (a directory mount) → one file `"{mountPath}/{file}"` per entry of `trino_catalogs_config_map(spec)["data"]`;
- volume `tls` with `subPath` `keystore.pem` → `trino_tls_secret(spec)["stringData"]["keystore.pem"]`, mode `0o600`;
- volume `data` → ignored (the image already has `/data/trino`, owned by uid 1000);
- anything else → `Err("unsupported volume mount <mountPath>")`.
`password.db` and the keystore get mode `0o600`, everything else `0o644`. Env is `trino_env_secret(spec)["stringData"]`. Resources come from the same `RoleConfig` (`cpu_limit`, `mem_limit`; request values are ignored on Docker). One coordinator plan plus `spec.config.worker.base.replicas` worker plans. The rejection list from Global Constraints is checked first, in one function `unsupported(spec) -> Vec<&'static str>` so the message names every offender.

- [ ] **Step 1: Write failing tests** in `plan.rs` (`#[cfg(test)]`), using a `sample_spec()` copied from `trino.rs` tests (`sample_spec`, line ~1308; copy the builder, do not make the test helper `pub`):
  - `parse_cpu("500m")`, `("2")`, `("0.5")`, bad input; `parse_memory("2Gi")`, `("512Mi")`, `("1G")`, `("1048576")`, bad input.
  - `plan_trino` on a one-coordinator spec (workers 0) → exactly 1 plan, name `egret-engine-egret-engines-<name>-coordinator`, aliases exactly the three from Global Constraints, labels exactly the five.
  - the plan's `/etc/trino/config.properties` equals `config-coordinator.properties` from `trino_config_map` (read the renderer, not a string literal); `/etc/trino/catalog/<c>.properties` equals the catalogs ConfigMap entry; `/etc/trino/jvm.config` equals `jvm-coordinator.config`.
  - golden tie to the Kubernetes mounts: for a spec with ranger, password users, resource groups, session properties, event listener and fault tolerance, the set of plan file paths equals the set of `mountPath`s of `trino_deployment` minus `/data/trino` (this is the test that fails the day someone adds a mount to the k8s renderer and forgets Docker).
  - TLS spec → `/etc/trino/tls/keystore.pem` present with mode `0o600` on the coordinator only.
  - workers: `worker.base.replicas = 2` → plans `…-worker-0`, `…-worker-1`, no aliases, `/etc/trino/config.properties` equals `config-worker.properties` (its `discovery.uri` is `http://<name>-trino:8080`, which resolves through the coordinator alias).
  - env contains the catalog env keys (e.g. an S3 secret key) and is sorted.
  - rejection: ingress enabled, autoscaling set, `image_pull_secret_json` set, an `ObjectMount`/extra volume, `kind == StarRocks` → `Err` whose text names each; two offenders → both named.
  - `image_allowed("trinodb/trino:483", ["trinodb/trino"])` true; `"trinodb/trino@sha256:…"` true; `"trinodb/trinoevil:1"` false; `"evil/trinodb/trino:1"` false; `"docker.io/trinodb/trino:483"` false (exact repository match, no registry-prefix guessing).
  - `files_tar`: round-trip through `tar::Archive` → same paths, contents, modes; every entry uid/gid 1000; parent directories present before files.
- [ ] **Step 2: Run** `cargo test -p egret-provisioner docker::plan` → FAIL.
- [ ] **Step 3: Implement** per the interface block. Keep `plan.rs` free of any Docker/bollard type so it stays testable without Docker.
- [ ] **Step 4: Run** `cargo test -p egret-provisioner` (the whole crate: Kubernetes renderer tests must still pass) and clippy.
- [ ] **Step 5: Commit** `feat(provisioner): render Trino containers for Docker from the Kubernetes renderers`

---

### Task 3: `DockerApi` seam and `DockerEngineProvisioner`

**Files:**
- Create: `crates/egret-provisioner/src/docker/{api.rs,provisioner.rs}`
- Modify: `crates/egret-provisioner/src/docker/mod.rs`, `crates/egret-provisioner/Cargo.toml` (dependency on `egret-engine-mgmt-api` is already there through `spec`)

**Interfaces:**
- Consumes: Task 2 (`ContainerPlan`, `plan_trino`, `files_tar`, `PlanOptions`).
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Created,
    Restarting,
    Running { healthy: Option<bool> },   // None = no health check configured
    Exited { code: i64 },
    Dead,
}

#[derive(Debug, Clone)]
pub struct ContainerInfo { pub name: String, pub labels: BTreeMap<String, String>, pub state: ContainerState }

#[async_trait::async_trait]
pub trait DockerApi: Send + Sync {
    async fn ensure_image(&self, image: &str) -> Result<(), String>;          // pull only when absent locally
    async fn create_and_start(&self, plan: &ContainerPlan, network: &str) -> Result<(), String>; // create, upload tar, start; removes the container if any later step fails
    async fn list(&self, label_filter: &BTreeMap<String, String>) -> Result<Vec<ContainerInfo>, String>;
    async fn restart(&self, name: &str) -> Result<(), String>;
    async fn stop_and_remove(&self, name: &str) -> Result<(), String>;        // missing container is Ok
    async fn log_tail(&self, name: &str, lines: usize) -> Result<String, String>;
}

pub struct FakeDocker { /* Mutex<state>; records calls; fail_next(op) hooks */ }

pub struct DockerEngineProvisioner { /* Arc<dyn DockerApi>, network, project, allowed_images */ }
impl DockerEngineProvisioner {
    pub fn new(api: Arc<dyn DockerApi>, network: String, project: String, allowed_images: Vec<String>) -> Self;
}
// impl egret_engine_mgmt_api::EngineProvisioningPort for DockerEngineProvisioner
```

Behaviour:
- `provision(spec)`: `kind` must be Trino (else `Err("the Docker engine provisioner supports Trino only")`); `plan_trino`; `ensure_image`; if any container with this engine's labels already exists → `Err("engine <name> already has containers")` (idempotence is for `update`); create coordinator first, then workers; on any failure remove everything created in this call and return the error.
- `status(kind, ns, name)`: `list` filtered by `egret.managed=true`, `egret.project=<project>`, `egret.engine=<name>`, `egret.namespace=<ns>`; apply the status mapping from Global Constraints; for `failed`, `message` = `log_tail(first failed container, 20)`.
- `update(spec)`: validate and plan **first** (a bad spec must not destroy the running engine), then remove all current containers of the engine, then create the new ones. Missing containers are fine (works as a provision).
- `delete`: `stop_and_remove` for every container returned by the labelled `list`; nothing else; no error when none exist.
- `restart`: `restart` each labelled container, coordinator first.
- Containers whose labels lack `egret.managed=true` or carry another `egret.project` are never returned by the filtered `list` in `FakeDocker`, and `DockerEngineProvisioner` additionally re-checks the labels on every `ContainerInfo` it acts on (defence in depth, tested).

- [ ] **Step 1: Write failing tests** against `FakeDocker`:
  - provision creates coordinator before workers, calls `ensure_image` once per distinct image, uses the configured network.
  - provision rejects StarRocks, a disallowed image (nothing created, `ensure_image` not called), and unsupported options (message names them).
  - provision failure on the second container removes the first (no leftovers) and returns the error.
  - provision twice → second is an error, the first engine untouched.
  - status table: none → `provisioning`; `Created` → `provisioning`; `Running{healthy:None}` → `running`; `Running{healthy:Some(false)}` → `provisioning`; `Running{Some(true)}` + `Exited{1}` worker → `failed` with the fake's log text in `message`; `Restarting` → `provisioning`; `Dead` → `failed`. `pods` has one entry per container with the phase and `ready` from Global Constraints.
  - update with an invalid spec leaves the old containers running (assert `stop_and_remove` not called); update with a valid spec replaces them (new plan content observable in the fake); update when nothing exists behaves as provision.
  - delete removes all of this engine's containers and ignores a foreign container (no `egret.managed` label) and a container of another project with the same engine name.
  - delete twice is Ok; restart restarts coordinator then workers.
- [ ] **Step 2: Run** `cargo test -p egret-provisioner docker::provisioner` → FAIL.
- [ ] **Step 3: Implement** `api.rs` (trait, DTOs, `FakeDocker` under `#[cfg(any(test, feature = "test-util"))]` only if another crate needs it; otherwise `#[cfg(test)]`) and `provisioner.rs`. `async_trait` and `anyhow` are already in the crate.
- [ ] **Step 4: Run** `cargo test -p egret-provisioner` and clippy.
- [ ] **Step 5: Commit** `feat(provisioner): DockerEngineProvisioner over a DockerApi seam`

---

### Task 4: `BollardDocker` and the live test

**Files:**
- Create: `crates/egret-provisioner/src/docker/bollard_api.rs`, `crates/egret-provisioner/tests/docker_live.rs`
- Modify: `crates/egret-provisioner/Cargo.toml` (`bollard = "0.21"`, features as needed, plus `[[test]] name = "docker_live" harness = false`), root `Cargo.toml` workspace deps if the repo pins deps there (it does for `testcontainers`)

**Interfaces:**
- Consumes: Task 3 `DockerApi`. Produces: `BollardDocker::connect(host: &str) -> Result<Self, String>` (`unix://…` via the unix socket, `tcp://…` via plain HTTP) implementing `DockerApi`.

Requirements for the implementation. **Read the bollard 0.21 docs first (Context7, or `~/.cargo/registry/src/*/bollard-0.21.0`): 0.21 uses generated models and `query_parameters` builders, and older examples do not compile.**
- `ensure_image`: inspect, pull only on 404.
- `create_and_start`: create with `HostConfig { cap_drop: ["ALL"], security_opt: ["no-new-privileges"], nano_cpus, memory, network_mode: network, restart_policy: none }`, `EndpointsConfig` carrying the plan's aliases for that network, the plan's labels, env, a health check `CMD-SHELL /usr/lib/trino/bin/health-check` (interval 5 s, timeout 5 s, retries 30, start period 20 s), no `ports`, no `binds`; then `upload_to_container(path "/", files_tar(..))`; then start. On any error after create, force-remove the container.
- `list`: `all=true` with the label filters; map Docker `State.Status`/`Health.Status` to `ContainerState`.
- `stop_and_remove`: stop with a 30 s timeout, remove with `force`, treat 404 as success.
- `log_tail`: stdout+stderr, last N lines, as UTF-8 lossy.

- [ ] **Step 1: Write the live test** `docker_live.rs` (hand-rolled `main`, same report format as `container_docker_live.rs`; read that file's header for the exact contract). Gate: `EGRET_TEST_DOCKER=1`; otherwise print the tests as `ignored` and exit 0. With the gate on it needs a running Docker daemon and uses a throwaway network `egret-test-<random>` and project label `egret-test-<random>`, and a cleanup guard that removes only containers carrying that project label. Cases: (1) `ensure_image("trinodb/trino:483")`; (2) provision a one-coordinator spec with resources `cpu 1`, `mem 1536Mi`, wait up to 120 s for `running`; (3) from a second throwaway container on the same network, GET `http://<name>-trino:8080/v1/info` through the alias returns JSON with `"starting":false`; (4) inspect shows no published ports, no mounts, `CapDrop=[ALL]`, `no-new-privileges`; (5) `update` with a changed `query.max-memory` property → new container, file content changed (read it back through `docker exec cat` or the archive API); (6) a foreign container with the same engine name and no `egret.managed` label survives `delete`; (7) `delete` leaves no container of the project.
- [ ] **Step 2: Implement** `bollard_api.rs`.
- [ ] **Step 3: Run** `cargo build -p egret-provisioner`, `cargo test -p egret-provisioner --test docker_live` (reports ignored), then `EGRET_TEST_DOCKER=1 cargo test -p egret-provisioner --test docker_live -- --nocapture` (pulls Trino 483; runs in the background, it can exceed the 10-minute tool limit). Report both outputs verbatim in the task report.
- [ ] **Step 4: Commit** `feat(provisioner): bollard-backed DockerApi and a gated live test`

---

### Task 5: Wiring: selection, coordinator transport, accurate messages

**Files:**
- Modify: `crates/egret-engine-mgmt-api/src/lib.rs` (trait), `crates/egret-engine-mgmt/src/state.rs` (field), `crates/egret-engine-mgmt/src/monitor.rs` (`coordinator_call`), `crates/egret-engine-mgmt/src/engines.rs` (log/exec/follow messages; the 6 `EngineMgmtState { .. }` literals in tests and `egret-bin`), `crates/egret-engine-mgmt/tests/routes.rs`, `crates/egret-bin/src/main.rs`
- Modify: `crates/egret-provisioner/src/kube_client.rs` (extract the in-cluster HTTP branch of `coordinator_request` into `pub async fn http_coordinator_request(host_port_url, method, path, headers, body)` and call it from both)

**Interfaces:**
- Produces in `egret-engine-mgmt-api`:

```rust
#[async_trait::async_trait]
pub trait CoordinatorTransport: Send + Sync {
    /// One HTTP request to the coordinator of `service.namespace` on port 8080.
    async fn request(&self, namespace: &str, service: &str, method: &str, path: &str,
                     headers: &[(String, String)], body: Option<Vec<u8>>)
                     -> Result<(u16, Vec<u8>), String>;
}
```
- `EngineMgmtState.coordinator_transport: Option<Arc<dyn CoordinatorTransport>>`.
- `egret_provisioner::docker::HttpCoordinatorTransport` implementing it with `http://{service}.{namespace}.svc:8080{path}` (the same URL the in-cluster Kubernetes branch uses, which is why the aliases make it work).

Behaviour:
- `coordinator_call` order: ingress (unchanged), then `st.kube` if present (unchanged), else `st.coordinator_transport`, else the existing "kubernetes not configured" error.
- Pod logs, follow and exec routes: when `st.kube` is `None` **and** `st.coordinator_transport` is `Some`, answer 400 `not available with the Docker engine provisioner`; otherwise the message they give today.
- `main.rs`: after the existing kube block, `match config.engines.provisioner`: `Auto` → today's code, unchanged; `Kubernetes` → today's code but a failure to build a client is a startup error; `Docker` → `BollardDocker::connect(&docker.host)`, ping it, build `DockerEngineProvisioner::new(..)`, set `engine_provisioner`, leave `kube = None`, set `coordinator_transport`; a failed ping is a startup error naming the host; `None` → no provisioner. `engine_egret_url` = `engines.docker.callback_url` if the provisioner is `docker` and it is set, else the existing expression.
- StarRocks under `docker` is answered by the provisioner (Task 3). Check that the create-engine routes surface the `provision` error text as the 400 body (they do today for `engine provisioning failed: …`); add no new code unless the test below fails.

- [ ] **Step 1: Write failing tests**
  - `monitor.rs`: `coordinator_call` with `kube = None` and a fake transport returns the transport's `(status, body)` and passes URL parts, method, path and the monitor headers unchanged; with both `kube` and transport set, kube wins; with neither, the old error.
  - `egret-provisioner`: `HttpCoordinatorTransport` against a one-shot `tokio::net::TcpListener` that answers a fixed HTTP/1.1 response (the crate's dev-dependencies are only `tokio` with `macros` and `rt`; add the `net` and `io-util` features, no new crate) — for a test the service host cannot be `*.svc`, so give the transport a constructor `with_base(|ns, svc| String)` used only by the test.
  - `egret-bin` unit test of the selection function, extracted as `fn select_provisioner(cfg: &EnginesConfig, kube_available: bool) -> Selection` (`Selection::{Kubernetes, Docker, Disabled}`): `Auto`+kube → Kubernetes, `Auto`+no kube → Disabled, `Docker` → Docker regardless of kube, `None` → Disabled, `Kubernetes`+no kube → error.
  - `routes.rs`: a DB-gated test is NOT needed; instead a plain test that the pod-log route's error text differs when only a transport is set (use the existing non-DB test-state helper if there is one; otherwise cover the message through a small pure function `kube_only_error(st) -> CpError` and test that).
- [ ] **Step 2: Run** the new tests → FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test -p egret-provisioner -p egret-engine-mgmt-api -p egret-engine-mgmt -p egret-bin` and `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --check`. DB-gated tests need `DATABASE_URL`; follow the repo's usual test-DB recipe and state in the report which tests were skipped for lack of it.
- [ ] **Step 5: Commit** `feat(bin): select the engine provisioner from [engines]; Docker coordinator transport`

---

### Task 6: Demo compose: socket proxy, egret settings, recipes

**Files:**
- Modify: `deploy/docker-compose.demo.yml`, `deploy/demo/demo.env`, `deploy/demo/wait.sh`, `justfile`, `docs/en/guides/demo-stack.md` (safety section only here; the rest in Task 7)

**Interfaces:**
- Consumes: Task 5 config keys. Produces: a stack where `EGRET_ENGINES__PROVISIONER=docker` and engine containers join `egret-demo_default` with project label `egret-demo`.

Requirements:
- New service `docker-proxy`: `tecnativa/docker-socket-proxy`, pinned by tag **and digest** (look the digest up; do not guess), `/var/run/docker.sock:/var/run/docker.sock:ro`, **not published to the host**, env allow-list `CONTAINERS=1`, `IMAGES=1`, `NETWORKS=1`, `POST=1`, `INFO=1`, `PING=1`, everything else `0` (EXEC, VOLUMES, SERVICES, SWARM, SECRETS, CONFIGS, NODES, BUILD, COMMIT, PLUGINS, SYSTEM, AUTH, SESSION, DISTRIBUTION, TASKS all `0`). **Verify that the archive upload (`PUT /containers/{id}/archive`) and `DELETE` are allowed by this allow-list; if the proxy refuses them, record exactly which setting or proxy change is needed, and choose the narrowest one.** Document the result in the task report.
- `egret` service: `EGRET_ENGINES__PROVISIONER=docker`, `EGRET_ENGINES__DOCKER__HOST=tcp://docker-proxy:2375`, `EGRET_ENGINES__DOCKER__NETWORK=egret-demo_default` (use the explicit compose `networks.default.name` so the name does not depend on the project name), `EGRET_ENGINES__DOCKER__PROJECT=egret-demo`; `depends_on: docker-proxy`. egret does **not** mount the socket.
- `demo-down` and `demo-reset` remove containers matching **both** labels `egret.managed=true` and `egret.project=egret-demo` (today: managed only; see the recipes in `justfile` and "Notes from WP-A for WP-C" in the spec). Engine containers are removed before the compose network is, otherwise `down` fails on the network.
- `wait.sh` prints that *Add engine* is on, and one line naming the socket-proxy risk.
- Image pull of `trinodb/trino:483` happens at first *Add engine*; the guide says so (no pre-pull in `demo-up`, keep start-up fast).
- No new published port.

- [ ] **Step 1: Write the acceptance check first** as a shell test script `deploy/demo/test-engine-cleanup.sh` that (a) starts a throwaway container labelled `egret.managed=true,egret.project=egret-demo` on the demo network and another labelled `egret.managed=true,egret.project=someone-else`, (b) runs the `demo-down` recipe's container-removal command, (c) asserts the first is gone and the second survives, then removes the second. It only touches containers it created (names prefixed `egret-demo-cleanup-test-`).
- [ ] **Step 2: Run it** → FAIL (recipe filters on `managed` only).
- [ ] **Step 3: Implement** the compose, env, recipe and `wait.sh` changes.
- [ ] **Step 4: Run** the script → PASS; `docker compose --env-file deploy/demo/demo.env -f deploy/docker-compose.demo.yml config -q`; then `just demo-up` (with the `DEMO_PORT` overrides), confirm `/readyz`, and confirm from inside the egret container that the Docker API answers through the proxy: `docker compose … exec -T egret sh -c 'wget -qO- http://docker-proxy:2375/_ping'` prints `OK`, and a forbidden call (`/volumes`) is refused. Then `just demo-down`.
- [ ] **Step 5: Commit** `feat(deploy): Docker socket proxy and engine provisioner settings in the demo stack`

---

### Task 7: Browser verification, guide, acceptance, PR

**Files:**
- Create: `deploy/demo/verify-engine.mjs`
- Modify: `justfile` (`demo-verify-engine`), `docs/en/guides/demo-stack.md`

**Interfaces:**
- Consumes: the running stack from Task 6. Follow `deploy/demo/verify-apps.mjs` for Playwright resolution (`createRequire`, `PLAYWRIGHT_MODULE_DIR`, exit 2 with the install hint), session reuse with the stale-session re-login, bounded retries, and `os.tmpdir()` defaults. Playwright as a Node library only; login is limited to 5 per 5 minutes per ip:email.

The script drives, as the seeded admin: Control Plane → Engines → *Add engine* (Trino, name `demo-e2e`, single node, delegation default, the seeded warehouse) → wait up to 180 s for status *running* → SQL Lab (or the database page) against a database that uses the new engine and run `SELECT 1` and a query on the warehouse catalog → Control Plane → the engine's monitor lists that query → delete the engine from the UI → `docker ps --filter label=egret.engine=demo-e2e` is empty. Exit non-zero with the failing step named. Screenshots go to `SHOT_DIR`. Use the real labels in the UI; read them from the components rather than guessing (`frontend/src/routes/**` for the engines screens).

- [ ] **Step 1: Write `verify-engine.mjs`** and the `just demo-verify-engine` recipe (same environment handling as `demo-verify-apps`).
- [ ] **Step 2: Run it against a clean stack** (`just demo-reset`, `just demo-up`, `just demo-verify-engine`). Fix product-side problems only if they are inside this PR's scope (provisioner, transport, compose). For every other defect found, **record it, do not fix it** (file/line, what happened) for the Known issues list. Expected defects to look for: JWT delegation with the demo's `catalog.public_url`, the Ranger-off allow-all engine, TLS SAN versus alias, the `/data/trino` ownership, the health-check path in the image.
- [ ] **Step 3: Run it a second time** without reset (idempotence) and once after `just demo-down && just demo-up` with an engine left running (the engine must be reported correctly after egret restarts; `demo-down` removes it, so also test `docker compose stop egret && start egret` with the engine up, and record what status the UI shows).
- [ ] **Step 4: Update the guide**: Add engine row (Trino only, first use pulls the image, ~1.5 GB RAM per engine), a Safety paragraph on the Docker API behind the socket proxy (root-equivalent on the host if the proxy is bypassed; what the proxy forbids; never expose), `engines.*` keys, the Kubernetes-only features list from "Decisions" item 5, troubleshooting (engine stuck provisioning: `docker logs egret-engine-…`; Docker Desktop memory), remove the "no Add engine" lines from Known behaviour.
- [ ] **Step 5: Whole-branch checks**: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test -p egret-config -p egret-provisioner -p egret-engine-mgmt-api -p egret-engine-mgmt -p egret-bin`, gitleaks clean on the diff, `git diff origin/main --stat` shows no machine paths (`grep -rn "/Users/" $(git diff origin/main --name-only)` empty).
- [ ] **Step 6: Commit** `feat(deploy): browser check of Add engine in the demo stack; guide`

(Pushing, the PR and the merge are controller steps after the final review, not part of this task.)
