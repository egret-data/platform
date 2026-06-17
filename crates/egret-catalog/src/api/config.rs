//! `GET /v1/config` — catalog configuration advertised to engines/pyiceberg.

use crate::routes::CatalogState;
use axum::extract::State;
use axum::Json;
use egret_iceberg_ext::catalog::CatalogConfig;
use std::collections::HashMap;

pub async fn get_config(State(s): State<CatalogState>) -> Json<CatalogConfig> {
    let signer_uri = format!("{}/", s.base_url); // clients append the sign path
    let overrides = HashMap::from([
        ("warehouse".to_string(), s.warehouse.to_string()),
        ("s3.remote-signing-enabled".to_string(), "true".to_string()),
        ("s3.signer".to_string(), "S3V4RestSigner".to_string()),
        ("s3.signer.uri".to_string(), signer_uri.clone()),
        ("signer.uri".to_string(), signer_uri),
        ("s3.region".to_string(), s.storage_cfg.region.clone()),
        (
            "s3.path-style-access".to_string(),
            s.storage_cfg.path_style.to_string(),
        ),
    ]);
    Json(CatalogConfig {
        overrides,
        defaults: HashMap::new(),
        endpoints: vec![],
    })
}
