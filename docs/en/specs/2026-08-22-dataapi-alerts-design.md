# Data API endpoint alerts, and the Explore primitives adoption it forces

Three deliverables that are one piece of work. Data API needs failure alerting, the
alert editor is where its configuration goes, and that editor sits on a page whose
list view never adopted the conventions this repo wrote down for itself. Adding a
section to a page built on the older idiom would make the page read as two idioms at
once, so the adoption comes first and the feature lands on clean ground.

The work was commissioned by a comparison against DBAPI Enterprise Edition
(72-page product guide, read in full). That comparison is Appendix A, because it is
what decided the scope — including the four things it decided **not** to build.

## 1. Where this came from

DBAPI is a low-code tool that turns SQL plus parameter configuration into an HTTP
API. egret's Data API is the same idea with a governed engine underneath. Measured
against DBAPI's feature list, egret is at or ahead of parity on the core —
endpoint definition, publish/version lifecycle, client credentials, rate limiting,
caching, access logging, OpenAPI, Try-it — and behind on breadth.

Four of DBAPI's breadth features were considered and **rejected**, with reasons, in
Appendix A: the HTTP proxy executor, the Elasticsearch executor, the MCP server, and
the API orchestration canvas. One was rejected because it already exists in a better
form: `render.rs` gives egret MyBatis-style conditional SQL with none of MyBatis's
injection surface (§A.2).

What survived is eight packages. This spec covers the first three; the other five are
named in §7 so the sequence is on the record.

| | Package | This spec |
|---|---|---|
| 1 | **U** — four Explore list pages onto the shared primitives | §3 |
| 2 | **E** — the Alerts editor onto `ConfigTable` | §4 |
| 3 | **Z2** — Data API endpoint alerts | §5 |
| 4 | Z1 — IP allow/deny and public endpoints | §7 |
| 5 | M — multiple SQL statements per endpoint (read-only) | §7 |
| 6 | C — parameter validation rules and response transformation | §7 |
| 7 | D — write statements and transactions | §7 |
| 8 | Z3 — endpoint configuration import/export | §7 |

Each package is a separate PR. U and E change no behaviour; Z2 adds a feature. They
are separated so that a regression in one is revertible without losing the others,
and so a reviewer of the feature is not also reviewing 1,788 lines of refactor.

## 2. What was measured, and what was not

Every claim below was measured on the tree at the time of writing rather than
recalled. Two things this spec expected to find and did **not** are recorded in §3.2,
because a defect list that only grows is a list nobody checked.

Two inherited numbers are deliberately **not** carried forward: the previous plan's
eslint baseline ("0 errors / 178 warnings") and any count in the conventions
document's Adoption status. Both must be re-measured at the start of implementation.
Quoting a number measured at another revision as if it were current is the specific
failure this repo has already recorded twice.

---

## 3. U — four Explore list pages onto the shared primitives

### 3.1 This is a commissioned deferral, not a new idea

`docs/en/specs/2026-08-18-data-api-frontend-rework-design.md` §10 lists it:

> **Migrating Datasets / Charts / Dashboards / Alerts onto the primitives.** The point
> of doing Data API first is that it cannot regress anything; migrating four working
> pages is a separate risk with a separate review.

and `docs/en/plans/2026-08-18-explore-list-primitives.md` carries it as a global
constraint:

> **Do not touch `Datasets.tsx`, `Charts.tsx`, `Dashboards.tsx` or `Alerts.tsx`.**
> Migrating them is a separate risk with a separate review (spec §10). This plan
> proves the primitives on pages that have none of this today, precisely so nothing
> working can regress.

That separate review is this package. The primitives have now been proven on five
Data API routes, which is the precondition the deferral was waiting for.

### 3.2 The measurement

```text
                 aria-pressed  ListPageShell  ListStates  ListToolbarButton   lines
Alerts.tsx                  0              0           0                  0     687
Datasets.tsx                0              0           0                  0     387
Charts.tsx                  0              0           0                  0     333
Dashboards.tsx              0              0           0                  0     381
```

**The confirmed defect is `aria-pressed`.** Engaged state is carried only by
`variant={onlyBookmarked ? "secondary" : "outline"}` — a visual class. A screen
reader announces "Bookmarks" identically whether the filter is engaged or idle. This
is §1.3 of the conventions document, which named these four pages as the source of
the rule and recorded that none of them set the attribute.

**Two things this spec expected to find and did not.** Both were checked before being
written down as findings, and both are already correct:

- The Trash toggle is **not** gated on `hasData` on any of the four pages — it sits
  outside every condition. §1.2's "keeps the way OUT of an empty trash" already holds.
- Entering the trash view clears the bookmark filter and bulk mode
  (`setOnlyBookmarked(false); bulk.setBulkMode(false)`). §1.2's second half already
  holds too.

Every toolbar button also already carries an `aria-label`, so the problem is state,
not name.

A detail that shows the primitive was designed for these pages: `Star` and `Trash2`
apply `fill-current` when engaged, `ListChecks` and `FolderPlus` do not. That is
exactly why `ListToolbarButton` separates `filled` from `active` (§1.3) — the
conversion needs no new prop for it.

### 3.3 The conversion

Per page: `ListPageShell` (§1.1), `ListStates` (§2), `ListToolbarButton` (§1.3),
`EmptyState` (§2.4). No behaviour changes.

`Alerts.tsx` hand-writes the header the shell exists to own:

```tsx
<div className="p-4">
  <div className="mb-4 flex items-center justify-between gap-2">
    <h1 className="text-lg font-semibold">…</h1>
```

and branches its states in a different order from the documented one
(`isError ? … : isLoading ? … : hasData ? …`, where §2 specifies
`isLoading → isError → !hasData → rows`).

**Alerts' ordering divergence is not claimed as a bug.** Under TanStack Query the two
flags rarely hold together, and no failing case was reproduced. It is recorded as a
divergence that consolidating on `ListStates` makes moot, so that a later reader does
not mistake the absence of a bug report for the absence of a check.

**`Dashboards.tsx` is a different matter, and it is a real defect.** It branches
`isLoading ? … : hasData ? … : isError ? …` — `hasData` **before** `isError` — which
is precisely the drift §2.1 of the conventions document records:

> `Dashboards.tsx` checked `hasData` before `isError`, so a failed refetch that still
> held cached rows rendered the table and swallowed the error, while the other five
> rendered the alert.

It is still present. `Datasets.tsx` and `Charts.tsx` both use the documented order, so
this is one page, not four. **U is therefore not purely a refactor: it fixes a
swallowed error.**

**A second fix all four pages get for free.** Every one of them renders
`(error as Error)?.message ?? "Failed to load X."`. A message-less error — an
`UnauthorizedError`, or an `ApiError` built from an empty `statusText` — produces `""`,
and `??` renders that as a blank destructive alert. `ListStates` uses `|| errorText`
for exactly this reason.

### 3.4 The one structural risk: the Alerts tab bar

`Alerts.tsx` renders a `Tabs` switcher (Alerts / Reports) **between the header and the
table**. `ListPageShell` has a `toolbar` slot and an `action` slot, both inside the
header row, and neither is where a tab bar goes.

**Resolution: render `Tabs` as the first child of the shell's body. The primitive is
not changed.** §7.1 requires shared components to change additively, and a slot added
for one consumer is the beginning of the prop-per-capability shell that §1.1 rejects.

**This must be checked in the browser, not by reading the diff.** The tab bar's left
edge has to align with the header's, and `ScheduleEditor.tsx` already carries a
comment about this exact failure on this exact page:

> When the form carried its own `max-w-3xl` it ended short of the tabs above it, which
> reads as a rendering fault rather than a choice.

§10 of the conventions document says the same thing generally: reading the code does
not settle layout.

---

## 4. E — the Alerts editor's fields onto `ConfigTable`

**This section was rewritten after reading the code it describes. An earlier
revision asserted three changes and two and a half of them were wrong.** The
errors are recorded rather than quietly dropped, because each came from reading
a rule in `docs/en/ui-conventions.md` and applying it without checking whether
the situation it was written for is the situation here.

### 4.1 What is actually wrong, and it is one thing

*(Resolved by package E — see the plan. This section describes the code as it stood before
that package; the hand-written stacks and two-up grid it names no longer exist.)*

Inside each of `ScheduleEditor.tsx`'s four sections, every field is a
hand-written stack: a `<Label>`, a control, and often a `<p class="text-xs
text-muted-foreground">` helper wedged underneath. §4.1's **positive** rule
applies — a group of settings is a `Setting | Details | Value` table — and the
gain is concrete: the helper text moves out from under the control and into the
Details column, where the convention puts it.

The `General` section's `Name` + `Description` two-up grid converts too. Its
comment says the grid exists because "a full-width row each left the section
mostly white space with the fields stretched past any length they will ever
hold" — and `ConfigTable` solves exactly that, better: the Value column is
capped (`w-52` by default, `valueClassName` to widen), so the control cannot
stretch, and the freed width goes to Details rather than to nothing.

### 4.2 Three things that stay, and why

**The `ConfigSection` frame stays.** §4.1's defect is *stacking* — "Cache three
table rows, Limits two, Rate limit three. Read down the page, that was a column
of mostly-empty boxes." `ScheduleEditor` renders `{section === "general" && …}`,
one card at a time, chosen by the tab bar. There is no column, so there are no
boxes in it.

**`ScheduleDetail.tsx`'s `mx-auto max-w-4xl` stays.** It carries its own reason
at `ScheduleDetail.tsx:114-116`: *"`4xl`, not `6xl`: with the section nav now
horizontal there is no column beside the form to fill, and a 1150px-wide
single-column form is past a readable measure. Everything on the page shares
this one edge."* §5.1's rule exists because the six Data API sections **want
different widths** — Config and Query want the page, Access and Try-it do not.
All four sections here are the same single-column form, and the header, tab bar
and form share one edge. Removing the cap widens all three past a readable
measure, which is the thing the comment already refused.

**`DestinationList` stays as it is.** §4.2's "a read-only listing is not a
setting" does not describe it: each row is a *form* whose fields differ by kind
— email takes addresses, Slack takes a channel, webhook takes a URL
and a secret. It is a composed set, not a listing of records, and pressing
per-kind forms into table rows would be worse than what is there.

### 4.3 Scope

`ScheduleEditor.tsx` only. Two things in that file are already correct, carry
comments explaining themselves, and must not be touched: dirtiness computed by
**comparing a snapshot** rather than by listening for change events
(`ScheduleEditor.tsx:367-391`), and the form being **one column with no width of
its own** so the page sets the measure.

## 5. Z2 — Data API endpoint alerts

### 5.1 Shape

No new scheduler and no new delivery layer. A Data API alert is a row in
`automation.schedules` with `type = 'dataapi_alert'`. The existing scheduler picks it
up **without modification**, because its claim query is
`WHERE active AND next_run_at <= now()` and does not read `type`.

Everything the Alerts & Reports programme already built is reused as-is: cron and
timezone, `grace_period_s`, plural owners, recipients across email / webhook / Slack,
the run log, folders / trash / bookmarks, the section-nav editor page, and `{{date}}`
subject tokens.

The comparison half is reused too. `validator_type = 'operator'` already compares a
scalar against a threshold. A Data API metric is a scalar. **What is new is the source
of that scalar, not the evaluation of it.**

Exactly two things are new: an evaluator that reads `dataapi.access_log`, and a source
section in the editor.

**One cost this understates, measured after the fact.** `evaluate_and_deliver`
(`runner.rs:185-190`) is not a match on `type` — it is `if s.r#type == "report" { … }`
followed by a fall-through to the alert path, so a third type routes itself into the
alert branch by default. There is a fail-safe (`NotRunnable("alert is missing its
query")` at `:194-200`), so the failure is a loud run-log error rather than a wrong run
— but making the dispatch explicit is work, not a no-op.

The runner branches on `type` — `alert` goes to `EngineRunner`
(SQL through `QueryExecPort` under an owner's identity), `dataapi_alert` goes to the
metric evaluator (metastore pool, no engine identity involved).

### 5.2 Why this cannot be configuration

`automation.schedules.database_id` is
`uuid REFERENCES query.databases(id) ON DELETE SET NULL`. An alert can only run SQL on
an engine. `dataapi.access_log` lives in the control-plane Postgres. **No existing
alert can observe it by any configuration.** That gap is the whole of Z2.

**One cheap-looking route is rejected.** Registering the control-plane Postgres as a
`query.databases` row would make SQL alerts work with no new code — and would expose
`users`, secret references and `access_grants` to anyone holding `alert:manage`.

### 5.3 Why cron and not events

`egret-events` is an in-process broadcast bus whose own documentation states that *a
lagging subscriber drops old events*. Notifying at the moment a serving request fails
therefore means the alert can **silently miss the very failure it exists to report**,
under load — which is precisely when it matters. Making that path reliable needs a
durable outbox and flood control, which is a package of its own rather than a hook.

`dataapi.access_log` is durable. A cron alert reading a window of it cannot lose an
event.

### 5.4 Data model — `0128_dataapi_alerts.sql`

```sql
-- Widen the type vocabulary. This is the only existing CHECK that changes.
ALTER TABLE automation.schedules DROP CONSTRAINT schedule_type_valid;
ALTER TABLE automation.schedules ADD CONSTRAINT schedule_type_valid
  CHECK (type IN ('alert', 'report', 'dataapi_alert'));

ALTER TABLE automation.schedules
  ADD COLUMN dataapi_endpoint_id uuid REFERENCES dataapi.endpoints(id) ON DELETE CASCADE,
  ADD COLUMN dataapi_metric      text,
  ADD COLUMN dataapi_error_code  text,
  ADD COLUMN dataapi_window_s    integer;

CREATE INDEX access_log_ws_ep_ts_idx
  ON dataapi.access_log (workspace_id, endpoint_id, ts DESC);
```

**A new `type` rather than a `subject` column on `type = 'alert'`.**
`schedule_alert_needs_query` and the report-shape CHECK are both written as
`type <> 'x' OR (…)`, so a new type excludes itself from both and **neither is
touched**. (The report one is `schedule_report_has_format`, not the
`schedule_report_needs_target` an earlier revision of this section named — `0115` renamed
it. Verifying against `0110` alone misses that, and did.) A `subject` discriminator would instead require relaxing a constraint that
is currently protecting live rows, which is exactly where a malformed row gets in.

Four constraints, each making a wrong state unrepresentable rather than merely
discouraged:

```sql
-- The metric vocabulary is closed at the database. A misspelled metric is not a
-- slightly-wrong row: it is a row the evaluator's match never selects, failing
-- silently and forever. Same reasoning as 0110's own enum CHECKs.
CHECK (dataapi_metric IS NULL OR dataapi_metric IN
  ('error_rate', 'error_count', 'p50_duration_ms', 'p95_duration_ms',
   'call_count', 'error_code_count'))

-- error_code if and only if the metric is error_code_count. Two one-directional
-- clauses rather than one equality, so NULL semantics stay obvious.
CHECK (dataapi_error_code IS NULL OR dataapi_metric = 'error_code_count')
CHECK (dataapi_metric <> 'error_code_count' OR dataapi_error_code IS NOT NULL)

-- The shape of a dataapi_alert. The load-bearing half is `sql IS NULL`: it makes
-- "a row both evaluators claim as theirs" impossible to write.
CHECK (type <> 'dataapi_alert' OR (
  dataapi_metric IS NOT NULL
  AND dataapi_window_s IS NOT NULL AND dataapi_window_s > 0
  AND validator_type = 'operator'
  AND database_id IS NULL AND sql IS NULL
  AND chart_id IS NULL AND dashboard_id IS NULL))

-- And the converse: an engine alert may not carry the Data API columns.
CHECK (type = 'dataapi_alert' OR (
  dataapi_metric IS NULL AND dataapi_endpoint_id IS NULL
  AND dataapi_window_s IS NULL AND dataapi_error_code IS NULL))
```

`dataapi_endpoint_id` is nullable and means "every endpoint in the workspace" when
NULL — one column, two scopes, no join table. Its FK cascades on delete because an
alert about a hard-deleted endpoint is meaningless; endpoints are soft-deleted in
normal operation, which §5.6 handles separately.

### 5.5 The metric definitions are copied, not reinvented

An alert that reports a different number from the one on the statistics panel is a
support ticket. `activity.rs` already fixes all three denominators, and the evaluator
takes them verbatim:

| Metric | Definition | Why |
|---|---|---|
| `error_rate`, `error_count` | `status_code >= 400`, denominator is every row | |
| `p50_duration_ms`, `p95_duration_ms` | `percentile_cont(…) FILTER (WHERE status_code BETWEEN 200 AND 299)` | a burst of 401s returning in 2 ms drags p50 down and makes latency look better at exactly the moment the system is failing |
| all of them | `actor_user_id IS NULL` | excludes trials. Without it, a developer clicking **Try it** twenty times against a broken draft fires a production alert |

The trial exclusion is the one most likely to be dropped by someone simplifying the
query later, and it is the one with the loudest failure mode.

### 5.6 The empty window, and why the handling is asymmetric

**This asymmetry is deliberate. It looks like an inconsistency and will be "cleaned
up" by a future refactor unless the reason travels with it.**

- `error_rate`, `p50_duration_ms`, `p95_duration_ms` → `NULL` → run state `noop`
  (already in the `schedule_last_state_valid` vocabulary). With no traffic there is no
  rate. Returning `0` would let a threshold of `< 0.99` fire every five minutes on an
  endpoint nobody called.
- `call_count`, `error_count`, `error_code_count` → **`0`**, not NULL. This is the
  entire point of `call_count`: "the endpoint went silent" *is* the absence of rows,
  and mapping it to `noop` would make the metric undetectable by construction.

**Not implemented (§8).** An endpoint that is trashed or `disabled` was meant to yield
`noop` with the reason recorded in the run log — not `error` — since moving an endpoint
to the trash is deliberate and an error every five minutes because of it is noise. That
part was deferred: `evaluate_and_deliver_dataapi_alert`'s own doc comment
(`runner.rs`) explains why, and there is no dedicated "endpoint was trashed" reason in
the run log today.

What actually happens instead, as a side effect of how trashing works rather than by
design: `endpoint_id` on `dataapi.access_log` carries no foreign key at all — see
`migrations/0123_dataapi_access_log.sql`'s comment, which explains at length why an
access log must survive the deletion of what it describes. So a trashed or disabled
endpoint doesn't get its access-log rows unlinked; it simply stops matching
`live_snapshot`'s `WHERE e.state = 'live'` (`crates/egret-dataapi/src/serving/router.rs`),
the router 404s before `entry.endpoint_id` is ever assigned, and no new row for that
endpoint is ever written — there is no reference left to clear. The practical effect is
the same: an endpoint-scoped watch's traffic simply stops accumulating from that point
on. `call_count`/`error_count`/`error_code_count` then read a real `0` — not a
fabricated `noop` — and `error_rate`/`p50_duration_ms`/`p95_duration_ms` read `noop`
from the resulting empty population, by the same asymmetry this section already
describes for an ordinary empty window, reached here by a different route. A
workspace-wide watch (`dataapi_endpoint_id IS NULL`) is unaffected either way — it
counts every endpoint's traffic, trashed or not.

**A stated limit.** This alert reads a log egret writes. If egret is not running there
are no rows, the window is empty, and the alert stays quiet. It detects a broken
endpoint; it does not detect a dead system. That needs an external probe and is out of
scope.

### 5.7 Authorization

`alert:manage` governs create and edit, as today. Added: **the selected endpoint must
be readable by the creator.** An endpoint's error rate and traffic are information
about it; without this check, `alert:manage` becomes a side channel for observing an
endpoint one cannot see.

Checked at save **and** re-checked at run under an owner's identity — an owner can lose
access after the schedule is written. Losing it records `error` with the reason and
sends nothing. The precedent already exists: a schedule whose owners are all
deactivated stops running.

**A second cost this understates.** `egret-automation`'s *route* state holds a PDP
(`routes.rs:61`), so the save-time half needs no new plumbing — but `runner.rs` has
none. The run-time re-check requires threading a PDP into the runner. A workspace-scoped alert (`dataapi_endpoint_id IS NULL`)
requires workspace-level Data API read.

### 5.8 UI

The same Alerts list in Explore, with a badge distinguishing the type. The editor keeps
its schedule, recipients and message sections; only the source section differs — an
endpoint picker (or "whole workspace"), metric, window, operator and threshold, built
as `ConfigTable` rows on the ground §4 clears.

`grace_period_s` defaults to **14,400 seconds (four hours)**, inherited from Superset
via `0110`. That is reasonable for a business alert and long for an operational one;
this type gets its own shorter default rather than silently inheriting.

---

## 6. How each claim is proven

Assertions are mutation-checked before being believed, and a mutation that does not
compile proves nothing — build to zero errors, confirm that, then read the result.

**Z2:**

- Remove the `actor_user_id IS NULL` filter → a test must go red.
- Change `BETWEEN 200 AND 299` in the p95 expression → a test must go red.
- Empty window: `call_count` returns `0` and `error_rate` returns `noop` — two separate
  tests, because one test passing tells you nothing about the other branch.
- `INSERT` a `dataapi_alert` row carrying `sql` → refused by **the database**, not by
  Rust. A Rust-side assertion would pass with the CHECK deleted.

**U and E:** the full frontend suite runs before every commit that touches a shared
component — a shared file changed under a narrowly-scoped test run is how a regression
reached `main` in an earlier session. `ListPageShell`'s existing guard asserts the `h1`
**count** is exactly one, not merely that one exists; each converted page inherits that
obligation.

**All three:** run on kind and check in the browser before calling it done. On this
codebase every UI defect that mattered was found live rather than by review or by the
test suite, and layout in particular cannot be settled by reading the code (§10).

## 7. The sequel, named so the order is on the record

`U → E → Z2 → Z1 → M → C → D → Z3`.

- **Z1 — IP allow/deny lists and public (unauthenticated) endpoints.** One package, not
  two: an IP allow-list is the compensating control for a public endpoint, and shipping
  them apart means shipping a bare public endpoint first. A public endpoint breaks three
  assumptions of the serving path — the rate-limit key stops being a client key and
  becomes an IP, `access_log` has no actor, and the Cedar invoke grant is bypassed
  entirely — so it needs a workspace feature gate, not a checkbox. Reuses the
  trusted-proxy/XFF handling already established rather than reading headers again.
- **M — multiple SQL statements per endpoint, read-only.** DBAPI's primary use for this
  is pagination: one statement for rows, one for the total. `EndpointSnapshot.sql:
  String` becomes a named statement list, which changes the frozen output columns, the
  response envelope, the cache key and the generated OpenAPI. Placed before C and D so
  the snapshot format changes **once**.
- **C — parameter validation rules and response transformation.** DBAPI uses scripts for
  validation because it has no typed parameter model; egret has one, and both examples in
  DBAPI's guide (`name.length <= 10`, an email regex) are expressible as declarative
  `ParamSpec` constraints. Whether the validation half needs a script runtime at all is
  an open question for that spec. The transformation half is the paused Starlark engine,
  transform-only.
- **D — write statements and transactions.** Two things must be settled before any code
  is written. First, **measure** whether a transaction spanning multiple statements
  exists on Trino and on StarRocks under egret's connection model; if it does not, "multi
  statement with rollback" is not a promise that can be made. DBAPI's own guide says the
  equivalent about HIVE. Second, answer "who is the writer": on a delegated connection a
  service account writes as itself and Ranger applies, which works; on a non-delegated
  connection every client writes as one shared identity, which should be refused at
  publish rather than warned about.
- **Z3 — endpoint configuration import/export between environments.** Last, because the
  export format *is* the snapshot and both C and D add fields to it.

## 8. Deferred, with the reason

- **HTTP proxy executor and Elasticsearch executor.** Dropped after discussion: egret
  serves the databases egret supports. Both would also require a datasource model
  outside `query.databases` (which is engine-backed and Ranger-governed), somewhere to
  hold their credentials, and an egress policy — an endpoint that proxies an arbitrary
  URL is an internal-network scanner published behind egret's authentication.
- **MCP server.** DBAPI exposes published APIs as MCP tools over Streamable HTTP on a
  separate port. It would reuse egret's OAuth2, Cedar and access log unchanged. Not now.
- **API orchestration canvas.** Composing endpoints with decision nodes is a workflow
  engine; it belongs with data pipelines rather than with the serving layer.
- **`${}` interpolation and a Java-class plugin market.** Both are DBAPI mechanisms whose
  egret equivalents are either already present in safer form (§A.2) or already scoped as
  the Starlark engine.
- **Migrating the remaining Explore list pages beyond the four named here**, and
  retiring `ConfigSection` in Control Plane. Unrelated to this work.
- **Reading `dataapi.endpoints` trashed/`disabled` state from the runner**, so §5.6's
  intended `noop`-with-reason handling of a trashed or disabled endpoint can actually be
  built. `evaluate_and_deliver_dataapi_alert`'s own doc comment (`runner.rs`) explains
  why: that state lives on a table owned by `egret-dataapi`, and `egret-automation`
  deliberately does not depend on it beyond the `egret-dataapi-api` leaf (the same
  boundary Task 4's `owner_can_read_dataapi_endpoint` re-check already respects). Its
  natural shape is sketched there: an `EndpointState { trashed: bool, enabled: bool }`
  DTO plus a small port trait, wired the way `pdp`/`query`/`renderer` already are. In the
  meantime the fallback is not silence — see §5.6's note on what happens today instead.

---

## Appendix A — the DBAPI comparison

Read from the DBAPI Enterprise Edition guide (72 pages) against
`crates/egret-dataapi` and `crates/egret-dataapi-api` (31,339 lines of Rust including tests,
migrations `0119`–`0126`).

### A.1 Feature comparison

| Area | DBAPI EE | egret today |
|---|---|---|
| SQL executor | any JDBC database, drivers loaded from `extlib` | Trino / StarRocks through egret's engines |
| Elasticsearch executor | DSL query to API | not built (§8) |
| HTTP proxy executor | forwards to an external service, pass-through or mapped | not built (§8) |
| Write statements | SELECT / INSERT / UPDATE / DELETE, stored procedures | read-only — package D |
| Multiple SQL, transactions | several statements per API, transaction toggle | one statement — package M, then D |
| Dynamic SQL | MyBatis `<if>`, `#{}` and `${}` | **present and safer** — see §A.2 |
| Parameters | form-urlencoded and arbitrarily nested JSON | flat, typed, `query` or `body`, `allowed` enum, default, required |
| Parameter validation | script rules with custom messages | type and enum only — package C |
| Grouping | API groups carrying datasource permissions | folders, bookmarks, trash |
| Lifecycle | save → offline → publish | draft → validate → preview → publish (frozen snapshot) → rollback → disable → trash / restore |
| Client credentials | clientId/secret to `/token/generate`, expiring token, optional RSA response encryption | **stronger** — OAuth2 client credentials on the machine IdP, per-endpoint invoke grants, Cedar |
| Public APIs | supported | not supported — package Z1 |
| Rate limiting | per API and per client QPS | `per_endpoint_qps`, `per_key_qps`, burst |
| Caching | user-implemented plugin | built in, `shared` / `principal` scope; not pluggable |
| IP firewall | allow and deny lists | not built — package Z1 |
| Monitoring | dashboard and call log; file, database or Kafka sink; ClickHouse store | Activity summary and call list over Postgres `access_log`, monthly partitions |
| Failure alerting | plugin: email, SMS, DingTalk, Feishu, WeCom | **package Z2** |
| Data transformation | per-SQL plugin, global plugin, inline Groovy script | `output.shape` and `output.naming` only — package C |
| Plugin ecosystem | five plugin types plus a marketplace | Starlark engine, paused — package C |
| Documentation export | export API docs | per-workspace OpenAPI download |
| Config import/export | test → production migration | not built — package Z3 |
| MCP | separate server, per-API toggle | not built (§8) |
| Deployment | standalone and cluster | Kubernetes |

### A.2 The one comparison that reversed on inspection

An earlier reading of this comparison recorded egret as lacking dynamic SQL. That was
wrong, and the correction matters because it removed a whole package from the plan.

`crates/egret-dataapi/src/render.rs` (1,845 lines) runs minijinja with
`set_formatter` replacing value emission wholesale: `{{ x }}` writes a `?` to the
output and pushes the value onto a bind list. **There is no code path from `{{ … }}`
to raw SQL text**, so an author cannot write an interpolating template even
deliberately. Available today:

```sql
WHERE 1=1 {% if region %} AND region = {{ region }} {% endif %}
          {% if regions %} AND region IN (
            {% for r in regions %}{{ r }}{% if not loop.last %},{% endif %}{% endfor %}) {% endif %}
ORDER BY {{ sort | ident }}
```

`{% for %}` expands to N binds. `| ident` is the one deliberate hole — identifiers
cannot be bound — and it emits **the matching entry from the author's own `allowed`
list**, never the caller's string.

DBAPI's `#{}` corresponds to `{{ }}`. DBAPI's `${}` is raw interpolation, and egret
deliberately has no equivalent: it is the same class of hole as the StarRocks
backslash vector patched in `#504`, and the publish-time gate against template escapes
is a **behavioural** check (render twice, opaque values versus plain ones) precisely
because five independent reviews found five different ways past a byte scanner.
