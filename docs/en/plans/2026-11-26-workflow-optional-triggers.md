# Workflow optional trigger types (package 11e) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Lift the two trigger types egret refuses with "package #11 lifts ScheduleOnDates and http.Trigger" — `ScheduleOnDates` (fire on an explicit date list) and `http.Trigger` (poll an HTTP endpoint on an interval and fire when a condition holds).

**Architecture:** Two new `TriggerKind` variants. ScheduleOnDates is Schedulable → reuse the existing scheduler (`workflow.triggers` + `fire.rs`), computing next-date from the date list. http.Trigger is a polling trigger → a new fire path: when its interval tick is due, claim the row, commit (release the lock), perform the HTTP request OUTSIDE any transaction via `egret-egress`, render `responseCondition`, then fire (binding the response) or advance, in a fresh transaction. Behaviour is built with the publish refusal KEPT for both types (tested via directly-parsed fixtures); the reconcile task lifts the refusal + moves the census in ONE commit.

**Tech Stack:** Rust, axum, sqlx/Postgres, `egret-egress`. Crate `crates/egret-workflow`.

## Global Constraints

- **Measurement-first:** no type is honoured until Task 0 pins its Kestra semantics on the local pinned source `/Users/1pro-l4r8/Documents/github/data-portal/kestra` @ `96893b589`.
- **D1** 11e = both types, one package. **D2** http.Trigger poll = claim→commit→poll-outside-lock→fire, reusing the scheduler + `workflow.triggers`; NO separate polling worker. **D3** honour `failOnTriggerError` + `allowConcurrent` for http.Trigger (it is a polling trigger). **D4** migration-free (Task 0 confirms; fallback column at the next free number). **D5** http.Trigger egress reuses `egret-egress` + `workflow.http.allow_hosts`; `secret()` resolves on request fields (#10a). **D6** realtime/fs/plugin triggers stay refused (#18).
- **LIFT-REFUSAL DISCIPLINE (load-bearing — bitten 9a×2/9b×1/and in 11d):** the publish/validate refusal of the two new types stays active until the reconcile task (Task 4), lifted in the SAME commit the fire behaviour is correct and the census is regenerated. Tasks 1–3 build behaviour tested via directly-parsed `Flow` fixtures (serde), NOT via publish. Before lifting, grep code + message + fixture YAML + any test asserting these types are refused.
- **Census:** `property-treatment.json` is probe-generated + golden-tested; it must NOT move in Tasks 1–3; it moves once, in Task 4. Baseline after 11d: **1228 honoured / 3 warned / 354 ignored / 117 refused / 1702 total**.
- **Scoped runs** include `--test routes --test execution_timestamps --test scheduler_fire --test scheduler_sync`; tick-driving tests hold the file-local serialising mutex (worker tick claims globally).
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; Conventional Commits; `Closes #N` in PR body only. `cargo fmt --all --check` before push; foreground builds, one at a time; new env vars → egret-config `ENV_OUTSIDE_CONFIG`.

---

### Task 0: Measure — Kestra semantics + egret seams + lift mechanism (load-bearing, report only)

**Files:** none (report).

- [ ] **Step 1 — ScheduleOnDates.** On the pinned source (`core/.../trigger/ScheduleOnDates.java`): exact `nextDate` semantics — is the list sorted? is `lastEvaluation` inclusive (the code uses `date.isEqual(lastEvaluation) || date.isAfter(lastEvaluation)`)? how is `timezone` applied? what does `nextEvaluationDate` return when the list is exhausted? recoverMissedSchedules interaction; which of inputs/labels/when it supports.
- [ ] **Step 2 — http.Trigger.** (`core/.../http/Trigger.java` + `HttpInterface` + `Request`): the exact `responseCondition` render context (what `response.*` exposes — statusCode, body, headers — and the var name, `response`); the `Request.Output` shape bound as the trigger output; how a poll ERROR differs from a falsy condition (no fire vs failOnTriggerError); interval default (60s) + overridability; which sub-keys egret will NOT model (`options`/HttpConfiguration, `encryptBody`, `formData`?) → each stays refused with its lift package. Confirm failOnTriggerError + allowConcurrent apply (polling trigger, `WorkerTriggerProcessor`).
- [ ] **Step 3 — egret trigger-kind + refusal mechanism.** Read `models/trigger.rs` `TriggerKind` (:75) + `kind_for` (:480): confirm both types currently route to `Leaf`. Find EXACTLY how validate refuses them (`spec/validate.rs:189`/`:3921`) — is the refusal keyed on `TriggerKind::Leaf`, on a type NOT in {schedule,webhook,flow}, or on an allowlist? This decides how to KEEP the refusal active after adding the variants (the lift-timing plan). Record the exact gate + how to keep the two new types refused at publish while they are typed variants.
- [ ] **Step 4 — scheduler dispatch sites.** List every `TriggerKind::Schedule(_)` site in `scheduler/sync.rs` (:115, :160, :607, :611, :661) and `scheduler/fire.rs` (:244, :257, :926) that must also admit the new kinds, and how `spec_key`/`next_evaluation_date` are computed, so Tasks 1–3 extend them consistently.
- [ ] **Step 5 — egret-egress entry.** The exact function #4's http.Request calls in `crates/egret-egress` (client.rs/guard.rs) + how the `workflow.http.allow_hosts` allowlist is enforced, to reuse for the poll. And how `secret()` resolves on http.Request fields (#10a) to mirror on trigger fields.
- [ ] **Step 6 — census + migration.** Baseline 1228/3/354/117/1702; the refusal-lift sites + a lift-refusal grep (code+message+fixture+tests) for the trigger-type refusal and any ScheduleOnDates/http.Trigger refusal assertion. Migration verdict (expect migration-free; next free number if needed). Record a BASE commit.

---

### Task 1: `ScheduleOnDates` — a Schedulable trigger on an explicit date list

**Files:**
- Modify: `crates/egret-workflow/src/models/trigger.rs` (`TriggerKind` enum :75, `kind_for` :480, re-serialise arm :411)
- Create: `crates/egret-workflow/src/trigger/schedule_on_dates.rs` (next-date computation)
- Modify: `crates/egret-workflow/src/scheduler/sync.rs` + `fire.rs` (admit the new kind; next-date from dates)
- Test: `crates/egret-workflow/tests/scheduler_fire.rs`

**Interfaces:**
- Consumes: Task 0 §1 (nextDate semantics) + §3 (keep publish-refused) + §4 (dispatch sites).
- Produces: `TriggerKind::ScheduleOnDates(ScheduleOnDates)`; `schedule_on_dates::next_after(dates, tz, after) -> Option<DateTime<Utc>>`.

- [ ] **Step 1 — Failing test (next-date).** In `trigger/schedule_on_dates.rs` unit tests: given `dates = [d1, d2, d3]` (unsorted) + a timezone, `next_after(.., after=d1) == Some(d2)` (strictly after, sorted), and `next_after(.., after=d3) == None` (exhausted). Pin inclusivity to Task 0 §1.

```rust
#[test]
fn next_after_returns_the_first_date_strictly_after() {
    let dates = parse_dates(&["2026-02-01T00:00:00Z", "2026-01-01T00:00:00Z", "2026-03-01T00:00:00Z"]);
    assert_eq!(next_after(&dates, Tz::UTC, dt("2026-01-01T00:00:00Z")), Some(dt("2026-02-01T00:00:00Z")));
    assert_eq!(next_after(&dates, Tz::UTC, dt("2026-03-01T00:00:00Z")), None);
}
```

- [ ] **Step 2 — Run → fail** (`next_after` undefined). `cargo test -p egret-workflow schedule_on_dates`.
- [ ] **Step 3 — Model + parse.** Add `ScheduleOnDates { dates: Vec<String>, timezone: Option<String>, inputs: Option<Map>, recover_missed_schedules: Option<...> }` mirroring `Schedule`'s field treatment (values kept as authored strings; dates parsed at fire). Add `TriggerKind::ScheduleOnDates(..)` + route it in `kind_for` (canonical `io.kestra.plugin.core.trigger.ScheduleOnDates` and short `scheduleOnDates`) + the re-serialise arm. Implement `next_after`.
- [ ] **Step 4 — Run → pass** the unit test.
- [ ] **Step 5 — Scheduler admit + fire.** Failing test in `scheduler_fire.rs`: a flow (parsed directly, NOT published) with a ScheduleOnDates trigger over two past dates fires once per date in order, then goes off the queue. Extend `sync.rs` (admit `ScheduleOnDates` to the triggers table; `spec_key` = digest of dates|timezone) and `fire.rs` (compute `next_evaluation_date` via `next_after`; fire via the Schedule path — inputs/when/labels/stopAfter/allowConcurrent reused). Keep the publish refusal active (Task 0 §3).
- [ ] **Step 6 — Run → pass.** Scoped: `cargo test -p egret-workflow --test scheduler_fire --test scheduler_sync --test routes --test execution_timestamps`. `cargo fmt --all --check`.
- [ ] **Step 7 — Commit.** `feat(workflow): ScheduleOnDates trigger (fire on an explicit date list)`

---

### Task 2: `http.Trigger` — model, parse, and interval scheduling (no poll yet)

**Files:**
- Modify: `models/trigger.rs` (`TriggerKind` + `kind_for` + re-serialise)
- Create: `crates/egret-workflow/src/trigger/http_trigger.rs` (model helpers + interval)
- Modify: `scheduler/sync.rs` + `fire.rs` (admit the kind; `next_evaluation_date = last + interval`)
- Test: `tests/scheduler_fire.rs`

**Interfaces:**
- Consumes: Task 0 §2 (fields, interval default) + §4 (dispatch).
- Produces: `TriggerKind::HttpTrigger(HttpTrigger)` with `uri/method/body/params/headers/content_type/response_condition/interval` (authored strings/Property); an `interval()` helper (default 60s). The actual poll+fire is Task 3; here a due http.Trigger tick is claimed and its interval advanced, with a seam (`poll_and_fire`) Task 3 fills — until then a due tick just advances (no fire), behind the publish refusal.

- [ ] **Step 1 — Failing test.** A directly-parsed flow with an http.Trigger (`interval: PT30S`) is admitted to `workflow.triggers` and its `next_evaluation_date` advances by the interval on a due pass (no execution yet). Assert the row exists and the date advances by 30s.
- [ ] **Step 2 — Run → fail.**
- [ ] **Step 3 — Model + parse + schedule.** Add the `HttpTrigger` struct + `TriggerKind::HttpTrigger` + `kind_for` routing (`io.kestra.plugin.core.http.Trigger`). Refuse the sub-keys Task 0 §2 says egret won't model (`options`/`encryptBody`/… ) at parse or validate, each citing its lift. Extend `sync.rs` to admit it; `fire.rs` to treat a due http.Trigger as "interval tick" (advance `next_evaluation_date += interval`) calling a `poll_and_fire` seam that is a no-op stub in this task.
- [ ] **Step 4 — Run → pass.**
- [ ] **Step 5 — Scoped + fmt.** As Task 1 Step 6.
- [ ] **Step 6 — Commit.** `feat(workflow): http.Trigger model + interval scheduling (poll in next task)`

---

### Task 3: `http.Trigger` — poll, condition, fire, and lifecycle

**Files:**
- Modify: `crates/egret-workflow/src/trigger/http_trigger.rs` (the poll) + `scheduler/fire.rs` (the claim→commit→poll→fire path)
- Modify: reuse `crates/egret-egress` (no change expected) + the render/secret surfaces
- Test: `tests/scheduler_fire.rs` (or a new `tests/http_trigger.rs`) with a local stub HTTP server

**Interfaces:**
- Consumes: Task 2's `poll_and_fire` seam + `HttpTrigger` model; Task 0 §2 (responseCondition ctx, Request.Output shape, error-vs-falsy) + §5 (egret-egress entry, allowlist, secret()).
- Produces: nothing downstream.

- [ ] **Step 1 — Failing tests (stub server).** Stand up a local HTTP stub. (a) endpoint returns 200 → `responseCondition` (`{{ response.statusCode < 400 }}`) truthy → one execution fires with the response bound under `{{ trigger.* }}`. (b) endpoint returns 500 → condition falsy → NO execution, interval advanced. (c) `failOnTriggerError: true` + the request errors (connection refused / disallowed host) → a born-failed execution. (d) `allowConcurrent: false` + a prior execution live → the poll is held (11d hold). (e) a non-allowlisted host → egress guard blocks, no fire (failOnTriggerError governs surfacing).
- [ ] **Step 2 — Run → fail** (poll_and_fire is a stub).
- [ ] **Step 3 — Implement the poll.** In the fire path, for a due http.Trigger: claim the row, COMMIT (release the lock), render the request fields (secret() via #10a), call `egret-egress` with the `workflow.http.allow_hosts` guard, render `responseCondition` against the response context (Task 0 §2), then in a FRESH transaction either fire (born-failed=false; bind `Request.Output` as the trigger record so 11a's `{{ trigger.* }}` binding exposes it) + advance, or just advance. On poll/eval error: `failOnTriggerError` true → born-failed execution (11d Task-2 mechanism) else `last_error`. Apply the 11d `allowConcurrent` hold before polling.
- [ ] **Step 4 — Run → pass** all five.
- [ ] **Step 5 — Scoped + fmt.** Add `--test http_trigger` if a new file; plus the scheduler/routes/timestamps set.
- [ ] **Step 6 — Commit.** `feat(workflow): http.Trigger poll + responseCondition + fire + lifecycle`

---

### Task 4: Reconcile — lift the refusal, census, docs

**Files:**
- Modify: `spec/validate.rs` (:189, :3921 refusal), `spec/keys.rs` (lift_of for the new types' keys + failOnTriggerError honoured-on-http.Trigger), `spec/schema/measure.rs` (read-set for the two types), `spec/schema/property-treatment.json` (regen), `spec/schema/README.md`
- Create: docs in egret-data/docs (separate PR, controller-handled)

**Interfaces:**
- Consumes: Task 0 §6 lift map. Both types become honoured; `failOnTriggerError` moves from flat Inert to honoured-on-http.Trigger (stays Inert/warned on Schedule/Webhook/Flow); allowConcurrent honoured on http.Trigger too.

- [ ] **Step 1 — Lift the refusal (one commit, behaviour correct).** Remove ScheduleOnDates + http.Trigger from the unsupported-trigger refusal; update the message so it lists only what is still refused (realtime/fs/plugin → #18). Grep + update every test/fixture that asserted these two were refused (lift-refusal discipline).
- [ ] **Step 2 — Census read-set + lift_of.** `measure.rs`: add the two types' honoured keys to the read-set. `keys.rs`: `failOnTriggerError` honoured on http.Trigger (not Inert there); allowConcurrent honoured on http.Trigger; keep Schedule/Webhook/Flow treatment from 11d.
- [ ] **Step 3 — Regenerate census golden.** `EGRET_WRITE_PROPERTY_TREATMENT=1`, then re-run without. Record the new aggregate (Task 0 pins the expected delta: two new trigger rows honoured + failOnTriggerError/allowConcurrent on http.Trigger; total rises by the two new types' key counts). Update README rows + Result line.
- [ ] **Step 4 — Run** `cargo test -p egret-workflow spec::keys` + census golden + `--test routes --test execution_timestamps` green; `cargo fmt --all --check`.
- [ ] **Step 5 — Docs (egret-data/docs, separate PR).** Document both triggers + divergences (poll in the scheduler not a worker; egress allowlist; unmodeled sub-keys). Bilingual.
- [ ] **Step 6 — Commit.** `feat(workflow): lift ScheduleOnDates + http.Trigger refusal + reconcile census (package 11e)`

---

## Self-Review notes

- **Spec coverage:** ScheduleOnDates → Task 1; http.Trigger model/schedule → Task 2, poll/fire/lifecycle → Task 3; refusal lift + census + failOnTriggerError-on-http.Trigger → Task 4; measurement/lift-map/migration → Task 0. D1–D6 are Global Constraints.
- **Lift timing:** refusal kept through Tasks 1–3 (behaviour tested via directly-parsed fixtures), lifted only in Task 4 — the 9a discipline; census moves once.
- **Migration:** expected migration-free (reuse `workflow.triggers`); if Task 0 finds polling needs stored state, insert a migration task before Task 2.
- **Risk:** Task 3 is the heaviest (new poll path + egress + lifecycle); if it over-runs, its five test cases (a–e) are independent enough to land incrementally, but keep it one task for one reviewer gate.
