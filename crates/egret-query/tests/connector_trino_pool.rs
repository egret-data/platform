//! The Trino connector's HTTP behaviour, driven against a **real** HTTP server.
//!
//! Package 3b changed two things that a unit test cannot honestly check, because
//! both are properties of what goes over a socket:
//!
//! * the `reqwest::Client` is now **shared per coordinator** instead of built
//!   per query — so the question "can two principals leak into each other?" is
//!   about what the client itself carries between requests, not about what a
//!   builder was configured with;
//! * a **503 is retried** per the Trino client protocol instead of being turned
//!   into a 500 that abandons a live query — so the question is how many times
//!   the coordinator is actually asked.
//!
//! Every test below therefore stands up a stub coordinator on `127.0.0.1:0`,
//! records what each request carried, and asserts on the recording. No engine
//! is required and nothing is gated: these run in ordinary CI.
//!
//! Each test binds its own port, so the process-wide client cache in
//! `connector::http_pool` gives each of them a distinct key and they do not
//! observe one another.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use tokio_util::sync::CancellationToken;

use egret_query::execution::connector::{build_connector, ConnSpec};
use egret_query::execution::model::Limits;
use egret_query::registry::model::{ConnOptions, Engine};

/// What one request to the stub coordinator carried.
#[derive(Debug, Clone)]
struct Seen {
    trino_user: Option<String>,
    authorization: Option<String>,
    cookie: Option<String>,
    /// The client's source port. Two requests sharing one means they travelled
    /// over one TCP connection — which is what "the handshake is not paid per
    /// statement" actually means.
    peer_port: u16,
}

#[derive(Clone)]
struct Stub {
    seen: Arc<Mutex<Vec<Seen>>>,
    /// How many of the first N requests answer `503` before a real body is
    /// returned. Decremented per request.
    fail_with_503: Arc<AtomicUsize>,
    /// When set, every 503 carries `Retry-After: <secs>`.
    retry_after: Option<u64>,
    /// When set, every 200 carries a `Set-Cookie`.
    set_cookie: Option<String>,
    /// When true the 503s never stop — the "permanently unavailable" case.
    always_503: bool,
}

/// A minimal but protocol-shaped Trino answer: one column, one row, no
/// `nextUri`, so `run_statement` completes in a single hop.
const TRINO_BODY: &str = r#"{"columns":[{"name":"n","type":"bigint"}],"data":[[1]],"stats":{}}"#;

async fn statement(
    State(st): State<Stub>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    _body: String,
) -> Response {
    let get = |k: &str| {
        headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    st.seen.lock().unwrap().push(Seen {
        trino_user: get("x-trino-user"),
        authorization: get("authorization"),
        cookie: get("cookie"),
        peer_port: peer.port(),
    });

    let still_failing = st.always_503
        || st
            .fail_with_503
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
    if still_failing {
        let mut resp = (StatusCode::SERVICE_UNAVAILABLE, "").into_response();
        if let Some(secs) = st.retry_after {
            resp.headers_mut()
                .insert("Retry-After", secs.to_string().parse().unwrap());
        }
        return resp;
    }

    let mut resp = (
        StatusCode::OK,
        [("content-type", "application/json")],
        TRINO_BODY,
    )
        .into_response();
    if let Some(c) = &st.set_cookie {
        resp.headers_mut().insert("Set-Cookie", c.parse().unwrap());
    }
    resp
}

/// Start the stub and return `(port, recording)`.
async fn start(stub: Stub) -> (u16, Arc<Mutex<Vec<Seen>>>) {
    let seen = stub.seen.clone();
    let app = Router::new()
        .route("/v1/statement", post(statement))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (port, seen)
}

fn stub() -> Stub {
    Stub {
        seen: Arc::new(Mutex::new(Vec::new())),
        fail_with_503: Arc::new(AtomicUsize::new(0)),
        retry_after: None,
        set_cookie: None,
        always_503: false,
    }
}

fn spec(port: u16, session_user: &str, bearer: &str) -> ConnSpec {
    ConnSpec {
        engine: Engine::Trino,
        host: "127.0.0.1".into(),
        port: port as i32,
        options: ConnOptions {
            user: Some("egret-service".into()),
            impersonate: Some(true),
            ..Default::default()
        },
        secret: None,
        session_user: Some(session_user.to_string()),
        // Plain HTTP + a bearer would be refused by the transport guard, so the
        // bearer here is carried through `EGRET_ENGINE_TLS_INSECURE`-free paths
        // only where the test sets it; see `spec_with_bearer`.
        bearer_token: Some(bearer.to_string()),
    }
}

/// The impersonation (basic-auth) shape: no bearer, so no TLS guard applies over
/// plain HTTP. `X-Trino-User` is the per-principal identity.
fn spec_impersonating(port: u16, session_user: &str) -> ConnSpec {
    ConnSpec {
        engine: Engine::Trino,
        host: "127.0.0.1".into(),
        port: port as i32,
        options: ConnOptions {
            user: Some("egret-service".into()),
            impersonate: Some(true),
            ..Default::default()
        },
        secret: Some(format!("password-for-{session_user}")),
        session_user: Some(session_user.to_string()),
        bearer_token: None,
    }
}

fn limits(secs: u64) -> Limits {
    Limits {
        max_rows: 100,
        timeout: Duration::from_secs(secs),
    }
}

/// **What the retry policy decides, recorded — not how long this machine took.**
///
/// The connector emits one `debug` event per retry carrying `wait_ms`: the wait
/// [`retry_wait`](../src/execution/connector/trino.rs) computed, emitted *before*
/// it is slept. Recording those is the difference between asserting a property of
/// the policy and asserting a property of the machine, and the difference is not
/// academic. The three tests below used to time the call with
/// `std::time::Instant` and require the elapsed under 3-4 seconds; each passed
/// alone in about a second and failed on a loaded machine having observed 3.0 to
/// 4.7s. Four workspace gate runs were spent re-reading them as regressions.
///
/// A recorded ask cannot drift with load: `retry_wait` returns the same number on
/// an idle laptop and on a machine with two dozen test binaries on it — while
/// still failing, loudly and with the offending number, when the cap is removed.
#[derive(Clone, Default)]
struct Waits {
    asked: Arc<Mutex<Vec<Duration>>>,
    /// Signalled as each wait is recorded, so a test can judge the ask WITHOUT
    /// waiting out the sleep it describes. That is what lets the absurd
    /// `Retry-After` test fail in milliseconds rather than in an hour.
    recorded: Arc<tokio::sync::Notify>,
}

impl Waits {
    /// Route this thread's tracing events into the recorder for as long as the
    /// returned guard lives. `#[tokio::test]` is a current-thread runtime, so
    /// the whole test — spawned tasks included — is polled on the thread that
    /// installs this, and each test binds its own recorder on its own thread.
    fn install(&self) -> tracing::subscriber::DefaultGuard {
        use tracing_subscriber::layer::SubscriberExt;
        tracing::subscriber::set_default(
            tracing_subscriber::registry().with(WaitRecorder(self.clone())),
        )
    }

    fn asked(&self) -> Vec<Duration> {
        self.asked.lock().unwrap().clone()
    }

    /// Return once the policy has ASKED for its first sleep — before that sleep
    /// happens.
    ///
    /// The bound is a liveness guard, not an assertion about duration: the event
    /// is emitted one localhost round-trip in, so thirty seconds is four orders
    /// of magnitude of headroom, and the only thing that can exhaust it is a
    /// connector that never retried at all.
    async fn first_ask(&self) {
        if !self.asked.lock().unwrap().is_empty() {
            return;
        }
        tokio::time::timeout(Duration::from_secs(30), self.recorded.notified())
            .await
            .expect("the connector never asked to sleep: the 503 was not retried at all");
    }
}

struct WaitRecorder(Waits);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WaitRecorder {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        #[derive(Default)]
        struct Grab(Option<u64>);
        impl tracing::field::Visit for Grab {
            fn record_u64(&mut self, f: &tracing::field::Field, v: u64) {
                if f.name() == "wait_ms" {
                    self.0 = Some(v);
                }
            }
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        }
        let mut grab = Grab::default();
        event.record(&mut grab);
        if let Some(ms) = grab.0 {
            self.0.asked.lock().unwrap().push(Duration::from_millis(ms));
            // `notify_one` stores a permit, so a `first_ask` that arrives after
            // the event still returns immediately.
            self.0.recorded.notify_one();
        }
    }
}

/// **The pooling safety property, measured rather than argued.**
///
/// Two different principals run a statement against ONE coordinator. Because the
/// client is now shared, both requests travel over the same TCP connection —
/// which is the win: the second statement pays no handshake. What must NOT be
/// shared is who they are, and each request carries its own `X-Trino-User` and
/// its own credential.
///
/// This is the test the fixture shape matters for: with only one principal in
/// it, a client that had somehow acquired an identity would pass. Two
/// principals, asserted separately, is what makes it evidence.
#[tokio::test]
async fn two_principals_on_one_coordinator_share_a_client_and_never_an_identity() {
    let (port, seen) = start(stub()).await;

    for user in ["alice", "bob"] {
        let c = build_connector(&spec_impersonating(port, user));
        c.execute("SELECT 1", &limits(10), CancellationToken::new())
            .await
            .expect("the stub answers a well-formed Trino body");
    }

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "one request each");

    // Each request carried its OWN identity.
    assert_eq!(seen[0].trino_user.as_deref(), Some("alice"));
    assert_eq!(seen[1].trino_user.as_deref(), Some("bob"));
    // And its own credential — the basic-auth values differ because the two
    // connections opened different secrets.
    assert!(seen[0].authorization.is_some() && seen[1].authorization.is_some());
    assert_ne!(
        seen[0].authorization, seen[1].authorization,
        "two principals must not present one credential"
    );

    // And the connection WAS reused, which is the whole point of the change: a
    // fresh `reqwest::Client` per query would have opened a second socket from a
    // different source port.
    assert_eq!(
        seen[0].peer_port, seen[1].peer_port,
        "the second statement paid a new TCP handshake — the client was not reused"
    );
}

/// **The leak a shared client makes possible, and the line that closes it.**
///
/// The workspace builds `reqwest` with the `cookies` feature on, so
/// `cookie_store(true)` is one word away and compiles. With it, a coordinator
/// (or an authenticating proxy in front of one) that sets a session cookie for
/// alice would have bob's request carry alice's session — a cross-principal
/// credential leak through a client neither of them knows exists.
///
/// Mutation: set `.cookie_store(true)` in `http_pool::build_client` and this
/// fails, because bob's request arrives carrying `sid=alice-session`.
#[tokio::test]
async fn a_shared_client_never_carries_one_principals_cookie_to_another() {
    let mut s = stub();
    s.set_cookie = Some("sid=alice-session; Path=/".into());
    let (port, seen) = start(s).await;

    for user in ["alice", "bob"] {
        let c = build_connector(&spec_impersonating(port, user));
        c.execute("SELECT 1", &limits(10), CancellationToken::new())
            .await
            .expect("served");
    }

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[0].cookie, None,
        "the first request cannot have a cookie yet"
    );
    assert_eq!(
        seen[1].cookie, None,
        "bob's request arrived carrying alice's session cookie: {:?}",
        seen[1].cookie
    );
}

/// **The 503 finding: the Trino client protocol says the query is still alive.**
///
/// The coordinator refuses twice and then answers. Before Package 3b that first
/// 503 became `EngineError::Engine("http 503")` → a 500, abandoning a live query
/// and reporting egret's own bug as the engine's.
///
/// The fixture forces the failure rather than hoping for it: without the two
/// forced 503s the first attempt would succeed and the test would pass with the
/// retry deleted.
#[tokio::test]
async fn a_503_is_retried_and_the_query_survives() {
    let s = stub();
    s.fail_with_503.store(2, Ordering::SeqCst);
    let (port, seen) = start(s).await;

    let c = build_connector(&spec_impersonating(port, "alice"));
    let out = c
        .execute("SELECT 1", &limits(10), CancellationToken::new())
        .await
        .expect("two 503s must not lose a live query");
    assert_eq!(out.row_count, 1);

    assert_eq!(
        seen.lock().unwrap().len(),
        3,
        "the coordinator should have been asked three times: 503, 503, 200"
    );
}

/// The retry is BOUNDED. A coordinator that is genuinely down must still produce
/// an error, must not be hammered forever, and must not sleep past the budget
/// every caller here already sits under — one it did not choose.
///
/// **Asserted on the policy's own decisions, recorded as it makes them.** The
/// previous version timed the whole call and required it under 4 seconds; that
/// is a property of the machine, and on a loaded one it was observed at 4.7s.
/// The sequence below is what `retry_wait` computes — `TRINO_RETRY_BASE_BACKOFF`
/// doubling, four times, under `TRINO_RETRY_MAX_BACKOFF` — and it is the same
/// sequence at any load.
#[tokio::test]
async fn a_permanent_503_gives_up_after_a_bounded_number_of_attempts() {
    let mut s = stub();
    s.always_503 = true;
    let (port, seen) = start(s).await;

    let waits = Waits::default();
    let _recording = waits.install();

    let c = build_connector(&spec_impersonating(port, "alice"));
    let err = c
        .execute("SELECT 1", &limits(30), CancellationToken::new())
        .await
        .expect_err("a permanently unavailable coordinator is still an error");

    assert!(
        err.to_string().contains("503"),
        "the caller should still be told what the coordinator said: {err}"
    );
    // Five attempts total (the initial one plus four retries) — the constant in
    // `trino.rs`. An unbounded loop would never reach the assertion at all.
    assert_eq!(
        seen.lock().unwrap().len(),
        5,
        "the retry budget is five attempts"
    );

    let asked = waits.asked();
    assert_eq!(
        asked,
        vec![
            Duration::from_millis(50),
            Duration::from_millis(100),
            Duration::from_millis(200),
            Duration::from_millis(400),
        ],
        "the four sleeps between five attempts are the base backoff doubling \
         under the 1s per-sleep cap; the policy asked for {asked:?}"
    );
    // 750ms in total, inside `TRINO_RETRY_MAX_TOTAL_WAIT` — which is the claim
    // "well inside every caller budget" actually rests on.
    let total: Duration = asked.iter().sum();
    assert!(
        total <= Duration::from_millis(4000),
        "the retries of one request may ask for at most TRINO_RETRY_MAX_TOTAL_WAIT \
         (4s) of sleep, which is what makes them fit inside every caller budget; \
         this one asked for {total:?}"
    );
}

/// `Retry-After` is honoured when the coordinator sends one — and capped, so the
/// engine cannot decide how long an egret request hangs.
///
/// Both halves are one assertion on the recorded ask. egret's own backoff for a
/// first retry is 50ms, so a 1000ms sleep can only have come from the header;
/// and 1000ms is exactly `TRINO_RETRY_MAX_BACKOFF`, so the header was honoured
/// as far as the cap allows and no further. The previous version made the same
/// claim as `elapsed >= 900ms && elapsed < 3s`, through a clock this test does
/// not control.
#[tokio::test]
async fn a_retry_after_header_is_honoured() {
    let mut s = stub();
    s.retry_after = Some(1);
    s.fail_with_503.store(1, Ordering::SeqCst);
    let (port, seen) = start(s).await;

    let waits = Waits::default();
    let _recording = waits.install();

    let c = build_connector(&spec_impersonating(port, "alice"));
    c.execute("SELECT 1", &limits(30), CancellationToken::new())
        .await
        .expect("served after the asked-for wait");

    assert_eq!(seen.lock().unwrap().len(), 2);
    let asked = waits.asked();
    assert_eq!(
        asked,
        vec![Duration::from_secs(1)],
        "`Retry-After: 1` must be what the connector sleeps — its own backoff for \
         a first retry is 50ms, so anything shorter means the header was ignored; \
         the policy asked for {asked:?}"
    );
}

/// **The cap, at the socket, with a header no test could previously notice.**
///
/// The other `Retry-After` test asks for one second, which IS the cap — so
/// `d.min(cap) == d` and it passes with the cap deleted. This one asks for an
/// hour, and asserts on the number the policy arrived at rather than on how long
/// the call took: `Retry-After: 3600` must become a 1000ms sleep.
///
/// The ask is judged *before* the sleep it describes, which is why this test is
/// not merely load-proof but fast in the failure case: with the cap removed the
/// recorded ask is `[3600s]` and the assertion fires in milliseconds, where the
/// old version could only fail by outliving an 8-second outer timeout.
///
/// Why it matters beyond this connector: `describe`/`describe_with_params` run at
/// publish time, and until Package 3b they had no outer wall clock at all
/// (`port_impl::describe_bounded` now provides one) — an unbounded honouring of
/// this header held a publish-validate open for as long as the coordinator said.
#[tokio::test]
async fn an_absurd_retry_after_is_capped_and_cannot_hang_the_request() {
    let mut s = stub();
    s.retry_after = Some(3600);
    s.fail_with_503.store(1, Ordering::SeqCst);
    let (port, seen) = start(s).await;

    let waits = Waits::default();
    let _recording = waits.install();

    let c = build_connector(&spec_impersonating(port, "alice"));
    let query = tokio::spawn(async move {
        c.execute("SELECT 1", &limits(30), CancellationToken::new())
            .await
    });

    waits.first_ask().await;
    let asked = waits.asked();
    assert_eq!(
        asked,
        vec![Duration::from_millis(1000)],
        "`Retry-After: 3600` must be capped at TRINO_RETRY_MAX_BACKOFF (1s): an \
         engine does not get to decide how long an egret request hangs. The \
         policy asked for {asked:?}"
    );

    // And the capped wait is really taken and the query really served — the cap
    // shortens the retry, it does not skip it. The bound on the join is a
    // liveness guard, not an assertion about duration: the sleep asserted above
    // is one second, so nothing here is being measured against a clock.
    let out = tokio::time::timeout(Duration::from_secs(60), query)
        .await
        .expect("the request never returned after a capped one-second wait")
        .expect("the query task panicked")
        .expect("the query survives one 503");

    assert_eq!(out.row_count, 1);
    assert_eq!(seen.lock().unwrap().len(), 2, "503 then 200");
}

/// **A statement that could change data is never re-POSTed.**
///
/// The retry-safety argument used to rest on "this connector only ever carries
/// `SELECT`s", which is false: `execution/job.rs`'s `run_query` sends SQL Lab and
/// notebook SQL straight through `Connector::execute`, and `apply_sql_limit`
/// leaves non-`SELECT` SQL unchanged. A coordinator's own 503 is emitted before
/// the statement is registered, but a proxy in front of one can synthesise a 503
/// *after* forwarding the POST — and then a re-POST is a second `INSERT`.
///
/// So the write is asked exactly once, while the read beside it is asked five
/// times. Both halves are here because either alone would pass for the wrong
/// reason: the write alone would pass with the whole retry deleted.
///
/// The third clause is the second review round's finding, and it is the one
/// direction the classifier may never move in: a statement that GAINED a retry.
/// `EXPLAIN ANALYZE <stmt>` **executes** `<stmt>`, and a leading `explain` used
/// to be proof of a read, so `EXPLAIN ANALYZE INSERT …` was re-POSTable — the
/// only write on this path that the guard itself waved through.
#[tokio::test]
async fn a_write_statement_is_never_re_posted_after_a_503() {
    let mut s = stub();
    s.always_503 = true;
    let (port, seen) = start(s).await;

    let c = build_connector(&spec_impersonating(port, "alice"));
    let err = c
        .execute(
            "INSERT INTO t SELECT * FROM u",
            &limits(30),
            CancellationToken::new(),
        )
        .await
        .expect_err("a permanent 503 is still an error");
    assert!(err.to_string().contains("503"), "{err}");
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "an INSERT that got a 503 must be reported, not sent again — a 503 \
         synthesised after the POST was forwarded would duplicate the rows"
    );

    // `EXPLAIN ANALYZE` runs the statement it is given. Same connector, same
    // stub, same permanent 503 — and it must be asked exactly once, for exactly
    // the reason the bare `INSERT` above is.
    seen.lock().unwrap().clear();
    let err = c
        .execute(
            "EXPLAIN ANALYZE INSERT INTO t SELECT * FROM u",
            &limits(30),
            CancellationToken::new(),
        )
        .await
        .expect_err("a permanent 503 is still an error");
    assert!(err.to_string().contains("503"), "{err}");
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "`EXPLAIN ANALYZE INSERT` EXECUTES the INSERT — Trino runs the \
         statement and reports its timings. Re-POSTing it duplicates rows just \
         as the bare INSERT would, so a leading `explain` is not proof of a read"
    );

    seen.lock().unwrap().clear();
    let _ = c
        .execute("SELECT * FROM u", &limits(30), CancellationToken::new())
        .await
        .expect_err("still a permanent 503");
    assert_eq!(
        seen.lock().unwrap().len(),
        5,
        "a read on the same connector still gets the full retry budget"
    );
}

/// A 401/403 is NOT a 503 and must not be retried: the credential will not
/// improve, and four extra requests to an engine that is rejecting identity is
/// exactly the wrong response to it.
#[tokio::test]
async fn an_auth_failure_is_not_retried() {
    async fn deny() -> Response {
        (StatusCode::FORBIDDEN, "").into_response()
    }
    let hits = Arc::new(AtomicUsize::new(0));
    let counted = hits.clone();
    let app = Router::new().route(
        "/v1/statement",
        post(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            deny()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let c = build_connector(&spec_impersonating(port, "alice"));
    let err = c
        .execute("SELECT 1", &limits(10), CancellationToken::new())
        .await
        .expect_err("403 is an error");
    assert!(
        matches!(err, egret_query::execution::EngineError::Forbidden),
        "got {err:?}"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1, "a 403 must be asked once");
}

/// The connector still refuses to put a per-user JWT on a plain-HTTP transport,
/// and the client cache did not create a way around it. `spec` builds the bearer
/// shape; it is here so that helper is used and the guard is re-pinned next to
/// the pooling change that touched the same constructor.
#[tokio::test]
async fn the_transport_guard_still_refuses_a_bearer_over_plain_http() {
    let (port, seen) = start(stub()).await;
    let c = build_connector(&spec(port, "alice", "eyJ.jwt.sig"));
    let err = c
        .execute("SELECT 1", &limits(10), CancellationToken::new())
        .await
        .expect_err("a JWT over plain HTTP must be refused");
    assert!(
        matches!(err, egret_query::execution::EngineError::Connect(_)),
        "got {err:?}"
    );
    assert!(
        seen.lock().unwrap().is_empty(),
        "the refusal must happen before anything reaches the network"
    );
}
