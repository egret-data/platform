# A schedule may only name objects in its own workspace

A report or alert in one workspace can name a chart or dashboard in another, and the
data is delivered. This is not a hypothesis: it was driven against real StarRocks with a
real SMTP sink, and the attachment that arrived carried the other workspace's row.

Everything below was measured before it was written. The probe is
`.superpowers/sdd/chart-target-scoping-probe.md`; this document cites it rather than
restating it, and marks what is **not established** as such.

## 1. What was proven

A `ws_admin` of workspace 1, holding **no grant of any kind** on workspace 2:

- stored workspace 2's `chart_id` and `dashboard_id` on a schedule — **HTTP 200**, at
  create *and* at update;
- stored workspace 2's objects through `automation.schedule_targets`, **and a UUID that
  exists nowhere at all**, because that column has no foreign key;
- received, at a real SMTP sink, a 39-byte attachment reading `"secret_col"` /
  `"WS2-CONFIDENTIAL-ROW-42"` — workspace 2's data, out of workspace 2;
- and, replaying the render token the runner itself minted, got
  `"title":"ws2 secret dashboard"` and `"rows":[["WS2-CONFIDENTIAL-ROW-42"]]` back from
  `/render/payload` and `/render/data`, which authorize on the token alone.

Separately: the FKs are `ON DELETE CASCADE`, so workspace 2 deleting **their own** chart
silently took workspace 1's schedule from one row to zero.

### Not established

That Chromium then draws those rows into the delivered PNG. The rows are already across
the API boundary at the point measured, so this changes nothing about severity — but it
was not driven, and is not claimed.

Whether a **delegated** connection's engine would refuse. What *was* measured is that
egret does not refuse: the delegated branch changes exactly one thing versus
non-delegated — a bearer for the workspace-1 owner — and dispatches the foreign SQL
anyway. There is no branch in `build_chart_report`, `run_card` or `owner_identity` that
can return "denied" on tenancy. **So any protection there is by policy, not by
construction**, and the default install ships `ranger.enabled: false` with Trino at
`access-control.name=allow-all`.

## 2. Why nothing stops it

| | |
|---|---|
| `migrations/0110:34-35` | both FKs are existence-only `ON DELETE CASCADE`, no workspace predicate |
| `migrations/0115:14-24` | `schedule_targets.object_id` has **no FK and no workspace column**; `kind` is polymorphic |
| `egret-automation/src/routes.rs` | **zero** occurrences of `chart_id` or `dashboard_id`. No object gate, at create or update |
| `egret-bi/src/chart.rs:82` | `ChartRepo::get(id)` is `SELECT … WHERE id = $1` |
| `egret-bi/src/dashboard.rs:177` | `DashboardRepo::get(id)` likewise |
| `egret-automation/src/runner.rs:583-603` | `build_chart_report` passes `chart.workspace_id` — the **chart's own** — to `compile_chart_sql` |

The non-delegated connector discards the owner and sends the connection's service user as
`X-Trino-User`, which is why `owner_identity` does not save it.

## 3. Three entry points, not one

The probe's most consequential finding, because it decides what a correct fix must cover:

- **`build_chart_report`** — `s.chart_id` only. Never reads targets. This is the `csv`
  and `text` path.
- **`render_image_report`** — `targets_for` first, `chart_id` only as a fallback when the
  target list is empty. This is `png` and `pdf`, and it **overrides** `chart_id`.
- **`render_alert_attachment`** (`runner.rs:767-800`) — mints a render token from
  `report_target`, on an **alert**. `model.rs:176-179` forbids naming *both* a chart and
  a dashboard on an alert; it never requires either and never checks ownership.

So a fix scoped to `chart_id` misses `schedule_targets`, and a fix scoped to reports
misses alerts.

## 4. The fix

### 4.1 Make the unscoped read unrepresentable

`ChartRepo::get(id)` becomes `get(id, workspace_id)`, and `DashboardRepo::get` likewise.
A caller cannot forget a parameter the compiler demands, and that property extends to the
twelfth caller nobody has written yet — which is the difference between this and a gate
each call site must remember. It is the principle this repo already records as
*unrepresentable beats unforgettable*, applied to the read rather than to a CHECK.

Measured blast radius: **11 call sites**, 8 for chart and 3 for dashboard, across
`egret-automation` and `egret-bi`. Four of them are in `render/payload.rs` — the token
path that also leaks — so the change lands on the leaking code rather than beside it.

Each caller passes **the schedule's** workspace, never the object's. A foreign object then
does not resolve, and the existing `NotRunnable("report's chart no longer exists")` path
takes over.

**The consequence, stated because it is a real behaviour change.** A schedule that
already holds a foreign target starts failing at run time with "no longer exists" rather
than being refused at save. That is the correct direction — it fails closed — but it is a
runtime failure for a row that was accepted, so §4.2 exists to stop new ones and the run
log is where an operator will meet the old ones.

### 4.2 Validate targets on the way in

`schedule_targets.object_id` cannot take a foreign key: `kind` is polymorphic, so the
column points at two different tables. It therefore gets application-level validation
instead — on create and on update, every `(kind, object_id)` must **exist** and belong to
the schedule's workspace — and a re-check before render, as defence in depth for rows
written before this ships.

The same validation covers `chart_id` and `dashboard_id`, so a save-time refusal is the
normal experience and §4.1's runtime failure is the backstop rather than the front door.

### 4.3 What this deliberately does not do

**It does not add a workspace column to `schedule_targets`.** That would duplicate a fact
that already lives on the object, and the application would still have to fill it
correctly — so it moves the bug rather than removing it.

**It does not split `schedule_targets` into per-kind tables with real FKs.** That is the
referentially cleanest shape and it is the right long-term answer, but it is a large
migration underneath an open cross-tenant disclosure, and the disclosure should not wait
for it.

**It does not change `ON DELETE CASCADE`.** Once every target is same-workspace, cascade
is correct: an object's owner deleting it should take the schedule that renders it. The
cross-tenant coupling disappears with the cross-tenant target.

**It does not fix `owner_identity`'s impersonate branch.** The probe found it mints a JWT
the connection was never configured to use, so `EXECUTE AS … WITH NO REVERT` is silently
skipped and the run fails at the engine. That is a reliability bug and it is **fail
closed** — it narrows this exposure rather than widening it, confining it to delegated
**jwt-mode** connections. It gets its own issue.

## 5. How this is proven, not asserted

The probe is the test plan. Each of these must fail before the fix and pass after:

- workspace 1 naming workspace 2's `chart_id` — refused at create, and at update;
- the same through `schedule_targets`, for both `chart` and `dashboard` kinds;
- a `schedule_targets.object_id` naming a UUID that exists nowhere — refused;
- an **alert** carrying a foreign `chart_id` — refused, because `render_alert_attachment`
  is the entry point a report-shaped fix would miss;
- a pre-existing foreign row — the render fails closed rather than delivering;
- and the negative control: a **same-workspace** target still renders and still delivers,
  because a fix that breaks the working case is not a fix.

Each authorization test must be run as a principal that is **not** an admin. An admin
passes every check, so an admin measurement would hold with the fix deleted.
