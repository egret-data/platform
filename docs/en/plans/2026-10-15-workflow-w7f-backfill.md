# W7f — Workflows UI part 6 (schedule-trigger backfill): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the browser, a user can:
- start a backfill on a `Schedule` trigger: run the schedule for every tick of a past window `[start, end]`, with chosen inputs and labels;
- watch it on the flow's Triggers tab and on the global Triggers page (progress, ticks done), pause it, resume it, delete it, one at a time or in bulk;
- see on each execution it fired that it came from a backfill, and the labels it was started with.

**Architecture:**
- **Backend** (`crates/egret-workflow`, one migration):
  - `workflow.trigger_backfills`, one row per `(flow_id, trigger_id)` (D1); `workflow.executions.backfill_id` with the per-tick unique index rebuilt around it (D2); `workflow.executions.labels` (D3);
  - `fire_one` builds `TriggerState.backfill` from the row, overlays the inputs, passes `backfill_id` and the labels to the execution, advances or deletes the row in the same transaction, and resumes the regular schedule under `recoverMissedSchedules` when the window ends (D6); a paused backfill is not claimed (D7);
  - `sync` re-derives a re-initialised schedule from its backfill and owns the backfill rows' lifecycle (D10);
  - the trigger routes gain the backfill reads (D15), the create/pause/resume/delete routes and their bulk forms (D14), and the re-enable rule (D11).
- **Frontend** (inside W7a's lazy Workflows area): a Backfill column, a progress strip, a drawer section, bulk actions and a facet on the existing `components/workflows/triggers/` family; a lazy `BackfillDialog` with a `LabelsEditor`; the execution overview's labels and "Backfill" mark.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9, utoipa), React 19 + TypeScript, TanStack Query 5, orval, vitest 4, Playwright.

**Spec:** `docs/{en,vi}/specs/2026-10-15-workflow-w7f-backfill-design.md`. **§n refers to the spec; Dn to its default decisions.** The spec's header holds the user's dated rulings.

**A warning about this document.** Code blocks show shapes, signatures and test cases checked against the code at `d01224556` (main `f73c13647` plus the spec). They are not text to paste. Every `file:line` below was read at that commit. Line numbers move as earlier tasks land: re-read each fact before relying on it.

## Global Constraints

- **The frontend rulebook binds every frontend line:** `docs/en/frontend-rules.md`. Every frontend task review answers its reviewer checklist.
- **User decisions (spec header, 2026-10-15)**, in effect verbatim:
  - A backfill over a period that already ran **re-runs those dates**, as Kestra does: a backfill tick for a date that already has an execution creates a second one. The unique index `workflow_executions_one_per_schedule_tick` stops covering backfill executions; one execution per `(trigger, backfill, tick)` still holds.
  - **Backfill inputs and labels are in scope.** Inputs override the schedule's `inputs:` key by key under the same `refuse_templated` rule; labels need the per-execution store (D3).
- **Default decisions D1–D16 (spec header)** bind. D17 does not: see the migration number below.
- **Exact values:**
  - routes (all `workflow:manage`): `POST` and `DELETE /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill`; `PUT …/flows/{id}/triggers/{trigger_id}/backfill/paused` body `{paused: bool}`; `PUT /api/ws/{workspace}/workflows/triggers/backfill/paused` body `{triggers: [{flow_id, trigger_id}], paused}`; `POST /api/ws/{workspace}/workflows/triggers/backfill/delete` body `{triggers}`;
  - create body `{start, end?, inputs?, labels?}`; `labels` accepts Kestra's list or map form; body cap **64 KiB** on the create and the two bulk routes;
  - caps: at most **10,000** ticks, `start` at most **ten years** (120 months) back, at most **50** labels, key ≤ **128** chars, value ≤ **1,024** chars, bulk at most **100** entries;
  - codes, §5 verbatim: `backfill_window_invalid`, `backfill_end_in_future`, `backfill_start_too_early`, `backfill_no_ticks`, `backfill_too_many_ticks`, `backfill_inputs_invalid`, `backfill_labels_invalid` (400); `no_backfill` (404); `backfill_not_supported`, `trigger_disabled`, `trigger_off_queue`, `backfill_exists` (409); W7e's `invalid_body`, `body_too_large`, `source_does_not_parse`, `not_found`, `too_many_triggers` reused;
  - `backfill_refusal` values, first match wins: `flow_deleted`, `not_schedule`, `forbidden`, `disabled`, `off_queue`, `running`;
  - list filter `backfill=running|paused|any`;
  - audit actions `workflow_trigger:backfill_create`, `:backfill_pause`, `:backfill_resume`, `:backfill_delete`, resource `workflow_trigger`, id `{flow uuid}/{trigger_id}`, only on a real change;
  - the strip polls every **5 s** while a backfill of the shown flow is running, else W7e's 30 s.
- **Migration number: `0191`, not `0190`.** Checked 2026-10-15 with `git ls-tree` on every local and remote ref **and on the disk of every worktree**: refs hold `0186`–`0188` (`feat/view-governance`) and `0189` (main); but the worktree `data-portal/egret-tagdef` (branch `fix/tag-policies-service-def`) has an **untracked** `migrations/0190_tag_block_serves_the_tag_def.sql`. `0191` collides with nothing. Re-check every ref and every worktree's `migrations/` before Task 2 and before merging; take the next free number if `0191` is gone, and say so in the migration's first line, as `0189` does. The number space is shared (memory `migration-numbers-are-a-shared-namespace`).
- **Migration prose is append-only:** `0156` and `0189` are not edited; 0191 states the new predicate of the rebuilt index.
- **Earlier rulings hold:** every execution runs the saved draft; W7e's lock order (flow row `FOR NO KEY UPDATE` → the trigger's `workflow.triggers` row `FOR UPDATE` → here, then the backfill row), and `fire::resync`'s `FOR SHARE SKIP LOCKED` (`scheduler/fire.rs:382-425`); the list-row DTO's flow key is `flow_uuid` (`triggers/routes.rs:209-226`); the webhook key stays masked in every view that shows a trigger's YAML (`triggerYaml.ts`); W7a/W7c's lazy-chunk discipline: new heavy UI goes through `lazy()`, gets a marker in `frontend/scripts/assert-workflows-lazy.mjs`, and the marker is proven by a scratch static import that makes the script fail naming it.
- **A parsed model property changes `crates/egret-workflow/src/spec/schema/property-treatment.json`.** This plan adds none (a backfill lives in the database, not the YAML). If a task does add one, it regenerates the file in the same task with `EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow --lib the_committed_property_treatment_is_what_egret_does_today` and commits it.
- **Tests before code, in every task.** Each task's Step 1 writes the tests, Step 2 runs them and records the failure (the test name and the assertion or compile error that failed). A test that passes before the code exists is reported, with why. Each named mutation is applied, the named test is seen to fail, and the mutation is reverted; the report says which test died.
- **Dependencies:** none new (the date controls are native, §8). Regenerate `openapi.json` with `cargo run -q -p egret-bin -- openapi > openapi.json` (from the worktree root) and the client only with `npm run gen:api` (in `frontend/`); never bare `orval`. No prettier.
- **Environment:**
  - Work only in `/Users/1pro-l4r8/Documents/github/data-portal/egret-w7f` (branch `feat/workflow-w7f-backfill`). Never read or build `/Users/1pro-l4r8/Documents/github/data-portal/egret` (a stale checkout).
  - **Database:** the branch container `egret-w7f-postgres` on port **5449**, created by the controller (`docker run -d --name egret-w7f-postgres -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret -e POSTGRES_DB=egret -p 5449:5432 postgres:16`). Every cargo test runs with `DATABASE_URL=postgres://egret:egret@localhost:5449/egret` exported and `EGRET_ALLOW_DB_SKIP` **unset**. Never the shared dev DB on :5432, shared buckets, `deploy/docker-compose.dev.yml` services, or the kind cluster `egret`.
  - **Build dir:** `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-w7f/target-check` (worktree-local, in `.gitignore`). Keep at least 10 G free on the disk.
  - Implementers and fix rounds run **only the tests they touch** (`--test <file> <name>`, `npx vitest run <path>`). **The full gate runs once, at the end, and the controller runs it — not Task 10.** Never two cargo or vitest suites at once; wait while `pgrep -x cargo` is non-empty.
  - Commit before a long run. Count tests from the full output, never from a `tail`.
  - Never `git add -A` or `commit -a`; stage named paths. Never commit `.superpowers/`. Another agent may write `docs/vi/...` in this worktree: stage only your own files.
  - Use `command cp -f` in scripts (a `cp -i` alias exists).
  - Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Decided while planning (default decisions, 2026-10-15; each reported to the controller)

- **P1. The execution list carries the execution's OWN labels; the detail carries the merged ones.** §3.3 asks `ExecutionListItemDto` for "the merged list". The list projection deliberately omits `source` (`repo.rs:955-968`, `execution_cols!`), and the flow's labels live only in `source`, so a merged list per row would select and parse fifty YAML documents per page. So `ExecutionListItemDto` gains `labels` (the column, as stored: the labels the run was started with beyond its flow's) and `backfill_id`; `ExecutionDto` (the detail, which has `source`, `routes.rs:1636-1664`) gains `effective_labels`, the merge §3.3 defines. The overview, which reads `ExecutionDto` (`ExecutionOverview.tsx:38`), shows `effective_labels`.
- **P2. A cron change that would give a backfill more than 10,000 ticks ends it.** §3.7 recounts `total_ticks = ticks_done + ticks left` on a re-initialisation and does not say what happens when that exceeds the table's `CHECK (total_ticks BETWEEN 1 AND 10000)`: the CHECK would abort the save with a 500 (a daily window of a week becomes 10,080 ticks under `* * * * *`). `sync` must never fail a save over a backfill, so it deletes the backfill, resumes the regular schedule as a delete does (§3.5), writes `last_error = "the backfill ended: the new schedule gives more than 10000 ticks in its window"`, and logs at `warn`.
- **P3. A re-snap with no tick keeps the stored position.** §3.7 re-snaps `previous_next_evaluation_date` "to the first tick of the new cron at or after it". A cron with no future tick gives none, and the column is `NOT NULL`: the stored value is kept, and the resume (§3.6) then finds no tick and takes the row off the queue with `never_fires`.
- **P4. `ticks_done` never exceeds `total_ticks`.** The advance writes `ticks_done = LEAST(ticks_done + 1, total_ticks)`. Both counts walk the same `Cron::next_after` sequence, so the clamp should never bite; but if it ever did, an unclamped write would violate the CHECK, fail the claimed trigger's transaction and end every pass (`fire.rs:88-89`): every schedule of the deployment would stop. A test drains a window exactly and asserts `ticks_done == total_ticks` just before the row goes.
- **P5. The fresh read is staged through a test-only hook.** §6 asks for "pause after the claim's snapshot (the fresh read) writes nothing". The window is between the claim's `SELECT … FOR UPDATE OF t SKIP LOCKED` and the next statement; the claim never waits (`SKIP LOCKED`) and nothing is written between the two, so no table trigger or lock can hold a pass there. `scheduler::fire` gets a `#[cfg(any(test, feature = "test-util"))]` hook that parks the pass right after the claim (the precedent is `worker`'s test-util exports, `src/worker/mod.rs:100-110`). It costs nothing in a release build.
- **P6. The dialog's bundle flag is a new `backfill` flag**, not `editor` as §4.3 says. `editor` checks only that a marker stays out of the flow list's chunk (`assert-workflows-lazy.mjs:383-392`); it would pass if `FlowTriggersTab.tsx` imported the dialog statically. `backfill` also keeps the marker out of what loads with the flow pages (`FLOW_PAGE_HOSTS`, as `files` does, `:361-372`), which is what D16 ("a lazy chunk") means.
- **P7. `deserialize_labels` is lifted to `pub(crate)`** (`models/flow.rs:375`, today `pub(super)`) so the create request accepts Kestra's list or map form through the one deserializer.

## Gates

- **Backend** (each with `DATABASE_URL` and `CARGO_TARGET_DIR` as above):
  - `cargo fmt --check`
  - `rtk proxy cargo clippy --workspace --all-targets -- -D warnings`
  - `EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 cargo test -p egret-workflow --no-fail-fast`
  - `cargo test -p egret-server` (it carries `tests/feature_route_coverage.rs`, which reads the OpenAPI document)
- **Frontend** (from `frontend/`): `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`, `npm run test:bundle:workflows`, `npm run test:bundle:monaco`.

**Per task**, an implementer runs `cargo fmt --check`, clippy on `-p egret-workflow --all-targets`, and only the test files it touched; or, for frontend tasks, `npm run lint`, `npx tsc -b`, the vitest files it touched (`npx vitest run <path>`), and `npm run build && npm run test:bundle:workflows` when a module was added. **The controller runs every gate above once, after Task 10, and records the counts.**

## File map

| Path | Task | Responsibility |
|---|---|---|
| `crates/egret-workflow/src/trigger/schedule.rs` | 1 | `first_tick_at_or_after`, `first_tick_after` |
| `crates/egret-workflow/src/trigger/mod.rs` | 1 | inclusive `attach_backfill`; `resume_from`, `resume_after_backfill`, `BackfillEnd`; doc of `TriggerState` (§3.12) |
| `crates/egret-workflow/src/labels.rs` (new), `src/lib.rs` | 1 | `execution_labels`, `own_labels`, `backfill_labels_refusal` |
| `crates/egret-workflow/src/scheduler/fire.rs` | 1, 4 | `overlay_inputs`, `refuse_templated` lifted (1); the fire wiring (4) |
| `migrations/0191_workflow_trigger_backfills.sql` | 2 | §3.1–§3.3's SQL |
| `crates/egret-workflow/src/scheduler/start.rs`, `scheduler/mod.rs` | 2 | `ONE_PER_BACKFILL_TICK`, `classify`; statement 2's doc |
| `crates/egret-workflow/src/worker/claim.rs` + every `NewExecution {` | 2 | `labels`, `backfill_id` |
| `crates/egret-workflow/src/worker/render.rs`, `flow_trigger/process.rs`, `worker/replay.rs`, `worker/spawn.rs`, `routes.rs` (replay, DTOs), `repo.rs` (`ExecutionRow`, `execution_cols!`) | 3 | labels read, merged, copied, shown |
| `crates/egret-workflow/src/scheduler/backfill.rs` (new) | 4 | `BackfillRow`, its reads |
| `crates/egret-workflow/src/scheduler/sync.rs` | 5 | re-derivation, lifecycle |
| `crates/egret-workflow/src/triggers/{state,entry,routes,switch}.rs` | 6 | reads, `can_backfill`, filter, re-enable |
| `crates/egret-workflow/src/triggers/backfill.rs`, `triggers/backfill_routes.rs` (new), `routes.rs` (router, OpenAPI) | 7 | the write transactions and routes |
| `frontend/src/components/workflows/triggers/*`, `routes/workflows/{TriggersList,ExecutionOverview,queryKeys}` | 8 | column, strip, drawer, bulk, facet, overview |
| `frontend/src/components/workflows/triggers/{BackfillDialog,LabelsEditor}.tsx`, `scripts/assert-workflows-lazy.mjs` | 9 | the create form |
| `frontend/tests/e2e/workflows-backfill.spec.ts`, the spec (EN; VI if owned) | 10 | end to end, Kestra comparison |

---

### Task 1: The pure model — inclusive start, the resume rule, labels, the inputs overlay (§3.3, §3.4, §3.6) — **sonnet**

Pure code, no database. It lands first because Tasks 4–7 call all of it.

**Files:**
- Modify: `crates/egret-workflow/src/trigger/schedule.rs` (beside `last_tick_at_or_before`/`ticks_through`, `:1419-1446`).
- Modify: `crates/egret-workflow/src/trigger/mod.rs` (`attach_backfill`, `:263-281`; a new `impl TriggerState` block or functions beside it; the `TriggerState` doc `:220-241`, §3.12), `trigger/mod.rs:131-134` (re-exports).
- Create: `crates/egret-workflow/src/labels.rs`; Modify: `crates/egret-workflow/src/lib.rs` (`pub(crate) mod labels;`, and the comment at `:99-102`, §3.12: "backfill's routes are `triggers::backfill_routes` (W7f)").
- Modify: `crates/egret-workflow/src/models/flow.rs:375` (`pub(super)` → `pub(crate)`, P7) and `models/mod.rs:15-18` (`pub(crate) use flow::deserialize_labels;`).
- Modify: `crates/egret-workflow/src/scheduler/fire.rs:427-442` (`refuse_templated` → `pub(crate)`; `overlay_inputs` beside it).
- Test: the same files' `#[cfg(test)] mod tests`.

**Interfaces — Produces:**
```rust
// trigger/schedule.rs, impl ScheduleTrigger
/// The first tick `t >= at` (`next_after(at − 1ns)`, as `ticks_through` starts).
pub fn first_tick_at_or_after(&self, at: DateTime<Utc>) -> Option<DateTime<Utc>>;
/// The first tick `t > at`.
pub fn first_tick_after(&self, at: DateTime<Utc>) -> Option<DateTime<Utc>>;

// trigger/mod.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillEnd {
    /// The window's last tick fired.
    Completed { window_end: DateTime<Utc> },
    /// Deleted (or ended by `sync`, P2) with `current_tick` not yet fired.
    Deleted { current_tick: DateTime<Utc> },
}
impl TriggerState {
    /// §3.6's `from`: completion `max(prev, first tick after window_end)`
    /// (`prev` alone when there is none); delete `max(prev, current_tick)`.
    pub fn resume_from(trigger: &ScheduleTrigger, prev: DateTime<Utc>, end: BackfillEnd) -> DateTime<Utc>;
    /// §3.6's rule. `from > now`: `Some(from)`. Else `All`: `Some(from)`;
    /// `Last`: the latest tick at or before `now` (`from` if none is >= `from`);
    /// `None`: `trigger.next_evaluation_date(None, None, now)` (the first tick
    /// after now, as W7e's skip computes it). `None` result = off the queue.
    pub fn resume_after_backfill(trigger: &ScheduleTrigger, from: DateTime<Utc>,
        policy: RecoverMissed, now: DateTime<Utc>) -> Option<DateTime<Utc>>;
}

// labels.rs
pub(crate) const MAX_BACKFILL_LABELS: usize = 50;
pub(crate) const MAX_LABEL_KEY_CHARS: usize = 128;
pub(crate) const MAX_LABEL_VALUE_CHARS: usize = 1024;
/// Kestra's `Execution.newExecution` + `Label.deduplicate`: `flow` then `own`,
/// an empty key or value dropped, the LAST value kept in the FIRST key's position.
pub(crate) fn execution_labels(flow: &[Label], own: &[Label]) -> Vec<Label>;
/// `workflow.executions.labels` read back: NULL or a malformed item is no label.
pub(crate) fn own_labels(v: Option<&serde_json::Value>) -> Vec<Label>;
/// §3.4's label rules; `Some(sentence naming the label)` on the first breach.
pub(crate) fn backfill_labels_refusal(labels: &[Label]) -> Option<String>;

// scheduler/fire.rs
pub(crate) fn refuse_templated(inputs: &BTreeMap<String, serde_json::Value>) -> Option<String>;
/// Kestra's `putAll`: `schedule` overlaid by `backfill`, key by key.
pub(crate) fn overlay_inputs(schedule: &BTreeMap<String, serde_json::Value>,
    backfill: &serde_json::Map<String, serde_json::Value>) -> BTreeMap<String, serde_json::Value>;
```

`attach_backfill`, inclusive (D4). Today it seeds `current_date = start` and recomputes with `next_after(start)`, which is strictly after (`:277-280`, `schedule.rs:1531-1544`):
```rust
pub fn attach_backfill(&mut self, trigger: &ScheduleTrigger, start: DateTime<Utc>,
                       end: Option<DateTime<Utc>>, clock: &dyn Clock) {
    let now = clock.now();
    let bf = Backfill::create(start, end, now);
    // D4: a tick exactly at `start` belongs to the window.
    let next = trigger
        .first_tick_at_or_after(bf.start)
        .filter(|t| *t <= bf.end)
        // An empty window: back to the live clock, as Kestra's recompute lands there.
        .or_else(|| trigger.next_evaluation_date(None, None, now));
    self.backfill = Some(bf);
    self.update_for_next_evaluation_date(next); // clears the backfill when next > end
}
```

- [ ] **Step 1: Write the tests (red).**
  1. `first_tick_at_or_after` with `*/15 * * * *`: at `10:07` → `10:15`; at exactly `10:15` → `10:15`; `first_tick_after` at `10:15` → `10:30`; with `timezone: Asia/Ho_Chi_Minh` and `0 2 * * *` at `2026-06-15T19:00Z` (02:00 local on 06-16) → itself.
  2. **Inclusive start:** `0 2 * * *`, window `[2026-06-01T02:00Z, 2026-06-03T02:00Z]` → `run_until_idle` fires `06-01T02:00`, `06-02T02:00`, `06-03T02:00` (three, both ends); the existing `a_backfill_produces_the_windows_executions_in_order`, `an_open_ended_backfill_ends_at_the_clock` and `a_paused_backfill_fires_nothing_and_advances_nothing` (`trigger/mod.rs:843, 893, 927`) stay green unchanged; a window holding no tick (`[06-01T03:00Z, 06-01T04:00Z]`) attaches nothing (`backfill: None`) and the next date is the live clock's.
  3. `resume_from`: `Completed` with `prev` before and after `first_tick_after(window_end)` (each `max`); `Deleted` with `prev` before and after `current_tick`.
  4. `resume_after_backfill` on `*/1 * * * *`, `now = 10:30:30`: `from = 10:40` (future) → `10:40` for every policy; `from = 10:20`: `All` → `10:20`, `Last` → `10:30`, `None` → `10:31`; a cron with no tick after now (`0 0 30 2 *`) and `None` → `None`.
  5. `execution_labels`: flow `[team: data, env: prod]` + own `[team: ops, reason: x]` → `[team: ops, env: prod, reason: x]` (order and last-wins); an empty key or value in either list is dropped; own `[]` → the flow's list unchanged.
  6. `own_labels`: `None`, `json!(null)`, `json!([{"key":"a","value":"b"}, 5, {"key":"c"}])` → `[]`, `[]`, `[a: b]`.
  7. `backfill_labels_refusal`: `system.x` key; empty key; empty value; a value with `{{`; a value with `{%`; 51 labels; a 129-char key; a 1,025-char value → each `Some(..)` naming the label; 50 labels at the caps → `None`.
  8. `overlay_inputs` + `refuse_templated`: schedule `{a: "{{ x }}", b: 1}` overlaid by `{a: "plain"}` → `{a: "plain", b: 1}` and `refuse_templated` is `None`; overlaid by `{c: 2}` → `refuse_templated` names `inputs.a`.
  9. **Mutations** (record which tests die): use `first_tick_after` in `attach_backfill` (test 2's first tick dies); keep the first value instead of the last in `execution_labels` (test 5); overlay the other way round (test 8).
- [ ] **Step 2: Run them, see them fail.** `cargo test -p egret-workflow --lib first_tick resume_ execution_labels own_labels backfill_labels overlay_inputs inclusive` (adjust the filters to the names you chose).
- [ ] **Step 3: Implement.** Rewrite the `TriggerState` doc (§3.12): the backfill's database home is `workflow.trigger_backfills`; the pure `Backfill` stays reduced to the window (`schedule.rs:1628-1639`), its inputs and labels live beside it in the row. Rewrite `attach_backfill`'s doc for D4 (it cites `onCreateBackfill`; say the start is inclusive here and exclusive in Kestra).
- [ ] **Step 4: Run the tests, then `cargo test -p egret-workflow --test flow_schema`** (the model changed nowhere; it must stay green).
- [ ] **Step 5: Commit.** `feat(workflow): inclusive backfill start, the resume rule, execution label merge`

### Task 2: The migration, `backfill_id` and `labels` on an execution (§3.1, §3.2, §3.3 SQL) — **opus**

**Files:**
- Create: `migrations/0191_workflow_trigger_backfills.sql`.
- Modify: `crates/egret-workflow/src/scheduler/start.rs:70-116` (`ONE_PER_BACKFILL_TICK`, `classify`, `Started::AlreadyFired`'s doc), `scheduler/mod.rs:74-91` (statement 2 names both indexes, §3.12), `scheduler/fire.rs:261-264` (the comment).
- Modify: `crates/egret-workflow/src/worker/claim.rs:2605-2653` (`NewExecution`), `:2699-2760` (destructure, the `INSERT` binds `labels` and `backfill_id`).
- Modify — every construction, each passing `labels: None, backfill_id: None` (the compiler lists them; at `d01224556` there are 16): `src/routes.rs:2668, 2944`; `src/scheduler/fire.rs:318`; `src/flow_trigger/process.rs:996`; `src/worker/claim.rs:2572`; `tests/storage_tasks.rs:652, 815`; `tests/storage_render.rs:332`; `tests/prefetch_hold.rs:1669`; `tests/worker.rs:8101, 8131, 8170, 8252, 8377, 8579`. (The spec's list at §3.3 names only the `src/` ones.) Task 3 changes the replay route's value; Task 4 the fire's.
- Test: `crates/egret-workflow/tests/scheduler_fire.rs` (beside `the_unique_index_refuses_a_second_fire`, `:511`).

**Interfaces — Produces:**
```rust
// worker/claim.rs, NewExecution
/// The labels this execution starts with beyond its flow's own `labels:`
/// (`[{key, value}]`, D3). `None` stores NULL.
pub labels: Option<serde_json::Value>,
/// The backfill that fired this tick (D2); `Some` only with `scheduled_date`.
pub backfill_id: Option<Uuid>,

// scheduler/start.rs
/// One execution per (backfill, tick) (migration 0191).
pub(crate) const ONE_PER_BACKFILL_TICK: &str = "workflow_executions_one_per_backfill_tick";
// classify: Some(ONE_PER_BACKFILL_TICK) => Started::AlreadyFired(format!(
//     "this backfill tick already has an execution: {err}")),
```

The migration, in this order (sqlx runs the file in one transaction, so the index is never absent):
```sql
-- 0191, not 0190: origin/main ends at 0189, feat/view-governance holds 0186-0188, and the
-- egret-tagdef worktree (fix/tag-policies-service-def) holds an uncommitted 0190, so 0191
-- collides with none of them in any merge order.
--
-- Workflow W7f: schedule-trigger backfill. See docs/en/specs/2026-10-15-workflow-w7f-backfill-design.md §3.1-§3.3.
CREATE TABLE workflow.trigger_backfills ( … §3.1 verbatim, its comments included … );
ALTER TABLE workflow.executions ADD COLUMN backfill_id uuid;
ALTER TABLE workflow.executions ADD CONSTRAINT workflow_executions_backfill_is_a_schedule_fire
  CHECK (backfill_id IS NULL OR scheduled_date IS NOT NULL);
-- The per-tick index of 0156 now covers regular fires only (user decision, 2026-10-15:
-- a backfill re-runs dates that already ran). 0156's prose is not edited.
DROP INDEX workflow.workflow_executions_one_per_schedule_tick;
CREATE UNIQUE INDEX workflow_executions_one_per_schedule_tick
  ON workflow.executions (flow_id, (execution_trigger->>'id'), scheduled_date)
  WHERE scheduled_date IS NOT NULL AND backfill_id IS NULL;
CREATE UNIQUE INDEX workflow_executions_one_per_backfill_tick
  ON workflow.executions (backfill_id, scheduled_date) WHERE backfill_id IS NOT NULL;
ALTER TABLE workflow.executions ADD COLUMN labels jsonb
  CHECK (labels IS NULL OR jsonb_typeof(labels) = 'array');
```
Add §3.2's sentence about the rebuild not being `CONCURRENTLY` (as `0189:43-48` words its own), and the comment of §3.3 on `labels`. The index rebuild's write block is one pass over the scheduled executions.

- [ ] **Step 1: Write the tests (red)** in `tests/scheduler_fire.rs`, with its `fx()` (it serialises on `SERIAL` and clears `next_evaluation_date`):
  1. **The regular index ignores backfill executions:** fire one tick (as `:511-526`); copy the row with `backfill_id = gen_random_uuid()` and the **same** `scheduled_date` → accepted; copy it again without `backfill_id` → `23505` on `workflow_executions_one_per_schedule_tick` (the regular half still refuses).
  2. **One per backfill tick:** two copies with the same `backfill_id` and `scheduled_date` → the second is `23505` on `workflow_executions_one_per_backfill_tick`; control: the same pair with another `backfill_id` is accepted.
  3. **The CHECKs:** `backfill_id` set on a row with `scheduled_date` NULL (an API-shaped row) → `23514` on `workflow_executions_backfill_is_a_schedule_fire`; `labels = '{}'::jsonb` → `23514`; `labels = '[]'` → accepted.
  4. **The insert writes both columns:** `start_execution_in` (test-util, as `tests/worker.rs:8093-8117` calls it) with `labels: Some(json!([{"key":"a","value":"b"}]))`, a schedule `trigger` record, a `scheduled_date` and a `backfill_id` → the row holds them; with both `None` → both NULL.
  5. **The constraint name reaches the caller:** two `start_execution_in` calls with one `(backfill_id, scheduled_date)` → the second's `Err` downcasts to a `sqlx` database error whose `constraint()` is `workflow_executions_one_per_backfill_tick` (what `classify` reads, `start.rs:100-104`). `classify`'s mapping to `AlreadyFired` is asserted end to end by Task 4's test 6, which needs the fire path.
  6. `the_unique_index_refuses_a_second_fire` (`:511`) and `a_tick_that_already_fired_is_consumed_not_taken_off_the_queue` unchanged and green.
  7. **Mutations:** drop `AND backfill_id IS NULL` from the rebuilt index (test 1's first copy dies); drop `workflow_executions_one_per_backfill_tick` (test 2 dies).
- [ ] **Step 2: Run them, see them fail** — `cargo test -p egret-workflow --test scheduler_fire` (the columns do not exist).
- [ ] **Step 3: Implement** the migration, the struct fields, the `INSERT` (two more columns and binds, `$14`, `$15`), every construction, `classify`, and the two comments.
- [ ] **Step 4: Run** `--test scheduler_fire`, `--test worker start_execution_in` (the constructions changed), `--test storage_tasks --test storage_render --test prefetch_hold` (they compile against the struct), and `--test oracle` (render and fire must not have moved).
- [ ] **Step 5: Commit.** `feat(workflow): trigger_backfills table, backfill_id and labels on executions`

### Task 3: Execution labels — render, the Flow-trigger source, copies, the DTOs (§3.3, §4.4 backend) — **sonnet**

**Files:**
- Modify: `crates/egret-workflow/src/worker/render.rs:168-178` (select `labels`), `:247-252` (the map from `labels::execution_labels(&f.labels, &own)`); `:504` (`system` still inserted last) unchanged.
- Modify: `crates/egret-workflow/src/flow_trigger/process.rs:778-797` (**`load_source`**, which §3.3 calls `source_of`: select `e.labels`, add `labels: Option<serde_json::Value>` to `SourceRow` `:181-191`, merge with the source flow's), `:1080-1089` (the comment about "egret has no execution labels" narrows to executions without own labels).
- Modify: `crates/egret-workflow/src/worker/replay.rs:243-270` (the retry `INSERT` copies `labels` from the parent row; add `labels` to whatever statement reads `row` and bind it), `worker/spawn.rs:393-410` (the loop iteration: `(SELECT labels FROM workflow.executions WHERE id = $6)`, the pattern the `inputs` column already uses).
- Modify: `crates/egret-workflow/src/routes.rs:2944-2959` (the replay route passes `labels: source.execution.labels.clone()`; never `backfill_id`, §3.3).
- Modify: `crates/egret-workflow/src/repo.rs:960-967` (`execution_cols!` gains `labels, backfill_id`), `:993-1040` (`ExecutionRow` gains both).
- Modify: `crates/egret-workflow/src/routes.rs:1417-1464` (`ExecutionListItemDto`), `:1467-` (`From<ExecutionRow>`), `:1636-1664` (`ExecutionDto::effective_labels`, P1), OpenAPI `components(..)` (`LabelDto`); `openapi.json`, the generated client.
- Test: `tests/worker.rs` (render), `tests/flow_trigger_fire.rs` (source), `tests/executions_routes.rs` (DTOs; the key-set test at `:838-860` gains `labels`, `backfill_id` in the list set and `effective_labels` in the detail set), `tests/worker.rs` or `tests/runtime.rs` for the retry and loop copies (wherever their existing copy tests of `inputs` live — find them with `grep -n "inputs" tests/worker.rs | grep -i "loop\|retry\|childExecution"`).

**Interfaces — Produces:**
```rust
// routes.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LabelDto { pub key: String, pub value: String }
// ExecutionListItemDto gains:
/// The labels this execution was started with beyond its flow's own `labels:`
/// (a backfill's, a replay's copy); `null` for none. Not the flow's.
pub labels: Option<Vec<LabelDto>>,
/// The backfill that fired this execution (W7f), `null` otherwise.
pub backfill_id: Option<Uuid>,
// ExecutionDto gains:
/// What the run sees as `labels`: the flow's, then `labels`, the later key winning.
pub effective_labels: Vec<LabelDto>,
```
`From<ExecutionRow>` maps the jsonb through `labels::own_labels` and gives `None` for an empty list; `ExecutionDto::new` parses `d.source` (`.ok()`; no labels when it does not parse) and calls `execution_labels`.

- [ ] **Step 1: Write the tests (red).**
  1. **Render** (`tests/worker.rs`, beside `the_render_surface_reaches_inputs_and_outputs_and_still_refuses_labels`, `:969`): a flow with `labels: {team: data, env: prod}` and a `Return` whose `format` is `"{{ labels.team }}/{{ labels.env }}/{{ labels.reason }}"`, started through `start_execution_in` with `labels: Some(json!([{"key":"team","value":"ops"},{"key":"reason","value":"x"}]))` → output `ops/prod/x`; the same flow started with `labels: None` → `{{ labels.reason }}` FAILS the run (control: nothing invents the key).
  2. **Flow-trigger source** (`tests/flow_trigger_fire.rs`): an upstream execution with own labels `[reason: backfill]`; (a) a window-free listener whose `inputs:` render `{{ trigger.executionLabels.reason }}` fires with `backfill`; (b) a `dependsOn` entry with `labels: {reason: backfill}` is met (`flow_trigger_windows.rs`'s fixtures show the shape); (c) control: an upstream without own labels does not meet (b).
  3. **Copies:** the replay route's new execution has the source's `labels` and `backfill_id` NULL; a `CREATE_NEW_EXECUTION` retry's child and a loop iteration copy `labels`; a subflow child does not (`inheritLabels` is not honoured, `worker/subflow.rs:79-82`).
  4. **DTOs:** `GET …/executions` row carries `labels` (own only, `null` when none) and `backfill_id`; `GET …/executions/{id}` carries `effective_labels` merged (flow `team: data` + own `team: ops` → `team: ops` in the first key's position); the key-set test updated.
  5. **Mutations:** skip the merge in `load_source` (2a and 2b die); select the flow's labels only in `Surface::load` (1 dies); drop `labels` from the replay route (3 dies).
- [ ] **Step 2: Run them, see them fail.**
- [ ] **Step 3: Implement.** Update W13 spec's recorded divergence only if this task owns the docs at that moment (otherwise list it for Task 10).
- [ ] **Step 4: Regenerate** `openapi.json` and the client; `npx tsc -b` in `frontend/` (the DTOs grew; nothing else changes).
- [ ] **Step 5: Run** the touched test files, `--test oracle`, and `cargo test -p egret-server --test feature_route_coverage`.
- [ ] **Step 6: Commit.** `feat(workflow): per-execution labels rendered, matched by Flow triggers, copied on replay`

### Task 4: Firing a backfill — the claim, `fire_one`, completion and the resume (§3.5 D7, §3.6) — **opus**

**Files:**
- Create: `crates/egret-workflow/src/scheduler/backfill.rs` (`BackfillRow`, its reads; `mod backfill;` and `pub(crate) use` in `scheduler/mod.rs:166-179`).
- Modify: `crates/egret-workflow/src/scheduler/fire.rs`: the claim (`:145-159`), `fire_one` (`:180-303`), `start` (`:314-346`), and the test-only hook (P5).
- Modify: `crates/egret-workflow/tests/common/mod.rs` (helpers below).
- Test: a new `crates/egret-workflow/tests/backfill_fire.rs` (its own `SERIAL` and `fx()`, copied from `scheduler_fire.rs:44-91`: a claim is global, so the tests take turns), and `tests/runtime.rs` (P5's race).

**Interfaces — Produces:**
```rust
// scheduler/backfill.rs
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub(crate) struct BackfillRow {
    pub id: Uuid, pub flow_id: Uuid, pub trigger_id: String,
    pub window_start: DateTime<Utc>, pub window_end: DateTime<Utc>, pub current_tick: DateTime<Utc>,
    pub paused: bool, pub inputs: serde_json::Value, pub labels: serde_json::Value,
    pub previous_next_evaluation_date: DateTime<Utc>, pub total_ticks: i32, pub ticks_done: i32,
}
pub(crate) const BACKFILL_COLS: &str = "id, flow_id, trigger_id, window_start, window_end, current_tick, \
    paused, inputs, labels, previous_next_evaluation_date, total_ticks, ticks_done";
/// A plain read (a fresh statement: under READ COMMITTED, the newest committed row).
pub(crate) async fn read(conn: &mut PgConnection, flow: Uuid, trigger: &str) -> Result<Option<BackfillRow>>;
/// The same, `FOR UPDATE` (the routes and `sync`, after the trigger row).
pub(crate) async fn read_for_update(conn: &mut PgConnection, flow: Uuid, trigger: &str) -> Result<Option<BackfillRow>>;
impl BackfillRow {
    /// The pure model's window; `paused: false` (a paused row is never ticked).
    pub fn window(&self) -> crate::trigger::Backfill;
}

// tests/common/mod.rs — staging, until Task 7's route exists (as W7e staged its switch by SQL)
/// Insert a backfill for `trig` exactly as the create route will: the row, the trigger's
/// `next_evaluation_date` moved to `first_tick`, its old value kept as
/// `previous_next_evaluation_date`. Returns the backfill id.
pub async fn stage_backfill(db: &Db, flow: Uuid, trig: &str, window: (DateTime<Utc>, DateTime<Utc>),
    first_tick: DateTime<Utc>, total_ticks: i32, inputs: Value, labels: Value) -> Uuid;
pub async fn backfill_row(db: &Db, flow: Uuid, trig: &str) -> Option<Value>; // row_to_json
```

The claim gains (D7), after `AND NOT t.disabled`:
```sql
AND NOT EXISTS (SELECT 1 FROM workflow.trigger_backfills b
                 WHERE b.flow_id = t.flow_id AND b.trigger_id = t.trigger_id AND b.paused)
```

`fire_one`'s shape (after `resolved`, `:216-222`):
```rust
// §3.6: a fresh statement, after the claim's lock; every writer of the backfill row holds
// the trigger row first (§3.11), so this read is current. A pause that committed between
// the claim's snapshot and its lock is seen here.
#[cfg(any(test, feature = "test-util"))]
hold::after_claim().await;
let backfill = super::backfill::read(at.conn(), c.flow_id, &c.trigger_id).await?;
if backfill.as_ref().is_some_and(|b| b.paused) {
    return Ok(Fate::Frozen);
}
let mut inputs = schedule.inputs.clone().unwrap_or_default();
if let Some(b) = &backfill {
    if let Some(over) = b.inputs.as_object() { inputs = overlay_inputs(&inputs, over); }
}
let refused = refuse_templated(&inputs);
let state = TriggerState { /* as today */ backfill: backfill.as_ref().map(BackfillRow::window), .. };
let mut t = tick(&resolved, &state, &inputs, &FixedClock(c.db_now));
// … the outcome match unchanged, except `start(at, c, &e, backfill.as_ref())` …
// Before the final UPDATE, in the same transaction:
if let Some(b) = &backfill {
    match &t.state.backfill {
        Some(next) => { /* UPDATE workflow.trigger_backfills SET current_tick = $3,
            ticks_done = LEAST(ticks_done + 1, total_ticks), updated_at = now()
            WHERE flow_id = $1 AND trigger_id = $2   (P4) */ }
        None => {
            // DELETE the row; the next date is the resume, not `tick`'s live-clock date.
            let policy = RecoverMissed::from_model(schedule.recover_missed_schedules.as_ref())
                .unwrap_or(RecoverMissed::All); // §3.6: unknown = ALL
            let from = TriggerState::resume_from(&resolved, b.previous_next_evaluation_date,
                BackfillEnd::Completed { window_end: b.window_end });
            t.state.next_evaluation_date =
                TriggerState::resume_after_backfill(&resolved, from, policy, c.db_now);
            // None: off the queue, `last_error = never_fires(schedule.cron, c.db_now)`
            // unless the outcome already set one.
        }
    }
}
```
`start` passes `backfill_id: Some(b.id)` and `labels: Some(b.labels)` (NULL when the array is empty) when `e.from_backfill` and a row was read; `None` otherwise. `execution_trigger` is `schedule_trigger_record(e)` unchanged (§3.2: the oracle shape).

The hook (P5), `#[cfg(any(test, feature = "test-util"))]`, in `fire.rs`: a process-wide `Mutex<Option<(Arc<Notify>, Arc<Notify>)>>` that `hold::arm() -> Hold` sets; `after_claim()` notifies "reached" and awaits "release" when armed, and returns at once otherwise. `Hold { reached().await, release() }`. Re-export it under `egret_workflow::scheduler::test_hold` behind the same `cfg`.

- [ ] **Step 1: Write the tests (red)** in `tests/backfill_fire.rs`. Windows are built from the database clock: `w0 = date_trunc('minute', now()) - interval '10 minutes'`, on the fixture cron `*/1 * * * *` (`scheduler_fire.rs:34-42`'s `flow_yaml`), so every tick is past and fixed.
  1. **Drain:** a 5-tick window `[w0, w0+4m]`, staged with `inputs {region: eu}` and `labels [{team: ops}, {reason: test}]`, on a flow whose schedule declares `inputs: {region: us, size: 3}`: five `fire_due` passes fire 5 executions, in order, `scheduled_date` = `w0 … w0+4m`; each has the same `backfill_id`, `inputs {region: eu, size: 3}`, `labels` as staged, and `execution_trigger` equal to what `schedule_trigger_record` gives a live fire of the same tick (compare with a live fire's record built in the test from `ScheduleTrigger::resolve` + `tick`); after pass 4 the row has `ticks_done = 4`, `current_tick = w0+4m`; after pass 5 the row is gone and `ticks_done` reached `total_ticks` just before (read it after pass 4 + 1 tick, P4).
  2. **The resume after completion:** `previous_next_evaluation_date` in the future → the trigger's next date is that value and no regular execution is created. Then three variants with `previous_next_evaluation_date = w0 - 3m` (a regular backlog of 3 ticks before the window, 4..8 minutes ago) — `recoverMissedSchedules` absent / `ALL`: next date = `max(prev, w0+5m)` = `w0+5m`; `LAST`: the latest tick at or before now; `NONE`: the first tick after now. (§3.6's `max` drops the 3 pre-window regular ticks: assert it, it is the spec's rule; Task 10 reports it.)
  3. **Re-run (user decision):** fire 3 regular ticks first (`make_due(.., 3)` and three passes), then stage a window over those three dates → 3 more executions for the same `scheduled_date`s, with `backfill_id`; stage a second backfill (new id) over the same window → 3 more; 9 executions, 3 per date.
  4. **A paused backfill is not claimed** (D7): staged with `paused = true, paused_at = now()`; three passes each return `PassReport::default()` (not `frozen: 1`: that would mean it was claimed), the row's `updated_at` and the trigger row are byte-identical (the `Row` struct of `scheduler_fire.rs:95-102`); `UPDATE … paused = false` → the next pass fires `current_tick`.
  5. **Templated merged inputs consume the tick** (§3.4, `fire.rs:256-258`): a schedule input `"{{ x }}"` not overridden → no execution, `skipped: 1`, `last_error` names `inputs.x`, and the backfill **advanced** (`ticks_done = 1`).
  6. **`AlreadyFired` advances the backfill:** insert by SQL an execution with the staged `backfill_id` and `scheduled_date = w0` before the first pass → the pass reports `skipped: 1`, `last_error` names "this backfill tick already has an execution", and `current_tick` moved to `w0+1m`.
  7. **Untouched on the error paths:** a source that no longer parses (SQL) → `take_off_queue`, the backfill row unchanged; `record_error` (stage a `Failed` start with the `w12_fail_*` injection of `scheduler_fire.rs:450-490` on `workflow.executions`) → the row unchanged and the next pass retries the same `current_tick`.
  8. **The oracle:** `cargo test -p egret-workflow --lib schedule_trigger_record_matches_the_captured_execution` stays green unchanged.
  9. **The fresh read (P5)**, in `tests/runtime.rs` (it already drives holds): arm the hook, start `fire_due` on a due backfill, await `reached`, set `paused = true` on the backfill row by SQL, `release` → the pass returns `frozen: 1`, no execution exists, the trigger row and the backfill row are unchanged.
  10. **Mutations** (record which tests die): drop the claim's `NOT EXISTS` term (4 dies: `frozen: 1` instead of nothing); read the backfill from the claim's snapshot instead (join it into the claim) and drop the fresh read (9 dies); keep `tick`'s live-clock date on completion instead of the resume (2's `ALL` case dies); drop `LEAST` (no test should die — say so; P4 is a guard, not a behaviour).
- [ ] **Step 2: Run them, see them fail** — `cargo test -p egret-workflow --test backfill_fire`.
- [ ] **Step 3: Implement.** The `tick` debug panic for `NotDue`/`NotSchedulable` (`fire.rs:241-254`) stays: a backfill date is past and a disabled trigger is not claimed (§3.6 "Safety"). Say in `fire_one`'s doc that `BackfillPaused` is unreachable here (the claim and the fresh read exclude it).
- [ ] **Step 4: Run** `--test backfill_fire`, `--test scheduler_fire`, `--test runtime <test 9's name>`, `--lib schedule_trigger_record`.
- [ ] **Step 5: Commit.** `feat(workflow): fire schedule backfills tick by tick, resume the regular schedule after`

### Task 5: `sync` and the backfill — re-derivation and lifecycle (§3.7, D10, P2, P3) — **opus**

**Files:**
- Modify: `crates/egret-workflow/src/scheduler/sync.rs` (`sync`, `:78-203`; its doc `:40-77` gains point 7).
- Modify: `crates/egret-workflow/src/scheduler/mod.rs:114-150` (statement 4's doc: `sync` reads the flow's trigger rows and backfill rows `FOR UPDATE` before the upsert).
- Test: `crates/egret-workflow/tests/scheduler_sync.rs`, `tests/runtime.rs` (one concurrency test).

**Interfaces — Consumes:** Task 1's `first_tick_at_or_after`, `ticks_through`, `resume_from`, `resume_after_backfill`, `BackfillEnd`; Task 4's `backfill::read_for_update`, `BackfillRow`. **Produces:** nothing public.

The decision moves into Rust (§3.7). Before the schedules loop, in the same transaction and in the lock order (trigger rows, then backfill rows, each `ORDER BY trigger_id`):
```rust
let current: HashMap<String, (String /*spec_key*/, Option<DateTime<Utc>>)> = sqlx::query_as(
    "SELECT trigger_id, spec_key, next_evaluation_date FROM workflow.triggers \
      WHERE flow_id = $1 ORDER BY trigger_id FOR UPDATE")…;
let backfills: HashMap<String, BackfillRow> = /* SELECT BACKFILL_COLS … ORDER BY trigger_id FOR UPDATE */;
// In the loop, per schedule `t` with key `key`:
let reinit = match current.get(&t.id) {
    None => true,
    Some((k, next)) => *k != key || next.is_none(),
};  // exactly the CASE's condition (`sync.rs:157-165`), read under the same lock
let (next, err) = match (ScheduleTrigger::resolve(t), reinit, backfills.get(&t.id)) {
    (Ok(st), true, Some(b)) => rederive(conn, &st, s, b, now).await?, // below
    /* every other case: today's (next, err) */
};
// The upsert's SQL text is unchanged; only the bound `next`/`err` differ.
```
`rederive` (§3.7):
- `cur = st.first_tick_at_or_after(b.current_tick)`; `None` or `> b.window_end` → **complete**: `DELETE` the row; `next = resume_after_backfill(st, resume_from(st, b.prev, Completed{window_end}), policy, now)`; `None` → `never_fires`.
- else `left = st.ticks_through(cur, b.window_end, 10_000 - b.ticks_done as u32)`; `left.1` (more) → **P2**: `DELETE`, `next = resume_after_backfill(st, resume_from(st, b.prev, Deleted{current_tick: cur}), policy, now)`, `err = Some("the backfill ended: the new schedule gives more than 10000 ticks in its window")`, `tracing::warn!`.
- else `UPDATE … SET current_tick = cur, total_ticks = ticks_done + left.0, previous_next_evaluation_date = COALESCE($snap, previous_next_evaluation_date), updated_at = now()` with `$snap = st.first_tick_at_or_after(b.prev)` (P3); `next = Some(cur)`.

Lifecycle, after the switch rows (`sync.rs:181-200`), with the same `gone`-first rule:
```rust
if gone { DELETE FROM workflow.trigger_backfills WHERE flow_id = $1 }
else if let Some(f) = &parsed {
    let schedules: Vec<String> = /* ids of declared `Schedule` triggers */;
    DELETE FROM workflow.trigger_backfills WHERE flow_id = $1 AND NOT (trigger_id = ANY($2))
}
// unparsable: keep every row (§3.7)
```
The lifecycle `DELETE` of a removed id or a kind change does **not** resume anything (there is no schedule left; confirmed acceptable in §3.7).

- [ ] **Step 1: Write the tests (red)** in `tests/scheduler_sync.rs`, seeding through `FlowRepo` and staging backfills with Task 4's `common::stage_backfill`:
  1. **A save that keeps the cron** (a task edit) leaves the backfill row and the trigger's dates untouched.
  2. **A cron change mid-backfill** (`*/1` → `*/2`, window of 10 one-minute ticks, 3 done, `current_tick` on an odd minute): `next_evaluation_date` = `current_tick` = the first even minute at or after the old one; `total_ticks = 3 + ticks left under */2`; `previous_next_evaluation_date` re-snapped; `ticks_done` unchanged. The current tick is not lost: with `current_tick` on an even minute it is kept exactly.
  3. **A cron change that puts `current_tick` after `window_end`:** the row is deleted and the next date is the resume.
  4. **P2:** a daily window of 8 days, 0 done, cron changed to `* * * * *` → the save succeeds, the row is deleted, `last_error` is P2's sentence, the next date is the resume.
  5. **P3:** a cron changed to `0 0 30 2 *` mid-backfill → the save succeeds; the re-snap keeps the stored `previous_next_evaluation_date` (and the row is complete or kept per rule 1 — assert what the rules give).
  6. **Off the queue:** a trigger taken off the queue (`next_evaluation_date = NULL` by SQL, as `fire::take_off_queue` leaves it) with a backfill; a save → re-queued at `current_tick`.
  7. **Unparsable source keeps it:** `source = 'not: [valid'` by SQL, `scheduler::reconcile` → the `workflow.triggers` row is deleted, the backfill row stays; a fixing save → the trigger row is re-created with `next_evaluation_date = current_tick`.
  8. **Lifecycle:** a save removing the trigger id deletes its backfill (no resume); a kind change (`Schedule` `s` → `Webhook` `s`) deletes it; a soft delete deletes every backfill of the flow; another flow's backfill is untouched by all three.
  9. **`reconcile`** over a flow with a backfill and an unchanged cron changes nothing.
  10. **Concurrency** (`tests/runtime.rs`, the hold pattern of `:1403-1520`): a save with a cron change and a pause (SQL standing in for the route: `SELECT … FROM workflow.flows … FOR NO KEY UPDATE`, then the trigger row, then `UPDATE … paused`) on one flow serialise on the flow row: hold the pause's transaction after its flow lock, start the save, observe it waiting (`wait_event` `transactionid` or `tuple`), release; the end state has `paused = true` and the re-derived dates.
  11. **Mutations:** drop the backfill branch (re-initialise from now as today: 2 dies); delete backfills also when `parsed` is `None` (7 dies); drop the `gone` branch (8's soft-delete case dies); skip the cap check (4 dies with a CHECK violation).
- [ ] **Step 2: Run them, see them fail** — `cargo test -p egret-workflow --test scheduler_sync`.
- [ ] **Step 3: Implement.** The deadlock note of `scheduler/mod.rs:136-150` still holds (a rename's `FOR UPDATE` against a pass's `KEY SHARE`); the new `FOR UPDATE` read of the trigger rows takes the locks the upsert already took, in `trigger_id` order instead of declaration order. Say so in statement 4's doc.
- [ ] **Step 4: Run** `--test scheduler_sync`, `--test scheduler_fire`, `--test backfill_fire`, `--test disabled_triggers`, `--test runtime <test 10's name>`, `--test flow_trigger_sync`.
- [ ] **Step 5: Commit.** `feat(workflow): sync re-derives a schedule from its backfill and owns its lifecycle`

### Task 6: Reading backfills, `can_backfill`, the filter, `missed_ticks`, and the re-enable rule (§3.8, §3.9, D11, D12, D15) — **opus**

**Files:**
- Modify: `crates/egret-workflow/src/triggers/state.rs` (`StateRows` `:89-111` gains `backfills`; `read_state` `:119-164` gains one statement with the creator's and pauser's names, as `:124-133` reads `disabled_by_name`).
- Modify: `crates/egret-workflow/src/triggers/entry.rs` (`entry`, `:65-218`; `missed_ticks`, `:226-237`, takes the `from` the caller picks; `backfill_progress`).
- Modify: `crates/egret-workflow/src/triggers/routes.rs` (`TriggerEntryDto` `:156-191`, `ListTriggersQuery` `:373-397`, `RowFilter` `:403-470`, `build_rows` `:486-537`; OpenAPI descriptions).
- Modify: `crates/egret-workflow/src/triggers/switch.rs` (step 7, `:232-304`; `Switched` `:35-48`), `triggers/routes.rs` `switch_one`'s audit (`:835-850`).
- Modify: `crates/egret-workflow/src/routes.rs` `components(..)` (`:3546-3564`), `openapi.json`, the client.
- Test: a new `crates/egret-workflow/tests/backfill_routes.rs` (reads; Task 7 adds the writes), unit tests in `triggers/entry.rs`, `tests/triggers_routes.rs` for the re-enable cases (beside W7e's re-enable tests).

**Interfaces — Produces:**
```rust
// triggers/state.rs
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub(crate) struct BackfillStateRow {
    pub flow_id: Uuid, pub trigger_id: String, pub id: Uuid,
    pub window_start: DateTime<Utc>, pub window_end: DateTime<Utc>, pub current_tick: DateTime<Utc>,
    pub paused: bool, pub paused_by_name: Option<String>, pub paused_at: Option<DateTime<Utc>>,
    pub inputs: serde_json::Value, pub labels: serde_json::Value,
    pub created_by_name: Option<String>, pub created_at: DateTime<Utc>,
    pub previous_next_evaluation_date: DateTime<Utc>, pub total_ticks: i32, pub ticks_done: i32,
}
// StateRows gains: pub backfills: HashMap<Key, BackfillStateRow>

// triggers/routes.rs
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
pub struct BackfillDto {
    pub id: Uuid, pub start: DateTime<Utc>, pub end: DateTime<Utc>, pub current_tick: DateTime<Utc>,
    pub paused: bool, pub paused_by_name: Option<String>, pub paused_at: Option<DateTime<Utc>>,
    #[schema(value_type = Object)] pub inputs: serde_json::Value,
    pub labels: Vec<crate::routes::LabelDto>,
    pub created_by_name: Option<String>, pub created_at: DateTime<Utc>,
    pub previous_next_evaluation_date: DateTime<Utc>,
    pub total_ticks: i32, pub ticks_done: i32,
    /// 0-100: Kestra's `(current_tick − start) / (end − start)`, rounded, clamped; 100 for an empty span.
    pub progress: u8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CannotBackfill { FlowDeleted, NotSchedule, Forbidden, Disabled, OffQueue, Running }
// TriggerEntryDto gains:
pub backfill: Option<BackfillDto>,
/// Whether the Backfill button is usable; the create route checks for itself.
pub can_backfill: bool,
pub backfill_refusal: Option<CannotBackfill>,
// ListTriggersQuery gains: pub backfill: Option<String>  // running | paused | any; else 400

// triggers/entry.rs
pub(crate) fn backfill_progress(start: DateTime<Utc>, end: DateTime<Utc>, current: DateTime<Utc>) -> u8;

// triggers/switch.rs, Switched gains:
/// A re-enable during a backfill moved the regular schedule's resume position here (§3.8).
pub resume_moved_to: Option<DateTime<Utc>>,
```
`CannotBackfill` is the read's enum; Task 7's `BackfillRefusal` is the write's. Rules, first match wins: `flow.deleted` → `FlowDeleted`; not `TriggerKind::Schedule` → `NotSchedule`; `!may_manage` → `Forbidden`; `effective_disabled` → `Disabled`; no schedule row or `next_evaluation_date` NULL → `OffQueue`; a backfill row → `Running`. `backfill` is `None` for a non-schedule. `missed_ticks` (D12): with a backfill, `missed_ticks(t, Some(b.previous_next_evaluation_date), db_now)`; without, as today. Both `entry` (`Detail::Full`) and `build_rows` (`:516-518`) apply the rule.

The re-enable rule (§3.8), in step 7 of `set_disabled`, after the trigger row: read the backfill row `FOR UPDATE` (Task 4's `read_for_update`). With a row:
- `next_evaluation_date` is **kept** for every choice (`moved_to: None`);
- "skip" (`recover_missed: false`) or `RecoverMissed::None`: `UPDATE workflow.trigger_backfills SET previous_next_evaluation_date = $3, updated_at = now()` with `$3 = TriggerState::initialise(&st, "", "", &FixedClock(db_now)).next_evaluation_date` when `Some` (none: leave it, P3); `resume_moved_to` = that date;
- `ALL`/`LAST`: nothing written; `RecoverUnknown` is checked as today (`:199-207`).
The audit event's JSON gains `"backfill_resume_from": resume_moved_to`.

- [ ] **Step 1: Write the tests (red).**
  1. `backfill_progress` (unit): `start = end` → 100; current at start → 0; midway → 50; `current > end` → 100; rounding at 1/3 → 33.
  2. **Entry fields** (route, `tests/backfill_routes.rs`, staged with `common::stage_backfill` and a paused variant with `paused_by` set): every `BackfillDto` field; `paused_by_name` and `created_by_name` resolve to the users' names; `progress`; the counts.
  3. **`can_backfill`/`backfill_refusal`, each cause alone, in precedence:** trashed flow (`flow_deleted`); a `Webhook` (`not_schedule`); `workflow:read` only (`forbidden`); YAML `disabled: true`, a switch row, flow `state = 'disabled'` (each `disabled`); `next_evaluation_date` NULL (`off_queue`); a staged backfill (`running`); none → `can_backfill: true`. A row that has two causes reports the first.
  4. **`missed_ticks` during a backfill** counts from `previous_next_evaluation_date`: future → `null`; three regular ticks back → `{count: 3}`, though `next_evaluation_date` is a window tick hours back.
  5. **The list:** rows carry `backfill`; `backfill=running` keeps a running one, not a paused one, not one without; `paused` the reverse; `any` both; `backfill=bogus` → 400; `missed_ticks` per row follows test 4.
  6. **Re-enable during a backfill** (`tests/triggers_routes.rs`): switch off, then on: with `recover_missed: false` → `next_evaluation_date` unchanged (= `current_tick`), `previous_next_evaluation_date` = the first tick after now, the answer's entry shows it, the audit event carries `backfill_resume_from`; with `true` and `ALL` or `LAST` → both dates unchanged; `NONE` → as `false`; `"SOME"` → 409 `recover_missed_unknown`, nothing written. Then one `fire_due` pass fires `current_tick` (the backfill resumed where it stopped; no window tick lost).
  7. **Mutations:** keep W7e's skip for a backfill (move `next_evaluation_date`; 6 dies); count `missed_ticks` from `next_evaluation_date` during a backfill (4 dies); put `Running` before `Disabled` (3's two-cause case dies).
- [ ] **Step 2: Run them, see them fail.**
- [ ] **Step 3: Implement.** `StateRows::empty` (`:101-110`) gains the empty map; `entry.rs`'s unit fixtures follow.
- [ ] **Step 4: Regenerate** `openapi.json` and the client; `npx tsc -b`.
- [ ] **Step 5: Run** `--test backfill_routes`, `--test triggers_routes`, `--lib triggers`, `cargo test -p egret-server --test feature_route_coverage`.
- [ ] **Step 6: Commit.** `feat(workflow): trigger entries carry their backfill; re-enabling keeps a backfill's place`

### Task 7: The backfill routes — create, pause/resume, delete, bulk, audit (§3.4, §3.5, §3.10, §3.11, §5) — **opus**

**Files:**
- Create: `crates/egret-workflow/src/triggers/backfill.rs` (the three transactions; no HTTP), `triggers/backfill_routes.rs` (DTOs, five handlers, codes, `refusal_parts`); `triggers/mod.rs` (two `pub(crate) mod`s; the module doc names them).
- Modify: `crates/egret-workflow/src/routes.rs`: the router beside W7e's (`:3330-3350`); each route that takes a body with `DefaultBodyLimit::max(MAX_BACKFILL_BODY_BYTES)` (as `:3343-3349`); the module doc's route list (`:16-19`); `paths(..)` (`:3496-3499`) and `components(..)`.
- Modify: `openapi.json`, the client.
- Test: `crates/egret-workflow/tests/backfill_routes.rs`, `tests/runtime.rs` (concurrency).

**Interfaces:**
- Consumes: Task 1's `first_tick_at_or_after`, `ticks_through`, `resume_from`, `resume_after_backfill`, `labels::backfill_labels_refusal`, `fire::overlay_inputs`/`refuse_templated` (make both reachable as `crate::scheduler::{overlay_inputs, refuse_templated}`); Task 4's `BackfillRow`, `read_for_update`; Task 6's entry fields; W7e's `entries_for_flow`, `read_state`, `last_executions`, `TriggerRouteError`, `BulkTriggerRef`, `BulkResultDto`, `BulkResultsDto`, `body_refusal`, `MAX_BULK_TRIGGERS`, `TOO_MANY_TRIGGERS`.
- Produces:
```rust
// triggers/backfill_routes.rs
pub const BACKFILL_WINDOW_INVALID: &str = "backfill_window_invalid";
pub const BACKFILL_END_IN_FUTURE: &str = "backfill_end_in_future";
pub const BACKFILL_START_TOO_EARLY: &str = "backfill_start_too_early";
pub const BACKFILL_NO_TICKS: &str = "backfill_no_ticks";
pub const BACKFILL_TOO_MANY_TICKS: &str = "backfill_too_many_ticks";
pub const BACKFILL_INPUTS_INVALID: &str = "backfill_inputs_invalid";
pub const BACKFILL_LABELS_INVALID: &str = "backfill_labels_invalid";
pub const NO_BACKFILL: &str = "no_backfill";
pub const BACKFILL_NOT_SUPPORTED: &str = "backfill_not_supported";
pub const TRIGGER_DISABLED: &str = "trigger_disabled";
pub const TRIGGER_OFF_QUEUE: &str = "trigger_off_queue";
pub const BACKFILL_EXISTS: &str = "backfill_exists";
pub const MAX_BACKFILL_TICKS: u32 = 10_000;
pub const MAX_BACKFILL_BODY_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateBackfillRequest {
    pub start: DateTime<Utc>,
    #[serde(default)] pub end: Option<DateTime<Utc>>,           // absent: the transaction's now()
    #[serde(default)] #[schema(value_type = Option<Object>)] pub inputs: Option<serde_json::Value>,
    #[serde(default, deserialize_with = "crate::models::deserialize_labels")]
    #[schema(value_type = Vec<crate::routes::LabelDto>)] pub labels: Vec<crate::models::Label>,
}
#[derive(Debug, Deserialize, utoipa::ToSchema)] pub struct SetBackfillPausedRequest { pub paused: bool }
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct SetBackfillsPausedRequest { pub triggers: Vec<BulkTriggerRef>, pub paused: bool }
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DeleteBackfillsRequest { pub triggers: Vec<BulkTriggerRef> }

// handlers (operation ids → orval hooks useWorkflowCreateTriggerBackfill, …SetTriggerBackfillPaused,
// …DeleteTriggerBackfill, …SetTriggersBackfillPaused, …DeleteTriggersBackfill)
pub(crate) async fn create_trigger_backfill(..) -> Result<(StatusCode, Json<TriggerEntryDto>), TriggerRouteError>;
pub(crate) async fn set_trigger_backfill_paused(..) -> Result<Json<TriggerEntryDto>, TriggerRouteError>;
pub(crate) async fn delete_trigger_backfill(..) -> Result<Json<TriggerEntryDto>, TriggerRouteError>;
pub(crate) async fn set_triggers_backfill_paused(..) -> Result<Json<BulkResultsDto>, TriggerRouteError>;
pub(crate) async fn delete_triggers_backfill(..) -> Result<Json<BulkResultsDto>, TriggerRouteError>;

// triggers/backfill.rs — each in the caller's transaction; the caller commits or rolls back
pub(crate) enum BackfillRefusal {
    NotFound, Unparsable(String), NotSupported, TriggerDisabled, OffQueue(Option<String>),
    WindowInvalid, EndInFuture { now: DateTime<Utc> }, StartTooEarly, NoTicks, TooManyTicks,
    InputsInvalid(String), LabelsInvalid(String), Exists, NoBackfill,
}
pub(crate) struct Created { pub flow: FlowRow, pub parsed: Flow, pub backfill: BackfillRow }
pub(crate) async fn create(conn: &mut PgConnection, ws: Uuid, flow: Uuid, trigger_id: &str,
    req: &CreateBackfillRequest, by: Uuid) -> Result<std::result::Result<Created, BackfillRefusal>>;
pub(crate) struct Changed { pub flow: FlowRow, pub parsed: Flow, pub changed: bool,
    pub resumed_at: Option<DateTime<Utc>> /* delete only */ }
pub(crate) async fn set_paused(conn: &mut PgConnection, ws: Uuid, flow: Uuid, trigger_id: &str,
    paused: bool, by: Uuid) -> Result<std::result::Result<Changed, BackfillRefusal>>;
pub(crate) async fn delete(conn: &mut PgConnection, ws: Uuid, flow: Uuid, trigger_id: &str)
    -> Result<std::result::Result<Changed, BackfillRefusal>>;
```

**`create`, in W7e's lock order** (§3.4; `switch.rs:103-139` is the template for steps 1–2):
1. The flow row `FOR NO KEY UPDATE` with the workspace predicate; none or trashed → `NotFound`.
2. `spec::parse` → `Unparsable`; the first trigger with this id → none is `NotFound`; not a `Schedule` → `NotSupported`.
3. Effective-disabled (`scheduler::effective_disabled(parsed.disabled, t.disabled, a disabled_triggers row exists)` OR `flow.state == "disabled"`, as `entry.rs:83-86`) → `TriggerDisabled`.
4. `SELECT next_evaluation_date, last_error FROM workflow.triggers … FOR UPDATE`; none, or NULL date → `OffQueue(last_error)`. `ScheduleTrigger::resolve(t)` failing → `OffQueue(Some(e.to_string()))`.
5. `db_now` (`SELECT now()`); `end = req.end.unwrap_or(db_now)`. In order: `start >= end` → `WindowInvalid`; `end > db_now` → `EndInFuture`; `start < db_now - 120 months` (`checked_sub_months(Months::new(120))`) → `StartTooEarly`; `(n, more) = st.ticks_through(start, end, MAX_BACKFILL_TICKS)`: `more` → `TooManyTicks`, `n == 0` → `NoTicks`. Inputs: `None`/`null` → `{}`; not an object → `InputsInvalid("inputs must be a JSON object")`; a key naming a `FILE` input → `InputsInvalid("{id} is a FILE input: a backfill has no upload")`; the merged map (`overlay_inputs`) through `refuse_templated` → `InputsInvalid(its sentence)`; each declared input with `required` and `defaults.is_none()` and no merged value → `InputsInvalid("{id} is required")`. Labels: `backfill_labels_refusal` → `LabelsInvalid`.
6. `INSERT INTO workflow.trigger_backfills (flow_id, trigger_id, workspace_id, window_start, window_end, current_tick, inputs, labels, previous_next_evaluation_date, total_ticks, created_by) VALUES (…) ON CONFLICT (flow_id, trigger_id) DO NOTHING RETURNING <BACKFILL_COLS>`; no row → `Exists`. `current_tick = st.first_tick_at_or_after(start)` (step 5 proved it exists and is `<= end`); `previous_next_evaluation_date` = step 4's date; `labels` stored as `[{key, value}]` in the given order.
7. `UPDATE workflow.triggers SET next_evaluation_date = $current_tick, updated_at = now()` — `evaluated_at` and `last_error` untouched (§3.4 step 7).

**`set_paused`**: steps 1–2 (a disabled trigger is **not** refused); the trigger row `FOR UPDATE` (absent is fine); `read_for_update` → none is `NoBackfill`; already in the state → `changed: false`, no write; else `UPDATE … SET paused = $3, paused_by = CASE WHEN $3 THEN $4 END, paused_at = CASE WHEN $3 THEN now() END, updated_at = now()`. Nothing else (D5, D7).

**`delete`**: as `set_paused` to the backfill row; `DELETE` it; when the trigger row exists and its `next_evaluation_date` is not NULL: `resume_after_backfill(st, resume_from(st, prev, Deleted{current_tick}), policy, db_now)` written to `next_evaluation_date` (`None` → NULL + `never_fires`, as `switch.rs:282-300`); an off-queue row stays off (the next save re-queues it, §3.7); a disabled trigger's dates are written too (it is frozen; the dates are what it resumes from).

Handlers: `require(WF_MANAGE)` first, then the body (`body_refusal`), then one transaction per trigger: the transaction above, the entry read in that transaction (as `switch_one`, `triggers/routes.rs:778-853`), commit, audit (`mutation_for(.., "workflow_trigger:backfill_create" | …, "workflow_trigger", Some(format!("{flow}/{trigger_id}")), json!(..))`; create: `{start, end, total_ticks, input_keys, label_keys}` — keys, never values; delete: `{resumed_at}`); only on a real change. Bulk: more than 100 → 400 `too_many_triggers` before any write; a refusal is `{ok: false, code, message}` and the next entry runs; a foreign flow is `not_found`. Messages: write each so it says what to do (FE-COPY-2 applies to what the UI shows verbatim): e.g. `EndInFuture` → "The end {end} is after the server's time {now}; pick an end in the past or leave it empty"; `Exists` → "This trigger already has a backfill; pause or delete it first"; `NoBackfill` → "This trigger has no backfill"; `TriggerDisabled` → "Enable the trigger to backfill it"; `OffQueue(e)` → "The schedule is off the queue: {e}; fix the flow and save it".

- [ ] **Step 1: Write the tests (red)** in `tests/backfill_routes.rs` (`common::app_with_trigger_flags`, `seed_workspace`, `grant`, `seed_flow`, `post`, `put`, `delete`, `json`, `audit_mutations`):
  1. **Create, the happy path:** a `*/1` schedule; `POST` a 5-tick window ending a minute ago with inputs and labels in **map** form (`{"team": "ops"}`) → 201, the entry's `backfill` (`total_ticks: 5`, `ticks_done: 0`, `current_tick` = first tick), the trigger row's `next_evaluation_date` = first tick and the backfill's `previous_next_evaluation_date` = the old date; one audit event with keys and no values. Then five `fire_due` passes fire five executions (the route and Task 4 agree).
  2. **Inclusive start:** `start` exactly on a tick → that tick is `current_tick`.
  3. **Every §5 refusal with its status and code:** `start >= end`; `end` in the future (the message carries the server's time); `start` 121 months back; a window with no tick (`0 2 * * *`, `[03:00, 04:00]`); 10,001 ticks (`* * * * * *` with `withSeconds: true` over 10,001 s) and exactly 10,000 accepted; `inputs: []`; a `FILE` input named; a templated schedule input not overridden, and the same overridden → accepted; a required input missing, and present → accepted; each label rule (`system.x`, empty value, `{{`, 51 labels); a second create → 409 `backfill_exists`; a `Webhook` → 409 `backfill_not_supported`; YAML `disabled`, a switch row, flow `state = 'disabled'` → 409 `trigger_disabled`; `next_evaluation_date` NULL → 409 `trigger_off_queue` with `last_error` in the message; unknown id, another workspace's flow, trashed flow → 404 `not_found`; unparsable (SQL) → 400; `workflow:read` → 403; no session → 401; bad JSON → 400 `invalid_body`; 65 KiB → 413. After each refusal, nothing was written (no backfill row; trigger row byte-identical).
  4. **Pause/resume:** pause → 200, `paused: true`, `paused_by_name`; the trigger row and `previous_next_evaluation_date` unchanged (D5); pause again → 200, no second audit event; resume → the next pass fires `current_tick`; no backfill → 404 `no_backfill`; a switched-off trigger's backfill can be paused (not refused).
  5. **Delete:** with a future `previous_next_evaluation_date` → the next date is it; **Kestra's delete bug** (§6): pause, resume, delete → the regular schedule resumes from the stored position, never from a window date; with `NONE` and a past position → first tick after now; no backfill → 404; executions already fired stay.
  6. **Bulk:** 100 entries accepted; 101 → 400 `too_many_triggers` and nothing written; a mix (one paused ok, one `no_backfill`, a foreign flow) → three results in order, codes `no_backfill` and `not_found`; bulk delete the same.
  7. **Concurrency** (`tests/runtime.rs`, the `w12_hold` pattern of `:1403-1520`): hold a pass of a backfill at its final `UPDATE workflow.triggers`; send `DELETE …/backfill`; observe it waiting on a row lock; release; the pass's fire committed first, the delete answered after, and a second `fire_due` fires nothing from the window (the row is gone; the next date is the resume). And: two concurrent creates on one trigger → one 201, one 409 `backfill_exists`, one row.
  8. **Mutations:** drop `ON CONFLICT … DO NOTHING`'s `Exists` check (7's second create gets a 500 instead of a 409, or two rows — record which); write `previous_next_evaluation_date` on pause (5's Kestra-bug case dies); compute `current_tick` with `first_tick_after` (2 dies); drop the flow lock (`FOR NO KEY UPDATE`) — record what 7 does.
- [ ] **Step 2: Run them, see them fail.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Regenerate** `openapi.json` and the client; `npx tsc -b`.
- [ ] **Step 5: Run** `--test backfill_routes`, `--test runtime <the new names>`, `cargo test -p egret-server --test feature_route_coverage`.
- [ ] **Step 6: Commit.** `feat(workflow): create, pause, resume and delete a schedule backfill, one or in bulk`

### Task 8: The Triggers tab and page — column, strip, drawer, bulk, facet; the overview (§4.1, §4.2, §4.4) — **opus**

**Files:**
- Create in `frontend/src/components/workflows/triggers/`: `BackfillStrip.tsx` (the sub-row), `useBackfill.ts` (pause/resume/delete, single and bulk; invalidation as `useTriggerSwitch.ts:64-76` does).
- Modify: `TriggersTable.tsx` (a Backfill column after Enabled, `:147`; the strip as an extra `TableRow` under a row with a backfill; new props `onBackfill`, `backfillActions`), `triggersModel.ts` (`statusText` `:43-62`; `backfillStatus`, `backfillCell`, `backfillRefusalText`, `backfillWindowText`, `ticksText`; `REFUSAL` copy), `TriggerDetailsDrawer.tsx` (a "Backfill" section under Schedule, §4.2), `EnableTriggerDialog.tsx` (§4.1's extra line), `FlowTriggersTab.tsx` (`refetchInterval`: 5 s while a backfill is running and not paused, else `TRIGGERS_POLL_MS`, `:27, :59`).
- Modify: `frontend/src/routes/workflows/TriggersList.tsx` (bulk "Pause backfills", "Resume backfills", "Delete backfills" beside `:331-350`; a `backfill` facet in the URL; per-entry results as W7e's, `no_backfill` reads "had no backfill"), `routes/workflows/ExecutionOverview.tsx` (`effective_labels` as `key: value` chips; "Backfill" beside the trigger id in `TriggerSection` when `backfill_id` is set, `:143-160`).
- Modify: `frontend/scripts/assert-workflows-lazy.mjs` (markers for `BackfillStrip.tsx` and `useBackfill.ts`, guarded against the entry only, like `TriggersTable.tsx`, `:256-259`).
- Test: `components/workflows/triggers/__tests__/{triggersModel,FlowTriggersTab,TriggerDetailsDrawer,EnableTriggerDialog}.test.ts(x)`, a new `BackfillStrip.test.tsx`; `routes/workflows/__tests__/TriggersList.test.tsx`, `ExecutionDetail.test.tsx` (the overview); `routes/workflows/__tests__/fakeWorkflowServer.ts` and `components/workflows/triggers/__tests__/triggerFixtures.ts` (the five routes, recording each body; fixtures with a running, a paused and a frozen backfill).

**Interfaces:**
- Consumes: the hooks Task 7 generated — read `@/api/generated/workflow/workflow` for the exact names (orval derives them from the handler names; expected `useWorkflowSetTriggerBackfillPaused`, `useWorkflowDeleteTriggerBackfill`, `useWorkflowSetTriggersBackfillPaused`, `useWorkflowDeleteTriggersBackfill`); `BackfillDto`, `CannotBackfill` from `@/api/generated/model`; `ReasonButton`/`DisabledReasons`, `ui/progress`, `ui/confirm-delete-dialog` (or `ui/alert-dialog`), `StateChip`, `RelativeTime`, `FacetSelect`.
- Produces for Task 9: the column's `onBackfill(row)` callback that Task 9 binds to the dialog; `backfillRefusalText`; `useBackfill`.

**Behaviour, per §4:**
- The column: a `Schedule` with none → "Backfill" (`ReasonButton`, disabled with `backfillRefusalText(backfill_refusal)`: `not_schedule` "Only schedules can be backfilled", `forbidden` "You need workflow:manage", `disabled` "Enable the trigger to backfill it", `off_queue` "Off the queue: {last_error}", `flow_deleted` "The flow is in the trash", `running` "This trigger already has a backfill"); running → "Running" + icon; paused → "Paused" + another icon (FE-A11Y-4); other kinds empty.
- The strip: "{start} → {end}" (the viewer's time), "{progress}% · {ticks_done} of {total_ticks} ticks", `ui/progress` (no animation, FE-A11Y-5; paused uses the muted token **and** the word "Paused"), Pause/Resume, Delete; Delete opens a confirm (FE-ACTION-1): "Delete this backfill? Its remaining {total_ticks − ticks_done} ticks will not run. Executions already started stay."; an effective-disabled trigger's strip says "Frozen while the trigger is off" and keeps Pause and Delete.
- Status (`statusText`): after the YAML, switch, flow and off-queue causes, "Backfilling ({progress}%)" or "Backfill paused ({progress}%)".
- The enable dialog with a backfill: "The backfill resumes at {current_tick}. This choice is about the regular runs that fell due."
- The drawer's section: window in the trigger's timezone and the viewer's, current tick, progress, created by/when, paused by/when, inputs (a key/value table), labels (chips), "Regular runs resume from {previous_next_evaluation_date}" and what `recoverMissedSchedules` does with the waiting ones, reusing `recoverDescription` (`triggersModel.ts:108`) and `missedLabel` (`:116`).
- Actions show the server's state only (the strip moves when the answer arrives); a refusal shows the server's message; success invalidates `workflowsKeys.flowTriggers(id)` and `workflowsKeys.triggers(ws)`; the bulk toasts (FE-ACTION-2).
- Global page: `backfill` facet values Running / Paused / Any, mapped to `backfill=running|paused|any`, resetting `page` (FE-PAGE-2); the three bulk buttons with `ReasonButton` reasons; Delete backfills confirms first.
- Read-only viewer: every action disabled with "You need workflow:manage".

- [ ] **Step 1: Write the tests (red).**
  1. `triggersModel`: `statusText` precedence with the two new statuses; `backfillRefusalText` for each value; `ticksText`; the window text in a fixed timezone.
  2. `FlowTriggersTab` against the fake server: the column for a schedule without, with a running and with a paused backfill, and for a webhook; the disabled Backfill button's reason per refusal; the strip's text and progress value (`aria-valuenow`); Pause sends `PUT …/backfill/paused` `{paused: true}` and the strip changes only after the answer; Delete opens the confirm, Cancel sends nothing, Confirm sends `DELETE`; a 404 `no_backfill` shows the server's message; the 5 s poll while running (fake timers: a refetch at 5 s), 30 s when paused or none; the frozen strip.
  3. `TriggerDetailsDrawer`: the section's fields; the resume sentence per policy.
  4. `EnableTriggerDialog`: the extra line only with a backfill.
  5. `TriggersList`: the facet in the URL and the request; the three bulk buttons send one bulk request each with the selection's pairs; per-entry refusals by name ("had no backfill").
  6. The overview (`ExecutionDetail.test.tsx`): `effective_labels` chips; "Backfill" beside the trigger id iff `backfill_id`.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Markers,** each proven with a scratch static import from `App.tsx` that makes `npm run test:bundle:workflows` fail naming it; revert the scratch import. Record the proofs in the report.
- [ ] **Step 4: Frontend gates for the touched files, `npm run build`, `npm run test:bundle:workflows`, then commit.** `feat(workflows): backfill column, progress strip, drawer section and bulk actions`

### Task 9: The create form — `BackfillDialog` and `LabelsEditor`, lazy (§4.3, D16, P6) — **sonnet**

**Files:**
- Create: `frontend/src/components/workflows/triggers/BackfillDialog.tsx` (default export, `ui/dialog`), `LabelsEditor.tsx`, `backfillForm.ts` (pure: `bodyFromForm`, `submitReasons`, `fieldOfError`); tests `__tests__/BackfillDialog.test.tsx`, `__tests__/LabelsEditor.test.tsx`, `__tests__/backfillForm.test.ts`.
- Modify: `FlowTriggersTab.tsx` and `routes/workflows/TriggersList.tsx` — `const BackfillDialog = lazy(() => import("@/components/workflows/triggers/BackfillDialog"))` in each, inside `<Suspense>`, bound to Task 8's `onBackfill`; the global page reads the flow's source with the existing flow query (`GET /flows/{id}`) when the dialog opens.
- Modify: `frontend/scripts/assert-workflows-lazy.mjs`: a `backfill` flag (P6) checked like `files` against `FLOW_PAGE_HOSTS` and against the list chunk (`:361-372`, `:383-392`), counted in the summary line; markers for `BackfillDialog.tsx`, `LabelsEditor.tsx` and `backfillForm.ts` flagged `backfill`.

**Interfaces — Consumes:** `inputsFromSource`, `formFromInputs`, `validateForm`, `isEmpty`, `serverErrorInput` (`run/inputModel.ts:98, 207, 212, 266, 388`), `InputField` (`run/inputControls.tsx:33`), the generated `useWorkflowCreateTriggerBackfill`, Task 8's `useBackfill` invalidation. **Produces:** `BackfillDialog({ ws, flowUuid, trigger: TriggerRow, source: string | null, open, onOpenChange })`.

**Behaviour, per §4.3:**
- Title "Backfill {trigger id}"; the description "Runs this schedule for each of its ticks from start to end, both included, one about every second. Regular runs wait until it ends."
- Start, End: `<Input type="datetime-local" step="1" max={nowLocal}>` with tied `<Label>`s (FE-FORM-2); `max` re-read on open and on each change; End may be empty ("Leave empty to end now"); local values are converted to UTC instants in the body; below: "In {trigger timezone}: {start} → {end}" (`Intl.DateTimeFormat` with `timeZone`).
- Inputs: from `inputsFromSource(source)`, `FILE` inputs not offered; a required `FILE` input disables submit with "A FILE input cannot be backfilled"; prefilled with the schedule's plain `inputs:` values (parse the trigger's YAML block from `source`; a templated value is left empty) over the flow defaults.
- `LabelsEditor`: rows of key/value with Add and Remove; per-row errors beside the field (FE-FORM-4): "Keys starting with system. are reserved", "Fill both the key and the value"; fully empty rows dropped from the body.
- Submit "Start backfill", a `ReasonButton` disabled with reasons until: a start; `start < end` (or no end); neither in the future; required inputs filled; no half-filled label. Hand-written form state (FE-FORM-1).
- Errors: 400 `backfill_start_too_early` → beside Start; `backfill_end_in_future`, `backfill_window_invalid` → beside End; `backfill_no_ticks`, `backfill_too_many_ticks` → above the buttons with what to do; `backfill_inputs_invalid` → beside the input `serverErrorInput` names; `backfill_labels_invalid` → beside Labels; 409 `backfill_exists` → refetch, close, the server's sentence as a toast; nothing typed is cleared (FE-ACTION-3).

- [ ] **Step 1: Write the tests (red).** `backfillForm`: `bodyFromForm` gives UTC instants from local values (a fixed `TZ` in the test), omits `end` when empty, drops empty label rows; `submitReasons` for each blocker; `fieldOfError` per code. `LabelsEditor`: add, remove, the two errors. `BackfillDialog` against the fake server: the body sent (start, end, inputs, labels); the field-level errors per code; 409 closes and refetches; Cancel sends nothing; a required FILE input blocks with its reason.
- [ ] **Step 2: Implement.**
- [ ] **Step 3: Bundle.** Add the `backfill` flag and the markers; prove each marker twice — a scratch static import from `App.tsx` (fails as "in the entry chunk") and one from `FlowTriggersTab.tsx` (fails as "loads with a flow page") — then revert. Record the four failures in the report.
- [ ] **Step 4: Frontend gates for the touched files, `npm run build`, `npm run test:bundle:workflows`, then commit.** `feat(workflows): the backfill form, with inputs and labels, in its own lazy chunk`

### Task 10: End to end, the Kestra comparison, the spec (§6 Playwright, §7) — **opus**

**Files:**
- Create: `frontend/tests/e2e/workflows-backfill.spec.ts`.
- Modify: the spec, EN (the status line; §2 rows this task measures relabelled **observed**; any new difference added; the planning deviations P1–P7 recorded in §2 or a new "Decided while planning" note; §3.3's `source_of` → `load_source`; D17 → 0191). VI only if its owner hands it over; never overwrite their uncommitted work. W13 spec §5.3 step 4's divergence narrows (§3.3), if Task 3 did not do it.

- [ ] **Step 1: A private stack** (W7e plan Task 8's recipe): a fresh database `egret_w7f_e2e` in `egret-w7f-postgres` (:5449); the backend built from this worktree with `EGRET_DATABASE__URL=postgres://egret:egret@localhost:5449/egret_w7f_e2e`, `EGRET_WORKFLOW__SCHEDULER_ENABLED=true`, `EGRET_WORKFLOW__WORKER_ENABLED=true`, local admin login (`loginLocalAdmin`); check `lsof -i :8080` before using the Vite proxy's port; extra containers prefixed `w7f-`, removed afterwards. Never the shared dev stack.
- [ ] **Step 2: Run §6's scenarios 1–7** with real assertions on the DOM or the database, waits bounded by the schedule, light and dark mode for scenario 1.
- [ ] **Step 3: Measure the DOM** at 1280 px and 390 px: the column, the strip, the dialog, the drawer section (FE-VISUAL-4, FE-TEST-2).
- [ ] **Step 4: The Kestra comparison** on the local image `kestra/kestra` id `28f59bf175e1` (untagged; `docker run` it by id, `server local`, a throwaway container `w7f-kestra`, removed afterwards): §7's list — exclusive start, a re-run over executed dates, a backfill label overriding a flow label, inputs overriding the schedule's, pause/resume/delete (which date is restored, whether regular ticks replay), natural completion with a regular tick inside the backfill (skipped), the API refusals Kestra lacks, bulk (202), the progress formula. Relabel each measured §2 row **observed**; add any difference found (FE-REF-2).
- [ ] **Step 5: Commit** the e2e spec and the EN spec (and the VI spec if owned). `test(workflows): end-to-end backfill; docs: W7f divergences observed against Kestra`. **Do not run the full gate**: report to the controller, which runs it once.

---

## Self-review (done while writing)

- **Spec coverage:** §3.1 → Task 2; §3.2 → Tasks 2 (indexes, `classify`), 4 (the fire); §3.3 → Tasks 1 (merge), 2 (column, `NewExecution`), 3 (render, source, copies, DTOs); §3.4 → Tasks 1 (inclusive start, overlay), 7 (route); §3.5 → Tasks 4 (claim term), 7 (routes); §3.6 → Tasks 1 (resume), 4; §3.7 → Task 5; §3.8 → Task 6; §3.9 → Task 6; §3.10 → Task 7; §3.11 → Tasks 5, 7 (concurrency tests), 4 (P5); §3.12 → Tasks 1 (`lib.rs`, `TriggerState`), 2 (`fire.rs:261-264`, `scheduler/mod.rs` statement 2), 10 (the W7e spec note); §4.1, §4.2, §4.4 → Task 8; §4.3 → Task 9; §5 → Task 7; §6 → every task's tests and Task 10; §7 → Task 10; §8 → Global Constraints; §9 → nothing built; §10 → nothing.
- **Deviations from the spec, each stated where it is used:** migration `0191` (0190 is held, uncommitted, in `egret-tagdef`); P1 (list labels own, detail merged); P2 (over-cap recount ends the backfill); P3 (re-snap with no tick keeps the value); P4 (`LEAST`); P5 (a test-only hook for the fresh read); P6 (a `backfill` bundle flag); P7 (`deserialize_labels` lifted). §3.3's `source_of` is `load_source`; §3.3's construction list omits ten test sites.
- **Type consistency:** `BackfillRow` (scheduler, Task 4) and `BackfillStateRow` (reads, Task 6) are different on purpose: the second carries the two display names. `CannotBackfill` (read) vs `BackfillRefusal` (write) likewise. `LabelDto` is defined once in `routes.rs` (Task 3) and reused by `BackfillDto` and `CreateBackfillRequest`'s schema.
