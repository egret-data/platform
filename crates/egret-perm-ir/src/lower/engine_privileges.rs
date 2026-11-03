//! The engine-privilege family's decisions (governance sub-project 1, Task
//! 9): which engine grants a profile lowers, which it skips and why. An
//! engine grant (F11, spec §4) hands its grantees one engine-level capability
//! from the catalogue — impersonating a user, setting a session property,
//! running a procedure — on the values it names.
//!
//! What a grant may name is the catalogue's to say, and the impersonation
//! target is a user read from the store (spec E21, E11), so both checks run in
//! the loader, which hands each grant's verdict in as a [`Check`]. This module
//! decides the order the skips are reported in and checks the grantees, the
//! same check a grant's principals get
//! ([`crate::lower::grants::check_expanded_principal`]).

use std::collections::BTreeMap;

use egret_perm::{CompileError, EngineGrant, PrincipalSet};
use uuid::Uuid;

use crate::audience::audience_from_expanded;
use crate::lower::grants::check_expanded_principal;
use crate::statement::{
    Derivation, Effect, LowerResult, Lowered, Lowering, ReasonCode, Source, Statement, Target,
};

/// The loader's verdict on one engine grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// The capability and its values, in the catalogue's key order with the
    /// impersonation target's current username in its slot, as checked.
    Ok {
        capability: String,
        values: Vec<String>,
    },
    /// The grant cannot be compiled this time; the text is the reason the
    /// recompile reports.
    Skip(String),
}

/// The text an engine grant with an empty include is skipped with, unchanged
/// from the recompile driver that used to hold this loop.
const EMPTY_AUDIENCE: &str = "the engine grant's principals name no engine principal (a role or \
                              group reference that expands to no member)";

/// Lower `grants` (already expanded) with the loader's `checks`, keyed by
/// grant id. Each grant, in order:
///
/// 1. an empty include is skipped ([`ReasonCode::EmptyAudience`]) BEFORE its
///    check is consulted, so the loader need not check it at all — today's
///    order, in which a grant that is both empty and has a dead impersonation
///    target reports the empty include;
/// 2. a [`Check::Skip`] is skipped with its text
///    ([`ReasonCode::InvalidEnginePrivilege`]);
/// 3. otherwise the grant lowers through [`engine_privilege_statement`], whose
///    `UnexpandedPrincipalKind` is skipped with the error's text
///    ([`ReasonCode::UnexpandedPrincipal`]) and every other error aborts.
///
/// A non-empty grant with no check is a loader bug and aborts.
pub fn lower_engine_privileges(
    grants: &[EngineGrant],
    checks: &BTreeMap<Uuid, Check>,
) -> LowerResult {
    let mut out = Lowered::default();
    for g in grants {
        let source = Source::EnginePrivilege {
            engine_grant_id: g.id,
        };
        let mut skip = |code: ReasonCode, detail: String| {
            out.lowerings.push(Lowering {
                source: source.clone(),
                code,
                detail,
            })
        };
        if g.principals.include.is_empty() {
            skip(ReasonCode::EmptyAudience, EMPTY_AUDIENCE.to_string());
            continue;
        }
        let (capability, values) = match checks.get(&g.id) {
            Some(Check::Ok { capability, values }) => (capability, values),
            Some(Check::Skip(reason)) => {
                skip(ReasonCode::InvalidEnginePrivilege, reason.clone());
                continue;
            }
            None => {
                return Err(CompileError::Db(format!(
                    "engine grant {} reached the lowering unchecked",
                    g.id
                )))
            }
        };
        match engine_privilege_statement(g, capability, values) {
            Ok(s) => out.statements.push(s),
            Err(e @ CompileError::UnexpandedPrincipalKind { .. }) => {
                skip(ReasonCode::UnexpandedPrincipal, e.to_string())
            }
            Err(other) => return Err(other),
        }
    }
    Ok(out)
}

/// The statement one checked engine grant lowers to: its include as the
/// audience (an engine grant is include-only, which the loader's check
/// enforces), its capability and values as the target. Each grantee is
/// checked first, include order, by
/// [`crate::lower::grants::check_expanded_principal`], so an engine grant's
/// grantees are refused exactly as a grant's are. An empty include is not
/// refused here: the statement then names nobody.
pub fn engine_privilege_statement(
    g: &EngineGrant,
    capability: &str,
    values: &[String],
) -> Result<Statement, CompileError> {
    for p in &g.principals.include {
        check_expanded_principal(p)?;
    }
    Ok(Statement {
        source: Source::EnginePrivilege {
            engine_grant_id: g.id,
        },
        audience: audience_from_expanded(&PrincipalSet {
            include: g.principals.include.clone(),
            exclude: Vec::new(),
        }),
        target: Target::EngineObject {
            capability: capability.to_string(),
            values: values.to_vec(),
        },
        effect: Effect::EnginePrivilege {
            capability: capability.to_string(),
        },
        derivation: Derivation::AsDeclared,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statement::*;
    use egret_perm::{Principal, PrincipalKind, PrincipalSet};

    const CAP: &str = "trino.set_system_session_property";

    fn eg(include: Vec<Principal>, target: Option<Uuid>) -> EngineGrant {
        EngineGrant {
            id: Uuid::from_u128(1),
            engine_id: Uuid::from_u128(2),
            capability: CAP.into(),
            values: vec!["query_max_memory".into()],
            target_user_id: target,
            principals: PrincipalSet {
                include,
                exclude: vec![],
            },
        }
    }
    fn user(n: &str) -> Principal {
        Principal {
            kind: PrincipalKind::User,
            r#ref: n.into(),
        }
    }
    fn ok() -> Check {
        Check::Ok {
            capability: CAP.into(),
            values: vec!["query_max_memory".into()],
        }
    }

    #[test]
    fn an_empty_include_is_skipped_with_todays_text() {
        let g = eg(vec![], None);
        let out =
            lower_engine_privileges(std::slice::from_ref(&g), &[(g.id, ok())].into()).unwrap();
        assert!(out.statements.is_empty());
        assert_eq!(out.lowerings[0].code, ReasonCode::EmptyAudience);
        assert_eq!(
            out.lowerings[0].detail,
            "the engine grant's principals name no engine principal (a role or group \
             reference that expands to no member)"
        );
    }

    /// The order trap: today the empty include is tested before the
    /// impersonation target, so a grant that is both keeps the empty text.
    #[test]
    fn an_empty_include_wins_over_a_refused_check() {
        let g = eg(vec![], Some(Uuid::from_u128(3)));
        let out = lower_engine_privileges(
            std::slice::from_ref(&g),
            &[(g.id, Check::Skip("impersonation target gone".into()))].into(),
        )
        .unwrap();
        assert_eq!(out.lowerings[0].code, ReasonCode::EmptyAudience);
        // And the loader need not have checked it at all.
        let out = lower_engine_privileges(&[g], &BTreeMap::new()).unwrap();
        assert_eq!(out.lowerings[0].code, ReasonCode::EmptyAudience);
    }

    #[test]
    fn a_checked_grant_is_one_engine_privilege_statement() {
        let g = eg(vec![user("ann")], None);
        let out =
            lower_engine_privileges(std::slice::from_ref(&g), &[(g.id, ok())].into()).unwrap();
        assert!(out.lowerings.is_empty());
        let s = &out.statements[0];
        assert_eq!(
            s.source,
            Source::EnginePrivilege {
                engine_grant_id: g.id
            }
        );
        assert_eq!(
            s.effect,
            Effect::EnginePrivilege {
                capability: CAP.into()
            }
        );
        assert_eq!(
            s.target,
            Target::EngineObject {
                capability: CAP.into(),
                values: vec!["query_max_memory".into()]
            }
        );
        assert_eq!(
            s.audience.include[0].name,
            crate::audience::MemberName::User("ann".into())
        );
        assert!(s.audience.except.is_empty());
    }

    #[test]
    fn a_refused_check_is_a_skip() {
        let g = eg(vec![user("ann")], Some(Uuid::from_u128(3)));
        let out = lower_engine_privileges(
            std::slice::from_ref(&g),
            &[(g.id, Check::Skip("impersonation target gone".into()))].into(),
        )
        .unwrap();
        assert!(out.statements.is_empty());
        assert_eq!(out.lowerings[0].code, ReasonCode::InvalidEnginePrivilege);
        assert_eq!(out.lowerings[0].detail, "impersonation target gone");
    }

    #[test]
    fn an_unexpanded_grantee_is_a_skip_with_the_errors_text() {
        let g = eg(
            vec![Principal {
                kind: PrincipalKind::Role,
                r#ref: "r".into(),
            }],
            None,
        );
        let out =
            lower_engine_privileges(std::slice::from_ref(&g), &[(g.id, ok())].into()).unwrap();
        assert_eq!(out.lowerings[0].code, ReasonCode::UnexpandedPrincipal);
        assert_eq!(
            out.lowerings[0].detail,
            CompileError::UnexpandedPrincipalKind {
                kind: PrincipalKind::Role
            }
            .to_string()
        );
    }

    #[test]
    fn an_unusable_grantee_aborts() {
        let g = eg(vec![user("a@b")], None);
        assert!(matches!(
            lower_engine_privileges(std::slice::from_ref(&g), &[(g.id, ok())].into()),
            Err(CompileError::UnusableUserRef { .. })
        ));
    }

    #[test]
    fn skips_keep_grant_order() {
        let mut a = eg(vec![user("ann")], None);
        a.id = Uuid::from_u128(20);
        let mut b = eg(vec![], None);
        b.id = Uuid::from_u128(10);
        let mut c = eg(vec![user("ann")], None);
        c.id = Uuid::from_u128(5);
        let checks = [(a.id, Check::Skip("a".into())), (c.id, ok())].into();
        let out = lower_engine_privileges(&[a.clone(), b.clone(), c.clone()], &checks).unwrap();
        let ids: Vec<_> = out.lowerings.iter().map(|l| l.source.clone()).collect();
        assert_eq!(
            ids,
            vec![
                Source::EnginePrivilege {
                    engine_grant_id: a.id
                },
                Source::EnginePrivilege {
                    engine_grant_id: b.id
                },
            ]
        );
        assert_eq!(out.statements.len(), 1);
    }
}
