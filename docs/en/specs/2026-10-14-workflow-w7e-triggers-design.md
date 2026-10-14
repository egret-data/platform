# W7e — Workflows UI, part 5: triggers (flow Triggers tab, global Triggers page, runtime switch)

**Status:** design, 2026-10-14. Not implemented.

**Depends on:**
- W7a (#659): the Workflows area, the flow detail and its tab list (`FLOW_TABS`, `frontend/src/routes/workflows/FlowDetail.tsx:52-53`, whose comment already reserves a slot for W7e).
- W7b (#662): the executions list and its state chips.
- W7d (#669): the off-runtime parse with a flow bound (`namespaces/dependencies.rs:204-221`, `routes.rs:3210-3233`), reused here.
- W12 (`workflow.triggers`, `scheduler::sync`, `fire_due`), W13a/W13b (`workflow.flow_listeners`, `flow_trigger_windows`), W6 Task 7 (the webhook route).

The frontend rulebook `docs/en/frontend-rules.md` binds every frontend line.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §8 (Kestra's UI as reference) and §9 row W7.

**Decided with the user, 2026-10-14:**
- **Backfill is a separate package, W7f** (its migration, routes, the `fire_one` wiring, the form and the progress banner). W7e builds none of it and must not foreclose it (§9).
- **A runtime enable/disable switch, stored in the database as Kestra stores it,** separate from the YAML `disabled:`. Re-enabling a `Schedule` asks the user whether to recover the missed ticks (Kestra's `recoverMissedSchedules`).

**Default decisions (2026-10-14),** taken under the brief's rule "follow Kestra unless egret has a reason; keep safety tight". Each is explained where it is used:
- D1. The switch is a table of **disabled** triggers, `workflow.disabled_triggers`: a row means "switched off at runtime", no row means on. There is no stored "enabled" row (§3.1).
- D2. The switch is the authority; `workflow.triggers.disabled` and `workflow.flow_listeners.disabled` become its **effective copy** (YAML flow OR YAML trigger OR runtime), written by `sync` and by the switch route. Every existing `NOT disabled` predicate then honours it unchanged (§3.2).
- D3. The switch covers `Schedule`, `Webhook` and `Flow` triggers. Kestra's covers only scheduler-driven triggers (§2). A `Leaf` trigger (any other type) is never fired by egret and has no switch.
- D4. A trigger disabled in the YAML cannot be enabled from the UI or the API: 409 `trigger_disabled_in_source` (Kestra's server accepts it and its UI hides the switch).
- D5. Re-enabling a `Schedule` with "skip missed" moves `next_evaluation_date` to the first tick after `now()` in the switch's transaction. "Recover missed" follows the trigger's own `recoverMissedSchedules` (absent = `ALL`), as Kestra does; W7e therefore starts reading that property, which egret drops today (§3.3).
- D6. A valid save that no longer declares a trigger id drops its switch row; a source that does not parse never drops one; a soft delete drops them all (§3.4).
- D7. A switched-off `dependsOn` listener's open window is left to expire, as for a YAML-disabled one today (§3.2).
- D8. A new index serves "last execution per trigger" (§3.6), kept only if `EXPLAIN` shows the query uses it (0183's rule).
- D9. The global list parses drafts off the async workers with a bound of 2,000 flows **after** the namespace and flow filters, 422 `triggers_too_large` beyond (§3.5).
- D10. Bulk switching is by selection only, at most 100 triggers, one transaction per trigger, synchronous, with a result per trigger. No "select all matching" (§3.7).
- D11. A duplicate trigger id becomes a validation error, `duplicate_trigger_id`, as in Kestra, but a **save-only** finding: it is shown on the draft and blocks publish, and it is not in `validate_for_run`, so stored flows that already declare a duplicate keep running (§3.8).
- D12. No unlock, restart or delete-state action (egret has no persisted lock; §3.9). No warning that a draft does not fire: a draft fires (§4.4).
- D13. The migration is numbered **0189**: `0186`-`0188` are taken by the unmerged branch `feat/view-governance` (§3.1).
- D14. `LAST` recovery uses a new `ScheduleTrigger::last_tick_at_or_before`; the existing `cron.last_before` is strictly-before and private (§3.3).
- D15. W7d's `off_runtime` (`dependencies.rs:213`) becomes `pub(crate)` with a neutral error message (§3.5).
- D16. The response carries `disabled_by_name` beside `disabled_by`, because the browser cannot resolve user ids (user listings are admin-gated) (§3.5, §4.1).
- D17. W7e adds **no** ingress, nginx or Vite routing for webhooks and does not build the webhook URL from `window.location.origin`: the UI shows the webhook path with a copy button and a note (§4.2).

## 1. Why

Every trigger a flow declares is visible only as YAML today. No route lists triggers or their state (`management_router`, `crates/egret-workflow/src/routes.rs:3270-3430`, has flows, versions, executions, namespaces, files, KV, resume and replay). A user cannot see when a schedule fires next, why it went off the queue (`workflow.triggers.last_error`), how far a `dependsOn` window has got, or the URL of a webhook. The only way to stop a trigger is to edit the flow's YAML, which also creates a new revision. Kestra users expect a Triggers tab on the flow, a global Triggers page, and a switch.

## 2. Divergences from Kestra

Every row below is **read in source** (the Kestra checkout at `data-portal/kestra`; paths relative to it). None is observed yet: the implementation plan's last task runs §7's scenario against the local Kestra image, as W7d's Task 9 did, and relabels each row it measures.

| Kestra | egret | Why |
|---|---|---|
| The runtime `disabled` lives in the trigger state and is read by the scheduler only (`scheduler/.../TriggerEventHandler.java:228-250`). The webhook path checks only the flow's `disabled` (`webserver/.../ExecutionController.java:844-858`) and matches webhooks without a disabled filter (`:879-899`); Flow triggers check only the YAML (`executor/.../FlowTriggerService.java:50, 61`) | The switch stops all three kinds: the schedule claim, the Flow-trigger listener select and outbox insert, and the webhook route (409) | A switch that leaves a webhook firing is a false control |
| The flow tab's switch is hidden when the trigger has no state row (`ui/src/components/flows/FlowTriggers.vue:146-163`) | Every `Schedule`, `Webhook` and `Flow` trigger has the switch; a `Leaf` shows "Not run by egret" and no switch | egret has no state row for webhooks, and the switch covers them (D3) |
| Enabling a trigger whose YAML says `disabled: true`: the server accepts it (`TriggerStateService.validateToggleable` checks only that flow and trigger exist, `webserver/.../TriggerStateService.java:385-393`); the UI disables the switch (`FlowTriggers.vue:155-161`) | 409 `trigger_disabled_in_source`, "This trigger is disabled in the flow's YAML; remove `disabled: true` there" | An API caller should not get a 200 for a change that has no effect |
| Re-enable with `recoverMissedSchedules=true` follows the trigger's `recoverMissedSchedules` (default `ALL`, `core/.../Schedulable.java:47-51`); absent or false is `NONE` (`TriggerEventHandler.java:259-283`). `NONE` also sets `evaluatedAt` to now | The same three outcomes (§3.3). `evaluated_at` is left alone | egret has no startup recovery that reads `evaluated_at`; it stays "the last tick processed" |
| The enable dialog offers "Do not recover missed schedules" (default) and "Follow `recoverMissedSchedules`" (`ui/src/components/triggers/TriggerEnableDialog.vue`) | The same two choices, plus the number of missed ticks and what the trigger's `recoverMissedSchedules` will do with them | The user decides with the cost in front of them |
| Unlock, restart and delete-state actions (`TriggerController.java:161-217, 339`) | None | egret holds no persisted lock (§3.9) |
| Bulk actions are asynchronous, "by triggers" or "by query" (`TriggersManage.vue:790-880`) | Synchronous, by selection, at most 100, a result per trigger | One transaction per trigger is fast enough; a result per row says what happened |
| Global page at `admin/triggers` with an "add" catalog tab and CSV export (`ui/src/routes/routes.ts:100`, `Triggers.vue`) | "Triggers" in the Workflows nav; no catalog, no CSV | Nothing to install; export is a follow-up |
| Filters: namespace, flow, date range, scope, trigger id, worker, enabled/disabled; no type filter (`ui/src/components/filter/configurations/triggerFilter.ts`) | Namespace (prefix), flow, kind, enabled/disabled, a text search | egret has no worker id; kind is what a user scans for |
| Columns `lastTriggeredDate`, `executionId`, `workerId` | "Last execution" (its id, state and time); no worker | egret stores no last-triggered date; the execution is the record |
| The expanded row embeds the trigger's logs | No trigger logs | egret writes none: schedule errors are `last_error`, Flow-trigger failures are server logs (§10) |
| No webhook URL copy on the flow tab (`FlowTriggers.vue:346-359` offers a test-event dialog) | The details drawer shows the URL with a copy button | The URL is the reason a user opens a webhook's details |
| `dependsOn` window progress is not shown | The drawer shows the open window and which conditions are met | egret has the data (`flow_trigger_windows.results`) |
| Duplicate trigger ids are a validation error (`core/.../FlowValidator.java:65-68`) | The same, from W7e (D11) | Every route here is keyed by trigger id |
| Backfill column, form and banner | W7f | User decision |

## 3. Backend

### 3.1 The switch table

Migration `0189_workflow_disabled_triggers.sql` (default decision, 2026-10-14). `0185` is the last on main, but the unmerged branch `feat/view-governance` holds `0186`-`0188`; `0189` collides with neither in either merge order. Re-check every ref before merging, since the number space is shared.

```sql
CREATE TABLE workflow.disabled_triggers (
  flow_id      uuid        NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  -- The trigger's `id:` in the YAML, of any kind.
  trigger_id   text        NOT NULL,
  -- Copied from the flow for filtering; not an authority.
  workspace_id uuid        NOT NULL,
  disabled_by  uuid        REFERENCES users (id) ON DELETE SET NULL,
  disabled_at  timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (flow_id, trigger_id)
);

COMMENT ON COLUMN workflow.triggers.disabled IS
  'Effective: flow YAML disabled OR trigger YAML disabled OR a row in workflow.disabled_triggers (W7e).';
COMMENT ON COLUMN workflow.flow_listeners.disabled IS
  'Effective: flow YAML disabled OR trigger YAML disabled OR a row in workflow.disabled_triggers (W7e).';
```

- **A row means off (D1).** The brief proposed a `disabled boolean` column. An "enabled" row would mean the same as no row, so enabling deletes the row and the column is dropped. Who switched it off and when are kept for the UI.
- **Why a table of its own:** `sync` deletes every `workflow.triggers` and `flow_listeners` row of a flow whose source does not parse (`crates/egret-workflow/src/scheduler/sync.rs:55-59, 76-82, 215-223`), and a `Webhook` has no row at all (`sync.rs:1-4`). The switch must survive both.
- **The old columns' meaning changes,** so the migration states it with `COMMENT ON` rather than by editing the prose of 0156 (`migrations/0156_workflow_triggers.sql:22-24`) and 0157 (`0157_workflow_flow_triggers.sql:32`), which stays as written.

### 3.2 Enforcement: the effective copy (D2)

`effective_disabled = flow YAML disabled || trigger YAML disabled || row in disabled_triggers`.

- **`sync`** (`sync.rs:48`) reads the flow's `disabled_triggers` ids once, in its transaction, and writes the effective value where it writes `disabled` today: `workflow.triggers` (`sync.rs:89, 119-134`) and `workflow.flow_listeners` (`sync.rs:184, 225-245`).
- **The switch route** writes the same value onto the existing row, in the transaction that writes `disabled_triggers` (§3.3).
- **What then honours it, unchanged:**
  - the schedule claim, `AND NOT t.disabled` (`scheduler/fire.rs:144-158`), so an off schedule is never claimed and never consumes a tick; the due index stays partial on `NOT disabled` (`0156:35-36`);
  - the Flow-trigger listener select, `AND NOT l.disabled` (`flow_trigger/process.rs:572-589`);
  - the outbox insert, which writes an event only if some listener with `NOT l.disabled` hears it (`worker/claim.rs:2477-2491`).
- **Why a copy and not a join:** three hot statements keep their shape, the partial indexes `workflow_triggers_due` and `workflow_flow_listeners_ws` (`0157:35`) stay right, and there is one function that computes the value.
- **The webhook route** has no row to read. After the key matches and after the YAML trigger check (`webhook_routes.rs:559-566`), it reads `disabled_triggers` for `(flow, trigger.id)` and answers the same 409 as a YAML-disabled trigger: "Cannot execute webhook: the trigger 'x' is disabled." The lookup runs only after the key matched, so the four absences stay one indistinguishable 404 (`webhook_routes.rs:107-125`).
- **`fire_one`** still compares the source with the claimed row (`fire.rs:204-213`); an off row is not claimed. Its `resync` (the Frozen path, which calls `sync`) now reads the flow `FOR SHARE SKIP LOCKED` (see **Concurrency** below).
- **Windows (D7).** A switched-off `dependsOn` listener's open window in `flow_trigger_windows` is left alone. It is not fed while off, it expires at `window_end` and `sweep_expired_windows` removes it (`process.rs:280-292`), which is what a YAML-disabled listener's window does today (the listener row stays, `sync.rs:184`, so the cascade from `0158` does not fire). A re-enable inside the window keeps the conditions met before the switch.

**Concurrency.** The switch route takes, in order: the flow row `FOR NO KEY UPDATE`, then the trigger's `workflow.triggers` or `flow_listeners` row `FOR UPDATE`. This is a save's order (flow, then trigger rows; `scheduler/mod.rs` statement 4).
- Against a save or `reconcile` (`sync.rs:319-334`, flow `FOR SHARE`): serialised on the flow row, so `sync` never computes the effective value from a switch that is about to change. Without the flow lock a save could read the old switch, wait on the row the route holds, and then overwrite the route's value: a lost update.
- Against a pass: the pass holds the trigger row and wants `KEY SHARE` on the flow, which `FOR NO KEY UPDATE` does not block, so there is no cycle. The route waits for an in-flight fire to commit. **After the route answers, no new fire of that schedule starts.**
- Against a pass's `resync` (`fire.rs`, the Frozen path: the claimed row disagrees with the source and `sync` re-runs inside the pass's transaction): `resync` reads the flow `FOR SHARE SKIP LOCKED`. Without the flow lock it could read the switch rows before the route commits, wait on the trigger row the route holds, and then write the stale value over the route's (review I1: a lost update, the switch row off and the copy on, so the claim fires it). A waiting `FOR SHARE` would deadlock when the route switches the claimed trigger itself (the route holds the flow and wants the trigger row; the pass holds the trigger row and wants the flow), and `NOWAIT` would abort the pass's transaction. When the flow is held, `resync` does nothing: the claimed trigger stays frozen (nothing fires, its dates stay), the holder's own write puts the copy right, and a later pass resyncs a row that still disagrees. `FOR SHARE` conflicts with neither the `KEY SHARE` of a fire's execution insert nor `reconcile`'s `FOR SHARE`. Tests: `disabled_triggers.rs` `a_resync_does_not_undo_a_concurrent_switch_on_another_trigger`, `a_resync_of_the_trigger_being_switched_does_not_deadlock`.
- A Flow-trigger pass reads listeners without a lock (`process.rs:570-571`), and a webhook request may have read the switch before it changed. Either may fire once more after the route answers. This is stated in the API description.

### 3.3 Switching on and off

`PUT /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/disabled` requires `workflow:manage` (`routes.rs:108`), like the other flow writes.

```json
{ "disabled": false, "recover_missed": false }
```

- **Refusals:**
  - 404 when the flow is not in this workspace or is in the trash (`FlowRepo::get` carries the workspace predicate; the trash is checked as `startable_flow` does, `routes.rs:2262-2270`), or the source declares no trigger with that id.
  - 400 when the source does not parse (the switch cannot know the trigger's kind).
  - 409 `trigger_disabled_in_source` on `disabled: false` when the flow or the trigger is `disabled: true` in the YAML (D4). Switching such a trigger **off** is accepted, so the switch is already set when the YAML is edited.
  - 409 `trigger_not_supported` for a `Leaf` trigger.
- **Off:** insert the row (idempotent), update the effective copy.
- **On:** delete the row, update the effective copy. For a `Schedule` with a row that is on the queue (`next_evaluation_date IS NOT NULL`) and whose next date has passed, apply `recover_missed`:
  - `false` or absent (Kestra's default `SKIP`): `next_evaluation_date` = the first tick after the transaction's `now()`, computed as `sync` computes a first date (`TriggerState::initialise`, `trigger/mod.rs:245-260`, with `FixedClock(now)`, `sync.rs:93-95`).
  - `true`: the trigger's `recoverMissedSchedules`:
    - absent or `ALL`: dates kept; every missed tick is replayed, one per pass (`trigger/schedule.rs:226-229`).
    - `NONE`: as `false`.
    - `LAST`: `next_evaluation_date` = the latest tick at or before `now()` (a new `ScheduleTrigger::last_tick_at_or_before`, since the existing `cron.last_before`, `schedule.rs:1391`, is strictly-before and private, so it would miss a tick exactly at `now()`) when it is after `evaluated_at`; otherwise kept. With no `evaluated_at` (never fired), as `NONE`. This is Kestra's `updateForLastMissedSchedule` (`TriggerEventHandler.java:283-306`).
  - `evaluated_at` and `last_error` are not touched. A row that is off the queue stays off; the next save re-queues it (`sync.rs:111-118`). `recover_missed` is ignored for other kinds.
- **A later save does not undo a skip.** `sync` keeps a queued row's dates while `spec_key` (`cron|withSeconds|timezone`, `sync.rs:338-345`) is unchanged (`sync.rs:126-134`), so the moved date stands. A changed `spec_key` re-initialises to the next tick after now, which is also a skip.
- **`recoverMissedSchedules` is read for the first time.** `models::Schedule` drops it today (`models/trigger.rs:93-96`) and `serde` has no `deny_unknown_fields`, so it parses and is ignored. W7e captures it as `lateMaximumDelay` is captured (`models/trigger.rs:130-150`): a raw value, kept on re-serialisation. Only this route reads it. A value other than `ALL`, `NONE` or `LAST` answers 409 `recover_missed_unknown` on `recover_missed: true`; the scheduler's other paths are unchanged (§10).
- **Answer:** the trigger's entry, as `GET …/triggers` returns it (§3.5).
- **Audit:** `workflow_trigger:disable` / `workflow_trigger:enable`, resource `workflow_trigger`, id `{flow uuid}/{trigger_id}`, with `recover_missed` and the moved date, through `st.audit.emit` as `update_flow` does (`routes.rs:787-797`).

### 3.4 The switch's lifecycle (D6)

Inside `sync`, after the rows:
- **Source parses:** delete the flow's `disabled_triggers` rows whose `trigger_id` the source no longer declares, under any kind. A renamed trigger id is a new trigger and starts on, as in Kestra.
- **Source does not parse:** keep every row. No user switch is lost to a broken draft.
- **`gone` (soft delete):** delete them all. `ON DELETE CASCADE` does not fire on a soft delete, and the delete already drops every trigger row so that a trashed flow keeps no trigger state (`repo.rs:655-679`). There is no restore route.
- **A kind change under the same id** keeps the row: the switch is per id, as Kestra's state is.

The brief's premise "an invalid or unparseable save must not wipe overrides" holds as a rule, with one correction: the API cannot store an unparseable source (`update_flow` and `create_flow` call `parse_draft`, `routes.rs:519-529`, which answers 400). A stored source can still fail to parse after a parser change, and `reconcile` runs `sync` on every flow at every start (`sync.rs:300-317`). That is the case the rule protects. A source that parses but fails validation saves normally (`parse_draft` does not validate), and its trigger ids are read as declared.

### 3.5 Reading triggers

Both routes require `workflow:read` (`routes.rs:105`).

**`GET /api/ws/{workspace}/workflows/flows/{id}/triggers`**: one entry per declared trigger, in declaration order.

| Field | Source |
|---|---|
| `trigger_id`, `type`, `kind` (`schedule` / `webhook` / `flow` / `other`) | the YAML |
| `yaml_disabled` (flow or trigger), `runtime_disabled`, `disabled_by`, `disabled_by_name`, `disabled_at`, `effective_disabled` | YAML, `disabled_triggers` (the name is read in the same statement; the browser cannot resolve a user id, as the user listings are admin-gated) |
| `can_toggle`, and `toggle_refusal` when false | D3, D4, the caller's `workflow:manage` (`caller_may_manage`, `routes.rs:1651-1660`) |
| `duplicate_id` | D11 |
| `schedule`: `cron`, `with_seconds`, `timezone` (absent = UTC), `next_evaluation_date`, `evaluated_at`, `last_error`, `off_queue`, `missed_ticks` (when switched off or overdue; counted up to 1,000, then "1000+"), `recover_missed_schedules` | YAML, `workflow.triggers` |
| `flow`: `states`, `listening` (a `flow_listeners` row exists) and `not_listening_reason` (a window-free trigger that fails `resolve` gets no row, `sync.rs:194-211`), `depends_on`: `mode`, `min_satisfied`, `conditions` [`{namespace, flow_id, states, labels, when, met}`], `window` `{kind, start, end}` or none | YAML, `flow_listeners`, `flow_trigger_windows` |
| `webhook`: `url_path` (`/workflows/v1/{ws}/executions/webhook/{namespace}/{flow}/{key}`, `webhook_routes.rs:469-472`), `key_error` when the key does not render, `unsupported` (`declared_unsupported`) | YAML, rendered key |
| `last_execution`: `{id, state, created_at}` | §3.6 |

And at the top: `flow_disabled` (YAML or `state = 'disabled'`), `flow_deleted`, `scheduler_enabled`, `worker_enabled`, `webhooks_enabled`.

- **Conditions** are labelled by recomputing `condition_key` (`trigger/depends_on.rs:92`) for each `dependsOn` entry; `met` is `results[key] == true`. A window whose `window_end <= now()` is reported as none (the sweep or the next event removes it).
- **The webhook key.** It renders against the flow's id and namespace, its `variables` and the trigger's id and type only (`KeyContext::render_context_for`, `trigger/webhook.rs:436-452`), all read from the source that `GET /flows/{id}` already returns to `workflow:read` (`FlowDto.source`, `routes.rs:314-321`). Returning the rendered URL therefore **does not widen** who can learn the key. The UI shows it masked until revealed.
- **Server flags.** `WorkflowState` (`routes.rs:200-251`) gains `trigger_flags { scheduler_enabled, worker_enabled, webhooks_enabled }`, set from `config.workflow` (`egret-config/src/lib.rs:596, 619, 689`) where the state is built (`egret-bin/src/main.rs:2302`). The deployment sets them once for every replica (`deploy/helm/egret/values.yaml:86-98`).
- **A trashed flow** answers its triggers with `flow_deleted: true`, every one effective-disabled and `can_toggle: false` (`get_flow` serves trashed drafts, `routes.rs:695-700`).

**`GET /api/ws/{workspace}/workflows/triggers`**: every trigger of the workspace's flows not in the trash.

- **Query:** `namespace_prefix` (dot-anchored, as W7b's), `flow_id` (uuid), `kind` (comma-separated), `state` (`enabled` / `disabled`, on `effective_disabled`), `q` (case-insensitive substring of trigger id, flow id or namespace), `limit` (default 50, max 200, clamped), `offset`.
- **Row:** the flow's uuid, namespace and flow id, plus the per-flow entry without `depends_on.conditions` and without `webhook` (the drawer fetches the per-flow route).
- **Page:** `{rows, total, limit, offset, unparsable, scheduler_enabled, worker_enabled, webhooks_enabled}`; `unparsable` counts flows in scope whose source did not parse (their triggers are unknown). Order: namespace, flow id, declaration order.
- **How (D9):** the flows in scope are read with the namespace and flow filters in SQL, at most 2,001 (`dependency_drafts`'s shape, `repo.rs:381-397`). More than 2,000 answers 422 `triggers_too_large`, "more than 2000 flows match; narrow by namespace". The sources are parsed on the blocking pool (W7d's `off_runtime`, `dependencies.rs:213`, lifted to `pub(crate)` with a neutral error message, since today it is private and its message names the dependencies graph). The state tables are read in one statement each for the flow ids in scope. Kind, state and search filter in memory, then the page is cut, then `last_execution` is read for that page only.
- **Why parse:** webhooks and `Leaf` triggers have no row anywhere, and only the source knows them.

### 3.6 Last execution per trigger (D8)

```sql
SELECT k.flow_id, k.trigger_id, e.id, e.state, e.created_at
  FROM unnest($1::uuid[], $2::text[]) AS k(flow_id, trigger_id)
  CROSS JOIN LATERAL (
    SELECT id, state, created_at FROM workflow.executions e
     WHERE e.flow_id = k.flow_id
       AND e.execution_trigger IS NOT NULL
       AND e.execution_trigger->>'id' = k.trigger_id
     ORDER BY e.created_at DESC LIMIT 1) e;
```

- **No index covers it today.** `workflow_executions_flow_idx` is on `flow_id` only (`0134:93`), so each probe reads every execution of the flow; `workflow_executions_one_per_schedule_tick` (`0156:49-51`) covers schedules only and orders by `scheduled_date`.
- **The migration adds** `CREATE INDEX workflow_executions_last_by_trigger ON workflow.executions (flow_id, (execution_trigger->>'id'), created_at DESC) WHERE execution_trigger IS NOT NULL`: one index-only descent per probe, at most 200 probes per page.
- **It is measured, as 0183 measured** (`0183:74-85`): a test pins `EXPLAIN` using the index. If the planner does not choose it, the index is dropped from the migration and the measurement is recorded there instead.
- **Cost:** one more index on every triggered execution's insert; executions started from the API or by replay have no `execution_trigger` (`routes.rs:2656, 2932`: `trigger: None`) and are outside the partial index.

### 3.7 Bulk switching (D10)

`PUT /api/ws/{workspace}/workflows/triggers/disabled` requires `workflow:manage`.

```json
{ "triggers": [{ "flow_id": "…uuid…", "trigger_id": "daily" }], "disabled": true, "recover_missed": false }
```

- At most 100 entries (400 beyond); a body cap of 64 KiB.
- Each entry runs §3.3 in its own transaction, in the order given. Entries are independent: a refusal is recorded and the next entry runs.
- 200 with `{results: [{flow_id, trigger_id, ok, code?, message?, trigger?}]}`. A flow outside the workspace is `not_found`, never a 403.
- One audit event per switched trigger.

### 3.8 Duplicate trigger ids (D11)

`spec::validate` gains `duplicate_trigger_id` (error). Kestra refuses the same (`FlowValidator.java:65-68`). Unlike `duplicate_task_id` (`spec/validate.rs:593`, raised in `validate_for_run`), it lives in `save_only_findings`: it is shown on the draft and blocks publish, and it is **not** in `validate_for_run`. `executor::build_plan` calls `validate_for_run` at run time (`validate.rs:335-338`), so a run-time error there would stop every execution of a stored flow that already declares one trigger id twice, a behaviour change this package does not ask for. Drafts still save with validation errors, so the read routes mark duplicates (`duplicate_id: true`) and the switch applies to the id, which is all the trigger tables can key on today: their primary key is `(flow_id, trigger_id)`, so `sync` already folds two schedules with one id into one row (`sync.rs:119-145`).

### 3.9 What is not built, and why

- **Unlock.** egret persists no lock: the claim's lock is a row lock held for one transaction (`fire.rs:157`; `scheduler/mod.rs` statement 1). There is nothing to unlock.
- **Restart and delete-state.** "Delete the state" of a schedule would re-initialise it to the next tick, which re-enabling with "skip" already does. Recorded in §10.
- **Backfill.** W7f.

### 3.10 Comments to correct

Each was checked in this worktree:
- `crates/egret-workflow/src/lib.rs:92-96` says the routes "that create a backfill" live outside the module. None exist. New text: the loop that drives the clock lives in `scheduler`; backfill has no route yet (W7f).
- `crates/egret-workflow/src/routes.rs:2271-2274` and `webhook_routes.rs:569-571` say the `state` column is "the one the management UI writes". No code writes `state = 'disabled'`: the only `UPDATE workflow.flows` statements are `repo.rs:552`, `:664` and `:739` (`state = 'live'`). New text: the column is honoured, but nothing writes it yet; the runtime switch is per trigger (`workflow.disabled_triggers`).
- `crates/egret-workspace/src/features.rs:312-315` tells operators to stop webhook traffic with `state = 'disabled'` on the flow, a switch nobody can set. New text: the trigger's runtime switch or `disabled: true` in the YAML. `webhook_routes.rs:33-36` gains the trigger switch beside the YAML.

## 4. Frontend

Everything lives inside W7a's lazy Workflows area (FE-ROUTE-2). New modules: `components/workflows/triggers/` (`TriggersTable`, `TriggerDetailsDrawer`, `EnableTriggerDialog`, `triggersModel.ts`) and `routes/workflows/TriggersList.tsx`. The client is regenerated with `npm run gen:api` (FE-DATA-1); keys in `routes/workflows/queryKeys.ts` (FE-DATA-2).

### 4.1 The flow's Triggers tab

`FLOW_TABS` becomes `["edit", "revisions", "executions", "triggers"]` (`FlowDetail.tsx:52-53`); the URL is `flows/:id/triggers` (FE-PAGE-2).

- **Columns** (`ui/table`, FE-PAGE-4): selection, ID, Type (kind icon and the short type name), Next run, Last tick (`evaluated_at`, labelled "Last evaluated" with a tooltip that it is the tick processed, not the wall-clock fire), Last execution (state chip, relative time, link), Status, Enabled (switch), Details.
- **Status** says why a trigger does not fire, in words: "Disabled in YAML", "Switched off by {disabled_by_name} {time}", "Off the queue: {last_error}", "Not listening: {reason}", "Not run by egret" (Leaf), "Key does not render". Never by colour alone (FE-A11Y).
- **The switch.**
  - Disabled with its reason (FE-ACTION-4): "Disabled in the flow's YAML", "Not run by egret", "You need workflow:manage", "The flow is in the trash".
  - Switching off sends the PUT at once.
  - Switching on a `Schedule` whose next date has passed opens `EnableTriggerDialog` (§4.3); any other trigger switches on at once.
  - The switch moves only when the server answers (FE-ACTION-3: a refusal shows the server's message and leaves the switch where it was).
- **Bulk:** "Enable" and "Disable" act on the selection through the bulk route; "Enable" with a `Schedule` in the selection opens the dialog in its bulk form; the result names the triggers that were refused and why.
- **Empty state** (FE-STATE-3): "This flow has no triggers" with a link to the editor.
- **Banners** above the table, each with what to do:
  - `scheduler_enabled` false and the flow has a `Schedule` or `Flow` trigger: "The scheduler is off on this server, so schedules and flow triggers do not fire. An administrator sets `workflow.schedulerEnabled`."
  - `worker_enabled` false: "The worker is off, so triggered executions are created but do not run."
  - `webhooks_enabled` false and the flow has a `Webhook`: "Webhooks are off on this server; their URLs answer 404."
  - The flow disabled (YAML or state): "This flow is disabled, so none of its triggers fire."
  - No banner for a draft: a draft fires (`process.rs:908-911`; `flow_is_on`, `scheduler/start.rs:51-67`, excludes only `state = 'disabled'`).
- **Refresh:** the tab refetches every 30 seconds while visible and after each switch (FE-ACTION-2).

### 4.2 The details drawer

Opened from Details or a row's ID; `?trigger=<id>` in the URL so it can be shared (FE-PAGE-2).

- **Definition:** the trigger's YAML block, read-only, with a link "Edit in the editor" that opens Code at that block.
- **Schedule:** cron, timezone, next run in the trigger's timezone and the viewer's, last tick, last error, missed ticks when switched off.
- **Flow trigger:** the states heard; for `dependsOn`, the mode, the window (kind, start and end, time left), and each condition with met or not met. "No window is open" when there is none.
- **Webhook** (default decision, 2026-10-14): the **path** `url_path` (`/workflows/v1/{ws}/executions/webhook/{namespace}/{flow}/{key}`), masked with Reveal and Copy (the copy button says "Copied" in an `aria-live` region), and a note that it is served by the egret backend at whatever host the operator exposes for webhooks. If `webhooks_enabled` is false, the same "Webhooks are off on this server" warning shows. W7e does **not** build the URL from `window.location.origin` and adds **no** routing: nothing at the browser's origin sends `/workflows/` to the backend (Helm ingress, `nginx-prod.conf`, Vite proxy), so such a URL would answer the SPA's `index.html` with 200 and never fire, and adding the routing would expose an anonymous surface whose review says "not approved for exposure" (`webhook_routes.rs:60-64`). Exposing webhooks through ingress is a follow-up that needs its own security review.
- **Last execution:** a link to it.

### 4.3 The enable dialog

For one trigger or for the `Schedule`s of a bulk selection.

- **Title:** "Enable trigger" / "Enable {n} triggers".
- **Choices** (radio group, FE-FORM-2):
  - "Skip missed runs; start at the next tick ({time})" (default, Kestra's `SKIP`).
  - "Recover missed runs ({n} missed)". Its description follows the trigger's `recoverMissedSchedules`: "runs every missed tick, one per scheduler pass" (`ALL`), "runs only the latest missed tick" (`LAST`), "skips them: this trigger's `recoverMissedSchedules` is NONE" (`NONE`).
- **Buttons:** Cancel, Enable.

### 4.4 The global Triggers page

`/explore/:workspace/workflows/triggers` (FE-ROUTE-3), a `ListPageShell` (FE-PAGE-1). Sidebar: "Triggers" after "Namespaces" in the Workflows group (`components/layout/Sidebar.tsx:55-59`), feature `workflows`, and the route beside the others (`routes/workflows/WorkflowsRoutes.tsx:64-82`).

- **Filters in the URL** (FE-PAGE-2): namespace prefix (a picker over W7d's namespace list, FE-FORM-3), flow, kind, Enabled / Disabled, search, page.
- **Columns:** the flow tab's, plus Namespace and Flow (links), minus Type's long name.
- **Bulk** enable and disable as on the flow tab.
- **States** (FE-STATE-1/2/3): skeleton, error first, "No triggers match these filters" with Clear filters. A 422 `triggers_too_large` shows its message and focuses the namespace filter. `unparsable > 0` shows "{n} flows could not be read; their triggers are not listed".
- **The same banners** for the server flags.

### 4.5 Permissions

A viewer with `workflow:read` and not `workflow:manage` sees everything; the switches, selection and bulk buttons are disabled with "You need workflow:manage" (FE-ACTION-4).

### 4.6 Chunks

The new modules load only inside the Workflows chunk; `test:bundle:workflows` (`frontend/package.json:14`) gains markers for `TriggersList` and `FlowTriggersTab`.

## 5. Errors

- 404: an unknown flow or trigger id ("This trigger no longer exists in the flow; reload").
- 409 `trigger_disabled_in_source`, `trigger_not_supported`, `recover_missed_unknown`: the server's sentence.
- 400: an unparseable source, a bulk list over 100.
- 422 `triggers_too_large`: §4.4.
- Network failures say what happened and what to do (FE-COPY-2).

## 6. Tests

**Rust** (`tests/` of `egret-workflow`, each with its own database):
- Switch table and `sync`:
  - the effective copy on both tables, for each of the three causes;
  - a valid save dropping an id drops its switch row; a source that no longer parses (written by SQL, as a parser change would leave it) keeps it, and `reconcile` keeps it; a soft delete drops it; another flow's rows are untouched.
- Enforcement:
  - a switched-off schedule is not claimed and keeps its dates;
  - a switched-off listener is not selected and no outbox event is written for it alone;
  - the webhook answers 409 with the YAML's sentence; a wrong key still answers the shared 404 body (`the_four_absences_are_indistinguishable` extended with a switched-off trigger).
- Re-enable: `recover_missed` false moves the date to the next tick; true with absent, `ALL`, `NONE`, `LAST` each give their outcome; an off-queue row stays off; a later save with the same cron keeps the moved date; one with a new cron re-initialises.
- Refusals: YAML-disabled enable (409), Leaf (409), unknown id (404), another workspace's flow (404, not 403), trashed flow (404), `workflow:read` only (403).
- Concurrency: a switch-off that meets a pass holding the row waits, and the pass's fire is the last (the shape of `a_save_that_meets_a_claimed_trigger_waits_and_keeps_the_advance`); a save and a switch on one flow end with the switch's value.
- Reads: every field of §3.5 per kind; conditions labelled and `met`; an expired window reported as none; the rendered webhook path; `missed_ticks` capped; the global list's filters, paging, 2,000-flow 422 and `unparsable`; the last-execution plan uses the index.
- Bulk: 100 accepted, 101 refused, mixed results.
- Validation: `duplicate_trigger_id`.
- Mutations: drop the switch term from `sync`'s effective value; drop the webhook lookup; drop the flow lock in the route (the concurrency test must die).

**Vitest:** the tab's columns and status text per state; the switch's disabled reasons; the dialog's choices and the body each sends; bulk results; the drawer per kind and the copy button; the global page's URL filters; the read-only viewer.

**Playwright** (`frontend/tests/e2e/workflows-triggers.spec.ts`), on a private stack with the scheduler, worker and webhooks on, never the shared dev stack:
1. A flow with a `* * * * *` schedule, a webhook and a `dependsOn` Flow trigger on two upstream flows. The Triggers tab shows each with its next run and the webhook URL; light and dark mode.
2. Switch the schedule off; wait past two ticks; no execution is created. Switch it on with "Skip": the next run is in the future and no missed execution appears. Repeat with "Recover": the missed ticks run.
3. Switch the webhook off: a `curl` of its URL answers 409; on again: 200.
4. Run one upstream flow: the drawer shows the window and one condition met; run the other: the downstream flow runs.
5. The global page: filter by namespace and kind in the URL, bulk-disable two triggers, reload, see them off.
6. A viewer without `workflow:manage` sees disabled switches with the reason.
7. A trigger with `disabled: true` in the YAML: the switch is disabled with "Disabled in the flow's YAML".

## 7. Comparing with Kestra

The last task of the plan runs §6's scenario 1-3 and 5 on the local Kestra image (`kestra/kestra` `28f59bf175e1`, `server local`, a throwaway container), and checks in particular: the runtime switch against a webhook and against a Flow trigger, enabling a YAML-disabled trigger through the API, and both dialog choices. Each row of §2 it measures is relabelled **observed**; a difference it finds is added there (FE-REF-2).

## 8. Rules this package must meet

- The frontend rulebook, every rule.
- No new dependencies.
- Regenerate the client only with `npm run gen:api`.
- Migration prose is append-only: 0156 and 0157 are not edited (§3.1).

## 9. W7f (backfill) is not foreclosed

- The trigger entry and the list row are objects; W7f adds a `backfill` field to both.
- The switch does not touch backfill state. W7f must keep Kestra's rule that re-enabling leaves a running backfill untouched (`TriggerEventHandler.java:286-289`): "skip missed" moves `next_evaluation_date` only, and W7f's backfill progress will live in its own columns.
- The flow tab reserves no column for backfill; W7f adds one beside Enabled.

## 10. Follow-ups

- `recoverMissedSchedules` is honoured only on re-enable. A trigger switched off in the YAML and back on still replays every missed tick, and egret has no startup recovery to apply it to.
- Trigger logs: schedule errors exist only as the latest `last_error`, Flow-trigger fire failures only in server logs (`process.rs:604-683`). A per-trigger log would let the drawer show history.
- A "reset schedule" action, if users want one beyond re-enable with "skip".
- "Select all matching" bulk and CSV export on the global page.
- `stopAfter` is parsed and read by nothing (`models/trigger.rs:51-60`); Kestra disables the trigger through the same runtime state W7e adds, so W7e's table is where it would write.
- The global list parses up to 2,000 drafts per request; a stored trigger index would remove the bound.
