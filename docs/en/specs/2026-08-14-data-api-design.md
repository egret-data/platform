# Data API — design

> **Status:** P2 (service accounts + OAuth2 clients + management routes) and P3 (endpoint drafts, validate/publish/rollback/disable, audit) SHIPPED; **no endpoint is callable yet** — the serving surface `/data-api/v1` is P4, the frontend P5 · **Date:** 2026-08-14 · **Reference:** [sqlrest](https://github.com/inrgihc/sqlrest) v1.9.0 (domain model + UX only; no code reuse — sqlrest is Java/Spring, egret is Rust)

## 1. Problem

egret can query Trino and StarRocks interactively (SQL Lab, Explore, dashboards), but has no way to expose a governed query as a **machine-callable HTTP endpoint**. Downstream consumers — internal services, low-code tools, partner integrations — currently have no option except direct engine credentials, which bypasses Ranger row/column policy and leaves no per-consumer audit trail.

Data API closes that gap: a workspace member authors a parameterized SELECT, publishes it at a stable URL, and external clients call it with an OAuth2 token minted for a service account (§6.1). Every call runs under a real egret principal, so Ranger enforcement and audit apply unchanged.

## 2. Locked decisions

| Decision | Choice | Rationale |
|---|---|---|
| Build strategy | Native Rust crate in egret; sqlrest is a **reference** for domain model and UX | One system, one authz model, one UI. Running sqlrest alongside would mean two identity stores, two datastores, two UIs. |
| Endpoint source | Raw SQL + params is the core; a "Publish as API" shortcut seeds SQL from a Saved Query / Dataset | Raw SQL is the flexible primitive; the shortcut removes the copy-paste step without constraining the model. |
| Consumer identity | **Service account = a real egret principal**, holding OAuth2 `client_credentials` clients on egret's own machine-IdP | Queries run through machine-idp → engine token → Ranger. One authz model; audit resolves to a named identity. |
| Read/write | **Read-only (SELECT) in v1** | Narrow attack surface; no transaction/rollback/idempotency design needed. Writes deferred to v2. |
| v1 scope | Version & rollback · rate limit / quota · response cache · auto-OpenAPI + Try-it | All four selected. |
| Serving surface | Same process as `egret-server`, but structured to split into its own binary later | Modular-monolith default without foreclosing isolation. |
| Tenancy / URL | Workspace-scoped, workspace in the path: `/data-api/v1/{workspace}/{path}` | Globally unique, debuggable, matches Cedar's workspace scoping. |

## 3. Blocking prerequisite — parameter escaping is unsafe for a public surface

**Two** sibling helpers render a string value as a SQL literal, and both are backslash-blind:

- `literal` — `crates/egret-query/src/execution/template.rs:62` (SQL Lab `{{param}}`)
- `sql_literal` — `crates/egret-bi/src/chartquery.rs:135` (chart and dashboard filter values), whose own doc comment calls it "the security-critical primitive"

Both do:

```rust
format!("'{}'", value.replace('\'', "''"))
```

They double single quotes only and **ignore backslash**.

This is an inconsistency between siblings rather than a blind spot: `like_literal` (`crates/egret-bi/src/schema.rs:161`) and `sr_escape_dq` (`crates/egret-engine-mgmt/src/engines.rs:3062`) already escape `\` correctly. `sql_literal` is reachable with the lowest BI permission (`CH_READ`), which makes it the more exposed of the two today. StarRocks is MySQL-dialect and by default does not set `NO_BACKSLASH_ESCAPES`, so `\` is an escape character and `\'` breaks out of the literal. Trino is unaffected (it does not process backslash escapes in standard string literals).

Today this is reachable only by an authenticated user with SQL Lab access. Data API would put the same render path **behind a machine credential on the public internet**. The fix is therefore in v1 scope, not a follow-on.

The existing unit tests on both sites — `string_param_neutralizes_injection` (`template.rs:93`) and `sql_literal_encodes_by_type` (`chartquery.rs:684`) — pass whether or not the hole is present, because neither uses a test value containing a backslash. They are not evidence.

`Connector::execute` (`crates/egret-query/src/execution/connector/mod.rs:70`) takes `sql: &str` and has no bound-parameter path, so this cannot be fixed by "just using prepared statements" without a trait change.

**Fix (v1):** one dialect-aware escaper in **`egret-query-api`**, parameterized by `Engine`, called by both sites. It goes in the contract crate rather than `egret-query` because `Engine` already lives there and `egret-bi` depends on it — that keeps a single implementation instead of two that can drift apart, which is how this inconsistency arose in the first place. The crate is a pure leaf (serde, uuid, utoipa, egret-core), and the escaper is pure string manipulation, so nothing is dragged in.

- **Trino** — double `'` only (current behaviour, which is already correct for Trino).
- **StarRocks** — escape `\` → `\\` and `'` → `''`. Quotes are doubled rather than backslash-escaped precisely so the two passes are order-independent: the quote pass emits no backslash and the backslash pass emits no quote. Had `\'` been used instead, the backslash pass would have to run first or it would corrupt the quotes the other pass produced — a fragility worth designing out rather than commenting on.

Because the fix lands in the shared contract crate, SQL Lab **and** chart/dashboard filters are patched by the same change.

A third site, `starrocks_create_service_user_sql` (`crates/egret-engine-mgmt/src/engines.rs`), had the same gap and is **fixed in the same branch**. An earlier revision of this spec called it latent on the grounds that the username it interpolates is a constant; that reasoning was wrong, because it only covered the username. The password is not a constant: `ensure_delegator_secret` returns `resolve_plaintext(None, id)` on every call after the first, so what gets interpolated is whatever is currently stored in the named secret, not the random value egret originally minted. A stored value containing `\'` breaks out of the literal, and this DDL runs as `root` on the StarRocks FE. It now calls the same `quote_string`, which supplies its own quotes.

**Deferred, deliberately:** bound parameters at the connector layer. StarRocks (`mysql_async`) supports them directly; Trino needs `X-Trino-Prepared-Statement` + `EXECUTE … USING`. That is a `Connector` trait change large enough to be its own project. The dialect escaper sits behind a single function so it can be replaced in one place.

**Parameters are values, never identifiers.** `{{param}}` may not appear in a column name, table name, or `ORDER BY` position. A dynamic column is expressed as an `enum` parameter with an explicit allow-list, rendered by looking the value up in that list. An allow-list is the only safe form for an identifier.

## 4. Architecture

### 4.1 Crates

| Crate | Role |
|---|---|
| `egret-dataapi-api` | Leaf contract crate, pure-serde: endpoint definition DTOs, param spec, published snapshot. Consumable by `egret-server` and frontend codegen without pulling in sqlx/axum. |
| `egret-dataapi` | Seam crate: store (owns the `dataapi` schema), publish lifecycle, service accounts + their OAuth2 clients, rate limit, cache, access log, **two routers**. |

### 4.2 Two routers, deliberately disjoint

- `management_router()` → nested under `/api` like every other seam. Session cookie + CSRF + Cedar. Authoring, publishing, service-account and client management.
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

dataapi.service_accounts     -- the workspace binding for a machine principal
  user_id uuid PK (→ users.id ON DELETE CASCADE),
  workspace_id, name, description, created_by, created_at
  UNIQUE (workspace_id, name)

dataapi.clients              -- OAuth2 client_credentials clients (P2, §6.1)
  id, service_account_id (→ dataapi.service_accounts.user_id ON DELETE CASCADE),
  name, client_id text UNIQUE, client_secret_hash text,
  expires_at, revoked_at, last_used_at, created_by, created_at

dataapi.access_log           -- one row per call
  id, ts, endpoint_id, version, service_account_id, client_id,
  status_code, row_count, duration_ms, cache_hit bool,
  engine_query_id, client_ip, error_code
```

**Why the workspace binding is its own table rather than a column on `users`.**
`users` is platform-level, and every other consumer of it — the admin Users
page, the Access console's principal picker, `find_by_id` for display — would
have to learn to ignore a workspace column that means nothing for a human. The
binding lives in `dataapi.service_accounts` instead, and `users` gains only what
it must: `provider = 'service'`, plus a `CHECK` that a service row can never
carry a `password_hash`. The login handler already requires one, but that is a
single code path away from being wrong forever; the constraint is where it
cannot be forgotten.

`dataapi.clients` is deliberately shaped like `catalog.catalog_clients` (0053) —
same idea, same machine-IdP, second audience. N clients per service account, so
rotation is mint-new-then-revoke-old rather than a break-then-fix window.

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

// output — response shaping. These two fields are the whole of it; there is
// no per-column rename or format list.
{ "shape": "objects",            // objects | arrays
  "naming": "as_is" }            // as_is | camel | snake

// cache
{ "enabled": true, "ttl_seconds": 60, "scope": "per_principal" }

// limits
{ "max_rows": 10000, "timeout_seconds": 30 }

// rate_limit
{ "per_key_qps": 5, "burst": 10, "per_endpoint_qps": 50 }
```

**A misspelled key on the request body is a 400, not a silently dropped value.** The endpoint create (`POST`) and update (`PATCH`) bodies carry `#[serde(deny_unknown_fields)]`, and so do the nested config objects `params`, `output`, `cache`, `limits` and `rate_limit`. Both layers are needed: the attribute on the request body does not reach a nested struct, and nested is where the damage was — `{"rate_limit": {"per_client_qps": 5}}`, the spelling an earlier draft of this document specified, used to return 201 with `per_key_qps` defaulted to `0`. A rate limit the author believes they set and did not is worse than an error. Every field is either required or defaulted, so nothing legitimate is refused; the shapes above are the exact spellings.

The two snapshot-internal types, `EndpointSnapshot` and its `columns`, deliberately do **not** carry the attribute: the server writes them for itself, so an author typo is impossible there. Note the cost this buys on the config types — they are also how the stored `jsonb` is read back, so adding a field in a later version and then rolling the binary back makes the older code refuse rows the newer one wrote, as a 500 on read rather than a silent misparse.

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

`dataapi.endpoints.folder_id` and `dataapi.service_accounts.user_id` / `.workspace_id` are **cross-schema FKs** into `analytics.folders(id)`, `public.users(id)` and `public.workspaces(id)`. This bends the schema-per-seam ownership rule, but `folders` and `users` are already shared cross-cutting tables and duplicating them would be worse. Accepted and recorded here.

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

**Invariant: routing comes from the endpoint row, behaviour comes from the snapshot.** P4 must match an inbound request against `dataapi.endpoints (method, path)` restricted to `state = 'live'`. That pair of columns is the one carrying `UNIQUE (workspace_id, method, path)`, so it is the only place where "two endpoints claiming one URL" is impossible; the snapshot's own `method`/`path` are the record of where a version was published, not a routing key, and routing on them would lose the constraint. Everything that decides what the request **returns** — `sql`, `params`, `output`, `cache`, `limits`, `rate_limit` and the frozen `columns` — is then read from the snapshot reached through `live_version_id`, and from nowhere else. That second half is what makes "editing the SQL does not change behaviour for running clients" true structurally rather than by discipline. If serving took its content from the draft table, versioning would be decoration. §6.2 step 2 is this same flow, per request.

`validate` runs before publish and rejects anything that fails:

- The SQL is exactly **one** statement, and that statement is a `SELECT`. A leading `WITH … SELECT` (CTE) is permitted; anything else — DML, DDL, `SET`, or a second statement after a `;` — is rejected.
- Every `{{param}}` resolves to a declared parameter, and every declared parameter is referenced.
- `describe_as` (§4.3) resolves the output schema, which is frozen into the snapshot.
- If `cache.scope` is `shared`, the target connection must not be delegated (§6.3).

**The one snapshot-external input P4 must re-check: `delegated`.** `EndpointSnapshot.delegated` records whether the connection delegated per-user credentials at the moment the version was published. `query.databases.delegated` is a **mutable** column: flipping it to `true` after a `cache.scope = shared` version was published turns a legitimately-published snapshot into exactly the cross-principal leak the check above exists to refuse, and nothing re-validates when it moves. **P4 must therefore compare the frozen `delegated` against the live column before serving from a shared cache, and refuse when they disagree.** Every other field in the snapshot is self-contained; this one is a witness, and a witness nobody questions is decoration.

**`output`, `limits` and `rate_limit` are frozen, but they are not validated.** The list above is the whole of validation, and it covers `sql`, `params` and `cache` only — there is no rule for those three fields anywhere: not before the publish lock, not in the re-validation after it, and not at serve time. What they gain from the lock is **consistency**, not validation: they are frozen from the same read that was validated rather than from a row nobody looked at. A nonsensical `max_rows` or an unknown `output.shape` is frozen into the snapshot exactly as written, so **P4 must treat all three as untrusted author input** and bound them itself.

**Allowed methods in v1 are `GET` and `POST`, both read-only.** `POST` exists only for parameter payloads too large or too structured for a query string; it carries the same single-SELECT restriction. `PUT`/`PATCH`/`DELETE` are rejected — accepting them would imply a write semantics v1 does not have.

**Seven mutations are emitted as `egret-audit` events** — no extra table. `DataApiState` carries the same `AuditSink` every other seam receives, so they land in the one `audit_log` an operator already queries rather than in a Data-API-local trail:

| Route | `action` | `resource_type` | `detail` |
|---|---|---|---|
| `POST …/endpoints/{id}/publish` | `data_api_endpoint:publish` | `data_api_endpoint` | `version`, `version_id`, `note` |
| `POST …/endpoints/{id}/rollback` | `data_api_endpoint:rollback` | `data_api_endpoint` | `version`, `version_id` — the version made LIVE, not the `VersionRef` the caller typed |
| `POST …/endpoints/{id}/disable` | `data_api_endpoint:disable` | `data_api_endpoint` | `state` |
| `POST …/endpoints/{id}/enable` | `data_api_endpoint:enable` | `data_api_endpoint` | `state`, `live_version_id` |
| `DELETE …/endpoints/{id}` | `data_api_endpoint:delete` | `data_api_endpoint` | `name`, `path`, `method`, `database_id` |
| `POST …/service-accounts/{id}/clients` | `data_api_client:issue` | `data_api_client` | `client_id`, `service_account_id`, `name`, `expires_at` |
| `DELETE …/clients/{id}` | `data_api_client:revoke` | `data_api_client` | — |

All seven go through `egret_audit::mutation_for`, so the workspace, the acting principal and any impersonator are stamped by construction. `resource_id` is the endpoint or client uuid.

**Why `delete` carries columns and `revoke` does not.** Every audited route but one leaves its subject behind for an auditor to join to: `revoke` is a soft `UPDATE revoked_at`, so the `dataapi.clients` row keeps its `client_id`, name and account; `disable` keeps the endpoint row and its version pointer. `DELETE …/endpoints/{id}` is a hard `DELETE` and `dataapi.endpoint_versions` cascades with it, so its event is the **only surviving record that the endpoint existed** — a bare uuid there would resolve to nothing and say that something was deleted without ever saying which URL stopped being served. The columns come from the `DELETE`'s own `RETURNING`, so there is no second query and nothing to race.

**A `client_secret` is never in the detail, and that is a rule rather than an omission.** The issue event records `client_id` and the account — which credential was minted, the question an auditor actually asks — and not the secret, which answers none of it. `audit_log` is append-only and read by more people, for longer, than the one-shot issue response ever reaches: a secret written there outlives every rotation, every revocation and the `dataapi.clients` row itself, and is the one credential lifetime nothing in the product can shorten. `crates/egret-dataapi/tests/audit.rs` asserts both that no `client_secret` key is present **and** that the secret's own bytes appear nowhere in the detail, since a renamed field would walk past a key-only check; the second half is applied to every event of every test in that file, so it is a crate-wide guard and not one assertion on one handler.

**The table above is the whole audited set, and the rest of the Data API management surface is NOT audited yet.** Stated positively so it cannot be read as a list with one footnote: the four mutating routes that emit nothing today are `POST …/service-accounts` (create), `DELETE …/service-accounts/{id}` (delete), `POST …/endpoints` (create draft) and `PATCH …/endpoints/{id}` (edit draft). The most significant of those is the service-account delete: it hard-deletes a machine principal along with its `access_grants` and every client it owns, and leaves no audit record of having done so. Closing these is not scheduled; it belongs with P5's management UI, where the surface stops changing shape.

**One further honest bound.** `AuditSink::emit` is a non-blocking `try_send` that drops the event (with a warning) on a full channel, so an audit record is best-effort evidence and never a precondition of the mutation it describes.

**Operational caveat that must surface in the UI:** publishing an endpoint does **not** grant data access. Because queries run as the service account under Ranger enforcement, an admin must also grant that service account in Ranger. An endpoint published without that step fails with an engine permission error, which must be mapped to a message saying so rather than leaking the raw engine error.

## 6. Auth and request flow

### 6.1 Credentials: OAuth2 `client_credentials`, not a bespoke API key

An earlier revision of this spec specified a bespoke key, `egt_<prefix>_<secret>`, hashed with SHA-256 and compared on every request. **That is not what was built, and the reason it was abandoned is worth stating**: egret already runs a machine-IdP (`egret-machine-idp`) that mints signed JWTs for the embedded Iceberg catalog's clients. Inventing a second credential format would have meant a second issuer, a second revocation story, a second thing to get constant-time comparison right in, and no interoperability with any OAuth2 client library.

What ships instead is a **second audience on the existing IdP**:

- A service account owns **N clients** in `dataapi.clients`. Each is a `client_id` (`egret-dataapi-<uuid>`) and a 32-byte random `client_secret`, base64url. The secret is shown exactly once, at creation, and is not recoverable — the management API's list DTO has no secret field *at all*, not a skipped or optional one.
- The client exchanges those at `POST /oauth/token` (`grant_type=client_credentials`) for a JWT with `aud: "data-api"`, plus `ws` and `egret_user_id` claims. `egret-dataapi`'s `ClientStore` implements the same `ClientVerifier` trait the Catalog's registry does; the IdP holds a list of verifiers and the audiences do not know about each other.
- Every failure mode of verification — unknown `client_id`, revoked, expired, disabled principal, archived workspace, wrong secret — returns the *same response*. A distinguishing error would be a free oracle for anyone probing client ids. **Two caveats, because "the same response" is narrower than "indistinguishable".** First, the *timing* differs: only a request that gets as far as a wrong secret pays the Argon2 verification, so a wrong secret against a real `client_id` is measurably slower than any rejection that returns before the hash — which distinguishes "this client id exists" from "it does not". Equalising that needs a dummy-hash verify on the early-return paths, and is deliberately not implemented in P2. Second, a *corrupt stored hash* is the one failure that is not `Ok(None)`: `PasswordHash::new` fails, `verify` returns `Err`, and the caller sees a 500 rather than an authentication failure.

**Hashing is Argon2, and the earlier objection to it no longer applies.** The objection was that paying ~50–100 ms of KDF on *every API request* is unacceptable — which is true, and is exactly why the KDF is not on that path. The secret is verified **once per token**, not once per call; the serving surface then validates a JWT signature. Argon2 costs a few tens of milliseconds every 10 minutes per client, and matches `CatalogStore::verify_client` rather than introducing a second hashing scheme.

**The revocation trade-off, stated plainly.** A stateless token cannot be revoked before it expires. Revoking a client sets `revoked_at`, so no *new* token can be minted, but a token already issued stays valid until it expires. `DATA_API_TTL_SECS = 600` is that bound: after revoking, the exposure window is at most 10 minutes. Instant revocation was considered and rejected — it requires a per-request lookup, which gives back precisely the throughput this design was chosen for.

**Be precise about what a "kill switch" buys, because it is easy to overstate.** Four actions stop a compromised client from minting any *further* token, and `verify` honours all four (each is test-covered in `clients_store.rs`): revoking the client (`revoked_at`), disabling the principal (`users.status <> 'active'`), archiving the workspace (`workspaces.status <> 'active'`), and deleting the service account (which cascades to its clients). **None of them recalls a token already issued** — the serving surface validates a JWT statelessly, which is the entire point of §6.2 step 1. The 600s TTL is therefore an irreducible bound, not a worst case that some faster action can beat.

**Why `verify` reads `workspaces.status`, and not just for tidiness.** Archiving a workspace is the operator's natural decommission action, and every Data API management route resolves through `WsContext`, which 404s on a workspace that is not active. Archiving therefore *removes the routes that could revoke a client or disable its principal*. Without the workspace-status check, archiving would leave every credential in that workspace simultaneously **live and unrevokable** — the operator performs the action that most looks like shutting it all down, and gets the exact opposite. Because the recovery surface is gone by then, authentication has to honour the workspace's status itself; nothing else is left to.

Disabling the principal is reachable over the API: `PATCH /api/control-plane/users/{id}` with `{"status":"disabled"}` accepts a service account. Only that route's *profile* fields (name/email) refuse `provider = 'service'` rows — the status branch does not, precisely because it is this kill switch, and it still applies the `user:disable` permission re-check, the last-platform-admin guard, and immediate session revocation.

**Who can mint a credential, and what the upgrade turns on.** `service_account:read` and `service_account:manage` are seeded onto the built-in `ws_admin` role (migration 0120) and stop there — not `ws_editor`, because a service account is a machine principal an admin then grants data access to in Ranger, which is an administrative act rather than an editorial one. State the consequence plainly rather than leaving it to be discovered: `ws_admin` is a single global role row, so that migration reaches **every existing `ws_admin` assignment in every existing workspace at once**, and the `data_api` feature module treats an absent entitlement row as enabled — which is every workspace predating it. So on upgrade, every current workspace admin can immediately mint machine credentials for their workspace, with no operator action and no per-workspace opt-in. That reach is intended, but it appears at upgrade rather than being switched on, so it is disclosed here and in the migration comment. An operator who does not want it must either write a `data_api` entitlement row disabling the module for those workspaces, or remove the two permissions from `ws_admin`.

`client_credentials` has no refresh token, by design: the client re-requests with its credentials. Every OAuth2 library already does this.

**Management surface** (P2, workspace-scoped, session + CSRF like every other `/api/ws/…` group):

```text
GET    /api/ws/{workspace}/data-api/service-accounts
POST   /api/ws/{workspace}/data-api/service-accounts
DELETE /api/ws/{workspace}/data-api/service-accounts/{id}
GET    /api/ws/{workspace}/data-api/service-accounts/{id}/clients
POST   /api/ws/{workspace}/data-api/service-accounts/{id}/clients   -> the secret, once
DELETE /api/ws/{workspace}/data-api/clients/{id}
```

Two permissions, both `Scope::Workspace` and both seeded on `ws_admin` only (migration 0120): `service_account:read` to view accounts and clients, `service_account:manage` to create, delete and **issue**. Issuing gates on `:manage` and never on `:read` — a client is a bearer credential for that account's data access, so handing one out is strictly more powerful than reading the list. Every handler resolves the account (or client) against the workspace from the `WsContext` extractor, never from a second read of the path segment, and returns **404 rather than 403** on a mismatch — the same rule as §6.2 step 3.

### 6.2 Per-request chain

```text
GET /data-api/v1/{ws}/{path}
 │
 1. verify JWT via JWKS → principal + workspace from claims      ✗→ 401
 2. (ws, method, path) → endpoint → live_version_id → snapshot   ✗→ 404
 3. service account's workspace == {ws} in path                  ✗→ 404
 4. Cedar: dataapi:invoke on Object data_api_endpoint:{id}       ✗→ 403
 5. rate limit (per endpoint and per client)                     ✗→ 429 + Retry-After
 6. cache lookup                                                 hit → return
 7. render params → dialect-safe SQL                             ✗→ 400
 8. machine-idp mints an engine token for the SA
    → QueryExecPort::execute_as
 9. Ranger filters rows/columns at the engine, as the SA
10. shape output → store in cache → access_log → return
```

**Step 1 is stateless and costs no DB round-trip.** The bearer token is a JWT signed by egret's own machine-IdP, so the serving path verifies a signature against JWKS and reads `egret_user_id` and `ws` straight out of the claims — there is no credential lookup, and no Argon2, on the hot path. That is the whole reason §6.1 chose a token exchange over a per-request key. The price is the revocation window §6.1 bounds at 600s.

Step 3 returns **404, not 403**, on purpose: a token belonging to another workspace must not be able to distinguish "no such endpoint" from "exists, but you may not call it". The management API applies the same rule to service accounts and clients (§6.1).

### 6.3 Cache must be principal-scoped

Step 9 filters data **per principal**, so two service accounts calling the same endpoint with the same parameters can legitimately receive different rows. A cache key that omits the principal leaks data straight through Ranger.

The cache config therefore carries a `scope`:

- `per_principal` (default) — the cache key includes `service_account_id`.
- `shared` — permitted **only when the connection is not delegated**. Checked at validate time, so a misconfiguration cannot be published in the first place.

Placing this check at validate time rather than at runtime means the unsafe **shape** can never enter `endpoint_versions`. That is precisely what publish-time validation guarantees, and it is not the whole problem: `query.databases.delegated` is mutable, so a connection can be flipped to delegated *after* a `shared` version was published, and no re-validation runs when it is. The residual is closed on the serving side, by the frozen witness §5.3 describes — **P4 compares `EndpointSnapshot.delegated` against the live column before serving from a shared cache and refuses when they disagree.**

### 6.4 Rate limiting

`crates/egret-auth/src/ratelimit.rs` exists but its **semantics are login-style lockout** (exceed the threshold → locked out for a fixed period), which is wrong for an API. Data API needs a token bucket: 429 + `Retry-After`, retryable as soon as the bucket refills.

Write a `TokenBucket` in `egret-dataapi`, but **keep that module's `check_at(key, now)` shape** — the clock is injected, so tests do not depend on wall-clock time.

Put it behind a `RateLimitStore` trait. v1 is in-memory per process; when the serving surface splits into a multi-replica binary, a shared-store implementation replaces it without touching call sites.

**`RateLimitSpec.burst` sizes the per-CLIENT bucket only.** There is one `burst` field and it is written next to `per_key_qps`; reusing it for the endpoint-wide bucket sizes a limit every client shares with a number the author picked for one of them — a `{per_key_qps: 1, burst: 1, per_endpoint_qps: 1000}` endpoint would then admit one request per second in total, and a second client would be refused by a limit set to 1000. The endpoint-wide bucket takes one second of its own rate as its burst. Both keys are the `endpoint_id` (plus the client id for the per-client one) and never the request path: axum percent-decodes the wildcard capture, so a path-keyed bucket is two buckets a caller can alternate between.

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
| 503 | `cache_scope_stale` | a `shared` version's frozen `delegated` no longer matches the live `query.databases` column (§6.3) |
| 504 | `timeout` | exceeded the snapshot's timeout |

`cache_scope_stale` is a 503 and not a 500: nothing on egret's side failed, the endpoint's published configuration has drifted, and the operator's fix is to re-publish it (or to put the connection back). It is deliberately NOT a silent downgrade to a per-principal key — serving the endpoint a different way would keep it working while hiding the drift from the only person who can correct it.

**The serving path applies its own timeout** rather than relying on JobManager. `describe()` (`connector/mod.rs:81`) already carries the identical note — it wraps its own timeout because it is called directly with no outer JobManager. The serving path is likewise direct, so it carries the same obligation.

**Exceeding the row cap is an error, not a silent truncation.** Returning 200 with truncated data is the worst available failure mode for a data API: the client believes it has every row and produces wrong numbers with no signal. `400 row_limit_exceeded`, stating the cap.

**The cap it states is unvalidated author input.** `limits`, `rate_limit` and `output` are frozen into the snapshot with no validation rule anywhere (§5.3), so the serving path must sanity-check `limits.max_rows` and `limits.timeout_seconds` before acting on them rather than assume a published value is sensible. The same applies to `rate_limit` before §6.4's bucket is sized from it, and to `output.shape` / `output.naming` before §5.1.2's response is shaped by them.

## 8. Observability

- `dataapi.access_log` records every call. It is the fastest-growing table in the system: partition monthly plus a retention job, reusing the existing gc-worker pattern from catalog CE6 rather than writing a new one.
- Management UI statistics (call count, error rate, p50/p95, cache hit rate per endpoint) are computed from `access_log` — the equivalent of sqlrest's `HourCount` / `DateCount` / `NameCount`.
- `request_id` is propagated to the engine and stored alongside `engine_query_id`, so a slow API call traces back to the exact query on Trino/StarRocks.
- Publish, rollback, disable, enable, endpoint deletion, client issue and client revocation ARE `egret-audit` events as of P3 — see §5.3 for the exact `action`/`resource_type`/`detail` of each, for why no `client_secret` is ever in a detail, for why the endpoint delete is the one event that must carry columns, and for the four mutating routes that are still not audited.

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
- **Bound parameters at the connector layer.** This is the largest deferral in the document and the one most likely to be misread, so the reasoning is here rather than only in §3.

  sqlrest — the implementation this design is modelled on — does **not** interpolate. Its MyBatis `#{param}` becomes a real `ParameterMapping`, i.e. a driver bind. We interpolate an escaped literal instead. On the parameter axis the reference implementation is therefore *structurally safer* than what we built.

  Every escaping defect this feature has had exists because of that one choice: the backslash-blind quoting on StarRocks (P1), the `{{param}}` publishable inside a string literal, the scanner that read `#` as a comment while StarRocks read it as whitespace, and the `;` that escaped `describe`'s wrapper (all P3). A bound parameter cannot break out of its own position, so none of that class is reachable.

  **It was deferred for cost, not because interpolation was judged adequate.** `Connector::execute` takes `sql: &str` with no parameter channel; StarRocks could bind natively through `mysql_async`, but Trino needs `X-Trino-Prepared-Statement` plus `EXECUTE … USING`, which is a `Connector` trait change and a second protocol path. Doing it would *delete* the dialect escaper, the raw-byte statement check, and the placeholder-position check — the fix is subtractive, which is the strongest argument for it.

- **Dynamic SQL** — conditional predicates that appear only when a parameter was supplied. sqlrest has this through MyBatis `<if>`, `<where>`, `<set>`, `<choose>`/`<when>`, `<foreach>`; we have none. `{{param}}` substitutes unconditionally, so an optional filter must be written `WHERE (region = {{r}} OR {{r}} IS NULL)`, which is awkward and usually defeats the index the predicate existed for.

  **`rbatis` does not solve this**, and the reason is worth recording so it is not re-proposed. Its dynamic SQL is compile-time: `rb_html(args: TokenStream, func: TokenStream) -> TokenStream` is a proc macro over a `syn::ItemFn`, so the template must be a literal in Rust source at build time. A Data API endpoint is a row in `dataapi.endpoint_versions.snapshot`, authored by a user and published while the system is running — no macro can run over it. It is also inseparable from its own `rbdc` driver layer, which has no Trino driver and would mean rebuilding per-user delegation, engine JWTs and Ranger.

  If it is ever built it must be a **runtime** renderer, and the shape matters more than the feature: a general template engine (Jinja-family) evaluates a user-authored template on a public endpoint, which is a known risk area for Superset and would be a new attack surface here. Build the smallest thing that answers "was this parameter supplied" — no expression language, no function calls, no attribute access.

  **Order matters if both are done: bound parameters first.** Adding conditional rendering on top of string interpolation compounds two risks; adding it on top of binding compounds none.
- **Groovy/script execution engine** (sqlrest's `ExecuteEngineEnum::SCRIPT`) — a large sandboxing problem for marginal benefit over SQL.
- **MCP tool export** (sqlrest's `McpToolEntity`) — natural v2 follow-on once endpoints are stable.
- **IP allow/deny lists** (sqlrest's `FirewallRulesEntity`) — better solved at ingress than in application code.
- **Bulk import/export of endpoint definitions** — worth having, not worth blocking v1.
- **Promoting `dataapi.clients` into `egret-auth`** — the client registry stays in the `dataapi` schema while the Data API is its only consumer. The *protocol* is already shared: `egret-machine-idp` verifies both this registry and the Catalog's through one `ClientVerifier` trait. When a third surface needs machine credentials, the storage moves up too.
