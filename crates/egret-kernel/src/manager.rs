//! `KernelManager` — orchestrates `KernelRegistry` + `KernelLauncher` to provide
//! ensure/status/stop/touch operations with a clean liveness check.

use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::error::KernelError;
use crate::launcher::{KernelEnv, KernelLauncher, KernelPhase};
use crate::registry::{KernelKey, KernelRegistry};

// ── Status ───────────────────────────────────────────────────────────────────

/// Public snapshot of a kernel: phase + gateway URL.
///
/// Serializes as `{ "phase": "ready", "url": "http://..." }`.
#[derive(Debug, Clone, Serialize)]
pub struct KernelStatus {
    #[serde(serialize_with = "serialize_phase")]
    pub phase: KernelPhase,
    pub url: String,
}

fn serialize_phase<S>(phase: &KernelPhase, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.serialize_str(phase.as_str())
}

// ── parse helper ─────────────────────────────────────────────────────────────

/// Parse a phase string from the registry back to `KernelPhase`.
/// Unknown strings map to `Dead` (safe fallback).
fn parse_phase(s: &str) -> KernelPhase {
    match s {
        "spawning" => KernelPhase::Spawning,
        "ready" => KernelPhase::Ready,
        _ => KernelPhase::Dead,
    }
}

// ── Manager ──────────────────────────────────────────────────────────────────

/// Orchestrates `KernelRegistry` + `KernelLauncher` for per-(user×workspace) kernels.
pub struct KernelManager {
    registry: KernelRegistry,
    launcher: Arc<dyn KernelLauncher>,
    idle_timeout: Duration,
}

impl KernelManager {
    pub fn new(
        registry: KernelRegistry,
        launcher: Arc<dyn KernelLauncher>,
        idle_timeout: Duration,
    ) -> Self {
        Self {
            registry,
            launcher,
            idle_timeout,
        }
    }

    /// Ensure the kernel for `key` is running.
    ///
    /// Reuse path: if the registry already has a "ready" row **and** the
    /// launcher confirms it is still live (`status == Ready`), just touch the
    /// timestamp and return the cached URL.
    ///
    /// Spawn path: otherwise call `launcher.ensure` (90 s timeout), write the
    /// result back to the registry, and return the new status.
    pub async fn ensure_running(
        &self,
        key: KernelKey,
        env: &KernelEnv,
    ) -> Result<KernelStatus, KernelError> {
        if let Some(row) = self.registry.get(key).await? {
            if row.phase == "ready" {
                let live = self.launcher.status(key).await?;
                if live == KernelPhase::Ready {
                    self.registry.touch(key).await?;
                    return Ok(KernelStatus {
                        phase: KernelPhase::Ready,
                        url: row.gateway_url,
                    });
                }
            }
        }

        // Spawn (or re-spawn after stale/dead). Mint a fresh kernel token and
        // inject it into the env so the kernel can call egret back.
        let (token_plain, token_hash) = crate::kernel_token::mint();
        let mut env = env.clone();
        env.insert("EGRET_KERNEL_TOKEN".to_string(), token_plain);

        let h = self
            .launcher
            .ensure(key, &env, Duration::from_secs(90))
            .await?;
        self.registry
            .upsert(key, &h.url, h.pid, h.phase.as_str())
            .await?;
        let expires_at = chrono::Utc::now()
            + chrono::Duration::from_std(crate::kernel_token::TOKEN_TTL).unwrap();
        self.registry
            .set_token(key, &token_hash, expires_at)
            .await?;
        Ok(KernelStatus {
            phase: h.phase,
            url: h.url,
        })
    }

    /// Return the current status from the registry without spawning.
    pub async fn status(&self, key: KernelKey) -> Result<Option<KernelStatus>, KernelError> {
        let row = self.registry.get(key).await?;
        Ok(row.map(|r| KernelStatus {
            phase: parse_phase(&r.phase),
            url: r.gateway_url,
        }))
    }

    /// Stop the kernel and remove the registry row.
    pub async fn stop(&self, key: KernelKey) -> Result<(), KernelError> {
        self.launcher.stop(key).await?;
        self.registry.delete(key).await?;
        Ok(())
    }

    /// Bump `last_used_at` for `key` (called on notebook activity).
    pub async fn touch(&self, key: KernelKey) -> Result<(), KernelError> {
        self.registry.touch(key).await
    }

    /// The configured idle timeout — used by the reaper.
    pub fn idle_timeout(&self) -> Duration {
        self.idle_timeout
    }

    /// Access the underlying registry (for the reaper).
    pub fn registry(&self) -> &KernelRegistry {
        &self.registry
    }

    /// Access the launcher (for the reaper).
    pub fn launcher(&self) -> Arc<dyn KernelLauncher> {
        self.launcher.clone()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use egret_metastore::Db;
    use uuid::Uuid;

    use crate::launcher::{FakeLauncher, KernelEnv};
    use crate::registry::KernelRegistry;

    /// Seed a user + workspace. Returns (workspace_id, user_id).
    async fn seed_kernel(db: &Db) -> (Uuid, Uuid) {
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name) \
             VALUES ('local','mgr-alice@test.local','Alice') RETURNING id",
        )
        .fetch_one(&db.pool)
        .await
        .expect("insert user");

        let ws_id: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by) \
             VALUES ('mgr-test-ws','Manager Test WS',$1) RETURNING id",
        )
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("insert workspace");

        (ws_id, user_id)
    }

    // ── T4/Plan step 1 ────────────────────────────────────────────────────────

    /// First call spawns; second call reuses (launcher.ensure may be called again,
    /// but the registry stays coherent and phase is Ready). stop() records the key.
    #[sqlx::test(migrations = "../../migrations")]
    async fn ensure_running_spawns_then_reuses(pool: sqlx::PgPool) {
        let db = Db { pool };
        let reg = KernelRegistry::new(db.clone());
        let launcher = Arc::new(FakeLauncher::default());
        let mgr = KernelManager::new(reg, launcher.clone(), Duration::from_secs(1800));
        let (ws, user) = seed_kernel(&db).await;
        let key = KernelKey {
            user_id: user,
            workspace_id: ws,
        };
        let env = KernelEnv::new();

        // First call — row doesn't exist, launcher.ensure is invoked.
        let s1 = mgr.ensure_running(key, &env).await.unwrap();
        assert_eq!(s1.phase, KernelPhase::Ready);
        assert!(s1.url.contains("fake"));

        // Second call — row exists with phase "ready" and launcher.status returns
        // Ready → reuse path → phase still Ready.
        let s2 = mgr.ensure_running(key, &env).await.unwrap();
        assert_eq!(s2.phase, KernelPhase::Ready);

        // Registry must have exactly one row.
        let row = mgr.registry.get(key).await.unwrap();
        assert!(row.is_some());

        // stop() delegates to the launcher and clears the registry row.
        mgr.stop(key).await.unwrap();
        assert!(
            launcher.inner.lock().unwrap().stopped.contains(&key),
            "launcher.stop must have been called"
        );
        assert!(
            mgr.registry.get(key).await.unwrap().is_none(),
            "registry row must be deleted after stop"
        );
    }

    // ── Re-spawn on stale Dead row ─────────────────────────────────────────────

    /// When a registry row exists but the launcher reports Dead (stale row),
    /// ensure_running must re-spawn via launcher.ensure — not return the dead row.
    #[sqlx::test(migrations = "../../migrations")]
    async fn ensure_running_respawns_on_dead_launcher(pool: sqlx::PgPool) {
        let db = Db { pool };
        let reg = KernelRegistry::new(db.clone());
        let launcher = Arc::new(FakeLauncher::default());
        // Seed the registry with a "ready" row first.
        let (ws, user) = seed_kernel(&db).await;
        let key = KernelKey {
            user_id: user,
            workspace_id: ws,
        };
        reg.upsert(key, "http://stale/1234", Some(99), "ready")
            .await
            .unwrap();

        // Now configure FakeLauncher to report Dead.
        launcher.inner.lock().unwrap().phase = Some(KernelPhase::Dead);

        let mgr = KernelManager::new(reg, launcher.clone(), Duration::from_secs(1800));
        let env = KernelEnv::new();

        // The registry row says "ready" but launcher.status returns Dead
        // → fall through to ensure path.
        let s = mgr.ensure_running(key, &env).await.unwrap();
        // FakeLauncher.ensure returns the configured phase (Dead).
        assert_eq!(s.phase, KernelPhase::Dead);

        // launcher.ensure was called (one entry in ensured).
        let inner = launcher.inner.lock().unwrap();
        assert_eq!(
            inner.ensured.len(),
            1,
            "launcher.ensure must have been called exactly once"
        );
    }

    // ── status() returns None for a missing key ────────────────────────────────

    #[sqlx::test(migrations = "../../migrations")]
    async fn status_returns_none_when_no_row(pool: sqlx::PgPool) {
        let db = Db { pool };
        let reg = KernelRegistry::new(db.clone());
        let launcher = Arc::new(FakeLauncher::default());
        let mgr = KernelManager::new(reg, launcher, Duration::from_secs(1800));
        let (ws, user) = seed_kernel(&db).await;
        let key = KernelKey {
            user_id: user,
            workspace_id: ws,
        };
        assert!(
            mgr.status(key).await.unwrap().is_none(),
            "status must return None when no kernel row exists"
        );
    }

    // ── P1-T2: mint token on spawn ─────────────────────────────────────────────

    #[sqlx::test(migrations = "../../migrations")]
    async fn ensure_running_mints_token_on_spawn_only(pool: sqlx::PgPool) {
        let db = Db { pool };
        let reg = KernelRegistry::new(db.clone());
        let launcher = Arc::new(FakeLauncher::default());
        let mgr = KernelManager::new(reg, launcher.clone(), Duration::from_secs(1800));
        let (ws, user) = seed_kernel(&db).await;
        let key = KernelKey {
            user_id: user,
            workspace_id: ws,
        };
        let env = KernelEnv::new();

        // Spawn: the env handed to the launcher must carry EGRET_KERNEL_TOKEN, and the
        // registry must resolve that token's hash back to this key.
        mgr.ensure_running(key, &env).await.unwrap();
        let plaintext = {
            let g = launcher.inner.lock().unwrap();
            g.ensured[0].1.get("EGRET_KERNEL_TOKEN").cloned()
        };
        let plaintext = plaintext.expect("spawn env must include EGRET_KERNEL_TOKEN");
        assert!(!plaintext.is_empty());
        let hash = crate::kernel_token::hash_token(&plaintext);
        assert_eq!(
            mgr.registry().resolve_token(&hash).await.unwrap(),
            Some(key)
        );

        // Reuse (FakeLauncher reports Ready): no new ensure() call → token unchanged.
        mgr.ensure_running(key, &env).await.unwrap();
        assert_eq!(
            launcher.inner.lock().unwrap().ensured.len(),
            1,
            "reuse must not re-spawn"
        );
        assert_eq!(
            mgr.registry().resolve_token(&hash).await.unwrap(),
            Some(key),
            "token must persist across reuse"
        );
    }

    // ── KernelStatus serialises correctly ──────────────────────────────────────

    #[test]
    fn kernel_status_serializes_phase_as_string() {
        let ks = KernelStatus {
            phase: KernelPhase::Ready,
            url: "http://fake/abc".into(),
        };
        let j = serde_json::to_value(&ks).unwrap();
        assert_eq!(j["phase"], "ready");
        assert_eq!(j["url"], "http://fake/abc");
    }
}
