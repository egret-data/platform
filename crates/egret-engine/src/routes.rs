//! Workspace-scoped query REST routes.

use std::sync::Arc;

use axum::extract::{FromRef, Path, Query as AxQuery, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use egret_auth::session::SessionService;
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
    let id = st
        .jobs
        .submit(
            ctx.workspace_id.0,
            body.database_id,
            Some(ctx.principal.user_id.0),
            &body.sql,
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

impl EngineState {
    /// Test constructor: real sub-states over `db`, an in-memory object store, default limits.
    pub fn for_test(db: egret_metastore::Db) -> Self {
        use std::collections::HashMap;
        use std::sync::Mutex;

        #[derive(Default)]
        struct MemStore(Mutex<HashMap<String, Vec<u8>>>);

        #[async_trait::async_trait]
        impl ObjectStore for MemStore {
            async fn put(
                &self,
                uri: &str,
                bytes: Vec<u8>,
            ) -> Result<(), egret_storage::StorageError> {
                self.0.lock().unwrap().insert(uri.into(), bytes);
                Ok(())
            }
            async fn get(&self, uri: &str) -> Result<Vec<u8>, egret_storage::StorageError> {
                self.0
                    .lock()
                    .unwrap()
                    .get(uri)
                    .cloned()
                    .ok_or_else(|| egret_storage::StorageError::NotFound(uri.into()))
            }
            async fn delete(&self, uri: &str) -> Result<(), egret_storage::StorageError> {
                self.0.lock().unwrap().remove(uri);
                Ok(())
            }
        }

        let store = QueryStore::new(db.clone());
        let databases = DatabaseRepo::new(
            db.clone(),
            egret_secrets::SecretStore::new(db.clone(), None),
        );
        let storage: Arc<dyn ObjectStore> = Arc::new(MemStore::default());
        let limits = Limits {
            max_rows: 100_000,
            timeout: std::time::Duration::from_secs(60),
        };
        let sessions = SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        );
        let jobs = JobManager::new(
            store.clone(),
            databases.clone(),
            Arc::new(crate::connector::DefaultConnectorFactory),
            storage.clone(),
            "s3://warehouse".into(),
            "query-results".into(),
            limits.clone(),
            100,
        );
        EngineState {
            store,
            jobs,
            databases,
            storage,
            bucket_uri: "s3://warehouse".into(),
            results_prefix: "query-results".into(),
            limits,
            sessions,
            workspaces: WorkspaceRepo::new(db.clone()),
            authz: Authorizer::new(db),
        }
    }
}
