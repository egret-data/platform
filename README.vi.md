# Egret

> ⚠️ Bản tham khảo tiếng Việt. Bản gốc (canonical) là tiếng Anh: [`README.md`](README.md).

**Egret là một nền tảng dữ liệu (data platform) cấp doanh nghiệp, cho phép nhiều team cùng làm việc
trên MỘT nền tảng dữ liệu thống nhất** — thay vì mỗi phòng ban phải tự dựng riêng công cụ BI,
catalog và hệ thống access-policy. Một lần triển khai duy nhất mang lại cho toàn tổ chức **một nguồn
sự thật duy nhất (single source of truth)**, mỗi team có một **workspace được cô lập**, và **mọi truy
cập dữ liệu đều được kiểm soát và ghi log (audit)**.

Được viết bằng **Rust** (modular monolith — một binary, nhiều crate), Egret gộp lại những năng lực mà
bình thường bạn phải ghép từ Metabase/Superset (BI & explorer), Lakekeeper (Iceberg REST catalog),
Apache Ranger (access policy), DBApi (data API builder) và n8n (workflow automation) — thành một nền
tảng có governance thống nhất và cùng một design system.

---

## Vì sao chọn Egret

| Vấn đề thường gặp | Cách Egret giải quyết |
|---|---|
| Mỗi team tự chạy BI, catalog, warehouse riêng → silo & trùng lặp dữ liệu | **Single source of truth** — chung một Iceberg catalog + engine, chung một governance plane |
| Các team dẫm chân nhau lên dashboard, dataset, connection | **Cô lập theo workspace** — mỗi dự án/phòng ban có sandbox riêng |
| Access control rải rác, khó audit | **Security-first** — authorization thống nhất (Cedar PDP), OIDC/SSO, enforcement bằng Ranger, masking theo tag, audit log đầy đủ |
| Thêm team mới = phải dựng hạ tầng mới | Thêm workspace = vài cú click, không cần stack mới |

---

## Bức tranh tổng thể — một nền tảng, nhiều team

```text
     Team A            Team B            Team C
   (Analytics)      (Engineering)      (Finance)
       │                 │                 │
       ▼                 ▼                 ▼
 ┌───────────────────────────────────────────────────────┐
 │            EGRET — nền tảng thống nhất duy nhất        │
 │                                                        │
 │   Governance plane:  AuthN (OIDC/SSO) · AuthZ (Cedar)  │
 │                      · Ranger · Audit                  │
 │  ────────────────────────────────────────────────────  │
 │   Workspace được cô lập:                               │
 │     ┌──────────┐   ┌──────────┐   ┌──────────┐         │
 │     │ Workspace│   │ Workspace│   │ Workspace│         │
 │     │    A     │   │    B     │   │    C     │         │
 │     └──────────┘   └──────────┘   └──────────┘         │
 └───────────────────────────┬───────────────────────────┘
                             │  truy cập có kiểm soát
                             ▼
 ┌───────────────────────────────────────────────────────┐
 │            SINGLE SOURCE OF TRUTH                      │
 │   Shared Iceberg REST Catalog                          │
 │        →  Engine (Trino · StarRocks)                   │
 │              →  Object storage (S3 / MinIO)            │
 └───────────────────────────────────────────────────────┘
```

Mọi team đăng nhập vào **cùng một nền tảng** và đọc từ **cùng một tập dữ liệu được kiểm soát** — nhưng
làm việc trong **workspace tách biệt**, và chỉ thấy được những gì policy cho phép.

---

## Cô lập đa workspace (multi-workspace)

Một **workspace** là sandbox độc lập cho một team hoặc dự án. Dashboard, dataset, chart, notebook,
saved query, database connection và định nghĩa API nằm *bên trong* workspace và không rò rỉ qua ranh
giới. **Catalog** và **engine** dùng chung nằm bên dưới, nên cô lập không có nghĩa là nhân bản dữ liệu.

```text
  ┌─────────────────────────┐      ┌─────────────────────────┐
  │  Workspace: Analytics   │      │  Workspace: Finance     │
  │  • Dashboards           │  ╳   │  • Dashboards           │
  │  • Notebooks            │ ◄──► │  • Notebooks            │
  │  • Saved queries        │(không│  • Saved queries        │
  │  • Member / role        │ rò rỉ)│  • Member / role        │
  └────────────┬────────────┘      └────────────┬────────────┘
               │                                │
     read/write có kiểm soát          read/write có kiểm soát
               │                                │
               ▼                                ▼
        ┌──────────────────────────────────────────────┐
        │   Catalog + Engine dùng chung                 │
        │   (single source of truth — không copy)       │
        └──────────────────────────────────────────────┘
```

- **Cô lập:** nội dung trong Workspace A không nhìn thấy được từ Workspace B.
- **Dùng chung bên dưới:** cả hai đọc từ một catalog — không copy, không lệch dữ liệu.
- **Access theo scope:** mỗi workspace có member, role và grant riêng.

---

## Security-first ngay từ thiết kế

Access không bao giờ là chuyện phụ. Mọi request đi qua nhiều lớp kiểm soát trước khi chạm tới dữ
liệu — và mọi quyết định đều được ghi lại.

```text
   Request của user
        │
        ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ 1 · Authentication   OIDC / SSO (Keycloak), group từ claim   │
  ├─────────────────────────────────────────────────────────────┤
  │ 2 · Authorization    Cedar PDP — role, grant, phân cấp,      │
  │                      explicit deny (deny thắng)              │
  ├─────────────────────────────────────────────────────────────┤
  │ 3 · Workspace scope  object thuộc đúng workspace người gọi   │
  ├─────────────────────────────────────────────────────────────┤
  │ 4 · Enforcement      Apache Ranger · delegation theo user ·  │
  │     data-plane       masking / row-filter theo tag           │
  ├─────────────────────────────────────────────────────────────┤
  │ 5 · Audit log        ghi lại mọi quyết định                  │
  └─────────────────────────────────────────────────────────────┘
        │
        ▼
   ✅ Truy cập dữ liệu có kiểm soát
```

- **Một authorizer duy nhất** — Cedar là điểm quyết định policy duy nhất (template theo role + linked
  policy theo grant, deny thắng).
- **OIDC / SSO** — định danh doanh nghiệp qua Keycloak; group lấy từ token claim.
- **Governance ở data-plane** — tải policy tương thích Ranger cho Trino/StarRocks, query delegate theo
  từng user, **masking cột** và **row-level filter** theo tag.
- **Classification & ABAC** — tag phân loại dữ liệu gate access; luật theo thuộc tính/độ mật (Cedar)
  cho các object BI.
- **Audit được** — audit log thật ghi lại ai truy cập gì, khi nào, và có được phép hay không.

---

## Năng lực chính

- 🏢 **Multi-workspace** — sandbox cô lập cho từng team/dự án trên nền dữ liệu dùng chung
- 🔐 **Bảo mật thống nhất** — OIDC/SSO, authorization Cedar, enforcement Ranger, masking theo tag, audit
- 🧊 **Iceberg REST Catalog** — nhúng sẵn, tích hợp engine, tenancy theo workspace (single source of truth)
- ⚙️ **Provision engine** — cấp phát & quản lý Trino / StarRocks / RisingWave trên Kubernetes
- 📊 **BI / Explorer** — SQL Lab, dataset, chart, dashboard (nhiều tab, filter, click behaviour), bám sát Metabase
- 📓 **Notebook** — notebook theo workspace với kernel Daft
- 🗄️ **Database management** — đăng ký & monitor database (Trino, StarRocks)
- 🔌 **Data API Builder** — expose HTTP API từ SQL (dynamic SQL kiểu MyBatis)
- ⚙️ **Workflow Automation** — tự động hoá kiểu n8n (schedule, gửi mail, gọi API)

---

## Kiến trúc

- **Backend** — modular monolith: Rust workspace ~27 crate, 1 binary (axum + tokio + sqlx).
- **Frontend** — 1 SPA (React + Vite + Tailwind v4 + Radix), 2 route-group: **Control plane**
  (governance/admin) và **Explorer** (làm việc theo workspace).
- **Hạ tầng** — PostgreSQL + Object storage (S3/MinIO) + Redis (tùy chọn); engine chạy trên Kubernetes.

📐 **Tài liệu kiến trúc đầy đủ:**
- 🇬🇧 **Bản gốc (canonical):** [`docs/en/architecture/2026-06-17-egret-architecture-design.md`](docs/en/architecture/2026-06-17-egret-architecture-design.md)
- 🇻🇳 **Bản tham khảo:** [`docs/vi/architecture/2026-06-17-egret-architecture-design.md`](docs/vi/architecture/2026-06-17-egret-architecture-design.md)

> Quy ước tài liệu: **tiếng Anh là bản gốc**, tiếng Việt là bản tham khảo; đặt trong `docs/en/` và
> `docs/vi/` (mirror cấu trúc).

---

## Trạng thái

**Nền tảng, governance và analytics đã dựng xong và đang dùng:** authentication/SSO, multi-workspace,
authorization Cedar thống nhất trên toàn bộ điểm enforcement, console RBAC & access-control,
enforcement Ranger ở data-plane với masking theo tag, và audit log đầy đủ. **BI/Explorer** bám sát
Metabase (dataset, chart, dashboard). **Iceberg catalog nhúng** đã chạy end-to-end (provision → auth →
data plane), **provision engine** (Trino/Stackable) đã ship, và **notebook** chạy cell thật end-to-end.
Đang làm tiếp: hoàn thiện catalog (storage profile, GC), delegation notebook/Daft, và mảng Data API +
Workflow.

Xem [Roadmap & trạng thái](docs/en/architecture/2026-06-17-egret-architecture-design.md#16-roadmap--status).

---

## Cấu trúc repo

```text
crates/      # ~27 crate Rust (egret-core, -server, -auth, -catalog-*, -policy, -engine-mgmt, -bi, -notebook, ...)
frontend/    # SPA React + Vite (control plane + explorer)
migrations/  # sqlx migrations
deploy/      # Dockerfile, compose, helm
docs/        # en/ (canonical) + vi/ (reference): architecture/, adr/, plans/
```

## License

TBD.
