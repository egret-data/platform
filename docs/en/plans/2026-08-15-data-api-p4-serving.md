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
async fn an_archived_workspace_stops_serving() {
    // P2's ClientStore::verify refuses to MINT for an archived workspace but
    // cannot recall a token already issued. The workspace row is already in
    // hand here, so the archive case costs nothing to close — and archiving is
    // the operator's most natural decommission action.
    let f = fixture().await;
    f.publish_endpoint("GET", "/x", "SELECT 1").await;
    let token = f.token().await;
    assert_eq!(f.call_with("/x", &token).await.status(), 200);

    f.archive_workspace().await;
    assert_eq!(f.call_with("/x", &token).await.status(), 404,
        "the same still-unexpired token must stop working");
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

- [ ] **Step 2: Implement the lookup**

The one query. Everything that decides the response comes from `v.snapshot`; `e` is joined only to route and to scope.

```rust
/// Resolve an inbound request to a live snapshot.
///
/// Routing comes from `dataapi.endpoints (method, path)` — the pair carrying
/// `UNIQUE (workspace_id, method, path)` — restricted to `state = 'live'` and
/// the token's workspace. CONTENT comes only from `v.snapshot`. See
/// `egret_dataapi::lifecycle`'s module comment for why the split is that way
/// round: a draft edit can move a live endpoint's URL, so routing must follow
/// the endpoint row, while what it returns must not.
async fn live_snapshot(
    db: &Db,
    ws: Uuid,
    method: &str,
    path: &str,
) -> Result<Option<(Uuid, i32, EndpointSnapshot)>, ServingError> {
    let row: Option<(Uuid, i32, serde_json::Value)> = sqlx::query_as(
        "SELECT e.id, v.version, v.snapshot \
         FROM dataapi.endpoints e \
         JOIN dataapi.endpoint_versions v ON v.id = e.live_version_id \
         WHERE e.workspace_id = $1 AND e.method = $2 AND e.path = $3 \
           AND e.state = 'live'",
    )
    .bind(ws)
    .bind(method)
    .bind(path)
    .fetch_optional(&db.pool)
    .await
    .map_err(ServingError::internal)?;

    let Some((id, version, raw)) = row else { return Ok(None) };
    let snapshot: EndpointSnapshot =
        serde_json::from_value(raw).map_err(ServingError::internal)?;
    Ok(Some((id, version, snapshot)))
}
```

And the handler's opening, where the inbound path is normalised **before** the lookup:

```rust
async fn serve(
    State(st): State<DataApiState>,
    Path((ws_slug, raw_path)): Path<(String, String)>,
    method: axum::http::Method,
    headers: axum::http::HeaderMap,
) -> Result<Json<ServingResponse>, ServingError> {
    let principal = verify_serving_token(&*st.keys, bearer(&headers)?).await?;

    // axum percent-decodes a `{*path}` capture; stored paths were never encoded,
    // and P3 stores them canonical. A path that fails the rule cannot match any
    // stored row, so it is a 404 — not a 400: a caller must not be able to tell
    // "malformed" from "absent" here.
    let path = egret_dataapi::endpoints::normalize_path(&raw_path)
        .map_err(|_| ServingError::NotFound)?;

    // The workspace comes from the TOKEN, and the slug in the path must agree.
    // Disagreement is 404, never 403 — see the module comment.
    //
    // `status` is checked here too, and it is not redundant with P2's
    // ClientStore::verify. That refuses to MINT a token for an archived
    // workspace; it cannot recall one already issued, and the design accepts up
    // to `exp` of exposure. The workspace row is already in hand, so closing the
    // archive case for the serving path costs nothing — and archiving is the
    // operator's most natural decommission action. P2 shipped a version where
    // that action locked the admin out of the revoke routes while leaving the
    // credential working; do not rebuild the same shape one layer up.
    let ws = st
        .workspaces
        .get_by_slug(&ws_slug)
        .await
        .map_err(ServingError::internal)?
        .filter(|w| w.id == principal.workspace_id && w.status == "active")
        .ok_or(ServingError::NotFound)?;

    let (endpoint_id, version, snap) =
        live_snapshot(&st.db, ws.id, method.as_str(), &path)
            .await?
            .ok_or(ServingError::NotFound)?;
    // …steps 4-10 continue in Tasks 4 and 5.
}
```

- [ ] **Step 3: Mount it**

In `crates/egret-server/src/lib.rs`, beside the existing `dataapi` block at line 326. The management router keeps its CSRF layer; the serving router is a **separate group with none**, mounted at the root rather than under `/api`:

```rust
    if let Some(dataapi) = state.dataapi.clone() {
        let group = Router::new()
            .nest("/api", egret_dataapi::management_router())
            .layer(axum::middleware::from_fn(egret_auth::csrf::csrf_layer))
            .with_state(dataapi.clone());
        router = router.merge(group);

        // The serving surface, WITHOUT the CSRF layer and outside `/api` —
        // the same reasoning `render_router` carries (`egret-automation/src/
        // routes.rs:115-126`). Double-submit CSRF defends requests a browser
        // authenticates AMBIENTLY; nothing is ambient here, because the caller
        // sets an `Authorization` header itself. Requiring a CSRF cookie an
        // external client can never have would simply make every POST endpoint
        // impossible — which is exactly what shipping the renderer inside the
        // layered group did, and no router-level test saw it.
        let serving = Router::new()
            .nest("/data-api/v1", egret_dataapi::serving_router())
            .with_state(dataapi);
        router = router.merge(serving);
    }
```

Add one `doc.merge(...)` line for the serving OpenAPI, keeping the list explicit.

- [ ] **Step 4: Prove the middleware is not inherited — through the COMPOSED router**

This step has a trap that this repo has already fallen into. `render_router`'s own comment records it (`crates/egret-automation/src/routes.rs:123-126`): *"the integration tests call this router directly, so the layer they never mount rejected every POST in production."*

**A test that calls `serving_router()` directly cannot see a CSRF failure**, because it never mounts the layer. It would pass whether or not the group is layered, which makes it worthless for exactly this property.

The test must go through the composed application router. `AppState::for_tests` currently sets `dataapi: None` (`crates/egret-server/src/lib.rs:80`), so no test has ever built the composed router with this group merged at all — that gap is why this is the first chance to catch it.

```rust
#[tokio::test]
async fn a_post_to_a_live_endpoint_needs_no_csrf_cookie() {
    // Built through egret_server::router(), NOT serving_router() — a direct
    // call never mounts the CSRF layer and so cannot fail this.
    let app = composed_app_with_dataapi().await;
    let token = mint_data_api_token().await;

    let res = app
        .oneshot(
            Request::post("/data-api/v1/default/reports/daily")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(r#"{"params":{}}"#.into())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(res.status(), 403, "the serving group must not carry the CSRF layer");
    assert_ne!(body_text(res).await, "csrf check failed");
}
```

You will need to extend `AppState::for_tests` (or add a sibling) so `dataapi` can be populated. Do that rather than skipping the test — it is the only mechanism that can catch this class.

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

- [ ] **Step 2: Implement the cache key and the delegated re-check**

```rust
/// What a cached response is keyed by.
///
/// `principal` is present unless the snapshot says `shared` AND the delegated
/// re-check passed. Ranger filters per principal, so two identities can
/// legitimately get different rows from the same endpoint and the same params;
/// a key without the principal serves one identity's rows to another.
fn cache_key(
    endpoint_id: Uuid,
    version: i32,
    params: &BTreeMap<String, String>,
    principal: Option<UserId>,
) -> String {
    let mut h = Sha256::new();
    h.update(endpoint_id.as_bytes());
    h.update(version.to_le_bytes());
    // BTreeMap so the iteration order is the key's, not the request's.
    for (k, v) in params {
        h.update(k.as_bytes());
        h.update([0u8]);
        h.update(v.as_bytes());
        h.update([0u8]);
    }
    match principal {
        Some(p) => h.update(p.0.as_bytes()),
        // A literal marker rather than nothing: without it a shared entry and a
        // per-principal entry for the nil UUID would collide.
        None => h.update(b"shared"),
    }
    format!("{:x}", h.finalize())
}

/// Decide whether this request may use a shared cache entry.
///
/// `snapshot.delegated` froze what the connection was at publish time.
/// `query.databases.delegated` is mutable, so a version published as `shared`
/// against a non-delegated connection becomes a cross-principal leak the moment
/// someone flips that database. Validate-time checking cannot close this; the
/// frozen value is a witness, and a witness nobody questions is decoration.
async fn shared_cache_allowed(
    db: &Db,
    snap: &EndpointSnapshot,
) -> Result<bool, ServingError> {
    if snap.cache.scope != CacheScope::Shared {
        return Ok(false);
    }
    let live: bool = sqlx::query_scalar(
        "SELECT delegated FROM query.databases WHERE id = $1",
    )
    .bind(snap.database_id)
    .fetch_one(&db.pool)
    .await
    .map_err(ServingError::internal)?;

    // Disagreement is a refusal, not a downgrade to per-principal: the operator
    // published a configuration that is no longer safe, and silently serving it
    // a different way hides that from them.
    if live != snap.delegated {
        return Err(ServingError::CacheScopeStale);
    }
    Ok(!live)
}
```

`CacheScopeStale` maps to `503` with code `cache_scope_stale` — the endpoint is not permanently broken, its configuration has drifted, and the operator's fix is to re-publish. Add it to spec §7's table in both languages.

- [ ] **Step 3: Implement the rate limiter**

Model the shape on `crates/egret-auth/src/ratelimit.rs` — same injected-clock signature — but **not** its semantics. That one is login-style lockout (exceed the threshold and you are locked out for a fixed period); an API needs a bucket that refills, 429 + `Retry-After`, retryable immediately once it does.

```rust
/// Token bucket with an injected clock. Behind `RateLimitStore` so a
/// multi-replica deployment can swap a shared implementation in without
/// touching a call site; in-memory per process is correct while the serving
/// surface is in-process.
#[derive(Clone)]
pub struct TokenBucket {
    inner: Arc<Mutex<HashMap<String, Bucket>>>,
    qps: f64,
    burst: f64,
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

impl TokenBucket {
    /// Returns `Ok(())` or the `Duration` a caller should wait — which becomes
    /// `Retry-After`. Takes `now` so tests never sleep.
    pub fn check_at(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut map = self.inner.lock().unwrap();
        let b = map.entry(key.to_string()).or_insert(Bucket { tokens: self.burst, last: now });
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * self.qps).min(self.burst);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            // Time until one whole token exists.
            Err(Duration::from_secs_f64((1.0 - b.tokens) / self.qps))
        }
    }
}
```

The tests must use two genuinely separated `Instant`s. Two readings that can coincide make a passing test that proves nothing — construct `now` and `now + Duration::from_secs(n)` explicitly rather than calling `Instant::now()` twice.

Rate-limit on both keys the snapshot configures: `per_key_qps` keyed by the client, `per_endpoint_qps` keyed by the endpoint. A single combined key would let one client's burst exhaust another's allowance.

- [ ] **Step 4: Run, implement, run, mutate**

Three mutations, run separately:
1. Drop `principal` from `cache_key`. Expected: `two_principals_do_not_share_a_cached_response` FAILS.
2. Make `shared_cache_allowed` return `Ok(true)` without reading the live column. Expected: `a_shared_cache_is_refused_when_delegated_flipped_after_publish` FAILS.
3. Make `check_at` always return `Ok(())`. Expected: the rate-limit test FAILS.

Revert each. A gate whose removal leaves the suite green is not a gate.

- [ ] **Step 4: Commit**

---

### Task 5: Errors, access log, retention

> **SHIPPED — and the sketch below is superseded on three points.** Read spec §7
> and `serving/error.rs` for what was actually built, not this. The differences,
> each a decision recorded there:
>
> 1. **`ParamInvalid`, `ParamMissing`, `EngineUnavailable` and `Timeout` were not
>    added.** None is constructible on today's serving path, and a dead variant
>    makes a published code table look finished while nothing can produce the
>    code. Spec §7 now lists them separately, with the reason each is unreachable
>    — for 502/504 it is that `QueryExecPort` flattens both the engine's
>    infrastructure failures and egret's own into one `Error::Internal`.
> 2. **`SchemaDrift` and `OutputNamingCollision` had to be added to the table**
>    (Tasks 3 and 4 landed them after this plan was written), and all three 503s
>    carry `Retry-After: 300` as a floor rather than a forecast.
> 3. **Mutation 1 as written below does not fail.** `e.to_string()` on
>    `ServingError::Internal` is already the literal `"internal error"` —
>    thiserror redacts at the `Display` level — so the mutation that proves the
>    test is load-bearing is rendering the SOURCE, not the error. See the task
>    report.

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

Retention follows `AuditStore::prune(retention_days)` (`crates/egret-audit/src/lib.rs:209`), which is the established pattern here — a plain `DELETE … WHERE ts < now() - make_interval(days => $1)` returning the count. There is no partitioning precedent in this repo; do not introduce one without a measured reason.

```rust
/// Delete rows older than `retention_days`; returns the deleted count.
/// Mirrors `AuditStore::prune`, deliberately — an operator who knows one
/// retention knob should not have to learn a second shape for this one.
pub async fn prune(&self, retention_days: i64) -> Result<u64, sqlx::Error> {
    let res = sqlx::query(
        "DELETE FROM dataapi.access_log WHERE ts < now() - make_interval(days => $1)",
    )
    .bind(retention_days.max(0) as i32)
    .execute(&self.db.pool)
    .await?;
    Ok(res.rows_affected())
}
```

- [ ] **Step 3: The error mapper**

One function, exhaustive over `egret_core::Error`, so a new variant is a compile error rather than a silent 500 with an engine message in it.

```rust
/// Map an internal error to the wire. The caller sees a fixed code and a fixed
/// message; the detail goes to the log against `request_id`.
///
/// Engine errors carry table names, column names and sometimes hosts. On a
/// key-authenticated public surface that is a metadata leak, so nothing from
/// the engine reaches the body — including via `Display`.
fn to_wire(e: &ServingError, request_id: &str) -> (StatusCode, Json<ErrorBody>) {
    let (status, code, message) = match e {
        ServingError::Unauthorized => (401, "key_invalid", "authentication failed"),
        ServingError::NotFound => (404, "not_found", "no such endpoint"),
        ServingError::ParamInvalid(_) => (400, "param_invalid", "a parameter is invalid"),
        ServingError::ParamMissing(_) => (400, "param_missing", "a required parameter is missing"),
        ServingError::RowLimitExceeded { cap } =>
            (400, "row_limit_exceeded", &format!("result exceeded the {cap}-row cap")),
        ServingError::RateLimited { .. } => (429, "rate_limited", "too many requests"),
        // The author's SQL is fine and their policy denies the table. Saying
        // "invalid input" sends them to debug the wrong thing.
        ServingError::DataAccessDenied =>
            (403, "data_access_denied",
             "the service account is not granted access to this data"),
        ServingError::CacheScopeStale =>
            (503, "cache_scope_stale",
             "this endpoint's cache configuration is no longer valid; re-publish it"),
        ServingError::EngineUnavailable => (502, "engine_unavailable", "the query engine is unreachable"),
        ServingError::Timeout => (504, "timeout", "the query exceeded its time limit"),
        ServingError::Internal(_) => (500, "internal", "internal error"),
    };
    // The ONLY place the internal detail is allowed to appear.
    tracing::warn!(%request_id, error = ?e, code, "data-api request failed");
    (status, Json(ErrorBody { error: ErrorDetail { code, message, request_id } }))
}
```

`ParamInvalid(_)` and `ParamMissing(_)` deliberately do not interpolate their payload: the parameter *name* is the author's, not the caller's, and naming it tells a prober which parameters exist. If that turns out to hurt usability more than it helps, change it as a decision with a note — not by accident.

- [ ] **Step 4: Verify by mutation**

Two mutations, run separately:
1. Make the mapper pass the engine's message through (`e.to_string()` as `message`). Expected: `an_engine_error_never_reaches_the_caller_verbatim` FAILS naming the leaked table.
2. Map `DataAccessDenied` to `param_invalid`. Expected: `a_ranger_denial_is_not_reported_as_bad_sql` FAILS.

Revert each.

- [ ] **Step 4: Full scoped check and commit**

```bash
cargo fmt --all --check
cargo clippy -p egret-dataapi -p egret-dataapi-api -p egret-query -p egret-query-api -p egret-server --all-targets
cargo test -p egret-dataapi -p egret-dataapi-api -p egret-query -p egret-query-api -p egret-server
```

---

### Task 6: `data_api_endpoint:invoke` — the missing authorization step

**Files:**
- Modify: `crates/egret-core/src/rbac.rs`, `crates/egret-dataapi/src/routes.rs`, `crates/egret-dataapi/src/serving/router.rs`
- Create: `migrations/0124_dataapi_invoke_permission.sql`
- Test: `crates/egret-dataapi/tests/serving_authz.rs` (create)

**This task exists because the plan was wrong.** Spec §6.2's chain has ten steps and this plan's Tasks 1–5 implement nine of them. Step 4 — *Cedar: `data_api_endpoint:invoke` on `Object data_api_endpoint:{id}`* (spec §6.2 writes this as `dataapi:invoke`; the permission key in `rbac.rs` is `data_api_endpoint:invoke`, matching its `:read`/`:manage` siblings — reconcile the spec to the code, not the other way round) — had no owner, and the review of Task 3 confirmed the consequence by reading the handler:

> any valid, unexpired `data-api` token for workspace W reaches **every** live endpoint in W. A service account created for one consumer can invoke every other consumer's published endpoint in the same workspace and receive its full result set. Service-account granularity collapses to workspace granularity; the endpoint is not an access-control boundary at all.

The residual defence is thinner than it looks. The query does run as the service account, so **where Ranger is enforcing**, per-principal row filtering still applies. On a connection to an engine without Ranger, or on StarRocks where enforcement is resource-only, there is no second line and the token is a workspace-wide read capability over every published endpoint's SQL.

**And the grant it would check does not exist either.** Spec §5 says granting a service account access to an endpoint is one `access_grants` row with `scope_type='object'`, `object_type='data_api_endpoint'`. `grep` finds no route in P2 or P3 that writes such a row, and no `dataapi:invoke` in the permission catalog. So this task builds both halves: the way to grant, and the check that reads it.

- [ ] **Step 1: Add the permission and seed it**

In `crates/egret-core/src/rbac.rs`, beside the existing `data_api_endpoint:*` entries:

```rust
    perm!(
        "data_api_endpoint:invoke",
        "Data API",
        "Call a published Data API endpoint",
        Scope::Workspace
    ),
```

`migrations/0124_dataapi_invoke_permission.sql` seeds it onto `ws_admin`, mirroring `0122` **including its disclosure comment** — and add one sentence that this permission is the one intended to be granted *object-scoped to service accounts*, not held workspace-wide by humans, so a reader does not assume the role seed is the whole story.

- [ ] **Step 2: Write the failing tests**

```rust
#[tokio::test]
async fn a_token_cannot_invoke_an_endpoint_it_was_not_granted() {
    // The finding this task exists for: before it, ANY valid token for the
    // workspace reached EVERY live endpoint in it.
    let f = fixture().await;
    let mine = f.publish_endpoint("GET", "/mine", "SELECT 1 AS n").await;
    let theirs = f.publish_endpoint("GET", "/theirs", "SELECT 2 AS n").await;
    f.grant_invoke(f.service_account, mine).await;

    assert_eq!(f.call("/mine").await.status(), 200);
    assert_eq!(f.call("/theirs").await.status(), 403,
        "a grant on one endpoint must not reach another");
}

#[tokio::test]
async fn revoking_the_grant_stops_the_calls() {
    let f = fixture().await;
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1").await;
    f.grant_invoke(f.service_account, ep).await;
    assert_eq!(f.call("/x").await.status(), 200);

    f.revoke_invoke(f.service_account, ep).await;
    assert_eq!(f.call("/x").await.status(), 403,
        "unlike the token itself, a grant revocation IS immediate");
}

#[tokio::test]
async fn granting_requires_manage_not_read() {
    let f = fixture().await;
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1").await;
    let reader = f.principal_with(&["data_api_endpoint:read"]).await;
    let res = f.grant_as(reader, f.service_account, ep).await;
    assert_eq!(res.status(), 403, "handing out invoke rights is a manage operation");
}

#[tokio::test]
async fn a_grant_cannot_be_written_across_workspaces() {
    let f = fixture().await;
    let ep = f.publish_endpoint("GET", "/x", "SELECT 1").await;
    let other = f.create_workspace("other").await;
    let mgr = f.principal_with_in(&["data_api_endpoint:manage"], other).await;
    let res = f.grant_as_in(mgr, other, f.service_account, ep).await;
    assert_eq!(res.status(), 404, "the endpoint does not belong to that workspace");
}
```

Note what the second test pins: **grant revocation is immediate, while token revocation is not.** The 600s TTL bounds the credential; it does not bound the grant, because the grant is read per request. That asymmetry is worth stating in the spec — it is the answer to "how do I cut off one consumer right now".

- [ ] **Step 3: The management routes**

```text
GET    /api/ws/{workspace}/data-api/endpoints/{id}/grants
POST   /api/ws/{workspace}/data-api/endpoints/{id}/grants   { "service_account_id": … }
DELETE /api/ws/{workspace}/data-api/endpoints/{id}/grants/{service_account_id}
```

All gate on `data_api_endpoint:manage` — handing out invoke rights is a mutation of the authorization graph, not a read. Both ids are scoped: the endpoint through `EndpointStore`'s workspace predicate, the service account through P2's `scoped_account`. A row is `access_grants (principal_type='user', principal_id=<service account>, scope_type='object', object_type='data_api_endpoint', scope_id=<endpoint id>, role_id=<the role carrying data_api_endpoint:invoke>)`.

Read `crates/egret-auth/src/grants.rs` first: `grants_for` is the reader, and it already handles `scope_type='object'` with `object_type`. You are writing rows it can already read — do not invent a second path.

- [ ] **Step 4: The check in the serving handler**

In `serving/router.rs`, after the snapshot is resolved and **before** `execute_as`:

```rust
    // Step 4 of spec §6.2. Authentication told us WHO; this is the first thing
    // that asks WHETHER. Without it a token is a workspace-wide read capability
    // over every published endpoint — see this task's rationale.
    //
    // Denial is 403, not 404: by this point the caller has proved it holds a
    // valid credential for this workspace, so "this endpoint exists and you may
    // not call it" tells it nothing it could not already enumerate. That is the
    // opposite of the earlier steps, where 404 hides existence from a caller
    // who has proved nothing.
    if !st
        .pdp
        .authorize(
            &principal.subject(),
            "data_api_endpoint:invoke",
            &Resource::Object(ObjectRef { workspace_id: ws.id, kind: "data_api_endpoint", id: endpoint_id }),
        )
        .await
        .map_err(ServingError::internal)?
        .allow
    {
        // NOTE: `ServingError` has no `Forbidden` variant today (it carries
        // Unauthorized / NotFound / DataAccessDenied / RowLimitExceeded /
        // SchemaDrift / OutputNamingCollision / Internal). Add one — and keep it
        // distinct from `DataAccessDenied`, which means the ENGINE refused the
        // data. This one means egret refused the CALL, and the operator's fix is
        // a grant on the endpoint rather than a grant in Ranger.
        return Err(ServingError::NotGranted);
    }
```

Check `ObjectRef`'s real field names and `Pdp::authorize`'s real signature before writing this — `crates/egret-core/src/authz.rs:108` shows `require_object`'s shape, but that helper takes a `WorkspaceContext` and the serving path has a `ServingPrincipal`, so you are calling `authorize` directly. If constructing the subject from a `ServingPrincipal` is awkward, say so rather than fabricating a `WorkspaceContext`.

- [ ] **Step 5: Mutation-verify**

Remove the `authorize` call. Expected: `a_token_cannot_invoke_an_endpoint_it_was_not_granted` FAILS with 200 where it wanted 403 — i.e. the finding reproduces. Revert.

Then remove the workspace predicate from the grant-write path. Expected: `a_grant_cannot_be_written_across_workspaces` FAILS.

- [ ] **Step 6: Reconcile the spec**

§6.2 step 4 and §7's `403 forbidden` row now describe implemented behaviour — check both languages say so accurately. Add the grant/revoke routes to §5, and add the asymmetry from Step 2: a grant revocation takes effect on the next request, while a token revocation is bounded by the TTL. That is the operational answer to "cut off one consumer now", and it is currently nowhere.

- [ ] **Step 7: Commit**

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
