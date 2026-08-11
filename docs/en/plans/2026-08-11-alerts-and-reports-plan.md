# Alerts & Reports — implementation plan

> Design: `docs/en/specs/2026-08-11-alerts-and-reports-design.md`

**Goal:** ship Superset-style Alerts & Reports in egret, as an Explore module.

**Shape:** six phases, each merging on its own and leaving the product working. The order is
chosen so that **the thing users can actually use arrives before the thing that is hardest to
build**: end-to-end alerts over webhook land in P2, and the headless renderer — the largest and
riskiest item — is last, so a delay there delays nothing else.

**Scope check.** This is more than one subsystem: a scheduler, a query-execution path, two
delivery transports, a browser-rendering service and a UI. P4 (renderer) gets **its own design
spec** before implementation; the rest are covered by the design above.

---

## Phase 0 — Move Alerts into Explore *(small, ships immediately)*

Independent of everything else, and worth landing first so the IA question is closed while the
engine is being built.

**Files**
- Modify: `frontend/src/lib/exploreModules.ts` — add the module
- Modify: `frontend/src/components/layout/Sidebar.tsx` — drop `Alerts` from the `Automate` group
- Modify: `frontend/src/App.tsx:106` — keep the `alerts` route, still a `Placeholder` for now
- Test: `frontend/src/components/layout/__tests__/Sidebar.test.tsx`,
  `frontend/src/lib/__tests__/exploreModules.test.ts` (or nearest existing)

- [ ] **Write the failing test.** Assert `Alerts & Reports` renders under the **Explore** heading
      and is absent from `Automate`; and that it disappears when the `alerts` feature is disabled
      (the entitlement path is the part that silently breaks).
- [ ] **Run it, watch it fail** for the right reason (item still under Automate).
- [ ] **Implement**: add `{ key: "alerts", label: "Alerts & Reports", icon: Bell, path: "alerts" }`
      to `EXPLORE_MODULES`; remove the `Automate` entry.
- [ ] **Run the tests.** Also check `useDefaultExplorePath` tests still pass — the module list is
      shared with it, and adding an entry changes which module a restricted user lands on.
- [ ] **Commit.**

---

## Phase 1 — Model + CRUD, no execution

The whole feature as data: create, list, edit, delete schedules and recipients. Nothing runs yet,
which keeps the review about the model.

**Files**
- Create: `migrations/0110_automation_schedules.sql` (schema `automation`, five tables per §4)
- Create: `crates/egret-automation/` — `lib.rs`, `model.rs`, `repo.rs`, `routes.rs`
- Create: `crates/egret-automation-api/src/lib.rs` — DTOs (`ScheduleDto`, `CreateSchedule`, …)
- Modify: `crates/egret-server/src/lib.rs` — mount the router, register the OpenAPI paths
- Modify: `migrations/…` — seed `alert:manage` to `ws_editor`/`ws_admin` (copy 0014's shape)
- Create: `crates/egret-automation/tests/schedule_repo.rs`, `…/tests/schedule_routes.rs`
- Frontend: `src/routes/explore/AlertsPage.tsx`, `ScheduleEditor.tsx`, `src/api/automation.ts`

- [ ] Migration first, with the CHECK constraints written out (`type`, `validator_type`,
      `report_format`, the five states). A state written by a typo is a row nothing will ever pick
      up.
- [ ] Repo tests (DB-gated, following `crates/egret-bi/tests/dashboard_repo.rs`): create, list by
      workspace, update, delete, and **the unique constraint on (workspace, type, name)**.
- [ ] Repo implementation.
- [ ] Route tests: authz (`alert:manage` required to write), workspace scoping (a schedule of
      another workspace is a 404, not a 403 — matches the rest of the API), validation (a cron that
      does not parse is a 400, an alert with no SQL is a 400, a report with neither chart nor
      dashboard is a 400).
- [ ] Routes + DTOs; regenerate `openapi.json` and the orval client.
- [ ] Frontend list page + editor modal against the real API, with the entitlement gate.
- [ ] Commit.

**Watch for:** `openapi.json` drift is a committed-file gate in this repo. Regenerate it in the
same commit, and verify with `shasum`, not with the `rtk diff` wrapper — it has reported
byte-different files as identical.

---

## Phase 2 — Scheduler + alert evaluation + webhook

The first phase that does something. Alerts fire end to end; email waits for P3.

**Files**
- Create: `crates/egret-automation/src/scheduler.rs`, `evaluator.rs`, `validator.rs`
- Create: `crates/egret-automation/src/deliver/webhook.rs`, `deliver/mod.rs`
- Modify: `crates/egret-bin/src/main.rs` — spawn the ticker beside the kernel reaper
- Create: `crates/egret-automation/tests/{validator,scheduler,webhook_ssrf}.rs`

- [ ] **Validator unit tests first** — pure functions, no DB: `not_null` on null/0/non-zero, every
      operator, `between` inclusive bounds, and a null value against `>` (must not fire).
- [ ] Validator implementation.
- [ ] **Scheduler claim test**: two concurrent claimers must not both take the same row
      (`FOR UPDATE SKIP LOCKED`); a row in `working` past `working_timeout_s` is force-released;
      a run inside `grace_period_s` returns `grace` and sends nothing.
- [ ] Scheduler implementation (`tokio::interval`, cancellation token, copying
      `egret-kernel/src/reaper.rs`).
- [ ] **Evaluation runs as the owner** — test that a schedule owned by a user without access to the
      table fails with `error` rather than returning rows. This is the privilege-escalation guard
      from §5.2 and it is the single most important test in this plan.
- [ ] Evaluator implementation against the existing query path, with an owner-scoped engine token.
- [ ] **Webhook SSRF tests before the sender**: `169.254.169.254`, `127.0.0.1`, `10.0.0.0/8`, a
      public hostname whose DNS resolves into a private range, and a URL of the shape
      `https://evil.test/http://internal/` (the recorded `url-normaliser` trap). All rejected;
      an allow-listed internal host accepted.
- [ ] Webhook sender + HMAC signature.
- [ ] Run-log writes on every path: success, error, noop, grace.
- [ ] **Live check on kind**: create an alert whose SQL is `SELECT 1`, point it at a webhook
      listener, watch it fire, then watch the grace period suppress the next tick.
- [ ] Commit.

**Watch for:** a scheduler test that passes because nothing was due. Assert on the run log, not on
the absence of an error.

---

## Phase 3 — Per-workspace SMTP + email delivery

**Files**
- Create: `migrations/0111_automation_smtp.sql`
- Create: `crates/egret-automation/src/deliver/email.rs`, `smtp_config.rs`
- Modify: `crates/egret-secrets` usage for the password
- Frontend: `src/routes/explore/settings/Notifications.tsx` (Workspace settings tab)

- [ ] Config repo + route tests, including **that the password never appears in a GET response**
      (assert on the serialized body, not on the struct).
- [ ] SMTP config CRUD, password stored as a typed secret, id referenced.
- [ ] `lettre` sender: STARTTLS / implicit TLS / none; CSV attachment of the triggering rows.
- [ ] **Send test email** endpoint + button — the only thing that distinguishes "configured" from
      "configured correctly".
- [ ] Live check on kind with a mail catcher (mailpit) as the workspace's SMTP host.
- [ ] Commit.

---

## Phase 4 — Report rendering *(own spec first)*

**Before writing code**, write `docs/en/specs/…-report-renderer-design.md` covering: the print
route, the per-render token (single-object, single-use, ~60s), image sizing, the k8s Deployment,
resource limits, and what happens when the renderer is down.

Sketch of the work: `egret-renderer` image (Playwright + Chromium), `POST /render`, cluster-only
Service, a `/embed/print/:kind/:id` route in the SPA that renders one object chrome-free, token
minting in the backend, PNG then PDF, and report delivery wired into P3's email sender.

**Do not start this phase until P2 and P3 are merged.** Alerts are useful without it; it is not
useful without them.

---

## Phase 5 — Run log UI, retention, polish

- [ ] Run-log drawer on the list page (state, duration, value, error).
- [ ] `log_retention_days` pruning, inline at insert like `MAX_REVISIONS_PER_DASHBOARD`.
- [ ] Humanised crontab in the list ("every day at 09:00"), timezone shown.
- [ ] "Run now" action; active toggle from the list row.
- [ ] Empty states and the entitlement-disabled state.

---

## Standing rules for every phase

- **Mutation-check the tests that matter.** Break the guard, watch the test go red, restore. This
  repo has shipped six green-but-empty assertions; the validator, the SSRF guard and the
  owner-permission check are the three where a passing-but-blind test would be worst.
- **Live-check on kind before claiming a phase works.** Every defect that mattered in the chart
  programme was found in a browser, not by the suite.
- **One gate per command** — never chain gates through a pipe into the rtk wrapper; a swallowed
  pipe silently skips the steps behind it.
- **Regenerate `openapi.json` + the orval client** in the same commit as any DTO change.
