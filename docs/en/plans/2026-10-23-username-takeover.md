# Username takeover after user delete: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A username is never held by two people over time, `egret-admin` names exactly the active platform admins
on every engine at every commit, a user delete recompiles everything that named them and leaves a record, and an
admin can see past takeovers.

**Architecture:**

- An append-only `username_ledger`. Triggers on `users` maintain it, and migration `0208` seeds it.
- `egret-admin` sync inside the single recompile chokepoint, plus admin-policy arms in the role and group affected
  sets, a status path and a boot pass.
- `hard_delete` gains the role and workspace arms and a `user:delete` governance event.
- A read-only report route, an Overview source, and frontend copy.

**Tech stack:** Rust (axum 0.8, sqlx 0.9, utoipa), PostgreSQL triggers, React 18 + TanStack Query + an
orval-generated client, Vitest, Playwright.

**Spec:** `docs/en/specs/2026-10-23-username-takeover-design.md`. Read §4 first. Every `file:line` there was
checked on `aec59807`. Re-check each one with `grep -n` before editing, because line numbers drift.

## Global Constraints

- **Worktree:** `egret-takeover`, branch `fix/username-takeover`. Work only there.
- **Build isolation:**
  - Use this worktree's own target dir: `export CARGO_TARGET_DIR=$PWD/target` from the worktree root. Never use
    another checkout's `target/`.
  - Build with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.
  - Keep at least 30 GiB of disk free; run `./deploy/prune-target.sh` before the gate.
- **Machine:**
  - No `rg`: use `/usr/bin/grep`.
  - zsh: quote globs (`--include='*.rs'`).
  - Never run `git stash`, `git checkout --` or `git clean`.
  - Use `pgrep -x`, never `pgrep -f`.
- **SQL:** sqlx 0.9 `query()` takes `&'static str`. No `format!` SQL.
- **Migration number:** `0208` is reserved for this package (`0207` belongs to another open branch). Before
  merging, check `origin/main` and the open PRs. If either holds 0208, renumber above both.
- **DB-gated tests must run, not skip:**
  - Use `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`, run under `env -u EGRET_ALLOW_DB_SKIP`.
  - After each run, `grep -c "skip: DATABASE_URL"` must print `0`.
  - Never point tests at the database `egret`.
  - Run suites one at a time, in the foreground or backgrounded. Never run two concurrently.
- **Red tests are written from the attacker's side:**
  - the NEXT person whose email slugs to a freed name;
  - a demoted, deleted or disabled admin (now an unprivileged principal).

  Each one must fail on `aec59807` for the stated reason before the fix. Mutation-check every new test: break the
  fix, watch it fail, restore with `cat backup > file`, and confirm `git status --porcelain` lists only intended
  files. Destructive red tests run only on the throwaway per-process test DB.
- **Engine-level assertions** evaluate the compiled or infra bodies with `egret-ranger-eval` (as the existing
  `tests/clearance*.rs` and `engine_grants_compile.rs` do). A JSON substring check is not an engine-level
  assertion.
- **OpenAPI:**
  - After any route or schema change, run `cargo run -q -p egret-bin --bin egret -- openapi > openapi.json`.
  - Then run `cd frontend && npm run gen:api` (never bare `orval`) and commit both.
  - `committed_openapi_json_is_current` must pass.
- **Commits:**
  - Conventional Commits, scope `perm` unless stated.
  - Trailer exactly `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
  - No `Closes #N` in commits. No scratch files in git. No `/Users/...` paths in committed files.
  - Do not push until the user says so.
- **Stacks:** the demo stack on `127.0.0.1:18080` belongs to another session. Never stop, reset or seed it. The
  live check uses its own compose project, `egret-takeover`, on port 18380.
- **Docs rule (`CLAUDE.md`):** this changes user-visible UI, API and behaviour. Task 9 updates the Platform docs in
  `egret-data/docs`. Confirm that repo's location with the user before editing it.

## File structure

| file | change | task |
|---|---|---|
| `migrations/0208_username_ledger.sql` | new: ledger, triggers, `username_ledger_backfill()`, `username_reuse_baseline` | 1 |
| `crates/egret-auth/tests/username_ledger.rs` | new | 1 |
| `crates/egret-auth/src/users.rs` | docs of `ensure_username_via`; `hard_delete` (actor, event, `DeletedUser`); `set_status_in_tx` | 1, 2, 5 |
| `crates/egret-audit/src/governance.rs` | `USER_DELETE`, `USER`, `FAMILIES`, `GOVERNANCE_ACTIONS` | 2 |
| `crates/egret-perm-compile/src/group_membership.rs` | role and workspace arms in `lock_affected_by_user_delete_in_tx` | 2 |
| `crates/egret-perm-compile/tests/user_delete_recompile.rs` | new | 2 |
| `crates/egret-controlplane/src/access/mod.rs` | actor to `hard_delete`; `UserDto.username`; status path; group role routes | 2, 5, 7 |
| `crates/egret-dataapi/src/routes.rs` | actor to `hard_delete`; `username` in SA event | 2 |
| `crates/egret-auth/src/roles.rs` | `engine_admin_user_ids_via` | 3 |
| `crates/egret-policy/src/default_policies.rs` | `sync_admin_all_access_in_tx`, `admin_policy_services_via`; `seed_admin_all_access` delegates | 3 |
| `crates/egret-engine-mgmt/src/engines.rs` | `admin_usernames` uses the shared set | 3 |
| `crates/egret-perm-compile/src/trigger.rs` | sync in `recompile_locked_service_in_tx`; admin-policy arms | 4 |
| `crates/egret-perm-compile/src/members.rs` | admins in `premint_model_usernames` | 4 |
| `crates/egret-perm-compile/tests/admin_policy_sync.rs` | new | 3, 4, 5 |
| `crates/egret-perm-compile/src/role_membership.rs` | `set_user_status_and_recompile`, `sync_admin_policies` | 5 |
| `crates/egret-bin/src/main.rs` | boot spawn; `create-admin` sync | 5 |
| `crates/egret-perm-compile/src/username_reuse.rs`, `src/routes.rs` | new report module and route | 6 |
| `crates/egret-perm-compile/tests/username_reuse_report.rs` | new | 6 |
| `crates/egret-controlplane/src/governance_overview.rs` | `username_reuse` source, `platform_admin` pseudo-permission, `Ref::ReusedNames` | 6 |
| `frontend/src/routes/control-plane/access/{UsersPage,UserDetail}.tsx` (+ tests) | copy, Disable instead, row Disable/Enable | 7 |
| `frontend/src/routes/governance/people/{PeopleIndex,ReusedNames}.tsx`, `overview/refUrl.ts`, Recent changes | Reused names tab, ref, label | 8 |
| `crates/egret-perm-compile/src/principal_refs.rs`, `crates/egret-core/src/engine_principals.rs` | correct "known gap" and "not yet reserved" docs | 1, 4 |

## Task 1: The ledger (spec §4.1)

**Files:**

- create `migrations/0208_username_ledger.sql` and `crates/egret-auth/tests/username_ledger.rs`;
- modify the docs of `crates/egret-auth/src/users.rs` `ensure_username_via`, `crates/egret-perm-compile/src/principal_refs.rs:86-99` and `crates/egret-core/src/engine_principals.rs:18-29`.

- [ ] **Step 1: Red tests, as the next holder.** In `username_ledger.rs`, using the `egret_test_db` per-process
  DB and unique emails per test:
  1. `the_next_person_never_gets_a_deleted_users_name`:
     - create `bob_<t>@a`, `ensure_username` → `bob_<t>`;
     - `hard_delete` it;
     - create `bob_<t>@b`, `ensure_username` → must be `bob_<t>-2`. On `aec59807` it is `bob_<t>`.
  2. `a_direct_write_of_a_retired_name_is_refused`: after the delete above, `UPDATE users SET username =
     'bob_<t>' WHERE id = <new>` → `23505`.
  3. `a_name_named_only_by_a_stale_ref_is_never_minted`:
     - insert a `perm.grant_principals` `user` ref `carol_<t>` with no holder;
     - run `SELECT username_ledger_backfill()`;
     - mint for `carol_<t>@x` → `carol_<t>-2`.
  4. Backfill step 3, the same shape for an `operator` policy `users[]` name, and for an `infra` `egret-admin` body
     name.
  5. Backfill step 4a: insert a `service_account:delete` audit row with email `svc.rep_<t>@ws.service.egret.local`,
     backfill, then mint for a new SA email with that local part → `svc-rep_<t>-2`.
  6. `the_delegator_name_is_reserved`: mint for `egret_delegator@x` → `egret_delegator-2`.
  7. `the_ledger_is_append_only`:
     - `DELETE FROM username_ledger` → error;
     - `UPDATE … SET username = …` → error;
     - `UPDATE … SET minted_for = …` → error.
  8. `a_holder_resetting_their_own_name_is_allowed`: `UPDATE users SET username = username` succeeds.
  9. `a_mint_racing_a_delete_cannot_take_the_name`:
     - connection A: `BEGIN`, `hard_delete`'s row delete of `dan_<t>`, not committed;
     - connection B: a task minting for `dan_<t>@b`;
     - commit A → B returns `dan_<t>-2`.
  10. `sql_slug_matches_slugify_localpart`: compare the backfill's SQL slug expression with `slugify_localpart`
      (make it `pub(crate)` plus a `#[doc(hidden)] pub` test hook if needed) over a corpus: `A.B`, `a..b`, `-a-`,
      `ü@x`, `a_b`, `""`, `a+tag`, `x--y`.
  11. The ledger row records `minted_for`, `minted_at` and `source = 'mint'`. After the delete, `user_id IS NULL`
      and `retired_at IS NOT NULL`.

  Run: `env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=… cargo test -p egret-auth --test username_ledger`. Expected: no
  compile errors; every test fails (no table, or the reuse succeeds).
- [ ] **Step 2: Migration.** Write `0208` per spec §4.1:
  - the table;
  - `username_ledger_record()` with the `AFTER INSERT` and `AFTER UPDATE OF username` triggers. Use `WHEN
    (NEW.username IS NOT NULL)` on insert and `WHEN (NEW.username IS DISTINCT FROM OLD.username)` on update. The
    update trigger also retires `OLD.username`;
  - `BEFORE DELETE ON users` → `retired_at = now()`;
  - the append-only guard on `username_ledger`;
  - `username_ledger_backfill()` (steps 1–5, each `ON CONFLICT DO NOTHING`; step 4b uses `jsonb_path_query(detail,
    'strict $.** ? (@.type() == "string" && @ starts with "user:")')` over governance mutation rows);
  - `SELECT username_ledger_backfill();`;
  - the `username_reuse_baseline` table, filled once (spec §4.5). The admin test mirrors `platform_admin_user_ids`
    plus `status = 'active'`.

  Header comments must explain every step and the reason for the trigger (no check-then-insert race).
- [ ] **Step 3: Docs at the site.**
  - `ensure_username_via`: the ledger trigger is why a retired name collides. The loop is unchanged.
  - `principal_refs.rs:86-99`: the user half of the gap is closed by the ledger; the role half stays open.
  - `engine_principals.rs`: the delegator is reserved through ledger seed step 5. Update the test name
    `the_delegator_principal_is_named_here_and_not_yet_reserved` to state that it is not in
    `RESERVED_ENGINE_USERNAMES` but is reserved by the ledger. Its assertions are unchanged.
- [ ] **Step 4: Green and sweep.**
  - Re-run Step 1.
  - Run the suites that write `users.username` directly, each alone: `cargo test -p egret-perm-compile`, `-p
    egret-auth`, `-p egret-bi`, `-p egret-workspace`, `-p egret-automation`, `-p egret-engine-mgmt --test routes`.
  - Fix only the tests that delete a user and re-insert the same username in one binary: give them unique names,
    and say why in the commit.
  - Expected: all green, and the skip grep prints `0`.
- [ ] **Step 5: Commit** `feat(perm): retire usernames forever through an append-only ledger (0208)`.

## Task 2: `hard_delete` recompiles everything that named the user, and leaves a record (spec §4.3, §4.4)

**Files:** the files above for Task 2. Every implementor of `GroupMembershipRecompiler` keeps compiling: the trait
signature does not change.

- [ ] **Step 1: Red tests, as the deleted user's still-compiled name.** In `user_delete_recompile.rs`:
  1. `deleting_a_role_holder_recompiles_the_role_grant`:
     - a grant `include role:analyst_<t>`, and user `eve_<t>` holding `analyst_<t>` (minted);
     - compile → the body names `eve_<t>`;
     - `hard_delete(eve)` → in the same commit, the compiled body no longer names `eve_<t>`. Evaluate with
       `egret-ranger-eval`: user `eve_<t>` is now denied.
  2. The same for a clearance (`classification_policies.required_roles` → `denyExceptions`).
  3. The same for a workspace-members grant (`access_grants` with `scope_type = 'workspace'`).
  4. The same for a `deny` with `exclude role:…` (the fail-open direction).
  5. `a_delete_records_who_was_deleted`: after `DELETE /users/{id}` (through the controlplane router), one
     `audit.audit_log` row with `action = 'user:delete'`, `resource_type = 'user'`, `resource_id = id`, and
     `detail.before.username`, `detail.before.email`. It is written in the delete's own transaction: a forced
     failure after the event (the 23503 path) leaves no event row.
  6. `a_service_account_delete_names_its_username`: the SA delete event carries `username`.

  Expected on the base: 1–4 still name the user; 5 finds no row.
- [ ] **Step 2: Implement.**
  - `lock_affected_by_user_delete_in_tx` reads `SELECT DISTINCT r.name FROM access_grants ag JOIN roles r … WHERE
    principal_type = 'user' AND principal_id = $1` and the workspace ids. It extends `services` with
    `affected_services_for_role_membership` and `affected_services_for_workspace_members`, then sorts and
    deduplicates before the one `lock_services_in_tx`.
  - `hard_delete(target, actor, recompiler)` → `Option<DeletedUser>`, with `RETURNING name, email, username,
    provider` and `record_change` before commit.
  - Add the audit constants, `FAMILIES` (family `Policy`, see spec Q1) and `GOVERNANCE_ACTIONS`.
  - Update both callers. Update `NoProfiles` and the other test call sites.
- [ ] **Step 3: Green.** Run Step 1, plus `cargo test -p egret-audit`, `-p egret-auth`, and `-p egret-perm-compile
  --test group_races --test group_grants --test engine_grants_routes`.
- [ ] **Step 4: Commit** `fix(perm): recompile role, workspace and clearance profiles on user delete; record user:delete`.

## Task 3: One engine-admin set, and an in-transaction `egret-admin` writer (spec §4.2)

- [ ] **Step 1: Red tests** (`admin_policy_sync.rs`, part 1):
  1. `the_engine_admin_set_excludes_disabled_admins`: two direct admins and one group admin; disable one →
     `engine_admin_user_ids_via` returns the other two, ascending.
  2. `an_empty_admin_set_clears_the_body`:
     - seed an `egret-admin` set for `zed_<t>` on a Trino and a StarRocks service;
     - `sync_admin_all_access_in_tx(svc, &[])` → every body has `users: []`, `policy_version` bumped once;
     - `egret-ranger-eval`: `zed_<t>` is denied `select` on `c.s.t`.

     On the base, `seed_admin_all_access(&[])` writes nothing.
  3. `an_unchanged_set_writes_nothing`: a second sync bumps neither the version nor `updated_at`.
  4. `engine_apply_seeds_the_same_set`: `admin_usernames` equals `engine_admin_user_ids_via` mapped to usernames,
     sorted (an `egret-engine-mgmt` test through the existing apply test harness).
- [ ] **Step 2: Implement** `engine_admin_user_ids_via`, `sync_admin_all_access_in_tx` (upsert `WHERE
  policy_jsonb IS DISTINCT FROM EXCLUDED.policy_jsonb`; bump the version only on change; return the removed and
  added names) and `admin_policy_services_via` (`is_seeded_hierarchy_name`). Then:
  - `seed_admin_all_access` delegates to the writer. An empty list rewrites the existing rows and creates none.
  - `admin_usernames` uses the shared set and sorts.
  - Keep `refuse_reserved_name` and `refuse_non_infrastructure_name` on the in-transaction path.
- [ ] **Step 3: Green.** Run Step 1, plus `cargo test -p egret-policy` and `-p egret-engine-mgmt --test routes`
  (admin-seed tests).
- [ ] **Step 4: Commit** `feat(policy): one engine-admin set and an in-transaction egret-admin writer`.

## Task 4: `egret-admin` converges in every recompile, and admin changes lock it (spec §4.2)

- [ ] **Step 1: Red tests** (`admin_policy_sync.rs`, part 2). Each one provisions an `egret-admin` set on a
  service, then performs the change **through the production entry point** and asserts **in the same transaction's
  result** (read after the call returns; no extra recompile). The actor is the demoted principal, evaluated with
  `egret-ranger-eval`:
  1. `unassigning_platform_admin_drops_them_from_egret_admin` (`unassign_platform_role_and_recompile`).
  2. `assigning_platform_admin_adds_them` (minted at the sync if they had no username).
  3. `replace_platform_roles_without_admin_drops_them` (egret-workspace path).
  4. `leaving_a_platform_admin_group_drops_them` (`GroupRepo::remove_member` with `PermGroupRecompiler`).
  5. `deleting_a_platform_admin_group_drops_its_members` (`delete_local`).
  6. `an_oidc_sync_that_removes_the_admin_group_drops_them` (`sync_user_oidc_groups`).
  7. `deleting_an_admin_drops_them` (`hard_delete`; complements the ledger).
  8. `a_recompile_of_an_untargeted_profile_still_syncs` (`compile_desired` → `None` path).

  Expected on the base: the demoted principal is still allowed everywhere.
- [ ] **Step 2: Implement.**
  - The sync at the top of `recompile_locked_service_in_tx` when `service_id ∈ admin_policy_services_via`.
  - The admin-policy arm in `affected_services_for_role_membership` (`role_name == PLATFORM_ADMIN_ROLE`) and in
    `affected_services_for_group_membership` (under `holds_platform_admin`).
  - Active admins in `premint_model_usernames` when any admin-policy service exists.
  - Update the doc of `recompile_after_infra_seed` (`engines.rs:2580`): the sync now runs in every recompile.
- [ ] **Step 3: Green.** Run Step 1, plus `cargo test -p egret-perm-compile` (whole crate, alone) and `-p
  egret-workspace`.
- [ ] **Step 4: Commit** `fix(perm): egret-admin follows platform_admin through the recompile chokepoint`.

## Task 5: Status, group-role routes, the CLI and the boot pass (spec §4.2)

- [ ] **Step 1: Red tests:**
  1. `disabling_an_admin_drops_them_and_enabling_restores` (`PATCH /users/{id}` through the controlplane router;
     assert the body and the evaluator after each call).
  2. `unassigning_platform_admin_from_a_group_drops_its_members` (`DELETE /groups/{id}/roles/{role}`). On the base
     it recompiles nothing (spec E10).
  3. `the_boot_pass_repairs_a_stale_body`: write an `egret-admin` body naming a non-admin directly, run
     `sync_admin_policies(pool)` → repaired; a second run writes nothing.
  4. `create_admin_cli_syncs` (call the CLI's function with an existing admin-policy service).
- [ ] **Step 2: Implement.**
  - `UserRepo::set_status_in_tx`.
  - `set_user_status_and_recompile`: compute the affected set (the admin-policy services when the user is a direct
    or group admin), lock, write, `recompile_services_in_tx`.
  - `update_user` uses it. Keep `user:disable`, `guard_not_last_admin` and session revocation in their order.
  - The group role routes call `assign/unassign_platform_role_and_recompile(pool, "group", …)` and keep their audit
    emits and `guard_last_admin`.
  - `sync_admin_policies` plus its spawn in `main.rs`, next to `recompile_role_referencing_profiles`.
  - The `create-admin` sync.
- [ ] **Step 3: Green.** Run Step 1, plus `cargo test -p egret-controlplane` (alone).
- [ ] **Step 4: Commit** `fix(perm): sync egret-admin on status, group-role and CLI changes; repair at boot`.

## Task 6: The report and the attention signal (spec §4.5)

- [ ] **Step 1: Red tests** (`username_reuse_report.rs`, controlplane `governance_attention.rs`). Arrange
  pre-0208 history with raw SQL on the test DB. Backdate `perm.grants.created_at`, governance events, policy
  `updated_at` and `users.created_at`, and insert `username_reuse_baseline` rows:
  1. `a_ref_older_than_its_holder_is_definite_with_an_event` / `possible_without`.
  2. `an_engine_grant_unchanged_since_before_the_holder_is_definite` (`updated_at`).
  3. `a_compiled_body_older_than_its_holder_is_definite`; an `operator` body is `possible`.
  4. `a_baseline_egret_admin_row_naming_a_non_admin_is_reported`.
  5. `a_holderless_ref_is_dead_and_says_if_it_fails_open`.
  6. `only_platform_admins_may_read_it`: `policy:read` + `user:read` → 403; platform_admin → 200.
  7. Attention: an admin sees `username_reuse` (critical with a definite finding; samples carry no `@`). A
     `policy:read` reader gets `notChecked` `{kind: "username_reuse", requires: ["platform_admin"]}`. With only
     `dead` refs, there is no finding.
- [ ] **Step 2: Implement** `username_reuse.rs` (queries in spec §4.5, `REPEATABLE READ, READ ONLY`), the route
  with utoipa docs, the `platform_admin` pseudo-permission in `attention()`, the source, and `Ref::ReusedNames`.
  Regenerate `openapi.json`.
- [ ] **Step 3: Green.** Run Step 1, plus `cargo test -p egret-server committed_openapi_json_is_current`.
- [ ] **Step 4: Commit** `feat(perm): admin-only username reuse report and Overview signal`.

## Task 7: Delete steers to Disable (spec §4.4 items 1–4)

- [ ] **Step 1: Red tests** (Vitest, `UsersPage.test.tsx` and `UserDetail.test.tsx`; follow FE-TEST rules in
  `docs/en/frontend-rules.md`):
  - the delete dialog text contains "is retired for good" and the username;
  - a user with no username shows "They have no engine username yet.";
  - **Disable instead** opens the disable confirmation and never calls `deleteUser`;
  - it is absent for a disabled user;
  - the disable dialog says "Their engine username stays theirs." and, for an admin, "lose all-access on engines";
  - the row menu offers Disable or Enable, and the item is `aria-disabled` with a reason on yourself.
- [ ] **Step 2: Implement.** Add `username` to `ControlPlaneUserDto` (`access/mod.rs`; read it with the user
  row), regenerate OpenAPI and run `npm run gen:api`. Write the copy exactly as in spec §4.4, with no raw palette
  classes (FE-VISUAL-1). Update the API 204 description.
- [ ] **Step 3: Green.** `npm run lint && npx tsc -b && npm run test -- access`.
- [ ] **Step 4: Commit** `feat(access): user delete says the username is retired and offers Disable instead`.

## Task 8: Reused names tab, Overview link, Recent changes label (spec §4.4 item 5)

- [ ] **Step 1: Red tests:**
  - `refUrl({type: "reused_names"})` → `/governance/people?kind=reused`;
  - `ReusedNames` renders verdict, username, place link (grant, rule or engine grant → existing pages; policy →
    profile page), holder and evidence time;
  - it renders a 403 with the existing `TabError`;
  - the tab is hidden for non-admins;
  - Recent changes renders `user:delete` as "Deleted user bob".
- [ ] **Step 2: Implement.** Use the existing `UnmatchedNames` table pattern and `STACKED_TABLE` at 390 px.
- [ ] **Step 3: Green.** Run lint, tsc, vitest and `npm run build`, plus `npm run test:bundle:governance-people`
  and `test:bundle:governance-overview`.
- [ ] **Step 4: Commit** `feat(governance): Reused names tab and username_reuse finding`.

## Task 9: Platform docs (`CLAUDE.md` docs rule)

- [ ] Update the `egret-data/docs` Platform pages for users and admins: Delete vs Disable, retired usernames,
  `egret-admin` following admins (disabled admins excluded), and the Reused names report. Refresh the screenshots of
  the delete dialog and the Reused names tab. **Confirm the repo path with the user first.** If it is not
  available, record it as a PR checklist item and do not tick the Docs box.

## Task 10: Final gate

- [ ] **Backend** (one command at a time; `pgrep -x cargo` empty first):

```bash
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test --workspace 2>&1 | tee "$SCRATCH"/takeover-ws.log | tail -40
grep -c "skip: DATABASE_URL" "$SCRATCH"/takeover-ws.log
grep -E "^test result: FAILED|panicked" "$SCRATCH"/takeover-ws.log | head
just test-ranger-gates 2>&1 | tee "$SCRATCH"/takeover-ranger.log | tail -20
cargo run -q -p egret-bin --bin egret -- openapi > openapi.json && git diff --exit-code openapi.json
cargo test -p egret-server committed_openapi_json_is_current
```

Expected:

- fmt and clippy are clean;
- every `test result: ok`;
- the skip grep prints `0`;
- the FAILED grep prints nothing;
- the Ranger gates pass on both jar versions;
- `openapi.json` has no diff.

- [ ] **Frontend** (from `frontend/`):

```bash
npm run gen:api && git status --short src/api/generated   # expect: no change
npm run lint
npx tsc -b
npm run test 2>&1 | tail -20
npm run build
npm run test:bundle && npm run test:bundle:governance-people && npm run test:bundle:governance-overview
```

Expected: every command exits 0, and `gen:api` leaves no diff.

## Task 11: Live check (own stack `egret-takeover`, port 18380; never the stack on 18080)

Bring the stack up with a compose override in the scratchpad, as in the classification plan's Task 12
(`-p egret-takeover`, `DEMO_PORT=18380`). Run the migration on an install that already has data (seed first,
then upgrade the image), so that 0208's backfill and baseline see real history. Record the results in the
scratchpad, not git.

1. Before the upgrade:
   - create `bob@a.test`, run a query as bob (mints `bob`), add a direct grant `user:bob`, and delete bob;
   - create local admin `carol@a.test`, provision a Ranger-bound Trino engine, and confirm `egret-admin - …` names
     `carol`;
   - remove carol's platform_admin **by SQL on the old image**, to simulate today's stale body.
2. Upgrade to this branch. `username_ledger` holds `bob` (`backfill_ref`) and `carol` (`backfill_user`).
   `username_reuse_baseline` holds carol's `egret-admin` row. After boot, the `egret-admin` bodies no longer name
   carol (boot pass).
3. Create `bob@b.test` and run a query → engine username `bob-2`. The old `user:bob` grant does not apply: the
   query on the granted table is denied.
4. As an admin, assign platform_admin to `dave@a.test`:
   - `egret-admin` names `dave` when the request returns;
   - after the plugin poll, dave can `SELECT` any table;
   - unassign → the body drops dave at once; after the poll, dave is denied.
5. Disable dave (re-assign first) → the body drops him. Enable → back. Repeat with a group-held platform_admin:
   remove dave from the group → dropped.
6. Delete an admin → dropped. Governance > Recent changes shows "Deleted user …". Confirm the audit row has the
   username and email.
7. Governance > People > Reused names lists carol's baseline row (`admin_policy_named_non_admin`, definite) and
   `bob`'s grant as `holderless_ref` (dead, plus its fail-open flag). The Overview shows `username_reuse`. A
   `policy:read`-only user sees it under "not checked".
8. Delete dialog: check the copy at 1440 px and 390 px; Disable instead works; the row menu Disable/Enable works.
   Screenshots go to the scratchpad.
9. JWT window, observation only: mint a query token for a user, delete them, and confirm egret mints no new token
   (`NotFound`). The old token is not reused by egret.

## Self-review

- **Spec coverage:** §4.1 → T1; §4.3 + event → T2; §4.2 → T3–T5; §4.5 → T6; §4.4 UI → T7–T8; docs → T9; §4.6
  needs no code (observed in T11.9).
- **Ordering and green main:** every task ends green. T2 depends on T1 only for the ledger-backed claim in its
  docs. T4 depends on T3. T6 depends on T1 (baseline) and T2 (event). T7 and T8 depend on T6's OpenAPI.
- **Open question:** spec §12 Q1 (family of `user:delete`). Only T2's `FAMILIES` line and T8's feed filter change
  if it is answered `User`.
