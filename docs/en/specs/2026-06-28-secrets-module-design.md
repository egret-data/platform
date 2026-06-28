# Secrets module — per-workspace named secrets — Design

**Date:** 2026-06-28 · **Status:** Design — approved · **Branch:** `feat/secrets-module` (off main)
**Program:** Epic B of the "DB-per-workspace + Secrets" program. Depends on Epic A (DB connections are now per-workspace, PR #78).

## Goal
Turn secrets into a **managed, per-workspace, named, reusable** entity. The existing `secrets` vault (AES-256-GCM) stays as the crypto layer; a new **named layer** sits above it; storage and database connections **reference a named secret** instead of sealing a value inline.

## Background (current state)
- `secrets` table is the crypto vault: `(id, kind, nonce, ciphertext, key_id, created_at)`. `egret_secrets::SecretStore` exposes `put(kind, plaintext) -> Uuid`, `get(id) -> String`, `delete(id)`.
- **Storage** (`crates/egret-catalog-engine/src/browse.rs`): connection create/update seals `req.secret_key` via `secret_store.put("workspace_storage_conn", s)` and stores the returned id as `workspace_storages.secret_ref` (FK → `secrets`). Resolution: `secret_store.get(secret_ref)`. `has_secret = secret_ref.is_some()`.
- **Database** (`crates/egret-database/src/repo.rs`): seals via `secrets.put("db_password", s)` → `databases.secret_id` (FK → `secrets`). Resolution: `open_secret` → `secrets.get(secret_id)`.
- Both create an **anonymous** `secrets` row inline — no name, not reusable, invisible to the user.
- The config-level `default_storage.secret_key` (a global bootstrap default) is NOT a connection and is out of scope (left inline).

## 1. Data model
New table **`workspace_secrets`** (one named, reusable, per-workspace secret = one sensitive value wrapping one vault row):
```sql
CREATE TABLE workspace_secrets (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name         text NOT NULL,
    description  text,
    secret_ref   uuid NOT NULL REFERENCES secrets(id),   -- the sealed value in the vault
    created_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);
CREATE INDEX workspace_secrets_workspace_id_idx ON workspace_secrets(workspace_id);
```
**Repoint the connection FKs** from the raw vault to the named layer:
- `workspace_storages.secret_ref` → references `workspace_secrets(id)` (rename to `secret_id` for clarity is optional; keep `secret_ref` to minimise churn but it now points at `workspace_secrets`).
- `databases.secret_id` → references `workspace_secrets(id)`.
Resolution of a connection's plaintext is a 2-hop join: `conn.<secret col>` → `workspace_secrets.secret_ref` → `secrets` → decrypt. A connection's secret column is **nullable** (a connection may have no secret).

## 2. Store layer — extend `egret-secrets`
Add a `WorkspaceSecretStore` (same `egret_metastore::Db` pool, reusable by both the storage and database crates which already depend on `egret-secrets`). Methods (all workspace-scoped → cross-workspace id is `NotFound`, IDOR-safe):
- `create(workspace_id, name, description, plaintext) -> Uuid` — `SecretStore::put` the value, insert a `workspace_secrets` row, return its id. Unique `(workspace_id, name)` → a clear `AlreadyExists`.
- `list(workspace_id) -> Vec<WorkspaceSecretMeta>` — id, name, description, created_at, **`used_by` count** (how many storage + database connections reference it).
- `get_meta(workspace_id, id)` — metadata only (never the plaintext).
- `update_value(workspace_id, id, plaintext)` — **rotation**: re-seal into the SAME `secrets` row (add `SecretStore::update(id, plaintext)` which re-seals nonce+ciphertext in place). Every referencing connection immediately uses the new value; no FK changes.
- `rename(workspace_id, id, name, description)` — metadata update.
- `delete(workspace_id, id)` — **blocked if in use**: if any `workspace_storages` or `databases` row references it → `Conflict("in use by N connections")`; else delete the `workspace_secrets` row + its vault `secrets` row.
- `resolve_plaintext(workspace_id, id) -> String` — the 2-hop lookup + decrypt; used by the storage and DB connection paths.
The secret value is **write-only** everywhere — never returned by any list/get DTO, never logged.

## 3. Backfill (migration)
For every connection with an inline secret today, wrap it into a named secret and repoint:
- For each `workspace_storages` row with a non-null `secret_ref` (currently → `secrets`): insert a `workspace_secrets` row `(workspace_id = the conn's workspace, name = "<conn.name> secret", secret_ref = the existing vault id)`, then set the storage row's `secret_ref` to the new `workspace_secrets.id`.
- For each `databases` row with a non-null `secret_id`: same — wrap the vault id into a `workspace_secrets` (name `"<db.name> secret"`, workspace = the db's `workspace_id` from Epic A), then repoint `databases.secret_id`.
- The underlying `secrets` rows are reused 1:1 (no re-encryption). Non-destructive. Add the new FK constraints AFTER the backfill so they validate. Touch `crates/egret-metastore/src/lib.rs` to force the embedded migrator to re-embed.
- **Naming collisions:** if two connections in one workspace would produce the same `"<name> secret"`, suffix with a short id fragment to satisfy `UNIQUE(workspace_id, name)`.

## 4. Backend rewire
- **Storage** (`browse.rs`): the connection create/update body carries a `secret_id` (an existing named secret) and/or an inline `{ secret_name, secret_value }` to create-and-reference. The handler: if `secret_id` given → validate it belongs to the workspace; else if inline value given → `WorkspaceSecretStore::create` then use its id; else keep the existing reference (edit with no change). When building the `S3Store`, resolve the plaintext via `resolve_plaintext`. `has_secret = secret_id.is_some()`.
- **Database** (`egret-database`): `DatabaseInput` replaces the raw `secret: Option<String>` with `secret_id: Option<Uuid>` (+ the same optional inline create-and-reference). `create_in_workspace`/`update_in_workspace` store/keep `secret_id`; `open_secret` resolves via `resolve_plaintext`. Update keeps the existing reference when none supplied.
- **Routes** `/ws/{ws}/secrets` (new, gated `workspace:manage_members`, IDOR-safe, mirror the storages/databases routers):
  - `GET /ws/{ws}/secrets` (list w/ used_by), `POST /ws/{ws}/secrets` (create), `PUT /ws/{ws}/secrets/{id}` (rename/description and/or rotate value), `DELETE /ws/{ws}/secrets/{id}` (blocked-if-in-use → 409).

## 5. Frontend
- **SecretsPage** (`src/routes/explore/SecretsPage.tsx`), MANAGE zone, mirroring StoragePage/DatabasesPage: registry table **Name · Description · Used by · Created · Actions**; Add/Edit dialog (name, description, **value** write-only — blank on edit keeps the existing value); Delete via `ConfirmDeleteDialog`, disabled / 409-explained when `used_by > 0`. 403 → admin notice. `src/api/secrets.ts` hooks (`useSecrets`, `useCreateSecret`, `useUpdateSecret`, `useDeleteSecret`), paths `/ws/${ws}/secrets`.
- **Sidebar**: add **Secrets** under MANAGE (after Databases).
- **Secret picker** component (`src/components/SecretPicker.tsx`, reused by the Storage and Database connection forms): a `Select` of the workspace's named secrets + a "＋ New secret" item that reveals inline `name` + `value` inputs (creates a named secret on submit and references it). Replaces the raw `secret_key` (Storage) and `password` (Database) inputs. The storage `access_key` input stays inline.

## 6. Testing
- **Backend (DB-gated):** `workspace_secrets` CRUD scoped to a workspace (cross-workspace id → 404; non-admin member → 403); rotation (`update_value` → `resolve_plaintext` returns the new value, referencing connection sees it); delete blocked when referenced (→ 409) and allowed when not (removes the vault row too); backfill migration test (each pre-existing inline secret becomes a named secret + the connection is repointed + `resolve_plaintext` returns the original value); storage + database connections still build/connect through a named secret.
- **Frontend (Vitest, mocked):** SecretsPage list/create/edit(rotate)/delete (+ disabled delete when used); SecretPicker pick-existing and create-new flows wired into the Storage and Database forms; value never rendered as plaintext.
- **Manual + full local CI** before merge (GitHub Actions out of quota): `cargo fmt --all --check`, `cargo clippy --workspace`, `cargo test --workspace`, FE lint/typecheck/test/build; live smoke — create a named secret, point a storage connection at it (browse files), point a DB connection at it (run a SQL Lab query), rotate the value.

## 7. Risks
- **2-hop resolution** must be correct in BOTH the storage and DB paths — a wrong join = a connection silently loses its secret. Covered by the live smoke + resolution tests.
- **Backfill atomicity** — wrap + repoint must be one transaction per connection so a connection is never left pointing at a dropped/!named ref. Run inside the migration transaction.
- **Delete protection** — must count references across BOTH `workspace_storages` and `databases` (and be re-checked at delete time) to avoid orphaning a connection.
- **Write-only hygiene** — the value must never appear in any DTO, list, or log.

## 8. Out of scope
- Ranger secret rewiring; cross-workspace secret sharing; scheduled/automatic rotation; secret versioning / audit history; real GCS/Azure storage backends.
