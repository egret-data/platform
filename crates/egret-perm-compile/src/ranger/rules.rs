//! Renders a rule's [`Statement`]s (masks and row filters) into Ranger
//! masking/row-filter policy bodies (governance sub-project 1, Task 6). Every
//! decision — which selector lowers to what, the expansion ceiling, every
//! refusal — is `egret_perm_ir::lower::rules`'s; this module renders shape
//! only. Read that module's doc for the D5 asymmetry and the
//! everyone-except-the-exempt audience this module realizes as Ranger's
//! `public`-group item pair.
//!
//! **`serviceType` is stamped explicitly for StarRocks.**
//! `masking_to_ranger_json`/`rowfilter_to_ranger_json` (`ranger_json.rs`)
//! hardcode `"serviceType": "trino"` unconditionally — a hazard Task 4 found
//! and left for this task. `stamp_service_type` overwrites it to
//! `"starrocks"` the same way `editor_to_ranger_json` already does for the
//! editor path; a StarRocks-targeted body rendered without this fix would
//! deploy cleanly and enforce nothing. Tag policies need no such fix:
//! `tagify` (inside `tag_masking_to_ranger_json`/`tag_rowfilter_to_ranger_json`)
//! always sets `serviceType: "tag"`, independent of the resource engine.
//!
//! **Verified live (Task 10, 2026-09-24, kind `kind-egret`).** The
//! `public`-group idiom above is no longer merely documented: a path-selected
//! mask compiled exactly this way returned the literal string `xxxx` in place
//! of the real values, and a path-selected row filter cut 4 rows to 2 with the
//! excluded rows probed genuinely absent
//! (`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3, cases
//! T3b and T4). The OTHER assumption that paragraph used to sit next to — the
//! resource-hierarchy ANCESTOR match `grants.rs` cited from upstream Ranger
//! source — was measured in the same run and **refuted** for access policies;
//! see `ranger::grants::scope_to_resources`. The masking and row-filter resource maps
//! this module emits are the ones that were measured working, and are
//! deliberately left as they are.
//!
//! **Where a rule's compiled policy is STORED is not this module's choice and
//! never was.** A tag body's own `service` field names the profile's TAG
//! service (`tagify`, `egret-policy/src/admin/ranger_json.rs`), a path body's
//! names the resource profile, and `trigger::apply_diff_in_tx` routes each row
//! by that field. Task 10 found the driver ignoring it and filing every tag
//! policy under the resource profile's `service_id`, where the tag plugin
//! never sees it (live-result §3, case T3: a mask that returned `book`, `pen`,
//! `lamp`, `desk` — the real values).
//!

use std::collections::BTreeMap;

use serde_json::Value;

use egret_policy::admin::{
    masking_to_ranger_json, rowfilter_to_ranger_json, tag_masking_to_ranger_json,
    tag_rowfilter_to_ranger_json, MaskItemInput, MaskingPolicyInput, ResourceLevel,
    RowFilterItemInput, RowFilterPolicyInput,
};
use egret_policy::{starrocks_service_def, trino_service_def, COMPILER_RULE_PREFIX};

use egret_perm::{CompileError, EngineType, Scope};
use egret_perm_ir::audience::Audience;
use egret_perm_ir::lower::rules::TaggedColumn;
use egret_perm_ir::statement::{Derivation, Effect, Level, Source, Statement, Target};

use super::grants::{
    level, member_buckets, resource_level_names, scope_to_resources, PolicyTarget,
    ROW_FILTER_LEVELS,
};

// `egret_perm_ir::lower::rules::ensure_row_filter_scope_is_a_table` spells out
// catalog + schema + table; a different row-filter depth needs it rewritten.
const _: () = assert!(
    ROW_FILTER_LEVELS == 3,
    "ensure_row_filter_scope_is_a_table spells out catalog+schema+table; a different row-filter depth needs it rewritten"
);

/// Render one rule statement for Ranger service `service` on engine `e`.
///
/// The policy name and resources follow from the statement's derivation and
/// target:
///
/// - `AsDeclared` on a path: `perm-rule-{id}` at the rule's own scope
///   (`scope_to_resources`; a masking policy and a row-filter policy are
///   matched against DIFFERENT declared hierarchies — `dataMaskDef.resources`
///   goes down to `column`, `rowFilterDef.resources` stops at `table` — so
///   which one this is travels with the call rather than being inferred later).
/// - `AsDeclared` on a classification: one tag policy, `perm-rule-{id}`, filed
///   under `tag_service` ([`build_tag_body`]).
/// - `ExpandedFromClassification` on a path naming a column:
///   `perm-rule-{id}-{catalog}.{schema}.{table}.{column}` ([`column_resources`]).
/// - `ExpandedFromClassification` on a path naming no column: one table's row
///   filter ([`table_row_filter_name`], [`table_resources`]).
///
/// Anything else is refused with `CompileError::Db`: no lowering produces it.
pub(crate) fn render(
    s: &Statement,
    service: &str,
    tag_service: Option<&str>,
    e: EngineType,
) -> Result<Value, CompileError> {
    let Source::Rule { rule_id, rule_name } = &s.source else {
        return Err(CompileError::Db(format!(
            "ranger::rules::render handed a non-rule statement ({:?})",
            s.source
        )));
    };
    let target = match &s.effect {
        Effect::Mask { .. } => PolicyTarget::Mask,
        Effect::RowFilter { .. } => PolicyTarget::RowFilter,
        other => {
            return Err(CompileError::Db(format!(
                "ranger::rules::render handed a non-rule effect ({other:?})"
            )))
        }
    };
    let named = |l: &Level| match l {
        Level::Named(v) => Some(v.clone()),
        Level::Unnamed => None,
    };
    match (&s.derivation, &s.target) {
        (Derivation::AsDeclared, Target::Path(path)) => {
            let scope = Scope {
                catalog_id: path.catalog_id.unwrap_or_default(),
                schema: named(&path.schema),
                table: named(&path.table),
                column: named(&path.column),
            };
            let resources = scope_to_resources(&scope, &path.catalog, e, target);
            let name = format!("{COMPILER_RULE_PREFIX}{rule_id}");
            build_body(s, e, service, name, resources)
        }
        (Derivation::AsDeclared, Target::Classification { name }) => {
            let svc = tag_service.ok_or_else(|| CompileError::MissingTagService {
                rule: rule_name.clone(),
            })?;
            build_tag_body(s, *rule_id, service, name, svc)
        }
        (Derivation::ExpandedFromClassification, Target::Path(path)) => {
            let (Some(schema), Some(table)) = (named(&path.schema), named(&path.table)) else {
                return Err(CompileError::Db(format!(
                    "ranger::rules::render handed an expansion with an unnamed level ({path:?})"
                )));
            };
            match named(&path.column) {
                Some(column) => {
                    let col = TaggedColumn {
                        catalog: path.catalog.clone(),
                        schema,
                        table,
                        column,
                    };
                    let name = format!(
                        "{COMPILER_RULE_PREFIX}{rule_id}-{}.{}.{}.{}",
                        col.catalog, col.schema, col.table, col.column
                    );
                    let resources = column_resources(e, &col, target);
                    build_body(s, e, service, name, resources)
                }
                None => {
                    let name = table_row_filter_name(*rule_id, &path.catalog, &schema, &table);
                    let resources = table_resources(e, &path.catalog, &schema, &table);
                    build_body(s, e, service, name, resources)
                }
            }
        }
        (d, t) => Err(CompileError::Db(format!(
            "ranger::rules::render handed a {d:?} statement on {t:?}"
        ))),
    }
}

/// The Ranger `resources` map for one fully-resolved column (StarRocks tag
/// expansion — every level is always known here, unlike a rule's own
/// possibly-partial `Scope`).
///
/// **It branches on `target` for the same reason `scope_to_resources` does,
/// and did not (whole-branch review, Critical 1).** This function emitted all
/// four levels unconditionally, ignoring the rule's `kind`, while its sibling
/// already consulted `PolicyTarget` — so a tag-selected ROW FILTER on
/// StarRocks compiled a body carrying a `column` resource that
/// `rowFilterDef.resources` does not declare. Ranger accepted it, egret listed
/// it as live, and it filtered no rows at all. A mask keeps all four levels
/// (`dataMaskDef` declares `column`); a row filter stops at `table`
/// ([`ROW_FILTER_LEVELS`]).
///
/// `pub(crate)`: SP4c Task 2d's per-column CLEARANCE expansion addresses
/// exactly the same resource, at `PolicyTarget::Access`. Reusing this rather
/// than building a second column-resource map is deliberate — the two would
/// otherwise be free to disagree about a level name or a depth, and a clearance
/// addressing a resource the mask does not is a deny that enforces nothing.
pub(crate) fn column_resources(
    e: EngineType,
    col: &TaggedColumn,
    target: PolicyTarget,
) -> BTreeMap<String, ResourceLevel> {
    let names = resource_level_names(e);
    let values = [&col.catalog, &col.schema, &col.table, &col.column];
    let depth = match target {
        PolicyTarget::RowFilter => ROW_FILTER_LEVELS,
        PolicyTarget::Access | PolicyTarget::Mask => names.len(),
    };
    let mut map = BTreeMap::new();
    for i in 0..depth {
        map.insert(names[i].to_string(), level(values[i].clone()));
    }
    map
}

/// The row-filter `resources` map for one table: the [`ROW_FILTER_LEVELS`]
/// levels `rowFilterDef` declares, never a column.
fn table_resources(
    e: EngineType,
    catalog: &str,
    schema: &str,
    table: &str,
) -> BTreeMap<String, ResourceLevel> {
    let names = resource_level_names(e);
    let values = [catalog, schema, table];
    let mut map = BTreeMap::new();
    for i in 0..ROW_FILTER_LEVELS {
        map.insert(names[i].to_string(), level(values[i].to_string()));
    }
    map
}

/// The name of a tag row filter's policy on one table, on either engine.
fn table_row_filter_name(rule_id: uuid::Uuid, catalog: &str, schema: &str, table: &str) -> String {
    format!("{COMPILER_RULE_PREFIX}{rule_id}-{catalog}.{schema}.{table}")
}

/// Build one non-tag (path-addressed) masking/row-filter body and stamp the
/// target engine's `serviceType` onto it.
fn build_body(
    s: &Statement,
    e: EngineType,
    service: &str,
    name: String,
    resources: BTreeMap<String, ResourceLevel>,
) -> Result<Value, CompileError> {
    let body = match &s.effect {
        Effect::Mask {
            mask_type,
            expression,
        } => {
            let input = MaskingPolicyInput {
                kind: "masking".to_string(),
                name,
                service: service.to_string(),
                is_enabled: true,
                resources,
                items: mask_items(&s.audience, mask_type, expression)?,
                description: String::new(),
                labels: Vec::new(),
                is_override: false,
            };
            masking_to_ranger_json(&input)
        }
        Effect::RowFilter { expression } => {
            let input = RowFilterPolicyInput {
                kind: "rowfilter".to_string(),
                name,
                service: service.to_string(),
                is_enabled: true,
                resources,
                items: rowfilter_items(&s.audience, expression)?,
                description: String::new(),
                labels: Vec::new(),
                is_override: false,
            };
            rowfilter_to_ranger_json(&input)
        }
        other => {
            return Err(CompileError::Db(format!(
                "ranger::rules::build_body handed a non-rule effect ({other:?})"
            )))
        }
    };
    Ok(stamp_service_type(body, e))
}

/// Build one tag-addressed masking/row-filter body (`tagify` inside the
/// `tag_*_to_ranger_json` builders sets `serviceType: "tag"` and the `tag`
/// resource itself; no per-engine stamping is needed here).
fn build_tag_body(
    s: &Statement,
    rule_id: uuid::Uuid,
    service: &str,
    tag_key: &str,
    tag_service: &str,
) -> Result<Value, CompileError> {
    let name = format!("{COMPILER_RULE_PREFIX}{rule_id}");
    let body = match &s.effect {
        Effect::Mask {
            mask_type,
            expression,
        } => {
            let input = MaskingPolicyInput {
                kind: "tag-masking".to_string(),
                name,
                service: service.to_string(),
                is_enabled: true,
                resources: BTreeMap::new(),
                items: mask_items(&s.audience, mask_type, expression)?,
                description: String::new(),
                labels: Vec::new(),
                is_override: false,
            };
            tag_masking_to_ranger_json(&input, tag_key, tag_service)
        }
        Effect::RowFilter { expression } => {
            let input = RowFilterPolicyInput {
                kind: "tag-rowfilter".to_string(),
                name,
                service: service.to_string(),
                is_enabled: true,
                resources: BTreeMap::new(),
                items: rowfilter_items(&s.audience, expression)?,
                description: String::new(),
                labels: Vec::new(),
                is_override: false,
            };
            tag_rowfilter_to_ranger_json(&input, tag_key, tag_service)
        }
        other => {
            return Err(CompileError::Db(format!(
                "ranger::rules::build_tag_body handed a non-rule effect ({other:?})"
            )))
        }
    };
    Ok(body)
}

/// `masking_to_ranger_json`/`rowfilter_to_ranger_json` hardcode
/// `"serviceType": "trino"` (a hazard Task 4 found and left for this task —
/// see the module doc comment). Overwrite it for StarRocks, exactly as
/// `editor_to_ranger_json` already does for the hand-authored editor path.
///
/// `pub(crate)`: `access_to_ranger_json` (`ranger::grants`' renderer, invoked from
/// `egret-perm-compile`'s recompile driver — Task 7) hardcodes the SAME
/// `"serviceType": "trino"` for exactly the same reason and was never fixed
/// for a StarRocks-targeted access policy, since nothing outside this crate's
/// tests called `compile_grant` before Task 7. The driver reuses this exact
/// function rather than duplicating the stamp.
pub(crate) fn stamp_service_type(mut body: Value, e: EngineType) -> Value {
    if e == EngineType::StarRocks {
        if let Value::Object(m) = &mut body {
            m.insert("serviceType".into(), Value::from("starrocks"));
        }
    }
    body
}

/// The Ranger `dataMaskType` names `e`'s service-def declares under
/// `dataMaskDef.maskTypes`. Read at call time from the embedded service-def
/// JSON (no I/O — `trino_service_def`/`starrocks_service_def` parse a
/// `include_str!`), not trusted from a table, because a name the plugin does
/// not know is silently ignored: the policy compiles, deploys, and masks
/// nothing. Returns `CompileError::ServiceDefUnreadable` rather than
/// panicking if the embedded JSON somehow fails to parse — every other
/// failure in this module is a `CompileError`, not a `.expect()`.
///
/// `pub(crate)` since SP4d-3 Task 4: `crate::grammar` refuses a hand-written
/// mask whose type the engine does not declare by reading THIS list, so the
/// compiler and its inverse cannot disagree about which names are masks.
pub(crate) fn declared_mask_types(e: EngineType) -> Result<Vec<String>, CompileError> {
    let def = match e {
        EngineType::Trino => trino_service_def(),
        EngineType::StarRocks => starrocks_service_def(),
    }
    .map_err(|err| CompileError::ServiceDefUnreadable {
        engine: e,
        detail: err.to_string(),
    })?;
    Ok(def["dataMaskDef"]["maskTypes"]
        .as_array()
        .map(|types| {
            types
                .iter()
                .filter_map(|t| t["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// The literal group ref the compiler itself uses to mean "everyone" (see
/// `mask_items`/`rowfilter_items`). An exempt principal naming this SAME
/// group would carry `MASK_NONE`/an empty filter for "everyone", exempting
/// every principal from the rule — the one fail-open this exempt design
/// otherwise avoids by refusing everything it cannot express correctly.
// `pub(crate)`: SP4c's clearance renderer (`crate::ranger::withhold`) puts this SAME
// literal on the deny side of a tag policy to mean "everybody", which is the
// half of Ranger's deny/deny-exception pair D0 measured IS expressible. Two
// copies of this string would drift, and a drift makes a control that withholds
// nothing.
pub(crate) const PUBLIC_GROUP: &str = egret_perm::PUBLIC_GROUP;

/// The mask items for a rule's `audience`: an exempt item first (the
/// `except` principals, `MASK_NONE`) when it names any, then the general item
/// for `include` (the `public` group) carrying the rule's real mask. See
/// `egret_perm_ir::lower::rules`' module doc for the Ranger idiom this follows.
fn mask_items(
    audience: &Audience,
    mask_type: &str,
    value_expr: &Option<String>,
) -> Result<Vec<MaskItemInput>, CompileError> {
    let (users, groups) = member_buckets(&audience.except)?;
    let mut items = Vec::new();
    if !users.is_empty() || !groups.is_empty() {
        items.push(MaskItemInput {
            users,
            groups,
            mask_type: "MASK_NONE".to_string(),
            value_expr: None,
        });
    }
    let (users, groups) = member_buckets(&audience.include)?;
    items.push(MaskItemInput {
        users,
        groups,
        mask_type: mask_type.to_string(),
        value_expr: value_expr.clone(),
    });
    Ok(items)
}

/// The row-filter items for a rule's `audience`: an exempt item first (the
/// `except` principals, an empty `filterExpr` — "no filtering", the same idiom
/// the cited Ranger wiki page uses), then the general item for `include` (the
/// `public` group) carrying the rule's real filter.
fn rowfilter_items(
    audience: &Audience,
    filter_expr: &str,
) -> Result<Vec<RowFilterItemInput>, CompileError> {
    let (users, groups) = member_buckets(&audience.except)?;
    let mut items = Vec::new();
    if !users.is_empty() || !groups.is_empty() {
        items.push(RowFilterItemInput {
            users,
            groups,
            filter_expr: String::new(),
        });
    }
    let (users, groups) = member_buckets(&audience.include)?;
    items.push(RowFilterItemInput {
        users,
        groups,
        filter_expr: filter_expr.to_string(),
    });
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M19: the matrix's mask types are exactly the ones each embedded
    /// service-def declares, in its order, and nothing else is native.
    #[test]
    fn matrix_mask_types_equal_the_service_def() {
        use egret_perm_ir::matrix::{
            support_for_kind, Capability, EpKind, Mechanism, Support,
            STARROCKS_MASK_MIN_EGRET_IMAGE, STARROCKS_MASK_TYPES, TRINO_MASK_TYPES,
        };
        for e in [EngineType::Trino, EngineType::StarRocks] {
            let k = EpKind {
                engine: e,
                mechanism: Mechanism::Ranger,
            };
            let declared = declared_mask_types(e).unwrap();
            assert!(!declared.is_empty(), "{e:?} declares no mask type");
            for t in &declared {
                assert_eq!(
                    support_for_kind(
                        k,
                        None,
                        Some(STARROCKS_MASK_MIN_EGRET_IMAGE),
                        true,
                        &Capability::MaskType(t.clone())
                    ),
                    Support::Native,
                    "{e:?} {t}"
                );
            }
            let matrix = match e {
                EngineType::Trino => TRINO_MASK_TYPES,
                EngineType::StarRocks => STARROCKS_MASK_TYPES,
            };
            assert_eq!(matrix, declared.as_slice(), "{e:?}");
            assert_ne!(
                support_for_kind(
                    k,
                    None,
                    Some(STARROCKS_MASK_MIN_EGRET_IMAGE),
                    true,
                    &Capability::MaskType("MASK_NOT_A_TYPE".into())
                ),
                Support::Native
            );
        }
    }
}
