# Trino Auth + Ingress Exposure (Phase 1.6a) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Secure a provisioned Trino with a file PASSWORD authenticator + a JWT
authenticator, and expose it through a standard Kubernetes Ingress.

**Architecture:** Extend `TrinoConfig` with an `auth` block (bcrypted password users +
JWT) and an `ingress` block; `trino.rs` renders the auth keys + `password.db` +
`password-authenticator.properties` and a `networking.k8s.io/v1` Ingress; the
provisioner applies/deletes the Ingress with the engine. bcrypt happens
control-plane-side (resolve the password secret → hash → hand the orchestrator
`username:hash`). TLS terminates at the ingress; Trino stays HTTP internally
(`allow-insecure-over-http` + `process-forwarded`). No egret query proxy.

**Tech Stack:** Rust (serde_json, `bcrypt` crate), kube-rs `DynamicObject`, axum
control-plane, React/TS. Trino file password + JWT authenticators.

**Spec:** `docs/en/specs/2026-07-04-trino-auth-ingress-design.md`.
**Chart reference:** `/Users/1pro-l4r8/Documents/github/data-portal/trino-charts/charts/trino`.

---

## File Structure

- **Modify** `crates/egret-orchestrator/src/spec.rs` — `Auth`, `PasswordEntry`, `Jwt`, `Ingress` structs + fields on `TrinoConfig`.
- **Modify** `crates/egret-orchestrator/src/trino.rs` — auth config.properties + password.db + password-authenticator.properties + mounts; `trino_ingress()` + provisioner apply/delete.
- **Modify** `crates/egret-controlplane/src/engines.rs` — carriers, bcrypt password users, JWKS default, wiring; `bcrypt` dep.
- **Modify** `crates/egret-bin/src/main.rs` — map auth/ingress carriers → orchestrator.
- **Modify** `frontend/src/api/controlPlane.ts` + `EngineDetail.tsx` — types + Exposure & Auth card.

---

## Task 1: Orchestrator types

**Files:** Modify `crates/egret-orchestrator/src/spec.rs`, `lib.rs`

- [ ] **Step 1** Add:

```rust
#[derive(Clone, Debug, Default)]
pub struct PasswordEntry { pub username: String, pub bcrypt_hash: String }
#[derive(Clone, Debug, Default)]
pub struct Jwt { pub jwks_uri: String, pub required_audience: Option<String> }
#[derive(Clone, Debug, Default)]
pub struct Auth { pub password_users: Vec<PasswordEntry>, pub jwt: Option<Jwt> }
#[derive(Clone, Debug, Default)]
pub struct Ingress {
    pub enabled: bool,
    pub class_name: Option<String>,
    pub host: String,
    pub tls_secret: Option<String>,
    pub annotations: std::collections::BTreeMap<String, String>,
}
```

Add to `TrinoConfig`: `pub auth: Option<Auth>,` and `pub ingress: Option<Ingress>,`.

- [ ] **Step 2** Re-export `Auth, PasswordEntry, Jwt, Ingress` from `lib.rs`.
- [ ] **Step 3** `cargo build -p egret-orchestrator` (fully compiles — additive; trino.rs unaffected). `cargo fmt --all`.
- [ ] **Step 4** Commit `refactor(orchestrator): auth + ingress config types`.

---

## Task 2: Orchestrator — auth config.properties + password.db + mounts (TDD)

**Files:** Modify `crates/egret-orchestrator/src/trino.rs`

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn auth_config_password_and_jwt() {
    let mut s = sample_spec();
    s.config.auth = Some(Auth {
        password_users: vec![PasswordEntry { username: "bob".into(), bcrypt_hash: "$2b$10$abc".into() }],
        jwt: Some(Jwt { jwks_uri: "https://kc/realms/egret/protocol/openid-connect/certs".into(), required_audience: Some("trino".into()) }),
    });
    let cm = trino_config_map(&s);
    let cc = cm["data"]["config-coordinator.properties"].as_str().unwrap();
    assert!(cc.contains("http-server.authentication.type=PASSWORD,JWT"));
    assert!(cc.contains("http-server.authentication.allow-insecure-over-http=true"));
    assert!(cc.contains("http-server.process-forwarded=true"));
    assert!(cc.contains("http-server.authentication.jwt.key-file=https://kc/realms/egret/protocol/openid-connect/certs"));
    assert!(cc.contains("http-server.authentication.jwt.required-audience=trino"));
    // workers don't authenticate clients
    assert!(!cm["data"]["config-worker.properties"].as_str().unwrap().contains("authentication.type"));
    assert!(cm["data"]["password.db"].as_str().unwrap().contains("bob:$2b$10$abc"));
    assert!(cm["data"]["password-authenticator.properties"].as_str().unwrap().contains("password-authenticator.name=file"));
}
#[test]
fn auth_type_password_only() {
    let mut s = sample_spec();
    s.config.auth = Some(Auth { password_users: vec![PasswordEntry { username: "a".into(), bcrypt_hash: "h".into() }], jwt: None });
    let cc = trino_config_map(&s)["data"]["config-coordinator.properties"].as_str().unwrap().to_string();
    assert!(cc.contains("http-server.authentication.type=PASSWORD"));
    assert!(!cc.contains("JWT"));
}
```

- [ ] **Step 2** Run → FAIL.

- [ ] **Step 3: Implement**

Add a helper + wire into `config_properties` (coordinator only) + `trino_config_map`:

```rust
fn auth_type(auth: &crate::spec::Auth) -> Option<String> {
    let mut parts = vec![];
    if !auth.password_users.is_empty() { parts.push("PASSWORD"); }
    if auth.jwt.is_some() { parts.push("JWT"); }
    if parts.is_empty() { None } else { Some(parts.join(",")) }
}
```

In `config_properties(spec, coord)` — only when `coord` and `spec.config.auth` is Some with a derived type:

```rust
    if coord {
        if let Some(auth) = &cfg.auth {
            if let Some(t) = auth_type(auth) {
                out.push_str(&format!("http-server.authentication.type={t}\n"));
                out.push_str("http-server.authentication.allow-insecure-over-http=true\n");
                out.push_str("http-server.process-forwarded=true\n");
                if let Some(j) = &auth.jwt {
                    out.push_str(&format!("http-server.authentication.jwt.key-file={}\n", j.jwks_uri));
                    if let Some(a) = &j.required_audience { out.push_str(&format!("http-server.authentication.jwt.required-audience={a}\n")); }
                }
            }
        }
    }
```

In `trino_config_map`, when `auth` has password users:

```rust
    if let Some(auth) = &spec.config.auth {
        if !auth.password_users.is_empty() {
            let db: String = auth.password_users.iter().map(|u| format!("{}:{}\n", u.username, u.bcrypt_hash)).collect();
            data.insert("password.db".into(), json!(db));
            data.insert("password-authenticator.properties".into(), json!(
                "password-authenticator.name=file\nfile.password-file=/etc/trino/auth/password/password.db\n"));
        }
    }
```

- [ ] **Step 4: Mounts** — in `trino_deployment`, when `auth.password_users` non-empty, add subPath mounts: `password-authenticator.properties` → `/etc/trino/password-authenticator.properties`, `password.db` → `/etc/trino/auth/password/password.db`. Add a test asserting the mounts appear when set + absent when unset.

- [ ] **Step 5** Run `cargo test -p egret-orchestrator` + `cargo clippy -p egret-orchestrator` → pass/clean. `cargo fmt --all`. Commit `feat(orchestrator): Trino PASSWORD + JWT auth config + password.db`.

---

## Task 3: Orchestrator — Ingress resource + provisioner apply/delete (TDD)

**Files:** Modify `crates/egret-orchestrator/src/trino.rs`

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn ingress_resource_shape() {
    let mut s = sample_spec();
    let mut anns = std::collections::BTreeMap::new();
    anns.insert("nginx.ingress.kubernetes.io/proxy-read-timeout".into(), "300".into());
    s.config.ingress = Some(crate::spec::Ingress {
        enabled: true, class_name: Some("nginx".into()),
        host: "trino-demo.egret.example.com".into(), tls_secret: Some("trino-demo-tls".into()), annotations: anns,
    });
    let ing = trino_ingress(&s).unwrap();
    assert_eq!(ing["apiVersion"], "networking.k8s.io/v1");
    assert_eq!(ing["kind"], "Ingress");
    assert_eq!(ing["metadata"]["name"], "demo-trino");
    assert_eq!(ing["metadata"]["annotations"]["nginx.ingress.kubernetes.io/proxy-read-timeout"], "300");
    assert_eq!(ing["spec"]["ingressClassName"], "nginx");
    assert_eq!(ing["spec"]["rules"][0]["host"], "trino-demo.egret.example.com");
    assert_eq!(ing["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"]["name"], "demo-trino");
    assert_eq!(ing["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"]["port"]["number"], 8080);
    assert_eq!(ing["spec"]["tls"][0]["secretName"], "trino-demo-tls");
}
#[test]
fn ingress_none_when_disabled() {
    let s = sample_spec(); // no ingress
    assert!(trino_ingress(&s).is_none());
}
```

- [ ] **Step 2** Run → FAIL.

- [ ] **Step 3: Implement `trino_ingress`**

```rust
pub fn trino_ingress(spec: &EngineSpec) -> Option<Value> {
    let ing = spec.config.ingress.as_ref()?;
    if !ing.enabled { return None; }
    let mut meta = json!({ "name": format!("{}-trino", spec.name), "namespace": spec.namespace,
        "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } });
    if !ing.annotations.is_empty() {
        meta["annotations"] = Value::Object(ing.annotations.iter().map(|(k, v)| (k.clone(), json!(v))).collect());
    }
    let mut spec_obj = json!({
        "rules": [{ "host": ing.host, "http": { "paths": [{
            "path": "/", "pathType": "Prefix",
            "backend": { "service": { "name": format!("{}-trino", spec.name), "port": { "number": 8080 } } }
        }]}}]
    });
    if let Some(cn) = &ing.class_name { spec_obj["ingressClassName"] = json!(cn); }
    if let Some(ts) = &ing.tls_secret { spec_obj["tls"] = json!([{ "hosts": [ing.host], "secretName": ts }]); }
    Some(json!({ "apiVersion": "networking.k8s.io/v1", "kind": "Ingress", "metadata": meta, "spec": spec_obj }))
}
```

- [ ] **Step 4: Provisioner apply/delete** — in `TrinoProvisioner::apply_all`, after applying the base objects, `if let Some(ing) = trino_ingress(spec) { self.kube.apply(&ing).await?; }`. In `delete`, always attempt `self.kube.delete("networking.k8s.io/v1", "Ingress", namespace, &format!("{name}-trino")).await` (best-effort, harmless if absent). Because `update` re-runs `apply_all` and the Ingress is only applied when enabled, a DISABLE won't remove a previously-applied Ingress — so in `update`, when `spec.config.ingress` is None or not enabled, ALSO best-effort delete the Ingress. Add an async test: `provision` with `ingress.enabled` applies `Ingress/demo-trino`; `delete` deletes it.

- [ ] **Step 5** Run tests + clippy + fmt. Commit `feat(orchestrator): Trino Ingress resource + provisioner apply/delete`.

---

## Task 4: Control-plane — carriers + bcrypt + JWKS default

**Files:** Modify `crates/egret-controlplane/src/engines.rs`, `Cargo.toml`; Test `tests/routes_api.rs`

- [ ] **Step 1** Add `bcrypt = "0.15"` (or the workspace-resolved version) to `crates/egret-controlplane/Cargo.toml`.
- [ ] **Step 2** Carriers: `AuthReq { password_users: Vec<PasswordEntryReq{username, bcrypt_hash}>, jwt: Option<JwtReq{jwks_uri, required_audience}> }`, `IngressReq { enabled, class_name, host, tls_secret, annotations }`. Add `auth: Option<AuthReq>`, `ingress: Option<IngressReq>` to `TrinoConfigReq`. Export from `lib.rs`.
- [ ] **Step 3** Wire shape parsing: the wire `trinoConfig.auth` is `{ passwordUsers: [{username, secretRef}], jwt?: {jwksUri, requiredAudience?} }` and `ingress` is `{ enabled, className?, host, tlsSecret?, annotations? }`. Parse them in `trino_config_from_json` into a RAW form (password users hold `secretRef`, not a hash). Then in `build_provision_req`, resolve + bcrypt: for each password user, `st.secrets.resolve_plaintext(None, secretRef)` → `bcrypt::hash(pw, bcrypt::DEFAULT_COST)` → `PasswordEntryReq { username, bcrypt_hash }`; 400 if a secret is missing. For JWT, if `jwksUri` is empty, default to a config value `st.engine_jwks_uri` (add a `String` field to `CpState`, populated in egret-bin from config — see Task 5; tests set a dummy). Build `AuthReq`/`IngressReq` for the provisioning request.
- [ ] **Step 4** `create_engine`/`update_engine` persist `auth` + `ingress` in the spec jsonb (update already persists the whole `trinoConfig`; on create they default to None). Store the RAW auth (with `secretRef`), not the hash — the hash is re-derived at `build_provision_req` time so rotating the secret takes effect.
- [ ] **Step 5** Route test: `PUT` a `trinoConfig` with `auth.passwordUsers=[{username:"bob", secretRef:<a seeded secret id>}]` + `ingress.enabled` → 200; `GET` reflects the auth/ingress (usernames + host, NOT the plaintext); a `PUT` with a missing secretRef → 400. `DATABASE_URL=... cargo test -p egret-controlplane` only.
- [ ] **Step 6** `cargo fmt --all`; commit `feat(controlplane): Trino auth (bcrypt) + ingress carriers`. (egret-bin deferred to Task 5.)

---

## Task 5: egret-bin adapter + JWKS config

**Files:** Modify `crates/egret-bin/src/main.rs`, `crates/egret-config/src/...`

- [ ] **Step 1** Add a config value for the default engine JWKS URI (`auth.engine_jwks_uri: Option<String>`, or reuse the existing OIDC/Keycloak config the `EngineToken` broker uses — grep `engine_broker.rs`/`KeycloakRefresher` for the realm/token URL and derive the `.../protocol/openid-connect/certs` JWKS from it). Populate `CpState.engine_jwks_uri` when constructing `CpState` in `serve`.
- [ ] **Step 2** Extend `spec_from`: map `AuthReq`→`egret_orchestrator::Auth` (`PasswordEntryReq`→`PasswordEntry`, `JwtReq`→`Jwt`) and `IngressReq`→`egret_orchestrator::Ingress` field-for-field.
- [ ] **Step 3** `cargo build --workspace 2>&1 | grep "^error"` → empty. `cargo fmt --all -- --check`. Commit `feat(bin): map auth/ingress to the orchestrator spec + default JWKS`.

---

## Task 6: Frontend — Exposure & Auth card

**Files:** Modify `frontend/src/api/controlPlane.ts`, `EngineDetail.tsx`

- [ ] **Step 1** TS types: `TrinoConfig.auth?: { passwordUsers: { username: string; secretRef: string }[]; jwt?: { jwksUri?: string; requiredAudience?: string } }`, `TrinoConfig.ingress?: { enabled: boolean; className?: string; host: string; tlsSecret?: string; annotations?: Record<string,string> }`. `tsc`.
- [ ] **Step 2** An **Exposure & Auth** card in the Cluster tab:
  - **Password users**: a list — username `<Input>` + a secret `<Select>` (`listSecrets()`) for the password; add/remove rows → `auth.passwordUsers`.
  - **JWT**: a toggle → an optional `jwksUri` `<Input>` (placeholder "default IdP JWKS") + optional `requiredAudience` `<Input>` → `auth.jwt` (undefined when off).
  - **Ingress**: an "Expose via ingress" `Switch` → `host` `<Input>`, `className` `<Input>` (default "nginx"), `tlsSecret` `<Input>` (optional), and an annotations `KeyValueEditor` → `ingress`.
  Prefill from `engine.spec.trinoConfig` (guard undefined); merge into the `updateEngine` `trinoConfig` payload alongside the existing fields. A helper note: "Clients connect to https://{host} with their username+password (or an egret JWT)."
  `tsc`/`eslint`/`build`. Commit `feat(ui): Trino Exposure & Auth (password users, JWT, ingress)`.

---

## Task 7: Full gate + live test + PR

- [ ] **Step 1** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace`.
- [ ] **Step 2** Frontend `tsc` / `eslint` / `vitest run` / `vite build`.
- [ ] **Step 3** Live on kind: create an egret secret holding a password (e.g. "s3cret"); on the engine detail page add a password user `bob` → that secret + enable ingress (host `trino-demo.local`, class nginx); Save → roll. Verify: `kubectl get configmap demo2-config -o yaml` (ANSI-stripped) shows `http-server.authentication.type=PASSWORD` + `password.db` with `bob:$2b$...`; `kubectl get ingress demo2-trino -o yaml` shows the host rule → `demo2-trino:8080`. From inside the cluster: `curl -u bob:s3cret http://<coordinator-pod>:8080/v1/statement -d "SELECT 1"` → 200 (not 401); a wrong password → **401**. (End-to-end ingress routing needs an nginx-ingress controller on kind — document as opt-in; the auth + Ingress-object rendering are the proof.)
- [ ] **Step 4** Commit; open PR; run the full local gate before an admin squash-merge.

---

## Self-Review notes

- **Spec coverage:** auth types (Task 1) ✓; authentication.type PASSWORD/JWT + allow-insecure + process-forwarded + jwt key-file (Task 2) ✓; password-authenticator + password.db + mounts (Task 2) ✓; Ingress resource + apply/delete (Task 3) ✓; control-plane bcrypt + JWKS default + carriers + persist-raw (Task 4) ✓; egret-bin + JWKS config (Task 5) ✓; frontend Exposure & Auth (Task 6) ✓; live verify auth 200/401 + ingress object (Task 7) ✓.
- **Type consistency:** orchestrator `Auth`/`PasswordEntry`/`Jwt`/`Ingress` ↔ control-plane `AuthReq`/`PasswordEntryReq`/`JwtReq`/`IngressReq` ↔ TS ↔ wire. The orchestrator's `PasswordEntry` holds the **bcrypt hash** (control-plane derives it); the wire/stored spec holds `secretRef` (re-hashed each provision so secret rotation applies). This asymmetry is intentional + documented.
- **bcrypt location:** control-plane (`egret-controlplane` gets the `bcrypt` dep); the orchestrator/kube layer never sees plaintext.
- **Ingress disable:** `update` best-effort deletes the Ingress when it's not enabled (Task 3 Step 4) so toggling off removes it.
- **Deferred:** the native monitor page (Phase 1.6b); autoscaling (1.5.4); observability (1.5.5); pod/scheduling (1.5.3).
