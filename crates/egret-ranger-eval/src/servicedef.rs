//! The service-def side of Ranger's matching: the resource hierarchy, each
//! resource's matcher class, and its `matcherOptions`.
//!
//! Everything here reproduces a named piece of Apache Ranger, read from the
//! source at tag `release-ranger-2.8.0` and confirmed byte-identical at
//! `release-ranger-2.5.0` for the files that decide a match (the only diffs
//! between the two tags in `RangerDefaultPolicyResourceMatcher` are a
//! `commons-lang` → `commons-lang3` import and plugin-context plumbing in
//! `createResourceMatcher`; `RangerAbstractResourceMatcher` and
//! `RangerDefaultResourceMatcher` differ only by that same import). 2.5.0 is
//! the version Trino 470 enforces and 2.8.0 the one StarRocks
//! fe-ubuntu:4.1 enforces — see
//! `docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`.
//!
//! The hierarchy walker ([`ResourceDef`], [`parse_resources`],
//! [`is_hierarchy_leaf`], [`hierarchy_chain`]) was **moved** here from
//! `crates/egret-policy/src/default_policies.rs`, which now depends on this
//! crate and calls these. A second walker that disagreed with the seeder
//! would be a defect in waiting, so there is exactly one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// Which of a service-def's three resource lists a hierarchy is drawn from.
/// Mirrors `RangerServiceDefHelper.Delegate.getResourceDefs`: access uses
/// `resources`, masking uses `dataMaskDef.resources`, row-filtering uses
/// `rowFilterDef.resources`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PolicyType {
    Access,
    DataMask,
    RowFilter,
}

impl PolicyType {
    /// Ranger's raw `policyType` code: `0` access, `1` masking, `2`
    /// row-filter. Code `3` is `POLICY_TYPE_AUDIT`, which
    /// `getResourceHierarchies` maps onto access; any other code falls back
    /// to `serviceDef.resources` the same way `getResourceDefs` does.
    ///
    /// **Known divergence, for an audit policy only (code `3`).** Folding
    /// audit onto access is right for picking the resource list, but it
    /// loses one thing: `RangerDefaultPolicyResourceMatcher.init` guards its
    /// failure teardown with `if (allMatchers == null && policyType !=
    /// RangerPolicy.POLICY_TYPE_AUDIT)`, so an audit policy whose `init`
    /// failed keeps its `serviceDefHelper` and can still reach the
    /// `ANCESTOR` branch of `getMatchType`, where
    /// [`crate::resource::match_resource_of_type`] returns
    /// [`crate::resource::MatchType::None`] (its `init_failed` short-circuit).
    /// This can only ever **under**-match, never over-match: `ANCESTOR` is
    /// not an allow for a resource request in the first place
    /// (`RangerDefaultPolicyEvaluator.evaluate` accepts it only for a
    /// `RangerTagAccessRequest`). Recorded rather than fixed, because
    /// reproducing it would mean threading an audit-only exception through
    /// the whole init/hierarchy path for a case that cannot change a
    /// decision.
    pub fn from_code(code: i32) -> PolicyType {
        match code {
            1 => PolicyType::DataMask,
            2 => PolicyType::RowFilter,
            _ => PolicyType::Access,
        }
    }
}

/// One `resources[]` entry of a service-def, with the fields that decide a
/// match. Field names and defaults come from Ranger's `RangerResourceDef`
/// plus `RangerAbstractResourceMatcher.getOption*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDef {
    pub name: String,
    /// `""` for a hierarchy root — the shape `default_policies.rs` has
    /// always used, kept so the walker's callers do not change.
    pub parent: String,
    /// `isValidLeaf` as the def actually wrote it. `None` (absent) is NOT
    /// the same as `Some(false)`: `RangerServiceDefHelper.isValid` rejects
    /// the whole graph only for a sink whose `isValidLeaf` is *explicitly*
    /// `false` (`Boolean.FALSE.equals(...)`), and an absent value never
    /// trips that.
    pub is_valid_leaf: Option<bool>,
    pub access_type_restrictions: Vec<String>,
    /// The `matcher` class name. Anything other than
    /// `RangerDefaultResourceMatcher` implements different semantics (path
    /// recursion, URL parsing, …) that this crate does not reproduce, so it
    /// is reported as [`ServiceDefIssue::UnmodelledMatcherClass`].
    pub matcher: String,
    pub matcher_options: MatcherOptions,
}

/// A resource's `matcherOptions`, resolved against Ranger's defaults.
///
/// `ServiceDefUtil.getBooleanOption` reads the option as a **string** and
/// runs `Boolean.parseBoolean` on it, so an absent option takes the default
/// and any non-`"true"` string is `false`. Jackson coerces a JSON boolean
/// into that string map, so both `{"wildCard": true}` (what egret's
/// embedded defs write) and `{"wildCard": "true"}` (what the captured
/// download writes) mean the same thing — [`MatcherOptions::parse`] accepts
/// either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatcherOptions {
    /// `ignoreCase`, default **true**.
    pub ignore_case: bool,
    /// `wildCard`, default **true**. With it off, `*` and `?` in a policy
    /// value are literal characters.
    pub wild_card: bool,
    /// `quotedCaseSensitive`, default false. Not implemented here (it makes
    /// case sensitivity depend on whether the *request* value starts with a
    /// quote char); reported as [`ServiceDefIssue::QuotedCaseSensitive`].
    pub quoted_case_sensitive: bool,
    /// `replaceTokens`, default true. Token replacement itself is reported
    /// per policy value by [`crate::model::Unmodelled::ResourceValueExpressionMarker`].
    pub replace_tokens: bool,
}

impl Default for MatcherOptions {
    fn default() -> Self {
        MatcherOptions {
            ignore_case: true,
            wild_card: true,
            quoted_case_sensitive: false,
            replace_tokens: true,
        }
    }
}

impl MatcherOptions {
    /// Read one resource's `matcherOptions` object. A missing object, or a
    /// missing key inside it, takes Ranger's default.
    pub fn parse(options: &Value) -> MatcherOptions {
        let default = MatcherOptions::default();
        MatcherOptions {
            ignore_case: boolean_option(options, "ignoreCase", default.ignore_case),
            wild_card: boolean_option(options, "wildCard", default.wild_card),
            quoted_case_sensitive: boolean_option(
                options,
                "quotedCaseSensitive",
                default.quoted_case_sensitive,
            ),
            replace_tokens: boolean_option(options, "replaceTokens", default.replace_tokens),
        }
    }
}

/// `ServiceDefUtil.getBooleanOption`: absent ⇒ default, otherwise
/// `Boolean.parseBoolean` of the option's string form — which is
/// case-insensitively `"true"` and nothing else. A JSON boolean is accepted
/// because Jackson stringifies it into Ranger's `Map<String, String>`.
fn boolean_option(options: &Value, name: &str, default: bool) -> bool {
    match options.get(name) {
        None | Some(Value::Null) => default,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        // Any other JSON type stringifies to something that is not "true".
        Some(_) => false,
    }
}

/// One `accessTypes[]` (or `markerAccessTypes[]`) entry of a service-def.
///
/// `impliedGrants` is what makes a single granted access type stand for
/// several: Ranger's `RangerServiceDefHelper.Delegate.computeImpliedGrants`
/// folds `accessTypes[]` and `markerAccessTypes[]` into one
/// `Map<String, Collection<String>>`, which
/// `RangerAbstractPolicyItemEvaluator.computeWithImpliedGrants` then applies
/// to a policy item's `accesses`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessTypeDef {
    pub name: String,
    pub implied_grants: Vec<String>,
}

/// A construct in a service-def that this crate detected but does not
/// reproduce. Reported, never silently ignored: a silently dropped
/// construct produces a confident wrong decision downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceDefIssue {
    /// A resource names a matcher class other than
    /// `RangerDefaultResourceMatcher`. Matching this resource uses the
    /// default semantics, which is not what the named class does.
    UnmodelledMatcherClass { resource: String, matcher: String },
    /// A resource sets `quotedCaseSensitive`, which makes case sensitivity
    /// depend on the request value's leading character. Matching falls back
    /// to plain `ignoreCase` behaviour for this resource.
    QuotedCaseSensitive { resource: String },
    /// Two `impliedGrants` entries form a **chain**: an access type implied
    /// by one access type is itself an implying access type.
    /// `computeWithImpliedGrants` iterates a `HashMap` while mutating the
    /// item it is expanding, so whether the second level is reached depends
    /// on that map's unspecified iteration order — Ranger's own comment says
    /// "Only one round of 'expansion' is done", but the code can do two when
    /// the order happens to favour it. Neither egret service-def has a chain
    /// (`trino.json` declares only a flat `all => [...]`; `starrocks.json`
    /// declares none), so this is reported rather than guessed at.
    ChainedImpliedGrants { implying: String, implied: String },
    /// `RangerServiceDefHelper.isValid` would reject this policy type's
    /// resource graph, which makes Ranger produce **no** hierarchies for it
    /// — so every policy of that type matches nothing.
    InvalidResourceGraph {
        policy_type: PolicyType,
        reason: GraphProblem,
    },
}

/// Why a resource graph is not a forest, in Ranger's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphProblem {
    /// `getSources()` or `getSinks()` came back empty.
    NoSourcesOrSinks,
    /// A sink is explicitly marked `isValidLeaf: false`.
    SinkIsNotAValidLeaf { resource: String },
    /// A `parent` chain cycles, or names a resource the def does not
    /// declare.
    BrokenParentChain { resource: String },
}

/// A parsed service-def: its resources and valid resource hierarchies, per
/// policy type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDef {
    name: String,
    per_type: BTreeMap<PolicyType, TypeDefs>,
    access_types: Vec<AccessTypeDef>,
    issues: Vec<ServiceDefIssue>,
    /// `Some(true)`/`Some(false)` when `contextEnrichers` was readable as a
    /// list, `None` when the key was absent or not a list — see
    /// [`ServiceDef::has_tag_enricher`].
    tag_enricher: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct TypeDefs {
    resources: Vec<ResourceDef>,
    /// Root-to-leaf resource-name chains. Empty when the graph is invalid,
    /// exactly as `RangerServiceDefHelper` stores `EMPTY_RESOURCE_HIERARCHY`.
    hierarchies: Vec<Vec<String>>,
}

impl ServiceDef {
    /// Parse a service-def object — the `serviceDef` of a
    /// `/policies/download` envelope
    /// ([`crate::model::ServicePolicySet::service_def`]), or one of the
    /// defs embedded at `crates/egret-policy/src/service-defs/`.
    ///
    /// Infallible by design: a def this crate cannot fully reproduce still
    /// parses, and says so through [`ServiceDef::issues`]. A def whose
    /// graph Ranger itself would reject yields no hierarchies for that
    /// policy type, which is what makes every match come back
    /// [`crate::resource::MatchType::None`] — the same answer the live
    /// plugin gives.
    pub fn parse(def: &Value) -> ServiceDef {
        let mut issues = Vec::new();
        let access = parse_resources(def);

        let mut per_type = BTreeMap::new();
        for policy_type in [
            PolicyType::Access,
            PolicyType::DataMask,
            PolicyType::RowFilter,
        ] {
            let resources = match policy_type {
                PolicyType::Access => access.clone(),
                PolicyType::DataMask => {
                    normalize_sub_def(&def["dataMaskDef"]["resources"], &access)
                }
                PolicyType::RowFilter => {
                    normalize_sub_def(&def["rowFilterDef"]["resources"], &access)
                }
            };

            let hierarchies = match build_hierarchies(&resources) {
                Ok(h) => h,
                Err(reason) => {
                    // Only report a broken graph for a policy type the def
                    // actually declares resources for: an absent
                    // `dataMaskDef` is not a defect, it just means the
                    // service does not support masking.
                    if !resources.is_empty() {
                        issues.push(ServiceDefIssue::InvalidResourceGraph {
                            policy_type,
                            reason,
                        });
                    }
                    Vec::new()
                }
            };

            for res in &resources {
                if !res.matcher.is_empty() && !res.matcher.ends_with("RangerDefaultResourceMatcher")
                {
                    let issue = ServiceDefIssue::UnmodelledMatcherClass {
                        resource: res.name.clone(),
                        matcher: res.matcher.clone(),
                    };
                    if !issues.contains(&issue) {
                        issues.push(issue);
                    }
                }
                if res.matcher_options.quoted_case_sensitive {
                    let issue = ServiceDefIssue::QuotedCaseSensitive {
                        resource: res.name.clone(),
                    };
                    if !issues.contains(&issue) {
                        issues.push(issue);
                    }
                }
            }

            per_type.insert(
                policy_type,
                TypeDefs {
                    resources,
                    hierarchies,
                },
            );
        }

        let access_types = parse_access_types(def);
        for chained in chained_implied_grants(&access_types) {
            if !issues.contains(&chained) {
                issues.push(chained);
            }
        }

        ServiceDef {
            name: def["name"].as_str().unwrap_or_default().to_string(),
            per_type,
            access_types,
            issues,
            tag_enricher: parse_tag_enricher(def),
        }
    }

    /// Every `accessTypes[]` entry, followed by every `markerAccessTypes[]`
    /// entry — the two lists `computeImpliedGrants` reads, in that order.
    pub fn access_types(&self) -> &[AccessTypeDef] {
        &self.access_types
    }

    /// `RangerServiceDefHelper.getImpliedAccessGrants()`: implying access
    /// type -> the access types it implies, folded over `accessTypes[]` and
    /// `markerAccessTypes[]`. Entries with no `impliedGrants` are absent,
    /// exactly as `computeImpliedGrants` builds the map.
    pub fn implied_access_grants(&self) -> BTreeMap<&str, BTreeSet<&str>> {
        let mut ret: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for at in &self.access_types {
            if at.implied_grants.is_empty() {
                continue;
            }
            ret.entry(at.name.as_str())
                .or_default()
                .extend(at.implied_grants.iter().map(String::as_str));
        }
        ret
    }

    /// The def's `name` (`"trino"`, `"starrocks"`, `"tag"`, …).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Constructs this crate detected but does not reproduce.
    pub fn issues(&self) -> &[ServiceDefIssue] {
        &self.issues
    }

    /// Whether this def declares a `RangerTagEnricher` in `contextEnrichers`.
    ///
    /// This, and NOT the engine type, is what decides whether tag policies can
    /// reach a decision. `RangerPolicyEngineImpl.evaluateTagPolicies` takes the
    /// request's tag set from
    /// `RangerAccessRequestUtil.getRequestTagsFromContext(request.getContext())`
    /// (offset 104, identical at 2.5.0 and 2.8.0), and the only thing in
    /// `ranger-plugins-common` that writes that context key is
    /// `RangerTagEnricher.enrich`, through
    /// `RangerAccessRequestUtil.setRequestTagsInContext` (offset 199 of
    /// `enrich`). `RangerPolicyRepository` instantiates the enrichers named in
    /// the service-def's `contextEnrichers` and nowhere else. With none
    /// declared, the tag set stays empty,
    /// `RangerPolicyRepository.getLikelyMatchPolicyEvaluators` returns
    /// `Collections.EMPTY_LIST` at its `isNotEmpty(tags)` branch (offsets 0-9),
    /// and `evaluateTagPolicies` exits at `isNotEmpty(evaluators)`.
    ///
    /// egret's `trino.json` declares the enricher and its `starrocks.json` has
    /// `contextEnrichers: []` — which is why two measured StarRocks FEs never
    /// wrote a `_tag.json` while a Trino pod wrote one within seconds for a
    /// service with no tag service bound. Gating on `EngineType::StarRocks`
    /// instead would be silently wrong the day that one field changes.
    ///
    /// # ⚠ `None` is UNKNOWN, and must not be read as "no enricher"
    ///
    /// `None` means the def carried no `contextEnrichers` key at all, or
    /// carried `null`, or carried something that is not a list.
    ///
    /// **Ranger itself does not make this distinction**, and the doc would be
    /// wrong to imply it does: `RangerServiceDef.getContextEnrichers()` is
    /// null for an absent key and for an explicit `null` alike, and a null or
    /// empty list both cause `RangerPolicyRepository` to instantiate no
    /// enrichers. The strictness here is egret's, and it is about
    /// **provenance**, not about Java.
    ///
    /// A def reaching this crate is not necessarily the one the plugin
    /// loaded. It may be a trimmed or re-serialised copy, and a trimmed copy
    /// that lost this key looks exactly like a def that declares no enricher.
    /// That shape is measured, not theoretical: the live download captured at
    /// `tests/fixtures/tag-policies-live.json` carries the key on its `trino`
    /// def, while the repo's older capture at
    /// `tests/ranger/fixtures/service-policies.json` — also a `trino` def —
    /// omits it entirely, as does the `tag` def embedded inside the live
    /// download. Reading that silence as `Some(false)` would license the
    /// narrowing for a def whose real counterpart does declare the enricher,
    /// and the answer that comes back would be an allow the engine's tag
    /// policies could have denied. Only an explicit, readable, enricher-free
    /// list is a licence to narrow.
    pub fn has_tag_enricher(&self) -> Option<bool> {
        self.tag_enricher
    }

    /// The resource defs backing `policy_type`.
    pub fn resources(&self, policy_type: PolicyType) -> &[ResourceDef] {
        self.per_type
            .get(&policy_type)
            .map(|t| t.resources.as_slice())
            .unwrap_or_default()
    }

    /// One resource def by name, for `policy_type`.
    pub fn resource(&self, policy_type: PolicyType, name: &str) -> Option<&ResourceDef> {
        self.resources(policy_type).iter().find(|r| r.name == name)
    }

    /// Every valid resource hierarchy for `policy_type`, each a
    /// root-to-leaf list of resource names.
    ///
    /// Equivalent to `RangerServiceDefHelper.getResourceHierarchies`, whose
    /// set is "every root→sink path, plus every prefix of such a path that
    /// ends at a resource marked `isValidLeaf`, plus every isolated
    /// resource". Ranger holds them in a `HashSet`, so its iteration order
    /// is unspecified; these are sorted (shortest first, then
    /// lexicographically) so that the "pick the shortest hierarchy"
    /// tie-break in `getMatchingHierarchy` is deterministic here. For a
    /// forest — which `isValid` already guarantees — no two hierarchies of
    /// equal length contain the same resource-name set, so the tie-break
    /// never actually has a choice to make.
    pub fn hierarchies(&self, policy_type: PolicyType) -> &[Vec<String>] {
        self.per_type
            .get(&policy_type)
            .map(|t| t.hierarchies.as_slice())
            .unwrap_or_default()
    }
}

/// `ServiceDefUtil.normalizeDataMaskDef` / `normalizeRowFilterDef`: a
/// mask/row-filter resource entry is merged **onto** the same-named access
/// resource def by `ServiceDefUtil.mergeResourceDef`.
///
/// Without this, a captured download's `dataMaskDef.resources` — which
/// carry neither `parent` nor `isValidLeaf`, only `matcherOptions` and
/// `uiHint` — would parse as a forest of isolated roots and no masking
/// policy would ever match past its first level.
///
/// What the merge keeps and what it replaces is **not** symmetric, and the
/// asymmetry decides how many mask hierarchies exist:
///
/// - `parent` is retained from the base. `mergeResourceDef` says so in its
///   own comment: `// retain base values for: itemId, name, type, level,
///   parent, lookupSupported`.
/// - `matcher` wins from the overlay when non-empty; `matcherOptions` are
///   overlaid key by key, so a key the overlay omits keeps the base's value.
/// - `accessTypeRestrictions` wins from the overlay when non-empty.
/// - **`isValidLeaf` is taken from the overlay, replacing the base's** —
///   the one field a reader would most expect to be inherited, and is not.
///   `mergeResourceDef` (2.8.0) ends with:
///
///   ```java
///   boolean copyLeafValue = false;
///   if (ret.getIsValidLeaf() != null) {
///       if (!ret.getIsValidLeaf().equals(delta.getIsValidLeaf())) copyLeafValue = true;
///   } else {
///       if (delta.getIsValidLeaf() != null) copyLeafValue = true;
///   }
///   if (copyLeafValue) ret.setIsValidLeaf(delta.getIsValidLeaf());
///   ```
///
///   That block is exactly an unconditional `ret = delta`, in all five
///   cases: equal values (copy is a no-op), differing non-null values
///   (copy), base non-null with a null overlay (`!ret.equals(null)` is
///   true, so it copies the **null** in), null base with a non-null
///   overlay (copy), and both null (copy is a no-op). So the assignment
///   below is the rule, not a simplification of it.
///
/// This is what makes Trino's masking hierarchy a single
/// `catalog/schema/table/column` chain: `catalog`, `schema` and `table` are
/// `isValidLeaf: true` in `resources`, every `dataMaskDef` entry omits the
/// field, and the merge therefore clears it — so none of the three is a
/// hierarchy termination point for a mask policy. Retaining the base's
/// value instead would invent three extra prefix hierarchies that Ranger
/// does not have.
fn normalize_sub_def(sub_resources: &Value, access: &[ResourceDef]) -> Vec<ResourceDef> {
    let Some(entries) = sub_resources.as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let name = entry["name"].as_str()?;
            let overlay = parse_resource(entry)?;
            let Some(base) = access.iter().find(|r| r.name == name) else {
                return Some(overlay);
            };

            let mut merged = base.clone();
            if !overlay.matcher.is_empty() {
                merged.matcher = overlay.matcher;
            }
            if !overlay.access_type_restrictions.is_empty() {
                merged.access_type_restrictions = overlay.access_type_restrictions;
            }
            // The `copyLeafValue` block above, which reduces to this.
            merged.is_valid_leaf = overlay.is_valid_leaf;
            // `parse_resource` has already resolved matcher-option defaults,
            // so re-read the raw object to tell "absent" from "explicitly
            // default".
            merged.matcher_options =
                merge_matcher_options(&base.matcher_options, &entry["matcherOptions"]);
            Some(merged)
        })
        .collect()
}

fn merge_matcher_options(base: &MatcherOptions, overlay: &Value) -> MatcherOptions {
    MatcherOptions {
        ignore_case: boolean_option(overlay, "ignoreCase", base.ignore_case),
        wild_card: boolean_option(overlay, "wildCard", base.wild_card),
        quoted_case_sensitive: boolean_option(
            overlay,
            "quotedCaseSensitive",
            base.quoted_case_sensitive,
        ),
        replace_tokens: boolean_option(overlay, "replaceTokens", base.replace_tokens),
    }
}

/// Every `resources[]` entry of a service-def, in declaration order.
///
/// Moved verbatim (plus the matcher fields Task 4 needs) from
/// `crates/egret-policy/src/default_policies.rs`, which still calls it.
pub fn parse_resources(def: &Value) -> Vec<ResourceDef> {
    def["resources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(parse_resource)
        .collect()
}

fn parse_resource(r: &Value) -> Option<ResourceDef> {
    let name = r["name"].as_str()?.to_string();
    Some(ResourceDef {
        name,
        parent: r["parent"].as_str().unwrap_or("").to_string(),
        is_valid_leaf: r["isValidLeaf"].as_bool(),
        access_type_restrictions: r["accessTypeRestrictions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        matcher: r["matcher"].as_str().unwrap_or("").to_string(),
        matcher_options: MatcherOptions::parse(&r["matcherOptions"]),
    })
}

/// A resource is a hierarchy termination point if it's explicitly marked
/// `isValidLeaf`, or if it's a true tree leaf (nothing declares it as
/// parent).
///
/// Moved from `default_policies.rs`. Equivalent to the set Ranger's
/// `Delegate.getHierarchies` produces — every root→sink path plus every
/// `isValidLeaf` prefix of one — because for a forest each termination
/// point has exactly one root path.
pub fn is_hierarchy_leaf(res: &ResourceDef, all: &[ResourceDef]) -> bool {
    res.is_valid_leaf.unwrap_or(false) || !all.iter().any(|r| r.parent == res.name)
}

/// Root-to-leaf resource chain for one hierarchy termination point.
///
/// Moved from `default_policies.rs`.
pub fn hierarchy_chain<'a>(leaf: &'a ResourceDef, all: &'a [ResourceDef]) -> Vec<&'a ResourceDef> {
    let mut chain = vec![leaf];
    let mut cur = leaf;
    while !cur.parent.is_empty() {
        match all.iter().find(|r| r.name == cur.parent) {
            Some(p) => {
                chain.push(p);
                cur = p;
            }
            None => break,
        }
    }
    chain.reverse();
    chain
}

/// Every valid hierarchy, as root-to-leaf name lists, or why the graph is
/// not a forest.
///
/// Reproduces `RangerServiceDefHelper.Delegate.isValid` + `getHierarchies`:
/// the graph must have at least one source and one sink, must not cycle,
/// and no sink may be explicitly `isValidLeaf: false`.
fn build_hierarchies(resources: &[ResourceDef]) -> Result<Vec<Vec<String>>, GraphProblem> {
    if resources.is_empty() {
        return Err(GraphProblem::NoSourcesOrSinks);
    }

    let has_source = resources.iter().any(|r| r.parent.is_empty());
    let sinks: Vec<&ResourceDef> = resources
        .iter()
        .filter(|r| !resources.iter().any(|c| c.parent == r.name))
        .collect();
    if !has_source || sinks.is_empty() {
        return Err(GraphProblem::NoSourcesOrSinks);
    }
    for sink in &sinks {
        if sink.is_valid_leaf == Some(false) {
            return Err(GraphProblem::SinkIsNotAValidLeaf {
                resource: sink.name.clone(),
            });
        }
    }

    let mut hierarchies: Vec<Vec<String>> = Vec::new();
    for res in resources {
        if !is_hierarchy_leaf(res, resources) {
            continue;
        }
        // `hierarchy_chain` stops at a parent the def does not declare, and
        // would loop forever on a cycle, so walk it here with both guards
        // and only then trust the chain.
        let chain = checked_chain(res, resources)?;
        if !hierarchies.contains(&chain) {
            hierarchies.push(chain);
        }
    }

    hierarchies.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    Ok(hierarchies)
}

fn checked_chain(leaf: &ResourceDef, all: &[ResourceDef]) -> Result<Vec<String>, GraphProblem> {
    let mut chain = vec![leaf.name.clone()];
    let mut cur = leaf;
    while !cur.parent.is_empty() {
        if chain.iter().any(|n| n == &cur.parent) {
            return Err(GraphProblem::BrokenParentChain {
                resource: cur.parent.clone(),
            });
        }
        match all.iter().find(|r| r.name == cur.parent) {
            Some(p) => {
                chain.push(p.name.clone());
                cur = p;
            }
            None => {
                return Err(GraphProblem::BrokenParentChain {
                    resource: cur.parent.clone(),
                })
            }
        }
    }
    chain.reverse();
    Ok(chain)
}

/// `accessTypes[]` then `markerAccessTypes[]`, in the order
/// `RangerServiceDefHelper.Delegate.computeImpliedGrants` reads them.
/// The class name Ranger's tag enricher is registered under. Matched on the
/// simple name so a repackaged-but-equivalent enricher is still recognised
/// (the same leniency `ServiceDefIssue::UnmodelledMatcherClass` uses for
/// matcher classes).
const TAG_ENRICHER_CLASS: &str = "RangerTagEnricher";

/// Read `contextEnrichers` and say whether a `RangerTagEnricher` is among
/// them — `None` when the key is absent or is not a list.
///
/// See [`ServiceDef::has_tag_enricher`] for why the absent case must stay
/// distinguishable from "declared none".
fn parse_tag_enricher(def: &Value) -> Option<bool> {
    let enrichers = def.get("contextEnrichers")?.as_array()?;
    Some(enrichers.iter().any(|e| {
        e.get("enricher")
            .and_then(Value::as_str)
            .is_some_and(|c| c.ends_with(TAG_ENRICHER_CLASS))
    }))
}

pub fn parse_access_types(def: &Value) -> Vec<AccessTypeDef> {
    let mut ret = Vec::new();
    for key in ["accessTypes", "markerAccessTypes"] {
        let Some(list) = def[key].as_array() else {
            continue;
        };
        for entry in list {
            let Some(name) = entry["name"].as_str() else {
                continue;
            };
            ret.push(AccessTypeDef {
                name: name.to_string(),
                implied_grants: entry["impliedGrants"]
                    .as_array()
                    .map(|g| {
                        g.iter()
                            .filter_map(|v| v.as_str())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            });
        }
    }
    ret
}

/// Every pair `(implying, implied)` where `implied` is itself an implying
/// access type — see [`ServiceDefIssue::ChainedImpliedGrants`].
fn chained_implied_grants(access_types: &[AccessTypeDef]) -> Vec<ServiceDefIssue> {
    let implying: BTreeMap<&str, &AccessTypeDef> = access_types
        .iter()
        .filter(|a| !a.implied_grants.is_empty())
        .map(|a| (a.name.as_str(), a))
        .collect();
    let mut ret = Vec::new();
    for (name, def) in &implying {
        for grant in &def.implied_grants {
            if grant.as_str() != *name && implying.contains_key(grant.as_str()) {
                ret.push(ServiceDefIssue::ChainedImpliedGrants {
                    implying: (*name).to_string(),
                    implied: grant.clone(),
                });
            }
        }
    }
    ret
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn def() -> Value {
        json!({
            "name": "t",
            "resources": [
                {"name": "catalog", "parent": "", "isValidLeaf": true,
                 "matcherOptions": {"wildCard": true, "ignoreCase": true}},
                {"name": "schema", "parent": "catalog", "isValidLeaf": true,
                 "matcherOptions": {"wildCard": "true", "ignoreCase": "false"}},
                {"name": "table", "parent": "schema"},
                {"name": "role", "parent": ""}
            ]
        })
    }

    #[test]
    fn boolean_options_accept_both_json_bool_and_string() {
        let d = ServiceDef::parse(&def());
        let schema = d.resource(PolicyType::Access, "schema").unwrap();
        assert!(schema.matcher_options.wild_card);
        assert!(!schema.matcher_options.ignore_case);
    }

    #[test]
    fn absent_matcher_options_take_rangers_defaults() {
        let d = ServiceDef::parse(&def());
        let table = d.resource(PolicyType::Access, "table").unwrap();
        assert_eq!(table.matcher_options, MatcherOptions::default());
        assert!(table.matcher_options.ignore_case);
        assert!(table.matcher_options.wild_card);
    }

    #[test]
    fn hierarchies_include_every_valid_leaf_prefix() {
        let d = ServiceDef::parse(&def());
        let h = d.hierarchies(PolicyType::Access);
        assert!(h.contains(&vec!["catalog".to_string()]));
        assert!(h.contains(&vec!["catalog".to_string(), "schema".to_string()]));
        assert!(h.contains(&vec![
            "catalog".to_string(),
            "schema".to_string(),
            "table".to_string()
        ]));
        assert!(h.contains(&vec!["role".to_string()]));
        assert_eq!(h.len(), 4);
    }

    #[test]
    fn a_cycle_yields_no_hierarchies_and_is_reported() {
        let cyclic = json!({
            "name": "c",
            "resources": [
                {"name": "a", "parent": "b"},
                {"name": "b", "parent": "a"}
            ]
        });
        let d = ServiceDef::parse(&cyclic);
        assert!(d.hierarchies(PolicyType::Access).is_empty());
        assert!(d
            .issues()
            .iter()
            .any(|i| matches!(i, ServiceDefIssue::InvalidResourceGraph { .. })));
    }

    #[test]
    fn a_non_default_matcher_class_is_reported() {
        let d = ServiceDef::parse(&json!({
            "name": "hdfs",
            "resources": [{
                "name": "path", "parent": "",
                "matcher": "org.apache.ranger.plugin.resourcematcher.RangerPathResourceMatcher"
            }]
        }));
        assert!(d.issues().iter().any(|i| matches!(
            i,
            ServiceDefIssue::UnmodelledMatcherClass { resource, .. } if resource == "path"
        )));
    }
}
