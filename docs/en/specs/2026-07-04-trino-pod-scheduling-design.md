# Trino pod & scheduling config (Phase 1.5.3, 2026-07-04)

VI reference: `docs/vi/specs/2026-07-04-trino-pod-scheduling-design.md`.
Builds on the shipped engine provisioning (1.5 / 1.5.1 / 1.5.2 / 1.6a / 1.6b) and the
re-architected config UI (`useEngineConfigForm` + Configuration sub-tabs).

**Goal:** Add the Kubernetes pod-placement + pod-shape config still missing from the
provisioned Trino Deployments — where pods run (nodeSelector/tolerations/affinity/
topology spread), as whom (serviceAccount, security contexts), from where (imagePull
secrets/policy), and with what extras (init/sidecar containers, extra mounts, envFrom,
deployment strategy). Frontend surfaces it in a new **Pod & Scheduling** Configuration
sub-tab. **One increment, structured**, with a **structured-common + raw-JSON override**
for the three deeply-nested objects (affinity, init/sidecar containers) so their editors
stay tractable.

**Non-goals:** custom pod `lifecycle` beyond the worker graceful-shutdown preStop (that
already exists); HPA/KEDA autoscaling (1.5.4); JMX/Prometheus observability (1.5.5);
mounting egret-managed Secrets as files (config/secret mounts reference k8s objects by
name — the admin creates them).

## Data model

### Per-role (added to `RoleConfig`, so coordinator + worker each carry them)
- `node_selector: BTreeMap<String,String>`
- `tolerations: Vec<Toleration>` — `Toleration { key: Option<String>, operator: Option<String>, value: Option<String>, effect: Option<String>, toleration_seconds: Option<i64> }` (structured builder).
- `topology_spread: Vec<TopologySpreadConstraint>` — `{ max_skew: u32, topology_key: String, when_unsatisfiable: String, match_labels: BTreeMap<String,String> }` (structured builder).
- `affinity: Option<Affinity>` — **structured-common + raw override**:
  `Affinity { node_match: Vec<NodeSelectorRequirement{key,operator,values:Vec<String>}>, pod_anti_affinity_topology_key: Option<String>, raw_override: Option<serde_json::Value> }`.
  When `raw_override` is `Some`, it is emitted verbatim as the pod's `affinity`. Otherwise
  egret assembles: `requiredDuringSchedulingIgnoredDuringExecution.nodeAffinity` from
  `node_match` (one `nodeSelectorTerm` of `matchExpressions`), and a
  `preferredDuringSchedulingIgnoredDuringExecution` `podAntiAffinity` (weight 100,
  `topologyKey` = the value, `labelSelector` = `app: {name}-{role}`) when the topology key
  is set.
- `termination_grace_seconds: Option<u32>`.
- `additional_exposed_ports: Vec<ContainerPort>` — `{ name: String, container_port: u16 }`.

### Cluster-wide (added to `TrinoConfig`, applied to both roles)
- `image_pull_secrets: Vec<String>` (secret names) · `image_pull_policy: Option<String>` (`Always`|`IfNotPresent`|`Never`) · `service_account: Option<String>`.
- `pod_security_context: Option<SecurityContext>` — structured-common `{ run_as_user: Option<i64>, run_as_group: Option<i64>, fs_group: Option<i64>, run_as_non_root: Option<bool>, raw_override: Option<Value> }`.
- `container_security_context: Option<SecurityContext>` — same struct (fs_group ignored at container level; `read_only_root_fs`/`allow_privilege_escalation` folded into `raw_override` when needed).
- `init_containers: Vec<MiniContainer>` + `sidecar_containers: Vec<MiniContainer>` — **structured-common + raw override**: `MiniContainer { name: String, image: String, command: Vec<String>, args: Vec<String>, env: BTreeMap<String,String>, volume_mounts: Vec<VolumeMount{name,mount_path,sub_path:Option<String>}>, raw_override: Option<Value> }`. When `raw_override` is `Some`, that container object is emitted verbatim.
- `env_from: Vec<EnvFromSource>` — `{ config_map_ref: Option<String>, secret_ref: Option<String> }` (one set per entry).
- `config_mounts: Vec<ObjectMount>` + `secret_mounts: Vec<ObjectMount>` — `{ name: String (existing k8s ConfigMap/Secret), mount_path: String, sub_path: Option<String>, default_mode: Option<i32> }`. Each adds a `volume` (configMap/secret) + a `volumeMount`.
- `deployment_strategy: Option<DeploymentStrategy>` — `{ kind: "RollingUpdate"|"Recreate", max_surge: Option<String>, max_unavailable: Option<String> }`.
- `deployment_annotations: BTreeMap<String,String>` · `revision_history_limit: Option<u32>` · `progress_deadline_seconds: Option<u32>` · `share_process_namespace: Option<bool>`.

All optional/empty ⇒ nothing rendered (current manifests unchanged).

## Orchestrator rendering (`trino.rs`, `trino_deployment`)

Splice into the **pod spec** (per-role from the role config; cluster-wide from TrinoConfig):
`nodeSelector`, `tolerations`, `affinity`, `topologySpreadConstraints`,
`serviceAccountName`, `imagePullSecrets`, `securityContext` (pod), `initContainers`,
`shareProcessNamespace`, and the config/secret-mount `volumes`. Into the **container**:
`imagePullPolicy`, `securityContext` (container), extra `ports`, `envFrom`, and the
config/secret-mount `volumeMounts`; append sidecar containers to `containers`. Into the
**Deployment spec**: `strategy`, `revisionHistoryLimit`, `progressDeadlineSeconds`, and
merge `deployment_annotations` into the Deployment metadata.

**Merge rules (no regressions):**
- Worker graceful-shutdown already sets `terminationGracePeriodSeconds` + the preStop
  `lifecycle`. When graceful shutdown is enabled it OWNS the worker's grace period; the
  standalone `termination_grace_seconds` applies to the coordinator, and to the worker only
  when graceful shutdown is off.
- The optional config-file `subPath` mounts (1.5.2) and the base config volume already in
  the pod are preserved; the new config/secret mounts are appended.
- `raw_override` (affinity / security contexts / mini-containers) is emitted verbatim and
  wins over the structured fields for that object.

## Control-plane (`engines.rs`) + `egret-bin`

Kube-free carriers mirror every type field-for-field; `build_provision_req` maps them
straight through (no secret resolution — `env_from`/mounts reference k8s objects by name;
`image_pull_secrets` are names). `spec_from` (egret-bin) maps carriers → orchestrator
types. `raw_override` fields are `serde_json::Value` passed opaquely; validation is
"is it valid JSON" (the wire already delivers parsed JSON) — the API server rejects a
malformed object at apply time and the error surfaces through the existing provision path.

## Frontend — new **Pod & Scheduling** Configuration sub-tab

Add a 7th Configuration sub-tab `Pod & Scheduling`, added to `useEngineConfigForm`:
- **Cluster-wide** section: serviceAccount, imagePullSecrets (list), pullPolicy (Select),
  pod + container securityContext (the flat fields + a raw-JSON `<Textarea>` override),
  envFrom (configMap/secret ref rows), config-mounts + secret-mounts (name/path/subPath
  rows), init + sidecar containers (a mini-container builder with a raw-JSON override per
  container), deployment strategy + annotations + revisionHistoryLimit +
  progressDeadlineSeconds + shareProcessNamespace.
- **Per-role** block with a Coordinator/Worker toggle: nodeSelector (KV), tolerations
  (rows), topologySpread (rows), affinity (node-match rows + anti-affinity topologyKey +
  raw-JSON override), termination grace, additional ports (rows).
Reuse `KeyValueEditor`; new small row-builders live in a `PodSchedulingSection` component
(kept out of the already-large files). Raw-JSON textareas validate JSON on blur and show
an inline error; an invalid raw field blocks Save.

## Test

- **Unit** (`trino.rs`): each field renders into the right podSpec/container/Deployment
  location; the structured affinity assembles the expected node/anti-affinity; a
  `raw_override` wins verbatim; config/secret mounts add volume + volumeMount; **worker
  graceful-shutdown grace period is NOT clobbered** by `termination_grace_seconds`; nothing
  renders when unset.
- **Live (kind)**: set nodeSelector (a label that exists on the kind node), a toleration,
  serviceAccount, pod securityContext `runAsUser`, an imagePullPolicy, and a sidecar
  container → provision → both pods Ready → `kubectl get pod -o yaml` shows the
  nodeSelector/toleration/serviceAccountName/securityContext/sidecar; a raw-override
  affinity renders verbatim. (Use a schedulable nodeSelector so pods still come up.)

## Compat
- All fields optional; no DB migration; a config change rolls the pods via the existing
  checksum annotation. No API/route removals.
