# Egret — Master Architecture Design

> **Status:** Living · **Date:** 2026-07-08 · **Version:** 0.3
> This is the **canonical (source-of-truth) document** for the Egret project. All later tasks/specs reference it.
> A Vietnamese reference translation lives at [`docs/vi/architecture/2026-06-17-egret-architecture-design.md`](../../vi/architecture/2026-06-17-egret-architecture-design.md).
> Major decisions are recorded in [§2 Architecture Decisions](#2-architecture-decisions-adr-summary).
> **v0.2 evolution:** since v0.1 the catalog was re-built on an embedded lakekeeper, the product reframed into 4 workspace pillars, and per-token catalog authz landed — see [§2 ADRs](#2-architecture-decisions-adr-summary), [§10.3](#103-iceberg-rest-catalog-egret-catalog), and the [§16 roadmap](#16-roadmap--status).
>
> **⚠️ v0.3 corrections (2026-07-08) — read first.** A [whole-project audit](./2026-07-08-project-review-audit.md) found this document had drifted from the code. Until the affected sections are rewritten in full, the audit + the [service-ready architecture redesign (Effort A)](./2026-07-08-service-ready-architecture-design.md) are authoritative where they conflict with the body below. Key corrections:
> - **Crate map is stale.** 22 crates exist. Crates present in code but missing from §5/§15: **`egret-controlplane`** (largest, ~7.3k LOC), **`egret-orchestrator`** (k8s engine provisioning), **`egret-kernel`** (Daft/Jupyter kernel runtime), **`egret-machine-idp`** (machine-IdP for catalog JWTs). Crates named in §5 that do **not** exist: `egret-dataapi`, `egret-workflow`, `egret-notify`, `egret-monitor`, `egret-events` (all still unbuilt; monitoring currently lives inside `egret-controlplane/monitor.rs`). `egret-notebook` is **shipped**, not "(planned)".
> - **ADR-16 is superseded.** The `EgretAuthorizer` + `catalog_tokens` design was removed (migration 0048); catalog authz is now a front-of-router **`CatalogGuard`** (in `egret-bin`) over machine-JWT claims + the **`catalog_grants`** tables (migration 0053), with lakekeeper on `AllowAllAuthorizer`.
> - **Frontend is a flat single-package app** (`frontend/`, React 18 + Vite), **not** the `apps/web + packages/*` monorepo §6/§15 describe.
> - **Migrations** are at **0061** (not ~0029).
> - **Architecture direction:** Effort A restructures the modular monolith into 7 service-extractable seams (contract-crate + owned-schema + event bus) — see the redesign doc. Foundation epic E0 in progress: unified `Error→HTTP` mapping and the platform authz gate now live in `egret-core`/`egret-auth`.

---

## Table of Contents

1. [Goals & scope](#1-goals--scope)
2. [Architecture decisions (ADR summary)](#2-architecture-decisions-adr-summary)
3. [System overview (C4)](#3-system-overview-c4)
4. [Deployment model](#4-deployment-model)
5. [Backend crate map](#5-backend-crate-map)
6. [Frontend architecture](#6-frontend-architecture)
7. [Workspace & multi-tenancy model](#7-workspace--multi-tenancy-model)
8. [AuthN / AuthZ](#8-authn--authz)
9. [Data model (Postgres)](#9-data-model-postgres)
10. [Subsystem specifications](#10-subsystem-specifications)
11. [Key sequence flows](#11-key-sequence-flows)
12. [Cross-cutting concerns](#12-cross-cutting-concerns)
13. [API surface & OpenAPI](#13-api-surface--openapi)
14. [Security](#14-security)
15. [Repository layout](#15-repository-layout)
16. [Roadmap & status](#16-roadmap--status)
17. [Open questions / future](#17-open-questions--future)
18. [Appendix: references](#18-appendix-references)

---

## 1. Goals & scope

Egret is a unified **data portal** that combines the capabilities of Metabase/Superset (BI &
exploration), Lakekeeper (Iceberg REST catalog), Apache Ranger (centralized access policy), DBApi
(data API builder), and n8n (workflow automation) into **a single product written in Rust**, with a
frontend that inherits the sparklabx design system.

### 1.1 Two portals in one tool

| Portal | Audience | Core capabilities |
|---|---|---|
| **Admin / Data Governance** | Admin, data steward | Manage databases, Iceberg catalog, Ranger policies, users/roles/workspaces, org-level Data APIs and workflows, monitoring/audit |
| **Explorer** | User / analyst | SQL Lab, dataset, chart, dashboard, notebook, workspace Data APIs, automation workflows — **isolated per workspace** |

**4-pillar product framing (ADR-17):** each workspace is a full self-contained data platform, organized into four pillars: **Catalog** (lakehouse core: per-workspace Iceberg catalog + external connections + lineage), **Analytics** (BI dataset/chart/dashboard + Notebook + alerts), **Engineering** (workflow + data API + pipeline IDE + CDC→Iceberg), and **Admin** (control plane: members, RBAC, catalog perms, database/engine connections, Ranger, secrets, storage). The two-portal view still holds: the Admin portal maps onto the Admin pillar; the Explorer portal spans Catalog + Analytics + Engineering, all scoped per workspace.

### 1.2 Capabilities

- **Database management** — register, test, health-check, monitor databases. Initial focus: **Trino** & **StarRocks**.
- **Apache Ranger API** — expose a Ranger Admin REST-compatible surface so the native Ranger plugins in Trino/StarRocks download policies and enforce them.
- **Iceberg REST Catalog** — provide an Iceberg-spec REST catalog so engines (Trino, StarRocks, Spark…) can integrate. References Lakekeeper.
- **Explorer / BI** — SQL Lab, dataset, chart, dashboard, notebook (references Metabase/Superset/sparklabx).
- **Data API Builder** — configure HTTP APIs from SQL (MyBatis-style dynamic SQL). References DBApi.
- **Workflow Automation** — n8n-style automation flows (schedule, send email, call API…).
- **Multi-workspace** — isolate projects/departments on the Explorer side.
- **OIDC / SSO** — integrated login.

### 1.3 Out of scope (initially — YAGNI)

- A native query engine (Egret does **not** run queries itself; it delegates to Trino/StarRocks).
- Connectors beyond Trino/StarRocks (the architecture allows extension, but the MVP targets these two).
- Self-hosting notebook kernels (reuse sparklabx's Jupyter/Spark kernel over the protocol).
- Real-time multi-cursor collaboration (later).

---

## 2. Architecture decisions (ADR summary)

| # | Decision | Choice | Rationale |
|---|---|---|---|
| ADR-01 | Backend shape | **Modular monolith** — one Rust workspace, many crates, one binary | Easy to deploy/develop early, still cleanly modular; follows the Lakekeeper model. The `catalog`/`ranger` crates are designed so they can later be split into services |
| ADR-02 | Frontend | **Single SPA**, two route groups (Admin/Explorer) gated by role | Maximize shared components & theme; simpler ops |
| ADR-03 | Ranger | **Ranger Admin REST-compatible** | The native Ranger plugins in Trino/StarRocks fetch directly, no plugin changes needed |
| ADR-04 | Metadata store | **PostgreSQL** (sqlx, compile-time checked) | Source of truth; follows Lakekeeper/Superset/Metabase |
| ADR-05 | Object storage | **S3-compatible (RustFS)**, pluggable abstraction | Iceberg data + result cache; inherits `lakekeeper/crates/io` |
| ADR-06 | Cache/queue | **Redis (optional)**, Postgres fallback | Sessions, async jobs, rate-limiting |
| ADR-07 | HTTP runtime | **axum + tokio + tower** | Ecosystem standard; Lakekeeper uses axum |
| ADR-08 | API contract | **OpenAPI via utoipa** | Auto-generate the FE client |
| ADR-09 | Charting | **Apache ECharts** | Rich; Superset uses ECharts |
| ADR-10 | Workflow canvas | **React Flow** (FE) + Rust engine | Standard for n8n-style DAG editors |
| ADR-11 | Email | **SMTP + Microsoft Graph** (pluggable) | Leverages Microsoft-Graph-SMTP-Relay |
| ADR-12 | Workspace isolation | **Global register → grant**; **logical isolation** (row-level `workspace_id` + Ranger policy) | Centralized governance, shared engine, flexible |
| ADR-13 | Secrets | **Envelope encryption** in Postgres, KMS-pluggable | Protect credentials at rest |
| ADR-14 | Catalog implementation | **Embed lakekeeper core as a Rust library** (`egret-catalog-engine`) instead of reimplementing the Iceberg REST catalog | Same iceberg-rust fork → a single `iceberg` version; production-grade store/REST/GC/storage reused; egret keeps identity/RBAC/UI/token + an `Authorizer` bridge. Supersedes the from-scratch `egret-catalog` of v0.1 (its tables/handlers were deleted in migration 0026) |
| ADR-15 | Catalog tenancy | **1 lakekeeper default project + 1 warehouse per workspace** (`{prefix}` = warehouse UUID), mapped in `workspace_warehouse`, provisioned on workspace-create | Deep per-workspace isolation — each workspace gets its own catalog/warehouse |
| ADR-16 | Catalog authz | **egret `front_auth`** (per-workspace token ownership + read/write + `namespace_scope`) **+ a minimal `EgretAuthorizer`** (lakekeeper `Authorizer` impl gating data actions by per-token scope claim); lakekeeper's own RBAC/OpenFGA is NOT used | egret owns identity; lakekeeper's full authz model is heavier than needed; defense-in-depth in two layers |
| ADR-17 | Product framing | **Each workspace = a full self-contained data platform**, organized into **4 pillars**: Catalog / Analytics / Engineering / Admin | Workspace is the isolation unit for catalog + engine + config; replaces the flat two-portal framing (Admin/Explorer still roughly map onto Admin + the other three pillars) |
| ADR-18 | Notebook engine | **Daft** (Rust-native Python dataframe) kernel per workspace, bound to the workspace's Iceberg catalog; **absorbs SQL Lab** into one notebook surface | Modern dataframe engine; avoids hosting Jupyter/Spark; one ad-hoc surface. Supersedes the Jupyter/Spark-kernel note in §10.6 |
| ADR-19 | Management model | **Three layers: (1) Control plane** — platform-wide admin (`/admin/*`): users + platform roles, workspace lifecycle, Ranger, monitoring/audit, KMS/root secrets, read-only cross-workspace catalog overview. **(2) Workspace admin** — per-workspace configuration (NEW surface): catalog access (tokens + read/write + namespace_scope), workspace storage, **database/engine connections (self-service per workspace)**, members + workspace RBAC, external catalog connections, workspace secrets. **(3) Workspace features** — the per-workspace tools (`/explore/*`): catalog **browse**, Analytics (notebook/dataset/chart/dashboard), Engineering (workflow/data API) | Every management surface gets a clear home; per-workspace self-service config realises "workspace = full platform" (ADR-17); platform admin stays cross-cutting only. **Revises ADR-12:** database/engine connections are now per-workspace (layer 2), not global-register-then-grant. Catalog splits: *browse* is a layer-3 feature; *tokens + storage* are layer-2 workspace-admin |

> Each ADR gets a detailed file under `docs/en/adr/` (and `docs/vi/adr/`) once implementation begins.
> **Note (ADR-19 supersession):** ADR-12's "global register → grant" applies to Ranger policy scope, not database connections — those are now per-workspace self-service (ADR-19 layer 2).
> **Refinement (ADR-19a, 2026-06-27):** the workspace sidebar is split into **zones** — **EXPLORE** (SQL Lab, Datasets, Charts, Dashboards, Notebooks, Catalog) and **MANAGE** (Storage, …later Members/Connections). Catalog management does NOT live under a generic "Settings" page — instead the **Catalog is a rich multi-tab AREA** (mirroring lakekeeper's web console `data-portal/console`: namespaces · details · tasks · statistics · permissions; table-detail: schema/snapshots/files…) and **catalog access (tokens) is a tab INSIDE that Catalog area**. **Storage is its own MANAGE page**, an editable per-workspace RustFS/S3 config (uses lakekeeper `update_storage_profile`/`update_storage_credential` to re-point the warehouse). Full lakekeeper-console catalog parity is a phased epic.

---

## 3. System overview (C4)

### 3.1 Context (Level 1)

```mermaid
graph TB
  subgraph Users
    A[Admin / Data Steward]
    U[Analyst / User]
  end
  subgraph External Engines
    TR[Trino]
    SR[StarRocks]
    SP[Spark / other Iceberg engines]
  end
  IDP[(OIDC / SSO Provider)]
  MAIL[(SMTP / MS Graph)]
  OBJ[(Object Storage S3/RustFS)]
  PG[(PostgreSQL)]

  A -->|Admin Portal| EG[Egret]
  U -->|Explorer Portal| EG
  EG -->|auth| IDP
  EG -->|notify| MAIL
  EG <-->|metadata/data| PG
  EG <-->|iceberg data, result cache| OBJ

  TR -->|Ranger policy download| EG
  SR -->|Ranger policy download| EG
  TR -->|Iceberg REST catalog| EG
  SR -->|Iceberg REST catalog| EG
  SP -->|Iceberg REST catalog| EG
  EG -->|run query| TR
  EG -->|run query| SR
```

**Key insight:** Egret is both a **client** of the engines (running user queries) and a **server**
the engines call into (downloading Ranger policies, using the Iceberg catalog). This bidirectional
relationship is the most important architectural fact.

### 3.2 Container (Level 2)

```mermaid
graph TB
  subgraph Browser
    SPA[Egret SPA<br/>Admin + Explorer]
  end
  subgraph Egret Binary [Egret — single binary modular monolith]
    HTTP[egret-server<br/>axum router + middleware + WS hub]
    subgraph Identity
      AUTH[egret-auth]
      AUTHZ[egret-authz]
      RANGER[egret-ranger]
    end
    subgraph DataPlane
      DB[egret-database]
      ENGINE[egret-engine]
      CAT[egret-catalog]
    end
    subgraph Product
      BI[egret-bi]
      NB[egret-notebook]
      API[egret-dataapi]
      WF[egret-workflow]
      NOTIFY[egret-notify]
      MON[egret-monitor]
    end
    subgraph Foundation
      META[egret-metastore]
      SEC[egret-secrets]
      STORE[egret-storage]
      EVT[egret-events]
    end
  end
  PG[(PostgreSQL)]
  RD[(Redis opt)]
  OBJ[(Object Storage)]

  SPA -->|REST + WS| HTTP
  HTTP --> AUTH & AUTHZ & RANGER & DB & ENGINE & CAT & BI & NB & API & WF & MON
  AUTH & AUTHZ & DB & CAT & BI & NB & API & WF & MON --> META
  DB & API --> SEC
  CAT & ENGINE --> STORE
  WF & MON & CAT & AUTHZ --> EVT
  META --> PG
  HTTP --> RD
  STORE --> OBJ
  RANGER --> AUTHZ
  BI & API & NB --> ENGINE
```

---

## 4. Deployment model

- **Packaging:** one Docker image (the single `egret` binary) + one static frontend image (nginx) — or the binary serves the static SPA directly.
- **Required dependencies:** PostgreSQL, object storage (S3/RustFS).
- **Optional dependencies:** Redis (cache/queue), Kafka/NATS (distributed event bus — defaults to in-proc).
- **CLI binary:** `egret serve` (run the server), `egret migrate` (DB migrations), `egret init` (bootstrap admin/workspace).
- **Helm chart** for K8s; HorizontalPodAutoscaler at scale (the monolith is stateless apart from DB/Redis/object store).
- **Future service split (ADR-01):** `egret-catalog` and `egret-ranger` are the first split candidates because engines call them directly with their own access patterns; the binary supports a "catalog/ranger only" mode via a feature flag / sub-command.

```mermaid
graph LR
  subgraph K8s
    EGP[egret pods x N]
    FE[frontend nginx]
  end
  PG[(Postgres HA)]
  RD[(Redis)]
  RUSTFS[(RustFS/S3)]
  EGP --- PG
  EGP --- RD
  EGP --- RUSTFS
  FE --- EGP
```

---

## 5. Backend crate map

Cargo workspace under `crates/`. Convention: each crate has **one clear purpose**, communicates via
**traits/interfaces in `egret-core`**, and is independently testable. The `*-server` crate composes
the router.

> Product crates `egret-notebook` **(planned)**, `egret-dataapi` **(planned)**, `egret-workflow` **(planned)**, `egret-notify` **(planned)**, `egret-monitor` **(planned)**, `egret-events` **(planned)** are not yet built; the rows below describe the target design.

### 5.1 Foundation

| Crate | Responsibility | Key deps | Public interface |
|---|---|---|---|
| `egret-bin` | Entrypoint, CLI (`serve`/`migrate`/`init`), wiring, graceful shutdown | all | `main()` |
| `egret-server` | axum app: root router, middleware (auth, workspace-context, tracing, CORS, rate-limit), OpenAPI mount, **WebSocket hub** | core, product crates | `build_router(AppState) -> Router` |
| `egret-core` | Domain primitives: ID types, `Error`/`Result`, pagination, `WorkspaceContext`, `Principal`, abstraction traits | — | types + traits |
| `egret-config` | Typed config (serde + figment), env + file, validation | core | `Config::load()` |
| `egret-metastore` | Postgres: sqlx pool, migration runner, base repository, transaction helper | core, sqlx | `Db`, `Repo` traits |
| `egret-secrets` | Envelope-encrypted credentials at rest, KMS-pluggable provider | core, metastore | `SecretStore` trait |
| `egret-storage` | Object store IO abstraction (S3/RustFS/GCS/Azure) — inherits `lakekeeper/crates/io` | core | `ObjectStore` trait |
| `egret-events` | Internal event bus (in-proc broadcast + optional Kafka/NATS); topics `policy.changed`, `catalog.changed`, `query.finished`… | core | `EventBus`, `Event` |

### 5.2 Identity & Access

| Crate | Responsibility | Public interface |
|---|---|---|
| `egret-auth` | OIDC/SSO (`openidconnect`), session/JWT, user/group JIT provisioning, **workspace membership**, portal-level RBAC | `Authenticator`, `SessionService`, middleware |
| `egret-authz` | PDP/PAP: policy model (resource/access/condition), tag-based access, **row-filter & column-masking**, decision API | `PolicyDecisionPoint::check()`, `PolicyAdmin` |
| `egret-ranger` | **Ranger Admin REST-compatible** surface: `service-defs`, `services`, `policies` CRUD, `policies/download/{service}`, `tags/download/{service}`, version polling. Maps Egret↔Ranger model | `ranger_router()` (mounted under `/service`) |

### 5.3 Data plane

| Crate | Responsibility | Public interface |
|---|---|---|
| `egret-database` | Connection lifecycle (Trino/StarRocks): register/test/health/monitor, pool registry, creds via `egret-secrets`, capability metadata | `DatabaseRegistry`, `Connection` |
| `egret-engine` | Query execution: **Trino (REST protocol)** + **StarRocks (MySQL protocol)** connectors, async **JobManager**, result streaming, **result cache/store** (Postgres + object store), cancellation, cost/limit guards | `QueryEngine::submit()`, `Job`, `ResultStore` |
| `egret-catalog` | The **egret-side identity layer** of the catalog — per-workspace `catalog_tokens` (hashed; `permissions[]`, `namespace_scope[]`), the `workspace_warehouse` mapping, and the Catalog browse API/UI. The v0.1 from-scratch Iceberg store/handlers/`iceberg_router` were deleted at the CE-2 cutover (migration 0026). | `CatalogTokenStore`, browse router |
| `egret-catalog-engine` | Embeds **lakekeeper** core as a library (sqlx 0.9, isolated Postgres pool with `search_path = lakekeeper`); builds the Iceberg REST router + state, the `front_auth` security middleware, the `EgretAuthorizer`, warehouse provisioning, and typed in-process browse reads. | `mount_catalog_engine`, `build_state`, `EgretAuthorizer`, `front_auth` |
| `egret-workspace` | Workspace provisioning + `WorkspaceProvisioner`/`WarehouseProvisioner` trait seam (avoids circular crate dependency between workspace and catalog-engine) | `WorkspaceProvisioner`, `WarehouseProvisioner` |
| `egret-iceberg-ext` | Iceberg format extensions (egret-specific Iceberg metadata helpers) | format extension types |

### 5.4 Application / Product

| Crate | Responsibility | Public interface |
|---|---|---|
| `egret-bi` | Semantic layer: **dataset** (logical table, column, metric, computed), **SQL Lab** saved query, **chart** (viz config → ECharts spec), **dashboard** (multi-tab grid layout, parameters/filters, click behaviour: cross-filter/URL/drill-through), querying via `egret-engine`, caching | `DatasetService`, `ChartService`, `DashboardService`, `SqlLabService` |
| `egret-notebook` **(planned)** | Notebook model (cells), **Daft** kernel per workspace (ADR-18), bound to the workspace Iceberg catalog, execution, absorbs SQL Lab | `NotebookService`, `KernelGateway` |
| `egret-dataapi` **(planned)** | **Data API Builder**: API definitions, **dynamic-SQL template engine** (MyBatis-style: if/foreach/where), param binding, multi-statement + transaction, publish/version/**hot-reload**, **gateway runtime** (token/api-key, IP allow/deny, rate-limit, cache, call-log) | `ApiDefService`, `ApiGateway`, `SqlTemplate` |
| `egret-workflow` **(planned)** | **Automation engine**: DAG model, **triggers** (cron via `tokio-cron-scheduler`, webhook, on-event), **executors** (run-query, send-email, http-request, transform, branch, call-dataapi, export), run engine + history, retry/alert | `WorkflowService`, `WorkflowRunner`, `NodeExecutor` trait |
| `egret-notify` **(planned)** | **SMTP + Microsoft Graph** providers (pluggable), template rendering | `Notifier` trait |
| `egret-monitor` **(planned)** | Query monitoring, **audit log**, usage metrics, Prometheus exporter, admin health rollups | `AuditLog`, `Metrics` |

### 5.5 Crate dependency diagram (simplified)

> Note: `egret-notebook`, `egret-dataapi`, `egret-workflow`, `egret-notify`, `egret-monitor`, `egret-events` are **(planned)** and not yet built.

```mermaid
graph TD
  bin --> server
  bin --> catalog-engine
  server --> auth & authz & ranger & database & engine & catalog & bi & workspace
  server -.->|planned| notebook & dataapi & workflow & monitor
  ranger --> authz
  bi --> engine
  dataapi -.->|planned| engine
  notebook -.->|planned| engine
  workflow -.->|planned| engine & notify & dataapi
  engine --> database & storage
  catalog --> metastore
  catalog-engine --> metastore & storage
  workspace --> catalog-engine
  database --> secrets
  authz --> metastore
  auth --> metastore
  monitor -.->|planned| metastore
  subgraph foundation
    core
    config
    metastore
    secrets
    storage
    events["events (planned)"]
  end
```

---

## 6. Frontend architecture

A **pnpm workspaces** monorepo, inheriting the sparklabx design system (React + Vite + Tailwind v4 +
Radix UI).

```text
frontend/
├── apps/
│   └── web/                  # single Vite SPA, two route groups
│       ├── src/routes/admin/     # Admin / Governance portal
│       └── src/routes/explore/   # Explorer portal
├── packages/
│   ├── ui/                   # design system extracted from sparklabx (theme tokens, Radix wrappers)
│   ├── api-client/           # generated from OpenAPI + TanStack Query hooks
│   ├── auth/                 # OIDC client, session, workspace switcher
│   ├── charts/               # ECharts wrapper (viz spec → render)
│   ├── sql-editor/           # Monaco + schema autocomplete
│   ├── workflow-canvas/      # React Flow (n8n-style DAG editor)
│   └── notebook/             # cells + Monaco + hyparquet parquet preview
```

### 6.1 App shell & navigation

- **Shared shell:** top bar, **workspace switcher**, theme toggle, user menu. Route guard by role/permission.
- **`/admin/*`** (requires admin/steward role): Databases · Catalog browser · Ranger Policies · Users/Roles/Workspaces · Data API mgmt · Workflows (org-level) · Monitoring/Audit.
- **`/explore/*`** (scoped to the selected workspace): SQL Lab · Datasets · Charts · Dashboards · Notebooks · Data APIs (workspace) · Workflows.

### 6.2 Key libraries

| Purpose | Library | Note |
|---|---|---|
| Data fetching | **TanStack Query** | already in sparklabx |
| Code/SQL editor | **Monaco** | already present |
| Charts | **Apache ECharts** | newly added |
| Workflow canvas | **React Flow** | newly added |
| Parquet preview | **hyparquet** | already present |
| Notebook kernel | **@jupyterlab/services** | already present |
| Auth | **openidconnect (BE)** + MSAL/Google (FE present) | standard OIDC |

### 6.3 Realtime communication

- **WebSocket** (via the `egret-server` WS hub) for: async query status (SQL Lab), live workflow run logs, kernel output streaming. Polling fallback.

---

## 7. Workspace & multi-tenancy model

### 7.1 Two tiers (ADR-12)

```mermaid
graph TB
  subgraph Platform [Platform / Global — Admin]
    USERS[Users & Groups]
    WS[Workspaces]
    DBREG[Databases registered]
    CATREG[Catalog]
    POL[Ranger Policies]
    AUDIT[Audit / Monitoring]
  end
  subgraph WSX [Workspace X — isolated]
    DS[Datasets]
    CH[Charts]
    DASH[Dashboards]
    SQL[SQL Lab queries]
    NBX[Notebooks]
    APIX[Data APIs]
    WFX[Workflows]
  end
  DBREG -->|grant| WSX
  CATREG -->|grant namespace| WSX
  WS --> WSX
```

- **Global:** users, workspaces, databases, catalog, Ranger policies, audit are managed by the Admin.
- **Workspace (isolated):** datasets, charts, dashboards, SQL Lab, notebooks, data APIs, workflows — department X **cannot see** department Y's.

### 7.2 Resource allocation

- Databases & catalogs are **registered globally**, then **granted** to a workspace (tables `workspace_database_grant`, `workspace_catalog_grant`).
- Inside a workspace, users can only select databases/namespaces that have been granted.

### 7.3 Isolation = **logical** (metadata + authz)

- Every workspace-scoped entity carries a `workspace_id`; every repository query **must** filter by the `WorkspaceContext` (middleware injects the context; repositories reject unscoped queries).
- **Ranger policies** restrict which schemas/tables/columns a workspace can access at the engine tier — this is the real enforcement layer when queries run on Trino/StarRocks.
- No physical namespace/DB split per workspace (ruled out in ADR-12) → the engine is shared, ops stay simple.

### 7.4 Roles (RBAC)

| Scope | Example role | Permissions |
|---|---|---|
| Platform | `platform_admin` | full control |
| Platform | `data_steward` | manage catalog, policy, databases |
| Workspace | `ws_admin` | manage workspace members & resources |
| Workspace | `ws_editor` | create/edit datasets, charts, dashboards, SQL Lab, data APIs, workflows |
| Workspace | `ws_viewer` | view dashboards, run queries within limits |

> RBAC (egret-auth) decides **access to Egret features/objects**; Ranger/authz (egret-authz) decides
> **access to data** at the engine tier. The two layers are separate and complementary (§8).

---

## 8. AuthN / AuthZ

### 8.1 Authentication (egret-auth)

- **OIDC/SSO** via `openidconnect`: authorization-code + PKCE. JIT provisioning of users/groups from claims.
- Session: httpOnly cookie + (optional) JWT for APIs. Refresh token rotation.
- Service-to-service: API key / token (used by the Data API gateway and engine callbacks).

### 8.2 Two authorization layers

```mermaid
graph LR
  REQ[Request] --> RBAC{egret-auth RBAC<br/>feature/object access?}
  RBAC -->|deny| D1[403]
  RBAC -->|allow| DATA{egret-authz / Ranger<br/>data access?}
  DATA -->|enforce at engine| Q[Query runs + row-filter/col-mask]
```

1. **Application layer (RBAC):** who can view/edit a dashboard/dataset/data API — checked in Egret.
2. **Data layer (Ranger/authz):** which schemas/tables/columns may be read, row-filters, column-masks — **enforced at the engine** via the policies it downloads from Egret.

### 8.3 Ranger REST compatibility (egret-ranger)

The native Ranger plugins inside Trino/StarRocks periodically call Egret to download policies. Egret
exposes (mounted under `/service`, following Ranger's `/service/plugins` base):

| Endpoint (Ranger-compatible) | Purpose |
|---|---|
| `GET /service/plugins/definitions` | service-defs (trino, starrocks) |
| `GET/POST/PUT/DELETE /service/plugins/services` | manage service instances |
| `GET/POST/PUT/DELETE /service/plugins/policies` | policy CRUD |
| `GET /service/plugins/policies/download/{serviceName}` | **plugin downloads policies** (supports `lastKnownVersion`, returns 304 if unchanged) |
| `GET /service/tags/download/{serviceName}` | **tag-based policies** |

- **Model mapping:** Egret keeps an internal policy model (resource/access/condition/row-filter/masking) in `egret-authz`; `egret-ranger` serializes it to the Ranger JSON schema (`RangerPolicy`, `ServicePolicies`) when a plugin downloads. Version-based caching enables 304 responses.
- **Benefit:** no need to build/maintain a separate Ranger Admin; Trino/StarRocks use the standard plugin.

### 8.4 Policy authoring (Admin portal)

- Policy creation UI: pick a database (service) → resource (catalog/schema/table/column) → user/group/role → permission (select/...) → conditions (row-filter expression, column-mask type).
- Tag-based policies: attach a tag (e.g. `pii`) → policy by tag.
- Every policy change emits a `policy.changed` event (bumps the version) → plugins download the new version on their next poll.

---

## 9. Data model (Postgres)

Schema is split by module (Postgres schema namespaces or table prefixes). Listed below are the **key
tables** (not full columns — details belong in each subsystem spec). All workspace-scoped product
tables carry `workspace_id` and an index on `(workspace_id, …)`.

### 9.1 Identity & workspace

```text
users(id, external_subject, email, name, status, created_at)
groups(id, name, source)             user_groups(user_id, group_id)
workspaces(id, slug, name, status, settings_jsonb, created_at)
workspace_members(workspace_id, user_id|group_id, role)         -- ws_admin/editor/viewer
roles(id, scope[platform|workspace], name)   role_permissions(role_id, permission)
sessions(id, user_id, expires_at, ...)        api_keys(id, owner, hash, scopes)
```

### 9.2 Database & secrets

```text
databases(id, name, engine[trino|starrocks], host, port, options_jsonb,
          secret_ref, status, created_by)                 -- globally registered
workspace_database_grant(workspace_id, database_id, granted_by)
secrets(id, kind, ciphertext, key_id, created_at)          -- envelope-encrypted
database_health(database_id, checked_at, healthy, latency_ms, detail_jsonb)
```

### 9.3 Authz / Ranger

```text
authz_services(id, name, engine, config_jsonb, policy_version)   -- ~ Ranger service
authz_service_defs(id, name, resources_jsonb, access_types_jsonb)
authz_policies(id, service_id, name, resource_jsonb, items_jsonb,
               row_filters_jsonb, masks_jsonb, enabled, version, updated_at)
authz_tags(id, name)        authz_tag_assignments(tag_id, resource_ref)
policy_audit(id, policy_id, action, actor, at)
```

### 9.4 Catalog (Iceberg)

```text
-- The from-scratch catalog tables of v0.1 (catalog_namespaces / catalog_tables /
-- catalog_views / catalog_commits) were REMOVED in migration 0026. egret now embeds
-- lakekeeper; Iceberg metadata lives in lakekeeper's own (~63) tables under the
-- Postgres schema `lakekeeper` (search_path-isolated). egret keeps only the
-- identity + tenancy tables:

catalog_tokens(id, workspace_id, token_hash, label,
               permissions text[],      -- {catalog:read, catalog:write}
               namespace_scope text[],  -- {} = all namespaces; else subtree-scoped (e.g. {marts})
               created_at, revoked_at)  -- per-workspace catalog access tokens (engine auth)
workspace_warehouse(workspace_id PK→workspaces, warehouse_id uuid, created_at)  -- {prefix} = warehouse UUID
role_permissions(role_id, 'catalog:read' | 'catalog:write')                      -- workspace RBAC for catalog
-- schema `lakekeeper`: projects, warehouses, namespaces, tabulars, … (owned by the embedded engine)
```

### 9.5 Engine / query

```text
queries(id, workspace_id, database_id, user_id, sql, status, submitted_at,
        finished_at, rows, bytes_scanned, error, result_ref)    -- result_ref → object store
query_results_cache(hash, database_id, result_ref, expires_at)
```

### 9.6 BI

```text
datasets(id, workspace_id, database_id, kind[table|sql], schema_table|sql,
         columns_jsonb, metrics_jsonb, created_by)
saved_queries(id, workspace_id, database_id, name, sql, created_by)   -- SQL Lab
charts(id, workspace_id, dataset_id, viz_type, config_jsonb)
dashboards(id, workspace_id, name, layout_jsonb, filters_jsonb)
dashboard_charts(dashboard_id, chart_id, position_jsonb)
```

### 9.7 Notebook

```text
notebooks(id, workspace_id, name, kernel_spec, created_by)
notebook_cells(id, notebook_id, idx, cell_type, source, outputs_jsonb)
kernel_sessions(id, notebook_id, status, started_at)
```

### 9.8 Data API

```text
api_defs(id, workspace_id, path, method, database_id, sql_template,
         params_jsonb, version, status[draft|published], cache_ttl)
api_versions(api_def_id, version, snapshot_jsonb, published_at)
api_clients(id, workspace_id, name, key_hash, ip_allow_jsonb, ip_deny_jsonb)
api_client_grants(client_id, api_def_id, rate_limit)
api_call_logs(id, api_def_id, client_id, at, status, latency_ms, rows)
```

### 9.9 Workflow

```text
workflows(id, workspace_id, name, graph_jsonb, enabled, created_by)
workflow_triggers(id, workflow_id, kind[cron|webhook|event], config_jsonb)
workflow_runs(id, workflow_id, status, started_at, finished_at, trigger_jsonb)
workflow_node_runs(run_id, node_id, status, input_jsonb, output_jsonb, error, attempt)
```

### 9.10 Monitor / audit

```text
audit_log(id, actor, action, target_type, target_id, workspace_id, at, detail_jsonb)
usage_metrics(...)    -- rollups
```

---

## 10. Subsystem specifications

Each subsystem states: responsibility, interface, data, integration points, references. Detailed
implementation belongs to a per-phase spec.

### 10.1 Database management (`egret-database`)
- **Function:** register databases (engine, host, creds→secrets), test connection, periodic health check (`database_health`), monitoring (latency, status), capability detection.
- **MVP engines:** Trino, StarRocks. A `Connection` trait allows other JDBC engines later.
- **Integration:** provides connections to `egret-engine`; granted to workspaces.

### 10.2 Query engine (`egret-engine`)
- **Connectors:** Trino via the **REST statement protocol** (`POST /v1/statement` + nextUri polling); StarRocks via the **MySQL protocol** (`sqlx`/mysql_async).
- **JobManager:** async submit, status tracking, result streaming over WS, cancellation, limit enforcement (row/byte/timeout).
- **ResultStore:** cache by hash(sql+db); large results written to object store, metadata in `queries`.
- **Clients:** SQL Lab, dataset preview, chart queries, Data API, the workflow run-query node.

### 10.3 Iceberg REST Catalog (`egret-catalog` + `egret-catalog-engine`)
- **Approach (ADR-14):** embed **lakekeeper** core as a library (`egret-catalog-engine`, sqlx 0.9) on the same iceberg-rust fork as egret → a single `iceberg` version. lakekeeper provides the spec-compliant Iceberg REST router, the Postgres catalog store, storage IO, and (future) GC/task workers. lakekeeper migrations run in an isolated `lakekeeper` Postgres schema (`search_path = lakekeeper,public`).
- **Tenancy (ADR-15):** one default lakekeeper project; **one warehouse per workspace** (`{prefix}` = warehouse UUID), mapped in `workspace_warehouse`, provisioned when a workspace is created via a `WarehouseProvisioner` trait seam (no circular crate dep).
- **Identity layer (`egret-catalog`):** per-workspace `catalog_tokens` (hashed; `permissions[]`, `namespace_scope[]`), the warehouse mapping, and the Catalog browse API/UI.
- **Security (`front_auth` + ADR-16):** the lakekeeper router is mounted at `/catalog/v1` behind an egret middleware enforcing (1) per-workspace token ownership (`{prefix}` must equal the workspace's warehouse → 403), (2) per-token read/write permission, (3) per-token `namespace_scope`. It injects a lakekeeper principal carrying the token's scope as an `egret` claim; the embedded **`EgretAuthorizer`** authorizes data actions (namespace/table/view) against that claim as defense-in-depth, while a system principal (bootstrap + in-process browse) bypasses.
- **egret-server does NOT link lakekeeper** — `egret-bin` builds the engine + browse routers and passes them in as prebuilt `axum::Router`s, keeping the lakekeeper dependency graph out of `egret-server`.
- **Status:** CE-1 (spike) → CE-2 (embedded live per-workspace + cutover; v0.1 catalog deleted) → CE-3 (per-token read/write scope) → CE-4 (`EgretAuthorizer` + `namespace_scope`) → CE-5 (per-workspace storage config) → CE-6 (built-in GC task-queue workers) — all merged. Next: CE-5 follow-ons (GCS/Azure, full secrets bridge); per-live-table Iceberg maintenance is a later, separate subsystem.

### 10.4 Apache Ranger API (`egret-ranger`) — see §8.3.

### 10.5 BI: SQL Lab / Dataset / Chart / Dashboard (`egret-bi`)
- **SQL Lab:** editor (Monaco) → async submit via the engine → results + save to `saved_queries` → "explore" into a chart.
- **Dataset:** a logical layer over a table or SQL; defines column types, metrics (aggregates), computed columns. The source of charts.
- **Chart:** `viz_type` + `config_jsonb` → build query → render an **ECharts** spec on the FE.
- **Dashboard:** grid layout (`layout_jsonb`), cross-filtering, dashboard-level filters; child charts via `dashboard_charts`.
- **References:** Superset (chart/dashboard/dataset model), Metabase.

### 10.6 Notebook (`egret-notebook`) **(planned)**
- **(ADR-18)** Per-workspace notebook on a **Daft** (Rust-native Python dataframe) kernel, bound to the workspace's Iceberg catalog. SQL cells **absorb SQL Lab** into one ad-hoc surface. Output streamed over WS, parquet preview (hyparquet). *(was: Jupyter/Spark kernel in v0.1)*

### 10.7 Data API Builder (`egret-dataapi`)
- **Definition:** path/method + `sql_template` (MyBatis-style dynamic SQL: `<if>`, `<foreach>`, `<where>`) + params schema + target database.
- **Lifecycle:** draft → test/debug (trial run) → **publish** (version, hot-reload without downtime) → snapshot in `api_versions`.
- **Gateway runtime:** route match → auth (api-key/token) → IP allow/deny → rate-limit → render SQL → run via the engine (multi-statement + transaction) → transform → cache (Redis, `cache_ttl`) → response → **call-log** + metrics.
- **References:** DBApi (dbapi-controller, dbapi-cluster-gateway).

### 10.8 Workflow Automation (`egret-workflow`)
- **Model:** DAG `graph_jsonb` (nodes + edges).
- **Triggers:** cron (`tokio-cron-scheduler`), webhook (inbound HTTP), on-event (via `egret-events`: query.finished, policy.changed, catalog.changed).
- **Executors (trait `NodeExecutor`):** run-query, **send-email** (egret-notify), http-request, transform (jq/script), branch/condition, call-dataapi, export-file (object store).
- **Run engine:** topological execution, node→node data passing, retry/backoff, alert on failure, persisted `workflow_runs`/`workflow_node_runs`, live log over WS.
- **Example:** daily cron → run-query (report) → transform → send-email.
- **Reference:** n8n.

### 10.9 Notification (`egret-notify`)
- `Notifier` trait: **SMTP** + **Microsoft Graph** providers (pluggable). Template rendering. Used by workflows & alerting (monitor, data API fail-alert).

### 10.10 Monitoring / Audit (`egret-monitor`)
- Audit log of all sensitive actions (policy change, database registration, grant, data API publish…), query monitoring, usage metrics, **Prometheus** exporter `/metrics`, health rollups for the Admin dashboard.

---

## 11. Key sequence flows

### 11.1 OIDC login

```mermaid
sequenceDiagram
  participant B as Browser
  participant E as egret-server/auth
  participant I as IdP
  B->>E: GET /auth/login
  E->>B: redirect to IdP (PKCE)
  B->>I: sign in
  I->>B: redirect with code
  B->>E: GET /auth/callback?code
  E->>I: exchange code → tokens
  E->>E: JIT provision user/group, create session
  E->>B: set cookie + redirect to portal
```

### 11.2 Run a query (SQL Lab)

```mermaid
sequenceDiagram
  participant B as Browser
  participant S as egret-server
  participant EN as egret-engine
  participant TR as Trino/StarRocks
  B->>S: POST /api/sqllab/execute {db, sql}  (RBAC check)
  S->>EN: submit(job)  -> job_id
  S-->>B: 202 {job_id} + WS subscribe
  EN->>TR: run (engine enforces Ranger policy)
  TR-->>EN: rows (stream)
  EN->>S: progress/result (cache→object store)
  S-->>B: WS push status + result ref
```

### 11.3 Engine downloads Ranger policy

```mermaid
sequenceDiagram
  participant P as Trino Ranger plugin
  participant R as egret-ranger
  participant Z as egret-authz
  P->>R: GET /service/plugins/policies/download/trino?lastKnownVersion=N
  R->>Z: load policies + current version
  alt version == N
    R-->>P: 304 Not Modified
  else changed
    R-->>P: 200 ServicePolicies(Ranger JSON schema, version=N+1)
  end
  P->>P: cache & enforce on next query
```

### 11.4 Iceberg commit (engine writes a table)

```mermaid
sequenceDiagram
  participant SP as Spark/Trino
  participant C as egret-catalog
  participant O as Object store
  participant PG as Postgres
  SP->>O: write data + metadata files
  SP->>C: POST /catalog/v1/.../tables/{t} (commit, metadata_location)
  C->>PG: optimistic validate (current_snapshot), update pointer
  C-->>SP: 200 (new metadata)
```

### 11.5 Publish & call a Data API

```mermaid
sequenceDiagram
  participant A as Admin/Editor
  participant D as egret-dataapi
  participant Cl as API Client
  participant EN as egret-engine
  A->>D: PUT /api/dataapi/{id}/publish (version++)
  D->>D: snapshot + hot-reload route
  Cl->>D: GET /gw/{workspace}/{path}?params (api-key)
  D->>D: auth + IP + rate-limit + render SQL
  D->>EN: execute(render)
  EN-->>D: rows
  D-->>Cl: JSON (+cache, call-log)
```

### 11.6 Scheduled workflow run

```mermaid
sequenceDiagram
  participant SCH as Scheduler(cron)
  participant W as egret-workflow
  participant EN as egret-engine
  participant N as egret-notify
  SCH->>W: trigger workflow (run)
  W->>EN: run-query node
  EN-->>W: rows
  W->>W: transform node
  W->>N: send-email node
  N-->>W: ok
  W->>W: persist run + node runs (WS live log)
```

---

## 12. Cross-cutting concerns

- **Config (`egret-config`):** layered (defaults → file → env). Validated at boot, fail-fast.
- **Secrets (`egret-secrets`):** envelope encryption; data keys wrapped by a KMS/master key; decrypted only when a connection is needed.
- **Events (`egret-events`):** in-proc broadcast by default; Kafka/NATS adapter (inheriting `lakekeeper-events-*`) for multi-instance. Drives workflow triggers, cache invalidation, audit.
- **Error handling:** `egret-core::Error` via thiserror; mapped to HTTP problem+json; fail-fast, log with context (tracing spans).
- **Observability:** `tracing` + OpenTelemetry export; Prometheus `/metrics`; structured JSON logs; request-id + workspace-id in spans.
- **Caching:** query results (engine), data API responses, Ranger policy version (304), dataset metadata. Redis or in-proc.
- **Migration:** sqlx migrations; `egret migrate` runs at deploy; backward-compatible.
- **Testing:** per-crate unit tests (mock traits), integration with testcontainers (Postgres/RustFS/Trino), contract tests for Ranger & Iceberg REST (against the spec), Playwright e2e for the SPA.
- **Multi-instance:** the monolith is stateless (state lives in PG/Redis/object store); the workflow scheduler needs leader election (Postgres advisory lock) to avoid duplicate runs.

---

## 13. API surface & OpenAPI

| Prefix | Module | Client |
|---|---|---|
| `/api/auth/*` | auth | SPA |
| `/api/admin/*` | databases, workspaces, users, monitor | Admin SPA |
| `/api/authz/*` | policy authoring | Admin SPA |
| `/service/plugins/*`, `/service/tags/*` | **Ranger compat** | Trino/StarRocks plugin |
| `/catalog/v1/*` | **Iceberg REST** | engines |
| `/api/ws/{workspace}/sqllab/*` | SQL Lab | Explorer SPA |
| `/api/ws/{workspace}/datasets|charts|dashboards/*` | BI | Explorer SPA |
| `/api/ws/{workspace}/notebooks/*` | notebook | Explorer SPA |
| `/api/ws/{workspace}/dataapi/*` | data API mgmt | Explorer/Admin SPA |
| `/gw/{workspace}/*` | **Data API gateway** runtime | API clients |
| `/api/ws/{workspace}/workflows/*` | workflow | Explorer SPA |
| `/ws` | WebSocket hub | SPA |
| `/metrics`, `/healthz` | monitor | ops |

- **OpenAPI** is auto-generated (utoipa) for the `/api/*` groups; the FE `api-client` is generated from it. The `/catalog` & `/service` groups follow external specs (Iceberg/Ranger) and are tested by contract.

---

## 14. Security

- **Least privilege:** RBAC + two-layer Ranger; service tokens have narrow scopes.
- **Credentials at rest:** encrypted (egret-secrets); secrets never logged.
- **Input validation:** validate all input; the **SQL template (dataapi)** allows **parameter binding only**, blocking SQL injection (no raw interpolation; dynamic parts only affect structure, values are always bound).
- **Network:** Ranger/Catalog endpoints are engine-only (mTLS/allowlist depending on deployment).
- **Audit:** every governance action is written to `audit_log`.
- **Rate-limit & IP allow/deny:** at the Data API gateway and auth.
- **Tenant isolation:** middleware enforces `workspace_id`; tested for cross-workspace leakage.
- **Compliance with the global CLAUDE.md:** no committed secrets; conventional commits; pinned dependencies.

---

## 15. Repository layout

```text
egret/
├── Cargo.toml                      # workspace
├── crates/
│   ├── egret-bin/
│   ├── egret-server/
│   ├── egret-core/
│   ├── egret-config/
│   ├── egret-metastore/
│   ├── egret-secrets/
│   ├── egret-storage/
│   ├── egret-events/
│   ├── egret-auth/
│   ├── egret-authz/
│   ├── egret-ranger/
│   ├── egret-database/
│   ├── egret-engine/
│   ├── egret-catalog/
│   ├── egret-catalog-engine/
│   ├── egret-workspace/
│   ├── egret-iceberg-ext/
│   ├── egret-bi/
│   ├── egret-notebook/             # planned
│   ├── egret-dataapi/              # planned
│   ├── egret-workflow/             # planned
│   ├── egret-notify/               # planned
│   └── egret-monitor/              # planned
├── migrations/                     # sqlx migrations
├── frontend/                       # pnpm monorepo (apps/web + packages/*)
├── deploy/                         # Dockerfile, docker-compose, helm/
├── docs/
│   ├── en/                         # English (canonical): architecture/, adr/
│   └── vi/                         # Vietnamese (reference): architecture/, adr/
└── tests/                          # integration / contract / e2e
```

---

## 16. Roadmap & status

> **Reframe (v0.2):** the build targets **each workspace as a full self-contained data platform** across **4 pillars** (ADR-17). Each item below is its own brainstorm → spec → plan → subagent-build cycle.

### 16.1 Delivered
- **Foundation:** Cargo workspace + crates; Postgres metastore + sqlx migrations (0001–0029); typed config; dev docker-compose (Postgres + RustFS + Trino/StarRocks).
- **Admin / control plane:** local auth + session, workspaces + members + RBAC; database/engine registration (Trino REST + StarRocks MySQL); Ranger REST-compatible policy surface.
- **Analytics (BI):** Dataset / Chart / Dashboard with deep **Metabase parity** (table & pivot, per-series chart options, axis range, series rename, table view-as + mini-bar, PNG export, Vietnam-map geo).
- **Catalog (lakehouse core):** per-workspace Iceberg catalog via **embedded lakekeeper** — CE-1 spike → CE-2 tenancy + cutover → CE-3 per-token read/write scope → CE-4 minimal `EgretAuthorizer` + `namespace_scope` → CE-5 per-workspace storage config (each workspace sets its own S3 at create-time; secret via `egret-secrets`; global default when unset) → CE-6 GC / task-queue workers (lakekeeper's built-in tabular-expiration/purge + task-log-cleanup workers spawned; dropped tables are reclaimed) (all merged; see §10.3).

### 16.2 Next — Catalog pillar (follow-ons)
- **CE-5 follow-ons** — GCS / Azure storage (the `workspace_storage.provider` column is reserved; lakekeeper supports them natively); a full lakekeeper `SecretStore` bridge to `egret-secrets`; per-workspace `key_prefix` override; orphaned-secret cleanup on re-provision.
- Later: external catalog connections (register REST/Glue/Nessie per workspace), lakekeeper-parity polish (soft-delete, statistics, generic tables), lineage.

### 16.3 Next pillars
- **Analytics — Notebook (Daft):** per-workspace notebook (ADR-18) on a **Daft** kernel bound to the workspace Iceberg catalog, with fast ad-hoc SQL cells that **absorb SQL Lab**; chart/dashboard alerts.
- **Engineering:** mature Workflow + Data API (dbapi); Pipeline IDE (Airflow/Astronomer-style); CDC → Iceberg ingestion.
- **Admin:** self-service per-workspace database/engine connections; Secrets + per-workspace Storage config UI; Ranger data policies.

### 16.4 Hardening
- Monitoring/audit, Prometheus, Helm, leader-election scheduler, distributed event bus.

---

## 17. Open questions / future

- At what scale do we split `egret-catalog`/`egret-ranger` into separate services? (ADR-01 leaves it open.)
- Additional engines (BigQuery, ClickHouse, Postgres-as-source) — the trait is ready, prioritized later.
- A caching layer for dashboards (materialized) & result-based alerting (Superset-style alerts).
- Advanced row-level security & dynamic data-masking policy templates.
- Embedded analytics / public dashboard sharing.
- Versioning & migration for datasets/charts when the source schema changes.

---

## 18. Appendix: references

| Repo (in `data-portal/`) | Used for |
|---|---|
| `lakekeeper/` | Iceberg REST catalog, storage IO, authz, events (Rust patterns) |
| `ranger/` | Ranger REST API schema & download endpoints |
| `superset/` | dataset/chart/dashboard model, ECharts, websocket |
| `metabase/` | BI UX, query/dataset model |
| `sparklabx/` | **frontend design system/theme**, notebook, kernel, parquet preview |
| `dbapi/` | Data API Builder (dynamic SQL, gateway, client mgmt) |
| `n8n/` (external) | workflow automation model |

---

*Living document v0.2 (2026-06-27). Update via PR; record major changes as new ADRs under `docs/en/adr/`.*
