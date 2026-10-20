//! `DockerApi` over a real daemon, through bollard.
//!
//! Containers are created without ports, mounts or privileges; the only way a
//! file reaches one is the tar `files_tar` builds, uploaded between create and
//! start. If anything after create fails the container is force-removed, so an
//! `Err` from `create_and_start` leaves nothing behind. A failure of create
//! itself (for instance a name conflict) removes nothing: that container is not ours.

use super::api::{ContainerInfo, ContainerState, DockerApi};
use super::plan::{files_tar, ContainerPlan};
use bollard::body_full;
use bollard::errors::Error as BollardError;
use bollard::models::{
    ContainerCreateBody, ContainerInspectResponse, ContainerStateStatusEnum, EndpointSettings,
    HealthConfig, HealthStatusEnum, HostConfig, NetworkingConfig, RestartPolicy,
    RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, ListContainersOptionsBuilder,
    LogsOptionsBuilder, RemoveContainerOptionsBuilder, RestartContainerOptionsBuilder,
    StopContainerOptionsBuilder, UploadToContainerOptionsBuilder,
};
use bollard::{Docker, API_DEFAULT_VERSION};
use bytes::Bytes;
use futures_util::StreamExt;
use std::collections::{BTreeMap, HashMap};

/// Seconds. Image pulls are one long request, so this must outlast the slowest.
const CLIENT_TIMEOUT_SECS: u64 = 900;
const STOP_TIMEOUT_SECS: i32 = 30;
const SECOND_NS: i64 = 1_000_000_000;

pub struct BollardDocker {
    docker: Docker,
}

impl BollardDocker {
    /// `unix:///path` through the unix socket, `tcp://host:port` (or `http://`)
    /// over plain HTTP. TLS endpoints are not supported.
    pub fn connect(host: &str) -> Result<Self, String> {
        let docker = if host.starts_with("unix://") {
            Docker::connect_with_unix(host, CLIENT_TIMEOUT_SECS, API_DEFAULT_VERSION)
        } else if host.starts_with("tcp://") || host.starts_with("http://") {
            Docker::connect_with_http(host, CLIENT_TIMEOUT_SECS, API_DEFAULT_VERSION)
        } else {
            return Err(format!(
                "unsupported docker host {host:?}: use unix:///path or tcp://host:port"
            ));
        }
        .map_err(|e| format!("connecting to docker at {host}: {e}"))?;
        Ok(Self { docker })
    }

    /// Round-trips to the daemon; used once at startup so an unreachable
    /// daemon fails the boot instead of the first "Add engine".
    pub async fn ping(&self) -> Result<(), String> {
        self.docker
            .ping()
            .await
            .map(|_| ())
            .map_err(|e| format!("pinging the docker daemon: {e}"))
    }

    async fn force_remove(&self, name: &str) {
        let opts = RemoveContainerOptionsBuilder::default().force(true).build();
        if let Err(e) = self.docker.remove_container(name, Some(opts)).await {
            if !is_not_found(&e) {
                tracing::warn!(container = %name, error = %e, "removing a half-created engine container failed");
            }
        }
    }
}

fn is_not_found(e: &BollardError) -> bool {
    matches!(
        e,
        BollardError::DockerResponseServerError {
            status_code: 404,
            ..
        }
    )
}

fn state_of(inspect: &ContainerInspectResponse) -> ContainerState {
    let Some(s) = &inspect.state else {
        return ContainerState::Created;
    };
    let health = s.health.as_ref().and_then(|h| h.status);
    match s.status {
        Some(ContainerStateStatusEnum::RUNNING) => ContainerState::Running {
            healthy: match health {
                Some(HealthStatusEnum::HEALTHY) => Some(true),
                Some(HealthStatusEnum::STARTING) | Some(HealthStatusEnum::UNHEALTHY) => Some(false),
                _ => None,
            },
        },
        // Paused and stopping containers are not serving.
        Some(ContainerStateStatusEnum::PAUSED) | Some(ContainerStateStatusEnum::STOPPING) => {
            ContainerState::Running {
                healthy: Some(false),
            }
        }
        Some(ContainerStateStatusEnum::RESTARTING) => ContainerState::Restarting,
        Some(ContainerStateStatusEnum::EXITED) => ContainerState::Exited {
            code: s.exit_code.unwrap_or(-1),
        },
        Some(ContainerStateStatusEnum::DEAD) | Some(ContainerStateStatusEnum::REMOVING) => {
            ContainerState::Dead
        }
        Some(ContainerStateStatusEnum::CREATED) | Some(ContainerStateStatusEnum::EMPTY) | None => {
            ContainerState::Created
        }
    }
}

fn create_body(plan: &ContainerPlan, network: &str) -> ContainerCreateBody {
    ContainerCreateBody {
        image: Some(plan.image.clone()),
        env: Some(plan.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        labels: Some(plan.labels.clone().into_iter().collect()),
        healthcheck: Some(HealthConfig {
            test: Some(vec![
                "CMD-SHELL".into(),
                "/usr/lib/trino/bin/health-check".into(),
            ]),
            interval: Some(5 * SECOND_NS),
            timeout: Some(5 * SECOND_NS),
            retries: Some(30),
            start_period: Some(20 * SECOND_NS),
            ..Default::default()
        }),
        host_config: Some(HostConfig {
            cap_drop: Some(vec!["ALL".into()]),
            security_opt: Some(vec!["no-new-privileges".into()]),
            nano_cpus: plan.nano_cpus,
            memory: plan.memory_bytes,
            network_mode: Some(network.to_string()),
            restart_policy: Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::NO),
                maximum_retry_count: None,
            }),
            ..Default::default()
        }),
        networking_config: Some(NetworkingConfig {
            endpoints_config: Some(HashMap::from([(
                network.to_string(),
                EndpointSettings {
                    aliases: Some(plan.aliases.clone()),
                    ..Default::default()
                },
            )])),
        }),
        ..Default::default()
    }
}

#[async_trait::async_trait]
impl DockerApi for BollardDocker {
    async fn ensure_image(&self, image: &str) -> Result<(), String> {
        match self.docker.inspect_image(image).await {
            Ok(_) => return Ok(()),
            Err(e) if is_not_found(&e) => {}
            Err(e) => return Err(format!("inspecting image {image}: {e}")),
        }
        let opts = CreateImageOptionsBuilder::default()
            .from_image(image)
            .build();
        let mut pull = std::pin::pin!(self.docker.create_image(Some(opts), None, None));
        while let Some(item) = pull.next().await {
            let info = item.map_err(|e| format!("pulling image {image}: {e}"))?;
            if let Some(d) = info.error_detail {
                return Err(format!(
                    "pulling image {image}: {}",
                    d.message.unwrap_or_default()
                ));
            }
        }
        Ok(())
    }

    async fn create_and_start(&self, plan: &ContainerPlan, network: &str) -> Result<(), String> {
        let opts = CreateContainerOptionsBuilder::default()
            .name(&plan.name)
            .build();
        self.docker
            .create_container(Some(opts), create_body(plan, network))
            .await
            .map_err(|e| format!("creating container {}: {e}", plan.name))?;

        let rest = async {
            let up = UploadToContainerOptionsBuilder::default().path("/").build();
            self.docker
                .upload_to_container(
                    &plan.name,
                    Some(up),
                    body_full(Bytes::from(files_tar(&plan.files))),
                )
                .await
                .map_err(|e| format!("uploading files to {}: {e}", plan.name))?;
            self.docker
                .start_container(&plan.name, None)
                .await
                .map_err(|e| format!("starting container {}: {e}", plan.name))
        }
        .await;
        if rest.is_err() {
            self.force_remove(&plan.name).await;
        }
        rest
    }

    async fn list(
        &self,
        label_filter: &BTreeMap<String, String>,
    ) -> Result<Vec<ContainerInfo>, String> {
        let filters: HashMap<String, Vec<String>> = HashMap::from([(
            "label".to_string(),
            label_filter
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect(),
        )]);
        let opts = ListContainersOptionsBuilder::default()
            .all(true)
            .filters(&filters)
            .build();
        let summaries = self
            .docker
            .list_containers(Some(opts))
            .await
            .map_err(|e| format!("listing containers: {e}"))?;
        let mut out = Vec::new();
        for s in summaries {
            let Some(name) = s
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
            else {
                continue;
            };
            // The summary has no exit code or health; inspect has both.
            let inspect = match self.docker.inspect_container(&name, None).await {
                Ok(i) => i,
                // Removed between list and inspect.
                Err(e) if is_not_found(&e) => continue,
                Err(e) => return Err(format!("inspecting container {name}: {e}")),
            };
            out.push(ContainerInfo {
                state: state_of(&inspect),
                labels: s.labels.unwrap_or_default().into_iter().collect(),
                name,
            });
        }
        Ok(out)
    }

    async fn restart(&self, name: &str) -> Result<(), String> {
        let opts = RestartContainerOptionsBuilder::default()
            .t(STOP_TIMEOUT_SECS)
            .build();
        self.docker
            .restart_container(name, Some(opts))
            .await
            .map_err(|e| format!("restarting container {name}: {e}"))
    }

    async fn stop_and_remove(&self, name: &str) -> Result<(), String> {
        let stop = StopContainerOptionsBuilder::default()
            .t(STOP_TIMEOUT_SECS)
            .build();
        match self.docker.stop_container(name, Some(stop)).await {
            Ok(()) => {}
            Err(e) if is_not_found(&e) => return Ok(()),
            Err(e) => return Err(format!("stopping container {name}: {e}")),
        }
        let rm = RemoveContainerOptionsBuilder::default().force(true).build();
        match self.docker.remove_container(name, Some(rm)).await {
            Ok(()) => Ok(()),
            Err(e) if is_not_found(&e) => Ok(()),
            Err(e) => Err(format!("removing container {name}: {e}")),
        }
    }

    async fn log_tail(&self, name: &str, lines: usize) -> Result<String, String> {
        let opts = LogsOptionsBuilder::default()
            .stdout(true)
            .stderr(true)
            .tail(&lines.to_string())
            .build();
        let mut stream = std::pin::pin!(self.docker.logs(name, Some(opts)));
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("reading logs of {name}: {e}"))?;
            buf.extend_from_slice(&chunk.into_bytes());
        }
        Ok(String::from_utf8_lossy(&buf).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::plan::PlanRole;

    fn plan() -> ContainerPlan {
        ContainerPlan {
            name: "c".into(),
            role: PlanRole::Coordinator,
            image: "trinodb/trino:483".into(),
            env: vec![("A".into(), "b=c".into())],
            labels: [("egret.managed".to_string(), "true".to_string())].into(),
            aliases: vec!["e-trino".into()],
            files: vec![],
            nano_cpus: Some(1_000_000_000),
            memory_bytes: Some(1 << 30),
        }
    }

    #[test]
    fn create_body_is_locked_down() {
        let b = create_body(&plan(), "net");
        let h = b.host_config.unwrap();
        assert_eq!(h.cap_drop, Some(vec!["ALL".to_string()]));
        assert_eq!(h.security_opt, Some(vec!["no-new-privileges".to_string()]));
        assert_eq!(h.network_mode.as_deref(), Some("net"));
        assert_eq!(h.nano_cpus, Some(1_000_000_000));
        assert_eq!(h.memory, Some(1 << 30));
        assert_eq!(h.privileged, None);
        assert_eq!(h.binds, None);
        assert_eq!(h.port_bindings, None);
        assert_eq!(
            h.restart_policy.unwrap().name,
            Some(RestartPolicyNameEnum::NO)
        );
        assert_eq!(b.exposed_ports, None);
        assert_eq!(b.volumes, None);
        assert_eq!(b.env, Some(vec!["A=b=c".to_string()]));
        let aliases = b.networking_config.unwrap().endpoints_config.unwrap()["net"]
            .aliases
            .clone();
        assert_eq!(aliases, Some(vec!["e-trino".to_string()]));
        let hc = b.healthcheck.unwrap();
        assert_eq!(hc.interval, Some(5_000_000_000));
        assert_eq!(hc.retries, Some(30));
    }

    #[test]
    fn connect_rejects_unknown_schemes() {
        assert!(BollardDocker::connect("ssh://x")
            .err()
            .unwrap()
            .contains("unsupported"));
        assert!(BollardDocker::connect("tcp://127.0.0.1:2375").is_ok());
    }
}
