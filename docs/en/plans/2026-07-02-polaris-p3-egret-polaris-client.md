# Control-Plane Polaris — Plan 3: the `egret-polaris` client crate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P3 of the epic.** Depends on nothing from P1/P2 to compile (a standalone crate); consumed by P4 (routes). VI reference: `docs/vi/plans/2026-07-02-polaris-p3-egret-polaris-client.md`.

**Goal:** A typed, tested Rust client (`egret-polaris`) over Apache Polaris's **management API** — catalogs, principals (+credentials), principal-roles, catalog-roles, grants — plus admin-token acquisition, with a mockable HTTP boundary so it unit-tests without a live Polaris.

**Architecture:** `reqwest`-based async client. It obtains an **admin bearer token via OAuth2 client-credentials from Keycloak** (Polaris `/v1/oauth/tokens` is deprecated-for-removal; MIXED mode accepts Keycloak OIDC). Every management call sends `Authorization: Bearer <token>` to `{base_url}/api/management/v1/…`. An `HttpTransport` trait wraps the HTTP layer so tests inject canned responses.

**Tech Stack:** Rust (`reqwest`, `serde`, `serde_json`, `tokio`), Polaris management OpenAPI (`polaris/spec/polaris-management-service.yml`).

---

## Background the implementer needs (Polaris management API, from `spec/polaris-management-service.yml`)

- **Base:** management endpoints are under `/api/management/v1`. Auth: `Authorization: Bearer <token>`.
- **Token:** Polaris `/v1/oauth/tokens` (client_credentials) is **DEPRECATED for removal** — do NOT use it. Obtain the admin token from **Keycloak** client-credentials: `POST {keycloak}/protocol/openid-connect/token` form `grant_type=client_credentials&client_id&client_secret&scope` → `{access_token, token_type, expires_in}`. Polaris (MIXED) validates it via OIDC.
- **Catalogs:** `POST /catalogs` body `{"catalog": Catalog}`, `Catalog = {type: "INTERNAL"|"EXTERNAL", name, properties: {..}, storageConfigInfo: {storageType: "S3"|"GCS"|"AZURE"|"FILE", allowedLocations: [".."], storageName?}}` (S3 adds `roleArn`/`externalId`/`region` via `AwsStorageConfigInfo`). `GET /catalogs`→`{catalogs:[Catalog]}`; `GET/DELETE /catalogs/{name}`. 201/403/404/409.
- **Principals:** `POST /principals` body `{"principal": {name, properties?}}` → `PrincipalWithCredentials {principal, credentials: {clientId, clientSecret}}` — **the created principal's client credentials are returned once** (the engine service credential). `GET /principals`, `GET/DELETE /principals/{name}`, `POST /principals/{name}/rotate`.
- **Principal-roles:** `POST /principal-roles` `{"principalRole": {name}}`; `GET/DELETE /principal-roles/{name}`. Assign to a principal: `PUT /principals/{name}/principal-roles` `{"principalRole": {name}}`; list `GET /principals/{name}/principal-roles`.
- **Catalog-roles:** `POST /catalogs/{c}/catalog-roles` `{"catalogRole": {name}}`; `GET/DELETE /catalogs/{c}/catalog-roles/{cr}`. Assign a catalog-role to a principal-role: `PUT /principal-roles/{pr}/catalog-roles/{c}` `{"catalogRole": {name}}`.
- **Grants:** `PUT /catalogs/{c}/catalog-roles/{cr}/grants` body `{"grant": GrantResource}`; revoke via `POST .../grants` with `RevokeGrantRequest` (per spec). `GrantResource` is discriminated by `type`: `TableGrant {type:"table", namespace:[..], tableName, privilege: TablePrivilege}`, `ViewGrant`, `NamespaceGrant`, `CatalogGrant` — table-level is the finest.
- Full field lists live in `spec/polaris-management-service.yml` (schemas `Catalog`, `AwsStorageConfigInfo`, `PrincipalWithCredentials`, `GrantResource`/`TableGrant`, `TablePrivilege`).

---

## Task 1: Crate scaffold + config + admin-token acquisition

**Files:**
- Create: `crates/egret-polaris/Cargo.toml`, `crates/egret-polaris/src/{lib.rs, client.rs, transport.rs, token.rs, error.rs}`
- Modify: root `Cargo.toml` (workspace member)

- [ ] **Step 1: Scaffold** — add the crate to `[workspace] members`; `Cargo.toml` deps: `reqwest` (json, rustls-tls), `serde`/`serde_json`, `tokio`, `thiserror`, `async-trait`, `url`. `lib.rs` re-exports `PolarisClient`, `PolarisConfig`, `PolarisError`.

- [ ] **Step 2: `transport.rs` — mockable HTTP boundary**

```rust
#[async_trait::async_trait]
pub trait HttpTransport: Send + Sync {
    /// Returns (status, body_bytes). `method` is "GET"/"POST"/"PUT"/"DELETE".
    async fn send(&self, method: &str, url: &str, bearer: Option<&str>,
                  json_body: Option<serde_json::Value>) -> Result<(u16, Vec<u8>), PolarisError>;
}
pub struct ReqwestTransport { pub http: reqwest::Client }
// impl HttpTransport for ReqwestTransport { build request, set bearer + json, return status+bytes }
```

- [ ] **Step 3: `token.rs` — Keycloak client-credentials, cached**

```rust
pub struct TokenSource { token_url: String, client_id: String, client_secret: String, scope: Option<String>,
                         cached: tokio::sync::Mutex<Option<(String, std::time::Instant)>> }
impl TokenSource {
    /// Returns a valid bearer, refreshing ≥30s before expiry via client_credentials.
    pub async fn bearer(&self, t: &dyn HttpTransport, now: std::time::Instant) -> Result<String, PolarisError> { /* form POST → access_token + expires_in; cache */ }
}
```
(Take `now` as a param so tests are deterministic — no wall-clock in logic.)

- [ ] **Step 4: Unit test — token acquisition + cache**

```rust
#[tokio::test]
async fn token_source_fetches_and_caches() {
    let t = FakeTransport::with(vec![(200, br#"{"access_token":"AT","token_type":"bearer","expires_in":3600}"#.to_vec())]);
    let ts = TokenSource::new("http://kc/token","cid","sec",None);
    let now = std::time::Instant::now();
    assert_eq!(ts.bearer(&t, now).await.unwrap(), "AT");
    assert_eq!(ts.bearer(&t, now).await.unwrap(), "AT"); // cached: only ONE transport call
    assert_eq!(t.calls(), 1);
}
```

- [ ] **Step 5: Run + commit**

Run: `cargo test -p egret-polaris token_source_fetches_and_caches` (PASS).
```bash
git add crates/egret-polaris Cargo.toml
git commit -m "feat(polaris): egret-polaris crate scaffold + Keycloak token source (mockable transport)"
```

---

## Task 2: `PolarisClient` + catalog methods

**Files:** Modify `crates/egret-polaris/src/{client.rs, lib.rs}`; types in `src/model.rs` (create)

- [ ] **Step 1: Failing test — create + list catalog**

```rust
#[tokio::test]
async fn create_catalog_posts_expected_body() {
    let t = FakeTransport::with(vec![
        (200, token_json()),                     // token
        (201, br#"{"type":"INTERNAL","name":"lakehouse","storageConfigInfo":{"storageType":"S3","allowedLocations":["s3://wh/"]}}"#.to_vec()),
    ]);
    let c = PolarisClient::new(cfg("http://polaris"), std::sync::Arc::new(t.clone()));
    let cat = c.create_catalog(&NewCatalog {
        name: "lakehouse".into(), catalog_type: CatalogType::Internal,
        storage: StorageConfig::S3 { allowed_locations: vec!["s3://wh/".into()], role_arn: None, region: Some("us-east-1".into()) },
        properties: Default::default(),
    }).await.unwrap();
    assert_eq!(cat.name, "lakehouse");
    let (m, url, body) = t.last_request();
    assert_eq!((m.as_str(), url.ends_with("/api/management/v1/catalogs")), ("POST", true));
    assert_eq!(body["catalog"]["name"], "lakehouse");
    assert_eq!(body["catalog"]["storageConfigInfo"]["storageType"], "S3");
}
```

- [ ] **Step 2: Implement `PolarisClient` + catalog model + methods**

```rust
pub struct PolarisClient { base: String, token: TokenSource, http: std::sync::Arc<dyn HttpTransport> }
impl PolarisClient {
    fn mgmt(&self, path: &str) -> String { format!("{}/api/management/v1{}", self.base.trim_end_matches('/'), path) }
    async fn call<T: serde::de::DeserializeOwned>(&self, method: &str, path: &str, body: Option<serde_json::Value>) -> Result<T, PolarisError> {
        let bearer = self.token.bearer(self.http.as_ref(), std::time::Instant::now()).await?;
        let (status, bytes) = self.http.send(method, &self.mgmt(path), Some(&bearer), body).await?;
        match status { 200|201 => Ok(serde_json::from_slice(&bytes)?),
                       204 => Ok(serde_json::from_slice(b"null").unwrap_or_else(|_| serde_json::from_value(serde_json::Value::Null).unwrap())),
                       403 => Err(PolarisError::Forbidden), 404 => Err(PolarisError::NotFound),
                       409 => Err(PolarisError::Conflict), s => Err(PolarisError::Status(s, String::from_utf8_lossy(&bytes).into())) }
    }
    pub async fn create_catalog(&self, c: &NewCatalog) -> Result<Catalog, PolarisError> {
        self.call("POST", "/catalogs", Some(serde_json::json!({ "catalog": c.to_wire() }))).await
    }
    pub async fn list_catalogs(&self) -> Result<Vec<Catalog>, PolarisError> {
        let w: Catalogs = self.call("GET", "/catalogs", None).await?; Ok(w.catalogs)
    }
    pub async fn get_catalog(&self, name: &str) -> Result<Catalog, PolarisError> { self.call("GET", &format!("/catalogs/{name}"), None).await }
    pub async fn delete_catalog(&self, name: &str) -> Result<(), PolarisError> { let _: serde_json::Value = self.call("DELETE", &format!("/catalogs/{name}"), None).await?; Ok(()) }
}
```
`model.rs`: `NewCatalog`, `CatalogType {Internal, External}`, `StorageConfig` enum (S3/Gcs/Azure/File with `allowed_locations` + S3 `role_arn`/`region`) with a `to_wire()` producing the `storageConfigInfo` JSON, `Catalog`, `Catalogs { catalogs: Vec<Catalog> }`.

- [ ] **Step 3: Run + commit** — `cargo test -p egret-polaris create_catalog` (PASS).
```bash
git add crates/egret-polaris
git commit -m "feat(polaris): PolarisClient + catalog CRUD (create/list/get/delete)"
```

---

## Task 3: Principals (+ returned credentials) + roles + assignments

**Files:** Modify `crates/egret-polaris/src/{client.rs, model.rs}`

- [ ] **Step 1: Failing test — create principal returns credentials**

```rust
#[tokio::test]
async fn create_principal_returns_client_credentials() {
    let t = FakeTransport::with(vec![(200, token_json()),
        (201, br#"{"principal":{"name":"eng-eng"},"credentials":{"clientId":"CID","clientSecret":"CSEC"}}"#.to_vec())]);
    let c = PolarisClient::new(cfg("http://polaris"), std::sync::Arc::new(t));
    let pc = c.create_principal("eng-eng").await.unwrap();
    assert_eq!((pc.credentials.client_id.as_str(), pc.credentials.client_secret.as_str()), ("CID","CSEC"));
}
```

- [ ] **Step 2: Implement principal + role methods**

```rust
pub async fn create_principal(&self, name: &str) -> Result<PrincipalWithCredentials, PolarisError> {
    self.call("POST", "/principals", Some(serde_json::json!({"principal":{"name":name}}))).await
}
pub async fn delete_principal(&self, name: &str) -> Result<(), PolarisError> { let _:serde_json::Value = self.call("DELETE", &format!("/principals/{name}"), None).await?; Ok(()) }
pub async fn create_principal_role(&self, name: &str) -> Result<(), PolarisError> { let _:serde_json::Value = self.call("POST","/principal-roles",Some(serde_json::json!({"principalRole":{"name":name}}))).await?; Ok(()) }
pub async fn create_catalog_role(&self, catalog: &str, name: &str) -> Result<(), PolarisError> { let _:serde_json::Value = self.call("POST",&format!("/catalogs/{catalog}/catalog-roles"),Some(serde_json::json!({"catalogRole":{"name":name}}))).await?; Ok(()) }
pub async fn assign_principal_role(&self, principal: &str, role: &str) -> Result<(), PolarisError> { let _:serde_json::Value = self.call("PUT",&format!("/principals/{principal}/principal-roles"),Some(serde_json::json!({"principalRole":{"name":role}}))).await?; Ok(()) }
pub async fn assign_catalog_role(&self, principal_role: &str, catalog: &str, catalog_role: &str) -> Result<(), PolarisError> { let _:serde_json::Value = self.call("PUT",&format!("/principal-roles/{principal_role}/catalog-roles/{catalog}"),Some(serde_json::json!({"catalogRole":{"name":catalog_role}}))).await?; Ok(()) }
```
`model.rs`: `PrincipalWithCredentials { principal: Principal, credentials: Credentials { client_id, client_secret } }` (serde rename `clientId`/`clientSecret`).

- [ ] **Step 3: Run + commit** — `cargo test -p egret-polaris create_principal` (PASS).
```bash
git add crates/egret-polaris
git commit -m "feat(polaris): principals (+credentials), principal-roles, catalog-roles, assignments"
```

---

## Task 4: Grants (table/namespace/catalog level) + error mapping + health

**Files:** Modify `crates/egret-polaris/src/{client.rs, model.rs, error.rs}`

- [ ] **Step 1: Failing test — add a table grant**

```rust
#[tokio::test]
async fn add_table_grant_puts_grant_body() {
    let t = FakeTransport::with(vec![(200, token_json()), (201, b"{}".to_vec())]);
    let c = PolarisClient::new(cfg("http://polaris"), std::sync::Arc::new(t.clone()));
    c.add_grant("lakehouse","analysts",&Grant::Table{namespace:vec!["marts".into()],table:"sales".into(),privilege:"TABLE_READ_DATA".into()}).await.unwrap();
    let (m,url,body)=t.last_request();
    assert_eq!(m,"PUT"); assert!(url.ends_with("/api/management/v1/catalogs/lakehouse/catalog-roles/analysts/grants"));
    assert_eq!(body["grant"]["type"],"table");
    assert_eq!(body["grant"]["privilege"],"TABLE_READ_DATA");
}
```

- [ ] **Step 2: Implement grants + error variants**

```rust
pub enum Grant {
    Catalog { privilege: String },
    Namespace { namespace: Vec<String>, privilege: String },
    Table { namespace: Vec<String>, table: String, privilege: String },
    View { namespace: Vec<String>, view: String, privilege: String },
}
impl Grant { fn to_wire(&self) -> serde_json::Value { /* {type, namespace?, tableName?/viewName?, privilege} */ } }
pub async fn add_grant(&self, catalog:&str, catalog_role:&str, g:&Grant) -> Result<(), PolarisError> {
    let _:serde_json::Value = self.call("PUT",&format!("/catalogs/{catalog}/catalog-roles/{catalog_role}/grants"),Some(serde_json::json!({"grant":g.to_wire()}))).await?; Ok(())
}
pub async fn revoke_grant(&self, catalog:&str, catalog_role:&str, g:&Grant) -> Result<(), PolarisError> { /* POST .../grants with RevokeGrantRequest */ }
```
`error.rs`: `enum PolarisError { Forbidden, NotFound, Conflict, Status(u16,String), Http(String), Decode(String) }` (`thiserror`; `From<reqwest::Error>`/`serde_json::Error`).

- [ ] **Step 3: Health check**

```rust
/// Liveness: token acquisition + a cheap authenticated GET (list catalogs).
pub async fn health(&self) -> Result<(), PolarisError> { self.list_catalogs().await.map(|_| ()) }
```

- [ ] **Step 4: Run + commit** — `cargo test -p egret-polaris` (all PASS).
```bash
git add crates/egret-polaris
git commit -m "feat(polaris): grants (table/namespace/catalog), error mapping, health"
```

---

## Task 5: Verification

- [ ] **Step 1: fmt + clippy + tests**

Run:
```bash
cargo fmt -p egret-polaris --check
cargo clippy -p egret-polaris --all-targets -- -D warnings
cargo test -p egret-polaris
```
Expected: clean/pass — the crate builds standalone with no live Polaris (all tests use `FakeTransport`).

- [ ] **Step 2: Confirm the public surface** — `lib.rs` exports `PolarisClient`, `PolarisConfig`, `PolarisError`, and the model types (`NewCatalog`, `Catalog`, `CatalogType`, `StorageConfig`, `PrincipalWithCredentials`, `Grant`) that P4 consumes.

---

## Self-review notes (spec coverage for P3)

- Spec **B. `egret-polaris` client**: token (Task 1), catalogs (Task 2), principals+credentials+roles+assignments (Task 3), grants+errors+health (Task 4). Mockable transport → unit-tested without live Polaris (Task 1/5).
- **Token decision:** admin token via **Keycloak client-credentials** (Polaris `/v1/oauth/tokens` is deprecated-for-removal). This refines the spec's "MIXED" — egret-admin authenticates to Polaris with a Keycloak-issued OIDC token.
- **Open item for P4 (flag):** the **engine service-credential model** — `create_principal` returns a Polaris-native `clientId`/`clientSecret` that only works via Polaris's *deprecated* internal token endpoint. For engines to reach Polaris without the deprecated endpoint, either (a) engines use Keycloak service accounts mapped to Polaris principals via OIDC (future-proof), or (b) accept the deprecated endpoint for now. **Decide in P4** (assignment provisions the credential).
- **Out of P3:** control-plane routes (P4), UI (P5), Polaris compose/e2e (P6).
