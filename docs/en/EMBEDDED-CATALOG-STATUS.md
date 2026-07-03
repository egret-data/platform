# Embedded Catalog epic — status (updated 2026-07-03)

Branch: `design/polaris-gateway-machine-idp`. Spec: `docs/en/specs/2026-07-02-egret-embedded-catalog-design.md`. Plans: `docs/en/plans/2026-07-02-eb{1..6}-*.md`.

## Done + live-verified (committed)

| Plan | Status | Evidence |
|---|---|---|
| **EB-1** re-embed lakekeeper | ✅ live | `/catalog/v1` served in-process, all-Rust; Polaris removed. |
| **EB-2** mini machine-IdP | ✅ live | `/.well-known/*` + `/oauth/token` (RS256, JWKS). |
| **EB-3** authN + authZ + provisioning | ✅ **live end-to-end** | see below. |
| **EB-4** control-plane mgmt + revert Polaris | ✅ | warehouses/roles/grants/clients routes; `egret-polaris` + polaris tables removed (migrations 0052–0054). |

### EB-3 is complete — the embedded catalog works fully end-to-end
Realized "egret owns authZ" as an **egret `CatalogGuard` middleware** on the `/catalog/v1` mount (lakekeeper keeps `AllowAllAuthorizer`; egret decides). This avoids reimplementing lakekeeper's ~25-method `Authorizer` trait while giving egret full control. Live-verified chain:

- **authN** — valid, unexpired egret JWT required: no/invalid/expired token → 401.
- **warehouse isolation** — `/config?warehouse=` ≠ token's warehouse → 403; data-plane `/{prefix}/…` prefix ≠ token's lakekeeper warehouse id → 403.
- **fine-grained grants** — role with no grants → 403; `read` grant → GET 200 / POST 403; `write`/`manage` grant → POST 200.
- **provisioning** — `POST /warehouses` creates the real lakekeeper warehouse (S3/MinIO storage profile, secret resolved, rollback-on-failure) → returns `lakekeeperWarehouseId`.
- **data-plane** — create + list namespaces succeed, writing to `s3://warehouse/…` on MinIO.

All-Rust, in-process: no Polaris, no JVM, no external IdP. Commits: `d813609`, `2878e4d`, `75fa5a6`, `97a9382` (+ EB-1/2/4 earlier).

## Remaining

1. **`/config` `overrides.uri` = `http://localhost:8181`** (lakekeeper's hardcoded metadata `base_url`; no prod setter — only a `test-utils`-gated constructor takes a custom base_url). Real Iceberg clients that follow `overrides.uri` for subsequent calls would break; hitting egret's configured URI + prefix directly works (as demonstrated). Fix needs a lakekeeper-metadata shim or setting the base_url at metadata construction. **Non-blocking, deferred.**
2. **EB-5 — UI**: warehouses list + detail (roles/grants/clients) pages; remove the stale `ControlPlaneCatalog`/`ControlPlaneRbac` Polaris pages (compile but call removed routes). Mgmt routes it needs already exist.
3. **EB-6 — e2e + engine wiring**: formalize an automated e2e (the manual chain above is proven); wire an engine (Trino/StarRocks/Daft) to authenticate via a Catalog Client.

## Dev notes
- `.env` is NOT auto-loaded by serve (figment reads process env). Run locally: `set -a; . ./.env; set +a; cargo run -p egret-bin -- serve`.
- Grant privilege convention (guard): `read` → reads; `write`/`manage` → writes. Grants are warehouse-wide for now (securable_type recorded but not yet scoped per-namespace/table in the guard).
- MinIO bucket must exist before provisioning (lakekeeper doesn't create buckets): `mc mb loc/warehouse`.
