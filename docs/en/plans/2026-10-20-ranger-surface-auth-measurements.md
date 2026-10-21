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
`pw-TWO` (`ZW5naW5lLTE6cHctVFdP`), clear the log, wait 3.5 s. Request paths are shown without the `/ranger` prefix.
Exact output (the stub's request log, in arrival order):

```text
Q1 after init+2.5s: allowed=true n=6
  1. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  2. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  3. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  4. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  5. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  6. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
Q1 after rewrite (pw-ONE=ZW5naW5lLTE6cHctT05F ; pw-TWO=ZW5naW5lLTE6cHctVFdP) n=8
  1. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  2. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  3. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  4. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  5. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  6. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  7. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  8. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
```

All 14 requests carry the pw-ONE value `ZW5naW5lLTE6cHctT05F`; `ZW5naW5lLTE6cHctVFdP` (pw-TWO) never appears.
**Agrees with the spec**: no re-read, a credential change needs an engine restart.

## Q2: what does a 401 do?

Method: full plugin, 1 s polling; the admin answers 401 to every request in the 401 phase. Exact output
(the `xN` in the warm label is the count taken at the time of the `allowed=` read; the list was printed a moment later):

Warm (200 first, then 401):

```text
Q2-warm before 401: allowed=true
Q2-warm after 401 x6: allowed=true n=7
  1. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  2. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  3. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  4. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  5. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  6. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  7. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
```

Cold (no cache file, 401 from the start):

```text
Q2-cold (no cache file): allowed=false n=8
  1. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  2. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  3. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  4. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  5. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  6. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  7. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  8. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
```

Restart with the cache file `trino_trino_prod.json` on disk, admin 401 from the start:

```text
Q2-restart-with-cache-file-on-disk, admin 401: allowed=true n=8
  1. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  2. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  3. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  4. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  5. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  6. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  7. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  8. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
```

**Agrees with the spec**: warm keeps serving (`allowed=true`), cold denies all (`allowed=false`), and the plugin keeps
retrying every second. Addition: a cold start with a cache file on disk behaves like warm.

## Q3: which URLs does a plugin call in secure mode?

Method: stub answers 404 to everything except `/policies/download/`; `RangerBasePlugin.init()` with credentials; 3.5 s.
Exact output, in the order the requests arrived:

```text
Q3 allowed=true requests n=8
  1. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  2. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  3. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  4. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  5. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  6. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  7. GET /service/roles/secure/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
  8. GET /service/plugins/secure/policies/download/trino_prod auth=Basic ZW5naW5lLTE6cHctT05F
```

The roles call comes first in every cycle, then the policies call; 4 cycles in 3.5 s. With roles answering 404 the
plugin still enforces (`allowed=true`). No tags or userstore endpoint was called.

**DISAGREES with the spec.** The spec (section 2, "Not measured", third bullet) assumed:

> the roles / tags / userstore secure endpoints a plugin may call (egret serves none of them today and engines work, so
> they are out of scope; Task 1 confirms no new call appears in secure mode beyond policies).

A new call does appear: `GET /service/roles/secure/download/<svc>`, every poll interval.

**Recommended spec change (single):** egret keeps NOT serving the roles route (out of scope, as today; the plugin
tolerates the 404 and still enforces). The only required change is that the auth middleware layer covers the whole
`/ranger` surface including unmatched routes, so a request with bad or missing credentials to
`/ranger/service/roles/secure/download/<svc>` gets 401 (not 404) in `required` mode.

## Q4: tag and userstore download paths

Measured 2026-10-21, committed test `EgretPluginAuthTest` (`withCredentialsTagsAreFetchedFromTheSecureTagsPath`,
`withoutCredentialsTagsAreFetchedFromThePlainTagsPath`, `rolesAndUserstorePathsAreRecorded`). Method: a real
`RangerAdminRESTClient` (`init("svc", ...)`, appType `trino`) against a recording stub `HttpServer` that answers 200 with
a minimal body of the right type (`tagVersion` / `roleVersion` / `userStoreVersion` 5), calling
`getServiceTagsIfUpdated(-1, 0)`, `getRolesIfUpdated(-1, 0)` and `getUserStoreIfUpdated(-1, 0)` directly, with
`policy.rest.client.username=engine-1`, password `s3cret` (`Basic ZW5naW5lLTE6czNjcmV0`) and with no credentials.
Command, run once per version, Maven offline: `mvn -o -q test -Dranger.version=<v> -Dtest=EgretPluginAuthTest`
(`Tests run: 6, Failures: 0, Errors: 0, Skipped: 0` for each). Exact output, 2.8.0:

```text
Q4 tags no-creds [GET /ranger/service/tags/download/svc auth=null, RESULT v5]
Q4 roles creds [GET /ranger/service/roles/secure/download/svc auth=Basic ZW5naW5lLTE6czNjcmV0, RESULT v5]
Q4 userstore creds [GET /ranger/service/xusers/secure/download/svc auth=Basic ZW5naW5lLTE6czNjcmV0, RESULT v5]
Q4 roles no-creds [GET /ranger/service/roles/download/svc auth=null, RESULT v5]
Q4 userstore no-creds [GET /ranger/service/xusers/download/svc auth=null, RESULT v5]
Q4 tags creds [GET /ranger/service/tags/secure/download/svc auth=Basic ZW5naW5lLTE6czNjcmV0, RESULT v5]
```

2.9.0: identical, line for line.

| call | with credentials | without credentials |
|---|---|---|
| tags | `GET /service/tags/secure/download/<svc>`, Basic | `GET /service/tags/download/<svc>`, no header |
| roles | `GET /service/roles/secure/download/<svc>`, Basic | `GET /service/roles/download/<svc>`, no header |
| userstore | `GET /service/xusers/secure/download/<svc>`, Basic | `GET /service/xusers/download/<svc>`, no header |

**DISAGREES with Q3's "no tags call".** Q3 saw none because its fixture's service-def has no `contextEnrichers` (and
the download no `tagPolicies`), so no tag enricher was built. egret's `trino.json` declares `RangerTagEnricher` with
`RangerAdminTagRetriever`, which polls through the same admin client, so a Trino engine with credentials fetches its tags
from the **secure** tag path. egret served only `/tags/download/{service}`: under the authenticated surface every such
engine got 404 on its tag download, and every tag-addressed mask, deny and row filter would silently stop applying.
**Fix:** `/ranger/service/tags/secure/download/{service}`, same handler (`download_tags`) and same profile binding
(`authorize_service`) as the plain path.

Userstore: egret serves neither `/xusers/download` nor `/xusers/secure/download`, and no service-def egret ships
declares a user-store enricher (Q3 saw no call). Left unserved, like roles (404 to an authenticated caller, 401 otherwise).
