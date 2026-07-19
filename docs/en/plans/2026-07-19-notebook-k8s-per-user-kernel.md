# K8s per-user kernel launcher (hardened) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox syntax.

**Goal:** Run one isolated kernel Pod per (user×workspace) in-cluster (JupyterHub/sparklabx-style) via a `KubernetesLauncher` implementing the existing `KernelLauncher` trait; hardened with a dedicated namespace, least-privilege RBAC, non-root pods, and a default-deny NetworkPolicy. Wire it through helm + kind so a notebook cell actually runs in-cluster.

**Architecture:** egret's `KernelManager` already owns registry/reaper/token/WS-proxy, so we only add a launcher. The launcher (in `egret-bin`, which already has `kube` 0.99 / k8s-openapi 0.24) creates a Pod running `egret-kernel:dev` (bundles gateway + Daft + the P2/P3 magic), polls PodReady, and returns `http://<podIP>:8888`. Config selects subprocess (local) vs kubernetes (in-cluster).

**Tech Stack:** Rust (kube 0.99, k8s-openapi 0.24 typed `Pod`, async-trait, tokio), Helm, kind. Reference: `sparklabx/backend/internal/services/k8s_per_user_gateway.go`.

**Spec:** `docs/en/specs/2026-07-19-notebook-k8s-per-user-kernel-design.md`.

**Branch:** `feat/notebook-k8s-kernel` off `main`.

**Conventions:** backend cmds from `egret/`; `DATABASE_URL=postgres://egret:egret@localhost:5432/egret` for tests. Commit per task; don't commit unless the human asks (branch already isolated).

---

## File Structure

| File | New/Mod | Responsibility |
|---|---|---|
| `crates/egret-bin/src/kernel_launcher_k8s.rs` | New | `KubernetesLauncher` + `build_pod_spec`/`pod_name`/`phase_of` + unit tests |
| `crates/egret-bin/src/main.rs` | Mod | launcher select switch + callback-url plumbing |
| `crates/egret-bin/src/config.rs` (or wherever config lives) | Mod | `kernel.*` config fields |
| `deploy/helm/egret/templates/namespace-kernels.yaml` | New | `egret-notebooks` ns + restricted PodSecurity |
| `deploy/helm/egret/templates/rbac-kernels.yaml` | New | Role + cross-ns RoleBinding for pods in `egret-notebooks` |
| `deploy/helm/egret/templates/networkpolicy-kernels.yaml` | New | default-deny + allowlist egress/ingress |
| `deploy/helm/egret/templates/backend-deployment.yaml` | Mod | `EGRET_KERNEL__*` env (gated on `.Values.kernel.launcher`) |
| `deploy/helm/egret/values.yaml` | Mod | `kernel:` block |
| `justfile` | Mod | build + `kind load` `egret-kernel:dev` |

---

## Task A1: KubernetesLauncher (pure spec builder + trait impl + unit tests)

**Files:** Create `crates/egret-bin/src/kernel_launcher_k8s.rs`; register the module in `main.rs` (`mod kernel_launcher_k8s;`). Test: inline `#[cfg(test)]`.

FIRST read `crates/egret-orchestrator/src/kube_client.rs:375-490` for the repo's typed-`Pod` `Api::namespaced` usage pattern (kube 0.99 / k8s-openapi 0.24), and `crates/egret-kernel/src/launcher.rs` for the exact trait + types.

- [ ] **Step 1: Write the failing test (pure fns first)**

Add at the bottom of the new file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use egret_kernel::launcher::KernelEnv;
    use egret_kernel::registry::KernelKey;
    use uuid::Uuid;

    fn cfg() -> K8sKernelConfig {
        K8sKernelConfig {
            namespace: "egret-notebooks".into(),
            image: "egret-kernel:dev".into(),
            image_pull_policy: "IfNotPresent".into(),
            cpu_request: "250m".into(),
            mem_request: "512Mi".into(),
            cpu_limit: "1".into(),
            mem_limit: "2Gi".into(),
            pull_secret: None,
        }
    }

    #[test]
    fn pod_name_is_deterministic_and_dns_safe() {
        let k = KernelKey { user_id: Uuid::nil(), workspace_id: Uuid::nil() };
        let a = pod_name(&k);
        let b = pod_name(&k);
        assert_eq!(a, b);
        assert!(a.starts_with("kernel-"));
        assert!(a.len() <= 63 && a.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'));
    }

    #[test]
    fn pod_spec_is_hardened_and_carries_env() {
        let k = KernelKey { user_id: Uuid::new_v4(), workspace_id: Uuid::new_v4() };
        let env: KernelEnv = KernelEnv::from([("EGRET_API_URL".into(), "http://egret:8080".into())]);
        let pod = build_pod_spec(&k, &env, &cfg());
        let spec = pod.spec.unwrap();
        // non-root pod security
        let psc = spec.security_context.unwrap();
        assert_eq!(psc.run_as_non_root, Some(true));
        assert_eq!(psc.run_as_user, Some(1000));
        // single container, hardened
        let c = &spec.containers[0];
        assert_eq!(c.image.as_deref(), Some("egret-kernel:dev"));
        let csc = c.security_context.clone().unwrap();
        assert_eq!(csc.allow_privilege_escalation, Some(false));
        assert_eq!(csc.capabilities.unwrap().drop.unwrap(), vec!["ALL".to_string()]);
        // env passthrough + injected port
        let names: Vec<_> = c.env.clone().unwrap().into_iter().map(|e| e.name).collect();
        assert!(names.contains(&"EGRET_API_URL".to_string()));
        assert!(names.contains(&"KERNEL_GATEWAY_PORT".to_string()));
        // port + readiness probe
        assert_eq!(c.ports.clone().unwrap()[0].container_port, 8888);
        assert!(c.readiness_probe.is_some());
        // restart policy + grace
        assert_eq!(spec.restart_policy.as_deref(), Some("OnFailure"));
        assert_eq!(spec.termination_grace_period_seconds, Some(5));
        // labels
        let labels = pod.metadata.labels.unwrap();
        assert_eq!(labels.get("managed-by").map(String::as_str), Some("egret"));
    }

    #[test]
    fn phase_of_maps_pod_status() {
        assert_eq!(phase_of_ready(true), KernelPhase::Ready);
        assert_eq!(phase_of_waiting_reason("ImagePullBackOff"), Some(KernelPhase::Dead));
        assert_eq!(phase_of_waiting_reason("CrashLoopBackOff"), Some(KernelPhase::Dead));
        assert_eq!(phase_of_waiting_reason("ContainerCreating"), None);
    }
}
```

- [ ] **Step 2: Run to confirm FAIL**

Run (from `egret/`): `cargo test -p egret-bin kernel_launcher_k8s`
Expected: does not compile (module/types missing).

- [ ] **Step 3: Implement the launcher**

Create `crates/egret-bin/src/kernel_launcher_k8s.rs`:

```rust
//! `KubernetesLauncher` — one kernel Pod per (user×workspace), addressed by Pod IP.
//! Implements `egret_kernel::KernelLauncher`. Reference: sparklabx k8s_per_user_gateway.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use k8s_openapi::api::core::v1::{
    Capabilities, Container, ContainerPort, EnvVar, LocalObjectReference, Pod, PodSecurityContext,
    PodSpec, Probe, ResourceRequirements, SeccompProfile, SecurityContext, TCPSocketAction,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, DeleteParams, PostParams};
use sha1::{Digest, Sha1};

use egret_kernel::error::KernelError;
use egret_kernel::launcher::{KernelEnv, KernelHandle, KernelLauncher, KernelPhase};
use egret_kernel::registry::KernelKey;

const GATEWAY_PORT: i32 = 8888;

#[derive(Clone, Debug)]
pub struct K8sKernelConfig {
    pub namespace: String,
    pub image: String,
    pub image_pull_policy: String,
    pub cpu_request: String,
    pub mem_request: String,
    pub cpu_limit: String,
    pub mem_limit: String,
    pub pull_secret: Option<String>,
}

pub struct KubernetesLauncher {
    client: kube::Client,
    cfg: K8sKernelConfig,
}

impl KubernetesLauncher {
    pub fn new(client: kube::Client, cfg: K8sKernelConfig) -> Self {
        Self { client, cfg }
    }
    fn pods(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.cfg.namespace)
    }
}

/// Deterministic, DNS-safe pod name: `kernel-<12 hex>` from sha1(user||ws).
pub fn pod_name(key: &KernelKey) -> String {
    let mut h = Sha1::new();
    h.update(key.user_id.as_bytes());
    h.update(key.workspace_id.as_bytes());
    let d = h.finalize();
    format!("kernel-{}", hex::encode(&d[..6]))
}

fn label_short(id: &uuid::Uuid) -> String {
    id.simple().to_string()[..8].to_string()
}

/// Map a container Ready flag to a phase (true → Ready).
pub fn phase_of_ready(ready: bool) -> KernelPhase {
    if ready { KernelPhase::Ready } else { KernelPhase::Spawning }
}

/// Terminal container waiting reasons → Dead; transient (ContainerCreating) → None.
pub fn phase_of_waiting_reason(reason: &str) -> Option<KernelPhase> {
    match reason {
        "ImagePullBackOff" | "ErrImagePull" | "CrashLoopBackOff" | "CreateContainerConfigError"
        | "CreateContainerError" => Some(KernelPhase::Dead),
        _ => None,
    }
}

/// Build the hardened Pod object for a kernel. Pure fn (unit-tested, no cluster).
pub fn build_pod_spec(key: &KernelKey, env: &KernelEnv, cfg: &K8sKernelConfig) -> Pod {
    let mut labels = BTreeMap::new();
    labels.insert("app".to_string(), "egret-kernel".to_string());
    labels.insert("managed-by".to_string(), "egret".to_string());
    labels.insert("egret-user".to_string(), label_short(&key.user_id));
    labels.insert("egret-workspace".to_string(), label_short(&key.workspace_id));

    // env passthrough + the port the entrypoint binds.
    let mut env_vars: Vec<EnvVar> = env
        .iter()
        .map(|(k, v)| EnvVar { name: k.clone(), value: Some(v.clone()), value_from: None })
        .collect();
    env_vars.push(EnvVar {
        name: "KERNEL_GATEWAY_PORT".to_string(),
        value: Some(GATEWAY_PORT.to_string()),
        value_from: None,
    });
    env_vars.sort_by(|a, b| a.name.cmp(&b.name)); // deterministic for tests

    let resources = ResourceRequirements {
        requests: Some(BTreeMap::from([
            ("cpu".to_string(), Quantity(cfg.cpu_request.clone())),
            ("memory".to_string(), Quantity(cfg.mem_request.clone())),
        ])),
        limits: Some(BTreeMap::from([
            ("cpu".to_string(), Quantity(cfg.cpu_limit.clone())),
            ("memory".to_string(), Quantity(cfg.mem_limit.clone())),
        ])),
        claims: None,
    };

    let container = Container {
        name: "egret-kernel".to_string(),
        image: Some(cfg.image.clone()),
        image_pull_policy: Some(cfg.image_pull_policy.clone()),
        ports: Some(vec![ContainerPort {
            container_port: GATEWAY_PORT,
            protocol: Some("TCP".to_string()),
            ..Default::default()
        }]),
        env: Some(env_vars),
        resources: Some(resources),
        readiness_probe: Some(Probe {
            tcp_socket: Some(TCPSocketAction {
                port: IntOrString::Int(GATEWAY_PORT),
                host: None,
            }),
            initial_delay_seconds: Some(5),
            period_seconds: Some(3),
            ..Default::default()
        }),
        security_context: Some(SecurityContext {
            allow_privilege_escalation: Some(false),
            capabilities: Some(Capabilities { add: None, drop: Some(vec!["ALL".to_string()]) }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let spec = PodSpec {
        restart_policy: Some("OnFailure".to_string()),
        termination_grace_period_seconds: Some(5),
        image_pull_secrets: cfg
            .pull_secret
            .as_ref()
            .map(|n| vec![LocalObjectReference { name: n.clone() }]),
        security_context: Some(PodSecurityContext {
            run_as_non_root: Some(true),
            run_as_user: Some(1000),
            fs_group: Some(1000),
            seccomp_profile: Some(SeccompProfile { type_: "RuntimeDefault".to_string(), localhost_profile: None }),
            ..Default::default()
        }),
        containers: vec![container],
        ..Default::default()
    };

    Pod {
        metadata: ObjectMeta {
            name: Some(pod_name(key)),
            namespace: Some(cfg.namespace.clone()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(spec),
        status: None,
    }
}

/// Read Ready condition / terminal states from a live Pod.
fn pod_phase(p: &Pod) -> KernelPhase {
    if p.metadata.deletion_timestamp.is_some() {
        return KernelPhase::Dead;
    }
    let status = match &p.status {
        Some(s) => s,
        None => return KernelPhase::Spawning,
    };
    if let Some(css) = &status.container_statuses {
        for cs in css {
            if let Some(w) = cs.state.as_ref().and_then(|s| s.waiting.as_ref()) {
                if let Some(reason) = &w.reason {
                    if let Some(ph) = phase_of_waiting_reason(reason) {
                        return ph;
                    }
                }
            }
            if cs.ready {
                return KernelPhase::Ready;
            }
        }
    }
    if status.phase.as_deref() == Some("Failed") {
        return KernelPhase::Dead;
    }
    KernelPhase::Spawning
}

fn ready_url(p: &Pod) -> Option<String> {
    let ip = p.status.as_ref()?.pod_ip.as_ref()?;
    if pod_phase(p) == KernelPhase::Ready {
        Some(format!("http://{ip}:{GATEWAY_PORT}"))
    } else {
        None
    }
}

#[async_trait]
impl KernelLauncher for KubernetesLauncher {
    async fn ensure(
        &self,
        key: KernelKey,
        env: &KernelEnv,
        timeout: Duration,
    ) -> Result<KernelHandle, KernelError> {
        let pods = self.pods();
        let name = pod_name(&key);

        // Fast path: existing ready pod.
        if let Ok(p) = pods.get(&name).await {
            if let Some(url) = ready_url(&p) {
                return Ok(KernelHandle { url, phase: KernelPhase::Ready, pid: None });
            }
        }

        // Create (idempotent).
        let pod = build_pod_spec(&key, env, &self.cfg);
        match pods.create(&PostParams::default(), &pod).await {
            Ok(_) => {}
            Err(kube::Error::Api(ae)) if ae.code == 409 => { /* AlreadyExists → reuse */ }
            Err(e) => return Err(KernelError::Spawn(format!("pod create: {e}"))),
        }

        // Poll until Ready or terminal or timeout.
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if tokio::time::Instant::now() >= deadline {
                let _ = pods.delete(&name, &DeleteParams::default()).await;
                return Err(KernelError::Spawn(format!(
                    "kernel pod {name} not ready within {timeout:?}"
                )));
            }
            match pods.get(&name).await {
                Ok(p) => {
                    if let Some(url) = ready_url(&p) {
                        return Ok(KernelHandle { url, phase: KernelPhase::Ready, pid: None });
                    }
                    if pod_phase(&p) == KernelPhase::Dead {
                        let _ = pods.delete(&name, &DeleteParams::default()).await;
                        return Err(KernelError::Spawn(format!("kernel pod {name} failed to start")));
                    }
                }
                Err(kube::Error::Api(ae)) if ae.code == 404 => { /* not yet visible */ }
                Err(e) => return Err(KernelError::Spawn(format!("pod get: {e}"))),
            }
        }
    }

    async fn status(&self, key: KernelKey) -> Result<KernelPhase, KernelError> {
        match self.pods().get(&pod_name(&key)).await {
            Ok(p) => Ok(pod_phase(&p)),
            Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(KernelPhase::Dead),
            Err(e) => Err(KernelError::Spawn(format!("pod status: {e}"))),
        }
    }

    async fn stop(&self, key: KernelKey) -> Result<(), KernelError> {
        match self.pods().delete(&pod_name(&key), &DeleteParams::default()).await {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
            Err(e) => Err(KernelError::Spawn(format!("pod delete: {e}"))),
        }
    }

    fn mode(&self) -> &'static str {
        "kubernetes"
    }
}
```

Add deps to `crates/egret-bin/Cargo.toml` if missing: `sha1`, `hex` (check first — many crates already pull them; use `workspace = true` if defined, else add). `async-trait`, `k8s-openapi`, `kube`, `tokio`, `uuid` are already deps.

- [ ] **Step 4: Register module + run tests**

In `main.rs` add `mod kernel_launcher_k8s;` near the other `mod` decls.
Run: `cargo test -p egret-bin kernel_launcher_k8s` → 3 tests pass.
Run: `cargo clippy -p egret-bin -- -D warnings` and `cargo fmt -p egret-bin`.

- [ ] **Step 5: Commit**
```bash
git add crates/egret-bin/src/kernel_launcher_k8s.rs crates/egret-bin/src/main.rs crates/egret-bin/Cargo.toml
git commit -m "feat(kernel): KubernetesLauncher (pod-per-user, hardened pod spec)"
```

---

## Task A2: Config + launcher selection + callback URL

**Files:** `crates/egret-bin/src/main.rs` (+ the config struct it reads — find where `config.api_url`/`EGRET_*` are defined; egret config is `figment`/`serde`-based, env-prefixed `EGRET_`). Read `main.rs:1240-1300` first.

- [ ] **Step 1: Add config fields**

Wherever the server config struct lives (the one exposing `api_url` at `main.rs:1278`), add a `kernel` sub-config (serde, env `EGRET_KERNEL__*`, all with defaults):
```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct KernelConfig {
    #[serde(default = "default_launcher")] pub launcher: String,     // "subprocess" | "kubernetes"
    #[serde(default = "default_ns")]       pub namespace: String,    // "egret-notebooks"
    #[serde(default = "default_image")]    pub image: String,        // "egret-kernel:dev"
    #[serde(default = "default_pull")]     pub image_pull_policy: String, // "IfNotPresent"
    #[serde(default = "default_cpu_req")]  pub cpu_request: String,  // "250m"
    #[serde(default = "default_mem_req")]  pub mem_request: String,  // "512Mi"
    #[serde(default = "default_cpu_lim")]  pub cpu_limit: String,    // "1"
    #[serde(default = "default_mem_lim")]  pub mem_limit: String,    // "2Gi"
    #[serde(default)]                      pub pull_secret: Option<String>,
    #[serde(default)]                      pub callback_url: Option<String>, // in-cluster backend URL for kernels
}
```
Provide the `default_*` fns returning the strings above; `default_launcher` → `"subprocess"`. Add `#[serde(default)] pub kernel: KernelConfig` to the top-level config (with a `Default` impl for `KernelConfig` so an absent block still parses). Match the repo's existing config style exactly (look at how other nested config blocks like `storage`/`security` are declared).

- [ ] **Step 2: Switch the launcher (main.rs:1246-1247)**

Replace:
```rust
    let kernel_launcher: std::sync::Arc<dyn egret_kernel::KernelLauncher> =
        std::sync::Arc::new(egret_kernel::SubprocessLauncher::default());
```
with:
```rust
    let kernel_launcher: std::sync::Arc<dyn egret_kernel::KernelLauncher> =
        if config.kernel.launcher == "kubernetes" {
            let client = kube::Client::try_default()
                .await
                .expect("kernel.launcher=kubernetes but no in-cluster kube config");
            let kcfg = crate::kernel_launcher_k8s::K8sKernelConfig {
                namespace: config.kernel.namespace.clone(),
                image: config.kernel.image.clone(),
                image_pull_policy: config.kernel.image_pull_policy.clone(),
                cpu_request: config.kernel.cpu_request.clone(),
                mem_request: config.kernel.mem_request.clone(),
                cpu_limit: config.kernel.cpu_limit.clone(),
                mem_limit: config.kernel.mem_limit.clone(),
                pull_secret: config.kernel.pull_secret.clone(),
            };
            std::sync::Arc::new(crate::kernel_launcher_k8s::KubernetesLauncher::new(client, kcfg))
        } else {
            std::sync::Arc::new(egret_kernel::SubprocessLauncher::default())
        };
```

- [ ] **Step 3: Callback URL for kernel env**

At `main.rs:1278` where `api_url` is computed, override it for the kernel env when `config.kernel.callback_url` is set:
```rust
    let kernel_api_url = config.kernel.callback_url.clone().unwrap_or_else(|| api_url.clone());
```
and pass `kernel_api_url` (instead of `api_url`) into the value used to build `manifest_to_env`/`KernelState` for kernels. Trace where `api_url` flows into the kernel env (grep `api_url` near the `KernelState`/`manifest_to_env` construction) and substitute `kernel_api_url` on that path ONLY — leave any non-kernel use of `api_url` (e.g. OIDC redirect) unchanged.

- [ ] **Step 4: Build + test**

Run: `cargo build -p egret-bin` (must compile). `cargo test -p egret-bin` (existing tests green). `cargo clippy -p egret-bin -- -D warnings`, `cargo fmt --all --check`.

- [ ] **Step 5: Commit**
```bash
git add crates/egret-bin/src/main.rs crates/egret-bin/src/config.rs
git commit -m "feat(kernel): select launcher via config + cluster-internal callback URL"
```

---

## Task B1: Helm — namespace + RBAC

**Files:** `deploy/helm/egret/templates/namespace-kernels.yaml`, `rbac-kernels.yaml`. Read `namespace-engines.yaml` + `rbac.yaml` + `serviceaccount.yaml` first to match style/helpers.

- [ ] **Step 1: Namespace** `templates/namespace-kernels.yaml`:
```yaml
{{- if eq .Values.kernel.launcher "kubernetes" }}
apiVersion: v1
kind: Namespace
metadata:
  name: {{ .Values.kernel.namespace }}
  labels:
    pod-security.kubernetes.io/enforce: restricted
    pod-security.kubernetes.io/enforce-version: latest
{{- end }}
```

- [ ] **Step 2: RBAC** `templates/rbac-kernels.yaml` (Role in the kernel ns; RoleBinding subject = backend SA in the release ns):
```yaml
{{- if eq .Values.kernel.launcher "kubernetes" }}
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: egret-kernel-manager
  namespace: {{ .Values.kernel.namespace }}
rules:
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list", "watch", "create", "delete"]
  - apiGroups: [""]
    resources: ["pods/log"]
    verbs: ["get"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: egret-kernel-manager
  namespace: {{ .Values.kernel.namespace }}
subjects:
  - kind: ServiceAccount
    name: egret
    namespace: {{ .Release.Namespace }}
roleRef:
  kind: Role
  name: egret-kernel-manager
  apiGroup: rbac.authorization.k8s.io
{{- end }}
```

- [ ] **Step 3: Lint** `helm template deploy/helm/egret --set kernel.launcher=kubernetes | kubectl apply --dry-run=client -f -` (or `helm lint`) → renders cleanly.

- [ ] **Step 4: Commit**
```bash
git add deploy/helm/egret/templates/namespace-kernels.yaml deploy/helm/egret/templates/rbac-kernels.yaml
git commit -m "feat(deploy): kernel namespace + least-privilege RBAC"
```

---

## Task B2: Helm — NetworkPolicy + backend env + values

**Files:** `templates/networkpolicy-kernels.yaml`, `templates/backend-deployment.yaml`, `values.yaml`. Read `backend-deployment.yaml` + `configmap-env.yaml` first.

- [ ] **Step 1: NetworkPolicy** `templates/networkpolicy-kernels.yaml`:
```yaml
{{- if and (eq .Values.kernel.launcher "kubernetes") .Values.kernel.networkPolicy.enabled }}
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: egret-kernel-isolation
  namespace: {{ .Values.kernel.namespace }}
spec:
  podSelector:
    matchLabels: { managed-by: egret }
  policyTypes: [Ingress, Egress]
  ingress:
    - from:
        - namespaceSelector:
            matchLabels: { kubernetes.io/metadata.name: {{ .Release.Namespace }} }
      ports:
        - { protocol: TCP, port: 8888 }
  egress:
    # DNS
    - to: []
      ports:
        - { protocol: UDP, port: 53 }
        - { protocol: TCP, port: 53 }
    # backend (engine-token callback)
    - to:
        - namespaceSelector:
            matchLabels: { kubernetes.io/metadata.name: {{ .Release.Namespace }} }
    # engines (Trino/StarRocks) + object store
    - to:
        - namespaceSelector:
            matchLabels: { kubernetes.io/metadata.name: egret-engines }
{{- end }}
```
(Note in a comment: kind's default kindnet does not enforce NetworkPolicy — the policy is declared and correct; enforcement needs Calico/Cilium.)

- [ ] **Step 2: Backend env** — in `backend-deployment.yaml`, add under the container `env:` list (gated):
```yaml
            {{- if eq .Values.kernel.launcher "kubernetes" }}
            - { name: EGRET_KERNEL__LAUNCHER, value: "kubernetes" }
            - { name: EGRET_KERNEL__NAMESPACE, value: "{{ .Values.kernel.namespace }}" }
            - { name: EGRET_KERNEL__IMAGE, value: "{{ .Values.kernel.image }}" }
            - { name: EGRET_KERNEL__IMAGE_PULL_POLICY, value: "{{ .Values.kernel.imagePullPolicy }}" }
            - { name: EGRET_KERNEL__CPU_REQUEST, value: "{{ .Values.kernel.resources.cpuRequest }}" }
            - { name: EGRET_KERNEL__MEM_REQUEST, value: "{{ .Values.kernel.resources.memRequest }}" }
            - { name: EGRET_KERNEL__CPU_LIMIT, value: "{{ .Values.kernel.resources.cpuLimit }}" }
            - { name: EGRET_KERNEL__MEM_LIMIT, value: "{{ .Values.kernel.resources.memLimit }}" }
            - { name: EGRET_KERNEL__CALLBACK_URL, value: "http://egret.{{ .Release.Namespace }}.svc.cluster.local:8080" }
            {{- if .Values.kernel.pullSecret }}
            - { name: EGRET_KERNEL__PULL_SECRET, value: "{{ .Values.kernel.pullSecret }}" }
            {{- end }}
            {{- end }}
```

- [ ] **Step 3: values.yaml** — add:
```yaml
kernel:
  launcher: subprocess        # "kubernetes" enables per-user kernel pods
  namespace: egret-notebooks
  image: egret-kernel:dev
  imagePullPolicy: IfNotPresent
  pullSecret: ""
  resources:
    cpuRequest: "250m"
    memRequest: "512Mi"
    cpuLimit: "1"
    memLimit: "2Gi"
  networkPolicy:
    enabled: true
```

- [ ] **Step 4: Lint** `helm template deploy/helm/egret --set kernel.launcher=kubernetes | kubectl apply --dry-run=client -f -` → clean.

- [ ] **Step 5: Commit**
```bash
git add deploy/helm/egret/templates/networkpolicy-kernels.yaml deploy/helm/egret/templates/backend-deployment.yaml deploy/helm/egret/values.yaml
git commit -m "feat(deploy): kernel NetworkPolicy + backend env + values"
```

---

## Task B3: justfile — build + load the kernel image

**Files:** `justfile`. Read the `images-build`/`deploy-kind*` recipes (lines 46-77).

- [ ] **Step 1: Extend `images-build`** to also build the kernel image:
```make
images-build:
    docker build -f deploy/docker/backend.Dockerfile -t egret-backend:dev .
    docker build -f frontend/Dockerfile -t egret-frontend:dev frontend
    docker build -t egret-kernel:dev kernel-image
```

- [ ] **Step 2: Extend each `kind load` line** in `deploy-kind`, `deploy-kind-oidc`, `deploy-kind-governance` to include `egret-kernel:dev`:
```make
    kind load docker-image egret-backend:dev egret-frontend:dev egret-kernel:dev --name egret
```
And add `--set kernel.launcher=kubernetes` to the `helm upgrade` in `deploy-kind-oidc` (and governance) so the in-cluster deploy uses k8s kernels. (Leave plain `deploy-kind` on subprocess, or add the flag there too — pick `deploy-kind-oidc` as the canonical notebook-execution path.)

- [ ] **Step 3: Commit**
```bash
git add justfile
git commit -m "chore(deploy): build + kind-load egret-kernel image; k8s launcher on oidc deploy"
```

---

## Task C: Live-prove on kind

**Files:** none (verification). Requires Docker + the kind `egret` cluster.

- [ ] **Step 1: Deploy** — `just deploy-kind-oidc` (builds all 3 images incl. kernel, loads them, helm upgrade with `kernel.launcher=kubernetes`). Then force a backend rollout if the tag is unchanged: `kubectl rollout restart deploy/egret -n egret-system && kubectl rollout status deploy/egret -n egret-system`.
- [ ] **Step 2: Verify namespace + RBAC** — `kubectl get ns egret-notebooks`, `kubectl auth can-i create pods --as=system:serviceaccount:egret-system:egret -n egret-notebooks` → `yes`.
- [ ] **Step 3: Run a cell** — open `http://egret.localtest.me`, login admin, open/create a notebook (in a workspace that has a provisioned warehouse), Run a Python cell `print(6*7)`. Watch `kubectl get pods -n egret-notebooks -w` → a `kernel-*` Pod appears, reaches `Running`/Ready, and the cell shows `42`.
- [ ] **Step 4: Run a SQL cell** — with a target engine enabled, `SELECT 1` → bound DataFrame + output (governed via the per-user JWT).
- [ ] **Step 5: Lifecycle** — delete the kernel from the UI (or idle-reap) → the Pod is deleted (`kubectl get pods -n egret-notebooks`). Restart the backend → a still-open notebook re-attaches to the live Pod (deterministic name) without a new spawn.
- [ ] **Step 6: Record findings** — note any fixes (image pull policy, callback URL, RBAC verbs, probe timing) and update the spec's gotchas.

---

## Self-Review Notes
- Spec §3.1 launcher → A1; §3.2 selection/callback → A2; §3.3 deploy → B1/B2/B3; §5 live → C. All covered.
- kube 0.99 / k8s-openapi 0.24 typed `Pod` API confirmed against `egret-orchestrator/src/kube_client.rs`.
- Pure fns (`build_pod_spec`/`pod_name`/`phase_of_*`) are unit-tested with no cluster; the live path is documented, not in CI.
- Default `kernel.launcher=subprocess` → existing local/subprocess installs are unaffected; k8s is opt-in via config/helm.
- ResourceUsageBadge / metrics-server / per-user resize are explicitly out of scope (follow-on).
- NetworkPolicy is declared but kindnet won't enforce it — documented; correctness verifiable under Calico.
