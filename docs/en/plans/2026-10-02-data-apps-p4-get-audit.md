# Data Apps P4 — audit of egret's `GET` routes

Spec §6.1 (P4 requirement): egret's `GET` routes must have no side effects, because the
apps origin is same-site with egret and S0 measured the viewer's `SameSite=Lax` cookies
riding requests that author JavaScript makes. This is the evidence for that requirement.
The enforcement is the `Sec-Fetch-Site` layer (`egret_auth::fetch_meta`, mounted outermost
in `build_router`); this audit is what says the layer is not the only thing standing.

## Method

`scripts/audit-get-routes.py` (2026-10-02, on the P4b branch):

1. finds every `.route("<path>", … get(<handler>) …)` registration in `crates/*/src`;
2. locates `async fn <handler>` in the same file and takes its body;
3. flags the body if it contains a write signal: `INSERT INTO`, `UPDATE … SET`,
   `DELETE FROM`, `.execute(`, `.emit(`, `Command::new`, `tokio::spawn`, `.send(`,
   `.revoke`, `.mint(`, `.issue`, `.insert(`, `.delete(`, `.save`, `.trigger`, … ;
4. a person reads every flagged or unresolved handler.

Result: **150 `GET` registrations; 133 unflagged, 12 flagged, 5 whose handler is in
another file.** The signal list is deliberately loose (it flags a `HashMap::insert`), so a
flag is a reason to read, not a finding.

## What the 17 are

| Route | Verdict |
|---|---|
| `GET /api/kernels/{id}` (kernel gateway `get_one`) | false positive: the matches are `tokio::spawn` and `.delete(` in the module's **tests** and in the sibling `del_one` registration on the same line. |
| `GET /policies/{id}` (`get_policy`) | false positive: `Map::insert` building the JSON response. |
| `GET /ws/{ws}/sqllab/queries`, `…/{id}` | false positive: the `.saved` field name. |
| `GET /ws/{ws}/dashboards/{id}/revisions` | false positive: a `HashSet::insert` of a label. |
| `GET /ws/{ws}/alerts` | false positive: a `HashMap::insert` of an endpoint-read cache key. |
| `GET /storage/{id}/download` | false positive: `HeaderMap::insert`. |
| `GET /.well-known/openid-configuration` | read-only discovery document; `.issue` is the word "issuer". |
| `GET /ws/{ws}/notebook-runtime` (`runtime`) | reads and returns a manifest and mints short-lived JWTs **into the response**; nothing is persisted. The response is unreadable cross-origin (no CORS allowance). |
| `GET /ws/{ws}/notebooks/{id}/kernel/channels` | **WebSocket upgrade**: it opens a channel to the user's kernel, which can execute code as them. It is a `GET` with a side effect in the cross-site-WebSocket-hijacking sense. Closed by the `Sec-Fetch-Site: same-site` refusal (a browser sends it on the handshake too). The notebook feature is deleted in P6. |
| `GET /api/auth/oidc/login`, `GET /callback` | create the login state cookie and, at the callback, the session — inherent to OIDC and protected by the state cookie. `/callback` is the one path the same-site guard lets through, only as a top-level document navigation. |
| `GET /{ws}/executions/webhook/{namespace}/{flow}/{key}` | triggers a flow. Intentional (a webhook), authenticated by the key in the URL, **no cookie involved**, and off unless `workflow.webhooks_enabled`. The apps origin gains nothing from it that any internet client lacks. |
| Ranger `GET …/policies/download/{service}`, `…/tags/download/{service}` | reads, bearer or anonymous machine endpoints. |
| `GET /metrics` | read-only scrape, no cookie. |
| `GET /{ws}` (Data API `serve_contract`) | read-only contract document of a published endpoint; the serving surface authenticates with a bearer token. |

**No `GET` route with a state-changing effect that a cookie authenticates was found.**
The two that act on the user's behalf (`kernel/channels`, and OIDC `callback`) are covered
above and by the layer.

## Limits of this audit — what it does not cover

- It only sees `get(<ident>)` registrations. A closure handler, a handler registered through
  a helper, and `nest_service` mounts are not parsed. The one `nest_service` is the embedded
  Iceberg REST catalog at `/catalog/v1`: lakekeeper's own routes, bearer-authenticated, with no
  session cookie to ride.
- It reads handler bodies, not what they call. A `GET` that calls a function which writes
  would be flagged only if the write shows in the handler itself. Reading the handler of each
  flag settles the flagged ones; it cannot rule out an unflagged one.
- It is a snapshot. The test that would keep it true — "a route that changes state refuses
  `GET`" — cannot be written against axum's `Router`, which cannot be enumerated. What keeps it
  true is the layer: a request from the apps origin never reaches a handler, whatever the handler does.
