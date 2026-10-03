//! `apps.pod.launcher = "kubernetes"`: one hardened Pod per (author, workspace) running
//! `egret-apps edit` (spec §6.3). Implements `egret_apps::AppPodLauncher`.
//!
//! What it adds to the kernel launcher's hardening (`kernel_launcher_k8s`):
//!
//! * **The two tokens are never in the Pod spec.** egret creates a per-pod `Secret` and the
//!   container reads `EGRET_POD_TOKEN` and `EGRET_MARIMO_TOKEN` through `secretKeyRef`, so
//!   `kubectl get pod -o yaml` and the API audit log do not show them. The Secret is owned by
//!   the Pod (deleting the Pod garbage-collects it). A launcher that restarts reads the marimo
//!   token back from the Secret.
//! * **A read-only root filesystem** with `emptyDir` mounts for the working directory, `$HOME`
//!   and `/tmp`; no service-account token; `tini` as PID 1 (the image's entrypoint).
//! * **`restartPolicy: Never`**, and egret acts on the entrypoint's exit codes: 64 (bad
//!   environment) is final, 65 (a configuration file that would shadow the locked one) is retried
//!   once with a fresh Pod and so fresh `emptyDir`s, 69 (version fetch failed) is run-pod only.
//! * **Idle is read from the cluster, not from memory**: each use stamps the Pod with
//!   `egret.io/last-used` (at most once a minute per replica), so several egret replicas agree
//!   on which pods nobody wants.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use egret_apps::podtoken::PodTokenRepo;
use egret_apps::{AppPodLauncher, EditPodKey, PodEndpoint, RunPodKey};
use egret_core::{token, Error, Result, UserId, WorkspaceId};
use k8s_openapi::api::core::v1::{
    Capabilities, Container, ContainerPort, EmptyDirVolumeSource, EnvVar, EnvVarSource,
    LocalObjectReference, Pod, PodSecurityContext, PodSpec, Probe, ResourceRequirements,
    SeccompProfile, Secret, SecretKeySelector, SecurityContext, TCPSocketAction, Volume,
    VolumeMount,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use kube::api::{Api, DeleteParams, ListParams, Patch, PatchParams, PostParams};
use uuid::Uuid;

const MARIMO_PORT: i32 = 2718;
const APP_LABEL: &str = "egret-apps";
const ANN_USER: &str = "egret.io/user";
const ANN_WORKSPACE: &str = "egret.io/workspace";
const ANN_VERSION: &str = "egret.io/version";
const ANN_LAST_USED: &str = "egret.io/last-used";
/// Distinguishes an edit pod from a run pod in both labels (for the reaper's list selector) and
/// annotations (so `key_of`/`run_key_of` each refuse to parse the other kind, even if a selector
/// string were ever wrong — defense in depth, not just a filter).
const LABEL_KIND: &str = "egret.io/kind";
/// How often one replica writes `last-used` for one pod.
const TOUCH_EVERY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct K8sAppPodConfig {
    pub namespace: String,
    pub image: String,
    pub image_pull_policy: String,
    pub cpu_request: String,
    pub mem_request: String,
    pub cpu_limit: String,
    pub mem_limit: String,
    pub pull_secret: Option<String>,
    /// egret's in-cluster URL, as the pod reaches it (`EGRET_URL`).
    pub callback_url: String,
}

/// `app-edit-<pod ref>`: DNS-safe and derived from the same ref the proxy's URLs use.
pub fn pod_name(key: &EditPodKey) -> String {
    format!("app-edit-{}", key.pod_ref())
}

fn labels(key: &EditPodKey) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app".to_string(), APP_LABEL.to_string()),
        ("managed-by".to_string(), "egret".to_string()),
        (LABEL_KIND.to_string(), "edit".to_string()),
        (
            "egret-user".to_string(),
            key.user_id.0.simple().to_string()[..8].to_string(),
        ),
        (
            "egret-workspace".to_string(),
            key.workspace_id.0.simple().to_string()[..8].to_string(),
        ),
    ])
}

fn annotations(key: &EditPodKey) -> BTreeMap<String, String> {
    BTreeMap::from([
        (LABEL_KIND.to_string(), "edit".to_string()),
        (ANN_USER.to_string(), key.user_id.0.to_string()),
        (ANN_WORKSPACE.to_string(), key.workspace_id.0.to_string()),
        (ANN_LAST_USED.to_string(), Utc::now().to_rfc3339()),
    ])
}

/// `app-run-<pod ref>`: DNS-safe, and distinct from an edit pod's name by prefix alone.
pub fn run_pod_name(key: &RunPodKey) -> String {
    format!("app-run-{}", key.pod_ref())
}

fn run_labels(key: &RunPodKey) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("app".to_string(), APP_LABEL.to_string()),
        ("managed-by".to_string(), "egret".to_string()),
        (LABEL_KIND.to_string(), "run".to_string()),
        (
            "egret-version".to_string(),
            key.version_id.simple().to_string()[..8].to_string(),
        ),
    ])
}

fn run_annotations(key: &RunPodKey) -> BTreeMap<String, String> {
    BTreeMap::from([
        (LABEL_KIND.to_string(), "run".to_string()),
        (ANN_USER.to_string(), key.user_id.0.to_string()),
        (ANN_VERSION.to_string(), key.version_id.to_string()),
        (ANN_LAST_USED.to_string(), Utc::now().to_rfc3339()),
    ])
}

/// The Secret holding the two tokens. `stringData`, so no base64 here; the API server encodes it.
pub fn build_secret(
    key: &EditPodKey,
    pod_token: &str,
    marimo_token: &str,
    cfg: &K8sAppPodConfig,
) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(pod_name(key)),
            namespace: Some(cfg.namespace.clone()),
            labels: Some(labels(key)),
            ..Default::default()
        },
        type_: Some("Opaque".to_string()),
        string_data: Some(BTreeMap::from([
            ("EGRET_POD_TOKEN".to_string(), pod_token.to_string()),
            ("EGRET_MARIMO_TOKEN".to_string(), marimo_token.to_string()),
        ])),
        ..Default::default()
    }
}

/// The Secret holding a run pod's two tokens, named and labelled like [`build_run_pod`]'s Pod.
pub fn build_run_secret(
    key: &RunPodKey,
    pod_token: &str,
    marimo_token: &str,
    cfg: &K8sAppPodConfig,
) -> Secret {
    Secret {
        metadata: ObjectMeta {
            name: Some(run_pod_name(key)),
            namespace: Some(cfg.namespace.clone()),
            labels: Some(run_labels(key)),
            ..Default::default()
        },
        type_: Some("Opaque".to_string()),
        string_data: Some(BTreeMap::from([
            ("EGRET_POD_TOKEN".to_string(), pod_token.to_string()),
            ("EGRET_MARIMO_TOKEN".to_string(), marimo_token.to_string()),
        ])),
        ..Default::default()
    }
}

fn from_secret(name: &str, key: &str) -> EnvVar {
    EnvVar {
        name: key.to_string(),
        value: None,
        value_from: Some(EnvVarSource {
            secret_key_ref: Some(SecretKeySelector {
                name: name.to_string(),
                key: key.to_string(),
                optional: Some(false),
            }),
            ..Default::default()
        }),
    }
}

fn plain(name: &str, value: &str) -> EnvVar {
    EnvVar {
        name: name.to_string(),
        value: Some(value.to_string()),
        value_from: None,
    }
}

fn empty_dir(name: &str, limit: &str) -> Volume {
    Volume {
        name: name.to_string(),
        empty_dir: Some(EmptyDirVolumeSource {
            medium: None,
            size_limit: Some(Quantity(limit.to_string())),
        }),
        ..Default::default()
    }
}

fn mount(name: &str, path: &str) -> VolumeMount {
    VolumeMount {
        name: name.to_string(),
        mount_path: path.to_string(),
        ..Default::default()
    }
}

/// What an edit and a run pod's container share: image, port, resources, readiness, hardening.
/// `mode` is the entrypoint's appended argument; `extra_env` adds to the three names every pod
/// gets (`EGRET_URL`, `EGRET_BASE_URL`, `APPS_WORKDIR`) before the two secret-backed tokens.
fn marimo_container(
    name: &str,
    mode: &str,
    base_url: String,
    extra_env: Vec<EnvVar>,
    cfg: &K8sAppPodConfig,
) -> Container {
    let mut env = vec![
        plain("EGRET_URL", &cfg.callback_url),
        plain("EGRET_BASE_URL", &base_url),
        plain("APPS_WORKDIR", "/work"),
    ];
    env.extend(extra_env);
    env.push(from_secret(name, "EGRET_POD_TOKEN"));
    env.push(from_secret(name, "EGRET_MARIMO_TOKEN"));
    Container {
        name: "marimo".to_string(),
        image: Some(cfg.image.clone()),
        image_pull_policy: Some(cfg.image_pull_policy.clone()),
        // The image's ENTRYPOINT is `tini -- python -I -m egret_marimo.launcher`.
        args: Some(vec![mode.to_string()]),
        ports: Some(vec![ContainerPort {
            container_port: MARIMO_PORT,
            protocol: Some("TCP".to_string()),
            ..Default::default()
        }]),
        env: Some(env),
        resources: Some(ResourceRequirements {
            requests: Some(BTreeMap::from([
                ("cpu".to_string(), Quantity(cfg.cpu_request.clone())),
                ("memory".to_string(), Quantity(cfg.mem_request.clone())),
            ])),
            limits: Some(BTreeMap::from([
                ("cpu".to_string(), Quantity(cfg.cpu_limit.clone())),
                ("memory".to_string(), Quantity(cfg.mem_limit.clone())),
            ])),
            claims: None,
        }),
        readiness_probe: Some(Probe {
            tcp_socket: Some(TCPSocketAction {
                port: IntOrString::Int(MARIMO_PORT),
                host: None,
            }),
            initial_delay_seconds: Some(2),
            period_seconds: Some(2),
            ..Default::default()
        }),
        security_context: Some(SecurityContext {
            allow_privilege_escalation: Some(false),
            read_only_root_filesystem: Some(true),
            capabilities: Some(Capabilities {
                add: None,
                drop: Some(vec!["ALL".to_string()]),
            }),
            ..Default::default()
        }),
        volume_mounts: Some(vec![
            mount("work", "/work"),
            mount("home", "/home/egret"),
            mount("tmp", "/tmp"),
        ]),
        ..Default::default()
    }
}

/// What every pod's spec shares once its container is built.
fn pod_spec(container: Container, cfg: &K8sAppPodConfig) -> PodSpec {
    PodSpec {
        // egret reads the exit code and decides; the kubelet must not restart behind its back.
        restart_policy: Some("Never".to_string()),
        termination_grace_period_seconds: Some(10),
        automount_service_account_token: Some(false),
        enable_service_links: Some(false),
        image_pull_secrets: cfg
            .pull_secret
            .as_ref()
            .map(|n| vec![LocalObjectReference { name: n.clone() }]),
        security_context: Some(PodSecurityContext {
            run_as_non_root: Some(true),
            run_as_user: Some(1000),
            run_as_group: Some(1000),
            fs_group: Some(1000),
            seccomp_profile: Some(SeccompProfile {
                type_: "RuntimeDefault".to_string(),
                localhost_profile: None,
            }),
            ..Default::default()
        }),
        containers: vec![container],
        volumes: Some(vec![
            empty_dir("work", "256Mi"),
            empty_dir("home", "256Mi"),
            empty_dir("tmp", "256Mi"),
        ]),
        ..Default::default()
    }
}

/// The hardened Pod for an author's edit pod. Pure (unit-tested without a cluster).
pub fn build_pod(key: &EditPodKey, cfg: &K8sAppPodConfig) -> Pod {
    let name = pod_name(key);
    let container = marimo_container(&name, "edit", format!("/e/{}", key.pod_ref()), vec![], cfg);
    Pod {
        metadata: ObjectMeta {
            name: Some(name),
            namespace: Some(cfg.namespace.clone()),
            labels: Some(labels(key)),
            annotations: Some(annotations(key)),
            ..Default::default()
        },
        spec: Some(pod_spec(container, cfg)),
        status: None,
    }
}

/// The hardened Pod for a viewer's run pod (D24: one per (viewer, published version)). Pure.
/// `app_id` is resolved by the caller from `key.version_id` before this is built — the Pod spec
/// itself has no database access.
pub fn build_run_pod(key: &RunPodKey, app_id: Uuid, cfg: &K8sAppPodConfig) -> Pod {
    let name = run_pod_name(key);
    let extra_env = vec![
        plain("EGRET_APP_ID", &app_id.to_string()),
        plain("EGRET_VERSION_ID", &key.version_id.to_string()),
    ];
    let container = marimo_container(
        &name,
        "run",
        format!("/r/{}", key.pod_ref()),
        extra_env,
        cfg,
    );
    Pod {
        metadata: ObjectMeta {
            name: Some(name),
            namespace: Some(cfg.namespace.clone()),
            labels: Some(run_labels(key)),
            annotations: Some(run_annotations(key)),
            ..Default::default()
        },
        spec: Some(pod_spec(container, cfg)),
        status: None,
    }
}

/// What the entrypoint's exit code means (apps-image `egret_marimo.launcher`).
#[derive(Debug, PartialEq, Eq)]
pub enum ExitAction {
    /// Final: report why, stop.
    Dead(String),
    /// A configuration file would shadow the locked one: a brand-new Pod has brand-new
    /// `emptyDir`s, so try once more.
    RecreateFresh,
    /// The version source could not be fetched (run pods only); retry with backoff.
    Retry,
}

pub fn exit_action(code: i32) -> ExitAction {
    match code {
        64 => ExitAction::Dead("the pod's environment is invalid (exit 64)".into()),
        65 => ExitAction::RecreateFresh,
        69 => ExitAction::Retry,
        other => ExitAction::Dead(format!("the app pod exited with status {other}")),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PodState {
    Ready(String),
    Spawning,
    Exited(i32),
    /// Image cannot be pulled, container cannot be created, evicted, being deleted.
    Failed(String),
}

fn pod_state(p: &Pod) -> PodState {
    if p.metadata.deletion_timestamp.is_some() {
        return PodState::Failed("the pod is being deleted".into());
    }
    let Some(status) = &p.status else {
        return PodState::Spawning;
    };
    for cs in status.container_statuses.iter().flatten() {
        if let Some(t) = cs.state.as_ref().and_then(|s| s.terminated.as_ref()) {
            return PodState::Exited(t.exit_code);
        }
        if let Some(reason) = cs
            .state
            .as_ref()
            .and_then(|s| s.waiting.as_ref())
            .and_then(|w| w.reason.as_deref())
        {
            if matches!(
                reason,
                "ImagePullBackOff"
                    | "ErrImagePull"
                    | "CrashLoopBackOff"
                    | "CreateContainerConfigError"
                    | "CreateContainerError"
                    | "InvalidImageName"
            ) {
                return PodState::Failed(format!("the app pod cannot start ({reason})"));
            }
        }
        if cs.ready {
            if let Some(ip) = &status.pod_ip {
                return PodState::Ready(ip.clone());
            }
        }
    }
    if status.phase.as_deref() == Some("Failed") {
        return PodState::Failed(
            status
                .reason
                .clone()
                .unwrap_or_else(|| "the pod failed".into()),
        );
    }
    PodState::Spawning
}

fn key_of(p: &Pod) -> Option<(EditPodKey, DateTime<Utc>)> {
    let ann = p.metadata.annotations.as_ref()?;
    if ann.get(LABEL_KIND).map(String::as_str) != Some("edit") {
        return None;
    }
    let key = EditPodKey {
        user_id: UserId(ann.get(ANN_USER)?.parse::<Uuid>().ok()?),
        workspace_id: WorkspaceId(ann.get(ANN_WORKSPACE)?.parse::<Uuid>().ok()?),
    };
    let last = ann
        .get(ANN_LAST_USED)
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| p.metadata.creation_timestamp.as_ref().map(|t| t.0))?;
    Some((key, last.with_timezone(&Utc)))
}

/// [`key_of`]'s run-pod counterpart. Refuses to parse an edit pod (or anything without the
/// `run` kind annotation) the same way `key_of` refuses a run pod — neither reaper can reap the
/// other kind even if a list selector were ever misconfigured.
fn run_key_of(p: &Pod) -> Option<(RunPodKey, DateTime<Utc>)> {
    let ann = p.metadata.annotations.as_ref()?;
    if ann.get(LABEL_KIND).map(String::as_str) != Some("run") {
        return None;
    }
    let key = RunPodKey {
        user_id: UserId(ann.get(ANN_USER)?.parse::<Uuid>().ok()?),
        version_id: ann.get(ANN_VERSION)?.parse::<Uuid>().ok()?,
    };
    let last = ann
        .get(ANN_LAST_USED)
        .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| p.metadata.creation_timestamp.as_ref().map(|t| t.0))?;
    Some((key, last.with_timezone(&Utc)))
}

fn api_err(what: &str, e: kube::Error) -> Error {
    anyhow::anyhow!("{what}: {e}").into()
}

fn is_status(e: &kube::Error, code: u16) -> bool {
    matches!(e, kube::Error::Api(ae) if ae.code == code)
}

pub struct KubernetesAppPodLauncher {
    client: kube::Client,
    cfg: K8sAppPodConfig,
    tokens: PodTokenRepo,
    touched: Mutex<HashMap<EditPodKey, Instant>>,
    touched_run: Mutex<HashMap<RunPodKey, Instant>>,
}

impl KubernetesAppPodLauncher {
    pub fn new(client: kube::Client, cfg: K8sAppPodConfig, tokens: PodTokenRepo) -> Self {
        Self {
            client,
            cfg,
            tokens,
            touched: Mutex::new(HashMap::new()),
            touched_run: Mutex::new(HashMap::new()),
        }
    }

    fn pods(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.cfg.namespace)
    }

    fn secrets(&self) -> Api<Secret> {
        Api::namespaced(self.client.clone(), &self.cfg.namespace)
    }

    async fn marimo_token(&self, name: &str) -> Result<String> {
        let secret = self
            .secrets()
            .get(name)
            .await
            .map_err(|e| api_err("read the pod's Secret", e))?;
        let bytes = secret
            .data
            .and_then(|mut d| d.remove("EGRET_MARIMO_TOKEN"))
            .ok_or_else(|| Error::from(anyhow::anyhow!("the pod's Secret has no marimo token")))?;
        String::from_utf8(bytes.0).map_err(|e| anyhow::Error::from(e).into())
    }

    async fn endpoint(&self, name: &str, ip: &str) -> Result<PodEndpoint> {
        Ok(PodEndpoint {
            base: url::Url::parse(&format!("http://{ip}:{MARIMO_PORT}"))
                .map_err(anyhow::Error::from)?,
            marimo_token: self.marimo_token(name).await?,
        })
    }

    /// Stamp the pod as in use, at most once a minute from this replica. Best effort: a missed
    /// stamp can only make the reaper stop an idle pod a little early, never keep one alive.
    async fn touch(&self, key: EditPodKey, name: &str) {
        {
            let mut t = self.touched.lock().unwrap();
            if t.get(&key).is_some_and(|at| at.elapsed() < TOUCH_EVERY) {
                return;
            }
            t.insert(key, Instant::now());
        }
        let patch = serde_json::json!({
            "metadata": { "annotations": { ANN_LAST_USED: Utc::now().to_rfc3339() } }
        });
        if let Err(e) = self
            .pods()
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
        {
            tracing::warn!(error = %e, pod = name, "could not stamp the app pod as used");
        }
    }

    /// Delete the Pod (its Secret follows by ownership; deleted explicitly too in case the
    /// owner reference was never set) and revoke its tokens.
    async fn discard(&self, key: EditPodKey, name: &str) {
        let _ = self.pods().delete(name, &DeleteParams::default()).await;
        let _ = self.secrets().delete(name, &DeleteParams::default()).await;
        let _ = self
            .tokens
            .revoke_edit_for(key.user_id, key.workspace_id)
            .await;
    }

    async fn create(&self, key: EditPodKey, name: &str) -> Result<()> {
        let pod_token = self.tokens.mint_edit(key.user_id, key.workspace_id).await?;
        let marimo_token = token::random_token_b64();
        let secret = build_secret(&key, &pod_token, &marimo_token, &self.cfg);
        match self.secrets().create(&PostParams::default(), &secret).await {
            Ok(_) => {}
            Err(e) if is_status(&e, 409) => {
                // A Secret with no Pod is a leftover; with a Pod, another replica is mid-create
                // and it owns these tokens, so ours go.
                let _ = self
                    .tokens
                    .revoke_edit_for(key.user_id, key.workspace_id)
                    .await;
                if self.pods().get(name).await.is_ok() {
                    return Ok(());
                }
                self.discard(key, name).await;
                return Err(anyhow::anyhow!("a stale Secret was removed; retry").into());
            }
            Err(e) => {
                let _ = self
                    .tokens
                    .revoke_edit_for(key.user_id, key.workspace_id)
                    .await;
                return Err(api_err("create the pod's Secret", e));
            }
        }
        let created = match self
            .pods()
            .create(&PostParams::default(), &build_pod(&key, &self.cfg))
            .await
        {
            Ok(p) => p,
            Err(e) if is_status(&e, 409) => return Ok(()), // another replica won; wait for its pod
            Err(e) => {
                self.discard(key, name).await;
                return Err(api_err("create the app pod", e));
            }
        };
        // The Pod owns the Secret from now on: deleting the Pod garbage-collects it.
        if let Some(uid) = created.metadata.uid {
            let owner = serde_json::json!({
                "metadata": { "ownerReferences": [OwnerReference {
                    api_version: "v1".into(),
                    kind: "Pod".into(),
                    name: name.to_string(),
                    uid,
                    block_owner_deletion: Some(true),
                    controller: None,
                }]}
            });
            if let Err(e) = self
                .secrets()
                .patch(name, &PatchParams::default(), &Patch::Merge(&owner))
                .await
            {
                tracing::warn!(error = %e, pod = name, "could not make the Pod own its Secret");
            }
        }
        Ok(())
    }

    async fn ensure_once(
        &self,
        key: EditPodKey,
        timeout: Duration,
        fresh_retry: bool,
    ) -> Result<PodEndpoint> {
        let name = pod_name(&key);
        let deadline = tokio::time::Instant::now() + timeout;
        let mut created_here = false;
        loop {
            match self.pods().get(&name).await {
                Ok(p) => match pod_state(&p) {
                    PodState::Ready(ip) => {
                        self.touch(key, &name).await;
                        return self.endpoint(&name, &ip).await;
                    }
                    PodState::Spawning => {}
                    PodState::Exited(code) => match exit_action(code) {
                        ExitAction::RecreateFresh if fresh_retry => {
                            self.discard(key, &name).await;
                            return Box::pin(self.ensure_once(key, timeout, false)).await;
                        }
                        ExitAction::Retry | ExitAction::RecreateFresh | ExitAction::Dead(_) => {
                            let why = match exit_action(code) {
                                ExitAction::Dead(w) => w,
                                _ => format!("the app pod exited with status {code}"),
                            };
                            self.discard(key, &name).await;
                            return Err(anyhow::anyhow!("{why}").into());
                        }
                    },
                    PodState::Failed(why) => {
                        self.discard(key, &name).await;
                        return Err(anyhow::anyhow!("{why}").into());
                    }
                },
                Err(e) if is_status(&e, 404) => {
                    if created_here {
                        // created, but not visible yet
                    } else {
                        created_here = true;
                        self.create(key, &name).await?;
                    }
                }
                Err(e) => return Err(api_err("read the app pod", e)),
            }
            if tokio::time::Instant::now() >= deadline {
                self.discard(key, &name).await;
                return Err(anyhow::anyhow!("the app pod was not ready within {timeout:?}").into());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    /// Stamp a run pod as in use, mirroring [`Self::touch`].
    async fn touch_run(&self, key: RunPodKey, name: &str) {
        {
            let mut t = self.touched_run.lock().unwrap();
            if t.get(&key).is_some_and(|at| at.elapsed() < TOUCH_EVERY) {
                return;
            }
            t.insert(key, Instant::now());
        }
        let patch = serde_json::json!({
            "metadata": { "annotations": { ANN_LAST_USED: Utc::now().to_rfc3339() } }
        });
        if let Err(e) = self
            .pods()
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
        {
            tracing::warn!(error = %e, pod = name, "could not stamp the app run pod as used");
        }
    }

    /// Delete the run pod and its Secret, and revoke the version's run token — mirroring
    /// [`Self::discard`]. `app_id` is `None` when the version is already gone (nothing to
    /// revoke by; the Pod and Secret are still removed).
    async fn discard_run(&self, key: RunPodKey, app_id: Option<(Uuid, WorkspaceId)>, name: &str) {
        let _ = self.pods().delete(name, &DeleteParams::default()).await;
        let _ = self.secrets().delete(name, &DeleteParams::default()).await;
        if let Some((app, ws)) = app_id {
            let _ = self.tokens.revoke_run_for(ws, app, key.version_id).await;
        }
        self.touched_run.lock().unwrap().remove(&key);
    }

    async fn create_run(
        &self,
        key: RunPodKey,
        app_id: Uuid,
        ws: WorkspaceId,
        name: &str,
    ) -> Result<()> {
        let pod_token = self.tokens.mint_run(ws, app_id, key.version_id).await?;
        let marimo_token = token::random_token_b64();
        let secret = build_run_secret(&key, &pod_token, &marimo_token, &self.cfg);
        match self.secrets().create(&PostParams::default(), &secret).await {
            Ok(_) => {}
            Err(e) if is_status(&e, 409) => {
                let _ = self.tokens.revoke_run_for(ws, app_id, key.version_id).await;
                if self.pods().get(name).await.is_ok() {
                    return Ok(());
                }
                self.discard_run(key, Some((app_id, ws)), name).await;
                return Err(anyhow::anyhow!("a stale Secret was removed; retry").into());
            }
            Err(e) => {
                let _ = self.tokens.revoke_run_for(ws, app_id, key.version_id).await;
                return Err(api_err("create the run pod's Secret", e));
            }
        }
        let created = match self
            .pods()
            .create(
                &PostParams::default(),
                &build_run_pod(&key, app_id, &self.cfg),
            )
            .await
        {
            Ok(p) => p,
            Err(e) if is_status(&e, 409) => return Ok(()),
            Err(e) => {
                self.discard_run(key, Some((app_id, ws)), name).await;
                return Err(api_err("create the run pod", e));
            }
        };
        if let Some(uid) = created.metadata.uid {
            let owner = serde_json::json!({
                "metadata": { "ownerReferences": [OwnerReference {
                    api_version: "v1".into(),
                    kind: "Pod".into(),
                    name: name.to_string(),
                    uid,
                    block_owner_deletion: Some(true),
                    controller: None,
                }]}
            });
            if let Err(e) = self
                .secrets()
                .patch(name, &PatchParams::default(), &Patch::Merge(&owner))
                .await
            {
                tracing::warn!(error = %e, pod = name, "could not make the run pod own its Secret");
            }
        }
        Ok(())
    }

    /// Mirrors [`Self::ensure_once`]'s retry/exit-code loop, with one run-only addition: exit 69
    /// (the pod could not fetch its version's source — the pod API was briefly unreachable) is
    /// retried with the same backoff instead of being treated as final.
    async fn ensure_once_run(
        &self,
        key: RunPodKey,
        timeout: Duration,
        fresh_retry: bool,
    ) -> Result<PodEndpoint> {
        let name = run_pod_name(&key);
        let (app_id, ws) = self
            .tokens
            .version_owner(key.version_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("unknown version {}", key.version_id))?;
        let deadline = tokio::time::Instant::now() + timeout;
        let mut created_here = false;
        loop {
            match self.pods().get(&name).await {
                Ok(p) => match pod_state(&p) {
                    PodState::Ready(ip) => {
                        self.touch_run(key, &name).await;
                        return self.endpoint(&name, &ip).await;
                    }
                    PodState::Spawning => {}
                    PodState::Exited(code) => match exit_action(code) {
                        ExitAction::RecreateFresh if fresh_retry => {
                            self.discard_run(key, Some((app_id, ws)), &name).await;
                            return Box::pin(self.ensure_once_run(key, timeout, false)).await;
                        }
                        ExitAction::Retry => {
                            self.discard_run(key, Some((app_id, ws)), &name).await;
                            created_here = false;
                        }
                        ExitAction::RecreateFresh | ExitAction::Dead(_) => {
                            let why = match exit_action(code) {
                                ExitAction::Dead(w) => w,
                                _ => format!("the run pod exited with status {code}"),
                            };
                            self.discard_run(key, Some((app_id, ws)), &name).await;
                            return Err(anyhow::anyhow!("{why}").into());
                        }
                    },
                    PodState::Failed(why) => {
                        self.discard_run(key, Some((app_id, ws)), &name).await;
                        return Err(anyhow::anyhow!("{why}").into());
                    }
                },
                Err(e) if is_status(&e, 404) => {
                    if created_here {
                        // created, but not visible yet
                    } else {
                        created_here = true;
                        self.create_run(key, app_id, ws, &name).await?;
                    }
                }
                Err(e) => return Err(api_err("read the run pod", e)),
            }
            if tokio::time::Instant::now() >= deadline {
                self.discard_run(key, Some((app_id, ws)), &name).await;
                return Err(anyhow::anyhow!("the run pod was not ready within {timeout:?}").into());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

#[async_trait]
impl AppPodLauncher for KubernetesAppPodLauncher {
    async fn ensure_edit(&self, key: EditPodKey, timeout: Duration) -> Result<PodEndpoint> {
        self.ensure_once(key, timeout, true).await
    }

    async fn stop_edit(&self, key: EditPodKey) -> Result<()> {
        let name = pod_name(&key);
        match self.pods().delete(&name, &DeleteParams::default()).await {
            Ok(_) => {}
            Err(e) if is_status(&e, 404) => {}
            Err(e) => return Err(api_err("delete the app pod", e)),
        }
        let _ = self.secrets().delete(&name, &DeleteParams::default()).await;
        self.tokens
            .revoke_edit_for(key.user_id, key.workspace_id)
            .await?;
        self.touched.lock().unwrap().remove(&key);
        Ok(())
    }

    async fn idle_edit_pods(&self, idle: Duration) -> Result<Vec<EditPodKey>> {
        let list = self
            .pods()
            .list(&ListParams::default().labels(&format!(
                "app={APP_LABEL},managed-by=egret,{LABEL_KIND}=edit"
            )))
            .await
            .map_err(|e| api_err("list app pods", e))?;
        let now = Utc::now();
        Ok(list
            .items
            .iter()
            .filter_map(key_of)
            .filter(|(_, last)| {
                now.signed_duration_since(*last)
                    .to_std()
                    .is_ok_and(|age| age >= idle)
            })
            .map(|(k, _)| k)
            .collect())
    }

    async fn ensure_run(&self, key: RunPodKey, timeout: Duration) -> Result<PodEndpoint> {
        self.ensure_once_run(key, timeout, true).await
    }

    async fn stop_run(&self, key: RunPodKey) -> Result<()> {
        let name = run_pod_name(&key);
        match self.pods().delete(&name, &DeleteParams::default()).await {
            Ok(_) => {}
            Err(e) if is_status(&e, 404) => {}
            Err(e) => return Err(api_err("delete the run pod", e)),
        }
        let _ = self.secrets().delete(&name, &DeleteParams::default()).await;
        if let Some((app_id, ws)) = self.tokens.version_owner(key.version_id).await? {
            self.tokens
                .revoke_run_for(ws, app_id, key.version_id)
                .await?;
        }
        self.touched_run.lock().unwrap().remove(&key);
        Ok(())
    }

    async fn idle_run_pods(&self, idle: Duration) -> Result<Vec<RunPodKey>> {
        let list = self
            .pods()
            .list(&ListParams::default().labels(&format!(
                "app={APP_LABEL},managed-by=egret,{LABEL_KIND}=run"
            )))
            .await
            .map_err(|e| api_err("list app pods", e))?;
        let now = Utc::now();
        Ok(list
            .items
            .iter()
            .filter_map(run_key_of)
            .filter(|(_, last)| {
                now.signed_duration_since(*last)
                    .to_std()
                    .is_ok_and(|age| age >= idle)
            })
            .map(|(k, _)| k)
            .collect())
    }

    fn mode(&self) -> &'static str {
        "kubernetes"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> EditPodKey {
        EditPodKey {
            user_id: UserId(Uuid::new_v4()),
            workspace_id: WorkspaceId(Uuid::new_v4()),
        }
    }

    fn cfg() -> K8sAppPodConfig {
        K8sAppPodConfig {
            namespace: "egret-apps".into(),
            image: "egret-apps:dev".into(),
            image_pull_policy: "IfNotPresent".into(),
            cpu_request: "250m".into(),
            mem_request: "512Mi".into(),
            cpu_limit: "1000m".into(),
            mem_limit: "2Gi".into(),
            pull_secret: None,
            callback_url: "http://egret:8080".into(),
        }
    }

    #[test]
    fn the_pod_is_named_from_the_shared_ref_and_is_dns_safe() {
        let k = key();
        let n = pod_name(&k);
        assert_eq!(n, format!("app-edit-{}", k.pod_ref()));
        assert!(
            n.len() <= 63
                && n.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        );
        assert_ne!(pod_name(&k), pod_name(&key()));
    }

    #[test]
    fn the_tokens_are_in_the_secret_and_reach_the_container_only_by_reference() {
        let k = key();
        let pod = build_pod(&k, &cfg());
        let spec = pod.spec.unwrap();
        let env = spec.containers[0].env.clone().unwrap();
        for name in ["EGRET_POD_TOKEN", "EGRET_MARIMO_TOKEN"] {
            let var = env
                .iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("{name}"));
            assert!(
                var.value.is_none(),
                "{name} must not be a literal in the Pod spec"
            );
            let r = var
                .value_from
                .as_ref()
                .unwrap()
                .secret_key_ref
                .as_ref()
                .unwrap();
            assert_eq!(
                (r.name.as_str(), r.key.as_str()),
                (pod_name(&k).as_str(), name)
            );
        }
        let plain: BTreeMap<_, _> = env
            .iter()
            .filter_map(|e| e.value.as_ref().map(|v| (e.name.as_str(), v.as_str())))
            .collect();
        assert_eq!(plain["EGRET_URL"], "http://egret:8080");
        assert_eq!(plain["EGRET_BASE_URL"], format!("/e/{}", k.pod_ref()));
        assert_eq!(plain["APPS_WORKDIR"], "/work");
        assert!(!serde_json::to_string(&pod_json(&k))
            .unwrap()
            .contains("tok-secret"));

        let secret = build_secret(&k, "tok-pod", "tok-marimo", &cfg());
        let data = secret.string_data.unwrap();
        assert_eq!(data["EGRET_POD_TOKEN"], "tok-pod");
        assert_eq!(data["EGRET_MARIMO_TOKEN"], "tok-marimo");
        assert_eq!(secret.metadata.name.as_deref(), Some(pod_name(&k).as_str()));
    }

    fn pod_json(k: &EditPodKey) -> serde_json::Value {
        serde_json::to_value(build_pod(k, &cfg())).unwrap()
    }

    #[test]
    fn the_pod_is_hardened() {
        let pod = build_pod(&key(), &cfg());
        let spec = pod.spec.unwrap();
        let psc = spec.security_context.unwrap();
        assert_eq!(psc.run_as_non_root, Some(true));
        assert_eq!(psc.run_as_user, Some(1000));
        assert_eq!(psc.seccomp_profile.unwrap().type_, "RuntimeDefault");
        assert_eq!(spec.automount_service_account_token, Some(false));
        assert_eq!(spec.enable_service_links, Some(false));
        assert_eq!(
            spec.restart_policy.as_deref(),
            Some("Never"),
            "egret reads the exit code"
        );
        assert_eq!(spec.host_network, None);
        assert_eq!(spec.share_process_namespace, None);
        let c = &spec.containers[0];
        let sc = c.security_context.as_ref().unwrap();
        assert_eq!(sc.allow_privilege_escalation, Some(false));
        assert_eq!(sc.read_only_root_filesystem, Some(true));
        assert_eq!(
            sc.capabilities.as_ref().unwrap().drop,
            Some(vec!["ALL".to_string()])
        );
        assert_eq!(sc.privileged, None);
        // the only writable places are the three emptyDirs
        let mounts: Vec<&str> = c
            .volume_mounts
            .iter()
            .flatten()
            .map(|m| m.mount_path.as_str())
            .collect();
        assert_eq!(mounts, ["/work", "/home/egret", "/tmp"]);
        assert!(spec.volumes.unwrap().iter().all(|v| v.empty_dir.is_some()));
        // the entrypoint (tini → launcher) comes from the image; only the mode is passed
        assert_eq!(c.args, Some(vec!["edit".to_string()]));
        assert!(c.command.is_none());
        assert_eq!(c.ports.as_ref().unwrap()[0].container_port, 2718);
    }

    #[test]
    fn labels_select_the_pods_and_annotations_carry_the_full_ids_for_the_reaper() {
        let k = key();
        let pod = build_pod(&k, &cfg());
        let labels = pod.metadata.labels.clone().unwrap();
        assert_eq!(labels["app"], "egret-apps");
        assert_eq!(labels["managed-by"], "egret");
        assert_eq!(labels["egret-user"].len(), 8);
        // the reaper rebuilds the key from annotations, so it round-trips
        let (back, last) = key_of(&pod).expect("annotations parse");
        assert_eq!(back, k);
        assert!((Utc::now() - last).num_seconds().abs() < 5);
    }

    #[test]
    fn a_pod_without_our_annotations_is_not_ours_to_reap() {
        let mut pod = build_pod(&key(), &cfg());
        pod.metadata.annotations = None;
        assert!(key_of(&pod).is_none());
    }

    #[test]
    fn exit_codes_mean_what_the_entrypoint_says() {
        assert!(matches!(exit_action(64), ExitAction::Dead(_)));
        assert_eq!(exit_action(65), ExitAction::RecreateFresh);
        assert_eq!(exit_action(69), ExitAction::Retry);
        assert!(matches!(exit_action(1), ExitAction::Dead(ref m) if m.contains("status 1")));
        assert!(
            matches!(exit_action(137), ExitAction::Dead(_)),
            "OOM-killed is final"
        );
    }

    fn status(json: serde_json::Value) -> Pod {
        serde_json::from_value(serde_json::json!({
            "metadata": { "name": "p" },
            "status": json
        }))
        .unwrap()
    }

    #[test]
    fn pod_states_from_the_cluster() {
        assert_eq!(
            pod_state(&status(serde_json::json!({}))),
            PodState::Spawning
        );
        assert_eq!(
            pod_state(&status(serde_json::json!({
                "podIP": "10.1.2.3",
                "containerStatuses": [{"name":"marimo","ready":true,"restartCount":0,"image":"i","imageID":"x",
                    "state":{"running":{}}}]
            }))),
            PodState::Ready("10.1.2.3".into())
        );
        assert_eq!(
            pod_state(&status(serde_json::json!({
                "containerStatuses": [{"name":"marimo","ready":false,"restartCount":0,"image":"i","imageID":"x",
                    "state":{"terminated":{"exitCode":65}}}]
            }))),
            PodState::Exited(65)
        );
        assert!(matches!(
            pod_state(&status(serde_json::json!({
                "containerStatuses": [{"name":"marimo","ready":false,"restartCount":0,"image":"i","imageID":"x",
                    "state":{"waiting":{"reason":"ImagePullBackOff"}}}]
            }))),
            PodState::Failed(_)
        ));
        // ContainerCreating is not a failure
        assert_eq!(
            pod_state(&status(serde_json::json!({
                "containerStatuses": [{"name":"marimo","ready":false,"restartCount":0,"image":"i","imageID":"x",
                    "state":{"waiting":{"reason":"ContainerCreating"}}}]
            }))),
            PodState::Spawning
        );
        assert!(matches!(
            pod_state(&status(serde_json::json!({"phase": "Failed", "reason": "Evicted"}))),
            PodState::Failed(ref m) if m == "Evicted"
        ));
    }

    #[test]
    fn a_pod_being_deleted_is_failed_not_reusable() {
        let mut pod = build_pod(&key(), &cfg());
        pod.metadata.deletion_timestamp = Some(
            k8s_openapi::apimachinery::pkg::apis::meta::v1::Time(Utc::now()),
        );
        assert!(matches!(pod_state(&pod), PodState::Failed(_)));
    }

    fn run_key() -> RunPodKey {
        RunPodKey {
            user_id: UserId(Uuid::new_v4()),
            version_id: Uuid::new_v4(),
        }
    }

    #[test]
    fn the_run_pod_is_named_from_the_shared_ref_and_is_dns_safe() {
        let k = run_key();
        let n = run_pod_name(&k);
        assert_eq!(n, format!("app-run-{}", k.pod_ref()));
        assert!(
            n.len() <= 63
                && n.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        );
        assert_ne!(run_pod_name(&k), run_pod_name(&run_key()));
        assert_ne!(
            run_pod_name(&k),
            pod_name(&EditPodKey {
                user_id: k.user_id,
                workspace_id: WorkspaceId(k.version_id),
            }),
            "a run pod's name must never collide with an edit pod's"
        );
    }

    #[test]
    fn the_run_pod_carries_its_app_and_version_as_plain_env_and_its_tokens_only_by_reference() {
        let k = run_key();
        let app_id = Uuid::new_v4();
        let pod = build_run_pod(&k, app_id, &cfg());
        let spec = pod.spec.unwrap();
        let env = spec.containers[0].env.clone().unwrap();
        let plain: BTreeMap<_, _> = env
            .iter()
            .filter_map(|e| e.value.as_ref().map(|v| (e.name.as_str(), v.as_str())))
            .collect();
        assert_eq!(plain["EGRET_URL"], "http://egret:8080");
        assert_eq!(plain["EGRET_BASE_URL"], format!("/r/{}", k.pod_ref()));
        assert_eq!(plain["APPS_WORKDIR"], "/work");
        assert_eq!(plain["EGRET_APP_ID"], app_id.to_string());
        assert_eq!(plain["EGRET_VERSION_ID"], k.version_id.to_string());
        for name in ["EGRET_POD_TOKEN", "EGRET_MARIMO_TOKEN"] {
            let var = env.iter().find(|e| e.name == name).unwrap();
            assert!(var.value.is_none(), "{name} must not be a literal");
            assert_eq!(
                var.value_from
                    .as_ref()
                    .unwrap()
                    .secret_key_ref
                    .as_ref()
                    .unwrap()
                    .name,
                run_pod_name(&k)
            );
        }
        assert_eq!(spec.containers[0].args, Some(vec!["run".to_string()]));

        let secret = build_run_secret(&k, "tok-pod", "tok-marimo", &cfg());
        let data = secret.string_data.unwrap();
        assert_eq!(data["EGRET_POD_TOKEN"], "tok-pod");
        assert_eq!(data["EGRET_MARIMO_TOKEN"], "tok-marimo");
        assert_eq!(
            secret.metadata.name.as_deref(),
            Some(run_pod_name(&k).as_str())
        );
    }

    #[test]
    fn the_run_pod_is_hardened_the_same_way_as_edit() {
        let pod = build_run_pod(&run_key(), Uuid::new_v4(), &cfg());
        let spec = pod.spec.unwrap();
        let psc = spec.security_context.unwrap();
        assert_eq!(psc.run_as_non_root, Some(true));
        assert_eq!(spec.restart_policy.as_deref(), Some("Never"));
        let c = &spec.containers[0];
        let sc = c.security_context.as_ref().unwrap();
        assert_eq!(sc.read_only_root_filesystem, Some(true));
        assert_eq!(
            sc.capabilities.as_ref().unwrap().drop,
            Some(vec!["ALL".to_string()])
        );
        let mounts: Vec<&str> = c
            .volume_mounts
            .iter()
            .flatten()
            .map(|m| m.mount_path.as_str())
            .collect();
        assert_eq!(mounts, ["/work", "/home/egret", "/tmp"]);
    }

    #[test]
    fn run_labels_and_annotations_round_trip_for_the_reaper() {
        let k = run_key();
        let pod = build_run_pod(&k, Uuid::new_v4(), &cfg());
        let labels = pod.metadata.labels.clone().unwrap();
        assert_eq!(labels["app"], "egret-apps");
        assert_eq!(labels[LABEL_KIND], "run");
        let (back, last) = run_key_of(&pod).expect("annotations parse");
        assert_eq!(back, k);
        assert!((Utc::now() - last).num_seconds().abs() < 5);
    }

    #[test]
    fn a_run_pod_without_our_annotations_is_not_ours_to_reap() {
        let mut pod = build_run_pod(&run_key(), Uuid::new_v4(), &cfg());
        pod.metadata.annotations = None;
        assert!(run_key_of(&pod).is_none());
    }

    #[test]
    fn key_of_and_run_key_of_each_refuse_the_others_kind() {
        let edit_pod = build_pod(&key(), &cfg());
        let run_pod = build_run_pod(&run_key(), Uuid::new_v4(), &cfg());
        assert!(key_of(&edit_pod).is_some());
        assert!(
            run_key_of(&edit_pod).is_none(),
            "an edit pod must never be reaped as a run pod"
        );
        assert!(run_key_of(&run_pod).is_some());
        assert!(
            key_of(&run_pod).is_none(),
            "a run pod must never be reaped as an edit pod"
        );
    }
}
