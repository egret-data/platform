# Data Apps P4e — live result (edit mode on kind, over HTTPS)

2026-10-03. Chromium 151 (Playwright 1.62) against a real Kubernetes deployment: kind cluster `egret`,
a separate release `egret-verify` (`just deploy-kind-apps`), images tagged `p4e`, ingress-nginx,
self-signed TLS, hosts `verify.localtest.me` (egret) and `apps.verify.localtest.me` (marimo), OIDC off,
`session.secure` on. marimo 0.25.0 runs in a hardened Pod started by egret's Kubernetes launcher; the
draft comes from Postgres through the pod API. Spec: `docs/en/specs/2026-09-30-marimo-data-apps-design.md`
§6.1, §10. Re-run with `just deploy-kind-apps && just e2e-kind-apps`.

## Result: 5 of 5 pass (33 s), after one real defect

`frontend/tests/e2e/apps-edit-kind.spec.ts`:

| Test | What it shows |
|---|---|
| a hostile app cannot read egret, change egret, or toss a cookie egret believes | author JavaScript, running in the iframe on the apps origin, got `SecurityError` reading the parent, saw no egret cookie, got `TypeError` reading an egret response, its header-bearing POST died at the CORS preflight, and **no app was created from inside an app**. A plain `egret_csrf=evil` cookie *was* tossed into the browser's jar (the apps origin may set a cookie for the parent domain) and the `__Host-egret_csrf` one was **not** overwritten; a POST whose `X-Egret-CSRF` matched the tossed cookie got **403**, one carrying the real token got **201**. |
| a cell asks egret for its databases through the real pod, as the author | pod → egret over the callback URL → gateway, with the per-session token the proxy injected. |
| a save over a draft someone else changed is refused in marimo itself, and a reload recovers | marimo shows egret's text ("saved by someone else"); the draft in Postgres is unchanged; **Reload editor** opens a new session at the latest revision. |
| ambiguous spellings of a blocked route never reach marimo, through the real ingress | see the table below. |
| logging out ends the apps-origin session | the same `__Secure-egret_apps_grant` cookie answers 200 before logout and **401** after (HttpOnly, Secure, path-scoped). |

### The defect it found

**The editor was a blank box.** egret's own SPA server (`frontend/nginx-prod.conf`) sends
`Content-Security-Policy: frame-src …`, a fixed list of embed hosts for the dashboard iframe card. The
apps origin was not on it, so Chromium refused the frame. Nothing short of running it in a browser could
show this: the proxy tests, the real-marimo test and 5,600 frontend tests all passed. Fixed by making the
nginx config a template that the image fills in at start (`APPS_ORIGIN`, restricted to `APPS_*` so nginx's
own `$uri` is untouched) and setting it from `apps.origin`; the two tests that pin that CSP now pin the
placeholder as the only addition.

### What the server logged (ingress-nginx, the passing run)

```text
GET     /api/auth/me          403  35   ← the author's fetch(), cookies and all
POST    /api/ws/default/apps  403  35   ← its no-cors POST
OPTIONS /api/ws/default/apps  403  35   ← the preflight of its header POST
POST    /api/ws/default/apps  403  17   ← the test's POST with a header matching the TOSSED cookie
POST    /api/ws/default/apps  201  343  ← the same POST with the real __Host-egret_csrf token
```

35 bytes is the length of the `Sec-Fetch-Site: same-site` refusal. **D20 is measured**: Chromium sent
`same-site` for requests made by JavaScript on the apps origin, and egret refused every one before any
handler ran — including a plain `GET` with the viewer's cookies, the case the spec worried about. The 17-byte
403 is the CSRF check, rejecting a header that matches only the tossed plain-name cookie. (marimo's
`Referrer-Policy: same-origin` strips the `Referer`, which is why the log has no apps-origin referer.)

### Raw paths through the real ingress (`curl --path-as-is`, a valid apps cookie)

| Path after `/e/<ref>` | Status |
|---|---|
| `/api/version` | 200 |
| `/api/packages/list` | 403 |
| `/../api/packages/list`, `/%2e%2e/…`, `/..%2fapi/…`, `/./…`, `//…`, `%5c…`, `/%252e%252e/…`, `/x/../api/files/list_files` | **400** |

ingress-nginx did not normalise any of them away, so each reached the path guard and was refused there:
0 of the 10 reached marimo.

## What this does not show

- **Firefox and Safari were not run.** D20 rests on `Sec-Fetch-Site`, which all current engines send; a
  browser that does not is protected only by `__Host-` cookies, header-based CSRF and the GET audit.
- **No SQL ran against an engine.** The queries path was exercised through `egret.databases()` (the
  gateway, the token, the pod → egret route), not `SELECT` on Trino or StarRocks, and two viewers under
  different Ranger grants is run mode (P5).
- **Impersonation was not driven end to end.** It is covered at the repository and handler level.
- **NetworkPolicy is declared, not enforced:** kind's CNI (kindnet) does not enforce it.
- **The run surface** (`/r/…`) and any run pod are P5.
- **The positive controls are the server logs and the earlier 5-test run's failure, not a re-mutation of
  the deployed image**: removing a defence from the deployed backend would need a 30-minute rebuild. The
  defences were each broken on purpose in their own PRs (#618, #619); this run shows them present and
  effective together.
- One run of five, on one machine, with the login rate limit (5 per 5 minutes) forcing the file to sign in
  once and reuse the session.

## Operating notes

- The live `egret` release was not touched: separate release, namespaces (`egret-verify`,
  `egret-verify-apps`, `egret-verify-engines`), image tags and nodePort (30081). `just undeploy-kind-apps`
  removes it.
- `helm upgrade` with an unchanged image tag rolls nothing; `deploy-kind-apps` restarts the Deployments.
  Check pod age.
- The first Pod start took 4 s with the image already on the node.
