//! Idle kernel reaper — periodically stops + removes kernels that have not been
//! used within the configured timeout window.

use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::launcher::KernelLauncher;
use crate::registry::KernelRegistry;

// ── reap_once ────────────────────────────────────────────────────────────────

/// Stop + remove every kernel idle longer than `idle`. Returns how many were reaped.
pub async fn reap_once(
    reg: &KernelRegistry,
    launcher: &Arc<dyn KernelLauncher>,
    idle: Duration,
) -> usize {
    let cutoff =
        chrono::Utc::now() - chrono::Duration::from_std(idle).unwrap_or(chrono::Duration::zero());
    let stale = reg.list_idle(cutoff).await.unwrap_or_default();
    for key in &stale {
        let _ = launcher.stop(*key).await;
        let _ = reg.delete(*key).await;
    }
    stale.len()
}

// ── spawn_reaper ─────────────────────────────────────────────────────────────

/// Spawn the periodic reaper; returns immediately. Cancelled via `cancel`.
pub fn spawn_reaper(
    reg: KernelRegistry,
    launcher: Arc<dyn KernelLauncher>,
    idle: Duration,
    every: Duration,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(every);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => { reap_once(&reg, &launcher, idle).await; }
            }
        }
    });
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use uuid::Uuid;

    use egret_metastore::Db;

    use crate::launcher::FakeLauncher;
    use crate::registry::{KernelKey, KernelRegistry};

    // ── helpers ──────────────────────────────────────────────────────────────

    /// Seed a single user + workspace. Returns (workspace_id, user_id).
    async fn seed_one(db: &Db, email: &str, slug: &str) -> (Uuid, Uuid) {
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name) \
             VALUES ('local', $1, 'Reaper User') RETURNING id",
        )
        .bind(email)
        .fetch_one(&db.pool)
        .await
        .expect("insert user");

        let ws_id: Uuid = sqlx::query_scalar(
            "INSERT INTO workspaces (slug, name, created_by) \
             VALUES ($1, 'Reaper WS', $2) RETURNING id",
        )
        .bind(slug)
        .bind(user_id)
        .fetch_one(&db.pool)
        .await
        .expect("insert workspace");

        (ws_id, user_id)
    }

    // ── reap_once: idle kernel is stopped, fresh kernel is untouched ──────────

    /// Seed two kernels:
    ///   - Kernel A: `last_used_at` set 40 minutes into the past (idle)
    ///   - Kernel B: just upserted (fresh)
    ///
    /// A single `reap_once` sweep with a 30-minute idle threshold must:
    ///   1. Return 1 (exactly one kernel reaped).
    ///   2. Stop kernel A (FakeLauncher.stopped contains key_a).
    ///   3. Delete kernel A's registry row.
    ///   4. Leave kernel B's row untouched.
    #[sqlx::test(migrations = "../../migrations")]
    async fn reap_once_removes_idle_keeps_fresh(pool: sqlx::PgPool) {
        let db = Db { pool };
        let reg = KernelRegistry::new(db.clone());

        // ── seed two (user, workspace) pairs ──────────────────────────────────
        let (ws_a, user_a) = seed_one(&db, "reaper-alice@test.local", "reaper-ws-a").await;
        let (ws_b, user_b) = seed_one(&db, "reaper-bob@test.local", "reaper-ws-b").await;

        let key_a = KernelKey {
            user_id: user_a,
            workspace_id: ws_a,
        };
        let key_b = KernelKey {
            user_id: user_b,
            workspace_id: ws_b,
        };

        // ── upsert both kernels ───────────────────────────────────────────────
        reg.upsert(key_a, "http://127.0.0.1:9991", Some(1001), "ready")
            .await
            .unwrap();
        reg.upsert(key_b, "http://127.0.0.1:9992", Some(1002), "ready")
            .await
            .unwrap();

        // ── force kernel A's last_used_at 40 minutes into the past ───────────
        sqlx::query(
            "UPDATE kernels SET last_used_at = now() - interval '40 minutes' \
             WHERE user_id = $1",
        )
        .bind(user_a)
        .execute(&db.pool)
        .await
        .expect("backdate kernel A");

        // ── run a single reaper sweep with 30-min idle threshold ──────────────
        // Keep a direct Arc<FakeLauncher> for inspection; coerce to trait object for reap_once.
        let fake = Arc::new(FakeLauncher::default());
        let launcher: Arc<dyn KernelLauncher> = fake.clone();
        let reaped = reap_once(&reg, &launcher, Duration::from_secs(30 * 60)).await;

        // ── assertions ────────────────────────────────────────────────────────

        // 1. Exactly one kernel was reaped.
        assert_eq!(reaped, 1, "exactly one kernel should have been reaped");

        // 2. FakeLauncher.stop was called for key_a but not key_b.
        {
            let inner = fake.inner.lock().unwrap();
            assert!(
                inner.stopped.contains(&key_a),
                "launcher.stop must have been called for key_a"
            );
            assert!(
                !inner.stopped.contains(&key_b),
                "launcher.stop must NOT have been called for key_b"
            );
        }

        // 3. Kernel A's registry row is deleted.
        assert!(
            reg.get(key_a).await.unwrap().is_none(),
            "kernel A row must be deleted after reap"
        );

        // 4. Kernel B's row is still present.
        assert!(
            reg.get(key_b).await.unwrap().is_some(),
            "kernel B row must remain untouched"
        );
    }
}
