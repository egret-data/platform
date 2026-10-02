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

use crate::launcher::{AppPodLauncher, EditPodKey};
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
    /// The base URL pods call back to (egret's pod API and query gateway).
    pub callback_url: String,
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
    async fn start(&self, key: EditPodKey, timeout: Duration) -> Result<Running> {
        let pod_ref = key.pod_ref();
        let dir = self.cfg.work_dir.join(&pod_ref);
        let (work, home) = (dir.join("work"), dir.join("home"));
        std::fs::create_dir_all(&work).map_err(anyhow::Error::from)?;
        std::fs::create_dir_all(&home).map_err(anyhow::Error::from)?;
        let log_path = dir.join("pod.log");
        let log = std::fs::File::create(&log_path).map_err(anyhow::Error::from)?;

        let pod_token = self.pods.mint_edit(key.user_id, key.workspace_id).await?;
        let marimo_token = token::random_token_b64();
        let port = free_port()?;
        let (program, args) = self
            .cfg
            .command
            .split_first()
            .ok_or_else(|| Error::from(anyhow::anyhow!("apps.pod.command is empty")))?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .arg("edit")
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &home)
            .env("EGRET_URL", &self.cfg.callback_url)
            .env("EGRET_POD_TOKEN", &pod_token)
            .env("EGRET_MARIMO_TOKEN", &marimo_token)
            .env("EGRET_BASE_URL", format!("/e/{pod_ref}"))
            .env("APPS_WORKDIR", &work)
            .env("EGRET_APPS_CONFIG", &self.cfg.config_dir)
            .env("EGRET_MARIMO_PORT", port.to_string())
            .env(
                "PYTEST_CURRENT_TEST",
                "egret subprocess launcher (development only)",
            )
            .stdout(log.try_clone().map_err(anyhow::Error::from)?)
            .stderr(log)
            .stdin(Stdio::null())
            .process_group(0)
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let _ = self
                    .pods
                    .revoke_edit_for(key.user_id, key.workspace_id)
                    .await;
                return Err(anyhow::anyhow!("cannot start {program}: {e}").into());
            }
        };

        // Wait until marimo answers its health check behind its own token.
        let health = format!("http://127.0.0.1:{port}/e/{pod_ref}/health");
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                let _ = self
                    .pods
                    .revoke_edit_for(key.user_id, key.workspace_id)
                    .await;
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
                let _ = self
                    .pods
                    .revoke_edit_for(key.user_id, key.workspace_id)
                    .await;
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
        let started = self.start(key, timeout).await?;
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
