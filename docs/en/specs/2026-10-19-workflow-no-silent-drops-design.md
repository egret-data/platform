# Workflow package #1 "No silent drops": every key egret parses and ignores is named

**Status:** built and verified 2026-10-26 (draft PR #756; the as-built notes below are dated 2026-10-26 and append to the design, nothing of it is deleted). Designed 2026-10-19. Roadmap row #1 of `docs/en/plans/2026-10-18-workflow-kestra-parity-roadmap.md`. Plan: `docs/en/plans/2026-10-19-workflow-no-silent-drops.md`. One PR, branch `feat/workflow-no-silent-drops`.

**Where the claims come from:** every claim about egret's code was checked in the worktree `egret-nodrops` at main `309405b1`. Code is cited `path:line` relative to `crates/egret-workflow/src/` unless another root is given. Kestra is the checkout at `data-portal/kestra`, pinned at `96893b589` (2.0.0-SNAPSHOT, 2026-08-17); its paths are relative to `core/src/main/java/io/kestra/`, shortened to `k/`, unless another root is given. The key census behind §3 was measured on the same two checkouts (a Ruby Psych scan of all 351 corpus files; nothing built or run). A claim that could not be checked statically says "to measure in Task N" and names the plan task that measures it.

**Goal (user, 2026-10-19):** egret is Kestra OSS's engine, except where egret diverges for security. Package #1 makes every key egret parses and then ignores either a named refusal or a named warning, and each one names the roadmap package that lifts it (roadmap Rule 1). Flow-level `finally:`/`afterExecution:` and every flowable `finally:` are **not** refused or warned here: they go to #2, which is next.

**What Kestra does, which #1 copies** (census §0):
- `k/core/serializers/YamlParser.java:29-31`: the strict mapper sets `FAIL_ON_UNKNOWN_PROPERTIES`. An unknown key becomes a `ConstraintViolationException` with Jackson's message (`Unrecognized field "invalid" (class …), not marked as ignorable`) at `YamlParser.java:144-157`.
- It is plain Jackson, so it applies to every nested bean: the flow, each task class, triggers, each input subtype, retry, `Concurrency`, `SLA`, `Check`, `CheckFrequency`, the `Dag` node. No model class carries `@JsonIgnoreProperties(ignoreUnknown = true)`.
- Strict parsing runs only on a **non-draft** save: `k/core/services/FlowService.java:134-141` (create) and `:167-174` (update) say "Strict parsing (unknown / duplicate properties) and constraint validation are both skipped for drafts: they are allowed to be saved invalid and will fail at execution time instead." egret's draft is Kestra's draft and egret's publish is Kestra's non-draft save, so #1 refuses at publish and reports on the draft, as `pluginDefaults:` already is.
- Kestra also has save-time warnings (`FlowService.warnings`, `FlowService.java:546-596`). egret ports only the missing-subflow one today (`subflow_not_found`, debt spec §5.1).

## Decisions

Decisions D1–D10 were made by the controller on 2026-10-19; their direction is fixed and the wording here is this spec's. D11–D16 are this spec's defaults, under the rule "follow Kestra unless egret has a reason not to, and keep safety tight".

- **D1. Unknown properties are refused, as Kestra refuses them, at save and publish only.** The sites are: core leaf tasks, the flow's top-level keys, flowables, the typed triggers (`Schedule`, `Webhook`, `Flow`), inputs (per type), flow outputs, and the nested beans of §3.7. The finding is the error `unknown_property`. It is raised only by the save-time entry point (D11), never by `validate_for_run`, so `executor::build_plan` (`executor/plan.rs:1093-1105`) is unchanged and a version published before #1 keeps running. Where serde already dropped the key, the check reads the raw source. The type-to-struct table that is `#[cfg(test)]` in `spec/schema/measure.rs:105-176` moves to production (`spec::keys`, §2.3). A key whose value is `null` is refused too: Jackson refuses an unknown name whatever it holds.
- **D2. A key Kestra accepts and egret does not act on is a warning, not a refusal.** The flow stays publishable, and the warning names the package that lifts it (§3). Pause `onResume` is a warning (it keeps the 3 `onResume` corpus flows publishable). This follows the census's call for every key: no golden-MATCH entry is lost and no LIVE flow is lost, except `invalids/invalid-property`, which Kestra itself refuses (§6).
- **D3. The warning code is `not_honoured`, distinct from `not_yet_enforced`.** `spec/schema/measure.rs` classifies a property as `warned` only when a new `not_yet_enforced` warning appears (`measure.rs:708-712`), and `warned` is served in the editor schema (`spec/schema/mod.rs:131-150`). A `not_honoured` warning therefore leaves a key `ignored` (or `honoured`) in `property-treatment.json`, and the editor does not start offering `logLevel` on every core task. The five existing `not_yet_enforced` rows keep their code, so their rows stay `warned`. A new test pins this (`a_not_honoured_warning_is_not_served`, plan Task 2); `the_committed_property_treatment_is_what_egret_does_today` (`measure.rs:860`) and `tests/flow_schema.rs`'s `no_served_property_is_one_egret_ignores_or_refuses` stay green because no row becomes served, and the only rows that move go from `ignored` to `refused` (§6.3).
- **D4. Every existing refusal or warning that names a stale package, or none, is reworded to name its roadmap package.** That is the `not_yet_enforced` rows ("W8", "a later package"), the script and plugin refusals ("W5a", "W5b"), and the flowable refusals that name nothing. The full old → new list is §5. Security refusals (the `Process` runner, `http()`, `secret()` outside plugin tasks) keep their text (D10).
- **D5. Kestra's own save-time warnings are ported** where egret has the construct (§4): `timeout`/`taskCache`/`workerSelector` on a non-runnable task (`FlowService.java:569-583`) and a Flow trigger with no `dependsOn` (`:553-566`). The plain-text secret warning (`:585-593`) has no egret construct to attach to: the only `@PluginProperty(secret = true)` fields at the pin are the http client configurations, and egret has no http task until #4, which ports it.
- **D6. Schedule `lateMaximumDelay` becomes a publish-time warning naming #11.** Today it is accepted at publish and refused only when the scheduler syncs the trigger (`trigger/schedule.rs:1368-1371`, surfacing as the trigger row's `last_error`), so the trigger never fires. Kestra accepts it at save and honours it. The census measured 0 corpus flows. A warning keeps the flow publishable as Kestra does, and says before publish what the sync says after it: this trigger will not fire. The same reasoning, and the same warning, covers the other refusals that only show after publish: Schedule `when` (`schedule.rs:1354`) and Webhook `wait`/`returnOutputs`/`responseContentType`/`responseCode` (`trigger/webhook.rs:345-360`, a 409 on every call). The sync-time and fire-time refusals themselves are unchanged.
- **D7. The stale present-tense "345" becomes 351** (§8). Historical deltas ("345 -> 351 at WorkingDirectory Task 8") stay. A sentence that pairs 345 with another count measured at the same time ("16 of 345") is history and stays.
- **D8. #1 does not capture flowable `finally:`.** A version published before #2 keeps it only in `snapshot.source` (`publish.rs`, the snapshot's `"source": draft.source`), because the snapshot `plan` is the re-serialised `Flow`, which drops it. §10 tells #2 to re-derive from the source.
- **D9. The roadmap corrections go into this package's PR**, EN and VI, as dated, append-only notes (§9).
- **D10. Security.** The new checks read only the flow being saved: no repository, port, workspace or other flow is consulted, so a finding cannot depend on, or reveal, anything outside the caller's draft. No expression is rendered at save: a value is never evaluated, and only key names and the author's own `type` strings appear in messages. The script and plugin gates, the `Process` runner refusal, the `http()` refusal and the `secret()` confinement are untouched (roadmap §3.1).
- **D11. One save-time entry point, `spec::validate_source(source, &flow)`.** It is `validate(&flow)` plus `spec::keys::findings(source, &flow, &mut report)`. The four places that report on a saved or publishing flow call it: `publish::publish_gated` (`publish.rs:311`), `routes::draft_dto` (`routes.rs:513`), `routes::validate_flow_source` (`routes.rs:899`) and `measure.rs`'s `run` (`measure.rs:227-260`). `validate(&Flow)` and `validate_for_run(&Flow)` keep their signatures and findings. A source-scan test pins that no production caller outside `spec/` calls `validate(` directly.
- **D12. One table names the lifting package for every key**: `spec::keys::lift_of`. The script gate, the plugin gate and the new walker all read it, so two messages cannot name two packages for the same key. A test cross-checks the "Kestra accepts it" half of the table against the committed rc13 schema export (`spec/schema/kestra-flow-schema.json`), with the differences listed by name. **The pin governs** (D9): rc13's `onChildFailure` on `Parallel`/`Dag` is newer than the pin (`git log -S onChildFailure` is empty at `96893b589`), so it is an unknown property and is refused.
- **D13. Script and plugin refusals stay refusals.** Their Task.java keys (`logLevel`, `logToFile`, `taskCache`, `version`, `assets`, `workerSelector`, `policyRefs`) are refused there and only warned on core tasks. Downgrading them is a behaviour change outside #1's scope; #13 lifts them on every task kind at once. Only their messages change (D4).
- **D14. A `Pause` `tasks:` stays refused by `executor::pause_gap`, under the same code, with a message that says it is not a Kestra property.** Kestra's `Pause` has no `tasks` field at the pin (`k/plugin/core/flow/Pause.java:158`, `extends Task implements FlowableTask`; its fields are `pauseDuration`, `behavior`, `onPause`, `onResume`, `errors`, `finally`). The refusal is permanent (an unknown property), and it stays in `validate_for_run`, because a stored version with a `Pause` `tasks:` stalls (`executor/plan.rs:860-863`). The code `unsupported_pause_property` is kept, because `measure.rs:77`'s `REFUSED_BY_NAME` and the `PUBLISHED_BUT_REFUSED` causes read it.
- **D15. Server-managed flow keys are accepted with no finding**, as Kestra does: `revision` (Kestra assigns it, `FlowService.java:506-508`), `updated`, `tenantId` (`@Hidden`) and `deleted` (`k/core/models/flows/AbstractFlow.java:41,44,65,76`). Kestra ignores a value an author writes for each; `deleted: true` is to measure in Task 4 from Kestra's `FlowRepository` create path, and becomes a `not_honoured` warning if Kestra acts on it.
- **D16. A default that diverges with no key written is a roadmap row, not a warning.** Schedule `allowConcurrent` defaults to `false` in Kestra and locks the trigger until its execution ends (`scheduler/src/main/java/io/kestra/scheduler/TriggerScheduler.java:345-349`); egret never locks. #1 warns only on an explicit `allowConcurrent: false`, and the default goes to #11 (§9). Input `required` (default `true`, `k/core/models/flows/Input.java:82`) is the same shape; #3 already lists it.

- **D17 (2026-10-26, default decision after rebasing onto main `28fb8693`). #747 (KI-121, KI-122) shipped part of this package, and #1 absorbs it instead of duplicating it.** `spec/properties.rs` (`property_warnings`, called at `publish.rs:317`, `routes.rs:533` and `routes.rs:1027`) already walks the raw source against `property-treatment.json` and warns `unknown_property` / `ignored_property`; `validate.rs` `schedule_findings` (KI-122) already warns `schedule_not_resolvable` for a Schedule `cron`, `timezone`, `when` or `lateMaximumDelay` the scheduler would refuse, and `schedule_input_templated`; `measure.rs` `NO_EFFECT` now lists `warningOnRetry`. Consequences: (1) `keys::findings` **replaces** `property_warnings`: `properties.rs` is deleted, its tests are ported into `keys.rs`, and the three call sites become `validate_source`. (2) D1 stands: `unknown_property` becomes an **error** (it was a warning in KI-121); `ignored_property` becomes `not_honoured` with the lifting package (D2, D3). Any test or doc that pins the KI-121 codes is updated. (3) D6's Schedule half (`lateMaximumDelay`, `when`) is already delivered by `schedule_not_resolvable`: Task 5 adds only the package name to that message and does not add a second warning for the same key. Webhook `wait`/`returnOutputs`/`responseContentType`/`responseCode` stay in Task 5. (4) Line numbers in this spec and the plan were measured at `309405b1`; the implementer re-greps every cited anchor.

**As built (2026-10-26), decisions D15 and D17.**
- **D17, what the rebase met.** The branch was rebased onto main `28fb8693`. #747 (KI-121, KI-122) had shipped `spec/properties.rs` (`property_warnings`) and `schedule_findings`. #1 absorbed `property_warnings` into `keys::findings` and deleted `properties.rs` (its tests moved to `keys.rs`). `unknown_property` became an error (KI-121 warned it); `ignored_property` became `not_honoured`, and its message names the lifting package. The Schedule `when` and `lateMaximumDelay` keys reuse KI-122's one `schedule_not_resolvable` warning, now at path `triggers.<id>.when` / `.lateMaximumDelay` and naming #11; no second warning exists for them. The `a_not_honoured_warning_is_not_served` test (`measure.rs`) pins D3.
- **D15, `deleted:` measured by reading code (not run against a live Kestra).** Kestra persists a source `deleted: true` as a tombstone: `FlowService.java:126-148` (create) calls `AbstractJdbcFlowRepository.java:924-938` (`create`), which calls `save` (`:981-1020`); the parsed `GenericFlow` keeps the YAML `deleted` (a field of `AbstractFlow.java:65`; only `revision` and `updated` are overwritten, near `:1008`) and persists it, and the column is `GENERATED ALWAYS AS (CAST(value ->> 'deleted' AS BOOL))` (`jdbc-postgres/.../baseline-postgres.sql:117`). So Kestra acts on it, and egret warns `not_honoured` with the reason that egret deletes a flow through its own delete action and never through a key in the source (a `KeptDesign` row, test `flow_deleted_warns_because_kestra_acts_on_it`). Caveat: Jackson's field-level deserialization of the Lombok field was inferred, not executed. `revision`, `updated` and `tenantId` stay silent, as D15 says.
- **Other facts from Task 4.** Descent into children is per flowable kind (`child_lists`), so a leaf property named `tasks` or `errors` is not walked as a task list. `unpaired(source, flow)` proves the walk fails open: over the corpus, 924 model tasks were walked and 0 were unpaired.

## 1. What #1 covers

| site | unknown key | Kestra key egret ignores | key egret refuses later | where it is checked |
|---|---|---|---|---|
| flow | `unknown_property` | `not_honoured`: `workerSelector`, `policyRefs`, `draft` | – | source (§3.1) |
| core leaf task | `unknown_property` | `not_honoured`: the 7 Task.java keys, `retry.warningOnRetry` | – | model: the leaf keeps its spec (§3.2) |
| flowable | `unknown_property` | `not_honoured`: §3.3's table | `Pause`'s existing refusals, reworded | source (§3.3) |
| typed trigger | `unknown_property` | `not_honoured`: §3.4's table | `not_honoured` at publish for the sync/fire refusals (D6) | source (§3.4) |
| input, output | `unknown_property`, including a key of another input subtype | `not_honoured`: §3.5's table | – | source (§3.5) |
| nested beans | `unknown_property` | `not_honoured`: §3.7 | – | source (§3.7) |
| script, plugin task | unchanged (`script_property_unknown`, `plugin_property_unknown`) | unchanged refusals, reworded (D13) | – | unchanged |
| `Leaf` trigger, unregistered or unserved leaf type | not checked: there is no property list to check against | – | – | unchanged (`not_yet_enforced`, `unknown_task_type`, `task_type_not_served`) |

**Not in #1, by Rule 1:** flow `finally:`/`afterExecution:`, flowable `finally:` on every kind, and `errors:` on `Loop`, `LoopUntil` and `WorkingDirectory`. #2 lifts all of them (§9 adds `LoopUntil`/`WorkingDirectory` `errors:` and `WorkingDirectory` `finally:` to #2's row). The walker skips these keys by name; it neither refuses nor warns, and it does not descend into them (a task inside a dropped list is not in the model).

## 2. Mechanism

### 2.1 Findings and codes

`Finding {code, message, path}` and `ValidationReport {errors, warnings}` (`spec/validate.rs:85-103`) are unchanged. Errors block publish; warnings ride into the snapshot's `warnings` and the draft DTO.

New codes, each egret's own (Kestra has codes for none of them):
- `unknown_property` (error): a key neither Kestra (at the pin) nor egret accepts on that object.
- `not_honoured` (warning): a key Kestra accepts and egret does not act on.
- `flow_trigger_without_depends_on` (warning): Kestra's Flow-trigger warning (§4).
- `runnable_property_on_non_runnable` (warning): Kestra's `timeout`/`taskCache`/`workerSelector` warning (§4).

### 2.2 Where it runs

- `spec::validate_source(source: &str, flow: &Flow) -> ValidationReport` (D11) is `validate(flow)` and then `spec::keys::findings(source, flow, &mut report)`.
- `keys::findings` parses `source` once more, into a `serde_json::Value`, with `serde_norway::from_str` (the same deserializer `spec::parse` uses, `spec/parse.rs:30-33`). The source has just parsed as a `Flow`, so this cannot fail on a saved draft; if it does, `findings` adds nothing (fail open on the instrument, never a false refusal) and a `debug_assert!` names it.
- It never runs at run time. `build_plan` calls `validate_for_run` (`plan.rs:1105`), which calls none of this.

### 2.3 The key tables (`src/spec/keys.rs`, new, production)

- `TASK_MODEL_KEYS`: what `models::Task`'s deserializer reads itself: `id type description retry timeout disabled runIf allowFailure allowWarning` (`models/task.rs:764-781`).
- `TASK_JAVA_UNREAD`: Kestra's `Task.java` keys egret does not read: `version workerSelector policyRefs logLevel logToFile taskCache assets` (`k/core/models/tasks/Task.java:45,68,74,77,85,97,102`).
- `leaf_keys(canonical_type) -> Option<Vec<&'static str>>`: moved from `measure.rs:105-150` (`leaf_handler_fields`), unchanged in content. `None` means "no property list" and the leaf is not checked (`egret.*`, `core.http`, `core.slack`, script, plugin and unregistered types).
- `flowable_keys(kind)`, `trigger_keys(kind)`: moved from `measure.rs:152-176`, built from `spec::schema::wire_fields_of` (`spec/schema/mod.rs:820`, production already).
- `input_keys(type)`: the common keys (`id type description required defaults prefill displayName dependsOn`, `Input.java:62-98`) plus the subtype's own, from §3.5.
- `lift_of(site, key) -> Option<Lift>`: the Kestra-accepted keys egret ignores, with the package that lifts each (§3). `None` for a key egret honours or that Kestra does not have.
- `enum Lift { Package(u8), Enterprise, KeptDesign(&'static str) }`. `Package(n)` renders "roadmap package #n lifts it"; `Enterprise` renders "it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap §3.3)"; `KeptDesign` renders its reason and "(roadmap §3.2)".

`measure.rs` imports these instead of keeping its own copies, so the instrument and the check read one table.

### 2.4 Walking the source beside the model

Tasks are paired with their raw maps by structure, never by searching the raw tree for objects that look like tasks (a plugin property can hold `{id, type}`):
- The walk starts from the four root lists of both trees (`spec/path.rs`'s `root_lists`) and descends, per flowable kind, into the same lists `Task::children()` and `Task::error_children()` walk (`models/task.rs:122-201`), in the same order: `tasks`, `then`/`else`, each `Switch` case and `defaults`, each `Dag` node's `task`, `AllowFailure`'s `finally`, `Pause`'s `tasks`/`errors`/`onPause`, and the `errors` of the kinds that have it.
- A pair whose raw `id` differs from the model's `id` stops the walk below it, with no finding (the instrument fails open), and the corpus test of plan Task 4 proves that no corpus flow takes that branch.
- Paths are `spec::path`'s, so a finding sits where `validate`'s other findings for the same task sit (`tasks.t.invalid`).

## 3. The keys, site by site

Each table row is a key and what #1 does with it. "warn #n" is a `not_honoured` warning naming package n. Kestra lines are at the pin.

### 3.1 Flow (`models/flow.rs:39-134`; Kestra `AbstractFlow.java:28-100`, `Flow.java:49-142`)

| key | Kestra | #1 |
|---|---|---|
| the keys `models::Flow` reads (`wire_fields_of::<Flow>()`) | – | none (unchanged; `pluginDefaults` keeps `removed_plugin_defaults`) |
| `workerSelector` | `AbstractFlow:99`; OSS routes nothing (`WorkerQueueService.java:85-87`) | warn, `Enterprise` |
| `policyRefs` | `Flow:93-98`, "parsed but ignored in the open-source edition" | warn, `Enterprise` |
| `draft` | `AbstractFlow:72` | warn, `KeptDesign("egret's draft and publish split replaces it: a published version runs whatever `draft:` says")`. Not a refusal: `valids/webhook-draft` is a golden MATCH |
| `revision`, `updated`, `tenantId`, `deleted` | server-managed | none (D15) |
| `finally`, `afterExecution` | `Flow:79-88` | none (#2, Rule 1) |
| anything else | strict | `unknown_property`. Corpus: 0 flows |

### 3.2 Core leaf tasks

The leaf's `spec` is kept verbatim by the model (`models/task.rs:949-975`), so this check needs no source: it runs on the model inside `keys::findings`. For each registered leaf with a property list (`leaf_keys`, 30 types plus the `core.log` alias; census §3b), every key of `spec` is:
- in `leaf_keys(type)` or `TASK_MODEL_KEYS`: nothing;
- in `TASK_JAVA_UNREAD`: warn, with `lift_of`: `logLevel`, `logToFile`, `taskCache`, `version`, `assets` → #13 (roadmap §3.3 has #13's spec decide `version` and `assets`); `workerSelector`, `policyRefs` → `Enterprise`;
- anything else: `unknown_property`, `task '<id>': unknown property '<key>' on <type>: neither Kestra nor egret accepts it`.

`retry.warningOnRetry: true` (task or flow) is parsed (`models/task.rs:697-705`) and read by nothing (`executor/plan.rs:687-693`): warn #13 on every task kind and on the flow. Corpus: 4 flows, 2 golden MATCH, so not a refusal.

Corpus: one flow, `invalids/invalid-property` (`Return` with `invalid:`); Kestra refuses it with `Unrecognized field "invalid"`. Zero valid flows are hit.

### 3.3 Flowables (`models/task.rs`; Kestra `AbstractBranch.java:27-41` gives `tasks`, `errors`, `finally` to the branch kinds)

Allowed keys are `flowable_keys(kind) ∪ TASK_MODEL_KEYS`. Then:

| kind | key | Kestra | #1 |
|---|---|---|---|
| every kind | `finally` (except `AllowFailure`, which reads it) | `AbstractBranch:32`, `If:114`, `Switch:115`, `Dag:109`, `Pause:199` | none (#2) |
| `Loop`, `LoopUntil`, `WorkingDirectory` | `errors` | `AbstractBranch` | none (#2; §9 widens #2 to `LoopUntil` and `WorkingDirectory`) |
| every kind | `logLevel`, `logToFile` | `Task.java:77,85` | warn #13 |
| every kind | `version`, `assets` | `Task.java:45,102` | warn #13 |
| every kind | `policyRefs` | `Task.java:74` | warn, `Enterprise` |
| every kind | `timeout`, `taskCache`, `workerSelector` | `Task.java:59,97,68` | Kestra's own warning (§4); `timeout` on `Pause` is read (`plan.rs:1436-1440`) and gets nothing |
| `Loop` | `outputs`, `fetchType` | `Loop.java:248,261` | warn #6. Corpus: 6 flows, 5 baselined, 1 harvest-rejected |
| `Subflow` | `revision`, `labels`, `inheritLabels`, `scheduleDate`, `restartBehavior` | `Subflow.java:102,117,138-139,144,155` | warn #12. `labels`/`inheritLabels`: 3 golden MATCH. `scheduleDate`: see Q2 |
| `Pause` | `onResume` | `Pause.java:193` | warn #9 (D2). Corpus: 3 not-standalone flows |
| `Pause` | `tasks` | not a Kestra property | `unsupported_pause_property` from `pause_gap`, reworded (D14); the walker does not report it a second time |
| `Parallel`, `Dag` | `onChildFailure` | not at the pin (rc13 only) | `unknown_property` (D12) |
| any kind | anything else | strict | `unknown_property`. Corpus: 0 |

### 3.4 Typed triggers (`models/trigger.rs`; Kestra `AbstractTrigger.java:38-108`)

The base keys `models::Trigger`'s deserializer reads (`trigger.rs:363-400`) are listed as a const `TRIGGER_BASE_KEYS`. Allowed keys are those plus `trigger_keys(kind)`. `Leaf` triggers are not checked (no property list; their `not_yet_enforced` row stays).

| key | Kestra | #1 |
|---|---|---|
| `stopAfter` | `AbstractTrigger:90` (read by `TriggerState.java:249,268`) | warn #11 (parsed, read by nothing, `trigger.rs:52-60`) |
| `failOnTriggerError` | `:98` | warn #11 |
| `allowConcurrent: false` written | `:105` | warn #11: "egret never locks a trigger while its execution runs". Absent or `true`: nothing (D16) |
| `logToFile`, `logLevel` | `:94,74` | warn #13 |
| `version`, `assets` | `:43,108` | warn #13 |
| `workerSelector`, `policyRefs` | `:65,71` | warn, `Enterprise` |
| `labels` on `Schedule`/`Webhook` | `:84` | warn #11 (read by the Flow trigger only). Corpus: `valids/webhook-plugin-labels`, golden MATCH |
| `when` on `Webhook` | `:55` | warn #11 (ignored) |
| `when`, `lateMaximumDelay` on `Schedule` | `:55`, `Schedule.java:230` | warn #11, from `ScheduleTrigger::resolve`'s own refusal: "this Schedule trigger will not fire" (D6) |
| `wait`, `returnOutputs`, `responseContentType`, `responseCode` on `Webhook` | `Webhook.java:188,196,208,218` | warn #11, from `webhook::declared_unsupported`: "every call to this webhook answers 409" (D6) |
| `interval` on `Schedule` | `Schedule.java:221`, `final … = null`, inert | none: the key is in `trigger_keys(Schedule)` if the model reads it; otherwise added to the allowed set with this citation, because Kestra accepts it and does nothing with it either |
| anything else | strict | `unknown_property`. Corpus: 0 |

The publish-time warning for a sync or fire refusal calls the same predicate the sync or the route calls, so the two cannot disagree. `measure.rs`'s stage 2 already classifies these keys `refused` (`measure.rs:722-727`); that does not change.

**As built (2026-10-26), §3.4 (Task 5).** A trigger key is allowed when it is in `TRIGGER_BASE_KEYS`, `trigger_keys(kind)` or is Schedule `interval` (Kestra `final = null`, inert); a key with a `lift_of(Site::Trigger)` row warns `not_honoured`; anything else is `unknown_property`. The Webhook keys (`wait`, `returnOutputs`, `responseContentType`, `responseCode`) warn with code `not_honoured` ("every call to this webhook answers 409. Roadmap package #11 lifts it"), while Schedule `when`/`lateMaximumDelay` keep KI-122's code `schedule_not_resolvable` (D17): the same family has two codes. `allowConcurrent` warns only when written `false`, and `stopAfter` warns. §7 held: neither predicate renders (`ScheduleTrigger::resolve`, `trigger/schedule.rs:1340-1385`, reads the raw `when` string; `webhook::declared_unsupported`, `trigger/webhook.rs:345`, reads the model).

**As built (2026-10-26), §3.5 (Task 5).** `INPUT_TYPES` has the 20 wire names, with `input_keys(ty)` per type. `the_input_rows_name_the_package_the_roadmap_gives` pins 21 input rows (#3, or Enterprise for FORM `inputs` and REUSABLE_INPUTS `ref`/`namespace`/`revision`) and the 12 trigger rows (#11). Measuring found an instrument artefact: Kestra's schema offers no `REUSABLE_INPUTS` in the `type` enum of `ReusableInputsInput-1`, so `measure.rs` sampled that class as a STRING input and its three keys flipped `ignored` to `refused` on a regenerate; the sample now uses type `REUSABLE_INPUTS` and `property-treatment.json` is byte-identical. Corpus: zero errors on triggers, inputs and outputs in valids and invalids (33 warnings only), so no cross-type input key exists in the 351 files.

### 3.5 Inputs and outputs (`models/input.rs:54-96`; Kestra `Input.java:62-98` plus each subtype)

One egret struct serves all 20 input types; Kestra has one class per type, so a key that is valid on one type is unknown on another.

| type | Kestra own keys | egret | #1 |
|---|---|---|---|
| STRING, SECRET | `validator` (`StringInput.java:22`, `SecretInput.java:23`) | dropped | warn #3 |
| INT, FLOAT, DURATION | `min`, `max` (`IntInput.java:17,20`, `FloatInput.java:17,20`, `DurationInput.java:19,22`) | dropped | warn #3 |
| DATE, DATETIME, TIME | `after`, `before` (`DateInput`/`DateTimeInput`/`TimeInput.java:19,22`) | dropped | warn #3 |
| SELECT | `values`, `expression`, `allowCustomValue` (`SelectInput.java:32-58`) | dropped | warn #3 (5 golden-MATCH flows write `values`) |
| SELECT, MULTISELECT | `isRadio`, `autoSelectFirst` (UI hints) | dropped | warn #3 (§9 adds them to #3's row as start-form hints) |
| MULTISELECT | `values`, `expression`, `allowCustomValue` (`MultiselectInput.java:33-59`); `itemType` | `itemType` kept | warn #3 for the dropped three |
| ARRAY | `itemType` (`ArrayInput.java:26`) | kept | none |
| JSON | `jsonSchema` (`JsonInput.java:33`) | dropped | warn #3 (1 golden MATCH) |
| FILE | `allowedFileExtensions` (`FileInput.java:27`) | kept, enforced | none |
| FORM | `inputs` (`FormInput.java:28`) | dropped | warn, `Enterprise` (roadmap §3.3 lists FORM) |
| REUSABLE_INPUTS | `ref`, `namespace`, `revision` (`ReusableInputsInput.java:35-48`) | dropped | warn, `Enterprise` |
| any | a key of another subtype only (`validator` on INT, `itemType` on STRING) | accepted today | `unknown_property` |
| any | anything else | dropped | `unknown_property` |

`input_keys` keeps `itemType` and `allowedFileExtensions` on the types Kestra has them on only: an `itemType` on a STRING is `unknown_property`. The census did not scan cross-type keys per type; Task 5 measures the corpus.

Flow outputs: `FlowOutput` has all six Kestra fields (`models/flow.rs:196-214`, `Output.java`); any other key is `unknown_property`.

### 3.6 Script and plugin tasks

Unchanged in behaviour (D13). Their messages name the roadmap package (§5).

**As built (2026-10-26), §3.6 (the mid-package regression).** The whole-suite run after Task 8 (not the unit tests) found a regression from Tasks 3 to 7: `scripts_gate::the_w4_aliases_still_publish` failed because the walker rejected the legacy `image:` key that `scripts::parse` accepts on a W4 alias. The fix (`task()` skips the check when `scripts::kind_of(row)` is `Some`; plugin tasks have no table row) restores D13: script and plugin tasks are left to their own gates, and `a_script_or_plugin_task_is_not_checked_for_unknown_keys_here` pins it. The same run showed that the `end_to_end` fixture `one_task_source` wrote both `message` and `format` on `Log` and `Return`. Kestra has `message` and `level` on `Log` (`Log.java:79,85`) and only `format` on `Return` (`Return.java:91`), so Kestra rejects that flow and the refusal was right; the fixture now writes `format` only on `Return`, and `a_log_key_on_a_return_and_a_return_key_on_a_log_are_refused` pins the pair.

### 3.7 Nested beans

Each is checked against its egret struct's wire fields and, where Kestra has a key egret drops, `lift_of`:
- `retry` (task and flow), per type: the common keys (`RetryCommon`, `models/task.rs:697-705`) plus the type's own (`constant`: `interval`; `exponential`: `interval`, `maxInterval`, `delayFactor`; `random`: `minInterval`, `maxInterval`; `k/core/models/tasks/retrys/{Constant,Exponential,Random}.java`). `Retry` is a tagged enum (`task.rs:723`), so the per-type sets are a const written from the Kestra classes, with a unit test that each listed key deserializes into the model (to measure in Task 6).
- `concurrency` (`Concurrency`, `flow.rs:164`), `sla` (per type), `checks` (`Check`, `flow.rs:299`), `checkFrequency` (`CheckFrequency`, `task.rs:514`), the `Dag` node (`DagNode`, `task.rs:422`), an input's `dependsOn` (`input.rs` `DependsOn`), and a Flow trigger's `dependsOn` entries.
- `quotas` is skipped: the whole key is an Enterprise warning already, and Kestra OSS does not read it.
- `Window` is `deny_unknown_fields` already (`trigger.rs:266`) and is unchanged.

A bean whose model is not a plain derived struct gets a hand-written const with its Kestra file:line; Task 6 lists which.

**As built (2026-10-26), §3.7 (Task 6).** Per-type consts, each with the Kestra `file:line`, replace the table's retry check: `RETRY_COMMON_KEYS` and `retry_keys(ty)` (`AbstractRetry.java:31,34,37,40`; `Constant.java:26,29`; `Exponential.java:27,30,33,35`; `Random.java:27,30,33`) and `sla_keys(ty)` (`SLA.java:42,45,48,53`; `MaxDurationSLA.java:25`; `ExecutionAssertionSLA.java:27-28`). Two unit tests check that every listed retry key and every listed SLA key deserializes into the model. `Concurrency`, `Check`, `CheckFrequency` (LoopUntil), `DagNode` (Dag), an input's `DependsOn` and a Flow trigger's `Dependency` are checked against their struct's wire fields. Two decisions of the implementer: (1) a new `Site::Check` lift row sends the deprecated Check `condition` (accepted by Jackson through `setCondition`, `Check.java:64-67`; egret reads `when` only) to #16 as a `not_honoured` warning; (2) `flowRef` on a Flow-trigger `dependsOn` entry is refused as unknown, because it is an egret-internal field that Kestra refuses. `quotas` is skipped and `Window` untouched, as designed. Corpus floors, measured: valids 323 files, 322 parsed, 39 triggers, 106 inputs, 20 outputs, 94 nested beans; invalids 28, 23, 4, 18, 1, 13. Known gap: `retry_keys` returns `None` for an unknown retry type and so fails open.

## 4. Kestra's own save-time warnings (D5)

**Runnable properties on a non-runnable task** (`FlowService.java:569-583`). Kestra walks `flow.allTasksWithChilds()` and, for every task that is not a `RunnableTask`, warns on a non-null `timeout` (except on a `Pause`), `taskCache` and `workerSelector`. At the pin the non-runnable core types are the 11 flowables plus `Exit`, `SetVariables`, `UnsetVariables` and `Labels` (each `extends Task` without `RunnableTask`). Messages, verbatim:
- `The task '<id>' cannot use the 'timeout' property as it's only relevant for runnable tasks.`
- `The task '<id>' cannot use the 'taskCache' property as it's only relevant for runnable tasks.`
- `The task '<id>' cannot use the 'workerSelector' property as it's only relevant for runnable tasks.`

Code `runnable_property_on_non_runnable`, path `<task path>.<key>`. On these tasks the #13/`Enterprise` warning of §3 is not added as well: Kestra does not honour the key there either, so no package lifts it. egret must not honour `timeout` on any of these types for the warning to be true: `WorkingDirectory` and the four executor-level types are to measure in Task 7 (`worker/timeout.rs`); a type egret does honour it on is left out of the warning, with a doc line.

**A Flow trigger with no `dependsOn`** (`FlowService.java:553-566`): when `dependsOn` is empty and `when` is absent or the literal `"true"`. Kestra's text is `This flow will be triggered for EVERY execution of EVERY flow on your instance. We recommend adding the dependsOn property to the Flow trigger '<id>'.` egret's listener is scoped to the workspace (`flow_trigger/process.rs:575-585`, `WHERE l.workspace_id = $1`), so egret's text replaces "on your instance" with "in this workspace" and is otherwise verbatim. Code `flow_trigger_without_depends_on`, path `triggers.<id>`.

**As built (2026-10-26), §4 (Task 7).** Both warnings are in (`runnable_property_on_non_runnable`, `flow_trigger_without_depends_on`, texts as in this section; "on your instance" became "in this workspace", Q4). The `timeout` measurement §4 asked for: **egret enforces `timeout` on none of the non-runnable types**, so none is left out of the warning and `Pause` is the only exemption (Kestra's own, and egret does enforce it there). Evidence: `worker/claim.rs:425-432` `leaf()` returns a timeout only for `TaskKind::Leaf`, and a block gets `NotALeaf` and `Budget::Refused` (`claim.rs:278-296`); `worker/working_dir.rs` has no `timeout` reference; `executor/exit.rs` and `executor/update.rs` never read `timeout`; `executor/plan.rs:1438` copies `timeout` into the plan for `Pause` only, and `executor/flowable/pause.rs:243` enforces it. A `null` value counts as absent, as Kestra's `!= null`. On the four executor-level types (`Exit`, `SetVariables`, `UnsetVariables`, `Labels`) `taskCache` and `workerSelector` draw Kestra's warning and not the #13 one, so each key has one finding. Corpus: 0 findings of either code (all corpus Flow triggers carry `dependsOn` or `when`; the six corpus `timeout`s are on runnable tasks or `Pause`).

## 5. User-visible messages that change

Every changed text, old → new. `<…>` is a placeholder filled as today.

| where | old | new |
|---|---|---|
| `not_yet_enforced`, all rows (`validate.rs:2108-2114`) | ``"`<name>` is declared in this flow but is not yet enforced by any package in this branch; enforcement lands in <package>"`` | ``"`<name>` is declared in this flow but egret does not enforce it yet; <lift>"`` |
| `concurrency` row | `W8` | `roadmap package #8 lifts it` |
| `sla` row (MAX_DURATION) | `W8` | `roadmap package #16 lifts it` |
| `checks` row | `W8` | `roadmap package #16 lifts it` |
| `quotas` row | `W8` | `it is Kestra Enterprise only, and Kestra OSS ignores it too (roadmap §3.3)` |
| `triggers` row (a `Leaf` trigger) | `a later package; egret fires only Schedule, Webhook and Flow triggers` | `egret fires only Schedule, Webhook and Flow triggers; roadmap package #11 lifts ScheduleOnDates and http.Trigger, #18 lifts plugin triggers, and McpToolTrigger is not engine scope (roadmap §3.3)` |
| `unsupported_pause_property`, `errors`/`behavior`/`onPause` (`validate.rs:1667-1675`) | `… Remove '<p>', or use a plain Pause (optionally bounded by pauseDuration or timeout)` | the same, then `. Roadmap package #9 lifts it` |
| `unsupported_pause_property`, `tasks` (D14) | the same sentence as above | ``task '<id>' is a pause with 'tasks', which Kestra's Pause does not have: it is an unknown property, refused on egret as Kestra refuses it. Put the tasks after the Pause instead`` |
| `unsupported_allow_failure_retry` (`validate.rs:1641-1648`) | `… put `retry:` on the child task that should be retried` | the same, then `. Roadmap package #9 lifts it` |
| `unsupported_loop_until_property` (`validate.rs:1528-1535`) | `… '<field>' must be a literal value` | the same, then `. Roadmap package #9 lifts it` (Q3) |
| script `script_property_not_implemented` (`scripts/mod.rs:507-517`) | `property '<k>' is accepted by Kestra on <fqcn> but is not implemented by egret (W5a); remove it` | `property '<k>' is accepted by Kestra on <fqcn> but is not implemented by egret (<lift>); remove it`, `<lift>` from `lift_of`: `logLevel`/`logToFile`/`taskCache`/`version`/`assets` → `roadmap package #13`; `runner`/`targetOS` and the seven python keys → `roadmap package #17`; `workerSelector`/`policyRefs` → `Kestra Enterprise only, ignored by Kestra OSS too, roadmap §3.3` |
| script runner properties (`scripts/mod.rs:706-717,798,832-835,863-866`) | `(W5a)` | `(roadmap package #17)` |
| script `outputFiles` patterns (`worker/commands.rs:3108-3124`) | `(W5a)` | `(roadmap package #17)` (§9 adds them to #17) |
| plugin `plugin_property_not_implemented` (`plugins/mod.rs:370-378`) | `… not implemented by egret (W5b); remove it` | `… not implemented by egret (<lift>); remove it`, as for scripts |
| plugin `taskRunner` (`plugins/mod.rs:358-366`) | `… (workflow.container_runner); remove it` | the same, then ` Roadmap package #18 lifts it` |
| `ScheduleError::UnsupportedLateMaximumDelay`, `UnsupportedWhenCondition` Display (`schedule.rs:1292-1299`) | `core.schedule `lateMaximumDelay` is not supported: <d>` / `core.schedule `when` conditions are not supported: <w>` | the same, then `; roadmap package #11 lifts it` |
| Webhook `Wait`/`ReturnOutputs`/`ResponseContentType`/`ResponseCode` (`webhook.rs:377-393`) | `… which egret does not implement yet -- …` | the same, then ` (roadmap package #11)` |

Unchanged on purpose: the `Process` runner refusal, `http()`, `secret()` (D10); Webhook `StoreBody`/`Multipart` (`webhook.rs:367-376`), which fire only on a deployment with no internal storage, a configuration and not a parity gap; `unsupported_foreach` and `unsupported_flow_task` (Kestra 2.0 removed both tasks); `removed_plugin_defaults` (agrees with Kestra 2.0).

Tests that pin the old texts are updated in the same task: `the_not_yet_enforced_list_is_pinned_in_both_directions` (`validate.rs:2906`), `tests/plugins_gate.rs:266` (it pins a prefix that is unchanged), `tests/publish.rs:770` (substring unchanged), and any `scripts` unit test asserting "(W5a)". The plan's sweep is `grep -rn "(W5a)\|(W5b)\|lands in\|a later package" src tests`.

**As built (2026-10-26), §5 (Task 8).** Every row of the table was applied, with these differences. (1) The two script expression messages at `scripts/mod.rs` (boolean and list-of-strings expression, `not implemented by egret in W5a`) are not in the table; they lost " in W5a" and name no package, since none was invented. (2) The plugin `taskRunner` text has a period: `... (workflow.container_runner); remove it. Roadmap package #18 lifts it` (the table's "the same, then ` Roadmap package #18 lifts it`" read without one). (3) The D10 fix: `ScheduleError::UnsupportedWhenCondition` and `UnsupportedLateMaximumDelay` became unit variants, so neither `Display` nor the save-time `schedule_not_resolvable` message echoes the author's value; `Display` ends `; roadmap package #11 lifts it`, and the validate message (which already ends "Roadmap package #11 lifts it") is built from the key name only, so the sentence is not doubled. `ScheduleError`'s `UnknownTimezone` and `Cron` variants still echo the author's text (a follow-up, not in this package). (4) False docs corrected with dated notes: `models/task.rs` and `executor/plan.rs` said `Pause.tasks` is Kestra's own field. (5) `property-treatment.json` regenerated: only message texts in `refused_downstream` changed (Schedule `when`/`lateMaximumDelay`, four Webhook keys). The brief's `no_message_names_a_w_package...` test missed the last line of a literal; the committed test checks each non-comment line whole.

## 6. Corpus and ratchet

### 6.1 Where the oracle looks

- The census (`oracle-exec/driver.rs` `LIVE`, `UNKNOWN_TYPE_REFUSED`, `VALIDATE_REFUSED`) comes from `publish_corpus`, which runs the real `publish::publish_gated` (`driver.rs:334-460`). A save-time refusal moves it.
- The ratchet's `refused` verdict comes from `executor::build_plan` (`driver.rs:1168-1174`, `Ran::Refused`), which runs only `validate_for_run`. **A save-only refusal does not move the ratchet.** The census's "banks `invalids/invalid-property`, baseline 73 → 72" assumed a refusal the ratchet would see. With D1 it does not (Q1).

### 6.2 Expected deltas (to measure, not assume)

On main: 301 golden, 73 mismatching, 73 baselined, 0 broken; `LIVE` 295, `VALIDATE_REFUSED` 25, `UNKNOWN_TYPE_REFUSED` 21; `PUBLISHED_BUT_REFUSED` 6.

| figure | expected after #1 | why |
|---|---|---|
| ratchet (golden / mismatching / baselined / broken) | 301 / 73 / 73 / 0 | warnings never move it; the one new error is save-only |
| `LIVE` | 294 | `invalids/invalid-property` is refused at publish (`unknown_property`) |
| `VALIDATE_REFUSED` | 26 | the same flow |
| `UNKNOWN_TYPE_REFUSED` | 21 | no type changes |
| `PUBLISHED_BUT_REFUSED` | 6 | `invalid-property` is a Kestra 422, not a Kestra-published flow |
| `STILL_OVER_ACCEPTED` / `UNATTRIBUTED` (`tests/executor.rs:6650,7093`) | `invalids/invalid-property` stays, with its cause rewritten: "refused at publish since #1 (`unknown_property`); the executor oracle compares `build_plan`, which plans it" | the ratchet still sees it over-accepted |

If the controller answers Q1 with (b), the plan's optional Task 8b changes the instrument and the ratchet reads 72 / 72 baselined, `invalid-property` moves from `STILL_OVER_ACCEPTED` to a `REFUSAL_GROUPS` entry with message `unknown_property`, and the census figures above still hold.

Any other census move is a finding: a cross-type input key (§3.5) or a nested-bean key (§3.7) the census did not scan. The plan stops on one and reports it, rather than adjusting the constant.

**As built (2026-10-26), §6.2 (measured; the table's "expected" column was wrong in two figures).** Sources: the ratchet printout of `cargo test -p egret-workflow --test executor -- --nocapture`, and the constants in `oracle-exec/driver.rs` (`LIVE` `:853`, `UNKNOWN_TYPE_REFUSED` `:931`, `VALIDATE_REFUSED` `:979`).

| figure | §6.2 expected | measured | source |
|---|---|---|---|
| ratchet (golden / mismatching / baselined / broken) | 301 / 73 / 73 / 0, or 72 / 72 with Q1 (b) | **301 / 72 / 72 / 0** | printout; `sixteen_banked` asserts (301, 229, 72) |
| `LIVE` | 294 | **295** (unchanged) | `driver.rs:853`; `publish_corpus` line "295 live" |
| `VALIDATE_REFUSED` | 26 | **25** (unchanged) | `driver.rs:979` |
| `UNKNOWN_TYPE_REFUSED` | 21 | 21 | `driver.rs:931` |
| `PUBLISHED_BUT_REFUSED` | 6 | 6 | `executor/mod.rs` |

Why `LIVE` and `VALIDATE_REFUSED` did not move: `invalids/invalid-property` is a name-contest loser. It shares `id: duplicate` with `invalids/duplicate`, so `publish_corpus` never files it (the printout says "4 name-contested"), and the refusal is invisible to the census (Task 3 measured this first; Tasks 2 to 8 held the same figures). It is therefore also not in `STILL_OVER_ACCEPTED` or `UNATTRIBUTED` any more.

How Q1 (b) was built, as `Seam::Save` (Task 8b): every `RefusalGroup` has a `Seam` (`Run` or `Save`). For an id listed in a `Seam::Save` group the oracle runs `spec::validate_source` on the corpus source and matches a golden `refused` entry when an error finding's code equals the group's `message`. The new group's message is `unknown_property`. `executor_matches_golden` got the arm `(Refused, Trace) if refused_at_save`, and a group test checks the save arm and that `build_plan` still plans the flow (D1). A mutant that accepted any save error for any id left the ratchet green (no other baselined refused entry has a save-time error), so `a_save_time_refusal_banks_only_for_its_own_group_and_code` was added. Derived figures that moved with 73 to 72: `sixteen_banked` (301, 229, 72), the timeout declarations (2, 70, 72), the `handler_surface.py` guard and its regenerated `handler-surface.txt` (ceiling 262), and the `oracle-exec/README.md` prose. Known nit: `RefusalGroup.message` means a substring for `Run` and an exact code for `Save`.

### 6.3 `property-treatment.json`

Regenerated with `EGRET_WRITE_PROPERTY_TREATMENT=1` once `measure.rs` calls `validate_source`. Expected diff: `io.kestra.plugin.core.flow.Parallel.onChildFailure` and `io.kestra.plugin.core.flow.Dag.onChildFailure` go from `ignored` to `refused` (D12). No row may become `honoured` or `warned` (D3). Any other moved row is explained in the task report or the change is wrong.

**As built (2026-10-26), §6.3.** Regenerated with `EGRET_WRITE_PROPERTY_TREATMENT=1`: the diff is exactly `Parallel.onChildFailure` and `Dag.onChildFailure`, `ignored` to `refused` (2 lines in, 2 out in the file diff against main `28fb8693`; `crates/egret-workflow/src/spec/schema/README.md` counts: 1625 properties, 1119 honoured, 4 warned, 378 ignored, 124 refused, counted from the file). No row became `honoured` or `warned`. Task 8 then changed message texts only (§5 note).

## 7. Security (D10)

- `keys::findings` takes `(&str, &Flow)` and returns findings. It has no repository, port, pool or workspace argument, so it cannot look anything up; a test of the signature is the type itself.
- It never calls the renderer. A `when` or `lateMaximumDelay` warning comes from `ScheduleTrigger::resolve` and `webhook::declared_unsupported`, which read the model and do not render (`schedule.rs:1343-1372`; `webhook.rs:345-360`). Task 5 re-reads both before relying on that.
- Messages carry the author's own key names, task ids and `type` strings, never a value. A key name is printed with `'<key>'` as the script gate does; a key is YAML the author wrote in this draft.
- Errors are save-only, so a refusal cannot be used to stop someone else's published version.

## 8. Stale "345" (D7)

The corpus is 351 (`driver.rs:1042` asserts it). Present-tense figures to fix:
- asserts and messages: `trigger/depends_on.rs:1188`, `trigger/flow.rs:1705`;
- doc comments: `oracle-exec/driver.rs:857`; `models/task.rs:50`; `executor/plan.rs:408,767,1227,1434,1567`; `executor/mod.rs:1271,2696,2781,4352,5334,5550,9545`; `executor/flowable/branch.rs:189`, `pause.rs:192`, `dag.rs:74`; `worker/spawn.rs:800`, `render.rs:89`, `replay.rs:148`, `subflow.rs:279`; `trigger/mod.rs:28`, `flow.rs:34`, `schedule.rs:266`; `worker/handlers/python.rs:47`.
- Sentences that state another count measured over 345 (`executor/mod.rs:2764` "52 `Loop` tasks across the 345", `worker/spawn.rs:1333` "43 of its 345 files", `worker/subflow.rs:990-1036` and `webhook_routes.rs:150,153` "16 of 345", "248 of 345") are history or need a re-count. The plan re-derives a count where a test computes it and otherwise drops the numeral ("across the corpus"), as `driver.rs:859-869` did for `LIVE`.
- Historical deltas stay: `executor/mod.rs:2990,4387`, `driver.rs:1037`, and the `tests/executor.rs:1474` comment.

**As built (2026-10-26), §8 (Task 9).** Commit `a4928361`: the present-tense "345" became 351 in comments and in two assert messages (`trigger/depends_on.rs`, `trigger/flow.rs`); no other code changed. Counts stated beside it were re-measured at 351: 52 `Loop` tasks in 43 files, 6 files declare `sla:` (so 345 of 351 declare none), 7 Schedule flows, 17 Flow-trigger flows, 4 files write `CREATE_NEW_EXECUTION`; one prose count that had already drifted from its assert was corrected to the assert (88, 59). Kept as history: the historical deltas, and the W10 census sentences ("16 of 345", "248 of 345"). Left untouched on purpose: about 60 dated measurement lines in `oracle-exec/README.md`, because no test pins a "345" sentence there. The standalone numeral in `oracle-exec/driver.rs:857` was dropped ("the size of the corpus").

## 9. Roadmap corrections (D9)

In `docs/{en,vi}/plans/2026-10-18-workflow-kestra-parity-roadmap.md`, as dated notes, nothing deleted:
1. **#9:** `Pause` `tasks:` is not a Kestra property (`Pause.java:158`); it is removed from #9's scope and stays refused permanently (D14). #9 also gains `LoopUntil` non-literal `checkFrequency`/`failOnMaxReached` (`unsupported_loop_until_property`; Q3).
2. **#11:** gains Schedule `allowConcurrent` (default `false` in Kestra, `TriggerScheduler.java:345-349`; egret never locks; D16), `failOnTriggerError`, and Webhook `when`.
3. **#2:** gains `LoopUntil` `errors:`, `WorkingDirectory` `errors:` and `WorkingDirectory` `finally:` (all `AbstractBranch`; census §4).
4. **#3:** gains SELECT/MULTISELECT `isRadio`/`autoSelectFirst` as start-form hints, and SELECT/MULTISELECT `expression`.
5. **#17:** gains the script `outputFiles` `**` and `[...]` patterns (`worker/commands.rs:3108-3124`).
6. **Appendix B §4:** "Schedule.lateMaximumDelay | supported" is wrong: it is refused when the scheduler syncs the trigger (`trigger/schedule.rs:1368-1371`), and since #1 it is warned at publish. A dated correction note under the table.
7. **The pin governs:** the schema export (rc13, built 2026-09-01) is newer than the source pin (2026-08-17); where they differ (`onChildFailure`), the pin decides. A note in §4 "Keeping this current".
8. **Appendix B §8 item 15** ("unknown and unsupported properties … ignored on core tasks") is closed by #1: a dated note.

**As built (2026-10-26), §9.** The eight corrections are written as dated notes in `docs/{en,vi}/plans/2026-10-18-workflow-kestra-parity-roadmap.md`. Items 6 and 8 target that roadmap's Appendix B, which the VI file does not carry (its header says so); they are recorded in EN under the Appendix B tables and, in VI, in §4 of the roadmap.

## 10. For later packages

- **#2:** a version published before #2 holds flowable `finally:` only in `snapshot.source`; its `plan` lacks it (`publish.rs`, `"plan"` is the re-serialised `Flow`). #2 must re-derive the plan from `snapshot.source` when it runs such a version, or the version silently keeps skipping its `finally:`. The same holds for `Loop`/`LoopUntil`/`WorkingDirectory` `errors:` and `WorkingDirectory` `finally:`. Flow-level `finally:`/`afterExecution:` are in the plan already (`models/flow.rs:81,92`).
- **Every lifting package:** delete the key's `lift_of` row and its `not_honoured` test in the same change that makes egret act on the key; the cross-check test (D12) then requires the key in egret's model.
- **#13:** decide `version` and `assets` (roadmap §3.3); if they are Enterprise-only, their `lift_of` rows become `Enterprise`.
- **#4:** port Kestra's plain-text secret warning (`FlowService.java:585-593`) with the http tasks.

## 11. Packaging and tests

One PR. TDD per task; unit tests in `spec::keys` for every table row class; DB-backed tests only for the publish and route paths. The oracle ratchet stays green after every task, and the census constants change only in the task that moves them, with a dated doc line. No migration, no config key, no frontend change (warnings already render by message).

## 12. Questions for the controller

- **Q1. D1 (save-only) and D2's "`invalids/invalid-property` banks, baseline 73 → 72" cannot both hold.** The ratchet's refused verdict is `build_plan`'s (`driver.rs:1168-1174`), which never runs save-only findings. Options: (a) keep D1; the ratchet stays 73/73, `invalid-property` stays in `STILL_OVER_ACCEPTED` with a new cause, and only the census moves (LIVE 295 → 294). (b) Keep D1 and change the instrument: a golden `refused` entry also matches when `publish_corpus` filed it `DraftOnly` with a message in a named `REFUSAL_GROUPS` group; this banks it (73 → 72) and is plan Task 8b. (c) Raise `unknown_property` in `validate_for_run`; this banks it but stops versions published before #1 from running, which contradicts D1. The plan builds (a) and carries (b) as an optional task. Note that the census's "no LIVE flow lost" also needs reading as "no Kestra-valid LIVE flow lost": `invalid-property` is LIVE today and leaves LIVE under every option.
  - **Answered 2026-10-19 (user): (b).** Task 8b is in scope, not optional: the oracle counts a save-time refusal in a named `REFUSAL_GROUPS` group as matching a golden `refused` entry, and the ratchet is expected at 72 mismatching / 72 baselined. Measure it.
- **Q2. Subflow `scheduleDate`: the census says refuse, D2 says a Kestra-valid key is a warning.** egret would start the child at once rather than at the date. Corpus: 0 flows. This spec warns (D2). Refusing it would be a save-only `unsupported_subflow_property` naming #12.
- **Q3. `unsupported_loop_until_property` has no roadmap row.** This spec names #9 and adds it to #9's row (§9 item 1). The alternative is a new row.
- **Q4. D5 asks for Kestra's warnings verbatim; the Flow-trigger one says "on your instance".** egret's listener is workspace-scoped, so the verbatim text would be false. This spec changes the phrase to "in this workspace" (§4).
- **Q5. D1 lists flowables, triggers and inputs as the nested sites.** Kestra's strictness also covers retry, concurrency, SLA, checks, `checkFrequency`, the `Dag` node and `dependsOn`, and a typo in any of them is dropped today. This spec includes them (§3.7, plan Task 6). Dropping Task 6 leaves those keys silent and the goal unmet for them.

**As built (2026-10-26), §12 (answers).**
- **Q1: (b), per the user (2026-10-19).** Built as Task 8b (`Seam::Save`, see §6.2 note). The ratchet is 301 / 72 / 72 / 0.
- **Q2 to Q5: the defaults this spec took were kept.** Q2: subflow `scheduleDate` warns (#12), it does not refuse. Q3: `unsupported_loop_until_property` is added to #9's row (roadmap note), not a new row. Q4: the Flow-trigger warning says "in this workspace" instead of "on your instance". Q5: the nested beans are checked (§3.7, Task 6).
