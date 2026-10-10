//! Resource matching: does a policy's `resources` map cover the resource a
//! request names, and *how*?
//!
//! This reproduces `RangerDefaultPolicyResourceMatcher.getMatchType`
//! together with the two matchers it drives,
//! `RangerDefaultResourceMatcher.getMatchType` and
//! `RangerAbstractResourceMatcher` (`init`, `getMatcher`, `applyExcludes`).
//! Read from Apache Ranger at tag `release-ranger-2.8.0`, confirmed
//! byte-identical at `release-ranger-2.5.0` for all three files apart from a
//! `commons-lang` → `commons-lang3` import and plugin-context plumbing in
//! `createResourceMatcher`; `applyExcludes` was additionally disassembled
//! out of the real `ranger-plugins-common-2.5.0.jar` with `javap -c`.
//!
//! **Classification only.** This module says what *kind* of match a policy
//! has; it does not decide access. In particular
//! [`MatchType::Ancestor`] — a policy naming fewer hierarchy levels than
//! the request — is **not** an allow. Ranger's
//! `RangerDefaultPolicyEvaluator.evaluate` computes
//! `isMatched = matchType == SELF || matchType == SELF_AND_ALL_DESCENDANTS
//! || (matchType == ANCESTOR && request instanceof RangerTagAccessRequest)`
//! (plus `DESCENDANT` when the request's scope is `SELF_OR_DESCENDANTS`),
//! so an `ANCESTOR` match on a resource request never even reaches the
//! policy items. Measured live: a grant compiled to `{catalog, schema}`
//! produced `Access Denied: Cannot select from columns [v, id] in table or
//! view sibling` on real Trino, while the same grant carrying
//! `column: "*"` returned rows — see
//! `docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3 T1
//! and `crates/egret-perm-compile/src/grants.rs:227-254`. Treating an
//! ancestor as covering its descendants would fail **open**.
//!
//! Not reproduced, deliberately: `ResourceElementMatchingScope`
//! (`SELF_OR_PREFIX` / `SELF_OR_CHILD`) — every match here is taken at
//! Ranger's default `SELF` scope, which is what a Trino/StarRocks access
//! request uses; a request element holding a *collection* of values rather
//! than one string; and `RangerRequestExprResolver` token expansion inside
//! a policy value, which [`crate::model::Unmodelled::ResourceValueExpressionMarker`]
//! already reports per value.

use std::collections::BTreeMap;

use crate::model::{RangerPolicy, ResourceLevel};
use crate::servicedef::{PolicyType, ResourceDef, ServiceDef};

/// `RangerPolicyResourceMatcher.MatchType`, verbatim:
/// `enum MatchType { NONE, SELF, DESCENDANT, ANCESTOR, SELF_AND_ALL_DESCENDANTS }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchType {
    /// The policy does not cover this resource.
    None,
    /// The policy names exactly the resource the request names.
    Self_,
    /// The policy names a resource *below* the requested one — e.g. a
    /// policy on `catalog/schema/table` against a request for
    /// `catalog/schema`.
    Descendant,
    /// The policy names a resource *above* the requested one — e.g. a
    /// policy on `catalog/schema` against a request for
    /// `catalog/schema/table/column`. **Not an allow**: see the module doc.
    Ancestor,
    /// The policy names the requested resource and every resource under it,
    /// because every hierarchy level the policy names beyond the request's
    /// is match-any.
    SelfAndAllDescendants,
}

/// The resource one access request names: `catalog=hive, schema=sales,
/// table=orders, column=id`.
///
/// Ranger's `RangerAccessResource` allows a *collection* of values per
/// element; this models the one-string-per-element shape a Trino or
/// StarRocks request actually carries. An absent key is Ranger's `null`
/// (the level is not part of the request); a key present with `""` — or with
/// `"*"` — is a request for all values at that level, exactly as
/// [`requests_all_values`] treats it. **Which is why a caller that lets a human
/// choose the value has to refuse those two**: see that function's own
/// `# Why this is pub`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestResource {
    values: BTreeMap<String, String>,
}

impl RequestResource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set one resource element, builder-style.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.values.insert(name.into(), value.into());
        self
    }

    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.values.insert(name.into(), value.into());
    }

    /// Ranger's `RangerAccessResource.getValue(name)`.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Ranger's `RangerAccessResource.getKeys()`.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.values.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl FromIterator<(String, String)> for RequestResource {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        RequestResource {
            values: iter.into_iter().collect(),
        }
    }
}

/// Which of a policy's resource maps produced a [`ResourceMatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResourceMapSource {
    /// The policy's own `resources` map.
    Primary,
    /// `additionalResources[i]`.
    Additional(usize),
}

/// One resource map's verdict on a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceMatch {
    pub source: ResourceMapSource,
    pub match_type: MatchType,
}

/// How `policy_resources` matches `request`, for an **access** policy.
///
/// Reproduces `RangerDefaultPolicyResourceMatcher.getMatchType` after an
/// `init()` with `policyType = POLICY_TYPE_ACCESS`. For a masking or
/// row-filter policy — whose hierarchy comes from `dataMaskDef` /
/// `rowFilterDef` instead — use [`match_resource_of_type`].
pub fn match_resource(
    policy_resources: &BTreeMap<String, ResourceLevel>,
    request: &RequestResource,
    service_def: &ServiceDef,
) -> MatchType {
    match_resource_of_type(policy_resources, request, service_def, PolicyType::Access)
}

/// [`match_resource`] for an explicit policy type.
pub fn match_resource_of_type(
    policy_resources: &BTreeMap<String, ResourceLevel>,
    request: &RequestResource,
    service_def: &ServiceDef,
    policy_type: PolicyType,
) -> MatchType {
    // getMatchType: `policyResources.size() == 0 && resourceKeysSize == 0`
    // short-circuits to SELF before any hierarchy is looked up.
    if policy_resources.is_empty() && request.is_empty() {
        return MatchType::Self_;
    }

    let matcher = PolicyResourceMatcher::init(policy_resources, service_def, policy_type);
    matcher.match_type(request)
}

/// Every resource map of `policy` — its `resources`, then each
/// `additionalResources[i]` — matched independently against `request`, in
/// Ranger's own order.
///
/// `RangerAbstractPolicyEvaluator.init` builds one
/// `RangerDefaultPolicyResourceEvaluator` per map and
/// `RangerDefaultPolicyEvaluator.evaluate` loops over all of them, so an
/// additional map gets a full, independent chance to match — including
/// against a different hierarchy from the primary map's. Deciding what to
/// do with several verdicts is precedence, which is Task 5's job; this
/// returns all of them.
pub fn match_policy_resources(
    policy: &RangerPolicy,
    request: &RequestResource,
    service_def: &ServiceDef,
) -> Vec<ResourceMatch> {
    let policy_type = PolicyType::from_code(policy.policy_type);
    let mut out = vec![ResourceMatch {
        source: ResourceMapSource::Primary,
        match_type: match_resource_of_type(&policy.resources, request, service_def, policy_type),
    }];
    for (i, additional) in policy.additional_resources.iter().enumerate() {
        out.push(ResourceMatch {
            source: ResourceMapSource::Additional(i),
            match_type: match_resource_of_type(additional, request, service_def, policy_type),
        });
    }
    out
}

/// Whether Ranger's `RangerResourceTrie` — for a **tag** request, the ONLY
/// filter that decides which policies are evaluated — returns the same
/// evaluator set that [`match_resource_of_type`] does for `policy_value`.
///
/// # Why a tag request needs this and a resource request does not
///
/// For a resource request the trie is a pre-filter and
/// `RangerDefaultPolicyEvaluator.evaluate` then asks the matcher itself
/// (offsets 222-258 in 2.5.0, 207-243 in 2.8.0), so an over-returning trie
/// costs nothing. For a `RangerTagAccessRequest` that branch is **not taken**:
/// `evaluate` reads `((RangerTagAccessRequest) request).getMatchType()` and the
/// policy's own matcher is never consulted. **The trie IS the filter**, so
/// anywhere it disagrees with the matcher, the matcher is the wrong instrument.
///
/// # Measured, not reasoned about
///
/// Against the real `ranger-plugins-common` 2.5.0 on the `tag` service-def
/// (`wildCard: true`, `ignoreCase: false`), building a real
/// `RangerResourceTrie` over one `RangerDefaultPolicyResourceMatcher` per
/// policy value and comparing `getEvaluatorsForResource` with `getMatchType`:
///
/// | policy value | compiled shape | trie == matcher |
/// |---|---|---|
/// | `pii` | [`ValueMatcher::Equals`] | yes |
/// | `*`, `**` | match-any | yes |
/// | `p*`, `p**` | [`ValueMatcher::StartsWith`] | yes |
/// | `*i`, `*ii` | [`ValueMatcher::EndsWith`] | **no** — returned for `p`, `pi`, `pix`, `PII` and `""` |
/// | `p*x`, `pi?`, `pi*i`, `p?i` | [`ValueMatcher::Wildcard`] | **no** — `p*x` and `pi*i` are returned for `pii` |
///
/// The mechanism is one line of `RangerResourceTrie.insert`: a value is keyed
/// by its literal prefix up to the FIRST wildcard character and parked there as
/// a wildcard evaluator, collected for every resource whose path runs through
/// that node. For `Equals` that prefix is the whole value; for `StartsWith` it
/// is exactly the prefix `starts_with` tests; for a leading `*` it is the empty
/// string, i.e. the root, i.e. everything.
///
/// The direction is the reason this is a refusal and not a footnote: the trie
/// is WIDER, so a tag policy Ranger evaluates is one this crate would not —
/// and for a DENY policy that is fail-open.
///
/// # This classifies a VALUE; the two flags are the caller's
///
/// `isExcludes` and `isRecursive` sit on the `RangerPolicyResource`, not on any
/// one value, and each widens the trie on its own —
/// `RangerResourceTrie.insert` ends
/// `if (isWildcard || isRecursive) addWildcardEvaluator(...)`, and an excludes
/// resource lands in `getInheritedEvaluators()`. They are refused by
/// [`crate::eval::TagResourceProblem`], which is where the whole resource map
/// is in view. Reading this function as "the resource is reproducible" rather
/// than "this value is" is exactly the mistake that let `isRecursive` through
/// the first time.
///
/// # No `ignore_case` parameter, and why that is not an omission
///
/// The trie lower-cases both the inserted key and the looked-up value when the
/// resource's `optIgnoreCase` is set, and the matcher folds both sides too — so
/// the two move together and the AGREEMENT classification above is the same
/// either way. (The `tag` def sets `ignoreCase: false`, so on the tag path the
/// question does not arise at all.) The one place that reasoning could break is
/// a character whose lowercase form is longer than one code point, where a
/// folded trie key and a folded request value can disagree on length; no
/// identifier on either engine admits one — see [`fold`].
pub fn trie_reproduces_matcher(policy_value: &str, wild_card: bool) -> bool {
    if policy_value.is_empty() {
        // `RangerAbstractResourceMatcher.init` drops an empty policy value
        // before any matcher is built, which can make a level match-ANY when
        // it was the only value. Whether the trie does the same was not
        // measured, so it is refused rather than guessed at.
        //
        // ⚠ This used to add "and egret emits no empty tag value" as if that
        // bounded the shape. It bounded the TAG path only, and said nothing
        // about the resource path, where egret's own grant compiler DID emit
        // one: `perm.grants` accepted `schema_name = ''` and
        // `scope_to_resources` copied it through, which made the level
        // `isMatchAny` and enforced the grant across the whole catalog. SP4d
        // debt Task 12 closed that — `migrations/0178_perm_scope_levels_are_named.sql`
        // plus `egret_perm_compile::grants::ensure_scope_levels_are_named`, which
        // runs at the authoring boundary AND inside `compile_grant`. Stated by
        // code path now, not by authorship; see
        // `crate::eval::UndecidableCause::ResourceCandidacyNotReproducible`'s table.
        //
        // The OTHER match-any spelling the compiler could emit is a run of `*`,
        // and it does NOT reach this early return: `classify` sends it to the
        // `None` arm below, which the trie reproduces. SP4d debt Task 14
        // (`migrations/0179`, `ensure_a_grant_wildcard_level_has_nothing_named_beneath_it`)
        // refuses it only with a NAMED level beneath it — a level skip, not a trie
        // question — so a bare `*` at the deepest named level is still ordinary
        // compiler output and still handled there.
        return false;
    }
    match ValueMatcher::classify(policy_value, wild_card) {
        // Match-any: the trie parks it at the root and returns it for every
        // resource, which is exactly `is_match_any`.
        None => true,
        Some(ValueMatcher::Equals(_)) | Some(ValueMatcher::StartsWith(_)) => true,
        Some(ValueMatcher::EndsWith(_)) | Some(ValueMatcher::Wildcard(_)) => false,
    }
}

/// Whether ONE resource level's trie provably does not return a policy carrying
/// `level`, for the request value at that level.
///
/// The per-level half of Ranger's candidate filter.
/// `RangerResourceEvaluatorsRetriever.getEvaluators` intersects, over the keys
/// the REQUEST carries, the evaluator set each level's trie returns — so one
/// level answering "not in this trie" removes the policy from the whole answer,
/// whatever the other levels say. That makes absence provable locally, which is
/// the only way it is provable at all: a policy that declares FEWER levels than
/// the request is added to the missing levels' tries as an *inherited*
/// evaluator (`isAncestorOf`) and is unioned into every lookup there, so nothing
/// can be concluded from a level the policy does not declare.
///
/// Three conditions, and all three are about this level only:
///
/// * **no `isExcludes`** — `RangerResourceTrie`'s build loop sends an excludes
///   resource to `addInheritedEvaluator` for THIS level, i.e. it is returned for
///   every value of it;
/// * **no `isRecursive`** — `insert` ends
///   `if (isWildcard || isRecursive) addWildcardEvaluator(...)`, so the value
///   becomes a prefix match in the trie while the matcher still reads it exactly;
/// * **every value trie-prefix-exact** ([`trie_reproduces_matcher`]) — otherwise
///   the trie parks the value at a shorter prefix than the matcher tests, and a
///   matcher non-match is not a trie non-match.
///
/// With those, this level's trie membership IS [`LevelMatcher::is_match`], and a
/// non-match is a proof of absence. Measured per row in
/// `tests/ranger/src/test/java/EgretResourceTrieCandidacyTest.java`.
pub fn level_excluded_by_trie(
    level: &ResourceLevel,
    def: Option<&ResourceDef>,
    request_value: &str,
) -> bool {
    // ⚠ **KNOWN GAP, measured and NOT closed: an EMPTY request value.**
    //
    // SP4d-2b Task 5, fix round 1. `RangerAbstractResourceMatcher.isAllValuesRequested`
    // treats `""` and `*` alike and the MATCHER short-circuits on it before reading
    // a policy value, so a policy value that is not match-any answers `false`
    // below and this function reports the policy ABSENT. The trie does not treat
    // the two alike, and it does not agree with the matcher for `""`.
    //
    // MEASURED on ranger 2.8.0, one fixture with only the request changed (a tag
    // deny at priority 0, a deny-capable allower at priority 1, and a policy whose
    // only level is the literal `no_such_value_at_all` at priority 0):
    //
    // | request | Ranger | what this function says |
    // |---|---|---|
    // | `{catalog: ""}` | reaches the literal policy → `Allowed` | absent ✗ |
    // | `{catalog: "*"}` | does not → `Denied` (the restore runs) | absent ✓ |
    // | `{catalog: "", database: …, table: …, column: …}` | does NOT → `Denied` | absent ✓ |
    //
    // So the first row is wrong and the third is right, and **the rule that
    // separates them is not established**: `RangerResourceEvaluatorsRetriever`
    // intersects over the request's keys and a policy declaring fewer levels is an
    // *inherited* evaluator in the deeper ones, so the depth of the request is in
    // it somewhere. A guard on "the request value is empty" fixes row 1 and breaks
    // row 3 — measured, 7 disagreements at the recorded seed — so it is not
    // written here. **A rule fitted to three rows would be a guess, and this
    // function's whole job is to be a proof.**
    //
    // Why it is deferred rather than refused: the only direction measured is
    // fail-CLOSED (the crate restores a tag deny where Ranger allows), and no
    // egret CALLER can now hand this function an empty level.
    //
    // ⚠ **That bound used to be stated by authorship and it was false.** It read:
    // *"an empty REQUEST level is not something egret sends — `perm_compile`'s
    // `request_resource` builds concrete values from the target"*.
    // `request_resource` copies `target.schema`/`table`/`column` through
    // VERBATIM, and the target is an HTTP body, so every `policy:read` caller of
    // `POST /api/control-plane/perm/explain` sent an empty level by typing one.
    // Nothing in the browser produced it (`ScopePicker` maps its own empty input
    // to `undefined`), which is exactly why the claim read as true.
    //
    // The bound is now a NAMED FUNCTION rather than a habit:
    // `egret_perm_compile::routes::validate_explain_target` 400s a level present
    // as `""` or `"*"`, keyed to this module's own `isAllValuesRequested`
    // predicate, and `scope_to_resources`' `Access` arm fills unnamed levels with
    // `*`, not `""`. It remains reachable from the corpus and from a hand-built
    // request, which is why the gap is written down here and in
    // `tests/ranger/README.md` rather than left in a report. `restore_duel` keeps
    // its root value concrete so the gate does not carry a standing disagreement.

    if level.is_excludes || level.is_recursive {
        return false;
    }
    let wild_card = def.map(|d| d.matcher_options.wild_card).unwrap_or(true);
    if !level
        .values
        .iter()
        .all(|v| trie_reproduces_matcher(v, wild_card))
    {
        return false;
    }
    !LevelMatcher::build(level, def).is_match(Some(request_value))
}

/// `RangerAbstractResourceMatcher.isAllValuesRequested`: does this request
/// element ask for **every value at its level** rather than naming one?
///
/// ```text
/// boolean isAllValuesRequested(String resource) {
///     return StringUtils.isEmpty(resource) || WILDCARD_ASTERISK.equals(resource);
/// }
/// ```
///
/// `null`, `""` and `"*"` alike — `isEmpty` covers the first two — and the three
/// are **not interchangeable everywhere else**, which is why this is one function
/// and not three comparisons at each use. [`LevelMatcher::is_match`] short-circuits
/// on it *before reading any policy value*
/// (`if (isMatchAny || (allValuesRequested && !isPrefixMatch))`), and
/// [`LevelMatcher::apply_excludes`] keys its carve-out to the same flag, so losing
/// an arm flips a non-match to a **match** on an excludes policy — the fail-open
/// direction.
///
/// # Why this is `pub`
///
/// **`egret_perm_compile::routes::validate_explain_target` is the second guard
/// that has to agree with this rule**, and it 400s an `explain` target whose level
/// is present as `""` or `"*"` precisely because no engine request carries one.
/// It used to spell the predicate out again in its own crate, with the agreement
/// kept by a pair of tests that named each other. Two guards that must agree,
/// written to different shapes, is a defect this programme has measured four
/// times; exporting the rule makes the pairing structural instead of remembered
/// (SP4d debt Task 4 raised it as a concern and its fix round settled it this way).
///
/// So this function is the rule, not a copy of it: `is_match` above calls it too,
/// and there is no second spelling left to drift.
///
/// **A caller that only means a PRESENT value must test presence itself** — the
/// `None` arm is deliberately part of the predicate (an absent request element is
/// Ranger's `null`, and `isEmpty(null)` is true), and for `explain` an absent
/// *tail* level is a legitimate question answered by its parent. That handler
/// therefore unwraps first and asks about the value it has.
pub fn requests_all_values(resource_value: Option<&str>) -> bool {
    match resource_value {
        None => true,
        Some(v) => v.is_empty() || v == "*",
    }
}

// ── The matcher itself ───────────────────────────────────────────────────

/// One initialised `RangerDefaultPolicyResourceMatcher`.
struct PolicyResourceMatcher<'a> {
    service_def: &'a ServiceDef,
    policy_type: PolicyType,
    policy_resources: &'a BTreeMap<String, ResourceLevel>,
    /// `allMatchers`. Empty when `init()` failed.
    matchers: BTreeMap<String, LevelMatcher>,
    /// `validResourceHierarchy`: set only when exactly one hierarchy
    /// containing all policy keys has no gaps in them.
    valid_hierarchy: Option<Vec<String>>,
    /// `init()` failed, which in Ranger nulls both `allMatchers` and
    /// `serviceDefHelper` — and a null helper makes `getMatchingHierarchy`
    /// return null for every request, i.e. `MatchType.NONE`.
    init_failed: bool,
}

impl<'a> PolicyResourceMatcher<'a> {
    /// `RangerDefaultPolicyResourceMatcher.init()`.
    fn init(
        policy_resources: &'a BTreeMap<String, ResourceLevel>,
        service_def: &'a ServiceDef,
        policy_type: PolicyType,
    ) -> Self {
        let mut me = PolicyResourceMatcher {
            service_def,
            policy_type,
            policy_resources,
            matchers: BTreeMap::new(),
            valid_hierarchy: None,
            init_failed: true,
        };
        if policy_resources.is_empty() {
            return me;
        }

        // `serviceDefHelper.getResourceHierarchies(policyType, policyResources.keySet())`
        let candidates: Vec<&Vec<String>> = service_def
            .hierarchies(policy_type)
            .iter()
            .filter(|h| policy_resources.keys().all(|k| h.contains(k)))
            .collect();

        // ... then keep only those with no *gaps* in the policy's levels
        // (`isHierarchyValidForResources`). Exactly one ⇒ that is the
        // policy's hierarchy; more than one ⇒ Ranger deliberately leaves
        // `validResourceHierarchy` null and re-derives per request.
        let valid: Vec<&Vec<String>> = candidates
            .iter()
            .copied()
            .filter(|h| has_no_gaps(h, |name| policy_resources.contains_key(name)))
            .collect();
        if valid.is_empty() {
            return me;
        }
        if valid.len() == 1 {
            me.valid_hierarchy = Some(valid[0].clone());
        }

        // One matcher per resource named by the policy that appears in some
        // candidate hierarchy. A policy key outside every candidate cannot
        // exist here: every candidate contains all the policy's keys.
        for hierarchy in &candidates {
            for name in *hierarchy {
                if me.matchers.contains_key(name) {
                    continue;
                }
                let Some(level) = policy_resources.get(name) else {
                    continue;
                };
                let def = service_def.resource(policy_type, name);
                me.matchers
                    .insert(name.clone(), LevelMatcher::build(level, def));
            }
        }
        me.init_failed = false;
        me
    }

    /// `RangerDefaultPolicyResourceMatcher.getMatchType(resource, ...)`.
    fn match_type(&self, request: &RequestResource) -> MatchType {
        let resource_keys_size = request.len();
        if self.policy_resources.is_empty() && resource_keys_size == 0 {
            return MatchType::Self_;
        }

        let Some(hierarchy) = self.matching_hierarchy(request) else {
            return MatchType::None;
        };

        // The prefix of the hierarchy that the policy actually has matchers
        // for, and the index of the last one that is not match-any.
        let mut last_non_any: i64 = -1;
        let mut index: usize = 0;
        for name in &hierarchy {
            match self.matchers.get(name) {
                Some(m) => {
                    if !m.is_match_any {
                        last_non_any = index as i64;
                    }
                    index += 1;
                }
                None => break,
            }
        }

        let mut ret = if resource_keys_size == 0 {
            MatchType::Self_
        } else {
            MatchType::None
        };

        for name in &hierarchy {
            let value = request.get(name);
            match self.matchers.get(name) {
                Some(m) => {
                    // A level the request does not name is skipped unless
                    // the policy's matcher is match-any — leaving `ret`
                    // untouched, which is how a policy that names MORE
                    // levels than the request still ends up SELF here and
                    // is demoted to DESCENDANT by the tail below.
                    if value.is_some() || m.is_match_any {
                        if m.is_match(value) {
                            ret = MatchType::Self_;
                        } else {
                            ret = MatchType::None;
                            break;
                        }
                    }
                }
                None => {
                    if value.is_some() {
                        // More resource-values than matchers: the policy
                        // sits ABOVE the requested resource.
                        ret = MatchType::Ancestor;
                    }
                    break;
                }
            }
        }

        if ret == MatchType::Self_ && resource_keys_size < self.policy_resources.len() {
            // More matchers than resource-values.
            ret = if (resource_keys_size as i64) > last_non_any {
                // Every remaining matcher is match-any.
                MatchType::SelfAndAllDescendants
            } else {
                MatchType::Descendant
            };
        }

        ret
    }

    /// `getMatchingHierarchy(RangerAccessResource)`.
    fn matching_hierarchy(&self, request: &RequestResource) -> Option<Vec<String>> {
        if self.init_failed {
            // Ranger nulls `serviceDefHelper` on a failed init, and every
            // branch below needs it.
            return None;
        }

        if request.is_empty() {
            return self
                .valid_hierarchy
                .clone()
                .or_else(|| self.shortest_hierarchy_containing(self.policy_resources.keys()));
        }

        let candidate = match &self.valid_hierarchy {
            Some(vh) => {
                // `hierarchyHasAllResources(validResourceHierarchy, resourceKeySet)`
                if request.keys().all(|k| vh.iter().any(|n| n == k)) {
                    Some(vh.clone())
                } else {
                    None
                }
            }
            None => {
                let policy_keys: Vec<&str> =
                    self.policy_resources.keys().map(String::as_str).collect();
                if request.keys().all(|k| policy_keys.contains(&k)) {
                    self.shortest_hierarchy_containing(self.policy_resources.keys())
                } else if policy_keys.iter().all(|k| request.get(k).is_some()) {
                    self.shortest_hierarchy_containing(request.keys())
                } else {
                    None
                }
            }
        };

        // `isHierarchyValidForResources(aValidHierarchy, resource.getAsMap())`
        candidate.filter(|h| has_no_gaps(h, |name| request.get(name).is_some()))
    }

    /// `getMatchingHierarchy(Set<String> resourceKeys)`: among the
    /// hierarchies containing every one of `keys`, the shortest.
    fn shortest_hierarchy_containing<K: AsRef<str>>(
        &self,
        keys: impl IntoIterator<Item = K>,
    ) -> Option<Vec<String>> {
        let keys: Vec<String> = keys.into_iter().map(|k| k.as_ref().to_string()).collect();
        if keys.is_empty() {
            return None;
        }
        self.service_def
            .hierarchies(self.policy_type)
            .iter()
            .filter(|h| keys.iter().all(|k| h.contains(k)))
            .min_by_key(|h| h.len())
            .cloned()
    }
}

/// `isHierarchyValidForResources`: walking the hierarchy root-to-leaf, once
/// a level is absent no deeper level may be present. A gap means the
/// resource map does not describe a contiguous path down the tree.
fn has_no_gaps(hierarchy: &[String], present: impl Fn(&str) -> bool) -> bool {
    let mut skipped = false;
    for name in hierarchy {
        if present(name) {
            if skipped {
                return false;
            }
        } else {
            skipped = true;
        }
    }
    true
}

// ── One hierarchy level's matcher ────────────────────────────────────────

/// `RangerDefaultResourceMatcher` + the `RangerAbstractResourceMatcher`
/// state it reads.
struct LevelMatcher {
    value_matchers: Vec<ValueMatcher>,
    /// `isMatchAny`: no usable policy values at all, or one of them is a
    /// pure run of `*` (on a resource whose def enables wildcards).
    is_match_any: bool,
    is_excludes: bool,
    ignore_case: bool,
}

impl LevelMatcher {
    /// `RangerAbstractResourceMatcher.init()` + `buildResourceMatchers()`.
    fn build(level: &ResourceLevel, def: Option<&ResourceDef>) -> LevelMatcher {
        let options = def.map(|d| d.matcher_options.clone()).unwrap_or_default();
        let mut value_matchers = Vec::new();
        for value in &level.values {
            // `init()` drops empty policy values before any matcher is built.
            if value.is_empty() {
                continue;
            }
            match ValueMatcher::classify(value, options.wild_card) {
                // `buildResourceMatchers`: a match-any value clears every
                // other matcher and stops — the level matches everything.
                None => {
                    value_matchers.clear();
                    break;
                }
                Some(m) => value_matchers.push(m),
            }
        }
        LevelMatcher {
            is_match_any: value_matchers.is_empty(),
            value_matchers,
            is_excludes: level.is_excludes,
            ignore_case: options.ignore_case,
        }
    }

    /// `RangerDefaultResourceMatcher.getMatchType(...)` reduced to the
    /// `SELF` matching scope, then `applyExcludes`.
    fn is_match(&self, resource_value: Option<&str>) -> bool {
        let all_values_requested = requests_all_values(resource_value);

        // `if (isMatchAny || (allValuesRequested && !isPrefixMatch)) { ret = isMatchAny ? SELF : NONE; }`
        // — `isPrefixMatch` is false at SELF scope, so a request for ALL
        // values against a policy that does not allow all values is NONE,
        // without consulting a single policy value.
        let without_excludes = if self.is_match_any || all_values_requested {
            self.is_match_any
        } else {
            let value = resource_value.unwrap_or_default();
            self.value_matchers
                .iter()
                .any(|m| m.is_match(value, self.ignore_case))
        };

        self.apply_excludes(all_values_requested, without_excludes)
    }

    /// `RangerAbstractResourceMatcher.applyExcludes`, verified against the
    /// bytecode in `ranger-plugins-common-2.5.0.jar`:
    ///
    /// ```text
    /// if (!policyIsExcludes)                 return resultWithoutExcludes;
    /// if (allValuesRequested && !isMatchAny) return resultWithoutExcludes;
    /// return !resultWithoutExcludes;
    /// ```
    ///
    /// `isExcludes` is therefore NOT plain negation. The carve-out — the
    /// request asks for every value while the policy does not allow every
    /// value — leaves the result alone, so an excludes policy still fails
    /// to match a request for `*`. Both halves of the guard matter: drop
    /// `allValuesRequested` and a normal named-value match stops flipping;
    /// drop `!isMatchAny` and an excludes-everything policy stops flipping
    /// its `*` request to NONE.
    fn apply_excludes(&self, all_values_requested: bool, without_excludes: bool) -> bool {
        if !self.is_excludes {
            return without_excludes;
        }
        if all_values_requested && !self.is_match_any {
            return without_excludes;
        }
        !without_excludes
    }
}

// ── One policy value's matcher ───────────────────────────────────────────

/// The four shapes `RangerAbstractResourceMatcher.getMatcher` compiles a
/// policy value into. (Ranger's quoted-case-sensitive variants are folded
/// into the plain ones; `ServiceDefIssue::QuotedCaseSensitive` reports the
/// difference.)
#[derive(Debug, Clone, PartialEq, Eq)]
enum ValueMatcher {
    /// `CaseSensitiveStringMatcher` / `CaseInsensitiveStringMatcher`.
    Equals(String),
    /// `*Sensitive/InsensitiveStartsWithMatcher` — the prefix before a
    /// trailing run of `*`.
    StartsWith(String),
    /// `*EndsWithMatcher` — the suffix after a leading run of `*`.
    EndsWith(String),
    /// `*WildcardMatcher`, i.e. `FilenameUtils.wildcardMatch`.
    Wildcard(String),
}

impl ValueMatcher {
    /// `RangerAbstractResourceMatcher.getMatcher(policyValue)`.
    /// `None` means the value is match-any (`"*"`, `"**"`, …), which Ranger
    /// expresses as an ends-with matcher on the empty string whose
    /// `isMatchAny()` is true.
    ///
    /// Byte indices are used where Ranger uses UTF-16 char indices. That is
    /// safe and equivalent: the only characters ever indexed are the ASCII
    /// `*`, and two `*` are adjacent by byte index exactly when they are
    /// adjacent by char index, and a `*` is the final byte exactly when it
    /// is the final char.
    fn classify(value: &str, wild_card: bool) -> Option<ValueMatcher> {
        let bytes = value.as_bytes();
        let len = bytes.len();
        let mut star_start: Option<usize> = None;
        let mut star_end: Option<usize> = None;
        let mut need_wildcard = false;

        if wild_card {
            for (i, b) in bytes.iter().enumerate() {
                match b {
                    b'?' => {
                        need_wildcard = true;
                        break;
                    }
                    b'*' => {
                        // A `*` continues the current run, or starts the
                        // first one; a second, non-contiguous run means a
                        // full wildcard match is needed.
                        if star_end.is_none() || star_end == Some(i.wrapping_sub(1)) {
                            star_end = Some(i);
                            star_start.get_or_insert(i);
                        } else {
                            need_wildcard = true;
                            break;
                        }
                    }
                    _ => {}
                }
            }
        }

        if need_wildcard {
            return Some(ValueMatcher::Wildcard(value.to_string()));
        }
        match (star_start, star_end) {
            // `test`, `testa` — no wildcard at all.
            (None, _) => Some(ValueMatcher::Equals(value.to_string())),
            // `*test`, `**test` — and, when nothing follows the run,
            // `*` / `**`, which is match-any.
            (Some(0), Some(end)) => {
                let suffix = &value[end + 1..];
                if suffix.is_empty() {
                    None
                } else {
                    Some(ValueMatcher::EndsWith(suffix.to_string()))
                }
            }
            (Some(start), Some(end)) => {
                if end != len - 1 {
                    // `test*a` — the run is in the middle.
                    Some(ValueMatcher::Wildcard(value.to_string()))
                } else {
                    // `test*`, `test**`.
                    Some(ValueMatcher::StartsWith(value[..start].to_string()))
                }
            }
            (Some(_), None) => unreachable!("star_start is only set together with star_end"),
        }
    }

    fn is_match(&self, resource_value: &str, ignore_case: bool) -> bool {
        match self {
            ValueMatcher::Equals(p) => {
                if ignore_case {
                    fold(resource_value) == fold(p)
                } else {
                    resource_value == p
                }
            }
            ValueMatcher::StartsWith(p) => {
                if ignore_case {
                    fold(resource_value).starts_with(&fold(p))
                } else {
                    resource_value.starts_with(p)
                }
            }
            ValueMatcher::EndsWith(p) => {
                if ignore_case {
                    fold(resource_value).ends_with(&fold(p))
                } else {
                    resource_value.ends_with(p)
                }
            }
            // NOT folded before matching, unlike the three above:
            // `wildcard_match` does index arithmetic against the text, and a
            // whole-string `fold` can change its LENGTH (`İ` lowercases to two
            // code points), which would shift every index away from the ones
            // Java computes. It takes `ignore_case` and compares per position
            // instead, which is also what `IOCase.checkRegionMatches` does.
            ValueMatcher::Wildcard(p) => wildcard_match(p, resource_value, ignore_case),
        }
    }
}

/// Case folding for `ignoreCase: true`.
///
/// Java folds with `String.equalsIgnoreCase` / `IOCase.INSENSITIVE`, which
/// compare per UTF-16 unit via `toUpperCase` then `toLowerCase`; this folds
/// the whole string with Unicode lowercasing. The two differ only for
/// characters whose lowercase form is longer than one code point (e.g. `İ`),
/// which no catalog/schema/table/column identifier on either engine uses.
fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// One token of a wildcard pattern, as `FilenameUtils.splitOnTokens` produces
/// them: the `String[]` it returns holds the one-character strings `"?"` and
/// `"*"` plus maximal runs of everything else.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// `"*"` — any run of characters, including none. Consecutive `*` in the
    /// pattern collapse into ONE of these.
    Any,
    /// `"?"` — exactly one character. These do NOT collapse.
    One,
    Literal(Vec<char>),
}

/// `FilenameUtils.splitOnTokens`, read from `javap -c` of
/// `commons-io-2.8.0.jar` and confirmed instruction-for-instruction identical
/// in `commons-io-2.16.1.jar` (see [`wildcard_match`] for which jar each
/// engine links).
///
/// ```java
/// if (text.indexOf('?') == -1 && text.indexOf('*') == -1) return new String[]{text};
/// char[] array = text.toCharArray();
/// ArrayList<String> list = new ArrayList<>();
/// StringBuilder buffer = new StringBuilder();
/// char prevChar = 0;
/// for (char ch : array) {
///     if (ch == '?' || ch == '*') {
///         if (buffer.length() != 0) { list.add(buffer.toString()); buffer.setLength(0); }
///         if (ch == '?') { list.add("?"); }
///         else if (prevChar != '*') { list.add("*"); }
///     } else { buffer.append(ch); }
///     prevChar = ch;
/// }
/// if (buffer.length() != 0) list.add(buffer.toString());
/// ```
///
/// `prevChar` is the previous character of the PATTERN, not the previous
/// token, so `**` collapses but `*?*` keeps both stars.
fn split_on_tokens(pattern: &[char]) -> Vec<Token> {
    if !pattern.contains(&'?') && !pattern.contains(&'*') {
        return vec![Token::Literal(pattern.to_vec())];
    }
    let mut tokens = Vec::new();
    let mut buffer: Vec<char> = Vec::new();
    let mut prev_char = '\u{0}';
    for &ch in pattern {
        if ch == '?' || ch == '*' {
            if !buffer.is_empty() {
                tokens.push(Token::Literal(std::mem::take(&mut buffer)));
            }
            if ch == '?' {
                tokens.push(Token::One);
            } else if prev_char != '*' {
                tokens.push(Token::Any);
            }
        } else {
            buffer.push(ch);
        }
        prev_char = ch;
    }
    if !buffer.is_empty() {
        tokens.push(Token::Literal(buffer));
    }
    tokens
}

/// `IOCase.checkRegionMatches(str, strStartIndex, search)`, i.e.
/// `str.regionMatches(ignoreCase, strStartIndex, search, 0, search.length())`.
fn check_region_matches(text: &[char], start: usize, search: &[char], ignore_case: bool) -> bool {
    if start > text.len() || text.len() - start < search.len() {
        return false;
    }
    text[start..start + search.len()]
        .iter()
        .zip(search)
        .all(|(a, b)| chars_equal(*a, *b, ignore_case))
}

/// `IOCase.checkIndexOf(str, strStartIndex, search)`:
///
/// ```java
/// int endIndex = str.length() - search.length();
/// if (endIndex >= strStartIndex) {
///     for (int i = strStartIndex; i <= endIndex; i++) {
///         if (checkRegionMatches(str, i, search)) return i;
///     }
/// }
/// return -1;
/// ```
///
/// `None` is Java's `-1`.
fn check_index_of(
    text: &[char],
    start: usize,
    search: &[char],
    ignore_case: bool,
) -> Option<usize> {
    let end = text.len().checked_sub(search.len())?;
    if end < start {
        return None;
    }
    (start..=end).find(|&i| check_region_matches(text, i, search, ignore_case))
}

/// One character compared the way `String.regionMatches(true, …)` does.
///
/// Java tries `c1 == c2`, then `toUpperCase(c1) == toUpperCase(c2)`, then
/// `toLowerCase(toUpperCase(c1)) == toLowerCase(toUpperCase(c2))` — all
/// single-character mappings. This tries `a == b` and then one lowercase
/// step, which is NOT the same.
///
/// The difference was measured rather than reasoned about, and measured
/// EXHAUSTIVELY the second time: over every BMP code point this JDK defines,
/// by two independent instruments (Java's fold key
/// `toLowerCase(toUpperCase(c))` grouped over all 62034 of them, and the real
/// `String.regionMatches(true, …)` brute-forced over every ordered pair of
/// the 2423 code points either language treats as cased). Both give
/// **104 disagreeing ordered pairs in 21 classes, and in every one Java is
/// `true` while this is `false`. There are none the other way.**
///
/// **This is strictly NARROWER than Java, never wider** — an earlier version
/// of this comment said the opposite, which mattered because the direction IS
/// the safety argument. Narrower means egret finds no match where Ranger
/// finds one: fail-CLOSED for an allow rule, and fail-OPEN for a deny or an
/// `isExcludes` rule, which is the direction that costs something.
///
/// A second earlier version said "exactly two classes … 22 disagreements".
/// That was the honest answer to a 1922-pair hand-picked cross product: the
/// claim was simply wider than the measurement behind it. The full set, its
/// size and its direction are now pinned in
/// [`tests::case_folding_is_narrower_than_java_on_104_ordered_pairs_in_21_classes`],
/// which enumerates every class, so the README and the code cannot drift
/// apart.
///
/// The 21 classes, by mechanism:
///
/// 1. **Dotted and dotless I** (U+0049 U+0069 U+0130 U+0131, 10 pairs).
///    Java's third step maps `İ` to a single `i`; Rust's
///    `char::to_lowercase` yields TWO code points for it, so no
///    single-character comparison can reach Java's answer.
/// 2. **The other twenty are one mechanism**: a missing UPPERCASE step.
///    `toUpperCase('ς') == toUpperCase('σ') == 'Σ'`, while lowercasing
///    leaves them distinct. That shape covers final sigma `ς`, the long s
///    `ſ`, the micro sign `µ`, iota subscript U+0345 / U+1FBE, the Greek
///    symbol variants `ϐ ϵ ϑ ϴ ϰ ϖ ϱ ϕ`, the nine Cyrillic historic shapes
///    U+1C80-U+1C88, and `ẛ` (U+1E9B).
///
/// Adding a `to_uppercase` step would close **96 of the 104** — measured by
/// making that change and re-running the test below, which then reports 8.
/// The 8 survivors are the six pairs involving `İ` (whose Rust lowercase is
/// TWO code points, so no single-character step reaches Java's answer) and
/// `ϑ` vs `ϴ`. It is left alone here because these rounds correct the claim
/// rather than the semantics — **no catalog/schema/database/table/column
/// identifier on either engine admits any of these code points.**
fn chars_equal(a: char, b: char, ignore_case: bool) -> bool {
    if a == b {
        return true;
    }
    if !ignore_case {
        return false;
    }
    a.to_lowercase().eq(b.to_lowercase())
}

/// `FilenameUtils.wildcardMatch(filename, wildcardMatcher, caseSensitivity)`,
/// ported token-for-token rather than approximated.
///
/// **This is NOT a classic greedy wildcard matcher**, and assuming it was is
/// what the differential gate caught (SP4d-1 Task 7, finding F1, fail-open).
/// A `*` token sets `anyChars` and otherwise advances nothing; the text index
/// moves only when a LITERAL token is consumed (`checkIndexOf` skips ahead to
/// the next occurrence) or when the `*` is the pattern's LAST token (it then
/// jumps straight to the end). So a `*` followed by a `?` consumes **nothing**:
///
/// ```text
/// wildcardMatch("HIVE", "?*?", INSENSITIVE) == false   (a greedy matcher: true)
/// wildcardMatch("HIVE", "*?",  INSENSITIVE) == false   (a greedy matcher: true)
/// wildcardMatch("ab",   "?*?", INSENSITIVE) == true
/// ```
///
/// Backtracking exists but only for literal tokens: when a literal is found
/// after a `*`, the NEXT occurrence of that same literal is pushed as a
/// resume point. One resume point per literal occurrence, not a full search.
///
/// ```java
/// String[] wcs = splitOnTokens(wildcardMatcher);
/// boolean anyChars = false; int textIdx = 0, wcsIdx = 0;
/// Deque<int[]> backtrack = new ArrayDeque<>(wcs.length);
/// do {
///     if (!backtrack.isEmpty()) { int[] a = backtrack.pop(); wcsIdx = a[0]; textIdx = a[1]; anyChars = true; }
///     while (wcsIdx < wcs.length) {
///         if (wcs[wcsIdx].equals("?")) {
///             textIdx++;
///             if (textIdx > filename.length()) break;
///             anyChars = false;
///         } else if (wcs[wcsIdx].equals("*")) {
///             anyChars = true;
///             if (wcsIdx == wcs.length - 1) textIdx = filename.length();
///         } else {
///             if (anyChars) {
///                 textIdx = caseSensitivity.checkIndexOf(filename, textIdx, wcs[wcsIdx]);
///                 if (textIdx == -1) break;
///                 int repeat = caseSensitivity.checkIndexOf(filename, textIdx + 1, wcs[wcsIdx]);
///                 if (repeat >= 0) backtrack.push(new int[]{wcsIdx, repeat});
///             } else if (!caseSensitivity.checkRegionMatches(filename, textIdx, wcs[wcsIdx])) {
///                 break;
///             }
///             textIdx += wcs[wcsIdx].length();
///             anyChars = false;
///         }
///         wcsIdx++;
///     }
///     if (wcsIdx == wcs.length && textIdx == filename.length()) return true;
/// } while (!backtrack.isEmpty());
/// return false;
/// ```
///
/// Read from `javap -c org.apache.commons.io.FilenameUtils` on
/// **commons-io 2.8.0**, which `mvn dependency:build-classpath` resolves for
/// `ranger-plugins-common:2.5.0` (Trino 470's version), and confirmed
/// identical in **commons-io 2.16.1**, which the same command resolves for
/// `ranger-plugins-common:2.8.0` (StarRocks 4.1's). `splitOnTokens` and
/// `wildcardMatch` disassemble to the same instruction sequence in both, and
/// a 3348-pair cross product produced byte-identical output from the two —
/// so the exact commons-io version does not change any answer here.
///
/// Character indices stand in for Java's UTF-16 indices. They differ only for
/// astral characters, which no engine's resource vocabulary admits.
fn wildcard_match(pattern: &str, text: &str, ignore_case: bool) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let wcs = split_on_tokens(&pattern);

    let mut any_chars = false;
    let mut text_idx: usize = 0;
    let mut wcs_idx: usize = 0;
    let mut backtrack: Vec<(usize, usize)> = Vec::with_capacity(wcs.len());

    loop {
        if let Some((w, t)) = backtrack.pop() {
            wcs_idx = w;
            text_idx = t;
            any_chars = true;
        }

        while wcs_idx < wcs.len() {
            match &wcs[wcs_idx] {
                Token::One => {
                    text_idx += 1;
                    if text_idx > text.len() {
                        break;
                    }
                    any_chars = false;
                }
                Token::Any => {
                    any_chars = true;
                    if wcs_idx == wcs.len() - 1 {
                        text_idx = text.len();
                    }
                }
                Token::Literal(literal) => {
                    if any_chars {
                        match check_index_of(&text, text_idx, literal, ignore_case) {
                            None => break,
                            Some(found) => {
                                text_idx = found;
                                if let Some(repeat) =
                                    check_index_of(&text, text_idx + 1, literal, ignore_case)
                                {
                                    backtrack.push((wcs_idx, repeat));
                                }
                            }
                        }
                    } else if !check_region_matches(&text, text_idx, literal, ignore_case) {
                        break;
                    }
                    text_idx += literal.len();
                    any_chars = false;
                }
            }
            wcs_idx += 1;
        }

        if wcs_idx == wcs.len() && text_idx == text.len() {
            return true;
        }
        if backtrack.is_empty() {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **`isAllValuesRequested` is a three-value predicate, and a SECOND guard
    /// CALLS it from another crate.** `egret_perm_compile::routes::validate_explain_target`
    /// 400s an `explain` target whose level is present as `""` or `"*"`, and since
    /// SP4d debt Task 4's fix round (concern 3, overruled) it does so through
    /// [`requests_all_values`] itself — not through a second spelling. So the two
    /// guards cannot drift, and what this test adds is the thing a shared function
    /// still cannot give: **that the flag has an observable effect at all**, and in
    /// which direction losing an arm fails.
    ///
    /// ⚠ The earlier version of this comment said the two "cannot be mechanically
    /// coupled across a crate boundary". That was a claim about this crate's public
    /// surface, not about the rule, and it was answered by exporting the rule.
    ///
    /// Asserted through the observable consequence rather than the flag: against a
    /// literal policy value, Ranger's
    /// `if (isMatchAny || (allValuesRequested && !isPrefixMatch))` short-circuits
    /// to NONE *without reading a single policy value*, so all three forms fail to
    /// match while the literal itself matches. The `is_match_any` row is what makes
    /// the short-circuit visible as a short-circuit: there the SAME three forms
    /// match, which an ordinary value comparison could not produce.
    #[test]
    fn all_values_requested_is_null_empty_or_star_and_pairs_with_explains_target_guard() {
        let literal = ResourceLevel {
            values: vec!["sales".to_string()],
            is_excludes: false,
            is_recursive: false,
        };
        let m = LevelMatcher::build(&literal, None);
        assert!(m.is_match(Some("sales")), "the literal itself matches");
        for form in [None, Some(""), Some("*")] {
            assert!(
                !m.is_match(form),
                "{form:?} is isAllValuesRequested, so a policy that does not allow all \
                 values is NONE without any value being read — and this is the shape \
                 `egret_perm_compile::routes::validate_explain_target` refuses in an \
                 `explain` target"
            );
        }

        let any = ResourceLevel {
            values: vec!["*".to_string()],
            is_excludes: false,
            is_recursive: false,
        };
        let m = LevelMatcher::build(&any, None);
        for form in [None, Some(""), Some("*")] {
            assert!(
                m.is_match(form),
                "against a match-any policy value the SAME three forms match, which is \
                 the short-circuit being a short-circuit rather than a comparison: {form:?}"
            );
        }
        // Whitespace is NOT one of the three. Shown against a policy value that IS
        // whitespace, because against a match-any level everything matches and
        // against a literal `sales` nothing but `sales` does — neither of those can
        // tell a literal comparison from the short-circuit.
        let blank = ResourceLevel {
            values: vec![" ".to_string()],
            is_excludes: false,
            is_recursive: false,
        };
        let m = LevelMatcher::build(&blank, None);
        assert!(
            m.is_match(Some(" ")),
            "whitespace is a literal value, compared literally — which is why \
             `validate_explain_target` leaves it answerable"
        );

        // ── The rows that actually PIN all three arms ───────────────────────
        //
        // ⚠ Everything above survives deleting an arm of `isAllValuesRequested`,
        // and a mutation proved it: against a literal `sales` the three forms fail
        // to match either way (no value matcher matches them), and against a
        // match-any level `is_match_any` wins before the flag is read. **Nor can a
        // policy value be chosen whose own matcher matches `""`** — `init()` drops
        // an empty policy value before building anything, `EndsWith` is built only
        // from a non-empty suffix, and every all-star value is match-any — so the
        // flag has no observable effect at all on a plain level.
        //
        // Where it IS observable is `applyExcludes`, whose carve-out is keyed to
        // the same flag: "the request asks for every value while the policy does
        // not allow every value" leaves the result alone instead of flipping it. So
        // on an excludes level each of the three forms is a NON-match, and losing
        // any one arm flips it to a match — which is the fail-OPEN direction for an
        // excludes-allow policy.
        let excludes = ResourceLevel {
            values: vec!["sales".to_string()],
            is_excludes: true,
            is_recursive: false,
        };
        let m = LevelMatcher::build(&excludes, None);
        assert!(
            m.is_match(Some("other")),
            "the excludes level does flip an ordinary non-match, or the rows below \
             would be vacuous"
        );
        assert!(!m.is_match(Some("sales")), "and does not flip a match");
        for form in [None, Some(""), Some("*")] {
            assert!(
                !m.is_match(form),
                "{form:?} is isAllValuesRequested, so `applyExcludes`' carve-out \
                 leaves the non-match alone rather than flipping it to a match. \
                 Delete this arm of the predicate and this request is ALLOWED by an \
                 excludes policy that names a different value — and the paired \
                 guard, `egret_perm_compile::routes::validate_explain_target`, is \
                 refusing the wrong set"
            );
        }
    }

    #[test]
    fn classify_reproduces_rangers_four_matcher_shapes() {
        assert_eq!(
            ValueMatcher::classify("test", true),
            Some(ValueMatcher::Equals("test".into()))
        );
        assert_eq!(
            ValueMatcher::classify("test*", true),
            Some(ValueMatcher::StartsWith("test".into()))
        );
        assert_eq!(
            ValueMatcher::classify("test**", true),
            Some(ValueMatcher::StartsWith("test".into()))
        );
        assert_eq!(
            ValueMatcher::classify("*test", true),
            Some(ValueMatcher::EndsWith("test".into()))
        );
        assert_eq!(
            ValueMatcher::classify("**test", true),
            Some(ValueMatcher::EndsWith("test".into()))
        );
        assert_eq!(
            ValueMatcher::classify("test*a", true),
            Some(ValueMatcher::Wildcard("test*a".into()))
        );
        assert_eq!(
            ValueMatcher::classify("te?t", true),
            Some(ValueMatcher::Wildcard("te?t".into()))
        );
        // A pure run of `*` is match-any, not a matcher.
        assert_eq!(ValueMatcher::classify("*", true), None);
        assert_eq!(ValueMatcher::classify("**", true), None);
        // With wildcards disabled every value is a literal.
        assert_eq!(
            ValueMatcher::classify("*", false),
            Some(ValueMatcher::Equals("*".into()))
        );
        assert_eq!(
            ValueMatcher::classify("te?t", false),
            Some(ValueMatcher::Equals("te?t".into()))
        );
    }

    /// Every row here is a MEASURED value, not a belief: it was produced by
    /// calling the real `FilenameUtils.wildcardMatch` on the harness
    /// classpath (`tests/ranger`), over a 3348-pair cross product of which
    /// this is the interesting slice. The full cross product produced
    /// byte-identical output from commons-io 2.8.0 (what
    /// `ranger-plugins-common:2.5.0` resolves) and 2.16.1 (what 2.8.0
    /// resolves), and the port above agrees with all 3348.
    ///
    /// The point of keeping them here rather than only in the differential
    /// gate: a regression is caught by `cargo test` alone, with no JVM, no
    /// Maven and no `EGRET_TEST_RANGER_E2E`.
    ///
    /// The first block is finding F1 — a `*` immediately followed by a `?`
    /// consumes NOTHING, which a greedy matcher gets wrong in the fail-open
    /// direction.
    ///
    /// The LAST block is the backtracking resume point, and it is here
    /// because review found that deleting
    /// `backtrack.push((wcs_idx, repeat))` left all 131 tests green AND the
    /// whole 4000-case differential green at the recorded seed. Every row in
    /// that block goes true → false without it. The corpus could not see it:
    /// almost every `Wildcard`-classified value it emitted ended in `*` (a
    /// trailing `*` jumps to the end of the text and never needs a resume
    /// point), and its literal runs never recurred in a request value. That
    /// is the difference between a shape being GENERATED and a rule being
    /// REACHED, and it is why these rows are pinned by hand rather than left
    /// to the generator.
    #[test]
    fn wildcard_match_reproduces_filenameutils() {
        let table: &[(&str, &str, bool, bool)] = &[
            ("?*?", "HIVE", true, false),
            ("?*?", "hive", true, false),
            ("?*?", "ab", true, true),
            ("?*?", "abc", true, false),
            ("?*?", "a", true, false),
            ("*?", "HIVE", true, false),
            ("*?", "a", true, true),
            ("*?", "ab", true, false),
            ("*?", "", true, false),
            ("?*", "HIVE", true, true),
            ("?*", "a", true, true),
            ("?*", "", true, false),
            ("*?*", "HIVE", true, true),
            ("*?*", "a", true, true),
            ("**?", "abc", true, false),
            ("?**", "abc", true, true),
            ("a*?", "abc", true, false),
            ("*a?", "abc", true, false),
            ("?a*", "abc", true, false),
            ("*?a", "abc", true, false),
            ("h*e", "HIVE", true, true),
            ("h*e", "hive", false, true),
            ("h*e", "HIVE", false, false),
            ("a*b", "ab", true, true),
            ("a**b", "ab", true, true),
            ("*a*b*", "sales", true, false),
            ("*a*b*", "ababc", true, true),
            ("a*b*", "abcd", true, true),
            ("*a*a*", "aaa", true, true),
            ("aa*aa", "aaaa", true, true),
            ("*aa*", "aaa", true, true),
            ("a*a*a", "aaa", true, true),
            ("*ab*c*", "abcabc", true, true),
            ("*abc*", "ababc", true, true),
            ("*", "", true, true),
            ("*", "HIVE", true, true),
            ("**", "HIVE", true, true),
            ("?", "", true, false),
            ("?", "a", true, true),
            ("??", "ab", true, true),
            ("??", "abc", true, false),
            ("???", "abc", true, true),
            ("HIVE", "hive", true, true),
            ("HIVE", "hive", false, false),
            ("H*E", "hive", true, true),
            ("H*E", "hive", false, false),
            ("or?er", "orders", true, false),
            ("or?ers", "orders", true, true),
            ("s**s", "orders", true, false),
            ("*s*", "orders", true, true),
            ("x?z", "xyz", true, true),
            ("x?z", "x_z", true, true),
            ("*_tmp", "hive_tmp", true, true),
            ("tmp_*", "tmp_1", true, true),
            // ── the backtracking resume point ────────────────────────────
            //
            // A literal token AFTER a `*` is found with `checkIndexOf`, and
            // the NEXT occurrence of that same literal is pushed as a resume
            // point. Where the first occurrence is a dead end and a later one
            // works, that push is the whole difference. Measured, like every
            // other row here.
            ("*ab*ab", "ababab", true, true),
            ("*ab*ab", "ababab", false, true),
            ("s**s", "sisters", true, true),
            ("s**s", "sss", true, true),
            ("s**s", "ssss", true, true),
            ("*a*a", "banana", true, true),
            ("*a*a", "ababa", true, true),
            ("*a*a", "aaa", true, true),
            ("*a*a?", "xabaxbay", true, true),
            ("*a*a?", "aabaa", true, true),
            ("*ab*b", "ababab", true, true),
            ("*b*ab", "ababab", true, true),
            ("*s*s", "assessors", true, true),
            ("*s*s", "sisters", false, true),
            ("*a*b*a", "ababa", true, true),
            ("*a*b*a", "aabaa", true, true),
            ("*ab*ab??", "ababab_x", true, true),
            ("*ab*ab", "abab", true, true),
            ("*ab*ab", "abcabc", true, false),
            ("*ab*ab", "ab", true, false),
            ("s**s", "sis", true, true),
            ("s**s", "x", true, false),
            ("*a*a", "x", true, false),
            ("*ab*ac", "ababab", true, false),
            ("*ba*ab", "ababab", true, true),
            ("*ab*ab?", "xababy", true, true),
            ("*ab*ab?", "ababab", true, false),
        ];
        for (pattern, text, ignore_case, expected) in table {
            assert_eq!(
                wildcard_match(pattern, text, *ignore_case),
                *expected,
                "wildcardMatch({text:?}, {pattern:?}, ignoreCase={ignore_case})"
            );
        }
    }

    /// Every Java case-fold class this crate SPLITS, and how many ordered
    /// pairs each splitting costs.
    ///
    /// Measured, not reasoned about, over **every BMP code point this JDK
    /// defines** (62034 of them) rather than over a hand-picked cross
    /// product — the earlier 1922-pair cross product is what let the declared
    /// set stop at two classes when there are 21. Two instruments agreed
    /// exactly:
    ///
    /// 1. Java's fold key `toLowerCase(toUpperCase(c))`, computed for every
    ///    defined code point and grouped. Java's three-step rule reduces to
    ///    that key.
    /// 2. The real `String.regionMatches(true, …)`, brute-forced over every
    ///    ordered pair of the 2423 code points either language treats as
    ///    cased. A pair outside that set cannot disagree: both mappings are
    ///    the identity there.
    ///
    /// Both give **104 ordered pairs, in 21 classes, and 0 the other way.**
    ///
    /// The brute force did surface 16 ordered pairs running the other way,
    /// and every one involves a code point `Character.isDefined` says this
    /// JDK does not know (U+A7CB, U+1C89, U+A7DC and their partners). That is
    /// Unicode-table skew between JDK 21 and Rust std, not a folding
    /// difference; it moves with the JVM, not with this crate.
    const JAVA_FOLD_CLASSES_THIS_CRATE_SPLITS: &[&[char]] = &[
        // 10 pairs — U+0049 U+0069 U+0130 U+0131
        &['\u{0049}', '\u{0069}', '\u{0130}', '\u{0131}'],
        // 4  pairs — U+0053 U+0073 U+017F
        &['\u{0053}', '\u{0073}', '\u{017F}'],
        // 4  pairs — U+0392 U+03B2 U+03D0
        &['\u{0392}', '\u{03B2}', '\u{03D0}'],
        // 4  pairs — U+0395 U+03B5 U+03F5
        &['\u{0395}', '\u{03B5}', '\u{03F5}'],
        // 6  pairs — U+0398 U+03B8 U+03D1 U+03F4
        &['\u{0398}', '\u{03B8}', '\u{03D1}', '\u{03F4}'],
        // 10 pairs — U+0345 U+0399 U+03B9 U+1FBE
        &['\u{0345}', '\u{0399}', '\u{03B9}', '\u{1FBE}'],
        // 4  pairs — U+039A U+03BA U+03F0
        &['\u{039A}', '\u{03BA}', '\u{03F0}'],
        // 4  pairs — U+00B5 U+039C U+03BC
        &['\u{00B5}', '\u{039C}', '\u{03BC}'],
        // 4  pairs — U+03A0 U+03C0 U+03D6
        &['\u{03A0}', '\u{03C0}', '\u{03D6}'],
        // 4  pairs — U+03A1 U+03C1 U+03F1
        &['\u{03A1}', '\u{03C1}', '\u{03F1}'],
        // 4  pairs — U+03A3 U+03C2 U+03C3
        &['\u{03A3}', '\u{03C2}', '\u{03C3}'],
        // 4  pairs — U+03A6 U+03C6 U+03D5
        &['\u{03A6}', '\u{03C6}', '\u{03D5}'],
        // 4  pairs — U+0412 U+0432 U+1C80
        &['\u{0412}', '\u{0432}', '\u{1C80}'],
        // 4  pairs — U+0414 U+0434 U+1C81
        &['\u{0414}', '\u{0434}', '\u{1C81}'],
        // 4  pairs — U+041E U+043E U+1C82
        &['\u{041E}', '\u{043E}', '\u{1C82}'],
        // 4  pairs — U+0421 U+0441 U+1C83
        &['\u{0421}', '\u{0441}', '\u{1C83}'],
        // 10 pairs — U+0422 U+0442 U+1C84 U+1C85
        &['\u{0422}', '\u{0442}', '\u{1C84}', '\u{1C85}'],
        // 4  pairs — U+042A U+044A U+1C86
        &['\u{042A}', '\u{044A}', '\u{1C86}'],
        // 4  pairs — U+0462 U+0463 U+1C87
        &['\u{0462}', '\u{0463}', '\u{1C87}'],
        // 4  pairs — U+1E60 U+1E61 U+1E9B
        &['\u{1E60}', '\u{1E61}', '\u{1E9B}'],
        // 4  pairs — U+1C88 U+A64A U+A64B
        &['\u{1C88}', '\u{A64A}', '\u{A64B}'],
    ];

    /// The case-folding divergence: **strictly narrower than Java, 104
    /// ordered pairs in 21 classes, none the other way.**
    ///
    /// Pins three things, so the README and the code cannot drift apart: the
    /// exact membership of the divergent set, its SIZE, and — the
    /// load-bearing one — its DIRECTION.
    ///
    /// Narrower means egret finds no match where Ranger finds one:
    /// fail-CLOSED for an allow rule, fail-OPEN for a deny or an
    /// `isExcludes` rule.
    #[test]
    fn case_folding_is_narrower_than_java_on_104_ordered_pairs_in_21_classes() {
        // Java folds every member of a class together. Count the ordered
        // pairs this crate does NOT.
        let mut split = 0usize;
        for class in JAVA_FOLD_CLASSES_THIS_CRATE_SPLITS {
            for &a in *class {
                for &b in *class {
                    if a != b && !chars_equal(a, b, true) {
                        split += 1;
                    }
                }
            }
        }
        assert_eq!(
            split, 104,
            "the declared case-folding divergence changed. Re-measure against the \
             real String.regionMatches(true, …) and update BOTH this table and the \
             bullet in tests/ranger/README.md; they must not drift apart."
        );
        assert_eq!(JAVA_FOLD_CLASSES_THIS_CRATE_SPLITS.len(), 21);

        // DIRECTION, within the measured universe: this crate never folds
        // together two characters Java keeps apart. Java's fold classes are
        // disjoint, so every cross-class pair is a pair Java splits.
        for (i, ci) in JAVA_FOLD_CLASSES_THIS_CRATE_SPLITS.iter().enumerate() {
            for (j, cj) in JAVA_FOLD_CLASSES_THIS_CRATE_SPLITS.iter().enumerate() {
                if i == j {
                    continue;
                }
                for &a in *ci {
                    for &b in *cj {
                        assert!(
                            !chars_equal(a, b, true),
                            "U+{:04X} and U+{:04X} are in different Java fold classes, so \
                             folding them equal would be WIDER than Java — the fail-open \
                             direction for a deny or isExcludes rule.",
                            a as u32,
                            b as u32
                        );
                    }
                }
            }
        }

        // The ordinary mappings all still work, so the divergence is confined
        // to the table above rather than being a hole in case-insensitive
        // matching.
        for (a, b) in [
            ('a', 'A'),
            ('A', 'a'),
            ('z', 'Z'),
            ('k', 'K'),
            ('s', 'S'),
            ('\u{00E5}', '\u{00C5}'),
            ('\u{00FF}', '\u{0178}'),
        ] {
            assert!(chars_equal(a, b, true), "{a:?} vs {b:?} must fold equal");
            assert!(
                !chars_equal(a, b, false),
                "{a:?} vs {b:?} must differ when case-sensitive"
            );
        }
    }

    #[test]
    fn split_on_tokens_collapses_only_adjacent_stars() {
        let split = |p: &str| split_on_tokens(&p.chars().collect::<Vec<_>>());
        let lit = |s: &str| Token::Literal(s.chars().collect());
        // No wildcard at all: one literal token holding the whole value.
        assert_eq!(split("test"), vec![lit("test")]);
        // Adjacent stars collapse...
        assert_eq!(split("a**b"), vec![lit("a"), Token::Any, lit("b")]);
        // ...but `?` between them keeps both, because `prevChar` is the
        // previous CHARACTER, not the previous token.
        assert_eq!(split("*?*"), vec![Token::Any, Token::One, Token::Any]);
        assert_eq!(split("?*?"), vec![Token::One, Token::Any, Token::One]);
        // `?` never collapses.
        assert_eq!(split("??"), vec![Token::One, Token::One]);
    }

    /// [`trie_reproduces_matcher`]'s table.
    ///
    /// # This is HALF of the claim, and the other half is in Java
    ///
    /// FIX ROUND 1, IMPORTANT 4: on its own this test asserts a hard-coded
    /// table against the function that implements that same table, which is a
    /// correlated oracle — it cannot fail unless someone edits both. The table
    /// is MEASURED from the real jar by
    /// `tests/ranger/src/test/java/EgretTagTrieAgreementTest.java`, which
    /// builds a real `RangerResourceTrie` and a real
    /// `RangerDefaultPolicyResourceMatcher` per value and compares them over
    /// the same request list. That one breaks on a `ranger.version` bump; this
    /// one breaks when the Rust classifier drifts from it. Neither is
    /// sufficient alone, and they must be edited together.
    ///
    /// Measured on 2.5.0 **and** 2.8.0, identically. Each row is a policy
    /// value; `true` means the trie returned exactly the evaluators
    /// `getMatchType` accepted, over the request values `pii`, `PII`, `p`,
    /// `pix`, `pi`, `xii`, `ii`, `""`, `piix` and `pii_x`.
    #[test]
    fn trie_agreement_matches_the_measured_table() {
        let table: &[(&str, bool)] = &[
            ("pii", true),
            ("PII", true),
            ("*", true),
            ("**", true),
            ("p*", true),
            ("p**", true),
            // The trie parks a leading-`*` value at the ROOT and returns it
            // for every resource; the matcher tests `ends_with`.
            ("*i", false),
            ("*ii", false),
            // ...and a value with an interior wildcard at its literal prefix:
            // `p*x` came back for `pii`, which `wildcardMatch` rejects.
            ("p*x", false),
            ("pi?", false),
            ("pi*i", false),
            ("p?i", false),
            // An empty value is dropped by the matcher before any matcher is
            // built; what the trie does with it was not measured.
            ("", false),
        ];
        for (value, expected) in table {
            assert_eq!(
                trie_reproduces_matcher(value, true),
                *expected,
                "trie_reproduces_matcher({value:?})"
            );
        }
        // With wildcards DISABLED every non-empty value is a literal, so the
        // trie keys on the whole string and the two always agree.
        for (value, _) in table {
            assert_eq!(trie_reproduces_matcher(value, false), !value.is_empty());
        }
    }

    #[test]
    fn has_no_gaps_rejects_a_skipped_level() {
        let h = vec!["catalog".to_string(), "schema".into(), "table".into()];
        assert!(has_no_gaps(&h, |n| n == "catalog"));
        assert!(has_no_gaps(&h, |n| n == "catalog" || n == "schema"));
        assert!(!has_no_gaps(&h, |n| n == "catalog" || n == "table"));
        assert!(!has_no_gaps(&h, |n| n == "table"));
    }
}
