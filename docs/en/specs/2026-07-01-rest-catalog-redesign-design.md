# REST Catalog Redesign — Design

> Bilingual: English canonical; VI reference at `docs/vi/specs/2026-07-01-rest-catalog-redesign-design.md`.

- **Date:** 2026-07-01
- **Status:** Approved (brainstorming) — ready for implementation plan
- **Scope:** Redesign egret's Iceberg REST catalog integration to fix, at the root, the five plumbing problems surfaced while making notebook Iceberg work live (PR #114 was a stop-gap), AND establish the governance model that keeps data authorization consistent across every read path. Unify notebook catalog auth on OIDC-JWT delegation, adopt vended STS credentials, split the overloaded config, fix the embedded-lakekeeper base URL at its source, make warehouse selection explicit by purpose, remove the legacy hand-rolled catalog, and enforce "governed data is read only through the query engine" by credential separation.

## Background / current state

egret embeds **lakekeeper** (git-pinned `0.12.3`, rev `79165057`) in-process and mounts it as the live Iceberg REST catalog at **`/catalog/v1`** (`AppState.catalog_engine`, assembled in `egret-bin`, fronted by the `egret-catalog-engine::front_auth` per-workspace token-ownership middleware). A session-authed browse API (`AppState.catalog_browse`) reads the same lakekeeper for the UI. `egret-catalog` provides the **identity layer** — `CatalogStore` (opaque `catalog_tokens` + `workspace_warehouses` → lakekeeper-warehouse-UUID mapping). An earlier **hand-rolled** Iceberg REST surface (`egret-catalog::api`, `egret-iceberg-ext` types) predates the lakekeeper embed and is now largely superseded.

`front_auth` is a **hybrid authenticator**: a bearer that is a JWT (three dot-segments) is verified via `CatalogJwtAuth`/`LimesVerifier` (F2-P2, `aud=egret-catalog`, the same OIDC delegation Trino/StarRocks use); anything else is an opaque `catalog_tokens` lookup. The notebook currently uses the **opaque** path (a 1-grant rw token minted per manifest fetch, mint-and-revoke).

### The five plumbing problems (all root-caused here)

1. **Internal base URL leaks (`:8181`).** lakekeeper reports its hardcoded internal bind address in self-referential fields — `/config` `overrides.uri` and per-table `s3.signer.uri` — because `RequestMetadata.base_url` defaults to `http://localhost:8181` (`new_lakekeeper_internal`) with no public setter at this rev, and egret injects that pre-built metadata (bypassing lakekeeper's header/`CONFIG.base_uri` determination). Clients follow those URLs and cannot reach `:8181` (lakekeeper is only exposed via the `/catalog` proxy). PR #114 works around this by rewriting response bodies — a band-aid.
2. **`config.catalog.public_url` is overloaded.** It feeds *both* the catalog base URL *and* the kernel callback `api_url` (`egret-bin`). Setting it correctly for the catalog points `api_url` at `.../catalog`, which breaks the governed engine-token / Trino / StarRocks kernel callbacks.
3. **Mint-and-revoke token lifecycle → flaky 401.** The notebook's opaque catalog token is minted per manifest fetch and revokes prior tokens (one active per user×workspace). A running kernel holds a token that any later manifest fetch (reopening another notebook) revokes → re-running a cell after reopening 401s.
4. **Remote S3 signing is unnecessary overhead.** Reads route through lakekeeper's `/signer/{warehouse}/…/aws/s3/sign` path (an extra round-trip per file op, and the source of a front_auth ownership bug fixed in #114).
5. **Legacy dual catalog code.** The hand-rolled `egret-catalog::api` + unused `egret-iceberg-ext` REST types coexist with lakekeeper, making it look like there are "two catalogs" and inviting confusion.

Plus a selection-fragility risk: the notebook catalog is chosen by **name convention** (`"notebook"`) + silent fallback-to-first — implicit and error-prone.

## Governance model (the design's north star)

The product goal is **authorize data once — a user reading through any engine gets the same result.** The hard constraint:

- **Column-level / row-level** control (masking, row filters) can only be enforced by a **query engine that rewrites the query at plan time** (Trino/StarRocks + Ranger). A direct file reader (Daft reading Iceberg/parquet) has no query-rewrite layer, and even vended STS credentials only scope to **table/prefix level** — never column/row.
- Iceberg REST catalog authz (lakekeeper today) is **table-level only**.

### Approach A — governed data is engine-only (adopted)

Rather than trying to make two read paths agree, remove the second path for governed data. There is exactly **one** enforcement point for column/row policy: **Ranger, via the engine.**

Two Iceberg catalogs, **both registered in Trino/StarRocks**:

| | `lakehouse` (governed) | `notebook` (EDA outputs) |
|---|---|---|
| **Notebook Daft-direct** (`iceberg`/`write_iceberg`) | ❌ credential has no grant | ✅ read-write the user's own namespace |
| **Notebook via `trino()`/`starrocks()`** | ✅ read, Ranger row/column applied | ✅ read (shared outputs), Ranger applies |
| **Pipelines / BI via engine** | ✅ read-write (governed) | ✅ read notebook outputs |

- `lakehouse` = the governed lake. Engines read-write it (pipelines/BI). Notebook users **read it only through `trino()`/`starrocks()`** (Ranger enforces column/row); notebook users cannot write it.
- `notebook` = EDA outputs. Notebook users **write via Daft-direct** into their own namespace; outputs are **shared for others to read through the engine** (the `notebook` catalog is a first-class engine-registered catalog, Ranger-governable on the engine path).

**Enforcement is by credential separation, not convention.** The kernel's catalog JWT (`aud=egret-catalog`) is authorized only on the **`notebook`-purpose** warehouse; it holds no grant on any governed warehouse. So `iceberg("lakehouse.t")` (Daft-direct) is *denied by the catalog*, not merely discouraged. The only way to reach `lakehouse` is `trino()`/`starrocks()`, where the user presents an `aud=trino`/`aud=starrocks` JWT and Ranger governs the result. One policy in Ranger; every read of governed data passes through it; consistent by construction.

**Trade-off (accepted):** notebook users do not get Daft's direct-parquet speed on `lakehouse` (governed reads go through the engine); they do get full Daft-direct speed on their own `notebook` data. Cross-user reads of notebook outputs are governed only when read via the engine (Daft-direct read is limited to the user's own namespace).

### Approach C — catalog-native FGAC (future direction, not a dependency)

The elegant long-term answer flips the single-source-of-truth to the **catalog**: the catalog holds row/column policy and, via REST **scan planning** (`planTable`/`fetchScanTasks`), returns an already-governed scan (rows filtered, columns masked) so **any** compliant reader — Daft-direct included — is consistent from one catalog-side policy. This is the industry direction (Apache Polaris issue [#137](https://github.com/apache/polaris/issues/137) "Row and Column Level Access Control" — open, in progress, no PR; lakekeeper has no equivalent yet) and it is **not available today**. It would also require Daft to honor catalog-returned residual predicates + column transforms (Daft does not yet). We do **not** depend on it and do **not** switch catalogs for it. We keep the catalog abstraction clean and the governed-read path isolated so that, if lakekeeper (or a future catalog) ships FGAC scan-planning and Daft learns to honor it, we can adopt Approach C without re-architecting.

## Credential model (three planes, one identity)

All three are per-user JWTs minted from one Keycloak identity by `EngineTokenMinter` (RFC 8693 token-exchange), differing only by audience, refreshed by the same refresh-grant:

| Plane | Credential | Verified by | Scope |
|---|---|---|---|
| Catalog (Daft-direct) | JWT `aud=egret-catalog` | egret `front_auth` (`CatalogJwtAuth`) | rw the user's namespace in the `notebook` warehouse only |
| Trino | JWT `aud=trino` | Trino (Keycloak JWKS) + Ranger | governed reads (lakehouse + notebook) |
| StarRocks | JWT `aud=starrocks` | StarRocks (Keycloak JWKS) + Ranger | governed reads (lakehouse + notebook) |

The **opaque** catalog token is retained only for admin / warehouse-management / non-OIDC use (`catalog mint-token` CLI, external tools). An opaque token authenticates only to egret's catalog REST endpoint; it is **not** a credential for Trino/StarRocks (those accept only Keycloak JWTs).

## Architecture / components

### A. base_url at the source (fixes #1) — `egret-catalog-engine`, lakekeeper fork

- Add `pub fn with_base_url(mut self, base_url: String) -> Self` to `RequestMetadata` in egret's **pinned lakekeeper fork** (egret already git-pins and patches deps — the `mysql_async` precedent). Minimal, explicit, unit-testable.
- Set `LAKEKEEPER__BASE_URI` and egret's `config.catalog.public_url` from one source of truth at boot.
- In `front_auth.rs`, the three `RequestMetadata` construction sites (`token_request_metadata`, the system-principal builder, the browse builder) set `base_url` = the configured public catalog URL.
- **Remove** `rewrite_lakekeeper_base` + the response-rewrite block + the JSON buffering added in PR #114. `overrides.uri` and `s3.signer.uri` are then correct at the source.
- *Fallback if forking is rejected:* keep a single clean, config-driven rewrite layer (documented, not the default).

### B. Config split (fixes #2) — `egret-config`, `egret-bin`

- `config.catalog.public_url` → the external catalog URL only (lakekeeper `base_uri` + the notebook manifest catalog URL).
- New `config.server.public_url` (kernel callback base / `api_url`) → where the kernel reaches egret for engine-token / notebook-runtime callbacks; default `http://127.0.0.1:{server.port}`.
- `egret-bin` derives `catalog_base_url` and `api_url` from the two separate fields. Independently settable for K8s.

### C. Notebook catalog auth via JWT delegation (fixes #3; enforces Approach A) — `egret-kernel`, `egret-notebook`, kernel helpers, `egret-catalog-engine`

- **Token acquisition:** the kernel obtains a per-user JWT `aud=egret-catalog` from the same `EngineTokenMinter` used for engines, via an extended `/kernel/engine-token` (accepting a catalog kind) or a sibling `/kernel/catalog-token`. Fetched lazily and cached like engine tokens; refreshed via refresh-grant on expiry.
- **`catalog()` (kernel helper):** builds the pyiceberg `RestCatalog` with the JWT bearer (front_auth's `looks_like_jwt` → `CatalogJwtAuth` path). No token from the manifest.
- **Authorization (the credential-separation enforcement of Approach A):** `CatalogJwtAuth` / `EgretAuthorizer` grants the JWT subject read-write on the subject's namespace (`notebook_{preferred_username}`) **in the `notebook`-purpose warehouse only**, and **nothing** on any `shared`/`governed` warehouse. A derived rule keyed on `preferred_username`, not a stored grant. This is what makes `iceberg("lakehouse.…")` a hard 403.
- **`provision_notebook_access`:** keeps provisioning the `notebook_{username}` namespace and selecting the notebook warehouse (by purpose, see E), but **drops opaque-token minting and the revoke**. The manifest carries `{catalog_url, warehouse, namespace}` (no token); the kernel supplies the JWT.
- **Opaque token path unchanged** but scoped to admin/warehouse-management. Both paths still share `front_auth`.

### D. Vended STS credentials (fixes #4) — `egret-bin` (warehouse provisioning), `egret-catalog-engine`, kernel helpers

- Provision the `notebook` warehouse's lakekeeper storage profile with **credential vending (STS AssumeRole)** against MinIO/S3 and `remote-signing-enabled=false`.
- `/config` / loadTable then vend short-lived STS credentials scoped to the table prefix; pyiceberg/Daft do direct S3 IO with them. (This only reaches the `notebook` catalog — governed `lakehouse` files are never handed to Daft, per Approach A.)
- **Remove** the signer-specific handling in `front_auth` (the `warehouse_prefix` `signer` branch and the `/aws/s3/sign` read-permission special-case). `warehouse_prefix` reverts to first-segment only.
- The kernel no longer needs raw `AWS_*` creds in its environment. Dev: document enabling MinIO STS.

### E. Explicit warehouse purpose (fixes selection fragility; realizes the two-catalog model) — `egret-metastore` migration, `egret-notebook`, admin API + Catalog UI

- Add `purpose` to `workspace_warehouses` (enum: `shared` | `notebook`; default `shared`). Constraint: **at most one `notebook`-purpose warehouse** per workspace; zero-or-more `shared` (a `shared` warehouse is a governed lakehouse — e.g. named `lakehouse`).
- `build_notebook_runtime` selects the `purpose == notebook` warehouse deterministically; no `notebook` warehouse → a clear "no notebook warehouse provisioned" error (no silent first-fallback). Replaces `select_notebook_warehouse` name matching.
- Admin designates purpose explicitly at warehouse creation (Catalog UI "Add warehouse" gains the purpose choice; the create API accepts it). Migration backfills any existing `name == "notebook"` warehouse to `purpose = notebook`.
- **Both catalogs are engine-registered.** Registering a warehouse as a Trino/StarRocks Iceberg catalog (so BI + cross-user notebook reads work) is an engine-side configuration step; this spec documents the requirement and the naming (`lakehouse`, `notebook`), and the engine connector configuration is an operational task tracked alongside provisioning (not an egret code path in this scope).

### F. Remove legacy catalog (fixes #5) — `egret-catalog`, `egret-iceberg-ext`

- Keep the identity layer of `egret-catalog` (`CatalogStore` tokens + warehouse mapping) used by `front_auth`.
- Remove the dead hand-rolled REST surface (`egret-catalog::api` config/table handlers) and `egret-iceberg-ext` types not referenced by the live path. Verify each symbol with `codegraph_impact` before deleting.
- Document: lakekeeper is the catalog engine; `egret-catalog` is identity/token/warehouse only.

## Data flow (notebook EDA session, after redesign)

1. **Read governed data:** `trino("SELECT … FROM lakehouse.sales")` → the kernel presents its `aud=trino` JWT → Trino + Ranger apply column masks + row filters → a governed Daft DataFrame. (`iceberg("lakehouse.…")` is a 403 — the `aud=egret-catalog` JWT has no grant on `lakehouse`.)
2. **Write EDA output:** `write_iceberg(df, "rev_by_region")` → `catalog()` uses the `aud=egret-catalog` JWT → `GET /catalog/v1/config` returns the correct `base_uri` (no `:8181`) + vended STS creds → create/commit in the user's namespace of the `notebook` warehouse → data files written direct to S3 with the STS creds (no `/signer`).
3. **Share:** another user reads the output via `trino("SELECT … FROM notebook.<user>.rev_by_region")` → engine + Ranger.
4. **Refresh:** JWT/STS expired → 401/auth error → the kernel refreshes via the minter → retries. Reopening a notebook and re-running a cell always works (no mint-and-revoke).

## Error handling

- **Expired JWT / STS creds:** surfaced as 401 / auth error; the kernel refreshes (JWT via minter refresh-grant; STS on the next `/config`/load) and retries. Clear `egret:`-prefixed helper errors on unrecoverable failures.
- **Daft-direct attempt on governed data:** a clean 403 from the catalog with an actionable message ("read governed data via trino()/starrocks()").
- **No notebook-purpose warehouse:** deterministic, actionable error from `build_notebook_runtime` — never a silent write into a shared warehouse.
- **OIDC disabled:** the notebook JWT path is unavailable and the feature reports it clearly; the opaque admin path is unaffected.

## Testing

- **Unit:** config split resolves two independent fields; `with_base_url` makes `RequestMetadata` carry the configured URL (and `base_uri_catalog`/`s3_signer_uri` reflect it); the derived authz rule grants rw on `notebook_{self}` in the notebook warehouse only, and denies any request against a `shared` warehouse (the Approach-A boundary); `build_notebook_runtime` selects by `purpose` (present → picked; absent → error, no first-fallback).
- **Integration (DB-gated):** `front_auth` JWT path end-to-end against the embedded lakekeeper; a notebook JWT is 403'd on a `shared` warehouse and 200 on its own namespace in the `notebook` warehouse; `/config` returns the correct `base_uri` (no `:8181`) and a vended-credentials shape; the signer path is gone.
- **e2e (opt-in, live):** the notebook-dataframe write→read roundtrip via JWT + vended STS creds; **plus a regression guard for #3** — reopen the notebook (force a fresh manifest fetch) and re-run the cell, asserting no 401; **plus an Approach-A guard** — `iceberg("<governed>.t")` returns a clear error while `trino("SELECT … FROM <governed>.t")` succeeds.
- **Removed:** the PR #114 `rewrite_lakekeeper_base` unit tests and the signer-path ownership test.

## Out of scope

- Approach C (catalog-native FGAC) implementation — tracked as a future direction; not built here.
- Configuring the Trino/StarRocks Ranger policies themselves, and the engine-side Iceberg connector registration for the two catalogs (operational; documented, not an egret code path here).
- Governed cross-warehouse reads from Daft-direct (barred by design — engine only).
- Multi-region / multi-cloud storage profiles; schema evolution and partition specs on notebook writes (append/overwrite of flat tables, as today).
- Auto-provisioning the notebook warehouse (admin still creates it; now with an explicit purpose).

## Risks / notes

- **Forking lakekeeper for the base_url setter** must track the pinned rev; a lakekeeper bump re-verifies the patch. Fallback: a single config-driven rewrite layer.
- **Vended STS against MinIO** requires MinIO STS/AssumeRole enabled and the warehouse storage profile configured for vending; verify the dev stack. A store lacking STS falls back to remote-signing for that warehouse (kept as a lakekeeper capability).
- **`preferred_username` stability:** the derived `notebook_{username}` rule keys on the JWT claim; usernames must be stable and collision-free (already assumed by today's `notebook_{username}` namespacing).
- **Engine registration of the notebook catalog** is what makes cross-user output sharing work; if a deployment does not register `notebook` in Trino/StarRocks, sharing is unavailable (Daft-direct stays own-namespace only). Documented as an ops requirement.
- **Sequencing:** one spec, six workstreams. Order: **A** (base_url + remove rewrite) → **B** (config split) → **D** (vended STS + remove signer) → **E** (warehouse purpose) → **C** (notebook → JWT + Approach-A authz, the largest) → **F** (legacy cleanup). A–B–D–E are correctness/cleanup and mostly independent; C depends on B (config) and D (creds model) and carries the governance boundary; F is last.
- **Back-compat:** existing workspaces backfill `purpose` (`name=="notebook"` → notebook, else shared); existing opaque-token admin flows are untouched.
