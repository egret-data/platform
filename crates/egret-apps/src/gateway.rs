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
//! 2. per-token rate limit → 429. Every outcome from here on is audited.
//! 3. user still `active` → 401; app not deleted and its workspace `active` →
//!    404; workspace has `apps` on → 403.
//! 4. PDP: run mode needs `app:read` on the app; edit mode needs `app:write` on
//!    the app AND `query:run` on the workspace (an author writes arbitrary SQL;
//!    a viewer only runs a published app, so `ws_viewer` — which lacks
//!    `query:run` — can view).
//! 5. database by name, scoped to the TOKEN's workspace → 404 identical for
//!    "exists elsewhere" and "exists nowhere".
//! 6. read-only gate → 400.

use std::time::{Duration, Instant};

use axum::{body::Bytes, extract::State, http::HeaderMap, routing::post, Json, Router};
use egret_core::authz::{self, AuthTarget, ObjectRef, Pdp, Resource, Subject};
use egret_core::ratelimit::Verdict;
use egret_core::{Error, GroupId, Result, TabularResult, UserId, WorkspaceId};
use egret_metastore::Db;
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

/// `POST /apps/query` (mounted at `/api/apps/query`, outside the CSRF layer).
///
/// Deliberately **not** in the OpenAPI document, like `/metrics`: it is an
/// internal app-pod → egret endpoint authenticated by a per-marimo-session
/// bearer token, with no frontend caller. Annotating it would make
/// `npm run gen:api` generate a client nothing uses.
pub fn gateway_router() -> Router<AppsState> {
    Router::new().route("/apps/query", post(query))
}

async fn query(
    State(st): State<AppsState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<TabularResult>> {
    // A 401 before a session resolves is NOT audited: there is no user, app or
    // workspace to attribute it to, only an unverified string.
    let token = bearer(&headers).ok_or(Error::Unauthorized)?;
    let idle = Duration::from_secs(st.cfg.session_idle_secs);
    let sess = st
        .sessions
        .resolve(token, idle)
        .await?
        .ok_or(Error::Unauthorized)?;
    let started = Instant::now();
    // From here on every outcome is audited: the session names who and what.
    // `check` is true when the call is ALLOWED.
    if let Verdict::Denied { first } = st.limiter.verdict(&sess.id.to_string()) {
        let outcome = Err(Error::TooManyRequests);
        // One audit row per lockout: the call that tripped the limiter. The calls it
        // then refuses are not events, or a runaway cell would fill the audit table.
        if first {
            audit(&st, &sess, None, &outcome, started.elapsed());
        }
        return outcome.map(Json);
    }
    let req: QueryRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => {
            let outcome = Err(Error::InvalidInput(format!("invalid request body: {e}")));
            audit(&st, &sess, None, &outcome, started.elapsed());
            return outcome.map(Json);
        }
    };
    let outcome = run(&st, &sess, &req).await;
    audit(&st, &sess, Some(&req), &outcome, started.elapsed());
    outcome.map(Json)
}

async fn run(st: &AppsState, sess: &AppSession, req: &QueryRequest) -> Result<TabularResult> {
    ensure_user_active(&st.db, sess.user_id).await?;
    let folder_id = app_folder(st, sess).await?;
    ensure_feature_enabled(&st.db, sess.workspace_id).await?;
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

/// The auth scheme is case-insensitive (RFC 7235 §2.1); the token is not.
pub(crate) fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
}

/// A token outlives nothing about its user: disabling the user kills it at once.
pub(crate) async fn ensure_user_active(db: &Db, user: UserId) -> Result<()> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM users WHERE id = $1")
        .bind(user.0)
        .fetch_optional(&db.pool)
        .await
        .map_err(anyhow::Error::from)?;
    if status.as_deref() == Some("active") {
        Ok(())
    } else {
        Err(Error::Unauthorized)
    }
}

pub(crate) async fn ensure_feature_enabled(db: &Db, ws: WorkspaceId) -> Result<()> {
    let disabled = egret_workspace::workspaces::WorkspaceRepo::new(db.clone())
        .disabled_features(ws)
        .await?;
    if disabled.contains("apps") {
        Err(Error::FeatureDisabled("apps"))
    } else {
        Ok(())
    }
}

/// The app's folder (its PDP ancestry). `NotFound` if the app is deleted, is
/// not in the token's workspace, or that workspace is no longer `active` — an
/// archived workspace is gone for the gateway exactly as it is for `WsContext`,
/// and with the same 404 as a missing app.
async fn app_folder(st: &AppsState, sess: &AppSession) -> Result<Option<Uuid>> {
    let row = sqlx::query(
        "SELECT a.folder_id FROM analytics.apps a \
           JOIN workspaces w ON w.id = a.workspace_id \
         WHERE a.id = $1 AND a.workspace_id = $2 AND a.deleted_at IS NULL \
           AND w.status = 'active'",
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

/// The caller as the PDP sees them: the user and every group they belong to.
pub(crate) async fn subject_for(db: &Db, user: UserId) -> Result<Subject> {
    let group_ids: Vec<Uuid> =
        sqlx::query_scalar("SELECT group_id FROM user_groups WHERE user_id = $1")
            .bind(user.0)
            .fetch_all(&db.pool)
            .await
            .map_err(anyhow::Error::from)?;
    Ok(Subject {
        user_id: user,
        group_ids: group_ids.into_iter().map(GroupId).collect(),
    })
}

/// `perm` on one app, for a caller that holds a token rather than a
/// `WorkspaceContext`. Shared by the query gateway and the pod API.
pub(crate) async fn require_on_app(
    pdp: &dyn Pdp,
    subject: &Subject,
    app: ObjectRef,
    perm: &str,
) -> Result<()> {
    let target = SessionTarget {
        subject: subject.clone(),
        resource: Resource::Object(app),
    };
    authz::require(pdp, &target, perm).await
}

async fn authorize(st: &AppsState, sess: &AppSession, folder_id: Option<Uuid>) -> Result<()> {
    let subject = subject_for(&st.db, sess.user_id).await?;
    let app = ObjectRef {
        kind: "app".into(),
        id: sess.app_id,
        folder_id,
        workspace_id: sess.workspace_id,
    };
    let on_ws = SessionTarget {
        subject: subject.clone(),
        resource: Resource::Workspace(sess.workspace_id),
    };
    match sess.mode {
        AppMode::Run => require_on_app(&*st.pdp, &subject, app, APP_READ).await,
        AppMode::Edit => {
            require_on_app(&*st.pdp, &subject, app, APP_WRITE).await?;
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
            // An integer must fit i64; one above i64::MAX is refused rather
            // than silently rounded through f64.
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) => Ok(Param::I64(i)),
                None => n
                    .as_f64()
                    .filter(|_| n.is_f64())
                    .map(Param::F64)
                    .ok_or_else(|| {
                        Error::InvalidInput(format!("integer parameter {n} is out of range"))
                    }),
            },
            serde_json::Value::String(s) => Ok(Param::Str(s.clone())),
            _ => Err(Error::InvalidInput(
                "params must be null, bool, number or string".into(),
            )),
        })
        .collect()
}

/// One `Mutation` per call whose session resolved. The SQL text is never
/// recorded — only its SHA-256 — because a query can carry literals the viewer
/// typed. `req` is `None` when the call was refused before its body was read
/// (rate limited) or the body did not parse: `database` is then empty and
/// `sql_sha256` null.
fn audit(
    st: &AppsState,
    sess: &AppSession,
    req: Option<&QueryRequest>,
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
        Err(Error::TooManyRequests) => ("rate_limited", None, None),
        Err(_) => ("error", None, None),
    };
    st.audit.emit(egret_audit::AuditEvent::Mutation {
        actor: Some(sess.user_id.0),
        impersonator: sess.impersonator.map(|u| u.0),
        workspace_id: Some(sess.workspace_id.0),
        action: "app:query".into(),
        resource_type: "app".into(),
        resource_id: Some(sess.app_id.to_string()),
        detail: serde_json::json!({
            "mode": sess.mode.as_str(),
            "version_id": sess.version_id,
            "database": req.map_or("", |r| r.database.as_str()),
            "sql_sha256": req.map(|r| hex::encode(Sha256::digest(r.sql.as_bytes()))),
            "outcome": label,
            "row_count": rows,
            "truncated": truncated,
            "duration_ms": took.as_millis() as u64,
        }),
    });
}
