# Egret

[![CI](https://github.com/egret-data/platform/actions/workflows/ci.yml/badge.svg)](https://github.com/egret-data/platform/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[Docs](https://egret-data.github.io/docs/) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)

> 🇻🇳 Vietnamese reference: [`README.vi.md`](README.vi.md)

**Egret is an enterprise data platform that lets many teams work on a single, unified data
foundation** — instead of every department standing up its own BI tool, catalog, and access-policy
stack. One deployment gives the whole organization **one source of truth**, while each team gets an
**isolated workspace**, and every access to data is **governed and audited**.

Written in **Rust** (a modular monolith — one binary, ~27 crates), Egret consolidates the
capabilities you would otherwise assemble from Metabase/Superset (BI & exploration), Lakekeeper
(Iceberg REST catalog), Apache Ranger (access policy), DBApi (data API builder) and n8n (workflow
automation) — into one governed platform with a shared design system.

Try everything on Docker with one command: see [docs/en/guides/demo-stack.md](docs/en/guides/demo-stack.md).

---

## Why Egret

| Problem in most orgs | How Egret solves it |
|---|---|
| Every team runs its own BI tool, catalog, and warehouse → data silos & duplication | **Single source of truth** — one shared Iceberg catalog + engines, one governance plane |
| Teams step on each other's dashboards, datasets, and connections | **Multi-workspace isolation** — each project/department gets its own sandbox |
| Access control is scattered and hard to audit | **Security-first** — unified authorization (Cedar PDP), OIDC/SSO with an owned machine identity (no shared engine creds), Ranger data-plane enforcement, tag-based masking, full audit log |
| Data lives in many systems (lake **and** RDBMS, NoSQL, streams) | **Query federation** — Trino reads Iceberg-on-S3 *and* 30+ external sources through one governed SQL surface |
| Onboarding a new team means new infrastructure | New workspace = a few clicks, no new stack |

---

## The big picture — one platform, many teams

```mermaid
flowchart LR
    subgraph TEAMS["Teams"]
        A["Team A · Analytics"]
        B["Team B · Engineering"]
        C["Team C · Finance"]
    end

    subgraph EGRET["EGRET — one unified platform (single Rust binary)"]
        WA["Workspace A<br/><i>isolated sandbox</i>"]
        WB["Workspace B<br/><i>isolated sandbox</i>"]
        WC["Workspace C<br/><i>isolated sandbox</i>"]
        GOV["Governance plane<br/>OIDC/SSO · Cedar authz<br/>Ranger · Audit"]
        WA --- GOV
        WB --- GOV
        WC --- GOV
    end

    subgraph SOT["Single source of truth"]
        ENG["Query engines<br/>Trino · StarRocks"]
        LAKE["Iceberg lakehouse — PRIMARY<br/>S3 / RustFS<br/>+ federated sources"]
    end

    A --> WA
    B --> WB
    C --> WC
    GOV ==>|"governed access"| ENG
    ENG --> LAKE
```

Every team logs into the **same platform** and reads from the **same governed data** — but works in a
**walled-off workspace**, and can only see what policy allows.

---

## Data plane — how Trino & StarRocks read your data (beyond S3)

Egret's data foundation is an **Iceberg lakehouse** — a shared **Iceberg REST catalog** (embedded
[lakekeeper](https://github.com/lakekeeper/lakekeeper)) on S3/RustFS. That lakehouse is the **primary
store**: Egret's own tables live there, and it is where both engines read and write. But the platform
is **not** limited to it — through **query federation**, both **Trino** and **StarRocks** can also
read data *in place* from external systems (relational databases, other lakehouse formats, NoSQL,
streams, cloud warehouses) without copying it in.

### Data-plane topology

The **Iceberg lakehouse on S3/RustFS is the primary store** (the source of truth). Both engines read
it over the **bold** paths; external systems are queried in place as **secondary** federation
(dotted). Only representative sources are drawn — the full connector list is in the table below.

```mermaid
flowchart TB
    subgraph CP["Egret · control + governance plane"]
        QR["egret-query router<br/>Cedar authz · per-user delegation · Ranger"]
    end
    QR --> TRINO
    QR --> SR

    TRINO["Trino<br/>(provisioned on k8s)"]
    SR["StarRocks<br/>(MySQL wire)"]

    subgraph LH["🧊 Lakehouse — the source of truth · PRIMARY"]
        direction TB
        ICE["Iceberg REST catalog<br/>(embedded lakekeeper · OAUTH2 vended creds)"]
        OBJ[("S3 / RustFS object storage")]
        ICE --> OBJ
    end

    TRINO ==>|"primary store"| ICE
    SR ==>|"primary store"| ICE

    subgraph FED["Federated sources · secondary — query where the data already lives"]
        direction LR
        RDBMS[("RDBMS<br/>PostgreSQL · MySQL · Oracle · Snowflake …")]
        LAKE2[("Other lakehouse / HMS<br/>Hive · Delta Lake · Hudi")]
        MISC[("NoSQL · streams · cloud<br/>MongoDB · Kafka · Elasticsearch · BigQuery …")]
    end

    TRINO -.->|"Trino catalogs (egret-managed)"| RDBMS
    TRINO -.-> LAKE2
    TRINO -.-> MISC
    SR -.->|"StarRocks external catalogs"| RDBMS
    SR -.-> LAKE2
```

- **Iceberg = primary.** The embedded catalog (`iceberg` connector, `rest` type) is auto-wired at
  provision time with **OAUTH2 vended credentials** — engines never hold long-lived S3 keys. This is
  where Egret's own tables are written and read.
- **Both engines federate.** **Trino** reaches external systems through **egret-managed catalogs**
  (the Add-catalog flow below); **StarRocks** brings its **own external catalogs** (Hive, Iceberg,
  JDBC, …). Federation lets a query join lake data with a live RDBMS or stream **without copying it in**.

### Adding a federated catalog — the credential flow

Any non-warehouse source is added through the **connector-first "Add catalog"** form. Egret stores
the catalog as an `EngineCatalog` row (`kind` = the Trino `connector.name`), keeps connection
**secrets in the vault**, and the orchestrator renders a `<name>.properties` file whose secret-backed
keys are injected from the pod's env Secret as `${ENV:VAR}` — **plaintext passwords never touch the
ConfigMap**.

```mermaid
sequenceDiagram
    actor Admin
    participant UI as "Add catalog (connector-first)"
    participant CP as "Egret control plane"
    participant VAULT as "Secrets vault"
    participant ORCH as "Orchestrator"
    participant K8S as "Kubernetes"
    participant TRINO as "Trino coordinator"
    participant SRC as "External source (e.g. PostgreSQL)"

    Admin->>UI: pick connector (postgresql) + properties + password
    UI->>CP: POST /engines/id/catalogs (kind, properties, secretProperties)
    CP->>VAULT: store password → secret_ref
    CP->>ORCH: build CatalogSpec (name, properties, env vars)
    ORCH->>K8S: render name.properties ConfigMap + env Secret
    K8S->>TRINO: mount catalog + roll pods
    Note over TRINO: connector.name = postgresql<br/>connection-password = env:PW (from Secret)
    TRINO->>SRC: JDBC connect (creds resolved from env)
    SRC-->>TRINO: rows
```

### Supported connectors (Trino, curated in the catalog wizard)

For **Trino** engines, Egret ships a curated, docs-backed picker for common connectors — plus a
**custom connector-name** escape hatch for anything Trino supports. The families below are
representative highlights, not the whole list:

| Family | Connectors (examples) |
|---|---|
| **Lakehouse / files** | **Iceberg** (Hive · Glue · JDBC · REST · Nessie metastore), Hive, Delta Lake, Hudi |
| **Relational (JDBC)** | PostgreSQL, MySQL, MariaDB, SQL Server, Oracle, Redshift, Snowflake, ClickHouse, … |
| **NoSQL / search** | MongoDB, Cassandra, Elasticsearch, OpenSearch, Redis |
| **Streaming** | Kafka, Kinesis |
| **Cloud / API** | BigQuery, Google Sheets |
| **Observability / misc** | Prometheus, JMX, Memory, TPC-H, TPC-DS |

> **StarRocks** reaches the same kinds of sources through its **own external catalogs** (Iceberg,
> Hive, JDBC/MySQL/PostgreSQL, Delta, …); those are configured on the StarRocks side, and Egret
> queries the result. **Object stores are S3-compatible** (AWS S3, RustFS, or any S3-API store via a
> custom `s3.endpoint`); lakehouse connectors can point at their own metastores/buckets, independent
> of Egret's embedded catalog.

### Governed query lifecycle

Federation never bypasses governance — every query is authorized, run **as the calling user**, and
audited:

```mermaid
flowchart TD
    U["User · SQL Lab / dataset / chart"] -->|"SELECT ..."| API["Egret /queries"]
    API --> PDP{"Cedar PDP<br/>authorized?"}
    PDP -->|deny| DENY["403 + audit"]
    PDP -->|allow| DELEG["Per-user delegation<br/>impersonate · jwt (TLS)<br/>— Trino AND StarRocks"]
    DELEG --> CONN["egret-query connector"]
    CONN -->|"Trino · HTTP, runs as caller"| TRINO["Trino"]
    CONN -->|"StarRocks · MySQL wire, runs as caller"| SR["StarRocks"]
    TRINO --> RANGER{"Ranger enforcement<br/>masking · row-filter<br/>(Trino + StarRocks plugins)"}
    SR --> RANGER
    RANGER --> FED[("Iceberg lakehouse (primary)<br/>+ federated sources")]
    RANGER --> RES["Governed rows"] --> U
    DELEG -.-> AUDIT[("Audit log")]
```

- **Per-user delegation (both engines)** — the query runs under the **caller's** identity, not a
  shared service account. **Trino and StarRocks both** support `impersonate` (a trusted delegator
  over TLS) and `jwt` (the caller's own short-lived JWT over TLS, OIDC users). Each engine then
  enforces **that user's** policies via its **Ranger** plugin (column masking, row-level filters) —
  Egret serves the Ranger-compatible policy download for both.

---

## Multi-workspace isolation

A **workspace** is a self-contained sandbox for a team or project. Dashboards, datasets, charts,
notebooks, saved queries, database connections and API definitions live *inside* a workspace and
never leak across the boundary. The shared **catalog** and **engines** sit underneath, so isolation
never means duplicating the data.

```mermaid
flowchart TB
    subgraph WA["Workspace · Analytics"]
        A1["Dashboards · Notebooks"]
        A2["Saved queries · Members / roles"]
    end
    subgraph WF["Workspace · Finance"]
        F1["Dashboards · Notebooks"]
        F2["Saved queries · Members / roles"]
    end
    WA <-.->|"no leak"| WF
    WA -->|"governed read/write"| SHARED
    WF -->|"governed read/write"| SHARED
    SHARED["Shared Catalog + Engines<br/>(single source of truth — no copies)"]
```

- **Isolated:** content in Workspace A is invisible to Workspace B.
- **Shared underneath:** both draw from one catalog — no copies, no drift.
- **Scoped access:** each workspace has its own members, roles and grants.

---

## Security-first, by design

Access is never an afterthought. Every request passes through layered controls before it reaches
data — and every decision is recorded.

```mermaid
flowchart TB
    REQ["User request"] --> L1
    L1["1 · Authentication — OIDC / SSO (Keycloak), groups from claims"] --> L2
    L2["2 · Authorization — Cedar PDP · roles, grants, hierarchy, explicit deny (deny wins)"] --> L3
    L3["3 · Workspace scope — object belongs to the caller's workspace"] --> L4
    L4["4 · Data-plane enforcement — Ranger · per-user delegation · tag-based masking / row-filter"] --> L5
    L5["5 · Audit log — every decision recorded"] --> OK["✅ Governed data access"]
```

- **One authorizer** — Cedar is the single policy decision point (template-per-role + linked
  policy-per-grant, deny-wins).
- **OIDC / SSO** — enterprise identity via Keycloak; groups come from token claims.
- **Data-plane governance** — Ranger-compatible policy download for Trino/StarRocks, per-user
  delegated queries, tag-based **column masking** and **row-level filters**.
- **Classification & ABAC** — data classification tags gate access; attribute/clearance-based rules
  (Cedar) for BI objects.
- **Auditable** — a real audit log captures who accessed what, when, and whether it was allowed.

### Identity, secrets & isolation

Security-first also means **owning identity end-to-end** and **never trusting a shared credential**:

- **Machine-IdP token broker** — Egret mints **per-user, short-lived engine tokens** from its own
  embedded identity provider (`aud: engine`, signed by Egret's JWKS). Engines trust Egret's issuer
  directly, so there are **no long-lived engine passwords**, and even **local / non-OIDC users** get
  scoped, delegated access. SSO stays authentication-only + JIT provisioning.
- **SSO back-channel logout** — an OIDC logout token from Keycloak (RS256, JWKS-verified) revokes
  **all** of that user's sessions server-side, so a central sign-out really ends access everywhere.
- **Session & account lifecycle** — admin password reset, immediate lockout via **session
  revocation**, hard-delete with **content re-assignment** (no orphaned dashboards), and a
  **last-admin guard** that stops the platform from being locked out of itself.
- **Typed secrets vault** — Kubernetes-style typed, multi-field secrets (`Opaque` / `kubernetes.io/tls`
  / `basic-auth`) back every connector, storage and TLS credential. Secrets are injected into pods as
  env references — **plaintext never lands in a ConfigMap** — and ingress TLS is served from
  operator-provided certificates.
- **Runtime isolation** — each notebook runs in a **hardened, per-user × per-workspace pod** (non-root
  `uid 1000`, all Linux capabilities dropped, a locked-down `NetworkPolicy`, minimal RBAC) and writes
  to a **private Iceberg namespace** via a namespace-scoped token — never a shared kernel.

---

## Core capabilities — every one is table-stakes for a data platform

An enterprise data platform can't ship half of these. Each capability below closes a specific gap that
would otherwise force a separate tool, a separate login, or a separate security review:

- 🏢 **Multi-workspace** — isolated sandbox per team/project over shared data *(no collisions, no silos)*
- 🔐 **Unified security** — OIDC/SSO, Cedar authorization, Ranger enforcement, tag masking, audit *(one authorizer, not five)*
- 🪪 **Owned identity & secrets** — machine-IdP per-user engine tokens, back-channel logout, session revocation, typed secrets vault *(no long-lived or shared credentials)*
- 🧊 **Iceberg REST Catalog** — embedded, engine-integrated, per-workspace tenancy *(the single source of truth)*
- 🔌 **Query federation** — Trino & StarRocks read Iceberg-on-S3 **and** 30+ external sources (RDBMS, NoSQL, streams, cloud) through one governed SQL surface *(query data where it already lives — no copies)*
- ⚙️ **Engine provisioning** — provision & manage **Trino** *and* **StarRocks** clusters on Kubernetes (HPA/KEDA autoscaling, bundled Ranger plugin) *(elastic, self-service compute)*
- 🕵️ **Per-user delegation** — queries run as the calling user (`impersonate` / `jwt` over TLS), governed by that user's data policies *(accountability at the row/column level)*
- 📊 **BI / Explorer** — SQL Lab, datasets, charts, dashboards (multi-tab, filters, click behaviour) with deep Metabase parity *(analysts self-serve)*
- 📓 **Notebook** — per-workspace notebooks on an **isolated** Daft kernel, with in-notebook Python viz (plotly / matplotlib) *(data science, in-platform)*
- 🔌 **Data API Builder** — expose governed HTTP APIs from SQL (MyBatis-style dynamic SQL) *(serve applications, not just dashboards)*
- ⚙️ **Workflow Automation** — n8n-style automation: schedule, email, call APIs *(operationalize the data)*

---

## Architecture

- **Backend** — modular monolith: a Rust workspace of ~27 crates, one binary (axum + tokio + sqlx).
- **Frontend** — a single SPA (React + Vite + Tailwind v4 + Radix), two route groups: **Control
  plane** (governance/admin) and **Explorer** (per-workspace work).
- **Infrastructure** — PostgreSQL + object storage (S3/RustFS); engines on Kubernetes.

```mermaid
flowchart TB
    subgraph FE["Frontend — React SPA"]
        CPUI["Control plane<br/>(users, roles, policies, engines, catalogs, warehouses)"]
        EXP["Explorer<br/>(SQL Lab, datasets, charts, dashboards, notebooks)"]
    end

    subgraph BE["Backend — Rust modular monolith (one binary)"]
        direction TB
        SRV["egret-server · axum HTTP"]
        subgraph GOVN["Governance"]
            AUTH["egret-auth · OIDC/SSO"]
            POL["egret-policy · Cedar PDP + Ranger"]
            AUD["egret-audit"]
        end
        subgraph DATA["Data & engines"]
            CATE["egret-catalog-engine · Iceberg REST (lakekeeper)"]
            EMG["egret-engine-mgmt · engines + catalogs"]
            ORCH["egret-provisioner · Trino on k8s"]
            QRY["egret-query · federated SQL router"]
            STOR["egret-storage · S3/RustFS"]
            SEC["egret-secrets · vault"]
        end
        subgraph APPS["Workspace apps"]
            BI["egret-bi"]
            NB["egret-notebook · Daft kernel"]
            WSP["egret-workspace"]
        end
        SRV --- GOVN & DATA & APPS
    end

    subgraph INFRA["Infrastructure"]
        PGSQL[("PostgreSQL")]
        OBJ[("S3 / RustFS")]
        K8S["Kubernetes<br/>(Trino clusters)"]
        IDP["Keycloak (OIDC)"]
    end

    FE -->|HTTPS| SRV
    AUTH --> IDP
    DATA --> PGSQL
    CATE --> OBJ
    ORCH --> K8S
    QRY --> K8S
```

📐 **Full architecture document:**
- 🇬🇧 **Canonical:** [`docs/en/architecture/2026-06-17-egret-architecture-design.md`](docs/en/architecture/2026-06-17-egret-architecture-design.md)
- 🇻🇳 **Reference:** [`docs/vi/architecture/2026-06-17-egret-architecture-design.md`](docs/vi/architecture/2026-06-17-egret-architecture-design.md)

> Documentation convention: **English is canonical**, Vietnamese is a reference translation; placed
> under `docs/en/` and `docs/vi/` (mirrored structure).

---

## Status

**Foundation, governance and analytics are built and in use:** authentication/SSO with a
**machine-IdP token broker** (per-user engine tokens, no long-lived engine creds) and **SSO
back-channel logout**, multi-workspace, unified Cedar authorization across all enforcement sites,
RBAC & access-control console, Ranger data-plane enforcement with tag-based masking, a **typed
secrets vault**, and a full audit log. **BI/Explorer** ships with deep Metabase parity (datasets,
charts, dashboards) — verified end-to-end against a provisioned Trino engine. The **embedded Iceberg
catalog** runs live end-to-end (provision → auth → data plane); **engine provisioning** covers **both
Trino and StarRocks on Kubernetes** with **catalog federation** (30+ connectors) and **secure
per-user delegation** (`impersonate` + `jwt` over TLS, live-verified); and **notebooks** run real
cells in **isolated per-user kernel pods** with in-notebook Python viz. Ongoing: catalog hardening
(storage profiles, GC), notebook/Daft delegation, and the Data API + Workflow surfaces.

See the [Roadmap & status](docs/en/architecture/2026-06-17-egret-architecture-design.md#16-roadmap--status).

---

## Repository layout

```text
crates/      # ~27 Rust crates (egret-core, -server, -auth, -catalog-*, -policy, -engine-mgmt,
             #  -orchestrator, -query, -storage, -secrets, -bi, -notebook, -workspace, ...)
frontend/    # React + Vite SPA (control plane + explorer)
migrations/  # sqlx migrations
deploy/      # Dockerfile, compose, helm
docs/        # en/ (canonical) + vi/ (reference): architecture/, adr/, plans/, specs/
```

## License

TBD.

---

## Contributing & license

See [CONTRIBUTING.md](CONTRIBUTING.md) (user-visible changes must ship with docs) and
[SECURITY.md](SECURITY.md). Licensed under the [Apache License 2.0](LICENSE).
