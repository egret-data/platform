# Ranger surface auth: measurements

Plan: `2026-10-20-ranger-surface-auth.md`, spec: `../specs/2026-10-20-ranger-surface-auth-design.md` section 2.
Jar under test for Q1-Q3: `ranger-plugins-common` 2.8.0, a real `RangerBasePlugin("trino","trino")` against a
stub `HttpServer` on 127.0.0.1 serving `tests/ranger/fixtures/service-policies.json` (service `trino_prod`),
`policy.pollIntervalMs=1000`, `policy.rest.client.username=engine-1`, password `pw-ONE`
(`Basic ZW5naW5lLTE6cHctT05F`). Scratch code is not committed.

## Floor: what a plugin sends (committed test `EgretPluginAuthTest`)

| version | result |
|---|---|
| 2.8.0 | PASS: `GET /ranger/service/plugins/secure/policies/download/svc auth=Basic ZW5naW5lLTE6czNjcmV0` |
| 2.9.0 | PASS, identical |
| 2.5.0 | FAIL (the evidence for the floor): `AssertionError: trino: [GET /ranger/service/plugins/policies/download/svc auth=null, RESULT v5]` |

Cold-cache 401 on the first `getServicePoliciesIfUpdated` (client level): 2.8.0 and 2.9.0 return `RESULT null`
without an exception; 2.5.0 (legacy path, no header) also `RESULT null`.

## Q1: does a running plugin re-read its security XML?

Method: `ranger-trino-security.xml` on the classpath from a directory; `init()`, 2.5 s, then rewrite the file with
`pw-TWO` (`ZW5naW5lLTE6cHctVFdP`), clear the log, wait 3.5 s (3 to 4 polls).
Output: after the rewrite every request still carried `auth=Basic ZW5naW5lLTE6cHctT05F` (pw-ONE); `pw-TWO` never appeared.
**Agrees with the spec**: no re-read, a credential change needs an engine restart.

## Q2: what does a 401 do?

Method: full plugin, 1 s polling, admin answers 401 to everything.
- Warm (200 first, allowed=true; then 401, 7 to 8 requests): `allowed=true` afterwards. The plugin keeps serving its in-memory policies.
- Cold, no cache file, 401 from the start: `allowed=false` (deny all), requests kept being retried each second.
- Restart with the cache file on disk (`trino_trino_prod.json`) and admin 401: `allowed=true`, served from the cache file.

**Agrees with the spec** (warm keeps serving, cold denies all). Addition: a cold start with a cache file on disk behaves warm.

## Q3: which URLs does a plugin call in secure mode?

Method: stub answers 404 to everything except `/policies/download/`; `RangerBasePlugin.init()` with credentials; 3.5 s.
Output (distinct requests, 8 total):
```text
GET /ranger/service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
GET /ranger/service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
```
With the roles URL returning 404 the plugin still enforces (`allowed=true`).
**DISAGREES with the spec**: besides the policies download, the plugin polls `GET /service/roles/secure/download/<svc>`
every poll interval (in these runs the roles call precedes each policies call). The spec said no new call appears
beyond policies. Consequences: egret must answer this route in the authenticated router (a 404 is tolerated, but it
will be unauthenticated-looking noise and, once `/service` is removed, the route lives under `/ranger/service/roles/...`);
the auth middleware must cover it, or a 404 behind the middleware must still be a 401 for bad credentials. The
roles download is not served by egret today, and engines work, so serving it stays out of scope. No tags or userstore
endpoint was called in these runs.
