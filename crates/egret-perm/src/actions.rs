//! The `Action` → Ranger access-type map (SP4a Task 4).
//!
//! Written against what the two service-defs actually declare, not against
//! memory or the plan's table — an access type Ranger's plugin does not know
//! is silently ignored: the policy compiles, deploys, and grants nothing.
//! Read with:
//!
//! ```text
//! python3 -c "
//! import json
//! for n in ('trino','starrocks'):
//!     d=json.load(open(f'crates/egret-policy/src/service-defs/{n}.json'))
//!     print(n, [a['name'] for a in d['accessTypes']])
//! "
//! ```
//!
//! `trino.json`'s `accessTypes` (2026-09-24): select, insert, create, drop,
//! delete, use, alter, grant, revoke, show, impersonate, execute,
//! read_sysinfo, write_sysinfo, all — **no `update`**.
//!
//! `starrocks.json`'s `accessTypes` (2026-09-24): grant, node, operate,
//! delete, drop, insert, select, alter, export, `update`, usage, plugin,
//! file, blacklist, repository, refresh, impersonate, create database,
//! create table, create view, create function, create global function,
//! create materialized view, create resource, create resource group, create
//! external catalog, create storage volume.
//!
//! `every_access_type_the_map_emits_is_declared_by_that_service_def`
//! (`tests/compile_grants.rs`) re-reads both files and checks every access
//! type this map emits against them, so the map can't drift from the
//! service-defs silently.

use crate::model::{Action, PrincipalKind};

/// The two engines SP4a's compiler targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineType {
    Trino,
    StarRocks,
}

/// Which decision an overlapping pair arbitrates — the `kind` of a
/// `rule_overlap` refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapKind {
    Mask,
    RowFilter,
}

impl OverlapKind {
    /// The wire spelling: `"mask"` / `"row_filter"`.
    pub fn as_str(self) -> &'static str {
        match self {
            OverlapKind::Mask => "mask",
            OverlapKind::RowFilter => "row_filter",
        }
    }
}

/// One member of an overlapping pair, as a steward finds it: a compiled
/// policy names its `perm.rules` row (`id` and the rule's own name); a
/// hand-written policy has no rule (`id: None`) and is named by its policy
/// name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverlapRule {
    pub id: Option<uuid::Uuid>,
    pub name: String,
}

/// The (engine type, Ranger plugin version) pairs `egret-ranger-eval` has been
/// read against: Trino at 2.5.0 and StarRocks at 2.8.0, and nothing else (user
/// ruling 2026-10-12). "Modelled" is a PAIR, never a version alone: the
/// evaluator's `PluginVersion` and `EngineType` are measured together, so a
/// Trino engine declaring 2.8.0 is not covered by the StarRocks measurement.
///
/// Spelled here, in the crate both `egret-engine-mgmt` (registering an external
/// engine) and `egret-perm-compile` (`engine_plugin_version`) already depend
/// on, so the answer the API reports (`rangerPluginVersionModelled`), the one
/// the migration gate proves on and the one `explain` evaluates at are ONE
/// predicate, [`ranger_plugin_is_modelled`].
pub const MODELLED_RANGER_PLUGINS: [(&str, &str); 2] = [("trino", "2.5.0"), ("starrocks", "2.8.0")];

/// Whether `engine_type` (`trino` / `starrocks`) at Ranger plugin `version` is a
/// modelled pair.
pub fn ranger_plugin_is_modelled(engine_type: &str, version: &str) -> bool {
    MODELLED_RANGER_PLUGINS.contains(&(engine_type, version))
}

/// What egret's `explain` and the SP4d-3 migration gate assume about an
/// engine's Ranger plugin. egret guarantees them for the engines it provisions
/// (its renderer sets them); for an external engine the admin's own plugin
/// configuration decides, and egret cannot check it. The plugin-config page and
/// `explain` quote these same sentences.
pub const EXTERNAL_PLUGIN_ASSUMPTIONS: [&str; 3] = [
    "The plugin has no super-users configured (no ranger.plugin.*.super.users), so every request is evaluated against the downloaded policies.",
    "The plugin has no group provider and no usersync, so a user belongs to no group beyond the one egret's policies name.",
    "Plugin audit is off, so egret's policy evaluation is the only decision record.",
];

/// `Trino@2.5.0 and StarRocks@2.8.0`, for sentences.
pub fn modelled_ranger_plugins_sentence() -> String {
    MODELLED_RANGER_PLUGINS
        .iter()
        .map(|(t, v)| format!("{}@{v}", if *t == "trino" { "Trino" } else { "StarRocks" }))
        .collect::<Vec<_>>()
        .join(" and ")
}

/// A grant the compiler cannot express against the target engine, or cannot
/// express *correctly* — every variant here exists because a silent
/// alternative would compile a policy that grants something other than what
/// the model states, with nothing anywhere to show it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompileError {
    /// `UnsupportedAction` is unreachable today — SP4a ships exactly `read`
    /// and `write`, and both map on both engines — but it stays in the
    /// error type because a later phase (SP4b/SP4c) may add an action, and
    /// a compiler that silently dropped one would grant less than the
    /// model says.
    #[error("action {action:?} has no access type declared on {engine:?}'s service-def")]
    UnsupportedAction { action: Action, engine: EngineType },
    #[error(
        "compiling rule {rule:?} would expand to {count} entries, over the ceiling of {ceiling}"
    )]
    ExpansionCeiling {
        rule: String,
        count: usize,
        ceiling: usize,
    },
    /// The per-CLEARANCE equivalent of [`Self::ExpansionCeiling`] — SP4c Task
    /// 2d. A clearance on StarRocks expands to one access deny per tagged
    /// column (D0b: that engine has no tag service, so there is nothing to
    /// short-circuit on and nothing to resolve the tag at query time), and
    /// unlike a row filter it does NOT dedup, because an access deny carries a
    /// column resource — so the emitted count is exactly N.
    ///
    /// **Named after the TAG rather than a rule**, for the reason
    /// [`Self::ProfileExpansionCeiling`] is named after the profile: a
    /// clearance is not a `perm.rules` row, so an operator handed
    /// "compiling rule X" would go looking through the rule list for something
    /// that is not there. The clearance's input is
    /// `classification_policies.tag_id`, and that is what this names.
    #[error(
        "compiling the clearance on tag {tag:?} would expand to {count} per-column denies, over \
         the ceiling of {ceiling}"
    )]
    ClearanceExpansionCeiling {
        tag: String,
        count: usize,
        ceiling: usize,
    },
    /// The per-rule [`Self::ExpansionCeiling`] bounds one rule; this bounds the
    /// profile's SUM across every rule and grant compiled for it — the gap
    /// SP4a Task 12 closes (see `egret-perm-compile::trigger::
    /// PROFILE_EXPANSION_CEILING`'s doc comment for the measurement). Named
    /// after the PROFILE and the running TOTAL, not a rule: an operator
    /// chasing "which rule is over its cap" when every rule sits under its
    /// own cap and the profile's SUM is what is over would find nothing —
    /// this refusal has to point at the profile itself.
    #[error(
        "recompiling profile {profile:?} would total {total} compiler-owned policies, over the \
         profile-total ceiling of {ceiling}"
    )]
    ProfileExpansionCeiling {
        profile: String,
        total: usize,
        ceiling: usize,
    },
    /// **The compiled set would be served with two mask (or two row-filter)
    /// policies that arbitrate the same request and arbitrate it differently**
    /// — SP4d debt Task 6, the WRITE side of
    /// `egret_ranger_eval`'s `UndecidableCause::EvalOrderAmbiguous`. Measured
    /// live as SP4d-1's finding F2
    /// (`docs/en/plans/2026-09-28-sp4d-1-live-result.md` §8): egret's own rule
    /// compiler put `country = 'US'` and `id > 2` on one table at one priority,
    /// and which rows a reader saw was decided by a tie-break the artifact does
    /// not express.
    ///
    /// **It is a whole-SET refusal, like [`Self::ProfileExpansionCeiling`] and
    /// unlike every per-rule variant above**, because no single `compile_rule`
    /// call can see the other rule's output. It therefore names the PROFILE as
    /// well: the trigger may be a workspace-membership write on a page that
    /// mentions neither rule.
    ///
    /// `refusal` is `egret_policy::UnitOverlap::sentence` with each compiled
    /// policy labelled by its RULE's name (SP4d-4 Task 3) — the same sentence
    /// the editor's 409 carries for the same shape, with the editor's own
    /// labels. The rule itself is `egret_policy::unit_overlaps` (which takes
    /// every within-download pair from `ambiguous_arbitrations_in`), spelled
    /// once; read `egret_policy::ambiguous_arbitration` for why an AGREEING
    /// tie is admitted.
    ///
    /// `kind`, `coordinate` and `rules` (exactly two; a hand-written policy has
    /// `id: None`) are what a route renders as the structured 409 body
    /// (`code: "rule_overlap"`); the Display stays one sentence.
    #[error("recompiling profile {profile:?} would emit an order-ambiguous pair: {refusal}")]
    AmbiguousMaskOrFilterArbitration {
        profile: String,
        refusal: String,
        kind: OverlapKind,
        coordinate: String,
        rules: Vec<OverlapRule>,
    },
    /// A `User` principal's `r#ref` is not the bare username the store is
    /// supposed to hold (`model::Principal`'s doc comment: "a username,
    /// never an email … written through unchanged"). The compiler does no
    /// I/O and so cannot resolve `users.username` itself (round 1's fix
    /// guessed a resolution by splitting on `@`, which can collide two
    /// distinct humans onto one engine principal — see the sibling crate
    /// `egret-perm-compile`'s `grants` module doc). Refusing is the only safe move
    /// left to a function that cannot look the real username up.
    // The format string can't spell the field name directly — `r#ref` is a
    // raw identifier, and Rust's format-string captures don't support those
    // — so the placeholder is bound explicitly to the field below.
    #[error(
        "user principal ref {value:?} is not usable as a bare username (empty, or contains \
         '@') — canonicalise at the authoring boundary, where `users.username` is readable, \
         not in the compiler",
        value = r#ref
    )]
    UnusableUserRef { r#ref: String },
    /// A principal kind this compiler cannot yet turn into Ranger users/
    /// groups/roles (`WorkspaceMembers` today — Task 6 expands it into the
    /// member list enumerated at compile time). Dropping it silently on an
    /// `include` fails closed (an empty item matches nobody) but on an
    /// `exclude` it fails OPEN: the subtract evaporates and the compiled
    /// policy grants more than the model states. Refusing both is the only
    /// answer that can't silently do the wrong thing in either position.
    #[error(
        "{kind:?} principals are not expanded by this compiler (Task 6) — compiling one would \
         either grant nobody (as an include) or silently drop an exclude, both wrong"
    )]
    UnexpandedPrincipalKind { kind: PrincipalKind },
    /// A rule's custom mask expression (`mask_type == "CUSTOM"`) fails the
    /// compiler's minimal syntax check: unbalanced parentheses, or no
    /// `{col}` token. Ranger never parses this string itself — it is spliced
    /// verbatim into the engine's SQL at query time — so a string that
    /// cannot even balance its parentheses would compile, deploy, and fail
    /// every query touching the masked column. A real SQL parser is SP5's;
    /// this check is deliberately the smallest one that still catches the
    /// obvious case (see the sibling crate `egret-perm-compile`'s `rules` module).
    #[error("rule {rule:?}'s custom mask expression does not parse: {expr:?}")]
    InvalidCustomMask { rule: String, expr: String },
    /// A rule names a `mask_type` the target engine's service-def does not
    /// declare under `dataMaskDef.maskTypes` (e.g. `MASK_SHOW_LAST_4`/
    /// `MASK_SHOW_FIRST_4`, which Trino's service-def declares and
    /// StarRocks's does not — read at compile time, not assumed). Emitting
    /// it anyway would compile and deploy a policy the plugin silently
    /// ignores, the same class of failure `UnsupportedAction` exists to
    /// prevent for access types.
    #[error("rule {rule:?}'s mask type {mask_type:?} is not declared by {engine:?}'s service-def")]
    UnsupportedMaskType {
        rule: String,
        mask_type: String,
        engine: EngineType,
    },
    /// A tag-selected rule targeting Trino with no `tag_service` given. The
    /// compiler has no registry I/O to resolve the profile's bound tag
    /// service itself; the caller must resolve and pass it.
    #[error("rule {rule:?} selects by tag but no tag_service was given for Trino")]
    MissingTagService { rule: String },
    /// A tag-selected rule with `resolved.tag_name == None`. Fixed in the
    /// round-1 review (Critical 1): the first signature of `compile_rule`
    /// took no resolved tag name at all, so the compiler stamped
    /// `Selector::Tag`'s `key` straight into the policy — but egret's own
    /// tag download pushes `tags.name`, a separate, mutable column
    /// (`egret-policy/src/store.rs:245` → `egret-policy-api/src/dto.rs:184`;
    /// `migrations/0080_tags.sql`), so a policy keyed on `key` deployed
    /// cleanly, was listed by Ranger, and matched no resource. Refusing here
    /// is the only correct move — falling back to the key is exactly the
    /// silent mis-stamp that caused the defect.
    #[error("rule {rule:?} selects by tag but no resolved tag name was given")]
    MissingTagName { rule: String },
    /// The embedded service-def JSON (`egret-policy::{trino,starrocks}_service_def`)
    /// failed to parse. Should be impossible — `egret-policy`'s own tests
    /// pin that the embedded `include_str!` parses — but this compiler
    /// returns `CompileError` for every other failure and a `.expect()`
    /// here would be the one path that panics instead.
    #[error("{engine:?}'s embedded service-def failed to parse: {detail}")]
    ServiceDefUnreadable { engine: EngineType, detail: String },
    /// A path-selected ROW-FILTER rule whose scope is not exactly
    /// catalog + schema + table (SP4a whole-branch review, Critical 1). Both
    /// engines' `rowFilterDef.resources` declares those three levels, every
    /// one `"mandatory": true`, and declares no `column` — so a row filter
    /// scoped to a column carries a resource the definition cannot match, and
    /// one scoped above the table omits a level the definition requires.
    /// Either way Ranger accepts the policy, egret lists it as live, and it
    /// filters nothing: every row is returned from a control whose purpose is
    /// to withhold rows. Refused where the author can still fix it rather than
    /// rendered into a different control than the one they wrote (see
    /// `egret-perm-compile`'s `rules::ensure_row_filter_scope_is_a_table`).
    #[error("rule {rule:?} cannot be compiled as a row filter: {reason}")]
    UnfilterableRowFilterScope { rule: String, reason: String },
    /// A `Scope` level is PRESENT but EMPTY (`Some("")`) — SP4d debt Task 12.
    ///
    /// # This is a privilege escalation, and it is a level SKIP the database
    /// could not see
    ///
    /// `perm.grants` has carried `CHECK (schema_name IS NOT NULL OR table_name
    /// IS NULL)` since `0154_perm_model.sql` because a path cannot skip a level.
    /// `schema_name = ''` IS that skip — in enforcement terms it is exactly "a
    /// table with no schema" — and `IS NOT NULL` cannot see it. Measured as a
    /// pair, which is what makes it a measurement rather than a probe that could
    /// not fail: `(schema_name = '', table_name = 'orders')` was **accepted**,
    /// while `(schema_name = NULL, table_name = 'orders')` was **rejected** by
    /// `grants_check` on the same connection.
    ///
    /// What the empty level then does:
    /// `crate::grants::scope_to_resources` copies the level through verbatim, so
    /// the compiled policy carries `"schema": {"values": [""]}`;
    /// `RangerAbstractResourceMatcher.init()` **drops an empty policy value**
    /// before a matcher is built for it; a level left with no matchers is
    /// `isMatchAny`. Measured through `egret_ranger_eval::resource::match_resource`
    /// (`tests/compile_grants.rs`,
    /// `an_empty_scope_level_would_be_enforced_in_every_schema`): the artifact for
    /// `sales.orders` with `sales` replaced by `""` matches a request for
    /// `finance.orders.amount` at `MatchType::Self_`, while the unedited artifact
    /// matches it at `MatchType::None`. A grant written for one schema was
    /// enforced across the whole catalog, reachable by any `policy:update` caller.
    ///
    /// # `'*'` is NOT refused, deliberately
    ///
    /// A grant scoped to a whole catalog or schema is an ordinary thing a steward
    /// writes, `scope_to_resources` emits `*` itself at every level the scope does
    /// not name, and the compiled resources a steward reads are full of it. Only
    /// `''` is wrong — it is the spelling that LOOKS like a named level. ⚠ The
    /// consequence, measured in the same test and stated rather than hidden:
    /// `(schema_name = '*', table_name = 'orders')` reaches the SAME catalog-wide
    /// enforcement and stays legal, so this refusal closes the accidental door and
    /// not the deliberate one.
    ///
    /// The keyed shape is `value.is_empty()`, one-for-one with
    /// `migrations/0178_perm_scope_levels_are_named.sql`'s `<> ''`, because two
    /// guards that must agree written to different shapes has produced a defect in
    /// this programme four times. In particular this is NOT
    /// `egret_ranger_eval::resource::requests_all_values`: that predicate is the
    /// REQUEST-side rule and folds `""`, `"*"` and `null` together, and keying a
    /// scope refusal to it would refuse `*`.
    #[error(
        "scope level `{level}` is present but empty — a level is a resource NAME. Ranger's \
         `RangerAbstractResourceMatcher.init()` drops an empty policy value before building a \
         matcher for it, so the level becomes `isMatchAny` and the policy is enforced at EVERY \
         {level} under its parent rather than at the one named. Omit the level to scope to its \
         parent, or name one"
    )]
    EmptyScopeLevel { level: &'static str },
    /// A `Scope` level is a run of `*` — Ranger's "every value at this level",
    /// so the level names nothing — while a level BELOW it names something.
    /// SP4d debt Task 14, the other half of [`Self::EmptyScopeLevel`].
    ///
    /// # This is NOT "`*` is illegitimate"
    ///
    /// A grant scoped to a whole catalog or a whole schema is an ordinary thing
    /// a steward writes, `crate::grants::scope_to_resources` emits `*` ITSELF at
    /// every level the scope does not name, and the compiled resources a steward
    /// reads back are full of it. `('*','*')`, `('sales','orders','*')` and every
    /// other shape where a `*` has nothing named beneath it stay legal, and the
    /// tests name each one.
    ///
    /// # What IS wrong is a gap in the path, and the table already said so
    ///
    /// `0154_perm_model.sql` gave `perm.grants` AND `perm.rules`
    /// `CHECK (schema_name IS NOT NULL OR table_name IS NULL)` because "a path
    /// cannot skip a level: a column implies a table implies a schema".
    /// `schema_name = '*'` IS that skip — `*` names no schema, so `('*',
    /// 'orders')` is "a table with no schema", the exact scope `grants_check`
    /// refuses when it is spelled NULL. **Two guards in one table disagreed
    /// about whether that scope exists**, and the model had already decided it
    /// does not: `scope_to_resources`' `wildcard_from_here` latch renders `*` at
    /// every level below the first unnamed one, so no scope the compiler can
    /// emit ever carries a named level under an unnamed one. This refusal is
    /// that latch stated over the AUTHORED values.
    ///
    /// # The measurement, and why this half is the reachable one
    ///
    /// `tests/compile_grants.rs`'s
    /// `an_empty_scope_level_would_be_enforced_in_every_schema` compiles the
    /// legal grant for `sales.orders`, substitutes one token in the schema
    /// level's values, and measures with
    /// `egret_ranger_eval::resource::match_resource` against the real Trino
    /// service-def: `"sales"` (unedited) matches `finance.orders.amount` at
    /// `MatchType::None`, while `""` AND `"*"` both match it at
    /// `MatchType::Self_`. Identical catalog-wide reach.
    ///
    /// Unlike `""`, `"*"` is reachable from the browser — by code path, not by
    /// authorship: `frontend/src/components/perm/ScopePicker.tsx`'s three levels
    /// are free-text `<Input>`s whose `onChange` is `e.target.value ||
    /// undefined`, which maps `""` to `undefined` (that is why `""` never
    /// arrived from the UI) while `"*"` is truthy and passes straight through;
    /// `canPickTable = !!value.schema`, so typing `*` into Schema ENABLES the
    /// Table field, and Table's `onChange` clears only `column`.
    ///
    /// # A RUN of stars, not the single character
    ///
    /// `RangerAbstractResourceMatcher.buildResourceMatchers()` treats a pure run
    /// of `*` as match-any and clears every other matcher on the level;
    /// `egret_ranger_eval::resource::ValueMatcher::classify` reproduces it, and
    /// `classify("**")` is the same match-any as `classify("*")`. Keying this on
    /// the literal `"*"` would close one spelling and leave `"**"` reaching the
    /// identical enforcement from the identical field. `"ord*"` is deliberately
    /// NOT in the class: it is a prefix matcher that names a SET of schemas, so
    /// the level is named and a table under it is not a gap.
    ///
    /// # ⚠ NOT keyed to `egret_ranger_eval::resource::requests_all_values`
    ///
    /// That predicate is the REQUEST-side rule and folds `null`, `""` and `"*"`
    /// together. Keying a SCOPE refusal to it would refuse `'*'` outright —
    /// [`Self::EmptyScopeLevel`]'s doc records the same trap for the same
    /// reason. `""` and a run of stars are both match-any, but they earn
    /// DIFFERENT refusals here (`""` is refused at every level unconditionally;
    /// a run of stars only above a named level), so one predicate cannot serve
    /// both.
    #[error(
        "scope level `{wildcard}` is {wildcard_value:?}, which is Ranger's \"every value at this \
         level\" and so names no {wildcard} at all — while `{named}` below it names \
         {named_value:?}. That is a path with a gap in it, and this model does not express it: \
         `perm.grants` and `perm.rules` have both carried `CHECK (schema_name IS NOT NULL OR \
         table_name IS NULL)` since `0154_perm_model.sql` because a column implies a table \
         implies a schema, and a match-any level is that same unnamed level spelled differently. \
         Name the `{wildcard}`, or drop `{named}` to scope to every {named} under it"
    )]
    NamedLevelUnderWildcard {
        wildcard: &'static str,
        wildcard_value: String,
        named: &'static str,
        named_value: String,
    },
    /// A rule's `exempt` set names something a masking/row-filter Ranger
    /// item cannot express: a non-empty `exempt.exclude` (those items carry
    /// no allow/deny-exception block the way an access-policy item does —
    /// `MaskItemInput`/`RowFilterItemInput` in `ranger_json.rs` have no such
    /// field), or a `Role` principal (those same item types declare no
    /// `roles` bucket at all, unlike `AccessItemInput`). `reason` names
    /// which.
    #[error("rule {rule:?}'s exempt set cannot be compiled: {reason}")]
    UnexpressibleExemption { rule: String, reason: String },
    /// A database error encountered by a caller doing I/O AROUND the pure
    /// compiler — today, only `egret-perm-compile`'s `trigger::recompile_service`
    /// (SP4a Task 7), which loads the model and diffs `perm.compiled` before
    /// ever calling `compile_grant`/`compile_rule`. Carries the rendered
    /// message, not the raw `sqlx::Error` / store error, because
    /// `CompileError` is `Clone + PartialEq + Eq` (every other variant is
    /// built from data that is) and neither `sqlx::Error` nor
    /// `egret_perm::PermError`/`egret_policy::PolicyStoreError` are — the same
    /// reason `ServiceDefUnreadable` above stores `detail: String` rather than
    /// the parse error itself.
    #[error("database error: {0}")]
    Db(String),
}

impl CompileError {
    /// The structured 409 body of an order-ambiguous pair —
    /// `{error, code: "rule_overlap", rules, coordinate, kind}` — or `None` for
    /// any other error. Spelled HERE, once, so the perm routes
    /// (`egret_perm_compile::overlap::refusal_body`) and the engine bind and
    /// attach routes (`egret-engine-mgmt`) answer one body.
    pub fn rule_overlap_body(&self) -> Option<serde_json::Value> {
        let CompileError::AmbiguousMaskOrFilterArbitration {
            kind,
            coordinate,
            rules,
            ..
        } = self
        else {
            return None;
        };
        Some(serde_json::json!({
            "error": self.to_string(),
            "code": "rule_overlap",
            "rules": rules
                .iter()
                .map(|r| serde_json::json!({ "id": r.id, "name": r.name }))
                .collect::<Vec<_>>(),
            "coordinate": coordinate,
            "kind": kind.as_str(),
        }))
    }
}

/// The Ranger access types `a` compiles to on `e`. Every engine declares
/// `select` for `read`; `write` differs by engine because Trino's
/// service-def declares no `update` access type.
pub fn engine_accesses(a: Action, e: EngineType) -> Result<Vec<&'static str>, CompileError> {
    match (a, e) {
        (Action::Read, EngineType::Trino) => Ok(vec!["select"]),
        (Action::Read, EngineType::StarRocks) => Ok(vec!["select"]),
        (Action::Write, EngineType::Trino) => Ok(vec!["insert", "delete"]),
        (Action::Write, EngineType::StarRocks) => Ok(vec!["insert", "update", "delete"]),
    }
}
