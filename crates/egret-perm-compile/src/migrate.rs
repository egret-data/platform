//! SP4d-3 Task 7: migrate a hand-written Ranger policy into the permission
//! model — provably, or not at all — and undo it.
//!
//! egret IS the Ranger Admin: a row in `policy.authz_policies` is the decision
//! the engine enforces. So this module's write path is not bookkeeping; it
//! changes what Trino and StarRocks download, and the claim it makes ("the
//! engines decide exactly as before") is checked against what they will
//! download AFTER the write, inside the same transaction, before it commits.
//!
//! # One transaction, in this order — the order is the design
//!
//! 0. **Re-classify inside the transaction** ([`ServiceInputs`], the
//!    classifier's own loader — the client's view of "migratable" is never
//!    trusted), refuse in the order below, compute the AFFECTED profiles with
//!    the compiler's own `affected_services_for_grant` / `_rule`, lock them,
//!    and read each one's download as it is served NOW.
//! 1. Switch the original off — the `enabled` COLUMN, through
//!    `egret_policy::set_policy_enabled_via`, the store's guarded flip. First
//!    (SP4d-4 Task 3 fix round 1), so the recompile's overlap check, which
//!    reads every enabled hand-written mask/row filter of the profile, does
//!    not pair the original with its own compiled twin — while every OTHER
//!    hand-written policy still on is checked against the new rule.
//! 2. Insert the grants / rules the grammar produced — through
//!    [`crate::routes::write_grant`] / [`crate::routes::write_rule`], the one
//!    implementation of a model write, so every authoring guard those carry
//!    (`validate_grant_shape`, the unreadable-table-mask refusal, …) applies to
//!    a migration exactly as it does to a hand-authored grant.
//! 3. Recompile — the same calls recompile every profile each object affects
//!    (`recompile_services_in_tx`), stamping them `generated`.
//! 4. Insert provenance (`perm.migrated_from`); its `UNIQUE` is what makes a
//!    racing second migration a 409.
//! 5. **Re-read every affected profile's download and run the equivalence gate
//!    over EVERY affected profile** ([`equivalence::compare_profiles`]). Any
//!    disagreement, or any refusal to answer, and the caller rolls back.
//! 6. Record the number of probe evaluations that agreed on the provenance
//!    rows.
//!
//! The caller ([`crate::routes`]' `migrate` handler) then commits **iff** the
//! gate agreed and `dry_run` is false. That is the ONLY place a dry run
//! differs: it runs every step above, and always rolls back.
//!
//! # Every affected profile (user ruling, 2026-10-11)
//!
//! A migrated grant on catalog X compiles into EVERY profile whose engine
//! serves X, and a tag rule into every compiler-targeted profile. So the set is
//! the union of `affected_services_for_grant` / `_rule` over the inserted
//! objects, plus the profiles whose download carries the ORIGINAL (the source
//! profile, or — for a tag service — every engine-bound profile naming it as
//! its `tagService`). One [`ProfileCase`] is built per member of that set,
//! directly from it, and the count is asserted ([`build_cases`]).
//!
//! **A member with no bound engine is REFUSED, not skipped.** By construction
//! none exists (every producer of the set joins `engine_mgmt.engines`), so the
//! state means the set was computed by something else — and the gate has no
//! plugin version to evaluate it on. Evaluating it on an assumed version would
//! be a claim about an engine nobody runs; skipping it would make "every
//! affected profile" false in silence. A refusal names the profile.
//!
//! # Preconditions the gate's own soundness rests on, ENFORCED here
//!
//! - **Fold divergence** (`routes::stored_coordinate_is_fold_divergent` /
//!   `fold_divergent`): `derive_probes` folds schema/table/column with
//!   `to_lowercase`, which agrees with the SQL resolver's `lower()` and the
//!   engine's `equalsIgnoreCase` only outside 25 code points. Every
//!   `catalog_tag_assignments` row of every affected profile and every
//!   resource value of every policy in either download is checked; one hit
//!   refuses the migration, naming the value — the way `explain` declines.
//! - **Pruned probes** (SP4d-4): the dry run and the real migration build every
//!   case with [`PruneMode::Pruned`] — only the probes where a changed policy
//!   may be a candidate — and COUNT the pruned set before building it. Any
//!   fallback the pruning takes (an envelope difference, a priority, a
//!   content-free construct…) gives the full set, and the same cap then
//!   applies to that bigger count: refused, never truncated. The response says
//!   per profile how many probes the full set would have been
//!   (`probes_full`) and why pruning fell back (`pruning_fallback`).
//! - **The probe budget** ([`MAX_PROBE_WORK`]): hypothetical tagging
//!   multiplies the probe set inside a transaction that holds profile locks.
//!   Over the cap the migration is REFUSED with the measured count — never
//!   truncated, because a truncated probe set is a smaller claim reported as
//!   the whole one.
//! - **No vacuous comparison** ([`build_cases`]): each case's `before` and
//!   `after` are distinct objects read at distinct times, and every download
//!   that carried a migrated original before must not carry it after — if it
//!   still does, the "after" is not the written state.
//!
//! # A consequence to state plainly
//!
//! After a migration every affected profile is `generated`. Editing and
//! deleting the hand-written policies left behind still works (those guards
//! are policy-scoped), but CREATING a new hand-written policy in that profile
//! is refused from then on (`create_policy` is profile-scoped — SP4a's D4).
//! The response carries that sentence ([`GENERATED_NOTICE`]) so the preview
//! says it before the operator commits.
//!
//! A first recompile of a never-compiled profile also brings in everything
//! else the model already holds for it (other grants on its catalogs, tag
//! rules, clearances). The gate sees those too — they are in the after
//! download — so such a migration is refused with the disagreements they
//! cause, rather than committed with effects nobody asked for.

use std::collections::{BTreeMap, BTreeSet};

use egret_policy::{
    catalogs_for_service_via, get_service_by_id_via, set_policy_enabled_via,
    tag_assignments_for_catalogs_via, PolicyStoreError, ServiceRow,
};
use egret_ranger_eval::eval::{EngineConfig, PluginVersion};
use egret_ranger_eval::model::ServicePolicySet;
use egret_ranger_eval::servicedef::ServiceDef;
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::classify::{Candidates, ServiceInputs};
use crate::equivalence::{
    compare_profiles, plan_probes, planned_probe_count, Check, Disagreement, GateRefusal,
    ProfileCase, PruneMode, TagAssignment, Tagging,
};
use crate::explain_wire::WirePluginVersion;
use crate::grammar::ServiceKind;
use crate::routes::{
    artifact_via, fold_divergent, stored_coordinate_is_fold_divergent, write_grant, write_rule,
    CompileResponse, NewGrant, NewRule, PermHttpError,
};
use crate::trigger::{
    affected_services_for_grant, affected_services_for_rule, lock_services_in_tx,
    merge_service_ids, recompile_services_in_tx, CompileSummary,
};

/// The most gate WORK one migration may run, in probe evaluations × policies
/// (Σ over every affected profile of `probes × plugin versions × (policies the
/// profile serves before + after)`, resource and tag policies both). Each
/// evaluation asks the access, masking and row-filter questions of both
/// downloads — six evaluator calls, each visiting every policy — inside a
/// transaction holding every affected profile's lock.
///
/// **Why the unit is not a probe count.** SP4d-3 capped the probe count
/// (50 000), measured on the UNPRUNED set. With pruning (SP4d-4) the count a
/// profile needs no longer tracks its time: the same ~5 000 probes take 0.12 s
/// in a 5-policy profile and 1.06 s in an 80-policy one, because each probe
/// visits every policy. Time does track probes × policies (about 1.1 µs per
/// unit), so that is what is capped.
///
/// Measured 2026-10-13, `--release`, one Trino profile of N single-user
/// policies on distinct tables, k = 1 changed policy, `compare` over the
/// PRUNED set on one version (`tests/gate_timing.rs`, `-- --ignored`):
///
/// | N | tag mask | probes full | probes pruned | policies | work (pruned × policies) | compare | µs / unit |
/// |---|---|---|---|---|---|---|---|
/// | 5 | none | 4 515 | 525 | 10 | 5 250 | 0.01 s | — |
/// | 20 | none | 53 790 | 1 650 | 40 | 66 000 | 0.08 s | 1.2 |
/// | 40 | none | 203 490 | 3 150 | 80 | 252 000 | 0.27 s | 1.1 |
/// | 80 | none | 790 890 | 6 150 | 160 | 984 000 | 1.06 s | 1.1 |
/// | 160 | none | 3 117 690 | 12 150 | 320 | 3 888 000 | 4.16 s | 1.1 |
/// | 5 | unchanged | 10 530 | 1 080 | 12 | 12 960 | 0.02 s | — |
/// | 20 | unchanged | 109 080 | 2 880 | 42 | 120 960 | 0.15 s | 1.2 |
/// | 40 | unchanged | 397 980 | 5 280 | 82 | 432 960 | 0.50 s | 1.2 |
/// | 80 | unchanged | 1 515 780 | 10 080 | 162 | 1 632 960 | 1.86 s | 1.1 |
/// | 160 | unchanged | 5 911 380 | 19 680 | 322 | 6 336 960 | 7.18 s | 1.1 |
/// | 5 | changed | 9 360 | 4 200 | 12 | 50 400 | 0.12 s | — |
/// | 20 | changed | 104 535 | 48 300 | 42 | 2 028 600 | 3.05 s | 1.5 |
/// | 40 | changed | 388 935 | 180 600 | 82 | 14 809 200 | 19.9 s | 1.3 |
/// | 80 | changed | 1 497 735 | 697 200 | 162 | 112 946 400 | 141 s | 1.2 |
///
/// "tag mask: unchanged" is one tag mask on both sides; "changed" is a run
/// whose one changed policy IS the tag mask — pruning keeps every probe that
/// carries its tag (~45% of the full set), the worst case, and it grows like the
/// unpruned gate did. The SP4d-3 measurement of the UNPRUNED set (the same
/// profile shape, probes = the full column above), kept for comparison: N=5
/// 0.05 s, N=20 2.3 s, N=40 19.4 s, N=20 + a tag mask 5.9 s.
///
/// The cap is the largest measured work whose `compare` took at most 2.5 s —
/// 1 632 960 (N=80, unchanged tag mask, 1.86 s) — rounded down to 1 600 000, so
/// that even at the slowest measured rate (1.5 µs per unit) a refusal-free
/// migration holds the locks for at most about 2.4 s. Above it the migration is
/// REFUSED with the counted total before any probe is built (see the module
/// doc).
///
/// In practice, for one profile of single-user policies and one changed policy
/// (k = 1): about 100 policies without a tag mask (pruned work ≈ 154 N²; N=80 is
/// admitted, N=160 refused), about 79 with an unchanged tag mask (≈ 252 N²; the
/// measured N=80 row is 2% over the cap and IS refused). Migrating the tag mask
/// itself, or several policies at once, lowers it sharply: with the mask
/// changed, 20 policies are already 2.0 M and refused.
pub const MAX_PROBE_WORK: usize = 1_600_000;

/// The consequence the preview must state before the operator commits.
pub const GENERATED_NOTICE: &str = "After this migration every listed profile is compiled by \
     egret's permission model. Its remaining hand-written policies can still be edited and \
     deleted, but a NEW hand-written policy can no longer be created in it — add grants or rules \
     instead.";

// ── wire shapes ──────────────────────────────────────────────────────────

/// `POST …/migrate`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MigrateRequest {
    /// Names of policies of THIS profile, exactly as stored.
    pub policy_names: Vec<String>,
    /// Run everything, report it, and roll back.
    #[serde(default)]
    pub dry_run: bool,
}

/// `POST …/migrate/undo`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UndoRequest {
    pub policy_names: Vec<String>,
}

/// What a migration (or its dry run) did, and the proof.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MigrateResponse {
    pub dry_run: bool,
    /// Whether the transaction committed. `false` for every dry run and for
    /// every migration the gate did not prove.
    pub committed: bool,
    /// The gate ran over every affected profile and found no disagreement.
    pub equivalent: bool,
    /// Why the gate refused to answer at all (no disagreement list is a proof
    /// then). Absent when it answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// Each migrated policy and the model objects it became.
    pub policies: Vec<MigratedPolicy>,
    /// EVERY profile the gate evaluated — the source and every other profile
    /// the new objects compile into — with what changed in its download.
    pub profiles: Vec<GatedProfile>,
    /// Probe evaluations run (probes × plugin versions, over every profile).
    pub probe_evaluations: usize,
    /// Every probe on which a profile answers differently after the write.
    pub disagreements: Vec<WireDisagreement>,
    /// What the recompile did.
    pub compile: CompileResponse,
    /// Consequences to read before committing (see the module doc).
    pub notices: Vec<String>,
}

/// One migrated policy.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MigratedPolicy {
    pub policy_id: i64,
    pub policy_name: String,
    /// The grants it became. When `committed` is false (a dry run, or a
    /// migration the gate did not prove) these ids belong to a write that was
    /// rolled back and will never exist: a real migration assigns new ids when
    /// it writes. Use the count, not the ids, to describe a preview.
    pub grant_ids: Vec<Uuid>,
    /// The rules it became; the same caveat as `grant_ids`.
    pub rule_ids: Vec<Uuid>,
}

/// One profile the gate evaluated, and the artifact diff the engine will see.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct GatedProfile {
    pub service_id: i64,
    pub service_name: String,
    pub plugin_versions: Vec<WirePluginVersion>,
    /// Probes evaluated (after pruning).
    pub probes: usize,
    /// Probes the unpruned gate would have evaluated.
    pub probes_full: usize,
    /// Why pruning gave the full set, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pruning_fallback: Option<String>,
    pub disagreements: usize,
    /// Policy names (resource and tag) the download no longer serves.
    pub removed_policies: Vec<String>,
    /// Policy names the download now serves that it did not. A compiled name
    /// embeds its grant or rule id (`perm-grant-<id>`), so when `committed` is
    /// false that id is the rolled-back write's and is never stored; the real
    /// migration's compiled policies are named when written.
    pub added_policies: Vec<String>,
}

/// Wire mirror of [`Check`].
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WireCheck {
    Access,
    DataMask,
    RowFilter,
}

/// Wire mirror of [`Tagging`].
#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WireTagging {
    Actual,
    Hypothetical,
}

/// Wire mirror of [`Disagreement`], plus its one-line rendering.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WireDisagreement {
    pub service_id: i64,
    pub service_name: String,
    pub plugin_version: WirePluginVersion,
    pub check: WireCheck,
    pub user: String,
    pub groups: Vec<String>,
    pub roles: Vec<String>,
    pub tags: Vec<String>,
    pub tagging: WireTagging,
    pub access_type: String,
    pub resource: String,
    pub before: String,
    pub after: String,
    /// `Disagreement`'s `Display` — the sentence the page shows.
    pub sentence: String,
}

impl From<&Disagreement> for WireDisagreement {
    fn from(d: &Disagreement) -> Self {
        WireDisagreement {
            service_id: d.service_id,
            service_name: d.service_name.clone(),
            plugin_version: WirePluginVersion::from(d.plugin_version),
            check: match d.check {
                Check::Access => WireCheck::Access,
                Check::DataMask => WireCheck::DataMask,
                Check::RowFilter => WireCheck::RowFilter,
            },
            user: d.user.clone(),
            groups: d.groups.clone(),
            roles: d.roles.clone(),
            tags: d.tags.clone(),
            tagging: match d.tagging {
                Tagging::Actual => WireTagging::Actual,
                Tagging::Hypothetical => WireTagging::Hypothetical,
            },
            access_type: d.access_type.clone(),
            resource: d.resource.clone(),
            before: d.before.clone(),
            after: d.after.clone(),
            sentence: d.to_string(),
        }
    }
}

/// What `undo` did.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UndoResponse {
    pub policies: Vec<UndonePolicy>,
    pub compile: CompileResponse,
    /// Consequences that outlive the undo (review I-2).
    pub notices: Vec<String>,
}

/// One restored policy.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UndonePolicy {
    pub policy_id: i64,
    pub policy_name: String,
    /// Model objects the migration created, deleted by the undo.
    pub grants_deleted: usize,
    pub rules_deleted: usize,
}

// ── refusal helpers ──────────────────────────────────────────────────────

fn validate_names(names: &[String]) -> Result<(), PermHttpError> {
    if names.is_empty() {
        return Err(PermHttpError::BadRequest(
            "name at least one policy in `policy_names`".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for n in names {
        if n.is_empty() {
            return Err(PermHttpError::BadRequest(
                "`policy_names` contains an empty name".into(),
            ));
        }
        if !seen.insert(n.as_str()) {
            return Err(PermHttpError::BadRequest(format!(
                "`policy_names` names {n:?} twice"
            )));
        }
    }
    Ok(())
}

/// A guarded store flip, with the ownership refusals mapped to 409 (the
/// request names a real policy; what is wrong is who owns it).
fn store_refusal(e: PolicyStoreError) -> PermHttpError {
    match e {
        PolicyStoreError::CompilerOwned(s) | PolicyStoreError::InfrastructureOwned(s) => {
            PermHttpError::Conflict(s)
        }
        other => PermHttpError::from(other),
    }
}

fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.is_unique_violation())
}

// ── the profile set ──────────────────────────────────────────────────────

/// The engine-bound profiles whose download carries a policy stored in
/// `svc`: the profile itself for a resource service; for a tag service,
/// every engine-bound profile naming it as its `tagService`.
async fn profiles_serving_source(
    conn: &mut sqlx::PgConnection,
    svc: &ServiceRow,
    kind: ServiceKind,
) -> Result<Vec<i64>, PermHttpError> {
    match kind {
        ServiceKind::Resource => Ok(vec![svc.id]),
        ServiceKind::Tag => Ok(sqlx::query_scalar(
            "SELECT DISTINCT s.id FROM policy.authz_services s \
               JOIN engine_mgmt.engines e ON e.authz_service_id = s.id \
              WHERE s.config_jsonb ->> 'tagService' = $1 ORDER BY 1",
        )
        .bind(&svc.name)
        .fetch_all(&mut *conn)
        .await
        .map_err(PermHttpError::db)?),
    }
}

/// The plugin versions of the engines bound to `service_id`, ascending and
/// distinct. `Err(sentence)` when none is bound, or one is of a type this
/// crate has no measured plugin version for.
async fn bound_plugin_versions(
    conn: &mut sqlx::PgConnection,
    svc: &ServiceRow,
) -> Result<Result<Vec<PluginVersion>, String>, PermHttpError> {
    let engines = crate::routes::bound_engines(conn, svc.id).await?;
    if engines.is_empty() {
        return Ok(Err(format!(
            "profile \"{}\" (#{}) is affected by this migration but no engine is bound to it, so \
             there is no plugin version to prove the migration on. Bind its engine, or remove \
             the catalog attachment that makes it affected, and try again.",
            svc.name, svc.id
        )));
    }
    let mut versions: Vec<PluginVersion> = Vec::new();
    for e in engines {
        // An external engine's version is the one its operator declared; one the
        // evaluator was not read against refuses the migration on this profile.
        let v = match e.plugin_version() {
            Ok(v) => v,
            Err(sentence) => {
                return Ok(Err(format!(
                    "profile \"{}\" (#{}): {sentence}, so the migration cannot be proved on it.",
                    svc.name, svc.id
                )));
            }
        };
        if !versions.contains(&v) {
            versions.push(v);
        }
    }
    Ok(Ok(versions))
}

/// Every resource value of every policy (resource and tag, every resource
/// map) in `set` that is fold-divergent — the first one, named.
fn fold_divergent_policy_value(set: &ServicePolicySet) -> Option<(String, String)> {
    for p in set.policies.iter().chain(set.tag_policies.iter()) {
        for map in std::iter::once(&p.resources).chain(p.additional_resources.iter()) {
            for level in map.values() {
                if let Some(v) = level.values.iter().find(|v| fold_divergent(v)) {
                    return Some((p.name.clone(), v.clone()));
                }
            }
        }
    }
    None
}

fn policy_names(set: &ServicePolicySet) -> BTreeSet<String> {
    set.policies
        .iter()
        .chain(set.tag_policies.iter())
        .map(|p| p.name.clone())
        .collect()
}

// ── the case builder (rulings 1 and 4) ───────────────────────────────────

/// One [`ProfileCase`] per affected profile, built directly from the affected
/// set, with the count asserted and the vacuity guards applied.
///
/// `before` and `after` are two maps the caller filled at two different
/// moments (before step 1, after step 4); this function never reads either
/// download itself, so it cannot hand the same object to both sides.
/// `originals` are the migrated policies' names and `carriers` the profiles
/// whose download served them before the write.
#[allow(clippy::type_complexity)]
pub(crate) fn build_cases<'a>(
    affected: &[i64],
    before: &'a BTreeMap<i64, (ServiceDef, ProfileInputs)>,
    after: &'a BTreeMap<i64, ServicePolicySet>,
    originals: &[String],
    carriers: &[i64],
) -> Result<(Vec<ProfileCase<'a>>, Vec<PlanNote>), String> {
    let mut cases = Vec::with_capacity(affected.len());
    let mut notes = Vec::with_capacity(affected.len());
    for id in affected {
        let (Some((def, b)), Some(a)) = (before.get(id), after.get(id)) else {
            return Err(format!(
                "profile #{id} is affected but its download was not read on both sides of the \
                 write; the gate would compare nothing on it."
            ));
        };
        if std::ptr::eq(&b.download, a) {
            return Err(format!(
                "profile #{id}'s before and after downloads are the same object; the gate would \
                 compare a state with itself."
            ));
        }
        if carriers.contains(id) {
            let (bn, an) = (policy_names(&b.download), policy_names(a));
            for name in originals {
                if !bn.contains(name) {
                    return Err(format!(
                        "profile \"{}\" (#{id}) did not serve policy \"{name}\" before the \
                         write, so the gate's \"before\" is not the state being migrated from.",
                        b.name
                    ));
                }
                if an.contains(name) {
                    return Err(format!(
                        "profile \"{}\" (#{id}) still serves policy \"{name}\" after it was \
                         switched off, so the gate's \"after\" is not the written state.",
                        b.name
                    ));
                }
            }
        }
        let plan = plan_probes(&b.download, a, def, &b.assignments, PruneMode::Pruned);
        notes.push(PlanNote {
            probes_full: plan.full_count,
            pruning_fallback: plan.fallback.map(|f| format!("{f:?}")),
        });
        let probes = plan.probes;
        cases.push(ProfileCase {
            service_id: *id,
            service_name: b.name.clone(),
            before: &b.download,
            after: a,
            service_def: def,
            // Super-users as measured for egret's provisioned plugins (none);
            // `engine` stays `None` and `plugin_version` is set per entry.
            engine: EngineConfig::without_super_users(),
            versions: b.versions.clone(),
            probes,
        });
    }
    // Ruling 1: one case per affected profile — no more, no fewer.
    assert_eq!(
        cases.len(),
        affected.len(),
        "one ProfileCase per affected profile"
    );
    Ok((cases, notes))
}

/// What pruning did for one profile, kept beside its [`ProfileCase`].
#[derive(Debug)]
pub(crate) struct PlanNote {
    /// The probes the unpruned gate would have run.
    pub(crate) probes_full: usize,
    /// The [`crate::equivalence::FallbackReason`] when pruning gave the full set.
    pub(crate) pruning_fallback: Option<String>,
}

/// The per-profile inputs [`build_cases`] needs from before the write.
pub(crate) struct ProfileInputs {
    pub(crate) name: String,
    pub(crate) versions: Vec<PluginVersion>,
    pub(crate) assignments: Vec<TagAssignment>,
    pub(crate) download: ServicePolicySet,
}

/// The gate's WORK for these profiles, checked against `cap`
/// ([`MAX_PROBE_WORK`]): `Σ probes × versions × (policies before + policies
/// after)` over each profile's `(probes, versions, policies before, policies
/// after)`. Returns the probe EVALUATIONS (`Σ probes × versions`, what the
/// response reports and the provenance records), or the refusal sentence when
/// the work is over `cap` (ruling 3: refuse, never truncate).
///
/// Every probe visits every policy of both downloads, so the time is the
/// product, not the probe count; the count itself is driven by the SIZE of
/// each affected profile's download rather than by how many policies the
/// request migrates — so the sentence names the largest download, which is
/// what an operator can act on (review M-5).
pub(crate) fn probe_budget(
    counts: impl IntoIterator<Item = (usize, usize, usize, usize)>,
    cap: usize,
) -> Result<usize, String> {
    let mut evaluations = 0usize;
    let mut work = 0usize;
    let mut profiles = 0usize;
    let mut largest = 0usize;
    for (probes, versions, before, after) in counts {
        let n = probes.saturating_mul(versions);
        evaluations = evaluations.saturating_add(n);
        work = work.saturating_add(n.saturating_mul(before.saturating_add(after)));
        profiles += 1;
        largest = largest.max(before);
    }
    if work > cap {
        return Err(format!(
            "proving this migration needs {evaluations} probe evaluations across {profiles} \
             profile(s), {work} units of work (probe evaluations x the policies each visits), \
             over the limit of {cap}. The work grows with the size of each affected profile's \
             download (the largest serves {largest} policies), roughly with its square, not \
             with how many policies this request migrates. The probe set is never cut short \
             — a partial proof would be reported as a whole one — so this migration cannot be \
             proved until those profiles are smaller."
        ));
    }
    Ok(evaluations)
}

fn render_refusal(r: &GateRefusal) -> String {
    match r {
        GateRefusal::NoProfiles => {
            "the gate was handed no profile to evaluate, so it proves nothing.".into()
        }
        GateRefusal::NoPluginVersions { service_id } => format!(
            "profile #{service_id} has no plugin version to evaluate on, so the gate proves \
             nothing about it."
        ),
        GateRefusal::NoProbes { service_id } => format!(
            "no probe could be derived for profile #{service_id}, so the gate proves nothing \
             about it."
        ),
    }
}

// ── content hashes (review I-2) ──────────────────────────────────────────

/// JSON text with every object's keys sorted, recursively — independent of
/// jsonb's key order and of `preserve_order`. Array order is kept: it is
/// meaningful in a policy body (rule items are positional).
pub(crate) fn canonical_json(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        Value::String(k.clone()),
                        canonical_json(&m[k.as_str()])
                    )
                })
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical_json).collect::<Vec<_>>().join(",")
        ),
        other => other.to_string(),
    }
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// A principal set's two lists are SETS (stored as rows with no order), so
/// they are sorted before hashing; everything else is hashed as stored.
fn sorted_principals(p: &egret_perm::PrincipalSet) -> egret_perm::PrincipalSet {
    let key =
        |x: &egret_perm::Principal| canonical_json(&serde_json::to_value(x).unwrap_or_default());
    let mut out = p.clone();
    out.include.sort_by_key(key);
    out.exclude.sort_by_key(key);
    out
}

pub(crate) fn grant_hash(g: &egret_perm::Grant) -> String {
    let mut g = g.clone();
    g.principals = sorted_principals(&g.principals);
    g.actions.sort_by_key(|a| a.as_str().to_string());
    sha256_hex(&canonical_json(
        &serde_json::to_value(&g).unwrap_or_default(),
    ))
}

pub(crate) fn rule_hash(r: &egret_perm::Rule) -> String {
    let mut r = r.clone();
    r.exempt = sorted_principals(&r.exempt);
    sha256_hex(&canonical_json(
        &serde_json::to_value(&r).unwrap_or_default(),
    ))
}

/// The original policy's body WITHOUT its `name` (a rename stays undoable —
/// review I-3), plus its `enabled` COLUMN. `None` when the row is gone.
async fn original_hash_via(
    conn: &mut sqlx::PgConnection,
    policy_id: i64,
) -> Result<Option<String>, PermHttpError> {
    let row: Option<(serde_json::Value, bool)> =
        sqlx::query_as("SELECT policy_jsonb, enabled FROM policy.authz_policies WHERE id = $1")
            .bind(policy_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(PermHttpError::db)?;
    Ok(row.map(|(mut body, enabled)| {
        if let serde_json::Value::Object(m) = &mut body {
            m.remove("name");
        }
        sha256_hex(&canonical_json(&serde_json::json!({
            "body": body,
            "enabled": enabled,
        })))
    }))
}

// ── the affected set (review I-4) ────────────────────────────────────────

/// `ids` plus every engine-bound profile that downloads the tag service of
/// any member. A recompile of a member writes into its tag service, and every
/// profile naming that tag service serves the result, so each of those
/// profiles' downloads can change and must be gated (or, for undo,
/// recompiled) too. Ascending and deduplicated.
pub(crate) async fn with_tag_service_sharers(
    conn: &mut sqlx::PgConnection,
    ids: &[i64],
) -> Result<Vec<i64>, PermHttpError> {
    let sharers: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT y.id FROM policy.authz_services x \
           JOIN policy.authz_services y \
             ON y.config_jsonb ->> 'tagService' = x.config_jsonb ->> 'tagService' \
           JOIN engine_mgmt.engines e ON e.authz_service_id = y.id \
          WHERE x.id = ANY($1) AND x.config_jsonb ->> 'tagService' IS NOT NULL \
          ORDER BY 1",
    )
    .bind(ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(PermHttpError::db)?;
    Ok(merge_service_ids(ids, &sharers))
}

/// Ruling 7: the grammar names a policy's N rules "{name} #i", and
/// `perm.rules.name` is globally UNIQUE — a taken name is a 409 naming it.
async fn refuse_taken_rule_names(
    tx: &mut Transaction<'_, Postgres>,
    rule_names: &[String],
) -> Result<(), PermHttpError> {
    if rule_names.is_empty() {
        return Ok(());
    }
    let taken: Vec<String> =
        sqlx::query_scalar("SELECT name FROM perm.rules WHERE name = ANY($1) ORDER BY name")
            .bind(rule_names)
            .fetch_all(&mut **tx)
            .await
            .map_err(PermHttpError::db)?;
    if taken.is_empty() {
        return Ok(());
    }
    Err(PermHttpError::Conflict(format!(
        "the migration would create rule(s) named {}, and a rule with that name already \
         exists; rename the policy or the existing rule first",
        taken
            .iter()
            .map(|n| format!("{n:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

// ── migrate ──────────────────────────────────────────────────────────────

fn new_grant(g: &egret_perm::Grant) -> NewGrant {
    NewGrant {
        effect: g.effect,
        actions: g.actions.clone(),
        scope: g.scope.clone(),
        principals: g.principals.clone(),
    }
}

fn new_rule(r: &egret_perm::Rule) -> NewRule {
    NewRule {
        name: r.name.clone(),
        kind: r.kind.clone(),
        selector: r.selector.clone(),
        exempt: r.exempt.clone(),
    }
}

/// The whole migration, steps 0–6, on the caller's transaction. Neither
/// commits nor rolls back: the caller decides, in exactly one place, from
/// [`MigrateResponse::equivalent`] and `dry_run`.
///
/// `Err` is a refusal BEFORE the gate, with nothing worth reporting beyond
/// its sentence (404 / 409 / 400). Every gate outcome — proof, disagreement,
/// or a refusal to answer — is `Ok`.
pub(crate) async fn migrate_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
    req: &MigrateRequest,
    p: &egret_core::Principal,
) -> Result<MigrateResponse, PermHttpError> {
    validate_names(&req.policy_names)?;
    let mut response = MigrateResponse {
        dry_run: req.dry_run,
        committed: false,
        equivalent: false,
        refusal: None,
        policies: Vec::new(),
        profiles: Vec::new(),
        probe_evaluations: 0,
        disagreements: Vec::new(),
        compile: CompileResponse::default(),
        notices: vec![GENERATED_NOTICE.to_string()],
    };

    // ── step 0: re-classify, in the refusal order ─────────────────────────
    // 404 (profile, then each name) → 409 (already migrated, then not
    // operator-owned) → 400 (not migratable) → 409 (rule-name collision).
    // Pass by pass over ALL names, so one request answers the first refusal
    // class any of its names hits.
    let inputs = ServiceInputs::load(tx, service_id).await?;
    let mut recs = Vec::with_capacity(req.policy_names.len());
    for name in &req.policy_names {
        let Some(rec) = inputs.rows.iter().find(|r| &r.name == name) else {
            return Err(PermHttpError::NotFound);
        };
        recs.push(rec);
    }
    for rec in &recs {
        if let Some(m) = inputs.migrated.get(&rec.id) {
            return Err(PermHttpError::Conflict(format!(
                "policy {:?} was already migrated on {} by {}; undo that migration before \
                 migrating it again",
                rec.name, m.migrated_at, m.migrated_by
            )));
        }
    }
    let classified: Vec<_> = recs.iter().map(|r| (*r, inputs.classify(r))).collect();
    for (rec, (c, _)) in &classified {
        if rec.managed_by != "operator" {
            return Err(PermHttpError::Conflict(format!(
                "policy {:?} is not hand-written (managed_by = {}): {}",
                rec.name,
                rec.managed_by,
                c.blockers.join(" ")
            )));
        }
    }
    let mut plan: Vec<(i64, String, Candidates)> = Vec::new();
    for (rec, (c, cands)) in classified {
        match cands {
            Some(cands) if c.migratable => plan.push((rec.id, rec.name.clone(), cands)),
            _ => {
                return Err(PermHttpError::BadRequest(format!(
                    "policy {:?} cannot be migrated: {}",
                    rec.name,
                    c.blockers.join(" ")
                )))
            }
        }
    }
    let Some((kind, _)) = inputs.target else {
        // Unreachable: a profile of another type has no migratable policy.
        return Err(PermHttpError::BadRequest(
            "this profile's type is not one egret's permission model compiles to".into(),
        ));
    };
    // Ruling 7: the grammar names a policy's N rules "{name} #i", and
    // `perm.rules.name` is globally UNIQUE — a collision is a 409 naming it,
    // with nothing written, never the 500 the INSERT would raise.
    let mut rule_names: Vec<String> = Vec::new();
    for (_, _, c) in &plan {
        if let Candidates::Rules(rules) = c {
            rule_names.extend(rules.iter().map(|r| r.name.clone()));
        }
    }
    let mut distinct = BTreeSet::new();
    if let Some(dup) = rule_names.iter().find(|n| !distinct.insert(n.as_str())) {
        return Err(PermHttpError::Conflict(format!(
            "two of the named policies would both become a rule named {dup:?}; rule names are \
             unique, so migrate them one at a time after renaming one"
        )));
    }
    refuse_taken_rule_names(tx, &rule_names).await?;

    // ── the affected set, locked, and each member's state BEFORE ──────────
    let carriers = profiles_serving_source(tx, &inputs.svc, kind).await?;
    let mut affected: Vec<i64> = carriers.clone();
    for (_, _, c) in &plan {
        match c {
            Candidates::Grants(gs) => {
                for g in gs {
                    affected =
                        merge_service_ids(&affected, &affected_services_for_grant(tx, g).await?);
                }
            }
            Candidates::Rules(rs) => {
                for r in rs {
                    affected =
                        merge_service_ids(&affected, &affected_services_for_rule(tx, r).await?);
                }
            }
        }
    }
    // Review I-4: a first recompile writes into each member's TAG service, and
    // every engine-bound profile naming that tag service downloads the result.
    affected = with_tag_service_sharers(tx, &affected).await?;
    lock_services_in_tx(tx, &merge_service_ids(&affected, &[service_id])).await?;
    // Review M-1: the first check ran before the lock; a concurrent migration
    // that created one of these names has committed by now if it shares a
    // lock with this one. (One that does not share a lock is caught at the
    // INSERT, which `write_rule` answers 409.)
    refuse_taken_rule_names(tx, &rule_names).await?;

    let mut before: BTreeMap<i64, (ServiceDef, ProfileInputs)> = BTreeMap::new();
    for &id in &affected {
        let svc = get_service_by_id_via(tx, id).await?;
        let versions = match bound_plugin_versions(tx, &svc).await? {
            Ok(v) => v,
            Err(sentence) => {
                response.refusal = Some(sentence);
                return Ok(response);
            }
        };
        let catalog_ids: Vec<Uuid> = catalogs_for_service_via(tx, id)
            .await?
            .into_iter()
            .map(|c| c.id)
            .collect();
        let rows = tag_assignments_for_catalogs_via(tx, &catalog_ids).await?;
        // Ruling 2: the fold-divergence precondition, enforced.
        if let Some(r) = rows.iter().find(|r| {
            stored_coordinate_is_fold_divergent(&r.schema_name, &r.table_name, &r.column_name)
        }) {
            response.refusal = Some(format!(
                "a tag assignment on {}.{}.{}{} (profile \"{}\") contains a character on which \
                 egret's case folding and the engine's can disagree, so the gate cannot say \
                 which columns carry which tag and cannot prove this migration.",
                r.catalog,
                r.schema_name,
                r.table_name,
                if r.column_name.is_empty() {
                    String::new()
                } else {
                    format!(".{}", r.column_name)
                },
                svc.name
            ));
            return Ok(response);
        }
        let assignments = rows
            .into_iter()
            .map(|r| TagAssignment {
                catalog: r.catalog,
                schema: r.schema_name,
                table: r.table_name,
                column: r.column_name,
                tag: r.tag_name,
            })
            .collect();
        let download = artifact_via(tx, &svc).await?;
        let def = ServiceDef::parse(&download.service_def);
        before.insert(
            id,
            (
                def,
                ProfileInputs {
                    name: svc.name.clone(),
                    versions,
                    assignments,
                    download,
                },
            ),
        );
    }

    // ── step 1: switch every original off (the column) ────────────────────
    // BEFORE the model writes (SP4d-4 Task 3 fix round 1, review C1): each
    // `write_grant`/`write_rule` recompiles, and the recompile's overlap check
    // reads every ENABLED hand-written mask/row filter of the profiles it
    // sweeps. Left on, an original would pair with its own compiled twin and
    // refuse the migration on itself; switched off, it is out of that read
    // while every OTHER hand-written policy still on is in it. The gate's two
    // readings are untouched by the move: BEFORE was read above (step 0) and
    // AFTER is read in step 5, both outside this window, and everything here
    // is one transaction.
    for (policy_id, _, _) in &plan {
        set_policy_enabled_via(tx, *policy_id, false)
            .await
            .map_err(store_refusal)?;
    }

    // ── step 2 + 3: insert through the one write implementation ───────────
    let mut summaries: Vec<CompileSummary> = Vec::new();
    let mut recompiled: Vec<i64> = Vec::new();
    for (policy_id, policy_name, cands) in &plan {
        let mut migrated = MigratedPolicy {
            policy_id: *policy_id,
            policy_name: policy_name.clone(),
            grant_ids: Vec::new(),
            rule_ids: Vec::new(),
        };
        match cands {
            Candidates::Grants(gs) => {
                for g in gs {
                    let out = write_grant(tx, None, new_grant(g), p).await?;
                    migrated.grant_ids.push(out.id);
                    recompiled = merge_service_ids(&recompiled, &out.services);
                    summaries.extend(out.summaries);
                }
            }
            Candidates::Rules(rs) => {
                for r in rs {
                    let out = write_rule(tx, None, new_rule(r), p).await?;
                    migrated.rule_ids.push(out.id);
                    recompiled = merge_service_ids(&recompiled, &out.services);
                    summaries.extend(out.summaries);
                }
            }
        }
        response.policies.push(migrated);
    }
    // Final review I-2: every profile a recompile wrote must be one the gate
    // evaluates. `write_grant`/`write_rule` recompute their own service set,
    // and a catalog attached concurrently since `affected` was computed adds a
    // profile the gate never reads. A `debug_assert!` here was compiled out of
    // release builds, which would commit that profile's new policy ungated.
    if let Some(sentence) = ungated_recompile(&recompiled, &affected) {
        response.refusal = Some(sentence);
        return Ok(response);
    }

    // ── step 4: provenance (the count lands in step 6) ────────────────────
    let actor = p.email.clone();
    for m in &response.policies {
        // Review I-2: the content undo will compare against, as STORED.
        let mut grant_hashes = Vec::with_capacity(m.grant_ids.len());
        for id in &m.grant_ids {
            let g = egret_perm::get_grant_via(tx, *id)
                .await?
                .ok_or_else(|| PermHttpError::db(format!("grant {id} vanished in its own tx")))?;
            grant_hashes.push(grant_hash(&g));
        }
        let mut rule_hashes = Vec::with_capacity(m.rule_ids.len());
        for id in &m.rule_ids {
            let r = egret_perm::get_rule_via(tx, *id)
                .await?
                .ok_or_else(|| PermHttpError::db(format!("rule {id} vanished in its own tx")))?;
            rule_hashes.push(rule_hash(&r));
        }
        let original = original_hash_via(tx, m.policy_id)
            .await?
            .ok_or_else(|| PermHttpError::db("the original vanished in its own tx"))?;
        let inserted = sqlx::query(
            "INSERT INTO perm.migrated_from \
               (service_id, policy_id, policy_name, grant_ids, rule_ids, grant_hashes, \
                rule_hashes, original_hash, probes_proved, migrated_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 0, $9)",
        )
        .bind(service_id)
        .bind(m.policy_id)
        .bind(&m.policy_name)
        .bind(&m.grant_ids)
        .bind(&m.rule_ids)
        .bind(&grant_hashes)
        .bind(&rule_hashes)
        .bind(&original)
        .bind(&actor)
        .execute(&mut **tx)
        .await;
        match inserted {
            Ok(_) => {}
            Err(e) if is_unique_violation(&e) => {
                return Err(PermHttpError::Conflict(format!(
                    "policy {:?} was migrated by a concurrent request; nothing was written here",
                    m.policy_name
                )))
            }
            Err(e) => return Err(PermHttpError::db(e)),
        }
    }
    response.compile = CompileResponse::from_summaries(recompiled, &summaries);

    // ── step 5: re-read every affected download and gate them all ─────────
    let mut after: BTreeMap<i64, ServicePolicySet> = BTreeMap::new();
    for &id in &affected {
        let svc = get_service_by_id_via(tx, id).await?;
        after.insert(id, artifact_via(tx, &svc).await?);
    }
    let originals: Vec<String> = response
        .policies
        .iter()
        .map(|m| m.policy_name.clone())
        .collect();
    for (id, set) in before
        .iter()
        .map(|(id, (_, b))| (id, &b.download))
        .chain(after.iter())
    {
        if let Some((policy, value)) = fold_divergent_policy_value(set) {
            response.refusal = Some(format!(
                "policy \"{policy}\" in profile #{id}'s download names `{value}`, which contains \
                 a character on which egret's case folding and the engine's can disagree, so \
                 the gate cannot prove this migration."
            ));
            return Ok(response);
        }
    }
    // Review I-1: COUNT the probes from their factors and refuse over the cap
    // BEFORE building any — the build is the cost the cap exists to bound.
    let mut counts = Vec::with_capacity(affected.len());
    for id in &affected {
        if let (Some((def, b)), Some(a)) = (before.get(id), after.get(id)) {
            counts.push((
                planned_probe_count(&b.download, a, def, &b.assignments, PruneMode::Pruned),
                b.versions.len(),
                b.download.policies.len() + b.download.tag_policies.len(),
                a.policies.len() + a.tag_policies.len(),
            ));
        }
    }
    let evaluations = match probe_budget(counts, MAX_PROBE_WORK) {
        Ok(n) => n,
        Err(sentence) => {
            response.refusal = Some(sentence);
            return Ok(response);
        }
    };
    let (cases, notes) = match build_cases(&affected, &before, &after, &originals, &carriers) {
        Ok(c) => c,
        Err(sentence) => {
            response.refusal = Some(sentence);
            return Ok(response);
        }
    };
    response.profiles = cases
        .iter()
        .zip(&notes)
        .map(|(c, note)| {
            let (bn, an) = (policy_names(c.before), policy_names(c.after));
            GatedProfile {
                service_id: c.service_id,
                service_name: c.service_name.clone(),
                plugin_versions: c.versions.iter().copied().map(Into::into).collect(),
                probes: c.probes.len(),
                probes_full: note.probes_full,
                pruning_fallback: note.pruning_fallback.clone(),
                disagreements: 0,
                removed_policies: bn.difference(&an).cloned().collect(),
                added_policies: an.difference(&bn).cloned().collect(),
            }
        })
        .collect();
    debug_assert_eq!(
        evaluations,
        cases
            .iter()
            .map(|c| c.probes.len() * c.versions.len())
            .sum::<usize>(),
        "the counted probes are the built probes"
    );
    response.probe_evaluations = evaluations;
    let verdict = compare_profiles(&cases);
    match verdict {
        Err(r) => {
            response.refusal = Some(render_refusal(&r));
            return Ok(response);
        }
        Ok(ds) if !ds.is_empty() => {
            for gp in &mut response.profiles {
                gp.disagreements = ds.iter().filter(|d| d.service_id == gp.service_id).count();
            }
            response.disagreements = ds.iter().map(WireDisagreement::from).collect();
            return Ok(response);
        }
        Ok(_) => {}
    }

    // ── step 6: record what proved it ─────────────────────────────────────
    let proved = i32::try_from(evaluations).unwrap_or(i32::MAX);
    let updated = sqlx::query(
        "UPDATE perm.migrated_from SET probes_proved = $3 \
          WHERE service_id = $1 AND policy_id = ANY($2)",
    )
    .bind(service_id)
    .bind(
        response
            .policies
            .iter()
            .map(|m| m.policy_id)
            .collect::<Vec<i64>>(),
    )
    .bind(proved)
    .execute(&mut **tx)
    .await
    .map_err(PermHttpError::db)?;
    if updated.rows_affected() != originals.len() as u64 {
        return Err(PermHttpError::db(format!(
            "provenance count mismatch: updated {} of {}",
            updated.rows_affected(),
            originals.len()
        )));
    }
    response.equivalent = true;
    Ok(response)
}

// ── undo ─────────────────────────────────────────────────────────────────

/// The consequence that outlives an undo.
pub const UNDO_GENERATED_NOTICE: &str =
    "Every profile this migration recompiled stays compiled by \
     egret's permission model after the undo: its hand-written policies can be edited and \
     deleted, but a NEW hand-written policy still cannot be created in it.";

/// What undo found for its named policies at one point in time.
struct UndoRecord {
    policy_id: i64,
    policy_name: String,
    grants: Vec<egret_perm::Grant>,
    rules: Vec<egret_perm::Rule>,
}

/// One reading of every named policy's provenance, created objects and
/// original: the records of what is still intact, the changes found (a missing
/// or differently-hashed object), and the profiles those objects reach NOW.
struct UndoReading {
    records: Vec<UndoRecord>,
    changed: Vec<String>,
    /// Profiles the intact objects reach, plus their tag-service sharers, plus
    /// the requested profile — ascending, deduplicated.
    affected: Vec<i64>,
}

/// Read and hash-check the named policies' migrations. `lock_provenance`
/// takes `FOR UPDATE` on the provenance rows; it is set only on the reading
/// made under the profile locks (migrate takes profile locks first, then writes
/// provenance, so undo must not hold a provenance row while it waits for a
/// profile).
async fn read_undo_state(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
    names: &[String],
    lock_provenance: bool,
) -> Result<UndoReading, PermHttpError> {
    let mut records = Vec::new();
    let mut changed: Vec<String> = Vec::new();
    let mut affected: Vec<i64> = Vec::new();
    for name in names {
        let policy_id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
        )
        .bind(service_id)
        .bind(name)
        .fetch_optional(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
        let Some(policy_id) = policy_id else {
            return Err(PermHttpError::NotFound);
        };
        type Prov = (Vec<Uuid>, Vec<Uuid>, Vec<String>, Vec<String>, String);
        let sql = if lock_provenance {
            "SELECT grant_ids, rule_ids, grant_hashes, rule_hashes, original_hash \
               FROM perm.migrated_from WHERE policy_id = $1 FOR UPDATE"
        } else {
            "SELECT grant_ids, rule_ids, grant_hashes, rule_hashes, original_hash \
               FROM perm.migrated_from WHERE policy_id = $1"
        };
        let row: Option<Prov> = sqlx::query_as(sql)
            .bind(policy_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(PermHttpError::db)?;
        let Some((grant_ids, rule_ids, grant_hashes, rule_hashes, original)) = row else {
            return Err(PermHttpError::NotFound);
        };

        let mut grants = Vec::new();
        for (id, recorded) in grant_ids.iter().zip(&grant_hashes) {
            match egret_perm::get_grant_via(tx, *id).await? {
                None => changed.push(format!(
                    "grant {id}, created by the migration of {name:?}, has been deleted"
                )),
                Some(g) if &grant_hash(&g) != recorded => changed.push(format!(
                    "grant {id}, created by the migration of {name:?}, has been edited"
                )),
                Some(g) => {
                    affected =
                        merge_service_ids(&affected, &affected_services_for_grant(tx, &g).await?);
                    grants.push(g);
                }
            }
        }
        let mut rules = Vec::new();
        for (id, recorded) in rule_ids.iter().zip(&rule_hashes) {
            match egret_perm::get_rule_via(tx, *id).await? {
                None => changed.push(format!(
                    "rule {id}, created by the migration of {name:?}, has been deleted"
                )),
                Some(r) if &rule_hash(&r) != recorded => changed.push(format!(
                    "rule {id} ({:?}), created by the migration of {name:?}, has been edited",
                    r.name
                )),
                Some(r) => {
                    affected =
                        merge_service_ids(&affected, &affected_services_for_rule(tx, &r).await?);
                    rules.push(r);
                }
            }
        }
        if original_hash_via(tx, policy_id).await?.as_deref() != Some(original.as_str()) {
            changed.push(format!(
                "the original policy {name:?} (id {policy_id}) has been edited or switched on \
                 since it was migrated"
            ));
        }
        records.push(UndoRecord {
            policy_id,
            policy_name: name.clone(),
            grants,
            rules,
        });
    }
    // Review I-4: the profiles sharing a recompiled profile's tag service.
    let affected = with_tag_service_sharers(tx, &affected).await?;
    let affected = merge_service_ids(&affected, &[service_id]);
    Ok(UndoReading {
        records,
        changed,
        affected,
    })
}

/// The refusal when a recompile wrote a profile outside the gated set (final
/// review I-2), or `None` when `recompiled ⊆ affected`. Pure, so the refusal
/// can be driven directly; its call site in [`migrate_in_tx`] is pinned by a
/// route test that attaches a catalog mid-transaction.
fn ungated_recompile(recompiled: &[i64], affected: &[i64]) -> Option<String> {
    let missed: Vec<String> = recompiled
        .iter()
        .filter(|id| !affected.contains(id))
        .map(|id| format!("#{id}"))
        .collect();
    if missed.is_empty() {
        return None;
    }
    Some(format!(
        "this migration recompiled profile(s) {}, which the equivalence gate does not \
         evaluate: a catalog was attached to an engine while the migration ran. Nothing was \
         written; retry the migration.",
        missed.join(", ")
    ))
}

fn undo_conflict(changed: &[String]) -> PermHttpError {
    PermHttpError::Conflict(format!(
        "undo refused, nothing was changed: {}. Undo deletes exactly what the migration \
         created and restores exactly what it switched off; reconcile these by hand (or \
         revert the edits) first.",
        changed.join("; ")
    ))
}

/// `migrate/undo`, on the caller's transaction (the caller commits): delete
/// the objects provenance names, recompile, switch the original back on,
/// delete the provenance row. A MECHANICAL rollback — possible only because
/// the original was switched off, not deleted.
///
/// **Guarded by content hashes** (review I-2, the user's ruling of
/// 2026-10-11): every created grant/rule and the original are re-hashed in
/// this transaction, and if ANY differs from what provenance recorded — an
/// object edited or deleted since, or the original edited or switched back on
/// — the answer is 409 naming each change, and nothing is written. An object
/// edited since the migration is no longer the migration's to delete.
///
/// **The check that counts is made UNDER the profile locks** (re-review N-1).
/// Every hand-written write to a profile — a grant or rule PATCH, the policy
/// editor — takes that profile's `authz_services` row lock before it writes,
/// and commits holding it. So: a first reading, taken before any lock, only
/// decides WHICH profiles to lock (the ones the intact objects reach); then
/// those locks are taken (ascending, as `lock_services_in_tx` requires); then
/// everything is read and hashed AGAIN, and that second reading alone decides
/// 409 or proceed. A writer that commits between the readings is seen by the
/// second; a writer that is in flight holds a lock this transaction waits for,
/// and its commit is visible to the statements after the wait (READ COMMITTED);
/// no writer can commit between the second reading and the DELETEs, because
/// the locks are held to commit. A grant moved to another catalog in the gap
/// changes its hash, so it is a 409, not a silent miss of the new profile; and
/// if the second reading reaches a profile the first did not (a catalog
/// re-bound to another engine), that is a 409 too, because deleting would
/// leave that profile's compiled policy behind.
///
/// Names resolve to the policy as it is NOW (a renamed original is undone by
/// its new name) and provenance is read by policy id. 404 for a profile, a
/// name, or a policy with no migration on record.
pub(crate) async fn undo_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
    req: &UndoRequest,
) -> Result<UndoResponse, PermHttpError> {
    validate_names(&req.policy_names)?;
    get_service_by_id_via(tx, service_id).await?;

    // Reading 1, unlocked: which profiles to lock. Its verdict is discarded.
    let first = read_undo_state(tx, service_id, &req.policy_names, false).await?;
    // Before any DELETE — see `lock_services_in_tx`.
    lock_services_in_tx(tx, &first.affected).await?;

    // Reading 2, under the locks: the verdict.
    let UndoReading {
        records,
        changed,
        affected,
    } = read_undo_state(tx, service_id, &req.policy_names, true).await?;
    if !changed.is_empty() {
        return Err(undo_conflict(&changed));
    }
    if let Some(missed) = affected.iter().find(|id| !first.affected.contains(id)) {
        return Err(PermHttpError::Conflict(format!(
            "undo refused, nothing was changed: profile {missed} became reachable from the \
             migrated objects while undo waited for its locks. Retry the undo."
        )));
    }
    let mut out = Vec::new();
    for r in &records {
        let mut grants_deleted = 0;
        for g in &r.grants {
            if egret_perm::delete_grant_in_tx(tx, g.id).await? {
                grants_deleted += 1;
            }
        }
        let mut rules_deleted = 0;
        for rule in &r.rules {
            if egret_perm::delete_rule_in_tx(tx, rule.id).await? {
                rules_deleted += 1;
            }
        }
        out.push(UndonePolicy {
            policy_id: r.policy_id,
            policy_name: r.policy_name.clone(),
            grants_deleted,
            rules_deleted,
        });
    }
    let summaries = recompile_services_in_tx(tx, &affected).await?;
    for r in &records {
        set_policy_enabled_via(tx, r.policy_id, true)
            .await
            .map_err(store_refusal)?;
        sqlx::query("DELETE FROM perm.migrated_from WHERE policy_id = $1")
            .bind(r.policy_id)
            .execute(&mut **tx)
            .await
            .map_err(PermHttpError::db)?;
    }
    Ok(UndoResponse {
        policies: out,
        compile: CompileResponse::from_summaries(affected, &summaries),
        notices: vec![UNDO_GENERATED_NOTICE.to_string()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_budget_counts_every_version_of_every_profile_and_refuses_over_the_cap() {
        // Evaluations: 100 + 100 = 200. Work: 100 x 4 + 100 x 20 = 2 400.
        assert_eq!(
            probe_budget([(100, 1, 2, 2), (50, 2, 9, 11)], 2_400),
            Ok(200)
        );
        let refused = probe_budget([(100, 1, 2, 2), (50, 2, 9, 11)], 2_399).unwrap_err();
        assert!(refused.contains("200 probe evaluations"), "{refused}");
        assert!(refused.contains("2400 units of work"), "{refused}");
        assert!(refused.contains("2 profile(s)"), "{refused}");
        assert!(refused.contains("never cut short"), "{refused}");
        assert!(refused.contains("limit of 2399"), "{refused}");
        assert!(
            refused.contains("the largest serves 9 policies"),
            "{refused}"
        );
        assert!(!refused.contains("fewer policies"), "{refused}");
    }

    fn envelope(names: &[&str]) -> ServicePolicySet {
        let policies: Vec<serde_json::Value> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                serde_json::json!({
                    "id": i + 1, "name": n, "service": "p", "policyType": 0, "isEnabled": true,
                    "resources": {
                        "catalog": {"values": ["wh"]}, "schema": {"values": ["s"]},
                        "table": {"values": ["t"]}, "column": {"values": ["*"]}
                    },
                    "policyItems": [{"users": ["alice"], "accesses": [{"type": "select", "isAllowed": true}]}]
                })
            })
            .collect();
        serde_json::from_value(serde_json::json!({
            "serviceName": "p", "serviceId": 1, "policyVersion": 1,
            "policies": policies,
            "serviceDef": egret_policy::trino_service_def().expect("trino def"),
        }))
        .expect("envelope")
    }

    fn inputs(download: ServicePolicySet) -> BTreeMap<i64, (ServiceDef, ProfileInputs)> {
        let def = ServiceDef::parse(&download.service_def);
        BTreeMap::from([(
            1,
            (
                def,
                ProfileInputs {
                    name: "p".into(),
                    versions: vec![PluginVersion::Ranger2_5_0],
                    assignments: Vec::new(),
                    download,
                },
            ),
        )])
    }

    #[test]
    fn an_after_that_still_serves_the_original_is_refused_as_not_the_written_state() {
        let before = inputs(envelope(&["orig"]));
        let after = BTreeMap::from([(1, envelope(&["orig", "perm-grant-x"]))]);
        let err = build_cases(&[1], &before, &after, &["orig".into()], &[1]).unwrap_err();
        assert!(err.contains("still serves policy \"orig\""), "{err}");
    }

    #[test]
    fn a_before_that_never_served_the_original_is_refused() {
        let before = inputs(envelope(&["other"]));
        let after = BTreeMap::from([(1, envelope(&["perm-grant-x"]))]);
        let err = build_cases(&[1], &before, &after, &["orig".into()], &[1]).unwrap_err();
        assert!(err.contains("did not serve policy \"orig\""), "{err}");
    }

    #[test]
    fn a_profile_read_on_one_side_only_is_refused_not_skipped() {
        let before = inputs(envelope(&["orig"]));
        let after: BTreeMap<i64, ServicePolicySet> = BTreeMap::new();
        let err = build_cases(&[1], &before, &after, &["orig".into()], &[1]).unwrap_err();
        assert!(err.contains("not read on both sides"), "{err}");
    }

    #[test]
    fn a_recompiled_profile_outside_the_gated_set_is_refused() {
        assert_eq!(ungated_recompile(&[1, 3], &[1, 2, 3]), None);
        assert_eq!(ungated_recompile(&[], &[1]), None);
        let refused = ungated_recompile(&[1, 4, 7], &[1, 2, 3]).expect("refused");
        assert!(
            refused.contains("recompiled profile(s) #4, #7,"),
            "{refused}"
        );
        assert!(refused.contains("Nothing was written; retry"), "{refused}");
    }

    #[test]
    fn one_case_per_affected_profile() {
        let before = inputs(envelope(&["orig"]));
        let after = BTreeMap::from([(1, envelope(&["perm-grant-x"]))]);
        let (cases, _) = build_cases(&[1], &before, &after, &["orig".into()], &[1]).expect("cases");
        assert_eq!(cases.len(), 1);
        assert!(!cases[0].probes.is_empty());
        assert!(!std::ptr::eq(cases[0].before, cases[0].after));
    }
}
