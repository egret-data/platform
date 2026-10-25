//! Engine-level assertions for the username-takeover tests: the profile's
//! download, read back through `egret-ranger-eval` exactly as `explain` does
//! (`crate::routes::artifact_via`), and one access decision asked of it as a
//! named user. A JSON substring check would pass against a body that names the
//! user in an item that does not decide the access.

// Each test binary that includes this uses a different subset of it.
#![allow(dead_code)]

use std::collections::BTreeSet;

use egret_metastore::Db;
use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, Decision, EngineConfig,
    EngineType, MaskDecision, PluginVersion, RowFilterDecision,
};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::resource::RequestResource;
use egret_ranger_eval::servicedef::ServiceDef;

/// The download `service_id`'s plugin would receive: its service-def and its
/// enabled policies, as `egret_policy::enabled_policies_for_service_via`
/// assembles them for the download route.
pub async fn download(db: &Db, service_id: i64) -> (ServicePolicySet, ServiceDef) {
    let mut conn = db.pool.acquire().await.expect("acquire");
    let svc = egret_policy::get_service_by_id_via(&mut conn, service_id)
        .await
        .expect("service");
    let def = egret_policy::get_service_def_via(&mut conn, &svc.r#type)
        .await
        .expect("service def")
        .def_jsonb;
    let policies = egret_policy::enabled_policies_for_service_via(&mut conn, service_id)
        .await
        .expect("policies");
    let set: ServicePolicySet = serde_json::from_value(serde_json::json!({
        "serviceName": svc.name,
        "serviceId": svc.id,
        "policyVersion": svc.policy_version,
        "serviceDef": def,
        "policies": policies,
    }))
    .expect("the download reads back");
    (set, ServiceDef::parse(&def))
}

/// The decision a StarRocks plugin (egret's default image, Ranger 2.8.0)
/// makes for `user` asking `access` on `resource`, with no groups and no
/// roles: no engine egret provisions resolves either.
pub async fn starrocks_decides(
    db: &Db,
    service_id: i64,
    user: &str,
    access: &str,
    resource: &[(&str, &str)],
) -> Decision {
    decides(
        db,
        service_id,
        user,
        access,
        resource,
        EngineType::StarRocks,
        PluginVersion::Ranger2_8_0,
    )
    .await
}

/// [`starrocks_decides`] for a Trino plugin (Ranger 2.8.0, the one modelled).
pub async fn trino_decides(
    db: &Db,
    service_id: i64,
    user: &str,
    access: &str,
    resource: &[(&str, &str)],
) -> Decision {
    decides(
        db,
        service_id,
        user,
        access,
        resource,
        EngineType::Trino,
        PluginVersion::Ranger2_8_0,
    )
    .await
}

async fn decides(
    db: &Db,
    service_id: i64,
    user: &str,
    access: &str,
    resource: &[(&str, &str)],
    engine: EngineType,
    plugin_version: PluginVersion,
) -> Decision {
    let (set, def) = download(db, service_id).await;
    let (request, config) = request_for(user, access, resource, engine, plugin_version);
    evaluate(&set, &request, &def, &config)
}

/// The data mask a StarRocks plugin (Ranger 2.8.0) applies for `user`
/// selecting the column `resource` names.
pub async fn starrocks_mask(
    db: &Db,
    service_id: i64,
    user: &str,
    resource: &[(&str, &str)],
) -> MaskDecision {
    let (set, def) = download(db, service_id).await;
    let (request, config) = request_for(
        user,
        "select",
        resource,
        EngineType::StarRocks,
        PluginVersion::Ranger2_8_0,
    );
    evaluate_data_mask(&set, &request, &def, &config)
}

/// The row filter a StarRocks plugin (Ranger 2.8.0) applies for `user`
/// selecting from the table `resource` names.
pub async fn starrocks_row_filter(
    db: &Db,
    service_id: i64,
    user: &str,
    resource: &[(&str, &str)],
) -> RowFilterDecision {
    let (set, def) = download(db, service_id).await;
    let (request, config) = request_for(
        user,
        "select",
        resource,
        EngineType::StarRocks,
        PluginVersion::Ranger2_8_0,
    );
    evaluate_row_filter(&set, &request, &def, &config)
}

/// The mask type `d` applies, `None` when no mask decided.
pub fn mask_type(d: &MaskDecision) -> Option<String> {
    match d {
        MaskDecision::Masked(m) => m.mask_type.clone(),
        MaskDecision::NoMask => None,
        MaskDecision::Undecidable { why } => panic!("undecidable mask: {why:?}"),
    }
}

fn request_for(
    user: &str,
    access: &str,
    resource: &[(&str, &str)],
    engine: EngineType,
    plugin_version: PluginVersion,
) -> (AccessRequest, EngineConfig) {
    let mut res = RequestResource::new();
    for (k, v) in resource {
        res.insert(*k, *v);
    }
    let request = AccessRequest {
        user: user.to_string(),
        groups: BTreeSet::new(),
        roles: BTreeSet::new(),
        access_type: access.to_string(),
        resource: res,
        owner: None,
        resolved_tags: None,
    };
    let config = EngineConfig {
        engine: Some(engine),
        plugin_version: Some(plugin_version),
        ..EngineConfig::without_super_users()
    };
    (request, config)
}

pub fn allowed(d: &Decision) -> bool {
    matches!(d, Decision::Allowed { .. })
}

pub fn denied(d: &Decision) -> bool {
    matches!(d, Decision::Denied { .. })
}
