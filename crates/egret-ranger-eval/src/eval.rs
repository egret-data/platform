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
    has_double_brackets, has_request_expression, AccessEntry, ItemBucket, PolicyItem, RangerPolicy,
    ServicePolicySet, Unmodelled,
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

/// Which engine's Ranger plugin a decision is being reproduced for.
///
/// The engine is NOT a synonym for the service-def — the def is a separate
/// input, because a download carries its own. It is a cross-check: the two
/// engines' resource vocabularies differ (Trino's middle level is `schema`,
/// StarRocks' is `database`) and their access-type vocabularies are
/// partly disjoint (`use` on Trino only, `update` on StarRocks only), so
/// pairing the wrong def with an engine answers "nothing matched" with full
/// confidence. Declaring the engine turns that into
/// [`UndecidableCause::EngineMismatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineType {
    Trino,
    StarRocks,
}

impl EngineType {
    /// The service-def `name` this engine's plugin downloads policies for.
    pub fn service_def_name(self) -> &'static str {
        match self {
            EngineType::Trino => "trino",
            EngineType::StarRocks => "starrocks",
        }
    }

    /// The `ranger-plugins-common` version the engine egret provisions
    /// actually loads, measured on the running pods and recorded in
    /// `docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`:
    /// `trinodb/trino:470` ships 2.5.0, `starrocks/fe-ubuntu:4.1-latest`
    /// ships 2.8.0.
    pub fn plugin_version(self) -> PluginVersion {
        match self {
            EngineType::Trino => PluginVersion::Ranger2_5_0,
            EngineType::StarRocks => PluginVersion::Ranger2_8_0,
        }
    }
}

/// The `ranger-plugins-common` version whose behaviour a decision is
/// reproducing.
///
/// Only the two versions this crate has actually been read against exist as
/// variants, so a version it has NOT been verified against cannot be
/// expressed — an unrepresentable state needs no runtime guard.
///
/// Every fidelity claim in this module was checked in both jars. On the
/// path a Trino or StarRocks check takes — a non-tag request, a concrete
/// access type, `ResourceMatchingScope.SELF` — they reduce identically for
/// access (Task 5) and, measured for this task, for masking and
/// row-filtering too: `RangerDefaultPolicyEvaluator.evaluate` is shared by
/// all three policy types and admits exactly `SELF | SELF_AND_ALL_DESCENDANTS`
/// in both; `getMatchingPolicyItem`'s `tableswitch`, the single-list
/// overload, `RangerDefault{DataMask,RowFilter}PolicyItemEvaluator`'s
/// constructor and `updateAccessResult`, `RangerRequestExprResolver.PATTERN`
/// and `JavaScriptEdits` are byte-identical apart from the
/// `commons-lang` → `commons-lang3` move in `StringUtils`, which changes no
/// behaviour here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginVersion {
    Ranger2_5_0,
    Ranger2_8_0,
}

/// Plugin-side configuration the engine consults but a policy download does
/// not carry.
///
/// Every field is an `Option`: `None` means "the caller did not measure
/// this", which is not the same as "empty" and must not be treated as such.
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
    /// Which engine's plugin this is. `None` leaves the cross-check off.
    pub engine: Option<EngineType>,
    /// Which `ranger-plugins-common` it loads. `None` leaves the
    /// cross-check off; `Some` must agree with
    /// [`EngineType::plugin_version`] when [`EngineConfig::engine`] is also
    /// set.
    pub plugin_version: Option<PluginVersion>,
}

impl EngineConfig {
    /// A plugin measured to declare no superusers and no super-groups.
    pub fn without_super_users() -> Self {
        EngineConfig {
            super_users: Some(BTreeSet::new()),
            super_groups: Some(BTreeSet::new()),
            engine: None,
            plugin_version: None,
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
    /// A matched mask policy's `dataMaskInfo.valueExpr` or `.conditionExpr`
    /// carries a `${{ … }}` request-time expression, so
    /// `RangerDefaultDataMaskPolicyItemEvaluator` reports
    /// `resolver.resolveExpressions(request)` rather than the literal — a
    /// resolution against request context (user attributes, tags, the
    /// UserStore) that egret does not have.
    ///
    /// The predicate is [`crate::model::has_request_expression`], i.e.
    /// Ranger's own `hasExpressions`, and NOT the broader
    /// `Unmodelled::MaskExpressionMarker` — see that function's doc for why
    /// a CUSTOM mask's `{col}` is a literal here.
    MaskExpression {
        policy_id: Option<i64>,
        policy_name: String,
        item_index: usize,
        expr: String,
    },
    /// The row-filter counterpart:
    /// `rowFilterInfo.filterExpr` carries a `${{ … }}` expression.
    RowFilterExpression {
        policy_id: Option<i64>,
        policy_name: String,
        item_index: usize,
        expr: String,
    },
    /// A matched row-filter policy's `filterExpr` satisfies
    /// `JavaScriptEdits.hasDoubleBrackets`, so the evaluator's constructor
    /// rewrites it with `replaceDoubleBrackets` before anything else sees
    /// it. This crate carries no regex engine and does not reproduce the
    /// rewrite.
    RowFilterDoubleBrackets {
        policy_id: Option<i64>,
        policy_name: String,
        item_index: usize,
        expr: String,
    },
    /// Two or more mask (or row-filter) policies tie at the same
    /// `(policyPriority, hasDeny)` and produce DIFFERENT results, so which
    /// one Ranger reports is decided by
    /// `RangerOptimizedPolicyEvaluator.getEvalOrder()` — a
    /// resource-specificity score this crate does not reproduce.
    ///
    /// For an ACCESS decision the same gap is harmless: within one band the
    /// allow/deny outcome does not depend on order (see the sort in
    /// [`evaluate`]). For a mask it is not, because the engine's
    /// arbitration for policy types 1 and 2 is `>=`, not `>`
    /// (`evaluatePoliciesForOneAccessTypeNoAudit`, `if_icmplt` at offset
    /// 529 versus `if_icmple` at 505) — the FIRST matching evaluator is
    /// final and the others never run, so the reported mask type is a
    /// function of an order this crate cannot compute.
    EvalOrderAmbiguous {
        policy_ids: Vec<Option<i64>>,
        policy_names: Vec<String>,
    },
    /// [`EngineConfig::engine`] names one engine and the service-def's
    /// `name` is another's (or is absent). The two engines' resource and
    /// access-type vocabularies differ, so evaluating against the wrong def
    /// silently produces "nothing matched".
    EngineMismatch {
        declared: EngineType,
        service_def_name: String,
    },
    /// [`EngineConfig::plugin_version`] disagrees with the version measured
    /// for [`EngineConfig::engine`] ([`EngineType::plugin_version`]). The
    /// deployment being described is not one this crate has been read
    /// against.
    PluginVersionMismatch {
        engine: EngineType,
        declared: PluginVersion,
        measured: PluginVersion,
    },
}

// ── mask and row-filter results ──────────────────────────────────────────

/// What a masking decision produced: the three fields
/// `RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult` writes
/// onto `RangerAccessResult`, verbatim.
///
/// A mask is DATA, not a verdict. The same method reports `isAllowed = true`
/// unconditionally (the `iconst_1` it passes to
/// `RangerPolicyEvaluator.updateAccessResult`), so the allow half carries no
/// information and collapsing a mask into allow/deny would throw away the
/// only part that does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskResult {
    pub policy_id: Option<i64>,
    pub policy_name: String,
    /// `setMaskType(dataMaskInfo.getDataMaskType())`. Never resolved.
    pub mask_type: String,
    /// `setMaskedValue(...)`: `dataMaskInfo.getValueExpr()` verbatim.
    /// `None` when the policy carries none — Ranger calls
    /// `setMaskedValue(null)`.
    pub masked_value: Option<String>,
    /// `setMaskCondition(...)`: `dataMaskInfo.getConditionExpr()` verbatim.
    pub mask_condition: Option<String>,
}

/// The three-valued masking result. See [`Decision`] for the contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaskDecision {
    /// `RangerAccessResult.getMaskType()` stays null: no mask policy
    /// determined anything and the host applies no transform. Distinct from
    /// a `MASK_NONE` mask, which is a policy deciding not to transform.
    NoMask,
    Masked(MaskResult),
    Undecidable {
        why: Vec<UndecidableCause>,
    },
}

/// What a row-filter decision produced: `RangerAccessResult.filterExpr`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowFilterResult {
    pub policy_id: Option<i64>,
    pub policy_name: String,
    /// `setFilterExpr(rowFilterExpr)` — the policy's `filterExpr`, verbatim
    /// (this crate refuses rather than reproducing the double-bracket
    /// rewrite; see [`UndecidableCause::RowFilterDoubleBrackets`]).
    pub filter_expr: String,
}

/// The three-valued row-filter result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowFilterDecision {
    NoFilter,
    Filtered(RowFilterResult),
    Undecidable { why: Vec<UndecidableCause> },
}

/// Which of Ranger's three policy worlds a decision is being taken in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecisionKind {
    Access,
    DataMask,
    RowFilter,
}

impl DecisionKind {
    /// Ranger's raw `policyType` code. `RangerPolicyRepository.init` files
    /// a policy by this RAW value, so it is the value the candidate filter
    /// tests.
    fn policy_type_code(self) -> i32 {
        match self {
            DecisionKind::Access => 0,
            DecisionKind::DataMask => 1,
            DecisionKind::RowFilter => 2,
        }
    }

    /// The single item bucket `getMatchingPolicyItem` reads for this kind.
    /// `None` for access, which reads four.
    fn sub_bucket(self) -> Option<ItemBucket> {
        match self {
            DecisionKind::Access => None,
            DecisionKind::DataMask => Some(ItemBucket::DataMaskPolicyItems),
            DecisionKind::RowFilter => Some(ItemBucket::RowFilterPolicyItems),
        }
    }
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

    if request_preamble(policies, request, service_def, engine, &mut why) == SuperUser::Yes {
        return Decision::Allowed {
            policy_id: Some(SUPERUSER_POLICY_ID),
            policy_name: "superuser".to_string(),
        };
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
        .filter(|p| p.policy_type == DecisionKind::Access.policy_type_code())
        .map(|policy| PolicyView::build(policy, request, service_def))
        .collect();

    for view in &candidates {
        view.collect_causes(
            DecisionKind::Access,
            request,
            &implied,
            &all_access_types,
            &mut why,
        );
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
    sort_eval_order(&mut candidates);

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

/// Whether the request's principal short-circuits every policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SuperUser {
    Yes,
    No,
}

/// Everything the engine checks before it reaches the first policy, and
/// everything about the download and the service-def that is true whatever
/// the policy type. Shared by [`evaluate`], [`evaluate_data_mask`] and
/// [`evaluate_row_filter`], because
/// `evaluatePoliciesForOneAccessTypeNoAudit` runs all of it before the
/// `policyType` is branched on (the superuser block at offsets 114-165 and
/// `evaluateTagPolicies` at 168 both precede it).
fn request_preamble(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    service_def: &ServiceDef,
    engine: &EngineConfig,
    why: &mut Vec<UndecidableCause>,
) -> SuperUser {
    // ── the engine/service-def cross-check ─────────────────────────────
    //
    // Not Ranger's — Ranger cannot be handed the wrong def, its plugin
    // downloads its own. It is egret's, and it exists because the failure
    // it catches is silent: `schema` vs `database` means no hierarchy
    // contains the policy's keys and every match comes back NONE, which is
    // indistinguishable from "no policy applies".
    if let Some(declared) = engine.engine {
        if service_def.name() != declared.service_def_name() {
            why.push(UndecidableCause::EngineMismatch {
                declared,
                service_def_name: service_def.name().to_string(),
            });
        }
        if let Some(version) = engine.plugin_version {
            if version != declared.plugin_version() {
                why.push(UndecidableCause::PluginVersionMismatch {
                    engine: declared,
                    declared: version,
                    measured: declared.plugin_version(),
                });
            }
        }
    }

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
    // priority check then determines the result before the first
    // evaluator's items run — so a superuser allow is final and is reported
    // without weighing anything else, unmodelled constructs included.
    //
    // For a MASK or ROW-FILTER request the same block runs and the same
    // MAX_VALUE wins, but nothing ever sets `maskType` or `filterExpr`:
    // the loop resets only `isAccessDetermined` (offset 391), the `>=`
    // arbitration for policy types 1 and 2 re-determines it on the first
    // evaluator, and `evaluate`'s `if (!isAccessDetermined)` guard then
    // skips `evaluatePolicyItems` entirely. A superuser's columns come back
    // UNMASKED and unfiltered — which is why the callers below map
    // `SuperUser::Yes` onto `NoMask` / `NoFilter`, not onto a mask.
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
        return SuperUser::Yes;
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
    //
    // `evaluateTagPolicies` runs for policy types 1 and 2 as well as 0, so
    // `tagPolicies` can decide a mask; `policyDeltas` makes `policies`
    // partial whatever is being asked; `securityZones` and `serviceConfig`
    // likewise.
    for construct in &policies.unmodelled {
        why.push(UndecidableCause::EnvelopeConstruct(construct.clone()));
    }
    for issue in service_def.issues() {
        if issue_affects_access(issue) {
            why.push(UndecidableCause::ServiceDef(issue.clone()));
        }
    }

    SuperUser::No
}

/// `Collections.sort(evaluators, EVAL_ORDER_COMPARATOR)`, which
/// `RangerPolicyRepository.init` applies to all three evaluator lists
/// (three `Collections.sort` calls, one per list, same comparator).
///
/// `PolicyEvalOrderComparator`:
///
/// ```java
/// Integer.compare(other.getPolicyPriority(), me.getPolicyPriority())   // DESC
/// then: hasDeny() first
/// then: Integer.compare(me.getEvalOrder(), other.getEvalOrder())
/// then: compareStrings(policy name)
/// ```
///
/// `getEvalOrder()` is `RangerOptimizedPolicyEvaluator.computeEvalOrder()`,
/// a resource-specificity score, and is NOT reproduced. A stable
/// name-then-id tie-break stands in for it; what that costs differs by
/// policy type and is spelled out at each caller.
fn sort_eval_order(candidates: &mut [PolicyView<'_>]) {
    candidates.sort_by(|a, b| {
        b.policy
            .policy_priority
            .cmp(&a.policy.policy_priority)
            .then_with(|| b.has_deny.cmp(&a.has_deny))
            .then_with(|| a.policy.name.cmp(&b.policy.name))
            .then_with(|| a.policy.id.cmp(&b.policy.id))
    });
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
        kind: DecisionKind,
        request: &AccessRequest,
        implied: &BTreeMap<&str, BTreeSet<&str>>,
        all_access_types: &BTreeSet<&str>,
        why: &mut Vec<UndecidableCause>,
    ) {
        {
            let mut push = |construct: &Unmodelled| {
                why.push(UndecidableCause::PolicyConstruct {
                    policy_id: self.policy.id,
                    policy_name: self.policy.name.clone(),
                    construct: construct.clone(),
                });
            };

            for construct in &self.policy.unmodelled {
                // A construct that decides whether the policy MATCHES has
                // to be reported whether or not `matched` came out true —
                // `matched` is exactly what it makes unreliable.
                let always = matches!(
                    construct,
                    Unmodelled::ResourceValueExpressionMarker { .. } | Unmodelled::ZoneName(_)
                );
                if always || (self.matched && construct_affects(kind, construct)) {
                    push(construct);
                }
            }
        }

        if !self.matched {
            return;
        }

        // The mask / row-filter expressions, tested with Ranger's own
        // `hasExpressions` rather than with the reader's broader
        // `Unmodelled::{Mask,RowFilter}ExpressionMarker` — see
        // [`crate::model::has_request_expression`].
        if let Some(bucket) = kind.sub_bucket() {
            self.collect_sub_expression_causes(kind, bucket, why);
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
        let owner_unknown = decided_buckets(kind, self.policy).any(|items| {
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

    /// A `${{ … }}` expression (or a double-bracket row filter) in the one
    /// bucket this decision reads.
    ///
    /// Reported for any policy whose RESOURCES matched, whether or not its
    /// items match this principal — the same rule the `Unmodelled` sweep
    /// above follows, and the same reason: a construct this crate cannot
    /// evaluate is exactly what makes "it would not have been chosen"
    /// unreliable.
    fn collect_sub_expression_causes(
        &self,
        kind: DecisionKind,
        bucket: ItemBucket,
        why: &mut Vec<UndecidableCause>,
    ) {
        for (item_index, item) in sub_items(bucket, self.policy).iter().enumerate() {
            match kind {
                DecisionKind::Access => {}
                DecisionKind::DataMask => {
                    let Some(info) = &item.data_mask_info else {
                        continue;
                    };
                    for expr in [&info.value_expr, &info.condition_expr]
                        .into_iter()
                        .flatten()
                    {
                        if has_request_expression(expr) {
                            why.push(UndecidableCause::MaskExpression {
                                policy_id: self.policy.id,
                                policy_name: self.policy.name.clone(),
                                item_index,
                                expr: expr.clone(),
                            });
                        }
                    }
                }
                DecisionKind::RowFilter => {
                    let Some(expr) = item
                        .row_filter_info
                        .as_ref()
                        .and_then(|i| i.filter_expr.as_ref())
                    else {
                        continue;
                    };
                    // ORDER MATTERS, and it is Ranger's: the constructor
                    // rewrites double brackets FIRST and only then asks
                    // `hasExpressions` of the REWRITTEN string. So a
                    // double-bracket expression is undecidable regardless
                    // of what the original contained.
                    if has_double_brackets(expr) {
                        why.push(UndecidableCause::RowFilterDoubleBrackets {
                            policy_id: self.policy.id,
                            policy_name: self.policy.name.clone(),
                            item_index,
                            expr: expr.clone(),
                        });
                    } else if has_request_expression(expr) {
                        why.push(UndecidableCause::RowFilterExpression {
                            policy_id: self.policy.id,
                            policy_name: self.policy.name.clone(),
                            item_index,
                            expr: expr.clone(),
                        });
                    }
                }
            }
        }
    }

    /// `getMatchingPolicyItem(request, dataMaskEvaluators)` /
    /// `(request, rowFilterEvaluators)` — the SINGLE-list overload, which
    /// passes `null` as the exception list, followed by the matched item's
    /// own `updateAccessResult`.
    ///
    /// `None` means the policy determines nothing, which is NOT the same as
    /// "no item matched": a matched item whose `dataMaskInfo` is null
    /// returns from `updateAccessResult` at its first `ifnull` without
    /// touching the result, and `getMatchingPolicyItem` has already
    /// committed to that item — later items in the same policy are never
    /// tried. The engine loop then moves on to the next policy.
    fn decide_sub(
        &self,
        kind: DecisionKind,
        request: &AccessRequest,
        implied: &BTreeMap<&str, BTreeSet<&str>>,
    ) -> Option<SubResult> {
        if !self.matched {
            return None;
        }
        let bucket = kind.sub_bucket()?;
        let item = sub_items(bucket, self.policy)
            .iter()
            .find(|i| item_is_match(i, request, implied) == ItemMatch::Yes)?;

        match kind {
            DecisionKind::Access => None,
            DecisionKind::DataMask => item.data_mask_info.as_ref().map(|info| {
                SubResult::Mask(MaskResult {
                    policy_id: self.policy.id,
                    policy_name: self.policy.name.clone(),
                    mask_type: info.mask_type.clone(),
                    masked_value: info.value_expr.clone(),
                    mask_condition: info.condition_expr.clone(),
                })
            }),
            DecisionKind::RowFilter => item
                .row_filter_info
                .as_ref()
                .and_then(|i| i.filter_expr.clone())
                .map(|filter_expr| {
                    SubResult::RowFilter(RowFilterResult {
                        policy_id: self.policy.id,
                        policy_name: self.policy.name.clone(),
                        filter_expr,
                    })
                }),
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

/// One policy's mask or row-filter contribution.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SubResult {
    Mask(MaskResult),
    RowFilter(RowFilterResult),
}

/// Evaluate one data-mask request against one policy download.
///
/// Pure, like [`evaluate`], and the same three-valued contract: a mask this
/// evaluator cannot pin down names itself as an [`UndecidableCause`]
/// instead of being guessed at.
///
/// **A mask is data.** `RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult`
/// writes `maskType`, `maskedValue` and `maskCondition` and then reports
/// allow unconditionally, so [`MaskDecision::Masked`] carries those three
/// fields and there is no allow/deny to return. Whether the column may be
/// read at all is a separate question, answered by [`evaluate`] — Ranger
/// asks it separately too, with a separate `evaluatePolicies` call.
pub fn evaluate_data_mask(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    service_def: &ServiceDef,
    engine: &EngineConfig,
) -> MaskDecision {
    match evaluate_sub(
        DecisionKind::DataMask,
        policies,
        request,
        service_def,
        engine,
    ) {
        Err(why) => MaskDecision::Undecidable { why },
        Ok(None) => MaskDecision::NoMask,
        Ok(Some(SubResult::Mask(m))) => MaskDecision::Masked(m),
        Ok(Some(SubResult::RowFilter(_))) => {
            unreachable!("DataMask kind yields only SubResult::Mask")
        }
    }
}

/// Evaluate one row-filter request against one policy download. See
/// [`evaluate_data_mask`]; the shape is the same and the source is
/// `RangerDefaultRowFilterPolicyItemEvaluator`.
pub fn evaluate_row_filter(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    service_def: &ServiceDef,
    engine: &EngineConfig,
) -> RowFilterDecision {
    match evaluate_sub(
        DecisionKind::RowFilter,
        policies,
        request,
        service_def,
        engine,
    ) {
        Err(why) => RowFilterDecision::Undecidable { why },
        Ok(None) => RowFilterDecision::NoFilter,
        Ok(Some(SubResult::RowFilter(f))) => RowFilterDecision::Filtered(f),
        Ok(Some(SubResult::Mask(_))) => {
            unreachable!("RowFilter kind yields only SubResult::RowFilter")
        }
    }
}

/// The body shared by [`evaluate_data_mask`] and [`evaluate_row_filter`].
///
/// It differs from [`evaluate`] in exactly the four ways the jars say it
/// should, and in no others:
///
/// 1. the candidate filter is `policyType == 1` / `== 2`, because
///    `RangerPolicyRepository.init` files by the RAW code into three
///    disjoint lists and `getLikelyMatchPolicyEvaluators` returns only the
///    one the request asks for;
/// 2. `getMatchingPolicyItem` takes the SINGLE-list overload, so there is
///    no deny bucket, no exception veto, and `isDenyAllElse` is inert (its
///    fallback in `evaluatePolicyItems` is guarded by
///    `policyType == null || policyType == 0`);
/// 3. a matched item reports DATA, and a matched item with no
///    `dataMaskInfo` / `filterExpr` reports nothing at all and lets the
///    loop continue;
/// 4. the engine's priority arbitration is `>=`, not `>`, so the first
///    matching evaluator in eval order is final — which makes the
///    unreproduced `getEvalOrder()` load-bearing and a genuine tie
///    [`UndecidableCause::EvalOrderAmbiguous`].
fn evaluate_sub(
    kind: DecisionKind,
    policies: &ServicePolicySet,
    request: &AccessRequest,
    service_def: &ServiceDef,
    engine: &EngineConfig,
) -> Result<Option<SubResult>, Vec<UndecidableCause>> {
    let mut why: Vec<UndecidableCause> = Vec::new();

    if request_preamble(policies, request, service_def, engine, &mut why) == SuperUser::Yes {
        return Ok(None);
    }

    let implied = service_def.implied_access_grants();
    let all_access_types: BTreeSet<&str> = service_def
        .access_types()
        .iter()
        .map(|a| a.name.as_str())
        .collect();

    let mut candidates: Vec<PolicyView<'_>> = policies
        .policies
        .iter()
        .filter(|p| p.is_enabled)
        .filter(|p| p.policy_type == kind.policy_type_code())
        .map(|policy| PolicyView::build(policy, request, service_def))
        .collect();

    for view in &candidates {
        view.collect_causes(kind, request, &implied, &all_access_types, &mut why);
    }
    if !why.is_empty() {
        return Err(why);
    }

    sort_eval_order(&mut candidates);

    // The deciding band. Evaluators run in `EVAL_ORDER_COMPARATOR` order;
    // the first one that writes a mask wins, because on the NEXT iteration
    // `result.getIsAllowed()` is true and
    // `result.getPolicyPriority() >= evaluator.getPolicyPriority()` holds
    // for every later (lower- or equal-priority) evaluator, which sets
    // `isAccessDetermined` and makes `evaluate`'s
    // `if (!isAccessDetermined)` guard skip its items. (And when the
    // winner has no deny items of its own, offset 553 has already set the
    // flag before the loop even advances.)
    //
    // Which evaluator is "first" is decided within a `(priority, hasDeny)`
    // band by `getEvalOrder()`, which this crate does not compute. So the
    // band decides, not the order inside it: if the band holds one yield
    // the answer is that yield, and if it holds several DIFFERENT ones
    // there is no answer to give.
    let mut deciding: Vec<(&PolicyView<'_>, SubResult)> = Vec::new();
    let mut band: Option<(i32, bool)> = None;
    for view in &candidates {
        let this_band = (view.policy.policy_priority, view.has_deny);
        if band.is_some_and(|b| b != this_band) {
            break;
        }
        if let Some(found) = view.decide_sub(kind, request, &implied) {
            band = Some(this_band);
            deciding.push((view, found));
        }
    }

    match deciding.len() {
        0 => Ok(None),
        1 => Ok(Some(deciding.pop().expect("length checked").1)),
        _ => {
            // A tie that all agree on is not a tie the caller can act
            // wrongly on — but the reported policy id still differs, and
            // this crate does not report a policy it cannot name.
            Err(vec![UndecidableCause::EvalOrderAmbiguous {
                policy_ids: deciding.iter().map(|(v, _)| v.policy.id).collect(),
                policy_names: deciding
                    .iter()
                    .map(|(v, _)| v.policy.name.clone())
                    .collect(),
            }])
        }
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

/// The one bucket `getMatchingPolicyItem` reads for policy type 1 or 2.
fn sub_items(bucket: ItemBucket, policy: &RangerPolicy) -> &[PolicyItem] {
    match bucket {
        ItemBucket::DataMaskPolicyItems => policy.data_mask_policy_items.as_slice(),
        ItemBucket::RowFilterPolicyItems => policy.row_filter_policy_items.as_slice(),
        _ => &[],
    }
}

/// The buckets whose items actually decide `kind`: four for access, one for
/// masking, one for row-filtering.
fn decided_buckets(
    kind: DecisionKind,
    policy: &RangerPolicy,
) -> Box<dyn Iterator<Item = &[PolicyItem]> + '_> {
    match kind.sub_bucket() {
        None => Box::new(access_buckets(policy)),
        Some(bucket) => Box::new(std::iter::once(sub_items(bucket, policy))),
    }
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
fn construct_affects(kind: DecisionKind, construct: &Unmodelled) -> bool {
    match construct {
        Unmodelled::ValiditySchedules
        | Unmodelled::PolicyConditions
        | Unmodelled::ZoneName(_)
        | Unmodelled::ResourceValueExpressionMarker { .. } => true,
        // Item conditions are evaluated by `isMatch` through
        // `matchCustomConditions`, and `isMatch` is what BOTH the access
        // path and the single-list mask/row-filter path call. So a
        // condition decides exactly when its bucket is one this kind of
        // decision reads — for a mask policy that is
        // `dataMaskPolicyItems` alone, and a condition sitting in an
        // `allowExceptions` bucket of the same policy cannot reach the
        // decision at all.
        Unmodelled::ItemConditions { bucket, .. } => match kind.sub_bucket() {
            None => matches!(
                bucket,
                ItemBucket::PolicyItems
                    | ItemBucket::DenyPolicyItems
                    | ItemBucket::AllowExceptions
                    | ItemBucket::DenyExceptions
            ),
            Some(decided) => *bucket == decided,
        },
        // Envelope-scoped; reported from `policies.unmodelled`, never here.
        Unmodelled::SecurityZones
        | Unmodelled::TagPolicies
        | Unmodelled::PolicyDeltas
        | Unmodelled::ServiceConfig => true,
        // The reader's mask/row-filter expression markers fire on a bare
        // `{TOKEN}` as well as on `${{ … }}`, which is right for a resource
        // value (`StringTokenReplacer` does expand those) and WRONG for a
        // mask: no token replacer runs there. These are therefore not used
        // as a trigger at all; the mask and row-filter paths test
        // `has_request_expression` / `has_double_brackets` directly in
        // [`PolicyView::collect_sub_expression_causes`], which is both
        // narrower (no false `Undecidable` for every CUSTOM mask) and
        // wider (it catches `conditionExpr` expressions and double
        // brackets, which the markers do not distinguish).
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
