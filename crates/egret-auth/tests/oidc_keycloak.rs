//! Tier-1 OIDC integration tests against a real Keycloak (via testcontainers).
//!
//! These tests start `quay.io/keycloak/keycloak:26.0` with the `egret` realm
//! imported and exercise the [`OidcService`] against it:
//!   * `ropc_token_validates_and_has_groups` — direct-access-grant (ROPC) → validate id_token.
//!   * `full_auth_code_flow_yields_claims`   — headless auth-code + PKCE + nonce flow.
//!
//! The whole file self-skips (prints a skip message and returns) when Docker is
//! unavailable, so the suite stays green offline. When Docker is available the
//! tests run for real. The first run pulls the Keycloak image (~minutes).
//!
//! Run with: `cargo test -p egret-auth --test oidc_keycloak -- --nocapture --test-threads=1`

use std::path::Path;
use std::time::{Duration, Instant};

use egret_auth::oidc::OidcService;
use egret_config::OidcConfig;
use testcontainers::core::{Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// Keep this handle bound in the test so the container stays alive for the duration.
type Keycloak = ContainerAsync<GenericImage>;

/// Absolute path to the `deploy/keycloak` realm-import directory, canonicalized.
fn realm_import_dir() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../deploy/keycloak")
        .canonicalize()
        .expect("deploy/keycloak must exist")
        .to_str()
        .expect("realm path is valid UTF-8")
        .to_string()
}

/// Start Keycloak with the `egret` realm imported and wait until its discovery
/// endpoint serves HTTP 200. Returns `(issuer_url, container)` or `None` if Docker
/// is unavailable (the caller then self-skips).
///
/// The container handle MUST be kept alive by the caller; dropping it stops the
/// container (testcontainers auto-removes on drop).
async fn start_keycloak() -> Option<(String, Keycloak)> {
    let realm_host = realm_import_dir();

    // Keycloak 26: `start-dev --import-realm`, admin bootstrap via KC_BOOTSTRAP_ADMIN_*.
    // We don't rely on a specific log line for readiness (it varies across versions);
    // instead we wait on a coarse log marker, then poll the discovery endpoint below.
    let image = GenericImage::new("quay.io/keycloak/keycloak", "26.0")
        .with_exposed_port(8080.into())
        .with_wait_for(WaitFor::message_on_stdout("Running the server"))
        .with_cmd(["start-dev", "--import-realm"])
        .with_env_var("KC_BOOTSTRAP_ADMIN_USERNAME", "admin")
        .with_env_var("KC_BOOTSTRAP_ADMIN_PASSWORD", "admin")
        .with_mount(Mount::bind_mount(realm_host, "/opt/keycloak/data/import"))
        .with_startup_timeout(Duration::from_secs(120));

    let container = match image.start().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("keycloak/Docker unavailable, skipping: {e}");
            return None;
        }
    };

    let port = match container.get_host_port_ipv4(8080).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("could not resolve keycloak host port, skipping: {e}");
            return None;
        }
    };

    let issuer = format!("http://localhost:{port}/realms/egret");
    if let Err(e) = wait_for_discovery(&issuer).await {
        eprintln!("keycloak discovery never became ready, skipping: {e}");
        return None;
    }
    Some((issuer, container))
}

/// Poll `{issuer}/.well-known/openid-configuration` until it returns HTTP 200 or a
/// timeout elapses.
async fn wait_for_discovery(issuer: &str) -> Result<(), String> {
    let url = format!("{issuer}/.well-known/openid-configuration");
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Ok(resp) = http.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out waiting for {url}"));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn oidc_cfg(issuer: &str) -> OidcConfig {
    OidcConfig {
        enabled: true,
        issuer_url: issuer.into(),
        client_id: "egret".into(),
        client_secret: "egret-secret".into(),
        redirect_url: "http://localhost:8080/api/auth/oidc/callback".into(),
        // NB: the realm attaches the `groups` mapper directly to the `egret` client
        // (not via a `groups` client scope), so the claim is emitted without requesting
        // a `groups` scope. Requesting one would fail with `invalid_scope`.
        scopes: vec!["openid".into(), "email".into(), "profile".into()],
        groups_claim: "groups".into(),
        post_login_redirect: "/".into(),
    }
}

#[tokio::test]
async fn ropc_token_validates_and_has_groups() {
    let Some((issuer, _kc)) = start_keycloak().await else {
        return;
    };
    let svc = OidcService::discover(&oidc_cfg(&issuer))
        .await
        .expect("discover");

    // Direct-access-grant (ROPC) to obtain a real id_token.
    let token_url = format!("{issuer}/protocol/openid-connect/token");
    let resp: serde_json::Value = reqwest::Client::new()
        .post(&token_url)
        .form(&[
            ("grant_type", "password"),
            ("client_id", "egret"),
            ("client_secret", "egret-secret"),
            ("username", "tester"),
            ("password", "pw"),
            ("scope", "openid email profile"),
        ])
        .send()
        .await
        .expect("token req")
        .json()
        .await
        .expect("token json");
    let id_token = resp["id_token"]
        .as_str()
        .unwrap_or_else(|| panic!("no id_token in token response: {resp}"));

    let claims = svc.verify_id_token(id_token, None).expect("validate");
    assert_eq!(claims.email, "tester@egret.local");
    assert!(
        claims.groups.iter().any(|g| g == "data-team"),
        "expected `data-team` in groups, got {:?}",
        claims.groups
    );
}

#[tokio::test]
async fn full_auth_code_flow_yields_claims() {
    let Some((issuer, _kc)) = start_keycloak().await else {
        return;
    };
    let svc = OidcService::discover(&oidc_cfg(&issuer))
        .await
        .expect("discover");
    let start = svc.start();

    // reqwest client with a cookie jar, redirects disabled so we can read Location.
    let http = reqwest::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // 1. GET the authorize URL → Keycloak login page (HTML with a form action).
    let get_resp = http.get(&start.authorize_url).send().await.unwrap();
    let get_status = get_resp.status();
    let login_html = get_resp.text().await.unwrap();
    let action = extract_form_action(&login_html)
        .unwrap_or_else(|| panic!("login form action not found; GET authorize -> {get_status}"));

    // 2. POST credentials to the form action → 302 to redirect_uri?code=...&state=...
    let resp = http
        .post(&action)
        .form(&[("username", "tester"), ("password", "pw")])
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_redirection(),
        "expected a 3xx redirect after login POST, got {} (body: {})",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
    let location = resp
        .headers()
        .get("location")
        .expect("redirect Location header")
        .to_str()
        .unwrap()
        .to_string();
    let code = extract_query_param(&location, "code").expect("code");
    let state = extract_query_param(&location, "state").expect("state");
    assert_eq!(state, start.state, "state echoed back");

    // 3. finish: exchange code (PKCE) + validate id_token (nonce).
    let login = svc
        .finish(&code, &start.pkce_verifier, &start.nonce)
        .await
        .expect("finish");
    let claims = login.claims;
    assert_eq!(claims.email, "tester@egret.local");
    assert!(
        claims.groups.iter().any(|g| g == "data-team"),
        "expected `data-team` in groups, got {:?}",
        claims.groups
    );
}

/// Extract the `action="..."` URL from Keycloak's login form. The value may contain
/// HTML-escaped `&amp;`; un-escape those back to `&`.
fn extract_form_action(html: &str) -> Option<String> {
    let i = html.find("action=\"")? + "action=\"".len();
    let rest = &html[i..];
    let j = rest.find('"')?;
    Some(rest[..j].replace("&amp;", "&"))
}

/// Extract a single query parameter value from a URL's query string.
fn extract_query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split('?').nth(1)?;
    for pair in q.split('&') {
        let mut it = pair.splitn(2, '=');
        if it.next() == Some(key) {
            return it.next().map(|v| v.to_string());
        }
    }
    None
}
