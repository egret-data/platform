# Trino Dev Config — JWT Auth + File-Based Group ACL

This directory contains dev-only Trino configuration files for F2-P3.
Enforcement and property reconciliation happen in **F2-P5** when the full
Keycloak + Trino stack is brought up.

## ⚠️ F2-P5 FINDING — plain HTTP does NOT enforce JWT auth (TLS required)

Verified live (2026-07-01): with this config over **plain HTTP**, Trino
**accepts** a valid Keycloak JWT (identity resolves, query runs as the user) —
but it does **NOT require** one. A request carrying only `X-Trino-User: <name>`
and **no `Authorization` header** still runs queries (returns data). This is
Trino's documented behaviour: JWT authentication is only truly **enforced over
HTTPS**; `allow-insecure-over-http=true` permits auth over HTTP but leaves the
insecure (trust-`X-Trino-User`) path open. **For real per-user enforcement Trino
must run with TLS** (`http-server.https.enabled=true` + a keystore) and
`allow-insecure-over-http` removed. The egret→Trino connector already sends the
Bearer JWT (F2-P3); the gap is transport-level enforcement on the Trino side.
Iceberg-catalog (egret-catalog, F2-P2) enforces in-process regardless of
transport, so this caveat is Trino-specific.

## How identity is derived

Trino derives the **user principal** from the JWT field named in
`http-server.authentication.jwt.principal-field` (set to `sub`).  That means
the Trino username for every query is the Keycloak subject UUID of the
authenticated user.

## Groups

Group membership is **not wired yet**.  In F2-P5, groups will come from one
of:

- **JWT `groups` claim** — set a `groups-field` property in
  `config.properties` (e.g. `http-server.authentication.jwt.groups-field=groups`)
  once Keycloak is configured to include the claim.
- **File-based group provider** — a separate `group-provider.properties` that
  maps usernames → groups independently of the JWT.

## rules.json

`rules.json` is a starter template:

- Members of the `data-team` group get **full** catalog access.
- All other authenticated users get **read-only** access.

The exact group names, catalog list, schema-level rules, and table-level
rules are refined in P5 after real user/group fixtures exist in Keycloak.

## Files

| File | Purpose |
|---|---|
| `config.properties` | Single-node coordinator config + JWT auth (dev only, plain HTTP) |
| `access-control.properties` | Enables the file-based system access control |
| `rules.json` | Starter catalog ACL — `data-team` full, everyone else read-only |

> **Warning**: `allow-insecure-over-http=true` and the placeholder
> `internal-communication.shared-secret` are **dev-only** values.  Remove
> them before any non-local environment.
