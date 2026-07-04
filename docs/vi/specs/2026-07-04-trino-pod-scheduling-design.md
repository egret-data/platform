# Config pod & scheduling cho Trino (Phase 1.5.3, 2026-07-04)

Bản EN (canonical): `docs/en/specs/2026-07-04-trino-pod-scheduling-design.md`.
Nối tiếp engine provisioning đã ship (1.5/1.5.1/1.5.2/1.6a/1.6b) + IA config mới.

**Mục tiêu:** Thêm config đặt pod + hình dạng pod còn thiếu cho Trino Deployment — chạy Ở
ĐÂU (nodeSelector/tolerations/affinity/topologySpread), NHƯ AI (serviceAccount, security
context), TỪ ĐÂU (imagePull secrets/policy), KÈM GÌ (init/sidecar container, mount thêm,
envFrom, deployment strategy). Frontend: 1 sub-tab **Pod & Scheduling** mới. **1 increment,
structured**, riêng 3 object k8s rất sâu (affinity, init/sidecar container) dùng
**structured-common + raw-JSON override** để editor gọn.

**Non-goals:** custom `lifecycle` ngoài preStop graceful-shutdown (đã có); autoscaling
(1.5.4); observability (1.5.5); mount egret Secret thành file (config/secret mount tham
chiếu object k8s theo tên).

## Data model
**Per-role (thêm vào `RoleConfig`):** `node_selector` (map) · `tolerations` (Vec, builder) ·
`topology_spread` (Vec, builder) · `affinity` (**structured-common + raw override**:
node_match + pod_anti_affinity_topology_key, hoặc raw_override verbatim) ·
`termination_grace_seconds` · `additional_exposed_ports` (Vec {name,port}).

**Cluster-wide (thêm vào `TrinoConfig`):** `image_pull_secrets` (list) · `image_pull_policy`
(enum) · `service_account` · `pod_security_context`/`container_security_context`
(flat fields + raw_override) · `init_containers`/`sidecar_containers` (MiniContainer
structured-common {name,image,command,args,env,volume_mounts} + raw_override) · `env_from`
({configMapRef|secretRef}) · `config_mounts`/`secret_mounts` ({name,mount_path,sub_path,
default_mode} tham chiếu ConfigMap/Secret k8s có sẵn) · `deployment_strategy` (kind +
maxSurge/maxUnavailable) · `deployment_annotations` · `revision_history_limit` ·
`progress_deadline_seconds` · `share_process_namespace`.

Optional/rỗng ⇒ không render (manifest cũ không đổi).

## Orchestrator (`trino_deployment`)
Splice vào **podSpec**: nodeSelector/tolerations/affinity/topologySpread/serviceAccountName/
imagePullSecrets/securityContext/initContainers/shareProcessNamespace + volume mount. Vào
**container**: imagePullPolicy/securityContext/ports/envFrom/volumeMounts + append sidecar.
Vào **Deployment spec**: strategy/revisionHistoryLimit/progressDeadlineSeconds + annotations.

**Merge (không regress):** worker graceful-shutdown SỞ HỮU terminationGracePeriodSeconds +
preStop khi bật; `termination_grace_seconds` áp cho coordinator (và worker khi graceful tắt).
Mount config-file subPath (1.5.2) + base volume giữ nguyên, mount mới append. `raw_override`
render verbatim, thắng field structured.

## Control-plane + egret-bin
Carrier kube-free mirror từng field; `build_provision_req` map thẳng (không resolve secret —
env_from/mount tham chiếu tên; imagePullSecrets là tên). `spec_from` map carrier→orchestrator.
`raw_override` = `serde_json::Value` opaque; validate = JSON hợp lệ (API server từ chối object
sai khi apply → lỗi nổi qua provision path).

## Frontend — sub-tab Pod & Scheduling (thứ 7)
Thêm vào `useEngineConfigForm`. Phần **cluster-wide** (serviceAccount, imagePullSecrets,
pullPolicy, securityContext flat + raw textarea, envFrom, config/secret mounts, init/sidecar
mini-container builder + raw override, strategy/annotations/revisionHistoryLimit/
progressDeadline/shareProcessNamespace) + khối **per-role** có toggle Coordinator/Worker
(nodeSelector, tolerations, topologySpread, affinity node-match + anti-affinity topologyKey +
raw override, termination grace, ports). Component `PodSchedulingSection` riêng (tránh phình
file lớn). Textarea raw-JSON validate khi blur, sai thì chặn Save.

## Test
Unit (mỗi field vào đúng chỗ podSpec/container/Deployment; affinity structured assemble đúng;
raw_override thắng verbatim; config/secret mount thêm volume+mount; graceful-shutdown grace
KHÔNG bị đè; rỗng không render) + live kind (nodeSelector label có thật + toleration +
serviceAccount + securityContext runAsUser + pullPolicy + sidecar → provision → pod Ready →
`kubectl get pod -o yaml` verify; raw-override affinity render verbatim).

Compat: field optional, không migration, đổi config roll pod qua checksum. Chi tiết xem bản EN.
