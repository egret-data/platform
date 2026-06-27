# Addendum — lazy warehouse provisioning + effective-default storage + Catalog in Manage

**Date:** 2026-06-27 · **Branch:** `feat/workspace-admin-catalog` · **Amends:** `2026-06-27-workspace-ia-catalog-storage-design.md`

## Problem (found in live testing)
Warehouse provisioning is **create-time only** (the `WarehouseProvisioner` seam fires when a workspace is created). Workspaces created **before catalog was enabled** (e.g. `eng`, and all pre-existing workspaces) have NO warehouse → the Catalog page shows "No catalog provisioned for this workspace yet." and Storage is unusable (GET 404, PUT 409). There is no mechanism to provision a warehouse for an existing workspace.

## Fixes

### 1. Lazy `ensure_warehouse` (egret-catalog-engine)
- `pub async fn ensure_warehouse(engine, store, secret_store, default_cfg, ws_id, slug, system_user) -> Result<Uuid>`: if `store.warehouse_for_workspace(ws)` is `Some` → return it; else `provision_workspace_warehouse(...)` (already idempotent + does the S3 validate) and return the new id. Resolves the effective storage (custom row or `default_cfg`) the same way provisioning does.
- Thread the global `StorageConfig` into `BrowseState` (a new `default_storage: StorageConfig` field, wired from egret-bin where it already exists).

### 2. Catalog self-provisions on access
- The catalog **summary** handler (`browse.rs`) calls `ensure_warehouse` before resolving the warehouse, so visiting Catalog provisions the workspace's warehouse on first view (idempotent thereafter). The "No catalog provisioned" empty state then only appears transiently/if provisioning fails (surface the error).

### 3. Storage = effective config, always editable (move read into the catalog router)
- Add `GET /ws/{ws}/catalog/storage` in `browse.rs` (gated `workspace:manage_members`): `ensure_warehouse` then return the **effective** `WorkspaceStorageInfo` — the custom `workspace_storage` row if present, else the **global default** (`default_storage`: provider "s3", its endpoint/region/bucket, the canonical `workspaces/<slug>/warehouse` prefix, its access_key, `has_secret = !default.secret_key.is_empty()`). Never a secret value.
- `PUT /ws/{ws}/catalog/storage` (exists) gains an `ensure_warehouse` call up front so it works without a prior Catalog visit.
- The FE storage read moves from `GET /ws/{ws}/storage` (workspace router) to `GET /ws/{ws}/catalog/storage`. The old workspace-router storage GET may stay (harmless) or be removed; the FE stops using it.
- Net effect: every workspace (after lazy provisioning) shows an editable Storage form prefilled with its effective config — the 404 "platform default, no edit" dead-end is gone. (Editing still re-points credentials/endpoint/region; bucket/prefix fixed.)

### 4. Catalog moves to the MANAGE sidebar zone (FE)
- Move the `Catalog` nav item from the EXPLORE group to the MANAGE group (with Storage). EXPLORE keeps SQL Lab/Datasets/Charts/Dashboards/Notebooks. Routes unchanged (`/explore/{ws}/catalog`).

## Testing
- BE: a workspace with NO warehouse → `GET /ws/{ws}/catalog/storage` provisions it (warehouse mapping now exists) and returns the effective default config (no secret); a second call is idempotent; the catalog summary on a fresh workspace returns 200 (provisioned) not an empty/error; `PUT` on a never-visited workspace provisions then re-points; non-admin → 403. Keep existing token + storage-update tests green.
- FE: Sidebar renders Catalog under MANAGE (not Explore); `useWorkspaceStorage` hits `/ws/${ws}/catalog/storage`; StoragePage shows the editable form for the effective-default case.

## Risks
- **Side-effect in GET:** lazy provisioning happens inside GET handlers (catalog summary + storage). It is idempotent + one-time (a real S3 validate on first access ~hundreds of ms). Acceptable for lazy-init; surface provisioning errors as 5xx with a clear message rather than a silent empty state.
- **No mass backfill:** only workspaces actually visited get provisioned (avoids provisioning hundreds of junk test workspaces).
- Default access_key shown to workspace-admins (not the secret) — acceptable; they manage this config.

## Out of scope
Backfill/CLI to provision all workspaces; GCS/Azure; full catalog-console parity.
