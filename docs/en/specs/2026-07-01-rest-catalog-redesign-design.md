# REST Catalog Redesign — Design

> Bilingual: English canonical; VI reference at `docs/vi/specs/2026-07-01-rest-catalog-redesign-design.md`.

- **Date:** 2026-07-01
- **Status:** Approved (brainstorming) — ready for implementation plan
- **Scope:** Redesign egret's Iceberg REST catalog integration to fix, at the root, the five problems surfaced while making notebook Iceberg work live (PR #114 was a stop-gap). Unify notebook catalog auth on OIDC-JWT delegation, adopt vended STS credentials, split the overloaded config, fix the embedded-lakekeeper base URL at its source, make warehouse selection explicit, and remove the legacy hand-rolled catalog.

## Background / current state

egret embeds **lakekeeper** (git-pinned `0.12.3`, rev `79165057`) in-process and mounts it as the live Iceberg REST catalog at **`/catalog/v1`** (`AppState.catalog_engine`, assembled in `egret-bin`, fronted by the `egret-catalog-engine::front_auth` per-workspace token-ownership middleware). A session-authed browse API (`AppState.catalog_browse`) reads the same lakekeeper for the UI. `egret-catalog` provides the **identity layer** — `CatalogStore` (opaque `catalog_tokens` + `workspace_warehouses` → lakekeeper-warehouse-UUID mapping). An earlier **hand-rolled** Iceberg REST surface (`egret-catalog::api`, `egret-iceberg-ext` types) predates the lakekeeper embed and is now largely superseded.

The notebook uses a **two-catalog** model on top of one lakekeeper: BI engines (Trino/StarRocks) query "shared" warehouses; the notebook gets its own read-write catalog **B** = the warehouse currently selected by the name convention `name == "notebook"` (fallback: first warehouse). `build_notebook_runtime` picks it, `provision_notebook_access` provisions the private namespace `notebook_{username}` + a **1-grant opaque rw token** (mint-and-revoke: at most one active per user×workspace), and returns a `RuntimeCatalog {url, warehouse, namespace, token}` carried in the kernel manifest. `catalog()`/`iceberg()`/`write_iceberg()` consume it.

### The five problems (all root-caused here)

1. **Internal base URL leaks (`:8181`).** lakekeeper reports its hardcoded internal bind address in self-referential fields — `/config` `overrides.uri` and per-table `s3.signer.uri` — because `RequestMetadata.base_url` defaults to `http://localhost:8181` (`new_lakekeeper_internal`) with no public setter at this rev, and egret injects that pre-built metadata (bypassing lakekeeper's header/`CONFIG.base_uri` determination). Clients follow those URLs and cannot reach `:8181` (lakekeeper is only exposed via the `/catalog` proxy). PR #114 works around this by rewriting response bodies — a band-aid.
2. **`config.catalog.public_url` is overloaded.** It feeds *both* the catalog base URL *and* the kernel callback `api_url` (`egret-bin`). Setting it correctly for the catalog points `api_url` at `.../catalog`, which breaks the governed engine-token / Trino / StarRocks kernel callbacks.
3. **Mint-and-revoke token lifecycle → flaky 401.** The notebook's opaque catalog token is minted per manifest fetch and revokes prior tokens (one active per user×workspace). A running kernel holds a token that any later manifest fetch (reopening another notebook) revokes → re-running a cell after reopening 401s.
4. **Remote S3 signing is unnecessary overhead.** Reads route through lakekeeper's `/signer/{warehouse}/…/aws/s3/sign` path (an extra round-trip per file op, and the source of a front_auth ownership bug fixed in #114), even though the kernel could access storage directly.
5. **Legacy dual catalog code.** The hand-rolled `egret-catalog::api` + unused `egret-iceberg-ext` REST types coexist with lakekeeper, making it look like there are "two catalogs" and inviting confusion.

Plus a selection-fragility risk: catalog B is chosen by **name convention** (`"notebook"`) + silent fallback-to-first — implicit and error-prone.

## Decisions (from brainstorming)

1. **Keep lakekeeper embedded; fix the integration at the root.** egret is deliberately all-in-one (one process embeds auth, workspace, engine, BI, notebook, kernel, catalog); a separate lakekeeper service would add ops burden, version-skew, and lose in-process principal injection. A native rewrite reverses the migration egret already made and re-introduces spec-compliance risk. Neither is justified.
2. **Vended STS credentials** for kernel data-plane access (scoped per-table AssumeRole), not remote signing and not raw bucket credentials. The kernel never sees root creds; the `/signer` path is removed. MinIO supports STS.
3. **Notebook catalog auth = per-user OIDC-JWT delegation** (`aud=egret-catalog`), the same model Trino/StarRocks use, minted by the same `EngineTokenMinter` (RFC 8693 token-exchange) and refreshed the same way. The opaque catalog token is **retained only** for admin / warehouse-management / non-OIDC use.
4. **Explicit warehouse purpose.** Multiple warehouses per workspace stays (standard, valuable for storage/governance isolation), but selection moves from name-convention to an explicit `purpose` designation.
5. **base_url fixed at the source** by adding a small `base_url` setter to egret's pinned lakekeeper fork (egret already git-pins and patches dependencies — the `mysql_async` precedent) and setting it from config; the PR #114 response-rewrite is removed. *Fallback if forking is rejected: keep a single clean, config-driven rewrite layer.*

## Architecture / components

### A. base_url at the source (fixes #1) — `egret-catalog-engine`, lakekeeper fork

- Add `pub fn with_base_url(mut self, base_url: String) -> Self` (and/or a `set_base_url`) to `RequestMetadata` in egret's **pinned lakekeeper fork**. Minimal, explicit, unit-testable.
- Set `LAKEKEEPER__BASE_URI` (and egret's own `config.catalog.public_url`) at boot from one source of truth.
- In `front_auth.rs`, the three `RequestMetadata` construction sites (`token_request_metadata`, the system-principal builder, the browse builder) set `base_url` = the configured public catalog URL instead of relying on the `:8181` default.
- **Remove** `rewrite_lakekeeper_base` + the response-rewrite block + the JSON-content buffering added in PR #114. `overrides.uri` and `s3.signer.uri` are then correct at the source.
- `FrontAuthState.catalog_public_url` stays (now used to set metadata base_url, not to rewrite bodies).

### B. Config split (fixes #2) — `egret-config`, `egret-bin`

- `config.catalog.public_url` → the external catalog URL only (lakekeeper `base_uri` + the notebook manifest catalog URL).
- New `config.server.public_url` (kernel callback base / `api_url`) → where the kernel reaches egret for engine-token / notebook-runtime callbacks; default `http://127.0.0.1:{server.port}`.
- `egret-bin` derives `catalog_base_url` from the former and `api_url` from the latter — they no longer share a field. Both independently settable for K8s (catalog behind ingress path; api_url on a service DNS).

### C. Notebook catalog auth via JWT delegation (fixes #3) — `egret-kernel`, `egret-notebook`, kernel helpers, `egret-catalog-engine`

- **Token acquisition:** the kernel obtains a per-user JWT with `aud=egret-catalog` from the same `EngineTokenMinter` used for engines, via an extended `/kernel/engine-token` (accepting a catalog "engine" kind) or a sibling `/kernel/catalog-token`. Fetched lazily and cached in the kernel like engine tokens, refreshed via the refresh-grant on expiry.
- **`catalog()` (kernel helper):** builds the pyiceberg `RestCatalog` with the JWT as bearer (front_auth's existing `looks_like_jwt` → `CatalogJwtAuth` path authenticates it). No token comes from the manifest.
- **Private rw authorization:** `CatalogJwtAuth` / `EgretAuthorizer` grants the JWT subject read-write on the namespace `notebook_{preferred_username}` in the **notebook-purpose** warehouse — a *derived rule* keyed on the JWT's `preferred_username`, not a stored grant. It grants nothing on shared/governed data (those remain Trino's plane). This preserves the two-plane access model.
- **`provision_notebook_access`:** keeps provisioning the `notebook_{username}` namespace and selecting warehouse B, but **drops opaque-token minting and the revoke**. The manifest carries `{catalog_url, warehouse, namespace}` (no token); the kernel supplies the JWT itself.
- **Opaque token path unchanged** but scoped to admin/warehouse-management (`catalog mint-token` CLI, external tools, non-OIDC). Both paths continue to share `front_auth`.

### D. Vended STS credentials (fixes #4) — `egret-bin` (warehouse provisioning), `egret-catalog-engine`, kernel helpers

- Provision the notebook-purpose warehouse's lakekeeper storage profile with **credential vending (STS AssumeRole)** against MinIO/S3 and `remote-signing-enabled=false`.
- `/config` / loadTable then vend short-lived STS credentials scoped to the table prefix; pyiceberg/Daft do direct S3 IO with them.
- **Remove** the signer-specific handling in `front_auth` (the `warehouse_prefix` `signer` branch and the `/aws/s3/sign` read-permission special-case) — the path is no longer exercised. `warehouse_prefix` reverts to first-segment only.
- Dev: document enabling MinIO STS; the kernel no longer needs raw `AWS_*` creds in its environment.

### E. Explicit warehouse purpose (fixes selection fragility) — `egret-metastore` migration, `egret-notebook`, `egret-catalog`/admin API + UI

- Add `purpose` to `workspace_warehouses` (enum: `shared` | `notebook`; default `shared`). Constraint: at most one `notebook`-purpose warehouse per workspace; zero-or-more `shared`.
- `build_notebook_runtime` selects the `purpose == notebook` warehouse (deterministic); no `notebook` → a clear "no notebook warehouse provisioned" error (no silent first-fallback). Replaces `select_notebook_warehouse` name matching.
- Admin designates purpose explicitly when creating a warehouse (Catalog UI "Add warehouse" gains a purpose choice; the create API accepts it). Migration backfills any existing `name == "notebook"` warehouse to `purpose = notebook`.

### F. Remove legacy catalog (fixes #5) — `egret-catalog`, `egret-iceberg-ext`

- Keep the identity layer of `egret-catalog` (`CatalogStore` tokens + warehouse mapping) used by `front_auth`.
- Remove the dead hand-rolled REST surface (`egret-catalog::api` config/table handlers) and `egret-iceberg-ext` types not referenced by the live path. Verify with `codegraph_impact` before deleting each symbol.
- Document: lakekeeper is the catalog engine; `egret-catalog` is identity/token/warehouse only.

## Data flow (notebook write→read, after redesign)

1. `write_iceberg(df, "t")` → `catalog()` fetches the per-user `aud=egret-catalog` JWT from the minter (cached; refreshed on expiry).
2. pyiceberg `RestCatalog` → `GET /catalog/v1/config` (JWT) → `front_auth` JWT path authenticates → lakekeeper returns config with a **correct `base_uri`** (no `:8181`) and **vended STS credentials**.
3. Table create/commit → `/catalog/v1/{warehouse}/…` (JWT) → `front_auth` authz: subject owns `notebook_{user}` rw → lakekeeper commits.
4. Data files → **direct S3** with the vended STS creds (no `/signer`).
5. JWT expired → 401 → the kernel refreshes via the minter → retries. No permanent 401; reopening a notebook and re-running a cell always works.

## Error handling

- **Expired JWT / STS creds:** surfaced as 401 / auth error; the kernel refreshes (JWT via minter refresh-grant; STS on the next `/config`/load) and retries. Clear `egret:`-prefixed helper errors on unrecoverable failures.
- **No notebook-purpose warehouse:** deterministic, actionable error from `build_notebook_runtime` ("this workspace has no notebook warehouse; an admin must create one") — never a silent write into a shared warehouse.
- **OIDC disabled:** the notebook JWT path is unavailable; the notebook feature reports it clearly. The opaque admin path is unaffected.

## Testing

- **Unit:** config split resolves two independent fields; `with_base_url` makes `RequestMetadata` carry the configured URL (and `base_uri_catalog`/`s3_signer_uri` reflect it); the derived authz rule grants rw on `notebook_{self}` only (not another user's namespace, not shared); `build_notebook_runtime` selects by `purpose` (notebook present → picked; absent → error, no first-fallback).
- **Integration (DB-gated):** `front_auth` JWT path end-to-end against the embedded lakekeeper; `/config` returns the correct `base_uri` (no `:8181`) and a vended-credentials shape; the signer path is gone.
- **e2e (opt-in, live):** the notebook-dataframe write→read roundtrip now via JWT + vended STS creds; **plus a regression guard for #3** — reopen the notebook (force a fresh manifest fetch) and re-run the cell, asserting no 401.
- **Removed:** the PR #114 `rewrite_lakekeeper_base` unit tests and the signer-path ownership test.

## Out of scope

- Migrating Trino/StarRocks off their current connector catalog configuration (unchanged; they keep querying shared warehouses).
- Governed cross-warehouse reads from the notebook (still Trino-only, two-plane model preserved).
- Multi-region / multi-cloud storage profiles; schema evolution and partition specs on notebook writes (append/overwrite of flat tables, as today).
- Auto-provisioning the notebook warehouse (admin still creates it; now with an explicit purpose).

## Risks / notes

- **Forking lakekeeper for the base_url setter** must track the pinned rev; a lakekeeper bump re-verifies the patch. Fallback: a single config-driven rewrite layer (documented, not the default).
- **Vended STS against MinIO** requires MinIO STS/AssumeRole enabled and the warehouse storage profile configured for vending; verify the dev stack. If a target store lacks STS, remote-signing (kept as a lakekeeper capability) is the fallback for that warehouse.
- **`preferred_username` stability:** the derived `notebook_{username}` rule keys on the JWT claim; usernames must be stable and collision-free (already assumed by `notebook_{username}` namespacing today).
- **Sequencing:** one spec, six workstreams. Suggested order: **A** (base_url + remove rewrite) → **B** (config split) → **D** (vended STS + remove signer) → **E** (warehouse purpose) → **C** (notebook → JWT, the largest) → **F** (legacy cleanup). A–B–D–E are correctness/cleanup and mostly independent; C depends on B (config) and D (creds model); F is last.
- **Back-compat:** existing workspaces without a `purpose` set are backfilled (`name=="notebook"` → notebook, else shared). Existing opaque-token admin flows are untouched.
