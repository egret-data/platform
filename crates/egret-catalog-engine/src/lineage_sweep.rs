//! View governance B3: the lineage **sweep**. It recomputes every view's
//! lineage, re-tags the view from it, and reports the views whose lineage
//! could not be computed. It runs on a schedule ([`LINEAGE_SWEEP_INTERVAL`],
//! wired in `egret-bin`) and on demand
//! (`POST /api/control-plane/classifications/lineage-sweep`).
//!
//! # Why a sweep
//!
//! - **Pruned branches (spec R6).** EXPLAIN reports inputs after optimization,
//!   so one arm of a `UNION` behind a planning-time-constant predicate is not
//!   seen at commit. When the predicate turns true, the next sweep sees the
//!   branch and tags the view.
//! - **Missing lineage (Task 8).** A view committed on a profile with no
//!   classified column has no lineage, so a later classification of a table it
//!   reads finds nothing to re-tag. The sweep computes it.
//! - **A narrowing replace (Task 8 M6).** The hold only adds tag rows. The
//!   sweep's re-tag replaces the view's lineage-derived rows with exactly what
//!   its current lineage derives, removals included.
//!
//! # What it never does
//!
//! - **Remove a tag on uncertainty.** A view whose EXPLAIN fails keeps its
//!   tags. Its recorded lineage is kept too when it was computed: the last
//!   lineage the engine reported keeps driving B3's re-tag on a base change,
//!   where marking it failed would stop that until a sweep succeeds. A view
//!   with no computed lineage is marked failed, with the reason.
//! - **Touch an admin's tag.** Only `from_lineage` rows (migration 0188) are
//!   rewritten ([`egret_perm_compile::view_tags`]).
//! - **Hold a transaction across an EXPLAIN.** Per view: load the view's
//!   current definition, run the EXPLAIN, then write in one short transaction.
//!
//! # One transaction per view
//!
//! Each view's lineage, its re-tag and, when a row changed, the tag-version
//! bump and recompile commit together. Per view rather than per batch:
//! - the EXPLAINs are serial and each takes up to [`EXPLAIN_BUDGET`], so a
//!   batch would have to collect them all before writing;
//! - one view whose tags would create an overlapping mask pair is refused alone,
//!   without rolling back every other view's update;
//! - the service rows a bump locks are held for one view's write, not for the
//!   whole sweep.
//!
//! A sweep that changes nothing bumps nothing, so the steady state costs the
//! plugins no download. A sweep that changes N views bumps up to N times,
//! which a plugin absorbs in its next poll.
//!
//! # Concurrency with the hold
//!
//! Each view is swept under the hold's own advisory lock
//! ([`crate::view_hold::view_lock_key`]), taken with `pg_try_advisory_lock` on
//! a connection that holds no transaction. A view whose lock is busy is being
//! judged by the hold right now; it is reported and left to the next sweep. A
//! change to the view that arrives while it is swept is refused by the hold
//! with its retry message, for at most one EXPLAIN.

use std::time::Duration;

use egret_metastore::Db;
use egret_perm_compile::lineage::{self, ExplainRunner, LineageTable, RecordedLineage, ViewRef};
use lakekeeper::api::iceberg::types::Prefix;
use lakekeeper::api::iceberg::v1::views::{LoadViewRequest, ViewParameters, ViewService};
use lakekeeper::api::ApiContext;
use lakekeeper::iceberg::{NamespaceIdent, TableIdent};
use serde::Serialize;
use sqlx::Connection as _;
use std::collections::BTreeSet;
use std::sync::Arc;
use uuid::Uuid;

use crate::bootstrap::system_request_metadata;
use crate::browse::{list_namespaces, list_tabulars, TabularKind};
use crate::mount::{EngineServer, EngineState};
use crate::view_hold::{current_definition, lock_key, EXPLAIN_BUDGET};

/// How often `egret-bin` runs the sweep. This is the longest a pruned branch
/// that starts reading a classified table (R6), or a view with no lineage over
/// a newly classified table, stays untagged. The operator guide states it.
pub const LINEAGE_SWEEP_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// One view (or one catalog listing) the sweep could not judge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SweepFailure {
    /// The registry catalog name.
    pub catalog: String,
    /// The namespace, or `None` when the catalog's namespaces could not be
    /// listed.
    pub schema: Option<String>,
    /// The view, or `None` when its namespace could not be listed.
    pub view: Option<String>,
    /// Why. The view's tags are unchanged.
    pub reason: String,
}

/// What one sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SweepReport {
    /// Views the sweep tried.
    pub views: usize,
    /// Views whose lineage it computed.
    pub computed: usize,
    /// Views whose lineage-derived tags it changed.
    pub retagged: usize,
    /// Views (or listings) it could not judge. Their tags are unchanged.
    pub failures: Vec<SweepFailure>,
}

/// Why a sweep did not run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SweepError {
    #[error("a lineage sweep is already running; its report is not shared, retry when it ends")]
    Busy,
    #[error("the lineage sweep is unavailable: {0}")]
    Unavailable(String),
}

/// The sweep, as the control-plane route sees it. A trait so the route test
/// needs no catalog.
#[async_trait::async_trait]
pub trait LineageSweeper: Send + Sync {
    /// Sweep every governed catalog.
    async fn sweep(&self) -> Result<SweepReport, SweepError>;
}

/// A sweeper for a build with no embedded catalog: every call is
/// [`SweepError::Unavailable`].
pub struct NoCatalogSweeper;

#[async_trait::async_trait]
impl LineageSweeper for NoCatalogSweeper {
    async fn sweep(&self) -> Result<SweepReport, SweepError> {
        Err(SweepError::Unavailable(
            "the embedded catalog is not enabled".into(),
        ))
    }
}

/// The real sweep (module doc).
pub struct LineageSweep {
    db: Db,
    engine: ApiContext<EngineState>,
    system_user: String,
    runner: Arc<dyn ExplainRunner>,
    running: tokio::sync::Mutex<()>,
}

/// One egret warehouse the sweep walks (and the DEFINER audit lists,
/// `crate::view_audit`).
pub(crate) struct Swept {
    pub(crate) warehouse_id: Uuid,
    pub(crate) prefix: String,
    pub(crate) catalog: String,
}

/// Every egret warehouse with a lakekeeper warehouse and a registry catalog
/// (the name the engines and the lineage use, as the hold resolves it),
/// optionally only `only`.
pub(crate) async fn governed_warehouses(
    pool: &sqlx::PgPool,
    only: Option<&[Uuid]>,
) -> Result<Vec<Swept>, sqlx::Error> {
    let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT w.id, w.lakekeeper_warehouse_id, \
                (SELECT c.name FROM registry.catalogs c WHERE c.warehouse_id = w.id \
                  ORDER BY c.name LIMIT 1) \
           FROM catalog.warehouses w \
          WHERE w.lakekeeper_warehouse_id IS NOT NULL \
            AND ($1::uuid[] IS NULL OR w.id = ANY($1)) \
          ORDER BY w.id",
    )
    .bind(only)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(warehouse_id, prefix, catalog)| {
            Some(Swept {
                warehouse_id,
                prefix,
                catalog: catalog?,
            })
        })
        .collect())
}

impl LineageSweep {
    pub fn new(
        db: Db,
        engine: ApiContext<EngineState>,
        system_user: impl Into<String>,
        runner: Arc<dyn ExplainRunner>,
    ) -> Self {
        Self {
            db,
            engine,
            system_user: system_user.into(),
            runner,
            running: tokio::sync::Mutex::new(()),
        }
    }

    /// Sweep only `warehouse_ids` (tests, which share one database).
    pub async fn sweep_warehouses(
        &self,
        warehouse_ids: &[Uuid],
    ) -> Result<SweepReport, SweepError> {
        self.run(Some(warehouse_ids), false).await
    }

    /// The sweep a classification write asks for (Task 9 fix round 1): the
    /// views with no computed lineage (missing or failed) in every warehouse
    /// the profiles of `base_warehouses` reach, `None` for all warehouses. A
    /// view with a computed lineage is re-tagged by the write itself.
    pub async fn sweep_unjudged(
        &self,
        base_warehouses: Option<&[Uuid]>,
    ) -> Result<SweepReport, SweepError> {
        let reach =
            match base_warehouses {
                None => None,
                Some(base) => Some(profile_warehouses(&self.db.pool, base).await.map_err(|e| {
                    SweepError::Unavailable(format!("listing the warehouses: {e}"))
                })?),
            };
        self.run(reach.as_deref(), true).await
    }

    async fn run(
        &self,
        only: Option<&[Uuid]>,
        unjudged_only: bool,
    ) -> Result<SweepReport, SweepError> {
        let _running = self.running.try_lock().map_err(|_| SweepError::Busy)?;
        let warehouses = self
            .warehouses(only)
            .await
            .map_err(|e| SweepError::Unavailable(format!("listing the warehouses: {e}")))?;
        let mut lock = self
            .db
            .pool
            .acquire()
            .await
            .map_err(|e| SweepError::Unavailable(format!("database: {e}")))?
            .detach();
        let mut report = SweepReport::default();
        for w in &warehouses {
            self.sweep_warehouse(&mut lock, w, unjudged_only, &mut report)
                .await;
        }
        if let Err(e) = lock.close().await {
            tracing::warn!(error = %e, "lineage sweep: closing the lock connection failed");
        }
        tracing::info!(
            views = report.views,
            computed = report.computed,
            retagged = report.retagged,
            failures = report.failures.len(),
            "lineage sweep finished"
        );
        Ok(report)
    }

    async fn warehouses(&self, only: Option<&[Uuid]>) -> Result<Vec<Swept>, sqlx::Error> {
        governed_warehouses(&self.db.pool, only).await
    }

    async fn sweep_warehouse(
        &self,
        lock: &mut sqlx::PgConnection,
        w: &Swept,
        unjudged_only: bool,
        report: &mut SweepReport,
    ) {
        let failure = |schema: Option<&str>, view: Option<&str>, reason: String| SweepFailure {
            catalog: w.catalog.clone(),
            schema: schema.map(str::to_string),
            view: view.map(str::to_string),
            reason,
        };
        let namespaces =
            match list_namespaces(self.engine.clone(), &self.system_user, &w.prefix).await {
                Ok(n) => n,
                Err(e) => {
                    report.failures.push(failure(
                        None,
                        None,
                        format!("the catalog's namespaces could not be listed: {e}"),
                    ));
                    return;
                }
            };
        for ns in namespaces {
            // Nothing can be governed under a multi-level namespace: the
            // Ranger resource has one schema level, and the hold refuses such
            // views on a classified profile.
            let [schema] = ns.parts.as_slice() else {
                continue;
            };
            let tabulars = match list_tabulars(
                self.engine.clone(),
                &self.system_user,
                &w.prefix,
                std::slice::from_ref(schema),
            )
            .await
            {
                Ok(t) => t,
                Err(e) => {
                    report.failures.push(failure(
                        Some(schema),
                        None,
                        format!("the namespace's views could not be listed: {e}"),
                    ));
                    continue;
                }
            };
            for view in tabulars
                .iter()
                .filter(|t| matches!(t.kind, TabularKind::View))
            {
                if unjudged_only && self.has_computed_lineage(w, schema, &view.name).await {
                    continue;
                }
                report.views += 1;
                match self.sweep_view(lock, w, schema, &view.name).await {
                    Ok(changed) => {
                        report.computed += 1;
                        if changed {
                            report.retagged += 1;
                        }
                    }
                    Err(reason) => {
                        report
                            .failures
                            .push(failure(Some(schema), Some(&view.name), reason))
                    }
                }
            }
        }
    }

    /// Whether `schema.name` already has a computed lineage. A read error
    /// counts as "no", so the view is swept rather than skipped.
    async fn has_computed_lineage(&self, w: &Swept, schema: &str, name: &str) -> bool {
        let view = ViewRef {
            catalog: w.catalog.clone(),
            schema: schema.to_string(),
            name: name.to_string(),
        };
        let Ok(mut conn) = self.db.pool.acquire().await else {
            return false;
        };
        matches!(
            lineage::recorded_lineage(&mut conn, &view).await,
            Ok(RecordedLineage::Computed(_))
        )
    }

    /// One view under its lock: `Ok(true)` when its derived tags changed,
    /// `Err(reason)` when its lineage could not be computed.
    async fn sweep_view(
        &self,
        lock: &mut sqlx::PgConnection,
        w: &Swept,
        schema: &str,
        name: &str,
    ) -> Result<bool, String> {
        let key = lock_key(&w.prefix, schema, name);
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
            .bind(&key)
            .fetch_one(&mut *lock)
            .await
            .map_err(|e| format!("the view's lock could not be taken: {e}"))?;
        if !got {
            return Err(
                "a change to this view was being held; the next sweep checks it".to_string(),
            );
        }
        let result = self.judge(w, schema, name).await;
        if let Err(e) = sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))")
            .bind(&key)
            .execute(&mut *lock)
            .await
        {
            tracing::warn!(error = %e, "lineage sweep: the view's lock was not released");
        }
        result
    }

    async fn judge(&self, w: &Swept, schema: &str, name: &str) -> Result<bool, String> {
        let view = ViewRef {
            catalog: w.catalog.clone(),
            schema: schema.to_string(),
            name: name.to_string(),
        };
        let computed = self.compute(w, schema, name).await;
        match computed {
            Ok((tables, columns)) => self
                .record_computed(w.warehouse_id, &view, &tables, &columns)
                .await
                .map_err(|e| format!("the lineage was computed but not recorded: {e}")),
            Err(reason) => {
                if let Err(e) = self.record_failure(&view, &reason).await {
                    tracing::warn!(?view, error = %e, "lineage sweep: the failure was not recorded");
                }
                Err(reason)
            }
        }
    }

    /// Load the view's current definition and run its EXPLAIN. No connection
    /// is held here.
    async fn compute(
        &self,
        w: &Swept,
        schema: &str,
        name: &str,
    ) -> Result<(BTreeSet<LineageTable>, Vec<String>), String> {
        let metadata = self.load_metadata(&w.prefix, schema, name).await?;
        let def = current_definition(&metadata)?;
        let default_catalog = def.default_catalog.as_deref().unwrap_or(&w.catalog);
        let default_schema = def.default_schema.as_deref().unwrap_or(schema);
        let tables = tokio::time::timeout(
            EXPLAIN_BUDGET,
            lineage::compute_lineage(
                self.runner.as_ref(),
                default_catalog,
                default_schema,
                &def.sql,
            ),
        )
        .await
        .map_err(|_| {
            format!(
                "the EXPLAIN did not answer within {} s",
                EXPLAIN_BUDGET.as_secs()
            )
        })??;
        Ok((tables, def.columns))
    }

    async fn load_metadata(
        &self,
        prefix: &str,
        schema: &str,
        name: &str,
    ) -> Result<serde_json::Value, String> {
        let unloadable = |e: String| format!("the view could not be loaded: {e}");
        let ident = TableIdent {
            namespace: NamespaceIdent::from_vec(vec![schema.to_string()])
                .map_err(|e| unloadable(format!("{e:?}")))?,
            name: name.to_string(),
        };
        let rm =
            system_request_metadata(&self.system_user).map_err(|e| unloadable(e.to_string()))?;
        let loaded = <EngineServer as ViewService<EngineState>>::load_view(
            ViewParameters {
                prefix: Some(Prefix::from(prefix.to_string())),
                view: ident,
            },
            LoadViewRequest::default(),
            self.engine.clone(),
            rm,
        )
        .await
        .map_err(|e| {
            unloadable(
                crate::error::CatalogEngineError::from_lakekeeper("load_view", e).to_string(),
            )
        })?;
        serde_json::to_value(&loaded.metadata).map_err(|e| unloadable(e.to_string()))
    }

    /// Record a computed lineage and the view's columns, re-tag the view, and,
    /// when a row changed, bump and recompile, all in one transaction.
    async fn record_computed(
        &self,
        warehouse_id: Uuid,
        view: &ViewRef,
        tables: &BTreeSet<LineageTable>,
        columns: &[String],
    ) -> Result<bool, String> {
        let s = |e: &dyn std::fmt::Display| e.to_string();
        let mut tx = self.db.pool.begin().await.map_err(|e| s(&e))?;
        egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut tx)
            .await
            .map_err(|e| s(&e))?;
        lineage::write_lineage(&mut tx, view, tables)
            .await
            .map_err(|e| s(&e))?;
        lineage::set_view_columns(&mut tx, view, Some(columns))
            .await
            .map_err(|e| s(&e))?;
        // Snapshot the overlaps before any tag row changes, as every
        // catalog-tag write does, so that only a pair this re-tag creates is
        // refused.
        let bound = egret_catalog_registry::services_bound_to_warehouse(&mut *tx, warehouse_id)
            .await
            .map_err(|e| s(&e))?;
        let before = egret_perm_compile::overlap::unit_snapshot_via(&mut tx, &bound)
            .await
            .map_err(|e| s(&e))?;
        let changed = match egret_perm_compile::retag_view_in_tx(&mut tx, view)
            .await
            .map_err(|e| s(&e))?
        {
            // The view's catalog is this warehouse's, so the re-tag wrote here.
            egret_perm_compile::ViewRetag::Changed { .. } => {
                egret_catalog_registry::bump_bound_tag_versions(&mut tx, warehouse_id)
                    .await
                    .map_err(|e| s(&e))?;
                egret_perm_compile::recompile_services_in_tx(&mut tx, &bound)
                    .await
                    .map_err(|e| format!("the view's tags cannot be assigned: {e}"))?;
                egret_perm_compile::overlap::refuse_created_overlaps_via(&mut tx, &before)
                    .await
                    .map_err(|e| format!("the view's tags cannot be assigned: {e}"))?;
                true
            }
            egret_perm_compile::ViewRetag::Unchanged => false,
            egret_perm_compile::ViewRetag::Kept { reason } => {
                return Err(format!("the view's tags were kept: {reason}"));
            }
        };
        tx.commit().await.map_err(|e| s(&e))?;
        Ok(changed)
    }

    /// A view whose lineage could not be computed keeps its tags. A computed
    /// lineage is kept too (module doc); otherwise the failure is recorded.
    async fn record_failure(&self, view: &ViewRef, reason: &str) -> Result<(), sqlx::Error> {
        let mut tx = self.db.pool.begin().await?;
        egret_perm_compile::view_tags::lock_view_tags_in_tx(&mut tx).await?;
        match lineage::recorded_lineage(&mut tx, view).await? {
            RecordedLineage::Computed(_) => {}
            RecordedLineage::Missing | RecordedLineage::Failed { .. } => {
                lineage::record_lineage_failure(&mut tx, view, reason).await?;
            }
        }
        tx.commit().await
    }
}

#[async_trait::async_trait]
impl LineageSweeper for LineageSweep {
    async fn sweep(&self) -> Result<SweepReport, SweepError> {
        self.run(None, false).await
    }
}

/// Every warehouse the profiles of `base` reach: the warehouses whose
/// registry catalog is attached to an engine bound to a profile that serves
/// one of `base`, plus `base` itself. A view reading a base table lives on an
/// engine that attaches the base's catalog, so it is in one of these.
async fn profile_warehouses(pool: &sqlx::PgPool, base: &[Uuid]) -> Result<Vec<Uuid>, sqlx::Error> {
    sqlx::query_scalar(
        "WITH profiles AS ( \
             SELECT DISTINCT e.authz_service_id AS id \
               FROM registry.catalogs c \
               JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id \
               JOIN engine_mgmt.engines e ON e.id = ea.engine_id \
              WHERE c.warehouse_id = ANY($1) AND e.authz_service_id IS NOT NULL) \
         SELECT c.warehouse_id \
           FROM registry.catalogs c \
           JOIN registry.engine_catalog_attachments ea ON ea.catalog_id = c.id \
           JOIN engine_mgmt.engines e ON e.id = ea.engine_id \
          WHERE e.authz_service_id IN (SELECT id FROM profiles) AND c.warehouse_id IS NOT NULL \
         UNION \
         SELECT unnest($1::uuid[]) \
         ORDER BY 1",
    )
    .bind(base)
    .fetch_all(pool)
    .await
}

/// How long the classification sweeper waits before it tries again after a
/// sweep that could not run (one already running, or the database).
const RETRY_AFTER: Duration = Duration::from_secs(1);

/// Run, after every classification write commits, a sweep of the views with
/// no computed lineage that the write's profiles reach (Task 9 fix round 1,
/// amended ruling). Without it, a view committed while its profile had no
/// classified column, and whose lineage then could not be computed, would
/// stay untagged until the scheduled sweep.
///
/// The writes ask through `NOTIFY` (`egret_perm_compile::view_tags::
/// notify_classification_in_tx`), which Postgres delivers only at commit, so
/// the sweep always reads the committed classification. Requests that arrive
/// while one sweep runs are merged into the next. A sweep that meets
/// [`SweepError::Busy`] (another sweep is running) is not dropped: it waits
/// [`RETRY_AFTER`] and runs again, until it has run.
///
/// A notification lost while the listener reconnects is caught by the
/// scheduled sweep ([`LINEAGE_SWEEP_INTERVAL`]).
pub fn spawn_classification_sweeps(
    sweep: Arc<LineageSweep>,
    db: Db,
) -> tokio::task::JoinHandle<()> {
    use egret_perm_compile::view_tags::{CLASSIFICATION_ALL, CLASSIFICATION_CHANNEL};
    tokio::spawn(async move {
        let mut listener = loop {
            match sqlx::postgres::PgListener::connect_with(&db.pool).await {
                Ok(mut l) => match l.listen(CLASSIFICATION_CHANNEL).await {
                    Ok(()) => break l,
                    Err(e) => tracing::warn!(error = %e, "classification sweeps: LISTEN failed"),
                },
                Err(e) => tracing::warn!(error = %e, "classification sweeps: cannot connect"),
            }
            tokio::time::sleep(RETRY_AFTER * 5).await;
        };
        // `None` inside: every warehouse.
        let mut pending: Option<Option<BTreeSet<Uuid>>> = None;
        let merge = |pending: &mut Option<Option<BTreeSet<Uuid>>>, payload: &str| {
            let ids: Option<BTreeSet<Uuid>> = if payload == CLASSIFICATION_ALL {
                None
            } else {
                Some(payload.split(',').filter_map(|p| p.parse().ok()).collect())
            };
            *pending = Some(match (pending.take(), ids) {
                (None, ids) => ids,
                (Some(None), _) | (Some(Some(_)), None) => None,
                (Some(Some(mut a)), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
            });
        };
        loop {
            if pending.is_none() {
                match listener.recv().await {
                    Ok(n) => merge(&mut pending, n.payload()),
                    Err(e) => {
                        tracing::warn!(error = %e, "classification sweeps: listener error");
                        tokio::time::sleep(RETRY_AFTER).await;
                        continue;
                    }
                }
            }
            // Fold in what else has arrived.
            while let Some(n) = listener.next_buffered() {
                merge(&mut pending, n.payload());
            }
            let Some(scope) = pending.clone() else {
                continue;
            };
            let base: Option<Vec<Uuid>> = scope.map(|s| s.into_iter().collect());
            match sweep.sweep_unjudged(base.as_deref()).await {
                Ok(report) => {
                    pending = None;
                    if !report.failures.is_empty() {
                        tracing::warn!(
                            failures = report.failures.len(),
                            first = ?report.failures.first(),
                            "classification sweep: some views' lineage could not be computed"
                        );
                    }
                }
                Err(e) => {
                    tracing::info!(error = %e, "classification sweep postponed; retrying");
                    tokio::time::sleep(RETRY_AFTER).await;
                }
            }
        }
    })
}
