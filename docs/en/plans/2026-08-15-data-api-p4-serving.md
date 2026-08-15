# Data API P4 — The serving surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

> Spec: `docs/en/specs/2026-08-14-data-api-design.md` §6.2–6.4, §7, §8
> Part: P4 of 5 — see `docs/en/plans/2026-08-14-data-api-decomposition.md`
> Stacked on: P1 (#504), P2 (#505), P3 (#506) — **all merged to `main`**. Branch off `main`.

**Goal:** an external client exchanges its OAuth2 credential for a token and calls `GET /data-api/v1/{workspace}/{path}`, receiving rows produced by the published snapshot under its own principal's Ranger policy.

**Architecture:** a second axum router with its own middleware stack — no cookie, no CSRF, bearer only. It routes on `dataapi.endpoints (method, path)` where `state = 'live'`, then serves entirely from the snapshot reached through `live_version_id`. Verification is stateless (JWT + JWKS, no DB round trip); execution goes through `QueryExecPort::execute_as`, which P4 adds.

**Tech Stack:** Rust (axum, sqlx) · JWT/JWKS · Trino / StarRocks connectors · Cedar PDP.

## This is the only public surface in the feature. Read these before writing code.

Five contracts were established by earlier phases and are not negotiable here. Each exists because something already went wrong.

1. **`verify_with_jwk` validates NOTHING but the signature.** `crates/egret-machine-idp/src/token.rs:33-35` sets `validate_aud = false`, `validate_exp = false`, and clears `required_spec_claims`. A signature check proves the token came from egret's IdP — not that it was meant for the Data API, and **not that it is unexpired**. P4 must assert `aud == "data-api"` and check `exp` itself. `EgretCatalogGuard` (`crates/egret-bin/src/main.rs:376-392`) already does exactly this and carries the reasoning in a comment; follow its shape.
2. **Apply `normalize_path` to the INBOUND path.** axum percent-decodes a `{*path}` wildcard capture, and stored paths were never encoded. A `WHERE path = $1` written without normalising will silently miss and the endpoint is permanently unreachable. `egret_dataapi::endpoints::normalize_path` is the one function that decides this. An inbound path that fails the rule cannot match any stored row, so it is a 404 — not a 400.
3. **Re-check `delegated`.** `EndpointSnapshot.delegated` froze whether the connection delegated per-user credentials at publish time. `query.databases.delegated` is mutable, so flipping it to `true` after a `cache.scope = shared` version was published is a cross-principal leak that validate-time checking cannot close. Compare frozen against live before serving from a shared cache and refuse when they disagree.
4. **Detect truncation with `TabularResult.truncated`, never by comparing `row_count` to the cap.** The connector already fetches `max_rows + 1` and sets the flag (`crates/egret-query/src/execution/connector/mod.rs:63`). A count equal to the cap cannot distinguish "exactly at" from "over".
5. **Do not assume the connector refuses a second statement.** `execute` batches by design — SQL Lab needs it, and StarRocks executes every statement in a `COM_QUERY` regardless of `CLIENT_MULTI_STATEMENTS` (it never advertises the capability; measured with a raw wire probe). The Data API is safe because P3's byte test means no `;` can reach a snapshot, **not** because the connector protects you. If P4 ever composes SQL that did not come from a snapshot, that safety evaporates.

## Global Constraints

- **The serving router declares its own middleware.** It must not inherit cookie or CSRF layers from the control plane. CSRF has silently killed a production POST in this codebase before, with no router-level test able to see it. Every layer on this router is written down explicitly.
- **404, never 403, for anything a caller should not be able to distinguish**: unknown endpoint, disabled endpoint, wrong workspace. A caller must not learn that an endpoint exists in a workspace they cannot reach.
- **The cache key includes the principal** unless the snapshot says `scope = shared` AND the delegated re-check passes. Ranger filters per principal; a shared key leaks straight through it.
- **Never return a raw engine error.** Trino/StarRocks errors carry table names, column names and sometimes hosts. Map to the fixed code table in spec §7 and log the detail internally against `request_id`.
- **The serving path applies its own timeout.** It calls the connector directly with no JobManager wrapping it, exactly as `describe` does and for the same reason.
- Row cap overflow is `400 row_limit_exceeded`, not a truncated 200. Silent truncation produces wrong business numbers with no signal.
- Run scoped checks, not `just check`. **Throwaway `DATABASE_URL`** — unset, DB-gated suites report `ok` in 0.00s having asserted nothing. Never run two workspace test suites at once.

## File Structure

| File | Responsibility |
|---|---|
| `crates/egret-query-api/src/port.rs` (modify) | Add `execute_as` beside `describe_as`. |
| `crates/egret-query/src/execution/port_impl.rs` (modify) | Implement it, reusing `delegation_for`. |
| `crates/egret-dataapi/src/serving/auth.rs` (create) | Token → principal + workspace. `aud` and `exp` checked here. |
| `crates/egret-dataapi/src/serving/router.rs` (create) | `serving_router()`, its own middleware stack, the request chain. |
| `crates/egret-dataapi/src/serving/cache.rs` (create) | Principal-scoped response cache behind a trait. |
| `crates/egret-dataapi/src/serving/ratelimit.rs` (create) | `TokenBucket` with an injected clock, behind `RateLimitStore`. |
| `crates/egret-dataapi/src/serving/error.rs` (create) | The error envelope and the code table. |
| `crates/egret-dataapi/src/serving/access_log.rs` (create) | Write + retention prune. |
| `migrations/0123_dataapi_access_log.sql` (create) | `dataapi.access_log`. |
| `crates/egret-server/src/lib.rs` (modify) | Nest `serving_router()` under `/data-api/v1`, outside the `/api` group. |

---

### Task 1: `execute_as` on the port

**Files:**
- Modify: `crates/egret-query-api/src/port.rs`, `crates/egret-query/src/execution/port_impl.rs`
- Test: `crates/egret-query/tests/port_describe.rs` (extend)

**Interfaces:**
- Produces: `async fn QueryExecPort::execute_as(&self, ws: Uuid, database_id: Uuid, principal: UserId, sql: &str, limits: Limits) -> Result<TabularResult, egret_core::Error>`

P3 added `describe_as` only, deliberately, because a trait method with no caller is untested surface. P4 is that caller.

- [ ] **Step 1: Write the failing test**

Extend `crates/egret-query/tests/port_describe.rs`, following the fixture already there (it builds a real `EngineState` and self-skips without a live Trino).

```rust
#[tokio::test]
async fn execute_as_returns_rows_and_scopes_to_the_workspace() {
    let Some(f) = fixture().await else { eprintln!("skip"); return };
    let port: &dyn egret_query_api::QueryExecPort = &f.state;

    let out = port
        .execute_as(f.ws, f.database_id, f.user, "SELECT 1 AS n", small_limits())
        .await
        .expect("execute must succeed");
    assert_eq!(out.rows.len(), 1);
    assert_eq!(out.columns[0].name, "n");

    // Same scoping guarantee describe_as carries: a database from another
    // workspace is NotFound, checked before any network call.
    let err = port
        .execute_as(f.ws, f.foreign_database_id, f.user, "SELECT 1", small_limits())
        .await
        .expect_err("a foreign database must not execute");
    assert!(matches!(err, egret_core::Error::NotFound(_)), "got {err:?}");
}

#[tokio::test]
async fn execute_as_reports_truncation_rather_than_silently_capping() {
    let Some(f) = fixture().await else { eprintln!("skip"); return };
    let port: &dyn egret_query_api::QueryExecPort = &f.state;
    let limits = Limits { max_rows: 2, timeout: std::time::Duration::from_secs(30) };

    let out = port
        .execute_as(f.ws, f.database_id, f.user,
                    "SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3", limits)
        .await
        .unwrap();
    assert!(out.truncated, "the caller must be able to tell it did not get everything");
    assert_eq!(out.rows.len(), 2);
}
```

Run: `cargo test -p egret-query --test port_describe`

Expected: compile error — `execute_as` does not exist.

- [ ] **Step 2: Add it to the trait**

In `crates/egret-query-api/src/port.rs`, beside `describe_as`. Mirror its rustdoc, including the `# Scoping` heading — the scoping obligation is identical and P4 must not have to guess.

```rust
    /// Run `sql` as `principal` and return the rows, honouring `limits`.
    ///
    /// # Scoping
    /// Like [`Self::describe_as`], this refuses a `database_id` not granted to
    /// `ws` with [`egret_core::Error::NotFound`], before resolving the principal
    /// and before any network call. The caller still owes the PERMISSION half.
    ///
    /// # Errors
    /// Engine failures are mapped by variant, not flattened: a Ranger denial is
    /// [`egret_core::Error::Forbidden`], a bad statement is `InvalidInput`, an
    /// unreachable engine is `Internal`.
    async fn execute_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
        limits: Limits,
    ) -> Result<TabularResult, egret_core::Error>;
```

**`Limits` is not reachable from the leaf crate and this signature will not compile as written.** It lives at `crates/egret-query/src/execution/model.rs:42` — in the seam crate, which `egret-query-api` must not depend on. It is a plain `{ max_rows: usize, timeout: Duration }` with no sqlx or axum in it, i.e. exactly the shape the contract crate already holds for `Engine` and `Database`.

Move it to `crates/egret-query-api/src/lib.rs` and re-export it from `egret-query`'s `execution::model` so every existing caller keeps compiling unchanged (`pub use egret_query_api::Limits;`). Do **not** duplicate the struct — two `Limits` that drift is the same failure mode `quote_string` and `QueryExecPort` both exist to prevent.

Verify no existing behaviour changed: `cargo test -p egret-query` covers the callers.

- [ ] **Step 3: Implement it**

In `port_impl.rs`, beside `describe_as`. The two share everything up to the call: the `get_in_workspace` scoping check, the principal's email, `delegation_for`, `open_secret`, `ConnSpec`. **Extract that shared prelude into one function** rather than copying it — a second copy of the delegation resolution is the drift this port exists to prevent. Reuse `describe_error`'s variant mapping for the same reason.

- [ ] **Step 4: Run against a live engine**

Run with `EGRET_TEST_TRINO_URL` set as `port_describe.rs`'s existing tests document. Expected: PASS, no "skip" in the output.

- [ ] **Step 5: Verify the scoping check by mutation**

Remove the `get_in_workspace` call from `execute_as`. Expected: `execute_as_returns_rows_and_scopes_to_the_workspace` FAILS on the second half. This is the same hole `describe_as` had in P3 — it was found by mutation, not by reading. Revert.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-query-api crates/egret-query
git commit -m "feat(query): execute_as on QueryExecPort

P3 added describe_as alone because a trait method with no caller is
untested surface; P4 is the caller. Shares one prelude with describe_as —
the scoping check, delegation resolution and error mapping — rather than
a second copy, which is the drift this port exists to prevent."
```

---

### Task 2: Token verification

**Files:**
- Create: `crates/egret-dataapi/src/serving/auth.rs`, `crates/egret-dataapi/src/serving/mod.rs`
- Modify: `crates/egret-dataapi/src/lib.rs`
- Test: `crates/egret-dataapi/tests/serving_auth.rs` (create)

**Interfaces:**
- Produces: `pub struct ServingPrincipal { pub user_id: UserId, pub workspace_id: Uuid, pub client_id: String }` and `pub async fn verify_serving_token(keys: &dyn SigningKeyStore, bearer: &str) -> Result<ServingPrincipal, ServingError>`

- [ ] **Step 1: Write the failing tests**

These are the tests that matter most in P4. Each names a property `verify_with_jwk` does **not** give you.

```rust
#[tokio::test]
async fn a_catalog_token_is_refused_by_the_data_api() {
    // Same signing key, different audience. verify_with_jwk sets
    // validate_aud = false, so the signature alone proves nothing about intent.
    let f = fixture().await;
    let catalog = f.mint_token("catalog", f.ws, f.user).await;
    assert!(matches!(
        verify_serving_token(&*f.keys, &catalog).await,
        Err(ServingError::Unauthorized)
    ));
}

#[tokio::test]
async fn an_expired_token_is_refused() {
    // verify_with_jwk ALSO sets validate_exp = false. Nothing but this check
    // stands between an expired credential and a served request.
    let f = fixture().await;
    let stale = f.mint_token_expiring_at("data-api", f.ws, f.user, past()).await;
    assert!(matches!(
        verify_serving_token(&*f.keys, &stale).await,
        Err(ServingError::Unauthorized)
    ));
}

#[tokio::test]
async fn a_token_with_no_exp_at_all_is_refused() {
    // required_spec_claims is cleared, so a token carrying no `exp` verifies.
    // Treat a missing exp as expired, never as "never expires".
    let f = fixture().await;
    let no_exp = f.mint_token_without_exp("data-api", f.ws, f.user).await;
    assert!(matches!(
        verify_serving_token(&*f.keys, &no_exp).await,
        Err(ServingError::Unauthorized)
    ));
}

#[tokio::test]
async fn a_valid_token_yields_its_principal_and_workspace() {
    let f = fixture().await;
    let t = f.mint_token("data-api", f.ws, f.user).await;
    let p = verify_serving_token(&*f.keys, &t).await.unwrap();
    assert_eq!(p.workspace_id, f.ws);
    assert_eq!(p.user_id, f.user);
}

#[tokio::test]
async fn a_token_signed_by_another_key_is_refused() {
    let f = fixture().await;
    let foreign = f.mint_with_foreign_key("data-api", f.ws, f.user).await;
    assert!(matches!(
        verify_serving_token(&*f.keys, &foreign).await,
        Err(ServingError::Unauthorized)
    ));
}
```

- [ ] **Step 2: Run, implement, run**

Implement following `EgretCatalogGuard`'s shape (`crates/egret-bin/src/main.rs:376-392`) — check `exp` against `chrono::Utc::now().timestamp()`, then `aud`, then read `ws` and `egret_user_id` from the claims. Treat a missing or unparseable claim as unauthorized, never as a default.

Every failure returns the same `ServingError::Unauthorized`. Do not distinguish "wrong audience" from "expired" from "bad signature" in the response — that is a free oracle.

- [ ] **Step 3: Verify each check by mutation**

Remove each of the three checks in turn — `exp`, `aud`, signature-key selection — and confirm the matching test fails. All three must bite independently; a token that fails two checks would otherwise hide a missing third.

- [ ] **Step 4: Commit**

---

### Task 3: Routing and snapshot resolution

**Files:**
- Create: `crates/egret-dataapi/src/serving/router.rs`
- Modify: `crates/egret-server/src/lib.rs`
- Test: `crates/egret-dataapi/tests/serving_routing.rs` (create)

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn an_inbound_path_is_normalised_before_lookup() {
    // axum percent-decodes the wildcard capture; stored paths were never
    // encoded. Without normalising the inbound path this lookup misses and the
    // endpoint is unreachable forever.
    let f = fixture().await;
    f.publish_endpoint("GET", "/reports/daily", "SELECT 1 AS n").await;

    for variant in ["reports/daily", "/reports/daily", "/reports/daily/"] {
        let res = f.call(variant).await;
        assert_eq!(res.status(), 200, "variant {variant:?} must reach the endpoint");
    }
}

#[tokio::test]
async fn a_draft_edit_does_not_change_what_is_served() {
    // The invariant P3 exists to protect, asserted through the serving surface
    // rather than the store.
    let f = fixture().await;
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1 AS n").await;
    assert_eq!(f.call("/x").await.json_rows(), vec![[1]]);

    f.patch_draft(ep, serde_json::json!({ "sql": "SELECT 2 AS n" })).await;
    assert_eq!(
        f.call("/x").await.json_rows(), vec![[1]],
        "serving must follow the published snapshot, not the draft"
    );
}

#[tokio::test]
async fn a_disabled_endpoint_is_404_not_403() {
    let f = fixture().await;
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1").await;
    f.disable(ep).await;
    assert_eq!(f.call("/x").await.status(), 404);
}

#[tokio::test]
async fn a_token_for_another_workspace_is_404() {
    // Not 403: a caller must not learn the endpoint exists.
    let f = fixture().await;
    f.publish_endpoint("GET", "/x", "SELECT 1").await;
    let other = f.token_for_workspace(f.other_ws).await;
    assert_eq!(f.call_with("/x", &other).await.status(), 404);
}
```

- [ ] **Step 2: Run, implement, run**

Route on `dataapi.endpoints (method, path)` restricted to `state = 'live'` and the token's workspace, join to `endpoint_versions` through `live_version_id`, and take **everything that decides the response** from the snapshot.

- [ ] **Step 3: Mount it**

In `crates/egret-server/src/lib.rs`, nest under `/data-api/v1` as its own group — **not** inside the `/api` group, and with no cookie or CSRF layer. Add one `doc.merge(...)` line, keeping the list explicit.

- [ ] **Step 4: Prove the middleware is not inherited**

Add a test asserting a `POST` to a live endpoint succeeds **without** a CSRF header. If it fails with a CSRF error, the router picked up the control plane's layer. That is the failure this repo has already shipped once, and no router-level test caught it then.

- [ ] **Step 5: Verify by mutation**

Point the lookup at the draft's `sql` instead of the snapshot's. Expected: `a_draft_edit_does_not_change_what_is_served` FAILS. Revert.

- [ ] **Step 6: Commit**

---

### Task 4: Cache and rate limit

**Files:**
- Create: `crates/egret-dataapi/src/serving/cache.rs`, `crates/egret-dataapi/src/serving/ratelimit.rs`
- Test: `crates/egret-dataapi/tests/serving_cache.rs`, `.../serving_ratelimit.rs`

- [ ] **Step 1: The cache-leak test, written first**

```rust
#[tokio::test]
async fn two_principals_do_not_share_a_cached_response() {
    // Ranger filters per principal, so the same endpoint and the same params
    // can legitimately return different rows. A cache key without the principal
    // serves one identity's rows to another.
    let f = fixture().await;
    let ep = f.publish_endpoint_on_delegated_connection("GET", "/x", "SELECT 1 AS n").await;

    let a = f.call_as(&f.principal_a, "/x").await.json_rows();
    let b = f.call_as(&f.principal_b, "/x").await.json_rows();

    assert_ne!(a, b, "the fixture grants A and B different rows");
    assert_eq!(f.call_as(&f.principal_b, "/x").await.json_rows(), b,
               "B's second call must still be B's rows");
}

#[tokio::test]
async fn a_shared_cache_is_refused_when_delegated_flipped_after_publish() {
    // The one snapshot-external input. Publish `shared` against a
    // non-delegated database (allowed), then flip the database to delegated.
    let f = fixture().await;
    let ep = f.publish_shared_cache_on_non_delegated("GET", "/x").await;
    f.set_database_delegated(true).await;

    let res = f.call("/x").await;
    assert_ne!(res.status(), 200,
        "the frozen witness disagrees with the live column — refuse rather than leak");
}
```

- [ ] **Step 2: The rate limiter**

`TokenBucket` with `check_at(key, now)` — the clock is injected, so no test sleeps. Behind a `RateLimitStore` trait; in-memory per process for v1, with a note that a multi-replica deployment needs a shared implementation. Model the shape on `crates/egret-auth/src/ratelimit.rs` but **not** its semantics: that one is login-style lockout, and an API needs 429 + `Retry-After`, retryable as soon as the bucket refills.

Two time points in any assertion must be genuinely separated — never able to coincide.

- [ ] **Step 3: Run, implement, run, mutate**

Remove the principal from the cache key. Expected: `two_principals_do_not_share_a_cached_response` FAILS. Remove the delegated re-check. Expected: the second test FAILS. Revert each.

- [ ] **Step 4: Commit**

---

### Task 5: Errors, access log, retention

**Files:**
- Create: `crates/egret-dataapi/src/serving/error.rs`, `.../access_log.rs`, `migrations/0123_dataapi_access_log.sql`
- Test: `crates/egret-dataapi/tests/serving_errors.rs`

- [ ] **Step 1: The error table**

Implement spec §7's code table exactly. The test that matters:

```rust
#[tokio::test]
async fn an_engine_error_never_reaches_the_caller_verbatim() {
    let f = fixture().await;
    f.publish_endpoint("GET", "/x", "SELECT * FROM does_not_exist").await;
    let body = f.call("/x").await.text().await;

    assert!(!body.contains("does_not_exist"), "leaked a table name: {body}");
    assert!(!body.contains(&f.engine_host), "leaked the engine host: {body}");
    assert!(body.contains("request_id"), "must be correlatable: {body}");
}

#[tokio::test]
async fn a_ranger_denial_is_not_reported_as_bad_sql() {
    // The author's SQL is fine; their policy denies the table. Telling them
    // their input is invalid sends them to debug the wrong thing.
    let f = fixture().await;
    let res = f.call_as(&f.principal_without_grant, "/x").await;
    assert_eq!(res.status(), 403);
    assert_eq!(res.json()["error"]["code"], "data_access_denied");
}

#[tokio::test]
async fn exceeding_the_row_cap_is_an_error_not_a_truncated_200() {
    let f = fixture().await;
    f.publish_endpoint_with_max_rows("GET", "/x", "SELECT 1 UNION ALL SELECT 2", 1).await;
    let res = f.call("/x").await;
    assert_eq!(res.status(), 400);
    assert_eq!(res.json()["error"]["code"], "row_limit_exceeded");
}
```

The last one must be driven by `TabularResult.truncated`, not by comparing `row_count` to the cap.

- [ ] **Step 2: The access log**

```sql
CREATE TABLE dataapi.access_log (
  id                 uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  ts                 timestamptz NOT NULL DEFAULT now(),
  workspace_id       uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  endpoint_id        uuid,
  version            integer,
  service_account_id uuid,
  client_id          text,
  status_code        integer NOT NULL,
  row_count          bigint,
  duration_ms        integer NOT NULL,
  cache_hit          boolean NOT NULL DEFAULT false,
  engine_query_id    text,
  client_ip          inet,
  error_code         text,
  request_id         text NOT NULL
);
CREATE INDEX access_log_ws_ts_idx ON dataapi.access_log (workspace_id, ts DESC);
```

Note the deliberate absence of FKs on `endpoint_id` / `service_account_id`: an access log must survive the deletion of what it describes. That is the opposite of the choice `0121` made for `endpoints.database_id`, and for the opposite reason — say so in the migration comment.

Retention follows `AuditStore::prune(retention_days)` (`crates/egret-audit/src/lib.rs:209`), which is the established pattern here. There is no partitioning precedent in this repo; do not introduce one without a measured reason.

- [ ] **Step 3: Verify by mutation**

Make the error mapper pass the engine's message through. Expected: `an_engine_error_never_reaches_the_caller_verbatim` FAILS naming the leaked table. Revert.

- [ ] **Step 4: Full scoped check and commit**

```bash
cargo fmt --all --check
cargo clippy -p egret-dataapi -p egret-dataapi-api -p egret-query -p egret-query-api -p egret-server --all-targets
cargo test -p egret-dataapi -p egret-dataapi-api -p egret-query -p egret-query-api -p egret-server
```

---

## Done when

- All five tasks' tests green, each observed FAILING against its stated mutation.
- **A live end-to-end run:** issue a client through the P2 management API, exchange it at `/oauth/token`, call a published endpoint with the resulting token, and get rows. Then revoke the client and confirm the next token request fails. This is the acceptance criterion; no test replaces it.
- The three P3 contracts are honoured and each has a test: inbound `normalize_path`, routing on `endpoints` with content from the snapshot, and the `delegated` re-check.
- `aud` and `exp` are both asserted, and both mutation-proven — `verify_with_jwk` gives you neither.
- A `POST` to a live endpoint succeeds with no CSRF header.
- The spec's §6.2 chain matches what the code does, in both languages.

## Not in this plan

- **The management UI** — P5, including the `egret-bi` `kind_perms` arm that makes `folder_id` reachable.
- **Auto-generated OpenAPI per endpoint and the Try-it console** — P5.
- **A shared-store rate limiter.** In-memory per process is correct while the serving surface is in-process; the trait is the seam for when it is not.
- **Instant revocation.** Deliberately rejected in the spec: a stateless token stays valid until `exp`, and the 600s TTL is the bound. Do not add a per-request revocation check — it gives back the throughput the design was chosen for.
- **The four unaudited management routes** and the `Redacted<String>` newtype, both recorded on #506.
