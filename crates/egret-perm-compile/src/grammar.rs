//! A CLOSED grammar: a Ranger access-policy body becomes either candidate
//! [`Grant`]s or a list of named [`Blocker`]s. Pure — no DB, no clock, no
//! network — so it can be differentially tested, which is the same property
//! that makes `egret-ranger-eval` trustworthy.
//!
//! **Closed, not best-effort.** The model's vocabulary is far narrower than
//! Ranger's: `Action` has two values and expands to a FIXED access-type set per
//! engine (`egret_perm::actions::engine_accesses`), `Scope` names ONE catalog by
//! registry UUID, `principals.exclude` emits exactly one exception item, and
//! `grants.rs::level` hard-codes `is_excludes: false` and `is_recursive: false`.
//! Anything outside that is refused with a blocker that names it.
//!
//! **The subset rule is the whole point.** `Action::Write` is atomic: on Trino
//! it expands to `insert` + `delete`. A policy granting only `insert` is
//! therefore NOT expressible — accepting it as `Write` would ADD `delete`,
//! widening access nobody asked to widen. So an access set must equal a union
//! of FULL expansions; a strict subset of one is
//! [`Blocker::PartialActionExpansion`], carrying what is missing so the
//! operator can see why. A translator that only rejected unknown access types
//! would make exactly this mistake, silently, and in the fail-open direction.
//!
//! **Why the catalog lookup is a closure.** `Scope.catalog_id` is a
//! `registry.catalogs` UUID, so a policy naming a catalog egret does not have a
//! row for cannot become a grant at all — which is precisely the "hand-written
//! profile for an external engine" case SP4a left in place. Taking the lookup
//! as `&dyn Fn(&str) -> Option<Uuid>` keeps this module pure and lets its tests
//! state the registry inline. **The lookup must be EXACT** (no case folding):
//! the compiler emits the registry's name for the id, so a lookup that matched
//! `WH` to `wh` would change the catalog value the engine sees.
//!
//! # The design oracle: this is the inverse of `compile_grant`
//!
//! Everything accepted here is written as a real permission and compiled back
//! by [`crate::grants::compile_grant`]. So the rule for every field is: if the
//! compiled-back body could decide differently from the body read here, the
//! field is refused. `tests/grammar_access.rs`'s
//! `every_accepted_body_compiles_back_to_what_it_came_from` checks that
//! inverse directly. The consequences that are not obvious:
//!
//! - **An ABSENT level is refused ([`Blocker::MissingLevel`]), it is not read
//!   as `*`.** The compiler emits all four levels of the engine's hierarchy,
//!   `*` below the scope. A body that stops at `table` earns
//!   `MatchType::Ancestor` on a column `select` and grants nothing there
//!   (measured live, `egret_ranger_eval::resource`'s module doc); its
//!   compiled-back form would grant it. A `*` level is accepted and becomes
//!   "unnamed"; so is any pure run of `*` (`**`), which Ranger classifies as the
//!   same match-any — every one of the four levels is `wildCard: true` on both
//!   service-defs.
//! - **Several values at a non-catalog level are refused
//!   ([`Blocker::MultipleValues`]), not split.** `Scope` holds one string per
//!   level. Splitting `table: [a, b]` into two grants would be a cartesian
//!   product across levels whose equivalence nobody has measured; several
//!   CATALOGS become N grants because the spec says so and a catalog is the
//!   one level `Scope` names by id anyway.
//! - **A Ranger `role` is refused ([`Blocker::RolePrincipal`]).** The compiler
//!   never emits Ranger's `roles` bucket — it expands a model role into its
//!   holders — and that bucket matched nobody on the engine (SP4c D0). A
//!   migrated role would start matching people.
//! - **Unknown keys are refused at every depth ([`Blocker::UnrecognisedField`]).**
//!   The list of inert header keys is explicit; anything else is a field this
//!   module has not ruled on.
//!
//! # Reading the body the way Ranger reads it
//!
//! The body is deserialised as [`egret_ranger_eval::model::RangerPolicy`], the
//! reader that reproduces Jackson's binding (absent and `null` mean the class
//! default; `isAllowed` defaults to TRUE) and reports every construct it does
//! not model in `unmodelled` — conditions, schedules, zones, `delegateAdmin`,
//! `isAllowed: false`, runtime expressions in users and resource values. Each
//! of those is mapped to a blocker by an EXHAUSTIVE match, so a new
//! `Unmodelled` variant cannot be silently accepted here. A body that reader
//! refuses (e.g. `"isExcludes": "true"`, which Jackson would coerce to `true`)
//! is [`Blocker::Unreadable`].
//!
//! `egret_policy::ambiguous_arbitration` also parses `isExcludes`/`isRecursive`
//! (its `canonical_resources`), and was not borrowed: it is module-private, it
//! folds an empty level into an absent one (which would lose
//! [`Blocker::EmptyLevel`]), and it reads a non-boolean flag as `false` — the
//! fail-open reading this module must not have.
//!
//! # Every blocker, never the first
//!
//! Every check runs and every blocker is collected (de-duplicated, order not
//! significant). Grants are built only when the list is empty. An operator
//! fixing one blocker at a time, with only the first one shown, is the
//! difference between a usable surface and a guessing game.

use std::collections::BTreeSet;

use egret_perm::{
    engine_accesses, Action, Effect, EngineType, Grant, GrantOrigin, Principal, PrincipalKind,
    PrincipalSet, Scope,
};
use egret_ranger_eval::model::{ItemBucket, PolicyItem, RangerPolicy, ResourceLevel, Unmodelled};
use serde_json::Value;
use uuid::Uuid;

use crate::grants::{level_is_a_wildcard_run, resource_level_names, validate_user_ref};

/// Why one Ranger policy cannot become model objects. Each variant names the
/// offending value where there is one, so Task 5 can render it as a sentence
/// an operator can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// An access type outside the engine's expressible set (spelling is not
    /// normalised: `SELECT` is refused as written).
    UnknownAccessType { access_type: String },
    /// Some, not all, of `action`'s expansion — the subset trap.
    PartialActionExpansion {
        action: Action,
        missing: Vec<String>,
    },
    /// A catalog value with no `registry.catalogs` row (including `*`).
    UnregisteredCatalog { catalog: String },
    /// `isExcludes` / `isRecursive` set on a resource level.
    ResourceFlag { level: String, flag: &'static str },
    /// More than one item in `allowExceptions` (or in `denyExceptions`).
    TooManyExceptions { count: usize },
    /// The one exception's access set differs from a main item's.
    ExceptionAccessMismatch,
    /// Non-empty `validitySchedules`.
    ValiditySchedules,
    /// Policy-level or item-level `conditions`.
    Conditions,
    /// `policyPriority` other than 0.
    NonDefaultPriority { priority: i64 },
    /// A resource level whose values are empty or contain `""` (0178).
    EmptyLevel { level: String },
    /// A wildcard run at `level` with a named level beneath it (0179, grant
    /// side only).
    WildcardOverNamedLevel { level: String },
    /// A tag selector's tag is not in `tags.key` — Task 4; never emitted for an
    /// access policy.
    UnknownTag { key: String },
    /// Neither `policyItems` nor `denyPolicyItems` has an item.
    NothingToMigrate,

    // ── Added by Task 3 (fields the brief did not list) ──────────────────
    /// The body does not read as a Ranger policy.
    Unreadable { error: String },
    /// `policyType` other than 0 (access).
    NotAnAccessPolicy { policy_type: i32 },
    /// `isEnabled: false` — the policy enforces nothing today; migrating it
    /// would create live grants.
    Disabled,
    /// `isDenyAllElse: true` — denies everything the items do not allow.
    DenyAllElse,
    /// Non-empty `additionalResources`.
    AdditionalResources,
    /// `zoneName` set.
    SecurityZone { zone: String },
    /// A level of the engine's hierarchy is absent (an absent level is not `*`).
    MissingLevel { level: String },
    /// A resource key outside the engine's catalog/schema|database/table/column
    /// hierarchy.
    UnknownLevel { level: String },
    /// More than one value at a non-catalog level.
    MultipleValues { level: String, count: usize },
    /// A resource value carrying a Ranger runtime expression.
    ResourceExpression { level: String, value: String },
    /// A principal in Ranger's `roles` bucket.
    RolePrincipal { role: String },
    /// A user that is a Ranger runtime expression (`{USER}`, `{OWNER}`, …).
    UserExpression { user: String },
    /// A user the compiler refuses (empty, or contains `@`).
    UnusableUserRef { user: String },
    /// An item or exception naming no user, group or role.
    NoPrincipals,
    /// An item or exception with no access types.
    EmptyAccesses,
    /// An access entry with `isAllowed: false`.
    AccessNotAllowed { access_type: String },
    /// An item with `delegateAdmin: true`.
    DelegateAdmin,
    /// An exception with no main item in its bucket to except from.
    OrphanException,
    /// Mask or row-filter items on an access policy.
    ForeignItems { bucket: &'static str },
    /// A key this grammar has not ruled on (path from the body root).
    UnrecognisedField { field: String },
}

/// The model objects one access policy becomes. Every grant's `id` is
/// [`Uuid::nil`]: `perm.grants` assigns the real one on insert
/// (`create_grant_in_tx` returns it), and a fresh id here would make this
/// function impure for no gain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateGrants {
    pub grants: Vec<Grant>,
}

/// Header keys that change no access decision. Every other header key is
/// either checked below or refused.
const INERT_HEADER_KEYS: &[&str] = &[
    "id",
    "guid",
    "version",
    "name",
    "service",
    // The compiler hard-codes `trino` here on both engines; per-policy
    // `serviceType` is not read by the resource-policy evaluator.
    "serviceType",
    "description",
    "policyLabels",
    // Gates only the TAG repository's prune of an item-less policy
    // (`egret_ranger_eval::model::RangerPolicy::is_audit_enabled`); an access
    // policy with no items is refused anyway.
    "isAuditEnabled",
    "resourceSignature",
    "createdBy",
    "updatedBy",
    "createTime",
    "updateTime",
];

/// Header keys whose VALUE is checked (via the typed read or below).
const CHECKED_HEADER_KEYS: &[&str] = &[
    "isEnabled",
    "policyType",
    "policyPriority",
    "isDenyAllElse",
    "resources",
    "additionalResources",
    "policyItems",
    "denyPolicyItems",
    "allowExceptions",
    "denyExceptions",
    "dataMaskPolicyItems",
    "rowFilterPolicyItems",
    "validitySchedules",
    "conditions",
    "zoneName",
    "options",
];

const ITEM_BUCKETS: &[&str] = &[
    "policyItems",
    "denyPolicyItems",
    "allowExceptions",
    "denyExceptions",
];
const ITEM_KEYS: &[&str] = &[
    "users",
    "groups",
    "roles",
    "accesses",
    "conditions",
    "delegateAdmin",
];
const ACCESS_KEYS: &[&str] = &["type", "isAllowed"];
const LEVEL_KEYS: &[&str] = &["values", "isExcludes", "isRecursive"];

/// Turn one Ranger access-policy body into the grants it is equivalent to, or
/// every reason it is not expressible. See the module doc for the rules.
pub fn grants_from_access_policy(
    body: &Value,
    engine: EngineType,
    catalog_ids: &dyn Fn(&str) -> Option<Uuid>,
) -> Result<CandidateGrants, Vec<Blocker>> {
    let mut b = Vec::new();

    unrecognised_fields(body, &mut b);

    let policy: RangerPolicy = match serde_json::from_value(body.clone()) {
        Ok(p) => p,
        Err(e) => {
            b.push(Blocker::Unreadable {
                error: e.to_string(),
            });
            return Err(dedup(b));
        }
    };

    header(&policy, &mut b);
    for u in &policy.unmodelled {
        b.push(from_unmodelled(u));
    }

    let scopes = scopes(&policy.resources, engine, catalog_ids, &mut b);

    if policy.policy_items.is_empty() && policy.deny_policy_items.is_empty() {
        b.push(Blocker::NothingToMigrate);
    }
    let allow = bucket(
        &policy.policy_items,
        &policy.allow_exceptions,
        Effect::Allow,
        engine,
        &mut b,
    );
    let deny = bucket(
        &policy.deny_policy_items,
        &policy.deny_exceptions,
        Effect::Deny,
        engine,
        &mut b,
    );

    if !b.is_empty() {
        return Err(dedup(b));
    }

    let mut grants = Vec::new();
    for scope in &scopes {
        for (effect, actions, principals) in allow.iter().chain(deny.iter()) {
            grants.push(Grant {
                id: Uuid::nil(),
                effect: *effect,
                actions: actions.clone(),
                scope: scope.clone(),
                principals: principals.clone(),
                origin: GrantOrigin::Explicit,
            });
        }
    }
    Ok(CandidateGrants { grants })
}

fn dedup(all: Vec<Blocker>) -> Vec<Blocker> {
    let mut out: Vec<Blocker> = Vec::with_capacity(all.len());
    for blocker in all {
        if !out.contains(&blocker) {
            out.push(blocker);
        }
    }
    out
}

/// Keys the grammar has not ruled on, at every depth it reads. `null` is
/// Ranger's "absent" and is never a blocker.
fn unrecognised_fields(body: &Value, b: &mut Vec<Blocker>) {
    let Some(obj) = body.as_object() else {
        return; // the typed read reports it
    };
    let mut flag = |field: String| b.push(Blocker::UnrecognisedField { field });

    for (k, v) in obj {
        if v.is_null() {
            continue;
        }
        if !INERT_HEADER_KEYS.contains(&k.as_str()) && !CHECKED_HEADER_KEYS.contains(&k.as_str()) {
            flag(k.clone());
        }
    }
    // `options` is Ranger's free-form policy-options map: refused unless empty.
    if obj
        .get("options")
        .and_then(Value::as_object)
        .is_some_and(|o| !o.is_empty())
    {
        flag("options".to_string());
    }

    if let Some(resources) = obj.get("resources").and_then(Value::as_object) {
        for (level, spec) in resources {
            if let Some(spec) = spec.as_object() {
                for (k, v) in spec {
                    if !v.is_null() && !LEVEL_KEYS.contains(&k.as_str()) {
                        flag(format!("resources.{level}.{k}"));
                    }
                }
            }
        }
    }

    for bucket in ITEM_BUCKETS {
        let Some(items) = obj.get(*bucket).and_then(Value::as_array) else {
            continue;
        };
        for (i, it) in items.iter().enumerate() {
            let Some(it) = it.as_object() else { continue };
            for (k, v) in it {
                if !v.is_null() && !ITEM_KEYS.contains(&k.as_str()) {
                    flag(format!("{bucket}[{i}].{k}"));
                }
            }
            let Some(accesses) = it.get("accesses").and_then(Value::as_array) else {
                continue;
            };
            for (j, a) in accesses.iter().enumerate() {
                let Some(a) = a.as_object() else { continue };
                for (k, v) in a {
                    if !v.is_null() && !ACCESS_KEYS.contains(&k.as_str()) {
                        flag(format!("{bucket}[{i}].accesses[{j}].{k}"));
                    }
                }
            }
        }
    }
}

/// The header fields the typed read keeps as values (the rest arrive through
/// `unmodelled`).
fn header(p: &RangerPolicy, b: &mut Vec<Blocker>) {
    if p.policy_type != 0 {
        b.push(Blocker::NotAnAccessPolicy {
            policy_type: p.policy_type,
        });
    }
    if !p.is_enabled {
        b.push(Blocker::Disabled);
    }
    if p.policy_priority != 0 {
        b.push(Blocker::NonDefaultPriority {
            priority: i64::from(p.policy_priority),
        });
    }
    if p.is_deny_all_else {
        b.push(Blocker::DenyAllElse);
    }
    if !p.additional_resources.is_empty() {
        b.push(Blocker::AdditionalResources);
    }
    if !p.data_mask_policy_items.is_empty() {
        b.push(Blocker::ForeignItems {
            bucket: "dataMaskPolicyItems",
        });
    }
    if !p.row_filter_policy_items.is_empty() {
        b.push(Blocker::ForeignItems {
            bucket: "rowFilterPolicyItems",
        });
    }
}

fn bucket_name(bucket: &ItemBucket) -> &'static str {
    match bucket {
        ItemBucket::PolicyItems => "policyItems",
        ItemBucket::DenyPolicyItems => "denyPolicyItems",
        ItemBucket::AllowExceptions => "allowExceptions",
        ItemBucket::DenyExceptions => "denyExceptions",
        ItemBucket::DataMaskPolicyItems => "dataMaskPolicyItems",
        ItemBucket::RowFilterPolicyItems => "rowFilterPolicyItems",
    }
}

/// Exhaustive on purpose: a new `Unmodelled` variant is a construct the
/// reader saw and does not model, and it must not reach a grant by default.
fn from_unmodelled(u: &Unmodelled) -> Blocker {
    match u {
        Unmodelled::ValiditySchedules => Blocker::ValiditySchedules,
        Unmodelled::PolicyConditions | Unmodelled::ItemConditions { .. } => Blocker::Conditions,
        Unmodelled::ZoneName(zone) => Blocker::SecurityZone { zone: zone.clone() },
        Unmodelled::ResourceValueExpressionMarker {
            resource_type,
            value,
            ..
        } => Blocker::ResourceExpression {
            level: resource_type.clone(),
            value: value.clone(),
        },
        Unmodelled::UserExpressionMarker { user, .. } => {
            Blocker::UserExpression { user: user.clone() }
        }
        Unmodelled::ExplicitDeny { access_type, .. } => Blocker::AccessNotAllowed {
            access_type: access_type.clone(),
        },
        Unmodelled::DelegateAdmin { .. } => Blocker::DelegateAdmin,
        // Only reachable from mask / row-filter items, which an access policy
        // must not carry at all.
        Unmodelled::MaskExpressionMarker { bucket, .. }
        | Unmodelled::MaskConditionExpr { bucket, .. }
        | Unmodelled::RowFilterExpressionMarker { bucket, .. } => Blocker::ForeignItems {
            bucket: bucket_name(bucket),
        },
        // Envelope-level constructs: not producible by a policy body, refused
        // all the same if a future reader starts reporting them per policy.
        Unmodelled::SecurityZones
        | Unmodelled::TagPolicies(_)
        | Unmodelled::PolicyDeltas
        | Unmodelled::ServiceConfig => Blocker::UnrecognisedField {
            field: format!("{u:?}"),
        },
    }
}

/// The resources map → one [`Scope`] per registered catalog value.
fn scopes(
    resources: &std::collections::BTreeMap<String, ResourceLevel>,
    engine: EngineType,
    catalog_ids: &dyn Fn(&str) -> Option<Uuid>,
    b: &mut Vec<Blocker>,
) -> Vec<Scope> {
    let names = resource_level_names(engine);

    for key in resources.keys() {
        if !names.contains(&key.as_str()) {
            b.push(Blocker::UnknownLevel { level: key.clone() });
        }
    }

    // `None` = unusable (already blocked); `Some(None)` = a wildcard run;
    // `Some(Some(v))` = a named value.
    let mut below_catalog: [Option<Option<String>>; 3] = [None, None, None];
    let mut catalogs: Vec<Uuid> = Vec::new();

    for (i, name) in names.iter().enumerate() {
        let Some(level) = resources.get(*name) else {
            b.push(Blocker::MissingLevel {
                level: (*name).to_string(),
            });
            continue;
        };
        if level.is_excludes {
            b.push(Blocker::ResourceFlag {
                level: (*name).to_string(),
                flag: "isExcludes",
            });
        }
        if level.is_recursive {
            b.push(Blocker::ResourceFlag {
                level: (*name).to_string(),
                flag: "isRecursive",
            });
        }
        if level.values.is_empty() || level.values.iter().any(String::is_empty) {
            b.push(Blocker::EmptyLevel {
                level: (*name).to_string(),
            });
            continue;
        }

        if i == 0 {
            for value in &level.values {
                match catalog_ids(value) {
                    Some(id) => {
                        if !catalogs.contains(&id) {
                            catalogs.push(id);
                        }
                    }
                    None => b.push(Blocker::UnregisteredCatalog {
                        catalog: value.clone(),
                    }),
                }
            }
            continue;
        }

        if level.values.len() > 1 {
            b.push(Blocker::MultipleValues {
                level: (*name).to_string(),
                count: level.values.len(),
            });
            continue;
        }
        let value = &level.values[0];
        below_catalog[i - 1] = Some(if level_is_a_wildcard_run(value) {
            None
        } else {
            Some(value.clone())
        });
    }

    // 0179, grant side: a wildcard run may not have a NAMED level beneath it —
    // that is the level skip `perm.grants`' own CHECK forbids, and the compiled
    // artifact would enforce the grant in every schema of the catalog.
    for i in 0..3 {
        if below_catalog[i] != Some(None) {
            continue;
        }
        if below_catalog[i + 1..]
            .iter()
            .any(|deeper| matches!(deeper, Some(Some(_))))
        {
            b.push(Blocker::WildcardOverNamedLevel {
                level: names[i + 1].to_string(),
            });
        }
    }

    let [schema, table, column] = below_catalog.map(Option::flatten);
    catalogs
        .into_iter()
        .map(|catalog_id| Scope {
            catalog_id,
            schema: schema.clone(),
            table: table.clone(),
            column: column.clone(),
        })
        .collect()
}

/// One effect's main items plus its exception bucket → `(effect, actions,
/// principals)` per main item.
fn bucket(
    items: &[PolicyItem],
    exceptions: &[PolicyItem],
    effect: Effect,
    engine: EngineType,
    b: &mut Vec<Blocker>,
) -> Vec<(Effect, Vec<Action>, PrincipalSet)> {
    let mut exclude = Vec::new();
    match exceptions {
        [] => {}
        [one] => {
            if items.is_empty() {
                b.push(Blocker::OrphanException);
            }
            exclude = principals(one, b);
            if one.accesses.is_empty() {
                b.push(Blocker::EmptyAccesses);
            }
            let exc = access_set(one);
            if items.iter().any(|it| access_set(it) != exc) {
                b.push(Blocker::ExceptionAccessMismatch);
            }
        }
        many => b.push(Blocker::TooManyExceptions { count: many.len() }),
    }

    items
        .iter()
        .map(|it| {
            let include = principals(it, b);
            let actions = actions(it, engine, b);
            (
                effect,
                actions,
                PrincipalSet {
                    include,
                    exclude: exclude.clone(),
                },
            )
        })
        .collect()
}

fn access_set(it: &PolicyItem) -> BTreeSet<&str> {
    it.accesses.iter().map(|a| a.access_type.as_str()).collect()
}

/// Users and groups verbatim (the compiler emits them byte-for-byte); roles
/// refused (module doc).
fn principals(it: &PolicyItem, b: &mut Vec<Blocker>) -> Vec<Principal> {
    if it.users.is_empty() && it.groups.is_empty() && it.roles.is_empty() {
        b.push(Blocker::NoPrincipals);
    }
    let mut out: Vec<Principal> = Vec::new();
    let mut push = |p: Principal| {
        if !out.contains(&p) {
            out.push(p);
        }
    };
    for user in &it.users {
        if validate_user_ref(user).is_err() {
            b.push(Blocker::UnusableUserRef { user: user.clone() });
        }
        push(Principal {
            kind: PrincipalKind::User,
            r#ref: user.clone(),
        });
    }
    for group in &it.groups {
        push(Principal {
            kind: PrincipalKind::Group,
            r#ref: group.clone(),
        });
    }
    for role in &it.roles {
        b.push(Blocker::RolePrincipal { role: role.clone() });
    }
    out
}

/// The item's access-type set → the actions whose FULL expansions union to
/// exactly it.
fn actions(it: &PolicyItem, engine: EngineType, b: &mut Vec<Blocker>) -> Vec<Action> {
    let set = access_set(it);
    if set.is_empty() {
        b.push(Blocker::EmptyAccesses);
        return Vec::new();
    }

    // An action the engine cannot express contributes nothing — and an EMPTY
    // expansion must not be read as "fully present", which `all` would do.
    let expansions: Vec<(Action, Vec<&'static str>)> = [Action::Read, Action::Write]
        .into_iter()
        .filter_map(|a| match engine_accesses(a, engine) {
            Ok(exp) if !exp.is_empty() => Some((a, exp)),
            _ => None,
        })
        .collect();

    for access_type in &set {
        if !expansions.iter().any(|(_, exp)| exp.contains(access_type)) {
            b.push(Blocker::UnknownAccessType {
                access_type: (*access_type).to_string(),
            });
        }
    }

    let mut out = Vec::new();
    for (action, expansion) in &expansions {
        let missing: Vec<String> = expansion
            .iter()
            .filter(|a| !set.contains(**a))
            .map(|a| (*a).to_string())
            .collect();
        if missing.is_empty() {
            out.push(*action);
        } else if missing.len() < expansion.len() {
            b.push(Blocker::PartialActionExpansion {
                action: *action,
                missing,
            });
        }
    }
    out
}
