# Report Target Workspace Scoping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a schedule unable to name a chart or dashboard outside its own workspace, closing a cross-tenant data disclosure that has been proven live.

**Architecture:** Two halves that cover each other. The read side changes `ChartRepo::get(id)` to `get(id, workspace_id)` and `DashboardRepo::get` likewise, so an unscoped read stops compiling — that reaches all three leaking entry points at once, including the twelfth caller nobody has written. The write side validates every target on create and update, so the normal experience is a refusal at save rather than a failure at run.

**Tech Stack:** Rust (axum, sqlx, Postgres).

**Spec:** `docs/en/specs/2026-08-23-report-target-workspace-scoping-design.md` (English canonical; Vietnamese mirror alongside). The evidence it cites is `.superpowers/sdd/chart-target-scoping-probe.md` — **read that probe before starting**; it is the test plan.

**Branch:** `fix/report-target-workspace-scoping`, off `main` at `fce564e9`. This is a pre-existing defect and must not be stacked behind the three open feature PRs (#534, #535, #536).

## Global Constraints

- **Every authorization test runs as a principal that is NOT an admin.** An admin passes every check, so an admin measurement holds with the fix deleted. Migration `0110:134` grants `alert:manage` to `ws_editor` and `ws_admin`; `ws_editor` is the principal that separates the checks.
- **A workspace argument is always the SCHEDULE's workspace, never the object's.** Passing `chart.workspace_id` is what the defect *is*.
- **`cargo test --workspace` stops at the first failing binary** and is not evidence of anything. Run the crates you touched explicitly.
- **The database ruling.** `.env` points `DATABASE_URL` at the **dev** database `egret`; never point a test there. **Always `export DATABASE_URL=<the same URL with the database name swapped to egret_test>`.** `#[sqlx::test]` calls `dotenvy::var("DATABASE_URL")` unconditionally, so with nothing exported it falls back to `.env` and writes a `_sqlx_test` schema into the dev database. If you hit `VersionMismatch`, `egret_test` carries a checksum from another branch — drop and recreate it, and say so.
- Backend gate before every commit: `cargo fmt --all -- --check`, `cargo clippy -p <crates touched> --all-targets -- -D warnings`, and the tests for those crates.
- **Do not** change `ON DELETE CASCADE`, do not add a workspace column to `schedule_targets`, do not split it into per-kind tables, and do not fix `owner_identity`'s impersonate branch. Spec §4.3 says why for each.

## File Structure

| File | Responsibility |
|---|---|
| `crates/egret-bi/src/chart.rs` | **Modify.** `get(id)` → `get(id, workspace_id)`. |
| `crates/egret-bi/src/dashboard.rs` | **Modify.** Same. |
| `crates/egret-bi/src/routes.rs` | **Modify.** 6 call sites, which already hold a workspace context. |
| `crates/egret-automation/src/render/payload.rs` | **Modify.** 4 call sites — the render-token path that leaks. |
| `crates/egret-automation/src/runner.rs` | **Modify.** 1 call site, plus the three entry points' re-check. |
| `crates/egret-automation/src/routes.rs` | **Modify.** Write-time validation on create and update. |
| `crates/egret-automation/src/repo.rs` | **Modify.** The two `schedule_targets` INSERTs at `:770` and `:783`. |

**Task order:** the getters first, because the compiler then lists every site that needs a workspace and the rest of the work is mechanical against that list. Write-time validation second. The entry points third, once both halves exist.

---

### Task 1: Make the unscoped read stop compiling

**Files:** `crates/egret-bi/src/chart.rs`, `crates/egret-bi/src/dashboard.rs`, and every caller the compiler names.

**Interfaces:**
- Produces: `ChartRepo::get(&self, id: Uuid, workspace_id: Uuid)` and `DashboardRepo::get(&self, id: Uuid, workspace_id: Uuid)`, each `WHERE id = $1 AND workspace_id = $2`. Tasks 2 and 3 call these.

- [ ] **Step 1: Establish the call-site list from the compiler, not from grep**

Change both signatures, then `cargo check --workspace` and record every error. The measured count is **11** — 8 chart, 3 dashboard — but take the compiler's list, not that number: a `grep` misses calls split across lines, which is how the first count of these came out wrong.

- [ ] **Step 2: Pass the schedule's workspace at every automation call site**

`runner.rs` and `render/payload.rs` are the four-plus-one that leak. **Each must pass the schedule's workspace.** If a call site does not have it in scope, that is a finding worth reporting rather than a reason to reach for the object's own workspace — say so and stop rather than passing `chart.workspace_id`.

- [ ] **Step 3: Pass the request's workspace at every `egret-bi` call site**

The six in `routes.rs` are the interactive API and already resolve a workspace from the path. Confirm each is the caller's workspace and not something derived from the object.

- [ ] **Step 4: Write the failing test first, then confirm the fix**

Seed a chart in workspace 2. Assert `ChartRepo::get(chart_id, workspace_1)` returns `None` while `get(chart_id, workspace_2)` returns it. Same for dashboards. Run it against `egret_test`, and **prove it ran rather than skipped** — a DB-gated test in these crates self-skips while reporting `ok`; a contrastive run (skip lines and 0.00s without `DATABASE_URL`, none with it) is the proof used elsewhere in this repo.

- [ ] **Step 5: Verify the parameter is load-bearing**

Change the `WHERE` back to `id = $1` alone, `cargo build`, **confirm zero errors before reading any test result**, then confirm the test fails. Restore. A mutation that does not compile proves nothing.

- [ ] **Step 6: Gate and commit**

```
export DATABASE_URL=<the egret_test url>
cargo fmt --all -- --check
cargo clippy -p egret-bi -p egret-automation --all-targets -- -D warnings
cargo test -p egret-bi
cargo test -p egret-automation
```

---

### Task 2: Refuse a foreign target at save

**Files:** `crates/egret-automation/src/routes.rs`, `crates/egret-automation/src/repo.rs`

**Interfaces:**
- Consumes: Task 1's scoped getters.

- [ ] **Step 1: Read what the write path accepts today**

`repo.rs:770` and `:783` are the two `schedule_targets` INSERTs; `:601` is the DELETE that precedes a rewrite. Read all three, and read how `chart_id`/`dashboard_id` reach the row. Record in your report which handler owns each.

- [ ] **Step 2: Write the failing tests**

As a **`ws_editor`** of workspace 1 — not an admin — each of these must be refused, and each must be a separate test:

- `create` a report naming workspace 2's `chart_id`;
- `create` a report naming workspace 2's `dashboard_id`;
- `update` an existing workspace-1 report to point at either;
- `create` with a `schedule_targets` entry of kind `chart` naming workspace 2's chart;
- the same for kind `dashboard`;
- a `schedule_targets` entry whose `object_id` **exists nowhere at all** — the probe stored one, because the column has no foreign key.

And the negative control, which is not optional: **a same-workspace target still saves.** A fix that refuses the working case is not a fix.

- [ ] **Step 3: Validate on create and on update**

Every `(kind, object_id)`, plus `chart_id` and `dashboard_id`, must **exist** and belong to the schedule's workspace. Use Task 1's getters — they answer both questions in one call, which is the point of doing Task 1 first.

Refuse with a 4xx that names the field. Consider whether a foreign object and a nonexistent one should be distinguishable in the response: elsewhere in this codebase they deliberately are not, because the difference is an existence oracle. Follow that, and say in your report that you did.

- [ ] **Step 4: Verify the checks are load-bearing**

Short-circuit each new check in turn, `cargo build`, **confirm zero errors first**, then record exactly which tests fail. Restore each. If short-circuiting a check fails no test, that check is unpinned.

- [ ] **Step 5: Gate and commit**

---

### Task 3: Close the three entry points against rows that already exist

**Files:** `crates/egret-automation/src/runner.rs`

**Interfaces:**
- Consumes: Task 1's getters.

Task 2 stops new foreign rows. Rows written before this ships are still there, and there are **three** entry points that read them — this is the finding that decides whether the fix is complete:

- `build_chart_report` — `s.chart_id` only, never targets. The `csv` and `text` path.
- `render_image_report` — `targets_for` first (`runner.rs:311`), `report_target` only as a fallback when the list is empty. The `png` and `pdf` path, and it **overrides** `chart_id`.
- `render_alert_attachment` (`runner.rs:767-800`) — mints a render token from `report_target`, on an **alert**. `model.rs:176-179` forbids naming *both* a chart and a dashboard on an alert; it never requires either and never checks ownership.

- [ ] **Step 1: Write the failing tests**

Insert a foreign target **directly by SQL** — the routes refuse it after Task 2, which is the point. Then assert each of the three entry points fails closed rather than rendering:

- a `csv` report whose `chart_id` is foreign;
- a `png` report whose `schedule_targets` row is foreign;
- an **alert** whose `chart_id` is foreign — this is the one a report-shaped fix misses.

Assert the run is recorded as an error and **nothing was delivered**. A test that only asserts the run state would pass if the delivery happened anyway.

- [ ] **Step 2: Make each entry point use the scoped getter**

With Task 1 done, this is mostly mechanical: the getter returns `None` for a foreign id and the existing `NotRunnable("report's chart no longer exists")` path takes over. **Confirm that is what happens rather than assuming it** — read the error mapping and say in your report what an operator sees in the run log.

- [ ] **Step 3: The render token**

`render_alert_attachment` mints a token from `(kind, object_id, owner)` with no workspace. Once the object cannot be fetched cross-workspace the token is useless, but check whether the mint happens **before** or **after** the fetch. If before, a token is minted for an object that will not resolve — say whether that matters and what you did.

- [ ] **Step 4: Verify, gate, commit**

Short-circuit each entry point's check in turn, build clean first, confirm the matching test fails, restore.

---

## The live re-run, which is what actually closes this

The probe proved the defect against real StarRocks with a real SMTP sink. The fix is not finished until the same probe comes back refused.

After Task 3, re-run the probe's scenarios end to end: create the two workspaces, the foreign chart and dashboard, and attempt each of the three entry points. What must change: the 200s become refusals, and no attachment arrives. What must **not** change: a same-workspace report still renders and still delivers.

Also replay the render-token step — `/render/payload` and `/render/data` returned workspace 2's title, spec and rows to a workspace-1 token. That must stop.

## Self-review notes

Checked against the spec before saving:

- §4.1 (unrepresentable read, 11 measured sites) — Task 1, with the list taken from the compiler rather than from the number.
- §4.2 (write-time validation for all three columns, existence *and* workspace) — Task 2, including the nonexistent-UUID case the probe actually stored.
- §3 (three entry points) — Task 3, one test each, with the alert door called out because it is the one a report-shaped fix misses.
- §4.1's stated consequence (pre-existing rows fail at run rather than at save) — Task 3 Step 2 makes the operator-visible message a reported fact rather than an assumption.
- §5 (every case, plus the negative control, plus the non-admin principal) — Task 2 Step 2 and Task 3 Step 1, and the Global Constraints.
- §4.3's four exclusions — Global Constraints, so no task reaches them.
