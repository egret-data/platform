//! `recompile_service`/`recompile_service_in_tx`: keep a profile's compiled
//! Ranger policies (`policy.authz_policies`, with `perm.compiled` recording
//! provenance) true to the CURRENT `perm` model and the CURRENT tag
//! assignments underneath it (SP4a Task 7).
//!
//! **Why this exists (D5).** On Trino, a tag-selected rule compiles to ONE
//! tag-masking/tag-rowfilter policy that the Ranger tag plugin resolves
//! itself; egret never has to touch it again. StarRocks has no tag service
//! (upstream `starrocks#67458`), so the SAME rule compiles to one policy PER
//! matching column, enumerated at compile time (`egret-perm-compile::rules`) —
//! and that enumeration is stale the moment a new column gets the tag.
//! `egret-catalog-registry::tag_version::bump_bound_tag_versions` already
//! bumps a bound service's `tag_version` on every tag write; this module is
//! what a caller (`egret-bi::catalog_tags`, alongside that bump) calls to
//! actually recompile the StarRocks profile on the same trigger. What
//! `ranger-tagsync` does for Trino, this does for StarRocks — and, because
//! the model can change too (a grant or rule edited/deleted), for every
//! profile on every recompile, not only the StarRocks-tag case.
//!
//! **Fix round 1 (review response) — two changes to how truth is found and
//! how writes are composed:**
//!
//! 1. **The sweep no longer reads through `perm.compiled`.** `perm.compiled.grant_id`/
//!    `rule_id` are `ON DELETE CASCADE` (`migrations/0154_perm_model.sql:121-122`):
//!    deleting a grant or rule erases its PROVENANCE row, but the
//!    `policy.authz_policies` row it produced survives untouched. Round 1's
//!    `apply_diff` joined `perm.compiled ⋈ policy.authz_policies` to find
//!    "what currently exists for this service" — so a deleted grant's policy
//!    became invisible to that join the instant the grant was deleted, never
//!    entered the delete sweep, and was enforced forever. `apply_diff_in_tx`
//!    now sources "what currently exists" directly from `policy.authz_policies`,
//!    scoped by `service_id` and this compiler's own name prefixes
//!    (`perm-grant-`, `perm-rule-`) — a dangling `policy.authz_policies` row
//!    is visible here with or without a surviving `perm.compiled` row, so it
//!    reaches the delete branch exactly like any other no-longer-desired
//!    policy. `perm.compiled` provenance rows are still written (and
//!    self-healed via `ON CONFLICT DO NOTHING` on every create/update/keep,
//!    not only on create) for the "why does this policy exist" story, but
//!    they are no longer the sweep's source of truth.
//! 2. **`recompile_service_in_tx` runs on a caller-owned transaction**, the
//!    same shape `egret_perm::sync_assignment_grants_in_tx` already
//!    established: every read (the model, the reachable catalogs, the
//!    tagged columns, the tag name, the existing compiled state) AND every
//!    write happen against the SAME `&mut Transaction`, via
//!    `egret_perm::{list_grants_via, list_rules_via}` and
//!    `egret_policy::{get_service_by_id_via, catalogs_for_service_via,
//!    tag_assignments_for_service_via, tag_name_for_key_via}` (all
//!    connection-generic, added in this fix round for exactly this reason).
//!    Round 1 read through `PermStore`/`PolicyStore`'s own POOLS — a second
//!    connection that, under Postgres' default READ COMMITTED isolation,
//!    cannot see the caller's own transaction's uncommitted writes (the very
//!    tag assignment that triggered the recompile), and whose WRITES (the
//!    `policy_version` bump) collided with the caller's already-open
//!    transaction on the SAME `policy.authz_services` row
//!    `bump_bound_tag_versions` had just locked — a self-deadlock, avoided in
//!    round 1 only by running the recompile in a SEPARATE, later-committing
//!    transaction after the tag write, which reintroduced exactly the
//!    silent-staleness risk this task exists to close (a crash or a closed
//!    browser between the two commits). `recompile_service` (the pool-based,
//!    brief-literal entry point) is now a thin wrapper: open one transaction,
//!    call `recompile_service_in_tx`, commit.
//!
//! **What this module does about `WorkspaceMembers` (SP4a D3, delivered).**
//! Every catalog assigned to a workspace carries exactly one implicit
//! `origin = 'workspace_assignment'` grant whose only principal is
//! `workspace_members` (`egret_perm::assignment`). D7 measured that a Ranger
//! GROUP item matches nobody on any engine egret provisions, so that principal
//! compiles to **the member list enumerated at compile time**, as bare
//! usernames — [`crate::members::expand_enumerated_principals`], which since
//! SP4c Task 2b expands a `role` principal the same way and for the same
//! measured reason, run here on the
//! caller's own connection before anything is compiled, so `compile_grant`
//! and `compile_rule` stay free of I/O and keep refusing the unexpanded kind
//! as a defence in depth.
//!
//! Two consequences worth stating where the driver lives:
//!
//! - **A membership write must recompile**, exactly as a tag write does — the
//!   same staleness shape D5 gave the StarRocks tag expansion, with the same
//!   answer, since egret owns the compile.
//!   [`affected_services_for_workspace_members`] answers "which profiles" and
//!   `egret_workspace::members::MemberRepo` is what asks it, inside its own
//!   write transaction. The paths that are NOT covered, and why, are in
//!   `crate::members`' module doc and in the design spec's D3 — they are
//!   recorded, not silently absent.
//! - **A grant whose principals expand to NOTHING compiles to no policy**,
//!   recorded in [`CompileSummary::skipped`] and logged, rather than to a
//!   Ranger item naming nobody. It is the same fail-closed outcome the
//!   evaluator reaches for that grant (`matches_set` needs one `include`
//!   match), and it keeps a meaningless item out of the profile.
//!
//! Every OTHER `CompileError` — `ExpansionCeiling`, `UnusableUserRef`,
//! `UnsupportedMaskType`, an unparseable custom mask, an unexpressible
//! exemption, a missing tag service — is a genuine authoring or resolution bug
//! and ABORTS the whole recompile for this service: weakening those refusals
//! to a skip would be exactly the "make something pass" the compiler exists to
//! refuse. `UnexpandedPrincipalKind` is still caught and skipped rather than
//! propagated, because it is now unreachable-by-construction and a driver that
//! crashed a tag write over an internal invariant would be worse than one that
//! records it.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use egret_perm::{
    list_engine_grants_for_profile_via, list_grants_via, list_rules_via, CompileError, EngineGrant,
    EngineType, Grant, PermStore, Rule, Selector,
};
use egret_policy::admin::access_to_ranger_json;
use egret_policy::{
    catalogs_for_service_via, compiler_targeted_services_via, get_service_by_id_via,
    service_id_by_name_via, services_and_tag_services_via, services_for_catalog_via,
    tag_assignments_for_service_via, tag_name_for_key_via, PolicyStore, COMPILER_CLEARANCE_PREFIX,
    COMPILER_ENGINE_GRANT_PREFIX, COMPILER_GRANT_PREFIX, COMPILER_RULE_PREFIX,
};

use crate::grants::compile_grant;
use crate::rules::{compile_rule, stamp_service_type, ResolvedNames, TableTarget};

/// The StarRocks per-column expansion ceiling this driver enforces.
///
/// **Measured, not guessed** (Task 10, 2026-09-24, kind `kind-egret`, StarRocks
/// profile `sp4a10-sr`; full table in the design spec's D5 and in
/// `docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md`). N tagged
/// columns → a full profile rebuild and a full Ranger download:
///
/// | policies | compile | download |
/// |---|---|---|
/// | 256 | 0.226 s | 198 KB |
/// | 506 | 0.489 s | 381 KB |
/// | 1006 | 0.653 s | 748 KB |
/// | 2006 | 2.224 s | 1.48 MB |
///
/// The download is linear (~740 bytes/policy) and never the constraint; compile
/// time is, and it turns super-linear past roughly 1000 policies in one profile
/// (1006 → 2006 costs 3.4× the time). 500 per rule is chosen so that a rule at
/// the cap plus the profile's other rules still sits inside the linear region.
///
/// **This ceiling alone does not bound the thing that costs (SP4a Task 12).**
/// It is per-RULE, and compile cost is driven by the profile TOTAL: four
/// rules of 500 columns each sat at or under this cap individually and
/// compiled 2006 policies with no refusal at all — the live gate's own
/// finding. [`PROFILE_EXPANSION_CEILING`] is the second half of the guard,
/// enforced on the profile's running total rather than any one rule.
pub const DEFAULT_EXPANSION_CEILING: usize = 500;

/// The profile-TOTAL ceiling — the follow-up [`DEFAULT_EXPANSION_CEILING`]'s
/// own doc comment named as a known gap, closed here (SP4a Task 12).
///
/// **Same measurement as `DEFAULT_EXPANSION_CEILING`** (Task 10, 2026-09-24,
/// kind `kind-egret`, StarRocks profile `sp4a10-sr`; full table in the design
/// spec's D5 and in `docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md`),
/// read as wall-clock compile time by policy count in ONE profile:
///
/// | policies | compile |
/// |---|---|
/// | 16 | 0.054 s |
/// | 256 | 0.226 s |
/// | 506 | 0.489 s |
/// | 1006 | 0.653 s |
/// | 1506 | 1.331 s |
/// | 2006 | 2.224 s |
///
/// Compile time is roughly linear up to ~1000 policies and turns
/// super-linear past it: 1006 → 2006 is a 2× policy count for a 3.4× time
/// cost (2.224 s / 0.653 s), not 2×. 1000 is the last point still inside the
/// linear region — the same reasoning `DEFAULT_EXPANSION_CEILING` uses for
/// 500, one level up.
///
/// **Why this has to be a SEPARATE ceiling from the per-rule one, not a
/// smaller per-rule number.** The per-rule cap bounds one rule; it says
/// nothing about how many rules a profile carries. Four rules each sitting
/// AT the per-rule cap (500) sum to 2000 — over the knee — while every rule
/// individually reports "under my cap." Only a check against the profile's
/// own running total catches that.
///
/// **Why a naive "total > ceiling ⇒ refuse" bricks an over-cap profile.**
/// [`apply_diff_in_tx`] is what a tag write, a grant write, AND a rule
/// *delete* all recompile through. If a profile is already over
/// `PROFILE_EXPANSION_CEILING` — as one legitimately could be, since this
/// ceiling did not exist before Task 12 — refusing every recompile of that
/// profile would refuse the delete an operator needs to shrink it back
/// under the cap: the delete writes the model row, the recompile that
/// accompanies it refuses, and the whole write rolls back (Task 9's I-1 is
/// this exact shape one level down — an uncompilable grant poisoning every
/// later recompile). So this ceiling refuses only a change that would push
/// the total OVER the ceiling; a recompile whose new total is less than or
/// equal to the CURRENTLY STORED total for this profile — shrinking, or
/// unchanged — is always allowed, even while both numbers stay above the
/// ceiling. See [`apply_diff_in_tx`] for where "currently stored" is read
/// and why that read cannot deadlock.
pub const PROFILE_EXPANSION_CEILING: usize = 1000;

// The name prefixes every policy `compile_grant`/`compile_rule` produces
// carries (`grants.rs`'s `format!("{COMPILER_GRANT_PREFIX}{}", g.id)`,
// `rules.rs`'s `format!("{COMPILER_RULE_PREFIX}{}", ...)`). `apply_diff_in_tx`
// uses these to find every policy THIS COMPILER owns under a service
// directly from `policy.authz_policies`, independent of whether
// `perm.compiled` still has a provenance row for it (fix round 1 Critical —
// see module doc).
//
// Task 8 fix round 2 (I-1): `egret-policy`'s per-policy write guard
// (`refuse_if_compiler_owned`) needs this SAME identity test to decide
// whether a policy a human is trying to edit/delete is one this sweep would
// itself reap/overwrite. Since `egret-perm-compile` depends on
// `egret-policy` and not the reverse, the constants live THERE
// (`egret_policy::COMPILER_GRANT_PREFIX`/`COMPILER_RULE_PREFIX`) and this
// crate imports them, rather than each crate keeping its own copy — two
// copies of this string WILL drift, and a drift makes a policy the sweep
// reaps look editable to the guard, or the reverse.

/// Which half of the `perm` model a compiled policy (or a skip) came from —
/// plus, since SP4c Task 2, a third source that is not in the `perm` model at
/// all: a TAG's classification clearance (`classification_policies`), compiled
/// by [`crate::clearance`]. Its `SkippedItem::id` is the `tags.id`, not a
/// grant or rule id. And since F11 a fourth: an ENGINE GRANT
/// (`perm.engine_grants`, [`crate::engine_grants`]), whose id is the engine
/// grant's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Grant,
    Rule,
    Clearance,
    EngineGrant,
}

impl SourceKind {
    fn as_str(self) -> &'static str {
        match self {
            SourceKind::Grant => "grant",
            SourceKind::Rule => "rule",
            SourceKind::Clearance => "clearance",
            SourceKind::EngineGrant => "engine_grant",
        }
    }
}

/// A grant or rule this recompile could not express and left uncompiled —
/// see the module doc comment for exactly which failures land here (only
/// `CompileError::UnexpandedPrincipalKind`) versus which abort the whole
/// recompile. Logged via `tracing::warn!` at the point it is discovered, in
/// addition to being recorded here (fix round 1 — the caller must not have
/// to remember to inspect this field for the skip to be visible anywhere).
#[derive(Debug, Clone)]
pub struct SkippedItem {
    pub source_kind: SourceKind,
    pub id: Uuid,
    pub reason: String,
}

/// What one `recompile_service`/`recompile_service_in_tx` call did. Every
/// count is scoped to THIS service — a caller comparing two calls' summaries
/// (or the DB state before/after) is the only way to tell a no-op recompile
/// from a busy one; see `crates/egret-perm-compile/tests/recompile.rs`'s
/// `recompiling_an_unchanged_model_writes_nothing_and_keeps_policy_ids`.
#[derive(Debug, Clone, Default)]
pub struct CompileSummary {
    pub created: usize,
    pub updated: usize,
    pub deleted: usize,
    pub unchanged: usize,
    pub skipped: Vec<SkippedItem>,
}

impl CompileSummary {
    /// Whether this recompile actually wrote anything — the same predicate
    /// that gates the single `policy_version` bump at the end (never bump on
    /// a no-op: "every plugin re-downloads for nothing" otherwise).
    pub fn wrote_anything(&self) -> bool {
        self.created > 0 || self.updated > 0 || self.deleted > 0
    }
}

/// One policy this recompile wants to exist. **Which service it must exist
/// under travels in the map KEY** ([`PolicyKey`]), not here: a tag-selected
/// rule on Trino compiles to a body whose own `service` field names the
/// profile's TAG service (`rules::build_tag_body` → `tagify`), and that is
/// where the row has to land or the tag plugin never downloads it. See
/// [`target_service_id`] and `apply_diff_in_tx`.
struct DesiredPolicy {
    body: Value,
    source_kind: SourceKind,
    grant_id: Option<Uuid>,
    rule_id: Option<Uuid>,
}

/// The `(service_id, name)` a desired policy is keyed by — the same pair
/// `policy.authz_policies`' own `UNIQUE (service_id, name)` uses, and the
/// reason the desired map is keyed by a pair at all: one recompile now writes
/// into two services (the profile and its tag service).
type PolicyKey = (i64, String);

/// Resolve the service a rendered policy body belongs to from the body's OWN
/// `service` field, against the names this recompile is allowed to write into.
///
/// **This is the fix for Task 10's case T3, and its shape is the point.**
/// Round 1 stamped every row with the profile's `service_id` and never read
/// the body at all, so a tag policy whose body said
/// `"service": "sp3c2-trino-live_tag"` was stored under the *trino* profile
/// (52) instead of the tag service (51). It deployed, egret listed it, the
/// tag plugin's own download returned **0 policies**, and the masked column
/// came back with its real values. Routing by the body's own declaration
/// makes the row and the body unable to disagree; an unknown name is a hard
/// error rather than a silent fallback to the profile, because a fallback is
/// exactly the behaviour that produced the defect.
fn target_service_id(
    body: &Value,
    writable: &HashMap<String, i64>,
    what: &str,
) -> Result<i64, CompileError> {
    let named = body
        .get("service")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            CompileError::Db(format!(
                "compiled body for {what} carries no 'service' field"
            ))
        })?;
    writable.get(named).copied().ok_or_else(|| {
        CompileError::Db(format!(
            "compiled body for {what} names service {named:?}, which is not a service this              recompile may write into ({:?})",
            writable.keys().collect::<Vec<_>>()
        ))
    })
}

fn db_err<E: std::fmt::Display>(e: E) -> CompileError {
    CompileError::Db(e.to_string())
}

/// `None` when `service_type` names something this compiler was never built
/// to target (SP4a's compiler is exactly Trino + StarRocks — see the crate's
/// own module doc) — a tag service (`type = "tag"`), or any other
/// `policy.authz_services` row a caller's `services_bound_to_warehouse`-style
/// resolution might hand this function. This is deliberately NOT a
/// `CompileError`: `engine_mgmt.engines.authz_service_id` only ever binds an
/// engine to its OWN trino/starrocks profile in production (never to a tag
/// service — that binding is reached separately, through the profile's
/// `config_jsonb->>'tagService'`, the same way `tag_assignments_for_service`
/// resolves it), so a non-trino/starrocks `service_id` reaching this function
/// means "nothing here for this compiler," not "something is broken" —
/// treating it as a hard error would fail an entire tag write's recompile
/// pass over a service this compiler was never asked to produce policies for.
// `pub(crate)`: SP4d-2a Task 5's `explain` asks the same question of a
// `policy.authz_services` row — which engine's plugin, if any, downloads this
// profile — and a second copy of the match would be free to disagree with the
// compiler about which profiles are in scope at all.
pub(crate) fn engine_type_for(service_type: &str) -> Option<EngineType> {
    match service_type {
        "trino" => Some(EngineType::Trino),
        "starrocks" => Some(EngineType::StarRocks),
        _ => None,
    }
}

/// Extract the `name` field `compile_grant`/`compile_rule`'s rendered JSON
/// always carries (every Ranger policy-body builder in `egret-policy`'s
/// `ranger_json` module sets it from the caller-supplied name) — this is the
/// desired policy's stable identity, matched against `policy.authz_policies`'s
/// own `UNIQUE (service_id, name)`.
fn body_name(body: &Value, what: &str) -> Result<String, CompileError> {
    body.get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            CompileError::Db(format!("compiled body for {what} carries no 'name' field"))
        })
}

/// Compile `service_id`'s desired policy set from the CURRENT `perm` model
/// (every read against `conn`, so a caller running this inside its own open
/// transaction sees its own uncommitted writes). `None` means `service_id`
/// names something this compiler doesn't target (see [`engine_type_for`]) —
/// nothing to diff, nothing to write.
async fn compile_desired(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Option<Compiled>, CompileError> {
    let svc = get_service_by_id_via(conn, service_id)
        .await
        .map_err(db_err)?;
    let Some(engine) = engine_type_for(&svc.r#type) else {
        return Ok(None);
    };

    let catalogs = catalogs_for_service_via(conn, service_id)
        .await
        .map_err(db_err)?;
    let catalog_names: HashMap<Uuid, String> =
        catalogs.into_iter().map(|c| (c.id, c.name)).collect();

    // The services this recompile may write into, by the NAME a compiled body
    // stamps on itself: the profile always, and its tag service when the
    // profile both configures one AND that configured name resolves to a real
    // `policy.authz_services` row. A `tagService` naming nothing is treated
    // exactly like an absent one — the tag plugin polls that service BY NAME
    // (`/service/plugins/policies/download/{service}`, 404 on an unknown
    // name), so a policy filed under a profile that does not exist could not
    // be served to anyone. That lands in the same `MissingTagService` skip the
    // unconfigured case already took, rather than aborting the recompile.
    let mut writable: HashMap<String, i64> = HashMap::new();
    writable.insert(svc.name.clone(), service_id);
    let mut sweep: Vec<i64> = vec![service_id];

    let tag_service: Option<String> = if engine == EngineType::Trino {
        match svc
            .config_jsonb
            .get(egret_policy_api::TAG_SERVICE_CONFIG_KEY)
            .and_then(|v| v.as_str())
        {
            Some(name) => match service_id_by_name_via(conn, name).await.map_err(db_err)? {
                Some(tag_id) => {
                    writable.insert(name.to_string(), tag_id);
                    sweep.push(tag_id);
                    Some(name.to_string())
                }
                None => {
                    tracing::warn!(
                        service_id,
                        tag_service = %name,
                        "profile's `tagService` names no policy profile — tag rules cannot \
                         compile for this profile"
                    );
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };

    // Ascending: `apply_diff_in_tx` takes a row lock per swept service and the
    // acquisition order is what turns two overlapping recompiles into a queue
    // instead of a deadlock (see that function).
    sweep.sort_unstable();
    sweep.dedup();

    let mut all_grants = list_grants_via(conn).await.map_err(db_err)?;
    let mut all_rules = list_rules_via(conn).await.map_err(db_err)?;
    // D3: turn every `workspace_members` principal — and, since SP4c Task 2b,
    // every `role` principal, and since J-F3 Task 1b every non-`public` `group`
    // principal — into the people it stands for BEFORE anything is compiled,
    // so the pure compilers below never see one. See `crate::members`
    // for what a member is, what it costs, and why a role name cannot be left
    // for Ranger to resolve.
    // F11: the engine grants of every engine bound to this profile, expanded
    // in the same call so a person named by a grant and an engine grant
    // resolves to one name.
    let mut engine_grants = list_engine_grants_for_profile_via(conn, service_id)
        .await
        .map_err(db_err)?;
    crate::members::expand_enumerated_principals(
        conn,
        &mut all_grants,
        &mut all_rules,
        &mut engine_grants,
    )
    .await?;

    let mut desired: BTreeMap<PolicyKey, DesiredPolicy> = BTreeMap::new();
    let mut skipped: Vec<SkippedItem> = Vec::new();

    for g in &all_grants {
        let Some(catalog_name) = catalog_names.get(&g.scope.catalog_id) else {
            continue; // not reachable by this service — out of scope
        };
        if g.principals.include.is_empty() {
            // Nothing to name. Emitting the policy anyway would write a Ranger
            // item with empty `users`/`groups`/`roles`, which matches nobody —
            // the same answer, said less clearly, and one more policy against
            // the profile total. Recorded so "my workspace has no members yet"
            // is visible rather than inferred from a missing policy.
            let reason = "the grant's principals name no engine principal (an empty include, or \
                          a workspace_members, role or group reference that expands to no \
                          member)"
                .to_string();
            tracing::warn!(
                service_id,
                grant_id = %g.id,
                reason = %reason,
                "grant skipped during recompile"
            );
            skipped.push(SkippedItem {
                source_kind: SourceKind::Grant,
                id: g.id,
                reason,
            });
            continue;
        }
        match compile_grant(g, catalog_name, &svc.name, engine) {
            Ok(input) => {
                let name = input.name.clone();
                let body = stamp_service_type(access_to_ranger_json(&input), engine);
                let target = target_service_id(&body, &writable, &format!("grant {}", g.id))?;
                desired.insert(
                    (target, name),
                    DesiredPolicy {
                        body,
                        source_kind: SourceKind::Grant,
                        grant_id: Some(g.id),
                        rule_id: None,
                    },
                );
            }
            Err(CompileError::UnexpandedPrincipalKind { kind }) => {
                let reason = format!(
                    "principal kind {kind:?} reached the compiler unexpanded — a bug in \
                     `crate::members::expand_enumerated_principals`, not in the grant"
                );
                tracing::warn!(
                    service_id,
                    grant_id = %g.id,
                    reason = %reason,
                    "grant skipped during recompile"
                );
                skipped.push(SkippedItem {
                    source_kind: SourceKind::Grant,
                    id: g.id,
                    reason,
                });
            }
            Err(other) => return Err(other),
        }
    }

    compile_engine_grants(
        conn,
        service_id,
        &svc.name,
        engine,
        &engine_grants,
        &writable,
        &mut desired,
        &mut skipped,
    )
    .await?;

    for r in &all_rules {
        compile_one_rule(
            r,
            engine,
            &svc.name,
            tag_service.as_deref(),
            conn,
            service_id,
            &catalog_names,
            &writable,
            &mut desired,
            &mut skipped,
        )
        .await?;
    }

    // **SP4c Task 2: a classification's CLEARANCE, which AMENDS SP4a's D2**
    // ("tags drive rules only, never access") — the argument is at the site,
    // in `crate::clearance`'s module doc, because a classification exists to
    // withhold data and a mask still answers the query. Clearances come from
    // `classification_policies`, not from the `perm` model.
    //
    // **Installation-wide on Trino, service-scoped on StarRocks** (SP4c Task
    // 2d). A Trino clearance is one TAG policy carrying no catalog resource, so
    // it compiles identically into every Trino profile with a tag service. A
    // StarRocks clearance expands to one ACCESS deny per tagged column
    // reachable from THIS service — the same `tag_assignments_for_service_via`
    // expansion a tag-selected rule uses above — so `service_id` and the
    // profile's own name both have to reach the compiler.
    let clearances = crate::clearance::compile_clearance(
        conn,
        engine,
        service_id,
        &svc.name,
        tag_service.as_deref(),
    )
    .await?;
    skipped.extend(clearances.skipped);
    let whole_tables = clearances.whole_tables;
    for (name, body) in clearances.policies {
        let target = target_service_id(&body, &writable, &format!("clearance {name}"))?;
        desired.insert(
            (target, name),
            DesiredPolicy {
                body,
                source_kind: SourceKind::Clearance,
                grant_id: None,
                rule_id: None,
            },
        );
    }

    // **View governance A2: the StarRocks clearance MASK, folded through the
    // evaluator.** A base deny does not hold through a plain view; a base mask
    // does. Each classified column gets a priority-1 data-mask policy whose
    // `{public}` item is `MASK_NULL` and whose cleared items reproduce what the
    // profile's OTHER masks give each cleared user — computed by
    // `egret-ranger-eval`, refused when undecidable. Folded HERE because only
    // the complete desired set holds every rule mask (exact AND wildcard) that
    // can reach the column, and BEFORE the overlap check below, which must see
    // the masks it will serve. Read `crate::clearance_mask`'s module doc.
    if engine == EngineType::StarRocks && !clearances.masks.is_empty() {
        fold_clearance_masks(
            conn,
            service_id,
            &svc.name,
            engine,
            clearances.masks,
            &writable,
            &mut desired,
            &mut skipped,
        )
        .await?;
    }

    // **SP4d debt Task 6, item 1: the write side of `EvalOrderAmbiguous`** —
    // widened by SP4d-4 Task 3 to the profile's whole evaluation unit.
    // Checked HERE, on the complete desired set, because no single
    // `compile_rule`/`compile_clearance` call can see another rule's output —
    // the pair SP4d-1's F2 measured came from two different `perm.rules` rows
    // compiled independently, and the ambiguity exists only in their union.
    //
    // The rule is `egret_policy::unit_overlaps`, spelled once and shared with
    // the policy editor; it takes every within-download pair from
    // `ambiguous_arbitrations_in` (read `egret_policy::ambiguous_arbitration`
    // for the measurement and for why an AGREEING tie is admitted) and adds
    // the pairs only the tag assignments reveal — a tag rule and a path rule on
    // one tagged column, two tag rules whose tags share a column. See
    // `crate::overlap` for the write paths this covers.
    //
    // ⚠ It runs BEFORE `apply_diff_in_tx`, so a refusal writes nothing at all
    // — the caller's transaction is intact and its route returns the refusal
    // rather than rolling back a partial sweep. That also makes
    // `POST /perm/preview` report it before the steward presses Save, since
    // preview runs the real `write_rule` in a rolled-back transaction.
    //
    // **Only pairs THIS compile creates are refused** (user ruling
    // 2026-10-12, SP4d-4 Task 3 fix round 2 — superseding the wedge round 1
    // widened). The stored unit (compiled and hand-written, as served now) is
    // the BEFORE; the unit with this profile's compiler-owned policies
    // replaced by `desired` is the AFTER; a pair whose identity (names, both
    // item arrays, the set of coordinates) is new is refused. A stored pair is
    // reported, not refused, so it no longer blocks unrelated writes — which
    // for a tag-rule write or a tag rename meant every engine-bound profile in
    // the installation. A migration's originals are switched off before its
    // rule writes (`migrate.rs`, step 1), so they are in neither set. See
    // `crate::overlap::refuse_created_by_compile_via`.
    let tag_service_id = sweep.iter().copied().find(|&id| id != service_id);
    let rule_names: HashMap<Uuid, &str> =
        all_rules.iter().map(|r| (r.id, r.name.as_str())).collect();
    let for_overlap: Vec<crate::overlap::CompiledForOverlap<'_>> = desired
        .iter()
        .map(|((sid, name), dp)| crate::overlap::CompiledForOverlap {
            service: *sid,
            is_tag_service: Some(*sid) == tag_service_id,
            policy_name: name,
            body: &dp.body,
            rule: dp.rule_id.map(|id| egret_perm::OverlapRule {
                id: Some(id),
                name: rule_names
                    .get(&id)
                    .map_or_else(|| name.clone(), |n| (*n).to_string()),
            }),
        })
        .collect();
    crate::overlap::refuse_created_by_compile_via(
        conn,
        &svc.name,
        service_id,
        tag_service_id,
        &for_overlap,
    )
    .await?;

    Ok(Some(Compiled {
        desired,
        skipped,
        sweep,
        profile_name: svc.name,
        largest: whole_tables,
    }))
}

/// F11 (spec §4.2): compile every engine grant of the profile into
/// `desired`. A grant with no grantee left, or whose impersonation target can
/// no longer be compiled (spec E11), or that the compiler cannot express, is
/// SKIPPED with its reason rather than failing the profile: a user deleted or
/// renamed elsewhere must not fail every recompile.
#[allow(clippy::too_many_arguments)]
async fn compile_engine_grants(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    profile_name: &str,
    engine: EngineType,
    engine_grants: &[EngineGrant],
    writable: &HashMap<String, i64>,
    desired: &mut BTreeMap<PolicyKey, DesiredPolicy>,
    skipped: &mut Vec<SkippedItem>,
) -> Result<(), CompileError> {
    let mut skip = |id: Uuid, reason: String| {
        tracing::warn!(
            service_id,
            engine_grant_id = %id,
            reason = %reason,
            "engine grant skipped during recompile"
        );
        skipped.push(SkippedItem {
            source_kind: SourceKind::EngineGrant,
            id,
            reason,
        });
    };
    for g in engine_grants {
        if g.principals.include.is_empty() {
            skip(
                g.id,
                "the engine grant's principals name no engine principal (a role or group \
                 reference that expands to no member)"
                    .to_string(),
            );
            continue;
        }
        let target = match g.target_user_id {
            Some(user_id) => {
                match crate::engine_grants::impersonation_target_ok(conn, service_id, user_id)
                    .await?
                {
                    Ok(username) => Some(username),
                    Err(reason) => {
                        skip(g.id, reason);
                        continue;
                    }
                }
            }
            None => None,
        };
        match crate::engine_grants::compile_engine_grant(g, target.as_deref(), profile_name, engine)
        {
            Ok(input) => {
                let name = input.name.clone();
                let body = stamp_service_type(access_to_ranger_json(&input), engine);
                let target_service =
                    target_service_id(&body, writable, &format!("engine grant {}", g.id))?;
                desired.insert(
                    (target_service, name),
                    DesiredPolicy {
                        body,
                        source_kind: SourceKind::EngineGrant,
                        grant_id: None,
                        rule_id: None,
                    },
                );
            }
            Err(e @ CompileError::InvalidEngineGrant { .. })
            | Err(e @ CompileError::UnexpandedPrincipalKind { .. }) => skip(g.id, e.to_string()),
            Err(other) => return Err(other),
        }
    }
    Ok(())
}

/// What one profile's compile produced: the desired policies keyed by the
/// `(service_id, name)` pair they must exist under, the items this compile
/// could not express, and **every service whose compiler-owned policies this
/// recompile is responsible for** — the profile, plus its tag service when it
/// has one.
///
/// `sweep` is the half Task 10's defect 2 makes load-bearing. Once a tag rule
/// compiles into a DIFFERENT service than the profile being recompiled, a
/// delete sweep scoped to the profile alone would never see that rule's policy
/// again — which is Task 7's own fix-round-1 Critical (a deleted rule's policy
/// enforced forever) re-created one service over. So the sweep set is computed
/// from the profile's configuration, not from what happened to compile: a
/// profile with a tag service sweeps it even when zero tag policies are
/// desired, which is exactly the "the rule was deleted" case.
struct Compiled {
    desired: BTreeMap<PolicyKey, DesiredPolicy>,
    skipped: Vec<SkippedItem>,
    sweep: Vec<i64>,
    /// The profile's own service NAME — carried alongside `sweep` so
    /// [`apply_diff_in_tx`]'s [`CompileError::ProfileExpansionCeiling`]
    /// refusal can name the profile, not just its numeric id.
    profile_name: String,
    /// The largest whole-table classified tables (D2), for the same refusal.
    largest: Vec<egret_perm::TableCount>,
}

/// Fold one priority-1 clearance mask per classified column into `desired`
/// (StarRocks only — see the call site and `crate::clearance_mask`).
///
/// `others` is every data-mask body this profile will serve that is not a
/// clearance mask: the desired rule masks, plus the profile's ENABLED
/// hand-written masks (stored policies outside the compiler's prefixes). The
/// latter are served beside the compiled set and reach a cleared user exactly
/// as a rule mask does, so leaving them out would fold a cleared user a weaker
/// mask than the one they are served today.
#[allow(clippy::too_many_arguments)]
async fn fold_clearance_masks(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    profile: &str,
    engine: EngineType,
    pending: Vec<crate::clearance_mask::PendingMask>,
    writable: &HashMap<String, i64>,
    desired: &mut BTreeMap<PolicyKey, DesiredPolicy>,
    skipped: &mut Vec<SkippedItem>,
) -> Result<(), CompileError> {
    let is_mask = |b: &Value| b.get("policyType").and_then(Value::as_i64) == Some(1);
    let mut others: Vec<Value> = desired
        .iter()
        .filter(|((sid, _), dp)| {
            *sid == service_id && dp.source_kind != SourceKind::Clearance && is_mask(&dp.body)
        })
        .map(|(_, dp)| dp.body.clone())
        .collect();
    others.extend(
        egret_policy::enabled_policies_for_service_via(conn, service_id)
            .await
            .map_err(db_err)?
            .into_iter()
            .filter(|b| {
                is_mask(b)
                    && !b
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(egret_policy::is_compiler_owned_policy_name)
            }),
    );

    use crate::clearance_mask::{FoldVersions, Folded};
    let merged = crate::clearance_mask::merge_by_column(pending);
    let mut skip = |tag_id: Uuid, reason: String| {
        tracing::warn!(service_id, tag_id = %tag_id, reason = %reason,
            "clearance mask skipped during recompile; the per-column deny is still compiled");
        skipped.push(SkippedItem {
            source_kind: SourceKind::Clearance,
            id: tag_id,
            reason,
        });
    };
    let plugins =
        match crate::clearance_mask::fold_plugin_versions(conn, service_id, profile, engine).await?
        {
            FoldVersions::Modelled(v) => v,
            // An engine-level reason: no mask for ANY column of this profile,
            // one skip per column so each classified column says so.
            FoldVersions::Unmodelled(reason) => {
                for p in &merged {
                    skip(p.tag_id, reason.clone());
                }
                return Ok(());
            }
        };
    for p in merged {
        let name = crate::clearance_mask::clearance_mask_name(p.tag_id, &p.column);
        let body = match crate::clearance_mask::fold_at_versions(
            profile, &name, &p.column, &p.cleared, &others, engine, &plugins,
        )? {
            Folded::Mask(body) => body,
            Folded::Skipped(reason) => {
                skip(p.tag_id, reason);
                continue;
            }
        };
        let target = target_service_id(&body, writable, &format!("clearance mask {name}"))?;
        desired.insert(
            (target, name),
            DesiredPolicy {
                body,
                source_kind: SourceKind::Clearance,
                grant_id: None,
                rule_id: None,
            },
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn compile_one_rule(
    r: &Rule,
    engine: EngineType,
    service_name: &str,
    tag_service: Option<&str>,
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    catalog_names: &HashMap<Uuid, String>,
    writable: &HashMap<String, i64>,
    desired: &mut BTreeMap<PolicyKey, DesiredPolicy>,
    skipped: &mut Vec<SkippedItem>,
) -> Result<(), CompileError> {
    let (catalog_name, tag_name, tagged): (String, Option<String>, Vec<TableTarget>) =
        match &r.selector {
            Selector::Path(scope) => {
                let Some(name) = catalog_names.get(&scope.catalog_id) else {
                    return Ok(()); // not reachable by this service — out of scope
                };
                (name.clone(), None, Vec::new())
            }
            Selector::Tag { key } => {
                let Some(name) = tag_name_for_key_via(conn, key).await.map_err(db_err)? else {
                    // The rule names a tag that no longer exists. Not a
                    // compiler defect and not the WorkspaceMembers case
                    // either — record and move on rather than aborting the
                    // whole service's recompile over one dangling rule.
                    let reason = format!("tag key {key:?} has no matching row in `tags`");
                    tracing::warn!(
                        service_id,
                        rule_id = %r.id,
                        reason = %reason,
                        "rule skipped during recompile"
                    );
                    skipped.push(SkippedItem {
                        source_kind: SourceKind::Rule,
                        id: r.id,
                        reason,
                    });
                    return Ok(());
                };
                let tagged = if engine == EngineType::StarRocks {
                    crate::rules::group_by_table(
                        tag_assignments_for_service_via(conn, service_id, key)
                            .await
                            .map_err(db_err)?,
                    )
                } else {
                    Vec::new()
                };
                (String::new(), Some(name), tagged)
            }
        };

    let resolved = ResolvedNames {
        catalog_name: &catalog_name,
        tag_name: tag_name.as_deref(),
    };

    let bodies = compile_rule(
        r,
        engine,
        service_name,
        tag_service,
        &resolved,
        &tagged,
        DEFAULT_EXPANSION_CEILING,
    );
    match bodies {
        Ok(bodies) => {
            for body in bodies {
                let what = format!("rule {:?}", r.name);
                let name = body_name(&body, &what)?;
                // A TAG body names the tag service here, a path body the
                // profile — the routing that Task 10 found missing.
                let target = target_service_id(&body, writable, &what)?;
                desired.insert(
                    (target, name),
                    DesiredPolicy {
                        body,
                        source_kind: SourceKind::Rule,
                        grant_id: None,
                        rule_id: Some(r.id),
                    },
                );
            }
            Ok(())
        }
        Err(CompileError::UnexpandedPrincipalKind { kind }) => {
            let reason = format!(
                "principal kind {kind:?} reached the compiler unexpanded — a bug in \
                 `crate::members::expand_enumerated_principals`, not in the rule"
            );
            tracing::warn!(
                service_id,
                rule_id = %r.id,
                reason = %reason,
                "rule skipped during recompile"
            );
            skipped.push(SkippedItem {
                source_kind: SourceKind::Rule,
                id: r.id,
                reason,
            });
            Ok(())
        }
        // A Trino profile with no `tagService` in its config cannot host a
        // tag policy at all — the Ranger tag plugin it would be resolved by
        // is not wired to this profile. That is a property of the PROFILE,
        // not a defect in the rule, so it is recorded as a skip (and warned
        // about) exactly like the dangling-tag-key case above rather than
        // aborting the service's recompile.
        //
        // **The discriminator for skip-vs-refuse, stated once here because it
        // is the question every future variant will have to answer: would the
        // skipped item have been ENFORCED had it compiled?**
        //
        // - A deny grant: yes. Skipping it silently drops an enforced denial,
        //   which fails OPEN, so the compiler refuses instead.
        // - A tag rule on a profile with no `tagService`: no. Ranger's
        //   `download_tags` (`egret-policy/src/ranger/api/download.rs`) reads
        //   the tag service strictly from `config_jsonb`, with no fallback, so
        //   this profile serves no tag policies to anyone whatever we do here.
        //   Aborting would have left it unmasked AND stopped every OTHER
        //   profile from getting the mask. Skipping is strictly more
        //   protective, not merely more convenient.
        //
        // **Why this matters as of SP4a Task 9 and not before.** Task 7's
        // only caller recompiled the handful of profiles bound to ONE
        // warehouse. Task 9's rule-mutation path recompiles every
        // compiler-targeted profile in the installation, because a Trino tag
        // rule is catalog-unscoped (see `affected_services_for_rule`). Under
        // an abort, one misconfigured Trino profile anywhere would make
        // every tag-rule create and delete fail with a 500 — a profile
        // nobody involved in the write has heard of blocking the write. Each
        // skip still reaches the caller in `CompileSummary::skipped` and the
        // logs, so it is visible rather than silent.
        Err(CompileError::MissingTagService { rule }) => {
            let reason = format!(
                "profile has no usable `tagService` (unconfigured, or naming a profile that \
                 does not exist), so tag rule {rule:?} cannot compile here"
            );
            tracing::warn!(
                service_id,
                rule_id = %r.id,
                reason = %reason,
                "rule skipped during recompile"
            );
            skipped.push(SkippedItem {
                source_kind: SourceKind::Rule,
                id: r.id,
                reason,
            });
            Ok(())
        }
        Err(other) => Err(other),
    }
}

#[derive(sqlx::FromRow)]
struct ExistingPolicyRow {
    id: i64,
    service_id: i64,
    name: String,
    policy_jsonb: Value,
}

/// Diff `desired` against the CURRENT `policy.authz_policies` state of every
/// service in `sweep` and write only the difference, on `tx` (never opens or
/// commits its own — the caller owns both). Bumps
/// `policy.authz_services.policy_version` at most once per service, and only
/// for a service something was actually written under.
///
/// **Fix round 1 Critical**: "current state" is read directly from
/// `policy.authz_policies` (scoped by service and this compiler's own name
/// prefixes), NOT through `perm.compiled` — see the module doc comment for
/// why a join through `perm.compiled` is blind to a policy whose grant or
/// rule was deleted (its `perm.compiled` row cascades away with it, but the
/// policy itself does not).
///
/// **SP4a Task 11, defect 2 — why the sweep is over a SET of services.** A
/// Trino tag policy belongs to the profile's TAG service, not the profile
/// (`target_service_id`). If the existing-state read stayed scoped to the
/// profile alone, a deleted tag rule's policy would sit in the tag service
/// where nothing enumerated it and nothing could ever reap it — the exact
/// shape of the Critical the round-1 fix above closed, re-created one service
/// over. `sweep` therefore comes from the profile's CONFIGURATION
/// (`Compiled::sweep`), so the tag service is enumerated on every recompile,
/// including the one where the rule that used to compile into it has just
/// been deleted and `desired` holds nothing for it at all.
///
/// `profile_service_id` is still distinguished from the rest of `sweep`: it is
/// the one this recompile stamps `origin = 'generated'` on (D4 — a tag service
/// is not an editable profile and no engine binds one), and the one whose
/// `policy_version` must bump whenever ANY swept service changed, because the
/// resource plugin's own download embeds the tag service's policies as
/// `tagPolicies` and is gated by the PROFILE's version
/// (`egret-policy/src/ranger/api/download.rs`): a tag-only change that left
/// the profile's version alone would answer that plugin `304 Not Modified`
/// and the new mask would never reach it.
async fn apply_diff_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    profile_service_id: i64,
    profile_name: &str,
    largest: &[egret_perm::TableCount],
    sweep: &[i64],
    desired: BTreeMap<PolicyKey, DesiredPolicy>,
    skipped: Vec<SkippedItem>,
) -> Result<CompileSummary, CompileError> {
    // **Serialize concurrent recompiles of the SAME profile** (SP4a Task 9).
    // Everything below reads `policy.authz_policies` for these services,
    // decides a diff from what it read, and then writes against the policy ids
    // it read. Under Postgres' default READ COMMITTED that plan is only valid
    // if no other transaction can commit a change to those policies in
    // between — and one can: a concurrently deleted policy row makes the
    // `Some((policy_id, _))` branch below UPDATE zero rows and then insert a
    // `perm.compiled` row pointing at an id that no longer exists, which the
    // `compiled_policy_id_fkey` foreign key rejects. That was not reachable
    // while the compiler's only caller was the tag-write path (one warehouse's
    // services, one writer); Task 9's model-mutation routes make two requests
    // recompiling one profile ordinary, and an installation-wide tag-rule
    // recompile makes it likely.
    //
    // A row lock on each service is the smallest thing that makes the
    // read-decide-write sequence atomic. **Callers must acquire services in
    // ASCENDING id order** — `recompile_services_in_tx`,
    // `services_for_catalog_via`, `compiler_targeted_services_via` and
    // `egret_catalog_registry::services_bound_to_warehouse` all order that way,
    // and `Compiled::sweep` is sorted — so two transactions recompiling
    // overlapping sets queue rather than deadlock.
    debug_assert!(
        sweep.is_sorted(),
        "apply_diff_in_tx requires an ascending sweep set, got {sweep:?}"
    );
    for &sid in sweep {
        sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE")
            .bind(sid)
            .execute(&mut **tx)
            .await
            .map_err(db_err)?;
    }

    let existing: Vec<ExistingPolicyRow> = sqlx::query_as(
        "SELECT id, service_id, name, policy_jsonb FROM policy.authz_policies \
         WHERE service_id = ANY($1) \
           AND (name LIKE $2 || '%' OR name LIKE $3 || '%' OR name LIKE $4 || '%' \
                OR name LIKE $5 || '%')",
    )
    .bind(sweep)
    .bind(COMPILER_GRANT_PREFIX)
    .bind(COMPILER_RULE_PREFIX)
    // SP4c Task 2: a clearance deny is reaped by this same sweep when its
    // gate is removed — the prefix has to be here or a lifted clearance
    // would be enforced forever (SP4a Task 7 fix round 1's Critical, one
    // source over).
    .bind(COMPILER_CLEARANCE_PREFIX)
    // F11: an engine grant's policy is reaped when the grant (or its engine,
    // or its target user) is deleted, for the same reason.
    .bind(COMPILER_ENGINE_GRANT_PREFIX)
    .fetch_all(&mut **tx)
    .await
    .map_err(db_err)?;

    // **The profile-total ceiling (SP4a Task 12).** "Currently stored" is
    // read from THIS SAME `existing` fetch — no extra query, no extra lock —
    // scoped to the profile's own service id (not the whole `sweep`: a tag
    // service's row count is a different, O(1)-per-rule world and is not
    // what the measurement bounds). Reading it here cannot deadlock: every
    // lock this recompile will take was already acquired above (and, for a
    // multi-service model write, by `lock_services_in_tx` before this
    // transaction touched any `perm.*` row), in the ascending order that
    // invariant depends on — this is a plain `SELECT` against rows this same
    // transaction already holds `FOR UPDATE`, not a new acquisition.
    //
    // **Refuse only a change that pushes the total OVER the ceiling.** A
    // profile already at or above `PROFILE_EXPANSION_CEILING` — reachable
    // only because this ceiling did not exist before Task 12 — must still be
    // able to recompile when the change shrinks it or leaves it unchanged;
    // see `PROFILE_EXPANSION_CEILING`'s own doc comment for why a naive
    // "over the ceiling ⇒ refuse" bricks exactly that profile (an operator
    // cannot delete their way back under the cap if every delete's
    // accompanying recompile refuses and rolls the delete back with it).
    let current_stored_total = existing
        .iter()
        .filter(|row| row.service_id == profile_service_id)
        .count();
    let new_total = desired
        .keys()
        .filter(|(service_id, _)| *service_id == profile_service_id)
        .count();
    if new_total > PROFILE_EXPANSION_CEILING && new_total > current_stored_total {
        return Err(CompileError::ProfileExpansionCeiling {
            profile: profile_name.to_string(),
            total: new_total,
            ceiling: PROFILE_EXPANSION_CEILING,
            largest: largest.to_vec(),
        });
    }

    let mut existing_by_key: HashMap<PolicyKey, (i64, Value)> = existing
        .into_iter()
        .map(|row| ((row.service_id, row.name), (row.id, row.policy_jsonb)))
        .collect();

    let mut summary = CompileSummary {
        skipped,
        ..Default::default()
    };
    // Which swept services this call actually wrote under — each one's
    // `policy_version` gates its own plugin's 304.
    let mut wrote_under: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();

    // SP4a Task 8 (D4): the moment this compiler intends to own at least one
    // policy under this service, the profile stops being editor-writable.
    // Stamped HERE — inside the actual compile, on the actual write path —
    // rather than by any HTTP handler, so every caller of `apply_diff_in_tx`
    // (today, only `recompile_service_in_tx`) gets the same protection with
    // nothing else to remember, and so `origin = 'generated'` is asserted
    // only by a compile that really happened, never by a hand-written
    // migration default. Sticky: a profile that currently compiles to ZERO
    // policies (every grant/rule that reached it was deleted) is left
    // `generated` as found — the compiler still claims it; only
    // `PolicyStore::create_service` (never called for an existing row) ever
    // starts a profile out as `manual`.
    //
    // **The TAG service this compile writes into is stamped too** (whole-branch
    // review, Important 5). Task 8 stamped the profile alone, on the reasoning
    // that a tag service is not an engine-bound, editor-facing profile and that
    // its compiler-owned rows are already protected by the reserved-prefix
    // guard. Task 11 then moved compiled tag policies INTO the tag service, and
    // that reasoning did not survive the move: the reserved-prefix guard
    // protects the compiler's OWN rows, and the hazard here is a different
    // policy entirely. Ranger evaluates tag policies FIRST and short-circuits,
    // so one hand-written tag ACCESS allow — which `create_policy` still
    // accepts into any `manual` service, and which the editor reaches through
    // `tag_editor_to_ranger_json` — ends evaluation before any compiled
    // resource policy, including every compiled DENY grant, is consulted. D2's
    // "egret never emits a tag access policy" was true of the compiler and not
    // of egret. Stamping the tag service makes `refuse_if_generated` cover it
    // exactly as it covers the profile: no NEW hand-written policy in a service
    // the compiler owns. It is scoped to services this compile actually writes
    // into (`desired`'s own service ids), so a tag service nothing compiles
    // into is left alone, and it does not affect `delete_service`, which is
    // keyed on remaining compiler-owned POLICIES rather than on `origin`.
    if !desired.is_empty() {
        let mut stamp: Vec<i64> = desired.keys().map(|(service_id, _)| *service_id).collect();
        stamp.push(profile_service_id);
        stamp.sort_unstable();
        stamp.dedup();
        sqlx::query(
            "UPDATE policy.authz_services SET origin = 'generated' \
             WHERE id = ANY($1) AND origin <> 'generated'",
        )
        .bind(&stamp)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    }

    for ((service_id, name), dp) in desired {
        debug_assert!(
            egret_policy::is_compiler_owned_policy_name(&name),
            "compiled policy name {name:?} does not carry this compiler's own prefix — \
             the existing-state sweep above would never find it again"
        );
        debug_assert!(
            sweep.contains(&service_id),
            "compiled policy {name:?} targets service {service_id}, which this recompile does \
             not sweep — it could never be reaped again"
        );
        match existing_by_key.remove(&(service_id, name.clone())) {
            None => {
                let policy_id: i64 = sqlx::query_scalar(
                    "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb, managed_by) \
                     VALUES ($1, $2, $3, 'compiler') RETURNING id",
                )
                .bind(service_id)
                .bind(&name)
                .bind(&dp.body)
                .fetch_one(&mut **tx)
                .await
                .map_err(db_err)?;
                record_compiled(tx, service_id, policy_id, &dp).await?;
                summary.created += 1;
                wrote_under.insert(service_id);
            }
            Some((policy_id, current_body)) => {
                if current_body == dp.body {
                    summary.unchanged += 1;
                } else {
                    sqlx::query(
                        "UPDATE policy.authz_policies \
                         SET policy_jsonb = $2, version = version + 1, updated_at = now() \
                         WHERE id = $1",
                    )
                    .bind(policy_id)
                    .bind(&dp.body)
                    .execute(&mut **tx)
                    .await
                    .map_err(db_err)?;
                    summary.updated += 1;
                    wrote_under.insert(service_id);
                }
                // Self-heal `perm.compiled` provenance whether the body
                // changed or not — a row that survived to this branch is, by
                // definition, still produced by a live grant/rule, so its
                // provenance row should exist regardless of what an earlier
                // recompile (pre fix round 1) may have left behind.
                record_compiled(tx, service_id, policy_id, &dp).await?;
            }
        }
    }

    // Everything left in `existing_by_key` is a policy this recompile no
    // longer produces (the grant/rule behind it was deleted, its catalog is
    // no longer reachable, or a StarRocks column lost its tag) — reaped here
    // regardless of whether a `perm.compiled` row for it still exists, and
    // regardless of WHICH swept service it lives under, which is what keeps a
    // deleted tag rule's policy from outliving the rule in the tag service.
    for ((service_id, _name), (policy_id, _body)) in existing_by_key {
        sqlx::query("DELETE FROM policy.authz_policies WHERE id = $1")
            .bind(policy_id)
            .execute(&mut **tx)
            .await
            .map_err(db_err)?;
        summary.deleted += 1;
        wrote_under.insert(service_id);
    }

    if summary.wrote_anything() {
        // The profile's own version always bumps when anything changed: its
        // download embeds the tag service's policies and is gated by THIS
        // version (see this function's doc comment). Exactly once: since
        // migration 0182, bumping a `tag` service already bumps every profile
        // that links it — this one included — so the profile is bumped here
        // only if nothing else moved it in this transaction.
        let version_of = |sid: i64| {
            sqlx::query_scalar::<_, i64>(
                "SELECT policy_version FROM policy.authz_services WHERE id = $1",
            )
            .bind(sid)
        };
        let before: i64 = version_of(profile_service_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(db_err)?;
        for sid in wrote_under
            .iter()
            .copied()
            .filter(|sid| *sid != profile_service_id)
        {
            bump_policy_version(tx, sid).await?;
        }
        let after: i64 = version_of(profile_service_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(db_err)?;
        if after == before {
            bump_policy_version(tx, profile_service_id).await?;
        }
    }

    Ok(summary)
}

async fn bump_policy_version(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<(), CompileError> {
    sqlx::query(
        "UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() \
         WHERE id = $1",
    )
    .bind(service_id)
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Idempotent provenance write: `perm.compiled` names which grant/rule
/// produced `policy_id` under `service_id`. `ON CONFLICT DO NOTHING` on the
/// table's own `(service_id, policy_id)` primary key makes this safe to call
/// on every create AND every still-produced update/keep, not only the first
/// insert — self-healing a provenance row an earlier recompile (pre fix
/// round 1) may have left cascaded away without also removing the policy it
/// described.
async fn record_compiled(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
    policy_id: i64,
    dp: &DesiredPolicy,
) -> Result<(), CompileError> {
    // **A CLEARANCE records no provenance row, deliberately** (SP4c Task 2).
    // `perm.compiled` is grant/rule-shaped in the schema itself — `source_kind`
    // is `CHECK (source_kind IN ('grant','rule'))` and each of the two ids is
    // CHECKed against it (`migrations/0154_perm_model.sql`) — and a clearance
    // comes from `classification_policies`, so it has neither id to record.
    // Widening that schema would buy nothing here: since Task 7's fix round 1
    // the sweep's source of truth is `policy.authz_policies` scoped by this
    // compiler's own name prefixes, never `perm.compiled`, and a clearance
    // policy's NAME already carries the `tags.id` it came from
    // (`COMPILER_CLEARANCE_PREFIX{tag_id}`), so the provenance question is
    // answered by the row that decides reaping rather than by a second table
    // that can cascade away beneath it.
    //
    // **An ENGINE GRANT records none either** (F11, spec C10), for the same
    // reason: it is neither a grant nor a rule, and its policy name
    // (`perm-engine-{id}`) already carries its id.
    if matches!(
        dp.source_kind,
        SourceKind::Clearance | SourceKind::EngineGrant
    ) {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO perm.compiled (service_id, policy_id, source_kind, grant_id, rule_id) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (service_id, policy_id) DO NOTHING",
    )
    .bind(service_id)
    .bind(policy_id)
    .bind(dp.source_kind.as_str())
    .bind(dp.grant_id)
    .bind(dp.rule_id)
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// Recompile `service_id`'s profile on the CALLER's own open transaction —
/// every read and every write goes through `tx`. Use this when the recompile
/// must be atomic with another write already in progress on the same
/// transaction (`egret-bi::catalog_tags`'s tag write is exactly that case —
/// same shape as `egret_perm::sync_assignment_grants_in_tx`). Does not
/// commit; the caller does, same as that function.
///
/// **Locks, then pre-mints, then compiles — the order every recompile path
/// takes.** A compile can mint usernames (group, workspace and, since the
/// 2026-10-18 design R5, role expansion, plus the clearance compile), and each
/// of those mints is ascending only WITHIN its own call; the calls together
/// are not. Run alone that is harmless, but against a concurrent
/// [`recompile_services_in_tx`], whose pre-mint sweep takes the same `users`
/// row locks in one global ascending order, the two could each hold a row the
/// other wanted, and Postgres killed one (observed: one full-suite run in
/// seven, `tests/role_grants.rs`). So this entry takes the profile locks first
/// (J-F3 order: profiles, then rows; re-taking a lock a caller already holds
/// is free) and then runs the same ascending
/// [`crate::members::premint_model_usernames`] sweep, after which the
/// per-pass expansions find every name already minted and take no `users`
/// lock at all. A caller that loops over several profiles must lock them all
/// first ([`lock_services_in_tx`]), as every caller does today; the repeated
/// sweep then finds nothing to mint. The sweep reads statement by statement,
/// so a person a concurrent commit names after it is still minted by a pass,
/// out of order; every compile mint therefore also takes one transaction-scoped
/// lock ([`crate::members::lock_compile_mints_in_tx`], which states the order:
/// profiles, then that lock, then any row write or mint), which is what rules
/// the cycle out — the sweep keeps it to one acquisition in the common case.
pub async fn recompile_service_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<CompileSummary, CompileError> {
    lock_services_in_tx(tx, &[service_id]).await?;
    crate::members::premint_model_usernames(tx).await?;
    recompile_locked_service_in_tx(tx, service_id).await
}

/// [`recompile_service_in_tx`] without its lock and pre-mint, for a caller
/// that has already taken both over a whole set ([`recompile_services_in_tx`]).
///
/// **The `egret-admin` chokepoint (username-takeover spec 2026-10-23 §4.2,
/// D2).** Every recompile path ends here, so the profile's `egret-admin - …`
/// policies are converged to the current engine-admin set first: before the
/// early return (a profile the compiler does not target still carries them)
/// and before the compile (the impersonation near-root check reads the fresh
/// body). A path that changes who is an admin therefore only has to put the
/// admin-policy services in the set it locks ([`admin_policy_services_in`]).
async fn recompile_locked_service_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<CompileSummary, CompileError> {
    sync_admin_policy_in_tx(tx, service_id).await?;
    let Some(compiled) = compile_desired(tx, service_id).await? else {
        return Ok(CompileSummary::default());
    };
    let summary = apply_diff_in_tx(
        tx,
        service_id,
        &compiled.profile_name,
        &compiled.largest,
        &compiled.sweep,
        compiled.desired,
        compiled.skipped,
    )
    .await?;
    // DG #3: the profile's last compile, on this compile's transaction — every
    // compile path reaches this function, and a caller that rolls back takes
    // the row with it (nothing changed on the engine).
    crate::compile_runs::record_compile_ok_in_tx(tx, service_id, &summary).await?;
    Ok(summary)
}

/// Rewrite `service_id`'s `egret-admin - …` policies to name exactly the
/// engine admins (`egret_auth::roles::engine_admin_user_ids_via`), when the
/// service holds any. The caller holds the profile lock; the usernames were
/// minted by `premint_model_usernames`, and one still missing is minted here
/// under the same mint lock. Writes nothing when the body is current.
pub(crate) async fn sync_admin_policy_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<(), CompileError> {
    if !egret_policy::default_policies::admin_policy_services_via(tx)
        .await
        .map_err(db_err)?
        .contains(&service_id)
    {
        return Ok(());
    }
    let users = crate::members::engine_admin_usernames_via(tx).await?;
    let sync = egret_policy::default_policies::sync_admin_all_access_in_tx(tx, service_id, &users)
        .await
        .map_err(db_err)?;
    if sync.changed {
        tracing::info!(
            service_id,
            removed = ?sync.removed,
            added = ?sync.added,
            "egret-admin now names exactly the engine admins"
        );
    }
    Ok(())
}

/// The services holding `egret-admin` policies, appended to an affected set
/// whose change can alter who is an engine admin.
async fn admin_policy_services_in(
    conn: &mut sqlx::PgConnection,
    ids: &mut Vec<i64>,
) -> Result<(), CompileError> {
    ids.extend(
        egret_policy::default_policies::admin_policy_services_via(conn)
            .await
            .map_err(db_err)?,
    );
    Ok(())
}

/// Recompile `service_id`'s profile in its own, self-contained transaction —
/// the brief-literal entry point (`store`/`policy` name the pool this opens
/// its transaction against; both must reach the SAME physical Postgres
/// database `perm.*` and `policy.*` share). Use [`recompile_service_in_tx`]
/// instead when the recompile must be atomic with another write already in
/// progress.
pub async fn recompile_service(
    store: &PermStore,
    _policy: &PolicyStore,
    service_id: i64,
) -> Result<CompileSummary, CompileError> {
    let result = async {
        let mut tx = store.pool().begin().await.map_err(db_err)?;
        let summary = recompile_service_in_tx(&mut tx, service_id).await?;
        tx.commit().await.map_err(db_err)?;
        Ok(summary)
    }
    .await;
    // DG #3: no write waits on this entry point, so nothing else would report
    // that the profile is still serving its previous compile. The transaction
    // is gone by now (dropped, so rolled back); the failure gets its own.
    if let Err(e) = &result {
        record_failure_logged(store.pool(), service_id, e).await;
    }
    result
}

/// [`crate::compile_runs::record_compile_failure`], logging (never returning)
/// its own failure: the caller's answer is the compile's error.
pub(crate) async fn record_failure_logged(
    pool: &sqlx::PgPool,
    service_id: i64,
    error: &CompileError,
) {
    if let Err(w) =
        crate::compile_runs::record_compile_failure(pool, service_id, &error.to_string()).await
    {
        tracing::warn!(
            service_id,
            error = %w,
            "could not record a failed compile in perm.compile_runs"
        );
    }
}

// ---------------------------------------------------------------------------
// SP4a Task 9: which services does a MODEL mutation affect?
// ---------------------------------------------------------------------------
//
// Task 7 answered this for a TAG write ("every service bound to the warehouse
// that was tagged"). A grant or rule mutation is a different question, and the
// answer is not the same for the two halves of the model:
//
// - A **grant** names exactly one catalog (`perm.grants.catalog_id`, NOT NULL).
//   `compile_desired` skips any grant whose catalog the service cannot reach
//   (`catalog_names.get(&g.scope.catalog_id)` → `continue`), so a grant can
//   only ever change the compiled output of a profile bound to THAT catalog.
//   The affected set is the reverse of `catalogs_for_service_via`.
//
// - A **rule** depends on its selector. A path-selector rule is catalog-scoped
//   in exactly the same way a grant is. A TAG-selector rule is not: on Trino it
//   compiles to one tag policy addressed to the profile's tag service, with no
//   catalog resource at all (D5, `rules::build_tag_body`), so it lands in EVERY
//   Trino profile regardless of which catalogs that profile reaches. On
//   StarRocks it compiles per tagged column, which is catalog-scoped in effect
//   — but "which StarRocks profiles have a column carrying this tag" is a
//   question whose answer changes with every tag write, and getting it wrong
//   leaves a mask off a column. The honest set for a tag rule is therefore
//   every profile this compiler targets.
//
// **Stated plainly, because it is a real operational property and not a
// footnote:** creating or deleting ONE tag-selected rule recompiles EVERY
// Trino and StarRocks profile in the installation that an engine binds. On an
// installation with many engines that is the cost of one such request, and a
// failure on any one of those profiles fails the whole request and rolls the
// rule write back. Grant mutations and path-rule mutations are narrow; tag-rule
// mutations are installation-wide.

/// The services a create/delete of `g` can change the compiled output of —
/// the profiles bound to the grant's own catalog. See this module's Task 9
/// section for the derivation.
pub async fn affected_services_for_grant(
    conn: &mut sqlx::PgConnection,
    g: &Grant,
) -> Result<Vec<i64>, CompileError> {
    let mut ids = services_for_catalog_via(conn, g.scope.catalog_id)
        .await
        .map_err(db_err)?;
    normalize_service_order(&mut ids);
    Ok(ids)
}

/// Ascending, deduplicated — the lock order `apply_diff_in_tx`'s `FOR UPDATE`
/// depends on. The queries already `ORDER BY`, but the ordering is what keeps
/// two overlapping recompiles from deadlocking, so it is asserted here rather
/// than trusted to every query that feeds this module.
fn normalize_service_order(ids: &mut Vec<i64>) {
    ids.sort_unstable();
    ids.dedup();
}

/// The UNION of two affected-service sets, ascending and deduplicated — what a
/// PATCH must recompile (SP4b D1).
///
/// **Why a patch needs the union and not just the new set.** A patch can move
/// a grant from one catalog to another, or a rule from a path selector to a
/// tag selector. Recompiling only the NEW affected set leaves the compiled
/// policy sitting in the profile the row used to reach, still being served to
/// that engine, with nothing in the model producing it any more and nothing
/// that will ever sweep it again — which is exactly SP4a's "a deleted grant's
/// policy is enforced forever" Critical, reintroduced through the edit door.
/// The old set is therefore read from the row as it stands BEFORE the write
/// and merged in here, so one recompile pass over the union reaps the old
/// profile in the same transaction that creates the policy in the new one.
pub fn merge_service_ids(a: &[i64], b: &[i64]) -> Vec<i64> {
    let mut ids = Vec::with_capacity(a.len() + b.len());
    ids.extend_from_slice(a);
    ids.extend_from_slice(b);
    normalize_service_order(&mut ids);
    ids
}

/// The services a create/delete of `r` can change the compiled output of.
/// A path selector is catalog-scoped like a grant; a **tag selector reaches
/// every compiler-targeted profile in the installation** — see this module's
/// Task 9 section.
pub async fn affected_services_for_rule(
    conn: &mut sqlx::PgConnection,
    r: &Rule,
) -> Result<Vec<i64>, CompileError> {
    let mut ids = match &r.selector {
        Selector::Path(scope) => services_for_catalog_via(conn, scope.catalog_id)
            .await
            .map_err(db_err)?,
        Selector::Tag { .. } => compiler_targeted_services_via(conn).await.map_err(db_err)?,
    };
    normalize_service_order(&mut ids);
    Ok(ids)
}

/// The services a RENAME of tag `tag_key` can change the compiled output of
/// (whole-branch review, Important 3 — D6's "a rename recompiles instead of
/// breaking").
///
/// **Why a rename changes compiled output at all.** D6 resolves `tags.key` →
/// `tags.name` at compile time, because the Ranger tag plugin matches on the
/// NAME. A Trino tag policy therefore carries the name as it stood when it was
/// compiled, while `download_tags` serves `t.name` live. Rename the tag and
/// the two agree only until the next plugin restart or `lastKnownVersion=-1`
/// poll, at which point the tag→resource map carries the NEW name and the
/// policy still names the OLD one: **the mask stops applying and the column
/// returns real values**, triggered by an unrelated restart. That is the exact
/// silent breakage D6 says this model exists to remove.
///
/// The set is the one a tag-RULE mutation uses, for the same reason (see this
/// module's Task 9 section): a tag is installation-wide, so any profile this
/// compiler targets may carry a policy naming it. It is empty when nothing live
/// compiles from that name at all — then a rename is a pure display change.
///
/// **SP4c Task 3 widened the "nothing compiles from it" test, and that was a
/// real fail-OPEN gap, not a tidy-up.** Until SP4c a tag's name reached a
/// compiled policy only through a `perm.rules` tag selector, so this function
/// early-returned on `perm.rules` alone. A CLEARANCE also stamps `tags.name`
/// into a Trino tag policy's `tag` resource ([`crate::clearance`]) — and a gated
/// tag needs no rule. So renaming a gated-but-unruled tag returned an EMPTY
/// affected set, the clearance deny kept naming the OLD tag, and at the next tag
/// map refresh the deny matched nothing: the classified column started answering
/// queries, triggered by nothing more than a display rename. Found by
/// re-deriving this list for Task 3 rather than inheriting it, which is the
/// method SP4a's fourth missing recompile taught.
pub async fn affected_services_for_tag_rename(
    conn: &mut sqlx::PgConnection,
    tag_key: &str,
) -> Result<Vec<i64>, CompileError> {
    let selected: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM perm.rules WHERE selector_kind = 'tag' AND tag_key = $1)               OR EXISTS (SELECT 1 FROM classification_policies cp JOIN tags t ON t.id = cp.tag_id                           WHERE t.key = $1 AND cardinality(cp.required_roles) > 0)",
    )
    .bind(tag_key)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    if !selected {
        return Ok(Vec::new());
    }
    let mut ids = compiler_targeted_services_via(conn).await.map_err(db_err)?;
    normalize_service_order(&mut ids);
    Ok(ids)
}

/// The services a change to a TAG's CLEARANCE (`classification_policies`) can
/// change the compiled output of — SP4c Task 3, input 1 of three.
///
/// **Installation-wide, on purpose, and not because it was easier.** A Trino
/// clearance is one tag policy carrying no catalog resource at all, so it
/// compiles identically into every Trino profile that has a tag service; and a
/// StarRocks clearance expands into every StarRocks profile from which any
/// column carrying the tag is reachable. Deriving the narrow StarRocks set would
/// mean walking `catalog_tag_assignments → catalogs → attachments → engines` for
/// the tag here, duplicating a resolution `compile_clearance` already does
/// per-service — and getting it wrong in the narrow direction is the failure
/// this function exists to prevent, while getting it wrong WIDE costs one
/// recompile that writes nothing (`apply_diff_in_tx` diffs each body
/// byte-for-byte, so a profile whose output did not change is `unchanged`).
///
/// Unlike [`affected_services_for_tag_rename`] there is no "nothing references
/// it" early return, because the write being accompanied IS the reference: after
/// a `set_policy` the gate exists, and after a `clear_policy` the compiled deny
/// must be reaped from wherever it was filed.
pub async fn affected_services_for_classification(
    conn: &mut sqlx::PgConnection,
) -> Result<Vec<i64>, CompileError> {
    let mut ids = compiler_targeted_services_via(conn).await.map_err(db_err)?;
    normalize_service_order(&mut ids);
    Ok(ids)
}

/// The services a change to who holds ROLE `role_name` can change the compiled
/// output of — SP4c Task 3, input 3 of three.
///
/// **ONE trigger, TWO consumers, deliberately not two triggers.** A role name is
/// unresolvable in every engine egret provisions (D0 measured it on Trino: the
/// plugin polls a `/service/roles/download/<svc>` route egret does not serve, so
/// `lastKnownRoleVersion` never leaves `-1`, `userRoles` is a hardcoded `null`
/// and `disableRoleResolution` defaults true; D0b re-derived it for StarRocks,
/// where `setUserRoles` is never called anywhere in `fe/`). So egret enumerates
/// the holders at compile time in **two** places — a clearance's
/// `denyExceptions` ([`crate::clearance`], SP4c Task 2/2d) and a `role`
/// principal in a grant ([`crate::members`], SP4c Task 2b) — and both read the
/// same `role_usernames`, which mints a holder's missing username (2026-10-18
/// design R5), so a holder is named from the compile their grant triggers, not
/// from a later one. Two triggers over one enumeration is two chances for
/// one of them to be forgotten, which is the shape of the defect SP4a shipped
/// four times; so this returns the UNION and each consumer contributes its own
/// arm below.
///
/// - **Clearance arm:** installation-wide when any `classification_policies` row
///   names this role, for the reason
///   [`affected_services_for_classification`] gives.
/// - **Grant arm:** catalog-scoped, exactly like [`affected_services_for_grant`]
///   — a `role` principal in `perm.grant_principals` only reaches the profiles
///   bound to that grant's catalog.
/// - **Engine-grant arm** (F11): the profile of the grant's engine.
///
/// **There is deliberately no rule-EXEMPTION arm**, and that is not the
/// defensive omission it looks like. `validate_rule_shape` refuses a `Role` in
/// an exemption and `compile_rule` answers
/// [`CompileError::UnexpressibleExemption`] for one that got in anyway — so such
/// a row has never compiled to anything that could be stale, and adding the arm
/// would make every membership write on that role FAIL the compile it triggered,
/// turning an unreachable row into an outage on an unrelated path.
///
/// Empty when nothing in the installation names the role, which is the common
/// case: then a membership write compiles nothing and costs two queries.
pub async fn affected_services_for_role_membership(
    conn: &mut sqlx::PgConnection,
    role_name: &str,
) -> Result<Vec<i64>, CompileError> {
    // Clearance: does any gated tag name this role? `required_roles` is a
    // `text[]`, so this is a containment test, not a join.
    let clears: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM classification_policies WHERE $1 = ANY(required_roles))",
    )
    .bind(role_name)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;

    let mut ids: Vec<i64> = Vec::new();
    if clears {
        ids.extend(compiler_targeted_services_via(conn).await.map_err(db_err)?);
    }

    // Grants: a `role` principal in either mode. `exclude` matters as much as
    // `include` — a role in an `exclude` compiles to `allowExceptions`, and a
    // stale exclusion keeps granting someone the model no longer grants.
    let catalogs: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT g.catalog_id FROM perm.grants g \
           JOIN perm.grant_principals p ON p.grant_id = g.id \
          WHERE p.kind = 'role'::perm.principal_kind AND p.ref = $1",
    )
    .bind(role_name)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    for catalog_id in catalogs {
        ids.extend(
            services_for_catalog_via(conn, catalog_id)
                .await
                .map_err(db_err)?,
        );
    }
    // F11: the profile of every engine whose engine grants name the role.
    ids.extend(engine_grant_profiles_naming_via(conn, "role", &[role_name]).await?);
    // F11: a platform admin is near-root as an impersonation target (egret
    // seeds them engine admin), so a change of who holds the role re-checks
    // every impersonation grant's target.
    //
    // Username takeover (spec 2026-10-23 §4.2): and every engine egret seeded
    // an `egret-admin` set on, which names exactly the role's active holders.
    if role_name == crate::engine_grants::PLATFORM_ADMIN_ROLE {
        ids.extend(impersonation_grant_profiles_via(conn).await?);
        admin_policy_services_in(conn, &mut ids).await?;
    }

    normalize_service_order(&mut ids);
    Ok(ids)
}

/// The services a WORKSPACE-MEMBERSHIP change in `workspace_id` can change the
/// compiled output of (SP4a D3).
///
/// **Why a membership write changes compiled output at all.** D7 measured that
/// a Ranger GROUP item matches nobody on any engine egret provisions, so a
/// `workspace_members` principal compiles to the member list enumerated at
/// compile time (`crate::members`). That enumeration is stale the moment
/// someone joins or leaves — the same shape D5's StarRocks tag expansion has,
/// and the same answer: egret owns the compile, so egret recompiles on the
/// write. Leaving it stale is not symmetric: a missed JOIN denies someone who
/// should have access (annoying), a missed LEAVE keeps granting someone who
/// should not (a breach).
///
/// The set is derived from the MODEL, not from the workspace: every profile
/// bound to a catalog carrying a grant that references this workspace, plus —
/// if any rule's exemption references it — the tag-rule set, for the same
/// installation-wide reason [`affected_services_for_rule`] gives. It is empty
/// when nothing in the model references the workspace at all, which is the
/// common case for a workspace with no catalogs assigned: then a membership
/// write compiles nothing and costs one query.
pub async fn affected_services_for_workspace_members(
    conn: &mut sqlx::PgConnection,
    workspace_id: Uuid,
) -> Result<Vec<i64>, CompileError> {
    let ws_ref = workspace_id.to_string();

    // Grants: catalog-scoped, exactly like `affected_services_for_grant`.
    let catalogs: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT g.catalog_id FROM perm.grants g            JOIN perm.grant_principals p ON p.grant_id = g.id           WHERE p.kind = 'workspace_members'::perm.principal_kind AND p.ref = $1",
    )
    .bind(&ws_ref)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;

    // Rules: a PATH selector is catalog-scoped; a TAG selector is not (see this
    // module's Task 9 section). `perm.rule_exemptions` cannot hold a
    // `workspace_members` row through egret's own API — `validate_rule_shape`
    // refuses that kind in an exemption — so this arm is normally empty; it is
    // here so a future writer of those rows cannot reintroduce the staleness
    // silently.
    let rule_catalogs: Vec<Option<Uuid>> = sqlx::query_scalar(
        "SELECT DISTINCT r.catalog_id FROM perm.rules r            JOIN perm.rule_exemptions e ON e.rule_id = r.id           WHERE e.kind = 'workspace_members'::perm.principal_kind AND e.ref = $1             AND r.selector_kind = 'path'",
    )
    .bind(&ws_ref)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let tag_rule_exempts: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM perm.rules r            JOIN perm.rule_exemptions e ON e.rule_id = r.id           WHERE e.kind = 'workspace_members'::perm.principal_kind AND e.ref = $1             AND r.selector_kind = 'tag')",
    )
    .bind(&ws_ref)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;

    let mut ids: Vec<i64> = Vec::new();
    if tag_rule_exempts {
        ids.extend(compiler_targeted_services_via(conn).await.map_err(db_err)?);
    }
    for catalog_id in catalogs
        .into_iter()
        .chain(rule_catalogs.into_iter().flatten())
    {
        ids.extend(
            services_for_catalog_via(conn, catalog_id)
                .await
                .map_err(db_err)?,
        );
    }
    normalize_service_order(&mut ids);
    Ok(ids)
}

/// The services a change to who belongs to any group in `group_names` can
/// change the compiled output of — J-F3 Task 1b (design R8).
///
/// **Why a membership write changes compiled output at all.** D7 measured that
/// a Ranger item naming a GROUP matches nobody on every engine egret
/// provisions, so a non-`public` `group` principal compiles to its members'
/// usernames, enumerated at compile time by [`crate::members`] — stale the
/// moment someone joins or leaves. Same argument, same asymmetry as
/// [`affected_services_for_role_membership`]: a missed leave keeps granting (or
/// keeps exempting, or keeps subtracting from a deny) someone the model no
/// longer names.
///
/// **ONE function, the UNION of every consumer of the group expansion.** There
/// are exactly two, both expanded by the one group arm of
/// `crate::members::expand_enumerated_principals`:
///
/// - **Grants**, include or exclude: catalog-scoped, exactly like
///   [`affected_services_for_grant`].
/// - **Rule exemptions**: a PATH selector is catalog-scoped; a TAG selector
///   reaches every compiler-targeted profile (this module's Task 9 section).
/// - **Engine grants** (F11): the profile of the grant's engine.
///
/// **There is deliberately no clearance arm.** A tag's `required_groups` is not
/// expanded — a group-only gate still compiles to nothing and reports the skip
/// (`crate::clearance`'s module doc) — so no clearance body names a group's
/// members, and nothing there can go stale.
///
/// `public` contributes nothing: it is never expanded (every engine resolves
/// it), so who "belongs" to a `groups` row that happens to carry that name
/// changes no compiled body. Empty when nothing in the model names any of the
/// groups, which is the common case for an OIDC-synced group: then a
/// membership write costs two queries and compiles nothing.
pub async fn affected_services_for_group_membership(
    conn: &mut sqlx::PgConnection,
    group_names: &[String],
) -> Result<Vec<i64>, CompileError> {
    let names: Vec<&str> = group_names
        .iter()
        .map(String::as_str)
        .filter(|n| *n != crate::rules::PUBLIC_GROUP)
        .collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }

    // Grants (include or exclude) and PATH-selected rule exemptions: the
    // catalog each names.
    let catalogs: Vec<Uuid> = sqlx::query_scalar(
        "SELECT g.catalog_id FROM perm.grants g \
           JOIN perm.grant_principals p ON p.grant_id = g.id \
          WHERE p.kind = 'group'::perm.principal_kind AND p.ref = ANY($1) \
         UNION \
         SELECT r.catalog_id FROM perm.rules r \
           JOIN perm.rule_exemptions e ON e.rule_id = r.id \
          WHERE e.kind = 'group'::perm.principal_kind AND e.ref = ANY($1) \
            AND r.selector_kind = 'path' AND r.catalog_id IS NOT NULL",
    )
    .bind(&names)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let tag_rule_exempts: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM perm.rules r \
           JOIN perm.rule_exemptions e ON e.rule_id = r.id \
          WHERE e.kind = 'group'::perm.principal_kind AND e.ref = ANY($1) \
            AND r.selector_kind = 'tag')",
    )
    .bind(&names)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;

    let mut ids: Vec<i64> = Vec::new();
    if tag_rule_exempts {
        ids.extend(compiler_targeted_services_via(conn).await.map_err(db_err)?);
    }
    for catalog_id in catalogs {
        ids.extend(
            services_for_catalog_via(conn, catalog_id)
                .await
                .map_err(db_err)?,
        );
    }
    // F11: the profile of every engine whose engine grants name the group.
    ids.extend(engine_grant_profiles_naming_via(conn, "group", &names).await?);
    // F11: a group holding `platform_admin` makes its members platform admins,
    // whom an impersonation grant may not target; joining or leaving it
    // re-checks every impersonation grant's target.
    let holds_platform_admin: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM access_grants ag \
           JOIN groups g ON g.id = ag.principal_id \
           JOIN roles r ON r.id = ag.role_id \
          WHERE ag.principal_type = 'group' AND ag.scope_type = 'platform' \
            AND r.name = $1 AND g.name = ANY($2))",
    )
    .bind(crate::engine_grants::PLATFORM_ADMIN_ROLE)
    .bind(&names)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    // Username takeover (spec 2026-10-23 §4.2): and every `egret-admin` set,
    // which names the group's members while it holds the role.
    if holds_platform_admin {
        ids.extend(impersonation_grant_profiles_via(conn).await?);
        admin_policy_services_in(conn, &mut ids).await?;
    }
    normalize_service_order(&mut ids);
    Ok(ids)
}

/// The profiles of every engine carrying an impersonation grant.
async fn impersonation_grant_profiles_via(
    conn: &mut sqlx::PgConnection,
) -> Result<Vec<i64>, CompileError> {
    sqlx::query_scalar(
        "SELECT DISTINCT e.authz_service_id FROM perm.engine_grants g \
           JOIN engine_mgmt.engines e ON e.id = g.engine_id \
          WHERE g.target_user_id IS NOT NULL AND e.authz_service_id IS NOT NULL",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

/// F11 (spec §4.5): the profiles of every engine whose engine grants name a
/// principal of `kind` in `refs` — the engine arm of the two membership
/// affected sets. An engine grant reaches exactly its engine's profile.
async fn engine_grant_profiles_naming_via(
    conn: &mut sqlx::PgConnection,
    kind: &str,
    refs: &[&str],
) -> Result<Vec<i64>, CompileError> {
    sqlx::query_scalar(
        "SELECT DISTINCT e.authz_service_id FROM perm.engine_grant_principals p \
           JOIN perm.engine_grants g ON g.id = p.engine_grant_id \
           JOIN engine_mgmt.engines e ON e.id = g.engine_id \
          WHERE p.kind = $1::perm.principal_kind AND p.ref = ANY($2) \
            AND e.authz_service_id IS NOT NULL",
    )
    .bind(kind)
    .bind(refs)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

/// F11 (spec E21, amended): the profiles of every engine whose impersonation
/// grants target `user_id`, with those engines' rows held `FOR SHARE`. Deleting that user cascades those grants away, so
/// the delete must recompile these profiles in its own transaction or the
/// compiled policy keeps naming a username the next person can be minted.
pub async fn affected_services_for_user_delete(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
) -> Result<Vec<i64>, CompileError> {
    // Security review M1: the engines' rows `FOR SHARE`, ascending, BEFORE
    // their profiles are read, so a concurrent rebind (which UPDATEs the row)
    // either commits first — and this reads the profile it moved to — or waits
    // until this delete has recompiled. Same order as an engine-grant write:
    // group locks, engine rows, profile locks.
    let engines: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT engine_id FROM perm.engine_grants WHERE target_user_id = $1 \
          ORDER BY engine_id",
    )
    .bind(user_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;
    let mut ids: Vec<i64> = Vec::new();
    for engine_id in engines {
        let profile: Option<Option<i64>> = sqlx::query_scalar(
            "SELECT authz_service_id FROM engine_mgmt.engines WHERE id = $1 FOR SHARE",
        )
        .bind(engine_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(db_err)?;
        ids.extend(profile.flatten());
    }
    normalize_service_order(&mut ids);
    Ok(ids)
}

/// Every stored reference to a group in `group_names` by NAME: the grant
/// principals, rule exemptions and (F11) engine-grant principals
/// [`affected_services_for_group_membership`] reads, one row each, ordered by name, source, id and position. The body of
/// `egret_auth::groups::GroupMembershipRecompiler::group_references_in_tx`
/// (2026-10-18 design R8), which the OIDC login asks before creating a group
/// for a claimed name with no row.
///
/// The SAME two tables as the affected set, and every rule regardless of its
/// selector, because a reference this misses is one a group created under
/// that name would inherit. `public` is filtered out, as it is there: it is
/// never expanded, so a `groups` row with that name takes nothing over.
pub async fn group_references_via(
    conn: &mut sqlx::PgConnection,
    group_names: &[String],
) -> Result<Vec<egret_auth::groups::GroupReference>, CompileError> {
    if group_names.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_as(
        "SELECT p.ref AS group_name, 'grant' AS source, g.id, \
                NULL::text AS rule_name, g.effect::text AS effect, p.mode::text AS position \
           FROM perm.grant_principals p JOIN perm.grants g ON g.id = p.grant_id \
          WHERE p.kind = 'group'::perm.principal_kind AND p.ref = ANY($1) AND p.ref <> $2 \
         UNION ALL \
         SELECT e.ref, 'rule', r.id, r.name, NULL::text, e.mode::text \
           FROM perm.rule_exemptions e JOIN perm.rules r ON r.id = e.rule_id \
          WHERE e.kind = 'group'::perm.principal_kind AND e.ref = ANY($1) AND e.ref <> $2 \
         UNION ALL \
         SELECT ep.ref, 'engine_grant', ep.engine_grant_id, NULL::text, 'allow', 'include' \
           FROM perm.engine_grant_principals ep \
          WHERE ep.kind = 'group'::perm.principal_kind AND ep.ref = ANY($1) AND ep.ref <> $2 \
         ORDER BY group_name, source, id, position",
    )
    .bind(group_names)
    .bind(crate::rules::PUBLIC_GROUP)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)
}

/// Take the row locks a recompile of `service_ids` will need, in ascending id
/// order, **before** the model write they accompany.
///
/// **This ordering is not an optimisation, it is what removes a deadlock.**
/// `perm.compiled` carries `ON DELETE CASCADE` foreign keys onto
/// `perm.grants`/`perm.rules`, so `apply_diff_in_tx`'s `record_compiled`
/// INSERT takes a FK share lock on the grant row it records. Meanwhile a
/// delete handler's `DELETE FROM perm.grants` takes an exclusive lock on that
/// same row. If the delete transaction locked the grant row first and only
/// then reached for the profile lock, while a concurrent tag-rule transaction
/// held the profile lock and was blocked on the grant row, the two waited on
/// each other and Postgres killed one with a deadlock — surfacing as an
/// intermittent 500 on an otherwise valid delete. (Observed: one run in three
/// of `tests/routes.rs` before this call was added.)
///
/// Acquiring every profile lock the request will need before touching any
/// `perm.*` row makes the order global: profiles ascending, then model rows.
/// `apply_diff_in_tx` still takes its own lock per service — re-taking a lock
/// this transaction already holds is free, and it keeps the invariant true for
/// callers that do not come through here (the tag-write path).
pub async fn lock_services_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_ids: &[i64],
) -> Result<(), CompileError> {
    // The ascending order is the invariant this function exists to hold, so it
    // is CHECKED here rather than left as a comment each producer is trusted to
    // honour (SP4a Task 9 review, I-2: the one producer that did not —
    // `bump_bound_tag_versions`' bulk UPDATE, which locked in scan order — is
    // exactly what showed a prose invariant was not enough).
    debug_assert!(
        service_ids.is_sorted(),
        "lock_services_in_tx requires ascending service ids, got {service_ids:?} — \
         an unordered producer reintroduces the deadlock this ordering removes"
    );
    // **SP4a Task 11, defect 2: the set is EXPANDED before it is sorted.** A
    // recompile now also reads, writes and locks the profile's TAG service
    // (`Compiled::sweep`), and a tag service's id can be LOWER than its
    // profile's — the live cluster's `sp3c2-trino-live` is profile 52 with tag
    // service 51. Pre-locking the profiles alone and discovering their tag
    // services only inside `apply_diff_in_tx` would therefore produce a
    // DESCENDING pair on this path while a caller that does not pre-lock takes
    // the same pair ascending: the exact opposed order two transactions
    // deadlock on. Locking the expanded, sorted set here keeps every path's
    // acquisition order ascending over the same universe.
    let expanded = services_and_tag_services_via(tx, service_ids)
        .await
        .map_err(db_err)?;
    // The one spelling of the row lock, shared with the policy editor's
    // overlap check (SP4d-4 Task 3 fix round 1, review I1): one statement per
    // id, ascending — see `egret_policy::lock_service_rows_via`.
    egret_policy::lock_service_rows_via(tx, &expanded)
        .await
        .map_err(db_err)
}

/// The policy editor's recompile seam (final review I-3): an editor write that
/// changes a data mask on a StarRocks profile recompiles that profile in the
/// editor's own transaction, so the clearance mask's fold of the profile's
/// other masks is never stale (read [`egret_policy::ProfileRecompiler`]).
/// A refusal of the recompile refuses the editor's write: an overlap with its
/// structured body, an unfoldable mask as a 409 that says why.
pub struct PermRecompiler;

#[async_trait::async_trait]
impl egret_policy::ProfileRecompiler for PermRecompiler {
    async fn recompile_in_tx(
        &self,
        tx: &mut Transaction<'static, Postgres>,
        service_ids: &[i64],
    ) -> Result<(), egret_policy::PolicyHttpError> {
        use egret_policy::PolicyHttpError;
        match recompile_services_in_tx(tx, service_ids).await {
            Ok(_) => Ok(()),
            Err(e) => Err(match crate::overlap::refusal_body(&e) {
                Some(body) => PolicyHttpError::Overlap { body },
                None => match e {
                    CompileError::Db(detail) => {
                        tracing::error!(%detail, ?service_ids, "editor write: recompile failed");
                        PolicyHttpError::Internal
                    }
                    other => PolicyHttpError::Conflict(format!(
                        "the profile cannot be recompiled with this change, which its \
                         classification masks depend on: {other}"
                    )),
                },
            }),
        }
    }
}

/// Recompile every service in `service_ids` on the CALLER's open transaction.
/// Stops at the first failure and propagates it, so the caller's `?` rolls the
/// model write back with it: a grant or rule that is stored but never compiled
/// is worse than a rejected one, because egret's UI shows it as live while the
/// engine enforces the previous policy set.
///
/// `service_ids` must be in ascending order — `apply_diff_in_tx` takes a row
/// lock per service, and a consistent acquisition order is what turns two
/// overlapping recompiles into a queue instead of a deadlock.
/// [`affected_services_for_grant`]/[`affected_services_for_rule`] guarantee it.
pub async fn recompile_services_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_ids: &[i64],
) -> Result<Vec<CompileSummary>, CompileError> {
    // Take every lock this loop will need, expanded and ascending, BEFORE the
    // first recompile — otherwise service N's tag service could be locked
    // after service N+1's profile and the global order stops being ascending
    // (see `lock_services_in_tx`). Re-taking a lock this transaction already
    // holds is free, so callers that pre-locked lose nothing.
    lock_services_in_tx(tx, service_ids).await?;
    // After the profile locks (J-F3 order: profiles, then rows), mint every
    // username this loop would mint in ONE ascending sweep, so the per-profile
    // expansions below take no `users` row lock out of order
    // (`crate::members::premint_model_usernames`).
    if !service_ids.is_empty() {
        crate::members::premint_model_usernames(tx).await?;
    }
    let mut out = Vec::with_capacity(service_ids.len());
    for &service_id in service_ids {
        let summary = recompile_locked_service_in_tx(tx, service_id).await?;
        if !summary.skipped.is_empty() {
            tracing::warn!(
                service_id,
                skipped_count = summary.skipped.len(),
                "perm-model write's recompile skipped some grants/rules for this service"
            );
        }
        out.push(summary);
    }
    Ok(out)
}
