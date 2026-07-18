# Typed Secrets P0 — Core model + backward-compat — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `egret-secrets` to a k8s-style typed multi-field secret model (`type` + a `data` key→sealed-value map, one seal per key) with per-type validation, keeping every existing consumer working via backward-compat shims.

**Architecture:** A new `secret_data` child table holds one sealed value per key; `secrets` gains a `type` column. New `put_typed`/`get_typed`/`get_key`/`update_typed` methods; the single-value `put`/`get`/`update` become shims over `Opaque{value}`. Crate + migration + tests only — no API/UI/projection/consumer changes.

**Tech Stack:** Rust, sqlx (runtime queries), Postgres, AES-256-GCM.

---

## Design notes (grounded)

- `secrets` (`0008`): `{ id, kind text, nonce bytea, ciphertext bytea, key_id text default 'cfg', created_at }`. Latest migration is `0087` → new file is **`0088_typed_secrets.sql`**.
- `SecretStore` (`store.rs`): `put(kind, plaintext)→Uuid`, `get(id)→String`, `update(id, plaintext)`, `delete(id)`; field `key_id: "cfg"`; DB via `self.db.pool`. `Crypto::seal(plaintext)→(Vec<u8> nonce, Vec<u8> ct)`, `open(&[u8], &[u8])→String`.
- `SecretsError` variants today: `Crypto(String)`, `NotConfigured`, `NotFound`, `AlreadyExists(String)`, `InUse(i64)`, `Db(#[from] sqlx::Error)`.
- `WorkspaceSecretStore` calls `secrets.put("workspace_secret", plaintext)` + `secrets.get(secret_ref)` — the shims keep it working.
- Migrations embed via `crates/egret-metastore/src/lib.rs:20` `MIGRATOR = sqlx::migrate!("../../migrations")`; touch the comment at the top (lines 2–3) to force a re-embed ([[egret-sqlx-migration-staleness]]).
- The crate's tests are DB-gated (`crates/egret-secrets/tests/{store,workspace_secrets}.rs`), self-skipping without `DATABASE_URL`.

## File Structure

- **Create** `migrations/0088_typed_secrets.sql`.
- **Modify** `crates/egret-metastore/src/lib.rs` — touch the migrations comment (re-embed).
- **Modify** `crates/egret-secrets/src/error.rs` — add `InvalidType` + `MissingKey`.
- **Create** `crates/egret-secrets/src/secret_type.rs` — the type registry + validation.
- **Modify** `crates/egret-secrets/src/lib.rs` — wire the module + export `TypedSecret`.
- **Modify** `crates/egret-secrets/src/store.rs` — `TypedSecret` + typed methods + shim rewrite.
- **Modify** `crates/egret-secrets/tests/store.rs` — typed + validation + shim tests.

---

### Task 1: Migration + re-embed

**Files:**
- Create: `migrations/0088_typed_secrets.sql`
- Modify: `crates/egret-metastore/src/lib.rs`

- [ ] **Step 1: Write the migration**

Create `migrations/0088_typed_secrets.sql`:

```sql
-- Typed secrets P0: move single-value secrets to a multi-field (key -> sealed value)
-- model. `secrets` gains a k8s-style `type`; values live in `secret_data`, one
-- sealed row per key. Legacy single-value secrets become Opaque with a `value` key.
ALTER TABLE secrets ADD COLUMN type text NOT NULL DEFAULT 'Opaque';

CREATE TABLE secret_data (
    secret_id  uuid  NOT NULL REFERENCES secrets(id) ON DELETE CASCADE,
    key        text  NOT NULL,
    nonce      bytea NOT NULL,
    ciphertext bytea NOT NULL,
    key_id     text  NOT NULL DEFAULT 'cfg',
    PRIMARY KEY (secret_id, key)
);

-- Copy every existing sealed value into secret_data under the `value` key.
INSERT INTO secret_data (secret_id, key, nonce, ciphertext, key_id)
    SELECT id, 'value', nonce, ciphertext, key_id FROM secrets;

-- secret_data is now the single source of truth for values.
ALTER TABLE secrets DROP COLUMN nonce, DROP COLUMN ciphertext;
```

- [ ] **Step 2: Force the migration to re-embed**

Edit the comment block at the top of `crates/egret-metastore/src/lib.rs` (around line 3, the `// migrations: through NNNN` line) to mention `0088 typed_secrets` — any text change to that file forces sqlx to re-embed the migrations dir. Example: change the `// migrations: through 0067 ...` line to append `; 0088 typed_secrets (multi-field secret_data)`.

- [ ] **Step 3: Verify it applies**

Run (with a disposable test DB): `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-secrets 2>&1 | tail -5`
Expected: the existing DB-gated tests still pass against the migrated schema (the old `get(id)` path now reads `secret_data`). If they self-skip (no DB), start Postgres first (`docker compose -f deploy/docker-compose.dev.yml up -d postgres`). This proves the migration + shim before writing new code — but if the shim isn't written yet (Tasks 2–3), expect a compile error, so this verification is fully meaningful only after Task 3; at this step just confirm the migration file SQL is valid by applying it manually or via a fresh `cargo build`.

- [ ] **Step 4: Commit**

```bash
git add migrations/0088_typed_secrets.sql crates/egret-metastore/src/lib.rs
git commit -m "feat(secrets): migration 0088 — secret_data multi-field table"
```

---

### Task 2: Error variants + type registry

**Files:**
- Modify: `crates/egret-secrets/src/error.rs`
- Create: `crates/egret-secrets/src/secret_type.rs`
- Modify: `crates/egret-secrets/src/lib.rs`

- [ ] **Step 1: Add error variants**

In `crates/egret-secrets/src/error.rs`, add to `SecretsError` (before the `Db` arm):

```rust
    /// Unknown/unsupported secret type.
    #[error("unknown secret type {0:?}")]
    InvalidType(String),
    /// A required key for the secret's type is missing or empty.
    #[error("secret is missing required key {0:?}")]
    MissingKey(String),
```

- [ ] **Step 2: Write the type registry**

Create `crates/egret-secrets/src/secret_type.rs`:

```rust
//! Typed-secret registry: k8s-style types and the data keys each requires.
//! P0 ships Opaque + TLS + basic-auth; adding a type later is one arm here
//! (plus, in a later phase, a k8s-projection mapping) — no schema change.

use std::collections::BTreeMap;

use crate::error::SecretsError;

pub const OPAQUE: &str = "Opaque";
pub const TLS: &str = "kubernetes.io/tls";
pub const BASIC_AUTH: &str = "kubernetes.io/basic-auth";

/// Required keys for a registered type; `Opaque` allows any keys. Returns `None`
/// for an unregistered type.
fn required_keys(r#type: &str) -> Option<&'static [&'static str]> {
    match r#type {
        OPAQUE => Some(&[]),
        TLS => Some(&["tls.crt", "tls.key"]),
        BASIC_AUTH => Some(&["username", "password"]),
        _ => None,
    }
}

/// Validate `data` against `type`: the type must be registered, and each of its
/// required keys must be present and non-empty.
pub fn validate(r#type: &str, data: &BTreeMap<String, String>) -> Result<(), SecretsError> {
    let required = required_keys(r#type).ok_or_else(|| SecretsError::InvalidType(r#type.into()))?;
    for k in required {
        if data.get(*k).map(String::is_empty).unwrap_or(true) {
            return Err(SecretsError::MissingKey((*k).into()));
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Wire the module**

In `crates/egret-secrets/src/lib.rs`, add `pub mod secret_type;` (after `mod store;`) and add `TypedSecret` to the store re-export (done in Task 3): change `pub use store::SecretStore;` to `pub use store::{SecretStore, TypedSecret};`.

- [ ] **Step 4: Verify compile (registry only)**

Run: `cargo build -p egret-secrets 2>&1 | tail -5`
Expected: compiles (the `TypedSecret` re-export will error until Task 3 adds it — if so, temporarily add `pub use store::SecretStore;` only and re-add `TypedSecret` in Task 3; otherwise do Task 3 before this build). Simplest: defer the `TypedSecret` re-export line to Task 3 and just add `pub mod secret_type;` here.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-secrets/src/error.rs crates/egret-secrets/src/secret_type.rs crates/egret-secrets/src/lib.rs
git commit -m "feat(secrets): type registry + validation (Opaque/tls/basic-auth)"
```

---

### Task 3: Typed store methods + shims

**Files:**
- Modify: `crates/egret-secrets/src/store.rs`
- Modify: `crates/egret-secrets/src/lib.rs` (the `TypedSecret` re-export, if deferred)

- [ ] **Step 1: Add the TypedSecret model + imports**

At the top of `crates/egret-secrets/src/store.rs`, add:

```rust
use std::collections::BTreeMap;
```

And define (below the `use` block, above `impl SecretStore` or after the struct):

```rust
/// A typed, multi-field secret: a k8s-style `type` plus a plaintext `data` map.
/// Returned only on read; values are never logged.
pub struct TypedSecret {
    pub r#type: String,
    pub data: BTreeMap<String, String>,
}
```

- [ ] **Step 2: Add the typed methods**

Add these methods to `impl SecretStore` (a transaction wraps the multi-row writes):

```rust
    /// Create a typed secret: validate, insert the `secrets` row, seal + insert one
    /// `secret_data` row per key (atomically). Returns the new id.
    pub async fn put_typed(
        &self,
        kind: &str,
        r#type: &str,
        data: &BTreeMap<String, String>,
    ) -> Result<Uuid, SecretsError> {
        crate::secret_type::validate(r#type, data)?;
        let crypto = self.crypto()?;
        let mut tx = self.db.pool.begin().await?;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO secrets (kind, type) VALUES ($1, $2) RETURNING id",
        )
        .bind(kind)
        .bind(r#type)
        .fetch_one(&mut *tx)
        .await?;
        for (key, value) in data {
            let (nonce, ct) = crypto.seal(value)?;
            sqlx::query(
                "INSERT INTO secret_data (secret_id, key, nonce, ciphertext, key_id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(key)
            .bind(nonce)
            .bind(ct)
            .bind(&self.key_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    /// Fetch + decrypt a typed secret (its type and full data map).
    pub async fn get_typed(&self, id: Uuid) -> Result<TypedSecret, SecretsError> {
        let r#type: String = sqlx::query_scalar("SELECT type FROM secrets WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or(SecretsError::NotFound)?;
        let rows: Vec<(String, Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT key, nonce, ciphertext FROM secret_data WHERE secret_id = $1")
                .bind(id)
                .fetch_all(&self.db.pool)
                .await?;
        let crypto = self.crypto()?;
        let mut data = BTreeMap::new();
        for (key, nonce, ct) in rows {
            data.insert(key, crypto.open(&nonce, &ct)?);
        }
        Ok(TypedSecret { r#type, data })
    }

    /// Fetch + decrypt a single key of a secret.
    pub async fn get_key(&self, id: Uuid, key: &str) -> Result<String, SecretsError> {
        let row: Option<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT nonce, ciphertext FROM secret_data WHERE secret_id = $1 AND key = $2",
        )
        .bind(id)
        .bind(key)
        .fetch_optional(&self.db.pool)
        .await?;
        let (nonce, ct) = row.ok_or(SecretsError::NotFound)?;
        self.crypto()?.open(&nonce, &ct)
    }

    /// Replace a typed secret's type + data map (re-validated, atomically).
    pub async fn update_typed(
        &self,
        id: Uuid,
        r#type: &str,
        data: &BTreeMap<String, String>,
    ) -> Result<(), SecretsError> {
        crate::secret_type::validate(r#type, data)?;
        let crypto = self.crypto()?;
        let mut tx = self.db.pool.begin().await?;
        let n = sqlx::query("UPDATE secrets SET type = $2 WHERE id = $1")
            .bind(id)
            .bind(r#type)
            .execute(&mut *tx)
            .await?;
        if n.rows_affected() == 0 {
            return Err(SecretsError::NotFound);
        }
        sqlx::query("DELETE FROM secret_data WHERE secret_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for (key, value) in data {
            let (nonce, ct) = crypto.seal(value)?;
            sqlx::query(
                "INSERT INTO secret_data (secret_id, key, nonce, ciphertext, key_id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(key)
            .bind(nonce)
            .bind(ct)
            .bind(&self.key_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
```

- [ ] **Step 3: Rewrite the single-value methods as shims**

Replace the existing `put`, `update`, and `get` method bodies (keep `delete` — `ON DELETE CASCADE` handles `secret_data`):

```rust
    /// Backward-compat: create an `Opaque` secret with a single `value` key.
    pub async fn put(&self, kind: &str, plaintext: &str) -> Result<Uuid, SecretsError> {
        let mut data = BTreeMap::new();
        data.insert("value".to_string(), plaintext.to_string());
        self.put_typed(kind, crate::secret_type::OPAQUE, &data).await
    }

    /// Backward-compat: re-seal the `value` key (preserves the row + type).
    pub async fn update(&self, id: Uuid, plaintext: &str) -> Result<(), SecretsError> {
        let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM secrets WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?;
        if exists.is_none() {
            return Err(SecretsError::NotFound);
        }
        let (nonce, ct) = self.crypto()?.seal(plaintext)?;
        sqlx::query(
            "INSERT INTO secret_data (secret_id, key, nonce, ciphertext, key_id) \
             VALUES ($1, 'value', $2, $3, $4) \
             ON CONFLICT (secret_id, key) DO UPDATE \
             SET nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext, key_id = EXCLUDED.key_id",
        )
        .bind(id)
        .bind(nonce)
        .bind(ct)
        .bind(&self.key_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Backward-compat: decrypt the single `value` key.
    pub async fn get(&self, id: Uuid) -> Result<String, SecretsError> {
        self.get_key(id, "value").await
    }
```

- [ ] **Step 4: Ensure the `TypedSecret` re-export**

In `crates/egret-secrets/src/lib.rs`, the store re-export is `pub use store::{SecretStore, TypedSecret};`.

- [ ] **Step 5: Verify compile + fmt + clippy**

Run: `cargo build -p egret-secrets && cargo fmt -p egret-secrets --check && cargo clippy -p egret-secrets -- -D warnings 2>&1 | tail -8`
Expected: clean.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-secrets/src/store.rs crates/egret-secrets/src/lib.rs
git commit -m "feat(secrets): typed put/get/update + single-value shims"
```

---

### Task 4: Tests + acceptance

**Files:**
- Modify: `crates/egret-secrets/tests/store.rs`

- [ ] **Step 1: Add typed + validation + shim tests**

Add to `crates/egret-secrets/tests/store.rs` (follow the file's existing DB-gated harness — reuse its pool/setup helper + master-key config; mirror an existing test's boilerplate):

```rust
#[sqlx::test]  // or the crate's existing DB-gated pattern — match the neighbouring tests
async fn typed_roundtrips_and_validates(pool: sqlx::PgPool) {
    let store = /* build SecretStore with a configured master key, per the existing tests */;
    use std::collections::BTreeMap;

    // Opaque multi-key round-trip.
    let mut opaque = BTreeMap::new();
    opaque.insert("a".into(), "1".into());
    opaque.insert("b".into(), "2".into());
    let id = store.put_typed("cfg", egret_secrets::secret_type::OPAQUE, &opaque).await.unwrap();
    let got = store.get_typed(id).await.unwrap();
    assert_eq!(got.r#type, "Opaque");
    assert_eq!(got.data.get("a").unwrap(), "1");
    assert_eq!(store.get_key(id, "b").await.unwrap(), "2");

    // TLS round-trip.
    let mut tls = BTreeMap::new();
    tls.insert("tls.crt".into(), "CERT".into());
    tls.insert("tls.key".into(), "KEY".into());
    let tid = store.put_typed("cfg", egret_secrets::secret_type::TLS, &tls).await.unwrap();
    assert_eq!(store.get_typed(tid).await.unwrap().r#type, "kubernetes.io/tls");

    // Validation: TLS missing tls.key → MissingKey; unknown type → InvalidType.
    let mut bad = BTreeMap::new();
    bad.insert("tls.crt".into(), "CERT".into());
    assert!(store.put_typed("cfg", egret_secrets::secret_type::TLS, &bad).await.is_err());
    assert!(store.put_typed("cfg", "not/a-type", &opaque).await.is_err());

    // Backward-compat shim: put/get still round-trip a single value as Opaque{value}.
    let sid = store.put("legacy", "hunter2").await.unwrap();
    assert_eq!(store.get(sid).await.unwrap(), "hunter2");
    let shimmed = store.get_typed(sid).await.unwrap();
    assert_eq!(shimmed.r#type, "Opaque");
    assert_eq!(shimmed.data.get("value").unwrap(), "hunter2");
    store.update(sid, "hunter3").await.unwrap();
    assert_eq!(store.get(sid).await.unwrap(), "hunter3");
}
```

Adapt the `store` construction + the `#[sqlx::test]`/pool wiring to whatever the existing tests in this file use (do NOT invent a new harness — copy the neighbouring test's setup).

- [ ] **Step 2: Run the crate tests (DB-gated)**

Ensure Postgres is up (`docker compose -f deploy/docker-compose.dev.yml up -d postgres`). Run:
`DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-secrets 2>&1 | tail -15`
Expected: all pass — the new typed test + the **existing** `store.rs` / `workspace_secrets.rs` tests (which prove the shim + migration keep single-value behavior).

- [ ] **Step 3: Workspace gates**

Run: `cargo fmt --all --check && cargo clippy -p egret-secrets -p egret-metastore -- -D warnings && cargo build -p egret-bin 2>&1 | tail -5`
Expected: clean (the `egret-bin` build confirms the migration re-embeds + nothing downstream broke).

- [ ] **Step 4: Commit**

```bash
git add crates/egret-secrets/tests/store.rs
git commit -m "test(secrets): typed round-trip, validation, and shim coverage"
```

---

## Self-Review

- **Spec coverage:** `secret_data` + `type` migration with legacy copy-then-drop (Task 1) ✓; error variants + type registry/validation (Task 2) ✓; `put_typed`/`get_typed`/`get_key`/`update_typed` + `TypedSecret` (Task 3) ✓; single-value shims over `Opaque{value}` (Task 3) ✓; tests incl. backward-compat + existing suite (Task 4) ✓; re-embed touch (Task 1) ✓.
- **No placeholders:** migration SQL, error variants, registry, and all store methods are complete; the only "adapt to existing" is the test harness boilerplate (explicitly: copy the neighbouring test's pool/store setup — do not invent).
- **Type consistency:** `put_typed`/`update_typed` params `(kind, r#type, data: &BTreeMap<String,String>)`; `TypedSecret { r#type, data }`; `secret_type::{OPAQUE,TLS,BASIC_AUTH}` + `validate` used consistently; shims call `put_typed`/`get_key`.
- **Out of scope (untouched):** HTTP API, UI, k8s projection, the ~113 `secret_ref` consumers — all keep working via the shims (P1+).
