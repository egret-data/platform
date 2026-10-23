# Username takeover after user delete: design

Status: design, 2026-10-23. Not implemented. Base: `origin/main` `aec59807`. Branch `fix/username-takeover`.
Plan: [`../plans/2026-10-23-username-takeover.md`](../plans/2026-10-23-username-takeover.md).
Programme: KI fix programme, security package. Follow-up of the #692 review (one of eight follow-ups; the other
seven are non-goals, §11).

The user settled the scope on 2026-10-23 (§3, D1–D5). This document implements those decisions and records the
choices the code forced (§4). The open questions of §12 are answered (D4, D5).

## 1. Problem

egret stores some references to users by **username**, not by id:

- `perm.grant_principals`, `perm.rule_exemptions` and `perm.engine_grant_principals` rows of kind `user`;
- the `users[]` of hand-written (`managed_by = 'operator'`) Ranger policies;
- the `egret-admin - …` infra policies, which give every platform admin ALL access on an engine.

The engine compares the name in the policy with `preferred_username` in the engine JWT, byte for byte.

`UserRepo::hard_delete` deletes the `users` row and so frees its `username`. `ensure_username_via` mints the
**lowest free** `base`, `base-2`, … for the next person whose email slugs to the same base. That person then
inherits every allow, every deny exemption and every mask or row-filter exemption still naming the name. If the
deleted person was a platform admin, the next person also inherits engine ALL access, because the `egret-admin`
policy is a snapshot written only at engine apply and never rewritten when admins change.

There are three smaller gaps:

- `hard_delete` does not recompile the profiles reached through the user's **roles**, **workspace memberships**
  or **clearances**. Those compiled bodies keep naming the freed username until something else recompiles them.
- Nothing records who was deleted. A user delete writes no mutation event, so a later audit cannot name them.
- Engine JWTs minted before the delete stay valid until they expire.

## 2. Evidence (verified at `aec59807`)

Every row was re-read in the worktree. Rows marked **new** were not in the investigation findings, or correct
them.

| id | fact | where |
|---|---|---|
| E1 | `users.username` is `citext UNIQUE`, NULLable. No insert path sets it | `migrations/0040_user_username.sql:1` |
| E2 | The only runtime writer of `users.username` is `ensure_username_via`: an `UPDATE … SET username = $2 WHERE id = $1 AND username IS NULL` per candidate inside a SAVEPOINT. A unique violation rolls back to the savepoint and tries the next suffix. The only skipped name is `egret-lineage` | `crates/egret-auth/src/users.rs:429-508`, `:470`, `:466` |
| E3 | The other writers of `users.username` are migrations: the 0040 backfill and `0187`'s `UPDATE users SET username = NULL WHERE username = 'egret-lineage'` (plus its CHECK). Seeds (`deploy/demo/seed.sh`, `scripts/`) never write it. Tests write it directly with `INSERT INTO users (…, username)` in about 30 test files | `migrations/0040_user_username.sql:22`, `migrations/0187_view_lineage.sql:51-52`; e.g. `crates/egret-perm-compile/src/principal_refs.rs:761` |
| E4 | Mint callers: first query (`crates/egret-query/src/execution/mod.rs:67`, `:87`); engine JWT (`crates/egret-bin/src/engine_minter.rs:74`); BI (`crates/egret-bi/src/routes.rs:2374`); alert owner (`crates/egret-automation/src/evaluator.rs:160`); engine admin seed (`crates/egret-engine-mgmt/src/engines.rs:2507`); compile (`crates/egret-perm-compile/src/members.rs:332`, `:359` `premint_model_usernames`); Governance allocate (`crates/egret-perm-compile/src/routes/people.rs:574`). Only the Governance route checks for stale refs (409, `:577-593`) | — |
| E5 | `hard_delete` locks the user's groups and recompiles the group profiles plus the profiles of engine grants that target the user. It deletes `sessions`, `user_groups` and `access_grants`, then the row (`RETURNING name, email`). It does not compute the role, workspace or clearance arms | `crates/egret-auth/src/users.rs:298-382`; `crates/egret-perm-compile/src/group_membership.rs:124-150`; `crates/egret-perm-compile/src/trigger.rs:1998-2033` |
| E6 | The arms that already exist and that `hard_delete` skips: `affected_services_for_role_membership` (clearance, grant, engine-grant, and impersonation when the role is `platform_admin`) and `affected_services_for_workspace_members`. Role holders and workspace members are read from `access_grants` joined to `users` | `trigger.rs:1726-1776`, `:1797-1860`; `members.rs:501-555` |
| E7 | `egret-admin - <chain>` policies (`managed_by = 'infra'`, one per resource hierarchy, `policyItems[0].users` = admins, `delegateAdmin: true`) are written only by `seed_admin_all_access`, called from the Trino and StarRocks engine apply paths with `admin_usernames()` | `crates/egret-policy/src/default_policies.rs:53-110`, `:167-197`; `crates/egret-engine-mgmt/src/engines.rs:771-776`, `:3821-3826`, `:2499-2512` |
| E8 **new** | `seed_admin_all_access` returns `Ok(0)` without writing when `users` is empty, so a set that shrinks to nothing never clears a stale body | `default_policies.rs:173-175` |
| E9 **new** | `admin_usernames` → `RoleRepo::platform_admin_user_ids`, the direct and group arms, **status ignored**. A disabled admin is in the `egret-admin` body today. `guard_not_last_admin` uses the same list, even though its doc says "last active platform admin" | `crates/egret-auth/src/roles.rs:176-201`; `crates/egret-controlplane/src/access/mod.rs:85-111` |
| E10 **new** | The writers of platform-role membership: `assign_user_role`/`unassign_user_role` (recompile through `with_recompile`, `access/mod.rs:444`, `:498`); `PUT /users/{id}/platform-roles` (`replace_platform_roles`, recompiles, `crates/egret-workspace/src/rbac.rs:77-160`); **`assign_group_role`/`unassign_group_role`, which call the non-recompiling `RoleRepo::assign`/`unassign`** (`access/mod.rs:852`, `:892`); the `create-admin` CLI's raw `INSERT INTO access_grants` (`crates/egret-bin/src/main.rs:133-145`); and role delete, which is refused for seeded roles such as `platform_admin` (`access/mod.rs:1176-1181`) | — |
| E11 | The writers of group membership all go through `GroupMembershipRecompiler::lock_affected_in_tx` + `recompile_in_tx`: OIDC sync, add/remove member, delete group, and `hard_delete`'s own variant | `crates/egret-auth/src/groups.rs:623-720`, `:897-950`, `:962-1065` |
| E12 | Every recompile funnels through `recompile_locked_service_in_tx`, which returns early when `compile_desired` is `None` (a profile the compiler does not target) | `trigger.rs:1439-1471`, `:2170-2201` |
| E13 | User status is written only by `UserRepo::set_status`, from `PATCH /users/{id}` (disable revokes sessions; there is no recompile) | `crates/egret-auth/src/users.rs:213-221`; `access/mod.rs:294-309` |
| E14 | `DELETE /users/{id}` writes no event. `service_account:delete` records `{name, email}` through the best-effort sink, not the username | `access/mod.rs:356-378`; `crates/egret-dataapi/src/routes.rs:679-711` |
| E15 | `record_change` inserts a `kind = 'mutation'` row on the caller's transaction. It `debug_assert!`s the pair is in `FAMILIES`. It records even when `audit.enabled` is false (decision O3). Every audit row is pruned after `retention_days` (default 90) | `crates/egret-audit/src/governance.rs:63-136`, `:194-240`; `crates/egret-audit/src/lib.rs:223-228` |
| E16 | Service-account email `svc.{slug}@{ws}.service.egret.local` slugs to `svc-{slug}`, the same namespace as humans | `crates/egret-dataapi/src/routes.rs:551` |
| E17 **new** | Engine JWT: `sub` = `preferred_username` = username, TTL **900 s**, no `cgen` claim. The machine-IdP token exchange requires `cgen`, so a user engine JWT cannot be refreshed. Tokens are minted server-side only (query, BI, automation). A deleted user cannot get a new one: `ensure_username` answers `NotFound` | `crates/egret-bin/src/engine_minter.rs:10-27`, `:60-96`; `crates/egret-bin/src/main.rs:1650`; `crates/egret-machine-idp/src/lib.rs:433` |
| E18 **new** | `egret_delegator` (the impersonate-mode service user) is **not** a reserved username, so a human can be minted it. This was recorded as a separate fix (engine-grants spec E21) | `crates/egret-core/src/engine_principals.rs:18-29` |
| E19 | The Governance attention model: `SOURCES` with `Requires` over a `held` set of permission strings, one savepoint each, typed `Ref`s the client turns into URLs | `crates/egret-controlplane/src/governance_overview.rs:174-300`, `:331-410`; `frontend/src/routes/governance/overview/refUrl.ts` |
| E20 | The delete dialogs: `UsersPage.tsx:504-516` and `UserDetail.tsx:340-350`. The description says "This user, their sessions, group memberships, and role grants will be permanently removed." `ConfirmDeleteDialog` accepts `children`. The disable dialog is `UserDetail.tsx:318-338`. `ControlPlaneUserDto` has no `username` | `frontend/src/routes/control-plane/access/*`; `frontend/src/components/ui/confirm-delete-dialog.tsx:61` |
| E21 | Migration numbers: the highest on `origin/main` is `0206_workspace_invites.sql`. Another open branch takes 0207. **This package reserves `0208`** | `migrations/` |

## 3. User decisions (settled 2026-10-23, not reopened)

- **D1.** A username is never reused. An append-only ledger enforces it for every mint path, humans and service
  accounts alike. The migration seeds it with every name that is knowable. The delete UI and API steer admins to
  Disable.
- **D2.** `egret-admin` follows platform_admin membership. Every change rewrites the `egret-admin` policies of
  every affected engine **in the same transaction**. A boot pass repairs stale bodies.
- **D3.** Past takeovers are **reported only**: an admin-only route and an Overview attention signal. Nothing is
  disabled automatically.
- Also in scope: close `hard_delete`'s recompile gaps; emit `user:delete` through `record_change`; decide the JWT
  question.
- **D4** (answers Q1). `user:delete` events are a separate `User` event family, readable with `user:read`, not
  `policy:read`, because they carry the deleted person's email (PII). §4.4 says how the feed gates it.
- **D5.** The Platform docs (`egret-data/docs`) are updated in a separate docs PR after the code PR merges, by the
  controller, not in this PR. `docs/vi/specs/2026-10-23-username-takeover-design.md` is written as a Vietnamese
  reference translation of this spec once the code is done (the English text is canonical).

## 4. Design

### 4.1 The ledger (D1)

**Table** (`migrations/0208_username_ledger.sql`, public schema next to `users`):

```sql
CREATE TABLE username_ledger (
    username   citext PRIMARY KEY,
    user_id    uuid NULL REFERENCES users(id) ON DELETE SET NULL, -- current holder
    minted_for uuid NULL,        -- the user it was minted for; no FK, survives the delete
    minted_at  timestamptz NULL, -- NULL = held before 0208 (unknown)
    retired_at timestamptz NULL,
    source     text NOT NULL CHECK (source IN
        ('mint', 'backfill_user', 'backfill_ref', 'backfill_policy', 'backfill_audit', 'reserved'))
);
```

**Enforced by triggers on `users`, not by Rust code.** This is the main design choice. The ledger row is written by
an `AFTER INSERT` trigger and an `AFTER UPDATE OF username` trigger on `users`. Each fires when `NEW.username` is
non-NULL and differs from `OLD.username`. The trigger function:

1. returns at once if a ledger row for `NEW.username` already has `user_id = NEW.id` (the holder re-setting their
   own name);
2. otherwise runs `INSERT INTO username_ledger (username, user_id, minted_for, minted_at, source) VALUES
   (NEW.username, NEW.id, NEW.id, now(), 'mint')`. If the name was ever held, backfilled or reserved, this raises
   `23505` on `username_ledger_pkey`.

Why a trigger:

- **It is the same savepoint.** The trigger runs inside `ensure_username_via`'s `UPDATE`, so the insert happens in
  the existing mint savepoint, as D1 asks. `is_unique_violation()` matches `23505` from either index. The existing
  `continue` then tries `base-2`. `ensure_username_via` needs no logic change (only its docs change).
- **There is no check-then-insert race.** A concurrent `hard_delete` of `bob` cannot open a window: the ledger row
  for `bob` was committed when `bob` was minted, and the delete only sets `user_id` to NULL. A mint of `bob` blocks
  on the `users` unique index until the delete commits, then fails on the ledger PK. A "SELECT from a retired
  table, then UPDATE" design would race under READ COMMITTED; this one cannot.
- **It covers every writer, including the ones nobody remembers.** That means E2–E4, the tests that write
  `username` directly (E3), a future seed, and an operator's `psql`. This follows the lesson "unrepresentable beats
  unforgettable".

**Retirement.** A `BEFORE DELETE ON users` trigger sets `retired_at = now()` on the user's ledger rows. The FK's
`ON DELETE SET NULL` then clears `user_id`. An `UPDATE OF username` that changes or NULLs a name (0187's shape)
retires the old row the same way. `minted_for` is never cleared, so the ledger permanently records who held the
name. This record outlives audit retention (E15).

**Append-only.** A `BEFORE DELETE OR UPDATE ON username_ledger` trigger refuses a `DELETE` and refuses any change
to `username`, `minted_for`, `minted_at` or `source`. Only `user_id` (the FK action) and `retired_at` may change.
No API frees a name. An operator who must free one has to drop the trigger in a manual migration, on purpose.

**Seed** (inside 0208, as a SQL function `username_ledger_backfill()` that the migration calls once). Each step is
`ON CONFLICT DO NOTHING`. It stays callable, so a test can insert pre-0208 shapes and run it again:

| step | source | rows | `retired_at` |
|---|---|---|---|
| 1 | `backfill_user` | every `users.username` (`user_id` = `minted_for` = the user, `minted_at` NULL) | NULL |
| 2 | `backfill_ref` | every `kind = 'user'` ref in the three `perm.*` principal tables that no `users.username` carries (citext-distinct) | `now()` |
| 3 | `backfill_policy` | every `users[]` name in any item list of **any** `policy.authz_policies` body (`operator`, `compiler` **and** `infra`), excluding `{…}` macros, that no `users.username` carries | `now()` |
| 4 | `backfill_audit` | (a) from `service_account:delete` events, the email's local part slugged exactly like `slugify_localpart` (the base slug; the suffix is unknowable); (b) every `"user:<name>"` string in the `before`/`after` of governance mutation events, with no current holder | `now()` |
| 5 | `reserved` | `egret-lineage`, `egret_delegator` | `now()` |

Step 3 is wider than D1's wording ("hand-written"), and deliberately so. A stale **compiled** body (E6 gaps) or a
stale **`egret-admin`** body (E7) names a freed name exactly as dangerously as a hand-written one does, and
`egret-admin` is the worst case. Step 3 also reserves `egret_delegator` wherever an impersonate engine exists. Step
5 then closes E18 for every installation, at the cost of one row. `is_reserved_engine_username` and the 0187 CHECK
stay as they are.

The SQL slug in step 4a must agree with `slugify_localpart` (lowercase the local part; replace each run outside
`[a-z0-9_]` with one `-`; trim `-`; empty becomes `user`). A test compares the two over a corpus.

**What the ledger costs.**

- A person who is deleted and comes back (an OIDC re-login creates a new row, `users.rs:182`; an admin re-creates a
  local account) gets `bob-2`. Allows naming `bob` no longer apply to them, which fails closed. **Denies,
  exclusions and mask exemptions naming `bob` no longer bind them either, which fails open for that one person.**
  The report (§4.5) lists those refs as holderless. This is the reason the UI steers admins to Disable (§4.4).
- A name that was pre-written in a hand-written policy for someone who never existed is retired by step 3. If that
  person joins later, they get `name-2`.
- A recreated service account gets `svc-x-2`. That is harmless: clients authenticate by OAuth client id.
- The ledger only grows (one row per name ever minted). `ensure_username_via`'s 999-suffix bound is unchanged.

**The Governance allocate guard stays** (`people.rs:577-593`). After the seed, a holderless ref names a ledger row
and is skipped by every mint. A ref inserted by raw SQL after 0208 still bypasses the ledger, though, and the 409
still catches it. Its tests (`tests/allocate_username.rs`) insert exactly that shape and keep passing.

### 4.2 `egret-admin` follows platform_admin (D2)

**One definition of the set.** `egret_auth::roles::engine_admin_user_ids_via(conn)` returns the platform_admin
holders through the direct arm and the group arm (E9's two arms), **`status = 'active'` only**, ascending by id.
`admin_usernames` (engine apply) and the in-transaction sync below both call it. Usernames are minted under
`lock_compile_mints_in_tx` (the compile's mint order) and sorted, so the body is deterministic. Today's
`UNION`-ordered list could churn the body.

**Disabled admins are dropped from `egret-admin`.** The reasons:

1. The policy means "may do anything on this engine". A disabled account may do nothing in egret.
2. `egret-admin` is the one body whose blast radius is the whole engine. Removing a disabled admin from it closes
   the only path by which a disabled admin's already-minted JWT (≤ 900 s, E17) or a scheduled alert running as them
   (`evaluator.rs:147-180` mints for the owner without a status check) keeps ALL access.
3. The cost is one more trigger point: a status change of an admin syncs the bodies.

The rest of the compiled model still names disabled users (role, group and workspace expansion do not filter
status). That is consistent with "Disable keeps memberships and grants" (the disable dialog's own words), and it is
not changed here. The impersonation near-root check `is_platform_admin` stays status-agnostic, which is the safe
direction: it refuses more targets, never fewer.

**The writer.** `egret_policy::default_policies::sync_admin_all_access_in_tx(conn, service_id, users)`:

- reads the service's name and type;
- builds the bodies with `build_default_policies` (`users` may be empty);
- upserts each hierarchy with `ON CONFLICT … DO UPDATE … WHERE authz_policies.policy_jsonb IS DISTINCT FROM
  EXCLUDED.policy_jsonb`;
- bumps `authz_services.policy_version` only if a row changed;
- returns the names it removed and added.

An empty set writes `users: []`, an item that matches nobody, so the stale body is cleared (this fixes E8). Which
services: `admin_policy_services_via(conn)`, every `service_id` holding at least one `managed_by = 'infra'` policy
whose name `is_seeded_hierarchy_name(_, "egret-admin")`. The sync rewrites only engines that were seeded. Engine
apply still creates the bodies on first provision: `seed_admin_all_access` becomes a call to the same writer and
keeps "create on apply", but no longer returns early on an empty list (it then rewrites existing rows and creates
none).

**The chokepoint is the recompile.** `recompile_locked_service_in_tx` (E12) calls
`sync_admin_all_access_in_tx(service_id, current set)` **before** `compile_desired`, when the service is in
`admin_policy_services_via`. It does this before the early return, because an engine profile the compiler does not
target still carries `egret-admin`. It also comes before compile, so the impersonation near-root check reads the
fresh body. Every existing recompile path therefore converges `egret-admin` for the services it locks. The
admin-changing paths only have to **add the admin-policy services to the set they lock**:

| change | path | how the services get into the locked set |
|---|---|---|
| user role grant/revoke | `assign/unassign_platform_role_and_recompile`, `replace_platform_roles` | `affected_services_for_role_membership("platform_admin")` gains the admin-policy arm, next to its impersonation arm |
| group role grant/revoke | `assign_group_role`/`unassign_group_role` | **changed** to call `assign/unassign_platform_role_and_recompile(pool, "group", …)` (today they recompile nothing, E10). Any role, not only platform_admin: one path, and an unchanged recompile writes nothing |
| group membership (add, remove, OIDC sync, group delete) | `GroupMembershipRecompiler::lock_affected_in_tx` | `affected_services_for_group_membership` gains the admin-policy arm under its existing `holds_platform_admin` test |
| user delete | `hard_delete` → `lock_affected_by_user_delete_in_tx` | the role arm (§4.3) for a direct admin; the group arm for a group admin |
| disable / enable | **new** `egret_perm_compile::set_user_status_and_recompile(pool, user_id, status)`, called from `PATCH /users/{id}` instead of `set_status` | adds the admin-policy services when the user holds platform_admin directly or through a group; locks; `UserRepo::set_status_in_tx`; `recompile_services_in_tx` |
| `create-admin` CLI | `egret-bin` | after its insert, one transaction runs `recompile_services_in_tx(admin_policy_services)` |
| drift, older bodies | **boot pass** `sync_admin_policies(pool)` | spawned next to `recompile_role_referencing_profiles` (`main.rs:2362-2376`): lock `admin_policy_services_via` ascending, `recompile_services_in_tx`. Idempotent and best-effort; a failure is logged and never blocks boot |

`premint_model_usernames` also mints the active admins when any admin-policy service exists. This keeps the
"mint ascending once, before the passes" invariant (`members.rs:336-358`).

Lock order does not change: profile locks ascending, then the mint lock, then row writes. The sync writes only rows
of services the caller has already locked.

### 4.3 `hard_delete` closes its recompile gaps

`PermGroupRecompiler::lock_affected_by_user_delete_in_tx` (the trait signature is unchanged) adds, read on the
same transaction **before** the `access_grants` delete:

- `affected_services_for_role_membership(name)` for every distinct role name in the user's `access_grants` (any
  scope). This brings in the clearance arm, the grant arm, the engine-grant arm, and (for `platform_admin`) the
  impersonation and admin-policy arms;
- `affected_services_for_workspace_members(ws)` for every `scope_type = 'workspace'` grant of the user.

The union is sorted and deduplicated, then locked in one ascending pass, then the mint lock is taken, exactly like
the existing group and engine-grant arms.

**Residual race, stated.** A role granted to the user concurrently, after this read and before the delete, is
swept by the `DELETE FROM access_grants` but its profiles are not recompiled. Their body then names a retired
username. The ledger makes that harmless (nobody can hold the name), and the next recompile of those profiles
removes it. The recompile closure is there so the bodies are correct. The ledger is the guarantee.

### 4.4 Delete event, Disable steering, UI copy

**Event.** `hard_delete(target, actor: &Principal, recompiler)` returns `Option<DeletedUser { name, email, username:
Option<String> }>` (`RETURNING name, email, username`). In the same transaction, before commit, it calls
`record_change` with:

- `action::USER_DELETE = "user:delete"`, `resource::USER = "user"`, `resource_id` = the user id;
- summary `{"before": {"username", "email", "name", "provider"}, "after": null}`.

The pair joins `FAMILIES` under a new family `User` (D4) and `GOVERNANCE_ACTIONS`. This covers both callers
(`DELETE /users/{id}` and the service-account delete).

**Who reads it (D4).** `GET /governance/changes` builds the list of families a reader may see from one
permission per family (`governance_changes.rs`: `policy:read` → `Policy`, `tag:read` → `Tag`) and passes it to
`governance_changes_via`, which filters `FAMILIES` by it. `User` joins that loop with `user:read`. The route's
entry gate (`require_governance_read`: `policy:read` or `tag:read`) is unchanged: the feed stays a Governance
page, and `user:delete` rows are an extra family inside it for a reader who also holds `user:read`. A
`policy:read` reader without `user:read` sees no `user:delete` row; a `user:read`-only principal still gets 403
from the feed (it is not a Governance reader). Platform admins hold all three. The route's description and the
Overview client's Recent changes label (Task 8) follow. The service-account route keeps its own `service_account:delete` sink
event and adds `username` to it.

**API copy.** The `DELETE /users/{id}` 204 description becomes: "User hard-deleted along with sessions, group
memberships and grants. Their engine username is retired permanently: no user can be given it again, so refs and
policies naming it match nobody. To revoke access but keep the username, set `status` to `disabled` instead."

**UI: exactly what changes.**

1. `ControlPlaneUserDto` gains `username: string | null` (read-only; `null` until minted).
2. The delete dialog (`UsersPage.tsx` and `UserDetail.tsx`). The title is unchanged. The description becomes, when
   `username` is set: "Deletes this user, their sessions, group memberships and role grants. Their engine username
   **{username}** is retired for good: nobody can be given it again, including this person if they come back, so
   grants and policies naming it will never apply to anyone. To stop them signing in and keep their username,
   grants and history, disable them instead." When `username` is null, the second sentence is "They have no engine
   username yet." and the last sentence stays. If the user is active, a **Disable instead** button is passed as
   `children`: it closes this dialog and opens the existing disable confirmation. Typing `delete` and the Delete
   button are unchanged.
3. The disable dialog (`UserDetail.tsx`, and the bulk Disable confirmation on `UsersPage.tsx`) appends: "Their
   engine username stays theirs." For a platform admin it also appends: "While disabled they lose all-access on
   engines."
4. The `UsersPage` row menu adds **Disable** (active users) or **Enable** (disabled users) above **Delete**, with
   the same guards as the detail page (not yourself). Today the row menu offers only Impersonate and Delete, so a
   per-row disable needs the bulk bar.
5. Governance > People gains a **Reused names** tab (`?kind=reused`, platform admins only) for the report (§4.5).
   Governance > Overview renders the new finding. Recent changes renders `user:delete` as "Deleted user
   {username or email}".

### 4.5 The report (D3)

**Route.** `GET /api/control-plane/perm/username-reuse`, in `egret-perm-compile` (it reads `perm.*`, `policy.*`,
`users`, the ledger and `audit.audit_log`). Platform admins only (`Principal` holds `platform_admin` at platform
scope), because it shows emails and holders across workspaces. Anyone else gets 403. Read-only, `REPEATABLE READ`.

**Findings** (each one carries the username, where it is (grant, rule or engine-grant id with position and
effect; or policy id, name, service and `managed_by`), the current holder (id, email, `created_at`, ledger
`minted_at`), the evidence timestamp, and a verdict):

| kind | rule | verdict |
|---|---|---|
| `ref_predates_holder` | a `user` ref whose earliest governance event naming it (`"user:<ref>"` in that row's events), or else the parent row's `created_at` (and, for an engine grant, `updated_at`), is **older than the holder's `created_at`** | `definite` when an event or an engine grant's `updated_at` proves it; otherwise `possible` (a PATCH may have added the ref for the holder later: grants and rules have no `updated_at`) |
| `policy_predates_holder` | a policy body naming a held username whose `updated_at` is older than the holder's `created_at`, read live and from the 0208 baseline (below) | `definite` for `compiler`/`infra`; `possible` for `operator` (it may be a deliberate pre-provisioning) |
| `admin_policy_named_non_admin` | an `egret-admin` body naming someone who is not an active platform admin (or nobody), read from the 0208 baseline and live | `definite` (live rows should be empty once D2 is in; a live row means the sync failed) |
| `holderless_ref` | a `user` ref or hand-written `users[]` name with no holder, with its ledger row (`retired_at`, `minted_for`) and whether its position fails open (deny include, allow exclude, rule exempt exclude) | `dead` (retired, matches nobody, forever) |

**The 0208 baseline.** The boot sync (§4.2) and the recompiles rewrite exactly the bodies the report needs. So
0208 snapshots them once, before any of that runs, into `username_reuse_baseline(service_id, policy_id,
policy_name, managed_by, policy_updated_at, username, holder_id, holder_created_at, holder_is_active_admin,
taken_at)`. It keeps only the rows that are evidence: no holder, `updated_at < holder.created_at`, or an
`egret-admin` row naming a non-admin. After 0208 every rewrite is correct by construction, so no later snapshot is
needed.

**Attention signal.** The model fits (E19): one more `Source`, kind `username_reuse`, with a new pseudo-permission
`platform_admin` in `held` (inserted when the principal holds the role; listed in `notChecked` for everyone else).

- Severity `critical` when any `definite` finding exists; otherwise `warning` for `possible` findings.
- No finding when there are only `dead` refs, which `unresolved_principals_fail_open` already reports.
- Samples show the username and the place, never an email.
- `Ref::ReusedNames` links to `/governance/people?kind=reused`.

**No automatic action.** Nobody is disabled and no ref is edited (D3).

**What the report cannot see.**

- **Retention:** governance events older than `retention_days` are gone. Before this package, a human delete left
  no event at all. Evidence older than that is only the timestamps that remain.
- **Removed refs:** a stale ref that was later edited away, or a grant that was deleted, leaves no row, only events
  within retention.
- **The mint time of names held before 0208:** `minted_at` is NULL, so `users.created_at` is a lower bound. A ref
  added between creation and mint reads as `possible`.
- **Transient exposure** from stale role, workspace or clearance expansions (E6) between a delete and the next
  recompile. The recompile overwrote the body and left no trace, unless the 0208 baseline caught it still stale.
- **Free text:** a literal username in a row-filter or mask expression or in a condition (`current_user = 'bob'`).
- **Engine-side ownership:** table, view and schema owners in Iceberg, Trino or StarRocks, Trino `DEFINER` views
  that run as their owner, and Ranger `{OWNER}` items. Those live in the engine or catalog, not in egret.
- **Actual use:** whether an inherited access was ever exercised. That needs the engines' query logs (the Trino
  event listener, the StarRocks audit log), and egret may not retain them. The report links no log.
- **`egret-admin` bodies re-seeded before 0208** by an engine re-apply that already rewrote them.

### 4.6 Old engine JWTs of a deleted user

**Decision: no revocation list.** Session deletion plus the ledger is enough. The reasons:

- **The window.** A JWT minted before the delete stays valid for up to **900 s** after its mint (E17), plus the
  Ranger plugin's poll interval for any policy change the delete recompiles. Queries already admitted keep running.
- **No new token after the delete.** Sessions are deleted in the same transaction (E5). The minter refuses: the
  user row is gone, so `ensure_username` answers `NotFound`. The token cannot be refreshed (no `cgen`), and it never
  leaves egret's server-side executors.
- **The ledger closes the takeover half.** Nobody can hold the name afterwards, so the old token can only exercise
  what still names *that* name: the deleted person's own direct refs and hand-written `users[]` entries, for at most
  15 minutes. It never acts with a successor's grants, and no successor ever acts with its grants.
- **Disable has the same shape today.** That is accepted for Disable, and Delete is no worse.

A `jti` deny-list would shorten 15 minutes to the next request, at the cost of a store read on every engine
statement. That is not worth it for a token that only egret holds.

## 5. Principles

- Make reuse unrepresentable in the database. Guards in the callers are not enough.
- One definition of "engine admin", used by engine apply, the sync and the report.
- Every change that alters who is an admin goes through the recompile chokepoint. No path has its own copy.
- Report evidence. Change nothing the user did not ask to change.

## 6. Backend changes (enumerated)

1. `migrations/0208_username_ledger.sql`:
   - `username_ledger`, its five triggers, and `username_ledger_backfill()`;
   - `username_reuse_baseline`, filled once.
2. `egret-auth`:
   - `ensure_username_via` docs;
   - `hard_delete` (actor, event, `DeletedUser`);
   - `set_status_in_tx`;
   - `roles::engine_admin_user_ids_via`.
3. `egret-audit`: `action::USER_DELETE`, `resource::USER`, `FAMILIES`, `GOVERNANCE_ACTIONS`.
4. `egret-policy`: `sync_admin_all_access_in_tx`, `admin_policy_services_via`; `seed_admin_all_access` delegates
   to them.
5. `egret-perm-compile`:
   - the sync in `recompile_locked_service_in_tx`;
   - admin-policy arms in the role and group affected sets;
   - admins in `premint_model_usernames`;
   - the role and workspace arms in `lock_affected_by_user_delete_in_tx`;
   - `set_user_status_and_recompile`;
   - `sync_admin_policies` (boot);
   - the report route and query module;
   - the `principal_refs.rs:86-99` "known gap" doc corrected.
6. `egret-controlplane`:
   - `delete_user` passes the actor;
   - `update_user` uses `set_user_status_and_recompile`;
   - group role routes use the recompiling path;
   - `UserDto.username`;
   - the `username_reuse` source, the `platform_admin` pseudo-permission, `Ref::ReusedNames`.
7. `egret-dataapi`: the SA delete passes the actor and adds `username` to its event.
8. `egret-engine-mgmt`: `admin_usernames` uses `engine_admin_user_ids_via`.
9. `egret-bin`: the boot pass spawn; the `create-admin` sync.
10. `egret-core`: `engine_principals` docs (the delegator is reserved through the ledger).

## 7. Wire shapes

```json
{
  "generatedAt": "2026-10-23T10:00:00Z",
  "findings": [
    {
      "kind": "ref_predates_holder",
      "verdict": "definite",
      "username": "bob",
      "place": {"type": "grant", "id": "…", "position": "include", "effect": "allow"},
      "holder": {"id": "…", "email": "bob@new.example", "createdAt": "…", "mintedAt": null},
      "evidenceAt": "2026-07-01T09:00:00Z",
      "evidence": "governance event perm_grant:create named user:bob"
    }
  ],
  "baselineTakenAt": "2026-10-24T08:00:00Z"
}
```

`place.type` ∈ `grant | rule | engine_grant | policy`; a `policy` place carries `{policyId, name, service,
managedBy}`. The new attention kind is `username_reuse`, and the new `Ref` variant is `reused_names`.

## 8. Migration and compatibility

- **0208** is reserved (0207 belongs to another open branch). Re-check both on `origin/main` and on the open PRs
  before merging; renumber above both if needed.
- 0208 is additive. Its triggers take effect for every later write. On a fresh install, step 1 seeds from an empty
  `users`.
- Tests that write `users.username` directly keep working unless one binary deletes a user and re-inserts the
  **same** username. Those now fail with `23505`, which is the intended behaviour. The plan sweeps for them.

## 9. Risks

- **Seeding too much** (step 3 over compiler and infra bodies, step 4b over events) retires names that were never
  a person's. The cost is a `-2` suffix for a future person with that slug. That is accepted.
- **The sync inside every recompile of an admin-policy service** adds one query plus a body comparison to those
  recompiles. It writes nothing when the body is unchanged.
- **Group role writes now recompile** (E10). A clearing role granted to a group triggers an installation-wide
  recompile that changes nothing (role expansion ignores group grants). It is correct and idempotent; the cost is
  noted.

## 10. Related defects found (not fixed here)

- `guard_not_last_admin` counts disabled admins (E9). Disabling every admin but one, then disabling that one, is
  allowed. One line would fix it once `engine_admin_user_ids_via` exists; it is left for a follow-up because it
  changes a 409 contract.
- The alert and report owner identity mints and runs as a disabled owner (`evaluator.rs:147-180`). This was not
  verified end to end.
- Role-name takeover (a recreated role name inherits `role` refs; `principal_refs.rs:94-96`).

## 11. Non-goals

The seven other #692 follow-ups, all still open on `aec59807`:

1. the boot repair runs one transaction per profile, with retry (`role_membership.rs:251-292`);
2. lazy `policy:read` in `reference_detail` (`access/mod.rs:711-721`);
3. `old_services` taken from the fresh read in `write_grant`/`write_rule` (`routes.rs:1177-1212`, `:1440`);
4. a pre-mint driven by the referenced sets instead of the whole model (`members.rs:359-422`);
5. skip the pre-mint when `compile_desired` returns `None` (M2; `trigger.rs:1442-1445`, `:2183-2186`);
6. the profile set that grows after the early mint lock (`members.rs:304-312`);
7. the `view_hold` lock timeout matched on text (`crates/egret-catalog-engine/src/view_hold.rs:858-864`).

Also not here:

- purging or rewriting refs on delete;
- refusing a delete while the name is referenced;
- auto-disabling suspected takeovers;
- a `jti` deny-list;
- a separate namespace for `svc-` names;
- linking a returning person to their old username;
- free-text and engine-side ownership remediation;
- the Platform docs in `egret-data/docs` (D5: a separate docs PR after this one merges);
- a `docs/vi` mirror of the plan. The spec gets a Vietnamese reference translation at the end (D5).

## 12. Open questions (answered 2026-10-23)

**Q1. Who may read the `user:delete` event?** **Answered (D4): a separate `User` family, read with `user:read`.**
The text below is the question as it was asked. `record_change` requires a family in `FAMILIES`, and the family
decides the reader. With the default, `Policy`, every `policy:read` holder sees the deleted user's username and
email in Governance > Recent changes. The alternative is a third family, `User` (`user:read`), which needs a feed
filter change (`governance_changes_via` and the Overview client). Default: `Policy`, because the username is what a
policy reader needs to understand dead refs. Only Task 2 changes if the answer is `User`.

**Q2. Where do the Platform docs and the `docs/vi` copy go?** **Answered (D5):** the Platform docs are a separate
docs PR after the code PR merges, written by the controller (plan Task 9 is out of this PR). The `docs/vi` copy of
this spec is a Vietnamese reference translation, written after the code is done because the spec may still change
during implementation; its header says it is a reference, links to this file and says the English text wins.

## 13. Implementation notes (append-only)

Deviations from the text above that the code forced or that implementation found, recorded as they happen.

### Task 1 (the ledger)

- **The append-only guard is stricter than §4.1.** §4.1 lets `user_id` and `retired_at` change freely. The guard
  lets `user_id` change only to NULL and `retired_at` only from NULL. Setting `user_id` to another user would hand
  them the name: the record trigger returns early when the ledger row already names the writer as holder. A
  `BEFORE TRUNCATE` trigger is added too, because a `TRUNCATE users … CASCADE` would empty the ledger without
  firing any row trigger.
- **Two helper SQL functions** are added: `username_slug(email)` (step 4a's slug, compared with
  `slugify_localpart` by a test) and `policy_body_usernames(body)` (every `users[]` name of a body's six item
  lists, `{…}` macros excluded), shared by step 3 and the baseline. The report (Task 6) can reuse the second.
- **Step 4b also reads engine-grant events.** Engine-grant summaries name principals as `{"kind": "user", "ref":
  …}` objects, not `"user:<name>"` strings, so step 4b reads both shapes. It reads the governance rows of
  `perm_grant`, `perm_rule` and `engine_grant` only (the families that carry principals).
- **`slugify_localpart_for_tests`** is a `#[doc(hidden)] pub` wrapper in `egret_auth::users`, for the slug
  agreement test.
- **`lower()` and non-ASCII.** The SQL slug lowercases with the database collation, which can differ from Rust's
  Unicode `to_lowercase` for a few non-ASCII letters (for example U+212A KELVIN SIGN lowercases to ASCII `k` in
  Rust). Every non-ASCII character becomes `-` either way, and step 4a reads only service-account emails, which
  are built from an ASCII slug, so the difference cannot reach the ledger.
