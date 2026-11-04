//! Governance SP2b Task 9: the OPA decision surface, driven end to end through
//! the router with a stub credential verifier and a hand-built snapshot cache.
//! No database.
//!
//! Endpoint authentication is tested in both directions on every route: right
//! secret, wrong secret, unknown engine, an engine bound to a Ranger profile,
//! an unbound engine, a not-built profile, and a missing context property.
//!
//! # Log capture
//!
//! `tracing` caches a callsite's interest process-wide, and `set_default` is
//! thread-local, so a per-test subscriber can miss events in a multi-test
//! binary. Every test therefore installs ONE global subscriber first
//! ([`logs`]), which writes into a shared buffer; a test that asserts on log
//! lines picks its own by a user name no other test uses.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use axum::body::Body;
use axum::http::Request;
use egret_engine_mgmt_api::credential::{EngineCredentialVerifier, EngineIdentity};
use egret_perm_compile::opa::cache::SnapshotCache;
use egret_perm_compile::opa::surface::{VerifiedCache, NEGATIVE_TTL};
use egret_perm_compile::{opa_surface, OpaState, OPA_SURFACE_PREFIX};
use egret_perm_eval::{Snapshot, SnapshotInput};
use egret_perm_ir::audience::{Audience, Member, MemberName, Via};
use egret_perm_ir::statement::*;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;

const GOOD: uuid::Uuid = uuid::Uuid::from_u128(1); // bound to service 1 (on OPA, Ready)
const GOOD2: uuid::Uuid = uuid::Uuid::from_u128(2); // bound to service 2 (not in the cache: a Ranger profile)
const UNBOUND: uuid::Uuid = uuid::Uuid::from_u128(3); // verified, bound to nothing
const RIGHT: &str = "right";
const WRONG: &str = "wrong";

struct Stub {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl EngineCredentialVerifier for Stub {
    async fn verify(&self, id: uuid::Uuid, pw: &str) -> Result<Option<EngineIdentity>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(
            ([GOOD, GOOD2, UNBOUND].contains(&id) && pw == RIGHT).then_some(EngineIdentity {
                engine_id: id,
                // Deliberately wrong: the surface must use the cache's binding.
                authz_service_id: Some(99),
            }),
        )
    }
}

/// A verifier whose store cannot be reached.
struct Down;

#[async_trait::async_trait]
impl EngineCredentialVerifier for Down {
    async fn verify(&self, _: uuid::Uuid, _: &str) -> Result<Option<EngineIdentity>, String> {
        Err("connection refused".into())
    }
}

// ---- log capture ----

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);
struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log lock").extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogCapture {
    type Writer = LogWriter;
    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(self.0.clone())
    }
}

/// The process-wide log buffer; installs the global subscriber on first use.
fn logs() -> &'static LogCapture {
    static CAPTURE: OnceLock<LogCapture> = OnceLock::new();
    CAPTURE.get_or_init(|| {
        let capture = LogCapture::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .finish();
        tracing::subscriber::set_global_default(subscriber).expect("global subscriber");
        capture
    })
}

fn log_text() -> String {
    String::from_utf8(logs().0.lock().expect("log lock").clone()).expect("utf8")
}

// ---- fixtures ----

fn who(users: &[&str]) -> Audience {
    Audience {
        include: users
            .iter()
            .map(|n| Member {
                name: if *n == "*" {
                    MemberName::Everyone
                } else {
                    MemberName::User((*n).into())
                },
                via: Via::Direct,
            })
            .collect(),
        except: vec![],
    }
}

fn path(c: &str, s: Option<&str>, t: Option<&str>, col: Option<&str>) -> Target {
    let l = |v: Option<&str>| v.map_or(Level::Unnamed, |v| Level::Named(v.into()));
    Target::Path(Path {
        catalog_id: None,
        catalog: c.into(),
        schema: l(s),
        table: l(t),
        column: l(col),
    })
}

fn st(n: u128, audience: Audience, target: Target, effect: Effect) -> Statement {
    Statement {
        source: Source::Grant {
            grant_id: uuid::Uuid::from_u128(n),
        },
        audience,
        target,
        effect,
        derivation: Derivation::AsDeclared,
    }
}

fn snapshot(service_id: i64, extra: Vec<Statement>) -> Snapshot {
    let mut statements = vec![
        st(
            1,
            who(&["ann"]),
            path("lake", None, None, None),
            Effect::Allow {
                privileges: vec![Privilege::Read],
            },
        ),
        st(
            2,
            who(&["*"]),
            path("lake", Some("s"), Some("t"), Some("c")),
            Effect::Mask {
                mask_type: "MASK_NULL".into(),
                expression: None,
            },
        ),
        st(
            3,
            who(&["*"]),
            path("lake", Some("s"), Some("t"), None),
            Effect::RowFilter {
                expression: "x = 1".into(),
            },
        ),
    ];
    statements.extend(extra);
    Snapshot::build(SnapshotInput {
        service_id,
        key: (1, 1),
        statements,
        classified: vec![],
    })
    .expect("snapshot")
}

struct App {
    router: axum::Router,
    cache: Arc<SnapshotCache>,
    stub: Arc<Stub>,
}

fn app_with(credentials: VerifiedCache) -> App {
    logs();
    let cache = Arc::new(SnapshotCache::default());
    cache.insert_for_test(1, snapshot(1, vec![]));
    cache.bind_for_test(GOOD, Some(1));
    cache.bind_for_test(GOOD2, Some(2));
    cache.bind_for_test(UNBOUND, None);
    let stub = Arc::new(Stub {
        calls: AtomicUsize::new(0),
    });
    let router = opa_surface(OpaState {
        cache: Arc::clone(&cache),
        verifier: stub.clone(),
        credentials: Arc::new(credentials),
    });
    App {
        router,
        cache,
        stub,
    }
}

fn app() -> App {
    app_with(VerifiedCache::new(Duration::from_secs(5)))
}

fn body(user: &str, engine: &str, secret: &str, action: Value) -> Value {
    json!({"input": {"context": {
        "identity": {"user": user, "groups": []},
        "softwareStack": {"trinoVersion": "483"},
        "properties": {"egret.engine": engine, "egret.secret": secret}},
        "action": action}})
}

async fn send(app: &axum::Router, route: &str, body: Value) -> (u16, Value) {
    let res = app
        .clone()
        .oneshot(
            Request::post(format!("{OPA_SURFACE_PREFIX}{route}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = res.status().as_u16();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    let v = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, v)
}

async fn post(
    app: &axum::Router,
    route: &str,
    engine: &str,
    secret: &str,
    action: Value,
) -> (u16, Value) {
    send(app, route, body("ann", engine, secret, action)).await
}

fn column(c: &str) -> Value {
    json!({"column": {"catalogName": "lake", "schemaName": "s", "tableName": "t",
        "columnName": c, "columnType": "varchar"}})
}

fn table() -> Value {
    json!({"table": {"catalogName": "lake", "schemaName": "s", "tableName": "t"}})
}

/// One well-formed request per route, which answers non-empty for `ann` on
/// a Ready snapshot.
fn every_route() -> Vec<(&'static str, Value)> {
    vec![
        (
            "/allow",
            json!({"operation": "SelectFromColumns", "resource":
                {"table": {"catalogName": "lake", "schemaName": "s", "tableName": "t", "columns": ["c"]}}}),
        ),
        (
            "/batch",
            json!({"operation": "FilterCatalogs", "filterResources":
                [{"catalog": {"name": "x"}}, {"catalog": {"name": "lake"}}]}),
        ),
        (
            "/row_filters",
            json!({"operation": "GetRowFilters", "resource": table()}),
        ),
        (
            "/batch_column_masks",
            json!({"operation": "GetColumnMask", "filterResources": [column("c")]}),
        ),
        (
            "/column_mask",
            json!({"operation": "GetColumnMask", "resource": column("c")}),
        ),
    ]
}

/// The refusal each route must answer: 200 false / [] on the decision
/// routes, 403 with an error on the value routes (never `[]` or `null`).
async fn assert_refused_everywhere(app: &axum::Router, engine: &str, secret: &str, why: &str) {
    for (route, action) in every_route() {
        let (status, v) = post(app, route, engine, secret, action).await;
        match route {
            "/allow" => assert_eq!(
                (status, v),
                (200, json!({"result": false})),
                "{why} {route}"
            ),
            "/batch" => assert_eq!((status, v), (200, json!({"result": []})), "{why} {route}"),
            _ => {
                assert_eq!(status, 403, "{why} {route}: {v}");
                assert!(v["error"].is_string(), "{why} {route}: {v}");
                assert_eq!(
                    v.as_object().map(|o| o.len()),
                    Some(1),
                    "{why} {route}: {v}"
                );
            }
        }
    }
}

// ---- tests ----

#[tokio::test]
async fn right_secret_answers_from_the_snapshot_on_every_route() {
    let a = app();
    let e = GOOD.to_string();
    let answers = [
        json!({"result": true}),
        json!({"result": [1]}),
        json!({"result": [{"expression": "(x = 1)"}]}),
        json!({"result": [{"index": 0, "viewExpression": {"expression": "NULL"}}]}),
        json!({"result": {"expression": "NULL"}}),
    ];
    for ((route, action), want) in every_route().into_iter().zip(answers) {
        assert_eq!(
            post(&a.router, route, &e, RIGHT, action).await,
            (200, want),
            "{route}"
        );
    }
}

#[tokio::test]
async fn value_routes_answer_the_empty_value_only_from_a_ready_snapshot() {
    let a = app();
    let e = GOOD.to_string();
    let other = json!({"table": {"catalogName": "lake", "schemaName": "s", "tableName": "u"}});
    assert_eq!(
        post(
            &a.router,
            "/row_filters",
            &e,
            RIGHT,
            json!({"operation": "GetRowFilters", "resource": other})
        )
        .await,
        (200, json!({"result": []}))
    );
    assert_eq!(
        post(
            &a.router,
            "/column_mask",
            &e,
            RIGHT,
            json!({"operation": "GetColumnMask", "resource": column("d")})
        )
        .await,
        (200, json!({"result": null}))
    );
    // Masks map back to the request's own indices.
    assert_eq!(
        post(
            &a.router,
            "/batch_column_masks",
            &e,
            RIGHT,
            json!({"operation": "GetColumnMask", "filterResources": [column("a"), column("c"), column("d")]})
        )
        .await,
        (
            200,
            json!({"result": [{"index": 1, "viewExpression": {"expression": "NULL"}}]})
        )
    );
}

#[tokio::test]
async fn filter_columns_indices_address_the_columns_not_the_one_table() {
    let a = app();
    let (status, v) = post(
        &a.router,
        "/batch",
        &GOOD.to_string(),
        RIGHT,
        json!({"operation": "FilterColumns", "filterResources": [
            {"table": {"catalogName": "lake", "schemaName": "s", "tableName": "t", "columns": ["a", "b", "c"]}}]}),
    )
    .await;
    assert_eq!((status, v), (200, json!({"result": [0, 1, 2]})));
    // A FilterColumns of another shape is refused, not answered.
    assert_eq!(
        post(
            &a.router,
            "/batch",
            &GOOD.to_string(),
            RIGHT,
            json!({"operation": "FilterColumns", "filterResources": [table(), table()]}),
        )
        .await,
        (200, json!({"result": []}))
    );
}

#[tokio::test]
async fn the_binding_comes_from_the_cache_not_the_verifier() {
    let a = app();
    let e = GOOD.to_string();
    let (route, action) = every_route().remove(0);
    assert_eq!(
        post(&a.router, route, &e, RIGHT, action.clone()).await,
        (200, json!({"result": true}))
    );
    // Within the credential TTL: the verdict is cached, the binding is not.
    a.cache.bind_for_test(GOOD, None);
    assert_eq!(
        post(&a.router, route, &e, RIGHT, action.clone()).await,
        (200, json!({"result": false}))
    );
    let (status, _) = post(
        &a.router,
        "/row_filters",
        &e,
        RIGHT,
        json!({"operation": "GetRowFilters", "resource": table()}),
    )
    .await;
    assert_eq!(status, 403);
    // Rebound to a profile not served on OPA: refused too.
    a.cache.bind_for_test(GOOD, Some(2));
    assert_eq!(
        post(&a.router, route, &e, RIGHT, action).await,
        (200, json!({"result": false}))
    );
    assert_eq!(a.stub.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn wrong_secret_is_refused_on_every_route() {
    let a = app();
    assert_refused_everywhere(&a.router, &GOOD.to_string(), WRONG, "wrong secret").await;
    assert_refused_everywhere(&a.router, &GOOD.to_string(), "", "empty secret").await;
}

#[tokio::test]
async fn unknown_engine_is_refused_on_every_route() {
    let a = app();
    let e = uuid::Uuid::new_v4().to_string();
    assert_refused_everywhere(&a.router, &e, RIGHT, "unknown engine").await;
}

#[tokio::test]
async fn a_ranger_profile_is_refused_on_every_route() {
    let a = app();
    assert_refused_everywhere(&a.router, &GOOD2.to_string(), RIGHT, "ranger profile").await;
}

#[tokio::test]
async fn an_unbound_engine_is_refused_on_every_route() {
    let a = app();
    assert_refused_everywhere(&a.router, &UNBOUND.to_string(), RIGHT, "unbound").await;
}

#[tokio::test]
async fn a_not_built_profile_is_refused_on_every_route() {
    let a = app();
    a.cache
        .insert_not_built_for_test(3, Some("lowering failed".into()));
    a.cache.bind_for_test(GOOD, Some(3));
    assert_refused_everywhere(&a.router, &GOOD.to_string(), RIGHT, "not built").await;
    let (_, v) = post(
        &a.router,
        "/row_filters",
        &GOOD.to_string(),
        RIGHT,
        json!({"operation": "GetRowFilters", "resource": table()}),
    )
    .await;
    assert_eq!(v, json!({"error": "no snapshot: lowering failed"}));
    a.cache.insert_not_built_for_test(3, None);
    let (_, v) = post(
        &a.router,
        "/column_mask",
        &GOOD.to_string(),
        RIGHT,
        json!({"operation": "GetColumnMask", "resource": column("c")}),
    )
    .await;
    assert_eq!(v, json!({"error": "no snapshot: building"}));
}

#[tokio::test]
async fn a_missing_context_property_is_refused_on_every_route() {
    let a = app();
    for (route, action) in every_route() {
        for properties in [
            json!(null),
            json!({}),
            json!({"egret.engine": GOOD.to_string()}),
            json!({"egret.secret": RIGHT}),
            json!({"egret.engine": "not-a-uuid", "egret.secret": RIGHT}),
        ] {
            let b = json!({"input": {"context": {"identity": {"user": "ann"}, "properties": properties},
                "action": action.clone()}});
            let (status, v) = send(&a.router, route, b).await;
            match route {
                "/allow" => assert_eq!((status, v), (200, json!({"result": false}))),
                "/batch" => assert_eq!((status, v), (200, json!({"result": []}))),
                _ => assert_eq!(
                    (status, v),
                    (
                        403,
                        json!({"error": "no engine credential in opa.context-file"})
                    ),
                    "{route} {properties}"
                ),
            }
        }
    }
    assert_eq!(a.stub.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_malformed_request_is_refused_on_every_route() {
    let a = app();
    for (route, _) in every_route() {
        for raw in ["not json", "{}", r#"{"input": {"context": {}}}"#] {
            let res = a
                .router
                .clone()
                .oneshot(
                    Request::post(format!("{OPA_SURFACE_PREFIX}{route}"))
                        .body(Body::from(raw))
                        .expect("request"),
                )
                .await
                .expect("response");
            let status = res.status().as_u16();
            let bytes = res.into_body().collect().await.expect("body").to_bytes();
            let v: Value = serde_json::from_slice(&bytes).expect("json body");
            match route {
                "/allow" => assert_eq!((status, v), (200, json!({"result": false}))),
                "/batch" => assert_eq!((status, v), (200, json!({"result": []}))),
                _ => {
                    assert_eq!(status, 403, "{route} {raw}");
                    assert!(
                        v["error"]
                            .as_str()
                            .is_some_and(|e| e.starts_with("malformed input: ")),
                        "{route} {raw}: {v}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_malformed_mask_request_is_not_an_empty_mask_list() {
    let a = app();
    let e = GOOD.to_string();
    let (status, v) = post(
        &a.router,
        "/batch_column_masks",
        &e,
        RIGHT,
        json!({"operation": "GetColumnMask", "filterResources": [table()]}),
    )
    .await;
    assert_eq!(
        (status, v),
        (
            403,
            json!({"error": "GetColumnMask needs column resources in filterResources"})
        )
    );
    // One column among non-columns: the whole request is refused.
    let (status, _) = post(
        &a.router,
        "/batch_column_masks",
        &e,
        RIGHT,
        json!({"operation": "GetColumnMask", "filterResources": [column("c"), table()]}),
    )
    .await;
    assert_eq!(status, 403);
    // No column at all is not "no mask".
    let (status, _) = post(
        &a.router,
        "/batch_column_masks",
        &e,
        RIGHT,
        json!({"operation": "GetColumnMask", "filterResources": []}),
    )
    .await;
    assert_eq!(status, 403);
    let (status, v) = post(
        &a.router,
        "/row_filters",
        &e,
        RIGHT,
        json!({"operation": "GetRowFilters", "resource": {"catalog": {"name": "lake"}}}),
    )
    .await;
    assert_eq!(
        (status, v),
        (
            403,
            json!({"error": "GetRowFilters needs a table resource"})
        )
    );
    // `*` and empty names never reach the evaluator.
    for t in ["*", ""] {
        let (status, _) = post(
            &a.router,
            "/row_filters",
            &e,
            RIGHT,
            json!({"operation": "GetRowFilters", "resource":
                {"table": {"catalogName": "lake", "schemaName": "s", "tableName": t}}}),
        )
        .await;
        assert_eq!(status, 403, "table {t:?}");
    }
    let (status, _) = post(
        &a.router,
        "/column_mask",
        &e,
        RIGHT,
        json!({"operation": "GetColumnMask", "resource": column("*")}),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = post(
        &a.router,
        "/column_mask",
        &e,
        RIGHT,
        json!({"operation": "GetColumnMask", "resource": table()}),
    )
    .await;
    assert_eq!(status, 403);
    // The wrong operation on a value route is refused.
    let (status, _) = post(
        &a.router,
        "/row_filters",
        &e,
        RIGHT,
        json!({"operation": "GetColumnMask", "resource": table()}),
    )
    .await;
    assert_eq!(status, 403);
}

#[tokio::test]
async fn two_masks_answer_500_so_the_plugin_fails_closed() {
    let a = app();
    let marker = "twomask-user-7d1e";
    a.cache.insert_for_test(
        1,
        snapshot(
            1,
            vec![st(
                9,
                who(&[marker]),
                path("lake", Some("s"), Some("t"), Some("c")),
                Effect::Mask {
                    mask_type: "CUSTOM".into(),
                    expression: Some("secret_expr_9f3({col})".into()),
                },
            )],
        ),
    );
    let e = GOOD.to_string();
    for (route, action) in [
        (
            "/batch_column_masks",
            json!({"operation": "GetColumnMask", "filterResources": [column("c")]}),
        ),
        (
            "/column_mask",
            json!({"operation": "GetColumnMask", "resource": column("c")}),
        ),
    ] {
        let (status, v) = send(&a.router, route, body(marker, &e, RIGHT, action)).await;
        assert_eq!(
            (status, v),
            (500, json!({"error": "two masks on one column"})),
            "{route}"
        );
    }
    let log = log_text();
    let lines: Vec<&str> = log.lines().filter(|l| l.contains(marker)).collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    for l in &lines {
        assert!(l.contains("ERROR"), "{l}");
        assert!(l.contains("column"), "{l}");
        assert!(
            !l.contains("secret_expr_9f3"),
            "a mask expression was logged: {l}"
        );
        assert!(!l.contains("NULL"), "a mask expression was logged: {l}");
    }
    // Another user, one mask: still answered.
    assert_eq!(
        post(
            &a.router,
            "/column_mask",
            &e,
            RIGHT,
            json!({"operation": "GetColumnMask", "resource": column("c")})
        )
        .await,
        (200, json!({"result": {"expression": "NULL"}}))
    );
}

#[tokio::test]
async fn an_unreachable_verifier_answers_500_on_every_route() {
    logs();
    let cache = Arc::new(SnapshotCache::default());
    cache.insert_for_test(1, snapshot(1, vec![]));
    cache.bind_for_test(GOOD, Some(1));
    let router = opa_surface(OpaState {
        cache,
        verifier: Arc::new(Down),
        credentials: Arc::new(VerifiedCache::new(Duration::from_secs(5))),
    });
    for (route, action) in every_route() {
        let (status, _) = post(&router, route, &GOOD.to_string(), RIGHT, action).await;
        assert_eq!(status, 500, "{route}");
    }
}

#[tokio::test(start_paused = true)]
async fn a_verified_secret_is_remembered_within_the_ttl_and_a_refused_one_briefly() {
    let a = app_with(VerifiedCache::new(Duration::from_secs(5)));
    let e = GOOD.to_string();
    let (route, action) = every_route().remove(0);
    let calls = || a.stub.calls.load(Ordering::SeqCst);

    assert_eq!(
        post(&a.router, route, &e, RIGHT, action.clone()).await,
        (200, json!({"result": true}))
    );
    assert_eq!(
        post(&a.router, route, &e, RIGHT, action.clone()).await,
        (200, json!({"result": true}))
    );
    assert_eq!(calls(), 1);

    // A refusal is remembered for NEGATIVE_TTL, keyed by the secret it refused.
    for _ in 0..2 {
        assert_eq!(
            post(&a.router, route, &e, WRONG, action.clone()).await,
            (200, json!({"result": false}))
        );
    }
    assert_eq!(calls(), 2);
    // ... and never refuses the right secret, nor lets the wrong one in.
    assert_eq!(
        post(&a.router, route, &e, RIGHT, action.clone()).await,
        (200, json!({"result": true}))
    );
    assert_eq!(calls(), 2);

    // Past both TTLs, each is asked again.
    tokio::time::advance(Duration::from_secs(5).max(NEGATIVE_TTL) + Duration::from_millis(1)).await;
    assert_eq!(
        post(&a.router, route, &e, RIGHT, action.clone()).await,
        (200, json!({"result": true}))
    );
    assert_eq!(calls(), 3);
    assert_eq!(
        post(&a.router, route, &e, WRONG, action.clone()).await,
        (200, json!({"result": false}))
    );
    assert_eq!(calls(), 4);
}

#[tokio::test(start_paused = true)]
async fn the_credential_verdict_is_per_engine() {
    let a = app();
    let (route, action) = every_route().remove(0);
    assert_eq!(
        post(&a.router, route, &GOOD.to_string(), RIGHT, action.clone()).await,
        (200, json!({"result": true}))
    );
    // Same secret, an engine the verifier does not know: asked, refused.
    let other = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        post(&a.router, route, &other, RIGHT, action).await,
        (200, json!({"result": false}))
    );
    assert_eq!(a.stub.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_batch_logs_one_line_however_many_items_it_denies() {
    let a = app();
    let marker = "batchlog-user-51ac";
    let resources: Vec<Value> = (0..100)
        .map(|i| json!({"catalog": {"name": format!("c{i}")}}))
        .collect();
    let (status, v) = send(
        &a.router,
        "/batch",
        body(
            marker,
            &GOOD.to_string(),
            RIGHT,
            json!({"operation": "FilterCatalogs", "filterResources": resources}),
        ),
    )
    .await;
    assert_eq!((status, v), (200, json!({"result": []})));
    let log = log_text();
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains(marker) && l.contains("opa: denied"))
        .collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("denied=100"), "{}", lines[0]);
    assert!(lines[0].contains("c0"), "{}", lines[0]);
    assert!(!lines[0].contains("c1\""), "{}", lines[0]);
}

#[tokio::test]
async fn a_denied_allow_and_a_refusal_each_log_one_line() {
    let a = app();
    let marker = "denylog-user-0c2b";
    let (route, action) = every_route().remove(0);
    send(
        &a.router,
        route,
        body(marker, &GOOD.to_string(), RIGHT, action.clone()),
    )
    .await;
    send(
        &a.router,
        "/row_filters",
        body(
            marker,
            &GOOD.to_string(),
            WRONG,
            json!({"operation": "GetRowFilters", "resource": table()}),
        ),
    )
    .await;
    let log = log_text();
    let lines: Vec<&str> = log
        .lines()
        .filter(|l| l.contains(marker) && l.contains("opa: denied"))
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines.iter().all(|l| l.contains("egret::opa")), "{lines:?}");
    assert!(lines[1].contains("engine credential refused"), "{lines:?}");
}

#[tokio::test]
async fn no_log_or_body_carries_the_secret() {
    let secret = "s3cr3t-value";
    let a = app();
    let mut bodies = Vec::new();
    for engine in [GOOD, GOOD2, UNBOUND, uuid::Uuid::new_v4()] {
        for (route, action) in every_route() {
            let (_, v) = post(&a.router, route, &engine.to_string(), secret, action).await;
            bodies.push(v.to_string());
        }
    }
    // A malformed request carrying the secret, and an engine of the wrong type.
    for (route, action) in every_route() {
        let mut b = body("ann", &GOOD.to_string(), secret, action.clone());
        b["input"]["action"]["operation"] = json!(7);
        bodies.push(send(&a.router, route, b).await.1.to_string());
        let mut b = body("ann", &GOOD.to_string(), secret, action);
        b["input"]["context"]["properties"]["egret.engine"] = json!([secret]);
        bodies.push(send(&a.router, route, b).await.1.to_string());
    }
    // The verifier failing on a request carrying it.
    let down = opa_surface(OpaState {
        cache: Arc::clone(&a.cache),
        verifier: Arc::new(Down),
        credentials: Arc::new(VerifiedCache::new(Duration::from_secs(5))),
    });
    for (route, action) in every_route() {
        bodies.push(
            post(&down, route, &GOOD.to_string(), secret, action)
                .await
                .1
                .to_string(),
        );
    }
    for b in &bodies {
        assert!(!b.contains(secret), "a body carries the secret");
    }
    let log = log_text();
    assert!(log.contains("opa: denied"), "nothing was logged at all");
    assert!(log.contains("connection refused"), "the 500 was not logged");
    assert!(!log.contains(secret), "a log line carries the secret");
}
