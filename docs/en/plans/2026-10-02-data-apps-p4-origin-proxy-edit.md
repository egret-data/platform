# Data Apps P4 — apps origin, proxy, edit mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An author opens an app in edit mode inside egret: a sandboxed iframe on a separate apps origin shows marimo, saves go to the draft in Postgres, queries run as the author through the gateway, and nothing the app's JavaScript does can reach egret as the viewer.

**Architecture:** egret gains a second listener (the apps host) that serves only a ticket-redeem route and a reverse proxy to marimo pods. A one-time ticket, issued on the egret origin, becomes an apps-origin cookie (a "grant"); the proxy turns the grant into a per-session query token and injects it. Pods are launched by a new `AppPodLauncher` (k8s and subprocess), call back to egret only through the pod API of spec §5.3 with a pod token, and the egret origin stops honouring requests marked `Sec-Fetch-Site: same-site`.

**Tech Stack:** Rust (axum 0.8, reqwest 0.12, tokio-tungstenite 0.30, kube 2, sqlx), Postgres, React/TypeScript (existing frontend), Python (`apps-image/`, small changes), Helm, kind, Playwright.

**Spec:** `docs/en/specs/2026-09-30-marimo-data-apps-design.md` §5.1, §5.3, §6.1–§6.3, §7, §9 (P4 entry), §10. S0 result: `docs/en/plans/2026-09-30-marimo-s0-result.md`.

## Scope and delivery

P4 is five PRs, in this order. Each ends with something testable on its own; later parts import earlier parts' interfaces, so do not reorder.

| PR | Part | Deliverable |
|---|---|---|
| P4a | Pod API, pod tokens, session hardening | The three §5.3 routes served; the P1 carry-overs closed. Backend only. |
| P4b | Grants, tickets, cookie names, Fetch Metadata guard | A grant can be issued and redeemed; egret's cookies renamed; same-site sub-requests refused. Backend plus a small FE edit. |
| P4c | The proxy | Path guard, route allow-list, header sanitising, HTTP and WS relay, token mint and revoke, second listener. Tested against a fake pod. |
| P4d | Edit-pod launcher | `AppPodLauncher`, k8s pod spec, subprocess launcher for dev, NetworkPolicy, image carry-overs. |
| P4e | Frontend, deploy, live verification | Apps list, Edit page, ingress host and TLS on kind, the browser tests of spec §10. |

## Global Constraints

- Origins: egret on `<host>`, marimo on `apps.<host>` (sibling subdomain, D12). Locally `egret.localtest.me` and `apps.egret.localtest.me` — never a different port on the same host, cookies are not port-isolated.
- Tickets: single use, TTL 30 s, bound to user, app, mode, version-or-draft.
- App-session tokens: prefix `ega_`, idle 30 min, absolute 8 h (`AppsConfig`), only the SHA-256 stored.
- Pod API routes are exactly those of spec §5.3, under `/api/apps/pod/`, bearer pod token, exempt from CSRF.
- The proxy checks and forwards **the same raw path**; it rejects `.` / `..` segments (also after one percent-decode), `//`, encoded `.` `/` `\` `%`, NUL and backslash.
- Any client `X-Egret-*` header is removed before forwarding. `Marimo-Server-Token` is passed through.
- CSRF stays header-based; egret adds no CORS allowance for the apps origin.
- Pod hardening carries over: non-root, drop ALL, seccomp RuntimeDefault. NetworkPolicy: ingress only from egret, egress only to egret and DNS.
- Rust: `cargo fmt`, clippy clean on the files touched (main carries clippy debt; do not add to it). Migrations are edited in place only before merge. Commits are Conventional Commits and never carry `Closes #`.
- Every security test is shown to fail when the code it guards is broken on purpose (spec §10). Each such task has a "break it" step.

## Decisions this plan makes (the spec left them open)

Recorded in the spec by Task A0. Items marked **ask** need the user before P4b/P4c start.

| # | Decision | Why |
|---|---|---|
| D17 | The apps host is served by a **separate listener** (`apps.listen`, default `0.0.0.0:8081`), a router that mounts only `/_ticket` and the proxy. The egret API router is never reachable on it, and the proxy is never reachable on the egret listener. | Host-header routing inside one router makes one bug expose both. Separate ports make it structural. The ingress sends `apps.<host>` to port 8081 and `<host>` to 8080. |
| D18 | **A grant is a redeemed ticket** (one row, table `analytics.app_grants`): `ticket_hash` is spent, `cookie_hash` is set. The apps cookie holds the plaintext whose hash is `cookie_hash`. `login_sid` cascades from `sessions`. | One table, atomic single use (`UPDATE … WHERE redeemed_at IS NULL RETURNING`), logout kills grants by construction, works with several egret replicas. |
| D19 | **Query tokens are minted per grant**, lazily, by the proxy, and cached in memory by grant id. A cache miss (restart, other replica, revoked token) mints a new token. **WebSocket close** revokes the grant's tokens when that was the grant's last open WebSocket on this replica. | S0's proxy kept one token per cookie session; a reload then hit a revoked token. Lazy re-mint fixes that and keeps "WS closes → token dies". With several replicas the open-WS count is per replica, so idle expiry (30 min) is the backstop. **ask:** acceptable, or do you want a shared count in the DB? |
| D20 | **Fetch Metadata guard on the egret listener:** any request with `Sec-Fetch-Site: same-site` is refused with 403, unless its path is on a small navigation allow-list (`/api/auth/oidc/callback`). | S0 measured that the viewer's Lax cookies ride requests from the apps origin. Auditing every `GET` is necessary but not sufficient; this closes the whole class for every browser that sends the header (all current ones). Not in the spec; must be measured in P4e. |
| D21 | **Cookie names:** `__Host-egret_session` / `__Host-egret_csrf` when `auth.cookie_secure = true`; the current names when false (plain-http dev). The frontend reads both. | `__Host-` needs `Secure`; Chromium refuses it over http (S0). |
| D22 | **The edit pod gets a new trait, `AppPodLauncher`, in `egret-apps`.** `egret-kernel`'s `KernelLauncher` is not generalised. | P6 deletes `egret-kernel`'s Jupyter half. Generalising code that is about to be deleted is waste; the k8s hardening is copied, not shared. Overrides spec §6.3's "generalised". |
| D23 | **Grid layout is blocked in edit mode** (not stored). | Storing `layouts/*.grid.json` with the draft needs a schema change and a marimo-side hook that does not exist; blocking is one config/proxy line. Task D4 first measures which request writes it. Reversible later. **ask:** is losing the grid layout acceptable for v1? |

## File structure

New:
- `migrations/0165_data_apps_origin.sql` — `app_grants`, `app_pod_tokens`, `app_sessions` columns.
- `crates/egret-apps/src/podapi.rs` — the three pod routes.
- `crates/egret-apps/src/podtoken.rs` — `PodTokenRepo`.
- `crates/egret-apps/src/grant.rs` — `GrantRepo` (ticket → cookie).
- `crates/egret-apps/src/proxy/mod.rs` — `ProxyState`, router, listener entry.
- `crates/egret-apps/src/proxy/pathguard.rs` — raw-path check.
- `crates/egret-apps/src/proxy/policy.rs` — route allow-list, `route_inventory.json` loader.
- `crates/egret-apps/src/proxy/headers.rs` — request and response header rules.
- `crates/egret-apps/src/proxy/relay.rs` — HTTP and WebSocket relay.
- `crates/egret-apps/src/proxy/route_inventory.json` — the 107 marimo 0.25.0 routes and their classification.
- `crates/egret-apps/src/launcher.rs` — `AppPodLauncher` trait and `FakeAppPodLauncher`.
- `crates/egret-apps/src/fetch_meta.rs` — the D20 middleware.
- `crates/egret-bin/src/apps_pod_k8s.rs` — k8s implementation.
- `crates/egret-apps/src/subprocess_pod.rs` — dev implementation.
- `deploy/helm/egret/templates/networkpolicy-apps.yaml`, `ingress.yaml` (edit), `values.yaml` (edit).
- `frontend/src/routes/explore/Apps.tsx`, `AppEditPage.tsx`, `frontend/src/api/apps.ts` (hand-written wrapper over generated client).

Modified: `crates/egret-apps/src/{lib,session,routes,gateway}.rs`, `crates/egret-auth/src/{cookie,csrf,principal,routes,oidc_routes}.rs`, `crates/egret-config/src/lib.rs`, `crates/egret-server/src/lib.rs`, `crates/egret-bin/src/main.rs`, `frontend/src/api/{client,sqllab}.ts`, `frontend/src/App.tsx`, `frontend/src/components/layout/Sidebar.tsx`, `apps-image/egret_marimo/*`.

---

# Part P4a — pod API, pod tokens, session hardening

Branch `feat/data-apps-p4a-pod-api` off main. Worktree `egret-wt-apps-p4a`.

### Task A0: Record the decisions in the spec

**Files:**
- Modify: `docs/en/specs/2026-09-30-marimo-data-apps-design.md` (§6.1, §6.2, §6.3, §9 P4), `docs/vi/specs/2026-09-30-marimo-data-apps-design.md`

- [ ] **Step 1:** Add D17–D23 from the table above to spec §1 (decisions list), one line each, and edit the places they change: §6.3 "generalised" → "a new `AppPodLauncher`"; §6.2 gains "token minted per grant (D19)"; §6.1 gains the Fetch Metadata guard (D20) and the cookie-name rule (D21).
- [ ] **Step 2:** Mirror in the Vietnamese spec (docs convention: English canonical, Vietnamese reference).
- [ ] **Step 3:** Commit `docs: Data Apps spec — P4 decisions D17–D23`.

### Task A1: Migration 0165

**Files:**
- Create: `migrations/0165_data_apps_origin.sql`
- Test: `crates/egret-apps/tests/origin_schema.rs`

**Interfaces:**
- Produces: tables `analytics.app_grants`, `analytics.app_pod_tokens`; columns `analytics.app_sessions.grant_id uuid NULL`, `analytics.app_sessions.impersonator_user_id uuid NULL`.

- [ ] **Step 1: Write the failing test.** Use the pattern of `crates/egret-apps/tests/session_repo.rs` (read it first for the test-DB helper). Assertions: (a) inserting a grant row and then a second redeem `UPDATE … WHERE redeemed_at IS NULL` affects 0 rows; (b) deleting the parent `sessions` row deletes the grant and, through `grant_id`, leaves `app_sessions` rows revoked-or-deleted (see SQL: `ON DELETE CASCADE` on both); (c) a pod token with `kind='run'` and `version_id NULL` violates the CHECK.

```rust
#[sqlx::test]
async fn second_redeem_affects_no_row(pool: sqlx::PgPool) { /* seed user, ws, app, session; insert grant; run the redeem UPDATE twice; assert rows_affected == 1 then 0 */ }
```

- [ ] **Step 2:** Run `cargo test -p egret-apps --test origin_schema` — FAIL (table missing). Note: `#[sqlx::test]` writes into the dev DB (see memory `sqlx-test-writes-into-the-dev-database`); use `egret-test-db`'s per-process DB exactly as `session_repo.rs` does.
- [ ] **Step 3: Write the migration.**

```sql
-- Data Apps P4 (spec §6.1, §6.2): the apps-origin grant, pod tokens, and what an
-- app session needs so a token can be traced to a grant and an impersonator.

-- A grant is a ticket that has been (or may yet be) redeemed. The ticket is spent
-- by setting redeemed_at in ONE statement; the apps cookie then carries the
-- plaintext whose hash is cookie_hash. login_sid cascades from egret's login
-- session, so logout and back-channel logout kill grants without any code
-- remembering to.
CREATE TABLE analytics.app_grants (
    id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    ticket_hash        text NOT NULL UNIQUE,
    cookie_hash        text NULL UNIQUE,
    user_id            uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id       uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    app_id             uuid NOT NULL,
    version_id         uuid NULL,
    mode               text NOT NULL CHECK (mode IN ('edit', 'run')),
    login_sid          uuid NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    impersonator_user_id uuid NULL REFERENCES users(id) ON DELETE SET NULL,
    created_at         timestamptz NOT NULL DEFAULT now(),
    ticket_expires_at  timestamptz NOT NULL,
    redeemed_at        timestamptz NULL,
    cookie_expires_at  timestamptz NULL,
    revoked_at         timestamptz NULL,
    CHECK ((mode = 'edit') = (version_id IS NULL)),
    CHECK ((redeemed_at IS NULL) = (cookie_hash IS NULL)),
    CONSTRAINT app_grants_app_workspace_fk FOREIGN KEY (app_id, workspace_id)
        REFERENCES analytics.apps (id, workspace_id) ON DELETE CASCADE
);
CREATE INDEX app_grants_login_sid_idx ON analytics.app_grants (login_sid);
CREATE INDEX app_grants_user_id_idx ON analytics.app_grants (user_id);

-- Pod tokens (spec §5.3). An edit pod's token is bound to (user, workspace); a run
-- pod's to (app, version). Revoked when egret stops the pod.
CREATE TABLE analytics.app_pod_tokens (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash   text NOT NULL UNIQUE,
    kind         text NOT NULL CHECK (kind IN ('edit', 'run')),
    user_id      uuid NULL REFERENCES users(id) ON DELETE CASCADE,
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    app_id       uuid NULL,
    version_id   uuid NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    revoked_at   timestamptz NULL,
    CHECK (kind <> 'edit' OR (user_id IS NOT NULL AND app_id IS NULL AND version_id IS NULL)),
    CHECK (kind <> 'run'  OR (app_id IS NOT NULL AND version_id IS NOT NULL))
);
CREATE INDEX app_pod_tokens_workspace_idx ON analytics.app_pod_tokens (workspace_id);

ALTER TABLE analytics.app_sessions
    ADD COLUMN grant_id uuid NULL REFERENCES analytics.app_grants(id) ON DELETE CASCADE,
    ADD COLUMN impersonator_user_id uuid NULL REFERENCES users(id) ON DELETE SET NULL;
CREATE INDEX app_sessions_grant_id_idx ON analytics.app_sessions (grant_id);
```

- [ ] **Step 4:** Run the test — PASS. Run the whole `egret-apps` suite.
- [ ] **Step 5:** Commit `feat(apps): migration 0165 — grants, pod tokens, session grant and impersonator columns`.

### Task A2: `PodTokenRepo`

**Files:**
- Create: `crates/egret-apps/src/podtoken.rs`; modify `lib.rs` (`pub mod podtoken;`)
- Test: `crates/egret-apps/tests/podtoken_repo.rs`

**Interfaces:**
- Produces:

```rust
pub const POD_TOKEN_PREFIX: &str = "egp_";
pub enum PodKind { Edit, Run }
pub struct PodPrincipal { pub id: Uuid, pub kind: PodKind, pub user_id: Option<UserId>,
    pub workspace_id: WorkspaceId, pub app_id: Option<Uuid>, pub version_id: Option<Uuid> }
pub struct PodTokenRepo { /* Db */ }
impl PodTokenRepo {
    pub fn new(db: Db) -> Self;
    pub async fn mint_edit(&self, user: UserId, ws: WorkspaceId) -> Result<String>;          // plaintext, once
    pub async fn mint_run(&self, ws: WorkspaceId, app: Uuid, version: Uuid) -> Result<String>;
    pub async fn resolve(&self, plain: &str) -> Result<Option<PodPrincipal>>;                // None if revoked
    pub async fn revoke_for_pod(&self, id: Uuid) -> Result<()>;
}
```

- [ ] **Step 1:** Write tests: mint → resolve returns the principal; resolve of a wrong prefix, an unknown string and a revoked token returns `None`; `mint_run` with a version belonging to another app fails (FK-less, so assert in code via a `SELECT 1 FROM analytics.app_versions WHERE id=$1 AND app_id=$2`).
- [ ] **Step 2:** Run — FAIL.
- [ ] **Step 3:** Implement, copying the shape of `session.rs::mint/resolve` (random `token::random_token_b64()`, `token::hash_token`).
- [ ] **Step 4:** Run — PASS. **Break it:** make `resolve` ignore `revoked_at`; the revoked test must fail; restore.
- [ ] **Step 5:** Commit `feat(apps): pod tokens`.

### Task A3: The pod API routes

**Files:**
- Create: `crates/egret-apps/src/podapi.rs`
- Modify: `crates/egret-apps/src/lib.rs`, `crates/egret-server/src/lib.rs` (mount without CSRF layer, next to the gateway group, ~line 400), `crates/egret-bin/src/main.rs` (construct `PodTokenRepo` into `AppsState`)
- Test: `crates/egret-apps/tests/podapi.rs`

**Interfaces:**
- Consumes: `PodTokenRepo::resolve`, `AppStore::{draft, save_draft, version}` (check their exact signatures in `store.rs` — P2), the authorizer used by `routes.rs` for `app:write`.
- Produces: `pub fn pod_router() -> Router<AppsState>` with

| Route | Behaviour (spec §5.3) |
|---|---|
| `GET /apps/pod/apps/{app_id}/draft` | edit token only; re-check `app:write` for the token's user on that app every call; `{source, revision}` |
| `PUT /apps/pod/apps/{app_id}/draft` | `If-Match: "<rev>"`, body `{source}`; 200 `{revision}` · 409 `{error, current_revision}` · 413 · 428 |
| `GET /apps/pod/apps/{app_id}/versions/{version_id}` | run token whose `app_id`/`version_id` equal the path; `{source, version_no}` |

- [ ] **Step 1: Write failing tests** (axum `oneshot` against `pod_router()` with a seeded DB, like `tests/gateway.rs`). Cases, each its own test:
  1. edit token, user has `app:write` → 200 and the body matches the draft;
  2. edit token whose user lost `app:write` → 403 (re-check every call);
  3. edit token for an app in another workspace → 404;
  4. run token on the draft route → 403; edit token on the version route → 403;
  5. run token for version A on version B's path → 403;
  6. PUT without `If-Match` → 428; stale revision → 409 with `current_revision`; body > 1 MiB → 413; success increments the revision;
  7. no / garbage / revoked bearer → 401;
  8. trashed app → 409 (same as the cookie API).
- [ ] **Step 2:** Run `cargo test -p egret-apps --test podapi` — FAIL.
- [ ] **Step 3: Implement.** Reuse the draft/version logic and error mapping from `routes.rs` by calling the same `AppStore` methods; do not duplicate the 409/413/428 rules — extract the shared "save draft with If-Match" function from `routes.rs` into `store.rs` first if it is inline, and have both call it (a refactor commit of its own, tests unchanged).
- [ ] **Step 4:** Run — PASS, plus the existing `http.rs` and `lifecycle_store.rs` tests.
- [ ] **Step 5: Break it:** remove the `app:write` re-check; test 2 must fail. Restore.
- [ ] **Step 6:** Also check the Python side against the real routes: `apps-image/tests/test_podapi.py` runs against `fake_egret.py`; add one contract test that starts the Rust test server and runs `PodApi.get_draft/put_draft/get_version` against it **only if** the harness makes that cheap; otherwise leave a one-line note in the PR that the pair is covered by P4e's live run.
- [ ] **Step 7:** Commit `feat(apps): pod API — draft read/write and version read under a pod token`.

### Task A4: P1 session hardening carry-overs

**Files:**
- Modify: `crates/egret-apps/src/session.rs`, `crates/egret-apps/src/gateway.rs`, `crates/egret-bin/src/main.rs`
- Test: extend `crates/egret-apps/tests/session_repo.rs`, `tests/gateway.rs`

**Interfaces:**
- Produces (added to `AppSessionRepo`):

```rust
pub async fn revoke_by_id(&self, id: Uuid) -> Result<()>;
pub async fn revoke_for_login_session(&self, login_sid: Uuid) -> Result<u64>;
pub async fn revoke_for_grant(&self, grant_id: Uuid) -> Result<u64>;
pub async fn sweep_expired(&self, older_than: Duration) -> Result<u64>; // deletes rows expired or revoked longer ago than `older_than`
```
and `NewAppSession` gains `grant_id: Option<Uuid>` and `impersonator: Option<UserId>`.

- [ ] **Step 1: Tests (failing):** each new method; `mint` with `impersonator: Some(admin)` stores it and `resolve` returns it in `AppSession.impersonator`; the sweeper keeps live rows.
- [ ] **Step 2:** Run — FAIL.
- [ ] **Step 3: Implement** the four methods and the two fields. The gateway audit gains `impersonator` in the event payload (look at `audit()` in `gateway.rs`).
- [ ] **Step 4: Expired-but-undeleted login session** (P1 review question). Decide by reading `egret-auth/src/session.rs::validate`: it rejects `idle_expires_at <= now` but the row stays until a prune. Make `resolve` join `sessions` and require `s.idle_expires_at > now() AND s.absolute_expires_at > now()`. **Do not** slide the login session's idle window from a gateway call (the user is not browsing egret); the app token's own idle window is independent. Test: expire the login session row without deleting it → gateway 401.
- [ ] **Step 5: Boot check.** In `main.rs`, fail at startup when `apps.session_idle_secs > apps.session_absolute_secs` (the migration's CHECK already forbids it per row, so mint would 500 at runtime). Unit test on a small `AppsConfig::validate()`.
- [ ] **Step 6: Rate-limit audit once.** `gateway.rs` audits every 429. Audit only the call that *starts* the lockout: the limiter's `check` returns a bool; add `RateLimiter::check_first_denial` if the limiter cannot tell (read `egret-core/src/ratelimit.rs` first), or keep a per-token `denied_since` in `AppsState`. Test: 50 calls over the limit → exactly one `rate_limited` audit row.
- [ ] **Step 7: Sweeper task** in `main.rs` beside the other hourly tasks: `sweep_expired(24h)` for `app_sessions`, and the same for `app_grants` (ticket expired unredeemed, or cookie expired / revoked). Add `GrantRepo::sweep` in Task B1.
- [ ] **Step 8:** Run `cargo test -p egret-apps` and the workspace suite with `EGRET_TEST_SHARED_STORE_URL` set (see memory `egret-run-full-ci-local-before-merge`). Commit `feat(apps): session hardening — revoke by id/login/grant, sweeper, impersonator, expired login sessions`.

### Task A5: PR P4a

- [ ] `cargo fmt --all`, clippy on touched crates, `npm run gen:api` only if an annotated route changed (the pod routes are deliberately un-annotated, like the gateway).
- [ ] Open the PR; stop and wait for the user's merge approval (memory `egret-no-auto-merge-await-approval`).

---

# Part P4b — grants, tickets, cookie names, Fetch Metadata guard

Branch `feat/data-apps-p4b-grants` off main after P4a merges.

### Task B1: `GrantRepo`

**Files:**
- Create: `crates/egret-apps/src/grant.rs`
- Test: `crates/egret-apps/tests/grant_repo.rs`

**Interfaces:**
- Produces:

```rust
pub const TICKET_PREFIX: &str = "egt_";
pub const COOKIE_PREFIX: &str = "egc_";
pub struct NewGrant { pub user_id: UserId, pub workspace_id: WorkspaceId, pub app_id: Uuid,
    pub version_id: Option<Uuid>, pub mode: AppMode, pub login_sid: Uuid, pub impersonator: Option<UserId> }
pub struct Grant { pub id: Uuid, pub user_id: UserId, pub workspace_id: WorkspaceId, pub app_id: Uuid,
    pub version_id: Option<Uuid>, pub mode: AppMode, pub login_sid: Uuid, pub impersonator: Option<UserId> }
impl GrantRepo {
    pub fn new(db: Db) -> Self;
    pub async fn issue_ticket(&self, new: &NewGrant, ttl: Duration) -> Result<String>;   // plaintext ticket
    pub async fn redeem(&self, ticket: &str, cookie_ttl: Duration) -> Result<Option<(Grant, String)>>; // (grant, cookie plaintext)
    pub async fn resolve_cookie(&self, cookie: &str) -> Result<Option<Grant>>;
    pub async fn revoke(&self, grant_id: Uuid) -> Result<()>;
    pub async fn sweep(&self, older_than: Duration) -> Result<u64>;
}
```

- [ ] **Step 1: Tests (failing):**
  1. redeem succeeds once; the second redeem of the same ticket returns `None` (replay);
  2. a ticket past `ticket_expires_at` → `None` (use a 0-second ttl and a short sleep, or set the column directly — see memory `time-dependent-test-assertions`: give a gap);
  3. `resolve_cookie` works after redeem and returns `None` after `revoke`, after the cookie expires, and after the parent login session is deleted;
  4. a ticket string cannot be used as a cookie and vice versa (prefixes differ and the hashes are in different columns);
  5. 20 concurrent `redeem` calls of one ticket → exactly one `Some`.
- [ ] **Step 2:** Run — FAIL.
- [ ] **Step 3: Implement.** The redeem statement is the whole security property:

```sql
UPDATE analytics.app_grants
   SET cookie_hash = $2, redeemed_at = now(),
       cookie_expires_at = now() + make_interval(secs => $3)
 WHERE ticket_hash = $1
   AND redeemed_at IS NULL
   AND revoked_at IS NULL
   AND ticket_expires_at > now()
RETURNING id, user_id, workspace_id, app_id, version_id, mode, login_sid, impersonator_user_id
```

`resolve_cookie` joins `sessions` and requires the login session to be unexpired (same rule as Task A4 step 4) and the user `active`.

- [ ] **Step 4:** Run — PASS. **Break it:** drop `AND redeemed_at IS NULL`; tests 1 and 5 must fail. Restore.
- [ ] **Step 5:** Commit `feat(apps): grants — one-time tickets that become apps-origin cookies`.

### Task B2: Ticket endpoint on the egret origin

**Files:**
- Modify: `crates/egret-apps/src/routes.rs`, `crates/egret-apps/src/lib.rs` (`AppsApiState` gains `grants: GrantRepo` and the `apps.origin` string), `crates/egret-config/src/lib.rs`
- Test: `crates/egret-apps/tests/http.rs`

**Interfaces:**
- Config: `AppsConfig { origin: String /* e.g. "https://apps.egret.localtest.me" */, ticket_ttl_secs: u64 = 30, cookie_ttl_secs: u64 = 28_800, listen: String = "0.0.0.0:8081" }`. `origin` empty = apps feature serves no iframe (the route answers 503 "apps origin not configured").
- Produces: `POST /api/ws/{workspace}/apps/{id}/ticket` body `{"mode":"edit"|"run","version_id":<uuid|null>}` → `{"url":"<origin>/_ticket?t=<ticket>"}`. CSRF-protected (cookie API). Run mode: `app:read` and, when `version_id` is null, the latest published version (409 if none). Edit mode: `app:write` **and** `query:run` (spec §4.2).

- [ ] **Step 1: Tests (failing):** viewer with `app:read` gets a run ticket and no edit ticket (403); author gets both; run ticket for an unpublished app → 409; for a trashed app → 409; the ticket is bound to the caller's login session id and, under impersonation, carries the impersonator; the URL host equals `apps.origin`.
- [ ] **Step 2:** Run — FAIL.
- [ ] **Step 3: Implement.** Take `login_sid` and `impersonator` from the `Principal` (read `egret-core/src/context.rs` for the exact field names; `Principal` has `impersonator: Option<Impersonator>`; the session id comes from the session extractor — check how `routes.rs` or the kernel routes obtain it, and add it to `Principal` only if absent, in a separate commit).
- [ ] **Step 4:** Run — PASS. Add the route to `AppsApi` (utoipa) and run `npm run gen:api` (never bare `orval`, memory `orval-regeneration-strips-url-encoding`).
- [ ] **Step 5:** Commit `feat(apps): ticket endpoint for the apps iframe`.

### Task B3: Cookie names (D21)

**Files:**
- Modify: `crates/egret-auth/src/cookie.rs`, `csrf.rs`, `principal.rs`, `routes.rs`, `oidc_routes.rs`; `frontend/src/api/client.ts`, `frontend/src/api/sqllab.ts`
- Test: the files' own `#[cfg(test)]` modules; `frontend/src/api/__tests__/client.test.ts`

**Interfaces:**
- Produces: `pub fn session_cookie_name(secure: bool) -> &'static str`, `pub fn csrf_cookie_name(secure: bool) -> &'static str`; the extractors and `csrf_layer` read **both** names (secure deployments accept only the `__Host-` names — an old plain-name cookie must not authenticate once secure mode is on, or a tossed plain cookie would win).

- [ ] **Step 1: Tests (failing).** Rust: `secure=true` builds `__Host-egret_session` with `Path=/`, `Secure`, **no** `Domain`; a request carrying only the plain `egret_session` is rejected when the layer is in secure mode; `csrf_layer` in secure mode ignores a plain `egret_csrf`. FE: `readCookie` helper returns the `__Host-` value when both are present.
- [ ] **Step 2:** Run both — FAIL.
- [ ] **Step 3: Implement.** The extractors need to know the mode: add it to `SessionService` (it already carries TTLs), and give `csrf_layer` a state via `from_fn_with_state` — update the seven mount sites in `egret-server/src/lib.rs` mechanically (one commit, no behaviour change when `secure=false`). FE: `readCookie("__Host-egret_csrf") ?? readCookie("egret_csrf")` in both `client.ts` and `sqllab.ts`.
- [ ] **Step 4:** Run — PASS, plus `crates/egret-auth` suite and `npm test -- client`.
- [ ] **Step 5:** Commit `feat(auth): __Host- cookie names when cookies are Secure`. Note in the PR body that the **defence is only proven in P4e** (needs HTTPS).

### Task B4: Fetch Metadata guard (D20)

**Files:**
- Create: `crates/egret-apps/src/fetch_meta.rs` (it lives with apps because the reason does; it is mounted on the egret router)
- Modify: `crates/egret-server/src/lib.rs` (outermost layer on the egret router)
- Test: unit tests in the module

**Interfaces:**
- Produces: `pub async fn refuse_same_site(request: Request, next: Next) -> Response`.

```rust
const NAVIGATION_ALLOW: &[&str] = &["/api/auth/oidc/callback"];

pub async fn refuse_same_site(request: Request, next: Next) -> Response {
    let same_site = request
        .headers()
        .get("sec-fetch-site")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("same-site"));
    if same_site && !NAVIGATION_ALLOW.contains(&request.uri().path()) {
        return (StatusCode::FORBIDDEN, "same-site requests are not accepted").into_response();
    }
    next.run(request).await
}
```

- [ ] **Step 1: Tests (failing):** `same-site` → 403; `same-origin`, `cross-site`, `none` and a missing header → pass; `same-site` to the allow-listed callback → pass (check what the OIDC redirect actually sends: it is a cross-site top-level navigation from Keycloak → `cross-site`; keep the allow-list only if P4e measures `same-site` there, otherwise delete it — YAGNI).
- [ ] **Step 2:** Run — FAIL. **Step 3:** implement, mount as the last `.layer(...)` on the egret router. **Step 4:** PASS.
- [ ] **Step 5: Audit.** Walk every `get(` route registered in `crates/*/src/**/routes.rs` and `egret-server/src/lib.rs` and write the result to `docs/en/plans/2026-10-02-data-apps-p4-get-audit.md`: route, handler, side effect (none / read-only / writes). Any `GET` with a side effect is fixed here (turned into a `POST` with the FE call updated) or listed with a reason. This is the spec's "audit that GET routes have no side effects"; the Fetch Metadata guard is the enforcement, the audit is the evidence.
- [ ] **Step 6:** Commit `feat(server): refuse Sec-Fetch-Site same-site requests on the egret origin` and `docs: P4 GET-route audit`.

### Task B5: PR P4b

- [ ] fmt, clippy on touched files, full workspace suite, `npm test` for `api/`, open the PR, stop for approval.

---

# Part P4c — the proxy

Branch `feat/data-apps-p4c-proxy` off main after P4b merges.

### Task C1: Raw path guard

**Files:**
- Create: `crates/egret-apps/src/proxy/pathguard.rs`, `crates/egret-apps/src/proxy/mod.rs` (declares the submodules)
- Test: unit tests in the module (the S0 matrix is the test)

**Interfaces:**
- Produces:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum PathRejection { Malformed, DotSegment, EncodedSeparator, DoubleSlash, Nul, Backslash }

/// `raw` is the request target's path exactly as the client sent it (no query).
/// Returns the percent-decoded, lower-cased form to MATCH against; the caller
/// forwards `raw` unchanged.
pub fn check_raw_path(raw: &str) -> Result<String, PathRejection>;
```

- [ ] **Step 1: Write the failing tests.** One test per rejection class plus a generated matrix equivalent to S0's 40 variants: for each blocked base (`/api/packages/list`, `/api/files/list_files`, `/terminal/ws`, `/api/secrets/keys`) and each prefix (`/e/x`, `/r/x`) build the forms `…/../…`, `…/%2e%2e/…`, `…/..%2f…`, `…/%2E%2E/…`, `…//…`, `…\…`, `…%5c…`, `…%00…`, `…/./…`, and assert every one is `Err`. Plus accept cases: `/e/pod/api/kernel/save`, `/r/v1/@file/12-a%20b.png`, `/e/pod/assets/index-AbC.js`.

```rust
#[test]
fn rejects_every_dot_form() {
    for base in ["/api/packages/list", "/api/files/list_files", "/terminal/ws", "/api/secrets/keys"] {
        for prefix in ["/e/x", "/r/x"] {
            for form in [
                format!("{prefix}/../{}", &base[1..]),
                format!("{prefix}/%2e%2e/{}", &base[1..]),
                format!("{prefix}/%2E%2E/{}", &base[1..]),
                format!("{prefix}/..%2f{}", &base[1..]),
                format!("{prefix}/./{}", &base[1..]),
                format!("{prefix}//{}", &base[1..]),
                format!("{prefix}\\{}", &base[1..]),
                format!("{prefix}%5c{}", &base[1..]),
                format!("{prefix}%00{base}"),
            ] {
                assert!(check_raw_path(&form).is_err(), "{form}");
            }
        }
    }
}
```

- [ ] **Step 2:** `cargo test -p egret-apps proxy::pathguard` — FAIL (not defined).
- [ ] **Step 3: Implement.** Order: reject `\\` and NUL; reject any `%` not followed by two hex digits; reject any `%2e %2f %5c %25 %00` (case-insensitive); reject `//`; split on `/`, reject a segment that is `.` or `..`; then percent-decode and lower-case and return it. Decoding happens only after the encoded separators are rejected, so one decode level is all there is.
- [ ] **Step 4:** PASS. **Break it:** delete the `%2e` check; the `%2e%2e` cases must fail. Restore.
- [ ] **Step 5:** Commit `feat(apps): proxy raw-path guard`.

### Task C2: Route policy (allow-list)

**Files:**
- Create: `crates/egret-apps/src/proxy/policy.rs`, `crates/egret-apps/src/proxy/route_inventory.json`, `apps-image/tests/e2e/record_routes.py`
- Test: unit tests in the module

**Interfaces:**
- Produces:

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Surface { Edit, Run }
#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Verdict { Allow, Deny, AllowStatic }
/// `rel` is the matched form from `check_raw_path` with the `/e/{pod}` or `/r/{version}` prefix removed.
pub fn classify(surface: Surface, method: &http::Method, rel: &str, websocket: bool) -> Verdict;
```

- [ ] **Step 1: Record the traffic.** Using the P3 e2e harness (`apps-image/tests/e2e/harness.py`, Playwright), write `record_routes.py` that opens an app in edit mode and in run mode, performs the editor's normal actions (open, edit a cell, run, save, reload, change a UI element) and writes every `(method, path template)` the browser sent to `route_inventory_traffic.json`. Commit the raw output next to the S0 inventory.
- [ ] **Step 2:** Write `route_inventory.json`: the 107 routes of marimo 0.25.0 (from S0's `q6-routes.json`, copied from `spikes/marimo-s0/results/` of branch `spike/marimo-s0`), each with `surface: ["edit","run"]`, `edit`, `run` or `deny`, and a one-line reason. The first classification, to be corrected by Step 1's traffic:
  - **edit and run:** `GET /`, `GET /@file/*`, `GET /public/*` (header pinned, see C3), `GET /public-files-sw.js`, `GET /health`, `GET /healthz`, `GET /api/version`, WS `/ws`, static `GET /{path}` under `/assets/*` and the other static prefixes the traffic shows;
  - **edit only:** `POST /api/kernel/{code_autocomplete,delete,execute,focus_cell,format,function_call,instantiate,interrupt,pdb/breakpoints,pdb/pm,read_code,restart_session,run,save,save_app_config,scratchpad/run,set_cell_config,set_model_value,set_ui_element_value,shutdown,stdin,takeover}`, `GET /api/kernel/status`, `POST /api/document/transaction`, `GET /api/documentation/snippets`, `POST /api/sql/validate`;
  - **run only:** the subset of the kernel routes the run page sends (from the traffic);
  - **deny, both:** the 22 prefixes of spec §6.2 plus `/api/kernel/copy`, `/api/kernel/rename`, `/api/kernel/install_missing_packages`, `/api/kernel/save_user_config`, `/api/export/*`, `/api/home/*`, `/api/storage`, `/api/datasources`, `/api/lsp`, `/api/ai`, `/terminal`, `/auth`, `/og`, WS `/ws_sync`, `GET /sse` unless the traffic shows it.
- [ ] **Step 3: Write the failing tests:**
  1. `every_inventory_route_is_classified` — loads the JSON, and for every route and both surfaces `classify` returns exactly the recorded verdict;
  2. `unknown_route_is_denied` — `/api/brand/new` → `Deny` (an upgrade that adds a route fails closed);
  3. the 22 prefixes are `Deny` under `/e` and `/r`, GET and POST and WebSocket;
  4. `GET /assets/x.js` → `AllowStatic`, and `POST /assets/x.js` → `Deny`.
- [ ] **Step 4:** Run — FAIL. **Step 5:** Implement `classify` as a table lookup on the matched path (patterns with `{}` and `*`), default `Deny`. **Step 6:** PASS.
- [ ] **Step 7: Break it:** change one denied prefix to `Allow`; test 3 fails. Restore.
- [ ] **Step 8:** Add `docs` note: "re-run `record_routes.py` and `every_inventory_route_is_classified` on every marimo upgrade" to `apps-image/README` (or the P3 contract-test list). Commit `feat(apps): proxy route allow-list from the recorded marimo inventory`.

### Task C3: Header rules and the `file` pin

**Files:**
- Create: `crates/egret-apps/src/proxy/headers.rs`
- Test: unit tests in the module

**Interfaces:**
- Produces:

```rust
pub struct Inject<'a> { pub app_token: &'a str, pub marimo_token: &'a str, pub pinned_notebook: Option<&'a str> }
/// Builds the header map to send upstream from the client's.
pub fn upstream_headers(client: &HeaderMap, inject: &Inject) -> HeaderMap;
/// The notebook file an edit request names, if any: `?file=` or a `file` WS query parameter.
pub fn named_file(query: Option<&str>) -> Option<String>;
/// Parses `<uuid>.py` strictly (fullmatch; a trailing newline is not accepted).
pub fn app_id_of_file(file: &str) -> Option<Uuid>;
```

- [ ] **Step 1: Tests (failing):** every `X-Egret-*` header (any case, repeated) is gone and replaced by exactly one `X-Egret-App-Token`; hop-by-hop headers (`Connection`, `Upgrade` on non-WS, `Proxy-*`, `TE`, `Trailer`, `Transfer-Encoding`, `Keep-Alive`) stripped; `Cookie` (the apps cookie) is **not** forwarded to the pod; `X-Notebook-Id` is overwritten with the pinned value or removed; `Marimo-Server-Token` and `Marimo-Session-Id` are passed through; `app_id_of_file("<uuid>.py\n")` is `None`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** PASS. **Step 5: Break it:** stop removing `X-Egret-*`; the client-supplied-token test (spec §10, first bullet) must fail. Restore.
- [ ] **Step 6:** Commit `feat(apps): proxy header rules`.

### Task C4: Relay, token minting, revocation

**Files:**
- Create: `crates/egret-apps/src/proxy/relay.rs`; extend `proxy/mod.rs` with `ProxyState`, `proxy_router(state) -> Router`, and `ticket_router`.
- Test: `crates/egret-apps/tests/proxy.rs` (a fake pod: a small axum app that echoes the headers it received and offers `/ws`)

**Interfaces:**
- Consumes: `GrantRepo::{redeem, resolve_cookie}`, `AppSessionRepo::{mint, revoke_for_grant}`, `AppPodLauncher::endpoint` (Task D1; here a trait object `PodResolver` is injected so this task needs only a fake):

```rust
#[async_trait] pub trait PodResolver: Send + Sync {
    /// Edit: the caller's own pod for (user, workspace), starting it if needed.
    async fn edit_pod(&self, user: UserId, ws: WorkspaceId) -> Result<PodEndpoint>;
    /// Run: the pod serving this version (P5; here returns `NotFound`).
    async fn run_pod(&self, app: Uuid, version: Uuid) -> Result<PodEndpoint>;
}
pub struct PodEndpoint { pub base: url::Url, pub marimo_token: String }
```
- Produces: routes on the apps listener — `GET /_ticket?t=…` (redeem, then `Set-Cookie: egret_apps_grant=<plaintext>; HttpOnly; SameSite=Lax; Path=/`, plus `Secure` when `apps.origin` is https — it is the apps origin's own cookie, so it does not use the `__Host-` rename of Task B3 — then `302` to `/e/{pod_ref}/?file=<app_id>.py`), `ANY /e/{pod_ref}/{*rest}` and `/r/{version}/{*rest}`, with WebSocket upgrade.

- [ ] **Step 1: Write the failing tests** (spec §10, each its own test):
  1. valid ticket → 302 + cookie; replay → 403; no cookie on a proxied path → 401;
  2. a **run** grant's cookie on `/e/…` → 401 (S0: "a run session's cookie → 401 on the edit upstream");
  3. the fake pod receives exactly one `X-Egret-App-Token`, never the client's value, and never the apps cookie;
  4. edit request with `?file=<other app>.py` where the user lacks `app:write` → 403 and the pod sees nothing;
  5. `/e/<someone else's pod ref>/…` → 403: the pod reference is derived from the grant (user, workspace), a request whose `{pod_ref}` differs is refused;
  6. a blocked route (`/api/packages/list`) → 403 under `/e` and `/r`, HTTP **and** WS (`/terminal/ws`);
  7. a dot-segment variant is 403/400 and the fake pod **sees no request at all**;
  8. WS: the first WS for a grant mints a token; the same grant's HTTP request carries the same token; closing the only WS revokes it (the gateway rejects it afterwards); a second request after revocation re-mints (D19, the reload case) and works;
  9. logout (delete the login session row) → the next proxied request is 401 and the token is dead;
  10. response headers: `Set-Cookie` from the pod is dropped, `Access-Control-Allow-*` is dropped, and `Content-Security-Policy: frame-ancestors <egret origin>` is added on every response (only egret may frame the apps origin).
- [ ] **Step 2:** `cargo test -p egret-apps --test proxy` — FAIL.
- [ ] **Step 3: Implement.**
  - Take the request target with `request.uri().path()` as the **raw** path only if the server preserves it: verify axum/hyper give the unnormalised path (S0's lesson). Add a test that sends `/e/x/%2e%2e/api/packages/list` through a real `TcpListener`, not `oneshot`, so hyper's parsing is the one exercised.
  - HTTP relay: `reqwest` with `redirect(Policy::none())`, streaming request and response bodies, per-request timeout from config; build the upstream URL as `pod.base + "/e/{pod_ref}" + raw_rest + "?" + query`, i.e. the **same string** the guard accepted.
  - WS relay: accept with `axum::extract::ws`, connect with `tokio_tungstenite::connect_async` passing the sanitised headers (the same pattern as `egret-kernel/src/proxy.rs::run_proxy`), pump both ways; on close, decrement the grant's open-WS counter and revoke at zero (D19).
  - Token cache: `DashMap<Uuid /*grant*/, String>`; miss → `AppSessionRepo::mint` with `grant_id` and the grant's impersonator.
- [ ] **Step 4:** PASS. **Step 5: Break it** three times, once per property: forward the client's `X-Egret-App-Token` (test 3 fails); skip the grant-binding check on `{pod_ref}` (test 5 fails); skip revoke on WS close (test 8 fails). Restore each.
- [ ] **Step 6:** Commit `feat(apps): the apps proxy — ticket redeem, HTTP and WebSocket relay, per-grant query tokens`.

### Task C5: The second listener

**Files:**
- Modify: `crates/egret-bin/src/main.rs`, `crates/egret-server/src/lib.rs` (a `serve_apps(listener, router)` helper), `crates/egret-config/src/lib.rs`
- Test: `crates/egret-apps/tests/proxy.rs` (extend: the real two-listener wiring)

- [ ] **Step 1: Test (failing):** build the two routers as `main.rs` does; the egret router answers `404` for `/e/x/` and `/_ticket`; the apps router answers `404` for `/api/auth/me` and `/api/ws/…` (D17: structurally separate).
- [ ] **Step 2:** FAIL. **Step 3:** wire `apps.listen`; start the apps listener only when `apps.origin` is set; share the `PodResolver`, `GrantRepo`, `AppSessionRepo`. Graceful shutdown on the same signal as the main listener.
- [ ] **Step 4:** PASS; commit `feat(server): serve the apps host on its own listener`.

### Task C6: PR P4c

- [ ] fmt, clippy on touched files, full suite, open the PR, stop for approval.

---

# Part P4d — the edit-pod launcher

Branch `feat/data-apps-p4d-launcher` off main after P4c merges (it can start in parallel after P4a since it needs only the pod tokens; it must merge after P4c because `main.rs` wiring touches the same lines).

### Task D1: `AppPodLauncher`

**Files:**
- Create: `crates/egret-apps/src/launcher.rs`
- Modify: `crates/egret-apps/src/lib.rs`
- Test: unit tests in the module

**Interfaces:**
- Produces:

```rust
pub struct EditPodKey { pub user_id: UserId, pub workspace_id: WorkspaceId }
pub enum PodPhase { Spawning, Ready, Dead { reason: String } }
pub struct PodHandle { pub base: url::Url, pub marimo_token: String, pub phase: PodPhase }
#[async_trait]
pub trait AppPodLauncher: Send + Sync {
    /// Start (or return) the author's edit pod; block until ready or `timeout`.
    async fn ensure_edit(&self, key: EditPodKey, timeout: Duration) -> Result<PodHandle>;
    async fn status_edit(&self, key: EditPodKey) -> Result<PodPhase>;
    async fn stop_edit(&self, key: EditPodKey) -> Result<()>;
    /// Pods this launcher has not seen used since `idle` (reaper input).
    async fn idle_edit_pods(&self, idle: Duration) -> Result<Vec<EditPodKey>>;
    fn mode(&self) -> &'static str;
}
pub struct FakeAppPodLauncher { /* records calls, serves a configured endpoint */ }
```

The launcher mints the pod token (`PodTokenRepo::mint_edit`) and the marimo token (`token::random_token_b64()`) itself when it creates a pod; `stop_edit` revokes the pod token. The marimo token is returned to the proxy and held in memory; it is **not** stored in the database, so a restarted egret asks the launcher, which reads it back from the pod's Secret (k8s) or process table (subprocess).

- [ ] **Step 1: Tests (failing)** against `FakeAppPodLauncher`: `ensure_edit` twice for one key creates one pod; `stop_edit` revokes the pod token (`PodTokenRepo::resolve` → `None`); two keys → two pods.
- [ ] **Step 2:** FAIL. **Step 3:** implement trait, fake, and the `PodResolver` adapter used by the proxy (`edit_pod` = `ensure_edit` + endpoint). **Step 4:** PASS. **Step 5:** commit `feat(apps): AppPodLauncher trait and fake`.

### Task D2: The Kubernetes launcher

**Files:**
- Create: `crates/egret-bin/src/apps_pod_k8s.rs`; modify `crates/egret-bin/src/main.rs`, `crates/egret-config/src/lib.rs` (`AppsConfig.pod: AppPodConfig { namespace, image, image_pull_policy, cpu_request, mem_request, cpu_limit, mem_limit, pull_secret, callback_url, ready_timeout_secs, idle_secs }`, defaults 250m/512Mi → 1000m/2Gi as for kernels)
- Test: unit tests of the pure `build_pod`/`build_secret` functions (like `kernel_launcher_k8s::build_pod_spec`)

**Interfaces:**
- Produces: `pub fn edit_pod_name(key: &EditPodKey) -> String` (`app-edit-<12 hex>` from sha1), `pub fn build_edit_pod(key, cfg, secret_name) -> Pod`, `pub fn build_edit_secret(key, cfg, pod_token, marimo_token) -> Secret`, `pub struct KubernetesAppPodLauncher`.

- [ ] **Step 1: Tests (failing)** on the pure builders:
  1. the pod runs as non-root, drops ALL, has `seccompProfile: RuntimeDefault`, `automountServiceAccountToken: false`, `readOnlyRootFilesystem: true` with `emptyDir` mounts at `/work` and `/tmp` (`/home/egret` too: marimo writes there);
  2. **the tokens are not in `env.value`**: `EGRET_POD_TOKEN` and `EGRET_MARIMO_TOKEN` come from `secretKeyRef` (P3 carry-over); `EGRET_URL`, `EGRET_BASE_URL=/e/<podref>`, `APPS_WORKDIR=/work` are plain values;
  3. PID 1 is `tini` (`command: ["/usr/bin/tini","--"]`, args the image entrypoint) — add `tini` to the image in D3;
  4. labels `app=egret-apps`, `managed-by=egret`, `egret-user`/`egret-workspace` short ids, and the pod name is deterministic;
  5. `restartPolicy: Never` (egret acts on exit codes, Step 3).
- [ ] **Step 2:** FAIL. **Step 3:** implement the builders and the launcher: create the Secret, then the Pod with an `ownerReference` to the Secret's absence — i.e. set the pod as owner of the Secret after creation so deleting the pod garbage-collects it; wait for the pod IP and a TCP-ready probe on 2718; read the container's terminated state and act on the P3 exit codes: **64** → `Dead("bad environment")`, no restart; **65** → delete the pod and recreate with a fresh `emptyDir` once, then `Dead`; **69** → retry with backoff (3 tries) for the version-fetch case (run pods, P5; the code path exists but only edit pods are launched in P4, so test the mapping function, not a live case).
- [ ] **Step 4:** PASS (`cargo test -p egret-bin apps_pod_k8s`). **Step 5:** commit `feat(apps): Kubernetes edit-pod launcher`.

### Task D3: Image carry-overs (Python, `apps-image/`)

**Files:**
- Modify: `apps-image/Dockerfile`, `apps-image/egret_marimo/{launcher,patches,client}.py`, `apps-image/config/marimo/marimo.toml`, `apps-image/tests/*`, `crates/egret-apps/src/gateway.rs` (+ a route)
- Test: `apps-image/tests/test_launcher.py`, `test_client.py`, `test_marimo_contract.py`

- [ ] **Step 1: tini.** Install it in the image (`apt-get install --no-install-recommends tini`, pin the Debian version in the Dockerfile comment) and check `smoke.sh` still passes.
- [ ] **Step 2: Kernel runs as a different uid than the server** (P3 review: a cell must not read the server's tokens). Measure first: write a failing pytest that, inside a kernel, tries to read `/proc/<server pid>/environ`, `/proc/<server pid>/mem` and `/proc/<server pid>/cmdline`'s fd 3 pipe; expected today: succeeds (same uid). Then choose, in this order, what the measurement allows: (a) the IPC launcher shim `setuid`s the kernel to uid 1001 (needs `CAP_SETUID` at the shim only, which conflicts with drop-ALL — if so, do not take this route); (b) `prctl(PR_SET_DUMPABLE, 0)` in the server process right after reading the secrets pipe (the P3 stopgap) plus `hidepid`-style isolation via the pod's `shareProcessNamespace: false` (default). Record which was chosen and what the test shows. If neither holds, say so in the PR and leave the test `xfail(strict=True)` with the reason.
- [ ] **Step 3: Lock the remaining config keys.** P3 locks only `ai` and `rtc_v2`. In `patches.install_config_lock` also lock the PEP 723 / package-management / runtime keys marimo 0.25.0 honours (read `marimo/_config/config.py` for the exact keys: `package_management`, `runtime.auto_instantiate`, `runtime.on_cell_change`, `runtime.pythonpath`, `runtime.dotenv`, `experimental`). Test: `test_marimo_contract.py` asserts each is the locked value after a `PATCH`-style user-config write.
- [ ] **Step 4: `egret.databases()`.** Add `GET /api/apps/databases` to `gateway_router()` (bearer app-session token, same authorization as the query route, returns `{"databases":[{"name","engine"}]}` for databases of the token's workspace the user may query); Python `egret.databases()` calls it. Rust test beside `tests/gateway.rs`; Python test against `fake_egret.py` extended with the route.
- [ ] **Step 5: Reload while the old WebSocket is connected needs a second reload** (P3 note). Reproduce in the e2e harness; the cause is the old session still being "live" for 120 s (spec §5.3). Fix at the proxy if possible: on a new `/ws` for the same grant and same `file`, close the grant's previous WS before relaying. If that cannot be done cleanly, document it as known behaviour in the PR and the spec; do not paper over it.
- [ ] **Step 6: Grid layout (D23).** Run the editor with the recorder (Task C2 step 1), switch a cell layout to grid, save, and read which request wrote `layouts/*.grid.json`. If it is `POST /api/kernel/save` with a `layout` field, the proxy does not inspect bodies, so the block has to be in marimo's locked config (or the image's patch): find the setting that removes the grid option and verify the option is absent or inert. Record the finding and the chosen block in the spec's P4 carry-over line.
- [ ] **Step 7:** Run `pytest apps-image/tests` (the e2e group needs Playwright; run it, do not skip it silently — memory `egret-db-gated-tests-silently-skip`). Commit per step, `feat(apps-image): …`.

### Task D4: Subprocess launcher (dev, no k8s)

**Files:**
- Create: `crates/egret-apps/src/subprocess_pod.rs`
- Test: unit test with a stub command

- [ ] **Step 1: Test (failing):** with `apps.pod.command = ["python","-c","…listen on a port…"]` the launcher starts a process, waits for the port, returns a handle; `stop_edit` kills it; two keys use two ports.
- [ ] **Step 2–4:** implement (`tokio::process::Command`, env passed via the environment since there is no Secret; free port by bind-and-release), PASS. Commit `feat(apps): subprocess pod launcher for local dev`.

### Task D5: Reaper, NetworkPolicy, Helm

**Files:**
- Modify: `crates/egret-bin/src/main.rs` (idle reaper tick calling `idle_edit_pods` → `stop_edit`), `deploy/helm/egret/templates/networkpolicy-apps.yaml` (create), `deploy/helm/egret/values.yaml`, `deploy/helm/egret/templates/configmap-env.yaml`, `deploy/helm/egret/templates/rbac.yaml` (pod + secret create/delete in the apps namespace)
- Test: `helm template` rendering test (see how `networkpolicy-kernels.yaml` is checked, if at all)

- [ ] **Step 1:** NetworkPolicy for pods labelled `app=egret-apps`: ingress only from the egret namespace on 2718/2719 (the proxy lives in egret); egress only DNS and egret's service port 8080 (gateway and pod API). Same note as the kernel policy about kindnet not enforcing it.
- [ ] **Step 2:** `values.yaml`: `apps.origin`, `apps.listen`, `apps.pod.*`, `apps.image: egret-apps:dev`; the egret `Service` gains port 8081; `rbac.yaml` gets the verbs on `pods` and `secrets` in the apps namespace (least privilege: `create,get,list,delete`, no `update`).
- [ ] **Step 3:** `helm template deploy/helm/egret --set apps.origin=https://apps.example --set kernel.launcher=kubernetes | kubectl apply --dry-run=client -f -` renders without error. Commit `feat(deploy): apps pods — NetworkPolicy, values, RBAC`.

### Task D6: PR P4d

- [ ] fmt, clippy, suites (`cargo test`, `pytest apps-image/tests`), open the PR, stop for approval.

---

# Part P4e — frontend, deploy, live verification

Branch `feat/data-apps-p4e-ui` off main after P4d merges.

### Task E1: Apps list

**Files:**
- Create: `frontend/src/routes/explore/Apps.tsx`, `frontend/src/api/apps.ts`
- Modify: `frontend/src/App.tsx` (route `apps`, gated by `RequireFeature feature="apps"`), `frontend/src/components/layout/Sidebar.tsx` (the Explore entry "Notebooks" becomes "Apps"; the `notebooks/*` routes stay until P6 but have no menu entry)
- Test: `frontend/src/routes/explore/__tests__/Apps.test.tsx`

- [ ] **Step 1:** Read `frontend/src/routes/explore/Notebooks.tsx` and the "canonical Explore list-page scaffold" in memory `egret-ui-redesign`. Copy the scaffold; do not invent a layout.
- [ ] **Step 2: Failing tests:** the list renders each app's status ("Draft only" / "Published v3"); the **Edit** button shows only with `app:write` and **Open** only with a published version (Open is a P5 target: render it disabled with a title "Run mode arrives in the next release" rather than a dead link); the sidebar shows "Apps" and not "Notebooks".
- [ ] **Step 3:** Implement with the generated client from `npm run gen:api`. **Step 4:** `npm test -- Apps` PASS. **Step 5:** commit `feat(frontend): Apps list replaces the Notebooks entry`.

### Task E2: Edit page

**Files:**
- Create: `frontend/src/routes/explore/AppEditPage.tsx`
- Modify: `frontend/src/App.tsx` (`apps/:id/edit`)
- Test: `frontend/src/routes/explore/__tests__/AppEditPage.test.tsx`

- [ ] **Step 1: Failing tests:** on mount the page `POST`s the ticket endpoint and sets the iframe `src` to the returned URL with `sandbox="allow-scripts allow-same-origin allow-forms allow-downloads"`; while the ticket request or the pod start is pending it shows the loading screen; a 403 shows an access message and **no iframe**; a 503 ("apps origin not configured") shows an operator message; the theme is passed as a query parameter; the header shows the name, the draft revision and **Publish** (wired to the existing publish endpoint, P2) and **Versions**.
- [ ] **Step 2:** Implement. The iframe is rendered only after the ticket resolves (a ticket is single-use and 30 s: never prefetch, never re-render the iframe with the same URL — keep `src` in a ref). Handle `window` `message` events **only** from the apps origin if marimo posts any; none is needed in P4, so add no listener (YAGNI).
- [ ] **Step 3:** PASS; commit `feat(frontend): app edit page with the sandboxed marimo iframe`.

### Task E3: Ingress, TLS, kind

**Files:**
- Modify: `deploy/helm/egret/templates/ingress.yaml`, `values.yaml`, `ingress-tls-secret.yaml`, `justfile` (kind recipes), `docs/en/` deploy note

- [ ] **Step 1:** Ingress: a second `rules` entry with `host: apps.{{ .Values.ingress.host }}`, path `/`, backend service `egret` port 8081; `tls.hosts` lists both; `nginx.ingress.kubernetes.io/proxy-read-timeout` stays pinned **and** add `proxy-send-timeout`, `proxy-http-version: "1.1"`, `proxy-buffering: "off"` on a **separate Ingress object for the apps host** (the existing annotation comment already says the single object cuts idle websockets at 60 s — the apps host needs 3600 s, so it gets its own object, and leaves the existing object's 60 s annotation alone).
- [ ] **Step 2:** TLS on kind: the self-signed cert's SANs must include `apps.egret.localtest.me`; follow `ingress-tls-secret.yaml` and memory `egret-deploy-kind`. `auth.cookie_secure=true` in the kind values (the whole point: `__Host-` cookies must exist).
- [ ] **Step 3:** OIDC (Keycloak) redirect URIs and web origins: confirm login still works with `cookie_secure=true`; add the apps host to Keycloak only if the SPA's token flow needs it (it should not — the apps origin never talks to Keycloak).
- [ ] **Step 4:** `just` recipe `kind-up-apps` (or the existing deploy recipe + values) deploys; `curl -k https://apps.egret.localtest.me/_ticket?t=x` → 403, `curl -k https://egret.localtest.me/_ticket` → 404. Commit `feat(deploy): apps host ingress and TLS on kind`.

### Task E4: Live verification (the acceptance criteria)

**Files:**
- Create: `frontend/e2e/apps-security.spec.ts` (or the repo's Playwright location — check `frontend/e2e/` and memory `egret-frontend-viz-verify`), `docs/en/plans/2026-10-02-data-apps-p4-live-result.md`
- Seed an app whose source is a hostile author: an anywidget cell whose JS (a) reads `parent.document` and `document.cookie`, (b) `fetch`es egret's `whoami` with and without `mode:'no-cors'`, (c) POSTs to egret with `X-Egret-CSRF`, (d) sets `__Host-egret_csrf` with a `Domain=` attribute then POSTs, (e) `fetch`es `/e/<other pod>/api/files/list_files` on the apps origin.

- [ ] **Step 1:** Run the stack on kind (`egret-deploy-kind`, `egret-frontend-viz-verify`; login admin@egret.local / Admin12345!; login is rate-limited 5 per 5 min — reuse one stored session across tests, memory `egret-dnd-e2e-and-login-limit`). Never run two suites concurrently.
- [ ] **Step 2:** Run, and record the observed result of each (pass **and** fail, with the evidence — a screenshot, the response, the cookie jar), in `…-p4-live-result.md`:
  1. an author opens an app on kind: iframe loads, a cell runs `egret.sql(...)` on Trino and on StarRocks, the query ran as the author (gateway audit row), save works;
  2. author JS cannot read the parent document, egret cookies, or any egret response;
  3. author JS cannot complete a state change: the no-cors POST and the header-bearing POST both fail;
  4. **a tossed `__Host-egret_csrf` never reaches egret** (S0's open question, now over HTTPS);
  5. **`Sec-Fetch-Site: same-site` is actually sent** for the widget's `fetch` to egret and egret answers 403 (D20 measured, not assumed); if it is not sent, say so and treat D20 as unproven;
  6. two editors: the second save gets 409 and marimo shows the toast; after a reload or a second tab the new session is at the latest revision (spec §5.1);
  7. the S0 matrix of 40 raw variants against the real proxy through the ingress, HTTP and WS: 0 reach marimo;
  8. an admin impersonating a user opens an app: the token carries the impersonator (audit row) — or the ticket is refused under impersonation; whichever the code does, the test pins it;
  9. logout while the editor is open: the next save and the next query fail with 401, not silently.
- [ ] **Step 3:** For every failure, stop and use `superpowers:systematic-debugging`; do not weaken a test. Memory `egret-live-check-beats-review`: defects will show up here that the unit tests did not.
- [ ] **Step 4:** Update the spec: P4 "done" status with the result doc link, and any decision the live run changed. Commit `docs: Data Apps P4 live result`.

### Task E5: PR P4e

- [ ] fmt, `npm run lint && npm test`, `cargo test`, `pytest`; open the PR; stop for approval.

---

## Self-review (against spec §9 P4 and §10)

| Spec requirement | Task |
|---|---|
| Ticket → cookie | B1, B2, C4 |
| Proxy with allow-list; one raw path checked and forwarded | C1, C2, C4 |
| `X-Notebook-Id` stripped or pinned | C3 |
| `__Host-` rename verified over HTTPS on kind | B3, E3, E4.4 |
| Audit that GETs have no side effects; CSRF header-based | B4 (+ D20 guard) |
| Edit launcher; Apps list; Edit page | D1–D5, E1, E2 |
| P1: revoke by id/login session; sweeper; expiry of login session; `impersonator`; boot check; audit-once | A1, A4 |
| P3: pod endpoints and pod tokens | A1–A3 |
| P3: proxy also blocks `/api/ai`, `/terminal`, copy, rename | C2 |
| P3: tokens via `secretKeyRef`; tini; exit codes; kernel uid; locked config; reload note; `egret.databases()`; grid layout | D2, D3 |
| P3: re-run contract tests, e2e and route inventory on every marimo upgrade | C2 step 8 |
| §10: client `X-Egret-*` never reaches a pod | C3, C4 test 3 |
| §10: token dies after logout / WS close / `app:read` removed | A4, C4 tests 8–9; `app:read` removal is covered by P1's per-call authorization (no new code) |
| §10: author JS cannot read cookies/responses, cannot change state, tossed `__Host-` cookie | E4.2–E4.4 |
| §10: two editors 409, with reload and second tab | E4.6 |
| §10: S0's 40 variants on HTTP and WS | C1, C4 test 7, E4.7 |
| §10: Ranger two-viewer test, `DELETE` 400, `app_versions` immutability | already covered by P1/P2; **not re-done here** (run mode is P5) |

Gaps and honest limits:
- **Run mode is not served** in P4: `PodResolver::run_pod` returns `NotFound`; the ticket endpoint accepts `mode: "run"` and the proxy has the `/r/` surface and policy so P5 only adds the launcher. The Open button is disabled (E1).
- **D19's open-WS count is per replica.** Idle expiry is the backstop; stated in the decision table.
- **D20 is unproven until E4.5.** If a target browser does not send `Sec-Fetch-Site`, the GET audit and the `__Host-` rename are the only defences, exactly as the spec had it.
- **Facts I did not verify while writing this plan** and that the tasks tell the implementer to read first: the exact signatures of `AppStore::{draft, save_draft, version}` and the `routes.rs` authorizer (A3); how a handler obtains the login session id (B2); `RateLimiter`'s API (A4.6); what the OIDC redirect sends in `Sec-Fetch-Site` (B4); the frontend Playwright location (E4); marimo's config key names (D3.3). Each is a read-then-adapt step, not a design question.
- The unmerged-local-state caveat: this plan was written against local `origin/main` at `972b2bb8` plus the P3 docs branch, because `git fetch` timed out. P3's merged code (#614, #615) was read from `origin/feat/data-apps-p3-image`. Rebase onto the real main before starting A1.

---

## As built — P4d (2026-10-02)

What differs from Part P4d above, and what was left.

**Built as planned:** the `AppPodLauncher` trait in `egret-apps` (D22) with a fake and a resolver adapter; the subprocess launcher (tested with real processes); the Kubernetes launcher (Pod and Secret builders unit-tested; the tokens reach the container only through `secretKeyRef`; read-only root, `emptyDir`s, no service-account token, `restartPolicy: Never`, exit codes 64/65/69 mapped); the idle reaper; `apps.pod.*` configuration with boot validation; Helm namespace (Pod Security `restricted`), RBAC, NetworkPolicy, values and env; `tini` as PID 1.

**Different from the plan:**

- **Idle is stamped in the cluster, not in memory** (`egret.io/last-used`, at most once a minute per replica), so several egret replicas agree on which pods nobody wants. The plan said "reaper on idle, as today".
- **`egret.databases()` is edit-only.** The plan did not say; listing a workspace's databases needs `database:manage` in egret's own API, so a viewer of a published app does not get it through an app they open. A run token gets 403. Connection details are never returned (name and engine only).
- **Kernel uid separation became `PR_SET_DUMPABLE=0` on the server**, measured on real Linux (control: a same-uid child reads the server's `/proc/<pid>/environ` and `mem`; after: denied) and pinned by the smoke test. It protects the server from its kernels, **not one kernel from another**. That matters for run pods (several viewers, one uid) and is **P5's problem**; in an edit pod there is one author whose cells already run arbitrary Python.
- **The config lock was measured before it was widened.** marimo 0.25.0 already ignores `server`, `ai`, `runtime.auto_instantiate` and `display.custom_css` from a notebook's PEP 723 header; what a header can still set (`dotenv`, `pythonpath`, `on_cell_change`, …) grants nothing a cell does not already have. Only `sharing.*` was added to the lock, derived from marimo's schema. `tests/test_config_lock.py` pins the measurement.
- **The smoke test now runs marimo with the Pod's exact hardening** (read-only root filesystem, all capabilities dropped, three writable volumes). Nobody had; it works.

**Not done:**

- **Grid layout (D23)** — blocked on a decision that is still open; nothing was changed.
- **A reload while the old WebSocket is still connected needing a second reload** — not reproduced or changed; the proxy's per-grant WebSocket count (D19) already keeps the new page's token alive, but the session-resume behaviour is marimo's (spec §5.3) and needs a browser (P4e).
- **The Kubernetes launcher has not run against a cluster.** Its pure parts are tested; creating the Secret and Pod, owner-reference patching, the readiness wait and the exit-code handling are exercised for the first time in P4e on kind.
