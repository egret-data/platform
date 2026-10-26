# Workflow package #1 "No silent drops" — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** `docs/en/specs/2026-10-19-workflow-no-silent-drops-design.md`. Read its Decisions (D1–D16) and §1's table first. The plan adds no decision. The spec's §12 has five questions for the controller; this plan builds the spec's default for each, and Task 8b is the optional half of Q1.

**Goal:** every key egret parses and then ignores becomes a named refusal (`unknown_property`) or a named warning (`not_honoured`) that names the roadmap package lifting it, except flow and flowable `finally:`/`afterExecution:` and the `errors:` lists #2 takes. Kestra's own save-time warnings are ported, the stale package names in today's messages are replaced, and the stale "345" figures are fixed.

**Architecture:**
- A new production module `src/spec/keys.rs` holds the key tables (moved out of `spec/schema/measure.rs`), the `Lift` table and the save-time walker `keys::findings(source, &flow, &mut report)`.
- A new entry point `spec::validate_source(source, &flow)` = `validate(&flow)` + `keys::findings`. The publish path, the draft DTO, the validate route and `measure.rs` call it. `validate_for_run` and `build_plan` are untouched, so nothing new runs at run time.
- Core leaves are checked on the model (their spec is kept verbatim). Everything serde drops is checked by walking the raw source beside the model.
- Scripts, plugins, `validate.rs`'s flowable refusals and the trigger sync/fire refusals read their package names from `keys::lift_of`.

**Tech stack:** Rust (serde, serde_norway, serde_json, sqlx, tokio), Postgres for the publish/route/oracle tests.

**Branch and worktree:** `feat/workflow-no-silent-drops` in `data-portal/egret-nodrops`, from main `309405b1`. One PR. No migration.

## Facts measured for this plan (2026-10-19, at `309405b1`)

Paths are relative to `crates/egret-workflow/src/` unless another root is given. Kestra is `data-portal/kestra` at `96893b589`, paths relative to `core/src/main/java/io/kestra/` (`k/`).

- **Findings.** `Finding {code: &'static str, message, path}` and `ValidationReport {errors, warnings}`: `spec/validate.rs:85-103`. `validate` = `validate_for_run` + `save_only_findings` (`:350-370`). `save_only_findings` holds `pluginDefaults`, duplicate trigger ids and `secret()` (`:360-371`). Re-exports: `spec/mod.rs` (`pub use validate::{validate, validate_for_run, Finding, ValidationReport}`).
- **Callers of `validate(&Flow)` in production:** `publish.rs:311` (`publish_gated`), `routes.rs:513` (`draft_dto`), `routes.rs:899` (`validate_flow_source`), and `spec/schema/measure.rs:239` (test-only). `trigger/depends_on.rs:1123,1208` are tests.
- **The run gate:** `executor::build_plan(source)` (`executor/plan.rs:1093-1105`) calls `validate_for_run` only.
- **Parse:** `spec::parse` (`spec/parse.rs:30-33`) runs `DuplicateKeyProbe` then `serde_norway::from_str::<Flow>`.
- **`not_yet_enforced`:** the table `NOT_YET_ENFORCED` (`validate.rs:173-184`), the emitter (`:2092-2117`, message at `:2110-2113`), the pin test `the_not_yet_enforced_list_is_pinned_in_both_directions` (`:2906`).
- **Flowable refusals:** `unsupported_loop_until_property` (`validate.rs:1526-1537`), `unsupported_allow_failure_retry` (`:1639-1649`), `unsupported_pause_property` (`:1664-1677`); `executor::pause_gap` (`executor/plan.rs:860-921`) answers `tasks`, `errors`, `behavior`, `onPause`. The REFUSED fixture table in `validate.rs`'s tests (`:4843-4870`) pins codes.
- **The measured tables:** `leaf_handler_fields` (`spec/schema/measure.rs:105-150`), `flowable_fields` (`:152-167`), `trigger_fields` (`:169-176`). `measure` is `#[cfg(test)] mod measure;` (`spec/schema/mod.rs:55-56`). `wire_fields_of` is production (`spec/schema/mod.rs:820`). `Treatment` and `served()` are at `spec/schema/mod.rs:131-150`. `classify` reads `not_yet_enforced` only for `warned` (`measure.rs:708-712`). `REFUSED_BY_NAME` (`:77`). The committed-file test is `the_committed_property_treatment_is_what_egret_does_today` (`measure.rs:860`); `EGRET_WRITE_PROPERTY_TREATMENT=1` rewrites the file. `tests/flow_schema.rs:389` `no_served_property_is_one_egret_ignores_or_refuses` checks the served schema against it.
- **The model:** `Task`'s deserializer reads `id type description retry timeout disabled runIf allowFailure allowWarning` (`models/task.rs:764-781`); `Task::children`/`error_children` (`:122-201`); `RetryCommon` (`:697-705`), `Retry` (`:723`), `DagNode` (`:422`), `CheckFrequency` (`:514`). `Trigger` has a hand-written deserializer (`models/trigger.rs:363-400`); `Schedule` (`:110`), `Webhook` (`:168`), `FlowTrigger` (`:217`), `Window` is `deny_unknown_fields` (`:266`). `Input` (`models/input.rs:54-96`). `Flow` (`models/flow.rs:39-134`), `Concurrency` (`:164`), `FlowOutput` (`:196-214`), `Check` (`:299`), `Quota` (`:341`).
- **Paths:** `spec::path::{root_lists, task_path, every_task}` (`spec/path.rs`), `{prefix}.{id}`.
- **Scripts:** `HANDLED_BY_TASK_MODEL` (`scripts/mod.rs:182-191`), `COMMON_REFUSED` (`:193-217`), `PYTHON_REFUSED` (`:219-228`), the not-implemented message (`:507-517`), runner property messages (`:704-717`, `:798`, `:832-835`, `:863-866`). `outputFiles` pattern messages: `worker/commands.rs:3108-3124`.
- **Plugins:** `HANDLED_BY_TASK_MODEL` (`plugins/mod.rs:208`), `REFUSED_BASE_PROPERTIES` (`:223`), `taskRunner` and base-property messages (`:358-378`). Pins: `tests/plugins_gate.rs:266`, `tests/publish.rs:770`.
- **Trigger refusals after publish:** `ScheduleTrigger::resolve` (`trigger/schedule.rs:1343-1372`; `when` at `:1354`, `lateMaximumDelay` at `:1368`), Display (`:1285-1300`). `webhook::declared_unsupported` (`trigger/webhook.rs:345-360`) and `Unsupported::message` (`:364-395`).
- **Flow-trigger scope:** `flow_trigger/process.rs:575-585` selects listeners `WHERE l.workspace_id = $1`.
- **Oracle:** `publish_corpus` runs `publish::publish_gated` (`oracle-exec/driver.rs:334-460`); census constants `LIVE` (`:853`), `UNKNOWN_TYPE_REFUSED` (`:931`), `VALIDATE_REFUSED` (`:979`). The ratchet's refused verdict is `build_plan`'s (`driver.rs:1168-1174`). `tests/executor.rs`: `egret_side` (`:1452-1520`), `executor_matches_golden` (`:2009`), `REFUSAL_GROUPS` (`:6492-…`), `STILL_OVER_ACCEPTED` (`:6648-…`), `UNATTRIBUTED` (`:7084-…`). `PUBLISHED_BUT_REFUSED` (`executor/mod.rs:4249`).
- **Baseline on main:** ratchet 301 golden, 73 mismatching, 73 baselined, 0 broken; `LIVE` 295, `VALIDATE_REFUSED` 25, `UNKNOWN_TYPE_REFUSED` 21; `PUBLISHED_BUT_REFUSED` 6.
- **Corpus hits** (census scan, `scratchpad/nodrops/hits.json`): leaf unknown key 1 (`invalids/invalid-property`); `logLevel` 1, `logToFile` 1, `taskCache` 1; `warningOnRetry` 4; Subflow `labels` 4, `inheritLabels` 6; `Pause` `onResume` 3; `Loop` `outputs` 8 hits in 6 flows, `fetchType` 1; flow `draft` 2, `revision` 2; Webhook `labels` 1; input `values` 7, `validator` 3, `jsonSchema` 1, `min`/`max`/`after`/`before` in `valids/inputs`. Not scanned: cross-type input keys, nested beans.

## Addendum 2026-10-26: rebased onto main `28fb8693` (spec D17)

Main moved 56 commits. #747 shipped `spec/properties.rs` (`property_warnings`) and `validate.rs` `schedule_findings`. Read spec D17 first. Changes to the tasks below:
- **Task 2** also deletes `spec/properties.rs`, removes `pub use properties::property_warnings` and replaces the three `validate(..)` + `property_warnings(..)` pairs (`publish.rs:311-317`, `routes.rs:527-533`, `routes.rs:1021-1027`) with `validate_source`. The tests in `properties.rs` move to `keys.rs` with the new codes (`unknown_property` error, `not_honoured` warning). Update the doc text in `spec/schema/README.md:143` and `spec/schema/mod.rs:142`.
- **Task 3 and Task 6** (`warningOnRetry`): `NO_EFFECT` already marks it `ignored` in `measure.rs`; the `not_honoured` warning names #13. `the_committed_property_treatment_is_what_egret_does_today` must stay green.
- **Task 5**: Schedule `lateMaximumDelay` / `when` are covered by `schedule_not_resolvable`; reword that message to name #11 and add no second warning.
- Every `file:line` anchor in the plan is from `309405b1`: re-grep before editing.
- Commit trailer: `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` (replaces the one in Global Constraints).

## Global Constraints

- **The private stack only.** Every cargo command runs with exactly:

  ```bash
  export DATABASE_URL=postgres://egret:egret@localhost:5457/egret_test
  export EGRET_TEST_SHARED_STORE_URL=redis://localhost:6457
  export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-nodrops/target
  unset EGRET_ALLOW_DB_SKIP EGRET_ALLOW_STORE_SKIP
  ```

  Never port 5432, never `deploy/docker-compose.dev.yml`, never an `egret-dev-*` container. Never set any `EGRET_ALLOW_*_SKIP`: a DB-gated test that returns early has asserted nothing.
- **One cargo command at a time.** Never `pkill` cargo. `rg` is not installed: use `grep -rn`.
- **The oracle ratchet stays green after every task:** `cargo test -p egret-workflow --test executor`. Record before and after in the task report: golden, mismatching, baselined, broken, `LIVE`, `VALIDATE_REFUSED`, `UNKNOWN_TYPE_REFUSED`. A census constant changes only in the task that moves it, with a dated doc line naming the flow(s). `oracle-exec/golden.json` is never edited.
- **Nothing new runs at run time.** No change to `validate_for_run`, `build_plan`, `parse_for_plan` or any worker path, except the message texts of Task 8.
- **Messages are verbatim** as the spec's §3–§5 give them. Each new message is egret's own unless the spec says it is Kestra's, and its doc comment says which, with Kestra's file:line.
- **Run only the tests for what you touch.** The full suites run once, at the final gate.
- **Mutation discipline.** For each new rule, break it once by hand, see a named test go red, restore it. The task report records the command and the red line.
- **Commits** follow Conventional Commits and end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never commit `.superpowers/` or anything under the scratchpad.

## File map

| File | Responsibility | Task |
|---|---|---|
| `src/spec/keys.rs` (new) | key tables, `Lift`, `lift_of`, `findings`, the walker | 1, 3–7 |
| `src/spec/mod.rs` | `mod keys`; re-export `validate_source` | 1, 2 |
| `src/spec/validate.rs` | `validate_source`; reworded messages (Task 8) | 2, 8 |
| `src/spec/schema/measure.rs` | import the tables; `run` calls `validate_source`; `a_not_honoured_warning_is_not_served` | 1, 2 |
| `src/spec/schema/property-treatment.json` | regenerated (two rows, `onChildFailure`) | 4 |
| `src/publish.rs`, `src/routes.rs` | call `validate_source` | 2 |
| `oracle-exec/driver.rs`, `tests/executor.rs`, `oracle-exec/README.md` | census `LIVE`/`VALIDATE_REFUSED`; `invalid-property` causes | 3 (8b) |
| `src/scripts/mod.rs`, `src/plugins/mod.rs`, `src/worker/commands.rs` | messages from `lift_of` | 8 |
| `src/trigger/schedule.rs`, `src/trigger/webhook.rs` | messages name #11 | 8 |
| `src/executor/plan.rs` | `pause_gap` doc for `tasks` | 8 |
| `tests/publish.rs`, `tests/routes.rs` | publish and draft show the findings | 3, 4 |
| the 345 sweep files (spec §8) | 345 → 351 | 9 |
| docs (EN, VI) | spec status, roadmap corrections, schema README, oracle README, programme §9 | 10 |

---

### Task 1: the key tables in production, and the lift table (D12)

**Files:**
- Create: `src/spec/keys.rs`.
- Modify: `src/spec/mod.rs`: `pub(crate) mod keys;`.
- Modify: `src/spec/schema/measure.rs`: delete `leaf_handler_fields`, `flowable_fields`, `trigger_fields` (`:105-176`) and call `crate::spec::keys::{leaf_keys, flowable_keys, trigger_keys}`; `flowable_keys`/`trigger_keys` panic on an unknown name exactly as the moved functions did.

**Interfaces (produced, used by Tasks 2–8):**

```rust
/// What `models::Task`'s deserializer reads itself (`models/task.rs:764-781`).
pub(crate) const TASK_MODEL_KEYS: &[&str] = &[
    "id", "type", "description", "retry", "timeout", "disabled", "runIf", "allowFailure", "allowWarning",
];
/// Kestra `Task.java` keys egret's `Task` does not read (`k/core/models/tasks/Task.java:45,68,74,77,85,97,102`).
pub(crate) const TASK_JAVA_UNREAD: &[&str] = &[
    "version", "workerSelector", "policyRefs", "logLevel", "logToFile", "taskCache", "assets",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lift {
    /// A roadmap package (`docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md` §2).
    Package(u8),
    /// Kestra Enterprise only; Kestra OSS ignores it too (roadmap §3.3).
    Enterprise,
    /// egret design kept (roadmap §3.2); the text says what replaces the key.
    KeptDesign(&'static str),
}

impl Lift {
    /// "roadmap package #13", "Kestra Enterprise only, ignored by Kestra OSS too, roadmap §3.3", …
    pub(crate) fn short(self) -> String;
    /// "roadmap package #13 lifts it", "it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap §3.3)", …
    pub(crate) fn sentence(self) -> String;
}

/// Where a key sits, for `lift_of`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Site<'a> {
    Flow,
    /// A task of this canonical type (core leaf, flowable, script or plugin).
    Task(&'a str),
    /// A typed trigger of this canonical type.
    Trigger(&'a str),
    /// An input of this `InputType` wire name ("STRING", …).
    Input(&'a str),
}

/// The package that lifts a key Kestra accepts at `site` and egret does not act on, or `None`.
pub(crate) fn lift_of(site: Site<'_>, key: &str) -> Option<Lift>;

pub(crate) fn leaf_keys(canonical: &str) -> Option<Vec<&'static str>>;   // moved, unchanged content
pub(crate) fn flowable_keys(canonical: &str) -> Vec<&'static str>;       // moved
pub(crate) fn trigger_keys(canonical: &str) -> Vec<&'static str>;        // moved
```

`lift_of`'s rows are exactly the spec's §3 tables: Task.java keys (§3.2/§3.3: `logLevel`, `logToFile`, `taskCache`, `version`, `assets` → `Package(13)`; `workerSelector`, `policyRefs` → `Enterprise`), `retry.warningOnRetry` is not a `lift_of` row (it is modelled; Task 3 handles it), `Loop` `outputs`/`fetchType` → 6, the five Subflow keys → 12, `Pause` `onResume` → 9, the trigger keys of §3.4 (11, 13, `Enterprise`), the input keys of §3.5 (3, `Enterprise`), the flow keys of §3.1 (`workerSelector`/`policyRefs` → `Enterprise`, `draft` → `KeptDesign(..)`), and the script-only keys for Task 8 (`runner`, `targetOS`, the seven `PYTHON_REFUSED` keys → 17; `taskRunner` on a plugin → 18).

- [ ] **Step 1: write the failing tests** in `keys.rs`'s `#[cfg(test)] mod tests`:

```rust
/// **One table names the package for every key** (spec D12). Spot rows from
/// each site, so a row moved to the wrong package fails here by name.
#[test]
fn lift_of_names_the_package_the_roadmap_gives() {
    use Lift::*;
    let log = "io.kestra.plugin.core.log.Log";
    let sub = "io.kestra.plugin.core.flow.Subflow";
    assert_eq!(lift_of(Site::Task(log), "logLevel"), Some(Package(13)));
    assert_eq!(lift_of(Site::Task(log), "workerSelector"), Some(Enterprise));
    assert_eq!(lift_of(Site::Task(sub), "inheritLabels"), Some(Package(12)));
    assert_eq!(lift_of(Site::Task("io.kestra.plugin.core.flow.Loop"), "outputs"), Some(Package(6)));
    assert_eq!(lift_of(Site::Task("io.kestra.plugin.core.flow.Pause"), "onResume"), Some(Package(9)));
    assert_eq!(lift_of(Site::Trigger("io.kestra.plugin.core.trigger.Schedule"), "stopAfter"), Some(Package(11)));
    assert_eq!(lift_of(Site::Input("SELECT"), "values"), Some(Package(3)));
    assert_eq!(lift_of(Site::Flow, "policyRefs"), Some(Enterprise));
    // Not lifted by #1's table: honoured, #2's, or unknown to Kestra.
    assert_eq!(lift_of(Site::Task(log), "message"), None);
    assert_eq!(lift_of(Site::Task("io.kestra.plugin.core.flow.Sequential"), "finally"), None);
    assert_eq!(lift_of(Site::Task("io.kestra.plugin.core.flow.Parallel"), "onChildFailure"), None);
    assert_eq!(lift_of(Site::Task(log), "invalid"), None);
}

/// **The pin governs, and the rc13 export is the second instrument** (spec
/// D12). For every typed row the committed schema export defines, each
/// Kestra property is honoured by egret's model, lifted by `lift_of`, or one
/// of the named differences. A Kestra property in none of the three is a
/// silent drop this package missed.
#[test]
fn every_kestra_property_is_read_lifted_or_a_named_difference() {
    // (row, key, why) -- rc13 is newer than the pin (2026-09-01 vs 2026-08-17).
    const DIFFERENCES: &[(&str, &str, &str)] = &[
        ("io.kestra.plugin.core.flow.Parallel", "onChildFailure", "not at the pin; unknown_property"),
        ("io.kestra.plugin.core.flow.Dag", "onChildFailure", "not at the pin; unknown_property"),
    ];
    /// #2's keys (Rule 1): neither read, lifted nor refused by #1.
    const TWO: &[&str] = &["finally", "errors"];
    use crate::spec::schema::{kestra_definition, FLOWABLE_TYPES, TRIGGER_TYPES};
    let rows = FLOWABLE_TYPES
        .iter()
        .map(|(t, _)| (*t, Site::Task(t), flowable_keys(t)))
        .chain(TRIGGER_TYPES.iter().map(|(t, _)| {
            (*t, Site::Trigger(t), [trigger_keys(t), TRIGGER_BASE_KEYS.to_vec()].concat())
        }))
        .chain(crate::spec::registry::TASK_TYPES.iter().filter_map(|s| {
            leaf_keys(s.r#type).map(|k| (s.r#type, Site::Task(s.r#type), k))
        }));
    let mut missed = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (row, site, egret) in rows {
        let Some(props) = kestra_definition(row).and_then(|d| d["properties"].as_object()) else {
            continue;
        };
        for k in props.keys().map(String::as_str) {
            if egret.contains(&k) || TASK_MODEL_KEYS.contains(&k) || lift_of(site, k).is_some() {
                continue;
            }
            if matches!(site, Site::Task(_)) && TWO.contains(&k) {
                continue;
            }
            if DIFFERENCES.iter().any(|(r, p, _)| *r == row && *p == k) {
                seen.insert((row, k));
                continue;
            }
            missed.push(format!("{row}.{k}"));
        }
    }
    assert!(missed.is_empty(), "Kestra properties egret neither reads nor lifts: {missed:#?}");
    assert_eq!(seen.len(), DIFFERENCES.len(), "a DIFFERENCES row the export no longer has is stale");
}
```

  `TRIGGER_BASE_KEYS` (the base keys `models::Trigger`'s deserializer reads, `models/trigger.rs:363-400`, copied exactly) is defined in this task too, so Task 5 only uses it. Check the names `registry::TASK_TYPES`, `TaskTypeSpec::r#type` and `kestra_definition`'s signature against the code; the logic stays as written. Input rows are not in this cross-check (the export names input subtypes differently); Task 5's tests pin them.
- [ ] **Step 2: run, expect a compile failure** (no `keys` module):

```bash
cargo test -p egret-workflow --lib -- spec::keys
```

- [ ] **Step 3: implement** `keys.rs` with the tables and `lift_of`, move the three functions, and write the cross-check body. Run `cargo test -p egret-workflow --lib -- spec::keys spec::schema` and `cargo test -p egret-workflow --test flow_schema`. Expect PASS, and `property-treatment.json` unchanged (the move is behaviour-free).
- [ ] **Step 4: mutation.** Delete the `("io.kestra.plugin.core.flow.Loop", "fetchType")` row from `lift_of`; see `every_kestra_property_is_read_lifted_or_a_named_difference` red naming `Loop.fetchType`. Restore.
- [ ] **Step 5: commit** `refactor(workflow): key tables and the lift table move to spec::keys`.

---

### Task 2: one save-time entry point, `validate_source` (D11, D3)

**Files:**
- Modify: `src/spec/validate.rs`: add `validate_source` next to `validate`.
- Modify: `src/spec/mod.rs`: `pub use validate::{validate_source, …}`.
- Modify: `src/spec/keys.rs`: `pub(crate) fn findings(source: &str, flow: &Flow, report: &mut ValidationReport)`, empty body that parses `source` to a `serde_json::Value` (Task 3 onward fills it).
- Modify: `src/publish.rs:311`, `src/routes.rs:513` (`draft_dto` has `row.source`), `src/routes.rs:899` (`validate_flow_source` has `source`): call `validate_source(&source, &parsed)`.
- Modify: `src/spec/schema/measure.rs` `run` (`:227-260`): `validate_source(&source, &f)`; add `a_not_honoured_warning_is_not_served`.

**Interfaces:**

```rust
/// **The save-time report** with the key census of package #1 (spec D11):
/// [`validate`] plus [`crate::spec::keys::findings`], which reads the raw
/// source for the keys serde dropped. Every caller that reports on a SAVED
/// or PUBLISHING flow calls this; the run-time gate never does
/// (`executor::build_plan` calls [`validate_for_run`]), so a version
/// published before #1 keeps running.
pub fn validate_source(source: &str, flow: &Flow) -> ValidationReport {
    let mut report = validate(flow);
    crate::spec::keys::findings(source, flow, &mut report);
    report
}
```

- [ ] **Step 1: write the failing tests.** In `validate.rs`'s tests, a source scan:

```rust
/// **No production caller reports on a saved flow without the key census**
/// (spec D11). `validate(&Flow)` stays public for tests and for
/// `validate_source` itself; a production call to it outside `spec/` would
/// publish a flow whose unknown keys nobody checked.
#[test]
fn every_production_save_path_calls_validate_source() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut offenders = Vec::new();
    for path in walk_rs(root) {
        if path.contains("/spec/") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let prod = text.split("#[cfg(test)]").next().unwrap();
        // `spec`'s `validate` reaches a file either by path or by a `use`;
        // other functions named `validate` (`runners/javaformat.rs:255`,
        // `worker/k8s/pod_spec.rs:150`) are not it.
        let imported = prod
            .lines()
            .any(|l| l.trim_start().starts_with("use crate::spec::") && l.contains("validate") && !l.contains("validate_source"));
        for (n, line) in prod.lines().enumerate() {
            let code = line.trim_start();
            if code.starts_with("//") || code.contains("fn validate") {
                continue;
            }
            let by_path = code.contains("spec::validate(&") || code.contains("validate::validate(&");
            let by_use = imported && (code.contains(" validate(&") || code.contains("(validate(&"));
            if by_path || by_use {
                offenders.push(format!("{path}:{}: {code}", n + 1));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}
```

  (`walk_rs` is a small recursive `std::fs::read_dir` helper in the test module returning every `.rs` path under `root`; `validate_for_run(&` and `validate_source(&` do not match.) After Step 3, `publish.rs` and `routes.rs` import `validate_source` and no longer `validate`; if one still needs `validate` for another reason, the scan names it and the reason goes in a doc line. In `measure.rs`'s tests:

```rust
/// **A `not_honoured` warning does not serve a key** (spec D3): `classify`
/// reads `not_yet_enforced` only, so a key #1 warns on stays `ignored`, and
/// the editor does not start offering it.
#[test]
fn a_not_honoured_warning_is_not_served() {
    let row = "io.kestra.plugin.core.log.Log";
    let flow = json!({"id": "f", "namespace": "a.b",
        "tasks": [{"id": "t", "type": row, "message": "m", "logLevel": "INFO"}]});
    let o = run(&flow);
    assert!(o.not_yet_enforced.is_empty(), "{:?}", o.not_yet_enforced);
    assert_eq!(super::property_treatment()[row].get("logLevel"), Some(&Treatment::Ignored));
}
```

  This test is green before Task 3 too (no `not_honoured` exists yet); its red run is the mutation in Task 3 Step 4 (c): give the core-leaf warning the code `not_yet_enforced`, regenerate nothing, and see this test and `the_committed_property_treatment_is_what_egret_does_today` go red. Restore.
- [ ] **Step 2: run, expect FAIL** (the scan finds `publish.rs:311` and `routes.rs:513,899`; `validate_source` does not exist):

```bash
cargo test -p egret-workflow --lib -- spec::validate::tests::every_production_save_path spec::schema
```

- [ ] **Step 3: implement** the entry point and the three call sites, and `measure.rs`'s `run`. Run `cargo test -p egret-workflow --lib -- spec`, `--test publish`, `--test routes -- validate`, `--test flow_schema`, then the ratchet. Expect PASS with no count moved (`findings` adds nothing yet).
- [ ] **Step 4: mutation.** Point `routes.rs:513` back at `validate(&parsed)`; see the scan red with that line. Restore.
- [ ] **Step 5: commit** `refactor(workflow): one save-time entry point, validate_source`.

---

### Task 3: core leaf tasks — unknown keys refused, Task.java keys and `warningOnRetry` warned

**Files:**
- Modify: `src/spec/keys.rs`: `findings` calls `core_leaves(flow, report)` (model-based) and `warning_on_retry(flow, report)`.
- Modify: `oracle-exec/driver.rs`: `LIVE` 295 → 294 and `VALIDATE_REFUSED` 25 → 26, **if measured** (Step 5), each with a dated doc line naming `invalids/invalid-property`.
- Modify: `tests/executor.rs`: `STILL_OVER_ACCEPTED`'s and `UNATTRIBUTED`'s cause for `invalids/invalid-property` (spec §6.2).
- Modify: `oracle-exec/README.md:3550-3551`: the same cause, dated.
- Test: `keys.rs` unit tests; `tests/publish.rs`.

**Rules** (spec §3.2): for every task in `spec::path::every_task(flow)` whose kind is `TaskKind::Leaf { spec }` and whose type has `leaf_keys(canonical)` (`registry::known_type(&task.r#type)` gives the canonical name, aliases included):
- key in `leaf_keys` or `TASK_MODEL_KEYS`: nothing;
- key in `TASK_JAVA_UNREAD`: `not_honoured`, message `` task '<id>': property '<key>' is accepted by Kestra on <canonical> but egret does not act on it yet; <lift.sentence()> ``, path `<task path>.<key>`; skipped on a non-runnable type, where Task 7's Kestra warning covers `taskCache`/`workerSelector`;
- any other key, `null` included: `unknown_property`, `` task '<id>': unknown property '<key>' on <canonical>: neither Kestra nor egret accepts it ``, path `<task path>.<key>`.

`warningOnRetry: true` on any task's `retry` or on the flow's: `not_honoured`, `` `retry.warningOnRetry` is accepted by Kestra but egret does not act on it yet: a retried task that succeeds ends SUCCESS, not WARNING; roadmap package #13 lifts it ``, path `<task path>.retry.warningOnRetry` or `retry.warningOnRetry`.

- [ ] **Step 1: write the failing tests** in `keys.rs`:

```rust
use std::collections::{BTreeMap, BTreeSet};

/// The save-time report every test below reads (spec D11).
fn report(src: &str) -> ValidationReport {
    crate::spec::validate_source(src, &crate::spec::parse(src).unwrap())
}

/// `invalids/invalid-property`, Kestra's `Unrecognized field "invalid"`
/// (`YamlParser.java:144-157`): refused at save, by name, at its path.
#[test]
fn an_unknown_key_on_a_core_task_is_refused() {
    let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: io.kestra.plugin.core.debug.Return\n    invalid: x\n");
    let e = r.errors.iter().find(|e| e.code == "unknown_property").unwrap_or_else(|| panic!("{r:?}"));
    assert_eq!(e.path, "tasks.t.invalid");
    assert_eq!(e.message, "task 't': unknown property 'invalid' on io.kestra.plugin.core.debug.Return: neither Kestra nor egret accepts it");
}

/// The alias spelling is checked against the canonical row, and a typo of a
/// real key (`mesage`) is the case the check exists for.
#[test]
fn a_typo_on_an_aliased_log_is_refused() {
    let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    mesage: hi\n");
    assert!(r.errors.iter().any(|e| e.code == "unknown_property" && e.path == "tasks.t.mesage"), "{r:?}");
}

/// Kestra accepts `logLevel` (`Task.java:77`); egret does not act on it, so
/// it is a warning naming #13 and the flow still publishes.
#[test]
fn a_task_java_key_on_a_core_task_is_warned_with_its_package() {
    let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n    logLevel: INFO\n");
    assert!(r.errors.is_empty(), "{r:?}");
    let w = r.warnings.iter().find(|w| w.code == "not_honoured").unwrap_or_else(|| panic!("{r:?}"));
    assert_eq!(w.path, "tasks.t.logLevel");
    assert!(w.message.ends_with("roadmap package #13 lifts it"), "{}", w.message);
}

/// An unknown key is refused at SAVE only: the run gate still plans it, so a
/// version published before #1 keeps running (spec D1).
#[test]
fn an_unknown_key_does_not_stop_the_run_gate() {
    let src = "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: io.kestra.plugin.core.debug.Return\n    format: x\n    invalid: x\n";
    assert!(crate::executor::build_plan(src).is_ok());
}

#[test]
fn warning_on_retry_is_warned_on_a_task_and_on_the_flow() {
    let r = report("id: f\nnamespace: a.b\nretry:\n  type: constant\n  interval: PT1S\n  warningOnRetry: true\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n    retry:\n      type: constant\n      interval: PT1S\n      warningOnRetry: true\n");
    let paths: BTreeSet<_> = r.warnings.iter().filter(|w| w.code == "not_honoured").map(|w| w.path.as_str()).collect();
    assert_eq!(paths, BTreeSet::from(["retry.warningOnRetry", "tasks.t.retry.warningOnRetry"]));
}
```

  And in `tests/publish.rs`, one DB test that a draft with `invalid:` on a `Return` is refused by `publish` with the message above, and one that a draft with `logLevel: INFO` publishes with the warning in `snapshot["warnings"]` (copy the setup of `a_subflow_absent_from_the_callers_workspace_publishes_with_the_warning_whoever_has_it`, `tests/publish.rs:308`).
- [ ] **Step 2: run, expect FAIL**: `cargo test -p egret-workflow --lib -- spec::keys` and `cargo test -p egret-workflow --test publish -- unknown_property not_honoured`.
- [ ] **Step 3: implement.** Run the unit tests, `--test publish`, `--test routes`, `--lib -- spec`.
- [ ] **Step 4: mutation.** (a) Remove `TASK_JAVA_UNREAD` from the warn arm so `logLevel` falls through to unknown; see `a_task_java_key_on_a_core_task_is_warned_with_its_package` red. (b) Call `keys::findings` from `validate_for_run` with the flow re-serialised as its source; see `an_unknown_key_does_not_stop_the_run_gate` red. (c) Give the core-leaf warning the code `not_yet_enforced`; see `a_not_honoured_warning_is_not_served` (Task 2) and `the_committed_property_treatment_is_what_egret_does_today` red. Restore all three.
- [ ] **Step 5: measure the census.** Run `cargo test -p egret-workflow --test executor`. Expected: the census assert fails with `LIVE` 294 and `VALIDATE_REFUSED` 26, and the `[publish_corpus] validate refused` printout names exactly `invalids/invalid-property` with `unknown_property`. **Measure, do not assume:** if another flow appears, stop and report it (spec §6.2). Then update `LIVE` and `VALIDATE_REFUSED` with dated doc lines, and the two cause texts, and re-run until green. The ratchet must read 301 / 73 / 73 / 0.
- [ ] **Step 6: commit** `feat(workflow): unknown properties on core tasks are refused at publish, as Kestra; unread Task keys warn with their package`.

---

### Task 4: the flow's own keys and flowables, from the source

**Files:**
- Modify: `src/spec/keys.rs`: `findings` parses the source and calls `flow_keys(raw, report)` and `walk_tasks(raw, flow, report)`.
- Modify: `src/spec/schema/property-treatment.json`: regenerate (Step 5).
- Test: `keys.rs`; `tests/routes.rs` (one draft-DTO case).

**Rules:** spec §3.1 and §3.3. Allowed flow keys: `wire_fields_of::<Flow>()` plus the four server-managed keys (D15). Allowed flowable keys: `flowable_keys(canonical) ∪ TASK_MODEL_KEYS ∪ {"finally", "errors"}` (the last two skipped by name, spec §1), minus nothing. Pause `tasks` is in `flowable_keys(Pause)` and is left to `pause_gap` (D14). The walker pairs model and raw tasks as spec §2.4 says; the per-kind raw child lists:

| kind | raw lists, in `children()` then `error_children()` order |
|---|---|
| Sequential, Parallel, Loop, LoopUntil, WorkingDirectory | `tasks`; then `errors` where the model has it |
| If | `then`, `else`; `errors` |
| Switch | each `cases.<k>` in the model's key order, then `defaults`; `errors` |
| Dag | `tasks[i].task`; `errors` |
| AllowFailure | `tasks`, `finally`; `errors` |
| Pause | `tasks`, `onPause` (one task); `errors` — read `Task::children` for `Pause` at `models/task.rs:146-…` and copy its exact order |
| Subflow, Leaf | none |

A raw entry whose `id` differs from the model child's `id` ends that subtree with no finding.

- [ ] **Step 1: write the failing tests:**

```rust
#[test]
fn an_unknown_top_level_key_is_refused_and_a_server_managed_one_is_not() {
    let r = report("id: f\nnamespace: a.b\nrevision: 7\nlabelz: {a: b}\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n");
    let unknown: Vec<_> = r.errors.iter().filter(|e| e.code == "unknown_property").map(|e| e.path.as_str()).collect();
    assert_eq!(unknown, ["labelz"]);
}

#[test]
fn flow_draft_and_worker_selector_warn_with_their_reason() {
    let r = report("id: f\nnamespace: a.b\ndraft: true\nworkerSelector: {tags: [x]}\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n");
    assert!(r.errors.is_empty(), "{r:?}");
    let w: BTreeMap<_, _> = r.warnings.iter().filter(|w| w.code == "not_honoured").map(|w| (w.path.as_str(), w.message.as_str())).collect();
    assert!(w["draft"].ends_with("(roadmap §3.2)"), "{w:?}");
    assert!(w["workerSelector"].contains("Kestra Enterprise"), "{w:?}");
}

/// Rule 1: `finally:` on a flowable and `errors:` on `Loop` go to #2, which
/// is next, so #1 says nothing about them -- not a refusal, not a warning.
#[test]
fn finally_and_the_errors_two_takes_draw_nothing() {
    let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n    finally:\n      - id: b\n        type: core.log\n        message: b\n  - id: l\n    type: io.kestra.plugin.core.flow.Loop\n    values: [1]\n    tasks:\n      - id: c\n        type: core.log\n        message: c\n    errors:\n      - id: d\n        type: core.log\n        message: d\n");
    assert!(r.errors.is_empty() && !r.warnings.iter().any(|w| w.code == "not_honoured" || w.code == "unknown_property"), "{r:?}");
}

#[test]
fn flowable_keys_kestra_has_warn_with_their_package_and_others_are_refused() {
    let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Subflow\n    namespace: x.y\n    flowId: g\n    inheritLabels: true\n  - id: p\n    type: io.kestra.plugin.core.flow.Parallel\n    onChildFailure: FAILED\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n        mesage: typo\n");
    assert!(r.warnings.iter().any(|w| w.code == "not_honoured" && w.path == "tasks.s.inheritLabels" && w.message.ends_with("roadmap package #12 lifts it")), "{r:?}");
    let unknown: BTreeSet<_> = r.errors.iter().filter(|e| e.code == "unknown_property").map(|e| e.path.as_str()).collect();
    assert_eq!(unknown, BTreeSet::from(["tasks.p.onChildFailure", "tasks.p.a.mesage"]));
}

#[test]
fn pause_on_resume_warns_and_pause_tasks_is_refused_once() {
    let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    onResume:\n      - id: ok\n        type: BOOL\n");
    assert!(r.errors.is_empty(), "{r:?}");
    assert!(r.warnings.iter().any(|w| w.path == "tasks.p.onResume" && w.message.ends_with("roadmap package #9 lifts it")), "{r:?}");
    let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n");
    assert_eq!(r.errors.iter().filter(|e| e.path == "tasks.p.tasks").count(), 1, "{r:?}");
}

/// The instrument fails open, and no corpus flow reaches that branch: every
/// model task of every corpus flow pairs with a raw map whose `id` matches.
#[test]
fn every_corpus_task_pairs_with_its_raw_map() {
    let root = format!("{}/oracle-exec/corpus", env!("CARGO_MANIFEST_DIR"));
    let mut unpaired = Vec::new();
    let mut tasks = 0usize;
    for dir in ["valids", "invalids"] {
        for entry in std::fs::read_dir(format!("{root}/{dir}")).unwrap() {
            let path = entry.unwrap().path();
            let src = std::fs::read_to_string(&path).unwrap();
            let Ok(flow) = crate::spec::parse(&src) else { continue };
            tasks += crate::spec::path::every_task(&flow).len();
            unpaired.extend(
                super::unpaired(&src, &flow)
                    .into_iter()
                    .map(|p| format!("{}: {p}", path.display())),
            );
        }
    }
    assert!(unpaired.is_empty(), "{unpaired:#?}");
    // A zero would satisfy the assertion above while measuring nothing.
    assert!(tasks >= 300, "only {tasks} corpus tasks were walked");
}
```

  `super::unpaired(source, &flow) -> Vec<String>` is a `pub(crate)` function of the walker that returns the path of every model task with no raw partner. Record the measured `tasks` count in the report.
- [ ] **Step 2: run, expect FAIL**: `cargo test -p egret-workflow --lib -- spec::keys`.
- [ ] **Step 3: implement.** First measure D15's open point: read Kestra's flow create path (`FlowService.create`, `k/core/services/FlowService.java:126-148`, and the JDBC `FlowRepository.create` it calls) for what a source `deleted: true` does. If Kestra ignores it, `deleted` stays in the silent set; if Kestra acts on it, it becomes a `lift_of(Site::Flow, "deleted")` row with the package that owns it and a `not_honoured` test. Record the finding with file:line. Then add one `tests/routes.rs` case: `GET` a draft whose source has `labelz:` shows `unknown_property` in `validation.errors` (copy an existing draft-validation route test).
- [ ] **Step 4: mutation.** (a) Drop `"finally"` from the skipped set; see `finally_and_the_errors_two_takes_draw_nothing` red. (b) Pair by position without the id check, and give one test a model/raw mismatch via a `Switch` whose cases the model reorders; see the pairing test or a misplaced path red. Restore.
- [ ] **Step 5: regenerate `property-treatment.json`:** `EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow --lib -- spec::schema::measure`, then `git diff --stat` and `git diff` on the file. Expected: exactly `Parallel.onChildFailure` and `Dag.onChildFailure`, `ignored` → `refused`. Any other row moving is reported and explained, or the change is wrong. Re-run without the variable, and `--test flow_schema`.
- [ ] **Step 6: ratchet.** Expect 301 / 73 / 73 / 0, `LIVE` 294, `VALIDATE_REFUSED` 26 (no change from Task 3). A move is a finding: stop and report.
- [ ] **Step 7: commit** `feat(workflow): unknown flow and flowable keys refused at publish; Kestra keys egret ignores warn with their package`.

---

### Task 5: typed triggers, inputs and outputs (D6)

**Files:** `src/spec/keys.rs` (`trigger_keys_findings`, `input_findings`, `output_findings`); tests in `keys.rs`.

**Rules:** spec §3.4 and §3.5.
- `TRIGGER_BASE_KEYS` const: `id type description when disabled labels stopAfter logToFile failOnTriggerError allowConcurrent` (read the deserializer at `models/trigger.rs:363-400` and copy exactly what it reads).
- `stopAfter`, `failOnTriggerError` present → warn #11; `allowConcurrent: false` written → warn #11 with `egret never locks a trigger while its execution runs`; `labels` on Schedule/Webhook, `when` on Webhook → warn #11.
- Schedule: if `ScheduleTrigger::resolve(t)` is `Err(UnsupportedLateMaximumDelay(_) | UnsupportedWhenCondition(_))`: warn at `triggers.<id>.lateMaximumDelay` / `.when`, `` trigger '<id>': <the error's Display>; egret refuses it when it syncs the trigger, so this Schedule trigger will not fire. Roadmap package #11 lifts it ``. Other `resolve` errors: nothing new (they are validate's already, or not #1's).
- Webhook: if `webhook::declared_unsupported(w)` is `Some(u)`: warn at `triggers.<id>.<key>`, `` <u.message(id)>; every call to this webhook answers 409. Roadmap package #11 lifts it ``.
- Inputs: `input_keys(type)`; a key Kestra has on this type and egret drops → `lift_of(Site::Input(type), key)`; any other → `unknown_property`, `` input '<id>': unknown property '<key>' on a <TYPE> input: neither Kestra nor egret accepts it ``, path `inputs.<id>.<key>`.
- Outputs: `wire_fields_of::<FlowOutput>()`; others `unknown_property` at `outputs.<id>.<key>`.

- [ ] **Step 1: write the failing tests:**

```rust
#[test]
fn a_late_maximum_delay_warns_at_publish_that_the_schedule_will_not_fire() {
    let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\ntriggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"0 * * * *\"\n    lateMaximumDelay: PT1H\n");
    assert!(r.errors.is_empty(), "{r:?}");
    let w = r.warnings.iter().find(|w| w.path == "triggers.s.lateMaximumDelay").unwrap_or_else(|| panic!("{r:?}"));
    assert!(w.message.contains("will not fire") && w.message.ends_with("Roadmap package #11 lifts it"), "{}", w.message);
}

const LOG: &str = "tasks:\n  - id: t\n    type: core.log\n    message: hi\n";

#[test]
fn a_webhook_wait_warns_at_publish_that_every_call_answers_409() {
    let r = report(&format!("id: f\nnamespace: a.b\n{LOG}triggers:\n  - id: w\n    type: io.kestra.plugin.core.trigger.Webhook\n    key: k\n    wait: true\n"));
    assert!(r.errors.is_empty(), "{r:?}");
    let w = r.warnings.iter().find(|w| w.path == "triggers.w.wait").unwrap_or_else(|| panic!("{r:?}"));
    assert!(w.message.contains("answers 409") && w.message.ends_with("Roadmap package #11 lifts it"), "{}", w.message);
}

#[test]
fn stop_after_and_an_explicit_allow_concurrent_false_warn_and_true_does_not() {
    let r = report(&format!(
        "id: f\nnamespace: a.b\n{LOG}triggers:\n\
         \x20 - id: a\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"0 * * * *\"\n    stopAfter: [FAILED]\n\
         \x20 - id: b\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"0 * * * *\"\n    allowConcurrent: false\n\
         \x20 - id: c\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"0 * * * *\"\n    allowConcurrent: true\n"
    ));
    let paths: BTreeSet<_> = r.warnings.iter().filter(|w| w.code == "not_honoured").map(|w| w.path.as_str()).collect();
    assert_eq!(paths, BTreeSet::from(["triggers.a.stopAfter", "triggers.b.allowConcurrent"]));
}

#[test]
fn an_unknown_trigger_key_is_refused() {
    let r = report(&format!("id: f\nnamespace: a.b\n{LOG}triggers:\n  - id: s\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: \"0 * * * *\"\n    crn: x\n"));
    assert!(r.errors.iter().any(|e| e.code == "unknown_property" && e.path == "triggers.s.crn"), "{r:?}");
}

/// One egret struct serves twenty Kestra classes: a key of another subtype
/// is unknown to Kestra (spec §3.5).
#[test]
fn an_input_key_of_another_type_is_refused_and_its_own_kestra_key_is_warned() {
    let r = report("id: f\nnamespace: a.b\ninputs:\n  - id: n\n    type: INT\n    min: 1\n    validator: \"^a$\"\n  - id: s\n    type: STRING\n    itemType: INT\ntasks:\n  - id: t\n    type: core.log\n    message: hi\n");
    assert!(r.warnings.iter().any(|w| w.path == "inputs.n.min" && w.message.ends_with("roadmap package #3 lifts it")), "{r:?}");
    let unknown: BTreeSet<_> = r.errors.iter().filter(|e| e.code == "unknown_property").map(|e| e.path.as_str()).collect();
    assert_eq!(unknown, BTreeSet::from(["inputs.n.validator", "inputs.s.itemType"]));
}

#[test]
fn an_unknown_output_key_is_refused() {
    let r = report(&format!("id: f\nnamespace: a.b\n{LOG}outputs:\n  - id: o\n    type: STRING\n    value: x\n    valu: y\n"));
    assert!(r.errors.iter().any(|e| e.code == "unknown_property" && e.path == "outputs.o.valu"), "{r:?}");
}
```

- [ ] **Step 2: run, expect FAIL**: `cargo test -p egret-workflow --lib -- spec::keys`.
- [ ] **Step 3: implement.** Before relying on `resolve` and `declared_unsupported` at save, re-read both and record in the report that neither renders nor reads anything but the model (spec §7).
- [ ] **Step 4: mutation.** Make `input_keys` return the union of all subtypes' keys (the struct's view); see `an_input_key_of_another_type_is_refused…` red. Restore.
- [ ] **Step 5: ratchet and census.** Expect no move from Task 4. The census did not scan cross-type input keys: if `LIVE` or `VALIDATE_REFUSED` moves, list each flow and key and stop (spec §6.2).
- [ ] **Step 6:** regenerate-check `property-treatment.json` (expect no diff: these keys were `refused` by stage 2 or are warnings).
- [ ] **Step 7: commit** `feat(workflow): trigger, input and output keys are checked at publish; sync- and fire-time refusals warn at publish`.

---

### Task 6: nested beans (spec §3.7)

**Files:** `src/spec/keys.rs` (`nested_findings`); tests in `keys.rs`.

**Rules:** each bean's allowed set is `wire_fields_of::<Struct>()` where the model is a derived struct; otherwise a const with Kestra's file:line:
- `retry` (flow and every task): `type` + `RetryCommon` fields + per type (`constant`: `interval`; `exponential`: `interval`, `maxInterval`, `delayFactor`; `random`: `minInterval`, `maxInterval`). Read `k/core/models/tasks/retrys/{AbstractRetry,Constant,Exponential,Random}.java` and record each line; if egret's `Retry` variant lacks one Kestra has, it is a `lift_of` row and a finding for the report (none expected).
- `concurrency`, each `sla` (per `type`), each `checks` entry, `checkFrequency`, each `Dag` node (`task`, `dependsOn`), each input's `dependsOn`, each Flow trigger `dependsOn` entry.
- `quotas`: skipped (spec §3.7). `Window`: unchanged (`deny_unknown_fields`).

- [ ] **Step 1: write the failing tests:** a `retry` with `maxAttempt: 3` (typo) → `unknown_property` at `tasks.t.retry.maxAttempt`; a `checkFrequency` with `interva: PT1S` → at `tasks.w.checkFrequency.interva`; a `Dag` node with `dependOn: [a]` → at `tasks.d.tasks[1].dependOn` (read how `validate.rs` paths a Dag node today and use the same); a Flow trigger `dependsOn` entry with `flowid:` → at `triggers.l.dependsOn[0].flowid`; and one positive control per bean with only valid keys and no finding.
- [ ] **Step 2: run, expect FAIL.**
- [ ] **Step 3: implement**, and one unit test per hand-written const that each listed key deserializes into the model (so the const cannot list a key the model drops).
- [ ] **Step 4: mutation.** Remove `delayFactor` from the exponential const; see its positive control red. Restore.
- [ ] **Step 5: ratchet and census.** Not scanned by the census: any move is listed and stops the task.
- [ ] **Step 6: commit** `feat(workflow): unknown keys in retry, concurrency, sla, checks and the other nested beans are refused at publish`.

---

### Task 7: Kestra's own save-time warnings (D5, spec §4)

**Files:** `src/spec/keys.rs` (`kestra_warnings`); tests in `keys.rs`.

- **Non-runnable types:** the 11 flowables and `executor::exit::TYPE_ID`, `executor::update::{SET_VARIABLES, UNSET_VARIABLES, LABELS}`. **Measure first:** read `worker/timeout.rs` and the `WorkingDirectory` claim path; if egret enforces `timeout` on `WorkingDirectory` or an executor-level type, that type is left out of the `timeout` warning, with a doc line saying why (the warning must be true for egret). Record what was found.
- Messages verbatim from `FlowService.java:575,578,581`; code `runnable_property_on_non_runnable`; path `<task path>.<key>`; `timeout` not on `Pause`.
- Flow trigger: `dependsOn` empty and `when` absent or literally `"true"` → `This flow will be triggered for EVERY execution of EVERY flow in this workspace. We recommend adding the dependsOn property to the Flow trigger '<id>'.`, code `flow_trigger_without_depends_on`, path `triggers.<id>`; the doc comment quotes Kestra's `FlowService.java:564` text and says why "on your instance" became "in this workspace" (`flow_trigger/process.rs:575-585`).

- [ ] **Step 1: write the failing tests:**

```rust
#[test]
fn kestra_warns_on_timeout_on_a_flowable_but_not_on_a_pause() {
    let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: s\n    type: io.kestra.plugin.core.flow.Sequential\n    timeout: PT1M\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n  - id: p\n    type: io.kestra.plugin.core.flow.Pause\n    timeout: PT1M\n");
    let w: Vec<_> = r.warnings.iter().filter(|w| w.code == "runnable_property_on_non_runnable").collect();
    assert_eq!(w.len(), 1, "{r:?}");
    assert_eq!(w[0].message, "The task 's' cannot use the 'timeout' property as it's only relevant for runnable tasks.");
}

#[test]
fn task_cache_on_a_flowable_gets_kestras_warning_and_not_the_13_one() {
    let r = report("id: f\nnamespace: a.b\ntasks:\n  - id: p\n    type: io.kestra.plugin.core.flow.Parallel\n    taskCache: {enabled: true}\n    tasks:\n      - id: a\n        type: core.log\n        message: a\n");
    let at: Vec<_> = r.warnings.iter().filter(|w| w.path == "tasks.p.taskCache").collect();
    assert_eq!(at.len(), 1, "{r:?}");
    assert_eq!(at[0].code, "runnable_property_on_non_runnable");
    assert_eq!(at[0].message, "The task 'p' cannot use the 'taskCache' property as it's only relevant for runnable tasks.");
}

const LISTENER: &str = "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n    message: hi\ntriggers:\n  - id: l\n    type: io.kestra.plugin.core.trigger.Flow\n    states: [SUCCESS]\n";

#[test]
fn a_flow_trigger_with_no_depends_on_warns_in_kestras_words_scoped_to_the_workspace() {
    let r = report(LISTENER);
    let w = r.warnings.iter().find(|w| w.code == "flow_trigger_without_depends_on").unwrap_or_else(|| panic!("{r:?}"));
    assert_eq!(w.path, "triggers.l");
    assert_eq!(
        w.message,
        "This flow will be triggered for EVERY execution of EVERY flow in this workspace. \
         We recommend adding the dependsOn property to the Flow trigger 'l'."
    );
}

#[test]
fn a_flow_trigger_with_depends_on_or_a_when_does_not_warn() {
    for extra in [
        "    dependsOn:\n      - namespace: a.b\n        flowId: g\n",
        "    when: \"{{ trigger.namespace == 'a.b' }}\"\n",
    ] {
        let r = report(&format!("{LISTENER}{extra}"));
        assert!(!r.warnings.iter().any(|w| w.code == "flow_trigger_without_depends_on"), "{extra}: {r:?}");
    }
}
```

  Read `models/trigger.rs`'s `FlowTrigger`/`Dependency` for the exact `dependsOn` and `when` spellings egret parses; adjust the YAML, not the assertions.

- [ ] **Step 2: run, expect FAIL.** **Step 3: implement.** Run `--lib -- spec`, `--test publish`, `--test routes`. Existing tests that assert an empty `warnings` on a fixture that now draws a Kestra warning are fixed by filtering to the code under test, never by deleting the assertion; list each one in the report.
- [ ] **Step 4: mutation.** Remove the `Pause` exception; see the first test red. Restore.
- [ ] **Step 5: ratchet** (no move expected). **Step 6: commit** `feat(workflow): Kestra's save-time warnings for runnable properties and unfiltered Flow triggers`.

---

### Task 8: every refusal and warning names its package (D4, D14, spec §5)

**Files:**
- `src/spec/validate.rs`: `NOT_YET_ENFORCED` rows and the emitter's sentence; `unsupported_pause_property` (with the `tasks` text of D14), `unsupported_allow_failure_retry`, `unsupported_loop_until_property`; the pin test `the_not_yet_enforced_list_is_pinned_in_both_directions`; the doc paragraph above `NOT_YET_ENFORCED` gains a dated note.
- `src/scripts/mod.rs`, `src/plugins/mod.rs`: the not-implemented messages read `keys::lift_of(Site::Task(fqcn), key).expect(..).short()`; runner, `resources` and `taskRunner` texts per spec §5.
- `src/worker/commands.rs:3108-3124`: `(W5a)` → `(roadmap package #17)`.
- `src/trigger/schedule.rs:1292-1299`, `src/trigger/webhook.rs:377-393`: append the package.
- `src/executor/plan.rs:860-863`: `pause_gap`'s doc for `tasks` says it is not a Kestra property (`Pause.java:158`), refused permanently.
- `src/models/task.rs:144-152,572-573`: the false "`Pause.tasks` is Kestra's own field" / "`extends Sequential`" docs are corrected with a dated note.

- [ ] **Step 1: change the pins first** (they fail on the old text): the `NOT_YET_ENFORCED` pin test's expected table becomes

```rust
&[
    ("concurrency", "roadmap package #8 lifts it"),
    ("sla", "roadmap package #16 lifts it"),
    ("checks", "roadmap package #16 lifts it"),
    ("quotas", "it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap §3.3)"),
    ("triggers", "egret fires only Schedule, Webhook and Flow triggers; roadmap package #11 lifts ScheduleOnDates and http.Trigger, #18 lifts plugin triggers, and McpToolTrigger is not engine scope (roadmap §3.3)"),
]
```

  and add one test per reworded refusal asserting the whole new message (spec §5's table), plus:

```rust
/// **No user-visible text names a retired package** (spec D4).
#[test]
fn no_message_names_a_w_package_or_a_later_package() {
    const STALE: &[&str] = &["(W5a)", "(W5b)", "lands in W", "a later package"];
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut offenders = Vec::new();
    for path in walk_rs(root) {
        let text = std::fs::read_to_string(&path).unwrap();
        let prod = text.split("#[cfg(test)]").next().unwrap();
        for (n, line) in prod.lines().enumerate() {
            let code = line.trim_start();
            // Doc comments and `//` lines are history; only text a user can
            // receive is checked, which is what sits inside a string literal.
            if code.starts_with("//") {
                continue;
            }
            let in_literal = code.split('"').skip(1).step_by(2).any(|lit| STALE.iter().any(|s| lit.contains(s)));
            // A continued literal (`\`-joined) starts with no quote on its own line.
            let continued = !code.contains('"') && STALE.iter().any(|s| code.contains(s));
            if in_literal || continued {
                offenders.push(format!("{path}:{}: {code}", n + 1));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:#?}");
}
```

  `walk_rs` is Task 2's helper; move it to a shared `#[cfg(test)]` module in `spec/` if both tests need it. `the_not_yet_enforced_list…`'s own expected table moves past `#[cfg(test)]`, so it is not scanned.

- [ ] **Step 2: run, expect FAIL**: `cargo test -p egret-workflow --lib -- spec::validate scripts plugins trigger` and `cargo test -p egret-workflow --test plugins_gate --test scripts_gate`.
- [ ] **Step 3: change the texts.** Sweep: `grep -rn "(W5a)\|(W5b)\|lands in\|a later package" src tests` — every hit in a string literal is changed; a hit in a test's expected string is updated to the new text. Run the same tests plus `--test publish` and the ratchet (`REFUSAL_GROUPS` match on message substrings; none of them is one of these texts, which the run confirms).
- [ ] **Step 4: mutation.** Restore one "(W5a)" literal; see `no_message_names_a_w_package_or_a_later_package` red. Restore.
- [ ] **Step 5: commit** `fix(workflow): every refusal and warning names the roadmap package that lifts it`.

---

### Task 8b (in scope: the user answered spec Q1 with (b) on 2026-10-19): a golden `refused` entry also matches a save-time refusal

**Files:** `tests/executor.rs` (`egret_side`, `executor_matches_golden`, `REFUSAL_GROUPS`, `STILL_OVER_ACCEPTED`, `UNATTRIBUTED`), `oracle-exec/driver.rs` (`publish_corpus`'s return is kept in `Side`), `oracle-exec/baseline.txt` (delete `invalids/invalid-property`), `oracle-exec/README.md`.

- [ ] **Step 1:** `Side` keeps `publish_corpus`'s `BTreeMap<String, Filed>`. In `executor_matches_golden`, `(Verdict::Refused, Outcome::Trace(_))` counts as matched when `filed[id]` is `Filed::DraftOnly(msg)` and `msg` contains the `message` of the `REFUSAL_GROUPS` group that lists `id`. An id in no group is never matched this way, so a refusal for another reason cannot bank by accident.
- [ ] **Step 2:** add the group `{cause: "an unknown property on a core task, refused at save as Kestra refuses it (YamlParser.java:29-31)", message: "unknown_property", ids: &["invalids/invalid-property"]}`; read the group test that drives `build_plan` per id and give it the same save-time arm.
- [ ] **Step 3:** run the ratchet; expect "PROGRESS NOT BANKED" naming `invalids/invalid-property`; delete its `baseline.txt` line with a dated comment; remove it from `STILL_OVER_ACCEPTED` and `UNATTRIBUTED`. Re-run: 301 / 72 / 72 / 0.
- [ ] **Step 4: mutation.** Drop the group-message check (match any `DraftOnly`); measure which other baselined refused entries would then bank, record them as the reason the check exists, and restore.
- [ ] **Step 5: commit** `test(workflow): the oracle counts Kestra's save-time refusals against egret's publish`.

---

### Task 9: the stale "345" (D7, spec §8)

**Files:** the files spec §8 lists.

- [ ] **Step 1:** `grep -rn "345" crates/egret-workflow/src crates/egret-workflow/oracle-exec/driver.rs | grep -v "\.json"` and classify every hit as present-tense (fix), co-stated count (re-count or drop the numeral) or historical delta (keep). Record the table in the report.
- [ ] **Step 2:** the two asserts first (`trigger/depends_on.rs:1188`, `trigger/flow.rs:1705`): their messages say "expected the 351-flow corpus". Run `cargo test -p egret-workflow --lib -- trigger::depends_on trigger::flow`.
- [ ] **Step 3:** the doc comments. Where a sentence states another count measured over 345 and a test computes it (`executor/mod.rs:2764`, `worker/spawn.rs:1333`), read the count from that test's current output and write both numbers, or write "the corpus" with no numeral; never 351 beside a count measured at 345.
- [ ] **Step 4:** `cargo test -p egret-workflow --lib -- executor worker trigger` and the ratchet. **Step 5: commit** `docs(workflow): the corpus is 351 flows, not 345`.

---

### Task 10: docs (EN, then VI section by section)

**Files:**
- `docs/{en,vi}/specs/2026-10-19-workflow-no-silent-drops-design.md`: `Status:` "built and verified <date>", and a dated "as built" note under each section that the build changed (measured census figures, the `timeout` measurement of Task 7, the nested-bean consts of Task 6, the answers to §12).
- `docs/{en,vi}/plans/2026-10-18-workflow-kestra-parity-roadmap.md`: spec §9's eight corrections as dated notes (nothing deleted); §1's table gains a dated column or note with the measured census; row #1 is marked done with the PR only after merge.
- `crates/egret-workflow/src/spec/schema/README.md`: the treatment table (`:122`) gains a line: a `not_honoured` warning does not make a key `warned`; the `onChildFailure` rows.
- `crates/egret-workflow/oracle-exec/README.md`: `invalids/invalid-property`'s entry (Task 3 or 8b).
- `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §9: a dated note for package #1.

- [ ] **Step 1:** EN edits, each count naming its source (the ratchet printout, `driver.rs`).
- [ ] **Step 2:** VI mirrors EN section by section; its header says EN wins.
- [ ] **Step 3:** `cargo test -p egret-workflow --test executor` (README counts are asserted) and `--test flow_schema`.
- [ ] **Step 4: commit** `docs(workflow): package #1 — roadmap corrections, schema README, programme §9`.

## Final gate (controller, after the whole-branch review)

On the branch rebased onto fresh `origin/main`, with the Global Constraints' environment, one command at a time:
1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test -p egret-workflow` (all targets). Ratchet: 301 / 73 / 73 / 0 (or 72 / 72 with Task 8b); `LIVE` 294, `VALIDATE_REFUSED` 26, `UNKNOWN_TYPE_REFUSED` 21, unless a task measured and recorded otherwise.
4. `cargo test -p egret-server` (OpenAPI and generated-client checks; the DTO shape is unchanged, so expect no diff).
5. No frontend change.

Known and accepted: the 7 `sql_starrocks_live` tests fail without an engine.
