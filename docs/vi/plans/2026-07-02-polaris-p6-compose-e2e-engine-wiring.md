# Control-Plane Polaris — Plan 6: dev Polaris + e2e + wiring engine (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-polaris-p6-compose-e2e-engine-wiring.md` (code/step đầy đủ).
> Spec: `docs/en/specs/2026-07-02-control-plane-polaris-catalog-design.md`. **P6 (cuối)** — phụ thuộc P1–P5. Thực thi qua subagent-driven-development.

**Mục tiêu:** chạy **Apache Polaris** trong stack dev (docker-compose, thay lakekeeper nhúng đã xoá); wire Keycloak cho Polaris MIXED; chứng minh vòng lặp đầy đủ: register connection → tạo catalog trên MinIO → gán workspace → engine (Trino) đọc/ghi → roundtrip `write_table` qua engine.

## Bối cảnh
- Compose dev: postgres/minio/trino(8090:8080,8453:8443,JWT/TLS,catalog `./trino/catalog`,ACL rules.json)/starrocks/keycloak(8081, realm egret). P1 đã xoá lakekeeper nhúng.
- Polaris config (application.properties): `polaris.persistence.type` in-memory|relational-jdbc; `polaris.realm-context.*` (default POLARIS); auth `INTERNAL|EXTERNAL|MIXED`; OIDC `quarkus.oidc.auth-server-url`. Template: `polaris/regtests/docker-compose.yml` + getting-started. Image `apache/polaris`.
- Keycloak realm `egret-realm.json` (client egret + aud egret-catalog/trino/starrocks) → thêm aud `polaris` + client service-account `polaris-admin`.
- P3/P4: egret-admin token qua Keycloak client-credentials; assignment cấp principal Polaris-native (clientId/secret lưu Secret) = credential engine (qua `/v1/oauth/tokens`).
- Trino Iceberg REST (đã spike với lakekeeper, shape giống Polaris): `iceberg.catalog.type=rest` + `rest-catalog.uri=<polaris>/api/catalog` + OAUTH2 + `fs.native-s3` MinIO.

## Task
- **T1 (compose polaris):** thêm service `polaris` (image apache/polaris; env: persistence in-memory, realm default POLARIS, auth MIXED, `quarkus.oidc.auth-server-url=http://keycloak:8080/realms/egret`, MinIO creds; port 8181; healthcheck). *Xác nhận tag image + key OIDC role-mapping/S3 theo `regtests/docker-compose.yml` + `application.properties` — đây là phần chưa-pin nhất, spike config ngắn.* Up + health.
- **T2 (Keycloak):** thêm aud-polaris mapper + client `polaris-admin` (service account, client-credentials) + realm role map sang Polaris principal-role có `SERVICE_MANAGE_ACCESS`. Re-import + verify token có `aud: polaris`.
- **T3 (bootstrap + register):** bootstrap realm POLARIS + root (admin tool / `POST /management/v1/bootstrap`); `PUT /api/control-plane/polaris/connection` (baseUrl polaris:8181, kcTokenUrl, polaris-admin + secret) → health ok. Document.
- **T4 (wiring Trino):** `deploy/trino/catalog/iceberg.properties` → Polaris REST + `oauth2.credential=clientId:clientSecret` (từ assignment) + `oauth2.server-uri=.../v1/oauth/tokens` + fs.native-s3 MinIO; restart Trino; verify `trino("SHOW SCHEMAS FROM iceberg")`. (Đẩy tự động credential vào connector = future engines-centralization/K8s epic; ở đây manual cho dev.)
- **T5 (e2e opt-in):** `polaris-catalog.spec.ts`: admin register connection → tạo catalog `lakehouse` trên `minio` → gán `eng`; tester → cell `write_table(df,"lakehouse.e2e.roundtrip",engine="trino",mode="overwrite")` + `trino("SELECT ...")` → assert `${marker} 42`. README ghi setup (polaris up + polaris-admin + Trino iceberg wired + group-provider/relax ACL).
- **T6 (verify + wrap-up):** stack đầy đủ + serve → chạy e2e (roundtrip pass); full CI local (cargo fmt/clippy/test workspace + frontend + kernel pytest) vì GitHub CI hết quota.

## Ghi chú
- Phần chưa-pin (flag): config container Polaris (tag + OIDC role-mapping) — spike theo example của Polaris; đẩy-tự-động credential vào engine config — manual dev, tự động ở future epic.
- **Epic xong sau P6:** P1 xoá → P2 storage/secrets→CP → P3 client → P4 routes+RBAC+assignment → P5 UI → P6 dev+e2e+wiring. Future: epic K8s-provisioning (egret provision engine + Polaris trên K8s qua operator/Helm).
