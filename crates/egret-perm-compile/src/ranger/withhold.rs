//! Renders a clearance's [`Statement`]s (the `withhold` effect and its
//! materialized-view guard) into Ranger access-policy bodies (governance
//! sub-project 1, Task 7). Every decision — which gates lower, what each
//! skips and why, the ceiling, the per-column and whole-table expansion, the
//! materialized-view guard's exceptions — is `egret_perm_ir::lower::withhold`'s;
//! this module renders shape only. Read that module's doc for the group-only
//! skip, the two engine shapes, the ceilings and the view limit.
//!
//! # The defect this closes
//!
//! A classification's clearance was enforced when someone CLICKED the object
//! and not enforced when they QUERIED the column. Cedar compiles a tag's
//! `required_roles`/`required_groups` into a `forbid` on control-plane object
//! actions (`egret_auth::pdp`); the permission model gates the data-plane query
//! through Ranger, and SP4a's rules read only a tag's `key`. So a user Cedar
//! forbids from opening a tagged dataset could read the same data through SQL
//! Lab or a notebook, and effective access was the UNION of whichever path the
//! user took rather than the intersection.
//!
//! # THIS AMENDS SP4a's D2 — "tags drive rules only, never access"
//!
//! **Stated here, at the site, so a future reader who finds a compiled tag
//! ACCESS policy does not conclude it is a bug.** D2 held that egret must never
//! emit one, because Ranger evaluates tag policies FIRST and short-circuits, so
//! a tag allow would end evaluation ahead of every compiled resource policy
//! including a compiled deny. A clearance cannot be expressed as either shape
//! D2 permits: a mask and a row filter both still ANSWER the query, and a
//! classification exists to WITHHOLD the data.
//!
//! The amendment is therefore narrow and one-directional. This module emits a
//! tag access policy that is **only ever a DENY** — never an allow, which is
//! the direction that could short-circuit ahead of a compiled deny grant — and
//! the tag-first short-circuit is exactly what makes a clearance deny WIN over
//! any resource grant, which is what a clearance has to do. See
//! `egret_policy::admin`'s re-export of `tag_access_to_ranger_json`, the other
//! half of this amendment.
//!
//! # Why the cleared principals are ENUMERATED, and why nothing names the role
//!
//! D0 (`docs/en/plans/2026-09-28-sp4c-d0-result.md`) measured, by mechanism and
//! corroborated by the live coordinator's own log, that **a Ranger policy item
//! naming a role matches nobody** on the Trino path egret provisions, and — the
//! part that decided the shape — that **an unmatched item is not neutral.** A
//! policy item matches only by set INTERSECTION and the role clause
//! short-circuits on an empty set, so an unresolvable role in `denyExceptions`
//! excepts NOBODY and the deny stands for EVERYONE. The obvious implementation,
//! *deny `{public}` except role `analyst`*, would have refused the tagged
//! column to every user INCLUDING the cleared ones.
//!
//! So: the deny item names `{public}` (measured to still match everybody with
//! no group provider — `RangerDefaultPolicyItemEvaluator` needs only a non-null
//! `userGroups`, and `setUserGroups` normalises `null` to an empty set), and
//! `denyExceptions` carries the clearing roles' MEMBERS, enumerated at compile
//! time as bare usernames read from `users.username`, a missing one minted
//! ([`crate::members::role_usernames`], 2026-10-18 design R5, R6: a holder
//! with no username is excepted at once, as Cedar's `classification_forbid`
//! clears them by role, instead of after some later recompile). The Ranger
//! `roles` bucket is not used
//! at all. Deny and exceptions ARE evaluated despite egret's service-defs
//! carrying `"options": null` — the hidden
//! `ranger.servicedef.enableDenyAndExceptionsInPolicies` default is `true`
//! (`ServiceDefUtil.getOption_enableDenyAndExceptionsInPolicies`), the second
//! adjacent fact D0 checked because it alone could have made this shape inert.
//!
//! Enumeration is why D3's third recompile trigger (role membership changing)
//! is mandatory rather than conditional — SP4c Task 3 owns it, for this module
//! and for the role-grant expansion Task 2b adds, which is the same expansion.
//!
//! # Names
//!
//! - A classification statement (Trino): one tag policy,
//!   `perm-clearance-{tag}`, filed under the TAG service.
//! - A per-column or whole-table withhold (StarRocks):
//!   `perm-clearance-{tag}-{catalog}.{schema}.{table}.{column}` ([`starrocks_deny`]),
//!   the column [`WHOLE_TABLE_COLUMN`] for a whole table.
//! - The materialized-view guard: `perm-clearance-mv` ([`mv_deny_input`]).

use std::collections::BTreeMap;

use serde_json::Value;
use uuid::Uuid;

use egret_perm::{CompileError, EngineType};
use egret_perm_ir::audience::MemberName;
use egret_perm_ir::ep::EpInput;
use egret_perm_ir::lower::rules::TaggedColumn;
use egret_perm_ir::lower::withhold::MATERIALIZED_VIEW_CATALOG;
use egret_perm_ir::statement::{Derivation, Effect, Level, Privilege, Source, Statement, Target};
use egret_policy::admin::{
    access_to_ranger_json, tag_access_to_ranger_json, AccessItemInput, AccessPolicyInput,
    ResourceLevel,
};
use egret_policy::COMPILER_CLEARANCE_PREFIX;

use super::grants::{level, member_buckets, read_write_accesses, PolicyTarget};
use super::rules::{column_resources, stamp_service_type, PUBLIC_GROUP};

/// Render one clearance statement for the profile `service` (its own Ranger
/// service name) on `ep`, whose TAG service is `tag_service` (`None` when the
/// profile has no usable one). Returns the policy name — the desired-set key —
/// and the body.
///
/// - [`Derivation::AsDeclared`] on a [`Target::Classification`]: one tag deny
///   (`tag_access_to_ranger_json`) filed under `tag_service`. `tagify`
///   replaces the resources map with the single `tag` resource, exactly as it
///   does for a compiled tag mask.
/// - [`Derivation::ExpandedFromClassification`] on a path naming a column, or
///   [`Derivation::WholeTable`] on a path naming none: [`starrocks_deny`].
/// - [`Derivation::MaterializedViewGuard`]: [`mv_deny_input`] on the path's
///   catalog, every database.
///
/// Anything else is refused with `CompileError::Db`: no lowering produces it.
pub(crate) fn render(
    s: &Statement,
    ep: &EpInput,
    service: &str,
    tag_service: Option<&str>,
) -> Result<(String, Value), CompileError> {
    let Source::Withhold {
        classification_id, ..
    } = &s.source
    else {
        return Err(CompileError::Db(format!(
            "ranger::withhold::render handed a non-withhold statement ({:?})",
            s.source
        )));
    };
    let engine = ep.kind.engine;
    if !matches!(s.audience.include.as_slice(), [m] if m.name == MemberName::Everyone) {
        return Err(CompileError::Db(format!(
            "ranger::withhold::render handed an audience that is not everyone ({:?})",
            s.audience.include
        )));
    }
    let (cleared, groups) = member_buckets(&s.audience.except)?;
    if !groups.is_empty() {
        return Err(CompileError::Db(format!(
            "ranger::withhold::render handed group exceptions ({groups:?})"
        )));
    }
    match (&s.effect, &s.derivation, &s.target) {
        (
            Effect::Withhold { privileges },
            Derivation::AsDeclared,
            Target::Classification { name },
        ) => {
            let Some(tag_service) = tag_service else {
                return Err(CompileError::Db(format!(
                    "ranger::withhold::render handed a classification withhold for a profile \
                     with no tag service (tag {name:?})"
                )));
            };
            let accesses = read_write_accesses(privileges, engine, "ranger::withhold::render")?;
            let policy = format!("{COMPILER_CLEARANCE_PREFIX}{classification_id}");
            let input = clearance_input(
                policy.clone(),
                tag_service.to_string(),
                BTreeMap::new(),
                &accesses,
                cleared,
            );
            let body = tag_access_to_ranger_json(&input, name, tag_service);
            Ok((policy, body))
        }
        (
            Effect::Withhold { privileges },
            d @ (Derivation::ExpandedFromClassification | Derivation::WholeTable),
            Target::Path(path),
        ) => {
            let named = |l: &Level| match l {
                Level::Named(v) => Some(v.clone()),
                Level::Unnamed => None,
            };
            let column = match (d, named(&path.column)) {
                (Derivation::WholeTable, None) => Some(WHOLE_TABLE_COLUMN.to_string()),
                (Derivation::ExpandedFromClassification, Some(c)) => Some(c),
                _ => None,
            };
            let (Some(schema), Some(table), Some(column)) =
                (named(&path.schema), named(&path.table), column)
            else {
                return Err(CompileError::Db(format!(
                    "ranger::withhold::render handed a {d:?} withhold on {path:?}"
                )));
            };
            let col = TaggedColumn {
                catalog: path.catalog.clone(),
                schema,
                table,
                column,
            };
            let accesses = read_write_accesses(privileges, engine, "ranger::withhold::render")?;
            Ok(starrocks_deny(
                *classification_id,
                service,
                &col,
                &accesses,
                cleared,
            ))
        }
        (Effect::Deny { privileges }, Derivation::MaterializedViewGuard, Target::Path(path))
            if privileges.as_slice() == [Privilege::CreateMaterializedView] =>
        {
            let (name, input) = mv_deny_input(&path.catalog, cleared, service);
            Ok((
                name,
                stamp_service_type(access_to_ranger_json(&input), engine),
            ))
        }
        (e, d, t) => Err(CompileError::Db(format!(
            "ranger::withhold::render handed a {d:?} statement with {e:?} on {t:?}"
        ))),
    }
}

/// The `column` value of a whole-table clearance deny (U1): every column of
/// the table, including ones added after the compile and the hidden ones.
pub(crate) const WHOLE_TABLE_COLUMN: &str = "*";

/// One StarRocks clearance deny on `col` (a named column, or
/// [`WHOLE_TABLE_COLUMN`]), named `perm-clearance-{tag}-{c}.{d}.{t}.{col}`.
pub(crate) fn starrocks_deny(
    tag_id: Uuid,
    service: &str,
    col: &TaggedColumn,
    accesses: &[String],
    cleared: Vec<String>,
) -> (String, Value) {
    let name = format!(
        "{COMPILER_CLEARANCE_PREFIX}{tag_id}-{}.{}.{}.{}",
        col.catalog, col.schema, col.table, col.column
    );
    let input = clearance_input(
        name.clone(),
        service.to_string(),
        column_resources(EngineType::StarRocks, col, PolicyTarget::Access),
        accesses,
        cleared,
    );
    // `access_to_ranger_json` hardcodes `serviceType: "trino"`; a
    // StarRocks-targeted body rendered without this stamp deploys cleanly and
    // enforces nothing (`stamp_service_type`'s own reason for existing).
    (
        name,
        stamp_service_type(access_to_ranger_json(&input), EngineType::StarRocks),
    )
}

/// The access type that lets a user store a query result as a materialized
/// view. Spelled as in `service-defs/starrocks.json`, where it is an access type
/// of the `database` resource.
pub(crate) const CREATE_MV_ACCESS: &str = "create materialized view";

/// The only catalog StarRocks creates a materialized view in (the IR's
/// [`MATERIALIZED_VIEW_CATALOG`], the measured fact the guard's path names).
pub const MV_CATALOG: &str = MATERIALIZED_VIEW_CATALOG;

/// What follows [`COMPILER_CLEARANCE_PREFIX`] in the name of the A3 deny
/// (`perm-clearance-mv`). `crate::attribute_policy_name` reads it back.
pub(crate) const MV_DENY_SUFFIX: &str = "mv";

/// The materialized-view guard's body (`egret_perm_ir::lower::withhold`'s
/// `materialized_view_guard`, view governance A3): deny `{public}`
/// [`CREATE_MV_ACCESS`] on `{catalog: <catalog>, database: *}`, except
/// `cleared` — the users cleared for every classification on the profile. The
/// lowering decided the catalog ([`MV_CATALOG`]) and the exceptions; this is
/// the shape.
pub(crate) fn mv_deny_input(
    catalog: &str,
    cleared: Vec<String>,
    service: &str,
) -> (String, AccessPolicyInput) {
    let name = format!("{COMPILER_CLEARANCE_PREFIX}{MV_DENY_SUFFIX}");
    let mut resources = BTreeMap::new();
    resources.insert("catalog".to_string(), level(catalog.to_string()));
    resources.insert("database".to_string(), level("*".to_string()));
    let input = clearance_input(
        name.clone(),
        service.to_string(),
        resources,
        &[CREATE_MV_ACCESS.to_string()],
        cleared,
    );
    (name, input)
}

/// The one clearance policy shape, built once for both engines: deny `{public}`
/// the withheld accesses on `resources`, except the enumerated cleared
/// usernames.
///
/// Shared deliberately. The two branches differ ONLY in which resource they
/// address and which service they are filed under; every other field is a
/// property of what a clearance IS, and three of them are load-bearing in ways a
/// second copy would be free to get wrong — see the inline notes.
pub(crate) fn clearance_input(
    name: String,
    service: String,
    resources: BTreeMap<String, ResourceLevel>,
    accesses: &[String],
    cleared: Vec<String>,
) -> AccessPolicyInput {
    AccessPolicyInput {
        name,
        service,
        is_enabled: true,
        resources,
        items: vec![AccessItemInput {
            users: Vec::new(),
            // The ONE expressible half of the pair: `{public}` matches
            // everybody even with no group provider (D0/D0b, measured — the
            // group check needs only a non-null `userGroups`, and
            // `setUserGroups` normalises `null` to an empty set).
            groups: vec![PUBLIC_GROUP.to_string()],
            // Deliberately empty. Naming the clearing role here — the obvious
            // implementation — is what D0 measured matches nobody, and D0b
            // re-derived the same answer for StarRocks: `setUserRoles` is never
            // called anywhere in `fe/`.
            roles: Vec::new(),
            accesses: accesses.to_vec(),
            effect: "deny".to_string(),
        }],
        allow_exceptions: Vec::new(),
        deny_exceptions: vec![AccessItemInput {
            users: cleared,
            groups: Vec::new(),
            roles: Vec::new(),
            accesses: accesses.to_vec(),
            effect: "deny".to_string(),
        }],
        // MUST stay false: `access_to_ranger_json` blanks BOTH
        // `denyPolicyItems` and `denyExceptions` when `isDenyAllElse` is set,
        // which would turn this policy into a deny nobody can be excepted from.
        is_deny_all_else: false,
        description: String::new(),
        labels: Vec::new(),
        // **MUST stay false on BOTH engines, for two DIFFERENT reasons.** On
        // Trino an override is simply unnecessary — the tag-first short-circuit
        // already wins. On StarRocks it would be actively HARMFUL: with no tag
        // service, deny-beats-allow comes from Ranger sorting `hasDeny()`
        // evaluators first, and priority DOMINATES that comparator
        // (`PolicyEvalOrderComparator.compareNormal`) — so raising the priority
        // breaks the very ordering it looks like it strengthens (D0b).
        is_override: false,
        additional: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(cleared: &[&str]) -> Vec<String> {
        cleared.iter().map(|s| s.to_string()).collect()
    }

    fn exceptions(i: &AccessPolicyInput) -> Vec<String> {
        i.deny_exceptions[0].users.clone()
    }

    /// A3 (revised 2026-10-14): StarRocks creates a materialized view only in
    /// `default_catalog` and checks the privilege on the TARGET database, so the
    /// deny is one policy on `default_catalog`/`*` whatever databases the
    /// classified columns live in, excepting the users cleared for every
    /// classification on the service (the intersection is the lowering's,
    /// `egret_perm_ir::lower::withhold::cleared_for_every`).
    #[test]
    fn the_materialized_view_deny_is_one_engine_wide_policy() {
        let (name, i) = mv_deny_input(MV_CATALOG, pending(&["bob"]), "sr");
        assert_eq!(name, format!("{COMPILER_CLEARANCE_PREFIX}mv"));
        assert_eq!(i.name, name);
        assert_eq!(i.service, "sr");
        assert_eq!(i.resources.len(), 2);
        assert_eq!(i.resources["catalog"].values, vec!["default_catalog"]);
        assert_eq!(i.resources["database"].values, vec!["*"]);
        assert_eq!(i.items[0].groups, vec!["public"]);
        assert_eq!(i.items[0].accesses, vec!["create materialized view"]);
        assert_eq!(i.items[0].effect, "deny");
        assert_eq!(exceptions(&i), vec!["bob"]);
    }

    #[test]
    fn an_empty_exception_list_denies_everyone() {
        let (_, i) = mv_deny_input(MV_CATALOG, Vec::new(), "sr");
        assert!(exceptions(&i).is_empty());
        assert_eq!(i.items[0].groups, vec!["public"]);
    }
}
