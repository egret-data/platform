//! Workspace-scoped management routes for service accounts and their OAuth2
//! clients (P2 Task 5), and for endpoint drafts (P3 Task 3).
//!
//! ```text
//! GET    /api/ws/{workspace}/data-api/service-accounts
//! POST   /api/ws/{workspace}/data-api/service-accounts
//! DELETE /api/ws/{workspace}/data-api/service-accounts/{id}
//! GET    /api/ws/{workspace}/data-api/service-accounts/{id}/clients
//! POST   /api/ws/{workspace}/data-api/service-accounts/{id}/clients
//! DELETE /api/ws/{workspace}/data-api/clients/{id}
//!
//! GET    /api/ws/{workspace}/data-api/endpoints
//! POST   /api/ws/{workspace}/data-api/endpoints
//! GET    /api/ws/{workspace}/data-api/endpoints/{id}
//! PATCH  /api/ws/{workspace}/data-api/endpoints/{id}
//! DELETE /api/ws/{workspace}/data-api/endpoints/{id}
//! ```
//!
//! Three invariants this module exists to hold, all of them security properties
//! rather than conveniences:
//!
//! 1. **Issuing a client gates on `service_account:manage`, never `:read`.** A
//!    client is a bearer credential for the account's data access; handing one
//!    out is strictly more powerful than looking at the list.
//! 2. **The workspace comes from [`WsContext`], never from the `{workspace}`
//!    path segment read a second time.** Gating on one and querying with the
//!    other is exactly how a cross-workspace hole opens. Every handler that
//!    names a service account or a client resolves it through
//!    [`scoped_account`] / [`scoped_client`] / [`scoped_endpoint`] first, which
//!    404 — never 403 — on a workspace mismatch, so a caller cannot distinguish
//!    "no such account" from "exists, but not yours".
//! 3. **A `database_id` arriving in a request body is resolved through
//!    [`scoped_database`] before it is stored.** It is the one identifier here
//!    that a caller supplies for an object it does not name in the URL, so
//!    nothing else in the draft path would stop an endpoint pointing at another
//!    workspace's connection. `describe_as` is workspace-scoped and would refuse
//!    it at publish time, but by then the reference has been accepted, stored,
//!    and echoed back to the author as valid.

use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::http::ErrorBody;
use egret_core::{Ack, Error, UserId, WorkspacePath};
use egret_dataapi_api::{CacheSpec, LimitsSpec, OutputSpec, ParamSpec, RateLimitSpec};
use egret_metastore::Db;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::clients::ClientStore;
use crate::endpoints::{
    normalize_path, CreateEndpointInput, DataApiEndpointDto, EndpointStore, UpdateEndpointInput,
    ALLOWED_METHODS,
};

/// View the accounts and their clients. Deliberately does NOT imply issuing.
const SA_READ: &str = "service_account:read";
/// Create/delete accounts and ISSUE clients. Issuing lives here because a
/// client hands out that account's access.
const SA_MANAGE: &str = "service_account:manage";

/// List and inspect endpoint drafts and their versions.
const EP_READ: &str = "data_api_endpoint:read";
/// Author endpoints. Flat, like every permission here: it implies no `:read`,
/// and `:read` implies none of it.
const EP_MANAGE: &str = "data_api_endpoint:manage";

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
    pub endpoints: EndpointStore,
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

/// The same containment for an endpoint draft.
async fn scoped_endpoint(st: &DataApiState, ws: Uuid, id: Uuid) -> Result<Uuid, DataApiError> {
    let found: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM dataapi.endpoints WHERE id = $1 AND workspace_id = $2")
            .bind(id)
            .bind(ws)
            .fetch_optional(&st.db.pool)
            .await
            .map_err(db_err)?;
    found.ok_or_else(|| Error::NotFound("endpoint".into()))
}

/// Confirm a `database_id` from a REQUEST BODY belongs to the workspace the
/// caller was authorized against.
///
/// `query.databases.workspace_id` is `NOT NULL` and ownership is direct, so this
/// is the same predicate `QueryExecPort::describe_as` runs through
/// `DatabaseRepo::get_in_workspace`. It is repeated here rather than deferred to
/// publish because the draft is written first: without it, an author can save an
/// endpoint naming another workspace's connection, see it echoed back as valid,
/// and only discover at publish time that it was never usable — having in the
/// meantime stored one workspace's connection id inside another's endpoint,
/// where it is readable by anyone with `data_api_endpoint:read` there.
///
/// `NotFound`, not `Forbidden`: a connection the workspace was never granted must
/// be indistinguishable from one that does not exist, exactly as `scoped_account`
/// and `scoped_endpoint` treat their objects.
async fn scoped_database(st: &DataApiState, ws: Uuid, database: Uuid) -> Result<(), DataApiError> {
    let found: Option<Uuid> =
        sqlx::query_scalar("SELECT id FROM query.databases WHERE id = $1 AND workspace_id = $2")
            .bind(database)
            .bind(ws)
            .fetch_optional(&st.db.pool)
            .await
            .map_err(db_err)?;
    found
        .map(|_| ())
        .ok_or_else(|| Error::NotFound("database".into()))
}

/// Normalise and validate the `method` an author typed.
///
/// Uppercased first, so `get` is the same request as `GET`, then checked against
/// the set the column's CHECK constraint accepts — otherwise the constraint
/// answers instead, as a 500 quoting `endpoints_method_check` at someone who
/// only wanted to know which verbs are allowed.
fn checked_method(raw: Option<&str>) -> Result<String, DataApiError> {
    let m = raw.map(|m| m.trim().to_ascii_uppercase());
    let m = match m {
        None => return Ok("GET".to_string()),
        Some(m) => m,
    };
    if ALLOWED_METHODS.contains(&m.as_str()) {
        Ok(m)
    } else {
        Err(Error::InvalidInput(format!(
            "method must be one of {}",
            ALLOWED_METHODS.join(", ")
        )))
    }
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
    // `dataapi.service_accounts` row that binds it to this workspace. They go in
    // ONE transaction, so a failed second insert — `UNIQUE (workspace_id, name)`
    // — rolls the first back rather than needing a compensating delete. The
    // compensation this replaces was itself a `let _ =`: if it failed (a dropped
    // connection is exactly when both writes are most likely to be interrupted)
    // the orphan `users` row stayed, invisible to every list and unreachable by
    // every delete route, and permanently holding the UNIQUE email that would
    // make a retry fail too.
    //
    // `UserRepo::insert_service` is not used here for the same reason: it runs
    // on the pool and cannot join this transaction. The CHECK constraint it
    // documents (`users_service_has_no_password`, 0119) is enforced by the
    // database, not by that method, so writing the row here loses nothing.
    let mut tx = st.db.pool.begin().await.map_err(db_err)?;
    let inserted: Result<(Uuid, DateTime<Utc>), sqlx::Error> = async {
        let user_id: Uuid = sqlx::query_scalar(
            "INSERT INTO users (provider, email, name) VALUES ('service', $1, $2) RETURNING id",
        )
        .bind(&email)
        .bind(name)
        .fetch_one(&mut *tx)
        .await?;
        let created_at: DateTime<Utc> = sqlx::query_scalar(
            "INSERT INTO dataapi.service_accounts (user_id, workspace_id, name, description, created_by) \
             VALUES ($1, $2, $3, $4, $5) RETURNING created_at",
        )
        .bind(user_id)
        .bind(ctx.workspace_id.0)
        .bind(name)
        .bind(&body.description)
        .bind(ctx.principal.user_id.0)
        .fetch_one(&mut *tx)
        .await?;
        Ok((user_id, created_at))
    }
    .await;

    let (user_id, created_at) = match inserted {
        Ok(v) => {
            tx.commit().await.map_err(db_err)?;
            v
        }
        Err(e) => {
            // Explicit, though dropping `tx` would also roll back — the rollback
            // is the whole point of this branch and should not be implicit.
            let _ = tx.rollback().await;
            // Either UNIQUE can fire and both mean the same thing to the caller:
            // this workspace already has that service account. `users.email` is
            // derived from the name and the workspace slug, so an email
            // collision IS a name collision seen from the other table.
            return Err(match &e {
                sqlx::Error::Database(d) if d.is_unique_violation() => {
                    Error::Conflict(format!("service account already exists: {name}"))
                }
                _ => db_err(e),
            });
        }
    };
    let user_id = UserId(user_id);

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
    let c = st
        .clients
        .create(sa, &body.name, body.expires_at, ctx.principal.user_id)
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

// ---------- Endpoint drafts (P3 Task 3) ----------
//
// Draft CRUD only. Nothing here writes `state` or `live_version_id`: publishing,
// rollback and disable are a separate lifecycle, so no amount of editing can put
// an endpoint live or change what an already-live one serves.

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/endpoints",
    tag = "data-api",
    params(WorkspacePath),
    responses(
        (status = 200, description = "Endpoint drafts in the workspace", body = Vec<DataApiEndpointDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_endpoints(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
) -> Result<Json<Vec<DataApiEndpointDto>>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_READ).await?;
    Ok(Json(st.endpoints.list(ctx.workspace_id.0).await?))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "The endpoint draft", body = DataApiEndpointDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:read permission", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<DataApiEndpointDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_READ).await?;
    let id = scoped_endpoint(&st, ctx.workspace_id.0, id).await?;
    Ok(Json(st.endpoints.get(id).await?))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints",
    tag = "data-api",
    params(WorkspacePath),
    request_body = CreateEndpointInput,
    responses(
        (status = 201, description = "Draft created", body = DataApiEndpointDto),
        (status = 400, description = "Empty name or path, or an unsupported method", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission — :read does not imply authoring", body = ErrorBody),
        (status = 404, description = "No such database in this workspace", body = ErrorBody),
        (status = 409, description = "Another endpoint already serves that method and path", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn create_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Json(body): Json<CreateEndpointInput>,
) -> Result<(StatusCode, Json<DataApiEndpointDto>), DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    if body.name.trim().is_empty() {
        return Err(Error::InvalidInput("name must not be empty".into()));
    }
    let path = normalize_path(&body.path)
        .ok_or_else(|| Error::InvalidInput("path must not be empty".into()))?;
    let method = checked_method(body.method.as_deref())?;
    // Before the insert, so a refused database never reaches the FK — and so the
    // answer is a 404 about a database rather than a 500 about a constraint.
    scoped_database(&st, ctx.workspace_id.0, body.database_id).await?;
    let created = st
        .endpoints
        .create(
            ctx.workspace_id.0,
            ctx.principal.user_id,
            &method,
            &path,
            &body,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(created)))
}

#[utoipa::path(
    patch,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    request_body = UpdateEndpointInput,
    responses(
        (status = 200, description = "Draft updated", body = DataApiEndpointDto),
        (status = 400, description = "Empty name or path, or an unsupported method", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission", body = ErrorBody),
        (status = 404, description = "No such endpoint, or no such database, in this workspace", body = ErrorBody),
        (status = 409, description = "Another endpoint already serves that method and path", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn update_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
    Json(mut body): Json<UpdateEndpointInput>,
) -> Result<Json<DataApiEndpointDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    let id = scoped_endpoint(&st, ctx.workspace_id.0, id).await?;

    if let Some(name) = &body.name {
        if name.trim().is_empty() {
            return Err(Error::InvalidInput("name must not be empty".into()));
        }
    }
    if let Some(raw) = &body.path {
        body.path = Some(
            normalize_path(raw)
                .ok_or_else(|| Error::InvalidInput("path must not be empty".into()))?,
        );
    }
    if body.method.is_some() {
        body.method = Some(checked_method(body.method.as_deref())?);
    }
    // A PATCH may repoint the endpoint at a different connection, so the same
    // containment applies here as on create — and it has to run before the
    // UPDATE, or a refused database would already have been written.
    if let Some(database_id) = body.database_id {
        scoped_database(&st, ctx.workspace_id.0, database_id).await?;
    }
    Ok(Json(st.endpoints.update(id, &body).await?))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "Endpoint and its published versions deleted", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn delete_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<Ack>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    let id = scoped_endpoint(&st, ctx.workspace_id.0, id).await?;
    // `dataapi.endpoint_versions` cascades from the endpoint (0121), so this
    // takes the published history with it — which is why it is `:manage`.
    st.endpoints.delete(id).await?;
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
        .route(
            "/ws/{workspace}/data-api/endpoints",
            get(list_endpoints).post(create_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}",
            get(get_endpoint)
                .patch(update_endpoint)
                .delete(delete_endpoint),
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
        list_endpoints,
        get_endpoint,
        create_endpoint,
        update_endpoint,
        delete_endpoint,
    ),
    components(schemas(
        DataApiServiceAccountDto,
        DataApiClientDto,
        DataApiNewClientDto,
        CreateServiceAccountInput,
        IssueClientInput,
        DataApiEndpointDto,
        CreateEndpointInput,
        UpdateEndpointInput,
        ParamSpec,
        OutputSpec,
        CacheSpec,
        LimitsSpec,
        RateLimitSpec,
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
}
