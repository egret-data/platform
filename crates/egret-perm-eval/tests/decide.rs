//! Task 3: the snapshot, path matching, `decide` and `filter` (plan D2).

use egret_perm_eval::{ClassifiedColumn, Decision, Resource, Snapshot, SnapshotInput};
use egret_perm_ir::audience::{Audience, Member, MemberName, Via};
use egret_perm_ir::statement::*;
use uuid::Uuid;

fn who(users: &[&str], except: &[&str]) -> Audience {
    let m = |n: &str| Member {
        name: if n == "*" {
            MemberName::Everyone
        } else {
            MemberName::User(n.into())
        },
        via: Via::Direct,
    };
    Audience {
        include: users.iter().map(|u| m(u)).collect(),
        except: except.iter().map(|u| m(u)).collect(),
    }
}
fn path(c: &str, s: Option<&str>, t: Option<&str>, col: Option<&str>) -> Target {
    let l = |v: Option<&str>| v.map_or(Level::Unnamed, |v| Level::Named(v.into()));
    Target::Path(Path {
        catalog_id: None,
        catalog: c.into(),
        schema: l(s),
        table: l(t),
        column: l(col),
    })
}
fn st(source_n: u128, audience: Audience, target: Target, effect: Effect) -> Statement {
    Statement {
        source: Source::Grant {
            grant_id: Uuid::from_u128(source_n),
        },
        audience,
        target,
        effect,
        derivation: Derivation::AsDeclared,
    }
}
fn infra(kind: InfraKind, audience: Audience, object: InfraObject, cap: &str) -> Statement {
    Statement {
        source: Source::Infra { kind },
        audience,
        target: Target::Infra(object),
        effect: Effect::EnginePrivilege {
            capability: cap.into(),
        },
        derivation: Derivation::AsDeclared,
    }
}
fn snap(statements: Vec<Statement>, classified: Vec<ClassifiedColumn>) -> Snapshot {
    Snapshot::build(SnapshotInput {
        service_id: 1,
        key: (1, 1),
        statements,
        classified,
        lineage_principal: None,
    })
    .unwrap()
}
fn cols(c: &str, s: &str, t: &str, cs: &[&str]) -> Resource {
    Resource::Table {
        catalog: c.into(),
        schema: s.into(),
        table: t.into(),
        columns: cs.iter().map(|x| x.to_string()).collect(),
    }
}
fn allowed(d: &Decision) -> bool {
    matches!(d, Decision::Allow { .. })
}

#[test]
fn a_read_grant_on_a_catalog_allows_select_of_any_column() {
    let s = snap(
        vec![st(
            1,
            who(&["ann"], &[]),
            path("lake", None, None, None),
            Effect::Allow {
                privileges: vec![Privilege::Read],
            },
        )],
        vec![],
    );
    assert!(allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["a", "b"])
    )));
    assert!(!allowed(&s.decide(
        "bob",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["a"])
    )));
    assert!(!allowed(&s.decide(
        "ann",
        "InsertIntoTable",
        &cols("lake", "s", "t", &[])
    )));
}

#[test]
fn deny_wins_over_allow_on_one_column() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), Some("ssn")),
                Effect::Deny {
                    privileges: vec![Privilege::Read],
                },
            ),
        ],
        vec![],
    );
    let d = s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["id", "ssn"]),
    );
    assert!(matches!(d, Decision::Deny { explicit: true, .. }), "{d:?}");
    assert!(allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["id"])
    )));
}

#[test]
fn an_except_member_is_not_in_the_audience() {
    let s = snap(
        vec![st(
            1,
            who(&["*"], &["bob"]),
            path("lake", None, None, None),
            Effect::Allow {
                privileges: vec![Privilege::Read],
            },
        )],
        vec![],
    );
    assert!(!allowed(&s.decide(
        "bob",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["a"])
    )));
}

#[test]
fn wildcards_match_case_insensitively() {
    let s = snap(
        vec![st(
            1,
            who(&["ann"], &[]),
            path("lake", Some("sa*"), Some("T?"), None),
            Effect::Allow {
                privileges: vec![Privilege::Read],
            },
        )],
        vec![],
    );
    assert!(allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("LAKE", "sales", "t1", &["x"])
    )));
    assert!(!allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "hr", "t1", &["x"])
    )));
}

#[test]
fn a_classification_withhold_refuses_the_tagged_column_and_the_whole_tagged_table() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read, Privilege::Write],
                },
            ),
            Statement {
                source: Source::Withhold {
                    classification_id: Uuid::nil(),
                    classification_name: "pii".into(),
                },
                audience: who(&["*"], &["cleared"]),
                target: Target::Classification { name: "pii".into() },
                effect: Effect::Withhold {
                    privileges: vec![Privilege::Read, Privilege::Write],
                },
                derivation: Derivation::AsDeclared,
            },
        ],
        vec![
            ClassifiedColumn {
                catalog: "lake".into(),
                schema: "s".into(),
                table: "t".into(),
                column: Some("ssn".into()),
                classification: "pii".into(),
            },
            ClassifiedColumn {
                catalog: "lake".into(),
                schema: "s".into(),
                table: "u".into(),
                column: None,
                classification: "pii".into(),
            },
        ],
    );
    assert!(!allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["ssn"])
    )));
    assert!(allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["id"])
    )));
    assert!(allowed(&s.decide(
        "cleared",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["ssn"])
    )));
    assert!(
        !allowed(&s.decide(
            "ann",
            "SelectFromColumns",
            &cols("lake", "s", "u", &["any"])
        )),
        "table tag rolls down"
    );
    assert!(
        !allowed(&s.decide("ann", "InsertIntoTable", &cols("lake", "s", "u", &[]))),
        "table tag on a table request"
    );
    assert!(
        allowed(&s.decide("ann", "InsertIntoTable", &cols("lake", "s", "t", &[]))),
        "a column tag is not on the table request"
    );
}

#[test]
fn reach_sees_a_grant_below_the_object() {
    let s = snap(
        vec![st(
            1,
            who(&["ann"], &[]),
            path("lake", Some("s"), Some("t"), None),
            Effect::Allow {
                privileges: vec![Privilege::Read],
            },
        )],
        vec![],
    );
    assert!(allowed(&s.decide(
        "ann",
        "AccessCatalog",
        &Resource::Catalog("lake".into())
    )));
    assert!(allowed(&s.decide(
        "ann",
        "ShowTables",
        &Resource::Schema {
            catalog: "lake".into(),
            schema: "s".into()
        }
    )));
    assert!(!allowed(&s.decide(
        "ann",
        "ShowTables",
        &Resource::Schema {
            catalog: "lake".into(),
            schema: "other".into()
        }
    )));
    assert_eq!(
        s.filter(
            "ann",
            "FilterCatalogs",
            &[
                Resource::Catalog("x".into()),
                Resource::Catalog("lake".into())
            ]
        ),
        vec![1]
    );
}

#[test]
fn admin_only_needs_the_whole_ep_statement() {
    let s = snap(
        vec![infra(
            InfraKind::AdminAllAccess,
            who(&["root"], &[]),
            InfraObject::WholeEp,
            "all",
        )],
        vec![],
    );
    assert!(allowed(&s.decide(
        "root",
        "CreateTable",
        &cols("lake", "s", "t", &[])
    )));
    let d = s.decide("ann", "CreateTable", &cols("lake", "s", "t", &[]));
    assert!(
        matches!(&d, Decision::Deny { reason, .. } if reason.contains("all-access")),
        "{d:?}"
    );
}

/// Task 18 (opa_differential, seed 0x5b2b00000001 state 8): Ranger denied
/// the all-access holder `CreateViewWithExecuteFunction`, because egret's
/// all-access grants only `create, drop, show` on `schemafunction` (the
/// service-def's `accessTypeRestrictions`). Only an engine grant executes a
/// catalog function; the other capabilities and `FilterFunctions` (`_any`)
/// stay with the all-access.
#[test]
fn the_all_access_does_not_execute_a_catalog_function() {
    let admin = infra(
        InfraKind::AdminAllAccess,
        who(&["root"], &[]),
        InfraObject::WholeEp,
        "all",
    );
    let grant = Statement {
        source: Source::EnginePrivilege {
            engine_grant_id: Uuid::nil(),
        },
        audience: who(&["ann"], &[]),
        target: Target::EngineObject {
            capability: "trino.execute_function".into(),
            values: vec!["lake".into(), "s".into(), "f".into()],
        },
        effect: Effect::EnginePrivilege {
            capability: "trino.execute_function".into(),
        },
        derivation: Derivation::AsDeclared,
    };
    let s = snap(vec![admin, grant], vec![]);
    let f = Resource::Function {
        catalog: "lake".into(),
        schema: "s".into(),
        function: "f".into(),
    };
    for op in ["ExecuteFunction", "CreateViewWithExecuteFunction"] {
        assert!(
            !allowed(&s.decide("root", op, &f)),
            "{op} by the all-access"
        );
        assert!(allowed(&s.decide("ann", op, &f)), "{op} by an engine grant");
    }
    assert!(allowed(&s.decide("root", "FilterFunctions", &f)));
    assert!(allowed(&s.decide("root", "ExecuteProcedure", &f)));
    let property = Resource::CatalogSessionProperty {
        catalog: "lake".into(),
        property: "p".into(),
    };
    assert!(allowed(&s.decide(
        "root",
        "SetCatalogSessionProperty",
        &property
    )));
}

#[test]
fn impersonation_rules() {
    let s = snap(
        vec![
            infra(
                InfraKind::DelegatorImpersonate,
                who(&["egret_delegator"], &[]),
                InfraObject::AnyUser,
                "impersonate",
            ),
            infra(
                InfraKind::SelfImpersonate,
                who(&["*"], &[]),
                InfraObject::SessionUser,
                "impersonate",
            ),
            infra(
                InfraKind::ExecuteQuery,
                who(&["*"], &[]),
                InfraObject::AnyQuery,
                "execute",
            ),
            infra(
                InfraKind::QueryTimeout,
                who(&["*"], &[]),
                InfraObject::SystemProperty("query_max_run_time".into()),
                "alter",
            ),
        ],
        vec![],
    );
    assert!(allowed(&s.decide(
        "egret_delegator",
        "ImpersonateUser",
        &Resource::User("ann".into())
    )));
    assert!(allowed(&s.decide(
        "ann",
        "KillQueryOwnedBy",
        &Resource::User("ann".into())
    )));
    assert!(!allowed(&s.decide(
        "ann",
        "KillQueryOwnedBy",
        &Resource::User("bob".into())
    )));
    assert!(allowed(&s.decide("ann", "ExecuteQuery", &Resource::None)));
    assert!(allowed(&s.decide(
        "ann",
        "SetSystemSessionProperty",
        &Resource::SystemProperty("query_max_run_time".into())
    )));
    assert!(!allowed(&s.decide(
        "ann",
        "SetSystemSessionProperty",
        &Resource::SystemProperty("query_max_memory".into())
    )));
    assert_eq!(
        s.filter(
            "ann",
            "FilterViewQueryOwnedBy",
            &[Resource::User("bob".into()), Resource::User("ann".into())]
        ),
        vec![1]
    );
}

#[test]
fn an_engine_capability_allows_exactly_its_values() {
    let s = snap(
        vec![Statement {
            source: Source::EnginePrivilege {
                engine_grant_id: Uuid::nil(),
            },
            audience: who(&["ann"], &[]),
            target: Target::EngineObject {
                capability: "trino.execute_procedure".into(),
                values: vec!["lake".into(), "system".into(), "rollback".into()],
            },
            effect: Effect::EnginePrivilege {
                capability: "trino.execute_procedure".into(),
            },
            derivation: Derivation::AsDeclared,
        }],
        vec![],
    );
    let f = |p: &str| Resource::Function {
        catalog: "lake".into(),
        schema: "system".into(),
        function: p.into(),
    };
    assert!(allowed(&s.decide(
        "ann",
        "ExecuteProcedure",
        &f("rollback")
    )));
    assert!(!allowed(&s.decide("ann", "ExecuteProcedure", &f("expire"))));
    assert!(
        !allowed(&s.decide("ann", "ExecuteFunction", &f("rollback"))),
        "a procedure grant is not a function grant"
    );
}

#[test]
fn unknown_and_malformed_are_denied_with_a_reason() {
    let s = snap(vec![], vec![]);
    assert_eq!(
        s.decide("ann", "GrantTablePrivilege", &Resource::None),
        Decision::Deny {
            reason: "operation not modelled: GrantTablePrivilege".into(),
            explicit: false
        }
    );
    assert!(
        matches!(s.decide("ann", "SelectFromColumns", &Resource::Catalog("x".into())),
                     Decision::Deny { ref reason, .. } if reason.starts_with("malformed resource"))
    );
}

#[test]
fn a_metadata_table_is_refused_to_someone_a_mask_restricts() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), Some("email")),
                Effect::Mask {
                    mask_type: "MASK_HASH".into(),
                    expression: None,
                },
            ),
        ],
        vec![],
    );
    assert!(!allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t$files", &["lower_bounds"])
    )));
    assert!(allowed(&s.decide(
        "bob",
        "SelectFromColumns",
        &cols("lake", "s", "t$files", &["lower_bounds"])
    )));
}

#[test]
fn an_unexpanded_member_fails_the_build() {
    let p = egret_perm::Principal {
        kind: egret_perm::PrincipalKind::Role,
        r#ref: "r".into(),
    };
    let mut a = who(&[], &[]);
    a.include.push(Member {
        name: MemberName::Unexpanded(p),
        via: Via::Direct,
    });
    let e = Snapshot::build(SnapshotInput {
        service_id: 1,
        key: (1, 1),
        classified: vec![],
        lineage_principal: None,
        statements: vec![st(
            1,
            a,
            path("lake", None, None, None),
            Effect::Allow {
                privileges: vec![Privilege::Read],
            },
        )],
    });
    assert!(e.is_err());
}

#[test]
fn count_star_needs_a_table_level_read() {
    // SELECT count(*) arrives as SelectFromColumns with no columns (D1, Ranger's forColumns → forTable).
    let none = snap(vec![], vec![]);
    assert!(
        !allowed(&none.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &[]))),
        "no grant, no count(*)"
    );
    let catalog = snap(
        vec![st(
            1,
            who(&["ann"], &[]),
            path("lake", None, None, None),
            Effect::Allow {
                privileges: vec![Privilege::Read],
            },
        )],
        vec![],
    );
    assert!(allowed(&catalog.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t", &[])
    )));
    let column_only = snap(
        vec![st(
            1,
            who(&["ann"], &[]),
            path("lake", Some("s"), Some("t"), Some("id")),
            Effect::Allow {
                privileges: vec![Privilege::Read],
            },
        )],
        vec![],
    );
    assert!(
        !allowed(&column_only.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &[]))),
        "a column grant does not answer a table-level select"
    );
}

#[test]
fn a_withheld_column_is_not_reached() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            Statement {
                source: Source::Withhold {
                    classification_id: Uuid::nil(),
                    classification_name: "pii".into(),
                },
                audience: who(&["*"], &[]),
                target: Target::Classification { name: "pii".into() },
                effect: Effect::Withhold {
                    privileges: vec![Privilege::Read],
                },
                derivation: Derivation::AsDeclared,
            },
        ],
        vec![ClassifiedColumn {
            catalog: "lake".into(),
            schema: "s".into(),
            table: "t".into(),
            column: Some("ssn".into()),
            classification: "pii".into(),
        }],
    );
    let one = |c: &str| cols("lake", "s", "t", &[c]);
    assert_eq!(
        s.filter("ann", "FilterColumns", &[one("id"), one("ssn")]),
        vec![0],
        "the withheld column is hidden"
    );
    assert!(
        allowed(&s.decide("ann", "ShowColumns", &cols("lake", "s", "t", &[]))),
        "a deny on a descendant is ignored"
    );
}

#[test]
fn a_deny_carrying_any_privilege_on_an_ancestor_hides_the_object() {
    let s = snap(
        vec![
            st(
                1,
                who(&["ann"], &[]),
                path("lake", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read, Privilege::Write],
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), None, None),
                Effect::Deny {
                    privileges: vec![Privilege::Write],
                },
            ),
        ],
        vec![],
    );
    assert!(
        !allowed(&s.decide(
            "ann",
            "ShowTables",
            &Resource::Schema {
                catalog: "lake".into(),
                schema: "s".into()
            }
        )),
        "Ranger's _any: a deny item with any access type on the object refuses it"
    );
    assert!(
        allowed(&s.decide("ann", "AccessCatalog", &Resource::Catalog("lake".into()))),
        "the deny is on a descendant of the catalog"
    );
}

#[test]
fn an_impersonation_grant_is_not_catalog_reach() {
    let s = snap(
        vec![Statement {
            source: Source::EnginePrivilege {
                engine_grant_id: Uuid::nil(),
            },
            audience: who(&["ann"], &[]),
            target: Target::EngineObject {
                capability: "trino.impersonate_user".into(),
                values: vec!["lake".into()],
            },
            effect: Effect::EnginePrivilege {
                capability: "trino.impersonate_user".into(),
            },
            derivation: Derivation::AsDeclared,
        }],
        vec![],
    );
    assert!(
        !allowed(&s.decide("ann", "AccessCatalog", &Resource::Catalog("lake".into()))),
        "a username that happens to equal a catalog name is not a catalog"
    );
}

#[test]
fn with_infra_replaces_only_the_infrastructure() {
    let s = snap(
        vec![
            st(
                1,
                who(&["ann"], &[]),
                path("lake", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            infra(
                InfraKind::AdminAllAccess,
                who(&["root"], &[]),
                InfraObject::WholeEp,
                "all",
            ),
        ],
        vec![],
    );
    let t = s.with_infra(vec![]).unwrap();
    assert!(
        !allowed(&t.decide("root", "CreateTable", &cols("lake", "s", "t", &[]))),
        "the disabled admin lost all-access"
    );
    assert!(
        allowed(&t.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["a"]))),
        "data statements kept"
    );
}

// ---- Beyond the brief's list: tests that pin rules the brief's tests leave open.

#[test]
fn a_read_grant_does_not_allow_an_admin_operation() {
    let s = snap(
        vec![st(
            1,
            who(&["ann"], &[]),
            path("lake", None, None, None),
            Effect::Allow {
                privileges: vec![Privilege::Read, Privilege::Write],
            },
        )],
        vec![],
    );
    let d = s.decide("ann", "CreateTable", &cols("lake", "s", "t", &[]));
    assert_eq!(
        d,
        Decision::Deny {
            reason: "CreateTable needs the all-access engine administration grant".into(),
            explicit: false
        }
    );
    assert!(!allowed(&s.decide(
        "ann",
        "DropSchema",
        &Resource::Schema {
            catalog: "lake".into(),
            schema: "s".into()
        }
    )));
}

#[test]
fn a_wildcard_catalog_statement_is_found_and_question_mark_is_one_character() {
    let s = snap(
        vec![
            st(
                1,
                who(&["ann"], &[]),
                path("la?e", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("L*", Some("s"), Some("t"), Some("ssn")),
                Effect::Deny {
                    privileges: vec![Privilege::Read],
                },
            ),
        ],
        vec![],
    );
    assert!(allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["id"])
    )));
    assert!(
        !allowed(&s.decide(
            "ann",
            "SelectFromColumns",
            &cols("lake", "s", "t", &["SSN"])
        )),
        "the deny's pattern matches case-insensitively"
    );
    assert!(
        !allowed(&s.decide(
            "ann",
            "SelectFromColumns",
            &cols("lakee", "s", "t", &["id"])
        )),
        "? is exactly one character"
    );
    assert!(
        !allowed(&s.decide("ann", "SelectFromColumns", &cols("lae", "s", "t", &["id"]))),
        "? is exactly one character"
    );
}

#[test]
fn a_session_user_statement_matches_only_the_session_user() {
    let s = snap(
        vec![infra(
            InfraKind::SelfImpersonate,
            who(&["*"], &[]),
            InfraObject::SessionUser,
            "impersonate",
        )],
        vec![],
    );
    assert!(
        allowed(&s.decide("Ann", "ImpersonateUser", &Resource::User("ann".into()))),
        "compared case-insensitively"
    );
    assert!(!allowed(&s.decide(
        "ann",
        "ImpersonateUser",
        &Resource::User("bob".into())
    )));
    assert!(
        !allowed(&s.decide("ann", "ImpersonateUser", &Resource::None)),
        "malformed"
    );
}

#[test]
fn a_withhold_spares_its_except_members_only() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            Statement {
                source: Source::Withhold {
                    classification_id: Uuid::nil(),
                    classification_name: "pii".into(),
                },
                audience: who(&["*"], &["cleared"]),
                target: Target::Path(Path {
                    catalog_id: None,
                    catalog: "lake".into(),
                    schema: Level::Named("s".into()),
                    table: Level::Named("t".into()),
                    column: Level::Named("ssn".into()),
                }),
                effect: Effect::Withhold {
                    privileges: vec![Privilege::Read],
                },
                derivation: Derivation::ExpandedFromClassification,
            },
        ],
        vec![],
    );
    let d = s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["ssn"]),
    );
    assert!(matches!(d, Decision::Deny { explicit: true, .. }), "{d:?}");
    assert!(allowed(&s.decide(
        "cleared",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["ssn"])
    )));
}

#[test]
fn a_metadata_table_is_unreachable_to_someone_restricted_on_its_base_table() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), None),
                Effect::RowFilter {
                    expression: "region = 'us'".into(),
                },
            ),
            st(
                3,
                who(&["bob"], &[]),
                path("lake", Some("s"), Some("t"), Some("email")),
                Effect::Mask {
                    mask_type: "MASK_NONE".into(),
                    expression: None,
                },
            ),
            st(
                4,
                who(&["cat"], &[]),
                path("lake", Some("s"), Some("t"), None),
                Effect::RowFilter {
                    expression: "  ".into(),
                },
            ),
        ],
        vec![],
    );
    let meta = cols("lake", "s", "t$partitions", &[]);
    assert!(
        !allowed(&s.decide(
            "ann",
            "SelectFromColumns",
            &cols("lake", "s", "t$partitions", &["x"])
        )),
        "a row filter restricts"
    );
    assert!(
        !allowed(&s.decide("ann", "ShowColumns", &meta)),
        "and hides the metadata table from reach"
    );
    assert!(
        !allowed(&s.decide("ann", "SelectFromColumns", &meta)),
        "count(*) on the metadata table too"
    );
    assert!(
        allowed(&s.decide(
            "bob",
            "SelectFromColumns",
            &cols("lake", "s", "t$files", &["x"])
        )),
        "MASK_NONE does not restrict"
    );
    assert!(
        allowed(&s.decide(
            "cat",
            "SelectFromColumns",
            &cols("lake", "s", "t$files", &["x"])
        )),
        "an empty filter does not restrict"
    );
    assert!(
        allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["x"]))),
        "the base table itself is the value routes' business"
    );
}

#[test]
fn a_metadata_table_under_a_classification_withhold_on_a_base_column() {
    let s = snap(
        vec![
            st(
                1,
                who(&["*"], &[]),
                path("lake", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            Statement {
                source: Source::Withhold {
                    classification_id: Uuid::nil(),
                    classification_name: "pii".into(),
                },
                audience: who(&["*"], &["cleared"]),
                target: Target::Classification { name: "PII".into() },
                effect: Effect::WithholdMask,
                derivation: Derivation::AsDeclared,
            },
        ],
        vec![ClassifiedColumn {
            catalog: "Lake".into(),
            schema: "s".into(),
            table: "T".into(),
            column: Some("ssn".into()),
            classification: "pii".into(),
        }],
    );
    assert!(!allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t$files", &["x"])
    )));
    assert!(allowed(&s.decide(
        "cleared",
        "SelectFromColumns",
        &cols("lake", "s", "t$files", &["x"])
    )));
    assert!(
        allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["id"]))),
        "a withhold mask is not a deny"
    );
}

#[test]
fn a_metadata_table_name_is_matched_as_written() {
    let s = snap(
        vec![st(
            1,
            who(&["ann"], &[]),
            path("lake", Some("s"), Some("t"), None),
            Effect::Allow {
                privileges: vec![Privilege::Read],
            },
        )],
        vec![],
    );
    assert!(
        !allowed(&s.decide(
            "ann",
            "SelectFromColumns",
            &cols("lake", "s", "t$files", &["x"])
        )),
        "a grant on t is not a grant on t$files"
    );
}

#[test]
fn an_engine_object_reaches_its_own_catalog_and_schema_only() {
    let s = snap(
        vec![Statement {
            source: Source::EnginePrivilege {
                engine_grant_id: Uuid::nil(),
            },
            audience: who(&["ann"], &[]),
            target: Target::EngineObject {
                capability: "trino.execute_procedure".into(),
                values: vec!["Lake".into(), "system".into(), "rollback".into()],
            },
            effect: Effect::EnginePrivilege {
                capability: "trino.execute_procedure".into(),
            },
            derivation: Derivation::AsDeclared,
        }],
        vec![],
    );
    let schema = |c: &str, x: &str| Resource::Schema {
        catalog: c.into(),
        schema: x.into(),
    };
    assert!(allowed(&s.decide(
        "ann",
        "AccessCatalog",
        &Resource::Catalog("lake".into())
    )));
    assert!(allowed(&s.decide(
        "ann",
        "ShowFunctions",
        &schema("lake", "system")
    )));
    assert!(
        !allowed(&s.decide("ann", "ShowFunctions", &schema("lake", "other"))),
        "the procedure is not below lake.other"
    );
    assert!(
        !allowed(&s.decide("ann", "ShowColumns", &cols("lake", "system", "t", &[]))),
        "nor below a table"
    );
    assert!(!allowed(&s.decide(
        "ann",
        "AccessCatalog",
        &Resource::Catalog("hive".into())
    )));
}

#[test]
fn write_checks_the_table_and_a_column_deny_does_not_reach_it() {
    let s = snap(
        vec![
            st(
                1,
                who(&["ann"], &[]),
                path("lake", Some("s"), None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Write],
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), Some("c")),
                Effect::Deny {
                    privileges: vec![Privilege::Write],
                },
            ),
            st(
                3,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("u"), Some("*")),
                Effect::Deny {
                    privileges: vec![Privilege::Write],
                },
            ),
        ],
        vec![],
    );
    assert!(allowed(&s.decide(
        "ann",
        "InsertIntoTable",
        &cols("lake", "s", "t", &[])
    )));
    assert!(
        allowed(&s.decide("ann", "DeleteFromTable", &cols("lake", "s", "t", &["c"]))),
        "Write ignores the column list"
    );
    assert!(
        !allowed(&s.decide("ann", "InsertIntoTable", &cols("lake", "s", "u", &[]))),
        "column `*` covers the whole level"
    );
    assert!(
        !allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["x"]))),
        "Write is not Read"
    );
}

#[test]
fn decide_privilege_matches_absent_levels_only_against_whole_levels() {
    use egret_perm_eval::trino_ops::DataPrivilege;
    use egret_perm_eval::ObjectPath;
    let s = snap(
        vec![
            st(
                1,
                who(&["ann"], &[]),
                path("lake", Some("s"), None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("*"), Some("ssn")),
                Effect::Deny {
                    privileges: vec![Privilege::Read],
                },
            ),
        ],
        vec![],
    );
    let o = |s: Option<&str>, t: Option<&str>, c: Option<&str>| ObjectPath {
        catalog: "lake".into(),
        schema: s.map(Into::into),
        table: t.map(Into::into),
        column: c.map(Into::into),
    };
    assert!(allowed(&s.decide_privilege(
        "ann",
        DataPrivilege::Read,
        &o(Some("s"), None, None)
    )));
    assert!(
        allowed(&s.decide_privilege("ann", DataPrivilege::Read, &o(Some("s"), Some("t"), None))),
        "the column deny is below"
    );
    assert!(!allowed(&s.decide_privilege(
        "ann",
        DataPrivilege::Read,
        &o(Some("s"), Some("t"), Some("ssn"))
    )));
    assert!(
        !allowed(&s.decide_privilege("ann", DataPrivilege::Read, &o(None, None, None))),
        "a schema grant does not cover the catalog"
    );
    assert!(!allowed(&s.decide_privilege(
        "ann",
        DataPrivilege::Write,
        &o(Some("s"), None, None)
    )));
}

#[test]
fn value_questions_and_the_snapshot_accessors() {
    let s = Snapshot::build(SnapshotInput {
        service_id: 7,
        key: (3, 4),
        classified: vec![],
        lineage_principal: None,
        statements: vec![infra(
            InfraKind::AdminAllAccess,
            who(&["root"], &[]),
            InfraObject::WholeEp,
            "all",
        )],
    })
    .unwrap();
    assert_eq!(
        (s.service_id(), s.key(), s.len(), s.is_empty()),
        (7, (3, 4), 1, false)
    );
    assert_eq!(
        s.decide("root", "GetRowFilters", &cols("lake", "s", "t", &[])),
        Decision::Deny {
            reason: "GetRowFilters is asked on a value route".into(),
            explicit: false
        }
    );
    assert_eq!(
        s.filter(
            "root",
            "FilterCatalogs",
            &[Resource::Catalog("a".into()), Resource::Catalog("b".into())]
        ),
        vec![0, 1]
    );
}

#[test]
fn an_unexpanded_except_member_fails_the_build_with_its_index() {
    let p = egret_perm::Principal {
        kind: egret_perm::PrincipalKind::Group,
        r#ref: "eng".into(),
    };
    let mut a = who(&["*"], &[]);
    a.except.push(Member {
        name: MemberName::Unexpanded(p),
        via: Via::Direct,
    });
    let ok = st(
        1,
        who(&["ann"], &[]),
        path("lake", None, None, None),
        Effect::Allow {
            privileges: vec![Privilege::Read],
        },
    );
    let bad = st(
        2,
        a,
        path("lake", None, None, None),
        Effect::Deny {
            privileges: vec![Privilege::Read],
        },
    );
    let e = Snapshot::build(SnapshotInput {
        service_id: 1,
        key: (1, 1),
        classified: vec![],
        lineage_principal: None,
        statements: vec![ok, bad],
    });
    assert_eq!(
        e.err(),
        Some(egret_perm_eval::SnapshotError::UnexpandedMember {
            index: 1,
            principal: "group:eng".into()
        })
    );
}

// ---- Fix round 1 (review I1, I2, M1, M3).

/// I1: Ranger treats every pure run of `*` as match-any, so a deny on `t / **`
/// covers a table-level request (`SelfAndAllDescendants`).
#[test]
fn a_wildcard_run_below_the_request_covers_it() {
    let allow = st(
        1,
        who(&["ann"], &[]),
        path("lake", None, None, None),
        Effect::Allow {
            privileges: vec![Privilege::Read, Privilege::Write],
        },
    );
    let read = snap(
        vec![
            allow.clone(),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), Some("**")),
                Effect::Deny {
                    privileges: vec![Privilege::Read],
                },
            ),
        ],
        vec![],
    );
    assert!(
        !allowed(&read.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &[]))),
        "count(*) under a `**` column deny"
    );
    let write = snap(
        vec![
            allow.clone(),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), Some("**")),
                Effect::Deny {
                    privileges: vec![Privilege::Write],
                },
            ),
        ],
        vec![],
    );
    for op in [
        "InsertIntoTable",
        "DeleteFromTable",
        "TruncateTable",
        "UpdateTableColumns",
    ] {
        assert!(
            !allowed(&write.decide("ann", op, &cols("lake", "s", "t", &[]))),
            "{op} under a `**` column deny"
        );
    }
    let catalog = snap(
        vec![
            allow,
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("**"), None, None),
                Effect::Deny {
                    privileges: vec![Privilege::Read],
                },
            ),
        ],
        vec![],
    );
    assert!(
        !allowed(&catalog.decide("ann", "AccessCatalog", &Resource::Catalog("lake".into()))),
        "reach under a `lake.**` deny"
    );
}

#[test]
fn decide_privilege_treats_a_wildcard_run_as_the_whole_level() {
    use egret_perm_eval::trino_ops::DataPrivilege;
    use egret_perm_eval::ObjectPath;
    let s = snap(
        vec![
            st(
                1,
                who(&["ann"], &[]),
                path("lake", None, None, None),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("***"), None),
                Effect::Deny {
                    privileges: vec![Privilege::Read],
                },
            ),
        ],
        vec![],
    );
    let o = ObjectPath {
        catalog: "lake".into(),
        schema: Some("s".into()),
        table: None,
        column: None,
    };
    assert!(!allowed(&s.decide_privilege(
        "ann",
        DataPrivilege::Read,
        &o
    )));
}

/// I2 (a): Ranger's all-access bodies are ordinary allow policies; a deny beats them.
#[test]
fn a_deny_beats_the_all_access_grant() {
    let s = snap(
        vec![
            infra(
                InfraKind::AdminAllAccess,
                who(&["root"], &[]),
                InfraObject::WholeEp,
                "all",
            ),
            st(
                1,
                who(&["root"], &[]),
                path("lake", Some("s"), Some("t"), Some("ssn")),
                Effect::Deny {
                    privileges: vec![Privilege::Read],
                },
            ),
            st(
                2,
                who(&["root"], &[]),
                path("lake", Some("hr"), None, None),
                Effect::Deny {
                    privileges: vec![Privilege::Write],
                },
            ),
        ],
        vec![],
    );
    assert!(allowed(&s.decide(
        "root",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["id"])
    )));
    let d = s.decide(
        "root",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["ssn"]),
    );
    assert!(matches!(d, Decision::Deny { explicit: true, .. }), "{d:?}");
    assert!(!allowed(&s.decide(
        "root",
        "ShowTables",
        &Resource::Schema {
            catalog: "lake".into(),
            schema: "hr".into()
        }
    )));
}

/// I2 (b): an engine object answers its grantee only.
#[test]
fn engine_objects_answer_only_their_grantee() {
    let s = snap(
        vec![Statement {
            source: Source::EnginePrivilege {
                engine_grant_id: Uuid::nil(),
            },
            audience: who(&["ann"], &[]),
            target: Target::EngineObject {
                capability: "trino.execute_procedure".into(),
                values: vec!["lake".into(), "system".into(), "rollback".into()],
            },
            effect: Effect::EnginePrivilege {
                capability: "trino.execute_procedure".into(),
            },
            derivation: Derivation::AsDeclared,
        }],
        vec![],
    );
    let f = Resource::Function {
        catalog: "lake".into(),
        schema: "system".into(),
        function: "rollback".into(),
    };
    assert!(allowed(&s.decide("ann", "ExecuteProcedure", &f)));
    assert!(
        !allowed(&s.decide("bob", "ExecuteProcedure", &f)),
        "bob is not the grantee"
    );
    assert!(allowed(&s.decide(
        "ann",
        "AccessCatalog",
        &Resource::Catalog("lake".into())
    )));
    assert!(
        !allowed(&s.decide("bob", "AccessCatalog", &Resource::Catalog("lake".into()))),
        "nor does it give bob reach"
    );
}

/// I2 (c)
#[test]
fn execute_query_needs_an_any_query_statement() {
    let s = snap(
        vec![infra(
            InfraKind::ExecuteQuery,
            who(&["ann"], &[]),
            InfraObject::AnyQuery,
            "execute",
        )],
        vec![],
    );
    assert!(allowed(&s.decide("ann", "ExecuteQuery", &Resource::None)));
    assert!(!allowed(&s.decide("bob", "ExecuteQuery", &Resource::None)));
    assert!(!allowed(&snap(vec![], vec![]).decide(
        "ann",
        "ExecuteQuery",
        &Resource::None
    )));
}

/// I2 (d): values are compared whole, never as a prefix.
#[test]
fn an_engine_object_with_fewer_values_is_not_a_prefix_match() {
    let s = snap(
        vec![Statement {
            source: Source::EnginePrivilege {
                engine_grant_id: Uuid::nil(),
            },
            audience: who(&["ann"], &[]),
            target: Target::EngineObject {
                capability: "trino.execute_procedure".into(),
                values: vec!["lake".into()],
            },
            effect: Effect::EnginePrivilege {
                capability: "trino.execute_procedure".into(),
            },
            derivation: Derivation::AsDeclared,
        }],
        vec![],
    );
    let f = Resource::Function {
        catalog: "lake".into(),
        schema: "system".into(),
        function: "rollback".into(),
    };
    assert!(!allowed(&s.decide("ann", "ExecuteProcedure", &f)));
}

fn build_err(p: Target) -> bool {
    Snapshot::build(SnapshotInput {
        service_id: 1,
        key: (1, 1),
        classified: vec![],
        lineage_principal: None,
        statements: vec![st(
            1,
            who(&["ann"], &[]),
            p,
            Effect::Deny {
                privileges: vec![Privilege::Read],
            },
        )],
    })
    .is_err()
}

/// M1: Ranger renders these shapes wider than they read; the snapshot refuses them.
#[test]
fn an_empty_named_level_fails_the_build() {
    assert!(build_err(path("lake", Some(""), None, None)));
    assert!(build_err(path("lake", Some("s"), Some("t"), Some(""))));
    assert!(build_err(path("", None, None, None)), "an empty catalog");
    assert!(!build_err(path("lake", Some("s"), Some("t"), Some("c"))));
}

#[test]
fn a_named_level_below_an_unnamed_one_fails_the_build() {
    assert!(build_err(path("lake", None, Some("t"), None)));
    assert!(build_err(path("lake", Some("s"), None, Some("c"))));
    assert!(build_err(path("lake", None, None, Some("c"))));
    assert!(!build_err(path("lake", Some("s"), None, None)));
}

/// M3: a request value that is literally `*` or empty matches only a match-any level.
#[test]
fn a_star_or_empty_request_value_matches_only_match_any() {
    let s = snap(
        vec![
            st(
                1,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("t"), Some("?")),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            st(
                2,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("u"), Some("**")),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
            st(
                3,
                who(&["ann"], &[]),
                path("lake", Some("s"), Some("v"), Some("x*")),
                Effect::Allow {
                    privileges: vec![Privilege::Read],
                },
            ),
        ],
        vec![],
    );
    assert!(allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "t", &["a"])
    )));
    assert!(
        !allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "t", &["*"]))),
        "`?` does not answer a column named `*`"
    );
    assert!(
        !allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "v", &[""]))),
        "`x*` does not answer an empty name"
    );
    assert!(
        allowed(&s.decide("ann", "SelectFromColumns", &cols("lake", "s", "u", &["*"]))),
        "a run of `*` is match-any"
    );
    assert!(allowed(&s.decide(
        "ann",
        "SelectFromColumns",
        &cols("lake", "s", "u", &[""])
    )));
}

/// Task 18 (opa_differential, seed 0x5b2b00000001 state 13 question 2): on a
/// JWT engine Ranger denied `egret-lineage` `TruncateTable` through the
/// lineage transform's override deny, while the evaluator allowed it by a
/// `public` write grant. The reserved lineage principal is now refused every
/// access `egret_perm_ir::lineage` does not keep, whatever the grants say;
/// what it keeps (select, use, its own impersonation, `query_max_run_time`,
/// `execute` on a query) follows the grants as for anyone.
#[test]
fn the_lineage_principal_is_refused_what_the_lineage_transform_denies() {
    const LP: &str = "egret-lineage";
    let everyone = || who(&["*"], &[]);
    let statements = vec![
        st(
            1,
            everyone(),
            path("lake", None, None, None),
            Effect::Allow {
                privileges: vec![Privilege::Read, Privilege::Write],
            },
        ),
        st(
            2,
            everyone(),
            path("w", None, None, None),
            Effect::Allow {
                privileges: vec![Privilege::Write],
            },
        ),
        infra(
            InfraKind::AdminAllAccess,
            everyone(),
            InfraObject::WholeEp,
            "all",
        ),
        infra(
            InfraKind::SelfImpersonate,
            everyone(),
            InfraObject::SessionUser,
            "impersonate",
        ),
        infra(
            InfraKind::ExecuteQuery,
            everyone(),
            InfraObject::AnyQuery,
            "execute",
        ),
        infra(
            InfraKind::QueryTimeout,
            everyone(),
            InfraObject::SystemProperty("query_max_run_time".into()),
            "alter",
        ),
        infra(
            InfraKind::DelegatorImpersonate,
            everyone(),
            InfraObject::AnyUser,
            "impersonate",
        ),
    ];
    let build = |statements: Vec<Statement>, lp: Option<&str>| {
        Snapshot::build(SnapshotInput {
            service_id: 1,
            key: (1, 1),
            statements,
            classified: vec![],
            lineage_principal: lp.map(str::to_string),
        })
        .unwrap()
    };
    let with = |lp: Option<&str>| build(statements.clone(), lp);
    let s = with(Some(LP));
    let table = cols("lake", "s", "t", &[]);
    let user = |u: &str| Resource::User(u.into());
    let prop = |p: &str| Resource::SystemProperty(p.into());
    let f = Resource::Function {
        catalog: "lake".into(),
        schema: "s".into(),
        function: "f".into(),
    };
    let refused = |op: &str, r: &Resource| {
        let d = s.decide(LP, op, r);
        assert!(
            matches!(&d, Decision::Deny { explicit: true, reason } if reason.contains("lineage")),
            "{op}: {d:?}"
        );
    };
    // Kept: reads, reach through a Read grant, its own impersonation, the
    // query timeout property, running a query.
    assert!(allowed(&s.decide(
        LP,
        "SelectFromColumns",
        &cols("lake", "s", "t", &["a"])
    )));
    assert!(allowed(&s.decide(LP, "SelectFromColumns", &table)));
    assert!(allowed(&s.decide(
        LP,
        "ShowSchemas",
        &Resource::Catalog("lake".into())
    )));
    assert!(allowed(&s.decide(LP, "ImpersonateUser", &user(LP))));
    assert!(allowed(&s.decide(
        LP,
        "SetSystemSessionProperty",
        &prop("query_max_run_time")
    )));
    assert!(allowed(&s.decide(LP, "ExecuteQuery", &Resource::None)));
    // Refused whatever the grants say: write, admin, others' queries and
    // impersonation, other properties, every engine capability.
    for op in [
        "InsertIntoTable",
        "TruncateTable",
        "DeleteFromTable",
        "CreateTable",
        "DropTable",
    ] {
        refused(op, &table);
    }
    refused("ImpersonateUser", &user("ann"));
    refused("KillQueryOwnedBy", &user("ann"));
    refused("SetSystemSessionProperty", &prop("query_max_memory"));
    refused("ReadSystemInformation", &Resource::None);
    for op in ["ExecuteFunction", "FilterFunctions", "ExecuteProcedure"] {
        refused(op, &f);
    }
    // A Write-only grant does not reach for the principal (it keeps only
    // select and use), while it does for anyone else.
    let no_admin = build(
        statements
            .iter()
            .filter(|s| s.target != Target::Infra(InfraObject::WholeEp))
            .cloned()
            .collect(),
        Some(LP),
    );
    let w = Resource::Catalog("w".into());
    assert!(!allowed(&no_admin.decide(LP, "ShowSchemas", &w)));
    assert!(allowed(&no_admin.decide("ann", "ShowSchemas", &w)));
    assert!(allowed(&no_admin.decide(
        LP,
        "ShowSchemas",
        &Resource::Catalog("lake".into())
    )));
    assert!(allowed(&s.decide("ann", "TruncateTable", &table)));
    // With no reserved principal the name is an ordinary user.
    assert!(allowed(&with(None).decide(LP, "TruncateTable", &table)));
    assert_eq!(s.lineage_principal(), Some(LP));
}
