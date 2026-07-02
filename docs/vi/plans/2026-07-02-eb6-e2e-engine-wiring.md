# EB-6: engine wiring + e2e (bản tham chiếu)

> Bản chuẩn: `docs/en/plans/2026-07-02-eb6-e2e-engine-wiring.md`. Spec Component 6. **EB-6 (cuối).** Phụ thuộc EB-1…EB-5. subagent-driven.

**Mục tiêu:** chứng minh vòng lặp embedded-catalog + wire engine: tạo warehouse+role+Catalog Client → `/oauth/token` → `/catalog/v1/*` (lakekeeper nhúng) → egret Authorizer → đọc/ghi Iceberg trên MinIO; rồi trỏ Trino vào egret bằng Catalog Client.

## Bối cảnh
- Sau EB-1…EB-5: egret serve `/catalog/v1/*` + `/oauth/token`+`/.well-known/*` + mgmt/UI. Không Polaris, không proxy. egret host (localhost:8080); Trino container → `host.docker.internal:8080`.

## Task
- **T1 (wire Trino):** tạo warehouse `lakehouse` + role `engine` (grant table rw) + Catalog Client(engine). `iceberg.properties`: `uri=http://host.docker.internal:8080/catalog` + `oauth2.credential=<clientId>:<secret>` + `oauth2.server-uri=http://host.docker.internal:8080/oauth/token` + fs.native-s3 MinIO. Restart Trino, catalog load OK. Ghi `deploy/embedded-catalog/engine-wiring.md`.
- **T2 (e2e opt-in):** `embedded-catalog.spec.ts`: admin tạo warehouse+role+grant+client; machine path (`/oauth/token`→JWT; `/catalog/v1/config` Bearer→200; tạo namespace+table qua `/catalog/v1/*`→commit MinIO→read back; role thiếu grant→403) hoặc Trino `CREATE TABLE lakehouse.e2e.t AS SELECT 42`→`SELECT`→42. README setup.
- **T3 (full CI + wrap-up):** full CI local (cargo fmt/clippy/test workspace + frontend + kernel); live loop pass (rw MinIO qua egret; deny→403).

## Ghi chú
- **Epic xong sau EB-6:** EB-1 embed → EB-2 mini-IdP → EB-3 store+Authorizer+authN → EB-4 mgmt+revert Polaris → EB-5 UI → EB-6 e2e. egret = catalog all-Rust tự chứa: own authN+authZ, nhúng lakekeeper, không JVM/proxy/IdP-doanh-nghiệp-cho-máy/endpoint-deprecated.
- Future: notebook Daft-direct un-pause; auto-push credential vào engine; K8s provisioning.
