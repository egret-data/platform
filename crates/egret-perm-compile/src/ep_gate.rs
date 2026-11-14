//! SP3d: the per-EP migration-complete gate. A pure verdict over an EP's own
//! rows — true only when no unconvertible operator policy remains (SP3b) and the
//! EP's role/workspace/clearance input is migrated (SP3c). The scoper that reads
//! those inputs lives in `ep_gate_scope`; this module decides nothing from the
//! database. Modeled on `egret_policy::opa_guard::stored_state_refusals`
//! ("empty = ready").

use crate::classify::EpMigrationStatus;

/// One reason an EP is not yet migration-complete. Order is stable (operator →
/// role → workspace → clearance) so the surfaced list and the tests agree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GateRefusal {
    /// Operator (hand-written) policies the EP still carries (SP3b).
    OperatorPolicies { pending: usize, blocked: usize },
    /// `kind='role'` principal refs the EP still names (SP3c-1).
    RolePrincipals { count: i64 },
    /// Implicit workspace grants the EP still carries (SP3c-3).
    WorkspaceGrants { count: i64 },
    /// Gated clearance tags with no `withhold` rule yet (SP3c-2, installation-wide).
    ClearanceTags { tags: Vec<String> },
}

impl GateRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::OperatorPolicies { pending, blocked } => format!(
                "{pending} operator policies await migration and {blocked} cannot be migrated"
            ),
            Self::RolePrincipals { count } => {
                format!("{count} role principal refs remain; migrate them to managed groups")
            }
            Self::WorkspaceGrants { count } => format!(
                "{count} implicit workspace grants remain; migrate them to `ws:<slug> members` groups"
            ),
            Self::ClearanceTags { tags } => format!(
                "{} gated clearance tags have no withhold rule yet: {}",
                tags.len(),
                tags.join(", ")
            ),
        }
    }
}

/// Whether an EP's migration is complete, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct GateVerdict {
    pub complete: bool,
    pub refusals: Vec<GateRefusal>,
}

/// Decide the verdict from the four already-computed inputs. Pure.
pub fn ep_gate(
    mig: &EpMigrationStatus,
    role_refs: i64,
    ws_grants: i64,
    unmigrated_clearance_tags: Vec<String>,
) -> GateVerdict {
    let mut refusals = Vec::new();
    if !mig.complete {
        refusals.push(GateRefusal::OperatorPolicies {
            pending: mig.pending,
            blocked: mig.blocked.len(),
        });
    }
    if role_refs > 0 {
        refusals.push(GateRefusal::RolePrincipals { count: role_refs });
    }
    if ws_grants > 0 {
        refusals.push(GateRefusal::WorkspaceGrants { count: ws_grants });
    }
    if !unmigrated_clearance_tags.is_empty() {
        refusals.push(GateRefusal::ClearanceTags {
            tags: unmigrated_clearance_tags,
        });
    }
    GateVerdict {
        complete: refusals.is_empty(),
        refusals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::EpMigrationStatus;

    fn done() -> EpMigrationStatus {
        EpMigrationStatus {
            complete: true,
            migrated: 3,
            pending: 0,
            blocked: vec![],
        }
    }

    #[test]
    fn all_inputs_clean_is_complete() {
        let v = ep_gate(&done(), 0, 0, vec![]);
        assert!(v.complete);
        assert!(v.refusals.is_empty());
    }

    #[test]
    fn a_pending_operator_policy_blocks_completion() {
        let mig = EpMigrationStatus {
            complete: false,
            migrated: 1,
            pending: 2,
            blocked: vec![],
        };
        let v = ep_gate(&mig, 0, 0, vec![]);
        assert!(!v.complete);
        assert_eq!(
            v.refusals,
            vec![GateRefusal::OperatorPolicies {
                pending: 2,
                blocked: 0
            }]
        );
    }

    #[test]
    fn role_workspace_and_clearance_each_produce_their_refusal_in_order() {
        let v = ep_gate(&done(), 2, 1, vec!["pii".into(), "phi".into()]);
        assert!(!v.complete);
        assert_eq!(
            v.refusals,
            vec![
                GateRefusal::RolePrincipals { count: 2 },
                GateRefusal::WorkspaceGrants { count: 1 },
                GateRefusal::ClearanceTags {
                    tags: vec!["pii".into(), "phi".into()]
                },
            ]
        );
    }

    #[test]
    fn each_refusal_has_a_nonempty_message() {
        for r in [
            GateRefusal::OperatorPolicies {
                pending: 1,
                blocked: 1,
            },
            GateRefusal::RolePrincipals { count: 1 },
            GateRefusal::WorkspaceGrants { count: 1 },
            GateRefusal::ClearanceTags {
                tags: vec!["pii".into()],
            },
        ] {
            assert!(!r.message().is_empty());
        }
    }
}
