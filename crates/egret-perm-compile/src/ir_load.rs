//! Every read the lowering of one enforcement point needs, in one place
//! (governance sub-project 1, Task 4).
//!
//! [`load_ep`] is the head `compile_desired` used to carry inline, moved here
//! verbatim: the same statements, on the caller's connection, in the same
//! order. It returns the profile as an [`EpInput`] plus the store rows the
//! lowering consumes, already expanded by
//! [`crate::members::expand_enumerated_principals`].
//!
//! The one read [`load_ep`] does NOT make is the bound engines' plugin
//! versions ([`bound_engines`]). Today only a StarRocks profile with at least
//! one classified column reads them, and only after its clearances and
//! enabled hand-written policies; reading them here would add a statement to
//! every other StarRocks compile and move it ahead of those reads. So the
//! [`EpInput`] leaves the loader with `engines` empty and `default_version`
//! `Err(NOT_READ)`, and the clearance-mask fold fills both at the old point.

use std::collections::{BTreeMap, HashMap};

use egret_perm::Selector;
use egret_perm::{
    list_engine_grants_for_profile_via, list_grants_via, list_rules_via, CompileError, EngineGrant,
    EngineType, Grant, Rule,
};
use egret_perm_ir::audience::Directory;
use egret_perm_ir::ep::{BoundEngine, EpInput, EpKind, Mechanism};
use egret_perm_ir::lower::rules::{needs_tagged_tables, RuleInputs};
use egret_perm_ir::statement::{Lowering, ReasonCode, Source};
use egret_policy::{
    catalogs_for_service_via, get_service_by_id_via, service_id_by_name_via,
    tag_assignments_for_service_via, tag_name_for_key_via,
};
use egret_ranger_eval::eval::PluginVersion;
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
    /// Who each workspace, role and group stood for at expansion. Not read
    /// yet: the lowering that consumes it lands in a later task of the
    /// sub-project.
    #[allow(dead_code)]
    pub directory: Directory,
}

/// Read profile `service_id` (every read against `conn`, so a caller running
/// this inside its own open transaction sees its own uncommitted writes).
/// `None` means `service_id` names something this compiler doesn't target
/// (see [`crate::trigger::engine_type_for`]).
pub(crate) async fn load_ep(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
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

    let mut grants = list_grants_via(conn)
        .await
        .map_err(CompileError::from_perm)?;
    let mut rules = list_rules_via(conn)
        .await
        .map_err(CompileError::from_perm)?;
    // D3: turn every `workspace_members` principal — and, since SP4c Task 2b,
    // every `role` principal, and since J-F3 Task 1b every non-`public` `group`
    // principal — into the people it stands for BEFORE anything is compiled,
    // so the pure compilers never see one. See `crate::members` for what a
    // member is, what it costs, and why a role name cannot be left for Ranger
    // to resolve.
    // F11: the engine grants of every engine bound to this profile, expanded
    // in the same call so a person named by a grant and an engine grant
    // resolves to one name.
    let mut engine_grants = list_engine_grants_for_profile_via(conn, service_id)
        .await
        .map_err(CompileError::from_perm)?;
    let directory = crate::members::expand_enumerated_principals(
        conn,
        &mut grants,
        &mut rules,
        &mut engine_grants,
    )
    .await?;

    let ep = EpInput {
        profile_id: service_id,
        profile_name: svc.name.clone(),
        kind: EpKind {
            engine,
            mechanism: Mechanism::Ranger,
        },
        engines: Vec::new(),
        default_version: Err(NOT_READ.to_string()),
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

/// Fill `ep.engines` (each engine bound to the profile, by name, with the
/// plugin version it runs or why that is unknown) and `ep.default_version`
/// (the plugin of egret's default image for the engine type). One SELECT on
/// `conn`, the one `clearance_mask::fold_plugin_versions` used to make.
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

#[cfg(test)]
mod tests {
    use super::*;

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
