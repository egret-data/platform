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
/// (the level is not part of the request); a key present with `""` is a
/// request for all values at that level, exactly as
/// `RangerAbstractResourceMatcher.isAllValuesRequested` treats it.
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
        // `isAllValuesRequested`: null, "" or "*".
        let all_values_requested = match resource_value {
            None => true,
            Some(v) => v.is_empty() || v == "*",
        };

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
            ValueMatcher::Wildcard(p) => {
                if ignore_case {
                    glob_match(&fold(p), &fold(resource_value))
                } else {
                    glob_match(p, resource_value)
                }
            }
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

/// `FilenameUtils.wildcardMatch`: `*` matches any run of characters
/// including none, `?` matches exactly one character, and the whole value
/// must be consumed.
///
/// Adapted from `crates/egret-perm/src/eval.rs:355-395`, which was written
/// precisely because a matcher that only special-cased the literal `"*"`
/// gives wrong answers. Unchanged from that version in algorithm; case
/// folding is applied by the caller rather than here, because whether to
/// fold is the service-def's decision, not the glob's.
fn glob_match(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    // Classic greedy wildcard match with backtracking: `star` remembers the
    // last `*` seen in the pattern and how far into `value` we'd consumed
    // when we saw it, so a later mismatch can retry by having that `*`
    // absorb one more character instead of failing outright.
    let (mut pi, mut vi) = (0usize, 0usize);
    let (mut star, mut star_vi) = (None::<usize>, 0usize);
    while vi < v.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == v[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            star_vi = vi;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            star_vi += 1;
            vi = star_vi;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn glob_backtracks() {
        assert!(glob_match("*", ""));
        assert!(glob_match("or*e*", "orders"));
        assert!(glob_match("o?ders", "orders"));
        assert!(!glob_match("o?ders", "oders"));
        assert!(!glob_match("or*z", "orders"));
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
