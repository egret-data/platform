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
    /// `[governance.opa] enabled` is false.
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
                "OPA is not enabled on this installation ([governance.opa] enabled)".to_string()
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
