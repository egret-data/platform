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
        cache: std::sync::Arc::new(egret_dataapi::serving::ResponseCache::new()),
        rate_limits: Arc::new(egret_dataapi::serving::TokenBucket::new()),
        activity: egret_dataapi::ActivityStore::new(db.clone(), Some(90)),
        group_recompiler: std::sync::Arc::new(egret_perm_compile::PermGroupRecompiler),
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
        body["error"]["code"], "deadline_exceeded",
        "the deadline must answer in the serving envelope with ITS OWN code, and \
         `deadline_exceeded` is what makes it distinguishable from the other 504 \
         on this surface — `timeout`, which is the endpoint author's published \
         budget being exceeded (`ServingError::EngineTimeout`). One string for \
         both and an operator could not separate `we are cutting callers off` \
         from `their queries are too slow`"
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

/// The deadline, in the direction that matters: a trial is cut exactly where a
/// consumer is.
///
/// Driven at the configuration the review named — **two statements at the
/// default `limits.timeout_seconds`**, i.e. 2 x 30 = 60 s of legal worst case,
/// built from the constants that produce it rather than typed as `60`.
///
/// This duration is the discriminator on purpose: it is past
/// `SERVING_REQUEST_DEADLINE` (50 s), and it is a duration a **120 s trial
/// deadline would have let through** — which is what an earlier version of this
/// branch did. Try it answers "can a consumer call this endpoint?", so a trial
/// that completed here would have told the author yes about a call a consumer is
/// cut off from; and behind an ingress it would be unreachable anyway, with
/// nginx answering the console at 60 s with a generic gateway error instead of
/// egret's envelope.
///
/// Both routers, one duration, one expectation. Give the trial path a longer
/// deadline and the second half fails.
#[tokio::test(start_paused = true)]
async fn a_trial_is_cut_at_exactly_the_deadline_a_consumer_is() {
    let _serialised = TRIAL_TESTS.lock().await;
    let legal_worst_case =
        Duration::from_secs(2 * u64::from(egret_dataapi_api::DEFAULT_TIMEOUT_SECONDS));
    assert!(
        legal_worst_case > egret_dataapi::serving::SERVING_REQUEST_DEADLINE,
        "this test only discriminates while a legal two-statement endpoint at the \
         defaults ({legal_worst_case:?}) can outrun the serving deadline ({:?})",
        egret_dataapi::serving::SERVING_REQUEST_DEADLINE
    );

    for (app, who) in [
        (
            composed_app(Arc::new(SleepyKeys(legal_worst_case))),
            "a consumer",
        ),
        (trial_app(Arc::new(SleepyKeys(legal_worst_case))), "a trial"),
    ] {
        let res = app
            .oneshot(serving_request("GET").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::GATEWAY_TIMEOUT,
            "{who} must be answered 504 at the serving deadline. A trial that \
             survived this would report success for a call a consumer is cut off \
             from, which is the question Try it exists to answer"
        );
        let body = body_json(res).await;
        assert_eq!(
            body["error"]["code"], "deadline_exceeded",
            "and in egret's own envelope, with a code — not a gateway's HTML. \
             `deadline_exceeded`, not `timeout`: the latter is the endpoint's \
             published budget, a different actor answering the same status"
        );
    }
}

/// The other direction, without which the test above would pass against a trial
/// path that refused everything.
///
/// An ordinary trial — well inside the deadline — reaches the handler and is
/// answered on its own merits (401 here: the token in this file is not real).
#[tokio::test(start_paused = true)]
async fn an_ordinary_trial_well_inside_the_deadline_still_reaches_the_handler() {
    let _serialised = TRIAL_TESTS.lock().await;
    let quick = Duration::from_secs(1);
    assert!(
        quick < egret_dataapi::serving::SERVING_REQUEST_DEADLINE,
        "the control has to be inside the bound to be a control"
    );

    let trial = trial_app(Arc::new(SleepyKeys(quick)));
    let res = trial
        .oneshot(serving_request("GET").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a short trial must be served, not refused by the deadline"
    );
}
