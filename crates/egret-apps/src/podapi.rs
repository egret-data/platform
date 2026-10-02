//! The pod API (spec §5.3): the only egret endpoints an app pod calls besides the
//! query gateway. Each takes a pod token (`egp_…`, `Authorization: Bearer`), minted
//! by egret when it launched the pod; no cookie is involved, so none of it sits
//! behind the CSRF layer.
//!
//! | Route | Token | What |
//! |---|---|---|
//! | `GET  /apps/pod/apps/{app}/draft` | edit | the draft, for a user who still holds `app:write` |
//! | `PUT  /apps/pod/apps/{app}/draft` | edit | save with `If-Match`, same 200/409/413/428 as the cookie API |
//! | `GET  /apps/pod/apps/{app}/versions/{version}` | run | the one version the token is bound to |
//!
//! Authorization is re-evaluated on EVERY call, as in the gateway: a token is not a
//! permission. An edit token is only ever the key to the PDP question "may this user
//! write this app", asked afresh.
//!
//! Left un-annotated for OpenAPI on purpose, like the gateway: bearer-only callers
//! inside pods, no frontend client.

use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::header::ETAG;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use egret_core::authz::{ObjectRef, Pdp};
use egret_core::{Error, UserId, WorkspaceId};
use egret_metastore::Db;
use serde::Serialize;
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;

use crate::gateway::{
    bearer, ensure_feature_enabled, ensure_user_active, require_on_app, subject_for,
};
use crate::podtoken::{PodKind, PodPrincipal, PodTokenRepo};
use crate::routes::{
    domain, etag, if_match, AppDraft, AppDraftSaved, AppSaveDraft, AppsHttpError, AppsResult,
    MAX_BODY_BYTES,
};
use crate::store::{AppStore, SaveOutcome, MAX_SOURCE_BYTES};

const APP_WRITE: &str = "app:write";

/// Everything the pod routes need. Built by the server binary.
#[derive(Clone)]
pub struct PodApiState {
    pub db: Db,
    pub pdp: Arc<dyn Pdp>,
    pub pods: PodTokenRepo,
    pub store: AppStore,
}

pub fn pod_router() -> Router<PodApiState> {
    Router::new()
        .route(
            "/apps/pod/apps/{app_id}/draft",
            get(draft_get)
                .put(draft_put)
                .layer(DefaultBodyLimit::max(MAX_BODY_BYTES)),
        )
        .route(
            "/apps/pod/apps/{app_id}/versions/{version_id}",
            get(version_get),
        )
}

#[derive(Debug, Serialize)]
struct PodVersion {
    source: String,
    version_no: i32,
}

async fn principal(st: &PodApiState, headers: &HeaderMap) -> Result<PodPrincipal, Error> {
    let token = bearer(headers).ok_or(Error::Unauthorized)?;
    st.pods.resolve(token).await?.ok_or(Error::Unauthorized)
}

/// What an edit call has established before it touches the draft.
pub(crate) struct EditTarget {
    pub(crate) user: UserId,
    pub(crate) trashed: bool,
}

/// An edit token, whose user is active, in a workspace with `apps` on, naming an
/// app of THAT workspace on which the user holds `app:write` right now.
async fn edit_target(
    st: &PodApiState,
    p: &PodPrincipal,
    app_id: Uuid,
) -> Result<EditTarget, Error> {
    let (PodKind::Edit, Some(user)) = (p.kind, p.user_id) else {
        return Err(Error::Forbidden);
    };
    require_app_write(&st.db, &*st.pdp, user, p.workspace_id, app_id).await
}

/// `user` may write `app_id` of `ws` right now: active, `apps` on, the app is theirs to
/// reach in that workspace, and the PDP says `app:write`. Asked afresh by every caller —
/// the pod API on each draft call, the proxy on each request that names a notebook file.
pub(crate) async fn require_app_write(
    db: &Db,
    pdp: &dyn Pdp,
    user: UserId,
    ws: WorkspaceId,
    app_id: Uuid,
) -> Result<EditTarget, Error> {
    ensure_user_active(db, user).await?;
    ensure_feature_enabled(db, ws).await?;
    let (folder_id, trashed) = app_in_workspace(db, ws, app_id).await?;
    let subject = subject_for(db, user).await?;
    let obj = ObjectRef {
        kind: "app".into(),
        id: app_id,
        folder_id,
        workspace_id: ws,
    };
    require_on_app(pdp, &subject, obj, APP_WRITE).await?;
    Ok(EditTarget { user, trashed })
}

/// `(folder_id, trashed)`. `NotFound` for an app that is absent, belongs to another
/// workspace, or whose workspace is no longer `active` — one body for all three.
async fn app_in_workspace(
    db: &Db,
    ws: WorkspaceId,
    app_id: Uuid,
) -> Result<(Option<Uuid>, bool), Error> {
    let row = sqlx::query(
        "SELECT a.folder_id, a.deleted_at IS NOT NULL AS trashed FROM analytics.apps a \
           JOIN workspaces w ON w.id = a.workspace_id \
          WHERE a.id = $1 AND a.workspace_id = $2 AND w.status = 'active'",
    )
    .bind(app_id)
    .bind(ws.0)
    .fetch_optional(&db.pool)
    .await
    .map_err(anyhow::Error::from)?;
    row.map(|r| (r.get("folder_id"), r.get("trashed")))
        .ok_or_else(|| Error::NotFound("app".into()))
}

async fn draft_get(
    State(st): State<PodApiState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
) -> AppsResult<Response> {
    let p = principal(&st, &headers).await?;
    edit_target(&st, &p, app_id).await?;
    let d = st
        .store
        .draft(app_id)
        .await?
        .ok_or_else(|| Error::NotFound("draft".into()))?;
    Ok((
        [(ETAG, etag(d.revision))],
        Json(AppDraft {
            source: d.source,
            revision: d.revision,
        }),
    )
        .into_response())
}

async fn draft_put(
    State(st): State<PodApiState>,
    headers: HeaderMap,
    Path(app_id): Path<Uuid>,
    Json(b): Json<AppSaveDraft>,
) -> AppsResult<Response> {
    let p = principal(&st, &headers).await?;
    let target = edit_target(&st, &p, app_id).await?;
    if target.trashed {
        return Err(domain(AppsHttpError::Trashed));
    }
    let expected = if_match(&headers)?;
    if b.source.len() > MAX_SOURCE_BYTES {
        return Err(domain(AppsHttpError::SourceTooLarge));
    }
    match st
        .store
        .save_draft(app_id, target.user, expected, &b.source)
        .await?
    {
        SaveOutcome::Saved { revision } => {
            Ok(([(ETAG, etag(revision))], Json(AppDraftSaved { revision })).into_response())
        }
        SaveOutcome::Stale { current_revision } => {
            Err(domain(AppsHttpError::StaleRevision { current_revision }))
        }
    }
}

async fn version_get(
    State(st): State<PodApiState>,
    headers: HeaderMap,
    Path((app_id, version_id)): Path<(Uuid, Uuid)>,
) -> AppsResult<Json<PodVersion>> {
    let p = principal(&st, &headers).await?;
    // A run token is bound to exactly one (app, version); anything else is refused
    // before the database is asked a question about it.
    if p.kind != PodKind::Run || p.app_id != Some(app_id) || p.version_id != Some(version_id) {
        return Err(Error::Forbidden.into());
    }
    let (_, trashed) = app_in_workspace(&st.db, p.workspace_id, app_id).await?;
    if trashed {
        return Err(Error::NotFound("app".into()).into());
    }
    let v = st
        .store
        .version(app_id, version_id)
        .await?
        .ok_or_else(|| Error::NotFound("version".into()))?;
    Ok(Json(PodVersion {
        source: v.source,
        version_no: v.summary.version_no,
    }))
}
