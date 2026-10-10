//! SP4d-3 Task 5: classify every policy in one Ranger service profile — who
//! owns it, whether it can be migrated into the `perm` model, and if not, every
//! reason why, each rendered as a sentence an operator can act on.
//!
//! The decisions are made by the closed grammar ([`crate::grammar`]); this
//! module supplies the three inputs the grammar cannot know from a body alone,
//! and renders its [`Blocker`]s. Getting those inputs wrong silently
//! reintroduces defects the grammar was built to refuse, so each is derived
//! from the database here, never assumed:
//!
//! - **[`ServiceKind`] and [`EngineType`] come from the service row's `type`**
//!   ([`service_target`]). `trino` / `starrocks` are resource services of
//!   that engine; `tag` is a tag service, and its engine is Trino because only
//!   a `trino` profile may carry a `tagService` (`egret_policy::store::
//!   refuse_tag_service_on_non_trino_profile`, and migration 0176's `CHECK`).
//!   A hard-coded `Resource` would let a tag body stored in a resource service
//!   — where it enforces nothing — migrate into the tag service and start
//!   enforcing.
//! - **The catalog lookup is EXACT and scoped to the catalogs this profile's
//!   engines reach** (`catalogs_for_service_via`, the compiler's own
//!   reachability join). `registry.catalogs.name` is globally `UNIQUE`, so an
//!   unscoped lookup could not be ambiguous — but it would resolve a catalog
//!   this profile does not serve. A policy on such a catalog enforces nothing
//!   here today; migrated, the grant would compile into every OTHER profile
//!   whose engine does serve that catalog, widening access there. Scoped, it is
//!   `UnregisteredCatalog`. For a tag service the scope is the union over the
//!   resource profiles that name it as their `tagService` (only path bodies
//!   use the lookup, and those are refused in a tag service anyway; the union
//!   just keeps a second, misleading "unregistered catalog" sentence out of
//!   that refusal).
//! - **The tag lookup is the real `tags.key → tags.name`**
//!   (`tag_names_for_keys_via`), never the key echoed back as the name, which
//!   would hide `TagNameDiffersFromKey`.
//!
//! **`enabled` is the column, stamped into the body before the grammar reads
//! it.** The download serves only `enabled = true` rows and stamps `isEnabled`
//! from that column (`PolicyStore::enabled_policies_for_service`), so the
//! body's own `isEnabled` decides nothing the engine sees. Stamping the column
//! makes the grammar's [`Blocker::Disabled`] fire exactly when the engine
//! enforces nothing — and a switched-off policy is NOT a migration candidate:
//! migrating it would turn an effect nobody has today into live grants. (A
//! policy already migrated is switched off by Task 7's write path, so this is
//! also what keeps a migrated original from being offered a second time.)

use std::collections::HashMap;

use egret_perm::EngineType;
use egret_policy::{
    catalogs_for_service_via, get_service_by_id_via, tag_names_for_keys_via, PolicyStoreError,
    TAG_DEF_NAME,
};
use serde_json::Value;
use uuid::Uuid;

use crate::grammar::{
    grants_from_access_policy, rules_from_mask_or_filter_policy, Blocker, ServiceKind,
};

/// One policy of the profile, as the classifier reports it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct ClassifiedPolicy {
    pub id: i64,
    pub name: String,
    /// `compiler` | `infra` | `operator` — `policy.authz_policies.managed_by`.
    pub managed_by: String,
    /// The `enabled` COLUMN — what decides whether the engine downloads it.
    pub enabled: bool,
    pub migratable: bool,
    /// Every reason the policy cannot be migrated, as operator-readable
    /// sentences naming the offending value. Empty iff `migratable`.
    pub blockers: Vec<String>,
    /// For a migratable policy, the model objects it would become.
    pub proposed: Option<ProposedObjects>,
}

/// How many model objects a migratable policy becomes. No `clearances`:
/// clearance migration is out of scope ([`Blocker::ClearanceNotExpressible`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct ProposedObjects {
    pub grants: usize,
    pub rules: usize,
}

/// One `policy.authz_policies` row, as the classifier reads it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PolicyRecord {
    pub id: i64,
    pub name: String,
    pub managed_by: String,
    pub enabled: bool,
    pub policy_jsonb: Value,
}

/// Which grammar inputs a service row's `type` implies, or `None` for a type
/// egret's permission model does not compile to. See the module doc.
pub fn service_target(service_type: &str) -> Option<(ServiceKind, EngineType)> {
    if service_type == TAG_DEF_NAME {
        return Some((ServiceKind::Tag, EngineType::Trino));
    }
    crate::trigger::engine_type_for(service_type).map(|e| (ServiceKind::Resource, e))
}

/// Classify one policy. Pure: every database fact arrives as an argument.
/// `target` is [`service_target`] of the profile; `Err` carries the profile's
/// type when it is not one egret compiles to.
pub fn classify_policy(
    rec: &PolicyRecord,
    target: Result<(ServiceKind, EngineType), &str>,
    catalog_ids: &dyn Fn(&str) -> Option<Uuid>,
    tag_names: &dyn Fn(&str) -> Option<String>,
) -> ClassifiedPolicy {
    let refuse = |blockers: Vec<String>| ClassifiedPolicy {
        id: rec.id,
        name: rec.name.clone(),
        managed_by: rec.managed_by.clone(),
        enabled: rec.enabled,
        migratable: false,
        blockers,
        proposed: None,
    };

    match rec.managed_by.as_str() {
        "operator" => {}
        "compiler" => {
            return refuse(vec![
                "egret's permission compiler wrote this policy from a grant or rule in the \
                 permission model, so it is already managed there and is not a migration \
                 candidate. Change the grant or rule instead; the compiler rewrites this policy."
                    .to_string(),
            ])
        }
        "infra" => {
            return refuse(vec![
                "egret's own plumbing wrote this policy (engine delegation, admin access or \
                 query limits), so it is not a migration candidate and cannot be edited here. \
                 Removing or changing it would break how egret itself reaches the engine."
                    .to_string(),
            ])
        }
        other => {
            return refuse(vec![format!(
                "This policy's owner is recorded as `{other}`, which egret does not recognise \
                 (expected compiler, infra or operator), so it is not offered for migration."
            )])
        }
    }

    let (service, engine) = match target {
        Ok(t) => t,
        Err(service_type) => {
            return refuse(vec![format!(
                "This profile's type is `{service_type}`, which egret's permission model does \
                 not compile to (it compiles to trino and starrocks profiles and their tag \
                 services), so none of its policies can be migrated."
            )])
        }
    };

    // The engine reads `isEnabled` from the column, not from the stored body.
    let mut body = rec.policy_jsonb.clone();
    if let Value::Object(m) = &mut body {
        m.insert("isEnabled".into(), Value::Bool(rec.enabled));
    }

    // Masking (1) and row filter (2) go to the rule grammar; everything else —
    // access (0, the Ranger default when absent), an unknown type, or a type
    // that is not an integer — to the access grammar, which names the problem
    // (`NotAnAccessPolicy`, `Unreadable`) rather than dropping the row.
    let is_rule = matches!(
        body.get("policyType").and_then(Value::as_i64),
        Some(1) | Some(2)
    );
    let outcome = if is_rule {
        rules_from_mask_or_filter_policy(&body, engine, catalog_ids, tag_names, service).map(|c| {
            ProposedObjects {
                grants: 0,
                rules: c.rules.len(),
            }
        })
    } else {
        grants_from_access_policy(&body, engine, catalog_ids, service).map(|c| ProposedObjects {
            grants: c.grants.len(),
            rules: 0,
        })
    };

    match outcome {
        Ok(proposed) => ClassifiedPolicy {
            id: rec.id,
            name: rec.name.clone(),
            managed_by: rec.managed_by.clone(),
            enabled: rec.enabled,
            migratable: true,
            blockers: Vec::new(),
            proposed: Some(proposed),
        },
        Err(blockers) => refuse(blockers.iter().map(render_blocker).collect()),
    }
}

fn quoted(values: &[String]) -> String {
    values
        .iter()
        .map(|v| format!("`{v}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One [`Blocker`] as a sentence an operator can act on, naming the offending
/// value. EXHAUSTIVE, with no catch-all arm, so a new variant fails to compile
/// here rather than reaching the page as an enum name. Item indices are shown
/// counting from 1, the way the editor lists items.
pub fn render_blocker(b: &Blocker) -> String {
    match b {
        Blocker::UnknownAccessType { access_type } => format!(
            "Access type `{access_type}` has no equivalent in egret's permission model on this \
             engine: egret grants only read and write, each a fixed set of Ranger access types, \
             and spelling is matched exactly (`SELECT` is not `select`). Remove `{access_type}` \
             from the policy, or keep the policy hand-written."
        ),
        Blocker::PartialActionExpansion { action, missing } => {
            let action = action.as_str();
            let missing = quoted(missing);
            format!(
                "The policy grants part of egret's `{action}` permission but not all of it: it \
                 lacks {missing}. egret's `{action}` always grants these together, so migrating \
                 would ADD {missing} — access nobody asked for. Add {missing} to the policy if \
                 that is intended, or keep the policy hand-written."
            )
        }
        Blocker::UnregisteredCatalog { catalog } => format!(
            "Catalog `{catalog}` is not a registered catalog that this profile's engines serve \
             (names are matched exactly, and `*` names no catalog). A migrated grant must point \
             at a catalog egret registers and this profile serves; a policy on any other catalog \
             enforces nothing here, and migrating it would grant on whichever profile does serve \
             it. Name a served catalog exactly, attach the catalog to this profile's engine, or \
             keep the policy hand-written."
        ),
        Blocker::ResourceFlag { level, flag } => format!(
            "Resource level `{level}` sets `{flag}`; egret's permissions never exclude or recurse, \
             so this level has no equivalent. Clear `{flag}` on `{level}`, or keep the policy \
             hand-written."
        ),
        Blocker::TooManyExceptions { count } => format!(
            "The policy has {count} exception items in one bucket; egret can express at most one \
             exception for the allow items and one for the deny items (a grant's excluded \
             principals). Merge the exceptions into one, or keep the policy hand-written."
        ),
        Blocker::ExceptionAccessMismatch => "An exception item's access types differ from its \
             main item's. egret's excluded principals are excluded from exactly the access the \
             grant gives, so an exception covering a different set cannot be expressed. Make the \
             exception's access types match the main item's."
            .to_string(),
        Blocker::ValiditySchedules => "The policy has validity schedules (it is active only at \
             set times); egret's permissions are always active. Remove the schedule, or keep the \
             policy hand-written."
            .to_string(),
        Blocker::Conditions => "The policy, or one of its items, has conditions; egret's \
             permission model has no conditions. Remove them, or keep the policy hand-written."
            .to_string(),
        Blocker::NonDefaultPriority { priority } => format!(
            "The policy has priority {priority}; every policy egret writes has the normal \
             priority (0), so the override would be lost. Set the priority to normal, or keep \
             the policy hand-written."
        ),
        Blocker::EmptyLevel { level } => format!(
            "Resource level `{level}` has no value, or an empty-string value, which names \
             nothing. Name the resource, or use `*` for every value."
        ),
        Blocker::WildcardOverNamedLevel { level } => format!(
            "Level `{level}` is `*` while a level beneath it is named; egret refuses a grant \
             that names something under a wildcard (it would mean that name in every \
             `{level}`). Name `{level}` explicitly, or make every level beneath it `*` too."
        ),
        Blocker::UnknownTag { key } => format!(
            "The policy names tag `{key}`, and egret has no tag with that key. The migration \
             never creates tags: create a tag whose key and name are both `{key}`, or keep the \
             policy hand-written."
        ),
        Blocker::NothingToMigrate => "The policy has no allow items and no deny items, so it \
             grants and denies nothing; there is nothing to migrate."
            .to_string(),
        Blocker::Unreadable { error } => format!(
            "The policy body cannot be read as a Ranger policy ({error}). Fix the body in the \
             policy editor first."
        ),
        Blocker::NotAnAccessPolicy { policy_type } => format!(
            "The policy's type is {policy_type}; egret recognises access (0), masking (1) and \
             row-filter (2) policies only, so it cannot say what this one does."
        ),
        Blocker::Disabled => "This policy is switched off, so the engine does not download it \
             and it enforces nothing today. Migrating it would create grants or rules that \
             enforce immediately (a policy that has already been migrated is switched off the \
             same way). Switch it on first if it is meant to be migrated."
            .to_string(),
        Blocker::DenyAllElse => "The policy denies everything its items do not allow \
             (`isDenyAllElse`); egret has no equivalent. Clear 'deny all else', or keep the \
             policy hand-written."
            .to_string(),
        Blocker::AdditionalResources => "The policy lists additional resources (more than one \
             resource set); an egret grant or rule addresses one. Split it into one policy per \
             resource set."
            .to_string(),
        Blocker::SecurityZone { zone } => format!(
            "The policy belongs to security zone `{zone}`; egret has no security zones. Keep \
             the policy hand-written."
        ),
        Blocker::MissingLevel { level } => format!(
            "The policy names no `{level}` level. egret's policy editor drops a level left empty \
             when it saves, so this is how most editor-saved policies look. Ranger does not read \
             an absent level as `*`: a policy that stops above `{level}` does not match \
             `{level}`-level requests the way every egret permission does, so it cannot be \
             migrated as it stands. Setting `{level}` to `*` in the editor makes it expressible \
             — but that also extends the policy to every `{level}` under what it names, so do \
             it only if that is what the policy is meant to grant."
        ),
        Blocker::UnknownLevel { level } => format!(
            "Resource `{level}` is not a level egret addresses for this engine and policy kind \
             (catalog, schema or database, table and column; a row filter stops at table; a tag \
             policy has only `tag`). Remove `{level}`, or keep the policy hand-written."
        ),
        Blocker::MultipleValues { level, count } => format!(
            "Level `{level}` lists {count} values; below the catalog egret names one value per \
             level (several catalogs are fine). Split it into one policy per `{level}` value."
        ),
        Blocker::ResourceExpression { level, value } => format!(
            "Value `{value}` at level `{level}` is a Ranger runtime expression, resolved per \
             request; egret matches literal names only. Replace it with the names it stands \
             for, or keep the policy hand-written."
        ),
        Blocker::RolePrincipal { role } => format!(
            "The policy names Ranger role `{role}`. egret never writes Ranger roles, and on \
             egret's engines that bucket matches nobody today, so migrating would make `{role}` \
             start matching people. Name the users or groups directly instead."
        ),
        Blocker::UserExpression { user } => format!(
            "User `{user}` is a Ranger runtime expression (such as {{USER}} or {{OWNER}}), \
             resolved per request; egret names users literally. Name the users, or keep the \
             policy hand-written."
        ),
        Blocker::UnusableUserRef { user } => format!(
            "User `{user}` cannot be an egret principal: egret refuses an empty user name and \
             one containing `@`. Use the engine user name, or keep the policy hand-written."
        ),
        Blocker::NoPrincipals => "An item or exception names no user, group or role, so it \
             applies to nobody. Name a principal, or delete the item."
            .to_string(),
        Blocker::EmptyAccesses => "An item or exception has no access types. Add access \
             types, or delete the item."
            .to_string(),
        Blocker::AccessNotAllowed { access_type } => format!(
            "Access type `{access_type}` is listed as not allowed (`isAllowed: false`); egret has \
             no equivalent for a listed-but-not-allowed access. Remove `{access_type}`, or \
             allow it."
        ),
        Blocker::DelegateAdmin => "An item grants delegated admin (the right to administer \
             this policy); egret has no equivalent. Clear delegate admin, or keep the policy \
             hand-written."
            .to_string(),
        Blocker::OrphanException => "The policy has an exception with no main item in the \
             same allow or deny set to make an exception to. Delete the exception, or add the \
             item it belongs to."
            .to_string(),
        Blocker::ForeignItems { bucket } => format!(
            "The policy carries `{bucket}` items, which do not belong to a policy of its type. \
             Remove them, or keep the policy hand-written."
        ),
        Blocker::UnrecognisedField { field } => format!(
            "Field `{field}` is one the migration has not ruled on, so it cannot vouch for what \
             the field does. Remove `{field}`, or keep the policy hand-written."
        ),
        Blocker::NotAMaskOrRowFilter { policy_type } => format!(
            "The policy's type is {policy_type}; egret recognises access (0), masking (1) and \
             row-filter (2) policies only, so it cannot say what this one does."
        ),
        Blocker::ClearanceNotExpressible => "This is a tag access policy. The only tag access \
             policy egret has is a clearance, whose cleared people are the members of egret \
             roles, worked out every time the policy is compiled; a hand-written list of users \
             cannot be turned back into roles, and a tag allow has no equivalent at all. \
             Migrating clearances is out of scope here: keep the policy hand-written, or recreate \
             it as a classification clearance in egret."
            .to_string(),
        Blocker::MaskWithoutType { index } => format!(
            "Mask item {} has no mask type. Ranger reads that as no mask, and in a tag service \
             such an item stops the engine's plugin from loading any policy. Choose a mask type.",
            index + 1
        ),
        Blocker::UnsupportedMaskType { mask_type } => format!(
            "Mask type `{mask_type}` is not one this engine supports; the engine ignores it and \
             serves the column unmasked. Choose a mask type the engine supports."
        ),
        Blocker::ValueExprWithoutCustom { mask_type } => format!(
            "A `{mask_type}` mask carries a custom expression; egret writes an expression only \
             for CUSTOM masks, so it would be dropped. Remove the expression, or change the type \
             to CUSTOM."
        ),
        Blocker::MaskExpression { expr } => format!(
            "Mask expression `{expr}` contains a Ranger request-time expression (`${{{{…}}}}`), \
             resolved per request; egret writes literal mask expressions only."
        ),
        Blocker::FilterExpression { expr } => format!(
            "Row filter `{expr}` contains something Ranger rewrites or resolves per request \
             (`${{{{…}}}}`, a `{{TOKEN}}` or `[[…]]`). egret refuses it: a filter read wrongly \
             leaves rows unfiltered. Write the filter literally, or keep the policy hand-written."
        ),
        Blocker::MissingFilterExpr { index } => format!(
            "Row-filter item {} has no filter expression, which Ranger reads as 'this item \
             decides nothing' — not the same as an empty filter. Set a filter expression, or \
             delete the item.",
            index + 1
        ),
        Blocker::TooManyRuleItems { count } => format!(
            "The policy has {count} items; an egret rule has at most two — an optional exemption \
             first, then one item for everyone. Reduce it to that shape, or keep the policy \
             hand-written."
        ),
        Blocker::NotForEveryone { index } => format!(
            "Item {} is in the 'everyone' position (the last item) but does not name exactly the \
             `public` group and nobody else. An egret rule applies to everyone; a mask or filter \
             for named principals only cannot be expressed.",
            index + 1
        ),
        Blocker::NotAnExemption { index } => format!(
            "Item {} comes before the 'everyone' item but does not lift the control (MASK_NONE \
             with no value, or an empty filter); egret has no per-principal masks or filters.",
            index + 1
        ),
        Blocker::PublicExemption => "The exemption item names the `public` group, which would \
             exempt everyone; egret refuses it. Name the exempt users or groups instead."
            .to_string(),
        Blocker::TagSelectorOnStarRocks => "The policy is addressed by tag on a StarRocks \
             profile. StarRocks has no tag service; egret compiles a tag rule there into one \
             policy per tagged column, a different artifact from this one, so it is not \
             migrated."
            .to_string(),
        Blocker::CompilerRefuses { error } => {
            format!("egret's compiler would refuse the rule this policy becomes: {error}")
        }
        Blocker::TagNameDiffersFromKey { key, name } => format!(
            "The policy names tag `{key}`; egret's tag with key `{key}` is named `{name}`, and \
             Ranger matches tag policies by name — so the migrated rule would cover the tag \
             named `{name}`, not the one this policy covers. Rename the tag so its key and name \
             agree, or rewrite the policy."
        ),
        Blocker::TagBodyInResourceService => "The policy is addressed by tag but stored in a \
             resource profile, where Ranger matches nothing by tag, so it enforces nothing \
             today. Migrated, egret would file it in the tag service and it would start \
             enforcing. Move it to the tag service if it is meant to enforce, or delete it."
            .to_string(),
        Blocker::PathBodyInTagService => "The policy is addressed by catalog, schema and table \
             but stored in a tag service, where it matches nothing, so it enforces nothing \
             today. Migrated, egret would file it in the resource profile and it would start \
             enforcing. Move it to the resource profile if it is meant to enforce, or delete \
             it."
        .to_string(),
        Blocker::Internal { detail } => format!(
            "egret could not classify this policy: an internal check failed ({detail}). This \
             is a defect in egret, not in the policy — please report it. The policy is not \
             migrated."
        ),
    }
}

/// Classify every policy of `service_id`, ordered by `id`. `NotFound` for an
/// unknown profile.
pub async fn classify_service(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<ClassifiedPolicy>, PolicyStoreError> {
    let svc = get_service_by_id_via(conn, service_id).await?;
    let target = service_target(&svc.r#type);

    // The catalog scope: this profile's engines, or — for a tag service — the
    // engines of every resource profile that names it (module doc).
    let mut scope_profiles = vec![service_id];
    if matches!(target, Some((ServiceKind::Tag, _))) {
        let linked: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM policy.authz_services WHERE config_jsonb ->> 'tagService' = $1",
        )
        .bind(&svc.name)
        .fetch_all(&mut *conn)
        .await?;
        scope_profiles.extend(linked);
    }
    let mut catalogs: HashMap<String, Uuid> = HashMap::new();
    for id in scope_profiles {
        for c in catalogs_for_service_via(conn, id).await? {
            catalogs.insert(c.name, c.id);
        }
    }

    let rows: Vec<PolicyRecord> = sqlx::query_as(
        "SELECT id, name, managed_by, enabled, policy_jsonb FROM policy.authz_policies \
         WHERE service_id = $1 ORDER BY id",
    )
    .bind(service_id)
    .fetch_all(&mut *conn)
    .await?;

    // Every string a tag-addressed body names, resolved in one round trip.
    let mut keys: Vec<String> = rows
        .iter()
        .filter_map(|r| r.policy_jsonb.pointer("/resources/tag/values"))
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    keys.sort();
    keys.dedup();
    let tags: HashMap<String, String> = tag_names_for_keys_via(conn, &keys)
        .await?
        .into_iter()
        .collect();

    let catalog_ids = |name: &str| catalogs.get(name).copied();
    let tag_names = |key: &str| tags.get(key).cloned();
    let target = target.ok_or(svc.r#type.as_str());
    Ok(rows
        .iter()
        .map(|r| classify_policy(r, target, &catalog_ids, &tag_names))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_type_decides_the_kind_and_engine() {
        assert_eq!(
            service_target("trino"),
            Some((ServiceKind::Resource, EngineType::Trino))
        );
        assert_eq!(
            service_target("starrocks"),
            Some((ServiceKind::Resource, EngineType::StarRocks))
        );
        assert_eq!(
            service_target("tag"),
            Some((ServiceKind::Tag, EngineType::Trino))
        );
        assert_eq!(service_target("hive"), None);
    }
}
