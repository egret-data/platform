//! `recompile_service`: keeps a profile's compiled Ranger policies (`perm.compiled`
//! + `policy.authz_policies`) true to the CURRENT `perm` model and the CURRENT
//! tag assignments underneath it (SP4a Task 7).
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
//! So: `recompile_service` SKIPS a grant or rule whose ONLY compile failure is
//! `UnexpandedPrincipalKind`, recording it in [`CompileSummary::skipped`] with
//! a reason, and continues compiling everything else. This is not a silent
//! drop — the caller can inspect `skipped`, and nothing is EMITTED for a
//! skipped grant (fail-closed: a workspace-assignment grant this driver
//! cannot yet express compiles to no policy, matching what a human reading
//! Task 6's report already knows to be true, rather than a policy that grants
//! nobody OR one that silently drops an exclusion). Every OTHER `CompileError`
//! — `ExpansionCeiling`, `UnusableUserRef`, `UnsupportedMaskType`, an
//! unparseable custom mask, an unexpressible exemption, a missing tag/tag
//! service — is a genuine authoring or resolution bug and still ABORTS the
//! whole recompile for this service: weakening those refusals to a skip
//! would be exactly the "make something pass" the compiler exists to refuse.
//! Expanding `WorkspaceMembers` for real (workspace-membership reads,
//! recompiling on a membership WRITE the same way this module recompiles on a
//! tag write) is Task 6's explicitly named follow-up and is NOT done here.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;
use uuid::Uuid;

use egret_perm::{CompileError, EngineType, PermStore, Rule, Selector};
use egret_policy::admin::access_to_ranger_json;
use egret_policy::PolicyStore;

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
/// recompile.
#[derive(Debug, Clone)]
pub struct SkippedItem {
    pub source_kind: SourceKind,
    pub id: Uuid,
    pub reason: String,
}

/// What one `recompile_service` call did. Every count is scoped to THIS
/// service — a caller comparing two calls' summaries (or the DB state
/// before/after) is the only way to tell a no-op recompile from a busy one;
/// see `crates/egret-perm-compile/tests/recompile.rs`'s
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

/// Recompile `service_id`'s profile: load the `perm` model, compile it for
/// that service's engine type, diff against `perm.compiled` +
/// `policy.authz_policies`, and write only the difference (create the new,
/// update the changed, delete the gone). Bumps `policy.authz_services.policy_version`
/// exactly once, only if something was actually written, in the SAME
/// transaction as those writes.
///
/// Reads (the model, the reachable catalogs, the tagged columns, the tag
/// name) run BEFORE that transaction opens, against `store`'s and `policy`'s
/// own pools — this call does not attempt serializable isolation across the
/// whole operation, only atomicity for its OWN writes plus the version bump
/// (see the module doc comment on `WorkspaceMembers` for the one place this
/// function deliberately does not abort on a known, named limitation).
pub async fn recompile_service(
    store: &PermStore,
    policy: &PolicyStore,
    service_id: i64,
) -> Result<CompileSummary, CompileError> {
    let svc = policy.get_service_by_id(service_id).await.map_err(db_err)?;
    let Some(engine) = engine_type_for(&svc.r#type) else {
        return Ok(CompileSummary::default());
    };

    let catalogs = policy
        .catalogs_for_service(service_id)
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

    let all_grants = store.list_grants().await.map_err(db_err)?;
    let all_rules = store.list_rules().await.map_err(db_err)?;

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
                skipped.push(SkippedItem {
                    source_kind: SourceKind::Grant,
                    id: g.id,
                    reason: format!(
                        "principal kind {kind:?} is not expanded by this compiler yet (SP4a Task 6)"
                    ),
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
            policy,
            service_id,
            &catalog_names,
            &mut desired,
            &mut skipped,
        )
        .await?;
    }

    apply_diff(store, service_id, desired, skipped).await
}

#[allow(clippy::too_many_arguments)]
async fn compile_one_rule(
    r: &Rule,
    engine: EngineType,
    service_name: &str,
    tag_service: Option<&str>,
    policy: &PolicyStore,
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
                let Some(name) = policy.tag_name_for_key(key).await.map_err(db_err)? else {
                    // The rule names a tag that no longer exists. Not a
                    // compiler defect and not the WorkspaceMembers case
                    // either — record and move on rather than aborting the
                    // whole service's recompile over one dangling rule.
                    skipped.push(SkippedItem {
                        source_kind: SourceKind::Rule,
                        id: r.id,
                        reason: format!("tag key {key:?} has no matching row in `tags`"),
                    });
                    return Ok(());
                };
                let tagged = if engine == EngineType::StarRocks {
                    policy
                        .tagged_columns_for_service(service_id, key)
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
            skipped.push(SkippedItem {
                source_kind: SourceKind::Rule,
                id: r.id,
                reason: format!(
                    "principal kind {kind:?} is not expanded by this compiler yet (SP4a Task 6)"
                ),
            });
            Ok(())
        }
        Err(other) => Err(other),
    }
}

#[derive(sqlx::FromRow)]
struct ExistingCompiledRow {
    id: i64,
    name: String,
    policy_jsonb: Value,
}

/// Diff `desired` against the service's current `perm.compiled` +
/// `policy.authz_policies` state and write only the difference, in one
/// transaction that also carries the single `policy_version` bump.
async fn apply_diff(
    store: &PermStore,
    service_id: i64,
    desired: BTreeMap<String, DesiredPolicy>,
    skipped: Vec<SkippedItem>,
) -> Result<CompileSummary, CompileError> {
    let mut tx = store.pool().begin().await.map_err(db_err)?;

    let existing: Vec<ExistingCompiledRow> = sqlx::query_as(
        "SELECT ap.id, ap.name, ap.policy_jsonb \
         FROM perm.compiled pc JOIN policy.authz_policies ap ON ap.id = pc.policy_id \
         WHERE pc.service_id = $1",
    )
    .bind(service_id)
    .fetch_all(&mut *tx)
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

    for (name, dp) in desired {
        match existing_by_name.remove(&name) {
            None => {
                let policy_id: i64 = sqlx::query_scalar(
                    "INSERT INTO policy.authz_policies (service_id, name, policy_jsonb) \
                     VALUES ($1, $2, $3) RETURNING id",
                )
                .bind(service_id)
                .bind(&name)
                .bind(&dp.body)
                .fetch_one(&mut *tx)
                .await
                .map_err(db_err)?;
                sqlx::query(
                    "INSERT INTO perm.compiled (service_id, policy_id, source_kind, grant_id, rule_id) \
                     VALUES ($1, $2, $3, $4, $5)",
                )
                .bind(service_id)
                .bind(policy_id)
                .bind(dp.source_kind.as_str())
                .bind(dp.grant_id)
                .bind(dp.rule_id)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
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
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                    summary.updated += 1;
                }
            }
        }
    }

    // Everything left in `existing_by_name` is a policy this recompile no
    // longer produces (the grant/rule behind it was deleted, its catalog is
    // no longer reachable, or a StarRocks column lost its tag). Delete the
    // policy row; `perm.compiled.policy_id REFERENCES policy.authz_policies(id)
    // ON DELETE CASCADE` (migration 0154) removes the compiled-provenance row
    // for free.
    for (policy_id, _body) in existing_by_name.into_values() {
        sqlx::query("DELETE FROM policy.authz_policies WHERE id = $1")
            .bind(policy_id)
            .execute(&mut *tx)
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
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    }

    tx.commit().await.map_err(db_err)?;
    Ok(summary)
}
