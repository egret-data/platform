# EB-3: data model + egret Authorizer + JWT authN Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.
> Spec: `docs/en/specs/2026-07-02-egret-embedded-catalog-design.md` (Component 3 + data model). **EB-3** — the integration core. Depends on EB-1 (embedded lakekeeper + `Authorizer` seam) + EB-2 (mini-IdP + `ClientVerifier`). VI: `docs/vi/plans/2026-07-02-eb3-store-authorizer-authn.md`.

**Goal:** egret's own RBAC store (`warehouses`, `catalog_roles`, `catalog_grants`, `catalog_clients`), the `CatalogClientStore` that verifies clients (implements EB-2's `ClientVerifier`), the egret **Authorizer** plugged into lakekeeper (deny-by-default; roles→grants), and wiring the embedded lakekeeper to **authenticate egret-issued JWTs** (the integration spike).

**Architecture:** `catalog_clients` binds a machine credential to a `(warehouse, catalog_role)`; `/oauth/token` (EB-2) then stamps `warehouse` + `roles:[catalog_role]` into the JWT. On each catalog op, lakekeeper validates the JWT (egret JWKS) and calls egret's Authorizer, which resolves the role's grants on the target securable → allow/deny.

**Tech Stack:** Rust (sqlx, argon2, jsonwebtoken, lakekeeper `Authorizer`), Postgres.

---

## Background the implementer needs

- EB-1 recovered lakekeeper's `Authorizer` trait (the seam the old `EgretAuthorizer` used) — study it via `git show 26a13f4~1:crates/egret-catalog-engine/src/…` for the old `EgretAuthorizer` as a reference for the trait shape + how the principal + resource reach it.
- EB-2 defined `ClientVerifier` (`verify(client_id,secret) -> Option<VerifiedClient{client_id,warehouse,catalog_role}>`) and mounted the IdP with a placeholder verifier. This plan provides the real one.
- Migration `0052` (EB-2) added `oidc_signing_keys`. This plan adds `0053`.
- `storage_connections` (P2) is kept — a warehouse references one for its S3 location.

---

## Task 1: Migration — warehouses / roles / grants / clients

**Files:** Create `migrations/0053_embedded_catalog_rbac.sql`; touch `crates/egret-metastore/src/lib.rs`

- [ ] **Step 1: Write + apply**

```sql
CREATE TABLE warehouses (
    id                    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name                  text NOT NULL,
    workspace_id          uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    storage_conn_id       uuid NOT NULL REFERENCES storage_connections(id),
    lakekeeper_warehouse_id text,                          -- lakekeeper's own id, set on create
    created_at            timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);
CREATE TABLE catalog_roles (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    warehouse_id  uuid NOT NULL REFERENCES warehouses(id) ON DELETE CASCADE,
    name          text NOT NULL,
    consumer_type text NOT NULL CHECK (consumer_type IN ('engine','notebook','external')),
    UNIQUE (warehouse_id, name)
);
CREATE TABLE catalog_grants (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    catalog_role_id uuid NOT NULL REFERENCES catalog_roles(id) ON DELETE CASCADE,
    securable_type  text NOT NULL CHECK (securable_type IN ('catalog','namespace','table')),
    namespace       text[],
    table_name      text,
    privilege       text NOT NULL
);
CREATE TABLE catalog_clients (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name               text NOT NULL,
    consumer_type      text NOT NULL CHECK (consumer_type IN ('engine','notebook','external')),
    workspace_id       uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    warehouse_id       uuid NOT NULL REFERENCES warehouses(id) ON DELETE CASCADE,
    catalog_role_id    uuid NOT NULL REFERENCES catalog_roles(id),
    client_id          text NOT NULL UNIQUE,
    client_secret_hash text NOT NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    last_used_at       timestamptz
);
CREATE INDEX catalog_clients_client_id_idx ON catalog_clients(client_id);
```
`touch … && cargo run -q -p egret-bin -- migrate`; verify all four `to_regclass` non-null.

- [ ] **Step 2: Commit** — `feat(db): warehouses + catalog_roles/grants/clients (embedded catalog RBAC)`.

---

## Task 2: Stores (warehouse / role / grant / client)

**Files:** Create `crates/egret-controlplane/src/embedded_catalog/{mod.rs,warehouse.rs,rbac.rs,clients.rs}`

- [ ] **Step 1: Failing test — create client → verify (implements ClientVerifier)**

```rust
#[tokio::test]
async fn create_client_then_verify() {
    // seed a workspace + warehouse + role "engine"(consumer_type engine)
    let created = clients.create(NewClient{ name:"trino".into(), consumer_type:"engine".into(),
        workspace_id: ws, warehouse_id: wh, catalog_role_id: role }).await.unwrap();
    assert!(!created.client_secret.is_empty());  // once
    let vc = clients.verify(&created.client_id, &created.client_secret).await.unwrap().unwrap();
    assert_eq!((vc.warehouse.as_str(), vc.catalog_role.as_str()), ("<wh name>","engine"));
    assert!(clients.verify(&created.client_id, "bad").await.unwrap().is_none());
}
```

- [ ] **Step 2: Implement**

- `WarehouseStore`: `create(name, ws, storage_conn_id) -> Warehouse` (also creates the lakekeeper warehouse via the embedded engine API + stores `lakekeeper_warehouse_id`; see EB-1's `build_state`), `list_for_workspace`, `get`, `delete`.
- `RoleStore`: `create_role(wh, name, consumer_type)`, `list_roles(wh)`; `add_grant(role_id, GrantInput)`, `list_grants(role_id)`, `revoke_grant`.
- `CatalogClientStore`: `create(NewClient) -> CreatedClient{client_id, client_secret /* once */}` (gen `client_id`=`egret_`+16hex, secret=32B base64url, argon2-hash, INSERT); `verify(client_id, secret) -> Option<VerifiedClient>` (argon2 constant-time; join `catalog_roles`+`warehouses` for the role name + warehouse name; update `last_used_at`); `list_for_warehouse`, `revoke`. Implement `egret_machine_idp::ClientVerifier for CatalogClientStore`.

- [ ] **Step 3: Run + commit** — store test PASS.

---

## Task 3: egret Authorizer (deny-by-default)

**Files:** Create `crates/egret-catalog-engine/src/egret_authorizer.rs` (or in egret-controlplane if the trait allows) — implements lakekeeper's `Authorizer`

- [ ] **Step 1: Failing test — role grants gate the action**

```rust
// Given a warehouse "eng" with role "engine" granted TABLE write on namespace ["marts"],
// a principal carrying roles=["engine"], warehouse="eng":
//  - authorize(write, table marts.sales) -> ALLOW
//  - authorize(write, table other.x)     -> DENY  (no grant)
//  - a principal with an unknown role     -> DENY  (deny-by-default)
```
(Test the pure decision fn `decide(grants: &[Grant], action, securable) -> bool` directly, independent of lakekeeper's trait plumbing.)

- [ ] **Step 2: Implement**

A pure `decide(role_grants, action, securable) -> bool` (deny-by-default; map lakekeeper actions → `privilege`/`securable_type`; a grant on `catalog` covers all namespaces/tables in the warehouse, `namespace` covers its tables, `table` is exact). Then `EgretAuthorizer` implementing lakekeeper's `Authorizer` trait: extract the principal's `roles`+`warehouse` (from the validated JWT claims lakekeeper passes through), load that role's grants (via a store handle), and return `decide(...)`. Load grants with a short cache if the trait is hot-path.

- [ ] **Step 3: Run + commit** — decision test PASS.

---

## Task 4: Wire lakekeeper authN to egret's JWT (the integration spike)

**Files:** `crates/egret-catalog-engine/src/*`, `crates/egret-bin/src/main.rs`

- [ ] **Step 1: Spike — how the embedded lakekeeper trusts egret's JWT**

Determine, against the pinned lakekeeper revision, the way to (a) validate an egret-signed JWT and (b) surface its `sub`/`roles`/`warehouse` to the `Authorizer`. Options, in preference order — pick what the pinned lakekeeper supports:
1. **Direct verification key (in-process):** configure lakekeeper's OIDC/authenticator with egret's public key (from `PgSigningKeyStore.active()`), no HTTP JWKS round-trip.
2. **OIDC via egret JWKS URL:** point lakekeeper's OIDC `auth-server-url`/`jwks_uri` at egret's own `/.well-known/*` (localhost), `audience=catalog`.
3. **Custom authenticator:** if lakekeeper exposes an authentication hook, validate the JWT in egret (`egret_machine_idp::token::verify_with_jwk`) and inject the principal.
Document the working approach in `crates/egret-catalog-engine/README-auth.md`.

- [ ] **Step 2: Swap in the real authorizer + verifier**

Replace EB-1's `AllowAllAuthorizer` with `EgretAuthorizer` in `mount_catalog_engine`. In `main.rs`, replace EB-2's placeholder `ClientVerifier` with `CatalogClientStore`. Ensure the `issuer`/`aud` egret stamps matches what lakekeeper validates.

- [ ] **Step 3: Build + commit** — `cargo build --workspace` 0 errors; `feat(catalog): egret Authorizer + JWT authN for embedded lakekeeper`.

---

## Task 5: Verify (in-process authN+authZ)

- [ ] **Step 1:** With serve up: create a warehouse + a role `engine` (grant table write on a namespace) + a Catalog Client → `POST /oauth/token` → JWT; call `/catalog/v1/...` (e.g. create a namespace/table) with the Bearer → **allowed**; a client whose role lacks the grant → **403** from the Authorizer.
- [ ] **Step 2: CI** — fmt + clippy + `DATABASE_URL=… cargo test -p egret-controlplane -p egret-catalog-engine`.

---

## Self-review notes (spec coverage for EB-3)

- Spec **Component 3 + data model**: warehouses/roles/grants/clients (Task 1/2), `CatalogClientStore` implements `ClientVerifier` (Task 2), egret `Authorizer` deny-by-default roles→grants (Task 3), embedded lakekeeper authN of egret JWTs (Task 4 spike). Table-level; no OpenFGA.
- **Least-pinned (flagged):** the lakekeeper auth-integration (Task 4) — resolved in the spike + documented.
- **Out of EB-3:** control-plane mgmt routes + egret-polaris revert (EB-4), UI (EB-5), e2e (EB-6).
