//! **Replace an execution's labels** -- workflow package 7d (spec
//! `docs/en/specs/2026-11-16-workflow-delete-labels-logs-design.md` Q3, Q8,
//! D5; Kestra `EC:2352-2402`). Under the execution's row lock in the mode
//! `claim::load` takes (`FOR NO KEY UPDATE`), so it serialises with a
//! settle and a restart; not `claim::load` itself, which plans the source
//! (plan S5). The body replaces the visible list; the visible `system.*`
//! labels are kept; `labels_replaced` becomes true (migration 0231).

use egret_core::{Error, Result};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::labels::{edit_labels_refusal, merge_system_labels, own_labels, visible_labels};
use crate::models::Label;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LabelsSet {
    /// The visible keys before, in order.
    pub keys_before: Vec<String>,
    /// The stored keys after (the body, then the kept `system.*`).
    pub keys_after: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LabelsRefused {
    /// D5: not idle; the execution's state.
    NotIdle(String),
    /// D5: the sentence of `labels::edit_labels_refusal`.
    Invalid(String),
}

/// `Err(NotFound)` for a missing or foreign id.
pub async fn set_execution_labels(
    pool: &PgPool,
    workspace: Uuid,
    id: Uuid,
    body: &[Label],
) -> Result<std::result::Result<LabelsSet, LabelsRefused>> {
    let mut tx = pool.begin().await.map_err(db_err)?;
    let row = sqlx::query(
        "SELECT state, source, labels, labels_replaced \
           FROM workflow.executions WHERE id = $1 AND workspace_id = $2 FOR NO KEY UPDATE",
    )
    .bind(id)
    .bind(workspace)
    .fetch_optional(&mut *tx)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound("execution".into()))?;
    // A second statement: `execution_is_idle` is STABLE, so it reads the snapshot
    // of the statement that calls it. Taken here, after the lock wait, it sees a
    // restart or settle that committed while the lock was held elsewhere.
    let idle: bool = sqlx::query_scalar("SELECT COALESCE(workflow.execution_is_idle($1), false)")
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
    if !idle {
        return Ok(Err(LabelsRefused::NotIdle(row.get("state"))));
    }
    // A frozen source that no longer parses contributes no flow labels.
    let flow_labels = crate::spec::parse(row.get::<&str, _>("source"))
        .map(|f| f.labels)
        .unwrap_or_default();
    let own = own_labels(row.get::<Option<serde_json::Value>, _>("labels").as_ref());
    let visible = visible_labels(&flow_labels, &own, row.get("labels_replaced"));
    if let Some(why) = edit_labels_refusal(body, &visible) {
        return Ok(Err(LabelsRefused::Invalid(why)));
    }
    let stored = merge_system_labels(body, &visible);
    sqlx::query("UPDATE workflow.executions SET labels = $2, labels_replaced = true WHERE id = $1 AND workspace_id = $3")
        .bind(id)
        .bind(serde_json::to_value(&stored).expect("labels serialise"))
        .bind(workspace)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    tx.commit().await.map_err(db_err)?;
    Ok(Ok(LabelsSet {
        keys_before: visible.into_iter().map(|l| l.key).collect(),
        keys_after: stored.into_iter().map(|l| l.key).collect(),
    }))
}

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
}
