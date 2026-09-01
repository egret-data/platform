# Data API — production readiness (platform tier)

**Status:** design approved; both §4 decisions taken (A → per-database `governed` flag;
B → Redis/Valkey with a startup refusal as the interim guard).
**Target agreed with the user:** *platform tier* — deployable and observable, safe on governed
data by refusal, and trustworthy for **several consuming teams**. Explicitly **out of scope**:
data classification / PII awareness, and re-deciding snapshot pinning (`FOR VERSION AS OF`).
Both are recorded in §6 as deliberate exclusions, not oversights.

## 1. Where the feature actually stands

Four independent read-only audits (`.superpowers/audit/{serving,operations,contract,scale}.md`,
each with `file:line`) converged on one sentence:

> **Correctness engineering is strong and evidenced. Operational engineering has not started.**

That shape matters more than any single finding. The SQL-safety architecture is better than most
production systems — `bind_formatter` has **no interpolating mode**, so safety is a property of
the type rather than of programmer discipline; served responses are validated against the frozen
column set, fail-closed, six refusals per statement; the dual-dialect escaping proof requires two
independent readings to agree. None of that needs rework. What is missing is everything that
happens *around* a correct request: seeing it, bounding it, scaling it, and telling consumers what
it promises.

## 2. The evidence base, and its weakness

Every integration suite in `crates/egret-dataapi/tests/` is **DB-gated and self-skips green**
without `DATABASE_URL`, and all of them drive a **fake `QueryExecPort`**. Nothing engine-side has
ever been executed by a test. In a repo that has shipped a green test over the very defect it
named **twelve** times, that is a standing risk, not a footnote.

The user chose to fix this **in parallel, non-blocking** (Package 0). Consequence to state
plainly: the packages that land before Package 0 will be accepted on the same weak evidence, and
some may need re-verification afterwards. Each package below therefore names the evidence it
rests on.

## 3. Packages

Ordered by value-per-effort, not by severity. Dependencies noted.

| # | Package | Why it is here | Depends on |
|---|---|---|---|
| 0 | **Test foundation** — ✅ `6aa2399a` (#555) | Missing `DATABASE_URL` becomes an error, not a skip; add a path that touches a real engine. | — (parallel stream) |
| 1 | **Governance gate at publish** — ✅ `2d7a3ba6` (#556) | A served query runs as the **service account**, never the caller, so Ranger row filters and column masks are evaluated at machine-identity granularity. Nothing today requires a delegated connection. | §4 decision A |
| 2 | **Metrics + `/metrics`** — ✅ `0caf720b` (#554) | There are no metrics at all. One outage class is invisible to all three existing instruments simultaneously — see §5. | — |
| 3a | **Serving guard rails** — ✅ `5de17862` (#558) | No graceful shutdown, so every deploy kills in-flight queries; no resource limits; `/readyz` exists and is never called (probes are `tcpSocket`); no concurrency cap, body limit, or timeout layer on the composed router. | — |
| 3b | **Engine coupling** — ✅ `3ff8096d` (#557) | No engine connection pool (a TCP+TLS handshake **per statement**); a retryable Trino 503 discarded as a 500; `internal` the single code for six causes. | — |
| 4 | **Multi-replica correctness** — ✅ `0c3febe5` | Response cache and token bucket are **process-local** while replica count is a Helm value, so every published rate limit silently multiplies by replica count and the cache fragments. | §4 decision B |
| 5 | **Script parameter constraints** — ✅ `bed1f969` (#560) | Package S lets a script concatenate a caller's value into SQL text. The publish-time sentinel probe has **two bypasses pinned open by tests asserting `is_ok()`**. Closed by construction rather than by a better probe, at publish AND — for endpoints frozen before the rule — at serve time; see §5.1. | — |
| 6 | **Publish-time diff + acknowledgement** — ✅ `c4b00c8e` (#561) | `publish` validates the draft against *itself*; the only read of `endpoint_versions` is `MAX(version)`. A removed column, narrowed type, renamed parameter or new required parameter publishes silently. Worst case: adding a second statement flips top-level `data` from an array to an object. | — |
| 7 | **Consumer version pinning** — ✅ `4b0d1da1` (#562) | Serving resolves `live_version_id` only; a publish moves every caller at commit. `meta.version` lets a consumer *observe* which version answered but never *ask* for one. | 6 |
| 8 | **Contract reachability** — ✅ `34260329` (#563) | The OpenAPI document sits behind a human session + `:manage`, so a consumer holding only a `data-api` token cannot fetch its own contract; `error.code` has no `enum` despite being documented as the stable code to branch on. | — |

### Package 6 as delivered, and the four decisions inside it

`crates/egret-dataapi/src/contract.rs` compares the snapshot a publish would
freeze against the snapshot reached through `endpoints.live_version_id`, files
every difference as breaking or safe, and `publish` refuses a breaking one
without an acknowledgement. Four things are worth carrying forward rather than
rediscovering:

- **One rule, not a list of rules.** A change is breaking when a request that
  succeeded against the live version could — with the caller changing nothing —
  be **refused**, **silently ignored**, or answered with a **body their existing
  code cannot read**. Every classification is derived from that sentence, and
  the restraint is the deliverable: a rule that fires on every publish teaches
  an author to click through, and an acknowledgement nobody reads is worse than
  none, because it also produces a record saying they agreed. "Silently ignored"
  is in the rule for exactly one case — a REMOVED PARAMETER. Serving walks the
  declared specs, so `?region=EU` against an endpoint that renamed `region` is
  not refused: it answers 200 with the filter no longer applied.

- **The envelope predicate is the router's own.** `data` is keyed by statement
  name iff `!(statements.len() == 1 && transform.is_none())` — read from
  `meta_contract` and `statements_data`, not restated. That means the trap has
  TWO doors, and the second was not in the brief: attaching a response transform
  to a single-statement endpoint flips `data` and `meta` exactly as adding a
  second statement does.

- **The acknowledgement is a digest, not a flag.** `POST /validate` returns the
  diff and a SHA-256 over (a domain tag, the endpoint id, the live version id,
  and each breaking change's kind/subject/detail, length-prefixed). `publish`
  recomputes it under the endpoint's row lock and compares. There is no constant
  a deploy script can hardcode, a token issued for yesterday's break does not
  match today's, and one issued for another endpoint or from before a rollback
  does not match at all. A token sent when nothing breaks is refused rather than
  ignored — accepting it is how a client learns to send it always, which is the
  `force: true` this shape exists to make unrepresentable. The refusal is a
  **409**: the same body is correct or incorrect depending on which version is
  live at that instant, so "your request is malformed" would be false the moment
  a rollback moved the pointer.

- **The record distinguishes silence from an answer, and names what it is about.**
  Migration `0131` adds `dataapi.endpoint_versions.breaking_changes jsonb`
  **NULL**, not `DEFAULT '[]'`. NULL means nobody looked — a version published
  before the column existed, or a first publish, where no consumer has ever
  received anything. `[]` means it was compared and broke nothing. A default
  would have stamped the second claim, retroactively, onto every row that only
  qualified for the first. Beside it, `compared_against` names the version the
  comparison was made against — whatever `live_version_id` pointed at inside the
  publishing transaction. That is usually the preceding version and after a
  rollback is not, so without it `[]` is a sentence with its subject missing:
  true of the baseline's consumers, and silent about the consumers of the
  version a rollback moved them off. With the row's existing
  `created_by`/`created_at` that is the audit trail: who accepted which break,
  against which baseline, when.

Two deliberate exclusions, recorded so they are read as decisions:

- **`rollback` is not gated — it is REPORTED.** Putting yesterday's version back
  can break a consumer who coded against today's, and the diff runs just as well
  in that direction. It does not refuse, for the reason `rollback` already
  refuses to re-run the full validator: rollback is usually someone stopping an
  outage, and a gate there is the one that must not exist. Publish is an author
  choosing to move forward and can afford to be stopped. But not refusing is not
  the same as not saying: `rollback` classifies what it moves consumers off
  through the same `compare_contracts`, returns it as `rollback_breaks`, and the
  route audits it. Before that the one act that could break every consumer of an
  endpoint left no trace anywhere, and the publish that followed it recorded an
  empty diff an auditor would read as "nobody was broken here".
- **The console does not classify.** The editor's state pill
  (`endpointState.tsx`, package 1) still answers its own question — "is there
  anything to publish", one boolean over nine fields — and is untouched. It is
  a different question from "does publishing break a consumer": a `sql` edit
  lights the pill and produces no breaking change, and both are correct. What
  the frontend does NOT do is decide what is breaking; it renders the server's
  list and echoes the server's token back.

### Package 7 as delivered: the consumer's half of package 6

Package 6 tells an author what a publish breaks and refuses it without an
acknowledgement. The acknowledgement is the author saying *"yes, I am breaking
them"* — and until package 7 the consumer on the other end had no way to keep
working while they adapted. `meta.version` let them **observe** which version
answered; nothing let them **ask**. Serving resolved `endpoints.live_version_id`
and nothing else, so a publish moved every caller at the instant it committed.

A consumer now sends `X-Egret-Version: <n>` and is answered from that frozen
version. Six things are worth carrying forward rather than rediscovering.

- **A header, not a query parameter, and the reason is specific to this
  surface.** The query string is the endpoint AUTHOR's namespace and it is
  unrestricted: nothing at publish reserves a name, so `version` is a legal
  `ParamSpec.name` today (`check_params` even *requires* a declared parameter to
  be referenced as `{{ version }}`), and a supplied name that is not declared is
  **silently ignored** by `resolve_supplied_with`. So `?version=` would either
  shadow an author's own parameter — their filter stops applying, `200`, no
  signal, which is the exact "silently ignored" failure package 6 classifies as
  breaking — or need a reserved-name rule applied retroactively to every already
  published snapshot. A header has its own namespace, needs no new route (there
  is exactly one), stays out of `resolved.key`, and OpenAPI keys a parameter on
  the `(name, in)` PAIR, so the generated document can declare it beside an
  author's `version` query parameter with no collision. The name matches the
  `x-egret-version` extension the generated document already stamps on each
  operation: the number is read out of one and sent back in the other.

- **The pin replaces exactly ONE clause of the routing query, and that is the
  whole security argument.** `router::live_snapshot` still decides *whether there
  is a row at all* from `e.workspace_id`, `e.method`, `e.path` and
  `e.state = 'live'`; the pin only changes which `endpoint_versions` row `v`
  binds. So a disabled endpoint, a trashed one (migration 0126's CHECK makes
  `deleted_at IS NOT NULL` and `state = 'live'` mutually exclusive), another
  workspace's, and one in an archived workspace are all unreachable by pinning
  **because nothing about them was asked differently**. Likewise the
  `LEFT JOIN query.databases` keys on the *pinned* snapshot's `database_id` and
  reads the *live* row, so package 1's governance predicate — deliberately built
  as a live predicate rather than a frozen witness, precisely so that marking a
  database `governed` takes effect on already-published versions — applies to a
  pinned old version with nothing added. And `invoke_allowed` runs on
  `endpoint_id` after all of it, against the PDP as it is now. **Pinning pins the
  contract, not the permissions**, and it holds by construction rather than by a
  rule someone has to remember. Each of those has a test that pins an old version
  and watches the refusal still fire.

- **Two predicates guard the pin, and only one of them is testable the obvious
  way.** `v.endpoint_id = e.id` scopes the version number to this endpoint
  (`UNIQUE (endpoint_id, version)` makes numbers per-endpoint), and
  `v.version <= lv.version` refuses a pin ABOVE the live version — which can only
  mean a `rollback` moved consumers off it, and a rollback that a caller can undo
  with a header is not a rollback. **The first version of the scoping test passed
  with `v.endpoint_id = e.id` deleted**, because published version numbers are
  CONTIGUOUS: any number at or below an endpoint's live version is one that
  endpoint has, so the two predicates overlap for every reachable state and the
  `<=` clause was doing the refusing. The fixture that reaches the real state has
  to put a **gap** in the endpoint's history — which is the state a version
  pruner would create — and then the sibling endpoint's row is the only match and
  an unscoped join serves another endpoint's SQL, `database_id` and frozen
  columns under a `200`.

- **The rate limit follows the LIVE version, never the pin, and this is the one
  field that does not.** The buckets are keyed on `endpoint_id` alone
  (`ratelimit::client_key`, `endpoint_key`) — no version — so one endpoint has
  one bucket. If the rate came from the pinned snapshot, a caller would be
  choosing the refill rate of shared mutable state with a header:
  `TokenBucket::take` tops the stored tokens up to `rate.burst` on every call, so
  alternating a pin to a generous old version with an unpinned call refills the
  bucket at the old rate and spends it at the new one, and an author's tightening
  is not a limit at all. That is package 4's "a shared counter driven by a
  caller-supplied value" defect wearing different clothes, and it is a defect
  package 7 would have INTRODUCED rather than one it found. `limits`
  (`max_rows`, `timeout_seconds`) deliberately stays with the pinned version: it
  bounds ONE request, is already clamped by `MAX_ROWS_CEILING` /
  `TIMEOUT_CEILING_SECS`, and the row cap is quoted back to the caller in
  `row_limit_exceeded`, so it is part of what the frozen contract promises.

- **Two refusals, because they name two different actors.**
  `400 version_pin_invalid` is the caller's own header — not a run of ASCII
  digits, over nine of them, below 1, or sent twice on one request (refused, not
  resolved by header order: a request with two answers must have none). It states
  the accepted shape and no endpoint fact. `409 version_unavailable` is the
  endpoint's history: never published, or above live. **One code for both of
  those**, because two would enumerate the shape of an endpoint's history for
  nothing — the consumer's action is identical either way. `409` rather than
  `400` follows package 6's acknowledgement refusal: the same request is correct
  or incorrect depending on which version is live at that instant. Neither
  carries a `Retry-After`; neither clears with time. Both went through
  `ServingError`, so `to_wire`'s exhaustive match forced them, and the generated
  per-service-account document — whose status list is derived from `to_wire` —
  describes them without anyone editing it.

- **Retention is UNBOUNDED and nothing enforces a policy, which is a decision
  rather than an omission.** Nothing prunes `dataapi.endpoint_versions`; a
  version lives exactly as long as its endpoint, and a hard delete of the
  endpoint cascades the versions away — but takes the endpoint with them, so a
  pin against it answers `404 not_found` like any unknown URL, never a silent
  fall-through to live. `version_unavailable` therefore has exactly two causes
  today, both tested. An operator who wants to retire an old version can find who
  is still on it from `dataapi.access_log.version`, which records the version that
  ANSWERED — so `access_log.version < live version` is the query, and a refused
  pin records no version because nothing answered. Two consequences to state
  plainly rather than leave to be discovered:
  **a pin defeats a rollback for the consumer that sends it** in one direction
  (they keep receiving their pinned version, which is the point) and is refused
  in the other (they may not pin the version the rollback withdrew); and **there
  is no `retired_at` on a version**, so an author cannot force a pinned consumer
  off one short of deleting the endpoint. If retention ever needs a bound, the
  refusal a pruner requires already exists and is already tested.

**The programme is complete at the platform tier.** All ten packages are on `main`.

**What CI's return revealed, recorded because it invalidates three labels this document and
its task briefs repeated.** CI was out of quota for the entire programme — `steps: 0`, failing
in two seconds — so every merge used `--squash --admin` after local gates. When it came back it
failed on real assertions, and `cargo test --workspace` **fail-fasts**: run `33460143378`
stopped after 45 of ~114 binaries, so the whole of `egret-controlplane` never ran. The step now
carries `--no-fail-fast` (#564), because a red run that answers one question at a time is how
the next item hides.

Behind it sat `catalog_drift::every_catalog_permission_is_enforced_somewhere`, called
"pre-existing, environmental" dozens of times here and in every task brief **without once being
checked**. It was a **false negative in the scanner**: `data_api_endpoint:invoke` *is* enforced;
the scan could not parse `pub(crate) const`, kept a per-file const table while `EP_INVOKE` is
declared in one file and used in another, and could not span a six-line gate.

Two further latent defects surfaced with it. A test compared a Postgres-round-tripped timestamp
(microsecond) against an in-memory nanosecond value — in one of the two files Package 0 found
were passing with **no database at all**. And `MAX_FRACTION_DIGITS = 100` is not portable:
ECMA-402 specified 0..20 until ES2023, so a saved chart with `decimals: 50` rendered on a new
browser and hit `ChartErrorBoundary` on an older one. CI pins **Node 20** while local machines
run newer, and no `.nvmrc` or `engines` field makes that visible.

**Carried out of packages 0–8, not fixed, so they are not rediscovered as new:**

- **StarRocks cannot be pooled**, and this was measured live rather than argued:
  `EXECUTE AS … WITH NO REVERT` survives `COM_RESET_CONNECTION`, the mechanism
  `mysql_async` uses to recycle a connection. StarRocks endpoints therefore still pay a
  handshake per statement, and the ~400–500 req/s ceiling stands for them.
- **`starrocks.rs`'s `execute`/`execute_with_params` do not call
  `reject_statement_separator`** while `describe` does, so a SQL Lab user can send
  `EXECUTE AS 'x' WITH NO REVERT; SELECT …`. Contained today **precisely because
  StarRocks is not pooled** — pooling it before fixing this turns a same-session
  escalation into a cross-principal leak. Pre-existing; needs its own package.
- **Nothing in package 3 was observed on a live cluster.** The drain has not been watched
  through a real `kubectl rollout restart`, nor the probes flipping a pod out of
  Endpoints.
- The latency histogram carries only a `status` label, so a refusal's `Duration::ZERO`
  shares a series with real latencies. `MAX_ROWS_CEILING` was not lowered — a contract
  change that belongs with streaming.

Two smaller items fold into the packages that own them rather than standing alone:

- **Error classification** (into Package 2 or 3): `internal` is the single code for six unrelated
  causes, so an operator cannot separate "this author's endpoint is broken" from "egret is
  broken". The design doc already argued this and it was never closed. PR #552 fixed only the
  *logging*; the classification is unchanged.
- **Trino 503 handling** (into Package 3): the Trino client protocol defines 503 as *retry, the
  query is alive*; it currently becomes a 500 and abandons a live query.

### Package 8 as delivered: the document, reachable by the credential that needs it

`GET /api/ws/{workspace}/data-api/openapi` was — and remains — behind a human
session and `data_api_endpoint:manage`. `routes.rs`' module comment gives the
reason and it is still true: reading grants "enumerates every consumer of the
endpoint", and this route "answers that question per account and iterable", so a
`:read` caller could walk a workspace's service accounts and reconstruct the
whole grant graph along with every path, parameter and column name in it.

So the answer is a **second route**, not a looser gate:
`GET /data-api/v1/{ws}`, authenticated by the `data-api` bearer alone. Four
things are worth carrying forward.

- **The bearer cannot name an account, because it IS one.** The service account
  comes from the token's `egret_user_id` claim; nothing in the request can
  change it. The `:manage` route's danger is that it takes a
  `service_account_id` and is therefore iterable — this one has no such
  parameter and no roster to enumerate.

- **Scoping is the same expression the call path uses, not a second answer.**
  `openapi_gen::build` already asks `invoke::can_invoke` per candidate endpoint,
  which is what `serving::router`'s step 4 evaluates before it runs anything. A
  document listing endpoints the account may not invoke would be an information
  disclosure — endpoint paths and parameter names are exactly the reconnaissance
  an unauthorised caller wants — and the way that is prevented is by there being
  one answer to "what may this account call". A grant revoked between two
  fetches is gone from the second document, and an endpoint reachable only
  through the Cedar `ws_admin` cascade is in both the document and the call.

- **The URL is `servers[0].url` itself, and it is the one URL under this mount a
  published endpoint can never take.** `normalize_path` refuses an empty path,
  so every published URL has at least one segment after the slug. A reserved
  literal *below* the slug — `/{ws}/openapi.json` — would have had no such
  guarantee: that spelling publishes today, and axum prefers a static segment to
  a wildcard, so the author's endpoint would have been silently shadowed by a
  document.

- **Not cacheable, and the reason is not cost.** The document varies by service
  account, so a cache keyed on the URL would hand one consumer another's
  contract — the trap `serving::cache`'s principal-keyed key exists to avoid,
  and worse here because what leaks is the paths and parameters of endpoints the
  recipient was never granted. Keying on the principal would fix that and still
  be wrong: the document also varies with every grant and every publish, neither
  of which invalidates anything, and a contract must not be staler than the
  calls it describes. `Cache-Control: no-store`.

- **It has a rate-limit bucket of its own, because it is the most expensive request the
  surface serves.** Building the document costs one PDP decision per live endpoint — two DB
  round trips each, up to about 400 for one request — and a holder of one valid bearer can
  loop it. Package 3a's bulkhead does not close that: it bounds concurrency, so a serial loop
  never trips it. The per-endpoint limiter genuinely does not fit (it is keyed on
  `endpoint_id` and this request names none), but that is an argument for a different bucket,
  not for no bucket — otherwise the cheapest request on the surface is limited and the most
  expensive one is not. So: a third `d:` namespace keyed on the workspace and the **service
  account**, both read off the signed token, at a fixed 1 qps with a burst of 10. Its own
  namespace so a contract fetch can never spend a token a CALL then needs; keyed on the
  account rather than `client_id` because the account is the dimension the document varies by
  (Package 4's lesson: a bucket keyed without the varying dimension lets a caller choose its
  own rate). No second workspace-wide bucket — that would let one account's loop deny every
  other consumer its contract, and the workspace-wide bound is the bulkhead. Because the rate
  is a constant rather than an author's published number, this route **always** consults the
  store, so a store outage refuses it (`503 rate_limiter_unavailable`) where a call to an
  endpoint with no configured limit keeps serving; that is the fail-closed policy applied to a
  route with no unlimited mode, and it is the cheaper failure — a consumer needs this document
  when it builds a client, not per call.

The second half is `error.code`. Both published documents described it as "the
stable code to branch on" and emitted a bare `{"type": "string"}`, so a
generated client typed it `string` and a consumer branching on it had nothing to
branch on. Both now carry an `enum` **derived** from
`serving::error::wire_codes()` — `to_wire` applied to the variant census — for
the reason `wire_answers` exists at all: the hand-maintained status lists in
these same two documents went stale twice in this programme, and a third hand
list would have gone the same way. The frontend client now exposes the codes as
a union type rather than `string`.

## 4. Two decisions — both taken

**Decision A: per-database `governed` flag.** **Decision B: Redis/Valkey, with a startup refusal
on >1 replica as the interim guard until it lands.** Rationale for each below, kept because the
alternatives explain the choice.


### Decision A — the shape of the governance gate (blocks Package 1)

Refusing every non-delegated publish is the simplest rule and would **break the endpoints that
work today**. Three candidates:

1. **Per-database `governed` flag.** Refuse publish onto a governed database without a delegated
   connection. Precise, and puts the decision with the person who knows the data.
2. **Workspace-level governed mode.** Coarser, one switch, easier to reason about at audit time.
3. **Author acknowledgement.** Publishing on a shared identity is permitted but requires an
   explicit, recorded acknowledgement, surfaced on the endpoint and in the audit trail.

Recommendation: **1, with 3 as the fallback for ungoverned databases.** A flag on the database is
the only one of the three where the safe answer does not depend on someone remembering — the
pattern this codebase already prefers (see `check_cache_scope`, which refuses at validate time so
the unsafe configuration can never exist).

### Decision B — where shared limiter/cache state lives (blocks Package 4)

There is **no Redis, Valkey or memcached anywhere in the stack today**; adding one is a new
operational dependency. The seam exists: `RateLimitStore` is a trait
(`serving/ratelimit.rs:110`) and `TokenBucket` is merely one implementation.

1. **Redis/Valkey.** The right tool; also fixes cache fragmentation. New dependency to run.
2. **Postgres-backed.** No new infrastructure, but the metastore pool is *already* the first thing
   predicted to break — 13 round-trips per request against a default pool of 10 shared with all of
   egret. This fixes one ceiling by lowering another.
3. **Refuse to scale.** Keep state in-process and make >1 replica a startup refusal until a shared
   store is configured. Honest, cheap, and does not deliver scale-out.

Recommendation: **1 for the limiter and the cache, with 3 as the interim guard** so that a
multi-replica deployment can never silently multiply limits in the window before 1 lands.

**Delivered as recommended.** `serving/shared.rs` is one `SharedStore` behind both seams
(`RateLimitStore`, and a `ResponseStore` trait the cache did not previously have), the
token bucket is a Lua script so the check is atomic in one round trip, and
`choose_store(replicas, url)` refuses to start on >1 replica with no store — wired from
`backend.replicas` through `EGRET_DATA_API__REPLICAS`. Three decisions worth carrying
forward rather than rediscovering:

- **The two failure policies are opposite.** The cache fails OPEN (a store error is a
  miss, which re-executes); the limiter fails CLOSED (`503 rate_limiter_unavailable`, a
  new taxonomy variant), because admitting would reinstate the multiplied-limit defect
  unbounded, at the one moment nobody is watching. The blast radius is bounded by
  `check_request` only touching the store for a limit an author actually configured.
- **The eviction policy is stated, not inherited.** Redis defaults to `noeviction`, under
  which a full instance fails writes — which, with a fail-closed limiter, would turn a
  full cache into 503s. The chart sets `allkeys-lru`. Cost: an evicted bucket comes back
  full, at most one extra burst, the same caveat the in-process map already carries.
- **The cache key is untouched.** The length-prefixed, principal-trailing digest is what
  the store keys on, verbatim, and `shared` scope still requires the live
  `(delegated, impersonate)` re-check. A shared cache makes a cross-principal leak
  fleet-wide rather than per-replica, so that was the constraint, not fragmentation.
- **A shared bucket has N clocks, and that is a token generator unless the stored
  timestamp is monotone.** Found in review, after the first version shipped: because
  each replica supplies `now`, a lagging one rewound the stored `l` and the leading one
  re-billed an interval it had already been charged for — on every alternation, without
  bound. Measured on Valkey 8 at `qps=10, burst=10`, 400 calls at one real instant: 10
  admitted at no skew, 20 at 5 ms, 110 at 50 ms, and **400 of 400 at 500 ms**, i.e. no
  limit at all. The fix is a monotone floor inside the script (`if now < last then now =
  last end`), which makes the bucket run on the envelope of every replica's clock — one
  clock, advancing at real rate — and leaves a one-time offset bounded by `burst`. The
  general lesson is what to carry: *any* shared counter driven by a caller-supplied clock
  has this shape, and a test that hands one clock reading to every replica cannot see it.
- **The two caches share the 4 MiB constant and not the quantity it bounds.** The
  in-process cache bounds an estimate; the shared store bounds the serialised length,
  and the estimate charges every number a flat 16 bytes. So a float-heavy response
  caches on one replica and stops caching after scale-out. The bound is genuinely
  enforced either way and the direction is the safe one (a miss re-executes), but the
  "scale-out changes nothing" sentence the first version wrote was false, and there is
  now a witness test rather than a claim.
- **`replicas: 1` is not "one of these exists".** A Deployment's default RollingUpdate
  surges a second pod before terminating the first, so the store split itself during its
  own upgrade. `strategy: {type: Recreate}` on `deps-valkey.yaml` — and on
  `deps-postgres.yaml` and `deps-minio.yaml`, which had the same shape and a
  ReadWriteOnce PVC to wedge on.

## 5.1 Package 5, as delivered: a rule instead of a better probe

**The instrument was never going to be enough, and that is a property of
instruments.** The sentinel probe evaluates a script with a long improbable value in
each parameter's place and refuses the definition if that value comes back inside the
emitted SQL. It is a **positive** test: it can prove a value *did* reach the text. It
cannot prove one never will, and its two documented bypasses are exactly that gap —
`params["region"].upper()` emits something that is not the sentinel, and a script that
length-checks its input *raises* on the sentinel so the concatenation on the accepting
path is never observed. There is a third shape the probe is equally blind to and
package 5 found it: a concatenation behind a branch chosen by a value no probe
supplies (`if params["mode"] == "raw"`), which does not even show up in `varies_with`.

Scanning the emitted bytes harder is the P4.5 mistake — five independent reviews, five
different bypasses of one byte scanner guarding a template language. **So the probe is
untouched, and the two tests pinning its residue still assert that it accepts both
scripts.** What changed is that a definition now has to pass a second rule that never
looks at the emitted text at all.

**The rule.** On an endpoint where any statement is a `script`, every `string`
parameter must carry a `pattern` whose accepted language is contained in **ASCII
letters, ASCII digits and `_`**. `check_script_parameter_constraints` in `lifecycle.rs`,
run from `validate_endpoint` and `validate_preview` — and, through the snapshot adapter
beside it, from `params::resolve` for endpoints frozen before the rule existed (see
below). This is the move
`check_enum_params` already makes for `Enum` — the only safe form for something that
may reach an identifier position is one the author wrote down — extended to the
`String` case package S opened.

**Why that alphabet.** It is exactly the body of an unquoted identifier on both
engines, chosen for what it excludes: nothing in it can open or close a string literal,
begin a comment, separate a statement, escape the next byte (the StarRocks half of the
dual-dialect proof), qualify a name into another schema, or end a token so a keyword
can follow. And — the property that actually matters, since the whole problem is that a
script may transform the value — **the set is closed under every Starlark string
operation that selects from its input**: case mapping (`.upper()` of `[a-z]+` is
`[A-Z]+`, still inside), slicing, indexing, `strip`, `removeprefix`/`removesuffix`,
`partition`, `split`, `elems`, repetition, and concatenation of two safe values.

**`join` was on that list and does not belong there** (corrected 2026-08-31, review of
this package). The justification for the list is *"every character of the result was a
character of the input"*, and `sep.join(pieces)` breaks it: the separator comes from
**author** text and is inserted once per gap, while the number of gaps is
`len(pieces)` — which the caller controls whenever the pieces came from their own
value. `" ".join(params["q"].split("_"))` is idiomatic humanisation, every operation on
it appears on the safe list, and a caller sending `q=1_OR_true` produces
`WHERE flag = 1 OR true`. It belongs with `replace` and `format` below.

**Where the assumption fails, stated rather than implied.** It is *not* closed under
operations that introduce bytes the value never contained — `replace("a", "'")`,
`format`, `chr(ord(c)+1)`, `sep.join(…)`, a table lookup keyed by the value. A script
can still emit a quote that way, and the earlier claim that *"no caller decides whether
it appears"* was wrong — contradicted two clauses later by its own example, since a
table lookup is keyed by the caller's value and a `join`'s separator is emitted once
per piece of it. The property that survives is narrower and is the one to quote:
**every byte of the result was typed by the author or drawn from the safe alphabet.**
The caller may choose which of the author's bytes appear, and how many times; they
cannot introduce one the author never wrote. That is package S's accepted residue,
unchanged — an author who writes `join` with a `'` separator has written an injection
by hand, with extra steps — and what package 5 removes is the other half, where the
caller supplies the metacharacter and an honest script passes it along. Non-ASCII is
refused as conservatism, not as proof: no Unicode case mapping produces a SQL
metacharacter, but "very likely safe" is not the standard for a rule whose job is to
hold under transformations nobody enumerated.

**The pattern must be a complete regular expression on its own, and that is the
anchoring** (added 2026-08-31, after review found the gate passed a breakout). egret
matches an author's `pattern` as `^(?:{pattern})$` — built in `anchored`, enforced by
an *unanchored* `is_match` in `coerce` — and wrapping author TEXT is not anchoring:
`a)|(a` becomes `^(?:a)|(a)$`, which compiles — and the two anchors have landed on two
different alternatives, `^(?:a)` bounding only the start and `(a)$` only the end (with a
third alternative, `a)|([a-z]+)|(a`, the middle one bounds nothing at all). Every
character of it is inside the safe alphabet, so the language analysis below answered
`Ok`, publish returned 200, and the endpoint accepted
`a' UNION SELECT password FROM users --` on a parameter a script concatenates: the
whole claim of this section, defeated by two parentheses.

`compile_pattern` now requires `regex_syntax::parse` to succeed on the **raw** pattern
before it wraps anything. A pattern that parses standalone has balanced groups, so
`(?:` … `)` encloses exactly it and the anchors bind the whole language; one that does
not is refused, naming egret's anchoring as the thing it fails, since the author's own
regex tester accepted it. Nothing scans the pattern for `)` — the check is the
structural property that makes the wrapper sound, which is the only form of it that
survives a `regex-syntax` that grows new constructs.

Structural anchoring was considered and not taken: `regex` has no full-match API
(`find` is leftmost-first, so `a|ab` "fully matches" `ab` only by accident of which
alternative won), and building the anchored HIR and printing it back would trust the
HIR printer to round-trip every construct and would make the matched pattern differ
textually from the one the author wrote and the OpenAPI document publishes. The
raw-parse requirement makes the textual wrapper provably equal to a full match, which
is the cheaper way to the same guarantee. Because `compile_pattern` is the one function
both publish and `coerce` go through, a snapshot frozen with a breakout pattern before
this existed now fails to compile at serve time and answers a redacted 500 rather than
the injection.

**Sufficiency is decided on the pattern's LANGUAGE, not on its text.** The author's
pattern is parsed (`regex-syntax`, already in the lock as `regex`'s own dependency) and
every node of the HIR must be placeable inside the alphabet; the match over `HirKind`
has **no `_` arm**, so a dependency upgrade that adds a way to match a character fails
the build rather than being waved through. This is refusal-on-doubt, the opposite of a
scanner that passes what it fails to recognise. `[A-z]` — the classic typo for
"letters", which admits `[`, `\`, `]`, `^` and `` ` `` — is refused with `[` as the
witness, and a check that only looked at a range's endpoints would have passed it.

**Which parameters, and the honest cost.** Every `string` parameter on the endpoint,
not the ones a script was observed to use. Deciding "this parameter reaches the text"
is precisely what the probe cannot do, so a rule resting on that decision would inherit
its blind spots. The other five types need nothing, and that is a fact about `coerce`
rather than a judgement: `Number` is gated by `is_plain_decimal`, `Boolean` is one of
two words, `Date`/`Timestamp` are chrono-reformatted so the caller's spelling never
reaches the engine, and `Enum` returns the **author's** own `allowed` entry.

The cost is real and is named rather than hidden: **an endpoint cannot combine a
generated statement with a free-text `string` parameter.** No pattern over the safe
alphabet accepts a search phrase, so an author who needs one must write that endpoint's
statements as `sql`, where `{{ q }}` binds and no script can reach the value.

**Endpoints already published are refused at SERVE time too** (added 2026-08-31, after
review). A publish-time rule only ever runs on a definition someone publishes, so every
scripted endpoint frozen before this package would have kept its unconstrained `string`
parameter — and stayed exploitable — for as long as nobody edited it. The snapshot
carries `statements` and `params`, so the question is answerable on the serving path,
and an answer that is available is not a disclosure to leave in a report:
`params::resolve` asks `snapshot_script_parameters_are_bounded` before the cache lookup
(so a body cached under the old rule cannot outlive it) and refuses the request as an
internal error. The wire body is `"internal error"`; the parameter's name goes to the
log, for the operator who has to republish. That breaks such an endpoint until it is
republished, which is the cost, and it is the right side of the trade against serving
an injection on a definition egret would no longer accept.

Operators can find them before the first 500 does. Verified against Postgres 16 on a
fixture carrying one affected endpoint and three decoys — a bounded scripted one, an
unbounded *written* one, and a draft with no live version — and it returns only the
first:

```sql
SELECT e.workspace_id, e.method, e.path, v.version, p->>'name' AS parameter
FROM dataapi.endpoints e
JOIN dataapi.endpoint_versions v ON v.id = e.live_version_id
CROSS JOIN LATERAL jsonb_array_elements(v.snapshot->'statements') AS s
CROSS JOIN LATERAL jsonb_array_elements(v.snapshot->'params') AS p
WHERE s ? 'script'
  AND p->>'type' = 'string'
  AND p->'pattern' IS NULL
GROUP BY e.workspace_id, e.method, e.path, v.version, p->>'name'
ORDER BY e.path;
```

**Serving re-asks nothing about the VALUE, and that is the design.** (It asks one
question about the snapshot — whether the constraint is there at all — for the frozen
endpoints above, and that is a different question with a different answer: a refusal of
the endpoint, not of the value.) The constraint itself lives in `pattern` precisely
because `coerce` already applies `pattern` to every `string` value on every request, on
the same path that produces the `Param` the script is handed. The rule the
author is refused by and the rule the caller is bounded by are one rule, so they cannot
drift — package C1's property, and package 5 is only sound because of it. `allowed` is
**not** accepted as an alternative constraint on a `String` for the same reason
inverted: `coerce` reads it only for `Enum`, so accepting it would be a publish-time
rule resting on an enforcement that does not exist.

**Fixed since, on review:** `ParamSpec::allowed` on a `String` was silently ignored by
`coerce` and `check_constraints` did not refuse it — verbatim the "teaches the author a
model where constraints apply everywhere" failure that function's own comment warns
about, missing from the very table that describes it. `allowed` now sits in that table,
gated to `Enum`, which is the one type `coerce` reads it for.

**Carried forward, not fixed:** `validate_preview` applies the rule only to the
statement on screen, because
`SiblingStatements` carries the other statements' text and cannot say whether any of
them is a script; so previewing a *written* statement whose sibling is a script is
silent about the rule and publish refuses it. Closing that needs the preview body to
carry each sibling's source.

## 5. The blind spot Package 2 exists to close

Worth stating on its own because it is a structural failure, not a missing feature. If the token
endpoint breaks or the signing key rotates:

- every call 401s **before** authentication resolves a workspace;
- `dataapi.access_log.workspace_id` is `NOT NULL`, so **no row is written**;
- activity `error_rate` is therefore `None`, which **never fires an alert**;
- auth failures log at `debug` deliberately, to avoid log flood.

The system goes dark and reports itself healthy. Three instruments, one cause, all blind.

## 6. Deliberate exclusions

- **Data classification / PII.** There is none today: `SELECT ssn, email FROM employees` publishes
  as easily as `SELECT count(*)`. Out of scope by decision; Package 1 reduces the exposure without
  addressing classification.
- **Snapshot pinning.** `FOR VERSION AS OF` appears in **no `.rs` file** — the design records it
  was measured working on both engines and then not adopted. A multi-statement endpoint can
  therefore return a `rows`/`total` mismatch with a 200 and no signal. This is a *decision* to
  revisit, not a defect to patch, and the user has deferred it.
- **Signing-key rotation** remains a documented mass outage: `active()` is the only key consulted,
  so rotation invalidates every live token at once. Not in this programme; recorded here so it is
  not rediscovered as new.
- **A workspace over `MAX_DOCUMENTED_ENDPOINTS` (200) live endpoints cannot serve the bearer
  contract at all.** `GET /data-api/v1/{ws}` answers `500 internal` and the consumer has no remedy.
  The management route's remedy is `?endpoint_id=`, which the bearer route deliberately does not
  offer: a consumer asking "what may I call" cannot narrow by an id it does not yet know, and
  accepting one would make the route an existence oracle over endpoint ids.

  Two things make deferring this right rather than merely convenient. First, **the fix a reader
  reaches for is worse than the defect**: a dedicated `ServingError` variant would be forced by
  `the_published_contract_names_every_answer_the_surface_can_give` onto `serve_get` and
  `serve_post`'s published response lists — those two operations can never produce it, so the
  document would be made untrue in order to satisfy a test. Second, the cap is what bounds the
  request's cost (one PDP decision per candidate endpoint), so raising it trades a refusal for an
  amplification.

  The oracle it carries, recorded so it is not rediscovered either: **the count runs over all live
  rows before `can_invoke` filters**, so an account granted nothing still learns that the workspace
  has more than 200 live endpoints — a 500 rather than the empty document it would otherwise get.
  That is a coarse fact about workspace size and nothing about any endpoint's path, parameters or
  columns. It is inherent to bounding the cost before paying it: a count taken after the filter
  would mean running exactly the PDP decisions the cap exists to avoid.

## 7. What "done" means for this programme

A consuming team can fetch the contract with its own credential, pin the version it built against,
and be told before an author breaks it. An operator can see request rate, latency, error rate and
cache behaviour on a dashboard, is paged when an endpoint starts failing **including when it fails
before authentication**, can run more than one replica without silently multiplying every limit,
and can deploy without killing in-flight queries. An endpoint over governed data either runs as
the caller or cannot be published.

Anything beyond that is §6, and is not this programme.
