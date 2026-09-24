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
//!    tagged_columns_for_service_via, tag_name_for_key_via}` (all
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
//! **What this module does about `WorkspaceMembers` (Task 6's deferred
//! expansion).** `compile_grant`/`compile_rule` both REFUSE a `WorkspaceMembers`
//! principal (`CompileError::UnexpandedPrincipalKind`) rather than silently
//! dropping it — and, per Task 6's report, EVERY catalog assigned to a
//! workspace carries exactly one such grant (the implicit workspace-assignment
//! grant), so that refusal is not a corner case here: it is the FIRST grant
//! this driver's very first real recompile will see on any catalog anyone
//! has ever assigned. Propagating that refusal as `recompile_service`'s own
//! `Err` would mean every tag write on every assigned catalog fails outright
//! — the StarRocks masking recompile this module exists for would never run
//! on the one class of catalog SP4a's own workspace-assignment feature
//! creates automatically. That is strictly worse than the staleness this
//! module is built to close.
//!
//! So: `recompile_service`/`recompile_service_in_tx` SKIP a grant or rule
//! whose ONLY compile failure is `UnexpandedPrincipalKind`, recording it in
//! [`CompileSummary::skipped`] with a reason AND logging it via
//! `tracing::warn!` (fix round 1 — the summary alone was silently discarded
//! by this module's only production caller, `egret-bi::catalog_tags`), and
//! continue compiling everything else. This is not a silent drop — nothing is
//! EMITTED for a skipped grant (fail-closed: a workspace-assignment grant
//! this driver cannot yet express compiles to no policy, matching what a
//! human reading Task 6's report already knows to be true, rather than a
//! policy that grants nobody OR one that silently drops an exclusion). Every
//! OTHER `CompileError` — `ExpansionCeiling`, `UnusableUserRef`,
//! `UnsupportedMaskType`, an unparseable custom mask, an unexpressible
//! exemption, a missing tag/tag service — is a genuine authoring or
//! resolution bug and still ABORTS the whole recompile for this service:
//! weakening those refusals to a skip would be exactly the "make something
//! pass" the compiler exists to refuse. Expanding `WorkspaceMembers` for real
//! (workspace-membership reads, recompiling on a membership WRITE the same
//! way this module recompiles on a tag write) is Task 6's explicitly named
//! follow-up and is NOT done here.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use egret_perm::{
    list_grants_via, list_rules_via, CompileError, EngineType, Grant, PermStore, Rule, Selector,
};
use egret_policy::admin::access_to_ranger_json;
use egret_policy::{
    catalogs_for_service_via, compiler_targeted_services_via, get_service_by_id_via,
    services_for_catalog_via, tag_name_for_key_via, tagged_columns_for_service_via, PolicyStore,
    COMPILER_GRANT_PREFIX, COMPILER_RULE_PREFIX,
};

use crate::grants::compile_grant;
use crate::rules::{compile_rule, stamp_service_type, ResolvedNames, TaggedColumn};

/// The StarRocks per-column expansion ceiling this driver enforces. D5's own
/// words: "The ceiling's value is set from a measurement on kind, not chosen
/// in advance" — no such measurement has been run in this worktree (unlike
/// D7's, which WAS measured and is cited by name in `assignment.rs`). This
/// number is therefore a conservative placeholder, not a measured one, and
/// should be replaced (and, ideally, made configurable per profile) the first
/// time a real StarRocks tag-rule fan-out is measured on kind.
pub const DEFAULT_EXPANSION_CEILING: usize = 500;

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

/// Which half of the `perm` model a compiled policy (or a skip) came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Grant,
    Rule,
}

impl SourceKind {
    fn as_str(self) -> &'static str {
        match self {
            SourceKind::Grant => "grant",
            SourceKind::Rule => "rule",
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

struct DesiredPolicy {
    body: Value,
    source_kind: SourceKind,
    grant_id: Option<Uuid>,
    rule_id: Option<Uuid>,
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
fn engine_type_for(service_type: &str) -> Option<EngineType> {
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
) -> Result<Option<(BTreeMap<String, DesiredPolicy>, Vec<SkippedItem>)>, CompileError> {
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

    let tag_service: Option<String> = if engine == EngineType::Trino {
        svc.config_jsonb
            .get("tagService")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    } else {
        None
    };

    let all_grants = list_grants_via(conn).await.map_err(db_err)?;
    let all_rules = list_rules_via(conn).await.map_err(db_err)?;

    let mut desired: BTreeMap<String, DesiredPolicy> = BTreeMap::new();
    let mut skipped: Vec<SkippedItem> = Vec::new();

    for g in &all_grants {
        let Some(catalog_name) = catalog_names.get(&g.scope.catalog_id) else {
            continue; // not reachable by this service — out of scope
        };
        match compile_grant(g, catalog_name, &svc.name, engine) {
            Ok(input) => {
                let name = input.name.clone();
                let body = stamp_service_type(access_to_ranger_json(&input), engine);
                desired.insert(
                    name,
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
                    "principal kind {kind:?} is not expanded by this compiler yet (SP4a Task 6)"
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

    for r in &all_rules {
        compile_one_rule(
            r,
            engine,
            &svc.name,
            tag_service.as_deref(),
            conn,
            service_id,
            &catalog_names,
            &mut desired,
            &mut skipped,
        )
        .await?;
    }

    Ok(Some((desired, skipped)))
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
    desired: &mut BTreeMap<String, DesiredPolicy>,
    skipped: &mut Vec<SkippedItem>,
) -> Result<(), CompileError> {
    let (catalog_name, tag_name, tagged): (String, Option<String>, Vec<TaggedColumn>) =
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
                    tagged_columns_for_service_via(conn, service_id, key)
                        .await
                        .map_err(db_err)?
                        .into_iter()
                        .map(|row| TaggedColumn {
                            catalog: row.catalog,
                            schema: row.schema_name,
                            table: row.table_name,
                            column: row.column_name,
                        })
                        .collect()
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
                let name = body_name(&body, &format!("rule {:?}", r.name))?;
                desired.insert(
                    name,
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
                "principal kind {kind:?} is not expanded by this compiler yet (SP4a Task 6)"
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
                "profile has no `tagService` configured, so tag rule {rule:?} cannot compile here"
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
    name: String,
    policy_jsonb: Value,
}

/// Diff `desired` against the service's CURRENT `policy.authz_policies` state
/// and write only the difference, on `tx` (never opens or commits its own —
/// the caller owns both). Bumps `policy.authz_services.policy_version`
/// exactly once, only if something was actually written.
///
/// **Fix round 1 Critical**: "current state" is read directly from
/// `policy.authz_policies` (scoped by `service_id` and this compiler's own
/// name prefixes), NOT through `perm.compiled` — see the module doc comment
/// for why a join through `perm.compiled` is blind to a policy whose grant
/// or rule was deleted (its `perm.compiled` row cascades away with it, but
/// the policy itself does not).
async fn apply_diff_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
    desired: BTreeMap<String, DesiredPolicy>,
    skipped: Vec<SkippedItem>,
) -> Result<CompileSummary, CompileError> {
    // **Serialize concurrent recompiles of the SAME profile** (SP4a Task 9).
    // Everything below reads `policy.authz_policies` for this service, decides
    // a diff from what it read, and then writes against the policy ids it
    // read. Under Postgres' default READ COMMITTED that plan is only valid if
    // no other transaction can commit a change to this service's policies in
    // between — and one can: a concurrently deleted policy row makes the
    // `Some((policy_id, _))` branch below UPDATE zero rows and then insert a
    // `perm.compiled` row pointing at an id that no longer exists, which the
    // `compiled_policy_id_fkey` foreign key rejects. That was not reachable
    // while the compiler's only caller was the tag-write path (one warehouse's
    // services, one writer); Task 9's model-mutation routes make two requests
    // recompiling one profile ordinary, and an installation-wide tag-rule
    // recompile makes it likely.
    //
    // A row lock on the service is the smallest thing that makes the
    // read-decide-write sequence atomic. **Callers must acquire services in
    // ASCENDING id order** — `recompile_services_in_tx`,
    // `services_for_catalog_via`, `compiler_targeted_services_via` and
    // `egret_catalog_registry::services_bound_to_warehouse` all order that way
    // — so two transactions recompiling overlapping sets queue rather than
    // deadlock.
    sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE")
        .bind(service_id)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;

    let existing: Vec<ExistingPolicyRow> = sqlx::query_as(
        "SELECT id, name, policy_jsonb FROM policy.authz_policies \
         WHERE service_id = $1 AND (name LIKE $2 || '%' OR name LIKE $3 || '%')",
    )
    .bind(service_id)
    .bind(COMPILER_GRANT_PREFIX)
    .bind(COMPILER_RULE_PREFIX)
    .fetch_all(&mut **tx)
    .await
    .map_err(db_err)?;

    let mut existing_by_name: HashMap<String, (i64, Value)> = existing
        .into_iter()
        .map(|row| (row.name, (row.id, row.policy_jsonb)))
        .collect();

    let mut summary = CompileSummary {
        skipped,
        ..Default::default()
    };

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
    if !desired.is_empty() {
        sqlx::query(
            "UPDATE policy.authz_services SET origin = 'generated' \
             WHERE id = $1 AND origin <> 'generated'",
        )
        .bind(service_id)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    }

    for (name, dp) in desired {
        debug_assert!(
            name.starts_with(COMPILER_GRANT_PREFIX) || name.starts_with(COMPILER_RULE_PREFIX),
            "compiled policy name {name:?} does not carry this compiler's own prefix — \
             the existing-state sweep above would never find it again"
        );
        match existing_by_name.remove(&name) {
            None => {
                let policy_id: i64 = sqlx::query_scalar(
                    "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) \
                     VALUES ($1, $2, $3) RETURNING id",
                )
                .bind(service_id)
                .bind(&name)
                .bind(&dp.body)
                .fetch_one(&mut **tx)
                .await
                .map_err(db_err)?;
                record_compiled(tx, service_id, policy_id, &dp).await?;
                summary.created += 1;
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

    // Everything left in `existing_by_name` is a policy this recompile no
    // longer produces (the grant/rule behind it was deleted, its catalog is
    // no longer reachable, or a StarRocks column lost its tag) — reaped here
    // regardless of whether a `perm.compiled` row for it still exists.
    for (policy_id, _body) in existing_by_name.into_values() {
        sqlx::query("DELETE FROM policy.authz_policies WHERE id = $1")
            .bind(policy_id)
            .execute(&mut **tx)
            .await
            .map_err(db_err)?;
        summary.deleted += 1;
    }

    if summary.wrote_anything() {
        sqlx::query(
            "UPDATE policy.authz_services SET policy_version = policy_version + 1, updated_at = now() \
             WHERE id = $1",
        )
        .bind(service_id)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
    }

    Ok(summary)
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
pub async fn recompile_service_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    service_id: i64,
) -> Result<CompileSummary, CompileError> {
    let Some((desired, skipped)) = compile_desired(&mut **tx, service_id).await? else {
        return Ok(CompileSummary::default());
    };
    apply_diff_in_tx(tx, service_id, desired, skipped).await
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
    let mut tx = store.pool().begin().await.map_err(db_err)?;
    let summary = recompile_service_in_tx(&mut tx, service_id).await?;
    tx.commit().await.map_err(db_err)?;
    Ok(summary)
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
    // One statement per id rather than `WHERE id = ANY(...) ORDER BY id FOR
    // UPDATE`: Postgres does not promise that a sort is applied before row
    // locks are taken, and the ORDER is the entire point here.
    for &service_id in service_ids {
        sqlx::query("SELECT id FROM policy.authz_services WHERE id = $1 FOR UPDATE")
            .bind(service_id)
            .execute(&mut **tx)
            .await
            .map_err(db_err)?;
    }
    Ok(())
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
    let mut out = Vec::with_capacity(service_ids.len());
    for &service_id in service_ids {
        let summary = recompile_service_in_tx(tx, service_id).await?;
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
