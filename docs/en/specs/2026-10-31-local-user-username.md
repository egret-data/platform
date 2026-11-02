# A new human user has a username from creation

## Premise (re-verified on main fb168a8f)

`users.username` is the only identity an engine sees for a person, and the perm write path
(`egret_perm_compile::principal_refs::lookup_user`) refuses a `user` ref no committed
`users.username` carries. No user-insert path set it:

| Creation path | Before | Where a name was minted |
|---|---|---|
| `POST /api/control-plane/users` (local create; the Users page uses it, there is no bulk create) | NULL, answered `"username": null` | first query / notebook / engine token, a compile naming a group/workspace/role they are in, or Governance's "Allocate username and add" |
| `egret create-admin` (bootstrap admin) | NULL | same |
| OIDC first sign-in (`provision_and_sync` → `insert_oidc`) | NULL; a later sign-in does not fill it | same |
| Workspace invite / SSO pre-registration | creates no user (binds at the first OIDC sign-in) | — |
| SCIM | does not exist | — |
| Service account (`insert_service_with`) | NULL | first token, or Governance allocation |

So a steward who created a user through the API and granted them in the same session got 400 ("that user has
no username yet"). Only the Governance picker worked around it, with a second write.

Was NULL intended? No design argued for lazy minting: the specs describe it as a fact ("no user-insert path in
egret sets it", `ensure_username_via`'s doc; DG People Q1). Migration 0040, which added the column, backfilled
every existing user, and every later design (SP4a D3, 2026-10-18 R5, People Q1) adds one more place that
mints because a NULL name kept surfacing as a defect.

## Decisions

1. **Mint at creation, for every human user** (local create, bootstrap admin, OIDC JIT insert), on the
   insert's own transaction, through the one mint path `ensure_username_via`. New repo methods
   `UserRepo::create_local` / `create_oidc` (insert + mint, one commit) are what production calls;
   `insert_local` / `insert_oidc` stay the bare insert (fixtures that model a user with no username yet).
   A row therefore never commits without a username. `POST /users` answers the minted name.
2. **Why no 409 stale-ref guard at creation** (the guard People Q1 put on the steward's allocation): since
   migration 0209 every name a stored `user` ref can carry is in `username_ledger` — the write path only
   accepts a committed username, the trigger ledgers every username a row takes, and 0209 seeded the ledger
   with every holderless ref — and `ensure_username_via` never mints a ledgered name. A creation mint is the
   same operation as the first-query mint, which has never had a guard. Only rows written behind the write
   path's back (raw SQL) could still match; the boot repair below keeps the guard for those.
3. **Service accounts are out of scope**: unchanged, they get a name at their first token or from
   Governance allocation (the service-account-as-principal design is a later package).
4. **Existing users with NULL: a boot repair, not a migration.** At boot, every human user (provider
   `<> 'service'`) still NULL is allocated through the same core as
   `POST /perm/people/users/{id}/username` (`ensure_username_via` + the stale-ref guard, one transaction per
   user). A user whose name a stored ref already names is skipped and logged; their username stays NULL and
   the Governance allocation answers 409 listing the refs, as today. A migration would need a second, SQL copy
   of the mint (slug, ledger-aware suffix, reserved names, the guard). Idempotent and cheap once converged
   (one indexed query). Best-effort: logged, never blocks boot.
5. No login-time mint: with 1 and 4, a human user lacks a name only when the guard refused it, and a login must
   not bypass that guard.
6. No migration (0211 not taken). No API shape change: `UserDto.username` was already `Option`.

## Tests (red first)

- `egret-controlplane/tests/create_user_username.rs`: as a principal holding only `user:create` and
  `policy:update`, `POST /users` answers a username equal to the ledgered one, and `POST /perm/grants` naming
  it succeeds; a second email with the same local part gets `-2`.
- `egret-auth/tests/oidc_db.rs`: a first OIDC sign-in leaves a username.
- `egret-perm-compile/tests/allocate_username.rs`: the boot repair mints a NULL human user, skips a service
  account, and skips (leaves NULL) a user whose name a stored grant principal names.
