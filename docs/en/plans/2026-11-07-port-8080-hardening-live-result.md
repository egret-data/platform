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
| 6 | coordinator `GET /v1/info`, no credential | 200 | 200 (unchanged) |
| 7 | coordinator `GET /v1/info/state`, `X-Trino-User: admin`, no credential (NOT the preStop, see row 10) | 200 `"ACTIVE"` | 200 `"ACTIVE"` (unchanged) |
| 8 | `SELECT count(*) FROM system.runtime.nodes` over HTTPS 8443, Basic `egret_delegator`, `X-Trino-Original-User: egret_delegator`, `X-Trino-User: ann` | 200, `2` | 200, `2` (worker registered over `http://coordinator:8080`) |
| 9 | `SELECT current_user`, same as 8 | `ann` | `ann` |

| 10 | the real preStop: on the WORKER, `curl -s -X PUT -d '"SHUTTING_DOWN"' -H 'Content-type: application/json' -H 'X-Trino-User: admin' http://localhost:8080/v1/info/state` (worker rendered as egret does: no authentication type, no access-control config) | **403** `Error 403 Forbidden: Management only resource`; worker stays `ACTIVE` | **403**, same text, worker stays `ACTIVE` |
| 10s | row 10 with a worker given file access control allowing `admin` system_information read+write (supplemental) | 200, worker `SHUTTING_DOWN` | 200, worker `SHUTTING_DOWN` |

Raw run: `2026-11-07-port-8080-hardening-live-result.output.txt` (output of `check.sh`, no secrets).
Rows 2 and 9 are in that file; the table above is checked against it.

## Second configuration: PASSWORD auth with NO https listener

egret renders TLS only for delegation-capable engines, so `check.sh` also runs a coordinator with
`http-server.authentication.type=PASSWORD` and no `http-server.https.*` keys
(`etc/coordinator-notls.config.properties`). Same file access control and password file.

| # | Request | `true` | `false` |
|---|---|---|---|
| start | coordinator reaches `"starting":false`, worker registers | yes | **yes: Trino starts at `false` with no https listener** |
| 1 | plain HTTP, `X-Trino-User: bob` | 200, `bob` | **403** `Error 403 Forbidden: Authentication over HTTP is not enabled` |
| 3 | plus `X-Forwarded-Proto: https`, no credential | 401 `Unauthorized` | 401 `Unauthorized` |
| 4 | forwarded https, Basic `egret-monitor` | 200, `egret-monitor` | 200, `egret-monitor` |
| 6 | coordinator `GET /v1/info` | 200 | 200 |
| 8 | `SELECT count(*) FROM system.runtime.nodes` as the monitor (plain HTTP + forwarded header) | `2` | `2` |
| 10 | worker preStop PUT | 403 `Management only resource` | 403 `Management only resource` |

So, with no https listener, the monitor (row 4) is NOT broken at `false`: it still authenticates
because its request carries `X-Forwarded-Proto: https`.

## Scope of the measurement (what was and was not measured)

- The monitor and the ingress are SIMULATED: row 4 sets `X-Forwarded-Proto: https` by hand on a
  direct connection to the coordinator. A real kube API-server proxy or ingress controller was
  NOT measured, so whether they send that header (and Trino accepts it) at `false` is unverified here.
- The worker has no `http-server.authentication.type`, so Trino's authentication filter and the
  flag do not apply to it; its preStop result depends on access control, not on the flag (row 10 vs 10s).

## Findings

- Row 3 is refused at both values and in both configurations: a request carrying
  `X-Forwarded-Proto: https` is "secure" to Trino, so password authentication is mandatory
  regardless of the flag. The spec's premise holds.
- The flag only affects requests that look insecure (plain HTTP without `X-Forwarded-Proto: https`).
  At `false` they are refused with 403 before any identity is read, so a spoofed `X-Trino-User`
  (rows 1, 2) no longer runs. This holds with and without an https listener.
- The coordinator's `/v1/info` and `/v1/info/state` answer 200 with no credential at `false`
  (rows 6, 7): a coordinator probe needs no credential.
- The monitor path measured (forwarded header + Basic) works at `false` and cannot impersonate (row 5).
- Worker registration over plain HTTP 8080 (internal, shared secret) works at `false`; 2 nodes seen.
- PRE-EXISTING, independent of the flag: with the worker rendered as egret renders it (no
  access-control config), the exact preStop PUT is refused 403 `Management only resource` at
  `true` as well as at `false`, and the worker does not enter `SHUTTING_DOWN`. It succeeds only when
  the worker's access control allows `admin` to write system information (row 10s). Whether a
  deployed egret worker has such a rule (for example through its Ranger/OPA wiring) was not measured.
- The first `nextUri` behind `X-Forwarded-Proto: https` is an `https://` URL; a client must follow
  its path on the channel it uses (`query.py` does this).
- The worker log lines matching `error` (4 per run) are all `io.trino.bootstrap.engine` config-table
  rows such as `http-server.notify-remote-async-errors`; there are 0 `ERROR`/`Exception` lines
  (the script prints the counts and any such lines).

## Decision (brief step 4)

- Rows 6 and 7 pass at `false` on the coordinator: a coordinator probe or GET needs no credential.
- The flag does not change the worker preStop (row 10 and 10s identical across flag values), so
  flipping the flag cannot break it. Task A2 should treat the preStop 403 as a separate, pre-existing
  question about worker access control.
- Row 3 is refused: Task A2 may flip the flag to `false` on engines with an authentication type, with and
  without an https listener, provided the monitor and ingress really send `X-Forwarded-Proto: https`
  (simulated here, unmeasured against the real proxies).
- Caveat: a client on plain `:8080` without the forwarded header (an in-cluster client such as the
  Trino CLI) is refused at `false`.

## End to end on the opa-e2e stack (Task A5)

Run 2026-11-08 on a local `egret-opa-e2e` stack (ports 28091/28092), scripts at commit
`26ff43e40`. This is NOT the run of record: the only `egret-opa-e2e-backend:dev` image on the
machine was built 2026-11-05, before Task A2, so egret rendered
`allow-insecure-over-http=true` on trino-opa. A new image could not be built safely (other stacks
were up). The CI `opa-e2e` job on the pushed branch is the run of record.

What the stale image showed, unchanged:

- `up.sh` step 11b, trino-opa on the Ranger plugin as rendered (`true`): `FAIL identity
  trino-opa/ranger plain-http-no-credential: want 403, got 200` (the query ran as bob); the run
  stopped there. Rows 2–4 of the check passed. So the check fails the job on a regression to `true`.

Then the A2 render was EMULATED: `allow-insecure-over-http` set to `false` by `sed` in the
provisioned trino-opa container and `docker restart` (once on Ranger, again after the switch
re-provisioned it on OPA); trino-ranger copies trino-opa's lines (up.sh step 7b).

| Check (identity-check.sh) | trino-opa, Ranger plugin | trino-opa, OPA plugin | trino-ranger |
|---|---|---|---|
| plain HTTP, `X-Trino-User: bob`, no credential | 403 | 403 | 403 |
| plus `X-Forwarded-Proto: https`, no credential | 401 | 401 | 401 |
| Basic `egret_delegator`, forwarded https, `X-Trino-Original-User: egret_delegator`, `X-Trino-User: bob` | 200 | 200 | 200 |
| its `SELECT current_user` | `[["bob"]]` | `[["bob"]]` | `[["bob"]]` |
| `GET /v1/info`, no credential | 200 | 200 | 200 |

`layer2.sh` then ran every scenario with the harness on the delegator path: 501 cases,
`same` 485, `opa-hides-rows` 15, `opa-refuses` 1, `PASS` (16 min). `@session-user` jobs still
reach Trino's impersonation check for the scenario user (`Access Denied: User … cannot impersonate
user carol|root|bob`), so authenticating as the delegator does not change what those jobs compare.

The delegator password came from the stack's Postgres, opened with the public dev master key in
`deploy/opa-e2e/egret.env` (`scripts/opa-e2e/trino-auth.sh`); the 200 rows above prove it.
