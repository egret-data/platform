# Typed Secrets P1b — Storage + DB connection creds on typed basic-auth — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Back storage (`access_key`+`secret_key`) and DB (`user`+`password`) connections with a `kubernetes.io/basic-auth` typed secret (`username`+`password` keys); read creds by key; hybrid (connection keeps its plaintext identity column for display); eager idempotent Rust boot backfill converts existing connections.

**Architecture:** A resolver primitive (`resolve_basic_auth*`) reads `(username, password)` from a basic-auth secret and falls back to `("", value)` for a legacy Opaque one. Write paths mint a basic-auth secret when the identity is non-empty (else Opaque). A boot backfill rewrites existing connection secrets to basic-auth using the low-level `SecretStore` vault ops. No SQL migration, no OpenAPI regen, no validation-contract change.

**Tech Stack:** Rust (egret-secrets, egret-controlplane, egret-query, egret-bin). DB-gated tests read `DATABASE_URL` (self-skip without). Test DB: `postgres://egret:egret@localhost:5432/egret_test`.

---

## Design references

- Spec: `docs/en/specs/2026-07-19-typed-secrets-p1b-connection-creds-design.md`
- `WorkspaceSecretStore` (`crates/egret-secrets/src/workspace_secrets.rs`): `create` (85), `create_typed(ws,name,desc,type,data)` (119), `update_typed(ws,id,type,data)` (155), `resolve_plaintext(ws,id)` (265), `resolve_typed(ws,id)->TypedSecret` (275), `resolve_plaintext_ws_or_global(ws,id)` (295), private `secret_ref(ws,id)`.
- `SecretStore` (`crates/egret-secrets/src/store.rs`): `get_typed(id)` (86), `update_typed(id,type,data)` (119), `get(id)` (192, the Opaque shim over key `value`). `secret_type::BASIC_AUTH = "kubernetes.io/basic-auth"`; `validate` requires `username`+`password` non-empty.
- Storage: `StorageRegistry::list_storage_conns()` (`crates/egret-storage/src/registry.rs:146`) → `StorageConn{ access_key, secret_ref, .. }`; table `storage.storage_connections`. Write: `resolve_secret_ref` (`crates/egret-controlplane/src/storage.rs:246`) → `st.secrets.create(None, name, None, value)`. Read: `open_store` (`storage.rs:156`) → `resolve_plaintext(None, secret_ref)`.
- DB: write `resolve_secret_id` (`crates/egret-query/src/registry/routes.rs:119`, mints `st.workspace_secrets.create(Some(ws), name, None, value)` at line 129; `body` is `DatabaseInput` with `options.user`). Read `open_secret` (`crates/egret-query/src/registry/repo.rs:268`) → `resolve_plaintext_ws_or_global(ws, secret_id)`. Table `query.databases(options jsonb, secret_id, workspace_id)`.
- Boot: `serve` (`crates/egret-bin/src/main.rs:930`); `db.migrate()` (934); `SecretStore` built ~996; `WorkspaceSecretStore` ~1002. The backfill hooks in after `secrets` is built (needs only `&db` + `&secrets`).

---

### Task 1: Resolver primitive `resolve_basic_auth` + `resolve_basic_auth_ws_or_global`

**Files:**
- Modify: `crates/egret-secrets/src/workspace_secrets.rs`
- Test: same file's `#[cfg(test)]` module (DB-gated, mirrors `resolve_typed` tests)

- [ ] **Step 1: Write the failing tests.** Mirror the existing DB-gated `resolve_typed`/`create_typed` tests (grep the module for the `store()`/`test_state` helper + the self-skip pattern). Add two tests:

```rust
#[tokio::test]
async fn resolve_basic_auth_reads_username_and_password() {
    let Some(store) = ws_store().await else { return }; // self-skip helper used by neighbors
    let mut data = std::collections::BTreeMap::new();
    data.insert("username".to_string(), "ak".to_string());
    data.insert("password".to_string(), "sk".to_string());
    let id = store.create_typed(None, "p1b-ba", None, egret_secrets::secret_type::BASIC_AUTH, &data).await.unwrap();
    let (u, p) = store.resolve_basic_auth(None, id).await.unwrap();
    assert_eq!((u.as_str(), p.as_str()), ("ak", "sk"));
}

#[tokio::test]
async fn resolve_basic_auth_falls_back_to_opaque_value() {
    let Some(store) = ws_store().await else { return };
    let id = store.create(None, "p1b-op", None, "justpw").await.unwrap(); // Opaque shim
    let (u, p) = store.resolve_basic_auth(None, id).await.unwrap();
    assert_eq!((u.as_str(), p.as_str()), ("", "justpw"));
}
```
Adapt `ws_store()`/`create` names to the module's actual helpers/signatures.

- [ ] **Step 2: Run, expect FAIL** (`resolve_basic_auth` missing): `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-secrets resolve_basic_auth`

- [ ] **Step 3: Implement** both resolvers next to `resolve_typed`/`resolve_plaintext_ws_or_global`:

```rust
/// Read a named secret as a `(username, password)` pair. A
/// `kubernetes.io/basic-auth` secret yields its two keys; any other (legacy
/// Opaque) secret yields `("", <single value>)` so callers keep working before
/// the P1b backfill converts it.
pub async fn resolve_basic_auth(
    &self,
    workspace_id: Option<Uuid>,
    id: Uuid,
) -> Result<(String, String), SecretsError> {
    let typed = self.resolve_typed(workspace_id, id).await?;
    Ok(split_basic_auth(typed))
}

/// Like `resolve_basic_auth` but resolves a workspace-scoped OR global secret
/// (mirrors `resolve_plaintext_ws_or_global`; same security precondition — `id`
/// must be an egret-set `secret_id`, never user-supplied).
pub async fn resolve_basic_auth_ws_or_global(
    &self,
    workspace_id: Uuid,
    id: Uuid,
) -> Result<(String, String), SecretsError> {
    let secret_ref: Option<Uuid> = sqlx::query_scalar(
        "SELECT secret_ref FROM workspace_secrets \
         WHERE id = $1 AND (workspace_id = $2 OR workspace_id IS NULL)",
    )
    .bind(id)
    .bind(workspace_id)
    .fetch_optional(&self.db.pool)
    .await?;
    let secret_ref = secret_ref.ok_or(SecretsError::NotFound)?;
    Ok(split_basic_auth(self.secrets.get_typed(secret_ref).await?))
}
```

Add the shared helper (module-private free fn):

```rust
fn split_basic_auth(typed: crate::store::TypedSecret) -> (String, String) {
    if typed.r#type == crate::secret_type::BASIC_AUTH {
        (
            typed.data.get("username").cloned().unwrap_or_default(),
            typed.data.get("password").cloned().unwrap_or_default(),
        )
    } else {
        // Opaque shim stores the single value under key `value`.
        (String::new(), typed.data.get("value").cloned().unwrap_or_default())
    }
}
```

- [ ] **Step 4: Run, expect PASS.** Same command as Step 2.
- [ ] **Step 5: Commit** — `feat(secrets): resolve_basic_auth + _ws_or_global (typed creds by key, Opaque fallback)`

---

### Task 2: Storage — mint basic-auth on write, read the password by key

**Files:**
- Modify: `crates/egret-controlplane/src/storage.rs` (`resolve_secret_ref` write, `open_store` read)

- [ ] **Step 1: Write path.** In `resolve_secret_ref` (`storage.rs:246`), replace the Opaque mint with a basic-auth mint when the body carries an `access_key`:

```rust
async fn resolve_secret_ref(st: &CpState, body: &StorageInput) -> Result<Option<Uuid>, CpError> {
    match &body.new_secret {
        Some(ns) => {
            let id = if body.access_key.is_empty() {
                st.secrets.create(None, &ns.name, None, &ns.value).await?
            } else {
                let mut data = std::collections::BTreeMap::new();
                data.insert("username".to_string(), body.access_key.clone());
                data.insert("password".to_string(), ns.value.clone());
                st.secrets
                    .create_typed(None, &ns.name, None, egret_secrets::secret_type::BASIC_AUTH, &data)
                    .await?
            };
            Ok(Some(id))
        }
        None => Ok(body.secret_ref),
    }
}
```
Confirm `egret_secrets` is the crate path in scope (else use the correct path to `secret_type::BASIC_AUTH` + `create_typed`).

- [ ] **Step 2: Read path.** In `open_store` (`storage.rs:156`), replace the `resolve_plaintext` block:

```rust
let secret_key = match conn.secret_ref {
    Some(ref_id) => {
        let (_user, pw) = st
            .secrets
            .resolve_basic_auth(None, ref_id)
            .await
            .map_err(|e| CpError::BadRequest(format!("resolve secret: {e}")))?;
        pw
    }
    None => return Err(CpError::BadRequest("connection has no secret".into())),
};
```

- [ ] **Step 3: Build** — `cargo build -p egret-controlplane` (compiles).
- [ ] **Step 4: Commit** — `feat(storage): mint kubernetes.io/basic-auth secret + read password by key`

---

### Task 3: DB — mint basic-auth on write (when user present), read the password by key

**Files:**
- Modify: `crates/egret-query/src/registry/routes.rs` (`resolve_secret_id` write)
- Modify: `crates/egret-query/src/registry/repo.rs` (`open_secret` read)

- [ ] **Step 1: Write path.** In `resolve_secret_id` (`routes.rs:119`), change the `new_secret` branch (line ~128) to mint basic-auth when `body.options.user` is non-empty, else keep the Opaque `create`:

```rust
Some(n) => {
    let user = body.options.user.clone().unwrap_or_default();
    let id = if user.is_empty() {
        st.workspace_secrets.create(Some(workspace_id), &n.name, None, &n.value).await?
    } else {
        let mut data = std::collections::BTreeMap::new();
        data.insert("username".to_string(), user);
        data.insert("password".to_string(), n.value.clone());
        st.workspace_secrets
            .create_typed(Some(workspace_id), &n.name, None, egret_secrets::secret_type::BASIC_AUTH, &data)
            .await?
    };
    Ok(id)
}
```
Match the branch's actual return type/shape (it returns a `Uuid` / `Result<Uuid,_>`). Confirm `body` is in scope with `options.user` (it is `DatabaseInput`).

- [ ] **Step 2: Read path.** In `open_secret` (`repo.rs:268`), replace `resolve_plaintext_ws_or_global` with the basic-auth resolver, returning the password:

```rust
let (_user, pw) = self
    .workspace_secrets
    .resolve_basic_auth_ws_or_global(workspace_id, secret_id)
    .await?;
Ok(Some(pw))
```
Adapt to the function's exact local variable names (`workspace_id`, `secret_id`) and its `Result<Option<String>, DatabaseError>` return + error mapping (mirror how it currently maps `resolve_plaintext_ws_or_global`'s error).

- [ ] **Step 3: Build** — `cargo build -p egret-query` (compiles).
- [ ] **Step 4: Commit** — `feat(query): DB connection creds on kubernetes.io/basic-auth (mint + read by key)`

---

### Task 4: Eager boot backfill

**Files:**
- Create: `crates/egret-bin/src/backfill.rs` (or a module in `main.rs` if the crate has no module split — prefer a new file)
- Modify: `crates/egret-bin/src/main.rs` (call it in `serve` after `secrets` is built)

- [ ] **Step 1: Write the failing test** (DB-gated, in `backfill.rs` `#[cfg(test)]`). Seed a storage connection whose secret is Opaque, run the backfill, assert the secret became `kubernetes.io/basic-auth` with `username` = the connection's `access_key` and `password` = the original value; assert a second run is a no-op. (If wiring a full `CpState` is heavy, test the core `convert_one(secrets, vault_ref, identity)` helper directly against a seeded vault secret.)

```rust
#[tokio::test]
async fn backfill_converts_opaque_to_basic_auth_idempotently() {
    let Some((db, secrets)) = test_db_and_secrets().await else { return };
    // Arrange: an Opaque vault secret with value "pw".
    let vault_ref = secrets.put_typed("workspace_secret", egret_secrets::secret_type::OPAQUE,
        &std::collections::BTreeMap::from([("value".to_string(), "pw".to_string())])).await.unwrap();
    // Act: convert with identity "ak".
    convert_one(&secrets, vault_ref, "ak").await.unwrap();
    // Assert: basic-auth with username=ak, password=pw.
    let typed = secrets.get_typed(vault_ref).await.unwrap();
    assert_eq!(typed.r#type, egret_secrets::secret_type::BASIC_AUTH);
    assert_eq!(typed.data.get("username").unwrap(), "ak");
    assert_eq!(typed.data.get("password").unwrap(), "pw");
    // Idempotent: second run is a no-op (skip when already basic-auth).
    convert_one(&secrets, vault_ref, "ak").await.unwrap();
    assert_eq!(secrets.get_typed(vault_ref).await.unwrap().data.get("password").unwrap(), "pw");
}
```
Adapt helper names to whatever exists; if there's no `test_db_and_secrets`, build a `Db` + `SecretStore` from `DATABASE_URL` inline (mirror how egret-secrets tests build a store).

- [ ] **Step 2: Run, expect FAIL** (module/fn missing): `DATABASE_URL=…egret_test cargo test -p egret-bin backfill_converts_opaque`

- [ ] **Step 3: Implement.** In `backfill.rs`:

```rust
use egret_metastore::Db;
use egret_secrets::{secret_type, SecretStore};
use uuid::Uuid;

/// Convert one vault secret (referenced by a connection) to basic-auth, unless it
/// already is. `identity` becomes the `username`; the existing single value
/// becomes the `password`. Idempotent; skips an empty identity (left Opaque).
pub async fn convert_one(secrets: &SecretStore, vault_ref: Uuid, identity: &str) -> anyhow::Result<bool> {
    if identity.is_empty() {
        return Ok(false);
    }
    let typed = secrets.get_typed(vault_ref).await?;
    if typed.r#type == secret_type::BASIC_AUTH {
        return Ok(false);
    }
    let password = secrets.get(vault_ref).await?; // Opaque shim value
    let data = std::collections::BTreeMap::from([
        ("username".to_string(), identity.to_string()),
        ("password".to_string(), password),
    ]);
    secrets.update_typed(vault_ref, secret_type::BASIC_AUTH, &data).await?;
    Ok(true)
}

/// One-shot boot backfill: convert every storage + DB connection's Opaque
/// credential secret to `kubernetes.io/basic-auth`. Idempotent; safe to run on
/// every boot.
pub async fn backfill_basic_auth_credentials(db: &Db, secrets: &SecretStore) -> anyhow::Result<()> {
    // Storage: identity = access_key.
    let storage: Vec<(String, Option<Uuid>)> = sqlx::query_as(
        "SELECT s.access_key, ws.secret_ref \
         FROM storage.storage_connections s JOIN workspace_secrets ws ON ws.id = s.secret_ref \
         WHERE s.secret_ref IS NOT NULL",
    ).fetch_all(&db.pool).await?;
    // DB: identity = options->>'user'.
    let dbs: Vec<(serde_json::Value, Option<Uuid>)> = sqlx::query_as(
        "SELECT d.options, ws.secret_ref \
         FROM query.databases d JOIN workspace_secrets ws ON ws.id = d.secret_id \
         WHERE d.secret_id IS NOT NULL",
    ).fetch_all(&db.pool).await?;

    let mut converted = 0usize;
    for (access_key, vault_ref) in storage {
        if let Some(v) = vault_ref {
            if convert_one(secrets, v, &access_key).await? { converted += 1; }
        }
    }
    for (options, vault_ref) in dbs {
        let user = options.get("user").and_then(|u| u.as_str()).unwrap_or("");
        if let Some(v) = vault_ref {
            if convert_one(secrets, v, user).await? { converted += 1; }
        }
    }
    if converted > 0 {
        tracing::info!(converted, "P1b: backfilled connection creds to kubernetes.io/basic-auth");
    }
    Ok(())
}
```
Note: the `ws.secret_ref` selected here is the **vault** `secrets` id (the `workspace_secrets.secret_ref` FK), which is exactly what `SecretStore::get_typed/get/update_typed` key on. Add `mod backfill;` to `main.rs`.

- [ ] **Step 4: Wire into `serve`.** In `main.rs` after the `SecretStore` (`secrets`) is constructed (~line 997), add:

```rust
backfill::backfill_basic_auth_credentials(&db, &secrets)
    .await
    .context("P1b basic-auth backfill")?;
```

- [ ] **Step 5: Run test, expect PASS.** Same command as Step 2. Also `cargo build -p egret-bin`.
- [ ] **Step 6: Commit** — `feat(bin): eager boot backfill of connection creds to basic-auth`

---

### Task 5: Full gates + live acceptance on kind (controller-run)

**Files:** none (verification)

- [ ] **Step 1: Full gates.** `cargo fmt --all --check && cargo clippy --workspace --all-targets && DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test --workspace --no-fail-fast` (expect only the 2 pre-existing `assignment.rs` failures). Frontend `npm run lint && npm run typecheck && npm run test && npm run build`.
- [ ] **Step 2: Rebuild images + load + rollout** (P2/P3 flow: `just images-build`, `kind load docker-image egret-backend:dev egret-frontend:dev --name egret`, `kubectl rollout restart deploy/egret deploy/egret-frontend -n egret-system`).
- [ ] **Step 3: Live acceptance.**
  1. Create a storage connection via `/api/control-plane/storage` (access_key + new_secret.value) → verify its `workspace_secrets`→vault secret is `kubernetes.io/basic-auth` with keys `username` (=access_key) + `password`; provision a warehouse through it (proves `open_store` reads the password).
  2. Create a DB (query) connection with `options.user` + `new_secret` → verify basic-auth secret; run a query (proves `open_secret` reads the password).
  3. Backfill: seed a legacy Opaque-secret storage connection (e.g. via psql insert of an Opaque secret + connection, OR reuse a pre-existing one), restart `deploy/egret`, verify the boot log reports the conversion and the secret is now basic-auth and the connection still works.
  4. Clean up the test resources.
- [ ] **Step 4:** Use superpowers:finishing-a-development-branch.

---

## Self-review notes

- **Spec coverage:** resolver primitive (Task 1), storage write+read (Task 2), DB write+read (Task 3), eager backfill + empty-identity skip + idempotency (Task 4), gates + live proof (Task 5). All spec sections mapped.
- **Type consistency:** `resolve_basic_auth(ws,id)` + `resolve_basic_auth_ws_or_global(ws:Uuid,id)` return `(String,String)`; `split_basic_auth` shared; `convert_one(secrets, vault_ref, identity)->bool` reused by the loop + test. `secret_type::BASIC_AUTH` keys `username`/`password` consistent across write, read, backfill.
- **No migration / no OpenAPI regen:** backfill is a Rust boot task keyed on the vault id; the request wire shape is unchanged (value→password, identity→username), so no generated-client change.
- **Empty-identity:** storage guards on `body.access_key.is_empty()`, DB on `options.user` empty, backfill on `identity.is_empty()` — all keep/leave Opaque, read via the `split_basic_auth` fallback.
