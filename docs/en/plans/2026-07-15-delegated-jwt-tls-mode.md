# Delegated Trino Queries — jwt Mode (TLS + bearer) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `delegationMode="jwt"`: a TLS coordinator (egret self-signed cert, pinned by the connector) where each delegated query authenticates with the caller's own JWT (bearer) — identity bound by the token, no delegator/impersonation.

**Architecture:** A jwt-mode engine is provisioned with `http-server.https` (keystore = an egret-generated self-signed cert+key, mounted from a k8s Secret) and JWT-only auth. Delegated `query.databases` rows carry `options.http_scheme=https` + `options.tls_cert` (the public cert) + the https port; the connector builds a reqwest client that pins that cert. The jwt mint path in `routes.rs` (from #254) is reused unchanged. jwt is per-engine exclusive and seeds NO `egret_delegator`.

**Tech Stack:** Rust (axum, sqlx), `rcgen` (self-signed cert), `reqwest` (rustls-tls, `add_root_cert`), Trino https over PEM keystore, Postgres.

**Spec:** `docs/en/specs/2026-07-15-delegated-jwt-tls-mode-design.md`

---

## File Structure

- `migrations/0086_engine_tls.sql` (create) — `tls_secret_id`, `coordinator_cert` columns on `engine_mgmt.engines`.
- `crates/egret-engine-mgmt/src/store.rs` (modify) — the two fields on `Engine` + `is_jwt_delegation()`; add columns to every `Engine` SELECT.
- `crates/egret-engine-mgmt/src/tls_cert.rs` (create) — `generate_self_signed(sans) -> (cert_pem, key_pem)` via rcgen (pure fn + test).
- `crates/egret-engine-mgmt-api/src/lib.rs` (modify) — `Tls { keystore_pem, port }` on the provisioning `TrinoConfig`.
- `crates/egret-orchestrator/src/trino.rs` (modify) — render `http-server.https.*` + a keystore Secret + mount when `tls` is present; JWT-only auth path.
- `crates/egret-engine-mgmt/src/engines.rs` (modify) — `ensure_tls_cert` (gen/reuse), wire into `build_provision_req`, gate delegator seeding to impersonate-only, set `TrinoConfig.tls` for jwt.
- `crates/egret-query-api/src/lib.rs` (modify) — `ConnOptions.tls_cert`.
- `crates/egret-query/src/execution/connector/trino.rs` (modify) — pin `options.tls_cert` via reqwest `add_root_cert`.
- `crates/egret-engine-mgmt/src/assignment.rs` (modify) — `delegated_conn_options` jwt branch.

Verify commands (from repo root): `cargo test -p <crate> --lib <filter>`, `cargo fmt -p <crate>`, `cargo clippy -p <crate> --lib --tests`. **Run every cargo command synchronously/inline — never two at once (corrupts `target/`).**

---

## Task 1: Migration — TLS columns

**Files:** Create `migrations/0086_engine_tls.sql`; Modify `crates/egret-metastore/src/lib.rs`

- [ ] **Step 1: Write the migration** (migrations live at repo-root `migrations/`, embedded via `sqlx::migrate!` in metastore).

```sql
-- 0086_engine_tls.sql
-- jwt-mode delegation TLS: the coordinator serves HTTPS with an egret-generated
-- self-signed cert. `tls_secret_id` -> the egret secret holding the keystore PEM
-- (cert+key, mounted to the coordinator); `coordinator_cert` is the PUBLIC cert PEM
-- the query connector pins. Both NULL until an engine is provisioned in jwt mode.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN tls_secret_id uuid,
    ADD COLUMN coordinator_cert text;
```

- [ ] **Step 2: Force the embedded migrator to re-embed:** `printf '\n' >> crates/egret-metastore/src/lib.rs`
- [ ] **Step 3: Verify:** `cargo build -p egret-metastore` → builds.
- [ ] **Step 4: Commit:**
```bash
git add migrations/0086_engine_tls.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(engine): add tls_secret_id + coordinator_cert columns (jwt-mode TLS)"
```

---

## Task 2: Engine model — `is_jwt_delegation` + TLS fields

**Files:** Modify `crates/egret-engine-mgmt/src/store.rs`

Context: `Engine` (`#[derive(sqlx::FromRow)]`) already has `delegation_mode()` and `delegator_secret_id`. Every `SELECT` mapping to `Engine` lists its columns explicitly (grep `delegator_secret_id` — currently 4 SELECTs: create RETURNING, `list_engines`, `get_engine`, `list_engines_for_workspace`).

- [ ] **Step 1: Failing tests** (in `store.rs` `#[cfg(test)] mod tests`, repeat the full `Engine` literal — add the two new fields):

```rust
#[test]
fn is_jwt_delegation_true_only_for_jwt_mode() {
    let mut e = Engine {
        id: Uuid::nil(), name: "e".into(), r#type: "trino".into(),
        namespace: "ns".into(), warehouse_id: Uuid::nil(),
        spec: serde_json::json!({ "trinoConfig": { "delegationMode": "jwt" } }),
        status: "provisioning".into(), authz_service_id: None,
        delegator_secret_id: None, tls_secret_id: None, coordinator_cert: None,
        created_at: chrono::Utc::now(),
    };
    assert!(e.is_jwt_delegation());
    assert!(!e.is_impersonate_delegation());
    e.spec = serde_json::json!({ "trinoConfig": {} }); // default = impersonate
    assert!(!e.is_jwt_delegation());
}
```
(Read the real `Engine` struct first; match its exact field list/order.)

- [ ] **Step 2:** `cargo test -p egret-engine-mgmt --lib is_jwt_delegation` → FAIL.
- [ ] **Step 3: Implement.** Add `pub tls_secret_id: Option<Uuid>,` and `pub coordinator_cert: Option<String>,` to the `Engine` struct (after `delegator_secret_id`). Add BOTH columns to EVERY `Engine` SELECT (grep `delegator_secret_id` in store.rs → add `, tls_secret_id, coordinator_cert` to each column list, matching the `e.` prefix where used). Add:
```rust
/// True when delegated queries authenticate with the caller's own JWT over TLS.
pub fn is_jwt_delegation(&self) -> bool {
    self.delegation_mode() == "jwt"
}
```
Also add `tls_secret_id: None, coordinator_cert: None` to every other `Engine` struct literal in the crate that fails to compile (grep `delegator_secret_id: None`).

- [ ] **Step 4:** `cargo test -p egret-engine-mgmt --lib is_jwt_delegation` → PASS.
- [ ] **Step 5: fmt + clippy + commit:**
```bash
cargo fmt -p egret-engine-mgmt && cargo clippy -p egret-engine-mgmt --lib --tests
git add crates/egret-engine-mgmt/src/store.rs
git commit -m "feat(engine): is_jwt_delegation + tls_secret_id/coordinator_cert on Engine"
```

---

## Task 3: Cert generation module (`rcgen`)

**Files:** Create `crates/egret-engine-mgmt/src/tls_cert.rs`; Modify root `Cargo.toml`, `crates/egret-engine-mgmt/Cargo.toml`, `crates/egret-engine-mgmt/src/lib.rs`

- [ ] **Step 1: Add the dep.** In root `Cargo.toml` `[workspace.dependencies]` add `rcgen = { version = "0.13", default-features = false, features = ["pem", "crypto"] }`. In `crates/egret-engine-mgmt/Cargo.toml` `[dependencies]` add `rcgen.workspace = true`.

- [ ] **Step 2: Write the module + test** (`tls_cert.rs`):

```rust
//! Self-signed TLS cert generation for jwt-mode coordinators. The coordinator
//! serves HTTPS with this cert; egret's query connector pins the PUBLIC cert
//! (`coordinator_cert`) to validate the server. SANs include the coordinator
//! Service host AND localhost/127.0.0.1 so a dev `kubectl port-forward` also
//! validates.

use rcgen::{CertificateParams, DistinguishedName, KeyPair, SanType};

/// Generate a self-signed cert for `sans` (DNS names + IPs). Returns
/// `(cert_pem, key_pem)`. The keystore mounted to Trino is `cert_pem + key_pem`.
pub fn generate_self_signed(dns_sans: &[String]) -> Result<(String, String), String> {
    let mut params = CertificateParams::new(vec![]).map_err(|e| e.to_string())?;
    params.distinguished_name = DistinguishedName::new();
    params.subject_alt_names = dns_sans
        .iter()
        .map(|s| SanType::DnsName(s.clone().try_into().map_err(|_| "bad SAN".to_string())))
        .collect::<Result<Vec<_>, String>>()?;
    // Always cover loopback for the dev port-forward.
    params.subject_alt_names.push(SanType::IpAddress(std::net::IpAddr::from([127, 0, 0, 1])));
    params.subject_alt_names.push(SanType::DnsName("localhost".try_into().map_err(|_| "localhost".to_string())?));
    let key = KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    Ok((cert.pem(), key.serialize_pem()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_cert_with_svc_and_localhost_sans() {
        let (cert_pem, key_pem) = generate_self_signed(&["rgr2-coordinator.egret-engines.svc".into()]).unwrap();
        assert!(cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(key_pem.contains("PRIVATE KEY"));
        // Parse it back to confirm it is a valid cert and carries the SANs.
        let der = pem::parse(&cert_pem).unwrap();
        let (_, parsed) = x509_parser::parse_x509_certificate(der.contents()).unwrap();
        let san_str = format!("{:?}", parsed.subject_alternative_name().unwrap());
        assert!(san_str.contains("rgr2-coordinator.egret-engines.svc"));
        assert!(san_str.contains("localhost"));
    }
}
```

If `pem`/`x509_parser` aren't dev-deps of the crate, add them under `[dev-dependencies]` (`pem = "3"`, `x509-parser = "0.16"`), or simplify the assertion to just the PEM-header checks + that `generate_self_signed` returns `Ok` for the `.svc` name (drop the parse) — but PREFER verifying the SANs.

Add `mod tls_cert;` to `crates/egret-engine-mgmt/src/lib.rs`.

- [ ] **Step 3:** `cargo test -p egret-engine-mgmt --lib tls_cert` → PASS (compiles rcgen; test green).
- [ ] **Step 4: fmt + clippy + commit:**
```bash
cargo fmt -p egret-engine-mgmt && cargo clippy -p egret-engine-mgmt --lib --tests
git add Cargo.toml crates/egret-engine-mgmt/Cargo.toml crates/egret-engine-mgmt/src/tls_cert.rs crates/egret-engine-mgmt/src/lib.rs
git commit -m "feat(engine): self-signed cert generation for jwt-mode coordinators"
```

---

## Task 4: Provisioner — render HTTPS + keystore Secret + mount

**Files:** Modify `crates/egret-engine-mgmt-api/src/lib.rs`, `crates/egret-orchestrator/src/trino.rs`

Context: the orchestrator consumes `egret_engine_mgmt_api::{EngineSpec, TrinoConfig, Auth, Jwt}`. `TrinoConfig` (lib.rs ~308) has `auth: Option<Auth>`. The coordinator config is built in `trino.rs` (the `if coord {…}` block near the `auth` render ~135–160); mounts are a `volume_mounts` vec (~580) and `volumes` (~697); a k8s Secret is already rendered elsewhere (~392) — follow the `secretName`/`secret` volume pattern (also used at ~826/940).

- [ ] **Step 1: Add the provisioning DTO field.** In `crates/egret-engine-mgmt-api/src/lib.rs` add:
```rust
/// jwt-mode coordinator TLS. `keystore_pem` = cert+key PEM mounted as the Trino
/// https keystore; `port` = the https listener port.
#[derive(Clone, Debug, Default)]
pub struct Tls {
    pub keystore_pem: String,
    pub port: u16,
}
```
and add `pub tls: Option<Tls>,` to `struct TrinoConfig`.

- [ ] **Step 2: Provisioner test (failing).** In `trino.rs` tests, add a test that builds a spec with `trino_config.tls = Some(Tls { keystore_pem: "PEMDATA".into(), port: 8443 })` and asserts the coordinator config + configmap/secret:
```rust
#[test]
fn jwt_tls_renders_https_and_keystore_secret() {
    let mut s = /* a minimal EngineSpec with auth = Some(Auth { jwt: Some(...), password_users: vec![] }) */;
    s.trino_config.tls = Some(egret_engine_mgmt_api::Tls { keystore_pem: "PEMDATA".into(), port: 8443 });
    let cm = trino_config_map(&s);
    let cc = cm["data"]["config-coordinator.properties"].as_str().unwrap();
    assert!(cc.contains("http-server.https.enabled=true"));
    assert!(cc.contains("http-server.https.port=8443"));
    assert!(cc.contains("http-server.https.keystore.path=/etc/trino/tls/keystore.pem"));
    // keystore Secret rendered with the PEM
    let objs = trino_objects(&s); // the fn that returns the Vec<Value> of k8s objects; use the real name
    assert!(objs.iter().any(|o| o["kind"]=="Secret" && o["metadata"]["name"]==format!("{}-tls", s.name)
        && o["stringData"]["keystore.pem"]=="PEMDATA"));
    // coordinator mounts it
    // (assert the coordinator pod has a volume `tls` + mountPath /etc/trino/tls/keystore.pem)
}
```
(Use the REAL renderer fn names — grep `fn trino_config_map` / the object-list fn in trino.rs; match the existing `auth_config_password_and_jwt` test's construction of `EngineSpec`.)

- [ ] **Step 3:** `cargo test -p egret-orchestrator --lib jwt_tls` → FAIL.
- [ ] **Step 4: Implement the rendering.** In the coordinator `if coord` block, after the auth render, add:
```rust
if let Some(tls) = &cfg.tls {
    out.push_str("http-server.https.enabled=true\n");
    out.push_str(&format!("http-server.https.port={}\n", tls.port));
    out.push_str("http-server.https.keystore.path=/etc/trino/tls/keystore.pem\n");
}
```
Render a k8s Secret `{name}-tls` with `stringData: { "keystore.pem": tls.keystore_pem }` (only when `cfg.tls` is `Some`), add it to the returned object list. Add a coordinator volume `{ "name": "tls", "secret": { "secretName": format!("{}-tls", spec.name) } }` and a coordinator mount `{ "name": "tls", "mountPath": "/etc/trino/tls/keystore.pem", "subPath": "keystore.pem" }` — gated on `cfg.tls.is_some()` (workers don't need it).

- [ ] **Step 5:** `cargo test -p egret-orchestrator --lib` → PASS (new + existing).
- [ ] **Step 6: fmt + clippy + commit:**
```bash
cargo fmt -p egret-engine-mgmt-api -p egret-orchestrator && cargo clippy -p egret-orchestrator --lib --tests
git add crates/egret-engine-mgmt-api/src/lib.rs crates/egret-orchestrator/src/trino.rs
git commit -m "feat(engine): render coordinator HTTPS + keystore Secret for jwt mode"
```

---

## Task 5: `build_provision_req` — generate/reuse cert, JWT-only auth, gate delegator to impersonate

**Files:** Modify `crates/egret-engine-mgmt/src/engines.rs`, `crates/egret-engine-mgmt/src/store.rs`

Context: `build_provision_req` (engines.rs ~461) builds the orchestrator `EngineSpec`, calls `resolve_auth`, `inject_monitor_user`, and (from #254) the impersonate delegator block guarded by `engine.is_impersonate_delegation()`. `st.secrets` is a `WorkspaceSecretStore` (`create(None, name, desc, plaintext) -> Uuid`, `resolve_plaintext(None, id)`). The coordinator Service host is `engine.coordinator_host()`.

- [ ] **Step 1: `set_tls` in the store** (`store.rs`), mirroring `set_delegator_secret`:
```rust
/// Persist the generated TLS keystore secret id + public cert PEM for an engine.
pub async fn set_tls(&self, engine_id: Uuid, secret_id: Uuid, cert_pem: &str) -> Result<(), EngineMgmtError> {
    sqlx::query("UPDATE engine_mgmt.engines SET tls_secret_id = $2, coordinator_cert = $3 WHERE id = $1")
        .bind(engine_id).bind(secret_id).bind(cert_pem)
        .execute(self.pool()).await?; // use the crate's real pool accessor
    Ok(())
}
```

- [ ] **Step 2: `ensure_tls_cert`** in engines.rs (idempotent — reuse when already set):
```rust
/// Ensure the engine has a TLS cert; generate a self-signed one (SAN = the
/// coordinator Service host + localhost) on first call, store the keystore in an
/// egret secret + the public cert on the engine. Returns (keystore_pem, cert_pem).
async fn ensure_tls_cert(st: &CpState, engine: &Engine) -> Result<(String, String), CpError> {
    if let (Some(sid), Some(cert)) = (engine.tls_secret_id, engine.coordinator_cert.clone()) {
        let keystore = st.secrets.resolve_plaintext(None, sid).await
            .map_err(|e| CpError::BadRequest(format!("resolve tls secret: {e}")))?;
        return Ok((keystore, cert));
    }
    let (cert_pem, key_pem) = crate::tls_cert::generate_self_signed(&[engine.coordinator_host()])
        .map_err(|e| CpError::BadRequest(format!("generate tls cert: {e}")))?;
    let keystore = format!("{cert_pem}{key_pem}");
    let sid = st.secrets.create(None, &format!("engine-tls-{}", engine.id), "jwt-mode coordinator keystore", &keystore).await
        .map_err(|e| CpError::BadRequest(format!("store tls keystore: {e}")))?;
    st.engine_mgmt.set_tls(engine.id, sid, &cert_pem).await?;
    Ok((keystore, cert_pem))
}
```
(Match the real `st.secrets.create(..)` signature — read how `ensure_delegator_secret` calls it; adjust arg order/names. Handle its `AlreadyExists` the same way `ensure_delegator_secret` does.)

- [ ] **Step 3: Wire into `build_provision_req`.** Where the #254 delegator block sits, make it exclusive per mode:
```rust
if engine.is_impersonate_delegation() {
    // ... existing delegator gen + inject + seed (unchanged) ...
} else if engine.is_jwt_delegation() {
    let (keystore_pem, _cert) = ensure_tls_cert(st, engine).await?;
    trino_config.tls = Some(egret_engine_mgmt_api::Tls { keystore_pem, port: 8443 });
    // jwt mode is JWT-only: ensure no PASSWORD authenticator / delegator is added.
    // (Do NOT call inject_delegator_user / seed_delegator_policies here.)
}
```
Ensure the auth `type` for a jwt engine is rendered as JWT (not PASSWORD,JWT): if `auth_type()` is derived from whether `password_users` is non-empty, a jwt engine with empty `password_users` + `jwt: Some` should yield `type=JWT`. Confirm `inject_monitor_user` does NOT force a password user onto a jwt engine — if the monitor injection would add PASSWORD auth to a jwt engine, skip `inject_monitor_user` for jwt mode (the monitor uses basic-auth; over a JWT-only coordinator it can't authenticate — that's acceptable, the monitor degrades). Gate: `if !engine.is_jwt_delegation() { inject_monitor_user(&mut auth, engine.id)?; }`.

- [ ] **Step 4: Test.** Add a unit test in engines.rs (or an integration test) asserting: for a jwt engine, `build_provision_req` produces a `TrinoConfig.tls = Some(_)` and `auth.password_users` is empty (no delegator/monitor). If `build_provision_req` needs DB/secret access that's awkward to unit-test, instead test the smaller pieces: `ensure_tls_cert` is covered by Task 3's generator; add a focused test that `is_jwt_delegation()` selects the tls branch (a pure helper `fn tls_for_mode(engine) -> bool` = `is_jwt_delegation()`), OR rely on the live smoke — but PREFER a unit test on the branch selection.

- [ ] **Step 5:** `cargo test -p egret-engine-mgmt --lib` then `cargo build -p egret-bin` → pass/build.
- [ ] **Step 6: fmt + clippy + commit:**
```bash
cargo fmt -p egret-engine-mgmt && cargo clippy -p egret-engine-mgmt --lib --tests
git add crates/egret-engine-mgmt/src/engines.rs crates/egret-engine-mgmt/src/store.rs
git commit -m "feat(engine): jwt-mode provisions coordinator TLS (gen/reuse cert), JWT-only auth"
```

---

## Task 6: Connector — pin the engine cert

**Files:** Modify `crates/egret-query-api/src/lib.rs`, `crates/egret-query/src/execution/connector/trino.rs`

Context: `ConnOptions` (egret-query-api/src/lib.rs:48) has `user/http_scheme/catalog/database/impersonate`, all `#[serde(default, skip_serializing_if = "Option::is_none")]`. The connector reads `spec.options.http_scheme` (trino.rs:28) and builds its reqwest client in `execute` (trino.rs ~150: `reqwest::Client::builder()...build()`). reqwest is `rustls-tls`.

- [ ] **Step 1: Add the option.** In `ConnOptions` add:
```rust
/// Trino jwt-mode only: PEM of the coordinator's self-signed cert to pin as a
/// trusted root (server-auth over TLS). Set on egret-provisioned jwt connections.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub tls_cert: Option<String>,
```

- [ ] **Step 2: Failing test** (`connector/trino.rs` tests): build a `TrinoConnector` from a `ConnSpec` whose `options.tls_cert = Some(<a valid PEM>)` and `options.http_scheme = Some("https".into())`, and assert the connector stores/uses it. Since the reqwest client is built inside `execute`, extract client construction into a small `fn build_client(&self, timeout) -> Result<reqwest::Client, EngineError>` and unit-test that with a `tls_cert` set it returns `Ok` (the PEM parses + `add_root_cert` succeeds). Use a self-signed PEM generated in the test via `rcgen` (add rcgen as a dev-dep of egret-query) OR a hardcoded valid test PEM string.

```rust
#[test]
fn build_client_pins_tls_cert() {
    let (cert_pem, _key) = /* rcgen generate, or a const TEST_CERT_PEM */;
    let c = TrinoConnector::new(&ConnSpec {
        engine: Engine::Trino, host: "h".into(), port: 8443,
        options: ConnOptions { http_scheme: Some("https".into()), tls_cert: Some(cert_pem), ..Default::default() },
        secret: None, session_user: Some("alice".into()), bearer_token: Some("eyJ.j.s".into()),
    });
    assert!(c.build_client(std::time::Duration::from_secs(5)).is_ok());
}
```

- [ ] **Step 3:** `cargo test -p egret-query --lib build_client_pins` → FAIL.
- [ ] **Step 4: Implement.** Store `tls_cert: Option<String>` on `TrinoConnector` (set from `spec.options.tls_cert.clone()` in `new`). Add:
```rust
fn build_client(&self, timeout: std::time::Duration) -> Result<reqwest::Client, EngineError> {
    let mut b = reqwest::Client::builder().timeout(timeout);
    if let Some(pem) = &self.tls_cert {
        let cert = reqwest::Certificate::from_pem(pem.as_bytes())
            .map_err(|e| EngineError::Connect(format!("engine cert: {e}")))?;
        b = b.add_root_cert(cert);
    }
    b.build().map_err(|e| EngineError::Connect(e.to_string()))
}
```
Replace the inline `reqwest::Client::builder().timeout(...).build()...` in `execute` with `let client = self.build_client(limits.timeout)?;`. (The `base_url` already uses `http_scheme` = https + port 8443, so the https URL is built; the pinned cert validates it.)

- [ ] **Step 5:** `cargo test -p egret-query --lib` → PASS.
- [ ] **Step 6: fmt + clippy + commit:**
```bash
cargo fmt -p egret-query-api -p egret-query && cargo clippy -p egret-query --lib --tests
git add crates/egret-query-api/src/lib.rs crates/egret-query/src/execution/connector/trino.rs
git commit -m "feat(query): pin engine TLS cert for jwt-mode (reqwest add_root_cert)"
```

---

## Task 7: Materialize the jwt delegated connection

**Files:** Modify `crates/egret-engine-mgmt/src/assignment.rs`

Context: `delegated_conn_options(engine) -> (serde_json::Value, Option<Uuid>)` (from #254) returns the impersonate options + delegator secret, else `({}, None)`. The jwt branch must set https + the pinned cert + no secret. The materialized `create_provisioned(..)` already takes `options` + `secret_id` + `port`; the jwt port is 8443.

- [ ] **Step 1: Failing test** (assignment.rs tests): extend/add a test for `delegated_conn_options`:
```rust
#[test]
fn delegated_conn_options_jwt_uses_https_and_pins_cert() {
    let e = Engine {
        /* full literal */, coordinator_cert: Some("PEMCERT".into()),
        spec: serde_json::json!({ "trinoConfig": { "delegationMode": "jwt" } }),
        /* ... */
    };
    let (opts, secret) = delegated_conn_options(&e);
    assert_eq!(opts["http_scheme"], "https");
    assert_eq!(opts["tls_cert"], "PEMCERT");
    assert!(opts.get("impersonate").is_none());
    assert_eq!(secret, None);
}
```

- [ ] **Step 2:** `cargo test -p egret-engine-mgmt --lib delegated_conn_options_jwt` → FAIL.
- [ ] **Step 3: Implement.** Extend `delegated_conn_options`:
```rust
fn delegated_conn_options(engine: &Engine) -> (serde_json::Value, Option<Uuid>) {
    if engine.is_impersonate_delegation() {
        // ... existing impersonate branch (user + impersonate:true + delegator secret) ...
    } else if engine.is_jwt_delegation() {
        let mut opts = serde_json::json!({ "http_scheme": "https" });
        if let Some(cert) = &engine.coordinator_cert {
            opts["tls_cert"] = serde_json::Value::String(cert.clone());
        }
        (opts, None)
    } else {
        (serde_json::json!({}), None)
    }
}
```
In the `assign` call site (assignment.rs), the materialized `port` must be **8443** for a jwt engine (https) and the coordinator port otherwise. Where it currently passes `engine.coordinator_port()`, use `let port = if engine.is_jwt_delegation() { 8443 } else { engine.coordinator_port() };` and pass `port`.

- [ ] **Step 4:** `cargo test -p egret-engine-mgmt --lib` → PASS.
- [ ] **Step 5: fmt + clippy + commit:**
```bash
cargo fmt -p egret-engine-mgmt && cargo clippy -p egret-engine-mgmt --lib --tests
git add crates/egret-engine-mgmt/src/assignment.rs
git commit -m "feat(engine): materialize jwt delegated connection (https + pinned cert, 8443)"
```

---

## Final verification

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets` (no new warnings in touched crates)
- [ ] `cargo test -p egret-engine-mgmt -p egret-orchestrator -p egret-query -p egret-query-api`
- [ ] `cargo build -p egret-bin`
- [ ] **Live (manual):** provision an engine with `delegationMode="jwt"` (update its spec `trinoConfig.delegationMode="jwt"`); confirm the coordinator boots HTTPS on 8443 with the egret cert, has NO `egret_delegator` in password.db, and NO impersonate policy. Assign to a workspace, and **as an OIDC user** (e.g. member-sso, with a stored refresh token) run a delegated query → runs as that user over TLS (`SELECT current_user` = the caller). A non-OIDC caller (local admin, no refresh token) gets 403. (Dev host-vs-cluster: the cert's SAN includes `localhost`, so a `kubectl port-forward` + repointed DB host validates.)

## Notes for the executor
- **jwt = per-engine exclusive, JWT-only:** never seed `egret_delegator` / impersonate policy / PASSWORD auth for a jwt engine.
- **Do not change `routes.rs`** — the `!impersonate ⇒ mint JWT (403 on NoRefreshToken)` branch already IS the jwt path; `tls_cert` flows via `options`.
- **`http_scheme=https` + `add_root_cert`** is the only connector change; the #255 `X-Forwarded-Proto`/`rebase` behavior is harmless over real TLS.
- **Migration staleness:** touching `egret-metastore/src/lib.rs` (Task 1) is required.
