# W7f — Workflows UI, part 6: schedule-trigger backfill

**Status:** design, 2026-10-15. Nothing is built. Every claim about egret's code was checked in the worktree `egret-w7f` at `f73c13647` (main, migrations up to 0189) and is cited `path:line` from `crates/egret-workflow/src/` unless another root is given. Kestra is the checkout at `data-portal/kestra`, `96893b589`; its paths are relative to it.

**Depends on:**
- W7e (#672, `f73c13647`): the trigger routes, `TriggerEntryDto`, the lock order, the Triggers tab, the global Triggers page, `useTriggerSwitch`, the enable dialog. Its spec is `docs/en/specs/2026-10-14-workflow-w7e-triggers-design.md`; §9 there reserved this package's place.
- W12 (`workflow.triggers`, `scheduler::sync`, `fire_due`) and W6 Task 5 (the pure backfill model in `trigger/`, ported and never wired).
- W7a (the Run form, `frontend/src/components/workflows/run/`), W7b (the execution overview).

The frontend rulebook `docs/en/frontend-rules.md` binds every frontend line.

**Programme spec:** `docs/{en,vi}/specs/2026-09-01-workflow-programme-design.md` §8 (Kestra's UI as reference) and §9 row W7.

**Decided with the user, 2026-10-15:**
- **A backfill over a period that already ran re-runs those dates, as Kestra does.** A backfill tick for a date that already has an execution creates a second one. The unique index `workflow_executions_one_per_schedule_tick` (0156) therefore stops covering backfill executions, and a backfill execution is identified in the database (D2). One execution per `(trigger, backfill, tick)` still holds.
- **Backfill inputs and labels are in scope.** Inputs override the schedule's `inputs:` key by key (Kestra's `putAll`), under the same `refuse_templated` rule. Labels need a per-execution store, which egret does not have today (D3).

**Default decisions (2026-10-15),** taken under the brief's rule "follow Kestra unless egret has a reason; keep correctness tight". Each is explained where it is used:
- D1. A backfill lives in **a table of its own**, `workflow.trigger_backfills`, keyed `(flow_id, trigger_id)`, not in columns of `workflow.triggers`. `sync` deletes every `workflow.triggers` row of a flow whose source does not parse; a running backfill must survive that, as W7e's switch rows do (§3.1).
- D2. A backfill execution carries **`workflow.executions.backfill_id`**, the id of the backfill that fired it. The per-tick unique index is rebuilt with `AND backfill_id IS NULL`, and a second unique index on `(backfill_id, scheduled_date)` keeps one execution per backfill tick. `execution_trigger` is unchanged, so the oracle equality test still holds (§3.2).
- D3. **`workflow.executions.labels`** (jsonb, a list of `{key, value}`) holds the labels an execution was started with **beyond its flow's own `labels:`**. The render surface and the Flow-trigger source merge flow labels, then these, the later key winning, as Kestra does. Replays and loop iterations copy them (§3.3).
- D4. The window is **inclusive at both ends**: `[start, end]`. Kestra's create excludes a tick that falls exactly on `start` (§2, §3.4).
- D5. Creating a backfill moves `next_evaluation_date` to the window's first tick and stores the schedule's position before it, `previous_next_evaluation_date`, **once**. Pause and resume never touch it (Kestra overwrites it on every pause and resume) (§3.4).
- D6. When a backfill ends, by completion or by delete, the regular schedule resumes from where it stood, and the regular ticks that fell due during the backfill are handled by the trigger's own `recoverMissedSchedules` (absent = `ALL`). Kestra skips them on completion and restores the old date on delete (§3.6).
- D7. A paused backfill is **not claimed**: the claim excludes it, so a paused trigger costs nothing per pass. Pause and resume write only the backfill row (§3.5).
- D8. The server validates what Kestra checks only in its UI, and more: `start < end`, `end <= now()`, `start` at most ten years back, at least one tick and at most 10,000 in the window, only a `Schedule`, only a trigger that is on and on the queue, inputs and labels checked. A second create is 409 `backfill_exists` (Kestra replaces silently); pause, resume or delete with no backfill is 404 `no_backfill` (Kestra answers 200 and does nothing) (§3.4, §3.5).
- D9. A switched-off or YAML-disabled trigger keeps its backfill frozen, as in Kestra. Creating one on a disabled trigger is refused, 409 `trigger_disabled` (§3.4).
- D10. `sync`: when a schedule with a backfill is re-initialised (its `spec_key` changed, it was off the queue, or its row was re-created), its next date is **re-derived from the backfill**, without losing the current tick. A removed trigger id, a kind change and a soft delete delete the backfill; a source that does not parse keeps it (§3.7).
- D11. Re-enabling a switched-off trigger during a backfill keeps its dates, so the backfill resumes at the tick it stopped on; the dialog's choice applies to the regular schedule's resume position. This replaces W7e §9's sentence, which misread Kestra (§2, §3.8).
- D12. During a backfill, `missed_ticks` counts the **regular** ticks that are waiting (from `previous_next_evaluation_date` through now), not the window's (§3.9).
- D13. One window tick per trigger per pass, as for every schedule today (§3.6).
- D14. Routes under W7e's: create, pause/resume and delete per trigger, `workflow:manage`; bulk pause/resume and delete by selection, at most 100, synchronous, a result per entry. No bulk create (as Kestra). An audit event per change (§3.10).
- D15. `TriggerEntryDto` gains `backfill`, `can_backfill` and `backfill_refusal`; the progress is Kestra's time formula, computed by the server, plus exact tick counts (§3.9).
- D16. The create form is a lazy chunk; dates use the native `datetime-local` control with `max` set to now; the inputs reuse the Run form's model and controls (§4.3).
- D17. The migration is **0192** (§3.1). Default decision (2026-10-15): 0190 and 0191 are held by the `egret-tagdef` worktree (branch `fix/tag-policies-service-def`).
- D18 (P1). The execution **list** carries the execution's own `labels` plus `backfill_id`; the **detail** (`ExecutionDto`) carries `effective_labels`, the merge (§3.3). Default decision (2026-10-15).
- D19 (P2). A cron change that would give a backfill more than 10,000 ticks **ends it**: `sync` resumes the regular schedule and writes `last_error` (§3.7). Default decision (2026-10-15).
- D20 (P3). A re-snap that finds no tick **keeps** the stored `previous_next_evaluation_date` (§3.7). Default decision (2026-10-15).
- D21 (P4). `ticks_done` is advanced with `LEAST(ticks_done + 1, total_ticks)`, so a CHECK violation cannot end a scheduler pass (§3.6). Default decision (2026-10-15).
- D22 (P5). The pause-between-claim-and-read race is staged by a test-only hook in `scheduler::fire` (§6). Default decision (2026-10-15).
- D23 (P6). The dialog's bundle flag is a new, stricter `backfill` flag, not `editor` (§4.3). Default decision (2026-10-15).
- D24 (P7). `deserialize_labels` is widened to `pub(crate)` (§3.4). Default decision (2026-10-15).
- D25. A backfill tick whose start is refused (`Started::Refused`) **ends the backfill** in the same transaction instead of taking the trigger off the queue (§3.6). Default decision (2026-10-15, review of Task 4).

## 1. Why

`Backfill` is ported and pure (`trigger/schedule.rs:1634-1652`, `trigger/mod.rs:270-310`), and `fire_one` hard-codes `backfill: None` (`scheduler/fire.rs:226-233`). No route creates one: W7e's routes are the two reads and the two switches (`triggers/routes.rs:5-10`). A Kestra user who adds a schedule to an existing flow, or who fixes a flow that failed for a week, expects "Backfill executions" on the trigger: run the schedule for a past window, with chosen inputs and labels, and watch it, pause it or stop it.

## 2. Divergences from Kestra

**Read in source** (the checkout above) unless a cell says otherwise; §7's comparison relabels each row it measures as **observed**.

| Kestra | egret | Why |
|---|---|---|
| **Read.** Create, pause, resume and delete validate nothing on the server: no `start < end`, no end-in-future check, no "already has one" (a create replaces the running backfill), no check that the trigger is a `Schedule` (`TriggerController.java:222-314`; `TriggerEventHandler.java:154-183`). The UI blocks future dates and `start > end` (`ui/src/components/flows/FlowTriggers.vue:262-277`), required inputs and half-filled labels (`:509-540`) | The server refuses each (D8, §3.4), with a code; a second create is 409 `backfill_exists` | An API caller must not get a 200 for a request the UI would never send, and a silent replace loses a window in progress |
| **Read.** Pause, resume or delete with no backfill does nothing and answers 200 (`TriggerEventHandler.java:193, 214`) | 404 `no_backfill` | Says what happened |
| **Read.** The window excludes a tick exactly at `start`: create recomputes the next date from `currentDate = start` with `executionTime.nextExecution`, which is strictly after (`TriggerEventHandler.java:177-179`, `Schedule.java:247-249, 273, 466-468`). It includes a tick at `end` (`TriggerState.java:338-342`: cleared only when `next > end`) | `[start, end]`, both ends included (D4) | "Backfill 1 to 7 October" on a midnight cron should run 7 times, not 6 |
| **Read.** Creating stores `previousNextExecutionDate = nextEvaluationDate`, and so does every pause and resume, because both rebuild the backfill through the same `TriggerState.backfill(...)` (`TriggerState.java:229-239`; `TriggerEventHandler.java:211-220`). After a pause, delete restores a **window date** (`:190-203`), and the scheduler then replays every regular tick from that date to now. With no stored date, delete would throw (`TriggerState.java:204-206`) | Stored once, at create (D5); delete resumes the regular schedule as §3.6 says | The quirk replays ticks nobody asked for |
| **Read.** On natural completion the next date is the next tick after **now** (`Schedule.java:278-280`): the regular ticks that fell due during the backfill are skipped. On delete, the old next date is restored, so they are replayed. The two ends disagree | Both ends resume from the regular schedule's own position and apply the trigger's `recoverMissedSchedules` (absent = `ALL`) to the regular ticks that fell due meanwhile (D6) | A backfill should not silently cost the regular runs, and how it ends should not decide whether they run |
| **Read.** A paused backfill is evaluated every pass and skipped (`Schedule.java:342-345`); its next date stays in the past | A paused backfill is not claimed (D7) | No work and no row write per paused trigger per second |
| **Read.** Re-enabling a switched-off trigger with a backfill: `ALL` keeps the state (`TriggerEventHandler.java:263-265`), `LAST` returns it untouched (`:283-288`), `NONE` and the default "skip" recompute the next date from the backfill's `currentDate` with the stale date dropped (`:268-270`, `:314-317`). The recompute is strictly after `currentDate`, which is the tick **not yet fired**, so a skip-enable loses one window tick. **W7e spec §9 said "skip missed moves `next_evaluation_date` only", citing `:286-289`: that is the `LAST` branch, and the sentence is wrong for skip** | Dates kept for every choice; the backfill resumes at the tick it stopped on; the choice applies to the regular schedule's resume position (D11) | No window tick is lost, and the user's choice still means what the dialog says |
| **Read.** A trigger definition update recomputes the next date from the context, backfill included (`TriggerEventHandler.java:500-513`), so the same strictly-after rule skips the current window tick | `sync` re-derives from the backfill at or after its current tick (D10) | Same reason |
| **Read.** A disabled trigger (runtime or YAML) is not fetched, so its backfill is frozen (`DefaultSchedulableTriggerFetcher.java:69, 104`). Create is accepted | Frozen too. Create on a disabled trigger: 409 `trigger_disabled` | A backfill that cannot move should not be started |
| **Read.** No marker on a backfill execution: the trigger record is the live one's (`SchedulableExecutionFactory.java:35`), and no system label names a backfill (`core/.../models/Label.java:21-51`) | `execution_trigger` is the live one's too (the oracle shape), and the execution carries `backfill_id` in a column (D2). The overview shows "Backfill" | egret needs the marker for its unique indexes; showing it is free |
| **Read.** Labels: the flow's (system labels removed), then the trigger's and the backfill's (rendered), then `system.from` and `system.correlationId`; deduplicated by key, the **last** value winning in the first key's position (`SchedulableExecutionFactory.java:37-44, 94-109`; `LabelService.java:29-51`; `Execution.java:209-217`; `Label.java:70-93`). Backfill label values are rendered (`SchedulableExecutionFactory.java:102`) | The flow's labels, then the backfill's, the later key winning (D3). A templated backfill label value is refused at create. The trigger's own `labels:` and `system.from` are still not applied (§10) | egret has no render context at this seam, as for `Schedule.inputs` (`scheduler/fire.rs:427-442`) |
| **Read.** Inputs: the trigger's rendered `inputs`, overlaid by the backfill's rendered `inputs` (`SchedulableExecutionFactory.java:76-88`) | The same overlay, unrendered: a templated value in the merged map is refused at create (§3.4) and, as today, consumes the tick at fire time (`fire.rs:256-258`) | Same reason |
| **Read.** Bulk pause, resume and delete by triggers or by query, asynchronous, `202` with an operation id, unknown triggers dropped (`TriggerController.java:248-337`). No bulk create | By selection only, at most 100, synchronous, a result per entry (D14); no bulk create | W7e D10's reasons |
| **Read.** The UI: a Backfill column with "Backfill executions" or a running/paused tag (`FlowTriggers.vue:120-140`); an always-expanded sub-row with "start → end · N%", a striped bar (amber when paused), pause/resume and stop (`BackfillBanner.vue`); progress = `(currentDate − start) / (end − start)`, rounded, clamped, 100 when the window is empty (`BackfillBanner.vue:75-82`); the form embeds the Run form (inputs, labels) | The same column, sub-row and formula; the bar is not animated (FE-A11Y-5) and the state is in words (FE-A11Y-4); "N of M ticks" beside the percentage; delete asks first (FE-ACTION-1) | The rulebook |
| **Read.** No limit on a window's size | At most 10,000 ticks, and `start` at most ten years back (D8) | A window drains one tick per pass (about 1 s each), and a mistyped year (`0026`) is otherwise a 2,000-year window |

## 3. Backend

### 3.1 The backfill table (D1, D17)

Migration `0192_workflow_trigger_backfills.sql` (default decision, 2026-10-15). Main ends at `0189`; the unmerged branch `feat/view-governance` holds `0186`-`0188` (checked with `git ls-tree` on every local and remote ref, 2026-10-15, and on the disk of every worktree). `0190` and `0191` are taken by the `egret-tagdef` worktree (branch `fix/tag-policies-service-def`, re-checked 2026-10-15 during Task 2), so this package uses `0192`, which collides with none of them in any merge order; re-check every ref and worktree before merging, since the number space is shared.

```sql
CREATE TABLE workflow.trigger_backfills (
  flow_id                       uuid        NOT NULL REFERENCES workflow.flows (id) ON DELETE CASCADE,
  -- The `Schedule` trigger's `id:` in the YAML.
  trigger_id                    text        NOT NULL,
  -- Copied from the flow for filtering; not an authority.
  workspace_id                  uuid        NOT NULL,
  -- This backfill's identity, written on every execution it fires
  -- (workflow.executions.backfill_id). A new backfill over the same window
  -- has a new id, so it re-runs those dates (user decision, 2026-10-15).
  id                            uuid        NOT NULL DEFAULT gen_random_uuid() UNIQUE,
  window_start                  timestamptz NOT NULL,
  window_end                    timestamptz NOT NULL,
  -- The next window tick to fire: equal to workflow.triggers.next_evaluation_date
  -- while the trigger row is on the queue. Not `current_date`, a reserved word.
  current_tick                  timestamptz NOT NULL,
  paused                        boolean     NOT NULL DEFAULT false,
  paused_by                     uuid        REFERENCES users (id) ON DELETE SET NULL,
  paused_at                     timestamptz,
  -- Overlaid on the schedule's `inputs:` key by key.
  inputs                        jsonb       NOT NULL DEFAULT '{}'::jsonb,
  -- [{key, value}], no `system.` key, in the order given.
  labels                        jsonb       NOT NULL DEFAULT '[]'::jsonb,
  -- The regular schedule's next date when the backfill was created. Written
  -- at create and re-snapped by `sync` on a cron change; never by pause/resume.
  previous_next_evaluation_date timestamptz NOT NULL,
  total_ticks                   integer     NOT NULL,
  ticks_done                    integer     NOT NULL DEFAULT 0,
  created_by                    uuid        REFERENCES users (id) ON DELETE SET NULL,
  created_at                    timestamptz NOT NULL DEFAULT now(),
  updated_at                    timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (flow_id, trigger_id),
  CHECK (window_start < window_end),
  CHECK (current_tick >= window_start),
  -- The row is deleted once the next tick passes window_end (§3.6, §3.7),
  -- and create refuses a window with no tick (§3.4).
  CONSTRAINT workflow_trigger_backfills_tick_within_window CHECK (current_tick <= window_end),
  CHECK (jsonb_typeof(inputs) = 'object'),
  CHECK (jsonb_typeof(labels) = 'array'),
  CHECK (paused = (paused_at IS NOT NULL)),
  CHECK (total_ticks BETWEEN 1 AND 10000 AND ticks_done BETWEEN 0 AND total_ticks)
);
```

- **Why not columns on `workflow.triggers`** (the brief's first proposal): `sync` deletes every `workflow.triggers` row of a flow whose source does not parse (`scheduler/sync.rs:113-119`, `keep` is empty when `parsed` is `None`, `:85-89, 93-98`). The API cannot store such a source (`parse_draft`, W7e §3.4), but a parser change can make a stored one fail, and `reconcile` runs `sync` on every flow at start. W7e put the switch in its own table for exactly this (`migrations/0189_workflow_disabled_triggers.sql:5-11`); a backfill in progress is the same kind of user state. No foreign key to `workflow.triggers`, for the same reason.
- **`end` and `current_date` are reserved words** in SQL, hence `window_start`, `window_end` and `current_tick`.
- **`ON DELETE CASCADE`** fires on a hard delete only; a soft delete is `sync`'s (§3.7).

### 3.2 Identifying a backfill execution (D2)

```sql
ALTER TABLE workflow.executions ADD COLUMN backfill_id uuid;
-- A backfill execution is a schedule fire: 0156's CHECK ties scheduled_date to it.
ALTER TABLE workflow.executions ADD CONSTRAINT workflow_executions_backfill_is_a_schedule_fire
  CHECK (backfill_id IS NULL OR scheduled_date IS NOT NULL);
DROP INDEX workflow.workflow_executions_one_per_schedule_tick;
CREATE UNIQUE INDEX workflow_executions_one_per_schedule_tick
  ON workflow.executions (flow_id, (execution_trigger->>'id'), scheduled_date)
  WHERE scheduled_date IS NOT NULL AND backfill_id IS NULL;
CREATE UNIQUE INDEX workflow_executions_one_per_backfill_tick
  ON workflow.executions (backfill_id, scheduled_date)
  WHERE backfill_id IS NOT NULL;
```

- **Today a backfill tick for a date that already ran would be refused**: 0156's index is `(flow_id, execution_trigger->>'id', scheduled_date) WHERE scheduled_date IS NOT NULL` (`migrations/0156_workflow_triggers.sql:49-51`); the violation is classified `AlreadyFired` (`scheduler/start.rs:96-116`) and the tick consumed with a warning (`scheduler/fire.rs:265-273`). The user's decision needs the backfill outside it.
- **Why a column and not a key in `execution_trigger`:** the record must stay the one Kestra stores, which `schedule_trigger_record_matches_the_captured_execution` pins against a captured artifact (`scheduler/fire.rs:475-510`). Kestra itself adds no marker (§2).
- **Why the backfill's id and not a boolean:** `(backfill_id, scheduled_date)` is exactly "one execution per (trigger, backfill, tick)". A boolean would make a second backfill over the same window collide with the first.
- **No foreign key** to `workflow.trigger_backfills`: the row is deleted when the backfill ends, and its executions stay.
- **Idempotency of a backfill tick.** It is a schedule fire like any other, so W12's three statements hold unchanged (`scheduler/mod.rs` "## 1." to "## 3."): the claim locks the trigger row `FOR UPDATE SKIP LOCKED` (`fire.rs:145-166`), so two replicas never evaluate one trigger at once; the execution's insert and the advance commit in one transaction (`fire.rs:289-302`), so a retried pass either sees the advance or redoes the whole tick; and `workflow_executions_one_per_backfill_tick` is the backstop that `classify` maps to `AlreadyFired` as it maps the per-tick index (a new constant beside `ONE_PER_TICK`, `start.rs:72`).
- **The migration locks `workflow.executions` ACCESS EXCLUSIVE from its first `ALTER TABLE` until it commits**, blocking reads as well as writes (execution lists, workers claiming, the scheduler): `ADD COLUMN`, `ADD CONSTRAINT` and `DROP INDEX` each take that lock, held to the end of the transaction. The window is about four full scans of the table: the two CHECK validations, and the two partial-index builds, each of which reads the whole heap. The rebuild is not `CONCURRENTLY`, since sqlx runs the file in one transaction, so there is no moment without either index. (0189's note, `0189:43-48`, describes a narrower window for its own index; it does not apply here.)
- **0156's prose is not edited** (migration prose is append-only); 0192 states the new predicate. The doc of `scheduler/mod.rs` statement 2 ("the unique index is the backstop") names both indexes.

### 3.3 Execution labels (D3)

```sql
-- The labels this execution was started with beyond its flow's own `labels:`
-- ([{key, value}], Kestra's list shape). NULL: none. The flow's labels are
-- read from `source`, which is the execution's frozen copy.
ALTER TABLE workflow.executions ADD COLUMN labels jsonb
  CONSTRAINT workflow_executions_labels_are_a_list
  CHECK (labels IS NULL OR jsonb_typeof(labels) = 'array');
```

**Today** an execution has no labels column (`migrations/0134_workflow_executions.sql` and every later `ALTER`). The `labels` a run renders are the flow's own `labels:` plus `system.correlationId` (`worker/render.rs:247-252`, `:504`); the Flow-trigger processor reads the **source flow's** declared labels for `dependsOn` label conditions and `executionLabels` (`flow_trigger/process.rs:795-797, 1080-1089, 1348`); and no route or page shows or filters execution labels (`ExecutionListItemDto`, `routes.rs:1417-1464`, has none; the frontend has no label filter on executions). The trigger's own `labels:` are parsed (`models/trigger.rs:51`) and applied to nothing.

- **The merge, in one function** (`labels::execution_labels(flow: &[Label], own: &[Label]) -> Vec<Label>`): the flow's labels, then the execution's, deduplicated by key, the **last** value kept in the **first** key's position, an empty key or value dropped. That is Kestra's `Execution.newExecution` + `Label.deduplicate` (§2).
- **Render:** `Surface::load` (`worker/render.rs:168-178`) also selects `labels`; the map at `:247-252` is built from the merge. The `system` key is still inserted last (`:504`), as today.
- **Flow-trigger source:** `load_source` (`flow_trigger/process.rs:778`) reads the column and merges the same way, so a downstream `dependsOn` condition with `labels:` and `{{ trigger.executionLabels }}` see a backfill's labels, as in Kestra. W13 spec §5.3 step 4's recorded divergence ("egret has no execution labels") narrows to executions without own labels.
- **Copied** where Kestra copies an execution's labels: the replay route (`routes.rs:2941-2959`), a `CREATE_NEW_EXECUTION` retry (`worker/replay.rs:243`, `Execution.childExecution`, `Execution.java:416`) and a loop iteration (`worker/spawn.rs:393`, `Execution.loopExecution`, `Execution.java:448`). **Not** a subflow child: `inheritLabels` is not honoured today (`worker/subflow.rs:79-82`), unchanged. `backfill_id` is never copied (a replay has no `scheduled_date`, and the CHECK would refuse it).
- **`NewExecution`** (`worker/claim.rs:2605-2654`) gains `labels: Option<serde_json::Value>` and `backfill_id: Option<Uuid>`; every other construction passes `None` except the replay route, which passes the source's `labels`. "Every other construction" means **all construction sites, src and tests, listed in the plan** (the sites named here, `routes.rs:2668, 2944`, `worker/claim.rs:2572` and `flow_trigger/process.rs:996`, are only the source ones).
- **Shown (D18):** `ExecutionListItemDto` gains `labels` (the column as stored, the execution's own labels beyond its flow's) and `backfill_id`. The list projection omits `source` on purpose (`repo.rs:955-968`, `execution_cols!`), and the flow's labels live only in `source`, so a merged list would select and parse a YAML document per row. `ExecutionDto` (the detail, which has `source`) gains `effective_labels`, the merge above. The execution overview shows `effective_labels` as `key: value` chips and, in its trigger block (`frontend/src/routes/workflows/ExecutionOverview.tsx:139-160`), "Backfill" beside the trigger id when `backfill_id` is set. Filtering executions by label is a follow-up (§10).

### 3.4 Creating a backfill

`POST /api/ws/{workspace}/workflows/flows/{id}/triggers/{trigger_id}/backfill`, `workflow:manage`, body cap 64 KiB.

```json
{ "start": "2026-10-01T00:00:00Z", "end": "2026-10-07T00:00:00Z",
  "inputs": { "region": "eu" }, "labels": [{ "key": "reason", "value": "replay outage" }] }
```

`end` is optional (absent = the transaction's `now()`, Kestra's default, `TriggerState.java:233`); `inputs` and `labels` are optional. `labels` accepts Kestra's list or map form through the one deserializer, `models/flow.rs:375`'s `deserialize_labels`, widened from `pub(super)` to `pub(crate)` for this (D24).

**In one transaction, in W7e's lock order** (`triggers/switch.rs:5-22`):
1. The flow row `FOR NO KEY UPDATE`, with the workspace predicate (`switch.rs:105-117`). Not found or trashed: 404 `not_found`.
2. Parse; the first trigger with this id (`switch.rs:120-126`). Unparsable: 400 `source_does_not_parse`. No such id: 404 `not_found`. Not a `Schedule`: 409 `backfill_not_supported`.
3. Effective-disabled (the flow's YAML, the trigger's YAML, the runtime switch, or `state = 'disabled'`, as the entry computes it, `triggers/entry.rs:83-87`): 409 `trigger_disabled` (D9).
4. The `workflow.triggers` row `FOR UPDATE`. Absent, or `next_evaluation_date IS NULL` (off the queue; `ScheduleTrigger::resolve` refuses it, or the cron has no future tick): 409 `trigger_off_queue`, with `last_error`.
5. Validate (codes in §5), with the transaction's `now()` and the resolved schedule:
   - `start < end` (`backfill_window_invalid`); `end <= now` (`backfill_end_in_future`); `start >= now − 10 years` (`backfill_start_too_early`, default decision: a two-digit year typed into a date control is the realistic cause).
   - The window's ticks, counted with `ScheduleTrigger::ticks_through(start, end, 10_000)` (`trigger/schedule.rs:1429-1446`, inclusive at both ends): zero is `backfill_no_ticks`; more than 10,000 is `backfill_too_many_ticks` (default decision: at the default one-second pass, `crates/egret-config/src/lib.rs:691-696, 1396-1398`, 10,000 ticks drain in under three hours; a minutely cron covers a week). The count is stored as `total_ticks`.
   - **Inputs:** a JSON object (`start_inputs`'s rule, `routes.rs:2306-2320`). No key may name a `FILE` input (`backfill_inputs_invalid`: a backfill has no upload, and a stored URI belongs to another execution). The **merged** map (schedule `inputs:` overlaid by these) must pass `refuse_templated` (`fire.rs:430-442`), so a templated schedule input must be overridden. Every declared input with `required` not false and no `defaults` must have a value in the merged map (`models/input.rs:54-62`), which is what Kestra's UI checks (`FlowTriggers.vue:518-530`).
   - **Labels:** no key starting `system.` in any letter case (Kestra's `@NoSystemLabelValidation`), no empty or whitespace-only key, no empty value, no key given twice in the request, no template syntax in a value, at most 50 labels, a key at most 128 characters (Unicode scalar values, not bytes) and a value at most 1,024 (`backfill_labels_invalid`). The case-insensitive prefix, the whitespace-only key and the duplicate-key refusal are default decisions (2026-10-15): they keep the stored labels unambiguous (a duplicate would silently collapse in the merge).
6. Insert the backfill row with `ON CONFLICT (flow_id, trigger_id) DO NOTHING`; no row inserted: 409 `backfill_exists` (D8).
7. **Attach**, through the pure model: `TriggerState::attach_backfill` (`trigger/mod.rs:270-282`) with the inclusive start (D4): the first tick **at or after** `start` (a new `ScheduleTrigger::first_tick_at_or_after`, the `next_after(at − 1 ns)` that `ticks_through` already uses). `current_tick` and `next_evaluation_date` are set to it; `previous_next_evaluation_date` is the row's `next_evaluation_date` before this write (D5); `evaluated_at` and `last_error` are left alone (Kestra keeps `evaluatedAt`).
8. Read the entry in the transaction, commit, audit (§3.10). 201 with the entry.

**Why the existing claim then runs it:** the new `next_evaluation_date` is in the past, so the row is due on the next pass and is claimed by the same `WHERE t.next_evaluation_date <= now()` (`fire.rs:150`) through the same due index (`0156:35-36`). There is no second loop, in Kestra or here: the backfill **is** the trigger's one next-date stream (`TriggerEventHandler.java:177-179`).

**Regular ticks during a backfill** do not fire: there is one stream, and it is walking the window. What happens to them when it ends is §3.6.

**`attach_backfill` changes** to the inclusive start; its tests' windows start at `00:00` on a `0 2 * * *` cron (`trigger/mod.rs:848-853, 898, 931-936, 1163-1168`), so none changes. A new pure test pins a tick exactly at `start` firing.

### 3.5 Pausing, resuming, deleting

- `PUT …/flows/{id}/triggers/{trigger_id}/backfill/paused`, body `{"paused": bool}`; `DELETE …/flows/{id}/triggers/{trigger_id}/backfill`. Both `workflow:manage`, both answer the entry.
- Same transaction shape: flow `FOR NO KEY UPDATE`, parse, trigger, the `workflow.triggers` row `FOR UPDATE` (absent is fine: the source did not parse and was fixed, or the trigger is off the queue), then the backfill row `FOR UPDATE`. None: 404 `no_backfill`. Steps 1-2's refusals are the same as create's; a disabled trigger is **not** refused (a frozen backfill can be paused or deleted).
- **Pause / resume** write `paused`, `paused_by`, `paused_at` and `updated_at` on the backfill row and nothing else. Already in the asked state: 200, no write, no audit (W7e's idempotency, `switch.rs:190-195`).
  - Paused, the claim skips it (D7): the claim gains `AND NOT EXISTS (SELECT 1 FROM workflow.trigger_backfills b WHERE b.flow_id = t.flow_id AND b.trigger_id = t.trigger_id AND b.paused)`. Today a paused backfill would be due for ever, claimed every pass, skipped with `BackfillPaused`, and its row rewritten (`trigger/mod.rs:927-975` shows the fixed point; Kestra pays the same evaluation).
  - The trigger's `next_evaluation_date` stays equal to `current_tick` while paused, so a resume needs no other write: the next pass claims the row and fires `current_tick`. That is the "re-queue".
  - Regular ticks do not fire while a backfill is paused either; §3.6 applies when it ends.
- **Delete** removes the backfill row and resumes the regular schedule as §3.6 says, with the resume position `max(previous_next_evaluation_date, current_tick)`: the backfill's unfired ticks are dropped. **Known limitation:** when the regular schedule was already overdue at create time (`previous_next_evaluation_date` before the window start), the part of that backlog before the window start is not recovered, because `max(...)` starts at `current_tick`; a regular backlog inside the window's span is still recovered. Listed in §10.

### 3.6 Firing, finishing, and the regular schedule (D6, D13)

**`fire_one`** (`scheduler/fire.rs:180-303`):
- After the claim, it reads the backfill row with a fresh statement. The claim's row lock on `workflow.triggers` is what serialises every writer of the backfill row (§3.11), so a fresh read is current. (A claim that EvalPlanQual re-checked reads the newest `t` but its snapshot's other tables, `fire.rs:59-61`; the fresh read avoids leaning on that.) A backfill found paused here (a pause committed between the claim's snapshot and its lock) writes nothing and returns `Frozen`.
- `TriggerState.backfill` is built from the row (`window_start`, `window_end`, `current_tick`, `paused: false`) instead of `None` (`fire.rs:226-233`).
- `inputs` = the schedule's `inputs:` (`fire.rs:224`) overlaid by the backfill's, then `refuse_templated` on the result (`:225`). `tick` is unchanged; its `ScheduledExecution.from_backfill` (`trigger/mod.rs:333-335, 473`), unread today, decides whether `start` (`fire.rs:314-346`) passes `backfill_id` and the backfill's `labels` to `NewExecution`.
- The final `UPDATE` (`fire.rs:289-301`) is joined, in the same transaction, by the backfill row's write: `tick`'s state still has a backfill → `UPDATE … SET current_tick = $next, ticks_done = LEAST(ticks_done + 1, total_ticks)` (D21: the `total_ticks` count and the walk use the same `Cron::next_after` sequence, so the clamp should never bite, but an unclamped write that broke the table's CHECK would fail the claimed trigger's transaction and end every pass, stopping every schedule of the deployment); it has none (the next tick passed `end`, `trigger/mod.rs:290-310`) → `DELETE` the row and replace `tick`'s next date (the first tick after now, `trigger/schedule.rs:1542-1549`) with the **resume** below.
- The early-return paths are unchanged and leave the backfill row alone: `record_error` keeps the dates and the next pass retries the same window tick (`fire.rs:282-285, 350-362`); `take_off_queue` freezes the trigger, backfill included, until the next save re-queues it from the backfill (§3.7); `AlreadyFired` consumes the tick (`:265-273`) and advances the backfill.
- **A refused start ends the backfill (D25).** `Started::Refused` on a backfill tick (an overlaid input the start refuses, a task type not served, a source that does not plan) is deterministic, and a save would re-derive the same tick with the same inputs (§3.7), so taking the trigger off the queue would leave it stuck until the user deleted the backfill. Instead, in the same transaction, the backfill row is deleted and the regular schedule resumes exactly as a delete does (`from = max(previous_next_evaluation_date, current_tick)`, then the policy), with `last_error = "backfill ended: <refusal>"`. The trigger stays on the queue unless the resume finds no tick. A transient `Failed` still retries the same tick. A refusal on a regular tick is unchanged (`take_off_queue`).

**Resume (D6).** A pure function beside `tick`, `TriggerState::resume_after_backfill(trigger, from, policy, now) -> Option<DateTime<Utc>>`, used by completion and by delete:
- `from`: on completion, `max(previous_next_evaluation_date, first tick after window_end)`; on delete, `max(previous_next_evaluation_date, current_tick)`. The regular schedule resumes where it stood; a regular backlog that the backfill already covered is not run twice. **Known limitation:** `max(...)` starts no earlier than the window (the first tick after `window_end` on completion, `current_tick` on delete), so when the regular schedule was already overdue at create time, the part of its backlog before the window start is not recovered (§10).
- `policy`: the trigger's `recoverMissedSchedules`, read as W7e reads it (`RecoverMissed::from_model`, `trigger/schedule.rs:1271-1281`); a value it does not know is `ALL`, egret's behaviour whenever it does not apply the property (W7e §10).
- `from > now`: `from` (nothing was missed). Otherwise `ALL`: `from` (each missed regular tick runs, one per pass); `LAST`: the latest tick at or before now (`last_tick_at_or_before`, `schedule.rs:1421-1425`); `NONE`: the first tick after now. No tick at all: off the queue with `never_fires` (`scheduler/sync.rs:33-38`), as the switch route does (`switch.rs:282-300`). When the last tick's outcome already set a `last_error`, both are kept, `never_fires` first (`"<never_fires>; <outcome>"`): it is the state the row stays in.
- **Why not Kestra's:** Kestra skips them on completion and replays them on delete (§2). The first loses runs the user never asked to lose (a three-hour backfill on an hourly schedule costs three runs), and the outcome depends on whether the user waited. egret replays them under the trigger's own setting, which is also what the re-enable dialog offers.

**Throughput (D13).** One window tick per trigger per pass, because `fire_due`'s `seen` list makes "at most once per pass" a property of the query (`fire.rs:79-136`). A window of N ticks takes about N passes, N seconds at the default interval: 10,000 ticks, under three hours. Firing several per pass would hold the trigger row and the transaction for a batch, and a render miss rolls back the whole attempt (`fire.rs:91-104`), so a batch would redo work. Kestra also fires one per evaluation (W7e §2 observed `ALL` replays about 1.6 s apart). The form says how long the window will take.

**Safety.** `tick` never sees a paused backfill in the loop, so the `BackfillPaused` skip is unreachable there; it stays for the pure model. `NotDue`/`NotSchedulable` cannot come from a backfill (its date is past and a disabled trigger is not claimed), so `fire.rs:241-254`'s debug panic is unchanged.

### 3.7 `sync` and the backfill (D10)

`sync` (`scheduler/sync.rs:78-200`) runs inside every save, `reconcile` and a pass's `resync`, each holding the flow row (`fire.rs:382-425`).

- **Re-initialisation.** Today a row whose `spec_key` changed, or which is off the queue, gets the next tick after now and `evaluated_at = NULL` (`sync.rs:150-165`), and a new row the same. With a backfill row for that trigger the new date is **re-derived from the backfill**: the first tick of the (new) cron at or after `current_tick`. If that is after `window_end`, the backfill is complete: the row is deleted and the date is the resume (§3.6). Otherwise `current_tick` takes the new date, `total_ticks` becomes `ticks_done` plus the ticks left (`ticks_through(current_tick, window_end)`), and `previous_next_evaluation_date` is re-snapped to the first tick of the new cron at or after it. Two edge cases (default decisions, 2026-10-15):
  - **Over the cap (D19).** If that recount exceeds the table's `CHECK (total_ticks BETWEEN 1 AND 10000)` (a week-long daily window becomes 10,080 ticks under `* * * * *`), the CHECK would abort the save with a 500, and `sync` must never fail a save over a backfill. So `sync` ends the backfill: it deletes the row, resumes the regular schedule as a delete does (§3.5, §3.6), writes `last_error = "the backfill ended: the new schedule gives more than 10000 ticks in its window"` and logs at `warn`.
  - **No tick to re-snap to (D20).** A new cron with no tick at or after `previous_next_evaluation_date` gives none, and the column is `NOT NULL`: the stored value is kept, and the resume (§3.6) then finds no tick and takes the row off the queue with `never_fires`. Kestra keeps the backfill on a definition change too (`TriggerEventHandler.java:500-513`); its strictly-after recompute would drop the current tick (§2).
- A schedule that no longer resolves (`lateMaximumDelay`, `when`, `trigger/schedule.rs:1298-1327`) is off the queue as today; its backfill is kept and frozen until a save that resolves.
- **The decision moves into Rust.** The upsert's `CASE` (`sync.rs:157-165`) cannot see the backfill table, so `sync` reads the flow's `workflow.triggers` rows `FOR UPDATE` and its backfill rows before the upsert, decides each date, and writes it. The lock order stays flow, trigger rows, backfill rows (§3.11).
- **Lifecycle,** after the trigger rows, beside W7e's switch rows (`sync.rs:181-200`):
  - the source parses: delete the backfills whose `trigger_id` is no longer a declared **`Schedule`** (a removed id or a kind change: the schedule state goes, as Kestra deletes the trigger state, `TriggerEventHandler.java:535-541`);
  - `gone` (soft delete): delete them all (the cascade does not fire);
  - the source does not parse: keep every one. When it parses again, the trigger row is re-created and re-derived from the backfill as above.
- Confirmed acceptable (default decision, 2026-10-15): a removed trigger id or a kind change ends its backfill without a resume, since there is no schedule left to resume. The executions already fired stay.

### 3.8 Re-enabling during a backfill (D11)

W7e's step 7 (`triggers/switch.rs:232-304`) computes "skip" with `TriggerState::initialise`, which has no backfill (`:250-255`), and writes it into `next_evaluation_date` (`:269-281`). With a backfill attached that would point the stream at the future while `current_tick` stays in the past: the window would never be walked, and the next regular tick would clear it as finished.

- With a backfill row, step 7 keeps `next_evaluation_date` (= `current_tick`) for every choice, so the backfill resumes at the tick it stopped on. No window tick is lost (Kestra's skip loses one, §2).
- The choice applies to the regular schedule's resume position instead: "skip" (or `NONE`) moves `previous_next_evaluation_date` to the first tick after now; "recover" with `ALL` or `LAST` leaves it, and §3.6 applies the policy when the backfill ends. `recover_missed_unknown` is checked as today.
- The answer's `moved_to` (`switch.rs:42-43`) is then `null`; the audit event carries the moved resume position instead.

### 3.9 Reading triggers (D12, D15)

`read_state` (`triggers/state.rs:119-164`) reads `workflow.trigger_backfills` for the flows in scope, with the creator's and pauser's display names as it reads `disabled_by_name` (`:126-133`). The entry builder (`triggers/entry.rs:65-218`) stays pure.

`TriggerEntryDto` (`triggers/routes.rs:158-192`) gains:

| Field | Meaning |
|---|---|
| `backfill` | `null`, or `{id, start, end, current_tick, paused, paused_by_name, paused_at, inputs, labels, created_by_name, created_at, previous_next_evaluation_date, total_ticks, ticks_done, progress}` |
| `backfill.progress` | 0-100, Kestra's formula: `(current_tick − start) / (end − start)`, rounded and clamped, 100 for an empty span (`BackfillBanner.vue:75-82`) |
| `can_backfill`, `backfill_refusal` | whether the Backfill button is usable, and why not, the first that applies: `flow_deleted`, `not_schedule`, `forbidden` (no `workflow:manage`), `disabled`, `off_queue`, `running` (one exists) |

- **`schedule.missed_ticks`** (`triggers/entry.rs:226-238`) today counts from `next_evaluation_date` through now. During a backfill that date is a past window tick, so it would count every tick from there to now, including ticks after `end` that will never run, and the enable dialog (`frontend/src/components/workflows/triggers/triggersModel.ts:100-105`) would offer to "recover" them. With a backfill it counts from `previous_next_evaluation_date` instead: the regular ticks waiting for the backfill to end, which §3.6 and §3.8 decide about.
- **The global list** (`GET …/workflows/triggers`) carries the same fields per row and gains a `backfill` filter: `running`, `paused` or `any` (in memory, like `kind` and `state`, `triggers/routes.rs:380-386`).
- **Last execution** (`triggers/state.rs:32-40`) includes backfill executions, as Kestra's "Execution Id" column does; the overview marks them.
- **Visibility:** a backfill's inputs and labels are shown to `workflow:read`, as every execution's inputs already are (`ExecutionListItemDto.inputs`, `routes.rs:1436`).

### 3.10 Routes, bulk, audit (D14)

| Route | Answer |
|---|---|
| `POST …/flows/{id}/triggers/{trigger_id}/backfill` | 201, the entry |
| `PUT …/flows/{id}/triggers/{trigger_id}/backfill/paused` `{paused}` | 200, the entry |
| `DELETE …/flows/{id}/triggers/{trigger_id}/backfill` | 200, the entry |
| `PUT …/workflows/triggers/backfill/paused` `{triggers: [{flow_id, trigger_id}], paused}` | 200, `BulkResultsDto` |
| `POST …/workflows/triggers/backfill/delete` `{triggers}` | 200, `BulkResultsDto` |

- Registered beside W7e's (`routes.rs:3330-3350`); all `workflow:manage`. Bulk: at most 100 (400 `too_many_triggers`), 64 KiB (`MAX_BULK_TRIGGERS_BODY_BYTES`, `triggers/routes.rs:640-643`), one transaction per entry, a refusal recorded and the next entry run, a foreign flow `not_found` (W7e §3.7). The bulk delete is a `POST` because a `DELETE` with a body is not portable.
- Errors go through `TriggerRouteError::Coded` (`triggers/routes.rs:258-290`); the refusal enum is a new `BackfillRefusal` with its own `refusal_parts`, as W7e's (`:740-776`).
- **Audit:** `workflow_trigger:backfill_create` (window, tick count, input keys and label keys, not values), `:backfill_pause`, `:backfill_resume`, `:backfill_delete` (with the resume date), resource `workflow_trigger`, id `{flow uuid}/{trigger_id}`, only on a real change. Completion is the scheduler's and is not audited.

### 3.11 Concurrency

Every writer of a backfill row holds, in order: the flow row (`FOR NO KEY UPDATE` for the routes and a save, `FOR SHARE` for `reconcile`, `FOR SHARE SKIP LOCKED` for `resync`), then the trigger's `workflow.triggers` row, then the backfill row. A pass holds the trigger row from its claim and writes the backfill row in its transaction.
- **Route against a pass:** the route waits on the trigger row; the pass takes only `KEY SHARE` on the flow (its execution insert), which `FOR NO KEY UPDATE` does not block. No cycle (W7e §3.2). After a pause or delete answers, no new backfill tick fires.
- **Route against a save:** serialised on the flow row, so `sync` never re-derives from a backfill that is being created or deleted.
- **Two creates:** serialised on the flow row; the second sees the first's row and answers 409.
- **A trigger row that does not exist** (the source does not parse): nothing fires it, and only `sync` (holding the flow) or a route (holding the flow) can touch the backfill.

### 3.12 Comments to correct

- `lib.rs:99-102`: "backfill has no route yet (W7f)" names the routes instead.
- `scheduler/fire.rs:261-264` and `scheduler/mod.rs` statement 2: the backstop is two indexes.
- `trigger/mod.rs:229-241` (`TriggerState`): the backfill's DB home is `workflow.trigger_backfills`; the pure `Backfill` stays reduced to the window (`schedule.rs:1628-1639`), its inputs and labels living beside it.
- W7e spec §9's second bullet: corrected by §2's re-enable row (not edited in place; this spec supersedes it).

## 4. Frontend

Everything stays in W7a's lazy Workflows area (FE-ROUTE-2). Client regenerated with `npm run gen:api` (FE-DATA-1); keys in `routes/workflows/queryKeys.ts` (FE-DATA-2). New modules in `components/workflows/triggers/`: `BackfillDialog.tsx`, `BackfillStrip.tsx`, `LabelsEditor.tsx`, `useBackfill.ts`; pure helpers in `triggersModel.ts`.

### 4.1 The Triggers tab and the global page

- **A Backfill column** beside Enabled (W7e §9), on both tables (`TriggersTable.tsx:105-152`):
  - a `Schedule` with no backfill: a "Backfill" button (`ReasonButton`, disabled with the server's `backfill_refusal` in words, FE-ACTION-4: "Only schedules can be backfilled", "You need workflow:manage", "Enable the trigger to backfill it", "Off the queue: {last_error}", "The flow is in the trash");
  - a running backfill: "Running" with an icon; a paused one: "Paused" with another (FE-A11Y-4); other kinds: empty.
- **The progress strip,** an extra `TableRow` under the trigger's row, spanning the columns (Kestra's always-expanded sub-row): "{start} → {end}", "{progress}% · {ticks_done} of {total_ticks} ticks", a `ui/progress` bar (no animation, FE-A11Y-5; the paused bar uses the muted token and the word "Paused"), "Pause"/"Resume" and "Delete" buttons. Delete opens a confirm (FE-ACTION-1): "Delete this backfill? Its remaining {n} ticks will not run. Executions already started stay." A disabled trigger's strip says "Frozen while the trigger is off".
- **Status** (`triggersModel.ts:43-62`): after the YAML, switch, flow and off-queue causes, "Backfilling ({progress}%)" or "Backfill paused ({progress}%)".
- **The enable dialog** (`EnableTriggerDialog.tsx`): with a backfill, one more line: "The backfill resumes at {current_tick}. This choice is about the regular runs that fell due." `missed_ticks` already means the regular ones (§3.9).
- **Global page bulk** (`routes/workflows/TriggersList.tsx:331-350`): "Pause backfills", "Resume backfills", "Delete backfills" (confirm) on the selection; results per entry as W7e's (`no_backfill` reads "had no backfill"). A `backfill` facet (Running, Paused, Any) in the URL (FE-PAGE-2).
- **Refresh:** the tab's 30 s poll (`FlowTriggersTab.tsx:27`) stays; while a backfill of the shown flow runs, the tab polls every 5 s so the strip moves. Each action invalidates both keys, as `useTriggerSwitch` does (`useTriggerSwitch.ts:68-76`), and toasts only for the bulk (FE-ACTION-2).

### 4.2 The details drawer

`TriggerDetailsDrawer.tsx` gains a "Backfill" section under Schedule: window (in the trigger's timezone and the viewer's), current tick, progress, created by and when, paused by and when, the inputs (key/value table) and labels (chips), and "Regular runs resume from {previous_next_evaluation_date}" with what `recoverMissedSchedules` will do with the waiting ones ("{n} regular runs are waiting; when the backfill ends, each will run", per the policy, reusing `recoverDescription`, `triggersModel.ts:107-113`).

### 4.3 The create form (D16)

`BackfillDialog`, loaded through `lazy()` from both the tab and the global page (it brings the Run form's model and the `yaml` library, `run/inputModel.ts:1`), and listed in `scripts/assert-workflows-lazy.mjs` with a marker, flagged with a new `backfill` flag, not `editor` (D23): `editor` only checks that the marker stays out of the flow list's chunk (`assert-workflows-lazy.mjs:383-392`) and would pass if `FlowTriggersTab.tsx` imported the dialog statically, whereas `backfill` also keeps it out of what loads with the flow pages (`FLOW_PAGE_HOSTS`, as `files` does, `:361-372`), which is what D16's "lazy chunk" means.
- **Title** "Backfill {trigger id}"; a description: "Runs this schedule for each of its ticks from start to end, both included, one about every second. Regular runs wait until it ends."
- **Start and End:** native `datetime-local` (as `KvValueControl.tsx:65` and the Run form's controls, `run/inputControls.tsx:30`), `step="1"`, `max` = now (read when the dialog opens and on each change), each with a tied `Label` (FE-FORM-2). End may be left empty ("Leave empty to end now"). The values are the browser's local time, converted to UTC; under the fields: "In {trigger timezone}: {start} → {end}", and the server's tick count after a 400 names it.
- **Inputs:** the flow's inputs from its saved source (the tab has it; the global page reads `GET /flows/{id}`), through `inputsFromSource`, `formFromInputs`, `validateForm` and `InputField` (`run/inputModel.ts:98, 207, 266`; `run/inputControls.tsx:33`), prefilled with the schedule's plain `inputs:` values over the flow's defaults. `FILE` inputs are not offered; a required one makes the submit disabled with "A FILE input cannot be backfilled".
- **Labels:** `LabelsEditor`, key/value rows with Add and Remove; a `system.` key or a half-filled row shows its error beside the field (FE-FORM-4); fully empty rows are dropped (Kestra's `cleanBackfill`).
- **Submit** "Start backfill", disabled with its reason until: a start, `start < end` (or no end), neither in the future, required inputs filled, no half-filled label (Kestra's `checkBackfill`, plus the order check). Hand-written form state (FE-FORM-1).
- **Errors:** a 400 whose code names a field goes beside it (`backfill_start_too_early` → Start; `backfill_end_in_future`, `backfill_window_invalid` → End; `backfill_inputs_invalid` → the named input via `serverErrorInput`, `run/inputModel.ts:388`; `backfill_labels_invalid` → Labels); others above the buttons with what to do (FE-COPY-2). 409 `backfill_exists` refetches and closes with the server's sentence. Nothing typed is cleared (FE-ACTION-3).

### 4.4 Executions

The overview's labels chips and the "Backfill" mark (§3.3). No list filter (§10).

## 5. Errors

| Status, code | When |
|---|---|
| 400 `invalid_body`, 413 `body_too_large` | W7e's |
| 400 `source_does_not_parse` | W7e's |
| 400 `backfill_window_invalid` | `start >= end` |
| 400 `backfill_end_in_future` | `end > now()` (the message gives the server's time) |
| 400 `backfill_start_too_early` | `start` more than ten years back |
| 400 `backfill_no_ticks` | the window holds no tick of this schedule |
| 400 `backfill_too_many_ticks` | more than 10,000 ticks |
| 400 `backfill_inputs_invalid` | not an object; a `FILE` input; template syntax in the merged inputs; a required input missing. The message names the input |
| 400 `backfill_labels_invalid` | a `system.` key, an empty key or value, template syntax, over the caps |
| 400 `too_many_triggers` | a bulk list over 100 |
| 404 `not_found` | the flow, or the trigger id (W7e's sentence) |
| 404 `no_backfill` | pause, resume or delete with none |
| 409 `backfill_not_supported` | not a `Schedule` |
| 409 `trigger_disabled` | create on an effective-disabled trigger |
| 409 `trigger_off_queue` | create on a trigger with no next date |
| 409 `backfill_exists` | create while one exists |

## 6. Tests

**Rust, pure** (`trigger/`): the inclusive start (a tick at `start` fires); `first_tick_at_or_after`; `resume_after_backfill` for `ALL`, `LAST`, `NONE`, unknown, `from > now`, no tick left, and both `from` rules; the label merge (order, last wins, empty dropped); the inputs overlay with `refuse_templated` on the merged map.

**Rust, database** (`tests/` of `egret-workflow`, each with its own database):
- Create: every refusal of §5 with its code; the trigger row's `next_evaluation_date` = first window tick and `previous_next_evaluation_date` = the old date; another workspace's flow 404; `workflow:read` 403.
- Drain: a `* * * * *` window of 5 ticks fires 5 executions in order over 5 passes, each with `backfill_id`, the overlaid inputs and the labels; the row is then deleted and the regular date is the resume.
- **Re-run (user decision):** a window over ticks that already have regular executions creates a second execution per tick; a second backfill over the same window creates a third; a forced second insert for one `(backfill_id, scheduled_date)` violates `workflow_executions_one_per_backfill_tick` and is consumed as `AlreadyFired`; the regular index still refuses a second regular execution (`the_unique_index_refuses_a_second_fire` unchanged).
- The oracle test `schedule_trigger_record_matches_the_captured_execution` unchanged, and a backfill execution's `execution_trigger` equal to a live one's for the same tick.
- Resume: completion and delete for `ALL` (the waiting regular ticks run), `LAST`, `NONE`; a regular backlog older than `end` is not run twice.
- Pause: a paused backfill is not claimed and its row is not written over several passes (`updated_at` unchanged); resume fires `current_tick` next; pause after the claim's snapshot (the fresh read) writes nothing, staged by a test-only hook (D22): the window is between the claim's `SELECT … FOR UPDATE OF t SKIP LOCKED` and the next statement, where nothing is written and the claim never waits, so no lock or table trigger can hold a pass there; `scheduler::fire` gets a `#[cfg(any(test, feature = "test-util"))]` hook that parks the pass right after the claim (precedent: `worker`'s test-util exports, `src/worker/mod.rs:100-110`), at no cost to a release build. A test also drains a window exactly and asserts `ticks_done == total_ticks` just before the row goes (D21); another changes the cron mid-backfill to one that exceeds 10,000 ticks and asserts the backfill ends with `last_error` (D19), and one that re-snaps to no tick and keeps the stored date (D20).
- Kestra's delete bug: pause, resume, delete: the regular schedule resumes from the stored position, not from a window date.
- `sync`: a cron change mid-backfill re-derives without losing the current tick and recounts; an unparseable source (written by SQL) keeps the backfill and a fixing save resumes it; a removed id, a kind change and a soft delete delete it.
- Re-enable during a backfill, each choice: dates kept; skip moves only the resume position.
- Labels: render sees them (`{{ labels.reason }}`), a backfill label overrides a flow label of the same key, a downstream `dependsOn` with `labels:` matches a backfill execution; replay, retry and loop iteration copy them; a subflow child does not.
- Reads: `backfill`, `progress`, the counts, `can_backfill`/`backfill_refusal` per cause, `missed_ticks` from the resume position, the `backfill` filter.
- Concurrency: a delete that meets a pass holding the row waits, and no tick fires after it; a save with a cron change and a pause on one flow serialise.
- Bulk: 100 accepted, 101 refused, mixed results with `no_backfill`.
- Mutations (each must kill a test): drop `AND backfill_id IS NULL` (the re-run test dies); drop the paused term from the claim; write `previous_next_evaluation_date` on pause; use strictly-after in `attach_backfill`; skip the label merge in `load_source`.

**Vitest:** the column's button and tags, disabled reasons; the strip's text, percentages and buttons; the delete confirm; the dialog's submit reasons, the body it sends (UTC instants, inputs, labels), field-level server errors; `LabelsEditor`; the drawer section; the global bulk bar and results; the overview's labels and mark.

**Playwright** (`frontend/tests/e2e/workflows-backfill.spec.ts`), on a private stack with the scheduler and worker on, never the shared dev stack (W7e §6's harness):
1. A flow with a `* * * * *` schedule, a flow label `team: data` and an input. Backfill the last five minutes with an input and the labels `team: ops`, `reason: test`: five executions appear, the strip moves to 100 % and goes away, each execution's overview shows `team: ops` and "Backfill"; light and dark mode.
2. Re-run: backfill a window that the schedule already ran: a second execution per tick.
3. A seconds cron (`withSeconds`) and a one-hour window: pause, wait, no new executions; resume, they continue; delete with the confirm: the strip goes, the next run is the regular one.
4. The form: future dates cannot be picked; Start after End disables the submit with its reason; a missing required input; a second create through the API answers 409.
5. The global page: filter `backfill=running`, bulk pause two, reload, both paused; bulk delete.
6. A viewer without `workflow:manage`: the button and the strip's actions are disabled with the reason.
7. Switch the trigger off during a backfill: "Frozen while the trigger is off"; on again: it resumes.

## 7. Comparing with Kestra

The plan's last task runs on the local image `kestra/kestra` `28f59bf175e1` (`server local`, a throwaway container), with §6's scenario 1-3 and 5 on a `* * * * *` flow, and checks each **read** row of §2: a window starting exactly on a tick (exclusive start); a window over executed dates (a second execution); a backfill label with the flow label's key (which wins); inputs overriding the schedule's; pause, resume, delete (which date is restored, and whether regular ticks replay); natural completion with a regular tick inside the backfill's duration (skipped); the API with `end` in the future, `start > end`, a second create, pause with no backfill; bulk by triggers (202); the progress formula on screen. Each measured row becomes **observed**; a difference found is added to §2 (FE-REF-2).

## 8. Rules this package must meet

- The frontend rulebook, every rule.
- No new dependencies (the date controls are native).
- Regenerate the client only with `npm run gen:api`.
- Migration prose is append-only: 0156 and 0189 are not edited.

## 9. What is not built

- **Bulk create,** by selection or by query (Kestra has none).
- **Pause/resume/delete by query** (Kestra's `by-query`): selection only, as W7e D10.
- **Several ticks per pass** (§3.6).

## 10. Follow-ups

- Apply the trigger's own `labels:` and `system.from` to schedule executions, which egret parses and drops (`models/trigger.rs:51`); it needs a render context for the trigger's variables.
- Filter executions by label and by "backfill" on the executions list.
- Recover the part of the regular backlog that precedes the window start when the schedule was already overdue at create time (§3.5, §3.6 known limitation).
- A backfill for a trigger that is switched off, if users want to queue one.
- A per-trigger log (W7e §10) would let the drawer show which backfill ticks were skipped and why; today only `last_error` keeps the latest.
