//! DB-gated Ranger HTTP tests. Self-skip without DATABASE_URL.
//!
//! The Ranger `/service` surface is read-only (policy download + GETs) since E2e;
//! services/policies are created through the store (as egret's admin API does),
//! not via HTTP write routes (which were removed).
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_metastore::Db;
use egret_policy::{ranger_router, RangerState};
use egret_policy::{seed_trino, PolicyStore};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "devtoken";

/// A bearer-gated Ranger app with the Trino def seeded and one service created.
/// Returns the router, the service name, and a store handle for further seeding.
async fn app() -> Option<(axum::Router, String, PolicyStore)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");
    let svc = format!("trino_{}", uuid::Uuid::new_v4().simple());
    store
        .create_service(&svc, "trino", &json!({}))
        .await
        .expect("create service");
    let state = RangerState {
        store: store.clone(),
        auth_token: Arc::from(TOKEN),
    };
    Some((ranger_router(state), svc, store))
}

fn req(method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    let body = match body {
        Some(v) => Body::from(serde_json::to_vec(&v).unwrap()),
        None => Body::empty(),
    };
    if matches!(method, "POST" | "PUT") {
        b = b.header("content-type", "application/json");
    }
    b.body(body).unwrap()
}

#[tokio::test]
async fn anonymous_mode_allows_download_without_bearer() {
    // Empty token = anonymous /service so stock Ranger plugins (no bearer) can download.
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed");
    let svc = format!("trino_{}", uuid::Uuid::new_v4().simple());
    store
        .create_service(&svc, "trino", &json!({}))
        .await
        .expect("create service");
    let app = ranger_router(RangerState {
        store,
        auth_token: Arc::from(""),
    });
    // download (no bearer) → 200 in anonymous mode.
    let r = app
        .oneshot(req(
            "GET",
            &format!("/plugins/policies/download/{svc}"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn download_requires_bearer() {
    let Some((app, svc, _store)) = app().await else {
        eprintln!("skip");
        return;
    };
    let r = app
        .oneshot(req(
            "GET",
            &format!("/plugins/policies/download/{svc}"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn seeded_service_policy_then_download_and_304() {
    let Some((app, svc, store)) = app().await else {
        eprintln!("skip");
        return;
    };

    // download (no lastKnownVersion) → 200 ServicePolicies with serviceDef.
    let r = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/plugins/policies/download/{svc}"),
            Some(TOKEN),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let sp: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sp["serviceName"], svc.as_str());
    assert_eq!(sp["serviceDef"]["name"], "trino");
    let ver = sp["policyVersion"].as_i64().unwrap();

    // same version → 304.
    let r = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/plugins/policies/download/{svc}?lastKnownVersion={ver}"),
            Some(TOKEN),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_MODIFIED);

    // add a policy through the store → version bumps → download at old version is
    // 200 with 1 policy.
    let svc_id = store.get_service(&svc).await.expect("get service").id;
    let pol = json!({"service": svc, "name":"p1","policyType":0,
        "resources":{"catalog":{"values":["hive"],"isExcludes":false,"isRecursive":false},
                     "schema":{"values":["default"]},"table":{"values":["t"]},"column":{"values":["*"]}},
        "policyItems":[{"accesses":[{"type":"select","isAllowed":true}],"users":["alice"]}]});
    store
        .create_policy(svc_id, "p1", &pol)
        .await
        .expect("create policy");

    let r = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/plugins/policies/download/{svc}?lastKnownVersion={ver}"),
            Some(TOKEN),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let sp: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sp["policies"].as_array().unwrap().len(), 1);
    assert!(sp["policyVersion"].as_i64().unwrap() > ver);
    // The real Ranger policy engine NPEs on a policy whose `id` is null; the download must
    // inject the DB-assigned id (+ isEnabled). Regression guard for the live-Trino finding.
    assert!(
        sp["policies"][0]["id"].as_i64().is_some(),
        "downloaded policy must carry a numeric id, got {}",
        sp["policies"][0]
    );
    assert_eq!(sp["policies"][0]["isEnabled"], serde_json::json!(true));
}

#[tokio::test]
async fn download_unknown_service_404() {
    let Some((app, _svc, _store)) = app().await else {
        eprintln!("skip");
        return;
    };
    let r = app
        .oneshot(req(
            "GET",
            "/plugins/policies/download/nope_xyz",
            Some(TOKEN),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
