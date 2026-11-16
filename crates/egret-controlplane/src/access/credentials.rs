//! Service-account credentials across both audiences (SA2).
//!
//! A credential is an OAuth2 client of the machine-IdP. Two tables hold them:
//! `dataapi.clients` (audience `data-api`) and `catalog.catalog_clients`
//! (audience `catalog`). [`Credential`] presents both as one row, so the
//! console lists a service account's credentials without knowing which table
//! each lives in, and the lifecycle verbs (rotate, revoke, expiry) dispatch to
//! the right one.
//!
//! Minting reuses the stores' own create paths (`egret_dataapi::ClientStore` and
//! `CatalogStore::create_client`), so secret generation/hashing is unchanged;
//! revoke is `revoked_at = now()` in both audiences, exactly as the Data-API
//! store does it, and both verifiers already refuse revoked/expired rows.
//!
//! Permissions: listing is `user:read`; revoke/expiry are `user:update` (they
//! only reduce access). Anything that mints a live secret — create AND rotate —
//! needs the permission the audience's own issue route takes: `service_account:manage`
//! against the account's workspace (data-api) and `catalog_registry:create`
//! (catalog). Rotate additionally needs `user:update`.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::principal::Auth;
use egret_auth::service_accounts::{SaScope, ServiceAccount};
use egret_core::http::ErrorBody;
use egret_core::UserId;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use super::{internal, service_accounts::require_sa};
use crate::error::CpError;
use crate::CpState;

const CONSUMER_TYPES: [&str; 2] = ["engine", "external"];

pub fn router() -> Router<CpState> {
    Router::new()
        .route(
            "/service-accounts/{id}/credentials",
            get(list_credentials).post(create_credential),
        )
        .route(
            "/service-accounts/{id}/credentials/{cid}/rotate",
            post(rotate_credential),
        )
        .route(
            "/service-accounts/{id}/credentials/{cid}/revoke",
            post(revoke_credential),
        )
        .route(
            "/service-accounts/{id}/credentials/{cid}/expiry",
            post(set_credential_expiry),
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub enum Audience {
    #[serde(rename = "data-api")]
    DataApi,
    #[serde(rename = "catalog")]
    Catalog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CredentialStatus {
    Active,
    Revoked,
    Expired,
}

/// Derived, never stored: `revoked` if `revoked_at` is set, else `expired` once
/// `expires_at` has passed, else `active`.
pub fn derive_status(
    revoked_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> CredentialStatus {
    if revoked_at.is_some() {
        CredentialStatus::Revoked
    } else if expires_at.is_some_and(|e| e <= now) {
        CredentialStatus::Expired
    } else {
        CredentialStatus::Active
    }
}

/// What a catalog credential is bound to.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CatalogBinding {
    pub warehouse_id: Uuid,
    pub catalog_role_id: Uuid,
    /// `engine` or `external`.
    pub consumer_type: String,
}

/// One credential of a service account, whichever table holds it.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Credential {
    pub id: Uuid,
    pub service_account_id: Uuid,
    pub audience: Audience,
    pub name: String,
    /// The OAuth2 `client_id` the client presents.
    pub client_id: String,
    pub status: CredentialStatus,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<Utc>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub last_used_at: Option<DateTime<Utc>>,
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expires_at: Option<DateTime<Utc>>,
    pub catalog: Option<CatalogBinding>,
}

/// A freshly minted credential: the row plus its one-time secret.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CreatedCredential {
    #[serde(flatten)]
    pub credential: Credential,
    /// Shown ONCE; only a hash is stored.
    pub client_secret: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateCredentialBody {
    pub audience: Audience,
    /// Defaults to `<service account name> <audience> credential`.
    #[serde(default)]
    pub name: Option<String>,
    /// Required for `catalog`, forbidden for `data-api`.
    #[serde(default)]
    pub catalog: Option<CatalogBinding>,
    #[serde(default)]
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetExpiryBody {
    /// `null` clears the expiry. A time in the past expires the credential now.
    #[schema(value_type = Option<String>, format = DateTime)]
    pub expires_at: Option<DateTime<Utc>>,
}

type DataApiRow = (
    Uuid,
    Uuid,
    String,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
);
type CatalogRow = (
    Uuid,
    Uuid,
    String,
    String,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Uuid,
    Uuid,
    String,
);

macro_rules! dataapi_select {
    ($w:literal) => {
        concat!(
            "SELECT id, service_account_id, name, client_id, created_at, last_used_at, \
             expires_at, revoked_at FROM dataapi.clients ",
            $w
        )
    };
}
macro_rules! catalog_select {
    ($w:literal) => {
        concat!(
            "SELECT id, service_account_id, name, client_id, created_at, last_used_at, \
             expires_at, revoked_at, warehouse_id, catalog_role_id, consumer_type \
             FROM catalog.catalog_clients ",
            $w
        )
    };
}

fn from_dataapi(r: DataApiRow, now: DateTime<Utc>) -> Credential {
    let (id, service_account_id, name, client_id, created_at, last_used_at, expires_at, revoked_at) =
        r;
    Credential {
        id,
        service_account_id,
        audience: Audience::DataApi,
        name,
        client_id,
        status: derive_status(revoked_at, expires_at, now),
        created_at,
        last_used_at,
        expires_at,
        catalog: None,
    }
}

fn from_catalog(r: CatalogRow, now: DateTime<Utc>) -> Credential {
    let (
        id,
        service_account_id,
        name,
        client_id,
        created_at,
        last_used_at,
        expires_at,
        revoked_at,
        warehouse_id,
        catalog_role_id,
        consumer_type,
    ) = r;
    Credential {
        id,
        service_account_id,
        audience: Audience::Catalog,
        name,
        client_id,
        status: derive_status(revoked_at, expires_at, now),
        created_at,
        last_used_at,
        expires_at,
        catalog: Some(CatalogBinding {
            warehouse_id,
            catalog_role_id,
            consumer_type,
        }),
    }
}

/// The credential repo: both audiences behind one interface.
pub struct CredentialRepo<'a> {
    pool: &'a PgPool,
}

impl<'a> CredentialRepo<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Every credential of `sa`, newest first, across both audiences.
    pub async fn list(&self, sa: Uuid) -> Result<Vec<Credential>, CpError> {
        let now = Utc::now();
        let d: Vec<DataApiRow> = sqlx::query_as(dataapi_select!("WHERE service_account_id = $1"))
            .bind(sa)
            .fetch_all(self.pool)
            .await
            .map_err(internal)?;
        let c: Vec<CatalogRow> = sqlx::query_as(catalog_select!("WHERE service_account_id = $1"))
            .bind(sa)
            .fetch_all(self.pool)
            .await
            .map_err(internal)?;
        let mut out: Vec<Credential> = d
            .into_iter()
            .map(|r| from_dataapi(r, now))
            .chain(c.into_iter().map(|r| from_catalog(r, now)))
            .collect();
        out.sort_by_key(|c| std::cmp::Reverse(c.created_at));
        Ok(out)
    }

    /// One credential of `sa` (404 semantics: a credential of another account is
    /// not found, so the path's account cannot be used to reach it).
    pub async fn get(&self, sa: Uuid, cid: Uuid) -> Result<Option<Credential>, CpError> {
        let now = Utc::now();
        let d: Option<DataApiRow> =
            sqlx::query_as(dataapi_select!("WHERE id = $1 AND service_account_id = $2"))
                .bind(cid)
                .bind(sa)
                .fetch_optional(self.pool)
                .await
                .map_err(internal)?;
        if let Some(r) = d {
            return Ok(Some(from_dataapi(r, now)));
        }
        let c: Option<CatalogRow> =
            sqlx::query_as(catalog_select!("WHERE id = $1 AND service_account_id = $2"))
                .bind(cid)
                .bind(sa)
                .fetch_optional(self.pool)
                .await
                .map_err(internal)?;
        Ok(c.map(|r| from_catalog(r, now)))
    }

    /// `revoked_at = now()` (idempotent: an already-revoked row keeps its time).
    pub async fn revoke(&self, cred: &Credential) -> Result<(), CpError> {
        let sql = match cred.audience {
            Audience::DataApi => {
                "UPDATE dataapi.clients SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL"
            }
            Audience::Catalog => {
                "UPDATE catalog.catalog_clients SET revoked_at = now() \
                 WHERE id = $1 AND revoked_at IS NULL"
            }
        };
        sqlx::query(sql)
            .bind(cred.id)
            .execute(self.pool)
            .await
            .map_err(internal)?;
        Ok(())
    }

    pub async fn set_expiry(
        &self,
        cred: &Credential,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<(), CpError> {
        let sql = match cred.audience {
            Audience::DataApi => "UPDATE dataapi.clients SET expires_at = $2 WHERE id = $1",
            Audience::Catalog => "UPDATE catalog.catalog_clients SET expires_at = $2 WHERE id = $1",
        };
        sqlx::query(sql)
            .bind(cred.id)
            .bind(expires_at)
            .execute(self.pool)
            .await
            .map_err(internal)?;
        Ok(())
    }
}

/// Mint a credential for `sa` and return it with its one-time secret.
async fn mint(
    st: &CpState,
    sa: &ServiceAccount,
    audience: Audience,
    name: &str,
    catalog: Option<&CatalogBinding>,
    expires_at: Option<DateTime<Utc>>,
    by: UserId,
) -> Result<CreatedCredential, CpError> {
    let repo = CredentialRepo::new(st.roles.pool());
    let (id, secret) = match audience {
        Audience::DataApi => {
            if catalog.is_some() {
                return Err(CpError::BadRequest(
                    "`catalog` is only valid for the catalog audience".into(),
                ));
            }
            // The Data API serves workspace service accounts only.
            if sa.scope != SaScope::Workspace {
                return Err(CpError::BadRequest(
                    "a platform service account cannot hold a data-api credential".into(),
                ));
            }
            let c = egret_dataapi::ClientStore::new(st.db.clone())
                .create(sa.user_id, name, expires_at, by)
                .await?;
            (c.id, c.client_secret)
        }
        Audience::Catalog => {
            let b = catalog.ok_or_else(|| {
                CpError::BadRequest("`catalog` is required for the catalog audience".into())
            })?;
            if !CONSUMER_TYPES.contains(&b.consumer_type.as_str()) {
                return Err(CpError::BadRequest(format!(
                    "consumer_type must be one of {}; got {:?}",
                    CONSUMER_TYPES.join(", "),
                    b.consumer_type
                )));
            }
            let wh = st
                .storage
                .get_warehouse(b.warehouse_id)
                .await?
                .ok_or(CpError::NotFound)?;
            // Workspace-consistency guard: a workspace service account owns
            // only its own workspace's credentials; a platform one may mint in
            // any workspace.
            if sa.scope == SaScope::Workspace && sa.workspace_id != Some(wh.workspace_id) {
                return Err(CpError::BadRequest(
                    "the warehouse belongs to a different workspace than this service account"
                        .into(),
                ));
            }
            let role_ok = st
                .storage
                .list_roles(wh.id)
                .await?
                .iter()
                .any(|r| r.id == b.catalog_role_id);
            if !role_ok {
                return Err(CpError::BadRequest(
                    "the catalog role does not belong to that warehouse".into(),
                ));
            }
            // The path service account is the owner; none is synthesized.
            let c = st
                .storage
                .create_client(&egret_catalog_engine::NewClient {
                    name: name.to_string(),
                    consumer_type: b.consumer_type.clone(),
                    workspace_id: wh.workspace_id,
                    warehouse_id: wh.id,
                    catalog_role_id: b.catalog_role_id,
                    service_account_id: sa.user_id,
                })
                .await?;
            if expires_at.is_some() {
                sqlx::query("UPDATE catalog.catalog_clients SET expires_at = $2 WHERE id = $1")
                    .bind(c.id)
                    .bind(expires_at)
                    .execute(st.roles.pool())
                    .await
                    .map_err(internal)?;
            }
            (c.id, c.client_secret)
        }
    };
    let credential = repo.get(sa.user_id, id).await?.ok_or(CpError::NotFound)?;
    Ok(CreatedCredential {
        credential,
        client_secret: secret,
    })
}

fn default_name(sa: &ServiceAccount, audience: Audience) -> String {
    let a = match audience {
        Audience::DataApi => "data-api",
        Audience::Catalog => "catalog",
    };
    format!("{} {a} credential", sa.name)
}

/// The permission that lets a caller mint a live secret for `audience`, checked
/// where the audience's own issue route checks it: `service_account:manage`
/// against the SA's WORKSPACE for data-api (as `issue_client` does through its
/// `WsContext`; a platform SA has no workspace and is refused by `mint` anyway,
/// so the bare principal is checked), `catalog_registry:create` against the
/// platform for catalog (as the warehouse-admin `create_client` does).
async fn require_mint(
    st: &CpState,
    p: &egret_core::Principal,
    sa: &ServiceAccount,
    audience: Audience,
) -> Result<(), CpError> {
    match (audience, sa.workspace_id) {
        (Audience::DataApi, Some(ws)) => {
            let ctx = egret_core::WorkspaceContext {
                workspace_id: egret_core::ids::WorkspaceId(ws),
                principal: p.clone(),
                workspace_role: String::new(),
            };
            egret_core::authz::require(&*st.pdp, &ctx, "service_account:manage").await?;
        }
        (Audience::DataApi, None) => {
            egret_core::authz::require(&*st.pdp, p, "service_account:manage").await?;
        }
        (Audience::Catalog, _) => {
            egret_core::authz::require(&*st.pdp, p, "catalog_registry:create").await?;
        }
    }
    Ok(())
}

async fn require_credential(st: &CpState, sa: Uuid, cid: Uuid) -> Result<Credential, CpError> {
    CredentialRepo::new(st.roles.pool())
        .get(sa, cid)
        .await?
        .ok_or(CpError::NotFound)
}

#[utoipa::path(
    get,
    path = "/api/control-plane/service-accounts/{id}/credentials",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Service account user id")),
    responses(
        (status = 200, description = "The account's credentials across both audiences, newest first", body = Vec<Credential>),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:read permission", body = ErrorBody),
        (status = 404, description = "No such service account", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn list_credentials(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Credential>>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:read").await?;
    require_sa(&st, id).await?;
    Ok(Json(CredentialRepo::new(st.roles.pool()).list(id).await?))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/service-accounts/{id}/credentials",
    tag = "control-plane-access",
    params(("id" = Uuid, Path, description = "Service account user id")),
    request_body = CreateCredentialBody,
    responses(
        (status = 201, description = "Credential minted; client_secret is shown ONCE", body = CreatedCredential),
        (status = 400, description = "Bad audience/binding, or the warehouse is in a different workspace than a workspace service account", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing service_account:manage in the account's workspace (data-api) or catalog_registry:create (catalog)", body = ErrorBody),
        (status = 404, description = "No such service account or warehouse", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn create_credential(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateCredentialBody>,
) -> Result<(StatusCode, Json<CreatedCredential>), CpError> {
    // The data-api permission is workspace-scoped, so the account is read before
    // the check (a missing account answers 404 to any caller).
    let sa = require_sa(&st, id).await?;
    require_mint(&st, &p, &sa, body.audience).await?;
    let name = body
        .name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| default_name(&sa, body.audience));
    let created = mint(
        &st,
        &sa,
        body.audience,
        &name,
        body.catalog.as_ref(),
        body.expires_at,
        p.user_id,
    )
    .await?;
    audit(&st, &p, &sa, "credential:create", &created.credential);
    Ok((StatusCode::CREATED, Json(created)))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/service-accounts/{id}/credentials/{cid}/rotate",
    tag = "control-plane-access",
    params(
        ("id" = Uuid, Path, description = "Service account user id"),
        ("cid" = Uuid, Path, description = "Credential id"),
    ),
    responses(
        (status = 200, description = "A replacement minted (same audience, name and binding) and the old one revoked; client_secret is shown ONCE", body = CreatedCredential),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update, or the audience mint permission (service_account:manage in the account workspace / catalog_registry:create)", body = ErrorBody),
        (status = 404, description = "No such service account or credential", body = ErrorBody),
        (status = 409, description = "The credential is already revoked", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn rotate_credential(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<CreatedCredential>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    let sa = require_sa(&st, id).await?;
    let old = require_credential(&st, id, cid).await?;
    // Rotation mints a live secret: it needs the audience's mint permission, not
    // just `user:update`, and the check precedes any mint.
    require_mint(&st, &p, &sa, old.audience).await?;
    if old.status == CredentialStatus::Revoked {
        return Err(CpError::Conflict(
            "the credential is already revoked".into(),
        ));
    }
    // Mint first, revoke second (as the Data-API store's rotation does): a
    // failure in between leaves two live credentials, never none.
    let keep_expiry = old.expires_at.filter(|e| *e > Utc::now());
    let created = mint(
        &st,
        &sa,
        old.audience,
        &old.name,
        old.catalog.as_ref(),
        keep_expiry,
        p.user_id,
    )
    .await?;
    CredentialRepo::new(st.roles.pool()).revoke(&old).await?;
    audit(&st, &p, &sa, "credential:rotate", &created.credential);
    Ok(Json(created))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/service-accounts/{id}/credentials/{cid}/revoke",
    tag = "control-plane-access",
    params(
        ("id" = Uuid, Path, description = "Service account user id"),
        ("cid" = Uuid, Path, description = "Credential id"),
    ),
    responses(
        (status = 200, description = "Revoked: the verifier refuses it from now on", body = Credential),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update permission", body = ErrorBody),
        (status = 404, description = "No such service account or credential", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn revoke_credential(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<Json<Credential>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    let sa = require_sa(&st, id).await?;
    let cred = require_credential(&st, id, cid).await?;
    CredentialRepo::new(st.roles.pool()).revoke(&cred).await?;
    let after = require_credential(&st, id, cid).await?;
    audit(&st, &p, &sa, "credential:revoke", &after);
    Ok(Json(after))
}

#[utoipa::path(
    post,
    path = "/api/control-plane/service-accounts/{id}/credentials/{cid}/expiry",
    tag = "control-plane-access",
    params(
        ("id" = Uuid, Path, description = "Service account user id"),
        ("cid" = Uuid, Path, description = "Credential id"),
    ),
    request_body = SetExpiryBody,
    responses(
        (status = 200, description = "Expiry set (null clears it)", body = Credential),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing user:update permission", body = ErrorBody),
        (status = 404, description = "No such service account or credential", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
pub(super) async fn set_credential_expiry(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Json(body): Json<SetExpiryBody>,
) -> Result<Json<Credential>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "user:update").await?;
    let sa = require_sa(&st, id).await?;
    let cred = require_credential(&st, id, cid).await?;
    CredentialRepo::new(st.roles.pool())
        .set_expiry(&cred, body.expires_at)
        .await?;
    let after = require_credential(&st, id, cid).await?;
    audit(&st, &p, &sa, "credential:set_expiry", &after);
    Ok(Json(after))
}

/// The secret is never in `detail`: the audit log outlives every rotation.
fn audit(
    st: &CpState,
    p: &egret_core::Principal,
    sa: &ServiceAccount,
    action: &str,
    c: &Credential,
) {
    st.audit.emit(egret_audit::mutation_for(
        p,
        sa.workspace_id,
        action,
        "credential",
        Some(c.id.to_string()),
        serde_json::json!({
            "service_account_id": sa.user_id,
            "audience": c.audience,
            "client_id": c.client_id,
            "expires_at": c.expires_at.map(|t| t.to_rfc3339()),
        }),
    ));
}
