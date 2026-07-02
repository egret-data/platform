# Control-Plane Polaris — Plan 4: routes catalog + RBAC + assignment (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-polaris-p4-catalog-rbac-assignment-routes.md` (code/step đầy đủ).
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P4/6.** Phụ thuộc **P2** (crate `egret-controlplane` + gate `platform_admin` + store storage/secrets global) + **P3** (`egret-polaris`). Thực thi qua subagent-driven-development.

**Mục tiêu:** routes control-plane (gate `platform_admin`): đăng ký connection Polaris, quản catalog + RBAC đầy đủ (principal/principal-role/catalog-role/grant) bằng proxy `egret-polaris`, và **gán catalog cho workspace** (cấp service credential Polaris-native/assignment) + route đọc consume-only cho workspace.

**Kiến trúc:** mở rộng crate `egret-controlplane`. Singleton `polaris_connection` (base URL + config admin-token Keycloak) → dựng `PolarisClient`. Catalog/RBAC routes proxy mỏng. Assignment cấp per (ws,catalog): principal Polaris (→clientId/secret lưu Secret global) + principal-role + catalog-role + grant; ghi `workspace_catalog_assignments`. **v1 engine credential = Polaris-native principal** (engine dùng `/v1/oauth/tokens` deprecated-nhưng-chạy; migrate sau).

## Bối cảnh
- P2: `egret-controlplane` + `require_platform_admin` + `CpState` + StorageConn global + secrets scope `Option<Uuid>` (None=global); nest `/api/control-plane` sau CSRF.
- P3: `egret_polaris::{PolarisClient,PolarisConfig,PolarisError,NewCatalog,CatalogType,StorageConfig,PrincipalWithCredentials,Grant}`; token Keycloak client-credentials.
- Auth: control-plane dùng `Auth(principal)` + `require_platform_admin`; workspace read dùng `WsContext`.
- Migration tiếp **0050**; `touch metastore`.

## Task
- **T1 (migration 0050):** `polaris_connection` (singleton: base_url, kc_token_url, kc_client_id, kc_secret_ref, kc_scope) + `workspace_catalog_assignments` (ws, catalog_name, principal_name, credential_ref→workspace_secrets, UNIQUE(ws,catalog)). Migrate+verify.
- **T2 (conn store + client provider):** `PolarisConnStore.upsert/get`; `polaris_client(store,secrets)` build `PolarisClient` (resolve admin secret); routes `GET/PUT /polaris/connection` + `/health`. Test upsert singleton.
- **T3 (catalog routes):** `POST /polaris/catalogs {name, storage_connection}` → map storage-connection → `StorageConfig::S3{allowed_locations:["s3://{bucket}/{name}/"]}` → `create_catalog`; list/get/delete proxy. Test map storage.
- **T4 (RBAC routes):** proxy principals(+credentials trả 1 lần)/principal-roles/catalog-roles/assignments/grants; map body JSON → `Grant`; tất cả `require_platform_admin`. Test add table grant proxy.
- **T5 (assignment):** `POST /control-plane/workspaces/{ws}/catalog-assignments {catalog}` → get_catalog → create_principal (clientId/secret) → principal-role+assign → catalog-role+assign → `add_grant(CATALOG_MANAGE_CONTENT)` → lưu Secret global (clientId+secret) → insert row (`ignore_conflict` cho idempotent); `DELETE` gỡ row+secret+best-effort delete_principal; `GET /api/ws/{ws}/catalogs` (WsContext, consume-only, không lộ credential). Test provision + row.
- **T6 (mount+verify):** wire `CpState` + nest full router `/api/control-plane` + workspace read; fmt+clippy+test; authz: non-admin → 403 mọi endpoint.

## Ghi chú
- Engine credential v1 = Polaris-native principal (clientId/secret lưu Secret global; engine dùng qua `/v1/oauth/tokens`). Migrate Keycloak-mapped sau.
- Grant assignment = `CATALOG_MANAGE_CONTENT` (rw toàn catalog, thô có chủ đích); governance mịn ở engine (Ranger).
- Ngoài P4: UI (P5), Polaris compose + e2e + wiring connector engine (P6).
