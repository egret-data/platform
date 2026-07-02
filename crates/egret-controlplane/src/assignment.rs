//! Control-plane workspace↔catalog assignment routes
//! (`/workspaces/{ws}/catalog-assignments`), nested under `/api/control-plane`.
//!
//! Assigning a Polaris catalog to a workspace provisions a per-assignment engine
//! service credential: a Polaris-native principal (v1 decision) whose one-time
//! clientId/clientSecret are sealed as a GLOBAL named secret, plus the
//! principal-role → catalog-role → grant chain that lets that principal manage
//! the catalog's content. The assignment row records the workspace, catalog,
//! provisioned principal name, and a reference to the credential secret; it never
//! stores or returns the credential itself.
//!
//! Every handler gates on `require_platform_admin`. Re-provisioning is idempotent:
//! role/assignment creation swallows Polaris `Conflict` via [`ignore_conflict`].

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use egret_auth::principal::Auth;
use egret_polaris::PolarisError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::require_platform_admin;
use crate::error::CpError;
use crate::polaris_conn::polaris_client;
use crate::CpState;

/// One `workspace_catalog_assignments` row.
#[derive(Debug, Clone)]
pub struct Assignment {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub catalog_name: String,
    pub principal_name: String,
    pub credential_ref: Uuid,
    pub created_at: DateTime<Utc>,
}

/// One assignment row, as selected from SQL.
type AssignmentRow = (Uuid, Uuid, String, String, Uuid, DateTime<Utc>);

impl From<AssignmentRow> for Assignment {
    fn from(
        (id, workspace_id, catalog_name, principal_name, credential_ref, created_at): AssignmentRow,
    ) -> Self {
        Assignment {
            id,
            workspace_id,
            catalog_name,
            principal_name,
            credential_ref,
            created_at,
        }
    }
}

/// Postgres-backed store over `workspace_catalog_assignments`.
#[derive(Clone)]
pub struct AssignmentStore {
    db: egret_metastore::Db,
}

const COLS: &str = "id, workspace_id, catalog_name, principal_name, credential_ref, created_at";

impl AssignmentStore {
    pub fn new(db: egret_metastore::Db) -> Self {
        Self { db }
    }

    /// Insert a new assignment. A `UNIQUE(workspace_id, catalog_name)` violation
    /// maps to [`CpError::AlreadyExists`].
    pub async fn insert(
        &self,
        ws: Uuid,
        catalog: &str,
        principal: &str,
        credential_ref: Uuid,
    ) -> Result<Assignment, CpError> {
        let res: Result<AssignmentRow, sqlx::Error> = sqlx::query_as(&format!(
            "INSERT INTO workspace_catalog_assignments \
             (workspace_id, catalog_name, principal_name, credential_ref) \
             VALUES ($1, $2, $3, $4) RETURNING {COLS}"
        ))
        .bind(ws)
        .bind(catalog)
        .bind(principal)
        .bind(credential_ref)
        .fetch_one(&self.db.pool)
        .await;
        match res {
            Ok(row) => Ok(row.into()),
            Err(e) if is_unique_violation(&e) => {
                Err(CpError::AlreadyExists(format!("{ws}/{catalog}")))
            }
            Err(_) => Err(CpError::Internal),
        }
    }

    /// List all assignments for a workspace, newest first.
    pub async fn list_for_workspace(&self, ws: Uuid) -> Result<Vec<Assignment>, CpError> {
        let rows: Vec<AssignmentRow> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM workspace_catalog_assignments \
             WHERE workspace_id = $1 ORDER BY created_at DESC"
        ))
        .bind(ws)
        .fetch_all(&self.db.pool)
        .await
        .map_err(|_| CpError::Internal)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Fetch a single assignment by (workspace, catalog), `None` if absent.
    pub async fn get(&self, ws: Uuid, catalog: &str) -> Result<Option<Assignment>, CpError> {
        let row: Option<AssignmentRow> = sqlx::query_as(&format!(
            "SELECT {COLS} FROM workspace_catalog_assignments \
             WHERE workspace_id = $1 AND catalog_name = $2"
        ))
        .bind(ws)
        .bind(catalog)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(|_| CpError::Internal)?;
        Ok(row.map(Into::into))
    }

    /// Delete an assignment by (workspace, catalog), returning the deleted row
    /// (so the caller can garbage-collect its credential secret + principal).
    pub async fn delete(&self, ws: Uuid, catalog: &str) -> Result<Option<Assignment>, CpError> {
        let row: Option<AssignmentRow> = sqlx::query_as(&format!(
            "DELETE FROM workspace_catalog_assignments \
             WHERE workspace_id = $1 AND catalog_name = $2 RETURNING {COLS}"
        ))
        .bind(ws)
        .bind(catalog)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(|_| CpError::Internal)?;
        Ok(row.map(Into::into))
    }
}

/// Whether a sqlx error is a Postgres unique-constraint violation (SQLSTATE 23505).
fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(db) if db.code().as_deref() == Some("23505"))
}

/// Swallow a Polaris `Conflict` (the securable already exists) so re-provisioning
/// an assignment is idempotent; propagate every other error.
fn ignore_conflict(r: Result<(), PolarisError>) -> Result<(), CpError> {
    match r {
        Ok(()) | Err(PolarisError::Conflict) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Request body for `POST /workspaces/{ws}/catalog-assignments`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignBody {
    pub catalog: String,
}

/// Response DTO for an assignment (camelCase; NEVER carries the credential).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentDto {
    pub catalog_name: String,
    pub principal_name: String,
}

impl From<Assignment> for AssignmentDto {
    fn from(a: Assignment) -> Self {
        AssignmentDto {
            catalog_name: a.catalog_name,
            principal_name: a.principal_name,
        }
    }
}

pub fn router() -> Router<CpState> {
    Router::new()
        .route(
            "/workspaces/{ws}/catalog-assignments",
            get(list).post(assign),
        )
        .route(
            "/workspaces/{ws}/catalog-assignments/{catalog}",
            axum::routing::delete(unassign),
        )
}

async fn assign(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(ws): Path<Uuid>,
    Json(body): Json<AssignBody>,
) -> Result<(StatusCode, Json<AssignmentDto>), CpError> {
    require_platform_admin(&principal)?;
    let catalog = body.catalog;
    let client = polaris_client(&st.polaris_conn, &st.secrets).await?;

    // The catalog must already exist at the control plane.
    client
        .get_catalog(&catalog)
        .await
        .map_err(|_| CpError::BadRequest("unknown catalog".into()))?;

    // Deterministic per-workspace names so re-provisioning is idempotent.
    let pname = format!("egret-ws-{ws}");
    let prole = format!("egret-ws-{ws}-role");
    let crole = format!("egret-ws-{ws}");

    // Provision the principal + role chain, then grant it content management.
    let creds = client.create_principal(&pname).await?;
    ignore_conflict(client.create_principal_role(&prole).await)?;
    ignore_conflict(client.assign_principal_role(&pname, &prole).await)?;
    ignore_conflict(client.create_catalog_role(&catalog, &crole).await)?;
    ignore_conflict(client.assign_catalog_role(&prole, &catalog, &crole).await)?;
    client
        .add_grant(
            &catalog,
            &crole,
            &egret_polaris::Grant::Catalog {
                privilege: "CATALOG_MANAGE_CONTENT".into(),
            },
        )
        .await?;

    // Seal the one-time credential as a GLOBAL named secret.
    let secret_val = serde_json::json!({
        "clientId": creds.credentials.client_id,
        "clientSecret": creds.credentials.client_secret,
    })
    .to_string();
    let cred_ref = st
        .secrets
        .create(
            None,
            &format!("polaris/{catalog}/{ws}"),
            Some("engine service credential"),
            &secret_val,
        )
        .await?;

    let row = st
        .assignments
        .insert(ws, &catalog, &pname, cred_ref)
        .await?;
    Ok((StatusCode::CREATED, Json(row.into())))
}

async fn list(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path(ws): Path<Uuid>,
) -> Result<Json<Vec<AssignmentDto>>, CpError> {
    require_platform_admin(&principal)?;
    let rows = st.assignments.list_for_workspace(ws).await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

async fn unassign(
    State(st): State<CpState>,
    Auth(principal): Auth,
    Path((ws, catalog)): Path<(Uuid, String)>,
) -> Result<StatusCode, CpError> {
    require_platform_admin(&principal)?;
    if let Some(row) = st.assignments.delete(ws, &catalog).await? {
        // Best-effort GC of the credential secret and the Polaris principal.
        st.secrets.delete(None, row.credential_ref).await.ok();
        if let Ok(client) = polaris_client(&st.polaris_conn, &st.secrets).await {
            let _ = client.delete_principal(&row.principal_name).await;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}
