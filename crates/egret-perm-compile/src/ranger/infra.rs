//! Renders an infrastructure statement into the Ranger policy bodies it
//! stands for (governance sub-project 2a). The lowering decided which
//! statements exist; the bodies come from the one shared builders
//! (`egret_policy_api::infra_bodies`, `egret_policy::default_policies`) so
//! what this writes is what engine apply wrote before.

use egret_perm::{CompileError, EngineType};
use egret_perm_ir::audience::MemberName;
use egret_perm_ir::statement::{InfraKind, InfraObject, Source, Statement, Target};
use egret_policy_api::infra_bodies as bodies;
use serde_json::Value;

/// Render `s` for the Ranger service `service_name` on `engine`: the
/// `(name, body)` rows it stands for. `AdminAllAccess` is one row per
/// resource hierarchy of the engine's service-def, `QueryLogGuard` one row per
/// StarRocks hierarchy under `database`; every other kind is one row. A statement of another family is refused (only reachable by building
/// one by hand).
pub(crate) fn render(
    s: &Statement,
    service_name: &str,
    engine: EngineType,
) -> Result<Vec<(String, Value)>, CompileError> {
    let Source::Infra { kind } = s.source else {
        return Err(CompileError::Db(
            "ranger::infra::render was handed a statement of another family".into(),
        ));
    };
    let one = |name: &str, body: Value| Ok(vec![(name.to_string(), body)]);
    match kind {
        InfraKind::AdminAllAccess => {
            let users: Vec<String> = s
                .audience
                .include
                .iter()
                .filter_map(|m| match &m.name {
                    MemberName::User(u) => Some(u.clone()),
                    _ => None,
                })
                .collect();
            let service_type = match engine {
                EngineType::Trino => "trino",
                EngineType::StarRocks => "starrocks",
            };
            egret_policy::default_policies::admin_all_access_bodies(
                service_type,
                service_name,
                &users,
            )
            .map_err(|e| CompileError::Db(format!("admin all-access bodies: {e}")))
        }
        InfraKind::DelegatorImpersonate => match (&s.target, engine) {
            (Target::Infra(InfraObject::AnyUser), EngineType::Trino) => one(
                bodies::IMPERSONATE_POLICY_NAME,
                bodies::delegator_impersonate_policy(service_name),
            ),
            (Target::Infra(InfraObject::AnyUserExcept(excluded)), EngineType::StarRocks)
                if !excluded.is_empty() =>
            {
                one(
                    bodies::IMPERSONATE_POLICY_NAME,
                    bodies::starrocks_delegator_impersonate_policy(service_name, excluded),
                )
            }
            (target, engine) => Err(CompileError::Db(format!(
                "ranger::infra::render: no delegator impersonation body for {target:?} on \
                 {engine:?} (an empty exclusion would exclude nothing)"
            ))),
        },
        InfraKind::SelfImpersonate => one(
            bodies::SELF_IMPERSONATE_POLICY_NAME,
            bodies::self_impersonate_policy(service_name),
        ),
        InfraKind::ExecuteQuery => one(
            bodies::EXECUTE_QUERY_POLICY_NAME,
            bodies::execute_query_policy(service_name),
        ),
        InfraKind::QueryTimeout => one(
            bodies::QUERY_TIMEOUT_POLICY_NAME,
            bodies::query_timeout_policy(service_name),
        ),
        InfraKind::QueryLogLoader => one(
            bodies::QUERY_LOG_LOADER_POLICY_NAME,
            bodies::query_log_loader_policy(service_name),
        ),
        InfraKind::QueryLogGuard => {
            egret_policy::default_policies::query_log_guard_bodies(service_name)
                .map_err(crate::db_error::from_store)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm::EngineType;
    use egret_perm_ir::lower::infra::{lower_infra, InfraInputs};
    use egret_policy_api::infra_bodies as b;

    #[test]
    fn trino_renders_todays_bodies_byte_for_byte() {
        let s = lower_infra(&InfraInputs {
            engine: EngineType::Trino,
            admins: vec!["ann".into(), "bob".into()],
            delegating: true,
            delegator_impersonates: true,
            query_log: false,
        });
        let got: Vec<(String, serde_json::Value)> = s
            .iter()
            .flat_map(|s| render(s, "svc", EngineType::Trino).unwrap())
            .collect();
        let mut want = egret_policy::default_policies::admin_all_access_bodies(
            "trino",
            "svc",
            &["ann".into(), "bob".into()],
        )
        .unwrap();
        want.push((
            b::IMPERSONATE_POLICY_NAME.into(),
            b::delegator_impersonate_policy("svc"),
        ));
        want.extend(
            b::user_baseline_policies("svc")
                .into_iter()
                .map(|(n, v)| (n.to_string(), v)),
        );
        assert_eq!(
            serde_json::to_vec(&got).unwrap(),
            serde_json::to_vec(&want).unwrap()
        );
    }

    #[test]
    fn starrocks_renders_the_starrocks_def() {
        let s = lower_infra(&InfraInputs {
            engine: EngineType::StarRocks,
            admins: vec!["ann".into()],
            delegating: true,
            delegator_impersonates: false,
            query_log: false,
        });
        let got: Vec<_> = s
            .iter()
            .flat_map(|s| render(s, "sr", EngineType::StarRocks).unwrap())
            .collect();
        let want = egret_policy::default_policies::admin_all_access_bodies(
            "starrocks",
            "sr",
            &["ann".into()],
        )
        .unwrap();
        assert_eq!(got.len(), 13);
        assert_eq!(
            serde_json::to_vec(&got).unwrap(),
            serde_json::to_vec(&want).unwrap()
        );
    }

    #[test]
    fn the_timeout_property_is_one_value() {
        let body = b::query_timeout_policy("svc");
        assert_eq!(
            body["resources"]["systemproperty"]["values"][0],
            egret_perm_ir::lower::infra::QUERY_TIMEOUT_PROPERTY
        );
    }

    #[test]
    fn a_statement_of_another_family_is_refused() {
        let mut s = lower_infra(&InfraInputs {
            engine: EngineType::Trino,
            admins: vec![],
            delegating: true,
            delegator_impersonates: false,
            query_log: false,
        })
        .remove(0);
        s.source = egret_perm_ir::statement::Source::EnginePrivilege {
            engine_grant_id: uuid::Uuid::nil(),
        };
        assert!(render(&s, "svc", EngineType::Trino).is_err());
    }

    fn query_log_rows() -> Vec<(String, serde_json::Value)> {
        lower_infra(&InfraInputs {
            engine: EngineType::StarRocks,
            admins: vec![],
            delegating: false,
            delegator_impersonates: false,
            query_log: true,
        })
        .iter()
        .flat_map(|s| render(s, "sr", EngineType::StarRocks).unwrap())
        .collect()
    }

    #[test]
    fn the_loader_renders_one_literal_body() {
        let rows = query_log_rows();
        let loader: Vec<_> = rows
            .iter()
            .filter(|(n, _)| n == b::QUERY_LOG_LOADER_POLICY_NAME)
            .collect();
        assert_eq!(loader.len(), 1);
        assert_eq!(
            rows[0].0, "egret-query-log-loader",
            "the loader comes first"
        );
        assert_eq!(
            loader[0].1,
            serde_json::json!({
                "name": "egret-query-log-loader",
                "service": "sr",
                "serviceType": "starrocks",
                "policyType": 0,
                "isEnabled": true,
                "resources": {
                    "catalog": { "values": ["default_catalog"], "isExcludes": false, "isRecursive": false },
                    "database": { "values": ["starrocks_audit_db__"], "isExcludes": false, "isRecursive": false },
                    "table": { "values": ["starrocks_audit_tbl__"], "isExcludes": false, "isRecursive": false }
                },
                "policyItems": [{
                    "users": ["egret_query_log"],
                    "groups": [],
                    "roles": [],
                    "accesses": [{ "type": "insert", "isAllowed": true }],
                    "delegateAdmin": false
                }],
                "denyPolicyItems": [],
                "allowExceptions": [],
                "denyExceptions": [],
                "isDenyAllElse": false,
                "policyPriority": 0
            })
        );
    }

    #[test]
    fn the_guard_denies_public_on_every_hierarchy_under_database() {
        let rows = query_log_rows();
        let guard: Vec<_> = rows
            .iter()
            .filter(|(n, _)| n.starts_with("egret-query-log-guard - "))
            .collect();
        let names: Vec<&str> = guard.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "egret-query-log-guard - catalog, database",
                "egret-query-log-guard - catalog, database, table",
                "egret-query-log-guard - catalog, database, table, column",
                "egret-query-log-guard - catalog, database, view",
                "egret-query-log-guard - catalog, database, materialized_view",
                "egret-query-log-guard - catalog, database, function",
            ]
        );
        assert_eq!(rows.len(), 1 + guard.len(), "the loader and the guard only");
        let def = egret_policy::service_def::starrocks_service_def().unwrap();
        for (name, body) in &guard {
            assert_eq!(body["name"], name.as_str());
            assert_eq!(body["serviceType"], "starrocks", "{name}");
            let res = body["resources"].as_object().unwrap();
            assert_eq!(
                res["catalog"]["values"],
                serde_json::json!(["default_catalog"]),
                "{name}"
            );
            assert_eq!(
                res["database"]["values"],
                serde_json::json!(["starrocks_audit_db__"]),
                "{name}"
            );
            for (k, v) in res {
                if k != "catalog" && k != "database" {
                    assert_eq!(v["values"], serde_json::json!(["*"]), "{name}: {k}");
                }
            }
            assert_eq!(
                body["policyItems"],
                serde_json::json!([]),
                "{name}: no allow"
            );
            let deny = body["denyPolicyItems"].as_array().unwrap();
            assert_eq!(deny.len(), 1, "{name}");
            assert_eq!(deny[0]["groups"], serde_json::json!(["public"]), "{name}");
            assert_eq!(deny[0]["users"], serde_json::json!([]), "{name}");
            // Every access type the def allows on the leaf.
            let leaf = name
                .rsplit(", ")
                .next()
                .unwrap()
                .trim_start_matches("egret-query-log-guard - ");
            let want: Vec<serde_json::Value> = def["resources"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["name"] == leaf)
                .unwrap()["accessTypeRestrictions"]
                .as_array()
                .unwrap()
                .clone();
            let got: Vec<serde_json::Value> = deny[0]["accesses"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["type"].clone())
                .collect();
            assert_eq!(got, want, "{name}: deny accesses");
            if leaf == "table" {
                // A literal, not read from the def: the builder reads the def
                // too, so only this pins what the table row actually denies.
                assert_eq!(
                    got,
                    ["delete", "drop", "insert", "alter", "export", "update", "refresh"]
                        .map(serde_json::Value::from)
                        .to_vec(),
                    "the table row denies every table-level access type"
                );
            }
            if leaf == "column" {
                assert_eq!(
                    got,
                    vec![serde_json::Value::from("select")],
                    "column: select"
                );
            }
            let exceptions = &body["denyExceptions"];
            if leaf == "table" {
                assert_eq!(
                    exceptions,
                    &serde_json::json!([{
                        "users": ["egret_query_log"],
                        "groups": [],
                        "roles": [],
                        "accesses": [{ "type": "insert", "isAllowed": true }],
                        "delegateAdmin": false
                    }]),
                    "the table row excepts the loader's insert"
                );
            } else {
                assert_eq!(exceptions, &serde_json::json!([]), "{name}: no exception");
            }
        }
    }

    fn sr_delegator(delegating: bool, imp: bool) -> Vec<Statement> {
        lower_infra(&InfraInputs {
            engine: EngineType::StarRocks,
            admins: vec![],
            delegating,
            delegator_impersonates: imp,
            query_log: false,
        })
    }

    /// Package D, D1b.
    #[test]
    fn starrocks_renders_the_delegator_row_on_the_user_resource() {
        let rows: Vec<_> = sr_delegator(true, true)
            .iter()
            .flat_map(|s| render(s, "sr", EngineType::StarRocks).unwrap())
            .collect();
        let excluded: Vec<String> = egret_core::engine_principals::STARROCKS_RESERVED_USERS
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            rows,
            vec![(
                b::IMPERSONATE_POLICY_NAME.to_string(),
                b::starrocks_delegator_impersonate_policy("sr", &excluded)
            )]
        );
    }

    #[test]
    fn a_delegator_row_whose_object_does_not_fit_the_engine_is_refused() {
        let trino = lower_infra(&InfraInputs {
            engine: EngineType::Trino,
            admins: vec![],
            delegating: true,
            delegator_impersonates: true,
            query_log: false,
        });
        let delegator = trino
            .iter()
            .find(|s| {
                matches!(
                    s.source,
                    Source::Infra {
                        kind: InfraKind::DelegatorImpersonate
                    }
                )
            })
            .unwrap();
        assert!(render(delegator, "sr", EngineType::StarRocks).is_err());
        let sr = sr_delegator(true, true).remove(0);
        assert!(render(&sr, "svc", EngineType::Trino).is_err());
    }

    #[test]
    fn an_empty_exclusion_list_is_refused() {
        let mut s = sr_delegator(true, true).remove(0);
        s.target = Target::Infra(InfraObject::AnyUserExcept(vec![]));
        assert!(render(&s, "sr", EngineType::StarRocks).is_err());
    }
}
