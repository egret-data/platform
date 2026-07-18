# Typed Secrets P1a — Typed-secret console — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The `/secrets` API + control-plane UI create/list/edit **typed** named secrets (`Opaque` / `kubernetes.io/tls` / `kubernetes.io/basic-auth`); `WorkspaceSecretStore` gains typed create/update; the DTO exposes `type` + data key names (never values).

**Architecture:** `WorkspaceSecretStore` grows `create_typed`/`update_typed` over P0's `SecretStore::put_typed`; its `WorkspaceSecretMeta` carries `type` + `keys` (JOIN `secrets.type` + aggregate `secret_data.key`). The API adds `type`+`data` (with a `value` back-compat alias) and `type`+`keys` on the DTO. The frontend regenerates the orval client from the new OpenAPI and gets a type-aware Add/Edit dialog.

**Tech Stack:** Rust (axum, sqlx), TypeScript/React, orval.

---

## Design notes (grounded)

- P0 crate: `SecretStore::{put_typed(kind,type,data), get_typed, get_key, update_typed}`; `secret_type::{OPAQUE, TLS, BASIC_AUTH, validate}`.
- `WorkspaceSecretStore` (`workspace_secrets.rs`): `create(ws, name, desc, plaintext)`, `list(ws)`, `get_meta(ws, id)`, `update_value`, `rename`, `delete`, `secret_ref(ws, id)`; `WorkspaceSecretMeta { id, name, description, created_at, used_by }`; `MetaRow = (Uuid, String, Option<String>, DateTime<Utc>, i64)`. List/get_meta compute `used_by` via subqueries on `storage.storage_connections` + `query.databases`.
- API (`egret-controlplane/src/secrets.rs`): handlers call `st.secrets.{list,create,get_meta,rename,update_value,delete}(None, …)`. DTOs `CreateSecretInput{name,description?,value}`, `UpdateSecretInput{name?,description?,value?}`, `SecretDto{id,name,description,created_at,used_by}`.
- Frontend: `ControlPlaneSecrets.tsx` imports `Secret`, `SecretInput`, `createSecret`/`updateSecret` from the **hand-written** `src/api/controlPlane.ts`, whose wrappers delegate to the **generated** `controlPlaneSecretsCreate`/`Update`. Orval input = committed `openapi.json` at repo root (emitted by `cargo run -p egret-bin -- openapi`); regen with `cd frontend && npm run gen:api`.

## File Structure

- **Modify** `crates/egret-secrets/src/workspace_secrets.rs` — `create_typed`, `update_typed`, `WorkspaceSecretMeta` + `type`/`keys`, list/get_meta SQL.
- **Modify** `crates/egret-secrets/tests/workspace_secrets.rs` — typed coverage.
- **Modify** `crates/egret-controlplane/src/secrets.rs` — typed DTOs + handlers.
- **Regenerate** `openapi.json` (repo root) + `frontend/src/api/generated/*` (orval).
- **Modify** `frontend/src/api/controlPlane.ts` — `Secret`/`SecretInput`/`SecretUpdate` types.
- **Modify** `frontend/src/routes/control-plane/ControlPlaneSecrets.tsx` — type-aware dialog + list.

---

### Task 1: `WorkspaceSecretStore` typed create/update + meta

**Files:**
- Modify: `crates/egret-secrets/src/workspace_secrets.rs`

- [ ] **Step 1: Add `type` + `keys` to the meta**

Change `WorkspaceSecretMeta` (add fields) and `MetaRow` (extend the tuple):

```rust
/// Metadata for a named workspace secret. Never exposes the plaintext value.
#[derive(Debug, Clone)]
pub struct WorkspaceSecretMeta {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Number of connections (storages + databases) referencing this secret.
    pub used_by: i64,
    /// k8s-style secret type (Opaque / kubernetes.io/tls / kubernetes.io/basic-auth).
    pub r#type: String,
    /// Data key names (NOT values).
    pub keys: Vec<String>,
}

/// One `workspace_secrets` row joined with its type, key names, and reference count.
type MetaRow = (Uuid, String, Option<String>, DateTime<Utc>, i64, String, Vec<String>);

impl From<MetaRow> for WorkspaceSecretMeta {
    fn from((id, name, description, created_at, used_by, r#type, keys): MetaRow) -> Self {
        WorkspaceSecretMeta { id, name, description, created_at, used_by, r#type, keys }
    }
}
```

- [ ] **Step 2: Extend the list + get_meta queries**

In `list` and `get_meta`, JOIN `secrets` for `type` and aggregate the key names. Replace both SELECTs' column list + FROM to add `s.type` and a `keys` array, joining `secrets s ON s.id = ws.secret_ref`:

```rust
// in list():
        let rows: Vec<MetaRow> = sqlx::query_as(
            "SELECT ws.id, ws.name, ws.description, ws.created_at, \
             (SELECT count(*) FROM storage.storage_connections WHERE secret_ref = ws.id) \
             + (SELECT count(*) FROM query.databases WHERE secret_id = ws.id) AS used_by, \
             s.type, \
             ARRAY(SELECT key FROM secret_data WHERE secret_id = ws.secret_ref ORDER BY key) AS keys \
             FROM workspace_secrets ws JOIN secrets s ON s.id = ws.secret_ref \
             WHERE ws.workspace_id IS NOT DISTINCT FROM $1 \
             ORDER BY ws.name",
        )
```

Apply the same added columns + JOIN to `get_meta`'s SELECT (keep its `WHERE ws.id = $1 AND ws.workspace_id IS NOT DISTINCT FROM $2`).

- [ ] **Step 3: Add `create_typed` + `update_typed`**

Add to `impl WorkspaceSecretStore` (mirror `create`'s duplicate-name rollback):

```rust
    /// Create a typed named secret (validates + seals the data map into a new
    /// vault row, then registers the named entry). Duplicate name → rollback.
    pub async fn create_typed(
        &self,
        workspace_id: Option<Uuid>,
        name: &str,
        description: Option<&str>,
        r#type: &str,
        data: &std::collections::BTreeMap<String, String>,
    ) -> Result<Uuid, SecretsError> {
        let vault_id = self.secrets.put_typed("workspace_secret", r#type, data).await?;
        let res: Result<Uuid, sqlx::Error> = sqlx::query_scalar(
            "INSERT INTO workspace_secrets (workspace_id, name, description, secret_ref) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(vault_id)
        .fetch_one(&self.db.pool)
        .await;
        match res {
            Ok(id) => Ok(id),
            Err(e) => {
                let _ = self.secrets.delete(vault_id).await;
                if is_unique_violation(&e) {
                    Err(SecretsError::AlreadyExists(name.to_string()))
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// Rotate a named secret's type + data map (re-validates; the named id is unchanged).
    pub async fn update_typed(
        &self,
        workspace_id: Option<Uuid>,
        id: Uuid,
        r#type: &str,
        data: &std::collections::BTreeMap<String, String>,
    ) -> Result<(), SecretsError> {
        let secret_ref = self.secret_ref(workspace_id, id).await?;
        self.secrets.update_typed(secret_ref, r#type, data).await
    }
```

- [ ] **Step 4: Build + fmt + clippy**

Run: `cargo build -p egret-secrets && cargo fmt -p egret-secrets --check && cargo clippy -p egret-secrets -- -D warnings 2>&1 | tail -6`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-secrets/src/workspace_secrets.rs
git commit -m "feat(secrets): WorkspaceSecretStore typed create/update + meta type/keys"
```

---

### Task 2: Typed `/secrets` API

**Files:**
- Modify: `crates/egret-controlplane/src/secrets.rs`

- [ ] **Step 1: Typed DTOs**

Replace the three DTO structs + the `From<WorkspaceSecretMeta>` impl:

```rust
use std::collections::BTreeMap;

/// Request body for creating a global named secret. Either a typed `data` map
/// (with `type`, default `Opaque`) or the legacy single `value` (→ Opaque{value}).
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateSecretInput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    #[serde(default)]
    pub data: Option<BTreeMap<String, String>>,
    /// Back-compat: a single value stored as `Opaque{value}`.
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateSecretInput {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "type")]
    pub r#type: Option<String>,
    #[serde(default)]
    pub data: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SecretDto {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub used_by: i64,
    #[serde(rename = "type")]
    pub r#type: String,
    pub keys: Vec<String>,
}

impl From<WorkspaceSecretMeta> for SecretDto {
    fn from(m: WorkspaceSecretMeta) -> Self {
        SecretDto {
            id: m.id,
            name: m.name,
            description: m.description,
            created_at: m.created_at,
            used_by: m.used_by,
            r#type: m.r#type,
            keys: m.keys,
        }
    }
}
```

Add a helper (module-level) to resolve `(type, data)` from an input:

```rust
/// Resolve the (type, data) to write from the typed fields or the legacy `value`.
fn resolve_type_data(
    r#type: &Option<String>,
    data: &Option<BTreeMap<String, String>>,
    value: &Option<String>,
) -> Result<(String, BTreeMap<String, String>), CpError> {
    match (data, value) {
        (Some(d), None) => Ok((r#type.clone().unwrap_or_else(|| "Opaque".into()), d.clone())),
        (None, Some(v)) => {
            let mut m = BTreeMap::new();
            m.insert("value".to_string(), v.clone());
            Ok(("Opaque".to_string(), m))
        }
        (None, None) => Err(CpError::BadRequest("secret needs `data` or `value`".into())),
        (Some(_), Some(_)) => Err(CpError::BadRequest("give `data` or `value`, not both".into())),
    }
}
```
(Confirm the `CpError::BadRequest` constructor name against `crate::error::CpError`; use whatever the crate exposes for a 400.)

- [ ] **Step 2: Typed create + update handlers**

`create`: replace the `create` call:

```rust
    let (r#type, data) = resolve_type_data(&body.r#type, &body.data, &body.value)?;
    let id = st
        .secrets
        .create_typed(None, &body.name, body.description.as_deref(), &r#type, &data)
        .await?;
    let meta = st.secrets.get_meta(None, id).await?;
    Ok((StatusCode::CREATED, Json(meta.into())))
```

`update`: rename as before, then rotate typed when `data`/`value` present:

```rust
    if let Some(name) = &body.name {
        st.secrets.rename(None, id, name, body.description.as_deref()).await?;
    }
    if body.data.is_some() || body.value.is_some() {
        let (r#type, data) = resolve_type_data(&body.r#type, &body.data, &body.value)?;
        st.secrets.update_typed(None, id, &r#type, &data).await?;
    }
    let meta = st.secrets.get_meta(None, id).await?;
    Ok(Json(meta.into()))
```

- [ ] **Step 3: Build + fmt + clippy**

Run: `cargo build -p egret-controlplane && cargo fmt -p egret-controlplane --check && cargo clippy -p egret-controlplane -- -D warnings 2>&1 | tail -6`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add crates/egret-controlplane/src/secrets.rs
git commit -m "feat(secrets): typed /secrets API (type + data, value alias, type/keys DTO)"
```

---

### Task 3: Regenerate the OpenAPI + orval client

**Files:**
- Modify: `openapi.json` (repo root) + `frontend/src/api/generated/*` (regenerated)

- [ ] **Step 1: Regenerate the OpenAPI spec**

Run (repo root): `cargo run -p egret-bin -- openapi > openapi.json`
Expected: `openapi.json` now shows the new `CreateSecretInput`/`UpdateSecretInput`/`SecretDto` fields (`type`, `data`, `keys`). (Confirm the exact subcommand — check `crates/egret-bin/src/main.rs` for how the OpenAPI is emitted; adjust if it's `openapi --out` etc.)

- [ ] **Step 2: Regenerate the frontend client**

Run: `cd frontend && npm run gen:api`
Expected: `src/api/generated/model/{createSecretInput,updateSecretInput,secretDto}.ts` gain the new fields; no manual edits.

- [ ] **Step 3: Commit**

```bash
git add openapi.json frontend/src/api/generated
git commit -m "chore(api): regenerate OpenAPI + orval client for typed secrets"
```

---

### Task 4: Frontend types + type-aware UI

**Files:**
- Modify: `frontend/src/api/controlPlane.ts`
- Modify: `frontend/src/routes/control-plane/ControlPlaneSecrets.tsx`

- [ ] **Step 1: Update the hand-written types**

In `frontend/src/api/controlPlane.ts`:

```ts
export interface Secret {
  id: string;
  name: string;
  description: string | null;
  created_at: string;
  used_by: number;
  type: string;
  keys: string[];
}

export interface SecretInput {
  name: string;
  description?: string | null;
  type?: string;
  data?: Record<string, string>;
  // Back-compat single value (Opaque{value}); prefer `type`+`data`.
  value?: string;
}

export interface SecretUpdate {
  name?: string;
  description?: string | null;
  type?: string;
  data?: Record<string, string>;
  value?: string;
}
```

(The `createSecret`/`updateSecret`/`listSecrets` wrappers pass the body through — no change needed beyond the types, as long as `gen:api` widened the generated `controlPlaneSecretsCreate` body.)

- [ ] **Step 2: Type-aware Add/Edit dialog**

In `ControlPlaneSecrets.tsx`, replace the single `value` field in the Add dialog with a **type Select** + conditional editors. Add state `const [type, setType] = useState("Opaque")` and per-type field state; on submit build `data` and post `{ name, description, type, data }`.

- A `Select` (`id="cp-secret-type"`) with options `Opaque`, `kubernetes.io/tls`, `kubernetes.io/basic-auth`.
- `Opaque`: a key→value rows editor — an array of `{key, value}` (start with one row `{key: "value", value: ""}`), add/remove-row buttons, `Input`s for key + password-type value. On submit, `data = Object.fromEntries(rows.filter(r => r.key))`.
- `kubernetes.io/tls`: two `Textarea`s — `tls.crt`, `tls.key`. On submit `data = { "tls.crt": crt, "tls.key": key }`.
- `kubernetes.io/basic-auth`: `username` `Input` + `password` `Input`. On submit `data = { username, password }`.

Keep the Name + Description inputs. Wire the submit `createMutation.mutate({ name, description, type, data })`.

- [ ] **Step 3: Show type + keys in the list**

In the secrets table, add a `type` Badge (use the existing `Badge` component) and a small muted line listing `secret.keys.join(", ")` under the name. Never render a value.

- [ ] **Step 4: Typecheck + lint**

Run: `cd frontend && npm run typecheck && npm run lint 2>&1 | tail -3`
Expected: clean (0 new warnings).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/api/controlPlane.ts frontend/src/routes/control-plane/ControlPlaneSecrets.tsx
git commit -m "feat(ui): type-aware secret console (Opaque/tls/basic-auth editors + type badge)"
```

> Note: the exact dialog markup + list layout are finalized against the live UI in Task 5 — expect minor tweaks (Textarea import, Select wiring, row-editor ergonomics).

---

### Task 5: Tests + acceptance

**Files:**
- Modify: `crates/egret-secrets/tests/workspace_secrets.rs`

- [ ] **Step 1: Backend typed tests**

Add to `crates/egret-secrets/tests/workspace_secrets.rs` (match the file's existing DB-gated harness — copy the neighbouring test's store/pool setup, do NOT invent):

```rust
// create_typed + list exposes type + key names (never the value).
async fn typed_named_secret_lists_type_and_keys(/* per existing harness */) {
    let store = /* WorkspaceSecretStore per existing tests */;
    use std::collections::BTreeMap;
    let mut tls = BTreeMap::new();
    tls.insert("tls.crt".into(), "CERT".into());
    tls.insert("tls.key".into(), "KEY".into());
    let id = store.create_typed(None, "wildcard", None, "kubernetes.io/tls", &tls).await.unwrap();
    let metas = store.list(None).await.unwrap();
    let m = metas.into_iter().find(|m| m.id == id).unwrap();
    assert_eq!(m.r#type, "kubernetes.io/tls");
    assert_eq!(m.keys, vec!["tls.crt".to_string(), "tls.key".to_string()]);
    // resolve_plaintext must still work per key path (P0 get_key) — value never in meta.

    // Legacy create still shows Opaque{value}.
    let lid = store.create(None, "legacy", None, "hunter2").await.unwrap();
    let lm = store.get_meta(None, lid).await.unwrap();
    assert_eq!(lm.r#type, "Opaque");
    assert_eq!(lm.keys, vec!["value".to_string()]);
}
```

- [ ] **Step 2: Run backend tests + gates**

Run (Postgres up): `DATABASE_URL=postgres://egret:egret@localhost:5432/egret cargo test -p egret-secrets -p egret-controlplane 2>&1 | tail -15`, then `cargo fmt --all --check && cargo build -p egret-bin`.
Expected: all pass; build clean (the DTO change re-embeds nothing, but confirms downstream compiles).

- [ ] **Step 3: Frontend gates**

Run: `cd frontend && npm run typecheck && npm run lint && npm test && npm run build 2>&1 | tail -3`
Expected: all pass.

- [ ] **Step 4: Live acceptance (opt-in)**

Against a running kind deploy, log in as admin, go to `/control-plane/secrets`, add a `kubernetes.io/tls` secret (paste any cert/key text) → it appears in the list as `kubernetes.io/tls` with keys `tls.crt, tls.key` and no value shown. Fix any selector/markup drift and re-run.

- [ ] **Step 5: Commit**

```bash
git add crates/egret-secrets/tests/workspace_secrets.rs
git commit -m "test(secrets): typed named-secret list shows type + keys, not values"
```

---

## Self-Review

- **Spec coverage:** `create_typed`/`update_typed` + meta `type`/`keys` (Task 1) ✓; typed DTOs + handlers with `value` alias (Task 2) ✓; OpenAPI + orval regen (Task 3) ✓; hand-written types + type-aware dialog + list badge (Task 4) ✓; tests + never-leak-value assertion + FE gates + live (Task 5) ✓.
- **No placeholders:** store methods, SQL, DTOs, handlers, and frontend types are complete; the UI dialog markup + the two "confirm against the crate" notes (CpError 400 ctor, `openapi` subcommand) are explicit finalize-live points.
- **Type consistency:** `create_typed`/`update_typed(ws, [id,] type, data: &BTreeMap)`; `WorkspaceSecretMeta { r#type, keys }` → `SecretDto { type, keys }`; frontend `Secret { type, keys }` / `SecretInput { type, data, value? }`.
- **Never leak values:** list/get_meta select `secret_data.key` only; DTO has no value field; UI renders keys, not values.
