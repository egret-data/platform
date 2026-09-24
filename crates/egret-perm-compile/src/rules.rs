//! `compile_rule`: turns one `perm.rules` row (plus its exempt set) into the
//! Ranger masking/row-filter JSON bodies for one engine — the second half of
//! the compiler (`grants.rs` is the first).
//!
//! **The asymmetry this file exists for (D5).** Trino's Ranger plugin
//! resolves tags itself, so a tag-selected rule compiles to ONE tag-masking
//! (or tag-rowfilter) policy on Trino, covering however many columns
//! currently carry the tag. StarRocks' plugin has no tag service (upstream
//! `starrocks#67458`), so the same rule compiles to ONE policy PER matching
//! column on StarRocks, enumerated at compile time from `tagged` — a list
//! this compiler does not resolve itself (no registry/tag I/O here; the
//! caller has already looked up which columns carry the rule's tag). That
//! enumeration is stale the moment a new column gets the tag; a later task
//! recompiles on tag writes.
//!
//! **The ceiling is a refusal, never a truncation.** Above `ceiling`,
//! `compile_rule` returns `CompileError::ExpansionCeiling` naming the rule
//! and the count, on the StarRocks-tag path only — Trino's tag policy is
//! O(1) regardless of how many columns carry the tag, so the ceiling does
//! not apply there.
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
//! **A rule's `exempt` set (D2: "how an individual escapes a mask — not a
//! competing grant") compiles to a specific-then-general item pair**, the
//! documented Apache Ranger idiom for "everyone, except these people" on a
//! masking/row-filter policy: a policy item scoped to the special `public`
//! group applies to every principal, and a more specific user/group item
//! ahead of it is evaluated first and wins for whoever it names
//! ([Row-level filtering and column-masking using Apache Ranger policies in
//! Apache Hive](https://cwiki.apache.org/confluence/pages/viewpage.action?pageId=65868896) —
//! Use Case #2 uses `group=public` for "applies to all users", Use Case #4
//! inserts a user-specific item with an empty filter to override it for one
//! user; the same page is cited in the design spec for this feature). The
//! exempt item uses `MASK_NONE` (masking) or an empty `filterExpr` (row
//! filter) — `MASK_NONE` is a declared type in every service-def this
//! compiler targets. `exempt.exclude` and a `Role` principal in `exempt`
//! cannot be expressed this way (see `CompileError::UnexpressibleExemption`)
//! and refuse rather than silently mis-compile.
//!
//! **Not yet independently verified**: the `public`-group idiom above, like
//! the resource-hierarchy ANCESTOR match `grants.rs` cites from upstream
//! Ranger source, is grounded in Apache's own documentation but has not been
//! checked against the live plugin from this worktree. Flagged as a live
//! check alongside the ones `grants.rs` and its round-1 fix already named.
//!
//! **`ResolvedNames` (added in the round-1 review fix, Criticals 1 and 2).**
//! The first version of `compile_rule` took no resolved name at all: a tag
//! selector stamped `Selector::Tag`'s `key` straight into the tag policy, and
//! a path selector stamped `scope.catalog_id`'s raw UUID as the catalog
//! resource. Both deploy cleanly, are listed by Ranger, and match no
//! resource — egret's own tag download pushes `tags.name` (a separate,
//! mutable column from `tags.key`;
//! `egret-policy/src/store.rs:245` → `egret-policy-api/src/dto.rs:184`;
//! `migrations/0080_tags.sql`), and every engine identifies a catalog by
//! name, never by id. Spec D6 ("resolves key → name and id → name at compile
//! time") named both halves; the original signature carried neither. The
//! compiler still does no I/O — `ResolvedNames` is caller-resolved, and a
//! tag selector with `tag_name: None` refuses rather than falling back to
//! the key.

use std::collections::BTreeMap;

use serde_json::Value;

use egret_policy::admin::{
    masking_to_ranger_json, rowfilter_to_ranger_json, tag_masking_to_ranger_json,
    tag_rowfilter_to_ranger_json, MaskItemInput, MaskingPolicyInput, ResourceLevel,
    RowFilterItemInput, RowFilterPolicyInput,
};
use egret_policy::{starrocks_service_def, trino_service_def};

use egret_perm::{CompileError, EngineType, PrincipalKind, PrincipalSet, Rule, RuleKind, Selector};

use crate::grants::{level, resource_level_names, scope_to_resources, validate_user_ref};

/// One column carrying the tag a rule selects on, resolved by the CALLER —
/// this compiler does no I/O and cannot itself know which columns carry
/// which `tags.key` (D6). Every field is the Ranger-facing NAME the engine
/// expects, not an id, since these values are stamped directly into a
/// policy's `resources`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedColumn {
    pub catalog: String,
    pub schema: String,
    pub table: String,
    pub column: String,
}

/// The engine-facing names `compile_rule` cannot resolve itself (no registry
/// or tag I/O in this compiler). `catalog_name` is used only for a `Path`
/// selector (`compile_grant`'s equivalent parameter); `tag_name` is used
/// only for a `Tag` selector, and MUST be the tag's `tags.name` — never
/// `tags.key`, which is stable but not what the Ranger tag plugin matches on
/// (D6). A `Tag`-selector rule with `tag_name: None` is a `CompileError`
/// (`MissingTagName`), never a silent fallback to the key.
pub struct ResolvedNames<'a> {
    pub catalog_name: &'a str,
    pub tag_name: Option<&'a str>,
}

/// Compile one `Rule` into the Ranger masking/row-filter JSON bodies for
/// engine `e`.
///
/// - A tag selector on Trino compiles to exactly one tag policy, covering
///   whatever the plugin currently resolves the tag to (D5) —
///   `tag_service` and `resolved.tag_name` must both be `Some`.
/// - A tag selector on StarRocks compiles to one policy per entry in
///   `tagged`, refusing with `CompileError::ExpansionCeiling` if that would
///   exceed `ceiling` (`resolved.tag_name` must still be `Some` — see the
///   module doc comment).
/// - A path selector compiles to exactly one policy on either engine, at
///   `r`'s own scope with `resolved.catalog_name` as the catalog resource
///   (`tagged`/`ceiling`/`resolved.tag_name` are unused on this path).
///
/// `service` is the target Ranger service name stamped onto the body (for a
/// tag policy, `tagify` overwrites it with `tag_service` regardless of what
/// is passed here).
pub fn compile_rule(
    r: &Rule,
    e: EngineType,
    service: &str,
    tag_service: Option<&str>,
    resolved: &ResolvedNames,
    tagged: &[TaggedColumn],
    ceiling: usize,
) -> Result<Vec<Value>, CompileError> {
    // Validated up front, before any engine/selector branching, so a rule
    // that cannot compile on ANY path is refused the same way regardless of
    // which one is taken.
    if let RuleKind::Mask {
        mask_type,
        value_expr,
    } = &r.kind
    {
        ensure_mask_type_declared(&r.name, mask_type, e)?;
        if mask_type == "CUSTOM" {
            ensure_custom_mask_parses(&r.name, value_expr.as_deref().unwrap_or_default())?;
        }
    }

    match (&r.selector, e) {
        (Selector::Tag { .. }, EngineType::Trino) => {
            let tag_name = resolved
                .tag_name
                .ok_or_else(|| CompileError::MissingTagName {
                    rule: r.name.clone(),
                })?;
            let svc = tag_service.ok_or_else(|| CompileError::MissingTagService {
                rule: r.name.clone(),
            })?;
            Ok(vec![build_tag_body(r, service, tag_name, svc)?])
        }
        (Selector::Tag { .. }, EngineType::StarRocks) => {
            // StarRocks' compiled body never carries the tag itself (each
            // policy is addressed by its own column path — see
            // `column_resources`), but a caller that hasn't resolved the
            // tag name has not done the resolution step D6 requires either,
            // and `tagged` being pre-filtered to the RIGHT tag is exactly
            // the invariant `resolved.tag_name` stands for. Checked here too
            // rather than only on the Trino path, so this branch can't
            // silently proceed on a caller bug the Trino branch would have
            // caught.
            if resolved.tag_name.is_none() {
                return Err(CompileError::MissingTagName {
                    rule: r.name.clone(),
                });
            }
            if tagged.len() > ceiling {
                return Err(CompileError::ExpansionCeiling {
                    rule: r.name.clone(),
                    count: tagged.len(),
                    ceiling,
                });
            }
            tagged
                .iter()
                .map(|col| {
                    let name = format!(
                        "perm-rule-{}-{}.{}.{}.{}",
                        r.id, col.catalog, col.schema, col.table, col.column
                    );
                    let resources = column_resources(e, col);
                    build_body(r, e, service, name, resources)
                })
                .collect()
        }
        (Selector::Path(scope), _) => {
            let resources = scope_to_resources(scope, resolved.catalog_name, e);
            let name = format!("perm-rule-{}", r.id);
            Ok(vec![build_body(r, e, service, name, resources)?])
        }
    }
}

/// The Ranger `resources` map for one fully-resolved column (StarRocks tag
/// expansion — every level is always known here, unlike a rule's own
/// possibly-partial `Scope`).
fn column_resources(e: EngineType, col: &TaggedColumn) -> BTreeMap<String, ResourceLevel> {
    let names = resource_level_names(e);
    let mut map = BTreeMap::new();
    map.insert(names[0].to_string(), level(col.catalog.clone()));
    map.insert(names[1].to_string(), level(col.schema.clone()));
    map.insert(names[2].to_string(), level(col.table.clone()));
    map.insert(names[3].to_string(), level(col.column.clone()));
    map
}

/// Build one non-tag (path-addressed) masking/row-filter body and stamp the
/// target engine's `serviceType` onto it.
fn build_body(
    r: &Rule,
    e: EngineType,
    service: &str,
    name: String,
    resources: BTreeMap<String, ResourceLevel>,
) -> Result<Value, CompileError> {
    let body = match &r.kind {
        RuleKind::Mask {
            mask_type,
            value_expr,
        } => {
            let input = MaskingPolicyInput {
                kind: "masking".to_string(),
                name,
                service: service.to_string(),
                is_enabled: true,
                resources,
                items: mask_items(r, mask_type, value_expr)?,
                description: String::new(),
                labels: Vec::new(),
                is_override: false,
            };
            masking_to_ranger_json(&input)
        }
        RuleKind::RowFilter { filter_expr } => {
            let input = RowFilterPolicyInput {
                kind: "rowfilter".to_string(),
                name,
                service: service.to_string(),
                is_enabled: true,
                resources,
                items: rowfilter_items(r, filter_expr)?,
                description: String::new(),
                labels: Vec::new(),
                is_override: false,
            };
            rowfilter_to_ranger_json(&input)
        }
    };
    Ok(stamp_service_type(body, e))
}

/// Build one tag-addressed masking/row-filter body (`tagify` inside the
/// `tag_*_to_ranger_json` builders sets `serviceType: "tag"` and the `tag`
/// resource itself; no per-engine stamping is needed here).
fn build_tag_body(
    r: &Rule,
    service: &str,
    tag_key: &str,
    tag_service: &str,
) -> Result<Value, CompileError> {
    let name = format!("perm-rule-{}", r.id);
    let body = match &r.kind {
        RuleKind::Mask {
            mask_type,
            value_expr,
        } => {
            let input = MaskingPolicyInput {
                kind: "tag-masking".to_string(),
                name,
                service: service.to_string(),
                is_enabled: true,
                resources: BTreeMap::new(),
                items: mask_items(r, mask_type, value_expr)?,
                description: String::new(),
                labels: Vec::new(),
                is_override: false,
            };
            tag_masking_to_ranger_json(&input, tag_key, tag_service)
        }
        RuleKind::RowFilter { filter_expr } => {
            let input = RowFilterPolicyInput {
                kind: "tag-rowfilter".to_string(),
                name,
                service: service.to_string(),
                is_enabled: true,
                resources: BTreeMap::new(),
                items: rowfilter_items(r, filter_expr)?,
                description: String::new(),
                labels: Vec::new(),
                is_override: false,
            };
            tag_rowfilter_to_ranger_json(&input, tag_key, tag_service)
        }
    };
    Ok(body)
}

/// `masking_to_ranger_json`/`rowfilter_to_ranger_json` hardcode
/// `"serviceType": "trino"` (a hazard Task 4 found and left for this task —
/// see the module doc comment). Overwrite it for StarRocks, exactly as
/// `editor_to_ranger_json` already does for the hand-authored editor path.
fn stamp_service_type(mut body: Value, e: EngineType) -> Value {
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
fn declared_mask_types(e: EngineType) -> Result<Vec<String>, CompileError> {
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

fn ensure_mask_type_declared(
    rule_name: &str,
    mask_type: &str,
    e: EngineType,
) -> Result<(), CompileError> {
    if declared_mask_types(e)?.iter().any(|m| m == mask_type) {
        Ok(())
    } else {
        Err(CompileError::UnsupportedMaskType {
            rule: rule_name.to_string(),
            mask_type: mask_type.to_string(),
            engine: e,
        })
    }
}

/// The smallest check that still catches the obvious broken case: balanced
/// parentheses and the `{col}` substitution token present. Ranger never
/// parses this string itself; it is spliced into the engine's SQL at query
/// time, so a string that fails even this check is refused here rather than
/// failing every query that touches the masked column. A real SQL parser is
/// SP5's — this task does not start one.
fn ensure_custom_mask_parses(rule_name: &str, expr: &str) -> Result<(), CompileError> {
    let mut depth: i32 = 0;
    for c in expr.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err(invalid_custom_mask(rule_name, expr));
                }
            }
            _ => {}
        }
    }
    if depth != 0 || !expr.contains("{col}") {
        return Err(invalid_custom_mask(rule_name, expr));
    }
    Ok(())
}

fn invalid_custom_mask(rule_name: &str, expr: &str) -> CompileError {
    CompileError::InvalidCustomMask {
        rule: rule_name.to_string(),
        expr: expr.to_string(),
    }
}

/// The literal group ref the compiler itself uses to mean "everyone" (see
/// `mask_items`/`rowfilter_items`). An exempt principal naming this SAME
/// group would carry `MASK_NONE`/an empty filter for "everyone", exempting
/// every principal from the rule — the one fail-open this exempt design
/// otherwise avoids by refusing everything it cannot express correctly.
const PUBLIC_GROUP: &str = "public";

/// Bucket a rule's exempt principals into (users, groups) for a
/// masking/row-filter item. Those Ranger item shapes
/// (`MaskItemInput`/`RowFilterItemInput`, `ranger_json.rs`) carry neither a
/// `roles` bucket (unlike `AccessItemInput`) nor an exception/subtract block
/// the way access-policy items do (`allowExceptions`/`denyExceptions`) — so
/// a `Role` principal or a non-empty `exempt.exclude` cannot be compiled
/// here and refuse via `CompileError::UnexpressibleExemption`, and neither
/// can an exempt `Group` principal literally named `"public"` (see
/// `PUBLIC_GROUP`). `WorkspaceMembers` refuses for the same not-yet-expanded
/// reason `compile_grant` refuses it (Task 6;
/// `CompileError::UnexpandedPrincipalKind`).
fn exempt_principals(rule: &Rule) -> Result<(Vec<String>, Vec<String>), CompileError> {
    let exempt: &PrincipalSet = &rule.exempt;
    if !exempt.exclude.is_empty() {
        return Err(CompileError::UnexpressibleExemption {
            rule: rule.name.clone(),
            reason: "exempt.exclude is non-empty, but a masking/row-filter item has no \
                     exception block to subtract from (unlike an access-policy item)"
                .to_string(),
        });
    }
    let mut users = Vec::new();
    let mut groups = Vec::new();
    for p in &exempt.include {
        match p.kind {
            PrincipalKind::User => {
                validate_user_ref(&p.r#ref)?;
                users.push(p.r#ref.clone());
            }
            PrincipalKind::Group => {
                if p.r#ref == PUBLIC_GROUP {
                    return Err(CompileError::UnexpressibleExemption {
                        rule: rule.name.clone(),
                        reason: format!(
                            "an exempt group literally named {PUBLIC_GROUP:?} would exempt \
                             everyone — that name is what this compiler's own \"applies to \
                             everyone\" item already uses"
                        ),
                    });
                }
                groups.push(p.r#ref.clone());
            }
            PrincipalKind::Role => {
                return Err(CompileError::UnexpressibleExemption {
                    rule: rule.name.clone(),
                    reason: "a Role principal has no roles bucket on a masking/row-filter item"
                        .to_string(),
                });
            }
            PrincipalKind::WorkspaceMembers => {
                return Err(CompileError::UnexpandedPrincipalKind { kind: p.kind });
            }
        }
    }
    Ok((users, groups))
}

/// The mask items for `rule`: an exempt item first (specific principals,
/// `MASK_NONE`) when `rule.exempt` names any, then the general `public`-group
/// item carrying the rule's real mask. See the module doc comment for the
/// Ranger idiom this follows.
fn mask_items(
    rule: &Rule,
    mask_type: &str,
    value_expr: &Option<String>,
) -> Result<Vec<MaskItemInput>, CompileError> {
    let (users, groups) = exempt_principals(rule)?;
    let mut items = Vec::new();
    if !users.is_empty() || !groups.is_empty() {
        items.push(MaskItemInput {
            users,
            groups,
            mask_type: "MASK_NONE".to_string(),
            value_expr: None,
        });
    }
    items.push(MaskItemInput {
        users: Vec::new(),
        groups: vec![PUBLIC_GROUP.to_string()],
        mask_type: mask_type.to_string(),
        value_expr: value_expr.clone(),
    });
    Ok(items)
}

/// The row-filter items for `rule`: an exempt item first (specific
/// principals, an empty `filterExpr` — "no filtering", the same idiom the
/// cited Ranger wiki page uses), then the general `public`-group item
/// carrying the rule's real filter.
fn rowfilter_items(
    rule: &Rule,
    filter_expr: &str,
) -> Result<Vec<RowFilterItemInput>, CompileError> {
    let (users, groups) = exempt_principals(rule)?;
    let mut items = Vec::new();
    if !users.is_empty() || !groups.is_empty() {
        items.push(RowFilterItemInput {
            users,
            groups,
            filter_expr: String::new(),
        });
    }
    items.push(RowFilterItemInput {
        users: Vec::new(),
        groups: vec![PUBLIC_GROUP.to_string()],
        filter_expr: filter_expr.to_string(),
    });
    Ok(items)
}
