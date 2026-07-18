# Typed Secrets P3 — Trino ingress TLS from a typed secret — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Trino engine's coordinator Ingress can reference a typed `kubernetes.io/tls` secret (picked in the UI); egret projects it into a real k8s `type: kubernetes.io/tls` Secret (`{name}-ingress-tls`) at provision and the Ingress references it. Backward-compatible with the raw `tlsSecret` string.

**Architecture:** Stored engine config carries a `tlsCertSecretRef` (Uuid). At provision, `engines.rs` resolves it (reusing P2's `resolve_tls_cert_secret`), injects the resolved cert/key PEMs into `EngineSpec.config.ingress`, and sets `tls_secret = "{name}-ingress-tls"`. The orchestrator renders + applies the `{name}-ingress-tls` Secret before the Ingress (mirroring `trino_tls_secret`). The provisioner delete path is extended to GC both `{name}-ingress-tls` and the previously-orphaned `{name}-tls`.

**Tech Stack:** Rust (egret-engine-mgmt, egret-engine-mgmt-api, egret-orchestrator), React/TS (frontend). No DB migration (ingress lives in the engine `config` JSON blob). No OpenAPI regen (engine config is opaque JSON; the FE reuses the existing `/secrets` list).

---

## Design references

- Spec: `docs/en/specs/2026-07-18-typed-secrets-p3-trino-ingress-tls-design.md`
- P2 primitive reused: `resolve_tls_cert_secret` at `crates/egret-engine-mgmt/src/engines.rs:1549`.
- Render pattern to mirror: `trino_tls_secret` at `crates/egret-orchestrator/src/trino.rs` (renders `{name}-tls` from `spec.config.tls.keystore_pem`).
- Ingress render: `trino_ingress` (`trino.rs:982`) already sets `spec.tls[].secretName = ing.tls_secret`.
- `apply_all` (`trino.rs:~1140`) applies `trino_tls_secret` before the Service and `trino_ingress` after the Deployments.
- Provisioner `delete(namespace, name)` (`trino.rs`) best-effort deletes engine objects by name — currently does NOT delete `{name}-tls` (the orphan bug) or any ingress Secret.
- `parse_ingress` (`engines.rs:1180`) + `Ingress` struct (`egret-engine-mgmt-api/src/lib.rs:253`, aliased `IngressReq`).
- Provision secret-resolution anchor: the `if tls_enabled { ensure_tls_cert(...) }` block (`engines.rs:~2207`).
- UI: `frontend/src/routes/control-plane/EngineSecurity.tsx` (`ingressTlsSecret` free-text) + its form hook `useEngineConfigForm.ts`.

---

### Task 1: `Ingress` struct + `parse_ingress` carry `tls_cert_secret_ref`

**Files:**
- Modify: `crates/egret-engine-mgmt-api/src/lib.rs:253` (the `Ingress` struct)
- Modify: `crates/egret-engine-mgmt/src/engines.rs:1180` (`parse_ingress`)

- [ ] **Step 1: Add fields to the `Ingress` struct.** The persisted `tls_cert_secret_ref` plus two runtime-only PEM fields the provision step injects (never serialized into stored config). Ensure `uuid::Uuid` is in scope in this file (it already uses `Uuid` elsewhere — check imports; add `use uuid::Uuid;` only if missing).

```rust
pub struct Ingress {
    pub enabled: bool,
    pub class_name: Option<String>,
    pub host: String,
    pub tls_secret: Option<String>,
    /// Typed `kubernetes.io/tls` secret backing the ingress TLS. When set, the
    /// provision step resolves it, projects a `{name}-ingress-tls` k8s Secret,
    /// and points `tls_secret` at that projected name.
    pub tls_cert_secret_ref: Option<Uuid>,
    /// Resolved cert/key PEMs, injected at provision (NOT persisted). The
    /// orchestrator renders the `{name}-ingress-tls` Secret from these.
    #[serde(skip)]
    pub tls_cert_pem: Option<String>,
    #[serde(skip)]
    pub tls_key_pem: Option<String>,
    pub annotations: BTreeMap<String, String>,
}
```

- [ ] **Step 2: `parse_ingress` reads `tlsCertSecretRef`.** In `crates/egret-engine-mgmt/src/engines.rs:1180`, extend the constructed `IngressReq`:

```rust
Some(IngressReq {
    enabled: v["enabled"].as_bool().unwrap_or(false),
    class_name: v["className"].as_str().map(Into::into),
    host: v["host"].as_str().unwrap_or_default().to_string(),
    tls_secret: v["tlsSecret"].as_str().map(Into::into),
    tls_cert_secret_ref: v["tlsCertSecretRef"].as_str().and_then(|s| s.parse().ok()),
    tls_cert_pem: None,
    tls_key_pem: None,
    annotations: str_map(&v["annotations"]),
})
```

- [ ] **Step 3: Build the workspace.** Run: `cargo build -p egret-engine-mgmt-api -p egret-engine-mgmt`
  Expected: compiles (other `Ingress {}` literal constructors — if any — may need the new fields; fix each with `..Default::default()` if the struct derives `Default`, else add `tls_cert_secret_ref: None, tls_cert_pem: None, tls_key_pem: None`).

- [ ] **Step 4: Commit.**

```bash
git add crates/egret-engine-mgmt-api/src/lib.rs crates/egret-engine-mgmt/src/engines.rs
git commit -m "feat(engine): Ingress carries tls_cert_secret_ref + resolved PEM fields"
```

---

### Task 2: Orchestrator renders `{name}-ingress-tls` + applies it before the Ingress

**Files:**
- Modify: `crates/egret-orchestrator/src/trino.rs` (add `trino_ingress_tls_secret`, wire into `apply_all`)
- Test: `crates/egret-orchestrator/src/trino.rs` (unit tests in the existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing test.** Add to the trino tests module. It builds an `EngineSpec` with an ingress carrying resolved PEMs and asserts the rendered Secret.

```rust
#[test]
fn ingress_tls_secret_renders_typed_kubernetes_io_tls() {
    let mut spec = sample_spec(); // existing test helper that builds a minimal EngineSpec
    spec.name = "eng1".into();
    spec.namespace = "egret-engines".into();
    spec.config.ingress = Some(egret_engine_mgmt_api::Ingress {
        enabled: true,
        class_name: Some("nginx".into()),
        host: "trino-eng1.localtest.me".into(),
        tls_secret: Some("eng1-ingress-tls".into()),
        tls_cert_secret_ref: None,
        tls_cert_pem: Some("CERTPEM".into()),
        tls_key_pem: Some("KEYPEM".into()),
        annotations: Default::default(),
    });
    let sec = trino_ingress_tls_secret(&spec).expect("secret rendered");
    assert_eq!(sec["type"], "kubernetes.io/tls");
    assert_eq!(sec["metadata"]["name"], "eng1-ingress-tls");
    assert_eq!(sec["metadata"]["namespace"], "egret-engines");
    assert_eq!(sec["metadata"]["labels"]["app.kubernetes.io/managed-by"], "egret");
    assert_eq!(sec["stringData"]["tls.crt"], "CERTPEM");
    assert_eq!(sec["stringData"]["tls.key"], "KEYPEM");
    // No PEMs -> None
    spec.config.ingress.as_mut().unwrap().tls_cert_pem = None;
    assert!(trino_ingress_tls_secret(&spec).is_none());
}
```

If `sample_spec()` does not exist, use whatever minimal-`EngineSpec` constructor the neighboring tests already use (grep the tests module for how they build an `EngineSpec`).

- [ ] **Step 2: Run test to verify it fails.** Run: `cargo test -p egret-orchestrator ingress_tls_secret_renders -- --nocapture`
  Expected: FAIL — `trino_ingress_tls_secret` not found.

- [ ] **Step 3: Implement the render fn.** Add next to `trino_tls_secret`:

```rust
/// Project the resolved ingress TLS cert/key into a real k8s
/// `type: kubernetes.io/tls` Secret named `{name}-ingress-tls`. Returns `None`
/// unless the ingress is enabled and carries resolved cert+key PEMs (populated
/// at provision from a typed `kubernetes.io/tls` secret ref).
pub fn trino_ingress_tls_secret(spec: &EngineSpec) -> Option<Value> {
    let ing = spec.config.ingress.as_ref()?;
    if !ing.enabled {
        return None;
    }
    let (crt, key) = (ing.tls_cert_pem.as_ref()?, ing.tls_key_pem.as_ref()?);
    Some(json!({
        "apiVersion": "v1", "kind": "Secret", "type": "kubernetes.io/tls",
        "metadata": { "name": format!("{}-ingress-tls", spec.name), "namespace": spec.namespace,
                      "labels": { "app.kubernetes.io/managed-by": "egret", "egret/engine": spec.name } },
        "stringData": { "tls.crt": crt, "tls.key": key }
    }))
}
```

- [ ] **Step 4: Wire into `apply_all`.** In `apply_all`, immediately before the `if let Some(ing) = trino_ingress(spec)` block, add:

```rust
if let Some(isec) = trino_ingress_tls_secret(spec) {
    self.kube.apply(&isec).await?;
}
```

- [ ] **Step 5: Run test to verify it passes.** Run: `cargo test -p egret-orchestrator ingress_tls_secret_renders`
  Expected: PASS.

- [ ] **Step 6: Commit.**

```bash
git add crates/egret-orchestrator/src/trino.rs
git commit -m "feat(orchestrator): project {name}-ingress-tls k8s Secret + apply before Ingress"
```

---

### Task 3: Provision resolves the ref → injects PEMs + points `tls_secret` at the projected Secret

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (the provision path, near the `ensure_tls_cert` anchor `~2207`)

- [ ] **Step 1: Add a provision-time resolution block.** Locate where the provision builds the `EngineSpec` that is handed to the provisioner (the same `EngineSpec` whose `config.ingress` came from the stored config via `parse_ingress`). Immediately after that spec is assembled (and after the `if tls_enabled { ensure_tls_cert }` block, which is the neighboring provision-time secret resolution), insert:

```rust
// Ingress TLS from a typed secret: resolve the referenced kubernetes.io/tls
// secret, project it into `{name}-ingress-tls`, and point the Ingress at it.
// Typed ref wins over a raw `tls_secret` string when both are set.
if let Some(ing) = spec.config.ingress.as_mut() {
    if let Some(ref_id) = ing.tls_cert_secret_ref {
        let (cert_pem, key_pem) = resolve_tls_cert_secret(st, ref_id).await?;
        ing.tls_cert_pem = Some(cert_pem);
        ing.tls_key_pem = Some(key_pem);
        ing.tls_secret = Some(format!("{}-ingress-tls", engine.name));
    }
}
```

Adapt `spec` / `engine.name` to the exact identifiers in scope at that point (the spec variable handed to `provision`/`update`, and the engine name). If `config.ingress` is not `mut`-accessible on the spec there, make the local spec binding `mut`.

- [ ] **Step 2: Build.** Run: `cargo build -p egret-engine-mgmt`
  Expected: compiles.

- [ ] **Step 3: Commit.**

```bash
git add crates/egret-engine-mgmt/src/engines.rs
git commit -m "feat(engine): resolve+project ingress tls_cert_secret_ref at provision"
```

---

### Task 4: Validate the ref at engine create/update (400 on a bad secret)

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (create + update handlers, where P2 validates `tls_cert_secret_ref`)
- Test: `crates/egret-engine-mgmt/src/engines.rs` (DB-gated tests module)

- [ ] **Step 1: Write the failing test.** Mirror the P2 validation tests (`resolve_tls_cert_secret_rejects_*`). Add a test that a create/update body with an ingress `tlsCertSecretRef` pointing at a non-TLS secret is rejected. Reuse the existing test harness that builds a `CpState` + a secret; assert `CpError::BadRequest`.

```rust
#[tokio::test]
async fn create_rejects_ingress_tls_cert_ref_non_tls() {
    // Arrange: a CpState with an Opaque (non-TLS) secret `sid` in the same way
    // the P2 test `create_rejects_non_tls_tls_cert_secret_ref` (or equivalent) does.
    // Build a create body with config.ingress = { enabled:true, host, tlsCertSecretRef: sid }.
    // Act: call the create handler / validation helper.
    // Assert: Err(CpError::BadRequest(_)).
}
```

Match the exact harness the P2 validation test uses (grep for the P2 create-validation test near `resolve_tls_cert_secret` and copy its setup).

- [ ] **Step 2: Run test to verify it fails.** Run: `DATABASE_URL=$EGRET_TEST_DB cargo test -p egret-engine-mgmt create_rejects_ingress_tls_cert_ref -- --nocapture`
  Expected: FAIL (validation not yet added — the ref is ignored).

- [ ] **Step 3: Implement validation.** In the create AND update handlers, where the config/ingress is parsed, after `parse_ingress`, if `ingress.tls_cert_secret_ref` is `Some`, call `resolve_tls_cert_secret(&st, ref_id).await?` (its error already maps to `BadRequest`). Place it alongside the existing P2 coordinator `tls_cert_secret_ref` validation so both are checked before `store.create`/`update`.

```rust
if let Some(ing) = parsed_config.ingress.as_ref() {
    if let Some(ref_id) = ing.tls_cert_secret_ref {
        resolve_tls_cert_secret(&st, ref_id).await?; // 400 if missing / not TLS
    }
}
```

- [ ] **Step 4: Run test to verify it passes.** Run: `DATABASE_URL=$EGRET_TEST_DB cargo test -p egret-engine-mgmt create_rejects_ingress_tls_cert_ref`
  Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add crates/egret-engine-mgmt/src/engines.rs
git commit -m "feat(engine): validate ingress tls_cert_secret_ref at create/update"
```

---

### Task 5: GC — delete `{name}-ingress-tls` and `{name}-tls` on engine delete + stale cleanup on update

**Files:**
- Modify: `crates/egret-orchestrator/src/trino.rs` (`delete` impl + `update` stale-cleanup)

- [ ] **Step 1: Extend the provisioner `delete` object set.** In the `async fn delete(&self, namespace, name)` impl, alongside the existing best-effort deletes, add deletes for both TLS Secrets (fixes the orphaned `{name}-tls` observed in P2 acceptance):

```rust
// Best-effort delete the coordinator keystore + ingress TLS Secrets (managed
// by egret; previously orphaned on delete).
let _ = self.kube.delete("v1", "Secret", namespace, &format!("{name}-tls")).await;
let _ = self.kube.delete("v1", "Secret", namespace, &format!("{name}-ingress-tls")).await;
```

- [ ] **Step 2: Stale-cleanup on update.** In `async fn update`, next to the existing best-effort Ingress delete (when ingress is disabled/absent), also delete a stale `{name}-ingress-tls` Secret in the same branch:

```rust
let _ = self.kube.delete("v1", "Secret", &spec.namespace, &format!("{}-ingress-tls", spec.name)).await;
```

(Place it inside the existing `if spec.config.ingress...enabled != Some(true)` block.)

- [ ] **Step 3: Build + run orchestrator tests.** Run: `cargo test -p egret-orchestrator`
  Expected: PASS (no behavior test needed for best-effort deletes; ensure nothing else broke).

- [ ] **Step 4: Commit.**

```bash
git add crates/egret-orchestrator/src/trino.rs
git commit -m "fix(orchestrator): GC {name}-tls + {name}-ingress-tls Secrets on delete/update"
```

---

### Task 6: UI — typed-TLS-secret picker for the ingress cert

**Files:**
- Modify: `frontend/src/routes/control-plane/EngineSecurity.tsx`
- Modify: `frontend/src/routes/control-plane/useEngineConfigForm.ts` (form state + submit wire)

- [ ] **Step 1: Add form state.** In `useEngineConfigForm.ts`, add `ingressTlsCertSecretRef: string` state (default `""`), initialized from the loaded config's `ingress.tlsCertSecretRef`. In the submit/serialize path that builds the `ingress` wire object, add `tlsCertSecretRef: ingressTlsCertSecretRef || undefined` (omit when empty).

- [ ] **Step 2: Replace the free-text input with a picker.** In `EngineSecurity.tsx`, replace the `ingressTlsSecret` text input with a `Select` listing `type === "kubernetes.io/tls"` secrets (fetch via the existing secrets list hook used by the P2 coordinator-cert picker in `ControlPlaneEngines.tsx`), plus a "None" default. Use the same Radix empty-value sentinel pattern as P2 (`__none__` mapped to `""`). Bind to `ingressTlsCertSecretRef` / `setIngressTlsCertSecretRef`. Keep the label "Coordinator ingress TLS cert". Leave the raw `ingressTlsSecret` prop wired at the API level but remove it from the visible form (back-compat stays server-side).

Reference the P2 picker JSX in `ControlPlaneEngines.tsx` (the `cpe-tls-cert` `Select`) and mirror it (id e.g. `es-ingress-tls-cert`).

- [ ] **Step 3: Typecheck + lint.** Run: `cd frontend && npm run typecheck && npm run lint`
  Expected: PASS.

- [ ] **Step 4: Commit.**

```bash
git add frontend/src/routes/control-plane/EngineSecurity.tsx frontend/src/routes/control-plane/useEngineConfigForm.ts
git commit -m "feat(ui): typed kubernetes.io/tls picker for engine ingress TLS cert"
```

---

### Task 7: Full gates + live acceptance on kind (controller-run)

**Files:** none (verification)

- [ ] **Step 1: Full workspace gates.** Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets && cargo test --workspace` and `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build`. All green (pre-existing unrelated failures noted, not regressions).

- [ ] **Step 2: Rebuild images + load into kind + rollout** (reuse the P2 acceptance flow: `just images-build`, kind load, restart `deploy/egret` + `deploy/egret-frontend`).

- [ ] **Step 3: Live acceptance.**
  1. Generate a leaf cert with SAN = the ingress host (e.g. `trino-p3.localtest.me`) — CA:FALSE + serverAuth (a company-cert shape; SAN must match the HOST, not the coordinator svc).
  2. Create a `kubernetes.io/tls` secret from it via the `/secrets` API.
  3. Provision a Trino engine with ingress enabled + `host = trino-p3.localtest.me` + `tlsCertSecretRef` = that secret (via the engine config API).
  4. Verify: `kubectl get secret {name}-ingress-tls -n egret-engines -o jsonpath='{.type}'` = `kubernetes.io/tls` and its `tls.crt` == the provided cert; the Ingress has `spec.tls[].secretName = {name}-ingress-tls`.
  5. `curl -k --resolve trino-p3.localtest.me:443:127.0.0.1 https://trino-p3.localtest.me/v1/info` through nginx-ingress serves the provided cert (verify the served cert fingerprint == the provided cert).
  6. Delete the engine → confirm `{name}-ingress-tls` and `{name}-tls` Secrets are gone (GC).
  7. Clean up the test secret.

- [ ] **Step 4:** Use superpowers:finishing-a-development-branch.

---

## Self-review notes

- **Spec coverage:** projection primitive (Task 2), config surface + no migration (Task 1), apply-time resolution + precedence (Task 3), validation (Task 4), GC decision (Task 5), UI picker (Task 6), SAN=host + curl live proof (Task 7). All spec sections mapped.
- **Type consistency:** `tls_cert_secret_ref: Option<Uuid>` (persisted) + `tls_cert_pem`/`tls_key_pem: Option<String>` (`#[serde(skip)]`) used identically across api struct, parse, provision, and render. Projected Secret name `{name}-ingress-tls` consistent in render (Task 2), provision (Task 3), and GC (Task 5).
- **No migration / no OpenAPI regen:** ingress config is an opaque JSON blob inside the engine `config`; the FE reuses the existing `/secrets` list — confirmed in the spec's grounded surface.
