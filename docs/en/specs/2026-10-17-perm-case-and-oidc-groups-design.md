# OIDC sync and local groups; egret-perm case folding: design and plan

Date: 2026-10-17. Base: main `0741a48f`. Branch `fix/perm-case-oidc-groups`. Follow-ups from #685 (J-F3) and
#687 (DG redesign #1).

## Questions for the user

**Answered by the user 2026-10-17:** Q1 → R1 (the sync manages OIDC-sourced groups only; R1-alt is dropped, no migration). Q2 → report only, no existing row is deleted (R5).

1. **Should an IdP group claim ever grant membership in a LOCAL group?** Today it does: a claim naming a local
   group inserts a membership, and no later login removes it (Part A). R1 recommends **no**: the sync touches only
   OIDC-sourced groups. That changes behaviour an admin may rely on: naming a local group in the IdP on purpose so
   that IdP members join it. After R1 such a mapping stops adding members. The supported way to give an IdP group
   roles already exists (assign the role to the OIDC group itself, `GroupsPage.tsx:195-200`). The alternative is
   R1-alt: record who added each row (migration `0195`) so the sync adds AND removes its own rows in local groups.
   That keeps the mapping working and makes removal correct. It also keeps the escalation path open: anyone who
   controls an IdP group name can join any local group, including one that holds `platform_admin`. **Which one?**
   The plan below is written for R1 and lists what R1-alt changes.
2. **Existing rows: delete, or only report?** Nothing records whether a row in `user_groups` was added by the
   sync or by an admin (evidence A4). So the stale memberships an earlier sync put into local groups cannot be told
   apart from legitimate admin adds. R5 recommends **no data change**: a startup `warn!` per local group that has
   OIDC-provider members, plus a per-login `warn!` when a claim names a local group the user already belongs to (the
   strongest signal there is). An automatic delete would remove legitimate admin-added memberships. **Confirm that
   no existing row is deleted, or name a deletion rule you accept.**

## Problem

### A. OIDC login sync and local groups (security)

`GroupRepo::sync_user_oidc_groups` (`crates/egret-auth/src/groups.rs:319`) joins `groups` by name with no source
filter, both when it computes the claimed set and when it inserts. Its DELETE removes only memberships in
OIDC-sourced groups. So:

- an IdP claim that names a LOCAL group makes the user a member of it, and
- no later login removes that membership, even after the IdP stops sending the claim.

Since J-F3 (R8) a group membership compiles into engine access, because groups expand to their members' usernames.
Group memberships also carry platform roles, workspace roles and app access (A3). So a stale membership is a live
grant on every one of those surfaces. Whoever controls group names in the IdP can join any local group by naming it,
including a local group that has been assigned `platform_admin`. This is older than J-F3, and J-F3 widened it to
the engines.

The mirror case: the API lets an admin add a member to an OIDC group. The UI forbids it (A5). For an OIDC user, the
next login's DELETE silently undoes the add. For a local-provider user no sync ever runs, so the membership is
permanent.

### B. egret-perm compares resource names case-sensitively

`egret_perm::eval` matches the schema, table and column levels with `glob_match`, case-sensitively. The engines
match them case-insensitively, and since #687 so does the Data UI. A deny written on `sales.Orders` therefore does
not cover the table `orders` in egret's oracle, while the engine enforces it there. Tag keys are case-sensitive on
the engines and stay so.

## Evidence (verified at `0741a48f`)

### A

| id | fact | where |
|---|---|---|
| A1 | The claimed set is `SELECT name FROM groups WHERE name = ANY($1)`, with no `source` filter. It feeds `after_set`, which keeps the non-OIDC rows the user holds and adds every claimed name | `groups.rs:344`, `groups.rs:350-355` |
| A2 | The DELETE removes only memberships in OIDC-sourced groups (`... group_id IN (SELECT id FROM groups WHERE source = 'oidc')`). The INSERT joins by name alone (`SELECT $1, id FROM groups WHERE name = $2`) | `groups.rs:376-382`, `groups.rs:392-401` |
| A2b | `provision_and_sync` first calls `upsert_oidc_group` for each claim. On a name collision that is `ON CONFLICT (name) DO UPDATE SET name = groups.name`, so it returns the LOCAL group's id and leaves its source as `local`. Then it syncs | `oidc.rs:49-54`, `groups.rs:286-297` |
| A3 | Membership is read for platform roles (`roles.rs:183-196`, `effective.rs:140-150`), workspace roles (`session.rs:240-249` and `:283`, `:302`, `:324`, `:346`), app access (`egret-apps/src/gateway.rs:270`) and, since J-F3, compiled engine policies (`egret-perm-compile/src/members.rs:375-385`). `platform_role_assignments.principal_type` admits `'group'` | `migrations/0003_workspace_rbac.sql:35-40` |
| A4 | `user_groups` is `(user_id, group_id)` and nothing else. No migration after `0002` alters it. `add_group_member` / `remove_group_member` emit no audit event, unlike `assign_group_role` (`access/mod.rs:820`). So no record exists of who added a row | `migrations/0002_identity.sql:21-25`, `egret-controlplane/src/access/mod.rs:744-810` |
| A5 | Ownership model as built: rename and delete refuse an OIDC group (409, `access/mod.rs:653-656`, `:685-688`, repo `groups.rs:448`, `:500`, `:519`). The UI disables "Manage members" for OIDC groups and states "their membership is owned by the identity provider" (`GroupsPage.tsx:61-68`, `:201-210`). The admin-RBAC spec says the same: "OIDC groups read-only but role-assignable" (`docs/en/specs/2026-07-07-platform-rbac-admin-design.md:47`). The API, however, does not refuse an add to an OIDC group: `change_member` checks only that the group exists | `groups.rs:564-620` |
| A6 | Nothing documents mapping a claim onto a local group on purpose. "syncs OIDC groups into local groups" (`docs/en/specs/2026-07-01-oidc-keycloak-default-dev-e2e-design.md:32`) uses "local" to mean egret-side rows. No migration or seed inserts a group (`INSERT INTO groups` appears only at `groups.rs:289`, `:415`) | — |
| A7 | Every `user_groups` writer: the sync (`groups.rs:376`, `:392`), `change_member` (`groups.rs:602-605`), `delete_local` (cascade, `groups.rs:519`), `UserRepo::hard_delete` (`users.rs:329`). No SCIM | — |
| A8 | The existing test `group_sync_replaces_oidc_keeps_local` pins "a local membership survives sync". It does not test a claim that names a local group | `egret-auth/tests/oidc_db.rs:52-108` |

### B

| id | fact | where |
|---|---|---|
| B1 | `covers` compares `catalog_id` (a UUID) for equality, and the three name levels through `level_covers` → `glob_match`, which compares `char`s exactly | `crates/egret-perm/src/eval.rs:285-292`, `:303-309`, `:323-354` |
| B2 | Trino `catalog`/`schema`/`table`/`column` and StarRocks `catalog`/`database`/`table`/`column` set `"ignoreCase": true` | `egret-policy/src/service-defs/trino.json:12-76`, `starrocks.json:11-112` |
| B3 | The tag def sets `ignoreCase: false`. `selector_hits` (tag arm) and the clearance check compare tag keys with `==` | `service-defs/tag.json:21`, `eval.rs:171`, `:370` |
| B4 | The server's own resource fold is `fold(s) = s.to_lowercase()`: whole-string Unicode lowercase on both sides for `Equals`/`StartsWith`/`EndsWith`. For `Wildcard` it compares per character (`a.to_lowercase().eq(b.to_lowercase())`), so that a length-changing fold cannot shift indices | `egret-ranger-eval/src/resource.rs:901-944`, `:1099-1107` |
| B5 | The Data UI lower-cases both sides with JS `toLowerCase()` and then runs a port of `glob_match`. Its comment names egret-perm's case-sensitivity as "a known divergence left as a follow-up" | `frontend/src/routes/governance/data/coverage.ts:22-34`, `:56`, `:192-218` |
| B6 | `FOLD_DIVERGENT`: 25 code points on which Java's per-character `equalsIgnoreCase` (the engine) and a full-string lowercase (Postgres `lower()`, and equally Rust `to_lowercase` / JS `toLowerCase`, which implement the same full mapping) can disagree. `fold_divergent` gates tag resolution on them | `egret-perm-compile/src/routes.rs:3175-3213`, `:3312-3314` |
| B7 | Consumers of `egret_perm::eval`. **`evaluate` has no production caller**: `explain` moved to the artifact evaluator in SP4d-2a (`egret-perm-compile/src/routes.rs:1877-1890`, `egret-perm/src/lib.rs:38-46`). It is the test oracle in `egret-perm/tests/eval.rs`, `egret-perm-compile/tests/{workspace_members,role_grants,clearance}.rs` and the drift alarm `the_artifact_answer_and_the_model_oracle_agree_on_the_happy_path` (`tests/routes.rs:6561`). **`glob_match` has one production caller**: `view_audit::tables_have_rule`, which folds with `to_ascii_lowercase` (`egret-perm-compile/src/view_audit.rs:141-146`), so a non-ASCII mixed-case name (`NGƯỜI` / `người`) is missed. `tables_have_rule` feeds the view-exposure audit (`egret-controlplane/src/view_exposure.rs:551`). A miss there hides a view from the audit, which fails open in reporting |
| B8 | Not consumers: attribution maps policy names only (`attribution.rs:1-40`). Overlap detection is `egret_policy::unit_overlaps` with its own Java-style fold (`egret-policy/src/ambiguous_arbitration.rs:576-614`). Tag resolution in compile/equivalence already folds (`equivalence.rs:822`) | — |

## Decisions

| id | decision | by |
|---|---|---|
| R1 | The OIDC sync manages memberships in **OIDC-sourced groups only**. The claimed-set query and the INSERT both add `AND source = 'oidc'`. A claim naming a local group is ignored: no membership, nothing moved, nothing recompiled. A local membership the user already holds stays untouched, as today (A8). No migration. Reason: it matches the ownership model the API, UI and RBAC spec already state (A5), and it closes the path from IdP group names to local groups' roles (A3) | **user, 2026-10-17** |
| R1-alt | (Only if Q1 says keep the mapping.) Migration `0195_user_groups_added_by.sql`: `ALTER TABLE user_groups ADD COLUMN added_by text NOT NULL DEFAULT 'admin' CHECK (added_by IN ('admin','oidc_sync','unknown'))`. Backfill: `'oidc_sync'` for rows in OIDC groups, `'unknown'` for OIDC-provider users' rows in local groups. The sync deletes and inserts only `added_by = 'oidc_sync'` rows; an admin add of an existing sync row sets `'admin'`. `0195` is the next free number on `origin/main` (latest `0194_workflow_task_run_working_dir_snapshot.sql`; no remote branch carries `0195`+, checked 2026-10-17). Re-check before merging | dropped (user chose R1, 2026-10-17) |
| R2 | `upsert_oidc_group` is unchanged: on a collision it still returns the local id and never flips `source`. `provision_and_sync` logs one `warn!` per claimed name that is a LOCAL group, with the user id, the group name, and whether the user is already a member ("membership not managed by the IdP; if it was added by a login before this fix, review it") | default decision (2026-10-17) |
| R3 | The recompile contract is unchanged: R1 only narrows `claimed`, so `moved` stays the symmetric difference and every moved group is still locked and recompiled in the same transaction (`groups.rs:357-374`, `:404-406`) | default decision (2026-10-17) |
| R4 | `GroupRepo::add_member` refuses an OIDC-sourced group with `Conflict` (409 at `POST /groups/{id}/members`, mirroring rename/delete). `remove_member` stays allowed for OIDC groups: it fails closed and serves as an emergency revoke until the next login. Reason: an add to an OIDC group is undone at the next login for OIDC users and never undone for local users. The UI already forbids it (A5). With R1 and R4, a row's provenance follows from its group's source | default decision (2026-10-17) |
| R5 | Existing rows are not changed. One startup pass logs: (a) one `warn!` per local group with ≥1 member whose `users.provider = 'oidc'`, with the count, saying provenance is unknown and an earlier login may have added them; (b) one `warn!` per membership of a `provider = 'local'` user in an OIDC group (the R4 class, which no login ever removes). The pass runs off the boot path, like `warn_fail_open_principals` (`egret-bin/src/main.rs:2179-2192`). No endpoint: the rows are mostly legitimate, and members are already listed per group in the console | default decision (2026-10-17); deletion is Q2 |
| R6 | egret-perm folds the resource levels (schema, table, column) the way the server does: whole-string `str::to_lowercase` on both sides, then `glob_match`. This is the same function as `egret_ranger_eval::resource::fold` (B4) and the Data UI's `toLowerCase` (B5). It lives in one `pub fn resource_level_matches(pattern, value)` in `eval.rs`; `level_covers` calls it. `glob_match` stays public and case-sensitive (the tag semantics, and the UI's port). The catalog compares by UUID and needs no fold | default decision (2026-10-17) |
| R7 | Tag keys (`Selector::Tag`, `ClearanceGate::tag`) and principal refs stay case-sensitive (B3). This package changes resource levels only | default decision (2026-10-17) |
| R8 | `FOLD_DIVERGENT` is a documented bound, not a gate, in egret-perm. `evaluate` has no decline channel and no production caller (B7). Its module doc gets a fifth bound naming the 25 code points and pointing at `egret_perm_compile::routes::FOLD_DIVERGENT`. The constant is not moved: egret-perm must not depend on egret-perm-compile, and one copy must not become two | default decision (2026-10-17) |
| R9 | `view_audit::tables_have_rule` calls `egret_perm::eval::resource_level_matches` instead of its ASCII-only fold, so the audit folds like the engine | default decision (2026-10-17) |
| R10 | `coverage.ts`'s comment stops calling egret-perm a divergence. No frontend behaviour change | default decision (2026-10-17) |

### Not in scope (recorded, verified)

- **The glob algorithm itself.** `glob_match` (and the UI's port) is a classic greedy matcher. The engine uses
  `FilenameUtils.wildcardMatch`, which differs for some patterns, e.g. `?*?` against `HIVE`
  (`egret-ranger-eval/src/resource.rs`, the `wildcard_match` doc, finding F1). `perm.grants` can store such a value,
  because `0154` puts no vocabulary CHECK on the levels. The oracle over-matches there. That is a separate follow-up.
- **`egret-controlplane/src/view_exposure.rs:305-377`** matches StarRocks MV text with an ASCII-only fold. It is
  not an egret-perm consumer; separate follow-up.

## Global Constraints

- **Build isolation is mandatory.** `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-fu/target`.
  Never another checkout's `target/`: a shared target produces `E0432` errors that look like a broken branch.
- sqlx 0.9: `query()` takes `&'static str`. No `format!`-built SQL.
- OpenAPI must satisfy `committed_openapi_json_is_current`. R4 adds a 409 to `POST /groups/{id}/members`, so
  regenerate with `cargo build -p egret-bin && target/debug/egret openapi > openapi.json` and `cd frontend && npm
  run gen:api` (never bare `orval`), and commit the output exactly as produced.
- `cargo build --workspace`, workspace clippy and `cargo fmt --all -- --check` clean.
- Tests: `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` only, **never** the dev database `egret`.
  Always run with `env -u EGRET_ALLOW_DB_SKIP`; never set `EGRET_ALLOW_DB_SKIP=1`. DB-gated tests skip silently
  without a database, and a skip reports `ok`. Scope assertions to ids you create; never weaken an assertion to do
  it. Run suites in the FOREGROUND, one at a time.
- Never `git stash`. Never commit `.superpowers/`.
- Every new test is verified by mutation: break the fix, watch that specific test fail **for the right reason**,
  restore with `cat backup > file`, and confirm `git status --porcelain` is clean.

## Plan

### Task 1: the sync touches OIDC groups only (R1, R2, R3)

**Files:** `crates/egret-auth/src/groups.rs`, `crates/egret-auth/src/oidc.rs`, `crates/egret-auth/tests/oidc_db.rs`.

- `groups.rs:344`: `SELECT name FROM groups WHERE name = ANY($1) AND source = 'oidc'`. `groups.rs:394`:
  `... WHERE name = $2 AND source = 'oidc'`. Update the doc at `groups.rs:299-318` ("every claimed name that names
  an OIDC group") and the module doc's writer list.
- `oidc.rs:49-54`: before the sync, read which claimed names are local groups, and whether the user is in each
  (one query). Log R2's `warn!` for each. Do not refuse the login.
- DB tests:
  - A claim naming a local group the user is not in → no membership, and the recorder sees no lock and no
    recompile.
  - A claim naming a local group the user IS in → the membership is kept and nothing moves; a later login without
    the claim keeps it too (pins R1: the sync never touches local rows).
  - A mixed claim `[oidc_a, local_b]` → joins `oidc_a` only, and `moved == [oidc_a]`.
  - `provision_and_sync` with a local name → the group's `source` is still `local` and no membership exists.
  - `group_sync_replaces_oidc_keeps_local` and `a_login_recompiles_only_the_groups_it_moved` stay green.
- Mutation: drop the `source` filter from either query, and the first and third tests must fail on the membership
  assertion.

### Task 2: no admin add into an OIDC group (R4)

**Files:** `crates/egret-auth/src/groups.rs` (`change_member`), `crates/egret-controlplane/src/access/mod.rs`
(utoipa 409 on `add_group_member`), `crates/egret-auth/tests/groups_db.rs`, the route test next to
`egret-policy/tests/admin_routes.rs:4086`, `openapi.json`, the generated client.

- In `change_member`, read `source` with the name. On `add && source == "oidc"`, return
  `Error::Conflict("Cannot add members to an OIDC-synced group; its membership is owned by the identity provider")`
  before any lock or recompile. Remove is unchanged.
- Tests: add to an OIDC group → Conflict, no row, no recompile; remove from an OIDC group → still works and
  recompiles; add to a local group unchanged; route answers 409.

### Task 3: report rows of unknown provenance (R5)

**Files:** `crates/egret-auth/src/groups.rs` (new `pub async fn membership_provenance_report(pool)` returning
both lists), `crates/egret-bin/src/main.rs` (spawned beside `warn_fail_open_principals`),
`crates/egret-auth/tests/groups_db.rs`.

- (a) local groups with OIDC-provider members (name, count); (b) `(local-provider user, OIDC group)` memberships.
  The function returns rows, and a thin wrapper logs one `warn!` per row, so the rows are testable.
- DB tests: a seeded OIDC user in a local group appears in (a) with the right count; a local user in an OIDC group
  appears in (b); a local user in a local group and an OIDC user in an OIDC group appear in neither. Filter
  assertions to the seeded ids.

### Task 4: egret-perm folds resource levels (R6, R7, R8)

**Files:** `crates/egret-perm/src/eval.rs`, `crates/egret-perm/tests/eval.rs`.

- Add `pub fn resource_level_matches(pattern: &str, value: &str) -> bool { glob_match(&pattern.to_lowercase(),
  &value.to_lowercase()) }`, with a doc that cites `trino.json`/`starrocks.json` `ignoreCase: true`,
  `egret_ranger_eval::resource::fold`, and `coverage.ts`. `level_covers` calls it.
- Module doc: a bound for `FOLD_DIVERGENT` (R8), and the note that tag keys stay exact (R7).
- Unit tests (`eval.rs`): `Orders`/`orders`; `ORD*`/`orders`; `NGƯỜI`/`người`; `s?LES`/`Sales`; a literal still
  needs the same letters (`orders`/`order` false).
- Oracle tests (`tests/eval.rs`): a deny on `sales.Orders` denies target `sales.orders` (the fail-open direction);
  an allow on `SALES.*` allows `sales.orders`; a path-selector mask on `Sales.Orders.Email` applies to
  `sales.orders.email`; a tag rule keyed `PII` does NOT hit `tags_on = ["pii"]`; a clearance gate on `PII` does not
  gate `pii`.
- Mutation: make `resource_level_matches` call `glob_match` without folding, and the deny and allow oracle tests
  must fail; change the tag arm to fold, and the tag test must fail.

### Task 5: consumers (R9, R10) and the drift alarm

**Files:** `crates/egret-perm-compile/src/view_audit.rs`, `crates/egret-perm-compile/tests/routes.rs`,
`frontend/src/routes/governance/data/coverage.ts`.

- `view_audit.rs:141-146`: `level` calls `egret_perm::eval::resource_level_matches`. Extract the path arm into a
  pure `fn path_rule_selects(index, tables) -> bool`, so a unit test can reach it without a database. Test: a rule
  on `NGƯỜI.Bảng` selects lineage table `người.bảng` (fails under `to_ascii_lowercase`).
- Extend `the_artifact_answer_and_the_model_oracle_agree_on_the_happy_path` (`tests/routes.rs:6561`) with a target
  whose table differs from the grant's only in case. The artifact evaluator folds (B4), so before Task 4 the two
  answers disagree and this test must fail; after it they agree. Run it once on the base to confirm it fails there.
- `coverage.ts:30-32`: replace "still compares case-sensitively, a known divergence left as a follow-up" with a
  pointer to `resource_level_matches`. Comment only; vitest unchanged.

### Gate

Whole-branch review. Then full local CI: `cargo fmt --all -- --check`, workspace clippy, shipping-resolution,
`cargo test --workspace` (DB per Global Constraints), `just test-ranger-gates`, and frontend
lint/tsc/vitest/build/bundle. Check the `committed_openapi_json_is_current` test. No live engine run is needed:
Part A changes only which rows the sync writes, and Part B makes the oracle and one audit match a fold the engine
already applies. The drift alarm in Task 5 is the differential check against the artifact evaluator.

PR notes: (1) an IdP claim naming a local group no longer adds a member (or, under R1-alt, its rows are now removed
when the claim goes); (2) `POST /groups/{id}/members` on an OIDC group answers 409; (3) startup warns about
memberships of unknown provenance, and no rows are changed.
