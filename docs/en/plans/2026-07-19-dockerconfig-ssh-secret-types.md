# dockerconfigjson + ssh-auth types (+ dockerconfig → imagePullSecrets) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Add `kubernetes.io/dockerconfigjson` + `kubernetes.io/ssh-auth` to the typed-secret registry + console; wire dockerconfigjson to a Trino engine's `imagePullSecrets` (ref → resolve → project `{name}-pull` → append), mirroring P3.

**Architecture:** Two `secret_type.rs` arms; two console editors; a `resolve_dockerconfigjson_secret` resolver + an `image_pull_secret_ref` config field resolved at provision into a projected `{name}-pull` `type: kubernetes.io/dockerconfigjson` Secret appended to `image_pull_secrets`. No migration, no OpenAPI regen.

**Tech Stack:** Rust (egret-secrets, egret-engine-mgmt, egret-orchestrator), React/TS. DB tests: `postgres://egret:egret@localhost:5432/egret_test`.

---

## Design references
- Spec: `docs/en/specs/2026-07-19-dockerconfig-ssh-secret-types-design.md`
- `secret_type.rs` `required_keys` (`egret-secrets`). `resolve_tls_cert_secret` (`engines.rs:1564`) — pattern to mirror.
- `TrinoConfig.image_pull_secrets: Vec<String>` (`egret-engine-mgmt-api/src/lib.rs:349`); parsed at `engines.rs:1165` (`v["imagePullSecrets"]`); rendered at `trino.rs:852` (auto — appending a name suffices).
- P3 render to mirror: `trino_ingress_tls_secret` / the shared `crate::ingress::ingress_tls_secret`.

---

### Task 1: Registry types + validation

**Files:** `crates/egret-secrets/src/secret_type.rs`; test `crates/egret-secrets/tests/store.rs` (or the secret_type unit tests).

- [ ] **Step 1:** Write failing test — `validate("kubernetes.io/dockerconfigjson", {".dockerconfigjson":"x"})` is Ok; missing key → `MissingKey`. Same for `kubernetes.io/ssh-auth` + `ssh-privatekey`. (Find where `secret_type::validate` is unit-tested — likely inline `#[cfg(test)]` in `secret_type.rs` or `tests/store.rs`; add there.)
- [ ] **Step 2:** Run, expect FAIL: `DATABASE_URL=…egret_test cargo test -p egret-secrets dockerconfig` (and ssh).
- [ ] **Step 3:** Implement — add consts + arms:
```rust
pub const DOCKER_CONFIG_JSON: &str = "kubernetes.io/dockerconfigjson";
pub const SSH_AUTH: &str = "kubernetes.io/ssh-auth";
// in required_keys match:
DOCKER_CONFIG_JSON => Some(&[".dockerconfigjson"]),
SSH_AUTH => Some(&["ssh-privatekey"]),
```
- [ ] **Step 4:** Run, expect PASS. `cargo test -p egret-secrets`.
- [ ] **Step 5:** Commit — `feat(secrets): register kubernetes.io/dockerconfigjson + ssh-auth types`

---

### Task 2: Console editors (dockerconfigjson + ssh-auth)

**Files:** `frontend/src/routes/control-plane/ControlPlaneSecrets.tsx`

- [ ] **Step 1:** Add both to the type `Select` (near `"kubernetes.io/tls"`, `"kubernetes.io/basic-auth"`).
- [ ] **Step 2:** dockerconfigjson editor — registry / username / password / email inputs; on submit build the value under key `.dockerconfigjson`:
```ts
const auth = btoa(`${user}:${pass}`);
const dockercfg = JSON.stringify({ auths: { [registry]: { username: user, password: pass, email, auth } } });
// data = { ".dockerconfigjson": dockercfg }
```
- [ ] **Step 3:** ssh-auth editor — a private-key `Textarea` → `data = { "ssh-privatekey": <textarea> }`.
- [ ] **Step 4:** Wire each editor's data into the existing create/update payload path (mirror how the tls / basic-auth editors build `data`). List rendering (type + keys) needs no change (generic).
- [ ] **Step 5:** `cd frontend && npm run typecheck && npm run lint`. Commit — `feat(ui): dockerconfigjson + ssh-auth secret editors`

---

### Task 3: Resolver + config field + provision wiring + validation

**Files:** `crates/egret-engine-mgmt-api/src/lib.rs` (config fields), `crates/egret-engine-mgmt/src/engines.rs` (resolver, parse, provision, validation)

- [ ] **Step 1:** `TrinoConfig` (`lib.rs`, near `image_pull_secrets`): add
```rust
pub image_pull_secret_ref: Option<Uuid>,   // persisted (parsed from config JSON)
#[serde(skip)] pub image_pull_secret_json: Option<String>,  // runtime-injected at provision
```
(If `TrinoConfig` isn't serde-derived — like `Ingress` — drop `#[serde(skip)]`; it's parsed/rendered by hand. Match the struct's actual derives.) Fix any exhaustive `TrinoConfig { .. }` literals to compile (`cargo build -p egret-engine-mgmt-api`).
- [ ] **Step 2:** `engines.rs` parse (near `image_pull_secrets` at ~1165): add `image_pull_secret_ref: v["imagePullSecretRef"].as_str().and_then(|s| s.parse().ok()), image_pull_secret_json: None`.
- [ ] **Step 3:** Resolver — mirror `resolve_tls_cert_secret`:
```rust
async fn resolve_dockerconfigjson_secret(st: &CpState, secret_ref: Uuid) -> Result<String, CpError> {
    let ts = st.secrets.resolve_typed(None, secret_ref).await
        .map_err(|e| CpError::BadRequest(format!("resolve dockerconfigjson secret: {e}")))?;
    if ts.r#type != egret_secrets::secret_type::DOCKER_CONFIG_JSON {
        return Err(CpError::BadRequest(format!("secret is not a {} secret", egret_secrets::secret_type::DOCKER_CONFIG_JSON)));
    }
    ts.data.get(".dockerconfigjson").filter(|v| !v.is_empty()).cloned()
        .ok_or_else(|| CpError::BadRequest("dockerconfigjson secret missing .dockerconfigjson".into()))
}
```
- [ ] **Step 4:** Provision (`build_provision_req`, near the P3 ingress-resolution block): if `config.image_pull_secret_ref` is `Some`, resolve → set `config.image_pull_secret_json = Some(json)` and append the projected name (de-dup):
```rust
if let Some(ref_id) = config.image_pull_secret_ref {
    let json = resolve_dockerconfigjson_secret(st, ref_id).await?;
    config.image_pull_secret_json = Some(json);
    let pull = format!("{}-pull", engine.name);
    if !config.image_pull_secrets.contains(&pull) { config.image_pull_secrets.push(pull); }
}
```
(`config` must be `mut`.)
- [ ] **Step 5:** Validation — in create (shared handler, near the P3 ingress cert-ref check) AND Trino update: if the config carries `imagePullSecretRef`, `resolve_dockerconfigjson_secret(&st, ref_id).await?` (400 on bad). Parse it via the same config parse helper.
- [ ] **Step 6:** `cargo build -p egret-engine-mgmt`. Commit — `feat(engine): resolve+project dockerconfigjson into imagePullSecrets at provision`

---

### Task 4: Orchestrator — render + apply + GC the `{name}-pull` Secret

**Files:** `crates/egret-orchestrator/src/trino.rs` (render fn + apply_all + delete + unit test)

- [ ] **Step 1:** Failing unit test (in the trino `mod tests`, reuse the spec builder): set `spec.config.image_pull_secret_json = Some(r#"{"auths":{}}"#.into())`; assert `trino_pull_secret(&spec)` renders `type == "kubernetes.io/dockerconfigjson"`, name `{name}-pull`, `data[".dockerconfigjson"]` = the json; `None` when the field is `None`.
- [ ] **Step 2:** Run, expect FAIL.
- [ ] **Step 3:** Implement:
```rust
/// Project the resolved dockerconfigjson value into a k8s
/// `type: kubernetes.io/dockerconfigjson` Secret named `{name}-pull`.
pub fn trino_pull_secret(spec: &EngineSpec) -> Option<Value> {
    let json = spec.config.image_pull_secret_json.as_ref()?;
    Some(json!({
        "apiVersion": "v1", "kind": "Secret", "type": "kubernetes.io/dockerconfigjson",
        "metadata": { "name": format!("{}-pull", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "data": { ".dockerconfigjson": base64_encode(json) }
    }))
}
```
Use the crate's existing base64 helper for the `data` value (grep how `trino_env_secret`/other Secrets base64-encode `data`, or use `stringData` instead to avoid manual base64 — prefer `stringData: { ".dockerconfigjson": json }` for simplicity, matching `trino_tls_secret`'s `stringData`).
- [ ] **Step 4:** `apply_all` — apply `trino_pull_secret(spec)` before the Deployments. `delete` — best-effort delete `{name}-pull` Secret.
- [ ] **Step 5:** Run tests. `cargo test -p egret-orchestrator`. Commit — `feat(orchestrator): project + apply + GC the {name}-pull dockerconfigjson Secret`

---

### Task 5: Full gates + live acceptance (controller-run)

- [ ] **Step 1:** `cargo fmt --all --check && cargo clippy --workspace --all-targets && DATABASE_URL=…egret_test cargo test --workspace --no-fail-fast` (green modulo the known PDP hot-reload flake). Frontend `lint`/`typecheck`/`test`/`build`.
- [ ] **Step 2:** Rebuild images + load + rollout.
- [ ] **Step 3:** Live acceptance:
  1. Create a `kubernetes.io/dockerconfigjson` secret via API (`data: { ".dockerconfigjson": "{\"auths\":{\"registry.example.com\":{...}}}" }`).
  2. Provision a Trino engine with `imagePullSecretRef` = that secret.
  3. Verify `kubectl get secret {name}-pull -n egret-engines -o jsonpath='{.type}'` = `kubernetes.io/dockerconfigjson`; the coordinator pod `spec.imagePullSecrets` includes `{name}-pull`.
  4. Delete the engine → `{name}-pull` gone.
  5. Create an `ssh-auth` secret via the console → lists as `kubernetes.io/ssh-auth`, key `ssh-privatekey`.
  6. Clean up.
- [ ] **Step 4:** superpowers:finishing-a-development-branch → PR → `gh pr merge --admin --squash`.

---

## Self-review notes
- **Spec coverage:** types (T1), console (T2), resolver+config+provision+validation (T3), orchestrator render/apply/GC (T4), live proof (T5). Mapped.
- **Type consistency:** `image_pull_secret_ref: Option<Uuid>` (persisted) + `image_pull_secret_json: Option<String>` (runtime); projected Secret `{name}-pull` consistent across render (T4), provision-append (T3), GC (T4), live checks (T5). `DOCKER_CONFIG_JSON`/`SSH_AUTH` consts used in registry (T1), resolver (T3), and console type strings (T2).
- **No deployment change:** `trino.rs:852` already renders `imagePullSecrets` from `config.image_pull_secrets`; appending `{name}-pull` at provision is sufficient.
