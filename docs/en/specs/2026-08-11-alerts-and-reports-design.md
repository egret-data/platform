# Alerts & Reports — design

**Status:** proposed
**Reference product:** Apache Superset, *Alerts & Reports* (`superset/reports/models.py`,
`superset/commands/report/execute.py`), checked out at `data-portal/superset`.

---

## 1. What exists today

Nothing. `Alerts` is a sidebar item under **Automate** pointing at a `<Placeholder>`
(`App.tsx:106`). There is no table, no route, no crate, no API. The whole feature is greenfield.

Three facts about egret decide the shape of this work, and all three were measured rather than
assumed:

1. **No notification transport exists.** No SMTP client (`lettre` is not a dependency), no Slack,
   no webhook sender anywhere in `crates/`.
2. **No shared scheduler exists.** The only periodic work is hand-rolled `tokio::interval` loops —
   `egret-kernel/src/reaper.rs` is the precedent and the pattern to copy.
3. **Chart images are produced in the BROWSER.** `downloadPng` reads the rendered `<svg>` out of
   the DOM (`components/chart/export-chart.ts`). There is no server-side chart rendering, so
   "email me this dashboard as a PNG" needs a headless browser — it is not a small addition to the
   alert engine, it is the largest single item in this design.

## 2. Decisions taken

| Question | Decision |
|---|---|
| Scope | **Alerts *and* Reports**, as in Superset |
| Alert condition | **SQL + validator** (Superset's model), not chart thresholds |
| Delivery | **Email (SMTP) + Webhook**; Slack deferred |
| SMTP configuration | **Per workspace**, not global |
| Report images | Headless browser service (accepted as in scope) |

Deferred deliberately: Slack native app, object-level sharing of a schedule, report `CSV`/`TEXT`
formats beyond what alerts already attach.

## 3. Information architecture

`Alerts` moves out of **Automate** and becomes an **Explore** module, labelled
**"Alerts & Reports"** — Superset's name, because the page holds both types and calling it
"Alerts" would misname half of it.

Explore items are not a hand-written list: they come from `EXPLORE_MODULES`
(`frontend/src/lib/exploreModules.ts`), the single source of truth shared with
`useDefaultExplorePath`, and each is gated by a workspace **feature entitlement** whose key is the
module key. So the move is: add `{ key: "alerts", label: "Alerts & Reports", icon: Bell, path:
"alerts" }` to that list and delete the item from the `Automate` group.

Two consequences, both wanted:

- The module inherits entitlement gating for free (`workspace_features`, migration 0094 — absence
  means enabled, so existing workspaces get it on).
- `useDefaultExplorePath` starts considering it, so a user whose only enabled module is Alerts
  lands there instead of on a dead route.

`Automate` is left holding `Workflows` alone. It stays: `Workflows` is a separate placeholder with
its own future, and collapsing the heading now would have to be undone later.

## 4. Data model

New schema `automation`. One table holds both types, as in Superset — they share ~80% of their
columns and all of their scheduling, and splitting them duplicates the scheduler.

```
automation.schedules
  id, workspace_id, type ('alert'|'report'), name, description, active
  crontab, timezone                       -- when
  -- alert only
  database_id, sql, validator_type ('not_null'|'operator'), validator_config jsonb
  last_eval_at, last_state, last_value, last_value_row jsonb
  -- report only
  chart_id, dashboard_id, report_format ('png'|'pdf'|'csv'|'text')
  custom_width, custom_height
  -- shared
  grace_period_s, working_timeout_s, log_retention_days, email_subject
  created_by, created_at, updated_by, updated_at
  UNIQUE (workspace_id, type, name)

automation.schedule_owners      (schedule_id, user_id)
automation.schedule_recipients  (id, schedule_id, kind ('email'|'webhook'), config jsonb)
automation.schedule_runs        (id, schedule_id, state, started_at, ended_at,
                                 value, error, notification_sent_at)
automation.smtp_config          (workspace_id PK, host, port, username, from_address,
                                 tls_mode, password_secret_id, updated_by, updated_at)
```

`last_state` / `schedule_runs.state` use Superset's five states verbatim, because they are the
vocabulary that makes the list page legible: `success`, `working`, `error`, `noop`
("not triggered"), `grace` ("on grace").

**Why the SMTP password is not a column.** It goes through `egret-secrets` as a typed secret and
the table stores only its id. A password in a JSONB blob is the shape that leaks through a
`SELECT *` in a support session.

## 5. Execution

### 5.1 Scheduler

One `tokio::interval` ticker per replica in a new crate `egret-automation`, started from
`egret-bin` alongside the kernel reaper. Each tick claims due schedules with

```sql
SELECT ... FROM automation.schedules
 WHERE active AND next_run_at <= now()
 FOR UPDATE SKIP LOCKED
```

`SKIP LOCKED` is what makes N replicas safe without a leader election or an external queue. This
is deliberately not a k8s CronJob per schedule: schedules are user data that change at runtime, and
a CronJob per row makes the cluster's object count a function of user behaviour.

`working_timeout_s` exists for the case this design cannot prevent: a replica that dies mid-run
leaves a row in `working`, and nothing else will ever pick it up. A run older than its timeout is
force-released on the next tick.

`grace_period_s` is the anti-spam rule: after an alert fires, it will not fire again until the
grace period has elapsed, even if the condition is still true. Without it a 1-minute alert on a
persistent condition sends 60 emails an hour.

### 5.2 Alert evaluation

Run `sql` against `database_id`, take the **first column of the first row**, then apply the
validator:

- `not_null` — fire when the value is non-null and non-zero (Superset's rule).
- `operator` — fire when `value <op> threshold` for `<`, `<=`, `>`, `>=`, `==`, `!=`, plus
  `between` / `not between` with two bounds.

**WHOSE PERMISSIONS THE QUERY RUNS WITH — the security decision of this design.** A schedule runs
with nobody logged in, and the naive implementation runs it as a service account, which turns any
alert into a privilege-escalation path: write SQL against a table you cannot read, receive its rows
by email. So:

- The query runs as the schedule's **owner**, using a per-user engine token minted by the machine
  IdP (the same token broker that already issues per-user engine tokens — see
  `docs/en/specs/*sts*`), never a shared service identity.
- Authorization is evaluated **at run time, not at save time**. If the owner loses access to the
  table, the run fails with `error` and the schedule keeps failing visibly, rather than continuing
  to deliver data the owner may no longer see.
- An owner who is deactivated disables their schedules.

### 5.3 Report rendering

A separate `egret-renderer` Deployment: headless Chromium driven by Playwright, exposing
`POST /render {url, width, height, format}` on the cluster network only (no Ingress).

The backend calls it with a URL to a dedicated print route and a **short-lived, single-object,
single-use token** minted per render: scoped to one dashboard or chart id, valid for ~60s, bound to
the schedule's owner. The renderer must never hold a standing credential that can read every
dashboard in the platform — that is the difference between a screenshot service and a data
exfiltration service.

This subsystem is large enough that it gets **its own spec** before implementation
(P4 in the plan); this section fixes only its contract and its security envelope.

### 5.4 Delivery

- **Email** — `lettre`, per-workspace SMTP, STARTTLS/implicit TLS/none. Alerts attach the
  triggering rows as CSV; reports attach the rendered PNG/PDF.
- **Webhook** — `POST` with a JSON body `{schedule, type, state, value, triggered_at, rows}`. An
  optional shared secret is sent as an `X-Egret-Signature` HMAC of the body so the receiver can
  verify origin.

**SSRF is the risk here and it is a known trap in this codebase.** A webhook URL is attacker-chosen
by construction: any workspace editor can point one at `http://169.254.169.254/…` or at an
in-cluster service. The destination must be validated **after DNS resolution** against a deny-list
of private, loopback, link-local and metadata ranges, with an explicit allow-list escape hatch for
operators who genuinely want an internal endpoint. `url-normaliser-turns-path-into-host` in this
repo's history is exactly the failure mode to avoid: a check that reads a path as a host and passes
a URL it should have rejected.

## 6. Permissions

`alert:manage` — create, edit, delete, run-now. Seeded to `ws_editor` and `ws_admin`, following
`chart:manage` (migration 0014). Read is implied by workspace membership plus the module
entitlement.

Object-level sharing of a schedule is **out of scope for v1**: schedules have owners, and an
editor sees the workspace's schedules. Adding per-schedule grants later fits the existing
`shares` machinery without a data migration.

## 7. UI

**List page** — one page, two tabs (`Alerts` | `Reports`), a row per schedule showing name, state
badge, schedule (humanised crontab), last run, owner, and an active toggle. Superset's shape; the
state badge is what makes the page worth opening.

**Editor** — a modal with the sections Superset uses, because they map to the model and users
coming from Superset will recognise them: General · Alert condition (SQL + validator) ·
Contents (chart/dashboard + format) · Schedule (crontab + timezone) · Notification (recipients) ·
Advanced (grace period, working timeout, log retention).

**Run log** — a drawer per schedule listing recent runs with state, duration, value and error.
This is the first thing anyone opens when an alert "didn't work", so it ships with the engine, not
after it.

**Workspace settings → Notifications** — SMTP host/port/credentials/from-address, with a
**Send test email** button. Untested SMTP config is indistinguishable from broken SMTP config until
an alert silently fails at 3am.

## 8. What this design does not do

- No Slack app, no PagerDuty, no SMS.
- No alert on a saved chart's value — SQL only, as chosen.
- No per-schedule object sharing.
- No backfill or replay of missed runs: a schedule that was inactive during its window does not
  catch up. Superset behaves the same way.
- No report `CSV`/`TEXT` delivery of a dashboard (only charts have a natural tabular form).

## 9. Risks

| Risk | Mitigation |
|---|---|
| Alert becomes a privilege-escalation path | Runs as owner, authorized at run time (§5.2) |
| Webhook used for SSRF | Post-DNS deny-list + operator allow-list (§5.4) |
| Renderer holds a god credential | Per-render, single-object, ~60s token (§5.3) |
| A dead replica strands runs in `working` | `working_timeout_s` force-release (§5.1) |
| Alert spam on a persistent condition | `grace_period_s` (§5.1) |
| SMTP misconfigured, failures invisible | Test-send button + run log + `error` state on the list |
| Headless Chromium cost/instability in kind | Its own Deployment, own spec, and it is the last phase — alerts ship without it |
