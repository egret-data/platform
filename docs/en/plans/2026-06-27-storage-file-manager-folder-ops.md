# Storage file manager — folder ops — Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use `- [ ]`.

**Goal:** Add New folder / Delete folder (recursive) / Move-Rename (files & folders) to the storage file browser, all path-confined + admin-gated. **Run full local CI before merge** (GitHub quota out).

**Spec:** `docs/en/specs/2026-06-27-storage-file-manager-folder-ops-design.md`. **Branch:** `feat/storage-file-manager` (off main).

**Verified seams:**
- `egret-storage`: `ObjectStore { put/get/delete/list }`; `S3Store` (aws-sdk-s3 client) → add `copy` via `copy_object`. `Listing { folders, objects }`. Test stubs (`MemStore`) in egret-engine tests + browse tests need the new method.
- `egret-catalog-engine/src/browse.rs`: `BrowseState`; `require_workspace_admin`; `WsContext`; the existing `safe_key(root, path)` / `conn_s3` / `strip_root` helpers (REUSE for path-safety on every new path arg); existing object routes (`list_objects`/`upload_object`/`download_object`/`delete_object`). `BrowseApiError`.
- FE `src/api/storage.ts` (`useStorageObjects`, `useUploadObject`, `useDeleteObject`, `downloadObject`, `storageKeys`); `src/routes/explore/storage/FileBrowser.tsx` (prefix state, breadcrumb, folder click → setPrefix, upload to `prefix+name`, ConfirmDeleteDialog).

---

## Task 1: Backend — copy + folder routes
**Files:** `egret-storage/src/{lib.rs,s3.rs}` (copy) + the test MemStore stubs; `egret-catalog-engine/src/browse.rs` (3 routes); test `egret-catalog-engine/tests/storage_folder_ops_api.rs`.
- [ ] `ObjectStore::copy(&self, src: &str, dst: &str) -> Result<(), StorageError>` + `S3Store` impl (`copy_object`, `copy_source = "<bucket>/<src_key>"`, same bucket). Update all `MemStore` test stubs (get+put fallback). Add an `egret-storage` test (MinIO) for copy.
- [ ] **New folder** `POST /ws/{ws}/storages/{id}/objects/folder?path=<p>`: `conn_s3` → `safe_key(root, p)` → ensure trailing `/` → `put(s3_uri(bucket, key), vec![])` → 201; empty name → 400.
- [ ] **Delete recursive** — extend `delete_object` (`DELETE …/objects?key=<k>&recursive=<bool>`): if `recursive` or `k` ends `/` → `list(<effective>, None)` → delete every object key + the marker (best-effort; on any failure 500 noting count); else single delete → 204.
- [ ] **Move/rename** `POST /ws/{ws}/storages/{id}/objects/move` body `{from, to}`: `safe_key` both; reject `from==to` or `to` inside `from` (folder-into-itself) → 400; file → `copy`+`delete`; folder (`from` ends `/`) → `list(from,None)` → each `copy(to+rel)`+`delete`, + marker → 200.
- [ ] Tests (DB+MinIO): new folder appears in list + navigable; recursive delete removes folder+contents; file move (old gone, new present, same bytes); rename; folder move (all contents relocated); folder-into-itself → 400; path-safety (`..`/abs on path/from/to → 400); non-admin → 403; cross-ws conn → 404. `cargo test -p egret-storage -p egret-catalog-engine` (use `--test-threads=1` if MinIO flakes); `cargo build --workspace`; clippy; fmt; single iceberg. Commit `feat(catalog-engine): storage folder ops — new folder, recursive delete, move/rename`.

## Task 2: Frontend — folder management UI
**Files:** `frontend/src/api/storage.ts` (hooks); `frontend/src/routes/explore/storage/FileBrowser.tsx`; tests.
- [ ] Hooks: `useCreateFolder(ws, connId)` (POST `…/objects/folder?path=`), `useMoveObject(ws, connId)` (POST `…/objects/move` body `{from,to}`), extend delete to a `recursive` flag (`useDeleteObject({key, recursive})` or a `useDeleteFolder`). Invalidate the objects query (current prefix) on success.
- [ ] **New folder** button next to Upload → inline input / small dialog for the name → `useCreateFolder` under the current `prefix` → refetch.
- [ ] **Folder rows** get a Delete action → `ConfirmDeleteDialog` warning "removes the folder and ALL contents" → delete with `recursive: true`.
- [ ] **Move/Rename** action on files AND folders (row button/dropdown) → a dialog: a "name/path" input prefilled with the current relative key (rename = change last segment; move = change the folder part). Submit → `useMoveObject({ from, to })` → refetch. Keep navigate/upload/download/delete-file as-is.
- [ ] Tests (mock `@/api/storage`): New-folder dialog → `useCreateFolder` with the name; folder Delete → delete with recursive; Move/Rename dialog → `useMoveObject` with the right `{from,to}`. `npm run lint && typecheck && test && build`. Commit `feat(web): storage file manager — new folder, delete folder, move/rename`.

## Self-review
- [ ] New folder / recursive delete / move-rename work for files AND folders; path-safety on every path arg (`..`/abs rejected; folder-into-itself rejected); admin-gated; IDOR-safe.
- [ ] Backend + clippy green, single iceberg; FE lint/typecheck/test/build green; full local CI + live Playwright verify before merge.

## Out of scope
Bulk multi-select; drag-and-drop; copy-vs-move; cross-connection move; versioning.
