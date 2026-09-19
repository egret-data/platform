# SP3 — Catalog registry

**Trạng thái:** thiết kế, chờ review
**Chương trình:** `docs/{en,vi}/specs/2026-09-15-control-plane-dg-review-programme.md`
**Tiếp theo:** SP4 (một mô hình quyền dữ liệu egret duy nhất trên các catalog registry), SP5 (các engine do egret thực thi)
**Triển khai:** ba PR — SP3a backend, SP3b StarRocks, SP3c frontend

> Bản tiếng Anh là bản chuẩn:
> `docs/en/specs/2026-09-19-catalog-registry-design.md`.

## 1. Vấn đề

Hiện tại một catalog là một row `engine_mgmt.engine_catalogs` thuộc sở hữu của một engine. Cùng
một kết nối Postgres được hai engine dùng thì phải cấu hình hai lần, mỗi lần với secret riêng; row
này lưu `.properties` thô của Trino, thứ mà bản thân egret không đọc hiểu được; StarRocks bỏ qua
mọi row trừ warehouse chính; và warehouse thuộc về đúng một workspace
(`catalog.warehouses.workspace_id NOT NULL`) trong khi engine lại là platform-wide. Người dùng yêu
cầu một mục **Catalogs** dưới Infrastructure, giữ các định nghĩa kết nối cho mọi loại catalog, gộp
cả Warehouses vào, và là nơi mà việc provisioning engine sẽ gắn (attach) vào.

## 2. Các quyết định (chốt cùng người dùng, 2026-09-19)

| # | Quyết định |
|---|---|
| D1 | Một catalog là **tài nguyên platform** (Control Plane, giống storage và engine). Platform admin tạo nó, gắn (attach) nó vào engine, và **gán (assign) nó cho workspace**. |
| D2 | Cấu hình được **typed theo từng kind**, cộng với **advanced properties tự do theo từng engine**. Kind `generic` mang các property thô cho các connector mà egret không mô hình hoá. |
| D3 | v1 typed bốn family (§4.2): Iceberg (gồm cả egret warehouse), JDBC, Hive/Delta/Hudi, Elasticsearch + TPC-H/TPC-DS. Mọi thứ còn lại đi qua `generic`, cho **cả hai** engine. |
| D4 | Sửa một catalog đang được engine dùng sẽ **apply ngay lập tức, sau một xác nhận** liệt kê các engine bị ảnh hưởng. |
| D5 | **Tên registry chính là tên catalog trong SQL**, giống hệt nhau trên mọi engine. SP4 sinh ra một Ranger policy cho mỗi catalog, nên một catalog không được mang tên khác nhau trên các engine khác nhau. |

## 3. Những gì cả hai engine đều đọc được

Đã xác nhận trong mã nguồn vendor (`trino/plugin`, `trino/docs/src/main/sphinx/connector`;
StarRocks `fe/fe-core/.../connector/ConnectorType.java`, `docs/en/data_source/catalog`).

| Nhóm | Kind | Trino | StarRocks |
|---|---|---|---|
| Lakehouse | Iceberg (REST, Hive metastore, Glue) | `iceberg` | `iceberg` (`iceberg.catalog.type` = rest/hive/glue) |
| | Hive, Delta Lake, Hudi | `hive`, `delta_lake`, `hudi` | `hive`, `deltalake`, `hudi` |
| JDBC | PostgreSQL, MySQL, MariaDB | `postgresql`, `mysql`, `mariadb` | `jdbc` (MySQL/PostgreSQL từ 3.0) |
| | Oracle, SQL Server | `oracle`, `sqlserver` | `jdbc` (từ 3.2.9 / 3.3.1) |
| | ClickHouse | `clickhouse` | `jdbc` (thử nghiệm từ 3.3.0) |
| Search | Elasticsearch | `elasticsearch` | `es` |
| Sample | TPC-H, TPC-DS | `tpch`, `tpcds` | `benchmark` (còn có SSB) |

Chỉ Trino: BigQuery, Snowflake, Redshift, SingleStore, Cassandra, MongoDB, Redis, Kafka, Druid,
Pinot, Prometheus, Loki, Google Sheets, Exasol, DuckDB, Ignite, OpenSearch, Lakehouse, JMX, Memory,
Blackhole, Faker, Thrift. Chỉ StarRocks: Paimon, Kudu, MaxCompute (ODPS), Fluss, Lance, Unified.
Các connector này chỉ tiếp cận được qua `generic`.

## 4. Thiết kế

### 4.1 Data model (schema mới `registry`)

**`registry.catalogs`**

| Cột | Kiểu | Ghi chú |
|---|---|---|
| `id` | uuid PK | |
| `name` | text UNIQUE NOT NULL | Tên SQL trên mọi engine. CHECK `name ~ '^[a-z][a-z0-9_]{0,62}$'`. Không đổi được sau khi tạo. |
| `kind` | text NOT NULL | CHECK theo các kind ở §4.2. |
| `config` | jsonb NOT NULL | Typed theo từng kind, được một Rust enum validate ở mỗi lần ghi. Không bao giờ chứa secret dạng plaintext — chỉ chứa secret id và storage-connection id. |
| `advanced` | jsonb NOT NULL DEFAULT `{}` | `{"trino": {k: v}, "starrocks": {k: v}}`, được render nguyên văn sau các key đã typed. |
| `description` | text | |
| `created_at`, `updated_at` | timestamptz | |

**`registry.engine_catalog_attachments`** — `(engine_id → engine_mgmt.engines ON DELETE CASCADE,
catalog_id → registry.catalogs ON DELETE RESTRICT, PRIMARY KEY both)`, cộng thêm
`catalog_client_id uuid NULL` (chỉ cho iceberg-egret: catalog client theo từng engine mà egret
mint, như hiện tại), `apply_status text NOT NULL DEFAULT 'applied'` (`applied` | `failed`),
`apply_error text NULL`, `applied_at timestamptz NULL`.

**`registry.catalog_workspaces`** — `(catalog_id → registry.catalogs ON DELETE RESTRICT,
workspace_id → workspaces ON DELETE CASCADE, PRIMARY KEY both)`.

**Warehouse vẫn là nơi lưu trữ backing của một catalog iceberg-egret.** `config` của catalog nêu
tên `warehouse_id`; lakekeeper, `catalog.catalog_roles`, `catalog_grants`, `catalog_clients`,
`catalog_tag_assignments` và `analytics.notebook_warehouse_settings` vẫn khoá theo warehouse và
không bị đụng tới. `catalog.warehouses.workspace_id` bị bỏ; liên kết workspace trở thành một row
`catalog_workspaces`. Tên warehouse lakekeeper độc lập với tên registry và không bao giờ đổi.

**`engine_mgmt.engines.warehouse_id` trở thành nullable** — một engine không còn cần warehouse nữa
— và bị xoá khi không còn gì đọc nó nữa (SP3c).

### 4.2 Các kind và cấu hình typed của chúng

| `kind` | Các trường `config` |
|---|---|
| `iceberg` | `metastore`: `egret` {`warehouse_id`} \| `rest` {`uri`, `warehouse`, `auth`: none \| oauth2 {`credential_secret_id`, `scope?`, `token_uri?`}} \| `hive` {`uri`} \| `glue` {`region`, `access_key?`, `secret_key_secret_id?`}; `storage_connection_id` (bắt buộc trừ khi `metastore = egret`, khi đó dùng storage connection của warehouse) |
| `hive`, `delta_lake`, `hudi` | `metastore`: `hive` {`uri`} \| `glue` {…như trên}; `storage_connection_id` |
| `jdbc` | `dialect`: postgresql \| mysql \| mariadb \| oracle \| sqlserver \| clickhouse; `host`, `port`, `database?`, `user`, `password_secret_id`, `ssl`: disable \| require \| verify_full {`ca_secret_id?`} |
| `elasticsearch` | `hosts[]`, `port`, `tls`, `user?`, `password_secret_id?` |
| `tpch`, `tpcds` | `scale?` (`tpch.splits-per-node` của Trino v.v. nằm trong `advanced`) |
| `generic` | `engines`: tập con của {trino, starrocks}; tên connector và mọi property nằm trong `advanced.<engine>` |

Một kind mà engine không đọc được sẽ bị từ chối khi attach (ví dụ: `generic` chỉ có `trino` gắn
vào một engine StarRocks; `tpch` trên StarRocks được render thành `benchmark`).

### 4.3 Rendering

Một hàm thuần cho mỗi target, mỗi hàm được unit-test theo từng kind so với expected output viết
tay:

- `render_trino(catalog, attachment) -> TrinoCatalogFile` — `connector.name` + property + các env
  entry cho secret (`${ENV:CAT_<n>_<i>}`, được resolve từ egret secrets tại thời điểm apply, như
  hiện nay). iceberg-egret render REST catalog với client credential mà attachment đã mint.
- `render_starrocks(catalog, attachment) -> StarRocksCatalogDdl` — `CREATE EXTERNAL CATALOG`,
  `ALTER CATALOG <name> SET (...)` (StarRocks rebuild lại connector tại chỗ:
  `CatalogMgr.alterCatalog`) và `DROP CATALOG`. JDBC render `jdbc_uri`, `driver_class`,
  `driver_url` (egret cung cấp sẵn một driver URL cố định (pinned) cho từng dialect trong config),
  `schema_resolver`.
- SP5 thêm một target thứ ba: kết nối riêng của egret.

Các key `advanced.<engine>` được nối vào sau các key đã typed; một key trùng với key đã typed sẽ
bị từ chối tại thời điểm lưu (để advanced không thể âm thầm ghi đè lên những gì form hiển thị).

### 4.4 Attach và detach

`POST/DELETE /api/control-plane/engines/{id}/catalogs/{catalog_id}`:

- **Trino**: thêm/xoá attachment, rebuild request provision qua path
  `build_provision_req_with` → `apply_built` hiện có (apply trước, commit sau; 502 nếu apply bị từ
  chối, không gì được persist). Trino roll lại pod của nó, như với mọi thay đổi catalog hiện nay.
- **StarRocks**: chạy `CREATE EXTERNAL CATALOG` / `DROP CATALOG` qua cùng kênh DDL mà bootstrap
  dùng; row attachment chỉ được ghi sau khi DDL thành công.
- Bootstrap StarRocks (`spawn_starrocks_catalog_wire`) tạo **mọi** catalog đã attach một khi FE sẵn
  sàng, không chỉ warehouse chính, và vẫn giữ các checkpoint của nó (SP1 Part B).
- Một attachment iceberg-egret sẽ mint một catalog client cho engine đó; detach sẽ thu hồi
  (revoke) client này nếu do egret mint.

### 4.5 Sửa một catalog đang được engine dùng

1. `GET /api/control-plane/catalogs/{id}/impact` trả về các engine đã attach cùng với việc apply
   sẽ làm gì trên từng engine (`restart` với Trino, `in_place` với StarRocks).
2. UI hiển thị danh sách đó và hỏi xác nhận.
3. `PUT /api/control-plane/catalogs/{id}` validate và lưu cấu hình mới — registry là source of
   truth — rồi apply lần lượt cho từng engine đã attach (Trino: `apply_built` Update; StarRocks:
   `ALTER CATALOG SET`).
4. Response liệt kê kết quả theo từng engine. Một engine thất bại sẽ có `apply_status = failed`
   và `apply_error` trên attachment của nó, được đánh dấu (flag) trong UI, và có một hành động
   **Retry** (`POST …/engines/{id}/catalogs/{catalog_id}/apply`). Thất bại không revert lại
   catalog, vì các engine khác đã giữ cấu hình mới rồi.

Ngoài ra:

- **Không đổi tên.** `name` chính là định danh (identity) SQL.
- **Delete** bị từ chối với 409 kèm danh sách engine và workspace, chừng nào còn tồn tại
  attachment hoặc assignment nào (giống hình dạng với việc xoá engine trong SP1 Part A).
- **Secret không được push khi rotate.** Rotate một secret không làm thay đổi bất kỳ catalog nào;
  trang Secrets liệt kê các catalog tham chiếu tới một secret, và một catalog có nút **Re-apply to
  engines**, chạy lại bước 3 với cấu hình không đổi.
- **Quy tắc thường trực từ SP1 Part B vẫn giữ nguyên:** egret không bao giờ xoá một secret mà một
  engine có thể vẫn còn giữ.

### 4.6 Gán (assignment) workspace

`GET/PUT /api/control-plane/catalogs/{id}/workspaces`. Trong SP3, một assignment quyết định
workspace nào thấy được catalog nào khi duyệt dữ liệu (data browsing) và khi chọn warehouse cho
notebook (chỉ iceberg-egret). Nó **chưa** giới hạn những gì một engine trả về — đó là phần
enforcement, thuộc SP4 (Ranger profile) và SP5 (các engine do egret thực thi).

### 4.7 Permissions

Các platform permission mới `catalog_registry:read`, `catalog_registry:create`,
`catalog_registry:update`, `catalog_registry:delete` (assign-to-workspace, re-apply và Retry dùng
`catalog_registry:update`). Migration cấp cho mỗi platform role permission `catalog_registry:*`
tương ứng với mỗi `warehouse:*` mà role đó đang có (nên `data_steward` có thêm
`catalog_registry:read`).

Các key này cố ý không phải `catalog:*`: `catalog:read` đã tồn tại như một permission ở phạm vi
workspace (migration 0024, mọi workspace role đều có, được workspace engine view kiểm tra), và key
quyền là chuỗi không phân biệt phạm vi trong `rbac::is_known`, trong danh sách quyền hiệu lực mà
frontend đọc, trong UI quản trị quyền và trong action Cedar — một key mang hai nghĩa sẽ gây nhập
nhằng ở tất cả những chỗ đó. (Nó **không** mở rộng quyền: grant platform của role không phải admin
bị ràng buộc `resource == Platform::"root"` nên không bao giờ thoả một check phạm vi workspace — bản
nháp trước của mục này khẳng định ngược lại, và điều đó sai.) `catalog:read` của workspace giữ nguyên. `warehouse:*` vẫn còn cho tới khi SP3c bỏ các trang Warehouses, sau đó mới bị xoá.

### 4.8 Migration dữ liệu hiện có

Một migration cộng với một backfill Rust chạy lúc boot (backfill cần bộ parser đã typed), ghi
report ra log và vào một bảng `registry.migration_report` mà trang Catalogs hiển thị một lần.

1. Mỗi warehouse → một catalog `iceberg` với `metastore = egret`. Tên: tên warehouse được chuẩn
   hoá (normalise) về dạng `^[a-z][a-z0-9_]*$` (chữ thường, ký tự khác → `_`, chữ số đứng đầu được
   thêm tiền tố `w_`), gắn hậu tố `_2`, `_3` … khi trùng nhau. `workspace_id` cũ của nó trở thành
   một row `catalog_workspaces`. Trên cụm kind hiện nay: 24 warehouse trong một workspace, tất cả
   đều khác nhau, không cái nào là một định danh (identifier) SQL hợp lệ — cả 24 đều được đổi tên
   (ví dụ `e2e-wh-1784519491585` → `e2e_wh_1784519491585`).
2. Row `egret-warehouse` chính của mỗi engine → một attachment tới catalog của warehouse đó, kèm
   theo catalog client được mang sang. **Ngoại lệ, để giữ cho SQL đã lưu vẫn chạy được:** nếu chỉ
   đúng một warehouse được attach làm primary của engine trên toàn platform, catalog của nó được
   đặt tên `egret` thay vì tên đã chuẩn hoá. Nếu có nhiều hơn một, chúng giữ tên đã chuẩn hoá và
   report liệt kê mọi object SQL đã lưu (dataset, chart, Data API endpoint, alert, saved query) có
   văn bản tham chiếu tới `egret.`.
3. Mọi row `engine_catalogs` còn lại → một catalog: một kind được nhận diện mà property parse được
   thì trở thành typed; mọi thứ khác trở thành `generic` với property được giữ nguyên. Hai engine
   có catalog cùng tên và cấu hình giống hệt nhau thì dùng chung một catalog; cùng tên nhưng cấu
   hình khác nhau thì được thêm hậu tố `_<engine>`, và được ghi vào report.
4. Ranger tag export (`egret-policy/src/store.rs:221-264`) resolve tên catalog qua attachment thay
   vì giả định catalog chính tên là `egret`.
5. `engine_mgmt.engine_catalogs` bị xoá trong SP3c, sau khi các endpoint engine-catalog cũ bị gỡ
   bỏ.

## 5. Giao diện người dùng (SP3c)

- **Control Plane → Infrastructure → Catalogs** thay thế Warehouses; `/control-plane/warehouses/*`
  redirect sang catalog tương ứng.
- **List**: name, kind (icon), engine đã attach, workspace đã assign, một cảnh báo khi có
  attachment nào apply thất bại.
- **Create**: chọn một kind → form đã typed → tab **Advanced** theo từng engine. Được dựng từ các
  primitive config hiện có (`ConfigSection`, `FieldLabel`, secret picker, storage-connection
  picker).
- **Detail tabs**: Configuration; Engines (attach/detach, apply status, Retry); Workspaces
  (assign); với iceberg-egret, các tab warehouse hiện có chuyển về đây (namespace browser, catalog
  roles, grants, clients, tags).
- **Engine detail → Catalogs** chọn từ registry thay vì dùng form inline.
- **Create-engine wizard**: chọn catalog để attach thay vì chọn warehouse.
- **Workspace detail (Control Plane)**: các catalog đã assign.
- **Test** trên một catalog: `SHOW SCHEMAS FROM <name>` qua một engine đã attach nếu có; nếu không
  thì kiểm tra khả năng kết nối (reachability) TCP tới host/port đã cấu hình. Các kiểm tra đầy đủ
  ở mức driver sẽ đến cùng SP5.

## 6. Delivery

| PR | Phạm vi | Vẫn hoạt động |
|---|---|---|
| SP3a | Schema, typed config enum + validation, Trino rendering, migration + backfill + report, registry và attachment API, Trino attach/detach và edit propagation, permissions, client được regenerate | Các endpoint engine-catalog hiện có được cài lại (reimplement) trên nền registry, nên UI hiện nay vẫn hoạt động |
| SP3b | StarRocks rendering (create/alter/drop), attach/detach, propagation, bootstrap được tổng quát hoá | — |
| SP3c | Mục Catalogs và các form cho bốn family, tab attach engine, workspace assignment, wizard, redirect; gỡ bỏ các endpoint engine-catalog cũ, `engine_catalogs`, `engines.warehouse_id` và `warehouse:*` | — |

## 7. Testing

- **Rendering**: theo từng kind × từng engine, so với expected output viết tay, gồm mọi dialect
  JDBC, từng iceberg metastore, việc merge `advanced` và việc từ chối khi trùng key.
- **Validation**: mỗi kind từ chối trường bắt buộc bị thiếu, trường không rõ, secret dạng
  plaintext, và tên nằm ngoài quy tắc identifier.
- **Migration**: dữ liệu seed bao phủ việc chuẩn hoá và thêm hậu tố tên warehouse, ngoại lệ
  `egret` khi chỉ có một primary, report khi có nhiều primary, việc phân loại typed vs generic, và
  việc dedupe cùng tên vs thêm hậu tố; các row report được assert, không chỉ riêng các catalog.
- **Routes** (DB-backed): attach/detach theo từng loại engine; edit propagation khi một engine
  thất bại (engine kia đã apply, engine thất bại được flag, Retry thành công); delete bị từ chối
  khi còn attach hoặc assign; permission gate với PDP thật.
- **Live trên kind** (thủ công, báo cáo đúng như vậy): attach một catalog PostgreSQL vào một
  engine Trino và query nó; attach vào một engine StarRocks và query nó; sửa password và quan sát
  cả hai engine nhận thay đổi; xác nhận 24 catalog warehouse đã migrate và report của chúng.

## 8. Ngoài phạm vi

- Enforce workspace assignment bên trong engine (SP4, SP5).
- egret query trực tiếp một catalog (SP5).
- Catalog theo từng workspace (per-workspace).
- Đổi tên catalog và lịch sử cấu hình theo version.
