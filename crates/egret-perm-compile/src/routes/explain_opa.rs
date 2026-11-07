//! Governance SP2b Task 16: `explain` for a profile on OPA, answered by the
//! IR evaluator (`egret-perm-eval`) over the profile's snapshot — the same
//! evaluator, over the same snapshot, the OPA surface answers Trino from.
//!
//! **The snapshot is chosen as the refresher chooses it** (review I1/I2), so
//! that explain answers what the surface answers, with the one exception
//! below:
//!
//! - the cache's `Ready` snapshot when its entry was decided at the profile's
//!   current `(policy_version, tag_version)` (read in explain's own read-only
//!   transaction), or when the snapshot's own key is that key — spliced or
//!   not, it is what the surface serves;
//! - the cache's `NotBuilt` decided at the current key: refused, with the
//!   refresher's refusal class;
//! - otherwise [`build_outcome_via`], the refresher's own build (the C8
//!   stored-state check, data ++ infrastructure, and the C4 splice into the
//!   cached snapshot as `previous`), in explain's transaction under a
//!   savepoint that is always rolled back, with [`crate::opa::build::MissingUsername::Omit`],
//!   which never mints. A failing read cannot poison the transaction the rest
//!   of the request still reads from.
//!
//! A spliced snapshot answers from the previous data, and every `ir_reason`
//! says so.
//!
//! **The dropped-snapshot window (final review T16 m-R1).** Explain cannot
//! see a snapshot the refresher dropped. When the profile's key has moved and
//! the refresher's next read then fails (`Outcome::Unread`, a transient
//! database error), the refresher drops the held snapshot because it no
//! longer describes the store, and the surface refuses every request of that
//! profile: fail closed. That entry is decided at no key, so explain does not
//! take it as the surface's answer; it runs its own build instead, which can
//! succeed, and then shows `Allowed` where the engine is refused. The window
//! lasts until the refresher's next good build, normally the next period
//! (`REFRESH_PERIOD`, 1 s). It is not an enforcement gap: in it explain can
//! only be looser than the engine, never the engine looser than egret's
//! model.
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

use egret_perm::{Action, EngineType, Scope};
use egret_perm_eval::trino_ops::DataPrivilege;
use egret_perm_eval::{render_mask, Decision, EvalError, ObjectPath, Snapshot};

use super::{ranger_engine, ExplainAccess, ExplainProfile, PermHttpError};
use crate::explain_wire::{
    ExplainOpa, WireDecision, WireDenyReason, WireEngineType, WireMechanism,
};
use crate::opa::cache::{build_outcome_via, Lookup, Outcome, SnapshotCache};

/// The column type explain renders a mask with: the engine substitutes the
/// column's own type, which explain does not know.
const EXPLAIN_MASK_COLUMN_TYPE: &str = "varchar";

/// What explain evaluates an OPA profile with.
pub(super) struct Served {
    pub snapshot: Arc<Snapshot>,
    /// The data lowering failed at the current key, and the surface serves
    /// the previous data with fresh infrastructure (C4).
    pub spliced: bool,
}

/// The snapshot explain evaluates, or the `ir_reason` every access of the
/// profile is refused with.
pub(super) type ExplainSnapshot = Result<Served, String>;

/// What the surface serves for `service_id` at `key` (see the module doc).
pub(super) async fn opa_snapshot_for_explain(
    conn: &mut sqlx::PgConnection,
    cache: Option<&SnapshotCache>,
    service_id: i64,
    key: (i64, i64),
) -> Result<ExplainSnapshot, PermHttpError> {
    let (lookup, decided) = match cache {
        Some(c) => (c.get(service_id), c.decided_at(service_id)),
        None => (Lookup::NotOpa, None),
    };
    let decided_now = decided.is_some_and(|(k, _)| k == key);
    let previous = match lookup {
        Lookup::Ready(s) if decided_now || s.key() == key => {
            let spliced = s.key() != key;
            return Ok(Ok(Served {
                snapshot: s,
                spliced,
            }));
        }
        Lookup::Ready(s) => Some(s),
        Lookup::NotBuilt { .. } if decided_now => {
            let class = decided.and_then(|(_, c)| c).unwrap_or("NotBuilt");
            return Ok(Err(refusal(class)));
        }
        Lookup::NotBuilt { .. } | Lookup::NotOpa => None,
    };
    let mut savepoint = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(PermHttpError::db)?;
    let built = build_outcome_via(&mut savepoint, service_id, key, previous).await;
    savepoint.rollback().await.map_err(PermHttpError::db)?;
    Ok(match built.outcome {
        Outcome::Rebuilt(s) => Ok(Served {
            snapshot: Arc::new(s),
            spliced: false,
        }),
        Outcome::Spliced { snapshot, .. } => Ok(Served {
            snapshot: Arc::new(snapshot),
            spliced: true,
        }),
        Outcome::Failed { class, .. } => Err(refusal(class)),
        // A database error: the refresher retries; explain refuses.
        Outcome::Unread { .. } => Err(refusal("Unread")),
    })
}

fn refusal(class: &str) -> String {
    format!("no OPA snapshot: build failed ({class})")
}

/// Appended to every `ir_reason` of a spliced snapshot.
const SPLICED_NOTE: &str =
    " (from the previous snapshot: the current model did not build, and the engine is answered \
     from the previous data)";

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
        Ok(served) => match opa_values(&served.snapshot, i) {
            Ok(opa) => Ok((served.snapshot.as_ref(), opa)),
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
        let ir_reason = match (&answered, i.snapshot) {
            (Ok(_), Ok(Served { spliced: true, .. })) => format!("{ir_reason}{SPLICED_NOTE}"),
            _ => ir_reason,
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
        engines_not_reapplied: Vec::new(),
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
