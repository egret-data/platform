//! `POST /api/apps/query` — the Data Apps query gateway (spec §4.2).
//!
//! A marimo kernel holds an app-session bearer token (`ega_…`, minted by the
//! apps proxy) and nothing else. Every query runs as the VIEWER the token was
//! minted for, through `QueryExecPort::execute_as`, so engine-side policy
//! (Ranger row filters, masks) applies to the person looking at the app — never
//! to its author.
//!
//! Order of checks, and why:
//! 1. token → 401. Before the body is even parsed, so an unauthenticated caller
//!    learns nothing (not even whether its JSON was well-formed) and spends no
//!    one's rate-limit quota.
//! 2. per-token rate limit → 429.
//! 3. user still `active`, workspace has `apps` on, app not deleted → 401/403/404.
//! 4. PDP: run mode needs `app:read` on the app; edit mode needs `app:write` on
//!    the app AND `query:run` on the workspace (an author writes arbitrary SQL;
//!    a viewer only runs a published app, so `ws_viewer` — which lacks
//!    `query:run` — can view).
//! 5. database by name, scoped to the TOKEN's workspace → 404 identical for
//!    "exists elsewhere" and "exists nowhere".
//! 6. read-only gate → 400.

use std::time::{Duration, Instant};

use axum::{body::Bytes, extract::State, http::HeaderMap, routing::post, Json, Router};
use egret_core::authz::{self, AuthTarget, ObjectRef, Resource, Subject};
use egret_core::{Error, GroupId, Result, TabularResult, UserId, WorkspaceId};
use egret_query_api::{Limits, Param};
use sha2::{Digest, Sha256};
use sqlx::Row;
use uuid::Uuid;

use crate::readonly::{check_read_only, EngineDialect};
use crate::session::{AppMode, AppSession};
use crate::AppsState;

/// Run mode: view a published app.
const APP_READ: &str = "app:read";
/// Edit mode: author the draft — on the app object.
const APP_WRITE: &str = "app:write";
/// Edit mode: also arbitrary SQL in the workspace, as SQL Lab requires.
const QUERY_RUN: &str = "query:run";

#[derive(Debug, serde::Deserialize)]
pub struct QueryRequest {
    /// `query.databases.name`, resolved within the token's workspace.
    pub database: String,
    pub sql: String,
    #[serde(default)]
    pub params: Vec<serde_json::Value>,
    /// Clamped to `AppsConfig::max_rows`.
    pub max_rows: Option<usize>,
}

pub fn gateway_router() -> Router<AppsState> {
    Router::new().route("/apps/query", post(query))
}

async fn query(
    State(st): State<AppsState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<TabularResult>> {
    let token = bearer(&headers).ok_or(Error::Unauthorized)?;
    let idle = Duration::from_secs(st.cfg.session_idle_secs);
    let sess = st
        .sessions
        .resolve(token, idle)
        .await?
        .ok_or(Error::Unauthorized)?;
    // `check` is true when the call is ALLOWED.
    if !st.limiter.check(&sess.id.to_string()) {
        return Err(Error::TooManyRequests);
    }

    let req: QueryRequest = serde_json::from_slice(&body)
        .map_err(|e| Error::InvalidInput(format!("invalid request body: {e}")))?;
    let started = Instant::now();
    let outcome = run(&st, &sess, &req).await;
    audit(&st, &sess, &req, &outcome, started.elapsed());
    outcome.map(Json)
}

async fn run(st: &AppsState, sess: &AppSession, req: &QueryRequest) -> Result<TabularResult> {
    ensure_user_active(st, sess.user_id).await?;
    ensure_feature_enabled(st, sess.workspace_id).await?;
    let folder_id = app_folder(st, sess).await?;
    authorize(st, sess, folder_id).await?;
    let (database_id, engine) = database_by_name(st, sess.workspace_id, &req.database).await?;
    let dialect = EngineDialect::from_engine(&engine)
        .ok_or_else(|| Error::InvalidInput("engine not supported by data apps".into()))?;
    check_read_only(&req.sql, dialect).map_err(|e| Error::InvalidInput(e.to_string()))?;

    let limits = Limits {
        max_rows: req.max_rows.unwrap_or(st.cfg.max_rows).min(st.cfg.max_rows),
        timeout: Duration::from_secs(st.cfg.timeout_secs),
    };
    let params = to_params(&req.params)?;
    if params.is_empty() {
        st.query
            .execute_as(
                sess.workspace_id.0,
                database_id,
                sess.user_id,
                &req.sql,
                limits,
            )
            .await
    } else {
        st.query
            .execute_as_with_params(
                sess.workspace_id.0,
                database_id,
                sess.user_id,
                &req.sql,
                &params,
                limits,
            )
            .await
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// A token outlives nothing about its user: disabling the user kills it at once.
async fn ensure_user_active(st: &AppsState, user: UserId) -> Result<()> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(user.0)
        .fetch_optional(&st.db.pool)
        .await
        .map_err(anyhow::Error::from)?;
    if status.as_deref() == Some("active") {
        Ok(())
    } else {
        Err(Error::Unauthorized)
    }
}

async fn ensure_feature_enabled(st: &AppsState, ws: WorkspaceId) -> Result<()> {
    let disabled = egret_workspace::workspaces::WorkspaceRepo::new(st.db.clone())
        .disabled_features(ws)
        .await?;
    if disabled.contains("apps") {
        Err(Error::FeatureDisabled("apps"))
    } else {
        Ok(())
    }
}

/// The app's folder (its PDP ancestry). `NotFound` if deleted or not in the
/// token's workspace.
async fn app_folder(st: &AppsState, sess: &AppSession) -> Result<Option<Uuid>> {
    let row = sqlx::query(
        "SELECT folder_id FROM analytics.apps \
         WHERE id = $1 AND workspace_id = $2 AND deleted_at IS NULL",
    )
    .bind(sess.app_id)
    .bind(sess.workspace_id.0)
    .fetch_optional(&st.db.pool)
    .await
    .map_err(anyhow::Error::from)?;
    row.map(|r| r.get::<Option<Uuid>, _>("folder_id"))
        .ok_or_else(|| Error::NotFound("app".into()))
}

/// The PDP subject and one resource. The gateway has a token, not the
/// `WorkspaceContext` the request extractors build, so it supplies the core PEP
/// (`authz::require`) with this instead of calling `Pdp::authorize` by hand.
struct SessionTarget {
    subject: Subject,
    resource: Resource,
}

impl AuthTarget for SessionTarget {
    fn subject(&self) -> Subject {
        self.subject.clone()
    }
    fn resource(&self) -> Resource {
        self.resource.clone()
    }
}

async fn authorize(st: &AppsState, sess: &AppSession, folder_id: Option<Uuid>) -> Result<()> {
    let group_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT group_id FROM user_groups WHERE user_id = $1")
            .bind(sess.user_id.0)
            .fetch_all(&st.db.pool)
            .await
            .map_err(anyhow::Error::from)?;
    let subject = Subject {
        user_id: sess.user_id,
        group_ids: group_ids.into_iter().map(GroupId).collect(),
    };
    let on_app = SessionTarget {
        subject: subject.clone(),
        resource: Resource::Object(ObjectRef {
            kind: "app".into(),
            id: sess.app_id,
            folder_id,
            workspace_id: sess.workspace_id,
        }),
    };
    let on_ws = SessionTarget {
        subject,
        resource: Resource::Workspace(sess.workspace_id),
    };
    match sess.mode {
        AppMode::Run => authz::require(&*st.pdp, &on_app, APP_READ).await,
        AppMode::Edit => {
            authz::require(&*st.pdp, &on_app, APP_WRITE).await?;
            authz::require(&*st.pdp, &on_ws, QUERY_RUN).await
        }
    }
}

/// Scoped to the token's workspace: a name that exists only in another
/// workspace is indistinguishable from one that exists nowhere.
async fn database_by_name(st: &AppsState, ws: WorkspaceId, name: &str) -> Result<(Uuid, String)> {
    let row =
        sqlx::query("SELECT id, engine FROM query.databases WHERE workspace_id = $1 AND name = $2")
            .bind(ws.0)
            .bind(name)
            .fetch_optional(&st.db.pool)
            .await
            .map_err(anyhow::Error::from)?;
    row.map(|r| (r.get("id"), r.get("engine")))
        .ok_or_else(|| Error::NotFound("database".into()))
}

fn to_params(values: &[serde_json::Value]) -> Result<Vec<Param>> {
    values
        .iter()
        .map(|v| match v {
            serde_json::Value::Null => Ok(Param::Null),
            serde_json::Value::Bool(b) => Ok(Param::Bool(*b)),
            serde_json::Value::Number(n) => n
                .as_i64()
                .map(Param::I64)
                .or_else(|| n.as_f64().map(Param::F64))
                .ok_or_else(|| Error::InvalidInput("unsupported number".into())),
            serde_json::Value::String(s) => Ok(Param::Str(s.clone())),
            _ => Err(Error::InvalidInput(
                "params must be null, bool, number or string".into(),
            )),
        })
        .collect()
}

/// One `Mutation` per resolved call. The SQL text is never recorded — only its
/// SHA-256 — because a query can carry literals the viewer typed.
fn audit(
    st: &AppsState,
    sess: &AppSession,
    req: &QueryRequest,
    outcome: &Result<TabularResult>,
    took: Duration,
) {
    let (label, rows, truncated) = match outcome {
        Ok(t) => ("ok", Some(t.row_count), Some(t.truncated)),
        Err(Error::Unauthorized) => ("unauthorized", None, None),
        Err(Error::Forbidden) => ("forbidden", None, None),
        Err(Error::FeatureDisabled(_)) => ("feature_disabled", None, None),
        Err(Error::NotFound(_)) => ("not_found", None, None),
        Err(Error::InvalidInput(_)) => ("invalid", None, None),
        Err(_) => ("error", None, None),
    };
    st.audit.emit(egret_audit::AuditEvent::Mutation {
        actor: Some(sess.user_id.0),
        impersonator: None,
        workspace_id: Some(sess.workspace_id.0),
        action: "app:query".into(),
        resource_type: "app".into(),
        resource_id: Some(sess.app_id.to_string()),
        detail: serde_json::json!({
            "mode": sess.mode.as_str(),
            "version_id": sess.version_id,
            "database": req.database,
            "sql_sha256": hex::encode(Sha256::digest(req.sql.as_bytes())),
            "outcome": label,
            "row_count": rows,
            "truncated": truncated,
            "duration_ms": took.as_millis() as u64,
        }),
    });
}
