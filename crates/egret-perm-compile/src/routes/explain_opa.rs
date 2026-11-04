//! Governance SP2b Task 16: `explain` for a profile on OPA, answered by the
//! IR evaluator (`egret-perm-eval`) over the profile's snapshot — the same
//! evaluator, over the same snapshot, the OPA surface answers Trino from.
//!
//! **The snapshot** is the refresher's when its key is the profile's current
//! `(policy_version, tag_version)` as read in explain's own read-only
//! transaction, and otherwise one built in that transaction with
//! [`MissingUsername::Omit`], which never mints. The build runs under a
//! savepoint, so a failing read cannot poison the transaction the rest of the
//! request (other profiles, the not-enforced line) still reads from.
//!
//! **A profile whose snapshot cannot be built is refused, never a 500.** Every
//! access is `Denied` and its `ir_reason` names the refusal class (the
//! [`CompileError`] variant) and nothing else. The build error's own text can
//! name other users, rules and SQL, so it is logged and never answered — the
//! rule the OPA surface's 403 body follows.
//!
//! **An evaluator error is refused the same way, for that profile only**
//! (`TwoMasks` from `column_mask_choice`, or any error `row_filters` grows):
//! the OPA surface answers it 500, which fails the query, so every access is
//! `Denied`, `ir_reason` names the error class, and there is no `opa` block.

use std::sync::Arc;

use egret_perm::{Action, CompileError, EngineType, Scope};
use egret_perm_eval::trino_ops::DataPrivilege;
use egret_perm_eval::{render_mask, Decision, EvalError, ObjectPath, Snapshot};

use super::{ranger_engine, ExplainAccess, ExplainProfile, PermHttpError};
use crate::explain_wire::{
    ExplainOpa, WireDecision, WireDenyReason, WireEngineType, WireMechanism,
};
use crate::opa::build::MissingUsername;
use crate::opa::cache::{Lookup, SnapshotCache};

/// The column type explain renders a mask with: the engine substitutes the
/// column's own type, which explain does not know.
const EXPLAIN_MASK_COLUMN_TYPE: &str = "varchar";

/// The snapshot explain evaluates, or the `ir_reason` every access of the
/// profile is refused with.
pub(super) type ExplainSnapshot = Result<Arc<Snapshot>, String>;

/// The refresher's snapshot when it is at `key`, else one built now in the
/// caller's (read-only, `REPEATABLE READ`) transaction.
pub(super) async fn opa_snapshot_for_explain(
    conn: &mut sqlx::PgConnection,
    cache: Option<&SnapshotCache>,
    service_id: i64,
    key: (i64, i64),
) -> Result<ExplainSnapshot, PermHttpError> {
    if let Some(cache) = cache {
        if let Lookup::Ready(s) = cache.get(service_id) {
            if s.key() == key {
                return Ok(Ok(s));
            }
        }
    }
    let mut savepoint = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(PermHttpError::db)?;
    let built =
        crate::opa::build::build_snapshot_via(&mut savepoint, service_id, MissingUsername::Omit)
            .await;
    savepoint.rollback().await.map_err(PermHttpError::db)?;
    Ok(match built {
        Ok(Some(s)) => Ok(Arc::new(s)),
        // `build_snapshot_via` answers `None` only for a profile that is not
        // Trino or StarRocks, which `engine_type_for` already excluded.
        Ok(None) => Err(refusal("NotATrinoProfile")),
        Err(e) => {
            tracing::warn!(
                service_id,
                error = %e,
                "explain: the profile's OPA snapshot did not build; its accesses are refused"
            );
            Err(refusal(refusal_class(&e)))
        }
    })
}

fn refusal(class: &str) -> String {
    format!("no OPA snapshot: build failed ({class})")
}

/// The [`CompileError`] variant's name, and nothing it carries. No wildcard
/// arm: a new variant must be named here before it can be answered. A
/// `Snapshot::build` refusal reaches here as `Db("OPA snapshot: …")`.
fn refusal_class(e: &CompileError) -> &'static str {
    match e {
        CompileError::Db(m) if m.starts_with("OPA snapshot:") => "SnapshotRefused",
        CompileError::Db(_) => "Db",
        CompileError::LockTimeout(_) => "LockTimeout",
        CompileError::UnsupportedAction { .. } => "UnsupportedAction",
        CompileError::ExpansionCeiling { .. } => "ExpansionCeiling",
        CompileError::ClearanceExpansionCeiling { .. } => "ClearanceExpansionCeiling",
        CompileError::ClearanceMaskUndecidable { .. } => "ClearanceMaskUndecidable",
        CompileError::ProfileExpansionCeiling { .. } => "ProfileExpansionCeiling",
        CompileError::AmbiguousMaskOrFilterArbitration { .. } => "AmbiguousMaskOrFilterArbitration",
        CompileError::UnusableUserRef { .. } => "UnusableUserRef",
        CompileError::ReservedEnginePrincipal { .. } => "ReservedEnginePrincipal",
        CompileError::UnexpandedPrincipalKind { .. } => "UnexpandedPrincipalKind",
        CompileError::InvalidCustomMask { .. } => "InvalidCustomMask",
        CompileError::UnsupportedMaskType { .. } => "UnsupportedMaskType",
        CompileError::MissingTagService { .. } => "MissingTagService",
        CompileError::MissingTagName { .. } => "MissingTagName",
        CompileError::ServiceDefUnreadable { .. } => "ServiceDefUnreadable",
        CompileError::EngineTypeMismatch { .. } => "EngineTypeMismatch",
        CompileError::UnfilterableRowFilterScope { .. } => "UnfilterableRowFilterScope",
        CompileError::EmptyScopeLevel { .. } => "EmptyScopeLevel",
        CompileError::NamedLevelUnderWildcard { .. } => "NamedLevelUnderWildcard",
        CompileError::UnexpressibleExemption { .. } => "UnexpressibleExemption",
        CompileError::HiddenColumnsUnenumerable { .. } => "HiddenColumnsUnenumerable",
        CompileError::InvalidEngineGrant { .. } => "InvalidEngineGrant",
        CompileError::ProfileSetChanged { .. } => "ProfileSetChanged",
    }
}

pub(super) struct OpaProfileInput<'a> {
    pub snapshot: &'a ExplainSnapshot,
    pub service_id: i64,
    pub service_name: &'a str,
    pub engine: EngineType,
    pub catalog: &'a str,
    pub user: &'a str,
    pub action: Action,
    pub target: &'a Scope,
}

/// The Trino access types an action is asked as, and the IR privilege each
/// one is decided by. A `write` is two verdicts, never collapsed.
fn accesses_for(action: Action) -> &'static [(&'static str, DataPrivilege)] {
    match action {
        Action::Read => &[("select", DataPrivilege::Read)],
        Action::Write => &[
            ("insert", DataPrivilege::Write),
            ("delete", DataPrivilege::Write),
        ],
    }
}

/// The IR verdict on the wire: an allow names its statement in
/// `policy_name`; a deny is `DenyPolicyItem` when a Deny or Withhold refused
/// it and `NoPolicyMatched` when nothing allowed it.
fn wire_decision(d: &Decision) -> (WireDecision, String) {
    match d {
        Decision::Allow { by } => (
            WireDecision::Allowed {
                policy_id: None,
                policy_name: by.clone(),
            },
            by.clone(),
        ),
        Decision::Deny { reason, explicit } => (
            WireDecision::Denied {
                policy_id: None,
                policy_name: String::new(),
                reason: if *explicit {
                    WireDenyReason::DenyPolicyItem
                } else {
                    WireDenyReason::NoPolicyMatched
                },
            },
            reason.clone(),
        ),
    }
}

/// The evaluator error's class, and nothing it carries (the column, the user,
/// the masks). No wildcard arm.
fn eval_error_class(e: &EvalError) -> &'static str {
    match e {
        EvalError::TwoMasks { .. } => "TwoMasks",
    }
}

/// The row filter and column mask for a `read` naming a table, from a Ready
/// snapshot; `Ok(None)` for every other request.
fn opa_values(
    snapshot: &Snapshot,
    i: &OpaProfileInput<'_>,
) -> Result<Option<ExplainOpa>, EvalError> {
    let target = i.target;
    let (Action::Read, Some(schema), Some(table)) = (i.action, &target.schema, &target.table)
    else {
        return Ok(None);
    };
    let filters = snapshot.row_filters(i.user, i.catalog, schema, table)?;
    let row_filter = (!filters.is_empty()).then(|| filters.join(" AND "));
    let (column_mask_type, column_mask) = match &target.column {
        Some(column) => {
            match snapshot.column_mask_choice(i.user, i.catalog, schema, table, column)? {
                Some((mask_type, custom)) => match render_mask(
                    &mask_type,
                    custom.as_deref(),
                    column,
                    EXPLAIN_MASK_COLUMN_TYPE,
                ) {
                    Some(expr) => (Some(mask_type), Some(expr)),
                    // `MASK_NONE`: no mask.
                    None => (None, None),
                },
                None => (None, None),
            }
        }
        None => (None, None),
    };
    Ok(Some(ExplainOpa {
        column_mask,
        column_mask_type,
        row_filter,
    }))
}

/// One OPA profile's answer for one user.
///
/// **An evaluator error is this profile's, never the request's.** The OPA
/// surface answers it 500 on a value route, which the Trino plugin turns into
/// a failed query, so explain answers every access of the profile `Denied`
/// with an `ir_reason` naming only the error class, and no `opa` block.
pub(super) fn opa_profile(i: &OpaProfileInput<'_>) -> ExplainProfile {
    let target = i.target;
    let object = ObjectPath {
        catalog: i.catalog.to_string(),
        schema: target.schema.clone(),
        table: target.table.clone(),
        column: target.column.clone(),
    };
    // The snapshot, or why nothing it answers can be shown.
    let answered: Result<(&Snapshot, Option<ExplainOpa>), String> = match i.snapshot {
        Ok(s) => match opa_values(s, i) {
            Ok(opa) => Ok((s.as_ref(), opa)),
            Err(e) => {
                tracing::warn!(
                    service_id = i.service_id,
                    error = %e,
                    "explain: the IR evaluator failed for this profile; its accesses are refused"
                );
                Err(format!(
                    "no OPA answer: evaluator error ({})",
                    eval_error_class(&e)
                ))
            }
        },
        Err(refused) => Err(refused.clone()),
    };
    let mut accesses = Vec::new();
    for (access_type, privilege) in accesses_for(i.action) {
        let (decision, ir_reason) = match &answered {
            Ok((s, _)) => wire_decision(&s.decide_privilege(i.user, *privilege, &object)),
            Err(refused) => (
                WireDecision::Denied {
                    policy_id: None,
                    policy_name: String::new(),
                    reason: WireDenyReason::NoPolicyMatched,
                },
                refused.clone(),
            ),
        };
        accesses.push(ExplainAccess {
            access_type: (*access_type).to_string(),
            decision,
            attribution: None,
            ir_reason: Some(ir_reason),
        });
    }
    let opa = answered.ok().and_then(|(_, opa)| opa);

    let mut profile = ExplainProfile {
        service_id: i.service_id,
        service_name: i.service_name.to_string(),
        engine: WireEngineType::from(ranger_engine(i.engine)),
        mechanism: WireMechanism::Opa,
        opa,
        accesses,
        tags: Vec::new(),
        tag_policies_in_download: 0,
        mask: None,
        row_filter: None,
        clearance_masks_missing: Vec::new(),
        masked_read_skips_access: false,
        other_columns_access: None,
        effect_lines: Vec::new(),
    };
    profile.effect_lines =
        crate::explain_effect::opa_effect_lines(i.user, i.action, target, &profile);
    profile
}
