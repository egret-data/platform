//! OIDC-JWT authentication for the catalog: verify a Keycloak access token, map
//! the subject to an egret user, and produce a default `notebook_{username}`
//! read/write grant. Verification is behind a trait so the mapping is unit-testable.
//!
//! This is the alternative to opaque `catalog_tokens`: a Keycloak access-token
//! JWT is accepted directly. The request's `{prefix}` path segment is a
//! lakekeeper warehouse UUID; we reverse-resolve it to an egret workspace, check
//! the verified subject's user is a member, and emit the SAME
//! [`egret_catalog::service::TokenAuth`] shape opaque tokens produce — one grant:
//! read+write on the user's own `notebook_{username}` namespace. Producing that
//! identical shape is the whole point: downstream front-auth + the per-grant
//! `EgretAuthorizer` stay unchanged.

use std::sync::Arc;

use egret_catalog::service::{CatalogStore, Grant, TokenAuth};
use egret_core::WorkspaceId;
use egret_metastore::Db;
use uuid::Uuid;

/// Verifies a Keycloak access-token JWT and returns its subject. Behind a trait
/// so unit tests can substitute a fake (no IdP / network).
#[async_trait::async_trait]
pub trait AccessTokenVerifier: Send + Sync {
    /// Verify a Keycloak access-token JWT (sig/iss/aud/exp) and return its
    /// subject (`sub`).
    async fn verify(&self, token: &str) -> Result<String, JwtAuthError>;
}

/// Failure modes of JWT catalog authentication.
#[derive(Debug)]
pub enum JwtAuthError {
    /// Bad/expired/untrusted token, or a verified subject with no egret user.
    Unauthenticated,
    /// Authenticated, but not a member of the target workspace, or the request's
    /// warehouse prefix does not resolve to a known warehouse.
    Forbidden,
}

/// Authenticates a bearer JWT against a request's warehouse and maps it to a
/// `notebook_{username}` read/write grant.
pub struct CatalogJwtAuth {
    verifier: Arc<dyn AccessTokenVerifier>,
    store: Arc<CatalogStore>,
    db: Db,
}

impl CatalogJwtAuth {
    pub fn new(verifier: Arc<dyn AccessTokenVerifier>, store: Arc<CatalogStore>, db: Db) -> Self {
        Self {
            verifier,
            store,
            db,
        }
    }

    /// Authenticate a bearer JWT for a request targeting `prefix_lakekeeper_uuid`
    /// (the `{prefix}` path segment = a lakekeeper warehouse UUID). Returns the
    /// same [`TokenAuth`] shape opaque tokens produce, with one grant: rw on
    /// `notebook_{username}`.
    pub async fn authenticate(
        &self,
        token: &str,
        prefix_lakekeeper_uuid: Uuid,
    ) -> Result<TokenAuth, JwtAuthError> {
        // 1. Verify the JWT and resolve its subject to an egret user.
        let subject = self.verifier.verify(token).await?;
        let user = egret_auth::users::UserRepo::new(self.db.clone())
            .find_by_external_subject(&subject)
            .await
            .map_err(|_| JwtAuthError::Unauthenticated)?
            .ok_or(JwtAuthError::Unauthenticated)?;

        // 2. Reverse-resolve the request's warehouse prefix to (workspace, row).
        let (workspace_id, warehouse_row_id) = self
            .store
            .warehouse_by_lakekeeper_id(prefix_lakekeeper_uuid)
            .await
            .map_err(|_| JwtAuthError::Forbidden)?
            .ok_or(JwtAuthError::Forbidden)?;

        // 3. Require workspace membership.
        let roles = egret_workspace::rbac::Authorizer::new(self.db.clone())
            .workspace_roles_for(user.id, WorkspaceId(workspace_id))
            .await
            .map_err(|_| JwtAuthError::Forbidden)?;
        if roles.is_empty() {
            return Err(JwtAuthError::Forbidden);
        }

        // 4. Emit the default rw grant on the user's own notebook namespace.
        let username = egret_auth::users::UserRepo::new(self.db.clone())
            .ensure_username(user.id)
            .await
            .map_err(|_| JwtAuthError::Unauthenticated)?;
        let ns = format!("notebook_{username}");
        let perms = vec!["catalog:read".to_string(), "catalog:write".to_string()];
        Ok(TokenAuth {
            workspace_id,
            warehouse_id: warehouse_row_id,
            grants: vec![Grant {
                namespace: ns,
                table: None,
                permissions: perms.clone(),
            }],
            permissions: perms,
        })
    }
}

// ---------------------------------------------------------------------------
// Real verifier — wraps lakekeeper's bundled `limes` JWKS authenticator.
// ---------------------------------------------------------------------------

/// Production [`AccessTokenVerifier`] backed by `limes`'
/// [`JWKSWebAuthenticator`](lakekeeper::limes::jwks::JWKSWebAuthenticator): it
/// fetches the IdP's JWKS from the issuer's OIDC discovery document and verifies
/// the JWT signature / issuer / audience / expiry.
pub struct LimesVerifier {
    inner: lakekeeper::limes::jwks::JWKSWebAuthenticator,
}

impl LimesVerifier {
    /// Build a verifier for `issuer_url` (the OIDC issuer; discovery is derived
    /// from it) accepting `audience` as a valid `aud` claim.
    pub async fn new(issuer_url: &str, audience: String) -> anyhow::Result<Self> {
        let inner = lakekeeper::limes::jwks::JWKSWebAuthenticator::new(issuer_url, None)
            .await?
            .set_accepted_audiences(vec![audience]);
        Ok(Self { inner })
    }
}

#[async_trait::async_trait]
impl AccessTokenVerifier for LimesVerifier {
    async fn verify(&self, token: &str) -> Result<String, JwtAuthError> {
        use lakekeeper::limes::Authenticator;
        let auth = self
            .inner
            .authenticate(token)
            .await
            .map_err(|_| JwtAuthError::Unauthenticated)?;
        // The subject's id within the IdP is the OIDC `sub` claim — exactly what
        // egret stores in `users.external_subject` for `provider = 'oidc'`.
        Ok(auth.subject().subject_in_idp().to_string())
    }
}
