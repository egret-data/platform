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
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/validate
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/publish
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/rollback
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/disable
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/enable
//! GET    /api/ws/{workspace}/data-api/endpoints/{id}/versions
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
//!    other is exactly how a cross-workspace hole opens. A service account or
//!    client is resolved through [`scoped_account`] / [`scoped_client`] first;
//!    an endpoint id carries its workspace into [`EndpointStore`], whose every
//!    statement has the predicate built in. Either way the answer on a mismatch
//!    is 404, never 403, so a caller cannot distinguish "no such thing" from
//!    "exists, but not yours".
//! 3. **Every id arriving in a request BODY is resolved against the caller's
//!    workspace before it is stored** — [`scoped_database`] for `database_id`,
//!    [`scoped_folder`] for `folder_id`. These are the identifiers a caller
//!    supplies for objects it does not name in the URL, so nothing else in the
//!    draft path constrains them. For `database_id`, `describe_as` is
//!    workspace-scoped and would refuse it at publish time — but by then the
//!    reference has been accepted, stored, and echoed back to the author as
//!    valid. For `folder_id` nothing downstream would refuse it at all.
//!
//! **`path` and `method` are editable on a `state = 'live'` row, deliberately.**
//! Task 4 answered the question this comment used to leave open: a URL is a
//! property of the endpoint OBJECT — uniquely constrained, renameable — while
//! behaviour is a property of the frozen version. P4 routes on
//! `dataapi.endpoints (method, path)` (the pair carrying `UNIQUE (workspace_id,
//! method, path)`, so two endpoints can never claim one URL) and then serves
//! entirely from the snapshot. See `lifecycle.rs`' module comment for the full
//! contract.

use axum::extract::{FromRef, Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
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
    ALLOWED_METHODS, FOLDER_ENTITY_TYPE,
};
use crate::lifecycle::{validate, DataApiEndpointVersionDto, DataApiPublishedDto, VersionRef};

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
    /// The Query seam, as a PORT. Publishing resolves an endpoint's output
    /// columns through `describe_as` and freezes them into the snapshot, so the
    /// published contract cannot drift when the underlying table changes. Held as
    /// `Arc<dyn …>` rather than a concrete `EngineState` so this crate never
    /// depends on `egret-query` and the seam can move to its own binary by
    /// swapping the implementation.
    pub query: Arc<dyn egret_query_api::QueryExecPort>,
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

// An endpoint id needs no `scoped_endpoint` helper here: `EndpointStore`'s
// `get`/`update`/`delete` take the workspace and carry `AND workspace_id = $n`
// in the statement itself, so containment cannot be bypassed by forgetting to
// call something. See the module comment on `endpoints.rs` for why that layer
// and not this one.

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
/// and `EndpointStore` treat their objects.
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

/// The same for a `folder_id`, which is the other identifier a caller supplies
/// in the body for an object it does not name in the URL — and so has exactly
/// the same failure modes as an unchecked `database_id`.
///
/// Unchecked, a `:manage` holder in workspace A could file an endpoint into
/// workspace B's folder; a folder id that exists nowhere would be a 500 quoting
/// `endpoints_folder_id_fkey`; and 201-versus-500 would be a free existence
/// oracle for folder ids in every other workspace.
///
/// `entity_type` is checked as well, not only the workspace. `analytics.folders`
/// is polymorphic with no constraint on the column (0021/0113), so a
/// workspace-local CHART folder is a perfectly valid uuid here — and an endpoint
/// filed into one disappears from the endpoint list without ever erroring. The
/// wrong TYPE reads as `NotFound` too: to an endpoint author, a folder that
/// cannot hold endpoints is a folder that does not exist.
async fn scoped_folder(st: &DataApiState, ws: Uuid, folder: Uuid) -> Result<(), DataApiError> {
    // Same lookup as `egret_bi::folder::FolderRepo::meta`, issued directly rather
    // than taking a dependency on `egret-bi` — this crate deliberately depends on
    // no other feature seam.
    let meta: Option<(Uuid, String)> =
        sqlx::query_as("SELECT workspace_id, entity_type FROM analytics.folders WHERE id = $1")
            .bind(folder)
            .fetch_optional(&st.db.pool)
            .await
            .map_err(db_err)?;
    match meta {
        Some((owner, etype)) if owner == ws && etype == FOLDER_ENTITY_TYPE => Ok(()),
        _ => Err(Error::NotFound("folder".into())),
    }
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
// an endpoint live, and the SQL a live endpoint runs comes from its frozen
// snapshot rather than from this row. Its URL does not — see the module comment.

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
    // `ctx.workspace_id`, never the path segment — and the store, not a guard
    // here, is what holds it.
    Ok(Json(st.endpoints.get(ctx.workspace_id.0, id).await?))
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
    let path = normalize_path(&body.path).map_err(Error::InvalidInput)?;
    let method = checked_method(body.method.as_deref())?;
    // Both before the insert, so a refused reference never reaches its FK — and
    // so the answer is a 404 about the thing that was wrong rather than a 500
    // about a constraint.
    scoped_database(&st, ctx.workspace_id.0, body.database_id).await?;
    if let Some(folder_id) = body.folder_id {
        scoped_folder(&st, ctx.workspace_id.0, folder_id).await?;
    }
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

    if let Some(name) = &body.name {
        if name.trim().is_empty() {
            return Err(Error::InvalidInput("name must not be empty".into()));
        }
    }
    if let Some(raw) = &body.path {
        body.path = Some(normalize_path(raw).map_err(Error::InvalidInput)?);
    }
    if body.method.is_some() {
        body.method = Some(checked_method(body.method.as_deref())?);
    }
    // A PATCH may repoint the endpoint at a different connection or folder, so
    // the same containment applies here as on create — and it has to run before
    // the UPDATE, or a refused reference would already have been written.
    if let Some(database_id) = body.database_id {
        scoped_database(&st, ctx.workspace_id.0, database_id).await?;
    }
    if let Some(Some(folder_id)) = body.folder_id {
        scoped_folder(&st, ctx.workspace_id.0, folder_id).await?;
    }
    Ok(Json(
        st.endpoints.update(ctx.workspace_id.0, id, &body).await?,
    ))
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
    // `dataapi.endpoint_versions` cascades from the endpoint (0121), so this
    // takes the published history with it — which is why it is `:manage`, and why
    // the workspace predicate is in the DELETE rather than in a separate lookup
    // that could go missing.
    st.endpoints.delete(ctx.workspace_id.0, id).await?;
    Ok(Json(Ack::ok()))
}

// ---------- Publish lifecycle (P3 Task 4) ----------
//
// The four routes that make versioning real, plus the version list. Everything
// that decides WHAT a published endpoint returns is frozen by `publish` into
// `dataapi.endpoint_versions.snapshot`; see `lifecycle.rs` for the contract P4's
// serving path has to honour.

/// The `POST /publish` body. Every field optional, so `{}` is a valid publish.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct PublishEndpointInput {
    /// Free-form "what changed", stored on the version.
    #[serde(default)]
    pub note: String,
}

/// The `POST /rollback` body: name the version by NUMBER or by ID, not both.
///
/// `version` is scoped to the endpoint by construction — version numbers are only
/// unique within one endpoint's history — so that spelling cannot name another
/// endpoint's version even in principle. `version_id` is a bare uuid and
/// therefore CAN, which is precisely why the store's statement carries
/// `v.endpoint_id = e.id`. Both exist because a UI listing versions has their ids
/// to hand while a human types a number.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RollbackEndpointInput {
    #[serde(default)]
    pub version: Option<i32>,
    #[serde(default)]
    pub version_id: Option<Uuid>,
}

/// Resolve the draft, its connection's delegation mode, and its output columns.
///
/// Shared by `validate` and `publish` so a definition that validates is exactly a
/// definition that publishes — two separate call sites would be two chances to
/// drift, and the one that drifted would be discovered by a client.
async fn validated_draft(
    st: &DataApiState,
    ws: Uuid,
    principal: UserId,
    id: Uuid,
) -> Result<(DataApiEndpointDto, Vec<egret_dataapi_api::ColumnSpec>), DataApiError> {
    let draft = st.endpoints.get(ws, id).await?;
    let delegated = st
        .endpoints
        .connection_is_delegated(ws, draft.database_id)
        .await?;
    // `ctx.principal`, not the service account: `describe_as` resolves under the
    // caller's own identity so a delegated connection applies THEIR policy, and
    // an author cannot publish a contract over a table they cannot themselves see.
    let columns = validate(&*st.query, ws, principal, &draft, delegated).await?;
    Ok((draft, columns))
}

/// What `POST /validate` answers with.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataApiValidationDto {
    /// The output schema `describe` resolved. This is what `publish` would
    /// freeze, so an author sees the contract before committing to it.
    pub columns: Vec<egret_dataapi_api::ColumnSpec>,
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/validate",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "The definition is publishable; the resolved output schema is returned", body = DataApiValidationDto),
        (status = 400, description = "Not a single SELECT, a parameter mismatch, an enum without an allowed list, a shared cache on a delegated connection, or SQL the engine rejects", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission, or the engine's policy denies a table the query names", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace, or the query names a relation that does not exist", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn validate_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<DataApiValidationDto>, DataApiError> {
    // `:manage`, not `:read` — this runs a query against the engine under the
    // caller's identity, which is an authoring action and not a read of metadata.
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    let (_, columns) = validated_draft(&st, ctx.workspace_id.0, ctx.principal.user_id, id).await?;
    Ok(Json(DataApiValidationDto { columns }))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/publish",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    request_body = PublishEndpointInput,
    responses(
        (status = 200, description = "A new immutable version was frozen and is now live", body = DataApiPublishedDto),
        (status = 400, description = "The definition does not validate", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission, or the engine's policy denies a table", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 409, description = "The draft was edited while it was being published", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn publish_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
    body: Option<Json<PublishEndpointInput>>,
) -> Result<Json<DataApiPublishedDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    let note = body.map(|Json(b)| b.note).unwrap_or_default();
    let ws = ctx.workspace_id.0;
    // Validation and the describe both run BEFORE any write — see
    // `EndpointStore::publish` on why that is stronger than wrapping them in the
    // transaction, not weaker.
    let (draft, columns) = validated_draft(&st, ws, ctx.principal.user_id, id).await?;
    Ok(Json(
        st.endpoints
            .publish(ws, id, ctx.principal.user_id, &note, &draft, columns)
            .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/rollback",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    request_body = RollbackEndpointInput,
    responses(
        (status = 200, description = "The named version is live again", body = DataApiPublishedDto),
        (status = 400, description = "Name exactly one of `version` or `version_id`", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission", body = ErrorBody),
        (status = 404, description = "No such endpoint here, or no such version OF THIS ENDPOINT", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn rollback_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
    Json(body): Json<RollbackEndpointInput>,
) -> Result<Json<DataApiPublishedDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    let target = match (body.version, body.version_id) {
        (Some(n), None) => VersionRef::Number(n),
        (None, Some(v)) => VersionRef::Id(v),
        _ => {
            return Err(Error::InvalidInput(
                "name exactly one of `version` (a number) or `version_id` (a uuid)".into(),
            ))
        }
    };
    Ok(Json(
        st.endpoints
            .rollback(ctx.workspace_id.0, id, target)
            .await?,
    ))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/disable",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "Stopped serving. The live version pointer is kept, so re-enabling needs no re-publish.", body = DataApiEndpointDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn disable_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<DataApiEndpointDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    Ok(Json(st.endpoints.disable(ctx.workspace_id.0, id).await?))
}

/// The other half of `disable`, and the reason `disable` keeps
/// `live_version_id`: without this route that promise is unfalsifiable, and an
/// endpoint taken out of service could only come back by publishing a NEW
/// version — a fresh describe, a new version number, and a fresh chance for the
/// definition to have drifted in the meantime.
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/enable",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "Serving again, from the same version that was live before", body = DataApiEndpointDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 409, description = "The endpoint has never been published, so there is nothing to serve", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn enable_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<DataApiEndpointDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    Ok(Json(st.endpoints.enable(ctx.workspace_id.0, id).await?))
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/versions",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "Published versions, newest first, each with its frozen snapshot", body = Vec<DataApiEndpointVersionDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:read permission", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_endpoint_versions(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<DataApiEndpointVersionDto>>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_READ).await?;
    Ok(Json(st.endpoints.versions(ctx.workspace_id.0, id).await?))
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
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/validate",
            post(validate_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/publish",
            post(publish_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/rollback",
            post(rollback_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/disable",
            post(disable_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/enable",
            post(enable_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/versions",
            get(list_endpoint_versions),
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
        validate_endpoint,
        publish_endpoint,
        rollback_endpoint,
        disable_endpoint,
        enable_endpoint,
        list_endpoint_versions,
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
        PublishEndpointInput,
        RollbackEndpointInput,
        DataApiValidationDto,
        DataApiPublishedDto,
        DataApiEndpointVersionDto,
        egret_dataapi_api::EndpointSnapshot,
        egret_dataapi_api::ColumnSpec,
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
