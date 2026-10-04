//! W9a: the retention pruner of execution files, and the orphan sweep (spec §10).
//!
//! Modelled on W11's log pruner ([`crate::logs`]' `prune`): one global
//! setting, `workflow.storage_retention_days` (default 30; **`0` keeps files
//! for ever**, so no pruner is started), an hourly pass with
//! `MissedTickBehavior::Skip`, and a per-pass budget. It runs on a task of its
//! own, so it never takes a worker's place.
//!
//! # What a pass prunes
//!
//! **A loop family, as one, on its loop root's end** (spec §10, the user's
//! ruling of 2026-10-01): a file written inside a `Loop` iteration carries the
//! iteration's own id (§3), so an iteration that ended while its root still
//! runs keeps its files and a task after the loop can still read them. A
//! candidate is a **loop root** (a row with no `loop_index`) whose family
//! still has index rows or `workflow.loop_file_values` rows, whose state is
//! terminal (`RunState::is_terminal`, the one list, as
//! `ExecutionRepo::log_status`) and whose `updated_at` is more than
//! `retention_days` old.
//!
//! **Keep a source while a dependent runs** (the user's ruling of
//! 2026-10-04): a family is skipped while any member is in the readable set
//! ([`readable_executions`], inverted) of an execution that has not ended --
//! a running iteration of its own, a running replay or retry of it, a running
//! `Subflow` child or Flow-trigger execution it triggered. It is pruned by the
//! first pass after the last such dependent ended. Since every execution is in
//! its own readable set, this also keeps every execution that has not ended:
//! **an execution that has not ended is never pruned.** The set is taken once
//! per pass, when its first candidate appears, set-based (one
//! [`readable_from_any`] query per workspace); a dependent started after that
//! is caught by a direct check just before each family's delete (a not-ended
//! execution whose `replayed_from_execution_id`, `trigger_execution_id` or
//! `parent_execution_id` names a member).
//!
//! Per family, in this order:
//!
//! 1. every member's **own** prefix is computed ([`own_prefix`]) and every index
//!    row of the family is checked to lie under its member's prefix -- so the
//!    prefix deletes below are proven to reach every indexed object;
//! 2. `delete_prefix` for each member. **A failure stops here**: the rows stay
//!    and the next pass retries the family. The pass skips it and goes on with
//!    the next family; only [`STORE_FAILURES_IN_A_ROW`] failures in a row of
//!    families that had not failed before (an outage; [`PruneState`] carries
//!    the ones that had from pass to pass) end the phase. Logged, never a crash;
//! 3. ONE statement deletes the family's index rows and its
//!    `workflow.loop_file_values` rows (a whole copy of a file's records, kept
//!    for a `Loop` over a file, Task 11) and sets `files_pruned_at` on every
//!    member -- which is what makes a later read of one of its URIs
//!    [`Unreadable::Pruned`](super::Unreadable::Pruned) rather than `Missing`.
//!
//! Objects first, then rows: the opposite order would leave objects nothing
//! points to whenever the store delete failed. (`Delete` and
//! `PurgeCurrentExecutionFiles` delete the row first for the opposite reason:
//! a running flow must stop seeing a file it deleted at once, and the object
//! they may leave behind is exactly what this sweep takes.) A pruned family
//! is not listed afterwards: `delete_prefix` took every object under its
//! prefixes, indexed or not.
//!
//! # Orphans
//!
//! Objects that were uploaded and never indexed (a refused start, a task run
//! whose outcome never committed, a `Delete` whose object delete failed): for
//! each FAILED or KILLED loop root that ended more than
//! [`StoragePruneConfig::orphan_age`] ago and was neither swept
//! (`files_swept_at`) nor pruned yet, and none of whose members is protected
//! as above (a running iteration's upload in flight is not an orphan), every
//! member's prefix is **listed**, and each object with no index row and a
//! `last_modified` older than `orphan_age` is deleted **by its key**. A key is
//! deleted only when it lies under that member's prefix and reads back as a
//! URI of that member ([`uri::parse`], round trip), whatever the listing
//! says; an object whose age the store does not say is kept. A swept family
//! gets `files_swept_at`; a skipped or failed one does not, and a later pass
//! retries it.
//!
//! # One pruner per database
//!
//! A pass runs under `pg_try_advisory_lock(`[`PRUNE_LOCK_KEY`]`)`: another
//! replica's pass, meanwhile, does nothing.
//!
//! # Never another execution's or workspace's keys
//!
//! [`FileStore::execution_prefix`] is not round-trip checked: for a namespace
//! the URI builders refuse (`a..b`, or one holding `executions.<uuid>`) it
//! names a key range that is not that execution's -- possibly another
//! execution's subtree. [`own_prefix`] refuses such a prefix: it is used only
//! when `uri::parse` reads a URI under it back as exactly that execution, and
//! `parse` takes the FIRST `executions/<uuid>` of a path, so every key under
//! an accepted prefix belongs to that execution and no other. An execution
//! with no own prefix cannot have written a file (its builders refuse), and
//! step 1 proves it has no row.
//!
//! Were two passes to run together anyway (the lock lost with its session),
//! a second `delete_prefix`, `DELETE` or `UPDATE` is a no-op.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{loop_family, readable_from_any, uri, FileStore};
use crate::executor::RunState;

/// The `pg_try_advisory_lock` key a pass takes, so one pruner runs per
/// database at a time ("egretprn" as eight ASCII bytes).
pub const PRUNE_LOCK_KEY: i64 = 0x6567_7265_7470_726e;

/// The pruner's settings. [`StoragePruneConfig::new`] fills in the spec's cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoragePruneConfig {
    /// A loop family whose root ended more than this many days ago loses its
    /// files. `0` keeps them for ever: [`spawn_storage_pruner`] starts nothing.
    pub retention_days: u32,
    /// Between passes.
    pub every: Duration,
    /// Loop roots per candidate query.
    pub batch: i64,
    /// A pass starts no new family once it has run this long (it always does
    /// at least one).
    pub pass_budget: Duration,
    /// An unindexed object older than this is an orphan; a FAILED/KILLED
    /// family is swept once its root ended more than this long ago.
    pub orphan_age: Duration,
}

impl StoragePruneConfig {
    /// `retention_days`, pruned every hour, 100 loop roots per query, a 30 s
    /// pass budget, and orphans older than 24 hours.
    pub fn new(retention_days: u32) -> Self {
        Self {
            retention_days,
            every: Duration::from_secs(3600),
            batch: 100,
            pass_budget: Duration::from_secs(30),
            orphan_age: Duration::from_secs(24 * 3600),
        }
    }
}

/// What one pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PruneReport {
    /// Executions (loop roots and iterations) whose files were pruned.
    pub executions: u64,
    /// Orphan objects deleted.
    pub orphans: u64,
    /// Families left for the next pass: a store failure, or index rows that
    /// do not lie under their execution's prefix.
    pub failed: u64,
}

/// Start the pruner, or return `None` when `cfg.retention_days` is `0` (keep
/// for ever). Stops on `cancel`, between passes.
pub fn spawn_storage_pruner(
    pool: PgPool,
    fs: Arc<FileStore>,
    cfg: StoragePruneConfig,
    cancel: CancellationToken,
) -> Option<JoinHandle<()>> {
    if cfg.retention_days == 0 {
        tracing::info!(
            "workflow storage retention is 0: execution files are kept for ever, no pruner"
        );
        return None;
    }
    tracing::info!(
        retention_days = cfg.retention_days,
        every_secs = cfg.every.as_secs(),
        "workflow storage pruner started"
    );
    Some(tokio::spawn(async move {
        let mut tick = tokio::time::interval(cfg.every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut state = PruneState::default();
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                _ = tick.tick() => match prune_pass_with(&pool, &fs, &cfg, &mut state).await {
                    Ok(r) if r != PruneReport::default() => tracing::info!(
                        executions = r.executions,
                        orphans = r.orphans,
                        failed = r.failed,
                        "workflow storage pruned"
                    ),
                    Ok(_) => {}
                    // The next pass tries again.
                    Err(e) => tracing::warn!(error = %e, "workflow storage prune pass failed"),
                },
            }
        }
        tracing::info!("workflow storage pruner stopped");
    }))
}

/// A run of store failures this long, family after family, is an outage: the
/// phase ends and the next pass retries. A shorter run is one family's
/// trouble: that family is skipped for the rest of the pass and the others go
/// on.
const STORE_FAILURES_IN_A_ROW: u32 = 3;

/// What the pruner remembers from one pass to the next: the families that
/// failed at the store (fix round 2, N1). Such a family is skipped for the
/// rest of a pass as any failing family is, but its failure in a later pass
/// is **not** outage evidence: three families that always fail (a bucket
/// policy or an object lock over their path) at the head of the oldest-first
/// order would otherwise end every pass's phase before any healthy family
/// behind them. Only a family that has not failed before counts toward
/// [`STORE_FAILURES_IN_A_ROW`]; one that is pruned or swept is forgotten.
#[derive(Debug, Default)]
pub struct PruneState {
    failed_prune: BTreeSet<Uuid>,
    failed_sweep: BTreeSet<Uuid>,
}

/// One pass with nothing remembered from an earlier one: [`prune_pass_with`]
/// over a fresh [`PruneState`].
pub async fn prune_pass(
    pool: &PgPool,
    fs: &FileStore,
    cfg: &StoragePruneConfig,
) -> Result<PruneReport> {
    prune_pass_with(pool, fs, cfg, &mut PruneState::default()).await
}

/// One pass: prune, then sweep, until there is nothing left, the budget is
/// spent or the store is down. `Err` is the database's; a store failure is
/// logged and counted in [`PruneReport::failed`]. `state` is carried from
/// pass to pass by [`spawn_storage_pruner`].
///
/// **One pruner per database.** The pass first takes the session-level
/// advisory lock [`PRUNE_LOCK_KEY`] with `pg_try_advisory_lock`; when another
/// replica holds it, the pass does nothing and answers an empty report. The
/// connection is held by a [`LockedSession`], which hands it back to the pool
/// only after `pg_advisory_unlock` answered `true`: on any other way out --
/// an unlock that failed, a panic, the pass's future dropped mid-way -- it
/// closes the connection instead, and closing a session releases its locks.
pub async fn prune_pass_with(
    pool: &PgPool,
    fs: &FileStore,
    cfg: &StoragePruneConfig,
    state: &mut PruneState,
) -> Result<PruneReport> {
    let conn = pool.acquire().await.map_err(db_err)?;
    let mut session = LockedSession {
        conn: Some(conn),
        unlocked: false,
    };
    let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(PRUNE_LOCK_KEY)
        .fetch_one(session.conn())
        .await
        .map_err(db_err)?;
    if !locked {
        session.unlocked = true; // nothing to release
        tracing::debug!("workflow storage prune pass skipped: another pruner holds the lock");
        return Ok(PruneReport::default());
    }
    let result = pass_locked(session.conn(), fs, cfg, state).await;
    let unlocked: std::result::Result<bool, _> =
        sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
            .bind(PRUNE_LOCK_KEY)
            .fetch_one(session.conn())
            .await;
    session.unlocked = matches!(unlocked, Ok(true));
    result
}

/// A pooled connection that may hold [`PRUNE_LOCK_KEY`]. Dropped without
/// `unlocked`, it is detached from the pool and closed, never handed back.
struct LockedSession {
    conn: Option<sqlx::pool::PoolConnection<sqlx::Postgres>>,
    unlocked: bool,
}

impl LockedSession {
    fn conn(&mut self) -> &mut PgConnection {
        self.conn.as_mut().expect("held until drop")
    }
}

impl Drop for LockedSession {
    fn drop(&mut self) {
        if !self.unlocked {
            if let Some(conn) = self.conn.take() {
                drop(conn.detach());
            }
        }
    }
}

async fn pass_locked(
    conn: &mut PgConnection,
    fs: &FileStore,
    cfg: &StoragePruneConfig,
    state: &mut PruneState,
) -> Result<PruneReport> {
    let start = Instant::now();
    let mut report = PruneReport::default();
    let mut units = 0u64;
    let over = |units: u64| units > 0 && start.elapsed() >= cfg.pass_budget;
    // Taken once, when the first candidate appears (a pass with nothing to
    // do costs no readable-set query).
    let mut protected: Option<BTreeSet<Uuid>> = None;

    // ── Prune ──────────────────────────────────────────────────────────────
    let mut skip: Vec<Uuid> = Vec::new();
    let mut in_a_row = 0;
    'prune: loop {
        let roots = prune_candidates(conn, cfg, &skip).await?;
        let short = (roots.len() as i64) < cfg.batch;
        for (workspace, root) in roots {
            if over(units) {
                return Ok(report);
            }
            units += 1;
            // Never twice in one pass, whatever happened to it.
            skip.push(root);
            if protected.is_none() {
                protected = Some(protected_set(conn).await?);
            }
            let protected = protected.as_ref().expect("just taken");
            match prune_family(conn, fs, workspace, root, protected).await? {
                Outcome::Done(n) => {
                    report.executions += n;
                    state.failed_prune.remove(&root);
                    in_a_row = 0;
                }
                Outcome::Kept => {}
                Outcome::Anomaly => report.failed += 1,
                Outcome::StoreDown => {
                    report.failed += 1;
                    // A family that failed before is its own trouble, not an
                    // outage (N1).
                    if state.failed_prune.insert(root) {
                        in_a_row += 1;
                        if in_a_row >= STORE_FAILURES_IN_A_ROW {
                            break 'prune;
                        }
                    }
                }
            }
        }
        if short {
            break;
        }
    }

    // ── Sweep FAILED/KILLED families ───────────────────────────────────────
    let mut skip: Vec<Uuid> = Vec::new();
    let mut in_a_row = 0;
    'sweep: loop {
        let roots = sweep_candidates(conn, cfg, &skip).await?;
        let short = (roots.len() as i64) < cfg.batch;
        for (workspace, root) in roots {
            if over(units) {
                return Ok(report);
            }
            units += 1;
            skip.push(root);
            if protected.is_none() {
                protected = Some(protected_set(conn).await?);
            }
            let protected = protected.as_ref().expect("just taken");
            match sweep_family(conn, fs, cfg, workspace, root, protected).await? {
                Outcome::Done(n) => {
                    report.orphans += n;
                    state.failed_sweep.remove(&root);
                    in_a_row = 0;
                }
                Outcome::Kept | Outcome::Anomaly => {}
                Outcome::StoreDown => {
                    report.failed += 1;
                    if state.failed_sweep.insert(root) {
                        in_a_row += 1;
                        if in_a_row >= STORE_FAILURES_IN_A_ROW {
                            break 'sweep;
                        }
                    }
                }
            }
        }
        if short {
            break;
        }
    }
    Ok(report)
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}

fn terminal() -> Vec<&'static str> {
    RunState::ALL
        .iter()
        .filter(|s| s.is_terminal())
        .map(|s| s.wire())
        .collect()
}

/// **The user's ruling of 2026-10-04: keep a source while a dependent runs.**
/// Every execution in the readable set ([`readable_executions`], the one
/// relation storage reads by) of an execution that has NOT ended: its loop
/// family, its replay and retry chain, its trigger execution and that
/// trigger's loop family. A family with a member here is neither pruned nor
/// swept; it is taken by the first pass after its last dependent ended.
///
/// Set-based: one [`readable_from_any`] query per workspace that has an
/// execution that has not ended (the same SQL as `readable_executions`,
/// seeded with all of them; the union of their readable sets).
async fn protected_set(conn: &mut PgConnection) -> Result<BTreeSet<Uuid>> {
    let live: Vec<(Uuid, Vec<Uuid>)> = sqlx::query_as(
        "SELECT workspace_id, array_agg(id) FROM workflow.executions \
          WHERE NOT (state = ANY($1)) GROUP BY workspace_id",
    )
    .bind(terminal())
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let mut out = BTreeSet::new();
    for (workspace, executions) in live {
        out.extend(readable_from_any(conn, workspace, &executions).await?);
    }
    Ok(out)
}

/// Loop roots that ended more than `retention_days` ago and whose family
/// still has index rows or `Loop` values (`workflow.loop_file_values`: a
/// `Loop` over a trigger's or a replay source's file writes values and no
/// file), oldest first. The walk starts from those rows (both tables hold
/// only what is not pruned yet) and climbs `parent_execution_id` while the
/// row is a loop iteration, as `index::loop_root` does.
async fn prune_candidates(
    conn: &mut PgConnection,
    cfg: &StoragePruneConfig,
    skip: &[Uuid],
) -> Result<Vec<(Uuid, Uuid)>> {
    // Saturated: above about 2.4 million days `make_interval` leaves the
    // timestamp range and the query fails (logged, retried); `0` is "for ever".
    let days = i32::try_from(cfg.retention_days).unwrap_or(i32::MAX);
    sqlx::query_as(
        "WITH RECURSIVE up(id, parent, loop_index, ws) AS ( \
           SELECT e.id, e.parent_execution_id, e.loop_index, e.workspace_id \
             FROM workflow.executions e \
            WHERE e.id IN (SELECT execution_id FROM workflow.execution_files \
                           UNION SELECT execution_id FROM workflow.loop_file_values) \
           UNION \
           SELECT e.id, e.parent_execution_id, e.loop_index, e.workspace_id \
             FROM workflow.executions e JOIN up ON e.id = up.parent AND e.workspace_id = up.ws \
            WHERE up.loop_index IS NOT NULL \
         ) \
         SELECT r.workspace_id, r.id FROM workflow.executions r \
          WHERE r.id IN (SELECT id FROM up WHERE loop_index IS NULL) \
            AND r.state = ANY($1) \
            AND r.updated_at < now() - make_interval(days => $2) \
            AND NOT (r.id = ANY($3)) \
          ORDER BY r.updated_at, r.id \
          LIMIT $4",
    )
    .bind(terminal())
    .bind(days)
    .bind(skip)
    .bind(cfg.batch)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

/// FAILED or KILLED loop roots that ended more than `orphan_age` ago, neither
/// swept nor pruned yet, oldest first. The states are literals, not bound, so
/// the planner matches migration 0168's partial index
/// `workflow_executions_unswept_idx` (the test
/// `the_swept_states_are_the_wire_spellings` pins the spelling).
async fn sweep_candidates(
    conn: &mut PgConnection,
    cfg: &StoragePruneConfig,
    skip: &[Uuid],
) -> Result<Vec<(Uuid, Uuid)>> {
    sqlx::query_as(
        "SELECT workspace_id, id FROM workflow.executions \
          WHERE loop_index IS NULL AND files_swept_at IS NULL AND files_pruned_at IS NULL \
            AND state IN ('FAILED', 'KILLED') \
            AND updated_at < now() - make_interval(secs => $1) \
            AND NOT (id = ANY($2)) \
          ORDER BY updated_at, id \
          LIMIT $3",
    )
    .bind(cfg.orphan_age.as_secs_f64())
    .bind(skip)
    .bind(cfg.batch)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

/// A member of a loop family, with what its prefix is made of.
struct Member {
    id: Uuid,
    namespace: String,
    flow: String,
}

/// `root` and every iteration under it ([`loop_family`]), with their rows.
async fn members(conn: &mut PgConnection, workspace: Uuid, root: Uuid) -> Result<Vec<Member>> {
    let family = loop_family(conn, workspace, root).await?;
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, namespace, flow_key \
           FROM workflow.executions WHERE workspace_id = $1 AND id = ANY($2)",
    )
    .bind(workspace)
    .bind(&family)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .map(|(id, namespace, flow)| Member {
            id,
            namespace,
            flow,
        })
        .collect())
}

/// An execution's own prefix: the store's (`s3://…/`, for `delete_prefix`)
/// and the URIs' (`kestra:///…/executions/{id}/`), or `None` when
/// [`FileStore::execution_prefix`] would name a key range that is not
/// provably this execution's alone. See the module doc.
pub(crate) fn own_prefix(
    fs: &FileStore,
    workspace: Uuid,
    namespace: &str,
    flow: &str,
    execution: Uuid,
) -> Option<(String, String)> {
    let ns_path = uri::namespace_as_path(namespace);
    let flow_slug = uri::slugify_str(flow);
    let uri_prefix = format!(
        "{}{ns_path}/{flow_slug}/executions/{}/",
        uri::SCHEME_PREFIX,
        execution.hyphenated()
    );
    let parsed = uri::parse(&format!("{uri_prefix}x")).ok()?;
    let own = parsed.execution == execution
        && parsed.ns_path == ns_path
        && parsed.flow_slug == flow_slug
        && parsed.tail == uri::Tail::Execution { file: "x".into() };
    let store_prefix = fs.execution_prefix(workspace, namespace, flow, execution);
    // The two spell the same path: the store's is `{prefix}{ws}/` + the URI's.
    let same = store_prefix.ends_with(&uri_prefix[uri::SCHEME_PREFIX.len()..]);
    (own && same).then_some((store_prefix, uri_prefix))
}

/// What became of one family.
enum Outcome {
    /// Pruned (the number of executions) or swept (the number of orphans).
    Done(u64),
    /// A member is in the readable set of an execution that has not ended:
    /// kept, and taken by a later pass.
    Kept,
    /// Index rows outside their execution's prefix: kept, logged.
    Anomaly,
    /// The store failed: kept, retried next pass.
    StoreDown,
}

fn any_protected(family: &[Member], protected: &BTreeSet<Uuid>) -> bool {
    family.iter().any(|m| protected.contains(&m.id))
}

async fn prune_family(
    conn: &mut PgConnection,
    fs: &FileStore,
    workspace: Uuid,
    root: Uuid,
    protected: &BTreeSet<Uuid>,
) -> Result<Outcome> {
    let family = members(conn, workspace, root).await?;
    // The root's own end is the candidate query's. A member some execution
    // that has not ended may read -- a running iteration of its own, a
    // running replay of it, a running child it triggered -- keeps the family.
    if any_protected(&family, protected) {
        return Ok(Outcome::Kept);
    }
    let prefixes: Vec<Option<(String, String)>> = family
        .iter()
        .map(|m| own_prefix(fs, workspace, &m.namespace, &m.flow, m.id))
        .collect();
    let ids: Vec<Uuid> = family.iter().map(|m| m.id).collect();
    let uri_prefixes: Vec<Option<String>> = prefixes
        .iter()
        .map(|p| p.as_ref().map(|(_, u)| u.clone()))
        .collect();
    // Step 1: every index row lies under its member's own prefix, so the
    // deletes below reach every indexed object.
    let stray: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow.execution_files f \
           JOIN UNNEST($2::uuid[], $3::text[]) AS p(id, pre) ON f.execution_id = p.id \
          WHERE f.workspace_id = $1 \
            AND (p.pre IS NULL OR left(f.uri, length(p.pre)) <> p.pre)",
    )
    .bind(workspace)
    .bind(&ids)
    .bind(&uri_prefixes)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    if stray > 0 {
        tracing::error!(
            %workspace,
            %root,
            stray,
            "workflow storage: index rows of this loop family lie outside their execution's \
             prefix; its files are kept"
        );
        return Ok(Outcome::Anomaly);
    }
    // The protected set is a snapshot taken when the pass began. A
    // dependent started since -- a replay of a member, a child it triggered,
    // an iteration or retry under it -- is looked for directly, once more,
    // just before the delete (fix round 2, m2).
    let late: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM workflow.executions \
          WHERE workspace_id = $1 AND NOT (state = ANY($2)) \
            AND (replayed_from_execution_id = ANY($3) OR trigger_execution_id = ANY($3) \
                 OR parent_execution_id = ANY($3)))",
    )
    .bind(workspace)
    .bind(terminal())
    .bind(&ids)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    if late {
        return Ok(Outcome::Kept);
    }
    // Step 2: the objects.
    for (store_prefix, _) in prefixes.iter().flatten() {
        if let Err(e) = fs.store.delete_prefix(store_prefix).await {
            tracing::warn!(
                %workspace,
                %root,
                error = %e,
                "workflow storage: an execution's files could not be deleted; \
                 its index rows are kept and the next pass retries"
            );
            return Ok(Outcome::StoreDown);
        }
    }
    // Step 3: the rows, in one statement.
    sqlx::query(
        "WITH files AS ( \
           DELETE FROM workflow.execution_files WHERE workspace_id = $1 AND execution_id = ANY($2) \
         ), loop_values AS ( \
           DELETE FROM workflow.loop_file_values WHERE execution_id = ANY($2) \
         ) \
         UPDATE workflow.executions SET files_pruned_at = now() \
          WHERE workspace_id = $1 AND id = ANY($2)",
    )
    .bind(workspace)
    .bind(&ids)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    // No listing for orphans after this: `delete_prefix` took every object
    // under these prefixes, indexed or not.
    Ok(Outcome::Done(family.len() as u64))
}

/// Sweep one FAILED/KILLED family: every member's prefix, then
/// `files_swept_at` on every member. A protected family or a store failure
/// leaves `files_swept_at` NULL, so a later pass sweeps it.
async fn sweep_family(
    conn: &mut PgConnection,
    fs: &FileStore,
    cfg: &StoragePruneConfig,
    workspace: Uuid,
    root: Uuid,
    protected: &BTreeSet<Uuid>,
) -> Result<Outcome> {
    let family = members(conn, workspace, root).await?;
    // A running iteration's upload in flight is not an orphan (I1).
    if any_protected(&family, protected) {
        return Ok(Outcome::Kept);
    }
    let mut deleted = 0;
    for m in &family {
        match sweep_member(conn, fs, cfg, workspace, m).await? {
            Ok(n) => deleted += n,
            Err(()) => return Ok(Outcome::StoreDown),
        }
    }
    let ids: Vec<Uuid> = family.iter().map(|m| m.id).collect();
    sqlx::query(
        "UPDATE workflow.executions SET files_swept_at = now() \
          WHERE workspace_id = $1 AND id = ANY($2)",
    )
    .bind(workspace)
    .bind(&ids)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    Ok(Outcome::Done(deleted))
}

/// List one member's own prefix and delete, by key, each object of that
/// member with no index row and older than `orphan_age`. `Ok(Err(()))` is
/// a store failure (logged).
async fn sweep_member(
    conn: &mut PgConnection,
    fs: &FileStore,
    cfg: &StoragePruneConfig,
    workspace: Uuid,
    m: &Member,
) -> Result<std::result::Result<u64, ()>> {
    let Some((store_prefix, _)) = own_prefix(fs, workspace, &m.namespace, &m.flow, m.id) else {
        return Ok(Ok(0));
    };
    let bucket_root = format!("s3://{}/", fs.bucket);
    let Some(key_prefix) = store_prefix.strip_prefix(&bucket_root) else {
        return Ok(Ok(0));
    };
    let listing = match fs.store.list(key_prefix, None).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(%workspace, execution = %m.id, error = %e,
                "workflow storage: an execution's prefix could not be listed for orphans");
            return Ok(Err(()));
        }
    };
    let workspace_root = format!("{}{}/", fs.prefix, workspace.hyphenated());
    let cutoff = chrono::Utc::now()
        - chrono::Duration::from_std(cfg.orphan_age).unwrap_or(chrono::Duration::MAX);
    // (key, uri) of every old object that reads back as this member's file.
    let mut old: Vec<(String, String)> = Vec::new();
    for o in &listing.objects {
        if !o.key.starts_with(key_prefix) {
            continue;
        }
        let Some(path) = o.key.strip_prefix(&workspace_root) else {
            continue;
        };
        let u = format!("{}{path}", uri::SCHEME_PREFIX);
        let ours = uri::parse(&u).is_ok_and(|p| p.execution == m.id && p.to_uri() == u);
        let aged = o
            .last_modified
            .as_deref()
            .and_then(parse_last_modified)
            .is_some_and(|t| t < cutoff);
        if ours && aged {
            old.push((o.key.clone(), u));
        }
    }
    if old.is_empty() {
        return Ok(Ok(0));
    }
    let uris: Vec<&str> = old.iter().map(|(_, u)| u.as_str()).collect();
    let indexed: BTreeSet<String> = sqlx::query_scalar(
        "SELECT uri FROM workflow.execution_files WHERE workspace_id = $1 AND uri = ANY($2)",
    )
    .bind(workspace)
    .bind(&uris)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?
    .into_iter()
    .collect();
    let mut deleted = 0;
    for (key, u) in &old {
        if indexed.contains(u) {
            continue;
        }
        if let Err(e) = fs.store.delete(&format!("{bucket_root}{key}")).await {
            tracing::warn!(%workspace, execution = %m.id, uri = %u, error = %e,
                "workflow storage: an orphan could not be deleted");
            return Ok(Err(()));
        }
        deleted += 1;
    }
    Ok(Ok(deleted))
}

/// `ObjectMeta::last_modified`: RFC 3339 (S3's `DateTime` display), or the
/// epoch seconds it falls back to. Anything else is `None`, and the object is
/// kept: an age the store does not say is not old.
fn parse_last_modified(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&chrono::Utc));
    }
    let secs: f64 = s.parse().ok()?;
    chrono::DateTime::from_timestamp(secs.trunc() as i64, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> FileStore {
        FileStore {
            store: Arc::new(egret_storage::MemStore::new()),
            bucket: "b".into(),
            prefix: "workflow/".into(),
            caps: super::super::Caps::default(),
            retention_days: 30,
        }
    }

    #[test]
    fn an_own_prefix_is_the_execution_prefix_and_names_only_that_execution() {
        let (ws, e) = (Uuid::new_v4(), Uuid::new_v4());
        let (store, uris) = own_prefix(&fs(), ws, "a.b", "My_Flow", e).expect("own");
        assert_eq!(store, fs().execution_prefix(ws, "a.b", "My_Flow", e));
        assert_eq!(uris, format!("kestra:///a/b/my-flow/executions/{e}/"));
    }

    /// The namespaces the URI builders refuse: `execution_prefix` would name
    /// another execution's subtree (`a.f.executions.<other>`) or a path that
    /// does not parse (`a..b`, `a.`).
    #[test]
    fn a_prefix_that_is_not_provably_the_executions_own_is_refused() {
        let (ws, e, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        for ns in [
            format!("a.f.executions.{other}"),
            "a..b".to_string(),
            "a.".to_string(),
        ] {
            assert_eq!(own_prefix(&fs(), ws, &ns, "tasks", e), None, "{ns}");
        }
    }

    /// `sweep_candidates` spells the states as literals (for the partial
    /// index); they must be the wire spellings.
    #[test]
    fn the_swept_states_are_the_wire_spellings() {
        assert_eq!(RunState::Failed.wire(), "FAILED");
        assert_eq!(RunState::Killed.wire(), "KILLED");
        assert!(RunState::Failed.is_terminal() && RunState::Killed.is_terminal());
    }

    #[test]
    fn last_modified_reads_rfc3339_and_epoch_seconds_and_nothing_else() {
        let t = parse_last_modified("2026-10-03T10:00:00Z").unwrap();
        assert_eq!(t.to_rfc3339(), "2026-10-03T10:00:00+00:00");
        assert!(parse_last_modified("2026-10-03T10:00:00.123+02:00").is_some());
        assert_eq!(
            parse_last_modified("1700000000.5").unwrap().timestamp(),
            1_700_000_000
        );
        assert_eq!(parse_last_modified("yesterday"), None);
    }
}
