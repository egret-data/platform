# Control-Plane Polaris Catalog — Design

> Bilingual: English canonical; VI reference at `docs/vi/specs/2026-07-02-control-plane-polaris-catalog-design.md`.
> **Supersedes** `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md` (embedded-lakekeeper direction) — that whole approach (embedded lakekeeper + egret opaque-token layer) is dropped. Plans 1/4 of that redesign are void; the `write_table` helper (engine-write) is retained.

- **Date:** 2026-07-02 · **Status:** Approved (brainstorming) — ready for implementation plan
- **Scope:** Remove egret's current (embedded-lakekeeper) catalog entirely and re-platform the Iceberg catalog on **Apache Polaris**, run as an **external service** that egret **connects to and drives via its management API** (the Engines model). Move **Catalog, Storage, and storage Secrets up to the control plane** (global/platform admin only); workspaces only *consume* assigned catalogs. Build the control-plane UI to manage Polaris (connection, catalogs, storage, full RBAC) + per-workspace assignment. **Notebook development is paused.**

## Decisions (final, from brainstorming)

1. **Remove all current catalog code + data** (greenfield, no migration).
2. **Catalog + Storage + storage-Secrets → control plane**, `platform_admin` only. Workspaces consume via assignment; no `ws_admin` management of these.
3. **egret connects to ONE external Polaris** (single global connection) and drives its **management API** — egret does not deploy or bootstrap the Polaris process (dev runs it via docker-compose; prod is ops-deployed).
4. **v1 management surface (in egret's control-plane UI):** catalogs + storage + workspace-assignment + engine service credential **and full RBAC** (principals / principal-roles / catalog-roles / grants, to table level) — proxied to Polaris.
5. **Polaris auth mode = MIXED**: Polaris issues service tokens (client_credentials, for egret-admin + engines) **and** accepts Keycloak OIDC.
6. **Notebook paused** — remove the notebook's catalog dependency; no further notebook work.

## Background / current state (being removed)

egret embeds **lakekeeper** as a Rust crate mounted at `/catalog/v1`, fronted by `egret-catalog-engine::front_auth` (opaque `catalog_tokens` + workspace-warehouse ownership), with `egret-catalog::CatalogStore` (tokens + `workspace_warehouses`), `egret-iceberg-ext` (legacy REST types), a browse API, and `provision_notebook_access`. Storage connections + Secrets are **workspace-admin** scoped (`/explore/{ws}/storage`, `/explore/{ws}/secrets`; tables `workspace_storages`, workspace secrets). The control-plane **Engines** page (`ControlPlaneEngines.tsx`) is a no-backend frame for a future platform registry (Name/Type/Endpoint/Status/Workspaces/Actions + per-workspace assignment) — Polaris follows this same registry pattern.

Prior comparison (recorded in memory `egret-catalog-single-catalog-pivot` and the superseded spec): lakekeeper does not issue its own tokens and lacks catalog federation; Polaris **issues its own tokens** (`TokenBroker`, `/v1/oauth/tokens`), supports **MIXED** auth, **multi-catalog** (Polaris "Catalog" ≈ lakekeeper "warehouse"), **realms** (tenant isolation), federated catalogs, and RBAC (Principal → PrincipalRole → CatalogRole → Privilege, table-level; no column/row). Polaris is a **Quarkus/JVM service** (not embeddable) — hence the "connect, don't embed" decision.

## Architecture

**Control plane owns the infrastructure; workspaces consume.**

- **Polaris** = the external Iceberg REST catalog + management API + RBAC + token issuer. egret registers **one** Polaris connection.
- **egret backend** = a thin, authenticated **client/proxy** to Polaris's management API (new crate `egret-polaris`). **Polaris is the source of truth** for catalogs / principals / roles / grants; egret does not mirror them. egret owns only two egret-specific concepts in its own DB: the **Polaris connection registration** and the **workspace↔catalog assignment** (Polaris has no notion of egret workspaces).
- **Term mapping:** egret workspace → (consumes) Polaris **Catalog**; Polaris Catalog ≈ old lakekeeper warehouse; each Catalog references a control-plane **storage connection**.
- **Auth (MIXED):** egret-admin authenticates to Polaris with a stored **admin credential** (client_credentials → Polaris token). Engines authenticate to Polaris with a **service credential** (a Polaris principal/token egret provisions per workspace-assignment). Keycloak OIDC is also accepted (future per-user).
- **Governance stays at the engine** (Trino/StarRocks + Ranger) for column/row; Polaris RBAC is catalog/namespace/table-level.

## Components

### A. Removal (greenfield)
- **Crates:** delete `egret-catalog-engine` (embedded lakekeeper + `front_auth` + browse + provisioning), `egret-catalog` (token/warehouse store), `egret-iceberg-ext`. Remove the lakekeeper `[dependencies]` + `[patch]` from Cargo. Remove `AppState.catalog_engine` / `catalog_browse` wiring in `egret-server`/`egret-bin`.
- **DB:** drop schema `lakekeeper`, tables `catalog_tokens`, `workspace_warehouses` (new migration).
- **Frontend:** remove workspace Catalog pages (`/explore/*/catalog`, WarehousesList/WarehouseDetail) and workspace Storage/Secrets pages (`/explore/*/storage`, `/explore/*/secrets`) — these move to the control plane.
- **Kernel helpers:** remove `iceberg()` / `write_iceberg()` / `catalog()` / `_resolve_identifier` + `RuntimeCatalog` (notebook no longer has a direct catalog); keep `trino()` / `starrocks()` / `write_table`. Remove `provision_notebook_access` + the notebook-runtime catalog manifest.

### B. `egret-polaris` client crate (new)
- Typed async client over Polaris **management API** + the OAuth2 token endpoint (client_credentials to obtain the admin token; token cached + refreshed).
- Operations: connection health/info; catalogs (create/list/get/delete, with `StorageConfigInfo`); principals (create/list/delete + credentials); principal-roles; catalog-roles; grants (assign/revoke privileges on catalog/namespace/table). Errors mapped to a typed `PolarisError`.
- Seam for testing: an HTTP-boundary trait so unit tests mock Polaris responses.

### C. Control-plane routes (`platform_admin`-gated)
- `POST/GET/DELETE /api/control-plane/polaris/connection` — register/inspect the single Polaris connection (URL + admin-credential secret ref; live health).
- `…/polaris/catalogs` — CRUD catalogs (each references a control-plane storage connection); proxied to Polaris.
- `…/polaris/principals`, `…/principal-roles`, `…/catalog-roles`, `…/grants` — full RBAC, proxied to Polaris.
- `…/storage` — control-plane storage connections (moved from workspace).
- `…/secrets` — control-plane storage secrets (moved from workspace).
- `POST/DELETE /api/control-plane/workspaces/{ws}/catalog-assignments` — assign/unassign a Polaris catalog to a workspace; on assign, egret provisions a Polaris **service credential** (principal + token) scoped to that catalog for the workspace's engines.
- Read side for workspaces: `GET /api/ws/{ws}/catalogs` — the catalogs assigned to the workspace (consume-only; no management).

### D. Persistence (egret DB)
- Drop the old tables (A). Add:
  - `polaris_connection` — singleton: `base_url`, `admin_secret_id`, health/status, timestamps.
  - `storage_connections` — global (moved from `workspace_storages`): provider/endpoint/bucket/region/secret ref.
  - control-plane `secrets` — global storage secrets (moved from workspace secrets).
  - `workspace_catalog_assignments` — `workspace_id`, `polaris_catalog` (name/id), `service_credential_secret_id`, timestamps.

### E. Frontend (control plane)
- Build real control-plane pages (replacing the stub frames): **Polaris** (connection status + catalogs table [Name/Storage/Type/Status/Workspaces/Actions] + create-catalog form + **RBAC**: principals/roles/grants), **Storage** (global connections), **Secrets** (global). All `RequireGlobalAdmin`.
- Per-workspace: a read-only "Catalogs" view listing assigned catalogs (for members); assignment itself is done from the control plane.
- Remove the workspace Storage/Secrets/Catalog admin routes.

### F. Engine integration
- When a catalog is assigned to a workspace, the workspace's registered engines (Trino/StarRocks) must be able to reach that Polaris catalog. egret provisions a **Polaris service credential** for the assignment and makes it available to the engine connection config (Iceberg REST connector → Polaris catalog URL + the service token; S3/MinIO via the catalog's storage config or engine creds). The exact engine-connector wiring is an operational + engine-registry concern (aligns with the Engines control-plane trajectory) and is **documented**, with egret providing the credential.

## Data flow

1. **Admin** manages via the control-plane UI → egret backend (`platform_admin`) → `egret-polaris` → Polaris management API (catalogs, RBAC), plus egret-DB for storage/secrets/assignments.
2. **Engine** (Trino/StarRocks) → Polaris catalog using the assignment's **service credential** (Polaris token); reads/writes Iceberg on the catalog's storage.
3. **User** → engine (Ranger governs column/row). Users never touch Polaris directly.
4. **Notebook** (paused): reads via `trino()`/`starrocks()`, writes via `write_table` through the engine — no catalog dependency.

## Auth (MIXED)

- **egret-admin → Polaris:** client_credentials (Polaris-issued token) using a stored admin secret; token cached/refreshed in `egret-polaris`.
- **Engine → Polaris:** per-assignment service principal + token (client_credentials).
- **Keycloak OIDC** accepted by Polaris too (future per-user / federation); not required for v1 flows.

## Testing

- `egret-polaris` client: unit tests with a mocked Polaris HTTP boundary (catalog/principal/role/grant CRUD, token acquisition/refresh, error mapping).
- Control-plane routes: authz tests — every endpoint rejects non-`platform_admin`; assignment logic (provision credential, DB row) unit-tested.
- Storage/Secrets moved: repo tests re-scoped to global.
- Removal: workspace builds green after the old crates/tables/UI are gone (no dangling references).
- Dev e2e (opt-in): docker-compose runs a **Polaris** container (replacing the embedded lakekeeper); register connection → create catalog on MinIO storage → assign to a workspace → an engine reads/writes it.

## Out of scope

- **Notebook feature work** (paused) — only its catalog dependency is removed.
- **Engines centralization** to the control plane (parallel trajectory; this spec assumes engines can be given a catalog credential but does not rebuild the engine registry).
- **Column/row governance** (engine + Ranger, unchanged).
- **Polaris deployment/ops** (external service; dev compose only).
- **Data migration** (greenfield).
- **Multiple Polaris connections / realms / federation** (single connection v1; realms/federation are Polaris capabilities we may use later).

## Future direction — K8s provisioning (separate epic, not in this epic)

egret's north star is a **K8s-native control plane** that not only *manages* but also *provisions* the backing services: creating engines (Trino / StarRocks / RisingWave) and the catalog (Polaris) on Kubernetes via operators/Helm, then managing them through this same control plane.

- **Two halves:** this epic builds the **Manage** half (connection registry + drive the management API). K8s provisioning is the **Provision** half layered on top later — it does not replace this work.
- **Keep the connection abstraction clean for a `Provisioner`.** Today a `platform_admin` *manually registers* a connection to an ops-deployed service. A future `Provisioner` (a K8s-operator CRD apply, or a Helm release) will *create* the service and **auto-register the same connection row** — so the register/manage routes here are reused unchanged. Do not couple management logic to "who deployed the service."
- **Per-service provisioning (future, to be spiked in its own epic):** StarRocks and RisingWave have official K8s **operators**; Trino via its official **Helm chart** (or the Stackable operator); **Polaris has no operator yet** — deploy via the in-repo **Helm chart** (`polaris/helm/polaris`) or raw manifests, then manage via `egret-polaris` (P3/P4). So Polaris fits the "Helm-deploy + management-API-manage" shape cleanly.
- **Own brainstorm when reached:** K8s client in egret, per-service provisioner (operator CRD vs Helm), deploy/health tracking, teardown, secrets injection.

## Risks / notes

- **New external JVM dependency** (Polaris) — dev must run it (docker-compose); prod ops must deploy it. Bigger operational surface than embedded lakekeeper.
- **egret holds a Polaris admin credential** — store in the (control-plane) secrets/Secret module; never log; least-privilege where possible.
- **Polaris is the source of truth** — egret must handle drift/errors gracefully (e.g., a catalog deleted in Polaris out-of-band); the UI reflects Polaris state, not an egret mirror.
- **Engine↔catalog credential wiring** is the least-specified piece; pin it during planning (how the Trino/StarRocks connector gets the Polaris catalog URL + service token).
- **Big removal** — verify nothing outside the notebook/catalog path depends on `egret-catalog`/`egret-catalog-engine` before deleting (codegraph_impact).
- **Decomposition:** this spec becomes several plans — (P1) remove old catalog + move Storage/Secrets to control plane, (P2) `egret-polaris` client, (P3) control-plane catalog + RBAC routes + assignment, (P4) control-plane UI, (P5) dev Polaris compose + e2e + engine wiring.
