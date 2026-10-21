<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset=".github/assets/egret-wordmark-dark.svg">
  <img alt="Egret" src=".github/assets/egret-wordmark-light.svg" width="260">
</picture>

### One governed data platform for every team

Isolated workspaces on one shared, governed data foundation: BI, data APIs, workflows and data apps,\
with access policy enforced in the query engine and every decision audited.

[![CI](https://github.com/egret-data/platform/actions/workflows/ci.yml/badge.svg)](https://github.com/egret-data/platform/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Docs](https://img.shields.io/badge/docs-egret--data.github.io-0f9d8a.svg)](https://egret-data.github.io/docs/)
[![Rust](https://img.shields.io/badge/built%20with-Rust-dea584.svg)](rust-toolchain.toml)

[**Documentation**](https://egret-data.github.io/docs/) ·
[**Quick start**](#quick-start) ·
[**Architecture**](#how-it-fits-together) ·
[**Known issues**](https://egret-data.github.io/docs/platform/known-issues) ·
[**Contributing**](CONTRIBUTING.md)

<sub>🇻🇳 Tiếng Việt: [`README.vi.md`](README.vi.md)</sub>

<br>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset=".github/assets/architecture-dark.svg">
  <img alt="Egret architecture: teams work in isolated workspaces on one governed platform; governed queries run on Trino and StarRocks over an Iceberg lakehouse and external sources." src=".github/assets/architecture-light.svg" width="100%">
</picture>

</div>

## What is Egret

Most organisations end up with one BI tool, one catalog, one access-policy stack and one workflow tool **per
department**: silos, copies of the data, and access rules nobody can audit. Egret replaces that with
**one deployment**:

- **One data foundation.** A shared Iceberg lakehouse and the query engines on top of it. Teams do not copy data; they get a workspace.
- **Isolated workspaces.** Dashboards, datasets, charts, saved queries, connections and APIs live inside a workspace and never leak across it.
- **Governance in the engine, not around it.** One authorizer (Cedar), per-user delegated queries, Ranger masking and row filters, and an audit log of every decision.

It is a Rust modular monolith (one binary, 37 crates) with a React front end, and it brings together what you would
otherwise assemble from Metabase or Superset (BI), Lakekeeper (Iceberg catalog), Apache Ranger (access policy),
a data-API builder and a workflow engine.

## Highlights

| Area | What you get | |
|---|---|---|
| **Explore** | SQL Lab, datasets with metrics, 19 chart types, dashboards with filters and drill-through, alerts and reports (email and Slack, PNG and PDF). | [Docs](https://egret-data.github.io/docs/platform/explore/sql-lab) |
| **Data API** | Publish a parameterised `SELECT` as a REST endpoint with OAuth2 clients, versions, rate limits and an activity log. | [Docs](https://egret-data.github.io/docs/platform/data-api/overview) |
| **Data Apps** | [marimo](https://marimo.io) apps that run as the person viewing them, so the data policy follows the viewer. | [Docs](https://egret-data.github.io/docs/platform/explore/apps) |
| **Workflows** | YAML flows with schedules, webhooks and triggers, executions with logs, namespaces and files. | [Docs](https://egret-data.github.io/docs/platform/workflows/concepts) |
| **Governance** | Cedar authorization, Ranger-compatible policy for Trino and StarRocks, tag-based masking, classifications, audit log, SSO (OIDC). | [Docs](https://egret-data.github.io/docs/platform/data-governance/overview) |
| **Engines and catalog** | Provision Trino and StarRocks from the UI (Docker or Kubernetes); an embedded Iceberg REST catalog; query federation over 30+ connectors. | [Docs](https://egret-data.github.io/docs/platform/control-plane/query-engines) |

## Screenshots

<table>
  <tr>
    <td width="50%"><a href="https://egret-data.github.io/docs/platform/explore/dashboards"><img alt="A dashboard in an Egret workspace" src="https://egret-data.github.io/docs/img/platform/explore-dashboard-view.png"></a><br><sub>A dashboard in a workspace.</sub></td>
    <td width="50%"><a href="https://egret-data.github.io/docs/platform/explore/charts"><img alt="The chart editor" src="https://egret-data.github.io/docs/img/platform/explore-chart-editor.png"></a><br><sub>The chart editor.</sub></td>
  </tr>
</table>

## Quick start

Run the whole platform on Docker, with no Kubernetes: Egret, Postgres, object storage, Keycloak SSO, a Trino engine,
Data Apps, a mail catcher, a PDF/PNG renderer, and a sample workspace with a dataset, charts and a dashboard.

```bash
git clone https://github.com/egret-data/platform.git
cd platform
just demo-up
```

Then open <http://egret.localtest.me:8080> and sign in with **Admin sign-in (break-glass)**:
`admin@egret.local` / `Admin12345!`. Single sign-on users `admin-sso` and `member-sso` are there too, and every
mail the demo sends lands in Mailpit at <http://localhost:8025>.

- **Needs** Docker with Compose v2, [`just`](https://github.com/casey/just), `curl` and `python3`, and 6 to 8 GB of free RAM.
- **The first run builds the backend** (20 to 40 minutes); later runs reuse the images.
- **Demo only.** The stack uses fixed public secrets. Never expose it to a network.

The full guide, with ports, engines, Data Apps and how to reset, is [`docs/en/guides/demo-stack.md`](docs/en/guides/demo-stack.md).
For production, see [Deploy and operate](https://egret-data.github.io/docs/platform/deploy-operate/kubernetes) (Kubernetes and Helm,
`deploy/helm`) and the [configuration keys](https://egret-data.github.io/docs/platform/reference/configuration-keys).

## How it fits together

- **Backend:** a Rust workspace (axum, tokio, sqlx) built into one binary that serves the API, the control plane and every workspace.
- **Front end:** one React single-page app (Vite, Tailwind, Radix) with two areas: **Explore** (per workspace) and **Control Plane and Governance** (platform admins).
- **State:** PostgreSQL for metadata and S3-compatible object storage (RustFS in the demo) for the lakehouse.
- **Engines:** Trino and StarRocks run as containers or pods that Egret provisions and watches; each query runs **as the calling user**.
- **Identity:** OIDC single sign-on for people; Egret mints short-lived per-user tokens for engines, so there are no shared engine passwords.

Deeper reading: [platform overview](docs/en/architecture/platform-overview.md) (data plane, federation, the governed query
lifecycle, isolation, security) and the full [architecture design](docs/en/architecture/2026-06-17-egret-architecture-design.md).

## Documentation

| | |
|---|---|
| [Get started](https://egret-data.github.io/docs/platform/get-started/overview) | What Egret is, the concepts, a first dashboard. |
| [Run the demo stack](https://egret-data.github.io/docs/platform/get-started/run-the-demo-stack) | The Docker quick start in detail. |
| [Explore, Data API, Workflows](https://egret-data.github.io/docs/platform/explore/sql-lab) | Page-by-page user guide with screenshots. |
| [Governance and Control Plane](https://egret-data.github.io/docs/platform/data-governance/overview) | Access policy, engines, catalogs, storage, secrets. |
| [Reference](https://egret-data.github.io/docs/platform/reference/configuration-keys) | Configuration keys and the REST API. |
| [Known issues](https://egret-data.github.io/docs/platform/known-issues) | What is open, with severity, and what is planned. |

The documentation lives in [`egret-data/docs`](https://github.com/egret-data/docs); a change that alters what users see ships with its docs update.

## Repository layout

```text
crates/       37 Rust crates (server, auth, policy, catalog, engines, query, bi, workflow, apps, ...)
frontend/     React + Vite single-page app
migrations/   sqlx migrations
deploy/       Dockerfiles, the Docker demo stack, Helm chart, Keycloak and engine configs
apps-image/   the Data Apps (marimo) runtime image
renderer/     the PDF and PNG renderer used by reports
plugin-host/  the JVM host that runs workflow plugins
docs/         architecture notes, specs and plans (English canonical, Vietnamese reference)
```

## Status

Egret is under active development. Authentication and SSO, workspaces, unified authorization, Ranger enforcement, the
secrets vault, BI and Explore, the Iceberg catalog, engine provisioning, Data Apps, the Data API and workflows are built and in use.
Open defects and gaps are tracked, with severity, in [Known issues](https://egret-data.github.io/docs/platform/known-issues).

## Contributing and security

Read [CONTRIBUTING.md](CONTRIBUTING.md): Conventional Commits, tests with every change, and docs with every user-visible change.
Report a vulnerability as [SECURITY.md](SECURITY.md) describes, not in a public issue. Everyone taking part follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

Licensed under the [Apache License 2.0](LICENSE).
