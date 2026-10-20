//! `EngineProvisioningPort` over Docker containers (Trino only).
//!
//! Every container the provisioner acts on is re-checked for our labels, even
//! though the daemon is asked to filter: removing or restarting a container that
//! is not ours (another project, or not egret's at all) is the one mistake this
//! type must not be able to make.

use super::api::{ContainerInfo, ContainerState, DockerApi};
use super::plan::{plan_trino, ContainerPlan, PlanOptions};
use crate::spec::{EngineKind, EngineSpec, EngineStatus, PodStatus};
use egret_engine_mgmt_api::EngineProvisioningPort;
use std::collections::BTreeMap;
use std::sync::Arc;

const LOG_LINES: usize = 20;

pub struct DockerEngineProvisioner {
    api: Arc<dyn DockerApi>,
    network: String,
    project: String,
    allowed_images: Vec<String>,
}

impl DockerEngineProvisioner {
    pub fn new(
        api: Arc<dyn DockerApi>,
        network: String,
        project: String,
        allowed_images: Vec<String>,
    ) -> Self {
        Self {
            api,
            network,
            project,
            allowed_images,
        }
    }

    fn require_trino(kind: &str) -> Result<(), String> {
        if kind == EngineKind::Trino.as_str() {
            Ok(())
        } else {
            Err("the Docker engine provisioner supports Trino only".into())
        }
    }

    fn plan(&self, spec: &EngineSpec) -> Result<Vec<ContainerPlan>, String> {
        if spec.kind != EngineKind::Trino {
            return Err("the Docker engine provisioner supports Trino only".into());
        }
        plan_trino(
            spec,
            &PlanOptions {
                project: &self.project,
                allowed_images: &self.allowed_images,
            },
        )
    }

    fn filter(&self, namespace: &str, name: &str) -> BTreeMap<String, String> {
        [
            ("egret.managed", "true"),
            ("egret.project", self.project.as_str()),
            ("egret.engine", name),
            ("egret.namespace", namespace),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    /// This engine's containers, coordinator first. Anything the daemon returns
    /// that does not carry the full filter is dropped.
    async fn engine_containers(
        &self,
        namespace: &str,
        name: &str,
    ) -> Result<Vec<ContainerInfo>, String> {
        let filter = self.filter(namespace, name);
        let mut found: Vec<ContainerInfo> = self
            .api
            .list(&filter)
            .await?
            .into_iter()
            .filter(|c| filter.iter().all(|(k, v)| c.labels.get(k) == Some(v)))
            .collect();
        found.sort_by_key(|c| {
            let coordinator = c.labels.get("egret.role").map(String::as_str) == Some("coordinator");
            (!coordinator, c.name.clone())
        });
        Ok(found)
    }

    async fn remove_all(&self, names: impl IntoIterator<Item = String>) -> Result<(), String> {
        for n in names {
            self.api.stop_and_remove(&n).await?;
        }
        Ok(())
    }

    /// Pull images, then create the plans in order. On failure everything this
    /// call created is removed again.
    async fn create_all(&self, plans: &[ContainerPlan]) -> Result<(), String> {
        self.ensure_images(plans).await?;
        self.start_all(plans).await
    }

    async fn ensure_images(&self, plans: &[ContainerPlan]) -> Result<(), String> {
        let mut seen: Vec<&str> = Vec::new();
        for p in plans {
            if !seen.contains(&p.image.as_str()) {
                seen.push(&p.image);
                self.api.ensure_image(&p.image).await?;
            }
        }
        Ok(())
    }

    async fn start_all(&self, plans: &[ContainerPlan]) -> Result<(), String> {
        let mut created: Vec<String> = Vec::new();
        for p in plans {
            if let Err(e) = self.api.create_and_start(p, &self.network).await {
                for n in created.iter().rev() {
                    if let Err(re) = self.api.stop_and_remove(n).await {
                        tracing::warn!(container = %n, error = %re, "rollback of engine container failed");
                    }
                }
                return Err(e);
            }
            created.push(p.name.clone());
        }
        Ok(())
    }
}

fn pod_of(c: &ContainerInfo) -> PodStatus {
    let (phase, ready) = match &c.state {
        ContainerState::Running { healthy } => ("Running", *healthy != Some(false)),
        ContainerState::Created | ContainerState::Restarting => ("Pending", false),
        ContainerState::Exited { code: 0 } => ("Succeeded", false),
        ContainerState::Exited { .. } | ContainerState::Dead => ("Failed", false),
    };
    PodStatus {
        name: c.name.clone(),
        ready,
        phase: phase.into(),
    }
}

#[async_trait::async_trait]
impl EngineProvisioningPort for DockerEngineProvisioner {
    async fn provision(&self, spec: &EngineSpec) -> Result<(), String> {
        let plans = self.plan(spec)?;
        if !self
            .engine_containers(&spec.namespace, &spec.name)
            .await?
            .is_empty()
        {
            return Err(format!("engine {} already has containers", spec.name));
        }
        self.create_all(&plans).await
    }

    async fn status(
        &self,
        kind: &str,
        namespace: &str,
        name: &str,
    ) -> Result<EngineStatus, String> {
        Self::require_trino(kind)?;
        let containers = self.engine_containers(namespace, name).await?;
        let pods: Vec<PodStatus> = containers.iter().map(pod_of).collect();
        let failed = containers.iter().find(|c| {
            matches!(
                c.state,
                ContainerState::Exited { .. } | ContainerState::Dead
            )
        });
        if let Some(c) = failed {
            let message = self.api.log_tail(&c.name, LOG_LINES).await?;
            return Ok(EngineStatus {
                phase: "failed".into(),
                message: Some(message),
                pods,
            });
        }
        let running = !pods.is_empty() && pods.iter().all(|p| p.phase == "Running" && p.ready);
        Ok(EngineStatus {
            phase: if running { "running" } else { "provisioning" }.into(),
            message: None,
            pods,
        })
    }

    async fn update(&self, spec: &EngineSpec) -> Result<(), String> {
        // Validate and plan before touching anything: a bad spec must not cost
        // the running engine. Pulling first keeps a registry outage from doing so.
        let plans = self.plan(spec)?;
        self.ensure_images(&plans).await?;
        let current = self.engine_containers(&spec.namespace, &spec.name).await?;
        self.remove_all(current.into_iter().map(|c| c.name)).await?;
        self.start_all(&plans).await
    }

    async fn delete(&self, kind: &str, namespace: &str, name: &str) -> Result<(), String> {
        Self::require_trino(kind)?;
        let current = self.engine_containers(namespace, name).await?;
        self.remove_all(current.into_iter().map(|c| c.name)).await
    }

    async fn restart(&self, kind: &str, namespace: &str, name: &str) -> Result<(), String> {
        Self::require_trino(kind)?;
        let current = self.engine_containers(namespace, name).await?;
        // Restart cannot recreate containers (it has no spec). Saying "ok" here
        // left an engine whose containers were removed outside egret (for
        // example by the demo's `just demo-down`) at "provisioning" for good.
        // An engine still being created has none yet either, so name both.
        if current.is_empty() {
            return Err(format!(
                "engine {name} has no containers to restart: either they are not created yet \
                 (try again once it is running) or they were removed outside egret (delete \
                 the engine and add it again)"
            ));
        }
        for c in current {
            self.api.restart(&c.name).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::api::FakeDocker;
    use crate::spec::{CatalogSpec, RoleConfig, TrinoConfig, WorkerRoleConfig};

    const NS: &str = "egret-engines";

    fn spec_with(name: &str, workers: u32) -> EngineSpec {
        EngineSpec {
            name: name.into(),
            namespace: NS.into(),
            image_version: "483".into(),
            config: TrinoConfig {
                worker: WorkerRoleConfig {
                    base: RoleConfig {
                        replicas: workers,
                        ..Default::default()
                    },
                    graceful_shutdown: None,
                },
                ..Default::default()
            },
            catalogs: vec![CatalogSpec {
                name: "egret".into(),
                properties: Default::default(),
                env: Default::default(),
            }],
            ranger: None,
            kind: EngineKind::Trino,
            starrocks: None,
        }
    }

    fn setup() -> (Arc<FakeDocker>, DockerEngineProvisioner) {
        let fake = Arc::new(FakeDocker::new());
        let p = DockerEngineProvisioner::new(
            fake.clone(),
            "demo_net".into(),
            "demo".into(),
            vec!["trinodb/trino".into()],
        );
        (fake, p)
    }

    fn coord(name: &str) -> String {
        format!("egret-engine-{NS}-{name}-coordinator")
    }

    fn worker(name: &str, i: u32) -> String {
        format!("egret-engine-{NS}-{name}-worker-{i}")
    }

    #[tokio::test]
    async fn provision_creates_coordinator_first_on_the_network() {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", 2)).await.unwrap();
        assert_eq!(
            fake.calls_to("create_and_start"),
            vec![
                format!("create_and_start:{}", coord("e1")),
                format!("create_and_start:{}", worker("e1", 0)),
                format!("create_and_start:{}", worker("e1", 1)),
            ]
        );
        assert_eq!(fake.network_of(&coord("e1")).as_deref(), Some("demo_net"));
        assert_eq!(
            fake.network_of(&worker("e1", 1)).as_deref(),
            Some("demo_net")
        );
    }

    #[tokio::test]
    async fn provision_pulls_each_distinct_image_once() {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", 2)).await.unwrap();
        assert_eq!(
            fake.calls_to("ensure_image"),
            vec!["ensure_image:trinodb/trino:483"]
        );
    }

    #[tokio::test]
    async fn provision_rejects_starrocks() {
        let (fake, p) = setup();
        let mut s = spec_with("e1", 0);
        s.kind = EngineKind::StarRocks;
        let e = p.provision(&s).await.unwrap_err();
        assert_eq!(e, "the Docker engine provisioner supports Trino only");
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn provision_rejects_a_disallowed_image_before_any_docker_call() {
        let fake = Arc::new(FakeDocker::new());
        let p = DockerEngineProvisioner::new(
            fake.clone(),
            "demo_net".into(),
            "demo".into(),
            vec!["other/image".into()],
        );
        let e = p.provision(&spec_with("e1", 1)).await.unwrap_err();
        assert!(e.contains("not in the allowed images"), "{e}");
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn provision_rejects_unsupported_options_naming_them() {
        let (fake, p) = setup();
        let mut s = spec_with("e1", 0);
        s.config.autoscaling = Some(Default::default());
        let e = p.provision(&s).await.unwrap_err();
        assert!(e.contains("autoscaling"), "{e}");
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn provision_failure_on_second_container_removes_the_first() {
        let (fake, p) = setup();
        fake.fail_after("create_and_start", 1);
        let e = p.provision(&spec_with("e1", 1)).await.unwrap_err();
        assert!(e.contains("fake create_and_start failure"), "{e}");
        assert!(fake.names().is_empty(), "{:?}", fake.names());
        assert_eq!(
            fake.calls_to("stop_and_remove"),
            vec![format!("stop_and_remove:{}", coord("e1"))]
        );
    }

    #[tokio::test]
    async fn provision_twice_fails_and_leaves_the_first_engine_alone() {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", 1)).await.unwrap();
        let e = p.provision(&spec_with("e1", 1)).await.unwrap_err();
        assert_eq!(e, "engine e1 already has containers");
        assert_eq!(fake.names(), vec![coord("e1"), worker("e1", 0)]);
        assert!(fake.calls_to("stop_and_remove").is_empty());
    }

    async fn status_with(states: &[(&str, ContainerState)], workers: u32) -> EngineStatus {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", workers)).await.unwrap();
        for (n, s) in states {
            fake.set_state(n, s.clone());
        }
        fake.set_log("boom: line 20");
        p.status("trino", NS, "e1").await.unwrap()
    }

    #[tokio::test]
    async fn status_with_no_containers_is_provisioning() {
        let (_fake, p) = setup();
        let s = p.status("trino", NS, "e1").await.unwrap();
        assert_eq!(s.phase, "provisioning");
        assert!(s.pods.is_empty());
    }

    #[tokio::test]
    async fn status_mapping_table() {
        let c = coord("e1");
        let running = |healthy| ContainerState::Running { healthy };
        let cases: Vec<(ContainerState, &str, &str, bool)> = vec![
            (ContainerState::Created, "provisioning", "Pending", false),
            (running(None), "running", "Running", true),
            (running(Some(true)), "running", "Running", true),
            (running(Some(false)), "provisioning", "Running", false),
            (ContainerState::Restarting, "provisioning", "Pending", false),
            (ContainerState::Dead, "failed", "Failed", false),
            (
                ContainerState::Exited { code: 1 },
                "failed",
                "Failed",
                false,
            ),
            (
                ContainerState::Exited { code: 0 },
                "failed",
                "Succeeded",
                false,
            ),
        ];
        for (state, phase, pod_phase, ready) in cases {
            let s = status_with(&[(&c, state.clone())], 0).await;
            assert_eq!(s.phase, phase, "{state:?}");
            assert_eq!(s.pods.len(), 1);
            assert_eq!(s.pods[0].name, c);
            assert_eq!(s.pods[0].phase, pod_phase, "{state:?}");
            assert_eq!(s.pods[0].ready, ready, "{state:?}");
        }
    }

    #[tokio::test]
    async fn status_failed_worker_carries_log_tail_and_one_pod_per_container() {
        let s = status_with(
            &[
                (
                    &coord("e1"),
                    ContainerState::Running {
                        healthy: Some(true),
                    },
                ),
                (&worker("e1", 0), ContainerState::Exited { code: 1 }),
            ],
            1,
        )
        .await;
        assert_eq!(s.phase, "failed");
        assert_eq!(s.message.as_deref(), Some("boom: line 20"));
        assert_eq!(s.pods.len(), 2);
        assert!(s.pods[0].ready);
        assert_eq!(s.pods[1].phase, "Failed");
    }

    #[tokio::test]
    async fn status_reads_the_last_20_log_lines_of_the_failed_container() {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", 1)).await.unwrap();
        fake.set_state(&worker("e1", 0), ContainerState::Dead);
        p.status("trino", NS, "e1").await.unwrap();
        assert_eq!(
            fake.calls_to("log_tail"),
            vec![format!("log_tail:{}:20", worker("e1", 0))]
        );
    }

    #[tokio::test]
    async fn status_one_unhealthy_container_keeps_the_engine_provisioning() {
        let s = status_with(
            &[
                (
                    &coord("e1"),
                    ContainerState::Running {
                        healthy: Some(true),
                    },
                ),
                (
                    &worker("e1", 0),
                    ContainerState::Running {
                        healthy: Some(false),
                    },
                ),
            ],
            1,
        )
        .await;
        assert_eq!(s.phase, "provisioning");
    }

    #[tokio::test]
    async fn update_with_an_invalid_spec_keeps_the_running_engine() {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", 1)).await.unwrap();
        let mut bad = spec_with("e1", 1);
        bad.config.autoscaling = Some(Default::default());
        let e = p.update(&bad).await.unwrap_err();
        assert!(e.contains("autoscaling"), "{e}");
        assert!(fake.calls_to("stop_and_remove").is_empty());
        assert_eq!(fake.names(), vec![coord("e1"), worker("e1", 0)]);
    }

    #[tokio::test]
    async fn update_with_a_pull_failure_keeps_the_running_engine() {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", 0)).await.unwrap();
        fake.fail_next("ensure_image");
        p.update(&spec_with("e1", 0)).await.unwrap_err();
        assert!(fake.calls_to("stop_and_remove").is_empty());
        assert_eq!(fake.names(), vec![coord("e1")]);
    }

    #[tokio::test]
    async fn update_replaces_the_containers_with_the_new_plan() {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", 2)).await.unwrap();
        let mut next = spec_with("e1", 1);
        next.image_version = "484".into();
        p.update(&next).await.unwrap();
        assert_eq!(fake.names(), vec![coord("e1"), worker("e1", 0)]);
        assert_eq!(
            fake.plan_of(&coord("e1")).unwrap().image,
            "trinodb/trino:484"
        );
    }

    #[tokio::test]
    async fn update_when_nothing_exists_behaves_as_provision() {
        let (fake, p) = setup();
        p.update(&spec_with("e1", 1)).await.unwrap();
        assert_eq!(fake.names(), vec![coord("e1"), worker("e1", 0)]);
        assert!(fake.calls_to("stop_and_remove").is_empty());
    }

    fn foreign_labels(project: &str, managed: &str) -> Vec<(String, String)> {
        [
            ("egret.managed", managed),
            ("egret.project", project),
            ("egret.engine", "e1"),
            ("egret.namespace", NS),
            ("egret.role", "coordinator"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    fn add_foreign(fake: &FakeDocker, name: &str, labels: Vec<(String, String)>) {
        let refs: Vec<(&str, &str)> = labels
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        fake.add_foreign(name, &refs);
    }

    #[tokio::test]
    async fn delete_removes_only_this_engines_containers() {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", 1)).await.unwrap();
        p.provision(&spec_with("e2", 0)).await.unwrap();
        // Same engine name and namespace, but another project / not egret's.
        add_foreign(
            &fake,
            "other-project-e1",
            foreign_labels("someone-else", "true"),
        );
        fake.add_foreign(
            "unmanaged-e1",
            &[("egret.engine", "e1"), ("egret.namespace", NS)],
        );
        p.delete("trino", NS, "e1").await.unwrap();
        assert_eq!(
            fake.names(),
            vec![
                coord("e2"),
                "other-project-e1".to_string(),
                "unmanaged-e1".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn delete_twice_is_ok() {
        let (_fake, p) = setup();
        p.provision(&spec_with("e1", 1)).await.unwrap();
        p.delete("trino", NS, "e1").await.unwrap();
        p.delete("trino", NS, "e1").await.unwrap();
    }

    #[tokio::test]
    async fn restart_restarts_coordinator_then_workers() {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", 2)).await.unwrap();
        p.restart("trino", NS, "e1").await.unwrap();
        assert_eq!(
            fake.calls_to("restart"),
            vec![
                format!("restart:{}", coord("e1")),
                format!("restart:{}", worker("e1", 0)),
                format!("restart:{}", worker("e1", 1)),
            ]
        );
    }

    #[tokio::test]
    async fn restart_without_containers_is_an_error_not_a_silent_success() {
        let (fake, p) = setup();
        let err = p.restart("trino", NS, "gone").await.unwrap_err();
        assert!(err.contains("engine gone has no containers"), "{err}");
        assert!(fake.calls_to("restart").is_empty());
    }

    // Defence in depth: even when the daemon ignores the label filter, the
    // provisioner must not act on containers that are not this engine's.
    #[tokio::test]
    async fn never_acts_on_containers_the_daemon_returns_unfiltered() {
        let (fake, p) = setup();
        p.provision(&spec_with("e1", 0)).await.unwrap();
        add_foreign(
            &fake,
            "other-project-e1",
            foreign_labels("someone-else", "true"),
        );
        add_foreign(&fake, "not-managed-true", foreign_labels("demo", "false"));
        fake.add_foreign(
            "unmanaged",
            &[("egret.engine", "e1"), ("egret.namespace", NS)],
        );
        fake.list_ignores_filter();

        p.restart("trino", NS, "e1").await.unwrap();
        assert_eq!(
            fake.calls_to("restart"),
            vec![format!("restart:{}", coord("e1"))]
        );
        let s = p.status("trino", NS, "e1").await.unwrap();
        assert_eq!(s.pods.len(), 1);
        p.delete("trino", NS, "e1").await.unwrap();
        assert_eq!(
            fake.calls_to("stop_and_remove"),
            vec![format!("stop_and_remove:{}", coord("e1"))]
        );
        assert_eq!(fake.names().len(), 3);
        // Provision must not mistake the look-alikes for this engine's containers.
        p.provision(&spec_with("e1", 0)).await.unwrap();
    }

    #[tokio::test]
    async fn non_trino_kind_is_refused_for_status_delete_restart() {
        let (_fake, p) = setup();
        assert!(p.status("starrocks", NS, "e1").await.is_err());
        assert!(p.delete("starrocks", NS, "e1").await.is_err());
        assert!(p.restart("starrocks", NS, "e1").await.is_err());
    }
}
