# View governance under clearance: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a classified column stays withheld from an uncleared reader whether that reader goes to the base table or through a view, on StarRocks 4.1.6 and Trino 483.

**Architecture:**
- **StarRocks** gets two new compiled shapes (spec Part A):
  - a priority-1 `MASK_NULL` clearance mask per classified column, whose cleared items `egret-ranger-eval` computes;
  - a per-database `create materialized view` deny.
- **Trino** gets lineage (spec Part B). Each Iceberg view committed through egret's embedded catalog gets table-level lineage from `EXPLAIN (TYPE IO)`. Its columns are tagged, and egret holds the commit until every plugin has downloaded those tags.

**Tech Stack:** Rust (axum, sqlx/Postgres, embedded lakekeeper), Apache Ranger plugin semantics through `egret-ranger-eval`, the real-jar differential (`just test-ranger-gates`), React/TS for two page sections.

**Spec:** `docs/en/specs/2026-10-13-view-governance-design.md`. Read it before any task. The parts are referred to below as A1–A5, B1–B6 and C.

## Global Constraints

**Security semantics**
- **Fail-closed everywhere.**
  - An undecidable fold, an uncomputable lineage, or an unconfirmed tag version refuses. It never guesses, truncates, or commits untagged.
  - Tags are never removed on uncertainty.
- **Cleared principals are enumerated bare usernames** (`crate::members::role_usernames`). Never put a role name in a Ranger item: it matches nobody, and an unmatched deny exception denies everybody (SP4c D0).
- **The clearance mask** has `policyPriority 1` and type `MASK_NULL` for `{public}`. `MASK_HASH` is never emitted.
- **The materialized-view deny** is on `{catalog, database}` with access type `create materialized view`, deny `{public}`. Its exceptions are the users cleared for **every** classification with a column in that database.

**Trino lineage and the commit hold**
- **Lineage is table-level and comes only from the engine** (`EXPLAIN (TYPE IO, FORMAT JSON)`). egret does not parse view SQL.
- **The `egret-lineage` principal** is used only for `EXPLAIN`. egret's client refuses any other statement before it reaches the engine.
- **Hold limit:** 3 × `egret_engine_mgmt_api::ranger_config::RANGER_POLL_INTERVAL_MS`, which is 90 s today (user ruling 2026-10-15, spec R7; it was 2 ×, 60 s, until Task 11's finding I1).
- **Confirmation** waits for every plugin id seen for the service within the last 3 poll intervals.

**Process**
- **Databases.**
  - Never use the dev DB `egret`. Never set `EGRET_ALLOW_DB_SKIP=1`.
  - DB tests take `DATABASE_URL=postgres://egret:egret@localhost:5432/egret_test` explicitly.
  - Live checks use a throwaway DB `egret_viewgov_live` in `egret-dev-postgres-1`.
- **Builds and tests.**
  - Run per-crate `cargo test -p <crate>` during tasks.
  - Run `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo fmt` on every crate you touch.
  - Frontend: follow `docs/en/frontend-rules.md` (normative), run `npm run lint`, `npx tsc --noEmit` and vitest on the touched files.
  - Regenerate the API client only with `npm run gen:api`.
- **Migrations.** New migrations start at **0186**. Re-check against `origin/main` before creating one.
- **Safety.**
  - Never kill processes by name. Never `git stash`, `git checkout -- <file>`, or `git clean`. Do not push.
  - Do not touch the user's `egret-dev-*` containers, except to create the throwaway DB.
  - Ports 5173, 5000 and 8080 are not yours.
  - Keep at least 30 GiB of disk free.
- **Commits.**
  - Identity `hoangquocvietbk2.13@gmail.com`; Conventional Commits.
  - End each message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

---

## File structure

| File | Responsibility | Task |
|---|---|---|
| `crates/egret-perm-compile/src/clearance_mask.rs` (new) | Build the StarRocks priority-1 clearance mask for one column from the desired set plus the cleared users. | 1 |
| `crates/egret-perm-compile/src/clearance.rs` | Emit the per-database materialized-view deny. Its existing per-column deny is unchanged. | 2 |
| `crates/egret-perm-compile/src/trigger.rs:~540` | Run the fold pass after the clearances join `desired` and before the overlap check. | 1 |
| `crates/egret-ranger-eval/tests/differential.rs` | Folded-mask and materialized-view-deny profiles at the four pairs. | 3 |
| `crates/egret-controlplane` or `egret-bi` classification routes | `GET …/classifications/{id}/view-exposure`: existing StarRocks materialized views, Trino views that read the classification, DEFINER audit. | 4, 10 |
| `migrations/0186_plugin_downloads.sql` | Per-plugin download ledger. | 6 |
| `crates/egret-policy/src/ranger/api/download.rs` | Record each tag and policy download; `plugins_hold_tag_version`. | 6 |
| `migrations/0187_view_lineage.sql` | `view_lineage(view → base table)` and a lineage state per view. | 7 |
| `crates/egret-perm-compile/src/lineage.rs` (new) | The EXPLAIN-only client, the lineage computation, and the view tag set. | 7 |
| `crates/egret-catalog-engine/src/view_hold.rs` (new) | The commit hold, in front of the Iceberg view create, replace and commit endpoints. | 8 |
| `crates/egret-perm-compile/src/trigger.rs` (classification triggers) | Re-tag dependent views in the same tag-version bump. | 9 |
| `docs/en/guides/view-governance.md` (new) | The operator guide: what is covered and what is not (A5, B6). | 10 |
| `docs/en/plans/2026-10-13-view-governance-live-result.md` (new) | The measurements from Task 5 and the live re-run from Task 11. | 5, 11 |

---

### Task 1: The StarRocks clearance mask, folded through the evaluator (A2)

**Files:**
- Create: `crates/egret-perm-compile/src/clearance_mask.rs`.
- Modify: `crates/egret-perm-compile/src/lib.rs` (mod), and `crates/egret-perm-compile/src/trigger.rs` (call it after the clearance block at `:540-560` and before `refuse_created_by_compile_via`).
- Test: unit tests in `clearance_mask.rs`, plus a DB test in `crates/egret-perm-compile/tests/` next to the existing clearance tests (find them with `git grep -l "SourceKind::Clearance" crates/egret-perm-compile/tests`).

**Interfaces:**
- Consumes:
  - `egret_ranger_eval` (find the data-mask evaluation entry the differential uses for `policyType 1` requests; `git grep -n "DataMask\|data_mask" crates/egret-ranger-eval/src/eval.rs | head`);
  - `TaggedColumn` and `column_resources(engine, col, PolicyTarget::…)` from `rules.rs:471`;
  - the cleared usernames `compile_clearance` already computes.
- Produces:
  - `pub(crate) fn fold_clearance_mask(column: &TaggedColumn, cleared: &[String], others: &[serde_json::Value], engine: EngineType, plugin: PluginVersion) -> Result<serde_json::Value, CompileError>`;
  - `CompileError::ClearanceMaskUndecidable { column: String, user: String }`;
  - policy names `"{COMPILER_CLEARANCE_PREFIX}{tag_id}-mask-{catalog}.{db}.{table}.{column}"`.

- [ ] **Step 1: Read first.**
  - Spec A2.
  - `clearance.rs:1-140` (why enumeration) and `:320-375` (the StarRocks branch).
  - `egret_policy::ambiguous_arbitration` (why priority 1 is ordered).
  - How the profile's plugin version is known at compile time: `engine_plugin_version` in `routes.rs`. If more than one engine is bound, use each bound version and refuse when the fold differs between them.

- [ ] **Step 2: Write the failing unit tests** in `clearance_mask.rs`:

```rust
#[test]
fn cleared_users_get_what_the_rule_masks_give_them_and_everyone_else_gets_null() {
    // A rule mask MASK_HASH for {users:[ann]} on sales.s.t.ssn, and a wildcard
    // rule mask MASK (redact) for {groups:[public]} on sales.s.t.*.
    // Cleared = [ann, bob]. Expected items, in order:
    //   {users:[ann], MASK_HASH}, {users:[bob], MASK}, {groups:[public], MASK_NULL}
    // policyPriority 1, resource = exactly the one column.
}

#[test]
fn a_cleared_user_no_rule_masks_gets_mask_none() { /* items: {users:[bob], MASK_NONE}, {public, MASK_NULL} */ }

#[test]
fn an_undecidable_cleared_user_refuses_the_compile() {
    // Two priority-0 rule masks with different types on the same column,
    // both matching ann → the evaluator says Undecidable →
    // Err(ClearanceMaskUndecidable { column: "sales.s.t.ssn", user: "ann" }).
}

#[test]
fn no_cleared_users_gives_only_the_public_null_item() {}

#[test]
fn never_emits_mask_hash_for_public() {}
```

  Fill each body with concrete JSON built by the existing test helpers in `rules.rs` tests (`git grep -n "fn masking_body\|fn mask_policy" crates/egret-perm-compile/src`). If none fits, write a local `fn mask_policy(name, column, items) -> Value` in the test module.

- [ ] **Step 3: Run the tests.**
  Run `cargo test -p egret-perm-compile clearance_mask`. Expected: compile errors or FAIL.

- [ ] **Step 4: Implement.**
  - For each cleared user, ask the evaluator for the data-mask outcome on the column with the `others` policies. Group the users by outcome (mask type plus value expression).
  - Emit one item per group, sorted by the first username so the output is deterministic. Then emit `{groups:["public"]} MASK_NULL`, with priority 1, stamped with `stamp_service_type`.
  - In `trigger.rs`, for StarRocks only: `others` is every `desired` body with `policyType == 1` that is not itself a clearance mask. Insert the result with `SourceKind::Clearance`.

- [ ] **Step 5: Write the DB test.** A StarRocks profile with one classified column and one rule mask:
  - the recompile stores a priority-1 clearance mask whose items match Step 2's first test;
  - the rule policy is unchanged;
  - `GET /api/control-plane/perm/overlaps` (or its store function) returns no pair for that column.

- [ ] **Step 6: Mutate to show the tests can fail.**
  - Swap the item order (put `{public}` first). The first unit test and the DB test must fail.
  - Restore by plain write and check the SHA-256.

- [ ] **Step 7: Run the per-crate tests, clippy and fmt, then commit.**
  Message: `feat(perm-compile): StarRocks clearance mask, folded through the evaluator`.

---

### Task 2: The per-database materialized-view deny (A3)

**Files:**
- Modify: `crates/egret-perm-compile/src/clearance.rs` (the StarRocks branch).
- Test: unit tests in `clearance.rs`, and the clearance DB test file from Task 1.

**Interfaces:**
- Consumes: `tagged_columns_for_service_via` (already used at `clearance.rs:~328`), and the cleared set per classification.
- Produces: policies named `"{COMPILER_CLEARANCE_PREFIX}mv-{catalog}.{database}"`, one per database that holds at least one classified column, with `SourceKind::Clearance`.

- [ ] **Step 1: Write the failing tests.**
  - A database holding columns of classifications X (cleared: ann, bob) and Y (cleared: bob, cy) gets one policy:
    - resources `{catalog, database}`;
    - `denyPolicyItems [{groups:[public], accesses:[create materialized view]}]`;
    - `denyExceptions [{users:[bob], …}]`.
  - A database with no classified column gets no policy.
  - When the intersection is empty, `denyExceptions` is empty: the deny holds for everyone, and that is correct.
- [ ] **Step 2: Run the tests.** They should FAIL.
- [ ] **Step 3: Implement.**
  - Group the tagged columns of **all** clearance-bearing classifications by `(catalog, database)`. Intersect the cleared sets. Emit one policy per database.
  - Check that `"create materialized view"` is spelled exactly as in `crates/egret-policy/src/service-defs/starrocks.json` (it is).
  - The expansion ceiling (`DEFAULT_EXPANSION_CEILING`) counts these policies too.
- [ ] **Step 4: Write the DB test.** A recompile after adding a column of a second classification in the same database narrows the exceptions.
- [ ] **Step 5: Mutate.** Use a union in place of the intersection: a test must fail. Restore.
- [ ] **Step 6: Run the per-crate tests, clippy and fmt, then commit.**
  Message: `feat(perm-compile): deny creating a materialized view in a database that holds classified columns`.

---

### Task 3: Real jars, explain and the probe budget (C)

**Files:**
- Modify: `crates/egret-ranger-eval/tests/differential.rs`, `crates/egret-perm-compile/src/routes.rs` (explain wording), and the explain wire types if a new cause is needed (`explain_wire.rs`).
- Test: the differential (run with `just test-ranger-gates`), and the explain unit tests.

**Interfaces:**
- Consumes: the bodies from Tasks 1–2.
- Produces: an explain line naming the classification as the mask (`MASK_NULL`) or as the materialized-view deny.

- [ ] **Step 1: Add the differential profiles.**
  - A StarRocks profile with a folded clearance mask: two rule masks (one wildcard) and three users (one cleared with a rule mask, one cleared with none, one uncleared).
  - A StarRocks profile with the materialized-view deny: requests for `create materialized view` on the database by each user.
  - Run both at StarRocks@2.8.0 and StarRocks@2.9.0, following the pattern of the existing `starrocks()` and `starrocks_2_9_0()` profiles.
  - Add both to every loop that runs `starrocks()`.
- [ ] **Step 2: Run the real-jar comparison.**
  Run `just test-ranger-gates` (about 40 min; run it in the background and wait). Expected: 0 disagreements and 0 `error` entries.
  **STOP and report** on any disagreement. Do not narrow the corpus.
- [ ] **Step 3: Update explain.**
  - For an uncleared reader of a classified StarRocks column, explain shows two reasons:
    - the clearance deny, for a direct read;
    - the clearance mask (`MASK_NULL`), for the masked request.
  - Write the tests first: one for each reader kind.
- [ ] **Step 4: Measure the probe budget.**
  - Build the worst shape this adds: N classified columns × the fold, plus one materialized-view deny per database.
  - Time it against `MAX_PROBE_WORK` with the existing timing test pattern (`git grep -n MAX_PROBE_WORK crates`).
  - Record the practical limit in the task report. Change the cap only if the measured whole-section time exceeds the documented bound, and say so.
- [ ] **Step 5: Run the per-crate tests, clippy and fmt, then commit.**
  Message: `test(ranger-eval): folded clearance masks and the materialized-view deny on the real jars`.

---

### Task 4: List the materialized views that already exist (A4)

**Files:**
- Create: a route `GET /api/control-plane/classifications/{id}/view-exposure`.
  - Find where the classification routes live: `git grep -n "classification" crates/*/src/*routes*.rs | head`.
  - Put it beside them, with the same permission as reading a classification.
- Modify: the classification page in the frontend (find it with `git grep -ln "classification" frontend/src/routes | head`).
- Test: a route test with a fake StarRocks lister, and a vitest for the page section.

**Interfaces:**
- Produces:
  - `ViewExposureDto { starrocksMaterializedViews: Vec<{engine, catalog, database, name}> | null, starrocksListError: Option<String>, trinoViews: … }`. Task 10 fills `trinoViews`; return `[]` here.
  - A trait `MaterializedViewLister` with `async fn list(&self, engine, database) -> Result<Vec<String>, String>`, so the route test does not need a live StarRocks.

- [ ] **Step 1: Write the failing tests.**
  - The listed views appear.
  - An unreachable engine gives `starrocksListError` set and `starrocksMaterializedViews: null`, never `[]`.
- [ ] **Step 2: Implement.**
  - Run `SHOW MATERIALIZED VIEWS FROM <db>` through the existing StarRocks admin connection. Quote identifiers the way `egret-starrocks-backslash-sql-injection` fixed it: find the quoting helper with `git grep -n "fn quote_ident\|backslash" crates/egret-engine-mgmt/src | head`.
  - The databases are the ones from Task 2.
- [ ] **Step 3: Build the page section.**
  - Show the warning ("may hold real classified values; egret does not drop them"), and the error line when listing fails.
  - Write the vitest first.
- [ ] **Step 4: Regenerate.** Re-emit `openapi.json` and run `npm run gen:api`.
- [ ] **Step 5: Run the tests and lint, then commit.**
  Message: `feat(control-plane): show the materialized views a classification's databases already hold`.

---

### Task 4b: Move the materialized-view deny and listing to `default_catalog` (A3/A4 revised 2026-10-14)

**Why:** measured on StarRocks 4.1.6 (`docs/en/plans/2026-10-14-starrocks-mv-placement-measurement.md`). A materialized view can only be created in `default_catalog`, and the privilege is checked on the target `default_catalog.<db>`. Task 2's per-(catalog, database) deny never matches, and Task 4's `SHOW MATERIALIZED VIEWS FROM <ext>.<db>` returns an empty set. Read spec §A3 and §A4 (revised) first.

**Files:**
- Modify: `crates/egret-perm-compile/src/clearance.rs` (`mv_deny_inputs`), plus its tests.
- Modify: `crates/egret-ranger-eval/tests/differential.rs`, the scenario test `the_clearance_mask_and_the_materialized_view_deny_decide_as_designed_on_the_real_jars`, and `harness_support` if needed.
- Modify: `crates/egret-perm-compile/src/view_exposure.rs` and `crates/egret-controlplane/src/view_exposure.rs`; the engine-mgmt lister; their tests; the frontend section.

**Interfaces:**
- Produces:
  - `mv_deny_inputs` emits **at most one** policy per StarRocks service:
    - name `{COMPILER_CLEARANCE_PREFIX}mv` (no catalog or database in the name);
    - resource `{catalog: ["default_catalog"], database: ["*"]}`;
    - deny `create materialized view` for group `public`;
    - `denyExceptions` = the users cleared for every classification with a column on the service, with the existing group-only-gate skip rule unchanged.
  - The lister trait becomes `list(engine) -> Result<Vec<MvRow>, String>`.
    - `MvRow { database, name, base_tables: Option<Vec<String>>, definition: String }`.
    - Read from `SELECT TABLE_SCHEMA, TABLE_NAME, BASE_TABLE_REFRESH_VERSION_TIMES, MATERIALIZED_VIEW_DEFINITION FROM information_schema.materialized_views`, selecting columns by name.
    - `base_tables` holds the keys of the JSON map, and is `None` when the map is empty or missing.
  - `ViewExposureDto` items gain `matchedBy: "baseTable" | "text"`, and the DTO gains `enginesNotChecked: Vec<{engine, status}>`.

- [ ] **Step 1: failing tests (compiler).**
  - A service with classifications X and Y gets exactly one MV deny on `default_catalog`/`*`, excepting exactly the users cleared for both.
  - A service with no classified column gets none.
  - A stored `…mv-{catalog}.{database}` policy from the old shape is swept on recompile (DB test).
- [ ] **Step 2: failing tests (real jars).**
  - Change the scenario's MV-deny probes to `create materialized view` on `{catalog: default_catalog, database: <some db>}`, including a database that holds no classified column.
  - The uncleared user is denied there; the user cleared for all is allowed.
  - Run only that test at StarRocks@2.8.0 and @2.9.0, then the full `just test-ranger-gates` once: 0 disagreements, and STOP on any disagreement.
- [ ] **Step 3: failing tests (listing).**
  - A row whose `base_tables` names a classified `catalog.db.table` of this classification, in a catalog attached to *this engine*, matches as `baseTable`.
  - A row with `base_tables: None` whose definition contains the classified catalog and db names matches as `text`.
  - A row over another engine's catalog does not match.
  - A non-running engine is not dialled and appears in `enginesNotChecked`.
  - Mixed success and failure, and the deadline behaviour from fix round 1, stay as they are.
  - This also closes Task 4 review finding I-3.
- [ ] **Step 4: implement.** The per-engine scope comes from that engine's own catalog attachments, not from every engine sharing the profile.
- [ ] **Step 5: frontend.**
  - Show `matchedBy: text` as "matched by text".
  - List `enginesNotChecked`.
  - The warning names the cost: users not cleared for every classification on the engine cannot create any materialized view there.
  - Write the vitest first, then run `npm run gen:api`.
- [ ] **Step 6: run tests and lint, then commit.** Message: `fix(perm-compile): deny materialized views where StarRocks creates them, and list them there`.

---

### Task 5: Measure before building Part B (STOP gates)

**Files:**
- Create: section 1 of `docs/en/plans/2026-10-13-view-governance-live-result.md`.
- Scratch code is not committed, unless it becomes a test.

**Interfaces:**
- Produces four answers that Tasks 6–8 read, each with evidence.

- [ ] **Step 1: Bring up the stack.**
  - Use the Task 6 stack recipe of the engine-upgrade live result (`docs/en/plans/2026-10-13-engine-upgrade-live-result.md` §1.1), on `egret_viewgov_live` and egret on :8096.
  - Use Trino 483 with an Iceberg catalog served by egret (the `ice` catalog recipe in the view-clearance measurement doc).
  - Prefix containers with `vg-`.
- [ ] **Step 2 (Q1): Which hook sees a view change before it is committed?**
  - Read lakekeeper's `contract_verification` (in the cargo registry source: `find ~/.cargo/git ~/.cargo/registry -path "*lakekeeper*contract_verification*" | head`).
  - Answer: does it run for view create, replace and commit; does it run before the commit; can it veto; does it see the SQL and the default namespace?
  - Then observe, by logging in an axum layer around the mounted router (`egret-catalog-engine/src/mount.rs`), which HTTP requests Trino sends for `CREATE VIEW`, `CREATE OR REPLACE VIEW` and `ALTER VIEW … AS` (if Trino supports it on Iceberg; record it if not).
  - **Choose one hook.** It must see all three before the commit.
- [ ] **Step 3 (Q2): Is the security mode stored?**
  - Create one DEFINER and one INVOKER view.
  - Load both through `egret_catalog_engine::load_view_detail`, and record where (or whether) the security mode appears.
- [ ] **Step 4 (Q3): What does EXPLAIN need?**
  - Create a principal `egret-lineage` with no grant, then add grants one by one until `EXPLAIN (TYPE IO, FORMAT JSON) <view SQL>` succeeds.
  - Record the minimum: `select` on columns, or `show`/metadata only.
  - Record the JSON shape of the input tables (catalog, schema, table) for a nested view and for a join.
- [ ] **Step 5 (Q4): The `pluginId` format.**
  - Record the `pluginId` query parameter that Trino 483 and StarRocks 4.1.6 send on `download_tags` and `download_policies` (from egret's request log).
  - Record how to map it to an engine (host or pod, and the service name).
- [ ] **Step 6: Write section 1.** One subsection per question: the answer, the evidence (quoted log lines and request lines) and the chosen hook.
- [ ] **Step 7: Stop your own PIDs and `vg-*` containers, then commit.**
  Message: `docs(plans): view-governance measurements before building the commit hold`.

**STOP and report BLOCKED** in any of these cases:
- no hook sees all three view changes before the commit;
- EXPLAIN requires broad `select` on data;
- `pluginId` cannot be mapped to an engine.

Each is a design question for the user.

---

### Task 6: The plugin download ledger and `plugins_hold_tag_version` (B4)

**Files:**
- Create: `migrations/0186_plugin_downloads.sql`.
- Modify: `crates/egret-policy/src/ranger/api/download.rs` (`download_tags` at `:150`, `download_policies` at `:41`), and `crates/egret-policy/src/store.rs`.
- Test: `crates/egret-policy/tests/` (a DB test, using the route test helpers already there).

**Interfaces:**
- Produces:
  - table `ranger_plugin_downloads(service_id uuid, plugin_id text, kind text check (kind in ('policies','tags')), last_known_version bigint, served_version bigint, at timestamptz, primary key (service_id, plugin_id, kind))`;
  - `pub async fn plugins_hold_tag_version(store, service_id: Uuid, version: i64, recent: Duration) -> Result<HoldState, StoreError>`, where `HoldState { confirmed: bool, waiting_on: Vec<String> }`;
  - `pub async fn wait_for_tag_version(store, service_id, version, deadline: Duration) -> Result<(), WaitError>`, which polls `plugins_hold_tag_version` every 500 ms. `WaitError::Timeout { waiting_on }` names the plugins.

- [ ] **Step 1: Write the failing tests.**
  - A download records a row (upsert).
  - A 200 that serves V does not by itself make the plugin hold V. The next poll with `lastKnownVersion=V` does.
  - Two plugin ids for one service: `confirmed` only when both hold version ≥ V.
  - A plugin not seen within `recent` is ignored.
  - With no plugin seen at all, `confirmed: false` and `waiting_on: ["<no plugin has polled>"]`. This is fail-closed: never confirm vacuously.
- [ ] **Step 2: Write the migration, the upsert in both handlers, and the two functions.**
  - A plugin "holds" only the `lastKnownVersion` it reports on a later poll. A version served on a 200 is recorded, but it never counts as held (spec B4, revised 2026-10-14).
  - Key the row by the service the poll names. Trino polls tags under its **resource** service name, not under the tag service (live result §1 Q4). So `wait_for_tag_version` takes the resource service ids of the Trino engines bound to the affected tag service. StarRocks never polls tags.
  - Record without delaying the response. One upsert is acceptable inline; if measured above 5 ms, move it off the request path and say so.
- [ ] **Step 3: Mutate.** Make `confirmed` true when any one plugin holds the version. The two-plugin test must fail. Restore.
- [ ] **Step 4: Run the tests, clippy and fmt, then commit.**
  Message: `feat(policy): record what each Ranger plugin downloaded and confirm a tag version`.

---

### Task 7: Lineage store and computation (B1)

**Files:**
- Create: `migrations/0187_view_lineage.sql`, `crates/egret-perm-compile/src/lineage.rs`.
- Modify: the policy DOWNLOAD, not the compiler (spec R5, revised 2026-10-14 in Task 7 fix round 1). `crates/egret-policy/src/lineage_transform.rs` transforms what `download_policies` serves, and `explain` evaluates the same result through `served_artifact_via`. Stored policies never name `egret-lineage`. The transform applies only to a plugin whose `pluginId` names the polled service, on a profile where every bound engine is a Trino engine egret provisions in JWT mode. It does five things:
  - a leading no-transform `egret-lineage` item in every served mask and row filter, hand-written and tag-based included;
  - an `egret-lineage` exception, for the reads, from every served deny;
  - an allow exception for every non-read access type any served allow grants;
  - `use` and `select` on every attached catalog;
  - an override deny of every other access type except what EXPLAIN needs.

  Evaluator tests over the transformed download show the principal is allowed, unmasked and unfiltered on classified, masked and filtered columns, compiled and hand-written. They also show it never writes where `{public}` may. An ineligible engine is served the stored policies. Also modify the engine connection code that runs statements on Trino (`git grep -n "fn run_query" crates/egret-query/src`).
- Test: unit tests in `lineage.rs`, and a DB test.

**Interfaces:**
- Consumes: Task 5's Q3 JSON shape.
- Produces:
  - tables:
    - `view_lineage(view_catalog, view_schema, view_name, base_catalog, base_schema, base_table, primary key (all six))`;
    - `view_lineage_state(view_catalog, view_schema, view_name, state text check (state in ('computed','failed')), detail text, at timestamptz)`;
  - `pub struct ViewRef { catalog: String, schema: String, name: String }`;
  - `pub trait ExplainRunner { async fn explain_io(&self, default_catalog: &str, default_schema: &str, sql: &str) -> Result<serde_json::Value, String>; }`;
  - `pub fn lineage_tables(explain_json: &Value) -> Result<BTreeSet<(String,String,String)>, String>`;
  - `pub async fn view_tag_set(conn, tables: &BTreeSet<…>) -> Result<BTreeSet<Uuid /*tag id*/>, LineageError>`: the union of the clearance-bearing tags on any column of those tables. An empty set is `Err(Empty)`, never "no tags". `recorded_view_tag_set(conn, view)` errors on a missing or failed lineage;
  - `write_lineage` refuses an empty set; `recorded_lineage` returns `RecordedLineage::{Missing, Failed{detail}, Computed(set)}`;
  - `pub fn explain_only(sql: &str) -> Result<String, String>`: wraps the statement as `EXPLAIN (TYPE IO, FORMAT JSON) <sql>`.
    - Do not write a lexer: a lexer-based gate has been bypassed four times in this repo.
    - The defences are, first, Trino's statement protocol, which runs exactly one statement per request, so the wrapped text is an EXPLAIN or a parse error. Second, the principal holds only `use` and `select` (enforced by the download transform's allow exceptions and its override deny).
    - `explain_only` refuses an empty `sql`, and a `sql` whose first keyword after whitespace is `EXPLAIN`, because `EXPLAIN ANALYZE` executes.
    - The runner checks that the response parses as the IO-plan shape.

- [ ] **Step 1: Write the failing tests.**
  - `lineage_tables` on the JSON recorded in Task 5 (nested view, join) gives exactly the base tables. The shape is `inputTableColumnInfos[].table = {catalog, schemaTable: {schema, table}}`; see live result §1 Q3.
  - An unexpected shape gives `Err`, never an empty set.
  - `view_tag_set` gives the union.
  - `explain_only` refuses `""` and `explain analyze select 1`.
  - A fake `ExplainRunner` returning an empty input list gives `Err` with the R6 reason ("the engine reports no input table; egret refuses views it cannot judge"). An empty lineage is always a refusal (spec R6).
- [ ] **Step 2: Implement**, including the runner bound to the `egret-lineage` principal on a Trino engine bound to the view's catalog.
- [ ] **Step 3: Mutate.** Make `lineage_tables` return `Ok(empty)` on an unknown shape. A test must fail. Restore.
- [ ] **Step 4: Run the tests, clippy and fmt, then commit.**
  Message: `feat(perm-compile): table-level view lineage from the engine's EXPLAIN`.

---

### Task 8: Hold the view commit until the tags are on every plugin (B2)

**Files:**
- Create: `crates/egret-catalog-engine/src/view_hold.rs`, wired as an axum layer in front of the mounted Iceberg router in `mount.rs`. This is the hook Task 5 Q1 chose; read live result §1 Q1 for the exact requests.
- Test: a DB test with a fake `ExplainRunner` and a fake plugin. The plugin is a task that calls `download_tags` with an advancing `lastKnownVersion`, or never does.

**Interfaces:**
- Consumes: Tasks 6 and 7.
- Produces: the hold. The view tag assignments use the existing catalog tag assignment store (`catalog_tag_assignments`; `git grep -n "catalog_tag_assignments" crates/egret-bi/src/catalog_tags.rs`), with one row per view column.

**Order of operations.** A DB transaction must never stay open across the wait.
1. Compute the lineage. On failure, refuse.
2. If no lineage table is classified: record the lineage (`computed`) and let the commit proceed.
3. Otherwise, in one transaction: write the view column tag assignments; write the lineage rows and `view_lineage_state = 'computed'`; bump the tag version. Commit that transaction.
4. Wait (`wait_for_tag_version`, limit 2 × poll interval) for every tag service a bound Trino engine uses.
5. On success, let the view commit proceed. On timeout, or if the view commit itself fails, compensate in one transaction: delete exactly the assignments and lineage rows written in step 3, and bump the tag version again. Then refuse.

**Requests (live result §1 Q1).**
- Create is `POST …/namespaces/{ns}/views`. Replace is `POST …/views/{v}` carrying `add-view-version`.
- The SQL, the default catalog and the default namespace come from the view version in the body.
- A commit with no new view version passes through untouched. Examples are `COMMENT ON VIEW` and a replace whose updates are empty.
- If lakekeeper answers non-2xx after a hold, compensate exactly as in step 5.
- Rename (`POST …/views/rename`) moves the lineage rows and the view column tag assignments to the new name, under the same hold.

**Refusal.** A `409` Iceberg REST error, which Trino prints verbatim (measured). The message names one of the reasons:
- `lineage could not be computed: …`;
- `engines did not confirm the classification tags within 60 s: <plugin ids>`. *(Task 12: the limit is now 90 s, per the Global Constraints.)*

- [ ] **Step 1: Write the failing DB tests.**
  1. With the fake plugin polling, the view is committed only after the plugin's download shows version ≥ V. Assert this by ordering: the commit observed at time T, the download at time ≤ T.
  2. With a plugin that never polls, the request is refused within the limit plus a margin. No view exists, and no assignment or lineage row remains.
  3. A view over unclassified tables commits with no wait.
  4. An EXPLAIN failure refuses, and nothing is written.
  5. `CREATE OR REPLACE` goes through the same hold. (`ALTER VIEW … AS` is not in Trino 483's grammar.)
  6. A commit with no new view version passes untouched.
  7. When lakekeeper answers non-2xx after the hold, nothing written in step 3 remains.
  8. Rename moves the lineage and the tags.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Mutate.** Skip the wait. Test 1 must fail. Restore.
- [ ] **Step 4: Run the tests, clippy and fmt, then commit.**
  Message: `feat(catalog): hold a classified view's commit until every plugin holds its tags`.

---

### Task 9: Keep view tags in step with base classifications (B3)

**Files:**
- Modify: the classification and tag-assignment write paths that already recompile (`crate::trigger`; find the tag-assignment trigger with `git grep -n "fn .*tag_assign" crates/egret-perm-compile/src/trigger.rs`), plus the view drop handling at the Task 5 hook.
- Create: a sweep route `POST /api/control-plane/classifications/lineage-sweep`, and a scheduled sweep. Use the existing scheduler pattern: `git grep -n "interval(" crates/egret-bin/src | head`.
- Test: DB tests.

**Interfaces:**
- Consumes: the Task 7 tables and `view_tag_set`.
- Produces: `pub async fn retag_dependent_views_in_tx(tx, changed_tables: &[(String,String,String)]) -> Result<usize, CompileError>`.

- [ ] **Step 1: Write the failing tests.**
  - Classifying a base column re-tags every dependent view, in the same tag version as the base assignment. Assert a single bump.
  - Declassifying removes the view tags. This is not "removal on uncertainty": the lineage is computed.
  - A view whose `view_lineage_state` is `failed` keeps its old tags.
  - Dropping a view deletes its lineage and its assignments.
  - The sweep recomputes lineage and reports the failures.
  - The sweep tags a view whose pruned branch now reads a classified table (spec R6). Simulate this with a fake runner whose answer changes between runs.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Mutate.** Bump the tag version twice (once for the base, once for the views). The single-bump test must fail. Restore.
- [ ] **Step 4: Run the tests, clippy and fmt, then commit.**
  Message: `feat(perm-compile): view tags follow their base tables' classifications`.

---

### Task 10: DEFINER audit, the Trino half of the page, and the guide (B5, A5, B6)

**Files:**
- Modify: the Task 4 route (fill `trinoViews`) and its page section.
- Create: `docs/en/guides/view-governance.md`.
- Test: a route test and a vitest.

**Interfaces:**
- Produces: `trinoViews: Vec<{catalog, schema, name, securityMode: "definer"|"invoker"|"unknown", owner: Option<String>, ownerCleared: Option<bool>, reason: "classified"|"mask_or_row_filter"}>`. The mode comes from the stored view property `trino.run-as-owner`. It is present for DEFINER (the value is the owner) and absent for INVOKER, as live result §1 Q2 found. `load_view_detail` drops properties, so read them separately. Use `"unknown"` when the metadata cannot be read. Never infer "definer". `trinoViews` items also carry `matchedBy: "lineage"`, on the shared item type from Task 4b.

- [ ] **Step 1: Write the failing tests.**
  - A view whose lineage includes a classified table is listed with `reason: "classified"`.
  - A view over a table with a mask rule is listed with `reason: "mask_or_row_filter"`.
  - When the metadata cannot be read, the mode is `"unknown"`.
  - A view with `trino.run-as-owner` is `"definer"` with that owner. A view without it is `"invoker"`.
- [ ] **Step 2: Implement the route.** Then build the page section: the table, plus a one-line explanation of DEFINER versus INVOKER. Write the vitest first.
- [ ] **Step 3: Write the guide.**
  - What is protected:
    - StarRocks: direct reads (deny), views (`NULL`), new materialized views (denied);
    - Trino: views in egret's Iceberg catalogs.
  - What is not (A5, B6):
    - data a cleared user copies out;
    - other connectors' views;
    - native StarRocks tables;
    - engines older than 4.1.6;
    - existing materialized views.
  - The `CREATE VIEW` wait and its refusal messages, and what to do about each.
  - The `SECURITY INVOKER` recommendation. Because of the Trino 483 defect, switching a view to INVOKER needs `DROP VIEW` then `CREATE VIEW`: `CREATE OR REPLACE … SECURITY INVOKER` keeps DEFINER.
  - The accepted risks, in plain words (spec R2, R5, R6, B4):
    - users not cleared for every classification on a StarRocks engine cannot create any materialized view there;
    - the `egret-lineage` principal can read governed data, and only egret holds its credential;
    - a view is refused when the engine reports no input table;
    - a pruned branch is caught only at the next sweep;
    - the anonymous download endpoint lets a forged poll end a hold early, and the real plugin catches up within one interval.
- [ ] **Step 4: Regenerate the API client, run the tests and lint, then commit.**
  Message: `feat(control-plane): audit DEFINER views over classified data; view-governance guide`.

---

### Task 11: The live re-run on Trino 483 and StarRocks 4.1.6

**Files:**
- Modify: section 2 of `docs/en/plans/2026-10-13-view-governance-live-result.md`.

- [ ] **Step 1: Write the predictions for every cell first, and commit them.**
  Message: `docs(plans): view-governance live predictions, written before observing`.
- [ ] **Step 2: Observe on StarRocks 4.1.6.** Use the users and SQL of the view-clearance measurement (round 2 A battery):
  - an uncleared reader gets `NULL` through plain, nested, expression and INVOKER views;
  - a direct read is refused;
  - a cleared reader with a rule mask gets the rule mask;
  - `create materialized view` by an uncleared user is refused, while `create view` and `create table` work;
  - the page lists a materialized view created before the deny;
  - the FE sends `create materialized view` on `{default_catalog, <db>}` to the plugin (from egret's or the FE's audit log);
  - the admin connection sees every materialized view in `information_schema.materialized_views` under Ranger.
- [ ] **Step 3: Observe on Trino 483**, with an Iceberg catalog served by egret:
  - a DEFINER view by a cleared owner is refused to an uncleared reader on the **first** read after `CREATE VIEW` returns (run the read immediately, in the same script);
  - a cleared reader reads it;
  - `CREATE VIEW` over a classified table returns after at most one poll interval; *(Task 11 measured one to two intervals, finding I1.)*
  - stopping the engine's polling (pause the container) gives the refusal message, and no view is created;
  - classifying a base column after the view exists denies the view to the uncleared reader once the tags arrive;
  - whether Trino workers, not only the coordinator, poll tags (from the plugin ledger), and that the hold waits for all of them;
  - a view with an empty lineage (`WHERE false`) is refused with the R6 reason.
- [ ] **Step 4: Compare explain and the page with the engine** for each reader. Any difference is **Critical**.
- [ ] **Step 5: Write section 2**, stop your own PIDs and `vg-*` containers, list what is left, and commit.
  Message: `docs(plans): view governance live on Trino 483 and StarRocks 4.1.6`.

---

## Self-review

**Spec coverage.**

| Spec | Task |
|---|---|
| A1 | Unchanged; the Task 1 DB test checks that it is still emitted. |
| A2 | 1 |
| A3 | 2 |
| A4 | 4 |
| A5, B6 limits | 10 (guide and page) |
| B1 | 5 (Q3), 7 |
| B2 | 5 (Q1), 8 |
| B3 | 9 |
| B4 | 5 (Q4), 6 |
| B5 | 5 (Q2), 10 |
| C: evaluator, explain, probe budget | 3 |
| Testing table: live | 11 |

**Deliberate deviation from the spec text.**
- The spec's B2 says "in one transaction … then commit the view". The plan commits the tag transaction **before** the wait, and compensates on failure. A transaction held open for up to 60 s across a network wait is not acceptable.
- The guarantee is unchanged: the view does not exist until the tags are confirmed.

**Order risk.**
- Tasks 1–3 touch the compiler and must run in order.
- Task 5 gates Tasks 6–8. Its STOP conditions are design questions for the user, not implementation choices.
