# Gateway P-D: Polaris EXTERNAL-OIDC + e2e (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-gw-pd-polaris-oidc-e2e.md`. Spec Trụ 4. **P-D (cuối).** Phụ thuộc P-A/P-B/P-C. subagent-driven.

**Mục tiêu:** Polaris tin egret mini-IdP (EXTERNAL/OIDC), map claim `roles`→catalog-role, chứng minh vòng lặp: tạo Catalog Client → `/oauth/token` → proxy `/catalog/v1/*` → Polaris verify qua egret JWKS → đọc/ghi Iceberg trên MinIO.

## Bối cảnh
- Polaris dev (INTERNAL, root/s3cr3t). egret chạy host (localhost:8080); container tới host qua `host.docker.internal`.
- egret mini-IdP (P-B): discovery/jwks/token; JWT `iss` = public root egret. **Wrinkle then chốt:** `iss` egret stamp PHẢI khớp issuer Polaris resolve từ `auth-server-url`. Dev: set cả hai = `http://host.docker.internal:8080` (client ngoài cũng dùng URL này, hoặc nới strict issuer). Báo cáo cái gì chạy.
- Key `polaris.oidc.*` mapping = phần **chưa-pin nhất** → spike (T1).

## Task
- **T1 (Polaris EXTERNAL-OIDC — spike):** set env polaris: `authentication.type=external|mixed`, `quarkus.oidc.auth-server-url=http://host.docker.internal:8080`, `token.audience=polaris`, `roles.role-claim-path=roles`. Serve egret trước (discovery/jwks live), restart Polaris, xác nhận init OIDC + fetch JWKS OK. Spike mapping `roles:["writer"]`→catalog-role `writer` (có thể `polaris.oidc.principal-roles-mapper.*`); nếu Polaris cần principal/identity → fallback option (b). Ghi `deploy/polaris/oidc.md`.
- **T2 (Trino qua proxy):** `iceberg.properties`: `uri=http://host.docker.internal:8080/catalog` (proxy egret, KHÔNG polaris:8181) + `oauth2.credential=<clientId>:<secret>` (Catalog Client trino) + `oauth2.server-uri=http://host.docker.internal:8080/oauth/token` (IdP egret) + fs.native-s3 MinIO. Restart Trino, catalog load OK. Ghi `engine-wiring.md`.
- **T3 (e2e + verify):** `polaris-gateway.spec.ts` opt-in: admin tạo catalog+role+grant+Client(trino); assert machine path (`/oauth/token`→JWT; `/catalog/v1/config` Bearer→200 + URL rewrite; create namespace/table qua proxy → commit MinIO → read back; hoặc Trino `CREATE TABLE ... AS SELECT 42`→`SELECT`→42). README setup. Full CI local (cargo fmt/clippy/test workspace + frontend + kernel).

## Ghi chú
- Chưa-pin (flag): key `polaris.oidc.*` mapping + đồng nhất issuer `iss` egret vs `auth-server-url` Polaris → chốt ở spike T1, ghi `oidc.md`.
- **Epic xong sau P-D:** P-A proxy → P-B IdP+clients → P-C RBAC theo catalog → P-D trust+e2e. egret là edge duy nhất; máy auth qua egret IdP; Polaris ẩn + authЗ; không dính endpoint deprecated, không Keycloak/EntraID cho máy.
