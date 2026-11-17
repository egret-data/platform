# Governance SP5b — catalog merge (one grant store) — design

> Second package of the SP5 capstone (one grant model). Builds on SP5a
> (full privilege vocabulary + `perm.securable_owners`, merged #873). The
> authoritative targets are the concepts spec
> `2026-11-02-governance-layer-concepts.md` (D4 vocab, migration principles)
> and the SP5 programme design `2026-11-16-gov-sp5-one-grant-model-design.md`
> (SP5b row + decomposition). SP5c (iceberg-rest EP) follows this.

## What SP5b is (programme design, package 2)

**Acceptance:** "one grant store; the embedded-catalog RBAC is gone; engines
keep working on their own credential; migration reversible while the switch is
open."

Fold the embedded Iceberg catalog's own RBAC
(`catalog.catalog_grants` / `catalog.catalog_roles` / `catalog.catalog_clients`)
into the unified `perm.grants` + `perm.securable_owners` store on catalog
securables; remove `catalog_roles`; replace the shared `engine` role (which
carries catalog-wide `manage`) with a per-engine credential whose service
account holds its own grants; switch the embedded-catalog enforcement to read
the unified store, gated by equivalence and reversible while the switch is open.

## Current state (verified on main `266e4730`, post-SP5a)

- **Catalog RBAC (schema `catalog`, migration `0053`):** `catalog_roles`
  (`id`, `warehouse_id`, `name`, `consumer_type IN ('engine','notebook','external')`,
  UNIQUE `(warehouse_id, name)`); `catalog_grants` (`catalog_role_id`,
  `securable_type IN ('catalog','namespace','table')`, `namespace text[]`,
  `table_name`, `privilege text` — free text, effectively `read`/`write`/`manage`
  via `grant_match::grants_write`); `catalog_clients` (machine credential bound
  to one role; SA4 migration `0230` added `service_account_id`, `revoked_at`,
  `expires_at`).
- **Enforcement — two readers, one rule.** The request guard
  `egret-bin/src/main.rs` (`EgretCatalogGuard::check`) runs in front of
  lakekeeper; the lakekeeper authorizer `egret-catalog-engine/src/authorizer.rs`
  (`EgretAuthorizer::client_may`) scopes vended S3 credentials. Both call
  `embedded::grants_for_role(warehouse, role)` and apply
  `grant_match::catalog_grant_covers_by` so they cannot disagree.
- **Shared `engine` role.** Exactly one role named `engine` per warehouse
  carrying catalog-wide `manage` (`egret-engine-mgmt/src/engines.rs`
  `ensure_engine_role`, `ENGINE_ROLE_NAME`). Each engine attachment mints its
  own `catalog_client` but reuses this one role, so every engine shares `manage`.
  The client credential `"{client_id}:{client_secret}"` is stored as a global
  secret referenced by `registry.engine_catalog_attachments.credential_secret_id`.
- **perm.grants (migration `0154`, actions widened by `0232`).** Securable =
  `(catalog_id → registry.catalogs, schema_name text, table_name, column_name)`,
  level encoded by which name columns are NULL; `actions <@
  {browse,read,write,create,manage}`. `perm.securable_owners` (migration `0233`)
  is one-owner-per-securable and **seeds nothing** (its own text names SP5b as
  the owner-backfill owner). Owners expand to all five actions as a synthetic
  allow in `ir_load.rs` (loses to deny, D6).
- **Catalog securable identity.** An embedded (egret-iceberg) catalog's
  `catalog.warehouses.id` maps 1:1 to a `registry.catalogs` row
  (`registry.catalogs.warehouse_id`), and that row's id is the `catalog_id`
  `perm.grants` uses. `perm.grants` already carries rows on catalog securables
  (governance overview reads them). Both sides identify objects by **name**, not
  Iceberg UUID; lakekeeper collation equality is resolved at decision time
  (`CatalogNameEq`).
- **Reversible-migration pattern.** `perm.migrated_from` (migration `0181`,
  SP4d-3) is the established provenance+undo scaffold: it records the model rows
  a switched-off source became, with SHA-256 content hashes; undo recomputes the
  hashes and refuses (409) if anything changed; the original is disabled, never
  deleted, while the switch is open.

## Decisions (resolved 2026-11-17)

1. **Namespace depth — dot-join into `schema_name`.** A multi-level Iceberg
   namespace maps to perm's single `schema_name` as a dot-joined path
   (`namespace.join(".")`), matching how `governance_overview` already labels
   catalog securables. This keeps the perm securable schema unchanged — no
   churn to the IR, the two compilers, the capability matrix, owners or eval —
   so the Ranger/OPA byte-identical guarantee holds. The pathological case (a
   single namespace level that itself contains a `.`) is **refused at migration
   with a reason**, not silently flattened.
2. **One SP5b package, multi-task plan.** The whole fold + enforcement switch +
   undo/finalize ship as one coherent package behind a reversible switch with an
   equivalence gate, reviewed once by the opus whole-branch review (as SP5a).
3. **Per-engine explicit grant, not a shared role.** Each engine becomes its own
   service-account principal holding an explicit allow grant of
   `{read,write,create,manage}` on the catalog securable in `perm.grants`. This
   preserves engine behaviour (engines genuinely create/alter/drop tables and
   write data across the warehouse) while killing the shared role: per-engine,
   auditable, revocable, in one store. The securable **owner** is the
   backfilled human creator or the platform-admins group, not the engine.

## In scope

### Securable + privilege mapping

- A catalog grant `(role, securable_type, namespace[], table_name, privilege)`
  becomes a `perm.grants` row on `(catalog_id, schema_name, table_name)` with
  `effect = allow` and the principal set to the service account(s) bound to that
  role's clients:
  - `securable_type = 'catalog'` → catalog-level securable (schema/table NULL).
  - `securable_type = 'namespace'` → `schema_name = namespace.join(".")`,
    table NULL.
  - `securable_type = 'table'` → `schema_name = namespace.join(".")`,
    `table_name = table_name`.
- Privilege → action set in the store (`grant_match` semantics confirmed:
  with `is_write=false` **any** grant covering the securable allows the read;
  writes require `write` or `manage`; a table grant lets its holder list the
  enclosing namespace — a browse affordance; `grant_sees_namespace` says any
  privilege sees the namespace path):
  - `read` → `{read}`
  - `write` → `{read, write}`
  - `manage` → `{read, write, create, manage}`
- SP5a's BROWSE implication and down-tree inheritance then apply in the IR as
  for any grant (a read on a table implies browse on its ancestors — exactly
  reproducing `grant_sees_namespace`'s "any privilege sees the path").

### The embedded catalog is a capability-matrix target (the equivalence linchpin)

The embedded catalog has **one write gate**: `grant_match` treats data write,
create-table, drop, alter and rename identically (all `is_write`), with no
finer create-vs-manage distinction. SP5a's richer vocab must therefore be
**lowered** at this target, exactly as OPA lowers create/manage:

- The embedded-catalog target enforces **`{browse, read, write}`**. Because the
  operation → action mapping below never requests `create` or `manage` at this
  target, those privileges are structurally **`NotEnforced`** here; a new SP5a
  capability-matrix entry records that (so `explain` and the docs tell the truth)
  — the catalog decision path itself is `perm-eval::decide`, not a compiled
  artifact, so this is honesty bookkeeping, not compiler lowering. This is what
  lets the equivalence gate pass despite the store carrying the finer vocab.
- **Operation → action mapping** (verified by the gate): list / load / read →
  `read` (listing a namespace → `browse`); **every mutation** — data write,
  create table/namespace, drop, alter, rename, commit — → `write`. Because the
  catalog's legacy gate is the single `is_write` boolean, mapping all mutations
  to the one enforced `write` action is what preserves legacy allow/deny exactly.
- Consequence for the engine grant (decision 3): its `{read, write, create,
  manage}` has `{browse, read, write}` **enforced** at the catalog (equal to the
  old shared-role `manage`, since the catalog write gate covers all DDL) and
  `{create, manage}` carried in the store but `NotEnforced` here — forward-looking
  for SP5c / finer enforcement, honest today.
- A catalog role with **no client / no service account** contributes no perm
  grant (nothing to attribute a principal to) and is reported, not silently
  dropped.

### Remove `catalog_roles`

- `catalog_roles` has no analogue in perm (perm has no grant-bundling role for
  catalog securables). Roles are **flattened**: each role's grants are
  re-attributed to the service-account principals of the clients bound to that
  role. The shared `engine` role is replaced per decision 3.
- `catalog_roles` (and `catalog_grants`) are **kept, disabled** while the switch
  is open; `finalize` drops them once the switch is closed. Nothing reads them in
  `unified` mode.

### Engine credential rework

- `ensure_engine_role` + the shared `engine` role are retired. On engine create /
  catalog attach, the engine's service-account principal receives an explicit
  `{read,write,create,manage}` allow grant on the catalog securable in
  `perm.grants` (provisioned in `engines.rs` / `catalog_apply.rs`). The per-engine
  client credential (`"{client_id}:{client_secret}"` secret) stays; only the role
  indirection and the shared `manage` go away.
- Existing attachments are migrated: each engine client's service account gets
  its per-engine grant; the shared `engine` role's `manage` grant is recorded in
  provenance and not carried forward as a shared grant.

### Owner backfill (the SP5a T3 deferral)

- Seed `perm.securable_owners` for embedded-catalog securables from the creator
  recorded in the catalog where one exists, else the platform-admins group
  (concepts spec "Owners of existing securables"). Idempotent, touches only
  securables without an owner row, additive (no behaviour change where no owner
  is seeded, per SP5a's owner model).

### Enforcement switch + unified decision path

- A per-catalog switch `catalog_grant_source ∈ {catalog_rbac, unified}` (default
  `catalog_rbac`; new catalogs created after finalize default `unified`). Stored
  on `registry.catalogs` or a dedicated `perm.catalog_grant_switch` table (plan
  decides; mirrors the SP3d EP switch).
- When `unified`, the guard and the authorizer resolve the caller's **service
  account** (from the catalog client) and evaluate `(principal, action,
  securable)` via `perm-eval::decide` over the catalog's loaded IR (grants +
  owners + rules), replacing `grants_for_role` + `catalog_grant_covers_by`. The
  operation → action mapping is the one above (read/load → `read`, listing →
  `browse`, every mutation → `write`), chosen so the unified decision equals the
  legacy single `is_write` gate; it is verified per lakekeeper operation by the
  equivalence gate.
- **Performance:** the guard is in the request hot path. The catalog's IR
  (grants/owners for that `catalog_id`) is loaded once and cached, invalidated
  when a grant/owner/rule for that catalog changes (same freshness signal the
  compile trigger already uses). The legacy path stays a simple indexed query;
  the unified path must not regress p99 materially — measured in the plan.

### Equivalence gate, undo, finalize

- **Gate:** before a catalog may switch to `unified`, an equivalence check runs
  `explain` over a sample of principals × objects (the clients/service accounts
  bound to that warehouse × the namespaces/tables named in its catalog grants,
  plus a sibling-not-covered probe) and compares the legacy decision
  (`catalog_grant_covers_by`) against the unified decision (`decide`). The switch
  is refused with the divergence list if any principal×object disagrees.
- **Provenance + undo:** a catalog-scoped provenance table (extending the
  `perm.migrated_from` pattern — e.g. `perm.catalog_rbac_migrated` mapping each
  source `catalog_role`/`catalog_grant` → the `perm.grants`/`securable_owners`
  rows it became, with content hashes). `migrate/undo` flips the switch back to
  `catalog_rbac`, removes the folded perm rows (by recorded id, refusing if their
  hash changed), and re-enables the catalog RBAC — reversible while the switch is
  open. The source `catalog_roles`/`catalog_grants` are never mutated by the
  migrate, only disabled from enforcement by the switch.
- **Finalize:** `migrate/finalize` closes the switch for a catalog (no more
  undo) and drops the now-dead `catalog_grants`/`catalog_roles` rows for that
  warehouse. Modeled on SP3d's `POST /perm/migrate/finalize`.
- HTTP surface: `POST /perm/catalog-migrate/{catalog}/plan|switch|undo|finalize`
  (exact paths in the plan), admin-only, mirroring the SP3d migrate endpoints.

## Out of scope (later / separate)

- **iceberg-rest EP (SP5c):** external Iceberg clients enforced by Lakekeeper
  itself + `withhold` + STS scope. SP5b keeps the current egret-guard + authorizer
  enforcement architecture, only pointing it at the unified store.
- **Deleting the legacy enforcement code paths** beyond the per-catalog finalize
  (the global removal of `grant_match`/`grants_for_role` once every catalog is
  finalized) — a cleanup after SP5c, per "no long-term backward compatibility."
- Any UI for the catalog migrate/undo/finalize beyond what SP3d already renders
  (a thin follow-up if needed).
- Multi-level-namespace-containing-a-dot support (refused, not handled).

## Verification (SP1/SP3d-style)

- **IR / mapping unit tests:** each `securable_type` → perm securable with a
  dot-joined schema; each privilege → action set; a role with multiple clients →
  grants on each service account; a catalog-level `manage` role → the per-engine
  grant set; the dot-containing-namespace refusal; owner backfill (creator else
  platform-admins), one owner per securable.
- **Equivalence (the gate itself):** property/shape tests that the unified
  `decide` matches the legacy `catalog_grant_covers_by` for a corpus of catalog
  grants × requests (read/write/create/manage × catalog/namespace/table, plus
  sibling-not-covered and namespace-subtree). Deny-wins and browse-implication
  behave per SP5a.
- **Reversibility:** migrate → undo restores the exact pre-migration state (grant
  set, owners, switch); undo refuses (409) if a folded row's hash changed;
  finalize is irreversible and drops the legacy rows.
- **Byte-identical (regression):** the Ranger/OPA goldens are unchanged — SP5b
  does not touch the IR securable schema or the compilers; re-run both mechanisms,
  zero diff, nothing re-blessed.
- **Differential + live-check:** on the demo stack, an engine (its new per-engine
  credential) and an unprivileged catalog client each hit a real embedded catalog
  before and after the switch; the allow/deny answers match the legacy path; the
  engine still reads/writes/creates; undo restores; finalize removes the legacy
  rows. Live-verify per the governance "every behaviour-changing PR" rule.

## Global constraints

- Branch `feat/gov-sp5b-catalog-merge` off main `266e4730` (worktree
  `../egret-sp5b`). `CARGO_TARGET_DIR=target-sp5b` on every cargo command.
  Throwaway Postgres; DB-gated tests MUST run, not skip.
- Chat VI; code/commits/docs EN. Co-author `Claude Opus 4.8`. Conventional
  Commits; `Closes #N` only in the PR body. Plain `git commit`; stage by explicit
  path (`target-sp5b/` not gitignored — never `git add -A`). No secrets.
- Migration numbers are a shared namespace — renumber above main AND open PRs at
  merge. Current highest on main is `0233`.
- Model-first: catalog grants live in the neutral store; the embedded catalog
  becomes a reader of the unified IR, not a second grant model.
- SDD per package; opus whole-branch review (it has repeatedly caught
  cross-cutting bypasses — SA4 `replace_platform_roles`, SP5a OPA over-reach).
- Each package merges on green CI after the user's explicit "merge đi"; docs land
  in egret-data/docs at once.

## Programme context

After SP5b, only SP5c (iceberg-rest EP) remains to complete SP5 and the
governance-layer vision (sub-projects 0–5). SP5b is the dependency gate for SP5c:
the iceberg-rest EP enforces the merged common grants, so the grant store must be
unified first.
