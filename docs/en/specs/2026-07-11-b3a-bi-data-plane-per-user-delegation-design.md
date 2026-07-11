# B3a — BI data-plane per-user delegation

> **Status:** Design (approved to plan) · **Date:** 2026-07-11 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) §2 (B3) · **Builds on:** [F2 OIDC delegation](./2026-06-30-notebook-f2-oidc-delegation-design.md) (per-user engine JWT for the notebook plane — DONE, live-verified PRs #99–104), B2-1b/B2-2 (object-scoped BI data endpoints).
>
> First B3 increment. Brings F2's per-user engine delegation to the **BI data plane** (SQL Lab, saved chart/dashboard/dataset data). Today the BI `run_query` path runs on the workspace connection's shared service credentials; B3a makes it present the **caller's** per-user engine JWT so the engine authorizes per-user. Ranger grant-federation is a separate increment (B3b).

---

## 1. Goal & scope

F2 delivered the per-user OIDC/Keycloak delegation mechanism (a short-lived downscoped engine JWT that IS the user, minted by the `EngineTokenMinter` broker; connectors already accept a `bearer_token`) — and proved it live for the **notebook/kernel** plane (Trino over TLS, StarRocks via a patched driver, egret-catalog in-process). The **BI plane** (`egret_query::execution::run_query`, called by SQL Lab submit + the object-scoped chart/dashboard/dataset data endpoints) still passes `bearer_token = None`, so BI queries run as the workspace connection's **shared service user** — the last non-delegated data path.

B3a closes that gap by reusing F2's machinery: on a connection marked **delegated**, the BI query path mints the caller's engine JWT and presents it as the connector bearer, so the engine (and whatever authz it runs — native ACL, Keycloak-group RBAC, or Ranger) enforces **per-user**.

**In scope:** a per-connection `delegated` flag on `databases` (+ input/DTO/UI); a `bearer_token` parameter on `run_query`; a `resolve_delegation` helper in egret-bi applied at every BI `run_query` call site (SQL Lab submit, `POST /charts/{id}/data`, `POST /dashboards/{id}/cards/{card_id}/data`, `POST /datasets/{id}/data`); injecting the existing `EngineTokenMinter` Arc into `BiState`; a governed-sharing error path (viewer without engine data access → friendly 403).

**Out of scope:** Ranger grant-federation — projecting egret's `access_grants` into engine authz (B3b); vended per-user S3 credentials (F2-P6); the notebook plane (already delegated via F2); non-delegated connections (unchanged).

**Decisions locked in brainstorming:** deliver B3a first, then B3b (Ranger) as a separate cycle; **all** BI data (SQL Lab + saved shared objects) runs as the **caller/viewer's** identity (per-user governed — a shared dashboard's data is still gated by the viewer's engine data access; the object-share only grants seeing the definition); delegation is opt-in **per connection** (`delegated` flag); a non-OIDC user hitting a delegated connection is **denied (403)**, not silently fallen back (no governance bypass); the flag is a **dedicated column**; the JWT is **minted per-request** (the broker caches internally — no extra BI-layer cache now).

## 2. Data model — the `delegated` flag

`ALTER TABLE databases ADD COLUMN delegated boolean NOT NULL DEFAULT false;` (new migration; repo is past 0074). Semantics:
- **`delegated = false` (default):** current behavior — the connector uses the connection's stored service credentials (`secret`) and the current `session_user`. No per-user JWT. Non-breaking for every existing connection.
- **`delegated = true`:** BI queries run as the caller's per-user engine JWT. A workspace admin marks a connection delegated only when its target engine is configured to accept the Keycloak JWT (F2's Trino/StarRocks JWT config). The connector presents the bearer (which takes precedence over basic-auth, per F2-P3) as the caller's identity.

Expose `delegated` on `DatabaseInput` (create/update), `DatabaseDetailDto`/`DatabaseListDto`, and add a toggle to the DatabasesPage connection form. Default false in all DTOs so omitting it preserves behavior.

## 3. Execution path

### 3.1 `run_query` gains a bearer
`egret_query::execution::run_query(databases, factory, database_id, sql, limit, session_user, limits, cancel)` currently calls `factory.build(&db, secret, session_user, None)`. Add a `bearer_token: Option<String>` parameter threaded into `factory.build(&db, secret, session_user, bearer_token)`. `ConnectorFactory::build` already takes `bearer_token` (F2), and `TrinoConnector`/`StarRocksConnector` already use it (bearer precedence over basic-auth, all request sites). No connector change.

### 3.2 The `resolve_delegation` helper (egret-bi)
A single helper DRYs the per-call-site logic:

```
async fn resolve_delegation(st: &BiState, ctx: &WorkspaceContext, database_id: Uuid)
    -> Result<(Option<String> /*session_user*/, Option<String> /*bearer*/), BiApiError>
```

- Load the database (`st.databases.get(database_id)`). If `!db.delegated`: return `(current_session_user, None)` — the existing behavior (session_user derived as today for audit/X-Trino-User).
- If `db.delegated`:
  1. Mint the caller's engine JWT: `st.engine_minter.mint(ctx.principal.user_id.0).await`.
     - `Err(BrokerError::NoRefreshToken)` → `Error::Forbidden` (a delegated connection requires an OIDC session; a non-OIDC/password user cannot query it). Surface a clear message ("this connection requires SSO sign-in").
     - `Err(other)` → `Error::Internal`.
  2. Derive `session_user` = the caller's username via `UserRepo::ensure_username(user_id)` — this MUST equal the JWT `preferred_username` (F2 live gotcha: the engine matches the login/session user to the JWT principal; a mismatch is rejected as "login name X not matched to user Y"). Keycloak's `preferred_username` is egret's username, so `ensure_username` is the source of truth for both.
  3. Return `(Some(username), Some(jwt.access_token))`.

Every BI `run_query` call site calls `resolve_delegation` first, then passes the returned `(session_user, bearer)` into `run_query`.

### 3.3 Call sites
Apply at all BI data paths (all funnel through `run_query`):
- SQL Lab submit (the interactive query path).
- `POST /ws/{ws}/charts/{id}/data` (B2-1b) — runs the saved chart's spec as the **caller** (viewer).
- `POST /ws/{ws}/dashboards/{id}/cards/{card_id}/data` (B2-1b) — runs the card's chart as the **caller** (transitive on the dashboard grant, but the DATA runs as the viewer's engine identity).
- `POST /ws/{ws}/datasets/{id}/data` (B2-2) — dataset preview as the caller.

## 4. Governed sharing

Per the per-user decision, a shared saved object's data runs as the **viewer's** engine identity. The object-share (B2) grants seeing the object's *definition*; the *data* is still gated by the engine's per-user authz. Consequence: a viewer with an object grant but no engine access to the underlying tables gets an engine denial. Surface this as a friendly error (e.g. map the connector's auth/permission error to a 403 with "you don't have data access to this source") rather than a raw 500, so the UI can explain it. This is the intended, secure governed model — not a regression.

## 5. State wiring

`BiState` gains `engine_minter: Arc<dyn EngineTokenMinter>` (the F2 seam, already in `egret-analytics-api`/`egret-notebook`) and access to `UserRepo` for `ensure_username` (BiState already has `sessions`/`db`; add a `users: UserRepo` or reuse an existing handle). In `egret-bin/main.rs`, the `engine_minter` Arc is built (~line 1079) AFTER `BiState` (~line 1060); relocate the minter construction above `BiState` (as B1b-3 relocated the pdp/bus block) and pass `engine_minter.clone()` into `BiState`, reusing the SAME Arc shared with `NotebookState`/`KernelState` (one broker).

## 6. Security

- **Delegation, not impersonation:** the bearer JWT IS the caller (short-lived, downscoped, `aud` = engines). egret never holds a privileged act-as credential on the BI path; the engine authorizes the real user. (F2-proven: Trino enforces per-user JWT over TLS; StarRocks via the patched driver.)
- **No bypass:** a non-OIDC user on a delegated connection is denied (403), so per-user enforcement can't be sidestepped by a password login. Non-delegated connections are explicitly the shared-service model (admin's choice).
- **session_user ↔ JWT binding:** `session_user` is derived from `ensure_username` = the JWT `preferred_username`; a mismatch is rejected by the engine (defense-in-depth; the helper sets both from one source).
- **Governed viewer:** a shared-object viewer runs as themselves → the engine gates the data. The object-share never widens data-plane access.
- **Token handling:** the JWT is minted per-request via the broker (which caches/refreshes internally) and passed as the bearer; it is never persisted by the BI layer and is redacted in logs (F2's `EngineToken` Debug redaction).

## 7. Testing

- **Delegated database:** with a mock `EngineTokenMinter`, `run_query` receives the minted bearer and `session_user` = the username; a minter returning `NoRefreshToken` → the endpoint returns 403; a delegated chart/dashboard/dataset data endpoint threads the bearer.
- **Non-delegated database:** `run_query` receives `bearer = None` and the current `session_user` — behavior byte-identical to today (regression guard).
- **All four call sites** route through `resolve_delegation` (assert each mints on delegated, passes None on non-delegated).
- **Connector reuse:** the existing `connector_trino`/`connector_starrocks` bearer tests cover that the bearer reaches the wire; no new connector test needed.
- **Full gate:** BE `cargo test --workspace`, FE `lint/typecheck/test/build`, openapi + client current (the `delegated` field is new), adversarial security review.

## 8. Open questions (for the plan)

- **session_user today:** confirm what the BI `run_query` call sites currently pass as `session_user` (the caller's username for X-Trino-User audit, or the service user). The helper preserves it for non-delegated; for delegated it forces `ensure_username`. Inspect the exact current value during the plan.
- **Error mapping granularity:** the connector returns an `EngineError`; the plan must map an auth/permission failure on a delegated connection to a 403 with a data-access message, distinct from a generic 500. Confirm the connector surfaces a distinguishable auth error, or wrap conservatively (a delegated-connection engine error that looks like auth → 403).
- **Migration number:** repo is past 0074; the plan picks the next free number and touches `egret-metastore/src/lib.rs` (sqlx re-embed).

---

*Design v1.0 (2026-07-11). Brings F2's per-user engine JWT delegation to the BI data plane: a per-connection `delegated` flag makes `run_query` present the caller's minted engine JWT (reusing the F2 broker + connector bearer), so BI queries — SQL Lab and shared saved objects alike — run as the viewer under the engine's per-user authz. Ranger grant-federation is B3b.*
