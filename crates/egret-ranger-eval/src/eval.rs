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
//! | `securityZones`, `policyDeltas`, `serviceConfig` | [`UndecidableCause::EnvelopeConstruct`] |
//! | a `tagPolicies` block that could decide | [`UndecidableCause::EnvelopeConstruct`] naming the count — see below |
//! | a non-default resource matcher, `quotedCaseSensitive`, chained `impliedGrants` | [`UndecidableCause::ServiceDef`] |
//! | a policy access type that matches the request's only case-insensitively | [`UndecidableCause::AccessTypeCaseMismatch`] |
//!
//! `tagPolicies` is the one entry in that table with a measured escape, and
//! it is deliberately the only one. A block is refused unless it can be shown
//! unable to decide, which happens in three ways: the block carries no
//! policies at all ([`crate::model::TagPolicyBlock`] — the tier every
//! auto-bound Trino profile lands in, since `bind_trino_profile` creates
//! `{name}_tag` with zero policies), the service-def declares no tag enricher
//! ([`ServiceDef::has_tag_enricher`]), or the request's resolved tag set is
//! explicitly empty ([`AccessRequest::resolved_tags`]). Anything else — a
//! non-empty block, an unreadable block, an unknown enricher, an unresolved
//! tag set — refuses.
//!
//! SP4d-2b Task 2 added the other half: [`evaluate_tag_policies`],
//! [`evaluate_tag_data_mask`] and [`evaluate_tag_row_filter`] reproduce
//! `RangerPolicyEngineImpl.evaluateTagPolicies` against a resolved tag set
//! ([`AccessRequest::resolved_tags`]) and the block's own service-def. They are
//! one PHASE, not an answer: composing their verdict with the resource
//! policies' is the cross-phase arbitration, and the refusal above still
//! stands in [`evaluate`] until that lands.
//!
//! No comparable escape exists for `serviceConfig`, and the resemblance is a
//! trap: an empty `tagPolicies` is a measured no-op, while an unmodelled
//! `serviceConfig` fails **OPEN** through a super-user allow at
//! `policyPriority = Integer.MAX_VALUE`. See the ⚠ block on
//! [`crate::model::Unmodelled::ServiceConfig`].
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
    ServicePolicySet, TagPolicyBlock, Unmodelled,
};
use crate::resource::{
    match_policy_resources, trie_reproduces_matcher, MatchType, RequestResource,
};
use crate::servicedef::{PolicyType, ServiceDef, ServiceDefIssue};

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
    /// The tags the caller resolved for [`AccessRequest::resource`] — the set
    /// `RangerTagEnricher.enrich` would have put in the request context, which
    /// `evaluateTagPolicies` reads at offset 104 via
    /// `RangerAccessRequestUtil.getRequestTagsFromContext`.
    ///
    /// `Some(empty)` is a licence to narrow: with no tags,
    /// `RangerPolicyRepository.getLikelyMatchPolicyEvaluators(request, tags,
    /// policyType, date)` returns `Collections.EMPTY_LIST` at its
    /// `isNotEmpty(tags)` branch (offsets 0-9, identical at 2.5.0 and 2.8.0),
    /// so no tag policy can touch the decision however many the envelope
    /// carries.
    ///
    /// # ⚠ `None` MUST NOT be read as an empty set — that is fail-open
    ///
    /// `None` means the caller did not resolve tags, the same discipline
    /// [`EngineConfig`]'s `Option` fields use, and it is kept distinct because
    /// **egret cannot currently establish the antecedent the narrowing needs.**
    /// The narrowing is sound only if the caller's tag resolution is at least
    /// as broad as the enricher's, and egret's is not:
    /// `tag_keys_for_resource_via` (`crates/egret-policy/src/store.rs`) matches
    /// `schema_name` / `table_name` / `column_name` by **exact string
    /// equality** with no hierarchy roll-up, so asked about `report.country` it
    /// does not return a tag attached to the table `report`. Ranger's
    /// `RangerTagEnricher` does the opposite: it walks the hierarchy and emits
    /// ancestor tags carrying `MatchType::ANCESTOR`, which Ranger **admits**
    /// for a tag request (unlike a resource request — see
    /// [`crate::resource::MatchType::Ancestor`]). So egret can report "no
    /// tags" for a column while the engine is enforcing a tag policy through
    /// a tag attached to its table.
    ///
    /// Every live tag assignment today is column-level, so the ancestor path
    /// is not exercised by current data — which is exactly how treating `None`
    /// as empty would ship green and wrong. This crate therefore only
    /// **offers** the narrowing; whether passing `Some` is sound is a property
    /// of the caller, decided in SP4d-2a Task 5, not of this type.
    ///
    /// # Why each entry carries a [`MatchType`] and why this is not a `Set`
    ///
    /// `RangerTagForEval` carries the match type the enricher stamped on the
    /// attachment, and [`crate::eval::evaluate_tag_policies`] reads it rather
    /// than matching the policy's own resource — see that function. Its
    /// `equals`/`hashCode` include `matchType`, so the same tag reached at two
    /// hierarchy levels is TWO members of Ranger's set, and collapsing them by
    /// name would silently reorder masks (the mask comparator is the match
    /// type). This crate takes a `Vec` because the resolver returns one and
    /// because **the caller's order is not authoritative** — the phase applies
    /// Ranger's three comparators itself — and de-duplicates on
    /// `(name, match_type)`, which is exactly `RangerTagForEval`'s identity.
    pub resolved_tags: Option<Vec<ResolvedTag>>,
}

/// One tag the caller resolved for [`AccessRequest::resource`]: the wire tag's
/// `type` and the `MatchType` the enricher stamped on it.
///
/// `name` is the identifier the ENGINE matches — `ServiceTags.tags[].type`,
/// which `ServiceTags::from_assignments` fills from `tags.name`. It is **not**
/// `tags.key`; live those differ for one of three tags, and reusing the key
/// here would be a silent fail-open (SP4d-2b design, D3).
///
/// `match_type` is a property of the ATTACHMENT, not of any policy:
/// `RangerTagEnricher.findMatchingTags` builds a real
/// `RangerDefaultPolicyResourceMatcher` per tag service resource and records
/// what it returned. The policy's own resource is a separate question, asked
/// separately — see [`evaluate_tag_policies`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTag {
    pub name: String,
    pub match_type: MatchType,
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

/// The PLUGIN-side half of the configuration the engine consults.
///
/// Every field is an `Option`: `None` means "the caller did not measure
/// this", which is not the same as "empty" and must not be treated as such.
/// `isSuperUser` is checked BEFORE any policy and wins with
/// `policyPriority = Integer.MAX_VALUE`, so an unknown superuser list makes
/// every decision unknowable. Both live Trino instances were measured to
/// set neither (`docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`) —
/// that measurement is what [`EngineConfig::without_super_users`] records.
///
/// # This struct is only half of the super-user question
///
/// **A policy download CAN also carry super-users**, and an earlier version
/// of this doc said it could not. `RangerPolicyEngineImpl.<init>` builds a
/// `RangerPolicyEngineImpl$ServiceConfig` from
/// `ServicePolicies.getServiceConfig()` (offsets 125-136, identical at 2.5.0
/// and 2.8.0), that class reads `ranger.plugin.super.users`,
/// `ranger.plugin.super.groups` and `ranger.plugin.service.admins`, and
/// `isSuperUser(user, groups)` ORs it against `RangerPluginConfig`. Either
/// side alone decides.
///
/// Filling this struct in therefore does NOT make a download's super-users
/// known. What keeps the crate safe today is a refusal one layer up:
/// [`crate::model::Unmodelled::ServiceConfig`] fires on any envelope with a
/// non-empty `serviceConfig`, and [`UndecidableCause::EnvelopeConstruct`]
/// turns it into `Undecidable` for every decision. That refusal, not this
/// struct, is the safety property — see the warning on
/// [`crate::model::Unmodelled::ServiceConfig`] before narrowing it.
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
    /// An envelope-level construct: `securityZones`, `policyDeltas`,
    /// `serviceConfig`, or a `tagPolicies` block that could decide. Any of
    /// them can change any decision in the download.
    ///
    /// For `tagPolicies` the payload names what the block was measured to
    /// carry ([`crate::model::TagPolicyBlock`]), and the cause is raised only
    /// after the two request/def escapes have been ruled out — see
    /// [`tag_policies_can_decide`].
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
    /// `(policyPriority, hasDeny)` and each yield a result, so which one
    /// Ranger reports is decided by
    /// `RangerOptimizedPolicyEvaluator.getEvalOrder()` — a
    /// resource-specificity score this crate does not reproduce.
    ///
    /// **The refusal fires on ANY such tie, not only on one where the tied
    /// policies yield different masks.** [`MaskResult`] and
    /// [`RowFilterResult`] both carry a `policy_id` and a `policy_name`, and
    /// a tie the crate cannot order is a tie whose winner it cannot name —
    /// so even when every tied policy yields the same mask there is no
    /// complete answer to return. Measured at the recorded differential seed
    /// (`tests/ranger/README.md`): 144 ties in 4000 cases, of which 111
    /// yield different results and 33 agree on everything but the policy id.
    /// Refusing the agreeing 33 costs 0.8% of the corpus and is the reason
    /// this variant's payload is a LIST of ids rather than one id.
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
    /// The envelope's `tagPolicies` block carries policies this decision's
    /// policy type would evaluate, and [`AccessRequest::resolved_tags`] is
    /// `None` — the caller did not resolve tags, which is NOT "there are
    /// none". See the ⚠ block on that field.
    TagsUnresolved,
    /// A tag policy's resource map is one whose CANDIDACY this crate cannot
    /// reproduce. See [`TagResourceProblem`] for the three shapes and the
    /// measurement behind each.
    ///
    /// # Why candidacy is a separate question on the tag path
    ///
    /// For a resource request the trie is a pre-filter and
    /// `RangerDefaultPolicyEvaluator.evaluate` re-asks the matcher, so an
    /// over-returning trie changes nothing. For a `RangerTagAccessRequest`
    /// `evaluate` reads the TAG's match type and never touches the policy's
    /// matcher, so the trie alone decides which tag policies run — and where
    /// the trie is wider than the matcher, this crate would skip a policy
    /// Ranger evaluates. For a DENY tag policy that is fail-open.
    TagResourceNotReproducible {
        policy_id: Option<i64>,
        policy_name: String,
        problem: TagResourceProblem,
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

/// Why a tag policy's resource map cannot be filtered the way Ranger filters
/// it. Each arm was measured against a real `RangerResourceTrie` built from
/// `ranger-plugins-common` 2.5.0 over the `tag` service-def — see
/// [`crate::resource::trie_reproduces_matcher`] for the harness and the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagResourceProblem {
    /// The map is not exactly `{"tag": …}`.
    ///
    /// `RangerResourceEvaluatorsRetriever.getEvaluators` intersects only the
    /// tries for resource keys the REQUEST carries, and a tag request carries
    /// exactly one (`RangerTagResource`'s `KEY_TAG = "tag"`). So a policy
    /// naming an extra level is still returned by the tag trie, while this
    /// crate's `init` finds no hierarchy containing all the policy's keys and
    /// answers `NONE`.
    NotOnlyTheTagResource,
    /// `isExcludes` is set.
    ///
    /// MEASURED: the trie parks an excludes resource in
    /// `getInheritedEvaluators()`, which the retriever unions into EVERY
    /// lookup — a policy excluding `pii` came back as a candidate for the tag
    /// `pii` itself, where the matcher answers `NONE`. (The tag service-def
    /// declares `excludesSupported: false`, so nothing egret writes can reach
    /// this; a hand-written payload can.)
    Excludes,
    /// A value whose compiled matcher is `EndsWith` or a general wildcard, or
    /// an empty value.
    ///
    /// MEASURED: the trie keys a value by the literal prefix BEFORE its first
    /// wildcard character, so `*ii` sits at the root and `p*x` at `p` — both
    /// are returned for tags the matcher rejects.
    ValueNotTriePrefixExact(String),
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
    /// `setMaskType(dataMaskInfo.getDataMaskType())`, passed straight
    /// through with no null check. Never resolved.
    ///
    /// `None` is Ranger reporting a null `maskType` — which a matched item
    /// carrying no `dataMaskInfo` at all produces, since the model class
    /// substitutes an empty one (finding F2). It is NOT the same as
    /// [`MaskDecision::NoMask`]: the host applies no transform either way,
    /// but this one DETERMINED the result, so no lower-priority policy is
    /// ever consulted.
    pub mask_type: Option<String>,
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
    // rest cannot deny. What CAN differ is which policy gets reported, so a
    // stable name-then-id tie-break stands in for the score.
    //
    // That reported-id difference is REAL and is measured, not waved away:
    // at the recorded corpus seed, 39 of 1972 compared ACCESS cases agree on
    // allow/deny while naming a different policy — and **31 of those are
    // DENIES**, not allows. (An earlier version of this comment said only an
    // allow's id could differ. It was wrong, and wrong in the direction that
    // makes the gap sound smaller than it is.) `tests/ranger/README.md`
    // carries it as a declared divergence; nothing in this crate asserts a
    // reported policy id for an ACCESS decision, which is why the gate is
    // green in spite of it.
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

/// Whether an envelope's `tagPolicies` block can reach THIS decision, given
/// this request and the TWO service-defs involved.
///
/// `false` is a licence to skip the refusal, so every arm here must be a
/// measurement rather than a plausibility argument. Two escapes exist, and
/// each is independently sufficient; the measurements are recorded on
/// [`crate::model::TagPolicyBlock`], [`ServiceDef::has_tag_enricher`] and
/// [`AccessRequest::resolved_tags`].
///
/// 1. **BOTH defs declare no tag enricher.** Nothing then writes the request's
///    tag context, so `getLikelyMatchPolicyEvaluators` returns
///    `Collections.EMPTY_LIST` at `isNotEmpty(tags)` and
///    `evaluateTagPolicies` exits at `isNotEmpty(evaluators)`. Only an
///    explicit `Some(false)` counts on each: `None` means the def handed to
///    this crate did not say, which is not the same as saying no.
/// 2. **The request's resolved tag set is empty.** Same branch, reached from
///    the request side instead, and independent of both defs — no tags means
///    no evaluators however many enrichers ran. Only an explicit
///    `Some(empty)` counts, and ⚠ **whether passing `Some` is sound is a
///    property of the CALLER** — egret's own tag resolution is narrower than
///    the enricher's, so a `Some` supplied from `tag_keys_for_resource_via` as
///    it stands today would fail open. See the ⚠ block on
///    [`AccessRequest::resolved_tags`].
///
/// # ⚠ Why BOTH defs, and not just the resource def
///
/// Ranger builds context enrichers from the resource def **and** from the
/// `tagPolicies` block's own `serviceDef`, then UNIONS the two lists. Checking
/// only the resource def is a fail-open. Measured in `ranger-plugins-common`
/// 2.5.0 and 2.8.0 — offsets are 2.5.0's, 2.8.0 has the same shape a few bytes
/// earlier:
///
/// * `PolicyEngine.<init>` builds the tag repository when
///   `!options.disableTagPolicyEvaluation && tagPolicies != null &&
///   !StringUtils.isEmpty(tagPolicies.getServiceName()) &&
///   tagPolicies.getServiceDef() != null` — offsets 471-500. **There is no
///   `getPolicies()` call anywhere in that gate**, so the repository exists
///   even for a block with no policies.
/// * `RangerPolicyRepository(TagPolicies, ctx, componentDef, componentName)`
///   sets `this.serviceDef` from `tagPolicies.getServiceDef()` (offsets 33-48)
///   and `this.zoneName = null` (offset 30).
/// * `buildContextEnrichers` reads `this.serviceDef.getContextEnrichers()`
///   under an `isEmpty(zoneName)` guard — offsets 12-38 — which the tag
///   repository satisfies by construction.
/// * `PolicyEngine.<init>` then sets `allContextEnrichers =
///   tagPolicyRepository.getContextEnrichers() ++
///   policyRepository.getContextEnrichers()` at offsets 619-702: an
///   `ArrayList.addAll` union, not a choice between the two.
/// * `RangerDefaultRequestProcessor.enrich` iterates
///   `PolicyEngine.getAllContextEnrichers()` (offset 4) and calls
///   `RangerContextEnricher.enrich` on each (offset 110); `preProcess` calls
///   `enrich` at offset 363.
///
/// And the reachable payload: **Apache Ranger's own stock
/// `service-defs/ranger-servicedef-tag.json`, bundled inside
/// `ranger-plugins-common` 2.5.0 AND 2.8.0, declares `RangerTagEnricher` in
/// `contextEnrichers`.** So a non-empty block whose `serviceDef` is the stock
/// tag def enriches and evaluates tag policies even when the resource def
/// declares nothing — see
/// `tests/tag_tiers.rs::the_block_s_own_service_def_can_supply_the_enricher`,
/// whose fixture is that file verbatim from the jar.
///
/// What keeps this from biting egret today is DATA, not this measurement:
/// `crates/egret-policy/src/service-defs/tag.json` happens to omit
/// `contextEnrichers`. `ranger/api/download.rs` embeds that def for **any**
/// engine type with a `tagService` in its config, and the differential corpus
/// and `examples/live_gate.rs` consume downloads captured elsewhere — so a
/// resource-def-only check would be closed by one unset field and nothing
/// would announce the day it is set.
fn tag_policies_can_decide(
    block: &TagPolicyBlock,
    request: &AccessRequest,
    service_def: &ServiceDef,
    tag_service_def: &ServiceDef,
) -> bool {
    match block {
        // Not shown inert and not countable, so no narrowing applies: a shape
        // this reader does not understand gets the full refusal.
        TagPolicyBlock::Unreadable => true,
        TagPolicyBlock::Policies(_) => {
            // Both, because Ranger unions the two enricher lists. `None` on
            // either side is unknown and refuses, exactly as the single-def
            // case did.
            if service_def.has_tag_enricher() == Some(false)
                && tag_service_def.has_tag_enricher() == Some(false)
            {
                return false;
            }
            if request
                .resolved_tags
                .as_ref()
                .is_some_and(|tags| tags.is_empty())
            {
                return false;
            }
            true
        }
    }
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
    //
    // `tagPolicies` is the one envelope construct with a request- and
    // def-dependent escape: `Unmodelled::TagPolicies` is only raised for a
    // block the reader could not show inert on the envelope alone, and two
    // further conditions can still make it unable to decide. Nothing here
    // narrows `ServiceConfig` — see the ⚠ block on
    // `crate::model::Unmodelled::ServiceConfig` for why the two refusals look
    // alike and carry opposite risk.
    for construct in &policies.unmodelled {
        if let Unmodelled::TagPolicies(block) = construct {
            // The block's own def is a second source of context enrichers, so
            // the gate needs it too — see `tag_policies_can_decide`. Parsed
            // through the same `ServiceDef::parse` as the resource def so
            // there is one enricher predicate, not two.
            let tag_service_def = ServiceDef::parse(&policies.tag_service_def);
            if !tag_policies_can_decide(block, request, service_def, &tag_service_def) {
                continue;
            }
        }
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

    /// The same view for a TAG evaluator, where `matched` is not this
    /// policy's to decide.
    ///
    /// `RangerDefaultPolicyEvaluator.evaluate` takes the match type from the
    /// `RangerTagAccessRequest` — the enricher's verdict on the tag ATTACHMENT
    /// — and never calls this policy's matcher. Candidacy (does this policy's
    /// `resources.tag` cover this tag name?) is the trie's separate question,
    /// answered in [`tag_candidates`] before this is built. Computing
    /// `matched` here as well would be a second, different rule for the same
    /// word.
    fn for_tag(policy: &'a RangerPolicy) -> PolicyView<'a> {
        PolicyView {
            policy,
            has_deny: policy.is_deny_all_else || !policy.deny_policy_items.is_empty(),
            has_allow: !policy.policy_items.is_empty(),
            matched: true,
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
    /// "no item matched": `getMatchingPolicyItem` has already committed to
    /// the first matching item, so later items in the same policy are never
    /// tried, and the engine loop moves on to the next policy.
    ///
    /// The two sub-kinds reach that state by DIFFERENT routes, and assuming
    /// they were the same is finding F2 (SP4d-1 Task 7):
    ///
    /// * **Mask.** `RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult`
    ///   opens with `if (getDataMaskInfo() == null) return;` — but
    ///   [`crate::model::PolicyItem::data_mask_info`] is always `Some` on this
    ///   bucket, because Ranger's model class cannot hold a null there. So
    ///   the guard never fires, a matched mask item ALWAYS determines, and
    ///   the `mask_type` it reports may itself be `None`.
    /// * **Row filter.** `RangerDefaultRowFilterPolicyItemEvaluator.updateAccessResult`
    ///   has no such guard at the top, but wraps its
    ///   `policyEvaluator.updateAccessResult(...)` call in
    ///   `if (result.getFilterExpr() != null)`. A null `filterExpr` therefore
    ///   determines NOTHING and the search continues. An empty string is not
    ///   null and does determine.
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
            // Always `Some` for a matched mask item — see the doc above.
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
    // the answer is that yield, and if it holds SEVERAL — agreeing or not —
    // there is no complete answer to give, because a result names the
    // policy that produced it and this crate cannot say which that was.
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

// ── the tag phase ────────────────────────────────────────────────────────
//
// `RangerPolicyEngineImpl.evaluateTagPolicies(request, policyType, zoneName,
// tagPolicyRepository, result)` — the phase that runs BEFORE the resource
// policies (offset 176 of `evaluatePoliciesForOneAccessTypeNoAudit`), against
// the `tagPolicies` block's own policies and its own service-def.
//
// Composing this phase's verdict with the resource phase's is SP4d-2b Task 3
// and is deliberately NOT done here: the functions below answer only "what did
// the tag policies determine", and the envelope-level refusals
// (`securityZones`, `policyDeltas`, `serviceConfig`, the superuser
// short-circuit) stay with [`evaluate`], which is where Ranger runs them too.

/// `RangerTagResource.KEY_TAG`, verified with `javap -constants` on both jars:
/// the resource key a tag request carries is the literal `"tag"`, hard-coded
/// in Ranger and not read from the service-def.
pub const TAG_RESOURCE_KEY: &str = "tag";

/// What the tag phase left on the shared `RangerAccessResult`.
///
/// The three non-refusal arms are the only states the phase can exit in.
/// `setIsAllowed(false)` calls `setIsAccessDetermined(true)` itself, and the
/// tail of `evaluateTagPolicies` (offsets 582-593) sets it for an allow that no
/// evaluator had determined — so at exit `isAccessDetermined` holds exactly
/// when some policy wrote a verdict.
///
/// `priority` is `RangerAccessResult.getPolicyPriority()`, which Task 3's
/// cross-phase arbitration compares against each resource evaluator's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagDecision {
    /// No tag policy determined anything: `isAllowed` false,
    /// `isAccessDetermined` false. The resource phase decides alone.
    NotDetermined,
    Allowed {
        policy_id: Option<i64>,
        policy_name: String,
        priority: i32,
    },
    Denied {
        policy_id: Option<i64>,
        policy_name: String,
        priority: i32,
        reason: DenyReason,
    },
    Undecidable {
        why: Vec<UndecidableCause>,
    },
}

/// One `(evaluator, tag)` pair — Ranger's `PolicyEvaluatorForTag`, which is
/// what the three comparators actually sort.
struct TagCandidate<'a> {
    view: PolicyView<'a>,
    /// `PolicyEvaluatorForTag.getTag().getMatchType()`, i.e. the ATTACHMENT's
    /// match type. `MATCH_TYPE_COMPARATOR` is the mask ordering's first key.
    match_type: MatchType,
}

/// Evaluate the `tagPolicies` block's ACCESS policies against the request's
/// resolved tag set.
///
/// Pure, like [`evaluate`]. `tag_service_def` is the BLOCK's own def
/// ([`ServiceDef::parse`] of [`ServicePolicySet::tag_service_def`]), because
/// that is the def `RangerPolicyRepository`'s tag constructor stores and the
/// one `RangerTagAccessRequest` is built with — the resource def decides
/// nothing here.
///
/// # ⚠ This is ONE PHASE, not an answer
///
/// It does not run [`request_preamble`], so it does not see the superuser
/// short-circuit or the `securityZones` / `policyDeltas` / `serviceConfig`
/// refusals, and it does not arbitrate with the resource policies. A caller
/// wanting a decision calls [`evaluate`]; this exists so Task 3 can compose the
/// two phases the way `evaluatePoliciesForOneAccessTypeNoAudit` does.
pub fn evaluate_tag_policies(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tag_service_def: &ServiceDef,
) -> TagDecision {
    let candidates = match tag_candidates(DecisionKind::Access, policies, request, tag_service_def)
    {
        Err(why) => return TagDecision::Undecidable { why },
        Ok(c) => c,
    };
    let implied = tag_service_def.implied_access_grants();

    // The loop at offsets 157-579, reduced. Three things make the reduction
    // exact, and each is a measured branch rather than a simplification:
    //
    // * offset 442 `tagEvalResult.setAccessResultFrom(result)` — each
    //   evaluator starts from the SHARED result, which is why `t_allowed`
    //   below is seeded from `allowed` rather than from `false`;
    // * offsets 472-493 `if (tagEvalResult.getIsAllowed() && !hasDeny())
    //   setIsAccessDetermined(true)` — an allow from a policy that HAS deny
    //   items is recorded but does not determine, so a later deny still wins;
    // * offsets 511-557 — the copy back is gated on the shared result not
    //   being determined, and once it is, no later evaluator can change the
    //   access outcome. (The loop itself only breaks when auditing is ALSO
    //   determined, at 560-576; the extra iterations are no-ops for access.)
    //
    // Unlike the resource loop this one compares no priorities: the first
    // evaluator to determine is final, and the order is the comparator's.
    let mut allowed = false;
    let mut determined = false;
    let mut winner: Option<Outcome> = None;
    for candidate in &candidates {
        if determined {
            break;
        }
        let mut t_allowed = allowed;
        let mut t_winner = winner.clone();
        let mut t_determined = false;

        if let Some(found) = candidate.view.decide(request, &implied) {
            if found.allowed {
                // `updateAccessResult(result, matchType, true, reason)` writes
                // only `if (!result.getIsAllowed())`. In 2.5.0 it also guards
                // `matchType != ANCESTOR` — unreachable on this path, because
                // `evaluate` has already REWRITTEN an ANCESTOR tag match to
                // SELF (offsets 206-219). 2.8.0 dropped the guard instead.
                // Both versions therefore record an ANCESTOR tag allow.
                if !t_allowed {
                    t_allowed = true;
                    t_winner = Some(found);
                }
            } else {
                // `setIsAllowed(false)` calls `setIsAccessDetermined(true)`.
                t_allowed = false;
                t_determined = true;
                t_winner = Some(found);
            }
        }

        if t_allowed && !candidate.view.has_deny {
            t_determined = true;
        }
        if t_determined {
            allowed = t_allowed;
            winner = t_winner;
            determined = true;
        } else if !allowed && t_allowed {
            allowed = t_allowed;
            winner = t_winner;
        }
    }
    // Offsets 582-593: an allow nobody determined is determined here.
    if allowed {
        determined = true;
    }

    match (determined, winner) {
        (false, _) => TagDecision::NotDetermined,
        (true, None) => TagDecision::NotDetermined,
        (true, Some(o)) if allowed => TagDecision::Allowed {
            policy_id: o.policy_id,
            policy_name: o.policy_name,
            priority: o.priority,
        },
        (true, Some(o)) => TagDecision::Denied {
            policy_id: o.policy_id,
            policy_name: o.policy_name,
            priority: o.priority,
            reason: o.reason,
        },
    }
}

/// [`evaluate_tag_policies`] for `policyType` 1. Same loop, same comparator
/// dispatch — the difference is that the deciding list is the block's MASK
/// policies and the ordering key is the tag's match type.
pub fn evaluate_tag_data_mask(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tag_service_def: &ServiceDef,
) -> MaskDecision {
    match evaluate_tag_sub(DecisionKind::DataMask, policies, request, tag_service_def) {
        Err(why) => MaskDecision::Undecidable { why },
        Ok(None) => MaskDecision::NoMask,
        Ok(Some(SubResult::Mask(m))) => MaskDecision::Masked(m),
        Ok(Some(SubResult::RowFilter(_))) => {
            unreachable!("DataMask kind yields only SubResult::Mask")
        }
    }
}

/// [`evaluate_tag_policies`] for `policyType` 2.
pub fn evaluate_tag_row_filter(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tag_service_def: &ServiceDef,
) -> RowFilterDecision {
    match evaluate_tag_sub(DecisionKind::RowFilter, policies, request, tag_service_def) {
        Err(why) => RowFilterDecision::Undecidable { why },
        Ok(None) => RowFilterDecision::NoFilter,
        Ok(Some(SubResult::RowFilter(f))) => RowFilterDecision::Filtered(f),
        Ok(Some(SubResult::Mask(_))) => {
            unreachable!("RowFilter kind yields only SubResult::RowFilter")
        }
    }
}

/// The body shared by [`evaluate_tag_data_mask`] and
/// [`evaluate_tag_row_filter`].
///
/// The state machine is [`evaluate_tag_policies`]'s, with `decide_sub` in place
/// of `decide`: `RangerDefault{DataMask,RowFilter}PolicyItemEvaluator.updateAccessResult`
/// writes its DATA and then calls `policyEvaluator.updateAccessResult(result,
/// matchType, true, reason)`, i.e. an allow, so the same "allow determines only
/// when the policy has no deny items" rule applies.
///
/// **There is no [`UndecidableCause::EvalOrderAmbiguous`] here, and its absence
/// is deliberate.** That refusal exists because the resource path's ordering
/// needs `RangerOptimizedPolicyEvaluator.getEvalOrder()`, a resource-specificity
/// score this crate does not compute. `MATCH_TYPE_COMPARATOR` and
/// `NAME_COMPARATOR` — the two the tag path uses for these policy types — call
/// no such thing: they are match type, then policy priority, then `hasDeny`,
/// then the policy name, all of which this crate has. The ordering is
/// reproduced, so there is nothing to refuse.
///
/// # One divergence, in a shape egret cannot emit
///
/// Ranger writes the mask FIELDS unconditionally and the policy IDENTITY only
/// `if (!result.getIsAllowed())`. The two can come apart only when a mask or
/// row-filter tag policy carries `denyPolicyItems` or `isDenyAllElse` (so
/// `hasDeny()` is true and its yield does not determine) and a LATER one also
/// yields: Ranger then reports the first policy's id with the second's mask.
/// [`SubResult`] carries the two together, so this crate reports the second's
/// id as well. Neither bucket is read for a mask or row-filter decision, and
/// egret's compiler emits neither.
fn evaluate_tag_sub(
    kind: DecisionKind,
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tag_service_def: &ServiceDef,
) -> Result<Option<SubResult>, Vec<UndecidableCause>> {
    let candidates = tag_candidates(kind, policies, request, tag_service_def)?;
    let implied = tag_service_def.implied_access_grants();

    let mut allowed = false;
    let mut determined = false;
    let mut found: Option<SubResult> = None;
    for candidate in &candidates {
        if determined {
            break;
        }
        let mut t_allowed = allowed;
        let mut t_found = found.clone();
        let mut t_determined = false;

        if let Some(result) = candidate.view.decide_sub(kind, request, &implied) {
            t_found = Some(result);
            t_allowed = true;
        }
        if t_allowed && !candidate.view.has_deny {
            t_determined = true;
        }
        if t_determined {
            allowed = t_allowed;
            found = t_found;
            determined = true;
        } else if !allowed && t_allowed {
            allowed = t_allowed;
            found = t_found;
        }
    }
    Ok(found)
}

/// The `PolicyEvaluatorForTag` list `evaluateTagPolicies` iterates:
/// `RangerPolicyRepository.getLikelyMatchPolicyEvaluators(request, tags,
/// policyType, accessTime)`, already sorted by this policy type's comparator.
///
/// Its four steps, in Ranger's order:
///
/// 1. the evaluators that exist at all — `skipBuildingPolicyEvaluator` drops a
///    disabled policy and `init` files the rest by RAW `policyType` into three
///    disjoint lists;
/// 2. one pass per TAG (offsets 28-226), skipping a tag `isApplicable` rejects;
/// 3. per tag, the trie lookup that decides which evaluators apply — reproduced
///    here with [`match_policy_resources`] against the TAG service-def, the tag
///    policy's `resources.tag` in the policy position and `{tag: <name>}` in
///    the request position. Where the trie and the matcher are measured to
///    disagree the policy is refused instead
///    ([`UndecidableCause::TagResourceNotReproducible`]);
/// 4. the sort (offsets 237-298).
///
/// # The admitted set, and why it is the TAG's match type
///
/// `RangerDefaultPolicyEvaluator.evaluate` does not ask the policy's matcher
/// for a `RangerTagAccessRequest`; it reads
/// `((RangerTagAccessRequest) request).getMatchType()`, which
/// `RangerTagAccessRequest`'s constructor took from `RangerTagForEval` — the
/// enricher's verdict on the ATTACHMENT. Step 3 above decides candidacy; this
/// decides admission, and they are different questions about different
/// resources.
///
/// At the default `SELF` scope with a concrete access type the two versions
/// admit exactly `SELF`, `SELF_AND_ALL_DESCENDANTS` and `ANCESTOR`, by
/// different routes that were both read:
///
/// * **2.5.0** rewrites `ANCESTOR` to `SELF` at offsets 206-219 and then tests
///   `matchType == SELF || matchType == SELF_AND_ALL_DESCENDANTS` (317-338).
///   The rewrite is why `updateAccessResult`'s own `matchType != ANCESTOR`
///   guard on the allow branch never fires for a tag request.
/// * **2.8.0** keeps the match type and adds the disjunct
///   `|| (matchType == ANCESTOR && request instanceof RangerTagAccessRequest)`
///   (offsets 334-370), having dropped the `updateAccessResult` guard.
///
/// `DESCENDANT` and `NONE` are in neither version's set at this scope —
/// `DESCENDANT` joins it only under `isAccessTypeAny()` or
/// `SELF_OR_DESCENDANTS`, and `NONE` never does. **This is the opposite of the
/// resource path**, where `ANCESTOR` is not an allow at all
/// ([`crate::resource::MatchType::Ancestor`]). Same enum, two rules.
fn tag_candidates<'a>(
    kind: DecisionKind,
    policies: &'a ServicePolicySet,
    request: &AccessRequest,
    tag_service_def: &ServiceDef,
) -> Result<Vec<TagCandidate<'a>>, Vec<UndecidableCause>> {
    let of_kind: Vec<&'a RangerPolicy> = policies
        .tag_policies
        .iter()
        .filter(|p| p.is_enabled)
        .filter(|p| p.policy_type == kind.policy_type_code())
        .collect();
    if of_kind.is_empty() {
        return Ok(Vec::new());
    }

    let mut why: Vec<UndecidableCause> = Vec::new();

    let Some(resolved) = request.resolved_tags.as_ref() else {
        // A block with policies this decision would evaluate, and no tag set
        // to evaluate them against. `None` is "the caller did not resolve",
        // which must not be read as the empty set that narrows.
        return Err(vec![UndecidableCause::TagsUnresolved]);
    };

    // `RangerTagForEval`'s `equals`/`hashCode` include `matchType` and the
    // enricher hands Ranger a `Set`, so `(name, matchType)` is the identity —
    // never the name alone (SP4d-2b design, D4). The caller's ORDER is dropped
    // here on purpose: the three comparators below are the only ordering that
    // decides anything.
    let mut seen: BTreeSet<(&str, u8)> = BTreeSet::new();
    let mut admitted: Vec<&ResolvedTag> = Vec::new();
    for tag in resolved {
        if !seen.insert((tag.name.as_str(), match_type_rank(tag.match_type))) {
            continue;
        }
        if tag_match_is_admitted(tag.match_type) {
            admitted.push(tag);
        }
    }
    if admitted.is_empty() {
        // Either no tags at all (`getLikelyMatchPolicyEvaluators` returns
        // `EMPTY_LIST` at `isNotEmpty(tags)`) or none the evaluator admits, so
        // no tag policy can write anything — including the ones this crate
        // could not have filtered, which is why the refusal sweep is below
        // this return and not above it.
        return Ok(Vec::new());
    }

    for policy in &of_kind {
        collect_tag_resource_causes(policy, tag_service_def, &mut why);
    }

    let implied = tag_service_def.implied_access_grants();
    let all_access_types: BTreeSet<&str> = tag_service_def
        .access_types()
        .iter()
        .map(|a| a.name.as_str())
        .collect();

    let mut candidates: Vec<TagCandidate<'a>> = Vec::new();
    for tag in admitted {
        let tag_resource = RequestResource::new().with(TAG_RESOURCE_KEY, tag.name.as_str());
        for policy in &of_kind {
            let in_candidate_set = match_policy_resources(policy, &tag_resource, tag_service_def)
                .into_iter()
                .any(|m| m.match_type != MatchType::None);
            if !in_candidate_set {
                continue;
            }
            candidates.push(TagCandidate {
                // Admission is the TAG's match type, already tested above, so
                // this view is matched by construction. Re-deriving `matched`
                // from the policy's own resource would be the one thing
                // `evaluate` does NOT do for a tag request.
                view: PolicyView::for_tag(policy),
                match_type: tag.match_type,
            });
        }
    }

    for candidate in &candidates {
        candidate
            .view
            .collect_causes(kind, request, &implied, &all_access_types, &mut why);
    }
    for issue in tag_service_def.issues() {
        if issue_affects_access(issue) {
            why.push(UndecidableCause::ServiceDef(issue.clone()));
        }
    }
    if !why.is_empty() {
        return Err(why);
    }

    sort_tag_candidates(kind, &mut candidates);
    Ok(candidates)
}

/// Whether `RangerDefaultPolicyEvaluator.evaluate` lets a tag carrying this
/// match type reach the policy items, at the default `SELF` scope with a
/// concrete access type. See [`tag_candidates`] for both versions' routes to
/// this same set.
fn tag_match_is_admitted(match_type: MatchType) -> bool {
    matches!(
        match_type,
        MatchType::Self_ | MatchType::SelfAndAllDescendants | MatchType::Ancestor
    )
}

/// `RangerPolicyResourceMatcher.MATCH_TYPE_COMPARATOR`'s key, ascending.
///
/// MEASURED by running the real comparator over every `MatchType` on both
/// jars, rather than read off the enum's declaration order — which is a
/// DIFFERENT order (`NONE, SELF, DESCENDANT, ANCESTOR, SELF_AND_ALL_DESCENDANTS`)
/// and would silently reorder masks if it were used by mistake. That is also
/// why [`MatchType`] deliberately does not derive `Ord`: there is no ordering
/// on it that is right in every context, so the one that matters is written
/// out here.
///
/// ```text
/// SELF < SELF_AND_ALL_DESCENDANTS < ANCESTOR < DESCENDANT < NONE
/// ```
fn match_type_rank(match_type: MatchType) -> u8 {
    match match_type {
        MatchType::Self_ => 0,
        MatchType::SelfAndAllDescendants => 1,
        MatchType::Ancestor => 2,
        MatchType::Descendant => 3,
        MatchType::None => 4,
    }
}

/// The `Collections.sort` at offsets 237-298 of
/// `getLikelyMatchPolicyEvaluators`, whose `tableswitch` on `policyType` picks
/// one of THREE comparators — `0` and `3` (audit) take the first, `1` the
/// second, `2` the third, and an unknown type is left unsorted with a warning.
///
/// All three were disassembled in both jars and are byte-identical there:
///
/// * `PolicyEvaluatorForTag.EVAL_ORDER_COMPARATOR` delegates to
///   `RangerPolicyEvaluator.EVAL_ORDER_COMPARATOR`: priority DESC, `hasDeny`
///   first, `getEvalOrder()`, then the policy name.
/// * `MATCH_TYPE_COMPARATOR` compares `getTag().getMatchType()` through
///   `RangerPolicyResourceMatcher.MATCH_TYPE_COMPARATOR` and, **only on a tie**,
///   falls through to `NAME_COMPARATOR`.
/// * `NAME_COMPARATOR` delegates to `RangerPolicyEvaluator.NAME_COMPARATOR`,
///   which is **not** just the name: `compare` is priority DESC and only then
///   `compareNormal`, which is `hasDeny` first and then
///   `compareStrings(policy name)`.
///
/// `getEvalOrder()` is `RangerOptimizedPolicyEvaluator.computeEvalOrder()`, a
/// resource-specificity score this crate does not reproduce (the same gap
/// [`sort_eval_order`] documents), so the access ordering collapses onto the
/// name-then-id tie-break and the two comparators are written once here. What
/// that costs is the same as on the resource path and no more: within one
/// `(priority, hasDeny)` band the allow/deny OUTCOME cannot depend on order —
/// in an all-`hasDeny` band an allow never determines, so every evaluator runs
/// and any deny wins; in an all-non-`hasDeny` band no evaluator can deny at all
/// (`hasDeny()` is `isDenyAllElse || !denyPolicyItems.isEmpty()`) — only the
/// reported policy id can differ.
///
/// For policy types 1 and 2 nothing is unreproduced, because neither
/// comparator reads `getEvalOrder()`.
fn sort_tag_candidates(kind: DecisionKind, candidates: &mut [TagCandidate<'_>]) {
    match kind {
        DecisionKind::Access => candidates.sort_by(tag_name_order),
        DecisionKind::DataMask => candidates.sort_by(|a, b| {
            match_type_rank(a.match_type)
                .cmp(&match_type_rank(b.match_type))
                .then_with(|| tag_name_order(a, b))
        }),
        DecisionKind::RowFilter => candidates.sort_by(tag_name_order),
    }
}

/// `RangerPolicyEvaluator.NAME_COMPARATOR`, and — with `getEvalOrder()`
/// unreproduced — `EVAL_ORDER_COMPARATOR` too. The trailing policy-id key is
/// this crate's own stand-in for the score, exactly as in [`sort_eval_order`].
fn tag_name_order(a: &TagCandidate<'_>, b: &TagCandidate<'_>) -> std::cmp::Ordering {
    b.view
        .policy
        .policy_priority
        .cmp(&a.view.policy.policy_priority)
        .then_with(|| b.view.has_deny.cmp(&a.view.has_deny))
        .then_with(|| a.view.policy.name.cmp(&b.view.policy.name))
        .then_with(|| a.view.policy.id.cmp(&b.view.policy.id))
}

/// Everything about one tag policy's RESOURCE map that stops this crate
/// filtering the way `RangerResourceEvaluatorsRetriever` does.
///
/// Swept over every enabled tag policy of the decision's type, not only the
/// ones that came out as candidates — a policy whose candidacy is exactly what
/// cannot be established is not one the crate may quietly leave out.
fn collect_tag_resource_causes(
    policy: &RangerPolicy,
    tag_service_def: &ServiceDef,
    why: &mut Vec<UndecidableCause>,
) {
    let wild_card = tag_service_def
        .resource(PolicyType::from_code(policy.policy_type), TAG_RESOURCE_KEY)
        .map(|r| r.matcher_options.wild_card)
        .unwrap_or(true);

    let maps = std::iter::once(&policy.resources).chain(policy.additional_resources.iter());
    for map in maps {
        let mut push = |problem: TagResourceProblem| {
            why.push(UndecidableCause::TagResourceNotReproducible {
                policy_id: policy.id,
                policy_name: policy.name.clone(),
                problem,
            });
        };
        let mut keys = map.keys();
        let names_only_tag =
            matches!(keys.next(), Some(k) if k == TAG_RESOURCE_KEY) && keys.next().is_none();
        if !names_only_tag {
            push(TagResourceProblem::NotOnlyTheTagResource);
            continue;
        }
        let level = &map[TAG_RESOURCE_KEY];
        if level.is_excludes {
            push(TagResourceProblem::Excludes);
        }
        for value in &level.values {
            if !trie_reproduces_matcher(value, wild_card) {
                push(TagResourceProblem::ValueNotTriePrefixExact(value.clone()));
            }
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
        | Unmodelled::TagPolicies(_)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `MATCH_TYPE_COMPARATOR`'s order, pinned against a MEASUREMENT rather
    /// than against the enum.
    ///
    /// Produced by sorting every `MatchType` with the real
    /// `RangerPolicyResourceMatcher.MATCH_TYPE_COMPARATOR` on the
    /// `ranger-plugins-common` 2.5.0 and 2.8.0 jars, which agreed:
    ///
    /// ```text
    /// [SELF, SELF_AND_ALL_DESCENDANTS, ANCESTOR, DESCENDANT, NONE]
    /// ```
    ///
    /// The second assertion is the one that earns its place: Java's
    /// `values()` — i.e. `ordinal()`, the order any "just use the enum"
    /// shortcut would inherit — is a DIFFERENT order, and using it would put
    /// `DESCENDANT` ahead of `ANCESTOR` and `NONE` first of all.
    #[test]
    fn match_type_rank_is_the_measured_comparator_order() {
        let mut all = [
            MatchType::None,
            MatchType::Self_,
            MatchType::Descendant,
            MatchType::Ancestor,
            MatchType::SelfAndAllDescendants,
        ];
        all.sort_by_key(|m| match_type_rank(*m));
        assert_eq!(
            all,
            [
                MatchType::Self_,
                MatchType::SelfAndAllDescendants,
                MatchType::Ancestor,
                MatchType::Descendant,
                MatchType::None,
            ]
        );

        // Java's declaration order, for contrast — NOT the comparator's.
        let declaration_order = [
            MatchType::None,
            MatchType::Self_,
            MatchType::Descendant,
            MatchType::Ancestor,
            MatchType::SelfAndAllDescendants,
        ];
        assert_ne!(
            all, declaration_order,
            "the comparator order and the enum's ordinal order differ; a rank \
             taken from the declaration would reorder masks"
        );
    }

    /// The admitted set, stated once as a table so the two versions' routes to
    /// it cannot drift apart from the predicate.
    #[test]
    fn only_self_self_and_all_descendants_and_ancestor_are_admitted() {
        assert!(tag_match_is_admitted(MatchType::Self_));
        assert!(tag_match_is_admitted(MatchType::SelfAndAllDescendants));
        assert!(tag_match_is_admitted(MatchType::Ancestor));
        assert!(!tag_match_is_admitted(MatchType::Descendant));
        assert!(!tag_match_is_admitted(MatchType::None));
    }
}
