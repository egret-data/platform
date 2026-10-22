//! `OidcService` against an in-process mock IdP (discovery + JWKS on a
//! loopback port): the authorize URL it builds, and the claims it reads off a
//! signed id_token. No database, no Keycloak.

use egret_auth::oidc::OidcService;
use egret_machine_idp::{generate_rsa_key, KeyMaterial};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde_json::json;

const CLIENT_ID: &str = "egret";

/// Serve discovery + JWKS for `km` and discover an `OidcService` against it.
async fn mock_idp(km: &KeyMaterial, scopes: &[&str]) -> (OidcService, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let discovery = json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/auth"),
        "token_endpoint": format!("{base}/token"),
        "jwks_uri": format!("{base}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    });
    let jwks = json!({ "keys": [km.public_jwk.clone()] });
    let app = axum::Router::new()
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
                let j = jwks.clone();
                async move { axum::Json(j) }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

    let oidc = OidcService::discover(&egret_config::OidcConfig {
        enabled: true,
        issuer_url: base.clone(),
        client_id: CLIENT_ID.into(),
        client_secret: "mock-secret".into(),
        redirect_url: format!("{base}/api/auth/oidc/callback"),
        scopes: scopes.iter().map(|s| s.to_string()).collect(),
        groups_claim: "groups".into(),
        post_login_redirect: "/".into(),
        catalog_audience: "catalog".into(),
    })
    .await
    .expect("discover against the mock IdP");
    (oidc, base)
}

/// KI-142: the default scopes already name `openid`, and the client library
/// adds it on its own, so the authorize request carried it twice.
#[tokio::test]
async fn the_authorize_request_names_openid_once() {
    let km = generate_rsa_key("mock-kid").expect("rsa key");
    let (oidc, _) = mock_idp(&km, &["openid", "email", "profile"]).await;

    let url = reqwest::Url::parse(&oidc.start().authorize_url).expect("authorize url");
    let scope = url
        .query_pairs()
        .find(|(k, _)| k == "scope")
        .map(|(_, v)| v.into_owned())
        .expect("scope parameter");

    assert_eq!(scope, "openid email profile");
}

fn id_token(km: &KeyMaterial, issuer: &str, extra: serde_json::Value) -> String {
    let now = chrono::Utc::now().timestamp();
    let mut claims = json!({
        "iss": issuer,
        "aud": CLIENT_ID,
        "sub": "sub-1",
        "iat": now,
        "exp": now + 300,
        "email": "someone@example.com",
    });
    for (k, v) in extra.as_object().unwrap() {
        claims[k] = v.clone();
    }
    let mut header = Header::new(jsonwebtoken::Algorithm::RS256);
    header.kid = Some("mock-kid".into());
    encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(km.private_pem.as_bytes()).unwrap(),
    )
    .unwrap()
}

/// KI-185: pre-registration binds on a VERIFIED email only, so the claim must
/// be read as the IdP sent it, and an absent claim must read as unverified.
#[tokio::test]
async fn email_verified_is_read_from_the_id_token_and_defaults_to_false() {
    let km = generate_rsa_key("mock-kid").expect("rsa key");
    let (oidc, issuer) = mock_idp(&km, &["openid", "email"]).await;

    let verified = oidc
        .verify_id_token(
            &id_token(&km, &issuer, json!({"email_verified": true})),
            None,
        )
        .unwrap();
    let unverified = oidc
        .verify_id_token(
            &id_token(&km, &issuer, json!({"email_verified": false})),
            None,
        )
        .unwrap();
    let absent = oidc
        .verify_id_token(&id_token(&km, &issuer, json!({})), None)
        .unwrap();

    assert!(verified.email_verified);
    assert!(!unverified.email_verified);
    assert!(!absent.email_verified);
}
