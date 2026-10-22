//! Append-only audit trail: an async best-effort sink + a batch writer + a store.
pub mod governance;

use egret_metastore::Db;
use tokio::sync::mpsc;
use uuid::Uuid;

/// One audit event. Decisions come from the PDP decorator; mutations from the
/// grant-change handlers.
#[derive(Debug, Clone)]
pub enum AuditEvent {
    Decision {
        actor: Option<Uuid>,
        workspace_id: Option<Uuid>,
        action: String,
        resource_type: String,
        resource_id: Option<String>,
        allow: bool,
        reasons: Vec<String>,
    },
    Mutation {
        actor: Option<Uuid>,
        impersonator: Option<Uuid>,
        workspace_id: Option<Uuid>,
        action: String,
        resource_type: String,
        resource_id: Option<String>,
        detail: serde_json::Value,
    },
}

/// Build a `Mutation` event from a request principal, stamping the impersonator
/// automatically when the request is an impersonation. Handlers should build
/// mutation audit events through this rather than the struct literal, so the
/// both-parties rule holds everywhere by construction.
pub fn mutation_for(
    principal: &egret_core::Principal,
    workspace_id: Option<uuid::Uuid>,
    action: impl Into<String>,
    resource_type: impl Into<String>,
    resource_id: Option<String>,
    detail: serde_json::Value,
) -> AuditEvent {
    AuditEvent::Mutation {
        actor: Some(principal.user_id.0),
        impersonator: principal.impersonator.as_ref().map(|i| i.user_id.0),
        workspace_id,
        action: action.into(),
        resource_type: resource_type.into(),
        resource_id,
        detail,
    }
}

/// Non-blocking best-effort sink. `emit` never awaits or fails the caller.
#[derive(Clone)]
pub struct AuditSink {
    tx: mpsc::Sender<AuditEvent>,
}

impl AuditSink {
    /// Build a sink + its receiver. The caller spawns `AuditWriter::run(rx, ...)`.
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<AuditEvent>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self { tx }, rx)
    }
    /// Emit best-effort: on a full channel the event is dropped + a warn logged.
    pub fn emit(&self, ev: AuditEvent) {
        if self.tx.try_send(ev).is_err() {
            tracing::warn!("audit channel full or closed; event dropped");
        }
    }
}

/// A stored audit row (query result).
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize)]
pub struct AuditRow {
    pub id: i64,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub kind: String,
    pub actor_user_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub decision: Option<String>,
    pub reasons: Option<Vec<String>>,
    pub detail: serde_json::Value,
    pub impersonator: Option<Uuid>,
    /// The actor's current email (KI-018); `None` for a system event or a
    /// deleted user. Read at query time, not stored in the row.
    pub actor_email: Option<String>,
}

/// Filters for the query API. `None` = unfiltered.
#[derive(Debug, Clone, Default)]
pub struct AuditQuery {
    pub actor: Option<Uuid>,
    pub action: Option<String>,
    pub kind: Option<String>,
    pub decision: Option<String>,
    pub workspace: Option<Uuid>,
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    /// Leave out authorization decisions that allowed the call (KI-017):
    /// every page view writes one, and they bury the changes and denials.
    pub hide_allowed: bool,
    /// Keyset cursor: return rows strictly older than (ts, id).
    pub before: Option<(chrono::DateTime<chrono::Utc>, i64)>,
    pub limit: i64,
}

#[derive(Clone)]
pub struct AuditStore {
    db: Db,
}

impl AuditStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Multi-row INSERT of a batch of events.
    pub async fn insert_batch(&self, events: &[AuditEvent]) -> Result<(), sqlx::Error> {
        if events.is_empty() {
            return Ok(());
        }
        let mut qb = sqlx::QueryBuilder::new(
            "INSERT INTO audit.audit_log \
             (kind, actor_user_id, impersonator, workspace_id, action, resource_type, resource_id, decision, reasons, detail) ",
        );
        qb.push_values(events, |mut b, ev| match ev {
            AuditEvent::Decision {
                actor,
                workspace_id,
                action,
                resource_type,
                resource_id,
                allow,
                reasons,
            } => {
                b.push_bind("decision")
                    .push_bind(*actor)
                    .push_bind(Option::<Uuid>::None) // impersonator
                    .push_bind(*workspace_id)
                    .push_bind(action)
                    .push_bind(resource_type)
                    .push_bind(resource_id)
                    .push_bind(if *allow { "allow" } else { "deny" })
                    .push_bind(reasons)
                    .push_bind(serde_json::json!({}));
            }
            AuditEvent::Mutation {
                actor,
                impersonator,
                workspace_id,
                action,
                resource_type,
                resource_id,
                detail,
            } => {
                b.push_bind("mutation")
                    .push_bind(*actor)
                    .push_bind(*impersonator)
                    .push_bind(*workspace_id)
                    .push_bind(action)
                    .push_bind(resource_type)
                    .push_bind(resource_id)
                    .push_bind(Option::<String>::None) // decision
                    .push_bind(Option::<Vec<String>>::None) // reasons
                    .push_bind(detail);
            }
        });
        qb.build().execute(&self.db.pool).await?;
        Ok(())
    }

    /// Filtered, keyset-paginated, newest-first query.
    pub async fn query(&self, q: &AuditQuery) -> Result<Vec<AuditRow>, sqlx::Error> {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT a.*, u.email::text AS actor_email FROM audit.audit_log a \
             LEFT JOIN public.users u ON u.id = a.actor_user_id WHERE 1=1",
        );
        if let Some(a) = q.actor {
            qb.push(" AND a.actor_user_id = ").push_bind(a);
        }
        if let Some(a) = &q.action {
            qb.push(" AND a.action = ").push_bind(a.clone());
        }
        if let Some(k) = &q.kind {
            qb.push(" AND a.kind = ").push_bind(k.clone());
        }
        if let Some(d) = &q.decision {
            qb.push(" AND a.decision = ").push_bind(d.clone());
        }
        if q.hide_allowed {
            qb.push(" AND NOT (a.kind = 'decision' AND a.decision IS NOT DISTINCT FROM 'allow')");
        }
        if let Some(w) = q.workspace {
            qb.push(" AND a.workspace_id = ").push_bind(w);
        }
        if let Some(f) = q.from {
            qb.push(" AND a.ts >= ").push_bind(f);
        }
        if let Some(t) = q.to {
            qb.push(" AND a.ts <= ").push_bind(t);
        }
        if let Some((ts, id)) = q.before {
            qb.push(" AND (a.ts, a.id) < (")
                .push_bind(ts)
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        qb.push(" ORDER BY a.ts DESC, a.id DESC LIMIT ")
            .push_bind(q.limit.clamp(1, 500));
        qb.build_query_as::<AuditRow>()
            .fetch_all(&self.db.pool)
            .await
    }

    /// Delete rows older than `retention_days`; returns the deleted count.
    pub async fn prune(&self, retention_days: i64) -> Result<u64, sqlx::Error> {
        let res =
            sqlx::query("DELETE FROM audit.audit_log WHERE ts < now() - make_interval(days => $1)")
                .bind(retention_days.max(0) as i32)
                .execute(&self.db.pool)
                .await?;
        Ok(res.rows_affected())
    }
}

/// Background batch writer. Owns the receiver; drains + inserts in batches.
pub struct AuditWriter;

impl AuditWriter {
    /// Run until the sink is dropped (channel closes). Batches by size or interval.
    pub async fn run(
        mut rx: mpsc::Receiver<AuditEvent>,
        store: AuditStore,
        batch_size: usize,
        flush: std::time::Duration,
    ) {
        let mut buf: Vec<AuditEvent> = Vec::with_capacity(batch_size);
        loop {
            let deadline = tokio::time::sleep(flush);
            tokio::pin!(deadline);
            tokio::select! {
                got = rx.recv() => match got {
                    Some(ev) => {
                        buf.push(ev);
                        if buf.len() >= batch_size {
                            if let Err(e) = store.insert_batch(&buf).await { tracing::warn!(error=%e, "audit insert failed"); }
                            buf.clear();
                        }
                    }
                    None => { // channel closed: final flush
                        if !buf.is_empty() { let _ = store.insert_batch(&buf).await; }
                        break;
                    }
                },
                _ = &mut deadline => {
                    if !buf.is_empty() {
                        if let Err(e) = store.insert_batch(&buf).await { tracing::warn!(error=%e, "audit insert failed"); }
                        buf.clear();
                    }
                }
            }
        }
    }
}
