use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Custom S3 endpoint (MinIO). Empty/None means real AWS.
    pub endpoint: Option<String>,
    pub region: String,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    /// Required for MinIO.
    pub path_style: bool,
}
