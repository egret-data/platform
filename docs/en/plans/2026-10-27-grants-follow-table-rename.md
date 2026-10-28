# Grants, rules and catalog grants follow a table rename: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** a table or column rename copies the deny grants, path masks and path row filters of the source to the new
name before the Iceberg commit and keeps the old-name rows; holds the commit until Trino's and StarRocks' plugins
serve the copies; moves allow grants and catalog grants to the new name after the commit; refuses (409) a rename that
a restrictive pattern or a hand-written restrictive policy would let escape; purges allows and catalog grants with a
recursively dropped namespace; reports stale references.

**Architecture:**

- A new public module `egret_perm_compile::follow` selects, for one coordinate move, the perm rows that follow
  (literal), the restrictive pattern rows that refuse, the allow patterns that are left, and the hand-written
  policies that refuse or are reported; it copies, moves and deletes rows on a caller's transaction.
- #757's table hook (`table_hook.rs`) widens its move plan with a perm part and a catalog-grant part: copies in the
  copy transaction, a Trino policy-version wait, allows and events in `finish`, catalog grants after it.
- `wait_for_versions` latches confirmed pairs. Grant and rule writes take the hold's name lock.
- Five Overview findings; feed sentences; a new `Catalog` event family.

**Tech stack:** Rust (axum 0.8, sqlx 0.9), embedded Lakekeeper, Ranger plugin ledger, React 18, Vitest.

**Spec:** `docs/en/specs/2026-10-27-grants-follow-table-rename-design.md` (G1-G6 user decisions, D1-D21 defaults).
Read it and #757's spec §3-§5 first. Every `file:line` there was checked on `4807c3e3`; re-check with
`/usr/bin/grep -n` before editing (Step 0).

## Global Constraints

- **Worktree:** `egret-grmove`, branch `fix/grants-follow-table-rename`. Work only there.
- **Prerequisite:** the F-A PR (`fix/catalog-rename-authorizes-source`) is merged into `main` and this branch is
  rebased onto it before Task 1. If it is not merged, stop and report.
- **Build isolation:** `export CARGO_TARGET_DIR=$PWD/target` from the worktree root;
  `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`. Keep 30 GiB free;
  `./deploy/prune-target.sh` before the gate.
- **Machine:** zsh; no `rg` (use `/usr/bin/grep`); no `timeout`; quote globs; use `/bin/rm -f`, `/bin/cp -f`; never
  `git stash`, `git checkout --` or `git clean`; `pgrep -x`, never `pgrep -f`.
- **SQL:** sqlx 0.9 `query()` takes `&'static str`; no `format!` SQL. `lakekeeper.` is written as a literal (the
  drift test asserts it equals `egret_catalog_engine::LAKEKEEPER_SCHEMA`).
- **DB-gated tests must run, not skip:** `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`, under
  `env -u EGRET_ALLOW_DB_SKIP`; afterwards `grep -c "skip: DATABASE_URL"` prints `0`. Never the database `egret`. One
  suite at a time.
- **Red first, through the real handler.** Every behaviour gets a failing test, seen failing on the old code for the
  stated reason, before the fix:
  - hook behaviour through the real Iceberg REST router, as `crates/egret-catalog-engine/tests/table_hook_db.rs`
    does (`mount` with `table_hook::layer`, `observe` in front of Lakekeeper, `oneshot`, the fake plugins of
    `tests/common/plugins.rs`);
  - perm-write behaviour through the real perm router (`crates/egret-perm-compile/tests/`, as the grant route tests
    do);
  - compiler behaviour through `recompile_service_in_tx`;
  - control-plane behaviour through its real router, as `governance_attention.rs` does.
  Engine-level assertions read the served download (`/plugins/policies/download/…`, `/tags/download/…`) or evaluate
  it with `egret-ranger-eval`, never a JSON substring of a stored row. A compile-error red is acceptable only for a
  new API, and then the behaviour test must also fail on a stub that returns the old behaviour. Mutation-check each
  new assertion once (break the fix, watch it fail, restore from a backup with `cat backup > file`;
  `git status --porcelain` lists only intended files).
- **Per-task checks** (each task names its own): `cargo fmt --all -- --check`; `cargo clippy -p <touched crates>
  --all-targets -- -D warnings`; the touched test binaries.
- **Copy:** `docs/en/frontend-rules.md` (FE-COPY-1..4, FE-A11Y-4, FE-TEST-*); feed sentences and sample details are
  plain text (#757 §14 Task 7).
- **Commits:** one per task, Conventional Commits (`feat(policy)`, `feat(perm)`, `fix(catalog)`,
  `feat(governance)`, `test(catalog)`), trailer exactly `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
  No `Closes #N`, no scratch files, no `/Users/...` paths. Do not push until the user says so.
- **Migrations:** none (D18). If one becomes necessary, take the next number free above main AND every open PR at
  merge time, and record it in spec §14.
- **Stacks:** the demo stack on `127.0.0.1:18080` belongs to another session; never stop, reset, seed or read it.
- **Deviations** go to spec §14 (append-only), per task.

## File structure

| file | change | task |
|---|---|---|
| `crates/egret-policy/src/plugin_downloads.rs` | latch in `wait_for_versions` | 1 |
| `crates/egret-perm-compile/src/follow.rs` (new), `src/lib.rs` | selection, hand-written check, copy/move/delete helpers, cover | 2, 3, 9 |
| `crates/egret-perm-compile/tests/follow_db.rs` (new) | follow selection and cover tests | 2, 3, 9 |
| `crates/egret-perm-compile/src/trigger.rs`, `src/table_columns.rs` | D12; `table_states_ns_via` | 4, 15 |
| `crates/egret-perm-compile/src/routes.rs` | D10 name lock | 5 |
| `crates/egret-perm-compile/src/governance_events.rs` | summaries reachable from `follow` | 7 |
| `crates/egret-catalog-engine/src/table_hook.rs` | plan parts, copy, wait, finish, cover, catalog follow, purge | 6-14 |
| `crates/egret-catalog-engine/src/store/embedded.rs` | catalog-grant follow and purge queries | 13, 14 |
| `crates/egret-catalog-engine/tests/table_hook_db.rs`, `tests/common/plugins.rs` | hook tests | 6-14 |
| `crates/egret-audit/src/governance.rs` | D9 actions, `CATALOG_GRANT`, `Family::Catalog` | 7, 8, 10, 13 |
| `crates/egret-controlplane/src/governance_changes.rs` | sentences, `Catalog` family | 7, 8, 10, 13 |
| `crates/egret-controlplane/src/governance_overview.rs`, `tests/governance_attention.rs` | five sources | 15, 16 |
| `frontend/src/routes/governance/overview/AttentionSection.tsx`, its `__tests__` | `CHECKS` | 17 |

## Step 0: Re-verify the evidence on the then-current main

- [ ] `git fetch origin && git log --oneline -1 origin/main`; confirm the F-A PR is in it; `git rebase origin/main`;
  record the new base in spec §14. Re-read the F-A diff: if it changed the guard's rename branch in a way this
  design depends on (it should not), record it.
- [ ] Re-check each row; record drift in spec §14; stop and report if a row is false (not merely moved).

| spec row | check |
|---|---|
| E1, E2 | `CREATE TABLE perm.grants`, `perm.grant_principals`, `perm.rules` (`name text NOT NULL UNIQUE`), `perm.rule_exemptions` in `migrations/0154_perm_model.sql`; `/usr/bin/grep -l "ALTER TABLE perm" migrations/*` lists only 0178, 0179 (and nothing newer) |
| E3 | `pub fn resource_level_matches` in `crates/egret-perm/src/eval.rs` |
| E5 | `affected_services_for_grant` and `affected_services_for_rule` (path arm) call `services_for_catalog_via` in `trigger.rs` |
| E7 | `update_grant_in_tx`'s SET list names no `origin`; `create_rule_in_tx` exists and is exported from `egret_perm` |
| E9 | `pub(crate) fn grant_hash`, `rule_hash` in `migrate.rs`; undo compares them |
| E10, E12 | no unique index on `catalog.catalog_grants` (`/usr/bin/grep -n "catalog_grants" migrations/*`); `fn catalog_grant_covers` compares with `==` |
| E15 | `if rows.is_empty() { return Ok(None); }` in `move_plan_for_rename`; the `classified` filter in `move_plan_for_columns` |
| E17 | `starrocks_policy_versions` is the only policy-version read in `copy` |
| E18 | `wait_for_versions` returns only when `behind_tags` and `behind_policies` are both empty in one pass |
| E20 | `pub async fn try_name_lock_in_tx` in `view_tags.rs`; `table_change_held` in `crates/egret-bi` or `egret-perm-compile` |
| E21 | `leaving.counts_body(&dp.body, &levels)` in `trigger.rs` filters every desired body regardless of source; find where `perm.compiled.source_kind` (or the `perm-grant-`/`perm-rule-` name prefix) is available at that point |
| E22 | `cardinality(t.tabular_namespace_name) = 1` in `table_states_via` |
| E23 | `pub type SourceFn` takes `&mut PgConnection` only |
| E26, E27 | the two live-result passages exist at the cited lines |
| D5 | the compiler emits the registry catalog name at the catalog level (`grants.rs`); list the hand-written policies of a fresh stack's seed (`managed_by = 'operator'`) and confirm their catalog values use the registry name or `*` |
| D12 | whether `rules.rs`' per-rule ceilings (mask columns, row-filter tables) see path rules or only tag expansions; record which ceiling sites D12 touches |
| migrations | `ls migrations \| tail -1` is `0209` (or the rebase's newest) |

## Task 1: A confirmed pair stays confirmed (D7)

**Files:** `crates/egret-policy/src/plugin_downloads.rs`; tests `crates/egret-policy/tests/plugin_downloads_db.rs`.

- [ ] **Red (DB, through the real download routes):**
  - `a_tag_pair_confirmed_once_stays_confirmed_while_the_policy_pair_waits` — a Trino profile; the fake plugin
    reports the tag version V on a tag poll, then (as Trino does after a policy download, E27) reports
    `lastKnownVersion=-1` on its next tag poll, and only later reports the policy version P on a policy poll. Spawn
    `wait_for_versions(&[(svc, V)], &[(svc, P)], 3 s)` before the first poll → `Ok` right after the policy report.
    Today: it waits for a tag poll reporting V again and times out (red).
  - guard `a_pair_never_confirmed_still_times_out` — a policy pair never reported → `Timeout` naming the plugin.
  - guard (view hold) `wait_for_tag_version(&[])` still returns `Timeout` at once.
- [ ] **Fix:** keep a `confirmed` set per pair inside `wait_for_versions`; a pair once confirmed is not re-queried.
- [ ] **Checks:** fmt; clippy `-p egret-policy`; `cargo test -p egret-policy --test plugin_downloads_db --test
  plugin_downloads_failure_db --lib`; `cargo test -p egret-catalog-engine --test view_hold_db --test table_hook_db`.
- [ ] Commit `feat(policy): a confirmed version stays confirmed for the rest of a wait`.

## Task 2: The follow module, selection (D3, D4, D14)

**Files:** `crates/egret-perm-compile/src/follow.rs` (new), `src/lib.rs`; tests
`crates/egret-perm-compile/tests/follow_db.rs` (new, real store rows written through `create_grant_in_tx` /
`create_rule_in_tx`).

- [ ] **Red (DB):** `plan_via` on a catalog with, for the move `sales.orders → sales.orders_v2`:
  - `literal_rows_follow_by_effect` — deny `sales.orders` → `copy_grants`; allow `sales.orders` and allow
    `Sales.ORDERS.email` → `move_grants` (folded); path mask `sales.orders.email` and row filter `sales.orders` →
    `copy_rules`; a `workspace_assignment` grant → nowhere.
  - `restrictive_patterns_that_stop_matching_refuse` — deny `sales.ord*` (matches both: nowhere); deny
    `sales.orders*` with destination `sales.archive` → `refused`; row filter `sales` (schema level) on a move to
    `finance.orders` → `refused`; mask `*.orders.email` (schema pattern) on a move to `sales.x` → `refused`.
  - `allow_patterns_are_left` — allow `sales.orders*` with destination `sales.archive` → `left`; allow `sales` on a
    move to `finance.orders` → `left`; allow `sales.tmp_*` on `sales.x → sales.tmp_x` (newly matches) → nowhere.
  - `a_multi_level_destination_refuses_restrictions_and_leaves_allows` (D14).
  - `a_column_move_selects_column_rows` — move `sales.orders.email → sales.orders.mail`: deny and mask on `email`
    copy, allow on `email` moves, a mask on `e*` → `refused`, a table-level deny → nowhere.
  Expected on the base: compile error; with a stub `plan_via` returning an empty plan, every test red.
- [ ] **Fix:** spec §5.2 `Move`, `Coord`, `PermFollow`, `plan_via` (one query per table for the catalog's explicit
  grants and path rules, matched in Rust with `resource_level_matches`).
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile`; `cargo test -p egret-perm-compile --test follow_db --lib`.
- [ ] Commit `feat(perm): select the grants and rules a table or column move carries`.

## Task 3: The follow module, hand-written policies and the write helpers (D1, D5, D13)

**Files:** `follow.rs`; tests `follow_db.rs`.

- [ ] **Red (DB):**
  - `handwritten_restrictions_naming_the_table_refuse` — on a profile bound to the catalog, enabled operator
    policies: a mask on `lake.sales.orders.email` → `refuse`; an access policy with `denyPolicyItems` naming
    `[orders, customers]` → `refuse`; an allow with `isExcludes: true` on the table level listing `orders` →
    `refuse`; a row filter on `sales.orders` → `refuse`; a plain allow naming `orders` → `report`; a plain allow on
    `ord*` → neither; the same deny but disabled → neither; an `infra` policy → neither; the same deny on an unbound
    profile → neither; a deny whose catalog value is another catalog → neither.
  - `copies_get_new_ids_principals_and_unique_names` — `copy_in_tx` of a deny with two principals and of a mask
    rule named `pii-email` → new rows on `orders_v2` with the same principals/exemptions; the rule named
    `pii-email (sales.orders_v2)`; a second copy of another rule whose generated name collides gets ` #2`; the
    originals unchanged (`grant_hash`/`rule_hash` equal before and after).
  - `an_equivalent_row_on_the_destination_is_not_copied_again` (D13).
  - `delete_copies_removes_exactly_the_copies` and `move_allows_keeps_the_id`.
  Expected on the base: compile error; with stubs, red.
- [ ] **Fix:** spec §5.2 `handwritten_via`, `copy_in_tx`, `move_allows_in_tx`, `delete_copies_in_tx`,
  `refusal_message` (spec §8 wording).
- [ ] **Checks:** as Task 2.
- [ ] Commit `feat(perm): check hand-written policies and copy, move or undo a move's rows`.

## Task 4: Dormant restrictions count in the copy's compile (D12)

**Files:** `crates/egret-perm-compile/src/trigger.rs` (and the rule ceiling sites Step 0 named); tests
`crates/egret-perm-compile/tests/clearance_starrocks.rs` or a new `tests/follow_ceiling.rs` (stub `lakekeeper`).

- [ ] **Red (DB, real compiler):** `a_path_policy_on_a_leaving_table_still_counts` — a StarRocks profile whose
  ceiling is lowered (the test hook the #757 Task 2 tests use) so that N path masks on `orders` plus their N copies on
  `orders_v2` exceed it, while N alone fit; set the overlay `[{orders_v2: columns}, {orders: leaving_table}]` and
  recompile → refused naming the ceiling. Today: accepted, because `counts_body` skips the `orders` bodies (red).
  Guard: #757's `a_moving_table_is_counted_once_against_the_profile_total` stays green (classification bodies are
  still skipped).
- [ ] **Fix:** the profile-total subtraction (and any rule ceiling site Step 0 found) skips only bodies the
  classification compile produced; bodies of `perm-grant-*` / path `perm-rule-*` always count.
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile`; the touched test binaries; `cargo test -p egret-catalog-engine
  --test table_hook_db`.
- [ ] Commit `fix(perm): count dormant restrictions in a held rename's compile`.

## Task 5: Grant and rule writes wait their turn behind a hold (D10)

**Files:** `crates/egret-perm-compile/src/routes.rs`; tests the perm route test binary that drives `POST/PATCH/DELETE
/api/perm/grants` and `/rules` (find it with `/usr/bin/grep -ln "perm/grants" crates/egret-perm-compile/tests`).

- [ ] **Red (route):** with a name lock on `name_lock_key(prefix, "sales", "orders")` held by another connection:
  - `a_grant_write_on_a_held_name_is_refused` — POST a deny on `sales.orders` → 409 `table_change_held`; nothing
    written, no recompile (profile `policy_version` unchanged).
  - `a_patch_moving_a_grant_off_a_held_name_is_refused` — PATCH an existing grant on `sales.orders` to
    `sales.other` → 409.
  - `a_rule_write_and_deletes_on_a_held_name_are_refused` — POST a path mask on `sales.orders.email`; DELETE a grant
    and a rule on `sales.orders` → 409 each.
  - guard `a_pattern_write_takes_no_name_lock` — POST a deny on `sales.ord*` → 201.
  - guard `a_grant_on_a_catalog_without_an_embedded_warehouse_is_not_locked` → 201.
  Today: 201/200 (red).
- [ ] **Fix:** D10 (map `catalog_id` → `registry.catalogs.warehouse_id`; `try_name_lock_in_tx` per literal
  `(schema, table)` of the old row and the new body, sorted).
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile`; the perm route test binaries; `cargo test -p egret-bi` (the
  catalog-tag lock test still passes).
- [ ] Commit `feat(perm): refuse grant and rule writes on a name a rename is holding`.

## Task 6: A table rename copies its restrictions before the commit and waits for Trino (G2, D1, D6, D11)

**Files:** `crates/egret-catalog-engine/src/table_hook.rs`; tests `table_hook_db.rs`, `tests/common/plugins.rs`
(the fake Trino plugin also polls `/plugins/policies/download/{profile}` and reports its last 200's
`policyVersion` on the next poll, as E26 measured).

- [ ] **Red (DB, real Iceberg REST router):**
  - `a_renamed_table_keeps_its_restrictions_on_both_names` — `orders(id, email)`, no classification; a deny read
    grant to group `g`, a path mask on `orders.email`, a row filter on `orders`; both fake plugins running; rename to
    `orders_v2` → 2xx; the deny, mask and row filter exist on `orders_v2` (new ids) AND on `orders` (original ids,
    bodies unchanged); the served Trino and StarRocks policies restrict both names. Today: no hold, rows stay on
    `orders` only (red).
  - `the_rename_reaches_lakekeeper_only_after_trino_serves_the_copied_restrictions` — `observe`'s `on_reach` probe
    on `POST …/tables/rename`: at the reach the copies exist and the fake Trino plugin has reported the Trino
    profile's `policy_version` the copy left (ledger `kind = 'policies'`). Today: no copy (red); with the copy but
    no Trino policy pairs (mutation), the reach precedes the report (red).
  - `a_failed_rename_deletes_the_copies` — a live `orders_v2` exists → Lakekeeper's non-2xx returned as it is; no
    copy remains; originals intact.
  - `a_rename_no_trino_plugin_confirms_is_refused_and_undone` — Trino bound, its plugin stopped,
    `with_limit(2 s)` → 409 "… Nothing was renamed."; no copy; Lakekeeper still has `orders`.
  - guard `a_classified_rename_still_moves_its_classifications` — #757's `a_renamed_table_keeps_its_classifications`
    with a deny grant added: tags moved, deny copied, one wait.
- [ ] **Fix:** spec §5.3 (plan perm part, copy additions, Trino policy pairs, compensation additions). `finish`
  changes nothing for restrictive rows. No events yet (Task 7).
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile -p egret-catalog-engine`; `cargo test -p egret-catalog-engine
  --test table_hook_db --test view_hold_db --lib`.
- [ ] Commit `fix(catalog): copy a table's restrictions to its new name before a rename commits`.

## Task 7: Allows move after the commit; events by the caller (G2, D2, D9)

**Files:** `table_hook.rs`; `crates/egret-audit/src/governance.rs`; `crates/egret-perm-compile/src/governance_events.rs`;
`crates/egret-controlplane/src/governance_changes.rs`; tests `table_hook_db.rs`, the controlplane feed test binary.

- [ ] **Red:**
  - `a_renamed_tables_allows_move_and_keep_their_ids` — an allow read grant on `orders` (id `A`) and a column allow
    on `orders.email`; rename → `A` now names `orders_v2`, same id; nothing on `orders`; the hold did not wait (the
    reach precedes any policy poll report; no Trino pair). Today: stays on `orders` (red).
  - `an_allow_is_not_on_the_new_name_before_the_commit` — at the `on_reach` probe, `A` still names `orders` and no
    allow row names `orders_v2`.
  - `a_completed_rename_records_move_and_copy_events_by_its_caller` — the guard stub's catalog client; one
    `perm_grant:move` for `A`, one `perm_grant:copy` for the deny, one `perm_rule:copy` per rule, each with
    `detail.actor`, `rename`, `trigger = "iceberg_rename"`, `before`/`after`. A refused or undone rename records none.
  - feed route test: the sentences of spec §5.7 for a `policy:read` reader; a `tag:read`-only reader sees none.
- [ ] **Fix:** spec §5.3 `finish` additions; D9 actions (`perm_grant:move`, `perm_grant:copy`, `perm_rule:copy`) in
  `FAMILIES`/`GOVERNANCE_ACTIONS`; sentences.
- [ ] **Checks:** fmt; clippy `-p egret-audit -p egret-perm-compile -p egret-catalog-engine -p egret-controlplane`;
  `cargo test -p egret-audit`; `cargo test -p egret-catalog-engine --test table_hook_db`; the feed test binary.
- [ ] Commit `feat(catalog): move a renamed table's allow grants and record every move`.

## Task 8: Patterns refuse or are reported (G4, D4, D14)

**Files:** `table_hook.rs`; `governance.rs` (`perm_grant:left`); `governance_changes.rs`; tests `table_hook_db.rs`.

- [ ] **Red (router):**
  - `a_restrictive_pattern_that_would_stop_covering_the_table_refuses_the_rename` — deny `sales.orders*`; rename
    `orders → archive` → 409 whose message names the grant and its scope and ends "Nothing was renamed."; Lakekeeper
    still has `orders`; no lock remained (a second request is not "being held"); no version moved. Same for a row
    filter on schema `sales` and a cross-namespace rename to `finance.orders`. Today: 2xx (red).
  - `an_allow_pattern_is_left_and_reported` — allow `sales.tmp_*`; rename `tmp_a → a` → 2xx; the grant unchanged;
    one `perm_grant:left` event naming it and the rename; the feed sentence of spec §5.7.
  - `a_restriction_into_a_multi_level_namespace_refuses` (D14) — deny on `sales.orders`, destination
    `["sales", "eu"]` → 409.
  - `a_pattern_written_during_the_plan_is_caught_under_the_locks` — the test inserts a deny `sales.orders*` from the
    `observe` hook of the first lock (or between plan and copy via a test hook the hold exposes) → 409 from the copy
    transaction, copy rolled back.
  - guard `a_pattern_that_covers_both_names_changes_nothing` — deny `sales.ord*`, rename `orders → orders_v2` → 2xx,
    no copy of it, no event for it.
- [ ] **Fix:** spec D4, D14 in the plan and the copy re-check; the left event in `finish`.
- [ ] **Checks:** as Task 7.
- [ ] Commit `feat(catalog): refuse a rename a restrictive pattern would let escape`.

## Task 9: A covered destination does not refuse (G5, spec §5.9, D21)

**Files:** `crates/egret-perm-compile/src/follow.rs` (`cover_via`, `refusal_message`); `table_hook.rs`; tests
`follow_db.rs`, `table_hook_db.rs`.

- [ ] **Red (DB, `follow_db.rs`)**, each for the move `sales.orders → sales.archive` with a restrictive pattern row on
  `sales.orders*` that D4 refuses today:
  - `a_deny_pattern_is_covered_by_an_equal_deny_on_the_new_name` — pattern deny `read` to `group:contractors`;
    an exact-name deny `read, write` to `group:contractors` and `user:bob` on `sales.archive` → `Full` (more actions
    and subjects still cover). Also covered by another pattern `sales.arch*` with the same body, and by two
    candidates (`read` to the group, `write` to the group) for a pattern deny of `read, write`.
  - `a_mask_pattern_is_covered_by_an_equal_mask_on_the_new_name` — pattern mask `sales.orders*.email`, Hash,
    exemptions `{include: [role:pii-cleared]}`; a mask on `sales.archive.EMAIL` (folded), Hash, same expression, no
    exemptions → `Full`. A candidate with type `Nullify`, or `value_expr` differing by one space, → `Partial` whose
    `missing` names the column, type and expression.
  - `a_row_filter_pattern_is_covered_by_an_equal_filter_on_the_new_name` — pattern filter `region = 'eu'`; an
    exact-name filter with the same text and the same exemptions → `Full`; `region='eu'` (no spaces) → `Partial`.
  - `a_partial_cover_refuses_and_names_what_is_missing` — pattern deny `read` to `group:contractors` and
    `group:vendors`; the new name denies `read` to `group:contractors` only → `Partial { missing: ["read for
    group:vendors"] }`; a deny to a user who is a member of `vendors` does not cover the group (no expansion).
  - `a_copy_this_rename_makes_counts_as_a_cover` (D21) — pattern deny `sales.orders*` and a literal deny with the
    same body on `sales.orders`, which D1 copies → `Full`.
  - guards: a tag-selected mask and a hand-written policy on the new name are never candidates (`Partial`); egret
    inserts no row to create a cover (row counts unchanged after `cover_via`).
  Expected on the base: compile error; with a stub returning `Partial` for everything, the `Full` cases red.
- [ ] **Red (router, `table_hook_db.rs`):**
  - `a_covered_destination_lets_the_rename_through` — pattern deny `sales.orders*` and an equal exact-name deny on
    `sales.archive`; rename `orders → archive` → 2xx; the pattern row unchanged, no copy of it, the served policies
    restrict `archive` through the exact-name row. Today (Task 8 code): 409 (red).
  - `a_partially_covered_destination_is_refused_with_what_is_missing` — the exact-name deny names one group of two →
    409 whose message contains "lacks a deny of read for group:vendors" and ends "Nothing was renamed."
  - `a_cover_deleted_before_the_copy_refuses` — the cover is deleted between plan and copy (the Task 8 test hook) →
    409 from the copy transaction.
- [ ] **Fix:** spec §5.9 (`cover_via`, the union rule for denies, `missing`), D21; the plan and the copy re-check
  call it before refusing; `refusal_message` prints `missing` (spec §8).
- [ ] **Checks:** as Task 7, plus `cargo test -p egret-perm-compile --test follow_db`.
- [ ] Commit `feat(catalog): let a rename through when the new name is already as restricted`.

## Task 10: Hand-written restrictive policies refuse the rename (G3, D5)

**Files:** `table_hook.rs`; `governance.rs` (`ranger_policy:left`); `governance_changes.rs`; tests `table_hook_db.rs`.

- [ ] **Red (router):**
  - `a_handwritten_mask_naming_the_table_refuses_the_rename` — an enabled operator mask policy on a profile bound
    to the catalog (a manual profile: no compiled policy in it) naming `sales.orders.email` → 409 naming
    `profile/policy (id)`; nothing renamed; the policy JSON byte-identical (`version` unchanged). Same for an
    `isExcludes` allow and a deny. Today: 2xx (red).
  - `a_plain_handwritten_allow_is_reported` — 2xx; `ranger_policy:left` recorded; the policy unchanged.
  - guard `a_disabled_or_unbound_policy_does_not_refuse`.
- [ ] **Fix:** spec D5 in the plan and the copy re-check; the left event.
- [ ] **Checks:** as Task 7, plus `cargo test -p egret-policy --lib`.
- [ ] Commit `feat(catalog): refuse a rename a hand-written restriction names`.

## Task 11: The rename defaults (D11, D13, swap, #757 interplay)

**Files:** `table_hook.rs`; tests `table_hook_db.rs`. Each red fails on the Task 10 code; guards stay green and are
mutation-checked.

- [ ] **Red / guards:**
  - `the_swap_pattern_leaves_the_new_table_restricted` — `t` with deny `D`, allow `A`; `t_new` with allow `A2`;
    rename `t → t_old`, then `t_new → t` → `t_old` carries `D'` and `A`; `t` carries `D` (original id) and `A2`; the
    served policies restrict the new `t` for group `g` (evaluate a read by a `g` member with `egret-ranger-eval`:
    denied).
  - `a_rename_onto_a_name_with_dormant_copies_does_not_stack_them` (D13) — rename `t → t_old`, rename back
    `t_old → t`, rename `t → t_old` again → exactly one deny on each name.
  - guard `an_allow_only_rename_is_held_without_a_wait` — no plugin running at all, Trino bound → 2xx within 1 s,
    `A` moved (a wait would time out).
  - guard `a_case_only_rename_moves_nothing` (#757 C10) — `orders → Orders`: no copy, no update, no event.
  - guard `a_view_sent_to_the_table_rename_endpoint_moves_no_grant` (#757 C9).
  - guard `a_rename_whose_source_carries_nothing_takes_the_755_path`.
  - `a_migrated_restriction_stays_undoable` — migrate a hand-written deny into a grant (`POST …/migrate`), rename
    → the migration's undo still succeeds (originals untouched). Guard `moving_a_migrated_allow_makes_undo_refuse`
    pins spec §10's consequence (undo 409).
  - `a_non_2xx_after_a_committed_rename_still_moves_allows` — #757 review fix 4's observer answers 500 after
    Lakekeeper committed → allows moved, copies kept.
- [ ] **Fix:** whatever the reds need (expected: D13 wiring, the allow-only no-wait path).
- [ ] **Checks:** as Task 6.
- [ ] Commit `fix(catalog): keep the swap pattern restricted and skip what a rename does not carry`.

## Task 12: A column rename carries its column grants and rules (G1, spec §5.4)

**Files:** `table_hook.rs`; tests `table_hook_db.rs` (Trino's `RENAME COLUMN` body: `add-schema` with the same field
ids and `set-current-schema -1`).

- [ ] **Red (router):**
  - `a_renamed_column_keeps_its_mask_and_deny_on_both_names` — no classification; a path mask and a column deny on
    `orders.email`; rename `email → mail` → copies on `mail`, originals on `email`; at the reach the Trino plugin
    reported the copy's `policy_version`. Today: not held (red).
  - `a_renamed_columns_allow_moves` — a column allow on `email` → names `mail`, same id, after the commit.
  - `a_column_pattern_that_stops_matching_refuses` — mask on `e*`, rename `email → mail` → 409.
  - `swapped_columns_carry_both_sets` — mask `M1` on `a`, `M2` on `b`; swap → `a` and `b` each carry `M1` and `M2`
    copies or originals (spec §5.4).
  - guard `a_dropped_column_keeps_its_rows_and_takes_no_hold`; guard `an_append_is_not_held` (#757).
- [ ] **Fix:** spec §5.4 (the column plan keeps a pair when `plan_via` returns work or a refusal for it).
- [ ] **Checks:** as Task 6.
- [ ] Commit `fix(catalog): carry a column's grants and rules with it on rename`.

## Task 13: Catalog grants follow (KI-309, D8)

**Files:** `crates/egret-catalog-engine/src/store/embedded.rs`; `table_hook.rs`; `governance.rs`
(`catalog_grant:move`, `catalog_grant:delete`, `CATALOG_GRANT`, `Family::Catalog`); `governance_changes.rs`; tests
`table_hook_db.rs` (through the guard: the test mounts the real `EgretCatalogGuard` or a stub that applies
`catalog_grant_covers`; use what F-A's tests use), the feed test binary.

- [ ] **Red (router):**
  - `a_table_catalog_grant_follows_the_rename` — role `analysts` with a `table` read grant on `["sales"],
    "orders"`; an external client of that role; rename (sent by a client F-A authorizes) → 2xx; the grant now names
    `orders_v2`; the analysts client loads `sales.orders_v2` (200) and a new table created as `sales.orders` is 403
    for it. Today: the grant stays (red: 403 on `orders_v2`, 200 on the new `orders`).
  - `a_cross_namespace_rename_moves_the_namespace_too` — to `["finance"], "orders"`; a multi-level source and
    destination (`["sales","eu"]` → `["sales","us"]`) also moves.
  - `a_moved_grant_merges_with_an_identical_one` — the role already holds the same grant on the destination → one
    row remains.
  - `catalog_grants_need_no_hold` — no classification, no perm rows, a name lock on the destination held by the test
    → the rename is not refused (no hold) and the grant moves.
  - guard `a_namespace_grant_stays`; guard `a_case_only_rename_moves_no_catalog_grant`.
  - `a_catalog_grant_move_is_recorded_for_catalog_readers` — one `catalog_grant:move` with the caller; the feed shows
    it to a `catalog_registry:read` reader and not to a `policy:read`-only one.
- [ ] **Fix:** spec §5.5, D8, D9 (catalog part).
- [ ] **Checks:** fmt; clippy `-p egret-audit -p egret-catalog-engine -p egret-controlplane`; `cargo test -p
  egret-catalog-engine --test table_hook_db --test warehouse_admin_routes`; `cargo test -p egret-bin` (guard tests);
  the feed test binary.
- [ ] Commit `fix(catalog): catalog grants follow a renamed table`.

## Task 14: Recursive namespace drop (D15)

**Files:** `table_hook.rs` (`purge_namespace`); `embedded.rs`; tests `table_hook_db.rs`.

- [ ] **Red (router, memory warehouse):** `a_recursive_drop_purges_allows_and_catalog_grants_but_keeps_restrictions`
  — in `sales`: an allow on `sales.orders`, a column allow on `sales.orders.email`, a deny on `sales.orders`, a path
  mask, an allow on schema `sales`, an allow on `sales.ord*`, a `table` catalog grant on `["sales"], "orders"`, one
  on `["sales","eu"], "x"`, a `namespace` catalog grant on `["sales"]`; `DELETE …/namespaces/sales?recursive=true`
  → 2xx; the two literal allows and both table catalog grants are gone; the deny, the mask, the schema allow, the
  pattern allow and the namespace grant stay; `perm_grant:delete` (trigger `iceberg_drop_namespace`) and
  `catalog_grant:delete` events by the caller; the served policies no longer carry the deleted allows. Today:
  everything stays (red).
  Guard: a plain namespace drop deletes nothing (#757).
- [ ] **Fix:** spec §5.6.
- [ ] **Checks:** as Task 13.
- [ ] Commit `fix(catalog): purge allows and catalog grants with a recursively dropped namespace`.

## Task 15: Stale-reference report (D16)

**Files:** `crates/egret-perm-compile/src/table_columns.rs` (`table_states_ns_via`); `governance_overview.rs`; tests
`table_hook_db.rs` (drift), `crates/egret-controlplane/tests/governance_attention.rs` (its `lakekeeper` stub).

- [ ] **Red (drift, real Lakekeeper):** `table_states_follow_lakekeeper_at_any_depth` — a table in `["sales","eu"]`
  → `Live`; after a rename the old multi-level name → `Missing`; case folded per element; single-level answers
  equal `table_states_via`'s. Expected on the base: compile error; with a stub returning `Missing`, red.
- [ ] **Red (route):** `permissions_on_missing_tables_are_findings` — a deny and a mask on `ghost` (no tabular), an
  enabled operator policy naming `sales.ghost2` literally, a deny on live `orders` → `GET /governance/attention`
  for a `policy:read` reader has `permissions_on_missing_tables` (warning, count 2, samples with kinds; a copy event
  naming `ghost` adds "kept when the table was renamed to …"); `orders` absent; a `tag:read`-only reader gets it in
  `notChecked`. `permissions_on_missing_columns` — a mask on `orders.email` after a field-id rename to `mail` →
  "renamed to mail". `catalog_grants_on_missing_tables` — table grants on `["sales"],"ghost"` and
  `["sales","eu"],"ghost"` → count 2 for a `catalog_registry:read` reader; a `policy:read`-only reader gets it in
  `notChecked`. Today: no such kinds (red).
- [ ] **Fix:** spec §5.8 (three sources).
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile -p egret-controlplane -p egret-catalog-engine`; `cargo test -p
  egret-controlplane --test governance_attention`; `cargo test -p egret-catalog-engine --test table_hook_db`.
- [ ] Commit `feat(governance): report grants, rules and catalog grants on tables that do not exist`.

## Task 16: Names a rename left without their allows (D17)

**Files:** `governance_overview.rs`; tests `governance_attention.rs`.

- [ ] **Red (route):** `a_reused_name_that_lost_its_allows_is_a_finding` — a `perm_grant:move` event `t → t_old`
  (inserted as the hook stores it) and a live `t` carrying none of the moved allows → `renamed_names_lost_grants`
  for a `policy:read` reader, sample ref `Object` of `t_old`; a `t` that carries an equivalent allow again → not
  reported. The same for `catalog_grant:move` → `renamed_names_lost_catalog_grants` for a `catalog_registry:read`
  reader. Today: no such kinds (red). Guard: #757's `renamed_names_reused` unchanged.
- [ ] **Fix:** spec D17.
- [ ] **Checks:** as Task 15.
- [ ] Commit `feat(governance): report names a rename left without their allow grants`.

## Task 17: Frontend

**Files:** `frontend/src/routes/governance/overview/AttentionSection.tsx`, its `__tests__`; the feed's family filter
if it lists families (check with `/usr/bin/grep -rn "tag:read\|policy:read" frontend/src/routes/governance`).

- [ ] **Red (Vitest):** the "Not checked for you" lines for the five kinds read "permissions on tables that do not
  exist", "permissions on columns that do not exist", "catalog grants on tables that do not exist", "table names a
  rename left without their allow grants", "table names a rename left without their catalog grants" (today: the raw
  kind).
- [ ] **Fix:** five `CHECKS` entries; nothing else changes on screen.
- [ ] **Checks:** `npm run lint`; `npx tsc -b`; `npx vitest run src/routes/governance`; if `openapi.json` changed in
  Tasks 7-15, `npm run gen:api` and commit the client.
- [ ] Commit `feat(governance): name the new stale-permission checks`.

## Task 18: Final gate

- [ ] **Backend** (one command at a time; `pgrep -x cargo` empty first):

```bash
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test --workspace --no-fail-fast 2>&1 | tee "$SCRATCH"/grmove-ws.log | tail -40
grep -c "skip: DATABASE_URL" "$SCRATCH"/grmove-ws.log
grep -E "^test result: FAILED|panicked" "$SCRATCH"/grmove-ws.log | head
just test-shipping-resolution
just test-ranger-gates 2>&1 | tee "$SCRATCH"/grmove-ranger.log | tail -20
cargo run -q -p egret-bin --bin egret -- openapi > openapi.json && git diff --exit-code openapi.json
```

- [ ] **Frontend** (from `frontend/`): `npm run gen:api && git status --short src/api/generated` (no change);
  `npm run lint`; `npx tsc -b`; `npm run test`; `npm run build`; the bundle tests.

Expected: every command exits 0; the skip grep prints `0`. Known environment-only failures (#757 §14: `egret-workflow
--test sql_starrocks_live` without a live StarRocks) are named, not ignored. Read the full log before calling it
green.

## Task 19: Live check (own compose project `egret-grmove`, port 18780; never 18080)

Compose override in the scratchpad (`-p egret-grmove`, `DEMO_PORT=18780`; confirm free with `lsof -i :18780`), with a
real Trino (Ranger plugin 2.5.0) and a real StarRocks (Ranger plugin 2.8/2.9) on one egret Iceberg catalog, the
warehouse on a soft-delete profile, the default 30 s polls. Results, timings and screenshots go to the scratchpad. Any
mismatch between egret's state and what an engine returns is a defect, not a note.

1. **Setup:** `sales.orders(id int, email varchar, region varchar)`, 4 rows (two `region = 'eu'`). Users: `reader-u`
   (allow read on `sales.orders`), `contractor-u` (allow read on the schema `sales` plus a **deny** read grant on
   `sales.orders` to group `contractors`), `analyst-u` (allow read). A path **mask** (Hash) on `sales.orders.email`
   for everyone except `pii-cleared`; a path **row filter** `region = 'eu'` on `sales.orders` for `analyst-u`. A
   catalog role `nb-analysts` with a `table` read grant on `["sales"], "orders"`, and a notebook client of that role.
   Baseline on both engines: `reader-u` reads `email` hashed and all rows; `contractor-u` refused; `analyst-u` sees 2
   rows; the notebook client loads `sales.orders`.
2. **Rename through a 2 s read loop:** in a second session, every 2 s, on BOTH engines, as `contractor-u` and
   `analyst-u`: `SELECT * FROM sales.orders_v2` (and `sales.orders` until it is gone). From Trino: `ALTER TABLE
   sales.orders RENAME TO orders_v2`; record its duration. Expect for the whole window: `contractor-u` never reads a
   row; `analyst-u` never sees a non-`eu` row; `email` never in clear for anyone uncleared. `reader-u` may be refused
   on `orders_v2` for up to two polls after the commit (G6's accepted gap; record its length for the docs'
   Limitation), then reads it hashed. The ledger shows
   Trino's `policies` row and StarRocks' row reporting the copy's versions before the commit time in egret's log.
   Then: the deny, mask and filter exist on both names; the allows name `orders_v2` with their old ids; the events
   in Governance > Changes name Trino's catalog client.
3. **Catalog grant before and after:** the notebook client loads `sales.orders_v2` (200), and `sales.orders` is
   404; `nb-analysts`' grant shows `orders_v2` in the warehouse Roles tab.
4. **Swap pattern:** create `sales.orders_new`, give `reader-u` a grant on it; rename `orders_v2 → orders_old`, then
   `orders_new → orders_v2`. As `contractor-u` on both engines: the new `orders_v2` is refused; as uncleared users
   its `email` (if the column exists) is hashed. The notebook client can load `orders_old`, not the new `orders_v2`.
   The Overview shows `renamed_names_lost_grants` and `renamed_names_lost_catalog_grants` naming `orders_v2`.
5. **G4 409:** add a deny on `sales.orders_*` to `contractors`; rename `orders_old` to `archive` from Trino → Trino
   prints the 409 naming the grant; `SHOW TABLES` still lists `orders_old`. Add an allow on `sales.tmp_*`, create
   `tmp_a`, rename `tmp_a → a` → 2xx; Changes shows the `perm_grant:left` sentence.
   **G5:** add an exact-name deny on `sales.archive` to `contractors` with the same action → the same rename of
   `orders_old` to `archive` now succeeds and `contractor-u` is refused on `archive` on both engines from the first
   read; remove `contractors` from a second pattern-deny group first to see a partial cover refused with the missing
   subject named.
6. **G3 409:** on a manual Trino profile bound to the catalog (or the StarRocks one if only it is manual), write an
   operator mask policy naming `sales.orders_old.email`; rename `orders_old → orders_x` → 409 naming the policy; the
   policy's JSON is unchanged. Delete the policy; the rename succeeds.
7. **Column rename:** `ALTER TABLE sales.orders_x RENAME COLUMN email TO mail`, with the same 2 s loop: `mail` hashed
   for uncleared users from the first read on both engines; the mask exists on `mail` and `email`.
8. **Timing with both Trino waits:** classify `sales.orders_x` (whole table, a clearance) so the rename waits for a
   Trino tag pair and a Trino policy pair; rename it and record the duration and the ledger's tag `-1` regression
   (E27); it must finish under 90 s (spec §10). Repeat three times.
9. **Silent Trino:** stop Trino, wait 100 s, rename a table with a deny grant from StarRocks or the REST API → 409
   after 90 s naming the Trino plugin or `<no plugin has polled>`; restart; the rename then succeeds. A table with
   only an allow renames at once while Trino is stopped.
10. **Recursive namespace drop** (throwaway `gm_drop` with a table carrying an allow, a deny and a table catalog
    grant; `force=true` on the soft-delete warehouse, #757 F8): the allow and the catalog grant are gone, the deny
    stays; the events name the caller.
11. **UI:** the five Overview findings, the Changes sentences for every new action (and the Who column), 1440 and
    390 px, light and dark.

## Task 20: Pull request

- [ ] Push only after the user says so. Title `fix(catalog): grants, rules and catalog grants follow a table
  rename`. Body: spec link; G1-G6; D1-D21 marked as defaults the user may revisit; the prerequisite F-A PR; gate
  output; live results with the read-loop timings and the step 8 durations; the release note of spec §9; the Docs box
  naming the follow-up docs PR (KI-308, KI-309, spec §11's rows and the G6 Limitation of spec §9, D20); non-goals;
  related defects. Append a dated note to #757's spec §14 saying its §11 items 1-3 are fixed by this package
  (append-only). End with the attribution line from the session reminder.

## Self-review

- **Spec coverage:** D7 → T1; §5.2 → T2, T3; D12 → T4; D10 → T5; §5.3 → T6, T7, T11; D4/D14 → T8; G5/§5.9/D21 →
  T9; D5 → T10; §5.4 → T12; §5.5/D8 → T13; §5.6/D15 → T14; §5.8/D16 → T15; D17 → T16; frontend → T17; §10 risks →
  T19 (steps 2, 8, 9).
- **Ordering:** each task ends green. T2-T3 before T6 (the hook uses `follow`); T4 before T6 (the copy's compile);
  T5 independent (after T3 only for shared fixtures); T7 needs T6; T8-T12 need T7 (events); T9 needs T8 (the
  refusal path); T13 needs T7's actor plumbing and Family work; T14 needs T13; T15 needs T7 (copy events for sample
  details); T16 needs T7 and T13; T17 after T16; T18 after T17; T19 after T18.
- **User decisions:** G1 → T2-T16 (F-A is Step 0's prerequisite; views are spec §11 item 1); G2 → T6 (copies stay),
  T7 (allows move after), T11 (swap); G3 → T3, T10; G4 → T2, T8, T12; G5 → T9, T19 step 5; G6 → T7 (allows move
  only in `finish`), T19 step 2 (the gap measured), spec §9 (the Limitation).
- **Open questions:** none (spec §13).
