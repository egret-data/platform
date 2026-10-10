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

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use crate::model::{
    has_double_brackets, has_request_expression, AccessEntry, ItemBucket, PolicyItem, RangerPolicy,
    ServicePolicySet, TagPolicyBlock, Unmodelled,
};
use crate::resource::{
    level_excluded_by_trie, match_policy_resources, trie_reproduces_matcher, MatchType,
    RequestResource,
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
    /// [`EngineConfig`]'s `Option` fields use, and it stays distinct from
    /// `Some(empty)` because the narrowing is sound only if the caller's tag
    /// resolution is at least as broad as the enricher's. Ranger's
    /// `RangerTagEnricher` walks the hierarchy and emits ancestor tags carrying
    /// `MatchType::ANCESTOR`, which Ranger **admits** for a tag request (unlike
    /// a resource request — see [`crate::resource::MatchType::Ancestor`]), so a
    /// caller matching by exact string equality can report "no tags" for a
    /// column while the engine is enforcing a tag policy through a tag attached
    /// to its table.
    ///
    /// ⚠ **This paragraph used to say egret could not establish that antecedent
    /// at all, and cited `tag_keys_for_resource_via`.** Both halves moved. That
    /// function is the ATTRIBUTION query and was never the one a caller would
    /// pass; SP4d-2b Task 1 wrote `resolved_tags_for_resource_via`, which rolls
    /// the table level up and returns `tags.name`; and SP4d-2b Task 7's
    /// `resolved_tags_for_target` (`egret-perm-compile/src/routes.rs`) gates that
    /// on the one stored shape it cannot roll up and passes `Some` otherwise. So
    /// egret establishes the antecedent today for the levels its storage was
    /// designed to express, and declines — `None` — for every state where it
    /// cannot.
    ///
    /// Live tag assignments are table- and column-level, so the schema- and
    /// catalog-level ancestor paths are not exercised by current data — which is
    /// exactly how treating `None` as empty, or passing an exact-match set as
    /// complete, would ship green and wrong. This crate therefore still only
    /// **offers** the narrowing; whether passing `Some` is sound remains a
    /// property of the caller, not of this type.
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
    /// [`ResolvedTag::identity`], which is `RangerTagForEval`'s own.
    ///
    /// # Why the newtype
    ///
    /// SP4d-2b Task 7. This was a bare `Option<Vec<ResolvedTag>>`, and the
    /// ordering obligation phase 1 is under — evaluate the set
    /// `RangerTagEnricher.processServiceTags` LEFT, not the one the caller
    /// handed in — was a convention held at two call sites. See
    /// [`UnenrichedTags`].
    pub resolved_tags: Option<UnenrichedTags>,
}

/// The two tag-set types and their ONE producer, in a module so that the
/// producer is the only thing that can build an [`EnrichedTags`].
///
/// # ⚠ Why a module and not just two newtypes in `eval`
///
/// **SP4d-2b Task 7 fix round 1, measured by the review.** `EnrichedTags`' tuple
/// field is module-private, and before this every phase function AND both
/// `evaluate` sites lived in the same module as the type — so
/// `EnrichedTags(t.0.clone())` compiled, and the claim "no way to reach the phase
/// without having run the enricher" was true across modules and FALSE in the one
/// file where every call site is. The guard there was four tests in
/// `tests/tag_refusals.rs`, not the type.
///
/// With the type and [`enriched_tags`] alone inside `tags`, and every consumer
/// outside it, the bypass is now a compile error from anywhere in the crate: the
/// field is not reachable and there is no other constructor. `already_enriched`
/// is the single exception, `#[cfg(test)]`, for `tag_eval_tests`, which drives the
/// phase directly and holds no RESOURCE service-def to enrich against.
///
/// **What is unrepresentable, and from where:** constructing an `EnrichedTags`
/// anywhere but [`enriched_tags`], in any module of this crate or any other, in a
/// non-test build. **What is not:** calling [`enriched_tags`] with the wrong
/// service-def, which is a correctness question no type here settles — it is
/// ruled on in that function's own ⚠ block ("which service-def, and why it is not
/// the tag def").
mod tags {
    use super::{service_resource_validates, AccessRequest, ResolvedTag};
    use crate::servicedef::ServiceDef;

    /// The tag set the CALLER resolved, as the caller resolved it — i.e. BEFORE
    /// `RangerTagEnricher.processServiceTags` has dropped anything from it. The only
    /// thing that can be put in [`AccessRequest::resolved_tags`].
    ///
    /// # ⚠ Why this is a newtype: it makes "the phase read the UN-enriched set" unrepresentable
    ///
    /// **SP4d-2b Task 7, carried from Task 4 rather than churning these sites
    /// twice.** Phase 1 must evaluate the set `processServiceTags` left behind, not
    /// the one the caller supplied: Ranger's order is
    /// `RangerDefaultRequestProcessor.preProcess` → the enricher writes the
    /// surviving set into the request context → `evaluateTagPolicies` reads it back
    /// out (`RangerAccessRequestUtil.getRequestTagsFromContext`, offset 104).
    ///
    /// Until this task nothing STRUCTURALLY held that order. [`evaluate`] and
    /// [`evaluate_sub`] each cloned the request with the enriched set into a second
    /// binding, and then every phase function took an `&AccessRequest` that might be
    /// either one: ONE field with two meanings, told apart only by which local the
    /// author passed. That is the two-guards-must-agree shape this package has been
    /// bitten by three times, and nothing would have failed if a future edit had
    /// passed `request` where `tag_request` was meant — the tag set would simply
    /// have been the un-dropped one.
    ///
    /// Now the phase takes `Option<&`[`EnrichedTags`]`>`, which only
    /// [`enriched_tags`] produces. There is no `AccessRequest` in flight carrying an
    /// enriched set, and — since fix round 1 put both types and that producer in
    /// their own module — no way to construct one anywhere else in a non-test
    /// build. **The exact scope of that claim, and what it does NOT cover, is on
    /// the `tags` module above**; the first version of this paragraph overstated it
    /// for `eval.rs` itself, where every call site lives.
    ///
    /// `None` still means "the caller did not resolve", through the `Option`
    /// [`enriched_tags`] preserves — never the empty set, which is the measured
    /// narrowing.
    ///
    /// [`UnenrichedTags::as_slice`] exists for a CALLER that has to read back what
    /// it supplied (the differential harness serialises it into the Java case file).
    /// **Nothing in `src/` calls it and nothing in `src/` may**: inside the phase the
    /// parameter's type is the whole guard, so reaching through
    /// `request.resolved_tags` there would be visibly reaching for the wrong set.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct UnenrichedTags(Vec<ResolvedTag>);

    impl UnenrichedTags {
        /// The one constructor. Deliberately not a `From<Vec<_>>` impl: putting a
        /// tag set in a request is the caller making the soundness claim documented
        /// on [`AccessRequest::resolved_tags`], and that is worth naming at the site
        /// rather than performing by coercion.
        pub fn new(tags: Vec<ResolvedTag>) -> Self {
            Self(tags)
        }

        /// What the caller supplied, for a caller reading back its own input. See
        /// the ⚠ block above for why no code in `src/` uses this.
        pub fn as_slice(&self) -> &[ResolvedTag] {
            &self.0
        }
    }

    /// [`UnenrichedTags`] after the one surviving effect of
    /// `RangerTagEnricher.processServiceTags` on a decision — the attachment DROP.
    /// [`enriched_tags`] is the only producer outside this crate's own tests, which
    /// is what makes the enrichment ordering a type and not a convention.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct EnrichedTags(Vec<ResolvedTag>);

    impl EnrichedTags {
        /// The measured narrowing's antecedent: with no tag surviving the enricher,
        /// `getLikelyMatchPolicyEvaluators` returns `Collections.EMPTY_LIST` at its
        /// `isNotEmpty(tags)` branch. Asked of the ENRICHED set on purpose — that is
        /// the set Ranger reads back out of the context.
        pub(crate) fn is_empty(&self) -> bool {
            self.0.is_empty()
        }

        pub(crate) fn as_slice(&self) -> &[ResolvedTag] {
            &self.0
        }

        /// **Test-only**, and the `#[cfg(test)]` is the point: `tag_eval_tests`
        /// drives the phase directly and holds no RESOURCE service-def to enrich
        /// against, so it presents its fixture as already enriched. No production
        /// path can reach this — [`enriched_tags`] is the only other producer.
        #[cfg(test)]
        pub(crate) fn already_enriched(tags: Vec<ResolvedTag>) -> Self {
            Self(tags)
        }
    }
    /// `RangerTagEnricher.processServiceTags`' one surviving effect on a decision:
    /// **which attachments exist at all** (SP4d-2b design, D9).
    ///
    /// Returns the request's tag set with every attachment Ranger would have
    /// dropped removed, or `None` when [`AccessRequest::resolved_tags`] is `None` —
    /// which stays `None`, because "the caller did not resolve" is not something an
    /// enricher can turn into a set.
    ///
    /// # What was measured, and where it is in the bytecode
    ///
    /// `processServiceTags` (2.5.0 offsets 91-234, identical shape in 2.8.0) walks
    /// `serviceTags.getServiceResources()` with a `ListIterator` and, for each
    /// resource, calls `createRangerServiceResourceMatcher`. When that returns
    /// **null** it does two things, at offsets 151 and 158-167:
    ///
    /// ```java
    /// iter.remove();                                        // 151
    /// serviceTags.getResourceToTagIds().remove(res.getId()); // 158-167
    /// ```
    ///
    /// — so the resource AND its tag links go, and no `RangerTagForEval` is ever
    /// built for that pair. Reproduced here by dropping the attachment, which is
    /// the same thing once the payload has been resolved into one entry per
    /// `(resource, tag)` pair.
    ///
    /// `createRangerServiceResourceMatcher` returns null exactly when **no** policy
    /// type validates the key set (offsets 57-296): it loops
    /// `RangerPolicy.POLICY_TYPES` — `{0, 1, 2}`, read from that class's `static {}`
    /// block — and for each asks whether any hierarchy of
    /// `RangerServiceDefHelper.getResourceHierarchies(type)` satisfies
    /// `hierarchyHasAllResources(hierarchy, keySet)`. That method (offsets 62-162)
    /// is a **subset** test — every name in `keySet` appears somewhere in the
    /// hierarchy — not a covering one, so `{catalog}` validates against Trino's
    /// `catalog/schema/table/column` chain and an EMPTY key set validates
    /// vacuously. Only a name no hierarchy of any policy type carries drops a
    /// resource.
    ///
    /// # ⚠ Which service-def, and why it is not the tag def
    ///
    /// The helper is the enricher's own `serviceDefHelper`, built by
    /// `RangerAbstractContextEnricher` from the **enriched service's** def — Trino's
    /// or StarRocks' — not from the `tagPolicies` block's `tag` def. Validating
    /// against the tag def instead would find one resource (`tag`), so every real
    /// attachment would fail every hierarchy and the whole tag set would vanish:
    /// a silent, total fail-CLOSED for masks and fail-OPEN for denies. That is why
    /// this runs in [`evaluate`] / [`evaluate_sub`], which hold the resource def,
    /// and not inside [`tag_candidates`], which is handed the tag def.
    ///
    /// # ⚠ `isEnabled` is NOT consulted, and that is measured, not assumed
    ///
    /// `RangerServiceResource.isEnabled` appears nowhere in `processServiceTags`;
    /// the only branch is on the matcher being null. [`TagServiceResource`] carries
    /// the field anyway so that `tests/tag_refusals.rs` can assert the inertness
    /// instead of the crate silently not having somewhere to put it.
    ///
    /// # Reach, by code path
    ///
    /// ⚠ **SP4d-2b TASK 7 MADE THE DROP LIVE, and this row used to say it was
    /// unreachable.** `egret_perm_compile`'s `attachment_for` is a producer, and the
    /// levels it synthesises are `ServiceTags::from_assignments`' own LITERAL keys —
    /// `catalog` / `schema` / `table` / `column` — because that is what
    /// `download_tags` serves, for every service type, with no engine argument at
    /// all.
    ///
    /// So the drop fires, and it fires for a real configuration: **StarRocks' def has
    /// no `schema` resource** (its middle level is `database`), so for a StarRocks
    /// profile carrying a `tagService` every attachment fails
    /// `service_resource_validates` and the whole tag set vanishes — which is what the
    /// real `processServiceTags` does with that payload too, and is therefore the
    /// right answer here. The DEFECT is in the writer (tag-selected masks and
    /// clearances cannot reach a StarRocks engine at all); this function reproduces
    /// it rather than hiding it, and `attachment_for`'s own ⚠ block carries the
    /// measurement.
    ///
    /// **SP4d debt Task 2 made that configuration unrepresentable, and this path
    /// still must not be weakened.** `PolicyStore::create_service` refuses a
    /// non-Trino profile carrying a `tagService` (it is the only `INSERT INTO
    /// policy.authz_services` in any `src/`), and migration 0176's VALIDATED
    /// `CHECK (config_jsonb ->> 'tagService' IS NULL OR type = 'trino')` refuses it
    /// for every `INSERT` and every `UPDATE` — including the `SET type =
    /// 'starrocks'` direction, which has no Rust writer at all — after clearing the
    /// key from any row that already carried it. So no StarRocks profile can hold a
    /// `tagService`, and the configuration in which this drop CHANGES AN OUTCOME is
    /// gone.
    ///
    /// The drop itself is not dead and must stay, for three reasons that are about
    /// code paths rather than about that configuration. (1) It still FIRES for every
    /// StarRocks profile with a tagged column: `resolved_tags` comes from
    /// `catalog_tag_assignments`, not from the `tagService`, which only ever decided
    /// whether a tag POLICY existed for the drop to discard. (2) It reproduces what
    /// the real `processServiceTags` does with that payload, which is what makes the
    /// inertness ASSERTABLE rather than argued. (3) `attachment_for` is a producer,
    /// and a level set it learns to emit later reaches this subset test with no
    /// configuration change at all. **Falsified by** the per-engine resource-level
    /// mapping landing, or by migration 0176's constraint being dropped.
    ///
    /// For a TRINO profile the drop is inert and provably so: the only level sets the
    /// mapping emits are `{catalog, schema, table}` and `{catalog, schema, table,
    /// column}`, both of which are the subset test's own hierarchy. **Falsified by**
    /// the mapping learning to emit another level set, or a second producer (a
    /// `/tags/download` reader, an import, a shim).
    pub(crate) fn enriched_tags(
        request: &AccessRequest,
        service_def: &ServiceDef,
    ) -> Option<EnrichedTags> {
        let resolved = request.resolved_tags.as_ref()?;
        // `.0` is THE reader of `UnenrichedTags`' contents in `src/` — see the ⚠
        // block on that type. Everything downstream of here sees `EnrichedTags`.
        Some(EnrichedTags(
            resolved
                .0
                .iter()
                .filter(|tag| service_resource_validates(&tag.attached_through.levels, service_def))
                .cloned()
                .collect(),
        ))
    }
}

pub use tags::UnenrichedTags;
pub(crate) use tags::{enriched_tags, EnrichedTags};

/// One tag ATTACHMENT the caller resolved for [`AccessRequest::resource`]: one
/// `(RangerServiceResource, RangerTag)` pair of the tag download, plus the
/// `MatchType` the enricher stamped on it.
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
///
/// # ⚠ Every field of `RangerTag` that Ranger reads has a row here
///
/// **SP4d-2b Task 4.** The type used to carry `name` and `match_type` only, and
/// that was a COARSENING of `RangerTagForEval` — whose `equals`/`hashCode` read
/// `type`, `attributes`, `matchType`, `options` AND `validityPeriods` (measured
/// at offsets 29-130 of `hashCode` and 63-196 of `equals`, identically in 2.5.0
/// and 2.8.0). Dropping three of the five silently merged two members Ranger
/// keeps apart. The standing rule: **a field added to `RangerTag`, to
/// `RangerTagForEval` or to `RangerServiceResource` is a new field here and a
/// new row in the table below** — there is no `Default`, so a new field is a
/// compile error at every construction site, which is where the ruling belongs.
///
/// | `RangerTag` field | modelled | ruling |
/// |---|---|---|
/// | `type` | yes, as `name` | the identifier the wire carries and the engine matches |
/// | `attributes` | yes | in the identity; a policy REFERENCE to one refuses through the `${{ … }}` / `conditions` sweeps (D8) |
/// | `options` | keys only | non-empty ⇒ [`TagAttachmentProblem::Options`]; enough to refuse, and nothing here needs more |
/// | `validityPeriods` | count only | non-empty ⇒ [`TagAttachmentProblem::ValidityPeriods`]; likewise |
/// | `owner` | no | `RangerTagForEval`'s constructor never reads it (offsets 11-92), so it reaches no decision |
/// | `id` / `guid` / `createdBy` / … | no | `RangerBaseModelObject` bookkeeping; no evaluator reads it |
///
/// | `RangerServiceResource` field | modelled | ruling |
/// |---|---|---|
/// | `resourceElements` | keys only, as `attached_through.levels` | the key SET is the whole of what `createRangerServiceResourceMatcher` validates, and the VALUES are already spent — they are what produced `match_type` |
/// | `isEnabled` | yes, and **never read** | `processServiceTags` never consults it (D9); modelled so that the inertness is assertable rather than a missing field nobody notices |
/// | `serviceName` / `id` / `resourceSignature` | no | the id keys `resourceToTagIds`, which this shape has already resolved away — one attachment per pair |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTag {
    pub name: String,
    pub match_type: MatchType,
    /// `RangerTag.attributes`, which `RangerTagForEval`'s constructor copies
    /// verbatim (`emptyMap()` when null) and its `equals` compares with
    /// `Map.equals` — order-independent, which is why this is a `BTreeMap`.
    pub attributes: BTreeMap<String, String>,
    /// The KEYS of `RangerTag.options`. Any key at all refuses: `isApplicable`
    /// reads `TAG_VALIDITY_PERIODS` out of this map and re-derives the validity
    /// evaluators from it (offsets 62-128), and the map is also a component of
    /// the identity, so this crate can neither evaluate it nor drop it.
    pub options: BTreeSet<String>,
    /// How many `RangerTag.validityPeriods` the payload carried. Non-zero
    /// refuses: `RangerTagForEval`'s constructor turns each into a
    /// `RangerValidityScheduleEvaluator` (the `invokespecial` at offset 89;
    /// `aload_0` occupies 87-88) and
    /// `getLikelyMatchPolicyEvaluators` then SKIPS the tag when
    /// `isApplicable(accessTime)` is false (offset 62) — a filter whose answer
    /// depends on `RangerAccessRequest.getAccessTime()`, which the PLUGIN sets
    /// (`RangerAccessRequestImpl`'s constructor leaves it null and
    /// `RangerDefaultRequestProcessor` never writes it) and this crate does not
    /// model.
    pub validity_periods: usize,
    /// The `RangerServiceResource` this attachment came through.
    pub attached_through: TagServiceResource,
}

impl ResolvedTag {
    /// `RangerTagForEval`'s `equals`, as a comparable key: `type`,
    /// `attributes`, `matchType`, `options`, `validityPeriods` — all five
    /// components, in the order `equals` compares them (offsets 32-196;
    /// `hashCode` folds the same five in a different order at 29-130).
    ///
    /// `attached_through` is deliberately NOT in it: `RangerTagForEval` carries
    /// no service resource, so two attachments of one tag at two resources that
    /// produced the same match type are ONE member of Ranger's set. Putting the
    /// resource in the key would invent a member Ranger does not have.
    ///
    /// # ⚠ Two of the five are LOSSY PROJECTIONS, and the refusal is what covers that
    ///
    /// FIX ROUND 1, MINOR 2. All five components are PRESENT, which is what the
    /// inherited dependency required — but two are not Ranger's values:
    ///
    /// | component | Ranger compares | this key holds | can two members collapse? |
    /// |---|---|---|---|
    /// | `type` | `String.equals` | the same `String` | no |
    /// | `attributes` | `Map.equals` — keys AND values | the same `BTreeMap` | no |
    /// | `matchType` | `MatchType.equals` | [`match_type_rank`], injective over all five variants | no |
    /// | `options` | `Map.equals` — keys AND values | the KEY SET only | **yes** — two members differing only in an option's VALUE |
    /// | `validityPeriods` | `List.equals` over `RangerValiditySchedule` | the COUNT only | **yes** — two members differing only in a schedule's CONTENT |
    ///
    /// Neither collapse can produce a wrong verdict, and the reason is the
    /// REFUSAL, not the key: a tag carrying any option key or any validity
    /// period is refused in [`tag_candidates`] whenever the evaluator admits it
    /// and a tag policy of the kind being decided exists — i.e. in every state
    /// where a collapsed member could have changed an answer. Where the refusal
    /// does not fire (no admitted tag, or no policy of that kind) nothing reads
    /// the set at all.
    ///
    /// **So the safety here is a property of two functions, not of one**, which
    /// is the pairing this package keeps getting bitten by. If the refusal is
    /// ever narrowed, these two projections have to become Ranger's values in
    /// the SAME commit — the identical rule the `attributes` widening was under.
    fn identity(
        &self,
    ) -> (
        &str,
        &BTreeMap<String, String>,
        u8,
        &BTreeSet<String>,
        usize,
    ) {
        (
            self.name.as_str(),
            &self.attributes,
            match_type_rank(self.match_type),
            &self.options,
            self.validity_periods,
        )
    }
}

/// The `RangerServiceResource` one [`ResolvedTag`] came through, reduced to
/// what the enricher reads off it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagServiceResource {
    /// The `resourceElements` KEY SET.
    ///
    /// `createRangerServiceResourceMatcher` validates exactly this set, and
    /// nothing else about the element map: it asks
    /// `RangerServiceDefHelper.hierarchyHasAllResources(hierarchy, keySet)` for
    /// each hierarchy of each of `RangerPolicy.POLICY_TYPES` (`{0, 1, 2}`,
    /// read from the class's `static {}` block), and that method tests
    /// `keySet ⊆ hierarchy` — a SUBSET test, not a covering one, so
    /// `{catalog}` alone validates against Trino's `catalog/schema/table/column`
    /// chain. See [`enriched_tags`].
    ///
    /// # ⚠ A `BTreeSet`, and the type IS the guarantee
    ///
    /// FIX ROUND 1, MINOR 1. This was a `Vec<String>` documented as "the keys",
    /// and the provenance refusal counts it (`len() < 3`). A `Vec` lets the two
    /// disagree: `["catalog", "catalog", "catalog"]` has three entries and a key
    /// set of one, so the crate ANSWERED for exactly the one-level attachment
    /// D2 exists to refuse (measured, before the fix:
    /// `Allowed { policy_id: Some(10), policy_name: "tag-allow" }`).
    ///
    /// Unreachable when it was fixed — no producer of a [`ResolvedTag`] existed
    /// outside this crate's tests — and **SP4d-2b Task 7 then wrote that
    /// producer**: `egret_perm_compile`'s `attachment_for` builds this set from
    /// `ServiceTags::from_assignments`' own rule (`catalog`, plus each of
    /// `schema`/`table`/`column` that is non-empty), so it cannot emit a
    /// duplicate — but the fix is still the TYPE and not a second check beside
    /// the first: the count and the key set cannot drift apart again whatever
    /// the next producer does. `resourceElements` is a
    /// `Map<String, RangerPolicyResource>` on the wire, so a set loses nothing;
    /// the payload's key ORDER was never read by anything.
    pub levels: BTreeSet<String>,
    /// `RangerServiceResource.isEnabled`.
    ///
    /// **Modelled so that it can be proven inert, and never read.**
    /// `processServiceTags` (2.5.0 offsets 100-234, same in 2.8.0) iterates the
    /// service resources and branches only on whether
    /// `createRangerServiceResourceMatcher` returned null; `isEnabled` appears
    /// nowhere in it. A field this crate omitted instead would leave nothing to
    /// assert, and the next reader would have to re-measure to find out whether
    /// the omission was a ruling or an oversight.
    pub is_enabled: bool,
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
    /// For an ACCESS decision the same gap is **almost** harmless, and the
    /// exception is SP4d-2b Task 3's: within one band the allow/deny outcome
    /// does not depend on order (see the sort in [`evaluate`]) **unless a tag
    /// policy has DENIED**, in which case offset 469's `>=` reads a
    /// `policyPriority` an allowing band member has just raised, and whichever
    /// of an allowing and a denying member ran first decides the request. That
    /// case raises this same variant from [`evaluate`]'s loop. Every other
    /// ACCESS shape is still order-free for the verdict and differs only in the
    /// reported id, which is a declared divergence. For a mask it is not, because the engine's
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
    /// For a `RangerTagAccessRequest` `evaluate` reads the TAG's match type and
    /// never touches the policy's matcher, so the trie alone decides which tag
    /// policies run — and where the trie is wider than the matcher, this crate
    /// would skip a policy Ranger evaluates. For a DENY tag policy that is
    /// fail-open.
    ///
    /// # ⚠ FIX ROUND 2: the resource-path half of this paragraph was FALSIFIED
    ///
    /// It used to open *"for a resource request the trie is a pre-filter and
    /// `RangerDefaultPolicyEvaluator.evaluate` re-asks the matcher, so an
    /// over-returning trie changes nothing"* — true when written, and the
    /// licence under which this crate takes every enabled policy as a candidate
    /// on the resource path.
    ///
    /// Modelling offset 553 made it false. `if (result.getIsAllowed() &&
    /// !evaluator.hasDeny()) setIsAccessDetermined(true)` is in the ENGINE loop,
    /// after `evaluate` returns, and reads no matcher — so an over-returning
    /// candidate list now DOES change the answer. The premise did not stop being
    /// true because anything about the trie changed; it stopped being true
    /// because a new mechanism was modelled beside it, and nothing re-checked
    /// it. [`trie_excludes`] is the resource-path answer, and its doc carries
    /// the jar measurement.
    TagResourceNotReproducible {
        policy_id: Option<i64>,
        policy_name: String,
        problem: TagResourceProblem,
    },
    /// A tag MASK or ROW-FILTER policy produced a result AND is deny-capable
    /// (`denyPolicyItems` or `isDenyAllElse`), so Ranger can report this
    /// policy's id with a LATER policy's mask. See [`evaluate_tag_sub`].
    TagSubPolicyIsDenyCapable {
        policy_id: Option<i64>,
        policy_name: String,
    },
    /// A tag policy the TAG policy repository would NORMALISE, in a way this
    /// crate does not reproduce (SP4d-2b Task 5).
    ///
    /// # ⚠ Found by the differential gate, and only the TAG path has it
    ///
    /// `RangerPolicyRepository`'s `ServicePolicies$TagPolicies` constructor —
    /// and **no other constructor in either jar**, which is the whole reason
    /// this was invisible — calls
    /// `normalizeAndPrunePolicies(tagPolicies.getPolicies(),
    /// componentServiceDef.getName())` at offsets 90-103, BEFORE `init()` and
    /// therefore before `skipBuildingPolicyEvaluator` has dropped anything. So
    /// it runs over EVERY tag policy in the block, disabled ones included.
    ///
    /// Measured in 2.5.0 and 2.8.0 (`normalizeAndPrunePolicyItems`, offsets
    /// 109-177 for the access half and 243-325 for the mask half; the
    /// `commons-lang` → `commons-lang3` move is the only difference):
    ///
    /// ```java
    /// String prefix = componentType + ":";                 // e.g. "trino:"
    /// for (RangerPolicyItemAccess access : item.getAccesses()) {
    ///     String type = access.getType();
    ///     if (StringUtils.startsWith(type, prefix)) {
    ///         access.setType(StringUtils.removeStart(type, prefix));   // STRIPPED
    ///     } else if (type.contains(":")) {
    ///         toRemove.add(access);                                    // DROPPED
    ///     }
    /// }
    /// // …and the same two arms for a RangerDataMaskPolicyItem's dataMaskType,
    /// // where the DROP removes the whole ITEM rather than one access.
    /// ```
    ///
    /// The prefix is the component service-DEF name (`componentServiceDef.getName()`
    /// — `trino`, `starrocks`), **not** the service name: the 4th constructor
    /// argument is `ServicePolicies.getServiceName()` and is used elsewhere, and
    /// confusing the two would strip the wrong prefix.
    ///
    /// This crate compares a tag policy item's access type to the request's as a
    /// literal. So a tag DENY policy granting `trino:select` denies a `select`
    /// request in Ranger and matches nothing here — **fail-open**, and the
    /// direction that matters.
    ///
    /// # Why this REFUSES rather than reproducing the strip
    ///
    /// The rule is fully measured and could be modelled. It is refused because
    /// modelling it correctly means modelling the two prunes it feeds (an item
    /// whose accesses all dropped, and a policy whose buckets all emptied) and
    /// their interaction with `hasDeny` and with the tag comparators, and
    /// because the refusal costs nothing on the population egret actually
    /// serves: `tests/fixtures/tag-policies-live.json`, the download captured
    /// from a live plugin's own cache, carries BARE access types
    /// (`select`, `insert`, `delete`) on all three of its policies, and so does
    /// everything `egret-perm-compile` emits. The namespaced spelling reaches
    /// egret only through the raw-JSON policy editor, which is the same
    /// reachability argument [`UndecidableCause::TagSubPolicyIsDenyCapable`] was
    /// added under. Modelling it is named as follow-up work in
    /// `tests/ranger/README.md` rather than guessed at here.
    ///
    /// The one arm of the normalisation that IS reproduced is the whole-policy
    /// prune, because its gate is a plain read of the wire and nothing about it
    /// is ambiguous — see [`tag_policy_is_pruned`].
    TagPolicyNotNormalisable {
        policy_id: Option<i64>,
        policy_name: String,
        problem: TagNormalisationProblem,
    },
    /// Two tag policies at DIFFERENT `policyPriority` both decide, and which of
    /// them the engine keeps is settled by an arbitration this crate does not
    /// model (SP4d-2b Task 5).
    ///
    /// # ⚠ Found by the differential gate, and it falsified this file's prose
    ///
    /// [`evaluate_tag_policies`] used to say *"unlike the resource loop this one
    /// compares no priorities: the first evaluator to determine is final"*. That
    /// is false, and the mechanism is not in `evaluateTagPolicies` at all — which
    /// is why reading that method could not find it.
    ///
    /// `RangerDefaultPolicyEvaluator.evaluatePolicyItems` keeps a per-access-type
    /// result map, `RangerAccessRequestUtil.getAccessTypeResults(request)`, which
    /// lives in the request CONTEXT and therefore **survives from one tag
    /// evaluator to the next** (every `RangerTagAccessRequest` wraps the same
    /// request). Offsets 366-573 in 2.8.0 arbitrate the slot by
    /// `policyPriority`, with the comparison depending on what is already stored:
    /// `<=` when an ACCESS policy overwrites a stored ALLOW, `<` otherwise, and a
    /// further pair of arms when the stored entry is a DENY. `getCompositeAccessResult`
    /// then copies the surviving slot back.
    ///
    /// MEASURED on the real 2.8.0 engine, three cases differing only in
    /// `policyPriority` (one tag allow and one tag deny, both matching the same
    /// tag, nothing else in the download):
    ///
    /// | allow priority | deny priority | Ranger |
    /// |---|---|---|
    /// | 1 | 0 | **Allowed** by the allow |
    /// | 0 | 0 | Denied by the deny |
    /// | 0 | 1 | Denied by the deny |
    ///
    /// The first row is the one this crate got wrong: it answered `Denied`, and
    /// ten further probe variants agreed with the table.
    ///
    /// # Why this REFUSES rather than modelling the arbitration
    ///
    /// The table above is consistent with "the highest-priority deciding
    /// candidate wins, and at equal priority a deny beats an allow", and that
    /// rule matched all thirteen measured rows.
    ///
    /// ⚠ **FIX ROUND 1: one of the two reasons this doc gave for refusing was
    /// FALSE, and it is removed rather than softened.** It claimed the five-armed
    /// branch structure at offsets 366-573 "is not reliably readable from a
    /// disassembly" because the operand order of the `if_icmplt` / `if_icmple`
    /// arms is not recoverable. It is recoverable: `javap -c -p` prints the whole
    /// instruction sequence and the operand order is fixed by the JVM spec
    /// (value1 is pushed first). The review decoded all five arms in one pass:
    ///
    /// ```text
    /// stored == null                       → put
    /// stored ALLOWED, this DENIED          → put when thisPrio >= storedPrio
    /// stored ALLOWED, this ALLOWED, ACCESS → put when thisPrio >  storedPrio
    /// stored ALLOWED, this ALLOWED, other  → put when thisPrio >= storedPrio
    /// stored DENIED                        → put when thisPrio >  storedPrio
    ///                                        (or >= with an ANY access type)
    /// ```
    ///
    /// **A refusal defended by a false reason is the thing this package has been
    /// bitten by five times**, so the reason that holds is the only one left:
    ///
    /// * the same `accessTypeResults` map is live on the RESOURCE path, where
    ///   this crate models a DIFFERENT mechanism (the engine loop's offsets
    ///   469/505/529), so a rule written here in terms of priority would be a
    ///   SECOND model of one mechanism until that audit is done.
    ///
    /// ⚠ **WHOLE-BRANCH REVIEW: the sentence that used to close that bullet was
    /// FALSIFIED and is deleted rather than softened.** It read *"The two coincide
    /// there — the engine loop shuts out a lower-priority evaluator before it can
    /// reach the map — but 'green over 4000 cases' is not a proof"*. They do not
    /// coincide, the green was over a population the shape is absent from (the
    /// corpus levels every random resource policy to `policyPriority: 0`), and the
    /// divergence was a confident over-deny on a shape reachable by one supported
    /// API call. It is now refused by
    /// [`UndecidableCause::AccessPriorityCompositeNotModelled`], whose doc carries
    /// the measurement. **The two models were not in agreement; one of them was
    /// untested.**
    ///
    /// And a fact that STRENGTHENS it rather than weakening it, which the first
    /// round missed: `evaluatePolicyItems` is near-identical between 2.5.0 and
    /// 2.8.0 in the 366-573 region (the only deltas in the whole method are the
    /// `ignoreDescendantDeny` arms and the `getCompositeAccessResult` signature).
    /// So **this map cannot be what makes
    /// [`UndecidableCause::TagDenyReopenedDiffersByPluginVersion`] a version
    /// split** — there is a second unmodelled mechanism, and writing a priority
    /// rule here would encode one of the two while the other stayed invisible.
    ///
    /// **The cost is measured and it is zero on egret's own data**:
    /// `egret-perm-compile` never emits `policyPriority` on a tag policy, and all
    /// three policies of `tests/fixtures/tag-policies-live.json` — the download
    /// captured from a live plugin's cache — carry `policyPriority: 0`. A
    /// single-priority tag block cannot reach this refusal at all, because the
    /// equal-priority case is the one this crate already reproduces.
    ///
    /// Modelling it is named as follow-up work in `tests/ranger/README.md`, with
    /// the measured table to model it against.
    TagPriorityArbitrationNotModelled {
        /// Every deciding tag policy, and its priority, in candidate order.
        policy_ids: Vec<Option<i64>>,
        policy_names: Vec<String>,
        priorities: Vec<i32>,
    },
    /// A tag DENY and a STRICTLY HIGHER-priority resource policy that allows —
    /// a shape the two plugin versions answer DIFFERENTLY, and this crate
    /// reproduces only 2.8.0's answer (SP4d-2b Task 5).
    ///
    /// # ⚠ Found by the differential gate, and it is a VERSION split
    ///
    /// MEASURED on both real engines, on one minimal download (a tag ACCESS
    /// policy denying `select` to bob at priority 0, one resource ACCESS policy
    /// allowing it, request `hive.sales`, tag `pii` at `SELF`):
    ///
    /// | resource allow | deny items on it | 2.5.0 | 2.8.0 |
    /// |---|---|---|---|
    /// | priority 1 | none | **Denied**, the TAG policy's id | **Allowed**, the resource policy's id |
    /// | priority 1 | present (matching nobody) | Denied, the resource policy's id | Denied, the resource policy's id |
    /// | priority 2 | none | **Denied**, the TAG policy's id | **Allowed**, the resource policy's id |
    /// | priority 0 | none | Denied, the TAG policy's id | Denied, the TAG policy's id |
    /// | (no tag block) | none | Allowed | Allowed |
    ///
    /// So on 2.8.0 a strictly higher-priority resource allow reopens a tag deny —
    /// which is Ranger's own log line at offset 224, *"access denied by a tag
    /// policy. Higher priority resource policies will be evaluated to check for
    /// allow"* — and **on 2.5.0 it does not**. The equal-priority row and the
    /// deny-capable row agree across versions, which is what makes this a
    /// specific divergence rather than a general one.
    ///
    /// The mechanism was NOT established. The candidate is
    /// `RangerDefaultPolicyEvaluator.evaluatePolicyItems`' per-access-type
    /// `accessTypeResults` map (`RangerAccessRequestUtil.getAccessTypeResults`),
    /// which lives in the request context and therefore spans both phases; its
    /// branch structure at 2.8.0 offsets 366-573 is a five-armed priority
    /// arbitration whose operand order could not be read reliably from a
    /// disassembly. **A rule fitted to five rows without its mechanism is a
    /// guess**, and in the allow/deny path a guess is a security defect, so this
    /// refuses instead — but only where the crate is measured WRONG:
    ///
    /// * `Some(Ranger2_8_0)` — the crate's model is the measured 2.8.0 answer, so
    ///   nothing refuses;
    /// * `Some(Ranger2_5_0)` — the crate would reopen a deny 2.5.0 keeps, so it
    ///   refuses;
    /// * `None` — the caller did not say which plugin is running, and the two
    ///   answers differ, so it refuses. This is the one place in this crate where
    ///   `plugin_version: None` costs an ACCESS answer, and it costs it because
    ///   the versions genuinely disagree.
    ///
    /// Modelling 2.5.0's arm is named as follow-up work in
    /// `tests/ranger/README.md`, with the table above to model it against.
    TagDenyReopenedDiffersByPluginVersion {
        /// What [`EngineConfig::plugin_version`] said, verbatim.
        ///
        /// ⚠ FIX ROUND 1, IMPORTANT 4. The cause carries this because the two
        /// states it can be in need DIFFERENT advice, and the wire shape without
        /// it forced one sentence to cover both. `Some(Ranger2_5_0)` means the
        /// running plugin is KNOWN and the refusal is that 2.5.0's arm is
        /// unmodelled — there is nothing for an operator to record. `None` means
        /// the caller did not say which plugin is running, and recording it WOULD
        /// make half of these answerable. The frontend's sentence said the second
        /// thing in both cases, which is unactionable advice in the only case
        /// `explain` can now produce (it always declares the version).
        ///
        /// `Some(Ranger2_8_0)` is unrepresentable here by construction: that is
        /// the version this crate reproduces, so the refusal is not raised for it.
        plugin_version: Option<PluginVersion>,
        /// The tag policy that denied.
        tag_policy_id: Option<i64>,
        tag_policy_name: String,
        /// The strictly higher-priority resource policy that allowed.
        resource_policy_id: Option<i64>,
        resource_policy_name: String,
    },
    /// The TAG phase produced a mask or row filter and a strictly
    /// higher-priority RESOURCE policy then produced one too, so Ranger
    /// reports the tag policy's id with the resource policy's data.
    ///
    /// The cross-phase twin of [`UndecidableCause::TagSubPolicyIsDenyCapable`],
    /// and the same mechanism seen from the other side.
    /// `RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult` writes
    /// `maskType` / `maskedValue` / `maskCondition` **unconditionally** (offsets
    /// 11-88) and only then calls
    /// `RangerPolicyEvaluator.updateAccessResult(result, matchType, true,
    /// comments)`, whose allow branch is guarded by
    /// `if (!result.getIsAllowed())` — and the tag phase has already set
    /// `isAllowed` on the shared result. [`MaskResult`] and [`RowFilterResult`]
    /// bundle the identity with the data and cannot express the split.
    ///
    /// Raised for a resource policy that YIELDS and that Ranger actually
    /// REACHES. One that runs and matches nothing writes neither half, and one
    /// behind an evaluator that determined at offset 553 never runs at all — in
    /// both cases the tag verdict stands and the answer is not lost. See
    /// `a_higher_priority_resource_mask_that_yields_nothing_leaves_the_tag_mask`
    /// and `a_reachable_non_yielder_shuts_out_the_lower_priority_yielder_behind_it`.
    ///
    /// # ⚠ A deliberate OVER-REFUSAL, at a place where reproduction WAS available
    ///
    /// Named here rather than only in a report, because the whole-branch review
    /// has to be able to triage it. **This crate could emit Ranger's answer
    /// verbatim.** At the composition both halves are in hand, so
    /// `MaskResult { policy_id: <the tag policy>, mask_type: <the resource
    /// policy's> }` is constructible and is exactly what the engine returns.
    /// That is not a guess and the contract does not require refusing it.
    ///
    /// It refuses anyway, for two reasons and neither is "we could not work it
    /// out":
    ///
    /// * **Consistency.** [`UndecidableCause::TagSubPolicyIsDenyCapable`] is the
    ///   same split seen from inside one phase and Task 2 refused it. Answering
    ///   one and refusing the other would make the pair unreadable.
    /// * **A result names the policy that produced it.** Handing an operator a
    ///   policy id whose body does not contain the mask being shown sends them
    ///   to edit the wrong policy — a worse failure than a refusal that says so.
    ///
    /// The cost is bounded and stated: it bites only when a tag mask or row
    /// filter yields AND a resource policy Ranger reaches also yields.
    ///
    /// # ⚠ And the refusal was once drawn WIDER than the split
    ///
    /// FIX ROUND 1. The first version refused on any yielder above the tag's
    /// priority, which over-refused a second time and for a worse reason: it
    /// treated "an earlier evaluator stops the walk" as an unknowable order when
    /// BETWEEN priority bands it is decided, and sorted. The boundary now sits
    /// at Ranger's reachable set — offset 529's shut-out, then offset 553's
    /// first non-`hasDeny` evaluator, taking that evaluator's whole band because
    /// only WITHIN a band is the order `getEvalOrder()`. The walk that computes
    /// it is in `evaluate_sub` with the three rules written out.
    ///
    /// So: one over-refusal is deliberate and argued above; the other was a
    /// defect and is gone. A narrowing proposal should be measured against the
    /// first, not the second.
    TagSubResultSplitByResourcePolicy {
        tag_policy_id: Option<i64>,
        tag_policy_name: String,
        resource_policy_id: Option<i64>,
        resource_policy_name: String,
    },
    /// Whether Ranger's candidate filter returns this policy decides the
    /// request, and this crate cannot establish which way.
    ///
    /// Raised only where it changes the answer: at offset 553
    /// (`if (result.getIsAllowed() && !evaluator.hasDeny())
    /// setIsAccessDetermined(true)`), which is in the ENGINE loop and reads no
    /// matcher, so a policy `RangerPolicyRepository.getLikelyMatchPolicyEvaluators`
    /// never returns can end the evaluation here and not in Ranger. If Ranger's
    /// trie holds this policy the verdict is the one the result already carried;
    /// if not, the evaluators behind it still run. Those two verdicts differ, or
    /// this cause is not raised — see [`trie_excludes`] for what the crate CAN
    /// prove and the jar measurement behind it.
    ///
    /// # ⚠ The REACH of this refusal, by code path
    ///
    /// FIX ROUND 4. Two earlier statements of this bound were wrong, in the same
    /// way twice: both named a POPULATION ("nothing egret authored", "downloads
    /// egret did not author") instead of a code path, and the second's
    /// authorship clause is not enforced by anything. Phrased properly, what
    /// lands here is a policy matching `NONE` on every resource map that the
    /// crate also cannot prove absent — i.e. every request level it declares
    /// either matches or is one of the three shapes
    /// [`crate::resource::level_excluded_by_trie`] declines to reason from —
    /// positioned where offset 553 would fire and where the verdict behind it
    /// differs.
    ///
    /// The shapes, and what does or does not keep egret's own compiler out of each.
    /// **FIX ROUND 5 adds the fourth row and corrects the third**: the table said
    /// three, and the fourth — an EMPTY policy value, which
    /// [`crate::resource::trie_reproduces_matcher`] declines on explicitly — was a
    /// shape egret's own grant compiler could emit, which is the finding SP4d debt
    /// Task 12 closed.
    ///
    /// | shape | what the compiler does | enforced? |
    /// |---|---|---|
    /// | `isExcludes` | `egret-perm-compile`'s `grants::level()` hard-codes `is_excludes: false` on every level it builds, and it is the only constructor `scope_to_resources` uses | **yes**, by construction |
    /// | `isRecursive` | the same constructor hard-codes `is_recursive: false` | **yes**, by construction |
    /// | an EMPTY value (`""`) | `scope_to_resources` copies a scope level through verbatim, and `perm.grants` accepted `schema_name = ''` — so a `policy:update` caller could store one and the compiler emitted `{"values": [""]}`. **SP4d debt Task 12** closed it at three points keyed to one shape: `migrations/0178_perm_scope_levels_are_named.sql`'s CHECK, `grants::ensure_scope_levels_are_named` at the authoring boundary, and the same function inside `compile_grant` | **yes** since 0178 — and **NO** before it. The same empty value also made the LEVEL `isMatchAny`, so the grant was enforced catalog-wide; that half is the escalation, this row is only its trie consequence |
    /// | a non-prefix wildcard in a VALUE (`*xyz`, `or?ers`) | `scope_to_resources` copies `scope.schema` / `scope.table` / `scope.column` into the policy **verbatim**. `validate_grant_shape` refuses an EMPTY level (row above) and, since **SP4d debt Task 14**, a level that is a RUN OF `*` with a NAMED level beneath it (`grants::ensure_a_grant_wildcard_level_has_nothing_named_beneath_it`, `migrations/0179`) — because that is the level skip `grants_check` already forbade, not because `*` is illegitimate. Nothing else about the string is refused: `*` with nothing named under it stays legal (a catalog- or schema-wide grant is ordinary, and `scope_to_resources` emits `*` itself for every unnamed level), and `?` and a non-prefix `*` are untouched | **NO.** A grant scoped to a table named, or mistyped as, `ord*rs` compiles to exactly this shape, and so does one scoped to `('sales', 'or?ers')` |
    ///
    /// So the honest statement is: two of the three shapes cannot come from
    /// egret's compiler because one constructor forbids them; the third can, and
    /// the only thing standing between it and this refusal is that nobody has
    /// typed a `*` into a scope. Everything in a download egret did not author —
    /// the raw-JSON policy editor, an imported payload, a real Ranger install —
    /// can carry all three, and that is the population `explain` reads off a
    /// live plugin.
    ///
    /// **What would falsify this table:** `grants::level()` ceasing to pin the
    /// two flags, or a scope-content validator refusing `?` and a non-prefix `*`
    /// ANYWHERE in a value. ⚠ An earlier round wrote the second of those as "a
    /// scope-content validator arriving (which would move the third row to yes)",
    /// and that prediction was MEASURABLY wrong — twice over. Two validators have
    /// now arrived and NEITHER moved the wildcard row:
    /// `ensure_scope_levels_are_named` (Task 12) moved the EMPTY row, because it
    /// refuses `''` only; and
    /// `ensure_a_grant_wildcard_level_has_nothing_named_beneath_it` (Task 14) refuses a
    /// whole-value run of `*` only when a level BELOW it is named, which leaves
    /// every shape in this row — `*xyz`, `or?ers`, and a bare `*` at the deepest
    /// named level — emittable. A validator is not one thing, and naming the class
    /// rather than the predicate is the same imprecision this doc keeps
    /// correcting; the second prediction was wrong in the same way as the first,
    /// which is the reason to state the PREDICATE here and not a class. It is NOT `scope_to_resources`'s `Access` arm and its
    /// `*` fill — an earlier version of this cell said so, and that falsifier
    /// belonged to the key-count rule FIX ROUND 3 replaced: an egret ACCESS
    /// policy declaring fewer than four keys now matches at `ANCESTOR` and
    /// determines, so the fill decides nothing here any more.
    ResourceCandidacyNotReproducible {
        policy_id: Option<i64>,
        policy_name: String,
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
    /// One resolved tag ATTACHMENT carries something this crate cannot
    /// reproduce — see [`TagAttachmentProblem`] for the three shapes and
    /// [`tag_candidates`] for where each is raised and against which tags.
    ///
    /// The tag is named rather than a policy, because the construct is on the
    /// tag download, not on any policy: the same attachment changes every tag
    /// policy's candidacy at once.
    TagAttachmentNotReproducible {
        tag: String,
        problem: TagAttachmentProblem,
    },
    /// The tag phase ALLOWED, and a resource ACCESS policy at a STRICTLY higher
    /// `policyPriority` then allowed too — a shape whose arbitration runs through
    /// `accessTypeResults` and is NOT modelled here.
    ///
    /// # ⚠ CRITICAL, found by the whole-branch review of SP4d-2b
    ///
    /// This is the resource-path instance of the same `accessTypeResults`
    /// composite that [`UndecidableCause::TagPriorityArbitrationNotModelled`]
    /// refuses on the tag path — and that variant's doc **asserted the two
    /// coincide here**, on the strength of 4000 green corpus cases. They do not.
    /// The corpus could not reach the shape because its generator levels every
    /// random resource policy to `policyPriority: 0` (a deliberate, documented
    /// hold-out), so "green over 4000 cases" was green over a population the
    /// shape is absent from.
    ///
    /// MEASURED on both real engines, one download, byte-identical to both:
    ///
    /// ```text
    /// tag ACCESS policy 901  @0  ALLOW  select / bob   on tag `pii`
    /// resource policy p1 id 1 @1  ALLOW  select / bob   on c1.d1.t1.col1, isDenyAllElse
    /// resource policy p2 id 2 @0  DENY   select / bob   on c1.d1.t1.col1
    /// request: bob / select / c1.d1.t1.col1, tag `pii` at SELF
    /// ```
    ///
    /// | | 2.5.0 | 2.8.0 |
    /// |---|---|---|
    /// | as above | **Allowed**, `policyId` 1 | **Allowed**, `policyId` 1 |
    /// | control: the same, no `tagPolicies` block | Allowed, `policyId` 1 | Allowed, `policyId` 1 |
    ///
    /// Before this refusal this crate answered **`Denied`, policy 2** for the
    /// first row at both versions. Same answer at both engines, so it is not a
    /// version split; the tag ALLOW is the sole trigger, since the control agrees.
    ///
    /// # The mechanism, read at offsets in BOTH jars
    ///
    /// `RangerDefaultPolicyEvaluator.evaluatePolicyItems` does not write the
    /// shared result directly. It builds a **fresh** per-access-type
    /// `RangerAccessResult` that copies only the audit flags, so
    /// `updateAccessResult`'s allow branch — guarded by
    /// `if (!result.getIsAllowed())` — **always** passes on that fresh result and
    /// therefore always writes `setPolicyPriority` and `setPolicyId`.
    /// `getCompositeAccessResult` then calls `setAccessResultFrom`, which copies
    /// `policyPriority` **unconditionally** into the shared result the engine loop
    /// compares at offsets 469/505.
    ///
    /// So a resource ALLOW landing on a result the tag phase already allowed DOES
    /// raise `policyPriority`, and the next lower-priority evaluator is shut out
    /// at offset 505. This crate modelled the guard as written — "writes nothing"
    /// — and so let the denier run. **The direction is always over-DENY**, which
    /// is a confident wrong answer, not a safe one.
    ///
    /// # Why this REFUSES rather than models the composite
    ///
    /// Ruled by the coordinator, and the three reasons are recorded here because
    /// a later reader will want to narrow it:
    ///
    /// * over-deny is still a confident wrong answer, and this crate's whole
    ///   proposition is that it does not produce those, so refusing is the
    ///   correct behaviour and not a dodge;
    /// * the cost is near zero on egret-authored data — SP4d-2b Task 5 asserts
    ///   `policyPriority == 0` on every compiled grant, rule, clearance and tag
    ///   body, so only a HAND-AUTHORED override policy reaches the shape (it is
    ///   reachable by one supported API call: `AccessPolicyInput.isOverride` on
    ///   `POST /api/control-plane/policies` emits `policyPriority: 1`);
    /// * every previous attempt to model this area from a reading has been
    ///   corrected, twice against this implementer and once against the
    ///   coordinator, so a refusal that can be proved beats a model that would be
    ///   defended again next week.
    ///
    /// Modelling the composite is open item 0 in `tests/ranger/README.md`.
    ///
    /// # Why the two named policies are the tag policy and a resource policy
    ///
    /// The arm fires on `allowed && found.allowed && found.priority > priority`,
    /// and `allowed` can only have come from the TAG phase there. `allowed`
    /// becomes true inside the loop at one place only, which also sets
    /// `priority = found.priority`; [`sort_eval_order`] is `policyPriority`
    /// DESCENDING, so a later candidate's priority is `<= priority` and
    /// `found.priority > priority` is then impossible. Asserted in debug builds
    /// rather than left as prose.
    AccessPriorityCompositeNotModelled {
        /// The tag policy whose ALLOW the shared result was already carrying.
        tag_policy_id: Option<i64>,
        tag_policy_name: String,
        /// Its `policyPriority`, which is what offsets 469/505 compare.
        tag_priority: i32,
        /// The strictly higher-priority resource policy that also allowed, and
        /// whose priority Ranger composites onto the result where this crate does
        /// not.
        resource_policy_id: Option<i64>,
        resource_policy_name: String,
        resource_priority: i32,
    },
}

/// What a [`ResolvedTag`] carries that stops the tag phase being defensible.
///
/// All three are raised ONLY for a tag whose `match_type` the evaluator admits
/// and only when the block holds a policy of the kind being decided — a tag
/// Ranger's `evaluate` would not let reach the policy items writes nothing
/// whatever its payload said, so refusing for it would be pure over-refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagAttachmentProblem {
    /// `RangerTag.options` is non-empty; the keys, sorted.
    ///
    /// `RangerTagForEval.isApplicable` re-reads `TAG_VALIDITY_PERIODS` out of
    /// this map when it holds anything and the constructor built no evaluators
    /// (offsets 62-128), so an option key this crate does not recognise still
    /// sits one branch away from a filter that removes the tag. And the map is
    /// a component of `equals`/`hashCode`, so it cannot be dropped from the
    /// identity either.
    Options(Vec<String>),
    /// `RangerTag.validityPeriods` is non-empty; how many.
    ValidityPeriods(usize),
    /// The attachment's `RangerServiceResource.resourceElements` names fewer
    /// than three levels (SP4d-2b design, D2).
    ///
    /// # ⚠ WHY THIS IS INERT TODAY — and it is NOT the column definitions
    ///
    /// FIX ROUND 1, IMPORTANT 1. This doc said egret "cannot emit" the shape
    /// **because `catalog_tag_assignments` stores `schema_name` and `table_name`
    /// `NOT NULL`**. That inference does not hold, and the review falsified it by
    /// building the payload:
    ///
    /// * `migrations/0081_catalog_tag_targets.sql:9-18` has **no `CHECK`**, and
    ///   `NOT NULL` does not exclude `''` — the table's own `column_name` uses
    ///   `''` as a documented sentinel, so it demonstrably DOES carry an
    ///   empty-string convention;
    /// * `egret_bi::catalog_tags`' `assign_resource_tag` (`:409-425`) and
    ///   `assign_catalog` (`:69-103`) validate nothing: `&b.schema` and
    ///   `&b.table` go straight into the INSERT;
    /// * `ServiceTags::from_assignments` (`egret-policy-api/src/dto.rs:193-201`)
    ///   writes `catalog` unconditionally and each of `schema`/`table`/`column`
    ///   **only if non-empty**.
    ///
    /// Measured end to end on the writer: `from_assignments` given an assignment
    /// with an empty schema and table emits a `RangerServiceResource` whose
    /// `resourceElements` is `{catalog}` alone — a one-level resource a real
    /// plugin KEEPS (the subset test above). So `PUT …/resource-tags` with
    /// `{"schema": "", "table": ""}` is a shipping path to the shape D2 declares
    /// egret cannot author. **That is carried debt on the catalog-tag WRITE path,
    /// recorded in `task-4-report.md`, and deliberately not fixed here.**
    ///
    /// What actually makes this refusal inert is two facts about the READ path,
    /// both falsifiable:
    ///
    /// **⚠ SP4d-2b TASK 7 REPLACED BOTH OF THEM.** A producer of
    /// [`ResolvedTag`] now exists — `egret_perm_compile`'s `attachment_for`,
    /// mapping from the two-field `egret_policy::ResolvedTag` — so fact 1 is
    /// gone, and fact 2 (the query's exact `schema_name = $2 AND table_name =
    /// $3`) was never a bound: FIX ROUND 2 had already measured that
    /// `resolved_tags_for_resource_via(conn, id, "", "", "")` returns a `('',
    /// '')` row and that nothing in the query stops it. What makes the refusal
    /// inert today is ONE fact, on the caller, and it is falsifiable:
    ///
    /// 1. **An empty `schema_name` or `table_name` — the row that would produce a
    ///    one- or two-level attachment — can no longer be STORED.** ⚠ Updated by
    ///    SP4d debt Task 3: it used to be `resolved_tags_for_target`
    ///    (`egret-perm-compile/src/routes.rs`) running
    ///    `tag_assignment_coordinates_via` first and declining the whole resolution
    ///    on the gate's `EmptyLevel` class; migration 0177's
    ///    `catalog_tag_assignments_levels_are_present_and_literal` makes it
    ///    unrepresentable instead, so the mapping is never reached with one. (That
    ///    gate still runs, for the fold-divergence class a `CHECK` must not take.)
    ///    The bound exists for a bigger reason than this refusal: such a row is an
    ///    ANCESTOR of resources the resolver's predicate never returns it for,
    ///    i.e. a MISSED tag, which no inspection of the mapping's input could
    ///    detect. ⚠ Fix round 1 widened it from one class to three — a wildcard
    ///    value and a non-ASCII value are the same fail-open — so a claim about
    ///    this refusal's reach now rests on that function's enumeration.
    ///
    /// The mapping is nonetheless written to be right for it — it follows
    /// `from_assignments`' own skip-an-empty-level rule, so a `('', '')` row
    /// becomes the `{catalog}` attachment this refusal answers, rather than
    /// being padded out to four levels — and
    /// `routes.rs::an_empty_schema_and_table_map_to_the_one_level_attachment_the_evaluator_refuses`
    /// pins that, saying in its own doc that the input cannot arise from the
    /// live path. **Falsified by** removing the gate, or by a second producer
    /// (a `/tags/download` reader, an import, a shim) that has none.
    ///
    /// **The refusal is what protects the verdict, not the schema.** It is
    /// falsified by a producer that reads a `/tags/download` payload instead of
    /// that query — which is exactly what would make the write-path debt bite.
    ///
    /// **This is a refusal by PROVENANCE, not a Ranger rule**: Ranger validates
    /// the key set as a subset of some hierarchy, and `{catalog}` passes that.
    /// What it says is that the tag reaching this decision was resolved from a
    /// payload egret did not author, so the caller's resolution rules — which
    /// this crate takes on trust, see [`AccessRequest::resolved_tags`] — are
    /// not the ones that were measured.
    ResourceLevels(Vec<String>),
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
    /// `isRecursive` is set.
    ///
    /// FIX ROUND 1, CRITICAL 1 — the same widening as
    /// [`TagResourceProblem::ValueNotTriePrefixExact`], through a field the
    /// first version of this sweep did not read.
    ///
    /// MEASURED, and byte-identical in 2.5.0 and 2.8.0:
    /// `RangerResourceTrie.insert(node, resource, isRecursive, evaluator)` ends
    /// `if (isWildcard || isRecursive) addWildcardEvaluator(...) else
    /// addEvaluator(...)` (offsets 46-69), and the caller passes
    /// `resource.getIsRecursive()` (offsets 136 / 546 / 567). A *wildcard*
    /// evaluator at node `N` is returned for every request value with prefix
    /// `N` — so `isRecursive` turns an EXACT tag value into a prefix match in
    /// the candidate filter. On the other side, `RangerDefaultResourceMatcher`
    /// and `RangerAbstractResourceMatcher` contain **zero** references to
    /// `isRecursive` in either jar, so the matcher's answer does not move at
    /// all. Live on 2.5.0: a policy on `pii` with `isRecursive: true` is a
    /// candidate for the tags `piix` and `pii_x`, where `getMatchType` is
    /// `NONE`.
    ///
    /// **This is a deliberate OVER-refusal**, and the measurement says by how
    /// much: a value that is ALREADY a wildcard (`p*`) is already parked as a
    /// wildcard evaluator, so `isRecursive` adds nothing there and the two
    /// agree. The crate refuses the FLAG rather than reproducing the trie —
    /// see the ⚠ block on [`evaluate_tag_policies`] for why that trade was
    /// taken. `tests/ranger` measures both rows.
    Recursive,
}

/// What a tag policy carries that the TAG repository's `normalizeAndPrunePolicies`
/// would change or reject, and this crate does not reproduce.
///
/// See [`UndecidableCause::TagPolicyNotNormalisable`] for the disassembly and
/// for why each is refused rather than modelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagNormalisationProblem {
    /// An `accesses[].type`, in any of the six buckets, containing a `:`.
    ///
    /// Ranger strips a leading `<componentServiceDefName>:` and DROPS the access
    /// otherwise. Both arms are refused, not just the strip: this crate is
    /// handed the tag block's own def and not the component's name, so it cannot
    /// even tell the two arms apart.
    NamespacedAccessType(String),
    /// A `dataMaskPolicyItems[].dataMaskInfo.dataMaskType` containing a `:`.
    ///
    /// Same two arms, except that the DROP removes the whole ITEM rather than
    /// one access entry.
    NamespacedMaskType(String),
    /// A `dataMaskPolicyItems` item with **no mask type at all** — the key
    /// absent, or explicitly null.
    ///
    /// ⚠ **This one is not a divergence in the verdict; it is a CRASH.** The
    /// normalisation reads `dataMaskType.contains(":")` with no null guard
    /// (offset 293 of `normalizeAndPrunePolicyItems`), and Ranger's model class
    /// substitutes an empty `RangerPolicyItemDataMaskInfo` — whose
    /// `dataMaskType` is null — for an absent or null `dataMaskInfo` (finding
    /// F2). So a tag block carrying such an item makes
    /// `RangerPolicyRepository`'s constructor throw
    /// `NullPointerException`, which propagates out of
    /// `RangerPolicyEngineImpl.<init>`: the plugin cannot build a policy engine
    /// from that download at all, and **every** request fails, of every policy
    /// type.
    ///
    /// Measured on the real 2.5.0 and 2.8.0 engines by the differential gate:
    /// `java.lang.NullPointerException: Cannot invoke
    /// "String.contains(java.lang.CharSequence)" because "maskType" is null
    /// at RangerPolicyRepository.normalizeAndPrunePolicyItems`.
    ///
    /// Note the asymmetry with the RESOURCE path, and that it is the asymmetry
    /// of Ranger's own code rather than of this crate's: the same item in
    /// `policies` is perfectly legal there and DETERMINES the result with a null
    /// mask type, which is SP4d-1's pinned finding F2. Only the tag repository
    /// normalises, so only the tag repository crashes.
    ///
    /// Raised for every tag policy in the block regardless of `isEnabled` and
    /// regardless of the decision's policy type, because the constructor runs
    /// before `init()` and the throw is not scoped to either.
    MaskTypeAbsent,
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
///
/// # ⚠ Every input the cross-phase arbitration reads, and every input it could
/// have read
///
/// SP4d-2b Task 3. Arbitration is dense with this package's recurring defect
/// shape — *a condition that changes the answer, reaching a function that does
/// not look at it* — because it reads two verdicts, two priorities and a policy
/// type. So the list below is exhaustive by construction rather than by
/// inspection, on the pattern [`evaluate_tag_policies`] set. **A new field on
/// [`TagDecision`], [`PolicyView`] or [`RangerPolicy`] is a new row here.**
///
/// Two rows were findings: the LIVE `policyPriority` (table C), which is what
/// makes the eval-order refusal below necessary, and `isApplicable` (table B).
///
/// ## A — what phase 1 left, [`TagDecision`]
///
/// | field / variant | read | ruling |
/// |---|---|---|
/// | `NotDetermined` | **yes** | both flags false, and the loop is then the pre-Task-3 resource loop exactly — which is why every existing resource test still holds |
/// | `Allowed { priority }` | **yes** | seeds `isAllowed` and `policyPriority`; selects offset 505's `>` arm |
/// | `Denied { priority }` | **yes** | seeds both; selects offset 469's `>=` arm, the tighter one |
/// | `Allowed`/`Denied` `{ policy_id, policy_name }` | **yes, for REPORTING only** | they are `RangerAccessResult.policyId`; no comparison reads them |
/// | `Denied { reason }` | reporting only | Ranger's `reason` is free text and is never compared |
/// | `Undecidable { why }` | **yes → refuses** | merged into the composed `why` by [`merge_causes`]. The engine has no such state, so it cannot be an arbitration input at all: a verdict this crate could not compute cannot be arbitrated with |
///
/// ## B — each resource candidate, [`PolicyView`] and its [`RangerPolicy`]
///
/// | field | read | ruling |
/// |---|---|---|
/// | `policy.policy_priority` | **yes** | the right-hand side of all three comparisons |
/// | `has_deny` | **yes** | offset 553, the eval-order band, and the refusal below |
/// | the six item buckets, `is_deny_all_else` | through `decide` | unchanged by this task |
/// | `policy.id`, `policy.name` | reporting only | |
/// | `policy.policy_type`, `is_enabled` | **yes, upstream** | the candidate filter. `RangerPolicyRepository.init` keeps the three evaluator lists disjoint, so a mask policy can never arbitrate against an ACCESS tag verdict — the two arms live in two functions rather than behind a flag |
/// | `unmodelled` | **yes → refuses** | `collect_causes`, before the loop |
/// | `isApplicable` / `validitySchedules` | **not read, and subsumed** | ⚠ **FIX ROUND 2 rewrote this row's reasoning.** Offset 437 `continue`s a policy outside its validity window before the arbitration, and this crate has no clock. Round 1 argued the skip was outcome-free because "an unmatched policy decides nothing"; offset 553 makes that false — an inapplicable unmatched evaluator latches here and is skipped in Ranger. The behaviour is nonetheless sound, by a DIFFERENT argument: a MATCHED policy carrying `validitySchedules` already refuses (`construct_affects` ⇒ true), and an UNMATCHED one now goes through the candidate-set rule below, whose two branches are exactly "Ranger ran it" and "Ranger did not" — which is what an inapplicable policy is. The wrong argument is recorded rather than deleted because it is the shape this round exists to catch |
///
/// ## C — the shared `RangerAccessResult`, which IS the loop's state
///
/// | field | read | ruling |
/// |---|---|---|
/// | `isAccessDetermined` | **yes** | cleared at 391, set by the arbitration and by 553, restored at 599 |
/// | `isAllowed` | **yes, and LIVE** | offset 481 re-reads it every iteration, so a resource allow changes which arm LATER evaluators take |
/// | `policyPriority` | **yes, and LIVE** | ⚠ **a finding.** `updateAccessResult` moves it, so under a tag deny an allowing resource policy raises the left-hand side of offset 469's `>=` and shuts out the rest of its own band. Modelling it as `tag.priority` — the obvious reading of "the tag verdict's priority" — would have been wrong, and it is a mutable local for that reason. FIX ROUND 1: an earlier version of this row called that mistake fail-OPEN, and the review measured it the other way on this task's own fixture. A fixed tag priority makes `priority >= view.priority` true LESS often, so fewer evaluators are shut out and more can deny: `Denied { b-deny }`, i.e. systematically fail-CLOSED. The `Allowed { a-allow }` belongs to the live priority with no refusal, which is what [`UndecidableCause::EvalOrderAmbiguous`] closes. Both are wrong answers; neither is a reason to relax the refusal |
/// | `isAuditedDetermined` | **no** | offsets 577-596 break only when audit is ALSO determined. The extra iterations cannot change access: `evaluatePolicyItems` is guarded by `!getIsAccessDetermined()` (`RangerDefaultPolicyEvaluator` 380-384) and nothing clears the flag again, so `break`-on-determined is exact |
/// | `getIsFallbackSupported()` | **no** | offsets 640-650 set `isAccessDetermined` for a deny and change neither `isAllowed` nor `policyId`. It is a signal to the HOST about a native fallback authorizer, which [`DenyReason::NoPolicyMatched`] already carries |
/// | `maskType` / `maskedValue` / `filterExpr` | not on this path | `evaluate_sub` owns them, and owns the id/data split they create |
///
/// ## D — which arm, and what chooses it
///
/// | input | read | ruling |
/// |---|---|---|
/// | `policyType` (offset 489) | **yes** | `0` ⇒ `>`; `1`/`2` ⇒ `>=`. Split across this function and `evaluate_sub` so each has exactly ONE arm — a wrong argument cannot select the wrong comparison |
/// | `hasResourcePolicies` (offset 337) | **no** | its `false` arm skips clear, loop and restore in one jump (350 → 715) and is measured indistinguishable from its `true` arm with an empty evaluator list. `tests/tag_arbitration.rs` pins the equivalence |
/// | the candidate ORDER | **yes** | `sort_eval_order`'s `getEvalOrder()` stand-in. Order-free for the outcome on every arm except the tag-deny one — see the ⚠ block in the loop |
/// | the candidate SET | **yes, at offset 553 and NOWHERE ELSE** | ⚠ **FIX ROUND 2, and the finding of this round.** Ranger's list is its resource trie's answer; this crate's is every enabled policy of the type. That gap is invisible to the arbitration — it only ever SETS `determined`, and on a priority-DESC list a firing at one candidate fires at every later one, so an extra candidate moves nothing — and invisible to `decide`, which re-asks the matcher. It is visible at 553, which reads `hasDeny()` alone. [`trie_excludes`] proves the ordinary case absent; what it cannot prove is ruled on after the loop |
///
/// ## The REACH of each refusal this function owns
///
/// FIX ROUND 3. Three times in this task a claim about *reach* — who lands in a
/// refusal, and how often — has been wrong while the claim about *behaviour* was
/// right. Behaviour is measured against the jar and gets caught; reach is
/// asserted in prose and does not. So each refusal below names its population,
/// the mechanism that decides it, and what would falsify it.
///
/// | refusal | population | by what mechanism | what would falsify it |
/// |---|---|---|---|
/// | [`UndecidableCause::EvalOrderAmbiguous`] (from this loop) | a tag DENY plus a `hasDeny` band above its priority holding both an allower and a denier | offset 469's `>=` reading a `policyPriority` an allowing band member just raised | a download where deny-capable resource policies share a priority above a clearance tag policy's — no corpus generates a tag block, so the rate is UNMEASURED |
/// | [`UndecidableCause::ResourceCandidacyNotReproducible`] | a policy matching `NONE` everywhere whose every declared request level is `isExcludes`, `isRecursive` or a non-prefix wildcard, positioned where 553 fires | the three shapes [`crate::resource::level_excluded_by_trie`] declines to reason from — the ⚠ table on that variant says which of them `grants::level()` forbids and which (the wildcard) **nothing** forbids | `grants::level()` ceasing to pin `is_excludes`/`is_recursive` false, or a scope-content validator arriving |
/// | [`UndecidableCause::TagSubResultSplitByResourcePolicy`] (from `evaluate_sub`) | a tag mask/filter that yields plus a resource policy Ranger reaches that also yields | offsets 11-88 writing the DATA unconditionally and the identity only `if (!isAllowed)` | nothing today; the shape needs a tag block, which no corpus generates |
/// | [`UndecidableCause::TagAttachmentNotReproducible`] (raised in [`tag_candidates`], on a set THIS function enriched) | a tag ATTACHMENT carrying `options`, `validityPeriods`, or a service resource naming fewer than three levels — and only for a tag whose match type the evaluator admits, in a download whose tag block holds a policy of the kind being decided | ⚠ **SP4d-2b TASK 7: a producer now exists and this row is no longer "unreachable at all".** `egret_perm_compile`'s `attachment_for` maps the two-field `egret_policy::ResolvedTag` into this type for every `explain` on a target naming a schema and a table. Which of the three problems can arrive is now a per-problem question, and the answer is on each variant: `Options` and `ValidityPeriods` cannot (`ServiceTags::from_assignments` writes a wire tag as `{"type", "owner"}` and nothing else, so the mapping synthesises them empty); `ResourceLevels` cannot either, and ⚠ **SP4d debt Task 3 changed WHY**: it used to be a CALLER reason — `resolved_tags_for_target` runs `tag_assignment_coordinates_via` first and declined the resolution for the whole catalog on the gate's `EmptyLevel` class — and it is now a SCHEMA reason, migration 0177's `catalog_tag_assignments_levels_are_present_and_literal`, which forbids an empty `schema_name`/`table_name` outright. FIX ROUND 1 corrected two earlier versions of this row: it named `resolved_tags_for_resource_via` as a producer of THIS type (it produces the other one), and it credited `catalog_tag_assignments`' `NOT NULL` with a guarantee it does not give — 0177 is the constraint that finally gives it | the gate going away, a second producer (a `/tags/download` reader, an import, a shim), or `from_assignments` learning to write `attributes`/`options`/`validityPeriods`. **What exercises it is still only this crate's own tests plus `routes.rs`' mapping unit tests** — no corpus generates a tag block, so the RATE is unmeasured |
/// | the other tag-phase refusals | see the ⚠ tables on [`evaluate_tag_policies`] | — | — |
///
/// **None of these rates is measured on a corpus**, and that is the honest state
/// rather than an omission being hidden: the differential corpus generates no
/// `tagPolicies` block, so on it `allowed` is never seeded before the resource
/// loop and the first two are unreachable by construction. Task 5 is where they
/// become measurable.
///
/// ## Not an input at all
///
/// * **[`EngineConfig`]** — the super-user short-circuit is at caller offsets
///   114-165, before `evaluateTagPolicies` at 176 and before everything here.
///   It is [`request_preamble`]'s, and running it exactly once, first, is what
///   makes both phases safe.
/// * **The security zone** — `evaluateTagPolicies` has its own zone gate; the
///   resource loop has none. Refused upstream through
///   `Unmodelled::SecurityZones`.
/// * **[`AccessRequest::resolved_tags`]** — not an ARBITRATION input: it sees a
///   verdict, never the tag set. ⚠ **SP4d-2b Task 4 falsified the stronger
///   claim this row used to make** ("read by phase 1 only"): this function now
///   reads the tag set itself, through [`enriched_tags`], because
///   `RangerTagEnricher.processServiceTags` validates each attachment's key set
///   against the RESOURCE service-def's hierarchies — a def phase 1 is not
///   handed. The premise was true when written and was falsified by a mechanism
///   added beside it, with nothing to signal it; the two halves are kept apart
///   here so the surviving half is not read as covering both.
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

    // ── phase 1: the tag policies (caller offset 176) ──────────────────
    //
    // After the super-user block at 114-165 and before everything below.
    // `tag_phase_input` is the only gate; a `None` from it is Ranger's own
    // "no evaluator can run", not a narrowing this crate invented.
    //
    // `enriched` first, and the phase reads only that: Ranger's own order is
    // `RangerDefaultRequestProcessor.preProcess` → the enricher writes the tag
    // set into the request context → `evaluateTagPolicies` reads it back out
    // (`RangerAccessRequestUtil.getRequestTagsFromContext`, offset 104).
    //
    // ⚠ SP4d-2b Task 7 made that order a TYPE. This used to clone `request`
    // with the enriched set into a second `AccessRequest` and hand the phase
    // whichever of the two bindings was right; the phase now takes
    // `Option<&EnrichedTags>`, which only `enriched_tags` can produce, so the
    // enrichment cannot be skipped and the RESOURCE phase below cannot
    // accidentally be handed an enriched request. See `UnenrichedTags`.
    let enriched = enriched_tags(request, service_def);
    let tag = match tag_phase_input(policies, enriched.as_ref(), service_def) {
        None => TagDecision::NotDetermined,
        Some(tag_service_def) => {
            evaluate_tag_policies(policies, request, enriched.as_ref(), &tag_service_def)
        }
    };
    if let TagDecision::Undecidable { why: tag_why } = &tag {
        merge_causes(&mut why, tag_why);
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
    // Read out of `ranger-plugins-common-2.5.0` (Trino 470's jar) and not out of
    // StarRocks's 2.8.0 — see [`sort_eval_order`]'s "WHICH JAR" note, which also
    // names the artifact the two plugin versions are measured to disagree about.
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
    // That reported-id difference is REAL and is MEASURED AND ASSERTED, not waved
    // away. ⚠ This comment used to end "nothing in this crate asserts a reported
    // policy id for an ACCESS decision, which is why the gate is green in spite of
    // it", and quoted `39 of 1972`. Both are superseded by SP4d-2b Task 5 (fix
    // round 1, RULING 3): the differential gate compares the reported ACCESS
    // `policyId` on its own axis, **1465 of 1602 agree (91.45%)** at the recorded
    // seed, and the 137 that do not are a CLOSED allow-list of two classes — this
    // one (47 cases) and the restore below (90) — each tested structurally on the
    // two policies the two sides named. Anything else fails the run with a
    // reproducer. `tests/ranger/README.md` carries the table, and it is the only
    // place in the tree that keeps a recorded-run count (whole-branch review: the
    // README carried two conflicting sets and this file a third).
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
    // The loop's state IS `RangerAccessResult`, seeded from what phase 1 left
    // on it. `createAccessResult` starts `isAllowed` false and
    // `policyPriority` 0, and the tag phase then writes a verdict or does not
    // — which is exactly what [`TagDecision`]'s three non-refusal arms say.
    let mut outcome: Option<Outcome> = tag_verdict_as_outcome(&tag);
    let tag_verdict = outcome.clone();
    let mut allowed = outcome.as_ref().is_some_and(|o| o.allowed);
    let mut priority = outcome.as_ref().map_or(0, |o| o.priority);

    // Offsets 291-335, both computed from the result BEFORE the clear, and
    // both constant for the whole loop.
    let is_allowed_by_tags = outcome.is_some() && allowed;
    let is_denied_by_tags = outcome.is_some() && !allowed;

    // Offset 391, `result.setIsAccessDetermined(false)` — the CLEAR, which is
    // what lets a resource policy decide at all after the tag phase determined.
    let mut determined = false;

    // Where Ranger MIGHT have stopped at offset 553 on an evaluator this crate
    // cannot place in its candidate list, and the verdict it carried there.
    // Resolved after the loop, because whether it matters depends on what the
    // rest of the loop does.
    let mut unknown_stop: Option<(Option<i64>, String, bool)> = None;

    for view in &candidates {
        // ── the candidate filter, BEFORE the arbitration ───────────────
        //
        // ⚠ SP4d-2b Task 5, found by the differential gate. Ranger's loop
        // iterates `getLikelyMatchPolicyEvaluators` — its resource trie's answer
        // — and this crate's `candidates` is every enabled policy of the type.
        // That gap was argued harmless because an extra candidate cannot
        // `decide` (trie ⊇ matcher, measured) and because offset 553 was guarded
        // separately. It is not harmless: **the three-way arbitration below sets
        // `determined` from the PRIORITY alone and never asks the matcher**, so a
        // policy Ranger's trie removed could end this crate's loop, skip the
        // restore at 599-632, and change the verdict.
        //
        // MEASURED, minimal reproducer (ranger 2.8.0): a tag DENY at priority 0,
        // one resource policy at priority 1 that ALLOWS and is deny-capable, and
        // one resource policy at priority 0 whose `database` value the trie
        // provably does not hold for this request. Ranger answers
        // `Denied, policyId 2` — the allowing policy wrote its id and the restore
        // then cleared `isAllowed` — with or without that third policy, because
        // its evaluator is never built into the loop's list. This crate answered
        // `Allowed` when it was present and `Denied` when it was not: the whole
        // divergence was a policy neither engine ever evaluates.
        //
        // The skip is a pure NARROWING of this crate's list towards Ranger's:
        // `trie_excludes` proves absence per level, and a policy it excludes
        // cannot match either, so it could never have decided anything. What it
        // could do — and now cannot — is determine.
        //
        // `evaluate_sub`'s walk needs no such guard and has none: it ends its walk
        // only at a view that is `provably_a_candidate()`, which is exactly the
        // complement of this test, and a view that decides nothing contributes
        // nothing to its split.
        if trie_excludes(
            view.policy,
            &request.resource,
            service_def,
            PolicyType::Access,
        ) {
            continue;
        }

        // ── the three-way arbitration, offsets 452-535 ─────────────────
        //
        // One character separates the two comparisons, and they are the two
        // halves of Ranger's own log lines at offsets 224 and 264: a tag DENY
        // lets only a STRICTLY higher-priority resource policy through, a tag
        // ALLOW lets a SAME-or-higher one through so it can still deny. The
        // third arm — `policyType != 0`, offset 529 — belongs to
        // `evaluate_sub`, which is the only place a mask request reaches.
        //
        // `setIsAccessDetermined(true)` here does not skip the evaluator: the
        // engine still calls `evaluate`, and `RangerDefaultPolicyEvaluator`
        // guards `evaluatePolicyItems` with `!getIsAccessDetermined()` at
        // offsets 380-384. Nothing clears the flag again, so a `break` is
        // exact for the access outcome.
        if is_denied_by_tags {
            //     469: if_icmplt 538 — determined unless the evaluator is
            //          STRICTLY higher priority.
            if priority >= view.policy.policy_priority {
                determined = true;
            }
        } else if allowed {
            //     505: if_icmple 538 — determined only when the result is
            //          STRICTLY higher priority, so an EQUAL-priority
            //          evaluator still runs and can deny.
            if priority > view.policy.policy_priority {
                determined = true;
            }
        }
        if determined {
            break;
        }

        if let Some(found) = view.decide(request, &implied) {
            // ⚠ THE ONE PLACE `getEvalOrder()` DECIDES AN ACCESS OUTCOME, and it
            // exists only under a tag DENY.
            //
            // Once an allowing member of this band has run, `priority` moves to
            // the band's and offset 469's `>=` shuts out every remaining
            // evaluator at that priority — including the rest of the band. So in
            // a `hasDeny` band above the tag deny's priority, whichever of an
            // allowing and a denying member runs FIRST wins, and which that is
            // comes from `RangerOptimizedPolicyEvaluator.computeEvalOrder()`, the
            // resource-specificity score this crate does not compute.
            //
            // Neither of the other two arms can reach this shape, which is why
            // the guard is on `is_denied_by_tags` rather than on the band:
            //
            // * with no tag verdict and with a tag ALLOW, offset 505's `>` lets
            //   the same-priority rival run after the allow, so the deny still
            //   wins whichever ran first;
            // * with a tag ALLOW the allow branch of `updateAccessResult` does
            //   not fire at all (`isAllowed` is already set), so `priority` never
            //   moves and nothing is shut out.
            //
            // And the test is `has_deny` rather than the band as a whole because
            // `sort_eval_order` puts every `hasDeny` member of a priority ahead
            // of every non-`hasDeny` one, and nothing in the non-`hasDeny` half
            // can deny at all (`has_deny` is `isDenyAllElse || !denyPolicyItems`),
            // so that half holds no rival to be ambiguous with.
            //
            // ⚠ FIX (SP4d-2b Task 5): this check used to sit in the ALLOW arm
            // ALONE, with a comment arguing that *"a rival earlier in this
            // crate's order would already have denied and broken the loop, so
            // scanning the whole band is the same scan as the rest of the
            // band — and it does not depend on this crate's order being
            // Ranger's, which is the point."*
            //
            // The scan did not depend on this crate's order. **Reaching the scan
            // at all did.** When this crate's name tie-break happened to put the
            // DENIER first, the deny arm below set `determined` and broke the
            // loop, the allow arm was never reached, and the crate answered
            // `Denied` with full confidence over a band whose winner Ranger picks
            // with a score this crate does not compute. The differential gate
            // measured it the first time the corpus generated a composed duel:
            // 6 of 387 compared cases at ranger 2.8.0, every one of them
            // `egret said Denied / ranger said Allowed` — so exactly half the
            // shape was being refused, and the half that was not was chosen by a
            // tie-break, not by a rule.
            //
            // The ambiguity is a property of the BAND — it holds a member that
            // allows and a member that denies — so the check belongs where the
            // band is observed, before either arm acts on one member's verdict.
            if is_denied_by_tags && view.has_deny {
                let band = (view.policy.policy_priority, view.has_deny);
                let rival = candidates
                    .iter()
                    .filter(|other| !std::ptr::eq(other.policy, view.policy))
                    .filter(|other| (other.policy.policy_priority, other.has_deny) == band)
                    .find(|other| {
                        // The OPPOSITE verdict to this member's: two allows
                        // cannot disagree about the outcome (whichever runs
                        // first, the band is shut out at an allow) and two denies
                        // cannot either.
                        other
                            .decide(request, &implied)
                            .is_some_and(|o| o.allowed != found.allowed)
                    });
                if let Some(rival) = rival {
                    return Decision::Undecidable {
                        why: vec![UndecidableCause::EvalOrderAmbiguous {
                            policy_ids: vec![view.policy.id, rival.policy.id],
                            policy_names: vec![view.policy.name.clone(), rival.policy.name.clone()],
                        }],
                    };
                }
            }
            // `RangerDefaultPolicyEvaluator.updateAccessResult`: the deny
            // branch (offsets 83-132) writes id, priority and reason
            // unconditionally; the allow branch (135-184) writes only
            // `if (!result.getIsAllowed())`.
            //
            // ⚠ CRITICAL, whole-branch review of SP4d-2b. This comment used to
            // continue *"So a resource ALLOW landing on a result the tag phase
            // already allowed writes NOTHING — the tag policy stays the reported
            // one, and `priority` does not move."* **That is false, and it was a
            // confident over-DENY.** The guard is read against a FRESH
            // per-access-type result out of `accessTypeResults`, not against the
            // shared one, so it always passes and always writes; then
            // `getCompositeAccessResult` → `setAccessResultFrom` copies
            // `policyPriority` unconditionally onto the shared result. The next
            // lower-priority evaluator is therefore shut out at offset 505 —
            // where this crate let it run and deny.
            //
            // The measurement, the mechanism at offsets in both jars, and the
            // ruling to refuse rather than model are on
            // [`UndecidableCause::AccessPriorityCompositeNotModelled`].
            if !found.allowed {
                allowed = false;
                priority = found.priority;
                outcome = Some(found);
                determined = true;
            } else if !allowed {
                allowed = true;
                priority = found.priority;
                outcome = Some(found);
            } else if found.priority > priority {
                // `allowed` can only be the TAG phase's here — see the variant's
                // doc for the proof from [`sort_eval_order`]'s descending order.
                debug_assert!(
                    is_allowed_by_tags,
                    "the composite refusal fired without a tag allow: \
                     sort_eval_order is no longer priority-descending"
                );
                let (tag_policy_id, tag_policy_name) = tag_verdict
                    .as_ref()
                    .map(|o| (o.policy_id, o.policy_name.clone()))
                    .unwrap_or((None, String::new()));
                return Decision::Undecidable {
                    why: vec![UndecidableCause::AccessPriorityCompositeNotModelled {
                        tag_policy_id,
                        tag_policy_name,
                        tag_priority: priority,
                        resource_policy_id: found.policy_id,
                        resource_policy_name: found.policy_name.clone(),
                        resource_priority: found.priority,
                    }],
                };
            }
        }

        // Offset 553: once allowed, the first evaluator with no deny items of
        // its own determines. (`setIsAllowed(false)` sets the flag itself, so
        // the deny above is already `determined`.)
        //
        // ⚠ FIX ROUND 2, CRITICAL. 553 reads `hasDeny()` and NOT the matcher, so
        // it fires for an evaluator that matched nothing — which makes Ranger's
        // candidate filter outcome-relevant here and nowhere else in this loop.
        // `matched` is the crate's matcher, and trie ⊇ matcher (measured), so a
        // matched view is in Ranger's list and may determine. An unmatched view
        // is in it only if the trie says so; [`trie_excludes`] proves the
        // ordinary "some other table's policy" absent, and anything it cannot
        // prove is recorded here and ruled on after the loop.
        if allowed && !view.has_deny {
            if view.provably_a_candidate() {
                determined = true;
            } else if !trie_excludes(
                view.policy,
                &request.resource,
                service_def,
                PolicyType::Access,
            ) && unknown_stop.is_none()
            {
                unknown_stop = Some((view.policy.id, view.policy.name.clone(), allowed));
            }
        }
        if determined {
            break;
        }
    }

    // ── the restore, offsets 599-632 ───────────────────────────────────
    //
    // Nothing determined, so the tag verdict goes back. The allow arm is a
    // no-op by construction and is written out anyway, because "provably
    // idempotent" is a claim a later change can quietly break: after a tag
    // allow, `allowed` can only become false through a resource deny, and a
    // deny sets `determined`.
    //
    // ⚠ The deny arm is where this crate reports a DIFFERENT policy from
    // Ranger, deliberately. `setIsAllowed(false)` does not touch `policyId`,
    // so Ranger names whatever wrote last — which in the one reachable shape
    // is a resource policy that ALLOWED and was then discarded. Reporting that
    // policy as the source of a deny would send a reader to a policy that
    // allows. This crate names the tag policy that actually denied, which is
    // an instance of the ACCESS reported-id divergence already declared above
    // and in `tests/ranger/README.md` (nothing in this crate asserts a
    // reported policy id for an ACCESS decision).
    //
    // Both arms leave `outcome.allowed == allowed`, so no fix-up follows: the
    // loop's invariant is that `allowed` is `outcome`'s (a deny writes both, an
    // allow writes both when it writes at all), and the deny arm restores an
    // outcome that is itself a deny.
    if !determined {
        if is_denied_by_tags {
            // `setIsAllowed(false)`, and with it the identity to report. The
            // one shape that reaches here with a resource verdict in hand is a
            // strictly higher-priority DENY-CAPABLE resource policy that
            // allowed: offset 553 cannot determine for it, so the loop runs
            // out and the tag deny is restored over its allow.
            outcome = tag_verdict.clone();
        } else if is_allowed_by_tags {
            // `setIsAllowed(true)`, a no-op BY CONSTRUCTION rather than by
            // accident: after a tag allow the only thing that can clear
            // `allowed` is a resource deny, and a deny sets `determined`.
            // Asserted rather than assumed, because "provably idempotent" is
            // the kind of claim a later change breaks quietly.
            debug_assert!(allowed && outcome.as_ref().is_some_and(|o| o.allowed));
        }
    }

    // ⚠ THE VERSION SPLIT, raised on the verdict this function would RETURN
    // (SP4d-2b Task 5, widened and moved in fix round 1).
    //
    // MEASURED on both real engines, four shapes, one minimal download (tag ACCESS
    // policy id 900 denying `select` to bob at priority 0; request `hive.sales`;
    // tag `pii` at SELF):
    //
    // | resource policies above the tag deny | 2.5.0 | 2.8.0 |
    // |---|---|---|
    // | one deny-capable allower @1 | Denied, id 900 | Denied, id 1 |
    // | TWO deny-capable allowers @5 | Denied, id 900 | **Allowed**, id 1 |
    // | deny-capable allower @1 + a matching no-deny policy @0 | Denied, id 900 | **Allowed**, id 1 |
    // | no-deny allower @1 + the same low policy | Denied, id 900 | **Allowed**, id 1 |
    //
    // **On 2.5.0 a tag DENY is never reopened.** On 2.8.0 it is reopened whenever
    // anything sets `isAccessDetermined` before the restore at 599-632 can run —
    // which is 553 for a non-deny-capable allower, and offset 469 for any later
    // lower-priority evaluator once an allow has raised `policyPriority`. This
    // crate reproduces 2.8.0 on all four rows.
    //
    // So the condition is exactly "this function is about to return an ALLOW while
    // the tag phase denied", and not a property of any single view:
    //
    // * ⚠ FIX ROUND 1 widened it. The first version required
    //   `!view.has_deny` on the allowing policy, on the strength of the first row —
    //   where both versions answer Denied. That row agrees for a DIFFERENT reason
    //   (the restore fires because nothing determined), and rows 2-4 all carry deny
    //   items or reach the allow some other way. The corpus found row 3 as a real
    //   `egret Allowed / ranger Denied` disagreement at 2.5.0.
    // * ⚠ FIX ROUND 1 also MOVED it here from the allowing view, which was
    //   MINOR 7: raised eagerly it refused shapes that the restore would have
    //   brought back to a Denied both versions agree on. Row 1 is exactly that
    //   shape, and it now answers instead of refusing.
    if is_denied_by_tags
        && outcome.as_ref().is_some_and(|o| o.allowed)
        && engine.plugin_version != Some(PluginVersion::Ranger2_8_0)
    {
        let (tag_policy_id, tag_policy_name) = tag_verdict
            .as_ref()
            .map(|o| (o.policy_id, o.policy_name.clone()))
            .unwrap_or((None, String::new()));
        let (resource_policy_id, resource_policy_name) = outcome
            .as_ref()
            .map(|o| (o.policy_id, o.policy_name.clone()))
            .unwrap_or((None, String::new()));
        return Decision::Undecidable {
            why: vec![UndecidableCause::TagDenyReopenedDiffersByPluginVersion {
                plugin_version: engine.plugin_version,
                tag_policy_id,
                tag_policy_name,
                resource_policy_id,
                resource_policy_name,
            }],
        };
    }

    // ⚠ The candidate-set ruling, after the restore because the restore is part
    // of the verdict being compared.
    //
    // Ranger either held that evaluator — in which case it stopped there and the
    // verdict is the one the result carried at that point — or it did not, in
    // which case everything after it ran and the verdict is the one below. When
    // the two agree the crate answers, whichever way the trie went; when they
    // disagree the trie decides the request and the crate cannot say which way.
    //
    // The comparison is on the VERDICT, not on the reported policy: a differing
    // ACCESS policy id is already a declared divergence
    // (`tests/ranger/README.md`) and nothing in this crate asserts one.
    if let Some((policy_id, policy_name, allowed_at_stop)) = unknown_stop {
        if allowed_at_stop != allowed {
            return Decision::Undecidable {
                why: vec![UndecidableCause::ResourceCandidacyNotReproducible {
                    policy_id,
                    policy_name,
                }],
            };
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

/// What phase 1 left on the shared `RangerAccessResult`, in the resource
/// loop's own currency.
///
/// `None` is `isAccessDetermined == false` with `isAllowed` false and
/// `policyPriority` 0 — `createAccessResult`'s untouched result, which is what
/// [`TagDecision::NotDetermined`] means. A refusal is not a state the engine
/// has, so it produces `None` here and is carried to the caller as a cause
/// instead.
///
/// The `reason` an ALLOW carries is `DenyReason::NoPolicyMatched`, the same
/// placeholder [`PolicyView::decide`] uses for an allow: Ranger's `reason` is a
/// free-text comment and this crate only reads it when the answer is a deny.
fn tag_verdict_as_outcome(tag: &TagDecision) -> Option<Outcome> {
    match tag {
        TagDecision::NotDetermined | TagDecision::Undecidable { .. } => None,
        TagDecision::Allowed {
            policy_id,
            policy_name,
            priority,
        } => Some(Outcome {
            allowed: true,
            priority: *priority,
            policy_id: *policy_id,
            policy_name: policy_name.clone(),
            reason: DenyReason::NoPolicyMatched,
        }),
        TagDecision::Denied {
            policy_id,
            policy_name,
            priority,
            reason,
        } => Some(Outcome {
            allowed: false,
            priority: *priority,
            policy_id: *policy_id,
            policy_name: policy_name.clone(),
            reason: *reason,
        }),
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
/// ⚠ **Both escapes are facts about what can RUN, and SP4d-2b Task 7 added the
/// question that comes before them**: whether there is an engine at all.
/// [`tag_block_fails_to_load`] is consulted first and overrides both — see that
/// function for the measurement and for the surviving mutation that found the
/// fail-open.
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
    policies: &ServicePolicySet,
    tags: Option<&EnrichedTags>,
    service_def: &ServiceDef,
    tag_service_def: &ServiceDef,
) -> bool {
    // ⚠ BEFORE either escape, and this ordering is the whole of SP4d-2b Task 7's
    // fix: both escapes below are facts about what can RUN, and
    // `tag_block_fails_to_load` is a fact about whether there is an engine to run
    // anything at all. A block that throws in `RangerPolicyRepository`'s
    // constructor answers nothing whatever the request's tags are and whatever
    // the defs declare, so it must reach the phase and be reported. See that
    // function for the measurement and for the mutation that found this.
    if !tag_block_fails_to_load(policies).is_empty() {
        return true;
    }
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
            if tags.is_some_and(EnrichedTags::is_empty) {
                return false;
            }
            true
        }
    }
}

/// Whether Ranger's candidate filter — `RangerPolicyRepository.getLikelyMatchPolicyEvaluators`,
/// i.e. the `policyResourceTrie` — provably does **not** return `policy` for
/// `request`.
///
/// # Why this question exists at all, and when it started mattering
///
/// FIX ROUND 2, CRITICAL. This crate evaluates every enabled policy of the
/// requested type and lets `RangerDefaultPolicyEvaluator.evaluate`'s own matcher
/// decide what each one does. The justification, written on
/// [`crate::resource::trie_reproduces_matcher`] and true when it was written,
/// was: *the trie is a pre-filter and `evaluate` re-asks the matcher, so an
/// over-returning trie changes nothing.*
///
/// **Modelling offset 553 made that false.**
/// `if (result.getIsAllowed() && !evaluator.hasDeny()) setIsAccessDetermined(true)`
/// lives in the ENGINE loop, after `evaluate` has returned, and reads only
/// `hasDeny()` — never the matcher. So a policy the trie never returns can end
/// the evaluation here and not in Ranger, and with a tag ALLOW seeding
/// `isAllowed` before the resource loop even starts, it does so before any
/// resource policy has matched anything. That is a fail-open, and it is this
/// task's, not an inherited one: before the phases were composed, `isAllowed`
/// could only become true through a MATCHED policy, so an unmatched evaluator
/// could never be the one that stopped the loop.
///
/// `RangerPolicyEngineOptions` reads
/// `.policyengine.option.disable.trie.lookup.prefilter` with default `false`, so
/// the trie is the live path on both plugins, not a configurable one.
///
/// # What is proved, and what is only "cannot tell"
///
/// `true` means PROVABLY ABSENT. `false` means present **or** not established —
/// the two are the same to every caller, because both end in a refusal.
///
/// Measured from the real jar in
/// `tests/ranger/src/test/java/EgretResourceTrieCandidacyTest.java`, against the
/// Trino four-level hierarchy and the request `hive.sales.orders.id`:
///
/// | policy | in Ranger's list | `getMatchType` |
/// |---|---|---|
/// | `hive/sales/orders/id` | **yes** | `SELF` |
/// | `hive/sales/orders/*` | **yes** | `SELF` |
/// | `hive/sales/other/*` | **no** | `NONE` |
/// | `hive/otherdb/othertable/*` | **no** | `NONE` |
/// | `otherc/sales/orders/*` | **no** | `NONE` |
/// | `hive/sales` (schema only) | **yes** | `ANCESTOR` |
/// | `hive/sales/orders/id/sub` | **yes** | `NONE` |
/// | `hive/sales/*ders/*` | **yes** | `SELF` |
/// | `hive/sales/orders/*` with `isExcludes` | **yes** | `NONE` |
///
/// Three things that table settles, and the third is why this is not simply
/// "skip what the matcher rejects":
///
/// 1. **trie ⊇ matcher**, so a `matched` view may determine at 553 without
///    further argument.
/// 2. **`NONE` with the request's own key set and literal values ⟹ absent**,
///    which is what lets ordinary requests answer at all.
/// 3. **ANCESTOR is PRESENT.** `RangerResourceTrie`'s build loop (2.5.0 offsets
///    376-430) adds a policy that declares no value for a level as an
///    *inherited* evaluator of that level `if (evaluator.isAncestorOf(
///    resourceDef))`, and inherited evaluators are unioned into every lookup —
///    that is the mechanism by which a schema-level policy is a candidate for a
///    column request. An `isExcludes` level lands there too. So excluding
///    everything the matcher rejects would be WRONG, in the direction that lets
///    a later policy decide where Ranger had already stopped.
///
/// # The conditions, each traced to a line of that build loop
///
/// Absence is proved **per level**, because that is the only place it is
/// provable: `RangerResourceEvaluatorsRetriever` intersects, over the keys the
/// REQUEST carries, what each level's trie returns — so ONE level answering "not
/// in this trie" removes the policy from the whole answer, whatever the others
/// say. A level the policy does not declare proves nothing: it is the inherited
/// path (row 6). [`crate::resource::level_excluded_by_trie`] holds the three
/// conditions a level must meet to serve as the proof, each traced to a line of
/// the build loop.
///
/// Every resource map of the policy (primary and each `additionalResources`,
/// because `RangerAbstractPolicyEvaluator.init` builds one evaluator per map)
/// must have at least one such level.
///
/// **FIX ROUND 3 replaced a whole-policy rule with this one.** The old rule
/// required the policy to declare EXACTLY the request's keys and then read
/// `getMatchType`'s `NONE` as "a level mismatched" — which needed a separate
/// guard that the request's keys were a gapless hierarchy prefix (else `NONE`
/// could mean `init()` failed), and which could prove nothing about a policy
/// declaring fewer keys, including ones measured ABSENT (`hive/otherdb` against
/// a `hive/sales/…` request). The per-level rule needs neither the key-count
/// test nor the hierarchy guard, and proves strictly more. The superseded
/// conditions, kept because the reasoning is the same and only its scope moved:
///
/// * **it declares exactly the request's resource keys.** Fewer, and the
///   `isAncestorOf` branch can make it an inherited evaluator (row 6); more or
///   different, and a key the def does not know sidesteps the intersection
///   (row 7).
/// * **no `isExcludes` and no `isRecursive` on any level.** Excludes is the
///   other route into `addInheritedEvaluator` (row 9); recursive turns an exact
///   value into a wildcard evaluator (`insert`'s `if (isWildcard ||
///   isRecursive)`).
/// * **every value is trie-prefix-exact** ([`crate::resource::trie_reproduces_matcher`]),
///   so per level the trie's membership IS the matcher's (row 8 is the
///   counter-example: `*ders` parks at `sales`'s child node for the empty
///   prefix and comes back for `orders`).
/// * **the matcher rejects it.** With the key sets equal, `getMatchType` has no
///   ANCESTOR or DESCENDANT to return, so a non-match is a per-level
///   value mismatch — and by the condition above, one the trie makes too.
///
/// The caller must additionally have established that the REQUEST's own key set
/// is a gapless hierarchy prefix ([`request_shape_is_reproducible`]); without
/// that, `getMatchType` can answer `NONE` because `init()` failed rather than
/// because a level mismatched, and the trie does not care about hierarchies.
fn trie_excludes(
    policy: &RangerPolicy,
    request: &RequestResource,
    service_def: &ServiceDef,
    policy_type: PolicyType,
) -> bool {
    let maps = std::iter::once(&policy.resources).chain(policy.additional_resources.iter());
    for map in maps {
        let excluded = request.keys().any(|name| {
            let Some(level) = map.get(name) else {
                // A level the policy does not declare is the INHERITED path
                // (row 6): the trie holds it there for every value, so it
                // proves nothing.
                return false;
            };
            let value = request.get(name).expect("name came from request.keys()");
            level_excluded_by_trie(level, service_def.resource(policy_type, name), value)
        });
        if !excluded {
            return false;
        }
    }
    true
}

/// `createRangerServiceResourceMatcher != null`: some policy type has a
/// hierarchy containing every level in `levels`. See [`enriched_tags`].
///
/// # ⚠ The loop over all THREE policy types has an oracle, and needed a built one
///
/// FIX ROUND 1, RULING 2. Ranger loops `RangerPolicy.POLICY_TYPES` and keeps a
/// resource that validates under ANY of them, so this does too — but on **every
/// service-def egret ships** that is unobservable: `trino.json`'s and
/// `starrocks.json`'s `dataMaskDef` / `rowFilterDef` resource names are subsets
/// of their access names, so any key set valid for masking is valid for access
/// and a one-type loop would answer identically. `tag.json` has one resource.
/// "Right because Ranger does it" is exactly the unfalsifiable assertion this
/// package keeps catching, so the real defs were checked FIRST and none
/// discriminates.
///
/// The oracle is therefore a **constructed** def — the real `trino.json` plus one
/// `dataMaskDef` resource, `maskonly`, with no counterpart in `resources` — and
/// it is measured on both sides:
/// `tests/tag_refusals.rs::a_key_set_valid_only_for_masking_survives_the_three_policy_type_loop`
/// for this function, and
/// `EgretTagServiceResourceValidationTest::aKeySetValidUnderMaskingOnlySurvivesBecauseTheLoopVisitsAllThreeTypes`
/// against the real `createRangerServiceResourceMatcher`, which keeps it.
fn service_resource_validates(levels: &BTreeSet<String>, service_def: &ServiceDef) -> bool {
    [
        PolicyType::Access,
        PolicyType::DataMask,
        PolicyType::RowFilter,
    ]
    .into_iter()
    .any(|policy_type| {
        service_def
            .hierarchies(policy_type)
            .iter()
            .any(|hierarchy| levels.iter().all(|level| hierarchy.contains(level)))
    })
}

/// The `tagPolicies` block constructs that make the plugin throw while BUILDING
/// its policy engine, so that no request of any type is answered — one cause per
/// offending policy, and empty when the block loads.
///
/// `normalizeAndPrunePolicies` runs in `RangerPolicyRepository`'s
/// `ServicePolicies$TagPolicies` constructor — **no other constructor in either
/// jar calls it** — so this is above `isEnabled`, above the policy-type filter
/// and above every per-request consideration. The `NullPointerException` a null
/// `dataMaskType` produces propagates out of `RangerPolicyEngineImpl.<init>`
/// (measured on the real 2.5.0 and 2.8.0 engines by the differential gate, not
/// read off the class). See [`TagNormalisationProblem::MaskTypeAbsent`].
///
/// # ⚠ Why this is a function and not a block inside [`tag_candidates`]
///
/// **SP4d-2b Task 7, found by a surviving mutation.** It WAS a block inside
/// `tag_candidates`, which the phase reaches only after
/// [`tag_policies_can_decide`] has said the block can decide — and that
/// function's second escape is a per-REQUEST measurement (an empty resolved tag
/// set makes `getLikelyMatchPolicyEvaluators` return `Collections.EMPTY_LIST` at
/// `isNotEmpty(tags)`). A per-request narrowing cannot be allowed to skip a
/// CONSTRUCTION-time failure: there is no engine for the narrowing to narrow
/// anything away from. Measured before the fix, with an empty tag set and a tag
/// mask policy carrying no `dataMaskType`: `Denied { NoPolicyMatched }`, with
/// full confidence, against a plugin that is serving nothing at all.
///
/// So the gate consults this too, and both call sites run the SAME function
/// rather than two checks that must agree — the pattern the envelope sweep in
/// [`request_preamble`] and [`tag_candidates`] already uses, with
/// [`dedup_causes`] collapsing the overlap. `tests/tag_refusals.rs` pins all
/// three entries: a tag set that carries a tag, a disabled policy, and the empty
/// set.
///
/// It became reachable from production in the commit that found it: until SP4d-2b
/// Task 7 the one production `AccessRequest` passed `resolved_tags: None`, so the
/// phase always ran; `Some(empty)` is now `explain`'s ordinary answer for a
/// target with no tag recorded on it.
fn tag_block_fails_to_load(policies: &ServicePolicySet) -> Vec<UndecidableCause> {
    let mut why = Vec::new();
    for policy in &policies.tag_policies {
        for item in &policy.data_mask_policy_items {
            let absent = match &item.data_mask_info {
                // The key was absent or null on the wire, which Ranger's model
                // class turns into an empty object with a null type — the F2
                // substitution, reaching a caller that does not null-check.
                None => true,
                Some(info) => info.mask_type.is_none(),
            };
            if absent {
                why.push(UndecidableCause::TagPolicyNotNormalisable {
                    policy_id: policy.id,
                    policy_name: policy.name.clone(),
                    problem: TagNormalisationProblem::MaskTypeAbsent,
                });
                break;
            }
        }
    }
    why
}

/// Phase 1's gate: the tag service-def to evaluate the `tagPolicies` block
/// against, or `None` when Ranger's tag phase provably cannot write to the
/// shared result for this request.
///
/// `None` is an answer, not a refusal — see [`tag_policies_can_decide`] for the
/// two measured escapes and why each is independently sufficient.
///
/// # The gate keys on the DECIDING SET, not only on the reader's label
///
/// `Unmodelled::TagPolicies` is a classification and
/// [`ServicePolicySet::tag_policies`] is what the phase actually evaluates. If
/// those two ever disagree — a reader change, a new tier — keying on the label
/// alone would skip the phase while a non-empty deciding set sat there, which
/// is this package's recurring defect shape: a condition that changes the
/// answer reaching a function that does not look at it. So a non-empty
/// `tag_policies` runs the phase whether or not a construct was reported, under
/// the block tier its own length implies.
fn tag_phase_input(
    policies: &ServicePolicySet,
    tags: Option<&EnrichedTags>,
    service_def: &ServiceDef,
) -> Option<ServiceDef> {
    let declared = policies.unmodelled.iter().find_map(|c| match c {
        Unmodelled::TagPolicies(block) => Some(block.clone()),
        _ => None,
    });
    let block = match declared {
        Some(block) => block,
        None if !policies.tag_policies.is_empty() => {
            TagPolicyBlock::Policies(policies.tag_policies.len())
        }
        None => return None,
    };
    // The block's own def is a second source of context enrichers, so the gate
    // needs it too — see `tag_policies_can_decide`. Parsed through the same
    // `ServiceDef::parse` as the resource def so there is one enricher
    // predicate, not two.
    let tag_service_def = ServiceDef::parse(&policies.tag_service_def);
    tag_policies_can_decide(&block, policies, tags, service_def, &tag_service_def)
        .then_some(tag_service_def)
}

/// Append phase-1 causes to the composed `why`, without repeating one the
/// preamble already raised.
///
/// The two sweeps overlap on purpose: [`tag_candidates`] refuses the same
/// envelope constructs [`request_preamble`] does, so the phase stays safe for
/// any caller rather than depending on one. What must not leak out of that
/// overlap is a `why` naming `policyDeltas` twice — a reader would read two
/// findings where there is one.
/// Drop repeated causes, keeping the first of each and the order of the rest.
///
/// # Why collapsing is safe, per variant that can reach this
///
/// FIX ROUND 2, MINOR B. This said "a cause carries the policy it is about", and
/// two of the variants that arrive here do not:
///
/// | variant | keyed on | why an exact duplicate is ONE finding |
/// |---|---|---|
/// | [`UndecidableCause::PolicyConstruct`] | the policy | the same construct on the same policy, reported by `collect_tag_resource_causes` and again by `collect_causes` — which is what this function exists for |
/// | [`UndecidableCause::TagResourceNotReproducible`] | the policy | same |
/// | [`UndecidableCause::TagAttachmentNotReproducible`] | the **tag**, not a policy | two attachments of one tag that differ only in the service resource are ONE member of Ranger's set (`RangerTagForEval` carries no service resource), so one finding is genuinely being derived twice |
/// | [`UndecidableCause::ServiceDef`] | the **issue**, not a policy | the def is swept once, so an exact duplicate can only come from a second sweep of the same def |
///
/// `distinct_causes_all_survive_and_only_duplicates_collapse` asserts both
/// directions, because a helper that collapsed two findings into one would leave
/// the verdict `Undecidable` and hand the operator one reason where there are
/// two, with nothing to say anything was dropped.
///
/// `UndecidableCause` is only `PartialEq`, so this is quadratic in the number of
/// causes; that number is bounded by the policies in one download.
fn dedup_causes(why: &mut Vec<UndecidableCause>) {
    let mut kept: Vec<UndecidableCause> = Vec::with_capacity(why.len());
    for cause in why.drain(..) {
        if !kept.contains(&cause) {
            kept.push(cause);
        }
    }
    *why = kept;
}

fn merge_causes(why: &mut Vec<UndecidableCause>, from: &[UndecidableCause]) {
    for cause in from {
        if !why.contains(cause) {
            why.push(cause.clone());
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
    // ⚠ `tagPolicies` is NOT refused here, and that is SP4d-2b Task 3's whole
    // change to this function. The block is now EVALUATED — `tag_phase_input`
    // gates it and the tag phase answers it — so refusing it here would refuse
    // the feature. The refusals that block still owes have not gone anywhere:
    // `tag_candidates`'s own envelope sweep raises
    // `EnvelopeConstruct(TagPolicies(Unreadable))` for a block whose shape the
    // reader could not count, and every other tag-side refusal reaches the
    // composed answer through the phase's `Undecidable`. Both callers of this
    // function run the phase; nothing else may.
    //
    // Nothing here narrows `ServiceConfig` — see the ⚠ block on
    // `crate::model::Unmodelled::ServiceConfig` for why the two refusals look
    // alike and carry opposite risk.
    // FIX ROUND 1, MINOR 2. The skip is `Policies(_)`, not the whole variant,
    // so that this sweep and `tag_candidates`'s are ONE rule written twice
    // rather than two rules that happen to agree. Before, `Unreadable` reached
    // its refusal only because `tag_policies_can_decide` returns `true` for it
    // on every path — a third coupling holding up a refusal, and this package
    // has been bitten twice by exactly that. Now either sweep alone refuses it,
    // and `merge_causes` collapses the overlap, so the redundancy costs a
    // comparison and buys the refusal not depending on the gate.
    for construct in &policies.unmodelled {
        if matches!(
            construct,
            Unmodelled::TagPolicies(TagPolicyBlock::Policies(_))
        ) {
            continue;
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
/// # ⚠ WHICH JAR (added 2026-10-10, whole-branch review)
///
/// That listing — four keys, total order, ending at the policy name — is
/// `compareNormal` **as disassembled from `ranger-plugins-common-2.5.0`**, the
/// version Trino 470 loads. The record is
/// `tests/differential.rs`'s `access_order_duel`, which states the jar. StarRocks
/// fe-ubuntu 4.1 loads **2.8.0**, and no disassembly of THIS comparator out of the
/// 2.8.0 jar is on record in this crate, so the listing is not asserted for it.
/// (The three TAG comparators are different: [`sort_tag_candidates`] records all
/// three disassembled in both jars and byte-identical.)
///
/// The distinction is not pedantry — the two plugins demonstrably disagree about
/// another artifact. A tag DENY under a strictly higher-priority resource ALLOW
/// carrying no deny items is `Allowed` on 2.8.0 and not on 2.5.0, which is why
/// [`UndecidableCause::TagDenyReopenedDiffersByPluginVersion`] exists and why
/// [`EngineConfig::plugin_version`] is an input rather than a constant. An
/// unversioned claim about plugin internals is how a comment comes to promise
/// 2.8.0's behaviour to a 2.5.0 reader.
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
    /// Whether any resource map matched at ALL — any match type other than
    /// `NONE`, which for a resource request means `ANCESTOR` and `DESCENDANT`
    /// as well as the two [`PolicyView::matched`] accepts.
    ///
    /// NOT an allow: `RangerDefaultPolicyEvaluator.evaluate` admits `ANCESTOR`
    /// only for a `RangerTagAccessRequest` and never admits `DESCENDANT` at the
    /// `SELF` scope, so this never makes `decide` return anything. It exists for
    /// [`PolicyView::provably_a_candidate`] alone.
    any_match: bool,
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
            any_match: match_policy_resources(policy, &request.resource, service_def)
                .into_iter()
                .any(|m| m.match_type != MatchType::None),
        }
    }

    /// Whether Ranger's candidate filter PROVABLY returns this policy — the
    /// positive half of the table [`trie_excludes`] reads the negative half of.
    ///
    /// **Any match type other than `NONE`**, which is exactly what
    /// `tests/ranger/src/test/java/EgretResourceTrieCandidacyTest.java` measures
    /// at both plugin versions over a column-level and a table-level request:
    /// every `present` row there is a non-`NONE` row, except the two the crate
    /// cannot prove either way (`isExcludes`, which the per-level proof
    /// correctly declines). One argument covers all four:
    ///
    /// * `SELF`, `SELF_AND_ALL_DESCENDANTS`, `DESCENDANT` — every level the
    ///   REQUEST carries matched, and the retriever intersects over exactly
    ///   those levels. A matching value is in its level's trie for every value
    ///   shape (the trie parks a value at a prefix the match already requires),
    ///   so the intersection holds the policy.
    /// * `ANCESTOR` — the policy declares no value for some of the request's
    ///   levels, and `RangerResourceTrie`'s build loop (2.5.0 offsets 376-430)
    ///   adds it to those levels as an *inherited* evaluator
    ///   `if (evaluator.isAncestorOf(resourceDef))`; inherited evaluators are
    ///   unioned into every lookup. The levels it DOES declare matched, or the
    ///   match type would not be `ANCESTOR`.
    ///
    /// The table's `ancestor-other-schema` row (`hive/otherdb` against a
    /// `hive/sales/…` request) is measured **absent** with match type `NONE`, so
    /// this is the hierarchy relation and not merely "declares fewer levels" —
    /// which is what makes `!= NONE` the right predicate rather than a
    /// key-count.
    fn provably_a_candidate(&self) -> bool {
        self.any_match
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
            // The tag phase has its own candidate filter (`tag_candidates`) and
            // its own refusal for what it cannot reproduce
            // ([`UndecidableCause::TagResourceNotReproducible`]); nothing on
            // that path reads this.
            any_match: true,
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

/// The policy a [`SubResult`] names. Both arms carry the same two fields;
/// bundling them with the DATA is exactly what
/// [`UndecidableCause::TagSubResultSplitByResourcePolicy`] exists to refuse.
fn sub_result_identity(result: &SubResult) -> (Option<i64>, String) {
    match result {
        SubResult::Mask(m) => (m.policy_id, m.policy_name.clone()),
        SubResult::RowFilter(f) => (f.policy_id, f.policy_name.clone()),
    }
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

    // ── phase 1, for policy type 1 or 2 (caller offset 176) ────────────
    //
    // `evaluateTagPolicies` runs for every policy type, so a tag mask decides a
    // mask request exactly as a tag deny decides an access one. The priority
    // comes back with the result because the arbitration below compares it.
    // `enriched_tags` first, for the reason written out in `evaluate` — and
    // since Task 7 the phase's own parameter type is what holds that order.
    let enriched = enriched_tags(request, service_def);
    let tag = match tag_phase_input(policies, enriched.as_ref(), service_def) {
        None => None,
        Some(tag_service_def) => {
            match evaluate_tag_sub(kind, policies, request, enriched.as_ref(), &tag_service_def) {
                Err(tag_why) => {
                    merge_causes(&mut why, &tag_why);
                    None
                }
                Ok(found) => found,
            }
        }
    };

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
    // ── arm 3 of the arbitration: a tag verdict already on the result ──
    //
    // Offset 529, `if_icmplt 538`: with `isAllowed` set — which a tag mask
    // leaves, because `RangerDefault{DataMask,RowFilter}PolicyItemEvaluator`
    // reports allow unconditionally — a resource evaluator is shut out unless
    // its priority is STRICTLY higher. That is the `>=` arm, and it is one
    // character from `evaluate`'s `>`.
    //
    // `isDeniedByTags` cannot arise here, and that is structural rather than
    // lucky: for policy types 1 and 2 `getMatchingPolicyItem` takes the
    // single-list overload (no deny bucket), `isDenyAllElse`'s fallback is
    // guarded by `policyType == null || policyType == 0`, and the item
    // evaluator's `updateAccessResult` passes `true`. No tag mask can deny, so
    // offset 452's branch is unreachable on this path and `evaluate_tag_sub`'s
    // type — which cannot express a deny — is the right one.
    //
    // ── which strictly-higher evaluators Ranger actually REACHES ───────
    //
    // FIX ROUND 1, IMPORTANT. The first version of this walked every evaluator
    // above `tag_priority` and refused if any of them yielded, on the argument
    // that "if any of them yields then every order either splits the result or
    // stops before doing so, and refusing covers both". The second half of that
    // sentence is the mistake: "stops before doing so" is not an unknowable
    // order — BETWEEN priority bands it is decided, and this list is already
    // sorted by it. So the walk refused cases Ranger answers deterministically.
    //
    // MEASURED (the shape the review probed): a tag mask at priority 0, a
    // resource mask at priority 5 whose items match nobody, and a resource mask
    // at priority 1 that would yield. Ranger runs the priority-5 evaluator
    // first, it writes nothing, **offset 553 fires anyway** — `isAllowed` is
    // true because the tag phase set it, and that evaluator has no deny items —
    // and `RangerDefaultPolicyEvaluator.evaluate`'s 380-384 guard then skips the
    // priority-1 evaluator's items. The tag mask stands, and nothing about that
    // depends on an order this crate cannot compute.
    //
    // So the walk models 553, exactly as `evaluate` already does on the ACCESS
    // path — that asymmetry was the smell. Three rules, and the third is why
    // the walk takes a whole band rather than stopping mid-band:
    //
    // 1. an evaluator at or below `tag_priority` is shut out by offset 529, and
    //    so is everything after it (the list is priority DESC);
    // 2. a `hasDeny()` evaluator runs and does NOT determine, so the walk
    //    continues past it;
    // 3. the first `!hasDeny()` evaluator determines once it has run — but
    //    WHICH member of its band is first is `getEvalOrder()`, unreproduced.
    //    `sort_eval_order` puts every `hasDeny` member of a priority ahead of
    //    every non-`hasDeny` one, so the band that determines is a contiguous
    //    tail at one priority, and any of its members could be the one that
    //    ran. The whole band is therefore "reachable", and the walk stops at
    //    the end of it.
    //
    // A yield anywhere in the reachable set is the split, for the same reason
    // as before: `updateAccessResult` writes the DATA unconditionally and the
    // IDENTITY only `if (!result.getIsAllowed())`, which the tag phase has
    // already set. A reachable set with no yield leaves both halves untouched
    // and the restore at 599-632 puts the tag verdict back.
    if let Some((tag_result, tag_priority)) = tag {
        let mut reachable: Vec<&PolicyView<'_>> = Vec::new();
        let mut determining_band: Option<i32> = None;
        for view in &candidates {
            // Rule 1 — the negation of offset 529's `>=`.
            if tag_priority >= view.policy.policy_priority {
                break;
            }
            // Rule 3 — past the determining band, nothing runs.
            if determining_band.is_some_and(|p| p != view.policy.policy_priority) {
                break;
            }
            reachable.push(view);
            // ⚠ Only a view PROVABLY in Ranger's candidate list may end the
            // walk at 553 — which is any match type other than `NONE`, see
            // [`PolicyView::provably_a_candidate`]. A view the crate cannot
            // place there might not be in it at all, and unlike the ACCESS path
            // that needs no ruling of its own: if Ranger DID stop on it, nothing
            // behind it wrote and the tag verdict stands, which is what this
            // walk answers when nothing behind it yields; and if anything behind
            // it DOES yield, the two possibilities disagree and the split
            // refusal below is the honest answer either way.
            //
            // FIX ROUND 4: this paragraph said "only a MATCHED view" until the
            // line below had already stopped meaning that — round-2 prose left
            // standing over round-3 code, in this file's most safety-relevant
            // walk. Twice now a superseded justification has outlived the branch
            // it justified, and the first time it was the stale comment that
            // made the defect invisible. A superseded justification is a defect
            // of the same weight as a superseded branch: the next reader cannot
            // tell which of the two is current.
            if !view.has_deny && view.provably_a_candidate() {
                determining_band.get_or_insert(view.policy.policy_priority);
            }
        }
        let split: Vec<&PolicyView<'_>> = reachable
            .into_iter()
            .filter(|view| view.decide_sub(kind, request, &implied).is_some())
            .collect();
        if !split.is_empty() {
            let (tag_policy_id, tag_policy_name) = sub_result_identity(&tag_result);
            return Err(split
                .into_iter()
                .map(|view| UndecidableCause::TagSubResultSplitByResourcePolicy {
                    tag_policy_id,
                    tag_policy_name: tag_policy_name.clone(),
                    resource_policy_id: view.policy.id,
                    resource_policy_name: view.policy.name.clone(),
                })
                .collect());
        }
        return Ok(Some(tag_result));
    }

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
/// It does not arbitrate with the resource policies, and it does not run
/// [`request_preamble`] — no super-user short-circuit, no `SuperUsersUnknown`,
/// no engine/service-def cross-check. A caller wanting a decision calls
/// [`evaluate`]; this exists so the composed evaluator can put the two phases
/// together the way `evaluatePoliciesForOneAccessTypeNoAudit` does.
///
/// **SP4d-2b Task 3 made that premise enforceable rather than documented.** The
/// function is `pub(crate)`, its two mask/row-filter siblings are `#[cfg(test)]`
/// as well, and the Task 2 tests that used to reach them across the crate
/// boundary moved into `crate::tag_eval_tests`. The whole caller set is now:
///
/// | caller | preamble | note |
/// |---|---|---|
/// | [`evaluate`] | **yes**, the statement before | the composed ACCESS answer |
/// | `evaluate_sub` (via [`evaluate_tag_sub`]) | **yes**, the statement before | the composed MASK / ROW-FILTER answer |
/// | `crate::tag_eval_tests` | no | `#[cfg(test)]`, in-crate, and pins THIS phase deliberately |
///
/// The claim to hold this to is not "a call was added" but "no path reaches
/// them without it", and the two production callers put the preamble in the
/// statement immediately before the phase, with only the `SuperUser::Yes`
/// return between them. `tests/tag_arbitration.rs` asserts the consequence from
/// the outside, for all three policy types, with a fixture whose tag policy
/// would otherwise deny.
///
/// # Every input this phase reads, and every input it could have read
///
/// FIX ROUND 1. The first version of this doc named the superuser short-circuit
/// and three envelope constructs and stopped — and the three findings against
/// it were all the same shape: **a condition that changes the answer, reaching a
/// function that does not look at it.** Three patches would have left the fourth
/// unfound, so the list below is exhaustive by construction: every field of
/// every input, with a ruling on each. A new field on any of these types is a
/// new row here.
///
/// ## [`ServicePolicySet`] — the envelope
///
/// | field | read | ruling |
/// |---|---|---|
/// | `tag_policies` | **yes** | the deciding set; filtered by `is_enabled` and RAW `policy_type` |
/// | `tag_service_def` | via the caller's `tag_service_def` argument | the block's own def is the only one that decides here |
/// | `unmodelled: TagPolicies(Policies(n))` | no, deliberately | this IS the block being evaluated; refusing it would refuse the feature |
/// | `unmodelled: TagPolicies(Unreadable)` | **yes → refuses** | `tag_policies` is empty for it, so silence would read as "decided nothing" |
/// | `unmodelled: SecurityZones` | **yes → refuses** | `evaluateTagPolicies` has its OWN zone gate, keyed on `isResourceZoneAssociatedWithTagService` (2.5.0 offsets 139-155 and 207-288). When the request's zone is NOT associated — the default zone included — Ranger keeps exactly the zone-LESS tag policies, which is what this crate evaluates, and the two agree; when it IS associated, Ranger keeps only policies whose `zoneName` equals the request's and skips the zone-less ones this crate evaluates. The crate sees neither the zone nor the association, so it cannot tell the arms apart |
/// | `unmodelled: PolicyDeltas` | **yes → refuses** | incremental download; the block's `policies` is partial — or ABSENT, which is why the sweep must run before the `of_kind.is_empty()` return rather than after it |
/// | `unmodelled: ServiceConfig` | **yes → refuses** | a super-user short-circuits BEFORE this phase runs (caller offsets 114-165 precede the call at 168), so it cannot corrupt the phase's own answer — refused anyway, as the one guard that stops a caller other than the composed evaluator reading a verdict a super-user would have overridden. **Nothing here narrows it** — see the ⚠ block on [`crate::model::Unmodelled::ServiceConfig`] |
/// | `policies` | no | `RangerPolicyRepository.init` keeps the three evaluator lists disjoint; a resource policy cannot answer a tag question |
/// | `service_def` | no | tested: handed the resource def, no hierarchy holds `tag` and everything is `NONE` |
/// | `service_name`, `service_id`, `policy_version` | no | nothing in `evaluateTagPolicies` reads them |
///
/// ## [`AccessRequest`]
///
/// | field | read | ruling |
/// |---|---|---|
/// | `user`, `groups`, `roles` | **yes** | `RangerTagAccessRequest` copies all three from the outer request |
/// | `access_type` | **yes**, and a pseudo one **refuses** | `_any` moves `evaluate` to a branch where `DESCENDANT` is admitted, which makes this phase's OWN admitted set wrong rather than merely incomplete |
/// | `owner` | **yes** | through [`UndecidableCause::ResourceOwnerUnknown`], like the resource path |
/// | `resolved_tags` | **yes** | `None` refuses; `Some(empty)` is the measured narrowing. **SP4d-2b Task 4 split this row into the five below**: the entry is no longer a `(name, match_type)` pair and each of its fields needed a ruling of its own |
/// | `resolved_tags[].name` | **yes** | the value put in the `tag` resource position for the candidacy question, and the `type` half of the identity |
/// | `resolved_tags[].match_type` | **yes** | admission (`tag_match_is_admitted`), the mask comparator, and the identity |
/// | `resolved_tags[].attributes` | **yes, for the IDENTITY only** | `RangerTagForEval.equals` compares it, so it separates two members; nothing in this phase READS an attribute's value, because every policy surface that could (a `${{ … }}` expression, a script condition) already refuses (D8) |
/// | `resolved_tags[].options` | **yes → refuses** | [`TagAttachmentProblem::Options`]; `isApplicable` re-derives the validity evaluators from this map, and the filter it feeds runs before the trie lookup |
/// | `resolved_tags[].validity_periods` | **yes → refuses** | [`TagAttachmentProblem::ValidityPeriods`]; the constructor builds one evaluator per entry, and the filter's answer depends on an access time this crate does not model |
/// | `resolved_tags[].attached_through.levels` | **yes → refuses** under three | [`TagAttachmentProblem::ResourceLevels`]. NOT a Ranger rule: `hierarchyHasAllResources` is a subset test and `{catalog}` passes it. It is a PROVENANCE rule (D2) — fewer than three levels is a payload `ServiceTags::from_assignments` cannot write, so the caller's resolution is not the one that was measured. The def-dependent half of the same payload question — the DROP — is [`enriched_tags`]', because it needs the resource def and this phase is handed the tag def |
/// | `resolved_tags[].attached_through.is_enabled` | **no, and measured** | `processServiceTags` never consults it (D9); `tests/tag_refusals.rs` asserts the two verdicts are identical rather than leaving the omission to be read as an oversight |
/// | `resource` | **no, correctly** | `RangerTagAccessRequest` REPLACES it with a `RangerTagResource` holding only `{tag: <name>}` |
///
/// ## Each tag [`RangerPolicy`]
///
/// | field | read | ruling |
/// |---|---|---|
/// | `is_enabled`, `policy_type`, `policy_priority`, `is_deny_all_else` | **yes** | filter, comparators, `hasDeny` |
/// | `resources` / `additional_resources` — `values` | **yes** | candidacy, plus the trie refusal. FIX ROUND 1: **and a `${{ … }}` / `{TOKEN}` marker in a value refuses, from the per-`of_kind` sweep as WELL as from `collect_causes`** — reported by both and collapsed by [`dedup_causes`], because only the sweep runs before candidacy is decided and the marker is what makes candidacy unreliable. It was a fail-open: the policy matched no tag literally, became no candidate, and its marker was never seen |
/// | `resources` — `is_excludes`, `is_recursive` | **yes → refuse** | both widen the trie; see [`TagResourceProblem`] |
/// | the six item buckets | **yes** | through the same `decide` / `decide_sub` the resource path uses |
/// | `unmodelled` (conditions, `validitySchedules`, …) | **yes** | through `collect_causes`, i.e. for CANDIDATE policies only — which is right for a construct that decides what a policy DOES, since a policy no admitted tag reaches decides nothing |
/// | `unmodelled: ZoneName` | **yes**, from the per-`of_kind` sweep | FIX ROUND 2, MINOR A: this row used to say "reported unconditionally" through `collect_causes`, which on the TAG path means "among candidates" — so a zoned tag policy matching no admitted tag was dropped silently. It is swept per policy of the kind now, like the resource-value marker, because a zoned policy is evidence that a zone EXISTS and the crate cannot tell Ranger's two zone arms apart |
/// | `id`, `name` | **yes** | ordering and reporting |
///
/// ## The tag [`ServiceDef`]
///
/// | accessor | read | ruling |
/// |---|---|---|
/// | `hierarchies`, `resource(..).matcher_options` | **yes** | the matcher, and `wildCard` for the trie classifier |
/// | `access_types` / `implied_access_grants` | **yes** | `computeWithImpliedGrants` runs against the TAG def |
/// | `issues` | **yes** | refuses, like the resource path |
/// | `has_tag_enricher` | no | whether the enricher would have produced this tag set is the CALLER's soundness obligation, documented on [`AccessRequest::resolved_tags`]; the phase is handed the set |
/// | `name` | no | there is no engine cross-check on the tag def — its name is always `tag` |
///
/// ## Where each early return stands, and what it may skip
///
/// FIX ROUND 2. A refusal is only as good as the path that reaches it, and the
/// round-1 sweep sat below a return that the refused condition itself produces.
/// The order is now part of the contract:
///
/// | early return | skips | ruling |
/// |---|---|---|
/// | envelope sweep → `Err` | nothing; it is FIRST | `PolicyDeltas` means "partial **or absent**", so no return may precede it |
/// | `of_kind.is_empty()` → `Ok` | the pseudo-access-type refusal, `TagsUnresolved`, **the per-ATTACHMENT sweep**, the per-policy sweep, `collect_causes`, the def's `issues()` | safe: with no evaluator, Ranger's `getLikelyMatchPolicyEvaluators` returns an empty list and nothing downstream of it can run on either side — the attachment sweep included, since a tag with no policy to reach cannot change an answer however its payload is shaped |
/// | pseudo access type → `Err` | everything below | required: under `_any` the admitted-set rule the code below applies is the wrong one, so it must return BEFORE `admitted` is computed |
/// | `resolved_tags` is `None` → `Err` | **the per-ATTACHMENT sweep**, the per-policy sweep, `collect_causes`, `issues()` | safe, and vacuous for the attachment sweep: there is no attachment to sweep. The verdict is already `Undecidable`; more causes would add detail, not change it |
/// | the per-ATTACHMENT sweep (TASK 4) | nothing; it only APPENDS | it sits above `admitted.is_empty()` and so cannot be routed around by it — and it is vacuous there anyway, because it fires only for tags that return says there are none of |
/// | `admitted.is_empty()` → `Ok` | the per-POLICY sweep, `collect_causes`, `issues()` | safe and MEASURED: Ranger builds candidates for every tag but `evaluate` admits none, so neither side writes anything |
///
/// ## Not an input at all
///
/// [`EngineConfig`] is deliberately not a parameter. The superuser
/// short-circuit sets `isAccessDetermined` and `policyPriority = MAX_VALUE`
/// before `evaluateTagPolicies` is reached, so in Ranger's own structure it is
/// the composed evaluator's, not the phase's — and [`evaluate`]'s
/// [`request_preamble`] already refuses an unknown super-user list for every
/// request.
///
/// # ⚠ A deliberate, named OVER-refusal
///
/// FIX ROUND 1 adds a second of the same shape, and with the same bound: the
/// resource-value expression refusal is also raised per POLICY, from the
/// `of_kind` sweep. Both sweeps sit BELOW the `admitted.is_empty()` return, so
/// the population is every request of that policy type **that resolves at least
/// one admitted tag** — not every request.
///
/// [`UndecidableCause::TagResourceNotReproducible`] is raised per POLICY and is
/// independent of the tag being asked about, so one hand-authored `p*x` tag
/// policy makes every such tag request undecidable — even for a
/// tag where the trie and the matcher demonstrably agree (measured: `*ii`
/// differs for `PII`/`p`/`pix`/`pi`/`""`/`piix`/`pii_x` but **not** for `pii`).
/// Likewise `isRecursive` on an already-wildcard value adds nothing and is
/// refused anyway.
///
/// This was reviewed and kept. Narrowing it means reproducing the trie's own
/// rule — `getNonWildcardPrefix(value)` (the substring before the first
/// wildcard character) plus "the evaluator is a candidate for every request
/// value having that prefix", unioned with every `isExcludes` evaluator — and
/// that is a SECOND matcher, with its own fail-open surface, inside the package
/// whose whole premise is that it refuses what it cannot reproduce. The cost is
/// bounded and measured: egret's own writer emits a single literal tag value
/// (`egret-policy/src/admin/ranger_json.rs`'s `tagify`), which classifies as
/// `Equals` and is answered, so the refusal does not touch anything egret
/// authored. A future narrowing has to reproduce the two lines above, and
/// `tests/ranger/src/test/java/EgretTagTrieAgreementTest.java` is the
/// measurement it would have to keep passing.
pub(crate) fn evaluate_tag_policies(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tags: Option<&EnrichedTags>,
    tag_service_def: &ServiceDef,
) -> TagDecision {
    let candidates = match tag_candidates(
        DecisionKind::Access,
        policies,
        request,
        tags,
        tag_service_def,
    ) {
        Err(why) => return TagDecision::Undecidable { why },
        Ok(c) => c,
    };
    let implied = tag_service_def.implied_access_grants();
    if let Some(why) =
        tag_priority_arbitration(DecisionKind::Access, &candidates, request, &implied)
    {
        return TagDecision::Undecidable { why };
    }

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
    // ⚠ CORRECTED (SP4d-2b Task 5). This comment used to read "unlike the
    // resource loop this one compares no priorities: the first evaluator to
    // determine is final". It is false, and reading `evaluateTagPolicies` is
    // exactly why: the priority arbitration is not in this method. It is in
    // `RangerDefaultPolicyEvaluator.evaluatePolicyItems`, on the
    // `accessTypeResults` map that lives in the request CONTEXT and therefore
    // survives from one tag evaluator to the next. See
    // [`UndecidableCause::TagPriorityArbitrationNotModelled`] for the measured
    // table and for why this loop refuses a multi-priority candidate set instead
    // of ordering it.
    //
    // What remains true, and is what the loop below models, is the
    // EQUAL-priority case: the first evaluator to determine is final, in the
    // comparator's order.
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
///
/// **Test-only since SP4d-2b Task 3.** The composed evaluator calls
/// [`evaluate_tag_sub`] directly, because the cross-phase arbitration needs the
/// winning tag policy's priority and [`MaskDecision`] cannot carry it. What
/// survives here is the phase's typed answer, which is what
/// `tag_eval_tests` pins — shipping it as dead code in the library would be
/// worse than saying so.
#[cfg(test)]
pub(crate) fn evaluate_tag_data_mask(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tags: Option<&EnrichedTags>,
    tag_service_def: &ServiceDef,
) -> MaskDecision {
    match evaluate_tag_sub(
        DecisionKind::DataMask,
        policies,
        request,
        tags,
        tag_service_def,
    ) {
        Err(why) => MaskDecision::Undecidable { why },
        Ok(None) => MaskDecision::NoMask,
        Ok(Some((SubResult::Mask(m), _))) => MaskDecision::Masked(m),
        Ok(Some((SubResult::RowFilter(_), _))) => {
            unreachable!("DataMask kind yields only SubResult::Mask")
        }
    }
}

/// [`evaluate_tag_policies`] for `policyType` 2. Test-only, for the reason on
/// [`evaluate_tag_data_mask`].
#[cfg(test)]
pub(crate) fn evaluate_tag_row_filter(
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tags: Option<&EnrichedTags>,
    tag_service_def: &ServiceDef,
) -> RowFilterDecision {
    match evaluate_tag_sub(
        DecisionKind::RowFilter,
        policies,
        request,
        tags,
        tag_service_def,
    ) {
        Err(why) => RowFilterDecision::Undecidable { why },
        Ok(None) => RowFilterDecision::NoFilter,
        Ok(Some((SubResult::RowFilter(f), _))) => RowFilterDecision::Filtered(f),
        Ok(Some((SubResult::Mask(_), _))) => {
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
/// # The two ties this path refuses
///
/// `MATCH_TYPE_COMPARATOR` and `NAME_COMPARATOR` call nothing this crate lacks
/// — match type, then policy priority, then `hasDeny`, then the policy name —
/// so unlike the resource path there is no `getEvalOrder()` score standing
/// between the crate and Ranger's order. Two things nevertheless remain
/// unorderable, and FIX ROUND 1 turned both from silent picks into refusals:
///
/// 1. **A policy-NAME tie between two distinct policies that each yield**
///    (review Minor 3). [`tag_name_order`] is Ranger's comparator exactly; the
///    policy-id tie-break [`sort_tag_candidates`] appends is a key Ranger does
///    **not** have, so on a genuine tie this crate would pick deterministically
///    where Ranger picks by repository order. That tie is representable:
///    `policy.authz_policies` is `UNIQUE (service_id, name)` on the COLUMN, but
///    the download's `name` comes from `policy_jsonb` and
///    `enabled_policies_for_service` injects only `id`/`version`/`isEnabled` —
///    the body's own `name` is never overwritten. Reported as
///    [`UndecidableCause::EvalOrderAmbiguous`], the variant that already means
///    "a tie whose winner this crate cannot name".
///
/// 2. **A yielding policy that is also deny-capable** (review question 3).
///    `RangerDefault{DataMask,RowFilter}PolicyItemEvaluator.updateAccessResult`
///    writes the mask FIELDS unconditionally and then calls
///    `policyEvaluator.updateAccessResult(..., true, ...)`, whose allow branch
///    is guarded by `if (!result.getIsAllowed())`. So once a `hasDeny()` policy
///    has yielded without determining, a LATER yielder replaces the DATA but
///    not the policy IDENTITY, and Ranger reports the first policy's id with
///    the second's mask. [`SubResult`] carries the two together and cannot
///    express that.
///
///    The first version of this function DOCUMENTED that instead of refusing
///    it, on the grounds that egret's compiler emits neither `denyPolicyItems`
///    nor `isDenyAllElse` on a tag mask policy. That was the wrong test:
///    nothing ENFORCES it. `create_policy`/`update_policy` take `Json<Value>`
///    (`egret-policy/src/admin/mod.rs`), the raw-JSON editor is still the only
///    policy UI, and `enabled_policies_for_service` re-serves the stored body
///    verbatim — so the buckets are reachable, and this crate's own rule is to
///    refuse what it cannot reproduce rather than to trust a sibling crate's
///    current behaviour. Refused as
///    [`UndecidableCause::TagSubPolicyIsDenyCapable`], at the point the split
///    can start rather than for every such policy in the block.
///
/// # What the `i32` is
///
/// `RangerAccessResult.getPolicyPriority()` as the phase leaves it — the
/// yielding tag policy's `policyPriority`, written by
/// `RangerDefaultPolicyEvaluator.updateAccessResult`'s allow branch alongside
/// the id. Task 3's cross-phase arbitration compares it against each resource
/// evaluator's, so it is returned rather than recomputed at the call site: the
/// policy that produced the result is the only one that may name it.
fn evaluate_tag_sub(
    kind: DecisionKind,
    policies: &ServicePolicySet,
    request: &AccessRequest,
    tags: Option<&EnrichedTags>,
    tag_service_def: &ServiceDef,
) -> Result<Option<(SubResult, i32)>, Vec<UndecidableCause>> {
    let candidates = tag_candidates(kind, policies, request, tags, tag_service_def)?;
    let implied = tag_service_def.implied_access_grants();
    if let Some(why) = tag_priority_arbitration(kind, &candidates, request, &implied) {
        return Err(why);
    }

    let mut allowed = false;
    let mut determined = false;
    let mut found: Option<SubResult> = None;
    let mut winner: Option<&TagCandidate<'_>> = None;
    for candidate in &candidates {
        if determined {
            break;
        }
        let mut t_allowed = allowed;
        let mut t_found = found.clone();
        let mut t_determined = false;

        if let Some(result) = candidate.view.decide_sub(kind, request, &implied) {
            // Refusal 2: this yield is the one Ranger would report the DATA of
            // while keeping an earlier policy's id, the moment a later
            // candidate also yields. Refused here — where the split can start —
            // rather than swept over every deny-capable policy in the block.
            if candidate.view.has_deny {
                return Err(vec![UndecidableCause::TagSubPolicyIsDenyCapable {
                    policy_id: candidate.view.policy.id,
                    policy_name: candidate.view.policy.name.clone(),
                }]);
            }
            t_found = Some(result);
            t_allowed = true;
            winner = Some(candidate);
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

    // Refusal 1: another candidate tying with the winner on every key Ranger's
    // comparator HAS — and yielding too — is one this crate orders by a key
    // Ranger lacks. `std::ptr::eq` rather than the policy id, because the same
    // policy reached through two tags of the same match type ties with itself
    // and is not an ambiguity.
    if let Some(won) = winner {
        let tied: Vec<&TagCandidate<'_>> = candidates
            .iter()
            .filter(|c| !std::ptr::eq(c.view.policy, won.view.policy))
            .filter(|c| tag_order_without_the_id_standin(kind, c, won) == Ordering::Equal)
            .filter(|c| c.view.decide_sub(kind, request, &implied).is_some())
            .collect();
        if !tied.is_empty() {
            let mut policy_ids = vec![won.view.policy.id];
            let mut policy_names = vec![won.view.policy.name.clone()];
            for c in tied {
                policy_ids.push(c.view.policy.id);
                policy_names.push(c.view.policy.name.clone());
            }
            return Err(vec![UndecidableCause::EvalOrderAmbiguous {
                policy_ids,
                policy_names,
            }]);
        }
    }
    // `found` and `winner` are set together and only together: a candidate that
    // yields and is `hasDeny` returns above, so the one that yields always
    // determines, and the loop cannot reach a second.
    Ok(found.map(|result| {
        let won = winner.expect("a yielded sub-result always records its candidate");
        (result, won.view.policy.policy_priority)
    }))
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
/// The tag phase's `policyPriority` arbitration, as far as this crate models it
/// — which is: only when every deciding candidate shares one priority.
///
/// `Some` is the refusal. See
/// [`UndecidableCause::TagPriorityArbitrationNotModelled`] for the measurement
/// and for why this is a refusal rather than a model.
///
/// Keyed on the POLICY, not on the candidate: one policy reached through two
/// tags is one policy, and reporting it twice would read as two findings. The
/// deciding test is the KIND's own (`decide` for access, `decide_sub` for a mask
/// or a row filter), because a candidate that decides nothing cannot occupy the
/// `accessTypeResults` slot and so cannot be arbitrated against anything.
///
/// # ⚠ It refuses a same-verdict set too, and the reason is the PRIORITY, not the id
///
/// FIX ROUND 1, MINOR 5 raised this; the **whole-branch review re-raised it,
/// because the reason given was inconsistent with the rest of the crate** — it
/// read *"a tie that all agree on is not a tie the caller can act wrongly on — but
/// the reported policy id still differs, and this crate does not report a policy it
/// cannot name"*, while the ACCESS path ships a DECLARED reported-id divergence
/// allow-list (138 of 1716 compared ids at the recorded run) on exactly that cost.
/// A crate cannot refuse here to protect id fidelity and declare it away there.
///
/// The reason that holds does not mention the id at all. **[`TagDecision`] carries
/// the winning policy's `policyPriority`, and that number is an INPUT to the
/// resource phase**: [`tag_verdict_as_outcome`] puts it in `priority`, and offsets
/// 469/505 compare it against every resource evaluator. A same-verdict set at two
/// priorities therefore leaves the composed evaluator's shut-out window
/// undetermined — which is a VERDICT uncertainty on the resource path, not an
/// identity one. The whole-branch review's CRITICAL is what makes this concrete
/// rather than theoretical: the same number, mis-modelled by one write, was a
/// confident over-deny (see
/// [`UndecidableCause::AccessPriorityCompositeNotModelled`]).
///
/// So the refusal is NOT wider than the verdict requires in the composed case, and
/// narrowing it would need a model of when the priority cannot matter — a second
/// model of the mechanism open item 0 exists to audit. The reported id is a
/// secondary cost and is counted on the differential's own id axis rather than
/// claimed here.
///
/// ⚠ What is still NOT measured, and the review is right about it: the
/// same-verdict FRACTION of this refusal was never split out (30 of 1259 composed
/// random cases at the recorded run, unsplit). Splitting it is the first thing
/// open item 0 needs, because it is the size of the prize for modelling the
/// arbitration. It is NOT split here for a structural reason rather than a
/// preference: the differential sees only the cause payload, which carries ids,
/// names and priorities and NOT the verdicts, so splitting the fraction means
/// putting the agreement on the payload — a wire-shape change through
/// `explain_wire.rs`, `openapi.json` and the frontend switch. That is handed to
/// open item 0 with the rest of the audit instead of being half-done here.
fn tag_priority_arbitration<'a>(
    kind: DecisionKind,
    candidates: &[TagCandidate<'a>],
    request: &AccessRequest,
    implied: &BTreeMap<&str, BTreeSet<&str>>,
) -> Option<Vec<UndecidableCause>> {
    let mut deciding: Vec<&'a RangerPolicy> = Vec::new();
    for candidate in candidates {
        if deciding
            .iter()
            .any(|p| std::ptr::eq(*p, candidate.view.policy))
        {
            continue;
        }
        let decides = match kind {
            DecisionKind::Access => candidate.view.decide(request, implied).is_some(),
            _ => candidate.view.decide_sub(kind, request, implied).is_some(),
        };
        if decides {
            deciding.push(candidate.view.policy);
        }
    }
    let mut priorities: Vec<i32> = deciding.iter().map(|p| p.policy_priority).collect();
    priorities.sort_unstable();
    priorities.dedup();
    if priorities.len() < 2 {
        return None;
    }
    Some(vec![UndecidableCause::TagPriorityArbitrationNotModelled {
        policy_ids: deciding.iter().map(|p| p.id).collect(),
        policy_names: deciding.iter().map(|p| p.name.clone()).collect(),
        priorities: deciding.iter().map(|p| p.policy_priority).collect(),
    }])
}

fn tag_candidates<'a>(
    kind: DecisionKind,
    policies: &'a ServicePolicySet,
    request: &AccessRequest,
    tags: Option<&EnrichedTags>,
    tag_service_def: &ServiceDef,
) -> Result<Vec<TagCandidate<'a>>, Vec<UndecidableCause>> {
    let mut why: Vec<UndecidableCause> = Vec::new();

    // ── the envelope constructs, all of them, BEFORE anything can return ──
    //
    // FIX ROUND 2, IMPORTANT. This sweep used to sit below the
    // `of_kind.is_empty()` return, and `PolicyDeltas` — whose whole meaning is
    // "`policies` is partial **or absent**" — was therefore unreachable in the
    // one state it exists to catch: an empty `of_kind` is not an edge case for
    // that refusal, it is its signature. That is the C1/I1/I2 shape one layer
    // up: a refusal routed around by the very condition it refuses. Nothing may
    // return before this loop.
    //
    // Every construct except the block being evaluated refuses:
    //
    // * `TagPolicies(Unreadable)` — `ServicePolicySet::tag_policies` is EMPTY
    //   for a block whose `policies` array did not bind, so silence here reads
    //   as "the policies decided nothing". (This is also why the round-1
    //   `block_unreadable` flag is gone: the sweep now refuses it directly,
    //   before the `of_kind` return it used to have to reach past.)
    // * `PolicyDeltas` — an incremental download, so the block's `policies` is
    //   partial in exactly the way `policies` is.
    // * `SecurityZones` — see the correction below.
    // * `ServiceConfig` — a super-user short-circuits at
    //   `policyPriority = Integer.MAX_VALUE` BEFORE `evaluateTagPolicies` is
    //   called at all (caller offsets 114-165 precede the call at 168), so it
    //   cannot corrupt the phase's own answer. Refused anyway, as the one guard
    //   that stops a caller other than the composed evaluator reading a tag
    //   verdict a super-user would have overridden. See the ⚠ block on
    //   `crate::model::Unmodelled::ServiceConfig` — nothing here narrows it.
    //
    // # FIX ROUND 2, MINOR: the zone reason, corrected
    //
    // Round 1 said "in a non-default zone Ranger evaluates only tag policies
    // whose `zoneName` matches the request's, so a zone-less policy this crate
    // evaluates is one Ranger skips". That is the right conclusion off a wrong
    // reason, and backwards in half the cases. MEASURED (2.5.0 offsets 139-155
    // for the flag, 207-288 for the two arms; same shape in 2.8.0):
    //
    // ```java
    // boolean isDefaultZone = !policyEngine.isResourceZoneAssociatedWithTagService(zoneName);
    // ...
    // if (isDefaultZone) { if (isNotEmpty(policy.getZoneName())) continue; }   // keep zone-LESS
    // else               { if (!StringUtils.equals(zoneName, policy.getZoneName())) continue; }
    // ```
    //
    // The gate keys on whether the request's zone is ASSOCIATED WITH THE TAG
    // SERVICE, not on default versus non-default. When it is not — which
    // includes the default zone, and any other zone that simply is not
    // associated — Ranger keeps exactly the tag policies with an EMPTY
    // `zoneName`, which is the set this crate evaluates, and the two AGREE.
    // The divergence is the other arm: when the zone IS associated, a zone-less
    // tag policy fails `equals(zoneName, null)` and Ranger skips it while this
    // crate evaluates it. The crate sees neither the request's zone nor the
    // association, so it cannot tell the arms apart — and `securityZones` is
    // the only signal that a zone exists at all.
    //
    // FIX ROUND 2, MINOR A: this parenthesis used to read "a policy WITH a
    // `zoneName` is already refused per policy through `Unmodelled::ZoneName`,
    // which `collect_causes` reports unconditionally". That was FALSE on this
    // path — `collect_causes` runs only over CANDIDATES here, so a zoned tag
    // policy whose value matches no admitted tag was never reported. It is now
    // refused from `collect_tag_resource_causes`, which sweeps every policy of
    // the kind; see the ⚠ block there for the measurement and for why the
    // verdict was nonetheless safe before.
    for construct in &policies.unmodelled {
        if matches!(
            construct,
            Unmodelled::TagPolicies(TagPolicyBlock::Policies(_))
        ) {
            continue;
        }
        why.push(UndecidableCause::EnvelopeConstruct(construct.clone()));
    }
    if !why.is_empty() {
        return Err(why);
    }

    // ── the TAG repository's constructor CRASH (SP4d-2b Task 5) ───────────
    //
    // Above the `of_kind` return, above the access-type refusal, and NOT
    // filtered by `isEnabled` or by policy type — because
    // `normalizeAndPrunePolicies` runs in `RangerPolicyRepository`'s
    // constructor, before `init()` drops a disabled policy, and the
    // `NullPointerException` it throws on a null `dataMaskType` propagates out
    // of `RangerPolicyEngineImpl.<init>`. There is no engine afterwards, so
    // there is no decision of any type to be had. See
    // [`TagNormalisationProblem::MaskTypeAbsent`].
    why.extend(tag_block_fails_to_load(policies));
    if !why.is_empty() {
        return Err(why);
    }

    let of_kind: Vec<&'a RangerPolicy> = policies
        .tag_policies
        .iter()
        .filter(|p| p.is_enabled)
        // The one arm of the tag normalisation this crate REPRODUCES rather
        // than refusing — see [`tag_policy_is_pruned`].
        .filter(|p| !tag_policy_is_pruned(p))
        .filter(|p| p.policy_type == kind.policy_type_code())
        .collect();
    if of_kind.is_empty() {
        // Nothing to evaluate, and — now that the sweep runs above this —
        // nothing hidden either. This is the tier every auto-bound Trino
        // profile lands in (`bind_trino_profile` creates `{name}_tag` with zero
        // policies), so it must stay an ANSWER.
        //
        // What this return still skips, and why each is safe: the
        // pseudo-access-type refusal below (with no evaluator, Ranger's
        // `isAccessTypeAny` branch is unreachable and neither side determines
        // anything), `TagsUnresolved` (no evaluator needs a tag set), the
        // per-policy resource sweep and `collect_causes` (vacuous — there are
        // no policies), and the tag def's `issues()` (a def this crate cannot
        // reproduce decides nothing when no policy uses it).
        return Ok(Vec::new());
    }

    // ── the request-shape refusal this phase owes ──────────────────────
    //
    // FIX ROUND 1, IMPORTANT 1. `tag_match_is_admitted` below is written for
    // ONE request shape — `ResourceMatchingScope.SELF` with a CONCRETE access
    // type — and a pseudo access type silently leaves that shape:
    // `RangerDefaultPolicyEvaluator.evaluate` takes the
    // `if (request.isAccessTypeAny())` branch, where `isMatched` is
    // `matchType != NONE` and **`DESCENDANT` is admitted** (2.5.0 offsets
    // 260-284, 2.8.0 268-331). So a `DESCENDANT` tag on a deny policy is a
    // Ranger deny and was this crate's `NotDetermined`.
    //
    // `UndecidableCause::PseudoAccessType` already exists and already covers
    // it — but it is raised in `request_preamble`, which this phase
    // deliberately does not run, so the phase has to raise it itself. This is
    // not an envelope check the composed evaluator can add later: it is what
    // makes the phase's OWN admitted set correct.
    //
    // Deliberately BELOW the `of_kind` return, unlike the envelope sweep: with
    // no evaluator to run, the access type cannot change an answer, and
    // refusing it there would cost every tagless download an answer for
    // nothing.
    if matches!(
        request.access_type.as_str(),
        ANY_ACCESS | ADMIN_ACCESS | SUPER_USER_ACCESS
    ) {
        // Returned before `admitted` is computed, because under `_any` the
        // admitted-set rule `admitted` uses is the wrong one.
        return Err(vec![UndecidableCause::PseudoAccessType(
            request.access_type.clone(),
        )]);
    }

    let Some(resolved) = tags else {
        // A block with policies this decision would evaluate, and no tag set
        // to evaluate them against. `None` is "the caller did not resolve",
        // which must not be read as the empty set that narrows.
        //
        // ⚠ The `None` arrives as this parameter and NOT off
        // `request.resolved_tags`, which is `UnenrichedTags` and which this
        // function must never read — see the ⚠ block on that type. The
        // `Option` is preserved across the enricher by `enriched_tags`, so
        // "the caller did not resolve" survives enrichment unchanged.
        return Err(vec![UndecidableCause::TagsUnresolved]);
    };

    // ── the attachment refusals (SP4d-2b Task 4) ──────────────────────
    //
    // Raised for every resolved tag the evaluator ADMITS, before the dedup and
    // independently of it. Two reasons it is written this way rather than
    // folded into the loop below:
    //
    // * **Order independence.** `attached_through` is deliberately NOT in
    //   `ResolvedTag::identity` (Ranger's `RangerTagForEval` carries no service
    //   resource), so one tag attached at two resources that produced the same
    //   match type is ONE member — and inside the dedup loop the refusal would
    //   then fire or not depending on which attachment the caller happened to
    //   list first. An answer that depends on the caller's `Vec` order is not
    //   an answer. The cost is named: where a tag reaches the request through
    //   BOTH a modelled shape and an unmodelled one, this refuses although
    //   Ranger's set is the same either way.
    // * **Only admitted tags.** `RangerDefaultPolicyEvaluator.evaluate` lets a
    //   tag reach the policy items only at the three match types
    //   `tag_match_is_admitted` names, and a tag that cannot reach them writes
    //   nothing whatever its payload carried. `isApplicable` runs earlier than
    //   admission in Ranger (offset 62, before the trie lookup at 88), so this
    //   is a deliberate NARROWING of where the refusal fires, not a copy of
    //   Ranger's filter order: it refuses exactly where the filter could change
    //   this decision.
    //
    // Nothing routes around it. The `admitted.is_empty()` return below is the
    // only return between here and the `why` check at the end, and it is
    // reachable only when no tag is admitted — which is precisely the state in
    // which this sweep has nothing to say.
    for tag in resolved.as_slice() {
        if !tag_match_is_admitted(tag.match_type) {
            continue;
        }
        let mut refuse = |problem: TagAttachmentProblem| {
            why.push(UndecidableCause::TagAttachmentNotReproducible {
                tag: tag.name.clone(),
                problem,
            });
        };
        if !tag.options.is_empty() {
            refuse(TagAttachmentProblem::Options(
                tag.options.iter().cloned().collect(),
            ));
        }
        if tag.validity_periods > 0 {
            refuse(TagAttachmentProblem::ValidityPeriods(tag.validity_periods));
        }
        if tag.attached_through.levels.len() < 3 {
            refuse(TagAttachmentProblem::ResourceLevels(
                tag.attached_through.levels.iter().cloned().collect(),
            ));
        }
    }

    // `RangerTagForEval`'s `equals`/`hashCode` include `matchType` and the
    // enricher hands Ranger a `Set`, so this crate keys on the tag's identity
    // rather than on the name alone (SP4d-2b design, D4).
    //
    // FIX ROUND 1, MINOR 2 / SP4d-2b TASK 4: the key used to be
    // `(name, matchType)`, which was a COARSENING — `equals`/`hashCode` also
    // read `attributes`, `options` and `validityPeriods`. It was harmless only
    // while none of the three was modelled. Task 4 models all three, so the key
    // is now [`ResolvedTag::identity`], all five components, and the two moved
    // in the SAME commit: had `attributes` arrived alone, two members Ranger
    // keeps apart would have collapsed into one and the dedup that looks
    // correct today would have started dropping a member.
    //
    // FIX ROUND 1, MINOR 1: the caller's order is NOT dropped. `admitted`
    // keeps it, candidates are pushed in it, and `sort_by` is stable — so it
    // survives as the last tie-break after the comparator and the policy-id
    // stand-in. It decides nothing today, because a full tie there is the same
    // policy reached through two tags; the comparators are what order anything
    // that differs.
    let mut seen: BTreeSet<_> = BTreeSet::new();
    let mut admitted: Vec<&ResolvedTag> = Vec::new();
    for tag in resolved.as_slice() {
        if !seen.insert(tag.identity()) {
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
        // could not have filtered, which is why the per-POLICY refusal sweep
        // is below this return and not above it.
        //
        // TASK 4: the per-ATTACHMENT sweep above is the exception, and it does
        // not make this return a bypass — it fires only for tags this very
        // condition says there are none of, so `why` is necessarily empty here.
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
    // FIX ROUND 1. `collect_tag_resource_causes` reports a resource-value
    // expression marker over EVERY policy of the kind (candidacy is not decided
    // yet there, which is the point), and `collect_causes` reports the same
    // construct again for a policy that turned out to be a candidate. One cause
    // carries its policy's id and name, so two equal causes are one finding
    // reported twice — a reader would see two. The composed evaluator's
    // `merge_causes` already collapses them; this makes the phase's own answer
    // correct for a caller that is not it.
    dedup_causes(&mut why);
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
    candidates.sort_by(|a, b| {
        tag_order_without_the_id_standin(kind, a, b)
            // The policy-id key is NOT Ranger's — see
            // [`tag_order_without_the_id_standin`]. It buys determinism; on the
            // ACCESS path it is the declared stand-in for `getEvalOrder()`, and
            // on the mask/row-filter paths [`evaluate_tag_sub`] refuses rather
            // than letting it decide.
            .then_with(|| a.view.policy.id.cmp(&b.view.policy.id))
    });
}

/// Everything in this policy type's comparator that Ranger actually HAS, and
/// nothing else — so a caller can ask whether two candidates are genuinely
/// tied for Ranger, separately from how this crate breaks that tie.
fn tag_order_without_the_id_standin(
    kind: DecisionKind,
    a: &TagCandidate<'_>,
    b: &TagCandidate<'_>,
) -> Ordering {
    match kind {
        // `EVAL_ORDER_COMPARATOR` also reads `getEvalOrder()`, which this crate
        // does not compute — so on this path the comparison below is already
        // incomplete and the id stand-in is doing declared work, not hiding a
        // tie. See the note in [`sort_tag_candidates`].
        DecisionKind::Access | DecisionKind::RowFilter => tag_name_order(a, b),
        DecisionKind::DataMask => match_type_rank(a.match_type)
            .cmp(&match_type_rank(b.match_type))
            .then_with(|| tag_name_order(a, b)),
    }
}

/// `RangerPolicyEvaluator.NAME_COMPARATOR`, and — with `getEvalOrder()`
/// unreproduced — `EVAL_ORDER_COMPARATOR` too: priority DESC, `hasDeny` first,
/// then `compareStrings(policy name)`, and **nothing after that**.
///
/// "Nothing after that" is a claim about a total order, so it names its source:
/// both comparators disassembled from `ranger-plugins-common` **2.5.0 and 2.8.0**
/// and byte-identical there, which is the record [`sort_tag_candidates`] carries
/// for all three tag comparators. That is wider than what is on record for the
/// RESOURCE path's `PolicyEvalOrderComparator`, where only the 2.5.0 jar is named
/// — see [`sort_eval_order`]'s "WHICH JAR" note. The policy-id
/// stand-in lives in [`sort_tag_candidates`] instead, so that
/// [`tag_order_without_the_id_standin`] can say when two candidates are tied
/// for RANGER rather than merely tied here.
fn tag_name_order(a: &TagCandidate<'_>, b: &TagCandidate<'_>) -> Ordering {
    b.view
        .policy
        .policy_priority
        .cmp(&a.view.policy.policy_priority)
        .then_with(|| b.view.has_deny.cmp(&a.view.has_deny))
        .then_with(|| a.view.policy.name.cmp(&b.view.policy.name))
}

/// Everything about one tag policy's RESOURCE map that stops this crate
/// filtering the way `RangerResourceEvaluatorsRetriever` does.
///
/// Swept over every enabled tag policy of the decision's type, not only the
/// ones that came out as candidates — a policy whose candidacy is exactly what
/// cannot be established is not one the crate may quietly leave out.
/// Whether the TAG policy repository's constructor removes `policy` outright.
///
/// The one arm of `normalizeAndPrunePolicies` this crate reproduces instead of
/// refusing, because its gate is a plain read of the wire with nothing
/// ambiguous in it: `isPolicyNeedsPruning` returns true when
/// `!getIsAuditEnabled()` and all SIX item buckets are empty (2.5.0 offsets
/// 60-133, identical in 2.8.0).
///
/// **`isDenyAllElse` is not one of the six**, and that is what makes this
/// observable rather than vacuous. A tag ACCESS policy with
/// `isDenyAllElse: true`, no items and `isAuditEnabled: false` decides nothing
/// in Ranger — it is not in the repository — while without this filter it would
/// be `has_deny` here and would DENY. Fail-CLOSED, which is why it went
/// unnoticed, and wrong either way.
///
/// The prune is reached only BEHIND the access-type refusal, and deliberately:
/// `normalizeAndPrunePolicyItems` can also empty a bucket by dropping every
/// access of an item whose accesses all carried a foreign `:` prefix, and that
/// path is refused by [`TagNormalisationProblem::NamespacedAccessType`] rather
/// than reproduced. An item with NO accesses at all is **not** pruned —
/// `CollectionUtils.isNotEmpty(accesses)` guards the whole inner block at offset
/// 67 — so an empty-`accesses` item still keeps its bucket non-empty, and this
/// function must not "helpfully" treat it as absent.
///
/// `isEnabled` is a different mechanism (`skipBuildingPolicyEvaluator`, at
/// `init` time) and is tested separately at every call site.
fn tag_policy_is_pruned(policy: &RangerPolicy) -> bool {
    !policy.is_audit_enabled
        && policy.policy_items.is_empty()
        && policy.deny_policy_items.is_empty()
        && policy.allow_exceptions.is_empty()
        && policy.deny_exceptions.is_empty()
        && policy.data_mask_policy_items.is_empty()
        && policy.row_filter_policy_items.is_empty()
}

fn collect_tag_resource_causes(
    policy: &RangerPolicy,
    tag_service_def: &ServiceDef,
    why: &mut Vec<UndecidableCause>,
) {
    // ── the namespaced access and mask types (SP4d-2b Task 5) ─────────────
    //
    // In this sweep rather than in `PolicyView::collect_causes` for the same
    // reason the expression marker is: candidacy has not been decided yet here,
    // and a refusal that only fires for policies whose tag value already matched
    // is not unconditional. Scoped to policies of the KIND, unlike the
    // constructor crash above, because the normalisation rewrites a policy's own
    // buckets and cannot reach a decision of another type.
    //
    // All six buckets, because `normalizeAndPrunePolicyItems` is called on all
    // six (offsets 0-59 of `isPolicyNeedsPruning`) and because the crate's own
    // `accessPerms` model (`checkIfHasAllPerms`) reads all six too.
    for items in all_buckets(policy) {
        for item in items {
            for access in &item.accesses {
                if access.access_type.contains(':') {
                    why.push(UndecidableCause::TagPolicyNotNormalisable {
                        policy_id: policy.id,
                        policy_name: policy.name.clone(),
                        problem: TagNormalisationProblem::NamespacedAccessType(
                            access.access_type.clone(),
                        ),
                    });
                }
            }
        }
    }
    for item in &policy.data_mask_policy_items {
        if let Some(mask_type) = item
            .data_mask_info
            .as_ref()
            .and_then(|i| i.mask_type.as_ref())
        {
            if mask_type.contains(':') {
                why.push(UndecidableCause::TagPolicyNotNormalisable {
                    policy_id: policy.id,
                    policy_name: policy.name.clone(),
                    problem: TagNormalisationProblem::NamespacedMaskType(mask_type.clone()),
                });
            }
        }
    }

    // ── FIX ROUND 1: a runtime expression in the tag VALUE ────────────────
    //
    // **This was a fail-open, and it was found by trying to write the test that
    // pins `ResourceValueExpressionMarker` on the tag path.**
    //
    // On the resource path `PolicyView::collect_causes` reports this construct
    // whether or not `matched` came out true — its `always` list — because the
    // marker is exactly what makes `matched` unreliable. The tag path could not
    // reach that rule: `tag_candidates` builds a `PolicyView` only for policies
    // whose `resources.tag` covers an admitted tag NAME, compared as a literal,
    // so a policy whose tag value is `${{ … }}` matched nothing, became no
    // candidate, and `collect_causes` never ran for it. A deny policy vanished
    // from the candidate set and `evaluate` answered `Denied{NoPolicyMatched}`
    // with full confidence.
    //
    // MEASURED, 2.5.0: `ResourceMatcher`'s constructor tests
    // `RangerRequestExprResolver.hasExpressions(value)` at offset 17 and stores
    // a resolver at 33; `getExpandedValue(Map)` calls
    // `exprResolver.resolveExpressions(request)` at offset 26. So the value is
    // resolved PER REQUEST, and the literal comparison this crate does is not
    // the question Ranger asks.
    //
    // Raised here, in the sweep over EVERY policy of the kind, rather than ONLY
    // in `collect_causes`, because this is the only place candidacy has not been
    // decided yet — the same reason the resource path puts it in the `always`
    // list. It is reported from BOTH and `tag_candidates` de-duplicates
    // afterwards, since a policy that IS a candidate reaches `collect_causes`
    // too.
    //
    // ── FIX ROUND 2, MINOR A: `ZoneName` belongs in the same loop ──────────
    //
    // `collect_causes`' `always` list means "regardless of `matched`", and on the
    // RESOURCE path that is enough because `evaluate` builds a `PolicyView` for
    // EVERY enabled policy of the type. On the TAG path it is not: a view exists
    // only for a CANDIDATE, so `always` there means "regardless of `matched`,
    // among policies whose tag value already matched" — which is not
    // unconditional at all. That conflation is exactly the sentence that produced
    // this round's fail-open, and the zone comment in `tag_candidates` repeated
    // it: a zoned tag policy whose value does not match an admitted tag was
    // observed and dropped on the floor.
    //
    // Measured before this fix, tag block `[{80, zoneName: "z1", tag=["phi"],
    // deny}]` against request tag `pii`: ANSWERED `Denied{NoPolicyMatched}`. With
    // a zone-LESS allow beside it, `Allowed{70}` — while Ranger's
    // associated-zone arm (offset 285, `equals(requestZone, policy.getZoneName())`)
    // skips every zone-less tag policy and would not have reached 70.
    //
    // It was not a live fail-open, because that arm needs a non-empty
    // `securityZones`, which already refuses at the envelope. But the verdict was
    // protected by a mechanism the comment did not name, which is the
    // configuration that has hidden two defects in this package. A zoned tag
    // policy is itself evidence that a zone exists; refusing it here makes the
    // protection sit where the prose says it does.
    for construct in &policy.unmodelled {
        if matches!(
            construct,
            Unmodelled::ResourceValueExpressionMarker { .. } | Unmodelled::ZoneName(_)
        ) {
            why.push(UndecidableCause::PolicyConstruct {
                policy_id: policy.id,
                policy_name: policy.name.clone(),
                construct: construct.clone(),
            });
        }
    }

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
        if level.is_recursive {
            push(TagResourceProblem::Recursive);
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

    fn bare_tag(name: &str) -> ResolvedTag {
        ResolvedTag {
            name: name.to_string(),
            match_type: MatchType::Self_,
            attributes: BTreeMap::new(),
            options: BTreeSet::new(),
            validity_periods: 0,
            attached_through: TagServiceResource {
                levels: ["catalog", "schema", "table", "column"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                is_enabled: true,
            },
        }
    }

    /// `RangerTagForEval`'s identity, component by component.
    ///
    /// # ⚠ Why this is a unit test and not an assertion on a verdict
    ///
    /// **SP4d-2b Task 4, stated plainly because the alternative is to imply a
    /// coverage that does not exist.** Widening the dedup key from
    /// `(name, match_type)` to all five components changes NO decision the
    /// public API can return today: two members differing only in `attributes`
    /// produce the same candidate list (the same policies, through the same
    /// `resources.tag`), the arbitration reads a verdict and never a count, and
    /// `evaluate_tag_sub`'s ambiguity check already excludes a second candidate
    /// carrying the same policy by `std::ptr::eq`. So there is no verdict that
    /// distinguishes the narrow key from the wide one, and an integration test
    /// claiming to pin this would be measuring something else.
    ///
    /// What the widening buys is that the crate cannot start dropping a member
    /// the moment something DOES read the tag set per member — and what pins it
    /// is this test, which fails if any component leaves the key.
    #[test]
    fn the_tag_identity_is_all_five_components_ranger_compares() {
        let base = bare_tag("pii");

        let mut other_name = base.clone();
        other_name.name = "phi".to_string();

        let mut other_match = base.clone();
        other_match.match_type = MatchType::Ancestor;

        let mut other_attrs = base.clone();
        other_attrs
            .attributes
            .insert("level".to_string(), "3".to_string());

        let mut other_options = base.clone();
        other_options.options.insert("anything".to_string());

        let mut other_validity = base.clone();
        other_validity.validity_periods = 1;

        for (component, differing) in [
            ("type", other_name),
            ("matchType", other_match),
            ("attributes", other_attrs),
            ("options", other_options),
            ("validityPeriods", other_validity),
        ] {
            assert_ne!(
                base.identity(),
                differing.identity(),
                "`RangerTagForEval.equals` compares {component}, so two tags \
                 differing only there are TWO members of Ranger's set and this \
                 crate must not collapse them"
            );
        }

        // And the resource is NOT in it: Ranger's set member carries no service
        // resource, so putting one in the key would invent a member.
        let mut other_resource = base.clone();
        other_resource.attached_through = TagServiceResource {
            levels: ["catalog", "schema"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            is_enabled: false,
        };
        assert_eq!(
            base.identity(),
            other_resource.identity(),
            "`RangerTagForEval` carries no service resource, so two attachments \
             that produced the same match type are ONE member"
        );
    }
}
