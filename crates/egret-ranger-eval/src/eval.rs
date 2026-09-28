//! Policy-item evaluation, precedence, and the final three-valued decision.
//!
//! This is the layer that turns [`crate::resource`]'s classification into an
//! answer. It reproduces, for the request shape a Trino or StarRocks access
//! check actually issues:
//!
//! * `RangerPolicyEngineImpl.evaluatePoliciesForOneAccessTypeNoAudit` — the
//!   superuser short-circuit, the evaluator loop, and the priority rules;
//! * `RangerPolicyRepository.{skipBuildingPolicyEvaluator, init}` — which
//!   policies get an evaluator at all, and the order they are evaluated in;
//! * `RangerPolicyEvaluator.PolicyEvalOrderComparator` — that order;
//! * `RangerDefaultPolicyEvaluator.{evaluate, evaluatePolicyItems,
//!   getMatchingPolicyItemForAccessPolicyForSpecificAccess,
//!   getMatchingPolicyItem, updateAccessResult}` — bucket order, exception
//!   veto, `isDenyAllElse`;
//! * `RangerDefaultPolicyItemEvaluator.{isMatch, matchUserGroupAndOwner}` and
//!   `RangerAbstractPolicyItemEvaluator.computeWithImpliedGrants` — principal
//!   matching and implied grants.
//!
//! Read from Apache Ranger at tags `release-ranger-2.5.0` (what Trino 470
//! enforces) and `release-ranger-2.8.0` (what StarRocks fe-ubuntu:4.1
//! enforces), and cross-checked with `javap -c` against the real
//! `ranger-plugins-common-{2.5.0,2.8.0}.jar`.
//!
//! # The contract
//!
//! [`Decision`] is three-valued. The failure this package exists to prevent
//! is a confident WRONG answer, so anything the evaluator cannot model
//! produces [`Decision::Undecidable`] naming the cause — never a guess, and
//! never a silent fallthrough. In particular a policy carrying an
//! [`Unmodelled`] construct makes the decision undecidable **even when
//! another policy would have allowed**: egret cannot know the construct did
//! not change the outcome.
//!
//! # What is NOT modelled, and what happens instead
//!
//! | construct | outcome |
//! |---|---|
//! | `ranger.plugin.super.users` / `.super.groups` unknown to the caller | [`UndecidableCause::SuperUsersUnknown`] |
//! | `{OWNER}` in a matched policy's items with no owner supplied | [`UndecidableCause::ResourceOwnerUnknown`] |
//! | `_any` / `_admin` / `_super_user` access types | [`UndecidableCause::PseudoAccessType`] |
//! | policy or item `conditions`, `validitySchedules`, `zoneName`, a resource-value expression | [`UndecidableCause::PolicyConstruct`] |
//! | `securityZones`, `tagPolicies`, `policyDeltas`, `serviceConfig` | [`UndecidableCause::EnvelopeConstruct`] |
//! | a non-default resource matcher, `quotedCaseSensitive`, chained `impliedGrants` | [`UndecidableCause::ServiceDef`] |
//! | a policy access type that matches the request's only case-insensitively | [`UndecidableCause::AccessTypeCaseMismatch`] |
//!
//! # Two divergences measured in the jars
//!
//! 1. **`RangerDefaultPolicyEvaluator.evaluate` is NOT identical between
//!    2.5.0 and 2.8.0.** 2.5.0 reads
//!    `if (request.isAccessTypeAny()) { isMatched = matchType != NONE; }`,
//!    admitting an `ANCESTOR` or `DESCENDANT` match into the policy items,
//!    and gates the allow later in `updateAccessResult`
//!    (`if (matchType != ANCESTOR)`). 2.8.0 moved the gate into `evaluate`
//!    (`… || (matchType == ANCESTOR && request instanceof RangerTagAccessRequest)`)
//!    and dropped it from `updateAccessResult` entirely; it also gained
//!    `|| !result.getAccessRequest().ignoreDescendantDeny()` on the
//!    `DESCENDANT` deny gate. **For a non-tag request naming a concrete
//!    access type at the default `SELF` scope — which is every Trino and
//!    StarRocks access check — both versions reduce to exactly
//!    `isMatched = SELF || SELF_AND_ALL_DESCENDANTS`, and both
//!    `updateAccessResult` guards are unreachable.** That is the path this
//!    module models, and it needs no plugin-version parameter. The divergent
//!    paths are reachable only through `_any` or `SELF_OR_DESCENDANTS`, which
//!    are [`UndecidableCause::PseudoAccessType`] and not expressible in
//!    [`AccessRequest`] respectively.
//!
//! 2. **`useAclSummaryForEvaluation` does not exist in either version.**
//!    `javap -p` on both jars shows `aclSummary` touched only by the
//!    constructor, `getPolicyACLSummary()` and `createPolicyACLSummary(…)`,
//!    and `javap -c` of `evaluate` never reads it — the ACL-summary
//!    evaluation path was removed before 2.5.0, so
//!    `disableAccessEvaluationWithPolicyACLSummary` cannot change an answer
//!    here.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{
    AccessEntry, ItemBucket, PolicyItem, RangerPolicy, ServicePolicySet, Unmodelled,
};
use crate::resource::{match_policy_resources, MatchType, RequestResource};
use crate::servicedef::{ServiceDef, ServiceDefIssue};

/// `RangerPolicyEngine.GROUP_PUBLIC`. Verified with
/// `javap -constants org.apache.ranger.plugin.policyengine.RangerPolicyEngine`
/// on the 2.5.0 jar.
pub const GROUP_PUBLIC: &str = "public";
/// `RangerPolicyEngine.USER_CURRENT`.
pub const USER_CURRENT: &str = "{USER}";
/// `RangerPolicyEngine.RESOURCE_OWNER`.
pub const RESOURCE_OWNER: &str = "{OWNER}";
/// `RangerPolicyEngine.ANY_ACCESS`.
pub const ANY_ACCESS: &str = "_any";
/// `RangerPolicyEngine.ADMIN_ACCESS`.
pub const ADMIN_ACCESS: &str = "_admin";
/// `RangerPolicyEngine.SUPER_USER_ACCESS`.
pub const SUPER_USER_ACCESS: &str = "_super_user";

/// Ranger's policy id for the superuser short-circuit
/// (`ret.setPolicyId(-1)` in `evaluatePoliciesForOneAccessTypeNoAudit`).
const SUPERUSER_POLICY_ID: i64 = -1;

// ── inputs ───────────────────────────────────────────────────────────────

/// One access request, in the shape a Trino or StarRocks plugin issues:
/// one user, one access type, one resource, at Ranger's default
/// `ResourceMatchingScope.SELF`.
///
/// `groups` and `roles` are INPUTS, not something this crate resolves.
/// (egret runs no usersync and does not serve `/service/roles/download`, so
/// on the engines egret provisions both are empty in practice and `public`
/// is the only group name that can match — but that is a property of
/// egret's deployment, not of Ranger, so it is not baked in here.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessRequest {
    pub user: String,
    pub groups: BTreeSet<String>,
    pub roles: BTreeSet<String>,
    pub access_type: String,
    pub resource: RequestResource,
    /// `RangerAccessResource.getOwnerUser()`, which `{OWNER}` resolves
    /// against. `None` means the caller does not know — which makes a
    /// decision that depends on it [`UndecidableCause::ResourceOwnerUnknown`]
    /// rather than a guess in either direction.
    pub owner: Option<String>,
}

/// Plugin-side configuration the engine consults but a policy download does
/// not carry.
///
/// Both fields are `Option`: `None` means "the caller did not measure this",
/// which is not the same as "empty" and must not be treated as such.
/// `isSuperUser` is checked BEFORE any policy and wins with
/// `policyPriority = Integer.MAX_VALUE`, so an unknown superuser list makes
/// every decision unknowable. Both live Trino instances were measured to
/// set neither (`docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`) —
/// that measurement is what [`EngineConfig::without_super_users`] records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineConfig {
    /// `ranger.plugin.super.users`.
    pub super_users: Option<BTreeSet<String>>,
    /// `ranger.plugin.super.groups`.
    pub super_groups: Option<BTreeSet<String>>,
}

impl EngineConfig {
    /// A plugin measured to declare no superusers and no super-groups.
    pub fn without_super_users() -> Self {
        EngineConfig {
            super_users: Some(BTreeSet::new()),
            super_groups: Some(BTreeSet::new()),
        }
    }
}

// ── outputs ──────────────────────────────────────────────────────────────

/// Why a [`Decision::Denied`] is a deny.
///
/// [`DenyReason::NoPolicyMatched`] is the one Ranger reaches by falling off
/// the end of the evaluator loop with `isAllowed` still false. It is kept
/// distinct because the host plugin may treat it differently from an
/// explicit deny: `RangerPolicyEngineImpl` only marks it
/// `isAccessDetermined` when `getIsFallbackSupported()` is false, and a host
/// that supports a native fallback authorizer consults that flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenyReason {
    /// A `denyPolicyItems` entry matched and no `denyExceptions` entry
    /// cancelled it.
    DenyPolicyItem,
    /// The policy's resources matched, nothing in any bucket did, and
    /// `isDenyAllElse` is set.
    DenyAllElse,
    /// No policy determined anything. Ranger's default.
    NoPolicyMatched,
}

/// Something that stands between this evaluator and a defensible answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndecidableCause {
    /// An envelope-level construct: `securityZones`, `tagPolicies`,
    /// `policyDeltas` or `serviceConfig`. Any of them can change any
    /// decision in the download.
    EnvelopeConstruct(Unmodelled),
    /// A construct on one policy that participates in this decision.
    PolicyConstruct {
        policy_id: Option<i64>,
        policy_name: String,
        construct: Unmodelled,
    },
    /// A construct in the service-def that [`crate::servicedef`] does not
    /// reproduce.
    ServiceDef(ServiceDefIssue),
    /// A matched policy has an item naming `{OWNER}` for this access type,
    /// and [`AccessRequest::owner`] is `None`.
    ResourceOwnerUnknown {
        policy_id: Option<i64>,
        policy_name: String,
    },
    /// [`EngineConfig::super_users`] or [`EngineConfig::super_groups`] is
    /// `None`.
    SuperUsersUnknown,
    /// The request names one of Ranger's pseudo access types (`_any`,
    /// `_admin`, `_super_user`), each of which takes a different branch —
    /// and `_any` is where the 2.5.0 and 2.8.0 `evaluate` methods actually
    /// disagree (see the module doc).
    PseudoAccessType(String),
    /// A matched policy grants an access type that equals the request's
    /// only case-insensitively, and grants none that equals it exactly.
    ///
    /// The evaluator `RangerPolicyRepository.buildPolicyEvaluator` always
    /// constructs is `RangerOptimizedPolicyEvaluator`, whose
    /// `hasMatchablePolicyItem` pre-filter tests
    /// `accessPerms.contains(request.getAccessType())` — a case-SENSITIVE
    /// `HashSet` lookup — while the item match it approximates uses
    /// `StringUtils.equalsIgnoreCase`. The pre-filter is armed only once
    /// `checkIfAllEvaluatorsInitialized()` is true, which happens after
    /// every item evaluator has computed its implied grants. So a cold
    /// plugin matches this policy and a warm one skips it. Warm-up state is
    /// not a function of the request, so there is no right answer to pick.
    AccessTypeCaseMismatch {
        policy_id: Option<i64>,
        policy_name: String,
        access_type: String,
    },
}

/// The three-valued result. See the module doc for the contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allowed {
        policy_id: Option<i64>,
        policy_name: String,
    },
    Denied {
        policy_id: Option<i64>,
        policy_name: String,
        reason: DenyReason,
    },
    Undecidable {
        why: Vec<UndecidableCause>,
    },
}

// ── the evaluator ────────────────────────────────────────────────────────

/// Evaluate one access request against one policy download.
///
/// Pure: no I/O, no clock, no globals. `service_def` is normally
/// [`ServiceDef::parse`] of `policies.service_def`, but is taken separately
/// so a caller can supply the embedded def for a download whose envelope
/// omitted one.
pub fn evaluate(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    service_def: &ServiceDef,
    engine: &EngineConfig,
) -> Decision {
    let mut why: Vec<UndecidableCause> = Vec::new();

    // ── the superuser short-circuit, first, exactly as the engine does ──
    //
    //   if (isSuperUser || accessType.equals(SUPER_USER_ACCESS)) {
    //       ret.setIsAllowed(isSuperUser);
    //       ret.setIsAccessDetermined(true);
    //       ret.setPolicyId(-1);
    //       ret.setPolicyPriority(Integer.MAX_VALUE);
    //   }
    //
    // MAX_VALUE beats every policy priority, and the resource-policy loop's
    // `ret.getPolicyPriority() > evaluator.getPolicyPriority()` check then
    // determines the result before the first evaluator's items run — so a
    // superuser allow is final and is reported without weighing anything
    // else, unmodelled constructs included.
    let mut super_config_known = true;
    let mut is_super_user = false;
    match &engine.super_users {
        Some(users) => is_super_user |= users.contains(&request.user),
        None => super_config_known = false,
    }
    match &engine.super_groups {
        // `hasSuperGroup` requires a non-empty request group set, which
        // `is_disjoint` gives for free.
        Some(groups) => is_super_user |= !groups.is_disjoint(&request.groups),
        None => super_config_known = false,
    }
    if is_super_user {
        return Decision::Allowed {
            policy_id: Some(SUPERUSER_POLICY_ID),
            policy_name: "superuser".to_string(),
        };
    }
    if !super_config_known {
        why.push(UndecidableCause::SuperUsersUnknown);
    }

    // ── request-shape constructs this module does not model ────────────
    if matches!(
        request.access_type.as_str(),
        ANY_ACCESS | ADMIN_ACCESS | SUPER_USER_ACCESS
    ) {
        why.push(UndecidableCause::PseudoAccessType(
            request.access_type.clone(),
        ));
    }

    // ── envelope- and service-def-level constructs ─────────────────────
    for construct in &policies.unmodelled {
        why.push(UndecidableCause::EnvelopeConstruct(construct.clone()));
    }
    for issue in service_def.issues() {
        if issue_affects_access(issue) {
            why.push(UndecidableCause::ServiceDef(issue.clone()));
        }
    }

    let implied = service_def.implied_access_grants();
    let all_access_types: BTreeSet<&str> = service_def
        .access_types()
        .iter()
        .map(|a| a.name.as_str())
        .collect();

    // ── which policies get an evaluator ────────────────────────────────
    //
    // `RangerPolicyRepository.skipBuildingPolicyEvaluator` drops a disabled
    // policy before an evaluator exists, and `init` files the rest by
    // policy type: null/0 into `policyEvaluators` (the only list
    // `evaluatePoliciesForOneAccessTypeNoAudit` reads for
    // POLICY_TYPE_ACCESS), 1 into `dataMaskPolicyEvaluators`, 2 into
    // `rowFilterPolicyEvaluators`, and anything else nowhere at all
    // ("ignoring policy id=… - invalid policyType"). So a masking policy
    // cannot deny an access request, and its unmodelled constructs cannot
    // make one undecidable.
    let mut candidates: Vec<PolicyView<'_>> = policies
        .policies
        .iter()
        .filter(|p| p.is_enabled)
        // `policy_type == 0` is the load-bearing test: `init` files a
        // policy by its RAW code, and anything that is not null/0/1/2 is
        // dropped with a warning rather than folded onto access. The
        // `PolicyType::from_code` call alongside it would fold code 3
        // (audit) and every unknown code ONTO access, which is right for
        // picking a resource hierarchy and wrong here — so it is the
        // raw-code test that decides, and the typed one is not consulted.
        .filter(|p| p.policy_type == 0)
        .map(|policy| PolicyView::build(policy, request, service_def))
        .collect();

    for view in &candidates {
        view.collect_causes(request, &implied, &all_access_types, &mut why);
    }

    if !why.is_empty() {
        return Decision::Undecidable { why };
    }

    // ── evaluation order ───────────────────────────────────────────────
    //
    // `Collections.sort(policyEvaluators, EVAL_ORDER_COMPARATOR)` with
    // `PolicyEvalOrderComparator`:
    //
    //   Integer.compare(other.getPolicyPriority(), me.getPolicyPriority())   // DESC
    //   then: hasDeny() first
    //   then: Integer.compare(me.getEvalOrder(), other.getEvalOrder())
    //   then: compareStrings(policy name)
    //
    // The first two keys are load-bearing: the priority order is what makes
    // a higher-priority allow final, and the hasDeny-first order is what
    // makes a deny beat an allow at EQUAL priority (a non-deny policy that
    // allows sets `isAccessDetermined` through
    // `if (ret.getIsAllowed()) { if (!evaluator.hasDeny()) … }`, which would
    // otherwise hide a same-priority deny behind it).
    //
    // `getEvalOrder()` is `RangerOptimizedPolicyEvaluator.computeEvalOrder()`,
    // a resource-specificity score, and is NOT reproduced here. Within one
    // (priority, hasDeny) band the allow/deny OUTCOME does not depend on
    // order: in an all-hasDeny band no evaluator can set
    // `isAccessDetermined` except by denying, so every one of them runs and
    // any deny wins; in an all-non-deny band the first allow wins and the
    // rest cannot deny. Only the reported `policy_id` of an allow can
    // differ, which is why a stable name-then-id tie-break stands in for it.
    candidates.sort_by(|a, b| {
        b.policy
            .policy_priority
            .cmp(&a.policy.policy_priority)
            .then_with(|| b.has_deny.cmp(&a.has_deny))
            .then_with(|| a.policy.name.cmp(&b.policy.name))
            .then_with(|| a.policy.id.cmp(&b.policy.id))
    });

    // ── the evaluator loop ─────────────────────────────────────────────
    //
    // `evaluatePoliciesForOneAccessTypeNoAudit`, reduced to the
    // single-access-type case. (It is always the single-access-type case
    // here: `RangerAccessRequestUtil.getAllRequestedAccessTypes` falls back
    // to `Collections.singleton(request.getAccessType())`, so
    // `evaluatePolicyItems` always takes its `accessTypeResults` branch with
    // exactly one entry, and `getCompositeAccessResult` copies that entry
    // straight back into the shared result.)
    let mut outcome: Option<Outcome> = None;
    for view in &candidates {
        // Top of the loop: an allow recorded by a strictly higher-priority
        // policy is final.
        //
        //   } else if (ret.getIsAllowed()) {
        //       if (policyType == POLICY_TYPE_ACCESS) {
        //           if (ret.getPolicyPriority() > evaluator.getPolicyPriority()) {
        //               ret.setIsAccessDetermined(true);
        if let Some(current) = &outcome {
            if current.allowed && current.priority > view.policy.policy_priority {
                break;
            }
        }

        if let Some(found) = view.decide(request, &implied) {
            match &outcome {
                None => outcome = Some(found),
                Some(current) => {
                    // The `accessTypeResults` arbitration. A deny of the
                    // same or higher priority replaces an allow; an allow
                    // needs a STRICTLY higher priority to replace an allow.
                    // (The `old is a deny` branches are unreachable: a deny
                    // sets `isAccessDetermined` and the loop stops below.)
                    let replace = if current.allowed && !found.allowed {
                        found.priority >= current.priority
                    } else if current.allowed && found.allowed {
                        found.priority > current.priority
                    } else {
                        false
                    };
                    if replace {
                        outcome = Some(found);
                    }
                }
            }
        }

        // `setIsAllowed(false)` sets `isAccessDetermined`, so a deny ends
        // the evaluation; and once allowed, the first evaluator with no
        // deny items of its own does too.
        match &outcome {
            Some(o) if !o.allowed => break,
            Some(o) if o.allowed && !view.has_deny => break,
            _ => {}
        }
    }

    match outcome {
        Some(Outcome {
            allowed: true,
            policy_id,
            policy_name,
            ..
        }) => Decision::Allowed {
            policy_id,
            policy_name,
        },
        Some(Outcome {
            allowed: false,
            policy_id,
            policy_name,
            reason,
            ..
        }) => Decision::Denied {
            policy_id,
            policy_name,
            reason,
        },
        None => Decision::Denied {
            policy_id: None,
            policy_name: String::new(),
            reason: DenyReason::NoPolicyMatched,
        },
    }
}

#[derive(Debug, Clone)]
struct Outcome {
    allowed: bool,
    priority: i32,
    policy_id: Option<i64>,
    policy_name: String,
    reason: DenyReason,
}

/// One candidate access policy, with the parts of its evaluator state that
/// do not depend on which bucket is being asked.
struct PolicyView<'a> {
    policy: &'a RangerPolicy,
    /// `RangerAbstractPolicyEvaluator.hasDeny()`:
    /// `policy.getIsDenyAllElse() || isNotEmpty(policy.getDenyPolicyItems())`.
    has_deny: bool,
    /// `hasAllow()`: `isNotEmpty(policy.getPolicyItems())`.
    has_allow: bool,
    /// Whether any of the policy's resource maps covers the request. See
    /// [`PolicyView::build`] for why `SELF`/`SELF_AND_ALL_DESCENDANTS` is
    /// the whole test.
    matched: bool,
}

impl<'a> PolicyView<'a> {
    fn build(
        policy: &'a RangerPolicy,
        request: &AccessRequest,
        service_def: &ServiceDef,
    ) -> PolicyView<'a> {
        // `RangerDefaultPolicyEvaluator.evaluate` loops over one resource
        // evaluator per resource map and asks each for a match type. For a
        // non-tag request naming a concrete access type at the default
        // `SELF` scope, both 2.5.0 and 2.8.0 reduce to
        // `isMatched = SELF || SELF_AND_ALL_DESCENDANTS` — see the module
        // doc for the two versions' differing routes to that same test.
        // ANCESTOR in particular is NOT an allow, which a live Trino
        // measurement confirmed (`…-sp4a-live-result.md` §3 T1).
        let matched = match_policy_resources(policy, &request.resource, service_def)
            .into_iter()
            .any(|m| {
                matches!(
                    m.match_type,
                    MatchType::Self_ | MatchType::SelfAndAllDescendants
                )
            });

        PolicyView {
            policy,
            has_deny: policy.is_deny_all_else || !policy.deny_policy_items.is_empty(),
            has_allow: !policy.policy_items.is_empty(),
            matched,
        }
    }

    /// Everything about this policy that stops the decision being
    /// defensible, appended to `why`.
    fn collect_causes(
        &self,
        request: &AccessRequest,
        implied: &BTreeMap<&str, BTreeSet<&str>>,
        all_access_types: &BTreeSet<&str>,
        why: &mut Vec<UndecidableCause>,
    ) {
        let mut push = |construct: &Unmodelled| {
            why.push(UndecidableCause::PolicyConstruct {
                policy_id: self.policy.id,
                policy_name: self.policy.name.clone(),
                construct: construct.clone(),
            });
        };

        for construct in &self.policy.unmodelled {
            // A construct that decides whether the policy MATCHES has to be
            // reported whether or not `matched` came out true — `matched`
            // is exactly what it makes unreliable.
            let always = matches!(
                construct,
                Unmodelled::ResourceValueExpressionMarker { .. } | Unmodelled::ZoneName(_)
            );
            if always || (self.matched && construct_affects_access(construct)) {
                push(construct);
            }
        }

        if !self.matched {
            return;
        }

        // `{OWNER}` with no owner supplied.
        //
        // REVIEW ROUND 1, CRITICAL 1 (fail-open): this used to re-implement
        // the test rather than run it — it compared the item's LITERAL
        // `accesses[].type` against the request's access type while the
        // thing it guards, [`item_is_match`], matches through
        // `computeWithImpliedGrants`. A deny item granting `all` to
        // `{OWNER}` therefore DECIDED a `select` request on `trino.json`
        // while being invisible here, and egret answered `Allowed` where
        // Ranger, given the owner, denies.
        //
        // The fix is structural, not a patched predicate: [`item_is_match`]
        // is now three-valued and IS the detector, so there is exactly one
        // place an item's access list is tested and the guard cannot drift
        // from the guarded again.
        let owner_unknown = access_buckets(self.policy).any(|items| {
            items
                .iter()
                .any(|item| item_is_match(item, request, implied) == ItemMatch::NeedsOwner)
        });
        if owner_unknown {
            why.push(UndecidableCause::ResourceOwnerUnknown {
                policy_id: self.policy.id,
                policy_name: self.policy.name.clone(),
            });
        }

        // The warm/cold pre-filter divergence. `accessPerms` is the union
        // over ALL SIX buckets, and `checkIfHasAllPerms` short-circuits the
        // whole test when the policy grants every access type the def
        // declares (or sets `isDenyAllElse`).
        //
        // REVIEW ROUND 1, M1 — the same blind spot as CRITICAL 1. The two
        // sets below are DIFFERENT on purpose, and each is pinned to the
        // moment of Ranger's own that it models:
        //
        // * `literal_granted` is `accessPerms` as `preprocessPolicyItems`
        //   leaves it at `init` time: the raw `policyItemAccess.getType()`
        //   of every allowed access. `checkIfHasAllPerms()` is called from
        //   `init` immediately after, BEFORE any item evaluator has
        //   computed its implied grants, so `hasAllPerms` is a fact about
        //   the literal set. Computing it from the expanded set would make
        //   it true more often, suppress this cause, and fail OPEN.
        // * `expanded_granted` is `accessPerms` once WARM:
        //   `checkIfWithImpliedGrantsInitialized` folds
        //   `evaluator.getWithImpliedGrants().getAccesses()` in as each
        //   evaluator initialises. That is the set the pre-filter's
        //   case-SENSITIVE `contains(request.getAccessType())` actually
        //   tests, so it is the one the exact/case-insensitive comparison
        //   has to use.
        if !self.policy.is_deny_all_else {
            let mut literal_granted: BTreeSet<&str> = BTreeSet::new();
            let mut expanded_granted: BTreeSet<String> = BTreeSet::new();
            for items in all_buckets(self.policy) {
                for item in items {
                    for access in &item.accesses {
                        if access.is_allowed {
                            literal_granted.insert(access.access_type.as_str());
                        }
                    }
                    for access in with_implied_grants(item, implied) {
                        if access.is_allowed {
                            expanded_granted.insert(access.access_type);
                        }
                    }
                }
            }
            let has_all_perms = !all_access_types.is_empty()
                && all_access_types.iter().all(|a| literal_granted.contains(a));
            let exact = expanded_granted.contains(&request.access_type);
            let case_insensitive = expanded_granted
                .iter()
                .find(|a| {
                    a.eq_ignore_ascii_case(&request.access_type) && **a != request.access_type
                })
                .cloned();
            if let Some(access_type) = case_insensitive {
                if !exact && !has_all_perms {
                    why.push(UndecidableCause::AccessTypeCaseMismatch {
                        policy_id: self.policy.id,
                        policy_name: self.policy.name.clone(),
                        access_type,
                    });
                }
            }
        }
    }

    /// `RangerDefaultPolicyEvaluator.evaluatePolicyItems` for one policy:
    /// what, if anything, this policy determines.
    fn decide(
        &self,
        request: &AccessRequest,
        implied: &BTreeMap<&str, BTreeSet<&str>>,
    ) -> Option<Outcome> {
        if !self.matched {
            return None;
        }
        // `hasMatchablePolicyItem(request)` — a policy carrying only
        // exceptions never reaches `evaluatePolicyItems`.
        if !(self.has_allow || self.has_deny) {
            return None;
        }

        // `getMatchingPolicyItemForAccessPolicyForSpecificAccess`: the DENY
        // pair first, the ALLOW pair only if it found nothing. An exception
        // sets `ret = null` — it VETOES the match rather than competing
        // with it, so a cancelled deny is not an allow; it falls through
        // here to the allow bucket, which must find its own item.
        let denied = matching_item(
            &self.policy.deny_policy_items,
            &self.policy.deny_exceptions,
            request,
            implied,
        );
        if denied {
            return Some(Outcome {
                allowed: false,
                priority: self.policy.policy_priority,
                policy_id: self.policy.id,
                policy_name: self.policy.name.clone(),
                reason: DenyReason::DenyPolicyItem,
            });
        }

        let allowed = matching_item(
            &self.policy.policy_items,
            &self.policy.allow_exceptions,
            request,
            implied,
        );
        if allowed {
            return Some(Outcome {
                allowed: true,
                priority: self.policy.policy_priority,
                policy_id: self.policy.id,
                policy_name: self.policy.name.clone(),
                reason: DenyReason::NoPolicyMatched,
            });
        }

        // `} else if (getPolicy().getIsDenyAllElse() && (policyType == null
        //   || policyType == POLICY_TYPE_ACCESS))` — the policy-type half of
        // the guard is already satisfied: only access policies reach here.
        if self.policy.is_deny_all_else {
            return Some(Outcome {
                allowed: false,
                priority: self.policy.policy_priority,
                policy_id: self.policy.id,
                policy_name: self.policy.name.clone(),
                reason: DenyReason::DenyAllElse,
            });
        }

        None
    }
}

/// `RangerDefaultPolicyEvaluator.getMatchingPolicyItem(request, evaluators,
/// exceptionEvaluators)`: the first matching item in `items`, then vetoed
/// outright if any item in `exceptions` also matches.
fn matching_item(
    items: &[PolicyItem],
    exceptions: &[PolicyItem],
    request: &AccessRequest,
    implied: &BTreeMap<&str, BTreeSet<&str>>,
) -> bool {
    if !items
        .iter()
        .any(|i| item_is_match(i, request, implied) == ItemMatch::Yes)
    {
        return false;
    }
    !exceptions
        .iter()
        .any(|e| item_is_match(e, request, implied) == ItemMatch::Yes)
}

/// `RangerDefaultPolicyItemEvaluator.isMatch`, for a request naming a
/// concrete (non-`_admin`) access type:
///
/// ```java
/// if (matchUserGroupAndOwner(request)) {
///     …
///     for (RangerPolicyItemAccess access : withImpliedGrants.getAccesses()) {
///         if (access.getIsAllowed() && StringUtils.equalsIgnoreCase(access.getType(), request.getAccessType())) {
///             isAccessTypeMatched = true; break;
///         }
///     }
///     if (isAccessTypeMatched) { if (matchCustomConditions(request)) { ret = true; } }
/// }
/// ```
///
/// `matchCustomConditions` is vacuously true here: an item carrying
/// `conditions` has already forced [`Decision::Undecidable`].
fn item_is_match(
    item: &PolicyItem,
    request: &AccessRequest,
    implied: &BTreeMap<&str, BTreeSet<&str>>,
) -> ItemMatch {
    let principal = match_user_group_and_owner(item, request);
    if principal == PrincipalMatch::No {
        return ItemMatch::No;
    }
    // THE one place an item's access list is tested, and it always goes
    // through `computeWithImpliedGrants`. Review round 1 found two callers
    // that had re-implemented this comparison against the LITERAL
    // `accesses[]` instead; both now call this function rather than
    // repeating it.
    let access_matched = with_implied_grants(item, implied)
        .iter()
        .any(|a| a.is_allowed && a.access_type.eq_ignore_ascii_case(&request.access_type));
    if !access_matched {
        return ItemMatch::No;
    }
    match principal {
        PrincipalMatch::Yes => ItemMatch::Yes,
        PrincipalMatch::OwnerUnknown => ItemMatch::NeedsOwner,
        PrincipalMatch::No => unreachable!("returned above"),
    }
}

/// Whether one policy item matches a request — three-valued, because
/// "does not match" and "cannot be told apart from matching without a
/// resource owner the caller did not supply" are different answers and only
/// the first may be acted on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemMatch {
    No,
    Yes,
    /// The item names `{OWNER}`, nothing earlier in
    /// [`match_user_group_and_owner`]'s order matched, the access type DOES
    /// match, and [`AccessRequest::owner`] is `None`.
    NeedsOwner,
}

/// The principal half of [`item_is_match`], three-valued for the same
/// reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrincipalMatch {
    No,
    Yes,
    OwnerUnknown,
}

/// `RangerDefaultPolicyItemEvaluator.matchUserGroupAndOwner`, verbatim
/// order and verbatim short-circuiting:
///
/// ```java
/// if (!ret && user != null && policyItem.getUsers() != null) {
///     ret = hasCurrentUser || policyItem.getUsers().contains(user);
/// }
/// if (!ret && userGroups != null && policyItem.getGroups() != null) {
///     ret = policyItem.getGroups().contains(GROUP_PUBLIC) || !Collections.disjoint(policyItem.getGroups(), userGroups);
/// }
/// if (!ret && isNotEmpty(roles) && isNotEmpty(policyItem.getRoles())) {
///     ret = !Collections.disjoint(policyItem.getRoles(), roles);
/// }
/// if (!ret && hasResourceOwner) { ret = user != null && user.equals(owner); }
/// ```
///
/// `hasCurrentUser` / `hasResourceOwner` come from `init`:
/// `users.contains(USER_CURRENT)` / `users.contains(RESOURCE_OWNER)` — a
/// `List.contains`, so both macros are matched EXACTLY, not
/// case-insensitively. Every other comparison here is exact too; only the
/// access type is `equalsIgnoreCase`.
fn match_user_group_and_owner(item: &PolicyItem, request: &AccessRequest) -> PrincipalMatch {
    let has_current_user = item.users.iter().any(|u| u == USER_CURRENT);
    if has_current_user || item.users.contains(&request.user) {
        return PrincipalMatch::Yes;
    }
    if item.groups.iter().any(|g| g == GROUP_PUBLIC)
        || item.groups.iter().any(|g| request.groups.contains(g))
    {
        return PrincipalMatch::Yes;
    }
    if !request.roles.is_empty()
        && !item.roles.is_empty()
        && item.roles.iter().any(|r| request.roles.contains(r))
    {
        return PrincipalMatch::Yes;
    }
    let has_resource_owner = item.users.iter().any(|u| u == RESOURCE_OWNER);
    if has_resource_owner {
        return match request.owner.as_deref() {
            Some(owner) => {
                if owner == request.user {
                    PrincipalMatch::Yes
                } else {
                    PrincipalMatch::No
                }
            }
            // Reached only when every earlier clause missed, which is
            // exactly when the owner decides — so this is the one state the
            // caller must not resolve by guessing.
            None => PrincipalMatch::OwnerUnknown,
        };
    }
    PrincipalMatch::No
}

/// `RangerAbstractPolicyItemEvaluator.computeWithImpliedGrants`, one round.
///
/// Ranger's own comment: "Only one round of 'expansion' is done;
/// multi-level impliedGrants … are not handled for now". Its loop
/// nevertheless reads `getAccess(ret, implyingAccessType)` from the list it
/// is appending to while iterating a `HashMap`, so a CHAIN can reach a
/// second level depending on that map's unspecified iteration order. This
/// version reads the implying access type from the item's ORIGINAL accesses,
/// which is order-independent and identical to Ranger for any def without a
/// chain; a def WITH one is reported as
/// [`ServiceDefIssue::ChainedImpliedGrants`] and never evaluated.
fn with_implied_grants(
    item: &PolicyItem,
    implied: &BTreeMap<&str, BTreeSet<&str>>,
) -> Vec<AccessEntry> {
    let mut accesses = item.accesses.clone();
    if accesses.is_empty() || implied.is_empty() {
        return accesses;
    }
    for (implying, grants) in implied {
        // `RangerDefaultPolicyEvaluator.getAccess` is `equalsIgnoreCase`.
        let Some(source) = item
            .accesses
            .iter()
            .find(|a| a.access_type.eq_ignore_ascii_case(implying))
        else {
            continue;
        };
        let is_allowed = source.is_allowed;
        for grant in grants {
            match accesses
                .iter_mut()
                .find(|a| a.access_type.eq_ignore_ascii_case(grant))
            {
                Some(existing) => {
                    if !existing.is_allowed {
                        existing.is_allowed = is_allowed;
                    }
                }
                None => accesses.push(AccessEntry {
                    access_type: (*grant).to_string(),
                    is_allowed,
                }),
            }
        }
    }
    accesses
}

/// The four buckets an ACCESS policy's evaluation reads.
/// `getMatchingPolicyItem` switches on the policy type and reaches
/// `dataMaskEvaluators` / `rowFilterEvaluators` only for policy types 1 / 2.
fn access_buckets(policy: &RangerPolicy) -> impl Iterator<Item = &[PolicyItem]> {
    [
        policy.policy_items.as_slice(),
        policy.deny_policy_items.as_slice(),
        policy.allow_exceptions.as_slice(),
        policy.deny_exceptions.as_slice(),
    ]
    .into_iter()
}

/// All six buckets — what `RangerOptimizedPolicyEvaluator.preprocessPolicyItems`
/// folds into its `accessPerms` pre-filter.
fn all_buckets(policy: &RangerPolicy) -> impl Iterator<Item = &[PolicyItem]> {
    [
        policy.policy_items.as_slice(),
        policy.deny_policy_items.as_slice(),
        policy.allow_exceptions.as_slice(),
        policy.deny_exceptions.as_slice(),
        policy.data_mask_policy_items.as_slice(),
        policy.row_filter_policy_items.as_slice(),
    ]
    .into_iter()
}

/// Whether an [`Unmodelled`] construct can change an ACCESS decision.
///
/// The three that cannot are deliberate, and each is justified by a method
/// this module reproduces rather than by convenience:
///
/// * **mask / row-filter constructs** live in `dataMaskPolicyItems` and
///   `rowFilterPolicyItems`, which `getMatchingPolicyItem` reads only for
///   `POLICY_TYPE_DATAMASK` / `POLICY_TYPE_ROWFILTER`. (Task 6 evaluates
///   those; for them these constructs DO decide.) A CUSTOM mask always
///   carries a `{col}` marker, so treating them as poisoning would make
///   every access decision on a masked column undecidable for no reason.
/// * **[`Unmodelled::ExplicitDeny`]** (`isAllowed: false` inside an allow
///   bucket) is modelled exactly: [`item_is_match`] tests
///   `access.getIsAllowed()` the way `isMatch` does.
/// * **[`Unmodelled::DelegateAdmin`]** is read only by
///   `request.isAccessTypeDelegatedAdmin()` — i.e. for the `_admin` access
///   type, which is already [`UndecidableCause::PseudoAccessType`].
///
/// [`Unmodelled::UserExpressionMarker`] is likewise modelled: the item
/// evaluator does a plain `List.contains` on `users` and resolves nothing,
/// so `{USER}` and `{OWNER}` (the only two macro literals that exist —
/// measured by scanning the whole 2.5.0 jar) are handled by
/// [`match_user_group_and_owner`] and anything else is a literal that
/// matches nobody, exactly as Ranger treats it.
fn construct_affects_access(construct: &Unmodelled) -> bool {
    match construct {
        Unmodelled::ValiditySchedules
        | Unmodelled::PolicyConditions
        | Unmodelled::ZoneName(_)
        | Unmodelled::ResourceValueExpressionMarker { .. } => true,
        Unmodelled::ItemConditions { bucket, .. } => matches!(
            bucket,
            ItemBucket::PolicyItems
                | ItemBucket::DenyPolicyItems
                | ItemBucket::AllowExceptions
                | ItemBucket::DenyExceptions
        ),
        // Envelope-scoped; reported from `policies.unmodelled`, never here.
        Unmodelled::SecurityZones
        | Unmodelled::TagPolicies
        | Unmodelled::PolicyDeltas
        | Unmodelled::ServiceConfig => true,
        Unmodelled::MaskExpressionMarker { .. }
        | Unmodelled::MaskConditionExpr { .. }
        | Unmodelled::RowFilterExpressionMarker { .. }
        | Unmodelled::UserExpressionMarker { .. }
        | Unmodelled::ExplicitDeny { .. }
        | Unmodelled::DelegateAdmin { .. } => false,
    }
}

/// Whether a [`ServiceDefIssue`] can change an ACCESS decision.
///
/// [`ServiceDefIssue::InvalidResourceGraph`] is excluded because it is
/// MODELLED, not unmodelled: `RangerServiceDefHelper` stores
/// `EMPTY_RESOURCE_HIERARCHY` for a graph `isValid` rejects, and
/// [`crate::servicedef`] reproduces that by yielding no hierarchies — so
/// every policy of that type matches nothing, which is the live plugin's
/// own answer.
fn issue_affects_access(issue: &ServiceDefIssue) -> bool {
    !matches!(issue, ServiceDefIssue::InvalidResourceGraph { .. })
}
