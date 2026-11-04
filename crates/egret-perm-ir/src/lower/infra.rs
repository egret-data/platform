//! Infrastructure statements: the grants the platform itself needs on an
//! enforcement point, derived from the engines bound to it rather than from
//! any authored model object. Pure; the caller loads [`InfraInputs`].

use egret_core::engine_principals::DELEGATOR_PRINCIPAL;
use egret_perm::EngineType;

use crate::audience::{Audience, Member, MemberName, Via};
use crate::statement::{Derivation, Effect, InfraKind, InfraObject, Source, Statement, Target};

/// The Trino system session property that bounds a query's run time.
pub const QUERY_TIMEOUT_PROPERTY: &str = "query_max_run_time";

/// What the infrastructure statements of one profile depend on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfraInputs {
    pub engine: EngineType,
    /// Engine admins, sorted and deduped.
    pub admins: Vec<String>,
    /// At least one qualifying engine is bound.
    pub delegating: bool,
    /// At least one delegator-impersonating Trino engine is bound.
    pub delegator_impersonates: bool,
}

fn user(name: &str) -> Member {
    Member {
        name: MemberName::User(name.to_string()),
        via: Via::Direct,
    }
}

fn statement(
    kind: InfraKind,
    include: Vec<Member>,
    object: InfraObject,
    capability: &str,
) -> Statement {
    Statement {
        source: Source::Infra { kind },
        audience: Audience {
            include,
            except: Vec::new(),
        },
        target: Target::Infra(object),
        effect: Effect::EnginePrivilege {
            capability: capability.to_string(),
        },
        derivation: Derivation::AsDeclared,
    }
}

/// The infrastructure statements for `i`, in the order `AdminAllAccess`,
/// `DelegatorImpersonate`, `SelfImpersonate`, `ExecuteQuery`, `QueryTimeout`.
pub fn lower_infra(i: &InfraInputs) -> Vec<Statement> {
    let mut out = Vec::new();
    if !i.delegating {
        return out;
    }
    if !i.admins.is_empty() {
        out.push(statement(
            InfraKind::AdminAllAccess,
            i.admins.iter().map(|a| user(a)).collect(),
            InfraObject::WholeEp,
            "all",
        ));
    }
    if i.engine != EngineType::Trino {
        return out;
    }
    if i.delegator_impersonates {
        out.push(statement(
            InfraKind::DelegatorImpersonate,
            vec![user(DELEGATOR_PRINCIPAL)],
            InfraObject::AnyUser,
            "impersonate",
        ));
    }
    let everyone = vec![Member {
        name: MemberName::Everyone,
        via: Via::Direct,
    }];
    out.push(statement(
        InfraKind::SelfImpersonate,
        everyone.clone(),
        InfraObject::SessionUser,
        "impersonate",
    ));
    out.push(statement(
        InfraKind::ExecuteQuery,
        everyone.clone(),
        InfraObject::AnyQuery,
        "execute",
    ));
    out.push(statement(
        InfraKind::QueryTimeout,
        everyone,
        InfraObject::SystemProperty(QUERY_TIMEOUT_PROPERTY.to_string()),
        "alter",
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audience::MemberName;
    use crate::statement::{Effect, InfraKind, InfraObject, Source, Target};
    use egret_perm::EngineType;

    fn kinds(v: &[Statement]) -> Vec<InfraKind> {
        v.iter()
            .map(|s| match s.source {
                Source::Infra { kind } => kind,
                _ => panic!(),
            })
            .collect()
    }
    fn inputs(engine: EngineType, admins: &[&str], delegating: bool, imp: bool) -> InfraInputs {
        InfraInputs {
            engine,
            admins: admins.iter().map(|s| s.to_string()).collect(),
            delegating,
            delegator_impersonates: imp,
        }
    }

    #[test]
    fn nothing_without_a_qualifying_engine() {
        assert!(lower_infra(&inputs(EngineType::Trino, &["a"], false, false)).is_empty());
    }

    #[test]
    fn trino_impersonate_emits_all_five_in_order() {
        let s = lower_infra(&inputs(EngineType::Trino, &["a", "b"], true, true));
        assert_eq!(
            kinds(&s),
            vec![
                InfraKind::AdminAllAccess,
                InfraKind::DelegatorImpersonate,
                InfraKind::SelfImpersonate,
                InfraKind::ExecuteQuery,
                InfraKind::QueryTimeout
            ]
        );
        assert_eq!(s[0].target, Target::Infra(InfraObject::WholeEp));
        assert_eq!(
            s[0].effect,
            Effect::EnginePrivilege {
                capability: "all".into()
            }
        );
        let admins: Vec<_> = s[0]
            .audience
            .include
            .iter()
            .map(|m| m.name.clone())
            .collect();
        assert_eq!(
            admins,
            vec![MemberName::User("a".into()), MemberName::User("b".into())]
        );
        assert_eq!(s[1].target, Target::Infra(InfraObject::AnyUser));
        assert_eq!(
            s[1].audience.include[0].name,
            MemberName::User(DELEGATOR_PRINCIPAL.into())
        );
        assert_eq!(s[2].target, Target::Infra(InfraObject::SessionUser));
        assert_eq!(s[2].audience.include[0].name, MemberName::Everyone);
        assert_eq!(s[3].target, Target::Infra(InfraObject::AnyQuery));
        assert_eq!(
            s[4].target,
            Target::Infra(InfraObject::SystemProperty("query_max_run_time".into()))
        );
    }

    #[test]
    fn jwt_trino_has_no_delegator_grant() {
        let s = lower_infra(&inputs(EngineType::Trino, &["a"], true, false));
        assert!(!kinds(&s).contains(&InfraKind::DelegatorImpersonate));
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn no_admins_means_no_admin_statement() {
        let s = lower_infra(&inputs(EngineType::Trino, &[], true, false));
        assert!(!kinds(&s).contains(&InfraKind::AdminAllAccess));
    }

    #[test]
    fn starrocks_gets_the_admin_set_only() {
        let s = lower_infra(&inputs(EngineType::StarRocks, &["a"], true, true));
        assert_eq!(kinds(&s), vec![InfraKind::AdminAllAccess]);
    }
}
