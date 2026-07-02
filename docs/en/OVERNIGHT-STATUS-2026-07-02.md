# Overnight status — Embedded Catalog epic (2026-07-02)

Branch: `design/polaris-gateway-machine-idp`. Spec: `docs/en/specs/2026-07-02-egret-embedded-catalog-design.md`. Plans: `docs/en/plans/2026-07-02-eb{1..6}-*.md`.

## Done + verified (committed)

| Plan | Status | Evidence |
|---|---|---|
| **EB-1** re-embed lakekeeper | ✅ live | `/catalog/v1/config` served in-process (proper `NoSuchWarehouse` — pipeline works: mount → system RequestMetadata → AllowAll → handler → DB). Polaris removed from compose. |
| **EB-2** mini machine-IdP | ✅ live | `/.well-known/openid-configuration` + `/oauth/token` (RS256, JWKS). 3 unit tests. |
| **EB-3 (part)** data model + stores + **authN token path** | ✅ live | migration 0053 (warehouses/roles/grants/clients); `CatalogStore` RBAC store + `create_client`(argon2)/`verify_client` + `ClientVerifier` impl; **create client → `/oauth/token` → egret JWT `{sub, aud=catalog, warehouse, roles}` verified end-to-end**. |
| **EB-4** control-plane mgmt + revert Polaris | ✅ | warehouses/roles/grants/clients mgmt routes (platform-admin); removed `egret-polaris` crate + polaris_conn/catalogs/rbac/assignment modules + CpState fields; migration 0054 drops `polaris_connection`/`workspace_catalog_assignments`. |

Full workspace builds; `cargo fmt --all --check` clean; egret-machine-idp + egret-catalog + egret-controlplane tests pass.

## Remaining (NOT done)

### EB-3 T3/T4 — the custom lakekeeper Authorizer + JWT authN (the hard spike)
This is the crux of "egret owns authZ" and is a **large, careful task, not a quick spike** — deliberately left for review rather than rushed:
- lakekeeper's `Authorizer` trait is **large** (dozens of per-securable methods: `is_allowed_{server,warehouse,namespace,table,view,role,user}_action` + batch `are_allowed_*` + relationship-management). See `~/.cargo/git/checkouts/lakekeeper-*/7916505/crates/lakekeeper/src/service/authz/`.
- Currently the mount uses lakekeeper's built-in **`AllowAllAuthorizer`** + a system `RequestMetadata` — so `/catalog/v1` serves but does **not** yet validate the egret JWT nor enforce per-role grants.
- The store already has `grants_for_role(warehouse_name, role_name)` (the lookup a real Authorizer needs from the JWT `warehouse`+`roles` claims).
- **Integration decision still open** (EB-3 plan T4): how the embedded lakekeeper validates an egret-issued JWT + surfaces `sub`/`roles`/`warehouse` to the Authorizer — options: (1) hand lakekeeper the egret public key in-process, (2) point its OIDC at egret's `/.well-known/*`, (3) a custom authenticator calling `egret_machine_idp::verify_with_jwk`. Needs a focused spike against the pinned lakekeeper rev.
- Also open: **provisioning the lakekeeper warehouse** when a warehouse is created (the mgmt route records the egret row + `set_lakekeeper_warehouse_id` exists, but the call into lakekeeper's create-warehouse API is not wired). A warehouse has no lakekeeper counterpart yet.

### EB-5 — UI (warehouses list + detail: roles/grants/clients); remove the stale Polaris pages
Not started. The mgmt routes it needs (EB-4) exist. The frontend still has the old `ControlPlaneCatalog`/`ControlPlaneRbac` pages calling now-removed Polaris routes — they compile but are dead; EB-5 replaces them.

### EB-6 — e2e + engine wiring
Not started; needs the Authorizer (deny path) + warehouse provisioning.

## Suggested next session order
1. **EB-3 T4 spike** — wire lakekeeper to validate the egret JWT + provision the lakekeeper warehouse on create (this unblocks a real read/write path).
2. **EB-3 T3** — the custom Authorizer (start minimal: map the JWT's warehouse+role → `grants_for_role` → allow/deny; expand method coverage).
3. **EB-5** UI, then **EB-6** e2e.

## Dev notes
- `.env` is NOT auto-loaded by serve (figment reads process env). To run the embedded catalog locally: `set -a; . ./.env; set +a; cargo run -p egret-bin -- serve` — `.env` now has `EGRET_CATALOG__ENABLED=true` + storage vars (local only, gitignored).
- Subagents kept stalling on the ~3 min cargo builds (600 s watchdog) — build-heavy tasks were driven directly.
