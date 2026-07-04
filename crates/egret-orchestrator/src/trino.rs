//! Trino plain-manifest templating + provisioner. Emits `apps/v1 Deployment`,
//! `v1 Service/ConfigMap/Secret` as `serde_json::Value`; Trino runs from the
//! official `trinodb/trino:<version>` image with config in ConfigMaps.

use crate::kube_client::KubeClient;
use crate::provisioner::EngineProvisioner;
use crate::spec::{
    EngineSpec, EngineStatus, PodStatus, ResourceGroup, ResourceGroups, RoleConfig, SessionRule,
};
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

fn jvm_config(role: &RoleConfig) -> String {
    let mut lines = vec!["-server".to_string()];
    if let Some(sz) = &role.heap.max_size {
        lines.push(format!("-Xmx{sz}"));
        lines.push(format!(
            "-Xms{}",
            role.heap.min_size.clone().unwrap_or_else(|| sz.clone())
        ));
    } else if let Some(pct) = role.heap.max_percent {
        lines.push(format!("-XX:MaxRAMPercentage={pct}"));
        if let Some(ip) = role.heap.initial_percent {
            lines.push(format!("-XX:InitialRAMPercentage={ip}"));
        }
    } else {
        lines.push("-Xmx1G".into());
        lines.push("-Xms1G".into());
    }
    lines.extend([
        "-XX:+UseG1GC".into(),
        "-XX:G1HeapRegionSize=32M".into(),
        "-XX:+ExplicitGCInvokesConcurrent".into(),
        "-XX:+HeapDumpOnOutOfMemoryError".into(),
        "-XX:+ExitOnOutOfMemoryError".into(),
        "-XX:-OmitStackTraceInFastThrow".into(),
        "-Djdk.attach.allowAttachSelf=true".into(),
    ]);
    lines.extend(role.additional_jvm_config.iter().cloned());
    lines.join("\n") + "\n"
}

fn config_properties(spec: &EngineSpec, coord: bool) -> String {
    let cfg = &spec.config;
    let role = if coord {
        &cfg.coordinator
    } else {
        &cfg.worker.base
    };
    let mut out = String::new();
    if coord {
        out.push_str("coordinator=true\n");
        // The coordinator IS the discovery server, so it announces to itself
        // over localhost (robust at startup). Workers reach it via its Service.
        let inc = cfg.worker.base.replicas == 0;
        out.push_str(&format!("node-scheduler.include-coordinator={inc}\n"));
        out.push_str("discovery.uri=http://localhost:8080\n");
    } else {
        out.push_str("coordinator=false\n");
        out.push_str(&format!("discovery.uri=http://{}-trino:8080\n", spec.name));
    }
    out.push_str("http-server.http.port=8080\n");
    if let Some(v) = &cfg.query_max_memory {
        out.push_str(&format!("query.max-memory={v}\n"));
    }
    if let Some(v) = &role.query_max_memory_per_node {
        out.push_str(&format!("query.max-memory-per-node={v}\n"));
    }
    if !coord {
        if let Some(gs) = &cfg.worker.graceful_shutdown {
            if gs.enabled {
                out.push_str(&format!(
                    "shutdown.grace-period={}s\n",
                    gs.grace_period_seconds
                ));
            }
        }
    }
    for (k, v) in &cfg.config_properties {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
}

fn log_properties(spec: &EngineSpec) -> String {
    format!(
        "io.trino={}\n",
        spec.config
            .log_level
            .clone()
            .unwrap_or_else(|| "INFO".into())
    )
}

fn rg_to_json(g: &ResourceGroup) -> Value {
    let mut o = json!({
        "name": g.name, "softMemoryLimit": g.soft_memory_limit,
        "hardConcurrencyLimit": g.hard_concurrency_limit, "maxQueued": g.max_queued,
    });
    if let Some(p) = &g.scheduling_policy {
        o["schedulingPolicy"] = json!(p);
    }
    if !g.sub_groups.is_empty() {
        o["subGroups"] = Value::Array(g.sub_groups.iter().map(rg_to_json).collect());
    }
    o
}

fn resource_groups_json(rg: &ResourceGroups) -> String {
    let selectors: Vec<Value> = rg
        .selectors
        .iter()
        .map(|s| {
            let mut o = json!({ "group": s.group });
            if let Some(v) = &s.user {
                o["user"] = json!(v);
            }
            if let Some(v) = &s.source {
                o["source"] = json!(v);
            }
            if let Some(v) = &s.query_type {
                o["queryType"] = json!(v);
            }
            o
        })
        .collect();
    serde_json::to_string_pretty(&json!({
        "rootGroups": rg.root_groups.iter().map(rg_to_json).collect::<Vec<_>>(),
        "selectors": selectors,
    }))
    .unwrap()
        + "\n"
}

fn session_properties_json(rules: &[SessionRule]) -> String {
    let specs: Vec<Value> = rules
        .iter()
        .map(|r| {
            let mut o = json!({ "sessionProperties": r.properties });
            if let Some(v) = &r.match_user {
                o["user"] = json!(v);
            }
            if let Some(v) = &r.match_source {
                o["source"] = json!(v);
            }
            if !r.match_client_tags.is_empty() {
                o["clientTags"] = json!(r.match_client_tags);
            }
            o
        })
        .collect();
    serde_json::to_string_pretty(&Value::Array(specs)).unwrap() + "\n"
}

pub fn trino_config_map(spec: &EngineSpec) -> Value {
    let mut data = serde_json::Map::new();
    data.insert(
        "config-coordinator.properties".into(),
        json!(config_properties(spec, true)),
    );
    data.insert(
        "config-worker.properties".into(),
        json!(config_properties(spec, false)),
    );
    data.insert(
        "jvm-coordinator.config".into(),
        json!(jvm_config(&spec.config.coordinator)),
    );
    data.insert(
        "jvm-worker.config".into(),
        json!(jvm_config(&spec.config.worker.base)),
    );
    data.insert(
        "node.properties".into(),
        json!("node.environment=egret\nnode.data-dir=/data/trino\n"),
    );
    data.insert("log.properties".into(), json!(log_properties(spec)));

    if let Some(rg) = &spec.config.resource_groups {
        data.insert(
            "resource-groups.json".into(),
            json!(resource_groups_json(rg)),
        );
        data.insert(
            "resource-groups.properties".into(),
            json!(
                "resource-groups.configuration-manager=file\nresource-groups.config-file=/etc/trino/resource-groups/resource-groups.json\n"
            ),
        );
    }
    if !spec.config.session_properties.is_empty() {
        data.insert(
            "session-property-config.json".into(),
            json!(session_properties_json(&spec.config.session_properties)),
        );
        data.insert(
            "session-property-config.properties".into(),
            json!(
                "session-property-config.configuration-manager=file\nsession-property-manager.config-file=/etc/trino/session-property-config.json\n"
            ),
        );
    }

    json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": format!("{}-config", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": Value::Object(data)
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

/// Render a k8s probe as `httpGet /v1/info` on 8080, merging any provided
/// fields over defaults (readiness defaults are more generous than liveness).
fn probe_json(defaults_ready: bool, p: Option<&crate::spec::Probe>) -> Value {
    let (mut init, mut period, mut timeout, mut fail, mut succ) = if defaults_ready {
        (15u32, 10, 5, 12, 1)
    } else {
        (10, 10, 5, 3, 1)
    };
    if let Some(p) = p {
        if let Some(v) = p.initial_delay_seconds {
            init = v;
        }
        if let Some(v) = p.period_seconds {
            period = v;
        }
        if let Some(v) = p.timeout_seconds {
            timeout = v;
        }
        if let Some(v) = p.failure_threshold {
            fail = v;
        }
        if let Some(v) = p.success_threshold {
            succ = v;
        }
    }
    json!({ "httpGet": { "path": "/v1/info", "port": 8080 },
            "initialDelaySeconds": init, "periodSeconds": period, "timeoutSeconds": timeout,
            "failureThreshold": fail, "successThreshold": succ })
}

pub fn trino_deployment(spec: &EngineSpec, role: Role) -> Value {
    let app = format!("{}-{}", spec.name, role.suffix());
    let (rc, jvm_file, cfg_file, replicas) = if role.is_coord() {
        (
            &spec.config.coordinator,
            "jvm-coordinator.config",
            "config-coordinator.properties",
            spec.config.coordinator.replicas,
        )
    } else {
        (
            &spec.config.worker.base,
            "jvm-worker.config",
            "config-worker.properties",
            spec.config.worker.base.replicas,
        )
    };

    // resources: only include a side if any of its values are set.
    let mut requests = serde_json::Map::new();
    let mut limits = serde_json::Map::new();
    if let Some(v) = &rc.resources.cpu_request {
        requests.insert("cpu".into(), json!(v));
    }
    if let Some(v) = &rc.resources.mem_request {
        requests.insert("memory".into(), json!(v));
    }
    if let Some(v) = &rc.resources.cpu_limit {
        limits.insert("cpu".into(), json!(v));
    }
    if let Some(v) = &rc.resources.mem_limit {
        limits.insert("memory".into(), json!(v));
    }
    let mut resources = serde_json::Map::new();
    if !requests.is_empty() {
        resources.insert("requests".into(), Value::Object(requests));
    }
    if !limits.is_empty() {
        resources.insert("limits".into(), Value::Object(limits));
    }

    // probes: readiness always (generous default), liveness/startup only if configured.
    let mut container = json!({
        "name": "trino",
        "image": format!("trinodb/trino:{}", spec.image_version),
        "ports": [{ "containerPort": 8080 }],
        "envFrom": [{ "secretRef": { "name": format!("{}-env", spec.name) } }],
        "resources": Value::Object(resources),
        "readinessProbe": probe_json(true, rc.probes.readiness.as_ref()),
        "volumeMounts": [
            { "name": "config", "mountPath": "/etc/trino/config.properties", "subPath": cfg_file },
            { "name": "config", "mountPath": "/etc/trino/jvm.config", "subPath": jvm_file },
            { "name": "config", "mountPath": "/etc/trino/node.properties", "subPath": "node.properties" },
            { "name": "config", "mountPath": "/etc/trino/log.properties", "subPath": "log.properties" },
            { "name": "catalogs", "mountPath": "/etc/trino/catalog" },
            { "name": "data", "mountPath": "/data/trino" }
        ]
    });
    if let Some(p) = rc.probes.liveness.as_ref() {
        container["livenessProbe"] = probe_json(false, Some(p));
    }
    if let Some(p) = rc.probes.startup.as_ref() {
        container["startupProbe"] = probe_json(false, Some(p));
    }

    // pod-spec: worker graceful shutdown adds a preStop + terminationGracePeriodSeconds.
    let mut pod_spec = json!({
        "containers": [container],
        "volumes": [
            { "name": "config", "configMap": { "name": format!("{}-config", spec.name) } },
            { "name": "catalogs", "configMap": { "name": format!("{}-catalogs", spec.name) } },
            { "name": "data", "emptyDir": {} }
        ]
    });
    if !role.is_coord() {
        if let Some(gs) = &spec.config.worker.graceful_shutdown {
            if gs.enabled {
                let grace = gs.grace_period_seconds;
                let term = std::cmp::max(grace * 2, grace + 30);
                pod_spec["terminationGracePeriodSeconds"] = json!(term);
                let cmd = "curl -s -X PUT -d '\"SHUTTING_DOWN\"' -H 'Content-type: application/json' -H 'X-Trino-User: admin' http://localhost:8080/v1/info/state\ntail --pid=1 -f /dev/null".to_string();
                pod_spec["containers"][0]["lifecycle"] =
                    json!({ "preStop": { "exec": { "command": ["/bin/sh", "-c", cmd] } } });
            }
        }
    }

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
                "spec": pod_spec
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

    fn role(replicas: u32) -> RoleConfig {
        RoleConfig {
            replicas,
            ..Default::default()
        }
    }
    fn sample_spec() -> EngineSpec {
        let mut coord = role(1);
        coord.heap.max_size = Some("2G".into());
        coord.query_max_memory_per_node = Some("1GB".into());
        let mut worker = role(2);
        worker.heap.max_percent = Some(80);
        worker.additional_jvm_config = vec!["-XX:MaxHeapFreeRatio=100".into()];
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
        let mut cat_env = BTreeMap::new();
        cat_env.insert("EGRET_C0".into(), "id:secret".into());
        EngineSpec {
            name: "demo".into(),
            namespace: "egret-engines".into(),
            image_version: "470".into(),
            config: TrinoConfig {
                query_max_memory: Some("4GB".into()),
                log_level: Some("DEBUG".into()),
                config_properties: BTreeMap::new(),
                env: BTreeMap::new(),
                coordinator: coord,
                worker: WorkerRoleConfig {
                    base: worker,
                    graceful_shutdown: Some(GracefulShutdown {
                        enabled: true,
                        grace_period_seconds: 120,
                    }),
                },
                ..Default::default()
            },
            catalogs: vec![CatalogSpec {
                name: "egret".into(),
                properties: props,
                env: cat_env,
            }],
        }
    }

    #[test]
    fn coordinator_jvm_uses_fixed_heap() {
        let j = jvm_config(&sample_spec().config.coordinator);
        assert!(j.contains("-Xmx2G"));
        assert!(!j.contains("MaxRAMPercentage"));
    }
    #[test]
    fn worker_jvm_uses_percent_and_extra_flags() {
        let s = sample_spec();
        let j = jvm_config(&s.config.worker.base);
        assert!(j.contains("-XX:MaxRAMPercentage=80"));
        assert!(!j.contains("-Xmx"));
        assert!(j.contains("-XX:MaxHeapFreeRatio=100"));
    }
    #[test]
    fn config_properties_per_role_memory_and_shutdown() {
        let s = sample_spec();
        let cm = trino_config_map(&s);
        let cc = cm["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap();
        assert!(cc.contains("query.max-memory=4GB"));
        assert!(cc.contains("query.max-memory-per-node=1GB"));
        let wc = cm["data"]["config-worker.properties"].as_str().unwrap();
        assert!(wc.contains("shutdown.grace-period=120s"));
    }
    #[test]
    fn log_properties_carries_level() {
        let cm = trino_config_map(&sample_spec());
        assert!(cm["data"]["log.properties"]
            .as_str()
            .unwrap()
            .contains("io.trino=DEBUG"));
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
        // Coordinator announces to itself; workers reach it via the Service.
        assert!(data["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .contains("discovery.uri=http://localhost:8080"));
        assert!(data["config-worker.properties"]
            .as_str()
            .unwrap()
            .contains("coordinator=false"));
        assert!(data["config-worker.properties"]
            .as_str()
            .unwrap()
            .contains("discovery.uri=http://demo-trino:8080"));
        assert!(data["jvm-coordinator.config"]
            .as_str()
            .unwrap()
            .contains("-Xmx2G"));
        assert!(data["node.properties"]
            .as_str()
            .unwrap()
            .contains("node.environment="));
    }

    #[test]
    fn single_node_includes_coordinator() {
        let mut s = sample_spec();
        s.config.worker.base.replicas = 0;
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

    #[test]
    fn deployment_resources_split_requests_limits() {
        let mut s = sample_spec();
        s.config.coordinator.resources = RoleResources {
            cpu_request: Some("1".into()),
            cpu_limit: Some("2".into()),
            mem_request: Some("2Gi".into()),
            mem_limit: Some("4Gi".into()),
        };
        let d = trino_deployment(&s, Role::Coordinator);
        let res = &d["spec"]["template"]["spec"]["containers"][0]["resources"];
        assert_eq!(res["requests"]["cpu"], "1");
        assert_eq!(res["limits"]["cpu"], "2");
        assert_eq!(res["requests"]["memory"], "2Gi");
        assert_eq!(res["limits"]["memory"], "4Gi");
    }
    #[test]
    fn deployment_renders_custom_probes() {
        let mut s = sample_spec();
        s.config.coordinator.probes.readiness = Some(Probe {
            timeout_seconds: Some(7),
            failure_threshold: Some(20),
            ..Default::default()
        });
        s.config.coordinator.probes.liveness = Some(Probe {
            initial_delay_seconds: Some(60),
            ..Default::default()
        });
        let d = trino_deployment(&s, Role::Coordinator);
        let c = &d["spec"]["template"]["spec"]["containers"][0];
        assert_eq!(c["readinessProbe"]["timeoutSeconds"], 7);
        assert_eq!(c["readinessProbe"]["failureThreshold"], 20);
        assert_eq!(c["livenessProbe"]["httpGet"]["path"], "/v1/info");
        assert_eq!(c["livenessProbe"]["initialDelaySeconds"], 60);
    }
    #[test]
    fn worker_graceful_shutdown_prestop_and_termination() {
        let d = trino_deployment(&sample_spec(), Role::Worker);
        let spec = &d["spec"]["template"]["spec"];
        // termination >= 2 * gracePeriod (120) => 240
        assert_eq!(spec["terminationGracePeriodSeconds"], 240);
        let cmd = spec["containers"][0]["lifecycle"]["preStop"]["exec"]["command"][2]
            .as_str()
            .unwrap();
        assert!(cmd.contains("SHUTTING_DOWN"));
        assert!(cmd.contains("/v1/info/state"));
        assert!(cmd.contains("tail --pid=1"));
    }
    #[test]
    fn coordinator_has_no_prestop() {
        let d = trino_deployment(&sample_spec(), Role::Coordinator);
        assert!(d["spec"]["template"]["spec"]["containers"][0]["lifecycle"].is_null());
    }

    #[test]
    fn resource_groups_json_serializes_tree_and_selectors() {
        let mut s = sample_spec();
        s.config.resource_groups = Some(ResourceGroups {
            root_groups: vec![ResourceGroup {
                name: "global".into(),
                soft_memory_limit: "80%".into(),
                hard_concurrency_limit: 100,
                max_queued: 1000,
                scheduling_policy: None,
                sub_groups: vec![ResourceGroup {
                    name: "etl".into(),
                    soft_memory_limit: "50%".into(),
                    hard_concurrency_limit: 10,
                    max_queued: 100,
                    scheduling_policy: None,
                    sub_groups: vec![],
                }],
            }],
            selectors: vec![ResourceSelector {
                user: Some("admin".into()),
                source: None,
                query_type: None,
                group: "global.etl".into(),
            }],
        });
        let cm = trino_config_map(&s);
        let rg: serde_json::Value =
            serde_json::from_str(cm["data"]["resource-groups.json"].as_str().unwrap()).unwrap();
        assert_eq!(rg["rootGroups"][0]["name"], "global");
        assert_eq!(rg["rootGroups"][0]["hardConcurrencyLimit"], 100);
        assert_eq!(rg["rootGroups"][0]["subGroups"][0]["name"], "etl");
        assert_eq!(rg["selectors"][0]["user"], "admin");
        assert_eq!(rg["selectors"][0]["group"], "global.etl");
        assert!(cm["data"]["resource-groups.properties"]
            .as_str()
            .unwrap()
            .contains("resource-groups.configuration-manager=file"));
    }

    #[test]
    fn session_properties_json_serializes_rules() {
        let mut s = sample_spec();
        let mut props = std::collections::BTreeMap::new();
        props.insert("query_max_memory".to_string(), "10GB".to_string());
        s.config.session_properties = vec![SessionRule {
            match_user: Some("admin".into()),
            match_source: None,
            match_client_tags: vec!["etl".into()],
            properties: props,
        }];
        let cm = trino_config_map(&s);
        let sp: serde_json::Value =
            serde_json::from_str(cm["data"]["session-property-config.json"].as_str().unwrap())
                .unwrap();
        // Trino file session-property manager: a top-level array of match specs.
        assert_eq!(sp[0]["user"], "admin");
        assert_eq!(sp[0]["clientTags"][0], "etl");
        assert_eq!(sp[0]["sessionProperties"]["query_max_memory"], "10GB");
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
