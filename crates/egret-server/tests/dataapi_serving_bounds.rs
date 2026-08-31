//! The three request-scoped bounds on the serving surface, asserted through the
//! **composed** application router and with the **production** constants.
//!
//! # What this file refuses to do, and why
//!
//! Two shapes were available and both are the failure this repo keeps shipping:
//!
//! * a timeout test that passes because the operation was fast anyway — it never
//!   crosses the bound, so it is green with the layer deleted;
//! * a bulkhead test that asserts the layer is *installed* — green with the
//!   semaphore set to `usize::MAX`.
//!
//! So every test here drives something that genuinely exceeds its bound. The
//! instrument that makes that affordable is [`SlowKeys`]: `serve`'s very first
//! step is `verify_serving_token`, which awaits `SigningKeyStore::active()`. A
//! store that parks there holds a request **inside the handler**, in flight,
//! with no database and no engine — which is exactly the state a deadline and a
//! bulkhead exist to bound.
//!
//! `#[tokio::test(start_paused = true)]` then lets the 50-second production
//! deadline be crossed in milliseconds: with every task parked, tokio advances
//! its clock to the next timer, which is the deadline itself. The bound crossed
//! is the real `SERVING_REQUEST_DEADLINE`, not a test-sized substitute.
//!
//! # Why the composed router and not `serving_router()`
//!
//! Same reasoning as `dataapi_serving_mount.rs`: the property is that a request
//! arriving at the deployed service is bounded, and that spans `build_router`'s
//! mount, `serving_router`'s layers and the constants they are given. A test
//! against a hand-layered router proves only that `tokio::time::timeout` works.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_core::authz::{Decision, Pdp, Resource, Subject};
use egret_dataapi::serving::{SERVING_MAX_BODY_BYTES, SERVING_MAX_CONCURRENCY};
use egret_dataapi::{ClientStore, DataApiState, EndpointStore};
use egret_machine_idp::{generate_rsa_key, IdpError, KeyMaterial, SigningKeyStore};
use egret_metastore::Db;
use egret_server::{build_router, AppState};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tower::ServiceExt;
use uuid::Uuid;

struct DenyPdp;

#[async_trait::async_trait]
impl Pdp for DenyPdp {
    async fn authorize(
        &self,
        _s: &Subject,
        _a: &str,
        _r: &Resource,
    ) -> egret_core::Result<Decision> {
        Ok(Decision {
            allow: false,
            reasons: vec![],
        })
    }
}

struct UnreachablePort;

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for UnreachablePort {
    async fn describe_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: egret_core::UserId,
        _sql: &str,
    ) -> Result<Vec<egret_core::ColumnMeta>, egret_core::Error> {
        unreachable!("no request in this file gets past the key store")
    }
    async fn execute_as(
        &self,
        _ws: Uuid,
        _database_id: Uuid,
        _principal: egret_core::UserId,
        _sql: &str,
        _limits: egret_query_api::Limits,
    ) -> Result<egret_core::TabularResult, egret_core::Error> {
        unreachable!("no request in this file gets past the key store")
    }
}

fn active_key() -> &'static KeyMaterial {
    static KEY: OnceLock<KeyMaterial> = OnceLock::new();
    KEY.get_or_init(|| generate_rsa_key("egret-active").expect("generate key"))
}

/// A signing-key store that answers immediately — for the tests whose bound is
/// not about time.
struct FastKeys;

#[async_trait::async_trait]
impl SigningKeyStore for FastKeys {
    async fn active(&self) -> Result<KeyMaterial, IdpError> {
        let km = active_key();
        Ok(KeyMaterial {
            kid: km.kid.clone(),
            private_pem: km.private_pem.clone(),
            public_jwk: km.public_jwk.clone(),
        })
    }
    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
        Ok(vec![active_key().public_jwk.clone()])
    }
}

/// A signing-key store that takes a fixed amount of time, so a test can drive a
/// request whose duration it chose against a deadline it did not.
///
/// Used with `#[tokio::test(start_paused = true)]`: the sleep is virtual, so
/// "this endpoint takes 60 seconds" costs nothing to run.
struct SleepyKeys(Duration);

#[async_trait::async_trait]
impl SigningKeyStore for SleepyKeys {
    async fn active(&self) -> Result<KeyMaterial, IdpError> {
        tokio::time::sleep(self.0).await;
        let km = active_key();
        Ok(KeyMaterial {
            kid: km.kid.clone(),
            private_pem: km.private_pem.clone(),
            public_jwk: km.public_jwk.clone(),
        })
    }
    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
        unreachable!("not reached on the serving path")
    }
}

/// A signing-key store that **parks**, holding the request inside the handler.
///
/// `entered` counts how many requests have reached this point, so a test can
/// wait for a precise number of genuinely in-flight requests rather than
/// sleeping and hoping. `release` un-parks them all, after which each request
/// finishes normally — the key is real, the token is not, so the handler
/// answers its own `401`. Erroring instead would answer `500` and make the
/// released requests indistinguishable from an egret fault.
struct SlowKeys {
    entered: Arc<AtomicUsize>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl SigningKeyStore for SlowKeys {
    async fn active(&self) -> Result<KeyMaterial, IdpError> {
        // Registered BEFORE the counter is bumped. The other order has a real
        // race: a waiter counted but not yet subscribed would miss
        // `notify_waiters` and hang the test — an intermittent failure that
        // would read as a bug in the bulkhead.
        let parked = self.release.notified();
        tokio::pin!(parked);
        parked.as_mut().enable();
        self.entered.fetch_add(1, Ordering::SeqCst);
        parked.await;
        let km = active_key();
        Ok(KeyMaterial {
            kid: km.kid.clone(),
            private_pem: km.private_pem.clone(),
            public_jwk: km.public_jwk.clone(),
        })
    }
    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
        unreachable!("not reached on the serving path")
    }
}

fn composed_app(keys: Arc<dyn SigningKeyStore>) -> axum::Router {
    build_router(AppState::without_db().with_dataapi(dataapi_state(keys)))
}

/// The trial path's router, nested at the same prefix the console's trial uses,
/// and built by the SAME function `try_it` calls — not an equivalent assembled
/// here, which would assert against a copy of the wiring rather than the wiring.
fn trial_app(keys: Arc<dyn SigningKeyStore>) -> axum::Router {
    axum::Router::new()
        .nest("/data-api/v1", egret_dataapi::trial_serving_router())
        .with_state(dataapi_state(keys))
}

fn dataapi_state(keys: Arc<dyn SigningKeyStore>) -> DataApiState {
    // Port 1: nothing listens, and nothing in this file needs a query to
    // succeed. The short `acquire_timeout` matters though — `serve` writes its
    // `dataapi.access_log` row AFTER the response, so with sqlx's 30 s default
    // every request that reaches the handler would hold its bulkhead permit for
    // half a minute and this file would take minutes to say nothing extra.
    let pool = sqlx::postgres::PgPoolOptions::new()
        .acquire_timeout(std::time::Duration::from_millis(250))
        .connect_lazy("postgres://egret:egret@127.0.0.1:1/egret")
        .expect("lazy pool");
    let db = Db { pool };
    DataApiState {
        db: db.clone(),
        clients: ClientStore::new(db.clone()),
        endpoints: EndpointStore::new(db.clone()),
        users: egret_auth::users::UserRepo::new(db.clone()),
        sessions: egret_auth::session::SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::minutes(60),
        ),
        workspaces: egret_workspace::workspaces::WorkspaceRepo::new(db.clone()),
        authz: egret_workspace::rbac::Authorizer::new(db.clone()),
        pdp: Arc::new(DenyPdp),
        query: Arc::new(UnreachablePort),
        keys,
        issuer: "https://egret.test".into(),
        consumer_base_url: "https://egret.test".into(),
        audit: egret_audit::AuditSink::new(1).0,
        cache: egret_dataapi::serving::ResponseCache::new(),
        rate_limits: Arc::new(egret_dataapi::serving::TokenBucket::new()),
        activity: egret_dataapi::ActivityStore::new(db.clone(), Some(90)),
    }
}

fn serving_request(method: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri("/data-api/v1/default/reports/daily")
        .header("authorization", "Bearer not-a-real-token")
}

async fn body_json(res: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "response body is not the serving envelope ({e}): {}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

/// The deadline, crossed for real.
///
/// The request parks inside `verify_serving_token` and never returns on its own.
/// With the clock paused, tokio advances to the next timer — which is the
/// deadline — so what fires is the production `SERVING_REQUEST_DEADLINE` and not
/// a number this test chose.
///
/// Delete the `serving_deadline` layer and this test does not fail, it **hangs**
/// forever, which is the honest signal: with no bound there is no answer.
#[tokio::test(start_paused = true)]
async fn a_request_that_never_finishes_is_answered_504_by_the_deadline() {
    let entered = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let app = composed_app(Arc::new(SlowKeys {
        entered: entered.clone(),
        release: release.clone(),
    }));

    let res = tokio::time::timeout(
        // Generously above the production deadline, and only a guard against
        // hanging the suite: if this is what fires, the bound did not.
        std::time::Duration::from_secs(600),
        app.oneshot(serving_request("GET").body(Body::empty()).unwrap()),
    )
    .await
    .expect("no answer within 600 virtual seconds — the deadline layer is not bounding this")
    .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::GATEWAY_TIMEOUT,
        "a request that outruns the serving deadline must be answered 504"
    );
    assert_eq!(
        entered.load(Ordering::SeqCst),
        1,
        "the request must have reached the handler; a 504 from anywhere else \
         would not be the deadline"
    );

    let body = body_json(res).await;
    assert_eq!(body["status"], "error");
    assert_eq!(
        body["error"]["code"], "timeout",
        "the deadline must answer in the serving envelope with its own code"
    );
    assert!(
        body["error"]["request_id"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "every serving error carries a correlation id: {body}"
    );
}

/// The bulkhead, filled for real.
///
/// `SERVING_MAX_CONCURRENCY` requests are parked inside the handler — counted,
/// not slept for — and the next one is refused. Then they are released and the
/// held requests are shown to have been genuinely in flight rather than already
/// finished.
///
/// Raise the semaphore (or delete the layer) and the extra request is answered
/// by the handler instead, so the `503` assertion fails.
#[tokio::test]
async fn the_bulkhead_sheds_once_max_concurrency_requests_are_in_flight() {
    let entered = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let app = composed_app(Arc::new(SlowKeys {
        entered: entered.clone(),
        release: release.clone(),
    }));

    let mut held = Vec::new();
    for _ in 0..SERVING_MAX_CONCURRENCY {
        let app = app.clone();
        held.push(tokio::spawn(async move {
            app.oneshot(serving_request("GET").body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status()
        }));
    }

    // Wait for the slots to be genuinely occupied. Sleeping instead would make
    // this test's outcome depend on scheduling luck.
    while entered.load(Ordering::SeqCst) < SERVING_MAX_CONCURRENCY {
        tokio::task::yield_now().await;
    }

    let res = app
        .clone()
        .oneshot(serving_request("GET").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "the {SERVING_MAX_CONCURRENCY}+1-th in-flight request must be shed"
    );
    assert_eq!(
        res.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("1"),
        "a shed request is told to retry on the timescale of one request, which \
         is how an operator tells this 503 from the config-drift ones (300)"
    );
    assert_eq!(
        entered.load(Ordering::SeqCst),
        SERVING_MAX_CONCURRENCY,
        "the shed request must NOT have reached the handler — that is what a \
         bulkhead is for"
    );

    let body = body_json(res).await;
    assert_eq!(
        body["error"]["code"], "overloaded",
        "a 503 from the bulkhead and a 503 from published-config drift must not \
         read the same to an operator"
    );

    // The held requests were in flight, not finished: they only complete once
    // released. Without this the test above would also pass against a router
    // that had already answered all 64.
    release.notify_waiters();
    for h in held {
        assert_eq!(h.await.unwrap(), StatusCode::UNAUTHORIZED);
    }

    // And the surface recovers: the permits came back when the guards dropped.
    // Measured by ADMISSION (the handler is entered) rather than by the final
    // status — this store parks every caller, so a completed status would only
    // ever be the deadline firing.
    let recovery = tokio::spawn({
        let app = app.clone();
        async move {
            let _ = app
                .oneshot(serving_request("GET").body(Body::empty()).unwrap())
                .await;
        }
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while entered.load(Ordering::SeqCst) <= SERVING_MAX_CONCURRENCY {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("a request after the drain was never admitted — permits are not released");
    recovery.abort();
}

/// The body cap, exceeded for real — one byte over, declared honestly.
///
/// `serve_post` reads no body at all, so before this layer a `POST` of any size
/// was accepted and discarded; the handler's `401` is what this test would see
/// with the layer deleted.
#[tokio::test]
async fn a_post_declaring_a_body_over_the_cap_is_refused_413() {
    let app = composed_app(Arc::new(FastKeys));
    let oversize = vec![b'x'; (SERVING_MAX_BODY_BYTES + 1) as usize];

    let res = app
        .oneshot(
            serving_request("POST")
                .header("content-type", "application/json")
                // Set explicitly: driving a `Router` as a tower `Service` skips
                // hyper, which is what would otherwise derive this header from
                // a sized body. Every real client sends it.
                .header("content-length", oversize.len().to_string())
                .body(Body::from(oversize))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "a body over SERVING_MAX_BODY_BYTES must be refused, not streamed and \
         discarded"
    );
    let body = body_json(res).await;
    assert_eq!(body["error"]["code"], "payload_too_large");
}

/// The other side of the boundary, so the test above cannot pass by refusing
/// everything. Exactly at the cap is accepted and reaches the handler.
#[tokio::test]
async fn a_post_at_exactly_the_cap_still_reaches_the_handler() {
    let app = composed_app(Arc::new(FastKeys));
    let at_cap = vec![b'x'; SERVING_MAX_BODY_BYTES as usize];

    let res = app
        .oneshot(
            serving_request("POST")
                .header("content-type", "application/json")
                .header("content-length", at_cap.len().to_string())
                .body(Body::from(at_cap))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a body at the cap must be served (and then rejected by the handler's \
         own first step), not refused by the cap"
    );
}

/// The trial bulkhead is a process-wide `static` — that is the whole point of
/// the fix below — so the three tests that fill or occupy it would see each
/// other's permits if the harness ran them at the same time.
///
/// `tokio::sync::Mutex`, not `std`'s: this is held across awaits, and tokio's
/// primitives are runtime-agnostic, so a test parked here is woken by whichever
/// runtime releases it (each `#[tokio::test]` builds its own).
static TRIAL_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The finding: `try_it` builds a router **per trial**, so a bulkhead built with
/// the router had `TRIAL_MAX_CONCURRENCY` permits per request and could never be
/// reached by any number of concurrent trials.
///
/// Two routers are built here the way two concurrent trials build them — the
/// first is filled, the second is a fresh call to the same constructor — and the
/// second must be refused. Give the trial path a per-router semaphore again and
/// the second router admits the request and answers `401` from the handler, so
/// this fails on the status.
#[tokio::test]
async fn a_router_built_per_trial_still_shares_one_process_wide_bulkhead() {
    let _serialised = TRIAL_TESTS.lock().await;
    let entered = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let keys = || -> Arc<dyn SigningKeyStore> {
        Arc::new(SlowKeys {
            entered: entered.clone(),
            release: release.clone(),
        })
    };

    let first = trial_app(keys());
    let mut held = Vec::new();
    for _ in 0..egret_dataapi::TRIAL_MAX_CONCURRENCY {
        let app = first.clone();
        held.push(tokio::spawn(async move {
            app.oneshot(serving_request("GET").body(Body::empty()).unwrap())
                .await
                .unwrap()
                .status()
        }));
    }
    while entered.load(Ordering::SeqCst) < egret_dataapi::TRIAL_MAX_CONCURRENCY {
        tokio::task::yield_now().await;
    }

    // A SECOND router, built exactly as the next trial would build it.
    let second = trial_app(keys());
    let res = second
        .oneshot(serving_request("GET").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "a trial router built while {} trials are in flight must be shed. If it \
         is not, the trial bulkhead is per-router and therefore per-request, \
         which is a cap nobody can reach",
        egret_dataapi::TRIAL_MAX_CONCURRENCY
    );
    assert_eq!(
        entered.load(Ordering::SeqCst),
        egret_dataapi::TRIAL_MAX_CONCURRENCY,
        "the shed trial must not have reached the handler"
    );
    let body = body_json(res).await;
    assert_eq!(body["error"]["code"], "overloaded");

    release.notify_waiters();
    for h in held {
        assert_eq!(h.await.unwrap(), StatusCode::UNAUTHORIZED);
    }
}

/// The deadline half of the same finding, driven against the configuration the
/// review named: **two statements at the default `limits.timeout_seconds`**.
///
/// That is 2 x 30 = 60 s of legal worst case. The consumer path is bounded at 50
/// s and answers `504` — correct, and derived from the ingress it crosses. The
/// trial path crosses no such edge (it is a `tower::ServiceExt::oneshot`, in
/// process, with no socket between the halves), and inheriting the consumer's
/// number meant an author clicking **Try it** on a configuration egret itself
/// accepts was told it timed out.
///
/// Both halves are driven here, in one test, from one duration. Set
/// `TRIAL_REQUEST_DEADLINE` back to `SERVING_REQUEST_DEADLINE` and the trial
/// assertion fails; delete the trial's own bound and the consumer assertion
/// stops being a control for it.
#[tokio::test(start_paused = true)]
async fn a_legal_two_statement_endpoint_is_504d_for_a_consumer_and_not_for_a_trial() {
    let _serialised = TRIAL_TESTS.lock().await;
    // The legal worst case, built from the two constants that produce it rather
    // than typed as `60`.
    let worst_case = Duration::from_secs(2 * u64::from(egret_dataapi_api::DEFAULT_TIMEOUT_SECONDS));
    assert!(
        worst_case > egret_dataapi::serving::SERVING_REQUEST_DEADLINE,
        "this test is only interesting while a legal two-statement endpoint can \
         outrun the consumer deadline"
    );

    let consumer = composed_app(Arc::new(SleepyKeys(worst_case)));
    let res = consumer
        .oneshot(serving_request("GET").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::GATEWAY_TIMEOUT,
        "the consumer path is bounded by the edge it crosses, and 60 s is past it"
    );

    let trial = trial_app(Arc::new(SleepyKeys(worst_case)));
    let res = trial
        .oneshot(serving_request("GET").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "the SAME endpoint tried from the console must reach the handler and be \
         answered on its own merits (401 — the token in this file is not real). \
         A 504 here is the trial path inheriting a bound derived from an edge it \
         never crosses"
    );
}

/// And the trial deadline is still a bound, not the absence of one.
///
/// The published ceiling is `MAX_STATEMENTS x TIMEOUT_CEILING_SECS` — 80 minutes
/// — and a browser tab held open for that is a hang, not a diagnosis. Past
/// `TRIAL_REQUEST_DEADLINE` the author gets egret's own envelope with a code.
#[tokio::test(start_paused = true)]
async fn a_trial_that_outruns_its_own_deadline_is_answered_504_in_the_envelope() {
    let _serialised = TRIAL_TESTS.lock().await;

    // The bound has to be a NUMBER and not merely present. The drive below
    // sleeps `TRIAL_REQUEST_DEADLINE + 1s`, so raising the constant raises the
    // sleep with it and a `504` would arrive at any value — including an hour,
    // which is no bound at all for the thing actually being bounded, a person
    // watching a browser tab. Held to five minutes, well under the 80-minute
    // published worst case that motivates having a deadline here at all.
    assert!(
        egret_dataapi::TRIAL_REQUEST_DEADLINE <= Duration::from_secs(300),
        "TRIAL_REQUEST_DEADLINE is {:?}; past a few minutes the console hangs \
         rather than diagnoses, and the deadline stops being a bound",
        egret_dataapi::TRIAL_REQUEST_DEADLINE
    );

    let over = egret_dataapi::TRIAL_REQUEST_DEADLINE + Duration::from_secs(1);
    let trial = trial_app(Arc::new(SleepyKeys(over)));

    let res = trial
        .oneshot(serving_request("GET").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::GATEWAY_TIMEOUT,
        "the trial path has its own deadline; without one a published 80-minute \
         worst case would hang the console"
    );
    let body = body_json(res).await;
    assert_eq!(body["error"]["code"], "timeout");
}
