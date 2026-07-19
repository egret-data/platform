# Notebook — Kubernetes per-user kernel launcher (hardened) — Design

- **Date:** 2026-07-19
- **Status:** Approved (brainstorm) → ready for plan
- **Scope area:** `egret-bin` (launcher + wiring), `egret-kernel` (trait already exists), `deploy/helm/egret`, `justfile`, `kernel-image`
- **Reference:** sparklabx `backend/internal/services/k8s_per_user_gateway.go` (mature k8s-per-user gateway). Related: notebook epic D1a/D1b (`KernelLauncher` trait, `SubprocessLauncher`), F2 OIDC delegation, [[egret-deploy-kind]].

## 1. Problem & Goal

In-cluster, egret hardcodes `SubprocessLauncher` (`egret-bin/src/main.rs:1247`), which shells out to `jupyter kernelgateway` from the backend pod's PATH. The backend image has **no** Python/jupyter/daft, so notebook cell execution does not work in-cluster. **Goal:** run **one isolated kernel Pod per (user × workspace)**, JupyterHub/sparklabx-style, using the existing `egret-kernel:dev` image (which already bundles the gateway + Daft + the P2/P3 `%%egret_sql` magic). Production-grade / hardened: dedicated namespace, least-privilege RBAC, non-root pods, and a default-deny NetworkPolicy.

**Key leverage:** egret already has `KernelManager` (registry, reaper, startup sweep), per-kernel token minting, and the D1c WS proxy. We do **not** reimplement that lifecycle (unlike sparklabx, which builds it all in the gateway). We implement only a `KubernetesLauncher` conforming to the existing `KernelLauncher` trait; the manager drives it.

### Non-goals (deferred)
- ResourceUsageBadge + `/kernel/usage` (metrics-server) — follow-on FE feature.
- Per-user resize / QoS picker (sparklabx has it) — later.
- Per-kernel Service / stable DNS — Pod IP is sufficient (see §4.2, matches sparklabx).
- docker-per-user launcher.

## 2. Existing trait (unchanged)

`egret_kernel::KernelLauncher` (`crates/egret-kernel/src/launcher.rs`):
```rust
async fn ensure(&self, key: KernelKey, env: &KernelEnv, timeout: Duration) -> Result<KernelHandle, KernelError>;
async fn status(&self, key: KernelKey) -> Result<KernelPhase, KernelError>;
async fn stop(&self, key: KernelKey) -> Result<(), KernelError>;
fn mode(&self) -> &'static str;
```
`KernelKey { user_id, workspace_id }`. `KernelHandle { url, phase, pid }`. `KernelPhase { Spawning, Ready, Dead }`. `KernelEnv = HashMap<String,String>`. The manager already calls `ensure` (reuse-or-spawn), `status` (liveness), `stop` (reaper/delete).

## 3. Architecture

### 3.1 `KubernetesLauncher` — new, in `egret-bin`
Lives in `egret-bin` (a new module `src/kernel_launcher_k8s.rs`) because `egret-bin` already depends on `kube` (`Cargo.toml:40`) and the orchestrator patterns live there; keeps `egret-kernel` free of a kube dependency. Implements `egret_kernel::KernelLauncher`.

Fields: `kube::Client`, `K8sKernelConfig { namespace, image, image_pull_policy, cpu_request, mem_request, cpu_limit, mem_limit, pull_secret: Option<String> }`.

- **`ensure(key, env, timeout)`**:
  1. `pod = pod_name(key)` — `kernel-<hex12>` where hex12 = first 6 bytes of `sha1(user_id || workspace_id)` (DNS-safe, ≤63, deterministic so a backend restart re-attaches). (Ref: sparklabx `podNameForUser`.)
  2. GET the pod. If it exists and `PodReady=True` and not terminating → return `KernelHandle { url: http://<podIP>:8888, phase: Ready }`. If it exists but `deletionTimestamp` set → wait until gone (≤ a slice of `timeout`).
  3. Build the hardened Pod spec (`build_pod_spec`, a **pure fn** for unit testing) and `create` it (idempotent: treat `AlreadyExists` as reuse).
  4. Poll (every 500ms) GET-pod until `PodReady=True` → return `http://<podIP>:8888`; or a terminal container state (`ImagePullBackOff`/`CrashLoopBackOff`/`CreateContainerConfigError`/exited-nonzero) → delete + `KernelError::Spawn`. On `timeout` → delete + `KernelError::Spawn`. (Ref: sparklabx `derivePhase`/`readyURL`; we poll rather than Watch — simpler, the manager already retries.)
- **`status(key)`**: GET the pod; `PodReady=True`→`Ready`, terminal/absent→`Dead`, else `Spawning`.
- **`stop(key)`**: `delete` the pod (ignore `NotFound`).
- **`mode()`**: `"kubernetes"`.

**Pod spec (`build_pod_spec(key, env, cfg) -> Pod`)** — hardened:
- `metadata`: name, namespace, labels `{ app: egret-kernel, managed-by: egret, egret-user: <user8>, egret-workspace: <ws8> }` (label-safe; used by an orphan sweep + selectors).
- `spec.restartPolicy: OnFailure`; `terminationGracePeriodSeconds: 5` (kernels hold no durable state).
- `spec.securityContext`: `runAsNonRoot: true`, `runAsUser: 1000`, `fsGroup: 1000`, `seccompProfile: RuntimeDefault`.
- container `egret-kernel`: `image: cfg.image`, `imagePullPolicy: cfg.image_pull_policy` (IfNotPresent for kind-loaded), `ports: [8888/TCP]`, `env`: every entry of `KernelEnv` **plus** `KERNEL_GATEWAY_PORT=8888`, `resources` from cfg (req 250m/512Mi, lim 1/2Gi defaults), `readinessProbe: tcpSocket 8888 (initialDelay 5, period 3)`, `securityContext`: `allowPrivilegeEscalation: false`, `readOnlyRootFilesystem: false` (jupyter writes runtime files; mount an `emptyDir` at `/home`/runtime if we later flip this), `capabilities.drop: [ALL]`.
- `imagePullSecrets`: `[cfg.pull_secret]` when set, else omitted.

The **kernel image already bundles the magic** (`kernel-image/Dockerfile` COPYs `egret_kernel_helpers`), so a kernel Pod runs the current P2/P3 `%%egret_sql`.

### 3.2 Launcher selection & config — `egret-bin/src/main.rs`
Replace the hardcoded construction (`main.rs:1246-1247`) with a config switch:
- New config field `kernel.launcher` (env `EGRET_KERNEL__LAUNCHER`, default `"subprocess"`). `"kubernetes"` → build `KubernetesLauncher` (in-cluster `kube::Client::try_default()`), else `SubprocessLauncher`.
- New config for the k8s launcher (all env-driven, helm-set): `EGRET_KERNEL__NAMESPACE` (default `egret-notebooks`), `EGRET_KERNEL__IMAGE` (default `egret-kernel:dev`), `EGRET_KERNEL__CPU_REQUEST|MEM_REQUEST|CPU_LIMIT|MEM_LIMIT`, `EGRET_KERNEL__PULL_SECRET` (optional).
- **Kernel callback URL:** the env injected as `EGRET_API_URL` must be **cluster-internal** for k8s kernels (a Pod can't reach the backend at localhost/the external ingress host reliably). Add `EGRET_KERNEL__CALLBACK_URL` (default: fall back to the existing `api_url` at `main.rs:1278`); when set, use it as the `api_url` argument to `manifest_to_env`. Helm sets it to `http://egret.<ns>.svc.cluster.local:8080`.

### 3.3 Deploy — `deploy/helm/egret` + `justfile` + kind
- **`justfile`**: `images-build` also builds `egret-kernel:dev` (`docker build kernel-image -t egret-kernel:dev`); `deploy-kind*` also `kind load docker-image egret-kernel:dev`.
- **Namespace** `templates/namespace-kernels.yaml` (model on `namespace-engines.yaml`): create `egret-notebooks`, labeled `pod-security.kubernetes.io/enforce: restricted`.
- **RBAC** `templates/rbac-kernels.yaml`: a `Role` in `egret-notebooks` — `pods` (get/list/watch/create/delete), `pods/log` (get) — bound to the `egret` ServiceAccount (cross-namespace RoleBinding: subject SA in `egret-system`, Role in `egret-notebooks`).
- **NetworkPolicy** `templates/networkpolicy-kernels.yaml` (the core hardening):
  - default-deny all ingress+egress in `egret-notebooks`;
  - allow **egress** to: kube-dns (UDP/TCP 53), the backend Service (`egret-system`, port 8080), the engines namespace (`egret-engines`, Trino/StarRocks ports), and MinIO (`egret-system`, 9000);
  - allow **ingress** from: the backend pod only (`egret-system`, to 8888).
  - Gated by `--set kernel.networkPolicy.enabled` (some CNIs — kind's default kindnet — don't enforce NetworkPolicy; document that enforcement needs Calico, but the policy is declared regardless).
- **backend-deployment env / configmap**: set `EGRET_KERNEL__LAUNCHER=kubernetes`, `EGRET_KERNEL__NAMESPACE`, `EGRET_KERNEL__IMAGE`, `EGRET_KERNEL__CALLBACK_URL`, resource values — gated on a helm `kernel.launcher` value (default `subprocess` so non-k8s installs are unaffected).
- **values.yaml**: a `kernel:` block (`launcher`, `image`, `namespace`, `resources`, `pullSecret`, `networkPolicy.enabled`).

## 4. Design decisions

### 4.1 Why the launcher in `egret-bin`, not `egret-kernel`
`egret-kernel` stays kube-free (its `FakeLauncher`/`SubprocessLauncher` need no cluster; its tests run anywhere). `egret-bin` already has `kube`. The trait is the seam.

### 4.2 Pod IP, not a Service (matches sparklabx)
sparklabx addresses kernels as `http://<podIP>:8888` directly (`readyURL`). The backend is in-cluster and reaches Pod IPs on the pod network. No per-kernel Service to create/GC. On pod IP churn (restart), the manager's next `ensure`/`status` re-resolves. The WS proxy (D1c) already dials the handle URL, so no proxy change.

### 4.3 Isolation model
The kernel Pod holds only the **per-user OIDC-delegated JWT + per-kernel callback token** (F2, already built) — never a privileged credential. That plus non-root + dropped caps + the NetworkPolicy is egret's equivalent of sparklabx's per-user MinIO IAM. Governed reads still go through engines (Ranger/OIDC); private scratch via the user's `notebook_{user}` namespace.

## 5. Testing
- **Unit (egret-bin, no cluster):** `build_pod_spec` — asserts name determinism, labels, non-root securityContext, dropped caps, resources from cfg, env passthrough + `KERNEL_GATEWAY_PORT`, port 8888, readiness probe, pull-secret presence/absence. `pod_name(key)` determinism + DNS-safety. Phase mapping from a fake pod status (Ready/ImagePullBackOff/CrashLoop/absent → Ready/Dead/Spawning).
- **Manager (existing):** unchanged — still uses `FakeLauncher`.
- **Live (kind, manual):** `just deploy-kind-oidc` with `kernel.launcher=kubernetes`; open a notebook, Run a Python cell → a `kernel-*` Pod appears in `egret-notebooks`, reaches Ready, output returns; Run a SQL cell (engine target) → governed result binds; delete kernel / idle reap → Pod deleted; backend restart → re-attaches to the live Pod (deterministic name).
- **CI:** full backend `cargo fmt/clippy/test` (the k8s launcher's pure fns are unit-tested; the live path is not in CI, documented). FE unaffected.

## 6. Risks / gotchas
- **kindnet doesn't enforce NetworkPolicy** — the policy is declared and correct but only enforced under Calico/Cilium. Document; keep `networkPolicy.enabled` default-on but note kind won't block.
- **Callback URL** must be cluster-internal or engine-token callbacks fail; fall back to `api_url` only if that is already cluster-internal.
- **RBAC cross-namespace**: the RoleBinding lives in `egret-notebooks`, subject is `system:serviceaccount:egret-system:egret`.
- **Image load**: `deploy-kind` must `kind load egret-kernel:dev` or pods `ErrImageNeverPull`/`ImagePullBackOff`; imagePullPolicy `IfNotPresent` + kind-loaded image.
- **First-cell latency**: cold pod spawn (image already on node via kind load) ~5–15s; the FE already shows a `spawning` phase (D1c connect returns `{phase:spawning}` and retries).
- **metrics-server** not required for this feature (ResourceUsageBadge deferred).

## 7. Phasing (for the plan)
- **P-A (backend):** `KubernetesLauncher` + `build_pod_spec`/`pod_name`/phase-map + unit tests; config fields; main.rs launcher switch + callback-url plumbing.
- **P-B (deploy):** helm namespace + RBAC + NetworkPolicy + backend env/values; justfile kernel-image build + kind load.
- **P-C (live-prove on kind):** deploy with `kernel.launcher=kubernetes`, run a cell end-to-end, verify Pod lifecycle + reap + restart-reattach.
