//! `POST /v1/aws/s3/sign` — S3 SigV4 remote signing for engine data I/O.

use crate::routes::CatalogState;
use crate::CatalogError;
use axum::extract::State;
use axum::Json;
use egret_iceberg_ext::sign::{S3SignRequest as ExtReq, S3SignResponse as ExtResp};

pub async fn sign(
    State(s): State<CatalogState>,
    Json(req): Json<ExtReq>,
) -> Result<Json<ExtResp>, CatalogError> {
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
