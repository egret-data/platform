//! DB-gated integration tests for OIDC Back-Channel Logout 1.0 (BCL-T2).
//!
//! The real Keycloak isn't available here, so these tests exercise the exact
//! chain the `POST /api/auth/oidc/backchannel-logout` handler runs:
//! `verify_logout_token` (signed against a test JWKS) → `sub` →
//! `find_by_external_subject` → `revoke_all_for_user`. Skip (pass) when
//! DATABASE_URL is unset, matching the sibling `*_db.rs` tests in this crate.

use egret_auth::oidc::verify_logout_token;
use egret_auth::session::SessionService;
use egret_auth::users::UserRepo;
use egret_machine_idp::{generate_rsa_key, KeyMaterial};
use egret_metastore::Db;
use jsonwebtoken::{encode, jwk::JwkSet, EncodingKey, Header};
use serde_json::json;

#[path = "support/recorder.rs"]
mod recorder;

const ISSUER: &str = "https://keycloak.example.com/realms/egret";
const AUDIENCE: &str = "egret-backend";

async fn connect() -> Option<Db> {
    let url = egret_test_db::url().await?;
    let db = Db::connect(&url, 5).await.expect("connect");
    db.migrate().await.expect("migrate");
    Some(db)
}

fn unique(tag: &str) -> String {
    format!("{tag}-{}", uuid::Uuid::new_v4().simple())
}

/// Build a fresh RSA keypair + the JWKS an issuer would publish for it.
fn keypair() -> (KeyMaterial, JwkSet) {
    let km = generate_rsa_key("test-kid").expect("generate rsa key");
    let jwks: JwkSet =
        serde_json::from_value(json!({ "keys": [km.public_jwk] })).expect("parse jwks");
    (km, jwks)
}

/// Sign a valid back-channel logout token for `sub`.
fn sign_logout_token(km: &KeyMaterial, sub: &str) -> String {
    let claims = json!({
        "iss": ISSUER,
        "aud": AUDIENCE,
        "sub": sub,
        "iat": chrono::Utc::now().timestamp(),
        "exp": (chrono::Utc::now() + chrono::Duration::minutes(5)).timestamp(),
        "jti": unique("logout-jti"),
        "events": {
            "http://schemas.openid.net/event/backchannel-logout": {}
        }
    });
    let mut header = Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some(km.kid.clone());
    let key = EncodingKey::from_rsa_pem(km.private_pem.as_bytes()).expect("encoding key");
    encode(&header, &claims, &key).expect("sign token")
}

/// The full chain: a valid logout token for a known `sub` revokes every
/// session belonging to that user.
#[tokio::test]
async fn valid_logout_token_revokes_user_sessions() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let users = UserRepo::new(db.clone());
    let sessions = SessionService::new(
        db.clone(),
        chrono::Duration::hours(24),
        chrono::Duration::hours(1),
    );

    let sub = unique("kc-sub");
    let email = format!("{}@idp.local", unique("u"));
    let uid = users.insert_oidc(&sub, &email, "U").await.unwrap();
    let tok = sessions.create(uid, "", "").await.unwrap();
    assert!(
        sessions.validate(&tok.token).await.unwrap().is_some(),
        "session valid before logout"
    );

    let (km, jwks) = keypair();
    let logout_token = sign_logout_token(&km, &sub);

    let verified_sub = verify_logout_token(&jwks, ISSUER, AUDIENCE, &logout_token).unwrap();
    assert_eq!(verified_sub, sub);

    let user = users
        .find_by_external_subject(&verified_sub)
        .await
        .unwrap()
        .expect("user found by sub");
    assert_eq!(user.id, uid);

    sessions.revoke_all_for_user(user.id).await.unwrap();

    assert!(
        sessions.validate(&tok.token).await.unwrap().is_none(),
        "session revoked after back-channel logout"
    );
}

/// A logout token for a `sub` egret has never provisioned is a no-op: lookup
/// returns `None` and there is nothing to revoke (the handler still returns
/// 200, matching the spec's "don't leak whether we know this subject").
#[tokio::test]
async fn unknown_sub_is_noop() {
    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };
    let users = UserRepo::new(db.clone());

    let (km, jwks) = keypair();
    let logout_token = sign_logout_token(&km, "never-seen-sub");
    let verified_sub = verify_logout_token(&jwks, ISSUER, AUDIENCE, &logout_token).unwrap();

    let found = users.find_by_external_subject(&verified_sub).await.unwrap();
    assert!(found.is_none(), "unknown sub has no matching user");
}

/// A tampered/invalid logout token (bad signature) must fail verification
/// before any lookup or revoke is attempted.
#[tokio::test]
async fn invalid_logout_token_is_rejected() {
    let (_km, jwks) = keypair();
    let (other_km, _other_jwks) = keypair();
    // Signed by a key not in `jwks` (kid collision reused deliberately so the
    // lookup succeeds but signature verification fails).
    let logout_token = sign_logout_token(&other_km, "user-x");
    assert!(verify_logout_token(&jwks, ISSUER, AUDIENCE, &logout_token).is_err());
}

/// HTTP-level test of the actual axum handler through `oidc_router()`: a POST
/// carrying a malformed `logout_token` must get **400** with
/// `Cache-Control: no-store` — the response contract the chain tests above don't
/// exercise (status code, header, and form decoding).
///
/// To construct a real `OidcService` (its only constructor does live discovery)
/// without Docker/Keycloak, we stand up a tiny in-process mock IdP serving the
/// discovery document + a JWKS. `provider_jwks()` then succeeds and
/// `verify_logout_token` rejects the garbage token → 400 + no-store.
#[tokio::test]
async fn handler_returns_400_no_store_on_malformed_token() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    let Some(db) = connect().await else {
        eprintln!("SKIPPED, asserted nothing — no DATABASE_URL, EGRET_ALLOW_DB_SKIP is set");
        return;
    };

    // --- tiny mock IdP: discovery + JWKS on a random loopback port ---
    let km = generate_rsa_key("mock-kid").expect("rsa key");
    let jwk_value = km.public_jwk.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let base = format!("http://{addr}");
    let issuer = base.clone();
    let discovery = json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{base}/auth"),
        "token_endpoint": format!("{base}/token"),
        "jwks_uri": format!("{base}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    });
    let jwks_doc = json!({ "keys": [jwk_value] });
    let mock = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(move || {
                let d = discovery.clone();
                async move { axum::Json(d) }
            }),
        )
        .route(
            "/jwks",
            axum::routing::get(move || {
                let j = jwks_doc.clone();
                async move { axum::Json(j) }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, mock).await.expect("mock idp serve");
    });

    // --- real OidcService discovered against the mock ---
    let oidc = egret_auth::oidc::OidcService::discover(&egret_config::OidcConfig {
        enabled: true,
        issuer_url: issuer,
        client_id: AUDIENCE.into(),
        client_secret: "mock-secret".into(),
        redirect_url: format!("{base}/api/auth/oidc/callback"),
        scopes: vec!["openid".into()],
        groups_claim: "groups".into(),
        post_login_redirect: "/".into(),
        catalog_audience: "catalog".into(),
    })
    .await
    .expect("discover against mock idp");

    let state = egret_auth::oidc_routes::OidcRoutesState {
        oidc,
        users: UserRepo::new(db.clone()),
        groups: egret_auth::groups::GroupRepo::new(db.clone()),
        sessions: SessionService::new(
            db.clone(),
            chrono::Duration::hours(24),
            chrono::Duration::hours(1),
        ),
        config: egret_config::Config::default(),
        cookie_key: axum_extra::extract::cookie::Key::generate(),
        impersonations: egret_auth::impersonation::ImpersonationRepo::new(db.clone()),
        audit: egret_audit::AuditSink::new(16).0,
        group_recompiler: std::sync::Arc::new(crate::recorder::Recorder::default()),
        invites: std::sync::Arc::new(crate::recorder::NoInvites),
    };
    let app = egret_auth::oidc_routes::oidc_router().with_state(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/backchannel-logout")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from("logout_token=not-a-valid-jwt"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "malformed logout token → 400"
    );
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store"),
        "back-channel logout responses must set Cache-Control: no-store"
    );
}
