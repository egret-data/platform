# W7e — Workflows UI part 5 (triggers): Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In the browser, a user can:
- see every trigger of a flow on its Triggers tab: next run, last tick, last execution, why it does not fire;
- switch a `Schedule`, `Webhook` or `Flow` trigger off and on at runtime, without editing the YAML, choosing to skip or recover missed ticks when re-enabling a schedule;
- open a trigger's details: its YAML, its schedule, its `dependsOn` window, its webhook URL;
- list and bulk-switch the triggers of the whole workspace on a global Triggers page.

**Architecture:**
- **Backend** (`crates/egret-workflow`, one migration):
  - `workflow.disabled_triggers` (a row = switched off), and `scheduler::sync` writing the **effective** value (flow YAML OR trigger YAML OR row) into `workflow.triggers.disabled` and `workflow.flow_listeners.disabled`, so every existing `NOT disabled` predicate honours the switch unchanged;
  - the webhook route reads the switch after the key matched;
  - a new `triggers` module: the per-flow and the workspace trigger listings (parsed from drafts off the async workers), the switch route and the bulk route;
  - `recoverMissedSchedules` captured on `models::Schedule`; `duplicate_trigger_id` as a validation finding.
- **Frontend** (inside W7a's lazy Workflows area): a `components/workflows/triggers/` family (model, table, drawer, enable dialog, banners), the flow detail's fourth tab, and `routes/workflows/TriggersList.tsx`.

**Tech Stack:** Rust (axum 0.8, sqlx 0.9, utoipa), React 19 + TypeScript, TanStack Query 5, orval, vitest 4, Playwright.

**Spec:** `docs/{en,vi}/specs/2026-10-14-workflow-w7e-triggers-design.md`. **§n refers to the spec; Dn to its default decisions.** The spec's header holds the user's dated rulings.

**A warning about this document.** Code blocks show shapes, signatures and test cases checked against the code at `8eb161e9f`; they are not text to paste. Every `file:line` below was read at that commit. Re-read each fact before relying on it: line numbers move as earlier tasks land.

## Global Constraints

- **The frontend rulebook binds every frontend line:** `docs/en/frontend-rules.md`. Every frontend task review answers its reviewer checklist.
- **User decisions (spec header, 2026-10-14)**, in effect verbatim:
  - Backfill is a separate package, W7f. W7e builds none of it (no migration, route, `fire_one` wiring, form or banner for it) and must not foreclose it (§9).
  - A runtime enable/disable switch, stored in the database as Kestra stores it, separate from the YAML `disabled:`. Re-enabling a `Schedule` asks the user whether to recover the missed ticks.
- **Default decisions D1–D12 (spec header)** bind. The ones that most shape code: a row in `workflow.disabled_triggers` means off (D1); the switch is the authority and the two `disabled` columns are its effective copy (D2); `Schedule`, `Webhook` and `Flow` have the switch, a `Leaf` has none (D3); enabling a YAML-disabled trigger is 409 (D4); bulk is by selection, at most 100, one transaction each (D10); no unlock/restart/delete-state (D12).
- **Exact values:**
  - routes: `GET /api/ws/{workspace}/workflows/flows/{id}/triggers` and `GET /api/ws/{workspace}/workflows/triggers` (`workflow:read`); `PUT …/flows/{id}/triggers/{trigger_id}/disabled` and `PUT …/workflows/triggers/disabled` (`workflow:manage`);
  - switch body `{ "disabled": bool, "recover_missed": bool }`; bulk body `{ "triggers": [{flow_id, trigger_id}], "disabled", "recover_missed" }`, at most 100 entries, body cap 64 KiB;
  - codes: `trigger_disabled_in_source`, `trigger_not_supported`, `recover_missed_unknown` (409), `triggers_too_large` (422), `duplicate_trigger_id` (validation);
  - list query: `namespace_prefix`, `flow_id`, `kind` (comma-separated `schedule,webhook,flow,other`), `state` (`enabled`/`disabled`), `q`, `limit` (default 50, max 200, clamped), `offset`;
  - the flow bound 2,000 flows (422 beyond, message "more than 2000 flows match; narrow by namespace");
  - `missed_ticks` counted to 1,000, then "1000+";
  - audit actions `workflow_trigger:disable` / `workflow_trigger:enable`, resource `workflow_trigger`, id `{flow uuid}/{trigger_id}`;
  - URLs: `/explore/:workspace/workflows/flows/:id/triggers` (with `?trigger=<id>` for the drawer) and `/explore/:workspace/workflows/triggers`;
  - `FLOW_TABS = ["edit", "revisions", "executions", "triggers"]`.
- **Migration number: `0189`, not `0186`.** origin/main ends at `0185`, but the unmerged, active branch `feat/view-governance` holds `0186_plugin_downloads.sql`, `0187_view_lineage.sql` and `0188_view_tag_provenance.sql` (checked with `git ls-tree` on every local and remote ref, 2026-10-14). The number space is shared (memory `migration-numbers-are-a-shared-namespace`). Re-check every ref before Task 2 and take the next free number if `0189` is gone; say so in the migration's first line, as `0156` and `0158` do.
- **Migration prose is append-only:** `0156` and `0157` are not edited; the changed meaning of their `disabled` columns is stated with `COMMENT ON` in the new migration (§3.1, §8).
- **Dependencies:** none new. Regenerate `openapi.json` with `cargo run -q -p egret-bin -- openapi > openapi.json` (from the worktree root) and the client only with `npm run gen:api` (in `frontend/`); never bare `orval`. No prettier.
- **Earlier rulings hold:** every execution runs the saved draft, and a draft fires (W7a; `process.rs:908-911`); the webhook's four absences stay one byte-identical 404 (`webhook_routes.rs:107-125`); the key context has exactly one consumer, `render_key` (`trigger/webhook.rs:2246-2286` counts `.render_context_for(` callers) — the listing must call `render_key`, never `render_context_for`; W7c's lazy-chunk discipline and the bundle guard.
- **Environment:**
  - Work only in `/Users/1pro-l4r8/Documents/github/data-portal/egret-w7e`. Never read or build `/Users/1pro-l4r8/Documents/github/data-portal/egret` (a stale checkout).
  - **Database:** the branch container `egret-w7e-postgres` on port **5448**, created by the controller (`docker run -d --name egret-w7e-postgres -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret -e POSTGRES_DB=egret -p 5448:5432 postgres:16`). Every cargo test runs with `DATABASE_URL=postgres://egret:egret@localhost:5448/egret` exported and `EGRET_ALLOW_DB_SKIP` **unset**. Never the shared dev DB on :5432, shared buckets, `deploy/docker-compose.dev.yml` services, or the kind cluster `egret`.
  - **Build dir:** `export CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-w7e/target-check` (worktree-local and already in `.gitignore`). Keep at least 10 G free on the disk.
  - Implementers and fix rounds run **only the tests they touch** (`--test <file> <name>`); the full suites run once, at the final gate (Task 8). Never two cargo or vitest suites at once; wait while `pgrep -x cargo` is non-empty.
  - Commit before a long suite. Count tests from the full output, never from a `tail`.
  - Never `git add -A` or `commit -a`; stage named paths. Never commit `.superpowers/`. Another agent may write `docs/vi/...` in this worktree: stage only your own files.
  - Use `command cp -f` in scripts (a `cp -i` alias exists).
  - Commit trailer: `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Decided: the webhook URL (default decision, 2026-10-14)

**The URL the spec once told the drawer to build, `window.location.origin + url_path`, does not reach the backend in any deployment shape.** `url_path` is `/workflows/v1/{ws}/executions/webhook/…` (`egret-server/src/lib.rs:533` nests the router at `/workflows/v1`). At the browser's origin:
- the Helm ingress sends `/` to the frontend and routes only `/auth`, `/data-api`, `/oauth`, `/catalog` and the two `/.well-known` documents to the backend (`deploy/helm/egret/templates/ingress.yaml:45-112`);
- `frontend/nginx-prod.conf` proxies `/api/`, `/data-api/`, `/oauth/`, `/catalog` and the `.well-known` documents, and its `location /` catch-all (`:205`) answers anything else with the SPA's `index.html`, 200 `text/html`;
- the Vite dev proxy forwards `/api`, `/catalog`, `/oauth` only (`frontend/vite.config.ts:40-43`).

Routing `/workflows/` to the backend would expose the anonymous surface whose own review says it is not approved for exposure (`webhook_routes.rs:60-64`). **Decision: W7e adds no routing and does not build the URL from `window.location.origin`.** The drawer shows the webhook **path** with a copy button and a note that it is served by the egret backend at whatever host the operator exposes for webhooks, plus the existing "Webhooks are off on this server" warning when the flag is off. Exposing webhooks through ingress is a follow-up needing its own security review. The spec (§4.2) agrees. Task 8 still curls the backend's own port.

## Gates

- **Backend** (each with `DATABASE_URL` and `CARGO_TARGET_DIR` as above):
  - `cargo fmt --check`
  - `rtk proxy cargo clippy --workspace --all-targets -- -D warnings`
  - `EGRET_ALLOW_ENGINE_SKIP=1 EGRET_ALLOW_DOCKER_SKIP=1 cargo test -p egret-workflow --no-fail-fast`
  - `cargo test -p egret-server` (it carries `tests/feature_route_coverage.rs`, which reads the OpenAPI document, and `tests/workflow_webhook_mount.rs`, which builds a `WorkflowState`)
  - `cargo test -p egret-config` if `egret-config` is touched (it is not, by this plan)
- **Frontend** (from `frontend/`): `npm run lint`, `npx tsc -b`, `npm run test`, `npm run build`, `npm run test:bundle:workflows`, `npm run test:bundle:monaco`.

**Per task**, an implementer runs `cargo fmt --check`, clippy on `-p egret-workflow --all-targets`, and only the test files it touched; or, for frontend tasks, `npm run lint`, `npx tsc -b`, the vitest files it touched (`npx vitest run <path>`), and `npm run build && npm run test:bundle:workflows` when a module was added.

## File map

| Path | Task | Responsibility |
|---|---|---|
| `crates/egret-workflow/src/models/trigger.rs` | 1 | `Schedule::recover_missed_schedules` (raw, round-tripped) |
| `crates/egret-workflow/src/trigger/schedule.rs` | 1 | pure: `RecoverMissed`, `ScheduleTrigger::last_tick_at_or_before`, `::ticks_through` |
| `crates/egret-workflow/src/spec/validate.rs` | 1 | `duplicate_trigger_id` (save-only) |
| `migrations/0189_workflow_disabled_triggers.sql` | 2 | the switch table, the column comments, the last-execution index |
| `crates/egret-workflow/src/scheduler/sync.rs` | 2 | effective value, switch-row lifecycle |
| `crates/egret-workflow/src/webhook_routes.rs` | 3 | the switch lookup after the key; module doc |
| `crates/egret-workflow/src/routes.rs`, `src/lib.rs`, `crates/egret-workspace/src/features.rs` | 3 | §3.10's comments |
| `crates/egret-workflow/src/triggers/{mod,entry,state,routes}.rs` | 4 | listings: pure entry builder, state reads, read routes, DTOs |
| `crates/egret-workflow/src/routes.rs` (`WorkflowState`, router, OpenAPI) | 4, 5 | `trigger_flags`, routes, schemas |
| `crates/egret-bin/src/main.rs:2302-2327`, `crates/egret-server/tests/workflow_webhook_mount.rs:110-129, 144-163`, `crates/egret-workflow/tests/common/mod.rs:396-414, 773-791` | 4 | the new `WorkflowState` field |
| `crates/egret-workflow/src/triggers/switch.rs` | 5 | the switch transaction, single and bulk |
| `frontend/src/components/workflows/triggers/*` | 6, 7 | model, table, drawer, dialog, banners, flow tab |
| `frontend/src/routes/workflows/{FlowDetail,WorkflowsRoutes,TriggersList,queryKeys}.tsx/.ts`, `src/components/layout/Sidebar.tsx`, `scripts/assert-workflows-lazy.mjs` | 6, 7 | wiring |
| `frontend/tests/e2e/workflows-triggers.spec.ts`, the spec (EN and VI) | 8 | end to end, Kestra comparison |

---

### Task 1: `recoverMissedSchedules`, the schedule helpers, and `duplicate_trigger_id` (§3.3, §3.8) — **sonnet**

Pure code, no database. It lands first because Tasks 4 and 5 call all three.

**Files:**
- Modify: `crates/egret-workflow/src/models/trigger.rs:93-96` (the "Dropped" doc line), `:108-151` (`Schedule`).
- Modify: `crates/egret-workflow/src/trigger/schedule.rs` (beside `ScheduleTrigger::schedule_dates`, `:1385-1393`), `crates/egret-workflow/src/trigger/mod.rs:131-133` (re-export `RecoverMissed`).
- Modify: `crates/egret-workflow/src/spec/validate.rs:349-359` (`save_only_findings`).
- Test: unit tests in the same files' `#[cfg(test)] mod tests`.

**Interfaces — Produces:**
```rust
// models/trigger.rs, inside `pub struct Schedule` (#[serde(rename_all = "camelCase")]):
#[serde(default, skip_serializing_if = "Option::is_none")]
pub recover_missed_schedules: Option<serde_json::Value>,

// trigger/schedule.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverMissed { All, None, Last }
impl RecoverMissed {
    /// Absent = `All` (Kestra's `Schedulable.java:47-51`). Anything but the
    /// strings "ALL", "NONE", "LAST" is `Err(raw value rendered as JSON)`.
    pub fn from_model(v: Option<&serde_json::Value>) -> Result<Self, String>;
}
impl ScheduleTrigger {
    /// The latest tick `t <= at` in this trigger's zone. `Cron::last_before`
    /// (`:700`) is STRICTLY before, so it is asked about `at + 1ns`.
    pub fn last_tick_at_or_before(&self, at: DateTime<Utc>) -> Option<DateTime<Utc>>;
    /// The ticks `t` with `from <= t <= at`, counted up to `cap`:
    /// `(count, more)` where `more` means "there are more than `cap`".
    pub fn ticks_through(&self, from: DateTime<Utc>, at: DateTime<Utc>, cap: u32) -> (u32, bool);
}
// spec/validate.rs: code "duplicate_trigger_id", path "triggers.{id}"
```

Shape of the two helpers (`cron` and `in_zone` are private to `ScheduleTrigger`, which is why they live there):
```rust
pub fn last_tick_at_or_before(&self, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    self.cron
        .last_before(self.in_zone(at) + Duration::nanoseconds(1))
        .map(|d| d.with_timezone(&Utc))
}

pub fn ticks_through(&self, from: DateTime<Utc>, at: DateTime<Utc>, cap: u32) -> (u32, bool) {
    let end = self.in_zone(at);
    let mut n = 0u32;
    let mut cur = self.cron.next_after(self.in_zone(from) - Duration::nanoseconds(1));
    while let Some(t) = cur {
        if t > end { break; }
        if n == cap { return (n, true); }
        n += 1;
        cur = self.cron.next_after(t);
    }
    (n, false)
}
```

**Where `duplicate_trigger_id` goes, and why it is not beside `duplicate_task_id`.** `duplicate_task_id` is raised in `validate_for_run` (`validate.rs:590-601`), which `executor::build_plan` calls at run time (`validate.rs:335-338`). A run-time error there would stop **every** execution of a stored flow that declares one trigger id twice — a behaviour change §3.8 does not ask for (it says drafts still save and the switch applies to the id). Put it in `save_only_findings`: it shows on the draft and blocks publish (`publish.rs:300` calls `validate`), and a stored flow keeps running. This is a plan decision on an ambiguity in §3.8; it is reported to the controller.

- [ ] **Step 1: Write the tests (red).**
  1. `models`: a `Schedule` with `recoverMissedSchedules: LAST` parses to `Some(json!("LAST"))` and re-serialises byte-for-byte to the same key; absent stays absent on re-serialisation (extend the round-trip test that covers `lateMaximumDelay`; find it with `grep -n late_maximum_delay crates/egret-workflow/src/models`).
  2. `RecoverMissed::from_model`: `None` → `All`; `"ALL"`, `"NONE"`, `"LAST"` → their variants; `"all"`, `true`, `1`, `"SOME"` → `Err`.
  3. `last_tick_at_or_before` with `*/15 * * * *`: at `10:07` → `10:00`; at exactly `10:15:00` → `10:15` (the case `last_before` alone gets wrong); with `timezone: Asia/Ho_Chi_Minh` and `0 2 * * *` at `2026-06-15T20:00Z` → `2026-06-14T19:00Z`.
  4. `ticks_through` with `*/1 * * * *`: `from = 10:00`, `at = 10:04:30` → `(5, false)`; `from > at` → `(0, false)`; `from = 10:00`, `at = 10:00` → `(1, false)`; with `cap = 3` over 5 ticks → `(3, true)`.
  5. `validate`: two triggers with id `t` (one `Schedule`, one `Webhook`) give exactly one `duplicate_trigger_id` error at `triggers.t`; distinct ids give none; `validate_for_run` on the same flow does **not** report it (the save-only placement is pinned).
  6. Mutation: move the check into `validate_for_run`; test 5's last assertion fails. Record it.
- [ ] **Step 2: Run them, see them fail.** `cargo test -p egret-workflow --lib recover_missed ticks_through last_tick duplicate_trigger` (adjust the filters to the names you chose).
- [ ] **Step 3: Implement.** Update the `Schedule` doc comment (`models/trigger.rs:93-96`) so it no longer lists `recoverMissedSchedules` as dropped, and say what reads it (the switch route only, §3.3). Do not change `ScheduleTrigger::resolve`: no scheduler path reads the property (§3.3, §10).
- [ ] **Step 4: Run the tests, then `cargo test -p egret-workflow --test flow_schema`** (the editor schema measures the model; a new property must not break it — if the schema lists properties, `recoverMissedSchedules` now appears, which is correct).
- [ ] **Step 5: Commit.** `feat(workflow): capture recoverMissedSchedules, schedule tick helpers, duplicate_trigger_id`

### Task 2: The switch table and `sync`'s effective value (§3.1, §3.2, §3.4, §3.6) — **opus**

**Files:**
- Create: `migrations/0189_workflow_disabled_triggers.sql`.
- Modify: `crates/egret-workflow/src/scheduler/sync.rs` (`sync`, `:48-151`; `sync_flow_listeners`, `:165-247`; the doc comment `:21-47`), `crates/egret-workflow/src/scheduler/mod.rs` (export `effective_disabled`; statement 4's doc, `:114-136`, gains one sentence: `sync` also reads `workflow.disabled_triggers` and deletes its undeclared rows).
- Test: `crates/egret-workflow/tests/scheduler_sync.rs`, `tests/flow_trigger_sync.rs`, a new `tests/disabled_triggers.rs` (enforcement on the claim and the outbox; the `EXPLAIN` test).

**Interfaces — Produces:**
```rust
// scheduler/sync.rs — the ONE function that computes the value (§3.2)
pub(crate) fn effective_disabled(flow_yaml: bool, trigger_yaml: bool, runtime: bool) -> bool {
    flow_yaml || trigger_yaml || runtime
}
// scheduler/mod.rs
pub(crate) use sync::effective_disabled;
```
Table `workflow.disabled_triggers (flow_id, trigger_id, workspace_id, disabled_by, disabled_at)`, PK `(flow_id, trigger_id)`; index `workflow_executions_last_by_trigger`.

The migration, as §3.1 and §3.6 give it, with a first line naming the renumbering:
```sql
-- 0189, not 0186: origin/main ends at 0185 and the unmerged feat/view-governance branch holds 0186-0188, so 0189 collides with neither in either merge order.
--
-- Workflow W7e: the runtime switch of a trigger. See docs/en/specs/2026-10-14-workflow-w7e-triggers-design.md §3.1.
CREATE TABLE workflow.disabled_triggers ( … as §3.1 … );
COMMENT ON COLUMN workflow.triggers.disabled IS '…as §3.1…';
COMMENT ON COLUMN workflow.flow_listeners.disabled IS '…as §3.1…';
CREATE INDEX workflow_executions_last_by_trigger
  ON workflow.executions (flow_id, (execution_trigger->>'id'), created_at DESC)
  WHERE execution_trigger IS NOT NULL;
```
Keep the index only if the `EXPLAIN` test (Step 2, test 9) shows the planner uses it; otherwise drop it from the migration and record the measurement in the migration instead, as `0183:74-86` does.

`sync`'s shape (the existing statements keep their SQL; only the bound `disabled` value changes):
```rust
// After `now` and `parsed`, before the schedules loop, in the same transaction:
let switched_off: Vec<String> =
    sqlx::query_scalar("SELECT trigger_id FROM workflow.disabled_triggers WHERE flow_id = $1")
        .bind(flow.id).fetch_all(&mut *conn).await.map_err(db_err)?;
let runtime_off = |id: &str| switched_off.iter().any(|s| s == id);
// sync.rs:89 becomes
let disabled = effective_disabled(flow_disabled, t.disabled, runtime_off(&t.id));
// sync_flow_listeners gains `switched_off: &[String]`; sync.rs:184 the same.

// Lifecycle (§3.4), after the rows:
if gone {
    sqlx::query("DELETE FROM workflow.disabled_triggers WHERE flow_id = $1") …
} else if let Some(f) = &parsed {
    let declared: Vec<String> = f.triggers.iter().map(|t| t.id.clone()).collect();
    sqlx::query("DELETE FROM workflow.disabled_triggers WHERE flow_id = $1 AND NOT (trigger_id = ANY($2))")
        .bind(flow.id).bind(&declared) …
}
// parsed == None && !gone: the source does not parse; keep every row.
```
`parsed` is `None` both for `gone` and for a source that does not parse (`sync.rs:55-59`): the branch must test `gone` first.

- [ ] **Step 1: Read** `sync.rs` whole, `scheduler/mod.rs` statements 1 and 4, `repo.rs:538-678` (`update`, `soft_delete`; both call `sync` in their transaction) and `fire.rs:179-213` (`fire_one`'s `resync` also calls `sync`). Confirm that every caller of `sync` is one of: `FlowRepo::create`, `update`, `soft_delete`, `reconcile_one`, `fire::resync`. Record the list with `file:line` in the report.
- [ ] **Step 2: Write the tests (red).** Seed through `FlowRepo` (as `scheduler_sync.rs`'s module doc requires); write `disabled_triggers` rows with SQL only to stage the switch, since the route arrives in Task 5.
  1. **Effective copy, `workflow.triggers`**, for each cause alone: flow `disabled: true`; trigger `disabled: true`; a `disabled_triggers` row then a save. Each gives `disabled = true`; none of them gives `false`. A save that keeps the cron keeps `next_evaluation_date` (switching never touches dates).
  2. **The same on `workflow.flow_listeners`**, for a window-free and a `dependsOn` Flow trigger.
  3. **Lifecycle:** a save that drops trigger id `b` deletes `b`'s row and keeps `a`'s; renaming `a` to `a2` drops `a`'s row and `a2` starts on; a `Webhook` trigger's row survives a save that keeps it (no `workflow.triggers` row exists for it); a soft delete deletes every row of that flow; another flow's rows are untouched by all of these.
  4. **A source that no longer parses keeps its rows:** write `source = 'not: [valid'` by SQL (a parser change leaves exactly this; the API cannot store it, §3.4), then run `scheduler::reconcile` — the `disabled_triggers` rows remain while the `workflow.triggers` rows are deleted, as today.
  5. **A kind change under the same id keeps the row** (a `Schedule` `t` becomes a `Webhook` `t`).
  6. **The claim honours it:** with a due schedule (`common::make_due`) switched off by a row + `reconcile`, `fire_due` fires nothing and `next_evaluation_date` is unchanged; delete the row + `reconcile`, `fire_due` fires it.
  7. **The outbox honours it:** a window-free listener switched off is the only listener of its workspace; drive an upstream execution to `SUCCESS` (`common::drive_to`) and `common::events` is empty; a second listener left on makes an event appear (control).
  8. **The listener select honours it:** two listeners on the same event, one switched off; `process_events` fires only the other (`common::fired_by`).
  9. **`EXPLAIN` of §3.6's statement** uses `workflow_executions_last_by_trigger` and has no `Seq Scan` on `workflow.executions`, over ≥ 3,000 executions of one flow (shape of `execution_timestamps.rs:1100-1162`: insert rows, `ANALYZE`, `force_custom_plan`, `PREPARE`, `EXPLAIN EXECUTE`), with a control: the same rows without the index (drop it inside the test's own database) are not served by it.
  10. **Mutations** (record which tests die): drop `runtime_off(..)` from `effective_disabled`'s call in the schedules loop (1, 6 must die); the same in `sync_flow_listeners` (2, 7, 8); drop the `gone` branch (3's soft-delete case); delete undeclared rows also when `parsed` is `None` (4).
- [ ] **Step 3: Run them, see them fail** — `cargo test -p egret-workflow --test scheduler_sync --test flow_trigger_sync --test disabled_triggers` (the tables do not exist yet).
- [ ] **Step 4: Implement** the migration and `sync`. Update `sync`'s doc comment points 1-5 with the effective value and the lifecycle. Keep the existing `INSERT … ON CONFLICT` statements' text except the bound value.
- [ ] **Step 5: Run the touched tests, then `--test scheduler_fire --test runtime`** (both stage `workflow.triggers` and must stay green).
- [ ] **Step 6: Commit.** `feat(workflow): a runtime trigger switch that sync copies into the trigger rows`

### Task 3: The webhook honours the switch; §3.10's comments (§3.2, §3.10) — **sonnet**

**Files:**
- Modify: `crates/egret-workflow/src/webhook_routes.rs` (after `:560-566`; the module doc `:31-36` and the refusal table `:109-117`).
- Modify: `crates/egret-workflow/src/routes.rs:2271-2274`, `webhook_routes.rs:567-571` (comments), `crates/egret-workflow/src/lib.rs:92-96`, `crates/egret-workspace/src/features.rs:310-313`.
- Test: `crates/egret-workflow/tests/webhook_route.rs`.

**Interfaces — Consumes:** Task 2's table. **Produces:** nothing new.

The lookup, after the YAML trigger check and before the flow check, so the order of 409s is: trigger YAML, trigger runtime, flow:
```rust
let switched_off: bool = sqlx::query_scalar(
    "SELECT EXISTS (SELECT 1 FROM workflow.disabled_triggers WHERE flow_id = $1 AND trigger_id = $2)",
)
.bind(flow_uuid)
.bind(&trigger.id)
.fetch_one(&st.db.pool)
.await
.map_err(|e| Error::Internal(anyhow::anyhow!("webhook switch lookup: {e}")))?;
if switched_off {
    return Err(Error::Conflict(format!(
        "Cannot execute webhook: the trigger '{}' is disabled.",
        trigger.id
    )));
}
```

- [ ] **Step 1: Write the tests (red).**
  1. A switched-off webhook (a `disabled_triggers` row) answers 409 with the **same body** as a YAML-disabled one (compare bodies, not just statuses), and `common::execution_count` stays 0.
  2. Deleting the row: the same request answers 200 and one execution exists.
  3. `the_four_absences_are_indistinguishable` (`webhook_route.rs:459`) extended: a switched-off trigger addressed with a **wrong** key still answers the shared 404 body.
  4. A switched-off trigger in workspace A does not affect the byte-identical flow in workspace B (its key still fires there).
  5. Mutation: delete the lookup; tests 1 and 4's A-side die. Record it.
- [ ] **Step 2: Implement; rewrite the comments** with §3.10's text:
  - `lib.rs:92-96`: the loop that drives the clock lives in `scheduler`; backfill has no route yet (W7f).
  - `routes.rs:2271-2274` and `webhook_routes.rs:567-571`: the `state` column is honoured, but nothing writes it yet; the runtime switch is per trigger (`workflow.disabled_triggers`).
  - `features.rs:310-313`: stopping webhook traffic is the trigger's runtime switch or `disabled: true` in the YAML.
  - `webhook_routes.rs:31-36` and the refusal table: add the runtime switch beside the YAML trigger row.
- [ ] **Step 3: Run `--test webhook_route`, then commit.** `feat(workflow): a switched-off webhook trigger answers 409`

### Task 4: Reading triggers — per flow and for the workspace (§3.5, §3.6) — **opus**

**Files:**
- Create: `crates/egret-workflow/src/triggers/mod.rs` (module doc: what is read, from where, the bound), `triggers/entry.rs` (pure), `triggers/state.rs` (I/O), `triggers/routes.rs` (DTOs, two handlers, the error type).
- Modify: `crates/egret-workflow/src/lib.rs` (`pub(crate) mod triggers;` beside `namespaces`, `:57`).
- Modify: `crates/egret-workflow/src/namespaces/dependencies.rs:212-221` — lift `off_runtime` to `pub(crate)` and make its join-error text neutral (`"a blocking parse stopped: {join}"`); `build_blocking` keeps calling it. Today it is a private `async fn` whose message names the dependencies graph, so §3.5's "W7d's `off_runtime`" is not callable as is.
- Modify: `crates/egret-workflow/src/routes.rs`: `WorkflowState` (`:200-251`) gains `pub trigger_flags: TriggerFlags`; `caller_may_manage` (`:1651`) becomes `pub(crate)`; two `.route(..)`s in `management_router` (`:3271-3426`); the handlers and DTOs in `paths(..)`/`components(..)` (`:3428-3510`).
- Modify: `crates/egret-workflow/src/repo.rs` — `FlowRepo::trigger_drafts` beside `dependency_drafts` (`:381-397`).
- Modify (the new field): `crates/egret-bin/src/main.rs:2302-2327` (from `config.workflow.{scheduler_enabled, worker_enabled, webhooks_enabled}`, `egret-config/src/lib.rs:596, 619, 689`), `crates/egret-server/tests/workflow_webhook_mount.rs` (its two `WorkflowState` literals, `:110` and `:144`), `crates/egret-workflow/tests/common/mod.rs` (`app_with_dbs_and_pdp`, `:396-414`; `webhook_app_with_guard`, `:773-791`), plus a helper `app_with_trigger_flags(db, flags) -> App`.
- Modify: `openapi.json`, the generated client (`npm run gen:api`).
- Test: a new `crates/egret-workflow/tests/triggers_routes.rs`; unit tests in `triggers/entry.rs`.

**Interfaces — Produces** (Tasks 5–7 rely on these names):
```rust
// routes.rs
#[derive(Debug, Clone, Copy, Default)]
pub struct TriggerFlags { pub scheduler_enabled: bool, pub worker_enabled: bool, pub webhooks_enabled: bool }

// repo.rs — flows not deleted, namespace (dot-anchored) and flow filters in SQL,
// ordered namespace, flow_id (COLLATE "C"), at most `limit` rows.
pub async fn trigger_drafts(&self, workspace: Uuid, namespace_prefix: Option<&str>,
    flow: Option<Uuid>, limit: i64) -> Result<Vec<crate::namespaces::dependencies::DraftRow>>;

// triggers/state.rs — one statement each, for the given flow ids:
pub(crate) struct Switch { flow_id: Uuid, trigger_id: String, disabled_by: Option<Uuid>,
    disabled_by_name: Option<String>, disabled_at: DateTime<Utc> }
pub(crate) struct ScheduleRow { flow_id, trigger_id, next_evaluation_date: Option<DateTime<Utc>>,
    evaluated_at: Option<DateTime<Utc>>, last_error: Option<String> }
pub(crate) struct ListenerRow { flow_id, trigger_id }
pub(crate) struct WindowRow { flow_id, trigger_id, window_start, window_end, results: serde_json::Value }
// Named `StateRows`, not `TriggerState`: `crate::trigger::TriggerState` already exists.
pub(crate) struct StateRows { switches, schedules, listeners, windows, db_now: DateTime<Utc> }
pub(crate) async fn read_state(conn: &mut PgConnection, flows: &[Uuid]) -> Result<StateRows>;
pub(crate) async fn last_executions(conn: &mut PgConnection, keys: &[(Uuid, String)])
    -> Result<HashMap<(Uuid, String), LastExecutionDto>>;   // §3.6's statement

// triggers/entry.rs — PURE: (flow row, parsed flow, state rows, may_manage) -> entries
pub(crate) struct FlowCtx<'a> { pub id: Uuid, pub ws_slug: &'a str, pub namespace: &'a str,
    pub flow_id: &'a str, pub state: &'a str, pub deleted: bool }
pub(crate) fn entries_for_flow(flow: &FlowCtx, parsed: &Flow, state: &StateRows,
    may_manage: bool, detail: Detail) -> Vec<TriggerEntryDto>;
pub(crate) enum Detail { Full, Row }   // Row omits depends_on.conditions and webhook (§3.5)
```
```rust
// triggers/routes.rs — wire shapes (utoipa::ToSchema, serde snake_case)
pub struct TriggerEntryDto {
    pub trigger_id: String, pub r#type: String, pub kind: TriggerKindDto,   // schedule|webhook|flow|other
    pub yaml_disabled: bool, pub runtime_disabled: bool,
    pub disabled_by: Option<Uuid>, pub disabled_by_name: Option<String>, pub disabled_at: Option<DateTime<Utc>>,
    pub effective_disabled: bool,
    pub can_toggle: bool, pub toggle_refusal: Option<ToggleRefusal>,         // yaml_disabled|not_supported|forbidden|flow_deleted
    pub duplicate_id: bool,
    pub schedule: Option<ScheduleInfoDto>, pub flow: Option<FlowTriggerInfoDto>,
    pub webhook: Option<WebhookInfoDto>, pub last_execution: Option<LastExecutionDto>,
}
pub struct ScheduleInfoDto { cron: Option<String>, with_seconds: bool, timezone: String /* "UTC" when absent */,
    next_evaluation_date, evaluated_at, last_error, off_queue: bool,
    missed_ticks: Option<MissedTicksDto> /* {count: u32, more: bool} */, recover_missed_schedules: Option<String> }
pub struct FlowTriggerInfoDto { states: Vec<String>, listening: bool, not_listening_reason: Option<String>,
    depends_on: Option<DependsOnDto> }
pub struct DependsOnDto { mode: String, min_satisfied: Option<i32>,
    conditions: Option<Vec<ConditionDto>> /* None in list rows */, window: Option<WindowDto> }
pub struct ConditionDto { namespace: Option<String>, flow_id: Option<String>, states: Option<Vec<String>>,
    labels: Option<serde_json::Value>, when: Option<String>, met: bool }
pub struct WindowDto { kind: String /* duration|sliding|daily_window|daily_deadline */, start, end }
pub struct WebhookInfoDto { url_path: Option<String>, key_error: Option<String>, unsupported: Option<String> }
pub struct LastExecutionDto { id: Uuid, state: String, created_at: DateTime<Utc> }
pub struct FlowTriggersDto { flow_disabled: bool, flow_deleted: bool, scheduler_enabled: bool,
    worker_enabled: bool, webhooks_enabled: bool, triggers: Vec<TriggerEntryDto> }
pub struct TriggerRowDto { flow: Uuid, namespace: String, flow_id: String, #[serde(flatten)] entry: TriggerEntryDto }
pub struct TriggerPageDto { rows: Vec<TriggerRowDto>, total: i64, limit: i64, offset: i64, unparsable: i64,
    scheduler_enabled: bool, worker_enabled: bool, webhooks_enabled: bool }
pub const TRIGGERS_TOO_LARGE_CODE: &str = "triggers_too_large";
pub enum TriggerRouteError { Core(Error), Coded { status: StatusCode, code: &'static str, message: String } }
```
`disabled_by_name` is not in §3.5's field list; it is added because §4.1's status reads "Switched off by {user}" and the browser cannot resolve a user id (the user listings are admin-gated, `repo.rs:129-132`). It is read in the same statement as the switch, with `with_creator_name!`'s expression (`COALESCE(NULLIF(u.name, ''), u.email::text)`, `repo.rs:138`). If utoipa cannot `flatten` into `TriggerRowDto` cleanly, repeat the fields; keep the JSON the same.

**What each field is read from** — re-verify each before coding:
- `kind`: `TriggerKind` (`models/trigger.rs:75-80`); `type`'s short name is the frontend's job.
- `yaml_disabled` = flow `disabled` OR trigger `disabled`; `runtime_disabled` = a `Switch` row; `effective_disabled` = `scheduler::effective_disabled(..)` OR `flow_deleted` (a trashed flow is effective-disabled, §3.5).
- `can_toggle` / `toggle_refusal`, first match wins: `flow_deleted` → `flow_deleted`; `Leaf` → `not_supported`; caller lacks `workflow:manage` → `forbidden`; `yaml_disabled` → `yaml_disabled`; otherwise `can_toggle: true`. `can_toggle` describes the **UI's switch**, which §4.1 disables for a YAML-disabled trigger. The API still accepts switching such a trigger **off** (D4, Task 5); only "on" is refused there. Say this in the `can_toggle` field's OpenAPI description.
- `duplicate_id`: the id occurs more than once in `parsed.triggers`.
- `schedule`: the YAML fields; the `workflow.triggers` row; `off_queue = next_evaluation_date IS NULL`; `missed_ticks` = `ScheduleTrigger::resolve(t)?.ticks_through(next_evaluation_date, db_now, 1000)` when on the queue and `next_evaluation_date <= db_now`, else `None` (a switched-off schedule keeps its date, so it is overdue by the same rule).
- `flow.listening` = a `flow_listeners` row exists; `not_listening_reason` = `WindowFreeFlowTrigger::resolve(t)`'s error text when it fails and `depends_on` is empty (the case `sync.rs:194-211` logs and skips).
- `flow.depends_on.conditions`: for each `spec.depends_on` entry, `trigger::condition_key(dep)` (`trigger/depends_on.rs:92`) and `met = window.results[key] == true`. `window`: `TimeWindow::from_model(spec.window.as_ref())` for `kind`; the row's `window_start`/`window_end`; a row with `window_end <= db_now` is reported as none.
- `webhook.url_path` = `/workflows/v1/{ws slug}/executions/webhook/{namespace}/{flow_id}/{key}`, the key from `trigger::webhook::render_key(t, w, &KeyContext { flow_id, namespace, vars })` built exactly as `webhook_routes.rs:550-554` builds it; `key_error` = "no key is declared" when `w.key` is `None`, else "the key does not render" when `render_key` is `None`; `unsupported` = `declared_unsupported(w).map(|u| u.message(&t.id))`. Percent-encode each path segment.
- `last_execution`: §3.6's statement, for the page only.
- Top: `flow_disabled` = YAML `disabled` OR `state = 'disabled'`; `flow_deleted` = `deleted_at IS NOT NULL`; the three flags from `st.trigger_flags`.

**The global list (D9), in order:** `require(WF_READ)` → clamp `limit`/`offset` → validate `namespace_prefix` with `spec::validate::is_valid_namespace` (400 otherwise) → `trigger_drafts(ws, prefix, flow, 2001)` → `> 2000` is 422 `triggers_too_large` → `caller_may_manage` → `read_state` for those ids → **parse and build on the blocking pool** (`off_runtime`), counting `unparsable` → filter by `kind`, `state`, `q` (case-insensitive substring of trigger id, flow id or namespace) → `total` → cut the page → `last_executions` and `missed_ticks` for the page only. Order: namespace, flow id, declaration order.

- [ ] **Step 1: Read** §3.5 and §3.6 whole, then the sources named above. Record in the report: the `execution_trigger->>'id'` a `Schedule`, a `Webhook` and a `Flow` fire each store (`fire.rs`, `webhook_routes.rs`, `flow_trigger/process.rs:1350`), which §3.6's statement relies on.
- [ ] **Step 2: Write the tests (red)** in `tests/triggers_routes.rs` (`common::app_with_trigger_flags`, `seed_workspace`, `grant`, `seed_flow`, `get`, `json`):
  1. **Per flow, one of each kind** (a `Schedule`, a `Webhook`, a window-free `Flow`, a `dependsOn` `Flow`, a `Leaf`): every field of the table above, in declaration order; `kind`, `type`, `can_toggle` and `toggle_refusal` per kind.
  2. **Schedule:** `next_evaluation_date`, `evaluated_at` and `last_error` come from the row; `off_queue` for a row with a NULL date; `missed_ticks` `{count: 3, more: false}` after `common::make_due(.., 3)`; `{count: 1000, more: true}` after setting the date 2,000 minutes back; `timezone` is `"UTC"` when absent; `recover_missed_schedules` echoes the YAML.
  3. **Switch fields:** after a `disabled_triggers` row with `disabled_by = user`: `runtime_disabled`, `disabled_by`, `disabled_by_name` (the user's name), `disabled_at`, `effective_disabled`.
  4. **Flow trigger:** `listening` true with a listener row; a `dependsOn` trigger with a window row whose `results` holds one condition key: that condition `met: true`, the other `false`, `window` `{kind, start, end}`; a window with `window_end` in the past is `window: null`.
  5. **Webhook:** `url_path` with the rendered key (a key `"k-{{ flow.id }}"` renders); a key that does not render gives `key_error` and no `url_path`; an `unsupported` declaration (`wait: true`) is named.
  6. **Last execution:** the newest of two executions of that trigger, not one of another trigger, not an API-started one (`execution_trigger IS NULL`).
  7. **Top:** `flow_disabled` for YAML and for `state = 'disabled'`; a trashed flow answers 200 with `flow_deleted: true`, every entry `effective_disabled` and `can_toggle: false`; the flags echo `TriggerFlags`.
  8. **The global list:** the `namespace_prefix` (dot-anchored: `company` matches `company.a`, not `companyx`), `flow_id`, `kind=schedule,webhook`, `state=disabled`, `q` filters; `total` and paging with `limit=1&offset=1`; `limit=500` echoes `200`; a trashed flow's triggers are absent; a flow whose source does not parse (written by SQL) is counted in `unparsable` and lists nothing; rows have no `conditions` and no `webhook`.
  9. **The bound:** 2,001 flows answer 422 `triggers_too_large`; the same workspace with `namespace_prefix` naming 10 of them answers 200 (the bound is after the filters, D9). Seed the 2,001 rows with one `INSERT … SELECT generate_series` (a parseable minimal source), not through `FlowRepo`.
  10. **Isolation and permissions:** another workspace's flow is 404 on the per-flow route and absent from the list; `workflow:read` only gives `can_toggle: false` with `forbidden`; no `workflow:read` is 403; no session is 401 (`common::anon_get`).
  11. **Off the async workers:** a unit test in `triggers/` mirroring `dependencies.rs:238` (`the_build_runs_off_the_awaiting_worker`).
  12. **Mutations** (record which tests die): read the state for **all** the workspace's flows instead of the ids in scope (10's isolation must die, or say why it does not); count `met` from any key in `results`; drop the `window_end` check.
- [ ] **Step 3: Implement.** Keep `entry.rs` pure (no `sqlx`), so the field rules are unit-testable without a database. Add the routes and schemas to `management_router` and `WorkflowApi`.
- [ ] **Step 4: Regenerate.** `cargo run -q -p egret-bin -- openapi > openapi.json`; in `frontend/`, `npm run gen:api`, then `npx tsc -b`.
- [ ] **Step 5: Run** `--test triggers_routes`, `--test webhook_route` (the harness changed), and `cargo test -p egret-server --test workflow_webhook_mount --test feature_route_coverage`.
- [ ] **Step 6: Commit.** `feat(workflow): list a flow's and a workspace's triggers with their state`

### Task 5: Switching triggers on and off, one or in bulk (§3.2 Concurrency, §3.3, §3.7) — **opus**

**Files:**
- Create: `crates/egret-workflow/src/triggers/switch.rs`.
- Modify: `crates/egret-workflow/src/triggers/routes.rs` (two handlers, request DTOs, the coded 409s), `routes.rs` (router; the bulk route with `axum::extract::DefaultBodyLimit::max(64 * 1024)`, as the KV bulk delete does at `routes.rs:3388-3396`; OpenAPI), `openapi.json`, the client.
- Test: `crates/egret-workflow/tests/triggers_routes.rs` (route behaviour), `tests/runtime.rs` (the concurrency test beside `a_save_that_meets_a_claimed_trigger_waits_and_keeps_the_advance`, `:1403`).

**Interfaces:**
- Consumes: Task 1's `RecoverMissed`, `last_tick_at_or_before`; Task 2's `effective_disabled`; Task 4's `entries_for_flow`, `read_state`, `last_executions`, `TriggerEntryDto`, `TriggerRouteError`.
- Produces:
```rust
pub const TRIGGER_DISABLED_IN_SOURCE: &str = "trigger_disabled_in_source";
pub const TRIGGER_NOT_SUPPORTED: &str = "trigger_not_supported";
pub const RECOVER_MISSED_UNKNOWN: &str = "recover_missed_unknown";

pub struct SetTriggerDisabledRequest { pub disabled: bool, #[serde(default)] pub recover_missed: bool }
pub struct BulkTriggerRef { pub flow_id: Uuid, pub trigger_id: String }
pub struct SetTriggersDisabledRequest { pub triggers: Vec<BulkTriggerRef>, pub disabled: bool,
    #[serde(default)] pub recover_missed: bool }
pub struct BulkResultDto { pub flow_id: Uuid, pub trigger_id: String, pub ok: bool,
    pub code: Option<String>, pub message: Option<String>, pub trigger: Option<TriggerEntryDto> }
pub struct BulkResultsDto { pub results: Vec<BulkResultDto> }

// switch.rs — one transaction; the caller commits.
pub(crate) struct Switched { pub flow: FlowRow, pub moved_to: Option<DateTime<Utc>> }
pub(crate) enum SwitchRefusal { NotFound, Unparsable(String), DisabledInSource, NotSupported, RecoverUnknown(String) }
pub(crate) async fn set_disabled(conn: &mut PgConnection, ws: Uuid, flow: Uuid, trigger_id: &str,
    disabled: bool, recover_missed: bool, by: Uuid) -> Result<std::result::Result<Switched, SwitchRefusal>>;
```

**The transaction, in this order** (§3.2's lock order is a save's: flow, then trigger rows):
1. `SELECT <flow_cols!()> FROM workflow.flows WHERE id = $1 AND workspace_id = $2 FOR NO KEY UPDATE` — none, or `deleted_at IS NOT NULL` → `NotFound` (a foreign id is a 404, never a 403, as `startable_flow` does, `routes.rs:2261-2270`). `flow_cols!` is `repo.rs:114`.
2. `spec::parse(&flow.source)` → `Unparsable` (400). Find the **first** trigger with `id == trigger_id` → none is `NotFound`. `Leaf` → `NotSupported`.
3. `disabled == false` and (`parsed.disabled || t.disabled`) → `DisabledInSource`. (Off is always accepted, D4.)
4. Lock the state row: `SELECT … FROM workflow.triggers WHERE flow_id = $1 AND trigger_id = $2 FOR UPDATE` for a `Schedule`; the same on `workflow.flow_listeners` for a `Flow`; nothing for a `Webhook`.
5. Off: `INSERT INTO workflow.disabled_triggers (flow_id, trigger_id, workspace_id, disabled_by) VALUES (…) ON CONFLICT (flow_id, trigger_id) DO NOTHING`. On: `DELETE` it.
6. `UPDATE` the locked row's `disabled` to `effective_disabled(parsed.disabled, t.disabled, disabled)` (no row: nothing to write).
7. On, `Schedule`, a row with `next_evaluation_date IS NOT NULL AND next_evaluation_date <= now()`: with `db_now` read in this transaction,
   - `recover_missed == false`, or `RecoverMissed::None`: `next_evaluation_date = TriggerState::initialise(&st, "", "", &FixedClock(db_now)).next_evaluation_date` (`trigger/mod.rs:245-260`, as `sync.rs:93-95` does);
   - `All`: keep the dates;
   - `Last`: `st.last_tick_at_or_before(db_now)` when it is after `evaluated_at`, else keep; with `evaluated_at` NULL, as `None`;
   - `recover_missed == true` and `RecoverMissed::from_model(..)` is `Err` → `RecoverUnknown` (409), checked before any write;
   - `evaluated_at` and `last_error` are never written; an off-queue row stays off; `recover_missed` is ignored for other kinds.
   If `ScheduleTrigger::resolve(t)` fails here, write nothing to the dates (the row is already off the queue or `fire_due` will take it off) and say so in a comment.
8. Return `Switched`; the handler re-reads the entry with Task 4's builder **in the same transaction** (so the answer is what was committed), commits, then emits the audit event (`st.audit.emit(egret_audit::mutation_for(&ctx.principal, Some(ws), "workflow_trigger:disable" | "workflow_trigger:enable", "workflow_trigger", Some(format!("{flow}/{trigger_id}")), json!({"recover_missed": .., "next_evaluation_date": moved_to})))`, as `routes.rs:787-797`).

**Bulk:** `require(WF_MANAGE)` once; more than 100 entries → 400 before any write; then each entry runs the transaction above in its own transaction, in the order given; a refusal becomes `{ok: false, code, message}` (`not_found` for a foreign or unknown flow, never a 403) and the next entry runs; one audit event per switched trigger. The API description says that a Flow-trigger pass or a webhook request that read the switch before it changed may fire once more after the route answers (§3.2).

- [ ] **Step 1: Write the tests (red).** Route tests in `tests/triggers_routes.rs`:
  1. **Off then on, per kind:** a `Schedule`, a `Webhook`, a window-free `Flow`: off writes the row and the effective copy; on deletes both; the answer is the entry with the new state; `audit_mutations` holds the event with the id `{flow}/{trigger}`.
  2. **Re-enable outcomes** (a schedule made due 5 ticks back with `make_due`, then switched off, then on): `recover_missed: false` → `next_evaluation_date` is the first tick after the transaction's `now()` and no execution exists; `true` with the property absent → dates kept; `ALL` → kept; `NONE` → as `false`; `LAST` with `evaluated_at` set → the latest tick at or before now; `LAST` with `evaluated_at` NULL → as `false`; `"SOME"` → 409 `recover_missed_unknown` and nothing written.
  3. **A not-overdue schedule** (date in the future) switched on keeps its date, whatever `recover_missed` says; **an off-queue row** stays off.
  4. **A later save keeps a skip:** after an on with `false`, a save that changes a task keeps the moved date; a save that changes the cron re-initialises it.
  5. **Refusals:** enable a trigger whose YAML says `disabled: true` → 409 `trigger_disabled_in_source` (and disabling it → 200); the flow `disabled: true` → the same 409; `Leaf` → 409 `trigger_not_supported`; an unknown id → 404; another workspace's flow → 404 (not 403); a trashed flow → 404; a source that does not parse (SQL) → 400; `workflow:read` only → 403; no session → 401.
  6. **Idempotence:** off twice → 200 twice and one row; on with no row → 200.
  7. **Bulk:** 100 entries accepted; 101 → 400 and nothing written; a mix (an ok one, a `Leaf`, a foreign flow, a YAML-disabled enable) gives one result each, in order, with the right codes, and the ok one is switched.
  8. **Duplicate ids:** two triggers `t`; the switch applies to `t` and the answer's entry carries `duplicate_id: true`.
  
  Concurrency tests in `tests/runtime.rs`, each with the hold pattern of `:1403-1520` (an advisory lock released by the test, `pg_stat_activity` waits observed, nothing timed):
  9. **A switch-off that meets a pass holding the row waits, and the pass's fire is the last:** hold a pass at its final `UPDATE` (the `w12_hold` trigger function); send the switch-off; observe it blocked on a row lock; release; the pass fired exactly once; the switch committed after it; a second `fire_due` after the clock passes the next tick fires nothing.
  10. **A save and a switch on one flow end with the switch's value:** hold a save inside its transaction after its `UPDATE workflow.flows` (a `BEFORE UPDATE` trigger on `workflow.triggers` that waits on the advisory lock, so the save holds the flow row); send the switch-off; observe it waiting on the flow row (`wait_event` `transactionid` or `tuple`); release; `workflow.triggers.disabled` is `true` and the `disabled_triggers` row exists. Then the reverse order: hold the switch (a trigger on `workflow.disabled_triggers`), send the save, release; the same end state.
  11. **Mutation:** drop `FOR NO KEY UPDATE` from step 1; test 10 must die (the lost update §3.2 describes: the save reads the old switch and overwrites the route's value). Record whether it dies 3 of 3; if it does not, record what the save's `sync` read.
- [ ] **Step 2: Run them, see them fail.**
- [ ] **Step 3: Implement** `switch.rs` and the two handlers. The coded 409s go through `TriggerRouteError::Coded` with `egret_core::http::ErrorBody { error, code: Some(..) }`, as `WriteError::DraftChanged` does (`routes.rs:158-165`). Messages: `trigger_disabled_in_source` → "This trigger is disabled in the flow's YAML; remove `disabled: true` there" (for the flow-level case: "This flow is disabled in its YAML; remove `disabled: true` there"); `trigger_not_supported` → "egret does not run this trigger type, so it has no switch"; `recover_missed_unknown` → "recoverMissedSchedules is {value}; egret knows ALL, NONE and LAST".
- [ ] **Step 4: Regenerate** `openapi.json` and the client; `npx tsc -b`.
- [ ] **Step 5: Run** `--test triggers_routes`, `--test runtime <the two new names>`, `cargo test -p egret-server --test feature_route_coverage`.
- [ ] **Step 6: Commit.** `feat(workflow): switch triggers on and off at runtime, one or in bulk`

### Task 6: The flow's Triggers tab, the details drawer and the enable dialog (§4.1–§4.3, §4.5, §4.6) — **opus**

**Files:**
- Create in `frontend/src/components/workflows/triggers/`:
  - `triggersModel.ts` (pure): `kindLabel`, `shortType(type)` (the last dotted segment), `statusText(entry, ctx)`, `switchEnablement(entry, ctx)` returning `Enablement` (`components/workflows/draftState.ts`), `needsEnableDialog(entry, now)`, `recoverDescription(raw)`, `enableBody(choice)`, `bannersFor(page)`, `webhookPath(url_path)`, `webhookNote(webhooks_enabled)`;
  - `TriggersTable.tsx` (`ui/table`, FE-PAGE-4; props `rows`, `variant: "flow" | "global"`, `selection`, `onToggle`, `onOpenDetails`, `manage`);
  - `TriggerDetailsDrawer.tsx` (`ui/sheet`), `EnableTriggerDialog.tsx` (`ui/dialog` + `ui/radio-group`), `TriggerBanners.tsx`, `useTriggerSwitch.ts` (the single and bulk mutations, invalidation, the dialog's state), `FlowTriggersTab.tsx`.
- Modify: `frontend/src/routes/workflows/FlowDetail.tsx:52-53` (`FLOW_TABS`), `:502-526` (a fourth `TabsTrigger` and `TabsContent`), `frontend/src/routes/workflows/WorkflowsRoutes.tsx:68-69` (the comment lists the tabs), `frontend/src/routes/workflows/queryKeys.ts` (`flowTriggers(id)`, under `flow(id)`; `triggers(ws, params?)`), `frontend/scripts/assert-workflows-lazy.mjs` (markers).
- Test: `frontend/src/components/workflows/triggers/__tests__/triggersModel.test.ts`, `FlowTriggersTab.test.tsx`, `TriggerDetailsDrawer.test.tsx`, `EnableTriggerDialog.test.tsx`; extend `frontend/src/routes/workflows/__tests__/fakeWorkflowServer.ts` with the four routes, recording each request's body (as it records `calls`).

**Interfaces:**
- Consumes: Task 4's and Task 5's generated hooks. orval names them from the operation ids, i.e. the Rust handler names: if the handlers are `list_flow_triggers`, `list_triggers`, `set_trigger_disabled`, `set_triggers_disabled`, the hooks are `useWorkflowListFlowTriggers`, `useWorkflowListTriggers`, `useWorkflowSetTriggerDisabled`, `useWorkflowSetTriggersDisabled` in `@/api/generated/workflow/workflow` (as `namespace_dependencies` became `useWorkflowNamespaceDependencies`). Read the generated file for the exact names and types; do not guess.
- Consumes: `StateChip` (`components/workflows/StateChip.tsx:135`, `kind="execution"`), `RelativeTime` (`components/explore/RelativeTime.tsx`), `ReasonButton`/`DisabledReasons` (`components/workflows/ReasonButton.tsx`), `EmptyState`, `ListSkeleton`, `ui/switch`, `ui/checkbox`, the editor's `focusRange` (`components/workflows/editor/EditorContext.ts:80`) with `rangeOfPath(source, "triggers.<id>")` (`editor/yamlPath.ts:273`).
- Produces for Task 7: `TriggersTable`, `TriggerDetailsDrawer`, `EnableTriggerDialog`, `TriggerBanners`, `useTriggerSwitch`, `triggersModel.ts`.

**Behaviour, per §4:**
- The tab's columns: selection, ID, Type (kind icon + `shortType`), Next run, Last evaluated (tooltip: the tick processed, not the wall-clock fire), Last execution (`StateChip` + relative time + link to `executions/:id/overview`), Status, Enabled (switch), Details.
- Status sentences (`statusText`), first match wins: "Disabled in YAML"; "Switched off by {name} {relative time}"; "Off the queue: {last_error}"; "Not listening: {reason}"; "Not run by egret" (`other`); "Key does not render"; otherwise "Active". Text, never colour alone (FE-A11Y-4).
- The switch's disabled reasons (FE-ACTION-4), from `toggle_refusal`: "Disabled in the flow's YAML", "Not run by egret", "You need workflow:manage", "The flow is in the trash".
- Off sends the PUT at once; on opens `EnableTriggerDialog` when `needsEnableDialog` (a `schedule` with `next_evaluation_date <= now` and not `off_queue`), else sends at once. The switch shows the server's state only: it moves when the answer arrives; a refusal shows the server's message (toast) and leaves it (FE-ACTION-3). Success invalidates `flowTriggers(id)` and `triggers(ws)` (FE-ACTION-2).
- The dialog: title "Enable trigger" / "Enable {n} triggers"; a radio group (FE-FORM-2) with "Skip missed runs; start at the next tick ({time})" (default) and "Recover missed runs ({n} missed)", the second described per `recoverMissedSchedules`: `ALL` (or absent) "runs every missed tick, one per scheduler pass", `LAST` "runs only the latest missed tick", `NONE` "skips them: this trigger's `recoverMissedSchedules` is NONE"; Cancel, Enable. `{n}` is `missed_ticks.count` with "1000+" when `more`. The "next tick" time is computed on the client from the cron only if a cron library is already a dependency; otherwise the option reads "Skip missed runs; start at the next tick" with no time — no new dependency (§8).
- Bulk: "Enable" and "Disable" over the selection; "Enable" with a schedule that `needsEnableDialog` opens the dialog in its bulk form; the result names each refused trigger and its message.
- The drawer (`?trigger=<id>` in the URL, FE-PAGE-2; opened from Details or the ID): the trigger's YAML block, read-only (cut from the source by `rangeOfPath`), "Edit in the editor" (navigate to the `edit` tab, then `focusRange`); Schedule (cron, timezone, next run in the trigger's zone and the viewer's, last evaluated, last error, missed ticks); Flow trigger (states; for `dependsOn` the mode, the window with time left, each condition met or not; "No window is open"); Webhook (Step 4); the last execution link.
- Banners (`bannersFor`), each with what to do, exactly §4.1's four sentences; none for a draft.
- Empty state (FE-STATE-3): "This flow has no triggers" with a link to the editor. Loading is a skeleton, error first (FE-STATE-1/2).
- Refresh: `refetchInterval: 30_000` while the tab is visible (`refetchIntervalInBackground: false`), and after each switch.
- Read-only viewer (§4.5): switches, selection and bulk buttons disabled with "You need workflow:manage".

- [ ] **Step 1: Write the tests (red).**
  1. `triggersModel`: `statusText` for each status, in precedence order; `switchEnablement` for each `toggle_refusal`; `needsEnableDialog` (overdue schedule, future schedule, off-queue schedule, webhook); `recoverDescription` for absent/`ALL`/`LAST`/`NONE`; `enableBody` for both choices; `webhookUrl` joins without a double slash.
  2. `FlowTriggersTab` against the fake server: the columns and status text for a fixture with one trigger of each kind; switching a webhook off sends `PUT …/disabled` with `{disabled: true, recover_missed: false}` and the switch moves only after the answer; a 409 leaves the switch and shows the server's message; switching on an overdue schedule opens the dialog, and each choice sends its body; a future schedule switches on with no dialog; bulk disable of two rows sends one bulk request and reports a refused row by name; the four banners from the flags and the flow; the empty state; the read-only viewer's disabled controls and reason text.
  3. `TriggerDetailsDrawer`: `?trigger=` opens it and closing removes it; per kind content; conditions met/not met; "No window is open"; the YAML block.
  4. `EnableTriggerDialog`: the radio group's names and descriptions per `recoverMissedSchedules`; "1000+"; Cancel sends nothing.
  5. `FlowDetail`'s existing tests stay green; a new case: `flows/:id/triggers` renders the tab, and an unknown tab still redirects to `edit`.
- [ ] **Step 2: Implement.** Follow `ExecutionsList.tsx`'s use of the generated hooks and `queryKeys` (`query: { queryKey }`).
- [ ] **Step 3: Bundle markers.** Add one marker per new module to `assert-workflows-lazy.mjs` (`FlowTriggersTab` and the drawer and dialog flagged `editor`: they load with the flow pages and must stay out of the list's chunk). Prove each with a scratch static import from `App.tsx` that makes the script fail naming it; revert the scratch import. Record the proofs in the report.
- [ ] **Step 4: The webhook path** (the decision above): the drawer shows `url_path` masked until "Reveal", a "Copy" button whose "Copied" is announced in an `aria-live` region, and a note that the path is served by the egret backend at whatever host the operator exposes for webhooks; with `webhooks_enabled` false, the same note plus "Webhooks are off on this server". `key_error` replaces the path with its sentence. **Do not add routing** (no Vite, nginx or Helm entry) and do not use `window.location.origin`.
- [ ] **Step 5: Frontend gates for the touched files, `npm run build`, `npm run test:bundle:workflows`, then commit.** `feat(workflows): a flow's Triggers tab with runtime switches, details and the enable dialog`

### Task 7: The global Triggers page (§4.4) — **sonnet**

**Files:**
- Create: `frontend/src/routes/workflows/TriggersList.tsx`, `frontend/src/routes/workflows/__tests__/TriggersList.test.tsx`.
- Modify: `frontend/src/routes/workflows/WorkflowsRoutes.tsx:64-82` (`<Route path="triggers" element={<TriggersList />} />` beside `namespaces`, imported statically like `NamespacesList`, `:6`), `frontend/src/components/layout/Sidebar.tsx:55-59` (`{ label: "Triggers", icon: <a Lucide icon not already used in the group, e.g. Zap>, to: "workflows/triggers", feature: "workflows" }` after Namespaces), `frontend/scripts/assert-workflows-lazy.mjs` (a `TriggersList` marker, unflagged: it lives in the area's chunk like `NamespacesList`), `WorkflowsRoutes.test.tsx`.

**Interfaces — Consumes:** Task 6's components and model; `ListPageShell` (FE-PAGE-1, as `NamespacesList.tsx:85`), `FacetSelect`/`FACET_ALL` (`components/explore/FacetSelect.tsx`, as `ExecutionsList.tsx:492-524`), `useWorkflowListNamespaces` for the namespace picker (FE-FORM-3), the flow list hook for the flow picker.

**Behaviour:**
- Filters in the URL (`useSearchParams`, FE-PAGE-2): `namespace` (prefix), `flow`, `kind` (comma list), `state` (`enabled`/`disabled`), `q`, `page`; each change resets `page`. The request maps them to `namespace_prefix`, `flow_id`, `kind`, `state`, `q`, `limit=50`, `offset`.
- Columns: Task 6's table with `variant="global"`: plus Namespace and Flow (links to the namespace and the flow's Triggers tab), Type shows the short name only.
- Bulk enable and disable as on the flow tab, rows keyed by `(flow, trigger_id)`.
- States: skeleton; error first; "No triggers match these filters" with "Clear filters" (FE-STATE-3); 422 `triggers_too_large` shows the server's message and focuses the namespace filter; `unparsable > 0` shows "{n} flows could not be read; their triggers are not listed" (write the plural, FE-COPY-4: "1 flow could not be read; its triggers are not listed").
- The same banners from the page's flags. The drawer opens with `?trigger=<flow uuid>/<trigger id>`.

- [ ] **Step 1: Write the tests (red).** URL → request mapping for each filter; paging; the filter-miss state and Clear filters; 422 message and focus; `unparsable` singular and plural; bulk across two flows sends both `(flow_id, trigger_id)` pairs; the read-only viewer; the sidebar entry's position and its `feature`; `WorkflowsRoutes` renders `triggers`.
- [ ] **Step 2: Implement, add the marker and prove it** with a scratch static import from `App.tsx`.
- [ ] **Step 3: Frontend gates for the touched files, `npm run build`, `npm run test:bundle:workflows`, then commit.** `feat(workflows): a global Triggers page with filters and bulk switching`

### Task 8: End to end, the Kestra comparison, docs, the final gate (§6 Playwright, §7) — **opus**

**Files:**
- Create: `frontend/tests/e2e/workflows-triggers.spec.ts`.
- Modify: the spec, EN and VI: the status line, §2 (each row this task measures relabelled **observed**; any new difference added), §10 if needed. Coordinate the VI edit with whoever owns `docs/vi/specs/...` at that time; never overwrite their uncommitted work.

- [ ] **Step 1: A private stack.** Never `deploy/docker-compose.dev.yml`, the shared DB, shared buckets or the kind cluster `egret`.
  - A fresh database `egret_w7e_e2e` in `egret-w7e-postgres` (:5448).
  - The backend on its own port, built from this worktree with `CARGO_TARGET_DIR` as above: `EGRET_DATABASE__URL=postgres://egret:egret@localhost:5448/egret_w7e_e2e`, `EGRET_WORKFLOW__SCHEDULER_ENABLED=true`, `EGRET_WORKFLOW__WORKER_ENABLED=true`, `EGRET_WORKFLOW__WEBHOOKS_ENABLED=true`, plus whatever `frontend/tests/e2e/README.md` lists for local admin login (use `loginLocalAdmin`, not the shared Keycloak). If the Vite proxy's hard-coded `:8080` (`vite.config.ts:41`) forces the backend onto `:8080`, first check nothing else listens there (`lsof -i :8080`); if something does, run the frontend as `npm run build && npx vite preview` with a scratch config that points the proxy elsewhere, kept out of git.
  - Any extra service in throwaway containers prefixed `w7e-`, removed afterwards.
- [ ] **Step 2: Run §6's scenarios 1-7 with real assertions** (each assertion reads the DOM or the database, not a screenshot). Scenario 3's `curl` goes to the backend's own port with `url_path`, since W7e adds no routing (decided); say so in the spec's divergences. Scenario 2 waits on the clock: assert on executions in the database, with the wait bounded by the schedule, not by sleeps tuned to a fast machine.
- [ ] **Step 3: Measure the DOM** in light and dark mode: the tab, the drawer per kind, the dialog, the global page at 1280 px and at 390 px (FE-VISUAL-4, FE-TEST-2).
- [ ] **Step 4: The Kestra comparison** on the local image `kestra/kestra` `28f59bf175e1` (`server local`, a throwaway container `w7e-kestra`, removed afterwards): §6 scenarios 1-3 and 5; in particular the runtime switch against a webhook and against a Flow trigger, enabling a YAML-disabled trigger through the API, and both dialog choices. Relabel each §2 row measured as **observed**, keep the others **read in source**, add any new difference (FE-REF-2).
- [ ] **Step 5: The final gate**, once, in order, never two suites at once, counting from full output: every backend gate, then every frontend gate (see Gates). Record the counts.
- [ ] **Step 6: Commit** the e2e spec and the EN spec (and the VI spec if you own it at that moment). `test(workflows): end-to-end triggers; docs: W7e divergences observed against Kestra`

---

## Self-review (done while writing)

- **Spec coverage:** §3.1 → Task 2; §3.2 → Tasks 2 (sync, claim, listener select, outbox), 3 (webhook), 5 (concurrency); §3.3 → Tasks 1, 5; §3.4 → Task 2; §3.5 → Task 4; §3.6 → Tasks 2 (index, `EXPLAIN`), 4 (the read); §3.7 → Task 5; §3.8 → Task 1; §3.9 → nothing built (D12); §3.10 → Task 3; §4.1–4.3, 4.5, 4.6 → Task 6; §4.4 → Task 7; §5 → Tasks 4–7; §6 → every task's tests and Task 8; §7 → Task 8; §8 → Global Constraints; §9 → no backfill code anywhere, objects left open for a `backfill` field; §10 → nothing.
- **Deviations from the spec, each stated where it is used:** migration `0189` (the number space); `duplicate_trigger_id` is save-only (§3.8's placement would stop runs); `off_runtime` lifted to `pub(crate)`; `last_tick_at_or_before` added (`last_before` is strictly before and `cron` is private); `disabled_by_name` added (§4.1 needs a name); the webhook URL is shown as a path with a copy button and a note, with no routing and no `window.location.origin` (§4.2, decided).
