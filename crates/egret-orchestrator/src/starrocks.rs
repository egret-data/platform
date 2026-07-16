//! StarRocks shared-data templating + provisioner. Emits the kube-starrocks
//! operator's `starrocks.com/v1 StarRocksCluster` CR plus its `v1/ConfigMap`s;
//! unlike Trino (plain Deployments), the operator owns the StatefulSets/pods
//! for the FE (metadata) and CN (stateless compute) roles. Shared-data mode
//! keeps table data in the object store, configured via `fe.conf`'s builtin
//! storage-volume block — there is no BE role.

use crate::kube_client::KubeClient;
use crate::provisioner::EngineProvisioner;
use crate::spec::{EngineSpec, EngineStatus, PodStatus, RoleResources};
use serde_json::{json, Value};
use std::sync::Arc;

/// Render `RoleResources` as k8s `limits`/`requests`, omitting empty sides —
/// mirrors `trino.rs`'s `trino_deployment` resource rendering, but the
/// operator's CR takes `limits`/`requests` directly on the role spec (no
/// nested `resources` object).
fn resources_json(r: &RoleResources) -> (Value, Value) {
    let mut limits = serde_json::Map::new();
    let mut requests = serde_json::Map::new();
    if let Some(v) = &r.cpu_limit {
        limits.insert("cpu".into(), json!(v));
    }
    if let Some(v) = &r.mem_limit {
        limits.insert("memory".into(), json!(v));
    }
    if let Some(v) = &r.cpu_request {
        requests.insert("cpu".into(), json!(v));
    }
    if let Some(v) = &r.mem_request {
        requests.insert("memory".into(), json!(v));
    }
    (Value::Object(limits), Value::Object(requests))
}

/// The FE role's `fe.conf` ConfigMap (`{name}-fe-cm`). Templates the base
/// ports/log block, the `run_mode = shared_data` object-store volume config
/// sourced from `spec.starrocks`, and — only when a Ranger binding is present
/// — `access_control = ranger`.
pub fn starrocks_fe_configmap(spec: &EngineSpec) -> Value {
    let sr = spec
        .starrocks
        .as_ref()
        .expect("starrocks_fe_configmap requires spec.starrocks");

    let mut conf = String::new();
    conf.push_str("LOG_DIR = ${STARROCKS_HOME}/log\n");
    conf.push_str("DATE = \"$(date +%Y%m%d-%H%M%S)\"\n");
    conf.push_str(
        "JAVA_OPTS=\"-Dlog4j2.formatMsgNoLookups=true -Xmx8192m -XX:+UseG1GC -Xlog:gc*:${LOG_DIR}/fe.gc.log.$DATE:time\"\n",
    );
    conf.push_str("http_port = 8030\n");
    conf.push_str("rpc_port = 9020\n");
    conf.push_str("query_port = 9030\n");
    conf.push_str("edit_log_port = 9010\n");
    conf.push_str("mysql_service_nio_enabled = true\n");
    conf.push_str("sys_log_level = INFO\n");
    conf.push_str("# config for shared-data mode\n");
    conf.push_str("run_mode = shared_data\n");
    conf.push_str("cloud_native_meta_port = 6090\n");
    conf.push_str("cloud_native_storage_type = S3\n");
    conf.push_str(&format!("aws_s3_endpoint = {}\n", sr.storage_endpoint));
    conf.push_str(&format!("aws_s3_path = {}\n", sr.storage_path));
    conf.push_str(&format!("aws_s3_access_key = {}\n", sr.storage_access_key));
    conf.push_str(&format!("aws_s3_secret_key = {}\n", sr.storage_secret_key));
    conf.push_str("aws_s3_use_aws_sdk_default_behavior = false\n");
    conf.push_str("aws_s3_enable_path_style_access = true\n");
    conf.push_str("enable_load_volume_from_conf = true\n");
    if spec.ranger.is_some() {
        conf.push_str("access_control = ranger\n");
    }
    // FE TLS on :9030 (Access delegation parity, slice 3): the exact `ssl_*`
    // keys are LIVE-ONLY (the T4 smoke tunes them against the `fe-ubuntu`
    // image); this documented set is the starting point. `ssl_force_secure_transport
    // = false` allows plaintext to keep working alongside TLS in dev — the
    // connector (S3-T1) opts into TLS per-connection via `options.tls_cert`.
    if let Some(d) = sr.delegation.as_ref().filter(|d| d.tls_enabled) {
        let _ = d;
        conf.push_str("ssl_keystore_location = /etc/starrocks/ssl/keystore\n");
        conf.push_str("ssl_keystore_password = changeit\n");
        conf.push_str("ssl_key_password = changeit\n");
        conf.push_str("ssl_force_secure_transport = false\n");
    }

    json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-fe-cm", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": { "fe.conf": conf }
    })
}

/// The Ranger plugin's `ranger-starrocks-security.xml` + audit ConfigMap
/// (`{name}-ranger-cm`). `None` unless `spec.ranger` is bound.
///
/// CRITICAL: no XML comment here may contain a `--` (double-hyphen) sequence
/// — that crashes the StarRocks FE boot (proven live in an earlier slice).
/// This renderer emits no comments at all to stay safe by construction.
pub fn starrocks_ranger_configmap(spec: &EngineSpec) -> Option<Value> {
    let r = spec.ranger.as_ref()?;
    let security_xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<configuration>\n\
  <property><name>ranger.plugin.starrocks.service.name</name><value>{svc}</value></property>\n\
  <property><name>ranger.plugin.starrocks.policy.rest.url</name><value>{url}</value></property>\n\
  <property><name>ranger.plugin.starrocks.policy.source.impl</name><value>org.apache.ranger.admin.client.RangerAdminRESTClient</value></property>\n\
  <property><name>ranger.plugin.starrocks.policy.cache.dir</name><value>/tmp/ranger/{svc}/policycache</value></property>\n\
  <property><name>ranger.plugin.starrocks.policy.pollIntervalMs</name><value>{poll}</value></property>\n\
  <property><name>ranger.plugin.starrocks.policy.rest.client.connection.timeoutMs</name><value>120000</value></property>\n\
  <property><name>ranger.plugin.starrocks.policy.rest.client.read.timeoutMs</name><value>30000</value></property>\n\
</configuration>\n",
        svc = r.service_name, url = r.egret_url, poll = r.poll_interval_ms
    );
    let audit_xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<configuration>\n\
  <property><name>xasecure.audit.is.enabled</name><value>false</value></property>\n\
</configuration>\n"
        .to_string();
    Some(json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-ranger-cm", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": {
            "ranger-starrocks-security.xml": security_xml,
            "ranger-starrocks-audit.xml": audit_xml,
        }
    }))
}

/// The FE TLS cert ConfigMap (`{name}-ssl-cm`). `None` unless the engine's
/// delegation config has `tls_enabled`. Carries ONLY the public cert
/// (`fe.crt` = `tls_cert_pem`) — informational/debugging. The cert+KEY bundle
/// used to build the keystore lives in [`starrocks_ssl_secret`], NOT here: a
/// private key must never sit in a plaintext ConfigMap.
pub fn starrocks_ssl_configmap(spec: &EngineSpec) -> Option<Value> {
    let d = spec
        .starrocks
        .as_ref()
        .and_then(|sr| sr.delegation.as_ref())
        .filter(|d| d.tls_enabled)?;
    Some(json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-ssl-cm", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": { "fe.crt": d.tls_cert_pem }
    }))
}

/// The FE TLS cert+KEY Secret (`{name}-ssl-secret`). `None` unless the
/// engine's delegation config has `tls_enabled`. Carries the cert+key PEM
/// bundle (`fe.pem` — `StarRocksDelegation::tls_keystore_pem`, i.e.
/// `ensure_tls_cert`'s `keystore_pem`) the T2c init-container (see
/// `starrocks_cluster_cr`) reads to build the Java keystore
/// `ssl_keystore_location` points at. This is a `v1/Secret` (NOT a ConfigMap)
/// because `fe.pem` contains the FE's private key. Mounted read-only via the
/// FE spec's `secrets` field at the SOURCE path (`/etc/starrocks/ssl-src`),
/// NOT `/etc/starrocks/ssl` — that path is the init-container-built keystore
/// volume.
pub fn starrocks_ssl_secret(spec: &EngineSpec) -> Option<Value> {
    let d = spec
        .starrocks
        .as_ref()
        .and_then(|sr| sr.delegation.as_ref())
        .filter(|d| d.tls_enabled)?;
    Some(json!({
        "apiVersion": "v1", "kind": "Secret",
        "type": "Opaque",
        "metadata": { "name": format!("{}-ssl-secret", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": { "fe.pem": d.tls_keystore_pem }
    }))
}

/// Name of the writable `storageVolumes` entry (see `starrocks_cluster_cr`)
/// the build-keystore init-container writes `keystore` into and the FE main
/// container reads `ssl_keystore_location` from.
const SSL_KEYSTORE_VOLUME_NAME: &str = "ssl-keystore";
const SSL_KEYSTORE_MOUNT_PATH: &str = "/etc/starrocks/ssl";

/// Pod-template annotation key stamped onto BOTH the FE and CN roles to force a
/// rolling restart. The kube-starrocks operator copies each role spec's
/// `annotations` map straight onto that role's pod-template `ObjectMeta`
/// (confirmed against the operator source: `pkg/subcontrollers/fe/fe_pod.go` and
/// `cn/cn_pod.go` both do `annotations := pod.Annotations(spec)` →
/// `PodTemplateSpec.ObjectMeta.Annotations`, and `pod.Annotations` just returns
/// `spec.GetAnnotations()` verbatim). Changing this annotation changes the pod
/// template, so the operator reconciles the StatefulSet and rolls its pods —
/// exactly the `kubectl rollout restart` effect, but through the CR (patching
/// the operator-owned StatefulSet directly would be reverted by the operator).
const RESTART_STAMP_KEY: &str = "egret.io/restarted-at";

/// JSON merge-patch (RFC 7386) body that stamps `stamp` onto the FE + CN pod
/// annotations of an existing `StarRocksCluster` CR. Merge semantics mean this
/// only sets the one annotation key on each role — every other CR field, and any
/// other annotations, are left untouched. Pure fn so the restart wiring is
/// unit-testable without a live cluster (see `restart_stamps_fe_and_cn`).
pub fn starrocks_restart_patch(stamp: &str) -> Value {
    json!({
        "spec": {
            "starRocksFeSpec": { "annotations": { RESTART_STAMP_KEY: stamp } },
            "starRocksCnSpec": { "annotations": { RESTART_STAMP_KEY: stamp } },
        }
    })
}

/// The `starrocks.com/v1 StarRocksCluster` CR: FE + CN roles only (shared-data
/// mode has no BE). FE mounts `fe.conf` via `configMapInfo`, plus the Ranger
/// XML ConfigMap under `configMaps` when a binding is present. When
/// delegation TLS is enabled it also gets the TLS cert+key source Secret (via
/// `secrets`), a build-keystore `initContainers` entry, and a writable
/// `storageVolumes` keystore volume (see the T2c comment block below for the
/// CRD constraints that shape this).
pub fn starrocks_cluster_cr(spec: &EngineSpec) -> Value {
    let sr = spec
        .starrocks
        .as_ref()
        .expect("starrocks_cluster_cr requires spec.starrocks");

    let mut fe_spec = json!({
        "image": sr.fe_image,
        "replicas": sr.fe_replicas,
        "configMapInfo": {
            "configMapName": format!("{}-fe-cm", spec.name),
            "resolveKey": "fe.conf",
        }
    });
    if let Some(r) = &sr.fe_resources {
        let (limits, requests) = resources_json(r);
        if limits.as_object().is_some_and(|m| !m.is_empty()) {
            fe_spec["limits"] = limits;
        }
        if requests.as_object().is_some_and(|m| !m.is_empty()) {
            fe_spec["requests"] = requests;
        }
    }
    // Extra ConfigMaps the FE mounts: the Ranger XML (when bound). The TLS
    // cert+key SOURCE is a Secret (see `starrocks_ssl_secret`), mounted via
    // the FE spec's `secrets` field below — NOT here, since it carries the
    // private key. The cert-only `{name}-ssl-cm` ConfigMap is still applied
    // (public-cert reference) but is not needed for the keystore build, so it
    // is not mounted into the FE.
    let mut extra_config_maps: Vec<Value> = Vec::new();
    if spec.ranger.is_some() {
        extra_config_maps.push(json!({
            "name": format!("{}-ranger-cm", spec.name),
            "mountPath": "/opt/starrocks/fe/conf",
        }));
    }
    if !extra_config_maps.is_empty() {
        fe_spec["configMaps"] = json!(extra_config_maps);
    }
    // T2c: build the Java keystore `ssl_keystore_location` needs, from the
    // cert+key PEM the `{name}-ssl-secret` Secret carries (`fe.pem`).
    //
    // CRD CONSTRAINT (checked against `deploy/starrocks-operator/crd.yaml`):
    // `starRocksFeSpec` has NO generic top-level `volumes` field — only
    // `configMaps`/`secrets` (name+mountPath+subPath, always read-only
    // ConfigMap/Secret projections), `storageVolumes` (hostPath OR PVC,
    // name+mountPath — the only WRITABLE volume type the CRD exposes), and
    // `initContainers` (a full container spec, including its own
    // `volumeMounts`). So the writable share between the init-container and
    // the main FE container has to be a `storageVolumes` entry (hostPath
    // here, as the closest available substitute for `emptyDir`), not a plain
    // `volumes: [{emptyDir: {}}]` — that field doesn't exist on this CRD. The
    // cert+key SOURCE is projected read-only via the `secrets` field (the
    // CRD's RO Secret-projection mount) so the private key never touches a
    // ConfigMap.
    //
    // ASSUMPTION (UNVERIFIED, live-only): the init-container's `volumeMounts`
    // reference pod-level volume NAMES. For `storageVolumes` the name is
    // exactly what we set below (`SSL_KEYSTORE_VOLUME_NAME`), so that half is
    // solid. For the `secrets` entry there is no separate "volume name" field
    // in the CRD schema — only the Secret's own `name` — so this assumes the
    // operator names the generated pod volume identically to the Secret
    // object's name (`{ssl_secret_name}`) and that this volume is reachable
    // from `initContainers[].volumeMounts` (not just the main container). If
    // the live smoke finds the operator uses a different internal volume
    // name, or doesn't expose Secret-derived volumes to init containers, this
    // mount needs a live-tuned fix (candidate: an initContainer image that
    // reads the cert+key via a k8s API/downward mechanism instead of a shared
    // volume).
    let tls_enabled = starrocks_ssl_secret(spec).is_some();
    if tls_enabled {
        let ssl_secret_name = format!("{}-ssl-secret", spec.name);
        // Writable volume the init-container builds `keystore` into and the FE
        // main container reads via `ssl_keystore_location`. `storageVolumes`
        // names are deterministic (the operator does NOT hash them), so the
        // init-container can reference `ssl-keystore` reliably.
        fe_spec["storageVolumes"] = json!([{
            "name": SSL_KEYSTORE_VOLUME_NAME,
            "mountPath": SSL_KEYSTORE_MOUNT_PATH,
            "hostPath": {
                "path": format!("/var/lib/egret/starrocks/{}/ssl-keystore", spec.name),
                "type": "DirectoryOrCreate",
            },
        }]);
        // The cert+key SOURCE is delivered to the init-container via a
        // `secretKeyRef` ENV var (referencing the `{name}-ssl-secret` Secret by
        // OBJECT NAME) rather than a mounted volume — the kube-starrocks
        // operator renames `secrets`/`configMaps` pod volumes to
        // `<name>-<fnv32a(mountInfo)[:4]>` (`getVolumeName`, VolumeNameWithHash),
        // a nondeterministic suffix an `initContainers[].volumeMounts` entry
        // can't reference. `secretKeyRef` sidesteps the volume-name hashing
        // entirely. The FE main container doesn't need the PEM (it reads the
        // built keystore), so no `secrets` mount is templated.
        //
        // LIVE-ONLY (flagged per T2c scope): the keystore FORMAT (PKCS12 here,
        // built by `openssl pkcs12 -export` straight from the PEM cert+key) and
        // whether StarRocks' `ssl_keystore_location` accepts PKCS12 vs requiring
        // JKS (`keytool`) is verified/tuned at the live smoke.
        fe_spec["initContainers"] = json!([{
            "name": "build-keystore",
            "image": "alpine/openssl:latest",
            "command": ["sh", "-c"],
            "args": [format!(
                "printf '%s' \"$FE_PEM\" > /tmp/fe.pem && \
                 openssl pkcs12 -export -in /tmp/fe.pem -inkey /tmp/fe.pem -out {dst}/keystore -passout pass:changeit -name fe && \
                 echo keystore built",
                dst = SSL_KEYSTORE_MOUNT_PATH,
            )],
            "env": [{
                "name": "FE_PEM",
                "valueFrom": { "secretKeyRef": { "name": ssl_secret_name, "key": "fe.pem" } },
            }],
            "volumeMounts": [
                { "name": SSL_KEYSTORE_VOLUME_NAME, "mountPath": SSL_KEYSTORE_MOUNT_PATH },
            ],
        }]);
    }

    let mut cn_spec = json!({
        "image": sr.cn_image,
        "replicas": sr.cn_replicas,
    });
    if let Some(r) = &sr.cn_resources {
        let (limits, requests) = resources_json(r);
        if limits.as_object().is_some_and(|m| !m.is_empty()) {
            cn_spec["limits"] = limits;
        }
        if requests.as_object().is_some_and(|m| !m.is_empty()) {
            cn_spec["requests"] = requests;
        }
    }

    json!({
        "apiVersion": "starrocks.com/v1", "kind": "StarRocksCluster",
        "metadata": { "name": spec.name, "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "spec": {
            "starRocksFeSpec": fe_spec,
            "starRocksCnSpec": cn_spec,
        }
    })
}

/// Provisions a StarRocks engine (shared-data mode) via the kube-starrocks
/// operator: an `fe.conf` ConfigMap, an optional Ranger XML ConfigMap, and the
/// `StarRocksCluster` CR. The operator owns the FE/CN StatefulSets and pods.
pub struct StarRocksProvisioner {
    kube: Arc<dyn KubeClient>,
}
impl StarRocksProvisioner {
    pub fn new(kube: Arc<dyn KubeClient>) -> Self {
        Self { kube }
    }

    /// Apply the fe.conf ConfigMap, the Ranger XML ConfigMap (when bound), the
    /// TLS cert ConfigMap + cert+key Secret (when delegation TLS is enabled),
    /// and the CR in order. On the happy path these are the objects a
    /// StarRocks engine needs; `provision` best-effort rolls them back on
    /// error.
    async fn apply_all(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        self.kube.apply(&starrocks_fe_configmap(spec)).await?;
        if let Some(cm) = starrocks_ranger_configmap(spec) {
            self.kube.apply(&cm).await?;
        }
        if let Some(cm) = starrocks_ssl_configmap(spec) {
            self.kube.apply(&cm).await?;
        }
        if let Some(secret) = starrocks_ssl_secret(spec) {
            self.kube.apply(&secret).await?;
        }
        self.kube.apply(&starrocks_cluster_cr(spec)).await?;
        Ok(())
    }

    /// Roll the FE + CN pods by JSON-merge-patching a fresh restart-stamp
    /// annotation onto the existing `StarRocksCluster` CR's fe/cn specs; the
    /// operator propagates the annotation to each role's pod template, which
    /// changes the template and rolls the StatefulSet pods (see
    /// [`starrocks_restart_patch`] / [`RESTART_STAMP_KEY`]).
    ///
    /// A targeted merge-patch is used rather than a full CR re-apply: restart
    /// only carries `namespace`+`name` (not the full `EngineSpec`), and a merge
    /// patch can't wipe operator-managed fields the way a partial server-side
    /// apply could. The `stamp` is generated by the caller (egret-bin, which has
    /// `std::time`) so this stays a pure, deterministic operation.
    pub async fn restart(&self, namespace: &str, name: &str, stamp: &str) -> anyhow::Result<()> {
        self.kube
            .merge_patch(
                "starrocks.com/v1",
                "StarRocksCluster",
                namespace,
                name,
                &starrocks_restart_patch(stamp),
            )
            .await
    }
}

#[async_trait::async_trait]
impl EngineProvisioner for StarRocksProvisioner {
    async fn provision(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // Mirror `TrinoProvisioner::provision`: if any apply fails partway, the
        // earlier objects (e.g. the ConfigMaps) would be orphaned, so best-effort
        // delete everything before returning the error.
        if let Err(e) = self.apply_all(spec).await {
            let _ = self.delete(&spec.namespace, &spec.name).await;
            return Err(e);
        }
        Ok(())
    }
    async fn update(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        self.provision(spec).await?;
        // `apply_all` only applies the Ranger ConfigMap when bound, so toggling a
        // RangerBinding OFF would otherwise leave `{name}-ranger-cm` orphaned (the
        // CR's `configMaps` mount is dropped, but the ConfigMap lingers).
        // Best-effort delete it when Ranger is absent.
        if spec.ranger.is_none() {
            let _ = self
                .kube
                .delete(
                    "v1",
                    "ConfigMap",
                    &spec.namespace,
                    &format!("{}-ranger-cm", spec.name),
                )
                .await;
        }
        // Mirror the same orphan-cleanup for the TLS ConfigMap AND the cert+key
        // Secret when delegation TLS is toggled off.
        if starrocks_ssl_secret(spec).is_none() {
            let _ = self
                .kube
                .delete(
                    "v1",
                    "ConfigMap",
                    &spec.namespace,
                    &format!("{}-ssl-cm", spec.name),
                )
                .await;
            let _ = self
                .kube
                .delete(
                    "v1",
                    "Secret",
                    &spec.namespace,
                    &format!("{}-ssl-secret", spec.name),
                )
                .await;
        }
        Ok(())
    }
    async fn status(&self, namespace: &str, name: &str) -> anyhow::Result<EngineStatus> {
        // The kube-starrocks operator owns the FE/CN pods and stamps
        // `app.starrocks.ownerreference/name=<cluster>-fe` / `-cn` (NOT the
        // `app.kubernetes.io/instance=<name>` the Trino path sets on its own
        // Deployment pods — verified at the T8 live smoke). Query both owner
        // labels and merge so the control-plane health UI sees FE + CN pods.
        let mut pods = self
            .kube
            .list_pods_selector(
                namespace,
                &format!("app.starrocks.ownerreference/name={name}-fe"),
            )
            .await?;
        pods.extend(
            self.kube
                .list_pods_selector(
                    namespace,
                    &format!("app.starrocks.ownerreference/name={name}-cn"),
                )
                .await?,
        );
        let pod_status: Vec<PodStatus> = pods
            .into_iter()
            .map(|(name, phase, ready)| PodStatus { name, phase, ready })
            .collect();
        let phase = if pod_status.is_empty() {
            "provisioning"
        } else if pod_status.iter().all(|p| p.ready) {
            "running"
        } else if pod_status.iter().any(|p| p.phase == "Failed") {
            "failed"
        } else {
            "provisioning"
        };
        Ok(EngineStatus {
            phase: phase.into(),
            message: None,
            pods: pod_status,
        })
    }
    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        self.kube
            .delete("starrocks.com/v1", "StarRocksCluster", namespace, name)
            .await?;
        let _ = self
            .kube
            .delete("v1", "ConfigMap", namespace, &format!("{name}-fe-cm"))
            .await;
        let _ = self
            .kube
            .delete("v1", "ConfigMap", namespace, &format!("{name}-ranger-cm"))
            .await;
        let _ = self
            .kube
            .delete("v1", "ConfigMap", namespace, &format!("{name}-ssl-cm"))
            .await;
        let _ = self
            .kube
            .delete("v1", "Secret", namespace, &format!("{name}-ssl-secret"))
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::*;

    fn sample_sr_spec() -> EngineSpec {
        EngineSpec {
            name: "sr1".into(),
            namespace: "egret-engines".into(),
            image_version: "3.3-latest".into(),
            config: TrinoConfig::default(),
            catalogs: vec![],
            ranger: None,
            kind: EngineKind::StarRocks,
            starrocks: Some(StarRocksConfig {
                fe_image: "starrocks/fe-ubuntu:3.3-latest".into(),
                cn_image: "starrocks/cn-ubuntu:3.3-latest".into(),
                fe_replicas: 1,
                cn_replicas: 1,
                storage_endpoint: "http://host.docker.internal:9000".into(),
                storage_path: "warehouse/starrocks".into(),
                storage_access_key: "egret".into(),
                storage_secret_key: "egretsecret".into(),
                fe_resources: None,
                cn_resources: None,
                delegation: None,
            }),
        }
    }

    /// `sample_sr_spec` plus a delegation block with TLS enabled — the fixture
    /// for the ssl-config / ssl-cm tests below.
    fn sample_sr_spec_with_tls() -> EngineSpec {
        let mut spec = sample_sr_spec();
        spec.starrocks.as_mut().unwrap().delegation = Some(StarRocksDelegation {
            jwks_url: "https://kc/realms/egret/protocol/openid-connect/certs".into(),
            issuer: "https://kc/realms/egret".into(),
            audience: "egret".into(),
            tls_cert_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n".into(),
            tls_keystore_pem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n\
                                -----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----\n"
                .into(),
            tls_enabled: true,
        });
        spec
    }

    #[test]
    fn cr_has_fe_and_cn_shared_data() {
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        assert_eq!(cr["apiVersion"], "starrocks.com/v1");
        assert_eq!(cr["kind"], "StarRocksCluster");
        let spec = &cr["spec"];
        assert_eq!(
            spec["starRocksFeSpec"]["image"],
            "starrocks/fe-ubuntu:3.3-latest"
        );
        assert_eq!(
            spec["starRocksFeSpec"]["configMapInfo"]["resolveKey"],
            "fe.conf"
        );
        assert_eq!(
            spec["starRocksCnSpec"]["image"],
            "starrocks/cn-ubuntu:3.3-latest"
        );
        assert!(spec.get("starRocksBeSpec").is_none()); // no BE in shared-data
    }

    #[test]
    fn fe_configmap_has_shared_data_storage_and_ports() {
        let cm = starrocks_fe_configmap(&sample_sr_spec());
        let conf = cm["data"]["fe.conf"].as_str().unwrap();
        assert!(conf.contains("run_mode = shared_data"));
        assert!(conf.contains("query_port = 9030"));
        assert!(conf.contains("cloud_native_storage_type = S3"));
        assert!(conf.contains("aws_s3_endpoint = http://host.docker.internal:9000"));
        assert!(conf.contains("aws_s3_enable_path_style_access = true"));
        assert!(conf.contains("enable_load_volume_from_conf = true"));
        assert!(!conf.contains("access_control = ranger")); // not unless bound
        assert!(!conf.contains("ssl_")); // not unless delegation.tls_enabled
    }

    #[test]
    fn ranger_binding_adds_access_control_and_xml_configmap() {
        let mut spec = sample_sr_spec();
        spec.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: 30_000,
        });
        let conf = starrocks_fe_configmap(&spec)["data"]["fe.conf"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(conf.contains("access_control = ranger"));
        let xml_cm = starrocks_ranger_configmap(&spec).unwrap();
        let xml = xml_cm["data"]["ranger-starrocks-security.xml"]
            .as_str()
            .unwrap();
        assert!(xml.contains("<name>ranger.plugin.starrocks.service.name</name>"));
        assert!(xml.contains("<value>sr1</value>"));
        assert!(xml.contains("http://host.docker.internal:8080"));
        let cr = starrocks_cluster_cr(&spec);
        let mounts = cr["spec"]["starRocksFeSpec"]["configMaps"]
            .as_array()
            .unwrap();
        assert!(mounts
            .iter()
            .any(|m| m["name"] == xml_cm["metadata"]["name"]));
    }

    #[test]
    fn no_ranger_binding_omits_xml_configmap() {
        assert!(starrocks_ranger_configmap(&sample_sr_spec()).is_none());
    }

    /// TDD (T2 step 2): `ssl_*` keys appear ONLY when `delegation.tls_enabled`.
    #[test]
    fn delegation_tls_enabled_adds_ssl_config_and_configmap() {
        let spec = sample_sr_spec_with_tls();
        let conf = starrocks_fe_configmap(&spec)["data"]["fe.conf"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(conf.contains("ssl_keystore_location = /etc/starrocks/ssl/keystore"));
        assert!(conf.contains("ssl_keystore_password = changeit"));
        assert!(conf.contains("ssl_key_password = changeit"));
        assert!(conf.contains("ssl_force_secure_transport = false"));

        // The ConfigMap carries ONLY the public cert — never the private key.
        let ssl_cm = starrocks_ssl_configmap(&spec).expect("ssl-cm present when tls_enabled");
        assert_eq!(ssl_cm["metadata"]["name"], "sr1-ssl-cm");
        assert!(ssl_cm["data"]["fe.crt"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE"));
        assert!(
            ssl_cm["data"].get("fe.pem").is_none(),
            "cert+key bundle must NOT be in the ConfigMap"
        );
        let cm_json = ssl_cm.to_string();
        assert!(
            !cm_json.contains("PRIVATE KEY"),
            "no private key may appear anywhere in the ConfigMap"
        );

        // The cert+KEY bundle lives in a Secret, keyed `fe.pem`.
        let ssl_secret = starrocks_ssl_secret(&spec).expect("ssl-secret present when tls_enabled");
        assert_eq!(ssl_secret["kind"], "Secret");
        assert_eq!(ssl_secret["type"], "Opaque");
        assert_eq!(ssl_secret["metadata"]["name"], "sr1-ssl-secret");
        let fe_pem = ssl_secret["stringData"]["fe.pem"].as_str().unwrap();
        assert!(fe_pem.contains("BEGIN CERTIFICATE"));
        assert!(fe_pem.contains("BEGIN PRIVATE KEY"));

        let cr = starrocks_cluster_cr(&spec);
        // The cert+key Secret is delivered to the init-container via a
        // `secretKeyRef` ENV (by object name), NOT a `secrets` volume mount —
        // the operator hashes secret/configmap volume names, which an
        // init-container can't reference. So no `secrets` field is templated.
        assert!(cr["spec"]["starRocksFeSpec"].get("secrets").is_none());
        let init = &cr["spec"]["starRocksFeSpec"]["initContainers"][0];
        let env = init["env"].as_array().unwrap();
        assert!(env.iter().any(|e| e["name"] == "FE_PEM"
            && e["valueFrom"]["secretKeyRef"]["name"] == ssl_secret["metadata"]["name"]
            && e["valueFrom"]["secretKeyRef"]["key"] == "fe.pem"));
        // And the cert-only ConfigMap is NOT mounted into the FE (it's not
        // needed for the keystore build), so no `configMaps` entry references
        // it when Ranger is absent.
        assert!(cr["spec"]["starRocksFeSpec"].get("configMaps").is_none());
    }

    /// T2c TDD: when delegation TLS is enabled, the FE spec gets a
    /// build-keystore `initContainers` entry (mounting the cert+key Secret
    /// source + the writable keystore volume) and a `storageVolumes` entry
    /// providing that writable keystore volume at `/etc/starrocks/ssl` —
    /// matching `ssl_keystore_location = /etc/starrocks/ssl/keystore`. The
    /// keystore source comes from the SECRET, never a ConfigMap.
    #[test]
    fn keystore_volume_and_init_container_present() {
        let spec = sample_sr_spec_with_tls();
        let cr = starrocks_cluster_cr(&spec);
        let fe_spec = &cr["spec"]["starRocksFeSpec"];

        let volumes = fe_spec["storageVolumes"].as_array().unwrap();
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0]["name"], "ssl-keystore");
        assert_eq!(volumes[0]["mountPath"], "/etc/starrocks/ssl");
        assert!(volumes[0]["hostPath"]["path"].is_string());

        let init_containers = fe_spec["initContainers"].as_array().unwrap();
        assert_eq!(init_containers.len(), 1);
        let init = &init_containers[0];
        assert_eq!(init["name"], "build-keystore");
        assert!(init["command"].is_array());
        let args = init["args"].as_array().unwrap();
        assert!(args[0].as_str().unwrap().contains("openssl"));
        assert!(args[0].as_str().unwrap().contains("/tmp/fe.pem"));
        assert!(args[0]
            .as_str()
            .unwrap()
            .contains("/etc/starrocks/ssl/keystore"));

        // The init-container reads the cert+key from the Secret via a
        // `secretKeyRef` env (FE_PEM) — NOT a mounted volume (the operator
        // hashes secret volume names). It mounts ONLY the writable keystore vol.
        let env = init["env"].as_array().unwrap();
        assert!(env.iter().any(|e| e["name"] == "FE_PEM"
            && e["valueFrom"]["secretKeyRef"]["name"] == "sr1-ssl-secret"
            && e["valueFrom"]["secretKeyRef"]["key"] == "fe.pem"));
        let init_mounts = init["volumeMounts"].as_array().unwrap();
        assert!(init_mounts
            .iter()
            .any(|m| m["name"] == "ssl-keystore" && m["mountPath"] == "/etc/starrocks/ssl"));
        assert!(
            !init_mounts
                .iter()
                .any(|m| m["name"] == "sr1-ssl-secret" || m["name"] == "sr1-ssl-cm"),
            "init-container sources the key via secretKeyRef env, not a volume"
        );
    }

    /// No delegation TLS → no init-container/storageVolumes/secrets noise on
    /// the FE spec.
    #[test]
    fn no_delegation_tls_omits_keystore_init_container() {
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        let fe_spec = &cr["spec"]["starRocksFeSpec"];
        assert!(fe_spec.get("initContainers").is_none());
        assert!(fe_spec.get("storageVolumes").is_none());
        assert!(fe_spec.get("secrets").is_none());
    }

    /// The private key must NEVER appear in any ConfigMap egret templates for
    /// a TLS-enabled StarRocks engine (fe-cm, ranger-cm, or ssl-cm).
    #[test]
    fn no_private_key_in_any_configmap() {
        let mut spec = sample_sr_spec_with_tls();
        spec.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: 30_000,
        });
        for cm in [
            Some(starrocks_fe_configmap(&spec)),
            starrocks_ranger_configmap(&spec),
            starrocks_ssl_configmap(&spec),
        ]
        .into_iter()
        .flatten()
        {
            assert_eq!(cm["kind"], "ConfigMap");
            assert!(
                !cm.to_string().contains("PRIVATE KEY"),
                "private key leaked into a ConfigMap: {}",
                cm["metadata"]["name"]
            );
        }
        // The key IS present — but only in the Secret.
        let secret = starrocks_ssl_secret(&spec).unwrap();
        assert!(secret.to_string().contains("PRIVATE KEY"));
    }

    /// No delegation / TLS-off → no ssl Secret (mirrors the ConfigMap gating).
    #[test]
    fn no_delegation_omits_ssl_secret() {
        assert!(starrocks_ssl_secret(&sample_sr_spec()).is_none());
        let mut spec = sample_sr_spec_with_tls();
        spec.starrocks
            .as_mut()
            .unwrap()
            .delegation
            .as_mut()
            .unwrap()
            .tls_enabled = false;
        assert!(starrocks_ssl_secret(&spec).is_none());
    }

    /// No delegation block at all → no ssl config/configmap (slice-2 default).
    #[test]
    fn no_delegation_omits_ssl_config_and_configmap() {
        assert!(starrocks_ssl_configmap(&sample_sr_spec()).is_none());
    }

    /// Delegation present but `tls_enabled = false` → also no ssl config/configmap.
    #[test]
    fn delegation_without_tls_enabled_omits_ssl_config_and_configmap() {
        let mut spec = sample_sr_spec_with_tls();
        spec.starrocks
            .as_mut()
            .unwrap()
            .delegation
            .as_mut()
            .unwrap()
            .tls_enabled = false;
        let conf = starrocks_fe_configmap(&spec)["data"]["fe.conf"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!conf.contains("ssl_"));
        assert!(starrocks_ssl_configmap(&spec).is_none());
        let cr = starrocks_cluster_cr(&spec);
        assert!(cr["spec"]["starRocksFeSpec"].get("configMaps").is_none());
    }

    /// Both Ranger AND TLS bound: the FE mounts the Ranger XML via
    /// `configMaps`, and the cert+key source reaches the keystore init-container
    /// via a `secretKeyRef` env (no `secrets` volume mount — the operator hashes
    /// those names). So `configMaps` carries only the Ranger entry, and no
    /// `secrets` field is templated.
    #[test]
    fn ranger_and_tls_mount_on_their_respective_fields() {
        let mut spec = sample_sr_spec_with_tls();
        spec.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: 30_000,
        });
        let cr = starrocks_cluster_cr(&spec);
        let cm_mounts = cr["spec"]["starRocksFeSpec"]["configMaps"]
            .as_array()
            .unwrap();
        assert_eq!(cm_mounts.len(), 1);
        assert!(cm_mounts.iter().any(|m| m["name"] == "sr1-ranger-cm"));
        assert!(
            !cm_mounts.iter().any(|m| m["name"] == "sr1-ssl-cm"),
            "cert-only ssl-cm is not mounted into the FE"
        );

        // No `secrets` volume mount; the key reaches the init-container via env.
        assert!(cr["spec"]["starRocksFeSpec"].get("secrets").is_none());
        let init = &cr["spec"]["starRocksFeSpec"]["initContainers"][0];
        assert!(init["env"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "FE_PEM"
                && e["valueFrom"]["secretKeyRef"]["name"] == "sr1-ssl-secret"));
    }

    /// T4 TDD: the restart merge-patch body stamps the restart annotation on
    /// BOTH the FE and CN pod-annotation field (`annotations`, which the
    /// operator propagates to each role's pod template), and the base CR carries
    /// no such annotation (restart is opt-in, never on a plain provision).
    #[test]
    fn restart_stamps_fe_and_cn() {
        let patch = starrocks_restart_patch("1720000000000");
        assert_eq!(
            patch["spec"]["starRocksFeSpec"]["annotations"]["egret.io/restarted-at"],
            "1720000000000"
        );
        assert_eq!(
            patch["spec"]["starRocksCnSpec"]["annotations"]["egret.io/restarted-at"],
            "1720000000000"
        );

        // A plain provision CR must NOT carry the restart stamp on either role.
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        assert!(cr["spec"]["starRocksFeSpec"].get("annotations").is_none());
        assert!(cr["spec"]["starRocksCnSpec"].get("annotations").is_none());
    }

    /// `StarRocksProvisioner::restart` JSON-merge-patches the existing
    /// `StarRocksCluster` CR (by name) with the stamp on both roles — it never
    /// re-applies the full CR (which would need the full spec and could wipe
    /// operator-managed fields).
    #[tokio::test]
    async fn provisioner_restart_merge_patches_cr() {
        use crate::kube_client::FakeKube;
        let kube = Arc::new(FakeKube::default());
        let prov = StarRocksProvisioner::new(kube.clone());
        prov.restart("egret-engines", "sr1", "1720000000000")
            .await
            .unwrap();
        let patched = kube.patched.lock().unwrap();
        assert_eq!(patched.len(), 1, "exactly one merge-patch");
        let (key, patch) = &patched[0];
        assert_eq!(key, "StarRocksCluster/sr1");
        assert_eq!(
            patch["spec"]["starRocksFeSpec"]["annotations"]["egret.io/restarted-at"],
            "1720000000000"
        );
        assert_eq!(
            patch["spec"]["starRocksCnSpec"]["annotations"]["egret.io/restarted-at"],
            "1720000000000"
        );
        // Restart must NOT re-apply the CR (no full-spec apply on this path).
        assert!(
            kube.applied.lock().unwrap().is_empty(),
            "restart uses merge-patch, not full apply"
        );
    }
}
