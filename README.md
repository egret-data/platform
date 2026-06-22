# Egret

> 🇻🇳 Vietnamese reference: [`README.vi.md`](README.vi.md)

**Egret** is a unified data portal written in **Rust** that combines the capabilities of
Metabase/Superset (BI & exploration), Lakekeeper (Iceberg REST catalog), Apache Ranger (centralized
access policy), DBApi (data API builder), and n8n (workflow automation) — with a frontend that
inherits the sparklabx design system.

## Two portals in one tool

- **Admin / Data Governance** — manage databases, the Iceberg catalog, Ranger policies, users/workspaces, monitoring/audit.
- **Explorer** — SQL Lab, dataset, chart, dashboard, notebook, Data APIs, workflows — **isolated per workspace** (project/department).

## Core capabilities

- 🗄️ **Database management** — register/monitor databases (MVP: Trino, StarRocks)
- 🛡️ **Apache Ranger API** — Ranger Admin REST-compatible so Trino/StarRocks download policies
- 🧊 **Iceberg REST Catalog** — engine-integrated catalog (references Lakekeeper)
- 📊 **BI / Explorer** — SQL Lab, dataset, chart (ECharts), dashboard (multi-tab, filters, click behaviour), notebook
- 🔌 **Data API Builder** — configure HTTP APIs from SQL (MyBatis-style dynamic SQL)
- ⚙️ **Workflow Automation** — n8n-style automation (schedule, send email, call APIs)
- 🏢 **Multi-workspace** · 🔐 **OIDC / SSO**

## Architecture

- **Backend:** modular monolith — a Rust workspace of many crates, one binary (axum + tokio + sqlx).
- **Frontend:** a single SPA (React + Vite + Tailwind v4 + Radix), two route groups Admin/Explorer.
- **Infrastructure:** PostgreSQL + object storage (S3/MinIO) + Redis (optional).

📐 **Full architecture document:**
- 🇬🇧 **Canonical:** [`docs/en/architecture/2026-06-17-egret-architecture-design.md`](docs/en/architecture/2026-06-17-egret-architecture-design.md)
- 🇻🇳 **Reference:** [`docs/vi/architecture/2026-06-17-egret-architecture-design.md`](docs/vi/architecture/2026-06-17-egret-architecture-design.md)

> Documentation convention: **English is canonical**, Vietnamese is a reference translation; placed under `docs/en/` and `docs/vi/` (mirrored structure).

## Status

🚧 Design phase. See the [Roadmap](docs/en/architecture/2026-06-17-egret-architecture-design.md#16-phased-roadmap).

## Repository layout (planned)

```text
crates/      # 21 Rust crates (egret-core, -server, -auth, -catalog, -ranger, -engine, -bi, -dataapi, -workflow, ...)
frontend/    # pnpm monorepo (apps/web + packages/ui|api-client|charts|workflow-canvas|...)
migrations/  # sqlx migrations
deploy/      # Dockerfile, compose, helm
docs/        # en/ (canonical) + vi/ (reference): architecture/, adr/
```

## License

TBD.
