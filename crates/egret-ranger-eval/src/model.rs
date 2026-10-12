//! A typed reader for Ranger policy bodies.
//!
//! Field names were verified against three sources, not transcribed from
//! any summary: `crates/egret-policy/src/admin/ranger_json.rs` (the
//! writer, for the shapes egret itself produces), Apache Ranger's own
//! `RangerPolicy`/`ServicePolicies` models
//! (`agents-common/.../plugin/model/RangerPolicy.java` and
//! `.../plugin/util/ServicePolicies.java`, fetched from `apache/ranger@master`),
//! and the captured download at `tests/ranger/fixtures/service-policies.json`.
//! Fix round 1 (review of the initial version) additionally verified the
//! request-time expression syntax and default values directly against
//! `javap` disassembly of `ranger-plugins-common` 2.5.0 and 2.8.0 — see
//! `contains_expression_marker`'s doc comment and the `defaults_match_ranger_*`
//! test for what was checked and how.
//!
//! Three kinds of field live here:
//!
//! - **Round-tripped** fields decide the outcome once Tasks 4-6 evaluate
//!   this shape: `policyType`, `isEnabled`, `policyPriority`,
//!   `isDenyAllElse`, `resources`/`additionalResources` (with `values`,
//!   `isExcludes`, `isRecursive`), the six item buckets, and per item
//!   `users`/`groups`/`roles`/`accesses[].type`/`accesses[].isAllowed`.
//! - **Retained but not yet typed**: the envelope's `serviceDef`. It is
//!   where Ranger's matching semantics actually live (`accessTypes` with
//!   `impliedGrants`, `resources[].matcherOptions`, `dataMaskDef`,
//!   `rowFilterDef`) and Task 4 needs it to lift the hierarchy walker out
//!   of `crates/egret-policy/src/default_policies.rs:20-90` — dropping it
//!   would be exactly the silent-drop defect this crate exists to prevent,
//!   so it round-trips as a raw `serde_json::Value` here; Task 4 gives it a
//!   real type.
//! - **Reported, not evaluated** constructs change the decision in ways
//!   this reader does not implement: a policy-wide validity window, a
//!   policy- or item-level condition, security-zone scoping, an explicit
//!   per-access deny sitting inside an "allow" bucket, `delegateAdmin`, a
//!   a `tagPolicies` block carrying policies (a whole second, DENY-capable
//!   policy set — but see [`TagPolicyBlock`]: an EMPTY block is measured
//!   inert and is not reported),
//!   `policyDeltas` (an incremental update, meaning `policies` is partial),
//!   a non-empty `serviceConfig`, and a resource value / mask-or-row-filter
//!   expression / item principal containing a Ranger runtime expression.
//!   Each is a variant of [`Unmodelled`], collected on the policy (or, for
//!   the envelope-level constructs, the [`ServicePolicySet`]) that carried
//!   it. Parsing still succeeds — the alternative, dropping the field
//!   silently, is the exact defect this crate exists to prevent.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

// ── Public types ─────────────────────────────────────────────────────────

/// One `ServicePolicies` download: the envelope Ranger (or egret's own
/// `/service/plugins/policies/download/<service>` equivalent) returns for a
/// service — see `tests/ranger/fixtures/service-policies.json` for a
/// captured example.
#[derive(Debug, Clone, PartialEq)]
pub struct ServicePolicySet {
    pub service_name: String,
    pub service_id: Option<i64>,
    pub policy_version: Option<i64>,
    pub policies: Vec<RangerPolicy>,
    /// The service's `serviceDef` object, round-tripped verbatim (not
    /// dropped, not yet typed — see the module doc). `Value::Null` when
    /// the envelope omitted it.
    pub service_def: serde_json::Value,
    /// The `tagPolicies` block's OWN `serviceDef`, round-tripped verbatim.
    /// `Value::Null` when there is no block or the block carried no def.
    ///
    /// This is a SECOND def, and it is not decoration: Ranger builds context
    /// enrichers from both defs and unions the two lists, so this one can
    /// supply a `RangerTagEnricher` that the top-level def does not — see
    /// `tag_policies_can_decide` in [`crate::eval`] for the measured chain.
    /// Dropping it would leave the tag narrowing reading only half of what
    /// decides whether tag policies can run.
    pub tag_service_def: serde_json::Value,
    /// The `tagPolicies` block's OWN `policies`, read with the same
    /// [`RangerPolicy`] reader as the resource policies — because they ARE
    /// the same shape: `ServicePolicies$TagPolicies.getPolicies()` is a
    /// `List<RangerPolicy>` in both jars, and `RangerPolicyRepository`'s
    /// tag-service constructor files them through the very same
    /// `skipBuildingPolicyEvaluator` / `init` path.
    ///
    /// Empty when there is no block, when the block carries no policies, or
    /// when the block's `policies` is a shape this reader could not read —
    /// and that last case is NOT a silent drop: it is reported as
    /// [`TagPolicyBlock::Unreadable`], see [`classify_tag_policies`].
    ///
    /// SP4d-2b Task 2: [`crate::eval::evaluate_tag_policies`] evaluates these.
    /// Before it, the block's policies were COUNTED (for the refusal) and
    /// then dropped.
    pub tag_policies: Vec<RangerPolicy>,
    /// Envelope-level constructs this reader does not model: a non-empty
    /// `securityZones`, `policyDeltas` or `serviceConfig`, and a `tagPolicies`
    /// block this reader could not show inert ([`TagPolicyBlock`]).
    /// Per-policy constructs live on each [`RangerPolicy::unmodelled`]
    /// instead — use [`ServicePolicySet::all_unmodelled`] to see both.
    pub unmodelled: Vec<Unmodelled>,
}

impl ServicePolicySet {
    /// Every [`Unmodelled`] construct anywhere in this envelope: its own
    /// envelope-level reports, folded together with every policy's
    /// per-policy reports. A caller asking "did anything in this download
    /// go unmodelled?" should call this rather than checking
    /// `self.unmodelled` alone, which only sees envelope-level constructs
    /// and misses every policy-level one.
    pub fn all_unmodelled(&self) -> Vec<&Unmodelled> {
        self.unmodelled
            .iter()
            .chain(self.policies.iter().flat_map(|p| p.unmodelled.iter()))
            .collect()
    }
}

/// One Ranger policy body (`policy.authz_policies.policy_jsonb`), typed for
/// reading. `policy_type` is the raw Ranger `policyType` code: `0` access,
/// `1` masking, `2` row-filter.
#[derive(Debug, Clone, PartialEq)]
pub struct RangerPolicy {
    pub id: Option<i64>,
    pub name: String,
    pub service: String,
    pub policy_type: i32,
    pub is_enabled: bool,
    /// `isAuditEnabled`, which Ranger's model class defaults to `TRUE` for an
    /// absent or null value — so only an explicit `false` is observable here.
    ///
    /// # Why a field, when nothing audits
    ///
    /// It is not read for auditing. It is the gate of a PRUNE that only the TAG
    /// policy repository performs: `RangerPolicyRepository`'s
    /// `ServicePolicies$TagPolicies` constructor — and **no other constructor in
    /// either jar** — calls `normalizeAndPrunePolicies`, whose
    /// `isPolicyNeedsPruning` removes a policy outright when
    /// `!getIsAuditEnabled()` and all SIX item buckets are empty (2.5.0 offsets
    /// 60-133 of `isPolicyNeedsPruning`, identical in 2.8.0).
    ///
    /// `isDenyAllElse` is **not** one of the six, which is what makes the prune
    /// observable rather than vacuous: a tag ACCESS policy with
    /// `isDenyAllElse: true`, no items and `isAuditEnabled: false` is removed by
    /// Ranger and DENIES without this field. Found by the differential gate
    /// (SP4d-2b Task 5); see `crate::eval::tag_policy_is_pruned`.
    pub is_audit_enabled: bool,
    pub policy_priority: i32,
    pub is_deny_all_else: bool,
    pub resources: BTreeMap<String, ResourceLevel>,
    pub additional_resources: Vec<BTreeMap<String, ResourceLevel>>,
    pub policy_items: Vec<PolicyItem>,
    pub deny_policy_items: Vec<PolicyItem>,
    pub allow_exceptions: Vec<PolicyItem>,
    pub deny_exceptions: Vec<PolicyItem>,
    pub data_mask_policy_items: Vec<PolicyItem>,
    pub row_filter_policy_items: Vec<PolicyItem>,
    /// Constructs on this policy that this reader detected but does not
    /// model. Never empty silently: if a field below would have changed
    /// the decision and isn't in the round-tripped set above, it is
    /// reported here instead of being dropped.
    pub unmodelled: Vec<Unmodelled>,
    /// The policy body exactly as it was read (SP4d-4 Task 5 fix round 1).
    ///
    /// The fields above are what the evaluator MODELS; several constructs
    /// (`conditions`, `validitySchedules`, item `conditions`, `delegateAdmin`,
    /// a mask's `conditionExpr`) are reduced to content-free
    /// [`Unmodelled`] markers, so two bodies that differ only in that content
    /// parse to EQUAL typed fields. A caller asking "did this policy change?"
    /// must compare this value (map equality is order-insensitive, so `jsonb`
    /// key reordering is not a change), never the typed fields alone. Not read
    /// by the evaluator.
    pub source: serde_json::Value,
}

/// One resource-type selector inside a policy's `resources` (or
/// `additionalResources`) map — e.g. `"catalog": {"values": ["hive"], ...}`.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct ResourceLevel {
    #[serde(default, deserialize_with = "null_as_default")]
    pub values: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default", rename = "isExcludes")]
    pub is_excludes: bool,
    #[serde(default, deserialize_with = "null_as_default", rename = "isRecursive")]
    pub is_recursive: bool,
}

/// One entry of a policy item's `accesses` list — e.g.
/// `{"type": "select", "isAllowed": true}`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct AccessEntry {
    /// `RangerPolicyItemAccess.setType(null)` substitutes `""`, and an
    /// ABSENT `type` leaves the constructor's own `""` — so neither is an
    /// error to Ranger, and neither may be one here. An empty access type
    /// matches no request (`equalsIgnoreCase` against a concrete type), which
    /// is a decision, not a refusal.
    #[serde(default, deserialize_with = "null_as_default", rename = "type")]
    pub access_type: String,
    /// `setIsAllowed(null)` substitutes `Boolean.TRUE`, as does an absent key.
    #[serde(
        default = "default_true",
        deserialize_with = "null_as_true",
        rename = "isAllowed"
    )]
    pub is_allowed: bool,
}

/// One item of any of the six policy-item buckets
/// (`policyItems`/`denyPolicyItems`/`allowExceptions`/`denyExceptions`/
/// `dataMaskPolicyItems`/`rowFilterPolicyItems`). `data_mask_info` and
/// `row_filter_info` are populated only for the mask/row-filter buckets.
///
/// Deliberately does NOT carry `delegateAdmin` or item-level `conditions`:
/// their presence changes the decision but this reader does not evaluate
/// either, so they are surfaced only through the owning policy's
/// [`RangerPolicy::unmodelled`] ([`Unmodelled::DelegateAdmin`] /
/// [`Unmodelled::ItemConditions`]), not stored redundantly here.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolicyItem {
    pub users: Vec<String>,
    pub groups: Vec<String>,
    pub roles: Vec<String>,
    pub accesses: Vec<AccessEntry>,
    /// **Always `Some` on a `dataMaskPolicyItems` entry**, and `None`
    /// everywhere else.
    ///
    /// SP4d-1 Task 7, finding F2: Ranger deserialises the download with
    /// **Jackson**, not Gson — `JsonUtils.getMapper()` is a
    /// `com.fasterxml.jackson.databind.ObjectMapper` and neither
    /// `ranger-plugins-common` jar carries a single Gson class. The library
    /// is the mechanism, not a footnote: Jackson binds through the SETTER,
    /// and `RangerDataMaskPolicyItem.setDataMaskInfo` substitutes
    /// `new RangerPolicyItemDataMaskInfo()` for a null argument (`ifnonnull`
    /// at offset 2, `new` at 5). The no-arg constructor routes through the
    /// same setter via the all-args one, so BOTH paths — key absent, key
    /// explicitly null — end at a non-null empty object, never null.
    ///
    /// Real Gson would predict the opposite: it assigns fields directly and
    /// bypasses setters, which would leave `dataMaskInfo` null and make the
    /// guard below FIRE. Naming the wrong library here would invert the
    /// finding. The `ifnull`
    /// guard at the top of
    /// `RangerDefaultDataMaskPolicyItemEvaluator.updateAccessResult`
    /// therefore never fires, and such an item DETERMINES the result with a
    /// null mask type instead of standing aside. Reading the wire format
    /// literally said the opposite, and the differential gate caught it.
    pub data_mask_info: Option<MaskInfo>,
    /// **Always `Some` on a `rowFilterPolicyItems` entry**, and `None`
    /// everywhere else — `RangerRowFilterPolicyItem.setRowFilterInfo` makes
    /// the identical substitution.
    ///
    /// The BEHAVIOUR does not follow the mask path, though, and assuming it
    /// did would be the same mistake in the opposite direction:
    /// `RangerDefaultRowFilterPolicyItemEvaluator.updateAccessResult` guards
    /// its `policyEvaluator.updateAccessResult(...)` call with
    /// `if (result.getFilterExpr() != null)` (offsets 40-44), which the mask
    /// evaluator has no equivalent of. So a row-filter item whose
    /// `filterExpr` is null determines NOTHING and the search continues —
    /// see [`RowFilterInfo::filter_expr`].
    pub row_filter_info: Option<RowFilterInfo>,
}

/// A `dataMaskPolicyItems[].dataMaskInfo` object.
///
/// Note what [`PolicyItem::data_mask_info`] says: on a `dataMaskPolicyItems`
/// entry this object is ALWAYS present, because Ranger's model class
/// substitutes an empty one for a missing or null `dataMaskInfo`. An empty one
/// is what this struct's [`Default`] is.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct MaskInfo {
    /// `RangerPolicyItemDataMaskInfo.getDataMaskType()`, which
    /// `updateAccessResult` passes STRAIGHT to
    /// `RangerAccessResult.setMaskType` with no null check. `None` is
    /// Ranger's null and is what the host reads as "no mask type"; `Some("")`
    /// is an explicit empty string, which is NOT null and which the host
    /// reads as a mask type of `""`. Folding the two onto `String` would lose
    /// the difference, and `RangerAccessResult.getMaskType()` reports it.
    #[serde(default, deserialize_with = "null_as_default", rename = "dataMaskType")]
    pub mask_type: Option<String>,
    #[serde(default, rename = "valueExpr")]
    pub value_expr: Option<String>,
    /// Ranger's `RangerPolicyItemDataMaskInfo.conditionExpr` — a
    /// conditional-masking expression. Not in the writer's shape and not
    /// evaluated here; a non-empty value is reported via
    /// [`Unmodelled::MaskConditionExpr`].
    #[serde(default, rename = "conditionExpr")]
    pub condition_expr: Option<String>,
}

/// A `rowFilterPolicyItems[].rowFilterInfo` object.
///
/// `filter_expr` is an `Option` because Ranger distinguishes the two states:
/// `RangerDefaultRowFilterPolicyItemEvaluator`'s constructor sets its
/// `rowFilterExpr` field to null when `getFilterExpr()` is null (the
/// `ifnull 67` at offset 33), so `updateAccessResult` never calls
/// `setFilterExpr` and the guard `if (result.getFilterExpr() != null)`
/// fails — the item determines nothing. An explicit empty string is NOT
/// null and does mark the result. Folding both onto `""` would erase that.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct RowFilterInfo {
    #[serde(default, rename = "filterExpr")]
    pub filter_expr: Option<String>,
}

/// Which of a policy's six item lists a [`PolicyItem`]-scoped
/// [`Unmodelled`] variant came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemBucket {
    PolicyItems,
    DenyPolicyItems,
    AllowExceptions,
    DenyExceptions,
    DataMaskPolicyItems,
    RowFilterPolicyItems,
}

/// What the envelope's `tagPolicies` block was measured to carry, for the
/// tiers that are NOT provably inert.
///
/// # The tier this enum cannot express, and why
///
/// A block present with an **empty** `policies` list is inert, so it is not
/// an [`Unmodelled`] construct at all and there is no variant for it here.
/// Measured with `javap -c` on `ranger-plugins-common` 2.5.0 and 2.8.0. Both
/// `RangerPolicyEngineImpl.evaluateTagPolicies` bodies are 256 instructions
/// and agree on every offset and branch target up to 604. Exactly three
/// differences, none behavioural: the `commons-lang` → `commons-lang3` move in
/// `StringUtils.isNotEmpty`/`equals` at offsets 214 and 285, and an `ldc` →
/// `ldc_w` at 617 inside the trailing debug-log block, which shifts the tail
/// one byte so the final `ifeq`/`return` land at 668 in 2.8.0 against 667 in
/// 2.5.0. Every offset cited below is therefore identical in both jars:
///
/// * Offsets 109-129 compute `evaluators`:
///   `RangerPolicyRepository.getLikelyMatchPolicyEvaluators(request, tags,
///   policyType, accessTime)`, or `null` if the tag repository itself is null.
///   **For a realistic empty block the repository IS built and the live path
///   is the empty LIST, not the null repository.** `PolicyEngine.<init>`
///   builds it whenever `tagPolicies != null`, `getServiceName()` is non-empty
///   and `getServiceDef() != null` (offsets 471-500) — there is no
///   `getPolicies()` call in that gate — so the null-repository path is
///   reached only by a block that also omits its service name or its def.
/// * Offsets 131-136 branch on `CollectionUtils.isNotEmpty(evaluators)` with
///   `ifeq 582`, jumping past the whole loop. `isNotEmpty` is false for the
///   empty list and for null alike, so both paths land here.
/// * Every write to the SHARED result — `setAuditResultFrom` at 508,
///   `setAccessResultFrom` at 531 and 557, `incrementEvaluatedPoliciesCount`
///   at 458 — is inside that loop.
///
/// One precision that matters if anyone re-derives this: offset 582 is not a
/// bare `return`. It reads `result.getIsAllowed()`, branches `ifeq 596` at
/// 587, and on the fall-through calls `setIsAccessDetermined(true)` — the
/// `invokevirtual` at **593** (590 is the `aload 5` that begins the guarded
/// block). That write cannot change anything on this path. The result is the
/// one `createAccessResult` produced at caller offset 112 with `isAllowed`
/// false, and the only block that can flip it before the call — the super-user
/// short-circuit at caller offsets 133-165 — already calls
/// `setIsAccessDetermined(true)` itself at 143. So the write is either
/// unreached or idempotent, and the block is inert either way.
///
/// The block's own `serviceConfig` is also safely ignored here.
/// `ServicePolicies$TagPolicies` does carry a `serviceConfig` map in both
/// jars, so an otherwise-empty block could in principle smuggle one — but its
/// single call site is `RangerPolicyRepository` offset 279, feeding
/// `buildAuditPolicyEvaluators(Map)` at 282 and nothing else. The super-user
/// `RangerPolicyEngineImpl$ServiceConfig` is built from the TOP-LEVEL
/// `ServicePolicies.getServiceConfig()` (offsets 125-136), never from the tag
/// block — which is why [`Unmodelled::ServiceConfig`] guards the envelope's map
/// and this tier need not guard the block's.
///
/// This tier is the whole point of the distinction: `bind_trino_profile`
/// creates `{name}_tag` with **zero** policies and egret's
/// `/service/plugins/policies/download` embeds the block whenever the profile
/// names a `tagService`, so every auto-bound Trino profile lands there.
/// Refusing it would cost each of them an answer and buy nothing.
///
/// # Both remaining tiers can decide
///
/// [`TagPolicyBlock::Policies`] is the live case: `evaluateTagPolicies` runs
/// at offset 176 of `evaluatePoliciesForOneAccessTypeNoAudit`, BEFORE the
/// resource-policy loop that starts at 343/401, calls the full
/// `RangerPolicyEvaluator.evaluate` at 467 and copies a determined result onto
/// the shared result at 531. A tag policy can therefore DENY before any
/// policy in `policies` is consulted. As of 2026-09-29 exactly one live
/// profile is in this tier (service 52, `sp3c2-trino-live`), and its plugin
/// has ingested the block — `tests/fixtures/tag-policies-live.json` is that
/// download, captured from the pod's own policy cache.
///
/// [`TagPolicyBlock::Unreadable`] is the fail-safe: a block whose shape this
/// reader cannot count policies in has NOT been shown inert, and reporting it
/// as "zero policies" would fail open on any future wire change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagPolicyBlock {
    /// The block's `policies` list has this many entries, and the count is
    /// at least 1 — an empty list is inert and is not reported at all.
    Policies(usize),
    /// The block is present but is not an object carrying a `policies` array
    /// (or a null/absent one), so its policy count could not be established.
    Unreadable,
}

/// A construct this reader detected in a policy or envelope body but does
/// not model. Each variant names the field, and (where applicable) exactly
/// where it was found, so a caller — today, a human; a later task, the
/// evaluator — can decide to force an `Undecidable` outcome rather than
/// proceed on an incomplete picture of what the policy actually says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmodelled {
    /// Top-level `validitySchedules`: the policy is only in effect during a
    /// time window this reader does not evaluate.
    ValiditySchedules,
    /// Top-level `conditions`: a policy-wide condition (e.g. an IP-range
    /// restriction) gating every item in the policy.
    PolicyConditions,
    /// An item-level `conditions` list on one policy item.
    ItemConditions { bucket: ItemBucket, index: usize },
    /// `zoneName`: the policy is scoped to a Ranger security zone.
    ZoneName(String),
    /// The envelope's `securityZones` map is non-empty.
    SecurityZones,
    /// The envelope's `tagPolicies` carries something this reader cannot
    /// show inert: a whole second policy set, bound to the service's tag
    /// service, that CAN deny independently of every policy in `policies`.
    /// Dropping it would produce a confident allow that a live tag policy
    /// actually denies.
    ///
    /// **Not reported for every block that is present** — see
    /// [`TagPolicyBlock`] for the tier this variant does NOT fire on and the
    /// measurement behind that.
    TagPolicies(TagPolicyBlock),
    /// The envelope's `policyDeltas` is non-empty: this download is an
    /// INCREMENTAL update, so `policies` alone is partial (or absent).
    /// Evaluating `policies` as if it were the full set would fail open —
    /// "no policy matched" — for anything the delta didn't happen to touch.
    PolicyDeltas,
    /// The envelope's `serviceConfig` is non-empty.
    ///
    /// # ⚠ DO NOT NARROW THIS REFUSAL WITHOUT READING THIS
    ///
    /// **A policy download CAN carry `ranger.plugin.super.users`, and this
    /// refusal is the only thing standing between that fact and a confident
    /// wrong allow.** It was documented for a long time that super-users are
    /// plugin-side only; that was false.
    ///
    /// Measured with `javap -c` on both jars, identically at 2.5.0 and
    /// 2.8.0:
    ///
    /// * `RangerPolicyEngineImpl.<init>` offsets 125-136 construct
    ///   `RangerPolicyEngineImpl$ServiceConfig` from
    ///   **`ServicePolicies.getServiceConfig()`** — i.e. from this very map,
    ///   which arrives in the download.
    /// * That inner class reads the keys `ranger.plugin.super.users`,
    ///   `ranger.plugin.super.groups`, `ranger.plugin.service.admins` and
    ///   `ranger.plugin.audit.exclude.{users,groups,roles}`.
    /// * `RangerPolicyEngineImpl.isSuperUser(user, groups)` ORs
    ///   `serviceConfig.isSuperUser(user)` with
    ///   `pluginContext.getConfig().isSuperUser(user)` (and the same for
    ///   groups via `hasSuperGroup`). Either side alone is enough.
    /// * A super-user hit sets `isAllowed`, `isAccessDetermined`,
    ///   `policyId = -1`, `policyPriority = Integer.MAX_VALUE` and
    ///   `reason = "superuser"` BEFORE any policy is consulted.
    ///
    /// SP4d-1 Task 8's F1 is pressure to narrow envelope refusals so
    /// `explain` can answer for the profiles egret actually provisions.
    /// Narrowing `serviceConfig` while trusting
    /// [`crate::eval::EngineConfig::without_super_users`] would fail
    /// **OPEN**: that
    /// constructor records a measurement of the PLUGIN side only
    /// (`docs/en/plans/2026-09-28-sp4d-plugin-version-result.md`), and says
    /// nothing about the download side.
    ///
    /// Whoever narrows this must either model the six keys above or keep
    /// refusing any envelope whose `serviceConfig` carries them. Narrowing
    /// it to "refuse only the keys we know about" is not safe on its own
    /// either: `ranger.plugin.service.admins` also short-circuits, through
    /// `isServiceAdmin`.
    ServiceConfig,
    /// A mask item's `dataMaskInfo.valueExpr` contains a Ranger runtime
    /// expression this reader cannot resolve (a `${{ ... }}` request-time
    /// expression, or a bare `{TOKEN}` placeholder such as a CUSTOM mask's
    /// `{col}`) — see [`contains_expression_marker`].
    MaskExpressionMarker {
        bucket: ItemBucket,
        item_index: usize,
        expr: String,
    },
    /// A mask item's `dataMaskInfo.conditionExpr` is set.
    MaskConditionExpr {
        bucket: ItemBucket,
        item_index: usize,
    },
    /// A row-filter item's `rowFilterInfo.filterExpr` contains a Ranger
    /// runtime expression this reader cannot resolve — see
    /// [`contains_expression_marker`].
    RowFilterExpressionMarker {
        bucket: ItemBucket,
        item_index: usize,
        expr: String,
    },
    /// A `resources` (or `additionalResources[N]`) value contains a Ranger
    /// runtime expression. `ResourceMatcher` resolves these via its own
    /// `exprResolver` and `StringTokenReplacer`
    /// (`RangerAbstractResourceMatcher.OPTION_REPLACE_TOKENS`, default
    /// delimiters `{`/`}`) — this reader treats `values` as literal
    /// strings, so a marked value is not a literal this reader can match.
    /// `additional_index` is `None` for the primary `resources` map, or
    /// `Some(i)` for `additionalResources[i]`.
    ResourceValueExpressionMarker {
        additional_index: Option<usize>,
        resource_type: String,
        value: String,
    },
    /// A policy item's `users` list contains a Ranger runtime expression —
    /// most commonly one of Ranger's own current-request macros
    /// (`RangerPolicyEngine.USER_CURRENT = "{USER}"`,
    /// `RESOURCE_OWNER = "{OWNER}"`), which name the requesting user or the
    /// resource's owner rather than a literal username.
    UserExpressionMarker {
        bucket: ItemBucket,
        index: usize,
        user: String,
    },
    /// An access entry has `isAllowed: false` — meaningful only inside an
    /// "allow" bucket (`policyItems`/`allowExceptions`/
    /// `dataMaskPolicyItems`/`rowFilterPolicyItems`), where list membership
    /// alone would otherwise imply allow; the writer never emits this.
    ExplicitDeny {
        bucket: ItemBucket,
        index: usize,
        access_type: String,
    },
    /// A policy item has `delegateAdmin: true`.
    DelegateAdmin { bucket: ItemBucket, index: usize },
}

fn default_true() -> bool {
    true
}

/// Read a field that may be JSON `null`, as Ranger reads it.
///
/// SP4d-1 Task 7, finding F3 — the sweep behind F2, widened. Ranger reads
/// the download with Jackson (`JsonUtils.getMapper()` is a
/// `com.fasterxml.jackson.databind.ObjectMapper`). A key that is ABSENT is
/// never bound at all and keeps whatever the class's constructor left there,
/// and `RangerPolicy`, `RangerPolicyItem`, `RangerPolicyResource` and
/// `RangerPolicyItemAccess` all normalise in their constructors
/// (`nullSafeList` for every list, `Boolean.FALSE` / `Boolean.TRUE` / `0` /
/// `""` for the scalars). An EXPLICIT `null` is bound through the setter,
/// which for most of these fields substitutes the same default the
/// constructor did, and is read back through the same getter — so Ranger
/// treats "absent" and "null" identically. Measured with
/// `JsonUtils.jsonToObject` on both shapes rather than derived from the
/// binding rules.
///
/// Plain `#[serde(default)]` covers only the absent case: `null` makes serde
/// fail with `invalid type: null`. A reader that REFUSES a document the live
/// plugin enforces has no answer exactly where one is needed, so every field
/// whose Ranger default is `Default::default()` goes through here.
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: serde::Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// [`null_as_default`] for the two booleans whose Ranger default is TRUE:
/// `RangerPolicy.setIsEnabled(null)` and
/// `RangerPolicyItemAccess.setIsAllowed(null)` both substitute
/// `Boolean.TRUE`.
fn null_as_true<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(true))
}

/// True if `s` contains a Ranger runtime expression this reader cannot
/// resolve. Two distinct syntaxes, both checked:
///
/// 1. **Ranger's own request-time expression**, `${{ ... }}` (non-greedy
///    inner match) — exactly `RangerRequestExprResolver.PATTERN`
///    (`\$\{\{(?<expr>.*?)\}\}`), verified identical in `ranger-plugins-common`
///    2.5.0 and 2.8.0 bytecode (fix round 1). Its holders are
///    `RangerDefaultDataMaskPolicyItemEvaluator`,
///    `RangerDefaultRowFilterPolicyItemEvaluator`, and `ResourceMatcher` —
///    exactly the three surfaces this function is applied to
///    (`dataMaskInfo.valueExpr`, `rowFilterInfo.filterExpr`, and a
///    resource/`additionalResources` `values` entry). Examples:
///    `${{USER}}`, `${{USER.dept}}`, `${{IS_IN_GROUP('finance')}}`.
/// 2. **A bare `{TOKEN}` placeholder** — a run of ASCII alphanumerics/`_`
///    between literal braces not already matched by rule 1 — covering a
///    CUSTOM mask's `{col}` and Ranger's current-request macros `{USER}` /
///    `{OWNER}` when not wrapped in the `${{ }}` form.
///
/// Checked in that order because `${{` starts with the same `{` byte a
/// naive `{TOKEN}` scan would otherwise try to treat as its own opener.
fn contains_expression_marker(s: &str) -> bool {
    contains_ranger_request_expr(s) || contains_braced_token(s)
}

/// `RangerRequestExprResolver.hasExpressions(String)`:
/// `PATTERN.matcher(s).find()` with `PATTERN = \$\{\{(?<expr>.*?)\}\}`,
/// read from the `static {}` block of both the 2.5.0 and the 2.8.0 jar.
///
/// One known widening, in the SAFE direction: Java compiles that pattern
/// without `DOTALL`, so its `.` cannot cross a newline, while this scan
/// only looks for a later `}}` anywhere in the rest of the string. So
/// `"${{\nfoo}}"` is an expression here and a literal to Ranger. The match
/// set here strictly CONTAINS Java's, which can only produce a spurious
/// `Undecidable` — never a confident wrong mask.
///
/// This is the ONE predicate that decides whether Ranger resolves a mask
/// value, a mask condition or a row-filter expression at request time:
/// `RangerDefaultDataMaskPolicyItemEvaluator` and
/// `RangerDefaultRowFilterPolicyItemEvaluator` construct a resolver only
/// `if (StringUtils.isNotBlank(expr) && RangerRequestExprResolver.hasExpressions(expr))`
/// and otherwise report the string verbatim.
///
/// It is deliberately NARROWER than [`contains_expression_marker`], which
/// also flags a bare `{TOKEN}`. `{TOKEN}` expansion is
/// `StringTokenReplacer`, referenced in the whole 2.5.0 jar only by
/// `ResourceMatcher`, `RangerAbstractPolicyEvaluator` and `PolicyEngine`
/// — i.e. by resource matching, never by the mask or row-filter evaluator.
/// So a CUSTOM mask's `{col}` is a literal as far as Ranger's decision is
/// concerned (the host plugin's `maskTypes[].transformer` substitutes it
/// afterwards), and reporting it verbatim is Ranger's own answer, not a
/// guess. Using the broader predicate for a mask would make every CUSTOM
/// mask undecidable.
pub fn has_request_expression(s: &str) -> bool {
    contains_ranger_request_expr(s)
}

/// `JavaScriptEdits.hasDoubleBrackets(String)`:
/// `StringUtils.contains(s, "[[") && StringUtils.contains(s, "]]")` — both
/// substrings, identical in 2.5.0 and 2.8.0.
///
/// `RangerDefaultRowFilterPolicyItemEvaluator`'s constructor REWRITES the
/// filter expression when this holds, via `replaceDoubleBrackets`, whose
/// `DOUBLE_BRACKET_REGEX` is
/// `\[\[([}{\$"a-zA-Z0-9_.\[\]]+)(\,['\"](.+?)['\"])*\]\]` and which turns
/// `[[expr,'delim']]` into `expr.split("delim")`. This crate carries no
/// regex engine and does not reproduce the rewrite, so a row filter for
/// which this holds is reported as undecidable rather than echoed back
/// unrewritten.
pub fn has_double_brackets(s: &str) -> bool {
    s.contains("[[") && s.contains("]]")
}

/// Rule 1 of [`contains_expression_marker`]: `${{ ... }}`. Byte comparisons
/// only (never a `&str` slice boundary risk on the scanning index itself);
/// the one slice taken, `s[i + 3..]`, starts immediately after the ASCII
/// `{` at `i + 2`, which is always a valid `char` boundary.
fn contains_ranger_request_expr(s: &str) -> bool {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i + 3 <= n {
        if bytes[i] == b'$'
            && bytes[i + 1] == b'{'
            && bytes[i + 2] == b'{'
            && s[i + 3..].contains("}}")
        {
            return true;
        }
        i += 1;
    }
    false
}

/// Rule 2 of [`contains_expression_marker`]: a bare `{TOKEN}`. Fix round 1:
/// the scan now advances by exactly one byte per iteration regardless of
/// whether a candidate `{...}` span was accepted or rejected — the
/// original version jumped past a rejected span's closing `}`, so a `{`
/// nested inside a rejected span (e.g. the inner `{USER}` in
/// `"{a-b{USER}}"`, or in `"concat('x{',{USER})"`) was never examined and
/// the marker was missed.
fn contains_braced_token(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(rel) = s[i + 1..].find('}') {
                let token = &s[i + 1..i + 1 + rel];
                if !token.is_empty()
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                {
                    return true;
                }
            } else {
                // No '}' anywhere in the remainder of the string, so no
                // '{' from here on can ever close — safe to stop entirely.
                break;
            }
        }
        i += 1;
    }
    false
}

// ── Deserialize: raw wire shapes, converted into the public types above ───
//
// `RangerPolicy` and `ServicePolicySet` implement `Deserialize` by hand
// (deserializing into a private `Raw*` struct that mirrors the JSON field
// names exactly, then converting) so the conversion step can detect and
// collect `Unmodelled` constructs in the same pass that builds the typed
// value — there is no separate "validate after the fact" step to
// accidentally skip.

#[derive(Deserialize)]
struct RawServicePolicySet {
    #[serde(default, rename = "serviceName")]
    service_name: String,
    #[serde(default, rename = "serviceId")]
    service_id: Option<i64>,
    #[serde(default, rename = "policyVersion")]
    policy_version: Option<i64>,
    #[serde(default, deserialize_with = "null_as_default")]
    policies: Vec<RangerPolicy>,
    #[serde(default, rename = "serviceDef")]
    service_def: serde_json::Value,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "securityZones"
    )]
    security_zones: serde_json::Map<String, serde_json::Value>,
    /// A whole second `TagPolicies`-shaped object when the service has tag
    /// policies. Not further typed here (see `Unmodelled::TagPolicies`);
    /// `Value::Null` means the key was absent, which is the common case.
    #[serde(default, rename = "tagPolicies")]
    tag_policies: serde_json::Value,
    #[serde(default, deserialize_with = "null_as_default", rename = "policyDeltas")]
    policy_deltas: Vec<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "serviceConfig"
    )]
    service_config: serde_json::Map<String, serde_json::Value>,
}

impl<'de> Deserialize<'de> for ServicePolicySet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawServicePolicySet::deserialize(deserializer)?;
        let mut unmodelled = Vec::new();
        if !raw.security_zones.is_empty() {
            unmodelled.push(Unmodelled::SecurityZones);
        }
        // The block's policies, read rather than counted. `from_value` is
        // all-or-nothing, so a `policies` array this reader cannot bind comes
        // back EMPTY — which is why `classify_tag_policies` is handed the
        // result and reports `Unreadable` rather than "zero policies" for it.
        let tag_policies: Vec<RangerPolicy> =
            serde_json::from_value(raw.tag_policies["policies"].clone()).unwrap_or_default();
        if let Some(block) = classify_tag_policies(&raw.tag_policies, tag_policies.len()) {
            unmodelled.push(Unmodelled::TagPolicies(block));
        }
        if !raw.policy_deltas.is_empty() {
            unmodelled.push(Unmodelled::PolicyDeltas);
        }
        if !raw.service_config.is_empty() {
            unmodelled.push(Unmodelled::ServiceConfig);
        }
        // The block's own `serviceDef`, kept rather than dropped. `Value`
        // indexing yields `Null` for a non-object or a missing key, which is
        // exactly the "the envelope did not say" signal the enricher gate
        // treats as unknown.
        let tag_service_def = raw.tag_policies["serviceDef"].clone();
        Ok(ServicePolicySet {
            service_name: raw.service_name,
            service_id: raw.service_id,
            policy_version: raw.policy_version,
            policies: raw.policies,
            service_def: raw.service_def,
            tag_service_def,
            tag_policies,
            unmodelled,
        })
    }
}

/// Classify an envelope's raw `tagPolicies` value into the tier it belongs
/// to, returning `None` for the one tier that is provably inert.
///
/// See [`TagPolicyBlock`] for the `javap` measurement behind each arm. The
/// asymmetry is deliberate: "absent" and "present with no policies" are the
/// only shapes this function is willing to call inert, and everything else —
/// including a shape it simply does not recognise — is reported.
///
/// `read` is how many of those policies [`ServicePolicySet::tag_policies`]
/// actually bound. A `policies` array the reader could not bind is a shape it
/// does not understand, so it is `Unreadable` rather than a count nobody can
/// evaluate — the same asymmetry one level down.
fn classify_tag_policies(raw: &serde_json::Value, read: usize) -> Option<TagPolicyBlock> {
    // Absent. `Value::Null` is what `#[serde(default)]` leaves behind for a
    // missing key AND what an explicit `"tagPolicies": null` deserializes to;
    // Ranger's `ServicePolicies.getTagPolicies()` is null for both, so the
    // plugin builds no tag repository and `evaluators` is null at offset 115.
    if raw.is_null() {
        return None;
    }
    let Some(obj) = raw.as_object() else {
        return Some(TagPolicyBlock::Unreadable);
    };
    match obj.get("policies") {
        // Present with an empty (or absent, or null) policy list: the inert
        // tier. `getLikelyMatchPolicyEvaluators` has nothing to return and
        // `CollectionUtils.isNotEmpty` is false for null and for empty alike.
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(policies)) if policies.is_empty() => None,
        Some(serde_json::Value::Array(policies)) if policies.len() == read => {
            Some(TagPolicyBlock::Policies(policies.len()))
        }
        Some(serde_json::Value::Array(_)) => Some(TagPolicyBlock::Unreadable),
        // A `policies` that is not a list is a shape this reader does not
        // understand, so it has not been shown inert.
        Some(_) => Some(TagPolicyBlock::Unreadable),
    }
}

#[derive(Deserialize)]
struct RawRangerPolicy {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default, deserialize_with = "null_as_default")]
    name: String,
    #[serde(default, deserialize_with = "null_as_default")]
    service: String,
    #[serde(default, deserialize_with = "null_as_default", rename = "policyType")]
    policy_type: i32,
    #[serde(
        default = "default_true",
        deserialize_with = "null_as_true",
        rename = "isEnabled"
    )]
    is_enabled: bool,
    #[serde(
        default = "default_true",
        deserialize_with = "null_as_true",
        rename = "isAuditEnabled"
    )]
    is_audit_enabled: bool,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "policyPriority"
    )]
    policy_priority: i32,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "isDenyAllElse"
    )]
    is_deny_all_else: bool,
    #[serde(default, deserialize_with = "null_as_default")]
    resources: BTreeMap<String, ResourceLevel>,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "additionalResources"
    )]
    additional_resources: Vec<BTreeMap<String, ResourceLevel>>,
    #[serde(default, deserialize_with = "null_as_default", rename = "policyItems")]
    policy_items: Vec<RawPolicyItem>,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "denyPolicyItems"
    )]
    deny_policy_items: Vec<RawPolicyItem>,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "allowExceptions"
    )]
    allow_exceptions: Vec<RawPolicyItem>,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "denyExceptions"
    )]
    deny_exceptions: Vec<RawPolicyItem>,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "dataMaskPolicyItems"
    )]
    data_mask_policy_items: Vec<RawPolicyItem>,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "rowFilterPolicyItems"
    )]
    row_filter_policy_items: Vec<RawPolicyItem>,
    /// Top-level `conditions` — Ranger's policy-wide condition list.
    #[serde(default, deserialize_with = "null_as_default")]
    conditions: Vec<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "validitySchedules"
    )]
    validity_schedules: Vec<serde_json::Value>,
    #[serde(default, rename = "zoneName")]
    zone_name: Option<String>,
}

#[derive(Deserialize)]
struct RawPolicyItem {
    #[serde(default, deserialize_with = "null_as_default")]
    users: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    groups: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    roles: Vec<String>,
    #[serde(default, deserialize_with = "null_as_default")]
    accesses: Vec<AccessEntry>,
    /// Item-level `conditions`.
    #[serde(default, deserialize_with = "null_as_default")]
    conditions: Vec<serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "null_as_default",
        rename = "delegateAdmin"
    )]
    delegate_admin: bool,
    // Both are normalised per bucket in `convert_item`, which is where
    // Ranger's own null substitution is reproduced. Read these raw fields as
    // "what the JSON said", never as "what Ranger has".
    #[serde(default, rename = "dataMaskInfo")]
    data_mask_info: Option<MaskInfo>,
    #[serde(default, rename = "rowFilterInfo")]
    row_filter_info: Option<RowFilterInfo>,
}

impl<'de> Deserialize<'de> for RangerPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let source = serde_json::Value::deserialize(deserializer)?;
        let raw = RawRangerPolicy::deserialize(&source).map_err(serde::de::Error::custom)?;
        let mut policy = RangerPolicy::from_raw(raw);
        policy.source = source;
        Ok(policy)
    }
}

impl RangerPolicy {
    fn from_raw(raw: RawRangerPolicy) -> Self {
        let mut unmodelled = Vec::new();

        if !raw.conditions.is_empty() {
            unmodelled.push(Unmodelled::PolicyConditions);
        }
        if !raw.validity_schedules.is_empty() {
            unmodelled.push(Unmodelled::ValiditySchedules);
        }
        if let Some(zone) = raw
            .zone_name
            .map(|z| z.trim().to_string())
            .filter(|z| !z.is_empty())
        {
            unmodelled.push(Unmodelled::ZoneName(zone));
        }

        scan_resources_for_markers(&raw.resources, None, &mut unmodelled);
        for (i, additional) in raw.additional_resources.iter().enumerate() {
            scan_resources_for_markers(additional, Some(i), &mut unmodelled);
        }

        let policy_items =
            convert_bucket(raw.policy_items, ItemBucket::PolicyItems, &mut unmodelled);
        let deny_policy_items = convert_bucket(
            raw.deny_policy_items,
            ItemBucket::DenyPolicyItems,
            &mut unmodelled,
        );
        let allow_exceptions = convert_bucket(
            raw.allow_exceptions,
            ItemBucket::AllowExceptions,
            &mut unmodelled,
        );
        let deny_exceptions = convert_bucket(
            raw.deny_exceptions,
            ItemBucket::DenyExceptions,
            &mut unmodelled,
        );
        let data_mask_policy_items = convert_bucket(
            raw.data_mask_policy_items,
            ItemBucket::DataMaskPolicyItems,
            &mut unmodelled,
        );
        let row_filter_policy_items = convert_bucket(
            raw.row_filter_policy_items,
            ItemBucket::RowFilterPolicyItems,
            &mut unmodelled,
        );

        RangerPolicy {
            id: raw.id,
            name: raw.name,
            service: raw.service,
            policy_type: raw.policy_type,
            is_enabled: raw.is_enabled,
            is_audit_enabled: raw.is_audit_enabled,
            policy_priority: raw.policy_priority,
            is_deny_all_else: raw.is_deny_all_else,
            resources: raw.resources,
            additional_resources: raw.additional_resources,
            policy_items,
            deny_policy_items,
            allow_exceptions,
            deny_exceptions,
            data_mask_policy_items,
            row_filter_policy_items,
            unmodelled,
            source: serde_json::Value::Null,
        }
    }
}

fn scan_resources_for_markers(
    resources: &BTreeMap<String, ResourceLevel>,
    additional_index: Option<usize>,
    unmodelled: &mut Vec<Unmodelled>,
) {
    for (resource_type, level) in resources {
        for value in &level.values {
            if contains_expression_marker(value) {
                unmodelled.push(Unmodelled::ResourceValueExpressionMarker {
                    additional_index,
                    resource_type: resource_type.clone(),
                    value: value.clone(),
                });
            }
        }
    }
}

fn convert_bucket(
    items: Vec<RawPolicyItem>,
    bucket: ItemBucket,
    unmodelled: &mut Vec<Unmodelled>,
) -> Vec<PolicyItem> {
    items
        .into_iter()
        .enumerate()
        .map(|(index, raw)| build_policy_item(raw, bucket, index, unmodelled))
        .collect()
}

fn build_policy_item(
    raw: RawPolicyItem,
    bucket: ItemBucket,
    index: usize,
    unmodelled: &mut Vec<Unmodelled>,
) -> PolicyItem {
    if !raw.conditions.is_empty() {
        unmodelled.push(Unmodelled::ItemConditions { bucket, index });
    }
    if raw.delegate_admin {
        unmodelled.push(Unmodelled::DelegateAdmin { bucket, index });
    }
    for access in &raw.accesses {
        if !access.is_allowed {
            unmodelled.push(Unmodelled::ExplicitDeny {
                bucket,
                index,
                access_type: access.access_type.clone(),
            });
        }
    }
    for user in &raw.users {
        if contains_expression_marker(user) {
            unmodelled.push(Unmodelled::UserExpressionMarker {
                bucket,
                index,
                user: user.clone(),
            });
        }
    }
    if let Some(mask_info) = &raw.data_mask_info {
        if mask_info
            .value_expr
            .as_deref()
            .is_some_and(contains_expression_marker)
        {
            unmodelled.push(Unmodelled::MaskExpressionMarker {
                bucket,
                item_index: index,
                expr: mask_info.value_expr.clone().unwrap_or_default(),
            });
        }
        if mask_info
            .condition_expr
            .as_deref()
            .is_some_and(|s| !s.is_empty())
        {
            unmodelled.push(Unmodelled::MaskConditionExpr {
                bucket,
                item_index: index,
            });
        }
    }
    if let Some(row_filter_info) = &raw.row_filter_info {
        if row_filter_info
            .filter_expr
            .as_deref()
            .is_some_and(contains_expression_marker)
        {
            unmodelled.push(Unmodelled::RowFilterExpressionMarker {
                bucket,
                item_index: index,
                expr: row_filter_info.filter_expr.clone().unwrap_or_default(),
            });
        }
    }

    // Ranger's own null substitution, reproduced at the layer that reads the
    // document rather than at the layer that acts on it: a `RangerPolicy` is
    // not the JSON, it is the JSON as JACKSON built it through these classes'
    // constructors and SETTERS, and those two readings differ. The setter is
    // the whole mechanism — `setDataMaskInfo(null)` substitutes an empty
    // object, which is why the line below reads an absent or null
    // `dataMaskInfo` as `MaskInfo::default()` rather than as `None`. Gson,
    // which assigns fields directly, would leave it null; neither
    // `ranger-plugins-common` jar contains Gson. Applied per
    // BUCKET because the substitution lives on the bucket's element class —
    // `RangerPolicyItem` (the four access buckets) declares neither field, so
    // a `dataMaskInfo` written into `policyItems` is dropped by Ranger and
    // must be dropped here.
    let (data_mask_info, row_filter_info) = match bucket {
        ItemBucket::DataMaskPolicyItems => (Some(raw.data_mask_info.unwrap_or_default()), None),
        ItemBucket::RowFilterPolicyItems => (None, Some(raw.row_filter_info.unwrap_or_default())),
        ItemBucket::PolicyItems
        | ItemBucket::DenyPolicyItems
        | ItemBucket::AllowExceptions
        | ItemBucket::DenyExceptions => (None, None),
    };

    PolicyItem {
        users: raw.users,
        groups: raw.groups,
        roles: raw.roles,
        accesses: raw.accesses,
        data_mask_info,
        row_filter_info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expression_marker_detects_braced_tokens_but_not_plain_text() {
        assert!(contains_expression_marker("substr({col},1,2)"));
        assert!(contains_expression_marker("owner = '{USER}'"));
        assert!(contains_expression_marker("{USER_IP}"));
        assert!(!contains_expression_marker("country = 'US'"));
        assert!(!contains_expression_marker(""));
        // An empty or unterminated brace pair is not a token.
        assert!(!contains_expression_marker("{}"));
        assert!(!contains_expression_marker("unterminated {col"));
    }

    /// Fix round 1 (review): `RangerRequestExprResolver.PATTERN`, verified
    /// identical in 2.5.0 and 2.8.0 bytecode via `javap`, is
    /// `\$\{\{(?<expr>.*?)\}\}` — not `{TOKEN}`. Every one of these was
    /// measured to return `false` (missed) before the fix.
    #[test]
    fn expression_marker_detects_dollar_brace_brace_family() {
        assert!(contains_expression_marker("${{USER}}"));
        assert!(contains_expression_marker("${{USER.dept}}"));
        assert!(contains_expression_marker("dept = '${{USER.dept}}'"));
        assert!(contains_expression_marker("${{IS_IN_GROUP('finance')}}"));
        assert!(contains_expression_marker("${{ USER_ATTRIBUTES }}"));
    }

    /// Fix round 1 (review): the pre-fix `{TOKEN}` scan jumped past a
    /// rejected span's closing `}`, so a `{` nested inside a rejected span
    /// was never examined. Both were measured to return `false` (missed)
    /// before the fix.
    #[test]
    fn expression_marker_detects_nested_braces_after_a_rejected_span() {
        assert!(contains_expression_marker("{a-b{USER}}"));
        assert!(contains_expression_marker("concat('x{',{USER})"));
    }
}
