# Service accounts SA2 — Unified credentials — design

> Second package of the "service accounts as principals" programme. Builds on
> SA1 (the `service_accounts` principal table, PR #867). Extends the authoritative
> programme design `2026-11-15-service-accounts-as-principals-design.md`.

## Background (from SA1 + current code)

- **SA1 shipped**: a service account is a `users` row (`provider='service'`) owned
  by `service_accounts` (scope platform|workspace). `dataapi.service_accounts` is a
  view over it; `dataapi.clients.service_account_id` FKs `service_accounts(user_id)`.
- **Two credential audiences exist, parallel-by-design:**
  - **Data-API** (`dataapi.clients`): already SA-owned (`service_account_id`),
    has `revoked_at` + `expires_at`, rotate = mint-new-then-revoke-old. Token ttl 600s.
    Verified by a `ClientVerifier` impl (`ClientStore` in `egret-dataapi/src/clients.rs`)
    that already refuses on `users.status`, `revoked_at`, `expires_at`, workspace status.
  - **Catalog** (`catalog.catalog_clients`, 0053→schema `catalog` 0065): **NOT**
    owned by a principal, authorised per-warehouse by `catalog_roles`/`catalog_grants`,
    reads Iceberg directly through lakekeeper. Columns: id, name, consumer_type
    (`engine`|`external`, 0169), workspace_id, warehouse_id, catalog_role_id,
    client_id, client_secret_hash, created_at, last_used_at. **No `revoked_at`/
    `expires_at`.** Verified by a second `ClientVerifier` impl in
    `egret-catalog-engine/src/store/embedded.rs` that today does **not** consult any
    user/SA status. Token ttl 3600s.
- **machine-idp** (`egret-machine-idp`): one provider, one `ClientVerifier` trait
  (`verify`/`current` → `ClientState`), two impls (one per audience). Catalog ttl
  3600, Data-API 600 (both stateless — a token cannot be revoked before it expires).

## Goal

A catalog client becomes a **credential of a service account**, so one SA holds both
Data-API and Catalog credentials; the SA kill switch (`users.status`) reaches the
Catalog audience; the credential lifecycle (rotate/expire/disable) is uniform; and
credentials are listed/managed under the SA. Backend only (the frontend tab is SA3).

## Decisions (fixed in brainstorming 2026-11-15)

- **Unification shape: two audience tables + a repo/API unifying layer.** Keep
  `dataapi.clients` and `catalog.catalog_clients` as separate tables, each gaining
  `service_account_id` + lifecycle columns; a `Credential` abstraction unifies them
  only at the repo/API layer. Catalog-specific columns (warehouse_id, catalog_role_id,
  consumer_type) do not belong on Data-API rows, and the machine-idp already treats
  the two as parallel-by-audience.
- **Backfill: one synthesized workspace-scoped SA per existing catalog client**, so
  every existing catalog client is owned and reached by the kill switch. No fail-open
  window (see §1).
- **SA2 scope: ownership + kill switch + lifecycle parity + credential management
  under the SA (backend).** Catalog credential creation still carries warehouse +
  catalog_role. Full consolidation of the creation UIs → SA3/later.

## 1. Schema + backfill (`catalog.catalog_clients`)

Add to `catalog.catalog_clients`:
- `service_account_id uuid NOT NULL REFERENCES service_accounts(user_id) ON DELETE CASCADE`
- `revoked_at timestamptz`
- `expires_at timestamptz`

**Backfill, no fail-open window.** Every existing `catalog.catalog_clients` row must
be owned the moment `service_account_id` becomes NOT NULL. For each existing client,
synthesize one **workspace-scoped** SA whose `workspace_id` is the client's
`workspace_id`, with a **deterministic, collision-free identity keyed off the client's
id** so neither the `users.email` (`citext UNIQUE`) nor the `svc-` username collides
with an existing SA:
- email `svc-catalog-{client_id}@service.egret.local` (or the platform SA email
  scheme from SA1), username `svc-catalog-{client_id}`, name e.g.
  `catalog:{client.name}`.

**Open implementation decision for the plan (surfaced, not placeholder):** the `svc-`
username is normally allocated through the username-takeover ledger
(`ensure_username`). A pure-SQL migration that inserts `users` rows directly bypasses
that. Two viable mechanisms, the plan chooses one with the rationale written down:
- **(a) SQL backfill in the migration** — deterministic UUID-based username/email +
  an explicit insert into the username ledger table for each, so no fail-open window
  and the ledger stays consistent. Requires knowing the ledger table's exact shape.
- **(b) In-migration column as NULLABLE + a one-shot Rust backfill** that reuses
  `ServiceAccountRepo`-style creation (ledger-correct), then a follow-up migration
  sets NOT NULL. Ledger-correct but introduces a NULL window — which the catalog
  verifier must treat as **fail-closed** (deny a NULL-SA client) to avoid fail-open,
  accepting that catalog access is denied until the backfill runs.
  Recommended: **(a)** if the ledger insert is simple, else **(b)** with fail-closed.
  The plan verifies the ledger table and picks.

## 2. Kill switch — catalog `ClientVerifier` gates on the SA

The catalog verifier in `egret-catalog-engine/src/store/embedded.rs` must join
`catalog_clients → service_accounts → users` and **refuse** (return the same
"no such client" `Ok(None)` the dataapi `ClientStore::verify` returns for every
failure, so it is indistinguishable) when:
- `users.status <> 'active'` (the SA is disabled), OR
- `catalog_clients.revoked_at IS NOT NULL`, OR
- `catalog_clients.expires_at IS NOT NULL AND expires_at < now()`.

Mirror the refusal structure of `ClientStore::verify` (egret-dataapi) exactly,
including the `verified_cache` interaction (a cache hit skips the DB read + the
`last_used_at` update — confirm a disabled SA is not served stale from the cache;
the cache revalidates against the registry hash, but a status change may need the
cache keyed or invalidated — the plan checks `verified_cache.rs`).

**Stateless-token window:** disabling an SA stops *minting* new catalog tokens; a
token already issued lives up to its 3600s ttl. This is inherent (same as Data-API's
600s) and is documented, not fixed, in this package.

## 3. Lifecycle parity + `Credential` repo

- Catalog gains `revoked_at`/`expires_at` (§1). Rotate = mint-new-then-revoke-old
  (set `revoked_at`), matching Data-API. Verify refuses revoked/expired (§2).
- A `Credential` repo abstraction (likely in a shared crate both audiences depend on,
  or a thin trait) presents a uniform view/row:
  `Credential { id, service_account_id, audience: 'data-api'|'catalog', name,
  status: active|revoked|expired, created_at, last_used_at, expires_at,
  catalog: Option<{ warehouse_id, catalog_role_id, consumer_type }> }`.
  It reads/writes both `dataapi.clients` and `catalog.catalog_clients`; audience-
  specific fields are populated only for catalog rows.
- Operations unified: `rotate`, `revoke`, `set_expiry` behave identically across
  audiences (each dispatches to the right table).

## 4. SA-scoped credential API (control-plane)

Under the SA, mounted beside the SA1 routes:
- `GET /api/control-plane/service-accounts/{id}/credentials` — list this SA's
  credentials across both audiences, with status and audience-specific detail
  (catalog: warehouse + role). Perm: `user:read`.
- `POST …/{id}/credentials` — create. Body carries `audience`; for `catalog`,
  `{ warehouse_id, catalog_role_id, consumer_type }`; for `data-api`, nothing extra.
  Returns the one-time secret. Perm: `user:update` (or the credential-mint perm the
  existing routes use — the plan confirms).
- `POST …/credentials/{cred_id}/rotate`, `POST …/credentials/{cred_id}/revoke`,
  `POST …/credentials/{cred_id}/expiry` — lifecycle. Perm: `user:update`.
- The existing standalone creation routes (Data-API client routes; the control-plane
  catalog client route) keep working but attach the owning SA. Full consolidation of
  the creation surfaces is SA3/later; SA2 adds the unified management surface and the
  owner.

Guard: a credential's workspace must be consistent with its SA — a workspace-scoped
SA may only own credentials in its workspace; a platform SA may own catalog
credentials across workspaces (the credential names its own warehouse/workspace).
The plan states the exact rule and tests it.

## 5. OpenAPI + client regen

Regenerate `openapi.json` + the generated TS client + digest for the new credential
routes; CI drift tests must pass. (No frontend UI — that's SA3.)

## Decomposition (plan tasks, each its own review)

- **T1** — migration: add `service_account_id` + `revoked_at` + `expires_at` to
  `catalog.catalog_clients`; backfill one SA per existing client (mechanism per §1);
  DB test the backfill + NOT NULL.
- **T2** — catalog `ClientVerifier` gates on SA status + revoked/expired (the kill
  switch), mirroring `ClientStore::verify`; DB test disable/revoke/expire deny.
- **T3** — `Credential` repo + lifecycle (rotate/revoke/set_expiry) across both
  tables; DB tests.
- **T4** — SA-scoped credential API (list/create/rotate/revoke/expiry) + the
  owner-on-creation wiring; DB tests incl. the workspace-consistency guard.
- **T5** — OpenAPI + client regen; full gate.

## Global constraints

Same as SA1: chat VI, code/commits/docs EN, co-author `Claude Opus 4.8`,
Conventional Commits, `Closes #N` only in PR body, plain `git commit`, stage by
explicit path (own target dir not gitignored), migration numbers are a shared
namespace (renumber at merge), each package merges on green CI after the user's word,
docs land in egret-data/docs. SDD per task; fresh worktree off main-with-SA1.

## Deferred (carried)

- SA4: the comprehensive "SA never holds platform_admin" enforcement (unchanged from
  SA1); and the catalog-raw-read vs DG-mask semantics docs + live-check.
- The frontend credential/credential-rotation UI → SA3.
- Full consolidation of the two creation surfaces into one → SA3/later.
