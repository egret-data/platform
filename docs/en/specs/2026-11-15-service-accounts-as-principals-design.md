# Service accounts as first-class principals — design

> Programme umbrella + **SA1 (Principal backbone)** authoritative design. Later
> packages (SA2–SA4) get their own specs; this document fixes the unified model
> and the SA1 contract.

## Background

Today a service account (SA) is a by-product of the Data API, not a principal in
its own right:

- It **is** a real `users` row with `provider = 'service'` (migration `0119`),
  so `access_grants`, Cedar, `ensure_username` (Ranger/Trino delegation) and
  audit all key off it with no parallel identity plumbing. Human-facing lists
  exclude it (`UserRepo::list` has `WHERE provider <> 'service'`).
- Its workspace binding and name live in `dataapi.service_accounts`
  (`user_id PK → users`, `workspace_id NOT NULL`, `name`, `description`); its
  credentials are `dataapi.clients` (OAuth2 `client_credentials`, N per SA,
  rotate = mint-new-then-revoke-old, soft `revoked_at`, `expires_at`).
- The control plane **refuses** to manage it as a person
  (`refuse_service_account_profile`): `PATCH /users/{id}` lets only the `status`
  branch through (the documented kill switch — `users.status <> 'active'`, which
  `ClientStore::verify` honours), and blocks name/email with "manage it through
  the Data API service-account routes".
- **Catalog Clients** (`catalog_clients`, migration `0053`) are a *separate*
  machine-credential model: OAuth2 clients minted by the same embedded
  machine-IdP (`egret-machine-idp`) but for the Catalog audience, authorised by
  `catalog_roles`/`catalog_grants` per warehouse, reading Iceberg **directly**
  through lakekeeper. They are **not** `users` rows and sit **outside** the DG
  permission model (masks/row-filters/clearance are engine-side and do not
  apply to direct catalog reads).

The machine-IdP already serves **both** audiences from one provider
(`VerifiedClient` carries the `aud` and audience-specific claims), so the two
credential tables (`dataapi.clients`, `catalog_clients`) are deliberately
parallel in shape.

## Goal

Make a service account a **first-class principal, peer to a user**: its own
management surface, platform **or** workspace scope, membership in groups and
roles, a unified credential lifecycle, and a single identity of record that both
Data-API and Catalog credentials hang off.

## Unified target model (fixed here, built across SA1–SA4)

- An SA is a `users` row (`provider = 'service'`) that **never** logs in
  interactively (`users_service_has_no_password` already enforces this) and
  **never** holds `platform_admin`.
- A **platform-level `service_accounts` table** is the principal of record:
  `user_id PK → users`, `scope ∈ {platform, workspace}`, `workspace_id`
  (NULL iff platform, NOT NULL iff workspace), `name`, `description`,
  `created_by`, `created_at`. Scope mirrors the user/group principal model that
  `platform_role_assignments` already supports (`principal_type`, `scope_type`).
- Under each SA hang **N credentials, each with an audience** — `data-api`
  (`dataapi.clients` today) or `catalog` (`catalog_clients` today, re-owned by an
  SA in SA2). Lifecycle is uniform: create, rotate (mint-new-then-revoke-old),
  expire, disable. Disabling the SA (`users.status`) is the single kill switch
  for **every** audience.
- An SA is a principal everywhere a user is: `access_grants`, groups, roles,
  clearance — all keyed off the user id, which already works.
- **Catalog credentials read Iceberg directly, outside DG.** This is inherent
  to direct-catalog access (masks are engine-side). The design **surfaces and
  documents** it and keeps the two audiences visibly distinct; it does **not**
  attempt to enforce DG masks at the catalog/storage layer.

## Decomposition (four packages, built SA1 → SA2 → SA3 → SA4, each its own PR)

| Package | Scope | Subsystems |
|---|---|---|
| **SA1 — Principal backbone** | `service_accounts` identity table + scope + backfill; control-plane SA routes (list/get/create/disable/enable/delete + group/role membership); guards. | `egret-auth`, `egret-controlplane` |
| **SA2 — Unified credentials** | Credentials as SA-owned across audiences: re-own `catalog_clients` to an SA (migration + backfill), uniform rotate/expire/disable via the machine-IdP; the SA kill switch reaches the Catalog audience. | `egret-catalog-engine`, `egret-machine-idp`, `egret-dataapi` |
| **SA3 — Service accounts tab (FE)** | Dedicated tab peer to People: list/create/edit SA, group/role membership, credential management, scope; surfaces the catalog-vs-DG distinction. | `frontend` |
| **SA4 — Guarantees + live** | Cross-cutting: no interactive login / no `platform_admin` enforced everywhere; OIDC group sync never adds an SA; username ledger covers `svc-` names; catalog-raw-read vs DG-mask semantics documented + tested; live-check on a stack. | cross-cutting + docs |

## Global constraints

- Chat VI; code/commits/PRs/docs EN. Co-author `Claude Opus 4.8`. Conventional
  Commits; `Closes #N` only in the PR body. Plain `git commit`; stage by
  explicit path (own target dir is not gitignored). No secrets.
- Each package merges on green CI after the user's word. Docs land in
  `egret-data/docs` as a docs PR.
- Migration numbers are a shared namespace — renumber above main **and** open
  PRs at merge.
- SDD per code package; a fresh worktree with its own `CARGO_TARGET_DIR`.

---

# SA1 — Principal backbone (authoritative design)

**Goal:** promote the service account to a control-plane-managed first-class
principal with platform/workspace scope, group/role membership, and
disable→immediate-revoke (Data-API audience). Backend only.

**In scope:** the `service_accounts` identity table + backfill from
`dataapi.service_accounts`; control-plane routes to list/get/create/
disable/enable/delete an SA and to add/remove group and role membership; scope
model; guards (no `platform_admin`, no SA in an OIDC group); opening
`refuse_service_account_profile` for name/description via the SA routes.

**Out of scope:** re-owning `catalog_clients` and the Catalog-audience kill
switch (SA2); the frontend tab (SA3); the live-check and the cross-cutting
guarantee sweep + docs (SA4). Credential **creation** (minting clients) stays on
the existing Data-API routes for now; SA1 gives the SA an identity and
membership, which is already useful (a credential inherits its SA's access).

## Identity & scope model

Approach chosen: a **platform-level `service_accounts` table as the principal of
record**, with `dataapi.service_accounts` folded into it.

```sql
-- migration NNNN_service_account_principals.sql
CREATE TABLE service_accounts (
  user_id      uuid PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
  scope        text NOT NULL CHECK (scope IN ('platform','workspace')),
  workspace_id uuid REFERENCES workspaces(id) ON DELETE CASCADE,
  name         text NOT NULL,
  description  text NOT NULL DEFAULT '',
  created_by   uuid REFERENCES users(id) ON DELETE SET NULL,
  created_at   timestamptz NOT NULL DEFAULT now(),
  -- scope and workspace_id agree; names are unique within their scope bucket.
  CONSTRAINT service_account_scope_ws CHECK (
    (scope = 'platform'  AND workspace_id IS NULL) OR
    (scope = 'workspace' AND workspace_id IS NOT NULL)
  )
);
-- One unique name per workspace, and one unique name across all platform SAs.
CREATE UNIQUE INDEX service_accounts_ws_name
  ON service_accounts (workspace_id, name) WHERE scope = 'workspace';
CREATE UNIQUE INDEX service_accounts_platform_name
  ON service_accounts (name) WHERE scope = 'platform';
```

**Backfill, in the same migration:** every existing `dataapi.service_accounts`
row is workspace-scoped, so:

```sql
INSERT INTO service_accounts (user_id, scope, workspace_id, name, description, created_by, created_at)
  SELECT user_id, 'workspace', workspace_id, name, description, created_by, created_at
    FROM dataapi.service_accounts;
```

`dataapi.service_accounts` is then **repointed**: it keeps only its Data-API
binding role by referencing the new table and dropping the duplicated identity
columns the principal now owns. To stay a drop-in for the existing Data-API
code without a same-PR rewrite of every reader, SA1 replaces the duplicated
columns (`name`, `description`, `created_by`, `created_at`, and the now-derived
`workspace_id`) with a backward-compatible **view** `dataapi.service_accounts`
over `service_accounts` (filtered to `scope='workspace'`, exposing the original
column set), and renames the base table to `dataapi.service_account_bindings`
holding only Data-API-specific columns (none today beyond the FK — so the base
table may be empty of extra columns and exist only as the FK anchor, or be
dropped and the view sourced directly from `service_accounts`). The reader
audit in the plan confirms which columns `egret-dataapi` actually selects; the
view exposes exactly those.

> Rationale for Approach 1 over the alternatives: a single source of SA identity
> makes scope first-class and gives SA2 a clean FK target for re-owning
> `catalog_clients`. A union-view over two identity tables (Approach 2) keeps two
> sources and fights the "unify now" decision; putting scope/workspace on `users`
> (Approach 3) was explicitly rejected by the `0119` comment because a workspace
> column is meaningless for human users.

## Store: `ServiceAccountRepo` (`egret-auth`)

A new repo alongside `UserRepo`:

- `create(scope, workspace_id, name, description, created_by) -> ServiceAccount`
  — inserts the `users` row (`provider='service'`, `status='active'`,
  `password_hash NULL`, `username` allocated as `svc-{slug}` via the existing
  `ensure_username`/ledger path) **and** the `service_accounts` row in one
  transaction. Rejects a `platform` scope with a `workspace_id` and vice versa
  (the DB CHECK is the backstop).
- `list(scope_filter: Option<Scope>, workspace_id: Option<Uuid>) -> Vec<ServiceAccount>`.
- `get(user_id) -> Option<ServiceAccount>`.
- `delete(user_id)` — deletes the `users` row (CASCADE removes the
  `service_accounts` row, `dataapi` bindings, grants, memberships).
- `disable(user_id)` / `enable(user_id)` — thin wrappers that set
  `users.status` through the existing status path so the session/token
  revocation and `user:disable` re-check already wired there still run.

`ServiceAccount { user_id, scope, workspace_id, name, description, created_by, created_at, status }`.

## Control-plane surface (`egret-controlplane`)

Mounted under `/api/control-plane/service-accounts` (requires the same
`user:*`-class permissions the people routes use; exact permission strings read
from the existing `access/mod.rs` guards in the plan):

- `GET  /service-accounts?scope=&workspace_id=` → list.
- `POST /service-accounts` `{ scope, workspace_id?, name, description? }` → create.
- `GET  /service-accounts/{id}` → one SA with its group and role memberships.
- `POST /service-accounts/{id}/disable` / `/enable`.
- `DELETE /service-accounts/{id}`.
- `POST /service-accounts/{id}/groups` `{ group_id }` /
  `DELETE /service-accounts/{id}/groups/{group_id}` — add/remove group
  membership (reuses the group-member plumbing; **rejects an OIDC group**).
- `POST /service-accounts/{id}/roles` `{ role_id, scope_type, workspace_id? }` /
  the matching remove — reuses `platform_role_assignments`
  (`principal_type='user'`); **rejects `platform_admin`**.

`refuse_service_account_profile` is **narrowed**, not removed: the human
`/users/{id}` routes still refuse an SA for name/email, but the new
`service-accounts` routes own name/description for an SA. The `status` kill
switch on `/users/{id}` stays and is now mirrored by the `disable`/`enable`
routes.

## Guards (SA1)

1. **No `platform_admin` for an SA** — the role-add route refuses when
   `role_id` resolves to `platform_admin` (reuse the role lookup already in
   `roles.rs`). A DB-level guard is a stretch goal; the route guard is required.
2. **No SA in an OIDC group** — the group-add route refuses when the target
   group's membership is IdP-owned (the predicate already exists in `groups.rs`
   for the OIDC-member reports; reuse it).
3. **No interactive login** — already enforced by
   `users_service_has_no_password`; SA1 adds no password path, so this holds by
   construction. A test asserts an SA created here has `password_hash IS NULL`.

## Lifecycle (SA1)

Disable sets `users.status`, which `ClientStore::verify` (Data-API) already
honours → a disabled SA stops minting `data-api` tokens within the stateless
token's remaining window, exactly as documented today. The **Catalog** audience
does not yet consult a user status (its `catalog_clients` have no user link);
SA2 re-owns `catalog_clients` to the SA and brings them under the same kill
switch. SA1 documents this boundary in the plan and tests only the Data-API
audience.

## Testing (SA1, DB-gated)

- Create a **platform** SA (no workspace) and a **workspace** SA; assert the
  `service_accounts` row, the `users` row (`provider='service'`,
  `password_hash IS NULL`, `svc-` username), and the scope/workspace CHECK.
- **Backfill:** seed a legacy `dataapi.service_accounts` row before the
  migration shape, confirm it appears as a workspace SA and that the Data-API
  reader still sees it through the compatibility view.
- **Membership:** add/remove a group and a role; assert the grant plumbing sees
  the SA as a principal.
- **Guards:** adding `platform_admin` is refused; adding the SA to an OIDC group
  is refused.
- **Disable:** a disabled SA's `data-api` client fails `verify`; enable
  restores it.
- **Human-route refusal unchanged:** `PATCH /users/{sa_id}` still refuses
  name/email and still lets `status` through.

## Self-review

- **Spec coverage:** every SA1 in-scope item maps to a section (identity table +
  backfill → Identity & scope model; routes → Control-plane surface; membership
  → surface + guards; disable → Lifecycle). Out-of-scope items are named and
  deferred to SA2–SA4.
- **Consistency:** `scope` values `{platform, workspace}` and the
  `service_accounts` column set are identical across the SQL, the repo struct,
  and the routes. The kill switch is `users.status` in every mention.
- **Open verification for the plan (not placeholders):** the exact column set
  `egret-dataapi` selects from `dataapi.service_accounts` (drives the
  compatibility view), the exact `user:*` permission strings on the people
  routes, and the OIDC-group predicate's function name in `groups.rs` — all read
  from existing code during planning.
