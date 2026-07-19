# StarRocks FE ingress TLS from a typed secret — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose a StarRocks engine's FE HTTP endpoint (`{name}-fe-service`:8030) via an Ingress whose TLS is backed by a typed `kubernetes.io/tls` secret, projected into `{name}-ingress-tls` — P3 parity for StarRocks.

**Architecture:** Extract P3's projection into a shared `ingress_tls_secret` helper; add a `starrocks_ingress` render (mirror `trino_ingress`, FE:8030 backend); apply both in the StarRocks `apply_all` and GC them in `delete`; add the P3 ingress-resolution block to `build_starrocks_provision_req`. No migration, no OpenAPI regen (shared `Ingress` config already carries `tls_cert_secret_ref`).

**Tech Stack:** Rust (egret-orchestrator, egret-engine-mgmt). DB-gated tests read `DATABASE_URL` (test DB `postgres://egret:egret@localhost:5432/egret_test`).

---

## Design references

- Spec: `docs/en/specs/2026-07-19-starrocks-fe-ingress-tls-design.md`
- Mirror: `trino_ingress` + `trino_ingress_tls_secret` (`crates/egret-orchestrator/src/trino.rs`). P3 resolution block: `crates/egret-engine-mgmt/src/engines.rs` fn `build_provision_req` (comment "Ingress TLS from a typed secret").
- Orchestrator modules: `lib.rs` declares `pub mod {kube_client, provisioner, spec, starrocks, trino}`.
- StarRocks: `apply_all` (`starrocks.rs:714`, applies configmaps/secrets then `starrocks_cluster_cr`), `delete(namespace,name)` (`starrocks.rs:893`), FE Service `{name}-fe-service` (operator-created), FE HTTP port 8030. `build_starrocks_provision_req` (`engines.rs:2109`).
- Shared `Ingress` struct: `egret_engine_mgmt_api::Ingress { enabled, class_name, host, tls_secret, tls_cert_secret_ref, tls_cert_pem, tls_key_pem, annotations }`.

---

### Task 1: Shared `ingress_tls_secret` projection helper

**Files:**
- Create: `crates/egret-orchestrator/src/ingress.rs`
- Modify: `crates/egret-orchestrator/src/lib.rs` (add `pub mod ingress;`)
- Modify: `crates/egret-orchestrator/src/trino.rs` (`trino_ingress_tls_secret` delegates to the shared fn)

- [ ] **Step 1: Write the shared helper + test.** Create `ingress.rs`:

```rust
//! Shared ingress helpers reused by the Trino and StarRocks provisioners.
use egret_engine_mgmt_api::Ingress;
use serde_json::{json, Value};

/// Project a resolved ingress TLS cert/key into a real k8s
/// `type: kubernetes.io/tls` Secret named `{name}-ingress-tls`. Returns `None`
/// unless the ingress is enabled and carries resolved cert+key PEMs (populated
/// at provision from a typed `kubernetes.io/tls` secret ref).
pub fn ingress_tls_secret(name: &str, namespace: &str, ing: &Ingress) -> Option<Value> {
    if !ing.enabled {
        return None;
    }
    let (crt, key) = (ing.tls_cert_pem.as_ref()?, ing.tls_key_pem.as_ref()?);
    Some(json!({
        "apiVersion": "v1", "kind": "Secret", "type": "kubernetes.io/tls",
        "metadata": { "name": format!("{name}-ingress-tls"), "namespace": namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": name } },
        "stringData": { "tls.crt": crt, "tls.key": key }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renders_typed_tls_secret_with_pems() {
        let ing = Ingress {
            enabled: true, class_name: Some("nginx".into()), host: "h".into(),
            tls_secret: Some("e-ingress-tls".into()), tls_cert_secret_ref: None,
            tls_cert_pem: Some("CERT".into()), tls_key_pem: Some("KEY".into()),
            annotations: Default::default(),
        };
        let s = ingress_tls_secret("e", "ns", &ing).unwrap();
        assert_eq!(s["type"], "kubernetes.io/tls");
        assert_eq!(s["metadata"]["name"], "e-ingress-tls");
        assert_eq!(s["stringData"]["tls.crt"], "CERT");
        let mut off = ing.clone();
        off.tls_cert_pem = None;
        assert!(ingress_tls_secret("e", "ns", &off).is_none());
    }
}
```
Add `pub mod ingress;` to `lib.rs` (near the other `pub mod`s).

- [ ] **Step 2: Refactor `trino_ingress_tls_secret` to delegate** (behavior must stay byte-identical):

```rust
pub fn trino_ingress_tls_secret(spec: &EngineSpec) -> Option<Value> {
    crate::ingress::ingress_tls_secret(&spec.name, &spec.namespace, spec.config.ingress.as_ref()?)
}
```

- [ ] **Step 3: Run tests.** `cargo test -p egret-orchestrator ingress` and `cargo test -p egret-orchestrator ingress_tls_secret_renders` (the P3 Trino test) — both pass, confirming the refactor didn't change Trino output.

- [ ] **Step 4: Commit** — `refactor(orchestrator): shared ingress_tls_secret helper (Trino + StarRocks)`

---

### Task 2: `starrocks_ingress` render

**Files:**
- Modify: `crates/egret-orchestrator/src/starrocks.rs` (add `starrocks_ingress` + unit test)

- [ ] **Step 1: Write the failing test** (in the starrocks `#[cfg(test)] mod tests`, reuse its EngineSpec builder — grep the module for how it builds a spec, e.g. a `sample_spec()`/inline builder):

```rust
#[test]
fn starrocks_ingress_targets_fe_http_with_tls() {
    let mut spec = /* the module's minimal StarRocks EngineSpec */;
    spec.name = "sr1".into();
    spec.namespace = "egret-engines".into();
    spec.config.ingress = Some(egret_engine_mgmt_api::Ingress {
        enabled: true, class_name: Some("nginx".into()), host: "sr1.localtest.me".into(),
        tls_secret: Some("sr1-ingress-tls".into()), tls_cert_secret_ref: None,
        tls_cert_pem: None, tls_key_pem: None, annotations: Default::default(),
    });
    let ing = starrocks_ingress(&spec).unwrap();
    assert_eq!(ing["kind"], "Ingress");
    assert_eq!(ing["metadata"]["name"], "sr1-fe");
    let backend = &ing["spec"]["rules"][0]["http"]["paths"][0]["backend"]["service"];
    assert_eq!(backend["name"], "sr1-fe-service");
    assert_eq!(backend["port"]["number"], 8030);
    assert_eq!(ing["spec"]["tls"][0]["secretName"], "sr1-ingress-tls");
    // disabled -> None
    spec.config.ingress.as_mut().unwrap().enabled = false;
    assert!(starrocks_ingress(&spec).is_none());
}
```

- [ ] **Step 2: Run, expect FAIL** (`starrocks_ingress` missing): `cargo test -p egret-orchestrator starrocks_ingress_targets_fe`

- [ ] **Step 3: Implement** (mirror `trino_ingress`, FE Service + 8030, Ingress name `{name}-fe`):

```rust
/// Render a `networking.k8s.io/v1` Ingress exposing the StarRocks FE HTTP
/// endpoint (`{name}-fe-service` port 8030). Returns `None` unless the ingress
/// is enabled. TLS terminates at the ingress; the FE stays HTTP internally.
/// (The MySQL query port 9030 is raw TCP and is NOT exposed here.)
pub fn starrocks_ingress(spec: &EngineSpec) -> Option<Value> {
    let ing = spec.config.ingress.as_ref()?;
    if !ing.enabled {
        return None;
    }
    let mut meta = json!({
        "name": format!("{}-fe", spec.name), "namespace": spec.namespace,
        "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name }
    });
    if !ing.annotations.is_empty() {
        meta["annotations"] = Value::Object(
            ing.annotations.iter().map(|(k, v)| (k.clone(), json!(v))).collect(),
        );
    }
    let mut spec_obj = json!({
        "rules": [{ "host": ing.host, "http": { "paths": [{
            "path": "/", "pathType": "Prefix",
            "backend": { "service": { "name": format!("{}-fe-service", spec.name), "port": { "number": 8030 } } }
        }]}}]
    });
    if let Some(cn) = &ing.class_name {
        spec_obj["ingressClassName"] = json!(cn);
    }
    if let Some(ts) = &ing.tls_secret {
        spec_obj["tls"] = json!([{ "hosts": [ing.host], "secretName": ts }]);
    }
    Some(json!({
        "apiVersion": "networking.k8s.io/v1", "kind": "Ingress",
        "metadata": meta, "spec": spec_obj
    }))
}
```
Confirm `json`, `Value` are already imported in `starrocks.rs` (they are — it renders JSON everywhere).

- [ ] **Step 4: Run, expect PASS.** `cargo test -p egret-orchestrator starrocks_ingress_targets_fe`
- [ ] **Step 5: Commit** — `feat(orchestrator): starrocks_ingress renders the FE HTTP ingress`

---

### Task 3: Wire into StarRocks `apply_all` + `delete`

**Files:**
- Modify: `crates/egret-orchestrator/src/starrocks.rs` (`apply_all` + `delete`)

- [ ] **Step 1: `apply_all`** (`starrocks.rs:714`) — before the final `Ok(())` (after `starrocks_cluster_cr`), apply the projected Secret then the Ingress:

```rust
if let Some(ing) = spec.config.ingress.as_ref() {
    if let Some(sec) = crate::ingress::ingress_tls_secret(&spec.name, &spec.namespace, ing) {
        self.kube.apply(&sec).await?;
    }
}
if let Some(ing) = starrocks_ingress(spec) {
    self.kube.apply(&ing).await?;
}
```

- [ ] **Step 2: `delete`** (`starrocks.rs:893`) — add best-effort deletes alongside the others:

```rust
let _ = self.kube.delete("networking.k8s.io/v1", "Ingress", namespace, &format!("{name}-fe")).await;
let _ = self.kube.delete("v1", "Secret", namespace, &format!("{name}-ingress-tls")).await;
```

- [ ] **Step 3: Build + test.** `cargo build -p egret-orchestrator` (0 errors) and `cargo test -p egret-orchestrator` (all pass).
- [ ] **Step 4: Commit** — `feat(orchestrator): apply + GC the StarRocks FE ingress & tls secret`

---

### Task 4: `build_starrocks_provision_req` — ingress resolution

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (`build_starrocks_provision_req`, ~2109)

- [ ] **Step 1: Add the resolution block.** In `build_starrocks_provision_req`, after the StarRocks `config` (the JSON→EngineSpec config) is built and near the `ensure_tls_cert` call (~2235), insert the same block P3 added to the Trino builder — adapted to StarRocks' local variable names (the config binding must be `mut`):

```rust
// Ingress TLS from a typed secret (FE HTTP ingress): resolve the referenced
// kubernetes.io/tls secret, project it into `{name}-ingress-tls`, and point the
// Ingress at it. Typed ref wins over a raw `tls_secret` string when both are set.
if let Some(ing) = config.ingress.as_mut() {
    if let Some(ref_id) = ing.tls_cert_secret_ref {
        let (cert_pem, key_pem) = resolve_tls_cert_secret(st, ref_id).await?;
        ing.tls_cert_pem = Some(cert_pem);
        ing.tls_key_pem = Some(key_pem);
        ing.tls_secret = Some(format!("{}-ingress-tls", engine.name));
    }
}
```
Adapt `config` / `engine.name` / `st` to the actual identifiers in `build_starrocks_provision_req`. If the ingress lives on a differently-named local spec/config there, match it. Make the binding `mut` if needed.

- [ ] **Step 2: Build.** `cargo build -p egret-engine-mgmt` (0 errors).
- [ ] **Step 3: Commit** — `feat(engine): resolve+project StarRocks FE ingress tls_cert_secret_ref at provision`

---

### Task 5: Verify validation + UI parity (likely no code)

**Files:** investigation; code only if a gap is found.

- [ ] **Step 1: Validation.** Confirm the engine create/update handlers validate the ingress `tls_cert_secret_ref` regardless of kind (P3 added a `parse_ingress`-based check). Grep `engines.rs` for the P3 ingress validation; confirm it runs for a StarRocks create/update body (the config wire object carries `ingress` the same way). If StarRocks parses its config through a different path that skips the check, add the same `resolve_tls_cert_secret` validation there. Report findings.
- [ ] **Step 2: UI.** Confirm `EngineSecurity.tsx`'s ingress picker (the `es-ingress-tls-cert` Select) is rendered for StarRocks engines (check how EngineSecurity/EngineDetail decide which sections to show per kind). If StarRocks hides the ingress section, expose it. Report findings.
- [ ] **Step 3:** If either needed a change, commit it: `feat(engine|ui): StarRocks parity for ingress tls_cert_secret_ref (validation|picker)`. Otherwise note "verified, no code needed".

---

### Task 6: Full gates + live acceptance on kind (controller-run)

**Files:** none (verification)

- [ ] **Step 1: Full gates.** `cargo fmt --all --check && cargo clippy --workspace --all-targets && DATABASE_URL=…egret_test cargo test --workspace --no-fail-fast` (expect fully green now — the assignment tests were fixed). Frontend `npm run lint && npm run typecheck && npm run test && npm run build`.
- [ ] **Step 2: Rebuild images + load + rollout** (P2/P3 flow).
- [ ] **Step 3: Live acceptance.**
  1. Generate a leaf cert with SAN = the FE ingress host (e.g. `sr-fe.localtest.me`).
  2. Create a `kubernetes.io/tls` secret from it.
  3. Provision a StarRocks engine (shared-data, small) with ingress enabled + `host = sr-fe.localtest.me` + `tlsCertSecretRef` = that secret.
  4. Verify: `kubectl get secret {name}-ingress-tls -n egret-engines -o jsonpath='{.type}'` = `kubernetes.io/tls` and `tls.crt` == provided; the Ingress `{name}-fe` has `spec.tls[].secretName`.
  5. `curl -k --resolve sr-fe.localtest.me:443:127.0.0.1 https://sr-fe.localtest.me/` through nginx serves the provided cert (fingerprint match); the FE HTTP responds (any 2xx/3xx/4xx from the FE, not a 502).
  6. Delete the engine → `{name}-ingress-tls` + `{name}-fe` Ingress are gone.
  7. Clean up the test secret.
- [ ] **Step 4:** Use superpowers:finishing-a-development-branch → open PR → (`gh pr merge --admin --squash`).

---

## Self-review notes

- **Spec coverage:** shared helper (Task 1), starrocks_ingress FE:8030 (Task 2), apply+GC (Task 3), provision resolution (Task 4), validation+UI parity (Task 5), live proof incl. serve-cert + GC (Task 6). All mapped.
- **Type consistency:** `ingress_tls_secret(name, ns, &Ingress)` used by both `trino_ingress_tls_secret` and the StarRocks `apply_all`; Ingress object name `{name}-fe`; projected Secret `{name}-ingress-tls`; FE backend `{name}-fe-service`:8030 consistent across render (Task 2), apply/GC (Task 3), and live checks (Task 6).
- **Refactor safety:** the P3 Trino render test (`ingress_tls_secret_renders_typed_kubernetes_io_tls`) must still pass after Task 1, guarding byte-identical Trino output.
- **No migration / no OpenAPI regen:** the shared `Ingress` config already carries `tls_cert_secret_ref`; ingress config is opaque JSON; FE reuses the existing `/secrets` list.
