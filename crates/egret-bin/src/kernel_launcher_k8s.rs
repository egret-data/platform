//! `KubernetesLauncher` — one kernel Pod per (user×workspace), addressed by Pod IP.
//! Implements `egret_kernel::KernelLauncher`. Reference: sparklabx k8s_per_user_gateway.
//!
//! Wired into `main.rs`'s launcher-selection switch (Task A2): constructed
//! when `kernel.launcher = "kubernetes"`.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use k8s_openapi::api::core::v1::{
    Capabilities, Container, ContainerPort, EnvVar, LocalObjectReference, Pod, PodSecurityContext,
    PodSpec, Probe, ResourceRequirements, SeccompProfile, SecurityContext, TCPSocketAction,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, DeleteParams, PostParams};
use sha1::{Digest, Sha1};

use egret_kernel::error::KernelError;
use egret_kernel::launcher::{KernelEnv, KernelHandle, KernelLauncher, KernelPhase};
use egret_kernel::registry::KernelKey;

const GATEWAY_PORT: i32 = 8888;

#[derive(Clone, Debug)]
pub struct K8sKernelConfig {
    pub namespace: String,
    pub image: String,
    pub image_pull_policy: String,
    pub cpu_request: String,
    pub mem_request: String,
    pub cpu_limit: String,
    pub mem_limit: String,
    pub pull_secret: Option<String>,
    /// Inject EGRET_ENGINE_TLS_INSECURE=1 so kernel `trino()`/`starrocks()` accept
    /// the engine's self-signed TLS cert. Dev/self-signed deploys only.
    pub engine_tls_insecure: bool,
}

pub struct KubernetesLauncher {
    client: kube::Client,
    cfg: K8sKernelConfig,
}

impl KubernetesLauncher {
    pub fn new(client: kube::Client, cfg: K8sKernelConfig) -> Self {
        Self { client, cfg }
    }
    fn pods(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.cfg.namespace)
    }
}

/// Deterministic, DNS-safe pod name: `kernel-<12 hex>` from sha1(user||ws).
pub fn pod_name(key: &KernelKey) -> String {
    let mut h = Sha1::new();
    h.update(key.user_id.as_bytes());
    h.update(key.workspace_id.as_bytes());
    let d = h.finalize();
    format!("kernel-{}", hex::encode(&d[..6]))
}

fn label_short(id: &uuid::Uuid) -> String {
    id.simple().to_string()[..8].to_string()
}

/// Terminal container waiting reasons → Dead; transient (ContainerCreating) → None.
pub fn phase_of_waiting_reason(reason: &str) -> Option<KernelPhase> {
    match reason {
        "ImagePullBackOff"
        | "ErrImagePull"
        | "CrashLoopBackOff"
        | "CreateContainerConfigError"
        | "CreateContainerError" => Some(KernelPhase::Dead),
        _ => None,
    }
}

/// Build the hardened Pod object for a kernel. Pure fn (unit-tested, no cluster).
pub fn build_pod_spec(key: &KernelKey, env: &KernelEnv, cfg: &K8sKernelConfig) -> Pod {
    let mut labels = BTreeMap::new();
    labels.insert("app".to_string(), "egret-kernel".to_string());
    labels.insert("managed-by".to_string(), "egret".to_string());
    labels.insert("egret-user".to_string(), label_short(&key.user_id));
    labels.insert(
        "egret-workspace".to_string(),
        label_short(&key.workspace_id),
    );

    // env passthrough + the port the entrypoint binds.
    let mut env_vars: Vec<EnvVar> = env
        .iter()
        .map(|(k, v)| EnvVar {
            name: k.clone(),
            value: Some(v.clone()),
            value_from: None,
        })
        .collect();
    env_vars.push(EnvVar {
        name: "KERNEL_GATEWAY_PORT".to_string(),
        value: Some(GATEWAY_PORT.to_string()),
        value_from: None,
    });
    if cfg.engine_tls_insecure {
        env_vars.push(EnvVar {
            name: "EGRET_ENGINE_TLS_INSECURE".to_string(),
            value: Some("1".to_string()),
            value_from: None,
        });
    }
    env_vars.sort_by(|a, b| a.name.cmp(&b.name)); // deterministic for tests

    let resources = ResourceRequirements {
        requests: Some(BTreeMap::from([
            ("cpu".to_string(), Quantity(cfg.cpu_request.clone())),
            ("memory".to_string(), Quantity(cfg.mem_request.clone())),
        ])),
        limits: Some(BTreeMap::from([
            ("cpu".to_string(), Quantity(cfg.cpu_limit.clone())),
            ("memory".to_string(), Quantity(cfg.mem_limit.clone())),
        ])),
        claims: None,
    };

    let container = Container {
        name: "egret-kernel".to_string(),
        image: Some(cfg.image.clone()),
        image_pull_policy: Some(cfg.image_pull_policy.clone()),
        ports: Some(vec![ContainerPort {
            container_port: GATEWAY_PORT,
            protocol: Some("TCP".to_string()),
            ..Default::default()
        }]),
        env: Some(env_vars),
        resources: Some(resources),
        readiness_probe: Some(Probe {
            tcp_socket: Some(TCPSocketAction {
                port: IntOrString::Int(GATEWAY_PORT),
                host: None,
            }),
            initial_delay_seconds: Some(5),
            period_seconds: Some(3),
            ..Default::default()
        }),
        security_context: Some(SecurityContext {
            allow_privilege_escalation: Some(false),
            capabilities: Some(Capabilities {
                add: None,
                drop: Some(vec!["ALL".to_string()]),
            }),
            ..Default::default()
        }),
        ..Default::default()
    };

    let spec = PodSpec {
        restart_policy: Some("OnFailure".to_string()),
        termination_grace_period_seconds: Some(5),
        image_pull_secrets: cfg
            .pull_secret
            .as_ref()
            .map(|n| vec![LocalObjectReference { name: n.clone() }]),
        security_context: Some(PodSecurityContext {
            run_as_non_root: Some(true),
            run_as_user: Some(1000),
            fs_group: Some(1000),
            seccomp_profile: Some(SeccompProfile {
                type_: "RuntimeDefault".to_string(),
                localhost_profile: None,
            }),
            ..Default::default()
        }),
        containers: vec![container],
        ..Default::default()
    };

    Pod {
        metadata: ObjectMeta {
            name: Some(pod_name(key)),
            namespace: Some(cfg.namespace.clone()),
            labels: Some(labels),
            ..Default::default()
        },
        spec: Some(spec),
        status: None,
    }
}

/// Read Ready condition / terminal states from a live Pod.
fn pod_phase(p: &Pod) -> KernelPhase {
    if p.metadata.deletion_timestamp.is_some() {
        return KernelPhase::Dead;
    }
    let status = match &p.status {
        Some(s) => s,
        None => return KernelPhase::Spawning,
    };
    if let Some(css) = &status.container_statuses {
        for cs in css {
            if let Some(w) = cs.state.as_ref().and_then(|s| s.waiting.as_ref()) {
                if let Some(reason) = &w.reason {
                    if let Some(ph) = phase_of_waiting_reason(reason) {
                        return ph;
                    }
                }
            }
            if cs.ready {
                return KernelPhase::Ready;
            }
        }
    }
    if status.phase.as_deref() == Some("Failed") {
        return KernelPhase::Dead;
    }
    KernelPhase::Spawning
}

fn ready_url(p: &Pod) -> Option<String> {
    let ip = p.status.as_ref()?.pod_ip.as_ref()?;
    if pod_phase(p) == KernelPhase::Ready {
        Some(format!("http://{ip}:{GATEWAY_PORT}"))
    } else {
        None
    }
}

#[async_trait]
impl KernelLauncher for KubernetesLauncher {
    async fn ensure(
        &self,
        key: KernelKey,
        env: &KernelEnv,
        timeout: Duration,
    ) -> Result<KernelHandle, KernelError> {
        let pods = self.pods();
        let name = pod_name(&key);

        // Fast path: existing ready pod.
        if let Ok(p) = pods.get(&name).await {
            if let Some(url) = ready_url(&p) {
                return Ok(KernelHandle {
                    url,
                    phase: KernelPhase::Ready,
                    pid: None,
                });
            }
        }

        // Create (idempotent).
        let pod = build_pod_spec(&key, env, &self.cfg);
        match pods.create(&PostParams::default(), &pod).await {
            Ok(_) => {}
            Err(kube::Error::Api(ae)) if ae.code == 409 => { /* AlreadyExists → reuse */ }
            Err(e) => return Err(KernelError::Spawn(format!("pod create: {e}"))),
        }

        // Poll until Ready or terminal or timeout. Evaluate readiness FIRST each
        // iteration so a pod that turns Ready in the last polling window isn't
        // deleted by the deadline check before it's ever inspected.
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            match pods.get(&name).await {
                Ok(p) => {
                    if let Some(url) = ready_url(&p) {
                        return Ok(KernelHandle {
                            url,
                            phase: KernelPhase::Ready,
                            pid: None,
                        });
                    }
                    if pod_phase(&p) == KernelPhase::Dead {
                        let _ = pods.delete(&name, &DeleteParams::default()).await;
                        return Err(KernelError::Spawn(format!(
                            "kernel pod {name} failed to start"
                        )));
                    }
                }
                Err(kube::Error::Api(ae)) if ae.code == 404 => { /* not yet visible */ }
                Err(e) => return Err(KernelError::Spawn(format!("pod get: {e}"))),
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = pods.delete(&name, &DeleteParams::default()).await;
                return Err(KernelError::Spawn(format!(
                    "kernel pod {name} not ready within {timeout:?}"
                )));
            }
        }
    }

    async fn status(&self, key: KernelKey) -> Result<KernelPhase, KernelError> {
        match self.pods().get(&pod_name(&key)).await {
            Ok(p) => Ok(pod_phase(&p)),
            Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(KernelPhase::Dead),
            Err(e) => Err(KernelError::Spawn(format!("pod status: {e}"))),
        }
    }

    async fn stop(&self, key: KernelKey) -> Result<(), KernelError> {
        match self
            .pods()
            .delete(&pod_name(&key), &DeleteParams::default())
            .await
        {
            Ok(_) => Ok(()),
            Err(kube::Error::Api(ae)) if ae.code == 404 => Ok(()),
            Err(e) => Err(KernelError::Spawn(format!("pod delete: {e}"))),
        }
    }

    fn mode(&self) -> &'static str {
        "kubernetes"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_kernel::launcher::KernelEnv;
    use egret_kernel::registry::KernelKey;
    use uuid::Uuid;

    fn cfg() -> K8sKernelConfig {
        K8sKernelConfig {
            namespace: "egret-notebooks".into(),
            image: "egret-kernel:dev".into(),
            image_pull_policy: "IfNotPresent".into(),
            cpu_request: "250m".into(),
            mem_request: "512Mi".into(),
            cpu_limit: "1".into(),
            mem_limit: "2Gi".into(),
            pull_secret: None,
            engine_tls_insecure: false,
        }
    }

    #[test]
    fn pod_name_is_deterministic_and_dns_safe() {
        let k = KernelKey {
            user_id: Uuid::nil(),
            workspace_id: Uuid::nil(),
        };
        let a = pod_name(&k);
        let b = pod_name(&k);
        assert_eq!(a, b);
        assert!(a.starts_with("kernel-"));
        assert!(
            a.len() <= 63
                && a.bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        );
    }

    #[test]
    fn pod_spec_is_hardened_and_carries_env() {
        let k = KernelKey {
            user_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
        };
        let env: KernelEnv =
            KernelEnv::from([("EGRET_API_URL".into(), "http://egret:8080".into())]);
        let pod = build_pod_spec(&k, &env, &cfg());
        let spec = pod.spec.unwrap();
        // non-root pod security
        let psc = spec.security_context.unwrap();
        assert_eq!(psc.run_as_non_root, Some(true));
        assert_eq!(psc.run_as_user, Some(1000));
        // single container, hardened
        let c = &spec.containers[0];
        assert_eq!(c.image.as_deref(), Some("egret-kernel:dev"));
        let csc = c.security_context.clone().unwrap();
        assert_eq!(csc.allow_privilege_escalation, Some(false));
        assert_eq!(
            csc.capabilities.unwrap().drop.unwrap(),
            vec!["ALL".to_string()]
        );
        // seccomp + fsGroup
        assert_eq!(psc.fs_group, Some(1000));
        assert_eq!(psc.seccomp_profile.unwrap().type_, "RuntimeDefault");
        // image pull policy + resources from cfg
        assert_eq!(c.image_pull_policy.as_deref(), Some("IfNotPresent"));
        let res = c.resources.clone().unwrap();
        assert_eq!(res.requests.as_ref().unwrap().get("cpu").unwrap().0, "250m");
        assert_eq!(res.limits.as_ref().unwrap().get("memory").unwrap().0, "2Gi");
        // env passthrough + injected port
        let names: Vec<_> = c.env.clone().unwrap().into_iter().map(|e| e.name).collect();
        assert!(names.contains(&"EGRET_API_URL".to_string()));
        assert!(names.contains(&"KERNEL_GATEWAY_PORT".to_string()));
        // port + readiness probe
        assert_eq!(c.ports.clone().unwrap()[0].container_port, 8888);
        assert!(c.readiness_probe.is_some());
        // restart policy + grace
        assert_eq!(spec.restart_policy.as_deref(), Some("OnFailure"));
        assert_eq!(spec.termination_grace_period_seconds, Some(5));
        // labels
        let labels = pod.metadata.labels.unwrap();
        assert_eq!(labels.get("managed-by").map(String::as_str), Some("egret"));
    }

    #[test]
    fn engine_tls_insecure_injects_env_only_when_enabled() {
        let k = KernelKey {
            user_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
        };
        let env: KernelEnv = KernelEnv::new();

        // disabled (default): flag absent
        let pod = build_pod_spec(&k, &env, &cfg());
        let c = &pod.spec.unwrap().containers[0];
        let names: Vec<_> = c.env.clone().unwrap().into_iter().map(|e| e.name).collect();
        assert!(!names.contains(&"EGRET_ENGINE_TLS_INSECURE".to_string()));

        // enabled: EGRET_ENGINE_TLS_INSECURE=1 present
        let mut insecure = cfg();
        insecure.engine_tls_insecure = true;
        let pod = build_pod_spec(&k, &env, &insecure);
        let c = &pod.spec.unwrap().containers[0];
        let var = c
            .env
            .clone()
            .unwrap()
            .into_iter()
            .find(|e| e.name == "EGRET_ENGINE_TLS_INSECURE")
            .expect("EGRET_ENGINE_TLS_INSECURE should be injected");
        assert_eq!(var.value.as_deref(), Some("1"));
    }

    #[test]
    fn phase_of_maps_pod_status() {
        assert_eq!(
            phase_of_waiting_reason("ImagePullBackOff"),
            Some(KernelPhase::Dead)
        );
        assert_eq!(
            phase_of_waiting_reason("CrashLoopBackOff"),
            Some(KernelPhase::Dead)
        );
        assert_eq!(phase_of_waiting_reason("ContainerCreating"), None);
    }
}
