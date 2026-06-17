//! DB-gated Ranger HTTP tests. Self-skip without DATABASE_URL.
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_authz::{seed_trino, AuthzStore};
use egret_metastore::Db;
use egret_ranger::{ranger_router, RangerState};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const TOKEN: &str = "devtoken";

async fn app() -> Option<(axum::Router, String)> {
    let url = std::env::var("DATABASE_URL").ok()?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = AuthzStore::new(db);
    seed_trino(&store).await.expect("seed");
    let svc = format!("trino_{}", uuid::Uuid::new_v4().simple());
    let state = RangerState { store, auth_token: Arc::from(TOKEN) };
    Some((ranger_router(state), svc))
}

fn req(method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(t) = token { b = b.header("authorization", format!("Bearer {t}")); }
    let body = match body { Some(v) => Body::from(serde_json::to_vec(&v).unwrap()), None => Body::empty() };
    if matches!(method, "POST"|"PUT") { b = b.header("content-type", "application/json"); }
    b.body(body).unwrap()
}

#[tokio::test]
async fn download_requires_bearer() {
    let Some((app, svc)) = app().await else { eprintln!("skip"); return; };
    let r = app.oneshot(req("GET", &format!("/plugins/policies/download/{svc}"), None, None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn create_service_policy_then_download_and_304() {
    let Some((app, svc)) = app().await else { eprintln!("skip"); return; };
    // create service
    let r = app.clone().oneshot(req("POST", "/plugins/services", Some(TOKEN),
        Some(json!({"name": svc, "type": "trino", "configs": {}})))).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);

    // download (no lastKnownVersion) → 200 ServicePolicies with serviceDef
    let r = app.clone().oneshot(req("GET", &format!("/plugins/policies/download/{svc}"), Some(TOKEN), None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let sp: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sp["serviceName"], svc.as_str());
    assert_eq!(sp["serviceDef"]["name"], "trino");
    let ver = sp["policyVersion"].as_i64().unwrap();

    // same version → 304
    let r = app.clone().oneshot(req("GET",
        &format!("/plugins/policies/download/{svc}?lastKnownVersion={ver}"), Some(TOKEN), None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_MODIFIED);

    // add a policy → version bumps → download at old version is 200 with 1 policy
    let pol = json!({"service": svc, "name":"p1","policyType":0,
        "resources":{"catalog":{"values":["hive"],"isExcludes":false,"isRecursive":false},
                     "schema":{"values":["default"]},"table":{"values":["t"]},"column":{"values":["*"]}},
        "policyItems":[{"accesses":[{"type":"select","isAllowed":true}],"users":["alice"]}]});
    let r = app.clone().oneshot(req("POST", "/plugins/policies", Some(TOKEN), Some(pol))).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = app.clone().oneshot(req("GET",
        &format!("/plugins/policies/download/{svc}?lastKnownVersion={ver}"), Some(TOKEN), None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let bytes = r.into_body().collect().await.unwrap().to_bytes();
    let sp: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sp["policies"].as_array().unwrap().len(), 1);
    assert!(sp["policyVersion"].as_i64().unwrap() > ver);
}

#[tokio::test]
async fn download_unknown_service_404() {
    let Some((app, _svc)) = app().await else { eprintln!("skip"); return; };
    let r = app.oneshot(req("GET", "/plugins/policies/download/nope_xyz", Some(TOKEN), None)).await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
