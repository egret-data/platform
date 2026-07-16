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

/// The FE TLS source ConfigMap (`{name}-ssl-cm`). `None` unless the engine's
/// delegation config has `tls_enabled`. Carries the public cert alone
/// (`fe.crt`, informational/debugging) AND the cert+key PEM bundle
/// (`fe.pem` — `StarRocksDelegation::tls_keystore_pem`, i.e.
/// `ensure_tls_cert`'s `keystore_pem`) the T2c init-container (see
/// `starrocks_cluster_cr`) reads to build the Java keystore
/// `ssl_keystore_location` points at. This ConfigMap is mounted at a SOURCE
/// path (`/etc/starrocks/ssl-src`), NOT `/etc/starrocks/ssl` — that path is
/// now the init-container-built keystore volume.
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
        "data": { "fe.crt": d.tls_cert_pem, "fe.pem": d.tls_keystore_pem }
    }))
}

/// Name of the writable `storageVolumes` entry (see `starrocks_cluster_cr`)
/// the build-keystore init-container writes `keystore` into and the FE main
/// container reads `ssl_keystore_location` from.
const SSL_KEYSTORE_VOLUME_NAME: &str = "ssl-keystore";
const SSL_KEYSTORE_MOUNT_PATH: &str = "/etc/starrocks/ssl";
const SSL_SRC_MOUNT_PATH: &str = "/etc/starrocks/ssl-src";

/// The `starrocks.com/v1 StarRocksCluster` CR: FE + CN roles only (shared-data
/// mode has no BE). FE mounts `fe.conf` via `configMapInfo`, plus the Ranger
/// XML ConfigMap under `configMaps` when a binding is present, and — when
/// delegation TLS is enabled — the TLS source ConfigMap + a build-keystore
/// `initContainers` entry + a writable `storageVolumes` keystore volume (see
/// the T2c comment block below for the CRD constraints that shape this).
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
    // Extra ConfigMaps the FE mounts: the Ranger XML (when bound) and the TLS
    // cert+key source (when the delegation config has TLS enabled). Both are
    // independent and additive, so collect whichever apply into one
    // `configMaps` array.
    let ssl_cm_name = format!("{}-ssl-cm", spec.name);
    let mut extra_config_maps: Vec<Value> = Vec::new();
    if spec.ranger.is_some() {
        extra_config_maps.push(json!({
            "name": format!("{}-ranger-cm", spec.name),
            "mountPath": "/opt/starrocks/fe/conf",
        }));
    }
    let tls_enabled = starrocks_ssl_configmap(spec).is_some();
    if tls_enabled {
        // SOURCE mount only — `/etc/starrocks/ssl` (SSL_KEYSTORE_MOUNT_PATH)
        // is now the init-container-built keystore volume below, not this
        // read-only ConfigMap.
        extra_config_maps.push(json!({
            "name": ssl_cm_name,
            "mountPath": SSL_SRC_MOUNT_PATH,
        }));
    }
    if !extra_config_maps.is_empty() {
        fe_spec["configMaps"] = json!(extra_config_maps);
    }
    // T2c: build the Java keystore `ssl_keystore_location` needs, from the
    // cert+key PEM the `{name}-ssl-cm` ConfigMap carries (`fe.pem`).
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
    // `volumes: [{emptyDir: {}}]` — that field doesn't exist on this CRD.
    //
    // ASSUMPTION (UNVERIFIED, live-only): the init-container's `volumeMounts`
    // reference pod-level volume NAMES. For `storageVolumes` the name is
    // exactly what we set below (`SSL_KEYSTORE_VOLUME_NAME`), so that half is
    // solid. For the `configMaps` entry there is no separate "volume name"
    // field in the CRD schema — only the ConfigMap's own `name` — so this
    // assumes the operator names the generated pod volume identically to the
    // ConfigMap object's name (`{ssl_cm_name}`) and that this volume is
    // reachable from `initContainers[].volumeMounts` (not just the main
    // container). If the live smoke finds the operator uses a different
    // internal volume name, or doesn't expose ConfigMap-derived volumes to
    // init containers, this mount needs a live-tuned fix (candidate: an
    // initContainer image that reads the cert+key via a k8s API/downward
    // mechanism instead of a shared volume).
    if tls_enabled {
        fe_spec["storageVolumes"] = json!([{
            "name": SSL_KEYSTORE_VOLUME_NAME,
            "mountPath": SSL_KEYSTORE_MOUNT_PATH,
            "hostPath": {
                "path": format!("/var/lib/egret/starrocks/{}/ssl-keystore", spec.name),
                "type": "DirectoryOrCreate",
            },
        }]);
        // LIVE-ONLY / best-guess (flagged per T2c scope): the exact command,
        // keystore FORMAT (PKCS12 vs JKS), and whether this init image even
        // has openssl are all things the live smoke must tune. PKCS12 is
        // used here because it's the format `openssl pkcs12 -export` builds
        // directly from a PEM cert+key with no intermediate JKS conversion;
        // whether StarRocks' `ssl_keystore_location` accepts PKCS12 (vs
        // requiring JKS, which would need `keytool` instead) is unverified.
        fe_spec["initContainers"] = json!([{
            "name": "build-keystore",
            "image": "alpine/openssl:latest",
            "command": ["sh", "-c"],
            "args": [format!(
                "openssl pkcs12 -export -in {src}/fe.pem -inkey {src}/fe.pem -out {dst}/keystore -passout pass:changeit -name fe && echo keystore built",
                src = SSL_SRC_MOUNT_PATH, dst = SSL_KEYSTORE_MOUNT_PATH,
            )],
            "volumeMounts": [
                { "name": ssl_cm_name, "mountPath": SSL_SRC_MOUNT_PATH, "readOnly": true },
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
    /// TLS cert ConfigMap (when delegation TLS is enabled), and the CR in
    /// order. On the happy path these are the objects a StarRocks engine
    /// needs; `provision` best-effort rolls them back on error.
    async fn apply_all(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        self.kube.apply(&starrocks_fe_configmap(spec)).await?;
        if let Some(cm) = starrocks_ranger_configmap(spec) {
            self.kube.apply(&cm).await?;
        }
        if let Some(cm) = starrocks_ssl_configmap(spec) {
            self.kube.apply(&cm).await?;
        }
        self.kube.apply(&starrocks_cluster_cr(spec)).await?;
        Ok(())
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
        // Mirror the same orphan-cleanup for the TLS ConfigMap when delegation
        // TLS is toggled off.
        if starrocks_ssl_configmap(spec).is_none() {
            let _ = self
                .kube
                .delete(
                    "v1",
                    "ConfigMap",
                    &spec.namespace,
                    &format!("{}-ssl-cm", spec.name),
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

        let ssl_cm = starrocks_ssl_configmap(&spec).expect("ssl-cm present when tls_enabled");
        assert_eq!(ssl_cm["metadata"]["name"], "sr1-ssl-cm");
        assert!(ssl_cm["data"]["fe.crt"]
            .as_str()
            .unwrap()
            .contains("BEGIN CERTIFICATE"));
        // T2c: the ssl-cm also carries the cert+key bundle the init-container
        // builds the keystore from.
        let fe_pem = ssl_cm["data"]["fe.pem"].as_str().unwrap();
        assert!(fe_pem.contains("BEGIN CERTIFICATE"));
        assert!(fe_pem.contains("BEGIN PRIVATE KEY"));

        let cr = starrocks_cluster_cr(&spec);
        // The ssl-cm is now mounted at the SOURCE path, not
        // `/etc/starrocks/ssl` — that path is the init-container-built
        // keystore volume (see `keystore_volume_and_init_container_present`).
        let mounts = cr["spec"]["starRocksFeSpec"]["configMaps"]
            .as_array()
            .unwrap();
        assert!(mounts
            .iter()
            .any(|m| m["name"] == ssl_cm["metadata"]["name"]
                && m["mountPath"] == "/etc/starrocks/ssl-src"));
    }

    /// T2c TDD: when delegation TLS is enabled, the FE spec gets a
    /// build-keystore `initContainers` entry (mounting the ssl-cm source +
    /// the writable keystore volume) and a `storageVolumes` entry providing
    /// that writable keystore volume at `/etc/starrocks/ssl` — matching
    /// `ssl_keystore_location = /etc/starrocks/ssl/keystore`.
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
        assert!(args[0]
            .as_str()
            .unwrap()
            .contains("/etc/starrocks/ssl-src/fe.pem"));
        assert!(args[0]
            .as_str()
            .unwrap()
            .contains("/etc/starrocks/ssl/keystore"));

        let init_mounts = init["volumeMounts"].as_array().unwrap();
        assert!(init_mounts
            .iter()
            .any(|m| m["name"] == "sr1-ssl-cm" && m["mountPath"] == "/etc/starrocks/ssl-src"));
        assert!(init_mounts
            .iter()
            .any(|m| m["name"] == "ssl-keystore" && m["mountPath"] == "/etc/starrocks/ssl"));
    }

    /// No delegation TLS → no init-container/storageVolumes noise on the FE spec.
    #[test]
    fn no_delegation_tls_omits_keystore_init_container() {
        let cr = starrocks_cluster_cr(&sample_sr_spec());
        let fe_spec = &cr["spec"]["starRocksFeSpec"];
        assert!(fe_spec.get("initContainers").is_none());
        assert!(fe_spec.get("storageVolumes").is_none());
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

    /// Both Ranger AND TLS bound: the FE mounts both ConfigMaps.
    #[test]
    fn ranger_and_tls_both_mount_their_configmaps() {
        let mut spec = sample_sr_spec_with_tls();
        spec.ranger = Some(RangerBinding {
            service_name: "sr1".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: 30_000,
        });
        let cr = starrocks_cluster_cr(&spec);
        let mounts = cr["spec"]["starRocksFeSpec"]["configMaps"]
            .as_array()
            .unwrap();
        assert_eq!(mounts.len(), 2);
        assert!(mounts.iter().any(|m| m["name"] == "sr1-ranger-cm"));
        assert!(mounts.iter().any(|m| m["name"] == "sr1-ssl-cm"));
    }
}
