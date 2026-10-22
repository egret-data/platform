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
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/trash
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/restore
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/bookmark
//! DELETE /api/ws/{workspace}/data-api/endpoints/{id}/bookmark
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/validate
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/publish
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/rollback
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/disable
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/enable
//! GET    /api/ws/{workspace}/data-api/endpoints/{id}/versions
//!
//! GET    /api/ws/{workspace}/data-api/endpoints/{id}/grants
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/grants
//! DELETE /api/ws/{workspace}/data-api/endpoints/{id}/grants/{service_account_id}
//! POST   /api/ws/{workspace}/data-api/endpoints/{id}/try
//!
//! GET    /api/ws/{workspace}/data-api/openapi
//! ```
//!
//! Three invariants this module exists to hold, all of them security properties
//! rather than conveniences:
//!
//! 1. **Issuing a client gates on `service_account:manage`, never `:read`.** A
//!    client is a bearer credential for the account's data access; handing one
//!    out is strictly more powerful than looking at the list. The same rule puts
//!    the three `/grants` routes — including the LIST — on
//!    `data_api_endpoint:manage`: an invoke grant is what makes an endpoint
//!    callable at all, so writing one mutates the authorization graph and
//!    reading them enumerates every consumer of the endpoint. `GET
//!    …/data-api/openapi` is on `:manage` for the same reason and is the
//!    stronger case — it answers that question per account and iterable.
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

use axum::extract::{FromRef, Path, Query, State};
use axum::http::{header, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_core::http::ErrorBody;
use egret_core::{Ack, Error, UserId, WorkspacePath};
use egret_dataapi_api::{
    validate_statements, CacheSpec, LimitsSpec, OutputSpec, ParamSpec, RateLimitSpec,
};
use egret_metastore::Db;
use egret_workspace::context::WsContext;
use egret_workspace::rbac::Authorizer;
use egret_workspace::workspaces::WorkspaceRepo;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::activity::{
    ActivityCallDto, ActivityRange, ActivityRow, ActivityStore, ActivitySummaryDto,
};
use crate::clients::ClientStore;
use crate::endpoints::{
    normalize_path, CreateEndpointInput, DataApiEndpointDto, DataApiEndpointListItemDto,
    EndpointStore, UpdateEndpointInput, ALLOWED_METHODS, FOLDER_ENTITY_TYPE,
};
use crate::lifecycle::{validate, DataApiEndpointVersionDto, DataApiPublishedDto, VersionRef};

/// View the accounts and their clients. Deliberately does NOT imply issuing.
const SA_READ: &str = "service_account:read";
/// Create/delete accounts and ISSUE clients. Issuing lives here because a
/// client hands out that account's access.
const SA_MANAGE: &str = "service_account:manage";

/// List and inspect endpoint drafts and their versions.
///
/// Re-exported from `egret-dataapi-api` (Task 4) rather than defined here: it
/// is now the ONE definition, and `egret-automation` names the same
/// permission through that crate without depending on this one.
use egret_dataapi_api::EP_READ;
/// Author endpoints. Flat, like every permission here: it implies no `:read`,
/// and `:read` implies none of it.
///
/// `pub(crate)` because [`crate::try_it`]'s handler gates on it too — running an
/// endpoint as somebody else is an authoring act, not a reading one — and the
/// three action strings stay one list rather than a literal per file.
pub(crate) const EP_MANAGE: &str = "data_api_endpoint:manage";
/// Call a published endpoint (spec §6.2 step 4).
///
/// Never checked by a route in this module — it is the action the SERVING path
/// checks, object-scoped, against a service account (see
/// [`crate::serving::router`]). It lives here beside its siblings so the three
/// action strings are one list rather than a literal in each file that needs
/// one; the routes below hand it out, they do not require it.
pub(crate) const EP_INVOKE: &str = "data_api_endpoint:invoke";

/// The built-in role an invoke grant names.
///
/// Seeded by migration 0124 with EXACTLY one permission, [`EP_INVOKE`], and
/// workspace-scoped so the PDP's cascading `resource in ?resource` form matches
/// an object resource. An object-scoped grant links its role's whole effective
/// action set onto the object, so this cannot be `ws_viewer` (as `egret-bi`'s
/// share API uses) without handing a service account every viewer action on the
/// endpoint it was merely meant to call.
const INVOKE_ROLE: &str = "data_api_invoker";

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
    /// The machine-IdP's signing keys — the SAME store `IdpState` mints with, so
    /// there is one source of truth for what egret's signing key currently is.
    ///
    /// **Sharing it does NOT make key rotation safe**, and it would be easy to
    /// read it that way: `verify_serving_token` verifies against `active()`
    /// only, and mint and verify both follow `active()` forward together, so at
    /// a rotation every already-issued token becomes unverifiable no matter how
    /// many instances of the store exist. Closing that needs verification
    /// against every JWK from `all_public_jwks()`; deferred, because no rotation
    /// tooling exists in-tree yet and the Data API TTL is 600s.
    ///
    /// Read only by [`crate::serving`]: `verify_serving_token` takes the active
    /// key's public JWK. The management routes above authenticate a HUMAN
    /// through `WsContext`/`SessionService` and never touch this.
    pub keys: Arc<dyn egret_machine_idp::SigningKeyStore>,
    /// The machine-IdP's `iss` — a TOKEN CLAIM, not an address anyone fetches.
    ///
    /// Stamped into the tokens this deployment signs and byte-compared by
    /// StarRocks' `required_issuer`, so it must stay exactly what
    /// `catalog.public_url` derives (in-cluster, that is a Service DNS name).
    /// **The URLs handed to a consumer come from [`Self::consumer_base_url`]
    /// instead** — the defect this pair exists to prevent was one value serving
    /// both audiences, which advertised a cluster-internal hostname to callers
    /// outside the cluster. Only [`crate::try_it`]'s trial-token minter reads
    /// this field.
    pub issuer: String,
    /// The base URL a CONSUMER uses to reach this deployment — an ADDRESS
    /// someone fetches, not a claim.
    ///
    /// Every URL that leaves the seam for a human or a client generator is built
    /// from it: the `token_endpoint` returned with a new client, the generated
    /// document's `servers[0].url` and `tokenUrl` (both from this one value —
    /// see `openapi_gen`), and the URL and `curl` line Try-it shows the author.
    ///
    /// Configured as `data_api.public_url` and resolved in `egret-bin` beside
    /// the issuer it falls back to, already trimmed of a trailing slash. Equal
    /// to [`Self::issuer`] unless a deployment sets that key, which is what
    /// makes this a widening.
    pub consumer_base_url: String,
    /// The append-only audit trail. The SAME sink `CpState` receives, so a Data
    /// API mutation lands in the one `audit_log` an operator already reads
    /// rather than in a second, seam-local trail nobody queries.
    ///
    /// [`egret_audit::AuditSink::emit`] is a non-blocking `try_send` that drops
    /// (with a warn) on a full channel and never returns an error, which is why
    /// no handler below changes its signature to carry an audit failure. That is
    /// also the honest bound on what these events promise: an audit record is
    /// best-effort evidence, never a precondition of the mutation.
    pub audit: egret_audit::AuditSink,
    /// The serving surface's response cache (spec §6.2 step 6).
    ///
    /// Held in the state rather than built per request for the obvious reason —
    /// a cache constructed per call is not one.
    ///
    /// Behind a trait since Package 4, for the same reason `rate_limits` below
    /// always was: it was a concrete `ResponseCache` with no seam at all, so a
    /// deployment that scaled out fragmented its cache with nothing to swap in.
    /// `crate::serving::shared::SharedStore` is the other implementation and
    /// `choose_store` decides which one this process gets.
    pub cache: std::sync::Arc<dyn crate::serving::ResponseStore>,
    /// The serving surface's rate-limit buckets (spec §6.2 step 5).
    ///
    /// Behind a trait so a multi-replica deployment can swap a shared store in
    /// without a call site changing; in-process buckets are correct only while
    /// the serving surface is in-process, and splitting the binary would
    /// otherwise multiply every published limit by the replica count in silence.
    /// Package 4 supplied that shared implementation and made >1 replica without
    /// one a startup refusal.
    pub rate_limits: std::sync::Arc<dyn crate::serving::RateLimitStore>,
    /// The Activity tab's store (P5 §4) — call statistics from `dataapi.access_log`.
    pub activity: ActivityStore,
    /// Recompiles the engine policies a service account's group memberships
    /// reach when the account is deleted (J-F3 Task 1b): a group compiles to
    /// its members' usernames, and `UserRepo::hard_delete` removes the account
    /// from every group. A PORT, like [`Self::query`], so this crate never
    /// depends on `egret-perm-compile`; `egret-bin` wires
    /// `egret_perm_compile::PermGroupRecompiler`. Not optional — see
    /// `egret_auth::groups`' module doc.
    pub group_recompiler: Arc<dyn egret_auth::groups::GroupMembershipRecompiler>,
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
    // Last full secret verification. The IdP serves repeats from a
    // verification cache, so this advances at most about once per 10 minutes
    // per (client, secret) — not on every token request. An RFC 8693 token
    // exchange verifies no secret and does not advance it, so a client that
    // only refreshes by exchange can read hours old while in use. (A `//`
    // comment on purpose: a doc comment would change the checked-in
    // openapi.json.)
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
pub(crate) async fn scoped_account(
    st: &DataApiState,
    ws: Uuid,
    sa: Uuid,
) -> Result<Uuid, DataApiError> {
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
    // The `users` row is written by `UserRepo::insert_service_with`, NOT by an
    // INSERT spelled out here. P2 inlined one because `insert_service` ran on
    // the pool and could not join this transaction — which left the repo method
    // with zero production callers, two copies of the same statement free to
    // drift, and `egret-auth`'s tests pinning the copy production had stopped
    // taking. The method is now generic over the executor, so `&mut *tx`
    // satisfies it and there is exactly one write again.
    let mut tx = st.db.pool.begin().await.map_err(db_err)?;
    let inserted: Result<(UserId, DateTime<Utc>), DataApiError> = async {
        // Either UNIQUE can fire and both mean the same thing to the caller:
        // this workspace already has that service account. `users.email` is
        // derived from the name and the workspace slug, so an email collision IS
        // a name collision seen from the other table — which is why
        // `insert_service_with`'s own "user already exists" is re-worded here
        // rather than surfaced. It names a synthetic address the caller never
        // chose and would read as a platform-wide clash.
        let user_id = UserRepo::insert_service_with(&mut *tx, &email, name)
            .await
            .map_err(|e| match e {
                Error::Conflict(_) => {
                    Error::Conflict(format!("service account already exists: {name}"))
                }
                other => other,
            })?;
        let created_at: DateTime<Utc> = sqlx::query_scalar(
            "INSERT INTO dataapi.service_accounts (user_id, workspace_id, name, description, created_by) \
             VALUES ($1, $2, $3, $4, $5) RETURNING created_at",
        )
        .bind(user_id.0)
        .bind(ctx.workspace_id.0)
        .bind(name)
        .bind(&body.description)
        .bind(ctx.principal.user_id.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                Error::Conflict(format!("service account already exists: {name}"))
            }
            _ => db_err(e),
        })?;
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
            return Err(e);
        }
    };

    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "service_account:create",
        "service_account",
        Some(user_id.0.to_string()),
        serde_json::json!({ "name": name, "email": email }),
    ));

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
    // It also removes the account from every group, and since J-F3 Task 1b a
    // group compiles to its members — so the policies those groups reach are
    // recompiled in the same transaction, through `group_recompiler`.
    let deleted = st
        .users
        .hard_delete(UserId(sa), &*st.group_recompiler)
        .await?;
    // Deleting the `users` row cascades through `dataapi.service_accounts` into
    // `dataapi.clients` (migration 0119), so no client outlives its account.
    //
    // The hardest of the four to add later and the most important to have:
    // this hard-deletes a machine principal along with its `access_grants` and
    // every client it owns. Nothing else survives to say it existed. The
    // columns come from `hard_delete`'s own `RETURNING`, so there is no second
    // query and nothing to race.
    //
    // `hard_delete` returns `None` only in the TOCTOU window where `sa` was
    // deleted between the `scoped_account` check above and the row `DELETE`
    // itself — `hard_delete` treats that as the idempotent success it has
    // always been for its other caller, not an error, so there is nothing
    // this route actually destroyed to describe. Emitting a `delete` event
    // with fabricated or absent columns would be worse than emitting none.
    if let Some((name, email)) = deleted {
        st.audit.emit(egret_audit::mutation_for(
            &ctx.principal,
            Some(ctx.workspace_id.0),
            "service_account:delete",
            "service_account",
            Some(sa.to_string()),
            serde_json::json!({ "name": name, "email": email }),
        ));
    }
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
    // **`c.client_secret` is deliberately absent from `detail`, and this is the
    // one place in the crate where it could have leaked.** `audit_log` is
    // append-only and read by more people than the issue response ever reaches,
    // so a secret written here outlives every rotation, every revocation and the
    // client row itself — the one credential lifetime nothing can shorten. The
    // `client_id` and the account identify WHICH credential was minted, which is
    // the whole question an auditor asks; the secret answers none of it.
    // `issuing_a_client_audits_the_client_id_and_never_the_secret` in
    // `tests/audit.rs` holds this, checking the KEY and the secret's VALUE — a
    // renamed field would walk straight past a key-only assertion.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_client:issue",
        "data_api_client",
        Some(c.id.to_string()),
        serde_json::json!({
            "client_id": c.client_id,
            "service_account_id": sa,
            "name": body.name,
            "expires_at": body.expires_at.map(|t| t.to_rfc3339()),
        }),
    ));
    Ok((
        StatusCode::CREATED,
        Json(DataApiNewClientDto {
            id: c.id,
            client_id: c.client_id,
            client_secret: c.client_secret,
            // The CONSUMER-facing origin, never `issuer`: this URL is handed
            // to whoever holds the credential, and in-cluster the issuer is a
            // Service DNS name they cannot resolve.
            token_endpoint: format!("{}/oauth/token", st.consumer_base_url.trim_end_matches('/')),
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
    // Emitted AFTER the revoke, so the trail records what happened rather than
    // what was attempted: `revoke` returning `Err` short-circuits above and no
    // event is written for a revocation that did not take.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_client:revoke",
        "data_api_client",
        Some(id.to_string()),
        serde_json::json!({}),
    ));
    Ok(Json(Ack::ok()))
}

// ---------- Endpoint drafts (P3 Task 3) ----------
//
// Draft CRUD only. Nothing here writes `state` or `live_version_id`: publishing,
// rollback and disable are a separate lifecycle, so no amount of editing can put
// an endpoint live, and the SQL a live endpoint runs comes from its frozen
// snapshot rather than from this row. Its URL does not — see the module comment.

/// The endpoint list's only query parameter.
///
/// `#[serde(default)]` so an absent `?trashed=` means "the ones that are not in
/// the trash" — the answer this route gave before trash existed, so no caller
/// that predates it changes behaviour. The name and shape are `egret-bi`'s
/// dataset and dashboard lists', because a second spelling of one idea is a thing
/// for the frontend to get wrong once per screen.
#[derive(Debug, Deserialize)]
struct ListEndpointsQuery {
    #[serde(default)]
    trashed: bool,
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/endpoints",
    tag = "data-api",
    params(
        WorkspacePath,
        ("trashed" = Option<bool>, Query, description = "List only trashed endpoints when true; only untrashed ones when absent or false"),
    ),
    responses(
        (status = 200, description = "Endpoint drafts in the workspace, each with the caller's own `bookmarked` flag", body = Vec<DataApiEndpointListItemDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_endpoints(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ListEndpointsQuery>,
) -> Result<Json<Vec<DataApiEndpointListItemDto>>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_READ).await?;
    // The user comes from the SESSION. `bookmarked` answers "did I star this",
    // and there is nowhere else it could legitimately come from.
    Ok(Json(
        st.endpoints
            .list_for(ctx.workspace_id.0, ctx.principal.user_id.0, q.trashed)
            .await?,
    ))
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
        (status = 409, description = "Another endpoint's draft already sits at that method and path", body = ErrorBody),
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
    // Before the insert, so the schema's non-empty CHECK never has to answer for
    // it: a constraint violation is a 500 with a constraint name in it, and this
    // is a 400 that names the statement and the rule it broke. Same division of
    // labour as `normalize_path` and the `method` CHECK.
    validate_statements(&body.statements).map_err(|e| Error::InvalidInput(e.to_string()))?;
    // Same argument one field over: `output.envelope` decides the SHAPE of every
    // response body, nothing in the schema constrains it, and an unrecognised
    // value is stored, echoed back to the author as if it were accepted, and
    // then served as the other shape with only a log line to say so.
    if let Some(output) = &body.output {
        egret_dataapi_api::validate_envelope(output)
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
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
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:create",
        "data_api_endpoint",
        Some(created.id.to_string()),
        serde_json::json!({
            "name": created.name,
            "path": created.path,
            "method": created.method,
            "database_id": created.database_id,
        }),
    ));
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
        (status = 409, description = "Another endpoint's draft already sits at that method and path, or the endpoint is in the trash — restore it before editing it. A new path or method on a live endpoint's draft is saved but not served: it goes live at the next publish", body = ErrorBody),
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
    // A PATCH replaces the whole list, so the same rules apply to it as to a
    // create — an author cannot edit their way to an empty or ambiguous one.
    if let Some(statements) = &body.statements {
        validate_statements(statements).map_err(|e| Error::InvalidInput(e.to_string()))?;
    }
    // A PATCH replaces the whole `output` object when it carries one, so the
    // envelope rule applies here exactly as it does on create.
    if let Some(output) = &body.output {
        egret_dataapi_api::validate_envelope(output)
            .map_err(|e| Error::InvalidInput(e.to_string()))?;
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
    let updated = st.endpoints.update(ctx.workspace_id.0, id, &body).await?;
    // Records the URL as it now stands, not a field-by-field diff: `path` and
    // `method` are editable on a live row, so "which URL does this endpoint
    // claim" is the question an auditor asks, and a diff of unchanged fields
    // would bury it.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:update",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({
            "name": updated.name,
            "path": updated.path,
            "method": updated.method,
            "database_id": updated.database_id,
        }),
    ));
    Ok(Json(updated))
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
    let gone = st.endpoints.delete(ctx.workspace_id.0, id).await?;
    // **The only audited route whose subject does not survive it.** `revoke` is a
    // soft `UPDATE revoked_at` and `disable` keeps the row, so for those the
    // `resource_id` alone is enough — an auditor joins back to a row that is
    // still there. Here the endpoint is gone and `endpoint_versions` cascaded
    // with it, so a bare uuid in the trail would resolve to nothing and record
    // that SOMETHING was deleted without ever saying which URL stopped being
    // served. The columns come from the DELETE's own `RETURNING`, so there is no
    // second query and nothing to race.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:delete",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({
            "name": gone.name,
            "path": gone.path,
            "method": gone.method,
            "database_id": gone.database_id,
        }),
    ));
    Ok(Json(Ack::ok()))
}

// ---------- Trash ----------
//
// `DELETE …/endpoints/{id}` above keeps its meaning: permanent, cascading the
// published versions away. These two are the reversible pair beside it.
//
// Both gate on [`EP_MANAGE`] — the same permission as `delete` and `disable`, and
// not `:read`. Trashing takes an endpoint out of the authoring surface and, for a
// disabled one, is the step before deleting it; restoring puts it back. Neither is
// a per-user view preference like a bookmark.
//
// Both emit an audit event, which is where this pair departs from the dataset
// trash in `egret-bi` that it otherwise copies. Every lifecycle mutation in this
// module is audited — create, update, delete, publish, rollback, disable, enable,
// grant, revoke — and only the per-user bookmark routes are not. An unaudited
// trash would be the one way to remove an endpoint from every list that leaves no
// trace of who did it, sitting next to a `delete` whose whole justification is
// that the trail is the only surviving record.

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/trash",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "Endpoint moved to the trash. Reversible: the row, its published versions and its invoke grants are all kept.", body = DataApiEndpointDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 409, description = "The endpoint is live — disable it first, so trashing can never take a served URL off the internet by surprise", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn trash_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<DataApiEndpointDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    // The live-state refusal is inside `trash`, in the UPDATE's own `WHERE`, so it
    // cannot be bypassed by a second caller of the store and cannot race a
    // concurrent publish. Migration 0126's CHECK is the backstop under it — but a
    // constraint violation reaches the author as a 500 naming
    // `dataapi_endpoints_trashed_not_live`, and what they need to be told is to
    // disable it first.
    let trashed = st.endpoints.trash(ctx.workspace_id.0, id).await?;
    // `path` and `method` go in the detail for the same reason they do on
    // `delete`: trashing removes the endpoint from every list an auditor would
    // join back to, and while the row survives, "which URL stopped being
    // authorable" is the question asked of the trail. `deleted_at` is the fact
    // itself.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:trash",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({
            "name": trashed.name,
            "path": trashed.path,
            "method": trashed.method,
            "state": trashed.state,
            "deleted_at": trashed.deleted_at,
        }),
    ));
    Ok(Json(trashed))
}

/// The other half of `trash`, and the reason trash keeps the invoke grants and the
/// live version pointer: without this route that promise is unfalsifiable.
///
/// Comes back `disabled`, never `live`. See [`EndpointStore::restore`].
#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/restore",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "Endpoint restored from the trash, `disabled` — never straight back to serving", body = DataApiEndpointDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 409, description = "The endpoint is not in the trash", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn restore_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<DataApiEndpointDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    let restored = st.endpoints.restore(ctx.workspace_id.0, id).await?;
    // Symmetric with `trash`, and not optional: auditing one half of a reversible
    // pair records that an endpoint left the authoring surface and never that it
    // came back. `state` is in the detail because the state restore lands in is
    // the decision this route makes — a trail showing `live` here would be the
    // first sign that the "never straight back to serving" rule had been lost.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:restore",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({
            "name": restored.name,
            "path": restored.path,
            "method": restored.method,
            "state": restored.state,
        }),
    ));
    Ok(Json(restored))
}

// ---------- Bookmarks ----------
//
// A bookmark is a per-user VIEW PREFERENCE, so both routes gate on [`EP_READ`],
// exactly as every sibling seam does — `egret-bi`'s dataset, chart and dashboard
// stars are all on their `:read`. Gating them on `:manage` would be a category
// error with teeth: permissions here are FLAT, so a plain reader holds no
// `:manage` at all and could never star anything they can see.
//
// The user is [`WsContext`]'s principal and never anything from the request. A
// bookmark that could name its own user would be a way to star things on other
// people's behalf.
//
// `analytics.bookmarks` is generic — `(user_id, entity_type, entity_id)` with no
// workspace of its own — so unlike every other id route in this module the
// containment predicate cannot ride along in the statement. Hence the explicit
// [`EndpointStore::scoped`] first, which answers a foreign id with 404 rather
// than 403 and, being first, ensures nothing is written before the answer.

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/bookmark",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "Endpoint bookmarked for the caller", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:read permission", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn bookmark_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<Ack>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_READ).await?;
    st.endpoints.scoped(ctx.workspace_id.0, id).await?;
    st.endpoints
        .add_bookmark(ctx.principal.user_id.0, id)
        .await?;
    Ok(Json(Ack::ok()))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/bookmark",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "Endpoint bookmark removed for the caller", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:read permission", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn unbookmark_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<Ack>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_READ).await?;
    st.endpoints.scoped(ctx.workspace_id.0, id).await?;
    st.endpoints
        .remove_bookmark(ctx.principal.user_id.0, id)
        .await?;
    Ok(Json(Ack::ok()))
}

// ---------- Publish lifecycle (P3 Task 4) ----------
//
// The four routes that make versioning real, plus the version list. Everything
// that decides WHAT a published endpoint returns is frozen by `publish` into
// `dataapi.endpoint_versions.snapshot`; see `lifecycle.rs` for the contract P4's
// serving path has to honour.

/// The `POST /publish` body. Every field optional, so `{}` is a valid publish —
/// of a version that breaks nobody.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct PublishEndpointInput {
    /// Free-form "what changed", stored on the version.
    #[serde(default)]
    pub note: String,
    /// The acknowledgement issued by `POST …/validate` for the breaking changes
    /// this publish would make, required only when there are some.
    ///
    /// # Why this is a token and not `force: true`
    ///
    /// A boolean can be set once, in a deploy script, by someone who has never
    /// read a diff — and every breaking publish that script ever makes is then
    /// pre-approved, silently. There is no constant to hardcode here: the value
    /// is a digest of the breaking changes themselves, mixed with the endpoint
    /// and the version currently live, so it cannot be produced without having
    /// been handed the list it acknowledges, and one issued for yesterday's
    /// break does not match today's. Acknowledging accidentally is not a
    /// discipline this field asks anyone to keep; it is not reachable.
    ///
    /// Sending one when the publish breaks nothing is refused too, rather than
    /// ignored — an acknowledgement that does not correspond to anything is a
    /// stale one, and staying silent about it is how a client learns to send it
    /// always.
    #[serde(default)]
    pub acknowledge_breaking: Option<String>,
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

/// Resolve the draft, its connection's [`ConnectionSafety`], and its output
/// columns.
///
/// Shared by `validate` and `publish` so a definition that validates is exactly a
/// definition that publishes — two separate call sites would be two chances to
/// drift, and the one that drifted would be discovered by a client.
async fn validated_draft(
    st: &DataApiState,
    ws: Uuid,
    principal: UserId,
    id: Uuid,
) -> Result<
    (
        DataApiEndpointDto,
        crate::lifecycle::ConnectionSafety,
        crate::lifecycle::ValidatedDraft,
    ),
    DataApiError,
> {
    let draft = st.endpoints.get(ws, id).await?;
    let conn = st
        .endpoints
        .connection_safety(ws, draft.database_id)
        .await?;
    // `ctx.principal`, not the service account: `describe_as` resolves under the
    // caller's own identity so a delegated connection applies THEIR policy, and
    // an author cannot publish a contract over a table they cannot themselves see.
    let validated = validate(&*st.query, ws, principal, &draft, conn).await?;
    Ok((draft, conn, validated))
}

/// What `POST /validate` answers with.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataApiValidationDto {
    /// Exactly what `publish` would freeze: one entry per statement, in the
    /// author's order, each carrying the output schema `describe` resolved for
    /// it — so an author sees the contract before committing to it.
    ///
    /// A LIST rather than the single `columns` this carried before M2, because
    /// an endpoint has one column set per statement and folding them into one
    /// would report a contract no version will ever hold: two statements each
    /// returning a `total` are two result sets, not one column declared twice.
    /// [`egret_dataapi_api::StatementSnapshot`] rather than a new pair type,
    /// because it IS the frozen shape and reusing it keeps "what validate says"
    /// and "what publish writes" the same words.
    pub statements: Vec<egret_dataapi_api::StatementSnapshot>,
    /// What publishing this draft would do to the consumers of the version
    /// currently live (package 6).
    ///
    /// `null` for an endpoint that has never been published — the FIRST publish,
    /// where no consumer has ever received anything and there is nothing to
    /// break. That is a different fact from an empty diff, which means the two
    /// contracts were compared and found compatible, and the two are kept apart
    /// here for the same reason they are kept apart in the column that records
    /// them.
    pub against_live: Option<crate::contract::ContractDiff>,
    /// The value to send back as `acknowledge_breaking` on `POST …/publish`,
    /// present exactly when `against_live.breaking` is non-empty.
    ///
    /// A digest of those breaking changes, not a constant — see
    /// [`PublishEndpointInput::acknowledge_breaking`].
    pub acknowledge_breaking: Option<String>,
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
        (status = 200, description = "The definition is publishable; the resolved output schema is returned, together with what publishing it would do to consumers of the live version and the acknowledgement to send if that includes a breaking change", body = DataApiValidationDto),
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
    let ws = ctx.workspace_id.0;
    let (draft, conn, validated) = validated_draft(&st, ws, ctx.principal.user_id, id).await?;
    // The snapshot publish WOULD freeze, built by the function publish itself
    // builds it with — so the diff an author is shown here and the diff publish
    // refuses on cannot be two different answers to one question.
    let next = crate::lifecycle::prospective_snapshot(
        &draft,
        &validated.statements,
        &validated.iterated,
        conn,
    )
    .map_err(|_| {
        // Unreachable through this route: both halves come from the one
        // `validate` call above, over the same statement list. A 500 rather than
        // publish's 409 because there is no concurrent edit to blame — if this
        // ever fires it is a bug in this crate, and saying "try again" would
        // send the author to do something that cannot help.
        DataApiError::Internal(anyhow::anyhow!(
            "validate described statements the draft does not have"
        ))
    })?;
    let live = st.endpoints.live_snapshot(ws, id).await?;
    let (against_live, acknowledge_breaking) = match live {
        None => (None, None),
        Some((live_version_id, live_snapshot)) => {
            let diff = crate::contract::compare_contracts(&live_snapshot, &next);
            let ack = diff.acknowledgement(id, live_version_id);
            (Some(diff), ack)
        }
    };
    Ok(Json(DataApiValidationDto {
        statements: validated.statements,
        against_live,
        acknowledge_breaking,
    }))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/preview",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    request_body = crate::preview::PreviewEndpointInput,
    responses(
        (status = 200, description = "The draft ran; up to 100 rows, with the engine's own column names", body = crate::preview::DataApiPreviewDto),
        (status = 400, description = "Not a single SELECT, a parameter mismatch, a test value that does not satisfy its declared type, SQL the engine rejects — with the engine's own message — or a `governed` database reached through a connection that does not run queries as the caller, which is refused here for the same reason publishing onto it is", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission, or the engine's policy denies a table the query names", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace, or a database_id belonging to another workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn preview_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
    Json(body): Json<crate::preview::PreviewEndpointInput>,
) -> Result<Json<crate::preview::DataApiPreviewDto>, DataApiError> {
    // `:manage`, exactly like `validate` and for a stronger reason: preview
    // executes author-supplied SQL against a real connection and returns the
    // engine's own error text, which serving deliberately hides from consumers.
    //
    // NOT "as the caller" — that is what this comment used to say and it is
    // false on a connection that is neither delegated nor impersonating, where
    // the engine sees the service account. The identity the ENGINE applies its
    // policy to is decided by `execution::delegation_for`, not by this route
    // passing a `UserId`; on a `governed` database `preview::run` therefore
    // refuses outright rather than reading under the machine identity.
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    // The endpoint row is read for containment only — the draft's CONTENT comes
    // from the body, which is the whole point of this route. A `{id}` naming
    // another workspace's endpoint must 404 here as it does everywhere else,
    // even though nothing in the body depends on the row.
    let _ = st.endpoints.get(ctx.workspace_id.0, id).await?;
    let out = crate::preview::run(&st, ctx.workspace_id.0, ctx.principal.user_id, &body).await?;
    Ok(Json(out))
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
        (status = 409, description = "The draft was edited while it was being published; the endpoint is in the trash — restore it first; or this version would break consumers of the live one and `acknowledge_breaking` was absent, stale, or sent when nothing breaks. The message names every breaking change; or another endpoint is still served at this draft's method and path (it keeps its published URL until it is published again)", body = ErrorBody),
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
    let (note, ack) = body
        .map(|Json(b)| (b.note, b.acknowledge_breaking))
        .unwrap_or_default();
    let ws = ctx.workspace_id.0;
    // Validation and the describe both run BEFORE any write — see
    // `EndpointStore::publish` on why that is stronger than wrapping them in the
    // transaction, not weaker.
    let (draft, _conn, validated) = validated_draft(&st, ws, ctx.principal.user_id, id).await?;
    let published = st
        .endpoints
        .publish(
            ws,
            id,
            crate::lifecycle::PublishRequest {
                by: ctx.principal.user_id,
                note: &note,
                draft: &draft,
                described: validated.statements,
                acknowledge_breaking: ack.as_deref(),
            },
        )
        .await?;
    // The version NUMBER is the load-bearing field: `dataapi.endpoint_versions`
    // is immutable, so "who put version N live, and when" is the question the
    // trail has to answer, and it is the one thing a later rollback cannot
    // rewrite.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ws),
        "data_api_endpoint:publish",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({
            "version": published.version,
            "version_id": published.version_id,
            "note": note,
            // What the author accepted, in the trail as well as on the version
            // row. `null` for a first publish, where nothing was compared —
            // distinct from `[]`, which means it was and broke nothing.
            "breaking_changes": published.breaking_changes,
            // And WHICH version that is a claim about. After a rollback the
            // live pointer is not the newest row, so `[]` alone does not say
            // whose consumers survived — see migration `0131`.
            "compared_against": published.compared_against,
        }),
    ));
    Ok(Json(published))
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
        (status = 409, description = "The endpoint is in the trash — restore it before changing which version it serves; or the named version can no longer be published onto its database (it became `governed` without a per-principal connection, or a `shared` cache became unsafe), which is refused here rather than left to 503 on every request; or another endpoint is now served at the URL that version was published at", body = ErrorBody),
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
    let rolled = st
        .endpoints
        .rollback(ctx.workspace_id.0, id, target)
        .await?;
    // The version the endpoint is now serving, resolved — not the `VersionRef`
    // the caller typed. A rollback named by number and one named by id are the
    // same act, and the trail should not make an auditor reconstruct which.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:rollback",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({
            "version": rolled.version,
            "version_id": rolled.version_id,
            // What this rollback did to the consumers of the version it moved
            // OFF. A rollback is not refused for breaking them — it is usually
            // the end of an outage — but until this it left no record anywhere
            // that they had been moved, and a later publish comparing against
            // the restored version could then record an empty diff an auditor
            // would read as "nobody was broken here". Classified by the same
            // function `publish` uses. `null` when there was no live version to
            // move off, or when a stored snapshot could not be read.
            "moves_consumers_off": rolled.rollback_breaks,
        }),
    ));
    Ok(Json(rolled))
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
        (status = 409, description = "The endpoint is in the trash — restore it first", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn disable_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<DataApiEndpointDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    let disabled = st.endpoints.disable(ctx.workspace_id.0, id).await?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:disable",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({ "state": disabled.state }),
    ));
    Ok(Json(disabled))
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
        (status = 409, description = "The endpoint has never been published, so there is nothing to serve, or it is in the trash — restore it first", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn enable_endpoint(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<DataApiEndpointDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    let enabled = st.endpoints.enable(ctx.workspace_id.0, id).await?;
    // Symmetric with `disable`, and not optional. Auditing one half of a state
    // machine records that an endpoint stopped serving and never that it started
    // again — the trail would show a permanently-disabled endpoint answering
    // requests. `live_version_id` goes with it because enable resumes serving a
    // version chosen when it was published, and which one that is, is the whole
    // effect of the call.
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:enable",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({
            "state": enabled.state,
            "live_version_id": enabled.live_version_id,
        }),
    ));
    Ok(Json(enabled))
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

// ── Invoke grants (spec §5, §6.2 step 4) ─────────────────────────────────────
//
// The write half of the serving path's authorization check. One `access_grants`
// row per (service account, endpoint) says "this account may call this
// endpoint"; `crate::serving::router::invoke_allowed` is the only reader, and it
// reads through the PDP rather than through this module.
//
// **All three gate on `data_api_endpoint:manage`, including the LIST.** Handing
// out invoke rights mutates the authorization graph — it is strictly more
// powerful than reading an endpoint, so `:read` must not reach it, exactly as
// issuing a client gates on `service_account:manage` and never `:read`. The list
// is manage-only for the reason `egret-bi`'s `grant_list` is: enumerating an
// object's ACL exposes every grantee, and a caller who may only look at the
// endpoint has no business knowing which consumers call it.
//
// **Both ids are scoped to the caller's workspace**, and through the existing
// helpers rather than a new predicate: the endpoint through `EndpointStore::get`
// (whose statement carries `AND workspace_id = $n` itself) and the service
// account through `scoped_account`. Either mismatch is a 404. Without both, a
// `:manage` holder in workspace A could grant one of A's service accounts on B's
// endpoint — a cross-tenant grant written by an entirely legitimate-looking
// call, and one that the serving path would then honour.
//
// **Revocation is immediate, and that asymmetry with token revocation is the
// operational point of this table.** The grant is read on EVERY request, so a
// DELETE here stops the next call. Revoking the CLIENT does not: the serving
// surface verifies a stateless JWT (spec §6.2 step 1), so an already-issued
// token stays good until `exp` and the 600s TTL is an irreducible bound. When
// the answer to "cut this consumer off right now" has to be *now*, it is a grant
// revocation; revoking the client only stops the next token being minted.

/// One service account's invoke grant on an endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DataApiEndpointGrantDto {
    /// The `access_grants` row id.
    pub id: Uuid,
    /// The granted service account (its `users.id`).
    pub service_account_id: Uuid,
    /// The account's display name, so the list is readable without a second
    /// call. `None` if the account row has since been deleted.
    pub service_account_name: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Whether the account ALSO reaches invoke through the workspace (a
    /// workspace role carrying `data_api_endpoint:invoke`), so that revoking
    /// this grant would not remove its access. Asked of the PDP at workspace
    /// scope, where this grant cannot count — see `invoke::inherits_invoke`.
    pub inherits_invoke: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct GrantInvokeInput {
    /// The service account to grant. Must live in this workspace.
    pub service_account_id: Uuid,
}

/// The id of the built-in [`INVOKE_ROLE`] role.
///
/// Resolved per call rather than cached in [`DataApiState`]: it is one indexed
/// lookup on a route that already does several, and a cached id would be a
/// second source of truth for something migration 0124 owns. `Internal`, not
/// `NotFound`, when it is missing — the row is seeded by a migration, so its
/// absence is a broken deployment and not anything the caller did.
async fn invoke_role_id(st: &DataApiState) -> Result<Uuid, DataApiError> {
    sqlx::query_scalar("SELECT id FROM roles WHERE scope = 'workspace' AND name = $1")
        .bind(INVOKE_ROLE)
        .fetch_optional(&st.db.pool)
        .await
        .map_err(db_err)?
        .ok_or_else(|| {
            Error::Internal(anyhow::anyhow!(
                "built-in role `{INVOKE_ROLE}` is missing; migration 0124 has not run"
            ))
        })
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/grants",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    responses(
        (status = 200, description = "Service accounts granted invoke on this endpoint", body = Vec<DataApiEndpointGrantDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage — listing the grantees is a manage operation, not a read", body = ErrorBody),
        (status = 404, description = "No such endpoint in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn list_endpoint_grants(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<DataApiEndpointGrantDto>>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    // 404s an endpoint outside `ctx.workspace_id` before anything is read.
    st.endpoints.get(ctx.workspace_id.0, id).await?;
    let role = invoke_role_id(&st).await?;
    // Joined to `service_accounts` on the WORKSPACE too, so a row somehow
    // written across workspaces is not listed as if it belonged here.
    let rows: Vec<(Uuid, Uuid, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT ag.id, ag.principal_id, sa.name, ag.created_at \
         FROM access_grants ag \
         LEFT JOIN dataapi.service_accounts sa \
           ON sa.user_id = ag.principal_id AND sa.workspace_id = $3 \
         WHERE ag.scope_type = 'object' AND ag.object_type = $1 \
           AND ag.scope_id = $2 AND ag.role_id = $4 \
           AND ag.principal_type = 'user' \
         ORDER BY ag.created_at",
    )
    .bind(crate::endpoints::OBJECT_KIND)
    .bind(id)
    .bind(ctx.workspace_id.0)
    .bind(role)
    .fetch_all(&st.db.pool)
    .await
    .map_err(db_err)?;
    // One PDP question per grant, the same expression the serving path's
    // cascade answers to. An error is a 500 rather than `false`: a wrong `false`
    // would present Revoke as the thing holding the access up.
    let mut out = Vec::with_capacity(rows.len());
    for (gid, sa, name, created_at) in rows {
        let inherits_invoke =
            crate::invoke::inherits_invoke(&st, UserId(sa), ctx.workspace_id).await?;
        out.push(DataApiEndpointGrantDto {
            id: gid,
            service_account_id: sa,
            service_account_name: name,
            created_at,
            inherits_invoke,
        });
    }
    Ok(Json(out))
}

#[utoipa::path(
    post,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/grants",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
    ),
    request_body = GrantInvokeInput,
    responses(
        (status = 200, description = "Granted (idempotent)", body = DataApiEndpointGrantDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage — granting invoke rights mutates the authorization graph, so :read does not reach it", body = ErrorBody),
        (status = 404, description = "No such endpoint, or no such service account, in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn grant_endpoint_invoke(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id)): Path<(String, Uuid)>,
    Json(body): Json<GrantInvokeInput>,
) -> Result<Json<DataApiEndpointGrantDto>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    // BOTH ids against `ctx.workspace_id`. Dropping either predicate is how a
    // cross-workspace grant gets written by a call that looks entirely ordinary.
    let ep = st.endpoints.get(ctx.workspace_id.0, id).await?;
    let sa = scoped_account(&st, ctx.workspace_id.0, body.service_account_id).await?;
    let role = invoke_role_id(&st).await?;

    // `ON CONFLICT DO NOTHING` against the `(principal_type, principal_id,
    // role_id, scope_type, scope_id, effect)` unique key, so re-granting is a
    // no-op rather than a 500 — and the row is re-read afterwards so the
    // response describes the stored grant either way.
    sqlx::query(
        "INSERT INTO access_grants \
           (principal_type, principal_id, role_id, scope_type, object_type, scope_id, effect, granted_by) \
         VALUES ('user', $1, $2, 'object', $3, $4, 'allow', $5) ON CONFLICT DO NOTHING",
    )
    .bind(sa)
    .bind(role)
    .bind(crate::endpoints::OBJECT_KIND)
    .bind(id)
    .bind(ctx.principal.user_id.0)
    .execute(&st.db.pool)
    .await
    .map_err(db_err)?;

    // `object_type` is in the predicate even though nothing can currently write
    // an object grant on this `(principal, role, scope_id)` with a different
    // one. This is the query that decides which row the caller is told it just
    // wrote, and the whole point of this feature is that all six columns of a
    // grant have to agree — a re-read that identifies the row by five of them
    // would be the one place in the pair where that stopped being true.
    let (grant_id, created_at): (Uuid, DateTime<Utc>) = sqlx::query_as(
        "SELECT id, created_at FROM access_grants \
         WHERE principal_type = 'user' AND principal_id = $1 AND role_id = $2 \
           AND scope_type = 'object' AND object_type = $3 AND scope_id = $4 \
           AND effect = 'allow'",
    )
    .bind(sa)
    .bind(role)
    .bind(crate::endpoints::OBJECT_KIND)
    .bind(id)
    .fetch_one(&st.db.pool)
    .await
    .map_err(db_err)?;

    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:grant_invoke",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({ "service_account_id": sa, "path": ep.path, "method": ep.method }),
    ));

    Ok(Json(DataApiEndpointGrantDto {
        id: grant_id,
        service_account_id: sa,
        // The UI re-fetches the list (which resolves names) after a write, so
        // resolving one here would be a query for a field nothing reads.
        service_account_name: None,
        created_at,
        inherits_invoke: crate::invoke::inherits_invoke(&st, UserId(sa), ctx.workspace_id).await?,
    }))
}

#[utoipa::path(
    delete,
    path = "/api/ws/{workspace}/data-api/endpoints/{id}/grants/{service_account_id}",
    tag = "data-api",
    params(
        WorkspacePath,
        ("id" = Uuid, Path, description = "Endpoint id"),
        ("service_account_id" = Uuid, Path, description = "Service account to revoke"),
    ),
    responses(
        (status = 200, description = "Revoked (idempotent). Takes effect on the caller's NEXT request — unlike revoking a client, which cannot recall an already-issued token before its 600s TTL expires", body = Ack),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage", body = ErrorBody),
        (status = 404, description = "No such endpoint, or no such service account, in this workspace", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn revoke_endpoint_invoke(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Path((_ws_ignored, id, sa_id)): Path<(String, Uuid, Uuid)>,
) -> Result<Json<Ack>, DataApiError> {
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    let ep = st.endpoints.get(ctx.workspace_id.0, id).await?;
    let sa = scoped_account(&st, ctx.workspace_id.0, sa_id).await?;
    let role = invoke_role_id(&st).await?;
    // Scoped by the endpoint AND the invoke role, so this route cannot delete
    // some other grant that happens to name the same principal — a share on a
    // dashboard, say, or a future second endpoint role.
    sqlx::query(
        "DELETE FROM access_grants \
         WHERE principal_type = 'user' AND principal_id = $1 AND role_id = $2 \
           AND scope_type = 'object' AND object_type = $3 AND scope_id = $4",
    )
    .bind(sa)
    .bind(role)
    .bind(crate::endpoints::OBJECT_KIND)
    .bind(id)
    .execute(&st.db.pool)
    .await
    .map_err(db_err)?;
    st.audit.emit(egret_audit::mutation_for(
        &ctx.principal,
        Some(ctx.workspace_id.0),
        "data_api_endpoint:revoke_invoke",
        "data_api_endpoint",
        Some(id.to_string()),
        serde_json::json!({ "service_account_id": sa, "path": ep.path, "method": ep.method }),
    ));
    Ok(Json(Ack::ok()))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ActivityQuery {
    pub range: ActivityRange,
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/activity/summary",
    tag = "data-api",
    params(WorkspacePath, ActivityQuery),
    responses(
        (status = 200, description = "Call statistics for the range", body = ActivitySummaryDto),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn activity_summary(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ActivityQuery>,
) -> Result<Json<ActivitySummaryDto>, DataApiError> {
    // Workspace-scoped `:read` for the whole tab. Activity is deliberately a
    // workspace-level view and is NOT sliced per endpoint: a per-row filter
    // would make the totals disagree with their own rows, which is the one thing
    // §4.2 spends a GROUPING SETS query to prevent.
    egret_core::authz::require(&*st.pdp, &ctx, EP_READ).await?;
    Ok(Json(
        st.activity
            .summary(ctx.workspace_id.0, q.range.since(Utc::now()))
            .await
            .map_err(db_err)?,
    ))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
pub struct ActivityCallsQuery {
    pub range: ActivityRange,
    #[serde(default)]
    pub endpoint_id: Option<Uuid>,
    #[serde(default)]
    pub status_code: Option<i32>,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    /// Off by default. A trial is not consumer traffic, so it must be asked for.
    #[serde(default)]
    pub include_trials: bool,
    /// Keyset cursor: return rows strictly older than this.
    #[serde(default)]
    pub before: Option<DateTime<Utc>>,
    // A CLAMP, where an out-of-range `range` is a hard 400 — two different
    // answers to "you asked for more than you may have", and the difference is
    // deliberate. `range` names a WINDOW: honouring "one year" by quietly
    // serving 30 days would report a number the caller reads as the year's, so
    // it has to refuse. `limit` names a PAGE SIZE: the caller pages with
    // `before` either way, and 200 rows in place of 1000 is not a wrong answer
    // to any question, only a shorter page. What would be indefensible is a
    // clamp nobody can see — hence the doc comment below, which utoipa renders
    // as this parameter's description.
    /// How many rows to return. Default 50, maximum 200; a larger value is
    /// clamped to 200 rather than refused.
    #[serde(default = "default_call_limit")]
    pub limit: i64,
}

fn default_call_limit() -> i64 {
    50
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/activity/calls",
    tag = "data-api",
    params(WorkspacePath, ActivityCallsQuery),
    responses(
        (status = 200, description = "Individual calls in the range, most recent first", body = Vec<ActivityCallDto>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:read permission", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn activity_calls(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Query(q): Query<ActivityCallsQuery>,
) -> Result<Json<Vec<ActivityCallDto>>, DataApiError> {
    // Same gate as `activity_summary`: workspace-scoped `:read`. The
    // `request_id` filter does NOT relax this — see `ActivityStore::calls`'s
    // doc comment for why the workspace predicate must stay in the statement
    // even though `access_log_request_id_idx` is global.
    egret_core::authz::require(&*st.pdp, &ctx, EP_READ).await?;
    Ok(Json(
        st.activity
            .calls(
                ctx.workspace_id.0,
                q.range.since(Utc::now()),
                q.endpoint_id,
                q.status_code,
                q.error_code.as_deref(),
                q.request_id.as_deref(),
                q.include_trials,
                q.before,
                q.limit,
            )
            .await
            .map_err(db_err)?,
    ))
}

// ── Generated OpenAPI (P5 §5) ────────────────────────────────────────────────
//
// The document a workspace hands a consumer team: the endpoints ONE service
// account may call, described from its frozen published contract. Assembled
// in `openapi_gen`, not through utoipa — utoipa describes this server's own
// compiled routes, and these paths exist only in a user's data.

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct OpenApiQuery {
    /// Whose invoke rights the document describes.
    pub service_account_id: Uuid,
    /// Narrow to a single endpoint's contract, e.g. for a per-endpoint download.
    #[serde(default)]
    pub endpoint_id: Option<Uuid>,
}

/// Map [`crate::openapi_gen::OpenApiGenError`] onto the crate's canonical
/// [`DataApiError`], the same way every other handler in this module maps its
/// failures: `Db` follows [`db_err`], `Core` passes the already-mapped
/// [`Error`] through, and `NamingCollision` becomes a 409 naming the endpoint
/// and telling the author to re-publish — there is no document to serve for a
/// version `serving` itself answers 503 for.
///
/// `TooManyEndpoints` becomes a 400 rather than a 409 or a 500: the caller has a
/// remedy in their own hands (`?endpoint_id=`, which the message names), and a
/// second unrelated meaning for 409 on this surface is the confusion this branch
/// otherwise had to fix in `try_it`.
fn openapi_gen_err(e: crate::openapi_gen::OpenApiGenError) -> DataApiError {
    use crate::openapi_gen::OpenApiGenError;
    match e {
        OpenApiGenError::NamingCollision { endpoint } => Error::Conflict(format!(
            "endpoint {endpoint} has an output naming collision; re-publish it"
        )),
        e @ OpenApiGenError::TooManyEndpoints { .. } => Error::InvalidInput(e.to_string()),
        OpenApiGenError::Db(e) => db_err(e),
        OpenApiGenError::Core(e) => e,
    }
}

#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/data-api/openapi",
    tag = "data-api",
    params(WorkspacePath, OpenApiQuery),
    responses(
        (status = 200, description = "OpenAPI 3.0.3 document describing what service_account_id may invoke. `servers[0].url` already carries the workspace slug, so joining it with a `paths` key gives the URL a consumer calls. `Cache-Control: no-store`: the document is per service account and reflects grants as they are at this instant", body = serde_json::Value),
        (status = 400, description = "The workspace has more live endpoints than one document may describe; request them one at a time with endpoint_id", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing data_api_endpoint:manage permission. The gate matches `/grants` LIST, not the other read routes: this document is the grant roster, resolved per service account", body = ErrorBody),
        (status = 404, description = "No such service account in this workspace", body = ErrorBody),
        (status = 409, description = "A described endpoint's output naming collides; re-publish it", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn get_openapi(
    State(st): State<DataApiState>,
    WsContext(ctx): WsContext,
    Query(q): Query<OpenApiQuery>,
) -> Result<
    (
        [(header::HeaderName, &'static str); 1],
        Json<serde_json::Value>,
    ),
    DataApiError,
> {
    // **`:manage`, not `:read`, and matching `/grants` LIST on purpose.** This
    // route takes a `service_account_id` and answers with exactly the set of
    // endpoints that account may invoke — the grant roster, read from the other
    // end and iterable over the workspace's accounts. The module comment's first
    // invariant puts `/grants` LIST on `:manage` because "reading them enumerates
    // every consumer of the endpoint"; this route answers a strictly MORE
    // powerful question than that one — full path, parameter and schema detail
    // for every endpoint a principal may invoke, not just a roster of ids — so
    // gating it lower let `:read` enumerate what `:manage` was there to protect.
    // Do not "fix" this back to `EP_READ` to match the other GETs in this
    // module: the other GETs describe endpoints, this one describes who can call
    // them.
    egret_core::authz::require(&*st.pdp, &ctx, EP_MANAGE).await?;
    // `ctx.workspace_id`, resolved through `scoped_account` before anything
    // else, exactly like every other handler in this module that takes an id
    // from the caller (see the module comment's containment invariant). The
    // permission gate above is not a substitute: it says the caller may ask the
    // question, not whose account it may ask it about. Without this, any uuid
    // (another workspace's principal, a random value, a non-service-account
    // user) supplied here would answer 200 with a DIFFERENT set of paths depending on
    // what it can invoke — precisely the content oracle `scoped_account`'s
    // uniform 404 exists to deny. The mismatch answers 404, never 403, so a
    // caller cannot distinguish "no such account" from "not yours".
    let sa = scoped_account(&st, ctx.workspace_id.0, q.service_account_id).await?;
    let doc = crate::openapi_gen::build(&st, ctx.workspace_id, UserId(sa), q.endpoint_id)
        .await
        .map_err(openapi_gen_err)?;
    // **`no-store`, for the same reason `serving::contract` carries it, and this
    // is the stronger case of the two.** That route's argument is that the
    // document varies by service account while the URL does not; here the
    // account is a QUERY PARAMETER, so the URL does vary — but the response is
    // still session-scoped (two admins with different permissions get different
    // documents from the same URL) and still describes, per account, which
    // endpoints exist and what their parameters are called. A shared cache
    // holding that is the grant roster at rest.
    //
    // It is also the staler-than-the-calls-it-describes problem: the document
    // changes with every grant and every publish, neither of which invalidates
    // anything. This route is where an admin checks what a consumer can see
    // right now, and a stored copy answers what it could see earlier.
    //
    // Being behind a session and `:manage` is not a substitute. `Cache-Control`
    // governs intermediaries and the browser's own disk cache, both of which
    // are downstream of authorization and neither of which knows the response
    // is per account.
    Ok(([(header::CACHE_CONTROL, "no-store")], Json(doc)))
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
            "/ws/{workspace}/data-api/endpoints/{id}/trash",
            post(trash_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/restore",
            post(restore_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/bookmark",
            post(bookmark_endpoint).delete(unbookmark_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/validate",
            post(validate_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/preview",
            post(preview_endpoint),
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
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/grants",
            get(list_endpoint_grants).post(grant_endpoint_invoke),
        )
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/grants/{service_account_id}",
            delete(revoke_endpoint_invoke),
        )
        // The handler lives in `try_it` rather than here: it is the one route in
        // this module that builds and drives the SERVING router, and keeping it
        // beside its own reasoning keeps that visible.
        .route(
            "/ws/{workspace}/data-api/endpoints/{id}/try",
            post(crate::try_it::try_endpoint),
        )
        .route(
            "/ws/{workspace}/data-api/activity/summary",
            get(activity_summary),
        )
        .route(
            "/ws/{workspace}/data-api/activity/calls",
            get(activity_calls),
        )
        .route("/ws/{workspace}/data-api/openapi", get(get_openapi))
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
        trash_endpoint,
        restore_endpoint,
        bookmark_endpoint,
        unbookmark_endpoint,
        validate_endpoint,
        preview_endpoint,
        publish_endpoint,
        rollback_endpoint,
        disable_endpoint,
        enable_endpoint,
        list_endpoint_versions,
        list_endpoint_grants,
        grant_endpoint_invoke,
        revoke_endpoint_invoke,
        crate::try_it::try_endpoint,
        activity_summary,
        activity_calls,
        get_openapi,
    ),
    components(schemas(
        DataApiServiceAccountDto,
        DataApiClientDto,
        DataApiNewClientDto,
        CreateServiceAccountInput,
        IssueClientInput,
        DataApiEndpointDto,
        DataApiEndpointListItemDto,
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
        crate::preview::PreviewEndpointInput,
        crate::preview::DataApiPreviewDto,
        DataApiPublishedDto,
        DataApiEndpointVersionDto,
        DataApiEndpointGrantDto,
        GrantInvokeInput,
        egret_dataapi_api::EndpointSnapshot,
        egret_dataapi_api::ColumnSpec,
        crate::contract::ContractDiff,
        crate::contract::ContractChange,
        crate::contract::ChangeKind,
        ActivitySummaryDto,
        ActivityRow,
        ActivityRange,
        ActivityCallDto,
        crate::try_it::TryRequest,
        crate::try_it::TryResponse,
        crate::try_it::TryRequestEcho,
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
