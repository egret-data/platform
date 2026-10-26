# Classifications follow a table or column rename: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** a table rename or a column rename carries the table's classification rows to the new name before the
Iceberg commit, holds the commit until Trino's and StarRocks' plugins serve the moved classifications, undoes the copy
when the commit fails, and deletes the old rows after it succeeds. Drops keep their rows dormant; a recursive namespace
drop purges its rows; stale rows are reported.

**Architecture:**

- The table-commit hook of #755 (`table_hook.rs`) gains a move branch: a move plan from the rename body (or from the
  schema update's field ids), name try-locks, the hook's profile locks, one copy transaction that is the pre-commit
  compile (it replaces #755's dry run), a wait on the Trino tag version and the StarRocks policy version, the
  forward, then a delete transaction, or a compensation.
- The column source overlay marks the moving source as `leaving`, so the copy's compile counts it once.
- `egret-policy` gains a policy-version wait beside the tag-version wait.
- `ViewReach` moves from `egret-bi` to `egret-perm-compile`, so the move re-tags dependent views like a tag write.
- Two governance events (`tag:move`, `tag:purge`) whose actor is the Iceberg caller (the guard passes the token
  subject to the hook), three Overview findings, and a `state` and `missingColumns` on "Where used" rows.

**Tech stack:** Rust (axum 0.8, sqlx 0.9, utoipa), embedded Lakekeeper, Ranger plugin ledger, React 18 + orval
client, Vitest.

**Spec:** `docs/en/specs/2026-10-26-classification-follows-table-rename-design.md`. Read §2-§5 first. Every
`file:line` there was checked on `28fb8693`; re-check with `/usr/bin/grep -n` before editing (Step 0).

## Global Constraints

- **Worktree:** `egret-tblmove`, branch `fix/classification-follows-table-rename`. Work only there.
- **Build isolation:** `export CARGO_TARGET_DIR=$PWD/target` from the worktree root, never another checkout's target.
  `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`. Keep 30 GiB free;
  `./deploy/prune-target.sh` before the gate.
- **Machine:** zsh; no `rg` (use `/usr/bin/grep`); no `timeout`; quote globs; `rm`/`cp` are aliased (use `/bin/rm -f`,
  `/bin/cp -f`); never `git stash`, `git checkout --` or `git clean`; `pgrep -x`, never `pgrep -f`.
- **SQL:** sqlx 0.9 `query()` takes `&'static str`; no `format!` SQL. `lakekeeper.` is written as a literal and the
  catalog-engine drift test asserts it equals `egret_catalog_engine::LAKEKEEPER_SCHEMA`.
- **DB-gated tests must run, not skip:** `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`, under
  `env -u EGRET_ALLOW_DB_SKIP`; afterwards `grep -c "skip: DATABASE_URL"` prints `0`. Never the database `egret`. One
  suite at a time.
- **Red first, through the real handler.** Every behaviour gets a failing test, seen failing on the old code for the
  stated reason, before the fix. Hook behaviour is driven through the real Iceberg REST router as `table_hook_db.rs`
  does (`mount` with `table_hook::layer`, `observe` in front of Lakekeeper, `oneshot`); control-plane behaviour
  through its real router as `governance_attention.rs` does; compiler behaviour through `recompile_service_in_tx`.
  Engine-level assertions read the served download (`/tags/download/…`, `/plugins/policies/download/…`) or evaluate
  it with `egret-ranger-eval`, never a JSON substring of a stored row. A "red" that is a compile error (a missing
  function) is acceptable only for a new API, and then the behaviour test that uses it must also be seen failing on a
  stub that returns the old behaviour. Mutation-check each new assertion once (break the fix, watch it fail, restore
  from a backup with `cat backup > file`; `git status --porcelain` lists only intended files).
- **Per-task checks** (each task lists its own): `cargo fmt --all -- --check`; `cargo clippy -p <touched crates>
  --all-targets -- -D warnings`; the touched test binaries with the per-process test DB.
- **OpenAPI:** after a schema change, `cargo run -q -p egret-bin --bin egret -- openapi > openapi.json`, then
  `cd frontend && npm run gen:api` (never bare `orval`); commit both.
- **Copy:** classification glossary and `docs/en/frontend-rules.md` (FE-COPY-1..4, FE-VISUAL-4/8, FE-A11Y-4,
  FE-STATE-4, FE-TEST-*).
- **Commits:** one per task, Conventional Commits (`feat(catalog)`, `feat(policy)`, `feat(perm)`,
  `feat(governance)`, `test(catalog)`), trailer exactly `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No
  `Closes #N`, no scratch files, no `/Users/...` paths. Do not push until the user says so.
- **Migrations:** none (spec C19). If one becomes necessary, take the next number free above main AND every open PR
  at merge time (`gh pr list --state open`, each PR's `migrations/`), and record it in spec §14.
- **Stacks:** the demo stack on `127.0.0.1:18080` belongs to another session; never stop, reset, seed or read it.
- **Deviations** go to spec §14 (append-only), per task.

## File structure

| file | change | task |
|---|---|---|
| `crates/egret-policy/src/plugin_downloads.rs` | `plugins_hold_policy_version`, `wait_for_versions` | 1 |
| `crates/egret-policy/tests/plugin_downloads_db.rs` | wait tests through the download routes | 1 |
| `crates/egret-perm-compile/src/table_columns.rs` | `top_level_fields`, `current_fields_via`, `leaving`, `table_states_via`, `missing_columns_via` | 2, 9, 10 |
| `crates/egret-perm-compile/src/{clearance,rules,trigger}.rs` | ceilings skip leaving coordinates | 2 |
| `crates/egret-perm-compile/src/view_tags.rs`, `Cargo.toml` | `ViewReach` (moved, public) | 3 |
| `crates/egret-bi/src/catalog_tags.rs` | uses the moved `ViewReach` | 3 |
| `crates/egret-catalog-engine/src/table_hook.rs` | move plans, hold, copy, wait, finish, compensate, purge | 3-8 |
| `crates/egret-catalog-engine/src/view_hold.rs` | `uncancellable`, `trino_tag_versions`, `delete_inserted_rows` `pub(crate)` | 3 |
| `crates/egret-catalog-engine/tests/common/plugins.rs` (new) | fake Trino tag plugin (moved from `view_hold_db.rs`), fake StarRocks policy plugin | 3, 4 |
| `crates/egret-catalog-engine/tests/table_hook_db.rs` | move tests; Trino profile in `env()` | 3-9 |
| `crates/egret-audit/src/governance.rs` | `tag:move`, `tag:purge`, `Actor`, `record_change_by` | 7 |
| `crates/egret-catalog-engine/src/mount.rs`, `crates/egret-bin/src/main.rs` | `CatalogCaller` from the guard into the request extensions | 7 |
| `crates/egret-controlplane/src/governance_changes.rs` | two sentences, `detail.actor` phrase | 7 |
| `crates/egret-controlplane/src/governance_overview.rs` | three sources | 9, 10 |
| `crates/egret-controlplane/src/governance_tag_usage.rs` | `TableUsage.state`, `TableUsage.missing_columns` | 11 |
| `openapi.json`, `frontend/src/api/generated/**` | regenerated | 11 |
| `frontend/src/routes/governance/overview/AttentionSection.tsx`, `classifications/tabs/WhereUsedTab.tsx` and their `__tests__` | `CHECKS`, badges | 12 |

## Step 0: Re-verify the evidence on the then-current main

- [ ] `git fetch origin && git log --oneline -1 origin/main`. If main moved past `28fb8693`, rebase and record the base
  in spec §14.
- [ ] Re-check each row; record drift in spec §14; stop and report if a row is false (not merely moved).

| spec row | check |
|---|---|
| E1 | `PRIMARY KEY (tag_id, warehouse_id, schema_name, table_name, column_name)` in `migrations/0081_catalog_tag_targets.sql` |
| E7, E8 | `fn classify` in `table_hook.rs`: `"tables", "rename"` arm present; the namespace-drop arm reads no query |
| E9 | `a.column_name = ''` and `cardinality(cp.required_roles) > 0` in `affected_starrocks_profiles_for_tables` (`trigger.rs`) |
| E10 | `/usr/bin/grep -c "tag_version\|bump_bound_tag_versions" crates/egret-catalog-engine/src/table_hook.rs` → `0`; the module doc still says "does not move a table's classifications" |
| E16 | `async fn hold_rename`, `async fn move_view`, `ON CONFLICT (tag_id, warehouse_id, schema_name, table_name, column_name) \` + `DO NOTHING RETURNING tag_id` in `view_hold.rs` |
| E19 | `struct ViewReach` is private to `crates/egret-bi/src/catalog_tags.rs`; `egret-bi` is not in `crates/egret-catalog-engine/Cargo.toml`'s `[dependencies]` |
| E20 | `egret-table-hook\u{1e}` keyspace and `COMMIT_LOCK_WAIT` in `table_hook.rs` |
| E23, E24 | `kind = 'tags'` in `plugins_hold_tag_version`; `DownloadKind::Policies` recorded in `download_policies` (`download.rs`) |
| E27 | `record_change` binds `c.principal.user_id.0`; `None => "egret".to_string()` in `governance_changes.rs` |
| E33 | `EgretCatalogGuard::check` returns `Ok(())` after reading `claims`; `claims.insert("sub".into(), vc.client_id…)` in `egret-machine-idp`'s `mint` |
| E35 | `if !recursive` refusal with `child_tabulars_deleted` in Lakekeeper's `lakekeeper-storage-postgres/src/namespace.rs` (git checkout at the pinned rev) |
| E32 | `ls migrations \| tail -1` (expect `0209`); `gh pr list --state open` and each PR's migrations |
| test | `a_rename_of_a_classified_table_is_dry_run_and_commits` in `table_hook_db.rs` still asserts the classification stays on the old name (Task 3 inverts it) |

## Task 1: A policy-version wait (spec §5.5, C5)

**Files:** `crates/egret-policy/src/plugin_downloads.rs`, `src/lib.rs` (exports); tests
`crates/egret-policy/tests/plugin_downloads_db.rs`.

- [ ] **Red (DB, through the real download routes):** each test drives `ranger_router` with `GET
  /plugins/policies/download/{service}?pluginId=starrocks@sr-fe-0-{service}&lastKnownVersion=N` (as the StarRocks
  plugin does, E24) and `GET /tags/download/…` for Trino.
  - `a_starrocks_plugin_that_reported_the_policy_version_confirms_it` — profile at `policy_version` V; one poll
    reporting V → `plugins_hold_policy_version(V)` confirmed; reporting V-1 → waiting on that plugin id.
  - `a_tag_poll_does_not_confirm_a_policy_version` — only a tags poll with V → not confirmed (kinds are separate).
  - `wait_for_versions_waits_for_both_kinds` — a Trino tags pair and a StarRocks policies pair; the policies plugin
    reports late (a spawned task polls after 600 ms) → `Ok` only after that poll; with it never polling → `Timeout`
    whose `waiting_on` names the StarRocks plugin id.
  - `an_empty_list_is_skipped_but_an_empty_wait_is_still_a_timeout_for_the_tag_wait` — `wait_for_versions(&[],
    &policies)` waits only on policies; `wait_for_tag_version(&[])` keeps returning `Timeout` (the view hold's
    contract).
  Expected on the base: compile error (no function); then, with a stub `plugins_hold_policy_version` that reads
  `kind = 'tags'` (the old query), the first two tests fail.
- [ ] **Fix:** spec §5.5.
- [ ] **Checks:** fmt; clippy `-p egret-policy`; `cargo test -p egret-policy --test plugin_downloads_db --test
  plugin_downloads_failure_db --lib`; `cargo test -p egret-catalog-engine --test view_hold_db` (the tag wait's callers).
- [ ] Commit `feat(policy): wait for a StarRocks policy version like a Trino tag version`.

## Task 2: Field ids, and a moving coordinate counted once (spec §5.4, C8)

**Files:** `crates/egret-perm-compile/src/table_columns.rs`, `clearance.rs`, `rules.rs`, `trigger.rs`; tests
`crates/egret-perm-compile/tests/clearance_starrocks.rs` (stub `lakekeeper`), `src/table_columns.rs` (unit),
`crates/egret-catalog-engine/tests/table_hook_db.rs` (drift).

- [ ] **Red (unit):** `top_level_fields_keeps_ids_in_field_id_order` (and `top_level_columns` equals its names, folded
  duplicates collapsed as today); `an_overlay_without_leaving_still_parses` (a #755-shaped JSON → `leaving` empty).
- [ ] **Red (DB, real compiler):** `a_moving_table_is_counted_once_against_the_ceiling` — stub table `wide` with 300
  columns, a gated tag on the whole table, the same rows copied to `wide_v2`; in one transaction set the overlay
  `[{wide_v2: 300 columns}, {wide: leaving_table}]` and run `recompile_service_in_tx` → accepted (1 + 300 counted);
  the served download has the `*` deny and 300 masks on BOTH names. Today: refused (602 > 500). Also
  `renamed_columns_marked_leaving_are_not_counted` — a 499-count table whose overlay lists one column old and new with
  the old one `leaving` → accepted; without the mark → refused.
- [ ] **Red (drift, real Lakekeeper):** extend `the_column_source_reads_what_lakekeeper_loads` — after each step
  `current_fields_via` returns the `(id, name)` pairs of `load_table_detail` and, after renaming `region`→`area`, the
  same id under the new name.
- [ ] **Fix:** spec §5.4; the four ceiling sites skip a policy whose folded table is `leaving_table` or whose column is
  in `leaving` (one helper, `counts_toward_ceiling`).
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile -p egret-catalog-engine`; `cargo test -p egret-perm-compile
  --test clearance_starrocks --test compile_rules --lib`; `cargo test -p egret-catalog-engine --test table_hook_db`.
- [ ] Commit `feat(perm): read field ids and count a moving table once`.

## Task 3: A table rename moves its classifications (R1, spec §5.1-§5.3, C7, C11, C18)

**Files:** `crates/egret-perm-compile/src/view_tags.rs`, `Cargo.toml`; `crates/egret-bi/src/catalog_tags.rs`;
`crates/egret-catalog-engine/src/table_hook.rs`, `view_hold.rs`; tests `table_hook_db.rs`, `tests/common/plugins.rs`.

- [ ] **Refactor first (no behaviour change):** move `ViewReach` to `egret_perm_compile::view_tags::ViewReach`
  (public; `egret-perm-compile` gains `egret-catalog-registry`, which does not depend on it). Its errors become
  `CompileError`; `egret-bi` maps them as before. Guard: `cargo test -p egret-bi` (the catalog-tag and view-tag
  suites) passes before and after, unchanged.
- [ ] **Test harness:** move the fake Trino plugin (`start_plugin`, `Plugin`) from `view_hold_db.rs` to
  `tests/common/plugins.rs`; `env()` in `table_hook_db.rs` also creates a Trino engine and profile bound to the
  catalog (the StarRocks one stays). The hook in tests uses `with_limit(Duration::from_secs(5))` and a 200 ms poll.
- [ ] **Red (DB, real Iceberg REST router):**
  - invert `a_rename_of_a_classified_table_is_dry_run_and_commits` into
    `a_renamed_table_keeps_its_classifications`: `orders(id, email)` with tag A on the table and tag B on `email`;
    Trino plugin running; rename to `orders_v2` → 2xx; rows `(A, orders_v2, '')` and `(B, orders_v2, email)`, none on
    `orders`; the served Trino tags name `orders_v2` and not `orders`; the StarRocks download carries the `*` deny and
    the masks on `orders_v2`. Today: rows stay on `orders` (red).
  - `the_rename_reaches_lakekeeper_only_after_trino_serves_the_moved_tags` — with `observe`'s `on_reach` probe on
    `POST /…/tables/rename`: at the moment the rename reaches Lakekeeper, the rows exist on both names and the Trino
    plugin has already reported the tag version the copy left (`report_of(v)` earlier than the reach). Today: no copy.
  - `a_rename_whose_move_is_refused_answers_409_and_does_not_rename` — a stored differing priority-1 mask tie that the
    destination name completes (a tag-addressed mask on tag A and a path mask on `sales.orders_v2.email`) → 409 whose
    message starts "egret refused the table change: the table's classifications cannot move to"; Lakekeeper still
    loads `orders`, not `orders_v2`; rows and every profile's `tag_version`/`policy_version` unchanged. Today: 2xx.
  - `a_failed_rename_undoes_the_copy` — a live table already named `orders_v2` → Lakekeeper's 409 is returned as it
    is; no row on `orders_v2` remains; `tag_version` bumped twice (copy, compensation); the rows on `orders` intact.
  - guard `a_rename_moves_classifications_only` (R4) — a path deny grant on `sales.orders` stays on `sales.orders`
    after the rename (spec §11 item 1, recorded, not fixed).
- [ ] **Fix:** spec §5.1-§5.3 for the table-level plan with the Trino wait (`wait_for_versions` with the tag pairs
  only; the StarRocks pairs come in Task 4): cap, name try-locks, profile locks over every bound profile, uncancellable
  work, copy transaction with `ViewReach::of(dst)`, overlay, retag/bump/recompile, overlap refusal, required versions;
  wait; forward; compensate; `finish` with `ViewReach::of(src)`. `TableHook` gains `policy`, `limit` and the
  semaphore; `TableHook::new(db)` keeps its signature. Without a plan the #755 path runs unchanged.
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile -p egret-bi -p egret-catalog-engine`; `cargo test -p
  egret-catalog-engine --test table_hook_db --test view_hold_db --lib`; `cargo test -p egret-bi`.
- [ ] Commit `fix(catalog): move a table's classifications with it on rename`.

## Task 4: Wait for StarRocks too; no Trino, silent engines, the deadline (C4, C5, C16)

**Files:** `table_hook.rs`; tests `table_hook_db.rs`, `tests/common/plugins.rs` (fake StarRocks policy plugin: polls
`/plugins/policies/download/{profile}` every 200 ms, reporting the `policyVersion` of its last 200).

- [ ] **Red:**
  - `the_rename_waits_for_the_starrocks_policy_version` — both fake plugins running; at the rename's reach the
    StarRocks plugin has reported the `policy_version` the copy left, and its download then holds masks on
    `orders_v2`. Today (Task 3 code): the rename reaches Lakekeeper before the StarRocks report (red).
  - guard `a_rename_with_no_trino_bound_moves_without_a_trino_wait` — Trino engine detached, StarRocks plugin
    running → 2xx, rows moved, no Trino wait (no `NO_PLUGIN_POLLED` refusal), the reach after the StarRocks report.
    Mutation: make an empty Trino list a timeout (the view hold's rule) → red.
  - `a_rename_no_plugin_confirms_is_refused_and_undone` (R5) — Trino bound, no plugin running, `with_limit(2 s)` → 409
    "engines did not confirm the moved classifications within 2 s: <no plugin has polled>"; no row on `orders_v2`;
    Lakekeeper still has `orders`.
  - `a_rename_with_nothing_bound_moves_at_once` — no engine bound at all → 2xx within 1 s, rows moved.
- [ ] **Fix:** required StarRocks pairs read in the copy transaction; empty Trino or StarRocks lists skipped; the
  refusal message of spec §5.3 step 6.
- [ ] **Checks:** as Task 3.
- [ ] Commit `feat(catalog): hold a classified rename until StarRocks serves it too`.

## Task 5: The rename defaults (C1-C3, C9, C10, C17, #755 interplay)

**Files:** `table_hook.rs`; tests `table_hook_db.rs`.

- [ ] **Red** (each through the router; the first four fail on the Task 4 code, the rest are guards that must stay
  green and are mutation-checked):
  - `a_rename_onto_a_name_with_rows_unions_them` — dormant `(C, orders_v2, '')` exists → after the rename
    `orders_v2` carries A, B and C; compensation of a failed variant leaves C in place.
  - `a_classified_rename_into_a_multi_level_namespace_is_refused` — destination namespace `["sales", "eu"]` → 409
    "… more than one level …"; nothing written.
  - `a_cross_namespace_rename_moves_the_rows` — `sales.orders` → `archive.orders` (both single-level) → rows under
    `archive`.
  - `a_rename_into_a_name_spelled_differently_writes_the_destination_spelling` — rows stored as `Sales.Orders`
    (folded match) → copies under the request's `sales.orders_v2`.
  - guard `a_case_only_rename_is_not_held` — `orders` → `Orders`: no lock taken (a concurrent name lock does not
    refuse it), no row written, no version bumped.
  - guard `an_unclassified_rename_is_not_held` — no row on the source: no copy, `tag_version` unchanged.
  - guard `a_view_sent_to_the_table_rename_endpoint_is_not_moved` — a classified view `v` renamed through
    `POST …/tables/rename` → no row copied; Lakekeeper's answer returned.
  - guard `a_rename_onto_a_classified_name_still_dry_runs` — the existing `small` → `target` ceiling case of
    `a_rename_of_a_classified_table_is_dry_run_and_commits` (moved here): source carries nothing, #755's dry run
    refuses with 409.
  - `a_held_name_refuses_a_second_rename_at_once` — a name lock held by another connection on `orders_v2` → 409
    "being held" within 1 s.
  - `a_held_rename_and_a_column_add_on_the_same_profile_run_one_after_the_other` — a column add on another
    whole-table-classified table of the warehouse, sent while the rename waits, completes after the rename's
    `finish` (profile locks), and its dry run sees the moved state.
- [ ] **Fix:** spec C1-C3, C9, C10, C17.
- [ ] **Checks:** as Task 3.
- [ ] Commit `feat(catalog): union, refuse multi-level and skip views and case-only renames`.

## Task 6: A column rename follows by field id (R3, spec §5.2 column plan, C12)

**Files:** `table_hook.rs`; tests `table_hook_db.rs`.

- [ ] **Red** (Trino's `ALTER TABLE … RENAME COLUMN` body: `add-schema` with the same field ids and
  `set-current-schema -1`):
  - `a_renamed_column_keeps_its_classification` — tag B on `orders.email` (field 2); rename `email`→`mail` → rows on
    `mail`, none on `email`; Trino serves `mail`; the commit reached Lakekeeper after the Trino plugin reported the
    copy's version. Today: the row stays on `email` (red).
  - `a_column_rename_on_a_whole_table_classification_waits_for_starrocks` — tag A on the whole table only, StarRocks
    bound; rename `email`→`mail` → at the reach the StarRocks download already masks `mail` (and still `email`); after
    `finish` only `mail`. Today: `mail` masked only after the commit (red).
  - `swapped_column_names_swap_their_classifications` — B on `a`, C on `b`; one commit renames `a`→`b` and `b`→`a`
    (ids kept) → B on `b`, C on `a`.
  - `a_multi_table_commit_moves_each_tables_renamed_columns` — `transactions/commit` renaming a classified column
    in two tables.
  - `a_dropped_column_keeps_its_rows_dormant` — drop `email` (id gone) → the row stays on `email`; no hold.
  - guard `an_append_is_not_held` — `add-snapshot` only: no name lock held while the commit reaches Lakekeeper
    (probe with `pg_try_advisory_lock` from `observe`), no row written, `tag_version` and `policy_version` unchanged
    (#755's `a_commit_on_an_unclassified_table_runs_no_compile`, extended).
  - guard `a_column_add_without_a_rename_takes_the_755_path` — dry run as before, no copy.
- [ ] **Fix:** spec §5.2 column plan and the set formulation (`Before`, `Target`); overlay `columns = old ∪ new`,
  `leaving = old \ new`.
- [ ] **Checks:** as Task 3.
- [ ] Commit `fix(catalog): move a column's classifications with it on rename`.

## Task 7: Governance events with the real caller (C6, C6a, R7, spec §5.7)

**Files:** `crates/egret-catalog-engine/src/mount.rs` (`CatalogCaller`, `CatalogGuard::check` return type,
`guard_then_inject_metadata`); `crates/egret-bin/src/main.rs` (`EgretCatalogGuard::check` returns the verified
`sub`/`aud`); `crates/egret-audit/src/governance.rs`; `table_hook.rs`; `crates/egret-controlplane/src/governance_changes.rs`;
tests `table_hook_db.rs`, `crates/egret-bin` guard unit tests (beside the existing `wrong audience` ones,
`main.rs:3150-3240`), the controlplane feed test binary.

- [ ] **Red:**
  - guard unit test `the_catalog_guard_returns_the_token_subject` — a token signed for `aud = "catalog"`, `sub =
    "cat1"` → `check` returns `CatalogCaller { subject: "cat1", audience: "catalog" }`. Expected on the base: compile
    error (the trait returns `()`); then, with the trait changed but the guard returning an empty subject, red.
  - `a_completed_move_records_one_tag_move_event_by_its_caller` (table_hook_db) — the test mounts a guard stub that
    returns `CatalogCaller { subject: <client_id of a catalog.catalog_clients row "trino-main"> }`; after Task 3's
    rename: one `audit.audit_log` row `action = 'tag:move'`, `resource_type = 'catalog'`, `actor_user_id IS NULL`,
    `detail.actor = {kind: catalog_client, id, client_id, name: "trino-main", consumer_type: "engine"}`, and
    `before`/`after`/`tags`/`rows`; after Task 5's union case `kept` lists C. Today: no event (red).
  - `a_move_by_a_deleted_client_keeps_its_client_id` — the client row deleted before the rename → `detail.actor.id`
    and `name` null, `client_id` kept.
  - `a_move_with_no_guard_is_by_egret` — no guard mounted → `detail.actor` absent, `actor_user_id` NULL.
  - `a_refused_or_undone_move_records_nothing` — the 409 and the failed-rename cases leave no `tag:move` row.
  - feed route test: `GET /api/control-plane/governance/changes` for a `tag:read` reader returns "Catalog client
    *trino-main* moved classification *A* from `sales.orders` to `sales.orders_v2` (table renamed)" (and the column
    variant); a deleted client reads "Catalog client `<client_id>` (deleted)"; a reader with only `policy:read` does
    not see it (family Tag).
- [ ] **Fix:** spec §5.7: `CatalogCaller` read by the guard from its verified claims and put in the extensions by
  `guard_then_inject_metadata`; the hook carries it into `finish` and resolves it there; `Actor`,
  `record_change_by` (`record_change` delegates to it, its callers unchanged); the feed's actor phrase reads
  `detail.actor`.
- [ ] **Checks:** fmt; clippy `-p egret-audit -p egret-catalog-engine -p egret-bin -p egret-controlplane`; `cargo test
  -p egret-audit`; `cargo test -p egret-bin` (guard tests); `cargo test -p egret-catalog-engine --test table_hook_db
  --test view_hold_db --test warehouse_admin_routes` (every test guard compiles against the new trait); the
  controlplane feed test binary.
- [ ] Commit `feat(governance): record classification moves by the catalog client that made them`.

## Task 8: Drops (R2, C13, spec §5.6)

**Files:** `table_hook.rs` (`classify` reads `recursive`); tests `table_hook_db.rs`.

- [ ] **Red:**
  - `a_recursive_namespace_drop_deletes_its_classifications` — memory (hard-delete) warehouse; rows on two tables and
    a column in `sales`, a classified view in `sales` with lineage, a view in `other` reading `sales.orders`; `DELETE
    …/namespaces/sales?recursive=true` → 2xx; no row with schema `sales` in any spelling; the `sales` view's lineage
    gone; the `other` view re-tagged (its derived rows gone); one `tag:purge` event whose `detail.actor` is the
    guard stub's catalog client; `tag_version` bumped. Today: rows stay (red).
  - guard `a_plain_namespace_drop_keeps_rows` — an empty namespace with a pre-placed row, non-recursive drop → the
    row stays.
  - guard `a_plain_drop_of_a_namespace_with_a_dropped_table_is_refused` (R9, E35) — soft-delete warehouse, drop the
    namespace's only table, then a plain namespace drop → Lakekeeper's 409 `NamespaceNotEmpty` passes through; the
    dropped table's rows stay. Pins the premise that makes R9's purge unnecessary; if it ever passes, R9 must be
    built.
  - guard `a_dropped_table_keeps_its_rows_and_an_undrop_restores_them` — soft-delete warehouse; drop `orders` → rows
    stay; `TableHook::undrop` by its `tabular_id` → Trino serves the tags on `orders`, StarRocks masks back.
- [ ] **Fix:** spec §5.6.
- [ ] **Checks:** as Task 3.
- [ ] Commit `fix(catalog): purge classifications with a recursively dropped namespace`.

## Task 9: Stale-row report (R2, C14, C15, spec §5.8)

**Files:** `table_columns.rs` (`table_states_via`, `TableState`); `crates/egret-controlplane/src/governance_overview.rs`;
tests `crates/egret-catalog-engine/tests/table_hook_db.rs` (drift), `crates/egret-controlplane/tests/governance_attention.rs`
(stub `lakekeeper`, as perm-compile's `tests/support/lakekeeper_stub.rs`; share it through `egret-test-db` or copy
it with its doc naming the drift test as its guard).

- [ ] **Red (drift, real Lakekeeper):** `table_states_follow_lakekeeper` — live table → `Live`; a view → `Live`;
  after a soft drop → `Dropped`; after a rename, the old name → `Missing`; a name never created → `Missing`; a
  multi-level namespace → `Missing`. Expected on the base: compile error; with a stub returning `Live`, red.
- [ ] **Red (route, real handler):** `classifications_on_missing_and_dropped_tables_are_findings` — rows on `ghost`
  (no tabular), on `gone` (soft-deleted only) and on `orders` (live) → `GET /governance/attention` for a `tag:read`
  reader has `classifications_on_missing_tables` (warning, count 1, sample `lake.sales.ghost` with the classification
  name, `ref` the tag) and `classifications_on_dropped_tables` (info, count 1); `orders` appears in neither; a reader
  without `tag:read` gets both in `notChecked`. Today: neither kind (red).
- [ ] **Fix:** spec §5.8 SQL, pinned by the drift test (record differences in §14); the two sources and sentences
  (FE-COPY-4: the server writes singular and plural).
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile -p egret-controlplane -p egret-catalog-engine`; `cargo test -p
  egret-controlplane --test governance_attention`; `cargo test -p egret-catalog-engine --test table_hook_db`.
- [ ] Commit `feat(governance): report classifications on missing and dropped tables`.

## Task 10: Columns that are not in their table (R8, spec §5.8 missing columns)

**Files:** `table_columns.rs` (`missing_columns_via`, `MissingColumn`); `governance_overview.rs`; tests
`table_hook_db.rs` (drift), `governance_attention.rs` (stub `lakekeeper` with several `table_schema` rows).

- [ ] **Red (drift, real Lakekeeper):** `missing_columns_follow_the_schema_history` — `orders(id 1, email 2,
  region 3)`; through the REST router rename `email`→`mail` and drop `region`; then write rows on `email`, `region`,
  `ghost` and `ID` directly (rows as a pre-fix rename left them) → `email` missing with `renamed_to = "mail"`;
  `region` missing with `renamed_to = None` (id 3 gone); `ghost` missing, `None`; `ID` not missing (case only). Then
  add a new `region` (id 4) and rename it to `area` → `region` now gets `renamed_to = "area"`: the rule (spec §5.8
  step 3) takes the latest field that carried the name, which is a suggestion a steward confirms, never applied;
  the test pins that behaviour. Expected on the base: compile error; with a stub returning nothing, red.
- [ ] **Red (route, real handler):** `columns_not_in_their_table_are_a_finding` — `GET /governance/attention` for a
  `tag:read` reader has `classifications_on_missing_columns` (warning) whose samples read
  `lake.sales.orders.email` "renamed to `mail`" and `lake.sales.orders.ghost` "not in the table"; a column row on a
  dropped or missing table is not counted here (those tables are Task 9's findings). Today: no such kind (red).
- [ ] **Fix:** spec §5.8 missing columns; one query over the live tables' column rows; the sentence with
  singular/plural and the renamed/not-renamed split (FE-COPY-4).
- [ ] **Checks:** fmt; clippy `-p egret-perm-compile -p egret-controlplane`; `cargo test -p egret-controlplane --test
  governance_attention`; `cargo test -p egret-catalog-engine --test table_hook_db`.
- [ ] Commit `feat(governance): report classified columns that are not in their table`.

## Task 11: "Where used" carries each table's state and missing columns; OpenAPI (C15, R8, spec §8)

**Files:** `crates/egret-controlplane/src/governance_tag_usage.rs`; `openapi.json`; `frontend/src/api/generated/**`;
tests `crates/egret-controlplane/tests/governance_tag_usage.rs`.

- [ ] **Red (route):** `where_used_says_which_tables_are_missing_or_dropped` — the Task 9 fixture → `GET
  /classifications/{id}/usage` rows carry `state` `missing`, `dropped`, `live`; a row whose warehouse has no
  Lakekeeper warehouse carries no `state` key. `where_used_lists_missing_columns_with_their_new_name` — the Task 10
  fixture → the live `orders` row carries `missingColumns: [{column: "email", renamedTo: "mail"}, {column: "ghost",
  renamedTo: null}]`; a table with none carries no key. Today: neither (red).
- [ ] **Fix:** one `table_states_via` and one `missing_columns_via` call per request over the tag's tables;
  `TableState` and `MissingColumn` registered in the OpenAPI components; regenerate `openapi.json` and the client
  (`npm run gen:api`).
- [ ] **Checks:** fmt; clippy `-p egret-controlplane`; `cargo test -p egret-controlplane --test governance_tag_usage`;
  `cargo test -p egret-server committed_openapi_json_is_current generated_client_was_emitted_from_the_committed_spec`.
- [ ] Commit `feat(governance): say which classified tables are missing or dropped`.

## Task 12: Frontend (spec §5.8)

**Files:** `frontend/src/routes/governance/overview/AttentionSection.tsx`,
`frontend/src/routes/governance/classifications/tabs/WhereUsedTab.tsx`; their `__tests__`.

- [ ] **Red (Vitest):** `AttentionSection.test.tsx` — the "Not checked for you" lines for the three kinds read
  "classifications on tables that do not exist", "classifications on dropped tables" and "classifications on columns
  that do not exist" (today: the raw kind); `WhereUsedTab.test.tsx` — a `missing` row shows a status badge with the
  text "Not in the catalog", a `dropped` row "Dropped", a `live` row and a row without `state` none; a missing column
  shows "Renamed to mail" or "Not in the table" beside its name (FE-COPY-3, FE-A11Y-4).
- [ ] **Fix:** three `CHECKS` entries; `StatusBadge` beside the table name and beside each missing column
  (FE-VISUAL-8), tone from tokens (FE-VISUAL-1), no layout change otherwise.
- [ ] **Checks:** `npm run lint`; `npx tsc -b`; `npx vitest run src/routes/governance`; the governance bundle tests.
- [ ] Commit `feat(governance): show stale classified tables in Where used and the Overview`.

## Task 13: Final gate

- [ ] **Backend** (one command at a time; `pgrep -x cargo` empty first):

```bash
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test --workspace 2>&1 | tee "$SCRATCH"/tblmove-ws.log | tail -40
grep -c "skip: DATABASE_URL" "$SCRATCH"/tblmove-ws.log
grep -E "^test result: FAILED|panicked" "$SCRATCH"/tblmove-ws.log | head
just test-ranger-gates 2>&1 | tee "$SCRATCH"/tblmove-ranger.log | tail -20
cargo run -q -p egret-bin --bin egret -- openapi > openapi.json && git diff --exit-code openapi.json
```

- [ ] **Frontend** (from `frontend/`): `npm run gen:api && git status --short src/api/generated` (no change);
  `npm run lint`; `npx tsc -b`; `npm run test`; `npm run build`; the bundle tests.

Expected: every command exits 0; the skip grep prints `0`. Read the full log, not the tail, before calling it green.

## Task 14: Live check (own compose project `egret-tblmove`, port 18680; never 18080)

Compose override in the scratchpad (`-p egret-tblmove`, `DEMO_PORT=18680`; confirm free with `lsof -i :18680`), with
a real Trino (Ranger plugin 2.5.0) and a real StarRocks (Ranger plugin 2.8/2.9) on one egret Iceberg catalog, the
warehouse on a **soft-delete** profile (for undrop). Results, timings and screenshots go to the scratchpad. Any
mismatch between egret's state and what an engine returns is a defect, not a note.

1. **Setup:** `sales.orders(id int, email varchar, region varchar)`, 4 rows; classification `tm-pii` (clearance role
   `pii-cleared`) on the **table**; classification `tm-email` with a Hash mask rule on the **column** `email`; a view
   `sales.v_orders` over the table. Users: `granted-u` (select grant, uncleared), `cleared-u` (grant + role).
   Baseline on both engines: `granted-u` refused/NULL as #755's live check recorded; `cleared-u` reads `email`
   hashed.
2. **Table rename** from Trino: `ALTER TABLE sales.orders RENAME TO orders_v2`. Record the statement's duration
   (expect one to two polls). In a second session, loop every 2 s through the whole window on BOTH engines as
   `granted-u`: `SELECT * FROM sales.orders_v2` and through a view re-created over `orders_v2` once it exists. Every
   read refused or NULL, never a real value; `cleared-u` sees `email` hashed from the first successful read. Then the
   ledger (`policy.ranger_plugin_downloads`) shows both plugins reported the copy's versions before the rename's
   commit time (egret log), and no row remains on `orders`.
3. **Column rename** from Trino: `ALTER TABLE sales.orders_v2 RENAME COLUMN email TO mail`. Same window loop: on
   Trino `mail` hashed for `cleared-u` from the first read, on StarRocks `granted-u` refused, through the view NULL.
   The row is on `mail`.
4. **Refusal 409:** create the tie of Task 3's refusal case (a path mask on `sales.orders_v3.mail` differing from
   `tm-email`'s Hash) and rename `orders_v2` to `orders_v3` from Trino → Trino prints the 409 message; `SHOW TABLES`
   still lists `orders_v2`; reads unchanged.
5. **Drop and undrop:** `DROP TABLE sales.orders_v2` → rows stay (Where used shows "Dropped"; the Overview shows
   `classifications_on_dropped_tables`); undrop from the recovery page → on both engines the restored table is
   classified again on its first read (refused/NULL for `granted-u`, hashed for `cleared-u`).
6. **Stale report:** pre-place `tm-pii` on `sales.ghost` → `classifications_on_missing_tables` names it; Where used
   shows "Not in the catalog". Then rename `region`→`area` from Trino (unclassified, so not held) and insert a row on
   `sales.orders_v2.region` directly in SQL, as a pre-fix rename left rows → `classifications_on_missing_columns`
   reads "renamed to `area`"; Where used shows it beside the column.
7. **Silent engine:** stop the StarRocks FE, wait 100 s, rename a classified table → 409 after 90 s naming the
   StarRocks plugin (or `<no plugin has polled>`); restart; the rename then succeeds. Record the measured behaviour for
   spec §13 Q1.
8. **Recursive namespace drop** (a throwaway namespace `tm_drop` with a classified table, `force=true`): rows gone,
   one `tag:purge` event in Governance > Changes naming the catalog client that sent it. Step 2's `tag:move` event
   names Trino's catalog client.
9. **UI:** the three Overview findings, the Where used badges (tables and columns), the Changes feed sentences for `tag:move` and `tag:purge`;
   1440 and 390 px, light and dark.

## Task 15: Pull request

- [ ] Push only after the user says so. Title `fix(catalog): classifications follow a table or column rename`. Body:
  spec link, R1-R9, C1-C20 and C6a (marked as defaults the user may revisit), gate output, live results (with the window loop
  timings), the release note of spec §9, the Docs box naming the follow-up docs PR and its KI rows (spec C20, §11),
  non-goals, related defects, open questions. Append a dated note to the StarRocks table-level classification spec §14
  saying §11 item 1 is fixed by this package (append-only). End with the attribution line from the session reminder.

## Self-review

- **Spec coverage:** §5.5 → T1; §5.4 → T2, T9, T10; §5.1-§5.3 → T3, T4, T5; §5.2 column plan → T6; §5.7 → T7; §5.6 →
  T8; §5.8 → T9, T10, T11, T12; §8 → T11; §10 risks → T14 (client hold, silent engine, StarRocks ledger).
- **Ordering:** each task ends green. T3 needs T1 (the wait) and T2 (the overlay); T4-T6 need T3; T7 needs T3 (and
  T5's union fixture); T8 needs T7 (`tag:purge`, the caller); T10 needs T9; T11 needs T10; T12 needs T11; T13 after
  T12; T14 after T13.
- **User decisions:** R1 → T3, T4; R2 → T8, T9; R3 → T6; R4 → nothing moved but classification rows (T3's guard);
  R5 → T4 (`a_rename_no_plugin_confirms_is_refused_and_undone`), T14 step 7; R6 → nothing built for column adds,
  T1's wait is generic; R7 → T7; R8 → T10, T11, T12; R9 → T8's guard pins that it cannot happen.
- **Open questions:** none (spec §13).
