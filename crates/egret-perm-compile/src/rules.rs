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
//! **Verified live (Task 10, 2026-09-24, kind `kind-egret`).** The
//! `public`-group idiom above is no longer merely documented: a path-selected
//! mask compiled exactly this way returned the literal string `xxxx` in place
//! of the real values, and a path-selected row filter cut 4 rows to 2 with the
//! excluded rows probed genuinely absent
//! (`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3, cases
//! T3b and T4). The OTHER assumption that paragraph used to sit next to — the
//! resource-hierarchy ANCESTOR match `grants.rs` cited from upstream Ranger
//! source — was measured in the same run and **refuted** for access policies;
//! see `grants::scope_to_resources`. The masking and row-filter resource maps
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
use egret_policy::{starrocks_service_def, trino_service_def, COMPILER_RULE_PREFIX};

use egret_perm::{CompileError, EngineType, PrincipalKind, PrincipalSet, Rule, RuleKind, Selector};

use crate::grants::{
    ensure_a_wildcard_level_has_nothing_named_beneath_it, ensure_scope_levels_are_named, level,
    resource_level_names, scope_to_resources, validate_user_ref, PolicyTarget, ROW_FILTER_LEVELS,
};

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

/// The part of compiling a rule that needs **no service and no engine** —
/// the rule-side twin of [`crate::grants::validate_grant_shape`]; read that
/// function's doc for why a write-time check is needed at all and how "service
/// independent" is drawn.
///
/// In: a path selector naming a level as the EMPTY string
/// (`EmptyScopeLevel` — SP4d debt Task 12; see
/// [`crate::grants::ensure_scope_levels_are_named`] for the measurement and for
/// why this is the rule side of a GRANT-side escalation rather than the same
/// severity: an empty level makes a mask or row filter cover every sibling at
/// that level, which withholds more than the author wrote instead of granting
/// more, and `ensure_row_filter_scope_is_a_table` below would otherwise accept
/// `schema = ''` as a named schema), a path selector that names a level BENEATH
/// a match-any one (`NamedLevelUnderWildcard` — SP4d debt Task 14; see
/// [`crate::grants::ensure_a_wildcard_level_has_nothing_named_beneath_it`], and
/// the same asymmetry and the same `ensure_row_filter_scope_is_a_table` hole
/// apply: it counts `schema = '*'` as a named schema exactly as it counted `''`
/// as one),
/// a `CUSTOM` mask expression that does not parse (`InvalidCustomMask`),
/// and an exempt set a masking/row-filter item cannot express — a non-empty
/// `exempt.exclude`, a `Role`, the literal `public` group
/// (`UnexpressibleExemption`), or an unexpanded principal kind. Out:
/// `UnsupportedMaskType` (reads the target engine's service-def),
/// `MissingTagName`/`MissingTagService` (read the profile), `ExpansionCeiling`
/// (counts that profile's tagged columns).
///
/// Like its grant-side twin this calls the same private helpers `compile_rule`
/// calls — `ensure_custom_mask_parses` and `exempt_principals` — rather than
/// restating their rules.
pub fn validate_rule_shape(r: &Rule) -> Result<(), CompileError> {
    if let RuleKind::Mask {
        mask_type,
        value_expr,
    } = &r.kind
    {
        if mask_type == "CUSTOM" {
            ensure_custom_mask_parses(&r.name, value_expr.as_deref().unwrap_or_default())?;
        }
    }
    if let Selector::Path(scope) = &r.selector {
        ensure_scope_levels_are_named(scope)?;
        ensure_a_wildcard_level_has_nothing_named_beneath_it(scope)?;
    }
    ensure_row_filter_scope_is_a_table(r)?;
    exempt_principals(r)?;
    Ok(())
}

/// A path-selected ROW FILTER must name exactly catalog + schema + table
/// (whole-branch review, Critical 1).
///
/// **Service-independent, so it belongs here.** `rowFilterDef.resources`
/// declares the same three mandatory levels and no `column` on BOTH engines
/// this compiler targets, so a scope that names a column — or that leaves a
/// gap above the table — is unrepresentable for every profile alike; nothing
/// about which service is being compiled for can change the answer, which is
/// the line `validate_grant_shape`'s doc draws for what may live at the
/// authoring boundary.
///
/// **Why a refusal and not a best-effort render.** `scope_to_resources` now
/// renders such a scope into something the def CAN match (the column dropped,
/// a gap wildcarded), so nothing silently fails to filter any more — but what
/// it renders is not what the author asked for. A row filter scoped to
/// `orders.customer_email` would filter every row of `orders` for everyone,
/// which reads to the author as "my column-scoped filter worked" and is a
/// different control from the one they wrote. Refusing says so at the one
/// moment the author can still fix it. The `Tag` selector needs no equivalent
/// check: a tag row filter is table-granular by construction on both engines
/// (Trino's tag policy carries no path at all, and `column_resources` stops at
/// `table`), so there is no scope for an author to get wrong.
///
/// ⚠ **`Some(_)` in the match below means "the level is PRESENT", not "the level
/// NAMES something"**, and that is why two SP4d debt tasks had to run ahead of
/// this function rather than widen it. `(Some(""), Some("orders"), None)` and
/// `(Some("*"), Some("orders"), None)` both land on the `(Some(_), Some(_),
/// None)` arm and are accepted here as "a table", while in enforcement terms
/// both are a table with NO schema. [`validate_rule_shape`] runs
/// [`crate::grants::ensure_scope_levels_are_named`] (Task 12) and
/// [`crate::grants::ensure_a_wildcard_level_has_nothing_named_beneath_it`]
/// (Task 14) BEFORE this call, so neither shape reaches it. Widening these arms
/// instead would have put one rule in two places; the ORDER of the three calls
/// in `validate_rule_shape` is what keeps it in one.
fn ensure_row_filter_scope_is_a_table(r: &Rule) -> Result<(), CompileError> {
    let (RuleKind::RowFilter { .. }, Selector::Path(scope)) = (&r.kind, &r.selector) else {
        return Ok(());
    };
    debug_assert_eq!(
        ROW_FILTER_LEVELS, 3,
        "this check spells out catalog+schema+table; a different row-filter depth needs it rewritten"
    );
    let reason = match (&scope.schema, &scope.table, &scope.column) {
        (_, _, Some(c)) => Some(format!(
            "it is scoped to column {c:?}, and a row filter has no column to be scoped to — \
             the engines' rowFilterDef declares catalog/schema/table and nothing below. Mask \
             the column instead, or scope the row filter to its table"
        )),
        (None, _, None) | (_, None, None) => Some(
            "it must name a catalog, a schema and a table — every level the engines' \
             rowFilterDef declares is mandatory, so a scope that stops short of the table \
             cannot be matched as written"
                .to_string(),
        ),
        (Some(_), Some(_), None) => None,
    };
    match reason {
        Some(reason) => Err(CompileError::UnfilterableRowFilterScope {
            rule: r.name.clone(),
            reason,
        }),
        None => Ok(()),
    }
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
///
/// # The two SCOPE-SHAPE refusals run here as well as at the authoring boundary
///
/// A `Path` selector's scope goes through
/// [`crate::grants::ensure_scope_levels_are_named`] (SP4d debt Task 12) and
/// [`crate::grants::ensure_a_wildcard_level_has_nothing_named_beneath_it`] (SP4d
/// debt Task 14), exactly as `compile_grant` does.
///
/// ⚠ **They did NOT run here until Task 14, and the reason they did not is worth
/// keeping because it was half right.** Task 12 left `compile_rule` unguarded on
/// a severity argument: an unnamed level makes a mask or row filter cover every
/// sibling at that level, so a wrong rule artifact withholds MORE than the author
/// wrote (fails closed) while a wrong grant artifact ALLOWS more (fails open).
/// That argument is sound **about severity** and not sound **about a `pub` entry
/// point**: `compile_rule` is public and can be handed a `Rule` built in Rust
/// that never went near the store, such a caller got no refusal at all, and an
/// artifact that withholds more than the author wrote is still a wrong artifact —
/// it is a control the author cannot read off what they wrote. It is the same
/// argument that put the guard inside `compile_grant` rather than only at the
/// boundary, and the severity difference does not change whether a `pub` function
/// should emit an artifact its own validator refuses.
///
/// **`ensure_row_filter_scope_is_a_table` deliberately stays at the authoring
/// boundary only, and that is NOT the same omission.** Its argument is not about
/// severity. The shapes it refuses — a row-filter scope naming a column, or
/// stopping above the table — are shapes `0154` still permits in `perm.rules`, so
/// rows written before that guard existed are STORABLE today; and
/// `crate::grants::scope_to_resources`' `RowFilter` arm exists precisely to make
/// such a row compile to something the plugin can still match instead of to a
/// silent no-op (read that arm's comment). Moving the refusal into `compile_rule`
/// would turn those stored rows uncompilable, which is a behaviour change rather
/// than a consistency fix. The two guards above have no such population: since
/// `0167` and `0168` nothing stored can carry either shape.
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
    //
    // SP4d debt Task 14, step 5 (carried from Task 12's concern 3): the two
    // scope-shape refusals run HERE too, not only in `validate_rule_shape` — see
    // this function's doc for why the fail-closed severity argument that left
    // them out does not answer "a `pub` entry point emits an artifact its own
    // validator refuses", and for why `ensure_row_filter_scope_is_a_table` is a
    // different case rather than the same omission.
    if let Selector::Path(scope) = &r.selector {
        ensure_scope_levels_are_named(scope)?;
        ensure_a_wildcard_level_has_nothing_named_beneath_it(scope)?;
    }
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
            // **A mask expands per COLUMN; a row filter expands per TABLE**
            // (whole-branch review, Critical 1, third shape). A row-filter
            // policy has no column resource to carry — `rowFilterDef` does not
            // declare one — so two tagged columns of the same table are one
            // and the same row filter, and emitting it twice would write two
            // policies with identical bodies under two different names for the
            // plugin to evaluate in an order nothing here controls. The
            // dedup key is the row-filter resource map's own granularity, so
            // it cannot disagree with what `column_resources` emits, and the
            // policy NAME is built from the same key for the same reason:
            // a name carrying a column this policy does not address would
            // say the expansion is per-column when it is not.
            //
            // The ceiling above still counts `tagged` rather than the deduped
            // set: it bounds the compile-time expansion the model asks for,
            // which is what D5's measurement is about, and dedup can only
            // ever make the emitted count smaller.
            let target = policy_target(&r.kind);
            let mut seen: std::collections::BTreeSet<Vec<String>> =
                std::collections::BTreeSet::new();
            let mut out = Vec::new();
            for col in tagged {
                let resources = column_resources(e, col, target);
                let key: Vec<String> = resources
                    .values()
                    .map(|l| l.values.join(","))
                    .collect::<Vec<_>>();
                if !seen.insert(key) {
                    continue;
                }
                let suffix = match target {
                    PolicyTarget::RowFilter => {
                        format!("{}.{}.{}", col.catalog, col.schema, col.table)
                    }
                    _ => format!(
                        "{}.{}.{}.{}",
                        col.catalog, col.schema, col.table, col.column
                    ),
                };
                let name = format!("{COMPILER_RULE_PREFIX}{}-{suffix}", r.id);
                out.push(build_body(r, e, service, name, resources)?);
            }
            Ok(out)
        }
        (Selector::Path(scope), _) => {
            // A masking policy and a row-filter policy are matched against
            // DIFFERENT declared hierarchies (`dataMaskDef.resources` goes
            // down to `column`; `rowFilterDef.resources` stops at `table`),
            // so which one this is travels with the call rather than being
            // inferred later.
            let resources =
                scope_to_resources(scope, resolved.catalog_name, e, policy_target(&r.kind));
            let name = format!("{COMPILER_RULE_PREFIX}{}", r.id);
            Ok(vec![build_body(r, e, service, name, resources)?])
        }
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

/// Which Ranger policy type a rule's own `kind` renders to — the parameter
/// `scope_to_resources` needs to pick the declared resource hierarchy.
fn policy_target(kind: &RuleKind) -> PolicyTarget {
    match kind {
        RuleKind::Mask { .. } => PolicyTarget::Mask,
        RuleKind::RowFilter { .. } => PolicyTarget::RowFilter,
    }
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
    let name = format!("{COMPILER_RULE_PREFIX}{}", r.id);
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
///
/// `pub(crate)`: `access_to_ranger_json` (`grants.rs`'s renderer, invoked from
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
// `pub(crate)`: SP4c's clearance compiler (`crate::clearance`) puts this SAME
// literal on the deny side of a tag policy to mean "everybody", which is the
// half of Ranger's deny/deny-exception pair D0 measured IS expressible. Two
// copies of this string would drift, and a drift makes a control that withholds
// nothing.
pub(crate) const PUBLIC_GROUP: &str = egret_perm::PUBLIC_GROUP;

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
