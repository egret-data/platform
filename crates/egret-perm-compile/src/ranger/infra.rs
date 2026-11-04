//! Renders an infrastructure statement into the Ranger policy bodies it
//! stands for (governance sub-project 2a). The lowering decided which
//! statements exist; the bodies come from the one shared builders
//! (`egret_policy_api::infra_bodies`, `egret_policy::default_policies`) so
//! what this writes is what engine apply wrote before.

use egret_perm::{CompileError, EngineType};
use egret_perm_ir::audience::MemberName;
use egret_perm_ir::statement::{InfraKind, Source, Statement};
use egret_policy_api::infra_bodies as bodies;
use serde_json::Value;

/// Render `s` for the Ranger service `service_name` on `engine`: the
/// `(name, body)` rows it stands for. `AdminAllAccess` is one row per
/// resource hierarchy of the engine's service-def; every other kind is one
/// row. A statement of another family is refused (only reachable by building
/// one by hand).
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first caller is crate::infra, added in Task 4")
)]
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
        InfraKind::DelegatorImpersonate => one(
            bodies::IMPERSONATE_POLICY_NAME,
            bodies::delegator_impersonate_policy(service_name),
        ),
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
        })
        .remove(0);
        s.source = egret_perm_ir::statement::Source::EnginePrivilege {
            engine_grant_id: uuid::Uuid::nil(),
        };
        assert!(render(&s, "svc", EngineType::Trino).is_err());
    }
}
