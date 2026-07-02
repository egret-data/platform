# Control-Plane Polaris — Plan 3: crate client `egret-polaris` (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-polaris-p3-egret-polaris-client.md` (code/step đầy đủ).
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P3/6.** Crate standalone; P4 tiêu thụ. Thực thi qua subagent-driven-development.

**Mục tiêu:** client Rust typed + test cho **management API** của Polaris (catalog, principal+credentials, principal-role, catalog-role, grant) + lấy admin token, có **HttpTransport mock** để unit-test không cần Polaris thật.

**Kiến trúc:** client `reqwest`. Lấy **admin bearer qua OAuth2 client-credentials từ Keycloak** (Polaris `/v1/oauth/tokens` **DEPRECATED**; MIXED nhận OIDC). Mọi call → `{base}/api/management/v1/…` với `Authorization: Bearer`. Trait `HttpTransport` để test.

## Bối cảnh (từ `spec/polaris-management-service.yml`)
- Base `/api/management/v1`. Token: **không dùng** `/v1/oauth/tokens` (deprecated) → lấy từ Keycloak `POST {kc}/protocol/openid-connect/token` (client_credentials → access_token+expires_in).
- Catalog: `POST /catalogs` `{"catalog":{type:INTERNAL|EXTERNAL,name,properties,storageConfigInfo:{storageType:S3|GCS|AZURE|FILE,allowedLocations,storageName?}}}`; `GET /catalogs`, `GET/DELETE /catalogs/{name}`.
- Principal: `POST /principals` → **PrincipalWithCredentials{principal,credentials:{clientId,clientSecret}}** (credential engine — trả 1 lần); `DELETE`, `POST /principals/{n}/rotate`.
- principal-role: `POST /principal-roles`; assign `PUT /principals/{n}/principal-roles`. catalog-role: `POST /catalogs/{c}/catalog-roles`; assign `PUT /principal-roles/{pr}/catalog-roles/{c}`.
- Grant: `PUT /catalogs/{c}/catalog-roles/{cr}/grants` `{"grant":GrantResource}`; discriminated `type`: TableGrant{namespace,tableName,privilege}, ViewGrant, NamespaceGrant, CatalogGrant (table-level là mịn nhất).

## Task
- **T1 (scaffold+token):** crate `egret-polaris` (reqwest/serde/tokio/thiserror/async-trait); trait `HttpTransport` (mock được) + `ReqwestTransport`; `TokenSource.bearer(transport, now)` client-credentials Keycloak, cache refresh trước 30s (truyền `now` để test tất định). Test token fetch+cache.
- **T2 (client+catalog):** `PolarisClient` (`mgmt(path)`, `call<T>` map status 200/201/204/403→Forbidden/404→NotFound/409→Conflict); `create_catalog`/`list_catalogs`/`get_catalog`/`delete_catalog`; model `NewCatalog`/`CatalogType`/`StorageConfig`(S3/Gcs/Azure/File +`to_wire`)/`Catalog`. Test body POST đúng.
- **T3 (principal+role):** `create_principal`→credentials; `delete_principal`; `create_principal_role`/`create_catalog_role`; `assign_principal_role`/`assign_catalog_role`; model `PrincipalWithCredentials{credentials{client_id,client_secret}}`. Test trả credentials.
- **T4 (grant+error+health):** enum `Grant{Catalog,Namespace,Table,View}` + `to_wire`; `add_grant`/`revoke_grant`; `PolarisError{Forbidden,NotFound,Conflict,Status,Http,Decode}`; `health()` = list_catalogs. Test add table grant body.
- **T5 (verify):** fmt+clippy+test (`FakeTransport`, không cần Polaris thật).

## Ghi chú
- **Token: Keycloak client-credentials** (không dùng endpoint deprecated của Polaris) — tinh chỉnh "MIXED" của spec.
- **Open item cho P4:** model **service-credential cho engine** — `create_principal` trả clientId/secret Polaris-native chỉ chạy qua endpoint deprecated. Để engine tới Polaris không dùng endpoint deprecated: (a) engine dùng service-account Keycloak map sang principal Polaris qua OIDC (future-proof), hoặc (b) tạm chấp nhận endpoint deprecated. **Chốt ở P4.**
- Ngoài P3: routes (P4), UI (P5), Polaris compose/e2e (P6).
