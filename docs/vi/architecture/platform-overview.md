# Tổng quan nền tảng: data plane, governance và cô lập

Phần phân tích sâu được chuyển ra khỏi README của repository. Nội dung gồm cách các query engine đọc dữ liệu, cách federated catalog và credential hoạt động, vòng đời một query có governance, cô lập workspace và các lớp bảo mật. Bản tiếng Anh là bản gốc: [`docs/en/architecture/platform-overview.md`](../../en/architecture/platform-overview.md).

---

## Data plane — Trino & StarRocks đọc dữ liệu thế nào (ngoài S3)

Nền tảng dữ liệu của Egret là một **Iceberg lakehouse** — một **Iceberg REST catalog** dùng chung
([lakekeeper](https://github.com/lakekeeper/lakekeeper) nhúng) trên S3/RustFS. Lakehouse đó là **store
chính**: các bảng của chính Egret nằm ở đây, và đây là nơi cả hai engine đọc/ghi. Nhưng nền tảng
**không** bị giới hạn ở đó — nhờ **query federation**, cả **Trino** và **StarRocks** đều có thể đọc dữ
liệu *tại chỗ* từ các hệ thống ngoài (cơ sở dữ liệu quan hệ, các định dạng lakehouse khác, NoSQL,
stream, cloud warehouse) mà không cần copy vào.

### Sơ đồ tổng thể data plane

**Iceberg lakehouse trên S3/RustFS là store chính** (nguồn sự thật). Cả hai engine đọc nó qua đường
**đậm**; các hệ thống ngoài được query tại chỗ như federation **thứ cấp** (đường nét đứt). Chỉ vẽ các
nguồn tiêu biểu — danh sách connector đầy đủ nằm ở bảng bên dưới.

```mermaid
flowchart TB
    subgraph CP["Egret · tầng control + governance"]
        QR["egret-query router<br/>Cedar authz · delegation theo người dùng · Ranger"]
    end
    QR --> TRINO
    QR --> SR

    TRINO["Trino<br/>(provision trên k8s)"]
    SR["StarRocks<br/>(giao thức MySQL)"]

    subgraph LH["🧊 Lakehouse — nguồn sự thật · CHÍNH"]
        direction TB
        ICE["Iceberg REST catalog<br/>(lakekeeper nhúng · OAUTH2 vended creds)"]
        OBJ[("Object storage S3 / RustFS")]
        ICE --> OBJ
    end

    TRINO ==>|"store chính"| ICE
    SR ==>|"store chính"| ICE

    subgraph FED["Nguồn federated · thứ cấp — query ngay tại nơi dữ liệu nằm"]
        direction LR
        RDBMS[("RDBMS<br/>PostgreSQL · MySQL · Oracle · Snowflake …")]
        LAKE2[("Lakehouse / HMS khác<br/>Hive · Delta Lake · Hudi")]
        MISC[("NoSQL · stream · cloud<br/>MongoDB · Kafka · Elasticsearch · BigQuery …")]
    end

    TRINO -.->|"catalog Trino (egret quản lý)"| RDBMS
    TRINO -.-> LAKE2
    TRINO -.-> MISC
    SR -.->|"external catalog của StarRocks"| RDBMS
    SR -.-> LAKE2
```

- **Iceberg = chính.** Catalog nhúng (connector `iceberg`, type `rest`) được auto-wire lúc provision
  với **OAUTH2 vended credentials** — engine không bao giờ giữ khóa S3 dài hạn. Đây là nơi các bảng
  của chính Egret được ghi và đọc.
- **Cả hai engine đều federate.** **Trino** vươn tới hệ thống ngoài qua **catalog do egret quản lý**
  (luồng Add-catalog bên dưới); **StarRocks** mang theo **external catalog riêng** của nó (Hive,
  Iceberg, JDBC, …). Federation cho phép một query join dữ liệu lake với RDBMS/stream đang chạy **mà
  không cần copy vào**.

### Thêm một federated catalog — luồng credential

Mọi nguồn không phải warehouse được thêm qua form **"Add catalog" theo connector**. Egret lưu catalog
thành một bản ghi `EngineCatalog` (`kind` = `connector.name` của Trino), giữ **secret kết nối trong
vault**, và orchestrator render file `<tên>.properties` với các key có secret được inject từ env Secret
của pod dưới dạng `${ENV:VAR}` — **mật khẩu dạng plaintext không bao giờ nằm trong ConfigMap**.

```mermaid
sequenceDiagram
    actor Admin
    participant UI as "Add catalog (theo connector)"
    participant CP as "Egret control plane"
    participant VAULT as "Kho Secrets"
    participant ORCH as "Orchestrator"
    participant K8S as "Kubernetes"
    participant TRINO as "Trino coordinator"
    participant SRC as "Nguồn ngoài (vd PostgreSQL)"

    Admin->>UI: chọn connector (postgresql) + properties + password
    UI->>CP: POST /engines/id/catalogs (kind, properties, secretProperties)
    CP->>VAULT: lưu password → secret_ref
    CP->>ORCH: build CatalogSpec (name, properties, env vars)
    ORCH->>K8S: render name.properties ConfigMap + env Secret
    K8S->>TRINO: mount catalog + roll pod
    Note over TRINO: connector.name = postgresql<br/>connection-password = env:PW (từ Secret)
    TRINO->>SRC: kết nối JDBC (credential lấy từ env)
    SRC-->>TRINO: rows
```

### Connector được hỗ trợ (Trino, curated trong wizard)

Với engine **Trino**, Egret có sẵn một picker được curate, bám tài liệu, cho các connector phổ biến —
cộng thêm ô **nhập tên connector tùy ý** cho bất cứ thứ gì Trino hỗ trợ. Các nhóm dưới đây là ví dụ
tiêu biểu, không phải toàn bộ:

| Nhóm | Connector (ví dụ) |
|---|---|
| **Lakehouse / file** | **Iceberg** (metastore Hive · Glue · JDBC · REST · Nessie), Hive, Delta Lake, Hudi |
| **Quan hệ (JDBC)** | PostgreSQL, MySQL, MariaDB, SQL Server, Oracle, Redshift, Snowflake, ClickHouse, … |
| **NoSQL / search** | MongoDB, Cassandra, Elasticsearch, OpenSearch, Redis |
| **Streaming** | Kafka, Kinesis |
| **Cloud / API** | BigQuery, Google Sheets |
| **Observability / khác** | Prometheus, JMX, Memory, TPC-H, TPC-DS |

> **StarRocks** vươn tới cùng các loại nguồn qua **external catalog riêng** của nó (Iceberg, Hive,
> JDBC/MySQL/PostgreSQL, Delta, …); những catalog đó được cấu hình phía StarRocks, còn Egret query kết
> quả. **Object store tương thích S3** (AWS S3, RustFS, hoặc bất kỳ store S3-API nào qua `s3.endpoint`
> tùy chỉnh); các connector lakehouse có thể trỏ tới metastore/bucket riêng, độc lập với catalog nhúng
> của Egret.

### Vòng đời một query có governance

Federation không bao giờ đi vòng qua governance — mọi query đều được authorize, chạy **dưới danh tính
người gọi**, và được ghi audit:

```mermaid
flowchart TD
    U["Người dùng · SQL Lab / dataset / chart"] -->|"SELECT ..."| API["Egret /queries"]
    API --> PDP{"Cedar PDP<br/>được phép?"}
    PDP -->|từ chối| DENY["403 + audit"]
    PDP -->|cho phép| DELEG["Delegation theo người dùng<br/>impersonate · jwt (TLS)<br/>— Trino VÀ StarRocks"]
    DELEG --> CONN["connector egret-query"]
    CONN -->|"Trino · HTTP, chạy dưới danh tính người gọi"| TRINO["Trino"]
    CONN -->|"StarRocks · MySQL wire, chạy dưới danh tính người gọi"| SR["StarRocks"]
    TRINO --> RANGER{"Enforcement Ranger<br/>masking · row-filter<br/>(plugin Trino + StarRocks)"}
    SR --> RANGER
    RANGER --> FED[("Iceberg lakehouse (chính)<br/>+ nguồn federated")]
    RANGER --> RES["Rows đã governance"] --> U
    DELEG -.-> AUDIT[("Audit log")]
```

- **Delegation theo người dùng (cả hai engine)** — query chạy dưới danh tính của **người gọi**, không
  phải một service account dùng chung. **Cả Trino và StarRocks** đều hỗ trợ `impersonate` (delegator
  tin cậy qua TLS) và `jwt` (JWT ngắn hạn của chính người gọi qua TLS, người dùng OIDC). Mỗi engine
  sau đó enforce policy của **chính người dùng đó** qua **plugin Ranger** của nó (column masking,
  row-level filter) — Egret phục vụ policy-download tương thích Ranger cho cả hai.

---

## Cô lập đa workspace

Một **workspace** là một sandbox độc lập cho một team hoặc dự án. Dashboard, dataset, chart, notebook,
saved query, database connection và định nghĩa API sống *bên trong* một workspace và không bao giờ rò
sang ranh giới khác. **Catalog** và **engine** dùng chung nằm bên dưới, nên việc cô lập không đồng
nghĩa với nhân bản dữ liệu.

```mermaid
flowchart TB
    subgraph WA["Workspace · Phân tích"]
        A1["Dashboard · Notebook"]
        A2["Saved query · Thành viên / role"]
    end
    subgraph WF["Workspace · Tài chính"]
        F1["Dashboard · Notebook"]
        F2["Saved query · Thành viên / role"]
    end
    WA <-.->|"không rò"| WF
    WA -->|"đọc/ghi có governance"| SHARED
    WF -->|"đọc/ghi có governance"| SHARED
    SHARED["Catalog + Engine dùng chung<br/>(single source of truth — không bản sao)"]
```

- **Cô lập:** nội dung ở Workspace A vô hình với Workspace B.
- **Chung bên dưới:** cả hai đọc từ một catalog — không bản sao, không lệch (drift).
- **Access theo phạm vi:** mỗi workspace có thành viên, role và grant riêng.

---

## Security-first theo thiết kế

Access không bao giờ là chuyện làm sau. Mọi request đều đi qua các lớp kiểm soát trước khi chạm dữ
liệu — và mọi quyết định đều được ghi lại.

```mermaid
flowchart TB
    REQ["Request của người dùng"] --> L1
    L1["1 · Authentication — OIDC / SSO (Keycloak), group từ claims"] --> L2
    L2["2 · Authorization — Cedar PDP · role, grant, phân cấp, explicit deny (deny thắng)"] --> L3
    L3["3 · Phạm vi workspace — object thuộc workspace của người gọi"] --> L4
    L4["4 · Enforcement ở data plane — Ranger · delegation theo người dùng · masking theo tag / row-filter"] --> L5
    L5["5 · Audit log — ghi lại mọi quyết định"] --> OK["✅ Truy cập dữ liệu có governance"]
```

- **Một authorizer duy nhất** — Cedar là điểm quyết định policy duy nhất (template-per-role + linked
  policy-per-grant, deny thắng).
- **OIDC / SSO** — danh tính doanh nghiệp qua Keycloak; group lấy từ token claims.
- **Governance ở data plane** — tải policy kiểu Ranger cho Trino/StarRocks, query delegated theo người
  dùng, **column masking** và **row-level filter** theo tag.
- **Phân loại & ABAC** — tag phân loại dữ liệu gác quyền truy cập; luật theo thuộc tính/độ-mật (Cedar)
  cho các object BI.
- **Có thể audit** — audit log thật ghi lại ai truy cập gì, khi nào, và có được phép hay không.

### Danh tính, secret & cô lập

Security-first còn nghĩa là **tự sở hữu danh tính đầu-cuối** và **không bao giờ tin vào credential dùng chung**:

- **Token broker Machine-IdP** — Egret tự cấp **token engine ngắn hạn theo từng người dùng** từ
  identity provider nhúng của chính nó (`aud: engine`, ký bằng JWKS của Egret). Engine tin thẳng issuer
  của Egret, nên **không có mật khẩu engine dài hạn**, và ngay cả **người dùng local / non-OIDC** cũng
  có được quyền truy cập được scope và delegated. SSO chỉ còn làm authentication + JIT provisioning.
- **SSO back-channel logout** — một logout token OIDC từ Keycloak (RS256, xác thực bằng JWKS) thu hồi
  **toàn bộ** session của người dùng đó ở phía server, nên đăng xuất tập trung thực sự chấm dứt truy
  cập ở mọi nơi.
- **Vòng đời session & tài khoản** — admin reset password, khóa tức thì bằng **thu hồi session**,
  hard-delete kèm **gán lại nội dung** (không để dashboard mồ côi), và **last-admin guard** ngăn nền
  tảng tự khóa chính mình ra ngoài.
- **Vault secret có kiểu** — secret đa-trường, có kiểu theo phong cách Kubernetes (`Opaque` /
  `kubernetes.io/tls` / `basic-auth`) đứng sau mọi credential của connector, storage và TLS. Secret
  được inject vào pod dưới dạng env reference — **plaintext không bao giờ nằm trong ConfigMap** — và
  TLS ở ingress được phục vụ từ chứng chỉ do operator cung cấp.
- **Cô lập runtime** — mỗi notebook chạy trong một **pod được gia cố, riêng theo người-dùng × workspace**
  (non-root `uid 1000`, drop toàn bộ Linux capability, `NetworkPolicy` siết chặt, RBAC tối thiểu) và
  ghi vào một **namespace Iceberg riêng tư** qua token scoped theo namespace — không bao giờ dùng kernel
  chung.

---
