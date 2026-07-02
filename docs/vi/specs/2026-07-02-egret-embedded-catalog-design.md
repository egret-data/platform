# egret Embedded Catalog — Thiết kế (Kiến trúc B) (bản tham chiếu)

> Bản chuẩn: `docs/en/specs/2026-07-02-egret-embedded-catalog-design.md`. **Supersede** `2026-07-02-control-plane-polaris-catalog-design.md` (đã merge PR #115) + `2026-07-02-polaris-gateway-machine-idp-design.md`.

## Vì sao thay hướng Polaris
egret rời lakekeeper→Polaris chỉ vì Polaris **tự cấp token**. Nhưng end-state là egret **tự own machine auth** (mini-IdP) — vì token nội bộ Polaris deprecated, Keycloak quá nặng, và cấp credential máy trong IdP doanh nghiệp (EntraID) bất khả thi. Khi egret own IdP, lợi thế Polaris biến mất → chạy **JVM Polaris + proxy** nặng hơn **nhúng lakekeeper (Rust, in-process)** vốn thiết kế để đứng sau IdP ngoài. → Kiến trúc đúng: **egret là catalog tự chứa, all-Rust** — nhúng lakekeeper, tự cấp token, tự authZ. Bản hiện đại hoá của mô hình embedded gốc + JWT machine identity + RBAC theo catalog.

## Nguyên tắc lõi — egret own catalog + auth của nó
| Việc | Chủ |
|---|---|
| Iceberg REST catalog (protocol, metadata, S3) | **lakekeeper nhúng** (crate Rust, trong egret-server) |
| AuthN máy (cấp token) | **egret mini-IdP** (`/oauth/token` + JWKS) |
| AuthZ | **egret Authorizer** (custom, cắm vào lakekeeper; **không OpenFGA**) |
| SSO người (login egret) | IdP doanh nghiệp (EntraID/Keycloak) — riêng |
→ Không service catalog riêng, **không proxy** (catalog LÀ route `/catalog/v1/*` của egret), không JVM, không dính IdP doanh nghiệp cho máy, không dính endpoint deprecated.

## Quyết định đã chốt
- **Kiến trúc B:** nhúng lakekeeper + egret mini-IdP + egret tự viết Authorizer (không OpenFGA).
- Consumers: engines/tools ngoài/notebook(Daft direct), mỗi cái 1 Catalog Client; không per-user trực tiếp.
- Credential: 1/consumer, revoke riêng.
- **Warehouse (hybrid):** warehouse **per workspace/domain**; trong mỗi warehouse có **catalog-role theo consumer-type** (engine/notebook/external) mang grants; Client gắn 1 role.

## Kiến trúc
```
Consumer (clientId/secret) ─▶ egret /oauth/token ─▶ JWT(sub,roles,warehouse)
Consumer (Bearer) ─▶ egret /catalog/v1/* (lakekeeper NHÚNG)
                        ├─ authN: validate JWT egret (JWKS)
                        └─ authZ: egret Authorizer (role→grants) ─▶ MinIO/S3
egret /api/control-plane/* : quản warehouse/role/grant/client
```
- **C1 lakekeeper nhúng:** add lại `egret-catalog-engine` (git history pre-P1, lakekeeper pinned), mount `/catalog/v1/*` trong egret-server. AuthN: cấu hình OIDC lakekeeper tin JWKS egret (in-process có thể đưa key trực tiếp — spike). AuthZ: cắm egret Authorizer vào trait `Authorizer`.
- **C2 mini-IdP:** `/oauth/token` (client_credentials→JWT: iss,sub,warehouse,roles,exp; RS256 rotatable) + JWKS + discovery; store `oidc_signing_keys`.
- **C3 Authorizer:** impl trait `Authorizer` lakekeeper; principal(JWT)+resource+action → allow/deny theo store egret (role→grants); table-level; deny-by-default; không service ngoài.

## Data model (store egret)
- `warehouses { id, name, workspace_id, storage_conn_id, lakekeeper_warehouse_id }` — per workspace/domain (dùng lại `storage_connections` P2).
- `catalog_roles { id, warehouse_id, name, consumer_type(engine|notebook|external) }`.
- `catalog_grants { id, catalog_role_id, securable_type, namespace text[], table_name, privilege }`.
- `catalog_clients { id, name, consumer_type, workspace_id, warehouse_id, catalog_role_id, client_id UNIQUE, client_secret_hash, created_at, last_used_at }`.
- `oidc_signing_keys { kid, algorithm, private_pem, public_jwk, created_at, retired_at }`.

## Re-add / revert
- **Add lại** (git history pre-P1): `egret-catalog-engine` + mount `/catalog/v1` + pin lakekeeper. Hiện đại hoá `catalog_tokens`/`workspace_warehouses` cũ → `warehouses` + `catalog_clients` + `catalog_roles`/`catalog_grants` (JWT+RBAC, không opaque token).
- **Revert** (PR #115): `egret-polaris`, route control-plane proxy Polaris, Polaris trong compose. Giữ crate control-plane + Storage/Secrets (P2) + platform-admin gate.
- **Giữ:** `storage_connections` (P2), shell control-plane + trang Storage/Secrets; làm lại trang catalog/RBAC trên store mới.

## Bảo mật
- `client_secret` argon2 + hiện 1 lần; revoke riêng. JWT TTL ngắn, `aud`/`iss` validate bởi lakekeeper, RS256 `kid` rotatable. Authorizer deny-by-default. `/oauth/token` rate-limit + constant-time; không log token/secret.

## Ngoài scope / future
- Un-pause notebook Daft-direct qua Catalog Client (sub-epic). Auto-đẩy credential vào engine (future). Per-user identity + governance cột/dòng (ở engine/Ranger). IdP doanh nghiệp cho máy (loại). Federation lakekeeper (chưa cần).

## Rủi ro
- **Tích hợp auth lakekeeper** (C1) chưa-pin nhất — cách đưa JWT egret + Authorizer vào lakekeeper nhúng (OIDC config vs key trực tiếp vs custom authenticator) cần spike vs revision lakekeeper đã pin.
- Churn: đảo hướng PR #115 + add lại code đã xoá — lớn nhưng recover từ git; cố ý theo lựa chọn chiến lược.
- egret giờ own code authN+authZ (trách nhiệm hơn) — đánh đổi cho stack all-Rust tự chứa, không JVM.

## Phân rã (1 plan / component)
- **EB-1** nhúng lại lakekeeper (`egret-catalog-engine` khôi phục + mount `/catalog/v1`, authorizer AllowAll trước); revert Polaris compose.
- **EB-2** egret mini-IdP (`/oauth/token`, JWKS, discovery, signing-key) + `catalog_clients`.
- **EB-3** data model (`warehouses`,`catalog_roles`,`catalog_grants`) + egret **Authorizer** cắm lakekeeper (deny-default, role→grants) + wiring authN JWT (spike tích hợp).
- **EB-4** quản control-plane (warehouses, roles/grants, clients) + revert `egret-polaris` + route Polaris.
- **EB-5** RBAC UI theo catalog (warehouse detail: role theo consumer-type + grants + clients) trên store mới; bỏ trang thời-Polaris.
- **EB-6** e2e (tạo warehouse+role+client → `/oauth/token` → `/catalog/v1` → authZ lakekeeper → đọc/ghi MinIO) + wiring engine + full CI.
