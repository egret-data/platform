//! Policy-item evaluation, precedence, and the three-valued decision
//! (SP4d Task 5).
//!
//! Every expectation below was read off Apache Ranger itself — source at
//! tags `release-ranger-2.5.0` and `release-ranger-2.8.0`, cross-checked
//! against `javap -c` of the real `ranger-plugins-common-{2.5.0,2.8.0}.jar`
//! in `~/.m2` — and not from any summary. The methods that decide an access
//! outcome are `RangerDefaultPolicyEvaluator.{evaluate, evaluatePolicyItems,
//! getMatchingPolicyItemForAccessPolicyForSpecificAccess, getMatchingPolicyItem,
//! updateAccessResult}`, `RangerDefaultPolicyItemEvaluator.{isMatch,
//! matchUserGroupAndOwner}`, `RangerAbstractPolicyItemEvaluator.computeWithImpliedGrants`,
//! `RangerAbstractPolicyEvaluator.{hasAllow, hasDeny, hasMatchablePolicyItem}`,
//! `RangerPolicyEvaluator.PolicyEvalOrderComparator`, and
//! `RangerPolicyEngineImpl.evaluatePoliciesForOneAccessTypeNoAudit`.
//!
//! 2.5.0 is what Trino 470 enforces, 2.8.0 what StarRocks fe-ubuntu:4.1
//! enforces (`docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`).
//! The two versions' `evaluate` methods are NOT identical — see
//! `the_two_plugin_versions_disagree_about_access_type_any` — but they
//! agree exactly on the path a Trino or StarRocks access check takes.

use std::collections::BTreeSet;

use egret_ranger_eval::eval::{
    evaluate, AccessRequest, Decision, DenyReason, EngineConfig, UndecidableCause,
};
use egret_ranger_eval::model::{ServicePolicySet, Unmodelled};
use egret_ranger_eval::resource::RequestResource;
use egret_ranger_eval::servicedef::{ServiceDef, ServiceDefIssue};
use serde_json::{json, Value};

// ── fixtures and helpers ─────────────────────────────────────────────────

/// The embedded Trino service-def the seeder uses, read as a fixture.
/// Library code in this crate never touches the filesystem; test code may.
fn trino_def_json() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../egret-policy/src/service-defs/trino.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read trino service-def"))
        .expect("parse trino service-def")
}

fn trino_def() -> ServiceDef {
    ServiceDef::parse(&trino_def_json())
}

/// A `{catalog, schema, table, column}` policy resource map.
fn full_resources() -> Value {
    json!({
        "catalog":   {"values": ["hive"]},
        "schema":    {"values": ["sales"]},
        "table":     {"values": ["orders"]},
        "column":    {"values": ["*"]},
    })
}

/// A request for `hive.sales.orders.id`, the shape a Trino column check has.
fn full_request(user: &str, access_type: &str) -> AccessRequest {
    AccessRequest {
        user: user.to_string(),
        groups: BTreeSet::new(),
        roles: BTreeSet::new(),
        access_type: access_type.to_string(),
        resource: RequestResource::new()
            .with("catalog", "hive")
            .with("schema", "sales")
            .with("table", "orders")
            .with("column", "id"),
        owner: None,
        resolved_tags: None,
    }
}

fn policies(policies: Value) -> ServicePolicySet {
    serde_json::from_value(json!({
        "serviceName": "trino",
        "policies": policies,
    }))
    .expect("parse policy set")
}

/// The engine config for a plugin measured to declare no superusers — what
/// both live Trino instances were measured to have
/// (`docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`).
fn no_superusers() -> EngineConfig {
    EngineConfig::without_super_users()
}

fn item(users: &[&str], accesses: &[&str]) -> Value {
    json!({
        "users": users,
        "groups": [],
        "roles": [],
        "accesses": accesses.iter().map(|a| json!({"type": a, "isAllowed": true})).collect::<Vec<_>>(),
    })
}

#[track_caller]
fn assert_allowed(d: &Decision) {
    assert!(
        matches!(d, Decision::Allowed { .. }),
        "expected Allowed: {d:?}"
    );
}

#[track_caller]
fn assert_denied(d: &Decision) {
    assert!(
        matches!(d, Decision::Denied { .. }),
        "expected Denied: {d:?}"
    );
}

#[track_caller]
fn causes(d: &Decision) -> &[UndecidableCause] {
    match d {
        Decision::Undecidable { why } => why,
        other => panic!("expected Undecidable: {other:?}"),
    }
}

// ── 1. bucket order ──────────────────────────────────────────────────────

/// `getMatchingPolicyItemForAccessPolicyForSpecificAccess` asks the DENY
/// pair first and only then the ALLOW pair:
///
/// ```java
/// RangerPolicyItemEvaluator ret = getMatchingPolicyItem(request, denyEvaluators, denyExceptionEvaluators);
/// if (ret == null && !result.getIsAccessDetermined()) {
///     ret = getMatchingPolicyItem(request, allowEvaluators, allowExceptionEvaluators);
/// }
/// ```
///
/// So one policy that both allows and denies the same user denies.
#[test]
fn deny_bucket_is_consulted_before_the_allow_bucket() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
        "denyPolicyItems": [item(&["alice"], &["select"])],
    }]));

    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert_denied(&d);
    assert!(
        matches!(
            d,
            Decision::Denied {
                reason: DenyReason::DenyPolicyItem,
                ..
            }
        ),
        "{d:?}"
    );
}

// ── 2. an exception VETOES a match ───────────────────────────────────────

/// The exception loop sets `ret = null` — it cancels the match rather than
/// competing with it:
///
/// ```java
/// if (ret != null && CollectionUtils.isNotEmpty(exceptionEvaluators)) {
///     for (T exceptionEvaluator : exceptionEvaluators) {
///         if (exceptionEvaluator.isMatch(request)) { ret = null; break; }
///     }
/// }
/// ```
///
/// A cancelled DENY therefore does not become an allow: `ret` is null, so
/// the caller falls through to the ALLOW pair, which must find its own
/// matching item. With no `policyItems` there is nothing to find.
#[test]
fn a_deny_exception_cancels_the_deny_without_allowing() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "denyPolicyItems": [item(&["alice"], &["select"])],
        "denyExceptions": [item(&["alice"], &["select"])],
    }]));

    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(
            d,
            Decision::Denied {
                reason: DenyReason::NoPolicyMatched,
                ..
            }
        ),
        "a cancelled deny must fall through to the allow bucket, not allow: {d:?}"
    );
}

/// The same veto, now with an allow bucket to fall through INTO.
#[test]
fn a_cancelled_deny_falls_through_to_the_allow_bucket() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
        "denyPolicyItems": [item(&["alice"], &["select"])],
        "denyExceptions": [item(&["alice"], &["select"])],
    }]));

    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// And an allow exception vetoes the allow.
#[test]
fn an_allow_exception_cancels_the_allow() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
        "allowExceptions": [item(&["alice"], &["select"])],
    }]));

    assert_denied(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// The exception must be matched against the SAME request, not blanket-applied:
/// an exception naming a different user does not cancel the deny.
#[test]
fn an_exception_naming_another_user_does_not_cancel() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "denyPolicyItems": [item(&["alice"], &["select"])],
        "denyExceptions": [item(&["bob"], &["select"])],
    }]));

    assert!(matches!(
        evaluate(
            &set,
            &full_request("alice", "select"),
            &trino_def(),
            &no_superusers()
        ),
        Decision::Denied {
            reason: DenyReason::DenyPolicyItem,
            ..
        }
    ));
}

// ── 3. isDenyAllElse ─────────────────────────────────────────────────────

/// `evaluatePolicyItems`:
/// ```java
/// } else if (getPolicy().getIsDenyAllElse() && (getPolicy().getPolicyType() == null
///            || getPolicy().getPolicyType() == RangerPolicy.POLICY_TYPE_ACCESS)) {
///     updateAccessResult(oneResult, matchType, false, "matched deny-all-else policy");
/// }
/// ```
#[test]
fn deny_all_else_denies_when_no_item_matched() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "isDenyAllElse": true,
        "resources": full_resources(),
        "policyItems": [item(&["bob"], &["select"])],
    }]));

    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(
            d,
            Decision::Denied {
                reason: DenyReason::DenyAllElse,
                ..
            }
        ),
        "{d:?}"
    );
}

/// …and it does NOT fire when an item did match.
#[test]
fn deny_all_else_does_not_override_a_matching_allow_item() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "isDenyAllElse": true,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
    }]));

    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// The `policyType == POLICY_TYPE_ACCESS` half of the same guard. A
/// masking policy (`policyType 1`) never contributes to an access decision
/// at all — `RangerPolicyRepository.init` files it into
/// `dataMaskPolicyEvaluators`, a list `evaluatePoliciesForOneAccessTypeNoAudit`
/// never reads for `POLICY_TYPE_ACCESS` — so its `isDenyAllElse` cannot deny.
#[test]
fn deny_all_else_on_a_masking_policy_does_not_deny_access() {
    let set = policies(json!([{
        "id": 1, "name": "mask", "service": "trino", "policyType": 1,
        "isDenyAllElse": true,
        "resources": full_resources(),
        "dataMaskPolicyItems": [],
    }]));

    assert!(matches!(
        evaluate(
            &set,
            &full_request("alice", "select"),
            &trino_def(),
            &no_superusers()
        ),
        Decision::Denied {
            reason: DenyReason::NoPolicyMatched,
            ..
        }
    ));
}

// ── 4. policyPriority ────────────────────────────────────────────────────

/// `evaluatePoliciesForOneAccessTypeNoAudit`:
/// ```java
/// } else if (ret.getIsAllowed()) {
///     if (policyType == RangerPolicy.POLICY_TYPE_ACCESS) {
///         if (ret.getPolicyPriority() > evaluator.getPolicyPriority()) { ret.setIsAccessDetermined(true); }
/// ```
/// paired with `PolicyEvalOrderComparator`, which sorts priority DESCENDING.
/// So an override-priority allow is final and a normal-priority deny never
/// gets to run.
#[test]
fn a_higher_priority_allow_beats_a_lower_priority_deny() {
    let set = policies(json!([
        {
            "id": 1, "name": "normal-deny", "service": "trino", "policyType": 0,
            "policyPriority": 0,
            "resources": full_resources(),
            "denyPolicyItems": [item(&["alice"], &["select"])],
        },
        {
            "id": 2, "name": "override-allow", "service": "trino", "policyType": 0,
            "policyPriority": 1,
            "resources": full_resources(),
            "policyItems": [item(&["alice"], &["select"])],
        },
    ]));

    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert_allowed(&d);
    assert!(
        matches!(
            d,
            Decision::Allowed {
                policy_id: Some(2),
                ..
            }
        ),
        "{d:?}"
    );
}

/// At EQUAL priority the deny wins, because `PolicyEvalOrderComparator`'s
/// `compareNormal` puts every `hasDeny()` evaluator ahead of every
/// non-deny one, and `setIsAllowed(false)` sets `isAccessDetermined`.
#[test]
fn at_equal_priority_a_deny_beats_an_allow_in_another_policy() {
    let set = policies(json!([
        {
            "id": 1, "name": "aaa-allow", "service": "trino", "policyType": 0,
            "resources": full_resources(),
            "policyItems": [item(&["alice"], &["select"])],
        },
        {
            "id": 2, "name": "zzz-deny", "service": "trino", "policyType": 0,
            "resources": full_resources(),
            "denyPolicyItems": [item(&["alice"], &["select"])],
        },
    ]));

    // Name order would put the allow first; the hasDeny-first sort must not.
    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(
            d,
            Decision::Denied {
                policy_id: Some(2),
                ..
            }
        ),
        "{d:?}"
    );
}

/// A higher-priority DENY is final too.
#[test]
fn a_higher_priority_deny_beats_a_lower_priority_allow() {
    let set = policies(json!([
        {
            "id": 1, "name": "allow", "service": "trino", "policyType": 0,
            "policyPriority": 0,
            "resources": full_resources(),
            "policyItems": [item(&["alice"], &["select"])],
        },
        {
            "id": 2, "name": "override-deny", "service": "trino", "policyType": 0,
            "policyPriority": 1,
            "resources": full_resources(),
            "denyPolicyItems": [item(&["alice"], &["select"])],
        },
    ]));

    assert!(matches!(
        evaluate(
            &set,
            &full_request("alice", "select"),
            &trino_def(),
            &no_superusers()
        ),
        Decision::Denied {
            policy_id: Some(2),
            ..
        }
    ));
}

// ── 5. impliedGrants ─────────────────────────────────────────────────────

/// `trino.json` declares `all => [select, insert, …]`, and
/// `computeWithImpliedGrants` expands an item's `accesses` through it, so a
/// grant of `all` satisfies a `select` request.
#[test]
fn implied_grants_expand_all_into_select() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["all"])],
    }]));

    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// Expansion runs in ONE direction only: `select` does not imply `all`.
#[test]
fn implied_grants_do_not_run_backwards() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
    }]));

    assert_denied(&evaluate(
        &set,
        &full_request("alice", "insert"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// Ranger's own comment: "Only one round of 'expansion' is done;
/// multi-level impliedGrants (like shown below) are not handled for now".
/// The code nevertheless iterates a `HashMap` while mutating the item it is
/// expanding, so a CHAIN (`admin => write`, `write => read`) may or may not
/// reach the second level depending on that map's unspecified order. A def
/// with a chain is therefore not decidable, and must say so rather than
/// pick one of the two answers.
#[test]
fn a_chained_implied_grant_is_undecidable() {
    let def = ServiceDef::parse(&json!({
        "name": "chained",
        "resources": [
            {"name": "catalog", "parent": "", "matcher": "org.apache.ranger.plugin.resourcematcher.RangerDefaultResourceMatcher", "matcherOptions": {"wildCard": true, "ignoreCase": true}},
        ],
        "accessTypes": [
            {"name": "read"},
            {"name": "write", "impliedGrants": ["read"]},
            {"name": "admin", "impliedGrants": ["write"]},
        ],
    }));

    let set = policies(json!([{
        "id": 1, "name": "p", "service": "chained", "policyType": 0,
        "resources": {"catalog": {"values": ["hive"]}},
        "policyItems": [item(&["alice"], &["admin"])],
    }]));

    let request = AccessRequest {
        user: "alice".to_string(),
        groups: BTreeSet::new(),
        roles: BTreeSet::new(),
        access_type: "read".to_string(),
        resource: RequestResource::new().with("catalog", "hive"),
        owner: None,
        resolved_tags: None,
    };

    let d = evaluate(&set, &request, &def, &no_superusers());
    // Named exactly, so the test cannot pass on some other service-def issue.
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::ServiceDef(ServiceDefIssue::ChainedImpliedGrants { implying, implied })
                if implying == "admin" && implied == "write"
        )),
        "{d:?}"
    );

    // And the control: the def egret actually ships declares only a FLAT
    // `all => [select, insert, …]`, so it raises no service-def cause at all.
    assert!(
        trino_def().issues().is_empty(),
        "{:?}",
        trino_def().issues()
    );
}

// ── 6-8. principal matching ──────────────────────────────────────────────

/// `matchUserGroupAndOwner`, verbatim order: users, then groups, then
/// roles, then owner — each guarded by `if (!ret ...)`, so the first hit
/// short-circuits. A user hit means the group list is never consulted; a
/// policy that names the user but a group the user is not in still matches.
#[test]
fn a_user_hit_short_circuits_the_group_check() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [{
            "users": ["alice"],
            "groups": ["finance"],
            "roles": ["analyst"],
            "accesses": [{"type": "select", "isAllowed": true}],
        }],
    }]));

    // alice is in no group and has no role.
    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// A group hit when the user list misses.
#[test]
fn a_group_hit_matches_when_the_user_list_misses() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [{
            "users": ["bob"],
            "groups": ["finance"],
            "accesses": [{"type": "select", "isAllowed": true}],
        }],
    }]));

    let mut request = full_request("alice", "select");
    request.groups.insert("finance".to_string());
    assert_allowed(&evaluate(&set, &request, &trino_def(), &no_superusers()));

    // …and misses when she is not in it.
    assert_denied(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// A role hit when users and groups both miss.
#[test]
fn a_role_hit_matches_when_users_and_groups_miss() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [{
            "users": ["bob"],
            "groups": ["finance"],
            "roles": ["analyst"],
            "accesses": [{"type": "select", "isAllowed": true}],
        }],
    }]));

    let mut request = full_request("alice", "select");
    request.roles.insert("analyst".to_string());
    assert_allowed(&evaluate(&set, &request, &trino_def(), &no_superusers()));
}

/// `RangerPolicyEngine.GROUP_PUBLIC = "public"` (confirmed with
/// `javap -constants` on the 2.5.0 jar). `matchUserGroupAndOwner` resolves
/// it by NAME — `policyItem.getGroups().contains(GROUP_PUBLIC)` — before
/// ever looking at the request's group set, so it matches a user who is in
/// no groups at all. This matters for egret specifically: egret runs no
/// usersync, so `public` is the only group name that can ever match.
#[test]
fn public_matches_a_user_with_no_groups() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [{
            "users": [],
            "groups": ["public"],
            "accesses": [{"type": "select", "isAllowed": true}],
        }],
    }]));

    let request = full_request("alice", "select");
    assert!(request.groups.is_empty());
    assert_allowed(&evaluate(&set, &request, &trino_def(), &no_superusers()));
}

/// `public` is matched exactly, not case-insensitively:
/// `List.contains(GROUP_PUBLIC)` in `RangerDefaultPolicyItemEvaluator`.
#[test]
fn public_is_matched_exactly() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [{
            "groups": ["PUBLIC"],
            "accesses": [{"type": "select", "isAllowed": true}],
        }],
    }]));

    assert_denied(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// `RangerPolicyEngine.USER_CURRENT = "{USER}"`. `init` sets
/// `hasCurrentUser = users.contains(USER_CURRENT)` and
/// `matchUserGroupAndOwner` then reads `hasCurrentUser || users.contains(user)`
/// — so `{USER}` matches whoever is asking.
#[test]
fn the_user_macro_matches_any_user() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["{USER}"], &["select"])],
    }]));

    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
    assert_allowed(&evaluate(
        &set,
        &full_request("bob", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// `RangerPolicyEngine.RESOURCE_OWNER = "{OWNER}"` resolves against
/// `request.getResource().getOwnerUser()`. egret's request carries an owner
/// only when the caller actually knows one; when it does not, the outcome
/// is not knowable and must say so rather than guess either way.
#[test]
fn the_owner_macro_needs_an_owner_and_says_so_when_it_has_none() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["{OWNER}"], &["select"])],
    }]));

    let unknown_owner = full_request("alice", "select");
    assert!(unknown_owner.owner.is_none());
    let d = evaluate(&set, &unknown_owner, &trino_def(), &no_superusers());
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::ResourceOwnerUnknown {
                policy_id: Some(1),
                ..
            }
        )),
        "{d:?}"
    );

    let mut owned = full_request("alice", "select");
    owned.owner = Some("alice".to_string());
    assert_allowed(&evaluate(&set, &owned, &trino_def(), &no_superusers()));

    let mut owned_by_other = full_request("alice", "select");
    owned_by_other.owner = Some("bob".to_string());
    assert_denied(&evaluate(
        &set,
        &owned_by_other,
        &trino_def(),
        &no_superusers(),
    ));
}

// ── 9. ANCESTOR never allows ─────────────────────────────────────────────

/// The live-measured T1 case, now at the decision layer. A policy naming
/// `{catalog, schema}` is an ANCESTOR match for a `catalog/schema/table/column`
/// request, and in neither plugin version does that reach an allow:
///
/// * 2.8.0 gates it in `evaluate`:
///   `isMatched = SELF || SELF_AND_ALL_DESCENDANTS || (ANCESTOR && request instanceof RangerTagAccessRequest)`.
/// * 2.5.0 gates it in `updateAccessResult`:
///   `if (matchType != MatchType.ANCESTOR) { result.setIsAllowed(true); … }`.
///
/// Measured live on Trino: the same grant compiled to `{catalog, schema}`
/// produced `Access Denied: Cannot select from columns [v, id]`, while with
/// `column: "*"` it returned rows
/// (`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3 T1).
/// Getting this wrong fails OPEN.
#[test]
fn an_ancestor_match_does_not_allow() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["sales"]},
        },
        "policyItems": [item(&["alice"], &["select"])],
    }]));

    assert!(matches!(
        evaluate(
            &set,
            &full_request("alice", "select"),
            &trino_def(),
            &no_superusers()
        ),
        Decision::Denied {
            reason: DenyReason::NoPolicyMatched,
            ..
        }
    ));
}

/// The same policy with `column: "*"` — the shape that DID return rows —
/// allows.
#[test]
fn the_same_policy_with_a_column_wildcard_allows() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
    }]));

    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// An ANCESTOR match must not reach the item buckets at all — so it cannot
/// deny either.
#[test]
fn an_ancestor_match_does_not_deny() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "isDenyAllElse": true,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["sales"]},
        },
        "denyPolicyItems": [item(&["alice"], &["select"])],
    }]));

    assert!(matches!(
        evaluate(
            &set,
            &full_request("alice", "select"),
            &trino_def(),
            &no_superusers()
        ),
        Decision::Denied {
            reason: DenyReason::NoPolicyMatched,
            ..
        }
    ));
}

// ── 10. nothing matched is a DENY, not an Undecidable ────────────────────

/// `evaluatePoliciesForOneAccessTypeNoAudit` starts from a result whose
/// `isAllowed` is false and never sets it; the final
/// `if (!ret.getIsAllowed() && !getIsFallbackSupported()) { ret.setIsAccessDetermined(true); }`
/// leaves `isAllowed` false. "No policy said anything" is a definite deny,
/// not an unknown — and the evaluator must not hide behind Undecidable for it.
#[test]
fn nothing_matched_is_denied_not_undecidable() {
    let set = policies(json!([]));
    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        matches!(
            d,
            Decision::Denied {
                reason: DenyReason::NoPolicyMatched,
                policy_id: None,
                ..
            }
        ),
        "{d:?}"
    );
}

// ── 11. an unmodelled construct forces Undecidable ───────────────────────

/// The failure this whole package exists to prevent is a confident wrong
/// answer. A policy carrying a construct the reader could not model makes
/// the decision undecidable EVEN WHEN another policy would have allowed,
/// because egret cannot know the construct did not change the outcome —
/// here, an item-level `conditions` list gating a deny.
#[test]
fn an_unmodelled_construct_beats_another_policys_allow() {
    let set = policies(json!([
        {
            "id": 1, "name": "clean-allow", "service": "trino", "policyType": 0,
            "resources": full_resources(),
            "policyItems": [item(&["alice"], &["select"])],
        },
        {
            "id": 2, "name": "conditional-deny", "service": "trino", "policyType": 0,
            "resources": full_resources(),
            "denyPolicyItems": [{
                "users": ["alice"],
                "accesses": [{"type": "select", "isAllowed": true}],
                "conditions": [{"type": "ip-range", "values": ["10.0.0.0/8"]}],
            }],
        },
    ]));

    // Without policy 2 the answer would be a clean allow.
    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::PolicyConstruct {
                policy_id: Some(2),
                ..
            }
        )),
        "{d:?}"
    );
}

/// A policy-wide `validitySchedules` does the same.
#[test]
fn a_validity_schedule_forces_undecidable() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "validitySchedules": [{"startTime": "2026/01/01 00:00:00"}],
        "policyItems": [item(&["alice"], &["select"])],
    }]));

    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    // Named exactly. Review round 1, M4: this asserted only
    // `!causes(&d).is_empty()`, which any cause at all would have
    // satisfied — the same looseness `8d0649c3` went back to fix in the
    // chained-grant test, missed in this member of the same population.
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::PolicyConstruct {
                construct: Unmodelled::ValiditySchedules,
                policy_id: Some(1),
                ..
            }
        )),
        "{d:?}"
    );
}

/// An envelope-level construct forces it too, and is reported as an envelope
/// cause rather than pinned on some policy.
///
/// The construct is `policyDeltas` — an INCREMENTAL download, so `policies` is
/// partial or absent. `tagPolicies` used to be the illustration here and no
/// longer can be: SP4d-2b Task 3 made the composed evaluator EVALUATE that
/// block rather than refuse it, so it is no longer an envelope cause at all.
/// The second half below keeps a foot in that door — a tag block still forces
/// `Undecidable` for this request, now through the tag phase's own refusal,
/// because `resolved_tags` is `None`. `tests/tag_tiers.rs` owns the tier
/// boundaries and `tests/tag_arbitration.rs` owns the composition.
#[test]
fn an_envelope_construct_forces_undecidable() {
    let allow_alice = json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
    }]);

    let set: ServicePolicySet = serde_json::from_value(json!({
        "serviceName": "trino",
        "policyDeltas": [{"id": 9}],
        "policies": allow_alice.clone(),
    }))
    .expect("parse");
    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::EnvelopeConstruct(Unmodelled::PolicyDeltas)
        )),
        "{d:?}"
    );

    let set: ServicePolicySet = serde_json::from_value(json!({
        "serviceName": "trino",
        "tagPolicies": {
            "serviceName": "tag",
            "policies": [{"id": 70, "name": "pii", "policyType": 0}],
        },
        "policies": allow_alice,
    }))
    .expect("parse");
    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).contains(&UndecidableCause::TagsUnresolved),
        "a tag block still refuses for an unresolved tag set: {d:?}"
    );
}

/// …but a construct that cannot reach an ACCESS decision does not poison
/// it. `dataMaskInfo.valueExpr` lives in `dataMaskPolicyItems`, which
/// `getMatchingPolicyItem` reads only for `POLICY_TYPE_DATAMASK`; the
/// masking policy is in a different evaluator list entirely. A CUSTOM mask
/// always carries a `{col}` marker, so treating it as poisoning would make
/// every access decision on a masked column undecidable for no reason.
#[test]
fn a_masking_policys_expression_does_not_poison_an_access_decision() {
    let set = policies(json!([
        {
            "id": 1, "name": "allow", "service": "trino", "policyType": 0,
            "resources": full_resources(),
            "policyItems": [item(&["alice"], &["select"])],
        },
        {
            "id": 2, "name": "mask", "service": "trino", "policyType": 1,
            "resources": full_resources(),
            "dataMaskPolicyItems": [{
                "users": ["alice"],
                "accesses": [{"type": "select", "isAllowed": true}],
                "dataMaskInfo": {"dataMaskType": "CUSTOM", "valueExpr": "substr({col}, 1, 4)"},
            }],
        },
    ]));

    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// A resource VALUE carrying a Ranger runtime expression is different: the
/// matcher resolves those (`RangerAbstractResourceMatcher.OPTION_REPLACE_TOKENS`
/// and its `exprResolver`) and this crate does not, so egret cannot tell
/// whether the policy matched — even though nothing else about it is odd.
#[test]
fn a_resource_expression_forces_undecidable_even_when_it_seems_not_to_match() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["${{USER}}"]},
            "table":   {"values": ["orders"]},
            "column":  {"values": ["*"]},
        },
        "denyPolicyItems": [item(&["alice"], &["select"])],
    }]));

    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::PolicyConstruct {
                construct: Unmodelled::ResourceValueExpressionMarker { resource_type, value, .. },
                ..
            } if resource_type == "schema" && value == "${{USER}}"
        )),
        "{d:?}"
    );
}

/// DG Task 7: egret seeds `trinouser = {USER}` (impersonate, users
/// `{USER}`) on every delegating Trino profile, the first resource-value
/// token egret itself writes. The rule above refused EVERY access decision on
/// such a profile, because a marker was reported "whether or not the policy
/// matched", so explain and every other evaluator caller would have gone
/// undecidable on a `select` the policy can never reach.
///
/// The token can only change what a matcher compares a REQUEST value against.
/// When no marked level is a level the request carries, and the policy is
/// `NONE` both as stored and with every marked value widened to `*` (which
/// matches whatever the token expands to), no expansion can make it match,
/// and the marker cannot decide anything.
#[test]
fn a_resource_expression_on_a_level_the_request_lacks_cannot_poison_it() {
    for token in ["{USER}", "${{USER}}"] {
        let set = policies(json!([
            {
                "id": 1, "name": "allow", "service": "trino", "policyType": 0,
                "resources": full_resources(),
                "policyItems": [item(&["alice"], &["select"])],
            },
            {
                "id": 2, "name": "self-impersonate", "service": "trino", "policyType": 0,
                "resources": { "trinouser": {"values": [token]} },
                "policyItems": [item(&["{USER}"], &["impersonate"])],
            },
        ]));
        assert_allowed(&evaluate(
            &set,
            &full_request("alice", "select"),
            &trino_def(),
            &no_superusers(),
        ));
    }
}

/// The other half: where the token's level IS in the request, the marker
/// still refuses, whoever asks for whom. So egret-ranger-eval does not claim
/// the self-impersonation seed allows or denies anything; it declines.
#[test]
fn a_resource_expression_on_a_level_the_request_carries_still_refuses() {
    let set = policies(json!([{
        "id": 2, "name": "self-impersonate", "service": "trino", "policyType": 0,
        "resources": { "trinouser": {"values": ["{USER}"]} },
        "policyItems": [item(&["{USER}"], &["impersonate"])],
    }]));
    for target in ["alice", "bob"] {
        let d = evaluate(
            &set,
            &AccessRequest {
                user: "alice".into(),
                groups: BTreeSet::new(),
                roles: BTreeSet::new(),
                access_type: "impersonate".into(),
                resource: RequestResource::new().with("trinouser", target),
                owner: None,
                resolved_tags: None,
            },
            &trino_def(),
            &no_superusers(),
        );
        assert!(
            causes(&d).iter().any(|c| matches!(
                c,
                UndecidableCause::PolicyConstruct {
                    construct: Unmodelled::ResourceValueExpressionMarker { resource_type, .. },
                    ..
                } if resource_type == "trinouser"
            )),
            "alice as {target}: {d:?}"
        );
    }
}

/// And "the request lacks the level" alone is NOT enough: a policy on
/// `{catalog: hive, schema: {USER}}` against a catalog-level request is a
/// DESCENDANT match whatever the token expands to, and a descendant is a trie
/// candidate. The widened-to-`*` test is what keeps this one refusing.
#[test]
fn a_resource_expression_below_the_requested_level_still_refuses() {
    let set = policies(json!([{
        "id": 1, "name": "per-user-schema", "service": "trino", "policyType": 0,
        "resources": {
            "catalog": {"values": ["hive"]},
            "schema":  {"values": ["{USER}"]},
        },
        "denyPolicyItems": [item(&["alice"], &["use"])],
    }]));
    let d = evaluate(
        &set,
        &AccessRequest {
            user: "alice".into(),
            groups: BTreeSet::new(),
            roles: BTreeSet::new(),
            access_type: "use".into(),
            resource: RequestResource::new().with("catalog", "hive"),
            owner: None,
            resolved_tags: None,
        },
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::PolicyConstruct {
                construct: Unmodelled::ResourceValueExpressionMarker { resource_type, .. },
                ..
            } if resource_type == "schema"
        )),
        "{d:?}"
    );
}

/// A DISABLED policy is dropped by `RangerPolicyRepository.skipBuildingPolicyEvaluator`
/// before an evaluator is ever built, so neither its items nor its
/// unmodelled constructs can reach a decision.
#[test]
fn a_disabled_policy_is_invisible_including_its_unmodelled_constructs() {
    let set = policies(json!([
        {
            "id": 1, "name": "allow", "service": "trino", "policyType": 0,
            "resources": full_resources(),
            "policyItems": [item(&["alice"], &["select"])],
        },
        {
            "id": 2, "name": "disabled-conditional-deny", "service": "trino", "policyType": 0,
            "isEnabled": false,
            "resources": full_resources(),
            "conditions": [{"type": "ip-range", "values": ["10.0.0.0/8"]}],
            "denyPolicyItems": [item(&["alice"], &["select"])],
        },
    ]));

    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

// ── things measured in the jars that the brief did not contain ───────────

/// `evaluatePoliciesForOneAccessTypeNoAudit` consults
/// `ranger.plugin.super.users` / `ranger.plugin.super.groups` BEFORE any
/// policy, and a superuser is allowed with `policyPriority = Integer.MAX_VALUE`,
/// which no policy can override. egret cannot read the engine's plugin
/// config, so an unknown superuser list makes every decision unknowable —
/// and the caller has to say what it measured.
#[test]
fn an_unknown_superuser_list_is_undecidable() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
    }]));

    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &EngineConfig::default(),
    );
    assert!(
        causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::SuperUsersUnknown)),
        "{d:?}"
    );
}

/// A declared superuser is allowed even where a policy denies.
#[test]
fn a_superuser_is_allowed_over_a_deny() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "policyPriority": 1,
        "resources": full_resources(),
        "denyPolicyItems": [item(&["root"], &["select"])],
    }]));

    let mut engine = EngineConfig::without_super_users();
    engine.super_users = Some(["root".to_string()].into_iter().collect());

    assert_allowed(&evaluate(
        &set,
        &full_request("root", "select"),
        &trino_def(),
        &engine,
    ));
}

/// **A measured divergence between the two plugin versions.**
///
/// 2.5.0: `if (request.isAccessTypeAny()) { isMatched = matchType != MatchType.NONE; }`
/// — an ANCESTOR or DESCENDANT match reaches the policy items.
/// 2.8.0: `isMatched = SELF || SELF_AND_ALL_DESCENDANTS || DESCENDANT
/// || (ANCESTOR && request instanceof RangerTagAccessRequest)` — ANCESTOR
/// does not. Both were read out of the jars with `javap -c`. So a `_any`
/// request against an ancestor policy with `isDenyAllElse` denies on Trino
/// 470 and does not on StarRocks 4.1. The evaluator refuses to pick one.
#[test]
fn the_two_plugin_versions_disagree_about_access_type_any() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
    }]));

    let d = evaluate(
        &set,
        &full_request("alice", "_any"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d)
            .iter()
            .any(|c| matches!(c, UndecidableCause::PseudoAccessType(a) if a == "_any")),
        "{d:?}"
    );
}

/// `_admin` and `_super_user` are pseudo access types with their own
/// branches (`isAccessTypeDelegatedAdmin`, and the `SUPER_USER_ACCESS`
/// short-circuit at the top of `evaluatePoliciesForOneAccessTypeNoAudit`).
#[test]
fn the_other_pseudo_access_types_are_undecidable_too() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
    }]));

    for access_type in ["_admin", "_super_user"] {
        let d = evaluate(
            &set,
            &full_request("alice", access_type),
            &trino_def(),
            &no_superusers(),
        );
        assert!(
            causes(&d)
                .iter()
                .any(|c| matches!(c, UndecidableCause::PseudoAccessType(a) if a == access_type)),
            "{access_type}: {d:?}"
        );
    }
}

/// **A second measured divergence, this time inside one version.** The
/// evaluator `RangerPolicyRepository.buildPolicyEvaluator` actually
/// constructs is always `RangerOptimizedPolicyEvaluator`, whose
/// `hasMatchablePolicyItem` pre-filter tests
/// `accessPerms.contains(request.getAccessType())` — a case-SENSITIVE
/// `HashSet` lookup — while the item match it is meant to approximate uses
/// `StringUtils.equalsIgnoreCase`. The pre-filter is also only armed once
/// `checkIfAllEvaluatorsInitialized()` is true, which happens after every
/// item evaluator has computed its implied grants. So a policy whose access
/// type differs from the request's only in CASE is matched by a cold plugin
/// and skipped by a warm one. Warm-up state is not a function of the
/// request, so this is not decidable.
#[test]
fn an_access_type_matching_only_case_insensitively_is_undecidable() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["SELECT"])],
    }]));

    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::AccessTypeCaseMismatch { access_type, policy_id: Some(1), .. }
                if access_type == "SELECT"
        )),
        "{d:?}"
    );
}

/// An access type spelled exactly as the request spells it is decidable.
#[test]
fn an_exactly_spelled_access_type_is_decidable() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
    }]));

    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// `RangerDefaultPolicyItemEvaluator.isMatch` reads
/// `access.getIsAllowed() && equalsIgnoreCase(access.getType(), …)` — so an
/// `isAllowed: false` entry inside an ALLOW bucket does not allow. This
/// crate models that exactly, which is why
/// `Unmodelled::ExplicitDeny` does not force Undecidable.
#[test]
fn an_is_allowed_false_entry_in_the_allow_bucket_does_not_allow() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [{
            "users": ["alice"],
            "accesses": [{"type": "select", "isAllowed": false}],
        }],
    }]));

    assert!(matches!(
        evaluate(
            &set,
            &full_request("alice", "select"),
            &trino_def(),
            &no_superusers()
        ),
        Decision::Denied {
            reason: DenyReason::NoPolicyMatched,
            ..
        }
    ));
}

/// `RangerAbstractPolicyEvaluator.hasMatchablePolicyItem` is
/// `hasAllow() || hasDeny()` — `policyItems` non-empty, or `denyPolicyItems`
/// non-empty / `isDenyAllElse`. A policy carrying ONLY exceptions never
/// reaches `evaluatePolicyItems` at all.
#[test]
fn a_policy_with_only_exceptions_is_never_evaluated() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "allowExceptions": [item(&["alice"], &["select"])],
        "denyExceptions": [item(&["alice"], &["select"])],
    }]));

    assert!(matches!(
        evaluate(
            &set,
            &full_request("alice", "select"),
            &trino_def(),
            &no_superusers()
        ),
        Decision::Denied {
            reason: DenyReason::NoPolicyMatched,
            policy_id: None,
            ..
        }
    ));
}

/// `RangerAbstractPolicyEvaluator.init` builds one resource evaluator per
/// `additionalResources[i]` and `evaluate` loops over all of them, so an
/// additional map gets a full, independent chance to match.
#[test]
fn an_additional_resource_map_can_carry_the_match() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": {
            "catalog": {"values": ["other"]},
            "schema":  {"values": ["sales"]},
            "table":   {"values": ["orders"]},
            "column":  {"values": ["*"]},
        },
        "additionalResources": [full_resources()],
        "policyItems": [item(&["alice"], &["select"])],
    }]));

    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// A policy whose resources do not cover the request contributes nothing —
/// not an allow, not a deny, and not an Undecidable either, even though it
/// carries a deny item.
#[test]
fn a_non_matching_policy_contributes_nothing() {
    let set = policies(json!([
        {
            "id": 1, "name": "allow", "service": "trino", "policyType": 0,
            "resources": full_resources(),
            "policyItems": [item(&["alice"], &["select"])],
        },
        {
            "id": 2, "name": "elsewhere-deny", "service": "trino", "policyType": 0,
            "resources": {
                "catalog": {"values": ["other"]},
                "schema":  {"values": ["sales"]},
                "table":   {"values": ["orders"]},
                "column":  {"values": ["*"]},
            },
            "denyPolicyItems": [item(&["alice"], &["select"])],
        },
    ]));

    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

// ── review round 1: a guard that matched differently from the thing it
//    guarded ───────────────────────────────────────────────────────────────

/// **CRITICAL 1, fail-open.** The `{OWNER}` cause detector compared an
/// item's LITERAL `accesses[].type` against the request's access type, but
/// the thing it guards — [`item_is_match`] — matches through
/// `computeWithImpliedGrants`. `trino.json` declares
/// `all => [select, insert, …]`, so a deny item granting `all` to `{OWNER}`
/// DECIDES a `select` request while being invisible to the detector: egret
/// answered `Allowed` where Ranger, given the owner, denies.
///
/// The three rows are the reviewer's probe table. Row 3 is the control that
/// isolates the cause: naming the access type literally always raised it.
#[test]
fn an_owner_item_reached_through_implied_grants_still_raises_the_cause() {
    let via_implied = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
        "denyPolicyItems": [item(&["{OWNER}"], &["all"])],
    }]));

    // Row 1: owner unknown. `all` implies `select`, so the deny item can
    // decide — the answer is not knowable.
    let d = evaluate(
        &via_implied,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::ResourceOwnerUnknown {
                policy_id: Some(1),
                ..
            }
        )),
        "row 1: {d:?}"
    );

    // Row 2: the ground truth the missing cause was hiding. With the owner
    // supplied, the very same policy denies.
    let mut owned = full_request("alice", "select");
    owned.owner = Some("alice".to_string());
    assert!(
        matches!(
            evaluate(&via_implied, &owned, &trino_def(), &no_superusers()),
            Decision::Denied {
                reason: DenyReason::DenyPolicyItem,
                ..
            }
        ),
        "row 2"
    );

    // Row 3, the control: the same shape with the access type named
    // literally always raised the cause, even before the fix.
    let literal = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["select"])],
        "denyPolicyItems": [item(&["{OWNER}"], &["select"])],
    }]));
    let d = evaluate(
        &literal,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::ResourceOwnerUnknown { policy_id, .. } if *policy_id == Some(1)
        )),
        "row 3 control: {d:?}"
    );
}

/// The allow-side half of the same defect — fail-CLOSED, but wrong in the
/// same way: with the owner unknown the answer was a confident
/// `Denied { NoPolicyMatched }`, while supplying the owner on identical
/// input allows.
#[test]
fn the_allow_side_of_the_owner_implied_grant_defect() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [{
            "users": ["{OWNER}"],
            "accesses": [{"type": "all", "isAllowed": true}],
        }],
    }]));

    let d = evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::ResourceOwnerUnknown { policy_id, .. } if *policy_id == Some(1)
        )),
        "{d:?}"
    );

    let mut owned = full_request("alice", "select");
    owned.owner = Some("alice".to_string());
    assert_allowed(&evaluate(&set, &owned, &trino_def(), &no_superusers()));
}

/// **M1**, the second instance of the same shape, found by the reviewer.
/// `AccessTypeCaseMismatch` also built its sets from literal accesses.
/// `RangerOptimizedPolicyEvaluator` warms `accessPerms` from
/// `evaluator.getWithImpliedGrants().getAccesses()`
/// (`checkIfWithImpliedGrantsInitialized`), so for a policy granting `all`
/// the warm set is `{all, select, insert, …}` and its case-SENSITIVE
/// `contains("SELECT")` misses — a warm plugin skips the policy and denies,
/// a cold one matches and allows. egret answered `Allowed` with no cause.
#[test]
fn a_case_mismatch_reached_through_implied_grants_still_raises_the_cause() {
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &["all"])],
    }]));

    let d = evaluate(
        &set,
        &full_request("alice", "SELECT"),
        &trino_def(),
        &no_superusers(),
    );
    assert!(
        causes(&d).iter().any(|c| matches!(
            c,
            UndecidableCause::AccessTypeCaseMismatch { access_type, policy_id, .. }
                if access_type == "select" && *policy_id == Some(1)
        )),
        "{d:?}"
    );

    // (The literal-access half of the same divergence is
    // `an_access_type_matching_only_case_insensitively_is_undecidable`.)
    // The control: spelled as the def spells it, the warm and cold
    // pre-filters agree and the implied grant is decidable.
    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "select"),
        &trino_def(),
        &no_superusers(),
    ));
}

/// `hasAllPerms` must keep reading the LITERAL access set.
/// `checkIfHasAllPerms()` runs inside `RangerOptimizedPolicyEvaluator.init`,
/// immediately after `preprocessPolicyItems` and before any item evaluator
/// has computed its implied grants — so a policy granting only `all` does
/// NOT have all perms as far as that check is concerned, and the pre-filter
/// stays armed. Computing it from the expanded set instead would suppress
/// the cause above and fail OPEN, which is exactly what
/// `a_case_mismatch_reached_through_implied_grants_still_raises_the_cause`
/// would then stop catching.
#[test]
fn has_all_perms_is_computed_from_literal_accesses_not_expanded_ones() {
    // Granting every access type the def declares DOES trip hasAllPerms,
    // so no cause — the pre-filter never engages for such a policy.
    let every: Vec<&str> = vec![
        "select",
        "insert",
        "create",
        "drop",
        "delete",
        "use",
        "alter",
        "grant",
        "revoke",
        "show",
        "impersonate",
        "execute",
        "read_sysinfo",
        "write_sysinfo",
        "all",
    ];
    let set = policies(json!([{
        "id": 1, "name": "p", "service": "trino", "policyType": 0,
        "resources": full_resources(),
        "policyItems": [item(&["alice"], &every)],
    }]));
    assert_allowed(&evaluate(
        &set,
        &full_request("alice", "SELECT"),
        &trino_def(),
        &no_superusers(),
    ));
}
