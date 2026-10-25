//! Data-mask and row-filter evaluation, and engine parameterisation
//! (SP4d Task 6).
//!
//! Every expectation below was read off Apache Ranger itself and confirmed
//! with `javap -c` against the real `ranger-plugins-common-{2.5.0,2.8.0}.jar`
//! in `~/.m2` — never from a summary. The methods that decide a mask or
//! row-filter outcome are:
//!
//! * `RangerPolicyRepository.{getLikelyMatchPolicyEvaluators,
//!   getLikelyMatchDataMaskPolicyEvaluators, init}` — masking and
//!   row-filtering live in their own evaluator lists, sorted with the same
//!   `EVAL_ORDER_COMPARATOR` as access;
//! * `RangerPolicyEngineImpl.evaluatePoliciesForOneAccessTypeNoAudit` —
//!   whose priority arbitration is `>=` for policy types 1 and 2 and `>`
//!   for access (offsets 489-537 in both jars);
//! * `RangerDefaultPolicyEvaluator.{evaluate, evaluatePolicyItems,
//!   getMatchingPolicyItem}` — the `tableswitch` that sends types 1 and 2
//!   to the SINGLE-list overload, i.e. no deny bucket and no exceptions;
//! * `RangerDefaultDataMaskPolicyItemEvaluator.{<init>, updateAccessResult}`
//!   and `RangerDefaultRowFilterPolicyItemEvaluator.{<init>,
//!   updateAccessResult}` — a mask result is DATA, and the only thing
//!   resolved at request time is `${{ … }}`;
//! * `RangerRequestExprResolver.{PATTERN, hasExpressions}` and
//!   `JavaScriptEdits.{hasDoubleBrackets, replaceDoubleBrackets}`.
use std::collections::BTreeSet;

use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, Decision, EngineConfig,
    EngineType, MaskDecision, PluginVersion, RowFilterDecision, TiedOutcome, UndecidableCause,
};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::resource::RequestResource;
use egret_ranger_eval::servicedef::{PolicyType, ServiceDef};
use serde_json::{json, Value};

// ── fixtures and helpers ─────────────────────────────────────────────────

fn def_json(file: &str) -> Value {
    let path = format!(
        "{}/../egret-policy/src/service-defs/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read service-def"))
        .expect("parse service-def")
}

fn trino_def() -> ServiceDef {
    ServiceDef::parse(&def_json("trino.json"))
}

fn starrocks_def() -> ServiceDef {
    ServiceDef::parse(&def_json("starrocks.json"))
}

/// A `{catalog, schema, table, column}` policy resource map — the Trino
/// mask hierarchy.
fn trino_resources() -> Value {
    json!({
        "catalog": {"values": ["hive"]},
        "schema":  {"values": ["sales"]},
        "table":   {"values": ["orders"]},
        "column":  {"values": ["ssn"]},
    })
}

/// The same map in StarRocks' vocabulary: `database`, not `schema`.
fn starrocks_resources() -> Value {
    json!({
        "catalog":  {"values": ["hive"]},
        "database": {"values": ["sales"]},
        "table":    {"values": ["orders"]},
        "column":   {"values": ["ssn"]},
    })
}

fn trino_request(user: &str) -> AccessRequest {
    AccessRequest {
        user: user.to_string(),
        groups: BTreeSet::new(),
        roles: BTreeSet::new(),
        access_type: "select".to_string(),
        resource: RequestResource::new()
            .with("catalog", "hive")
            .with("schema", "sales")
            .with("table", "orders")
            .with("column", "ssn"),
        owner: None,
        resolved_tags: None,
    }
}

/// A row-filter check is issued at TABLE level: `rowFilterDef.resources`
/// for both engines stops at `table` and names no `column` at all, so a
/// column-level resource would make every row-filter policy an ANCESTOR
/// match — which `RangerDefaultPolicyEvaluator.evaluate` does not admit.
fn trino_table_request(user: &str) -> AccessRequest {
    AccessRequest {
        user: user.to_string(),
        groups: BTreeSet::new(),
        roles: BTreeSet::new(),
        access_type: "select".to_string(),
        resource: RequestResource::new()
            .with("catalog", "hive")
            .with("schema", "sales")
            .with("table", "orders"),
        owner: None,
        resolved_tags: None,
    }
}

fn starrocks_request(user: &str) -> AccessRequest {
    AccessRequest {
        user: user.to_string(),
        groups: BTreeSet::new(),
        roles: BTreeSet::new(),
        access_type: "select".to_string(),
        resource: RequestResource::new()
            .with("catalog", "hive")
            .with("database", "sales")
            .with("table", "orders")
            .with("column", "ssn"),
        owner: None,
        resolved_tags: None,
    }
}

fn policies(policies: Value) -> ServicePolicySet {
    serde_json::from_value(json!({"serviceName": "trino", "policies": policies}))
        .expect("parse policy set")
}

fn engine() -> EngineConfig {
    EngineConfig::without_super_users()
}

/// One `dataMaskPolicyItems` entry for `user`, granting `select`.
fn mask_item(user: &str, mask_info: Value) -> Value {
    json!({
        "users": [user],
        "groups": [],
        "roles": [],
        "accesses": [{"type": "select", "isAllowed": true}],
        "dataMaskInfo": mask_info,
    })
}

fn filter_item(user: &str, filter_expr: Value) -> Value {
    json!({
        "users": [user],
        "groups": [],
        "roles": [],
        "accesses": [{"type": "select", "isAllowed": true}],
        "rowFilterInfo": {"filterExpr": filter_expr},
    })
}

/// A whole `policyType: 1` policy over [`trino_resources`].
fn mask_policy(id: i64, name: &str, items: Value) -> Value {
    json!({
        "id": id,
        "name": name,
        "service": "trino",
        "policyType": 1,
        "isEnabled": true,
        "resources": trino_resources(),
        "dataMaskPolicyItems": items,
    })
}

fn row_filter_policy(id: i64, name: &str, items: Value) -> Value {
    json!({
        "id": id,
        "name": name,
        "service": "trino",
        "policyType": 2,
        "isEnabled": true,
        // `rowFilterDef.resources` for trino stops at `table`.
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["sales"]},
            "table":   {"values": ["orders"]},
        },
        "rowFilterPolicyItems": items,
    })
}

#[track_caller]
fn masked(d: &MaskDecision) -> &egret_ranger_eval::eval::MaskResult {
    match d {
        MaskDecision::Masked(m) => m,
        other => panic!("expected Masked: {other:?}"),
    }
}

#[track_caller]
fn mask_causes(d: &MaskDecision) -> &[UndecidableCause] {
    match d {
        MaskDecision::Undecidable { why } => why,
        other => panic!("expected Undecidable: {other:?}"),
    }
}

#[track_caller]
fn filtered(d: &RowFilterDecision) -> &egret_ranger_eval::eval::RowFilterResult {
    match d {
        RowFilterDecision::Filtered(f) => f,
        other => panic!("expected Filtered: {other:?}"),
    }
}

#[track_caller]
fn filter_causes(d: &RowFilterDecision) -> &[UndecidableCause] {
    match d {
        RowFilterDecision::Undecidable { why } => why,
        other => panic!("expected Undecidable: {other:?}"),
    }
}

// ── 1. a mask result is DATA, not a verdict ──────────────────────────────

/// `RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult` sets
/// `maskType` from `dataMaskInfo.getDataMaskType()` and then reports
/// `isAllowed = true` (the `iconst_1` at offset 91 of both jars). The
/// verdict half carries no information — every matching mask "allows" — so
/// the answer this evaluator owes its caller is the mask itself.
#[test]
fn a_mask_policy_returns_the_mask_type_as_data() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
    )]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    let m = masked(&d);
    assert_eq!(m.mask_type.as_deref(), Some("MASK_HASH"));
    assert_eq!(m.policy_id, Some(1));
    assert_eq!(m.masked_value, None);
    assert_eq!(m.mask_condition, None);
}

/// `MASK_NONE` is a mask type like any other: Ranger sets it and the host
/// plugin decides it means "no transform". Collapsing it into
/// [`MaskDecision::NoMask`] here would lose the difference between "a
/// policy decided not to mask" and "no policy matched".
#[test]
fn mask_none_is_a_mask_result_not_an_absence() {
    let set = policies(json!([mask_policy(
        1,
        "no mask",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_NONE"}))])
    )]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert_eq!(masked(&d).mask_type.as_deref(), Some("MASK_NONE"));
}

/// Nothing matched: `maskType` stays null and the host applies no mask.
#[test]
fn no_matching_mask_policy_is_no_mask() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item("bob", json!({"dataMaskType": "MASK_HASH"}))])
    )]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert_eq!(d, MaskDecision::NoMask);
}

/// **The `ifnull` guard at the top of `updateAccessResult` (offset 8) can
/// never fire, so this is the opposite of what it looks like.**
///
/// SP4d-1 Task 7, finding F2. Reading the bytecode alone says a null
/// `getDataMaskInfo()` returns immediately and the policy determines
/// nothing. Reading the MODEL CLASS says `getDataMaskInfo()` is never null:
/// Ranger deserialises with JACKSON (`JsonUtils.getMapper()` is a
/// `com.fasterxml.jackson.databind.ObjectMapper`; neither jar carries a Gson
/// class), which binds through the SETTER —
/// `RangerDataMaskPolicyItem.setDataMaskInfo(null)` substitutes
/// `new RangerPolicyItemDataMaskInfo()`, and the no-arg constructor reaches
/// the same setter through the all-args one. Measured, not inferred:
/// `JsonUtils.jsonToObject("{}", RangerDataMaskPolicyItem.class)` comes back
/// with a non-null `dataMaskInfo` whose `dataMaskType` is null, and so does
/// `{"dataMaskInfo": null}`.
///
/// So `updateAccessResult` runs its whole body: `setMaskType(null)` and then
/// `policyEvaluator.updateAccessResult(result, matchType, true, …)`, which
/// sets `policyId`, `policyPriority` and `isAllowed`. The item DETERMINES,
/// with a null mask type, and policy 2 is never reached.
///
/// The differential gate found this; the previous version of this test
/// asserted `policy_id == Some(2)` and passed, because the test and the code
/// shared one misreading.
#[test]
fn a_mask_item_without_data_mask_info_still_determines() {
    // The empty one is given the HIGHER priority, so which policy Ranger
    // reaches first is decided by `policyPriority` and not by the
    // `getEvalOrder()` this crate does not reproduce. Left at equal
    // priorities the answer would be `EvalOrderAmbiguous` — the refusal
    // fires on any tie inside one band, whatever the tied policies yield —
    // which would test the refusal rather than the finding.
    let mut empty = mask_policy(
        1,
        "a empty",
        json!([{
            "users": ["alice"],
            "accesses": [{"type": "select", "isAllowed": true}],
        }]),
    );
    empty["policyPriority"] = json!(1);
    let set = policies(json!([
        empty,
        mask_policy(
            2,
            "b real",
            json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
        ),
    ]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    let m = masked(&d);
    assert_eq!(
        m.policy_id,
        Some(1),
        "policy 1 determines; policy 2 is never reached"
    );
    assert_eq!(m.mask_type, None, "it determines with a NULL mask type");
}

/// The row-filter path makes the identical substitution and then behaves the
/// OPPOSITE way, which is why the sweep behind F2 could not stop at one fix.
///
/// `RangerRowFilterPolicyItem.setRowFilterInfo(null)` also substitutes an
/// empty object, so `getRowFilterInfo()` is never null either. But
/// `RangerDefaultRowFilterPolicyItemEvaluator.updateAccessResult` has no
/// guard at the top; it guards the `policyEvaluator.updateAccessResult(...)`
/// call instead, with `if (result.getFilterExpr() != null)` (offsets 40-44).
/// A null `filterExpr` never marks the result, so the item determines
/// NOTHING and the next policy IS reached.
#[test]
fn a_null_row_filter_expression_lets_a_later_policy_decide() {
    let set = policies(json!([
        row_filter_policy(
            1,
            "a empty",
            json!([{
                "users": ["alice"],
                "accesses": [{"type": "select", "isAllowed": true}],
            }])
        ),
        row_filter_policy(
            2,
            "b real",
            json!([filter_item("alice", json!("country = 'US'"))])
        ),
    ]));
    let d = evaluate_row_filter(&set, &trino_table_request("alice"), &trino_def(), &engine());
    assert_eq!(filtered(&d).policy_id, Some(2));
}

// ── 2. row filter returns the expression as data ─────────────────────────

/// `RangerDefaultRowFilterPolicyItemEvaluator.updateAccessResult` sets
/// `filterExpr` and only then, `if (result.getFilterExpr() != null)`, marks
/// allow.
#[test]
fn a_row_filter_policy_returns_the_filter_expression_as_data() {
    let set = policies(json!([row_filter_policy(
        7,
        "us rows only",
        json!([filter_item("alice", json!("country = 'US'"))])
    )]));
    let d = evaluate_row_filter(&set, &trino_table_request("alice"), &trino_def(), &engine());
    let f = filtered(&d);
    assert_eq!(f.filter_expr, "country = 'US'");
    assert_eq!(f.policy_id, Some(7));
}

/// `rowFilterExpr` is null when `rowFilterInfo.getFilterExpr()` is null
/// (the `ifnull 67` at offset 33), so `setFilterExpr` is never called and
/// the guard `if (result.getFilterExpr() != null)` fails: the policy marks
/// nothing. An ABSENT `filterExpr` and an empty-string one are therefore
/// different, which is why [`egret_ranger_eval::model::RowFilterInfo`]
/// keeps it as an `Option`.
///
/// Note the `rowFilterInfo` object here is present but empty, and that an
/// ABSENT `rowFilterInfo` behaves identically — `setRowFilterInfo(null)`
/// substitutes an empty one, so the two shapes are indistinguishable to
/// Ranger. What differs between the mask and row-filter paths is not the
/// substitution but the guard; see
/// `a_null_row_filter_expression_lets_a_later_policy_decide`.
#[test]
fn a_row_filter_item_without_a_filter_expression_determines_nothing() {
    let set = policies(json!([row_filter_policy(
        7,
        "empty",
        json!([{
            "users": ["alice"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "rowFilterInfo": {},
        }])
    )]));
    let d = evaluate_row_filter(&set, &trino_table_request("alice"), &trino_def(), &engine());
    assert_eq!(d, RowFilterDecision::NoFilter);
}

/// The other half of the same distinction, and the reason `filter_expr` is
/// an `Option` at all. Ranger's guard is `if (result.getFilterExpr() != null)`
/// — not `isNotEmpty` — so an EXPLICIT empty string is not the same as an
/// absent one: it marks the result and the item determines something.
/// Without this, the test above passes equally well against a `String` that
/// folds null onto empty, and the distinction is only pinned in the
/// direction that does not need it.
#[test]
fn an_explicitly_empty_filter_expression_still_marks_the_result() {
    let set = policies(json!([row_filter_policy(
        7,
        "explicitly empty",
        json!([filter_item("alice", json!(""))])
    )]));
    let d = evaluate_row_filter(&set, &trino_table_request("alice"), &trino_def(), &engine());
    let f = filtered(&d);
    assert_eq!(
        f.filter_expr, "",
        "an explicit empty filterExpr marks: {d:?}"
    );
    assert_eq!(f.policy_id, Some(7));
}

// ── 3. no deny bucket, no exceptions ─────────────────────────────────────

/// `getMatchingPolicyItem(request, result)` is a `tableswitch` on the raw
/// policy type: case 1 and case 2 call the SINGLE-list overload
/// `getMatchingPolicyItem(request, evaluators)`, which passes `null` for
/// the exception list. `denyPolicyItems` is never even looked at.
#[test]
fn a_deny_bucket_on_a_mask_policy_is_ignored() {
    let mut policy = mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))]),
    );
    policy["denyPolicyItems"] = json!([{
        "users": ["alice"],
        "accesses": [{"type": "select", "isAllowed": true}],
    }]);
    let set = policies(json!([policy]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert_eq!(masked(&d).mask_type.as_deref(), Some("MASK_HASH"));
}

/// The same for `allowExceptions` / `denyExceptions`: the single-list
/// overload passes `null` as the exception list, so
/// `CollectionUtils.isNotEmpty(null)` is false and the veto loop is skipped
/// entirely.
#[test]
fn exceptions_on_a_mask_policy_are_ignored() {
    let mut policy = mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))]),
    );
    policy["allowExceptions"] = json!([{
        "users": ["alice"],
        "accesses": [{"type": "select", "isAllowed": true}],
    }]);
    policy["denyExceptions"] = json!([{
        "users": ["alice"],
        "accesses": [{"type": "select", "isAllowed": true}],
    }]);
    let set = policies(json!([policy]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert_eq!(masked(&d).mask_type.as_deref(), Some("MASK_HASH"));
}

/// The deny-all-else fallback in `evaluatePolicyItems` is guarded by
/// `policyType == null || policyType == 0` (offsets 248-268), so a mask
/// policy's `isDenyAllElse` cannot produce anything at all.
#[test]
fn is_deny_all_else_on_a_mask_policy_is_ignored() {
    let mut policy = mask_policy(1, "mask ssn", json!([]));
    policy["isDenyAllElse"] = json!(true);
    let set = policies(json!([policy]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert_eq!(d, MaskDecision::NoMask);
}

/// The same holds for a row-filter policy.
#[test]
fn a_deny_bucket_on_a_row_filter_policy_is_ignored() {
    let mut policy = row_filter_policy(
        7,
        "us rows only",
        json!([filter_item("alice", json!("country = 'US'"))]),
    );
    policy["denyPolicyItems"] = json!([{
        "users": ["alice"],
        "accesses": [{"type": "select", "isAllowed": true}],
    }]);
    let set = policies(json!([policy]));
    let d = evaluate_row_filter(&set, &trino_table_request("alice"), &trino_def(), &engine());
    assert_eq!(filtered(&d).filter_expr, "country = 'US'");
}

// ── 4. request-time expressions ──────────────────────────────────────────

/// `RangerDefaultDataMaskPolicyItemEvaluator`'s constructor builds a
/// `RangerRequestExprResolver` only when
/// `RangerRequestExprResolver.hasExpressions(valueExpr)`, and
/// `updateAccessResult` then reports `resolveExpressions(request)` instead
/// of the literal. That resolution reads the request context — user
/// attributes, tags, `UserStore` — which egret does not have, so the masked
/// value is genuinely unknowable.
#[test]
fn a_mask_value_with_a_request_expression_is_undecidable() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item(
            "alice",
            json!({"dataMaskType": "CUSTOM", "valueExpr": "${{USER.dept}}"})
        )])
    )]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert!(
        mask_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::MaskExpression { .. })),
        "{:?}",
        mask_causes(&d)
    );
}

/// The `conditionExpr` half of the same constructor, and the same
/// `setMaskCondition(resolver.resolveExpressions(request))` branch.
#[test]
fn a_mask_condition_with_a_request_expression_is_undecidable() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item(
            "alice",
            json!({"dataMaskType": "MASK_HASH", "conditionExpr": "${{USER.dept}} = 'hr'"})
        )])
    )]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert!(
        mask_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::MaskExpression { .. })),
        "{:?}",
        mask_causes(&d)
    );
}

/// THE TRAP, decided deliberately and measured rather than assumed.
///
/// `RangerRequestExprResolver.PATTERN` is `\$\{\{(?<expr>.*?)\}\}` in both
/// 2.5.0 and 2.8.0, so `hasExpressions("cast({col} as varchar)")` is
/// **false**: no resolver is constructed and `updateAccessResult` takes the
/// `else` branch, `setMaskedValue(dataMaskInfo.getValueExpr())` — the
/// literal, braces and all. `StringTokenReplacer`, the mechanism that does
/// expand `{TOKEN}`, is referenced by exactly three classes in the whole
/// 2.5.0 jar (`ResourceMatcher`, `RangerAbstractPolicyEvaluator`,
/// `PolicyEngine`) and by NEITHER mask nor row-filter evaluator; the string
/// `{col}` does not occur anywhere in `ranger-plugins-common` at all. It is
/// the host plugin (`trino.json`'s `maskTypes[].transformer`) that
/// substitutes the column name, downstream of the decision this crate
/// reproduces.
///
/// So reporting the literal here is not a guess — it is byte-for-byte what
/// `RangerAccessResult.getMaskedValue()` holds. Treating every `{TOKEN}` as
/// unknowable would make EVERY CUSTOM mask undecidable for no reason.
#[test]
fn a_custom_mask_carrying_a_col_token_is_data_not_an_unknown() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item(
            "alice",
            json!({"dataMaskType": "CUSTOM", "valueExpr": "cast({col} as varchar)"})
        )])
    )]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    let m = masked(&d);
    assert_eq!(m.mask_type.as_deref(), Some("CUSTOM"));
    assert_eq!(m.masked_value.as_deref(), Some("cast({col} as varchar)"));
}

/// A literal `conditionExpr` is likewise data: `setMaskCondition` gets the
/// string verbatim.
#[test]
fn a_literal_mask_condition_is_data() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item(
            "alice",
            json!({"dataMaskType": "MASK_HASH", "conditionExpr": "region = 'eu'"})
        )])
    )]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert_eq!(masked(&d).mask_condition.as_deref(), Some("region = 'eu'"));
}

#[test]
fn a_row_filter_with_a_request_expression_is_undecidable() {
    let set = policies(json!([row_filter_policy(
        7,
        "own rows",
        json!([filter_item("alice", json!("owner = '${{USER}}'"))])
    )]));
    let d = evaluate_row_filter(&set, &trino_table_request("alice"), &trino_def(), &engine());
    assert!(
        filter_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::RowFilterExpression { .. })),
        "{:?}",
        filter_causes(&d)
    );
}

/// A `{TOKEN}` in a row filter is likewise a literal for Ranger — the same
/// `hasExpressions` gate guards the row-filter resolver.
#[test]
fn a_row_filter_carrying_a_bare_token_is_data_not_an_unknown() {
    let set = policies(json!([row_filter_policy(
        7,
        "own rows",
        json!([filter_item("alice", json!("owner = '{USER}'"))])
    )]));
    let d = evaluate_row_filter(&set, &trino_table_request("alice"), &trino_def(), &engine());
    assert_eq!(filtered(&d).filter_expr, "owner = '{USER}'");
}

/// A row-filter expression is NOT stored verbatim when it carries double
/// brackets: the constructor runs
/// `JavaScriptEdits.replaceDoubleBrackets(filterExpr)` FIRST — rewriting
/// `[[expr,'delim']]` into `expr.split("delim")` with a
/// `DOUBLE_BRACKET_REGEX` of
/// `\[\[([}{\$"a-zA-Z0-9_.\[\]]+)(\,['\"](.+?)['\"])*\]\]` — and only then
/// checks for `${{ }}`. This crate carries no regex engine and does not
/// reproduce that rewrite, so the expression it would report is not the one
/// Ranger reports.
#[test]
fn a_row_filter_with_double_brackets_is_undecidable() {
    let set = policies(json!([row_filter_policy(
        7,
        "own rows",
        json!([filter_item("alice", json!("dept in ([[USER.depts,',']])"))])
    )]));
    let d = evaluate_row_filter(&set, &trino_table_request("alice"), &trino_def(), &engine());
    assert!(
        filter_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::RowFilterDoubleBrackets { .. })),
        "{:?}",
        filter_causes(&d)
    );
}

/// `hasDoubleBrackets` needs BOTH `[[` and `]]`: together they force a
/// refusal, one alone is a literal. Both halves are asserted below.
#[test]
fn double_brackets_refuse_together_and_are_data_apart() {
    let set = policies(json!([row_filter_policy(
        7,
        "arrays",
        json!([filter_item("alice", json!("tags[[1]] is null"))])
    )]));
    // `[[` and `]]` both present — this one IS rewritten.
    assert!(matches!(
        evaluate_row_filter(&set, &trino_table_request("alice"), &trino_def(), &engine()),
        RowFilterDecision::Undecidable { .. }
    ));

    let set = policies(json!([row_filter_policy(
        7,
        "arrays",
        json!([filter_item("alice", json!("tags[[1] is null"))])
    )]));
    assert_eq!(
        filtered(&evaluate_row_filter(
            &set,
            &trino_table_request("alice"),
            &trino_def(),
            &engine()
        ))
        .filter_expr,
        "tags[[1] is null"
    );
}

/// Item-level `conditions` are evaluated by `isMatch` through
/// `matchCustomConditions`, and `isMatch` is exactly what the single-list
/// `getMatchingPolicyItem` calls for a mask policy. So a condition on a
/// MASK item decides the mask.
#[test]
fn conditions_on_a_mask_item_make_the_mask_undecidable() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([{
            "users": ["alice"],
            "accesses": [{"type": "select", "isAllowed": true}],
            "conditions": [{"type": "ip-range", "values": ["10.0.0.0/8"]}],
            "dataMaskInfo": {"dataMaskType": "MASK_HASH"},
        }])
    )]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert!(
        mask_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::PolicyConstruct { .. })),
        "{:?}",
        mask_causes(&d)
    );
}

/// …and conversely, a condition on an `allowExceptions` item of a MASK
/// policy cannot: that bucket is never read for policy type 1.
#[test]
fn conditions_on_an_access_bucket_of_a_mask_policy_do_not_decide_the_mask() {
    let mut policy = mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))]),
    );
    policy["allowExceptions"] = json!([{
        "users": ["alice"],
        "accesses": [{"type": "select", "isAllowed": true}],
        "conditions": [{"type": "ip-range", "values": ["10.0.0.0/8"]}],
    }]);
    let set = policies(json!([policy]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert_eq!(masked(&d).mask_type.as_deref(), Some("MASK_HASH"));
}

// ── 5. the two policy worlds do not see each other ───────────────────────

/// `RangerPolicyRepository.init` files policy type 1 into
/// `dataMaskPolicyEvaluators`, which
/// `getLikelyMatchPolicyEvaluators(request, POLICY_TYPE_ACCESS)` never
/// returns. Task 5 relies on this; it is asserted here from the other side.
#[test]
fn a_mask_policy_is_invisible_to_an_access_decision() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
    )]));
    assert!(matches!(
        evaluate(&set, &trino_request("alice"), &trino_def(), &engine()),
        Decision::Denied { .. }
    ));
}

/// And the reverse: an ACCESS policy contributes nothing to a mask
/// decision, however generous it is.
#[test]
fn an_access_policy_is_invisible_to_a_mask_decision() {
    let set = policies(json!([{
        "id": 1,
        "name": "allow all",
        "policyType": 0,
        "isEnabled": true,
        "resources": trino_resources(),
        "policyItems": [{
            "users": ["alice"],
            "accesses": [{"type": "select", "isAllowed": true}],
        }],
    }]));
    assert_eq!(
        evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine()),
        MaskDecision::NoMask
    );
}

/// A row-filter policy is not a mask policy either.
#[test]
fn a_row_filter_policy_is_invisible_to_a_mask_decision() {
    let set = policies(json!([row_filter_policy(
        7,
        "us rows only",
        json!([filter_item("alice", json!("country = 'US'"))])
    )]));
    assert_eq!(
        evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine()),
        MaskDecision::NoMask
    );
}

// ── 6. precedence: `>=`, not `>` ─────────────────────────────────────────

/// A higher-priority mask policy wins, exactly as for access.
#[test]
fn a_higher_priority_mask_policy_wins() {
    let mut high = mask_policy(
        1,
        "a high",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_NULL"}))]),
    );
    high["policyPriority"] = json!(1);
    let low = mask_policy(
        2,
        "b low",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))]),
    );
    let set = policies(json!([low, high]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert_eq!(masked(&d).mask_type.as_deref(), Some("MASK_NULL"));
}

/// `evaluatePoliciesForOneAccessTypeNoAudit` arbitrates with `>=` for
/// policy types 1 and 2 (`if_icmplt` at offset 529) and `>` for access
/// (`if_icmple` at offset 505). For a mask, therefore, the FIRST matching
/// evaluator in `EVAL_ORDER_COMPARATOR` order is final and every
/// same-priority one after it is skipped.
///
/// That order's third key is `RangerOptimizedPolicyEvaluator.getEvalOrder()`
/// — a resource-specificity score this crate does not reproduce. For access
/// that was harmless (within one band the allow/deny OUTCOME is
/// order-independent); for a mask it is NOT, because a mask is data and the
/// reported result names the policy that produced it. Here the two policies
/// also carry different mask types, so not even the mask type survives the
/// ambiguity.
///
/// The name says what varies in THIS case, not what triggers the refusal:
/// the trigger is the TIE. `evaluate_sub` never compares the two yields, so
/// [`a_same_priority_mask_tie_is_undecidable_when_the_masks_are_identical`]
/// refuses on the same input shape with the masks made equal.
#[test]
fn a_same_priority_mask_tie_is_undecidable_when_the_masks_differ() {
    let set = policies(json!([
        mask_policy(
            1,
            "a",
            json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
        ),
        mask_policy(
            2,
            "b",
            json!([mask_item("alice", json!({"dataMaskType": "MASK_NULL"}))])
        ),
    ]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert!(
        mask_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::EvalOrderAmbiguous { .. })),
        "{:?}",
        mask_causes(&d)
    );
}

/// The tie is refused whether or not the tied policies AGREE.
///
/// `evaluate_sub` never compares the two `SubResult`s: any band holding two
/// deciding policies is `EvalOrderAmbiguous`. That is deliberate rather than
/// an oversight — [`MaskResult`] carries a `policy_id` and a `policy_name`,
/// and a tie this crate cannot order is a tie whose winner it cannot name,
/// so there is no complete answer even when the mask type is not in doubt.
///
/// It also matters in practice: the one live StarRocks profile measured in
/// `docs/en/plans/2026-09-28-sp4d-1-live-result.md` §8 hits exactly this
/// shape — mask policies 8777 and 8801 BOTH carry `dataMaskType: "MASK"`
/// with no `valueExpr`. An `explain` written against the narrower rule would
/// tell an operator the two policies produce different masks. They do not.
///
/// Sibling of
/// [`a_same_priority_mask_tie_is_undecidable_when_the_masks_differ`]: the
/// two differ only in whether the tied masks are equal, and both refuse.
#[test]
fn a_same_priority_mask_tie_is_undecidable_when_the_masks_are_identical() {
    let set = policies(json!([
        mask_policy(
            1,
            "a",
            json!([mask_item("alice", json!({"dataMaskType": "MASK"}))])
        ),
        mask_policy(
            2,
            "b",
            json!([mask_item("alice", json!({"dataMaskType": "MASK"}))])
        ),
    ]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert!(
        mask_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::EvalOrderAmbiguous { .. })),
        "an AGREEING tie must refuse too — the winner's policy id is still unknown: {:?}",
        d
    );
}

/// The tied policies' own outcomes travel on the refusal, in `policy_names`
/// order, so a caller can say "both rules hash" without re-deriving a yield
/// the evaluator already computed. The decision itself stays `Undecidable`.
#[track_caller]
fn tied(why: &[UndecidableCause]) -> (&[String], &[TiedOutcome]) {
    match why {
        [UndecidableCause::EvalOrderAmbiguous {
            policy_names,
            outcomes,
            ..
        }] => (policy_names, outcomes),
        other => panic!("expected one EvalOrderAmbiguous: {other:?}"),
    }
}

fn hash_outcome(mask_type: &str) -> TiedOutcome {
    TiedOutcome::Mask {
        mask_type: Some(mask_type.to_string()),
        masked_value: None,
        mask_condition: None,
    }
}

#[test]
fn an_agreeing_resource_mask_tie_reports_both_outcomes() {
    let set = policies(json!([
        mask_policy(
            1,
            "a",
            json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
        ),
        mask_policy(
            2,
            "b",
            json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
        ),
    ]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    let (names, outcomes) = tied(mask_causes(&d));
    assert_eq!(names, ["a", "b"]);
    assert_eq!(
        outcomes,
        [hash_outcome("MASK_HASH"), hash_outcome("MASK_HASH")]
    );
}

#[test]
fn a_conflicting_tie_reports_each_outcome() {
    let set = policies(json!([
        mask_policy(
            1,
            "a",
            json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
        ),
        mask_policy(
            2,
            "b",
            json!([mask_item("alice", json!({"dataMaskType": "MASK"}))])
        ),
    ]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    let (names, outcomes) = tied(mask_causes(&d));
    assert_eq!(names, ["a", "b"]);
    assert_eq!(outcomes, [hash_outcome("MASK_HASH"), hash_outcome("MASK")]);
}

#[test]
fn a_row_filter_tie_reports_each_outcome() {
    let set = policies(json!([
        row_filter_policy(
            1,
            "a",
            json!([filter_item("alice", json!("region = 'us'"))])
        ),
        row_filter_policy(
            2,
            "b",
            json!([filter_item("alice", json!("region = 'eu'"))])
        ),
    ]));
    let d = evaluate_row_filter(&set, &trino_table_request("alice"), &trino_def(), &engine());
    let (names, outcomes) = tied(filter_causes(&d));
    assert_eq!(names, ["a", "b"]);
    assert_eq!(
        outcomes,
        [
            TiedOutcome::RowFilter {
                filter_expr: "region = 'us'".to_string()
            },
            TiedOutcome::RowFilter {
                filter_expr: "region = 'eu'".to_string()
            },
        ]
    );
}

/// A lower-priority policy that also matches is NOT ambiguity: the bands
/// are ordered, so the higher one decides on its own.
#[test]
fn a_lower_priority_mask_policy_does_not_make_the_answer_ambiguous() {
    let mut high = mask_policy(
        1,
        "a high",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_NULL"}))]),
    );
    high["policyPriority"] = json!(1);
    let low = mask_policy(
        2,
        "b low",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))]),
    );
    let set = policies(json!([high, low]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert_eq!(masked(&d).mask_type.as_deref(), Some("MASK_NULL"));
}

// ── 7. the superuser short-circuit ───────────────────────────────────────

/// `evaluatePoliciesForOneAccessTypeNoAudit` sets `isAllowed(true)` and
/// `policyPriority(Integer.MAX_VALUE)` for a superuser BEFORE the policy
/// type is consulted. The resource-policy loop then resets only
/// `isAccessDetermined`, so on the first evaluator the `>=` arbitration
/// (MAX_VALUE >= anything) re-determines the result and
/// `evaluatePolicyItems` never runs. A superuser's columns come back
/// **unmasked**, not masked-and-allowed.
#[test]
fn a_superuser_gets_no_mask() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
    )]));
    let mut engine = EngineConfig::without_super_users();
    engine.super_users = Some(["alice".to_string()].into_iter().collect());
    assert_eq!(
        evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine),
        MaskDecision::NoMask
    );
}

/// An unmeasured superuser list makes a mask decision unknowable for the
/// same reason it makes an access decision unknowable.
#[test]
fn an_unknown_superuser_list_makes_a_mask_undecidable() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
    )]));
    let d = evaluate_data_mask(
        &set,
        &trino_request("alice"),
        &trino_def(),
        &EngineConfig::default(),
    );
    assert!(mask_causes(&d).contains(&UndecidableCause::SuperUsersUnknown));
}

// ── 8. engine parameterisation ───────────────────────────────────────────

/// The mask hierarchy comes from `dataMaskDef.resources` merged onto the
/// access defs (`ServiceDefUtil.normalizeDataMaskDef`), and the two engines
/// name their middle level differently: `schema` on Trino, `database` on
/// StarRocks. Engine type is therefore not decoration — it selects a
/// different resource vocabulary.
#[test]
fn the_two_engines_have_different_mask_hierarchies() {
    assert_eq!(
        trino_def().hierarchies(PolicyType::DataMask),
        &[vec![
            "catalog".to_string(),
            "schema".to_string(),
            "table".to_string(),
            "column".to_string()
        ]]
    );
    assert_eq!(
        starrocks_def().hierarchies(PolicyType::DataMask),
        &[vec![
            "catalog".to_string(),
            "database".to_string(),
            "table".to_string(),
            "column".to_string()
        ]]
    );
}

/// The same policy body and the same request, evaluated as one engine or
/// the other, give different answers — which is the whole point of making
/// the engine an input rather than a constant.
#[test]
fn the_same_mask_policy_differs_between_the_two_engines() {
    let sr_policy = json!({
        "id": 1,
        "name": "mask ssn",
        "policyType": 1,
        "isEnabled": true,
        "resources": starrocks_resources(),
        "dataMaskPolicyItems": [mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))],
    });
    let set = policies(json!([sr_policy]));

    let mut sr_engine = EngineConfig::without_super_users();
    sr_engine.engine = Some(EngineType::StarRocks);
    sr_engine.plugin_version = Some(PluginVersion::Ranger2_8_0);
    assert_eq!(
        masked(&evaluate_data_mask(
            &set,
            &starrocks_request("alice"),
            &starrocks_def(),
            &sr_engine
        ))
        .mask_type
        .as_deref(),
        Some("MASK_HASH")
    );

    // The same body read through Trino's def: `database` is not a Trino
    // resource, so no hierarchy contains the policy's keys and nothing
    // matches.
    let mut trino_engine = EngineConfig::without_super_users();
    trino_engine.engine = Some(EngineType::Trino);
    trino_engine.plugin_version = Some(PluginVersion::Ranger2_8_0);
    assert_eq!(
        evaluate_data_mask(
            &set,
            &starrocks_request("alice"),
            &trino_def(),
            &trino_engine
        ),
        MaskDecision::NoMask
    );
}

/// The two engines' access-type vocabularies are disjoint in places:
/// `update` exists on StarRocks and not on Trino; `use` exists on Trino and
/// not on StarRocks.
#[test]
fn the_two_engines_have_different_access_type_vocabularies() {
    let trino_def = trino_def();
    let starrocks_def = starrocks_def();
    let trino: Vec<&str> = trino_def
        .access_types()
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    let sr: Vec<&str> = starrocks_def
        .access_types()
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert!(trino.contains(&"use") && !sr.contains(&"use"));
    assert!(sr.contains(&"update") && !trino.contains(&"update"));
}

/// Declaring an engine and then handing the evaluator the OTHER engine's
/// service-def is a configuration error that would otherwise answer
/// "nothing matched" with full confidence. It is refused instead.
#[test]
fn a_declared_engine_must_agree_with_the_service_def() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
    )]));
    let mut engine = EngineConfig::without_super_users();
    engine.engine = Some(EngineType::StarRocks);
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine);
    assert!(
        mask_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::EngineMismatch { .. })),
        "{:?}",
        mask_causes(&d)
    );

    // …and it guards an ACCESS decision too.
    let access = policies(json!([{
        "id": 1, "name": "allow", "policyType": 0, "isEnabled": true,
        "resources": trino_resources(),
        "policyItems": [{"users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}]}],
    }]));
    assert!(matches!(
        evaluate(&access, &trino_request("alice"), &trino_def(), &engine),
        Decision::Undecidable { .. }
    ));
}

/// Which `ranger-plugins-common` each engine loads was MEASURED on the
/// running pods (`docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`):
/// Trino 470 → 2.5.0, StarRocks fe-ubuntu 4.1 → 2.8.0 (Trino 483 → 2.8.0 and
/// fe-ubuntu 4.1.6 → 2.9.0 were read from the images later). A caller that
/// declares a different pairing is describing a deployment this crate has
/// not been verified against.
#[test]
fn a_declared_plugin_version_must_match_the_measured_one_for_the_engine() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
    )]));
    let mut engine = EngineConfig::without_super_users();
    engine.engine = Some(EngineType::Trino);
    // 2.9.0 is the off-pairing now: Trino 483 runs 2.8.0 and is modelled.
    engine.plugin_version = Some(PluginVersion::Ranger2_9_0);
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine);
    assert!(
        mask_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::PluginVersionMismatch { .. })),
        "{:?}",
        mask_causes(&d)
    );

    // 2.5.0 (Trino 470) is no longer supported: its arm is history, not modelled.
    assert!(!EngineType::Trino.models(PluginVersion::Ranger2_5_0));
    // Trino 483 runs 2.8.0: read and proved on the real jar (engine-upgrade Task 5).
    assert!(EngineType::Trino.models(PluginVersion::Ranger2_8_0));
    assert!(EngineType::StarRocks.models(PluginVersion::Ranger2_8_0));
    assert!(!EngineType::StarRocks.models(PluginVersion::Ranger2_5_0));
    // 2.9.0: read (`docs/en/plans/2026-10-13-ranger-2-9-0-reading.md`) and proved on
    // the real jar for StarRocks only; Trino does not run it.
    assert!(EngineType::StarRocks.models(PluginVersion::Ranger2_9_0));
    assert!(!EngineType::Trino.models(PluginVersion::Ranger2_9_0));
}

/// Declaring the engine that DOES match changes nothing — the guard is a
/// guard, not a second source of truth.
#[test]
fn a_matching_engine_declaration_changes_nothing() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
    )]));
    let mut engine = EngineConfig::without_super_users();
    engine.engine = Some(EngineType::Trino);
    engine.plugin_version = Some(PluginVersion::Ranger2_8_0);
    assert_eq!(
        masked(&evaluate_data_mask(
            &set,
            &trino_request("alice"),
            &trino_def(),
            &engine
        ))
        .mask_type
        .as_deref(),
        Some("MASK_HASH")
    );
}

// ── 9. shared machinery still applies ────────────────────────────────────

/// `{OWNER}` is resolved by `matchUserGroupAndOwner`, which the mask item
/// evaluator inherits unchanged.
#[test]
fn owner_unknown_makes_a_mask_undecidable() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item("{OWNER}", json!({"dataMaskType": "MASK_HASH"}))])
    )]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert!(
        mask_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::ResourceOwnerUnknown { .. })),
        "{:?}",
        mask_causes(&d)
    );
}

/// A disabled mask policy never gets an evaluator
/// (`skipBuildingPolicyEvaluator`).
#[test]
fn a_disabled_mask_policy_is_dropped() {
    let mut policy = mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))]),
    );
    policy["isEnabled"] = json!(false);
    let set = policies(json!([policy]));
    assert_eq!(
        evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine()),
        MaskDecision::NoMask
    );
}

/// An envelope-level construct poisons a mask decision exactly as it
/// poisons an access one: everything before the policy-type split runs for
/// policy types 1 and 2 too.
///
/// `tagPolicies` used to be the illustration and no longer can be — SP4d-2b
/// Task 3 EVALUATES that block for every policy type instead of refusing it.
/// The second half keeps the tag claim alive in its new form: the tag phase
/// runs for a MASK request as well, and refuses this one because
/// `resolved_tags` is `None`. `tests/tag_arbitration.rs` owns what happens
/// when it does not.
#[test]
fn envelope_constructs_poison_a_mask_decision() {
    let mask = json!([mask_policy(
        1,
        "mask ssn",
        json!([mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))])
    )]);

    let set: ServicePolicySet = serde_json::from_value(json!({
        "serviceName": "trino",
        "policyDeltas": [{"id": 9}],
        "policies": mask.clone(),
    }))
    .expect("parse policy set");
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert!(
        mask_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::EnvelopeConstruct(_))),
        "{:?}",
        mask_causes(&d)
    );

    let set: ServicePolicySet = serde_json::from_value(json!({
        "serviceName": "trino",
        "tagPolicies": {
            "serviceName": "tag",
            "policies": [{"id": 70, "name": "pii", "policyType": 1}],
        },
        "policies": mask,
    }))
    .expect("parse policy set");
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert!(
        mask_causes(&d).contains(&UndecidableCause::TagsUnresolved),
        "the tag phase runs for a MASK request too: {:?}",
        mask_causes(&d)
    );
}

/// The warm/cold `accessPerms` pre-filter divergence is a property of
/// `RangerOptimizedPolicyEvaluator`, which every policy gets — mask
/// policies included, and its `accessPerms` union spans all six buckets.
#[test]
fn an_access_type_case_mismatch_makes_a_mask_undecidable() {
    let set = policies(json!([mask_policy(
        1,
        "mask ssn",
        json!([{
            "users": ["alice"],
            "accesses": [{"type": "SELECT", "isAllowed": true}],
            "dataMaskInfo": {"dataMaskType": "MASK_HASH"},
        }])
    )]));
    let d = evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine());
    assert!(
        mask_causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::AccessTypeCaseMismatch { .. })),
        "{:?}",
        mask_causes(&d)
    );
}

/// A mask policy that names fewer hierarchy levels than the request is an
/// ANCESTOR match, and `RangerDefaultPolicyEvaluator.evaluate` admits only
/// `SELF` and `SELF_AND_ALL_DESCENDANTS` for a non-tag request — in both
/// 2.5.0 (offsets 317-338) and 2.8.0 (offsets 334-370).
#[test]
fn an_ancestor_match_does_not_mask() {
    let set = policies(json!([{
        "id": 1,
        "name": "mask table",
        "policyType": 1,
        "isEnabled": true,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["sales"]},
            "table":   {"values": ["orders"]},
        },
        "dataMaskPolicyItems": [mask_item("alice", json!({"dataMaskType": "MASK_HASH"}))],
    }]));
    assert_eq!(
        evaluate_data_mask(&set, &trino_request("alice"), &trino_def(), &engine()),
        MaskDecision::NoMask
    );
}
