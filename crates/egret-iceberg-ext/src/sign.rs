use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Serde helper for `http::Method` (http 1.x does not have `http-serde` 1 support).
mod http_method_serde {
    use serde::{Deserialize, Deserializer, Serializer};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct S3SignRequest {
    pub region: String,
    pub uri: url::Url,
    #[serde(with = "http_method_serde")]
    pub method: http::Method,
    pub headers: HashMap<String, Vec<String>>,
    pub body: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct S3SignResponse {
    pub uri: url::Url,
    pub headers: HashMap<String, Vec<String>>,
}
