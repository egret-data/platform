# StarRocks Delegation / Security Parity — Design

**Date:** 2026-07-16 · **Status:** Design — awaiting approval · **Branch:** `feat/starrocks-delegation` (off `feat/starrocks-provisioning`)
**Program:** StarRocks-parity-with-Trino, **slice 3 of 3** (final). Slice 1 (Ranger enforcement) merged (#259); slice 2 (provisioning) is PR #261 — this branch is off it. This spec brings StarRocks per-user delegation + transport security to parity with Trino's recent delegation work (#254–#257).

## Goal

Bring StarRocks per-user query delegation and transport security to full parity with Trino:
- **Delegation-mode selection** — `jwt` | `impersonate`, per connection, exactly as Trino.
- **Impersonate mode** — egret connects as a service user and runs the query **as the calling user** via `EXECUTE AS <session_user> WITH NO REVERT` (the StarRocks analogue of Trino's `X-Trino-User`), authorized by Ranger on the switched identity.
- **TLS** — the StarRocks MySQL-protocol connection (`:9030`) runs over TLS with the FE cert pinned, closing the cleartext-JWT-over-plain-TCP hole (`mysql_clear_password` sends the token unencrypted — flagged in slice 1 as prod-unsafe).
- **Provisioning** — a provisioned StarRocks FE is configured for the JWT security integration + FE TLS on `:9030` + an impersonation-capable service user, gated on the engine's delegation config (no more manual `security-integration.sql`).

Proven live: a delegated query in each mode over TLS, Ranger-enforced on the delegated identity.

## Non-goals (scope boundaries)

- **The shared delegation decision + token mint are UNCHANGED.** `egret-query`'s `submit_query` (`execution/routes.rs`) already resolves `(session_user, bearer)` per mode **engine-agnostically** (delegated + `impersonate` → `session_user=Some, bearer=None`; delegated + jwt → mint the caller's engine JWT). The per-user OIDC engine-JWT minter is shared with Trino. This slice does NOT touch either — it only makes the StarRocks **connector** honor the impersonate path + TLS, and provisions the FE to accept them.
- **No new authz.** Impersonate relies on StarRocks Ranger (slice 1) authorizing the switched identity; jwt relies on the FE's `authentication_jwt`.
- **RisingWave / other engines out.**

## Background — current state (verified)

- **Delegation decision is shared + engine-agnostic** (`crates/egret-query/src/execution/routes.rs::submit_query`): for a `db.delegated` connection it produces `(session_user, bearer)` — impersonate mode = `(Some(username), None)`; jwt mode = `(Some(username), Some(minted_jwt))`; a non-OIDC caller on a delegated connection is denied (403), never falling back to service creds.
- **`ConnSpec`** (`connector/mod.rs`) already carries `secret`, `session_user`, `bearer_token`; **`ConnOptions`** (`egret-query-api`) already carries `impersonate: Option<bool>`, `http_scheme`, `tls_cert: Option<String>`. Trino uses all of them; StarRocks uses only `bearer_token`.
- **Trino connector** (`connector/trino.rs`) implements both modes + TLS: bearer → `Authorization: Bearer` + `X-Trino-User=session_user`; impersonate (no bearer) → basic-auth service user + `X-Trino-User=session_user`; TLS via `reqwest::Certificate::from_pem(tls_cert)` pinned as a root when `https`.
- **StarRocks connector** (`connector/starrocks.rs`) does **jwt only, no TLS**: bearer set → `mysql_clear_password` presents the JWT, login user = `session_user`. On the impersonate path (`bearer=None`, `session_user=Some`) it **ignores `session_user`** and connects as the service user — no `EXECUTE AS`, so no impersonation happens. Plain TCP on `:9030`.
- **Provisioning** (slice 2, `egret-orchestrator/src/starrocks.rs`) templates the FE `ConfigMap` but does NOT configure `authentication_jwt`, FE TLS, or an impersonation service user; slice-1's `deploy/starrocks/security-integration.sql` was a manual step.
- **StarRocks capabilities:** `EXECUTE AS <user> WITH NO REVERT` switches the session's identity irreversibly for the connection (the impersonation primitive); the service user needs the `IMPERSONATE ON USER <user>` privilege (or a broad grant). StarRocks FE supports SSL on the query port via `ssl_*` `fe.conf` keys + a keystore. `mysql_async` supports client TLS via `SslOpts` (root-cert pinning).

## Architecture — components

### A. StarRocks connector: impersonate + TLS (`crates/egret-query/src/execution/connector/starrocks.rs`)

The `ConnSpec` → connector mapping gains two behaviors, mirroring `trino.rs`:

1. **Impersonate.** Compute the switch identity the same way Trino does — impersonate when `bearer.is_some() || options.impersonate == Some(true)` and a `session_user` is resolved. Store it as `execute_as: Option<String>`. On **every** connection, if `execute_as` is set, run `EXECUTE AS \`<escaped_user>\` WITH NO REVERT` as the first statement before the query (a StarRocks identifier-escape helper doubles backticks). jwt mode already logs in AS the user, so `EXECUTE AS` is only emitted on the impersonate (non-bearer) path — but a single `execute_as`-when-set rule is correct for both (harmless if the login user already equals it). Decide: emit `EXECUTE AS` only when `bearer.is_none()` (impersonate) to avoid a redundant self-switch on jwt.
2. **TLS.** When `options.tls_cert` is set (egret-provisioned TLS) — or a StarRocks-TLS signal — build `mysql_async::SslOpts` pinning the FE cert (`with_root_certs` / a pinned DER/PEM), and set it on the `OptsBuilder`. `mysql_clear_password` MUST run over TLS; when a bearer/cleartext-plugin path is active and TLS is NOT configured, log a loud warning (dev-only) — production provisioning always sets the cert.

`StarRocksConnector` gains an `execute_as: Option<String>` field + the SslOpts wiring; `execute()` prepends the `EXECUTE AS` statement. Unit-test the opts (SSL present when cert set) + the `EXECUTE AS` emission/escaping.

### B. Provisioning: FE JWT + TLS + impersonation user (`egret-orchestrator/src/starrocks.rs` + `egret-engine-mgmt`)

Extend the slice-2 provisioner's FE `ConfigMap` / bootstrap, gated on the engine's delegation config (a new `StarRocksConfig` delegation block, or reuse `RangerBinding` + a delegation flag):

1. **`authentication_jwt` security integration** — the `fe.conf` / bootstrap SQL that registers the Keycloak JWKS/issuer/audience integration (from slice-1's `security-integration.sql`, now templated with the engine's OIDC coordinates) so jwt-mode logins are accepted.
2. **FE TLS on `:9030`** — `fe.conf` `ssl_*` keys + a mounted keystore/cert (a self-signed cert egret generates + pins via `tls_cert` on the connection, mirroring Trino's provisioned-TLS keystore). The connection egret stores for this engine carries that `tls_cert`.
3. **Impersonation service user** — a service account with `IMPERSONATE` (or `GRANT IMPERSONATE ON ALL USERS`) so impersonate-mode `EXECUTE AS` is authorized; its creds are the connection's `secret`.

The engine's stored connection (`databases` row) is written with the right `impersonate` flag / `tls_cert` so the connector picks the mode automatically.

### C. Frontend: delegation-mode + TLS surfacing

The delegation-mode selector (#257) sets `db.options.impersonate` engine-agnostically; ensure it (and the TLS indicator) is shown for **StarRocks** connections/engines, not Trino-gated. Mostly reuse; audit the engine-config + connection-edit surfaces for `engine === "trino"` gates and open them to StarRocks where the semantics match.

## Data flow

Delegated query → `submit_query` resolves `(session_user, bearer)` by mode (unchanged) → `ConnSpec` → **StarRocks connector**: jwt → TLS + `mysql_clear_password(JWT)` login as the user; impersonate → TLS + service-user login + `EXECUTE AS <session_user> WITH NO REVERT` → StarRocks FE authenticates (JWT integration or service user) → Ranger authorizes the effective identity → rows.

## Testing strategy

- **Unit (CI):** connector opts — SSL present when `tls_cert` set; `EXECUTE AS` emitted (escaped) on the impersonate path and NOT on jwt; the mode-selection mirrors Trino's `bearer || impersonate` rule. Provisioner templating — FE `ConfigMap` carries the JWT integration + `ssl_*` when the delegation block is set, absent otherwise.
- **Live-only (controller-driven):** a provisioned StarRocks with FE TLS + JWT integration + an impersonation user; run a delegated query in **jwt** mode and in **impersonate** mode over TLS, each Ranger-enforced on the delegated identity (masked/denied differs per user). Report exactly what ran.

## Live-only risks (validated at smoke)

1. **`EXECUTE AS … WITH NO REVERT` semantics** on the target StarRocks version + the service user's `IMPERSONATE` grant — one-line SQL/grant fix if the privilege model differs.
2. **FE TLS on `:9030`** — the exact `ssl_*` `fe.conf` keys + keystore format for the `fe-ubuntu` image (live-only, like slice-2's storage/Ranger keys); `mysql_async` cert-pin acceptance.
3. **JWT integration activation** — slice-1 found `CREATE SECURITY INTEGRATION` needs a separate activation step; confirm the templated form activates on the provisioned FE.

## Work breakdown (for the plan)

1. **Connector impersonate** — `execute_as` + `EXECUTE AS` prepend + escaping; unit tests.
2. **Connector TLS** — `SslOpts` cert-pin from `tls_cert`; cleartext-without-TLS warning; unit tests.
3. **Provisioning** — FE JWT integration + TLS keystore + impersonation user, gated on the delegation config; store `impersonate`/`tls_cert` on the engine's connection; templating tests.
4. **Frontend** — surface delegation-mode + TLS for StarRocks (de-Trino-gate); tests.
5. **Gate + LIVE SMOKE** — jwt + impersonate delegated queries over TLS, Ranger-enforced.

## Self-review

- **Scope focused:** connector (EXECUTE AS + TLS) + provisioning FE config + FE surfacing; shared decision/mint explicitly untouched. ✔
- **No placeholders:** concrete `ConnSpec`/`ConnOptions` fields, the shared `submit_query` decision, the `EXECUTE AS` primitive, `mysql_async` SslOpts, and the FE `ssl_*`/JWT-integration wiring are named. ✔
- **Consistency:** mirrors `trino.rs`'s bearer/impersonate/TLS structure; reuses `impersonate`/`tls_cert`/`session_user`; provisioning extends slice-2's `StarRocksProvisioner`. ✔
- **Ambiguity:** "parity" pinned to jwt+impersonate modes + TLS + provisioning; impersonate is `EXECUTE AS`, not a StarRocks proxy-user scheme. ✔
