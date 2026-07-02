//! Control-plane RBAC routes (`/polaris/...`), proxying the external Apache
//! Polaris management API through [`egret_polaris::PolarisClient`].
//!
//! egret drives Polaris RBAC (principals, principal-roles, catalog-roles,
//! role assignments, and grants) from the control plane. Every handler gates
//! on `require_platform_admin` and builds a fresh `PolarisClient` from the
//! singleton connection.
//!
//! Note: `egret-polaris` exposes no list endpoints for principals/roles, so the
//! `GET /polaris/principals` list route is intentionally NOT implemented here;
//! only create/delete/assign/grant operations are proxied.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{post, put};
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_polaris::{Grant, PrincipalWithCredentials};
use serde::{Deserialize, Serialize};

use crate::auth::require_platform_admin;
use crate::error::CpError;
use crate::polaris_conn::polaris_client;
use crate::CpState;

/// Request body carrying a single `name` (principals, principal-roles,
/// catalog-roles).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NameBody {
    pub name: String,
}

/// Request body for assigning a principal-role to a principal.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignPrincipalRoleBody {
    pub role: String,
}

/// Request body for assigning a catalog-role to a principal-role.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignCatalogRoleBody {
    pub catalog_role: String,
}

/// Request body describing a single grant on a securable.
///
/// `kind` selects the securable (`catalog`/`namespace`/`table`/`view`); the
/// `namespace`/`table`/`view` fields are required only for the matching kinds.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantBody {
    pub kind: String,
    pub privilege: String,
    pub namespace: Option<Vec<String>>,
    pub table: Option<String>,
    pub view: Option<String>,
}

/// Response DTO for a principal (camelCase for the frontend).
///
/// Currently unused by any handler: `egret-polaris` exposes no principal-list
/// method, so the `GET /polaris/principals` route is not implemented. Kept for
/// the eventual list endpoint once the client gains a `list_principals` method.
#[allow(dead_code)]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalDto {
    pub name: String,
}

/// Response DTO for a freshly created principal, including its one-time
/// client credentials. This is the ONLY time `clientSecret` is exposed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrincipalWithCredentialsDto {
    pub name: String,
    pub client_id: String,
    pub client_secret: String,
}

impl From<PrincipalWithCredentials> for PrincipalWithCredentialsDto {
    fn from(p: PrincipalWithCredentials) -> Self {
        PrincipalWithCredentialsDto {
            name: p.principal.name,
            client_id: p.credentials.client_id,
            client_secret: p.credentials.client_secret,
        }
    }
}

/// Map a [`GrantBody`] onto a typed [`egret_polaris::Grant`].
///
/// Pure (no I/O) so the securable/required-field mapping can be asserted
/// directly in unit tests. A missing required field for the selected `kind`
/// yields [`CpError::BadRequest`].
fn grant_from_body(b: &GrantBody) -> Result<Grant, CpError> {
    let privilege = b.privilege.clone();
    match b.kind.as_str() {
        "catalog" => Ok(Grant::Catalog { privilege }),
        "namespace" => {
            let namespace = b
                .namespace
                .clone()
                .ok_or_else(|| CpError::BadRequest("namespace is required".into()))?;
            Ok(Grant::Namespace {
                namespace,
                privilege,
            })
        }
        "table" => {
            let namespace = b
                .namespace
                .clone()
                .ok_or_else(|| CpError::BadRequest("namespace is required".into()))?;
            let table = b
                .table
                .clone()
                .ok_or_else(|| CpError::BadRequest("table is required".into()))?;
            Ok(Grant::Table {
                namespace,
                table,
                privilege,
            })
        }
        "view" => {
            let namespace = b
                .namespace
                .clone()
                .ok_or_else(|| CpError::BadRequest("namespace is required".into()))?;
            let view = b
                .view
                .clone()
                .ok_or_else(|| CpError::BadRequest("view is required".into()))?;
            Ok(Grant::View {
                namespace,
                view,
                privilege,
            })
        }
        other => Err(CpError::BadRequest(format!("unknown grant kind: {other}"))),
    }
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route("/polaris/principals", post(create_principal))
        .route(
            "/polaris/principals/{name}",
            axum::routing::delete(remove_principal),
        )
        .route("/polaris/principal-roles", post(create_principal_role))
        .route(
            "/polaris/catalogs/{catalog}/catalog-roles",
            post(create_catalog_role),
        )
        .route(
            "/polaris/principals/{principal}/principal-roles",
            put(assign_principal_role),
        )
        .route(
            "/polaris/principal-roles/{principal_role}/catalog-roles/{catalog}",
            put(assign_catalog_role),
        )
        .route(
            "/polaris/catalogs/{catalog}/catalog-roles/{catalog_role}/grants",
            put(add_grant),
        )
        .route(
            "/polaris/catalogs/{catalog}/catalog-roles/{catalog_role}/grants:revoke",
            post(revoke_grant),
        )
}

async fn create_principal(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Json(body): Json<NameBody>,
) -> Result<(StatusCode, Json<PrincipalWithCredentialsDto>), CpError> {
    require_platform_admin(&principal)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    let created = client.create_principal(&body.name).await?;
    Ok((StatusCode::CREATED, Json(created.into())))
}

async fn remove_principal(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(name): Path<String>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&principal)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    client.delete_principal(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_principal_role(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Json(body): Json<NameBody>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&principal)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    client.create_principal_role(&body.name).await?;
    Ok(StatusCode::CREATED)
}

async fn create_catalog_role(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(catalog): Path<String>,
    Json(body): Json<NameBody>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&principal)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    client.create_catalog_role(&catalog, &body.name).await?;
    Ok(StatusCode::CREATED)
}

async fn assign_principal_role(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(principal_name): Path<String>,
    Json(body): Json<AssignPrincipalRoleBody>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&principal)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    client
        .assign_principal_role(&principal_name, &body.role)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn assign_catalog_role(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path((principal_role, catalog)): Path<(String, String)>,
    Json(body): Json<AssignCatalogRoleBody>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&principal)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    client
        .assign_catalog_role(&principal_role, &catalog, &body.catalog_role)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn add_grant(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path((catalog, catalog_role)): Path<(String, String)>,
    Json(body): Json<GrantBody>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&principal)?;
    let grant = grant_from_body(&body)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    client.add_grant(&catalog, &catalog_role, &grant).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn revoke_grant(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path((catalog, catalog_role)): Path<(String, String)>,
    Json(body): Json<GrantBody>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&principal)?;
    let grant = grant_from_body(&body)?;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;
    client.revoke_grant(&catalog, &catalog_role, &grant).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(kind: &str) -> GrantBody {
        GrantBody {
            kind: kind.into(),
            privilege: "TABLE_READ_DATA".into(),
            namespace: None,
            table: None,
            view: None,
        }
    }

    #[test]
    fn grant_from_body_catalog() {
        let b = body("catalog");
        match grant_from_body(&b).expect("catalog grant") {
            Grant::Catalog { privilege } => assert_eq!(privilege, "TABLE_READ_DATA"),
            other => panic!("expected Grant::Catalog, got {other:?}"),
        }
    }

    #[test]
    fn grant_from_body_table() {
        let b = GrantBody {
            table: Some("orders".into()),
            namespace: Some(vec!["analytics".into(), "sales".into()]),
            ..body("table")
        };
        match grant_from_body(&b).expect("table grant") {
            Grant::Table {
                namespace,
                table,
                privilege,
            } => {
                assert_eq!(namespace, vec!["analytics", "sales"]);
                assert_eq!(table, "orders");
                assert_eq!(privilege, "TABLE_READ_DATA");
            }
            other => panic!("expected Grant::Table, got {other:?}"),
        }
    }

    #[test]
    fn grant_from_body_table_missing_table_is_bad_request() {
        let b = GrantBody {
            namespace: Some(vec!["analytics".into()]),
            ..body("table")
        };
        assert!(matches!(grant_from_body(&b), Err(CpError::BadRequest(_))));
    }
}
