# Storage file browser — folder ops (new folder / delete folder / move-rename) — Design

**Date:** 2026-06-27 · **Status:** Design — approved · **Branch:** `feat/storage-file-manager` (off main)
**Amends:** the storage file browser merged in PR #74.

## Goal
The file browser can list/upload/download/delete individual files + navigate folders, but cannot **create folders**, **delete folders (recursively)**, or **move/rename** files & folders. Add these so it's a real folder-based file manager. All ops stay path-confined under the connection's `key_prefix` root + gated `workspace:manage_members`.

## Backend (`egret-storage` + `egret-catalog-engine/src/browse.rs`)

### `ObjectStore::copy` (egret-storage)
Add `async fn copy(&self, src: &str, dst: &str) -> Result<(), StorageError>` → S3 `copy_object` (server-side; `CopySource = <bucket>/<src_key>`, same bucket). Implement in `S3Store`; the `MemStore` test stubs get a get+put fallback or a simple map copy.

### Routes (all gate `require_workspace_admin`, `WsContext`, every path through `safe_key`)
- **New folder:** `POST /ws/{ws}/storages/{id}/objects/folder?path=<p>` → `safe_key(root, p)`, ensure a single trailing `/`, `store.put(<effective>/, vec![])` (empty marker so the folder shows in `list`) → 201. Reject empty name.
- **Delete (file or folder, recursive):** extend `DELETE /ws/{ws}/storages/{id}/objects?key=<k>&recursive=<bool>`:
  - `recursive=true` (or `key` ends with `/`): `store.list(<effective>, None)` (flat) → delete every returned object key + the folder marker. Best-effort loop; if any delete fails, return 500 with how many succeeded (document partial-failure: re-running is safe/idempotent).
  - else: single `store.delete(<effective>)`.
  → 204.
- **Move / rename:** `POST /ws/{ws}/storages/{id}/objects/move` body `{ from: String, to: String }` → `safe_key(root, from)` + `safe_key(root, to)`:
  - **File** (`from` not ending `/`): `copy(from→to)` then `delete(from)`. (Rename = move where `to` shares the parent with a new last segment.)
  - **Folder** (`from` ends `/`): `list(<from>, None)` → for each object, compute `to + (key stripped of from)` → `copy` + `delete`; also move the folder marker. Best-effort; partial-failure documented.
  - Reject `to` that is inside `from` (moving a folder into itself) → 400; reject `from == to` → 400.
  → 200.

## Frontend (`FileBrowser.tsx` + `storage.ts`)
- Hooks: `useCreateFolder(ws, connId)` (POST folder), `useMoveObject(ws, connId)` (`{from,to}` POST move); extend delete to pass `recursive` (a `useDeleteObject` arg, or a `useDeleteFolder`). All invalidate the objects query.
- **New folder** button (next to Upload) → small dialog/inline input for the name → create under the current `prefix` → refetch (folder appears, navigable).
- **Folders** get a Delete action (recursive) → `ConfirmDeleteDialog` with a "this removes the folder and ALL its contents" warning.
- **Move/Rename** action on files AND folders (a row action / dropdown) → a dialog:
  - **Rename:** prefilled with the current name (last segment); submit → move within the same parent.
  - **Move:** a destination path input (or "move to folder" — a text field for the target prefix, validated). Submit → `useMoveObject({ from: <full relative key>, to: <new relative key> })`.
  - After success → refetch the current folder.
- Keep the existing navigate/upload-into-folder/download/delete-file behaviour.

## Testing
- **BE (DB+MinIO):** new folder → appears in `list` (as a folder) + is navigable (list under it works); upload into it; delete-folder recursive → folder + all contents gone; move a file to another folder → old key gone, new key present with same bytes; rename a file → renamed; folder move → all contents relocated; `move` of `from` into itself → 400; path-safety on `path`/`from`/`to` (reject `..`/abs); non-admin → 403; cross-workspace conn → 404.
- **FE (mock):** New-folder dialog calls `useCreateFolder`; folder Delete calls delete with recursive; Move/Rename dialog calls `useMoveObject` with the right `{from,to}`; refetch after each.

## Risks
- **Path-safety on BOTH `from` and `to`** (move) and on the folder path — a bug = read/write outside the connection root. Reuse the proven `safe_key`. Reject moving a folder into its own subtree.
- **Partial failure** on recursive delete / folder move (no transaction across N S3 objects) — best-effort + idempotent re-run; document it; surface a clear error if some objects remain.
- **Large folders:** recursive ops loop over `list` (paginated) — fine for typical sizes; note a cap isn't enforced (a huge folder is a slow op).

## Out of scope
Multi-select bulk ops; drag-and-drop move; copy (vs move); cross-connection move; object versioning.
