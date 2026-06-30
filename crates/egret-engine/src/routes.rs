//! Workspace-scoped query REST routes.

use std::sync::Arc;

use axum::extract::{FromRef, Path, Query as AxQuery, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::Error;
use egret_database::DatabaseRepo;
use egret_storage::ObjectStore;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::Deserialize;
use uuid::Uuid;

use crate::error::EngineApiError;
use crate::job::JobManager;
use crate::model::{Limits, QueryListItem, QueryStatusDto, ResultsPage, SubmitAck, SubmitQuery};
use crate::store::{Query, QueryStore};

#[derive(Clone)]
pub struct EngineState {
    pub store: QueryStore,
    pub jobs: JobManager,
    pub databases: DatabaseRepo,
    pub storage: Arc<dyn ObjectStore>,
    pub bucket_uri: Arc<str>,
    pub results_prefix: Arc<str>,
    pub limits: Limits,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
    pub users: UserRepo,
}

impl FromRef<EngineState> for SessionService {
    fn from_ref(s: &EngineState) -> Self {
        s.sessions.clone()
    }
}
impl FromRef<EngineState> for WorkspaceRepo {
    fn from_ref(s: &EngineState) -> Self {
        s.workspaces.clone()
    }
}
impl FromRef<EngineState> for Authorizer {
    fn from_ref(s: &EngineState) -> Self {
        s.authz.clone()
    }
}

const PERM: &str = "query:run";

pub fn engine_router() -> Router<EngineState> {
    Router::new()
        .route("/ws/{workspace}/queries", post(submit).get(history))
        .route("/ws/{workspace}/queries/{id}", get(status))
        .route("/ws/{workspace}/queries/{id}/results", get(results))
        .route("/ws/{workspace}/queries/{id}/results.csv", get(results_csv))
        .route("/ws/{workspace}/queries/{id}/cancel", post(cancel))
}

async fn authorize(
    st: &EngineState,
    ctx: &egret_core::WorkspaceContext,
    database_id: Uuid,
) -> Result<(), EngineApiError> {
    st.authz
        .require_workspace(ctx, PERM)
        .await
        .map_err(EngineApiError::from)?;
    let granted = st
        .databases
        .list_for_workspace(ctx.workspace_id.0)
        .await
        .map_err(|e| EngineApiError::from(crate::EngineError::from(e)))?;
    if !granted.iter().any(|d| d.id == database_id) {
        return Err(Error::NotFound("database not granted to workspace".into()).into());
    }
    Ok(())
}

async fn submit(
    State(st): State<EngineState>,
    WsContext(ctx): WsContext,
    Json(body): Json<SubmitQuery>,
) -> Result<(axum::http::StatusCode, Json<SubmitAck>), EngineApiError> {
    authorize(&st, &ctx, body.database_id).await?;
    let session_user = Some(
        st.users
            .ensure_username(ctx.principal.user_id)
            .await
            .map_err(EngineApiError::from)?,
    );
    let id = st
        .jobs
        .submit(
            ctx.workspace_id.0,
            body.database_id,
            Some(ctx.principal.user_id.0),
            session_user,
            &body.sql,
            body.limit,
        )
        .await
        .map_err(EngineApiError::from)?;
    Ok((
        axum::http::StatusCode::ACCEPTED,
        Json(SubmitAck {
            id: id.to_string(),
            status: crate::model::QueryStatus::Queued,
        }),
    ))
}

async fn owned(
    st: &EngineState,
    ctx: &egret_core::WorkspaceContext,
    id: Uuid,
) -> Result<Query, EngineApiError> {
    let q = st
        .store
        .get(id)
        .await
        .map_err(EngineApiError::from)?
        .ok_or_else(|| Error::NotFound("query".into()))?;
    if q.workspace_id != ctx.workspace_id.0 {
        return Err(Error::NotFound("query".into()).into());
    }
    let is_admin = ctx
        .principal
        .platform_roles
        .iter()
        .any(|r| r == "platform_admin");
    if !is_admin && q.user_id != Some(ctx.principal.user_id.0) {
        return Err(Error::Forbidden.into());
    }
    Ok(q)
}

async fn status(
    State(st): State<EngineState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<QueryStatusDto>, EngineApiError> {
    st.authz
        .require_workspace(&ctx, PERM)
        .await
        .map_err(EngineApiError::from)?;
    let q = owned(&st, &ctx, id).await?;
    Ok(Json(QueryStatusDto {
        id: q.id.to_string(),
        database_id: q.database_id.to_string(),
        status: q.status,
        columns: q.columns,
        preview: q.preview,
        row_count: q.row_count,
        bytes_scanned: q.bytes_scanned,
        truncated: q.truncated,
        error: q.error,
    }))
}

#[derive(Deserialize)]
struct Page {
    page: Option<usize>,
    page_size: Option<usize>,
}

async fn results(
    State(st): State<EngineState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
    AxQuery(p): AxQuery<Page>,
) -> Result<Json<ResultsPage>, EngineApiError> {
    st.authz
        .require_workspace(&ctx, PERM)
        .await
        .map_err(EngineApiError::from)?;
    let q = owned(&st, &ctx, id).await?;
    let page = p.page.unwrap_or(0);
    let page_size = p.page_size.unwrap_or(100).clamp(1, 1000);
    let rows = match &q.result_ref {
        Some(uri) => st
            .store
            .read_results(&st.storage, uri, page * page_size, page_size)
            .await
            .map_err(EngineApiError::from)?,
        None => vec![],
    };
    Ok(Json(ResultsPage {
        columns: q.columns.unwrap_or(serde_json::json!([])),
        rows,
        page,
        page_size,
        row_count: q.row_count.unwrap_or(0),
        truncated: q.truncated,
    }))
}

async fn results_csv(
    State(st): State<EngineState>,
    WsContext(ctx): WsContext,
    Path((_w, id)): Path<(String, uuid::Uuid)>,
) -> Result<axum::response::Response, EngineApiError> {
    st.authz
        .require_workspace(&ctx, PERM)
        .await
        .map_err(EngineApiError::from)?;
    let q = owned(&st, &ctx, id).await?;
    let rows = match &q.result_ref {
        Some(uri) => {
            let n = q.row_count.unwrap_or(0).max(0) as usize;
            st.store
                .read_results(&st.storage, uri, 0, n)
                .await
                .map_err(EngineApiError::from)?
        }
        None => vec![],
    };
    let columns = q.columns.clone().unwrap_or(serde_json::json!([]));
    let body = rows_to_csv(&columns, &rows);
    use axum::http::header;
    use axum::response::IntoResponse;
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"query-{id}.csv\""),
            ),
        ],
        body,
    )
        .into_response())
}

/// Build a CSV (RFC 4180) from a columns spec `[{name,type}]` + JSONL rows (`[[v,...]]`).
pub fn rows_to_csv(columns: &serde_json::Value, rows: &[serde_json::Value]) -> String {
    let mut out = String::new();
    if let Some(cols) = columns.as_array() {
        if !cols.is_empty() {
            let headers: Vec<String> = cols
                .iter()
                .map(|c| {
                    c.get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string()
                })
                .collect();
            out.push_str(
                &headers
                    .iter()
                    .map(|h| csv_field(h))
                    .collect::<Vec<_>>()
                    .join(","),
            );
            out.push('\n');
        }
    }
    for row in rows {
        if let Some(arr) = row.as_array() {
            let line = arr
                .iter()
                .map(|v| csv_field(&cell(v)))
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

fn cell(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

async fn cancel(
    State(st): State<EngineState>,
    WsContext(ctx): WsContext,
    Path((_ws, id)): Path<(String, Uuid)>,
) -> Result<Json<serde_json::Value>, EngineApiError> {
    st.authz
        .require_workspace(&ctx, PERM)
        .await
        .map_err(EngineApiError::from)?;
    let _q = owned(&st, &ctx, id).await?;
    st.jobs.cancel(id).await.map_err(EngineApiError::from)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn history(
    State(st): State<EngineState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<QueryListItem>>, EngineApiError> {
    st.authz
        .require_workspace(&ctx, PERM)
        .await
        .map_err(EngineApiError::from)?;
    let qs = st
        .store
        .list_for_user(ctx.workspace_id.0, ctx.principal.user_id.0, 50)
        .await
        .map_err(EngineApiError::from)?;
    Ok(Json(
        qs.into_iter()
            .map(|q| QueryListItem {
                id: q.id.to_string(),
                database_id: q.database_id.to_string(),
                status: q.status,
                sql: q.sql,
                row_count: q.row_count,
                submitted_at: q.submitted_at,
            })
            .collect(),
    ))
}
