//! `KernelLauncher` trait, associated types, and `FakeLauncher` test double.

use async_trait::async_trait;
use std::collections::HashMap;

use crate::error::KernelError;
use crate::registry::KernelKey;

// ── Env ──────────────────────────────────────────────────────────────────────

/// Environment variables forwarded to the kernel gateway process.
pub type KernelEnv = HashMap<String, String>;

// ── Phase ─────────────────────────────────────────────────────────────────────

/// Lifecycle phase of a kernel gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelPhase {
    Spawning,
    Ready,
    Dead,
}

impl KernelPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spawning => "spawning",
            Self::Ready => "ready",
            Self::Dead => "dead",
        }
    }
}

// ── Handle ────────────────────────────────────────────────────────────────────

/// Snapshot of a running (or recently dead) kernel gateway.
#[derive(Debug, Clone)]
pub struct KernelHandle {
    pub url: String,
    pub phase: KernelPhase,
    pub pid: Option<i32>,
}

// ── Trait ─────────────────────────────────────────────────────────────────────

#[async_trait]
pub trait KernelLauncher: Send + Sync {
    /// Spawn (or no-op if already live) the kernel for `key` with `env`; block
    /// until ready or `timeout`. Returns the gateway URL + phase.
    async fn ensure(
        &self,
        key: KernelKey,
        env: &KernelEnv,
        timeout: std::time::Duration,
    ) -> Result<KernelHandle, KernelError>;

    async fn status(&self, key: KernelKey) -> Result<KernelPhase, KernelError>;

    async fn stop(&self, key: KernelKey) -> Result<(), KernelError>;

    fn mode(&self) -> &'static str;
}

// ── FakeLauncher ──────────────────────────────────────────────────────────────

/// Test-only launcher: records calls, returns a configured phase + dummy url, no process.
#[derive(Default)]
pub struct FakeLauncher {
    pub inner: std::sync::Mutex<FakeState>,
}

#[derive(Default)]
pub struct FakeState {
    pub ensured: Vec<(KernelKey, KernelEnv)>,
    pub stopped: Vec<KernelKey>,
    pub phase: Option<KernelPhase>,
    pub url: Option<String>,
}

#[async_trait]
impl KernelLauncher for FakeLauncher {
    async fn ensure(
        &self,
        key: KernelKey,
        env: &KernelEnv,
        _t: std::time::Duration,
    ) -> Result<KernelHandle, KernelError> {
        let mut g = self.inner.lock().unwrap();
        g.ensured.push((key, env.clone()));
        let phase = g.phase.unwrap_or(KernelPhase::Ready);
        let url = g
            .url
            .clone()
            .unwrap_or_else(|| format!("http://fake/{}", key.user_id));
        Ok(KernelHandle {
            url,
            phase,
            pid: Some(1),
        })
    }

    async fn status(&self, _key: KernelKey) -> Result<KernelPhase, KernelError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .phase
            .unwrap_or(KernelPhase::Ready))
    }

    async fn stop(&self, key: KernelKey) -> Result<(), KernelError> {
        self.inner.lock().unwrap().stopped.push(key);
        Ok(())
    }

    fn mode(&self) -> &'static str {
        "fake"
    }
}

// ── Smoke test ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn fake_launcher_mode_and_round_trip() {
        let launcher = FakeLauncher::default();
        assert_eq!(launcher.mode(), "fake");

        let key = KernelKey {
            user_id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
        };
        let env: KernelEnv = HashMap::from([("FOO".into(), "bar".into())]);

        let handle = launcher
            .ensure(key, &env, std::time::Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(handle.phase, KernelPhase::Ready);
        assert!(handle.url.contains("fake"));
        assert_eq!(handle.pid, Some(1));

        let phase = launcher.status(key).await.unwrap();
        assert_eq!(phase, KernelPhase::Ready);

        launcher.stop(key).await.unwrap();

        let inner = launcher.inner.lock().unwrap();
        assert_eq!(inner.ensured.len(), 1);
        assert_eq!(inner.ensured[0].0, key);
        assert!(inner.stopped.contains(&key));
    }
}
