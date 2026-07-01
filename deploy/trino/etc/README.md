# Trino Dev Config — JWT Auth + File-Based Group ACL

This directory contains dev-only Trino configuration files for F2-P3.
Enforcement and property reconciliation happen in **F2-P5** when the full
Keycloak + Trino stack is brought up.

## ✅ F2-P5 — verified live (2026-07-01): JWT auth ENFORCED over TLS

Three things were found and fixed while validating against the real
Keycloak + Trino stack:

1. **TLS is required for enforcement.** Over plain HTTP, Trino *accepts* a valid
   JWT but does **not require** one — a request with only `X-Trino-User` and no
   `Authorization` header still ran queries. TLS closes this: enable
   `http-server.https.enabled` + a keystore and drop `allow-insecure-over-http`.
   Now verified: no-auth → **401**, bad token → **401**, valid JWT → **200** and
   runs as the user. Run `./gen-certs.sh` to make the dev keystore
   (`trino-keystore.p12`, gitignored) and connect on the HTTPS port (8443,
   host-mapped 8453).
2. **`principal-field=preferred_username`** (not `sub`) — so the Trino user is
   the human username from the token (`tester`), not the subject UUID.
3. **The egret→Trino connector must NOT send `X-Trino-User` with a JWT** — the
   token *is* the identity; sending `X-Trino-User` too made Trino reject it as
   impersonation (`User <sub> cannot impersonate user tester`). Fixed in the
   connector (`egret-engine` `TrinoConnector::auth`): bearer → JWT only.

Iceberg-catalog (egret-catalog, F2-P2) enforces in-process via limes regardless
of transport, so the TLS caveat is Trino-specific.

> Note (deployment): the egret connector talks to Trino over HTTPS — for a
> self-signed dev cert the connector must trust it (real cert in prod). The
> Keycloak realm also needs the `basic` client scope so access tokens carry
> `sub` (used by egret-catalog); fixed in `deploy/keycloak/egret-realm.json`.

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
