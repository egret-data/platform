# Verify a mask per user (Classification UX, Package 2): implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task by task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Who can read? answers, per person and engine, in one sentence that names the rule, the exemption's
principal and the clearance: masks on Trino and StarRocks, row filters, ties, and the two known engine gaps.

**Architecture:**

- The evaluator reports each tied policy's outcome (one field on `EvalOrderAmbiguous`).
- `explain_users` asks the mask on every engine, adds a table-level row-filter question, replays StarRocks clearance
  masks, rebuilds exemption reasons from the model, and finds the D2/D3 gaps, on one read-only snapshot with a
  per-request cache.
- A pure builder turns those facts into `ExplainEffectLine`s (structured parts + text).
- The frontend renders the lines first; the policy detail folds. `RuleEffect` gains the D3 count for the rule dialog.

**Tech stack:** Rust (axum 0.8, sqlx 0.9, utoipa), `egret-ranger-eval`, React 18 + TanStack Query + orval client,
Vitest, Playwright.

**Spec:** `docs/en/specs/2026-10-24-verify-mask-per-user-design.md`. Read §2–§5 first. Every `file:line` there was
checked on `af793f4a`; re-check with `grep -n` before editing (Step 0), because line numbers drift.

## Global Constraints

- **Worktree:** `egret-vmask`, branch `feat/verify-mask-per-user`. Work only there.
- **Build isolation:** `export CARGO_TARGET_DIR=$PWD/target` from the worktree root, never another checkout's
  target. Build with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`. Keep 30 GiB free;
  run `./deploy/prune-target.sh` before the gate.
- **Machine:** no `rg` (use `/usr/bin/grep`); zsh: quote globs (`--include='*.rs'`); never `git stash`,
  `git checkout --` or `git clean`; `pgrep -x`, never `pgrep -f`.
- **SQL:** sqlx 0.9 `query()` takes `&'static str`; no `format!` SQL.
- **DB-gated tests must run, not skip:** `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`, under
  `env -u EGRET_ALLOW_DB_SKIP`; afterwards `grep -c "skip: DATABASE_URL"` prints `0`. Never the database `egret`.
  One suite at a time.
- **Red first, through the real handler.** Every backend behaviour gets a failing route test (`tower::oneshot` on
  the router, as `crates/egret-perm-compile/tests/routes.rs` does) before the fix, failing for the stated reason.
  Pure builders get table-driven unit tests. Mutation-check each new assertion once (break the fix, watch it fail,
  restore with `cat backup > file`, `git status --porcelain` lists only intended files).
- **Engine-level truth comes from `egret-ranger-eval`** over the served artifact; a JSON substring check on a
  policy body is not an engine-level assertion.
- **OpenAPI:** after any schema change, `cargo run -q -p egret-bin --bin egret -- openapi > openapi.json`, then
  `cd frontend && npm run gen:api` (never bare `orval`); commit both; `committed_openapi_json_is_current` passes.
- **Copy:** every UI string uses the parent glossary (`docs/en/specs/2026-10-21-classification-ux-design.md` §4)
  and `docs/en/frontend-rules.md` (FE-COPY-1..4, FE-ACTION-4, FE-VISUAL-1/4, FE-DATA-1, FE-TEST-*).
- **Commits:** Conventional Commits (`feat(perm)`, `feat(governance)`, `test(perm)`…), trailer exactly
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No `Closes #N` in commits, no scratch files, no
  `/Users/...` paths in committed files. Do not push until the user says so.
- **Stacks:** the demo stack on `127.0.0.1:18080` belongs to another session; never stop, reset or seed it. The live
  check uses its own compose project `egret-vmask` on port **18480** (confirm free with `lsof -i :18480`).
- **Merge risk:** open PR #741 edits `crates/egret-perm-compile/src/routes.rs`, `members.rs`, `trigger.rs`. If it
  merges first, rebase before Task 13. No migration in this package.
- **Docs rule (`CLAUDE.md`):** the Platform docs update is a separate docs PR after merge (spec C13). The PR's Docs
  box names that follow-up instead of being ticked.

## File structure

| file | change | task |
|---|---|---|
| `crates/egret-ranger-eval/src/eval.rs` | `TiedOutcome`; `EvalOrderAmbiguous.outcomes` | 1 |
| `crates/egret-ranger-eval/tests/{mask_filter,tag_arbitration,differential}.rs` | red tests; `..` in patterns | 1 |
| `crates/egret-perm-compile/src/explain_wire.rs` | wire `outcomes`; `WireRowFilterDecision`, `WireRowFilterResult` | 1, 4 |
| `crates/egret-perm-compile/src/routes.rs` | snapshot, cache, `Rule.name`, masks on Trino, row filter, exemption, replay, gaps, lines | 2–7 |
| `crates/egret-perm-compile/src/routes/people.rs` | snapshot transaction in `explain_principal` | 2 |
| `crates/egret-perm-compile/src/explain_effect.rs` (new) | pure line builder | 7 |
| `crates/egret-perm-compile/src/effect.rs` | `RuleEffect.trino_column_only` | 9 |
| `crates/egret-perm-compile/tests/routes.rs` | red route tests; helper `trino_tag_profile` | 3–7, 9 |
| `crates/egret-perm-compile/tests/explain_principal.rs` | group fan-out, exemption via group, timing | 5, 8 |
| `crates/egret-perm-compile/tests/clearance_starrocks.rs` | replay (D4), table-level gap (D2) | 6 |
| `openapi.json`, `frontend/src/api/generated/**` | regenerated | 10 |
| `frontend/src/components/perm/ExplainResult.tsx` (+ new `EffectLine.tsx`) | lines first, details fold, limits copy, `DecidedBy` name | 11 |
| `frontend/src/components/perm/MembersExplainView.tsx` | member lines | 11 |
| `frontend/src/routes/governance/data/AddTagDialog.tsx`, `classifications/tabs/ClearanceTab.tsx` | D2 copy | 12 |
| `frontend/src/routes/governance/{RuleDialog,RulePreview}.tsx` | D3 hint, warning, tick | 12 |
| tests under `frontend/src/components/perm/__tests__/`, `routes/governance/**/__tests__/` | vitest | 11, 12 |

## Step 0: Re-verify the evidence on the then-current main

- [ ] `git fetch origin && git log --oneline -1 origin/main`. If main moved past `af793f4a`, rebase this branch and
  record the new base in spec §14.
- [ ] Re-check each row below with `grep -n`; record any drift in spec §14 before Task 1. Stop and report if a row is
  false (not merely moved).

| spec row | check |
|---|---|
| E3 | `grep -n 'engine == EngineType::StarRocks' crates/egret-perm-compile/src/routes.rs` still guards the mask block; `resolved_tags: None` beside "StarRocks has no tag service" |
| E4 | `grep -n 'evaluate_tag_sub' crates/egret-ranger-eval/src/eval.rs` inside `evaluate_sub` for mask/row filter |
| E5 | `grep -n 'RowFilter' crates/egret-perm-compile/src/explain_wire.rs` → nothing |
| E9 | `grep -n 'MASK_NONE\|filter_expr: String::new()' crates/egret-perm-compile/src/rules.rs` → only in `mask_items`/`rowfilter_items` |
| E11 | the three `EvalOrderAmbiguous {` construction sites in `eval.rs` |
| E12 | `clearance_mask_name` format `…-mask-…` in `clearance_mask.rs` |
| E13 | `a.column_name <> ''` in `tagged_columns_for_service_via` (`crates/egret-policy/src/store.rs`). If a fix landed, drop the D2 explain gap and copy from this plan and say so |
| E14 | Trino tag rule compiles one `tag-rowfilter` (`rules.rs`); no compiler change since |
| E15 | the two strings in `AddTagDialog.tsx` and `ClearanceTab.tsx` |
| E19/C8 | the explain path writes nothing: `grep -n 'INSERT\|UPDATE\|DELETE\|ensure_username' ` over `served_artifact_via`, `envelope_via`, `crates/egret-policy/src/lineage_transform.rs` `lineage_catalogs_for_poll_via`, `resolved_tags_for_target`. A write there → drop `READ ONLY` (keep `REPEATABLE READ`) and record it |
| E26 | `gh pr view 741 --json state`; `ls migrations | tail -1` |

## Task 1: Tied outcomes from the evaluator (spec §5.6, C4)

**Files:** `crates/egret-ranger-eval/src/eval.rs`; tests `crates/egret-ranger-eval/tests/mask_filter.rs`,
`tag_arbitration.rs`; pattern updates in `tests/differential.rs`, `src/tag_eval_tests.rs`,
`crates/egret-perm-compile/src/explain_wire.rs`, `crates/egret-perm-compile/tests/wire_dtos.rs`.

- [ ] **Red.** In `mask_filter.rs`: `an_agreeing_resource_mask_tie_reports_both_outcomes` (two `MASK_HASH` mask
  policies on one column, same priority) → `Undecidable` with `EvalOrderAmbiguous.outcomes` = two equal
  `TiedOutcome::Mask{mask_type: Some("MASK_HASH")…}` in `policy_names` order; `a_conflicting_tie_reports_each_outcome`
  (`MASK_HASH` vs `MASK`); the same for two row filters (`RowFilter{filter_expr}`). In `tag_arbitration.rs`: a tied
  tag mask pair → outcomes filled. An access tie (`EV:2405` shape) → `outcomes` empty. Expected on the base: compile
  error (no field).
- [ ] **Fix.** `pub enum TiedOutcome { Mask { mask_type, masked_value, mask_condition }, RowFilter { filter_expr } }`
  from each tied `SubResult`; fill at the band site (`deciding` vec) and the tag-band site (the `tied` candidates'
  `decide_sub` result, which is already computed in the filter: keep it instead of discarding). `Vec::new()` at the
  access site.
- [ ] Wire: `WireUndecidableCause::EvalOrderAmbiguous` gains `outcomes: Vec<WireTiedOutcome>`
  (`skip_serializing_if = "Vec::is_empty"`).
- [ ] Run `cargo test -p egret-ranger-eval`, `cargo test -p egret-perm-compile --test wire_dtos`, then
  `just test-ranger-gates`.

**Acceptance:** outcomes match yields in order; decisions unchanged everywhere (differential passes on both jar
versions).
**Pitfalls:** do not change any `Decision`/`MaskDecision` value: the differential compares them with the jars.
Destructure with `..` where tests match the variant. `crosscheck-prepared` must still agree (`cargo test -p
egret-ranger-eval --features crosscheck-prepared`).

## Task 2: One snapshot, a per-request cache, rule names (spec §5.9, C8, C11)

**Files:** `routes.rs` (`explain`, `explain_users`, `attribution_for_name`, `ExplainAttribution`), `routes/people.rs`,
`tests/routes.rs`.

- [ ] **Red (unit, in-file next to the attribution tests at the bottom of `routes.rs`):**
  `a_clearance_is_looked_up_once_per_request` — a counting fake `ClearanceLookups` wrapped by `ExplainCache`;
  attribute the same `perm-clearance-{id}` name three times → `tag_key` and `gates_for` called once each.
- [ ] **Red (route):** `explain_names_the_rule_it_attributes` in `tests/routes.rs` — a StarRocks path mask rule on
  `sales.orders.email` (reuse the fixture at `tests/routes.rs:4486`), explain a column read →
  `profile.mask.attribution.name == <rule name>`. Fails: no `name`.
- [ ] **Fix.**
  - `explain` and `explain_principal`: `let mut tx = pool.begin()`, `SET TRANSACTION ISOLATION LEVEL REPEATABLE
    READ, READ ONLY` (as `PR:1807`), run `check_explain_target` + `explain_users` on `&mut *tx`, then `tx.commit()`.
  - `ExplainCache` (struct in `routes.rs` or `explain_cache.rs`): `clearance: HashMap<String,
    Option<ExplainAttribution>>`, `rules: HashMap<Uuid, Option<RuleFacts{name, kind, exempt_include}>>`,
    `exemptions`, `members`, `tags`. `RuleFacts` read with `egret_perm::store::get_rule_via`
    (`crates/egret-perm/src/store.rs:652`).
  - `ExplainAttribution::Rule { rule_id, locator, name: Option<String> }` (`None` when the rule row is gone).
- [ ] Run `cargo test -p egret-perm-compile --lib` and `--test routes` (DB-gated).

**Acceptance:** both tests green; every existing explain test unchanged.
**Pitfalls:** `PermHttpError::db` on `begin`/`commit`. A route test cannot observe the snapshot itself; it is
covered by the Step 0 no-write check and by Task 5's model-vs-artifact test.

## Task 3: Masks on Trino (spec §5.2, C2)

**Files:** `routes.rs` (`PR:2502-2540`, doc `PR:744-748`), `tests/routes.rs`.

- [ ] **Helper** `trino_tag_profile(f, tag_label) -> (catalog_id, service_id)` in `tests/routes.rs`: a Trino profile
  via `trino_catalog_with_config` (`:8901`) whose `tagService` names a real tag service (the binding steps of
  `bind_tag_service_with_one_policy`, `:6786`, without its hand-written policy), then `assign_tag(f, catalog_id,
  "sales", "orders", "email")` (`:6708`).
- [ ] **Red:** `explain_reports_a_trino_classification_mask` — `POST /perm/rules` a tag mask `MASK_HASH` on the
  assigned tag's key, a grant so the reader is allowed, explain the column → Trino profile `mask.decision.kind ==
  "masked"`, `mask_type == "MASK_HASH"`, `attribution.kind == "rule"`. Fails: `mask` absent. Second test: the same
  with the tag on the table only (`attach_fixture_tag_to_whole_table` shape is refused for masks, so use a manual
  tag mask policy) → masked via `ANCESTOR`. Third: a declined resolution (`tag_resolution_declined` fixture used by
  existing tests) → `undecidable` with `tags_unresolved`.
- [ ] **Fix:** drop `engine == EngineType::StarRocks` from the mask condition; `resolved_tags: resolved_tags.clone()`;
  keep `masked_read_skips_access` StarRocks-only; rewrite the field doc.
- [ ] Run `--test routes`; the StarRocks mask tests (`tests/routes.rs:4204-4660`) stay green.

**Pitfalls:** the Trino rule compiles into the tag service only when `tagService` resolves
(`a_trino_engine_without_a_usable_tag_service_does_not_enforce_a_classification_rule`, `:8942`). Use `exclusive(&f)`
like the neighbours. Tag name ≠ key in the fixture on purpose; engines match by name.

## Task 4: Row-filter decision (spec §5.3, C3)

**Files:** `explain_wire.rs`, `routes.rs`, `tests/routes.rs`.

- [ ] **Red:**
  1. `explain_reports_a_path_row_filter_on_starrocks` — a path row filter `region = 'us'` on `sales.orders` →
     `row_filter.decision.kind == "filtered"`, `filter_expr == "region = 'us'"`, attribution rule; asked for the
     **column** target too (same answer).
  2. `explain_reports_a_trino_classification_row_filter_on_a_tagged_table` — `trino_tag_profile` with the tag on the
     table (`column_name = ''`) and a tag row-filter rule → `filtered`.
  3. `a_trino_row_filter_on_a_column_classification_filters_nothing` — tag on the column only → `no_filter` (E14).
  4. A table target with no rule → `no_filter`; a `write` explain → no `row_filter`.
- [ ] **Fix:** `WireRowFilterDecision`/`WireRowFilterResult` with `From<&RowFilterDecision>`; `ExplainRowFilter`;
  a second `resolved_tags_for_target` on `table_scope` once per request (before the profile loop);
  `object_request(engine, &catalog_name, &table_scope)`; `evaluate_row_filter`; attribution via the cache.
- [ ] Run `--test routes`.

**Pitfalls:** never reuse the column-level tag set for the table request (that would make test 3 filter, which no
engine does). A target without a table gets no row filter.

## Task 5: Exemption reasons (spec §5.4, D1)

**Files:** `routes.rs` (+ cache), `members.rs` (visibility only, if needed), `tests/explain_principal.rs`,
`tests/routes.rs`.

- [ ] **Red** (`explain_principal.rs`, group fan-out through `/perm/explain/principal`): a mask rule exempting
  group `data-team-<t>` and user `alice-<t>`, and role `analyst-<t>`; members: `bob` (in the group), `alice`
  (direct), `carol` (role holder and group member), `dave` (none).
  - bob → `mask.exemption.via == [{group, data-team-<t>}]`, `more == 0`;
  - alice → `[{user, alice-<t>}]` (rendered "directly");
  - carol → group and role, group first;
  - dave → no `exemption`, mask `MASK_HASH`.
  - Five exempting groups for one person → three in `via`, `more == 2`.
  - Row filter: the same for an exempt member (empty `filter_expr`).
- [ ] **Red (lag):** compile, then delete bob from the group **by raw SQL without recompiling** → bob's mask is still
  `MASK_NONE` and `exemption.via` is empty (spec §5.4 step 4).
- [ ] **Fix:** detect the exempt item (`MASK_NONE` / `""` from a `Rule` attribution); read `perm.rule_exemptions`
  `mode = 'include'` via the cached `RuleFacts`; resolve with `group_members_via`, `role_usernames(…,
  MissingUsername::Omit)`, `workspace_members_via` (cached per principal); workspace label from `workspaces.name`.
- [ ] Run `--test explain_principal`, `--test routes`.

**Pitfalls:** never `MissingUsername::Mint` here (read-only snapshot; it would fail). Compare usernames exactly as the
compiled body does. `public` is never an exemption (refused at compile, `rules.rs:717-760`).

## Task 6: StarRocks clearance replay and the table-level gap (spec §5.5, §5.8 D2, D4)

**Files:** `routes.rs`, `tests/clearance_starrocks.rs`, `tests/routes.rs`.

- [ ] **Red (D4)** in `clearance_starrocks.rs`, next to the fold tests (`:637`, `:818`, `:873`): a gated tag on a
  column, a rule `MASK_HASH` on it, a cleared reader → `mask.attribution.kind == "clearance"` and
  `mask.replayed.decision.result.mask_type == "MASK_HASH"`, `replayed.attribution.kind == "rule"`. Cleared and exempt
  from the rule → `replayed` `MASK_NONE` with `exemption.via`. Cleared, no rule → `replayed.decision.kind ==
  "no_mask"`. Uncleared → `MASK_NULL`, no `replayed`. Also update `tests/routes.rs:4606-4625`'s expectations only by
  adding `replayed` (`no_mask`).
- [ ] **Red (D2):** the tag on the **table** only, gated with a required role, StarRocks profile → explain the column
  → `clearance_not_enforced == [{tag_id, tag_name}]`, `select` allowed for an uncleared reader with a grant, no mask.
  The same tag also on the column → empty list.
- [ ] **Fix:** replay = `evaluate_data_mask` over a copy of the artifact with every policy whose name parses as
  `Clearance` with a `mask-` locator removed; gap = resolved tags with only `attached_at_column == false` ∩
  `clearance_gates_for_tags_via` with non-empty roles (tag name → key via the cache).
- [ ] Run `--test clearance_starrocks`, `--test routes`.

**Pitfalls:** filter by the attribution parser (`attribute_policy_name`), not by a string prefix written twice. Replay
only when the clearance outcome is not `MASK_NULL`. Trino profiles get neither field.

## Task 7: The effect lines and the Not-enforced line (spec §5.1, §5.7, §5.8 D3)

**Files:** `crates/egret-perm-compile/src/explain_effect.rs` (new, `mod` in `lib.rs`), `routes.rs`,
`tests/routes.rs`.

- [ ] **Red (unit, table-driven)** in `explain_effect.rs`: one case per row of spec §5.1's two tables, plus the mask
  word for each glossary type and an unlisted type; assert `text` and the part kinds; a property test that `text ==
  parts.map(text).concat()` for every case. Fails: module empty.
- [ ] **Red (route):**
  - `no_profile_explain_says_not_enforced_with_the_catalog` — orphan catalog (no engine) →
    `effect_line.text == "Not enforced: <name> has no engine."`; an engine attached without a profile → "…'s
    engines have no policy profile.";
  - Task 3's Trino mask → `effect_lines[0].text == "<u> reads email as hashed (rule <name>)"`;
  - Task 4 test 3 with a rule → rows line names the D3 gap, `row_filter_not_applied` has the rule.
- [ ] **Fix:** builder input = verdicts + `RuleFacts` + exemption + replay + gaps + subject + column; `row_filter_
  not_applied` = row-filter rules on a classification attached to a column of the target table and not to the table,
  on a Trino profile whose tag service resolves (one query per request); `ExplainResponse.effect_line` from
  `effect::NotEnforced` and the attachment read of `attached_engines` (`effect.rs:393`).
- [ ] Run `--lib`, `--test routes`.

**Pitfalls:** words come only from the builder; the route assembles facts. Keep each line one sentence.

## Task 8: Measure the cost (spec §5.9, C7)

**Files:** `tests/explain_principal.rs`.

- [ ] `#[ignore]` test `explain_200_members_on_two_engines_timing`: 200 users in one group, a Trino and a StarRocks
  profile on one catalog, a mask rule exempting another group, a row filter, a clearance; time `POST
  /perm/explain/principal` (5 runs, report min/median/max) with
  `cargo test --release -p egret-perm-compile --test explain_principal explain_200 -- --ignored --nocapture`.
- [ ] Record the numbers in spec §14. If the median exceeds **1 s**, build `PreparedPolicySet` once per profile and
  use `evaluate_*_prepared`; re-measure; record both.

## Task 9: The D3 count on the rule effect (spec §5.8, C9)

**Files:** `effect.rs`, `tests/routes.rs`.

- [ ] **Red:** `a_classification_row_filter_preview_counts_trino_column_only_tables` — `trino_tag_profile` with the
  tag on two columns of one table and on a second table as a whole → preview of a tag row filter has
  `effect.trinoColumnOnly == {catalogs: [<name>], tables: 1}`; a mask preview and a StarRocks-only catalog → `null`;
  the stored-rule `GET /perm/rules/{id}/effect` agrees with the preview (existing equality test stays green).
- [ ] **Fix:** in `rule_effect`, for `EffectKind::RowFilter` on a tag selector: tables with a column attachment and no
  table attachment, restricted to reached catalogs whose `engines` include `engine_type == "trino"`.
- [ ] Run `--test routes`.

## Task 10: OpenAPI and the generated client

- [ ] `cargo run -q -p egret-bin --bin egret -- openapi > openapi.json`; `cd frontend && npm run gen:api`; `npx tsc
  -b` (expect errors only where Tasks 11–12 change code); commit both.

## Task 11: Frontend, the answer (spec §5.10)

**Files:** `frontend/src/components/perm/ExplainResult.tsx`, new `EffectLine.tsx`, `MembersExplainView.tsx`; tests
`components/perm/__tests__/ExplainResult.test.tsx`, new `EffectLine.test.tsx`, `routes/governance/data/__tests__/
WhoCanReadDialog.test.tsx`.

- [ ] **Red:**
  - `EffectLine` renders a `strong` for the outcome, a link to `/governance/rules/<id>` named by the rule, a link to
    the classification's `?tab=clearance`, a principal link, mono code; its accessible text equals `text`.
  - `ExplainResultView`: the no-profile answer shows "Not enforced: <catalog> has no engine." and the badge "Not
    enforced"; a profile block shows its lines before a closed `details` "Which policies decided this" that holds the
    access rows; the limits line no longer says "Masks on Trino"; the folded paragraph is the new text.
  - `DecidedBy` shows the rule name; uuid when `name` is null.
  - `MembersExplainView`: each member row shows the first line per profile; counts unchanged.
  - `WhoCanReadDialog`: a user answer leads with the line (mocked response).
- [ ] **Fix**, then `npm run lint`, `npx tsc -b`, `npx vitest run src/components/perm src/routes/governance/data`.

**Pitfalls:** never parse `text`. Keep `maskDecisionText` only where still used (details); delete it if not
(dead code). Bundle markers (`scripts/assert-governance-data-lazy.mjs`) must still find `WHO_CAN_READ_INTRO`.

## Task 12: Frontend, the gaps (spec §5.8)

**Files:** `AddTagDialog.tsx`, `ClearanceTab.tsx`, `RuleDialog.tsx`, `RulePreview.tsx`; their `__tests__`.

- [ ] **Red:** the new hint text in `AddTagDialog` (and the old sentence absent, FE-COPY-3); `COMPILES_TO` contains the
  StarRocks whole-table sentence; `RuleDialog` with Applies to = Classification and Row filter shows the one-line
  hint, not for Mask or Path; a preview with `trinoColumnOnly` shows the warning with N and the catalogs, and Create
  is `aria-disabled` with its reason until "Create anyway" is ticked; a new preview clears the tick.
- [ ] **Fix**, reusing the C6 tick state (`RuleDialog.tsx:391-392`); then lint, tsc, vitest for those folders.

## Task 13: Final gate

- [ ] **Backend** (one command at a time; `pgrep -x cargo` empty first):

```bash
./deploy/prune-target.sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
env -u EGRET_ALLOW_DB_SKIP DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test --workspace 2>&1 | tee "$SCRATCH"/vmask-ws.log | tail -40
grep -c "skip: DATABASE_URL" "$SCRATCH"/vmask-ws.log
grep -E "^test result: FAILED|panicked" "$SCRATCH"/vmask-ws.log | head
cargo test -p egret-ranger-eval --features crosscheck-prepared
just test-ranger-gates 2>&1 | tee "$SCRATCH"/vmask-ranger.log | tail -20
cargo run -q -p egret-bin --bin egret -- openapi > openapi.json && git diff --exit-code openapi.json
cargo test -p egret-server committed_openapi_json_is_current
```

Expected: fmt and clippy clean; every `test result: ok`; the skip grep prints `0`; FAILED grep empty; Ranger gates
pass on every modelled jar version; no `openapi.json` diff.

- [ ] **Frontend** (from `frontend/`):

```bash
npm run gen:api && git status --short src/api/generated   # expect: no change
npm run lint
npx tsc -b
npm run test 2>&1 | tail -20
npm run build
npm run test:bundle && npm run test:bundle:governance-data && npm run test:bundle:governance-people \
  && npm run test:bundle:governance-classification && npm run test:bundle:governance-overview
```

Expected: every command exits 0.

## Task 14: Live check (own stack `egret-vmask`, port 18480; never the stack on 18080)

Compose override in the scratchpad (`-p egret-vmask`, `DEMO_PORT=18480`), as in the classification plan's Task 12.
Results and screenshots go to the scratchpad, not git.

1. Setup: a Trino engine (bound profile with a tag service) and a StarRocks engine on catalogs over the same table
   `sales.customers` (or one catalog each); classification `vmask-pii` attached to `email`; a second classification
   `vmask-region` attached to the **table**; rule *Hash personal data* (mask Hash on `vmask-pii`, exempt group
   `data-team`); clearance on `vmask-pii` for role `pii-cleared`; a row filter `region = 'us'` on `vmask-region`; a
   second row filter on `vmask-pii` (column-level, for D3). Users: `masked-u` (grant only), `exempt-u` (in
   `data-team`), `cleared-u` (holds `pii-cleared`).
2. Who can read this column? for each user. Expect on Trino: `masked-u` "reads email as **hashed** (rule *Hash
   personal data*)" or **refused** (clearance *vmask-pii*) — the Trino clearance deny wins for the uncleared; `exempt-u`
   refused likewise unless cleared; `cleared-u` hashed. On StarRocks: `masked-u` **NULL** (clearance); `cleared-u`
   "**hashed** by rule *Hash personal data* (cleared for *vmask-pii*)". Then lift the clearance and re-check:
   `exempt-u` "the real value (exempt from rule … via group data-team)" on both engines.
3. **Compare with the engines:** run `SELECT email FROM …` as each user on Trino and StarRocks (engine JWT via the
   query API); every line must match what the engine returns. Any mismatch is a defect, not a note.
4. Rows: each user sees "rows where `region = 'us'` (rule …)"; the `vmask-pii` row filter's line on Trino states the
   D3 gap, and the Trino query returns unfiltered rows while StarRocks filters.
5. D2: attach a gated classification to the whole table only; StarRocks line says the clearance is **not enforced**;
   the StarRocks query returns the clear value. Check the corrected `AddTagDialog` hint and the Clearance tab text.
6. D3 rule dialog: create a row filter on `vmask-pii` → warning with the count; Create needs the tick.
7. A group answer (`data-team`, then a 200-member group if seeded) renders per-member lines; time it.
8. A catalog with no engine → "Not enforced: <catalog> has no engine."
9. 1440 px and 390 px screenshots of the dialog in light and dark (FE-VISUAL-4); no horizontal scroll.

## Task 15: Pull request

- [ ] Push only after the user says so. PR title `feat(perm): verify a mask per user in Who can read?`. Body: the
  spec link, D1–D4, C1–C13, gate output, live results summary, the Docs box naming the follow-up docs PR (C13), and
  the non-goals (StarRocks table-level clearance compile next; Trino column-level row filter later). End with the
  attribution line from the session reminder.

## Self-review

- **Spec coverage:** §5.6 → T1; §5.9 → T2, T8; §5.2 → T3; §5.3 → T4; §5.4 → T5; §5.5 + D2 explain → T6; §5.1, §5.7,
  D3 explain → T7; D3 dialog → T9, T12; D2 copy → T12; §5.10 → T11.
- **Ordering:** each task ends green. T3–T6 depend on T2 (cache, `Rule.name`); T7 depends on T3–T6; T11–T12 on T10.
- **Open questions:** none blocking (spec §13).
