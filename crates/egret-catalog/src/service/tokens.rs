//! Per-workspace catalog token store.
//!
//! Only the SHA-256 hex hash is persisted; the raw token is returned once at
//! creation and never stored.

use crate::CatalogError;
use chrono::{DateTime, Utc};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::store::CatalogStore;

// ---------------------------------------------------------------------------
// Resource-grant model (token scope v2).
// ---------------------------------------------------------------------------

/// A single resource grant: a namespace (or the `"*"` all-namespaces sentinel),
/// an optional exact table within it, and the permissions granted on that
/// resource. Token scope is a list of grants, evaluated with OR semantics: an
/// action is allowed if ANY grant matches the target resource AND carries the
/// needed permission. An empty list grants nothing (least-privilege).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Grant {
    /// Dotted namespace prefix (subtree match), or `"*"` for all namespaces.
    pub namespace: String,
    /// `None` = all tables in the namespace subtree; else an exact table name.
    #[serde(default)]
    pub table: Option<String>,
    /// Full perm strings: a non-empty subset of `{"catalog:read","catalog:write"}`.
    pub permissions: Vec<String>,
}

// ---------------------------------------------------------------------------
// Public row type — token_hash is intentionally absent.
// ---------------------------------------------------------------------------

/// A catalog token row as returned from the store (hash is never exposed).
#[derive(Debug)]
pub struct CatalogToken {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub warehouse_id: Uuid,
    pub label: String,
    /// The token's resource-grant scope (OR-evaluated; empty = no access).
    pub scope: Vec<Grant>,
    pub created_at: DateTime<Utc>,
}

/// The result of a successful token authentication.
#[derive(Debug)]
pub struct TokenAuth {
    pub workspace_id: Uuid,
    pub warehouse_id: Uuid,
    /// The token's resource grants (the per-resource authz decision lives in the
    /// authorizer, which reads these from the claim).
    pub grants: Vec<Grant>,
    /// The deduped UNION of every grant's permissions — used by the front-auth
    /// coarse gate (does the token hold ANY read / ANY write).
    pub permissions: Vec<String>,
}

// ---------------------------------------------------------------------------
// Hashing helper (pub so tests can assert the stored value is the hash).
// ---------------------------------------------------------------------------

/// Hex-encoded SHA-256 of `raw`.
pub fn hash_token(raw: &str) -> String {
    hex::encode(Sha256::digest(raw.as_bytes()))
}

/// Generate a fresh CSPRNG raw token with a recognisable `egcat_` prefix.
fn generate_raw_token() -> String {
    let mut bytes = [0u8; 32]; // 256 bits
    OsRng.fill_bytes(&mut bytes);
    format!("egcat_{}", hex::encode(bytes))
}

/// The set of permission values that callers may request.
const VALID_PERMISSIONS: &[&str] = &["catalog:read", "catalog:write"];

fn validate_permissions(permissions: &[String]) -> Result<(), CatalogError> {
    if permissions.is_empty() {
        return Err(CatalogError::BadRequest(
            "permissions must not be empty".into(),
        ));
    }
    for perm in permissions {
        if !VALID_PERMISSIONS.contains(&perm.as_str()) {
            return Err(CatalogError::BadRequest(format!(
                "unknown permission '{perm}'; valid values are: {}",
                VALID_PERMISSIONS.join(", ")
            )));
        }
    }
    Ok(())
}

/// Validate a single namespace segment string.
///
/// Rules: must be non-empty; when split by `.`, every segment must be
/// non-empty (i.e. no leading/trailing dot and no `..`).
fn validate_namespace(ns: &str) -> bool {
    !ns.is_empty() && ns.split('.').all(|s| !s.is_empty())
}

/// The all-namespaces sentinel for a grant's `namespace`.
///
/// `"*"` is RESERVED as this sentinel: a grant with `namespace == "*"` matches
/// every namespace. A literal Iceberg namespace named `*` would therefore
/// collide with the sentinel and is not addressable by a concrete grant; this is
/// acceptable because `*` is not a valid Iceberg namespace level in practice.
const ALL_NS: &str = "*";

/// Validate a token's resource-grant scope.
///
/// Rules:
/// * the list must be non-empty (least-privilege: an empty scope grants nothing,
///   so an empty scope is almost certainly a mistake and is rejected up front);
/// * each grant's `namespace` is either the `"*"` sentinel or a valid dotted
///   namespace (no leading/trailing/consecutive dots);
/// * a present `table` must be a single non-empty segment (no dots) and requires
///   a concrete namespace (a `"*"` table grant is meaningless);
/// * each grant's `permissions` is a non-empty subset of the valid perms.
fn validate_grants(grants: &[Grant]) -> Result<(), CatalogError> {
    if grants.is_empty() {
        return Err(CatalogError::BadRequest(
            "scope must have at least one grant".into(),
        ));
    }
    for g in grants {
        if g.namespace != ALL_NS && !validate_namespace(&g.namespace) {
            return Err(CatalogError::BadRequest(format!(
                "invalid grant namespace '{}'",
                g.namespace
            )));
        }
        if let Some(t) = &g.table {
            if t.is_empty() || t.contains('.') {
                return Err(CatalogError::BadRequest(format!(
                    "invalid grant table '{t}'"
                )));
            }
            if g.namespace == ALL_NS {
                return Err(CatalogError::BadRequest(
                    "a table grant needs a concrete namespace".into(),
                ));
            }
        }
        validate_permissions(&g.permissions)?;
    }
    Ok(())
}

/// Deduped union of every grant's permissions (stable order, deduped).
fn union_permissions(grants: &[Grant]) -> Vec<String> {
    let mut perms: Vec<String> = grants
        .iter()
        .flat_map(|g| g.permissions.iter().cloned())
        .collect();
    perms.sort();
    perms.dedup();
    perms
}

// ---------------------------------------------------------------------------
// CatalogStore — token methods
// ---------------------------------------------------------------------------

/// Internal row type for sqlx, mapping the DB columns we SELECT. `scope` is the
/// jsonb grants column, decoded into a typed `Vec<Grant>`.
#[derive(sqlx::FromRow)]
struct TokenRow {
    id: Uuid,
    workspace_id: Uuid,
    warehouse_id: Uuid,
    label: String,
    scope: sqlx::types::Json<Vec<Grant>>,
    created_at: DateTime<Utc>,
}

impl From<TokenRow> for CatalogToken {
    fn from(r: TokenRow) -> Self {
        CatalogToken {
            id: r.id,
            workspace_id: r.workspace_id,
            warehouse_id: r.warehouse_id,
            label: r.label,
            scope: r.scope.0,
            created_at: r.created_at,
        }
    }
}

/// Internal row type for authenticate_token — only selects what it needs.
#[derive(sqlx::FromRow)]
struct AuthRow {
    workspace_id: Uuid,
    warehouse_id: Uuid,
    scope: sqlx::types::Json<Vec<Grant>>,
}

impl CatalogStore {
    /// Create a new catalog token for `workspace_id` with the given resource
    /// `scope` (token scope v2).
    ///
    /// `scope` is a non-empty list of [`Grant`]s, OR-evaluated by the authorizer.
    /// Each grant's namespace is either the `"*"` sentinel or a valid dotted
    /// namespace; a present `table` must be a single non-empty segment in a
    /// concrete namespace; each grant's permissions are a non-empty subset of
    /// `{"catalog:read","catalog:write"}`. Any violation → `CatalogError::BadRequest`.
    ///
    /// Returns the persisted `CatalogToken` row plus the **raw token** (shown
    /// once; the caller must relay it to the user immediately).
    pub async fn create_token(
        &self,
        workspace_id: Uuid,
        warehouse_id: Uuid,
        label: &str,
        scope: &[Grant],
    ) -> Result<(CatalogToken, String), CatalogError> {
        validate_grants(scope)?;

        let raw = generate_raw_token();
        let hash = hash_token(&raw);

        let row: TokenRow = sqlx::query_as::<_, TokenRow>(
            "INSERT INTO catalog_tokens \
             (workspace_id, warehouse_id, token_hash, label, scope) \
             VALUES ($1, $2, $3, $4, $5) \
             RETURNING id, workspace_id, warehouse_id, label, scope, created_at",
        )
        .bind(workspace_id)
        .bind(warehouse_id)
        .bind(&hash)
        .bind(label)
        .bind(sqlx::types::Json(scope))
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                CatalogError::AlreadyExists("token".into())
            }
            _ => CatalogError::Db(e),
        })?;

        Ok((row.into(), raw))
    }

    /// Authenticate `raw` and return the workspace + warehouse + resource `grants`
    /// (plus the deduped union `permissions`), only if not revoked.
    ///
    /// Maps a miss to `CatalogError::NotFound` so the caller can return 401.
    pub async fn authenticate_token(&self, raw: &str) -> Result<TokenAuth, CatalogError> {
        let hash = hash_token(raw);
        let row: AuthRow = sqlx::query_as::<_, AuthRow>(
            "SELECT workspace_id, warehouse_id, scope FROM catalog_tokens \
             WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(&hash)
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?
        .ok_or_else(|| CatalogError::NotFound("catalog token".into()))?;

        let grants = row.scope.0;
        let permissions = union_permissions(&grants);
        Ok(TokenAuth {
            workspace_id: row.workspace_id,
            warehouse_id: row.warehouse_id,
            grants,
            permissions,
        })
    }

    /// Look up the workspace that owns `raw` (only if not revoked).
    ///
    /// Maps a miss to `CatalogError::NotFound` so the caller can return 401.
    ///
    /// **Deprecated**: prefer [`CatalogStore::authenticate_token`] which also
    /// returns the permission scope. This method is kept for Task-3 migration.
    pub async fn workspace_for_token(&self, raw: &str) -> Result<Uuid, CatalogError> {
        self.authenticate_token(raw)
            .await
            .map(|auth| auth.workspace_id)
    }

    /// Revoke a token by id (sets `revoked_at = now()`).
    ///
    /// **Admin-CLI only — intentionally has NO workspace guard.** It revokes any
    /// token by id regardless of owning workspace. HTTP / multi-tenant callers
    /// MUST use [`CatalogStore::revoke_token_for_workspace`] instead, which
    /// scopes the revoke to the caller's workspace and so closes the cross-tenant
    /// IDOR. Do not wire this method to a network endpoint.
    pub async fn revoke_token(&self, id: Uuid) -> Result<(), CatalogError> {
        sqlx::query("UPDATE catalog_tokens SET revoked_at = now() WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await
            .map_err(CatalogError::Db)?;
        Ok(())
    }

    /// Revoke a token by id, but only if it belongs to `workspace_id` and is not
    /// already revoked.
    ///
    /// Returns `true` when exactly one row was revoked, `false` otherwise (the
    /// token does not exist, belongs to another workspace, or was already
    /// revoked). The `workspace_id` guard closes the cross-tenant IDOR so a
    /// workspace admin can only revoke their own workspace's tokens.
    pub async fn revoke_token_for_workspace(
        &self,
        id: Uuid,
        workspace_id: Uuid,
    ) -> Result<bool, CatalogError> {
        let r = sqlx::query(
            "UPDATE catalog_tokens SET revoked_at = now() \
             WHERE id = $1 AND workspace_id = $2 AND revoked_at IS NULL",
        )
        .bind(id)
        .bind(workspace_id)
        .execute(self.pool())
        .await
        .map_err(CatalogError::from)?;
        Ok(r.rows_affected() == 1)
    }

    /// Revoke a token by id, scoped to BOTH its `workspace_id` and `warehouse_id`
    /// (and only if not already revoked).
    ///
    /// Returns `true` when exactly one row was revoked, `false` otherwise (the
    /// token does not exist, belongs to another workspace, belongs to another
    /// warehouse within this workspace, or was already revoked). The
    /// `warehouse_id` guard is defense-in-depth: a token minted for warehouse A
    /// can never be revoked via warehouse B's path even within the same workspace.
    pub async fn revoke_token_for_warehouse(
        &self,
        workspace_id: Uuid,
        warehouse_id: Uuid,
        id: Uuid,
    ) -> Result<bool, CatalogError> {
        let r = sqlx::query(
            "UPDATE catalog_tokens SET revoked_at = now() \
             WHERE id = $1 AND workspace_id = $2 AND warehouse_id = $3 AND revoked_at IS NULL",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(warehouse_id)
        .execute(self.pool())
        .await
        .map_err(CatalogError::from)?;
        Ok(r.rows_affected() == 1)
    }

    /// List all non-revoked tokens for a workspace (hashes excluded).
    pub async fn list_tokens(&self, workspace_id: Uuid) -> Result<Vec<CatalogToken>, CatalogError> {
        let rows: Vec<TokenRow> = sqlx::query_as::<_, TokenRow>(
            "SELECT id, workspace_id, warehouse_id, label, scope, created_at \
             FROM catalog_tokens \
             WHERE workspace_id = $1 AND revoked_at IS NULL \
             ORDER BY created_at ASC",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await
        .map_err(CatalogError::Db)?;

        Ok(rows.into_iter().map(CatalogToken::from).collect())
    }

    /// List all non-revoked tokens for a single warehouse within a workspace
    /// (hashes excluded). The `workspace_id` guard keeps the lookup
    /// workspace-scoped even though `warehouse_id` is globally unique.
    pub async fn list_tokens_for_warehouse(
        &self,
        workspace_id: Uuid,
        warehouse_id: Uuid,
    ) -> Result<Vec<CatalogToken>, CatalogError> {
        let rows: Vec<TokenRow> = sqlx::query_as::<_, TokenRow>(
            "SELECT id, workspace_id, warehouse_id, label, scope, created_at \
             FROM catalog_tokens \
             WHERE workspace_id = $1 AND warehouse_id = $2 AND revoked_at IS NULL \
             ORDER BY created_at ASC",
        )
        .bind(workspace_id)
        .bind(warehouse_id)
        .fetch_all(self.pool())
        .await
        .map_err(CatalogError::Db)?;

        Ok(rows.into_iter().map(CatalogToken::from).collect())
    }

    /// Update a token's label/scope in place (hash untouched, so the existing
    /// secret keeps working). Scoped to (id, workspace, warehouse): returns the
    /// updated row, or `NotFound` when the id is not this warehouse's token.
    pub async fn update_token_for_warehouse(
        &self,
        workspace_id: Uuid,
        warehouse_id: Uuid,
        id: Uuid,
        label: &str,
        scope: &[Grant],
    ) -> Result<CatalogToken, CatalogError> {
        validate_grants(scope)?;
        let row: Option<TokenRow> = sqlx::query_as::<_, TokenRow>(
            "UPDATE catalog_tokens \
             SET label = $4, scope = $5 \
             WHERE id = $1 AND workspace_id = $2 AND warehouse_id = $3 AND revoked_at IS NULL \
             RETURNING id, workspace_id, warehouse_id, label, scope, created_at",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(warehouse_id)
        .bind(label)
        .bind(sqlx::types::Json(scope))
        .fetch_optional(self.pool())
        .await
        .map_err(CatalogError::Db)?;
        row.map(CatalogToken::from)
            .ok_or_else(|| CatalogError::NotFound("catalog token".into()))
    }

    /// Rotate a token's secret: generate a new raw, overwrite token_hash for the
    /// SAME row (label/scopes/created_at preserved, rotated_at = now). Returns the
    /// row + the new raw (shown once). NotFound when not this warehouse's token.
    pub async fn rotate_token_for_warehouse(
        &self,
        workspace_id: Uuid,
        warehouse_id: Uuid,
        id: Uuid,
    ) -> Result<(CatalogToken, String), CatalogError> {
        let raw = generate_raw_token();
        let hash = hash_token(&raw);
        let row: Option<TokenRow> = sqlx::query_as::<_, TokenRow>(
            "UPDATE catalog_tokens \
             SET token_hash = $4, rotated_at = now() \
             WHERE id = $1 AND workspace_id = $2 AND warehouse_id = $3 AND revoked_at IS NULL \
             RETURNING id, workspace_id, warehouse_id, label, scope, created_at",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(warehouse_id)
        .bind(&hash)
        .fetch_optional(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                CatalogError::AlreadyExists("token".into())
            }
            _ => CatalogError::Db(e),
        })?;
        let token = row
            .map(CatalogToken::from)
            .ok_or_else(|| CatalogError::NotFound("catalog token".into()))?;
        Ok((token, raw))
    }
}
