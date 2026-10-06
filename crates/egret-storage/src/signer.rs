//! S3 remote signing: take an unsigned request shape from the client, return SigV4 headers.
use crate::{StorageConfig, StorageError};
use aws_sigv4::{
    http_request::{
        sign as aws_sign, PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest,
        SigningSettings,
    },
    sign::v4,
};
use std::collections::HashMap;
use std::time::SystemTime;

/// Headers the client manages itself — never include in the signature.
const UNSIGNED_HEADERS: &[&str] = &[
    "authorization",
    "range",
    "x-amz-date",
    "amz-sdk-invocation-id",
    "amz-sdk-retry",
];

/// Serde helper for `http::Method` (http 1.x does not have `http-serde` 1 support).
mod http_method_serde {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<http::Method, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        http::Method::from_bytes(s.as_bytes()).map_err(serde::de::Error::custom)
    }
}

#[derive(serde::Deserialize)]
pub struct S3SignRequest {
    pub region: String,
    pub uri: url::Url,
    #[serde(deserialize_with = "http_method_serde::deserialize")]
    pub method: http::Method,
    pub headers: HashMap<String, Vec<String>>,
    pub body: Option<String>,
}

#[derive(serde::Serialize)]
pub struct S3SignResponse {
    pub uri: url::Url,
    pub headers: HashMap<String, Vec<String>>,
}

/// Sign `req` with the catalog's S3 credentials; return the URI + headers to send to S3.
pub async fn sign_s3_request(
    cfg: &StorageConfig,
    req: &S3SignRequest,
) -> Result<S3SignResponse, StorageError> {
    // Guard: only sign requests aimed at the configured storage endpoint. Without this the
    // signer is an SSRF oracle — it would sign requests to any host using our S3 credentials.
    if let Some(endpoint) = &cfg.endpoint {
        let want = url::Url::parse(endpoint)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string));
        let got = req.uri.host_str().map(str::to_string);
        if let (Some(want), Some(got)) = (&want, &got) {
            if want != got {
                return Err(StorageError::Signing(format!(
                    "refusing to sign request for unexpected host: {got} (expected {want})"
                )));
            }
        }
    }

    let body_bytes = req.body.clone().map(String::into_bytes);
    let signable_body = match &body_bytes {
        Some(b) => SignableBody::Bytes(b),
        None => SignableBody::UnsignedPayload,
    };

    let mut settings = SigningSettings::default();
    // `Single` percent-encoding is correct for S3-compatible stores, which is the
    // Phase 2a target. Real AWS S3 requires double-encoding for some keys; revisit when AWS is supported.
    settings.percent_encoding_mode = PercentEncodingMode::Single;
    settings.payload_checksum_kind = PayloadChecksumKind::XAmzSha256;

    let creds = aws_credential_types::Credentials::new(
        cfg.access_key.clone(),
        cfg.secret_key.clone(),
        None,
        None,
        "egret-signer",
    );
    let identity = creds.into();

    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(&req.region)
        .name("s3")
        .time(SystemTime::now())
        .settings(settings)
        .build()
        .map_err(|e| StorageError::Signing(e.to_string()))?
        .into();

    let mut header_pairs: Vec<(String, String)> = Vec::new();
    for (k, values) in &req.headers {
        if UNSIGNED_HEADERS.iter().any(|u| u.eq_ignore_ascii_case(k)) {
            continue;
        }
        for v in values {
            header_pairs.push((k.clone(), v.clone()));
        }
    }

    let signable = SignableRequest::new(
        req.method.as_str(),
        req.uri.to_string(),
        header_pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        signable_body,
    )
    .map_err(|e| StorageError::Signing(e.to_string()))?;

    let (instructions, _sig) = aws_sign(signable, &params)
        .map_err(|e| StorageError::Signing(e.to_string()))?
        .into_parts();

    let mut out_uri = req.uri.clone();
    for (k, v) in instructions.params() {
        out_uri.query_pairs_mut().append_pair(k, v);
    }

    let mut out_headers = req.headers.clone();
    for (k, v) in instructions.headers() {
        out_headers.insert(k.to_string(), vec![v.to_string()]);
    }

    Ok(S3SignResponse {
        uri: out_uri,
        headers: out_headers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> StorageConfig {
        StorageConfig {
            endpoint: Some("http://localhost:9000".into()),
            region: "us-east-1".into(),
            bucket: "warehouse".into(),
            access_key: "AKIDEXAMPLE".into(),
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".into(),
            path_style: true,
        }
    }

    #[tokio::test]
    async fn signs_put_with_authorization_header() {
        let req = S3SignRequest {
            region: "us-east-1".into(),
            uri: url::Url::parse("http://localhost:9000/warehouse/ns/t/data/f.parquet").unwrap(),
            method: http::Method::PUT,
            headers: HashMap::from([(
                "x-amz-content-sha256".to_string(),
                vec!["UNSIGNED-PAYLOAD".to_string()],
            )]),
            body: None,
        };
        let resp = sign_s3_request(&cfg(), &req).await.unwrap();
        let auth = resp
            .headers
            .get("authorization")
            .or_else(|| resp.headers.get("Authorization"));
        assert!(
            auth.map(|v| v[0].starts_with("AWS4-HMAC-SHA256"))
                .unwrap_or(false),
            "expected AWS4 auth header, got {:?}",
            resp.headers
        );
    }

    #[tokio::test]
    async fn refuses_foreign_host() {
        let req = S3SignRequest {
            region: "us-east-1".into(),
            uri: url::Url::parse("http://169.254.169.254/latest/meta-data/").unwrap(),
            method: http::Method::GET,
            headers: std::collections::HashMap::new(),
            body: None,
        };
        // cfg().endpoint is http://localhost:9000 — foreign host must be refused.
        assert!(sign_s3_request(&cfg(), &req).await.is_err());
    }

    #[tokio::test]
    async fn client_authorization_header_is_not_signed_over() {
        // A client-supplied Authorization header must not corrupt signing; result still has a valid AWS4 auth header.
        let req = S3SignRequest {
            region: "us-east-1".into(),
            uri: url::Url::parse("http://localhost:9000/warehouse/ns/t/data/f.parquet").unwrap(),
            method: http::Method::PUT,
            headers: std::collections::HashMap::from([(
                "authorization".to_string(),
                vec!["Bearer stale".to_string()],
            )]),
            body: None,
        };
        let resp = sign_s3_request(&cfg(), &req).await.unwrap();
        let auth = resp
            .headers
            .get("authorization")
            .or_else(|| resp.headers.get("Authorization"))
            .unwrap();
        assert!(
            auth.iter().any(|v| v.starts_with("AWS4-HMAC-SHA256")),
            "expected AWS4 auth, got {auth:?}"
        );
    }
}
