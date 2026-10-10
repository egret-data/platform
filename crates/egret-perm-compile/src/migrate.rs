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
//! 1. Insert the grants / rules the grammar produced — through
//!    [`crate::routes::write_grant`] / [`crate::routes::write_rule`], the one
//!    implementation of a model write, so every authoring guard those carry
//!    (`validate_grant_shape`, the unreadable-table-mask refusal, …) applies to
//!    a migration exactly as it does to a hand-authored grant.
//! 2. Recompile — the same calls recompile every profile each object affects
//!    (`recompile_services_in_tx`), stamping them `generated`.
//! 3. Switch the original off — the `enabled` COLUMN, through
//!    `egret_policy::set_policy_enabled_via`, the store's guarded flip.
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
//! - **The probe budget** ([`MAX_PROBE_EVALUATIONS`]): hypothetical tagging
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
    compare_profiles, derive_probes, Check, Disagreement, GateRefusal, ProfileCase, TagAssignment,
    Tagging,
};
use crate::explain_wire::WirePluginVersion;
use crate::grammar::ServiceKind;
use crate::routes::{
    artifact_via, fold_divergent, ranger_engine, stored_coordinate_is_fold_divergent, write_grant,
    write_rule, CompileResponse, NewGrant, NewRule, PermHttpError,
};
use crate::trigger::{
    affected_services_for_grant, affected_services_for_rule, engine_type_for, lock_services_in_tx,
    merge_service_ids, recompile_services_in_tx, CompileSummary,
};

/// The most probe EVALUATIONS (probes × plugin versions, summed over every
/// affected profile) one migration may run. Each evaluation asks the access,
/// masking and row-filter questions of both downloads — six evaluator calls —
/// inside a transaction holding every affected profile's lock.
///
/// Measured 2026-10-11 (`compare` on one Trino profile of N single-user
/// policies on distinct tables, `--release`, laptop): N=5 → 3 870 evaluations
/// in 0.20 s; N=20 → 51 345 in 9.4 s; N=40 → 198 645 in 70 s. The cost per
/// evaluation grows with the download's size (53 → 183 → 352 µs), and the
/// probe count grows roughly with the square of it. The route fixtures in
/// `tests/migrate_route.rs` run 330. 20 000 keeps the worst measured density
/// near 4 s of locked transaction; above it the migration is REFUSED with the
/// measured count (see the module doc) — on a profile of more than ~15
/// policies that refusal is expected today, and it is the honest answer until
/// the gate gets faster, not a reason to truncate.
pub const MAX_PROBE_EVALUATIONS: usize = 20_000;

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
    pub grant_ids: Vec<Uuid>,
    pub rule_ids: Vec<Uuid>,
}

/// One profile the gate evaluated, and the artifact diff the engine will see.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct GatedProfile {
    pub service_id: i64,
    pub service_name: String,
    pub plugin_versions: Vec<WirePluginVersion>,
    pub probes: usize,
    pub disagreements: usize,
    /// Policy names (resource and tag) the download no longer serves.
    pub removed_policies: Vec<String>,
    /// Policy names the download now serves that it did not.
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
}

/// One restored policy.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UndonePolicy {
    pub policy_id: i64,
    pub policy_name: String,
    /// Model objects provenance named that still existed and were deleted.
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
    let types: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT type FROM engine_mgmt.engines WHERE authz_service_id = $1 ORDER BY 1",
    )
    .bind(svc.id)
    .fetch_all(&mut *conn)
    .await
    .map_err(PermHttpError::db)?;
    if types.is_empty() {
        return Ok(Err(format!(
            "profile \"{}\" (#{}) is affected by this migration but no engine is bound to it, so \
             there is no plugin version to prove the migration on. Bind its engine, or remove \
             the catalog attachment that makes it affected, and try again.",
            svc.name, svc.id
        )));
    }
    let mut versions: Vec<PluginVersion> = Vec::new();
    for t in types {
        let Some(engine) = engine_type_for(&t) else {
            return Ok(Err(format!(
                "profile \"{}\" (#{}) is bound to an engine of type `{t}`, for which egret has \
                 no measured Ranger plugin version, so the migration cannot be proved on it.",
                svc.name, svc.id
            )));
        };
        let v = ranger_engine(engine).plugin_version();
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
) -> Result<Vec<ProfileCase<'a>>, String> {
    let mut cases = Vec::with_capacity(affected.len());
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
        let probes = derive_probes(&b.download, a, def, &b.assignments);
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
    Ok(cases)
}

/// The per-profile inputs [`build_cases`] needs from before the write.
pub(crate) struct ProfileInputs {
    pub(crate) name: String,
    pub(crate) versions: Vec<PluginVersion>,
    pub(crate) assignments: Vec<TagAssignment>,
    pub(crate) download: ServicePolicySet,
}

/// Probe evaluations the cases will run — `Σ probes × versions` over the
/// `(probes, versions)` count of each case — or the refusal sentence when
/// over `cap` (ruling 3: refuse, never truncate).
pub(crate) fn probe_budget(
    counts: impl IntoIterator<Item = (usize, usize)>,
    cap: usize,
) -> Result<usize, String> {
    let mut total = 0usize;
    let mut profiles = 0usize;
    for (probes, versions) in counts {
        total = total.saturating_add(probes.saturating_mul(versions));
        profiles += 1;
    }
    if total > cap {
        return Err(format!(
            "proving this migration needs {total} probe evaluations across {profiles} \
             profile(s), over the limit of {cap}. The probe set is never cut short — a partial \
             proof would be reported as a whole one — so migrate fewer policies at once, or \
             reduce the tag policies and tagged columns these profiles carry."
        ));
    }
    Ok(total)
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
        if let Some(m) = inputs.migrated.get(&rec.name) {
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
    let taken: Vec<String> =
        sqlx::query_scalar("SELECT name FROM perm.rules WHERE name = ANY($1) ORDER BY name")
            .bind(&rule_names)
            .fetch_all(&mut **tx)
            .await
            .map_err(PermHttpError::db)?;
    if !taken.is_empty() {
        return Err(PermHttpError::Conflict(format!(
            "the migration would create rule(s) named {}, and a rule with that name already \
             exists; rename the policy or the existing rule first",
            taken
                .iter()
                .map(|n| format!("{n:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

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
    lock_services_in_tx(tx, &merge_service_ids(&affected, &[service_id])).await?;

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

    // ── step 1 + 2: insert through the one write implementation ───────────
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

    // ── step 3: switch every original off (the column) ────────────────────
    for m in &response.policies {
        set_policy_enabled_via(tx, m.policy_id, false)
            .await
            .map_err(store_refusal)?;
    }

    // ── step 4: provenance (the count lands in step 6) ────────────────────
    let actor = p.email.clone();
    for m in &response.policies {
        let inserted = sqlx::query(
            "INSERT INTO perm.migrated_from \
               (service_id, policy_name, grant_ids, rule_ids, probes_proved, migrated_by) \
             VALUES ($1, $2, $3, $4, 0, $5)",
        )
        .bind(service_id)
        .bind(&m.policy_name)
        .bind(&m.grant_ids)
        .bind(&m.rule_ids)
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
    let cases = match build_cases(&affected, &before, &after, &originals, &carriers) {
        Ok(c) => c,
        Err(sentence) => {
            response.refusal = Some(sentence);
            return Ok(response);
        }
    };
    response.profiles = cases
        .iter()
        .map(|c| {
            let (bn, an) = (policy_names(c.before), policy_names(c.after));
            GatedProfile {
                service_id: c.service_id,
                service_name: c.service_name.clone(),
                plugin_versions: c.versions.iter().copied().map(Into::into).collect(),
                probes: c.probes.len(),
                disagreements: 0,
                removed_policies: bn.difference(&an).cloned().collect(),
                added_policies: an.difference(&bn).cloned().collect(),
            }
        })
        .collect();
    let evaluations = match probe_budget(
        cases.iter().map(|c| (c.probes.len(), c.versions.len())),
        MAX_PROBE_EVALUATIONS,
    ) {
        Ok(n) => n,
        Err(sentence) => {
            response.refusal = Some(sentence);
            return Ok(response);
        }
    };
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
          WHERE service_id = $1 AND policy_name = ANY($2)",
    )
    .bind(service_id)
    .bind(&originals)
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

/// `migrate/undo`, on the caller's transaction (the caller commits): delete
/// the objects provenance names, recompile, switch the original back on,
/// delete the provenance row. A MECHANICAL rollback — possible only because
/// the original was switched off, not deleted.
///
/// 404 for a profile, or a name with no migration on record; 409 when the
/// original policy no longer exists (nothing to switch back on). Objects
/// provenance names that were since deleted by hand are simply not there to
/// delete, and the counts say so.
pub(crate) async fn undo_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
    req: &UndoRequest,
) -> Result<UndoResponse, PermHttpError> {
    validate_names(&req.policy_names)?;
    get_service_by_id_via(tx, service_id).await?;

    struct Record {
        policy_id: i64,
        policy_name: String,
        grants: Vec<egret_perm::Grant>,
        rules: Vec<egret_perm::Rule>,
    }
    let mut records = Vec::new();
    let mut affected: Vec<i64> = Vec::new();
    for name in &req.policy_names {
        let row: Option<(Vec<Uuid>, Vec<Uuid>)> = sqlx::query_as(
            "SELECT grant_ids, rule_ids FROM perm.migrated_from \
              WHERE service_id = $1 AND policy_name = $2 FOR UPDATE",
        )
        .bind(service_id)
        .bind(name)
        .fetch_optional(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
        let Some((grant_ids, rule_ids)) = row else {
            return Err(PermHttpError::NotFound);
        };
        let policy_id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM policy.authz_policies WHERE service_id = $1 AND name = $2",
        )
        .bind(service_id)
        .bind(name)
        .fetch_optional(&mut **tx)
        .await
        .map_err(PermHttpError::db)?;
        let Some(policy_id) = policy_id else {
            return Err(PermHttpError::Conflict(format!(
                "the hand-written policy {name:?} no longer exists, so undoing its migration \
                 would delete the grants and rules that replaced it and restore nothing"
            )));
        };
        let mut grants = Vec::new();
        for id in grant_ids {
            if let Some(g) = egret_perm::get_grant_via(tx, id).await? {
                affected =
                    merge_service_ids(&affected, &affected_services_for_grant(tx, &g).await?);
                grants.push(g);
            }
        }
        let mut rules = Vec::new();
        for id in rule_ids {
            if let Some(r) = egret_perm::get_rule_via(tx, id).await? {
                affected = merge_service_ids(&affected, &affected_services_for_rule(tx, &r).await?);
                rules.push(r);
            }
        }
        records.push(Record {
            policy_id,
            policy_name: name.clone(),
            grants,
            rules,
        });
    }

    // Before any DELETE — see `lock_services_in_tx`.
    lock_services_in_tx(tx, &merge_service_ids(&affected, &[service_id])).await?;
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
        sqlx::query("DELETE FROM perm.migrated_from WHERE service_id = $1 AND policy_name = $2")
            .bind(service_id)
            .bind(&r.policy_name)
            .execute(&mut **tx)
            .await
            .map_err(PermHttpError::db)?;
    }
    Ok(UndoResponse {
        policies: out,
        compile: CompileResponse::from_summaries(affected, &summaries),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_budget_counts_every_version_of_every_profile_and_refuses_over_the_cap() {
        assert_eq!(probe_budget([(100, 1), (50, 2)], 200), Ok(200));
        let refused = probe_budget([(100, 1), (50, 2)], 199).unwrap_err();
        assert!(refused.contains("200 probe evaluations"), "{refused}");
        assert!(refused.contains("2 profile(s)"), "{refused}");
        assert!(refused.contains("never cut short"), "{refused}");
        assert!(refused.contains("limit of 199"), "{refused}");
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
    fn one_case_per_affected_profile() {
        let before = inputs(envelope(&["orig"]));
        let after = BTreeMap::from([(1, envelope(&["perm-grant-x"]))]);
        let cases = build_cases(&[1], &before, &after, &["orig".into()], &[1]).expect("cases");
        assert_eq!(cases.len(), 1);
        assert!(!cases[0].probes.is_empty());
        assert!(!std::ptr::eq(cases[0].before, cases[0].after));
    }
}
