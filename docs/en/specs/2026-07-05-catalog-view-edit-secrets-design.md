# Engine Catalogs — View / Edit / Secrets + Master-Detail UX — Design

- **Status:** Approved (design)
- **Date:** 2026-07-05
- **Branch:** `feat/engine-config-ui-remap`
- **Area:** `egret-controlplane` (catalog endpoints) + control-plane frontend (`EngineDetail.tsx` Catalogs tab)

## 1. Motivation

The Catalogs tab (newly promoted from Overview) is add/remove-only: you cannot
view an existing catalog's configuration or edit it, and only the built-in
Postgres path can pull a credential from an egret secret. Requirements:

1. **View** an existing catalog's stored config.
2. **Edit** an existing catalog's config (and re-provision).
3. **Use egret secrets** in a catalog's config (any kind, not just the Postgres
   default), so credentials are never entered as plaintext.
4. Redesign the tab to mirror the Configuration page's master-detail UX.

## 2. Grounding: current backend model

- Each catalog row (`EngineCatalog`) stores: `id`, `engine_id`, `name`, `kind`,
  `config: serde_json::Value` (kind-specific fields), `secret_ref: Option<Uuid>`
  (→ one egret secret), `is_primary`.
- `build_catalog_req` (`engines.rs:601`) translates the stored `config` +
  resolved `secret_ref` into flat Trino properties + env at provision time
  (secret → `${ENV:…}`). Postgres uses `secret_ref` for `connection-password`;
  generic currently ignores `secret_ref`.
- Routes today: `GET /engines/{id}/catalogs` (list), `POST` (add),
  `DELETE /…/{cid}` (remove). No detail, no update.
- `st.storage.get_engine_catalog(cid)` **already exists**. `create_engine_catalog`
  exists; there is **no** `update_engine_catalog`.
- Re-provision = `build_provision_req(&st, &engine)` then
  `prov.update(&req)` (rolls pods). Both add + remove already do this.
- **Security:** an `egret-warehouse` catalog's stored `config` contains a minted
  client `credential` (id:secret) and `clientRowId`. These MUST NOT be returned
  by a detail endpoint.

## 3. Design

### 3.1 Backend — new endpoints

**`GET /engines/{id}/catalogs/{cid}`** → `EngineCatalogDetailDto`:
```
{ id, name, kind, isPrimary, config: <redacted>, secretRef: Uuid|null }
```
- `config` is the stored JSON **with sensitive keys stripped** for
  `egret-warehouse` (`credential`, `clientRowId` removed). Postgres/generic
  config carries no plaintext secret (the password lives in `secret_ref`), so it
  is returned as-is.
- Reuses `get_engine_catalog(cid)`; guards cross-engine access (`cat.engine_id
  == engine.id`) like `remove_catalog`.

**`PUT /engines/{id}/catalogs/{cid}`** ← `{ config, secretRef? }` → updated
`EngineCatalogDto`:
- Guards: catalog exists, belongs to this engine, **not primary** (primary is
  managed), kind is **immutable** (taken from the stored row, body `kind`
  ignored), name immutable.
- `egret-warehouse` catalogs are **not editable** via this route (their config is
  system-minted) → `BadRequest`.
- Persists via a new `update_engine_catalog(cat_id, &config, secret_ref)` storage
  method, then re-provisions (`build_provision_req` + `prov.update`). On
  provision failure, **roll back** to the previous `config`/`secret_ref` (fetch
  before update; restore on error) so a bad edit never leaves the row ahead of
  the running cluster.

**`list`/detail DTO:** add `secretRef` to the list DTO too (cheap; lets the left
nav show a "secret" indicator without a per-row fetch). `config` stays
detail-only.

### 3.2 Backend — generic connector secret

Generalize `build_catalog_req`'s `generic` arm so a `secret_ref` can back one
property (parity with Postgres):
- Store the target property name in `config.secretProperty` (default
  `connection-password`).
- When `secret_ref` is set: emit `props[secretProperty] = "${ENV:GEN_{prefix}}"`
  and inject the resolved secret into `env["GEN_{prefix}"]`. When unset, generic
  behaves exactly as today.
- The plain `properties` map still renders verbatim; `secretProperty` is stripped
  from the verbatim pass so it isn't double-emitted.

### 3.3 Storage

Add to the storage trait + impl:
`update_engine_catalog(id: Uuid, config: &serde_json::Value, secret_ref: Option<Uuid>) -> Result<EngineCatalog>`
— an `UPDATE engine_catalogs SET config = $2, secret_ref = $3 WHERE id = $1
RETURNING …`. No migration (columns already exist).

### 3.4 Frontend — master-detail Catalogs tab

Replace `CatalogsCard`'s table + modal with a Config-style master-detail:

- **Left nav** (`md:w-56`): one button per catalog (name · kind sub-label ·
  `primary`/`secret` badges), styled like the Configuration `NAV_ITEMS`; a
  divider; then a highlighted **"+ Add catalog"** entry. Selection state drives
  the right pane. Default selection: first catalog, else "add".
- **Right pane:**
  - *Existing catalog:* a `CatalogForm` prefilled from `GET …/{cid}`, editable,
    with a **Save changes** button (PUT) + a **Remove** action (`ConfirmDelete`),
    both hidden/disabled for `primary` and `egret-warehouse` (read-only summary
    for those). tpch/tpcds show a "no configurable properties" note.
  - *Add:* the same `CatalogForm` blank, with **Add catalog** (POST).
- **`CatalogForm`** (extracted from the current `AddCatalogDialog` body): Name +
  Kind (add only; read-only on edit), per-kind fields (egret-warehouse warehouse
  picker; postgres host/port/db/user; generic connector combobox + the Phase-2
  `KeyedPropertyList` property picker), and a **Secret** block: an egret-secret
  `Select` (from `listSecrets`) + the property it fills (postgres:
  `connection-password` fixed; generic: a `secretProperty` input defaulting to
  `connection-password`). The secret value is never shown — only its egret
  secret name.
- The old `AddCatalogDialog` modal is retired; its logic moves into `CatalogForm`
  + the master-detail container.

### 3.5 API client (`controlPlane.ts`)

- `EngineCatalog` gains `secretRef?: string | null`.
- New `EngineCatalogDetail = EngineCatalog & { config: Record<string, unknown> }`.
- `getEngineCatalog(id, cid): Promise<EngineCatalogDetail>`.
- `updateEngineCatalog(id, cid, { config, secretRef }): Promise<EngineCatalog>`.

## 4. Non-goals

- Kind and name are immutable after creation (edit changes config + secret only).
- `egret-warehouse` / primary catalogs remain system-managed (read-only detail).
- No multi-secret-per-catalog (one `secret_ref`, one designated property) — the
  common credential case; multiple secrets is future work.
- No new DB migration (reuses existing `config`/`secret_ref` columns).

## 5. Risks

- **Credential leakage:** the detail endpoint must redact egret-warehouse
  `credential`/`clientRowId`. Covered by a redaction step + a test asserting the
  keys are absent.
- **Edit rollback:** a failed re-provision after `update_engine_catalog` must
  restore the prior `config`/`secret_ref` (fetch-before-update) so the row never
  diverges from the running cluster.
- **Payload/behavior parity:** add + generic-without-secret must render
  byte-identically to today (verified by the existing provision path + a
  build_catalog_req test for the generic-with-secret branch).

## 6. Acceptance

- View: opening an existing Postgres/generic catalog shows its stored fields;
  egret-warehouse shows a redacted read-only summary (no credential).
- Edit: changing a Postgres host (or generic property/secret) + Save re-renders
  the catalog and rolls the pods; a forced provision failure leaves the stored
  config unchanged.
- Secret: a generic catalog can set `connection-password` from an egret secret;
  the rendered catalog uses `${ENV:…}` and the secret is resolved into env.
- UX: Catalogs tab is master-detail (left nav + right form), mirroring Config;
  primary/egret-warehouse are read-only.
- Full backend fmt/clippy/test + frontend lint/typecheck/test/build green; live
  smoke on `demo2`.
