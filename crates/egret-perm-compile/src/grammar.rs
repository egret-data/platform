//! A CLOSED grammar: a Ranger access-policy body becomes either candidate
//! [`Grant`]s or a list of named [`Blocker`]s ([`grants_from_access_policy`]);
//! a masking, row-filter or tag-addressed body becomes candidate [`Rule`]s the
//! same way ([`rules_from_mask_or_filter_policy`], whose doc carries the
//! rule-side rulings). Pure — no DB, no clock, no
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
    PrincipalSet, Rule, RuleKind, Scope, Selector, PUBLIC_GROUP,
};
use egret_ranger_eval::model::{
    has_double_brackets, has_request_expression, ItemBucket, PolicyItem, RangerPolicy,
    ResourceLevel, Unmodelled,
};
use serde_json::Value;
use uuid::Uuid;

use crate::grants::{
    level_is_a_wildcard_run, resource_level_names, validate_user_ref, PolicyTarget,
    ROW_FILTER_LEVELS,
};
use crate::rules::{declared_mask_types, validate_rule_shape};

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

    // ── Added by Task 4 (masking, row-filter and tag-addressed policies) ──
    /// `policyType` is neither 1 (masking) nor 2 (row filter), on a body that
    /// is not tag-addressed.
    NotAMaskOrRowFilter { policy_type: i32 },
    /// A tag-addressed ACCESS policy (`policyType` 0 with a `tag` resource).
    /// The only tag access policy the model has is a CLEARANCE, and its
    /// cleared set is the MEMBERSHIP of `required_roles`, enumerated at
    /// compile time (`crate::clearance`): a hand-written list of users cannot
    /// be inverted into roles, and a tag ALLOW has no model at all. Clearance
    /// migration is therefore OUT OF SCOPE (user ruling, 2026-10-10).
    ClearanceNotExpressible,
    /// Mask item `index` has no `dataMaskInfo`, a `null` one, or one without
    /// `dataMaskType` — the same object to Ranger (Jackson's setter
    /// substitutes an empty info whose type is null). On the TAG repository
    /// such an item NPEs the plugin's constructor and the engine serves
    /// nothing.
    MaskWithoutType { index: usize },
    /// A mask type the engine's service-def does not declare (`dataMaskDef.
    /// maskTypes`): the plugin ignores it and the column is served unmasked.
    UnsupportedMaskType { mask_type: String },
    /// A `valueExpr` on a mask type other than `CUSTOM` — the compiler emits
    /// `valueExpr` for `CUSTOM` only, so it would be dropped.
    ValueExprWithoutCustom { mask_type: String },
    /// A mask value carrying a Ranger request-time expression (`${{…}}`).
    MaskExpression { expr: String },
    /// A row-filter expression Ranger rewrites or resolves per request
    /// (`${{…}}`, a `{TOKEN}`, or `[[…]]`).
    FilterExpression { expr: String },
    /// Row-filter item `index` has no `filterExpr` (absent or `null`), which
    /// Ranger reads as "this item decides nothing" — not the same as `""`.
    MissingFilterExpr { index: usize },
    /// More than the two items the compiler emits (an exemption, then
    /// everyone).
    TooManyRuleItems { count: usize },
    /// Item `index` is in the "everyone" position but does not name exactly
    /// the `public` group and nobody else. A Rule applies to everyone.
    NotForEveryone { index: usize },
    /// Item `index` sits ahead of the everyone item but does not LIFT the
    /// control (`MASK_NONE` with no value, or `filterExpr: ""`) — a
    /// per-principal mask or filter the model has no shape for.
    NotAnExemption { index: usize },
    /// The exemption item names the `public` group, which would exempt
    /// everyone (`rules::exempt_principals` refuses it).
    PublicExemption,
    /// A tag-addressed policy for a StarRocks profile: StarRocks has no tag
    /// service, so a tag rule compiles there to one resource policy per
    /// tagged column — a different artifact from the body it came from.
    TagSelectorOnStarRocks,
    /// The candidate rule failed the compiler's own authoring check
    /// (`validate_rule_shape`) — a backstop; the message is the compiler's.
    CompilerRefuses { error: String },

    // ── Added by Task 4, fix round 1 ─────────────────────────────────────
    /// The body's tag value `key` resolves as a `tags.key`, but that tag's
    /// NAME is `name`, a different string. The engine matches a tag policy on
    /// the tag NAME and the compiler stamps the name back, so the migrated
    /// rule would cover the tag named `name` while the original covers
    /// whatever tag is named `key` — possibly another tag entirely, whose
    /// columns would be left unmasked. The operator remedy is to keep the
    /// policy hand-written (`crate::classify::render_blocker`, SP4d-3 Task 5
    /// fix round 1). Renaming the tag so key and name agree is NOT a safe
    /// default: the engine matches every tag policy on `tags.name`, so a
    /// rename changes what every policy naming `name` covers (unmasking those
    /// columns), and `tags.name` is not unique, so it can leave two tags under
    /// one name. Rewriting the policy to name `name` is not equivalent either.
    TagNameDiffersFromKey { key: String, name: String },
    /// A tag-addressed body (`resources: {tag: …}`) stored in a RESOURCE
    /// service, where it matches nothing; compiled back, it would be filed in
    /// the tag service and start enforcing.
    TagBodyInResourceService,
    /// A path-addressed body (catalog/schema/…) stored in a TAG service, where
    /// it matches nothing; compiled back, it would be filed in the resource
    /// service and start enforcing.
    PathBodyInTagService,
    /// An internal invariant failed: the grammar found no blocker yet has
    /// nothing to build. Never expected; present so that no refusal is ever
    /// an empty list.
    Internal { detail: &'static str },
}

/// Which kind of Ranger service the body is stored in — the caller knows it
/// (the profile row), the body alone does not. A tag-addressed body only
/// means something in a TAG service and a path-addressed one only in a
/// RESOURCE service; the compiler files each in the matching service, so a
/// body in the wrong one would start enforcing after migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    Resource,
    Tag,
}

/// The one resource key a tag-addressed body carries (`tagify`).
const TAG_LEVEL: &str = "tag";

/// The service-placement ruling, shared by both grammars. Returns whether the
/// body is tag-addressed.
fn service_placement(
    resources: &std::collections::BTreeMap<String, ResourceLevel>,
    service: ServiceKind,
    b: &mut Vec<Blocker>,
) -> bool {
    let tag_addressed = resources.contains_key(TAG_LEVEL);
    match (service, tag_addressed) {
        (ServiceKind::Resource, true) => b.push(Blocker::TagBodyInResourceService),
        (ServiceKind::Tag, false) => b.push(Blocker::PathBodyInTagService),
        _ => {}
    }
    tag_addressed
}

/// The model objects one access policy becomes. Every grant's `id` is
/// [`Uuid::nil`]: `perm.grants` assigns the real one on insert
/// (`create_grant_in_tx` returns it), and a fresh id here would make this
/// function impure for no gain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateGrants {
    pub grants: Vec<Grant>,
}

/// The model objects one masking / row-filter / tag policy becomes. Every
/// rule's `id` is [`Uuid::nil`] for the same reason [`CandidateGrants`]' are.
///
/// There is no `clearances` field: clearance migration is OUT OF SCOPE (user
/// ruling, 2026-10-10). See [`Blocker::ClearanceNotExpressible`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRules {
    pub rules: Vec<Rule>,
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
/// The two rule buckets, scanned only by the rule grammar (an access policy
/// refuses them wholesale as [`Blocker::ForeignItems`]). Each item may carry
/// its payload object, whose own keys are ruled on too.
const RULE_ITEM_BUCKETS: &[&str] = &["dataMaskPolicyItems", "rowFilterPolicyItems"];
const MASK_INFO_KEYS: &[&str] = &["dataMaskType", "valueExpr", "conditionExpr"];
const ROW_FILTER_INFO_KEYS: &[&str] = &["filterExpr"];
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
    service: ServiceKind,
) -> Result<CandidateGrants, Vec<Blocker>> {
    let mut b = Vec::new();

    unrecognised_fields(body, false, &mut b);

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
    // A tag-addressed ACCESS body is a clearance's shape, which is out of
    // scope wherever it is stored.
    if service_placement(&policy.resources, service, &mut b) {
        b.push(Blocker::ClearanceNotExpressible);
    }

    let scopes = scopes(
        &policy.resources,
        engine,
        PolicyTarget::Access,
        catalog_ids,
        &mut b,
    );

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
/// Ranger's "absent" and is never a blocker. `rules` also scans the mask and
/// row-filter buckets and their payload objects.
fn unrecognised_fields(body: &Value, rules: bool, b: &mut Vec<Blocker>) {
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

    let rule_buckets: &[&str] = if rules { RULE_ITEM_BUCKETS } else { &[] };
    for bucket in ITEM_BUCKETS.iter().chain(rule_buckets) {
        let Some(items) = obj.get(*bucket).and_then(Value::as_array) else {
            continue;
        };
        let payload: Option<(&str, &[&str])> = match *bucket {
            "dataMaskPolicyItems" => Some(("dataMaskInfo", MASK_INFO_KEYS)),
            "rowFilterPolicyItems" => Some(("rowFilterInfo", ROW_FILTER_INFO_KEYS)),
            _ => None,
        };
        for (i, it) in items.iter().enumerate() {
            let Some(it) = it.as_object() else { continue };
            for (k, v) in it {
                let is_payload = payload.is_some_and(|(key, _)| key == k);
                if !v.is_null() && !ITEM_KEYS.contains(&k.as_str()) && !is_payload {
                    flag(format!("{bucket}[{i}].{k}"));
                }
            }
            if let Some((key, allowed)) = payload {
                if let Some(info) = it.get(key).and_then(Value::as_object) {
                    for (k, v) in info {
                        if !v.is_null() && !allowed.contains(&k.as_str()) {
                            flag(format!("{bucket}[{i}].{key}.{k}"));
                        }
                    }
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
    common_header(p, b);
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

/// The header rulings every policy kind shares: the compiler emits
/// `isEnabled: true`, priority 0, no deny-all-else and no additional
/// resources for a grant and a rule alike.
fn common_header(p: &RangerPolicy, b: &mut Vec<Blocker>) {
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

/// Whether a match-any level may have a NAMED level beneath it. Refused on a
/// grant only (0179): there it widens AUTHORITY, the measured escalation. On a
/// rule it widens RESTRICTION and both plugins enforce it — product decision
/// 2026-10-10, `migrations/0179`'s closing section, and the reason
/// `rules::validate_rule_shape` does not call the grant-side guard either.
fn refuses_wildcard_over_named(target: PolicyTarget) -> bool {
    matches!(target, PolicyTarget::Access)
}

/// Whether a scope may stop above the deepest level by OMITTING the rest. Only
/// a mask: `scope_to_resources`' `Mask` arm renders exactly the levels a scope
/// names, so an absent trailing level is `None` and compiles back absent. A
/// grant's and a row filter's renderers emit every level of their hierarchy,
/// so for them an absent level is a body the compiler cannot produce.
fn may_stop_short(target: PolicyTarget) -> bool {
    matches!(target, PolicyTarget::Mask)
}

/// The resources map → one [`Scope`] per registered catalog value, read
/// against the hierarchy `target` renders into (four levels; three for a row
/// filter, which `rowFilterDef` stops at `table`).
///
/// Values are carried as the compiler renders them back: on a grant a wildcard
/// run is "unnamed" (`None`, rendered `*`); on a rule every present value is
/// kept VERBATIM (`Some("*")` renders `*`, and `None` renders absent on a mask).
fn scopes(
    resources: &std::collections::BTreeMap<String, ResourceLevel>,
    engine: EngineType,
    target: PolicyTarget,
    catalog_ids: &dyn Fn(&str) -> Option<Uuid>,
    b: &mut Vec<Blocker>,
) -> Vec<Scope> {
    let all = resource_level_names(engine);
    let names: &[&str] = match target {
        PolicyTarget::RowFilter => &all[..ROW_FILTER_LEVELS],
        PolicyTarget::Access | PolicyTarget::Mask => &all,
    };

    for key in resources.keys() {
        if !names.contains(&key.as_str()) {
            b.push(Blocker::UnknownLevel { level: key.clone() });
        }
    }

    // `raw[i]` is the one usable value at `names[i + 1]` (`None` = absent or
    // already blocked); `absent[i]` is whether `names[i]` is missing.
    let mut raw: [Option<String>; 3] = [None, None, None];
    let mut absent = [false; 4];
    let mut catalogs: Vec<Uuid> = Vec::new();

    for (i, name) in names.iter().enumerate() {
        let Some(level) = resources.get(*name) else {
            absent[i] = true;
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
        raw[i - 1] = Some(level.values[0].clone());
    }

    // An absent level is refused unless it is part of a trailing run a mask is
    // allowed to omit. The catalog is never optional (`Scope.catalog_id`).
    for (i, name) in names.iter().enumerate() {
        let omissible =
            may_stop_short(target) && i > 0 && absent[i..names.len()].iter().all(|a| *a);
        if absent[i] && !omissible {
            b.push(Blocker::MissingLevel {
                level: (*name).to_string(),
            });
        }
    }

    // 0179, grant side only: a wildcard run may not have a NAMED level beneath
    // it — that is the level skip `perm.grants`' own CHECK forbids, and the
    // compiled artifact would enforce the grant in every schema of the catalog.
    let wild = |v: &Option<String>| v.as_deref().is_some_and(level_is_a_wildcard_run);
    if refuses_wildcard_over_named(target) {
        for i in 0..3 {
            if !wild(&raw[i]) {
                continue;
            }
            if raw[i + 1..]
                .iter()
                .any(|deeper| deeper.is_some() && !wild(deeper))
            {
                b.push(Blocker::WildcardOverNamedLevel {
                    level: names[i + 1].to_string(),
                });
            }
        }
    }

    let [schema, table, column] = raw.map(|v| match target {
        PolicyTarget::Access => v.filter(|s| !level_is_a_wildcard_run(s)),
        PolicyTarget::Mask | PolicyTarget::RowFilter => v,
    });
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

/// Users and groups verbatim; roles refused (module doc).
///
/// **A non-`public` group is no longer emitted byte-for-byte (J-F3 Task 1b).**
/// `compile_grant` still buckets one verbatim, which is what the grammar
/// differential compares, but the recompile a migration runs first expands it
/// into the group's members (`crate::members`). The hand-written group item
/// matched nobody on the engine (D7) and the compiled one names real people,
/// so the migration's equivalence gate (`crate::equivalence`, whose synthetic
/// `egret-probe-member-of-*` user carries the group) sees the two differ and
/// refuses the migration. That refusal is correct — it is not equivalent — but
/// it arrives as a probe divergence rather than a named blocker.
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

// ── Rules: masking, row-filter and tag-addressed policies (Task 4) ──────────

/// Turn one Ranger masking, row-filter or tag-addressed body into the
/// [`Rule`]s it is equivalent to, or every reason it is not expressible.
///
/// The design oracle is [`crate::rules::compile_rule`], exactly as
/// [`crate::grants::compile_grant`] is for [`grants_from_access_policy`]: an
/// accepted body, compiled back, must decide identically. What that compiler
/// emits, and therefore the only shape accepted here:
///
/// - **Items: `[exemption?, everyone]`.** The last item names exactly the
///   `public` group and carries the rule's mask or filter; an optional first
///   item names the exempt users/groups and LIFTS the control (`MASK_NONE`
///   with no value, or `filterExpr: ""`). Ranger takes the first matching
///   item, so the order is part of the meaning, and anything else — a mask for
///   named principals only, two different masks, an exemption after the
///   everyone item — is a per-principal control a [`Rule`] cannot hold.
/// - **Accesses: exactly `select`** on every item.
/// - **Resources: as `scope_to_resources` renders them.** A mask renders only
///   the levels its scope names, so it may stop above the column; a row filter
///   always renders catalog + schema|database + table, so all three are
///   required and a `column` is [`Blocker::UnknownLevel`]. Values are kept
///   verbatim, so a `*` above a named level is ACCEPTED here (product decision
///   2026-10-10) — the asymmetry with the grant grammar is
///   [`refuses_wildcard_over_named`].
/// - **A tag body** (`resources: {tag: [v]}`) becomes `Selector::Tag { key:
///   v }` when `tag_names` resolves `v` as a `tags.key` (NEVER as a
///   `tags.name`, which is mutable and non-unique; SP4a D6) AND that tag's
///   name is exactly `v` ([`Blocker::TagNameDiffersFromKey`]).
/// - **`service`** is where the body is stored; a tag body must be in a tag
///   service and a path body in a resource service.
///
/// Every candidate also passes [`validate_rule_shape`], the compiler's own
/// authoring check, as a backstop ([`Blocker::CompilerRefuses`]).
pub fn rules_from_mask_or_filter_policy(
    body: &Value,
    engine: EngineType,
    catalog_ids: &dyn Fn(&str) -> Option<Uuid>,
    tag_names: &dyn Fn(&str) -> Option<String>,
    service: ServiceKind,
) -> Result<CandidateRules, Vec<Blocker>> {
    let mut b = Vec::new();

    unrecognised_fields(body, true, &mut b);

    let policy: RangerPolicy = match serde_json::from_value(body.clone()) {
        Ok(p) => p,
        Err(e) => {
            b.push(Blocker::Unreadable {
                error: e.to_string(),
            });
            return Err(dedup(b));
        }
    };

    let tag_addressed = service_placement(&policy.resources, service, &mut b);
    let target = match policy.policy_type {
        1 => Some(PolicyTarget::Mask),
        2 => Some(PolicyTarget::RowFilter),
        0 if tag_addressed => {
            b.push(Blocker::ClearanceNotExpressible);
            None
        }
        other => {
            b.push(Blocker::NotAMaskOrRowFilter { policy_type: other });
            None
        }
    };
    common_header(&policy, &mut b);

    let own_bucket = match target {
        Some(PolicyTarget::Mask) => "dataMaskPolicyItems",
        Some(PolicyTarget::RowFilter) => "rowFilterPolicyItems",
        _ => "",
    };
    for (bucket, items) in [
        ("policyItems", &policy.policy_items),
        ("denyPolicyItems", &policy.deny_policy_items),
        ("allowExceptions", &policy.allow_exceptions),
        ("denyExceptions", &policy.deny_exceptions),
        ("dataMaskPolicyItems", &policy.data_mask_policy_items),
        ("rowFilterPolicyItems", &policy.row_filter_policy_items),
    ] {
        if bucket != own_bucket && !items.is_empty() {
            b.push(Blocker::ForeignItems { bucket });
        }
    }

    for u in &policy.unmodelled {
        if let Some(blocker) = rule_blocker_from_unmodelled(u) {
            b.push(blocker);
        }
    }

    let selectors: Vec<Selector> = if tag_addressed {
        tag_selector(&policy.resources, engine, tag_names, &mut b)
            .into_iter()
            .collect()
    } else if let Some(target) = target {
        scopes(&policy.resources, engine, target, catalog_ids, &mut b)
            .into_iter()
            .map(Selector::Path)
            .collect()
    } else {
        Vec::new()
    };

    let payload = match target {
        Some(PolicyTarget::Mask) => rule_items(
            &policy.data_mask_policy_items,
            PolicyTarget::Mask,
            engine,
            &mut b,
        ),
        Some(PolicyTarget::RowFilter) => rule_items(
            &policy.row_filter_policy_items,
            PolicyTarget::RowFilter,
            engine,
            &mut b,
        ),
        _ => None,
    };

    if !b.is_empty() {
        return Err(dedup(b));
    }
    // Every path that leaves `payload` empty has pushed a blocker; an empty
    // list here would be a bug, and it still refuses rather than accepts.
    let Some((kind, exempt)) = payload else {
        b.push(Blocker::Internal {
            detail: "no blocker was recorded, yet the items yielded no mask or filter",
        });
        return Err(b);
    };

    // `perm.rules.name` is UNIQUE, so N rules from one policy (one per
    // catalog) need N names; Task 7 owns collisions with existing rows.
    let n = selectors.len();
    let rules: Vec<Rule> = selectors
        .into_iter()
        .enumerate()
        .map(|(i, selector)| Rule {
            id: Uuid::nil(),
            name: if n == 1 {
                policy.name.clone()
            } else {
                format!("{} #{}", policy.name, i + 1)
            },
            kind: kind.clone(),
            selector,
            exempt: exempt.clone(),
        })
        .collect();

    for rule in &rules {
        if let Err(e) = validate_rule_shape(rule) {
            b.push(Blocker::CompilerRefuses {
                error: e.to_string(),
            });
        }
    }
    if !b.is_empty() {
        return Err(dedup(b));
    }

    Ok(CandidateRules { rules })
}

/// [`from_unmodelled`] with the three rule-payload constructs ruled on for
/// rules (an access policy refuses them as foreign items). `None` = the
/// construct is exactly representable and is not a blocker.
fn rule_blocker_from_unmodelled(u: &Unmodelled) -> Option<Blocker> {
    match u {
        // The reader flags a bare `{TOKEN}` too, but Ranger's mask evaluator
        // resolves only `${{…}}` (`has_request_expression`'s doc): a CUSTOM
        // mask's `{col}` is reported verbatim, and the compiler emits it
        // verbatim — it is exactly what `ensure_custom_mask_parses` requires.
        Unmodelled::MaskExpressionMarker { expr, .. } => {
            has_request_expression(expr).then(|| Blocker::MaskExpression { expr: expr.clone() })
        }
        // Refused whole: a filter is spliced into the engine's SQL, so being
        // wrong about a token here fails open (rows unfiltered).
        Unmodelled::RowFilterExpressionMarker { expr, .. } => {
            Some(Blocker::FilterExpression { expr: expr.clone() })
        }
        Unmodelled::MaskConditionExpr { .. } => Some(Blocker::Conditions),
        other => Some(from_unmodelled(other)),
    }
}

/// A tag-addressed body's single `tag` resource → `Selector::Tag`.
fn tag_selector(
    resources: &std::collections::BTreeMap<String, ResourceLevel>,
    engine: EngineType,
    tag_names: &dyn Fn(&str) -> Option<String>,
    b: &mut Vec<Blocker>,
) -> Option<Selector> {
    if engine == EngineType::StarRocks {
        b.push(Blocker::TagSelectorOnStarRocks);
    }
    for key in resources.keys() {
        if key != TAG_LEVEL {
            b.push(Blocker::UnknownLevel { level: key.clone() });
        }
    }
    let level = resources.get(TAG_LEVEL)?;
    if level.is_excludes {
        b.push(Blocker::ResourceFlag {
            level: TAG_LEVEL.to_string(),
            flag: "isExcludes",
        });
    }
    if level.is_recursive {
        b.push(Blocker::ResourceFlag {
            level: TAG_LEVEL.to_string(),
            flag: "isRecursive",
        });
    }
    if level.values.is_empty() || level.values.iter().any(String::is_empty) {
        b.push(Blocker::EmptyLevel {
            level: TAG_LEVEL.to_string(),
        });
        return None;
    }
    // A policy on two tags matches a column carrying either; two tag rules
    // would be two policies arbitrated against each other on a column
    // carrying both — not the same artifact.
    if level.values.len() > 1 {
        b.push(Blocker::MultipleValues {
            level: TAG_LEVEL.to_string(),
            count: level.values.len(),
        });
        return None;
    }
    // Resolved by `tags.key` (D6) — and then the NAME must be the same
    // string, because the engine matches the body's value against tag names
    // and `compile_rule` stamps the resolved tag's name back. Names are not
    // unique, but the original and the compiled body then match on the same
    // name string, so they cover the same tags.
    let key = &level.values[0];
    let Some(name) = tag_names(key) else {
        b.push(Blocker::UnknownTag { key: key.clone() });
        return None;
    };
    if name != *key {
        b.push(Blocker::TagNameDiffersFromKey {
            key: key.clone(),
            name,
        });
        return None;
    }
    Some(Selector::Tag { key: key.clone() })
}

/// The rule bucket's items → the rule's kind and its exempt set, when they
/// are exactly `[exemption?, everyone]` (see
/// [`rules_from_mask_or_filter_policy`]).
fn rule_items(
    items: &[PolicyItem],
    target: PolicyTarget,
    engine: EngineType,
    b: &mut Vec<Blocker>,
) -> Option<(RuleKind, PrincipalSet)> {
    if items.is_empty() {
        b.push(Blocker::NothingToMigrate);
        return None;
    }
    let declared = if target == PolicyTarget::Mask {
        declared_mask_types(engine).unwrap_or_else(|e| {
            b.push(Blocker::CompilerRefuses {
                error: e.to_string(),
            });
            Vec::new()
        })
    } else {
        Vec::new()
    };

    let payloads: Vec<Option<RuleKind>> = items
        .iter()
        .enumerate()
        .map(|(index, it)| {
            select_only(it, b);
            rule_payload(index, it, target, &declared, b)
        })
        .collect();

    if items.len() > 2 {
        b.push(Blocker::TooManyRuleItems { count: items.len() });
        return None;
    }

    let last = items.len() - 1;
    let everyone = &items[last];
    if !(everyone.users.is_empty()
        && everyone.roles.is_empty()
        && everyone.groups.len() == 1
        && everyone.groups[0] == PUBLIC_GROUP)
    {
        b.push(Blocker::NotForEveryone { index: last });
    }
    for role in &everyone.roles {
        b.push(Blocker::RolePrincipal { role: role.clone() });
    }

    let mut include = Vec::new();
    if last == 1 {
        let exemption = &items[0];
        if exemption.groups.iter().any(|g| g == PUBLIC_GROUP) {
            b.push(Blocker::PublicExemption);
        }
        include = principals(exemption, b);
        if let Some(kind) = &payloads[0] {
            if !lifts_the_control(kind) {
                b.push(Blocker::NotAnExemption { index: 0 });
            }
        }
    }

    let kind = payloads[last].clone()?;
    Some((
        kind,
        PrincipalSet {
            include,
            exclude: Vec::new(),
        },
    ))
}

/// What `rules::mask_items` / `rowfilter_items` put in the exemption item.
fn lifts_the_control(kind: &RuleKind) -> bool {
    match kind {
        RuleKind::Mask {
            mask_type,
            value_expr,
        } => mask_type == "MASK_NONE" && value_expr.is_none(),
        RuleKind::RowFilter { filter_expr } => filter_expr.is_empty(),
    }
}

/// The compiler gives every mask and row-filter item `accesses: [select]`.
fn select_only(it: &PolicyItem, b: &mut Vec<Blocker>) {
    let set = access_set(it);
    if set.is_empty() {
        b.push(Blocker::EmptyAccesses);
    }
    for access_type in set {
        if access_type != "select" {
            b.push(Blocker::UnknownAccessType {
                access_type: access_type.to_string(),
            });
        }
    }
}

/// One item's mask or filter payload, or `None` with a blocker pushed.
fn rule_payload(
    index: usize,
    it: &PolicyItem,
    target: PolicyTarget,
    declared: &[String],
    b: &mut Vec<Blocker>,
) -> Option<RuleKind> {
    match target {
        PolicyTarget::Mask => {
            let info = it.data_mask_info.clone().unwrap_or_default();
            let Some(mask_type) = info.mask_type else {
                b.push(Blocker::MaskWithoutType { index });
                return None;
            };
            if !declared.contains(&mask_type) {
                b.push(Blocker::UnsupportedMaskType {
                    mask_type: mask_type.clone(),
                });
            }
            if mask_type != "CUSTOM" && info.value_expr.is_some() {
                b.push(Blocker::ValueExprWithoutCustom {
                    mask_type: mask_type.clone(),
                });
            }
            Some(RuleKind::Mask {
                mask_type,
                value_expr: info.value_expr,
            })
        }
        PolicyTarget::RowFilter => {
            let Some(filter_expr) = it
                .row_filter_info
                .as_ref()
                .and_then(|i| i.filter_expr.clone())
            else {
                b.push(Blocker::MissingFilterExpr { index });
                return None;
            };
            // `replaceDoubleBrackets` rewrites `[[…]]` in the constructor; the
            // reader does not flag it, so it is checked here.
            if has_double_brackets(&filter_expr) {
                b.push(Blocker::FilterExpression {
                    expr: filter_expr.clone(),
                });
            }
            Some(RuleKind::RowFilter { filter_expr })
        }
        PolicyTarget::Access => None,
    }
}
