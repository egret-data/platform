//! `POST /api/control-plane/engines/{id}/security-upgrade`: bring a legacy
//! provisioned StarRocks engine up to egret's security baseline.
//!
//! What the engine lacks is decided by
//! [`crate::engines::security_upgrade_reasons`], the same rule the engine DTO
//! reports through `securityUpgradeReasons`:
//!
//! 1. no FE root password secret: one is minted
//!    ([`crate::engines::ensure_root_password_secret`], idempotent);
//! 2. an FE image without every egret security patch: FE and CN move to the
//!    default images, unless the engine already runs a newer release, which is
//!    never downgraded.
//!
//! Before either step writes anything the row is marked
//! `security_upgrade_pending`. The engine is then ALWAYS re-applied the way
//! `rotate_engine_secrets` re-applies one, and only an accepted apply clears
//! the mark (`engines::finish_apply`, like any other successful StarRocks
//! apply). So an apply that fails, or a request that dies between the writes
//! and the apply, leaves the engine reporting the `notApplied` reason, and a
//! repeat call, whose steps are both idempotent, finishes the run. The
//! provisioner renders the `{name}-initpwd` Job, which sets the root password
//! on the running FE, and the FE and CN pods restart on the new images.

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use egret_auth::principal::Auth;
use egret_core::http::ErrorBody;
use egret_perm::DEFAULT_ENGINE_IMAGES;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::engines::{
    apply_built, apply_conflict, build_provision_req_with, ensure_root_password_secret,
    security_upgrade_reasons, ApplyBuiltError, ApplyOp, BuildOpts, EngineDto,
    SecurityUpgradeReason, RANGER_CREDENTIAL_CHANGED,
};
use crate::EngineHttpError as CpError;
use crate::EngineMgmtState as CpState;

pub fn routes() -> Router<CpState> {
    Router::new().route("/engines/{id}/security-upgrade", post(security_upgrade))
}

/// What `POST /engines/{id}/security-upgrade` did.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecurityUpgradeDto {
    /// The engine as it now stands.
    engine: EngineDto,
    /// `true` when FE and CN were moved to the default patched images.
    image_changed: bool,
    /// `true` when a FE root password was minted for the engine.
    root_password_set: bool,
    /// Why the image was kept although it lacks the patches: the engine runs a
    /// newer release than the default FE, and egret does not downgrade. Absent
    /// otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    image_kept_because: Option<String>,
}

/// Bring a legacy StarRocks engine up to egret's security baseline
///
/// Mints a FE root password if the engine has none, moves FE and CN to the
/// default patched images unless that would be a downgrade, and re-applies
/// the engine: a Job sets the root password on the running FE and the FE and
/// CN pods restart. The engine is re-applied on every call, so a repeat
/// finishes a run whose apply failed.
#[utoipa::path(
    post,
    path = "/api/control-plane/engines/{id}/security-upgrade",
    tag = "control-plane-engines",
    params(("id" = Uuid, Path, description = "Engine id")),
    responses(
        (status = 200, description = "The engine is at the security baseline as far as egret can take it. The engine was re-applied (on every call): a Job sets the FE root password on the running FE, and the FE and CN pods restart (on the default patched images when `imageChanged`). `rootPasswordSet` says a root password was minted by this call. `imageKeptBecause` says why an unpatched image was kept (the engine runs a newer release than the default FE, which egret does not downgrade)", body = SecurityUpgradeDto),
        (status = 400, description = "Kubernetes not configured, or the engine's spec could not be assembled (e.g. no primary catalog with a warehouse)", body = ErrorBody),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing engine:update permission", body = ErrorBody),
        (status = 404, description = "No such engine", body = ErrorBody),
        (status = 409, description = "Not a StarRocks engine; or the engine changed concurrently (e.g. a delete in progress), retry; or the engine is external (`code` = `engine_is_external`): egret does not upgrade the cluster of it", body = ErrorBody),
        (status = 502, description = "Kubernetes rejected the apply. The stored spec and root password secret already carry the upgrade; the engine keeps the `notApplied` security upgrade reason (`securityUpgradeNeeded` stays true), and a repeat call finishes the run", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody),
    ),
)]
async fn security_upgrade(
    State(st): State<CpState>,
    Auth(p): Auth,
    Path(id): Path<Uuid>,
) -> Result<Json<SecurityUpgradeDto>, CpError> {
    egret_core::authz::require(&*st.pdp, &p, "engine:update").await?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    crate::external::refuse_if_external(&engine, "upgrade its security baseline")?;
    if engine.r#type != "starrocks" {
        return Err(CpError::Conflict(format!(
            "the security upgrade applies to StarRocks engines only, not {} engines",
            engine.r#type
        )));
    }
    if engine.status == crate::engine_status::DELETING {
        return Err(apply_conflict());
    }
    // Refuse before writing anything: the steps below are only meaningful if
    // the engine can then be re-applied.
    let Some(prov) = st.engine_provisioner.clone() else {
        return Err(CpError::BadRequest("kubernetes not configured".into()));
    };

    // Committed BEFORE either step writes: whatever happens to the apply
    // below, the engine keeps reporting that the upgrade is unfinished until
    // an apply is accepted.
    st.engine_mgmt
        .set_security_upgrade_pending(id, true)
        .await?;
    let reasons = security_upgrade_reasons(&engine);
    let mut root_password_set = false;
    let mut image_changed = false;
    let mut image_kept_because = None;

    if engine.root_secret_id.is_none() {
        ensure_root_password_secret(&st, &engine).await?;
        root_password_set = true;
    }

    if reasons.contains(&SecurityUpgradeReason::UnpatchedImage) {
        let current = egret_perm::stored_engine_image("starrocks", &engine.spec);
        match egret_perm::starrocks_fe_upgrade_target(current.as_deref()) {
            Ok(target) => {
                let mut spec = engine.spec.clone();
                spec["starrocksConfig"]["feImage"] = target.into();
                spec["starrocksConfig"]["cnImage"] =
                    DEFAULT_ENGINE_IMAGES.starrocks_cn_image.into();
                if !st
                    .engine_mgmt
                    .update_engine(id, &spec, &engine.status)
                    .await?
                {
                    return Err(apply_conflict());
                }
                image_changed = true;
            }
            Err(release) => {
                image_kept_because = Some(format!(
                    "No patched image for release {release} yet: the image was not changed."
                ));
            }
        }
    }

    // Always applied, not only when this call changed something: a previous
    // call may have stored its changes and then failed to apply them. The
    // apply, once accepted, clears `security_upgrade_pending`.
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;
    let built = build_provision_req_with(&st, &engine, BuildOpts::default()).await?;
    apply_built(&st, prov, &engine, &built, ApplyOp::Update)
        .await
        .map_err(|e| match e {
            ApplyBuiltError::Conflict => apply_conflict(),
            ApplyBuiltError::CredentialChanged => {
                CpError::Conflict(RANGER_CREDENTIAL_CHANGED.into())
            }
            ApplyBuiltError::Failed(msg) => CpError::ClusterApplyFailed(msg),
        })?;
    let engine = st
        .engine_mgmt
        .get_engine(id)
        .await?
        .ok_or(CpError::NotFound)?;

    Ok(Json(SecurityUpgradeDto {
        engine: EngineDto::from(engine).with_runtime(&st),
        image_changed,
        root_password_set,
        image_kept_because,
    }))
}

/// OpenAPI fragment for the security upgrade route, unioned into
/// `egret_engine_mgmt::openapi()`.
#[derive(utoipa::OpenApi)]
#[openapi(
    paths(security_upgrade),
    components(schemas(SecurityUpgradeDto, ErrorBody))
)]
pub struct SecurityUpgradeApi;
