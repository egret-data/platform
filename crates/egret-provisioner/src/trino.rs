//! Trino plain-manifest templating + provisioner. Emits `apps/v1 Deployment`,
//! `v1 Service/ConfigMap/Secret` as `serde_json::Value`; Trino runs from the
//! official `trinodb/trino:<version>` image with config in ConfigMaps.

use crate::kube_client::KubeClient;
use crate::provisioner::EngineProvisioner;
use crate::spec::{
    EngineSpec, EngineStatus, KedaTrigger, PodStatus, RangerRollout, ResourceGroup, ResourceGroups,
    RoleConfig, SessionRule,
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
        // access-control.properties is always rendered (ranger when bound, else
        // allow-all), so always point Trino at it — otherwise it falls back to the
        // impersonation-denying default and delegated queries fail.
        out.push_str("access-control.config-files=/etc/trino/access-control.properties\n");
        if let Some(auth) = &cfg.auth {
            if let Some(t) = auth_type(auth) {
                out.push_str(&format!("http-server.authentication.type={t}\n"));
                // jwt-mode (TLS + JWT-only) binds query identity to the TLS-validated
                // JWT — reject auth over plain HTTP:8080 so a spoofed X-Trino-User can't
                // run as another user. impersonate engines keep insecure-over-http for
                // their basic-auth monitor; safe because impersonation is delegator-only.
                let insecure_over_http = !(cfg.tls.is_some() && t == "JWT");
                out.push_str(&format!(
                    "http-server.authentication.allow-insecure-over-http={insecure_over_http}\n"
                ));
                if let Some(j) = &auth.jwt {
                    out.push_str(&format!(
                        "http-server.authentication.jwt.key-file={}\n",
                        j.jwks_uri
                    ));
                    // Authenticate the caller as their username, not the raw OIDC
                    // subject. The engine JWT carries `preferred_username` (= egret's
                    // derived username), and the delegated query path sends that same
                    // value as `X-Trino-User` — so principal == user, a self-
                    // impersonation. Ranger does NOT allow that by default: Trino 483's
                    // plugin checks `impersonate` on `trinouser=<user>` on every
                    // session, principal == user included (live result
                    // `docs/en/plans/2026-10-13-view-governance-live-result.md` §4.8,
                    // `Principal u cannot become user u`). egret seeds the allow as the
                    // `egret-user-impersonate-self` infrastructure policy
                    // (`egret_engine_mgmt::delegator_policy`). Leaving the Trino default
                    // (`sub`) makes principal (a UUID) != user, which that self-only
                    // policy does not cover. Mirrors the StarRocks `authentication_jwt`
                    // principal handling.
                    out.push_str(
                        "http-server.authentication.jwt.principal-field=preferred_username\n",
                    );
                    if let Some(a) = &j.required_audience {
                        out.push_str(&format!(
                            "http-server.authentication.jwt.required-audience={a}\n"
                        ));
                    }
                }
            }
        }
        // jwt-mode (and impersonate-over-TLS) engines serve HTTPS on the coordinator
        // with an egret-generated keystore mounted from the `{name}-tls` Secret. The
        // internal http listener on 8080 (used for loopback discovery + workers) is
        // left untouched.
        if let Some(tls) = &cfg.tls {
            out.push_str("http-server.https.enabled=true\n");
            out.push_str(&format!("http-server.https.port={}\n", tls.port));
            out.push_str("http-server.https.keystore.path=/etc/trino/tls/keystore.pem\n");
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

/// Ranger loggers a bound engine holds at a level egret sets, whatever the
/// user's log properties say. `AuditProviderFactory` logs every
/// `ranger.plugin.trino.*` property at INFO on start (`AUDIT PROPERTY: …`),
/// `policy.rest.client.password` included, so at INFO the engine's `/ranger`
/// credential is in every coordinator's and worker's stdout (live result
/// `docs/en/plans/2026-10-20-ranger-surface-auth-live-result.md`, finding 1).
/// That run's secret sweep found the password on this line only, and
/// `RangerRESTClient` (2.8.0 `ranger-plugins-common`) reads it without
/// logging it.
const TRINO_RANGER_FORCED_LOG_LEVELS: &[(&str, &str)] = &[(
    "org.apache.ranger.audit.provider.AuditProviderFactory",
    "WARN",
)];

fn log_properties(spec: &EngineSpec) -> String {
    let mut out = String::new();
    // Legacy structured log level → `io.trino=<level>`. New engines carry the
    // level as an `io.trino` entry in additionalLogProperties instead, so only
    // emit the structured line when set (Trino defaults the root logger to INFO
    // when absent) — avoiding a duplicate `io.trino` line.
    if let Some(level) = &spec.config.log_level {
        out.push_str(&format!("io.trino={level}\n"));
    }
    let forced: &[(&str, &str)] = if spec.ranger.is_some() {
        TRINO_RANGER_FORCED_LOG_LEVELS
    } else {
        &[]
    };
    for (k, v) in &spec.config.additional_log_properties {
        // The user's entry for a forced logger is dropped rather than left to
        // be overridden, so the file states one level for it.
        if forced.iter().any(|(logger, _)| k.trim() == *logger) {
            continue;
        }
        out.push_str(&format!("{k}={v}\n"));
    }
    // Last, so they win even over a user value that smuggles in a line of its
    // own (a later key in a properties file replaces an earlier one).
    for (logger, level) in forced {
        out.push_str(&format!("{logger}={level}\n"));
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

    if let Some(r) = &spec.ranger {
        // The access-control selector only. The security XML carries the
        // plugin's credential, so it lives in `{name}-rc-{tag}`
        // ([`trino_ranger_secret`]), never in this ConfigMap.
        let rendered = crate::ranger_config::ranger_plugin_files(
            egret_engine_mgmt_api::EngineKind::Trino,
            &r.service_name,
            &r.egret_url,
            r.poll_interval_ms.into(),
            None,
        );
        for (name, content) in rendered.files {
            if name != TRINO_RANGER_SECURITY_XML {
                data.insert(name, json!(content));
            }
        }
    } else {
        // No Ranger binding (Ranger disabled): pin Trino's built-in ALLOW-ALL
        // access control EXPLICITLY. Omitting access-control.properties falls back
        // to Trino's default system access control, which DENIES impersonation —
        // so egret's per-user delegation (the `egret_delegator` service account
        // impersonating the calling user via X-Trino-User) fails with
        // "cannot impersonate user". allow-all permits it. This engine is then
        // ungoverned at the data plane; enable Ranger for policy enforcement.
        data.insert(
            "access-control.properties".into(),
            json!("access-control.name=allow-all\n"),
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

/// The coordinator's HTTPS keystore Secret (`{name}-tls`), rendered only when the
/// engine is TLS-enabled. `keystore.pem` holds the cert+key PEM egret generated.
pub fn trino_tls_secret(spec: &EngineSpec) -> Option<Value> {
    let tls = spec.config.tls.as_ref()?;
    Some(json!({
        "apiVersion": "v1", "kind": "Secret",
        "metadata": { "name": format!("{}-tls", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": { "keystore.pem": tls.keystore_pem }
    }))
}

/// The Ranger plugin's security XML: the file name, the Secret key and the
/// `subPath` it is mounted by.
const TRINO_RANGER_SECURITY_XML: &str = crate::ranger_config::TRINO_SECURITY_XML;

/// The `{name}-rc-{tag}` Secret ([`crate::rollout::ranger_secret_name`]):
/// the Ranger plugin's security XML, which carries the engine's `/ranger`
/// credential (spec 2026-10-20-ranger-surface-auth A9). One per credential, so
/// a pod of a template that names a credential can only mount that one's
/// content. `None` unless the engine is bound to a profile. Mounted by
/// `subPath` into both roles, so a change reaches a pod only when it is
/// replaced: [`config_checksum`] hashes this Secret for exactly that reason.
pub fn trino_ranger_secret(spec: &EngineSpec) -> Option<Value> {
    let r = spec.ranger.as_ref()?;
    let rendered = crate::ranger_config::ranger_plugin_files(
        egret_engine_mgmt_api::EngineKind::Trino,
        &r.service_name,
        &r.egret_url,
        r.poll_interval_ms.into(),
        r.credential_ref(),
    );
    let xml = rendered
        .get(TRINO_RANGER_SECURITY_XML)
        .expect("the renderer emits it");
    let mut secret = json!({
        "apiVersion": "v1", "kind": "Secret", "type": "Opaque",
        "metadata": { "name": crate::rollout::spec_ranger_secret_name(spec), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name,
                                  "egret/component": crate::rollout::RANGER_SECRET_COMPONENT } },
        "stringData": { TRINO_RANGER_SECURITY_XML: xml }
    });
    // Which credential this Secret now holds (its non-secret id), so a rollout
    // probe can tell a later write of an OLDER credential (`crate::rollout`).
    if let Some(id) = crate::rollout::ranger_credential_id(spec) {
        secret["metadata"]["annotations"] =
            json!({ crate::rollout::RANGER_CREDENTIAL_ANNOTATION: id });
    }
    Some(secret)
}

/// Project the resolved dockerconfigjson value (populated at provision from a
/// typed `kubernetes.io/dockerconfigjson` secret) into a k8s Secret named
/// `{name}-pull`, referenced from the pod's `imagePullSecrets`.
pub fn trino_pull_secret(spec: &EngineSpec) -> Option<Value> {
    let json = spec.config.image_pull_secret_json.as_ref()?;
    Some(json!({
        "apiVersion": "v1", "kind": "Secret", "type": "kubernetes.io/dockerconfigjson",
        "metadata": { "name": format!("{}-pull", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": { ".dockerconfigjson": json }
    }))
}

/// Project the resolved ingress TLS cert/key into a real k8s
/// `type: kubernetes.io/tls` Secret named `{name}-ingress-tls`. Returns `None`
/// unless the ingress is enabled and carries resolved cert+key PEMs (populated
/// at provision from a typed `kubernetes.io/tls` secret ref).
pub fn trino_ingress_tls_secret(spec: &EngineSpec) -> Option<Value> {
    crate::ingress::ingress_tls_secret(&spec.name, &spec.namespace, spec.config.ingress.as_ref()?)
}

fn config_checksum(spec: &EngineSpec) -> String {
    // Rolls the pods whenever rendered config or catalogs change.
    let mut h = DefaultHasher::new();
    trino_config_map(spec).to_string().hash(&mut h);
    trino_catalogs_config_map(spec).to_string().hash(&mut h);
    trino_env_secret(spec).to_string().hash(&mut h);
    // The Ranger security XML (and so its credential): a rotation changes only
    // this Secret, and its `subPath` mount is never refreshed in a running pod.
    // `None` (unbound) hashes differently from any bound XML.
    trino_ranger_secret(spec)
        .map(|s| s.to_string())
        .unwrap_or_default()
        .hash(&mut h);
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

/// Render a [`SecurityContext`] to k8s JSON: the flat `Some` fields first, then
/// any `raw_override` keys merged verbatim ON TOP (so an override wins).
fn security_context_json(sc: &crate::spec::SecurityContext) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(v) = sc.run_as_user {
        out.insert("runAsUser".into(), json!(v));
    }
    if let Some(v) = sc.run_as_group {
        out.insert("runAsGroup".into(), json!(v));
    }
    if let Some(v) = sc.fs_group {
        out.insert("fsGroup".into(), json!(v));
    }
    if let Some(v) = sc.run_as_non_root {
        out.insert("runAsNonRoot".into(), json!(v));
    }
    if let Some(raw) = &sc.raw_override {
        if let Some(obj) = raw.as_object() {
            for (k, v) in obj {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// Render a [`MiniContainer`] to k8s JSON. A `raw_override` wins verbatim;
/// otherwise build `{name, image}` plus optional `command`/`args`/`env`/
/// `volumeMounts`, omitting any empty collection.
fn container_json(c: &crate::spec::MiniContainer) -> Value {
    if let Some(raw) = &c.raw_override {
        return raw.clone();
    }
    let mut out = serde_json::Map::new();
    out.insert("name".into(), json!(c.name));
    out.insert("image".into(), json!(c.image));
    if !c.command.is_empty() {
        out.insert("command".into(), json!(c.command));
    }
    if !c.args.is_empty() {
        out.insert("args".into(), json!(c.args));
    }
    if !c.env.is_empty() {
        let env: Vec<Value> = c
            .env
            .iter()
            .map(|(k, v)| json!({ "name": k, "value": v }))
            .collect();
        out.insert("env".into(), Value::Array(env));
    }
    if !c.volume_mounts.is_empty() {
        let vms: Vec<Value> = c
            .volume_mounts
            .iter()
            .map(|m| {
                let mut o = serde_json::Map::new();
                o.insert("name".into(), json!(m.name));
                o.insert("mountPath".into(), json!(m.mount_path));
                if let Some(sp) = &m.sub_path {
                    o.insert("subPath".into(), json!(sp));
                }
                Value::Object(o)
            })
            .collect();
        out.insert("volumeMounts".into(), Value::Array(vms));
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
    // access-control.properties is ALWAYS rendered (ranger when bound, else
    // allow-all — see the config map builder), so ALWAYS mount it; without the
    // mount Trino falls back to its impersonation-denying default.
    volume_mounts.push(json!({ "name": "config", "mountPath": "/etc/trino/access-control.properties", "subPath": "access-control.properties" }));
    if spec.ranger.is_some() {
        volume_mounts.push(json!({ "name": "ranger-secret", "mountPath": "/etc/trino/ranger-trino-security.xml", "subPath": TRINO_RANGER_SECURITY_XML }));
    }
    // Only the coordinator serves HTTPS, so only it mounts the keystore Secret.
    if role.is_coord() && spec.config.tls.is_some() {
        volume_mounts.push(json!({ "name": "tls", "mountPath": "/etc/trino/tls/keystore.pem", "subPath": "keystore.pem" }));
    }

    // probes: readiness always (generous default), liveness/startup only if configured.
    let mut container = json!({
        "name": "trino",
        "image": spec
            .config
            .image
            .clone()
            .unwrap_or_else(|| format!("trinodb/trino:{}", spec.image_version)),
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
    // The coordinator serves HTTPS on 8443 when TLS is enabled; delegated
    // in-cluster connections target it. Name both ports (k8s requires names once
    // a container declares more than one). Runs before the additional-exposed-ports
    // append below, which just pushes onto the same array.
    if role.is_coord() && spec.config.tls.is_some() {
        let ports = container["ports"].as_array_mut().unwrap();
        ports[0] = json!({ "name": "http", "containerPort": 8080 });
        ports.push(json!({ "name": "https", "containerPort": 8443 }));
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

    // Cluster-wide container fields (from spec.config, applied to both roles).
    if let Some(p) = &spec.config.image_pull_policy {
        container["imagePullPolicy"] = json!(p);
    }
    if let Some(sc) = &spec.config.container_security_context {
        container["securityContext"] = security_context_json(sc);
    }
    // Cluster-wide extra env sources: prepend user ConfigMap/Secret refs ahead of
    // the base `-env` secret (kept last so the container still loads it).
    if !spec.config.env_from.is_empty() {
        let mut sources: Vec<Value> = spec
            .config
            .env_from
            .iter()
            .flat_map(|ef| {
                let mut v = Vec::new();
                if let Some(cm) = &ef.config_map_ref {
                    v.push(json!({ "configMapRef": { "name": cm } }));
                }
                if let Some(sec) = &ef.secret_ref {
                    v.push(json!({ "secretRef": { "name": sec } }));
                }
                v
            })
            .collect();
        if let Some(existing) = container["envFrom"].as_array() {
            sources.extend(existing.iter().cloned());
        }
        container["envFrom"] = Value::Array(sources);
    }
    // Per-role config/secret file mounts: a volumeMount per entry on the main
    // container (the matching volume is added to the pod spec below).
    for m in rc.config_mounts.iter().chain(&rc.secret_mounts) {
        let mut o = serde_json::Map::new();
        o.insert("name".into(), json!(m.name));
        o.insert("mountPath".into(), json!(m.mount_path));
        if let Some(sp) = &m.sub_path {
            o.insert("subPath".into(), json!(sp));
        }
        container["volumeMounts"]
            .as_array_mut()
            .unwrap()
            .push(Value::Object(o));
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
    // Both roles run the Ranger plugin, so both mount its security XML from the
    // credential's own Secret (`crate::rollout::ranger_secret_name`).
    if spec.ranger.is_some() {
        pod_spec["volumes"].as_array_mut().unwrap().push(json!({
            "name": "ranger-secret",
            "secret": { "secretName": crate::rollout::spec_ranger_secret_name(spec) }
        }));
    }
    // Coordinator-only HTTPS keystore volume, sourced from the `{name}-tls` Secret.
    if role.is_coord() && spec.config.tls.is_some() {
        pod_spec["volumes"].as_array_mut().unwrap().push(
            json!({ "name": "tls", "secret": { "secretName": format!("{}-tls", spec.name) } }),
        );
    }
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

    // Cluster-wide pod fields (from spec.config, applied to both roles).
    if let Some(sa) = &spec.config.service_account {
        pod_spec["serviceAccountName"] = json!(sa);
    }
    if !spec.config.image_pull_secrets.is_empty() {
        pod_spec["imagePullSecrets"] = Value::Array(
            spec.config
                .image_pull_secrets
                .iter()
                .map(|s| json!({ "name": s }))
                .collect(),
        );
    }
    if let Some(b) = rc.share_process_namespace {
        pod_spec["shareProcessNamespace"] = json!(b);
    }
    if let Some(sc) = &spec.config.pod_security_context {
        pod_spec["securityContext"] = security_context_json(sc);
    }

    // Per-role init/sidecar containers + config/secret volumes.
    if !rc.init_containers.is_empty() {
        pod_spec["initContainers"] =
            Value::Array(rc.init_containers.iter().map(container_json).collect());
    }
    for sc in &rc.sidecar_containers {
        pod_spec["containers"]
            .as_array_mut()
            .unwrap()
            .push(container_json(sc));
    }
    for m in &rc.config_mounts {
        let mut inner = serde_json::Map::new();
        inner.insert("name".into(), json!(m.name));
        if let Some(dm) = m.default_mode {
            inner.insert("defaultMode".into(), json!(dm));
        }
        pod_spec["volumes"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": m.name, "configMap": Value::Object(inner) }));
    }
    for m in &rc.secret_mounts {
        let mut inner = serde_json::Map::new();
        inner.insert("secretName".into(), json!(m.name));
        if let Some(dm) = m.default_mode {
            inner.insert("defaultMode".into(), json!(dm));
        }
        pod_spec["volumes"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "name": m.name, "secret": Value::Object(inner) }));
    }

    let mut deployment = json!({
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
    });

    // Both roles run the Ranger plugin: name the credential their pods are
    // rendered with (its non-secret id), so the rollout of THAT credential can
    // be observed before the previous one is retired (`crate::rollout`).
    if let Some(id) = crate::rollout::ranger_credential_id(spec) {
        deployment["spec"]["template"]["metadata"]["annotations"]
            [crate::rollout::RANGER_CREDENTIAL_ANNOTATION] = json!(id);
    }

    // Cluster-wide Deployment-level fields (from spec.config).
    if let Some(strat) = &spec.config.deployment_strategy {
        let mut s = json!({ "type": strat.kind });
        if strat.kind == "RollingUpdate"
            && (strat.max_surge.is_some() || strat.max_unavailable.is_some())
        {
            let mut ru = serde_json::Map::new();
            if let Some(v) = &strat.max_surge {
                ru.insert("maxSurge".into(), json!(v));
            }
            if let Some(v) = &strat.max_unavailable {
                ru.insert("maxUnavailable".into(), json!(v));
            }
            s["rollingUpdate"] = Value::Object(ru);
        }
        deployment["spec"]["strategy"] = s;
    }
    if let Some(v) = spec.config.revision_history_limit {
        deployment["spec"]["revisionHistoryLimit"] = json!(v);
    }
    if let Some(v) = spec.config.progress_deadline_seconds {
        deployment["spec"]["progressDeadlineSeconds"] = json!(v);
    }
    // Merge deployment_annotations onto the OUTER Deployment metadata (distinct
    // from the pod-template's config-checksum annotation map).
    if !spec.config.deployment_annotations.is_empty() {
        if !deployment["metadata"]["annotations"].is_object() {
            deployment["metadata"]["annotations"] = json!({});
        }
        let anns = deployment["metadata"]["annotations"]
            .as_object_mut()
            .unwrap();
        for (k, v) in &spec.config.deployment_annotations {
            anns.insert(k.clone(), json!(v));
        }
    }

    // When autoscaling owns the worker, omit `spec.replicas` so the HPA /
    // ScaledObject is the sole authority. The coordinator is untouched.
    if !role.is_coord() && spec.config.autoscaling.is_some() {
        deployment["spec"]
            .as_object_mut()
            .unwrap()
            .remove("replicas");
    }

    deployment
}

pub fn trino_service(spec: &EngineSpec) -> Value {
    let mut ports = vec![json!({ "port": 8080, "targetPort": 8080, "name": "http" })];
    // Expose the coordinator's HTTPS listener so delegated in-cluster clients can
    // reach 8443. Only present when TLS is enabled; otherwise the Service is 8080-only.
    if spec.config.tls.is_some() {
        ports.push(json!({ "port": 8443, "targetPort": 8443, "name": "https" }));
    }
    json!({
        "apiVersion": "v1", "kind": "Service",
        "metadata": { "name": format!("{}-trino", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "spec": { "selector": { "app": format!("{}-coordinator", spec.name) },
                  "ports": ports }
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

/// Render an `autoscaling/v2 HorizontalPodAutoscaler` targeting the
/// `{name}-worker` Deployment. Returns `None` unless autoscaling is configured
/// with `mode == "hpa"`. `minReplicas` = the configured worker replicas.
pub fn trino_hpa(spec: &EngineSpec) -> Option<Value> {
    let auto = spec.config.autoscaling.as_ref()?;
    if auto.mode != "hpa" {
        return None;
    }
    let hpa = auto.hpa.as_ref()?;
    let worker = format!("{}-worker", spec.name);
    let mut metrics = Vec::new();
    if let Some(pct) = hpa.target_cpu_percent {
        metrics.push(json!({
            "type": "Resource",
            "resource": { "name": "cpu", "target": { "type": "Utilization", "averageUtilization": pct } }
        }));
    }
    if let Some(pct) = hpa.target_memory_percent {
        metrics.push(json!({
            "type": "Resource",
            "resource": { "name": "memory", "target": { "type": "Utilization", "averageUtilization": pct } }
        }));
    }
    let mut hpa_spec = json!({
        "scaleTargetRef": { "apiVersion": "apps/v1", "kind": "Deployment", "name": worker },
        "minReplicas": spec.config.worker.base.replicas,
        "maxReplicas": hpa.max_replicas,
        "metrics": metrics
    });
    if let Some(behavior) = &hpa.behavior_raw {
        hpa_spec["behavior"] = behavior.clone();
    }
    Some(json!({
        "apiVersion": "autoscaling/v2", "kind": "HorizontalPodAutoscaler",
        "metadata": { "name": format!("{}-worker", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "spec": hpa_spec
    }))
}

/// Render one KEDA ScaledObject trigger. `raw` triggers pass through verbatim;
/// structured `cron`/`prometheus` triggers render KEDA metadata as STRINGS.
fn keda_trigger_json(t: &KedaTrigger) -> Value {
    match t.kind.as_str() {
        "raw" => t.raw_override.clone().unwrap_or_else(|| json!({})),
        "cron" => {
            let c = t.cron.clone().unwrap_or_default();
            json!({
                "type": "cron",
                "metadata": {
                    "start": c.start,
                    "end": c.end,
                    "desiredReplicas": c.desired_replicas.to_string(),
                    "timezone": c.timezone
                }
            })
        }
        "prometheus" => {
            let p = t.prometheus.clone().unwrap_or_default();
            json!({
                "type": "prometheus",
                "metadata": {
                    "serverAddress": p.server_address,
                    "metricName": p.metric_name,
                    "query": p.query,
                    "threshold": p.threshold
                }
            })
        }
        _ => json!({}),
    }
}

/// Render a `keda.sh/v1alpha1 ScaledObject` targeting the `{name}-worker`
/// Deployment. Returns `None` unless autoscaling is configured with
/// `mode == "keda"`.
pub fn trino_scaled_object(spec: &EngineSpec) -> Option<Value> {
    let auto = spec.config.autoscaling.as_ref()?;
    if auto.mode != "keda" {
        return None;
    }
    let keda = auto.keda.as_ref()?;
    let triggers: Vec<Value> = keda.triggers.iter().map(keda_trigger_json).collect();
    let mut so_spec = json!({
        "scaleTargetRef": { "name": format!("{}-worker", spec.name) },
        "minReplicaCount": keda.min_replica_count,
        "maxReplicaCount": keda.max_replica_count,
        "triggers": triggers
    });
    if let Some(v) = keda.polling_interval {
        so_spec["pollingInterval"] = json!(v);
    }
    if let Some(v) = keda.cooldown_period {
        so_spec["cooldownPeriod"] = json!(v);
    }
    if let Some(fb) = &keda.fallback_raw {
        so_spec["fallback"] = fb.clone();
    }
    Some(json!({
        "apiVersion": "keda.sh/v1alpha1", "kind": "ScaledObject",
        "metadata": { "name": format!("{}-worker", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "spec": so_spec
    }))
}

/// Provisions a Trino engine as plain Kubernetes manifests: two Deployments
/// (coordinator + worker), a Service, two ConfigMaps (config + catalogs), an
/// env Secret and, when bound to a profile, the Ranger credential Secret. A checksum annotation on the pod template rolls pods on change.
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
        if let Some(tls) = trino_tls_secret(spec) {
            self.kube.apply(&tls).await?;
        }
        // Before the Deployments that mount it: a pod scheduled first would
        // fail to start (or, on an update, start with the old credential).
        if let Some(ranger) = trino_ranger_secret(spec) {
            self.kube.apply(&ranger).await?;
        }
        // Apply the image pull Secret before the Deployments so it exists when
        // the pods schedule and pull the image.
        if let Some(ps) = trino_pull_secret(spec) {
            self.kube.apply(&ps).await?;
        }
        self.kube.apply(&trino_service(spec)).await?;
        self.kube
            .apply(&trino_deployment(spec, Role::Coordinator))
            .await?;
        self.kube
            .apply(&trino_deployment(spec, Role::Worker))
            .await?;
        if let Some(isec) = trino_ingress_tls_secret(spec) {
            self.kube.apply(&isec).await?;
        }
        if let Some(ing) = trino_ingress(spec) {
            self.kube.apply(&ing).await?;
        }
        // Autoscaling owns the worker Deployment's replica count. Each renderer
        // returns Some only for its matching mode, so at most one is applied.
        if let Some(h) = trino_hpa(spec) {
            self.kube.apply(&h).await?;
        }
        if let Some(so) = trino_scaled_object(spec) {
            self.kube.apply(&so).await?;
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
    async fn delete_ranger_secret(
        &self,
        namespace: &str,
        name: &str,
        credential_id: &str,
    ) -> anyhow::Result<()> {
        crate::rollout::delete_retired_ranger_secret(&*self.kube, namespace, name, credential_id)
            .await
    }
    async fn ranger_rollout(
        &self,
        namespace: &str,
        name: &str,
        credential_id: &str,
    ) -> anyhow::Result<RangerRollout> {
        use crate::rollout::Workload;
        crate::rollout::observe(
            &*self.kube,
            namespace,
            name,
            &[
                Workload::Deployment(format!("{name}-coordinator")),
                Workload::Deployment(format!("{name}-worker")),
            ],
            &format!("app.kubernetes.io/instance={name}"),
            credential_id,
        )
        .await
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
            // Also clean up the stale ingress TLS Secret (managed by egret).
            let _ = self
                .kube
                .delete(
                    "v1",
                    "Secret",
                    &spec.namespace,
                    &format!("{}-ingress-tls", spec.name),
                )
                .await;
        }
        // Unbinding the profile drops the mount; the Secrets egret rendered
        // for it would otherwise linger. Best-effort, like the ingress TLS above.
        if spec.ranger.is_none() {
            crate::rollout::delete_all_ranger_secrets_best_effort(
                &*self.kube,
                &spec.namespace,
                &spec.name,
            )
            .await;
        }
        // Likewise, `apply_all` only applies the autoscaler for the current mode.
        // Best-effort delete the stale object when autoscaling is off or the mode
        // switched, so HPA↔KEDA↔off never leaves an orphaned scaler behind.
        let worker = format!("{}-worker", spec.name);
        let hpa = || {
            self.kube.delete(
                "autoscaling/v2",
                "HorizontalPodAutoscaler",
                &spec.namespace,
                &worker,
            )
        };
        let so = || {
            self.kube
                .delete("keda.sh/v1alpha1", "ScaledObject", &spec.namespace, &worker)
        };
        match spec.config.autoscaling.as_ref().map(|a| a.mode.as_str()) {
            Some("hpa") => {
                let _ = so().await;
            }
            Some("keda") => {
                let _ = hpa().await;
            }
            _ => {
                let _ = hpa().await;
                let _ = so().await;
            }
        }
        Ok(())
    }
    async fn delete(&self, namespace: &str, name: &str) -> anyhow::Result<()> {
        // Every object egret applies for a Trino engine, including the ones only
        // some engines have (TLS/ingress/pull secrets, Ingress, both autoscalers):
        // an object that was never applied is a 404, which `KubeClient::delete`
        // treats as success. Anything else is an error the caller must see.
        let objects = [
            ("apps/v1", "Deployment", format!("{name}-coordinator")),
            ("apps/v1", "Deployment", format!("{name}-worker")),
            ("v1", "Service", format!("{name}-trino")),
            ("v1", "ConfigMap", format!("{name}-config")),
            ("v1", "ConfigMap", format!("{name}-catalogs")),
            ("v1", "Secret", format!("{name}-env")),
            ("v1", "Secret", format!("{name}-tls")),
            ("v1", "Secret", format!("{name}-ingress-tls")),
            ("v1", "Secret", format!("{name}-pull")),
            ("v1", "Secret", format!("{name}-ranger-secret")),
            ("networking.k8s.io/v1", "Ingress", format!("{name}-trino")),
            (
                "autoscaling/v2",
                "HorizontalPodAutoscaler",
                format!("{name}-worker"),
            ),
            ("keda.sh/v1alpha1", "ScaledObject", format!("{name}-worker")),
        ];
        crate::rollout::delete_with_ranger_secrets(&*self.kube, namespace, name, &objects).await
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
            image_version: "483".into(),
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
            ranger: None,
            kind: EngineKind::Trino,
            starrocks: None,
        }
    }

    const TEST_CREDENTIAL_ID: &str = "0b5ad7e2-0000-4000-8000-00000000c0de";

    fn bound(credential: Option<(&str, &str)>) -> RangerBinding {
        bound_as(TEST_CREDENTIAL_ID, credential)
    }

    fn bound_as(id: &str, credential: Option<(&str, &str)>) -> RangerBinding {
        RangerBinding {
            service_name: "demo2".into(),
            egret_url: "http://host.docker.internal:8080".into(),
            poll_interval_ms: egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS,
            credential: credential.map(|(u, p)| {
                crate::ranger_config::RenderableCredential::new(id, u, p).expect("renderable")
            }),
        }
    }

    /// Unbound (no credential) the files are the pre-refactor golden: the
    /// access-control file in the ConfigMap, the security XML in the Secret.
    #[test]
    fn the_provisioned_ranger_files_equal_the_renderer_and_the_pre_refactor_golden() {
        let mut spec = sample_spec();
        spec.ranger = Some(bound(None));
        let rendered = crate::ranger_config::ranger_plugin_files(
            egret_engine_mgmt_api::EngineKind::Trino,
            "demo2",
            "http://host.docker.internal:8080",
            egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS.into(),
            None,
        );
        let cm = trino_config_map(&spec);
        let secret = trino_ranger_secret(&spec).expect("a bound engine has the Secret");
        assert_eq!(
            cm["data"]["access-control.properties"].as_str().unwrap(),
            rendered.get("access-control.properties").unwrap()
        );
        assert_eq!(
            secret["stringData"]["ranger-trino-security.xml"]
                .as_str()
                .unwrap(),
            rendered.get("ranger-trino-security.xml").unwrap()
        );
        assert_eq!(
            cm["data"]["access-control.properties"].as_str().unwrap(),
            include_str!(
                "../../egret-engine-mgmt-api/tests/golden/trino.access-control.properties"
            )
        );
        assert_eq!(
            secret["stringData"]["ranger-trino-security.xml"]
                .as_str()
                .unwrap(),
            include_str!(
                "../../egret-engine-mgmt-api/tests/golden/trino.ranger-trino-security.xml"
            )
        );
    }

    /// A9: the credential is in the `{name}-rc-{tag}` Secret and nowhere
    /// else — not the ConfigMap, not the env Secret, not a pod template.
    #[test]
    fn the_credential_is_rendered_only_into_the_ranger_secret() {
        let mut spec = sample_spec();
        spec.ranger = Some(bound(Some(("engine-1", "Pw_only-in-the-secret"))));
        let cm = trino_config_map(&spec);
        assert!(
            cm["data"].get("ranger-trino-security.xml").is_none(),
            "the security XML left the ConfigMap"
        );
        let secret = trino_ranger_secret(&spec).unwrap();
        assert_eq!(secret["kind"], "Secret");
        assert_eq!(secret["type"], "Opaque");
        assert_eq!(secret["metadata"]["name"], "demo-rc-0b5ad7".to_string());
        assert_eq!(secret["metadata"]["namespace"], "egret-engines");
        assert_eq!(secret["metadata"]["labels"]["egret/engine"], "demo");
        let xml = secret["stringData"]["ranger-trino-security.xml"]
            .as_str()
            .unwrap();
        assert!(xml.contains(
            "<name>ranger.plugin.trino.policy.rest.client.password</name><value>Pw_only-in-the-secret</value>"
        ));
        let elsewhere = [
            cm,
            trino_catalogs_config_map(&spec),
            trino_env_secret(&spec),
            trino_deployment(&spec, Role::Coordinator),
            trino_deployment(&spec, Role::Worker),
            trino_service(&spec),
        ];
        for obj in elsewhere {
            assert!(
                !obj.to_string().contains("Pw_only-in-the-secret"),
                "{} {} carries the password",
                obj["kind"],
                obj["metadata"]["name"]
            );
        }
        assert!(!format!("{spec:?}").contains("Pw_only-in-the-secret"));
    }

    #[test]
    fn both_roles_mount_the_security_xml_from_the_ranger_secret() {
        let mut spec = sample_spec();
        spec.ranger = Some(bound(Some(("engine-1", "pw"))));
        for role in [Role::Coordinator, Role::Worker] {
            let dep = trino_deployment(&spec, role);
            let pod = &dep["spec"]["template"]["spec"];
            let volume = pod["volumes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == "ranger-secret")
                .expect("a ranger-secret volume");
            assert_eq!(volume["secret"]["secretName"], "demo-rc-0b5ad7".to_string());
            let mount = pod["containers"][0]["volumeMounts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|m| m["mountPath"] == "/etc/trino/ranger-trino-security.xml")
                .expect("the security XML is mounted");
            assert_eq!(mount["name"], "ranger-secret");
            assert_eq!(mount["subPath"], "ranger-trino-security.xml");
        }
    }

    #[test]
    fn an_unbound_engine_has_no_ranger_secret_and_no_volume() {
        let spec = sample_spec();
        assert!(trino_ranger_secret(&spec).is_none());
        let dep = trino_deployment(&spec, Role::Coordinator);
        assert!(dep["spec"]["template"]["spec"]["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["name"] != "ranger-secret"));
    }

    /// A rotation changes nothing but the Secret, and a `subPath` mount is
    /// copied at container start, so only the pod-template checksum can make
    /// the pods pick the new password up.
    #[test]
    fn a_new_ranger_password_rolls_the_pods_and_the_same_one_does_not() {
        let checksum = |pw: &str| {
            let mut spec = sample_spec();
            spec.ranger = Some(bound(Some(("engine-1", pw))));
            [Role::Coordinator, Role::Worker].map(|role| {
                trino_deployment(&spec, role)["spec"]["template"]["metadata"]["annotations"]
                    ["egret/config-checksum"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
        };
        assert_eq!(checksum("pw-one"), checksum("pw-one"), "deterministic");
        let (one, two) = (checksum("pw-one"), checksum("pw-two"));
        assert_ne!(one[0], two[0], "the coordinator would not roll");
        assert_ne!(one[1], two[1], "the workers would not roll");
    }

    #[tokio::test]
    async fn the_ranger_secret_is_applied_before_the_deployments() {
        use crate::kube_client::FakeKube;
        let kube = std::sync::Arc::new(FakeKube::default());
        *kube.fail_apply_kind.lock().unwrap() = Some("Deployment".into());
        let p = TrinoProvisioner::new(kube.clone());
        let mut spec = sample_spec();
        spec.ranger = Some(bound(Some(("engine-1", "pw"))));
        p.update(&spec)
            .await
            .expect_err("the Deployment apply fails");
        let applied = kube.applied.lock().unwrap();
        assert!(
            applied.contains_key("Secret/demo-rc-0b5ad7"),
            "the Secret must exist before any pod mounts it"
        );
        assert!(!applied.contains_key("Deployment/demo-coordinator"));
    }

    #[tokio::test]
    async fn unbinding_removes_the_ranger_secret_on_update() {
        use crate::kube_client::FakeKube;
        let kube = std::sync::Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        let mut bound_spec = sample_spec();
        bound_spec.ranger = Some(bound(Some(("engine-1", "pw"))));
        p.update(&bound_spec).await.unwrap();
        p.update(&sample_spec()).await.unwrap();
        let deleted = kube.deleted.lock().unwrap();
        assert!(deleted.contains(&"Secret/demo-ranger-secret".to_string()));
        assert!(
            deleted.contains(&"Secret/demo-rc-0b5ad7".to_string()),
            "{deleted:?}"
        );
    }

    #[test]
    fn ranger_binding_templates_access_control() {
        let mut spec = sample_spec();
        spec.ranger = Some(bound(None));
        let cm = trino_config_map(&spec);
        let data = &cm["data"];
        let ac = data["access-control.properties"].as_str().unwrap();
        assert!(ac.contains("access-control.name=ranger"));
        assert!(ac.contains("ranger.service.name=demo2"));
        assert!(ac.contains("ranger.plugin.config.resource=/etc/trino/ranger-trino-security.xml"));
        let secret = trino_ranger_secret(&spec).unwrap();
        let xml = secret["stringData"]["ranger-trino-security.xml"]
            .as_str()
            .unwrap();
        assert!(xml.contains("<name>ranger.plugin.trino.policy.rest.url</name>"));
        assert!(xml.contains("<value>http://host.docker.internal:8080/ranger</value>"));
        assert!(xml.contains("<name>ranger.plugin.trino.service.name</name>"));
        assert!(xml.contains("<value>demo2</value>"));
        let cc = data["config-coordinator.properties"].as_str().unwrap();
        assert!(cc.contains("access-control.config-files=/etc/trino/access-control.properties"));
    }
    #[test]
    fn no_ranger_binding_uses_allow_all_access_control() {
        // Without a Ranger binding the engine must still get an EXPLICIT allow-all
        // access control (+ config-files pointing at it), not Trino's default —
        // otherwise the coordinator denies `egret_delegator`'s per-user
        // impersonation. The ranger XML stays absent.
        let cm = trino_config_map(&sample_spec());
        assert_eq!(
            cm["data"]["access-control.properties"].as_str().unwrap(),
            "access-control.name=allow-all\n"
        );
        assert!(cm["data"].get("ranger-trino-security.xml").is_none());
        assert!(cm["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .contains("access-control.config-files=/etc/trino/access-control.properties"));
    }
    #[test]
    fn ranger_binding_mounts_config_files() {
        let mut spec = sample_spec();
        spec.ranger = Some(bound(None));
        let dep = trino_deployment(&spec, Role::Coordinator);
        let mounts = dep["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]
            .as_array()
            .unwrap();
        let paths: Vec<&str> = mounts
            .iter()
            .map(|m| m["mountPath"].as_str().unwrap())
            .collect();
        assert!(paths.contains(&"/etc/trino/access-control.properties"));
        assert!(paths.contains(&"/etc/trino/ranger-trino-security.xml"));
    }

    #[test]
    fn hpa_object_shape() {
        let mut s = sample_spec();
        s.config.worker.base.replicas = 2;
        s.config.autoscaling = Some(Autoscaling {
            mode: "hpa".into(),
            hpa: Some(HpaConfig {
                max_replicas: 5,
                target_cpu_percent: Some(60),
                target_memory_percent: Some(80),
                behavior_raw: None,
            }),
            keda: None,
        });
        let h = trino_hpa(&s).unwrap();
        assert_eq!(h["apiVersion"], "autoscaling/v2");
        assert_eq!(h["kind"], "HorizontalPodAutoscaler");
        assert_eq!(h["metadata"]["name"], format!("{}-worker", s.name));
        assert_eq!(
            h["spec"]["scaleTargetRef"]["name"],
            format!("{}-worker", s.name)
        );
        assert_eq!(h["spec"]["minReplicas"], 2);
        assert_eq!(h["spec"]["maxReplicas"], 5);
        let metrics = h["spec"]["metrics"].as_array().unwrap();
        assert!(metrics.iter().any(|m| m["resource"]["name"] == "cpu"
            && m["resource"]["target"]["averageUtilization"] == 60));
        assert!(metrics.iter().any(|m| m["resource"]["name"] == "memory"
            && m["resource"]["target"]["averageUtilization"] == 80));
        assert!(trino_scaled_object(&s).is_none()); // mode=hpa ⇒ no ScaledObject
    }
    #[test]
    fn scaledobject_and_triggers() {
        let mut s = sample_spec();
        s.config.autoscaling = Some(Autoscaling {
            mode: "keda".into(),
            hpa: None,
            keda: Some(KedaConfig {
                min_replica_count: 0,
                max_replica_count: 4,
                polling_interval: Some(30),
                cooldown_period: Some(300),
                triggers: vec![
                    KedaTrigger {
                        kind: "cron".into(),
                        cron: Some(CronTrigger {
                            start: "0 8 * * *".into(),
                            end: "0 20 * * *".into(),
                            desired_replicas: 3,
                            timezone: "UTC".into(),
                        }),
                        prometheus: None,
                        raw_override: None,
                    },
                    KedaTrigger {
                        kind: "raw".into(),
                        cron: None,
                        prometheus: None,
                        raw_override: Some(json!({"type":"kafka","metadata":{"topic":"t"}})),
                    },
                ],
                fallback_raw: None,
            }),
        });
        let so = trino_scaled_object(&s).unwrap();
        assert_eq!(so["apiVersion"], "keda.sh/v1alpha1");
        assert_eq!(so["kind"], "ScaledObject");
        assert_eq!(
            so["spec"]["scaleTargetRef"]["name"],
            format!("{}-worker", s.name)
        );
        assert_eq!(so["spec"]["minReplicaCount"], 0);
        assert_eq!(so["spec"]["maxReplicaCount"], 4);
        assert_eq!(so["spec"]["triggers"][0]["type"], "cron");
        assert_eq!(
            so["spec"]["triggers"][0]["metadata"]["desiredReplicas"],
            "3"
        ); // KEDA wants strings
        assert_eq!(so["spec"]["triggers"][1]["type"], "kafka"); // raw verbatim
        assert!(trino_hpa(&s).is_none());
    }
    #[test]
    fn worker_omits_replicas_when_autoscaling() {
        let mut s = sample_spec();
        s.config.autoscaling = Some(Autoscaling {
            mode: "hpa".into(),
            hpa: Some(HpaConfig {
                max_replicas: 3,
                ..Default::default()
            }),
            keda: None,
        });
        let w = trino_deployment(&s, Role::Worker);
        assert!(w["spec"].get("replicas").is_none() || w["spec"]["replicas"].is_null());
        // coordinator still has replicas:
        assert!(trino_deployment(&s, Role::Coordinator)["spec"]["replicas"].is_number());
        // and with no autoscaling the worker keeps replicas:
        let mut s2 = sample_spec();
        assert!(trino_deployment(&s2, Role::Worker)["spec"]["replicas"].is_number());
        let _ = &mut s2;
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
    fn image_override_and_default() {
        // Default: repository is trinodb/trino, tag is image_version.
        let s = sample_spec();
        let d = trino_deployment(&s, Role::Coordinator);
        assert_eq!(
            d["spec"]["template"]["spec"]["containers"][0]["image"],
            "trinodb/trino:483"
        );

        // Override: full image string wins verbatim.
        let mut s2 = sample_spec();
        s2.config.image = Some("ghcr.io/acme/trino:471-patched".into());
        let d2 = trino_deployment(&s2, Role::Coordinator);
        assert_eq!(
            d2["spec"]["template"]["spec"]["containers"][0]["image"],
            "ghcr.io/acme/trino:471-patched"
        );
    }
    #[test]
    fn auth_config_password_and_jwt() {
        let mut s = sample_spec();
        s.config.auth = Some(Auth {
            password_users: vec![
                PasswordEntry {
                    username: "bob".into(),
                    bcrypt_hash: "$2b$10$abc".into(),
                },
                PasswordEntry {
                    username: "egret_delegator".into(),
                    bcrypt_hash: "$2y$10$delegator".into(),
                },
            ],
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
        // Delegated queries run as the caller's username; authenticate the JWT as
        // `preferred_username` so principal == the sent X-Trino-User (self-impersonation).
        // Trino 483 does NOT allow that by default: it checks `impersonate` on
        // `trinouser=<user>` even when principal == user (live result
        // docs/en/plans/2026-10-13-view-governance-live-result.md §4.8), so egret
        // seeds `egret-user-impersonate-self` on every delegating profile.
        assert!(cc.contains("http-server.authentication.jwt.principal-field=preferred_username"));
        // workers don't authenticate clients
        assert!(!cm["data"]["config-worker.properties"]
            .as_str()
            .unwrap()
            .contains("authentication.type"));
        assert!(cm["data"]["password.db"]
            .as_str()
            .unwrap()
            .contains("bob:$2b$10$abc"));
        // The delegator service user is rendered like any other password entry.
        assert!(cm["data"]["password.db"]
            .as_str()
            .unwrap()
            .contains("egret_delegator:$2y$"));
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
    fn jwt_tls_renders_https_secret_and_coordinator_mount() {
        let mut s = sample_spec();
        s.config.auth = Some(Auth {
            password_users: vec![],
            jwt: Some(Jwt {
                jwks_uri: "https://kc/realms/egret/protocol/openid-connect/certs".into(),
                required_audience: None,
            }),
        });
        s.config.tls = Some(Tls {
            keystore_pem: "PEMDATA".into(),
            port: 8443,
        });

        // Coordinator config carries the https listener directives.
        let cm = trino_config_map(&s);
        let cc = cm["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap();
        assert!(cc.contains("http-server.https.enabled=true"));
        assert!(cc.contains("http-server.https.port=8443"));
        assert!(cc.contains("http-server.https.keystore.path=/etc/trino/tls/keystore.pem"));
        // The internal http port + discovery URI are untouched.
        assert!(cc.contains("http-server.http.port=8080"));
        // Workers do not serve https.
        assert!(!cm["data"]["config-worker.properties"]
            .as_str()
            .unwrap()
            .contains("http-server.https.enabled"));

        // A `{name}-tls` Secret carries the keystore PEM.
        let sec = trino_tls_secret(&s).expect("tls secret present");
        assert_eq!(sec["kind"], "Secret");
        assert_eq!(sec["metadata"]["name"], format!("{}-tls", s.name));
        assert_eq!(sec["stringData"]["keystore.pem"], "PEMDATA");

        // The coordinator pod mounts the keystore.
        let d = trino_deployment(&s, Role::Coordinator);
        let volumes = d["spec"]["template"]["spec"]["volumes"].as_array().unwrap();
        assert!(volumes
            .iter()
            .any(|v| v["name"] == "tls" && v["secret"]["secretName"] == format!("{}-tls", s.name)));
        let mounts = d["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]
            .as_array()
            .unwrap();
        assert!(mounts.iter().any(|m| m["name"] == "tls"
            && m["mountPath"] == "/etc/trino/tls/keystore.pem"
            && m["subPath"] == "keystore.pem"));

        // Workers get neither the volume nor the mount.
        let dw = trino_deployment(&s, Role::Worker);
        assert!(!dw["spec"]["template"]["spec"]["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "tls"));
        assert!(
            !dw["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["name"] == "tls")
        );

        // Absent when tls is unset.
        assert!(trino_tls_secret(&sample_spec()).is_none());
    }

    // jwt-only auth + TLS on 8443 = a delegation-capable jwt-mode coordinator.
    fn jwt_tls_spec() -> EngineSpec {
        let mut s = sample_spec();
        s.config.auth = Some(Auth {
            password_users: vec![],
            jwt: Some(Jwt {
                jwks_uri: "https://kc/realms/egret/protocol/openid-connect/certs".into(),
                required_audience: None,
            }),
        });
        s.config.tls = Some(Tls {
            keystore_pem: "P".into(),
            port: 8443,
        });
        s
    }

    #[test]
    fn jwt_engine_rejects_insecure_over_http() {
        let cc = trino_config_map(&jwt_tls_spec())["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(cc.contains("http-server.authentication.allow-insecure-over-http=false"));
        assert!(!cc.contains("http-server.authentication.allow-insecure-over-http=true"));
    }

    #[test]
    fn impersonate_engine_keeps_insecure_over_http() {
        let mut s = sample_spec();
        // impersonate engine = PASSWORD monitor user + JWT + TLS.
        s.config.auth = Some(Auth {
            password_users: vec![PasswordEntry {
                username: "egret_delegator".into(),
                bcrypt_hash: "$2b$10$abc".into(),
            }],
            jwt: Some(Jwt {
                jwks_uri: "https://kc/realms/egret/protocol/openid-connect/certs".into(),
                required_audience: None,
            }),
        });
        s.config.tls = Some(Tls {
            keystore_pem: "P".into(),
            port: 8443,
        });
        let cc = trino_config_map(&s)["data"]["config-coordinator.properties"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(cc.contains("http-server.authentication.allow-insecure-over-http=true"));
    }

    #[test]
    fn service_exposes_https_port_when_tls() {
        let svc = trino_service(&jwt_tls_spec());
        let ports = svc["spec"]["ports"].as_array().unwrap();
        assert!(ports
            .iter()
            .any(|p| p["port"] == 8443 && p["name"] == "https"));
        assert!(ports
            .iter()
            .any(|p| p["port"] == 8080 && p["name"] == "http"));

        // No TLS ⇒ single 8080 http port only.
        let svc2 = trino_service(&sample_spec());
        let ports2 = svc2["spec"]["ports"].as_array().unwrap();
        assert_eq!(ports2.len(), 1);
        assert_eq!(ports2[0]["port"], 8080);
        assert!(!ports2.iter().any(|p| p["port"] == 8443));
    }

    #[test]
    fn coordinator_pod_exposes_https_container_port_when_tls() {
        let d = trino_deployment(&jwt_tls_spec(), Role::Coordinator);
        let ports = d["spec"]["template"]["spec"]["containers"][0]["ports"]
            .as_array()
            .unwrap();
        assert!(ports.iter().any(|p| p["containerPort"] == 8443));
        assert!(ports.iter().any(|p| p["containerPort"] == 8080));

        // Workers never serve https.
        let dw = trino_deployment(&jwt_tls_spec(), Role::Worker);
        assert!(!dw["spec"]["template"]["spec"]["containers"][0]["ports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["containerPort"] == 8443));

        // A non-TLS coordinator does not expose 8443 either.
        let dn = trino_deployment(&sample_spec(), Role::Coordinator);
        assert!(!dn["spec"]["template"]["spec"]["containers"][0]["ports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["containerPort"] == 8443));
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
    fn log_level_from_additional_log_properties_has_no_duplicate() {
        // New engines carry the level as an io.trino entry (no structured
        // log_level) — must not emit a second default io.trino line.
        let mut s = sample_spec();
        s.config.log_level = None;
        s.config
            .additional_log_properties
            .insert("io.trino".into(), "WARN".into());
        let cm = trino_config_map(&s);
        let lp = cm["data"]["log.properties"].as_str().unwrap().to_string();
        assert!(lp.contains("io.trino=WARN"));
        assert_eq!(lp.matches("io.trino=").count(), 1);
    }

    /// Live result 2026-10-20 finding 1: Ranger's `AuditProviderFactory`
    /// logs every plugin property at INFO, the REST client password
    /// included. A bound engine's log.properties holds that logger at WARN,
    /// after the user's lines, and keeps them.
    #[test]
    fn a_bound_engine_silences_the_logger_that_prints_the_ranger_password() {
        let mut s = sample_spec();
        s.ranger = Some(bound(Some(("engine-1", "pw"))));
        s.config
            .additional_log_properties
            .insert("io.trino.execution".into(), "DEBUG".into());
        let lp = trino_config_map(&s)["data"]["log.properties"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(lp.contains("io.trino=DEBUG\n"), "{lp}");
        assert!(lp.contains("io.trino.execution=DEBUG\n"), "{lp}");
        assert!(
            lp.ends_with("org.apache.ranger.audit.provider.AuditProviderFactory=WARN\n"),
            "{lp}"
        );
    }

    /// The user's own value for that logger cannot turn the leak back on: it
    /// is dropped and egret's line is the only one, and the last.
    #[test]
    fn a_user_level_for_the_leaking_logger_does_not_win() {
        let mut s = sample_spec();
        s.ranger = Some(bound(Some(("engine-1", "pw"))));
        s.config.additional_log_properties.insert(
            "org.apache.ranger.audit.provider.AuditProviderFactory".into(),
            "DEBUG".into(),
        );
        s.config.additional_log_properties.insert(
            " org.apache.ranger.audit.provider.AuditProviderFactory ".into(),
            "INFO".into(),
        );
        s.config
            .additional_log_properties
            .insert("org.apache.ranger".into(), "DEBUG".into());
        let lp = trino_config_map(&s)["data"]["log.properties"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(lp.matches("AuditProviderFactory").count(), 1, "{lp}");
        assert!(
            lp.ends_with("org.apache.ranger.audit.provider.AuditProviderFactory=WARN\n"),
            "{lp}"
        );
        // A parent-logger level is the user's to set: the child's explicit
        // WARN outranks it.
        assert!(lp.contains("org.apache.ranger=DEBUG\n"), "{lp}");
    }

    /// An unbound engine loads no Ranger plugin, so its log.properties is
    /// unchanged (and so is its checksum: no needless roll).
    #[test]
    fn an_unbound_engine_has_no_ranger_logger_line() {
        let lp = trino_config_map(&sample_spec())["data"]["log.properties"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(lp, "io.trino=DEBUG\n");
    }

    /// log.properties is in the pod-template checksum, so a change to it (as
    /// this override on upgrade) rolls the pods.
    #[test]
    fn a_log_properties_change_rolls_the_pods() {
        let checksum = |level: &str| {
            let mut s = sample_spec();
            s.config.log_level = Some(level.into());
            trino_deployment(&s, Role::Coordinator)["spec"]["template"]["metadata"]["annotations"]
                ["egret/config-checksum"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_ne!(checksum("DEBUG"), checksum("INFO"));
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
            "trinodb/trino:483"
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
            s3_endpoint: "http://rustfs:9000".into(),
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
            s3_endpoint: "http://rustfs:9000".into(),
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
            tls_cert_secret_ref: None,
            tls_cert_pem: None,
            tls_key_pem: None,
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
    fn ingress_tls_secret_renders_typed_kubernetes_io_tls() {
        let mut s = sample_spec();
        s.config.ingress = Some(crate::spec::Ingress {
            enabled: true,
            class_name: Some("nginx".into()),
            host: "trino-demo.egret.example.com".into(),
            tls_secret: Some("demo-ingress-tls".into()),
            tls_cert_secret_ref: None,
            tls_cert_pem: Some("CERTPEM".into()),
            tls_key_pem: Some("KEYPEM".into()),
            annotations: BTreeMap::new(),
        });
        let sec = trino_ingress_tls_secret(&s).unwrap();
        assert_eq!(sec["type"], "kubernetes.io/tls");
        assert_eq!(sec["metadata"]["name"], "demo-ingress-tls");
        assert_eq!(sec["metadata"]["namespace"], "egret-engines");
        assert_eq!(
            sec["metadata"]["labels"]["app.kubernetes.io/managed-by"],
            "egret"
        );
        assert_eq!(sec["stringData"]["tls.crt"], "CERTPEM");
        assert_eq!(sec["stringData"]["tls.key"], "KEYPEM");

        // No resolved cert PEM -> no Secret projected.
        let mut s2 = sample_spec();
        s2.config.ingress = Some(crate::spec::Ingress {
            enabled: true,
            class_name: Some("nginx".into()),
            host: "trino-demo.egret.example.com".into(),
            tls_secret: Some("demo-ingress-tls".into()),
            tls_cert_secret_ref: None,
            tls_cert_pem: None,
            tls_key_pem: Some("KEYPEM".into()),
            annotations: BTreeMap::new(),
        });
        assert!(trino_ingress_tls_secret(&s2).is_none());
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

    #[test]
    fn cluster_pod_and_deployment_fields() {
        let mut s = sample_spec();
        s.config.service_account = Some("trino-sa".into());
        s.config.image_pull_secrets = vec!["regcred".into()];
        s.config.image_pull_policy = Some("Always".into());
        s.config.coordinator.share_process_namespace = Some(true);
        s.config.pod_security_context = Some(SecurityContext {
            run_as_user: Some(1000),
            fs_group: Some(1000),
            ..Default::default()
        });
        s.config.container_security_context = Some(SecurityContext {
            run_as_non_root: Some(true),
            raw_override: Some(json!({"readOnlyRootFilesystem": true})),
            ..Default::default()
        });
        s.config.deployment_strategy = Some(DeploymentStrategy {
            kind: "Recreate".into(),
            max_surge: None,
            max_unavailable: None,
        });
        s.config
            .deployment_annotations
            .insert("team".into(), "data".into());
        s.config.revision_history_limit = Some(3);
        s.config.progress_deadline_seconds = Some(600);
        let d = trino_deployment(&s, Role::Coordinator);
        let ps = &d["spec"]["template"]["spec"];
        assert_eq!(ps["serviceAccountName"], "trino-sa");
        assert_eq!(ps["imagePullSecrets"][0]["name"], "regcred");
        assert_eq!(ps["shareProcessNamespace"], true);
        assert_eq!(ps["securityContext"]["runAsUser"], 1000);
        assert_eq!(ps["containers"][0]["imagePullPolicy"], "Always");
        assert_eq!(ps["containers"][0]["securityContext"]["runAsNonRoot"], true);
        assert_eq!(
            ps["containers"][0]["securityContext"]["readOnlyRootFilesystem"],
            true
        ); // raw merged
        assert_eq!(d["spec"]["strategy"]["type"], "Recreate");
        assert_eq!(d["spec"]["revisionHistoryLimit"], 3);
        assert_eq!(d["spec"]["progressDeadlineSeconds"], 600);
        assert_eq!(d["metadata"]["annotations"]["team"], "data");
    }

    #[test]
    fn trino_pull_secret_renders_dockerconfigjson() {
        let mut spec = sample_spec();
        spec.name = "eng1".into();
        spec.namespace = "egret-engines".into();
        spec.config.image_pull_secret_json = Some(r#"{"auths":{}}"#.into());
        let s = trino_pull_secret(&spec).unwrap();
        assert_eq!(s["type"], "kubernetes.io/dockerconfigjson");
        assert_eq!(s["metadata"]["name"], "eng1-pull");
        assert_eq!(s["stringData"][".dockerconfigjson"], r#"{"auths":{}}"#);
        spec.config.image_pull_secret_json = None;
        assert!(trino_pull_secret(&spec).is_none());
    }

    #[test]
    fn containers_envfrom_mounts_render_and_gs_preserved() {
        let mut s = sample_spec();
        // worker graceful shutdown sets a grace period we must not clobber:
        s.config.worker.base.termination_grace_seconds = Some(30); // must lose to graceful shutdown
        s.config.worker.graceful_shutdown = Some(GracefulShutdown {
            enabled: true,
            grace_period_seconds: 240,
        });
        s.config.worker.base.init_containers = vec![MiniContainer {
            name: "wait".into(),
            image: "busybox".into(),
            command: vec!["sh".into(), "-c".into()],
            args: vec!["echo hi".into()],
            ..Default::default()
        }];
        s.config.worker.base.sidecar_containers = vec![MiniContainer {
            name: "raw-side".into(),
            image: "".into(),
            raw_override: Some(json!({"name":"exporter","image":"prom/exporter"})),
            ..Default::default()
        }];
        s.config.env_from = vec![EnvFromSource {
            config_map_ref: Some("shared-env".into()),
            secret_ref: None,
        }];
        s.config.worker.base.config_mounts = vec![ObjectMount {
            name: "extra-cm".into(),
            mount_path: "/etc/extra".into(),
            sub_path: None,
            default_mode: None,
        }];
        s.config.worker.base.secret_mounts = vec![ObjectMount {
            name: "kt".into(),
            mount_path: "/etc/krb".into(),
            sub_path: Some("krb5.keytab".into()),
            default_mode: Some(0o400),
        }];
        let d = trino_deployment(&s, Role::Worker);
        let ps = &d["spec"]["template"]["spec"];
        assert_eq!(ps["initContainers"][0]["name"], "wait");
        assert!(ps["containers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "exporter")); // sidecar raw appended
        assert_eq!(
            ps["containers"][0]["envFrom"][0]["configMapRef"]["name"],
            "shared-env"
        );
        assert!(ps["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "extra-cm" && v["configMap"]["name"] == "extra-cm"));
        assert!(ps["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["secret"]["secretName"] == "kt"));
        let vm = &ps["containers"][0]["volumeMounts"];
        assert!(vm
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["mountPath"] == "/etc/krb" && m["subPath"] == "krb5.keytab"));
        // graceful shutdown grace period preserved (worker GS uses its own value, not 30):
        assert_ne!(ps["terminationGracePeriodSeconds"], 30);
    }

    #[test]
    fn init_containers_are_per_role() {
        let mut s = sample_spec();
        s.config.coordinator.init_containers = vec![MiniContainer {
            name: "coord-init".into(),
            image: "busybox".into(),
            ..Default::default()
        }];
        s.config.worker.base.init_containers = vec![MiniContainer {
            name: "worker-init".into(),
            image: "busybox".into(),
            ..Default::default()
        }];
        let coord = trino_deployment(&s, Role::Coordinator);
        let worker = trino_deployment(&s, Role::Worker);
        assert_eq!(
            coord["spec"]["template"]["spec"]["initContainers"][0]["name"],
            "coord-init"
        );
        assert_eq!(
            worker["spec"]["template"]["spec"]["initContainers"][0]["name"],
            "worker-init"
        );
        // Coordinator must NOT carry the worker's init container.
        assert!(coord["spec"]["template"]["spec"]["initContainers"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["name"] != "worker-init"));
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
            tls_cert_secret_ref: None,
            tls_cert_pem: None,
            tls_key_pem: None,
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
    async fn provision_applies_autoscaler_and_delete_removes_both() {
        use crate::kube_client::FakeKube;
        let kube = std::sync::Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        let mut s = sample_spec();
        s.config.autoscaling = Some(Autoscaling {
            mode: "hpa".into(),
            hpa: Some(HpaConfig {
                max_replicas: 3,
                ..Default::default()
            }),
            keda: None,
        });
        p.provision(&s).await.unwrap();
        assert!(kube
            .applied
            .lock()
            .unwrap()
            .contains_key(&format!("HorizontalPodAutoscaler/{}-worker", s.name)));

        p.delete(&s.namespace, &s.name).await.unwrap();
        // delete is best-effort for BOTH kinds regardless of mode:
        let d = kube.deleted.lock().unwrap();
        assert!(d.contains(&format!("HorizontalPodAutoscaler/{}-worker", s.name)));
        assert!(d.iter().any(|x| x.starts_with("ScaledObject/")));
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
        assert_eq!(d.len(), 13); // 2 deployments, service, 2 configmaps, env secret, tls secret, ingress-tls secret, pull secret, ranger secret, ingress, hpa, scaledobject
        assert!(d.contains(&"Secret/demo-ranger-secret".to_string()));
    }

    /// D6: every `let _ =` used to swallow a 403/timeout, so the row was deleted
    /// and the cluster orphaned. A non-404 failure must now surface — after the
    /// other objects have still been deleted.
    #[tokio::test]
    async fn delete_reports_a_failed_object_after_attempting_the_rest() {
        use crate::kube_client::FakeKube;
        let kube = std::sync::Arc::new(FakeKube::default());
        kube.fail_delete("Deployment", "demo-coordinator", "forbidden");
        let p = TrinoProvisioner::new(kube.clone());
        let err = p
            .delete("egret-engines", "demo")
            .await
            .expect_err("a non-404 failure must surface");
        assert!(format!("{err:#}").contains("Deployment/demo-coordinator: forbidden"));
        assert_eq!(
            kube.deleted.lock().unwrap().len(),
            12,
            "the other 12 objects are deleted"
        );
    }

    // ── Task 6b: the rollout of ONE Ranger credential is observable ─────────

    /// Every workload and the Secret name the credential they were rendered
    /// with (its id, never its password); nothing does when unbound.
    #[test]
    fn both_roles_and_the_secret_name_the_rendered_credential() {
        use crate::rollout::RANGER_CREDENTIAL_ANNOTATION as ANN;
        let mut spec = sample_spec();
        spec.ranger = Some(bound(Some(("engine-1", "Pw_secret"))));
        for role in [Role::Coordinator, Role::Worker] {
            let d = trino_deployment(&spec, role);
            assert_eq!(
                d["spec"]["template"]["metadata"]["annotations"][ANN],
                TEST_CREDENTIAL_ID
            );
            assert!(!d.to_string().contains("Pw_secret"));
        }
        let secret = trino_ranger_secret(&spec).unwrap();
        assert_eq!(secret["metadata"]["annotations"][ANN], TEST_CREDENTIAL_ID);

        // Bound without a credential, or unbound: no annotation anywhere.
        for ranger in [Some(bound(None)), None] {
            let mut spec = sample_spec();
            spec.ranger = ranger;
            let d = trino_deployment(&spec, Role::Worker);
            assert!(d["spec"]["template"]["metadata"]["annotations"]
                .get(ANN)
                .is_none());
        }
    }

    /// The live cluster of a fully rolled Trino engine `demo` on `cred`.
    fn rolled_trino(kube: &crate::kube_client::FakeKube, cred: &str) {
        use crate::rollout::tests::{ready_pod, rolled_deployment, secret};
        kube.set_live(secret(&format!("demo-rc-{cred}"), cred));
        kube.set_live(rolled_deployment("demo-coordinator", cred));
        kube.set_live(rolled_deployment("demo-worker", cred));
        let labels = json!({ "app.kubernetes.io/instance": "demo" });
        *kube.live_pods.lock().unwrap() = vec![
            ready_pod("demo-coordinator-a", labels.clone(), cred),
            ready_pod("demo-worker-a", labels.clone(), cred),
            ready_pod("demo-worker-b", labels, cred),
        ];
    }

    #[tokio::test]
    async fn a_trino_rollout_is_complete_only_for_the_credential_it_rolled_out() {
        use crate::kube_client::FakeKube;
        let kube = Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        rolled_trino(&kube, "c2");
        assert_eq!(
            p.ranger_rollout("egret-engines", "demo", "c2")
                .await
                .unwrap(),
            RangerRollout::Complete
        );
        // An older rollout never answers for a newer credential.
        rolled_trino(&kube, "c1");
        assert!(matches!(
            p.ranger_rollout("egret-engines", "demo", "c2")
                .await
                .unwrap(),
            RangerRollout::Pending(_)
        ));
    }

    #[tokio::test]
    async fn a_stalled_trino_worker_rollout_is_pending() {
        use crate::kube_client::FakeKube;
        use crate::rollout::tests::{ready_pod, rolled_deployment};
        let kube = Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        rolled_trino(&kube, "c2");
        // The worker's new ReplicaSet has one of two pods; an old one still runs.
        let mut worker = rolled_deployment("demo-worker", "c2");
        worker["status"]["updatedReplicas"] = json!(1);
        worker["status"]["replicas"] = json!(3);
        kube.set_live(worker);
        kube.live_pods.lock().unwrap().push(ready_pod(
            "demo-worker-old",
            json!({ "app.kubernetes.io/instance": "demo" }),
            "c1",
        ));
        let RangerRollout::Pending(reason) = p
            .ranger_rollout("egret-engines", "demo", "c2")
            .await
            .unwrap()
        else {
            panic!("a stalled rollout is not complete");
        };
        assert!(reason.contains("demo-worker"), "{reason}");
    }

    #[tokio::test]
    async fn a_trino_rollout_with_a_workload_missing_or_a_failed_read_is_not_complete() {
        use crate::kube_client::FakeKube;
        let kube = Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        rolled_trino(&kube, "c2");
        kube.live.lock().unwrap().remove("Deployment/demo-worker");
        assert!(matches!(
            p.ranger_rollout("egret-engines", "demo", "c2")
                .await
                .unwrap(),
            RangerRollout::Pending(_)
        ));
        rolled_trino(&kube, "c2");
        *kube.fail_reads.lock().unwrap() = Some("connection refused".into());
        assert!(p
            .ranger_rollout("egret-engines", "demo", "c2")
            .await
            .is_err());
    }

    /// Each credential is rendered into its OWN Secret, which both roles mount:
    /// a pod built from a template that names a credential can only mount that
    /// credential's content (review round 1 of Task 6b).
    #[test]
    fn each_credential_has_its_own_secret_which_both_roles_mount() {
        let spec_for = |id: &str| {
            let mut spec = sample_spec();
            spec.ranger = Some(bound_as(id, Some(("engine-1", "pw"))));
            spec
        };
        let (a, b) = (spec_for("cred-1"), spec_for("cred-2"));
        let name = |s: &EngineSpec| {
            trino_ranger_secret(s).unwrap()["metadata"]["name"]
                .as_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(name(&a), "demo-rc-cred1");
        assert_eq!(name(&b), "demo-rc-cred2");
        assert_eq!(name(&a), name(&spec_for("cred-1")), "deterministic");
        let secret = trino_ranger_secret(&a).unwrap();
        assert_eq!(
            secret["metadata"]["labels"]["egret/component"],
            crate::rollout::RANGER_SECRET_COMPONENT
        );
        for role in [Role::Coordinator, Role::Worker] {
            let dep = trino_deployment(&a, role);
            let volume = dep["spec"]["template"]["spec"]["volumes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|v| v["name"] == "ranger-secret")
                .cloned()
                .unwrap();
            assert_eq!(volume["secret"]["secretName"], "demo-rc-cred1");
        }
    }

    /// The interleave the per-credential name closes: the Deployments and pods
    /// name c2, but only c1's Secret was ever written. The pods cannot be
    /// running c2's content, so the rollout is not complete.
    #[tokio::test]
    async fn pods_of_the_new_template_without_its_secret_are_not_rolled_out() {
        use crate::kube_client::FakeKube;
        use crate::rollout::tests::secret;
        let kube = Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        rolled_trino(&kube, "c2");
        kube.live.lock().unwrap().remove("Secret/demo-rc-c2");
        kube.set_live(secret("demo-rc-c1", "c1"));
        assert!(matches!(
            p.ranger_rollout("egret-engines", "demo", "c2")
                .await
                .unwrap(),
            RangerRollout::Pending(_)
        ));
    }

    /// Engine delete removes every Ranger Secret of THIS engine — listed by its
    /// labels, plus the legacy name — and none of another engine whose name
    /// starts the same.
    #[tokio::test]
    async fn delete_removes_every_ranger_secret_of_this_engine_only() {
        use crate::kube_client::FakeKube;
        let kube = Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        for (engine, id) in [("demo", "c1"), ("demo", "c2"), ("demo-x", "c3")] {
            let mut spec = sample_spec();
            spec.name = engine.into();
            spec.ranger = Some(bound_as(id, Some(("engine-1", "pw"))));
            kube.apply(&trino_ranger_secret(&spec).unwrap())
                .await
                .unwrap();
        }
        p.delete("egret-engines", "demo").await.unwrap();
        let deleted = kube.deleted.lock().unwrap();
        for gone in ["demo-rc-c1", "demo-rc-c2", "demo-ranger-secret"] {
            assert!(deleted.contains(&format!("Secret/{gone}")), "{deleted:?}");
        }
        assert!(!deleted.contains(&"Secret/demo-x-rc-c3".to_string()));
    }

    /// The retire of a credential deletes exactly its Secret (and the legacy
    /// one), never the current credential's.
    #[tokio::test]
    async fn retiring_deletes_exactly_the_retired_credentials_secret() {
        use crate::kube_client::FakeKube;
        use crate::rollout::tests::secret;
        let kube = Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        kube.set_live(secret("demo-rc-c1", "c1"));
        p.delete_ranger_secret("egret-engines", "demo", "c1")
            .await
            .unwrap();
        assert_eq!(
            *kube.deleted.lock().unwrap(),
            vec![
                "Secret/demo-rc-c1".to_string(),
                "Secret/demo-ranger-secret".to_string()
            ]
        );
    }

    /// Review round 3 (M1): the delete is conditional on the credential the
    /// Secret names. A Secret of that name written for ANOTHER credential, or
    /// changed since it was read, is kept.
    #[tokio::test]
    async fn a_same_named_secret_of_another_credential_is_not_deleted() {
        use crate::kube_client::FakeKube;
        use crate::rollout::tests::secret;
        let kube = Arc::new(FakeKube::default());
        let p = TrinoProvisioner::new(kube.clone());
        kube.set_live(secret("demo-rc-c1", "c1-but-another"));
        p.delete_ranger_secret("egret-engines", "demo", "c1")
            .await
            .unwrap();
        assert_eq!(
            *kube.deleted.lock().unwrap(),
            vec!["Secret/demo-ranger-secret".to_string()],
            "only the legacy Secret"
        );

        // A changed version (the precondition) is not deleted either.
        let kube = Arc::new(FakeKube::default());
        let mut live = secret("demo-rc-c1", "c1");
        live["metadata"]["resourceVersion"] = serde_json::json!("7");
        kube.set_live(live.clone());
        let found = kube
            .get_object_metadata("v1", "Secret", "egret-engines", "demo-rc-c1")
            .await
            .unwrap()
            .unwrap();
        live["metadata"]["resourceVersion"] = serde_json::json!("8");
        kube.set_live(live);
        assert!(kube
            .delete_if_unchanged(
                "v1",
                "Secret",
                "egret-engines",
                "demo-rc-c1",
                found["metadata"]["resourceVersion"].as_str(),
            )
            .await
            .is_err());
        assert!(kube.deleted.lock().unwrap().is_empty());
    }

    /// Trino mounts the Secret through a fixed volume name (`ranger-secret`),
    /// so only the Secret's own name bounds the engine name: a DNS-1123
    /// subdomain (253) — far above any name egret accepts (63).
    #[test]
    fn the_ranger_secret_name_fits_for_the_longest_engine_name() {
        let mut spec = sample_spec();
        spec.name = "a".repeat(63);
        spec.ranger = Some(bound_as(
            "ffffffff-ffff-4fff-bfff-ffffffffffff",
            Some(("engine-1", "pw")),
        ));
        let name = crate::rollout::spec_ranger_secret_name(&spec);
        assert!(name.len() <= 253 && name.len() <= 63 + 10, "{name}");
        assert!(name.ends_with("-rc-ffffff"), "{name}");
    }
}
