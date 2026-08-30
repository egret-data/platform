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
| 0 | **Test foundation** | Missing `DATABASE_URL` becomes an error, not a skip; add a path that touches a real engine. | — (parallel stream) |
| 1 | **Governance gate at publish** | A served query runs as the **service account**, never the caller, so Ranger row filters and column masks are evaluated at machine-identity granularity. Nothing today requires a delegated connection. | §4 decision A |
| 2 | **Metrics + `/metrics`** | *In progress.* There are no metrics at all. One outage class is invisible to all three existing instruments simultaneously — see §5. | — |
| 3 | **Deployment hardening** | No engine connection pool (a TCP+TLS handshake **per statement**); no graceful shutdown, so every deploy kills in-flight queries; no resource limits; `/readyz` exists and is never called (probes are `tcpSocket`); no concurrency cap, body limit, or timeout layer on the composed router. | — |
| 4 | **Multi-replica correctness** | Response cache and token bucket are **process-local** while replica count is a Helm value, so every published rate limit silently multiplies by replica count and the cache fragments. | §4 decision B |
| 5 | **Script parameter constraints** | Package S lets a script concatenate a caller's value into SQL text. The publish-time sentinel probe has **two bypasses pinned open by tests asserting `is_ok()`** (`lifecycle.rs:6058`, `:6083`). Nothing re-asks at serve time. | — |
| 6 | **Publish-time diff + acknowledgement** | `publish` validates the draft against *itself*; the only read of `endpoint_versions` is `MAX(version)`. A removed column, narrowed type, renamed parameter or new required parameter publishes silently. Worst case: adding a second statement flips top-level `data` from an array to an object. | — |
| 7 | **Consumer version pinning** | Serving resolves `live_version_id` only; a publish moves every caller at commit. `meta.version` lets a consumer *observe* which version answered but never *ask* for one. | 6 |
| 8 | **Contract reachability** | The OpenAPI document sits behind a human session + `:manage`, so a consumer holding only a `data-api` token cannot fetch its own contract; `error.code` has no `enum` despite being documented as the stable code to branch on. | — |

Two smaller items fold into the packages that own them rather than standing alone:

- **Error classification** (into Package 2 or 3): `internal` is the single code for six unrelated
  causes, so an operator cannot separate "this author's endpoint is broken" from "egret is
  broken". The design doc already argued this and it was never closed. PR #552 fixed only the
  *logging*; the classification is unchanged.
- **Trino 503 handling** (into Package 3): the Trino client protocol defines 503 as *retry, the
  query is alive*; it currently becomes a 500 and abandons a live query.

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

## 7. What "done" means for this programme

A consuming team can fetch the contract with its own credential, pin the version it built against,
and be told before an author breaks it. An operator can see request rate, latency, error rate and
cache behaviour on a dashboard, is paged when an endpoint starts failing **including when it fails
before authentication**, can run more than one replica without silently multiplying every limit,
and can deploy without killing in-flight queries. An endpoint over governed data either runs as
the caller or cannot be published.

Anything beyond that is §6, and is not this programme.
