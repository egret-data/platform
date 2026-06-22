# Egret

> ⚠️ Bản tham khảo tiếng Việt. Bản gốc (canonical) là tiếng Anh: [`README.md`](README.md).

**Egret** là một data portal thống nhất viết bằng **Rust**, gộp năng lực của Metabase/Superset (BI &
explorer), Lakekeeper (Iceberg REST catalog), Apache Ranger (centralized access policy), DBApi (data
API builder) và n8n (workflow automation) — với frontend kế thừa design system của sparklabx.

## Hai portal trong một tool

- **Admin / Data Governance** — quản lý database, Iceberg catalog, Ranger policy, users/workspaces, monitoring/audit.
- **Explorer** — SQL Lab, dataset, chart, dashboard, notebook, Data API, workflow — **cô lập theo workspace** (dự án/phòng ban).

## Năng lực chính

- 🗄️ **Database management** — đăng ký/monitor database (MVP: Trino, StarRocks)
- 🛡️ **Apache Ranger API** — tương thích Ranger Admin REST để Trino/StarRocks tải policy
- 🧊 **Iceberg REST Catalog** — catalog tích hợp engine (tham khảo Lakekeeper)
- 📊 **BI / Explorer** — SQL Lab, dataset, chart (ECharts), dashboard (nhiều tab, filter, click behaviour), notebook
- 🔌 **Data API Builder** — cấu hình HTTP API từ SQL (dynamic SQL kiểu MyBatis)
- ⚙️ **Workflow Automation** — tự động hoá kiểu n8n (schedule, gửi mail, gọi API)
- 🏢 **Multi-workspace** · 🔐 **OIDC / SSO**

## Kiến trúc

- **Backend:** modular monolith — Rust workspace nhiều crate, 1 binary (axum + tokio + sqlx).
- **Frontend:** 1 SPA (React + Vite + Tailwind v4 + Radix), 2 route-group Admin/Explorer.
- **Hạ tầng:** PostgreSQL + Object storage (S3/MinIO) + Redis (tùy chọn).

📐 **Tài liệu kiến trúc đầy đủ:**
- 🇬🇧 **Bản gốc (canonical):** [`docs/en/architecture/2026-06-17-egret-architecture-design.md`](docs/en/architecture/2026-06-17-egret-architecture-design.md)
- 🇻🇳 **Bản tham khảo:** [`docs/vi/architecture/2026-06-17-egret-architecture-design.md`](docs/vi/architecture/2026-06-17-egret-architecture-design.md)

> Quy ước tài liệu: **tiếng Anh là bản gốc**, tiếng Việt là bản tham khảo; đặt trong `docs/en/` và `docs/vi/` (mirror cấu trúc).

## Trạng thái

🚧 Giai đoạn thiết kế. Xem [Roadmap](docs/en/architecture/2026-06-17-egret-architecture-design.md#16-phased-roadmap).

## Cấu trúc repo (dự kiến)

```text
crates/      # 21 crate Rust (egret-core, -server, -auth, -catalog, -ranger, -engine, -bi, -dataapi, -workflow, ...)
frontend/    # pnpm monorepo (apps/web + packages/ui|api-client|charts|workflow-canvas|...)
migrations/  # sqlx migrations
deploy/      # Dockerfile, compose, helm
docs/        # en/ (canonical) + vi/ (reference): architecture/, adr/
```

## License

TBD.
