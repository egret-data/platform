//! Pure renderer: `EngineSpec` -> one `ContainerPlan` per Trino node.
//!
//! No Docker types here. The Kubernetes renderers stay the single source of the
//! engine's files: each `volumeMounts` entry of `trino_deployment` is resolved
//! to the ConfigMap/Secret data it points at, so a mount added on the Kubernetes
//! side shows up here (or fails loudly as an unsupported mount).

use crate::spec::{EngineKind, EngineSpec};
use crate::trino::{
    trino_catalogs_config_map, trino_config_map, trino_deployment, trino_env_secret,
    trino_tls_secret, Role,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanRole {
    Coordinator,
    Worker(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanFile {
    pub path: String,
    pub content: Vec<u8>,
    pub mode: u32,
}

#[derive(Clone, Debug)]
pub struct ContainerPlan {
    pub name: String,
    pub role: PlanRole,
    pub image: String,
    pub env: Vec<(String, String)>,
    pub labels: BTreeMap<String, String>,
    pub aliases: Vec<String>,
    pub files: Vec<PlanFile>,
    pub nano_cpus: Option<i64>,
    pub memory_bytes: Option<i64>,
}

pub struct PlanOptions<'a> {
    pub project: &'a str,
    pub allowed_images: &'a [String],
}

/// Everything the Docker path cannot honour. One list, so the error names every
/// offender at once instead of one per retry.
fn unsupported(spec: &EngineSpec) -> Vec<&'static str> {
    let c = &spec.config;
    let mut out = Vec::new();
    if spec.kind == EngineKind::StarRocks {
        out.push("engine kind StarRocks (Trino only)");
    }
    if c.ingress.as_ref().is_some_and(|i| i.enabled) {
        out.push("ingress");
    }
    if c.autoscaling.is_some() {
        out.push("autoscaling");
    }
    if c.image_pull_secret_json.is_some() {
        out.push("image_pull_secret");
    }
    for r in [&c.coordinator, &c.worker.base] {
        if !r.config_mounts.is_empty() || !r.secret_mounts.is_empty() {
            out.push("extra config/secret mounts");
            break;
        }
    }
    for r in [&c.coordinator, &c.worker.base] {
        if !r.additional_exposed_ports.is_empty() {
            out.push("additional exposed ports (no published ports on Docker)");
            break;
        }
    }
    for r in [&c.coordinator, &c.worker.base] {
        if r.share_process_namespace == Some(true) {
            out.push("shared process namespace");
            break;
        }
    }
    for r in [&c.coordinator, &c.worker.base] {
        if !r.init_containers.is_empty() {
            out.push("init containers");
            break;
        }
    }
    for r in [&c.coordinator, &c.worker.base] {
        if !r.sidecar_containers.is_empty() {
            out.push("extra containers");
            break;
        }
    }
    out
}

pub fn container_name(namespace: &str, name: &str, role: &PlanRole) -> String {
    match role {
        PlanRole::Coordinator => format!("egret-engine-{namespace}-{name}-coordinator"),
        PlanRole::Worker(i) => format!("egret-engine-{namespace}-{name}-worker-{i}"),
    }
}

/// Exact repository match: the image minus its digest and tag must equal an
/// allow-list entry. No registry-prefix guessing.
pub fn image_allowed(image: &str, allowed: &[String]) -> bool {
    let no_digest = image.split('@').next().unwrap_or(image);
    // A tag is a `:` after the last `/` (an earlier `:` is a registry port).
    let repo = match no_digest.rfind(':') {
        Some(i) if !no_digest[i..].contains('/') => &no_digest[..i],
        _ => no_digest,
    };
    allowed.iter().any(|a| a == repo)
}

pub fn parse_cpu(q: &str) -> Result<i64, String> {
    let bad = || format!("invalid cpu quantity {q:?}");
    let (num, scale) = match q.strip_suffix('m') {
        Some(n) => (n, 1_000_000.0),
        None => (q, 1_000_000_000.0),
    };
    let v: f64 = num.parse().map_err(|_| bad())?;
    let nano = (v * scale).round();
    // `as i64` saturates, so bound the value before converting.
    if !nano.is_finite() || nano < 1.0 || nano >= i64::MAX as f64 {
        return Err(bad());
    }
    Ok(nano as i64)
}

pub fn parse_memory(q: &str) -> Result<i64, String> {
    let bad = || format!("invalid memory quantity {q:?}");
    const UNITS: [(&str, i64); 8] = [
        ("Ki", 1 << 10),
        ("Mi", 1 << 20),
        ("Gi", 1 << 30),
        ("Ti", 1 << 40),
        ("K", 1_000),
        ("M", 1_000_000),
        ("G", 1_000_000_000),
        ("T", 1_000_000_000_000),
    ];
    let (num, mult) = UNITS
        .iter()
        .find_map(|(s, m)| q.strip_suffix(s).map(|n| (n, *m)))
        .unwrap_or((q, 1));
    let n: i64 = num.parse().map_err(|_| bad())?;
    if n <= 0 {
        return Err(bad());
    }
    n.checked_mul(mult).ok_or_else(bad)
}

fn file_mode(path: &str) -> u32 {
    if path.ends_with("/password.db") || path.ends_with("/keystore.pem") {
        0o600
    } else {
        0o644
    }
}

/// The files of one role: every `volumeMounts` entry of the Kubernetes
/// Deployment, resolved to the ConfigMap/Secret data behind it.
fn role_files(spec: &EngineSpec, role: Role) -> Result<Vec<PlanFile>, String> {
    let dep = trino_deployment(spec, role);
    let mounts = dep["spec"]["template"]["spec"]["containers"][0]["volumeMounts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let tls = trino_tls_secret(spec);
    resolve_mounts(
        &mounts,
        &trino_config_map(spec),
        &trino_catalogs_config_map(spec),
        tls.as_ref(),
    )
}

/// Resolve `volumeMounts` entries to the ConfigMap/Secret data behind them.
fn resolve_mounts(
    mounts: &[serde_json::Value],
    config: &serde_json::Value,
    catalogs: &serde_json::Value,
    tls: Option<&serde_json::Value>,
) -> Result<Vec<PlanFile>, String> {
    let mut files = Vec::new();
    for m in mounts {
        let path = m["mountPath"].as_str().unwrap_or_default();
        let sub = m["subPath"].as_str();
        let unsupported = || format!("unsupported volume mount {path}");
        match (m["name"].as_str().unwrap_or_default(), sub) {
            ("config", Some(s)) => {
                let body = config["data"][s].as_str().ok_or_else(unsupported)?;
                files.push(PlanFile {
                    path: path.into(),
                    content: body.as_bytes().to_vec(),
                    mode: file_mode(path),
                });
            }
            ("catalogs", None) => {
                for (file, body) in catalogs["data"].as_object().into_iter().flatten() {
                    files.push(PlanFile {
                        path: format!("{path}/{file}"),
                        content: body.as_str().unwrap_or_default().as_bytes().to_vec(),
                        mode: 0o644,
                    });
                }
            }
            ("tls", Some("keystore.pem")) => {
                let pem = tls
                    .and_then(|t| t["stringData"]["keystore.pem"].as_str())
                    .ok_or_else(unsupported)?;
                files.push(PlanFile {
                    path: path.into(),
                    content: pem.as_bytes().to_vec(),
                    mode: 0o600,
                });
            }
            // The image already ships /data/trino, owned by uid 1000.
            ("data", None) if path == "/data/trino" => {}
            _ => return Err(unsupported()),
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Scheduling and replica fields (coordinator replicas above 1, probes, node
/// selectors, tolerations, affinity, topology spread, termination grace) have no
/// Docker meaning and are intentionally not applied.
pub fn plan_trino(spec: &EngineSpec, opts: &PlanOptions) -> Result<Vec<ContainerPlan>, String> {
    let bad = unsupported(spec);
    if !bad.is_empty() {
        return Err(format!(
            "not supported by the Docker engine provisioner: {}",
            bad.join(", ")
        ));
    }
    let mut env: Vec<(String, String)> = trino_env_secret(spec)["stringData"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
        .collect();
    env.sort();

    let mut plans = Vec::new();
    let roles = std::iter::once(PlanRole::Coordinator)
        .chain((0..spec.config.worker.base.replicas).map(PlanRole::Worker));
    for role in roles {
        let (k8s_role, rc, role_label) = match role {
            PlanRole::Coordinator => (Role::Coordinator, &spec.config.coordinator, "coordinator"),
            PlanRole::Worker(_) => (Role::Worker, &spec.config.worker.base, "worker"),
        };
        let image = trino_deployment(spec, k8s_role)["spec"]["template"]["spec"]["containers"][0]
            ["image"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if !image_allowed(&image, opts.allowed_images) {
            return Err(format!(
                "image {image} is not in the allowed images ({})",
                opts.allowed_images.join(", ")
            ));
        }
        let labels = [
            ("egret.managed", "true"),
            ("egret.project", opts.project),
            ("egret.engine", spec.name.as_str()),
            ("egret.namespace", spec.namespace.as_str()),
            ("egret.role", role_label),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let aliases = match role {
            PlanRole::Coordinator => {
                let host = format!("{}-trino", spec.name);
                vec![
                    host.clone(),
                    format!("{host}.{}", spec.namespace),
                    format!("{host}.{}.svc", spec.namespace),
                ]
            }
            PlanRole::Worker(_) => Vec::new(),
        };
        plans.push(ContainerPlan {
            name: container_name(&spec.namespace, &spec.name, &role),
            files: role_files(spec, k8s_role)?,
            image,
            env: env.clone(),
            labels,
            aliases,
            nano_cpus: rc
                .resources
                .cpu_limit
                .as_deref()
                .map(parse_cpu)
                .transpose()?,
            memory_bytes: rc
                .resources
                .mem_limit
                .as_deref()
                .map(parse_memory)
                .transpose()?,
            role,
        });
    }
    Ok(plans)
}

const CONFIG_ROOT: &str = "etc/trino";

/// A tar of `files` for `PUT /containers/{id}/archive` at `/`: parent
/// directories first, everything owned by uid/gid 1000 (the Trino user).
pub fn files_tar(files: &[PlanFile]) -> Vec<u8> {
    let mut dirs = std::collections::BTreeSet::new();
    for f in files {
        let mut p = std::path::Path::new(f.path.trim_start_matches('/')).parent();
        // Only directories at or below the config root: put_archive applies tar
        // metadata to existing directories, so an `etc/` entry would chown /etc.
        while let Some(d) = p.filter(|d| d.starts_with(CONFIG_ROOT)) {
            dirs.insert(d.to_path_buf());
            p = d.parent();
        }
    }
    let mut b = tar::Builder::new(Vec::new());
    let header = |mode: u32, size: usize, dir: bool| {
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(if dir {
            tar::EntryType::Directory
        } else {
            tar::EntryType::Regular
        });
        h.set_mode(mode);
        h.set_size(size as u64);
        h.set_uid(1000);
        h.set_gid(1000);
        h.set_mtime(0);
        h
    };
    // BTreeSet order puts "etc" before "etc/trino".
    for d in &dirs {
        let mut h = header(0o755, 0, true);
        b.append_data(&mut h, d, std::io::empty())
            .expect("write to Vec");
    }
    for f in files {
        let mut h = header(f.mode, f.content.len(), false);
        b.append_data(&mut h, f.path.trim_start_matches('/'), f.content.as_slice())
            .expect("write to Vec");
    }
    b.into_inner().expect("write to Vec")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::*;
    use std::collections::BTreeSet;
    use std::io::Read;

    fn sample_spec() -> EngineSpec {
        let mut coord = RoleConfig {
            replicas: 1,
            ..Default::default()
        };
        coord.heap.max_size = Some("2G".into());
        coord.query_max_memory_per_node = Some("1GB".into());
        let mut worker = RoleConfig {
            replicas: 0,
            ..Default::default()
        };
        worker.heap.max_percent = Some(80);
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
        cat_env.insert("S3_SECRET_KEY".into(), "s3secret".into());
        EngineSpec {
            name: "demo".into(),
            namespace: "egret-engines".into(),
            image_version: "483".into(),
            config: TrinoConfig {
                query_max_memory: Some("4GB".into()),
                coordinator: coord,
                worker: WorkerRoleConfig {
                    base: worker,
                    graceful_shutdown: None,
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

    fn allowed() -> Vec<String> {
        vec!["trinodb/trino".to_string()]
    }

    fn plan(spec: &EngineSpec) -> Result<Vec<ContainerPlan>, String> {
        let a = allowed();
        plan_trino(
            spec,
            &PlanOptions {
                project: "demo",
                allowed_images: &a,
            },
        )
    }

    fn file<'a>(p: &'a ContainerPlan, path: &str) -> &'a PlanFile {
        p.files
            .iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("no file {path}"))
    }

    fn cm(spec: &EngineSpec, key: &str) -> Vec<u8> {
        trino_config_map(spec)["data"][key]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn cpu_quantities() {
        assert_eq!(parse_cpu("500m").unwrap(), 500_000_000);
        assert_eq!(parse_cpu("2").unwrap(), 2_000_000_000);
        assert_eq!(parse_cpu("0.5").unwrap(), 500_000_000);
        for bad in ["", "abc", "-1", "0", "1x", "m"] {
            assert!(parse_cpu(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn memory_quantities() {
        assert_eq!(parse_memory("2Gi").unwrap(), 2_147_483_648);
        assert_eq!(parse_memory("512Mi").unwrap(), 536_870_912);
        assert_eq!(parse_memory("1G").unwrap(), 1_000_000_000);
        assert_eq!(parse_memory("1048576").unwrap(), 1_048_576);
        for bad in ["", "abc", "-1", "0", "2Xi", "Gi"] {
            assert!(parse_memory(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn coordinator_only_plan_has_names_aliases_labels() {
        let plans = plan(&sample_spec()).unwrap();
        assert_eq!(plans.len(), 1);
        let p = &plans[0];
        assert_eq!(p.name, "egret-engine-egret-engines-demo-coordinator");
        assert_eq!(p.role, PlanRole::Coordinator);
        assert_eq!(p.image, "trinodb/trino:483");
        assert_eq!(
            p.aliases,
            vec![
                "demo-trino".to_string(),
                "demo-trino.egret-engines".to_string(),
                "demo-trino.egret-engines.svc".to_string()
            ]
        );
        let want: BTreeMap<String, String> = [
            ("egret.managed", "true"),
            ("egret.project", "demo"),
            ("egret.engine", "demo"),
            ("egret.namespace", "egret-engines"),
            ("egret.role", "coordinator"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        assert_eq!(p.labels, want);
    }

    #[test]
    fn files_equal_the_kubernetes_renderer_output() {
        let s = sample_spec();
        let p = &plan(&s).unwrap()[0];
        assert_eq!(
            file(p, "/etc/trino/config.properties").content,
            cm(&s, "config-coordinator.properties")
        );
        assert_eq!(
            file(p, "/etc/trino/jvm.config").content,
            cm(&s, "jvm-coordinator.config")
        );
        let cats = trino_catalogs_config_map(&s);
        assert_eq!(
            file(p, "/etc/trino/catalog/egret.properties").content,
            cats["data"]["egret.properties"]
                .as_str()
                .unwrap()
                .as_bytes()
        );
        assert_eq!(file(p, "/etc/trino/config.properties").mode, 0o644);
    }

    fn rich_spec() -> EngineSpec {
        let mut s = sample_spec();
        s.ranger = Some(RangerBinding {
            service_name: "demo".into(),
            egret_url: "http://egret:8080".into(),
            poll_interval_ms: 30_000,
        });
        s.config.auth = Some(Auth {
            password_users: vec![PasswordEntry {
                username: "bob".into(),
                bcrypt_hash: "$2b$10$abc".into(),
            }],
            jwt: None,
        });
        s.config.resource_groups = Some(ResourceGroups::default());
        let mut props = BTreeMap::new();
        props.insert("query_max_memory".to_string(), "10GB".to_string());
        s.config.session_properties = vec![SessionRule {
            match_user: Some("admin".into()),
            match_source: None,
            match_client_tags: vec![],
            properties: props,
        }];
        s.config
            .event_listener
            .insert("event-listener.name".into(), "http".into());
        s.config.fault_tolerant = Some(FaultTolerant {
            retry_policy: "TASK".into(),
            s3_base_dir: "s3://spool/exchange".into(),
            s3_endpoint: "http://rustfs:9000".into(),
            s3_region: "us-east-1".into(),
            s3_path_style: true,
        });
        s
    }

    #[test]
    fn plan_files_match_the_kubernetes_mounts() {
        let s = rich_spec();
        let p = &plan(&s).unwrap()[0];
        let dep = trino_deployment(&s, Role::Coordinator);
        let mut want: BTreeSet<String> = dep["spec"]["template"]["spec"]["containers"][0]
            ["volumeMounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["mountPath"].as_str().unwrap().to_string())
            .filter(|m| m != "/data/trino" && m != "/etc/trino/catalog")
            .collect();
        want.insert("/etc/trino/catalog/egret.properties".into());
        let got: BTreeSet<String> = p.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(got, want);
        assert!(got.contains("/etc/trino/auth/password/password.db"));
        assert!(got.contains("/etc/trino/ranger-trino-security.xml"));
        assert_eq!(file(p, "/etc/trino/auth/password/password.db").mode, 0o600);
        let sorted: Vec<&String> = p.files.iter().map(|f| &f.path).collect();
        let mut again = sorted.clone();
        again.sort();
        assert_eq!(sorted, again);
    }

    #[test]
    fn tls_keystore_is_coordinator_only_and_private() {
        let mut s = sample_spec();
        s.config.tls = Some(Tls {
            keystore_pem: "PEMDATA".into(),
            port: 8443,
        });
        s.config.worker.base.replicas = 1;
        let plans = plan(&s).unwrap();
        let k = file(&plans[0], "/etc/trino/tls/keystore.pem");
        assert_eq!(k.content, b"PEMDATA");
        assert_eq!(k.mode, 0o600);
        assert!(plans[1]
            .files
            .iter()
            .all(|f| f.path != "/etc/trino/tls/keystore.pem"));
    }

    #[test]
    fn worker_plans() {
        let mut s = sample_spec();
        s.config.worker.base.replicas = 2;
        let plans = plan(&s).unwrap();
        assert_eq!(plans.len(), 3);
        assert_eq!(plans[1].name, "egret-engine-egret-engines-demo-worker-0");
        assert_eq!(plans[2].name, "egret-engine-egret-engines-demo-worker-1");
        assert_eq!(plans[2].role, PlanRole::Worker(1));
        assert_eq!(plans[1].labels["egret.role"], "worker");
        assert!(plans[1].aliases.is_empty());
        assert_eq!(
            file(&plans[1], "/etc/trino/config.properties").content,
            cm(&s, "config-worker.properties")
        );
        assert_eq!(
            file(&plans[1], "/etc/trino/jvm.config").content,
            cm(&s, "jvm-worker.config")
        );
    }

    #[test]
    fn env_is_sorted_and_carries_catalog_secrets() {
        let p = &plan(&sample_spec()).unwrap()[0];
        assert!(p
            .env
            .contains(&("S3_SECRET_KEY".to_string(), "s3secret".to_string())));
        let keys: Vec<&String> = p.env.iter().map(|(k, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        let sec = trino_env_secret(&sample_spec());
        assert_eq!(p.env.len(), sec["stringData"].as_object().unwrap().len());
    }

    #[test]
    fn resources_come_from_the_limits() {
        let mut s = sample_spec();
        s.config.coordinator.resources.cpu_limit = Some("500m".into());
        s.config.coordinator.resources.mem_limit = Some("2Gi".into());
        s.config.coordinator.resources.cpu_request = Some("4".into());
        let p = &plan(&s).unwrap()[0];
        assert_eq!(p.nano_cpus, Some(500_000_000));
        assert_eq!(p.memory_bytes, Some(2_147_483_648));
        let none = &plan(&sample_spec()).unwrap()[0];
        assert_eq!((none.nano_cpus, none.memory_bytes), (None, None));
    }

    #[test]
    fn unsupported_options_are_rejected_by_name() {
        let mut s = sample_spec();
        s.config.ingress = Some(Ingress {
            enabled: true,
            ..Default::default()
        });
        let e = plan(&s).unwrap_err();
        assert!(e.contains("ingress"), "{e}");

        let mut s = sample_spec();
        s.config.autoscaling = Some(Autoscaling {
            mode: "hpa".into(),
            hpa: None,
            keda: None,
        });
        assert!(plan(&s).unwrap_err().contains("autoscaling"));

        let mut s = sample_spec();
        s.config.image_pull_secret_json = Some("{}".into());
        assert!(plan(&s).unwrap_err().contains("image_pull_secret"));

        let mut s = sample_spec();
        s.config.coordinator.config_mounts = vec![ObjectMount {
            name: "x".into(),
            mount_path: "/etc/x".into(),
            sub_path: None,
            default_mode: None,
        }];
        assert!(plan(&s).unwrap_err().contains("mounts"));

        let mut s = sample_spec();
        s.kind = EngineKind::StarRocks;
        assert!(plan(&s).unwrap_err().to_lowercase().contains("starrocks"));

        let mut s = sample_spec();
        s.config.autoscaling = Some(Autoscaling {
            mode: "hpa".into(),
            hpa: None,
            keda: None,
        });
        s.config.image_pull_secret_json = Some("{}".into());
        let e = plan(&s).unwrap_err();
        assert!(
            e.contains("autoscaling") && e.contains("image_pull_secret"),
            "{e}"
        );
    }

    #[test]
    fn disallowed_image_is_rejected() {
        let mut s = sample_spec();
        s.config.image = Some("evil/miner:1".into());
        let e = plan(&s).unwrap_err();
        assert!(e.contains("evil/miner:1"), "{e}");
    }

    #[test]
    fn image_allow_list_is_an_exact_repository_match() {
        let a = allowed();
        assert!(image_allowed("trinodb/trino:483", &a));
        assert!(image_allowed("trinodb/trino", &a));
        assert!(image_allowed(
            "trinodb/trino@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            &a
        ));
        assert!(!image_allowed("trinodb/trinoevil:1", &a));
        assert!(!image_allowed("evil/trinodb/trino:1", &a));
        assert!(!image_allowed("docker.io/trinodb/trino:483", &a));
        assert!(image_allowed(
            "localhost:5000/x/trino:1",
            &["localhost:5000/x/trino".into()]
        ));
    }

    #[test]
    fn tar_round_trips_with_uid_gid_1000_and_dirs_first() {
        let files = vec![
            PlanFile {
                path: "/etc/trino/catalog/a.properties".into(),
                content: b"a=1\n".to_vec(),
                mode: 0o644,
            },
            PlanFile {
                path: "/etc/trino/tls/keystore.pem".into(),
                content: b"PEM".to_vec(),
                mode: 0o600,
            },
        ];
        let bytes = files_tar(&files);
        let mut ar = tar::Archive::new(bytes.as_slice());
        let mut seen: Vec<(String, bool, u32, u64, u64, Vec<u8>)> = Vec::new();
        for e in ar.entries().unwrap() {
            let mut e = e.unwrap();
            let path = e.path().unwrap().to_string_lossy().to_string();
            let h = e.header().clone();
            let mut buf = Vec::new();
            e.read_to_end(&mut buf).unwrap();
            seen.push((
                path,
                h.entry_type().is_dir(),
                h.mode().unwrap(),
                h.uid().unwrap(),
                h.gid().unwrap(),
                buf,
            ));
        }
        assert!(seen.iter().all(|e| e.3 == 1000 && e.4 == 1000));
        for f in &files {
            let rel = f.path.trim_start_matches('/');
            let i = seen
                .iter()
                .position(|e| e.0 == rel && !e.1)
                .unwrap_or_else(|| panic!("missing {rel}"));
            assert_eq!(seen[i].2 & 0o777, f.mode);
            assert_eq!(seen[i].5, f.content);
            let mut dir = std::path::Path::new(rel).parent().unwrap();
            while dir.starts_with("etc/trino") {
                let d = dir.to_string_lossy().to_string();
                let di = seen
                    .iter()
                    .position(|e| e.1 && e.0.trim_end_matches('/') == d)
                    .unwrap_or_else(|| panic!("missing dir {d}"));
                assert!(di < i, "dir {d} after file {rel}");
                dir = dir.parent().unwrap();
            }
        }
    }

    #[test]
    fn tar_never_touches_directories_above_the_config_root() {
        let files = vec![PlanFile {
            path: "/etc/trino/catalog/a.properties".into(),
            content: b"a=1\n".to_vec(),
            mode: 0o644,
        }];
        let bytes = files_tar(&files);
        let names: Vec<String> = tar::Archive::new(bytes.as_slice())
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(
            !names.iter().any(|n| n.trim_end_matches('/') == "etc"),
            "{names:?}"
        );
        assert_eq!(
            names
                .iter()
                .map(|n| n.trim_end_matches('/'))
                .collect::<Vec<_>>(),
            vec![
                "etc/trino",
                "etc/trino/catalog",
                "etc/trino/catalog/a.properties"
            ]
        );
        // A path outside the config root gets no directory entries at all.
        let other = files_tar(&[PlanFile {
            path: "/data/x/y".into(),
            content: vec![],
            mode: 0o644,
        }]);
        let n = tar::Archive::new(other.as_slice())
            .entries()
            .unwrap()
            .count();
        assert_eq!(n, 1);
    }

    #[test]
    fn exposed_ports_and_shared_process_namespace_are_rejected() {
        let mut s = sample_spec();
        s.config.worker.base.additional_exposed_ports = vec![ContainerPort {
            name: "x".into(),
            container_port: 9000,
        }];
        let e = plan(&s).unwrap_err();
        assert!(e.contains("exposed ports"), "{e}");

        let mut s = sample_spec();
        s.config.coordinator.share_process_namespace = Some(true);
        s.config.autoscaling = Some(Autoscaling {
            mode: "hpa".into(),
            hpa: None,
            keda: None,
        });
        let e = plan(&s).unwrap_err();
        assert!(
            e.contains("shared process namespace") && e.contains("autoscaling"),
            "{e}"
        );

        let mut s = sample_spec();
        s.config.coordinator.share_process_namespace = Some(false);
        assert!(plan(&s).is_ok());
    }

    #[test]
    fn cpu_rounding_to_zero_or_overflow_is_rejected() {
        assert!(parse_cpu("0.0000004m").is_err());
        assert!(parse_cpu("1e30").is_err());
        assert!(parse_cpu("99999999999").is_err());
        assert_eq!(parse_cpu("1m").unwrap(), 1_000_000);
    }

    #[test]
    fn unknown_or_misplaced_volumes_are_unsupported_mounts() {
        let none = serde_json::json!({});
        let m = |name: &str, path: &str, sub: Option<&str>| {
            let mut v = serde_json::json!({ "name": name, "mountPath": path });
            if let Some(s) = sub {
                v["subPath"] = s.into();
            }
            v
        };
        for bad in [
            m("mystery", "/etc/trino/x", None),
            m("data", "/somewhere/else", None),
            m("config", "/etc/trino/x", Some("not-a-key")),
            m("tls", "/etc/trino/tls/keystore.pem", Some("keystore.pem")),
        ] {
            let e = resolve_mounts(&[bad], &none, &none, None).unwrap_err();
            assert!(e.contains("unsupported volume mount"), "{e}");
        }
        assert!(
            resolve_mounts(&[m("data", "/data/trino", None)], &none, &none, None)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn worker_files_match_the_kubernetes_mounts() {
        let mut s = rich_spec();
        s.config.worker.base.replicas = 1;
        let plans = plan(&s).unwrap();
        let dep = trino_deployment(&s, Role::Worker);
        let mut want: BTreeSet<String> = dep["spec"]["template"]["spec"]["containers"][0]
            ["volumeMounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["mountPath"].as_str().unwrap().to_string())
            .filter(|m| m != "/data/trino" && m != "/etc/trino/catalog")
            .collect();
        want.insert("/etc/trino/catalog/egret.properties".into());
        let got: BTreeSet<String> = plans[1].files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn shared_namespace_alone_and_exposed_ports_combined_are_named() {
        let mut s = sample_spec();
        s.config.coordinator.share_process_namespace = Some(true);
        let e = plan(&s).unwrap_err();
        assert!(e.contains("shared process namespace"), "{e}");

        let mut s = sample_spec();
        s.config.worker.base.additional_exposed_ports = vec![ContainerPort {
            name: "x".into(),
            container_port: 9000,
        }];
        s.config.ingress = Some(Ingress {
            enabled: true,
            ..Default::default()
        });
        let e = plan(&s).unwrap_err();
        assert!(e.contains("exposed ports") && e.contains("ingress"), "{e}");
    }
}
