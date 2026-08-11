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

## Phase 0 — Move Alerts into Explore ✅ *(done)*

Independent of everything else, and worth landing first so the IA question is closed while the
engine is being built.

**Files** *(as built)*
- Modify: `frontend/src/lib/exploreModules.ts` — the module, **last** in nav order
- Modify: `frontend/src/components/layout/Sidebar.tsx` — dropped `Alerts` from `Automate`
- Modify: `frontend/src/App.tsx` — the `alerts` route, now wrapped in `RequireFeature`
- Modify: `crates/egret-workspace/src/features.rs` — `FEATURE_KEYS` + a `FEATURES` entry
- Test: `frontend/src/components/layout/__tests__/Sidebar.test.tsx`,
  `frontend/src/hooks/__tests__/useWorkspaceFeatures.test.tsx`, `features.rs` unit tests
- Modify: `crates/egret-workspace/tests/features_routes.rs` — catalog size read from
  `FEATURE_KEYS` instead of the literal `5`

- [x] **Write the failing test.** Asserts `Alerts & Reports` renders inside the **Explore** group
      and not `Automate` (structurally, by group containment — mere presence was already true
      before the move), and that it disappears when the `alerts` entitlement is absent.
- [x] **Run it, watch it fail** — 5 frontend + 1 backend red, all for the missing module.
- [x] **Implement** both sides. The backend catalog entry is not optional: the nav filters on the
      keys `/me/workspaces` returns, so a frontend-only change is invisible in the browser.
- [x] **Run the tests.** Full frontend suite green (4300); `useDefaultExplorePath` gained a case
      pinning that alerts, being last, can only ever be a fallback landing page.
- [x] **Mutation-check** the group assertion: moving the item back under `Automate` fails 3 tests,
      so the structural assertion is not passing on presence alone.
- [x] **Commit.**

**Carried into P1:** the `alerts` feature declares `prefix("alerts")` over routes that do not
exist yet. That is deliberate — an unmatched prefix rule is inert, whereas a route that lands
before its rule fails OPEN — but P1 must mount its router under `/api/ws/{workspace}/alerts` for
the declaration to bite.

---

## Phase 1 — Model + CRUD, no execution ✅ *(done)*

The whole feature as data: create, list, edit, delete schedules and recipients. Nothing runs yet,
which keeps the review about the model.

**Files** *(as built)*
- Create: `migrations/0110_automation_schedules.sql` — schema `automation`, **four** tables, not
  five: `smtp_config` belongs to P3, which already declares `0111_automation_smtp.sql`.
- Create: `crates/egret-automation/` — `lib.rs`, `cron.rs`, `error.rs`, `model.rs`, `repo.rs`,
  `routes.rs`. `cron.rs` was not in the plan; see below.
- Create: `crates/egret-automation-api/src/lib.rs` — the wire DTOs
- Modify: `crates/egret-server/src/lib.rs`, `crates/egret-bin/src/main.rs` — mount + state
- Modify: `crates/egret-core/src/rbac.rs` — the `alert:manage` catalog entry
- Modify: `crates/egret-controlplane/tests/catalog_drift.rs` — scan the new seam's routes
- Create: `crates/egret-automation/tests/{schedule_repo,schedule_routes}.rs`
- Frontend: `routes/explore/Alerts.tsx`, `components/alerts/{ScheduleEditor,RunLogDrawer}.tsx`,
  `api/alerts.ts`, `routes/explore/__tests__/Alerts.test.tsx`

- [x] Migration first, with the CHECK constraints written out. Every constraint was **probed
      directly against a database clone** before any Rust was written — which caught one written
      backwards (`schedule_report_needs_target` rejected exactly the valid case).
- [x] Repo tests (13, DB-gated), watched fail on `todo!()`, then the repo.
- [x] Route tests (12): authz, cross-workspace 404-not-403, validation 400s, 409 on duplicate.
- [x] Routes + DTOs; `openapi.json` and the orval client regenerated (drift test green).
- [x] Frontend list page + editor modal + run-log drawer (10 tests), behind `RequireFeature`.
- [x] Commit.

**Two additions the plan did not anticipate**

1. **`cron.rs` and two new dependencies** (`croner`, `chrono-tz`). Validating "a cron that does not
   parse is a 400" needs a parser, and `next_run_at` — which §5.1's claim query keys on — has to be
   computed on every write or P2 inherits a table of rows it can never claim. `croner` over the
   older `cron` crate because `cron` requires a leading SECONDS field: it would read the `0 9 * * *`
   a user types as *minute 9 of every hour*, firing 60× a day with no error to notice.
2. **`can_manage` on the summary DTO.** Not in the plan, but the list page cannot derive it — a
   session's `permissions` carries PLATFORM grants only, so workspace-scoped `alert:manage` never
   appears there. Without it a reader is shown Create/Edit/Delete that 403 on click, which is the
   exact defect `dashboard_can_manage_routes.rs` exists to prevent.

**Carried into P2:** `automation.schedule_runs` is written by nothing yet, so every run log is
empty and every `last_state` is NULL — the UI says so rather than implying a schedule ran and did
nothing. `next_run_at` IS populated, so P2's claim query has rows to find on day one.

---

## Phase 2 — Scheduler + alert evaluation + webhook ✅ *(done, except the live check)*

The first phase that does something. Alerts fire end to end; email waits for P3.

**Files** *(as built)*
- Create: `crates/egret-automation/src/{validator,scheduler,evaluator,runner}.rs`
- Create: `crates/egret-automation/src/deliver/{mod,destination,webhook}.rs` — `destination.rs`
  was not in the plan; the SSRF check earned its own module (see below)
- Create: `crates/egret-automation/tests/tick_end_to_end.rs`
- Modify: `crates/egret-bin/src/main.rs` — ticker beside the kernel reaper, same cancel token
- Modify: `crates/egret-config/src/lib.rs` — an `automation` section (enabled, tick interval,
  webhook allow-list)

- [x] **Validator unit tests first**, then the validator. 10 tests; mutation-checked against a
      null coerced to zero, `not_null` accepting zero, an off-by-one at `<=`, and `between` made
      exclusive.
- [x] **Scheduler claim tests**, then the scheduler. `#[sqlx::test]` for a database per test,
      rather than the shared `egret_test` — the P1 sweep had already been bitten by cross-binary
      interference.
- [x] **Evaluation runs as the owner.** Tested at both seams: `owner_identity` mints for the owner
      and nobody else, and the tick hands that identity to the engine. Mutation-checked against
      minting for a service identity and against falling back to an empty token.
- [x] Evaluator against the existing `run_query` path, behind a `QueryRunner` trait so the
      security property is assertable without an engine.
- [x] **Webhook SSRF tests before the sender**, with an injectable resolver — the interesting
      cases are exactly the ones real DNS will not reproduce on demand.
- [x] Webhook sender + HMAC. The vetted addresses are PINNED on the client and redirects are
      refused, so the checked destination is the dialled one.
- [x] Run-log writes on every path, with a `nothing_due_writes_nothing` control so the others
      cannot pass by writing rows unconditionally.
- [ ] **Live check on kind** — NOT DONE. Everything above is verified against fakes.
- [x] Commit.

**Two corrections to this plan, made while building**

1. **`https://evil.test/http://internal/` is NOT rejected**, contrary to the checklist above.
   That request goes to `evil.test`, an ordinary public host; blocking it would be substring
   matching rather than address checking, and a check that fires on the wrong signal cannot be
   reasoned about. The genuinely dangerous shape — tested instead — is
   `https://hooks.example.com@169.254.169.254/`, where the authority's host IS the metadata
   address and a naive parser sees the trusted name first.
2. **Grace keys on FIRING (`state='success'`), not on delivery.** Keying on
   `notification_sent_at` would make an alert whose only recipients are email — i.e. every alert
   until P3 — re-fire on every tick and fill the run log with a success a minute.

**Carried into P3/P4:** reports are claimed and evaluated by nothing yet (the tick runs alerts
only); an email recipient is skipped with no error until P3 wires SMTP.

**Watch for:** a scheduler test that passes because nothing was due. Assert on the run log, not on
the absence of an error. *(This bit: the first concurrency test passed with and without
`SKIP LOCKED`, because plain `FOR UPDATE` blocks and then returns empty. It now asserts the second
claimer returns inside 500ms.)*

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
