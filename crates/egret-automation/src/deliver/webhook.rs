//! Webhook delivery: `POST` a JSON body, optionally signed.

use std::time::Duration;

use hmac::{Hmac, Mac};
use sha2::Sha256;

use egret_egress::{EgressError, EgressOptions, RequestSpec};
use reqwest::header::HeaderValue;

use super::destination::{Resolver, SsrfPolicy};

/// Header carrying the HMAC, so a receiver can tell a real delivery from
/// anything else that can reach its endpoint.
pub const SIGNATURE_HEADER: &str = "X-Egret-Signature";

/// A delivery attempt is capped hard: a webhook receiver that never answers
/// must not hold a scheduler slot open.
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    // The shared guard's texts carry no feature name; "webhook " restores
    // the texts this error had before the guard moved to egret-egress.
    #[error("webhook {0}")]
    Destination(#[from] super::destination::DestinationError),
    #[error("webhook request failed: {0}")]
    Transport(String),
    #[error("webhook returned {0}")]
    Status(u16),
}

/// The body a receiver gets. Field names are part of the contract — a receiver
/// parses these — so they are spelled out rather than derived from an internal
/// type that might be refactored.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebhookPayload<'a> {
    pub schedule: &'a str,
    pub schedule_id: &'a str,
    #[serde(rename = "type")]
    pub ty: &'a str,
    pub state: &'a str,
    pub value: Option<f64>,
    pub triggered_at: chrono::DateTime<chrono::Utc>,
    pub rows: serde_json::Value,
}

/// Hex-encoded HMAC-SHA256 of `body` under `secret`.
///
/// Computed over the exact bytes sent, not over a re-serialization: a receiver
/// verifying a different rendering of the same JSON would see a mismatch on
/// every delivery.
pub fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts a key of any length");
    mac.update(body);
    hex::encode(mac.finalize().into_bytes())
}

/// Send `payload` to `url`, after vetting the destination.
///
/// Sent through [`egret_egress::send`], which vets the destination, PINS every
/// vetted address onto the client (letting reqwest do its own lookup would
/// reopen the gap between the check and the connection, which is where DNS
/// rebinding lives) and never uses a proxy (through an environment
/// `HTTP_PROXY`, DNS happens at the proxy and pinning means nothing).
///
/// Redirects are **not followed**: a redirect is a second destination, and a
/// receiver answering 3xx gets [`WebhookError::Status`], as any non-2xx does.
pub async fn deliver(
    url: &str,
    secret: Option<&str>,
    payload: &WebhookPayload<'_>,
    policy: &SsrfPolicy,
    resolver: &dyn Resolver,
) -> Result<(), WebhookError> {
    let body = serde_json::to_vec(payload).map_err(|e| WebhookError::Transport(e.to_string()))?;

    let mut spec = RequestSpec::new(reqwest::Method::POST, url);
    spec.headers.insert(
        reqwest::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Some(secret) = secret {
        let signature = HeaderValue::from_str(&sign(secret, &body))
            .expect("a hex signature is a valid header value");
        spec.headers.insert(SIGNATURE_HEADER, signature);
    }
    spec.body = Some(body.into());

    let options = EgressOptions {
        connect_timeout: TIMEOUT,
        timeout: Some(TIMEOUT),
        read_timeout: None,
        max_redirects: 0,
        follow_redirects: false,
    };
    let res = egret_egress::send(policy, resolver, &options, spec)
        .await
        .map_err(|e| match e {
            EgressError::Destination { source, .. } => WebhookError::Destination(source),
            EgressError::Transport(t) => WebhookError::Transport(t),
            other => WebhookError::Transport(other.to_string()),
        })?;
    let status = res.status();
    if !status.is_success() {
        return Err(WebhookError::Status(status.as_u16()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_signature_is_stable_and_key_dependent() {
        // A receiver cannot verify a signature that moves, and a signature that
        // ignores the key authenticates nothing.
        let body = br#"{"state":"success"}"#;
        let a = sign("s3cret", body);
        assert_eq!(a, sign("s3cret", body), "same key + body must agree");
        assert_ne!(a, sign("other", body), "a different key must differ");
        assert_ne!(
            a,
            sign("s3cret", br#"{"state":"error"}"#),
            "a different body must differ"
        );
        assert_eq!(a.len(), 64, "hex-encoded SHA-256");
    }

    #[test]
    fn the_signature_matches_an_independently_computed_hmac() {
        // Pinned against a value computed from the primitive rather than from
        // this function, so a change of algorithm cannot go unnoticed.
        let mut mac = Hmac::<Sha256>::new_from_slice(b"key").unwrap();
        mac.update(b"payload");
        assert_eq!(
            sign("key", b"payload"),
            hex::encode(mac.finalize().into_bytes())
        );
    }

    #[test]
    fn the_payload_serializes_with_the_documented_field_names() {
        // These names are the receiver's contract.
        let p = WebhookPayload {
            schedule: "orders dropped",
            schedule_id: "abc",
            ty: "alert",
            state: "success",
            value: Some(12.0),
            triggered_at: chrono::DateTime::parse_from_rfc3339("2026-08-11T09:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            rows: serde_json::json!([{"n": 12}]),
        };
        let v: serde_json::Value =
            serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        assert_eq!(v["schedule"], "orders dropped");
        assert_eq!(v["type"], "alert", "serialized as `type`, not `ty`");
        assert_eq!(v["state"], "success");
        assert_eq!(v["value"], 12.0);
        assert_eq!(v["rows"][0]["n"], 12);
        assert!(v["triggered_at"].is_string());
    }

    #[test]
    fn destination_errors_read_as_they_did_before_the_guard_moved() {
        // Delivery errors are stored and shown; moving the guard to
        // egret-egress must not change a word of them.
        use super::super::destination::DestinationError as D;
        let cases = [
            (D::BadUrl, "webhook url is not a valid url"),
            (D::BadScheme, "webhook url must be http or https"),
            (D::NoHost, "webhook url has no host"),
            (D::Unresolvable, "webhook host does not resolve"),
            (
                D::Blocked("169.254.169.254".parse().unwrap(), "link-local"),
                "webhook host resolves to a blocked address (169.254.169.254): link-local",
            ),
        ];
        for (err, text) in cases {
            assert_eq!(WebhookError::Destination(err).to_string(), text);
        }
    }

    /// A blocked destination must fail BEFORE any request is attempted.
    #[tokio::test]
    async fn delivery_to_a_blocked_address_never_leaves_the_process() {
        struct Metadata;
        #[async_trait::async_trait]
        impl Resolver for Metadata {
            async fn resolve(
                &self,
                _h: &str,
                port: u16,
            ) -> Result<Vec<std::net::SocketAddr>, std::io::Error> {
                Ok(vec![std::net::SocketAddr::new(
                    "169.254.169.254".parse().unwrap(),
                    port,
                )])
            }
        }
        let p = WebhookPayload {
            schedule: "x",
            schedule_id: "1",
            ty: "alert",
            state: "success",
            value: None,
            triggered_at: chrono::Utc::now(),
            rows: serde_json::json!([]),
        };
        let err = deliver(
            "https://looks-fine.example/hook",
            None,
            &p,
            &SsrfPolicy::default(),
            &Metadata,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, WebhookError::Destination(_)),
            "must fail on the destination check, not on transport: {err:?}"
        );
    }
}
