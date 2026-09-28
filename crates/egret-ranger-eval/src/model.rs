//! A typed reader for Ranger policy bodies.
//!
//! Field names below were verified against two sources, not transcribed
//! from any summary: `crates/egret-policy/src/admin/ranger_json.rs` (the
//! writer, for the shapes egret itself produces) and Apache Ranger's own
//! `RangerPolicy` model
//! (`agents-common/.../plugin/model/RangerPolicy.java`, fetched from
//! `apache/ranger@master`), plus the captured download at
//! `tests/ranger/fixtures/service-policies.json`.
//!
//! Two kinds of field live here:
//!
//! - **Round-tripped** fields decide the outcome once Tasks 4-6 evaluate
//!   this shape: `policyType`, `isEnabled`, `policyPriority`,
//!   `isDenyAllElse`, `resources`/`additionalResources` (with `values`,
//!   `isExcludes`, `isRecursive`), the six item buckets, and per item
//!   `users`/`groups`/`roles`/`accesses[].type`/`accesses[].isAllowed`.
//! - **Reported, not evaluated** constructs change the decision in ways
//!   this reader does not implement: a policy-wide validity window, a
//!   policy- or item-level condition, security-zone scoping, an explicit
//!   per-access deny sitting inside an "allow" bucket, `delegateAdmin`,
//!   and a mask/row-filter expression containing a Ranger runtime macro
//!   (`{USER}`, a CUSTOM mask's `{col}`, …). Each is a variant of
//!   [`Unmodelled`], collected on the policy (or, for `securityZones`, the
//!   envelope) that carried it. Parsing still succeeds — the alternative,
//!   dropping the field silently, is the exact defect this crate exists to
//!   prevent.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

// ── Public types ─────────────────────────────────────────────────────────

/// One `ServicePolicies` download: the envelope Ranger (or egret's own
/// `/service/plugins/policies/download/<service>` equivalent) returns for a
/// service — see `tests/ranger/fixtures/service-policies.json` for a
/// captured example.
#[derive(Debug, Clone, PartialEq)]
pub struct ServicePolicySet {
    pub service_name: String,
    pub service_id: Option<i64>,
    pub policy_version: Option<i64>,
    pub policies: Vec<RangerPolicy>,
    /// Envelope-level constructs this reader does not model (currently:
    /// only a non-empty `securityZones`). Per-policy constructs live on
    /// each [`RangerPolicy::unmodelled`] instead.
    pub unmodelled: Vec<Unmodelled>,
}

/// One Ranger policy body (`policy.authz_policies.policy_jsonb`), typed for
/// reading. `policy_type` is the raw Ranger `policyType` code: `0` access,
/// `1` masking, `2` row-filter.
#[derive(Debug, Clone, PartialEq)]
pub struct RangerPolicy {
    pub id: Option<i64>,
    pub name: String,
    pub service: String,
    pub policy_type: i32,
    pub is_enabled: bool,
    pub policy_priority: i32,
    pub is_deny_all_else: bool,
    pub resources: BTreeMap<String, ResourceLevel>,
    pub additional_resources: Vec<BTreeMap<String, ResourceLevel>>,
    pub policy_items: Vec<PolicyItem>,
    pub deny_policy_items: Vec<PolicyItem>,
    pub allow_exceptions: Vec<PolicyItem>,
    pub deny_exceptions: Vec<PolicyItem>,
    pub data_mask_policy_items: Vec<PolicyItem>,
    pub row_filter_policy_items: Vec<PolicyItem>,
    /// Constructs on this policy that this reader detected but does not
    /// model. Never empty silently: if a field below would have changed
    /// the decision and isn't in the round-tripped set above, it is
    /// reported here instead of being dropped.
    pub unmodelled: Vec<Unmodelled>,
}

/// One resource-type selector inside a policy's `resources` (or
/// `additionalResources`) map — e.g. `"catalog": {"values": ["hive"], ...}`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ResourceLevel {
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default, rename = "isExcludes")]
    pub is_excludes: bool,
    #[serde(default, rename = "isRecursive")]
    pub is_recursive: bool,
}

/// One entry of a policy item's `accesses` list — e.g.
/// `{"type": "select", "isAllowed": true}`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AccessEntry {
    #[serde(rename = "type")]
    pub access_type: String,
    #[serde(default = "default_true", rename = "isAllowed")]
    pub is_allowed: bool,
}

/// One item of any of the six policy-item buckets
/// (`policyItems`/`denyPolicyItems`/`allowExceptions`/`denyExceptions`/
/// `dataMaskPolicyItems`/`rowFilterPolicyItems`). `data_mask_info` and
/// `row_filter_info` are populated only for the mask/row-filter buckets.
///
/// Deliberately does NOT carry `delegateAdmin` or item-level `conditions`:
/// their presence changes the decision but this reader does not evaluate
/// either, so they are surfaced only through the owning policy's
/// [`RangerPolicy::unmodelled`] ([`Unmodelled::DelegateAdmin`] /
/// [`Unmodelled::ItemConditions`]), not stored redundantly here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolicyItem {
    pub users: Vec<String>,
    pub groups: Vec<String>,
    pub roles: Vec<String>,
    pub accesses: Vec<AccessEntry>,
    pub data_mask_info: Option<MaskInfo>,
    pub row_filter_info: Option<RowFilterInfo>,
}

/// A `dataMaskPolicyItems[].dataMaskInfo` object.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct MaskInfo {
    #[serde(default, rename = "dataMaskType")]
    pub mask_type: String,
    #[serde(default, rename = "valueExpr")]
    pub value_expr: Option<String>,
    /// Ranger's `RangerPolicyItemDataMaskInfo.conditionExpr` — a
    /// conditional-masking expression. Not in the writer's shape and not
    /// evaluated here; a non-empty value is reported via
    /// [`Unmodelled::MaskConditionExpr`].
    #[serde(default, rename = "conditionExpr")]
    pub condition_expr: Option<String>,
}

/// A `rowFilterPolicyItems[].rowFilterInfo` object.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RowFilterInfo {
    #[serde(default, rename = "filterExpr")]
    pub filter_expr: String,
}

/// Which of a policy's six item lists a [`PolicyItem`]-scoped
/// [`Unmodelled`] variant came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemBucket {
    PolicyItems,
    DenyPolicyItems,
    AllowExceptions,
    DenyExceptions,
    DataMaskPolicyItems,
    RowFilterPolicyItems,
}

/// A construct this reader detected in a policy body but does not model.
/// Each variant names the field, and (where applicable) exactly where it
/// was found, so a caller — today, a human; a later task, the evaluator —
/// can decide to force an `Undecidable` outcome rather than proceed on an
/// incomplete picture of what the policy actually says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmodelled {
    /// Top-level `validitySchedules`: the policy is only in effect during a
    /// time window this reader does not evaluate.
    ValiditySchedules,
    /// Top-level `conditions`: a policy-wide condition (e.g. an IP-range
    /// restriction) gating every item in the policy.
    PolicyConditions,
    /// An item-level `conditions` list on one policy item.
    ItemConditions { bucket: ItemBucket, index: usize },
    /// `zoneName`: the policy is scoped to a Ranger security zone.
    ZoneName(String),
    /// The envelope's `securityZones` map is non-empty.
    SecurityZones,
    /// A mask item's `dataMaskInfo.valueExpr` contains a `{TOKEN}` runtime
    /// macro/placeholder (e.g. a CUSTOM mask's `{col}`) this reader cannot
    /// substitute.
    MaskExpressionMarker { item_index: usize, expr: String },
    /// A mask item's `dataMaskInfo.conditionExpr` is set.
    MaskConditionExpr { item_index: usize },
    /// A row-filter item's `rowFilterInfo.filterExpr` contains a `{TOKEN}`
    /// runtime macro (e.g. `{USER}`) this reader cannot substitute.
    RowFilterExpressionMarker { item_index: usize, expr: String },
    /// An access entry has `isAllowed: false` — meaningful only inside an
    /// "allow" bucket (`policyItems`/`allowExceptions`/
    /// `dataMaskPolicyItems`/`rowFilterPolicyItems`), where list membership
    /// alone would otherwise imply allow; the writer never emits this.
    ExplicitDeny {
        bucket: ItemBucket,
        index: usize,
        access_type: String,
    },
    /// A policy item has `delegateAdmin: true`.
    DelegateAdmin { bucket: ItemBucket, index: usize },
}

fn default_true() -> bool {
    true
}

/// True if `s` contains a Ranger runtime macro/placeholder of the form
/// `{TOKEN}` — a bare run of alphanumerics/underscore between braces, e.g.
/// `{USER}`, `{USER_IP}`, or a CUSTOM mask's `{col}`. Ranger substitutes
/// these at evaluation time using request context (the current user, the
/// column being read) this reader does not have, so any such marker
/// changes the decision in a way this reader cannot resolve on its own.
fn contains_expression_marker(s: &str) -> bool {
    let mut i = 0;
    while i < s.len() {
        if s.as_bytes()[i] == b'{' {
            match s[i + 1..].find('}') {
                Some(rel) => {
                    let token = &s[i + 1..i + 1 + rel];
                    if !token.is_empty()
                        && token
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    {
                        return true;
                    }
                    i += 1 + rel;
                }
                None => break,
            }
        }
        i += 1;
    }
    false
}

// ── Deserialize: raw wire shapes, converted into the public types above ───
//
// `RangerPolicy` and `ServicePolicySet` implement `Deserialize` by hand
// (deserializing into a private `Raw*` struct that mirrors the JSON field
// names exactly, then converting) so the conversion step can detect and
// collect `Unmodelled` constructs in the same pass that builds the typed
// value — there is no separate "validate after the fact" step to
// accidentally skip.

#[derive(Deserialize)]
struct RawServicePolicySet {
    #[serde(default, rename = "serviceName")]
    service_name: String,
    #[serde(default, rename = "serviceId")]
    service_id: Option<i64>,
    #[serde(default, rename = "policyVersion")]
    policy_version: Option<i64>,
    #[serde(default)]
    policies: Vec<RangerPolicy>,
    #[serde(default, rename = "securityZones")]
    security_zones: serde_json::Map<String, serde_json::Value>,
}

impl<'de> Deserialize<'de> for ServicePolicySet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawServicePolicySet::deserialize(deserializer)?;
        let mut unmodelled = Vec::new();
        if !raw.security_zones.is_empty() {
            unmodelled.push(Unmodelled::SecurityZones);
        }
        Ok(ServicePolicySet {
            service_name: raw.service_name,
            service_id: raw.service_id,
            policy_version: raw.policy_version,
            policies: raw.policies,
            unmodelled,
        })
    }
}

#[derive(Deserialize)]
struct RawRangerPolicy {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    service: String,
    #[serde(default, rename = "policyType")]
    policy_type: i32,
    #[serde(default = "default_true", rename = "isEnabled")]
    is_enabled: bool,
    #[serde(default, rename = "policyPriority")]
    policy_priority: i32,
    #[serde(default, rename = "isDenyAllElse")]
    is_deny_all_else: bool,
    #[serde(default)]
    resources: BTreeMap<String, ResourceLevel>,
    #[serde(default, rename = "additionalResources")]
    additional_resources: Vec<BTreeMap<String, ResourceLevel>>,
    #[serde(default, rename = "policyItems")]
    policy_items: Vec<RawPolicyItem>,
    #[serde(default, rename = "denyPolicyItems")]
    deny_policy_items: Vec<RawPolicyItem>,
    #[serde(default, rename = "allowExceptions")]
    allow_exceptions: Vec<RawPolicyItem>,
    #[serde(default, rename = "denyExceptions")]
    deny_exceptions: Vec<RawPolicyItem>,
    #[serde(default, rename = "dataMaskPolicyItems")]
    data_mask_policy_items: Vec<RawPolicyItem>,
    #[serde(default, rename = "rowFilterPolicyItems")]
    row_filter_policy_items: Vec<RawPolicyItem>,
    /// Top-level `conditions` — Ranger's policy-wide condition list.
    #[serde(default)]
    conditions: Vec<serde_json::Value>,
    #[serde(default, rename = "validitySchedules")]
    validity_schedules: Vec<serde_json::Value>,
    #[serde(default, rename = "zoneName")]
    zone_name: Option<String>,
}

#[derive(Deserialize)]
struct RawPolicyItem {
    #[serde(default)]
    users: Vec<String>,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    roles: Vec<String>,
    #[serde(default)]
    accesses: Vec<AccessEntry>,
    /// Item-level `conditions`.
    #[serde(default)]
    conditions: Vec<serde_json::Value>,
    #[serde(default, rename = "delegateAdmin")]
    delegate_admin: bool,
    #[serde(default, rename = "dataMaskInfo")]
    data_mask_info: Option<MaskInfo>,
    #[serde(default, rename = "rowFilterInfo")]
    row_filter_info: Option<RowFilterInfo>,
}

impl<'de> Deserialize<'de> for RangerPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawRangerPolicy::deserialize(deserializer)?;
        Ok(RangerPolicy::from_raw(raw))
    }
}

impl RangerPolicy {
    fn from_raw(raw: RawRangerPolicy) -> Self {
        let mut unmodelled = Vec::new();

        if !raw.conditions.is_empty() {
            unmodelled.push(Unmodelled::PolicyConditions);
        }
        if !raw.validity_schedules.is_empty() {
            unmodelled.push(Unmodelled::ValiditySchedules);
        }
        if let Some(zone) = raw.zone_name.filter(|z| !z.is_empty()) {
            unmodelled.push(Unmodelled::ZoneName(zone));
        }

        let policy_items =
            convert_bucket(raw.policy_items, ItemBucket::PolicyItems, &mut unmodelled);
        let deny_policy_items = convert_bucket(
            raw.deny_policy_items,
            ItemBucket::DenyPolicyItems,
            &mut unmodelled,
        );
        let allow_exceptions = convert_bucket(
            raw.allow_exceptions,
            ItemBucket::AllowExceptions,
            &mut unmodelled,
        );
        let deny_exceptions = convert_bucket(
            raw.deny_exceptions,
            ItemBucket::DenyExceptions,
            &mut unmodelled,
        );
        let data_mask_policy_items = convert_bucket(
            raw.data_mask_policy_items,
            ItemBucket::DataMaskPolicyItems,
            &mut unmodelled,
        );
        let row_filter_policy_items = convert_bucket(
            raw.row_filter_policy_items,
            ItemBucket::RowFilterPolicyItems,
            &mut unmodelled,
        );

        RangerPolicy {
            id: raw.id,
            name: raw.name,
            service: raw.service,
            policy_type: raw.policy_type,
            is_enabled: raw.is_enabled,
            policy_priority: raw.policy_priority,
            is_deny_all_else: raw.is_deny_all_else,
            resources: raw.resources,
            additional_resources: raw.additional_resources,
            policy_items,
            deny_policy_items,
            allow_exceptions,
            deny_exceptions,
            data_mask_policy_items,
            row_filter_policy_items,
            unmodelled,
        }
    }
}

fn convert_bucket(
    items: Vec<RawPolicyItem>,
    bucket: ItemBucket,
    unmodelled: &mut Vec<Unmodelled>,
) -> Vec<PolicyItem> {
    items
        .into_iter()
        .enumerate()
        .map(|(index, raw)| build_policy_item(raw, bucket, index, unmodelled))
        .collect()
}

fn build_policy_item(
    raw: RawPolicyItem,
    bucket: ItemBucket,
    index: usize,
    unmodelled: &mut Vec<Unmodelled>,
) -> PolicyItem {
    if !raw.conditions.is_empty() {
        unmodelled.push(Unmodelled::ItemConditions { bucket, index });
    }
    if raw.delegate_admin {
        unmodelled.push(Unmodelled::DelegateAdmin { bucket, index });
    }
    for access in &raw.accesses {
        if !access.is_allowed {
            unmodelled.push(Unmodelled::ExplicitDeny {
                bucket,
                index,
                access_type: access.access_type.clone(),
            });
        }
    }
    if let Some(mask_info) = &raw.data_mask_info {
        if mask_info
            .value_expr
            .as_deref()
            .is_some_and(contains_expression_marker)
        {
            unmodelled.push(Unmodelled::MaskExpressionMarker {
                item_index: index,
                expr: mask_info.value_expr.clone().unwrap_or_default(),
            });
        }
        if mask_info
            .condition_expr
            .as_deref()
            .is_some_and(|s| !s.is_empty())
        {
            unmodelled.push(Unmodelled::MaskConditionExpr { item_index: index });
        }
    }
    if let Some(row_filter_info) = &raw.row_filter_info {
        if contains_expression_marker(&row_filter_info.filter_expr) {
            unmodelled.push(Unmodelled::RowFilterExpressionMarker {
                item_index: index,
                expr: row_filter_info.filter_expr.clone(),
            });
        }
    }

    PolicyItem {
        users: raw.users,
        groups: raw.groups,
        roles: raw.roles,
        accesses: raw.accesses,
        data_mask_info: raw.data_mask_info,
        row_filter_info: raw.row_filter_info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expression_marker_detects_braced_tokens_but_not_plain_text() {
        assert!(contains_expression_marker("substr({col},1,2)"));
        assert!(contains_expression_marker("owner = '{USER}'"));
        assert!(contains_expression_marker("{USER_IP}"));
        assert!(!contains_expression_marker("country = 'US'"));
        assert!(!contains_expression_marker(""));
        // An empty or unterminated brace pair is not a token.
        assert!(!contains_expression_marker("{}"));
        assert!(!contains_expression_marker("unterminated {col"));
    }
}
