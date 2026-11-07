# Port 8080 hardening: live result on Trino 483 (Task A1)

Measured 2026-11-07 with `scripts/trino-insecure-http/check.sh`: one coordinator, one worker,
`trinodb/trino:483`, own compose project `trino-insecure-http`, no egret image. The coordinator
config mirrors egret's impersonate-engine render (`http-server.process-forwarded=true`,
`authentication.type=PASSWORD`, HTTPS 8443 with a self-signed PEM keystore, shared secret) with
file access control: catalogs allow all; `impersonation` lets `egret_delegator` become anyone and
`egret-monitor` become only itself. That `catalogs` + `impersonation` set is enough; no `queries`
or `system_information` rules were needed. Passwords, bcrypt (`$2y$`, cost 10) file, shared secret
and keystore are generated per run into a temp dir.

## Measured table

HTTP status is the status of the first `POST /v1/statement`; a query error comes back as HTTP 200
with an error in the body.

| # | Request | `allow-insecure-over-http=true` | `=false` |
|---|---|---|---|
| 1 | plain HTTP, `X-Trino-User: bob`, no credential | 200, `bob` | **403** `Error 403 Forbidden: Authentication over HTTP is not enabled` |
| 2 | Trino CLI (`/usr/bin/trino` inside the coordinator container) `--user ann --session-user bob` | `bob` | `Error starting query at http://localhost:8080/v1/statement returned an invalid response: JsonResponse{statusCode=403, ...} [Error: Error 403 F...` (same 403) |
| 3 | as 1 plus `X-Forwarded-Proto: https`, no credential | **401** `Unauthorized` | **401** `Unauthorized` |
| 4 | as 3 with Basic `egret-monitor`, `X-Trino-User: egret-monitor` | 200, `egret-monitor` | 200, `egret-monitor` |
| 5 | as 4 with `X-Trino-User: bob` | 200 with query error `Access Denied: User egret-monitor cannot impersonate user bob` | same |
| 6 | `GET /v1/info`, no credential | 200 | 200 (unchanged) |
| 7 | `GET /v1/info/state`, `X-Trino-User: admin`, no credential | 200 `"ACTIVE"` | 200 `"ACTIVE"` (unchanged) |
| 8 | `SELECT count(*) FROM system.runtime.nodes` over HTTPS 8443, Basic `egret_delegator`, `X-Trino-Original-User: egret_delegator`, `X-Trino-User: ann` | 200, `2` | 200, `2` (worker registered over `http://coordinator:8080`) |
| 9 | `SELECT current_user`, same as 8 | `ann` | `ann` |

## Findings

- Row 3 is refused at both values: a forwarded-https request is "secure" to Trino, so password
  authentication is mandatory regardless of the flag. The spec's premise holds; not BLOCKED.
- The flag only affects requests that look insecure, that is, plain HTTP without
  `X-Forwarded-Proto: https`. At `false` they are refused with 403 before any identity is read, so
  a spoofed `X-Trino-User` (rows 1, 2) no longer runs.
- `/v1/info` and `/v1/info/state` are not behind the authentication filter: both answer 200 with no
  credential at `false`. The probe and `preStop` need no change.
- The monitor (Basic + forwarded https) works at `false` and still cannot impersonate (row 5).
- Worker registration over plain HTTP 8080 (internal, shared-secret) works at `false`; 2 nodes seen.
- The first `nextUri` behind `X-Forwarded-Proto: https` is an `https://` URL; a client must follow
  its path on the channel it uses (`query.py` does this).
- The worker log lines matching `error` are only config-table names such as
  `http-server.notify-remote-async-errors`; no real errors.

## Decision (brief step 4)

- Rows 6 and 7 pass at `false`: the probe and `preStop` need NO authentication; Task A2 step 4
  is not needed.
- Row 3 is refused: the spec's premise holds, so Task A2 may flip the flag to `false` on every
  egret engine that has an authentication type.
- Caveat for A2: a client reaching the coordinator on plain `:8080` with no forwarded header (an
  in-cluster client such as the Trino CLI) is refused at `false`; only the monitor/ingress path
  (forwarded https plus credential) and HTTPS 8443 work.
