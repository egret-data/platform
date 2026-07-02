//! The top-level [`PolarisClient`]: typed management operations over the
//! Polaris management API, authenticated with a cached Keycloak bearer token.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::error::PolarisError;
use crate::model::{Catalog, Catalogs, Grant, NewCatalog, PrincipalWithCredentials};
use crate::token::{PolarisConfig, TokenSource};
use crate::transport::HttpTransport;

/// Async client for the Polaris management API (`/api/management/v1`).
pub struct PolarisClient {
    base: String,
    token: TokenSource,
    http: Arc<dyn HttpTransport>,
}

impl PolarisClient {
    /// Build a client from config and a shared transport. The [`TokenSource`]
    /// is derived from the Keycloak fields of `cfg`.
    pub fn new(cfg: PolarisConfig, http: Arc<dyn HttpTransport>) -> Self {
        let token = TokenSource::new(
            cfg.kc_token_url,
            cfg.kc_client_id,
            cfg.kc_client_secret,
            cfg.kc_scope,
        );
        Self {
            base: cfg.base_url,
            token,
            http,
        }
    }

    /// Build a fully-qualified management URL for `path` (which must start `/`).
    fn mgmt(&self, path: &str) -> String {
        format!(
            "{}/api/management/v1{}",
            self.base.trim_end_matches('/'),
            path
        )
    }

    /// Perform a request, decoding the JSON body into `T` on success and
    /// mapping status codes to typed errors.
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<T, PolarisError> {
        let bearer = self
            .token
            .bearer(self.http.as_ref(), std::time::Instant::now())
            .await?;
        let (status, bytes) = self
            .http
            .send(method, &self.mgmt(path), Some(&bearer), body)
            .await?;
        match status {
            200 | 201 => Ok(serde_json::from_slice(&bytes)?),
            204 => Ok(serde_json::from_slice(b"null")?),
            403 => Err(PolarisError::Forbidden),
            404 => Err(PolarisError::NotFound),
            409 => Err(PolarisError::Conflict),
            s => Err(PolarisError::Status(
                s,
                String::from_utf8_lossy(&bytes).into_owned(),
            )),
        }
    }

    /// Like [`Self::call`] but discards the success body (for mutations with no
    /// useful response). `200`/`201`/`204` all map to `Ok(())`.
    async fn call_unit(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Result<(), PolarisError> {
        let bearer = self
            .token
            .bearer(self.http.as_ref(), std::time::Instant::now())
            .await?;
        let (status, bytes) = self
            .http
            .send(method, &self.mgmt(path), Some(&bearer), body)
            .await?;
        match status {
            200 | 201 | 204 => Ok(()),
            403 => Err(PolarisError::Forbidden),
            404 => Err(PolarisError::NotFound),
            409 => Err(PolarisError::Conflict),
            s => Err(PolarisError::Status(
                s,
                String::from_utf8_lossy(&bytes).into_owned(),
            )),
        }
    }

    // --- catalogs ---

    /// Create a catalog: `POST /catalogs`.
    pub async fn create_catalog(&self, c: &NewCatalog) -> Result<Catalog, PolarisError> {
        self.call("POST", "/catalogs", Some(json!({ "catalog": c.to_wire() })))
            .await
    }

    /// List all catalogs: `GET /catalogs`.
    pub async fn list_catalogs(&self) -> Result<Vec<Catalog>, PolarisError> {
        let cs: Catalogs = self.call("GET", "/catalogs", None).await?;
        Ok(cs.catalogs)
    }

    /// Fetch a single catalog: `GET /catalogs/{name}`.
    pub async fn get_catalog(&self, name: &str) -> Result<Catalog, PolarisError> {
        self.call("GET", &format!("/catalogs/{name}"), None).await
    }

    /// Delete a catalog: `DELETE /catalogs/{name}`.
    pub async fn delete_catalog(&self, name: &str) -> Result<(), PolarisError> {
        self.call_unit("DELETE", &format!("/catalogs/{name}"), None)
            .await
    }

    // --- principals ---

    /// Create a principal, returning its one-time credentials: `POST /principals`.
    pub async fn create_principal(
        &self,
        name: &str,
    ) -> Result<PrincipalWithCredentials, PolarisError> {
        self.call(
            "POST",
            "/principals",
            Some(json!({ "principal": { "name": name } })),
        )
        .await
    }

    /// Delete a principal: `DELETE /principals/{name}`.
    pub async fn delete_principal(&self, name: &str) -> Result<(), PolarisError> {
        self.call_unit("DELETE", &format!("/principals/{name}"), None)
            .await
    }

    // --- roles ---

    /// Create a principal-role: `POST /principal-roles`.
    pub async fn create_principal_role(&self, name: &str) -> Result<(), PolarisError> {
        self.call_unit(
            "POST",
            "/principal-roles",
            Some(json!({ "principalRole": { "name": name } })),
        )
        .await
    }

    /// Create a catalog-role in a catalog: `POST /catalogs/{catalog}/catalog-roles`.
    pub async fn create_catalog_role(&self, catalog: &str, name: &str) -> Result<(), PolarisError> {
        self.call_unit(
            "POST",
            &format!("/catalogs/{catalog}/catalog-roles"),
            Some(json!({ "catalogRole": { "name": name } })),
        )
        .await
    }

    // --- assignments ---

    /// Assign a principal-role to a principal:
    /// `PUT /principals/{principal}/principal-roles`.
    pub async fn assign_principal_role(
        &self,
        principal: &str,
        role: &str,
    ) -> Result<(), PolarisError> {
        self.call_unit(
            "PUT",
            &format!("/principals/{principal}/principal-roles"),
            Some(json!({ "principalRole": { "name": role } })),
        )
        .await
    }

    /// Assign a catalog-role (of `catalog`) to a principal-role:
    /// `PUT /principal-roles/{principal_role}/catalog-roles/{catalog}`.
    pub async fn assign_catalog_role(
        &self,
        principal_role: &str,
        catalog: &str,
        catalog_role: &str,
    ) -> Result<(), PolarisError> {
        self.call_unit(
            "PUT",
            &format!("/principal-roles/{principal_role}/catalog-roles/{catalog}"),
            Some(json!({ "catalogRole": { "name": catalog_role } })),
        )
        .await
    }

    // --- grants ---

    /// Add a grant to a catalog-role:
    /// `PUT /catalogs/{catalog}/catalog-roles/{catalog_role}/grants`.
    pub async fn add_grant(
        &self,
        catalog: &str,
        catalog_role: &str,
        g: &Grant,
    ) -> Result<(), PolarisError> {
        self.call_unit(
            "PUT",
            &format!("/catalogs/{catalog}/catalog-roles/{catalog_role}/grants"),
            Some(json!({ "grant": g.to_wire() })),
        )
        .await
    }

    /// Revoke a grant from a catalog-role (same body, `POST`):
    /// `POST /catalogs/{catalog}/catalog-roles/{catalog_role}/grants`.
    pub async fn revoke_grant(
        &self,
        catalog: &str,
        catalog_role: &str,
        g: &Grant,
    ) -> Result<(), PolarisError> {
        self.call_unit(
            "POST",
            &format!("/catalogs/{catalog}/catalog-roles/{catalog_role}/grants"),
            Some(json!({ "grant": g.to_wire() })),
        )
        .await
    }

    // --- health ---

    /// Liveness probe: succeeds if catalogs can be listed.
    pub async fn health(&self) -> Result<(), PolarisError> {
        self.list_catalogs().await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CatalogType, StorageConfig};
    use crate::transport::FakeTransport;
    use std::collections::HashMap;

    /// A scripted 200 Keycloak token response, consumed by the first
    /// `TokenSource::bearer` call before any management request runs.
    const TOKEN_OK: &[u8] = br#"{"access_token":"AT","token_type":"bearer","expires_in":3600}"#;

    fn client(fake: &FakeTransport) -> PolarisClient {
        PolarisClient::new(
            PolarisConfig {
                base_url: "http://polaris".into(),
                kc_token_url: "http://kc".into(),
                kc_client_id: "c".into(),
                kc_client_secret: "s".into(),
                kc_scope: None,
            },
            Arc::new(fake.clone()),
        )
    }

    #[tokio::test]
    async fn create_catalog_posts_expected_body() {
        let fake = FakeTransport::new(vec![
            (200, TOKEN_OK.to_vec()),
            (201, br#"{"type":"INTERNAL","name":"lakehouse"}"#.to_vec()),
        ]);
        let c = client(&fake);
        let new = NewCatalog {
            name: "lakehouse".into(),
            catalog_type: CatalogType::Internal,
            storage: StorageConfig::S3 {
                allowed_locations: vec!["s3://bucket/warehouse".into()],
                role_arn: Some("arn:aws:iam::123:role/polaris".into()),
                region: Some("us-east-1".into()),
            },
            properties: HashMap::new(),
        };
        let out = c.create_catalog(&new).await.unwrap();
        assert_eq!(out.name, "lakehouse");

        let (method, url, body) = fake.last_request();
        assert_eq!(method, "POST");
        assert!(url.ends_with("/api/management/v1/catalogs"), "url: {url}");
        assert_eq!(body["catalog"]["name"], "lakehouse");
        assert_eq!(body["catalog"]["storageConfigInfo"]["storageType"], "S3");
        assert_eq!(
            body["catalog"]["storageConfigInfo"]["allowedLocations"][0],
            "s3://bucket/warehouse"
        );
    }

    #[tokio::test]
    async fn create_principal_returns_credentials() {
        let fake = FakeTransport::new(vec![
            (200, TOKEN_OK.to_vec()),
            (
                201,
                br#"{"principal":{"name":"svc"},"credentials":{"clientId":"CID","clientSecret":"SEC"}}"#.to_vec(),
            ),
        ]);
        let c = client(&fake);
        let pc = c.create_principal("svc").await.unwrap();
        assert_eq!(pc.principal.name, "svc");
        assert_eq!(pc.credentials.client_id, "CID");
        assert_eq!(pc.credentials.client_secret, "SEC");

        let (method, url, body) = fake.last_request();
        assert_eq!(method, "POST");
        assert!(url.ends_with("/api/management/v1/principals"), "url: {url}");
        assert_eq!(body["principal"]["name"], "svc");
    }

    #[tokio::test]
    async fn add_table_grant_puts_grant_body() {
        let fake = FakeTransport::new(vec![(200, TOKEN_OK.to_vec()), (200, b"".to_vec())]);
        let c = client(&fake);
        c.add_grant(
            "lakehouse",
            "reader",
            &Grant::Table {
                namespace: vec!["marts".into()],
                table: "sales".into(),
                privilege: "TABLE_READ_DATA".into(),
            },
        )
        .await
        .unwrap();

        let (method, url, body) = fake.last_request();
        assert_eq!(method, "PUT");
        assert!(
            url.ends_with("/catalogs/lakehouse/catalog-roles/reader/grants"),
            "url: {url}"
        );
        assert_eq!(body["grant"]["type"], "table");
        assert_eq!(body["grant"]["namespace"][0], "marts");
        assert_eq!(body["grant"]["tableName"], "sales");
        assert_eq!(body["grant"]["privilege"], "TABLE_READ_DATA");
    }

    #[tokio::test]
    async fn list_catalogs_parses() {
        let fake = FakeTransport::new(vec![
            (200, TOKEN_OK.to_vec()),
            (
                200,
                br#"{"catalogs":[{"type":"INTERNAL","name":"lakehouse"}]}"#.to_vec(),
            ),
        ]);
        let c = client(&fake);
        let cats = c.list_catalogs().await.unwrap();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].name, "lakehouse");
        assert_eq!(cats[0].catalog_type, "INTERNAL");
    }

    #[tokio::test]
    async fn forbidden_maps_to_error() {
        let fake = FakeTransport::new(vec![(200, TOKEN_OK.to_vec()), (403, b"nope".to_vec())]);
        let c = client(&fake);
        let err = c.list_catalogs().await.unwrap_err();
        assert!(matches!(err, PolarisError::Forbidden), "err: {err:?}");
    }

    #[tokio::test]
    async fn assign_catalog_role_puts_expected() {
        let fake = FakeTransport::new(vec![(200, TOKEN_OK.to_vec()), (204, b"".to_vec())]);
        let c = client(&fake);
        c.assign_catalog_role("analysts", "lakehouse", "reader")
            .await
            .unwrap();

        let (method, url, body) = fake.last_request();
        assert_eq!(method, "PUT");
        assert!(
            url.ends_with("/principal-roles/analysts/catalog-roles/lakehouse"),
            "url: {url}"
        );
        assert_eq!(body["catalogRole"]["name"], "reader");
    }
}
