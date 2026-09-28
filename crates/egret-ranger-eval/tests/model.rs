//! Task 3: a typed reader for Ranger policy bodies. These tests assert two
//! things at once, per the brief: (1) a realistic captured policy body
//! parses into the typed shape with the decision-relevant fields intact,
//! and (2) a body carrying a construct this crate does not model still
//! parses successfully AND reports that construct via `Unmodelled` — never
//! silently drops it.

use egret_ranger_eval::model::{ItemBucket, RangerPolicy, ServicePolicySet, Unmodelled};

/// `tests/ranger/fixtures/service-policies.json` — a captured real Ranger
/// download (`ServicePolicies` envelope) with 3 policies covering access,
/// mask and row-filter. Read from the repo root, not invented.
fn fixture_json() -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/ranger/fixtures/service-policies.json"
    );
    std::fs::read_to_string(path).expect("read captured fixture")
}

#[test]
fn parses_captured_service_policies_fixture() {
    let raw = fixture_json();
    let set: ServicePolicySet = serde_json::from_str(&raw).expect("parse captured fixture");

    assert_eq!(set.service_name, "trino_prod");
    assert_eq!(set.service_id, Some(1));
    assert_eq!(set.policy_version, Some(5));
    assert_eq!(set.policies.len(), 3);
    assert!(
        set.unmodelled.is_empty(),
        "captured fixture carries no envelope-level unmodelled constructs: {:?}",
        set.unmodelled
    );

    let access = set
        .policies
        .iter()
        .find(|p| p.name == "p-access")
        .expect("p-access policy");
    assert_eq!(access.policy_type, 0);
    assert!(access.is_enabled);
    assert_eq!(access.resources["catalog"].values, vec!["hive".to_string()]);
    assert_eq!(access.resources["column"].values, vec!["*".to_string()]);
    assert_eq!(access.policy_items.len(), 1);
    assert_eq!(access.policy_items[0].users, vec!["alice".to_string()]);
    assert_eq!(access.policy_items[0].accesses[0].access_type, "select");
    assert!(access.policy_items[0].accesses[0].is_allowed);
    assert!(
        access.unmodelled.is_empty(),
        "realistic access policy carries no unmodelled constructs: {:?}",
        access.unmodelled
    );

    let mask = set
        .policies
        .iter()
        .find(|p| p.name == "p-mask")
        .expect("p-mask policy");
    assert_eq!(mask.policy_type, 1);
    assert_eq!(mask.data_mask_policy_items.len(), 1);
    let mask_info = mask.data_mask_policy_items[0]
        .data_mask_info
        .as_ref()
        .expect("dataMaskInfo present");
    assert_eq!(mask_info.mask_type, "MASK_HASH");
    assert!(mask.unmodelled.is_empty(), "{:?}", mask.unmodelled);

    let rowfilter = set
        .policies
        .iter()
        .find(|p| p.name == "p-rowfilter")
        .expect("p-rowfilter policy");
    assert_eq!(rowfilter.policy_type, 2);
    let rf_info = rowfilter.row_filter_policy_items[0]
        .row_filter_info
        .as_ref()
        .expect("rowFilterInfo present");
    assert_eq!(rf_info.filter_expr, "country = 'US'");
    assert!(
        rowfilter.unmodelled.is_empty(),
        "{:?}",
        rowfilter.unmodelled
    );
}

#[test]
fn validity_schedules_parse_successfully_and_are_reported() {
    let body = serde_json::json!({
        "id": 10, "name": "time-bound", "service": "trino_prod", "serviceType": "trino",
        "policyType": 0, "isEnabled": true,
        "resources": {"catalog": {"values": ["hive"], "isExcludes": false, "isRecursive": false}},
        "policyItems": [{"users": ["alice"], "groups": [], "accesses": [{"type": "select", "isAllowed": true}]}],
        "validitySchedules": [
            {"startTime": "2026-01-01 00:00:00", "endTime": "2026-12-31 23:59:59", "timeZone": "UTC"}
        ]
    });
    let policy: RangerPolicy =
        serde_json::from_value(body).expect("parses successfully despite validitySchedules");
    assert!(
        policy.unmodelled.contains(&Unmodelled::ValiditySchedules),
        "{:?}",
        policy.unmodelled
    );
    // Modelled fields still round-trip alongside the report.
    assert_eq!(policy.policy_items.len(), 1);
    assert_eq!(policy.policy_items[0].users, vec!["alice".to_string()]);
}

#[test]
fn policy_conditions_zone_delegate_admin_and_explicit_deny_are_reported() {
    let body = serde_json::json!({
        "id": 11, "name": "mixed", "service": "trino_prod", "serviceType": "trino",
        "policyType": 0, "isEnabled": true, "zoneName": "zone-a",
        "resources": {"catalog": {"values": ["hive"], "isExcludes": false, "isRecursive": false}},
        "conditions": [{"type": "ip-range", "values": ["10.0.0.0/8"]}],
        "policyItems": [
            {
                "users": ["alice"], "groups": [],
                "accesses": [{"type": "select", "isAllowed": false}],
                "delegateAdmin": true,
                "conditions": [{"type": "ip-range", "values": ["10.0.0.0/8"]}]
            }
        ]
    });
    let policy: RangerPolicy =
        serde_json::from_value(body).expect("parses successfully despite unmodelled constructs");

    assert!(policy.unmodelled.contains(&Unmodelled::PolicyConditions));
    assert!(policy
        .unmodelled
        .iter()
        .any(|u| matches!(u, Unmodelled::ZoneName(z) if z == "zone-a")));
    assert!(policy.unmodelled.iter().any(|u| matches!(
        u,
        Unmodelled::ItemConditions {
            bucket: ItemBucket::PolicyItems,
            index: 0
        }
    )));
    assert!(policy.unmodelled.iter().any(|u| matches!(
        u,
        Unmodelled::DelegateAdmin {
            bucket: ItemBucket::PolicyItems,
            index: 0
        }
    )));
    assert!(policy.unmodelled.iter().any(|u| matches!(
        u,
        Unmodelled::ExplicitDeny {
            bucket: ItemBucket::PolicyItems,
            index: 0,
            ..
        }
    )));
    // The item's principals/accesses still round-trip.
    assert_eq!(policy.policy_items[0].users, vec!["alice".to_string()]);
}

#[test]
fn mask_and_rowfilter_expression_markers_are_reported_but_literal_values_are_not() {
    let custom_mask = serde_json::json!({
        "id": 12, "name": "dynamic-mask", "service": "trino_prod", "serviceType": "trino",
        "policyType": 1, "isEnabled": true,
        "resources": {"catalog": {"values": ["hive"], "isExcludes": false, "isRecursive": false}},
        "dataMaskPolicyItems": [{
            "users": ["alice"], "groups": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "CUSTOM", "valueExpr": "substr({col},1,2)"}
        }]
    });
    let policy: RangerPolicy = serde_json::from_value(custom_mask).expect("parses");
    assert!(
        policy
            .unmodelled
            .iter()
            .any(|u| matches!(u, Unmodelled::MaskExpressionMarker { item_index: 0, .. })),
        "{:?}",
        policy.unmodelled
    );

    let dynamic_rowfilter = serde_json::json!({
        "id": 13, "name": "dynamic-rf", "service": "trino_prod", "serviceType": "trino",
        "policyType": 2, "isEnabled": true,
        "resources": {"catalog": {"values": ["hive"], "isExcludes": false, "isRecursive": false}},
        "rowFilterPolicyItems": [{
            "users": ["alice"], "groups": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "rowFilterInfo": {"filterExpr": "owner = '{USER}'"}
        }]
    });
    let policy2: RangerPolicy = serde_json::from_value(dynamic_rowfilter).expect("parses");
    assert!(
        policy2.unmodelled.iter().any(|u| matches!(
            u,
            Unmodelled::RowFilterExpressionMarker { item_index: 0, .. }
        )),
        "{:?}",
        policy2.unmodelled
    );

    let literal_rowfilter = serde_json::json!({
        "id": 14, "name": "literal-rf", "service": "trino_prod", "serviceType": "trino",
        "policyType": 2, "isEnabled": true,
        "resources": {"catalog": {"values": ["hive"], "isExcludes": false, "isRecursive": false}},
        "rowFilterPolicyItems": [{
            "users": ["alice"], "groups": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "rowFilterInfo": {"filterExpr": "country = 'US'"}
        }]
    });
    let policy3: RangerPolicy = serde_json::from_value(literal_rowfilter).expect("parses");
    assert!(
        policy3.unmodelled.is_empty(),
        "a literal filter expression carries no marker: {:?}",
        policy3.unmodelled
    );
}

#[test]
fn mask_condition_expr_is_reported() {
    let body = serde_json::json!({
        "id": 15, "name": "conditional-mask", "service": "trino_prod", "serviceType": "trino",
        "policyType": 1, "isEnabled": true,
        "resources": {"catalog": {"values": ["hive"], "isExcludes": false, "isRecursive": false}},
        "dataMaskPolicyItems": [{
            "users": ["alice"], "groups": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK_HASH", "conditionExpr": "user_role() = 'pii'"}
        }]
    });
    let policy: RangerPolicy = serde_json::from_value(body).expect("parses");
    assert!(
        policy
            .unmodelled
            .iter()
            .any(|u| matches!(u, Unmodelled::MaskConditionExpr { item_index: 0 })),
        "{:?}",
        policy.unmodelled
    );
}

#[test]
fn security_zones_at_envelope_level_are_reported() {
    let body = serde_json::json!({
        "serviceName": "trino_prod", "serviceId": 1, "policyVersion": 1,
        "securityZones": {"zone-a": {"name": "zone-a"}},
        "policies": []
    });
    let set: ServicePolicySet = serde_json::from_value(body).expect("parses");
    assert!(set.unmodelled.contains(&Unmodelled::SecurityZones));
}
