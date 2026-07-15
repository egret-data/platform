# Egret

> 🇻🇳 Vietnamese reference: [`README.vi.md`](README.vi.md)

**Egret is an enterprise data platform that lets many teams work on a single, unified data
foundation** — instead of every department standing up its own BI tool, catalog, and access-policy
stack. One deployment gives the whole organization **one source of truth**, while each team gets an
**isolated workspace**, and every access to data is **governed and audited**.

Written in **Rust** (a modular monolith — one binary, many crates), Egret consolidates the
capabilities you would otherwise assemble from Metabase/Superset (BI & exploration), Lakekeeper
(Iceberg REST catalog), Apache Ranger (access policy), DBApi (data API builder) and n8n (workflow
automation) — into one governed platform with a shared design system.

---

## Why Egret

| Problem in most orgs | How Egret solves it |
|---|---|
| Every team runs its own BI tool, catalog, and warehouse → data silos & duplication | **Single source of truth** — one shared Iceberg catalog + engines, one governance plane |
| Teams step on each other's dashboards, datasets, and connections | **Multi-workspace isolation** — each project/department gets its own sandbox |
| Access control is scattered and hard to audit | **Security-first** — unified authorization (Cedar PDP), OIDC/SSO, Ranger data-plane enforcement, tag-based masking, full audit log |
| Onboarding a new team means new infrastructure | New workspace = a few clicks, no new stack |

---

## The big picture — one platform, many teams

```text
     Team A            Team B            Team C
   (Analytics)      (Engineering)      (Finance)
       │                 │                 │
       ▼                 ▼                 ▼
 ┌───────────────────────────────────────────────────────┐
 │                EGRET — one unified platform            │
 │                                                        │
 │   Governance plane:  AuthN (OIDC/SSO) · AuthZ (Cedar)  │
 │                      · Ranger · Audit                  │
 │  ────────────────────────────────────────────────────  │
 │   Isolated workspaces:                                 │
 │     ┌──────────┐   ┌──────────┐   ┌──────────┐         │
 │     │ Workspace│   │ Workspace│   │ Workspace│         │
 │     │    A     │   │    B     │   │    C     │         │
 │     └──────────┘   └──────────┘   └──────────┘         │
 └───────────────────────────┬───────────────────────────┘
                             │  governed access
                             ▼
 ┌───────────────────────────────────────────────────────┐
 │            SINGLE SOURCE OF TRUTH                      │
 │   Shared Iceberg REST Catalog                          │
 │        →  Engines (Trino · StarRocks)                  │
 │              →  Object storage (S3 / MinIO)            │
 └───────────────────────────────────────────────────────┘
```

Every team logs into the **same platform** and reads from the **same governed data** — but works in
a **walled-off workspace**, and can only see what policy allows.

---

## Multi-workspace isolation

A **workspace** is a self-contained sandbox for a team or project. Dashboards, datasets, charts,
notebooks, saved queries, database connections and API definitions live *inside* a workspace and
never leak across the boundary. The shared **catalog** and **engines** sit underneath, so isolation
never means duplicating the data.

```text
  ┌─────────────────────────┐      ┌─────────────────────────┐
  │  Workspace: Analytics   │      │  Workspace: Finance     │
  │  • Dashboards           │  ╳   │  • Dashboards           │
  │  • Notebooks            │ ◄──► │  • Notebooks            │
  │  • Saved queries        │ (no  │  • Saved queries        │
  │  • Members / roles      │ leak)│  • Members / roles      │
  └────────────┬────────────┘      └────────────┬────────────┘
               │                                │
   governed read/write                 governed read/write
               │                                │
               ▼                                ▼
        ┌──────────────────────────────────────────────┐
        │   Shared Catalog + Engines                    │
        │   (single source of truth — no copies)        │
        └──────────────────────────────────────────────┘
```

- **Isolated:** content in Workspace A is invisible to Workspace B.
- **Shared underneath:** both draw from one catalog — no copies, no drift.
- **Scoped access:** each workspace has its own members, roles and grants.

---

## Security-first, by design

Access is never an afterthought. Every request passes through layered controls before it reaches
data — and every decision is recorded.

```text
   User request
        │
        ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ 1 · Authentication   OIDC / SSO (Keycloak), groups from claims│
  ├─────────────────────────────────────────────────────────────┤
  │ 2 · Authorization    Cedar PDP — roles, grants, hierarchy,    │
  │                      explicit deny (deny wins)                │
  ├─────────────────────────────────────────────────────────────┤
  │ 3 · Workspace scope  object belongs to the caller's workspace │
  ├─────────────────────────────────────────────────────────────┤
  │ 4 · Data-plane       Apache Ranger · per-user delegation ·    │
  │     enforcement      tag-based masking / row-level filter     │
  ├─────────────────────────────────────────────────────────────┤
  │ 5 · Audit log        every decision recorded                  │
  └─────────────────────────────────────────────────────────────┘
        │
        ▼
   ✅ Governed data access
```

- **One authorizer** — Cedar is the single policy decision point (template-per-role + linked
  policy-per-grant, deny-wins).
- **OIDC / SSO** — enterprise identity via Keycloak; groups come from token claims.
- **Data-plane governance** — Ranger-compatible policy download for Trino/StarRocks, per-user
  delegated queries, tag-based **column masking** and **row-level filters**.
- **Classification & ABAC** — data classification tags gate access; attribute/clearance-based rules
  (Cedar) for BI objects.
- **Auditable** — a real audit log captures who accessed what, when, and whether it was allowed.

---

## Core capabilities

- 🏢 **Multi-workspace** — isolated sandbox per team/project over shared data
- 🔐 **Unified security** — OIDC/SSO, Cedar authorization, Ranger enforcement, tag masking, audit
- 🧊 **Iceberg REST Catalog** — embedded, engine-integrated, per-workspace tenancy (single source of truth)
- ⚙️ **Engine provisioning** — provision & manage Trino / StarRocks / RisingWave on Kubernetes
- 📊 **BI / Explorer** — SQL Lab, datasets, charts, dashboards (multi-tab, filters, click behaviour) with deep Metabase parity
- 📓 **Notebook** — per-workspace notebooks with a Daft kernel
- 🗄️ **Database management** — register & monitor databases (Trino, StarRocks)
- 🔌 **Data API Builder** — expose HTTP APIs from SQL (MyBatis-style dynamic SQL)
- ⚙️ **Workflow Automation** — n8n-style automation (schedule, email, call APIs)

---

## Architecture

- **Backend** — modular monolith: a Rust workspace of ~27 crates, one binary (axum + tokio + sqlx).
- **Frontend** — a single SPA (React + Vite + Tailwind v4 + Radix), two route groups: **Control
  plane** (governance/admin) and **Explorer** (per-workspace work).
- **Infrastructure** — PostgreSQL + object storage (S3/MinIO) + Redis (optional); engines on Kubernetes.

📐 **Full architecture document:**
- 🇬🇧 **Canonical:** [`docs/en/architecture/2026-06-17-egret-architecture-design.md`](docs/en/architecture/2026-06-17-egret-architecture-design.md)
- 🇻🇳 **Reference:** [`docs/vi/architecture/2026-06-17-egret-architecture-design.md`](docs/vi/architecture/2026-06-17-egret-architecture-design.md)

> Documentation convention: **English is canonical**, Vietnamese is a reference translation; placed
> under `docs/en/` and `docs/vi/` (mirrored structure).

---

## Status

**Foundation, governance and analytics are built and in use:** authentication/SSO, multi-workspace,
unified Cedar authorization across all enforcement sites, RBAC & access-control console, Ranger
data-plane enforcement with tag-based masking, and a full audit log. **BI/Explorer** ships with deep
Metabase parity (datasets, charts, dashboards). The **embedded Iceberg catalog** runs live
end-to-end (provision → auth → data plane), **engine provisioning** (Trino/Stackable) is shipped,
and **notebooks** run real cells end-to-end. Ongoing: catalog hardening (storage profiles, GC),
notebook/Daft delegation, and the Data API + Workflow surfaces.

See the [Roadmap & status](docs/en/architecture/2026-06-17-egret-architecture-design.md#16-roadmap--status).

---

## Repository layout

```text
crates/      # ~27 Rust crates (egret-core, -server, -auth, -catalog-*, -policy, -engine-mgmt, -bi, -notebook, ...)
frontend/    # React + Vite SPA (control plane + explorer)
migrations/  # sqlx migrations
deploy/      # Dockerfile, compose, helm
docs/        # en/ (canonical) + vi/ (reference): architecture/, adr/, plans/
```

## License

TBD.
