//! Workspace-scoped management routes for service accounts and their OAuth2
//! clients (P2 Task 5).
//!
//! ```text
//! GET    /api/ws/{workspace}/data-api/service-accounts
//! POST   /api/ws/{workspace}/data-api/service-accounts
//! DELETE /api/ws/{workspace}/data-api/service-accounts/{id}
//! GET    /api/ws/{workspace}/data-api/service-accounts/{id}/clients
//! POST   /api/ws/{workspace}/data-api/service-accounts/{id}/clients
//! DELETE /api/ws/{workspace}/data-api/clients/{id}
//! ```
//!
//! Two invariants this module exists to hold, both of them security properties
//! rather than conveniences:
//!
//! 1. **Issuing a client gates on `service_account:manage`, never `:read`.** A
//!    client is a bearer credential for the account's data access; handing one
//!    out is strictly more powerful than looking at the list.
//! 2. **The workspace comes from [`WsContext`], never from the `{workspace}`
//!    path segment read a second time.** Gating on one and querying with the
//!    other is exactly how a cross-workspace hole opens. Every handler that
//!    names a service account or a client resolves it through
//!    [`scoped_account`] / [`scoped_client`] first, which 404 — never 403 — on
//!    a workspace mismatch, so a caller cannot distinguish "no such account"
//!    from "exists, but not yours".

use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::http::ErrorBody;
use egret_core::{Ack, Error, UserId, WorkspacePath};
use egret_metastore::Db;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::clients::ClientStore;

/// View the accounts and their clients. Deliberately does NOT imply issuing.
const SA_READ: &str = "service_account:read";
/// Create/delete accounts and ISSUE clients. Issuing lives here because a
/// client hands out that account's access.
const SA_MANAGE: &str = "service_account:manage";

/// Handler error type. The Data API management surface has no domain-specific
/// failure mode of its own — every arm is already expressible as an
/// [`egret_core::Error`], which carries the canonical HTTP mapping — so there is
/// nothing for an `ApiError<Domain>` split to hold.
pub type DataApiError = Error;

/// State for [`management_router`].
#[derive(Clone)]
pub struct DataApiState {
    pub db: Db,
    pub clients: ClientStore,
    pub users: UserRepo,
    pub sessions: SessionService,
    pub workspaces: WorkspaceRepo,
    pub authz: Authorizer,
    pub pdp: Arc<dyn egret_core::authz::Pdp>,
    /// egret's public root URL — the machine-IdP's `iss`. The `token_endpoint`
    /// handed back with a new client is derived from it so a caller never has to
    /// guess where to exchange the credential.
    pub issuer: String,
}

impl FromRef<DataApiState> for SessionService {
    fn from_ref(s: &DataApiState) -> Self {
        s.sessions.clone()
    }
}
impl FromRef<DataApiState> for WorkspaceRepo {
    fn from_ref(s: &DataApiState) -> Self {
        s.workspaces.clone()
    }
}
impl FromRef<DataApiState> for Authorizer {
    fn from_ref(s: &DataApiState) -> Self {
        s.authz.clone()
    }
}

/// A service account as the management UI sees it. `id` is the `users.id` — a
/// service account is a real user row, so this is the id that appears in
/// `access_grants`, Cedar and audit.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataApiServiceAccountDto {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    /// The synthetic address `users.email` requires. Not a mailbox.
    pub email: String,
    pub created_at: DateTime<Utc>,
}

/// What a client list returns. There is deliberately NO secret field — not a
/// skipped one, not an `Option`. A field that cannot exist cannot leak through a
/// future serde attribute change.
///
/// Named with the `DataApi` prefix because utoipa keys components by Rust type
/// name and those names are GLOBAL after `openapi_doc()` merges the seams —
/// `egret-catalog-engine` already publishes a `ClientDto`, and a collision
/// silently overwrites one of them in the merged document.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataApiClientDto {
    pub id: Uuid,
    pub name: String,
    pub client_id: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// The issue response, and the only type in the crate carrying secret material.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataApiNewClientDto {
    pub id: Uuid,
    pub client_id: String,
    /// Shown once; not recoverable afterwards.
    pub client_secret: String,
    /// Where to exchange these for a token.
    pub token_endpoint: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateServiceAccountInput {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct IssueClientInput {
    pub name: String,
    /// Optional hard expiry. Absent means the client lives until revoked.
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

/// `ClientStore::create` takes `time::OffsetDateTime`; the wire and the rest of
/// the workspace speak `chrono`. Mirrors `clients::offset_to_chrono` in the
/// other direction — see its comment for why sqlx's `time` feature is not the
/// answer.
fn chrono_to_offset(d: DateTime<Utc>) -> Result<time::OffsetDateTime, DataApiError> {
    let nanos = d
        .timestamp_nanos_opt()
        .ok_or_else(|| Error::InvalidInput("expires_at out of range".into()))?;
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(nanos))
        .map_err(|e| Error::InvalidInput(format!("expires_at out of range: {e}")))
}

fn db_err(e: sqlx::Error) -> DataApiError {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// Resolve a service account and confirm it belongs to the workspace the caller
/// was authorized against. Returns 404 — never 403 — on mismatch, so a caller in
/// another workspace cannot distinguish "no such account" from "not yours".
async fn scoped_account(st: &DataApiState, ws: Uuid, sa: Uuid) -> Result<Uuid, DataApiError> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM dataapi.service_accounts WHERE user_id = $1 AND workspace_id = $2",
    )
    .bind(sa)
    .bind(ws)
    .fetch_optional(&st.db.pool)
    .await
    .map_err(db_err)?;
    found.ok_or_else(|| Error::NotFound("service account".into()))
}

/// The same containment for a client id, which names its workspace only
/// transitively through its service account.
async fn scoped_client(st: &DataApiState, ws: Uuid, client: Uuid) -> Result<Uuid, DataApiError> {
    let found: Option<Uuid> = sqlx::query_scalar(
        "SELECT c.id FROM dataapi.clients c \
         JOIN dataapi.service_accounts sa ON sa.user_id = c.service_account_id \
         WHERE c.id = $1 AND sa.workspace_id = $2",
    )
    .bind(client)
    .bind(ws)
    .fetch_optional(&st.db.pool)
    .await
    .map_err(db_err)?;
    found.ok_or_else(|| Error::NotFound("client".into()))
}

/// `users.email` is `citext NOT NULL UNIQUE` and a service account has no
/// mailbox, so one is synthesised: `svc.<slug>@<workspace-slug>.service.egret.local`
/// (spec §5). Making `email` nullable would touch every consumer that assumes it
/// exists; the synthetic address is the smaller change.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = true; // leading dashes are trimmed by starting "dashed"
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn dto_row(
    (id, name, description, email, created_at): (Uuid, String, String, String, DateTime<Utc>),
) -> DataApiServiceAccountDto {
    DataApiServiceAccountDto {
        id,
        name,
        description,
        email,
        created_at,
    }
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/service-accounts",
    tag = "data-api",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Service accounts in the workspace", body = Vec<DataApiServiceAccountDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing service_account:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_service_accounts(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<DataApiServiceAccountDto>>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SA_READ).await?;
    let rows: Vec<(Uuid, String, String, String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT sa.user_id, sa.name, sa.description, u.email::text, sa.created_at \
         FROM dataapi.service_accounts sa JOIN users u ON u.id = sa.user_id \
         WHERE sa.workspace_id = $1 ORDER BY sa.created_at DESC",
    )
    .bind(ctx.workspace_id.0)
    .fetch_all(&st.db.pool)
    .await
    .map_err(db_err)?;
    Ok(Json(rows.into_iter().map(dto_row).collect()))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/service-accounts",
    tag = "data-api",
    params(WorkspacePath),
    request_body = CreateServiceAccountInput,
    responses(
        (status = 201, description = "Service account created", body = DataApiServiceAccountDto),
        (status = 400, description = "Empty or unusable name", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing service_account:manage permission", body = ErrorBody),
        (status = 409, description = "A service account with that name already exists here", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_service_account(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Json(body): Json<CreateServiceAccountInput>,
) -> Result<(StatusCode, Json<DataApiServiceAccountDto>), DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SA_MANAGE).await?;

    let name = body.name.trim();
    let slug = slugify(name);
    if slug.is_empty() {
        return Err(Error::InvalidInput(
            "name must contain at least one alphanumeric character".into(),
        ));
    }
    let ws_slug: String = sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
        .bind(ctx.workspace_id.0)
        .fetch_one(&st.db.pool)
        .await
        .map_err(db_err)?;
    let email = format!("svc.{slug}@{ws_slug}.service.egret.local");

    // A service account is BOTH rows: the platform-level `users` row (which is
    // what makes it a real principal for Cedar/grants/Ranger/audit) and the
    // `dataapi.service_accounts` row that binds it to this workspace. If the
    // second fails — `UNIQUE (workspace_id, name)` — the first is compensated
    // away rather than left as an orphan user nobody can see or delete.
    let user_id = st.users.insert_service(&email, name).await?;
    let inserted: Result<DateTime<Utc>, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO dataapi.service_accounts (user_id, workspace_id, name, description, created_by) \
         VALUES ($1, $2, $3, $4, $5) RETURNING created_at",
    )
    .bind(user_id.0)
    .bind(ctx.workspace_id.0)
    .bind(name)
    .bind(&body.description)
    .bind(ctx.principal.user_id.0)
    .fetch_one(&st.db.pool)
    .await;

    let created_at = match inserted {
        Ok(t) => t,
        Err(e) => {
            let _ = sqlx::query("DELETE FROM users WHERE id = $1")
                .bind(user_id.0)
                .execute(&st.db.pool)
                .await;
            return Err(match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() => {
                    Error::Conflict(format!("service account already exists: {name}"))
                }
                _ => db_err(e),
            });
        }
    };

    Ok((
        StatusCode::CREATED,
        Json(DataApiServiceAccountDto {
            id: user_id.0,
            name: name.to_string(),
            description: body.description,
            email,
            created_at,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/data-api/service-accounts/{id}",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Service account id (a users.id)"),
    ),
    responses(
        (status = 200, description = "Service account, its grants and all its clients deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing service_account:manage permission", body = ErrorBody),
        (status = 404, description = "No such service account in this workspace", body = ErrorBody),
        (status = 409, description = "The account still owns content that has no delete rule", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_service_account(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, sa)): Path<(String, Uuid)>,
) -> Result<Json<Ack>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SA_MANAGE).await?;
    let sa = scoped_account(&st, ctx.workspace_id.0, sa).await?;
    // `UserRepo::hard_delete`, NOT a raw `DELETE FROM users`. `access_grants`
    // holds a POLYMORPHIC `principal_id` with no FK to `users(id)` (migration
    // 0068) and no cleanup trigger, so a bare row delete leaves live-looking
    // grants behind for a principal that no longer exists. That is not a corner
    // case here: P2's whole premise (spec §5.3) is that an admin grants the
    // service account a role through `access_grants`, so this is the ONE
    // principal type that is nearly always granted before it is deleted.
    // `hard_delete` sweeps grants, sessions and group memberships in one
    // transaction and maps a 23503 to a 409.
    //
    // `reassign_notebooks_to` is the deleting admin, which is what the parameter
    // means. For a service account the reassignment matches zero rows —
    // `notebooks.owner_user_id` is only ever set by an interactive session and a
    // service account can never log in (`users_service_has_no_password`, 0119) —
    // so this is a no-op that stays correct if that ever changes.
    st.users
        .hard_delete(UserId(sa), ctx.principal.user_id)
        .await?;
    // Deleting the `users` row cascades through `dataapi.service_accounts` into
    // `dataapi.clients` (migration 0119), so no client outlives its account.
    Ok(Json(Ack::ok()))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/service-accounts/{id}/clients",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Service account id"),
    ),
    responses(
        (status = 200, description = "The account's OAuth2 clients. Never carries secret material.", body = Vec<DataApiClientDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing service_account:read permission", body = ErrorBody),
        (status = 404, description = "No such service account in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_clients(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, sa)): Path<(String, Uuid)>,
) -> Result<Json<Vec<DataApiClientDto>>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SA_READ).await?;
    let sa = scoped_account(&st, ctx.workspace_id.0, sa).await?;
    type Row = (
        Uuid,
        String,
        String,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        Option<DateTime<Utc>>,
        DateTime<Utc>,
    );
    // `client_secret_hash` is not in the projection either: the list query has no
    // reason to pull hash material into the process at all.
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, client_id, expires_at, last_used_at, revoked_at, created_at \
         FROM dataapi.clients WHERE service_account_id = $1 ORDER BY created_at DESC",
    )
    .bind(sa)
    .fetch_all(&st.db.pool)
    .await
    .map_err(db_err)?;
    Ok(Json(
        rows.into_iter()
            .map(
                |(id, name, client_id, expires_at, last_used_at, revoked_at, created_at)| {
                    DataApiClientDto {
                        id,
                        name,
                        client_id,
                        expires_at,
                        last_used_at,
                        revoked_at,
                        created_at,
                    }
                },
            )
            .collect(),
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/service-accounts/{id}/clients",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Service account id"),
    ),
    request_body = IssueClientInput,
    responses(
        (status = 201, description = "Client issued; client_secret is shown ONCE and is not recoverable", body = DataApiNewClientDto),
        (status = 400, description = "expires_at out of range", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing service_account:manage permission — :read does not imply issuing", body = ErrorBody),
        (status = 404, description = "No such service account in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn issue_client(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, sa)): Path<(String, Uuid)>,
    Json(body): Json<IssueClientInput>,
) -> Result<(StatusCode, Json<DataApiNewClientDto>), DataApiError> {
    // `:manage`, never `:read` — issuing a client hands out this account's access.
    egret_core::authz::require(&*st.pdp, &ctx, SA_MANAGE).await?;
    // `ctx.workspace_id`, never the path segment — see the module comment.
    let sa = scoped_account(&st, ctx.workspace_id.0, sa).await?;
    let expires_at = body.expires_at.map(chrono_to_offset).transpose()?;
    let c = st
        .clients
        .create(sa, &body.name, expires_at, ctx.principal.user_id)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(DataApiNewClientDto {
            id: c.id,
            client_id: c.client_id,
            client_secret: c.client_secret,
            token_endpoint: format!("{}/oauth/token", st.issuer.trim_end_matches('/')),
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/data-api/clients/{id}",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Client id"),
    ),
    responses(
        (status = 200, description = "Client revoked. Tokens already minted stay valid for up to DATA_API_TTL_SECS.", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing service_account:manage permission", body = ErrorBody),
        (status = 404, description = "No such client in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn revoke_client(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<Ack>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, SA_MANAGE).await?;
    let id = scoped_client(&st, ctx.workspace_id.0, id).await?;
    st.clients.revoke(id).await?;
    Ok(Json(Ack::ok()))
}

/// The Data API management router. Mount under `/api` so the live paths are
/// `/api/ws/{workspace}/data-api/…` — `WsContext`'s feature gate only recognises
/// a path beginning `/api/ws/` or `/ws/`, and `egret-server`'s
/// `every_workspace_scoped_route_is_mounted_under_api_ws` holds that to CI.
pub fn management_router() -> Router<DataApiState> {
    Router::new()
        .route(
            "/ws/{workspace}/data-api/service-accounts",
            get(list_service_accounts).post(create_service_account),
        )
        .route(
            "/ws/{workspace}/data-api/service-accounts/{id}",
            delete(delete_service_account),
        )
        .route(
            "/ws/{workspace}/data-api/service-accounts/{id}/clients",
            get(list_clients).post(issue_client),
        )
        .route(
            "/ws/{workspace}/data-api/clients/{id}",
            delete(revoke_client),
        )
}

#[derive(utoipa::OpenApi)]
#[openapi(
    paths(
        list_service_accounts,
        create_service_account,
        delete_service_account,
        list_clients,
        issue_client,
        revoke_client,
    ),
    components(schemas(
        DataApiServiceAccountDto,
        DataApiClientDto,
        DataApiNewClientDto,
        CreateServiceAccountInput,
        IssueClientInput,
    )),
    tags((name = "data-api", description = "Data API service accounts and OAuth2 clients")),
)]
pub struct DataApiAdminApi;

/// The Data API seam's OpenAPI document, merged by `egret_server::openapi_doc`.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    DataApiAdminApi::openapi()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_produces_an_email_local_part() {
        assert_eq!(slugify("Reporting"), "reporting");
        assert_eq!(slugify("Nightly ETL"), "nightly-etl");
        assert_eq!(slugify("  a__b  "), "a-b");
        assert_eq!(slugify("!!!"), "");
    }

    #[test]
    fn chrono_and_offset_round_trip() {
        let now = Utc::now();
        let o = chrono_to_offset(now).expect("in range");
        assert_eq!(
            o.unix_timestamp_nanos(),
            i128::from(now.timestamp_nanos_opt().unwrap())
        );
    }
}
