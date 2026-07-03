//! Trino plain-manifest templating + provisioner. Emits `apps/v1 Deployment`,
//! `v1 Service/ConfigMap/Secret` as `serde_json::Value`; Trino runs from the
//! official `trinodb/trino:<version>` image with config in ConfigMaps.

use crate::kube_client::KubeClient;
use crate::provisioner::EngineProvisioner;
use crate::spec::{EngineSpec, EngineStatus, PodStatus};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub enum Role {
    Coordinator,
    Worker,
}
impl Role {
    fn suffix(self) -> &'static str {
        match self {
            Role::Coordinator => "coordinator",
            Role::Worker => "worker",
        }
    }
    fn is_coord(self) -> bool {
        matches!(self, Role::Coordinator)
    }
}

fn jvm_config(spec: &EngineSpec) -> String {
    let heap = spec.config.jvm_heap.clone().unwrap_or_else(|| "1G".into());
    [
        "-server",
        &format!("-Xmx{heap}"),
        &format!("-Xms{heap}"),
        "-XX:+UseG1GC",
        "-XX:G1HeapRegionSize=32M",
        "-XX:+ExplicitGCInvokesConcurrent",
        "-XX:+HeapDumpOnOutOfMemoryError",
        "-XX:+ExitOnOutOfMemoryError",
        "-XX:-OmitStackTraceInFastThrow",
        "-Djdk.attach.allowAttachSelf=true",
    ]
    .join("\n")
        + "\n"
}

fn config_properties(spec: &EngineSpec, coord: bool) -> String {
    let mut out = String::new();
    if coord {
        out.push_str("coordinator=true\n");
        let inc = spec.worker_replicas == 0;
        out.push_str(&format!("node-scheduler.include-coordinator={inc}\n"));
    } else {
        out.push_str("coordinator=false\n");
    }
    out.push_str("http-server.http.port=8080\n");
    out.push_str(&format!("discovery.uri=http://{}-trino:8080\n", spec.name));
    if let Some(v) = &spec.config.query_max_memory {
        out.push_str(&format!("query.max-memory={v}\n"));
    }
    if let Some(v) = &spec.config.query_max_memory_per_node {
        out.push_str(&format!("query.max-memory-per-node={v}\n"));
    }
    for (k, v) in &spec.config.config_properties {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}

pub fn trino_config_map(spec: &EngineSpec) -> Value {
    json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-config", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": {
            "config-coordinator.properties": config_properties(spec, true),
            "config-worker.properties": config_properties(spec, false),
            "jvm.config": jvm_config(spec),
            "node.properties": "node.environment=egret\nnode.data-dir=/data/trino\n",
        }
    })
}

pub fn trino_catalogs_config_map(spec: &EngineSpec) -> Value {
    let mut data = serde_json::Map::new();
    for c in &spec.catalogs {
        let body: String = c
            .properties
            .iter()
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect();
        data.insert(format!("{}.properties", c.name), json!(body));
    }
    json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-catalogs", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": data
    })
}

pub fn trino_env_secret(spec: &EngineSpec) -> Value {
    let mut data = serde_json::Map::new();
    for (k, v) in &spec.config.env {
        data.insert(k.clone(), json!(v));
    }
    for c in &spec.catalogs {
        for (k, v) in &c.env {
            data.insert(k.clone(), json!(v));
        }
    }
    json!({
        "apiVersion": "v1", "kind": "Secret",
        "metadata": { "name": format!("{}-env", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": data
    })
}

fn config_checksum(spec: &EngineSpec) -> String {
    // Rolls the pods whenever rendered config or catalogs change.
    let mut h = DefaultHasher::new();
    trino_config_map(spec).to_string().hash(&mut h);
    trino_catalogs_config_map(spec).to_string().hash(&mut h);
    trino_env_secret(spec).to_string().hash(&mut h);
    format!("{:016x}", h.finish())
}

pub fn trino_deployment(spec: &EngineSpec, role: Role) -> Value {
    let app = format!("{}-{}", spec.name, role.suffix());
    let cfg_file = if role.is_coord() {
        "config-coordinator.properties"
    } else {
        "config-worker.properties"
    };
    let replicas = if role.is_coord() {
        spec.coordinator_replicas
    } else {
        spec.worker_replicas
    };
    json!({
        "apiVersion": "apps/v1", "kind": "Deployment",
        "metadata": { "name": app, "namespace": spec.namespace,
                      "labels": { "app": app, "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "spec": {
            "replicas": replicas,
            "selector": { "matchLabels": { "app": app } },
            "template": {
                "metadata": { "labels": { "app": app, "app.kubernetes.io/instance": spec.name, "egret/engine": spec.name },
                              "annotations": { "egret/config-checksum": config_checksum(spec) } },
                "spec": {
                    "containers": [{
                        "name": "trino",
                        "image": format!("trinodb/trino:{}", spec.image_version),
                        "ports": [{ "containerPort": 8080 }],
                        "envFrom": [{ "secretRef": { "name": format!("{}-env", spec.name) } }],
                        "resources": {
                            "requests": { "cpu": spec.resources.cpu_min, "memory": spec.resources.memory },
                            "limits": { "cpu": spec.resources.cpu_max, "memory": spec.resources.memory }
                        },
                        "readinessProbe": { "httpGet": { "path": "/v1/info", "port": 8080 },
                            "initialDelaySeconds": 15, "periodSeconds": 10, "timeoutSeconds": 5, "failureThreshold": 12 },
                        "volumeMounts": [
                            { "name": "config", "mountPath": "/etc/trino/config.properties", "subPath": cfg_file },
                            { "name": "config", "mountPath": "/etc/trino/jvm.config", "subPath": "jvm.config" },
                            { "name": "config", "mountPath": "/etc/trino/node.properties", "subPath": "node.properties" },
                            { "name": "catalogs", "mountPath": "/etc/trino/catalog" },
                            { "name": "data", "mountPath": "/data/trino" }
                        ]
                    }],
                    "volumes": [
                        { "name": "config", "configMap": { "name": format!("{}-config", spec.name) } },
                        { "name": "catalogs", "configMap": { "name": format!("{}-catalogs", spec.name) } },
                        { "name": "data", "emptyDir": {} }
                    ]
                }
            }
        }
    })
}

pub fn trino_service(spec: &EngineSpec) -> Value {
    json!({
        "apiVersion": "v1", "kind": "Service",
        "metadata": { "name": format!("{}-trino", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "spec": { "selector": { "app": format!("{}-coordinator", spec.name) },
                  "ports": [{ "port": 8080, "targetPort": 8080, "name": "http" }] }
    })
}

/// Provisions a Trino engine as plain Kubernetes manifests: two Deployments
/// (coordinator + worker), a Service, two ConfigMaps (config + catalogs), and an
/// env Secret. A checksum annotation on the pod template rolls pods on change.
pub struct TrinoProvisioner {
    kube: Arc<dyn KubeClient>,
}
impl TrinoProvisioner {
    pub fn new(kube: Arc<dyn KubeClient>) -> Self {
        Self { kube }
    }
}

impl TrinoProvisioner {
    async fn apply_all(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // ConfigMaps + Secret first so the Deployments mount current data; the
        // checksum annotation on the pod template rolls pods when they change.
        self.kube.apply(&trino_config_map(spec)).await?;
        self.kube.apply(&trino_catalogs_config_map(spec)).await?;
        self.kube.apply(&trino_env_secret(spec)).await?;
        self.kube.apply(&trino_service(spec)).await?;
        self.kube
            .apply(&trino_deployment(spec, Role::Coordinator))
            .await?;
        self.kube
            .apply(&trino_deployment(spec, Role::Worker))
            .await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl EngineProvisioner for TrinoProvisioner {
    async fn provision(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        if let Err(e) = self.apply_all(spec).await {
            let _ = self.delete(&spec.namespace, &spec.name).await;
            return Err(e);
        }
        Ok(())
    }
    async fn status(&self, namespace: &str, name: &str) -> anyhow::Result<EngineStatus> {
        let pods = self.kube.list_pods(namespace, name).await?;
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
    async fn update(&self, spec: &EngineSpec) -> anyhow::Result<()> {
        // Re-apply everything; the recomputed checksum annotation rolls the pods
        // so Trino reloads the (static) config + catalogs.
        self.apply_all(spec).await
    }
    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        let _ = self
            .kube
            .delete(
                "apps/v1",
                "Deployment",
                namespace,
                &format!("{name}-coordinator"),
            )
            .await;
        let _ = self
            .kube
            .delete(
                "apps/v1",
                "Deployment",
                namespace,
                &format!("{name}-worker"),
            )
            .await;
        let _ = self
            .kube
            .delete("v1", "Service", namespace, &format!("{name}-trino"))
            .await;
        let _ = self
            .kube
            .delete("v1", "ConfigMap", namespace, &format!("{name}-config"))
            .await;
        let _ = self
            .kube
            .delete("v1", "ConfigMap", namespace, &format!("{name}-catalogs"))
            .await;
        let _ = self
            .kube
            .delete("v1", "Secret", namespace, &format!("{name}-env"))
            .await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::*;
    use std::collections::BTreeMap;

    fn sample_spec() -> EngineSpec {
        let mut props = BTreeMap::new();
        props.insert("iceberg.catalog.type".into(), "rest".into());
        props.insert(
            "iceberg.rest-catalog.uri".into(),
            "http://egret:8080/catalog".into(),
        );
        props.insert(
            "iceberg.rest-catalog.oauth2.credential".into(),
            "${ENV:EGRET_C0}".into(),
        );
        let mut env = BTreeMap::new();
        env.insert("EGRET_C0".into(), "id:secret".into());
        EngineSpec {
            name: "demo".into(),
            namespace: "egret-engines".into(),
            image_version: "470".into(),
            coordinator_replicas: 1,
            worker_replicas: 2,
            resources: Resources {
                cpu_min: "1".into(),
                cpu_max: "2".into(),
                memory: "4Gi".into(),
            },
            config: TrinoConfig {
                jvm_heap: Some("2G".into()),
                query_max_memory: Some("3GB".into()),
                query_max_memory_per_node: None,
                config_properties: BTreeMap::new(),
                env: BTreeMap::new(),
            },
            catalogs: vec![CatalogSpec {
                name: "egret".into(),
                properties: props,
                env,
            }],
        }
    }

    #[test]
    fn config_map_has_coordinator_and_worker_config() {
        let cm = trino_config_map(&sample_spec());
        assert_eq!(cm["kind"], "ConfigMap");
        assert_eq!(cm["metadata"]["name"], "demo-config");
        let data = &cm["data"];
        assert!(data["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .contains("coordinator=true"));
        assert!(data["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .contains("node-scheduler.include-coordinator=false"));
        assert!(data["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .contains("query.max-memory=3GB"));
        assert!(data["config-worker.properties"]
            .as_str()
            .unwrap()
            .contains("coordinator=false"));
        assert!(data["config-worker.properties"]
            .as_str()
            .unwrap()
            .contains("discovery.uri=http://demo-trino:8080"));
        assert!(data["jvm.config"].as_str().unwrap().contains("-Xmx2G"));
        assert!(data["node.properties"]
            .as_str()
            .unwrap()
            .contains("node.environment="));
    }

    #[test]
    fn single_node_includes_coordinator() {
        let mut s = sample_spec();
        s.worker_replicas = 0;
        let cm = trino_config_map(&s);
        assert!(cm["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .contains("node-scheduler.include-coordinator=true"));
    }

    #[test]
    fn catalogs_config_map_has_one_file_per_catalog() {
        let cm = trino_catalogs_config_map(&sample_spec());
        assert_eq!(cm["metadata"]["name"], "demo-catalogs");
        let f = cm["data"]["egret.properties"].as_str().unwrap();
        assert!(f.contains("iceberg.catalog.type=rest"));
        assert!(f.contains("iceberg.rest-catalog.oauth2.credential=${ENV:EGRET_C0}"));
    }

    #[test]
    fn env_secret_aggregates_catalog_env() {
        let sec = trino_env_secret(&sample_spec());
        assert_eq!(sec["kind"], "Secret");
        assert_eq!(sec["metadata"]["name"], "demo-env");
        assert_eq!(sec["stringData"]["EGRET_C0"], "id:secret");
    }

    #[test]
    fn deployment_coordinator_mounts_config_and_scales() {
        let d = trino_deployment(&sample_spec(), Role::Coordinator);
        assert_eq!(d["kind"], "Deployment");
        assert_eq!(d["metadata"]["name"], "demo-coordinator");
        assert_eq!(d["spec"]["replicas"], 1);
        assert_eq!(
            d["spec"]["template"]["spec"]["containers"][0]["image"],
            "trinodb/trino:470"
        );
        // config checksum annotation present (rolls pods on change)
        assert!(
            d["spec"]["template"]["metadata"]["annotations"]["egret/config-checksum"].is_string()
        );
        let w = trino_deployment(&sample_spec(), Role::Worker);
        assert_eq!(w["metadata"]["name"], "demo-worker");
        assert_eq!(w["spec"]["replicas"], 2);
    }

    #[test]
    fn service_targets_coordinator() {
        let s = trino_service(&sample_spec());
        assert_eq!(s["metadata"]["name"], "demo-trino");
        assert_eq!(s["spec"]["selector"]["app"], "demo-coordinator");
        assert_eq!(s["spec"]["ports"][0]["port"], 8080);
    }

    #[tokio::test]
    async fn provision_applies_all_objects() {
        use crate::kube_client::FakeKube;
        let kube = std::sync::Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        p.provision(&sample_spec()).await.unwrap();
        let a = kube.applied.lock().unwrap();
        for k in [
            "ConfigMap/demo-config",
            "ConfigMap/demo-catalogs",
            "Secret/demo-env",
            "Deployment/demo-coordinator",
            "Deployment/demo-worker",
            "Service/demo-trino",
        ] {
            assert!(a.contains_key(k), "missing {k}");
        }
    }

    #[tokio::test]
    async fn delete_removes_all_objects() {
        use crate::kube_client::FakeKube;
        let kube = std::sync::Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        p.delete("egret-engines", "demo").await.unwrap();
        let d = kube.deleted.lock().unwrap();
        assert_eq!(d.len(), 6); // 2 deployments, service, 2 configmaps, secret
    }
}
