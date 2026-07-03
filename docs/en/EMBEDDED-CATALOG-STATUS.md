# Embedded Catalog epic — status (updated 2026-07-03)

Branch: `design/polaris-gateway-machine-idp`. Spec: `docs/en/specs/2026-07-02-egret-embedded-catalog-design.md`. Plans: `docs/en/plans/2026-07-02-eb{1..6}-*.md`.

## Done + live-verified (committed)

| Plan | Status | Evidence |
|---|---|---|
| **EB-1** re-embed lakekeeper | ✅ live | `/catalog/v1` served in-process, all-Rust; Polaris removed. |
| **EB-2** mini machine-IdP | ✅ live | `/.well-known/*` + `/oauth/token` (RS256, JWKS). |
| **EB-3** authN + authZ + provisioning | ✅ **live end-to-end** | see below. |
| **EB-4** control-plane mgmt + revert Polaris | ✅ | warehouses/roles/grants/clients routes; `egret-polaris` + polaris tables removed (migrations 0052–0054). |
| **EB-5** control-plane UI | ✅ live | Warehouses list + detail (roles/grants/clients, once-shown client secret); Polaris pages removed. Verified in the browser + 703 frontend tests. |
| **EB-6** engine wiring + e2e | ✅ live | **pyiceberg** (real Iceberg client) create-table + append + read (3 rows) round-trips to MinIO through egret; opt-in Playwright e2e (`EGRET_E2E_CATALOG=1`) drives the machine path + deny; `deploy/embedded-catalog/engine-wiring.md` (Trino + pyiceberg). |

**Epic complete (EB-1 → EB-6).** egret is a self-contained, all-Rust Iceberg catalog: owns authN (mini-IdP) + authZ (CatalogGuard), embeds lakekeeper, provisions warehouses, and serves real engines — no Polaris, no JVM, no external IdP for machines, no proxy. Full local CI green (backend fmt/clippy/93 test-suites; frontend tsc/lint/703 tests/build).

### EB-3 is complete — the embedded catalog works fully end-to-end
Realized "egret owns authZ" as an **egret `CatalogGuard` middleware** on the `/catalog/v1` mount (lakekeeper keeps `AllowAllAuthorizer`; egret decides). This avoids reimplementing lakekeeper's ~25-method `Authorizer` trait while giving egret full control. Live-verified chain:

- **authN** — valid, unexpired egret JWT required: no/invalid/expired token → 401.
- **warehouse isolation** — `/config?warehouse=` ≠ token's warehouse → 403; data-plane `/{prefix}/…` prefix ≠ token's lakekeeper warehouse id → 403.
- **fine-grained grants** — role with no grants → 403; `read` grant → GET 200 / POST 403; `write`/`manage` grant → POST 200.
- **provisioning** — `POST /warehouses` creates the real lakekeeper warehouse (S3/MinIO storage profile, secret resolved, rollback-on-failure) → returns `lakekeeperWarehouseId`.
- **data-plane** — create + list namespaces succeed, writing to `s3://warehouse/…` on MinIO.

All-Rust, in-process: no Polaris, no JVM, no external IdP. Commits: `d813609`, `2878e4d`, `75fa5a6`, `97a9382` (+ EB-1/2/4 earlier).

## Resolved along the way

- **`/config` `overrides.uri` = `:8181`** → FIXED: rewritten to the request `Host` in the mount middleware (both host + container clients get a reachable uri).
- **S3 access delegation** → FIXED: warehouses provisioned with STS + remote-signing disabled (the embedded mount has no signer endpoint), so clients use direct access-key creds. pyiceberg write/read verified.

## Future (out of epic)

- Un-pause notebook Daft-direct writes onto this catalog; auto-push a Catalog Client into engine configs; K8s provisioning of warehouses/engines.

## Dev notes
- `.env` is NOT auto-loaded by serve (figment reads process env). Run locally: `set -a; . ./.env; set +a; cargo run -p egret-bin -- serve`.
- Grant privilege convention (guard): `read` → reads; `write`/`manage` → writes. Grants are warehouse-wide for now (securable_type recorded but not yet scoped per-namespace/table in the guard).
- MinIO bucket must exist before provisioning (lakekeeper doesn't create buckets): `mc mb loc/warehouse`.
