//! S3 remote signing: take an unsigned request shape from the client, return SigV4 headers.
use crate::{StorageConfig, StorageError};
use aws_sigv4::{
    http_request::{
        PayloadChecksumKind, PercentEncodingMode, SignableBody, SignableRequest, SigningSettings,
        sign as aws_sign,
    },
    sign::v4,
};
use std::collections::HashMap;
use std::time::SystemTime;

/// Headers the client manages itself — never include in the signature.
const UNSIGNED_HEADERS: &[&str] = &[
    "range",
    "x-amz-date",
    "amz-sdk-invocation-id",
    "amz-sdk-retry",
];

/// Serde helper for `http::Method` (http 1.x does not have `http-serde` 1 support).
mod http_method_serde {
    use serde::{Deserialize, Deserializer, Serializer};

    #[allow(dead_code)]
    pub fn serialize<S>(method: &http::Method, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(method.as_str())
    }

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
    #[serde(with = "http_method_serde")]
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
    let body_bytes = req.body.clone().map(String::into_bytes);
    let signable_body = match &body_bytes {
        Some(b) => SignableBody::Bytes(b),
        None => SignableBody::UnsignedPayload,
    };

    let mut settings = SigningSettings::default();
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
        if UNSIGNED_HEADERS.contains(&k.to_ascii_lowercase().as_str()) {
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
            uri: url::Url::parse(
                "http://localhost:9000/warehouse/ns/t/data/f.parquet",
            )
            .unwrap(),
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
            auth.map(|v| v[0].starts_with("AWS4-HMAC-SHA256")).unwrap_or(false),
            "expected AWS4 auth header, got {:?}",
            resp.headers
        );
    }
}
