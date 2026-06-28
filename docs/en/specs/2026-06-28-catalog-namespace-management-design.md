# Catalog — namespace management (create / drop) — Design

**Date:** 2026-06-28 · **Status:** Design — approved · **Branch:** `feat/catalog-namespaces` (off main)
**Program:** Sub-epic 2 of the Catalog-console-parity program (warehouse mgmt ✅ → **namespace mgmt** → table/view detail → tasks/statistics → permissions). Reference: the lakekeeper console.

## Goal
Let a workspace admin **create** and **drop** namespaces (including nested) within a warehouse, from the warehouse-detail **Browse** tab. The catalog already browses the namespace tree; this adds the two DDL mutations.

## Background (current state)
- Browse is warehouse-scoped (sub-epic 1): `GET /ws/{ws}/catalog/warehouses/{wid}/namespaces` recursively flattens the namespace tree (BFS over `NamespaceService::list_namespaces`, paginating each level) → a flat list of dotted names; `…/{wid}/namespaces/{ns}/tables` lists a namespace's tables/views.
- lakekeeper exposes `NamespaceService::{create_namespace, drop_namespace, list_namespaces, namespace_exists}` (iceberg v1). Namespaces are `NamespaceIdent` = a vector of levels; egret renders/parses them as dot-joined strings (`parse_ns`/`NamespaceIdent::from_vec`).
- `{wid}` → lakekeeper warehouse uuid resolves via `CatalogStore::lakekeeper_id(workspace_id, wid)` (ownership-checked → 404 cross-ws). Warehouse-management routes are gated `require_workspace_admin`.
- FE `CatalogBrowse.tsx` (rendered in the warehouse-detail Browse tab, props `{ ws, wid }`): a left **Namespaces** panel (flat dotted list, click → tables) + a right table/contents panel. Hooks `useCatalogNamespaces(ws, wid)` / `useNamespaceContents(ws, wid, ns)` in `api/catalog.ts`.

## 1. Backend (`egret-catalog-engine/src/browse.rs`)
Two new routes under the existing warehouse scope, each gated `require_workspace_admin` and resolving `{wid}` → lakekeeper id (ownership-checked):
- **Create:** `POST /ws/{ws}/catalog/warehouses/{wid}/namespaces` body `{ parent: Option<String>, name: String }`.
  - Build the `NamespaceIdent`: `parent` (a dotted existing namespace, or `null`/empty for root) parsed to levels, then push `name` (a single new level — `name` must not contain the dot separator; reject if it does → 400). → `NamespaceService::create_namespace` (empty properties).
  - 201 with the created dotted name. A name collision → **409**. A missing parent (lakekeeper rejects creating under a non-existent parent) → **400/409** with lakekeeper's message.
- **Drop:** `DELETE /ws/{ws}/catalog/warehouses/{wid}/namespaces/{ns}` (`{ns}` = the dotted name, URL-encoded) → `NamespaceService::drop_namespace`.
  - Iceberg drops only an **empty** namespace (no tables, no child namespaces); lakekeeper returns an error when non-empty → map to **409** "namespace is not empty". 204 on success.
- Reuse the existing `ns_query`/`parse_ns`/`NamespaceIdent::from_vec` helpers + the `{wid}` resolution from the browse handlers. `BrowseApiError` maps NotFound→404, BadRequest→400, Conflict→409, Forbidden→403.

## 2. Frontend (`api/catalog.ts` + `CatalogBrowse.tsx`)
- Hooks: `useCreateNamespace(ws, wid)` (POST `/ws/${ws}/catalog/warehouses/${wid}/namespaces` `{parent, name}`), `useDeleteNamespace(ws, wid)` (DELETE `…/namespaces/${encodeURIComponent(ns)}`). Both invalidate the `useCatalogNamespaces(ws, wid)` query.
- **Browse tab** namespace panel:
  - A **＋ New namespace** button at the panel header → a small dialog: **Name** input + **Parent** `Select` (the existing namespaces from `useCatalogNamespaces`, plus a "(root)" default). Submit → `useCreateNamespace`; a 409/400 server message surfaces inline.
  - Each namespace row gets a **Delete** action (icon button) → `ConfirmDeleteDialog` (warning: "Only an empty namespace can be deleted; if it still has tables or child namespaces the server will reject it.") → `useDeleteNamespace`; a 409 "not empty" surfaces in the dialog.
  - Refetch the namespace list after create/drop; keep the existing empty/loading/error states.

## 3. Testing
- **Backend (DB + MinIO + lakekeeper):** create a root namespace → appears in the list; create a nested namespace under it (`parent` set) → appears; create a duplicate → 409; `name` containing a dot → 400; drop an empty namespace → 204 + gone; drop a non-empty namespace (seed a table, or a child namespace) → 409; a `{wid}` from another workspace → 404; a non-admin member → 403. Mirror the `warehouse_api.rs` fixture (seeded storage connection + a created warehouse).
- **Frontend (Vitest, mock `@/api/catalog`):** the New-namespace dialog calls `useCreateNamespace` with `{parent, name}` (root and nested); a row Delete calls `useDeleteNamespace` with the dotted name; a 409 "not empty" is shown; the list refetches.
- **Manual + full local CI** before merge: live smoke — open a warehouse, create a namespace (and a nested one), drop the empty one, confirm a non-empty drop is rejected.

## 4. Risks
- **Drop is empty-only** (no cascade) — matches iceberg semantics; surface the "not empty" error clearly rather than forcing a recursive delete.
- **Nested create needs an existing parent** — lakekeeper validates; surface its error.
- **`{wid}` ownership** must be re-checked on both routes (cross-tenant IDOR → 404).
- The flat-list browse means a created nested namespace appears as a dotted entry (consistent with the current UI); a tree view is a later refinement.

## 5. Out of scope (later)
Namespace **properties** edit (key-value) — defer to the table-detail sub-epic or a follow-up; namespace rename (iceberg has no direct rename); table/view create/drop (done via the iceberg REST API by a write-scoped catalog token, not UI DDL); a hierarchical tree view of namespaces.
