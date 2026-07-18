# Typed Secrets P2 — Engine coordinator TLS from a secret — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A jwt-mode Trino engine can reference a P1a `kubernetes.io/tls` secret for its coordinator TLS — if a cert is provided, use it; otherwise self-generate (unchanged). Adds the reusable projection primitive (resolve a typed-tls secret → cert+key).

**Architecture:** A nullable `engines.tls_cert_secret_ref` column; `ensure_tls_cert` gains a branch that resolves the referenced typed-TLS secret's `tls.crt`+`tls.key` into the keystore instead of generating self-signed. Everything downstream (`{name}-tls` k8s Secret, coordinator mount, `coordinator_cert` pin) is unchanged. Backward-compatible: NULL ref = self-signed.

**Tech Stack:** Rust (sqlx, axum), TypeScript/React.

---

## Design notes (grounded)

- `ensure_tls_cert(st, engine) -> (keystore, cert)` (`engine-mgmt/src/engines.rs:1544`): reuse `(tls_secret_id, coordinator_cert)` if set (idempotent); else `generate_self_signed(&[engine.coordinator_host()])` → `keystore = cert_pem + key_pem` → `secrets.create("engine-tls-{id}", keystore)` → `store.set_tls(engine.id, sid, cert_pem)`.
- `Engine` (`store.rs:13`) is `#[derive(sqlx::FromRow)]` with `tls_secret_id`, `coordinator_cert`; loaded by `query_as::<_, Engine>` over explicit column lists at `store.rs:185` (INSERT…RETURNING), `205`, `214`, `474`. FromRow maps by column NAME → adding a struct field requires adding the column to every one of those lists. `store.create` INSERTs `(name, type, namespace, warehouse_id, spec)`.
- P1a: `SecretStore::get_typed(id) -> TypedSecret { r#type, data: BTreeMap }`; `secret_type::TLS = "kubernetes.io/tls"` (pub); `WorkspaceSecretStore::resolve_plaintext(ws, id)` resolves `secret_ref` then `secrets.get(ref)`.
- Latest migration `0088` → new `0089`. Re-embed by touching `crates/egret-metastore/src/lib.rs`.
- `CreateEngineBody` (`engines.rs`, camelCase wire) is the create body; `create_engine` handler resolves the warehouse, mints a catalog client, calls `store.create`, then provisions (`build_provision_req` → `ensure_tls_cert`).

## File Structure

- **Create** `migrations/0089_engine_tls_cert_ref.sql`; **modify** `crates/egret-metastore/src/lib.rs` (re-embed).
- **Modify** `crates/egret-secrets/src/workspace_secrets.rs` — `resolve_typed`.
- **Modify** `crates/egret-engine-mgmt/src/store.rs` — `Engine.tls_cert_secret_ref` + SELECT/INSERT lists + `store.create` param.
- **Modify** `crates/egret-engine-mgmt/src/engines.rs` — `resolve_tls_cert_secret`, `ensure_tls_cert` branch, `CreateEngineBody` + `create_engine` wiring.
- **Modify** `frontend/src/routes/control-plane/ControlPlaneEngines.tsx` (+ api types) — TLS-cert picker.
- **Modify** tests.

---

### Task 1: Migration + Engine column plumbing

**Files:**
- Create: `migrations/0089_engine_tls_cert_ref.sql`
- Modify: `crates/egret-metastore/src/lib.rs`, `crates/egret-engine-mgmt/src/store.rs`

- [ ] **Step 1: Migration**

Create `migrations/0089_engine_tls_cert_ref.sql`:

```sql
-- Optional: back jwt-mode coordinator TLS with a user-provided kubernetes.io/tls
-- secret (a shared/company cert) instead of the auto self-signed cert.
-- NULL keeps the current per-engine self-signed behavior.
ALTER TABLE engine_mgmt.engines ADD COLUMN tls_cert_secret_ref uuid;
```

Touch `crates/egret-metastore/src/lib.rs` (append `; 0089 engine_tls_cert_ref` to the `// migrations: through NNNN` comment) to force re-embed.

- [ ] **Step 2: Engine struct + column lists**

In `crates/egret-engine-mgmt/src/store.rs`: add to `struct Engine` (after `coordinator_cert`):

```rust
    pub tls_cert_secret_ref: Option<Uuid>,
```

Add `tls_cert_secret_ref` to EVERY engine column list — the INSERT `RETURNING` (line ~186), and the three SELECT lists (~205, ~214, ~474; the last is `e.tls_cert_secret_ref`). Add the field (`tls_cert_secret_ref: None`) to any literal `Engine { … }` test constructors (search `tls_secret_id: None`).

- [ ] **Step 3: Persist on create**

Extend `store.create` to accept `tls_cert_secret_ref: Option<Uuid>` and include it in the INSERT:
`INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec, tls_cert_secret_ref) VALUES ($1,$2,$3,$4,$5,$6) RETURNING …` (with `tls_cert_secret_ref` also in RETURNING). Bind the new param. Update both call sites (the Trino + StarRocks create paths) to pass the value (from `CreateEngineBody`, Task 4).

- [ ] **Step 4: Build**

Run: `cargo build -p egret-engine-mgmt -p egret-metastore 2>&1 | tail -6` — expect compile errors only where Task 4 hasn't passed the new arg yet; otherwise clean. (Do Task 4 before the final build if the arg is required.)

- [ ] **Step 5: Commit**

```bash
git add migrations/0089_engine_tls_cert_ref.sql crates/egret-metastore/src/lib.rs crates/egret-engine-mgmt/src/store.rs
git commit -m "feat(engine): engines.tls_cert_secret_ref column + Engine plumbing"
```

---

### Task 2: Projection primitive

**Files:**
- Modify: `crates/egret-secrets/src/workspace_secrets.rs`
- Modify: `crates/egret-engine-mgmt/src/engines.rs`

- [ ] **Step 1: `resolve_typed` on the named layer**

Add to `impl WorkspaceSecretStore` (`crates/egret-secrets/src/workspace_secrets.rs`):

```rust
    /// Decrypt and return a named secret as its typed form (type + data map).
    pub async fn resolve_typed(
        &self,
        workspace_id: Option<Uuid>,
        id: Uuid,
    ) -> Result<crate::store::TypedSecret, SecretsError> {
        let secret_ref = self.secret_ref(workspace_id, id).await?;
        self.secrets.get_typed(secret_ref).await
    }
```

(Confirm `TypedSecret` is re-exported/visible as `crate::store::TypedSecret`; P1a made `pub use store::{SecretStore, TypedSecret}` in lib.rs — use `crate::TypedSecret` if that's the path.)

- [ ] **Step 2: `resolve_tls_cert_secret` helper**

Add to `crates/egret-engine-mgmt/src/engines.rs` (near `ensure_tls_cert`):

```rust
/// Projection primitive: resolve a `kubernetes.io/tls` named secret to
/// `(cert_pem, key_pem)`. Reused wherever a typed TLS secret backs a k8s cert.
async fn resolve_tls_cert_secret(
    st: &CpState,
    secret_ref: Uuid,
) -> Result<(String, String), CpError> {
    let ts = st
        .secrets
        .resolve_typed(None, secret_ref)
        .await
        .map_err(|e| CpError::BadRequest(format!("resolve tls cert secret: {e}")))?;
    if ts.r#type != egret_secrets::secret_type::TLS {
        return Err(CpError::BadRequest(format!(
            "secret is not a {} secret",
            egret_secrets::secret_type::TLS
        )));
    }
    let get = |k: &str| {
        ts.data
            .get(k)
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or_else(|| CpError::BadRequest(format!("tls secret missing {k}")))
    };
    Ok((get("tls.crt")?, get("tls.key")?))
}
```

- [ ] **Step 3: Build + fmt + clippy**

Run: `cargo build -p egret-secrets -p egret-engine-mgmt && cargo fmt -p egret-secrets -p egret-engine-mgmt --check && cargo clippy -p egret-secrets -p egret-engine-mgmt -- -D warnings 2>&1 | tail -6`
Expected: clean (the `resolve_tls_cert_secret` `dead_code` warning is fine until Task 3 uses it — or add `#[allow(dead_code)]` temporarily; Task 3 removes the need).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-secrets/src/workspace_secrets.rs crates/egret-engine-mgmt/src/engines.rs
git commit -m "feat(secrets): resolve_typed + resolve_tls_cert_secret projection primitive"
```

---

### Task 3: `ensure_tls_cert` provided-cert branch

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs`

- [ ] **Step 1: Use the provided cert when referenced**

In `ensure_tls_cert`, replace the self-signed generation line

```rust
    let (cert_pem, key_pem) =
        crate::tls_cert::generate_self_signed(&[engine.coordinator_host()])
            .map_err(|e| CpError::BadRequest(format!("generate engine tls cert: {e}")))?;
```

with a branch (the earlier `(tls_secret_id, coordinator_cert)` idempotent-reuse block above it is unchanged and still wins first):

```rust
    // A referenced kubernetes.io/tls secret backs the coordinator TLS; otherwise
    // fall back to a fresh self-signed cert (SAN = the coordinator host).
    let (cert_pem, key_pem) = if let Some(ref_id) = engine.tls_cert_secret_ref {
        resolve_tls_cert_secret(st, ref_id).await?
    } else {
        crate::tls_cert::generate_self_signed(&[engine.coordinator_host()])
            .map_err(|e| CpError::BadRequest(format!("generate engine tls cert: {e}")))?
    };
```

The rest of `ensure_tls_cert` (build `keystore`, store it in `engine-tls-{id}`, `set_tls`) is unchanged — it now materializes the provided cert's keystore. `coordinator_cert` becomes the provided `tls.crt` (the connector pins it).

- [ ] **Step 2: Build + fmt + clippy**

Run: `cargo build -p egret-engine-mgmt && cargo fmt -p egret-engine-mgmt --check && cargo clippy -p egret-engine-mgmt -- -D warnings 2>&1 | tail -6`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add crates/egret-engine-mgmt/src/engines.rs
git commit -m "feat(engine): ensure_tls_cert uses a referenced kubernetes.io/tls secret"
```

---

### Task 4: API — create body carries the reference

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs`

- [ ] **Step 1: Add the field to `CreateEngineBody`**

In `CreateEngineBody`, add (camelCase wire; it already has `#[serde(rename_all = "camelCase")]`):

```rust
    /// Optional: a kubernetes.io/tls named secret to back the coordinator TLS.
    #[serde(default)]
    tls_cert_secret_ref: Option<Uuid>,
```

- [ ] **Step 2: Validate + persist in `create_engine`**

In the `create_engine` handler:
- Before `store.create`, if `b.tls_cert_secret_ref` is `Some(ref_id)`, call `resolve_tls_cert_secret(&st, ref_id).await?` to validate the secret exists + is a valid TLS secret (fails with 400 otherwise). Discard the result.
- Pass `b.tls_cert_secret_ref` into the `store.create(...)` call (the new param from Task 1). Do the same in the StarRocks create path if it shares the body (StarRocks TLS-from-secret enforcement is a later slice, but persist the column so it round-trips).

The provisioning that follows (`build_provision_req` → `ensure_tls_cert`) reads `engine.tls_cert_secret_ref` from the freshly-created row (now populated), so it uses the provided cert.

- [ ] **Step 3: Surface it on the DTO (optional read-back)**

If `EngineDto`/`EngineSpecDto` should show the reference, add `tls_cert_secret_ref` to the response DTO. Minimum: create + provisioning works; the read-back is optional — include it if trivial.

- [ ] **Step 4: Build + fmt + clippy + full build**

Run: `cargo build -p egret-engine-mgmt && cargo fmt --all --check && cargo clippy -p egret-engine-mgmt -- -D warnings && cargo build -p egret-bin 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-engine-mgmt/src/engines.rs
git commit -m "feat(engine): create-engine accepts + validates tlsCertSecretRef"
```

---

### Task 5: UI — TLS-cert picker (+ OpenAPI/orval regen)

**Files:**
- Regenerate `openapi.json` + `frontend/src/api/generated/*`
- Modify: `frontend/src/routes/control-plane/ControlPlaneEngines.tsx`

- [ ] **Step 1: Regenerate the client**

Run (repo root): `cargo run -p egret-bin -- openapi > openapi.json`, then `cd frontend && npm run gen:api`. Commit the regenerated files.
```bash
git add openapi.json frontend/src/api/generated
git commit -m "chore(api): regenerate client for tlsCertSecretRef"
```

- [ ] **Step 2: TLS-cert Select in the Add-engine dialog**

In `ControlPlaneEngines.tsx`'s `AddEngineDialog`, fetch the secrets (`listSecrets` from `@/api/controlPlane`) and add an optional **"Coordinator TLS cert"** `Select` (id `cpe-tls-cert`) listing the ones with `type === "kubernetes.io/tls"`, plus a default **"Self-signed"** option (empty value). Keep a `const [tlsCertSecretRef, setTlsCertSecretRef] = useState("")` and include it in the Trino create body when non-empty:
```ts
...(tlsCertSecretRef ? { tlsCertSecretRef } : {})
```
Add `tlsCertSecretRef?: string` to the `CreateEngineInput` type used here. A one-line helper hint ("Only used for JWT/TLS engines") is enough.

- [ ] **Step 3: Typecheck + lint**

Run: `cd frontend && npm run typecheck && npm run lint 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/control-plane/ControlPlaneEngines.tsx
git commit -m "feat(ui): Add-engine coordinator TLS cert picker (kubernetes.io/tls secrets)"
```

---

### Task 6: Tests + acceptance

**Files:**
- Modify: crate tests

- [ ] **Step 1: Backend tests**

- `egret-secrets` (`tests/workspace_secrets.rs`, existing DB-gated harness): `resolve_typed` returns the TLS secret's `{type, data}`.
- `egret-engine-mgmt` (its test harness): `resolve_tls_cert_secret` returns `(crt, key)` for a `kubernetes.io/tls` secret and errors on a non-TLS type / a secret missing `tls.key`. If `ensure_tls_cert` is unit-testable with a fake store, assert the provided-cert branch yields `keystore == crt+key` and `coordinator_cert == crt`; otherwise rely on the live acceptance.

- [ ] **Step 2: Backend gates**

Run (Postgres up): `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-secrets -p egret-engine-mgmt 2>&1 | tail -15`, then `cargo fmt --all --check && cargo clippy -p egret-engine-mgmt -- -D warnings && cargo build -p egret-bin`.
Expected: all pass.

- [ ] **Step 3: Frontend gates**

Run: `cd frontend && npm run typecheck && npm run lint && npm test && npm run build 2>&1 | tail -3`
Expected: all pass.

- [ ] **Step 4: Live acceptance (opt-in — reuses the data-plane path)**

Against a running kind deploy (rebuild + load the images): create a `kubernetes.io/tls` secret (any cert+key text) via the secrets console; provision a jwt-mode Trino engine referencing it (via the Add-engine picker or the API `tlsCertSecretRef`); enable JWT → confirm the `{name}-tls` k8s Secret's `keystore.pem` carries the PROVIDED cert (`kubectl get secret {name}-tls -o jsonpath …`), the coordinator serves HTTPS, and a query succeeds (egret pins `coordinator_cert` = the provided `tls.crt`). A NULL-ref engine still self-signs. Fix any drift; re-run.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-secrets/tests crates/egret-engine-mgmt
git commit -m "test(engine): tls-cert projection primitive + provided-cert keystore"
```

---

## Self-Review

- **Spec coverage:** `tls_cert_secret_ref` column + plumbing (Task 1) ✓; `resolve_typed` + `resolve_tls_cert_secret` primitive (Task 2) ✓; `ensure_tls_cert` provided-vs-self-signed branch (Task 3) ✓; create body + validation + persist (Task 4) ✓; UI picker + regen (Task 5) ✓; tests + live data-plane acceptance (Task 6) ✓.
- **No placeholders:** migration, struct field, `resolve_typed`, `resolve_tls_cert_secret`, and the `ensure_tls_cert` branch are complete; the SELECT-list edits, `store.create` param, `create_engine` wiring, and UI markup are explicit "read the exact code and edit" steps.
- **Backward-compat:** NULL `tls_cert_secret_ref` → unchanged self-signed path; the `{name}-tls` Secret + mount + `coordinator_cert` pinning are untouched.
- **Type consistency:** `tls_cert_secret_ref: Option<Uuid>` (struct/body) ↔ `tlsCertSecretRef` (camelCase wire, UI); `resolve_tls_cert_secret → (cert_pem, key_pem)`; `secret_type::TLS`.
