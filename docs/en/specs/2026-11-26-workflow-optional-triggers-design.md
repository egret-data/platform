# Workflow optional trigger types (package 11e) — design

**Status:** approved (brainstorm 2026-11-26). Package #11 part 5.
**Roadmap:** egret = Kestra OSS engine; package #11 (triggers) part 5 — the two optional
trigger types the roadmap deferred. Predecessors: 11a–11d. Measured against local Kestra
source `96893b589`.

## 1. Goal

Lift the two trigger types egret currently refuses with the promise "roadmap package #11
lifts ScheduleOnDates and http.Trigger" (`spec/validate.rs:189`, `:3921`):

- **`io.kestra.plugin.core.trigger.ScheduleOnDates`** — a Schedulable trigger that fires on
  an explicit list of dates (not a cron). Light: reuses the existing scheduler.
- **`io.kestra.plugin.core.http.Trigger`** — a PollingTriggerInterface trigger that polls an
  HTTP endpoint on an interval and fires when a `responseCondition` holds. Heavy: egret has
  no polling-trigger infra; this package builds it (claim→commit→poll-outside-lock→fire).

Both become new `TriggerKind` variants (today they fall into `TriggerKind::Leaf` → refused).

## 2. Measurement (Kestra `96893b589`)

- **ScheduleOnDates** (`core/.../trigger/ScheduleOnDates.java`): `extends AbstractTrigger
  implements Schedulable, TriggerOutput<VoidOutput>`. `interval = null` (hidden — NOT polling).
  Fields: `dates: Property<List<ZonedDateTime>>`, `timezone` (default system zone), `inputs`,
  `recoverMissedSchedules`. `eval`/`nextEvaluationDate` compute the next date from the sorted
  `dates` list ≥ last evaluation — the scheduler evaluates it exactly like Schedule.
- **http.Trigger** (`core/.../http/Trigger.java`): `extends AbstractTrigger implements
  PollingTriggerInterface, HttpInterface, TriggerOutput<Request.Output>`. `interval =
  Duration.ofSeconds(60)`. Fields: `uri`, `method` (GET), `body`, `params`, `formData`,
  `contentType` (application/json), `headers`, `options` (HttpConfiguration), `responseCondition`
  (`{{ response.statusCode < 400 }}`), `encryptBody`. On each interval the worker makes the
  HTTP request, renders `responseCondition` against the response, and fires (binding the
  `Request.Output` as trigger output) iff it is truthy.
- **failOnTriggerError / allowConcurrent**: Kestra honours these for WORKER/POLLING triggers
  (`WorkerTriggerProcessor.java:181`). ScheduleOnDates is Schedulable (scheduler-evaluated, so
  allowConcurrent applies as for Schedule; failOnTriggerError does NOT — same as Schedule, 11d
  D6). http.Trigger is a polling trigger, so BOTH apply to it (this is the polling trigger 11d's
  `failOnTriggerError` Inert verdict was waiting for).
- **egret today**: `TriggerKind` (models/trigger.rs:75) = Schedule/Webhook/Flow/Leaf;
  `kind_for` (:480) routes unknown types to Leaf; validate refuses them. Scheduler (`sync.rs`,
  `fire.rs`) is keyed on `TriggerKind::Schedule(_)` in ~6 places. `egret-egress` (client.rs +
  guard.rs) provides the HTTP egress + host allowlist used by #4's http.Request.

## 3. Architecture

### 3.1 Two new TriggerKind variants
`TriggerKind::ScheduleOnDates(ScheduleOnDates)` and `TriggerKind::HttpTrigger(HttpTrigger)`,
with `kind_for` routing `"scheduleOnDates"` / `"http.Trigger"` (and the full canonical names)
to them. New model structs under `models::trigger` + `trigger/`.

### 3.2 ScheduleOnDates (light — reuse the scheduler)
- `scheduler/sync.rs`: the filter that today admits only `TriggerKind::Schedule(_)` to the
  `workflow.triggers` table also admits `ScheduleOnDates`. Its `spec_key` is a digest of
  `dates|timezone` (re-init the dates on change, as a cron change does).
- `scheduler/fire.rs`: `next_evaluation_date` = the first date in the sorted list strictly
  after the last evaluation (timezone-aware), or NULL when the list is exhausted (off the
  queue, like a cron with no future tick). Fire reuses the Schedule path: inputs render,
  labels, when skip-and-advance (11b), stopAfter (11b/11d), allowConcurrent hold (11d),
  recoverMissedSchedules (11b). `execution_trigger.type` = the ScheduleOnDates canonical name.

### 3.3 http.Trigger (heavy — new polling path; D-poll)
- Scheduled in `workflow.triggers` with `next_evaluation_date = last + interval`. When due,
  the scheduler **claims the row, commits (releases the lock), performs the HTTP request
  OUTSIDE any transaction, then fires + advances in a fresh transaction** — the webhook/#4
  egress pattern, so network I/O never holds a DB lock or blocks the fire pass's locks.
- The request goes through `egret-egress` (client + guard), gated by the existing
  `workflow.http.allow_hosts` allowlist (#4). Fields (`uri`/`method`/`body`/`params`/
  `headers`/`contentType`) render against the trigger context; `secret()` resolves on them
  (#10a). `responseCondition` renders against the response (`{{ response.statusCode < 400 }}`
  default); a truthy result fires an execution with the `Request.Output` bound as the trigger
  output (`{{ trigger.* }}` — 11a binding), a falsy result just advances the interval.
- **failOnTriggerError** (D-poll-honour): a poll/eval error with `true` → a born-failed
  execution (fire.rs born_failed, 11d Task 2 mechanism), else logged to `last_error`.
  **allowConcurrent**: the 11d derived hold applies (don't poll-fire while the previous
  execution is live). Both are honoured for http.Trigger in the census.

### 3.4 Reconcile
Drop ScheduleOnDates + http.Trigger from the unsupported-trigger refusal (validate.rs:189,
:3921). Add both to the census; `failOnTriggerError` moves from a flat `Lift::Inert` to
honoured-on-http.Trigger (still Inert/warned on Schedule/Webhook/Flow). Regenerate
property-treatment.json + README.

## 4. Decisions

- **D1** 11e = both types in one package (user-approved).
- **D2 (poll execution)** claim→commit→poll-outside-lock→fire, reusing the scheduler + the
  `workflow.triggers` table; NO separate polling worker loop.
- **D3 (lifecycle on http.Trigger)** honour `failOnTriggerError` + `allowConcurrent` for
  http.Trigger (it is the polling trigger they apply to); update `failOnTriggerError` census
  treatment accordingly.
- **D4 (state/migration)** reuse the `workflow.triggers` table; `fire.rs` already reads the
  flow source to dispatch on `TriggerKind`, so no `trigger_type` column is needed. Expected
  migration-free; Task 0 confirms (fallback: a column at mig 0244, or whatever is free then).
- **D5 (egress)** http.Trigger outbound requests reuse `egret-egress` + `workflow.http.allow_hosts`
  (the #4 guard); `secret()` resolves on request fields (#10a).
- **D6 (out of scope)** realtime triggers (`*.RealtimeTrigger`), fs.http.Trigger and all
  third-party plugin triggers stay refused (roadmap #18 plugin host). Only the two core types
  named in the refusal message are lifted.

## 5. Task 0 — load-bearing measurement (pinned source + egret)

1. ScheduleOnDates: exact `nextDate` semantics (sorted? inclusive of lastEvaluation?
   timezone application), recoverMissedSchedules interaction, inputs/labels/when applicability.
2. http.Trigger: exact `responseCondition` render context (what `response.*` exposes:
   statusCode, body, headers), the `Request.Output` shape bound as trigger output, how a poll
   ERROR vs a falsy condition differ, interval default + overridability, encryptBody/options
   handling (likely refuse encryptBody/options sub-keys egret does not model — record which).
3. egret scheduler: every `TriggerKind::Schedule(_)` dispatch site in `sync.rs`/`fire.rs` that
   must also admit the two new kinds; confirm the triggers table hosts them without a new column.
4. egret-egress: the exact entry point + how #4's http.Request calls it, to reuse for the poll.
5. Census baseline (post-11d: 1228/3/354/117/1702) + the refusal lift sites + a lift-refusal
   grep (code + message + fixture) for the trigger-type refusal and any test asserting
   ScheduleOnDates/http.Trigger is refused. Migration verdict.

## 6. Testing

- Scoped runs include `--test routes --test execution_timestamps --test scheduler_fire
  --test scheduler_sync`; tick-driving tests hold the file-local serialising mutex.
- ScheduleOnDates: fires on each listed date in order; exhausted list → off the queue;
  honours when/labels/stopAfter/allowConcurrent.
- http.Trigger: a stub HTTP endpoint (wiremock-style or a local test server) → a truthy
  `responseCondition` fires with the response bound; a falsy one advances without firing; a
  poll error with `failOnTriggerError:true` → born-failed execution; allowConcurrent holds the
  next poll while a prior execution is live; egress allowlist blocks a non-allowed host.
- Census golden regenerated; both types honoured; total adjusts (Task 0 pins the exact count).

## 7. Divergences (to document)

- http.Trigger poll runs in the single scheduler (claim→commit→poll→fire), not Kestra's
  separate worker-trigger processor; interval is best-effort around the fire-pass cadence.
- egress allowlist gates http.Trigger (egret security divergence, as #4 http.Request).
- Any http.Trigger sub-key egret does not model (e.g. `options`/`encryptBody`) stays refused
  with the package that would lift it (record from Task 0).
