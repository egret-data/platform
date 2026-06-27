# Workspace Storage — registry + file browser — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** A per-workspace Storage area = a registry of N S3/MinIO storage connections + a MinIO-console-like file browser (list/upload/download/delete). Decouples storage from catalog (catalog provisioning is untouched this epic).

**Architecture:** New `workspace_storages` table (N/ws). `ObjectStore::list` added to egret-storage. Storage routes (registry CRUD + object ops) in egret-catalog-engine's browse router (has secret_store + auth), each building an `S3Store` from a registered connection. FE: Storage area (Connections + File browser) replacing the merged single-storage page.

**Tech Stack:** Rust (axum, sqlx 0.8 egret-catalog, aws-sdk-s3, egret-secrets), React 18 + TanStack Query + Vitest.

**Branch:** `feat/workspace-storage-registry` (off main). BE tests: DB+MinIO (`DATABASE_URL`, two `psql -c` for reset). FE: `frontend/`. **Run full local CI before merge** (GitHub quota out).

**Spec:** `docs/en/specs/2026-06-27-workspace-storage-registry-filebrowser-design.md`.

**Verified seams:**
- `egret-storage`: `pub trait ObjectStore { put/get/delete }`; `S3Store` uses `aws_sdk_s3::Client` (so `list_objects_v2` is available). `StorageConfig { endpoint:Option<String>, region, bucket, access_key, secret_key, path_style }`.
- `egret-catalog` `CatalogStore` (sqlx 0.8): existing `workspace_storage_for`/`upsert_workspace_storage` (the OLD 1-per-ws table — leave it). `CatalogError`. `pool()`.
- `egret-catalog-engine/src/browse.rs` `BrowseState { engine, store: CatalogStore, secret_store: egret_secrets::SecretStore, default_storage: StorageConfig, system_user, sessions, workspaces, authz }`; `require_workspace_admin(&st,&ctx)`; `WsContext(ctx)`; routes on `browse_router` mounted at `/api`.
- `egret_secrets::SecretStore`: `put(kind,&str)->Uuid`, `get(Uuid)->String`.
- Latest migration is `0029_workspace_storage.sql` → use `0030`.

---

## Task 1: `workspace_storages` table + store (BE)
**Files:** `migrations/0030_workspace_storages.sql`; touch `egret-metastore/src/lib.rs`; `egret-catalog/src/service/storage_conn.rs` (+ export); test `egret-catalog/tests/store_storage_conn.rs`.
- [ ] Migration: the `workspace_storages` table from spec §2.1 (own `id` PK, `UNIQUE(workspace_id, name)`, FK CASCADE). Reset test DB.
- [ ] `WorkspaceStorageConn` struct (no secret value) + `CatalogStore` methods: `create_storage_conn`, `list_storage_conns(ws)`, `get_storage_conn(ws, id)`, `update_storage_conn`, `delete_storage_conn(ws, id) -> bool` — every method workspace-scoped (filters `workspace_id`). Bound SQL, `CatalogError`.
- [ ] Tests: create→list (no secret); get by (ws,id); cross-workspace get/update/delete → None/false (IDOR); unique-name conflict → error; delete returns bool. `cargo test -p egret-catalog`; fmt; clippy. Commit `feat(catalog): workspace_storages registry store`.

## Task 2: `ObjectStore::list` (BE)
**Files:** `egret-storage/src/lib.rs` (trait + `Listing`/`ObjectMeta`), `egret-storage/src/s3.rs` (impl); test `egret-storage/tests/s3_list.rs` (DB-free, MinIO only).
- [ ] Add to the trait: `async fn list(&self, prefix: &str, delimiter: Option<&str>) -> Result<Listing, StorageError>` with `pub struct Listing { pub folders: Vec<String>, pub objects: Vec<ObjectMeta> }`, `pub struct ObjectMeta { pub key: String, pub size: i64, pub last_modified: Option<String> }`.
- [ ] `S3Store::list`: `client.list_objects_v2().bucket(...).prefix(prefix).set_delimiter(delimiter.map(str::to_string))` + paginate (continuation token); map `common_prefixes` → `folders`, `contents` → `objects` (key/size/last_modified as RFC3339 string).
- [ ] Test (MinIO): put a few objects under `t/`, `t/a.txt`, `t/sub/b.txt`; `list("t/", Some("/"))` → folders contains `t/sub/`, objects contains `t/a.txt`; `list("t/", None)` → flat list. `cargo test -p egret-storage`; fmt; clippy. Commit `feat(storage): ObjectStore::list (folder-style S3 listing)`.

## Task 3: registry routes (BE)
**Files:** `egret-catalog-engine/src/browse.rs` (or new `storage_admin.rs` + wire into the router); test `egret-catalog-engine/tests/storage_registry_api.rs`.
- [ ] DTOs: `WorkspaceStorageConnDto` (no secret), `CreateStorageConnRequest { name, purpose?, provider, endpoint?, region, bucket, key_prefix?, access_key, secret_key, path_style }`, `UpdateStorageConnRequest` (secret_key optional).
- [ ] Routes (gate `workspace:manage_members`, `WsContext`):
  - `GET /ws/{ws}/storages` → `list_storage_conns` → DTOs.
  - `POST /ws/{ws}/storages` → validate reachability: build a `StorageConfig` from the request + `egret_storage::S3Store::new(&cfg)` then `list(key_prefix.unwrap_or(""), Some("/"))` (a bounded call) — on error → 400 with the message; on success → `secret_store.put` + `create_storage_conn`; 201.
  - `PUT /ws/{ws}/storages/{id}` → resolve effective secret (req or existing), re-validate, `update_storage_conn`.
  - `DELETE /ws/{ws}/storages/{id}` → `delete_storage_conn(ws,id)` → 204/404.
- [ ] Tests (DB+MinIO): create with valid MinIO creds → 201 + appears in list (no secret); bad creds → 400; cross-workspace id on GET/PUT/DELETE → 404 (IDOR); non-admin → 403. `cargo test -p egret-catalog-engine`; clippy; single iceberg. Commit `feat(catalog-engine): workspace storage registry routes (CRUD + validate)`.

## Task 4: file browser object routes (BE)
**Files:** `egret-catalog-engine/src/browse.rs`/`storage_admin.rs`; test extend `storage_registry_api.rs` or new `storage_objects_api.rs`.
- [ ] A helper `conn_store(&st, ws, id) -> S3Store` (resolve the connection, fetch secret, build `S3Store`) → 404 if no such connection. A path-safety helper: reject `key`/`prefix` containing `..` or not starting under the connection's `key_prefix` root; the EFFECTIVE key = `<conn.key_prefix>/<user path>` (confine).
- [ ] Routes (gate admin):
  - `GET /ws/{ws}/storages/{id}/objects?prefix=<p>` → `store.list(root+p, Some("/"))` → `{ prefix: p, folders, objects }` (strip the root prefix from returned keys so the UI sees workspace-relative paths).
  - `GET /ws/{ws}/storages/{id}/objects/content?key=<k>` → `store.get(root+k)` → body with `Content-Disposition: attachment; filename=...`. Refuse if the object exceeds a size cap (e.g. read the size via list/head first, or stream).
  - `POST /ws/{ws}/storages/{id}/objects?key=<k>` (body bytes) → `store.put(root+k, bytes)` → 201.
  - `DELETE /ws/{ws}/storages/{id}/objects?key=<k>` → `store.delete(root+k)` → 204.
- [ ] Tests (DB+MinIO): upload `hello.txt` → list shows it under the right prefix → download returns the bytes → delete removes it; a `key` with `..` or escaping root → 400; non-admin → 403; folders appear via delimiter. `cargo test -p egret-catalog-engine -p egret-server`; `cargo build --workspace`; clippy; fmt; single iceberg. Commit `feat(catalog-engine): storage file browser object routes (list/upload/download/delete)`.

## Task 5: FE — Connections registry
**Files:** `frontend/src/api/storage.ts` (new); `frontend/src/routes/explore/StoragePage.tsx` (rewrite as the Storage area shell + Connections view); components; tests.
- [ ] `storage.ts` hooks: `useStorageConns(ws)`, `useCreateStorageConn(ws)`, `useUpdateStorageConn(ws)`, `useDeleteStorageConn(ws)` (TanStack Query; `apiJson`/`apiFetch`, paths `/ws/${ws}/storages...`, invalidate on mutate; let `ApiError` propagate).
- [ ] StoragePage: a header + (for now) a "Connections" view: a table of connections (name/purpose/provider/bucket/endpoint) + **Add** (dialog form, secret write-only, server-validated → show 400 message), **Edit**, **Delete** (confirm). 403 → admin notice. Selecting a connection opens its file browser (Task 6) — for this task, a stub/placeholder is fine.
- [ ] Tests (mock `@/api/storage`): list renders (no secret); add submits create; edit/delete call mutations; 403 notice. `npm run lint && typecheck && test && build`. Commit `feat(web): workspace storage connections registry`.

## Task 6: FE — file browser
**Files:** `frontend/src/routes/explore/storage/FileBrowser.tsx` + components; `storage.ts` (object hooks); tests.
- [ ] `storage.ts`: `useStorageObjects(ws, connId, prefix)` (GET objects), `useUploadObject(ws, connId)` (POST), `useDeleteObject(ws, connId)` (DELETE); download via a direct `apiFetch` GET → blob → save.
- [ ] `FileBrowser`: breadcrumb of the current prefix; a list of folders (click → navigate into) + files (name, size, modified) with Download + Delete (confirm) actions; an **Upload** control (file input → read bytes → POST to the current prefix); an Up/Back control; empty state; 403 notice. Wire it into the StoragePage when a connection is selected.
- [ ] Tests (mock): renders folders+files from a mocked listing; clicking a folder navigates (prefix changes → refetch); upload calls `useUploadObject`; delete calls `useDeleteObject` after confirm; download triggers the fetch. `npm run lint && typecheck && test && build`. Commit `feat(web): storage file browser (navigate/upload/download/delete)`.

## Self-review checklist
- [ ] N storages per workspace (registry CRUD), workspace-scoped (IDOR-safe), secret write-only/never returned.
- [ ] `ObjectStore::list` folder-style; file browser list/upload/download/delete confined under the connection root (no `..` traversal); large-download guard.
- [ ] All Storage routes gated `workspace:manage_members`; FE 403-graceful.
- [ ] Catalog provisioning UNTOUCHED (old `workspace_storage` still serves it).
- [ ] Backend + clippy green, single iceberg; FE lint/typecheck/test/build green; full local CI before merge.

## Out of scope
Catalog epic (warehouse-on-storage + remove old auto-provision); GCS/Azure; multipart/resumable upload; versioning; per-purpose storage wiring.
