//! `GET /v1/config` — catalog configuration advertised to engines/pyiceberg.
//!
//! Iceberg clients (Daft/Trino/pyiceberg) call `GET /v1/config?warehouse=<identifier>`,
//! read `overrides`/`defaults`, then target the catalog using the returned `prefix`.
//! When `warehouse` names a workspace slug we resolve its managed catalog and advertise
//! the `prefix`/`warehouse` overrides; without it we return signer-only discovery config.

use crate::api::resolve::resolve_catalog;
use crate::routes::CatalogState;
use crate::CatalogError;
use axum::extract::{Query, State};
use axum::Json;
use egret_iceberg_ext::catalog::CatalogConfig;
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct ConfigQuery {
    pub warehouse: Option<String>,
}

pub async fn get_config(
    State(s): State<CatalogState>,
    Query(q): Query<ConfigQuery>,
) -> Result<Json<CatalogConfig>, CatalogError> {
    let signer_uri = format!("{}/", s.base_url); // clients append the sign path
    let mut overrides = HashMap::from([
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
    if let Some(slug) = q.warehouse {
        let cat = resolve_catalog(&s, &slug).await?;
        overrides.insert("prefix".to_string(), slug);
        overrides.insert("warehouse".to_string(), cat.warehouse);
    }
    Ok(Json(CatalogConfig {
        overrides,
        defaults: HashMap::new(),
        endpoints: vec![],
    }))
}
