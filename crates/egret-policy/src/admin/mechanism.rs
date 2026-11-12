//! Governance SP2b Task 11: the conditions under which a profile may switch
//! from Ranger to OPA, each refused by name.
//!
//! The stored-state conditions are not spelled here. The `all - …` and
//! hand-written rows come from [`crate::opa_guard::stored_state_refusals_via`]
//! and the stored mask/row-filter pairs from
//! [`ProfileRecompiler::stored_overlaps_in_tx`] (in production
//! `egret_perm_compile::opa::stored_overlaps_of_via`): the same functions the
//! OPA snapshot refresher re-checks on every build. One definition, two
//! callers, so a profile that passes the switch is never NotBuilt for a stored
//! row the switch accepted, nor the reverse.
//!
//! Switching back to Ranger has no condition, and the gate does not apply to it.

use super::{EngineBinding, PolicyHttpError, ProfileRecompiler};
use crate::opa_guard::StoredStateRefusal;
use crate::ServiceRow;
use egret_core::http::ErrorBody;

/// How a profile's engines are asked (`policy.authz_services.mechanism`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum MechanismName {
    Ranger,
    Opa,
}

/// One reason a profile may not switch to OPA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchRefusal {
    /// `[governance.opa] enabled` is false, or `ranger.enabled` is off (OPA does not run without it).
    Disabled,
    /// OPA exists for Trino only (migration 0213's CHECK).
    NotTrino { r#type: String },
    /// A manual profile is hand-written Ranger JSON, which OPA cannot evaluate.
    NotGenerated,
    /// A provisioned engine bound to the profile runs an image without Trino's
    /// OPA plugin (C6: provisioned engines only).
    EngineImage {
        engine: String,
        image: Option<String>,
    },
    /// A stored row the OPA evaluator would ignore (C8).
    Stored(StoredStateRefusal),
    /// Stored mask/row-filter pairs whose winner the artifact does not express.
    StoredOverlaps { pairs: Vec<String> },
}

/// The one image whose Trino ships the OPA plugin, as the refusal names it.
const SUPPORTED_OPA_IMAGE: &str = "trinodb/trino:483";

impl SwitchRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::Disabled => {
                "OPA is not running on this installation: it needs both [governance.opa] enabled \
                 and ranger.enabled"
                    .to_string()
            }
            Self::NotTrino { r#type } => {
                format!("only a Trino profile can use OPA; this profile is of type {type}")
            }
            Self::NotGenerated => "only a generated profile can use OPA: hand-written policies \
                                   are Ranger JSON that OPA cannot evaluate"
                .to_string(),
            Self::EngineImage { engine, image } => format!(
                "engine {engine} runs {}, which does not ship the Trino OPA plugin; \
                 supported: {SUPPORTED_OPA_IMAGE}",
                image.as_deref().unwrap_or("an unknown image")
            ),
            Self::Stored(r) => r.message(),
            Self::StoredOverlaps { pairs } => format!(
                "the profile serves {} mask/row-filter pairs whose winner the artifact does not \
                 express (see GET /perm/overlaps); resolve them first: {}",
                pairs.len(),
                pairs.join("; ")
            ),
        }
    }
}

/// What [`opa_switch_check`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchCheck {
    pub refusals: Vec<SwitchRefusal>,
    /// External engines bound to the profile: not refused, listed (C6); their operator installs the plugin.
    pub external_engines_unchanged: Vec<String>,
}

/// Every condition, in the enum's order; empty `refusals` means the switch to OPA may proceed.
///
/// `bindings` may list every engine: only those bound to `svc` are judged, in
/// the order given. Reads only; the caller decides what the transaction does.
pub async fn opa_switch_check(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    opa_enabled: bool,
    svc: &ServiceRow,
    bindings: &[EngineBinding],
    recompiler: &dyn ProfileRecompiler,
) -> Result<SwitchCheck, PolicyHttpError> {
    let mut refusals = Vec::new();
    if !opa_enabled {
        refusals.push(SwitchRefusal::Disabled);
    }
    if svc.r#type != crate::TRINO_DEF_NAME {
        refusals.push(SwitchRefusal::NotTrino {
            r#type: svc.r#type.clone(),
        });
    }
    if svc.origin != "generated" {
        refusals.push(SwitchRefusal::NotGenerated);
    }
    let mut external_engines_unchanged = Vec::new();
    for b in bindings
        .iter()
        .filter(|b| b.authz_service_id == Some(svc.id))
    {
        if b.origin == "external" {
            external_engines_unchanged.push(b.name.clone());
            continue;
        }
        let ships = b
            .image
            .as_deref()
            .is_some_and(|image| egret_perm::ships_opa_plugin(&b.r#type, image));
        if !ships {
            refusals.push(SwitchRefusal::EngineImage {
                engine: b.name.clone(),
                image: b.image.clone(),
            });
        }
    }
    refusals.extend(
        crate::opa_guard::stored_state_refusals_via(tx, svc.id)
            .await?
            .into_iter()
            .map(SwitchRefusal::Stored),
    );
    let pairs = recompiler.stored_overlaps_in_tx(tx, svc.id).await?;
    if !pairs.is_empty() {
        refusals.push(SwitchRefusal::StoredOverlaps { pairs });
    }
    Ok(SwitchCheck {
        refusals,
        external_engines_unchanged,
    })
}

// ── Task 12: the switch itself, `PATCH /policies/services/{service}` ──

pub use egret_policy_api::ReconfigureReport;

/// Re-applies the engines egret provisioned for a profile, after a switch
/// commits — the seam `egret-engine-mgmt` fills (this crate cannot depend on
/// it).
#[async_trait::async_trait]
pub trait ProfileEngineReconfigurer: Send + Sync {
    /// Re-apply every egret-provisioned engine bound to `service_id`, each with the profile's
    /// current mechanism. Never stops at the first failure: every engine is tried.
    async fn reconfigure_bound_engines(&self, service_id: i64) -> ReconfigureReport;
}

/// The reconfigurer until the engine seam provides one (governance SP2b
/// Task 15): reconfigures nothing, says so in the log, and reports an empty
/// result, so the switch answers 200 with no engine reconfigured.
pub struct NoReconfigure;

#[async_trait::async_trait]
impl ProfileEngineReconfigurer for NoReconfigure {
    async fn reconfigure_bound_engines(&self, service_id: i64) -> ReconfigureReport {
        tracing::warn!(
            service_id,
            "a profile's mechanism was set, but no engine reconfigurer is wired: its bound \
             engines keep the configuration they run"
        );
        ReconfigureReport::default()
    }
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct SetMechanismBody {
    pub mechanism: MechanismName,
}

/// The profile's mechanism after the switch, and what happened to its engines.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct ProfileMechanism {
    pub name: String,
    pub mechanism: MechanismName,
    /// The bound egret-provisioned engines re-applied with the new mechanism.
    pub engines_reconfigured: Vec<String>,
    /// The bound external engines: egret does not configure them; their
    /// operator installs the new plugin files.
    pub external_engines_unchanged: Vec<String>,
    /// What the operator of each external engine must do now, for the
    /// direction of this switch. Absent when no external engine is bound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_engines_warning: Option<String>,
    /// Set when this is a Trino profile with no tag service: classification
    /// rules and clearances do not apply to its engines, on either mechanism.
    /// Absent otherwise (a StarRocks profile, or one with a tag service).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_tag_service_warning: Option<String>,
}

/// What a Trino profile with no tag service is told (`no_tag_service_warning`
/// on the switch answer and on explain's profile).
pub const NO_TAG_SERVICE_WARNING: &str = "This enforcement point has no tag service, so classification \
    rules and clearances do not apply to its engines. Bind a tag service in the profile's \
    settings.";

/// [`NO_TAG_SERVICE_WARNING`] when `svc` is a Trino profile whose
/// `config.tagService` resolves to no profile (the same answer
/// [`crate::usable_tag_services_via`] gives the plugin download), else `None`.
/// Reads in the caller's transaction.
pub async fn no_tag_service_warning_via(
    conn: &mut sqlx::PgConnection,
    svc: &ServiceRow,
) -> Result<Option<String>, crate::PolicyStoreError> {
    if svc.r#type != crate::TRINO_DEF_NAME {
        return Ok(None);
    }
    let bound = crate::usable_tag_services_via(conn, &[svc.id]).await?;
    Ok(bound.is_empty().then(|| NO_TAG_SERVICE_WARNING.to_string()))
}

/// What an external engine's operator must do after the profile moved to
/// `mechanism`, or `None` when `engines` is empty (SP2b Task 20F, T15 M7).
/// egret does not configure an external engine, so after Ranger→OPA it keeps
/// enforcing the Ranger artifact (still compiled). After OPA→Ranger, one whose
/// operator installed the OPA plugin asks an OPA surface that no longer
/// serves its profile, so it fails every query until its Ranger plugin is
/// back; one still on the Ranger plugin is unaffected.
pub fn external_engines_warning(mechanism: MechanismName, engines: &[String]) -> Option<String> {
    if engines.is_empty() {
        return None;
    }
    let names = engines.join(", ");
    Some(match mechanism {
        MechanismName::Opa => format!(
            "external engines not reconfigured: {names}. They keep enforcing the Ranger \
             policies until their operator installs the OPA plugin files from \
             GET /api/control-plane/engines/{{id}}/opa-plugin-config (Trino 483 or later \
             with plugin/opa)."
        ),
        MechanismName::Ranger => format!(
            "external engines not reconfigured: {names}. Any of them running the OPA \
             plugin fails every query until its operator re-installs the Ranger plugin \
             files from GET /api/control-plane/engines/{{id}}/ranger-plugin-config; one \
             still running the Ranger plugin is unaffected."
        ),
    })
}

impl MechanismName {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ranger => "ranger",
            Self::Opa => "opa",
        }
    }
}

/// PATCH /policies/services/{service} — switch a profile between Ranger and
/// OPA (`policy.authz_services.mechanism`).
///
/// Under the row locks of the profile **and** its tag service, taken in the
/// order a recompile takes them, the switch to OPA re-checks every condition
/// (the profile is a generated Trino profile, OPA is enabled, every bound
/// egret-provisioned engine runs an image with the OPA plugin, no `all - …`
/// defaults, no hand-written policies, no stored mask/row-filter pairs) in
/// the same transaction that writes, so an operator insert into the tag
/// service cannot slip between the check and the write. Switching to Ranger
/// has no condition and skips the check (the gate included). A change writes
/// `mechanism`, recompiles the profile's Ranger artifact (which every profile
/// keeps, whatever its mechanism) and records `profile:mechanism`. It bumps
/// `policy_version` at least once, plus once more if the Ranger recompile
/// writes. The same mechanism again writes nothing.
///
/// After the commit the bound engines are always reconfigured, also when
/// nothing was written: re-sending the PATCH is how a failed reconfigure is
/// retried.
#[utoipa::path(
    patch,
    path = "/api/control-plane/policies/services/{service}",
    tag = "policy-admin",
    params(("service" = String, Path, description = "Enforcement point (Ranger service) name")),
    request_body = SetMechanismBody,
    responses(
        (status = 200, description = "The coordinator of every bound egret-provisioned engine restarts to load the new access control; queries running on it fail. External engines are listed in `external_engines_unchanged`, and `external_engines_warning` says what their operator must do: after a switch to OPA they keep enforcing the Ranger policies until the OPA plugin files are installed (Trino 483 or later with `plugin/opa`); after a switch to Ranger, those running the OPA plugin fail every query until the Ranger plugin files are re-installed. `no_tag_service_warning` is present when the enforcement point is a Trino enforcement point with no tag service, so classification rules and clearances do not apply to its engines.", body = ProfileMechanism),
        (status = 401, description = "Not authenticated", body = ErrorBody),
        (status = 403, description = "Missing policy:update permission", body = ErrorBody),
        (status = 404, description = "No such enforcement point", body = ErrorBody),
        (status = 409, description = "The switch to OPA is refused: `{error, code: \"mechanism_refused\", refusals}`, `refusals` naming every condition that holds (`error` is the first). Nothing is written", body = Object),
        (status = 500, description = "Internal error", body = ErrorBody),
        (status = 502, description = "The switch is committed, but some bound engines were not reconfigured: `{error, engines_reconfigured, engines_failed: [{engine, error}], external_engines_unchanged, external_engines_warning?, no_tag_service_warning?}`. Send the same PATCH again to retry", body = Object),
    ),
)]
pub async fn set_mechanism(
    axum::extract::State(st): axum::extract::State<super::PolicyAdminState>,
    egret_auth::principal::Auth(p): egret_auth::principal::Auth,
    axum::extract::Path(service): axum::extract::Path<String>,
    axum::Json(b): axum::Json<SetMechanismBody>,
) -> Result<axum::response::Response, PolicyHttpError> {
    use axum::response::IntoResponse;
    use serde_json::json;

    egret_core::authz::require(&*st.pdp, &p, "policy:update").await?;
    let svc = st.store.get_service(&service).await?;

    let mut tx = st.store.begin().await?;
    super::lock_unit_rows(&mut tx, svc.id).await?;
    let svc = crate::get_service_by_id_via(&mut tx, svc.id).await?;
    // The bound engines and their images, read once the profile's rows are
    // locked, so the image check judges the bindings current at the switch.
    // A bind or re-image that lands after the commit is refused at apply time
    // by the engine seam's image guard for an OPA profile (governance SP2b
    // Task 15, I6), not here.
    let bindings: Vec<EngineBinding> = st
        .engines
        .list_engine_bindings()
        .await?
        .into_iter()
        .filter(|e| e.authz_service_id == Some(svc.id))
        .collect();
    let external_engines_unchanged = match b.mechanism {
        MechanismName::Opa => {
            let check =
                opa_switch_check(&mut tx, st.opa_enabled, &svc, &bindings, &*st.recompiler).await?;
            if !check.refusals.is_empty() {
                let refusals: Vec<String> =
                    check.refusals.iter().map(SwitchRefusal::message).collect();
                return Err(PolicyHttpError::ConflictBody {
                    body: json!({
                        "error": refusals[0],
                        "code": "mechanism_refused",
                        "refusals": refusals,
                    }),
                });
            }
            check.external_engines_unchanged
        }
        // Switching back is always allowed: no condition, no gate.
        MechanismName::Ranger => bindings
            .iter()
            .filter(|e| e.origin == "external")
            .map(|e| e.name.clone())
            .collect(),
    };
    let after = b.mechanism.as_str();
    if svc.mechanism != after {
        sqlx::query(
            "UPDATE policy.authz_services \
                SET mechanism = $2, policy_version = policy_version + 1, updated_at = now() \
              WHERE id = $1",
        )
        .bind(svc.id)
        .bind(after)
        .execute(&mut *tx)
        .await
        .map_err(crate::PolicyStoreError::from)?;
        // S8: every profile keeps its Ranger artifact; a switch never skips it.
        st.recompiler.recompile_in_tx(&mut tx, &[svc.id]).await?;
        egret_audit::governance::record_change(
            &mut tx,
            egret_audit::governance::Change {
                action: egret_audit::governance::action::PROFILE_MECHANISM,
                resource_type: egret_audit::governance::resource::POLICY_PROFILE,
                resource_id: svc.id.to_string(),
                summary: json!({ "service": svc.name, "before": svc.mechanism, "after": after }),
                principal: &p,
            },
        )
        .await
        .map_err(crate::PolicyStoreError::from)?;
    }
    let no_tag_service_warning = no_tag_service_warning_via(&mut tx, &svc).await?;
    tx.commit().await.map_err(crate::PolicyStoreError::from)?;

    // Always, also when nothing was written: the retry path of a failed
    // reconfigure (plan-review ruling a).
    let report = st.reconfigurer.reconfigure_bound_engines(svc.id).await;
    let warning = external_engines_warning(b.mechanism, &external_engines_unchanged);
    if !report.failed.is_empty() {
        let names: Vec<&str> = report.failed.iter().map(|(n, _)| n.as_str()).collect();
        let failed: Vec<serde_json::Value> = report
            .failed
            .iter()
            .map(|(engine, error)| json!({ "engine": engine, "error": error }))
            .collect();
        let mut body = json!({
            "error": format!(
                "the profile is now on {after}; engines not reconfigured: {}. \
                 Send the same PATCH again to retry.",
                names.join(", ")
            ),
            "engines_reconfigured": report.applied,
            "engines_failed": failed,
            "external_engines_unchanged": external_engines_unchanged,
        });
        if let Some(warning) = warning {
            body["external_engines_warning"] = json!(warning);
        }
        if let Some(warning) = no_tag_service_warning {
            body["no_tag_service_warning"] = json!(warning);
        }
        return Ok((axum::http::StatusCode::BAD_GATEWAY, axum::Json(body)).into_response());
    }
    Ok(axum::Json(ProfileMechanism {
        name: svc.name,
        mechanism: b.mechanism,
        engines_reconfigured: report.applied,
        external_engines_unchanged,
        external_engines_warning: warning,
        no_tag_service_warning,
    })
    .into_response())
}
