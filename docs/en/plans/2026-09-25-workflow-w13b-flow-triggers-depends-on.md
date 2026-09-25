# W13b — Flow triggers, `dependsOn` arm, Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Flow trigger with `dependsOn` fires once its dependencies are satisfied inside its time window, per `mode`. Examples of dependencies: "A and B both succeeded today", "any of A, B", "at least 2 of 3".

**Architecture:** This builds on W13a's outbox processor. For a `depends_on` listener it takes the listener's row in the new `workflow.flow_trigger_windows` table:
- `INSERT … ON CONFLICT DO NOTHING`, then `SELECT … FOR UPDATE`;
- merges this event's per-dependency results (true results only, sticky);
- fires through W13a's path when `mode` is satisfied;
- then deletes the window (`resetOnSuccess`).

Window bounds come from a pure port of Kestra's `TimeWindow` (four kinds), fed from W13b Task 1's measurements.

**Tech Stack:** Rust, sqlx runtime queries, tokio, Postgres, chrono / chrono-tz.

**Spec:** `docs/{en,vi}/specs/2026-09-25-workflow-w13-flow-triggers-design.md` §5, §6, §7.2.
**Depends on:** the W13a branch. This branch is cut from W13a's tip. If W13a has merged, it is cut from `main`.

**A warning about this document.** Code blocks are shapes to verify, not text to paste. The plan is wrong wherever it disagrees with the code.

---

## Global Constraints

- **Everything in W13a's Global Constraints applies unchanged.** That covers the ratchet (109 → 109), the pure `src/trigger/`, the six off switches, draft listeners firing, mutation discipline, commits, and the environment.
- **Migration `0158`.** Re-check `main` and the SP4a branch before naming it.
- **The window's time source is Postgres `now()`**, read in the event's transaction and passed into the pure bound functions. There is no wall-clock read in `src/trigger/`.
- **Over-acceptance is the hazard.** A window shape, `Duration` or field combination that is not implemented is **refused at save**, not approximated. That is the same rule as W12's `lateMaximumDelay`.
- **Window semantics come from Task 1's measurements.** Where the Java and a measurement disagree, the measurement wins and the disagreement is written down.

## File Structure

```
migrations/0158_workflow_flow_trigger_windows.sql   NEW
crates/egret-workflow/src/
  models/trigger.rs          FlowTrigger gains `window: Option<Window>`; Window model
  trigger/window.rs          NEW, pure: TimeWindow, Window -> TimeWindow, bounds(now) -> (start, end)
  trigger/duration.rs        NEW, pure: ISO-8601 Duration subset parser
  trigger/depends_on.rs      NEW, pure: Dependency match, merge, mode test
  spec/validate.rs           refuse conflicting / unsupported windows at save
  flow_trigger/process.rs    the depends_on branch replaces W13a's skip
  scheduler/fire.rs or runtime.rs   expired-window sweep once per pass
crates/egret-workflow/tests/
  flow_trigger_windows.rs    NEW
crates/egret-workflow/oracle-exec/
  w13b-windows.json          NEW, Task 1's measurements
```

---

## Task 1: Measure window behaviour on the artifact

**Files:** create `crates/egret-workflow/oracle-exec/w13b-windows.json`. Amend spec §5 (both languages) only where a measurement contradicts it.

**Interfaces:** Produces the fixtures Task 3's pure tests load with `include_str!`.

- [ ] **Step 1:** Bring up the pinned artifact (`oracle-exec/docker compose up -d`; the API base and auth are in `harvest.py`).
- [ ] **Step 2: For each question below,** deploy producers `p1`, `p2`, `p3` (one Log task each) and one listener. Drive the producers by hand, and record which listener executions appear, when, and with what `trigger` object.
  1. `ALL` over `p1`, `p2`: `p1` alone, then `p2`. How many fires? Does a second `p1` + `p2` fire again (reset)?
  2. `ANY` over `p1`, `p2`, and `AT_LEAST` with `minSatisfied: 2` over `p1`, `p2`, `p3`. How many fires, and when does the window reset?
  3. `DURATION_WINDOW` with `every: PT2M`: the bounds. Read them from the stored window state if the API exposes it (search the API for `multiple`/`window`). Otherwise infer them from a met condition that expires. Note the zone: is the start aligned to UTC midnight or the server zone?
  4. `every: PT2M`, `offset: PT1M`: how the start shifts.
  5. `SLIDING_WINDOW` (`lookback: PT2M`): does a condition met 3 minutes ago count?
  6. `DAILY_TIME_WINDOW` (`from`/`to` bracketing now, and one not bracketing now): is an event outside ignored?
  7. `DAILY_TIME_DEADLINE`: the behaviour before and after the deadline.
  8. A met condition that did not fire, then window expiry: does it carry over?
  9. The `trigger.variables` of a `dependsOn` fire. Do they differ from the window-free shape (W13a Task 1)?
- [ ] **Step 3: Save** every observation verbatim, with timestamps, the digest and the zone of the container, plus a `findings` array that quotes the value each finding came from. If a question cannot be answered on the artifact, say so and name what the code will do instead. The spec is amended to "chosen".
- [ ] **Step 4:** `docker compose down -v`, then commit: `docs(workflow): W13b Task 1 — window behaviour on the artifact`.

---

## Task 2: The `window` model, `Duration`, and validation at save

**Files:** modify `crates/egret-workflow/src/models/trigger.rs`; create `crates/egret-workflow/src/trigger/duration.rs`; modify `crates/egret-workflow/src/spec/validate.rs`; unit tests beside the code, and a routes-level test that a save is refused.

**Interfaces:**

```rust
pub struct Window {            // Kestra 2.x `Window`, camelCase serde
    pub deadline: Option<String>, pub from: Option<String>, pub to: Option<String>,
    pub every: Option<String>,   pub offset: Option<String>, pub lookback: Option<String>,
}
pub fn parse_duration(s: &str) -> Result<chrono::Duration, DurationError>;   // ISO-8601 subset
```

- [ ] **Step 1: Failing tests.**
  - The YAML round trip preserves `window`; before this task it was dropped. The test must fail on HEAD.
  - `parse_duration` handles `PT10M`, `PT6H`, `P1D`, `PT1H30M` and `P1DT2H`, and refuses `P1M`, `P1Y`, fractional seconds, negatives and garbage. Months and years are refused because they have no fixed length; say so in the error.
  - Validation refuses: `deadline` + `from`; `from` without `to`; `lookback` + `every`; an unparseable duration; `offset` without `every`; a `window` on a trigger with no `dependsOn`. Check that last case against Task 1: if Kestra ignores it, egret refuses rather than silently ignoring.
- [ ] **Step 2: Implement.** The mapping to kinds follows spec §5.1. Validation lives in `spec::validate` beside `schedule_missing_inputs`, so a flow with a bad window never saves. Check the `LocalTime` format of `deadline`/`from`/`to` (`HH:mm`, `HH:mm:ss`) against Task 1.
- [ ] **Step 3: Mutations:** drop each validation arm in turn; accept `P1M`. Each must be killed.
- [ ] **Step 4: Full crate gate; commit:** `feat(workflow): W13b — the window model, ISO durations, and refusal at save`.

---

## Task 3: Pure `TimeWindow` bounds and `dependsOn` evaluation

**Files:** create `crates/egret-workflow/src/trigger/window.rs` and `trigger/depends_on.rs`.

**Interfaces:**

```rust
pub enum TimeWindow { Duration { every: Duration, offset: Duration }, Sliding { lookback: Duration },
                      DailyWindow { from: NaiveTime, to: NaiveTime }, DailyDeadline { deadline: NaiveTime } }
impl TimeWindow {
    pub fn from_model(w: Option<&Window>) -> Result<Self, WindowError>;          // default: Duration{1 day, 0}
    pub fn bounds(&self, now: DateTime<Utc>, tz: Tz) -> (DateTime<Utc>, DateTime<Utc>);
    pub fn is_valid(start: DateTime<Utc>, end: DateTime<Utc>, now: DateTime<Utc>) -> bool; // Kestra: after start && before end
}
pub fn dependency_matches(dep: &Dependency, source: &DepSource, when: &WhenVerdict) -> bool;
pub fn merge(results: &BTreeMap<String,bool>, now_true: &BTreeMap<String,bool>) -> BTreeMap<String,bool>; // true-only, sticky
pub fn satisfied(mode: FlowTriggerMode, min: Option<i32>, n_conditions: usize, results: &BTreeMap<String,bool>) -> bool;
```

- The condition key for each dependency must match Kestra's `DependsOnMultipleCondition` key (read `Flow.java` ~440-470; likely positional). Name the rule.
- `tz` is whatever Task 1 measured, UTC or the server zone. It is a parameter, never read from the environment.

- [ ] **Step 1: Failing tests fed from `w13b-windows.json`.** For every measured case, assert `bounds(now)` / `is_valid` / `satisfied` equal the measurement. Add pure edge tests:
  - `now` exactly at `start` and exactly at `end` (Kestra's `isAfter` / `isBefore` are strict);
  - `DURATION_WINDOW` across a DST change in a zone that has one;
  - `AT_LEAST` with `minSatisfied` greater than the number of conditions (refused at save in Task 2, and asserted here too).
- [ ] **Step 2: Implement; run; mutations.**
  - Make the bounds comparison non-strict; each must be killed.
  - Drop the stickiness in `merge`.
  - Swap `ALL`/`ANY`.
- [ ] **Step 3: Commit:** `feat(workflow): W13b — TimeWindow bounds and dependsOn evaluation, pure`.

---

## Task 4: Migration 0158 and the `dependsOn` branch of the processor

**Files:** create `migrations/0158_workflow_flow_trigger_windows.sql`; modify `crates/egret-workflow/src/flow_trigger/process.rs` and the scheduler ticker (expired-window sweep); create `crates/egret-workflow/tests/flow_trigger_windows.rs`.

**Interfaces:**
- Consumes: W13a's `process_events`, listener SELECT, savepoint/`done`/`attempts`, `flow_trigger_record`, `start_execution_in`; Task 3's pure functions; W13a's `execution_render_context` (for `Dependency.when` and the trigger's `when`).
- Produces: `flow_trigger::sweep_expired_windows(&PgPool) -> Result<u64>`.

- [ ] **Step 1: The migration** per spec §5.2. Also add a CHECK that `window_start < window_end`.
- [ ] **Step 2: Failing tests** (static mutex + clear tables, as in W13a's `flow_trigger_fire.rs`):
  - `all_fires_once_both_are_met_and_resets`: `p1` done gives 0 fires and a window with one result. Then `p2` done gives 1 fire, and the window row is gone. Another `p1` + `p2` fires again.
  - `any_fires_on_the_first`; `at_least_two_of_three`.
  - `two_processors_satisfying_all_together_fire_once`: two events (`p1`, `p2`) processed concurrently by two `process_events` calls. Count insert attempts as in W12 Task 5 and assert exactly one listener execution **and** one attempt.
  - `an_expired_window_does_not_count`: backdate the window's `window_end` by SQL; a new event starts a fresh window, and the old result is not counted.
  - `an_event_outside_a_daily_window_is_ignored`.
  - `a_dependency_when_renders_on_the_source`.
  - `off_switches_stop_accumulation`: a disabled listener's window is not touched by events.
  - `the_sweep_deletes_only_expired_windows`.
  - Remove W13a's `a_depends_on_listener_is_skipped_with_a_named_reason` (it now fires), and **say so in the commit**.
- [ ] **Step 3: Implement** spec §5.3 inside W13a's per-listener savepoint:
  - `INSERT … ON CONFLICT (flow_id, trigger_id) DO NOTHING` with bounds computed at the transaction's `now()`.
  - `SELECT … FOR UPDATE`. An expired row is replaced with fresh bounds and empty results.
  - Skip if `!is_valid(now)`.
  - Evaluate the dependencies, merge, `UPDATE`.
  - If the trigger's own `when` holds and the result is `satisfied`: fire via W13a's path, then `DELETE` the window when `resetOnSuccess` holds (default true; Task 1 confirms the ANY/AT_LEAST behaviour).
  - The expired sweep runs once per scheduler pass, after `process_events`.
- [ ] **Step 4: The safety section.** Extend `flow_trigger/mod.rs` with the window statements: the insert-if-absent, the `FOR UPDATE` (the serialisation that makes "fire once" hold), the update, the delete-on-fire, and the sweep. Each names its test and mutation result.
- [ ] **Step 5: Mutations:**

| mutation | must kill |
|---|---|
| drop `FOR UPDATE` on the window | `two_processors_satisfying_all_together_fire_once` |
| check-then-insert instead of `ON CONFLICT` | same test, or a duplicate-key error surfacing, whichever happens; report which |
| don't delete on fire | `all_fires_once_both_are_met_and_resets` (a third event fires again early) |
| drop the expiry replacement | `an_expired_window_does_not_count` |
| render `Dependency.when` on the listener | `a_dependency_when_renders_on_the_source` |

- [ ] **Step 6: Full crate gate; commit:** `feat(workflow): W13b — dependsOn windows in the flow-trigger processor`.

---

## Task 5: Coverage statement, sweep, full gate

- [ ] **Step 1:** Add the W13b section of the coverage statement: ratchet 109 → 109, and it would read 109 with the `dependsOn` branch deleted. Also count the corpus: of the 20 `dependsOn` declarations, how many now resolve, and how many are refused at save and why. List them.
- [ ] **Step 2:** Sweep "dependsOn is refused" / "UnsupportedDependsOn" / "window is dropped" across code docs and specs, EN and VI. Report N examined / M corrected per population.
- [ ] **Step 3:** Full local CI and a name-set diff against the base, as in W13a Task 5.
- [ ] **Step 4:** Commit: `docs(workflow): W13b coverage statement and the sweep`.

---

## Final review

A whole-branch review on the most capable model. Point it at:
1. Any path where two events can both see an unsatisfied window and both fire, or neither fires.
2. Window bounds against Task 1's measurements, including DST and midnight.
3. `resetOnSuccess` under ANY/AT_LEAST.
4. Refusals at save that silently accept an unimplemented shape.
5. The expired-window sweep deleting a live window.
