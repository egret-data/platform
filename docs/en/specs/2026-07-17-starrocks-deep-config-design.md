# StarRocks Deep Cluster Config (Trino-level) — Design

**Date:** 2026-07-17 · **Status:** Design — awaiting approval · **Branch:** `feat/starrocks-deep-config` (off `main`)
**Program:** StarRocks-parity-with-Trino, config-depth follow-on. The previous epic (#265) gave StarRocks editable Configuration/Security tabs at the *essential* level (image/replicas/resources/delegation). This brings the cluster-config **depth** to parity with Trino's engine detail — `fe.conf`/`cn.conf` overrides, CN autoscaling, and Pod & Containers/scheduling.

> Vietnamese reference: `docs/vi/specs/2026-07-17-starrocks-deep-config-design.md`.

## Problem

Trino's Configuration tab exposes deep tuning — Server (query max memory, log level, arbitrary config props, JVM/env), Workload, Pod & Containers (serviceAccount, security context, nodeSelector, affinity, tolerations, env, init/sidecar containers), and Autoscaling (HPA/KEDA). StarRocks currently exposes only Image / Compute (replicas) / Resources / Storage. The `fe.conf` egret generates is **hardcoded** (`starrocks_fe_configmap` — fixed `JAVA_OPTS -Xmx8192m`, `sys_log_level = INFO`, ports, run_mode), with no admin override; there is **no** `cn.conf`; and the CR carries no autoscaling or pod-level scheduling fields — even though the operator's `StarRocksCluster` CR supports all of them (`autoScalingPolicy` on the CN spec; LoadSpec `serviceAccount`/`nodeSelector`/`affinity`/`tolerations`/`annotations`/`initContainers`/`sidecars`/`imagePullSecrets`/`podLabels`).

## Goal

Deepen the StarRocks Configuration tab to Trino-level detail by adding four sections, all editable + save-and-reprovision (reusing the #265 update/restart machinery):

1. **FE config** — `fe.conf` key-value overrides (JVM heap via `JAVA_OPTS`, `sys_log_level`, query timeouts, any key), merged over egret's generated base (admin wins).
2. **CN config** — `cn.conf` key-value overrides, via a new CN ConfigMap wired onto `starRocksCnSpec.configMapInfo`.
3. **Autoscaling** — CN HPA (`autoScalingPolicy.hpaPolicy`): enable + min/max replicas + target CPU%.
4. **Pod & Containers** — serviceAccount, nodeSelector, annotations, env, tolerations/affinity/securityContext (raw-JSON, validated), init/sidecar containers, imagePullSecrets — applied cluster-wide to **both** FE and CN.

Proven live: set an FE `fe.conf` override + a CN HPA + a nodeSelector → Save re-provisions → the FE ConfigMap carries the merged key, the CN spec carries the `autoScalingPolicy`, and the pods carry the scheduling fields.

## Non-goals

- **Per-role Pod & Containers split** — MVP applies pod/scheduling fields to both FE and CN uniformly; a per-role (FE-only / CN-only) split is a later refinement.
- **BE (non-shared-data) topology** — shared-data FE+CN only, as today.
- **KEDA autoscaling** — HPA only (the operator's `autoScalingPolicy.hpaPolicy`); KEDA later if needed.
- **Changing storage / run_mode / the egret-owned shared-data keys** — the FE-config merge must NOT let an admin override the storage/run_mode/ssl/ranger keys egret manages (those stay authoritative); the override editor targets *tuning* keys.
- **New authz / delegation mechanisms** — unchanged from #265.

## Background — current state (verified)

- **`starrocks_fe_configmap`** (`egret-orchestrator/src/starrocks.rs:40`) builds `fe.conf` as a `String` via `push_str` and returns `json!({ …, "data": { "fe.conf": conf } })`. The egret-owned keys (ports, `run_mode`, `aws_s3_*`, `access_control = ranger`, `ssl_*`) are appended based on the spec. There is no CN ConfigMap.
- **`starrocks_cluster_cr`** (`:211`) sets FE/CN `image`, `replicas`, `limits`/`requests`, `configMapInfo` (FE→`fe.conf`), plus the SSL keystore `storageVolumes`/`initContainers` and the restart annotation. No autoscaling, no pod/scheduling fields.
- **`StarRocksConfig`** (`egret-engine-mgmt-api/src/lib.rs:407`) is the CR-building contract: `fe_image`/`cn_image`/`fe_replicas`/`cn_replicas`/`storage_*`/`fe_resources`/`cn_resources`/`delegation`. It's assembled by `build_starrocks_provision_req` from the stored `engine.spec["starrocksConfig"]`, which `build_starrocks_spec` builds from the body (both in `egret-engine-mgmt/src/engines.rs`, from #265).
- **Operator support** (`starrocks-kubernetes-operator/pkg/apis/starrocks/v1/`): `AutoScalingPolicy{ version, hpaPolicy:{metrics, behavior}, minReplicas, maxReplicas }` on FeSpec/CnSpec; LoadSpec carries `annotations`, `podLabels`, `serviceAccount`, `nodeSelector`, `schedulerName`, `affinity`, `tolerations`, `initContainers`, `sidecars`, `imagePullSecrets`, security context.
- **Frontend `config/` toolkit** (`frontend/src/routes/control-plane/config/`) is engine-agnostic and reusable: `KeyedPropertyList`/`PropertyList` (key-value editors), `PlacementSection` (nodeSelector/affinity/tolerations), `PodSection` + `ContainersMountsSection` (serviceAccount/security context/init/sidecar/env), `RawJsonField` + `validityRegistry`/`ValidityProvider` (invalid raw-JSON blocks Save). `StarRocksConfigTab` (from #265) already mirrors `ConfigurationTab`'s left-nav + `SaveBar` shell.

## Architecture — components

### A. Contract: extend `StarRocksConfig` (`egret-engine-mgmt-api/src/lib.rs`)

Add optional fields (all default-empty so existing engines are unaffected):
- `fe_config: BTreeMap<String, String>` / `cn_config: BTreeMap<String, String>` — conf overrides.
- `cn_autoscaling: Option<CnAutoscaling>` where `CnAutoscaling { min_replicas: Option<u32>, max_replicas: u32, target_cpu_percent: Option<u32> }`.
- Pod block `pod: Option<StarRocksPod>` where `StarRocksPod { service_account: Option<String>, node_selector: BTreeMap<String,String>, annotations: BTreeMap<String,String>, pod_labels: BTreeMap<String,String>, env: Vec<EnvVar>, tolerations: Option<Value>, affinity: Option<Value>, security_context: Option<Value>, init_containers: Option<Value>, sidecars: Option<Value>, image_pull_secrets: Vec<String> }` (raw `Value` for the complex k8s types, mirroring how the FE stores them as JSON).

### B. Provisioning: render the new fields (`egret-orchestrator/src/starrocks.rs`)

1. **FE config merge** — refactor `starrocks_fe_configmap` to build an *ordered map* of the egret base, then apply `sr.fe_config` overrides (admin key overrides the base line; new keys appended), **rejecting** overrides of the protected egret-owned keys (`run_mode`, `aws_s3_*`, `cloud_native_*`, `access_control`, `ssl_*`, the ports) — a protected override is dropped with a warning (or a 400 at the API layer; see §C). Re-serialize to the `fe.conf` string.
2. **CN config** — new `starrocks_cn_configmap(spec)` → `{name}-cn-cm` with `cn.conf` = the `sr.cn_config` overrides (CN has no egret-mandated base beyond operator defaults; only emit the ConfigMap when `cn_config` is non-empty). Wire `starRocksCnSpec.configMapInfo = { configMapName, resolveKey: "cn.conf" }` in `starrocks_cluster_cr`, and have the provisioner apply the CN ConfigMap alongside the FE one.
3. **Autoscaling** — when `cn_autoscaling` is set, render `starRocksCnSpec.autoScalingPolicy = { version: "v2", minReplicas, maxReplicas, hpaPolicy: { metrics: [<CPU utilization MetricSpec at target_cpu_percent>] } }`. (When autoscaling is on, the operator manages replicas; `cn_replicas` becomes the initial/floor — document the interaction.)
4. **Pod & Containers** — render the `sr.pod` fields onto BOTH `starRocksFeSpec` and `starRocksCnSpec` (serviceAccount, nodeSelector, annotations, podLabels, env, tolerations, affinity, securityContext, initContainers, sidecars, imagePullSecrets). Merge carefully with the SSL init-container already injected on FE (append admin initContainers, don't clobber the keystore builder).

### C. Backend spec/body plumbing (`egret-engine-mgmt/src/engines.rs`)

- Extend `StarRocksConfigBody` (the `b.starrocks` wire type) with the new fields (camelCase: `feConfig`, `cnConfig`, `cnAutoscaling`, `pod`).
- `build_starrocks_spec` stores them in `starrocksConfig` (omit=preserve on update, same rule as #265).
- `build_starrocks_provision_req` reads them back into `StarRocksConfig`.
- **Validation:** reject a `feConfig` that targets a protected egret-owned key with a `400` (clear message) — the authoritative place to enforce §B.1's protection so the admin gets immediate feedback.

### D. Frontend (`frontend/src/routes/control-plane/`)

- **`useStarRocksConfigForm`** — add state + seed for `feConfig`/`cnConfig` (Record<string,string>), `cnAutoscaling`, and the pod block; include them in the PUT body; extend the `resolveConfig`-based dirty computation (so blank/omitted stays consistent, as #265 established).
- **`StarRocksConfigTab`** — add nav sections **FE config**, **CN config**, **Autoscaling**, **Pod & Containers**, reusing `KeyedPropertyList`/`PropertyList` (conf overrides), a small autoscaling form (min/max/target CPU), and `PodSection`/`PlacementSection`/`ContainersMountsSection` (pod). Wrap the raw-JSON fields in the existing `ValidityProvider` so invalid JSON blocks Save (mirror `ConfigurationTab`). Add per-section non-default counts to the nav badges if cheap (reuse the Trino pattern) — optional.

## Data flow

Edit a deep-config section → `useStarRocksConfigForm.save()` → `PUT /engines/{id}` (extended `starrocks` body) → `update_starrocks_engine` → `build_starrocks_spec` (validate protected keys → 400) → persist → `build_starrocks_provision_req` → `StarRocksProvisioner::update` re-applies the CR (+ FE/CN ConfigMaps) → operator rolls the affected role. (Reuses #265's update path verbatim.)

## Testing strategy

- **Unit (CI):**
  - Provisioner: `starrocks_fe_configmap` merges an override (admin key wins) + preserves/rejects a protected key; `starrocks_cn_configmap` emits `cn.conf` only when non-empty; `starrocks_cluster_cr` renders `autoScalingPolicy` (min/max/CPU metric) on CN, the pod fields on FE+CN, and the FE SSL init-container survives alongside an admin sidecar.
  - Backend: `build_starrocks_spec` round-trips the new fields (omit=preserve); `build_starrocks_provision_req` reads them; protected-key `feConfig` → 400.
  - Frontend: `useStarRocksConfigForm` seeds + builds the PUT body for each section; the new tab sections render + submit; invalid raw-JSON blocks Save.
- **Live (controller-driven):** on a real 3.4 StarRocks — set an FE `sys_log_level`/heap override + a CN HPA (min 1 max 3, 70% CPU) + a nodeSelector → Save → confirm the FE ConfigMap carries the merged key, the CN spec/HPA object exists (`kubectl get hpa`/the CR's `autoScalingPolicy`), and the pods carry the nodeSelector. Report exactly what ran.

## Live-only risks (validated at smoke)

1. **fe.conf duplicate-key semantics** — confirm StarRocks honors the merged (de-duplicated) conf; the parse-and-replace merge avoids duplicates regardless.
2. **CN configMapInfo + operator** — confirm the operator mounts a CN `cn.conf` via `starRocksCnSpec.configMapInfo` the same way it does FE (resolveKey/mountPath).
3. **autoScalingPolicy** — confirm the operator creates the HPA and it doesn't fight the static `cn_replicas` (document precedence: autoscaling on → replicas is the floor).
4. **Pod-field merge on FE** — the admin `initContainers`/`storageVolumes` must not clobber the SSL keystore builder egret injects for TLS engines.

## Work breakdown (for the plan)

1. **Contract + body/spec plumbing** — extend `StarRocksConfig` + `StarRocksConfigBody` + `build_starrocks_spec`/`build_starrocks_provision_req`; protected-key 400; tests.
2. **FE config merge** — `starrocks_fe_configmap` ordered-map merge + protected-key guard; tests.
3. **CN config** — `starrocks_cn_configmap` + wire `starRocksCnSpec.configMapInfo` + provisioner applies it; tests.
4. **Autoscaling** — render `autoScalingPolicy` on CN; tests.
5. **Pod & Containers** — render pod/scheduling fields on FE+CN (SSL-init-safe merge); tests.
6. **Frontend — FE/CN config + Autoscaling sections** — `useStarRocksConfigForm` + `StarRocksConfigTab` (KeyedPropertyList + autoscaling form); tests.
7. **Frontend — Pod & Containers section** — reuse PodSection/PlacementSection/ContainersMountsSection + ValidityProvider; tests.
8. **Full CI + live smoke.**

## Self-review

- **Scope focused:** four new config sections + their CR rendering; reuses #265's update/restart + the FE `config/` toolkit; storage/run_mode/authz untouched (protected). ✔
- **No placeholders:** concrete types (`StarRocksConfig`/`CnAutoscaling`/`StarRocksPod`), the exact merge point (`starrocks_fe_configmap`), the operator fields (`autoScalingPolicy`, LoadSpec), and the reusable FE components are named. ✔
- **Consistency:** merge = parse-and-replace (override wins, protected keys guarded); pod fields cluster-wide (both FE+CN) for MVP; raw-JSON validated via the existing ValidityProvider. ✔
- **Ambiguity:** "Trino-level" pinned to the four sections above, not a literal field-for-field copy; autoscaling = HPA only; protected egret-owned keys can't be overridden. ✔
