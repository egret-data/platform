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

fn auth_type(auth: &crate::spec::Auth) -> Option<String> {
    let mut parts = vec![];
    if !auth.password_users.is_empty() {
        parts.push("PASSWORD");
    }
    if auth.jwt.is_some() {
        parts.push("JWT");
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(","))
    }
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
    if let Some(v) = &role.heap_headroom_per_node {
        out.push_str(&format!("memory.heap-headroom-per-node={v}\n"));
    }
    if let Some(ft) = &cfg.fault_tolerant {
        out.push_str(&format!("retry-policy={}\n", ft.retry_policy));
    }
    if !cfg.event_listener.is_empty() {
        out.push_str("event-listener.config-files=/etc/trino/event-listener.properties\n");
    }
    for (k, v) in &cfg.config_properties {
        out.push_str(&format!("{k}={v}\n"));
    }
    if coord {
        // egret always reaches the coordinator through a proxy — the ingress for
        // external clients, and the kube API-server proxy / Service for the monitor —
        // which add X-Forwarded-* headers. Trino (Jetty) rejects requests carrying
        // those unless process-forwarded is enabled, so set it unconditionally.
        out.push_str("http-server.process-forwarded=true\n");
        if let Some(auth) = &cfg.auth {
            if let Some(t) = auth_type(auth) {
                out.push_str(&format!("http-server.authentication.type={t}\n"));
                out.push_str("http-server.authentication.allow-insecure-over-http=true\n");
                if let Some(j) = &auth.jwt {
                    out.push_str(&format!(
                        "http-server.authentication.jwt.key-file={}\n",
                        j.jwks_uri
                    ));
                    if let Some(a) = &j.required_audience {
                        out.push_str(&format!(
                            "http-server.authentication.jwt.required-audience={a}\n"
                        ));
                    }
                }
            }
        }
    }
    // Trino REQUIRES a coordinator<->worker shared secret whenever any client
    // authentication is enabled (it refuses to start otherwise). Both roles must
    // carry the same value; egret injects it via the env Secret.
    if cfg.auth.as_ref().and_then(auth_type).is_some() {
        out.push_str("internal-communication.shared-secret=${ENV:TRINO_INTERNAL_SECRET}\n");
    }
    out
}

fn log_properties(spec: &EngineSpec) -> String {
    let mut out = format!(
        "io.trino={}\n",
        spec.config
            .log_level
            .clone()
            .unwrap_or_else(|| "INFO".into())
    );
    for (k, v) in &spec.config.additional_log_properties {
        out.push_str(&format!("{k}={v}\n"));
    }
    out
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
    let mut node = "node.environment=egret\nnode.data-dir=/data/trino\n".to_string();
    for (k, v) in &spec.config.additional_node_properties {
        node.push_str(&format!("{k}={v}\n"));
    }
    data.insert("node.properties".into(), json!(node));
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
    if let Some(ft) = &spec.config.fault_tolerant {
        data.insert(
            "exchange-manager.properties".into(),
            json!(format!(
                "exchange-manager.name=filesystem\nexchange.base-directories={}\nexchange.s3.region={}\nexchange.s3.endpoint={}\nexchange.s3.path-style-access={}\nexchange.s3.aws-access-key=${{ENV:EXCHANGE_S3_ACCESS_KEY}}\nexchange.s3.aws-secret-key=${{ENV:EXCHANGE_S3_SECRET_KEY}}\n",
                ft.s3_base_dir, ft.s3_region, ft.s3_endpoint, ft.s3_path_style
            )),
        );
    }
    if !spec.config.event_listener.is_empty() {
        let body: String = spec
            .config
            .event_listener
            .iter()
            .map(|(k, v)| format!("{k}={v}\n"))
            .collect();
        data.insert("event-listener.properties".into(), json!(body));
    }
    if let Some(auth) = &spec.config.auth {
        if !auth.password_users.is_empty() {
            let db: String = auth
                .password_users
                .iter()
                .map(|u| format!("{}:{}\n", u.username, u.bcrypt_hash))
                .collect();
            data.insert("password.db".into(), json!(db));
            data.insert(
                "password-authenticator.properties".into(),
                json!(
                    "password-authenticator.name=file\nfile.password-file=/etc/trino/auth/password/password.db\n"
                ),
            );
        }
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

/// Render an [`Affinity`] to k8s JSON. A `raw_override` wins verbatim; otherwise
/// assemble node affinity from `node_match` and a preferred pod-anti-affinity
/// (weight 100) selecting on the role's `app` label when a topology key is set.
fn affinity_json(a: &crate::spec::Affinity, app_label: &str) -> Value {
    if let Some(raw) = &a.raw_override {
        return raw.clone();
    }
    let mut out = serde_json::Map::new();
    if !a.node_match.is_empty() {
        let exprs: Vec<Value> = a
            .node_match
            .iter()
            .map(|r| json!({ "key": r.key, "operator": r.operator, "values": r.values }))
            .collect();
        out.insert(
            "nodeAffinity".into(),
            json!({ "requiredDuringSchedulingIgnoredDuringExecution": { "nodeSelectorTerms": [{ "matchExpressions": exprs }] } }),
        );
    }
    if let Some(tk) = &a.pod_anti_affinity_topology_key {
        out.insert(
            "podAntiAffinity".into(),
            json!({ "preferredDuringSchedulingIgnoredDuringExecution": [{
                "weight": 100, "podAffinityTerm": { "topologyKey": tk, "labelSelector": { "matchLabels": { "app": app_label } } } }] }),
        );
    }
    Value::Object(out)
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

    // volumeMounts: base mounts always, plus a subPath mount for each optional
    // config file that was rendered into the config map.
    let mut volume_mounts = vec![
        json!({ "name": "config", "mountPath": "/etc/trino/config.properties", "subPath": cfg_file }),
        json!({ "name": "config", "mountPath": "/etc/trino/jvm.config", "subPath": jvm_file }),
        json!({ "name": "config", "mountPath": "/etc/trino/node.properties", "subPath": "node.properties" }),
        json!({ "name": "config", "mountPath": "/etc/trino/log.properties", "subPath": "log.properties" }),
        json!({ "name": "catalogs", "mountPath": "/etc/trino/catalog" }),
        json!({ "name": "data", "mountPath": "/data/trino" }),
    ];
    if spec.config.resource_groups.is_some() {
        volume_mounts.push(json!({ "name": "config", "mountPath": "/etc/trino/resource-groups.properties", "subPath": "resource-groups.properties" }));
        volume_mounts.push(json!({ "name": "config", "mountPath": "/etc/trino/resource-groups/resource-groups.json", "subPath": "resource-groups.json" }));
    }
    if !spec.config.session_properties.is_empty() {
        volume_mounts.push(json!({ "name": "config", "mountPath": "/etc/trino/session-property-config.properties", "subPath": "session-property-config.properties" }));
        volume_mounts.push(json!({ "name": "config", "mountPath": "/etc/trino/session-property-config.json", "subPath": "session-property-config.json" }));
    }
    if !spec.config.event_listener.is_empty() {
        volume_mounts.push(json!({ "name": "config", "mountPath": "/etc/trino/event-listener.properties", "subPath": "event-listener.properties" }));
    }
    if spec.config.fault_tolerant.is_some() {
        volume_mounts.push(json!({ "name": "config", "mountPath": "/etc/trino/exchange-manager.properties", "subPath": "exchange-manager.properties" }));
    }
    if let Some(auth) = &spec.config.auth {
        if !auth.password_users.is_empty() {
            volume_mounts.push(json!({ "name": "config", "mountPath": "/etc/trino/password-authenticator.properties", "subPath": "password-authenticator.properties" }));
            volume_mounts.push(json!({ "name": "config", "mountPath": "/etc/trino/auth/password/password.db", "subPath": "password.db" }));
        }
    }

    // probes: readiness always (generous default), liveness/startup only if configured.
    let mut container = json!({
        "name": "trino",
        "image": format!("trinodb/trino:{}", spec.image_version),
        "ports": [{ "containerPort": 8080 }],
        "envFrom": [{ "secretRef": { "name": format!("{}-env", spec.name) } }],
        "resources": Value::Object(resources),
        "readinessProbe": probe_json(true, rc.probes.readiness.as_ref()),
        "volumeMounts": Value::Array(volume_mounts)
    });
    if let Some(p) = rc.probes.liveness.as_ref() {
        container["livenessProbe"] = probe_json(false, Some(p));
    }
    if let Some(p) = rc.probes.startup.as_ref() {
        container["startupProbe"] = probe_json(false, Some(p));
    }
    // Per-role additional exposed ports: APPEND to the base http port array.
    if !rc.additional_exposed_ports.is_empty() {
        if !container["ports"].is_array() {
            container["ports"] = json!([]);
        }
        let ports = container["ports"].as_array_mut().unwrap();
        for p in &rc.additional_exposed_ports {
            ports.push(json!({ "name": p.name, "containerPort": p.container_port }));
        }
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

    // Per-role pod placement. Splice each field only when set; `app` is the
    // role's app label used for pod-anti-affinity selection.
    if !rc.node_selector.is_empty() {
        pod_spec["nodeSelector"] = Value::Object(
            rc.node_selector
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect(),
        );
    }
    if !rc.tolerations.is_empty() {
        let tols: Vec<Value> = rc
            .tolerations
            .iter()
            .map(|t| {
                let mut o = serde_json::Map::new();
                if let Some(v) = &t.key {
                    o.insert("key".into(), json!(v));
                }
                if let Some(v) = &t.operator {
                    o.insert("operator".into(), json!(v));
                }
                if let Some(v) = &t.value {
                    o.insert("value".into(), json!(v));
                }
                if let Some(v) = &t.effect {
                    o.insert("effect".into(), json!(v));
                }
                if let Some(v) = &t.toleration_seconds {
                    o.insert("tolerationSeconds".into(), json!(v));
                }
                Value::Object(o)
            })
            .collect();
        pod_spec["tolerations"] = Value::Array(tols);
    }
    if !rc.topology_spread.is_empty() {
        let tsc: Vec<Value> = rc
            .topology_spread
            .iter()
            .map(|t| {
                json!({
                    "maxSkew": t.max_skew,
                    "topologyKey": t.topology_key,
                    "whenUnsatisfiable": t.when_unsatisfiable,
                    "labelSelector": { "matchLabels": t.match_labels },
                })
            })
            .collect();
        pod_spec["topologySpreadConstraints"] = Value::Array(tsc);
    }
    if let Some(a) = &rc.affinity {
        pod_spec["affinity"] = affinity_json(a, &app);
    }
    // Only set the grace period if the worker graceful-shutdown block above did
    // not already set it — GS wins.
    if let Some(tg) = rc.termination_grace_seconds {
        if pod_spec.get("terminationGracePeriodSeconds").is_none() {
            pod_spec["terminationGracePeriodSeconds"] = json!(tg);
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

/// Render a `networking.k8s.io/v1` Ingress exposing the `{name}-trino`
/// coordinator Service. Returns `None` when no ingress is configured or it is
/// disabled. TLS terminates at the ingress; Trino stays HTTP internally.
pub fn trino_ingress(spec: &EngineSpec) -> Option<Value> {
    let ing = spec.config.ingress.as_ref()?;
    if !ing.enabled {
        return None;
    }
    let mut meta = json!({
        "name": format!("{}-trino", spec.name), "namespace": spec.namespace,
        "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name }
    });
    if !ing.annotations.is_empty() {
        meta["annotations"] = Value::Object(
            ing.annotations
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect(),
        );
    }
    let mut spec_obj = json!({
        "rules": [{ "host": ing.host, "http": { "paths": [{
            "path": "/", "pathType": "Prefix",
            "backend": { "service": { "name": format!("{}-trino", spec.name), "port": { "number": 8080 } } }
        }]}}]
    });
    if let Some(cn) = &ing.class_name {
        spec_obj["ingressClassName"] = json!(cn);
    }
    if let Some(ts) = &ing.tls_secret {
        spec_obj["tls"] = json!([{ "hosts": [ing.host], "secretName": ts }]);
    }
    Some(json!({
        "apiVersion": "networking.k8s.io/v1", "kind": "Ingress",
        "metadata": meta, "spec": spec_obj
    }))
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
        if let Some(ing) = trino_ingress(spec) {
            self.kube.apply(&ing).await?;
        }
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
        self.apply_all(spec).await?;
        // `apply_all` only applies the Ingress when enabled, so toggling ingress
        // off would otherwise leave a stale Ingress behind. Best-effort delete it
        // when it is absent or disabled.
        if spec.config.ingress.as_ref().map(|i| i.enabled) != Some(true) {
            let _ = self
                .kube
                .delete(
                    "networking.k8s.io/v1",
                    "Ingress",
                    &spec.namespace,
                    &format!("{}-trino", spec.name),
                )
                .await;
        }
        Ok(())
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
        // Best-effort; harmless when no Ingress was ever applied.
        let _ = self
            .kube
            .delete(
                "networking.k8s.io/v1",
                "Ingress",
                namespace,
                &format!("{name}-trino"),
            )
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
    fn auth_config_password_and_jwt() {
        let mut s = sample_spec();
        s.config.auth = Some(Auth {
            password_users: vec![PasswordEntry {
                username: "bob".into(),
                bcrypt_hash: "$2b$10$abc".into(),
            }],
            jwt: Some(Jwt {
                jwks_uri: "https://kc/realms/egret/protocol/openid-connect/certs".into(),
                required_audience: Some("trino".into()),
            }),
        });
        let cm = trino_config_map(&s);
        let cc = cm["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap();
        assert!(cc.contains("http-server.authentication.type=PASSWORD,JWT"));
        assert!(cc.contains("http-server.authentication.allow-insecure-over-http=true"));
        assert!(cc.contains("http-server.process-forwarded=true"));
        assert!(cc.contains(
            "http-server.authentication.jwt.key-file=https://kc/realms/egret/protocol/openid-connect/certs"
        ));
        assert!(cc.contains("http-server.authentication.jwt.required-audience=trino"));
        // workers don't authenticate clients
        assert!(!cm["data"]["config-worker.properties"]
            .as_str()
            .unwrap()
            .contains("authentication.type"));
        assert!(cm["data"]["password.db"]
            .as_str()
            .unwrap()
            .contains("bob:$2b$10$abc"));
        assert!(cm["data"]["password-authenticator.properties"]
            .as_str()
            .unwrap()
            .contains("password-authenticator.name=file"));
    }
    #[test]
    fn auth_type_password_only() {
        let mut s = sample_spec();
        s.config.auth = Some(Auth {
            password_users: vec![PasswordEntry {
                username: "a".into(),
                bcrypt_hash: "h".into(),
            }],
            jwt: None,
        });
        let cc = trino_config_map(&s)["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(cc.contains("http-server.authentication.type=PASSWORD"));
        assert!(!cc.contains("JWT"));
    }
    #[test]
    fn deployment_mounts_auth_files() {
        // Absent when unset.
        let d = trino_deployment(&sample_spec(), Role::Coordinator);
        let mounts = d["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]
            .as_array()
            .unwrap();
        assert!(!mounts
            .iter()
            .any(|m| m["mountPath"] == "/etc/trino/password-authenticator.properties"));
        assert!(!mounts
            .iter()
            .any(|m| m["mountPath"] == "/etc/trino/auth/password/password.db"));

        // Present when set.
        let mut s = sample_spec();
        s.config.auth = Some(Auth {
            password_users: vec![PasswordEntry {
                username: "bob".into(),
                bcrypt_hash: "$2b$10$abc".into(),
            }],
            jwt: None,
        });
        let d = trino_deployment(&s, Role::Coordinator);
        let mounts = d["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]
            .as_array()
            .unwrap();
        for path in [
            "/etc/trino/password-authenticator.properties",
            "/etc/trino/auth/password/password.db",
        ] {
            assert!(
                mounts.iter().any(|m| m["mountPath"] == path),
                "missing mount {path}"
            );
        }
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

    #[test]
    fn fault_tolerant_renders_retry_and_exchange() {
        let mut s = sample_spec();
        s.config.fault_tolerant = Some(FaultTolerant {
            retry_policy: "TASK".into(),
            s3_base_dir: "s3://spool/exchange".into(),
            s3_endpoint: "http://minio:9000".into(),
            s3_region: "us-east-1".into(),
            s3_path_style: true,
        });
        let cm = trino_config_map(&s);
        assert!(cm["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .contains("retry-policy=TASK"));
        assert!(cm["data"]["config-worker.properties"]
            .as_str()
            .unwrap()
            .contains("retry-policy=TASK"));
        let ex = cm["data"]["exchange-manager.properties"].as_str().unwrap();
        assert!(ex.contains("exchange-manager.name=filesystem"));
        assert!(ex.contains("exchange.base-directories=s3://spool/exchange"));
        assert!(ex.contains("exchange.s3.aws-access-key=${ENV:EXCHANGE_S3_ACCESS_KEY}"));
        assert!(ex.contains("exchange.s3.path-style-access=true"));
    }

    #[test]
    fn heap_headroom_and_raw_props() {
        let mut s = sample_spec();
        s.config.coordinator.heap_headroom_per_node = Some("2GB".into());
        s.config
            .additional_node_properties
            .insert("node.internal-address-source".into(), "FQDN".into());
        s.config
            .additional_log_properties
            .insert("io.trino.execution".into(), "DEBUG".into());
        let cm = trino_config_map(&s);
        assert!(cm["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .contains("memory.heap-headroom-per-node=2GB"));
        assert!(cm["data"]["node.properties"]
            .as_str()
            .unwrap()
            .contains("node.internal-address-source=FQDN"));
        assert!(cm["data"]["log.properties"]
            .as_str()
            .unwrap()
            .contains("io.trino.execution=DEBUG"));
    }

    #[test]
    fn event_listener_properties_rendered() {
        let mut s = sample_spec();
        s.config
            .event_listener
            .insert("event-listener.name".into(), "http".into());
        let cm = trino_config_map(&s);
        assert!(cm["data"]["event-listener.properties"]
            .as_str()
            .unwrap()
            .contains("event-listener.name=http"));
        assert!(cm["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .contains("event-listener.config-files=/etc/trino/event-listener.properties"));
    }

    #[test]
    fn deployment_mounts_optional_files() {
        // Absent when unset.
        let d = trino_deployment(&sample_spec(), Role::Coordinator);
        let mounts = d["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]
            .as_array()
            .unwrap();
        assert!(!mounts
            .iter()
            .any(|m| m["mountPath"] == "/etc/trino/exchange-manager.properties"));
        assert!(!mounts
            .iter()
            .any(|m| m["mountPath"] == "/etc/trino/resource-groups.properties"));

        // Present when set.
        let mut s = sample_spec();
        s.config.fault_tolerant = Some(FaultTolerant {
            retry_policy: "TASK".into(),
            s3_base_dir: "s3://spool/exchange".into(),
            s3_endpoint: "http://minio:9000".into(),
            s3_region: "us-east-1".into(),
            s3_path_style: true,
        });
        s.config.resource_groups = Some(ResourceGroups::default());
        s.config
            .event_listener
            .insert("event-listener.name".into(), "http".into());
        let mut props = std::collections::BTreeMap::new();
        props.insert("query_max_memory".to_string(), "10GB".to_string());
        s.config.session_properties = vec![SessionRule {
            match_user: Some("admin".into()),
            match_source: None,
            match_client_tags: vec![],
            properties: props,
        }];
        let d = trino_deployment(&s, Role::Coordinator);
        let mounts = d["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]
            .as_array()
            .unwrap();
        for path in [
            "/etc/trino/exchange-manager.properties",
            "/etc/trino/resource-groups.properties",
            "/etc/trino/resource-groups/resource-groups.json",
            "/etc/trino/session-property-config.properties",
            "/etc/trino/session-property-config.json",
            "/etc/trino/event-listener.properties",
        ] {
            assert!(
                mounts.iter().any(|m| m["mountPath"] == path),
                "missing mount {path}"
            );
        }
    }

    #[test]
    fn ingress_resource_shape() {
        let mut s = sample_spec();
        let mut anns = std::collections::BTreeMap::new();
        anns.insert(
            "nginx.ingress.kubernetes.io/proxy-read-timeout".into(),
            "300".into(),
        );
        s.config.ingress = Some(crate::spec::Ingress {
            enabled: true,
            class_name: Some("nginx".into()),
            host: "trino-demo.egret.example.com".into(),
            tls_secret: Some("trino-demo-tls".into()),
            annotations: anns,
        });
        let ing = trino_ingress(&s).unwrap();
        assert_eq!(ing["apiVersion"], "networking.k8s.io/v1");
        assert_eq!(ing["kind"], "Ingress");
        assert_eq!(ing["metadata"]["name"], "demo-trino");
        assert_eq!(
            ing["metadata"]["annotations"]["nginx.ingress.kubernetes.io/proxy-read-timeout"],
            "300"
        );
        assert_eq!(ing["spec"]["ingressClassName"], "nginx");
        assert_eq!(
            ing["spec"]["rules"][0]["host"],
            "trino-demo.egret.example.com"
        );
        assert_eq!(
            ing["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"]["name"],
            "demo-trino"
        );
        assert_eq!(
            ing["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"]["port"]["number"],
            8080
        );
        assert_eq!(ing["spec"]["tls"][0]["secretName"], "trino-demo-tls");
    }
    #[test]
    fn ingress_none_when_disabled() {
        let s = sample_spec(); // no ingress
        assert!(trino_ingress(&s).is_none());
    }

    #[test]
    fn per_role_placement_renders() {
        let mut s = sample_spec();
        let c = &mut s.config.coordinator;
        c.node_selector.insert("disktype".into(), "ssd".into());
        c.tolerations = vec![Toleration {
            key: Some("dedicated".into()),
            operator: Some("Equal".into()),
            value: Some("trino".into()),
            effect: Some("NoSchedule".into()),
            toleration_seconds: None,
        }];
        c.topology_spread = vec![TopologySpreadConstraint {
            max_skew: 1,
            topology_key: "kubernetes.io/hostname".into(),
            when_unsatisfiable: "ScheduleAnyway".into(),
            match_labels: [("app".to_string(), "x".to_string())].into(),
        }];
        c.termination_grace_seconds = Some(60);
        c.additional_exposed_ports = vec![ContainerPort {
            name: "jmx".into(),
            container_port: 9080,
        }];
        let d = trino_deployment(&s, Role::Coordinator);
        let ps = &d["spec"]["template"]["spec"];
        assert_eq!(ps["nodeSelector"]["disktype"], "ssd");
        assert_eq!(ps["tolerations"][0]["key"], "dedicated");
        assert_eq!(ps["tolerations"][0]["effect"], "NoSchedule");
        assert_eq!(ps["topologySpreadConstraints"][0]["maxSkew"], 1);
        assert_eq!(ps["terminationGracePeriodSeconds"], 60);
        let ports = &ps["containers"][0]["ports"];
        assert!(ports
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "jmx" && p["containerPort"] == 9080));
    }

    #[test]
    fn affinity_structured_and_raw_override() {
        let mut s = sample_spec();
        s.config.coordinator.affinity = Some(Affinity {
            node_match: vec![NodeSelectorRequirement {
                key: "role".into(),
                operator: "In".into(),
                values: vec!["trino".into()],
            }],
            pod_anti_affinity_topology_key: Some("kubernetes.io/hostname".into()),
            raw_override: None,
        });
        let ps = trino_deployment(&s, Role::Coordinator)["spec"]["template"]["spec"].clone();
        assert_eq!(
            ps["affinity"]["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]
                ["nodeSelectorTerms"][0]["matchExpressions"][0]["key"],
            "role"
        );
        assert_eq!(
            ps["affinity"]["podAntiAffinity"]["preferredDuringSchedulingIgnoredDuringExecution"][0]
                ["podAffinityTerm"]["topologyKey"],
            "kubernetes.io/hostname"
        );
        // raw override wins verbatim
        let mut s2 = sample_spec();
        s2.config.coordinator.affinity = Some(Affinity {
            node_match: vec![],
            pod_anti_affinity_topology_key: None,
            raw_override: Some(serde_json::json!({"custom": true})),
        });
        assert_eq!(
            trino_deployment(&s2, Role::Coordinator)["spec"]["template"]["spec"]["affinity"]
                ["custom"],
            true
        );
    }

    #[tokio::test]
    async fn provision_applies_ingress_and_delete_removes_it() {
        use crate::kube_client::FakeKube;
        let kube = std::sync::Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        let mut s = sample_spec();
        s.config.ingress = Some(crate::spec::Ingress {
            enabled: true,
            class_name: Some("nginx".into()),
            host: "trino-demo.egret.example.com".into(),
            tls_secret: None,
            annotations: std::collections::BTreeMap::new(),
        });
        p.provision(&s).await.unwrap();
        assert!(kube
            .applied
            .lock()
            .unwrap()
            .contains_key("Ingress/demo-trino"));

        p.delete(&s.namespace, &s.name).await.unwrap();
        assert!(kube
            .deleted
            .lock()
            .unwrap()
            .contains(&"Ingress/demo-trino".to_string()));
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
        assert_eq!(d.len(), 7); // 2 deployments, service, 2 configmaps, secret, ingress
    }
}
