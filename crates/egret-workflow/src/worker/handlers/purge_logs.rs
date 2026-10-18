//! `core.log.PurgeLogs` (group C1 Task 4, spec §2.3): delete execution log
//! lines in bulk, scoped to the flow's own namespace.
//!
//! # Where egret differs, on purpose
//!
//! - **The scope** (the user's ruling, 2026-10-18). Every delete is limited
//!   to the running execution's workspace. With no `namespace`, the scope is
//!   the flow's own namespace and its children on a dot boundary (Kestra:
//!   every namespace, after `acl().allowAllNamespaces()`, which Kestra OSS
//!   does not check). A given `namespace` must pass
//!   [`authorize`]`(…, Resource::Logs, Access::Purge)`, the rule `PurgeFiles`
//!   and `PurgeKV` use, or the run FAILS naming the rule and nothing is
//!   deleted.
//! - **Non-execution logs** (D2): every line in `workflow.task_logs` belongs
//!   to an execution, so `purgeNonExecutionLogs` deletes nothing and
//!   `nonExecutionLogsCount` is always 0.
//! - **`batchSize`** applies on Postgres (Kestra: MySQL only).
//! - **The window** is `[startDate, endDate]`, both inclusive, as Kestra's
//!   (spec §2.3, amended 2026-10-18). With `flowId` the namespace matches
//!   exactly (Kestra's rule); its children are reached only without one.
//! - **The flow's namespace is read from the execution row**
//!   (`workflow.executions.namespace`, by id and workspace), never from the
//!   render scope, so the task runs whether or not `[storage]` is configured.

use egret_core::Result;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use uuid::Uuid;

use super::super::{Handlers, Outcome, TaskHandler, TaskInput};
use crate::logs::{LogLevel, LogRepo, PurgeQuery};
use crate::models::Property;
use crate::runners::RenderContext;
use crate::storage::nsfiles::access::{authorize, Access, Actor, Resource};

/// `io.kestra.plugin.core.log.PurgeLogs`.
pub const TYPE_ID: &str = "io.kestra.plugin.core.log.PurgeLogs";

/// `PurgeLogs.java:75-130`, every property a `Property<…>`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Spec {
    namespace: Option<Property<String>>,
    #[serde(rename = "flowId")]
    flow_id: Option<Property<String>>,
    #[serde(rename = "executionId")]
    execution_id: Option<Property<String>>,
    #[serde(rename = "logLevels")]
    log_levels: Option<Property<Vec<String>>>,
    #[serde(rename = "startDate")]
    start_date: Option<Property<String>>,
    #[serde(rename = "endDate")]
    end_date: Option<Property<String>>,
    #[serde(rename = "purgeExecutionLogs")]
    purge_execution_logs: Option<Property<bool>>,
    #[serde(rename = "purgeNonExecutionLogs")]
    purge_non_execution_logs: Option<Property<bool>>,
    #[serde(rename = "batchSize")]
    batch_size: Option<Property<i64>>,
}

/// Register `PurgeLogs`, which needs the pool. `runtime::handlers_for` calls
/// this for every deployment, beside `kv::register`.
pub fn register(handlers: &mut Handlers, pool: PgPool) {
    handlers.add_publishable(Arc::new(PurgeLogs::new(pool)));
}

#[derive(Debug)]
pub struct PurgeLogs {
    pool: PgPool,
    repo: LogRepo,
}

impl PurgeLogs {
    pub fn new(pool: PgPool) -> Self {
        Self {
            repo: LogRepo::new(pool.clone()),
            pool,
        }
    }

    /// The running execution's namespace, from its row (by id AND
    /// workspace); `None` when there is no such execution.
    async fn own_namespace(&self, task: &TaskInput) -> Result<Option<String>> {
        sqlx::query_scalar(
            "SELECT namespace FROM workflow.executions WHERE id = $1 AND workspace_id = $2",
        )
        .bind(task.execution_id)
        .bind(task.workspace_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| {
            egret_core::Error::Internal(anyhow::anyhow!(
                "{TYPE_ID}: the database is unavailable: {e}"
            ))
        })
    }
}

fn render<T: serde::de::DeserializeOwned>(
    p: &Option<Property<T>>,
    name: &str,
    cx: &RenderContext,
) -> std::result::Result<Option<T>, String> {
    p.as_ref()
        .map(|p| p.render(cx).map_err(|e| format!("`{name}`: {e}")))
        .transpose()
}

/// Java's `ZonedDateTime.parse`, as `TypeConverter.toZonedDateTime` reads a
/// rendered date: RFC 3339, with an optional trailing `[Region/City]`.
fn parse_date(s: &str, name: &str) -> std::result::Result<chrono::DateTime<chrono::Utc>, String> {
    let t = s.trim();
    let t = match (t.ends_with(']'), t.rfind('[')) {
        (true, Some(i)) => &t[..i],
        _ => t,
    };
    chrono::DateTime::parse_from_rfc3339(t)
        .map(|d| d.with_timezone(&chrono::Utc))
        .map_err(|e| {
            format!(
                "`{name}` must be an ISO 8601 date-time such as 2025-01-01T00:00:00Z, and it is \
                 {s:?}: {e}"
            )
        })
}

/// The scope decision and every property, without a database: `Ok(None)`
/// when `purgeExecutionLogs` is false (nothing to delete), `Err` the
/// message of a FAILED run. The namespace is authorized first, as Kestra
/// checks its ACL before it renders anything else.
pub(crate) fn plan(
    spec: &Spec,
    cx: &RenderContext,
    workspace: Uuid,
    own: &str,
) -> std::result::Result<Option<PurgeQuery>, String> {
    let target = render(&spec.namespace, "namespace", cx)?.unwrap_or_else(|| own.to_string());
    let grant = authorize(
        workspace,
        Actor::Flow { namespace: own },
        &target,
        Resource::Logs,
        Access::Purge,
    )?;
    let flow_key = render(&spec.flow_id, "flowId", cx)?;
    let execution_id = render(&spec.execution_id, "executionId", cx)?
        .map(|s| {
            Uuid::parse_str(s.trim()).map_err(|_| {
                format!("`executionId` must be an execution id (a UUID), and it is {s:?}")
            })
        })
        .transpose()?;
    let levels = render(&spec.log_levels, "logLevels", cx)?
        .map(|ls| {
            ls.iter()
                .map(|l| {
                    LogLevel::parse(l).ok_or_else(|| {
                        format!(
                            "`logLevels`: {l:?} is not a log level; one of TRACE, DEBUG, \
                             INFO, WARN or ERROR"
                        )
                    })
                })
                .collect::<std::result::Result<Vec<_>, _>>()
        })
        .transpose()?
        // Kestra: `logLevelsRendered.isEmpty() ? null : …` -- no filter.
        .filter(|ls| !ls.is_empty());
    let start = render(&spec.start_date, "startDate", cx)?
        .map(|s| parse_date(&s, "startDate"))
        .transpose()?;
    let end = match render(&spec.end_date, "endDate", cx)? {
        Some(s) => parse_date(&s, "endDate")?,
        None => return Err("`endDate` must not be null".to_string()),
    };
    let execution_logs = render(&spec.purge_execution_logs, "purgeExecutionLogs", cx)?;
    // Rendered for its errors only: D2, there are no non-execution logs.
    render(&spec.purge_non_execution_logs, "purgeNonExecutionLogs", cx)?;
    let batch = render(&spec.batch_size, "batchSize", cx)?;
    if let Some(n) = batch.filter(|n| *n < 1) {
        return Err(format!("`batchSize` must be 1 or more, and it is {n}"));
    }
    if !execution_logs.unwrap_or(true) {
        return Ok(None);
    }
    Ok(Some(PurgeQuery {
        workspace_id: grant.workspace(),
        namespaces: vec![grant.namespace().to_string()],
        flow_key,
        execution_id,
        levels,
        start,
        end,
        batch,
    }))
}

fn counts(n: u64) -> serde_json::Value {
    serde_json::json!({"count": n, "executionLogsCount": n, "nonExecutionLogsCount": 0})
}

#[async_trait::async_trait]
impl TaskHandler for PurgeLogs {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let spec: Spec = match serde_json::from_value(task.spec.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::failed_with(format!("{TYPE_ID}: {e}"))),
        };
        // A database failure is infrastructure (`Err`, re-claimed).
        let Some(own) = self.own_namespace(task).await? else {
            return Ok(Outcome::failed_with(
                "this run is not a task run of a flow's execution, so it has no namespace to \
                 purge logs from",
            ));
        };
        let q = match plan(&spec, &task.render, task.workspace_id, &own) {
            Ok(q) => q,
            Err(e) => return Ok(Outcome::failed_with(e)),
        };
        let Some(q) = q else {
            task.log
                .info("purged 0 log lines: purgeExecutionLogs is false")
                .await;
            return Ok(Outcome::succeeded_with(counts(0)));
        };
        // A database failure is infrastructure: `Err`, so the run is
        // re-claimed. A re-run deletes what is still there, idempotently.
        let n = self.repo.purge(&q).await?;
        task.log
            .info(&format!(
                "purged {n} log lines of namespace '{}' and its child namespaces",
                q.namespaces.join("', '")
            ))
            .await;
        Ok(Outcome::succeeded_with(counts(n)))
    }

    /// Nothing to cancel: one statement, or a few, each short.
    async fn cancel(&self, _task: &TaskInput) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runners::render_str;

    fn spec(v: serde_json::Value) -> Spec {
        serde_json::from_value(v).unwrap()
    }

    fn cx() -> RenderContext {
        RenderContext::new()
    }

    const END: &str = "2026-10-18T10:00:00Z";

    fn planned(v: serde_json::Value) -> std::result::Result<Option<PurgeQuery>, String> {
        plan(&spec(v), &cx(), Uuid::nil(), "a.b")
    }

    fn ok(v: serde_json::Value) -> PurgeQuery {
        planned(v).unwrap().expect("a query")
    }

    fn err(v: serde_json::Value) -> String {
        planned(v).unwrap_err()
    }

    #[test]
    fn no_namespace_is_the_flows_own() {
        let q = ok(serde_json::json!({"endDate": END}));
        assert_eq!(q.namespaces, ["a.b"]);
        assert_eq!(q.workspace_id, Uuid::nil());
        assert_eq!(
            (q.flow_key, q.execution_id, q.levels, q.start, q.batch),
            (None, None, None, None, None)
        );
        assert_eq!(q.end, parse_date(END, "endDate").unwrap());
    }

    #[test]
    fn a_child_namespace_is_allowed_and_another_is_refused() {
        let q = ok(serde_json::json!({"endDate": END, "namespace": "a.b.child"}));
        assert_eq!(q.namespaces, ["a.b.child"]);
        for other in ["other", "a.bc", "a"] {
            let e = err(serde_json::json!({"endDate": END, "namespace": other}));
            assert!(
                e.contains(&format!("may not purge logs of namespace '{other}'")),
                "{e}"
            );
        }
    }

    #[test]
    fn end_date_is_required_and_dates_must_parse() {
        assert!(err(serde_json::json!({})).contains("endDate"));
        let e = err(serde_json::json!({"endDate": "tomorrow"}));
        assert!(e.contains("`endDate`"), "{e}");
        let e = err(serde_json::json!({"endDate": END, "startDate": "2026-13-01"}));
        assert!(e.contains("`startDate`"), "{e}");
    }

    /// The text egret's own `now() | dateAdd(2, 'HOURS')` renders, whatever
    /// its shape, parses -- rendered here, not hand-written.
    #[test]
    fn a_rendered_now_parses() {
        let rendered = render_str("{{ now() | dateAdd(2, 'HOURS') }}", &cx()).unwrap();
        let q = ok(serde_json::json!({"endDate": "{{ now() | dateAdd(2, 'HOURS') }}"}));
        let ahead = q.end - chrono::Utc::now();
        assert!(
            ahead > chrono::Duration::minutes(119) && ahead <= chrono::Duration::hours(2),
            "{rendered} -> {}",
            q.end
        );
        // And the `[Region/City]` suffix Java's ZonedDateTime prints.
        assert_eq!(
            parse_date("2026-10-18T17:00:00+07:00[Asia/Ho_Chi_Minh]", "endDate").unwrap(),
            parse_date(END, "endDate").unwrap()
        );
    }

    #[test]
    fn log_levels_are_case_sensitive() {
        let q = ok(serde_json::json!({"endDate": END, "logLevels": ["INFO", "ERROR"]}));
        assert_eq!(q.levels, Some(vec![LogLevel::Info, LogLevel::Error]));
        let e = err(serde_json::json!({"endDate": END, "logLevels": ["info"]}));
        assert!(e.contains("logLevels") && e.contains("\"info\""), "{e}");
        let q = ok(serde_json::json!({"endDate": END, "logLevels": []}));
        assert_eq!(q.levels, None, "an empty list is no filter, as Kestra's");
    }

    #[test]
    fn flow_and_execution_ids_narrow() {
        let id = Uuid::new_v4();
        let q = ok(serde_json::json!({
            "endDate": END, "flowId": "f", "executionId": id.to_string(),
        }));
        assert_eq!(
            (q.flow_key.as_deref(), q.execution_id),
            (Some("f"), Some(id))
        );
        let e = err(serde_json::json!({"endDate": END, "executionId": "nope"}));
        assert!(e.contains("executionId"), "{e}");
    }

    #[test]
    fn purge_execution_logs_false_is_no_query() {
        assert_eq!(
            planned(serde_json::json!({"endDate": END, "purgeExecutionLogs": false})),
            Ok(None)
        );
        // Still authorized first, and still validated.
        assert!(err(serde_json::json!({
            "endDate": END, "purgeExecutionLogs": false, "namespace": "z",
        }))
        .contains("may not purge logs"));
        let q = ok(serde_json::json!({
            "endDate": END, "purgeExecutionLogs": true, "purgeNonExecutionLogs": false,
        }));
        assert_eq!(q.namespaces, ["a.b"]);
    }

    #[test]
    fn batch_size_must_be_positive() {
        assert_eq!(
            ok(serde_json::json!({"endDate": END, "batchSize": 2})).batch,
            Some(2)
        );
        for bad in [0, -1] {
            let e = err(serde_json::json!({"endDate": END, "batchSize": bad}));
            assert!(e.contains("batchSize"), "{bad}: {e}");
        }
    }

    /// The namespace comes from the execution row, so a database that cannot
    /// be reached is infrastructure: `Err` (re-claimed), never a FAILED run
    /// and never a delete. (The missing-row FAILED case is a DB test,
    /// `tests/purge_logs.rs`.)
    #[tokio::test]
    async fn an_unreachable_database_is_err_not_a_failed_run() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(std::time::Duration::from_secs(1))
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .unwrap();
        let err = PurgeLogs::new(pool)
            .run(&super::super::testing::input(
                TYPE_ID,
                serde_json::json!({"endDate": END}),
            ))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("database is unavailable"), "{err}");
    }
}
