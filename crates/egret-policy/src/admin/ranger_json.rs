//! Editor form ↔ Ranger access-policy JSON. The React editor posts an
//! `AccessPolicyInput`; `access_to_ranger_json` renders it to the camelCase
//! Ranger policy body stored verbatim in `authz_policies.policy_jsonb`.
//! P2 covers policyType 0 (access) only; masking/row-filter come in P3.
//!
//! Called by the create/update handlers in `policies/mod.rs`.
//!
//! NOTE: this serializer is lossy for policies NOT authored by egret's editor.
//! It always emits `delegateAdmin: false` and `isAllowed: true` per access.
//! The individual builders default `serviceType` to `"trino"`, but
//! `editor_to_ranger_json` stamps the target service's actual Ranger type
//! (`trino` | `starrocks`) onto the body. Editing a foreign/Ranger-native
//! policy through the UI therefore drops the omitted fields; egret-authored
//! policies are the design target — revisit if importing external Ranger
//! policies becomes a requirement.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Debug, Default, Deserialize, Serialize, utoipa::ToSchema)]
pub struct ResourceLevel {
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default, rename = "isExcludes")]
    pub is_excludes: bool,
    #[serde(default, rename = "isRecursive")]
    pub is_recursive: bool,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AccessItemInput {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub accesses: Vec<String>,
    #[serde(default = "default_effect")]
    pub effect: String,
}

fn default_effect() -> String {
    "allow".to_string()
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct AccessPolicyInput {
    pub name: String,
    pub service: String,
    #[serde(default = "default_true", rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(default)]
    pub resources: std::collections::BTreeMap<String, ResourceLevel>,
    #[serde(default)]
    pub items: Vec<AccessItemInput>,
    #[serde(default, rename = "allowExceptions")]
    pub allow_exceptions: Vec<AccessItemInput>,
    #[serde(default, rename = "denyExceptions")]
    pub deny_exceptions: Vec<AccessItemInput>,
    #[serde(default, rename = "isDenyAllElse")]
    pub is_deny_all_else: bool,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, rename = "isOverride")]
    pub is_override: bool,
    /// Extra independent resource hierarchies (Ranger additionalResources).
    #[serde(default)]
    pub additional: Vec<std::collections::BTreeMap<String, ResourceLevel>>,
}

fn resource_obj(l: &ResourceLevel) -> Value {
    json!({ "values": l.values, "isExcludes": l.is_excludes, "isRecursive": l.is_recursive })
}

/// Build a Ranger `resources` object from a generic map, dropping empty levels.
fn resources_object(map: &std::collections::BTreeMap<String, ResourceLevel>) -> Value {
    let mut m = Map::new();
    for (name, level) in map {
        if !level.values.is_empty() {
            m.insert(name.clone(), resource_obj(level));
        }
    }
    Value::Object(m)
}

fn item_obj(it: &AccessItemInput) -> Value {
    let accesses: Vec<Value> = it
        .accesses
        .iter()
        .map(|a| json!({ "type": a, "isAllowed": true }))
        .collect();
    json!({
        "users": it.users, "groups": it.groups, "roles": it.roles,
        "accesses": accesses, "delegateAdmin": false,
    })
}

/// Render an `AccessPolicyInput` into a Ranger policyType-0 JSON body.
pub fn access_to_ranger_json(input: &AccessPolicyInput) -> Value {
    let resources = resources_object(&input.resources);
    let additional_resources: Vec<Value> = input
        .additional
        .iter()
        .map(resources_object)
        .filter(|v| v.as_object().map(|o| !o.is_empty()).unwrap_or(false))
        .collect();
    let (allow, deny): (Vec<_>, Vec<_>) = input.items.iter().partition(|it| it.effect != "deny");
    let policy_items: Vec<Value> = allow.iter().map(|it| item_obj(it)).collect();
    let deny_all = input.is_deny_all_else;
    let deny_items: Vec<Value> = if deny_all {
        vec![]
    } else {
        deny.iter().map(|it| item_obj(it)).collect()
    };
    let deny_exc: Vec<Value> = if deny_all {
        vec![]
    } else {
        input.deny_exceptions.iter().map(item_obj).collect()
    };
    let allow_exc: Vec<Value> = input.allow_exceptions.iter().map(item_obj).collect();
    json!({
        "name": input.name,
        "service": input.service,
        "serviceType": "trino",
        "policyType": 0,
        "isEnabled": input.is_enabled,
        "description": input.description,
        "policyLabels": input.labels,
        "resources": resources,
        "additionalResources": additional_resources,
        "policyItems": policy_items,
        "denyPolicyItems": deny_items,
        "allowExceptions": allow_exc,
        "denyExceptions": deny_exc,
        "isDenyAllElse": deny_all,
        "policyPriority": if input.is_override { 1 } else { 0 },
    })
}

#[derive(Debug, Deserialize, Serialize, utoipa::ToSchema)]
pub struct MaskItemInput {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default, rename = "maskType")]
    pub mask_type: String,
    #[serde(default, rename = "valueExpr", skip_serializing_if = "Option::is_none")]
    pub value_expr: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, utoipa::ToSchema)]
pub struct MaskingPolicyInput {
    #[serde(default = "kind_masking")]
    pub kind: String,
    pub name: String,
    pub service: String,
    #[serde(default = "default_true", rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(default)]
    pub resources: std::collections::BTreeMap<String, ResourceLevel>,
    #[serde(default)]
    pub items: Vec<MaskItemInput>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, rename = "isOverride")]
    pub is_override: bool,
}

fn kind_masking() -> String {
    "masking".to_string()
}

fn mask_item_obj(it: &MaskItemInput) -> Value {
    let mut info = Map::new();
    info.insert("dataMaskType".into(), Value::from(it.mask_type.clone()));
    if it.mask_type == "CUSTOM" {
        if let Some(expr) = &it.value_expr {
            info.insert("valueExpr".into(), Value::from(expr.clone()));
        }
    }
    json!({
        "users": it.users, "groups": it.groups, "roles": [],
        "accesses": [{ "type": "select", "isAllowed": true }],
        "dataMaskInfo": Value::Object(info),
    })
}

/// Render a `MaskingPolicyInput` into a Ranger policyType-1 (masking) body.
pub fn masking_to_ranger_json(input: &MaskingPolicyInput) -> Value {
    json!({
        "name": input.name, "service": input.service, "serviceType": "trino",
        "policyType": 1, "isEnabled": input.is_enabled,
        "description": input.description, "policyLabels": input.labels,
        "resources": resources_object(&input.resources),
        "dataMaskPolicyItems": input.items.iter().map(mask_item_obj).collect::<Vec<_>>(),
        "policyItems": [], "denyPolicyItems": [],
        // **Always 0 or 1, and the pair this used to warn about is now
        // GUARDED rather than commented** (SP4d debt Task 6). Two rules that
        // overlap on one resource still compile to two policies at this same
        // priority — one serializer call cannot see the other's output — but a
        // SET that would be served with such a pair is refused:
        // [`crate::ambiguous_arbitration::ambiguous_arbitration_in`] is the one
        // rule, called by `egret_perm_compile::trigger::compile_desired` for
        // every recompile and by this crate's own `create_policy`/
        // `update_policy` for the editor. Read that module for the measurement
        // (SP4d-1 F2), for why an AGREEING tie is admitted and a DIFFERING one
        // is not, and for the correction to this comment's old claim that
        // "Ranger defines no order" — the comparator is a total order; what it
        // falls through to is the policy NAME, which is a rule's uuid.
        "policyPriority": if input.is_override { 1 } else { 0 },
    })
}

#[derive(Debug, Deserialize, Serialize, utoipa::ToSchema)]
pub struct RowFilterItemInput {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default, rename = "filterExpr")]
    pub filter_expr: String,
}

#[derive(Debug, Deserialize, Serialize, utoipa::ToSchema)]
pub struct RowFilterPolicyInput {
    #[serde(default = "kind_rowfilter")]
    pub kind: String,
    pub name: String,
    pub service: String,
    #[serde(default = "default_true", rename = "isEnabled")]
    pub is_enabled: bool,
    #[serde(default)]
    pub resources: std::collections::BTreeMap<String, ResourceLevel>,
    #[serde(default)]
    pub items: Vec<RowFilterItemInput>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, rename = "isOverride")]
    pub is_override: bool,
}

fn kind_rowfilter() -> String {
    "rowfilter".to_string()
}

fn rowfilter_item_obj(it: &RowFilterItemInput) -> Value {
    json!({
        "users": it.users, "groups": it.groups, "roles": [],
        "accesses": [{ "type": "select", "isAllowed": true }],
        "rowFilterInfo": { "filterExpr": it.filter_expr },
    })
}

/// Render a `RowFilterPolicyInput` into a Ranger policyType-2 (row-filter) body.
pub fn rowfilter_to_ranger_json(input: &RowFilterPolicyInput) -> Value {
    json!({
        "name": input.name, "service": input.service, "serviceType": "trino",
        "policyType": 2, "isEnabled": input.is_enabled,
        "description": input.description, "policyLabels": input.labels,
        "resources": resources_object(&input.resources),
        "rowFilterPolicyItems": input.items.iter().map(rowfilter_item_obj).collect::<Vec<_>>(),
        "policyItems": [], "denyPolicyItems": [],
        // **Always 0 or 1, and the pair this used to warn about is now
        // GUARDED rather than commented** (SP4d debt Task 6). Two rules that
        // overlap on one resource still compile to two policies at this same
        // priority — one serializer call cannot see the other's output — but a
        // SET that would be served with such a pair is refused:
        // [`crate::ambiguous_arbitration::ambiguous_arbitration_in`] is the one
        // rule, called by `egret_perm_compile::trigger::compile_desired` for
        // every recompile and by this crate's own `create_policy`/
        // `update_policy` for the editor. Read that module for the measurement
        // (SP4d-1 F2), for why an AGREEING tie is admitted and a DIFFERING one
        // is not, and for the correction to this comment's old claim that
        // "Ranger defines no order" — the comparator is a total order; what it
        // falls through to is the policy NAME, which is a rule's uuid.
        "policyPriority": if input.is_override { 1 } else { 0 },
    })
}

/// A serialized policy plus the metadata the handlers need to store it.
pub struct RangerPolicy {
    pub name: String,
    pub service: String,
    pub is_enabled: bool,
    pub body: Value,
    /// Set only by [`editor_to_ranger_json`], and only when the save filled an
    /// emptied level with `*` (SP4d-4 Task 11, B1). `None` everywhere else.
    pub filled: Option<FilledLevels>,
}

/// What an editor save changed about the form's resource levels (SP4d-4
/// Task 11, B1): which emptied levels it stored as `*`, and the body the same
/// form would have rendered WITHOUT that fill. The second is what lets the
/// handler tell whether the fill moved this policy ahead of another one in
/// Ranger's evaluation order — see
/// [`crate::ambiguous_arbitration::eval_order_shifts`].
#[derive(Debug, Clone)]
pub struct FilledLevels {
    /// Level names, in hierarchy order, deduplicated across the primary
    /// resource map and every `additional` one.
    pub names: Vec<String>,
    /// The Ranger body the unmodified form renders to.
    pub unfilled_body: Value,
}

/// Dispatch an editor form (`{kind, ...}`) to the right serializer and stamp the
/// target service's type onto the body. Missing `kind` defaults to "access" so
/// P2 clients stay wire-compatible. `service_type` is the profile's Ranger
/// service type (`"trino"` | `"starrocks"`) — the builders default to `"trino"`,
/// this overrides it for the actual target service.
pub fn editor_to_ranger_json(v: &Value, service_type: &str) -> Result<RangerPolicy, String> {
    let kind = v.get("kind").and_then(Value::as_str).unwrap_or("access");
    let (form, filled) = normalize_editor_levels(v, service_type, kind)?;
    let mut pol = render_editor_form(&form, service_type)?;
    if !filled.is_empty() {
        pol.filled = Some(FilledLevels {
            names: filled,
            unfilled_body: render_editor_form(v, service_type)?.body,
        });
    }
    Ok(pol)
}

fn render_editor_form(v: &Value, service_type: &str) -> Result<RangerPolicy, String> {
    let kind = v.get("kind").and_then(Value::as_str).unwrap_or("access");
    let mut pol: RangerPolicy = match kind {
        "access" => {
            let inp: AccessPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            RangerPolicy {
                name: inp.name.clone(),
                service: inp.service.clone(),
                is_enabled: inp.is_enabled,
                body: access_to_ranger_json(&inp),
                filled: None,
            }
        }
        "masking" => {
            let inp: MaskingPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            RangerPolicy {
                name: inp.name.clone(),
                service: inp.service.clone(),
                is_enabled: inp.is_enabled,
                body: masking_to_ranger_json(&inp),
                filled: None,
            }
        }
        "rowfilter" => {
            let inp: RowFilterPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            RangerPolicy {
                name: inp.name.clone(),
                service: inp.service.clone(),
                is_enabled: inp.is_enabled,
                body: rowfilter_to_ranger_json(&inp),
                filled: None,
            }
        }
        other => return Err(format!("unknown policy kind: {other}")),
    };
    if let Value::Object(m) = &mut pol.body {
        m.insert("serviceType".into(), Value::from(service_type));
    }
    Ok(pol)
}

/// The resource levels an editor form of `kind` addresses on a `service_type`
/// profile, root first, or `None` for a service type or kind the editor does
/// not normalise (a tag service, an unknown kind). A row filter stops at
/// `table` (`rowFilterDef`); access and mask forms run to `column`.
///
/// The names are the service defs' (`trino`: catalog, schema, table, column;
/// `starrocks`: catalog, database, table, column) — the same four
/// `egret_perm_compile::grants::resource_level_names` spells, which this crate
/// cannot call (it depends on this one).
fn editor_level_chain(service_type: &str, kind: &str) -> Option<&'static [&'static str]> {
    let all: &'static [&'static str] = match service_type {
        "trino" => &["catalog", "schema", "table", "column"],
        "starrocks" => &["catalog", "database", "table", "column"],
        _ => return None,
    };
    match kind {
        "access" | "masking" => Some(all),
        "rowfilter" => Some(&all[..3]),
        _ => None,
    }
}

/// A level the form carries but names nothing at: no value, or only blank ones.
fn level_is_empty(level: &Value) -> bool {
    level
        .get("values")
        .and_then(Value::as_array)
        .is_none_or(|vs| {
            vs.iter()
                .all(|v| v.as_str().is_none_or(|s| s.trim().is_empty()))
        })
}

/// **SP4d-4 Task 11, branch B1 — what an editor save does with an empty level.**
///
/// Measured on the real Ranger jars (`docs/en/plans/2026-10-11-sp4d-4-task1-
/// levels-result.md`): a policy that stops at catalog / schema / table is
/// denied `select` at column depth on Trino 470 and StarRocks 4.1.4 at both
/// Ranger versions, because both plugins check `SELECT col` per column. The
/// editor used to drop an emptied level on save, so "table `orders`, all
/// columns" was stored as a policy that lets nobody read a column. Per level,
/// along `chain` (root first), for each resource map of the form:
///
/// | the form's level | saved as |
/// |---|---|
/// | named | as written |
/// | **key absent** ("— none —") | stays absent |
/// | **key present, empty, root (catalog)** | **refused** — `*` would cover every catalog this profile serves |
/// | **key present, empty, non-root, every level above present** | `{"values":["*"],"isExcludes":false,"isRecursive":false}` |
/// | key present, empty, below an absent level | dropped (it is part of the truncated tail) |
/// | named, below an absent level | **refused** — a gap: Ranger decides nothing for it |
///
/// A map with every level absent or empty is left as the form had it (it
/// renders no resources, exactly as before). Levels outside `chain` (a tag, an
/// unknown name) are not touched.
///
/// **Editor path only.** [`resources_object`] is shared with the compiler's
/// serializers and still drops an empty level; a compiled body is unchanged
/// (`a_compiled_body_is_unchanged`). Stored rows are never rewritten.
fn normalize_editor_levels(
    v: &Value,
    service_type: &str,
    kind: &str,
) -> Result<(Value, Vec<String>), String> {
    let Some(chain) = editor_level_chain(service_type, kind) else {
        return Ok((v.clone(), Vec::new()));
    };
    let mut form = v.clone();
    let mut filled: Vec<&str> = Vec::new();

    let mut normalize_map = |map: &mut Value, is_primary: bool| -> Result<(), String> {
        let Some(obj) = map.as_object_mut() else {
            return Ok(());
        };
        let state = |obj: &Map<String, Value>, name: &str| match obj.get(name) {
            None => Presence::Absent,
            Some(l) if level_is_empty(l) => Presence::Empty,
            Some(_) => Presence::Named,
        };
        let none_named = chain
            .iter()
            .all(|n| !matches!(state(obj, n), Presence::Named));
        // A map that names nothing is the pre-B1 shape of an untouched form —
        // left as it was — unless it is the primary map with an emptied
        // catalog, which is refused below.
        if none_named && (!is_primary || matches!(state(obj, chain[0]), Presence::Absent)) {
            return Ok(());
        }
        if matches!(state(obj, chain[0]), Presence::Empty) {
            return Err(format!(
                "name a {root} — `*` here would cover every {root} this profile serves",
                root = chain[0]
            ));
        }
        let mut first_absent: Option<&str> = None;
        for name in chain {
            match (state(obj, name), first_absent) {
                (Presence::Absent, None) => first_absent = Some(name),
                (Presence::Absent, Some(_)) => {}
                (Presence::Empty, None) => {
                    obj.insert(
                        (*name).to_string(),
                        json!({ "values": ["*"], "isExcludes": false, "isRecursive": false }),
                    );
                    if !filled.contains(name) {
                        filled.push(name);
                    }
                }
                (Presence::Empty, Some(_)) => {
                    obj.remove(*name);
                }
                (Presence::Named, Some(missing)) => {
                    return Err(format!(
                        "the policy names `{name}` but not `{missing}`, the level above it — \
                         Ranger decides nothing for a policy with a gap in its resource levels. \
                         Name `{missing}` (use `*` for every {missing}), or remove `{name}`"
                    ));
                }
                (Presence::Named, None) => {}
            }
        }
        Ok(())
    };

    if let Some(r) = form.get_mut("resources") {
        normalize_map(r, true)?;
    }
    if let Some(Value::Array(extra)) = form.get_mut("additional") {
        for m in extra {
            normalize_map(m, false)?;
        }
    }
    Ok((form, filled.into_iter().map(str::to_string).collect()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Presence {
    Absent,
    Empty,
    Named,
}

// ── Tag-based policies (Access P3.2) ────────────────────────────────────────
//
// A tag policy is the same shape as a trino resource policy — the SAME
// item/mask/rowfilter buckets — but bound to the tag service and keyed on a
// classification TAG rather than a catalog.schema.table.column path. We reuse
// the trino builders verbatim and only retarget the service binding + resource
// selector; nothing about the enforcement item structure changes. Enforcement
// is LIVE-ONLY (the Ranger tag plugin applies the mask/filter); the body shape,
// summary, and storage are unit-tested here.

/// Rewrite a trino-shaped body into a TAG policy: `serviceType:"tag"`,
/// `service:<tagService>`, and a single `tag` resource. Leaves the
/// policyType/item buckets (`policyItems`/`dataMaskPolicyItems`/
/// `rowFilterPolicyItems`) exactly as the trino builder produced them.
fn tagify(body: &mut Value, tag: &str, tag_service: &str) {
    if let Value::Object(m) = body {
        m.insert("serviceType".into(), Value::from("tag"));
        m.insert("service".into(), Value::from(tag_service.to_string()));
        m.insert(
            "resources".into(),
            json!({ "tag": { "values": [tag], "isExcludes": false, "isRecursive": false } }),
        );
        // A tag policy targets exactly one tag — no additional resource hierarchies.
        m.remove("additionalResources");
    }
}

/// Tag access policy (policyType 0) keyed on `tag`, stored under `tag_service`.
pub fn tag_access_to_ranger_json(input: &AccessPolicyInput, tag: &str, tag_service: &str) -> Value {
    let mut body = access_to_ranger_json(input);
    tagify(&mut body, tag, tag_service);
    body
}

/// Tag masking policy (policyType 1) keyed on `tag`, stored under `tag_service`.
pub fn tag_masking_to_ranger_json(
    input: &MaskingPolicyInput,
    tag: &str,
    tag_service: &str,
) -> Value {
    let mut body = masking_to_ranger_json(input);
    tagify(&mut body, tag, tag_service);
    body
}

/// Tag row-filter policy (policyType 2) keyed on `tag`, stored under `tag_service`.
pub fn tag_rowfilter_to_ranger_json(
    input: &RowFilterPolicyInput,
    tag: &str,
    tag_service: &str,
) -> Value {
    let mut body = rowfilter_to_ranger_json(input);
    tagify(&mut body, tag, tag_service);
    body
}

/// True if an editor form is a tag policy (`kind` starts with `tag-`).
pub fn is_tag_kind(v: &Value) -> bool {
    v.get("kind")
        .and_then(Value::as_str)
        .map(|k| k.starts_with("tag-"))
        .unwrap_or(false)
}

/// Dispatch a tag-policy editor form (`{kind:"tag-*", tag, ...}`) to the right
/// serializer. `tag_service` is the resolved `<profile>_tag` service name the
/// handler looked up; the body's `service` binds to it. The trino profile the
/// editor targeted arrives in the form's `service` field and is ignored here
/// (the handler used it to resolve `tag_service`).
pub fn tag_editor_to_ranger_json(v: &Value, tag_service: &str) -> Result<RangerPolicy, String> {
    let kind = v
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("tag-access");
    let tag = v
        .get("tag")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "tag policy requires a non-empty `tag`".to_string())?
        .to_string();
    match kind {
        "tag-access" => {
            let inp: AccessPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            Ok(RangerPolicy {
                name: inp.name.clone(),
                service: tag_service.to_string(),
                is_enabled: inp.is_enabled,
                body: tag_access_to_ranger_json(&inp, &tag, tag_service),
                filled: None,
            })
        }
        "tag-masking" => {
            let inp: MaskingPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            Ok(RangerPolicy {
                name: inp.name.clone(),
                service: tag_service.to_string(),
                is_enabled: inp.is_enabled,
                body: tag_masking_to_ranger_json(&inp, &tag, tag_service),
                filled: None,
            })
        }
        "tag-rowfilter" => {
            let inp: RowFilterPolicyInput =
                serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
            Ok(RangerPolicy {
                name: inp.name.clone(),
                service: tag_service.to_string(),
                is_enabled: inp.is_enabled,
                body: tag_rowfilter_to_ranger_json(&inp, &tag, tag_service),
                filled: None,
            })
        }
        other => Err(format!("unknown tag policy kind: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::super::summarize_policy;
    use super::*;
    use serde_json::json;

    fn sample() -> AccessPolicyInput {
        serde_json::from_value(json!({
            "name": "allow-analysts-select",
            "service": "trino",
            "isEnabled": true,
            "resources": {
                "catalog": {"values": ["iceberg"]},
                "schema":  {"values": ["sales"]},
                "table":   {"values": ["orders"]},
                "column":  {"values": ["*"]}
            },
            "items": [
                {"users": ["alice"], "groups": ["analysts"], "accesses": ["select"], "effect": "allow"},
                {"groups": ["contractors"], "accesses": ["select"], "effect": "deny"}
            ]
        })).unwrap()
    }

    #[test]
    fn serializes_access_policy_to_ranger_json() {
        let body = access_to_ranger_json(&sample());
        assert_eq!(body["policyType"], 0);
        assert_eq!(body["name"], "allow-analysts-select");
        assert_eq!(body["isEnabled"], true);
        assert_eq!(body["resources"]["catalog"]["values"][0], "iceberg");
        assert_eq!(body["resources"]["column"]["values"][0], "*");
        assert_eq!(body["resources"]["catalog"]["isExcludes"], false);
        assert_eq!(body["policyItems"].as_array().unwrap().len(), 1);
        assert_eq!(body["denyPolicyItems"].as_array().unwrap().len(), 1);
        assert_eq!(body["policyItems"][0]["users"][0], "alice");
        assert_eq!(body["policyItems"][0]["accesses"][0]["type"], "select");
        assert_eq!(body["policyItems"][0]["accesses"][0]["isAllowed"], true);
    }

    #[test]
    fn round_trips_through_summary() {
        let body = access_to_ranger_json(&sample());
        let s = summarize_policy(7, "trino", true, "operator", &body);
        assert_eq!(s.policy_type, "access");
        assert_eq!(s.resource_path, "iceberg.sales.orders.*");
        assert!(s.principals.contains(&"alice".to_string()));
        assert!(s.principals.contains(&"analysts".to_string()));
        assert_eq!(s.permissions, vec!["select".to_string()]);
    }

    #[test]
    fn omits_empty_resource_levels() {
        let mut inp = sample();
        inp.resources.get_mut("column").unwrap().values.clear();
        let body = access_to_ranger_json(&inp);
        assert!(body["resources"].get("column").is_none());
        assert!(body["resources"].get("catalog").is_some());
    }

    fn masking_sample() -> MaskingPolicyInput {
        serde_json::from_value(json!({
            "kind": "masking", "name": "mask-email", "service": "trino", "isEnabled": true,
            "resources": {"catalog":{"values":["iceberg"]},"schema":{"values":["sales"]},
                          "table":{"values":["users"]},"column":{"values":["email"]}},
            "items": [{"users":["alice"],"groups":["pii"],"maskType":"MASK_HASH"}]
        }))
        .unwrap()
    }

    #[test]
    fn serializes_masking_policy() {
        let body = masking_to_ranger_json(&masking_sample());
        assert_eq!(body["policyType"], 1);
        assert_eq!(body["resources"]["column"]["values"][0], "email");
        let items = body["dataMaskPolicyItems"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["dataMaskInfo"]["dataMaskType"], "MASK_HASH");
        assert_eq!(items[0]["accesses"][0]["type"], "select");
        assert_eq!(items[0]["users"][0], "alice");
        let s = summarize_policy(1, "trino", true, "operator", &body);
        assert_eq!(s.policy_type, "masking");
        assert_eq!(s.resource_path, "iceberg.sales.users.email");
    }

    #[test]
    fn serializes_masking_custom_value_expr() {
        let mut inp = masking_sample();
        inp.items[0].mask_type = "CUSTOM".into();
        inp.items[0].value_expr = Some("substr({col},1,2)".into());
        let body = masking_to_ranger_json(&inp);
        assert_eq!(
            body["dataMaskPolicyItems"][0]["dataMaskInfo"]["valueExpr"],
            "substr({col},1,2)"
        );
    }

    fn rowfilter_sample() -> RowFilterPolicyInput {
        serde_json::from_value(json!({
            "kind": "rowfilter", "name": "us-only", "service": "trino", "isEnabled": true,
            "resources": {"catalog":{"values":["iceberg"]},"schema":{"values":["sales"]},"table":{"values":["orders"]}},
            "items": [{"groups":["analysts"],"filterExpr":"country = 'US'"}]
        }))
        .unwrap()
    }

    #[test]
    fn serializes_rowfilter_policy() {
        let body = rowfilter_to_ranger_json(&rowfilter_sample());
        assert_eq!(body["policyType"], 2);
        assert!(body["resources"].get("column").is_none());
        let items = body["rowFilterPolicyItems"].as_array().unwrap();
        assert_eq!(items[0]["rowFilterInfo"]["filterExpr"], "country = 'US'");
        assert_eq!(items[0]["groups"][0], "analysts");
        let s = summarize_policy(2, "trino", true, "operator", &body);
        assert_eq!(s.policy_type, "rowfilter");
        assert_eq!(s.resource_path, "iceberg.sales.orders");
    }

    #[test]
    fn access_serializes_exceptions_and_deny_all_else() {
        let inp: AccessPolicyInput = serde_json::from_value(json!({
            "kind":"access","name":"p","service":"trino","isEnabled":true,
            "description":"analysts read","labels":["pii","sales"],
            "resources":{"catalog":{"values":["iceberg"],"isExcludes":false,"isRecursive":false},
                         "schema":{"values":["sales"]},"table":{"values":["a","b"]},"column":{"values":["*"]}},
            "items":[{"users":["alice"],"accesses":["select"],"effect":"allow"}],
            "allowExceptions":[{"groups":["contractors"],"accesses":["select"]}],
            "isDenyAllElse":true,
            "denyExceptions":[{"users":["x"],"accesses":["select"]}]
        })).unwrap();
        let b = access_to_ranger_json(&inp);
        assert_eq!(b["policyType"], 0);
        assert_eq!(b["description"], "analysts read");
        assert_eq!(b["policyLabels"][0], "pii");
        assert_eq!(
            b["resources"]["table"]["values"].as_array().unwrap().len(),
            2
        );
        assert_eq!(b["allowExceptions"][0]["groups"][0], "contractors");
        assert_eq!(b["isDenyAllElse"], true);
        assert!(b["denyPolicyItems"].as_array().unwrap().is_empty());
        assert!(b["denyExceptions"].as_array().unwrap().is_empty());
    }

    #[test]
    fn summary_unions_deny_and_exception_principals() {
        let body = json!({
            "name":"deny-only","policyType":0,
            "resources":{"catalog":{"values":["iceberg"]}},
            "policyItems":[],
            "denyPolicyItems":[{"groups":["contractors"],"accesses":[{"type":"select"}]}],
            "allowExceptions":[{"users":["alice"],"accesses":[{"type":"insert"}]}],
            "description":"d","policyLabels":["l1"]
        });
        let s = summarize_policy(1, "trino", true, "operator", &body);
        assert!(s.principals.contains(&"contractors".to_string()));
        assert!(s.principals.contains(&"alice".to_string()));
        assert!(s.permissions.contains(&"select".to_string()));
        assert!(s.permissions.contains(&"insert".to_string()));
        assert_eq!(s.description, "d");
        assert_eq!(s.labels, vec!["l1".to_string()]);
    }

    #[test]
    fn serializes_override_priority() {
        let mut inp = sample();
        inp.is_override = true;
        let body = access_to_ranger_json(&inp);
        assert_eq!(body["policyPriority"], 1);

        let default_body = access_to_ranger_json(&sample());
        assert_eq!(default_body["policyPriority"], 0);
    }

    #[test]
    fn serializes_impersonation_as_generic_resource() {
        let inp: AccessPolicyInput = serde_json::from_value(json!({
            "name": "allow-alice-impersonate-bob",
            "service": "trino",
            "isEnabled": true,
            "resources": {"trinouser": {"values": ["bob"]}},
            "items": [
                {"users": ["alice"], "accesses": ["impersonate"], "effect": "allow"}
            ]
        }))
        .unwrap();
        let body = access_to_ranger_json(&inp);
        assert_eq!(body["policyType"], 0);
        assert_eq!(body["resources"]["trinouser"]["values"][0], "bob");
        assert!(body["resources"].get("catalog").is_none());
        assert_eq!(body["policyItems"][0]["accesses"][0]["type"], "impersonate");
    }

    #[test]
    fn serializes_arbitrary_non_table_resource_type() {
        // Any resource-type name works generically — no special-casing needed.
        let inp: AccessPolicyInput = serde_json::from_value(json!({
            "name": "allow-role-admin",
            "service": "trino",
            "isEnabled": true,
            "resources": {"role": {"values": ["*"]}},
            "items": [
                {"users": ["alice"], "accesses": ["alter"], "effect": "allow"}
            ]
        }))
        .unwrap();
        let body = access_to_ranger_json(&inp);
        assert_eq!(body["resources"]["role"]["values"][0], "*");
        assert!(body["resources"].get("catalog").is_none());
        assert!(body["resources"].get("schema").is_none());
        assert!(body["resources"].get("table").is_none());
        assert!(body["resources"].get("column").is_none());
    }

    #[test]
    fn table_resources_still_serialize_when_no_other_type_present() {
        let body = access_to_ranger_json(&sample());
        assert!(body["resources"].get("catalog").is_some());
        assert!(body["resources"].get("trinouser").is_none());
    }

    #[test]
    fn serializes_additional_resources_and_roles() {
        let inp: AccessPolicyInput = serde_json::from_value(json!({
            "kind":"access","name":"p","service":"trino","isEnabled":true,
            "resources":{"catalog":{"values":["iceberg"]},"schema":{"values":["sales"]}},
            "additional":[{"catalog":{"values":["hive"]},"schema":{"values":["reports"]},"table":{"values":["daily"]}}],
            "items":[{"users":["alice"],"groups":["g1"],"roles":["analyst_role"],"accesses":["select"],"effect":"allow"}]
        })).unwrap();
        let b = access_to_ranger_json(&inp);
        assert_eq!(b["resources"]["catalog"]["values"][0], "iceberg");
        let add = b["additionalResources"].as_array().unwrap();
        assert_eq!(add.len(), 1);
        assert_eq!(add[0]["catalog"]["values"][0], "hive");
        assert_eq!(add[0]["table"]["values"][0], "daily");
        assert_eq!(b["policyItems"][0]["roles"][0], "analyst_role");
    }

    #[test]
    fn dispatcher_routes_by_kind() {
        let acc = editor_to_ranger_json(
            &json!({"kind":"access","name":"a","service":"trino",
            "resources":{"catalog":{"values":["c"]}},"items":[]}),
            "trino",
        )
        .unwrap();
        assert_eq!(acc.body["policyType"], 0);
        assert_eq!(acc.name, "a");
        let m = editor_to_ranger_json(&serde_json::to_value(masking_sample()).unwrap(), "trino")
            .unwrap();
        assert_eq!(m.body["policyType"], 1);
        let d = editor_to_ranger_json(
            &json!({"name":"x","service":"trino","resources":{},"items":[]}),
            "trino",
        )
        .unwrap();
        assert_eq!(d.body["policyType"], 0);
    }

    #[test]
    fn editor_stamps_starrocks_service_type() {
        let pol = editor_to_ranger_json(
            &json!({
                "kind": "masking", "name": "m", "service": "sr1", "isEnabled": true,
                "resources": { "catalog": { "values": ["ice"] },
                               "database": { "values": ["sales"] },
                               "table": { "values": ["orders"] },
                               "column": { "values": ["phone"] } },
                "items": [{ "users": ["u"], "groups": [], "maskType": "MASK_HASH" }]
            }),
            "starrocks",
        )
        .unwrap();
        assert_eq!(pol.body["serviceType"], "starrocks");
        // StarRocks uses `database`, not Trino's `schema`.
        assert!(pol.body["resources"]["database"].is_object());
        assert_eq!(
            pol.body["dataMaskPolicyItems"][0]["dataMaskInfo"]["dataMaskType"],
            "MASK_HASH"
        );
    }

    // ── Tag-based policies (Access P3.2) ─────────────────────────────────────

    #[test]
    fn tag_masking_body_targets_tag_service() {
        let inp: MaskingPolicyInput = serde_json::from_value(json!({
            "kind": "tag-masking", "name": "mask-pii", "service": "eng", "tag": "pii",
            "isEnabled": true,
            "items": [{"users": ["alice"], "groups": ["pii"], "maskType": "MASK_HASH"}]
        }))
        .unwrap();
        let body = tag_masking_to_ranger_json(&inp, "pii", "eng_tag");
        assert_eq!(body["serviceType"], "tag");
        assert_eq!(body["service"], "eng_tag");
        assert_eq!(body["policyType"], 1);
        assert_eq!(body["resources"]["tag"]["values"][0], "pii");
        assert_eq!(body["resources"]["tag"]["isRecursive"], false);
        // No catalog/schema/table/column selectors on a tag policy.
        assert!(body["resources"].get("catalog").is_none());
        assert!(body.get("additionalResources").is_none());
        let items = body["dataMaskPolicyItems"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["dataMaskInfo"]["dataMaskType"], "MASK_HASH");
        assert_eq!(items[0]["users"][0], "alice");
    }

    #[test]
    fn tag_access_body_has_policy_items() {
        let inp: AccessPolicyInput = serde_json::from_value(json!({
            "kind": "tag-access", "name": "allow-pii-read", "service": "eng", "tag": "pii",
            "isEnabled": true,
            "items": [{"groups": ["analysts"], "accesses": ["select"], "effect": "allow"}]
        }))
        .unwrap();
        let body = tag_access_to_ranger_json(&inp, "pii", "eng_tag");
        assert_eq!(body["serviceType"], "tag");
        assert_eq!(body["policyType"], 0);
        assert_eq!(body["resources"]["tag"]["values"][0], "pii");
        let items = body["policyItems"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["groups"][0], "analysts");
        assert_eq!(items[0]["accesses"][0]["type"], "select");
    }

    #[test]
    fn tag_rowfilter_body_has_row_filter_info() {
        let inp: RowFilterPolicyInput = serde_json::from_value(json!({
            "kind": "tag-rowfilter", "name": "pii-us-only", "service": "eng", "tag": "pii",
            "isEnabled": true,
            "items": [{"groups": ["analysts"], "filterExpr": "country = 'US'"}]
        }))
        .unwrap();
        let body = tag_rowfilter_to_ranger_json(&inp, "pii", "eng_tag");
        assert_eq!(body["serviceType"], "tag");
        assert_eq!(body["policyType"], 2);
        assert_eq!(body["resources"]["tag"]["values"][0], "pii");
        let items = body["rowFilterPolicyItems"].as_array().unwrap();
        assert_eq!(items[0]["rowFilterInfo"]["filterExpr"], "country = 'US'");
    }

    #[test]
    fn tag_dispatcher_routes_by_kind_and_requires_tag() {
        let m = tag_editor_to_ranger_json(
            &json!({"kind":"tag-masking","name":"m","service":"eng","tag":"pii",
                    "items":[{"groups":["g"],"maskType":"MASK_NULL"}]}),
            "eng_tag",
        )
        .unwrap();
        assert_eq!(m.body["policyType"], 1);
        assert_eq!(m.body["serviceType"], "tag");
        assert_eq!(m.service, "eng_tag");
        assert_eq!(m.name, "m");

        assert!(is_tag_kind(&json!({"kind":"tag-access"})));
        assert!(!is_tag_kind(&json!({"kind":"masking"})));

        // Missing/empty `tag` is a bad request.
        assert!(tag_editor_to_ranger_json(
            &json!({"kind":"tag-access","name":"x","service":"eng","items":[]}),
            "eng_tag"
        )
        .is_err());
    }

    #[test]
    fn tag_policy_summary_surfaces_tag_resource() {
        let inp: MaskingPolicyInput = serde_json::from_value(json!({
            "kind": "tag-masking", "name": "mask-pii", "service": "eng", "tag": "pii",
            "isEnabled": true,
            "items": [{"groups": ["pii"], "maskType": "MASK_HASH"}]
        }))
        .unwrap();
        let body = tag_masking_to_ranger_json(&inp, "pii", "eng_tag");
        let s = summarize_policy(9, "eng_tag", true, "operator", &body);
        assert_eq!(s.policy_type, "masking");
        assert_eq!(s.resource_path, "tag: pii");
        assert!(s.principals.contains(&"pii".to_string()));
    }

    // ── SP4d-4 Task 11 (B1) ────────────────────────────────────────────────

    fn lvl(values: &[&str]) -> Value {
        json!({ "values": values, "isExcludes": false, "isRecursive": false })
    }

    fn form(resources: Value) -> Value {
        json!({ "name": "p", "service": "s", "resources": resources,
                "items": [{"users": ["alice"], "accesses": ["select"]}] })
    }

    /// The compiler's serializers share `resources_object`, which still DROPS
    /// an empty level: the fill belongs to the editor path only, so a compiled
    /// body is byte-for-byte what it was.
    #[test]
    fn the_shared_serializer_still_drops_an_empty_level() {
        let inp: AccessPolicyInput = serde_json::from_value(form(json!({
            "catalog": lvl(&["c"]), "schema": lvl(&["s"]),
            "table": lvl(&["t"]), "column": lvl(&[])
        })))
        .unwrap();
        let direct = access_to_ranger_json(&inp);
        assert!(
            direct["resources"].get("column").is_none(),
            "the serializer is unchanged: {direct}"
        );
        let editor = editor_to_ranger_json(
            &form(json!({
                "catalog": lvl(&["c"]), "schema": lvl(&["s"]),
                "table": lvl(&["t"]), "column": lvl(&[])
            })),
            "trino",
        )
        .unwrap();
        assert_eq!(editor.body["resources"]["column"], lvl(&["*"]));
        assert_eq!(editor.filled.as_ref().unwrap().names, ["column"]);
        assert_eq!(
            editor.filled.as_ref().unwrap().unfilled_body["resources"],
            direct["resources"],
            "the recorded unfilled body is what the serializer renders"
        );
    }

    #[test]
    fn starrocks_names_its_second_level_database() {
        let pol = editor_to_ranger_json(
            &form(json!({"catalog": lvl(&["c"]), "database": lvl(&[])})),
            "starrocks",
        )
        .unwrap();
        assert_eq!(pol.body["resources"]["database"], lvl(&["*"]));
        assert_eq!(pol.filled.unwrap().names, ["database"]);
    }

    #[test]
    fn a_row_filter_form_is_filled_only_down_to_table() {
        let rf = json!({
            "kind": "rowfilter", "name": "r", "service": "s",
            "resources": {"catalog": lvl(&["c"]), "schema": lvl(&[]), "table": lvl(&["t"]),
                          "column": lvl(&[])},
            "items": [{"groups": ["public"], "filterExpr": "true"}]
        });
        let pol = editor_to_ranger_json(&rf, "trino").unwrap();
        assert_eq!(pol.body["resources"]["schema"], lvl(&["*"]));
        assert!(
            pol.body["resources"].get("column").is_none(),
            "a row filter has no column level: {}",
            pol.body
        );
    }

    #[test]
    fn an_empty_level_under_an_absent_one_is_dropped_not_filled() {
        // `table` removed ("— none —") with a stale empty `column` key below it.
        let pol = editor_to_ranger_json(
            &form(json!({"catalog": lvl(&["c"]), "schema": lvl(&["s"]), "column": lvl(&[])})),
            "trino",
        )
        .unwrap();
        assert!(
            pol.body["resources"].get("column").is_none(),
            "{}",
            pol.body
        );
        assert!(pol.filled.is_none());
    }

    #[test]
    fn additional_resources_are_normalised_too() {
        let mut f = form(json!({"catalog": lvl(&["c"]), "schema": lvl(&["s"])}));
        f["additional"] = json!([{"catalog": lvl(&["d"]), "schema": lvl(&[])}]);
        let pol = editor_to_ranger_json(&f, "trino").unwrap();
        assert_eq!(pol.body["additionalResources"][0]["schema"], lvl(&["*"]));
    }
}
