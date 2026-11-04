//! The per-profile OPA snapshot cache and its refresher (governance SP2b,
//! D3). The decision path reads [`SnapshotCache::get`] and
//! [`SnapshotCache::binding`] only; it never touches the database. The
//! refresher ([`spawn_snapshot_refresher`]) runs [`SnapshotCache::refresh_once`]
//! each period and rebuilds a profile only when its `(policy_version,
//! tag_version)` moved.
//!
//! **Every build** runs in one `REPEATABLE READ, READ ONLY` transaction with
//! the key read inside it, and:
//!
//! 1. **re-checks the stored state** (C8): a profile holding a row the
//!    evaluator would ignore (`egret_policy::opa_guard`) or a stored pair of
//!    mask/row-filter policies whose winner the IR does not express
//!    ([`crate::overlap::stored_overlaps_via`]) is **not built**, and every
//!    request on it is denied. No previous snapshot is kept: the stored state
//!    is the problem, not the build.
//! 2. builds data ++ infrastructure into a fresh snapshot;
//! 3. **on a data-lowering failure** (C4), including a `Snapshot::build`
//!    refusal, splices freshly lowered infrastructure into the previous
//!    snapshot, so the SP2a kill switch still takes effect: a disabled admin
//!    loses all-access within one refresh even while the profile's data
//!    lowering is broken. With no previous snapshot, or when the
//!    infrastructure cannot be lowered either, the profile is not built.
//!
//! The key a build read is recorded whatever the outcome, so the same failure
//! is not retried every period. The one exception is a failure to read the
//! state at all (a transport error): the entry fails closed and is retried
//! next period.
//!
//! **Which profiles** (open question (a)): those with `mechanism = 'opa'`, and
//! those with a bound engine whose `applied_mechanism = 'opa'`. A switch back
//! to Ranger is not atomic across engines; an engine not yet re-applied still
//! runs the OPA plugin and keeps being answered until it is.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use egret_perm_eval::{Snapshot, SnapshotInput};
use sqlx::PgPool;

use super::build::{lower_infra_via, snapshot_parts_via, MissingUsername};

/// What the decision path finds for one profile.
pub enum Lookup {
    /// Not served over OPA (no entry).
    NotOpa,
    /// Served over OPA, but no snapshot is held: deny everything.
    NotBuilt {
        error: Option<String>,
    },
    Ready(Arc<Snapshot>),
}

impl std::fmt::Debug for Lookup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOpa => f.write_str("NotOpa"),
            Self::NotBuilt { error } => f.debug_struct("NotBuilt").field("error", error).finish(),
            Self::Ready(s) => f
                .debug_struct("Ready")
                .field("service_id", &s.service_id())
                .field("key", &s.key())
                .finish(),
        }
    }
}

struct CacheEntry {
    /// The key the last build read; None: the state could not be read, so
    /// the next refresh retries.
    key: Option<(i64, i64)>,
    snapshot: Option<Arc<Snapshot>>,
    error: Option<String>,
}

#[derive(Default)]
struct State {
    entries: HashMap<i64, CacheEntry>,
    bindings: HashMap<uuid::Uuid, i64>,
}

#[derive(Default)]
pub struct SnapshotCache {
    state: RwLock<State>,
}

/// What one [`SnapshotCache::refresh_once`] did, by profile id.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct RefreshReport {
    /// A fresh snapshot at the profile's current key.
    pub rebuilt: Vec<i64>,
    /// The data lowering failed: the previous data with fresh infrastructure.
    pub spliced: Vec<i64>,
    /// Not built: every request on the profile is denied.
    pub failed: Vec<i64>,
    /// No longer served over OPA: the entry is gone.
    pub dropped: Vec<i64>,
}

/// How one build ended, before it is stored.
enum Outcome {
    Rebuilt(Snapshot),
    Spliced { snapshot: Snapshot, error: String },
    Failed { error: String },
}

struct Built {
    /// None: the state could not be read (retry next period).
    key: Option<(i64, i64)>,
    outcome: Outcome,
    /// The message `record_compile_failure` records, without its prefix.
    record: Option<String>,
}

impl SnapshotCache {
    pub fn get(&self, service_id: i64) -> Lookup {
        let state = self.state.read().unwrap_or_else(|p| p.into_inner());
        match state.entries.get(&service_id) {
            None => Lookup::NotOpa,
            Some(CacheEntry {
                snapshot: Some(s), ..
            }) => Lookup::Ready(Arc::clone(s)),
            Some(e) => Lookup::NotBuilt {
                error: e.error.clone(),
            },
        }
    }

    /// The profile an engine is bound to, as of the last refresh; None: unknown or bound to no served profile.
    pub fn binding(&self, engine_id: uuid::Uuid) -> Option<i64> {
        let state = self.state.read().unwrap_or_else(|p| p.into_inner());
        state.bindings.get(&engine_id).copied()
    }

    pub async fn refresh_once(&self, pool: &PgPool) -> Result<RefreshReport, sqlx::Error> {
        let listed: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT s.id, s.policy_version, s.tag_version FROM policy.authz_services s \
              WHERE s.mechanism = 'opa' \
                 OR EXISTS (SELECT 1 FROM engine_mgmt.engines e \
                             WHERE e.authz_service_id = s.id AND e.applied_mechanism = 'opa') \
              ORDER BY s.id",
        )
        .fetch_all(pool)
        .await?;
        let ids: Vec<i64> = listed.iter().map(|(id, _, _)| *id).collect();
        // I3: the surface never trusts a cached binding; the map is replaced
        // from the engine rows on every refresh.
        let bindings: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
            "SELECT id, authz_service_id FROM engine_mgmt.engines \
              WHERE authz_service_id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(pool)
        .await?;

        let mut report = RefreshReport::default();
        {
            let mut state = self.state.write().unwrap_or_else(|p| p.into_inner());
            state.bindings = bindings.into_iter().collect();
            let keep: HashSet<i64> = ids.iter().copied().collect();
            report.dropped = state
                .entries
                .keys()
                .copied()
                .filter(|id| !keep.contains(id))
                .collect();
            report.dropped.sort_unstable();
            for id in &report.dropped {
                state.entries.remove(id);
            }
        }

        for (id, policy_version, tag_version) in listed {
            let (cached, previous) = {
                let state = self.state.read().unwrap_or_else(|p| p.into_inner());
                match state.entries.get(&id) {
                    Some(e) => (e.key, e.snapshot.clone()),
                    None => (None, None),
                }
            };
            if cached == Some((policy_version, tag_version)) {
                continue;
            }
            let built = build_entry(pool, id, previous).await;
            if let Some(msg) = &built.record {
                if let Err(e) = crate::compile_runs::record_compile_failure(
                    pool,
                    id,
                    &format!("OPA snapshot: {msg}"),
                )
                .await
                {
                    tracing::warn!(service_id = id, error = %e, "could not record the OPA snapshot failure");
                }
            }
            let entry = match built.outcome {
                Outcome::Rebuilt(s) => {
                    report.rebuilt.push(id);
                    CacheEntry {
                        key: built.key,
                        snapshot: Some(Arc::new(s)),
                        error: None,
                    }
                }
                Outcome::Spliced { snapshot, error } => {
                    report.spliced.push(id);
                    CacheEntry {
                        key: built.key,
                        snapshot: Some(Arc::new(snapshot)),
                        error: Some(error),
                    }
                }
                Outcome::Failed { error } => {
                    report.failed.push(id);
                    CacheEntry {
                        key: built.key,
                        snapshot: None,
                        error: Some(error),
                    }
                }
            };
            let mut state = self.state.write().unwrap_or_else(|p| p.into_inner());
            state.entries.insert(id, entry);
        }
        Ok(report)
    }
}

/// One profile's build, in its own `REPEATABLE READ, READ ONLY` transaction
/// (rolled back when it returns, before the caller records a failure on the
/// pool).
async fn build_entry(pool: &PgPool, id: i64, previous: Option<Arc<Snapshot>>) -> Built {
    let unread = |e: String| {
        tracing::warn!(service_id = id, error = %e, "OPA snapshot: could not read the profile");
        Built {
            key: None,
            outcome: Outcome::Failed {
                error: format!("the profile's state could not be read: {e}"),
            },
            record: None,
        }
    };
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return unread(e.to_string()),
    };
    if let Err(e) = sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
    {
        return unread(e.to_string());
    }
    let key: (i64, i64) = match sqlx::query_as(
        "SELECT policy_version, tag_version FROM policy.authz_services WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(k) => k,
        Err(e) => return unread(e.to_string()),
    };

    // (a) The stored-state checks (C8).
    match stored_state_refusal(&mut tx, id).await {
        Ok(Some(msg)) => {
            tracing::warn!(service_id = id, reason = %msg, "OPA snapshot refused: stored state");
            return Built {
                key: Some(key),
                outcome: Outcome::Failed { error: msg.clone() },
                record: Some(msg),
            };
        }
        Ok(None) => {}
        Err(e) => return unread(e),
    }

    // (b) data ++ infrastructure.
    let data_error = match snapshot_parts_via(&mut tx, id, MissingUsername::Omit).await {
        Ok(Some(parts)) => {
            let infra = parts.infra.clone();
            let mut statements = parts.data;
            statements.extend(parts.infra);
            match Snapshot::build(SnapshotInput {
                service_id: id,
                key: parts.key,
                statements,
                classified: parts.classified,
            }) {
                Ok(s) => {
                    return Built {
                        key: Some(key),
                        outcome: Outcome::Rebuilt(s),
                        record: None,
                    }
                }
                // A malformed path is a lowering error: the infrastructure
                // already lowered at this key is spliced like (c).
                Err(e) => {
                    return spliced_or_failed(id, key, e.to_string(), Ok(Some(infra)), previous)
                }
            }
        }
        Ok(None) => {
            let msg = "the profile is not a Trino or StarRocks profile".to_string();
            return Built {
                key: Some(key),
                outcome: Outcome::Failed { error: msg.clone() },
                record: Some(msg),
            };
        }
        Err(e) => e.to_string(),
    };

    // (c) The data lowering failed (C4): fresh infrastructure alone.
    let infra = lower_infra_via(&mut tx, id, MissingUsername::Omit)
        .await
        .map_err(|e| e.to_string());
    spliced_or_failed(id, key, data_error, infra, previous)
}

fn spliced_or_failed(
    id: i64,
    key: (i64, i64),
    error: String,
    infra: Result<Option<Vec<egret_perm_ir::statement::Statement>>, String>,
    previous: Option<Arc<Snapshot>>,
) -> Built {
    let failed = |error: String| {
        tracing::warn!(service_id = id, error = %error, "OPA snapshot build failed; the profile is not built");
        Built {
            key: Some(key),
            outcome: Outcome::Failed {
                error: error.clone(),
            },
            record: Some(error),
        }
    };
    let infra = match infra {
        Ok(Some(infra)) => infra,
        Ok(None) => return failed(error),
        Err(e) => {
            return failed(format!(
                "{error}; the infrastructure did not lower either: {e}"
            ))
        }
    };
    let Some(previous) = previous else {
        return failed(error);
    };
    match previous.with_infra(infra) {
        Ok(snapshot) => {
            tracing::warn!(
                service_id = id,
                error = %error,
                "OPA snapshot build failed; serving the previous data with fresh infrastructure"
            );
            Built {
                key: Some(key),
                outcome: Outcome::Spliced {
                    snapshot,
                    error: error.clone(),
                },
                record: Some(error),
            }
        }
        Err(e) => failed(format!("{error}; the infrastructure did not splice: {e}")),
    }
}

/// The first stored-state refusal, or the stored overlap count, of `id`.
async fn stored_state_refusal(
    conn: &mut sqlx::PgConnection,
    id: i64,
) -> Result<Option<String>, String> {
    let refusals = egret_policy::opa_guard::stored_state_refusals_via(conn, id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(first) = refusals.first() {
        return Ok(Some(first.message()));
    }
    let pairs = crate::overlap::stored_overlaps_via(conn)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|o| o.service_id == id)
        .count();
    Ok((pairs > 0).then(|| {
        format!(
            "the profile serves {pairs} mask/row-filter pairs whose winner the artifact does not \
             express (see GET /perm/overlaps)"
        )
    }))
}

/// Runs [`SnapshotCache::refresh_once`] every `period` until `shutdown`
/// resolves. A refresh that cannot list the profiles leaves the cache as it
/// was and is retried next period.
pub fn spawn_snapshot_refresher(
    cache: Arc<SnapshotCache>,
    pool: PgPool,
    period: std::time::Duration,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        tokio::pin!(shutdown);
        let mut tick = tokio::time::interval(period);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = tick.tick() => {
                    if let Err(e) = cache.refresh_once(&pool).await {
                        tracing::warn!(error = %e, "OPA snapshot refresh failed");
                    }
                }
            }
        }
    })
}
