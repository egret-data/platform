# Data API P5 — the management UI

Completes `2026-08-14-data-api-design.md` by building the surface an author uses: the
`Data API` area, its three sections, and the endpoint detail page. Supersedes
`2026-08-16-data-api-p5-ui-decisions.md`, whose sections 1 and 2 are folded in here
verbatim in intent; that file can be deleted once this one is merged.

Where this document and the P4/P4.5 specs disagree, this one governs for the management
surface only. **It changes nothing about serving.** P4.5's rendering rules, P4's cache and
rate-limit behaviour, and the §5.3 lifecycle invariant are inputs here, not subjects.

## 1. What P5 is, and the one thing it is not

Every part of the Data API exists except the way a person operates it. P1–P4.5 shipped the
dialect escaping, service accounts, endpoint definitions, the serving path, and parameter
rendering. `frontend/src/api/generated/data-api/` already holds a typed client for the whole
management API, generated from the OpenAPI the server emits, and **not one component
imports it.** The `Data API` item in `Sidebar.tsx` has no `to:` and navigates nowhere.

**Audience: internal only.** Try-it and the generated OpenAPI serve the endpoint's *author*.
Consumers get a URL and a downloadable OpenAPI file — nothing else, and no portal. The
`ui-ux-pro-max` skill classifies a developer portal as a different product type ("Trust &
Authority + Minimalism", dark code theme) from egret's "Data-Dense Dashboard"; building both
would put two design languages in one product. This is the one thing P5 is not.

**Deliberately out of scope:** endpoints get no bookmark, trash or share. They are
operational contracts, not saved content — the Explore idioms for personal collections do
not apply to a URL that deployed clients depend on.

## 2. Navigation and page structure

### 2.1 Placement

The `Data Engineering` sidebar group, not Explore. `frontend/src/components/layout/Sidebar.tsx`
already carries the inert placeholder:

```tsx
{ heading: "Data Engineering", items: [
    { label: "Data API", icon: Plug },      // no `to:` — an inert placeholder
    { label: "Pipelines", icon: Waypoints },
]},
```

P5 gives that item `to: "data-api"` and `feature: "data_api"` (the feature key exists at
`crates/egret-workspace/src/features.rs:39`). Explore is where you *consume* and analyse;
Data API is where you *produce* an interface, next to Pipelines.

### 2.2 Route and section nav

Route `/explore/:ws/data-api`, with a **vertical section-nav on the left** — the Workspace
Settings idiom, not a tab strip, because the three areas are of different natures and more
will come:

```text
Data API
  Endpoints     list, canonical Explore scaffold
  Clients       service accounts + OAuth2 clients
  Activity      statistics from dataapi.access_log
```

Keeping `Clients` *inside* Data API rather than in Workspace Settings → Connections is
deliberate: you ship an API from one place.

### 2.3 Endpoints list

Follows the canonical Explore list-page scaffold: `p-4`, `h1 text-lg`, a `"+ Endpoint"`
button, a hand-rolled `<table>` like Charts/Datasets — **not** `FilterableTable`, which
cannot model folder grouping — `SummaryStrip` counts that double as filter chips, and
`RowActionsMenu` per row.

Columns: name · method + path · state (draft/live/disabled) · version · engine · last edited.

### 2.4 Endpoint detail

**Its own route** (`/data-api/:id`), never a dialog — the lesson from Alerts #485 — with its
own section nav:

```text
Definition · Response · Access · Try it · Versions
```

Being a route obliges it to arm `useUnsavedChangesWarning`. Per the Alerts finding: dirtiness
must be a **comparison against the loaded value**, never an `onChange` flag, or Radix
hydration raises a phantom "Leave site?" on a page nobody edited.

There is **no `Activity` section on the detail page**. Per-endpoint statistics live in the
workspace-level Activity tab, filtered to that endpoint (§4). One query, one place.

### 2.5 Clients

A flat list, so `FilterableTable` + `SummaryStrip` is right here (as `DatabasesPage` does).
Two levels: service account → its clients.

**The client secret appears exactly once**, in a dialog after creation, with a copy button
and a sentence saying it cannot be shown again. It is never re-fetchable, never in a list
cell, and — per the P4 audit rule — never in an audit detail.

### 2.6 No `location` control anywhere

`ParamLocation::Body` is refused at publish and `serve_post` has no body extractor. Rather
than offer a control and then reject one of its values, **the parameter editor has no
`location` control at all**: every parameter is `query`. Nothing to offer means nothing to
block, and the refusal cannot drift out of sync with the picker.

## 3. Try-it authenticates by server-side proxy

The serving path takes only service-account bearer tokens — it does not accept a session
cookie — so the browser cannot call it directly.

**New route:** `POST /api/ws/{ws}/data-api/endpoints/{id}/try`, gated on
`data_api_endpoint:manage` (**not** `:read` — this acts on someone else's behalf). Body:
`{ service_account_id, params }`.

### 3.1 It must invoke the real serving router in-process

Through its tower `Service` — the same router, the same layers, the same handler a real
consumer traverses — after minting a real short-lived (60 s) token. **Not** a re-implemented
"equivalent" path, and **not** an HTTP loopback, which would need the server's own URL and
TLS. The P4 tests already drive the router this way, so the mechanism is proven.

*Why this is non-negotiable:* P4.5 repeatedly showed a simulated path staying green while the
real one failed. A Try-it that says OK while consumers get 403 is worse than no Try-it — it
converts the one tool that could have found the problem into the reason nobody looked.

### 3.2 It must not bypass any gate

No grant → the console shows a real `403 not_granted`, which is the most common first
failure; that screen names the fix and links to the `Access` section. Rate limit, cache,
Ranger and the access log all apply for real.

Three consequences, documented rather than discovered:

- A trial **consumes the service account's rate-limit budget.**
- A trial **writes to `access_log`**, which would pollute Activity statistics. §6.1's
  `actor_user_id` column is the fix, and it doubles as the audit trail.
- On a `cache.scope: shared` endpoint a trial **can populate the cache real consumers read.**
  Same data, so not a leak — but it belongs in the docs.

### 3.3 Audit and the token

This is an impersonation action; write an `audit.audit_log` row (who, endpoint, service
account, result). Precedent: user impersonation, #332.

**The token never reaches the browser.** The console shows the full request with
`Authorization: Bearer ••••`, plus a copyable `curl` block using a `$TOKEN` placeholder.

## 4. Activity

### 4.1 Two routes, fixed ranges

```text
GET /api/ws/{ws}/data-api/activity/summary?range=      → tiles + per-endpoint table
GET /api/ws/{ws}/data-api/activity/calls?…            → call list, cursor-paginated
```

`range` is one of `1h` · `24h` · `7d` · `30d`. **No free-form date picker.** The presets are
what keep every query inside the reach of the existing `access_log_ws_ts_idx`
(`workspace_id, ts DESC`); a field that accepts "one year" on the fastest-growing table in
the system has nothing stopping the person who types it.

Gate: `data_api_endpoint:read` at **workspace scope**, for the whole tab. This tab is
deliberately workspace-level and is not sliced per endpoint; someone who may not see all of
it does not enter it. Stated positively so nobody later "fixes" it into a per-row filter that
would make the totals disagree with their own rows.

### 4.2 Tiles and table come from ONE query

`GROUP BY GROUPING SETS ((), (endpoint_id))`. Not for speed — because two queries run moments
apart against a table being written continuously produce a total that does not equal the sum
of the rows beneath it, and the reader will conclude they misread the screen.

### 4.3 The four metrics, and the three that lie under a naive denominator

| Metric | Numerator | Denominator |
|---|---|---|
| Calls | all rows | — |
| Error rate | `status_code >= 400` | all rows |
| p50 / p95 | `percentile_cont` over `duration_ms` | **2xx rows only** |
| Cache-hit rate | `cache_hit` | **2xx rows on cache-enabled endpoints only** |

**Latency is computed over successful calls only.** A burst of 401s returning in 2 ms drags
p50 down and makes latency look *better* at exactly the moment the system is failing. The
label must say "across successful calls" — a percentile whose population is unstated is a
number that cannot be acted on.

**Cache-hit rate's denominator excludes endpoints with caching off.** Including them makes
the ratio permanently and meaninglessly low. When no endpoint in range has caching enabled,
render `—`, not `0%`: zero of zero is not zero.

### 4.4 The table's total is smaller than the tile, and that is correct

`access_log` records a row even when routing never resolved an endpoint — migration 0123 says
so outright: "Null until routing resolves an endpoint, which is why a 404 still produces a
row." Those rows have `endpoint_id IS NULL` and belong to no table row.

The table therefore ends with a **"No matching endpoint"** row carrying them. That population
is usually a client calling a wrong URL or one calling an endpoint that was just disabled —
a signal, not noise, and the only place in the product where it is visible.

**But the log cannot say which URL was called.** `dataapi.access_log` records no method and no
path (verified against migration 0123 — the columns simply are not there), so as the table
stands today this tier is a number with nothing actionable behind it. §6.1 adds
`request_method` and `request_path` for exactly this reason. Without them, "a signal, not
noise" would be a claim the schema does not support.

### 4.5 Trial calls are excluded from statistics and shown in the list

Statistics (tiles and table) filter `actor_user_id IS NULL`.

The **call list shows them**, labelled `Try it — <person>`, behind an *"Include trial runs"*
toggle that is **off by default**. These rows are Try-it's audit trail and are exactly what an
author wants when debugging their own endpoint; hiding them everywhere would discard the
record §3.3 exists to create.

### 4.6 Call list

Columns: `ts` · endpoint · status + `error_code` · `duration_ms` · `cache_hit` · `client_id` ·
`request_id`. Filters: endpoint, status, `error_code`, and a direct `request_id` lookup —
support's entry point when a caller quotes it out of an error body.

Rows with `endpoint_id IS NULL` (§4.4) render `request_method` + `request_path` from the log
(§6.1) instead of an endpoint name, marked as unmatched. Those two columns are the whole
diagnostic value of such a row; without them the tier is a count nobody can act on.

**The `request_id` lookup must still carry `AND workspace_id = $ws`.** `access_log_request_id_idx`
is deliberately global so support can find a row without knowing the workspace; a handler that
trusts the index and omits the predicate lets a guessable `request_id` read across a tenancy
boundary. The index's shape is not the query's authorization.

### 4.7 Retention bounds the ranges

`access_log` is pruned on a configured `retention_days`, which may be shorter than 30 days,
may be `0` (keep nothing — `AccessLogStore::prune` treats it that way deliberately), and may
not be scheduled at all (`scheduled_retention` returns `None`).

The UI reads that value and **disables presets longer than retention**, with a line saying
"kept for N days". Without it, someone selects `30d`, sees data thin out toward the old end,
and concludes traffic fell — a wrong conclusion the screen actively supports.

### 4.8 No time-series chart

Deliberate. The three tiers already answer "is it healthy → which endpoint → which call", and
a bucketed `date_trunc` query over the fastest-growing table buys a trend line at the cost of
pulling echarts into a page that otherwise needs no charting stack.

## 5. Generated OpenAPI

### 5.1 One route, one builder

```text
GET /api/ws/{ws}/data-api/openapi?service_account_id=<uuid>[&endpoint_id=<uuid>]
```

Gate `data_api_endpoint:read`. Per-endpoint is the same route with `endpoint_id` — one
builder, not two mechanisms that can drift.

Output is **OpenAPI 3.0.3**, not 3.1: Postman and `openapi-generator` support 3.0 far more
widely, and this artifact's whole purpose is being consumed by someone else's tooling.

### 5.2 Which endpoints go in — the part that fails silently

Two conditions: `state = 'live'` (a draft has no published contract; a disabled endpoint
returns 404, so documenting it would describe a lie), **and** the chosen service account can
actually invoke it.

**The second condition must be asked of Cedar, not of `access_grants`.** After migration 0124
a `ws_admin` membership carries `data_api_endpoint:invoke`, and the Cedar workspace→object
cascade applies it to *every* endpoint in the workspace. A raw scan of the grant table misses
exactly the endpoints reachable through that cascade and emits a document with paths missing
— with no error, and no way for the reader to tell. Ask the authorizer the same question
serving asks.

### 5.3 `servers[0].url` is derived from the request

egret has no "public API base URL" setting. `catalog.public_url` is an `Option` defaulting to
`http://localhost:8080` and semantically belongs to the catalog; `automation.browser_url`
belongs to notification links. Borrowing either is borrowing the wrong knob, and both can be
unset — producing a file pointing at localhost that looks perfectly valid.

**Derive it from the request that downloads the file**: scheme + `X-Forwarded-Host` / `Host`,
joined with the serving mount `/data-api/v1` (`crates/egret-server/src/lib.rs:369`). The
person downloading is sitting on the origin the consumer will use.

**And display that URL next to the download button.** A derived value needs a place to be
seen wrong; a human eye on the screen is the check, not a config knob nobody revisits.

### 5.4 Contents

- `securitySchemes`: `oauth2` / `clientCredentials` pointing at the machine-IdP token
  endpoint — not a bare `http bearer`. The consumer's real task is obtaining a token, and
  only the OAuth2 form describes it.
- Parameters from `snapshot.params`, skipping `location: body` defensively for snapshots
  frozen before that was refused.
- `enum` → `schema.enum` from `allowed`.
- 200 response built from `snapshot.columns` and `snapshot.output`.
- Error responses for the error codes of `2026-08-14-data-api-design.md` §7 (not this
  document's §7), sharing one `ServingErrorBody` schema.
- Each path carries `x-egret-version: N`, so a consumer knows which frozen contract they hold.

### 5.5 Three limits, written down rather than discovered

**`ColumnSpec.ty` is a raw engine type string**, and Trino and StarRocks name types
differently. Map conservatively to JSON Schema, default to `string`, and **always** preserve
the original in `x-egret-engine-type`. A confident wrong mapping is worse than `string`.

**`ParamSpec` has no multiplicity field, but a repeated query parameter is a list**
(P4.5: `?regions=EU&regions=US` → `IN (?, ?)`). The generator cannot know which parameters are
lists. Declare them scalar and say so in `description`; do **not** emit
`oneOf [scalar, array]`, which breaks most generators. The same `description` carries P4.5's
measured trap: **`+` decodes to a space**, so anything whose alphabet contains `+` must be
sent as `%2B` — a failure that returns 200 with the wrong rows and says nothing.

**Column renaming must call the same function serving calls**, never a reimplementation. And
when `output.naming` maps two columns onto one key, serving returns `output_naming_collision`
(503) — the generator must then **refuse to produce a document and say why**, rather than
emit a schema quietly missing a column.

### 5.6 UI

A button on the Endpoints list opens a dialog to choose the service account, then downloads.
On endpoint detail, the `Response` section has the same button with that endpoint pinned.

## 6. Access, and the §5.3 warning

### 6.1 Migration: three columns on `dataapi.access_log`

```sql
ALTER TABLE dataapi.access_log ADD COLUMN actor_user_id  uuid;
ALTER TABLE dataapi.access_log ADD COLUMN request_method text;
ALTER TABLE dataapi.access_log ADD COLUMN request_path   text;
```

**`actor_user_id`** — nullable; non-null means "run from Try-it by this person". No foreign key
— consistent with `endpoint_id` and `service_account_id` in 0123, and for the same stated
reason: the log must survive the deletion of what it describes. It both filters trials out of
statistics (§4.5) and serves as Try-it's audit trail.

**`request_method` and `request_path`** — what the caller actually asked for. Nullable, because
every row written before this migration has no answer and inventing one would be worse than a
NULL. These exist because of §4.4: a 404 row carries `endpoint_id IS NULL`, and without the
requested URL there is nothing to look at — the serving path holds both values at the moment it
writes the log, so this is recording something already in hand, not reconstructing it.

Do not add an index on `request_path`. Nothing in §4 looks a row up by it: the call list already
narrows by `(workspace_id, ts)` and the path is read out of rows that query returns, so an index
here would only slow down the busiest insert in the system.

### 6.2 The `Access` section is a real status checklist

One row per service account, two conditions.

**Condition 1 — invoke, read truthfully through Cedar** (not a raw `access_grants` scan, per
§5.2). This forces a distinction into the UI that a naive design would collapse:

- *Granted on this endpoint* → revocable here.
- *Inherited from the workspace* → say plainly that revoking here does nothing, and link to
  membership, which is where it can actually be changed.

Collapsing both into one ✓ next to a "Revoke" button produces this sequence: the user clicks
revoke, the backend deletes a grant that does not exist, the ✓ remains, and the product looks
broken while behaving correctly.

**Condition 2 — Ranger permission: egret does not know, and says so.** No inference from the
endpoint's SQL: answering correctly would mean parsing the SQL for its tables and projecting
Ranger policy onto them, and a half-right answer here is worse than none.

Next to it, a **"Run Try it"** button. This is where Try-it earns its place: it is the only
thing that can answer the question, because it traverses the real serving router (§3.1), so a
green result here is evidence rather than a simulation.

### 6.3 The §5.3 warning appears in three places, each with a different job

Publishing does **not** grant data access: queries run as the service account under Ranger
enforcement, so an admin must also grant that service account in Ranger.

1. **The publish dialog** — states it *before* the endpoint goes live.
2. **The `Access` section** — the *current* status, per §6.2.
3. **Engine permission errors are mapped** to a message naming the missing step, instead of
   leaking the raw engine error. Spec §5.3 already requires this and it does not exist yet.

### 6.4 Feature-flag warning goes at the toggle

Disabling the `data_api` feature **does not stop serving** (decided in P2): live endpoints
keep running and only the management surface hides. An operator reaching for that toggle to
stop an API will otherwise believe they have.

The only place this can be said is **at the toggle**, in Workspace Settings → Features. Saying
it on the Data API page is useless: disabling the feature hides that page, exactly when the
sentence is needed.

## 7. Inherited constraints P5 must close

### 7.1 The member picker must filter or label `provider = 'service'`

`MemberRepo::add` has no `users.provider` filter, so a service account can be invited as a
workspace member today. If the role is `ws_admin`, migration 0124 gives it
`data_api_endpoint:invoke`, which the Cedar cascade applies to **every** endpoint in the
workspace — a full-catalog grant issued through a member picker.

`data_api_invoker` must **never** be offered as a membership role.

### 7.2 `kind_perms` arm for `data_api_endpoints` in `egret-bi`

So endpoints appear in the Access Control console like dashboards and datasets, rather than
existing only in a grant table reachable from one page.

### 7.3 Four management routes emit no audit event

`POST …/service-accounts`, `DELETE …/service-accounts/{id}`, `POST …/endpoints` and
`PATCH …/endpoints/{id}`. The P4 spec deferred these to "P5's management UI, where the
surface stops changing shape" — this is that point.

The most serious is **service-account delete**: it hard-deletes a machine principal along with
its `access_grants` and every client it owns, leaving no record that it existed. It follows the
`delete endpoint` pattern — carry the identifying columns out of the `DELETE`'s own `RETURNING`,
because the event is the only surviving record.

All four go through `egret_audit::mutation_for`, so workspace, acting principal and any
impersonator are stamped by construction. No `client_secret` in any detail, ever.

## 8. Testing

**A DB-gated test that skips is not a test.** Missing `DATABASE_URL` makes it report `ok` while
asserting nothing; only the duration distinguishes the two outcomes. Every backend test below is
DB-gated and must be run with the variable set — against `egret_test`, never a dev database,
which cannot be pointed at a running server without breaking its boot. The last item is a
frontend test and carries no such gate.

- **Try-it goes through the real router.** Assert against a *stubbed* handler that the trial
  fails: if replacing the serving handler does not break the Try-it test, the test is not
  exercising the router and the whole guarantee in §3.1 is decoration.
- **Try-it does not bypass gates.** A service account with no grant produces `403 not_granted`
  through `/try`, not a success.
- **Statistics exclude trials**, and the call list includes them only with the toggle on.
- **`request_id` lookup is workspace-scoped:** a row in workspace A is not returned to a
  reader in workspace B who quotes its `request_id`.
- **Latency and cache-hit denominators:** a fixture of 2xx and 4xx rows where including the
  4xx rows would change p50, asserting the reported value matches the 2xx-only population.
- **OpenAPI includes cascade-reachable endpoints:** a service account whose only invoke path
  is a `ws_admin` membership still gets those paths in its document. This is the §5.2 failure
  and a grant-table implementation passes every other OpenAPI test.
- **OpenAPI refuses on `output_naming_collision`** rather than emitting a short schema.
- **Access section distinguishes granted from inherited** for the same service account.

Assertions must be checked by mutation before being believed — stub the implementation and
watch the test fail. Six assertions in the chart programme survived their implementation being
removed. And a mutation that fails to *compile* is not evidence of anything: build first,
confirm zero errors, then read the result.

## 9. Decomposition

One spec, several PRs, in this order — each independently mergeable:

1. **Backing routes and migration** — §6.1's three columns, activity summary/calls, the OpenAPI
   generator, `/try`, and §7.3's four audit events. Everything in 3–6 reads from this.
2. **`kind_perms` arm** (§7.2) and the member-picker filter (§7.1) — small and independent of
   the rest; sequenced early because §7.1 is a live over-grant, not a cosmetic gap.
3. **Endpoints list + detail** (Definition · Response · Access · Versions).
4. **Try it section** (§3), which needs 1 and 3.
5. **Clients section** (§2.5).
6. **Activity section** (§4).
7. **Feature-toggle warning** (§6.4) — one sentence in someone else's page, kept separate so
   it is not lost inside a large frontend diff.

The seam between 1 and 3–6 is where P4's worst defect lived: a cache key that checked
`delegated` but not `options.impersonate` survived five review rounds because it sat *between*
tasks. Live verification in a browser is required before this programme is called complete —
every chart defect that mattered was found there, not in review and not by a green suite.
