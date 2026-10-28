//! Who may make an `egret.sql` task run, given that it runs as the flow's
//! creator (spec D10; user decision 2026-10-28 on final-review I1).
//!
//! `egret.sql` executes as `workflow.flows.created_by` ([`super::TaskInput`]'s
//! `principal`). Without a gate, any workspace editor could add an
//! `egret.sql` to someone else's flow and read rows with the creator's data
//! rights (a confused deputy). The gate has three doors:
//!
//! 1. **Publish** ([`refuse_publish`]): only the creator may publish a flow
//!    that contains `egret.sql`.
//! 2. **A start a person makes** (the API start, a manual replay): only the
//!    creator may start such a flow, because a start runs the *draft* source,
//!    which any editor can change.
//! 3. **A start no person makes** (a schedule or backfill tick, a webhook, a
//!    Flow trigger, a `Subflow` child): the source to run must be
//!    byte-identical to a version the creator published
//!    (`flow_versions.created_by = flows.created_by`, compared on
//!    `snapshot->>'source'`, which `publish` stores verbatim). A `Subflow`
//!    child also needs the parent's flow to have the same creator, so another
//!    user's flow cannot call it.
//!
//! An execution created from another execution's frozen source (a loop
//! iteration, a retry replay) inherits the decision its origin passed, so it
//! is not judged again. A flow with no creator (the user was deleted) is not
//! refused here: it has no principal, and the task itself refuses.

use egret_core::{Error, Result};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::models::Flow;

/// The task type this gate is about.
pub(crate) const SQL_TYPE: &str = "egret.sql";

/// Whether any leaf of `flow`, at any depth and in any of the four root
/// lists, is an `egret.sql` task.
pub(crate) fn runs_sql(flow: &Flow) -> bool {
    crate::plugins::leaves(flow)
        .iter()
        .any(|(_, task, _)| task.r#type == SQL_TYPE)
}

/// Door 1: `Err` when `flow` runs `egret.sql` and `publisher` is not its
/// creator.
pub(crate) fn refuse_publish(flow: &Flow, creator: Option<Uuid>, publisher: Uuid) -> Result<()> {
    if runs_sql(flow) && creator != Some(publisher) {
        return Err(Error::InvalidInput(
            "the flow contains an `egret.sql` task, which runs as the flow's creator, so only \
             its creator can publish it"
                .into(),
        ));
    }
    Ok(())
}

/// The flow's creator, and whether `source` is byte-identical to a version
/// that creator published.
async fn creator_and_published(
    conn: &mut PgConnection,
    flow_id: Uuid,
    source: &str,
) -> Result<(Option<Uuid>, bool)> {
    let row: Option<(Option<Uuid>, bool)> = sqlx::query_as(
        "SELECT f.created_by, \
                EXISTS (SELECT 1 FROM workflow.flow_versions v \
                         WHERE v.flow_id = f.id AND v.created_by = f.created_by \
                           AND v.snapshot->>'source' = $2) \
           FROM workflow.flows f WHERE f.id = $1",
    )
    .bind(flow_id)
    .bind(source)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| Error::Internal(anyhow::Error::new(e)))?;
    Ok(row.unwrap_or((None, false)))
}

/// The refusal a start by someone who is not the creator gets.
const NOT_THE_CREATOR: &str = "the flow contains an `egret.sql` task, which runs as the flow's \
     creator, so only its creator can start it";

/// The refusal a start no person made gets.
const NOT_PUBLISHED_BY_CREATOR: &str = "the flow contains an `egret.sql` task, which runs as \
     the flow's creator, and its current source is not a version its creator published; the \
     creator must publish it before a trigger or a `Subflow` can start it";

/// Doors 2 and 3, at the start chokepoint. `started_by` is the person who
/// started it (`Some`) or `None` for a trigger's start.
pub(crate) async fn check_start(
    conn: &mut PgConnection,
    flow: &Flow,
    flow_id: Option<Uuid>,
    source: &str,
    started_by: Option<Uuid>,
) -> Result<()> {
    let Some(flow_id) = flow_id else {
        return Ok(());
    };
    if !runs_sql(flow) {
        return Ok(());
    }
    let (creator, published) = creator_and_published(conn, flow_id, source).await?;
    let Some(creator) = creator else {
        return Ok(());
    };
    match started_by {
        Some(user) if user == creator => Ok(()),
        Some(_) => Err(Error::InvalidInput(NOT_THE_CREATOR.into())),
        None if published => Ok(()),
        None => Err(Error::InvalidInput(NOT_PUBLISHED_BY_CREATOR.into())),
    }
}

/// Door 3 for a `Subflow` child: `Some(reason)` when the child runs
/// `egret.sql` and either its source is not one its creator published or the
/// parent execution's flow has another creator.
pub(crate) async fn refuse_subflow_child(
    conn: &mut PgConnection,
    parent_execution: Uuid,
    child: &Flow,
    child_flow: Uuid,
    source: &str,
) -> Result<Option<&'static str>> {
    if !runs_sql(child) {
        return Ok(None);
    }
    let (creator, published) = creator_and_published(conn, child_flow, source).await?;
    let Some(creator) = creator else {
        return Ok(None);
    };
    let parent_creator: Option<Uuid> = sqlx::query_scalar(
        "SELECT f.created_by FROM workflow.executions e \
           JOIN workflow.flows f ON f.id = e.flow_id WHERE e.id = $1",
    )
    .bind(parent_execution)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| Error::Internal(anyhow::Error::new(e)))?
    .flatten();
    if parent_creator != Some(creator) {
        return Ok(Some(
            "the flow contains an `egret.sql` task, which runs as the flow's creator, so only \
             a flow with the same creator can call it as a `Subflow`",
        ));
    }
    if !published {
        return Ok(Some(NOT_PUBLISHED_BY_CREATOR));
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(src: &str) -> Flow {
        crate::spec::parse(src).expect("parses")
    }

    #[test]
    fn runs_sql_sees_a_nested_and_an_errors_task() {
        let top = flow(
            "id: f\nnamespace: n\ntasks:\n  - id: q\n    type: egret.sql\n    database: d\n    sql: s\n",
        );
        assert!(runs_sql(&top));
        let nested = flow(
            "id: f\nnamespace: n\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Sequential\n    \
             tasks:\n      - id: q\n        type: egret.sql\n        database: d\n        sql: s\n",
        );
        assert!(runs_sql(&nested));
        let errors = flow(
            "id: f\nnamespace: n\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    \
             message: m\nerrors:\n  - id: q\n    type: egret.sql\n    database: d\n    sql: s\n",
        );
        assert!(runs_sql(&errors));
        let none = flow(
            "id: f\nnamespace: n\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: m\n",
        );
        assert!(!runs_sql(&none));
    }

    #[test]
    fn only_the_creator_publishes_a_flow_with_egret_sql() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let sql = flow(
            "id: f\nnamespace: n\ntasks:\n  - id: q\n    type: egret.sql\n    database: d\n    sql: s\n",
        );
        assert!(refuse_publish(&sql, Some(a), a).is_ok());
        assert!(matches!(
            refuse_publish(&sql, Some(a), b),
            Err(Error::InvalidInput(_))
        ));
        assert!(refuse_publish(&sql, None, b).is_err(), "no creator: nobody");
        let plain = flow(
            "id: f\nnamespace: n\ntasks:\n  - id: l\n    type: io.kestra.plugin.core.log.Log\n    message: m\n",
        );
        assert!(refuse_publish(&plain, Some(a), b).is_ok());
    }
}
