//! DB-gated Ranger HTTP tests. Self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`, and otherwise FAIL.
//!
//! The Ranger `/service` surface is read-only (policy download + GETs) since E2e;
//! services/policies are created through the store (as egret's admin API does),
//! not via HTTP write routes (which were removed).
use axum::body::Body;
use axum::http::{Request, StatusCode};
use egret_metastore::Db;
use egret_policy::{ranger_router, ranger_surface, RangerState};
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

/// A capturing `tracing::Subscriber`, so a test can assert that a handler
/// SAID something — SP4d debt Task 9, step 3b.
///
/// `download_policies` used to resolve a dangling `tagService` with `if let
/// Ok(tsvc)`, and the observable HTTP response of that swallow is IDENTICAL to
/// the fixed code's: 200 with no `tagPolicies` block. So a test that asserts
/// only the response cannot tell the fix from the defect, and restoring the
/// swallow would leave it green. The thing that changed is that the omission is
/// now AUDIBLE (and that a database error propagates instead of serving a
/// mask-free download). The warning is the half a test can reach, so it is
/// asserted; the DB-error arm is reachable only with a fault injector and is
/// held by the `match` being exhaustive.
///
/// Hand-rolled on `tracing` alone rather than pulling in `tracing-subscriber`:
/// the only thing needed is the event's fields as text.
#[derive(Clone, Default)]
struct CapturedLogs(Arc<std::sync::Mutex<Vec<String>>>);

struct FieldText(String);

impl tracing::field::Visit for FieldText {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!(" {}={:?}", field.name(), value));
    }
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.push_str(&format!(" {}={}", field.name(), value));
    }
}

impl tracing::Subscriber for CapturedLogs {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut text = FieldText(String::new());
        event.record(&mut text);
        self.0
            .lock()
            .expect("log mutex")
            .push(format!("{}{}", event.metadata().level(), text.0));
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// Inserts a profile WITHOUT `create_service`'s guards. Task 5 (D) made
/// `create_service` refuse a `tagService` that names a missing or non-`tag`
/// profile, but rows of exactly that shape already exist (spec D4: they are not
/// migrated), and what this file pins is how the system treats THEM.
async fn insert_legacy_service(
    pool: &sqlx::PgPool,
    name: &str,
    typ: &str,
    config: &serde_json::Value,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO policy.authz_services (name, type, config_jsonb) VALUES ($1,$2,$3) RETURNING id",
    )
    .bind(name)
    .bind(typ)
    .bind(config)
    .fetch_one(pool)
    .await
    .expect("raw insert of a pre-existing-shape profile")
}

/// **SP4d debt Task 9, step 3b — the third mechanism.** `download_policies`
/// resolved the profile's `tagService` with `if let Ok(tsvc)`, which collapsed
/// "names no profile" and "the database read failed" into one unlogged omission
/// of the `tagPolicies` block. The plugin then caches a complete-looking
/// download in which every tag-addressed mask and row filter is simply absent.
///
/// This pins the two outcomes that are now distinguishable:
/// * a dangling `tagService` still serves **200 with no `tagPolicies`** — a 404
///   here would strip the plugin of its RESOURCE policies too — and the handler
///   now logs a warning saying so, which is what `trigger.rs` already did for
///   the same shape;
/// * a resolvable one embeds the block, with the tag service's own def and its
///   enabled policies.
///
/// The second half is the accept-control: "always omits the block" would pass
/// the first assertion on its own, and omitting it is precisely the silent
/// failure being fixed. The DB-error arm (now propagated instead of swallowed)
/// is not reachable from here without a fault injector; it is the arm that
/// changed from a FAIL-OPEN to a 500.
#[tokio::test]
async fn a_dangling_tag_service_omits_the_block_and_a_real_one_embeds_it() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed trino");
    egret_policy::seed_tag(&store).await.expect("seed tag def");
    let app = ranger_router(RangerState {
        store: store.clone(),
        auth_token: Arc::from(TOKEN),
    });

    let suffix = uuid::Uuid::new_v4().simple().to_string();

    // ── A `tagService` naming no policy profile at all.
    let dangling = format!("t9dang_{suffix}");
    let missing = format!("t9missing_{suffix}");
    insert_legacy_service(&pool, &dangling, "trino", &json!({ "tagService": missing })).await;
    let logs = CapturedLogs::default();
    let log_guard = tracing::subscriber::set_default(logs.clone());
    let r = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/plugins/policies/download/{dangling}"),
            Some(TOKEN),
            None,
        ))
        .await
        .unwrap();
    drop(log_guard);
    let captured = logs.0.lock().expect("log mutex").clone();
    assert!(
        captured.iter().any(|l| l.starts_with("WARN")
            && l.contains("names no policy profile")
            && l.contains(&missing)
            && l.contains(&dangling)),
        "the omission must be AUDIBLE — a WARN naming the profile and the tagService it \
         could not resolve. This is the assertion the restored `if let Ok(_)` swallow \
         fails, because the HTTP response is identical either way. captured={captured:#?}"
    );
    assert_eq!(
        r.status(),
        StatusCode::OK,
        "a dangling tagService must not fail the whole download — the plugin would lose \
         its resource policies too"
    );
    let sp: Value =
        serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(
        sp.get("tagPolicies").is_none() || sp["tagPolicies"].is_null(),
        "a dangling tagService carries no tagPolicies block: {sp}"
    );
    assert_eq!(
        sp["serviceName"], dangling,
        "the resource half is still served: {sp}"
    );

    // ── ACCEPT CONTROL: a resolvable `tagService` embeds the block, with the
    // tag service's own def and its enabled policies.
    let tag_name = format!("t9real_{suffix}_tag");
    let tag_svc = store
        .create_service(&tag_name, "tag", &json!({}))
        .await
        .expect("create tag service");
    let linked = format!("t9real_{suffix}");
    insert_legacy_service(&pool, &linked, "trino", &json!({ "tagService": tag_name })).await;
    store
        .create_policy(
            tag_svc.id,
            "tp1",
            &json!({
                "name": "tp1", "policyType": 1, "serviceType": "tag",
                "service": tag_name,
                "resources": {"tag": {"values": ["pii"], "isExcludes": false, "isRecursive": false}},
                "dataMaskPolicyItems": [
                    {"groups": ["analysts"], "accesses": [{"type": "select", "isAllowed": true}],
                     "dataMaskInfo": {"dataMaskType": "MASK_HASH"}}
                ]
            }),
        )
        .await
        .expect("create tag policy");

    let r = app
        .oneshot(req(
            "GET",
            &format!("/plugins/policies/download/{linked}"),
            Some(TOKEN),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let sp: Value =
        serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(
        sp["tagPolicies"]["serviceName"], tag_name,
        "a resolvable tagService must embed the block: {sp}"
    );
    assert_eq!(sp["tagPolicies"]["serviceDef"]["name"], "tag", "{sp}");
    assert_eq!(
        sp["tagPolicies"]["policies"].as_array().map(Vec::len),
        Some(1),
        "the tag service's enabled policies must be in the block: {sp}"
    );
}

/// **The `tagPolicies` block carries the `tag` service-def, whatever the tag
/// service's own `type` is.** Found live on Trino 483 / plugin 2.5.0.
///
/// `download_policies` used to fetch the block's def by `tsvc.r#type`. The admin
/// API (`create_profile`) accepts only `trino` and `starrocks`, so a tag service
/// made through it is a `trino` profile, and the block went out carrying
/// `trino.json`. That def has no `tag` resource, so the plugin logged
/// `RangerDefaultPolicyResourceMatcher.init() failed: policyResources elements
/// are not part of any valid resourcedef hierarchy. (serviceDef=trino,
/// policyResourceKeys=tag` per tag policy and then applied EVERY tag policy to
/// EVERY tagged column — measured against the real jar in
/// `tests/ranger/src/test/java/EgretTagBlockServiceDefTest.java`. Real Ranger
/// Admin always serves this block with the `tag` def.
#[tokio::test]
async fn the_tag_block_carries_the_tag_def_even_for_a_trino_typed_tag_service() {
    let Some(url) = egret_test_db::url().await else {
        eprintln!("skip");
        return;
    };
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let pool = db.pool.clone();
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed trino");
    egret_policy::seed_tag(&store).await.expect("seed tag def");
    let app = ranger_router(RangerState {
        store: store.clone(),
        auth_token: Arc::from(TOKEN),
    });

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    // The shape the admin API produces: the tag service is a `trino` profile.
    let tag_name = format!("tagdef_{suffix}_tag");
    let tag_svc = store
        .create_service(&tag_name, "trino", &json!({}))
        .await
        .expect("create trino-typed tag service");
    let linked = format!("tagdef_{suffix}");
    insert_legacy_service(&pool, &linked, "trino", &json!({ "tagService": tag_name })).await;
    for (name, tag) in [("tp_key", "vgkey"), ("tp_secret", "vgsecret")] {
        store
            .create_policy(
                tag_svc.id,
                name,
                &json!({
                    "name": name, "policyType": 0, "serviceType": "tag",
                    "service": tag_name,
                    "resources": {"tag": {"values": [tag], "isExcludes": false, "isRecursive": false}},
                    "denyPolicyItems": [
                        {"users": ["bob"], "accesses": [{"type": "select", "isAllowed": true}]}
                    ]
                }),
            )
            .await
            .expect("create tag policy");
    }

    let r = app
        .oneshot(req(
            "GET",
            &format!("/plugins/policies/download/{linked}"),
            Some(TOKEN),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let sp: Value =
        serde_json::from_slice(&r.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let block = &sp["tagPolicies"];
    assert_eq!(block["serviceName"], tag_name, "{sp}");
    assert_eq!(
        block["serviceDef"]["name"], "tag",
        "the tag block must carry the `tag` service-def, not the tag service's own \
         type's def: {block}"
    );
    let resources: Vec<&str> = block["serviceDef"]["resources"]
        .as_array()
        .expect("serviceDef.resources")
        .iter()
        .filter_map(|r| r["name"].as_str())
        .collect();
    assert_eq!(
        resources,
        vec!["tag"],
        "the block's def must declare exactly the `tag` resource its policies address"
    );
    // The resource half keeps its own def.
    assert_eq!(sp["serviceDef"]["name"], "trino", "{sp}");
    assert_eq!(block["policies"].as_array().map(Vec::len), Some(2), "{sp}");
}

/// The surface exactly as the server mounts it, anonymous, with one service.
async fn anonymous_surface() -> Option<(axum::Router, String)> {
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
    let app = ranger_surface(RangerState {
        store,
        auth_token: Arc::from(""),
    });
    Some((app, svc))
}

#[tokio::test]
async fn the_secure_path_serves_the_same_body_as_the_plain_path() {
    let Some((app, svc)) = anonymous_surface().await else {
        eprintln!("skip");
        return;
    };
    let plain = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/ranger/service/plugins/policies/download/{svc}"),
            None,
            None,
        ))
        .await
        .unwrap();
    let secure = app
        .oneshot(req(
            "GET",
            &format!("/ranger/service/plugins/secure/policies/download/{svc}"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(plain.status(), StatusCode::OK);
    assert_eq!(secure.status(), StatusCode::OK);
    let body = |r: axum::response::Response| async move {
        let b = r.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice::<Value>(&b).unwrap()
    };
    assert_eq!(body(plain).await, body(secure).await);
}

#[tokio::test]
async fn the_old_root_service_prefix_is_gone() {
    let Some((app, svc)) = anonymous_surface().await else {
        eprintln!("skip");
        return;
    };
    let r = app
        .oneshot(req(
            "GET",
            &format!("/service/plugins/policies/download/{svc}"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}
