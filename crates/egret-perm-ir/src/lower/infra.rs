//! Infrastructure statements: the grants the platform itself needs on an
//! enforcement point, derived from the engines bound to it rather than from
//! any authored model object. Pure; the caller loads [`InfraInputs`].

use egret_core::engine_principals::{
    DELEGATOR_PRINCIPAL, QUERY_LOG_LOADER_PRINCIPAL, STARROCKS_RESERVED_USERS,
};
use egret_perm::EngineType;

use crate::audience::{Audience, Member, MemberName, Via};
use crate::statement::{
    Derivation, Effect, InfraKind, InfraObject, Privilege, Source, Statement, Target,
};

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
    /// At least one bound engine impersonates with `egret_delegator`
    /// (`egret_core::engine_delegation::delegator_impersonates`).
    pub delegator_impersonates: bool,
    /// At least one StarRocks engine egret provisioned (origin not external)
    /// is bound: its AuditLoader writes the query log. Independent of
    /// `delegating`.
    pub query_log: bool,
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
/// `QueryLogLoader`, `QueryLogGuard` (StarRocks only), `DelegatorImpersonate`,
/// `SelfImpersonate`, `ExecuteQuery`, `QueryTimeout` (the last three Trino
/// only).
///
/// `AdminAllAccess` and the Trino statements need a delegating engine; the
/// two query-log statements need only `query_log`, so a StarRocks engine that
/// does not delegate still gets its loader allow and its guard.
pub fn lower_infra(i: &InfraInputs) -> Vec<Statement> {
    let mut out = Vec::new();
    if i.delegating && !i.admins.is_empty() {
        out.push(statement(
            InfraKind::AdminAllAccess,
            i.admins.iter().map(|a| user(a)).collect(),
            InfraObject::WholeEp,
            "all",
        ));
    }
    if i.engine == EngineType::StarRocks && i.query_log {
        out.push(statement(
            InfraKind::QueryLogLoader,
            vec![user(QUERY_LOG_LOADER_PRINCIPAL)],
            InfraObject::QueryLogTable,
            "insert",
        ));
        out.push(Statement {
            source: Source::Infra {
                kind: InfraKind::QueryLogGuard,
            },
            audience: Audience {
                include: vec![Member {
                    name: MemberName::Everyone,
                    via: Via::Direct,
                }],
                except: vec![user(QUERY_LOG_LOADER_PRINCIPAL)],
            },
            target: Target::Infra(InfraObject::QueryLogDatabase),
            effect: Effect::Deny {
                privileges: vec![Privilege::Read, Privilege::Write],
            },
            derivation: Derivation::AsDeclared,
        });
    }
    if i.engine == EngineType::StarRocks && i.delegating && i.delegator_impersonates {
        out.push(statement(
            InfraKind::DelegatorImpersonate,
            vec![user(DELEGATOR_PRINCIPAL)],
            InfraObject::AnyUserExcept(
                STARROCKS_RESERVED_USERS
                    .iter()
                    .map(|u| u.to_string())
                    .collect(),
            ),
            "impersonate",
        ));
    }
    if !i.delegating || i.engine != EngineType::Trino {
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
            query_log: false,
        }
    }
    fn with_query_log(mut i: InfraInputs) -> InfraInputs {
        i.query_log = true;
        i
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

    fn reserved() -> Vec<String> {
        vec![
            "egret_delegator".into(),
            "egret_query_log".into(),
            "root".into(),
        ]
    }

    /// Package D, D1b: a StarRocks profile whose engine impersonates with the
    /// delegator gets the delegator's impersonation, with root, the delegator and
    /// the query-log loader excluded, and none of the Trino baseline.
    #[test]
    fn starrocks_impersonate_emits_the_delegator_row_without_the_reserved_users() {
        let s = lower_infra(&inputs(EngineType::StarRocks, &["a"], true, true));
        assert_eq!(
            kinds(&s),
            vec![InfraKind::AdminAllAccess, InfraKind::DelegatorImpersonate]
        );
        assert_eq!(
            s[1].target,
            Target::Infra(InfraObject::AnyUserExcept(reserved()))
        );
        assert_eq!(s[1].audience.include, vec![user("egret_delegator")]);
        assert!(s[1].audience.except.is_empty());
        assert_eq!(
            s[1].effect,
            Effect::EnginePrivilege {
                capability: "impersonate".into()
            }
        );
    }

    #[test]
    fn starrocks_with_the_query_log_orders_the_delegator_row_after_it() {
        let s = lower_infra(&with_query_log(inputs(
            EngineType::StarRocks,
            &["a"],
            true,
            true,
        )));
        assert_eq!(
            kinds(&s),
            vec![
                InfraKind::AdminAllAccess,
                InfraKind::QueryLogLoader,
                InfraKind::QueryLogGuard,
                InfraKind::DelegatorImpersonate
            ]
        );
    }

    #[test]
    fn starrocks_without_the_delegator_or_not_delegating_emits_no_impersonation() {
        let s = lower_infra(&inputs(EngineType::StarRocks, &["a"], true, false));
        assert_eq!(kinds(&s), vec![InfraKind::AdminAllAccess]);
        assert!(lower_infra(&inputs(EngineType::StarRocks, &["a"], false, true)).is_empty());
    }

    #[test]
    fn starrocks_with_the_query_log_gets_the_loader_then_the_guard() {
        let s = lower_infra(&with_query_log(inputs(
            EngineType::StarRocks,
            &["a"],
            true,
            false,
        )));
        assert_eq!(
            kinds(&s),
            vec![
                InfraKind::AdminAllAccess,
                InfraKind::QueryLogLoader,
                InfraKind::QueryLogGuard
            ]
        );
        let loader = &s[1];
        assert_eq!(loader.target, Target::Infra(InfraObject::QueryLogTable));
        assert_eq!(
            loader.effect,
            Effect::EnginePrivilege {
                capability: "insert".into()
            }
        );
        assert_eq!(
            loader
                .audience
                .include
                .iter()
                .map(|m| m.name.clone())
                .collect::<Vec<_>>(),
            vec![MemberName::User(QUERY_LOG_LOADER_PRINCIPAL.into())]
        );
        assert!(loader.audience.except.is_empty());
        let guard = &s[2];
        assert_eq!(guard.target, Target::Infra(InfraObject::QueryLogDatabase));
        assert_eq!(
            guard.effect,
            Effect::Deny {
                privileges: vec![Privilege::Read, Privilege::Write]
            }
        );
        assert_eq!(
            guard
                .audience
                .include
                .iter()
                .map(|m| m.name.clone())
                .collect::<Vec<_>>(),
            vec![MemberName::Everyone]
        );
        assert_eq!(
            guard
                .audience
                .except
                .iter()
                .map(|m| m.name.clone())
                .collect::<Vec<_>>(),
            vec![MemberName::User(QUERY_LOG_LOADER_PRINCIPAL.into())]
        );
    }

    #[test]
    fn starrocks_without_the_query_log_gets_neither_query_log_kind() {
        let s = lower_infra(&inputs(EngineType::StarRocks, &["a"], true, false));
        assert_eq!(kinds(&s), vec![InfraKind::AdminAllAccess]);
    }

    /// Correction 5: the query-log statements do not wait on delegation.
    #[test]
    fn a_starrocks_engine_that_does_not_delegate_still_gets_the_query_log() {
        let s = lower_infra(&with_query_log(inputs(
            EngineType::StarRocks,
            &["a"],
            false,
            false,
        )));
        assert_eq!(
            kinds(&s),
            vec![InfraKind::QueryLogLoader, InfraKind::QueryLogGuard],
            "no AdminAllAccess without delegation, but both query-log statements"
        );
    }

    /// The Trino evaluator (`egret-perm-eval`'s `decide`) never sees the
    /// query-log objects: a Trino profile does not lower them.
    #[test]
    fn trino_with_query_log_set_emits_neither_query_log_kind() {
        for delegating in [true, false] {
            let s = lower_infra(&with_query_log(inputs(
                EngineType::Trino,
                &["a"],
                delegating,
                true,
            )));
            let k = kinds(&s);
            assert!(
                !k.contains(&InfraKind::QueryLogLoader) && !k.contains(&InfraKind::QueryLogGuard),
                "delegating={delegating}: {k:?}"
            );
            assert_eq!(
                s,
                lower_infra(&inputs(EngineType::Trino, &["a"], delegating, true)),
                "query_log changed a Trino lowering"
            );
        }
    }
}
