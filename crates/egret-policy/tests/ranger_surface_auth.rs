//! Authentication of the Ranger plugin surface (`/ranger/service/...`).
//!
//! DB-gated: self-skip without DATABASE_URL ONLY under `EGRET_ALLOW_DB_SKIP`,
//! and otherwise FAIL (`egret_test_db::url`). A stub verifier stands in for the
//! engine-credential table, so these pin the middleware + profile binding only.
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use egret_engine_mgmt_api::credential::{EngineCredentialVerifier, EngineIdentity};
use egret_metastore::Db;
use egret_policy::ranger::auth::AuthMode;
use egret_policy::{ranger_surface, seed_tag, seed_trino, PolicyStore, RangerState};
use http_body_util::BodyExt;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

const TOKEN: &str = "operator-token";
const ENGINE_PW: &str = "s3cret-engine-password";

struct Stub {
    good: (Uuid, &'static str),
    bound_to: Option<i64>,
}

#[async_trait::async_trait]
impl EngineCredentialVerifier for Stub {
    async fn verify(&self, id: Uuid, pw: &str) -> Result<Option<EngineIdentity>, String> {
        Ok(
            (id == self.good.0 && pw == self.good.1).then_some(EngineIdentity {
                engine_id: id,
                authz_service_id: self.bound_to,
            }),
        )
    }
}

fn basic(user: &str, pw: &str) -> String {
    use base64::Engine as _;
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pw}"))
    )
}

fn engine_user(id: Uuid) -> String {
    egret_engine_mgmt_api::credential::plugin_username(id)
}

/// A store with the trino + tag defs seeded and one trino profile; returns its name + id.
async fn store_with_profile() -> Option<(PolicyStore, String, i64)> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    let store = PolicyStore::new(db);
    seed_trino(&store).await.expect("seed trino");
    seed_tag(&store).await.expect("seed tag");
    let name = format!("trino_{}", Uuid::new_v4().simple());
    let row = store
        .create_service(&name, "trino", &json!({}))
        .await
        .expect("create service");
    Some((store, name, row.id))
}

fn surface(
    store: PolicyStore,
    mode: AuthMode,
    verifier: Option<Arc<dyn EngineCredentialVerifier>>,
) -> axum::Router {
    ranger_surface(RangerState {
        store,
        auth_token: Arc::from(TOKEN),
        mode,
        verifier,
    })
}

fn get(uri: &str, authorization: Option<&str>) -> Request<Body> {
    let mut b = Request::builder().method("GET").uri(uri);
    if let Some(a) = authorization {
        b = b.header(header::AUTHORIZATION, a);
    }
    b.body(Body::empty()).unwrap()
}

fn download(svc: &str) -> String {
    format!("/ranger/service/plugins/secure/policies/download/{svc}")
}

fn assert_basic_challenge(r: &axum::response::Response) {
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        r.headers()
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Basic realm=\"egret-ranger\""),
        "a 401 must carry the Basic challenge"
    );
}

#[tokio::test]
async fn required_mode_refuses_anonymous() {
    let Some((store, svc, _)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let r = surface(store, AuthMode::Required, None)
        .oneshot(get(&download(&svc), None))
        .await
        .unwrap();
    assert_basic_challenge(&r);
}

#[tokio::test]
async fn required_mode_accepts_a_valid_engine_basic_credential() {
    let Some((store, svc, id)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let engine = Uuid::new_v4();
    let v = Arc::new(Stub {
        good: (engine, ENGINE_PW),
        bound_to: Some(id),
    });
    let app = surface(store, AuthMode::Required, Some(v));
    let auth = basic(&engine_user(engine), ENGINE_PW);
    let r = app
        .clone()
        .oneshot(get(&download(&svc), Some(&auth)))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // The tag download for its own profile is served too.
    let r = app
        .oneshot(get(
            &format!("/ranger/service/tags/download/{svc}"),
            Some(&auth),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_wrong_password_is_401() {
    let Some((store, svc, id)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let engine = Uuid::new_v4();
    let v: Arc<dyn EngineCredentialVerifier> = Arc::new(Stub {
        good: (engine, ENGINE_PW),
        bound_to: Some(id),
    });
    let app = surface(store, AuthMode::Required, Some(v));
    let cases = [
        basic(&engine_user(engine), "wrong"),
        basic(&engine_user(Uuid::new_v4()), ENGINE_PW),
        basic("admin", ENGINE_PW),
        "Basic !!!not-base64!!!".to_string(),
        "Bearer wrong-token".to_string(),
    ];
    let mut bodies = Vec::new();
    for auth in cases {
        let r = app
            .clone()
            .oneshot(get(&download(&svc), Some(&auth)))
            .await
            .unwrap();
        assert_basic_challenge(&r);
        bodies.push(r.into_body().collect().await.unwrap().to_bytes());
    }
    // No hint which part was wrong: every refusal has the same body.
    assert!(bodies.windows(2).all(|w| w[0] == w[1]), "{bodies:?}");
}

#[tokio::test]
async fn an_engine_credential_for_another_profile_is_403() {
    let Some((store, svc, id)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let other = store
        .create_service(
            &format!("trino_{}", Uuid::new_v4().simple()),
            "trino",
            &json!({}),
        )
        .await
        .expect("create other");
    assert_ne!(other.id, id);
    let engine = Uuid::new_v4();
    let v = Arc::new(Stub {
        good: (engine, ENGINE_PW),
        bound_to: Some(other.id),
    });
    let app = surface(store.clone(), AuthMode::Required, Some(v));
    let auth = basic(&engine_user(engine), ENGINE_PW);
    for uri in [
        download(&svc),
        format!("/ranger/service/plugins/policies/download/{svc}"),
        format!("/ranger/service/tags/download/{svc}"),
    ] {
        let r = app.clone().oneshot(get(&uri, Some(&auth))).await.unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{uri}");
    }
    // An engine bound to no profile reads nothing.
    let engine2 = Uuid::new_v4();
    let unbound = surface(
        store,
        AuthMode::Required,
        Some(Arc::new(Stub {
            good: (engine2, ENGINE_PW),
            bound_to: None,
        })),
    );
    let r = unbound
        .oneshot(get(
            &download(&svc),
            Some(&basic(&engine_user(engine2), ENGINE_PW)),
        ))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn an_engine_credential_cannot_call_the_crud_routes() {
    let Some((store, _svc, id)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let engine = Uuid::new_v4();
    let v = Arc::new(Stub {
        good: (engine, ENGINE_PW),
        bound_to: Some(id),
    });
    let app = surface(store, AuthMode::Required, Some(v));
    let auth = basic(&engine_user(engine), ENGINE_PW);
    for uri in [
        "/ranger/service/plugins/services",
        "/ranger/service/plugins/definitions",
        "/ranger/service/plugins/definitions/name/trino",
    ] {
        let r = app.clone().oneshot(get(uri, Some(&auth))).await.unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{uri}");
    }
}

#[tokio::test]
async fn the_operator_bearer_still_works_everywhere() {
    let Some((store, svc, _)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let app = surface(store, AuthMode::Required, None);
    let auth = format!("Bearer {TOKEN}");
    for uri in [
        download(&svc),
        format!("/ranger/service/tags/download/{svc}"),
        "/ranger/service/plugins/services".to_string(),
    ] {
        let r = app.clone().oneshot(get(&uri, Some(&auth))).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
async fn an_empty_operator_token_never_matches_an_empty_bearer() {
    let Some((store, svc, _)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let app = ranger_surface(RangerState {
        store,
        auth_token: Arc::from(""),
        mode: AuthMode::Required,
        verifier: None,
    });
    let r = app
        .oneshot(get(&download(&svc), Some("Bearer ")))
        .await
        .unwrap();
    assert_basic_challenge(&r);
}

#[tokio::test]
async fn anonymous_mode_serves_without_credentials() {
    let Some((store, svc, _)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let app = surface(store, AuthMode::Anonymous, None);
    let r = app
        .clone()
        .oneshot(get(&download(&svc), None))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // Anonymous mode serves everyone, a bad credential included.
    let r = app
        .oneshot(get(&download(&svc), Some(&basic("engine-x", "nope"))))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_unserved_route_under_the_surface_is_401_not_404_without_credentials() {
    let Some((store, svc, id)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let engine = Uuid::new_v4();
    let v = Arc::new(Stub {
        good: (engine, ENGINE_PW),
        bound_to: Some(id),
    });
    let app = surface(store, AuthMode::Required, Some(v));
    // Measured: a 2.8.0 plugin polls this; egret does not serve it.
    let uri = format!("/ranger/service/roles/secure/download/{svc}");
    let r = app.clone().oneshot(get(&uri, None)).await.unwrap();
    assert_basic_challenge(&r);
    let r = app
        .clone()
        .oneshot(get(&uri, Some(&basic(&engine_user(engine), "wrong"))))
        .await
        .unwrap();
    assert_basic_challenge(&r);
    let r = app
        .oneshot(get(&uri, Some(&basic(&engine_user(engine), ENGINE_PW))))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_tag_service_of_the_bound_profile_is_allowed() {
    let Some((store, _svc, _)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let suffix = Uuid::new_v4().simple().to_string();
    let tag = store
        .create_service(&format!("p_{suffix}_tag"), "tag", &json!({}))
        .await
        .expect("create tag service");
    let unrelated = store
        .create_service(&format!("u_{suffix}_tag"), "tag", &json!({}))
        .await
        .expect("create unrelated tag service");
    let profile = store
        .create_service(
            &format!("p_{suffix}"),
            "trino",
            &json!({ "tagService": tag.name }),
        )
        .await
        .expect("create profile");
    let engine = Uuid::new_v4();
    let v = Arc::new(Stub {
        good: (engine, ENGINE_PW),
        bound_to: Some(profile.id),
    });
    let app = surface(store, AuthMode::Required, Some(v));
    let auth = basic(&engine_user(engine), ENGINE_PW);
    let status = |uri: String| {
        let app = app.clone();
        let auth = auth.clone();
        async move { app.oneshot(get(&uri, Some(&auth))).await.unwrap().status() }
    };
    assert_eq!(status(download(&profile.name)).await, StatusCode::OK);
    assert_eq!(status(download(&tag.name)).await, StatusCode::OK);
    assert_eq!(
        status(download(&unrelated.name)).await,
        StatusCode::FORBIDDEN
    );
}

/// Anonymous mode serves everyone, but a VALID engine credential still makes
/// the caller that engine: bound to profile X, it reads X (200) and is refused
/// profile Y (403), while a request with no credential reads both.
#[tokio::test]
async fn anonymous_mode_still_binds_a_valid_engine_credential_to_its_profile() {
    let Some((store, x, x_id)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let y = store
        .create_service(
            &format!("trino_{}", Uuid::new_v4().simple()),
            "trino",
            &json!({}),
        )
        .await
        .expect("create other profile");
    let engine = Uuid::new_v4();
    let v = Arc::new(Stub {
        good: (engine, ENGINE_PW),
        bound_to: Some(x_id),
    });
    let app = surface(store, AuthMode::Anonymous, Some(v));
    let auth = basic(&engine_user(engine), ENGINE_PW);
    let status = |uri: String, auth: Option<String>| {
        let app = app.clone();
        async move {
            app.oneshot(get(&uri, auth.as_deref()))
                .await
                .unwrap()
                .status()
        }
    };
    assert_eq!(
        status(download(&x), Some(auth.clone())).await,
        StatusCode::OK
    );
    assert_eq!(
        status(download(&y.name), Some(auth.clone())).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(
            format!("/ranger/service/tags/download/{}", y.name),
            Some(auth)
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(status(download(&x), None).await, StatusCode::OK);
    assert_eq!(status(download(&y.name), None).await, StatusCode::OK);
}

/// The secure TAG path a 2.8.0+ plugin with a username and password calls
/// (measured, `2026-10-20-ranger-surface-auth-measurements.md` Q4: `GET
/// /service/tags/secure/download/<svc>` with `Authorization: Basic ...`) gets
/// the same authentication and profile binding as every other download: 401
/// without a credential, 200 for the bound profile and the tag service it
/// names, 403 for anything else, and nothing outside `/ranger`.
#[tokio::test]
async fn the_secure_tags_path_is_authenticated_and_bound_to_the_profile() {
    let Some((store, _svc, _)) = store_with_profile().await else {
        eprintln!("skip");
        return;
    };
    let suffix = Uuid::new_v4().simple().to_string();
    let tag = store
        .create_service(&format!("p_{suffix}_tag"), "tag", &json!({}))
        .await
        .expect("create tag service");
    let unrelated_tag = store
        .create_service(&format!("u_{suffix}_tag"), "tag", &json!({}))
        .await
        .expect("create unrelated tag service");
    let other = store
        .create_service(&format!("o_{suffix}"), "trino", &json!({}))
        .await
        .expect("create other profile");
    let profile = store
        .create_service(
            &format!("p_{suffix}"),
            "trino",
            &json!({ "tagService": tag.name }),
        )
        .await
        .expect("create profile");
    let engine = Uuid::new_v4();
    let v = Arc::new(Stub {
        good: (engine, ENGINE_PW),
        bound_to: Some(profile.id),
    });
    let app = surface(store, AuthMode::Required, Some(v));
    let auth = basic(&engine_user(engine), ENGINE_PW);
    let secure_tags = |svc: &str| format!("/ranger/service/tags/secure/download/{svc}");

    let r = app
        .clone()
        .oneshot(get(&secure_tags(&profile.name), None))
        .await
        .unwrap();
    assert_basic_challenge(&r);
    let r = app
        .clone()
        .oneshot(get(
            &secure_tags(&profile.name),
            Some(&basic(&engine_user(engine), "wrong")),
        ))
        .await
        .unwrap();
    assert_basic_challenge(&r);

    let status = |uri: String| {
        let app = app.clone();
        let auth = auth.clone();
        async move { app.oneshot(get(&uri, Some(&auth))).await.unwrap().status() }
    };
    assert_eq!(status(secure_tags(&profile.name)).await, StatusCode::OK);
    assert_eq!(status(secure_tags(&tag.name)).await, StatusCode::OK);
    assert_eq!(
        status(secure_tags(&unrelated_tag.name)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status(secure_tags(&other.name)).await,
        StatusCode::FORBIDDEN
    );
    // Only under `/ranger`: the old root `/service/...` prefix serves nothing.
    assert_eq!(
        status(format!("/service/tags/secure/download/{}", profile.name)).await,
        StatusCode::NOT_FOUND
    );
}
