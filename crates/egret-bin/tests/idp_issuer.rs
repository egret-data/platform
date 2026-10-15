//! The machine-IdP's identity, driven end to end through the real router.
//!
//! The property under test is the one OIDC Discovery actually requires and the
//! one this deployment got wrong: **the `iss` of a minted token, the `issuer`
//! in the discovery document, and the URL the document was fetched from are all
//! the same string.** Asserting any two of those in isolation passes while the
//! document still names a host no client outside the cluster can resolve —
//! which is exactly the state `/.well-known/openid-configuration` was in, and
//! why these are asserted together.
//!
//! Not DB-gated: the signing key and the client registry are fakes, so this
//! file asserts something on every run rather than reporting `ok` without a
//! `DATABASE_URL`.
//!
//! The fixtures give `idp.issuer` and `catalog.public_url` DIFFERENT values on
//! purpose. Equal, an implementation that ignored the new key entirely would
//! pass every assertion here.

use axum::body::Body;
use axum::http::Request;
use egret_bin::identity::EgretIdentity;
use egret_config::Config;
use egret_core::ratelimit::RateLimiter;
use egret_machine_idp::{
    generate_rsa_key, idp_router, ClientState, ClientVerifier, IdpError, IdpState, KeyMaterial,
    SigningKeyStore, VerifiedClient,
};
use std::sync::Arc;
use tower::ServiceExt as _;

/// What a client outside the cluster resolves — an ingress hostname.
const EXTERNAL: &str = "http://egret.localtest.me";
/// What an engine pod resolves — a Service DNS name, unreachable from outside.
const IN_CLUSTER: &str = "http://egret.egret-system.svc.cluster.local:8080";

struct FakeKeys(KeyMaterial);

#[async_trait::async_trait]
impl SigningKeyStore for FakeKeys {
    async fn active(&self) -> Result<KeyMaterial, IdpError> {
        Ok(KeyMaterial {
            kid: self.0.kid.clone(),
            private_pem: self.0.private_pem.clone(),
            public_jwk: self.0.public_jwk.clone(),
        })
    }
    async fn all_public_jwks(&self) -> Result<Vec<serde_json::Value>, IdpError> {
        Ok(vec![self.0.public_jwk.clone()])
    }
}

/// Stands in for the Catalog Client registry: recognises one machine client.
struct OneClient;

#[async_trait::async_trait]
impl ClientVerifier for OneClient {
    async fn verify(&self, id: &str, secret: &str) -> Result<Option<VerifiedClient>, IdpError> {
        if secret == "sec" {
            Ok(self.current(id).await?.map(|s| s.verified))
        } else {
            Ok(None)
        }
    }
    async fn current(&self, id: &str) -> Result<Option<ClientState>, IdpError> {
        Ok((id == "cid").then(|| ClientState {
            verified: VerifiedClient {
                client_id: id.to_string(),
                audience: "catalog".into(),
                extra_claims: serde_json::Map::new(),
                ttl_secs: 3600,
            },
            secret_hash: "$argon2id$v=19$m=19456,t=2,p=1$b25l$b25lLWNsaWVudA".into(),
        }))
    }
}

/// A config shaped like a deployment behind an ingress: the engines' address and
/// egret's identity are two different hosts.
fn config(idp_issuer: Option<&str>) -> Config {
    let mut cfg = Config::default();
    cfg.catalog.public_url = Some(IN_CLUSTER.to_string());
    cfg.idp.issuer = idp_issuer.map(str::to_string);
    cfg
}

/// The Data API's consumer origin defaults to the ISSUER, not to the in-cluster
/// address.
///
/// The two land side by side in one document a consumer receives: `tokenUrl`
/// and the token's `iss` come from the issuer, `servers[0].url` and
/// `token_endpoint` come from the consumer origin. Defaulting the second to the
/// Service DNS name means a deployment that sets `EGRET_IDP__ISSUER` and
/// nothing else — the exact deployment the new key exists for — advertises an
/// externally-valid issuer beside a `token_endpoint` on a host the consumer
/// cannot resolve. Not a URL that merely looks odd: a document that cannot be
/// used, because fetching the token is the first thing it asks for.
///
/// The fixture's issuer and in-cluster address are different hosts on purpose;
/// equal, this test would pass with the fallback pointing either way.
#[tokio::test]
async fn the_consumer_origin_defaults_to_the_issuer_not_the_in_cluster_address() {
    let identity = EgretIdentity::from_config(&config(Some(EXTERNAL)));

    assert_eq!(
        identity.consumer_base_url(),
        EXTERNAL,
        "a consumer must be sent to the same origin the issuer names"
    );
    assert_ne!(
        identity.consumer_base_url(),
        identity.in_cluster_base_url(),
        "the consumer origin must not fall back to the address engines dial"
    );
}

/// With `data_api.public_url` set it still wins, over the issuer as it used to
/// over the in-cluster address. The two audiences are allowed to differ; only
/// the DEFAULT moved.
#[tokio::test]
async fn an_explicit_consumer_origin_still_beats_the_issuer() {
    let mut cfg = config(Some(EXTERNAL));
    cfg.data_api.public_url = Some("https://api.example.test/".into());
    let identity = EgretIdentity::from_config(&cfg);

    assert_eq!(identity.consumer_base_url(), "https://api.example.test");
    assert_eq!(
        identity.issuer(),
        EXTERNAL,
        "the issuer is not dragged along"
    );
}

/// With neither key set every value is what it was before either existed —
/// including the consumer origin, which is where an existing deployment's
/// generated OpenAPI documents and issued `token_endpoint`s already point.
#[tokio::test]
async fn without_either_key_the_consumer_origin_is_the_in_cluster_address() {
    let identity = EgretIdentity::from_config(&config(None));

    assert_eq!(identity.consumer_base_url(), IN_CLUSTER);
}

fn idp_state(identity: &EgretIdentity) -> IdpState {
    let km = generate_rsa_key("kid-t").expect("generate signing key");
    identity.idp_state(
        Arc::new(FakeKeys(km)),
        vec![Arc::new(OneClient)],
        // Long window, high ceiling: these tests are about identity, and must
        // never race the limiter.
        RateLimiter::new(1_000, 3_600, 3_600),
    )
}

/// Fetch the discovery document from `url`, through the real router, with the
/// `Host` header a client asking at that address would send. The URL it was
/// fetched from is returned alongside the document so a test cannot compare the
/// document against a value it made up separately.
async fn fetch_discovery(st: IdpState, origin: &str) -> (String, serde_json::Value) {
    let url = format!("{origin}/.well-known/openid-configuration");
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .expect("fixture url has a host");
    let resp = idp_router()
        .with_state(st)
        .oneshot(
            Request::builder()
                .uri(&url)
                .header("host", host)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("discovery request");
    assert_eq!(resp.status(), 200, "discovery must be served");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (url, serde_json::from_slice(&bytes).unwrap())
}

/// Mint a token through the real `POST /oauth/token` and return its `iss`.
async fn minted_iss(st: IdpState) -> String {
    let resp = idp_router()
        .with_state(st)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/oauth/token")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(
                    "grant_type=client_credentials&client_id=cid&client_secret=sec",
                ))
                .unwrap(),
        )
        .await
        .expect("token request");
    assert_eq!(resp.status(), 200, "the token endpoint must mint");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let jwt = body["access_token"].as_str().expect("access_token");
    claims_of(jwt)["iss"]
        .as_str()
        .expect("iss claim")
        .to_string()
}

/// A JWT's claims, unverified — this file asserts on the payload the IdP built,
/// which is independent of the signature.
fn claims_of(jwt: &str) -> serde_json::Value {
    use base64::Engine as _;
    let payload = jwt.split('.').nth(1).expect("jwt payload segment");
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .expect("base64url payload");
    serde_json::from_slice(&raw).expect("json payload")
}

/// **The invariant this whole change is about.** A conforming OIDC client
/// fetches the document from an address, reads `issuer`, and rejects the
/// provider unless that `issuer` equals both the address it asked at and the
/// `iss` of the tokens it later receives. All three, in one test, from one
/// config — because the failure mode is precisely two of them agreeing while
/// the third does not.
#[tokio::test]
async fn the_iss_the_document_and_the_address_it_was_served_from_are_one_value() {
    let identity = EgretIdentity::from_config(&config(Some(EXTERNAL)));

    let (fetched_from, doc) = fetch_discovery(idp_state(&identity), EXTERNAL).await;
    let iss = minted_iss(idp_state(&identity)).await;

    let advertised = doc["issuer"].as_str().expect("issuer in the document");
    let origin_asked = fetched_from
        .strip_suffix("/.well-known/openid-configuration")
        .expect("the document is fetched from the well-known path");

    assert_eq!(
        advertised, origin_asked,
        "the document must name the address it was fetched from"
    );
    assert_eq!(
        iss, advertised,
        "a minted token's `iss` must be the issuer the document advertises"
    );
    // And the guard that makes the two above falsifiable: an implementation
    // ignoring `idp.issuer` would answer with the in-cluster address here.
    assert_ne!(
        advertised, IN_CLUSTER,
        "the identity must follow `idp.issuer`, not `catalog.public_url`"
    );
    assert_eq!(
        doc["token_endpoint"],
        serde_json::json!(format!("{EXTERNAL}/oauth/token")),
        "every endpoint in the document must be reachable from where it was fetched"
    );
}

/// Two JWKS URLs, deliberately different, and a test that can tell them apart.
///
/// The one INSIDE the discovery document is dialled by an external client, so it
/// follows the issuer. `engine_jwks_uri` is dialled by an engine pod, so it stays
/// on the Service DNS name: the keys are identical either way, and hairpinning
/// the data plane's key fetches out through the ingress and back would make
/// every query depend on the ingress being healthy. A test that asserted only
/// one of them would let the other drift silently.
#[tokio::test]
async fn the_documents_jwks_uri_follows_the_issuer_while_the_engines_stays_in_cluster() {
    let identity = EgretIdentity::from_config(&config(Some(EXTERNAL)));

    let (_, doc) = fetch_discovery(idp_state(&identity), EXTERNAL).await;

    assert_eq!(
        doc["jwks_uri"],
        serde_json::json!(format!("{EXTERNAL}/.well-known/jwks.json")),
        "an external client must be sent to an address it can resolve"
    );
    assert_eq!(
        identity.engine_jwks_uri(),
        format!("{IN_CLUSTER}/.well-known/jwks.json"),
        "an engine must keep fetching keys in-cluster"
    );
    assert_ne!(
        doc["jwks_uri"].as_str().unwrap(),
        identity.engine_jwks_uri(),
        "these are two different audiences; a fixture that made them equal \
         would hide either one moving"
    );
}

/// With the key absent every value is byte-identical to what it was before the
/// key existed. This is a widening, not a change: an existing deployment's
/// tokens, its discovery document, and every StarRocks `required_issuer`
/// already provisioned against the old `iss` must all keep working untouched.
#[tokio::test]
async fn without_the_key_every_value_is_what_it_was_before() {
    let identity = EgretIdentity::from_config(&config(None));

    assert_eq!(identity.issuer(), IN_CLUSTER);
    assert_eq!(
        identity.engine_jwks_uri(),
        format!("{IN_CLUSTER}/.well-known/jwks.json")
    );

    let (_, doc) = fetch_discovery(idp_state(&identity), IN_CLUSTER).await;
    assert_eq!(doc["issuer"], serde_json::json!(IN_CLUSTER));
    assert_eq!(
        doc["jwks_uri"],
        serde_json::json!(format!("{IN_CLUSTER}/.well-known/jwks.json"))
    );
    assert_eq!(
        doc["token_endpoint"],
        serde_json::json!(format!("{IN_CLUSTER}/oauth/token"))
    );
    assert_eq!(minted_iss(idp_state(&identity)).await, IN_CLUSTER);
}

/// A trailing slash never reaches a consumer. StarRocks byte-compares
/// `required_issuer` against the `iss`, so `…me/` in the chart and `…me` in the
/// token would break StarRocks JWT auth while Trino — which never reads the
/// issuer — kept working: a partial failure that looks like anything but a
/// config typo. Trimmed at the source, no call site can forget.
#[tokio::test]
async fn a_trailing_slash_on_the_configured_issuer_never_reaches_a_token() {
    let identity = EgretIdentity::from_config(&config(Some("http://egret.localtest.me/")));

    assert_eq!(identity.issuer(), EXTERNAL);
    assert_eq!(minted_iss(idp_state(&identity)).await, EXTERNAL);
}
