# EB-3: data model + egret Authorizer + JWT authN (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-eb3-store-authorizer-authn.md`. Spec Component 3. **Lõi tích hợp.** Phụ thuộc EB-1 (lakekeeper nhúng + seam `Authorizer`) + EB-2 (mini-IdP + `ClientVerifier`). subagent-driven.

**Mục tiêu:** RBAC store của egret (`warehouses`,`catalog_roles`,`catalog_grants`,`catalog_clients`); `CatalogClientStore` (impl `ClientVerifier`); egret **Authorizer** cắm lakekeeper (deny-default, role→grants); wiring lakekeeper nhúng **validate JWT egret** (spike tích hợp).

## Task
- **T1 (migration 0053):** `warehouses{id,name,workspace_id,storage_conn_id→storage_connections,lakekeeper_warehouse_id,UNIQUE(ws,name)}` + `catalog_roles{id,warehouse_id,name,consumer_type CHECK(engine|notebook|external),UNIQUE(wh,name)}` + `catalog_grants{id,catalog_role_id,securable_type CHECK(catalog|namespace|table),namespace text[],table_name,privilege}` + `catalog_clients{...,warehouse_id,catalog_role_id,client_id UNIQUE,client_secret_hash,...}`. migrate+verify.
- **T2 (stores):** `WarehouseStore.create` (tạo warehouse lakekeeper qua embedded engine + lưu id)/list/get/delete; `RoleStore` create_role/list_roles/add_grant/list_grants/revoke_grant; `CatalogClientStore.create` (client_id `egret_`+hex, secret 32B base64url, argon2, INSERT)/`verify` (argon2 constant-time + join role+warehouse + update last_used)/list/revoke + impl `ClientVerifier`. Test create→verify.
- **T3 (Authorizer):** pure `decide(role_grants, action, securable)→bool` (deny-default; grant catalog⊇namespace⊇table); `EgretAuthorizer` impl trait `Authorizer` lakekeeper (lấy roles+warehouse từ JWT claims → load grants → decide). Test allow/deny/unknown-role.
- **T4 (authN spike):** chốt cách lakekeeper nhúng validate JWT egret + đưa sub/roles/warehouse tới Authorizer — ưu tiên: (1) key trực tiếp in-process, (2) OIDC trỏ JWKS egret localhost, (3) custom authenticator (`token::verify_with_jwk`). Ghi `README-auth.md`. Swap AllowAll→EgretAuthorizer + placeholder→CatalogClientStore. build.
- **T5 (verify):** serve → tạo warehouse+role(grant)+client → `/oauth/token`→JWT → `/catalog/v1/...` allowed; role thiếu grant → 403. fmt+clippy+test.

## Ghi chú
- Chưa-pin (flag): tích hợp auth lakekeeper (T4) → spike + ghi doc. Table-level; không OpenFGA.
- Ngoài EB-3: mgmt routes + revert egret-polaris (EB-4), UI (EB-5), e2e (EB-6).
