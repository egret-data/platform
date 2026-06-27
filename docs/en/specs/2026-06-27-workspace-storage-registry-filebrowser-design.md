# Workspace Storage — registry (N storages) + file browser — Design

**Date:** 2026-06-27
**Status:** Design — approved
**Branch:** `feat/workspace-storage-registry` (off main)
**Parent:** ADR-19/19a (Workspace Admin / MANAGE zone). Re-architects the single-storage model merged in PR #73.

## 0. Goal & re-architecture

The merged model coupled storage to the catalog: one auto-provisioned warehouse per workspace on one fixed storage. The new model **decouples storage from catalog** (the lakekeeper model):

- **Storage** = a per-workspace **registry of N storage connections** (each = an S3/MinIO bucket connection with a name + purpose) **+ a file browser/manager** (MinIO-console-like: navigate, view, upload, download, delete objects in a registered storage).
- **Catalog** (a LATER epic) = create/manage warehouses that each **consume a chosen registered storage** (lakekeeper `create_warehouse`, N per workspace).

**This epic delivers the Storage area only.** Catalog keeps using its current auto-provision model (the old `workspace_storage` table + lazy provisioning) UNCHANGED until the Catalog epic rewires it onto the registry and removes the old path. The two coexist temporarily; nothing in this epic touches catalog provisioning.

## 1. Decisions (locked)
- **New table `workspace_storages`** (N per workspace, own `id` PK) — additive; the existing `workspace_storage` (1-per-ws, used by catalog auto-provision) is left in place for now.
- **Secrets** via `egret-secrets` (each connection's secret_key → `secret_ref`); never returned/logged.
- **All Storage routes gated `workspace:manage_members`** (workspace-admin).
- **File browser is full**: list (folder navigation via prefix+delimiter), download, upload, delete. Scoped to one registered storage's bucket (operations confined to the bucket + an optional `key_prefix` root for the connection).
- **S3 only** for now (`provider` reserved for GCS/Azure). Connection registration **validates reachability** (a real S3 head/list) before saving.

## 2. Backend

### 2.1 Schema (migration `00NN_workspace_storages.sql`)
```sql
CREATE TABLE workspace_storages (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name         text NOT NULL,
  purpose      text,                       -- free-form, e.g. "warehouse", "uploads", "results"
  provider     text NOT NULL DEFAULT 's3',
  endpoint     text,                       -- NULL = AWS
  region       text NOT NULL,
  bucket       text NOT NULL,
  key_prefix   text,                       -- optional root prefix for this connection
  access_key   text NOT NULL,
  secret_ref   uuid,                        -- → egret-secrets
  path_style   boolean NOT NULL DEFAULT true,
  created_at   timestamptz NOT NULL DEFAULT now(),
  UNIQUE (workspace_id, name)
);
```
(Use the next free migration number; touch `egret-metastore/src/lib.rs` to re-embed.)

### 2.2 Store (`egret-catalog`, sqlx 0.8 — alongside the existing storage store)
`WorkspaceStorageConn { id, workspace_id, name, purpose, provider, endpoint, region, bucket, key_prefix, access_key, secret_ref, path_style, created_at }` (no secret value). Methods on `CatalogStore`: `create_storage_conn`, `list_storage_conns(ws)`, `get_storage_conn(ws, id)`, `update_storage_conn`, `delete_storage_conn(ws, id)` — all workspace-scoped (IDOR-safe: every read/update/delete filters `workspace_id = $ws`).

### 2.3 `ObjectStore::list` (`egret-storage`)
Extend the `ObjectStore` trait (currently `put`/`get`/`delete`) with:
```rust
async fn list(&self, prefix: &str, delimiter: Option<&str>) -> Result<Listing, StorageError>;
// Listing { folders: Vec<String> /* common prefixes */, objects: Vec<ObjectMeta> }
// ObjectMeta { key: String, size: i64, last_modified: Option<String> }
```
Implement in `S3Store` via `aws_sdk_s3` `list_objects_v2` (`.prefix()`, `.delimiter("/")`, paginate). The browser passes `delimiter="/"` for folder-style navigation.

### 2.4 Routes (`egret-catalog-engine/src/browse.rs` — has `secret_store` + auth; or a dedicated `storage_admin` module). All gated `workspace:manage_members`, all build an `S3Store` from a registered connection's config (creds from egret-secrets).
**Registry:**
- `GET /ws/{ws}/storages` → `[WorkspaceStorageConnDto]` (no secret).
- `POST /ws/{ws}/storages` `{ name, purpose?, provider, endpoint?, region, bucket, key_prefix?, access_key, secret_key, path_style }` → validate reachability (build `S3Store`, do a bounded `list(prefix, "/")` or head-bucket; fail → 400 with the message) → store (secret→egret-secrets) → 201.
- `PUT /ws/{ws}/storages/{id}` → edit (secret_key optional = keep current) → re-validate → update.
- `DELETE /ws/{ws}/storages/{id}` → delete the connection row + (optionally) its secret. (Does NOT touch objects in the bucket.)
**File browser (scoped to a connection):**
- `GET /ws/{ws}/storages/{id}/objects?prefix=<p>` → resolve the connection → `S3Store::list(connection.key_prefix + p, "/")` → `{ prefix, folders: [..], objects: [{key, size, last_modified}] }`. Confine all keys under the connection's `key_prefix` root (never list outside it).
- `GET /ws/{ws}/storages/{id}/objects/content?key=<k>` → `get` → stream the bytes (Content-Disposition attachment) for download. Cap/refuse very large objects (e.g. a size limit) to avoid loading huge blobs into memory.
- `POST /ws/{ws}/storages/{id}/objects?key=<k>` (body = bytes / multipart) → `put`. Confine `key` under the connection root.
- `DELETE /ws/{ws}/storages/{id}/objects?key=<k>` → `delete`. Confine under root.
- **Path safety:** reject `key`/`prefix` containing `..` or escaping the connection's `key_prefix` root (prevent traversal outside the intended scope).

## 3. Frontend (`/explore/:ws/storage` — replaces the merged single-storage editable page)
A Storage AREA with two views (sub-nav or master/detail):
- **Connections** (registry): a list/table of registered storages (name · purpose · provider · bucket · endpoint). **Add** (dialog/form: name, purpose, endpoint, region, bucket, key_prefix, access_key, secret_key, path_style → validates server-side), **Edit**, **Delete** (confirm). Secret write-only ("leave blank to keep current"); never rendered.
- **File browser** (per selected connection): a MinIO-console-like manager — breadcrumb path, folder/file list (folders navigable, files show size + modified), **Upload** (file picker → POST), **Download** (GET content), **Delete** (confirm), back/up navigation. 403 → admin notice; empty → "No objects".
- Hooks in `src/api/storage.ts` (new) or `catalog.ts`: `useStorageConns`, `useCreateStorageConn`, `useUpdateStorageConn`, `useDeleteStorageConn`, `useStorageObjects(connId, prefix)`, `useUploadObject`, `useDeleteObject`, download via a direct link. (Paths WITHOUT `/api` prefix — `apiFetch` adds it.)

## 4. Testing
- **BE (DB+MinIO):** registry CRUD (create validates reachability; bad creds → 400; list/get/update/delete workspace-scoped; cross-workspace id → 404 IDOR; non-admin → 403; no secret in any response). File browser: upload an object then `list` shows it (+ folder grouping via delimiter), download returns the bytes, delete removes it; a `key`/`prefix` with `..` or outside the connection root → 400; non-admin → 403. `ObjectStore::list` unit/integration against MinIO.
- **FE (Vitest, mocked):** Connections list/add/edit/delete; secret never rendered; file browser lists folders+files, navigates prefixes, upload/download/delete call the right hooks; 403 notice.

## 5. File map
- BE: migration; `egret-catalog/src/service/` storage-conn store (+ test); `egret-storage/src/{lib.rs,s3.rs}` (`list` + `Listing`/`ObjectMeta`); `egret-catalog-engine/src/browse.rs` (or new `storage_admin.rs`) registry + object routes (+ test); egret-bin wiring if a new router.
- FE: `src/api/storage.ts` (hooks); `src/routes/explore/StoragePage.tsx` rewritten as the Storage area (Connections + File browser); components for the file browser; tests.

## 6. Risks
- **Secret hygiene** across N connections: secret_key write-only, stored via egret-secrets, never returned/logged.
- **Path traversal / scope escape:** confine all object keys under the connection's `key_prefix` root; reject `..`. A bug here = reading/writing arbitrary bucket paths.
- **Large objects:** cap download size / stream; don't load huge blobs into memory.
- **Reachability validation:** registration does a live S3 call; a bad config fails loudly (400) — good.
- **Coexistence:** this epic does NOT change catalog provisioning (the old `workspace_storage` + auto-provision still serve the catalog). The Catalog epic later migrates catalog onto the registry and removes the old path — call that out so the two storage concepts don't confuse during the interim.

## 7. Out of scope (later)
- **Catalog epic:** create/manage warehouses choosing a registered storage (lakekeeper `create_warehouse`, N per ws); migrate catalog off the old `workspace_storage` + remove auto-provision + the old table; then full catalog-console parity.
- GCS/Azure providers; multipart/resumable uploads; object versioning; per-purpose wiring (which storage backs warehouse vs results vs uploads).
