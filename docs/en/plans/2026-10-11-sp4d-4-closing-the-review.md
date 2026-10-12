# SP4d-4 Implementation Plan — closing the Control Plane & Data Governance review

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. **Task 1 ends with a STOP**: do not dispatch Tasks 11 and 12 until the user has picked branch B1 or B2 from Task 1's report.

**Goal:** Close the four items SP4d-3 left open: refuse (and report) mask/row-filter rules that overlap; make the policy editor's saved bodies grant what the editor shows, after measuring what they grant; let an operator register an externally run Trino/StarRocks engine and bind it to a profile; and make the SP4d-3 equivalence gate scale by pruning coordinates soundly.

**Architecture:** A — the one overlap rule (`egret_policy::ambiguous_arbitration`) widens from *equal resources* to *may decide the same request*, the compiler adds the tag cases only it can see (it has the tag assignments), every refusal is a 409 naming rules, and a read-only route reports every pair. B — the real Ranger jars decide what an absent level means; the editor then either saves an empty non-root level as `*` and flags existing rows (B1), or the grammar accepts what provably decides the same (B2). C — `engine_mgmt.engines` gains an `origin` column whose `CHECK` makes an external row's shape unrepresentable otherwise; every lifecycle handler learns to refuse it; attachments to it are *declared*; its plugin version is declared, not derived. D — `egret-ranger-eval` exports the candidacy predicate it already has, and the gate keeps only probes at coordinates some changed policy may be a candidate for, falling back to the full set where the argument does not hold.

**Tech Stack:** Rust (axum, sqlx, serde_json), PostgreSQL, `egret-ranger-eval`, the Java differential harness under `tests/ranger/` (Maven + real `ranger-plugins-common` 2.5.0 / 2.8.0 jars), React + react-query + Vite.

**Spec:** `docs/en/specs/2026-10-11-sp4d-4-closing-the-review-design.md` — read its "What is already true" section before any task; several premises of the original brief turned out false there (A is half-closed already; the broader rule wins Ranger's evalOrder).

---

## Global Constraints

- **egret IS the Ranger Admin.** No Apache Ranger server exists; the plugin inside each engine downloads policy straight from egret, so a row in `policy.authz_policies` *is* the decision the engine enforces. Trino 470 runs `ranger-plugins-common-2.5.0`; StarRocks fe-ubuntu 4.1 runs `2.8.0`. Any claim about plugin behaviour must name a version.
- **The action expansion is fixed** (`crates/egret-perm/src/actions.rs::engine_accesses`): `Action::Read` → Trino `["select"]`, StarRocks `["select"]`; `Action::Write` → Trino `["insert", "delete"]`, StarRocks `["insert", "update", "delete"]`.
- **Every compiled rule is emitted at `policyPriority 0`** (`crates/egret-policy/src/admin/ranger_json.rs:220`, `:288`; `crates/egret-perm-compile/src/rules.rs:521,535,567,581` pass `is_override: false`). This package does not change that — refusal is the ruling, not priority assignment.
- **Ranger's evalOrder puts the BROADER policy first** (spec, Part A, item 1): within one `(policyPriority, hasDeny)` band, a policy with more match-any levels has a LOWER `evalOrder` and is evaluated first, and for masks and row filters the first matching evaluator is final. Do not write a comment, sentence or test that says the more specific rule wins.
- **Verify the next migration number against `origin/main` at the moment you create the file:** `git fetch origin && git ls-tree --name-only origin/main migrations/ | sort | tail -3`. Newest on 2026-10-11 is `0182_tag_service_bump_reaches_its_linkers.sql`. This plan creates exactly ONE migration (Task 7). On the branch merged as #656 four migrations were written at numbers main had since used, `migrate()` aborted at the first collision, and none of the four `CHECK`s was ever created while the Rust refusals shipped green.
- **Migration prose is append-only** for any migration applied anywhere — sqlx checksums applied migrations.
- `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test`, passed **explicitly** on every command. **Never** the dev database `egret`. **Never** `EGRET_ALLOW_DB_SKIP=1`.
- **Per-crate `-p` only.** Never `cargo test --workspace`, `cargo clippy --workspace`, or `just check` during a task. Free disk was ~20 GiB on 2026-10-11 and `cargo test --workspace` drove it to 4 GiB that day. Run `df -h / | tail -1` before and after anything that builds and put both numbers in the report. If free space is below 8 GiB, STOP and report rather than build. `cargo clippy -p <crate> --no-deps --all-targets`.
- Regenerate the API client with **`npm run gen:api`** only — bare `orval` drops `encodeURIComponent` from path params.
- Never pass `-q`; empty output is not a pass; capture exit codes; `--no-fail-fast`. **Never read a suite total through `tail`** — grep the whole log for `test result:` and sum with `awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'`.
- Log files go to `/tmp/sp4d4-<task>-<what>.log` — the prefix matters: parallel agents share temp directories.
- **Mutations:** assert the mutated **code** expression occurs exactly once (`grep -c` == 1, on code, not on a doc comment), confirm the mutant **compiles** (`grep -c '^error\[' <log>` is 0), confirm the intended test fails **by name**, restore by plain write and compare SHA-256 before and after (`shasum -a 256`). `cp -i` and `rm -i` are aliased live here and `cp -f` does not defeat them.
- Never `git stash` — the stack is shared with other worktrees and sessions. Use a WIP commit.
- **Nothing may compare two JSON values through `to_string()`**: `preserve_order` is on, `Map` is an `IndexMap`, and a `jsonb` round-trip reorders keys. Compare `Value == Value`.
- **DB tests:** `#[tokio::test]`, `let dsn = egret_test_db::url().await?; let db = Db::connect(&dsn, 5).await.expect("connect"); db.migrate().await.expect("migrate");` (the shape `crates/egret-perm-compile/tests/migrate_route.rs:85-89` uses), and every assertion scoped to the test's own fixture — the test database is shared with concurrent tests.
- **The real-engine gate is `just test-ranger-gates`** (`justfile:162-168`). Report each test's **elapsed time**: the vacuous form of every gate is `0.00 s`. Keep `EGRET_TEST_RANGER_E2E=1` **literal** on every recipe line.
- **Frontend tasks** follow `docs/en/frontend-rules.md` (normative) and answer its reviewer checklist; the gate is FE-TEST-4: from `frontend/`, `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`. Vitest paths must include `__tests__/` — a wrong path exits 0 with "No test files found".

### Baselines

Last known, from the #660 merge (2026-10-11, not re-measured for this plan): `cargo test --workspace` 6155 pass / 21 fail, every failure an environment guard (`serving_shared_store` needs Valkey on :6399, `sql_starrocks_live` needs StarRocks on :9030); frontend vitest 6007/6007. **Before dispatching Task 1, the controller records per-crate baselines in the SDD ledger** with the same commands every task uses, for `-p egret-policy`, `-p egret-perm-compile`, `-p egret-ranger-eval`, `-p egret-engine-mgmt`, `-p egret-bi`, `-p egret-server --lib`, and `just test-ranger-gates` with elapsed times. A task's "nothing else broke" claim is against those numbers.

---

## File Structure

**New files**

| Path | Task | Responsibility |
|---|---|---|
| `docs/en/plans/2026-10-11-sp4d-4-task1-levels-result.md` | 1 | the measured matrix and the B1/B2 recommendation |
| `crates/egret-perm-compile/src/overlap.rs` | 3 | the compiler-only overlap cases (tag ↔ tag, tag ↔ path) and the rule-naming of a refusal |
| `crates/egret-perm-compile/tests/overlap.rs` | 3, 4 | DB tests for rule writes, tag assignment writes, and the report route |
| `crates/egret-perm-compile/tests/gate_timing.rs` | 6 | the `#[ignore]`d timing harness behind `MAX_PROBE_EVALUATIONS` |
| `migrations/0184_engine_external.sql` | 7 | `origin`, the external columns, the shape `CHECK`, `'declared'` attachments (number re-checked at creation) |
| `crates/egret-engine-mgmt/src/external.rs` | 7, 8 | registration, the `origin` guard every handler calls, declared attachments |
| `crates/egret-provisioner/src/ranger_config.rs` | 9 | the ONE renderer of the Ranger plugin files, used by provisioning and by the new route |
| `frontend/src/routes/control-plane/EngineRegister.tsx` | 10 | the register page |
| `frontend/src/routes/control-plane/__tests__/EngineRegister.test.tsx` | 10 | its tests |
| `frontend/src/routes/control-plane/EnginePluginConfig.tsx` | 10 | the plugin-config tab for an external engine |

**Modified files (by task)**

| Path | Task | Change |
|---|---|---|
| `tests/ranger/src/test/java/EgretCaseRunner.java` | 1 | optional `resourceMatchingScope` on a request (additive) |
| `crates/egret-ranger-eval/tests/differential.rs` | 1, 6 | `ask_ranger_json`; the levels matrix; the dropped-probe check |
| `justfile` (`test-ranger-gates`, `:162-168`) | 1, 6 | the two new gates |
| `crates/egret-policy/src/ambiguous_arbitration.rs` | 2 | may-decide intersection; return every pair |
| `crates/egret-policy/src/admin/mod.rs:1171-1237` | 2 | 409, and the `isOverride` remedy |
| `crates/egret-perm/src/actions.rs:102-125` | 3 | `AmbiguousMaskOrFilterArbitration` carries rules and coordinate |
| `crates/egret-perm-compile/src/trigger.rs:562-598` | 3 | call `overlap::refuse_overlaps` on the desired set + assignments |
| `crates/egret-perm-compile/src/routes/error.rs:81-93` | 3 | that variant → 409 with a structured body |
| `crates/egret-bi/src/catalog_tags.rs` | 3 | the recompile's 409 survives as a 409 |
| `crates/egret-perm-compile/src/routes.rs` | 4, 7 | `GET /perm/overlaps`; `explain`'s plugin version |
| `frontend/src/routes/governance/MaskFilterRules.tsx` | 4 | the overlap banner |
| `crates/egret-ranger-eval/src/eval.rs` | 5 | export `may_be_candidate` (built from `provably_a_candidate` + `trie_excludes`) |
| `crates/egret-perm-compile/src/equivalence.rs` | 5 | `PruneMode`, `derive_probes_pruned`, `probe_count_pruned` |
| `crates/egret-perm-compile/src/migrate.rs` | 6, 7 | use pruning; re-measured cap; `engine_plugin_version` |
| `crates/egret-engine-mgmt/src/store.rs`, `engines.rs`, `catalogs.rs`, `catalog_apply.rs`, `monitor.rs`, `assignment.rs` | 7, 8 | origin-aware |
| `crates/egret-provisioner/src/trino.rs:348-370`, `starrocks.rs:185-215` | 9 | call `ranger_config` |
| `frontend/src/routes/control-plane/ControlPlaneEngines.tsx`, `EngineDetail.tsx`, `frontend/src/App.tsx:437-439` | 10 | entry, external detail, route |
| `crates/egret-policy/src/admin/ranger_json.rs:305` (`editor_to_ranger_json`) | 11 | empty-level handling per branch (editor path only) |
| `crates/egret-perm-compile/src/classify.rs` | 11 | per-policy `warnings`; the `MissingLevel` sentence |
| `frontend/src/components/policy/ResourceSelector.tsx`, `ResourceLevelEditor.tsx`, `frontend/src/routes/governance/ProfileDetailPage.tsx` | 12 | the note, the warning, the flagged list |

**Order and parallelism.** Task 1 first (it also pins a Part A fact). Then A (2 → 3 → 4), D (5 → 6) and C (7 → 8 → 9 → 10) are independent chains and may run in any interleaving, **one task at a time per worktree**. Tasks 11–12 wait for the user's B1/B2 answer. Task 13 last. Tasks 3, 4, 6, 7 each change `crates/egret-perm-compile` — never run two of them at once.

---

## Task 1: what an absent level grants on the real Ranger jars, and how the plugins ask — then STOP

**Files:**
- Modify: `tests/ranger/src/test/java/EgretCaseRunner.java` (`request()`, `:192-212`)
- Modify: `crates/egret-ranger-eval/tests/differential.rs` (`ask_ranger`, `:4820-4898`; new test at the end)
- Modify: `justfile` (`test-ranger-gates`)
- Create: `docs/en/plans/2026-10-11-sp4d-4-task1-levels-result.md`

**Interfaces:**
- Produces: `fn ask_ranger_json(payload: Vec<Value>, ranger_version: &str, tag: &str) -> Vec<Value>` in `differential.rs`; `ask_ranger(cases, service_type, version, tag)` becomes `ask_ranger_json(cases.iter().map(|c| c.to_json(service_type)).collect(), version, tag)` — identical behaviour, including both of its assertions (the `Tests run: 4 … EgretRangerEnforcementTest` anchor line and `verdicts.len() == payload.len()`).
- Produces: request JSON accepts `"resourceMatchingScope": "SELF" | "SELF_OR_DESCENDANTS"`; absent means Ranger's default (`SELF`).
- Consumes: `load_engine`, `load_tag_def`, `mig_envelope`, `mig_level`, `mig_item`, `mig_mask_item`, `tag_block`, `trino()`, `starrocks()` (all in `differential.rs`); `egret_ranger_eval::eval::{evaluate, evaluate_data_mask, AccessRequest, EngineConfig, PluginVersion}`.

Why both versions for both service-defs: an EXTERNAL engine (Task 7) may run either; the provisioned pairing (Trino → 2.5.0, StarRocks → 2.8.0) is not the only one that will exist after this package.

- [ ] **Step 1: The Java change (additive)**

In `EgretCaseRunner.request(JsonObject req)`, after `r.setAccessType(...)` and before the tags block:

```java
// SP4d-4 Task 1: the Trino plugin's filter* methods ask with
// SELF_OR_DESCENDANTS (RangerSystemAccessControl.java:972/:990 at tag 470).
// Absent = Ranger's default (SELF), so every existing case serialises and
// evaluates exactly as before.
if (req.has("resourceMatchingScope")) {
    r.setResourceMatchingScope(RangerAccessRequest.ResourceMatchingScope.valueOf(
            req.get("resourceMatchingScope").getAsString()));
}
```

Add the import `org.apache.ranger.plugin.policyengine.RangerAccessRequest` if it is not already imported. Verify the setter exists on `RangerAccessRequestImpl` at both versions before relying on it:

```bash
cd tests/ranger && for v in 2.5.0 2.8.0; do
  unzip -p ~/.m2/repository/org/apache/ranger/ranger-plugins-common/$v/ranger-plugins-common-$v.jar \
    org/apache/ranger/plugin/policyengine/RangerAccessRequestImpl.class > /tmp/sp4d4-t1-req-$v.class
  javap -cp /tmp -p /tmp/sp4d4-t1-req-$v.class | grep -c setResourceMatchingScope
done
```
Expected: `1` twice. If either is 0, STOP and report — the matrix's filter half cannot be measured that way.

- [ ] **Step 2: Refactor `ask_ranger` into `ask_ranger_json` and prove nothing moved**

Mechanical: the body of `ask_ranger` from `let payload` down becomes `ask_ranger_json`. Run the existing gate unchanged and compare the elapsed times and pass counts with the controller's baseline:

```bash
df -h / | tail -1
EGRET_TEST_RANGER_E2E=1 DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-ranger-eval --test differential --no-fail-fast -- --nocapture \
  --skip a_migration_the_grammar_accepts_decides_identically_on_the_real_engine \
  > /tmp/sp4d4-t1-refactor.log 2>&1; echo "EXIT=$?"
grep -c '^error\[' /tmp/sp4d4-t1-refactor.log
grep 'test result:' /tmp/sp4d4-t1-refactor.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
grep -E 'finished in' /tmp/sp4d4-t1-refactor.log
```

- [ ] **Step 3: Write the matrix test**

`editor_shaped_levels_on_the_real_ranger_jars`, gated on `EGRET_TEST_RANGER_E2E=1` exactly like its neighbours. For each `profile ∈ {trino(), starrocks()}` and each `version ∈ {"2.5.0", "2.8.0"}`:

*Policies* (one per case set; each set is that one policy plus nothing — no background, so nothing else can grant). All grant user `alice` the accesses `["select","insert","delete"]` (StarRocks adds `"update"`), `policyType 0`, `isEnabled: true`, priority 0. With `h = profile.hierarchy`:

| id | resources | name |
|---|---|---|
| `d1` | `{h0: [wh]}` | stops at catalog |
| `d2` | `{h0: [wh], h1: [sales]}` | stops at schema/database |
| `d3` | `{h0: [wh], h1: [sales], h2: [orders]}` | stops at table |
| `d4star` | `{h0: [wh], h1: [sales], h2: [orders], h3: [*]}` | the control egret's compiler emits |
| `gap` | `{h0: [wh], h2: [orders]}` | a named level below an absent one (what the editor saves when a middle level is emptied) |

*Requests*, all for `alice`: resources at depth 1 `{h0: wh}`, 2 `{…, h1: sales}`, 3 `{…, h2: orders}`, 4 `{…, h3: email}`; access types `select`, `insert`, `delete` (+ `update` on StarRocks) at `SELF`; and `_any` at `SELF` and at `SELF_OR_DESCENDANTS`.

Send every (policy, request) pair through `ask_ranger_json` (one Maven run per profile × version; tag the run `levels-<engine>`), with `"policyType": 0`. For every concrete-access cell at `SELF`, evaluate the same pair with `egret_ranger_eval::eval::evaluate` using `load_engine(&profile)`'s config with `plugin_version` set to the version under test (`PluginVersion::Ranger2_5_0` / `Ranger2_8_0`), and **assert the Rust answer equals Java's `isAllowed`** — a disagreement here is a Critical finding about the evaluator, reported, not "fixed" by adjusting the assertion. `_any` cells and `SELF_OR_DESCENDANTS` cells are Java-only (the evaluator refuses `_any` as `PseudoAccessType`).

*The two Part A pins*, in the same runs, `policyType 1`, request `{h0: wh, h1: sales, h2: orders, h3: email}`, access `select`, user `alice`:

1. **Broader wins.** Policy `m-broad` on `{wh, sales, *, email}` → `MASK` for `groups: ["public"]`; policy `m-narrow` on `{wh, sales, orders, email}` → `MASK_NULL` for `groups: ["public"]`; both priority 0. Assert Java's `maskType == "MASK"` at both versions. **Control:** the same two with their NAMES swapped (`a-narrow` / `z-broad`) — assert `MASK` again, which shows the winner is `evalOrder`, not the name.
2. **Tag beats path at equal priority** (Trino def only, both versions). A resource mask `MASK_NULL` on `{wh, sales, orders, email}` and a tag mask `MASK` on tag `pii` (via `tag_block`), the request carrying the resolved tag `pii` at `SELF`. Assert Java's `maskType == "MASK"`.

For both pins, also record what the Rust evaluator answers (expected `Undecidable` with `EvalOrderAmbiguous` for pin 1 — that is the modelling gap the spec names — and `Masked{MASK}` or a split refusal for pin 2) without asserting it.

*Print* one table per profile × version: rows = policy id, columns = request depth × access (× scope for `_any`), cells `A` (allowed) / `-` (denied). Assert the total number of Java verdicts is the number of cases sent (non-zero).

- [ ] **Step 4: Run it, and report the elapsed time**

```bash
df -h / | tail -1
EGRET_TEST_RANGER_E2E=1 DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-ranger-eval --test differential --no-fail-fast -- --nocapture \
  --exact editor_shaped_levels_on_the_real_ranger_jars > /tmp/sp4d4-t1-matrix.log 2>&1; echo "EXIT=$?"
grep -c '^error\[' /tmp/sp4d4-t1-matrix.log
grep 'test result:' /tmp/sp4d4-t1-matrix.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
grep -E 'finished in' /tmp/sp4d4-t1-matrix.log
```
Expected: PASS, elapsed well above `0.00 s`.

- [ ] **Step 5: Re-read the plugin request formation at the deployed tags**

Do not trust the spec's citations; re-read and quote:

```bash
git -C ../trino show 470:plugin/trino-ranger/src/main/java/io/trino/plugin/ranger/RangerSystemAccessControl.java \
  | sed -n '415,445p;515,560p;965,995p'
git -C ../trino show 470:plugin/trino-ranger/src/main/java/io/trino/plugin/ranger/RangerTrinoResource.java | sed -n '95,110p'
git -C ../starrocks show 4.1.4:fe/fe-core/src/main/java/com/starrocks/authorization/ColumnPrivilege.java | sed -n '85,145p'
git -C ../starrocks show 4.1.4:fe/fe-core/src/main/java/com/starrocks/authorization/ranger/starrocks/RangerStarRocksAccessController.java | sed -n '100,172p'
```
(`../trino` and `../starrocks` are the checkouts beside `egret-sp4a/`.) Record, per engine: which resource depth a `SELECT col FROM t`, a `SELECT count(*) FROM t`, an `INSERT`, a `DELETE`, `SHOW TABLES` and a table filter ask at, with which access type and scope.

- [ ] **Step 6: Add the gate and commit**

Add to `test-ranger-gates`, with `EGRET_TEST_RANGER_E2E=1` literal on the line, a `--exact editor_shaped_levels_on_the_real_ranger_jars` run plus the same `--list … | grep -qx` guard the migration gate has (`justfile:167`), and extend the existing `--skip` line to skip it there. Then:

```bash
just test-ranger-gates > /tmp/sp4d4-t1-gates.log 2>&1; echo "EXIT=$?"
grep -E 'finished in|test result:' /tmp/sp4d4-t1-gates.log
git add tests/ranger/src/test/java/EgretCaseRunner.java crates/egret-ranger-eval/tests/differential.rs justfile
git commit -m "test(ranger-eval): measure what a policy stopping above the leaf grants on 2.5.0 and 2.8.0"
```

- [ ] **Step 7: Write the result doc and STOP**

`docs/en/plans/2026-10-11-sp4d-4-task1-levels-result.md`: the four tables, the two pins' Java answers, the Rust/Java agreement count, the elapsed times, and Step 5's per-engine request-formation list with the quoted lines. Then a recommendation using this rule, stated in the doc:

- **B1** if `d3` (and `d2`, `d1`) is denied `select` at depth 4 on every profile × version **and** Step 5 shows both engines check `SELECT col` at column depth.
- **B2** if `d3` is allowed `select` at depth 4 on every profile × version.
- **Mixed** otherwise — name exactly which cells differ; the user decides.

Also state what `gap` decided (expected: nothing anywhere — it fixes in both branches).

```bash
git add docs/en/plans/2026-10-11-sp4d-4-task1-levels-result.md
git commit -m "docs(plans): SP4d-4 Task 1 — what an absent level grants, measured"
```

**STOP. Report to the controller: the recommendation, the four tables, and the two pins. The controller asks the user "B1 or B2?" and records the answer in the ledger before Tasks 11 and 12 are dispatched.** If either Part A pin did NOT come out as `MASK`, also stop Tasks 2–4: the spec's evalOrder finding is wrong and Part A's wording must be revisited with the user.

---

## Task 2: the one overlap rule decides "may decide the same request", returns every pair, and the editor answers 409

**Files:**
- Modify: `crates/egret-policy/src/ambiguous_arbitration.rs`
- Modify: `crates/egret-policy/src/lib.rs:19` (re-exports)
- Modify: `crates/egret-policy/src/admin/mod.rs:1171-1237` (`refuse_ambiguous_arbitration`)
- Test: the module's `#[cfg(test)]` block; `crates/egret-policy/tests/admin_routes.rs:1523` (`a_second_row_filter_on_one_table_is_refused_only_when_it_disagrees`) and the other 400 assertions this guard owns (`rg -n 'refusal\(\)|Narrow one of the two' crates/egret-policy/tests/admin_routes.rs`)

**Interfaces:**
- Produces (replacing the single-pair return):

```rust
/// Every overlapping pair in `policies`, in a deterministic order (by kind,
/// then the pair's two names). Empty = nothing to refuse.
pub fn ambiguous_arbitrations_in<'a>(
    policies: impl IntoIterator<Item = PolicyUnderWrite<'a>>,
) -> Vec<AmbiguousArbitration>;

/// The first of `ambiguous_arbitrations_in`, for write paths that report one.
pub fn ambiguous_arbitration_in<'a>(
    policies: impl IntoIterator<Item = PolicyUnderWrite<'a>>,
) -> Option<AmbiguousArbitration>;

/// Whether two Ranger resource maps may select a common request coordinate.
/// Over-approximates; `true` is the fail-closed answer.
pub fn resources_may_intersect(a: &Value, b: &Value) -> bool;
```

  `AmbiguousArbitration` gains `pub coordinate_hint: String` — the rendered intersection where it is exact, else both resource renderings joined by " ∩ ".
- Consumes: nothing new.

**The rule, exactly** (spec Part A, o1–o5, for the single-download case):
two enabled policies with the same `service`, the same `policyType ∈ {1,2}`, the same `has_deny` and the same `policyPriority`, whose resources **may intersect**, and whose decisive item arrays differ (`Value !=`). Not "whose canonical resources are equal".

`resources_may_intersect(a, b)`: for every level name in the union of both maps' keys, the two levels may-intersect, where a level that is absent, empty (values `[]`, read as absent exactly as `canonical_resources` already does, `:354-382`), or carries `isExcludes: true` may-intersect anything, and otherwise some value pair may-intersect:
- equal under ASCII lowercase (every level of both service-defs is `ignoreCase: true`), or
- one is exactly `prefix*` (a single trailing `*`, no other `*`/`?`) and the other, lowercased, starts with the lowercased prefix (two such: either prefix starts with the other), or
- either contains `*` or `?` in any other position → `true`.

⚠ Keep `canonical_resources`' own reading of `"isExcludes": "true"` (a string) unchanged in this task — it is a known pre-existing defect (it reads as `false`), out of scope here; do NOT widen the change.

- [ ] **Step 1: Rewrite the module doc's "Why this is NARROWER" section first**

`:106-126` argues that different-specificity pairs are admitted because `getEvalOrder()` separates them. Replace it with a section **"Why different resources are refused too"** that states, with the citations from the spec's Part A item 1: the `evalOrder` arithmetic at both release tags, the ascending sort, the `>=` arbitration, and the consequence (the broader policy wins; the narrower never applies to anyone the broader one's items cover). Cite Task 1's result doc for the measured pin. Keep the "egret can still emit an artifact its own evaluator declines" paragraph only if it is still true after this task (it is, for agreeing ties); delete the sentence that calls a schema-wide plus column mask "an ordinary, correct configuration".

- [ ] **Step 2: Write the failing tests** (in the module's test block)

1. `a_wildcard_mask_and_a_named_mask_on_one_column_are_refused` — `{wh, sales, *, email}` `MASK` vs `{wh, sales, orders, email}` `MASK_NULL` → `Some`, `kind = Mask`. This is the case the old key admitted.
2. `a_prefix_glob_and_a_disjoint_name_are_admitted` — `{wh, sales, ord*, email}` vs `{wh, sales, items, email}` with differing masks → `None`; and `{wh, sales, ord*, email}` vs `{wh, sales, ORDERS, email}` → `Some` (case fold).
3. `two_different_columns_still_do_not_overlap` — the existing accept-control 1 (`:683-692`) must stay `None`.
4. `different_exemptions_with_the_same_mask_are_refused` — both `MASK` on one column, but A's bucket is `[exempt(vip) MASK_NONE, public MASK]` and B's is `[public MASK]` → `Some`. The test's doc says why: for `vip`, A answers `MASK_NONE` and B answers `MASK`.
5. `identical_buckets_on_intersecting_resources_are_admitted` — `{wh, sales, *, email}` and `{wh, sales, orders, email}` with **identical** `MASK` buckets → `None` (agreeing tie, F2's rule).
6. `an_excluded_level_is_assumed_to_intersect` — `isExcludes: true` on one side's `table` → `Some`.
7. `every_pair_is_returned_in_a_stable_order` — three mutually overlapping differing masks → `ambiguous_arbitrations_in` returns 3 pairs, and the same 3 in the same order for the reversed input.

Keep every existing test in the block passing unchanged except where its assertion encoded the old equality key (none should; if one does, report it rather than editing it silently).

- [ ] **Step 3: Run, fail, implement, run**

```bash
df -h / | tail -1
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-policy --lib ambiguous_arbitration --no-fail-fast > /tmp/sp4d4-t2-unit.log 2>&1; echo "EXIT=$?"
grep -c '^error\[' /tmp/sp4d4-t2-unit.log
grep 'test result:' /tmp/sp4d4-t2-unit.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
```
Implementation note: the band key loses `canonical_resources`; group by `(service, policy_type, has_deny, priority)` and compare every pair inside a group with `resources_may_intersect` and `!=` buckets. Quadratic per band is fine (bands are per profile).

- [ ] **Step 4: The editor answers 409**

In `refuse_ambiguous_arbitration`, return `PolicyHttpError::Conflict(...)` instead of `BadRequest`, and append to the sentence, for the editor only: "or set Override on the one that should win (an override is evaluated first)". Update the editor route tests that asserted 400 for this guard to assert 409 **and** the substring of the remedy; leave every other 400 alone. Update the utoipa `responses(...)` of `create_policy`/`update_policy` to document the 409.

- [ ] **Step 5: Mutation — the intersection**

Make `resources_may_intersect` return `canonical_resources(a) == canonical_resources(b)` (the old key). Expected: tests 1, 2 (second half) and 6 fail by name and nothing else. `count == 1`, compiles, restore with SHA-256 compare.

- [ ] **Step 6: The crate, clippy, commit**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-policy --no-fail-fast > /tmp/sp4d4-t2-crate.log 2>&1; echo "EXIT=$?"
grep 'test result:' /tmp/sp4d4-t2-crate.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
cargo clippy -p egret-policy --no-deps --all-targets 2>&1 | tail -3
df -h / | tail -1
git add crates/egret-policy
git commit -m "fix(policy): refuse mask and row-filter pairs that may decide one request differently"
```

---

## Task 3: the compiler refuses tag overlaps it alone can see, names the rules, and answers 409 everywhere

**Files:**
- Create: `crates/egret-perm-compile/src/overlap.rs`; declare it in `crates/egret-perm-compile/src/lib.rs`
- Modify: `crates/egret-perm/src/actions.rs:102-125`
- Modify: `crates/egret-perm-compile/src/trigger.rs:562-598`
- Modify: `crates/egret-perm-compile/src/routes/error.rs`
- Modify: `crates/egret-bi/src/catalog_tags.rs` (the two recompiling writes, `:138`, `:176`) and `crates/egret-bi/src/error.rs` if its mapping loses the status
- Test: create `crates/egret-perm-compile/tests/overlap.rs`; update `crates/egret-perm-compile/tests/recompile.rs` (`two_rules_masking_one_column_differently_refuse_the_whole_recompile`, ~`:1392`)

**Interfaces:**
- Produces:

```rust
// egret-perm, actions.rs — the variant grows; the Display stays one sentence.
AmbiguousMaskOrFilterArbitration {
    profile: String,
    refusal: String,
    kind: OverlapKind,              // Mask | RowFilter
    coordinate: String,
    rules: Vec<OverlapRule>,        // exactly two; a hand-written policy has id None
}
pub struct OverlapRule { pub id: Option<uuid::Uuid>, pub name: String }

// egret-perm-compile, overlap.rs
pub struct CompiledForOverlap<'a> {
    pub service: i64,               // the service the body is stored under
    pub is_tag_service: bool,
    pub policy_name: &'a str,
    pub body: &'a serde_json::Value,
    pub rule: Option<OverlapRule>,  // from DesiredPolicy.rule_id + the rule's name
}
/// Every overlap in one profile's desired set (resource service + its tag
/// service) given the profile's tag assignments. Pure.
pub fn overlaps(
    desired: &[CompiledForOverlap<'_>],
    assignments: &[crate::equivalence::TagAssignment],
    service_def: &egret_ranger_eval::servicedef::ServiceDef,
) -> Vec<Overlap>;
pub struct Overlap { pub kind: OverlapKind, pub coordinate: String,
                     pub rules: [OverlapRule; 2], pub sentence: String }
```

- Consumes: `egret_policy::ambiguous_arbitrations_in` (Task 2) for every within-download pair — **do not restate it**; `egret_ranger_eval::resource::match_policy_resources` for "does this path policy apply at this assigned coordinate"; `egret_policy::tagged_columns_for_service_via` or the assignment query `equivalence::TagAssignment` is built from in `migrate.rs` (read how SP4d-3 builds `TagAssignment`s for a profile and reuse that query); `crate::routes::fold_divergent`.

**What `overlaps` adds on top of Task 2** (spec Part A, o1 across the tag/resource boundary and the tag ↔ tag case):
For each assigned coordinate (a column, and — for row filters — its table; a table-level assignment applies to the table and to every column of it that appears in any assignment or path policy), collect the policies of one `policyType` that **apply** there: tag policies whose `resources.tag` values cover (Ranger wildcard match, ASCII-folded) a tag resolved at that coordinate, and path policies whose `match_policy_resources` against the coordinate is `Self_` or `SelfAndAllDescendants`. If two of them have different decisive arrays → an `Overlap`. Band equality (o3) is required only between two tag policies; tag ↔ path overlaps at any priority (spec, o3's row). A coordinate value that `fold_divergent` flags → an `Overlap` is NOT emitted for it; instead `overlaps` returns a refusal sentence the caller turns into the same 409 ("cannot be checked: `<value>` is fold-divergent") — fail closed, as the gate does.

The refusal sentence names **both rules by name** ("rule \"mask emails\" and rule \"null pii\" both decide wh.sales.orders.email …"), says which one Ranger applies today when that is determinate (tag over path at equal priority; the broader of two path rules) and says plainly when it is the policy-name tie-break, and gives the remedy (narrow one, delete one, make them agree; for tag ↔ tag, or unassign one of the two tags from that column). It must not contain "random", "undefined", or "may change between queries".

- [ ] **Step 1: Re-verify the write-path list by code, before writing a test**

```bash
rg -n 'recompile_service_in_tx|recompile_services_in_tx|recompile_service\(' crates --glob '!**/tests/**'
rg -n 'fn lock_services_in_tx' -A30 crates/egret-perm-compile/src/trigger.rs
```
Write the list into `overlap.rs`'s module doc (by code path). From the lock function, determine whether two concurrent transactions recompiling two profiles that share one tag service serialise. Record the answer in the module doc as measured; if they do NOT serialise, say so as a named limit and tell the controller — do not add locking in this task.

- [ ] **Step 2: Write the failing DB tests** (`tests/overlap.rs`; build the fixture the way `tests/recompile.rs`' `create_tag_and_mask_rule` does)

1. `a_second_rule_on_an_intersecting_path_is_409_naming_the_first` — rule A `('sales','*','email') MASK`, then `POST /api/control-plane/perm/rules` rule B `('sales','orders','email') MASK_NULL` → **409**, body `code == "rule_overlap"`, `rules` holds both ids and names, the `error` sentence contains A's **name**, and `perm.rules` holds only A afterwards.
2. `a_patch_that_makes_two_rules_agree_succeeds` — after test 1's refusal, `PATCH` A to `MASK_NULL`, then create B → 201 (agreeing tie admitted).
3. `a_tag_assignment_that_puts_two_masked_tags_on_one_column_is_409` (Trino profile with a tag service) — tag rules on `pii` (`MASK`) and `gdpr` (`MASK_HASH`); assign `pii` to `orders.email` → 200; assign `gdpr` to the same column → **409** naming both rules and the column; `catalog_tag_assignments` holds only the first row.
4. `two_tag_rules_with_no_shared_column_are_admitted` — the two rules exist, `pii` on `email`, `gdpr` on `phone` → both assignments succeed. (The compile-time-only case: refused only when co-occurrence becomes real.)
5. `a_tag_rule_and_a_path_rule_on_one_tagged_column_are_409` (Trino) — path rule on `('sales','orders','email') MASK_NULL`, tag rule on `pii MASK`, `pii` assigned to `orders.email` → whichever write completes the pair (test both orders: rule-after-assignment and assignment-after-rule) is 409.
6. `the_same_pair_on_starrocks_is_caught_by_the_path_rule_alone` — StarRocks profile; tag rule expands per column (`rules.rs:5-14`), so the pair is two path policies → 409 (via Task 2's function).
7. ~~`a_preexisting_pair_wedges_unrelated_writes_and_its_removal_is_allowed`~~ — **superseded by the user ruling of 2026-10-12** (spec Part A, "Pre-existing overlaps: reported, not refused"): a stored pair no longer makes recompiles fail. As built (fix round 2): `a_stored_compiled_pair_does_not_block_unrelated_writes_but_an_edit_into_it_is_409` — a stored compiled pair, then an unrelated grant write → 201; an edit of one member into a different overlapping shape → 409; `DELETE` one → 204. Plus the compiled × hand-written cells with an unrelated rule and with a tag rule (installation-wide recompile) → 201.
8. `migration_of_a_policy_that_would_overlap_is_refused` — an SP4d-3 `POST …/migrate` whose produced rule overlaps an existing rule → the migration answers the overlap refusal and writes nothing (assert `perm.rules`, `perm.migrated_from`, and the original's `enabled` unchanged).
9. `different_exemptions_on_one_tagged_column_are_409` — tag ↔ path, same `MASK`, different exempt sets.

Update `recompile.rs`' `two_rules_masking_one_column_differently_refuse_the_whole_recompile` to match the variant's new fields (assert `rules.len() == 2`).

- [ ] **Step 3: Run, fail, implement, run**

```bash
df -h / | tail -1
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test overlap --test recompile --no-fail-fast > /tmp/sp4d4-t3.log 2>&1; echo "EXIT=$?"
grep -c '^error\[' /tmp/sp4d4-t3.log
grep 'test result:' /tmp/sp4d4-t3.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
```
Implementation: in `compile_desired`, after the existing call (keep it as the within-download check, now returning `ambiguous_arbitrations_in(...).first()`), load the profile's `TagAssignment`s on the same connection, build `CompiledForOverlap` from `desired` (rule names from `all_rules`, already loaded in that function), call `overlap::overlaps`, and return the first as `CompileError::AmbiguousMaskOrFilterArbitration`. In `routes/error.rs`, map that variant to **409** with the structured body (add a `PermHttpError::Overlap { body: serde_json::Value }` arm, or carry the JSON through `Conflict` — the body shape is what the tests pin). In `egret-bi`, make sure the recompile's error reaches the client as 409 with the same body (read `catalog_tags.rs`' error path; if `BiError` flattens it to 400, add the arm).

- [ ] **Step 4: Two mutations**

(a) Drop the tag ↔ path arm in `overlaps` (return only tag ↔ tag pairs). Expected: tests 5 and 9 fail by name, nothing else. (b) Make the refusal sentence use the policy name instead of the rule name. Expected: test 1 fails by name. Each: `count == 1`, compiles, restore with SHA-256.

- [ ] **Step 5: Regenerate the client, run the crates, commit**

```bash
cargo run --bin egret -- openapi > /tmp/sp4d4-openapi.json && cat /tmp/sp4d4-openapi.json > openapi.json
(cd frontend && npm run gen:api)
for c in egret-perm egret-perm-compile egret-bi; do
  DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    cargo test -p $c --no-fail-fast > /tmp/sp4d4-t3-$c.log 2>&1; echo "$c EXIT=$?"
  grep 'test result:' /tmp/sp4d4-t3-$c.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
done
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-server --lib
df -h / | tail -1
git add crates/egret-perm crates/egret-perm-compile crates/egret-bi openapi.json frontend/src/api/generated frontend/src/api/generated.digest.json
git commit -m "feat(perm-compile): refuse overlapping rules across tag and path, naming both rules"
```

---

## Task 4: report every overlap, and show it where rules are written

**Files:**
- Modify: `crates/egret-perm-compile/src/routes.rs` (new route in `perm_router`, `:114-140`; handler; `PermApi` paths)
- Modify: `crates/egret-perm-compile/src/trigger.rs` (a `pub(crate)` read-only "compile the desired set and run every overlap check, return all" entry, sharing `compile_desired`'s body — no second compile)
- Modify: `frontend/src/routes/governance/MaskFilterRules.tsx`
- Test: `crates/egret-perm-compile/tests/overlap.rs` (extend); `frontend/src/routes/governance/__tests__/MaskFilterRules.test.tsx` (create if absent, else extend)

**Interfaces:**
- Produces: `GET /api/control-plane/perm/overlaps` → `200 [{ serviceId, serviceName, kind, coordinate, rules: [{id, name}, {id, name}], sentence }]`, gated `policy:read` (the permission `list_rules` checks, `routes.rs:1388`). It **writes nothing** and takes no profile locks; each profile is compiled on a read-only connection.
- Consumes: Task 3's `overlap::overlaps`, Task 2's `ambiguous_arbitrations_in`.

- [ ] **Step 1: Failing tests**

Backend (`tests/overlap.rs`):
1. `the_report_lists_every_pair_and_writes_nothing` — two pre-existing pairs on two profiles (stored bypassing the checks, as Task 3's fix-round-2 test 7 does with `move_rule_column_unchecked`; note the report must list every pair PRESENT in each stored unit through `egret_policy::unit_overlaps`, not the compiler's created-only check, which by the 2026-10-12 ruling returns nothing for a stored pair) → the report lists both, each naming its profile; `policy.authz_policies` and every profile's `policy_version` are byte-identical before and after (compare the rows as `Value`s, not strings).
2. `the_report_is_empty_on_a_clean_profile` — with a control: the same fixture plus one pair is non-empty, so an always-empty implementation fails.
3. `the_report_needs_policy_read` — a session without it → 403.

Frontend (`MaskFilterRules.test.tsx`, real component, MSW or the repo's existing fetch mock — follow the file's neighbours):
1. With a non-empty report, a banner (role `alert` or `status`, whichever the repo's banners use — read `frontend/src/components` for the existing one and reuse it, FE-TEST-3) names both rules of each pair and links to each rule.
2. With an empty report, no banner.
3. A 409 from creating a rule shows the server's sentence beside the form (FE-ACTION-3, FE-COPY-2) and clears nothing.

- [ ] **Step 2: Run, fail, implement, run**

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test overlap --no-fail-fast > /tmp/sp4d4-t4.log 2>&1; echo "EXIT=$?"
grep 'test result:' /tmp/sp4d4-t4.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
cargo run --bin egret -- openapi > /tmp/sp4d4-openapi.json && cat /tmp/sp4d4-openapi.json > openapi.json
cd frontend && npm run gen:api && npx vitest run src/routes/governance/__tests__/MaskFilterRules.test.tsx; cd ..
```

- [ ] **Step 3: Mutation**

Make the report return only the first pair per profile. Expected: backend test 1 fails by name. Standard discipline.

- [ ] **Step 4: Frontend gate and commit**

```bash
cd frontend && npm run lint && npx tsc -b && npm run test -- --run > /tmp/sp4d4-t4-fe.log 2>&1; echo "EXIT=$?"
grep -E 'Tests +[0-9]+ (passed|failed)' /tmp/sp4d4-t4-fe.log; npm run build; cd ..
git add crates/egret-perm-compile openapi.json frontend/src/api/generated frontend/src/api/generated.digest.json \
        frontend/src/routes/governance/MaskFilterRules.tsx frontend/src/routes/governance/__tests__/MaskFilterRules.test.tsx
git commit -m "feat(governance): list every overlapping rule pair, and show it on the rules page"
```
Answer the frontend reviewer checklist in the report. If `npm run lint` fails with `ERR_MODULE_NOT_FOUND: eslint-plugin-jsx-a11y-x`, run `npm ci` first.

---

## Task 5: the gate probes only where a changed policy may be a candidate

**Files:**
- Modify: `crates/egret-ranger-eval/src/eval.rs` (export the predicate; `trie_excludes` at `:2826-2849`, `PolicyView::provably_a_candidate` at `:3288-3317`)
- Modify: `crates/egret-ranger-eval/src/lib.rs` (re-export if the crate re-exports eval items there)
- Modify: `crates/egret-perm-compile/src/equivalence.rs`
- Test: `crates/egret-ranger-eval/src/eval.rs` unit test for the predicate; `crates/egret-perm-compile/tests/equivalence.rs` (extend)

**Interfaces:**
- Produces, in `egret-ranger-eval`:

```rust
/// Whether Ranger's candidate list for `request` MAY contain `policy` —
/// `true` unless it provably does not. Resource policies: some resource map's
/// match type is not NONE, or the trie does not provably exclude it
/// (`trie_excludes`). This is the SAME predicate `evaluate` relies on at
/// offset 553; it is exported so callers do not restate it.
pub fn may_be_candidate(
    policy: &RangerPolicy,
    request: &RequestResource,
    service_def: &ServiceDef,
    policy_type: PolicyType,
) -> bool;
```

- Produces, in `equivalence.rs`:

```rust
pub enum PruneMode {
    /// Every probe `derive_probes` builds (SP4d-3 behaviour).
    Full,
    /// Only probes where some changed policy may be a candidate — unless a
    /// fallback condition holds, in which case this is `Full`.
    Pruned,
}
pub enum FallbackReason { EnvelopeDiffers, DenyAllElseInDelta, NonZeroPriorityInDelta }
pub struct ProbePlan { pub probes: Vec<Probe>, pub full_count: usize,
                       pub fallback: Option<FallbackReason> }
pub fn plan_probes(before: &ServicePolicySet, after: &ServicePolicySet,
                   service_def: &ServiceDef, assignments: &[TagAssignment],
                   mode: PruneMode) -> ProbePlan;
/// The count `plan_probes(..).probes.len()` would have, without building.
pub fn planned_probe_count(/* same args */) -> usize;
/// Δ: policies whose (name, body minus `id`/`version`) differ, or exist on one side only.
pub fn changed_policies<'a>(before: &'a ServicePolicySet, after: &'a ServicePolicySet)
    -> Vec<&'a RangerPolicy>;
```
  `derive_probes` and `probe_count` stay, unchanged, as the `Full` path (SP4d-3's tests pin them).
- Consumes: `egret_ranger_eval::eval::may_be_candidate`; for tag policies in Δ, a coordinate is kept when a probe's resolved tags (actual or hypothetical) include a name the policy's `resources.tag` values cover — reuse whatever `tag_candidates` uses for coverage (export it alongside if it is private; do not write a second glob matcher).

**The argument this implements** is the spec's Part D (lemma, theorem, fallback table). Put a short version in `plan_probes`' doc with a pointer to the spec, and list the three fallback conditions there.

**Equality for Δ.** Compare policies as `serde_json::Value` with `id` and `version` removed (`RangerPolicy` → `Value` through the same serde the crate already uses, or compare the parsed structs field-by-field if they derive `PartialEq` — read which). Never through `to_string()`.

**Envelope equality.** Everything in `ServicePolicySet` except `policies` and `tag_policies` (and their own `id`/`version` fields): compare as values.

- [ ] **Step 1: Failing tests**

In `eval.rs`' test block:
1. `may_be_candidate_is_true_for_an_ancestor_and_false_for_a_provably_absent_sibling` — a `{wh, sales}` policy vs request `{wh, sales, orders, email}` → `true` (ANCESTOR); vs `{wh, other, orders, email}` with plain literal values → `false`; a policy with `isExcludes: true` on `schema` vs the sibling → `true` (trie cannot prove absence).

In `tests/equivalence.rs`:
2. `pruned_and_full_find_the_same_disagreements` — for EVERY existing case in this file that builds a `ProfileCase` (wrap them in a helper rather than copying), run `compare` with `Full` and with `Pruned` probes and assert the two `Vec<Disagreement>` are equal as multisets (sort by a derived key first). Plus a seeded random corpus: 200 pairs from a generator over {access/mask/row-filter policies on 2-3 tables, 1-2 changed policies}, seed printed, same assertion.
3. `an_unchanged_interacting_deny_is_still_caught` — before: unchanged `D` (deny `alice` select on `wh.sales.orders.*`), `H` (allow `alice` select on `wh.sales.*.*`, priority 0); after: `D`, `G` = `H` with `policyPriority: 1`. Assert `compare` under `Pruned` reports a disagreement for `alice`/`select` at a `wh.sales.orders.…` coordinate, AND that `plan_probes` did NOT fall back (`fallback == None` is false here — G has a non-zero priority, so this case MUST fall back; assert `fallback == Some(NonZeroPriorityInDelta)`). Then the same shape with G differing from H by an added user instead of by priority → `fallback == None`, disagreement still reported.
4. `a_tag_interaction_is_still_caught_under_pruning` — SP4d-3's version-split shape (tag DENY plus a strictly higher-priority no-deny resource ALLOW; reuse the fixture from the existing per-version test): Δ = the resource ALLOW (changed), tag DENY unchanged; assert the disagreement at the tagged coordinate is reported on the same version it is reported on under `Full`.
5. `pruning_removes_probes` — a 20-policy profile with one changed policy: `pruned < full / 4`, and `full_count` equals `probe_count(...)`. (Without this a "pruning" that keeps everything passes 2–4.)
6. `an_envelope_change_falls_back` — change `serviceDef` between before and after → `fallback == Some(EnvelopeDiffers)` and `probes.len() == full_count`.
7. `an_id_only_change_is_not_in_delta` — the same policy with a different `id` → `changed_policies` is empty for it.
8. `planned_probe_count_matches_the_build` — for cases 3, 5, 6.

- [ ] **Step 2: Run, fail, implement, run**

```bash
df -h / | tail -1
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-ranger-eval --lib may_be_candidate --no-fail-fast > /tmp/sp4d4-t5-eval.log 2>&1; echo "EXIT=$?"
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test equivalence --no-fail-fast > /tmp/sp4d4-t5-eq.log 2>&1; echo "EXIT=$?"
for l in /tmp/sp4d4-t5-eval.log /tmp/sp4d4-t5-eq.log; do grep -c '^error\[' $l; grep 'test result:' $l | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'; done
```
Implementation: build the full coordinate set as `ProbeFactors::new` does today (it is O(N)), then filter coordinates by `∃ p ∈ Δ: may_be_candidate(p, coord, def, PolicyType::from_code(p.policy_type))` for resource policies — evaluate it for every `PolicyType` a Δ policy has, since a probe asks all three questions — and keep a (coordinate, tagging) pair for a tag policy in Δ when the tagging covers its tag. Principals and access types are not filtered.

- [ ] **Step 3: Two mutations**

(a) Make `may_be_candidate` ignore `trie_excludes` (return `any_match` only). Expected: test 1's `isExcludes` cell fails by name. (b) Make the fallback check skip the envelope comparison. Expected: test 6 fails by name. Standard discipline for each.

- [ ] **Step 4: Crates and commit**

```bash
for c in egret-ranger-eval egret-perm-compile; do
  DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    cargo test -p $c --no-fail-fast > /tmp/sp4d4-t5-$c.log 2>&1; echo "$c EXIT=$?"
  grep 'test result:' /tmp/sp4d4-t5-$c.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
done
cargo clippy -p egret-ranger-eval -p egret-perm-compile --no-deps --all-targets 2>&1 | tail -3
git add crates/egret-ranger-eval/src crates/egret-perm-compile/src/equivalence.rs crates/egret-perm-compile/tests/equivalence.rs
git commit -m "feat(perm-compile): probe only where a changed policy may be a candidate"
```

---

## Task 6: the migration uses pruning; the real engine confirms what was dropped; the budget is re-measured

**Files:**
- Modify: `crates/egret-perm-compile/src/migrate.rs` (the case builder and the cap check — `probe_count` call sites; `MAX_PROBE_EVALUATIONS` `:118-142`)
- Create: `crates/egret-perm-compile/tests/gate_timing.rs`
- Modify: `crates/egret-ranger-eval/tests/differential.rs` (new test beside `a_migration_the_grammar_accepts_decides_identically_on_the_real_engine`, `:7470`)
- Modify: `justfile` (`test-ranger-gates`)
- Modify: `crates/egret-perm-compile/src/routes.rs` / the migrate response DTO if it reports probe counts (add `probesFull` and `fallback` beside the existing count so the preview can show both)

**Interfaces:**
- Consumes: Task 5's `plan_probes`, `planned_probe_count`, `PruneMode::Pruned`.
- Produces: migrate response fields `probesFull: usize` and `pruningFallback: Option<String>` per profile; the cap check uses `planned_probe_count(.., Pruned)`.

- [ ] **Step 1: Wire it, and keep SP4d-3's tests green**

The migration builds every `ProfileCase` with `plan_probes(.., PruneMode::Pruned)`. `tests/migrate_route.rs` must stay green **unchanged**, except `a_profile_too_large_to_prove_is_refused_by_count_with_nothing_written` (`:842`), whose fixture may now fit: make its fixture large enough to exceed the cap **under pruning** (several changed policies, or a tag rule), and say in the test's doc why it changed. Do not raise the cap in this step.

```bash
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test migrate_route --no-fail-fast > /tmp/sp4d4-t6-mig.log 2>&1; echo "EXIT=$?"
grep 'test result:' /tmp/sp4d4-t6-mig.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
```

- [ ] **Step 2: The real engine on the dropped probes**

`probes_the_gate_prunes_decide_identically_on_the_real_engine`, gated on `EGRET_TEST_RANGER_E2E=1`. For each accepted pair the Task 8 corpus builds (reuse `mig_prepare`'s output: `MigPair { before, after, .. }`, `differential.rs:6913-6920`, both envelopes as `Value` — parse each with the same `ServicePolicySet` reader the gate uses, and pass `assignments = []`, so tags enter only through the gate's hypothetical tagging), compute `plan_probes(Full)` and `plan_probes(Pruned)` with the gate's own functions, and take the dropped probes (full minus pruned, as requests). Take a deterministic sample of **at most 4 000 per Ranger version** (stratified: every pair contributes, round-robin), turn each into two cases (`…-before` / `…-after`) exactly as `mig_case` does, send them through `ask_ranger` at **2.5.0 and 2.8.0** (both service-defs at both versions), and assert for every pair of verdicts: equal `isAllowed`, `isAccessDetermined`, `maskType`, `maskedValue`, `maskCondition`, `filterExpr`. Assert the dropped count is **non-zero** and print it, the sample size and the elapsed time. A disagreement is a Critical: the pruning dropped a probe the real engine decides differently — STOP and report it; do not tune the sample.

- [ ] **Step 3: The timing harness**

`tests/gate_timing.rs`, one `#[test] #[ignore]` that builds, for N ∈ {5, 20, 40, 80, 160}, a Trino profile of N single-user access policies on distinct tables (the shape `migrate.rs:123-133` measured), with k = 1 changed policy, and again with one tag mask added; for each prints N, full probe count, pruned probe count, and the wall time of `compare` over the pruned set on one version. Release only, and only with disk headroom:

```bash
df -h / | tail -1     # STOP if below 15 GiB free: a release build is a second configuration
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --release --test gate_timing -- --ignored --nocapture \
  > /tmp/sp4d4-t6-timing.log 2>&1; echo "EXIT=$?"
grep -E '^N=' /tmp/sp4d4-t6-timing.log
df -h / | tail -1
```
After measuring, delete the release artefacts this created if free space fell by more than 5 GiB (`rm -r target/release/deps/egret_perm_compile-* target/release/deps/gate_timing-*` — name them from `ls`, never a glob over `target/release`), and report both `df` numbers.

- [ ] **Step 4: Raise the cap only as far as the table allows**

The rule (spec Part D): the new cap is the largest measured pruned probe count whose `compare` time is **≤ 2.5 s**. If the table shows time per evaluation growing with N enough that a count cap no longer bounds time (time at the cap differs by more than 2× between the smallest and largest N that reach it), replace the unit with evaluations × (|before| + |after|) and say so. Rewrite `MAX_PROBE_EVALUATIONS`' doc with the new table (keep the SP4d-3 table above it, labelled as the unpruned measurement).

- [ ] **Step 5: Gates and commit**

Add the Step 2 test to `test-ranger-gates` (literal `EGRET_TEST_RANGER_E2E=1`, `--exact`, the `--list … | grep -qx` guard, and the `--skip` on the general line).

```bash
just test-ranger-gates > /tmp/sp4d4-t6-gates.log 2>&1; echo "EXIT=$?"
grep -E 'finished in|test result:' /tmp/sp4d4-t6-gates.log
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --no-fail-fast > /tmp/sp4d4-t6-crate.log 2>&1; echo "EXIT=$?"
grep 'test result:' /tmp/sp4d4-t6-crate.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
cargo run --bin egret -- openapi > /tmp/sp4d4-openapi.json && cat /tmp/sp4d4-openapi.json > openapi.json
(cd frontend && npm run gen:api)
git add crates/egret-perm-compile crates/egret-ranger-eval/tests/differential.rs justfile openapi.json frontend/src/api/generated frontend/src/api/generated.digest.json
git commit -m "feat(perm-compile): prove migrations on pruned probes, confirmed against the real engines"
```
Report: the `a_migration_the_grammar_accepts…` gate's disagreement count (must still be 0), the new gate's dropped/sampled counts, every elapsed time, and the timing table.

---

## Task 7: an external engine is a row egret can hold, register and bind

**Files:**
- Create: `migrations/0184_engine_external.sql` (**re-check the number first**)
- Create: `crates/egret-engine-mgmt/src/external.rs`; declare in `crates/egret-engine-mgmt/src/lib.rs`
- Modify: `crates/egret-engine-mgmt/src/store.rs` (`Engine` `:27-47` gains fields; `get_engine`/`list_engines` select them; `coordinator_host`/`coordinator_port` `:64-81` answer the external address for an external row)
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (router `:94-125`; `get_engine`/`list_engines` DTOs; reuse `apply_authz_service_update` `:4990`)
- Modify: `crates/egret-engine-mgmt/src/catalog_apply.rs` (`attach_to_engine` `:895` → declared for external)
- Modify: `crates/egret-perm-compile/src/migrate.rs` (`bound_plugin_versions` `:377-411`) and `crates/egret-perm-compile/src/routes.rs` (`explain`, `:1832-1836`)
- Test: `crates/egret-engine-mgmt/tests/routes.rs` (new section, reuse `setup_with_ranger_and_compiler`, `session_for`, `req`, `into_json`, `FakeEnginePort`); `crates/egret-engine-mgmt/tests/registry_schema_constraints.rs` (the CHECKs); `crates/egret-perm-compile/tests/migrate_route.rs` and `tests/routes.rs` (plugin version)

**Interfaces:**
- Produces (SQL):

```sql
-- 0184_engine_external.sql  (number re-checked against origin/main at creation)
--
-- SP4d-4 Part C: an engine egret did not provision and never touches, held so
-- a profile can be bound to it and catalogs declared attached to it. Before
-- this, SP4d-3's live check had to INSERT such a row and its attachment by
-- hand. The CHECK makes the external shape unrepresentable any other way: an
-- external row carries an address and a declared Ranger plugin version and
-- NONE of the provisioning state (namespace, spec, secrets, certs) egret holds
-- for clusters it runs; a provisioned row carries none of the external fields.
-- `status` is deliberately not widened: every reader shows `origin` instead.
-- `'declared'` attachments carry no client and no credential. That a declared
-- attachment belongs to an external engine is a cross-table fact a CHECK
-- cannot express; `egret_engine_mgmt::external` is its one writer and a test
-- asserts it across the whole table.
ALTER TABLE engine_mgmt.engines
    ADD COLUMN origin text NOT NULL DEFAULT 'provisioned',
    ADD COLUMN external_host text,
    ADD COLUMN external_port integer,
    ADD COLUMN ranger_plugin_version text,
    ADD CONSTRAINT engines_origin_check CHECK (origin IN ('provisioned', 'external')),
    ADD CONSTRAINT engines_external_shape CHECK (
        CASE origin
        WHEN 'external' THEN
            external_host IS NOT NULL AND external_host <> ''
            AND external_port BETWEEN 1 AND 65535
            AND ranger_plugin_version ~ '^[0-9]+\.[0-9]+\.[0-9]+$'
            AND namespace = '' AND spec = '{}'::jsonb
            AND delegator_secret_id IS NULL AND tls_secret_id IS NULL
            AND coordinator_cert IS NULL AND root_secret_id IS NULL
            AND tls_cert_secret_ref IS NULL AND internal_secret_id IS NULL
            AND monitor_secret_id IS NULL
        ELSE
            external_host IS NULL AND external_port IS NULL
            AND ranger_plugin_version IS NULL
        END
    );

ALTER TABLE registry.engine_catalog_attachments
    DROP CONSTRAINT engine_catalog_attachments_apply_status_check,
    ADD CONSTRAINT engine_catalog_attachments_apply_status_check
        CHECK (apply_status IN ('applied', 'failed', 'pending', 'declared')),
    ADD CONSTRAINT attachments_declared_holds_nothing CHECK (
        apply_status <> 'declared'
        OR (catalog_client_id IS NULL AND credential_secret_id IS NULL
            AND NOT minted_client AND NOT is_primary)
    );
```
  Before writing the `DROP CONSTRAINT`, confirm the constraint's real name on `egret_test`: `psql "$DATABASE_URL" -X -c '\d registry.engine_catalog_attachments'` (it was `engine_catalog_attachments_apply_status_check` on 2026-10-11).

- Produces (Rust, `external.rs`):

```rust
pub const ORIGIN_EXTERNAL: &str = "external";
pub fn is_external(e: &crate::Engine) -> bool;
/// The 409 every lifecycle handler returns for an external engine (Task 8).
pub fn refuse_if_external(e: &crate::Engine, action: &str) -> Result<(), crate::EngineHttpError>;

#[derive(Deserialize, ToSchema)] #[serde(rename_all = "camelCase")]
pub struct RegisterExternalEngine {
    pub name: String, pub r#type: String /* trino|starrocks */,
    pub host: String, pub port: u16, pub ranger_plugin_version: String,
    pub authz_service_id: Option<i64>, #[serde(default)] pub catalogs: Vec<Uuid>,
}
// POST /api/control-plane/engines/external   (engine:create) → 201 EngineDto
```
  `EngineDto` (and the list row) gain `origin`, `host`, `port`, `rangerPluginVersion`.

- Produces (perm-compile): `pub(crate) fn engine_plugin_version(origin: &str, r#type: &str, declared: Option<&str>) -> Result<PluginVersion, String>` — provisioned → today's type mapping; external → `"2.5.0"` → `Ranger2_5_0`, `"2.8.0"` → `Ranger2_8_0`, any other → `Err("engine \"{name}\" declares Ranger plugin {v}, which egret's evaluator has not been verified against …")`. `bound_plugin_versions` selects `type, origin, ranger_plugin_version` and calls it; `explain` uses the bound engine's version when exactly one engine is bound (today's type mapping when none), and answers a refusal sentence when the declared version is unmeasured.

**Registration, in one transaction then one recompile:**
1. Validate name (the existing engine-name rule — read `create_engine` for it), type, host (non-empty, no scheme, no path), port, version format.
2. Insert the row with `origin = 'external'`, `namespace = ''`, `spec = '{}'`, `status = 'running'`.
3. If `authzServiceId`: run `apply_authz_service_update`'s checks (type match → 409, bound elsewhere → 409, Trino needs a usable `tagService` → its existing refusal) — refactor its check half into a function both paths call rather than duplicating it.
4. Each catalog id: must exist in `registry.catalogs` (404 naming it otherwise); insert `apply_status = 'declared'`, `minted_client = false`, `is_primary = false`, no client, no credential.
5. Commit, then recompile the bound profile through `st.perm_compiler` (`ProfileCompilerPort`) exactly as attach does today; a recompile failure is returned (the row stays; the response says so) — read how `attach_catalog` handles the same and match it.

- [ ] **Step 1: Write the migration and its constraint tests first**

In `tests/registry_schema_constraints.rs`, add: an external row missing `ranger_plugin_version` is refused by `engines_external_shape`; an external row with a `root_secret_id` is refused; a provisioned row with `external_host` is refused; a `'declared'` attachment with `minted_client = true` is refused by `attachments_declared_holds_nothing`; a well-formed external row and declared attachment are accepted. Each refusal asserts the **constraint name** in the error, not just "an error".

```bash
git fetch origin && git ls-tree --name-only origin/main migrations/ | sort | tail -3
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-engine-mgmt --test registry_schema_constraints --no-fail-fast > /tmp/sp4d4-t7-schema.log 2>&1; echo "EXIT=$?"
grep 'test result:' /tmp/sp4d4-t7-schema.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
```

- [ ] **Step 2: Failing route tests** (`tests/routes.rs`, new section "external engines")

1. `registering_an_external_starrocks_binds_it_and_declares_its_catalogs` — `POST /engines/external` with a StarRocks profile and one registry catalog → 201; the row has `origin = external`; the attachment is `declared`; **`FakeEnginePort`'s `provisioned`, `updated`, `deleted`, `restarted` are all empty**; the profile's download (`GET /service/plugins/policies/download/{service}`) now carries a grant compiled for that catalog when one exists (seed a grant on the catalog first).
2. `registering_against_a_profile_of_the_other_type_is_409` — and nothing is written (row count unchanged).
3. `registering_against_a_profile_another_engine_binds_is_409`.
4. `an_unknown_catalog_is_404_naming_it_and_writes_nothing`.
5. `get_engine_never_asks_the_provisioner_for_an_external_engine` — add a `status_calls` counter to `FakeEnginePort` (additive) and assert it stays 0; the DTO says `origin: "external"`.
6. `the_whole_table_agrees_declared_means_external` — after the tests above, `SELECT count(*) FROM registry.engine_catalog_attachments a JOIN engine_mgmt.engines e ON e.id = a.engine_id WHERE (a.apply_status = 'declared') <> (e.origin = 'external')` scoped to this test's engine ids is 0, **and** the unscoped count is reported in the test output (not asserted — the DB is shared).

In `crates/egret-perm-compile/tests/migrate_route.rs`:
7. `an_external_engine_declaring_an_unmeasured_plugin_version_refuses_the_migration` — bind an external engine declaring `2.6.0` (insert the row directly, the shape the CHECK allows); migrate → 422 whose sentence names `2.6.0`.
8. `an_external_trino_declaring_2_8_0_is_proved_on_2_8_0` — the response's `plugin_versions` is `[2.8.0]`, not `[2.5.0]`.

- [ ] **Step 3: Run, fail, implement, run**

```bash
df -h / | tail -1
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-engine-mgmt --test routes --no-fail-fast -- external > /tmp/sp4d4-t7-routes.log 2>&1; echo "EXIT=$?"
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-perm-compile --test migrate_route --no-fail-fast > /tmp/sp4d4-t7-mig.log 2>&1; echo "EXIT=$?"
for l in /tmp/sp4d4-t7-routes.log /tmp/sp4d4-t7-mig.log; do grep -c '^error\[' $l; grep 'test result:' $l | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'; done
```

- [ ] **Step 4: Mutation**

In `engine_plugin_version`, map every external engine by its type (ignore the declared version). Expected: tests 7 and 8 fail by name. Standard discipline.

- [ ] **Step 5: Crates, client, commit**

```bash
for c in egret-engine-mgmt egret-perm-compile; do
  DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    cargo test -p $c --no-fail-fast > /tmp/sp4d4-t7-$c.log 2>&1; echo "$c EXIT=$?"
  grep 'test result:' /tmp/sp4d4-t7-$c.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
done
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test cargo test -p egret-server --lib
cargo run --bin egret -- openapi > /tmp/sp4d4-openapi.json && cat /tmp/sp4d4-openapi.json > openapi.json
(cd frontend && npm run gen:api)
df -h / | tail -1
git add migrations/0184_engine_external.sql crates/egret-engine-mgmt crates/egret-perm-compile openapi.json \
        frontend/src/api/generated frontend/src/api/generated.digest.json
git commit -m "feat(engine-mgmt): register an external engine, bind it to a profile, declare its catalogs"
```

---

## Task 8: every lifecycle operation refuses, or no-ops, for an external engine

**Files:**
- Modify: `crates/egret-engine-mgmt/src/engines.rs`, `catalogs.rs`, `catalog_apply.rs`, `monitor.rs`, `assignment.rs`
- Test: `crates/egret-engine-mgmt/tests/routes.rs` (the "external engines" section)

**Interfaces:**
- Consumes: Task 7's `external::refuse_if_external(engine, action)`, which returns `EngineHttpError::Conflict` with body `{ "error": "<engine> is an external engine: egret does not <action> it", "code": "engine_is_external" }` (add the `code` through whatever mechanism `EngineHttpError` already uses for coded 409s — read `http_error.rs`; `EngineConflictBody` exists).

**Step 0 is the enumeration, and it is the task's spine.** A missed handler is a 500, or a provisioner call against a cluster egret does not own (`delete` would call `prov.delete(type, "", name)`).

- [ ] **Step 0: Enumerate by code, and put the list in the report**

```bash
rg -n 'engine_provisioner|\.kube\b|starrocks_ddl\.|coordinator_host\(\)|coordinator_port\(\)' \
   crates/egret-engine-mgmt/src --glob '!**/tests/**' | rg -v '^\S+:\s*//'
rg -n '^\s*\.route\(' crates/egret-engine-mgmt/src
```
For every handler reached, decide one of: **refuse** (409 `engine_is_external`), **no-op** (skip the external engine, e.g. a converge loop over attachments), or **external branch** (does the external thing). The expected table — confirm or correct it from the `rg` output, never silently:

| Route / function | External behaviour |
|---|---|
| `PUT /engines/{id}` (`update_engine`) | accept only `authzServiceId`, `host`, `port`, `rangerPluginVersion`; anything else 409; never `prov.update` |
| `DELETE /engines/{id}` | dependents preflight; NO Kubernetes requirement; no `prov.delete`; delete row; no secret cleanup beyond what the row references (none) |
| `POST /engines/{id}/restart`, `/rotate-secrets` | refuse |
| `GET /engines/{id}/pods/{pod}/logs`, `/logs/follow`, `/exec` | refuse |
| monitor routes (`monitor.rs:134-140`) | refuse |
| `POST /engines/{id}/catalogs/{cid}` | declared attachment + recompile (Task 7's writer) |
| detach (`catalog_apply.rs::detach_from_engine` `:1153`) | delete the declared row + recompile; no DDL, no render |
| `POST /engines/{id}/catalogs/{cid}/test` | refuse |
| `POST /catalogs/{id}/reapply`, `converge_*` over attachments | skip `declared` attachments (no-op), and say so in the response's per-engine list |
| workspace assignment (`/engines/{id}/workspaces`, `/workspaces/{id}/engines`) | refuse (out of scope: no credentials for an external engine) |
| `get_engine` / `list_engines` | never call `prov.status`; DTO phase from `origin` |

- [ ] **Step 1: Failing tests** — one per row of the table, each asserting the status, the `code`, and that `FakeEnginePort` recorded **no** call; for the converge/reapply row, that a provisioned engine attached to the same catalog IS converged while the external one is skipped (so a test that skips everything fails); for `DELETE`, with **no** provisioner configured (`engine_provisioner: None`), the external row is deleted (204) while a provisioned engine's delete still answers today's 400 "kubernetes not configured".

- [ ] **Step 2: Run, fail, implement, run**

```bash
df -h / | tail -1
DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
  cargo test -p egret-engine-mgmt --no-fail-fast > /tmp/sp4d4-t8.log 2>&1; echo "EXIT=$?"
grep -c '^error\[' /tmp/sp4d4-t8.log
grep 'test result:' /tmp/sp4d4-t8.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
```
Put `refuse_if_external` as the FIRST statement after the permission check and the row load in each refusing handler, so no partial work happens first.

- [ ] **Step 3: Mutation**

Remove the `refuse_if_external` call from `delete_engine`'s path (so it falls through to `prov.delete`). Expected: the DELETE test fails by name (the fake records a delete, or the 400 appears). Standard discipline.

- [ ] **Step 4: Re-run Step 0's `rg` and diff it against the table in your report; commit**

```bash
cargo clippy -p egret-engine-mgmt --no-deps --all-targets 2>&1 | tail -3
cargo run --bin egret -- openapi > /tmp/sp4d4-openapi.json && cat /tmp/sp4d4-openapi.json > openapi.json
(cd frontend && npm run gen:api)
git add crates/egret-engine-mgmt openapi.json frontend/src/api/generated frontend/src/api/generated.digest.json
git commit -m "feat(engine-mgmt): an external engine is never provisioned, restarted, exec'd or deleted by egret"
```

---

## Task 9: the Ranger plugin configuration an admin installs, from the one renderer

**Files:**
- Create: `crates/egret-provisioner/src/ranger_config.rs`; declare in `crates/egret-provisioner/src/lib.rs`
- Modify: `crates/egret-provisioner/src/trino.rs:348-370` and `starrocks.rs:185-215` (call the renderer; the `fe.conf` line at `starrocks.rs:121-123`)
- Modify: `crates/egret-engine-mgmt/src/engines.rs` (new route) — and `EngineMgmtState` if it needs the Ranger auth-mode flag (read where `RangerState.auth_token` is built in `crates/egret-bin/src/main.rs` and pass a boolean "a token is configured", never the token)
- Test: `crates/egret-provisioner/src/ranger_config.rs` unit tests; the existing provisioner tests that assert the XML (`trino.rs:1375-1380`, `starrocks.rs:1373-1376`) stay green **unchanged**; `crates/egret-engine-mgmt/tests/routes.rs`

**Interfaces:**
- Produces:

```rust
pub struct RangerPluginFiles { pub files: Vec<(String /*name*/, String /*content*/)> }
/// The exact files egret mounts into its own engines, for `kind` ∈ {trino, starrocks}.
pub fn ranger_plugin_files(kind: EngineKind, service_name: &str, egret_url: &str,
                           poll_interval_ms: u64) -> RangerPluginFiles;
```
  Trino → `access-control.properties`, `ranger-trino-security.xml`. StarRocks → `fe.conf (add this line)` containing `access_control = ranger`, `ranger-starrocks-security.xml`, `ranger-starrocks-audit.xml`.
  ⚠ StarRocks: **no XML comment may contain `--`** — it crashes the FE boot (`starrocks.rs:187-190`). The renderer emits no comments at all.
- Produces: `GET /api/control-plane/engines/{id}/ranger-plugin-config` (`engine:read`) → `{ serviceName, downloadUrl, authMode: "anonymous" | "bearer-configured", warnings: [..], assumptions: [..], files: [{name, content}] }`. 409 for a provisioned engine (egret mounts its files itself); 409 when the engine is bound to no profile ("bind a profile first").
  `downloadUrl` = `{public_url}/service/plugins/policies/download/{serviceName}` with `public_url` = the `engine_egret_url` already on the state (`config.catalog.public_url`, default `http://egret:8080`, `crates/egret-bin/src/main.rs:1978-1985`). `warnings` include: the URL is the in-cluster default or contains `.svc` ("an engine outside the cluster cannot reach it; set `[catalog] public_url`"); `authMode` is `bearer-configured` ("a stock Ranger plugin cannot send egret's bearer token"); `authMode` is `anonymous` ("anyone who can reach this URL can download every profile's policies"). `assumptions` = the three sentences from the spec (no super-users, no group provider/usersync, audit off) — the same strings `explain` will cite.

- [ ] **Step 1: Failing tests**

1. `the_renderer_reproduces_what_egret_mounts` — for both kinds, the file contents `ranger_plugin_files` returns equal the strings the provisioner's ConfigMap carries today for the same inputs (build a minimal `EngineSpec` as the existing provisioner tests do and compare the ConfigMap's `data` values) — `Value ==` on the ConfigMap, string equality on file contents.
2. `no_starrocks_file_contains_a_double_hyphen`.
3. Route: an external StarRocks bound to `sr-prof` → 200, `serviceName == "sr-prof"`, `downloadUrl` ends with `/service/plugins/policies/download/sr-prof`, the default-URL warning is present, `files` names are exactly the three StarRocks names; a provisioned engine → 409; an unbound external engine → 409.

- [ ] **Step 2: Run, fail, implement, run; then the provisioner tests unchanged**

```bash
df -h / | tail -1
for c in egret-provisioner egret-engine-mgmt; do
  DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    cargo test -p $c --no-fail-fast > /tmp/sp4d4-t9-$c.log 2>&1; echo "$c EXIT=$?"
  grep -c '^error\[' /tmp/sp4d4-t9-$c.log
  grep 'test result:' /tmp/sp4d4-t9-$c.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
done
```

- [ ] **Step 3: Mutation, commit**

Mutation: change the StarRocks poll property name in the renderer. Expected: test 1 fails by name (and the provisioner's own XML test). Then:

```bash
cargo run --bin egret -- openapi > /tmp/sp4d4-openapi.json && cat /tmp/sp4d4-openapi.json > openapi.json
(cd frontend && npm run gen:api)
git add crates/egret-provisioner crates/egret-engine-mgmt crates/egret-bin openapi.json frontend/src/api/generated frontend/src/api/generated.digest.json
git commit -m "feat(engine-mgmt): show the Ranger plugin files an external engine must install"
```

---

## Task 10: the Control Plane pages for an external engine

**Files:**
- Create: `frontend/src/routes/control-plane/EngineRegister.tsx`, `frontend/src/routes/control-plane/__tests__/EngineRegister.test.tsx`, `frontend/src/routes/control-plane/EnginePluginConfig.tsx`
- Modify: `frontend/src/App.tsx` (`:437-439` — add `engines/register` **before** `engines/:id`, behind `RequirePermission perm="engine:create"`), `frontend/src/routes/control-plane/ControlPlaneEngines.tsx` (a "Register external engine" link beside "New engine", and an `External` chip in the list), `frontend/src/routes/control-plane/EngineDetail.tsx` (external branch)
- Test: `frontend/src/routes/control-plane/__tests__/EngineRegister.test.tsx`; extend the existing `EngineDetail`/`ControlPlaneEngines` tests under `frontend/src/routes/control-plane/__tests__/`

**Interfaces:**
- Consumes: the generated client for `POST /engines/external`, `GET /engines/{id}` (with `origin`), `GET /engines/{id}/ranger-plugin-config`, the existing profile list and registry catalog list hooks (read `EngineCreate.tsx` for how it lists profiles and catalogs, and reuse — FE-DATA-2).

Rules that bind this task (from `docs/en/frontend-rules.md`): FE-PAGE-3 (register is a page: several sections), FE-PAGE-2 (the detail's tabs in the URL), FE-FORM-1..4 (hand-written state, tied labels, pickers for the type, the plugin version — a select of `2.5.0`, `2.8.0` and "Other…" with a free field — the profile and the catalogs), FE-ACTION-3/4 (server message on failure; a disabled control says why), FE-COPY-1/2, FE-STATE-1..3, FE-A11Y-*, FE-TEST-1..4.

The register page sections: Engine (name, type, host, port), Ranger (plugin version; a note "egret proves migrations and explains access only for 2.5.0 and 2.8.0; any other version is recorded and those features refuse on this engine"), Policy profile (a picker filtered to the chosen type), Catalogs (a multi-picker from the registry; a note "egret does not create these catalogs on your engine — create them with the same names"). After 201, navigate to the detail's **Plugin config** tab.

The detail page for `origin === "external"`: an `External` chip where status would be (FE-STATE-4: not a green dot — egret does not monitor it), host:port, plugin version, the bound profile (link), declared catalogs with their names; tabs: Overview, Catalogs, Plugin config. **Hidden** (not disabled): pods, logs, terminal, monitor, autoscaling, every config editor, rotate secrets — their routes refuse. Delete stays, with the existing confirm dialog, its copy saying "egret removes its record; your cluster is not touched."

The Plugin config tab: service name, download URL, auth mode, every warning (as a warning callout, not colour alone — FE-A11Y-4), the assumptions list, and one block per file with a Download button (reuse `TrinoConfigDialog.tsx`'s `download` helper by moving it to a shared module — FE-TEST-3, additive).

- [ ] **Step 1: Failing tests**

`EngineRegister.test.tsx`:
1. The profile picker lists only profiles of the chosen type; switching type clears an incompatible selection.
2. Submit posts exactly the fields the API takes (assert the request body), and on 201 navigates to `/control-plane/settings/engines/<id>?tab=plugin-config` (assert the route, not a mocked navigate call — FE-TEST-1).
3. A 409 shows the server's sentence beside the form and keeps every field's value (FE-ACTION-3).
4. Submit is disabled with a visible reason until name, host, port and version are set (FE-ACTION-4) — assert on the **button element**, not an ancestor (`toBeDisabled` walks up to a disabled `fieldset`).

`EngineDetail` (extend):
5. An external engine renders the `External` chip, no pods/logs/monitor tabs, and the Plugin config tab with every warning text the API returned.
6. A provisioned engine still renders today's tabs (control).

- [ ] **Step 2: Run, fail, implement, run**

```bash
cd frontend
npx vitest run src/routes/control-plane/__tests__/EngineRegister.test.tsx src/routes/control-plane/__tests__/EngineDetail*.test.tsx
```

- [ ] **Step 3: The gate, a browser check, commit**

```bash
npm run lint && npx tsc -b && npm run test -- --run > /tmp/sp4d4-t10-fe.log 2>&1; echo "EXIT=$?"
grep -E 'Tests +[0-9]+ (passed|failed)' /tmp/sp4d4-t10-fe.log
npm run build
cd ..
git add frontend/src
git commit -m "feat(control-plane): register an external engine and read its Ranger plugin config"
```
FE-TEST-2: the layout of the register page and the plugin-config blocks is checked in a real browser in Task 13 (the stack is not up here); say so in the report rather than claiming it.

---

## Task 11: the editor's absent levels — backend, per the user's branch

**Precondition:** the ledger records the user's answer to Task 1 ("B1" or "B2"). Do the steps for that branch only; the "both branches" steps always.

**Files:**
- Modify: the editor's form-to-body function `editor_to_ranger_json` (`crates/egret-policy/src/admin/ranger_json.rs:305`) — NOT `resources_object` (`:83-91`), which the compiler's serializers share — and its callers in `crates/egret-policy/src/admin/mod.rs` (create/update)
- Modify: `crates/egret-perm-compile/src/classify.rs` (per-policy `warnings`; the `MissingLevel` sentence `:395-410`)
- Modify (B2 only): `crates/egret-perm-compile/src/grammar.rs` (`:740-760`, the absent-level rule); the comments that state the opposite in `crates/egret-ranger-eval/src/resource.rs:16-31` only if Task 1 measured them wrong
- Test: `crates/egret-policy/tests/admin_routes.rs`; `crates/egret-perm-compile/tests/classify_route.rs`; (B2) `crates/egret-perm-compile/tests/grammar_access.rs` (`an_absent_level_is_refused_but_a_star_level_is_accepted`, `:340-363`)

**Interfaces:**
- Produces: classify response, per policy, `warnings: [{ kind: "select_stops_above_column", level } | { kind: "level_gap", missing }]` with a `sentence` each; the profile-level response carries `warningCount`.
- ⚠ `resources_object` is ALSO used by the compiler's serializers (`masking_to_ranger_json`, `rowfilter_to_ranger_json`, `access_to_ranger_json`). The editor-only behaviour must not change a compiled body: put the empty-level handling in the **editor** path (before the serializer), not in `resources_object`. Assert it: test 5 below.

**Both branches:**
- A body with a **gap** (a named level below an absent one, along the engine's hierarchy) is refused on editor save with 400 naming the missing level. Existing gap rows get the `level_gap` warning.

**B1:**
- An editor save with an EMPTY **root** level (catalog) → 400 "name a catalog — `*` here would cover every catalog this profile serves".
- An editor save with an EMPTY non-root level → that level is stored as `{"values": ["*"], "isExcludes": false, "isRecursive": false}`.
- A level **absent** from the form (removed via "— none —") stays absent.
- Classifier warning `select_stops_above_column` for an access policy (type 0) in a resource service whose chain stops above `column` and whose `policyItems` allow `select` (directly, or via an access type whose `impliedGrants` include `select` — read the service-def's `impliedGrants` with the evaluator's `implied_access_grants`, do not restate it).
- The `MissingLevel` sentence: drop "this is how most editor-saved policies look"; say the policy "stops above `{level}`, so Ranger does not apply it to `{level}`-level requests (measured: docs/en/plans/2026-10-11-sp4d-4-task1-levels-result.md)", and point at the warning.

**B2:**
- The editor is unchanged except the gap refusal.
- The grammar accepts an absent **trailing** level as `*` when Task 1's table shows identical decisions for every access type and depth on both versions; `an_absent_level_is_refused_but_a_star_level_is_accepted` becomes `an_absent_trailing_level_reads_as_star` and its doc cites Task 1. Correct `grammar.rs:43-48` and the `MissingLevel` sentence accordingly.

- [ ] **Step 1: Failing tests** (`admin_routes.rs` + `classify_route.rs`)

Both: 1. `an_editor_body_with_a_level_gap_is_refused` (400, names the level, nothing stored). 2. `a_stored_gap_row_is_warned` (insert directly, classify lists `level_gap`).
B1: 3. `an_empty_catalog_is_refused`. 4. `an_empty_non_root_level_is_stored_as_star` (read the stored body back; compare the level as a `Value`). 5. `a_compiled_body_is_unchanged` (compile a grant before and after this change in the same test via `compile_grant`; `Value ==`). 6. `a_truncated_chain_stays_truncated` (no `column` key). 7. `select_above_column_is_warned_and_insert_above_column_is_not` (two stored policies; only the `select` one is warned — the control). 8. `an_existing_row_is_never_rewritten` (classify, then re-read `policy_jsonb` — unchanged).
B2: 3'. `an_absent_trailing_level_reads_as_star` in `grammar_access.rs` and its compiled-back form decides identically (reuse `every_accepted_body_compiles_back_to_what_it_came_from`).

- [ ] **Step 2: Run, fail, implement, run**

```bash
df -h / | tail -1
for t in "egret-policy --test admin_routes" "egret-perm-compile --test classify_route" "egret-perm-compile --test grammar_access"; do
  DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    cargo test -p $t --no-fail-fast > /tmp/sp4d4-t11-$(echo $t | tr ' ' '_').log 2>&1; echo "$t EXIT=$?"
done
grep -h 'test result:' /tmp/sp4d4-t11-*.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
```

- [ ] **Step 3: Mutation**

B1: make the editor path also turn an empty **root** level into `*`. Expected: test 3 fails by name. B2: make the grammar accept a missing **middle** level. Expected: the gap test fails by name.

- [ ] **Step 4: Crates, client, commit**

```bash
for c in egret-policy egret-perm-compile; do
  DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test \
    cargo test -p $c --no-fail-fast > /tmp/sp4d4-t11-$c.log 2>&1; echo "$c EXIT=$?"
  grep 'test result:' /tmp/sp4d4-t11-$c.log | awk '{p+=$4; f+=$6; n++} END {print "suites="n" passed="p" failed="f}'
done
cargo run --bin egret -- openapi > /tmp/sp4d4-openapi.json && cat /tmp/sp4d4-openapi.json > openapi.json
(cd frontend && npm run gen:api)
git add crates/egret-policy crates/egret-perm-compile openapi.json frontend/src/api/generated frontend/src/api/generated.digest.json
git commit -m "fix(policy): an editor-saved level says what Ranger will enforce"   # B1
# or: "fix(perm-compile): accept an absent trailing level the engines read as *"   # B2
```

---

## Task 12: the editor's absent levels — what the operator sees

**Precondition:** Task 11 merged on the branch; the ledger records B1 or B2.

**Files:**
- Modify: `frontend/src/components/policy/ResourceLevelEditor.tsx`, `frontend/src/components/policy/ResourceSelector.tsx` (additive props — FE-TEST-3: measure the blast radius by import path, `rg -n "components/policy/Resource(Selector|LevelEditor)" frontend/src`)
- Modify: `frontend/src/routes/governance/PolicyEditorPage.tsx` (pass the access types in force to the selector for the warning), `frontend/src/routes/governance/ProfileDetailPage.tsx` (the flagged list)
- Test: `frontend/src/components/policy/__tests__/ResourceLevelEditor.test.tsx`, `__tests__/ResourceSelector.test.tsx`, `frontend/src/routes/governance/__tests__/ProfileDetailPage.test.tsx` (extend or create)

Both branches:
- When the chain has a gap the editor cannot save, the Save button is disabled with the reason (FE-ACTION-4).
- `ProfileDetailPage` shows, when `warningCount > 0`, one callout with the count and a list of the flagged policies, each with its sentence and a link to the editor (FE-COPY-2: what happened and what to do).

B1 only:
- Under an EMPTY non-root level, a hint tied to the field (`aria-describedby`, FE-FORM-2): "Empty means every {level} — saved as *."
- Under an EMPTY root level: the existing invalid state, with "Name a catalog."
- When the chain stops above `column` and the selected access types include `select` (or `all`), a warning beside the chain: "Stops above column: grants no column reads, only table-level checks such as `count(*)`." Shown, not blocking.

B2 only: no hint; the gap refusal and the flagged list only.

- [ ] **Step 1: Failing tests** — each message appears in exactly the state above and not in its control state (an empty non-root level vs a `*` level; a `select` chain stopping at table vs an `insert`-only chain stopping at table; a profile with `warningCount: 0` shows no callout).
- [ ] **Step 2: Run, fail, implement, run, gate**

```bash
cd frontend
npx vitest run src/components/policy/__tests__/ResourceLevelEditor.test.tsx src/components/policy/__tests__/ResourceSelector.test.tsx src/routes/governance/__tests__/ProfileDetailPage.test.tsx
npm run lint && npx tsc -b && npm run test -- --run > /tmp/sp4d4-t12-fe.log 2>&1; echo "EXIT=$?"
grep -E 'Tests +[0-9]+ (passed|failed)' /tmp/sp4d4-t12-fe.log
npm run build
cd ..
git add frontend/src
git commit -m "feat(governance): the policy editor says what an empty or missing level will enforce"
```

---

## Task 13: the live check — A, C and D on real engines, and B in the browser

**Files:** none committed except the report `docs/en/plans/2026-10-11-sp4d-4-live-result.md`.

In this programme the browser found defects on pages carrying dozens of green tests, and only real engines found the defects that mattered. This is a task, not an afterthought.

- [ ] **Step 1: Bring up the stack on a throwaway database**

Follow the SP4d-3 live check's recipe (a throwaway DB, `just up` from `deploy/docker-compose.dev.yml`, `valkey` and `rustfs` both up), plus a real **StarRocks 4.1.4 FE run outside egret** (as SP4d-3 Task 10 did) and, if the host allows, a Trino 470. Record `df -h /` before and after.

- [ ] **Step 2: C — register the external StarRocks the supported way**

In the browser: Control Plane → Engines → Register external engine; type StarRocks, the FE's host and port, version `2.8.0`, a StarRocks profile, one registry catalog. Confirm: the detail page shows `External`, no pods/monitor tabs, the Plugin config tab with the files and the warnings. **Install** those files on the StarRocks FE (`fe.conf` line + the XMLs), restart it yourself, create the catalog on the FE with the registry name, and confirm the plugin polls egret (egret's access log shows `GET /service/plugins/policies/download/<profile>`). Report every place the UI or the files were wrong. No SQL `INSERT` into `engine_mgmt` or `registry` is allowed in this step — that is the point of it.

- [ ] **Step 3: A — on the real engine**

On that profile: create path rule A `('sales','*','email') MASK`; try rule B `('sales','orders','email') MASK_NULL` → the UI shows the 409 naming A. Query `SELECT email FROM <catalog>.sales.orders` as a user both rules cover → masked by `MASK` (A, the broader — confirming the spec's evalOrder finding live). Then insert B **bypassing** the routes (the one permitted SQL write in this task, recorded verbatim), confirm the rules page banner lists the pair and an unrelated grant write is refused, delete B through the UI, confirm the banner clears and the grant write succeeds. On a Trino profile with a tag service (if Trino is up): the tag ↔ path case of Task 3 test 5, by tagging a column in the UI.

- [ ] **Step 4: D — a migration that did not fit before**

Build a manual profile with ~40 hand-written policies (a script against `POST /api/control-plane/policies` is fine), bound to the external engine. Migrate one of them through the SP4d-3 page: the preview shows `probesFull` and the pruned count, the migration proves and commits, and the real query's answer changes exactly as the preview said; then undo and re-query. Record the request's wall time. Before this package this profile was refused by count.

- [ ] **Step 5: B — in the browser**

Open the policy editor; reproduce the branch's behaviour (B1: the empty-level hint, the root refusal, the `select`-above-column warning, the flagged list on the profile page). For B1, save a table-stopping `select` policy and query a column through the real engine as its subject: it must be denied, exactly as the warning says. Measure the layout of the hint and the callout with `getBoundingClientRect` (FE-TEST-2).

- [ ] **Step 6: The whole-table agreement for declared attachments**

On the live database (yours alone), unscoped:

```bash
psql "$LIVE_DATABASE_URL" -X -A -F' | ' -c "
SELECT e.origin, a.apply_status, count(*)
FROM registry.engine_catalog_attachments a JOIN engine_mgmt.engines e ON e.id = a.engine_id
GROUP BY 1,2 ORDER BY 1,2;"
```
Every `declared` row must be on an `external` engine and no `external` engine may hold any other status. Report the table **with its total**.

- [ ] **Step 7: The report**

`docs/en/plans/2026-10-11-sp4d-4-live-result.md`: every step's observations, query results, timings, and screenshots' descriptions. **A difference between what a page or a preview claimed and what the engine did is a Critical**, not a note. Commit it:

```bash
git add docs/en/plans/2026-10-11-sp4d-4-live-result.md
git commit -m "docs(plans): SP4d-4 live check"
```

---

## Self-review

**Spec coverage.** Part A → Tasks 2 (the widened single-download rule, 409 in the editor), 3 (tag ↔ tag and tag ↔ path, rule names, 409 on every write path including tag assignment and migration), 4 (the report and the banner), with the evalOrder premise pinned on both jars in Task 1 and live in Task 13. Part B → Task 1 (the measurement and the STOP), Tasks 11–12 (either branch, the gap fix in both), Task 13 Step 5. Part C → Tasks 7 (schema, registration, binding, declared attachments, the plugin version source), 8 (every lifecycle handler), 9 (the plugin files), 10 (the pages), Task 13 Step 2 (registration without SQL). Part D → Tasks 5 (the predicate export, pruning, the fallback, the three required tests plus pruned-equals-full), 6 (wiring, the real-engine check on the dropped probes, the timing harness and the re-measured cap), Task 13 Step 4. The programme doc is updated in the spec commit. Out-of-scope items (SP5, priority assignment, signature dedup, external workspace connections and credentials, download auth, `_any`, rewriting stored bodies) appear in no task.

**Placeholders.** None that a task cannot resolve by reading a named file. Where a fact could not be measured while writing (the exact `EngineHttpError` mechanism for a coded 409, whether `lock_services_in_tx` serialises two profiles sharing a tag service, the editor-side function that turns a form into a body, the constraint name of `apply_status`), the step names the file to read and what to do with each answer. Branch-dependent steps are written out for both branches.

**Type consistency.** `ambiguous_arbitrations_in` / `ambiguous_arbitration_in` / `resources_may_intersect` (Task 2) are consumed unchanged by Task 3's `overlap::overlaps` and Task 4's report. `CompileError::AmbiguousMaskOrFilterArbitration { profile, refusal, kind, coordinate, rules }` is defined in Task 3 and read by Task 4. `may_be_candidate` (Task 5, `egret-ranger-eval`) is consumed by `plan_probes`; `PruneMode`, `ProbePlan`, `FallbackReason`, `planned_probe_count` are defined in Task 5 and consumed by Task 6. `external::refuse_if_external` and `RegisterExternalEngine` are defined in Task 7 and consumed by Tasks 8–10; `engine_plugin_version` is defined in Task 7 and is the only place an engine's `PluginVersion` is derived. `ranger_plugin_files` (Task 9) is the only renderer of the plugin files. The migration number `0184` is the expected value only; Task 7 Step 1 re-checks it.

**Risk of order.** Tasks 3, 4, 6 and 7 all touch `egret-perm-compile`; the plan forbids running two at once. Task 6's release build is the one large disk consumer; it checks free space first and cleans up after itself.
