//! `apps.pod.launcher = "subprocess"`: one marimo edit process per author on THIS host.
//!
//! For local development and for tests that need a real process; production uses the
//! Kubernetes launcher. It is not multi-tenant isolation: the processes share a machine and a
//! uid, and nothing here limits them. What it does keep is the contract every launcher keeps —
//! a pod token minted at start and revoked at stop, a marimo token that is never printed or
//! stored, and an `ensure_edit` that doubles as the "in use" signal for the idle reaper.
//!
//! The entrypoint honours `EGRET_APPS_CONFIG` and `EGRET_MARIMO_PORT` only under pytest (so a
//! production pod cannot be redirected by its environment). A process started outside the image
//! has no `/opt/egret-apps`, so this launcher sets `PYTEST_CURRENT_TEST` to unlock them: a
//! deliberate dev-only act, which is why this launcher must never be configured in production.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use egret_core::{token, Error, Result};
use tokio::process::{Child, Command};

use crate::launcher::{AppPodLauncher, EditPodKey, RunPodKey};
use crate::podtoken::PodTokenRepo;
use crate::proxy::PodEndpoint;

#[derive(Clone, Debug)]
pub struct SubprocessConfig {
    /// The entrypoint, e.g. `["apps-image/.venv/bin/egret-apps"]`; `edit` is appended.
    pub command: Vec<String>,
    /// Each pod gets `<work_dir>/<pod ref>/{work,home,pod.log}`.
    pub work_dir: PathBuf,
    /// The locked marimo configuration directory.
    pub config_dir: PathBuf,
    /// The venv shim directory (`apps-image/venv-shim`). Run pods only: the launcher refuses to
    /// start one without it, and outside the image it has no other way to find it.
    pub venv_shim_dir: PathBuf,
    /// The base URL pods call back to (egret's pod API and query gateway).
    pub callback_url: String,
    /// `apps.pod.run_kernel_mode` (D24): `"thread"` or `"forced_ipc"`. Run pods only — edit
    /// pods always use marimo's normal single-session kernel.
    pub run_kernel_mode: String,
}

struct Running {
    child: Child,
    port: u16,
    marimo_token: String,
    last_used: Instant,
    log: PathBuf,
}

pub struct SubprocessPodLauncher {
    pods: PodTokenRepo,
    cfg: SubprocessConfig,
    http: reqwest::Client,
    // Held across a start: two requests for one cold pod must not start two processes.
    running: tokio::sync::Mutex<HashMap<EditPodKey, Running>>,
    running_run: tokio::sync::Mutex<HashMap<RunPodKey, Running>>,
}

impl SubprocessPodLauncher {
    pub fn new(pods: PodTokenRepo, cfg: SubprocessConfig) -> Self {
        Self {
            pods,
            cfg,
            http: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(2))
                .build()
                .expect("reqwest client"),
            running: tokio::sync::Mutex::new(HashMap::new()),
            running_run: tokio::sync::Mutex::new(HashMap::new()),
        }
    }
}

fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(anyhow::Error::from)?;
    Ok(l.local_addr().map_err(anyhow::Error::from)?.port())
}

/// SIGTERM the process group, then SIGKILL it if it has not gone in `grace`. The group, not the
/// pid: marimo's kernels are children and must not outlive the pod.
async fn kill_group(child: &mut Child, grace: Duration) {
    let Some(pid) = child.id() else { return };
    let signal = |sig: &str| {
        let _ = std::process::Command::new("kill")
            .args([sig, &format!("-{pid}")])
            .stderr(Stdio::null())
            .status();
    };
    signal("-TERM");
    if tokio::time::timeout(grace, child.wait()).await.is_err() {
        signal("-KILL");
        let _ = child.wait().await;
    }
}

fn log_tail(path: &PathBuf) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(5)..].join(" | ")
}

impl SubprocessPodLauncher {
    /// What an edit and a run pod share: spawn marimo under `pod_ref` in `mode` ("edit" or
    /// "run"), with `extra_env` added on top of the shared contract, and wait until it answers
    /// its health check at `/{prefix}/{pod_ref}/health`. On any failure the caller — who knows
    /// which pod token it minted and how to revoke it — gets `Err`; this function never touches
    /// pod tokens itself.
    async fn spawn_and_wait(
        &self,
        pod_ref: &str,
        mode: &str,
        pod_token: &str,
        extra_env: &[(&str, String)],
        timeout: Duration,
    ) -> Result<Running> {
        let prefix = match mode {
            "edit" => "e",
            "run" => "r",
            other => return Err(anyhow::anyhow!("unknown pod mode {other}").into()),
        };
        let dir = self.cfg.work_dir.join(pod_ref);
        let (work, home) = (dir.join("work"), dir.join("home"));
        std::fs::create_dir_all(&work).map_err(anyhow::Error::from)?;
        std::fs::create_dir_all(&home).map_err(anyhow::Error::from)?;
        let log_path = dir.join("pod.log");
        let log = std::fs::File::create(&log_path).map_err(anyhow::Error::from)?;

        let marimo_token = token::random_token_b64();
        let port = free_port()?;
        let (program, args) = self
            .cfg
            .command
            .split_first()
            .ok_or_else(|| Error::from(anyhow::anyhow!("apps.pod.command is empty")))?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .arg(mode)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &home)
            .env("EGRET_URL", &self.cfg.callback_url)
            .env("EGRET_POD_TOKEN", pod_token)
            .env("EGRET_MARIMO_TOKEN", &marimo_token)
            .env("EGRET_BASE_URL", format!("/{prefix}/{pod_ref}"))
            .env("APPS_WORKDIR", &work)
            .env("EGRET_APPS_CONFIG", &self.cfg.config_dir)
            .env("EGRET_MARIMO_PORT", port.to_string())
            .env(
                "PYTEST_CURRENT_TEST",
                "egret subprocess launcher (development only)",
            );
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.stdout(log.try_clone().map_err(anyhow::Error::from)?)
            .stderr(log)
            .stdin(Stdio::null())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Err(anyhow::anyhow!("cannot start {program}: {e}").into()),
        };

        // Wait until marimo answers its health check behind its own token.
        let health = format!("http://127.0.0.1:{port}/{prefix}/{pod_ref}/health");
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(anyhow::anyhow!(
                    "the app pod exited ({status}) before it was ready: {}",
                    log_tail(&log_path)
                )
                .into());
            }
            let up = self
                .http
                .get(&health)
                .bearer_auth(&marimo_token)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            if up {
                break;
            }
            if Instant::now() >= deadline {
                kill_group(&mut child, Duration::from_secs(2)).await;
                return Err(anyhow::anyhow!(
                    "the app pod was not ready within {timeout:?}: {}",
                    log_tail(&log_path)
                )
                .into());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok(Running {
            child,
            port,
            marimo_token,
            last_used: Instant::now(),
            log: log_path,
        })
    }

    async fn start_edit(&self, key: EditPodKey, timeout: Duration) -> Result<Running> {
        let pod_token = self.pods.mint_edit(key.user_id, key.workspace_id).await?;
        match self
            .spawn_and_wait(&key.pod_ref(), "edit", &pod_token, &[], timeout)
            .await
        {
            Ok(r) => Ok(r),
            Err(e) => {
                let _ = self
                    .pods
                    .revoke_edit_for(key.user_id, key.workspace_id)
                    .await;
                Err(e)
            }
        }
    }

    /// `None` when `key.version_id` names no version (the version was deleted, or the key is
    /// forged): there is nothing to mint a token for, so the caller gets a clear error instead
    /// of a pod started for content that does not exist.
    async fn start_run(&self, key: RunPodKey, timeout: Duration) -> Result<Running> {
        let (app_id, ws) = self
            .pods
            .version_owner(key.version_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("unknown version {}", key.version_id))?;
        let pod_token = self.pods.mint_run(ws, app_id, key.version_id).await?;
        let extra = [
            ("EGRET_APP_ID", app_id.to_string()),
            ("EGRET_VERSION_ID", key.version_id.to_string()),
            ("EGRET_RUN_KERNEL_MODE", self.cfg.run_kernel_mode.clone()),
            (
                "EGRET_VENV_SHIM",
                self.cfg.venv_shim_dir.to_string_lossy().into_owned(),
            ),
        ];
        match self
            .spawn_and_wait(&key.pod_ref(), "run", &pod_token, &extra, timeout)
            .await
        {
            Ok(r) => Ok(r),
            Err(e) => {
                let _ = self.pods.revoke_run_for(ws, app_id, key.version_id).await;
                Err(e)
            }
        }
    }
}

#[async_trait]
impl AppPodLauncher for SubprocessPodLauncher {
    async fn ensure_edit(&self, key: EditPodKey, timeout: Duration) -> Result<PodEndpoint> {
        let mut running = self.running.lock().await;
        if let Some(r) = running.get_mut(&key) {
            if matches!(r.child.try_wait(), Ok(None)) {
                r.last_used = Instant::now();
                return Ok(endpoint(r));
            }
            // It died on its own: forget it, and revoke the token it held.
            tracing::warn!(log = %log_tail(&r.log), "app pod exited; starting a new one");
            running.remove(&key);
            let _ = self
                .pods
                .revoke_edit_for(key.user_id, key.workspace_id)
                .await;
        }
        let started = self.start_edit(key, timeout).await?;
        let ep = endpoint(&started);
        running.insert(key, started);
        Ok(ep)
    }

    async fn stop_edit(&self, key: EditPodKey) -> Result<()> {
        if let Some(mut r) = self.running.lock().await.remove(&key) {
            kill_group(&mut r.child, Duration::from_secs(5)).await;
        }
        self.pods
            .revoke_edit_for(key.user_id, key.workspace_id)
            .await?;
        Ok(())
    }

    async fn idle_edit_pods(&self, idle: Duration) -> Result<Vec<EditPodKey>> {
        Ok(self
            .running
            .lock()
            .await
            .iter()
            .filter(|(_, r)| r.last_used.elapsed() >= idle)
            .map(|(k, _)| *k)
            .collect())
    }

    async fn ensure_run(&self, key: RunPodKey, timeout: Duration) -> Result<PodEndpoint> {
        let mut running = self.running_run.lock().await;
        if let Some(r) = running.get_mut(&key) {
            if matches!(r.child.try_wait(), Ok(None)) {
                r.last_used = Instant::now();
                return Ok(endpoint(r));
            }
            tracing::warn!(log = %log_tail(&r.log), "app run pod exited; starting a new one");
            running.remove(&key);
            if let Ok(Some((app_id, ws))) = self.pods.version_owner(key.version_id).await {
                let _ = self.pods.revoke_run_for(ws, app_id, key.version_id).await;
            }
        }
        let started = self.start_run(key, timeout).await?;
        let ep = endpoint(&started);
        running.insert(key, started);
        Ok(ep)
    }

    async fn stop_run(&self, key: RunPodKey) -> Result<()> {
        if let Some(mut r) = self.running_run.lock().await.remove(&key) {
            kill_group(&mut r.child, Duration::from_secs(5)).await;
        }
        if let Some((app_id, ws)) = self.pods.version_owner(key.version_id).await? {
            self.pods.revoke_run_for(ws, app_id, key.version_id).await?;
        }
        Ok(())
    }

    async fn idle_run_pods(&self, idle: Duration) -> Result<Vec<RunPodKey>> {
        Ok(self
            .running_run
            .lock()
            .await
            .iter()
            .filter(|(_, r)| r.last_used.elapsed() >= idle)
            .map(|(k, _)| *k)
            .collect())
    }

    fn mode(&self) -> &'static str {
        "subprocess"
    }
}

fn endpoint(r: &Running) -> PodEndpoint {
    PodEndpoint {
        base: url::Url::parse(&format!("http://127.0.0.1:{}", r.port)).expect("loopback url"),
        marimo_token: r.marimo_token.clone(),
    }
}
