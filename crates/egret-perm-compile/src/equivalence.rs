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
//! | access (`evaluate`) | both `Allowed`, or both `Denied` for the same [`egret_ranger_eval::eval::DenyReason`] | either side `Undecidable` |
//! | mask (`evaluate_data_mask`) | both `NoMask`, or both `Masked` with equal `mask_type`, `masked_value` and `mask_condition` | either side `Undecidable` |
//! | row filter (`evaluate_row_filter`) | both `NoFilter`, or both `Filtered` with an equal `filter_expr` | either side `Undecidable` |
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
    evaluate_data_mask_prepared, evaluate_prepared, evaluate_row_filter_prepared, may_be_candidate,
    may_be_tag_candidate, AccessRequest, Decision, DenyReason, EngineConfig, MaskDecision,
    MaskResult, PluginVersion, PreparedPolicySet, ResolvedTag, RowFilterDecision,
    TagServiceResource, UnenrichedTags,
};
use egret_ranger_eval::model::{
    PolicyItem, RangerPolicy, ResourceLevel, ServicePolicySet, TagPolicyBlock, Unmodelled,
};
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

/// `Ranger 2.5.0` / `Ranger 2.8.0` / `Ranger 2.9.0` — how a version reads on the page.
pub fn plugin_version_label(v: PluginVersion) -> &'static str {
    match v {
        PluginVersion::Ranger2_5_0 => "Ranger 2.5.0",
        PluginVersion::Ranger2_8_0 => "Ranger 2.8.0",
        PluginVersion::Ranger2_9_0 => "Ranger 2.9.0",
    }
}

impl std::fmt::Display for Check {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Check::Access => "access",
            Check::DataMask => "masking",
            Check::RowFilter => "row filter",
        })
    }
}

/// One line an operator can read, naming everything the disagreement carries:
/// profile, plugin version, question, principal (with the synthetic member's
/// group or role), access type, resource, tags and whether that tagging is
/// today's, and both answers. E.g.
///
/// `profile "trino-prod" (#7), Ranger 2.5.0: access for user alice, select on
/// wh.sales.orders.email: allowed (policy "sales read") → denied (no policy
/// matched)`
impl std::fmt::Display for Disagreement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "profile \"{}\" (#{}), {}: {} for user {}",
            self.service_name,
            self.service_id,
            plugin_version_label(self.plugin_version),
            self.check,
            self.user
        )?;
        if !self.groups.is_empty() {
            write!(f, " (in group {})", self.groups.join(", "))?;
        }
        if !self.roles.is_empty() {
            write!(f, " (holding role {})", self.roles.join(", "))?;
        }
        write!(f, ", {} on {}", self.access_type, self.resource)?;
        match (self.tagging, self.tags.is_empty()) {
            (Tagging::Actual, false) => write!(f, " (tagged {})", self.tags.join(", "))?,
            (Tagging::Hypothetical, _) => write!(
                f,
                " (only if it were tagged {} — no column need carry it today)",
                self.tags.join(", ")
            )?,
            (Tagging::Actual, true) => {}
        }
        write!(f, ": {} → {}", self.before, self.after)
    }
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
    ProbeFactors::new(before, after, service_def, assignments).build(assignments)
}

/// How many probes [`derive_probes`] would build for these inputs, computed
/// from the factors WITHOUT building any probe (Task 7 review, I-1): the
/// migration refuses over its cap before it spends the memory and the time.
/// Always equal to `derive_probes(..).len()` — `tests/equivalence.rs` pins it.
pub fn probe_count(
    before: &ServicePolicySet,
    after: &ServicePolicySet,
    service_def: &ServiceDef,
    assignments: &[TagAssignment],
) -> usize {
    ProbeFactors::new(before, after, service_def, assignments).count()
}

// ── pruning (SP4d-4 Task 5) ──────────────────────────────────────────────

/// Which probes [`plan_probes`] builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PruneMode {
    /// Every probe [`derive_probes`] builds (SP4d-3 behaviour).
    Full,
    /// Only probes where some changed policy may be a candidate — unless a
    /// fallback condition holds, in which case this is [`PruneMode::Full`].
    Pruned,
}

/// Why [`PruneMode::Pruned`] gave the full probe set instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// `E(B) != E(A)`: something in the download other than the two policy
    /// lists differs (service-def, tag service-def, `serviceName`/`serviceId`,
    /// envelope-level constructs). The lemma's other input moved.
    EnvelopeDiffers,
    /// Either side carries an envelope-level construct the evaluator does not
    /// model (zones, deltas, `serviceConfig`, an unreadable tag block). Every
    /// decision is then `Undecidable` on both sides and pruning would only
    /// change WHICH of them are reported.
    EnvelopeUnmodelled,
    /// Either side holds a policy carrying a construct the reader keeps only
    /// as a content-free marker (`conditions`, `validitySchedules`, item
    /// `conditions`, `delegateAdmin`, a mask `conditionExpr`). Δ now compares
    /// raw bodies and sees their content, but the evaluator cannot decide
    /// around them, so the gate keeps asking every question.
    ContentFreeConstruct,
    /// No policy differs. Nothing to prune towards; the gate keeps asking
    /// every question, as before.
    EmptyDelta,
    /// A changed policy has `isDenyAllElse: true`.
    DenyAllElseInDelta,
    /// A changed policy has a non-zero `policyPriority`.
    NonZeroPriorityInDelta,
}

/// The probes to evaluate, and how they were chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbePlan {
    pub probes: Vec<Probe>,
    /// What [`probe_count`] says the FULL set would be.
    pub full_count: usize,
    /// `Some` when [`PruneMode::Pruned`] fell back to the full set.
    pub fallback: Option<FallbackReason>,
}

/// Δ: the policies whose `(name, body without id)` differs between the two
/// sets, or that exist on one side only — BOTH versions of a changed policy.
/// Resource policies first, then tag policies.
///
/// Bodies are compared as values — the raw body and the typed fields — never
/// through a rendered string: the stored body is `jsonb`, which reorders keys, and two equivalent
/// bodies must not read as "changed".
pub fn changed_policies<'a>(
    before: &'a ServicePolicySet,
    after: &'a ServicePolicySet,
) -> Vec<&'a RangerPolicy> {
    let (resource, tag) = delta(before, after);
    resource.into_iter().chain(tag).collect()
}

/// Bookkeeping Ranger (or the store) stamps on a body; none of it reaches a
/// decision, and all of it moves on a rewrite.
const POLICY_METADATA: [&str; 7] = [
    "id",
    "version",
    "guid",
    "createTime",
    "updateTime",
    "createdBy",
    "updatedBy",
];

fn comparable_source(p: &RangerPolicy) -> serde_json::Value {
    let mut v = p.source.clone();
    if let Some(o) = v.as_object_mut() {
        for k in POLICY_METADATA {
            o.remove(k);
        }
    }
    v
}

/// Equal `(name, body)`. The body is compared as the RAW policy value
/// (`RangerPolicy::source`, map equality is order-insensitive) AND as the typed
/// fields: the reader reduces `conditions`, `validitySchedules`, item
/// `conditions`, `delegateAdmin` and a mask `conditionExpr` to content-free
/// markers, so the typed fields alone cannot see a change of their content.
fn same_policy(a: &RangerPolicy, b: &RangerPolicy) -> bool {
    a.name == b.name && comparable_source(a) == comparable_source(b) && {
        let (mut a, mut b) = (a.clone(), b.clone());
        a.id = None;
        b.id = None;
        a.source = serde_json::Value::Null;
        b.source = serde_json::Value::Null;
        a == b
    }
}

/// Whether the reader kept only a content-free marker for something in this
/// policy (see [`FallbackReason::ContentFreeConstruct`]).
fn has_content_free_marker(p: &RangerPolicy) -> bool {
    p.unmodelled.iter().any(|c| {
        matches!(
            c,
            Unmodelled::PolicyConditions
                | Unmodelled::ValiditySchedules
                | Unmodelled::ItemConditions { .. }
                | Unmodelled::DelegateAdmin { .. }
                | Unmodelled::MaskConditionExpr { .. }
        )
    })
}

/// Policies of `before` with no equal in `after`, then policies of `after`
/// with no equal in `before` (multiset difference on [`same_policy`]).
fn list_delta<'a>(before: &'a [RangerPolicy], after: &'a [RangerPolicy]) -> Vec<&'a RangerPolicy> {
    let mut unmatched_after: Vec<&RangerPolicy> = after.iter().collect();
    let mut out = Vec::new();
    for b in before {
        match unmatched_after.iter().position(|a| same_policy(a, b)) {
            Some(i) => {
                unmatched_after.remove(i);
            }
            None => out.push(b),
        }
    }
    out.extend(unmatched_after);
    out
}

fn delta<'a>(
    before: &'a ServicePolicySet,
    after: &'a ServicePolicySet,
) -> (Vec<&'a RangerPolicy>, Vec<&'a RangerPolicy>) {
    (
        list_delta(&before.policies, &after.policies),
        list_delta(&before.tag_policies, &after.tag_policies),
    )
}

/// The envelope-level constructs that are not just the tag block's policy
/// COUNT (which is derived from `tag_policies` and so belongs to Δ).
fn envelope_constructs(set: &ServicePolicySet) -> Vec<&Unmodelled> {
    set.unmodelled
        .iter()
        .filter(|c| !matches!(c, Unmodelled::TagPolicies(TagPolicyBlock::Policies(_))))
        .collect()
}

/// `E(B) == E(A)`: everything but the two policy lists (and the download's
/// `policyVersion`, which every write bumps).
fn envelopes_equal(before: &ServicePolicySet, after: &ServicePolicySet) -> bool {
    before.service_name == after.service_name
        && before.service_id == after.service_id
        && before.service_def == after.service_def
        && before.tag_service_def == after.tag_service_def
        && envelope_constructs(before) == envelope_constructs(after)
}

/// The probes for one profile, optionally pruned to where a changed policy may
/// matter.
///
/// # The argument (full version: design spec Part D, `2026-10-11-sp4d-4-closing-the-review-design.md`)
///
/// Ranger's access, mask and row-filter decisions for a request `r` are a
/// function of the envelope `E`, `r`, the engine config, and the ORDERED list
/// of policies in Ranger's candidate lists for `r` (the resource trie's answer
/// and the tag trie's answer for `r`'s resolved tags). A policy in neither list
/// is never invoked. Let Δ be [`changed_policies`]. If `E(B) == E(A)` and no
/// policy in Δ may be a candidate for `r`, the candidate lists on both sides
/// consist of byte-identical policies in the same relative order, so `B` and
/// `A` decide `r` identically — such a probe cannot disagree and is dropped.
/// "May be a candidate" OVER-approximates Ranger's list:
/// [`egret_ranger_eval::eval::may_be_candidate`] for a resource policy (a
/// non-`NONE` match type, or the trie fails to exclude it — asked under all
/// three policy types, because a probe asks all three questions) and
/// [`egret_ranger_eval::eval::may_be_tag_candidate`] for a tag policy against
/// each tag in the probe's resolved tags, actual or hypothetical.
///
/// Only coordinates (with their tagging) are pruned; every principal and every
/// access type is kept.
///
/// # Fallbacks to the full set ([`FallbackReason`])
///
/// 1. `E(B) != E(A)` ([`FallbackReason::EnvelopeDiffers`]), or either side has
///    an unmodelled envelope construct ([`FallbackReason::EnvelopeUnmodelled`]).
/// 2. A Δ policy with `isDenyAllElse: true`
///    ([`FallbackReason::DenyAllElseInDelta`]) or a non-zero `policyPriority`
///    ([`FallbackReason::NonZeroPriorityInDelta`]) — not needed by the theorem,
///    but cheap, and the SP4d-3 grammar produces neither.
/// 3. Δ empty ([`FallbackReason::EmptyDelta`]).
/// 4. Any policy on either side with a content-free marker
///    ([`FallbackReason::ContentFreeConstruct`]).
///
/// A pruned set that is empty while Δ is not is returned empty with no
/// fallback: [`compare`] then refuses with [`GateRefusal::NoProbes`], never
/// "equivalent".
pub fn plan_probes(
    before: &ServicePolicySet,
    after: &ServicePolicySet,
    service_def: &ServiceDef,
    assignments: &[TagAssignment],
    mode: PruneMode,
) -> ProbePlan {
    let factors = ProbeFactors::new(before, after, service_def, assignments);
    let full_count = factors.count();
    let (fallback, keep) = pruning(before, after, service_def, mode);
    let probes = match &keep {
        Some(keep) => factors.build_where(assignments, &|r, t| keep.admits(r, t)),
        None => factors.build(assignments),
    };
    ProbePlan {
        probes,
        full_count,
        fallback,
    }
}

/// The count `plan_probes(..).probes.len()` would have, without building.
pub fn planned_probe_count(
    before: &ServicePolicySet,
    after: &ServicePolicySet,
    service_def: &ServiceDef,
    assignments: &[TagAssignment],
    mode: PruneMode,
) -> usize {
    let factors = ProbeFactors::new(before, after, service_def, assignments);
    match pruning(before, after, service_def, mode).1 {
        Some(keep) => factors.count_where(assignments, &|r, t| keep.admits(r, t)),
        None => factors.count(),
    }
}

/// The fallback taken (if any) and, when pruning is on, the keep predicate.
fn pruning<'a>(
    before: &'a ServicePolicySet,
    after: &'a ServicePolicySet,
    service_def: &'a ServiceDef,
    mode: PruneMode,
) -> (Option<FallbackReason>, Option<Keep<'a>>) {
    if mode == PruneMode::Full {
        return (None, None);
    }
    let (resource, tag) = delta(before, after);
    let fallback = if !envelopes_equal(before, after) {
        Some(FallbackReason::EnvelopeDiffers)
    } else if !envelope_constructs(before).is_empty() {
        Some(FallbackReason::EnvelopeUnmodelled)
    } else if [before, after]
        .iter()
        .flat_map(|set| set.policies.iter().chain(&set.tag_policies))
        .any(has_content_free_marker)
    {
        Some(FallbackReason::ContentFreeConstruct)
    } else if resource.is_empty() && tag.is_empty() {
        Some(FallbackReason::EmptyDelta)
    } else if resource.iter().chain(&tag).any(|p| p.is_deny_all_else) {
        Some(FallbackReason::DenyAllElseInDelta)
    } else if resource.iter().chain(&tag).any(|p| p.policy_priority != 0) {
        Some(FallbackReason::NonZeroPriorityInDelta)
    } else {
        None
    };
    if fallback.is_some() {
        return (fallback, None);
    }
    let tag_def = ServiceDef::parse(&before.tag_service_def);
    (
        None,
        Some(Keep {
            resource,
            tag,
            service_def,
            tag_def,
        }),
    )
}

/// "Some changed policy may be a candidate" for a (coordinate, tagging).
struct Keep<'a> {
    resource: Vec<&'a RangerPolicy>,
    tag: Vec<&'a RangerPolicy>,
    service_def: &'a ServiceDef,
    tag_def: ServiceDef,
}

impl Keep<'_> {
    fn admits(&self, resource: &RequestResource, tags: &[ResolvedTag]) -> bool {
        // A probe asks the access, mask and row-filter questions, so a policy
        // is asked under each policy type's hierarchy.
        let by_resource = self.resource.iter().any(|p| {
            [
                PolicyType::Access,
                PolicyType::DataMask,
                PolicyType::RowFilter,
            ]
            .into_iter()
            .any(|pt| may_be_candidate(p, resource, self.service_def, pt))
        });
        // A tag def that cannot even name the `tag` resource proves nothing:
        // keep, as the over-approximation requires.
        let tag_def_usable = self.tag_def.resource(PolicyType::Access, "tag").is_some();
        by_resource
            || self.tag.iter().any(|p| {
                !tag_def_usable
                    || tags
                        .iter()
                        .any(|t| may_be_tag_candidate(p, &t.name, &self.tag_def))
            })
    }
}

/// The three axes of the probe cross product, plus the tag names that
/// multiply the table coordinates — computed once, then either counted or
/// built, so the count and the build cannot disagree.
struct ProbeFactors {
    principals: Vec<(String, BTreeSet<String>, BTreeSet<String>)>,
    access_types: Vec<String>,
    coordinates: Vec<Coordinate>,
    tag_names: Vec<String>,
}

impl ProbeFactors {
    fn new(
        before: &ServicePolicySet,
        after: &ServicePolicySet,
        service_def: &ServiceDef,
        assignments: &[TagAssignment],
    ) -> Self {
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
        let coordinates = coordinates(&policies, service_def, &assigned)
            .into_iter()
            .collect();
        let tag_names = tag_names(before, after).into_iter().collect();
        ProbeFactors {
            principals: principals.into_iter().collect(),
            access_types: access_types.into_iter().collect(),
            coordinates,
            tag_names,
        }
    }

    /// The taggings one coordinate is probed under: the profile's real tags,
    /// then (for a coordinate naming a table) one hypothetical tagging per tag
    /// name either set's tag policies address.
    fn taggings(
        &self,
        resource: &RequestResource,
        assignments: &[TagAssignment],
    ) -> Vec<(Tagging, Vec<ResolvedTag>)> {
        let mut taggings = vec![(Tagging::Actual, resolve_tags(resource, assignments))];
        if table_of(resource).is_some() {
            for name in &self.tag_names {
                taggings.push((
                    Tagging::Hypothetical,
                    vec![attachment(name, MatchType::Self_, resource)],
                ));
            }
        }
        taggings
    }

    /// Principals × access types: what every kept (coordinate, tagging) is
    /// multiplied by. Never pruned.
    fn per_tagging(&self) -> usize {
        self.principals.len() * self.access_types.len()
    }

    fn count(&self) -> usize {
        self.coordinates
            .iter()
            .map(|c| {
                let resource: RequestResource = c.iter().cloned().collect();
                (1 + if table_of(&resource).is_some() {
                    self.tag_names.len()
                } else {
                    0
                }) * self.per_tagging()
            })
            .sum()
    }

    /// [`ProbeFactors::count`] over the (coordinate, tagging) pairs `keep`
    /// admits, without building a probe.
    fn count_where(
        &self,
        assignments: &[TagAssignment],
        keep: &dyn Fn(&RequestResource, &[ResolvedTag]) -> bool,
    ) -> usize {
        let mut kept = 0;
        for coordinate in &self.coordinates {
            let resource: RequestResource = coordinate.iter().cloned().collect();
            kept += self
                .taggings(&resource, assignments)
                .iter()
                .filter(|(_, tags)| keep(&resource, tags))
                .count();
        }
        kept * self.per_tagging()
    }

    fn build(self, assignments: &[TagAssignment]) -> Vec<Probe> {
        self.build_where(assignments, &|_, _| true)
    }

    /// The probes at the (coordinate, tagging) pairs `keep` admits — every
    /// principal and every access type at each. Pruning is by coordinate and
    /// tagging ONLY.
    fn build_where(
        self,
        assignments: &[TagAssignment],
        keep: &dyn Fn(&RequestResource, &[ResolvedTag]) -> bool,
    ) -> Vec<Probe> {
        let mut out = Vec::new();
        for coordinate in &self.coordinates {
            let resource: RequestResource = coordinate.iter().cloned().collect();
            for (tagging, tags) in self.taggings(&resource, assignments) {
                if !keep(&resource, &tags) {
                    continue;
                }
                let tags = UnenrichedTags::new(tags);
                for (user, groups, roles) in &self.principals {
                    for access_type in &self.access_types {
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
        // View governance R5: `egret-lineage` is not a principal a migration
        // can change. The compiler alone decides its access (read on every
        // governed catalog, excepted from every deny, mask and row filter;
        // `crate::lineage`), so a profile's first compile adds that access
        // whatever is migrated, and probing it would refuse every migration on
        // a Trino profile for a difference the spec requires. Its access is
        // proved by `lineage_compile_tests` instead.
        users.extend(
            item.users
                .iter()
                .filter(|u| !is_macro(u) && u.as_str() != crate::lineage::LINEAGE_PRINCIPAL)
                .cloned(),
        );
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

    // Each side prepared ONCE for every probe and every version (Task 7 review
    // I-5): the evaluator's per-set work — one matcher per policy, the
    // implied grants, the parsed tag def — does not depend on the request or
    // on the engine config, and `egret-ranger-eval`'s `tests/differential.rs`
    // proves the prepared path decides exactly as the unprepared one.
    let before = PreparedPolicySet::new(case.before, case.service_def);
    let after = PreparedPolicySet::new(case.after, case.service_def);
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

            let b = evaluate_prepared(&before, probe, &engine);
            let a = evaluate_prepared(&after, probe, &engine);
            if !same_access(&b, &a) {
                push(Check::Access, render_decision(&b), render_decision(&a));
            }

            let b = evaluate_data_mask_prepared(&before, probe, &engine);
            let a = evaluate_data_mask_prepared(&after, probe, &engine);
            if !same_mask(&b, &a) {
                push(Check::DataMask, render_mask(&b), render_mask(&a));
            }

            let b = evaluate_row_filter_prepared(&before, probe, &engine);
            let a = evaluate_row_filter_prepared(&after, probe, &engine);
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
