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
//! **The key is remembered** whatever a deterministic outcome was (built,
//! spliced, refused, not built), so the same failure is not retried, nor
//! recorded again, every period.
//!
//! **A failure to read the state is not an outcome** ("unread"): an error
//! beginning the transaction, reading the key or running the stored-state
//! queries, or a database error (`CompileError::Db`/`LockTimeout`) from
//! either lowering. A database error aborts the Postgres transaction, so
//! nothing read after it means anything. The entry's key becomes `None`, so
//! the next period retries, and a single unread records nothing in
//! `compile_runs`. Whether the held snapshot survives depends on what the
//! build learnt before the error:
//!
//! - **The key was read and differs from the key the held snapshot was
//!   verified at** (a later step failed: the stored-state queries or either
//!   lowering): the snapshot is **dropped** and the profile denies. The state
//!   it was verified against has changed (an operator row may have been
//!   added, an admin disabled); serving it while the error lasts, which for a
//!   deterministic sqlx error is forever, would answer from state the store
//!   no longer holds.
//! - **The key could not be read** (begin, `SET TRANSACTION` or the key read
//!   failed), or it equals the verified key: the snapshot is **kept**. While
//!   the database cannot be read, no commit can land either, so the state
//!   the snapshot was verified at is the latest committed state this
//!   refresher could have seen.
//! - **Bound:** at [`MAX_UNREAD`] consecutive unread outcomes (any other
//!   outcome resets the count) any held snapshot is dropped and one failure
//!   is recorded, `"OPA snapshot: state unreadable for {n} refreshes:
//!   {err}"`. Nothing more is recorded until the count resets.
//!
//! **Which key is authoritative.** The entry's key is the key the last build
//! READ; the refresher compares against it. [`Snapshot::key`] is the key the
//! snapshot's DATA was read at: after a splice it is older than the entry's
//! key, because the data is the previous build's and only the
//! infrastructure is fresh. A caller that reports "the version this decision
//! was made at" should report the snapshot's key (the data's), and take the
//! entry's freshness from the refresher, not from the snapshot.
//!
//! **One refresh at a time.** [`SnapshotCache::refresh_once`] holds an async
//! mutex for its whole run, so two callers (the refresher and a warm-up) can
//! neither overwrite a newer build with an older one nor re-insert an entry
//! the other just dropped.
//!
//! **Which profiles** (open question (a)): those with `mechanism = 'opa'`, and
//! those with a bound engine whose `applied_mechanism = 'opa'`. A switch back
//! to Ranger is not atomic across engines; an engine not yet re-applied still
//! runs the OPA plugin and keeps being answered until it is.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use egret_perm::CompileError;
use egret_perm_eval::{Snapshot, SnapshotInput};
use egret_perm_ir::statement::Statement;
use sqlx::PgPool;

use super::build::{lower_infra_via, snapshot_parts_via, MissingUsername, SnapshotParts};

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

/// Consecutive unread refreshes after which a profile's held snapshot is
/// dropped and one failure is recorded (about 30 s at the 1 s period).
pub const MAX_UNREAD: u32 = 30;

/// How often the refresher runs [`SnapshotCache::refresh_once`] in
/// production. [`MAX_UNREAD`] assumes it.
pub const REFRESH_PERIOD: std::time::Duration = std::time::Duration::from_secs(1);

struct CacheEntry {
    /// The key the last build read (authoritative for the refresh); None:
    /// the state could not be read, so the next refresh retries.
    key: Option<(i64, i64)>,
    /// The key the last deterministic outcome was decided at: what the held
    /// snapshot, if any, was verified against. An unread leaves it as it was.
    verified: Option<(i64, i64)>,
    snapshot: Option<Arc<Snapshot>>,
    error: Option<String>,
    /// Consecutive unread outcomes; any other outcome resets it.
    unread: u32,
}

#[derive(Default)]
struct State {
    entries: HashMap<i64, CacheEntry>,
    bindings: HashMap<uuid::Uuid, i64>,
}

#[derive(Default)]
pub struct SnapshotCache {
    state: RwLock<State>,
    /// Held across [`Self::refresh_once`]: one refresh at a time.
    refreshing: tokio::sync::Mutex<()>,
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
    /// The state could not be read: retried next period, a previous snapshot
    /// kept (none: not built).
    pub unread: Vec<i64>,
}

/// How one build ended, before it is stored.
enum Outcome {
    Rebuilt(Snapshot),
    Spliced {
        snapshot: Snapshot,
        error: String,
    },
    Failed {
        error: String,
    },
    /// `read`: the key the build read before it failed; None: unknown. The
    /// held snapshot is kept only when `read` is unknown or equals the key
    /// that snapshot was verified at.
    Unread {
        cause: String,
        read: Option<(i64, i64)>,
    },
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

    /// Test seam: hold `snapshot` as `service_id`'s Ready entry.
    #[doc(hidden)]
    pub fn insert_for_test(&self, service_id: i64, snapshot: Snapshot) {
        let key = Some(snapshot.key());
        let mut state = self.state.write().unwrap_or_else(|p| p.into_inner());
        state.entries.insert(
            service_id,
            CacheEntry {
                key,
                verified: key,
                snapshot: Some(Arc::new(snapshot)),
                error: None,
                unread: 0,
            },
        );
    }

    /// Test seam: `service_id` is served over OPA but holds no snapshot.
    #[doc(hidden)]
    pub fn insert_not_built_for_test(&self, service_id: i64, error: Option<String>) {
        let mut state = self.state.write().unwrap_or_else(|p| p.into_inner());
        state.entries.insert(
            service_id,
            CacheEntry {
                key: None,
                verified: None,
                snapshot: None,
                error,
                unread: 0,
            },
        );
    }

    /// Test seam: bind `engine` to `service_id` (None: unbind).
    #[doc(hidden)]
    pub fn bind_for_test(&self, engine: uuid::Uuid, service_id: Option<i64>) {
        let mut state = self.state.write().unwrap_or_else(|p| p.into_inner());
        match service_id {
            Some(id) => state.bindings.insert(engine, id),
            None => state.bindings.remove(&engine),
        };
    }

    pub async fn refresh_once(&self, pool: &PgPool) -> Result<RefreshReport, sqlx::Error> {
        let _one_at_a_time = self.refreshing.lock().await;
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
            if let Some(msg) = self.apply(id, built, &mut report) {
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
        }
        Ok(report)
    }

    /// Store one build's outcome as `id`'s entry.
    /// Store one build's outcome as `id`'s entry. Some: a failure to record
    /// (the unread bound was reached), without its `OPA snapshot: ` prefix.
    fn apply(&self, id: i64, built: Built, report: &mut RefreshReport) -> Option<String> {
        let mut state = self.state.write().unwrap_or_else(|p| p.into_inner());
        let decided = |snapshot: Option<Snapshot>, error: Option<String>| CacheEntry {
            key: built.key,
            verified: built.key,
            snapshot: snapshot.map(Arc::new),
            error,
            unread: 0,
        };
        let mut record = None;
        let entry = match built.outcome {
            Outcome::Rebuilt(s) => {
                report.rebuilt.push(id);
                decided(Some(s), None)
            }
            Outcome::Spliced { snapshot, error } => {
                report.spliced.push(id);
                decided(Some(snapshot), Some(error))
            }
            Outcome::Failed { error } => {
                report.failed.push(id);
                decided(None, Some(error))
            }
            Outcome::Unread { cause, read } => {
                report.unread.push(id);
                let (held, verified, count) = match state.entries.get(&id) {
                    Some(e) => (e.snapshot.clone(), e.verified, e.unread + 1),
                    None => (None, None, 1),
                };
                // The key moved since the snapshot was verified: it no longer
                // describes the store.
                let still_current = read.is_none() || read == verified;
                let mut snapshot = if still_current { held } else { None };
                if count >= MAX_UNREAD {
                    snapshot = None;
                }
                if count == MAX_UNREAD {
                    record = Some(format!("state unreadable for {count} refreshes: {cause}"));
                }
                CacheEntry {
                    key: None,
                    verified,
                    snapshot,
                    error: Some(format!("the profile's state could not be read: {cause}")),
                    unread: count,
                }
            }
        };
        state.entries.insert(id, entry);
        record
    }
}

/// A database error: the transaction is aborted and the build says nothing
/// about the stored state.
fn is_transient(e: &CompileError) -> bool {
    matches!(e, CompileError::Db(_) | CompileError::LockTimeout(_))
}

/// `read`: the key the build read before `e`; None: it could not be read.
fn unread(id: i64, read: Option<(i64, i64)>, e: String) -> Built {
    tracing::warn!(service_id = id, error = %e, "OPA snapshot: could not read the profile; retrying next period");
    Built {
        key: None,
        outcome: Outcome::Unread { cause: e, read },
        record: None,
    }
}

fn failed(id: i64, key: (i64, i64), error: String) -> Built {
    tracing::warn!(service_id = id, error = %error, "OPA snapshot build failed; the profile is not built");
    Built {
        key: Some(key),
        outcome: Outcome::Failed {
            error: error.clone(),
        },
        record: Some(error),
    }
}

/// One profile's build, in its own `REPEATABLE READ, READ ONLY` transaction
/// (rolled back when it returns, before the caller records a failure on the
/// pool).
async fn build_entry(pool: &PgPool, id: i64, previous: Option<Arc<Snapshot>>) -> Built {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return unread(id, None, e.to_string()),
    };
    if let Err(e) = sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
    {
        return unread(id, None, e.to_string());
    }
    let key: (i64, i64) = match sqlx::query_as(
        "SELECT policy_version, tag_version FROM policy.authz_services WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    {
        Ok(k) => k,
        Err(e) => return unread(id, None, e.to_string()),
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
        Err(e) => return unread(id, Some(key), e),
    }

    // (b) data ++ infrastructure; (c) on a data-lowering failure, fresh
    // infrastructure alone (C4).
    match snapshot_parts_via(&mut tx, id, MissingUsername::Omit).await {
        Ok(Some(parts)) => outcome_from_parts(id, key, parts, previous),
        Ok(None) => failed(
            id,
            key,
            "the profile is not a Trino or StarRocks profile".to_string(),
        ),
        // The transaction is aborted: lowering the infrastructure on it
        // would only fail too.
        Err(e) if is_transient(&e) => unread(id, Some(key), e.to_string()),
        Err(e) => {
            let infra = lower_infra_via(&mut tx, id, MissingUsername::Omit).await;
            outcome_from_data_error(id, key, e, infra, previous)
        }
    }
}

/// (b): the lowered parts into a fresh snapshot at the key the build read. A
/// `Snapshot::build` refusal (a malformed path, an unexpanded member) is a
/// lowering error: the infrastructure lowered at this key is spliced into
/// the previous snapshot as in (c).
fn outcome_from_parts(
    id: i64,
    key: (i64, i64),
    parts: SnapshotParts,
    previous: Option<Arc<Snapshot>>,
) -> Built {
    let infra = parts.infra.clone();
    let mut statements = parts.data;
    statements.extend(parts.infra);
    match Snapshot::build(SnapshotInput {
        service_id: id,
        key: parts.key,
        statements,
        classified: parts.classified,
    }) {
        Ok(s) => Built {
            key: Some(key),
            outcome: Outcome::Rebuilt(s),
            record: None,
        },
        Err(e) => spliced_or_failed(id, key, e.to_string(), infra, previous),
    }
}

/// (c): the data lowering failed with `error`; `infra` is the infrastructure
/// lowered alone on the same transaction.
fn outcome_from_data_error(
    id: i64,
    key: (i64, i64),
    error: CompileError,
    infra: Result<Option<Vec<Statement>>, CompileError>,
    previous: Option<Arc<Snapshot>>,
) -> Built {
    if is_transient(&error) {
        return unread(id, Some(key), error.to_string());
    }
    match infra {
        Ok(Some(infra)) => spliced_or_failed(id, key, error.to_string(), infra, previous),
        Ok(None) => failed(id, key, error.to_string()),
        Err(e) if is_transient(&e) => unread(id, Some(key), e.to_string()),
        Err(e) => failed(
            id,
            key,
            format!("{error}; the infrastructure did not lower either: {e}"),
        ),
    }
}

/// The previous snapshot's data with `infra`, or not built when there is no
/// previous snapshot or the infrastructure does not build.
fn spliced_or_failed(
    id: i64,
    key: (i64, i64),
    error: String,
    infra: Vec<Statement>,
    previous: Option<Arc<Snapshot>>,
) -> Built {
    let Some(previous) = previous else {
        return failed(id, key, error);
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
        Err(e) => failed(
            id,
            key,
            format!("{error}; the infrastructure did not splice: {e}"),
        ),
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

#[cfg(test)]
mod tests {
    //! The outcome rules, without a database: C4 with and without a previous
    //! snapshot, an infrastructure failure, the `Snapshot::build` refusal arm,
    //! and a database error (unread).

    use egret_perm::{Principal, PrincipalKind};
    use egret_perm_eval::{Decision, Resource};
    use egret_perm_ir::audience::{Audience, Member, MemberName, Via};
    use egret_perm_ir::statement::{
        Derivation, Effect, InfraKind, InfraObject, Level, Path, Privilege, Source, Target,
    };
    use uuid::Uuid;

    use super::*;

    const ID: i64 = 7;
    const KEY: (i64, i64) = (5, 6);

    fn who(name: MemberName) -> Audience {
        Audience {
            include: vec![Member {
                name,
                via: Via::Direct,
            }],
            except: vec![],
        }
    }

    fn grant_read(user: &str, catalog: &str) -> Statement {
        Statement {
            source: Source::Grant {
                grant_id: Uuid::from_u128(1),
            },
            audience: who(MemberName::User(user.into())),
            target: Target::Path(Path {
                catalog_id: None,
                catalog: catalog.into(),
                schema: Level::Unnamed,
                table: Level::Unnamed,
                column: Level::Unnamed,
            }),
            effect: Effect::Allow {
                privileges: vec![Privilege::Read],
            },
            derivation: Derivation::AsDeclared,
        }
    }

    fn admin(name: MemberName) -> Statement {
        Statement {
            source: Source::Infra {
                kind: InfraKind::AdminAllAccess,
            },
            audience: who(name),
            target: Target::Infra(InfraObject::WholeEp),
            effect: Effect::EnginePrivilege {
                capability: "all".into(),
            },
            derivation: Derivation::AsDeclared,
        }
    }

    fn root() -> Statement {
        admin(MemberName::User("root".into()))
    }

    /// An infrastructure statement `Snapshot::build` refuses.
    fn unexpanded_admin() -> Statement {
        admin(MemberName::Unexpanded(Principal {
            kind: PrincipalKind::Group,
            r#ref: "admins".into(),
        }))
    }

    /// Built at key (1, 1): ann reads `lake`, root holds all-access.
    fn previous() -> Arc<Snapshot> {
        Arc::new(
            Snapshot::build(SnapshotInput {
                service_id: ID,
                key: (1, 1),
                statements: vec![grant_read("ann", "lake"), root()],
                classified: vec![],
            })
            .expect("the previous snapshot"),
        )
    }

    fn parts(data: Vec<Statement>, infra: Vec<Statement>) -> SnapshotParts {
        SnapshotParts {
            key: KEY,
            data,
            infra,
            classified: vec![],
        }
    }

    fn reads(s: &Snapshot, user: &str) -> bool {
        let t = Resource::Table {
            catalog: "lake".into(),
            schema: "s".into(),
            table: "t".into(),
            columns: vec!["id".into()],
        };
        matches!(
            s.decide(user, "SelectFromColumns", &t),
            Decision::Allow { .. }
        )
    }

    fn creates(s: &Snapshot, user: &str) -> bool {
        let t = Resource::Table {
            catalog: "lake".into(),
            schema: "s".into(),
            table: "new_t".into(),
            columns: vec![],
        };
        matches!(s.decide(user, "CreateTable", &t), Decision::Allow { .. })
    }

    fn not_built(b: &Built) -> &str {
        assert_eq!(b.key, Some(KEY), "a not-built outcome remembers its key");
        match &b.outcome {
            Outcome::Failed { error } => {
                assert_eq!(b.record.as_deref(), Some(error.as_str()));
                error
            }
            _ => panic!("expected not built"),
        }
    }

    fn user_ref_error() -> CompileError {
        CompileError::UnusableUserRef {
            r#ref: "bad@ref".into(),
        }
    }

    #[test]
    fn well_formed_parts_are_rebuilt_at_the_key_read() {
        // Arrange
        let p = parts(vec![grant_read("ann", "lake")], vec![root()]);

        // Act
        let b = outcome_from_parts(ID, KEY, p, Some(previous()));

        // Assert
        assert_eq!(b.key, Some(KEY));
        assert!(b.record.is_none());
        let Outcome::Rebuilt(s) = b.outcome else {
            panic!("expected rebuilt")
        };
        assert_eq!(s.key(), KEY);
        assert!(reads(&s, "ann") && creates(&s, "root"));
    }

    #[test]
    fn a_malformed_data_path_splices_fresh_infra_into_the_previous_snapshot() {
        // Arrange: the admin was disabled (no infra), and a data path is malformed.
        let p = parts(vec![grant_read("bob", "")], vec![]);

        // Act
        let b = outcome_from_parts(ID, KEY, p, Some(previous()));

        // Assert
        assert_eq!(b.key, Some(KEY));
        let Outcome::Spliced { snapshot, error } = b.outcome else {
            panic!("expected spliced")
        };
        assert!(error.contains("an empty catalog"), "{error}");
        assert_eq!(b.record.as_deref(), Some(error.as_str()));
        assert!(
            !creates(&snapshot, "root"),
            "the disabled admin keeps all-access"
        );
        assert!(reads(&snapshot, "ann"), "the previous data is kept");
        assert_eq!(
            snapshot.key(),
            (1, 1),
            "the data's key is the previous build's"
        );
    }

    #[test]
    fn a_malformed_data_path_with_no_previous_snapshot_is_not_built() {
        // Arrange
        let p = parts(vec![grant_read("bob", "")], vec![root()]);

        // Act
        let b = outcome_from_parts(ID, KEY, p, None);

        // Assert
        assert!(not_built(&b).contains("an empty catalog"));
    }

    #[test]
    fn a_malformed_infra_statement_is_not_built_even_with_a_previous_snapshot() {
        // Arrange
        let p = parts(vec![grant_read("ann", "lake")], vec![unexpanded_admin()]);

        // Act
        let b = outcome_from_parts(ID, KEY, p, Some(previous()));

        // Assert
        assert!(not_built(&b).contains("unexpanded"), "{}", not_built(&b));
    }

    #[test]
    fn a_data_lowering_failure_with_no_previous_snapshot_is_not_built() {
        // Act
        let b = outcome_from_data_error(ID, KEY, user_ref_error(), Ok(Some(vec![root()])), None);

        // Assert
        assert_eq!(not_built(&b), user_ref_error().to_string());
    }

    #[test]
    fn a_data_lowering_failure_splices_fresh_infra() {
        // Act
        let b = outcome_from_data_error(
            ID,
            KEY,
            user_ref_error(),
            Ok(Some(vec![])),
            Some(previous()),
        );

        // Assert
        assert_eq!(b.key, Some(KEY));
        let Outcome::Spliced { snapshot, .. } = b.outcome else {
            panic!("expected spliced")
        };
        assert!(!creates(&snapshot, "root") && reads(&snapshot, "ann"));
    }

    #[test]
    fn an_infra_lowering_failure_is_not_built_even_with_a_previous_snapshot() {
        // Act
        let b = outcome_from_data_error(
            ID,
            KEY,
            user_ref_error(),
            Err(user_ref_error()),
            Some(previous()),
        );

        // Assert
        assert!(not_built(&b).contains("the infrastructure did not lower either"));
    }

    #[test]
    fn a_database_error_at_the_verified_key_is_unread_unrecorded_and_keeps_the_held_snapshot() {
        let cache = SnapshotCache::default();
        let mut report = RefreshReport::default();
        let ready = outcome_from_parts(
            ID,
            KEY,
            parts(vec![grant_read("ann", "lake")], vec![root()]),
            None,
        );
        cache.apply(ID, ready, &mut report);
        let db = || CompileError::Db("connection reset by peer".into());
        let lock = || CompileError::LockTimeout("lock timeout".into());
        for (data, infra) in [
            (db(), Ok(Some(vec![]))),
            (lock(), Ok(Some(vec![]))),
            (user_ref_error(), Err(db())),
        ] {
            // Act
            let b = outcome_from_data_error(ID, KEY, data, infra, Some(previous()));
            assert_eq!(b.key, None, "an unread build remembers no key");
            assert!(b.record.is_none(), "an unread build records nothing");
            let mut report = RefreshReport::default();
            cache.apply(ID, b, &mut report);

            // Assert
            assert_eq!(report.unread, vec![ID]);
            let Lookup::Ready(s) = cache.get(ID) else {
                panic!(
                    "a database error denied a Ready profile: {:?}",
                    cache.get(ID)
                )
            };
            assert!(reads(&s, "ann") && creates(&s, "root"));
            let state = cache.state.read().unwrap();
            assert_eq!(state.entries[&ID].key, None, "retried next period");
        }
    }

    /// A cache holding a Ready snapshot verified at `KEY`.
    fn ready_cache() -> SnapshotCache {
        let cache = SnapshotCache::default();
        let built = outcome_from_parts(
            ID,
            KEY,
            parts(vec![grant_read("ann", "lake")], vec![root()]),
            None,
        );
        assert!(cache
            .apply(ID, built, &mut RefreshReport::default())
            .is_none());
        assert!(matches!(cache.get(ID), Lookup::Ready(_)));
        cache
    }

    fn apply(cache: &SnapshotCache, built: Built) -> Option<String> {
        cache.apply(ID, built, &mut RefreshReport::default())
    }

    fn is_ready(cache: &SnapshotCache) -> bool {
        matches!(cache.get(ID), Lookup::Ready(_))
    }

    #[test]
    fn a_moved_key_then_a_database_error_denies_rather_than_serving_the_held_snapshot() {
        let moved = (KEY.0 + 1, KEY.1);
        for built in [
            // The data lowering failed after the key was read.
            outcome_from_data_error(
                ID,
                moved,
                CompileError::Db("connection reset".into()),
                Ok(None),
                Some(previous()),
            ),
            // The infrastructure lowering failed after the key was read.
            outcome_from_data_error(
                ID,
                moved,
                user_ref_error(),
                Err(CompileError::LockTimeout("lock timeout".into())),
                Some(previous()),
            ),
            // A stored-state query failed after the key was read.
            unread(ID, Some(moved), "decode error".into()),
        ] {
            // Arrange
            let cache = ready_cache();

            // Act
            let record = apply(&cache, built);

            // Assert
            assert!(record.is_none(), "one unread records nothing");
            assert!(
                matches!(cache.get(ID), Lookup::NotBuilt { error: Some(ref e) } if e.contains("could not be read")),
                "the held snapshot was served past a moved key: {:?}",
                cache.get(ID)
            );
            assert_eq!(cache.state.read().unwrap().entries[&ID].key, None);
            // An unknown key afterwards does not bring it back.
            apply(&cache, unread(ID, None, "connection refused".into()));
            assert!(!is_ready(&cache));
        }
    }

    #[test]
    fn an_unknown_key_then_a_database_error_keeps_the_held_snapshot() {
        // Arrange
        let cache = ready_cache();

        // Act: begin, SET TRANSACTION or the key read failed.
        let record = apply(&cache, unread(ID, None, "connection refused".into()));

        // Assert
        assert!(record.is_none());
        assert!(is_ready(&cache), "{:?}", cache.get(ID));
        assert_eq!(
            cache.state.read().unwrap().entries[&ID].key,
            None,
            "retried"
        );

        // Act: next period the key reads as the verified one, and a later step fails.
        apply(&cache, unread(ID, Some(KEY), "decode error".into()));

        // Assert: the key did not move, so the snapshot still describes the store.
        assert!(is_ready(&cache), "{:?}", cache.get(ID));
    }

    #[test]
    fn the_unread_bound_drops_the_held_snapshot_and_records_once() {
        // Arrange
        let cache = ready_cache();
        let blip = || unread(ID, None, "connection refused".into());

        // Act / Assert: below the bound, kept and unrecorded.
        for n in 1..MAX_UNREAD {
            assert_eq!(apply(&cache, blip()), None, "refresh {n}");
            assert!(is_ready(&cache), "refresh {n}");
        }
        // At the bound: dropped, recorded once.
        assert_eq!(
            apply(&cache, blip()),
            Some(format!(
                "state unreadable for {MAX_UNREAD} refreshes: connection refused"
            ))
        );
        assert!(!is_ready(&cache));
        // Past it: still dropped, not recorded again.
        assert_eq!(apply(&cache, blip()), None);
        assert!(!is_ready(&cache));

        // Any other outcome resets the count.
        let rebuilt = outcome_from_parts(
            ID,
            KEY,
            parts(vec![grant_read("ann", "lake")], vec![root()]),
            None,
        );
        assert_eq!(apply(&cache, rebuilt), None);
        assert!(is_ready(&cache));
        for n in 1..MAX_UNREAD {
            assert_eq!(apply(&cache, blip()), None, "after reset, refresh {n}");
            assert!(is_ready(&cache), "after reset, refresh {n}");
        }
        assert!(apply(&cache, blip()).is_some(), "the bound fires again");
        assert!(!is_ready(&cache));
    }

    #[test]
    fn a_database_error_with_no_previous_snapshot_stays_not_built() {
        // Arrange
        let cache = SnapshotCache::default();
        let b = outcome_from_data_error(
            ID,
            KEY,
            CompileError::Db("connection reset".into()),
            Ok(None),
            None,
        );

        // Act
        cache.apply(ID, b, &mut RefreshReport::default());

        // Assert
        assert!(
            matches!(cache.get(ID), Lookup::NotBuilt { error: Some(ref e) } if e.contains("could not be read")),
            "{:?}",
            cache.get(ID)
        );
    }
}
