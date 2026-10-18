# Group-name takeover and role-holder minting: design and plan

Date: 2026-10-18. Base: main `6225b5a8` (includes #685 J-F3, #688). Branch `fix/perm-group-names-role-mint`.
Two pre-existing holes in how egret-perm expands principals. The user approved fixing both in one package.

## Questions for the user

**Answered by the user 2026-10-18:** Q1 → refuse delete/rename with 409 (R1; R1-alt dropped). Q2 → do not create, warn (R2). Q3 → mint at compile (R5, R6). Q4 → background pass at startup (R7).


Each question has a default decision, and the plan is written for that default. The answer changes only the task
named.

1. **Refuse to delete or rename a local group while the permission model names it? (R1, Task 2)** Today both
   succeed. Every grant, deny, exclude and rule exemption that names the group then matches nobody, so a deny or an
   exclude silently stops applying. Default: `DELETE` and `PATCH /groups/{id}` answer **409** and list the grants
   and rules that name the group. The admin edits those first. A rename is also refused when the **new** name is
   still named by leftover references, because otherwise the group's members would pick up those grants. This
   changes behaviour an admin may rely on: deleting or renaming a group that is in use now takes an extra step.
   `group:delete`/`group:update` can be delegated to people who are not platform admins (E9), and for those people
   the 409 is the point. The alternative is **R1-alt**: a rename rewrites `perm.grant_principals.ref` and
   `perm.rule_exemptions.ref` from the old name to the new one, and a delete stays refused. That edits rows that a
   permission author wrote, and the person doing it is a group admin who may not be allowed to author permissions.
   **Refuse both (default), or rewrite on rename?**
2. **Refuse to create an OIDC group whose name is still in the model but has no group row? (R2, Task 1)** Default:
   at login, a claimed name that has no `groups` row and that a stored grant principal or rule exemption names is
   **not created**. The user is not joined to it, the login goes ahead, and a `warn!` names the group. This changes
   behaviour an admin may rely on: an IdP group that has the same name as a deleted local group, put there on
   purpose so that it "takes over" the old grants, no longer gets them. To keep the old grants, remove the leftover
   references, let the next login create the group, and then name it again. Either that, or create a local group
   of that name, which R4 allows. **Confirm.**
3. **Mint usernames for role holders at compile time? (R5, R6, Task 3)** Default: yes, the same way group and
   workspace expansion already do. This **widens access to match egret's model**. A holder of a clearing role who
   has never used an engine is now listed in that clearance's `denyExceptions` straight away. Before, they were
   refused until the next recompile after their first query. Cedar already clears them (E15). The same applies to a
   role in a grant's `include`. It also writes `users.username` for those holders earlier than any query would.
   **Confirm.**
4. **Repair policies that are already compiled, at startup? (R7, Task 4)** Default: a best-effort background pass
   at boot recompiles every profile whose model names a role (in a grant principal or a clearance's
   `required_roles`), through the normal locked recompile. This **changes existing data**: it mints usernames and
   rewrites compiled bodies that left out a holder. It is idempotent, so a profile that did not change writes
   nothing (E16). The alternative is to report only and have operators run `POST /perm/compile/{service_id}` for
   each profile. **Boot pass (default), or manual?**

## Problem

### A. A group name left behind in the model can be claimed by the IdP (security)

Grants and rule exemptions name a group by NAME, and nothing links them to `groups` (E1). The compiler expands a
group name into the members of whichever group has that name now (E2). Deleting or renaming a local group leaves
every stored reference to the old name in place, and those references then expand to nobody (E3). Later, an IdP
group claim with that name creates an OIDC group (E4). The next login syncs its members (E5), and that recompiles
every policy that names it. The new group now has every allow, every subtraction from a deny (an `exclude`), and
every mask exemption that was written for the old group. Whoever controls group names in the IdP picks up grants
written for a group that no longer exists.

The same leftover reference is also a hole without any IdP involved:

- A deny include or an allow exclude on a deleted group stops applying to its former members (E3).
- Renaming a group that has members INTO a name with leftover references gives those members the old grants. A
  rename needs only `group:update`, which a non-admin can be delegated, while membership is platform-admin only
  (E9).

### B. A role holder without a username is left out until the next recompile (security, fails open)

`role_usernames` reads `users.username` and never mints one (E10). A holder who has no username yet is logged and
left out of every role item: a role deny's `include`, a role in an allow's `exclude`, a clearance's
`denyExceptions`. Their username is minted later by a path that does not recompile (E11). They then exist on the
engine, and the stale policy still does not name them. Wherever another item gives them access, the role deny or
role exclude does not apply to them until something else recompiles that profile.

The doc comment's argument that this is safe ("cannot appear in an include item either", E10) does not hold. A
`public` allow names everyone (E12), and so does any allow that does not depend on the username existing. Group
and workspace expansion already mint at compile time for exactly this reason (E13).

## Evidence (verified at `6225b5a8`)

| id | fact | where |
|---|---|---|
| E1 | `perm.grant_principals` and `perm.rule_exemptions` store `(kind, ref text, mode)`. There is no FK to `groups` | `migrations/0154_perm_model.sql:46-50`, `:99-103` |
| E2 | Group expansion is `user_groups ⋈ groups ⋈ users WHERE g.name = ANY($1)`, matched by exact text (`groups.name` is `text UNIQUE`, `0002_identity.sql:15-19`) | `crates/egret-perm-compile/src/members.rs:379-394` |
| E3 | `rename` and `delete_local` each take the per-group lock, lock the affected profiles, write and recompile. Neither checks for references. Their docs say the old name "expands to nobody" afterwards. The J-F3 test `deleting_or_renaming_a_group_recompiles` pins exactly that ("the old name now names nobody") | `crates/egret-auth/src/groups.rs:472-528`, `:530-576`; `crates/egret-perm-compile/tests/group_grants.rs:643-697` |
| E4 | `provision_and_sync` calls `upsert_oidc_group` for each claim, on the pool. There is no lock and no check: `INSERT … ON CONFLICT (name) DO UPDATE SET name = groups.name` | `crates/egret-auth/src/oidc.rs:49-51`, `groups.rs:288-297` |
| E5 | The sync joins only names that already have an OIDC row (`… AND source = 'oidc'`), so a name that has no row is not joined | `groups.rs:387-389`, `:437-440` |
| E6 | At write time a group ref must have a `groups` row (`lookup`: `SELECT EXISTS … groups WHERE name = $1`; `public` is exempt). So a stored ref with no row can only come from a later delete or rename, or from a row stored before J-F3 | `crates/egret-perm-compile/src/principal_refs.rs:214-229` |
| E7 | **The write path validates before it locks.** `write_grant` resolves refs at `:1104` and takes the group locks at `:1136`. `write_rule` does the same at `:1271` and `:1320`. So a grant that validated `G` can wait on `G`'s lock while a delete removes `G`, and then store a ref to a group that no longer exists. The migration path takes the locks first (`migrate.rs:965`) and then goes through the same writers | `crates/egret-perm-compile/src/routes.rs:1104`, `:1136-1140`, `:1271`, `:1320` |
| E8 | `GET /perm/unresolved-principals` already lists every stored group ref that has no row (`status: "unresolved"`, reason "no group has this name"). It marks `failsOpen` only for deny-include, allow-exclude and rule-exclude positions, and the startup `warn!` logs only those. An allow include on such a name is not flagged, yet it is the position a takeover turns into a grant | `principal_refs.rs:392-466`, `:471-484`, `:514-540` |
| E9 | `create_group`, `rename_group` and `delete_group` require `group:create` / `group:update` / `group:delete`. Membership requires `group:manage_members` (OpenAPI: "Requires platform_admin"). The console says lifecycle "stays delegatable". Rename and delete map `rows == 0` to 409 "OIDC-synced". Both dialogs show the server's error message | `crates/egret-controlplane/src/access/mod.rs:612`, `:646`, `:679`, `:739-751`, `:653-657`, `:685-689`; `frontend/src/routes/control-plane/access/GroupsPage.tsx:73-76`, `:101`, `:480` |
| E9b | No path deletes an OIDC group: the only `DELETE FROM groups` is `delete_local` (`source = 'local'`). Clearance `required_groups` is `uuid[]`, and `access_grants` / `platform_role_assignments` hold groups by id. So these cannot be taken over by name | `groups.rs:564`; `migrations/0083_classification_required_groups.sql:13`; `0068_access_grants.sql:7-8`; `0003_workspace_rbac.sql:35-40` |
| E10 | `role_usernames` selects holders from `access_grants`, with no join to `users`, and resolves names through `usernames_via`, which only reads. A holder with no username gets a `warn!` and is left out. The doc argues that is "the right answer" | `members.rs:146-217` |
| E11 | The only writer of `users.username` is `ensure_username_via` (`users.rs:467`). Its callers outside the compiler are the first query (`egret-query/src/execution/mod.rs:64`, `:84`), the engine token minter (`egret-bin/src/engine_minter.rs:74`), BI (`egret-bi/src/routes.rs:2169`), the automation evaluator (`egret-automation/src/evaluator.rs:160`) and the engine admin (`egret-engine-mgmt/src/engines.rs:2340`). None of them recompiles. (The People "allocate" route belongs to the in-flight #2 branch, not to main, and has the same shape) | — |
| E12 | `public` is a valid group ref and is never expanded. Every engine resolves it for every user | `principal_refs.rs:217-219`; `members.rs:247-254` |
| E13 | Workspace and group expansion mint missing usernames on the compile's connection, in ascending user-id order, before the role read. The doc relies on that order so that a person reached by both kinds gets the same name | `members.rs:396-427`, `:319-328`, `:474-478` |
| E14 | `role_usernames` has three callers. Two are compiles: `expand_enumerated_principals` (`members.rs:478`) and `compile_clearance` (`clearance.rs:257`). One is a read: `view_audit::owner_cleared` (`view_audit.rs:234`), served from `view_exposure.rs:598` and `governance_lineage.rs:356`. `owner_cleared` compares against a given username, so a holder with no username can never be the owner | — |
| E15 | Clearance: the deny names `{public}`, and `denyExceptions` is the union of the clearing roles' holders (`clearance.rs:257`, `:283-308`). Cedar's `classification_forbid` clears anyone who holds a required role, whether or not they have a username (`egret-auth/src/pdp/mod.rs:169-192`) | — |
| E16 | Compiled bodies are diffed byte for byte. A recompile that changes nothing writes nothing | `members.rs:157-159`, `:439-442` |
| E17 | The role-membership trigger (`affected_services_for_role_membership`) returns the union of the clearance arm (installation-wide) and the grant arm (by catalog). It is the set a role change recompiles | `crates/egret-perm-compile/src/trigger.rs:1505-1539` |
| E18 | Lock order (J-F3): user advisory locks, then group advisory locks (two-int4, `hashtext(name)`), then profile locks ascending, then rows. A grant or rule write starts at the group step. A writer re-reads after locking until the set stops changing | `groups.rs:84-106`, `:162-206` |
| E19 | `ensure_username_via` returns `NotFound` for a missing user. In a race with a concurrent delete, it fails on the re-read (`fetch_one`). The group-mint path has the same race today | `crates/egret-auth/src/users.rs:426-505` |
| E20 | Implementors of `GroupMembershipRecompiler`: `PermGroupRecompiler` (`group_membership.rs:93`), the test `Recorder` (`egret-auth/tests/support/recorder.rs:41`), `CreatesGroupMidSync` (`egret-auth/tests/oidc_db.rs:278`), `NoProfiles` (`egret-auth/src/users.rs:607`) and `Pausing` (`egret-perm-compile/tests/support/pausing.rs:76`) | — |

## Decisions

| id | decision | by |
|---|---|---|
| R1 | **Refuse delete/rename of a local group that the model names.** After `delete_local` / `rename` take their group locks (and before the profile locks), they read every grant principal and rule exemption whose `kind = 'group'` and whose `ref` is the group's name. For a rename, both the old and the new name are checked. Any hit returns `Error::Conflict` with the list (grant id with effect and position, or rule name with position; at most 20 entries plus a count of the rest). Nothing is written and nothing is recompiled. `public` is never counted, because a ref to it does not depend on a row. The check runs under the group lock, and every model writer takes that lock for every group it names (E7, after R3), so the answer cannot go stale before commit. **The entries are listed only to a caller who holds `policy:read`**: `group:delete` / `group:update` can be granted alone, and grant ids, effects and rule names are policy metadata. Anyone else gets the count and "ask an admin with policy:read and policy:update to remove them first" (default decision 2026-10-18, review M-2) | default decision (2026-10-18); **Q1** |
| R1-alt | (Only if Q1 says rewrite.) A rename runs `UPDATE perm.grant_principals / perm.rule_exemptions SET ref = $new WHERE kind = 'group' AND ref = $old` under the same locks. A row that already names `$new` in the same mode is collapsed first (the PK is `(id, kind, ref, mode)`), then the affected profiles are recompiled. A delete stays refused (R1) | dropped unless Q1 chooses it |
| R2 | **The IdP cannot create a group whose name the model still names.** `provision_and_sync` replaces its `upsert_oidc_group` loop with `GroupRepo::upsert_claimed_oidc_groups(names, port)`. It opens one transaction, takes the group locks for all claimed non-`public` names, sorted (group step only: no user lock, no profile lock), and reads which names already have a row. For the rest it asks the port which are referenced (R8) and inserts only the unreferenced ones (`ON CONFLICT (name) DO NOTHING`). It returns the refused names, and the login logs one `warn!` for each (`group = ?name`, Debug format like R2 of 2026-10-17). The login is not refused. The sync then ignores those names, because they have no OIDC row (E5). An existing row, whether local or OIDC, behaves as it does today | default decision (2026-10-18); **Q2** |
| R3 | **Close the validate-then-lock window (E7).** `write_grant` and `write_rule` call `lock_named_groups` **before** `resolve_principal_refs`. The group names are known from the body before resolution, which only rewrites user refs. The lock order does not change: group locks still come before profile locks. Without this, R1 can approve a delete while a grant that has already validated waits to store a ref to the deleted name | default decision (2026-10-18) |
| R4 | `create_local` with a name the model already names is **allowed**, with one `warn!`. The new group has no members, and only a platform admin (`group:manage_members`) can add any, and a platform admin already controls every group's membership. A rename INTO such a name is refused (R1), because a rename brings its members along and needs only `group:update` (E9) | default decision (2026-10-18) |
| R5 | **Role expansion mints, at the same point as groups.** `role_usernames` gains a `MissingUsername::{Mint, Omit}` argument. With `Mint`, it reads holders joined to `users` (`JOIN users u ON u.id = ag.principal_id`, like E2 and E13, which also skips the `access_grants` rows of deleted users) and mints the missing usernames on the caller's connection in ascending id order with `ensure_username_via`. Both compile callers (`expand_enumerated_principals`, `compile_clearance`) pass `Mint`. `view_audit::owner_cleared` passes `Omit`: it is served on GET routes, and it compares against a username it was given (E14), so reading only is exact there. One predicate for all callers, so the compile and the audit cannot disagree about who holds a role | default decision (2026-10-18); **Q3** |
| R6 | **Clearance:** minting means a clearing-role holder with no username is excepted from the deny at compile time. Access widens to Cedar's answer (E15). It only removes a fail-closed window: the next recompile after their first query would have excepted them anyway | default decision (2026-10-18); **Q3** |
| R7 | **Repair existing bodies.** A boot task, off the boot path like `warn_fail_open_principals` (`egret-bin/src/main.rs:2179-2192`), collects every role named by `perm.grant_principals (kind = 'role')` or by any `classification_policies.required_roles`. It takes the union of `affected_services_for_role_membership` over those roles (E17), and recompiles that set in one transaction with `lock_services_in_tx` + `recompile_services_in_tx`. It is idempotent (E16). A failure is logged and never blocks boot. It runs on every boot, because no record says whether it has run and the cost of an unchanged profile is one compile with no write | default decision (2026-10-18); **Q4** |
| R8 | **One new port method** on `GroupMembershipRecompiler`: `group_references_in_tx(tx, names) -> Result<Vec<GroupReference>>`. `GroupReference { group_name, source: "grant"\|"rule", id, rule_name: Option<String>, effect: Option<String>, position }` is defined in `egret-auth`. `PermGroupRecompiler` implements it with the same two tables `affected_services_for_group_membership` reads (`trigger.rs:1687-1730`), with `public` filtered out. R1 and R2 both use it, and egret-auth does not learn the `perm` schema. The method is required, with no default body, as the trait's module doc demands (`groups.rs:40-44`) | default decision (2026-10-18) |
| R9 | Docs that state the old premise are corrected at the site. `principal_refs.rs:80-84` ("a principal deleted after it was validly written is not a defect"): a delete is now refused while referenced, and a leftover reference is protected from takeover by R2. The `rename`/`delete_local` docs. The role-expansion docs `members.rs:21-32`, `:146-155`, `:319-328` and the warn text at `:203-209` / `:562-571` | default decision (2026-10-18) |
| R10 | **No migration.** If a later round needs one, `0195` is the next free number on `origin/main` (latest `0194_workflow_task_run_working_dir_snapshot.sql`, checked 2026-10-18). Re-check before merging | default decision (2026-10-18) |

### Not in scope (recorded, verified)

- **An OIDC group renamed or removed in the IdP.** Its row is never deleted (E9b). Each member's next login drops
  their membership (`groups.rs:420-427`). Until then, references to the old name still expand to members who have
  not logged in again. That is stale membership, which predates this package, not a name takeover. The fix needs a
  sweeper or SCIM.
- **A role held through a group** is still not expanded (`members.rs:74-88`, narrowing 1). That is fail-open for a
  role deny, and it is recorded there.
- **Clearance `required_groups`** and `access_grants` hold groups by uuid (E9b). A deleted group's id never comes
  back, so they cannot be taken over.
- **A concurrent user delete during a mint** fails the compile (E19). The group path has the same behaviour today:
  it fails closed and a retry succeeds. Unchanged.
- **Existing leftover references** are not rewritten or deleted. They stay listed by `GET
  /perm/unresolved-principals` (E8), and R2 makes them inert against the IdP.

## Global Constraints

- **Build isolation:** `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-fu2/target`.
  Never use another checkout's `target/`. Build with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
  CARGO_PROFILE_TEST_DEBUG=0`. Keep at least 30 GiB of disk free before building.
- sqlx 0.9: `query()` takes `&'static str`. No SQL built with `format!`.
- Tests use `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` only, **never** the database `egret`.
  Run with `env -u EGRET_ALLOW_DB_SKIP`, and never set `EGRET_ALLOW_DB_SKIP=1` (a skipped DB test reports `ok`).
  Scope assertions to the ids a test creates. Run suites in the foreground, one at a time.
- OpenAPI: R1 changes the 409 descriptions on `PATCH`/`DELETE /groups/{id}`. Regenerate with `cargo build -p
  egret-bin && target/debug/egret openapi > openapi.json` and `cd frontend && npm run gen:api` (never bare
  `orval`). `committed_openapi_json_is_current` must pass.
- Never run `git stash`, `git checkout --` or `git clean`. Never commit `.superpowers/`. Commit as
  hoangquocvietbk2.13@gmail.com, with the trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Verify every new test by mutation: break the fix, watch that test fail for the right reason, restore with `cat
  backup > file`, and check that `git status --porcelain` shows only the intended files.

## Plan

### Task 1: the port method, and no IdP takeover (R2, R8)

**Files:** `crates/egret-auth/src/groups.rs` (trait method, `GroupReference`, `upsert_claimed_oidc_groups`),
`crates/egret-auth/src/oidc.rs`, `crates/egret-perm-compile/src/group_membership.rs`, every implementor in E20,
`crates/egret-auth/tests/oidc_db.rs`, `crates/egret-perm-compile/tests/group_grants.rs`.

- Add the method and type (R8). Implement it in `PermGroupRecompiler` with one `UNION ALL` over
  `perm.grant_principals ⋈ perm.grants` and `perm.rule_exemptions ⋈ perm.rules`, with `kind = 'group'`, `ref =
  ANY($1)` and `ref <> 'public'`, ordered. The test doubles return a configured list.
- `upsert_claimed_oidc_groups` as R2. `upsert_oidc_group` stays for `egret-bin/src/lib.rs:38` (an operator
  command that provisions a named group on purpose) and for tests. Its doc says that login must not use it.
- `oidc.rs:49-51` calls the new function and logs each refused name.
- Tests (DB):
  - A grant that names a deleted group `G` (stored with `PermStore` directly), then a login claiming `G`: no
    `groups` row, no membership, and the grant's compiled body unchanged (still no item).
  - The same login when nothing names `G`: the group is created and joined, as today.
  - A claim naming an existing OIDC group that a grant names: joined and recompiled, as today. Refs to an existing
    row never block.
  - A claim `public` with an allow on `public` stored: created as today (not counted).
  - The port returns grant and rule references with the right effect and position.
- Mutation: skip the reference check, and the first test must fail on "no `groups` row".

### Task 2: refuse delete/rename while referenced, and lock before validating (R1, R3, R4, R9)

**Files:** `crates/egret-auth/src/groups.rs` (`rename`, `delete_local`, `create_local` warn),
`crates/egret-controlplane/src/access/mod.rs` (409 descriptions), `crates/egret-perm-compile/src/routes.rs`
(`write_grant`, `write_rule`), `crates/egret-perm-compile/src/principal_refs.rs` (doc),
`crates/egret-perm-compile/tests/group_grants.rs`, `crates/egret-perm-compile/tests/group_races.rs`,
`crates/egret-auth/tests/groups_db.rs`, `crates/egret-controlplane/tests/routes_api.rs`, `openapi.json`, the
generated client.

- In `delete_local`: after the lock loop, `group_references_in_tx(&[name])`. If it is not empty, return `Conflict`
  with the R1 sentence ("group `x` is named by 3 permission entries; remove them first: grant <id> (deny,
  include), rule `mask_email` (exempt include), …"). Do the same in `rename` for `[old, new]`, naming which name
  each entry refers to. Both checks run before `lock_affected_in_tx`.
- `write_grant` and `write_rule`: move `lock_named_groups` above `resolve_principal_refs`, and keep a comment
  citing E7.
- `create_local`: after the insert, one `warn!` if the name is referenced (one read on the pool; log only).
- Rewrite `deleting_or_renaming_a_group_recompiles` (`group_grants.rs:643`), because it pins the old behaviour:
  delete and rename of a referenced group now return `Conflict`, and the bodies are unchanged. Then remove the
  grants, and the delete and rename succeed and recompile.
- New tests:
  - Rename into a name only a leftover grant names: Conflict, and the members are not added to that grant's body.
  - The route answers 409 with the entry in the message.
  - Race (`group_races.rs`, the `Pausing` double pattern): a grant on `G` paused after validation, a delete of `G`
    run concurrently. Either the delete answers 409 or the grant fails validation. Never a stored ref to a missing
    `G`. Run it on the base first to show that it fails there (E7).
- Mutation: drop the new-name half of the rename check, and the rename-into test must fail. Put the lock back after
  validation, and the race test must fail.

### Task 3: role expansion mints (R5, R6, R9)

**Files:** `crates/egret-perm-compile/src/members.rs`, `crates/egret-perm-compile/src/clearance.rs`,
`crates/egret-perm-compile/src/view_audit.rs`, doc touch-ups in `trigger.rs:1517`, `role_membership.rs:29`,
`principal_refs.rs:63`, `egret-workspace/src/members.rs:173` and `egret-auth/src/users.rs:548`,
`crates/egret-perm-compile/tests/role_grants.rs`, `crates/egret-perm-compile/tests/clearance.rs`.

- `role_usernames(conn, roles, MissingUsername)`, as R5. With `Mint`, the `warn!` for a missing username goes
  away, and an `info!` per mint replaces it, as at `members.rs:420-425`. With `Omit`, the current behaviour stays.
- Tests (DB), each with a holder seeded with `username = NULL`:
  - Allow `public` on a table plus deny role `R` (holder `h`): the compiled deny names `h`'s minted username, and
    `users.username` is now set.
  - Allow `public` with exclude role `R`: the exclude names `h`.
  - A clearance gated on `R`: `denyExceptions` contains `h`'s minted username.
  - `owner_cleared` with `h` still unminted: it does not mint (`users.username` is still NULL) and answers as
    before.
  - The minted name is the one `ensure_username` later returns (the stable-allocation property).
- Audit before changing: `grep` the perm-compile, bi and controlplane tests for users seeded without a username who
  hold a role named in a model, and update any expected list that left them out. None were found while writing
  this spec (`role_grants.rs` and `clearance.rs` seed every holder with a username), but the grep must run on the
  branch.
- Mutation: pass `Omit` from `expand_enumerated_principals`, and the deny test must fail. Pass `Omit` from
  `compile_clearance`, and the clearance test must fail.

### Task 4: repair at boot (R7)

**Files:** `crates/egret-perm-compile/src/role_membership.rs` (new `pub async fn
recompile_role_referencing_profiles(pool) -> Result<Vec<i64>, CompileError>`), `crates/egret-bin/src/main.rs`
(spawned beside `warn_fail_open_principals`), `crates/egret-perm-compile/tests/role_grants.rs`.

- Test (DB): compile a role deny with an unminted holder on the old path (set the body by hand to what the old
  compiler wrote, without `h`). Run the function. The body now names `h`, and the returned ids include that
  profile. A second run writes nothing (the profile's `policy_version` does not change).

### Gate

Whole-branch review. Then the full local CI: `cargo fmt --all -- --check`, workspace clippy,
shipping-resolution, `cargo test --workspace` (database as in Global Constraints), `just test-ranger-gates`, and
frontend lint/tsc/vitest/build/bundle (only the generated client changes). Check
`committed_openapi_json_is_current`.

Live check on kind, short:

1. Delete a group named in a grant from the console: the dialog shows the 409 list.
2. Log in through Keycloak with a group claim equal to a leftover reference: no group row appears.
3. A user with a clearing role and no username runs a query on a tagged column right after the role grant, with no
   recompile in between: the query is allowed. Before this change it was refused until a recompile.

PR notes:

1. `DELETE`/`PATCH /groups/{id}` answer 409 while the model names the group.
2. An IdP claim naming a referenced name with no group row does not create the group.
3. Role holders are minted at compile time, and clearance exceptions now include holders who have never used an
   engine.
4. A boot task recompiles profiles that name a role.
