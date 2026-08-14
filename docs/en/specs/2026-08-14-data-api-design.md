# Data API — design

> **Status:** Design approved, not yet implemented · **Date:** 2026-08-14 · **Reference:** [sqlrest](https://github.com/inrgihc/sqlrest) v1.9.0 (domain model + UX only; no code reuse — sqlrest is Java/Spring, egret is Rust)

## 1. Problem

egret can query Trino and StarRocks interactively (SQL Lab, Explore, dashboards), but has no way to expose a governed query as a **machine-callable HTTP endpoint**. Downstream consumers — internal services, low-code tools, partner integrations — currently have no option except direct engine credentials, which bypasses Ranger row/column policy and leaves no per-consumer audit trail.

Data API closes that gap: a workspace member authors a parameterized SELECT, publishes it at a stable URL, and external clients call it with an API key. Every call runs under a real egret principal, so Ranger enforcement and audit apply unchanged.

## 2. Locked decisions

| Decision | Choice | Rationale |
|---|---|---|
| Build strategy | Native Rust crate in egret; sqlrest is a **reference** for domain model and UX | One system, one authz model, one UI. Running sqlrest alongside would mean two identity stores, two datastores, two UIs. |
| Endpoint source | Raw SQL + params is the core; a "Publish as API" shortcut seeds SQL from a Saved Query / Dataset | Raw SQL is the flexible primitive; the shortcut removes the copy-paste step without constraining the model. |
| Consumer identity | **Service account = a real egret principal** | Queries run through machine-idp → engine token → Ranger. One authz model; audit resolves to a named identity. |
| Read/write | **Read-only (SELECT) in v1** | Narrow attack surface; no transaction/rollback/idempotency design needed. Writes deferred to v2. |
| v1 scope | Version & rollback · rate limit / quota · response cache · auto-OpenAPI + Try-it | All four selected. |
| Serving surface | Same process as `egret-server`, but structured to split into its own binary later | Modular-monolith default without foreclosing isolation. |
| Tenancy / URL | Workspace-scoped, workspace in the path: `/data-api/v1/{workspace}/{path}` | Globally unique, debuggable, matches Cedar's workspace scoping. |

## 3. Blocking prerequisite — parameter escaping is unsafe for a public surface

`crates/egret-query/src/execution/template.rs:62` renders string parameters as:

```rust
_ => Ok(format!("'{}'", p.value.replace('\'', "''"))),
```

It doubles single quotes only and **ignores backslash**. StarRocks is MySQL-dialect and by default does not set `NO_BACKSLASH_ESCAPES`, so `\` is an escape character and `\'` breaks out of the literal. Trino is unaffected (it does not process backslash escapes in standard string literals).

Today this is reachable only by an authenticated user with SQL Lab access. Data API would put the same render path **behind an API key on the public internet**. The fix is therefore in v1 scope, not a follow-on.

The existing unit test `string_param_neutralizes_injection` (`template.rs:93`) passes whether or not the hole is present — it only covers the classic `' OR '1'='1` case. It is not evidence.

`Connector::execute` (`crates/egret-query/src/execution/connector/mod.rs:70`) takes `sql: &str` and has no bound-parameter path, so this cannot be fixed by "just using prepared statements" without a trait change.

**Fix (v1):** split `literal()` into a dialect-aware escaper in `egret-query`, parameterized by `Engine`:

- **Trino** — double `'` only (current behaviour, which is already correct for Trino).
- **StarRocks** — escape `\` → `\\` **first**, then `'` → `''`. Order matters; reversing it re-introduces the hole.

Because the fix lands in `egret-query`, SQL Lab is patched by the same change.

**Deferred, deliberately:** bound parameters at the connector layer. StarRocks (`mysql_async`) supports them directly; Trino needs `X-Trino-Prepared-Statement` + `EXECUTE … USING`. That is a `Connector` trait change large enough to be its own project. The dialect escaper sits behind a single function so it can be replaced in one place.

**Parameters are values, never identifiers.** `{{param}}` may not appear in a column name, table name, or `ORDER BY` position. A dynamic column is expressed as an `enum` parameter with an explicit allow-list, rendered by looking the value up in that list. An allow-list is the only safe form for an identifier.

## 4. Architecture

### 4.1 Crates

| Crate | Role |
|---|---|
| `egret-dataapi-api` | Leaf contract crate, pure-serde: endpoint definition DTOs, param spec, published snapshot. Consumable by `egret-server` and frontend codegen without pulling in sqlx/axum. |
| `egret-dataapi` | Seam crate: store (owns the `dataapi` schema), publish lifecycle, service accounts + keys, rate limit, cache, access log, **two routers**. |

### 4.2 Two routers, deliberately disjoint

- `management_router()` → nested under `/api` like every other seam. Session cookie + CSRF + Cedar. Authoring, publishing, key management.
- `serving_router()` → nested under `/data-api/v1`. **No cookie, no CSRF**, `Authorization: Bearer` only. A fully separate middleware stack.

This boundary matters more than it looks. CSRF has previously killed a production POST in this codebase with no router-level test able to see it. The serving surface must inherit **no** control-plane middleware implicitly; every layer on it is declared explicitly.

### 4.3 Execution seam

Add a `QueryExecPort` trait to `egret-query-api`. `egret-dataapi` holds `Arc<dyn QueryExecPort>`; `egret-query` implements it over its concrete state. This mirrors the `NotebookDocsPort` pattern in `crates/egret-analytics-api/src/port.rs`.

```rust
// crates/egret-query-api/src/port.rs (new)
#[async_trait::async_trait]
pub trait QueryExecPort: Send + Sync {
    /// Run already-rendered SQL as `principal`, honouring delegation
    /// (Trino `X-Trino-User` / StarRocks `EXECUTE AS`) and Ranger.
    async fn execute_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
        limits: Limits,
    ) -> Result<TabularResult, egret_core::Error>;

    /// Render parameters into safe SQL for the target database's dialect.
    fn render(
        &self,
        database_id: Uuid,
        sql: &str,
        params: &[QueryParam],
    ) -> Result<String, egret_core::Error>;

    /// Resolve the output column schema without running the query. Used by
    /// validate-before-publish (§5.3) to freeze the output shape into the
    /// snapshot.
    async fn describe_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, egret_core::Error>;
}
```

**Contract change to note:** `egret-query-api`'s module doc (lines 10–11) currently states the crate deliberately has *no* `QueryPort` trait, because "the seam's runtime surface is the concrete `Connector` data-plane". Adding this trait reverses that intent for a good reason — `egret-dataapi` needs a substitutable seam so it can move to its own binary in v2. **The doc comment must be updated in the same change**, not left contradicting the code.

### 4.4 Registration

- Migration `0119_dataapi.sql`, `CREATE SCHEMA IF NOT EXISTS dataapi` — schema-per-seam, matching `query`, `policy`, `automation`.
- `crates/egret-server/src/lib.rs`: one `doc.merge(egret_dataapi::openapi())` line and two `.nest()` blocks. The merge list stays explicit — the file's own note at line 90 says "do NOT loop".

## 5. Data model

Two pieces of existing egret infrastructure remove tables that sqlrest needs:

**`access_grants` is the unified grant table** (`crates/egret-auth/src/grants.rs:47`) with `principal_type` ('user' | 'group') and `scope_type` ('platform' | 'workspace' | 'object') + `object_type`. No bespoke grant table (sqlrest's `ApiAssignment ↔ ClientGroup`) is needed. Granting a service account access to an endpoint is one row:

```text
principal_type='user', principal_id=<service_account>,
scope_type='object',  object_type='data_api_endpoint', scope_id=<endpoint_id>
```

**Service accounts are rows in `users`.** `migrations/0002_identity.sql:5` has `provider CHECK (provider IN ('local','oidc'))`; extend to `('local','oidc','service')`. This is what makes "a real principal" real: Cedar, `access_grants`, Ranger, machine-idp and audit all treat a service account exactly like a user, with no second identity store.

*Known wart:* `users.email` is `citext NOT NULL UNIQUE`. Service accounts have no real email, so one is synthesised as `svc.<slug>@<workspace-slug>.service.egret.local`. Making `email` nullable would touch a large amount of code that assumes it exists; the synthetic address is the smaller change. `users.status` already allows `'disabled'`, which serves as the service-account kill switch.

### 5.1 Schema `dataapi`

```sql
dataapi.endpoints            -- the draft; freely editable
  id, workspace_id, folder_id, name, description,
  path, method, database_id, sql,
  params jsonb, output jsonb, cache jsonb, limits jsonb, rate_limit jsonb,
  source_kind ('raw'|'saved_query'|'dataset'), source_ref uuid,
  live_version_id uuid NULL,          -- points at the serving snapshot
  state ('draft'|'live'|'disabled'),
  created_by, created_at, updated_at
  UNIQUE (workspace_id, method, path)

dataapi.endpoint_versions    -- IMMUTABLE snapshots
  id, endpoint_id, version int, snapshot jsonb, note, created_by, created_at
  UNIQUE (endpoint_id, version)

dataapi.api_keys             -- credentials for the serving surface
  id, workspace_id, service_account_id (→ users.id),
  name, prefix text, secret_hash text, expires_at, last_used_at,
  revoked_at, created_by, created_at
  UNIQUE (prefix)

dataapi.access_log           -- one row per call
  id, ts, endpoint_id, version, service_account_id, api_key_id,
  status_code, row_count, duration_ms, cache_hit bool,
  engine_query_id, client_ip, error_code
```

### 5.1.1 Config shapes

The four jsonb config columns, and the shape frozen into `endpoint_versions.snapshot`:

```jsonc
// params — declared parameters, in declaration order
[ { "name": "date", "type": "date", "location": "query",
    "required": true, "default": null, "allowed": null,
    "description": "Reporting day" },
  { "name": "region", "type": "enum", "location": "query",
    "required": false, "default": "ALL",
    "allowed": ["ALL", "APAC", "EMEA"] } ]
// type:     string | number | boolean | date | timestamp | enum
// location: query | body        (never path — the path is free-form, §5.2)
// allowed:  required for enum; the only safe form for an identifier (§3)

// output — response shaping
{ "shape": "objects",            // objects | arrays
  "naming": "as_is",             // as_is | camel | snake
  "columns": [ { "source": "total_revenue", "name": "revenue",
                 "format": "number:2" } ] }

// cache
{ "enabled": true, "ttl_seconds": 60, "scope": "per_principal" }

// limits
{ "max_rows": 10000, "timeout_seconds": 30 }

// rate_limit
{ "per_key_qps": 5, "burst": 10, "per_endpoint_qps": 50 }
```

### 5.1.2 Success response

```json
{
  "data": [ { "day": "2026-08-01", "revenue": 1234.50 } ],
  "meta": {
    "columns": [ { "name": "day", "type": "date" },
                 { "name": "revenue", "type": "decimal" } ],
    "row_count": 1,
    "version": 3,
    "cached": false,
    "request_id": "01J..."
  }
}
```

`meta.version` is the published version that served the call — it makes a client-side bug reproducible against the exact snapshot that produced it. `meta.cached` is not cosmetic either: without it, a stale-looking result is indistinguishable from a fresh one during debugging.

`dataapi.endpoints.folder_id` and `dataapi.api_keys.service_account_id` are **cross-schema FKs** into `public.folders(id)` and `public.users(id)`. This bends the schema-per-seam ownership rule, but `folders` and `users` are already shared cross-cutting tables and duplicating them would be worse. Accepted and recorded here.

### 5.2 Folders are not URL segments

egret's `folders` (`migrations/0021_folders.sql:4`) is `workspace_id + entity_type + name`, attached via `folder_id … ON DELETE SET NULL`. Users rename, move and delete folders freely. sqlrest's `SQLREST_API_GROUP` is likewise only `id + name`, with `path` a separate field on the assignment — **sqlrest does not put the group in the URL either**.

A URL is a published contract with external clients; a folder is a display convenience. If a URL segment came from the folder name, renaming a folder or moving an endpoint would silently break every deployed client, and `ON DELETE SET NULL` would leave an endpoint with no segment at all.

| | Source | Mutable? |
|---|---|---|
| Folder | `folders` with `entity_type='data_api_endpoint'`, same as chart/dashboard/dataset | Freely; no URL impact |
| URL path | `dataapi.endpoints.path` — free-form user-set string, may contain `/` | Changing it is a breaking change; the UI must warn |

```text
GET /data-api/v1/sales/reports/daily-revenue
                 └─ws─┘ └─────path────────┘
```

`reports/` there is part of the user-typed `path`, unrelated to which folder the endpoint sits in.

### 5.3 Lifecycle

```text
draft ──validate──> publish(v N) ──> live
  ▲                                   │
  └────── keep editing (no effect) ───┘
                    rollback ──> point live_version_id at v N-1
                    disable  ──> state='disabled', serving returns 404
```

**Invariant: the serving path never reads `dataapi.endpoints`.** It reads `endpoint_versions.snapshot` via `live_version_id`, only. This is what makes "editing the SQL does not change behaviour for running clients" true structurally rather than by discipline. If serving read the draft table, versioning would be decoration.

`validate` runs before publish and rejects anything that fails:

- The SQL is exactly **one** statement, and that statement is a `SELECT`. A leading `WITH … SELECT` (CTE) is permitted; anything else — DML, DDL, `SET`, or a second statement after a `;` — is rejected.
- Every `{{param}}` resolves to a declared parameter, and every declared parameter is referenced.
- `describe_as` (§4.3) resolves the output schema, which is frozen into the snapshot.
- If `cache.scope` is `shared`, the target connection must not be delegated (§6.3).

**Allowed methods in v1 are `GET` and `POST`, both read-only.** `POST` exists only for parameter payloads too large or too structured for a query string; it carries the same single-SELECT restriction. `PUT`/`PATCH`/`DELETE` are rejected — accepting them would imply a write semantics v1 does not have.

Publish / rollback / key creation / key revocation are emitted as `egret-audit` events — no extra table.

**Operational caveat that must surface in the UI:** publishing an endpoint does **not** grant data access. Because queries run as the service account under Ranger enforcement, an admin must also grant that service account in Ranger. An endpoint published without that step fails with an engine permission error, which must be mapped to a message saying so rather than leaking the raw engine error.

## 6. Auth and request flow

### 6.1 API keys

Format `egt_<prefix>_<secret>`: 12-character `prefix` (unique-indexed, used for lookup) and a 32-byte random `secret`, base64url. Stored as `prefix` + `secret_hash`. The secret is shown exactly once, at creation.

**Hash with SHA-256; do not use `crates/egret-auth/src/password.rs`.** Argon2 there is right for user-chosen passwords and wrong for API keys: a 32-byte random secret already has sufficient entropy that no brute-force-resistant KDF is needed, while paying ~50–100 ms of KDF on *every* API request is unacceptable. Compare in constant time.

### 6.2 Per-request chain

```text
GET /data-api/v1/{ws}/{path}
 │
 1. key → service account   (active, not revoked, not expired)   ✗→ 401
 2. (ws, method, path) → endpoint → live_version_id → snapshot   ✗→ 404
 3. service account's workspace == {ws} in path                  ✗→ 404
 4. Cedar: dataapi:invoke on Object data_api_endpoint:{id}       ✗→ 403
 5. rate limit (per endpoint and per key)                        ✗→ 429 + Retry-After
 6. cache lookup                                                 hit → return
 7. render params → dialect-safe SQL                             ✗→ 400
 8. machine-idp mints an engine token for the SA
    → QueryExecPort::execute_as
 9. Ranger filters rows/columns at the engine, as the SA
10. shape output → store in cache → access_log → return
```

Step 3 returns **404, not 403**, on purpose: a key belonging to another workspace must not be able to distinguish "no such endpoint" from "exists, but you may not call it".

### 6.3 Cache must be principal-scoped

Step 9 filters data **per principal**, so two service accounts calling the same endpoint with the same parameters can legitimately receive different rows. A cache key that omits the principal leaks data straight through Ranger.

The cache config therefore carries a `scope`:

- `per_principal` (default) — the cache key includes `service_account_id`.
- `shared` — permitted **only when the connection is not delegated**. Checked at validate time, so a misconfiguration cannot be published in the first place.

Placing this check at validate time rather than at runtime means the unsafe configuration never reaches production.

### 6.4 Rate limiting

`crates/egret-auth/src/ratelimit.rs` exists but its **semantics are login-style lockout** (exceed the threshold → locked out for a fixed period), which is wrong for an API. Data API needs a token bucket: 429 + `Retry-After`, retryable as soon as the bucket refills.

Write a `TokenBucket` in `egret-dataapi`, but **keep that module's `check_at(key, now)` shape** — the clock is injected, so tests do not depend on wall-clock time.

Put it behind a `RateLimitStore` trait. v1 is in-memory per process; when the serving surface splits into a multi-replica binary, a shared-store implementation replaces it without touching call sites.

## 7. Error handling

One stable, machine-readable envelope:

```json
{ "error": { "code": "param_invalid", "message": "...", "request_id": "01J..." } }
```

**Never return a raw engine error to an external caller.** Trino/StarRocks errors carry table names, column names and sometimes host information — on a key-authenticated public endpoint that is a metadata leak. Map to fixed codes and log the full detail internally against `request_id` so support can correlate.

| HTTP | code | When |
|---|---|---|
| 400 | `param_invalid` / `param_missing` | wrong type, absent, or outside the allow-list |
| 400 | `row_limit_exceeded` | result exceeded the configured row cap |
| 401 | `key_invalid` | key unknown, expired, or revoked |
| 403 | `forbidden` | Cedar denied `dataapi:invoke` |
| 403 | `data_access_denied` | Ranger denied — message states the service account lacks a data grant (§5.3) |
| 404 | `not_found` | absent, disabled, or wrong workspace |
| 429 | `rate_limited` | with `Retry-After` |
| 502 | `engine_unavailable` | engine unreachable |
| 504 | `timeout` | exceeded the snapshot's timeout |

**The serving path applies its own timeout** rather than relying on JobManager. `describe()` (`connector/mod.rs:81`) already carries the identical note — it wraps its own timeout because it is called directly with no outer JobManager. The serving path is likewise direct, so it carries the same obligation.

**Exceeding the row cap is an error, not a silent truncation.** Returning 200 with truncated data is the worst available failure mode for a data API: the client believes it has every row and produces wrong numbers with no signal. `400 row_limit_exceeded`, stating the cap.

## 8. Observability

- `dataapi.access_log` records every call. It is the fastest-growing table in the system: partition monthly plus a retention job, reusing the existing gc-worker pattern from catalog CE6 rather than writing a new one.
- Management UI statistics (call count, error rate, p50/p95, cache hit rate per endpoint) are computed from `access_log` — the equivalent of sqlrest's `HourCount` / `DateCount` / `NameCount`.
- `request_id` is propagated to the engine and stored alongside `engine_query_id`, so a slow API call traces back to the exact query on Trino/StarRocks.
- Publish, rollback, key creation and key revocation are `egret-audit` events.

## 9. Testing

Green tests are not the same as verified behaviour, and this repository has the scar tissue to prove it. Each item below states what would have to fail for the test to be evidence.

1. **Escaping** — per-dialect unit tests **plus** an integration test that runs against a real StarRocks. The existing unit test passes with the hole present, which is exactly how it was missed the first time; a unit test alone is not evidence here.
2. **Every authz gate is mutation-checked** — remove the gate and the test must go red. Tests run as a real non-admin service account: measuring with an admin identity bypasses the very check under test.
3. **Version isolation** — publish v1, edit the draft, call again, and the v1 behaviour must persist. This is the test that proves §5.3's invariant. Verify it by pointing serving at the draft and confirming the test goes red.
4. **Cache leak** — two service accounts with different Ranger grants, same endpoint and parameters, must receive different results, and the second must not be served the first's cached rows. Requires a real Ranger.
5. **Rate limit** — driven through `check_at(key, now)` with an injected clock, no `sleep`. The two time points in any assertion must be genuinely separated, never able to coincide.
6. **Live check** — a real `curl` against a published endpoint on kind, as a required step in the plan rather than an optional one.

CI runs in full locally before merge, including `cargo fmt`. Never run two workspace test suites concurrently.

## 10. Out of scope for v1

Deferred deliberately, each recorded so it is a decision rather than an omission:

- **Writes (INSERT/UPDATE/DELETE)** and multi-statement transactions — needs transaction boundaries, rollback, write audit, and a policy for who may publish a write endpoint.
- **Bound parameters at the connector layer** — see §3.
- **Groovy/script execution engine** (sqlrest's `ExecuteEngineEnum::SCRIPT`) — a large sandboxing problem for marginal benefit over SQL.
- **MCP tool export** (sqlrest's `McpToolEntity`) — natural v2 follow-on once endpoints are stable.
- **IP allow/deny lists** (sqlrest's `FirewallRulesEntity`) — better solved at ingress than in application code.
- **Bulk import/export of endpoint definitions** — worth having, not worth blocking v1.
- **Promoting `dataapi.api_keys` into `egret-auth`** — keys stay in the `dataapi` schema while Data API is the only consumer. When a second surface needs machine credentials, they move up.
