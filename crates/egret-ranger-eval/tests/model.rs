//! Task 3: a typed reader for Ranger policy bodies. These tests assert two
//! things at once, per the brief: (1) a realistic captured policy body
//! parses into the typed shape with the decision-relevant fields intact,
//! and (2) a body carrying a construct this crate does not model still
//! parses successfully AND reports that construct via `Unmodelled` — never
//! silently drops it.
//!
//! Fix round 1 (review) added: serviceDef retention (C2), envelope
//! tagPolicies/policyDeltas/serviceConfig detection (C3), round-trip and
//! default-value pinning for fields no earlier test exercised (I1),
//! resource-value and item-user expression-marker detection (I2), and
//! `ServicePolicySet::all_unmodelled` (I3).

use egret_ranger_eval::model::{
    ItemBucket, RangerPolicy, ServicePolicySet, TagPolicyBlock, Unmodelled,
};
use serde_json::{json, Value};

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

    // C2: serviceDef must be RETAINED (not dropped, not reported as
    // Unmodelled) — Task 4 needs it (accessTypes/impliedGrants,
    // resources[].matcherOptions, dataMaskDef, rowFilterDef).
    assert!(
        set.service_def.is_object(),
        "serviceDef must round-trip, not be dropped: {:?}",
        set.service_def
    );
    assert_eq!(set.service_def["name"], "trino");
    assert!(
        set.service_def["resources"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "serviceDef.resources must be non-empty: {:?}",
        set.service_def["resources"]
    );
    assert!(
        set.service_def["accessTypes"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "serviceDef.accessTypes must be non-empty: {:?}",
        set.service_def["accessTypes"]
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
    assert_eq!(mask_info.mask_type.as_deref(), Some("MASK_HASH"));
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
    assert_eq!(rf_info.filter_expr.as_deref(), Some("country = 'US'"));
    assert!(
        rowfilter.unmodelled.is_empty(),
        "{:?}",
        rowfilter.unmodelled
    );

    // I3: the fold-up sees nothing extra here either (nothing unmodelled
    // exists anywhere in this fixture, at either level).
    assert!(set.all_unmodelled().is_empty());
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
        "policyType": 0, "isEnabled": true, "zoneName": "  zone-a  ",
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
    // Minor fix: zoneName is trimmed before the emptiness check, so
    // whitespace padding around a real zone name doesn't get compared
    // literally.
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
fn zone_name_that_is_only_whitespace_is_not_reported() {
    let body = serde_json::json!({
        "id": 16, "name": "blank-zone", "service": "trino_prod", "serviceType": "trino",
        "policyType": 0, "isEnabled": true, "zoneName": "   ",
        "resources": {"catalog": {"values": ["hive"]}},
        "policyItems": []
    });
    let policy: RangerPolicy = serde_json::from_value(body).expect("parses");
    assert!(
        !policy
            .unmodelled
            .iter()
            .any(|u| matches!(u, Unmodelled::ZoneName(_))),
        "{:?}",
        policy.unmodelled
    );
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
        policy.unmodelled.iter().any(|u| matches!(
            u,
            Unmodelled::MaskExpressionMarker {
                bucket: ItemBucket::DataMaskPolicyItems,
                item_index: 0,
                ..
            }
        )),
        "{:?}",
        policy.unmodelled
    );

    // A ${{ ... }} request-time expression in a mask value (fix round 1).
    let request_expr_mask = serde_json::json!({
        "id": 17, "name": "dynamic-mask-2", "service": "trino_prod", "serviceType": "trino",
        "policyType": 1, "isEnabled": true,
        "resources": {"catalog": {"values": ["hive"]}},
        "dataMaskPolicyItems": [{
            "users": ["alice"], "groups": [],
            "accesses": [{"type": "select", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "CUSTOM", "valueExpr": "${{USER.dept}}"}
        }]
    });
    let policy_req: RangerPolicy = serde_json::from_value(request_expr_mask).expect("parses");
    assert!(
        policy_req.unmodelled.iter().any(|u| matches!(
            u,
            Unmodelled::MaskExpressionMarker {
                bucket: ItemBucket::DataMaskPolicyItems,
                item_index: 0,
                ..
            }
        )),
        "{:?}",
        policy_req.unmodelled
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
            Unmodelled::RowFilterExpressionMarker {
                bucket: ItemBucket::RowFilterPolicyItems,
                item_index: 0,
                ..
            }
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
        policy.unmodelled.iter().any(|u| matches!(
            u,
            Unmodelled::MaskConditionExpr {
                bucket: ItemBucket::DataMaskPolicyItems,
                item_index: 0
            }
        )),
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

/// C3: tagPolicies (a whole second, DENY-capable policy set) and
/// policyDeltas (an incremental update, meaning `policies` is partial) must
/// not be silently dropped. serviceConfig is metadata, but a defensible
/// fourth reported construct — included per review.
///
/// SP4d-2a Task 1 narrowed the `tagPolicies` half: a block whose `policies`
/// list is EMPTY is measured inert and is no longer reported, so this test now
/// carries a block with a policy in it. The tier boundaries, the `javap`
/// measurement behind them and the real captured block live in
/// `tests/tag_tiers.rs`.
#[test]
fn tag_policies_policy_deltas_and_service_config_are_reported() {
    let body = serde_json::json!({
        "serviceName": "trino_prod", "serviceId": 1, "policyVersion": 6,
        "tagPolicies": {
            "serviceName": "trino_prod_tag",
            "policies": [{"id": 70, "name": "pii", "policyType": 0}],
        },
        "policyDeltas": [{"id": 99, "changeType": 1, "policy": {"id": 99}}],
        "serviceConfig": {"username": "admin"},
        "policies": []
    });
    let set: ServicePolicySet = serde_json::from_value(body).expect("parses");
    assert!(
        set.unmodelled
            .contains(&Unmodelled::TagPolicies(TagPolicyBlock::Policies(1))),
        "{:?}",
        set.unmodelled
    );
    assert!(
        set.unmodelled.contains(&Unmodelled::PolicyDeltas),
        "{:?}",
        set.unmodelled
    );
    assert!(
        set.unmodelled.contains(&Unmodelled::ServiceConfig),
        "{:?}",
        set.unmodelled
    );
}

/// Negative control for C3: an absent tagPolicies/policyDeltas/serviceConfig
/// (the fixture's own shape) reports none of the three.
#[test]
fn absent_tag_policies_policy_deltas_and_service_config_report_nothing() {
    let body = serde_json::json!({
        "serviceName": "trino_prod", "serviceId": 1, "policyVersion": 1,
        "policies": []
    });
    let set: ServicePolicySet = serde_json::from_value(body).expect("parses");
    assert!(set.unmodelled.is_empty(), "{:?}", set.unmodelled);
}

/// I1: Ranger's own null-field defaults (verified in fix round 1 via
/// `javap` against `ranger-plugins-common` 2.5.0/2.8.0:
/// `setIsEnabled(null)` -> true, `RangerPolicyItemAccess.setIsAllowed(null)`
/// -> true, `setIsExcludes`/`setIsRecursive(null)` -> false,
/// `setPolicyPriority(null)` -> 0, `setIsDenyAllElse(null)` -> false) must
/// hold when the corresponding JSON key is omitted entirely, not just when
/// it's present with that value.
#[test]
fn defaults_match_ranger_when_keys_are_omitted() {
    let body = serde_json::json!({
        "id": 20, "name": "defaults", "service": "trino_prod", "serviceType": "trino",
        "policyType": 0,
        "resources": {"catalog": {"values": ["hive"]}},
        "policyItems": [{"users": ["alice"], "accesses": [{"type": "select"}]}]
    });
    let policy: RangerPolicy = serde_json::from_value(body).expect("parses");
    assert!(
        policy.is_enabled,
        "isEnabled omitted -> Ranger defaults true"
    );
    assert_eq!(policy.policy_priority, 0);
    assert!(!policy.is_deny_all_else);
    assert!(!policy.resources["catalog"].is_excludes);
    assert!(!policy.resources["catalog"].is_recursive);
    assert!(
        policy.policy_items[0].accesses[0].is_allowed,
        "accesses[].isAllowed omitted -> Ranger defaults true"
    );
}

/// I1: fields the review found were on the "must round-trip" list but
/// untested — a mutation flipping any of these previously left 7/7 tests
/// green.
#[test]
fn resource_and_item_fields_that_decide_outcomes_round_trip() {
    let body = serde_json::json!({
        "id": 21, "name": "roundtrip", "service": "trino_prod", "serviceType": "trino",
        "policyType": 0, "isEnabled": true, "policyPriority": 1, "isDenyAllElse": true,
        "resources": {"catalog": {"values": ["hive"], "isExcludes": true, "isRecursive": true}},
        "additionalResources": [{"catalog": {"values": ["iceberg"], "isExcludes": false, "isRecursive": false}}],
        "policyItems": [{
            "users": ["alice"], "groups": ["analysts"], "roles": ["r1"],
            "accesses": [{"type": "select", "isAllowed": true}]
        }]
    });
    let policy: RangerPolicy = serde_json::from_value(body).expect("parses");
    assert_eq!(policy.policy_priority, 1);
    assert!(policy.is_deny_all_else);
    assert!(policy.resources["catalog"].is_excludes);
    assert!(policy.resources["catalog"].is_recursive);
    assert_eq!(policy.additional_resources.len(), 1);
    assert_eq!(
        policy.additional_resources[0]["catalog"].values,
        vec!["iceberg".to_string()]
    );
    assert_eq!(policy.policy_items[0].groups, vec!["analysts".to_string()]);
    assert_eq!(policy.policy_items[0].roles, vec!["r1".to_string()]);
}

/// I2: marker detection applied consistently to the two surfaces the
/// review found missing — a resource `values` entry (both the primary
/// `resources` map and an `additionalResources[N]` map) and an item's
/// `users` list (Ranger's own `{USER}`/`{OWNER}` current-request macros).
#[test]
fn resource_value_and_user_expression_markers_are_reported() {
    let body = serde_json::json!({
        "id": 22, "name": "dynamic-resource", "service": "trino_prod", "serviceType": "trino",
        "policyType": 0, "isEnabled": true,
        "resources": {"trinouser": {"values": ["{USER}"]}},
        "additionalResources": [{"catalog": {"values": ["${{USER.dept}}"]}}],
        "policyItems": [{"users": ["{OWNER}"], "accesses": [{"type": "select", "isAllowed": true}]}]
    });
    let policy: RangerPolicy = serde_json::from_value(body).expect("parses");

    assert!(
        policy.unmodelled.iter().any(|u| matches!(
            u,
            Unmodelled::ResourceValueExpressionMarker {
                additional_index: None,
                resource_type,
                ..
            } if resource_type == "trinouser"
        )),
        "{:?}",
        policy.unmodelled
    );
    assert!(
        policy.unmodelled.iter().any(|u| matches!(
            u,
            Unmodelled::ResourceValueExpressionMarker {
                additional_index: Some(0),
                resource_type,
                ..
            } if resource_type == "catalog"
        )),
        "{:?}",
        policy.unmodelled
    );
    assert!(
        policy.unmodelled.iter().any(|u| matches!(
            u,
            Unmodelled::UserExpressionMarker {
                bucket: ItemBucket::PolicyItems,
                index: 0,
                ..
            }
        )),
        "{:?}",
        policy.unmodelled
    );

    // Still round-trips: the literal value is kept even though it's flagged.
    assert_eq!(
        policy.resources["trinouser"].values,
        vec!["{USER}".to_string()]
    );
    assert_eq!(policy.policy_items[0].users, vec!["{OWNER}".to_string()]);
}

/// Negative control for I2: an ordinary literal resource value and
/// username report nothing.
#[test]
fn literal_resource_values_and_users_report_no_marker() {
    let body = serde_json::json!({
        "id": 23, "name": "literal-resource", "service": "trino_prod", "serviceType": "trino",
        "policyType": 0, "isEnabled": true,
        "resources": {"catalog": {"values": ["hive"]}},
        "policyItems": [{"users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}]}]
    });
    let policy: RangerPolicy = serde_json::from_value(body).expect("parses");
    assert!(policy.unmodelled.is_empty(), "{:?}", policy.unmodelled);
}

/// I3: a caller checking only `ServicePolicySet::unmodelled` misses every
/// policy-level report; `all_unmodelled` must fold both levels together.
#[test]
fn all_unmodelled_folds_envelope_and_policy_level_reports() {
    let body = serde_json::json!({
        "serviceName": "trino_prod", "serviceId": 1, "policyVersion": 1,
        "securityZones": {"zone-a": {"name": "zone-a"}},
        "policies": [{
            "id": 30, "name": "p", "service": "trino_prod", "serviceType": "trino",
            "policyType": 0, "isEnabled": true,
            "resources": {"catalog": {"values": ["hive"]}},
            "policyItems": [],
            "validitySchedules": [{"startTime": "x", "endTime": "y", "timeZone": "UTC"}]
        }]
    });
    let set: ServicePolicySet = serde_json::from_value(body).expect("parses");

    // Checking only the envelope-level field misses the policy-level report.
    assert_eq!(set.unmodelled.len(), 1);
    assert!(matches!(set.unmodelled[0], Unmodelled::SecurityZones));

    let all = set.all_unmodelled();
    assert!(all.iter().any(|u| matches!(u, Unmodelled::SecurityZones)));
    assert!(all
        .iter()
        .any(|u| matches!(u, Unmodelled::ValiditySchedules)));
    assert_eq!(
        all.len(),
        set.unmodelled.len()
            + set
                .policies
                .iter()
                .map(|p| p.unmodelled.len())
                .sum::<usize>()
    );
}

// ── the absent/null sweep (SP4d-1 Task 7, finding F3) ────────────────────

/// **Every expectation here is a MEASURED value**, read back from
/// `JsonUtils.jsonToObject(json, <class>)` on the real
/// `ranger-plugins-common` 2.5.0 classpath — not from the wire format, and
/// not from the field declarations.
///
/// That distinction is the finding. `model.rs` was written by reading the
/// JSON; Ranger reads the same bytes with JACKSON
/// (`JsonUtils.getMapper()` is a `com.fasterxml.jackson.databind.ObjectMapper`
/// — neither `ranger-plugins-common` jar carries a Gson class) into classes
/// whose no-arg constructors call setters that substitute non-null defaults.
/// An ABSENT key is never bound and keeps the constructor's value; an
/// EXPLICIT null is bound through the setter, which substitutes the same
/// default, and is read back through the same getter — so Ranger cannot tell
/// the two apart. Measured, both shapes:
///
/// ```text
/// RangerPolicy{}            policyPriority=0  isEnabled=true  isDenyAllElse=false
///                           resources=Map(0)  policyItems=List(0) … validitySchedules=List(0)
///                           policyType=NULL   additionalResources=NULL  conditions=NULL
/// RangerPolicyResource{}    values=List(0)    isExcludes=false  isRecursive=false
/// RangerPolicyItem{}        users/groups/roles/accesses/conditions=List(0)  delegateAdmin=false
/// RangerPolicyItemAccess{}  type=""           isAllowed=true
/// ```
///
/// Before this was measured, `serde` rejected every explicit null with
/// `invalid type: null` — so this crate REFUSED documents the live plugin
/// enforces. A reader that cannot read is not fail-safe; it simply has no
/// answer where one is needed.
#[test]
fn absent_and_null_keys_take_rangers_own_defaults() {
    let parse = |policy: Value| -> ServicePolicySet {
        serde_json::from_value(json!({"serviceName": "trino", "policies": [policy]}))
            .expect("Ranger accepts this shape, so this crate must too")
    };

    // Everything absent.
    let p = parse(json!({"name": "p"}));
    let p = &p.policies[0];
    assert_eq!(
        p.policy_type, 0,
        "policyType null is Ranger's access bucket"
    );
    assert!(p.is_enabled, "isEnabled defaults TRUE, not false");
    assert_eq!(p.policy_priority, 0);
    assert!(!p.is_deny_all_else);
    assert!(p.resources.is_empty());
    assert!(p.policy_items.is_empty());

    // Everything explicitly null. Ranger reads this identically.
    let p = parse(json!({
        "name": null, "service": null, "policyType": null, "isEnabled": null,
        "policyPriority": null, "isDenyAllElse": null, "resources": null,
        "additionalResources": null, "policyItems": null, "denyPolicyItems": null,
        "allowExceptions": null, "denyExceptions": null, "dataMaskPolicyItems": null,
        "rowFilterPolicyItems": null, "conditions": null, "validitySchedules": null,
    }));
    let p = &p.policies[0];
    assert_eq!(p.name, "");
    assert_eq!(p.policy_type, 0);
    assert!(p.is_enabled);
    assert_eq!(p.policy_priority, 0);
    assert!(!p.is_deny_all_else);
    assert!(p.resources.is_empty());
    assert!(p.policy_items.is_empty());
    assert!(
        p.unmodelled.is_empty(),
        "a null `conditions` is an EMPTY condition list, not a condition: {:?}",
        p.unmodelled
    );

    // `RangerPolicyResource`: null lists and null booleans.
    let p = parse(json!({
        "name": "p",
        "resources": {"catalog": {"values": null, "isExcludes": null, "isRecursive": null}},
    }));
    let level = &p.policies[0].resources["catalog"];
    assert!(level.values.is_empty());
    assert!(!level.is_excludes);
    assert!(!level.is_recursive);

    // `RangerPolicyItemAccess`: an absent or null `type` is `""`, and a null
    // `isAllowed` is TRUE. `""` matches no concrete access type, so this is a
    // decision rather than a refusal — and it must not be an `ExplicitDeny`
    // either, since `isAllowed` came back true.
    let p = parse(json!({
        "name": "p",
        "policyItems": [
            {"users": ["a"], "accesses": [{"isAllowed": true}]},
            {"users": ["a"], "accesses": [{"type": null, "isAllowed": null}]},
        ],
    }));
    let items = &p.policies[0].policy_items;
    assert_eq!(items[0].accesses[0].access_type, "");
    assert!(items[0].accesses[0].is_allowed);
    assert_eq!(items[1].accesses[0].access_type, "");
    assert!(items[1].accesses[0].is_allowed);
    assert!(p.policies[0].unmodelled.is_empty());

    // `RangerDataMaskPolicyItem` / `RangerRowFilterPolicyItem`: the info
    // object is substituted, so it is present even when the JSON says null.
    // The BEHAVIOURS differ — see the mask_filter tests — but the READING
    // does not.
    let p = parse(json!({
        "name": "p", "policyType": 1,
        "dataMaskPolicyItems": [{"users": ["a"], "dataMaskInfo": null}],
    }));
    let info = p.policies[0].data_mask_policy_items[0]
        .data_mask_info
        .as_ref()
        .expect("Ranger substitutes an empty RangerPolicyItemDataMaskInfo");
    assert_eq!(
        info.mask_type, None,
        "…whose dataMaskType is NULL, not \"\""
    );

    let p = parse(json!({
        "name": "p", "policyType": 2,
        "rowFilterPolicyItems": [{"users": ["a"]}],
    }));
    let info = p.policies[0].row_filter_policy_items[0]
        .row_filter_info
        .as_ref()
        .expect("Ranger substitutes an empty RangerPolicyItemRowFilterInfo");
    assert_eq!(info.filter_expr, None);

    // The four ACCESS buckets have no such field on `RangerPolicyItem`, so a
    // `dataMaskInfo` written into `policyItems` is dropped by Ranger.
    let p = parse(json!({
        "name": "p",
        "policyItems": [{"users": ["a"], "dataMaskInfo": {"dataMaskType": "MASK"}}],
    }));
    assert_eq!(p.policies[0].policy_items[0].data_mask_info, None);
}
