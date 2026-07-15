# Secure delegated Trino queries — impersonate mode (design)

**Status:** design · **Date:** 2026-07-15 · **Author:** egret

**Goal:** Make per-user delegated queries on egret-provisioned Trino engines
work **securely** on Ranger-governed engines, so a query submitted by user *U*
runs in Trino **as U** and is governed by *U*'s Ranger data policies — without a
shared long-lived credential leaking cross-user access.

This spec covers **impersonate mode** (the default). A second **jwt mode** (TLS
coordinator + bearer-only per-user JWT) is a separate follow-up spec; this design
adds the mode framework so jwt can slot in later.

---

## Background — why this design

`query.databases.delegated = true` marks a connection that must run as the
calling end-user (feature B3a). Two prior attempts and their outcomes:

- **PR #252 (merged):** the connector's bearer path now sends
  `X-Trino-User = <username>` and the provisioner renders
  `jwt.principal-field = preferred_username`. This gets a JWT request *past*
  Trino's "X-Trino-User must be sent" 401.
- **PR #253 (closed, INSECURE):** serving a Ranger self-impersonation policy so
  a user can impersonate themselves. Live testing on a real Ranger engine proved
  this is a **security hole**: over plain HTTP, Trino authorizes the impersonation
  check against the client-supplied `X-Trino-User` / `X-Trino-Original-User`
  header, **not** the authenticating JWT principal. So *any* valid engine-JWT
  holder can run as *any* user that has an impersonation grant. Both a `{USER}`
  macro policy and per-user literal policies leak. Proven: member-sso's own JWT +
  `X-Trino-User=admin-sso` → ran as admin-sso once admin-sso had a self-policy.

The insight: over plain HTTP the JWT does not bind the query identity. Two secure
ways forward:

- **impersonate mode** (this spec): don't rely on the end-user's JWT for identity
  at all. egret authenticates to Trino as a **trusted admin service user** (basic
  auth, password only in egret) and sets `X-Trino-User = <the authenticated
  egret user>`. Trino authorizes "can admin impersonate U?" — admin holds the
  impersonate grant, so it is allowed, and the query runs as U. End-users never
  reach Trino directly, so they cannot forge `X-Trino-User`. Works over HTTP.
- **jwt mode** (later): TLS coordinator + bearer-only. Proven on a throwaway TLS
  Trino: over HTTPS the JWT binds the identity, `bearer-only` works with no 401
  (`current_user = member-sso`), and `X-Trino-User=admin-sso` spoofing is
  **rejected** (`User member-sso cannot impersonate ...`, principal correctly
  member-sso). Needs coordinator TLS provisioning; out of scope here.

---

## Architecture

A provisioned Trino engine carries a **`delegationMode`** (`impersonate` |
`jwt`, default `impersonate`). For an impersonate-mode engine:

1. **Provision time:** egret generates a random password for a fixed Trino
   service user `egret_delegator`, stores it in an egret secret, adds
   `egret_delegator` to the coordinator's file password authenticator, and seeds
   a Ranger policy granting `egret_delegator` the **impersonate** privilege on
   all users (plus a benign per-user session-property allow — see §4).
2. **Assignment/materialization:** the delegated `query.databases` row for the
   engine is materialized with the admin secret and the impersonate connection
   options.
3. **Query time:** the engine query route resolves the caller's username, opens
   the admin secret, and hands the existing `TrinoConnector` a basic-auth +
   impersonate spec (`X-Trino-User = <caller>`). No JWT is minted.

Components and their responsibilities:

| Unit | Responsibility | Depends on |
|---|---|---|
| Engine spec `delegationMode` | Selects impersonate vs jwt at provision | engine-mgmt CRUD |
| Provisioner (`egret-orchestrator/src/trino.rs`, engine-mgmt) | Generate admin cred → secret; add `egret_delegator` to `passwordUsers`/`password.db`; seed Ranger admin policy | egret-secrets, egret-policy store |
| Materialization (egret-query registry, engine→workspace assignment) | Write delegated DB row: `secret_id=<admin>`, `options={user, impersonate:true}` | engine row + admin secret |
| Query route (`egret-query/src/execution/routes.rs`) | Delegated + impersonate → `session_user=caller`, open admin secret, no mint | ensure_username, secret store |
| `TrinoConnector` (unchanged) | basic-auth as `egret_delegator` + `X-Trino-User=session_user` | — |

---

## 1. Data model & mode selection

- **Engine spec:** add `delegationMode: "impersonate" | "jwt"` under
  `trinoConfig` (engine spec jsonb). Default `impersonate` when absent (so
  existing engines are impersonate on next re-provision). Surfaced through the
  `CreateEngineBody.trinoConfig` passthrough — no new top-level body field.
- **Materialized delegated DB (impersonate):** `delegated=true`,
  `engine_id=<engine>`, `secret_id=<admin secret>`,
  `options={ "user": "egret_delegator", "impersonate": true }`, `host` =
  coordinator, `engine="trino"`. (jwt-mode materialization keeps `secret_id=null`
  and no `impersonate` — the mint path.)
- The delegated DB row therefore self-describes its mode: `delegated &&
  options.impersonate == true` ⇒ impersonate; `delegated && !impersonate` ⇒ jwt.
  No separate lookup of the engine at query time.

## 2. Provisioning

When `delegationMode == impersonate`, at provision (create/update) time:

- **Admin credential:** egret generates a random password for user
  `egret_delegator`, stores the plaintext in an egret **secret** (per engine),
  and records the secret id on the engine (so materialization can reference it).
  Rotatable by re-provision.
- **Trino password auth:** add `egret_delegator` (with a bcrypt hash of the
  generated password) to the rendered `passwordUsers` → `password.db`. This
  requires `http-server.authentication.type` to include `PASSWORD` (it already
  does: `PASSWORD,JWT`). The delegator uses the PASSWORD authenticator.
- **Seed Ranger policy (stored, not injected):** create a real, auditable policy
  in the policy store for the engine's Trino service via the existing
  policy-admin create path — visible/editable in the Policies UI:
  - `egret_delegator-impersonate`: users `[egret_delegator]`, resource
    `trinouser = [*]`, access `impersonate` = allow.
  Seeding is idempotent (skip if a policy of that name already exists on the
  service). Bump the service `policy_version` so plugins re-download.

> **Least privilege (refinement over "full-priv admin"):** the delegator needs
> **only the impersonate grant**, not broad data access. After impersonation the
> Trino identity *is* the end-user, so all data/table checks run against the
> end-user's own policies — the delegator never reads data as itself. This is
> tighter than a full-privilege admin and sufficient.

## 3. Query path & connector

- `execution/routes.rs`, delegated branch:
  - `options.impersonate == true` ⇒ `session_user = ensure_username(caller)`,
    `bearer = None`; the admin `secret_id` is opened by the normal job-submit
    secret path and passed as the connection secret. **No JWT mint.**
  - else (jwt mode) ⇒ current mint path (unchanged; used once jwt mode ships).
  - A non-delegated connection is unchanged (`session_user = caller email`, no
    bearer, no impersonate).
- **`TrinoConnector` is unchanged.** With `options.user = egret_delegator`,
  `options.impersonate = true`, `secret = <admin password>`,
  `session_user = <caller>`, its existing non-bearer path already sends
  `X-Trino-User = session_user` + `basic_auth(egret_delegator, secret)`.

## 4. Session property (query timeout)

The connector sets `X-Trino-Session: query_max_run_time=<n>s` on every statement.
Trino checks `alter` on the `systemproperty` resource for the **effective query
user** (the impersonated end-user). Without a grant this fails with
`Cannot set system session property query_max_run_time`. Seed one benign,
auditable allow policy per Trino service alongside the delegator policy:

- `egret-query-timeout`: users `[{USER}]`, resource
  `systemproperty = [query_max_run_time]`, access `alter` = allow.

This is **not** an impersonation leak: `{USER}` here scopes a system *session
property* to the requesting user's own session; it grants no cross-user identity
or data access. (This is the one salvageable, benign half of the closed #253.)

## 5. Security model

- The `egret_delegator` password lives **only** in an egret secret; end-users
  never learn it and never connect to Trino directly.
- `X-Trino-User` is set by egret's query route to the **authenticated caller's**
  username (`ensure_username(ctx.principal.user_id)`), derived from the egret
  session — an end-user cannot inject or override it.
- Trino authorizes "can `egret_delegator` impersonate `<caller>`?" against the
  seeded impersonate policy (allowed), then runs the query **as the caller**;
  Ranger enforces the caller's own data policies (masking / row-filter / table
  grants).
- Impersonate mode works for **any authenticated egret user**, including
  non-OIDC/local accounts — it does not depend on a stored OIDC refresh token
  (unlike jwt mode).
- Non-Ranger engines: basic-auth impersonate still runs (default allow-all); no
  seeded policy needed, but seeding is harmless.

## 6. Migration / existing engines

- Existing provisioned engines default to `impersonate` but were **not** rendered
  with the `egret_delegator` user or seeded policy. They must be
  **re-provisioned** (`PUT /api/control-plane/engines/{id}` → re-renders config +
  seeds) to enable secure delegation. Until then their delegated DB rows keep the
  old (now non-functional-on-Ranger) mint behavior; no security regression.

## 7. Out of scope (separate specs)

- **jwt mode** (TLS coordinator + bearer-only). Framework field is added here;
  provisioning TLS + connector bearer-only-over-TLS is its own spec.
- Reverting/retiring the `#252` bearer `X-Trino-User` send: kept as-is for jwt
  mode; harmless for impersonate mode (impersonate uses the basic-auth path).
- Per-catalog visibility scoping, StarRocks delegation parity.

## 8. Testing

- **Unit:**
  - provisioner renders `egret_delegator` into `password.db` + emits the
    delegator + query-timeout seed policies when `delegationMode=impersonate`;
    emits neither for `jwt`.
  - materialization writes `secret_id` + `options.impersonate` for an
    impersonate engine.
  - `routes.rs` delegated branch: impersonate ⇒ `(session_user=caller,
    bearer=None)`; jwt ⇒ mint.
- **Live (manual Trino + Ranger):** re-provision an engine in impersonate mode;
  submit a query as an end-user; assert it runs as that user (`current_user`),
  is subject to that user's masking, and that a different end-user cannot be
  impersonated (egret sets `X-Trino-User`, so this is enforced by construction).

## Success criteria

1. A delegated query on an impersonate-mode Ranger engine returns data and runs
   as the calling user (`current_user = caller`).
2. Ranger masking / row-filter for the calling user applies to the delegated
   query.
3. No path lets one end-user run as another (egret owns `X-Trino-User`; the
   delegator credential is never exposed).
4. `delegationMode=jwt` engines are unaffected (no delegator user/policy seeded).
