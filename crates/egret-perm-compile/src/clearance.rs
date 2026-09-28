//! Compiling a classification's CLEARANCE into an enforceable deny — SP4c
//! **D1**, on the branch D0's measurement forced.
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
//! time as bare usernames read from `users.username`
//! ([`crate::members::role_usernames`]). The Ranger `roles` bucket is not used
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
//! # A tag gated ONLY by `required_groups` compiles to NOTHING, and says so
//!
//! D7 measured that a Ranger item naming a GROUP matches nobody either (no
//! group provider on Trino, no usersync in egret). A group-only gate would
//! therefore compile to a deny whose exceptions except nobody — a control that
//! withholds the column from everybody — or, if the group were put on the deny
//! side, to one that withholds nothing. Neither is the clearance that was
//! authored, so nothing is emitted and the skip is REPORTED
//! ([`crate::SkippedItem`]) and logged rather than being silent. SP4c Task 5
//! says the same thing in the UI, where the clearance is set.
//!
//! # Trino only, because the tag service is
//!
//! A clearance is addressed to the profile's Ranger TAG service, which only a
//! Trino profile has (`config_jsonb->>'tagService'`; StarRocks has no tag
//! service — upstream `starrocks#67458`). A profile with no usable tag service
//! records a skip, exactly as a tag-selected RULE already does
//! (`CompileError::MissingTagService`'s handling in `crate::trigger`), and for
//! the same reason: nothing filed there could be served to anyone. Expressing a
//! StarRocks clearance would need the per-column expansion SP4a's rules use,
//! **and D0's measurement explicitly does not cover StarRocks** (its own plugin
//! has native roles and may populate `userRoles` where Trino passes `null`), so
//! it is not attempted here on a premise nobody has measured.

use std::collections::BTreeMap;

use serde_json::Value;
use uuid::Uuid;

use egret_perm::{engine_accesses, Action, CompileError, EngineType};
use egret_policy::admin::{tag_access_to_ranger_json, AccessItemInput, AccessPolicyInput};
use egret_policy::COMPILER_CLEARANCE_PREFIX;

use crate::rules::PUBLIC_GROUP;
use crate::trigger::{SkippedItem, SourceKind};

/// One `classification_policies` row joined to the tag it gates. A row whose
/// tag no longer exists is dropped by the join — there is no tag left to deny
/// on.
#[derive(sqlx::FromRow)]
struct GateRow {
    tag_id: Uuid,
    /// `tags.name`, never `tags.key`: the tag RESOURCE in a Ranger tag policy
    /// must match the tag type egret serves in its tag download
    /// (`ServiceTags::from_assignments` carries `tag_name`), which is the same
    /// resolution `crate::rules` uses via `tag_name_for_key_via`.
    tag_name: String,
    required_roles: Vec<String>,
    required_groups: Vec<Uuid>,
}

/// What compiling this installation's clearances produced for ONE profile: the
/// rendered tag-policy bodies (already addressed to the tag service by
/// `tagify`, so `crate::trigger::target_service_id` routes them from the body's
/// own `service` field) and the gated tags this profile could not express.
pub(crate) struct Clearances {
    pub(crate) policies: Vec<(String, Value)>,
    pub(crate) skipped: Vec<SkippedItem>,
}

/// Compile every gated tag into a clearance deny for the profile named
/// `tag_service`.
///
/// Clearances are **installation-wide**, exactly like a Trino tag-selected
/// rule: the policy carries no catalog resource at all, and the tag plugin
/// resolves the tag wherever it has been assigned. So every gated tag compiles
/// into every Trino profile that has a tag service, and a clearance write
/// recompiles every such profile (SP4c Task 3's affected set).
pub(crate) async fn compile_clearance(
    conn: &mut sqlx::PgConnection,
    engine: EngineType,
    tag_service: Option<&str>,
) -> Result<Clearances, CompileError> {
    let gates: Vec<GateRow> = sqlx::query_as(
        "SELECT cp.tag_id, t.name AS tag_name, cp.required_roles, cp.required_groups \
           FROM classification_policies cp \
           JOIN tags t ON t.id = cp.tag_id \
          ORDER BY cp.tag_id",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| CompileError::Db(e.to_string()))?;

    let mut out = Clearances {
        policies: Vec::new(),
        skipped: Vec::new(),
    };
    if gates.is_empty() {
        return Ok(out);
    }

    // One query for every role named by every gate, rather than one per tag.
    let wanted: Vec<String> = {
        let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for g in &gates {
            set.extend(g.required_roles.iter().cloned());
        }
        set.into_iter().collect()
    };
    let members = crate::members::role_usernames(conn, &wanted).await?;

    let accesses = withheld_accesses(engine)?;

    for g in &gates {
        // BOTH lists empty is NO gate — an informational tag
        // (`egret_bi::tags::ClassificationPolicyRepo`'s own semantics, and the
        // Cedar loader's). Not a skip: there is no control here to report the
        // absence of.
        if g.required_roles.is_empty() && g.required_groups.is_empty() {
            continue;
        }
        if g.required_roles.is_empty() {
            out.skipped.push(skip(
                g,
                "the tag's clearance names only `required_groups`, and D7 measured that a Ranger \
                 item naming a group matches nobody on any engine egret provisions — so a deny \
                 emitted for it would withhold the column from everybody, and the gate stays \
                 enforced on object access (Cedar) only"
                    .to_string(),
            ));
            continue;
        }
        let Some(tag_service) = tag_service else {
            out.skipped.push(skip(
                g,
                format!(
                    "this {engine:?} profile has no usable `tagService`, and a clearance is a \
                     TAG policy — nothing filed here could be served to any plugin. A StarRocks \
                     clearance would need the per-column expansion its rules use, on a premise \
                     D0 did not measure for StarRocks"
                ),
            ));
            continue;
        };

        // Cleared = holds AT LEAST ONE of the required roles
        // (`ClassificationPolicyRepo`'s semantics: `>= 1` role OR `>= 1`
        // group), so the exception list is the UNION of their member sets.
        // Sorted + deduplicated so a recompile that changed nothing writes
        // nothing (`apply_diff_in_tx` diffs the body byte-for-byte).
        let mut cleared: Vec<String> = Vec::new();
        for role in &g.required_roles {
            if let Some(names) = members.get(role) {
                cleared.extend(names.iter().cloned());
            }
        }
        cleared.sort();
        cleared.dedup();
        if cleared.is_empty() {
            // NOT a skip, and not a defect: nobody holds the clearing role, so
            // nobody is cleared, and a deny standing for everyone is the
            // correct compilation of that — the same answer Cedar's `forbid …
            // unless context.roles.containsAny(...)` reaches. Logged because
            // "the column is refused to everybody" is worth being able to find
            // in a log when someone asks why.
            tracing::info!(
                tag_id = %g.tag_id,
                roles = ?g.required_roles,
                "a clearance's required roles have no holders with a username — the compiled \
                 deny withholds the tagged column from everybody, which is what 'nobody is \
                 cleared' means"
            );
        }

        let name = format!("{COMPILER_CLEARANCE_PREFIX}{}", g.tag_id);
        let input = AccessPolicyInput {
            name: name.clone(),
            // `tagify` overwrites this with `tag_service` regardless; passing
            // it explicitly keeps the body from ever naming the profile.
            service: tag_service.to_string(),
            is_enabled: true,
            // `tagify` replaces the resources map with the single `tag`
            // resource, exactly as it does for a compiled tag mask.
            resources: BTreeMap::new(),
            items: vec![AccessItemInput {
                users: Vec::new(),
                // The ONE expressible half of the pair: `{public}` matches
                // everybody even with no group provider (D0, measured).
                groups: vec![PUBLIC_GROUP.to_string()],
                // Deliberately empty. Naming the clearing role here — the
                // obvious implementation — is what D0 measured matches nobody.
                roles: Vec::new(),
                accesses: accesses.clone(),
                effect: "deny".to_string(),
            }],
            allow_exceptions: Vec::new(),
            deny_exceptions: vec![AccessItemInput {
                users: cleared,
                groups: Vec::new(),
                roles: Vec::new(),
                accesses: accesses.clone(),
                effect: "deny".to_string(),
            }],
            // MUST stay false: `access_to_ranger_json` blanks BOTH
            // `denyPolicyItems` and `denyExceptions` when `isDenyAllElse` is
            // set, which would turn this policy into a deny nobody can be
            // excepted from.
            is_deny_all_else: false,
            description: String::new(),
            labels: Vec::new(),
            is_override: false,
            additional: Vec::new(),
        };
        out.policies.push((
            name,
            tag_access_to_ranger_json(&input, &g.tag_name, tag_service),
        ));
    }

    Ok(out)
}

fn skip(g: &GateRow, reason: String) -> SkippedItem {
    tracing::warn!(
        tag_id = %g.tag_id,
        reason = %reason,
        "classification clearance skipped during recompile"
    );
    SkippedItem {
        source_kind: SourceKind::Clearance,
        id: g.tag_id,
        reason,
    }
}

/// Every access type a clearance withholds on `engine`.
///
/// **Read from the `Action` → access-type map rather than spelled out**, so a
/// clearance cannot drift from what the model can express (an access type the
/// plugin does not declare is silently ignored: the policy compiles, deploys,
/// and withholds nothing — `egret_perm::actions`' own warning).
///
/// **Both actions, not just `read`.** A classification withholds the DATA, not
/// only the `SELECT`: an uncleared user who could still `INSERT` into a
/// classified column would be writing into data they are not cleared to see,
/// and under-denying here is the one failure direction that cannot be noticed
/// from a read test. On Trino that is `select`/`insert`/`delete`.
fn withheld_accesses(engine: EngineType) -> Result<Vec<String>, CompileError> {
    let mut out: Vec<String> = Vec::new();
    for action in [Action::Read, Action::Write] {
        for acc in engine_accesses(action, engine)? {
            if !out.iter().any(|a| a == acc) {
                out.push((*acc).to_string());
            }
        }
    }
    Ok(out)
}
