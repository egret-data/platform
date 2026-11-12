//! Every read the lowering of one enforcement point needs, in one place
//! (governance sub-project 1, Task 4).
//!
//! [`load_ep`] is the head `compile_desired` used to carry inline, moved here
//! verbatim: the same statements, on the caller's connection, in the same
//! order. It returns the profile as an [`EpInput`] plus the store rows the
//! lowering consumes, as stored, and the [`Directory`]
//! [`crate::members::expand_enumerated_principals`] read for them, which each
//! lowering expands its audiences against.
//!
//! The one read [`load_ep`] does NOT make is the bound engines' plugin
//! versions ([`bound_engines`]). Today only a StarRocks profile with at least
//! one classified column reads them, and only after its clearances and
//! enabled hand-written policies; reading them here would add a statement to
//! every other StarRocks compile and move it ahead of those reads. So the
//! [`EpInput`] leaves the loader with `engines` empty and `default_version`
//! `Err(NOT_READ)`, and the clearance-mask fold fills both at the old point.

use std::collections::{BTreeMap, HashMap, HashSet};

use egret_perm::Selector;
use egret_perm::{
    list_engine_grants_for_profile_via, list_grants_for_catalogs_via, list_rules_for_catalogs_via,
    CompileError, EngineGrant, EngineType, Grant, Rule,
};
use egret_perm_ir::audience::Directory;
use egret_perm_ir::ep::{BoundEngine, EpInput, EpKind, Mechanism};
use egret_perm_ir::lower::engine_privileges::Check;
use egret_perm_ir::lower::rules::{needs_tagged_tables, RuleInputs};
use egret_perm_ir::lower::withhold::{
    needs_table_shapes, needs_targets, Gate, GateTarget, TableShape,
};
use egret_perm_ir::statement::{Lowering, ReasonCode, Source};
use egret_policy::{
    catalogs_for_service_via, get_service_by_id_via, service_id_by_name_via,
    tag_assignments_for_service_via, tag_name_for_key_via,
};
use egret_ranger_eval::eval::PluginVersion;

use crate::table_columns::{active_views_batch_via, current_columns_batch_via, TableKey};
use uuid::Uuid;

/// `EpInput::default_version` before [`bound_engines`] has run.
pub(crate) const NOT_READ: &str = "not read";

/// One enforcement point and everything `compile_desired` lowers for it.
pub(crate) struct Loaded {
    /// `engines` is empty and `default_version` is `Err(NOT_READ)` until
    /// [`bound_engines`] fills them (see the module doc).
    pub ep: EpInput,
    pub profile_name: String,
    pub tag_service: Option<String>,
    pub writable: HashMap<String, i64>,
    pub sweep: Vec<i64>,
    pub leaving: crate::table_columns::Leaving,
    pub grants: Vec<Grant>,
    pub rules: Vec<Rule>,
    pub engine_grants: Vec<EngineGrant>,
    /// Who each workspace, role and group referenced by `grants`, `rules` and
    /// `engine_grants` stands for; each lowering expands against it.
    pub directory: Directory,
}

/// Read profile `service_id` (every read against `conn`, so a caller running
/// this inside its own open transaction sees its own uncommitted writes).
/// `None` means `service_id` names something this compiler doesn't target
/// (see [`crate::trigger::engine_type_for`]).
pub(crate) async fn load_ep(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    missing: crate::members::MissingUsername,
) -> Result<Option<Loaded>, CompileError> {
    let svc = get_service_by_id_via(conn, service_id)
        .await
        .map_err(crate::db_error::from_store)?;
    let Some(engine) = crate::trigger::engine_type_for(&svc.r#type) else {
        return Ok(None);
    };

    let catalogs = catalogs_for_service_via(conn, service_id)
        .await
        .map_err(crate::db_error::from_store)?;
    // Only ever looked up by key, so the map's order is irrelevant.
    let catalog_names: BTreeMap<Uuid, String> =
        catalogs.into_iter().map(|c| (c.id, c.name)).collect();
    // A held rename's leaving coordinates (rename spec C8): emitted, never
    // counted against a ceiling.
    let leaving = crate::table_columns::leaving_via(conn).await?;

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
            Some(name) => match service_id_by_name_via(conn, name)
                .await
                .map_err(crate::db_error::from_store)?
            {
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

    // Only the model rows this profile can lower: the grants and path rules on
    // the catalogs it reaches, and every tag rule. The lowerings skip a grant
    // or path rule on any other catalog (`lower_grants`, `lower_rules`), so
    // reading them changed nothing but the cost, which grew with the whole
    // installation on every profile's rebuild (SP2b Task 26 follow-up).
    // Principals are expanded below for these rows only, so a compile mints a
    // username only for someone this profile's policies name.
    let reachable: Vec<Uuid> = catalog_names.keys().copied().collect();
    let grants = list_grants_for_catalogs_via(conn, &reachable)
        .await
        .map_err(CompileError::from_perm)?;
    let rules = list_rules_for_catalogs_via(conn, &reachable)
        .await
        .map_err(CompileError::from_perm)?;
    // D3: read who every `workspace_members` principal — and, since SP4c Task
    // 2b, every `role` principal, and since J-F3 Task 1b every non-`public`
    // `group` principal — stands for BEFORE anything is compiled, so each
    // lowering expands its audiences against the directory and the pure
    // compilers never see one. See `crate::members` for what a
    // member is, what it costs, and why a role name cannot be left for Ranger
    // to resolve.
    // F11: the engine grants of every engine bound to this profile, expanded
    // in the same call so a person named by a grant and an engine grant
    // resolves to one name.
    let engine_grants = list_engine_grants_for_profile_via(conn, service_id)
        .await
        .map_err(CompileError::from_perm)?;
    let directory = crate::members::expand_enumerated_principals(
        conn,
        &grants,
        &rules,
        &engine_grants,
        missing,
    )
    .await?;

    let ep = EpInput {
        profile_id: service_id,
        profile_name: svc.name.clone(),
        kind: EpKind {
            engine,
            mechanism: match svc.mechanism.as_str() {
                "opa" => Mechanism::Opa,
                _ => Mechanism::Ranger,
            },
        },
        engines: Vec::new(),
        default_version: Err(NOT_READ.to_string()),
        default_image_level: None,
        classification_channel: tag_service.is_some(),
        catalogs: catalog_names,
    };
    Ok(Some(Loaded {
        ep,
        profile_name: svc.name,
        tag_service,
        writable,
        sweep,
        leaving,
        grants,
        rules,
        engine_grants,
        directory,
    }))
}

/// Read what each of `rules` needs from the classification store, in rule
/// order: for a tag selector, the tag's `tags.name` and — when the matrix
/// expands the rule on this EP ([`needs_tagged_tables`]) — the tables carrying
/// the tag, grouped by [`crate::rules::group_by_table`]. A path selector reads
/// nothing. These are the statements the recompile driver used to make inside
/// its per-rule loop, in the same order; the lowering between them did no
/// I/O, so issuing them all first changes neither the statements nor their
/// order.
///
/// A rule naming a tag that no longer exists is a read result, so its skip is
/// produced here, keyed by rule id for [`egret_perm_ir::lower::rules::lower_rules`]
/// to place in rule order.
pub(crate) async fn load_rule_inputs(
    conn: &mut sqlx::PgConnection,
    ep: &EpInput,
    rules: &[Rule],
) -> Result<(BTreeMap<Uuid, RuleInputs>, BTreeMap<Uuid, Lowering>), CompileError> {
    let mut inputs = BTreeMap::new();
    let mut skips = BTreeMap::new();
    for r in rules {
        let Selector::Tag { key } = &r.selector else {
            continue;
        };
        let Some(name) = tag_name_for_key_via(conn, key)
            .await
            .map_err(crate::db_error::from_store)?
        else {
            // The rule names a tag that no longer exists. Not a compiler
            // defect and not the WorkspaceMembers case either — record and
            // move on rather than aborting the whole service's recompile over
            // one dangling rule.
            let reason = format!("tag key {key:?} has no matching row in `tags`");
            tracing::warn!(
                service_id = ep.profile_id,
                rule_id = %r.id,
                reason = %reason,
                "rule skipped during recompile"
            );
            skips.insert(
                r.id,
                Lowering {
                    source: Source::Rule {
                        rule_id: r.id,
                        rule_name: r.name.clone(),
                    },
                    code: ReasonCode::UnknownClassification,
                    detail: reason,
                },
            );
            continue;
        };
        // StarRocks expands every tag rule; Trino expands a row filter over
        // the tables carrying the tag on columns only (KI-287).
        let tagged = if needs_tagged_tables(ep, r) {
            crate::rules::group_by_table(
                tag_assignments_for_service_via(conn, ep.profile_id, key)
                    .await
                    .map_err(crate::db_error::from_store)?,
            )
            .iter()
            .map(crate::rules::TableTarget::to_tagged)
            .collect()
        } else {
            Vec::new()
        };
        inputs.insert(
            r.id,
            RuleInputs {
                classification_name: Some(name),
                tagged,
            },
        );
    }
    Ok((inputs, skips))
}

/// Check every engine grant of the profile for the lowering
/// ([`egret_perm_ir::lower::engine_privileges::lower_engine_privileges`]),
/// keyed by grant id. Per grant, in grant order, the statements the recompile
/// driver used to make inside its per-grant loop:
///
/// Each grant's grantees are expanded against `dir` as the lowering expands
/// them.
///
/// - a grant whose grantees include nobody is not checked and reads nothing — the
///   lowering skips it before it looks for a check, as the old loop did
///   before it read the target;
/// - an impersonation grant reads its target
///   ([`crate::engine_grants::impersonation_target_ok`]), and one refused
///   there is skipped with that reason and goes no further;
/// - every other check is [`crate::engine_grants::check_engine_grant_for`], which
///   is pure, and an `InvalidEngineGrant` (or `UnexpandedPrincipalKind`) is
///   skipped with the error's text.
///
/// The lowering between those reads did no I/O, so issuing them all first
/// changes neither the statements nor their order on the success path. It
/// runs at the point the old loop ran: after the grant lowering, before the
/// rule reads ([`load_rule_inputs`]).
pub(crate) async fn load_engine_privilege_checks(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    engine: EngineType,
    grants: &[EngineGrant],
    dir: &Directory,
) -> Result<BTreeMap<Uuid, Check>, CompileError> {
    let mut checks = BTreeMap::new();
    for g in grants {
        let grantees = egret_perm_ir::audience::audience(&g.principals, dir);
        if grantees.include.is_empty() {
            continue;
        }
        let target = match g.target_user_id {
            Some(user_id) => {
                match crate::engine_grants::impersonation_target_ok(conn, service_id, user_id)
                    .await?
                {
                    Ok(username) => Some(username),
                    Err(reason) => {
                        checks.insert(g.id, Check::Skip(reason));
                        continue;
                    }
                }
            }
            None => None,
        };
        let check = match crate::engine_grants::check_engine_grant_for(
            g,
            !grantees.except.is_empty(),
            target.as_deref(),
            engine,
        ) {
            Ok(values) => Check::Ok {
                capability: g.capability.clone(),
                values,
            },
            Err(e @ CompileError::InvalidEngineGrant { .. })
            | Err(e @ CompileError::UnexpandedPrincipalKind { .. }) => Check::Skip(e.to_string()),
            Err(other) => return Err(other),
        };
        checks.insert(g.id, check);
    }
    Ok(checks)
}

/// One `classification_policies` row joined to the tag it gates. A row whose
/// tag no longer exists is dropped by the join — there is no tag left to deny
/// on.
#[derive(sqlx::FromRow)]
struct GateRow {
    tag_id: Uuid,
    /// `tags.name`, never `tags.key` (see [`Gate::classification_name`]).
    tag_name: String,
    /// `tags.key` — the STABLE identifier, needed only to read the targets:
    /// `tag_assignments_for_service_via` selects assignments by key, the same
    /// lookup [`load_rule_inputs`] does for a tag-selected rule.
    tag_key: String,
    required_roles: Vec<String>,
    required_groups: Vec<Uuid>,
}

/// Read every gated classification for the enforcement point `ep`
/// (`service_id`), as [`egret_perm_ir::lower::withhold::lower_withholds`]
/// consumes them. The statements, on `conn`, in the order the clearance
/// compile always made them:
///
/// 1. every `classification_policies` row joined to its tag, by `tag_id`
///    (no gate: nothing else is read);
/// 2. the holders of every role any gate names, in one query, a holder with
///    no username minted under `MissingUsername::Mint` (2026-10-18 design R5, R6:
///    a clearing-role holder is excepted now, not after the recompile that
///    follows their first query). This runs after the model expansion's
///    mints ([`load_ep`]), as it always has;
/// 3. per gate, in order, the tables carrying its tag on this service, when
///    [`needs_targets`] (StarRocks, a gate naming a role);
/// 4. when [`needs_table_shapes`]: one column read for every table any gate
///    classifies as a whole (folded, first spelling), then one view read for
///    the names the column read did not find — a name with no columns may be
///    a VIEW, which never becomes a table (KI-307 follow-up).
///
/// `cleared` is the UNION of the gate's roles' holders, sorted and
/// deduplicated.
pub(crate) async fn load_withhold_inputs(
    conn: &mut sqlx::PgConnection,
    ep: &EpInput,
    service_id: i64,
    missing: crate::members::MissingUsername,
) -> Result<Vec<Gate>, CompileError> {
    use egret_ranger_eval::resource::fold_identifier as fold;

    let rows: Vec<GateRow> = sqlx::query_as(
        "SELECT cp.tag_id, t.name AS tag_name, t.key AS tag_key, cp.required_roles, \
                cp.required_groups \
           FROM classification_policies cp \
           JOIN tags t ON t.id = cp.tag_id \
          ORDER BY cp.tag_id",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    // One query for every role named by every gate, rather than one per tag.
    let wanted: Vec<String> = rows
        .iter()
        .flat_map(|g| g.required_roles.iter().cloned())
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect();
    let members = crate::members::role_usernames(conn, &wanted, missing).await?;

    let mut gates: Vec<Gate> = Vec::with_capacity(rows.len());
    // Each gate's targets with their warehouse, which the catalog reads need.
    let mut targets: Vec<Vec<crate::rules::TableTarget>> = Vec::with_capacity(rows.len());
    for row in rows {
        let mut cleared: Vec<String> = Vec::new();
        for role in &row.required_roles {
            if let Some(names) = members.get(role) {
                cleared.extend(names.iter().cloned());
            }
        }
        cleared.sort();
        cleared.dedup();
        let gate = Gate {
            classification_id: row.tag_id,
            classification_name: row.tag_name,
            required_roles: row.required_roles,
            required_groups_empty: row.required_groups.is_empty(),
            cleared,
            targets: Vec::new(),
        };
        let read = if needs_targets(ep, &gate) {
            crate::rules::group_by_table(
                tag_assignments_for_service_via(conn, service_id, &row.tag_key)
                    .await
                    .map_err(crate::db_error::from_store)?,
            )
        } else {
            Vec::new()
        };
        gates.push(gate);
        targets.push(read);
    }
    if !needs_table_shapes(ep, &gates) {
        return Ok(gates);
    }

    type Key = (Uuid, String, String);
    let key_of = |t: &crate::rules::TableTarget| -> Key {
        (t.warehouse_id, fold(&t.schema), fold(&t.table))
    };
    let mut keys: Vec<TableKey> = Vec::new();
    let mut index: HashMap<Key, usize> = HashMap::new();
    for t in targets.iter().flatten().filter(|t| t.table_level) {
        index.entry(key_of(t)).or_insert_with(|| {
            keys.push(TableKey {
                warehouse_id: t.warehouse_id,
                schema: t.schema.clone(),
                table: t.table.clone(),
            });
            keys.len() - 1
        });
    }
    let found = current_columns_batch_via(conn, &keys).await?;
    let missing: Vec<usize> = (0..keys.len()).filter(|&i| found[i].is_none()).collect();
    let missing_keys: Vec<TableKey> = missing.iter().map(|&i| keys[i].clone()).collect();
    let views: HashSet<usize> = missing
        .iter()
        .zip(active_views_batch_via(conn, &missing_keys).await?)
        .filter_map(|(&i, view)| view.then_some(i))
        .collect();
    for (gate, read) in gates.iter_mut().zip(targets) {
        gate.targets = read
            .iter()
            .map(|t| GateTarget {
                table: t.to_tagged(),
                shape: match index.get(&key_of(t)) {
                    Some(&i) => match &found[i] {
                        Some(c) => TableShape::Known {
                            schema: c.schema.clone(),
                            table: c.table.clone(),
                            columns: c.columns.clone(),
                        },
                        None if views.contains(&i) => TableShape::View,
                        None => TableShape::Unknown,
                    },
                    None => TableShape::Unknown,
                },
            })
            .collect();
    }
    Ok(gates)
}

/// Fill `ep.engines` (each engine bound to the profile, by name, with the
/// plugin version it runs or why that is unknown) and `ep.default_version`
/// (the plugin of egret's default image for the engine type). One SELECT on
/// `conn`, the one the clearance-mask fold has always made, at the same point
/// (`trigger::fold_clearance_masks`, before
/// `egret_perm_ir::lower::withhold::view_guard_versions` decides the versions).
pub(crate) async fn bound_engines(
    conn: &mut sqlx::PgConnection,
    ep: &mut EpInput,
) -> Result<(), CompileError> {
    let bound: Vec<crate::routes::BoundEngine> = sqlx::query_as(
        "SELECT name, origin, type, ranger_plugin_version, spec FROM engine_mgmt.engines \
          WHERE authz_service_id = $1 ORDER BY name",
    )
    .bind(ep.profile_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    ep.engines = bound
        .iter()
        .map(|e| BoundEngine {
            name: e.name.clone(),
            version: e
                .plugin_version()
                .map(|v| plugin_version_label(v).to_string()),
            egret_image_level: e.egret_image_level(),
        })
        .collect();
    ep.default_version =
        crate::routes::default_image_plugin_version(crate::routes::ranger_engine(ep.kind.engine))
            .map(|v| plugin_version_label(v).to_string());
    Ok(())
}

/// The version string an [`EpInput`] carries for `v`.
fn plugin_version_label(v: PluginVersion) -> &'static str {
    match v {
        PluginVersion::Ranger2_5_0 => "2.5.0",
        PluginVersion::Ranger2_8_0 => "2.8.0",
        PluginVersion::Ranger2_9_0 => "2.9.0",
    }
}

/// The inverse of [`plugin_version_label`]; `None` for a string it never produces.
pub(crate) fn plugin_version_from_label(label: &str) -> Option<PluginVersion> {
    match label {
        "2.5.0" => Some(PluginVersion::Ranger2_5_0),
        "2.8.0" => Some(PluginVersion::Ranger2_8_0),
        "2.9.0" => Some(PluginVersion::Ranger2_9_0),
        _ => None,
    }
}

/// One profile's infrastructure inputs (governance SP2a, D2), with the
/// service name and type the Ranger renderer needs.
pub(crate) struct LoadedInfra {
    pub service_name: String,
    pub service_type: String,
    pub inputs: egret_perm_ir::lower::infra::InfraInputs,
}

/// Whether a bound engine of `engine_type` and `origin` writes the StarRocks
/// query log: a StarRocks engine egret provisioned (its AuditLoader runs as
/// `QUERY_LOG_LOADER_PRINCIPAL`). An external engine is not egret's to guard.
pub(crate) fn writes_query_log(engine_type: &str, origin: &str) -> bool {
    engine_type == "starrocks" && origin != egret_core::engine_delegation::ORIGIN_EXTERNAL
}

/// Read what the infrastructure statements of profile `service_id` depend on,
/// on the caller's connection. `None` when the profile is not one this
/// compiler targets (see [`crate::trigger::engine_type_for`]).
///
/// The engine admins are read only when some bound engine qualifies: an
/// admin's missing username is minted on that read under
/// [`MissingUsername::Mint`](crate::members::MissingUsername::Mint) (left out
/// under `Omit`, the OPA snapshot), and a profile that gets no `egret-admin`
/// row must not mint one.
pub(crate) async fn load_infra_inputs(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
    missing: crate::members::MissingUsername,
) -> Result<Option<LoadedInfra>, CompileError> {
    use egret_core::engine_delegation::{delegator_impersonates, qualifies_for_infra};

    let svc: Option<(String, String)> =
        sqlx::query_as("SELECT name, type FROM policy.authz_services WHERE id = $1")
            .bind(service_id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(CompileError::from_db)?;
    let Some((service_name, service_type)) = svc else {
        return Ok(None);
    };
    let Some(engine) = crate::trigger::engine_type_for(&service_type) else {
        return Ok(None);
    };
    let engines: Vec<(String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT type, origin, spec FROM engine_mgmt.engines WHERE authz_service_id = $1",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    let qualifying: Vec<&(String, String, serde_json::Value)> = engines
        .iter()
        .filter(|(t, origin, spec)| qualifies_for_infra(t, origin, spec))
        .collect();
    let delegating = !qualifying.is_empty();
    let delegator_impersonates = qualifying
        .iter()
        .any(|(t, _, spec)| delegator_impersonates(t, spec));
    let query_log = engines
        .iter()
        .any(|(t, origin, _)| writes_query_log(t, origin));
    let admins = if delegating {
        crate::members::engine_admin_usernames_via(conn, missing).await?
    } else {
        Vec::new()
    };
    Ok(Some(LoadedInfra {
        service_name,
        service_type,
        inputs: egret_perm_ir::lower::infra::InfraInputs {
            engine,
            admins,
            delegating,
            delegator_impersonates,
            query_log,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_provisioned_starrocks_engine_writes_the_query_log() {
        assert!(writes_query_log("starrocks", "provisioned"));
        assert!(!writes_query_log("starrocks", "external"));
        assert!(!writes_query_log("trino", "provisioned"));
    }

    #[test]
    fn every_plugin_version_round_trips_through_its_label() {
        for v in [
            PluginVersion::Ranger2_5_0,
            PluginVersion::Ranger2_8_0,
            PluginVersion::Ranger2_9_0,
        ] {
            assert_eq!(plugin_version_from_label(plugin_version_label(v)), Some(v));
        }
    }
}
