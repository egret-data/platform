//! `compile_grant`: turns one `perm.grants` row (plus its principal
//! subtract) into a Ranger `AccessPolicyInput`.
//!
//! Two things a reader can't check by eye, pinned by
//! `tests/compile_grants.rs`:
//!
//! - **The principal subtract compiles to `allowExceptions`** (or
//!   `denyExceptions` for a deny grant). That four-way Ranger structure
//!   (items / denyItems / allowExceptions / denyExceptions) is a
//!   COMPILATION TARGET ONLY — it appears nowhere in `perm`'s model types,
//!   egret's API, or any UI-facing name (global constraint). `PrincipalSet`
//!   (`include` minus `exclude`) is the only shape a human or the OpenAPI
//!   schema ever sees; this function is where that subtract is realized as
//!   Ranger's exception block, and only here.
//! - **A `User` principal's `r#ref` is emitted verbatim, never rewritten.**
//!   Round 1 of this file split on `@` to turn an email into a guessed
//!   username. That is not egret's username rule. The real one —
//!   `ensure_username` (`crates/egret-auth/src/users.rs`) plus
//!   `migrations/0040_user_username.sql` — lowercases, replaces
//!   `[^a-z0-9_-]+` with `-`, and resolves collisions with a `-2`, `-3`, …
//!   suffix. A compiler with no store handle cannot reproduce that: it
//!   cannot see the collision suffix, and a naive split-on-`@` can compile
//!   two distinct humans (`bob` and `bob-2`, both slugging from the same
//!   local part) onto the SAME engine principal — an `exclude` of the
//!   second lands on the first, and the one meant to be excluded stays
//!   included. `model::Principal`'s doc comment already says refs are "a
//!   username, never an email … written through unchanged" — canonicalising
//!   here contradicted the store's own contract. So: this compiler performs
//!   NO canonicalisation. It emits `r#ref` byte-for-byte and refuses
//!   (`CompileError::UnusableUserRef`) a `User` ref that's empty or
//!   contains `@`, since either one is a sign the value reaching this
//!   function was never resolved to `users.username` in the first place. The
//!   I/O-capable half lives at the authoring boundary, where `users.username`
//!   is readable: `crate::principal_refs` (J-F3) checks every ref of every
//!   kind before a write is stored, rewrites a username to its stored
//!   spelling, and refuses an email or a user id with the username to use —
//!   it never silently rewrites one into the other.
//!
//! **Groups still compile through**, unchanged, even though D7
//! (`docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`) found
//! groups never reach the engine (no group provider on Trino, no usersync in
//! egret) — egret is not the only possible Ranger consumer, and the
//! situation may change. No claim of enforcement is implied or tested here;
//! see D7 §2-3 and `model::PrincipalKind`'s doc comment.
//!
//! **ROLES DO NOT, and that changed in SP4c Task 2b.** Round 1 bucketed a
//! `PrincipalKind::Role` into Ranger's `roles` alongside groups, on the same
//! "egret is not the only possible consumer" reasoning. D0
//! (`docs/en/plans/2026-09-28-sp4c-d0-result.md`) measured that the bucket is
//! never populated on the Trino path egret provisions — the roles document
//! comes from a `/service/roles/download/<svc>` route egret does not serve, so
//! `lastKnownRoleVersion` never leaves `-1`, and the plugin passes `userRoles`
//! as a hardcoded `null` — while `egret_perm::eval::matches_principal` matches
//! a role happily, so `explain` reported an access the engine did not grant.
//! Unlike the group case that divergence is now fixable, because
//! `access_grants` knows who holds a role: a `role` principal is expanded into
//! its holders' usernames before this file ever sees it
//! (`crate::members::expand_enumerated_principals`), and reaching
//! `principal_item` with one unexpanded is refused exactly as
//! `WorkspaceMembers` is — see [`Expansion`] for why the authoring boundary
//! still accepts one.
//!
//! **`WorkspaceMembers` principals are refused, not dropped.** Round 1
//! silently contributed nothing for a `WorkspaceMembers` principal, on the
//! theory that Task 6 (which expands it into the member list enumerated at
//! compile time, needing workspace-membership I/O this compiler does not
//! have) hadn't landed yet. That reasoning holds for `include` — an empty
//! item matches nobody, so the grant fails closed — but not for `exclude`:
//! dropping the only principal in an `exclude` set still produces a
//! non-empty exception list containing one EMPTY item, which Ranger matches
//! against nobody, so the subtract evaporates and the compiled policy grants
//! MORE than the model states. There is no position-dependent way to get
//! this right without the I/O this compiler doesn't have, so both positions
//! refuse via `CompileError::UnexpandedPrincipalKind`.

use std::collections::BTreeMap;

// `egret-policy` keeps `admin::ranger_json` module-private (D2: egret must
// never emit a tag *access* policy, and that module's tag builders sit next
// to the ones used here) and re-exports just these three struct types from
// `admin` itself.
use egret_policy::admin::{AccessItemInput, AccessPolicyInput, ResourceLevel};
use egret_policy::COMPILER_GRANT_PREFIX;

use egret_perm::{
    engine_accesses, CompileError, Effect, EngineType, Grant, Principal, PrincipalKind, Scope,
};

/// The one check every `User` ref gets before it is emitted, shared by
/// `principal_item` here and `crate::rules::exempt_principals` — the two
/// places this compiler ever buckets a `User` principal. A `User` ref is
/// emitted verbatim (see module doc comment) after being checked for the two
/// shapes that prove it was never resolved to a bare `users.username`: empty,
/// or containing `@`.
pub(crate) fn validate_user_ref(r#ref: &str) -> Result<(), CompileError> {
    if r#ref.is_empty() || r#ref.contains('@') {
        Err(CompileError::UnusableUserRef {
            r#ref: r#ref.to_string(),
        })
    } else if egret_core::engine_principals::is_reserved_engine_username(r#ref) {
        // View governance R5: what `egret-lineage` may do is decided where
        // egret serves a provisioned Trino engine's download
        // (`egret_policy::lineage_transform`), never by the model, so no grant
        // can widen it to a write and no rule can mask it.
        Err(CompileError::ReservedEnginePrincipal {
            r#ref: r#ref.to_string(),
        })
    } else {
        Ok(())
    }
}

/// Whether `crate::members::expand_enumerated_principals` has already run over
/// the principal set being bucketed. It decides ONE thing: what a `role`
/// principal means here.
///
/// **SP4c Task 2b, and the distinction is load-bearing.** A `role` grant is a
/// perfectly legal thing to author — SP4b's picker offers it — and it is
/// expanded into its holders at compile time. So the authoring boundary
/// ([`validate_grant_shape`], which runs before anything is stored and long
/// before anything is expanded) must ACCEPT one, while `compile_grant` — which
/// only ever runs after the expansion — must REFUSE one, because a `role` still
/// present there is a bug in the expansion rather than an authoring mistake,
/// and bucketing it into Ranger's `roles` would compile a grant that matches
/// nobody (module doc comment).
///
/// `WorkspaceMembers` is refused in BOTH positions, unchanged: no authoring
/// path may write one (they are produced only by `egret_perm::assignment`), so
/// there is no legal author to accommodate.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expansion {
    /// `compile_grant`: the principal set has been expanded, so an enumerated
    /// kind surviving here is an internal invariant violation.
    Done,
    /// `validate_grant_shape`: nothing has been expanded yet, so a `role` is
    /// simply an unexpanded-but-valid author's choice.
    NotYet,
}

/// Sort principals in `refs` into a Ranger item's `users`/`groups`/`roles`
/// buckets and set `effect`/`accesses` on it. A `WorkspaceMembers` principal —
/// and, once `expansion` is [`Expansion::Done`], a `Role` — refuses the whole
/// compile rather than being silently dropped (module doc comment).
fn principal_item(
    refs: &[Principal],
    effect: Effect,
    accesses: &[String],
    expansion: Expansion,
) -> Result<AccessItemInput, CompileError> {
    let mut users = Vec::new();
    let mut groups = Vec::new();
    for p in refs {
        match p.kind {
            PrincipalKind::User => {
                validate_user_ref(&p.r#ref)?;
                users.push(p.r#ref.clone());
            }
            PrincipalKind::Group => groups.push(p.r#ref.clone()),
            // Legal to author, expanded before any compile sees it.
            PrincipalKind::Role if expansion == Expansion::NotYet => {}
            PrincipalKind::Role | PrincipalKind::WorkspaceMembers => {
                return Err(CompileError::UnexpandedPrincipalKind { kind: p.kind });
            }
        }
    }
    Ok(AccessItemInput {
        users,
        groups,
        // **Always empty, deliberately** (SP4c Task 2b). Ranger's `roles`
        // bucket is never populated on the Trino path egret provisions (D0), so
        // a name written here matches nobody: in an item it grants nobody, and
        // in an exception block it excepts nobody and the deny stands for
        // everyone. Roles reach the engine as their expanded members instead.
        roles: Vec::new(),
        accesses: accesses.to_vec(),
        effect: effect.as_str().to_string(),
    })
}

/// The Ranger resource-level names the target engine's service-def declares
/// for a catalog/schema/table/column path, in that order. Trino calls the
/// second level `schema`; StarRocks calls it `database`
/// (`crates/egret-policy/src/service-defs/{trino,starrocks}.json`) — same
/// position in the hierarchy, different declared name, so the compiler must
/// pick the name per engine rather than hard-coding Trino's.
/// `tests/compile_grants.rs`'s
/// `the_scope_becomes_the_ranger_resource_levels_the_service_def_declares`
/// re-reads both files' `resources` arrays and checks these four names
/// against each one's declared name + parent chain, so this can't drift
/// from the service-defs silently the way the write-up in round 1 could
/// have (its Trino-only test never opened a service-def).
// `pub(crate)`: `crate::rules` (Task 5) reuses this and `level`/
// `scope_to_resources` rather than duplicating the per-engine hierarchy
// naming a second time.
pub(crate) fn resource_level_names(e: EngineType) -> [&'static str; 4] {
    match e {
        EngineType::Trino => ["catalog", "schema", "table", "column"],
        EngineType::StarRocks => ["catalog", "database", "table", "column"],
    }
}

pub(crate) fn level(value: String) -> ResourceLevel {
    ResourceLevel {
        values: vec![value],
        is_excludes: false,
        is_recursive: false,
    }
}

/// The Ranger resource value that means "every child at this level". A
/// policy's resources map uses it to say "this table, whatever the column"
/// in a way the plugin's matcher actually evaluates — see
/// [`scope_to_resources`] for why an ABSENT level is not the same thing.
pub(crate) const WILDCARD: &str = "*";

/// Which Ranger policy type a `resources` map is being rendered for. The
/// three do not share one resource hierarchy: an ACCESS policy is matched
/// against the full `catalog/schema/table/column` chain the service-def's
/// top-level `resources` array declares, a masking policy against
/// `dataMaskDef.resources` (also down to `column`), and a row-filter policy
/// against `rowFilterDef.resources`, which stops at `table`. They also do
/// not behave the same way when a level is missing — which is the whole
/// reason this enum exists rather than one map-builder for everybody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PolicyTarget {
    Access,
    Mask,
    RowFilter,
}

/// Render a `Scope` into the engine's `resources` map. `catalog_name` is the
/// name the caller already resolved from `scope.catalog_id` (the compiler
/// stays free of registry I/O, and does not itself check that `catalog_name`
/// actually names `scope.catalog_id` — that binding is the caller's
/// responsibility, same as the rest of the registry lookup it already owns).
///
/// # An ACCESS policy fills every level below the scope with `*`
///
/// **This is measured, and it replaces an assumption the live gate refuted.**
/// Round 1 of this file left an unnamed level OUT of the map entirely, on the
/// strength of a reading of Apache Ranger's
/// `RangerDefaultPolicyResourceMatcher.getMatchType()` (`agents-common/…/
/// policyresourcematcher/RangerDefaultPolicyResourceMatcher.java`): once the
/// matcher runs out of policy-side resource entries, deeper levels present on
/// the *request* were said to resolve to `MatchType.ANCESTOR`
/// unconditionally, so a policy stopping at `table` would cover every column
/// beneath it. That paragraph even flagged itself as **not verified against a
/// live plugin**. Task 10 verified it, and **it is false for access policies**
/// (`docs/en/plans/2026-09-24-permission-model-sp4a-live-result.md` §3, case
/// T1): against a real Trino 4.7 + Ranger plugin on kind, a grant of `read` on
/// a SCHEMA compiled to `{catalog, schema}` and produced
///
/// ```text
/// Access Denied: Cannot select from columns [v, id] in table or view sibling
/// ```
///
/// A table-level grant (`{catalog, schema, table}`) failed identically. The
/// same grant written down to `column: "*"` returned all four rows. The
/// ancestor match holds for the *catalog* check — the denial moved from
/// `Cannot access catalog` to `Cannot select from columns` — and does **not**
/// hold downward to the column-granular `select` Trino actually asks about.
/// A grant above column level therefore compiled cleanly, deployed cleanly,
/// was listed as live by egret, and granted nothing.
///
/// So an `Access` target now emits the FULL hierarchy the target engine's
/// service-def declares — `catalog/schema/table/column` on Trino (service-def
/// id 203, levels 10/20/30/40, every one of them `mandatory: true`),
/// `catalog/database/table/column` on StarRocks — with `WILDCARD` at every
/// level the scope does not name, and at every level below the first one it
/// does not name (a scope cannot name a table without naming its schema, and
/// if one ever did, `*` at the schema level is the only honest rendering of
/// "unspecified"). `tests/compile_grants.rs`'s
/// `the_scope_becomes_the_ranger_resource_levels_the_service_def_declares`
/// re-reads both service-defs and asserts the emitted map carries exactly the
/// declared chain, so this cannot drift back.
///
/// # A MASK or ROW_FILTER target keeps today's behaviour, deliberately
///
/// The same live gate measured both of these paths WORKING as they are
/// emitted today: T3b masked a column for real (`item` came back as `xxxx`)
/// and T4 removed rows for real (4 → 2, and the excluded rows probed absent),
/// both from a path selector that named every level down to its own target.
/// Nothing measured says a masking or row-filter policy needs — or tolerates
/// — a `*` at an unnamed level, and the design spec's own reading of Ranger
/// (§1, "masking and row-filter policies take no wildcards and no
/// multi-values": one column per policy item, items ordered, first match
/// wins) says the opposite. Widening them here would be swapping one
/// unverified assumption for another, in the direction that grants MORE than
/// the model states. The fix is applied exactly where the measurement demands
/// it and nowhere else; if a mask scoped above column level is ever wanted,
/// it needs its own live measurement first.
pub(crate) fn scope_to_resources(
    scope: &Scope,
    catalog_name: &str,
    e: EngineType,
    target: PolicyTarget,
) -> BTreeMap<String, ResourceLevel> {
    let names = resource_level_names(e);
    let named: [Option<&str>; 4] = [
        Some(catalog_name),
        scope.schema.as_deref(),
        scope.table.as_deref(),
        scope.column.as_deref(),
    ];

    let mut map = BTreeMap::new();
    match target {
        PolicyTarget::Access => {
            // Once a level is unnamed every level below it is unnamed too:
            // `*` from there down, never a named leaf hanging under a gap.
            let mut wildcard_from_here = false;
            for (i, name) in names.iter().enumerate() {
                let value = match named[i] {
                    Some(v) if !wildcard_from_here => v.to_string(),
                    _ => {
                        wildcard_from_here = true;
                        WILDCARD.to_string()
                    }
                };
                map.insert((*name).to_string(), level(value));
            }
        }
        PolicyTarget::Mask => {
            for (i, name) in names.iter().enumerate() {
                if let Some(v) = named[i] {
                    map.insert((*name).to_string(), level(v.to_string()));
                }
            }
        }
        PolicyTarget::RowFilter => {
            // **The row-filter hierarchy is three levels, and every one of
            // them is mandatory** (whole-branch review, Critical 1).
            // `rowFilterDef.resources` declares exactly
            // `catalog`/`schema`|`database`/`table`, each `"mandatory": true`,
            // and declares no `column` at all — re-read from both service-def
            // files by `tests/compile_rules.rs`'s
            // `the_row_filter_resource_map_matches_what_the_service_def_declares`,
            // not taken from this comment. So neither of the two things the
            // `Mask` arm above does is legal here: emitting a `column` adds a
            // resource the def cannot match, and omitting an unnamed `schema`
            // or `table` drops a level the def marks mandatory. Both compile,
            // deploy and are listed live, and both filter NOTHING — every row
            // comes back, on a control whose whole purpose is to withhold
            // rows.
            //
            // `RULE_SCOPE` at the authoring boundary
            // (`crate::rules::validate_rule_shape`) refuses a path row-filter
            // scope that does not name exactly catalog+schema+table, so a
            // rule written through egret's own API never reaches either
            // shape. This arm is what makes a row already in `perm.rules`
            // (written before that guard existed) compile to something the
            // plugin can still match rather than to a silent no-op: the
            // column is dropped, and an unnamed level becomes `*` — the same
            // "unspecified means every child" rendering the `Access` arm's
            // live measurement validated, and the only one that can satisfy
            // a mandatory level at all.
            let mut wildcard_from_here = false;
            for (i, name) in names.iter().take(ROW_FILTER_LEVELS).enumerate() {
                let value = match named[i] {
                    Some(v) if !wildcard_from_here => v.to_string(),
                    _ => {
                        wildcard_from_here = true;
                        WILDCARD.to_string()
                    }
                };
                map.insert((*name).to_string(), level(value));
            }
        }
    }
    map
}

/// How many levels of [`resource_level_names`] a ROW-FILTER policy may name:
/// `catalog`, `schema`|`database`, `table` — and never `column`, which
/// `rowFilterDef.resources` does not declare on either engine (Trino
/// service-def id 203, StarRocks id 205). One constant, read by the emission
/// (`scope_to_resources`, `crate::rules::column_resources`) and by the
/// write-time refusal (`crate::rules::validate_rule_shape`), so the two
/// cannot drift apart the way the mask and row-filter paths did.
pub(crate) const ROW_FILTER_LEVELS: usize = 3;

/// Refuse a [`Scope`] whose `schema`/`table`/`column` is PRESENT but EMPTY —
/// SP4d debt Task 12, a privilege escalation in the permission model.
///
/// [`CompileError::EmptyScopeLevel`] carries the whole measurement: the
/// accepted/rejected pair that proves `perm.grants`' level-skip CHECK could not
/// see this, the `MatchType` the compiled artifact then earns in a schema it was
/// never written for, and why `'*'` is deliberately NOT refused here. Read that
/// before changing this predicate.
///
/// A `*` IS refused in one shape, and by a different function:
/// [`ensure_a_grant_wildcard_level_has_nothing_named_beneath_it`] (SP4d debt Task 14)
/// refuses a match-any level with a NAMED level under it, because that is the
/// level skip `perm.grants`' own `grants_check` already forbids. It is a separate
/// predicate on purpose — `''` is refused at every level unconditionally, a
/// wildcard run only above a named level — and folding the two would be the
/// `requests_all_values` mistake this variant's doc warns about.
///
/// **Four call sites: the authoring boundary and the compiler, on each of the two
/// model types.** [`validate_grant_shape`] runs it before a grant is stored
/// (create, patch and the rolled-back preview all reach it), [`compile_grant`]
/// runs it again because the compiler is the last point at which a wrong artifact
/// becomes an enforced one and it is `pub` to callers that never went near the
/// store, and `crate::rules::validate_rule_shape` and
/// `crate::rules::compile_rule` are the same pair on the rule side.
///
/// ⚠ **`compile_rule` was NOT a call site when Task 12 wrote this**, on the
/// argument that an empty level in a rule renders a mask or row filter covering
/// MORE than it was written for (fails closed) while in a grant it ALLOWS more.
/// That argument is sound about severity and does not answer the question a `pub`
/// entry point raises — SP4d debt Task 14 step 5 added the call and
/// `crate::rules::compile_rule`'s doc carries the full reasoning, including why
/// `crate::rules::ensure_row_filter_scope_is_a_table` staying boundary-only is a
/// different case and not the same omission.
///
/// `catalog` is absent from the loop and needs nothing: the value emitted at
/// that level is `registry.catalogs.name`, and `0147_catalog_registry.sql` gives
/// that column `CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$')` — an allow-list under
/// which the empty string is impossible. (`0177`'s comment records the same
/// division of labour for the tag coordinate, including that weakening 0147's
/// regex re-opens it.)
pub(crate) fn ensure_scope_levels_are_named(scope: &Scope) -> Result<(), CompileError> {
    for (level, value) in [
        ("schema", scope.schema.as_deref()),
        ("table", scope.table.as_deref()),
        ("column", scope.column.as_deref()),
    ] {
        // Presence is tested here rather than folded into the predicate: an
        // ABSENT level is the ordinary catalog- or schema-wide scope, and
        // `scope_to_resources` renders it as `*` on purpose.
        let Some(value) = value else { continue };
        if value.is_empty() {
            return Err(CompileError::EmptyScopeLevel { level });
        }
    }
    Ok(())
}

/// Whether Ranger reads this policy value as "every value at this level" by way
/// of a WILDCARD run, rather than as a resource name.
///
/// **The class is a pure run of `*`, and that is the mechanism rather than a list
/// of spellings.** `RangerAbstractResourceMatcher.buildResourceMatchers()` stops
/// at the first match-any value and clears every matcher already built for the
/// level, so the level matches everything;
/// `egret_ranger_eval::resource::ValueMatcher::classify` reproduces the four
/// matcher shapes it can produce, and `classify("**")` / `classify("***")` are
/// the same match-any as `classify("*")` (pinned by that module's
/// `classify_reproduces_rangers_four_matcher_shapes`). Keying on the literal
/// `WILDCARD` alone would close one spelling and leave `"**"` reaching identical
/// enforcement from the identical free-text field.
///
/// **`"ord*"` is deliberately NOT in the class.** It classifies to
/// `ValueMatcher::StartsWith`, which names a SET of schemas: the level IS named,
/// and a table under it is not a gap. `egret_perm::eval::glob_match` exists
/// because such values are storable and the engine glob-matches them. Nor is
/// `" "` — whitespace is a literal resource name, the same line
/// `crate::routes::validate_explain_target` draws.
///
/// **`""` is match-any too and is NOT folded in here.** It is refused at every
/// level unconditionally by [`ensure_scope_levels_are_named`], and this rule
/// refuses a match-any level only when something is named beneath it — two
/// different refusals, so one predicate cannot serve both. That is the same trap
/// `CompileError::EmptyScopeLevel`'s doc records about
/// `egret_ranger_eval::resource::requests_all_values`, which folds `null`, `""`
/// and `"*"` together because it is the REQUEST-side rule.
///
/// **Keyed one-for-one with `migrations/0179`'s `~ '^[*]+$'`, and `!value.is_empty()`
/// is the term that makes that true on `''`.** `+` is one-or-more, so the SQL
/// regex does NOT match the empty string. Drop the emptiness term and
/// `value.bytes().all(..)` is VACUOUSLY true on `""`: the Rust half would call an
/// empty level a wildcard run while the CHECK called it a name, and the two
/// halves of one guardrail would disagree about which refusal a steward gets for
/// `('', 'orders')` — `NamedLevelUnderWildcard` from here, nothing at all from
/// `0179`.
///
/// **The term is not a redundant guard in front of
/// [`ensure_scope_levels_are_named`].** That function refuses `''` at every level
/// unconditionally and runs FIRST at all four call sites, so the difference is
/// unreachable end-to-end — which is exactly why it went unpinned: deleting
/// `!value.is_empty() && ` compiled and left 291 tests passing across 20
/// binaries, mutating a live disagreement into silence. The mask is call order,
/// and call order is not a test. `tests::empty_level_is_not_a_wildcard_run_so_0178_owns_the_refusal`
/// pins it by calling this rule DIRECTLY, past that mask.
///
/// `pub(crate)` since SP4d-3 Task 3: `crate::grammar` reads a hand-written
/// policy's levels with this SAME predicate, so "what is a wildcard run" has one
/// spelling for the compiler and for its inverse.
pub(crate) fn level_is_a_wildcard_run(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b == b'*')
}

/// Refuse a [`Scope`] that names a level BENEATH a match-any one — SP4d debt
/// Task 14, and the half `0178` deliberately left.
///
/// **A level may be a wildcard run only if every level below it is a wildcard
/// run or absent.** [`CompileError::NamedLevelUnderWildcard`] carries the whole
/// argument: that this is not "`*` is illegitimate", that `perm.grants`'
/// `grants_check` already forbids naming a table without a schema and
/// `schema_name = '*'` expresses precisely that, the `MatchType` the compiled
/// artifact earns in a schema it was never written for, and the `ScopePicker`
/// code path that makes this half reachable from the browser while `''` was not.
/// Read it before changing this predicate.
///
/// **It is the authored-value form of [`scope_to_resources`]' own
/// `wildcard_from_here` latch.** That latch renders `*` at every level below the
/// first unnamed one, so no scope this compiler can emit carries a named level
/// under an unnamed one; this function says the same thing about the values an
/// author writes. What it does NOT refuse, and what the tests pin: a
/// catalog-wide grant, a schema-wide grant, `('*','*')`,
/// `('sales','orders','*')` — every shape where a wildcard has nothing named
/// beneath it.
///
/// # ⚠ `perm.grants` ONLY — and the name of this function is how that is carried
///
/// **Two call sites, not four**: `validate_grant_shape` and `compile_grant`.
/// [`ensure_scope_levels_are_named`] still has four, because `''` is refused on
/// both tables; this one is not. Product decision 2026-10-10, recorded in
/// `migrations/0179`'s closing section, which also dropped that migration's
/// `perm.rules` constraint.
///
/// The ruling: *wider than the author wrote* points in OPPOSITE directions on
/// the two tables. On `perm.grants` a match-any level above a named one widens
/// **AUTHORITY** — the measured escalation, a grant written for one table
/// enforced in every schema of the catalog — and fails OPEN. On `perm.rules` the
/// same shape widens **RESTRICTION**: it withholds more, it fails CLOSED, both
/// plugins enforce it (every `dataMaskDef` level is `"wildCard": true` on
/// `ranger-plugins-common` 2.5.0 for Trino 470 and 2.8.0 for StarRocks
/// fe-ubuntu 4.1), and `('sales','*','email')` — mask `email` in every table of
/// `sales` — is a control a steward writes on purpose. It used to work. This is
/// the same line `0178` draws: `''` is an absence and stays refused everywhere,
/// a literal `*` the author typed is an authored value.
///
/// **How the grant/rule distinction is carried, and how it is NOT.** There is
/// still exactly ONE predicate over a level value — [`level_is_a_wildcard_run`] —
/// and ONE rule that walks the levels, this function. The distinction lives in
/// this function's NAME and in which call sites reference it; `crate::rules` does
/// not import it. It is deliberately **not** a `bool` or a table-kind enum
/// threaded through the callers: a flag two call sites must agree about is the
/// two-guards-must-agree shape that has produced a defect on this branch five
/// times, once inside the fix for it. Adding a parameter here to re-enable the
/// rule path would re-create exactly that.
///
/// **⚠ The trade-off the ruling accepts**, stated here because this is where a
/// reader meets the asymmetry: a broad mask can win Ranger's evaluation order and
/// apply a WEAKER mask than a narrow mask on the same table would — two mask
/// policies on one column at the same `policyPriority` are arbitrated with `>=`
/// and the tie-break is not expressed in the artifact. That is the already-open
/// ambiguous-priority debt (`egret_policy::ambiguous_arbitration` refuses the
/// differing pair at the authoring boundary), **not** a new hole opened by this
/// decision, and it is deliberately not addressed here.
///
/// `catalog` is out of the loop for the same measured reason as
/// [`ensure_scope_levels_are_named`] (`0147`'s `name` allow-list cannot hold a
/// `*`).
///
/// Both cells are asserted on this branch — the grant-path refusal in
/// `tests/compile_grants.rs`, the rule-path ACCEPT in `tests/compile_rules.rs`'s
/// `a_wildcard_path_level_over_a_named_one_is_allowed_on_a_rule` — because a
/// predicate that answers differently for two tables is unproven without both.
pub(crate) fn ensure_a_grant_wildcard_level_has_nothing_named_beneath_it(
    scope: &Scope,
) -> Result<(), CompileError> {
    let levels = [
        ("schema", scope.schema.as_deref()),
        ("table", scope.table.as_deref()),
        ("column", scope.column.as_deref()),
    ];
    for (i, (wildcard, value)) in levels.iter().enumerate() {
        // An ABSENT level is the ordinary catalog- or schema-wide scope, and
        // `perm.grants`' own `grants_check` already refuses naming anything
        // under it — this rule is about the levels that LOOK present.
        let Some(value) = value else { continue };
        if !level_is_a_wildcard_run(value) {
            continue;
        }
        for (named, named_value) in &levels[i + 1..] {
            let Some(named_value) = named_value else {
                continue;
            };
            // A wildcard run beneath a wildcard run is not a gap: both levels
            // are unnamed, and `scope_to_resources` would emit exactly this.
            if level_is_a_wildcard_run(named_value) {
                continue;
            }
            return Err(CompileError::NamedLevelUnderWildcard {
                wildcard,
                wildcard_value: value.to_string(),
                named,
                named_value: (*named_value).to_string(),
            });
        }
    }
    Ok(())
}

/// The part of compiling a grant that needs **no service and no engine** —
/// run at the authoring boundary, before the grant is stored (SP4a Task 9
/// review, I-1).
///
/// **Why a write-time check exists at all.** The HTTP create path recompiles
/// every profile the grant affects in the same transaction, so a grant that
/// cannot compile is normally refused by that recompile. But when the affected
/// set is EMPTY — a catalog no engine attaches yet —
/// `recompile_services_in_tx(&[])` validates nothing and the grant commits. It
/// then sits there until someone attaches an engine to that catalog, at which
/// point EVERY recompile of that profile fails: `POST /perm/compile` 400s, and
/// `egret-bi::catalog_tags` rolls back every tag write on that warehouse. A
/// grant nobody can see breaks tagging. Compilation has two failure classes,
/// and this is the one that does not need a service to detect.
///
/// **What is in and what is out.** In: the checks that would fail against
/// every service alike — a scope level present but EMPTY
/// ([`CompileError::EmptyScopeLevel`], SP4d debt Task 12: it is a level skip
/// `perm.grants`' own CHECK could not see, and the compiled policy is enforced
/// across the whole catalog), a `User` ref that was never resolved to a bare
/// `users.username` (`UnusableUserRef`), and a principal kind NO compile can
/// ever expand (`UnexpandedPrincipalKind` for `WorkspaceMembers`; a `role` is
/// expanded at compile time and so is accepted here — see [`Expansion`]). Out: everything that depends on
/// which engine or profile is being compiled for (`UnsupportedMaskType` reads
/// a service-def; `MissingTagService` reads a profile's config), because a
/// grant can be perfectly valid for one profile and not another and only the
/// recompile can tell.
///
/// This calls `principal_item`, the very function `compile_grant` uses, and
/// throws the item away. Reusing it rather than restating the rules is the
/// point: a permission model with two copies of "which principals can be
/// compiled" gets a drift between them, and the drift is a grant that passes
/// the gate and fails the compile.
pub fn validate_grant_shape(g: &Grant) -> Result<(), CompileError> {
    ensure_scope_levels_are_named(&g.scope)?;
    ensure_a_grant_wildcard_level_has_nothing_named_beneath_it(&g.scope)?;
    principal_item(&g.principals.include, g.effect, &[], Expansion::NotYet)?;
    principal_item(&g.principals.exclude, g.effect, &[], Expansion::NotYet)?;
    Ok(())
}

/// Compile one `Grant` into a Ranger `AccessPolicyInput` for engine `e`.
///
/// `catalog_name` is the caller-resolved name for `g.scope.catalog_id` —
/// nothing here checks that the two actually correspond; that binding is the
/// caller's, made when it resolved the id in the first place. `service` is
/// the target Ranger service name (e.g. `"trino-1"`) stamped onto the policy
/// body.
///
/// Returns `CompileError::UnsupportedAction` if any of `g.actions` has no
/// access type on `e` (unreachable for SP4a's two actions — see
/// `actions::CompileError`); `CompileError::UnusableUserRef` if a `User`
/// principal's ref is empty or contains `@`; `CompileError::
/// UnexpandedPrincipalKind` if any principal (include or exclude) is a
/// `WorkspaceMembers` reference this compiler cannot yet expand; and
/// `CompileError::EmptyScopeLevel` if `g.scope` names a level as the empty
/// string, which Ranger enforces as "every value at that level" — see
/// [`ensure_scope_levels_are_named`], which runs here as well as at the
/// authoring boundary.
pub fn compile_grant(
    g: &Grant,
    catalog_name: &str,
    service: &str,
    e: EngineType,
) -> Result<AccessPolicyInput, CompileError> {
    ensure_scope_levels_are_named(&g.scope)?;
    ensure_a_grant_wildcard_level_has_nothing_named_beneath_it(&g.scope)?;

    let mut accesses: Vec<String> = Vec::new();
    for action in &g.actions {
        for acc in engine_accesses(*action, e)? {
            if !accesses.iter().any(|a| a == acc) {
                accesses.push((*acc).to_string());
            }
        }
    }

    let item = principal_item(&g.principals.include, g.effect, &accesses, Expansion::Done)?;

    let mut allow_exceptions = Vec::new();
    let mut deny_exceptions = Vec::new();
    if !g.principals.exclude.is_empty() {
        let exception =
            principal_item(&g.principals.exclude, g.effect, &accesses, Expansion::Done)?;
        match g.effect {
            Effect::Allow => allow_exceptions.push(exception),
            Effect::Deny => deny_exceptions.push(exception),
        }
    }

    Ok(AccessPolicyInput {
        name: format!("{COMPILER_GRANT_PREFIX}{}", g.id),
        service: service.to_string(),
        is_enabled: true,
        resources: scope_to_resources(&g.scope, catalog_name, e, PolicyTarget::Access),
        items: vec![item],
        allow_exceptions,
        deny_exceptions,
        is_deny_all_else: false,
        description: String::new(),
        labels: Vec::new(),
        is_override: false,
        additional: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(schema: Option<&str>, table: Option<&str>, column: Option<&str>) -> Scope {
        Scope {
            catalog_id: uuid::Uuid::nil(),
            schema: schema.map(str::to_string),
            table: table.map(str::to_string),
            column: column.map(str::to_string),
        }
    }

    /// `''` is NOT a wildcard run, so this rule must stay silent on it.
    ///
    /// **This exists because a mutation survived.** Deleting `!value.is_empty() && `
    /// from [`level_is_a_wildcard_run`] compiled and left 291 tests passing across
    /// 20 binaries, exit 0. On `""` the SQL in `migrations/0179` says false —
    /// `^[*]+$` needs one-or-more `*` — and the mutated Rust says true, because
    /// `bytes().all(..)` is vacuously true on an empty iterator. The `**`/`***`
    /// half of the same predicate IS pinned (`tests/compile_grants.rs` kills
    /// `bytes().all(..)` -> `value == "*"`); the emptiness half was not.
    ///
    /// **What masked it was call order, not coverage.** All four production call
    /// sites run [`ensure_scope_levels_are_named`] first, and it refuses `''` at
    /// every level, so no end-to-end test can reach the disagreement. This test
    /// therefore calls [`ensure_a_grant_wildcard_level_has_nothing_named_beneath_it`]
    /// directly. A guardrail whose correctness rests on another function running
    /// first needs a test at its own boundary, because the next caller is one
    /// `pub(crate)` away from not running it.
    ///
    /// The ruling it pins: an empty level names nothing, which is `0178`'s
    /// `EmptyScopeLevel` refusal and not this one. Answering
    /// `NamedLevelUnderWildcard` for `('', 'orders')` would hand the steward the
    /// wrong remedy for a row `0179`'s CHECK admits.
    #[test]
    fn empty_level_is_not_a_wildcard_run_so_0178_owns_the_refusal() {
        assert!(
            !level_is_a_wildcard_run(""),
            "`^[*]+$` is one-or-more, so the empty string is not a wildcard run; \
             `bytes().all(..)` on its own is VACUOUSLY true here"
        );

        for s in [
            scope(Some(""), Some("orders"), None),
            scope(Some(""), Some("orders"), Some("email")),
            scope(Some("sales"), Some(""), Some("email")),
        ] {
            assert_eq!(
                ensure_a_grant_wildcard_level_has_nothing_named_beneath_it(&s),
                Ok(()),
                "{s:?}: an EMPTY level is a level that NAMES NOTHING — `0178`'s \
                 refusal, not this one. Firing here contradicts `0179`'s CHECK, \
                 which admits this row, and the one-for-one keying the predicate \
                 claims"
            );
        }

        // Control: the rule still fires on a REAL wildcard run with a named level
        // beneath it, so the assertions above cannot be passing because the
        // function has gone inert.
        assert!(
            ensure_a_grant_wildcard_level_has_nothing_named_beneath_it(&scope(
                Some("*"),
                Some("orders"),
                None,
            ))
            .is_err(),
            "the rule must still refuse `('*','orders')`, or everything above is \
             measuring an inert function"
        );
    }
}
