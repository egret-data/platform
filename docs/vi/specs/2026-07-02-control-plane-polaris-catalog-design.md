# Catalog Polaris ở Control Plane — Thiết kế (bản tham chiếu)

> Bản chuẩn: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. File này rút gọn.
> **Thay thế** `docs/en/specs/2026-07-01-rest-catalog-redesign-design.md` (hướng embedded-lakekeeper — bỏ). Plan 1/4 của redesign đó vô hiệu; giữ helper `write_table` (ghi-qua-engine).
> Ngày 2026-07-02 · Trạng thái: đã duyệt (brainstorming).

## Mục tiêu
Xoá toàn bộ catalog hiện tại (embedded lakekeeper) và re-platform catalog Iceberg lên **Apache Polaris** — chạy như **service ngoài** mà egret **kết nối + drive qua management API** (mô hình Engines). Chuyển **Catalog + Storage + Secrets(storage) lên control plane** (chỉ `platform_admin`); workspace chỉ *tiêu thụ* catalog được gán. Xây UI control-plane quản Polaris (connection, catalog, storage, RBAC đầy đủ) + gán workspace. **Notebook tạm dừng.**

## Quyết định (chốt)
1. **Xoá sạch catalog cũ** (greenfield, không migrate).
2. **Catalog + Storage + Secrets(storage) → control plane**, chỉ `platform_admin`. Workspace tiêu thụ qua assignment; ws_admin không quản mấy cái này.
3. egret **kết nối MỘT Polaris global** + drive **management API** (không deploy/không bootstrap; dev chạy Polaris qua docker-compose).
4. v1 quản: catalog + storage + gán-workspace + service credential engine **và RBAC đầy đủ** (principals/principal-roles/catalog-roles/grants tới table-level) — proxy sang Polaris.
5. Polaris **auth MIXED** (tự cấp service token + nhận Keycloak OIDC).
6. **Notebook tạm dừng** — gỡ phụ thuộc catalog của notebook.

## Bối cảnh (bị xoá)
lakekeeper nhúng `/catalog/v1` + `front_auth` + `egret-catalog` (token/warehouse) + `egret-iceberg-ext` + browse + `provision_notebook_access`. Storage/Secrets đang ở **ws_admin**. Trang control-plane Engines là khung chưa-backend — Polaris theo cùng mẫu registry. (So sánh Polaris vs lakekeeper: xem memory `egret-catalog-single-catalog-pivot`. Polaris tự cấp token + MIXED + multi-catalog (Catalog≈warehouse) + RBAC table-level; là service JVM **không nhúng được** → "connect, don't embed".)

## Kiến trúc
Control plane sở hữu hạ tầng; workspace tiêu thụ.
- **Polaris** = catalog Iceberg REST + management API + RBAC + token issuer. egret đăng ký **một** connection.
- **egret backend** = client/proxy mỏng tới management API Polaris (crate mới `egret-polaris`). **Polaris là nguồn-sự-thật** cho catalog/principal/role/grant; egret KHÔNG mirror. egret chỉ giữ trong DB: **connection registration** + **mapping workspace↔catalog assignment**.
- Map: workspace egret → tiêu thụ Polaris **Catalog**; Catalog ≈ warehouse cũ; mỗi Catalog trỏ một **storage connection** ở control plane.
- **Auth MIXED:** egret-admin → Polaris bằng admin credential (client_credentials → token); engine → Polaris bằng **service credential** (principal/token egret cấp per-assignment); Keycloak OIDC cũng nhận (per-user tương lai).
- Governance column/row vẫn ở **engine (Ranger)**; RBAC Polaris ở mức catalog/namespace/table.

## Components
- **A. Xoá (greenfield):** crate `egret-catalog-engine`/`egret-catalog`/`egret-iceberg-ext`; bỏ dep+patch lakekeeper; bỏ `catalog_engine`/`catalog_browse` khỏi AppState. DB: drop schema `lakekeeper` + `catalog_tokens` + `workspace_warehouses`. Frontend: bỏ trang catalog/storage/secrets ở workspace. Kernel: bỏ `iceberg`/`write_iceberg`/`catalog`/`RuntimeCatalog` + `provision_notebook_access`; giữ `trino`/`starrocks`/`write_table`.
- **B. `egret-polaris` (crate mới):** client typed tới management API + OAuth2 token (client_credentials, cache/refresh); ops: connection health; catalog CRUD (+StorageConfigInfo); principal/principal-role/catalog-role/grant; `PolarisError`; seam HTTP để mock test.
- **C. Control-plane routes (`platform_admin`):** `polaris/connection` (đăng ký 1 connection + health); `polaris/catalogs` (CRUD, trỏ storage); `polaris/{principals,principal-roles,catalog-roles,grants}` (RBAC proxy); `storage` (global); `secrets` (global); `workspaces/{ws}/catalog-assignments` (gán + cấp service credential); read `ws/{ws}/catalogs` (consume-only).
- **D. DB:** drop cũ; thêm `polaris_connection` (singleton: url+admin_secret+health), `storage_connections` (global), `secrets` control-plane (global), `workspace_catalog_assignments` (ws↔catalog+service_credential_secret).
- **E. Frontend:** trang control-plane thật: **Polaris** (connection + bảng catalog + RBAC), **Storage** (global), **Secrets** (global) — `RequireGlobalAdmin`; workspace có view "Catalogs" read-only. Bỏ route storage/secrets/catalog ở workspace.
- **F. Engine integration:** khi gán catalog cho workspace, egret cấp **service credential** Polaris cho engine của workspace (Iceberg REST connector → Polaris catalog URL + token; S3/MinIO qua storage config). Chi tiết wiring connector = ops/engine-registry, **document**; egret cấp credential.

## Data flow
1. Admin → UI control-plane → egret backend → `egret-polaris` → Polaris mgmt API (+ egret DB cho storage/secrets/assignment).
2. Engine → Polaris catalog bằng service credential → đọc/ghi Iceberg trên storage.
3. User → engine (Ranger). Không chạm Polaris.
4. Notebook (paused): đọc `trino()`/`starrocks()`, ghi `write_table` — không phụ thuộc catalog.

## Test
`egret-polaris` (mock HTTP Polaris: CRUD + token + error); control-plane authz (chỉ platform_admin) + assignment logic; storage/secrets re-scope global; build workspace xanh sau khi xoá; dev e2e opt-in: docker-compose chạy **Polaris** (thay lakekeeper nhúng) → đăng ký connection → tạo catalog trên MinIO → gán workspace → engine đọc/ghi.

## Ngoài phạm vi
Notebook (paused, chỉ gỡ phụ thuộc catalog); centralization Engines (trajectory song song); governance column/row (engine+Ranger); deploy/ops Polaris; migrate (greenfield); multi-connection/realms/federation (v1 một connection).

## Future direction — K8s provisioning (epic riêng, KHÔNG thuộc epic này)

North star của egret: control plane **K8s-native** không chỉ *quản* mà còn *provision* service nền — tạo engine (Trino/StarRocks/RisingWave) + catalog (Polaris) trên K8s qua operator/Helm, rồi quản qua chính control plane này.

- **Hai nửa:** epic này xây nửa **Manage** (registry + drive management API). K8s provisioning là nửa **Provision** chồng lên sau — không thay thế.
- **Giữ abstraction "connection" sạch cho `Provisioner`.** Hôm nay `platform_admin` **đăng ký tay** connection tới service ops deploy. Tương lai `Provisioner` (apply CRD operator, hoặc Helm release) sẽ **tạo service + auto-register cùng connection row** → routes register/manage ở đây tái dùng nguyên. Đừng ràng buộc logic quản trị vào "ai deploy service".
- **Provision từng service (future, brainstorm riêng):** StarRocks + RisingWave có **operator** chính thức; Trino qua **Helm chart** chính thức (hoặc Stackable operator); **Polaris chưa có operator** → deploy bằng **Helm chart in-repo** (`polaris/helm/polaris`) hoặc manifest, rồi quản qua `egret-polaris` (P3/P4). Polaris khớp mẫu "Helm-deploy + management-API-manage".
- **Brainstorm riêng khi tới:** K8s client trong egret, provisioner per-service (CRD vs Helm), theo dõi deploy/health, teardown, inject secret.

## Rủi ro
- Thêm phụ thuộc **JVM ngoài** (Polaris) — dev chạy compose, prod ops deploy.
- egret giữ **admin credential Polaris** — lưu ở Secrets control-plane, không log, least-privilege.
- **Polaris là nguồn-sự-thật** — xử lý drift/lỗi (catalog bị xoá ngoài); UI phản ánh state Polaris, không mirror.
- **Wiring engine↔catalog credential** là chỗ chưa rõ nhất — chốt lúc plan.
- **Xoá lớn** — verify không gì ngoài notebook/catalog phụ thuộc `egret-catalog*` trước khi xoá (codegraph_impact).
- **Decompose thành plan:** (P1) xoá cũ + chuyển Storage/Secrets lên control plane; (P2) client `egret-polaris`; (P3) routes catalog+RBAC+assignment; (P4) UI control-plane; (P5) dev Polaris compose + e2e + engine wiring.
