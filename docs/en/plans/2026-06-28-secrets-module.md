# Secrets module Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make secrets a per-workspace, named, reusable entity; storage and database connections reference a named secret instead of sealing a value inline.

**Architecture:** A new `workspace_secrets` table (named layer) wraps the existing `secrets` vault. Storage (`workspace_storages.secret_ref`) and database (`databases.secret_id`) connections repoint their FK from `secrets` to `workspace_secrets`. A `WorkspaceSecretStore` in `egret-secrets` does create/list/rotate/delete/resolve; the storage and DB code resolve plaintext through it (2-hop). A migration backfills every existing inline secret into a named secret and repoints the connections.

**Tech Stack:** Rust (axum, sqlx 0.8, Postgres), egret-secrets / egret-catalog-engine / egret-database / egret-server; React 18 + Vite + TanStack Query + shadcn; Vitest.

**Spec:** `docs/en/specs/2026-06-28-secrets-module-design.md`. **Branch:** `feat/secrets-module` (off main, created).

**Key facts (verified):**
- Migration head is `migrations/0031_databases_per_workspace.sql` → new file is `0032_workspace_secrets.sql`.
- `egret-secrets`: `SecretStore { put(kind,plaintext)->Uuid, get(id)->String, delete(id) }` over `egret_metastore::Db`. `lib.rs` exports `Crypto, SecretsError, SecretStore`.
- **Storage registry** (`crates/egret-catalog-engine/src/browse.rs`): `BrowseState` already holds `secret_store: SecretStore`, `store: CatalogStore`, `authz`, and uses `WsContext` + `require_workspace_admin`. The registry conn store is `crates/egret-catalog/src/service/storage_conn.rs` (`WorkspaceStorageConn.secret_ref: Option<Uuid>`, `StorageConnInput.secret_ref: Option<Uuid>` — the STORE takes a pre-sealed Uuid; sealing happens in browse.rs). Seal site: `create_storage_conn` (~line 1305, `secret_store.put(...)→secret_ref`) and `update_storage_conn`. Resolve site: `conn_s3` (line 1516-1548, `secret_store.get(conn.secret_ref)`). HTTP DTOs: register body has `secret_key: String` (~1191, required), edit body `secret_key: Option<String>` (~1211).
- **Database** (`crates/egret-database`): `databases.secret_id` FK→secrets. `model.rs` `DatabaseInput.secret: Option<String>` (~72/84). `repo.rs` seals in `create_in_workspace` (~68) + `update_in_workspace` (~197) via `secrets.put("db_password",s)`; resolves in `open_secret` (~140, `secrets.get(secret_id)`). The `secrets: SecretStore` is injected into `DatabaseRepo`.
- The legacy SINGLE `workspace_storage` (singular) catalog-default table and `default_storage.secret_key` config are OUT OF SCOPE — only the REGISTRY (`workspace_storages` plural) + `databases` repoint.
- Test DB: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`; Postgres + MinIO + Trino running. Use `--test-threads=1`. Touch `crates/egret-metastore/src/lib.rs` after adding a migration (re-embed).
- The crypto master key for tests: `egret_secrets::Crypto::new(b"0123456789abcdef0123456789abcdef")` (32 bytes) — see `crates/egret-catalog-engine/tests/storage_folder_ops_api.rs`.

---

## Task 1: `workspace_secrets` table + backfill + `WorkspaceSecretStore` + `SecretStore::update`

**Files:**
- Create: `migrations/0032_workspace_secrets.sql`
- Modify: `crates/egret-secrets/src/store.rs` (add `SecretStore::update`)
- Create: `crates/egret-secrets/src/workspace_secrets.rs` (`WorkspaceSecretStore`)
- Modify: `crates/egret-secrets/src/lib.rs` (module + exports)
- Modify: `crates/egret-metastore/src/lib.rs` (whitespace touch → re-embed)
- Test: `crates/egret-secrets/tests/workspace_secrets.rs` (DB-gated)

- [ ] **Step 1: Migration `migrations/0032_workspace_secrets.sql`**
```sql
-- Named, per-workspace secrets (the managed layer above the `secrets` vault).
CREATE TABLE workspace_secrets (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name         text NOT NULL,
    description  text,
    secret_ref   uuid NOT NULL REFERENCES secrets(id),
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);
CREATE INDEX workspace_secrets_workspace_id_idx ON workspace_secrets(workspace_id);

-- Backfill STORAGE registry connections: wrap each inline vault ref into a named
-- secret, then repoint the connection. The vault id (secret_ref) is unique per
-- connection, so the UPDATE join is 1:1. Connection names are unique per
-- workspace, so the derived secret names are too.
WITH ins AS (
    INSERT INTO workspace_secrets (workspace_id, name, secret_ref)
    SELECT workspace_id, name || ' secret', secret_ref
    FROM workspace_storages
    WHERE secret_ref IS NOT NULL
    RETURNING id, secret_ref AS old_ref
)
UPDATE workspace_storages s
SET secret_ref = ins.id
FROM ins
WHERE s.secret_ref = ins.old_ref;

-- Backfill DATABASE connections the same way.
WITH ins AS (
    INSERT INTO workspace_secrets (workspace_id, name, secret_ref)
    SELECT workspace_id, name || ' secret', secret_id
    FROM databases
    WHERE secret_id IS NOT NULL AND workspace_id IS NOT NULL
    RETURNING id, secret_ref AS old_ref
)
UPDATE databases d
SET secret_id = ins.id
FROM ins
WHERE d.secret_id = ins.old_ref;

-- Repoint the FKs from the raw vault to the named layer.
ALTER TABLE workspace_storages DROP CONSTRAINT workspace_storages_secret_ref_fkey;
ALTER TABLE workspace_storages
    ADD CONSTRAINT workspace_storages_secret_ref_fkey
    FOREIGN KEY (secret_ref) REFERENCES workspace_secrets(id);

ALTER TABLE databases DROP CONSTRAINT databases_secret_id_fkey;
ALTER TABLE databases
    ADD CONSTRAINT databases_secret_id_fkey
    FOREIGN KEY (secret_id) REFERENCES workspace_secrets(id);
```
> If the actual FK constraint names differ, find them first: `SELECT conname FROM pg_constraint WHERE conrelid = 'workspace_storages'::regclass AND contype='f';` (and for `databases`). A `databases` row with a secret but NULL workspace_id (an ungranted orphan from Epic A) is skipped by the `workspace_id IS NOT NULL` guard — such a row keeps a now-dangling `secret_id` but is unreachable (filtered out everywhere); acceptable. Verify there are none in dev (`SELECT count(*) FROM databases WHERE secret_id IS NOT NULL AND workspace_id IS NULL;`).

- [ ] **Step 2: `SecretStore::update` (in-place rotation)** — `crates/egret-secrets/src/store.rs`:
```rust
/// Re-seal a new plaintext into an existing secret row (rotation). Referencing
/// rows keep their id, so consumers immediately read the new value.
pub async fn update(&self, id: Uuid, plaintext: &str) -> Result<(), SecretsError> {
    let (nonce, ct) = self.crypto()?.seal(plaintext)?;
    let n = sqlx::query("UPDATE secrets SET nonce=$2, ciphertext=$3, key_id=$4 WHERE id=$1")
        .bind(id)
        .bind(nonce)
        .bind(ct)
        .bind(&self.key_id)
        .execute(&self.db.pool)
        .await?;
    if n.rows_affected() == 0 {
        return Err(SecretsError::NotFound);
    }
    Ok(())
}
```

- [ ] **Step 3: `WorkspaceSecretStore`** — `crates/egret-secrets/src/workspace_secrets.rs`. Holds `db: Db` + `secrets: SecretStore`. A `WorkspaceSecretMeta { id, name, description, created_at, used_by }` struct (NO value). Methods:
```rust
// create: secrets.put("workspace_secret", plaintext) -> vault id;
//   INSERT workspace_secrets(workspace_id,name,description,secret_ref) RETURNING id.
//   On unique(workspace_id,name) violation -> SecretsError::AlreadyExists; roll back the vault put.
// list(workspace_id) -> Vec<WorkspaceSecretMeta> with used_by =
//   (SELECT count(*) FROM workspace_storages WHERE secret_ref = ws.id)
//   + (SELECT count(*) FROM databases WHERE secret_id = ws.id).
// get_meta(workspace_id, id) -> WorkspaceSecretMeta (404 if not in workspace).
// update_value(workspace_id, id, plaintext): look up secret_ref scoped to workspace
//   (404 if absent), then secrets.update(secret_ref, plaintext).
// rename(workspace_id, id, name, description): UPDATE ... WHERE id AND workspace_id;
//   unique violation -> AlreadyExists.
// delete(workspace_id, id): compute used_by; if > 0 -> SecretsError::InUse(n);
//   else read secret_ref, DELETE workspace_secrets row, secrets.delete(secret_ref).
// resolve_plaintext(workspace_id, id) -> String:
//   SELECT secret_ref FROM workspace_secrets WHERE id=$1 AND workspace_id=$2 (404),
//   then secrets.get(secret_ref).
```
Add `SecretsError::AlreadyExists(String)`, `SecretsError::InUse(i64)`, and (if missing) `SecretsError::NotFound` variants in `crates/egret-secrets/src/error.rs`.

- [ ] **Step 4: `lib.rs`** — `mod workspace_secrets;` + `pub use workspace_secrets::{WorkspaceSecretStore, WorkspaceSecretMeta};`.

- [ ] **Step 5: Touch `crates/egret-metastore/src/lib.rs`** (whitespace/comment) to re-embed the migration.

- [ ] **Step 6: Test `crates/egret-secrets/tests/workspace_secrets.rs`** (DB-gated; build a `SecretStore` with the 32-byte test key + a `WorkspaceSecretStore`; create a workspace row directly or via a helper). Assert:
```
// create(ws_a,"db-pw","",.."s3cret") -> id; resolve_plaintext(ws_a,id) == "s3cret".
// resolve_plaintext(ws_b,id) -> NotFound (IDOR).
// list(ws_a) shows it with used_by == 0; create with the same name -> AlreadyExists.
// update_value(ws_a,id,"rotated") -> resolve_plaintext == "rotated" (same id).
// Simulate a reference: INSERT a workspace_storages (or databases) row with secret_ref=id;
//   list used_by == 1; delete(ws_a,id) -> InUse(1). Remove the ref; delete -> Ok and the
//   vault `secrets` row is gone (secrets.get -> NotFound).
```

- [ ] **Step 7: Run + fmt + clippy**
```
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-secrets --test workspace_secrets -- --test-threads=1
cargo fmt -p egret-secrets -p egret-metastore -- --check
cargo clippy -p egret-secrets --all-targets
cargo build -p egret-secrets -p egret-catalog-engine -p egret-database -p egret-server
```
The build of the downstream crates confirms the migration/FK change did not break their existing `secret_ref`/`secret_id` usage (it shouldn't — they still bind a `Uuid`).

- [ ] **Step 8: Commit** `feat(secrets): workspace_secrets named layer + backfill + rotation`. Stage `migrations/`, `crates/egret-secrets`, `crates/egret-metastore/src/lib.rs`.

---

## Task 2: Secrets routes + rewire storage & database to named secrets

**Files:**
- Modify: `crates/egret-catalog-engine/src/browse.rs` (BrowseState + secrets router + storage seal/resolve)
- Modify: `crates/egret-database/src/{model.rs,repo.rs,routes.rs}` (secret_id input + resolve)
- Modify: `crates/egret-server/src/lib.rs` (inject `WorkspaceSecretStore`)
- Modify: `crates/egret-bin/src/main.rs` (construct `WorkspaceSecretStore`, pass to states)
- Test: `crates/egret-catalog-engine/tests/secrets_api.rs` + extend `crates/egret-database/tests/workspace_routes_api.rs`

- [ ] **Step 1: Add `workspace_secrets: WorkspaceSecretStore` to `BrowseState`** (browse.rs) and to `DatabaseState` (egret-database routes). Construct it in `crates/egret-bin/src/main.rs` (where `SecretStore` is built) and thread it into both states + `DatabaseRepo`.

- [ ] **Step 2: Secrets router in `browse.rs`** (mirror the storages routes; `WsContext` + `require_workspace_admin`):
```rust
// GET    /ws/{ws}/secrets          -> list (WorkspaceSecretMeta[], includes used_by)
// POST   /ws/{ws}/secrets          -> create { name, description?, value }  (201, returns meta)
// PUT    /ws/{ws}/secrets/{id}     -> { name?, description?, value? }  (rename and/or rotate)
// DELETE /ws/{ws}/secrets/{id}     -> 204, or 409 if InUse
```
Map `SecretsError::AlreadyExists` → 409, `InUse` → 409 (with the count in the body), `NotFound` → 404. The value is never echoed back.

- [ ] **Step 3: Storage rewire** (browse.rs). Change the register/edit DTOs: replace `secret_key: String`/`Option<String>` with `secret_id: Option<Uuid>` and `new_secret: Option<NewSecret>` where `NewSecret { name: String, value: String }`. In `create_storage_conn`/`update_storage_conn`, resolve the connection's `secret_ref` (a `workspace_secrets.id`) by precedence: if `new_secret` → `workspace_secrets.create(ws, name, None, value)` → its id; else if `secret_id` → validate it belongs to `ws` (`get_meta`) → use it; else on edit keep the existing `secret_ref`, on create leave `None`. Store that id as `StorageConnInput.secret_ref`. In **`conn_s3`** (line 1527-1534) change `st.secret_store.get(secret_ref)` → `st.workspace_secrets.resolve_plaintext(ws, secret_ref)`. `has_secret` stays `secret_ref.is_some()`.

- [ ] **Step 4: Database rewire** (egret-database). `model.rs`: `DatabaseInput.secret: Option<String>` → `secret_id: Option<Uuid>` + `new_secret: Option<NewSecret>`. `repo.rs`: `create_in_workspace`/`update_in_workspace` resolve a `secret_id` (named) by the same precedence (the repo gets a `WorkspaceSecretStore` handle, or the route resolves the id before calling the repo — prefer resolving in the **route** so the repo just stores a `secret_id: Option<Uuid>`). `open_secret` (line 140): **keep its `open_secret(id)` signature so the engine `job.rs` needs NO change** (same principle as Epic A — the engine resolves by id post-authorize). Change its body to `SELECT secret_id, workspace_id FROM databases WHERE id = $1`, and if both are present call `self.workspace_secrets.resolve_plaintext(workspace_id, secret_id)`; a NULL `workspace_id` (orphan) or NULL `secret_id` → `Ok(None)`. This requires injecting a `WorkspaceSecretStore` handle into `DatabaseRepo` (alongside the existing `secrets: SecretStore`).

- [ ] **Step 5: Tests.** `crates/egret-catalog-engine/tests/secrets_api.rs` (mirror `storage_registry_api.rs`): secrets CRUD scoped to a workspace (create; list w/ used_by; cross-ws id → 404; non-admin → 403; rotate via PUT; delete blocked 409 when a storage conn references it, allowed otherwise). Extend the storage + database route tests to register a connection via `new_secret`/`secret_id` and confirm `has_secret` + that the file browser / a DB test-connection resolves the secret. `cargo test -p egret-catalog-engine -p egret-database -- --test-threads=1`; `cargo build --workspace`; fmt; clippy.

- [ ] **Step 6: Commit** `feat(secrets): /ws/{ws}/secrets routes; storage + database reference named secrets`.

---

## Task 3: Frontend — SecretsPage + SecretPicker + wire into Storage & DB forms

**Files:**
- Create: `frontend/src/api/secrets.ts`
- Create: `frontend/src/routes/explore/SecretsPage.tsx` + `__tests__/SecretsPage.test.tsx`
- Create: `frontend/src/components/SecretPicker.tsx`
- Modify: `frontend/src/components/layout/Sidebar.tsx` (Secrets under MANAGE)
- Modify: `frontend/src/api/storage.ts` + `StoragePage.tsx` (secret_key → SecretPicker → `secret_id`/`new_secret`)
- Modify: `frontend/src/api/databases.ts` + `DatabasesPage.tsx` (secret → SecretPicker)
- Modify: route table (add `secrets`)

- [ ] **Step 1: `api/secrets.ts`** — mirror `storage.ts`. Types: `WorkspaceSecret { id, name, description?, created_at, used_by }`, `SecretInput { name, description?: string; value: string }`, `SecretUpdate { name?: string; description?: string; value?: string }`. Hooks: `useSecrets(ws)` (GET `/ws/${ws}/secrets`), `useCreateSecret(ws)`, `useUpdateSecret(ws)` (PUT `/ws/${ws}/secrets/${id}`), `useDeleteSecret(ws)`. `secretKeys.all(ws)`.

- [ ] **Step 2: `SecretsPage.tsx`** — mirror StoragePage: table **Name · Description · Used by · Created · Actions**; Add/Edit dialog (name, description, **value** write-only; on edit, blank value = keep, a filled value = rotate); Delete via ConfirmDeleteDialog, **disabled with a tooltip "in use by N connections"** when `used_by > 0` (and surface the 409 message if the server rejects). 403 → admin notice. Value never rendered.

- [ ] **Step 3: `SecretPicker.tsx`** — a controlled component used by the connection forms. Props `{ ws, value: { secret_id?: string } , onChange }`. Renders a shadcn `Select` listing `useSecrets(ws)` named secrets + a final item "＋ New secret". Selecting an existing secret → `onChange({ secret_id })`. Selecting "＋ New secret" reveals inline `name` + `value` inputs and emits `onChange({ new_secret: { name, value } })`. Shows the current reference on edit (the connection's `has_secret` + secret name if available). The form submits whichever of `secret_id` / `new_secret` the picker produced.

- [ ] **Step 4: Wire into Storage form** (`storage.ts` + `StoragePage.tsx`): `StorageConnInput` replaces `secret_key?: string|null` with `secret_id?: string` + `new_secret?: { name; value }`. Replace the Secret-key `Input` in the dialog with `<SecretPicker ws=... value=... onChange=... />`. Keep `access_key` inline. Update the storage tests' payload expectations.

- [ ] **Step 5: Wire into Database form** (`databases.ts` + `DatabasesPage.tsx`): `DatabaseConnInput` replaces `secret: string|null` with `secret_id?: string` + `new_secret?: { name; value }`. Replace the Password `Input` with `<SecretPicker />`. Update the database tests.

- [ ] **Step 6: Sidebar + route.** Add `Secrets` to the MANAGE NavGroup (after Databases; a `KeyRound` or `Lock` lucide icon). Register `/explore/:workspace/secrets` → `<SecretsPage />`.

- [ ] **Step 7: Tests + gate.** `SecretsPage.test.tsx` (mirror StoragePage.test.tsx): list incl. used_by; create/edit(rotate)/delete; delete disabled when used_by>0; value not rendered. A `SecretPicker.test.tsx`: pick-existing emits `secret_id`; create-new emits `new_secret`. Update the storage + database page tests for the new payload shape. `cd frontend && npm run lint && npm run typecheck && npm test && npm run build` — all green.

- [ ] **Step 8: Commit** `feat(web): Secrets page + SecretPicker; storage & database forms reference named secrets`.

---

## Self-review (controller, before final review)
- [ ] Migration: `workspace_secrets` created; storage + database inline secrets backfilled into named secrets + connections repointed; FKs swapped to `workspace_secrets`; metastore lib touched; orphan-db guard noted.
- [ ] `SecretStore::update` rotates in place; `WorkspaceSecretStore` create/list(used_by)/get_meta/update_value/rename/delete(InUse)/resolve_plaintext all workspace-scoped (cross-ws → NotFound).
- [ ] `/ws/{ws}/secrets` gated `workspace:manage_members`; AlreadyExists/InUse → 409; value never returned.
- [ ] Storage `conn_s3` + create/update resolve through `WorkspaceSecretStore`; DB `open_secret` + create/update do too; engine job passes/derives the workspace id.
- [ ] FE: SecretsPage in MANAGE; SecretPicker wired into BOTH connection forms; value write-only; delete-when-used disabled.
- [ ] **Before merge:** full local CI (fmt/clippy/`cargo test --workspace`, FE lint/typecheck/test/build) + live smoke (create a named secret; point a storage conn at it → browse files; point a DB conn at it → SQL Lab query; rotate the value) — GitHub Actions out of quota.

## Out of scope
Ranger secret rewiring; cross-workspace secret sharing; scheduled rotation; secret versioning/audit; GCS/Azure backends.
