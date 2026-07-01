# StarRocks Dev Config — JWT Security Integration (F2-P4)

This directory contains the StarRocks `authentication_jwt` security integration
artifact for F2-P4. The integration is applied manually and validated end-to-end
in **F2-P5** against the running `starrocks/allin1-ubuntu:latest` image.

## Connector status

The egret-engine `StarRocksConnector` (Task 1 of this phase) is **complete and
unit-tested**. It presents the user's OIDC engine JWT via the `mysql_clear_password`
plugin on every query connection. This README covers the server-side configuration.

## How to apply

Wait until the StarRocks FE is healthy (the compose healthcheck polls
`mysql -h127.0.0.1 -P9030 -uroot -e 'select 1'`), then run:

```bash
mysql -h127.0.0.1 -P9030 -uroot < deploy/starrocks/init/security-integration.sql
```

This creates the `egret_oidc` security integration. Activation is a separate step
confirmed at P5 (see below).

## What F2-P5 finalizes

| Item | Detail |
|---|---|
| **JWKS URL host** | The SQL uses compose-internal DNS (`keycloak:8080`). If the FE resolves JWKs from the host instead (e.g. when running StarRocks outside compose), change `jwks_url` to `http://localhost:8081/realms/egret/protocol/openid-connect/certs`. |
| **Issuer / audience** | `required_issuer` and `required_audience` must match what the `starrocks` audience mapper in Keycloak realm `egret` (added in F2-P1) emits. Confirm by decoding a live token and comparing the `iss` and `aud` claims. |
| **Group → role mapping** | Notebook users carry a `groups` JWT claim. Options: (a) native StarRocks RBAC — `CREATE ROLE` per group, `GRANT ... TO ROLE` keyed on the JWT groups claim once the integration surfaces it; (b) a StarRocks GROUP PROVIDER DDL that reads `groups` from the integration and maps to database roles. The right approach depends on the allin1-ubuntu version's DDL surface area. |
| **Integration activation** | `CREATE SECURITY INTEGRATION` registers the integration but does not activate it. Three candidate forms are documented as comments in `init/security-integration.sql` (ADMIN SET FRONTEND CONFIG `authentication_chain`, ALTER SYSTEM, or GROUP PROVIDER DDL). The correct syntax is confirmed and uncommented at P5. |

## Files

| File | Purpose |
|---|---|
| `init/security-integration.sql` | Creates the `egret_oidc` JWT security integration; activation options are commented for P5 confirmation |

## Security note

The `mysql_clear_password` plugin transmits the JWT as cleartext over plain TCP.
Port 9030 in this compose has **no TLS**. This is intentional for local dev only.
Any deployment beyond the dev compose must enable StarRocks FE TLS (or mTLS) on
port 9030 before enabling `mysql_clear_password` authentication in production.
