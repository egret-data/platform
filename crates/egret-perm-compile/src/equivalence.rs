//! The equivalence gate (SP4d-3 Task 6): does a migration change any decision
//! the engines will take?
//!
//! Pure — no DB, no clock, no I/O. It takes the policy download each affected
//! profile serves BEFORE the migration and the one it serves AFTER, asks
//! `egret-ranger-eval` the same questions of both, per plugin version, and
//! reports every answer that differs. An empty answer is the only thing that
//! lets Task 7's transaction commit.
//!
//! # What is compared, and why the whole profile
//!
//! Ranger decides over the policy SET: a `denyPolicyItems` or a
//! `policyPriority` in a sibling policy changes the answer. So the unit is a
//! profile's whole download ([`ServicePolicySet`], resource policies and the
//! `tagPolicies` block together), never the migrated policy in isolation.
//!
//! **Every affected profile, not only the source one** (user ruling,
//! 2026-10-11). A migrated grant on catalog X compiles into EVERY profile whose
//! engine serves X, and a tag rule into the tag service of every Trino profile
//! (`trigger::affected_services_for_grant` / `_rule`). So migrating a policy
//! out of profile P can change what profile Q enforces. [`compare_profiles`]
//! takes one [`ProfileCase`] per affected profile and tags every
//! [`Disagreement`] with the profile it is on; WHICH profiles are affected is
//! the caller's (DB) question, not this module's.
//!
//! **Per [`PluginVersion`] of the engines bound to that profile.** The two
//! versions answer at least one shape differently (a tag DENY plus a strictly
//! higher-priority resource ALLOW: Allowed on 2.8.0, Denied on 2.5.0 — see
//! `egret_ranger_eval::eval::UndecidableCause::TagDenyReopenedDiffersByPluginVersion`).
//!
//! # What counts as "the same answer"
//!
//! | question | same when | never the same |
//! |---|---|---|
//! | access ([`evaluate`]) | both `Allowed`, or both `Denied` for the same [`egret_ranger_eval::eval::DenyReason`] | either side `Undecidable` |
//! | mask ([`evaluate_data_mask`]) | both `NoMask`, or both `Masked` with equal `mask_type`, `masked_value` and `mask_condition` | either side `Undecidable` |
//! | row filter ([`evaluate_row_filter`]) | both `NoFilter`, or both `Filtered` with an equal `filter_expr` | either side `Undecidable` |
//!
//! Policy ids and names are NOT compared — a migration renames by construction
//! (a hand-written policy's answer becomes a `perm-grant-…` policy's) — but
//! they are rendered, so the operator can see which policy decided each side.
//!
//! **`Undecidable` is a disagreement even when both sides are `Undecidable`.**
//! The gate's claim is "equivalent", and an answer the evaluator could not
//! compute proves nothing about equivalence. Fail-closed by design: a profile
//! carrying something the evaluator refuses (an `{OWNER}` item, a validity
//! schedule, an unresolved tag set) blocks every migration that reaches it
//! until that is dealt with.
//!
//! **`DenyReason` is compared.** `NoPolicyMatched` and `DenyPolicyItem` are both
//! denials, but Ranger marks only the second `isAccessDetermined` when the host
//! supports a native fallback authorizer, so they are not interchangeable in
//! general. No correct grammar output changes a reason (a deny item becomes a
//! deny grant, and an allow-only policy leaves the unmatched unmatched), so the
//! strict reading costs nothing on a correct migration.
//!
//! # Probes
//!
//! See [`derive_probes`]. They reach past both sets' vocabulary on purpose: a
//! probe set built only from the two policies' own coordinates and principals
//! inherits exactly their blind spots — a correlated oracle is not an oracle.
//!
//! # Service tags
//!
//! A tag policy's answer depends on which columns carry which tag NAME. That
//! enters as [`AccessRequest::resolved_tags`] on each PROBE, never on a policy
//! set, so the before and the after evaluation of one probe read the same tag
//! input by construction — there is no second tag argument that could differ.
//!
//! ⚠ **ONE tag input, and it cannot be vacuous** (Task 6 fix round 1, review
//! I1). The first version took the tagged coordinates and the tag resolver as
//! two separate inputs, and the natural call shape (no coordinates, a real
//! resolver) put every probe at an untagged coordinate — so a changed tag mask,
//! a dropped exemption or a dropped tag DENY compared EQUIVALENT. Now
//! [`derive_probes`] takes the profile's [`TagAssignment`] rows once and derives
//! BOTH which coordinates are probed as tagged AND every probe's resolved tags
//! from them ([`Tagging::Actual`]). On top of that, for every tag NAME either
//! set's tag policies address, every table/column coordinate is probed again
//! carrying only that tag ([`Tagging::Hypothetical`]) — so a tag-rule change is
//! caught even when no column carries the tag today, which is also the state in
//! which it would otherwise lie dormant until somebody tags a column.

use std::collections::{BTreeMap, BTreeSet};

use egret_ranger_eval::eval::{
    evaluate, evaluate_data_mask, evaluate_row_filter, AccessRequest, Decision, DenyReason,
    EngineConfig, MaskDecision, MaskResult, PluginVersion, ResolvedTag, RowFilterDecision,
    TagServiceResource, UnenrichedTags,
};
use egret_ranger_eval::model::{PolicyItem, RangerPolicy, ResourceLevel, ServicePolicySet};
use egret_ranger_eval::resource::{MatchType, RequestResource};
use egret_ranger_eval::servicedef::{PolicyType, ServiceDef};

/// The user name of the probe principal no policy names.
pub const PROBE_UNNAMED_USER: &str = "egret-probe-unnamed-user";
/// Prefix of the synthetic user that is a member of exactly one named group.
pub const PROBE_GROUP_MEMBER_PREFIX: &str = "egret-probe-member-of-";
/// Prefix of the synthetic user that holds exactly one named role.
pub const PROBE_ROLE_HOLDER_PREFIX: &str = "egret-probe-holder-of-";
/// Prefix of a SIBLING value: a name at one level that neither set mentions,
/// under parents that one of them does.
pub const PROBE_SIBLING_PREFIX: &str = "egret-probe-other-";
/// Prefix of the value standing in for a `*` (or absent) level.
pub const PROBE_ANY_PREFIX: &str = "egret-probe-any-";

/// Which of Ranger's three questions a [`Disagreement`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Check {
    Access,
    DataMask,
    RowFilter,
}

/// One probe on which the before-state and the after-state answer differently
/// (or on which either could not be decided), on one profile, on one plugin
/// version. Every string is meant to be shown to an operator as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disagreement {
    /// The profile (`policy.authz_services.id`) this disagreement is on.
    pub service_id: i64,
    /// That profile's name, for display.
    pub service_name: String,
    pub plugin_version: PluginVersion,
    pub check: Check,
    pub user: String,
    /// The probe user's groups and roles — non-empty only for the synthetic
    /// members [`derive_probes`] adds for each group/role a policy names.
    pub groups: Vec<String>,
    pub roles: Vec<String>,
    /// The tag names the probe's resource carried, and whether that tagging
    /// is today's ([`Tagging::Actual`]) or one a column COULD have
    /// ([`Tagging::Hypothetical`]). Both kinds block.
    pub tags: Vec<String>,
    pub tagging: Tagging,
    pub access_type: String,
    /// The coordinate, e.g. `wh.sales.orders.email` — dotted in the
    /// service-def's level order when it is a catalog path, `level=value, …`
    /// otherwise.
    pub resource: String,
    /// The rendered answer before / after, e.g. `allowed (policy "sales read")`.
    pub before: String,
    pub after: String,
}

/// One affected profile, as [`compare_profiles`] takes it.
#[derive(Debug, Clone)]
pub struct ProfileCase<'a> {
    pub service_id: i64,
    pub service_name: String,
    /// The download this profile serves now.
    pub before: &'a ServicePolicySet,
    /// The download it will serve after the migration (or, inside Task 7's
    /// transaction, the one it serves NOW that the write has happened).
    pub after: &'a ServicePolicySet,
    /// The profile's RESOURCE service-def (`trino` / `starrocks`).
    pub service_def: &'a ServiceDef,
    /// Super-users / super-groups as measured for this profile's plugins.
    /// `plugin_version` is overwritten per entry of [`ProfileCase::versions`];
    /// leave `engine` `None` unless every version agrees with it (the
    /// evaluator answers `EngineMismatch` otherwise, which fails closed).
    pub engine: EngineConfig,
    /// The `PluginVersion` of every engine bound to this profile.
    pub versions: Vec<PluginVersion>,
    /// Usually `derive_probes(before, after, service_def, assignments)`.
    pub probes: Vec<Probe>,
}

/// Why the gate refused to answer at all. Each is a state in which an empty
/// disagreement list would be a vacuous "equivalent".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateRefusal {
    /// [`compare_profiles`] was handed no profile.
    NoProfiles,
    /// A profile with no plugin version: nothing would be evaluated.
    NoPluginVersions { service_id: i64 },
    /// A profile with no probe: nothing would be evaluated.
    NoProbes { service_id: i64 },
}

// ── probes ───────────────────────────────────────────────────────────────

/// Whether a probe's tags are the profile's real tagging or a hypothetical one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tagging {
    /// Resolved from the [`TagAssignment`] rows the caller passed.
    Actual,
    /// The resource carries exactly one tag a policy in either set addresses,
    /// whether or not any column carries it today.
    Hypothetical,
}

/// One probe: the request evaluated against both sets, and where its tags
/// came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub request: AccessRequest,
    pub tagging: Tagging,
}

/// One `catalog_tag_assignments` row, as names: the tag's `tags.name` (never
/// `tags.key`) on `catalog.schema.table[.column]`. An empty `column` is a
/// TABLE-level assignment. `catalog` is the registry name the compiler emits.
///
/// ⚠ The resolution [`derive_probes`] performs from these rows is
/// `egret_policy::store::resolved_tags_for_resource_via` + `routes.rs`'s
/// `attachment_for`, restated: a schema/table match folded with
/// `to_lowercase`, `SELF` for the same column (or the table on a table
/// request), `ANCESTOR` for a table-level row under a column request, and a
/// column row excluded from a table request (`DESCENDANT`). The fold is only
/// that resolver's fold OUTSIDE the 25 code points `routes.rs`'
/// `fold_divergent` names, so the caller must run that gate first — over the
/// stored rows and over the policies' resource values, which are where every
/// probe coordinate's values come from (the synthetic names are ASCII) — and
/// refuse the migration on a decline, as `explain` declines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagAssignment {
    pub catalog: String,
    pub schema: String,
    pub table: String,
    pub column: String,
    pub tag: String,
}

/// Probes derived from BOTH policy sets, plus probes neither mentions.
///
/// The probe set is the cross product of three axes:
///
/// * **Principals** — every user named by any item of any policy (resource and
///   tag, every bucket) in either set; one synthetic member per named group and
///   one synthetic holder per named role; and [`PROBE_UNNAMED_USER`], whom no
///   policy names. Ranger macros (`{USER}`, `{OWNER}`) are not principals and
///   are skipped. Each synthetic member carries exactly ONE group — sound only
///   because no egret engine receives groups at all today (D7, see
///   `members.rs`' module doc); a group provider would make a user-exception
///   on a group item un-probed.
/// * **Access types** — every access type the service-def declares (so one
///   that neither set mentions is always probed, which is what catches an
///   `Action` expansion that ADDED an access) plus any either set mentions.
///   `_any` is not probed: the evaluator answers it `PseudoAccessType`.
/// * **Coordinates** — for each resource map of each policy in either set, and
///   for each of `assignments`: along the longest service-def hierarchy that
///   holds its levels, every combination of its values with `*`/absent levels
///   filled by a [`PROBE_ANY_PREFIX`] name; every prefix of that path (the
///   engines ask at every depth); and at each level a [`PROBE_SIBLING_PREFIX`]
///   sibling under the same parents, both cut at that level and continued to
///   full depth with the original values below.
///
/// Every probe carries the tags `assignments` resolve to on its resource
/// ([`Tagging::Actual`], `Some(empty)` where none). Then every coordinate that
/// names a table is probed once more per tag name either set's tag policies
/// address, carrying only that tag at `SELF` ([`Tagging::Hypothetical`]).
/// `owner` is always `None`.
pub fn derive_probes(
    before: &ServicePolicySet,
    after: &ServicePolicySet,
    service_def: &ServiceDef,
    assignments: &[TagAssignment],
) -> Vec<Probe> {
    let policies: Vec<&RangerPolicy> = [before, after]
        .iter()
        .flat_map(|s| s.policies.iter().chain(s.tag_policies.iter()))
        .collect();

    let principals = principals(&policies);
    let access_types = access_types(&policies, service_def);
    let schema_level = if service_def.resource(PolicyType::Access, "schema").is_some() {
        "schema"
    } else {
        "database"
    };
    let assigned: Vec<RequestResource> = assignments
        .iter()
        .map(|a| {
            let mut r = RequestResource::new()
                .with("catalog", a.catalog.clone())
                .with(schema_level, a.schema.clone())
                .with("table", a.table.clone());
            if !a.column.is_empty() {
                r.insert("column", a.column.clone());
            }
            r
        })
        .collect();
    let coordinates = coordinates(&policies, service_def, &assigned);
    let tag_names = tag_names(before, after);

    let mut out = Vec::new();
    for coordinate in &coordinates {
        let resource: RequestResource = coordinate.iter().cloned().collect();
        let mut taggings = vec![(Tagging::Actual, resolve_tags(&resource, assignments))];
        if table_of(&resource).is_some() {
            for name in &tag_names {
                taggings.push((
                    Tagging::Hypothetical,
                    vec![attachment(name, MatchType::Self_, &resource)],
                ));
            }
        }
        for (tagging, tags) in taggings {
            let tags = UnenrichedTags::new(tags);
            for (user, groups, roles) in &principals {
                for access_type in &access_types {
                    out.push(Probe {
                        request: AccessRequest {
                            user: user.clone(),
                            groups: groups.clone(),
                            roles: roles.clone(),
                            access_type: access_type.clone(),
                            resource: resource.clone(),
                            owner: None,
                            resolved_tags: Some(tags.clone()),
                        },
                        tagging,
                    });
                }
            }
        }
    }
    out
}

/// `(catalog, schema, table, column)` of a request, `column` `""` when the
/// request stops at the table; `None` when it names no table.
fn table_of(r: &RequestResource) -> Option<(&str, &str, &str, &str)> {
    let catalog = r.get("catalog")?;
    let schema = r.get("schema").or_else(|| r.get("database"))?;
    let table = r.get("table")?;
    Some((catalog, schema, table, r.get("column").unwrap_or("")))
}

/// The tags `assignments` put on `r` — see [`TagAssignment`] for whose rule
/// this restates.
fn resolve_tags(r: &RequestResource, assignments: &[TagAssignment]) -> Vec<ResolvedTag> {
    let Some((catalog, schema, table, column)) = table_of(r) else {
        return Vec::new();
    };
    let fold = |s: &str| s.to_lowercase();
    let mut seen: BTreeSet<(bool, String)> = BTreeSet::new();
    for a in assignments {
        if a.catalog == catalog
            && fold(&a.schema) == fold(schema)
            && fold(&a.table) == fold(table)
            && (fold(&a.column) == fold(column) || a.column.is_empty())
        {
            seen.insert((fold(&a.column) != fold(column), a.tag.clone()));
        }
    }
    // `is_self DESC, name` — the resolver's ORDER BY.
    seen.into_iter()
        .map(|(ancestor, name)| {
            let mt = if ancestor {
                MatchType::Ancestor
            } else {
                MatchType::Self_
            };
            attachment(&name, mt, r)
        })
        .collect()
}

/// One attachment as `attachment_for` shapes it: `catalog`, `schema`, `table`,
/// and `column` only for a `SELF` attachment on a column request.
fn attachment(name: &str, match_type: MatchType, r: &RequestResource) -> ResolvedTag {
    let column = r.get("column").is_some_and(|c| !c.is_empty());
    let levels: BTreeSet<String> = ["catalog", "schema", "table"]
        .into_iter()
        .chain((column && match_type == MatchType::Self_).then_some("column"))
        .map(str::to_owned)
        .collect();
    ResolvedTag {
        name: name.to_string(),
        match_type,
        attributes: BTreeMap::new(),
        options: BTreeSet::new(),
        validity_periods: 0,
        attached_through: TagServiceResource {
            levels,
            is_enabled: true,
        },
    }
}

/// Every tag name either set's tag policies address; a pure wildcard value
/// becomes a name nobody wrote (it matches any tag).
fn tag_names(before: &ServicePolicySet, after: &ServicePolicySet) -> BTreeSet<String> {
    [before, after]
        .iter()
        .flat_map(|s| s.tag_policies.iter())
        .filter_map(|p| p.resources.get("tag"))
        .flat_map(|l| l.values.iter())
        .map(|v| {
            if v.is_empty() || v.bytes().all(|b| b == b'*') {
                format!("{PROBE_ANY_PREFIX}tag")
            } else {
                v.clone()
            }
        })
        .collect()
}

type Principal = (String, BTreeSet<String>, BTreeSet<String>);

fn all_items(p: &RangerPolicy) -> impl Iterator<Item = &PolicyItem> {
    p.policy_items
        .iter()
        .chain(&p.deny_policy_items)
        .chain(&p.allow_exceptions)
        .chain(&p.deny_exceptions)
        .chain(&p.data_mask_policy_items)
        .chain(&p.row_filter_policy_items)
}

fn is_macro(name: &str) -> bool {
    name.starts_with('{')
}

fn principals(policies: &[&RangerPolicy]) -> Vec<Principal> {
    let mut users = BTreeSet::new();
    let mut groups = BTreeSet::new();
    let mut roles = BTreeSet::new();
    for item in policies.iter().flat_map(|p| all_items(p)) {
        users.extend(item.users.iter().filter(|u| !is_macro(u)).cloned());
        groups.extend(item.groups.iter().filter(|g| !is_macro(g)).cloned());
        roles.extend(item.roles.iter().filter(|r| !is_macro(r)).cloned());
    }
    let mut out: Vec<Principal> = users
        .into_iter()
        .map(|u| (u, BTreeSet::new(), BTreeSet::new()))
        .collect();
    for g in groups {
        out.push((
            format!("{PROBE_GROUP_MEMBER_PREFIX}{g}"),
            BTreeSet::from([g]),
            BTreeSet::new(),
        ));
    }
    for r in roles {
        out.push((
            format!("{PROBE_ROLE_HOLDER_PREFIX}{r}"),
            BTreeSet::new(),
            BTreeSet::from([r]),
        ));
    }
    out.push((
        PROBE_UNNAMED_USER.to_string(),
        BTreeSet::new(),
        BTreeSet::new(),
    ));
    out
}

fn access_types(policies: &[&RangerPolicy], def: &ServiceDef) -> Vec<String> {
    let mut out: BTreeSet<String> = def.access_types().iter().map(|a| a.name.clone()).collect();
    for item in policies.iter().flat_map(|p| all_items(p)) {
        out.extend(item.accesses.iter().map(|a| a.access_type.clone()));
    }
    out.into_iter().collect()
}

/// A coordinate: `(level, value)` in hierarchy order.
type Coordinate = Vec<(String, String)>;

/// One resource map as `(level, values)` pairs.
type LevelValues = Vec<(String, Vec<String>)>;

fn coordinates(
    policies: &[&RangerPolicy],
    def: &ServiceDef,
    tagged: &[RequestResource],
) -> BTreeSet<Coordinate> {
    // Every (level -> values) map to expand: each policy's resources and
    // additional resources (tag-addressed maps carry tag NAMES, not
    // coordinates, and are skipped), and each tagged coordinate.
    let mut maps: Vec<(PolicyType, LevelValues)> = Vec::new();
    for p in policies {
        let policy_type = PolicyType::from_code(p.policy_type);
        for resources in std::iter::once(&p.resources).chain(&p.additional_resources) {
            if resources.is_empty() || resources.contains_key("tag") {
                continue;
            }
            maps.push((policy_type, levels_of(resources)));
        }
    }
    for t in tagged {
        let levels = t
            .keys()
            .map(|k| {
                (
                    k.to_string(),
                    vec![t.get(k).unwrap_or_default().to_string()],
                )
            })
            .collect();
        maps.push((PolicyType::Access, levels));
    }

    let mut out = BTreeSet::new();
    for (policy_type, levels) in maps {
        let chain = hierarchy_for(def, policy_type, &levels);
        // Values per level along the chain; `*` runs and absent levels become
        // a name nobody wrote.
        let per_level: Vec<Vec<String>> = chain
            .iter()
            .map(|level| {
                let named: Vec<String> = levels
                    .iter()
                    .find(|(k, _)| k == level)
                    .map(|(_, vs)| {
                        vs.iter()
                            .filter(|v| !v.is_empty() && !v.bytes().all(|b| b == b'*'))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                if named.is_empty() {
                    vec![format!("{PROBE_ANY_PREFIX}{level}")]
                } else {
                    named
                }
            })
            .collect();
        for path in cartesian(&per_level) {
            for depth in 1..=chain.len() {
                out.insert(zip(&chain[..depth], &path[..depth]));
            }
            for i in 0..chain.len() {
                let mut sibling = path.clone();
                sibling[i] = format!("{PROBE_SIBLING_PREFIX}{}", chain[i]);
                out.insert(zip(&chain[..=i], &sibling[..=i]));
                out.insert(zip(&chain, &sibling));
            }
        }
    }
    out
}

fn levels_of(resources: &std::collections::BTreeMap<String, ResourceLevel>) -> LevelValues {
    resources
        .iter()
        .map(|(k, l)| (k.clone(), l.values.clone()))
        .collect()
}

/// The longest hierarchy of `policy_type` (falling back to access) that holds
/// every level the map names; the map's own keys if none does.
fn hierarchy_for(
    def: &ServiceDef,
    policy_type: PolicyType,
    levels: &[(String, Vec<String>)],
) -> Vec<String> {
    let keys: BTreeSet<&str> = levels.iter().map(|(k, _)| k.as_str()).collect();
    for pt in [policy_type, PolicyType::Access] {
        if let Some(h) = def
            .hierarchies(pt)
            .iter()
            .filter(|h| keys.iter().all(|k| h.iter().any(|l| l == k)))
            .max_by_key(|h| h.len())
        {
            return h.clone();
        }
    }
    levels.iter().map(|(k, _)| k.clone()).collect()
}

fn cartesian(per_level: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = vec![Vec::new()];
    for values in per_level {
        out = out
            .into_iter()
            .flat_map(|prefix| {
                values.iter().map(move |v| {
                    let mut p = prefix.clone();
                    p.push(v.clone());
                    p
                })
            })
            .collect();
    }
    out
}

fn zip(levels: &[String], values: &[String]) -> Coordinate {
    levels.iter().cloned().zip(values.iter().cloned()).collect()
}

// ── comparison ───────────────────────────────────────────────────────────

/// Compare one profile's before and after on every probe, on every version in
/// `case.versions`. `Ok(empty)` = equivalent.
pub fn compare(case: &ProfileCase<'_>) -> Result<Vec<Disagreement>, GateRefusal> {
    if case.versions.is_empty() {
        return Err(GateRefusal::NoPluginVersions {
            service_id: case.service_id,
        });
    }
    if case.probes.is_empty() {
        return Err(GateRefusal::NoProbes {
            service_id: case.service_id,
        });
    }

    let mut out = Vec::new();
    for &version in &case.versions {
        let engine = EngineConfig {
            plugin_version: Some(version),
            ..case.engine.clone()
        };
        for Probe {
            request: probe,
            tagging,
        } in &case.probes
        {
            let mut push = |check: Check, before: String, after: String| {
                out.push(Disagreement {
                    service_id: case.service_id,
                    service_name: case.service_name.clone(),
                    plugin_version: version,
                    check,
                    user: probe.user.clone(),
                    groups: probe.groups.iter().cloned().collect(),
                    roles: probe.roles.iter().cloned().collect(),
                    tags: probe
                        .resolved_tags
                        .as_ref()
                        .map(|t| t.as_slice().iter().map(|t| t.name.clone()).collect())
                        .unwrap_or_default(),
                    tagging: *tagging,
                    access_type: probe.access_type.clone(),
                    resource: render_resource(&probe.resource, case.service_def),
                    before,
                    after,
                });
            };

            let b = evaluate(case.before, probe, case.service_def, &engine);
            let a = evaluate(case.after, probe, case.service_def, &engine);
            if !same_access(&b, &a) {
                push(Check::Access, render_decision(&b), render_decision(&a));
            }

            let b = evaluate_data_mask(case.before, probe, case.service_def, &engine);
            let a = evaluate_data_mask(case.after, probe, case.service_def, &engine);
            if !same_mask(&b, &a) {
                push(Check::DataMask, render_mask(&b), render_mask(&a));
            }

            let b = evaluate_row_filter(case.before, probe, case.service_def, &engine);
            let a = evaluate_row_filter(case.after, probe, case.service_def, &engine);
            if !same_filter(&b, &a) {
                push(Check::RowFilter, render_filter(&b), render_filter(&a));
            }
        }
    }
    Ok(out)
}

/// [`compare`] over every affected profile. `Ok(empty)` = the migration
/// changes no decision on any of them; otherwise every disagreement, each
/// naming its profile. Refuses (rather than answering "equivalent") when
/// handed no profile, or a profile with no version or no probe.
pub fn compare_profiles(cases: &[ProfileCase<'_>]) -> Result<Vec<Disagreement>, GateRefusal> {
    if cases.is_empty() {
        return Err(GateRefusal::NoProfiles);
    }
    let mut out = Vec::new();
    for case in cases {
        out.extend(compare(case)?);
    }
    Ok(out)
}

fn same_access(b: &Decision, a: &Decision) -> bool {
    match (b, a) {
        (Decision::Allowed { .. }, Decision::Allowed { .. }) => true,
        (Decision::Denied { reason: rb, .. }, Decision::Denied { reason: ra, .. }) => rb == ra,
        _ => false,
    }
}

fn same_mask(b: &MaskDecision, a: &MaskDecision) -> bool {
    match (b, a) {
        (MaskDecision::NoMask, MaskDecision::NoMask) => true,
        (MaskDecision::Masked(mb), MaskDecision::Masked(ma)) => {
            mb.mask_type == ma.mask_type
                && mb.masked_value == ma.masked_value
                && mb.mask_condition == ma.mask_condition
        }
        _ => false,
    }
}

fn same_filter(b: &RowFilterDecision, a: &RowFilterDecision) -> bool {
    match (b, a) {
        (RowFilterDecision::NoFilter, RowFilterDecision::NoFilter) => true,
        (RowFilterDecision::Filtered(fb), RowFilterDecision::Filtered(fa)) => {
            fb.filter_expr == fa.filter_expr
        }
        _ => false,
    }
}

// ── rendering ────────────────────────────────────────────────────────────

/// The catalog chain, in either engine's spelling. A coordinate whose levels
/// are a prefix of one of these renders dotted.
const CATALOG_CHAINS: [[&str; 4]; 2] = [
    ["catalog", "schema", "table", "column"],
    ["catalog", "database", "table", "column"],
];

fn render_resource(r: &RequestResource, def: &ServiceDef) -> String {
    let keys: Vec<&str> = r.keys().collect();
    for chain in CATALOG_CHAINS {
        let prefix = &chain[..keys.len().min(4)];
        if keys.len() <= 4 && prefix.iter().all(|k| keys.contains(k)) {
            return prefix
                .iter()
                .map(|k| r.get(k).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(".");
        }
    }
    // Service-def order, then anything the def does not declare.
    let mut ordered: Vec<&str> = def
        .resources(PolicyType::Access)
        .iter()
        .map(|d| d.name.as_str())
        .filter(|n| keys.contains(n))
        .collect();
    let undeclared: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|k| !ordered.contains(k))
        .collect();
    ordered.extend(undeclared);
    ordered
        .iter()
        .map(|k| format!("{k}={}", r.get(k).unwrap_or_default()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn by(policy_name: &str) -> String {
    format!("(policy \"{policy_name}\")")
}

/// The variant names of an `Undecidable`'s causes — the full `Debug` of a
/// cause can run to several hundred characters; the name is what an operator
/// can look up.
fn render_causes<T: std::fmt::Debug>(why: &[T]) -> String {
    let names: Vec<String> = why
        .iter()
        .map(|c| {
            let s = format!("{c:?}");
            s.split([' ', '{', '('])
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    format!("cannot be decided: {}", names.join(", "))
}

fn render_decision(d: &Decision) -> String {
    match d {
        Decision::Allowed { policy_name, .. } => format!("allowed {}", by(policy_name)),
        Decision::Denied {
            policy_name,
            reason,
            ..
        } => match reason {
            DenyReason::DenyPolicyItem => format!("denied by a deny item {}", by(policy_name)),
            DenyReason::DenyAllElse => format!("denied: deny-all-else {}", by(policy_name)),
            DenyReason::NoPolicyMatched => "denied: no policy matched".to_string(),
        },
        Decision::Undecidable { why } => render_causes(why),
    }
}

fn render_mask_result(m: &MaskResult) -> String {
    let mut s = format!(
        "masked {}",
        m.mask_type.as_deref().unwrap_or("(no mask type)")
    );
    if let Some(v) = &m.masked_value {
        s.push_str(&format!(" value \"{v}\""));
    }
    if let Some(c) = &m.mask_condition {
        s.push_str(&format!(" when \"{c}\""));
    }
    format!("{s} {}", by(&m.policy_name))
}

fn render_mask(d: &MaskDecision) -> String {
    match d {
        MaskDecision::NoMask => "no mask".to_string(),
        MaskDecision::Masked(m) => render_mask_result(m),
        MaskDecision::Undecidable { why } => render_causes(why),
    }
}

fn render_filter(d: &RowFilterDecision) -> String {
    match d {
        RowFilterDecision::NoFilter => "no row filter".to_string(),
        RowFilterDecision::Filtered(f) => {
            format!("filtered by \"{}\" {}", f.filter_expr, by(&f.policy_name))
        }
        RowFilterDecision::Undecidable { why } => render_causes(why),
    }
}
