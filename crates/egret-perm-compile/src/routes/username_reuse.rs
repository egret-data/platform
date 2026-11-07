//! `GET /perm/username-reuse` — the username-reuse report (username-takeover
//! spec §4.5, decision D3). Platform admins only: it shows emails and holders
//! across workspaces. Read-only, one `REPEATABLE READ, READ ONLY` snapshot.
//! The rules are in [`crate::username_reuse`].

use axum::extract::State;
use axum::Json;

use egret_auth::principal::PlatformAuth;
use egret_core::http::ErrorBody;

use super::{PermHttpError, PermState};
use crate::username_reuse::{findings_via, UsernameReuseReport};

#[utoipa::path(
    get,
    path = "/api/control-plane/perm/username-reuse",
    tag = "perm",
    description = "Evidence of past username takeovers (before migration 0209 a deleted user's \
                   username could be minted for the next person): user refs and policy bodies \
                   older than the person who now holds the name, `egret-admin` bodies naming \
                   someone who is not an active platform admin, and refs nobody holds. Reported \
                   only; nothing is changed.",
    responses(
        (status = 200, description = "Every finding, ordered by username", body = UsernameReuseReport),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Not a platform admin", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn username_reuse(
    State(st): State<PermState>,
    _admin: PlatformAuth,
) -> Result<Json<UsernameReuseReport>, PermHttpError> {
    let mut tx = st.perm.pool().begin().await.map_err(PermHttpError::db)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(PermHttpError::db)?;
    let report = findings_via(&mut tx).await.map_err(PermHttpError::db)?;
    tx.rollback().await.map_err(PermHttpError::db)?;
    Ok(Json(report))
}
