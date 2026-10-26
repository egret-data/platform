# StarRocks table-level classification compile (Classification UX, Package 2, D2): implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** a classification attached to a whole table enforces on StarRocks as on Trino: a `column: *` clearance deny
per (classification, table), a NULL mask per real column, a row filter per table; a table-commit hook and a boot pass
keep it current; Explain and the UI stop stating the gap.

**Architecture:**

- One expansion returns both attachment levels; the compiler folds case once and groups by table.
- The column source reads Lakekeeper's current schema over the compile's own connection; a drift test against real
  Lakekeeper guards it.
- Clearance: a `*` deny per table-level (tag, table), masks per column through the existing merge and fold; an unknown
  table keeps the deny and reports the missing masks.
- A middleware beside the view hold dry-runs, then recompiles, the bound StarRocks profiles on a schema change.
- Explain and the rule effect lose the StarRocks table-level gap fields and gain `clearance_masks_missing`.

**Tech stack:** Rust (axum 0.8, sqlx 0.9, utoipa), `egret-ranger-eval`, embedded Lakekeeper, React 18 + orval client,
Vitest.

**Spec:** `docs/en/specs/2026-10-25-starrocks-table-classification-design.md`. Read §2–§5 first. Every `file:line`
there was checked on `596d8eea`; re-check with `/usr/bin/grep -n` before editing (Step 0).

## Global Constraints

- **Worktree:** `egret-srtbl`, branch `feat/starrocks-table-classification`. Work only there.
- **Build isolation:** `export CARGO_TARGET_DIR=$PWD/target` from the worktree root, never another checkout's target.
  `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`. Keep 30 GiB free;
  `./deploy/prune-target.sh` before the gate.
- **Machine:** no `rg` (use `/usr/bin/grep`); zsh: quote globs; `rm`/`cp` are aliased (use `/bin/rm -f`,
  `/bin/cp -f`); never `git stash`, `git checkout --` or `git clean`; `pgrep -x`, never `pgrep -f`.
- **SQL:** sqlx 0.9 `query()` takes `&'static str`; no `format!` SQL. The `lakekeeper.` schema is written as a literal;
  a unit test asserts it equals `egret_catalog_engine::pool::LAKEKEEPER_SCHEMA` (in the catalog-engine drift test).
- **DB-gated tests must run, not skip:** `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`, under
  `env -u EGRET_ALLOW_DB_SKIP`; afterwards `grep -c "skip: DATABASE_URL"` prints `0`. Never the database `egret`. One
  suite at a time.
- **Red first, through the real compiler or handler.** Every behaviour gets a failing test that drives
  `recompile_service_in_tx` (or the router with `tower::oneshot`, as `tests/routes.rs` does), failing for the stated
  reason, before the fix. Engine-level assertions evaluate the served artifact with `egret-ranger-eval`, never a JSON
  substring. Mutation-check each new assertion once (break the fix, watch it fail, restore from a backup with
  `cat backup > file`; `git status --porcelain` lists only intended files).
- **Per-task checks** (each task lists its own): `cargo fmt --all -- --check`; `cargo clippy -p <touched crates>
  --all-targets -- -D warnings`; the touched test binaries with the per-process test DB; `just test-ranger-gates` when
  the evaluator, `harness_support.rs` or the differential is touched.
- **OpenAPI:** after a schema change, `cargo run -q -p egret-bin --bin egret -- openapi > openapi.json`, then
  `cd frontend && npm run gen:api` (never bare `orval`); commit both.
- **Copy:** parent glossary and `docs/en/frontend-rules.md` (FE-COPY-1..4, FE-VISUAL-1/4, FE-TEST-*).
- **Commits:** one per task, Conventional Commits (`feat(perm)`, `feat(catalog)`, `feat(governance)`, `test(perm)`),
  trailer exactly `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `Closes #N`, no scratch files, no
  `/Users/...` paths. Do not push until the user says so.
- **Migrations:** none (spec C16). If one becomes necessary, take the next number free above main AND every open PR at
  merge time, and record it in spec §14.
- **Stacks:** the demo stack on `127.0.0.1:18080` belongs to another session; never stop, reset or seed it.
- **Deviations** go to spec §14 (append-only), per task.

## File structure

| file | change | task |
|---|---|---|
| `crates/egret-ranger-eval/src/resource.rs` | export `fold_identifier` | 1 |
| `crates/egret-policy/src/store.rs` | `tag_assignments_for_service_via`; delete `tagged_columns_for_service_via` | 1 |
| `crates/egret-perm-compile/src/rules.rs` | `TableTarget`, `TaggedColumn::new`, row-filter tables input | 1, 5 |
| `crates/egret-perm-compile/src/clearance.rs` | `*` deny, C6 dedup, MV contributors, masks, U4 skip, ceiling | 2, 4, 6 |
| `crates/egret-perm-compile/src/clearance_mask.rs` | folded merge key, spelling rule | 1 |
| `crates/egret-perm-compile/src/table_columns.rs` (new) | column source, batch, overlay | 3, 8 |
| `crates/egret-perm-compile/src/trigger.rs` | expansion callers, boot pass, affected-profile query | 1, 5, 7, 9 |
| `crates/egret-perm/src/actions.rs` | ceiling error fields | 6 |
| `crates/egret-catalog-engine/src/table_hook.rs` (new), `mount.rs`, `view_hold.rs` | hook; shared prefix resolution | 7, 8 |
| `crates/egret-bin/src/main.rs` | boot pass spawn; hook construction | 7, 9 |
| `crates/egret-perm-compile/src/{routes,explain_effect,effect}.rs` | explain changes; gap removal | 10, 11 |
| `crates/egret-perm-compile/src/harness_support.rs`, `crates/egret-ranger-eval/tests/differential.rs` | differential | 14 |
| `crates/egret-perm-compile/tests/support/lakekeeper_stub.rs` (new) | stub `lakekeeper` tables for perm-compile tests | 3 |
| `crates/egret-perm-compile/tests/{clearance_starrocks,compile_rules,clearance_triggers,routes}.rs` | red tests | 1–6, 9–11 |
| `crates/egret-catalog-engine/tests/table_hook_db.rs` (new) | drift test; hook tests | 3, 7, 8 |
| `openapi.json`, `frontend/src/api/generated/**` | regenerated | 12 |
| `frontend/src/routes/governance/{RuleDialog,RulePreview}.tsx`, `data/AddTagDialog.tsx`, `classifications/tabs/ClearanceTab.tsx` and their `__tests__` | removals and copy | 13 |

## Step 0: Re-verify the evidence on the then-current main

- [ ] `git fetch origin && git log --oneline -1 origin/main`. If main moved past `596d8eea`, rebase and record the base
  in spec §14.
- [ ] Re-check each row; record drift in spec §14; stop and report if a row is false (not merely moved).

| spec row | check |
|---|---|
| E1 | `a.column_name <> ''` in `tagged_columns_for_service_via` (`store.rs`) |
| E2, E3 | `for col in &tagged` and `mv_deny_inputs(&out.masks` in `clearance.rs` |
| E4 | `let key = (` in `merge_by_column` uses `p.column.*.clone()` verbatim |
| E5 | `tagged_columns_for_service_via` in `compile_one_rule`; `let mut seen` in `rules.rs` |
| E9 | `a_table_level_deny_does_not_reach_a_mask_null_column_on_starrocks` still passes on main |
| E10 | the `.sqlx-lakekeeper` files still name `table_current_schema`, `table_schema`, `tabular_namespace_name`, `deleted_at`; Lakekeeper rev in `crates/egret-catalog-engine/Cargo.toml` unchanged |
| E11 | `classify` in `view_hold.rs` has no `"tables"` arm |
| E12 | `/usr/bin/grep -rn "UPDATE catalog_tag_assignments" --include='*.rs' crates` → none outside tests |
| E17 | the gap items: `/usr/bin/grep -rn "starrocks_table_only\|clearance_not_enforced\|ROW_FILTER_GAPS_TABLE_ONLY\|StarRocksTableOnlyNotice" crates frontend/src` |
| E22 | `ls migrations \| tail -1`; `gh pr list --state open` and each PR's migrations |

## Task 1: Both levels in the expansion; one case fold (spec §5.1, C5)

**Files:** `crates/egret-ranger-eval/src/resource.rs`, `crates/egret-policy/src/store.rs`,
`crates/egret-perm-compile/src/{rules,clearance,clearance_mask,trigger}.rs`, tests
`crates/egret-perm-compile/tests/clearance_starrocks.rs`, `src/clearance_mask.rs` (unit).

- [ ] **Red (unit, `clearance_mask.rs`):** `two_spellings_of_one_column_merge_into_one_mask` — `PendingMask`s for
  `sales.Orders.Email` (tag A) and `sales.orders.email` (tag B) → one merged mask, cleared = intersection.
  `the_catalog_spelling_wins_the_merge` — a pending mask marked as catalog-spelled keeps its spelling.
- [ ] **Red (DB, `clearance_starrocks.rs`):** `a_column_tagged_in_two_spellings_compiles_one_clearance_mask` — two
  gated tags on `Orders.Email` and `orders.email` → exactly one `perm-clearance-*-mask-*` on the column in the
  served download (today: two masks, then the overlap refusal fails the compile).
  `a_column_only_table_keeps_its_policy_names` — a fixture with one spelling: every policy name and body is
  byte-identical before and after (guards C5's no-churn rule).
- [ ] **Fix.** Export `fold_identifier` (the evaluator's `fold`). Add `tag_assignments_for_service_via` (both levels,
  verbatim, `ORDER BY c.name, a.schema_name, a.table_name, a.column_name`) and `TagAssignmentRow`; delete
  `tagged_columns_for_service_via` and the `PolicyStore` wrapper. Add `TableTarget` grouping under the folded key,
  `TaggedColumn::new` refusing an empty level (returns `CompileError::EmptyScopeLevel`). Move both callers
  (`clearance.rs`, `compile_one_rule`) to the new read, still using **column-level rows only** in this task (behaviour
  for table-level rows comes in Tasks 2–5). Fold the merge key.
- [ ] **Checks:** fmt; clippy `-p egret-ranger-eval -p egret-policy -p egret-perm-compile`; `cargo test -p
  egret-perm-compile --test clearance_starrocks --test compile_rules --lib`; `cargo test -p egret-policy --lib`.
- [ ] Commit `refactor(perm): one tag expansion for both levels, folded once`.

## Task 2: The whole-table clearance deny (U1, C6, C7)

**Files:** `clearance.rs`; tests `clearance_starrocks.rs` (new helper `tag_table`).

- [ ] **Red:**
  - `a_table_level_clearance_compiles_one_wildcard_deny_per_table` — gate tag on table `orders` only → exactly one
    access policy `perm-clearance-{tag}-{c}.sales.orders.*`, resources `column: ["*"]`, `{public}` denied,
    `denyExceptions` = cleared users, `select`; evaluated with `egret-ranger-eval`: uncleared column request denied,
    cleared allowed, a request for column `_file` denied. Today: no policy.
  - `a_table_level_attachment_covers_the_column_denies_of_its_tag` — same tag on the table and on `orders.email` → the
    `*` deny only (no per-column deny); a second tag on `email` keeps its own per-column deny.
  - `no_table_level_compile_emits_an_empty_resource_value` — walk every desired policy of a table-level fixture: no
    `resources.*.values` contains `""` (E8). Mutation: build the column from the raw row → red.
  - `a_table_only_classification_narrows_the_materialized_view_deny` — only a table-level gated tag → the MV deny
    exists, excepting its cleared users (E3).
- [ ] **Fix** per spec §5.3 steps 1 (deny only; masks wait for Task 4), 2 and 4.
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile`; `cargo test -p egret-perm-compile --test clearance_starrocks
  --test clearance --test clearance_triggers`.
- [ ] Commit `feat(perm): a whole-table clearance deny on StarRocks`.

## Task 3: The column source and its drift guard (spec §5.2, C2–C4)

**Files:** `crates/egret-perm-compile/src/table_columns.rs` (new), `src/lib.rs`;
`crates/egret-perm-compile/tests/support/lakekeeper_stub.rs` (new);
`crates/egret-catalog-engine/tests/table_hook_db.rs` (new).

- [ ] **Red (catalog-engine, real Lakekeeper, reuse `view_hold_db.rs`'s `lakekeeper_ready`/`create_memory_warehouse`
  setup):** `the_column_source_reads_what_lakekeeper_loads` — create `sales.orders(id int, email string, region string)`
  through Lakekeeper's REST router; after each of: create, add column `phone`, rename `region`→`area`, drop `phone`,
  assert `current_columns_via` == `load_table_detail(..).columns` names (order included); rename the table → old name
  `None`, new name the columns; drop → `None`. A view of the same name → `None`. A staged create (no
  `metadata_location`) → `None`. Also asserts the SQL literal schema equals `LAKEKEEPER_SCHEMA`. Expected on the base:
  compile error (no module).
- [ ] **Red (perm-compile, stub):** `table_columns` unit-ish DB tests over the stub: case-folded lookup returns the
  catalog spelling; two fields folding equal collapse to the lower field id; a multi-level namespace → `None`.
- [ ] **Fix.** The query of spec §5.2, its batched `unnest` variant, JSON field extraction. The stub creates only the
  tables/columns the query reads (`CREATE SCHEMA IF NOT EXISTS lakekeeper` …); its doc names the drift test as its
  guard. Pin the exact Lakekeeper column names and casts from the drift test; record any difference from the spec's
  sketch in §14.
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile -p egret-catalog-engine`; `cargo test -p egret-catalog-engine
  --test table_hook_db`; `cargo test -p egret-perm-compile --lib`.
- [ ] Commit `feat(perm): read a table's current columns from the embedded catalog`.

## Task 4: NULL masks for every column of a whole-table classification (C1, C5)

**Files:** `clearance.rs`, `clearance_mask.rs`; tests `clearance_starrocks.rs` (stub tables).

- [ ] **Red:**
  - `a_table_level_clearance_masks_every_column_of_the_table` — stub `orders(id, email, region)`, gated tag on the
    table → three priority-1 `MASK_NULL` clearance masks; evaluated: uncleared `MASK_NULL` on each column, cleared user
    gets the rule mask's outcome where a rule masks `email`.
  - `a_column_and_a_table_attachment_give_one_mask_per_column` — tag A on the table, tag B on `Email` (other case) →
    one mask on `email`, catalog spelling, cleared = A ∩ B, named after the lower tag id; no refusal.
- [ ] **Fix:** spec §5.3 step 1 `Some(cols)` branch.
- [ ] **Checks:** as Task 2.
- [ ] Commit `feat(perm): NULL masks for whole-table classifications on StarRocks`.

## Task 5: Row filters on a whole-table classification (C8)

**Files:** `rules.rs`, `trigger.rs` (`compile_one_rule`); tests `compile_rules.rs`, `clearance_starrocks.rs` or
`routes.rs` (route test inverted in Task 11).

- [ ] **Red:** `a_starrocks_tag_row_filter_on_a_table_attachment_filters_the_table` (compile_rules, unit) — tables input
  `[orders]` → one `perm-rule-{id}-{c}.sales.orders` body identical in shape to the column-derived one;
  `a_row_filter_on_both_levels_of_one_table_is_one_policy` — table-level `Orders` + column-level `orders.email` → one
  policy; `a_row_filter_ceiling_counts_tables` — 501 tables refused, 1 table with 600 tagged columns accepted.
- [ ] **Fix:** spec §5.4.
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile`; `cargo test -p egret-perm-compile --test compile_rules --test
  clearance_starrocks --test overlap`.
- [ ] Commit `feat(perm): StarRocks row filters on whole-table classifications`.

## Task 6: Unknown table report and ceiling messages (U3, U4, C9)

**Files:** `clearance.rs`, `crates/egret-perm/src/actions.rs`, `trigger.rs` (profile ceiling); tests
`clearance_starrocks.rs`, `routes.rs` (one route test for the compile response).

- [ ] **Red:**
  - `an_unknown_table_keeps_its_deny_and_reports_the_missing_masks` — table-level gate on `ghost` (no stub row) → the
    `*` deny exists, no mask, `CompileSummary.skipped` has one clearance item naming `c.sales.ghost`; through the rule
    write route the `CompileResponse.skipped` carries it.
  - `a_wide_table_level_clearance_is_refused_with_its_tables` — stub `wide` with 520 columns → refused,
    message contains `wide` and `520`; a 400-column table accepted (count 401).
  - `the_profile_ceiling_names_the_largest_tables` — growth over 1000 via table-level masks → message names them.
- [ ] **Fix:** spec §5.3 steps 1 (`None`) and 3; `ProfileExpansionCeiling.largest`.
- [ ] **Checks:** fmt; clippy `-p egret-perm -p egret-perm-compile`; `cargo test -p egret-perm-compile --test
  clearance_starrocks --test routes`.
- [ ] Commit `feat(perm): report missing masks and name tables in ceiling refusals`.

## Task 7: Table-commit hook, after-commit recompile (U2, C10, C12)

**Files:** `crates/egret-catalog-engine/src/table_hook.rs` (new), `mount.rs`, `view_hold.rs` (share `resolve`),
`crates/egret-perm-compile/src/trigger.rs` (`affected_starrocks_profiles_for_table`), `crates/egret-bin/src/main.rs`;
tests `crates/egret-catalog-engine/tests/table_hook_db.rs`, unit tests for `classify`.

- [ ] **Red (unit):** the classifier table of spec §5.5: each path/method → its kind; an update with only
  `add-snapshot` → pass; `GET` → pass; a view path → not this hook.
- [ ] **Red (DB, real Lakekeeper + a StarRocks profile bound to the warehouse):**
  `adding_a_column_to_a_classified_table_masks_it_before_the_commit_returns` — table-level gated tag on `orders`; Iceberg
  commit `add-schema` (+ `phone`) + `set-current-schema -1` through the mounted router → when the response arrives,
  the served download has a clearance mask on `phone`. Today: none until an unrelated recompile.
  `a_drop_reaps_the_masks_and_keeps_the_deny`; `a_commit_on_an_unclassified_table_runs_no_compile` (the profile's
  `policy_version` unchanged); `a_failed_recompile_still_returns_the_commit_and_is_recorded` (force a refusal, e.g. a
  pre-stored tie fixture; response 2xx, a `perm.compile_runs` failure row).
- [ ] **Fix:** spec §5.5 steps 1–3, 5–6.
- [ ] **Checks:** fmt; clippy `-p egret-catalog-engine -p egret-perm-compile -p egret-bin`; `cargo test -p
  egret-catalog-engine --test table_hook_db --test view_hold_db --lib`.
- [ ] Commit `feat(catalog): recompile StarRocks profiles when a classified table's schema changes`.

## Task 8: Table-commit hook, pre-commit dry run (C11)

**Files:** `table_hook.rs`, `table_columns.rs` (overlay); tests `table_hook_db.rs`.

- [ ] **Red:** `a_column_add_that_would_cross_the_ceiling_is_refused_before_commit` — table at 499 counted policies for
  its tag; add two columns → 409 Iceberg error whose message names the table and counts; Lakekeeper's schema unchanged
  (load the table). `a_column_add_within_the_ceiling_commits` — the dry run leaves nothing written (no policy row
  changed by the dry run, checked between dry run and commit with a pausing observer as `view_hold_db.rs` does).
  `the_overlay_is_invisible_outside_the_dry_run` (perm-compile, stub) — `SET LOCAL` in one transaction; another
  transaction reads the stored columns.
- [ ] **Fix:** spec §5.2 overlay; §5.5 step 4.
- [ ] **Checks:** as Task 7.
- [ ] Commit `feat(catalog): refuse a table change whose recompile would be refused`.

## Task 9: Boot pass (C13)

**Files:** `trigger.rs` or `role_membership.rs` (beside `sync_admin_policies`), `lib.rs`, `crates/egret-bin/src/main.rs`;
tests `crates/egret-perm-compile/tests/clearance_triggers.rs`.

- [ ] **Red:** `the_boot_pass_compiles_table_level_classifications_written_before_upgrade` — write a table-level gated
  assignment and a classification row filter directly (no recompile), run the pass → `*` deny, masks, row filter
  served; a second run writes nothing. `one_refusing_profile_does_not_stop_the_others` — two StarRocks profiles, one
  with a differing row-filter tie → the other is compiled, the first recorded failed.
- [ ] **Fix:** spec §5.6; spawn in `main.rs` beside `sync_admin_policies`, logging the count.
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile -p egret-bin`; `cargo test -p egret-perm-compile --test
  clearance_triggers --test admin_policy_sync`.
- [ ] Commit `feat(perm): a boot pass for StarRocks table-level classifications`.

## Task 10: Explain for the new state (spec §5.7)

**Files:** `routes.rs`, `explain_effect.rs`; tests `tests/routes.rs`, `explain_effect.rs` table test.

- [ ] **Red (route):**
  - invert `a_table_level_clearance_is_not_enforced_on_starrocks` into
    `a_table_level_clearance_refuses_an_uncleared_reader_on_starrocks` — the StarRocks line reads "… is **refused**
    `email` (clearance *PII*, on the whole table)"; no `clearance_not_enforced` key.
  - `a_reader_cleared_for_a_table_and_a_column_classification_is_told_both` — `cleared_for` names both.
  - `an_unknown_table_says_its_masks_are_missing` — `clearance_masks_missing[0].reason == "table_not_in_catalog"` and
    the sentence; `a_column_added_since_the_compile_says_not_compiled` (stub gains a column, no recompile).
- [ ] **Red (unit table):** the two new wordings; the D2 sentence row removed.
- [ ] **Fix:** spec §5.7 (excluding removals of Task 11 that are not needed for these to pass).
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile`; `cargo test -p egret-perm-compile --test routes --test
  explain_principal --lib`.
- [ ] Commit `feat(perm): explain whole-table clearances on StarRocks`.

## Task 11: Remove the Package 2 gap statements, backend (C15)

**Files:** `effect.rs`, `routes.rs`, `explain_effect.rs`, `store.rs` doc; tests `tests/routes.rs`.

- [ ] **Red:** invert `a_starrocks_row_filter_on_a_table_classification_filters_nothing` into
  `a_starrocks_row_filter_on_a_table_classification_filters_rows` — the rows line reads "sees **rows where** …", no
  StarRocks entry in `row_filter_not_applied`; the preview's `RuleEffect` has no `starrocksTableOnly` key; Trino's
  `trinoColumnOnly` and D3 line unchanged (existing tests stay green).
- [ ] **Fix:** delete `starrocks_table_only`, `StarRocksTableOnly`, `TABLE_ONLY_TABLES`, `clearance_not_enforced`,
  the `gated_clearances` partition, `ROW_FILTER_GAPS_TABLE_ONLY`, the StarRocks arm and tuple, the D2 sentence and call
  sites; edit the `ExplainRowFilterGap` doc; `ExplainClearanceGap` → internal `ClearanceRef`. Check that
  `effect::tag_reach` counts a table-level table for a StarRocks row filter (record in §14 if it needs a change).
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile`; `cargo test -p egret-perm-compile --test routes --test
  wire_dtos --lib`.
- [ ] Commit `refactor(perm): drop the StarRocks table-level gap from explain and the rule effect`.

## Task 12: OpenAPI and generated client

- [ ] `cargo run -q -p egret-bin --bin egret -- openapi > openapi.json`; expect `StarRocksTableOnly` and
  `ExplainClearanceGap` gone, `ExplainClearanceMaskGap` added. `cd frontend && npm run gen:api`.
- [ ] **Checks:** `cargo test -p egret-server committed_openapi_json_is_current`; the generated-client test; `npx tsc
  -b` (errors expected only where Task 13 removes uses; list them in §14).
- [ ] Commit `chore(api): regenerate for the StarRocks table-level classification`.

## Task 13: Frontend removals and copy (spec §5.8)

**Files:** `RulePreview.tsx`, `RuleDialog.tsx`, `data/AddTagDialog.tsx`, `classifications/tabs/ClearanceTab.tsx`;
`__tests__/RuleDialog.test.tsx`, `data/__tests__/AddTagDialog.test.tsx`, `classifications/__tests__/ClearanceTab.test.tsx`.

- [ ] **Red:** the new `TRINO_ROW_FILTER_HINT`, `TABLE_LEVEL_NOTE` and `COMPILES_TO` texts, and the old StarRocks
  sentences absent (FE-COPY-3); replace "a preview counting table-only tables warns about StarRocks…" with "a row filter
  preview never shows a StarRocks table-only warning, and Create needs no tick for it".
- [ ] **Fix:** delete `StarRocksTableOnlyNotice`, `starrocksTickReason` and their wiring (`starrocksWarnId`,
  `aria-describedby` entry); `needsTick = (secondMask || trinoColumnOnly !== null) && !ticked`.
- [ ] **Checks:** `npm run lint`; `npx tsc -b`; `npx vitest run src/routes/governance`; the governance bundle tests.
- [ ] Commit `feat(governance): StarRocks enforces whole-table classifications`.

## Task 14: Differential on the real jars

May run any time after Task 2; Task 10's explain relies on it.

**Files:** `crates/egret-perm-compile/src/harness_support.rs`, `crates/egret-ranger-eval/tests/differential.rs`.

- [ ] **Red:** `a_whole_table_clearance_deny_decides_as_designed_on_the_real_jars` — `harness_support::
  starrocks_table_clearance_deny` (built by the compiler's own function) plus a grant allowing `select` on
  `{c, d, t, *}`; requests: column `email`, a column-less table request, `_file`, `___count___`; subjects uncleared and
  cleared; on 2.8.0 and 2.9.0 the jars and the evaluator agree, and the decisions are deny/allow as designed.
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile -p egret-ranger-eval`; `cargo test -p egret-ranger-eval`; `just
  test-ranger-gates`.
- [ ] Commit `test(perm): pin the whole-table clearance deny on the real Ranger jars`.

## Task 15: Final gate

- [ ] **Backend** (one command at a time; `pgrep -x cargo` empty first):

```bash
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test --workspace 2>&1 | tee "$SCRATCH"/srtbl-ws.log | tail -40
grep -c "skip: DATABASE_URL" "$SCRATCH"/srtbl-ws.log
grep -E "^test result: FAILED|panicked" "$SCRATCH"/srtbl-ws.log | head
cargo test -p egret-ranger-eval --features crosscheck-prepared
just test-ranger-gates 2>&1 | tee "$SCRATCH"/srtbl-ranger.log | tail -20
cargo run -q -p egret-bin --bin egret -- openapi > openapi.json && git diff --exit-code openapi.json
```

- [ ] **Frontend** (from `frontend/`): `npm run gen:api && git status --short src/api/generated` (no change);
  `npm run lint`; `npx tsc -b`; `npm run test`; `npm run build`; the bundle tests.

Expected: every command exits 0; the skip grep prints `0`.

## Task 16: Live check (own compose project `egret-srtbl`, port 18580; never 18080)

Compose override in the scratchpad (`-p egret-srtbl`, `DEMO_PORT=18580`, confirm free with `lsof -i :18580`), with a
real StarRocks (Ranger plugin 2.8/2.9) and Trino on one egret Iceberg catalog. Results and screenshots go to the
scratchpad. Any mismatch between Explain and the engine is a defect, not a note.

1. Setup: table `sales.orders(id int, email string, region string, tags array<string>)`, 4 rows (2 with
   `region = 'us'`); gated classification `srtbl-pii` (role `pii-cleared`) attached to the **table**; a mask rule
   (Hash) on a second, column-level classification on `email`; a view `sales.v_orders` over the table. Users:
   `granted-u` (grant, uncleared), `cleared-u` (grant + role), `nogrant-u`.
2. StarRocks as `granted-u`: `SELECT id`, `SELECT email`, `SELECT *`, `SELECT count(*)` → refused each (record the
   column StarRocks names); through the view → NULL in every column, or record what it does. As `cleared-u`: real
   values, `email` hashed. Same on Trino (refused; cleared hashed).
3. `ALTER TABLE sales.orders ADD COLUMN phone varchar` on Trino; immediately as `granted-u` on StarRocks `SELECT phone`
   → refused; as `cleared-u` → value; after one plugin poll the download has a mask on `phone` (hook).
4. Row filter `region = 'us'` on a table-level classification with an exempt user: StarRocks 4 → 2 rows for others,
   4 for the exempt user; Trino the same.
5. Mixed attachment (the row-filter classification also on `orders.region`): the download has one
   `perm-rule-{id}-…orders` policy.
6. Unknown table: attach `srtbl-pii` to `sales.ghost` → compile result reports the missing masks; create
   `sales.ghost` → masks appear (hook).
7. Ceiling: a 520-column table, attach at table level → the refusal message names it and 520; adding columns to a
   499-count table → the Iceberg 409 names the table.
8. Explain (Who can read?) for each user and column of steps 2–6: lines match the engine; the unknown-table line
   before step 6's create.
9. UI: the Clearance tab text, the add-tag note, the rule dialog hint; 1440 and 390 px, light and dark; no StarRocks
   table-only warning.

## Task 17: Pull request

- [ ] Push only after the user says so. Title `feat(perm): compile whole-table classifications on StarRocks`. Body:
  spec link, U1–U4, C1–C17, gate output, live results, the release note of spec §9, the Docs box naming the
  follow-up docs PR (C17), non-goals, related defects. Append a dated note to the Package 2 spec §14 pointing to this
  package (append-only). End with the attribution line from the session reminder.

## Self-review

- **Spec coverage:** §5.1 → T1; §5.3 → T2, T4, T6; §5.2 → T3, T8; §5.4 → T5; §5.5 → T7, T8; §5.6 → T9; §5.7 → T10,
  T11, T14; §5.8 → T11, T13; §8 → T12; §10 risks → T16.
- **Ordering:** each task ends green. T4 needs T3; T6 needs T4; T7–T8 need T3–T6; T10 needs T4, T6 and (for its
  table-request claim) T14; T12 after T10–T11; T13 after T12.
- **Open questions:** spec §13 (C11 refusal of an Iceberg commit; the StarRocks fallback if step 2 reads NULLs).
