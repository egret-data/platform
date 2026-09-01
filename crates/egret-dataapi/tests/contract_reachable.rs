//! Package 8 — a consumer can fetch its own contract with the credential it
//! holds, and the document tells it what to branch on.
//!
//! Three groups of assertions, and they fail for different reasons:
//!
//! * **Reachability and scoping.** `GET /data-api/v1/{ws}` with a `data-api`
//!   bearer returns the OpenAPI document for the service account that token was
//!   issued for — and for that account ONLY. The scoping assertions below all
//!   run against a workspace with THREE live endpoints where the account under
//!   test is granted ONE, because a fixture with one endpoint, or one where the
//!   account happens to hold every grant, cannot tell "scoped correctly" from
//!   "returned everything".
//! * **The route is bounded.** Building the document costs one PDP decision per
//!   live endpoint, so it is the highest-amplification request on this surface
//!   and it has a bucket of its own — keyed on the service account, in its own
//!   namespace, always consulted. The assertions drive it past the limit rather
//!   than checking that a limiter was installed.
//! * **`error.code` carries an `enum`.** The field both documents describe as
//!   "the stable code to branch on" was a bare `string`, so a generated client
//!   gave a consumer nothing to branch on.
//!
//! DB-gated. Self-skips ONLY when `EGRET_ALLOW_DB_SKIP` is set; without it a run
//! with no `DATABASE_URL` FAILS, so check the DURATION, not just the `ok`.

mod serving_common;

use axum::http::StatusCode;
use egret_core::UserId;
use serving_common::{fixture, grant_invoke, mint, Fixture};
use std::collections::BTreeSet;
use uuid::Uuid;

macro_rules! skip_without_db {
    () => {
        match fixture().await {
            Some(f) => f,
            None => {
                eprintln!(
                    "SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set"
                );
                return;
            }
        }
    };
}

fn no_cache() -> serde_json::Value {
    serde_json::json!({ "enabled": false })
}

fn no_limits() -> serde_json::Value {
    serde_json::json!({})
}

/// A filter behind an `{% if %}`, so the parameter has a placeholder to
/// reference — publish refuses a parameter no statement uses.
fn optional_filter(param: &str) -> String {
    format!(
        "SELECT 1 AS n WHERE 1=1 {{% if {param} %}} AND region = {{{{ {param} }}}} {{% endif %}}"
    )
}

/// **The fixture every scoping assertion runs against.**
///
/// Three live endpoints, and the account under test is granted exactly one of
/// them:
///
/// | path      | parameter           | granted to `alpha` |
/// |-----------|---------------------|--------------------|
/// | `/probe`  | — (from `fixture()`)| no                 |
/// | `/alpha`  | `alpha_region`      | **yes**            |
/// | `/beta`   | `beta_region`       | no                 |
///
/// Every column of that table is load-bearing. Without `/beta` and `/probe`
/// there is nothing for an over-broad document to leak; without the distinct
/// PARAMETER names there is no way to tell "the path is absent" from "the path
/// is absent but its parameters are described somewhere else in the document";
/// and without a second granted account (`beta_account`, below) a document that
/// simply returned the FIRST endpoint would pass.
struct Scoped {
    alpha_account: Uuid,
    beta_account: Uuid,
    ungranted_account: Uuid,
    alpha: Uuid,
    beta: Uuid,
}

async fn scoped(f: &Fixture) -> Scoped {
    let alpha = f
        .publish_with_params(
            "/alpha",
            f.database_id,
            no_cache(),
            no_limits(),
            &optional_filter("alpha_region"),
            serde_json::json!([{ "name": "alpha_region", "type": "string", "location": "query" }]),
        )
        .await;
    let beta = f
        .publish_with_params(
            "/beta",
            f.database_id,
            no_cache(),
            no_limits(),
            &optional_filter("beta_region"),
            serde_json::json!([{ "name": "beta_region", "type": "string", "location": "query" }]),
        )
        .await;

    // REAL service accounts, through the real route, so the `users` row is the
    // kind a `data-api` token is minted for.
    let alpha_account = f.create_service_account("pkg8-alpha").await;
    let beta_account = f.create_service_account("pkg8-beta").await;
    let ungranted_account = f.create_service_account("pkg8-none").await;

    // Crossed on purpose: each account is granted the endpoint the OTHER one is
    // not. A document built from anything but the grant — the first row, every
    // row, the newest row — cannot satisfy both assertions at once.
    grant_invoke(&f.db, UserId(alpha_account), alpha).await;
    grant_invoke(&f.db, UserId(beta_account), beta).await;

    Scoped {
        alpha_account,
        beta_account,
        ungranted_account,
        alpha,
        beta,
    }
}

/// `account` still holds invoke on `endpoint`, asked of the real PDP.
///
/// Every "the contract is empty" assertion needs this beside it. An empty
/// `paths` object has two causes — the endpoint is not live, or the account may
/// not invoke it — and they are indistinguishable in the response. Without this
/// the state assertions would pass just as well against an implementation whose
/// authorization had silently broken, which is the "empty for the wrong reason"
/// hole.
async fn still_granted(f: &Fixture, account: UserId, endpoint: Uuid) {
    assert!(
        egret_dataapi::can_invoke_for_test(
            &f.state,
            account,
            egret_core::WorkspaceId(f.ws_id),
            endpoint,
        )
        .await
        .expect("pdp"),
        "the grant on {endpoint} must still be in place, or the emptiness this \
         asserts is about authorization rather than about state"
    );
}

// ---------------------------------------------------------------------------
// Half (a) — reachability
// ---------------------------------------------------------------------------

/// The gap, stated as a test: the credential a consumer HAS fetches the document
/// it needs.
///
/// Mutation: delete the `.route("/{ws}", …)` line in
/// `serving_router_sharing_bulkhead` and this is a 405/404 instead of a
/// document.
#[tokio::test]
async fn a_bearer_fetches_its_own_contract() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    let res = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.body["openapi"], "3.0.3", "{}", res.body);
    assert_eq!(res.paths(), vec!["/alpha".to_string()], "{}", res.body);
}

/// A consumer that follows `servers[0].url` arrives back at this route.
///
/// That is the whole of the discovery story — the document names its own
/// address, so nothing has to be written down anywhere else — and it is also
/// what makes the reserved-URL argument in `serving::contract` true. Composed
/// exactly as `openapi_gen`'s `the_documented_url_is_the_url_try_it_calls`
/// composes the two halves of a call.
#[tokio::test]
async fn the_contract_url_is_the_documents_own_server_url() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    let res = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let server = res.body["servers"][0]["url"].as_str().expect("servers url");
    assert_eq!(
        server,
        format!("https://egret.test/data-api/v1/{}", f.ws_slug),
        "the document must name the URL it was fetched from: {}",
        res.body
    );
}

/// The reserved URL cannot be taken by an author, and nothing BELOW the slug was
/// reserved to achieve that.
///
/// Two halves, and the second is the one that would catch a well-meant "let's
/// move it to `/{ws}/openapi.json`" refactor:
///
/// 1. `normalize_path` refuses an empty path, so every published URL has at
///    least one segment after the slug and none of them can ever be `/{ws}`
///    itself.
/// 2. An endpoint published AT `/openapi.json` still serves its rows. axum
///    prefers a static segment to a wildcard, so a literal reserved anywhere
///    below the slug would shadow this endpoint and answer with a document
///    instead — silently, to the author and to the caller alike.
#[tokio::test]
async fn a_published_endpoint_cannot_be_shadowed_by_the_contract_url() {
    let f = skip_without_db!();

    for empty in ["", "/", "  ", "//"] {
        assert!(
            egret_dataapi::endpoints::normalize_path(empty).is_err(),
            "`{empty:?}` must not normalise to a publishable path, or an endpoint \
             could claim the contract's own URL"
        );
    }

    // The spelling a reserved literal would most plausibly have used.
    f.publish("/openapi.json", f.database_id, no_cache(), no_limits())
        .await;
    let res = f.call_as(f.principal_a, "openapi.json").await;
    assert_eq!(
        res.status,
        StatusCode::OK,
        "an endpoint at /openapi.json must still serve its own rows: {}",
        res.body
    );
    assert!(
        res.body["data"].is_array(),
        "shadowed by a document instead of serving rows: {}",
        res.body
    );
}

// ---------------------------------------------------------------------------
// Half (a) — the scoping property
// ---------------------------------------------------------------------------

/// **The property that makes this route safe.** The document describes exactly
/// what this account may invoke and nothing else.
///
/// Three live endpoints, one grant. The assertion is set-EQUALITY on `paths`
/// plus a substring sweep of the whole serialised document for the other two
/// endpoints' paths and parameter names — because "the path is missing from
/// `paths`" and "no fact about that endpoint appears anywhere" are different
/// claims, and it is the second one that makes an endpoint's parameter names
/// non-reconnaissance.
///
/// Mutation: drop the `can_invoke` guard in `openapi_gen::build` (or pass a
/// `ws_admin` in place of `principal.user_id`) and `paths` becomes all three.
#[tokio::test]
async fn the_contract_describes_only_what_this_account_may_invoke() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    let res = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(
        res.paths(),
        vec!["/alpha".to_string()],
        "one grant, three live endpoints: {}",
        res.body
    );

    let doc = res.body.to_string();
    // What the account MAY call is described in full — a contract that named no
    // parameters would satisfy every "does not leak" assertion below and be
    // useless.
    assert!(
        doc.contains("alpha_region"),
        "the granted endpoint's own parameter must be described: {doc}"
    );
    for leaked in ["/beta", "beta_region", "/probe"] {
        assert!(
            !doc.contains(leaked),
            "`{leaked}` belongs to an endpoint this account may not invoke, and \
             appears in its contract: {doc}"
        );
    }
}

/// The mirror, and the reason the fixture grants the two accounts opposite
/// endpoints.
///
/// Run alone, the test above cannot distinguish "the document follows the
/// grant" from "the document returns the alphabetically first endpoint", or
/// "…the one published first". This one is the same workspace, the same three
/// endpoints, the other grant — and it must come back with the other path.
#[tokio::test]
async fn a_second_account_gets_its_own_contract_and_not_the_first_ones() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    let alpha = f.contract_as(UserId(s.alpha_account)).await;
    let beta = f.contract_as(UserId(s.beta_account)).await;
    assert_eq!(alpha.status, StatusCode::OK, "{}", alpha.body);
    assert_eq!(beta.status, StatusCode::OK, "{}", beta.body);

    assert_eq!(alpha.paths(), vec!["/alpha".to_string()], "{}", alpha.body);
    assert_eq!(beta.paths(), vec!["/beta".to_string()], "{}", beta.body);
    assert!(
        !beta.body.to_string().contains("alpha_region"),
        "the second account's contract carries the first's parameter: {}",
        beta.body
    );
}

/// An account with no grants gets a valid, EMPTY document — not a 403, and not
/// somebody else's contract.
///
/// 200 rather than 403 because the question ("what may I call") was answered;
/// the answer is "nothing". A 403 would be a refusal to answer, which is not
/// what happened, and would leave a consumer unable to tell it apart from a bad
/// token.
#[tokio::test]
async fn an_account_with_no_grants_gets_an_empty_document() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    let res = f.contract_as(UserId(s.ungranted_account)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert!(
        res.paths().is_empty(),
        "an ungranted account must be described nothing: {}",
        res.body
    );
    // Still a usable document: the origin and the security scheme are there, so
    // a client generated from it fails at the CALL with `not_granted` rather
    // than failing to generate at all.
    assert_eq!(res.body["openapi"], "3.0.3", "{}", res.body);
    assert!(res.body["servers"][0]["url"].is_string(), "{}", res.body);
}

/// A grant revoked between two fetches is gone from the second document.
///
/// This is what "the same source of truth as the serving path" buys, and it is
/// also the argument against caching this response: a cached document would
/// keep describing an endpoint that now answers 403 for the whole of its TTL.
#[tokio::test]
async fn revoking_a_grant_removes_the_path_from_the_next_contract() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    let before = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(
        before.paths(),
        vec!["/alpha".to_string()],
        "{}",
        before.body
    );

    f.revoke_invoke(UserId(s.alpha_account), s.alpha).await;

    let after = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(after.status, StatusCode::OK, "{}", after.body);
    assert!(
        after.paths().is_empty(),
        "a revoked grant must disappear from the contract at once: {}",
        after.body
    );
}

/// Not cacheable, and it says so on the wire.
///
/// The URL is identical for every service account in the workspace and the
/// TOKEN is what tells them apart, so anything caching by URL — a shared proxy,
/// a CDN, a client library's default — would hand one consumer another's
/// contract. `no-store` rather than `no-cache`, because a stored copy is the
/// problem, not a revalidated one.
///
/// Mutation: delete the header tuple in `serve_contract` and this fails.
#[tokio::test]
async fn the_contract_is_never_stored() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    let res = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(
        res.cache_control.as_deref(),
        Some("no-store"),
        "a per-service-account document must never be stored"
    );
}

// ---------------------------------------------------------------------------
// Half (a) — the checks a document must not be exempt from
// ---------------------------------------------------------------------------

/// No credential, no contract — and the refusal is the serving surface's own
/// envelope, not axum's.
#[tokio::test]
async fn an_unauthenticated_fetch_is_key_invalid() {
    let f = skip_without_db!();
    let _ = scoped(&f).await;

    for authorization in ["", "Bearer not-a-jwt", "Basic abc"] {
        let res = f.contract_raw(&f.ws_slug, authorization).await;
        assert_eq!(
            res.status,
            StatusCode::UNAUTHORIZED,
            "`{authorization}`: {}",
            res.body
        );
        assert_eq!(res.code(), "key_invalid", "`{authorization}`");
    }
}

/// **A token for another workspace cannot fetch this one's contract.**
///
/// The token is valid, signed by the same key, for a real service account that
/// really does hold invoke grants — in a DIFFERENT workspace. The only thing
/// wrong with it is the `ws` claim, which is exactly the check under test.
///
/// Mutation: drop `.filter(|w| w.id.0 == principal.workspace_id …)` from
/// `build_contract` and this returns 200 with `/probe` in it — `principal_a`
/// holds invoke on the fixture's own endpoint, so the mutant leaks a real
/// document across a workspace boundary rather than an empty one.
#[tokio::test]
async fn a_token_for_another_workspace_is_not_found() {
    let f = skip_without_db!();
    let other_ws = f.make_workspace("pkg8-other").await;

    let token = mint(other_ws, f.principal_a, "client-cross-ws");
    let res = f.contract_raw(&f.ws_slug, &format!("Bearer {token}")).await;

    assert_eq!(res.status, StatusCode::NOT_FOUND, "{}", res.body);
    assert_eq!(res.code(), "not_found");
    assert!(
        res.body["paths"].is_null(),
        "a refusal must not carry a document: {}",
        res.body
    );
}

/// An archived workspace stops handing out contracts, even to a token minted
/// while it was active.
///
/// `ClientStore::verify` refuses to MINT a token for an archived workspace and
/// cannot recall one already issued; `router::run` closes that window on the
/// call path with one comparison, and a document must not be the way around it.
///
/// Mutation: drop `&& w.status == "active"` and this returns the document.
#[tokio::test]
async fn an_archived_workspace_answers_no_contract() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    // Active first, so the failure below cannot be "this account never had a
    // contract in the first place".
    let before = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(before.status, StatusCode::OK, "{}", before.body);

    sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
        .bind(f.ws_id)
        .execute(&f.db.pool)
        .await
        .expect("archive workspace");

    let after = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(after.status, StatusCode::NOT_FOUND, "{}", after.body);
    assert_eq!(after.code(), "not_found");
}

/// A draft, a disabled endpoint and a trashed one are not contract material
/// even for an account that holds invoke on them.
///
/// `build`'s `state = 'live'` predicate is what does it, and the reason it
/// matters here is the same reason it matters there: a documented path that
/// answers 404 sends a consumer to debug their own client.
///
/// **All three states, because they are excluded by different things** — the
/// docstring claimed all three before the test drove only `disable`, and the
/// three are not one predicate:
///
/// * a **disabled** endpoint keeps its `live_version_id` (`disable` only writes
///   `state`), so `e.state = 'live'` is the whole of what excludes it;
/// * a **draft** has never been published, so it is excluded by `build`'s
///   `JOIN … ON ev.id = e.live_version_id` — a different clause, which would
///   still hold if the state predicate were dropped and vice versa;
/// * a **trashed** endpoint is excluded by neither clause directly. It is
///   excluded because `trash` refuses a LIVE endpoint (its UPDATE's own `WHERE`,
///   with migration 0126's `dataapi_endpoints_trashed_not_live` under it), so a
///   trashed row can never be `state = 'live'`. `build` has no `deleted_at`
///   predicate at all. That is a real dependency between two files and it is
///   what this case pins; the assertion on the row's state below is there to
///   name it rather than let it be inferred.
///
/// Every case re-asserts that the grant is still in place. Without that, an
/// empty document is ambiguous: "not live" and "no longer authorized" produce
/// exactly the same `paths: {}`, and a mutation to the grant path would pass
/// here while the state predicate was gone.
#[tokio::test]
async fn a_draft_a_disabled_and_a_trashed_endpoint_all_leave_the_contract() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    let account = UserId(s.alpha_account);

    // 1. A DRAFT the account may invoke. Never published, so there is no frozen
    //    version to describe — and `/alpha` is still there, which is what makes
    //    the assertion about the draft rather than about an empty document.
    let draft = f.create_draft_endpoint("pkg8-draft", "/pkg8-draft").await;
    grant_invoke(&f.db, UserId(s.alpha_account), draft).await;
    still_granted(&f, account, draft).await;

    let res = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(
        res.paths(),
        vec!["/alpha".to_string()],
        "a draft has no published version to describe: {}",
        res.body
    );

    // 2. A TRASHED endpoint the account may invoke. Trash keeps the row, the
    //    versions and the grants — the doc on `trash_endpoint` says so — so this
    //    is a state predicate and not an absence of data.
    let trashed = f
        .create_draft_endpoint("pkg8-trashed", "/pkg8-trashed")
        .await;
    grant_invoke(&f.db, UserId(s.alpha_account), trashed).await;
    let (status, body) = f
        .mgmt_post(
            &format!("/api/ws/{}/data-api/endpoints/{}/trash", f.ws_slug, trashed),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "trash failed: {body}");
    still_granted(&f, account, trashed).await;
    // The dependency, named. `build` has no `deleted_at` predicate; what keeps a
    // trashed endpoint out of the document is that it cannot be `live`.
    let state: String = sqlx::query_scalar(
        "SELECT state FROM dataapi.endpoints WHERE id = $1 AND deleted_at IS NOT NULL",
    )
    .bind(trashed)
    .fetch_one(&f.db.pool)
    .await
    .expect("the trashed row is there and really is trashed");
    assert_ne!(
        state, "live",
        "trash must leave the endpoint non-live — that, and not a `deleted_at` \
         predicate in `build`, is what keeps it out of the contract"
    );

    let res = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(
        res.paths(),
        vec!["/alpha".to_string()],
        "a trashed endpoint keeps its grants and its versions, and must still \
         leave the contract: {}",
        res.body
    );

    // 3. DISABLED — the one that was live and stopped being so, which is the
    //    case a consumer is most likely to meet.
    let (status, body) = f
        .mgmt_post(
            &format!(
                "/api/ws/{}/data-api/endpoints/{}/disable",
                f.ws_slug, s.alpha
            ),
            Some(serde_json::json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "disable failed: {body}");
    still_granted(&f, account, s.alpha).await;

    let res = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert!(
        res.paths().is_empty(),
        "a disabled endpoint answers 404; documenting it sends a consumer to \
         debug their own client: {}",
        res.body
    );
    let _ = s.beta;
}

// ---------------------------------------------------------------------------
// Half (a) — the bucket the contract route has of its own
// ---------------------------------------------------------------------------

/// **The most expensive request on this surface is bounded, and by a real
/// allowance rather than a layer that was installed.**
///
/// The clock is frozen (`TestClock`), so nothing refills between requests and
/// every admission below is a token that was genuinely there — the property a
/// wall-clock version of this test would lose the moment the machine was slow.
///
/// Driven past the limit rather than asserted against the constant: the loop
/// spends `CONTRACT_BURST` tokens and the next fetch is refused. Then the clock
/// advances and the same account is admitted again, which is the difference
/// between a limiter and a lockout.
///
/// Mutation: delete the `contract_limit(st, &principal).await?;` line in
/// `build_contract` and every request below is a 200.
#[tokio::test]
async fn a_contract_loop_is_rate_limited_and_refills() {
    let f = skip_without_db!();
    let s = scoped(&f).await;
    let account = UserId(s.alpha_account);

    let burst = egret_dataapi::serving::ratelimit::CONTRACT_BURST;
    for i in 0..burst {
        let res = f.contract_as(account).await;
        assert_eq!(
            res.status,
            StatusCode::OK,
            "request {i} of a burst of {burst} must be admitted: {}",
            res.body
        );
    }

    let refused = f.contract_as(account).await;
    assert_eq!(
        refused.status,
        StatusCode::TOO_MANY_REQUESTS,
        "a burst of {burst} is spent; the next fetch must be refused. Building \
         this document costs one PDP decision per live endpoint, and it was the \
         only unlimited request on the surface: {}",
        refused.body
    );
    assert_eq!(refused.code(), "rate_limited", "{}", refused.body);
    assert!(
        refused.body["paths"].is_null(),
        "a refusal must not carry a document: {}",
        refused.body
    );
    // A 429 with no `Retry-After` tells a consumer to guess, and guessing is a
    // retry storm.
    assert_eq!(
        refused.retry_after.as_deref(),
        Some("1"),
        "the refusal must say when the allowance refills"
    );

    // The refill. One second at `CONTRACT_QPS` is one token, and one token is
    // one document.
    f.clock.advance(std::time::Duration::from_secs(1));
    let after = f.contract_as(account).await;
    assert_eq!(
        after.status,
        StatusCode::OK,
        "the bucket must refill — a limit that never lets the caller back in is \
         a lockout: {}",
        after.body
    );
}

/// **The contract bucket cannot starve a call, and cannot starve another
/// account.**
///
/// The two ways a new bucket goes wrong, in one test because they are the same
/// mistake seen from two sides: a key that is too coarse. Reusing the endpoint's
/// bucket would make a discovery loop refuse paid traffic; keying on the
/// workspace alone would make one consumer's loop deny every other consumer its
/// contract.
///
/// The call half is driven against an endpoint whose published allowance is a
/// burst of **one**, so a single stolen token is the difference between 200 and
/// 429. Driving it against `/probe` — which publishes `{}` and therefore never
/// touches the store at all — would have passed under every implementation,
/// including one that spent the endpoint's tokens.
///
/// Mutations: drop the service-account half of `contract_key` and the second
/// account is refused; key the contract on the endpoint-wide bucket under a
/// synthetic id (one of the shapes weighed for this) and the second account is
/// refused too, because every account in the workspace then shares one bucket.
/// The namespace claim itself — that `d:` can never be spelled as `c:` or `e:` —
/// is pinned by `ratelimit`'s `the_contract_bucket_is_its_own_namespace`, which
/// is where a key-format assertion belongs.
#[tokio::test]
async fn one_accounts_contract_loop_starves_neither_a_call_nor_another_account() {
    let f = skip_without_db!();
    let s = scoped(&f).await;
    let account = UserId(s.alpha_account);

    // A published allowance of exactly one request. Anything the contract loop
    // takes from it is visible immediately.
    let limited = f
        .publish(
            "/pkg8-limited",
            f.database_id,
            no_cache(),
            serde_json::json!({ "per_key_qps": 1, "burst": 1, "per_endpoint_qps": 1 }),
        )
        .await;
    grant_invoke(&f.db, f.principal_a, limited).await;

    // Spend the whole allowance, then confirm it really is spent — otherwise
    // both assertions below would pass against a route with no limiter at all.
    for _ in 0..egret_dataapi::serving::ratelimit::CONTRACT_BURST {
        assert_eq!(f.contract_as(account).await.status, StatusCode::OK);
    }
    assert_eq!(
        f.contract_as(account).await.status,
        StatusCode::TOO_MANY_REQUESTS,
        "the premise of this test is an exhausted contract bucket"
    );

    // A CALL to the endpoint with the one-request allowance. Both of its buckets
    // — the per-client one and the endpoint-wide one — must be untouched.
    let call = f.call_as(f.principal_a, "pkg8-limited").await;
    assert_eq!(
        call.status,
        StatusCode::OK,
        "a contract fetch must never spend a token an endpoint call then needs; \
         this endpoint's published burst is 1: {}",
        call.body
    );

    // And another service account's contract is untouched.
    let other = f.contract_as(UserId(s.beta_account)).await;
    assert_eq!(
        other.status,
        StatusCode::OK,
        "one account's discovery loop must not deny another its contract: {}",
        other.body
    );
    assert_eq!(other.paths(), vec!["/beta".to_string()], "{}", other.body);
}

/// **A store outage does not turn this route into an unlimited one.**
///
/// The route's rate is a constant rather than an author's published number, so
/// unlike a call it ALWAYS consults the store — which is exactly why the outage
/// answer has to be a refusal. `503 rate_limiter_unavailable`, the same code and
/// the same reasoning the call path uses: an admission here would mean an
/// uncounted, unbounded allowance under the one condition nobody is watching.
///
/// The router is built here rather than taken from the fixture because the
/// substitution IS the subject: only a store that fails can produce
/// `Refusal::Unavailable`, and the in-process bucket has no unreachable state.
///
/// Mutation: map `Refusal::Unavailable` to `Ok(())` in `contract_limit` and this
/// is a 200.
#[tokio::test]
async fn an_unreachable_limiter_refuses_the_contract_rather_than_admitting_it() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    struct Unreachable;

    #[async_trait::async_trait]
    impl egret_dataapi::serving::RateLimitStore for Unreachable {
        async fn check_at(
            &self,
            _key: &str,
            _rate: egret_dataapi::serving::Rate,
            _now: std::time::Instant,
        ) -> Result<(), egret_dataapi::serving::Refusal> {
            Err(egret_dataapi::serving::Refusal::Unavailable)
        }
    }

    let mut state = f.state.clone();
    state.rate_limits = std::sync::Arc::new(Unreachable);
    let router = axum::Router::new()
        .nest("/data-api/v1", egret_dataapi::serving_router())
        .with_state(state);

    let token = mint(f.ws_id, UserId(s.alpha_account), "client-outage");
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/data-api/v1/{}", f.ws_slug))
        .header("authorization", format!("Bearer {token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    let res =
        serving_common::contract_res(tower::ServiceExt::oneshot(router, request).await.unwrap())
            .await;

    assert_eq!(
        res.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "with the limiter unreachable the contract must be refused, not served \
         without an allowance: {}",
        res.body
    );
    assert_eq!(res.code(), "rate_limiter_unavailable", "{}", res.body);
    assert!(
        res.body["paths"].is_null(),
        "a refusal must not carry a document: {}",
        res.body
    );
}

// ---------------------------------------------------------------------------
// Half (b) — `error.code` has an `enum`
// ---------------------------------------------------------------------------

/// Spec §7's code table, written out.
///
/// **Deliberately a second hand-written copy** of what
/// `serving::error`'s `the_code_table_is_spec_section_7` states, and the
/// duplication is the point: the assertions below compare the published `enum`
/// against BOTH this list and `wire_answers()`. Comparing only against
/// `wire_answers()` would be a tautology — the `enum` is built from it — and
/// would pass with `wire_codes` returning any subset of itself, which is the
/// mutation most likely to happen. A hand list in a TEST is where an expectation
/// belongs; the thing this package exists to prevent is a hand list in the
/// PRODUCT.
///
/// A new `ServingError` variant fails here, and that is correct: a new code a
/// consumer can receive is a change to the published contract, and somebody
/// should have to write it down.
fn spec_section_7_codes() -> BTreeSet<&'static str> {
    BTreeSet::from([
        "cache_scope_stale",
        "data_access_denied",
        "deadline_exceeded",
        "endpoint_query_failed",
        "engine_unavailable",
        "governance_stale",
        "internal",
        "key_invalid",
        "not_found",
        "not_granted",
        "output_naming_collision",
        "overloaded",
        "param_gate_stale",
        "param_invalid",
        "param_missing",
        "payload_too_large",
        "rate_limited",
        "rate_limiter_unavailable",
        "row_limit_exceeded",
        "schema_drift",
        "timeout",
        "version_pin_invalid",
        "version_unavailable",
    ])
}

fn code_enum_of(schema: &serde_json::Value) -> BTreeSet<String> {
    schema["properties"]["code"]["enum"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("`error.code` must declare an enum; got {schema}");
        })
        .iter()
        .map(|v| {
            v.as_str()
                .unwrap_or_else(|| panic!("every enum value must be a string; got {v}"))
                .to_string()
        })
        .collect()
}

/// The document a consumer generates a client from names every code it can
/// receive.
///
/// Before this, `code` was `{"type": "string"}` with one `example`, under a
/// description reading "the stable code to branch on" — so the generated client
/// typed it `string` and a consumer's `switch` had no cases and no compiler
/// help against a typo.
///
/// Mutation: delete the `"enum"` line in `openapi_gen`, or make `wire_codes`
/// return a subset (`.filter(|c| c.starts_with('p'))`), and this fails naming
/// the difference.
#[tokio::test]
async fn the_generated_document_names_every_code_a_consumer_can_receive() {
    let f = skip_without_db!();
    let s = scoped(&f).await;

    let res = f.contract_as(UserId(s.alpha_account)).await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);

    let published = code_enum_of(&res.body["components"]["schemas"]["ServingErrorDetail"]);
    let expected: BTreeSet<String> = spec_section_7_codes()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        published, expected,
        "the generated document's `error.code` enum is not spec §7's code table"
    );

    // …and it is DERIVED from the taxonomy rather than typed out beside it, so
    // it cannot go stale the way this surface's status lists did twice.
    let derived: BTreeSet<String> = egret_dataapi::serving::error::wire_answers()
        .into_values()
        .flatten()
        .map(str::to_string)
        .collect();
    assert_eq!(
        published, derived,
        "the enum must come from `to_wire` applied to the variant census"
    );
}

/// The same claim about the OTHER document this surface publishes.
///
/// Both documents carried a hand-maintained status list, and both went stale
/// when Package 3b made `502 engine_unavailable` and `504 timeout` live — one
/// was corrected and the other was not, because the byte gate over the root
/// document compares that file to ITSELF and can never notice that it is
/// untrue. Two documents, one derivation, one test each.
///
/// No fixture and no database: `serving_openapi()` is a pure function of the
/// compiled routes.
#[test]
fn the_published_root_document_names_every_code_too() {
    let doc = serde_json::to_value(egret_dataapi::serving_openapi()).expect("serialise");
    let published = code_enum_of(&doc["components"]["schemas"]["ServingErrorDetail"]);
    let expected: BTreeSet<String> = spec_section_7_codes()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(
        published, expected,
        "the root serving document's `error.code` enum is not spec §7's code table"
    );

    // The four codes that were live and undocumented when the last hand-written
    // list went stale, named individually — a set comparison that both sides got
    // wrong the same way would still pass, and these are the four that history
    // says get missed.
    for code in [
        "engine_unavailable",
        "timeout",
        "deadline_exceeded",
        "endpoint_query_failed",
    ] {
        assert!(
            published.contains(code),
            "`{code}` is live on this surface and missing from the published enum"
        );
    }
}

/// The bearer-reachable route is in the published document, with the same
/// security scheme the call operations use.
///
/// A route a consumer cannot discover is only half-reachable; and a `security`
/// requirement naming a scheme the document never defines reads as documented
/// auth while telling a generator nothing, which is why `BearerAddon` exists.
#[test]
fn the_contract_route_is_itself_published() {
    let doc = serde_json::to_value(egret_dataapi::serving_openapi()).expect("serialise");
    let op = &doc["paths"]["/data-api/v1/{ws}"]["get"];
    assert!(
        op.is_object(),
        "the contract route must be documented: {doc}"
    );
    assert_eq!(
        op["security"][0]["data_api_bearer"],
        serde_json::json!([]),
        "the contract route must require the same bearer the calls do: {op}"
    );
    assert!(
        doc["components"]["securitySchemes"]["data_api_bearer"].is_object(),
        "the scheme it names must be defined: {doc}"
    );
    // The refusals it can actually give, so a generated client has a case for
    // each. 503 is `output_naming_collision`, which only this route's own
    // failure mode produces here — and `rate_limiter_unavailable`, which it can
    // now produce because it consults a limiter. 429 is `rate_limited`: this
    // route has a bucket of its own (see `serving::contract`), and a status a
    // consumer will really meet and that the document did not name would be the
    // same defect `the_published_contract_names_every_answer_the_surface_can_give`
    // exists for, one operation over.
    for status in ["200", "401", "404", "429", "500", "503"] {
        assert!(
            op["responses"][status].is_object(),
            "the contract route must document {status}: {op}"
        );
    }
}
