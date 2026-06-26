//! `POST /v1/aws/s3/sign` — S3 SigV4 remote signing for engine data I/O.

use crate::api::resolve::{resolve_catalog, AuthnPrincipal};
use crate::routes::CatalogState;
use crate::CatalogError;
use axum::extract::{Path, State};
use axum::{Extension, Json};
use egret_iceberg_ext::sign::{S3SignRequest as ExtReq, S3SignResponse as ExtResp};

pub async fn sign(
    State(s): State<CatalogState>,
    Extension(principal): Extension<AuthnPrincipal>,
    Path(prefix): Path<String>,
    Json(req): Json<ExtReq>,
) -> Result<Json<ExtResp>, CatalogError> {
    // Resolve + ownership-check the `{prefix}` BEFORE signing, so an unauthorized
    // token can't mint S3 SigV4 credentials against a foreign workspace
    // (403) or an unknown prefix (404). The actual signing is catalog-agnostic
    // and uses the shared `storage_cfg` once the ownership check passes.
    resolve_catalog(&s, &principal, &prefix).await?;
    let storage_req = egret_storage::signer::S3SignRequest {
        region: req.region,
        uri: req.uri,
        method: req.method,
        headers: req.headers,
        body: req.body,
    };
    let r = egret_storage::signer::sign_s3_request(&s.storage_cfg, &storage_req).await?;
    Ok(Json(ExtResp {
        uri: r.uri,
        headers: r.headers,
    }))
}
