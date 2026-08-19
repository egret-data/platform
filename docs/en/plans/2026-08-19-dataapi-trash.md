# Data API Trash Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Data API endpoints the trash / restore / delete-permanently lifecycle the other four Explore objects have, without ever letting a trashed endpoint keep serving traffic.

**Architecture:** A migration adds `deleted_at` and — the load-bearing part — a database CHECK that makes `state = 'live'` and `deleted_at IS NOT NULL` a state the row cannot be in. Store, routes, and UI follow.

**Tech Stack:** Rust — axum 0.8, sqlx 0.9, utoipa. Frontend — React 18, TypeScript, TanStack Query via orval, vitest.

**Spec:** `docs/en/specs/2026-08-18-data-api-frontend-rework-design.md` §6 — **phase 6 of six, the last.**

## The rule this phase exists to keep

> **A trashed endpoint must stop serving.**

Serving resolves through exactly one query — `live_snapshot` in `crates/egret-dataapi/src/serving/router.rs:912`, which selects `FROM dataapi.endpoints e JOIN dataapi.endpoint_versions v ON v.id = e.live_version_id WHERE e.workspace_id = $1 AND e.method = $2 AND e.path = $3 AND e.state = 'live'`.

**The Alerts programme shipped this defect**: a soft-deleted schedule kept firing until `deleted_at IS NULL` was added to the claim query. The same shape, one subsystem over.

**So do not rely on remembering to add a predicate.** Trash moves the endpoint out of `live`, and a CHECK constraint makes the dangerous combination unrepresentable. A predicate can be forgotten by the next person who writes a query; a CHECK cannot.

## Global Constraints

- **Trash accepts only `draft` and `disabled` endpoints.** A `live` endpoint must be disabled first. Two reasons, both from the spec: it keeps the state honest (a row saying `live` that serves nothing, or `trashed` that serves traffic, is a lie either way), and it avoids a **second** path to "stop serving" running beside `disable` — two mechanisms are two places to forget one, and the forgotten one is always the newer.
- **The refusal must be a real refusal at the API, not a disabled button.** A control the UI hides is not a rule the server enforces.
- **Restore returns an endpoint to `disabled`, never to `live`.** Coming back from the trash is not a publish; re-serving traffic should be a deliberate second action.
- **`entity_type` strings and permission names must match what already exists.** Measure them; do not infer. Seven claims in this programme asserted a state of the world without measuring, and each was caught only downstream.
- **Backend tests are DB-gated: a missing `DATABASE_URL` makes them report `ok` while asserting nothing**, distinguishable only by duration. Run against `egret_test` and read the duration. Never point a test at the dev `egret` database.
- **`openapi.json` is a committed artifact with a byte-identity gate in a different crate** (`egret-server`'s `committed_openapi_json_is_current`). Any utoipa change must regenerate it.
- **The eslint baseline is 0 errors / 177 warnings.** No msw.
- **Every assertion is mutation-checked before being believed, and a mutation that fails to compile proves nothing** — build to zero errors first, confirm, then read. Re-cut a non-compiling mutation and report the first run as inconclusive.
- **Run the full suite before committing, unpiped, from the repo root or with absolute paths.** Never pipe a test run into `head`/`tail`.
- **`cp` is aliased to `cp -i`** and silently declines to overwrite; restore with `git checkout --` and verify with `grep`.
- **Before writing an assertion, ask what constant would satisfy it.** Roughly ten fix rounds in this programme went to assertions a hardcoded implementation also passed: a one-row fixture, one user, one database in a picker, a test value identical to its placeholder, an enum where asserting the non-default value only moved which constant was unfalsifiable.

---

### Task 1: Migration and the CHECK that makes the defect unrepresentable

**Files:**
- Create: `migrations/0126_dataapi_endpoint_soft_delete.sql` (confirm the next free number yourself — `ls migrations/ | tail`)
- Test: `crates/egret-dataapi/tests/trash.rs`

**Interfaces:**
- Produces: `dataapi.endpoints.deleted_at timestamptz NULL`, an index, and a CHECK constraint.

- [ ] **Step 1: Read the siblings first**

`migrations/0019_dashboard_soft_delete.sql` and `migrations/0022_charts_datasets_soft_delete.sql` are the house pattern: a nullable `deleted_at` plus `CREATE INDEX <table>_deleted_idx ON <table> (workspace_id, deleted_at)`. Follow it.

- [ ] **Step 2: Write the migration**

```sql
ALTER TABLE dataapi.endpoints ADD COLUMN deleted_at timestamptz NULL;
CREATE INDEX dataapi_endpoints_deleted_idx ON dataapi.endpoints (workspace_id, deleted_at);

-- The serving path resolves an endpoint through exactly one query
-- (`serving/router.rs::live_snapshot`), which filters on `state = 'live'`. This
-- constraint is what makes "trashed but still serving" unrepresentable rather
-- than merely unlikely: a predicate can be forgotten by whoever writes the next
-- query, a CHECK cannot. The Alerts programme shipped the forgotten-predicate
-- version of this bug — a soft-deleted schedule kept firing.
ALTER TABLE dataapi.endpoints
  ADD CONSTRAINT dataapi_endpoints_trashed_not_live
  CHECK (deleted_at IS NULL OR state <> 'live');
```

- [ ] **Step 3: Prove the constraint refuses the dangerous row**

Create `crates/egret-dataapi/tests/trash.rs`. Follow `crates/egret-dataapi/tests/bookmarks.rs` for the fixture idiom and its DB gate.

First test: take a `live` endpoint and `UPDATE … SET deleted_at = now()` **directly in SQL, bypassing every route**. Assert the database refuses it. That is the point — the guarantee must hold against a writer that never read this plan.

- [ ] **Step 4: Run it**

Run: `DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test" cargo test -p egret-dataapi --test trash`
Expected: PASS, with a duration that proves it ran.

- [ ] **Step 5: Mutation-check**

Drop the CHECK from the migration, re-run against a fresh `egret_test`, and confirm the test fails. **A migration mutation needs the schema rebuilt** — say how you did it, since a stale schema would make this inconclusive rather than green.

- [ ] **Step 6: Commit**

---

### Task 2: Store and routes

**Files:**
- Modify: `crates/egret-dataapi/src/endpoints.rs`, `crates/egret-dataapi/src/routes.rs`
- Modify: `openapi.json`
- Test: `crates/egret-dataapi/tests/trash.rs`

**Interfaces:**
- Produces: `POST …/endpoints/{id}/trash`, `POST …/endpoints/{id}/restore`, and `deleted_at` on the list DTO. `DELETE …/endpoints/{id}` keeps its meaning: permanent.

**Measured facts — use these, they have been checked:**

- The permission constant is `EP_MANAGE` (`data_api_endpoint:manage`), the same one `delete_endpoint` and `disable` use. `crates/egret-dataapi/src/routes.rs:1043` is the delete handler; read it for the handler shape.
- `st.endpoints.scoped(ctx.workspace_id.0, id).await?` is this crate's existing workspace-ownership check. Use it — it is what makes a cross-workspace id a 404 instead of an IDOR.
- **Both new routes must emit an audit event**, `data_api_endpoint:trash` and `data_api_endpoint:restore`. This is measured, not assumed: all thirteen `audit.emit` calls in this file cover every lifecycle mutation — create, update, delete, publish, rollback, disable, enable, grant_invoke, revoke_invoke — and only the per-user bookmark routes are exempt. **`egret-bi`'s dataset trash does NOT audit**, so copying that precedent wholesale would make trash the only unaudited lifecycle change in this crate. Follow the crate, not the sibling.

Read `crates/egret-bi/src/routes.rs:906-951` for the route/DTO shape and the `?trashed=` list parameter it established — but take the authz and audit idiom from this crate.

Four behaviours, each its own test:

1. **Trashing a `live` endpoint is refused at the API** with a clear error naming disable-first. Not a 500 from the CHECK — the route must refuse before it writes, so the message is useful.
2. **Trashing a `draft` or `disabled` endpoint succeeds** and sets `deleted_at`.
3. **Restore returns the endpoint to `disabled`**, never `live`, whatever it was before.
4. **The list excludes trashed rows by default** and includes them only when asked. Follow the dataset precedent for the parameter's name and shape: `?trashed=true` lists only trashed rows.
5. **Trash and restore each emit their audit event.** Assert the action string and the resource id — an audit test that only counts events passes when the wrong action is recorded.
6. **A trashed endpoint keeps its URL slot, and the conflict says so.** `migrations/0121_dataapi_endpoints.sql:44` has `UNIQUE (workspace_id, method, path)`, and `crates/egret-dataapi/src/endpoints.rs:498` raises `"an endpoint already exists at {method} {path} in this workspace"`. After trashing `GET /orders`, that message points at a row the author cannot see anywhere — a dead end.

   **User-approved decision:** the trashed endpoint keeps the slot (so restore never fails), but the refusal must name the real cause — that a *trashed* endpoint holds the path, and that restoring it or deleting it permanently is the way forward. Detecting which case applies needs a lookup once the unique violation is caught; `is_path_conflict` at `endpoints.rs:494` is where that branch belongs.

   **The test must distinguish the two messages** — one fixture where a LIVE endpoint blocks the path, one where a TRASHED one does. A test asserting only the trashed message would pass unchanged if both cases produced it, which is the whole defect it is supposed to prevent.

**The fixture must contain a case where correct and broken disagree**: at least two endpoints, one trashed and one not, so "the list filters" is distinguishable from "the list happens to be empty".

Mutation-check each: remove the live-state refusal; make restore set `live`; drop the list's `deleted_at IS NULL`; change the audit action string to a wrong-but-plausible value.

---

### Task 3: The trash boundary holds against every state transition

**Files:**
- Modify: `crates/egret-dataapi/src/lifecycle.rs`
- Test: `crates/egret-dataapi/tests/trash.rs`

**This task replaces an earlier version of itself, and the reason matters.**

The original Task 3 said: test that a trashed endpoint stops serving, and mutation-check it by removing the `deleted_at` predicate from the serving query. Both halves were wrong, and measuring is what showed it:

- **`trash` never changes `state`** (`crates/egret-dataapi/src/endpoints.rs:851` refuses `AND state <> 'live'`). An endpoint can only reach the trash from `draft` or `disabled`, so by the time it is trashed it was already not being served. "Trashing stops serving" is a property of `disable`, and a test asserting it would pass even if trash did nothing at all.
- **There is no `deleted_at` predicate in the serving query to remove**, and there should not be one. `live_snapshot` (`crates/egret-dataapi/src/serving/router.rs:918`) filters `state = 'live'`; because the CHECK makes `(live, trashed)` unrepresentable, a `deleted_at IS NULL` there could never change an outcome — unverifiable dead code that invites a reader to believe the predicate is what protects them.

**The real danger runs the other way: a route that puts a trashed row back into `live`.** Measured — `crates/egret-dataapi/src/lifecycle.rs` contains **zero** occurrences of `deleted_at`:

- `enable` (`lifecycle.rs:1884`) calls `set_state(workspace, id, "live")` with no trash guard.
- `publish` (`lifecycle.rs:1803`) sets `state = CASE WHEN state = 'disabled' THEN 'disabled' ELSE 'live' END`.

**Without Task 1's CHECK, `enable` on a trashed endpoint would set it live and `live_snapshot` would serve it** — a trashed object serving traffic, precisely the Alerts defect. The CHECK converts that hole into a constraint violation, which is the design working. But a constraint violation surfaces as a **500**, and a 500 is not a refusal: it leaks a database error and tells the author nothing.

- [ ] **Step 1: Enumerate every transition that writes `state` or `live_version_id`**

Search `lifecycle.rs` yourself and list them; the ones already measured are `publish`, `enable`, `disable`, and `set_state`. **Check `rollback` and `update` too** — they were not measured, so treat them as unknown rather than assuming they are safe. Report what you find, including anything the list above missed.

- [ ] **Step 2: Write the failing tests, one per transition**

For each transition that could put a trashed endpoint into `live`, or that writes to a trashed endpoint at all: trash the endpoint, attempt the transition, and assert a **409 whose message names restoring first**.

**The falsifiability trap here is specific and worth stating.** Asserting only "it returned an error" passes against the 500 that exists today, which means the test would prove nothing. Assert the **status is 409**, assert it is **not 500**, and assert the message names restore. Run these before writing the guards and confirm they fail with a 500 — that is your evidence the tests bind.

Note also: publishing a trashed **`disabled`** endpoint currently **succeeds**, because the `CASE` leaves the state alone and the CHECK is therefore satisfied. It publishes a new version into the trash. That one fails silently rather than with a 500, so it needs its own test.

- [ ] **Step 3: Add the guards**

Refuse the transition when `deleted_at IS NOT NULL`, with a message naming restore as the way forward. Prefer putting the predicate **in the statement's `WHERE`** and distinguishing the reasons on the no-row path, the way `trash` (`endpoints.rs:847-873`) and `restore` already do — a separate read-then-write is a race.

- [ ] **Step 4: Run the tests**

Run: `DATABASE_URL="postgres://egret:egret@localhost:5432/egret_test" cargo test -p egret-dataapi --test trash`
Expected: PASS, with a duration proving the DB gate engaged.

- [ ] **Step 5: The serving test, for the end state**

Drive the real serving router (`crates/egret-dataapi/tests/serving_common/mod.rs`): publish an endpoint, confirm a consumer call succeeds, disable it, trash it, and confirm the consumer now gets the same answer as for an endpoint that never existed. Then **restore it and confirm it still does not serve** until explicitly enabled.

Be honest in the report about what this test does and does not prove: it pins the end state and would catch a trash or restore that resurrected serving, but the endpoint was already disabled, so it is a regression guard rather than proof that trash stops traffic. The guarantee lives in Step 3's guards plus Task 1's CHECK.

- [ ] **Step 6: Mutation-check each guard**

Remove one guard at a time, rebuild to zero errors, confirm, then read the result. Each removal must make its own test fail — and note whether it fails with a **500** (the CHECK catching it) or by **serving traffic** (nothing catching it). Those two outcomes mean very different things, and which one each guard produces is worth recording.

- [ ] **Step 7: Commit**

**Commit before your first mutation.** Task 2's implementer lost every uncommitted store edit to a `git checkout --` restore and had to re-apply eleven edits by hand.

### Task 4: The UI

**Files:**
- Modify: `frontend/src/routes/explore/dataapi/Endpoints.tsx`, `EndpointActionsMenu.tsx`
- Modify: `frontend/src/api/dataApi.ts`
- Test: `frontend/src/routes/explore/dataapi/__tests__/Endpoints.test.tsx`

Adopt the existing primitives rather than inventing anything: `ListToolbarButton` for the trash toggle, and the row menu for Move to Trash / Restore / Delete permanently. Read `frontend/src/routes/explore/Datasets.tsx` for the shape the other four pages use, and `docs/en/ui-conventions.md` §1 for what the toolbar owns.

- **Route the mutations through `frontend/src/api/dataApi.ts`** so the list invalidates; six mutations in an earlier phase toasted success over a stale list.
- **Move to Trash must be absent or disabled for a `live` endpoint**, and the reason must be visible — a control that silently does nothing is worse than one that is not there. The server refuses it regardless; this is about not offering it.
- **Bulk mode gains Move to Trash** only if every selected row can be trashed. If any is `live`, say so rather than partially succeeding.

Mutation-check: make the live-state guard a no-op, and confirm a test dies. Assert on the **payload**, not just that a mutation was called.

---

## After the plan: verify it in a browser

Every phase of this programme found a defect in a browser that no test could see.

1. **Confirm the running server is the binary you just built**, and that it has the new routes — probe behaviourally (`404` means missing, `403` means present and CSRF-guarded), not by grepping a document the ingress may not even serve.
2. **Trash a draft, reload, confirm it is gone from the list and present in Trash.**
3. **Restore it and confirm it comes back as `disabled`, not `live`.**
4. **Try to trash a live endpoint** — the control should not offer it, and if you force the request the API must refuse.
5. **Publish an endpoint, call it as a consumer, disable and trash it, then call again** — this is the rule the phase exists for, and the only place it can be seen end to end.

---

## Self-Review

**Spec coverage:** §6's migration, store, routes and serving exclusion are Tasks 1-3; the UI is Task 4. The state rule (only `draft`/`disabled` may be trashed; restore returns to `disabled`) is in Task 2's behaviours 1 and 3 and again in Task 4's UI guard, deliberately — the API is where it is enforced, the UI is where it is explained.

**Placeholder scan:** Task 2's tests are described rather than written out, with the fixture requirement named. That is the thinnest point; its four mutations are what keep them honest.

**Type consistency:** `deleted_at` is the column, the DTO field, and the list filter throughout. The two new routes are `trash` and `restore`; `DELETE` keeps meaning permanent.
