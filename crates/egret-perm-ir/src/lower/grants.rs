//! The grant family's decisions (governance sub-project 1, Task 5): which
//! grants a profile lowers, which it skips and why, and every refusal. The
//! rendering of a [`Statement`] into a target's shape lives in that target
//! (`egret-perm-compile`'s `ranger::grants`).
//!
//! **A `User` principal's `r#ref` is emitted verbatim, never rewritten.**
//! Round 1 of the old compiler split on `@` to turn an email into a guessed
//! username. That is not egret's username rule. The real one —
//! `ensure_username` (`crates/egret-auth/src/users.rs`) plus
//! `migrations/0040_user_username.sql` — lowercases, replaces
//! `[^a-z0-9_-]+` with `-`, and resolves collisions with a `-2`, `-3`, …
//! suffix. A compiler with no store handle cannot reproduce that: it
//! cannot see the collision suffix, and a naive split-on-`@` can compile
//! two distinct humans (`bob` and `bob-2`, both slugging from the same
//! local part) onto the SAME engine principal — an `exclude` of the
//! second lands on the first, and the one meant to be excluded stays
//! included. `model::Principal`'s doc comment already says refs are "a
//! username, never an email … written through unchanged" — canonicalising
//! here contradicted the store's own contract. So: this lowering performs
//! NO canonicalisation. It keeps `r#ref` byte-for-byte and refuses
//! (`CompileError::UnusableUserRef`) a `User` ref that's empty or
//! contains `@`, since either one is a sign the value reaching this
//! function was never resolved to `users.username` in the first place. The
//! I/O-capable half lives at the authoring boundary, where `users.username`
//! is readable: `egret-perm-compile`'s `principal_refs` (J-F3) checks every
//! ref of every kind before a write is stored, rewrites a username to its
//! stored spelling, and refuses an email or a user id with the username to
//! use — it never silently rewrites one into the other.
//!
//! **Groups no longer reach a grant's statement either, except `public`
//! (J-F3 Task 1b, design R8).** Until R8 a group compiled straight into
//! Ranger's `groups` bucket, on the reasoning that egret is not the only
//! possible Ranger consumer. D7
//! (`docs/en/plans/2026-09-24-permission-model-sp4a-d7-result.md`) had
//! measured that groups never reach any engine egret provisions (no group
//! provider on Trino, no usersync in egret), so that bucket matched nobody: an
//! allow on a group granted nobody while `explain` said it did, and a group in
//! a deny include or an exclude failed OPEN. A group's membership is knowable
//! from `user_groups`, so `egret-perm-compile`'s
//! `members::expand_enumerated_principals` now expands every non-`public`
//! group into its members' usernames before this module sees it, and every
//! writer of `user_groups` recompiles (`group_membership`). A
//! `PrincipalKind::Group` here therefore only ever carries `public`, which
//! every engine resolves. It is deliberately NOT refused the way an
//! unexpanded `Role` is: `compile_grant` is also the function the migration
//! grammar's differential exercises directly on hand-written bodies
//! (`egret-ranger-eval/tests/differential.rs`), where a group is the input
//! being compared, not an expansion that went missing.
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
//! That divergence is fixable, because `access_grants` knows who holds a
//! role: a `role` principal is expanded into its holders' usernames before
//! this module ever sees it, and reaching [`grant_statement`] with one
//! unexpanded is refused exactly as `WorkspaceMembers` is — while
//! [`validate_grant_shape`], which runs before anything is expanded, accepts
//! one.
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

use egret_perm::{Action, CompileError, Grant, Principal, PrincipalKind, Scope};
use uuid::Uuid;

use crate::audience::{audience, audience_from_expanded, Audience, Directory};
use crate::ep::EpInput;
use crate::statement::{
    Derivation, Effect, Level, LowerResult, Lowered, Lowering, Path, Privilege, ReasonCode, Source,
    Statement, Target,
};

/// The one check every `User` ref gets before it is emitted, shared by
/// [`grant_statement`] here and [`crate::lower::rules`]' `exempt_audience`
/// — the two places a `User` principal is ever bucketed. A `User` ref is
/// emitted verbatim (see module doc comment) after being checked for the two
/// shapes that prove it was never resolved to a bare `users.username`: empty,
/// or containing `@`.
pub fn validate_user_ref(r#ref: &str) -> Result<(), CompileError> {
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
/// (create, patch and the rolled-back preview all reach it), `compile_grant` (`egret-perm-compile`)
/// runs it again because the compiler is the last point at which a wrong artifact
/// becomes an enforced one and it is `pub` to callers that never went near the
/// store, and [`crate::lower::rules::validate_rule_shape`] and
/// [`crate::lower::rules::lower_rule`] are the same pair on the rule side.
///
/// ⚠ **`compile_rule` was NOT a call site when Task 12 wrote this**, on the
/// argument that an empty level in a rule renders a mask or row filter covering
/// MORE than it was written for (fails closed) while in a grant it ALLOWS more.
/// That argument is sound about severity and does not answer the question a `pub`
/// entry point raises — SP4d debt Task 14 step 5 added the call and
/// [`crate::lower::rules::lower_rule`]'s doc carries the full reasoning, including why
/// `crate::lower::rules::ensure_row_filter_scope_is_a_table` staying boundary-only is a
/// different case and not the same omission.
///
/// `catalog` is absent from the loop and needs nothing: the value emitted at
/// that level is `registry.catalogs.name`, and `0147_catalog_registry.sql` gives
/// that column `CHECK (name ~ '^[a-z][a-z0-9_]{0,62}$')` — an allow-list under
/// which the empty string is impossible. (`0177`'s comment records the same
/// division of labour for the tag coordinate, including that weakening 0147's
/// regex re-opens it.)
pub fn ensure_scope_levels_are_named(scope: &Scope) -> Result<(), CompileError> {
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
/// `egret_perm_compile::routes::validate_explain_target` draws.
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
/// `pub(crate)` since SP4d-3 Task 3: `egret_perm_compile::grammar` reads a hand-written
/// policy's levels with this SAME predicate, so "what is a wildcard run" has one
/// spelling for the compiler and for its inverse.
pub fn level_is_a_wildcard_run(value: &str) -> bool {
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
/// **It is the authored-value form of `scope_to_resources`' own
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
/// this function's NAME and in which call sites reference it; `egret_perm_compile::rules` does
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
pub fn ensure_a_grant_wildcard_level_has_nothing_named_beneath_it(
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

/// The one check every principal of an EXPANDED set gets before a target
/// buckets it: a `User` ref is checked by [`validate_user_ref`], `Group`
/// passes (only `public` survives expansion; module doc), and a `Role` or
/// `WorkspaceMembers` still present is a bug in the expansion, refused with
/// `CompileError::UnexpandedPrincipalKind`. Shared by [`grant_statement`]
/// and an engine grant's grantees, so both are checked alike.
pub fn check_expanded_principal(p: &Principal) -> Result<(), CompileError> {
    match p.kind {
        PrincipalKind::User => validate_user_ref(&p.r#ref),
        PrincipalKind::Group => Ok(()),
        PrincipalKind::Role | PrincipalKind::WorkspaceMembers => {
            Err(CompileError::UnexpandedPrincipalKind { kind: p.kind })
        }
    }
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
/// expanded at compile time and so is accepted here). Out: everything that depends on
/// which engine or profile is being compiled for (`UnsupportedMaskType` reads
/// a service-def; `MissingTagService` reads a profile's config), because a
/// grant can be perfectly valid for one profile and not another and only the
/// recompile can tell.
///
/// The principal rule is [`check_expanded_principal`], the very check
/// [`grant_statement`] applies, with exactly one difference: nothing has been
/// expanded yet, so a `Role` is an unexpanded-but-valid author's choice and is
/// accepted (SP4c Task 2b: SP4b's picker offers it, and it is expanded into its
/// holders at compile time). `WorkspaceMembers` stays refused: no authoring
/// path may write one (they are produced only by `egret_perm::assignment`), so
/// there is no legal author to accommodate. Reusing the check rather than
/// restating it is the point: a permission model with two copies of "which
/// principals can be compiled" gets a drift between them, and the drift is a
/// grant that passes the gate and fails the compile.
pub fn validate_grant_shape(g: &Grant) -> Result<(), CompileError> {
    ensure_scope_levels_are_named(&g.scope)?;
    ensure_a_grant_wildcard_level_has_nothing_named_beneath_it(&g.scope)?;
    for p in g.principals.include.iter().chain(&g.principals.exclude) {
        if p.kind != PrincipalKind::Role {
            check_expanded_principal(p)?;
        }
    }
    Ok(())
}

/// The text a grant with an empty include is skipped with, unchanged from the
/// recompile driver that used to hold this loop.
const EMPTY_AUDIENCE: &str =
    "the grant's principals name no engine principal (an empty include, or \
                              a workspace_members, role or group reference that expands to no \
                              member)";

/// Lower `grants` for the enforcement point `ep`. Each grant's audience is
/// its principals expanded against `dir` ([`audience`]), every grant's, in
/// order, before its scope is looked at.
///
/// A grant whose catalog is not in `ep.catalogs` is out of scope and silently
/// absent. A grant whose audience includes nobody, or one with a member left
/// unexpanded, is a [`Lowering`] carrying the exact text the recompile has
/// always reported. Every other error aborts the lowering.
pub fn lower_grants(ep: &EpInput, grants: &[Grant], dir: &Directory) -> LowerResult {
    let mut out = Lowered::default();
    for g in grants {
        let a = audience(&g.principals, dir);
        let Some(catalog_name) = ep.catalogs.get(&g.scope.catalog_id) else {
            continue; // not reachable by this service — out of scope
        };
        if a.include.is_empty() {
            // Nothing to name. Emitting the policy anyway would write an item
            // with no principal, which matches nobody — the same answer, said
            // less clearly, and one more policy against the profile total.
            // Recorded so "my workspace has no members yet" is visible rather
            // than inferred from a missing policy.
            let reason = EMPTY_AUDIENCE.to_string();
            tracing::warn!(
                service_id = ep.profile_id,
                grant_id = %g.id,
                reason = %reason,
                "grant skipped during recompile"
            );
            out.lowerings.push(Lowering {
                source: Source::Grant { grant_id: g.id },
                code: ReasonCode::EmptyAudience,
                detail: reason,
            });
            continue;
        }
        match statement_for_audience(g, a, catalog_name, g.scope.catalog_id) {
            Ok(s) => out.statements.push(s),
            Err(CompileError::UnexpandedPrincipalKind { kind }) => {
                let reason = format!(
                    "principal kind {kind:?} reached the compiler unexpanded — a bug in \
                     `crate::members::expand_enumerated_principals`, not in the grant"
                );
                tracing::warn!(
                    service_id = ep.profile_id,
                    grant_id = %g.id,
                    reason = %reason,
                    "grant skipped during recompile"
                );
                out.lowerings.push(Lowering {
                    source: Source::Grant { grant_id: g.id },
                    code: ReasonCode::UnexpandedPrincipal,
                    detail: reason,
                });
            }
            Err(other) => return Err(other),
        }
    }
    Ok(out)
}

/// The statement one grant lowers to, on catalog `catalog` (the
/// caller-resolved name of `catalog_id`; nothing here checks that the two
/// correspond — that binding is the caller's, made when it resolved the id).
///
/// Refuses, in this order (the order the old compiler raised them in, so the
/// first error is the same one): a scope level present but empty
/// ([`ensure_scope_levels_are_named`]), a named level under a wildcard run
/// ([`ensure_a_grant_wildcard_level_has_nothing_named_beneath_it`]), then each
/// principal, include before exclude, through [`check_expanded_principal`].
///
/// Privileges keep the grant's action order with duplicates dropped at their
/// first appearance, so a target expanding them in order reproduces the
/// access list the old compiler built action by action.
///
/// The grant's principals are taken as already expanded
/// ([`audience_from_expanded`]); the recompile lowers through
/// [`lower_grants`], which expands them.
pub fn grant_statement(
    g: &Grant,
    catalog: &str,
    catalog_id: Uuid,
) -> Result<Statement, CompileError> {
    statement_for_audience(
        g,
        audience_from_expanded(&g.principals),
        catalog,
        catalog_id,
    )
}

/// [`grant_statement`] with the audience the caller expanded: each member is
/// checked as the principal it stands for ([`check_expanded_principal`]),
/// include before except.
fn statement_for_audience(
    g: &Grant,
    audience: Audience,
    catalog: &str,
    catalog_id: Uuid,
) -> Result<Statement, CompileError> {
    ensure_scope_levels_are_named(&g.scope)?;
    ensure_a_grant_wildcard_level_has_nothing_named_beneath_it(&g.scope)?;
    for m in audience.include.iter().chain(&audience.except) {
        check_expanded_principal(&m.name.to_principal())?;
    }
    let mut privileges: Vec<Privilege> = Vec::new();
    for a in &g.actions {
        let p = match a {
            Action::Read => Privilege::Read,
            Action::Write => Privilege::Write,
        };
        if !privileges.contains(&p) {
            privileges.push(p);
        }
    }
    let level = |v: &Option<String>| v.clone().map_or(Level::Unnamed, Level::Named);
    Ok(Statement {
        source: Source::Grant { grant_id: g.id },
        audience,
        target: Target::Path(Path {
            catalog_id: Some(catalog_id),
            catalog: catalog.to_string(),
            schema: level(&g.scope.schema),
            table: level(&g.scope.table),
            column: level(&g.scope.column),
        }),
        effect: match g.effect {
            egret_perm::Effect::Allow => Effect::Allow { privileges },
            egret_perm::Effect::Deny => Effect::Deny { privileges },
        },
        derivation: Derivation::AsDeclared,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ep::{EpInput, EpKind, Mechanism};
    use crate::statement::*;
    use egret_perm::{
        Action, Effect, Grant, GrantOrigin, Principal, PrincipalKind, PrincipalSet, Scope,
    };

    fn ep(catalog: Uuid) -> EpInput {
        EpInput {
            profile_id: 1,
            profile_name: "p".into(),
            kind: EpKind {
                engine: egret_perm::EngineType::Trino,
                mechanism: Mechanism::Ranger,
            },
            engines: vec![],
            default_version: Err("unread".into()),
            default_image_level: None,
            classification_channel: false,
            catalogs: [(catalog, "sales".to_string())].into_iter().collect(),
        }
    }
    fn grant(
        catalog: Uuid,
        include: Vec<Principal>,
        exclude: Vec<Principal>,
        effect: Effect,
    ) -> Grant {
        Grant {
            id: Uuid::from_u128(1),
            effect,
            actions: vec![Action::Read, Action::Write],
            scope: Scope {
                catalog_id: catalog,
                schema: Some("s".into()),
                table: None,
                column: None,
            },
            principals: PrincipalSet { include, exclude },
            origin: GrantOrigin::Explicit,
        }
    }
    fn user(n: &str) -> Principal {
        Principal {
            kind: PrincipalKind::User,
            r#ref: n.into(),
        }
    }

    #[test]
    fn a_deny_with_an_exclude_lowers_to_deny_except() {
        let c = Uuid::from_u128(9);
        let out = lower_grants(
            &ep(c),
            &[grant(c, vec![user("a")], vec![user("b")], Effect::Deny)],
            &Directory::default(),
        )
        .unwrap();
        let s = &out.statements[0];
        assert_eq!(
            s.effect,
            crate::statement::Effect::Deny {
                privileges: vec![Privilege::Read, Privilege::Write]
            }
        );
        assert_eq!(
            s.audience.except[0].name,
            crate::audience::MemberName::User("b".into())
        );
        assert_eq!(
            s.target,
            Target::Path(Path {
                catalog_id: Some(c),
                catalog: "sales".into(),
                schema: Level::Named("s".into()),
                table: Level::Unnamed,
                column: Level::Unnamed,
            })
        );
    }

    #[test]
    fn a_grant_outside_the_scope_is_silently_absent() {
        let out = lower_grants(
            &ep(Uuid::from_u128(9)),
            &[grant(
                Uuid::from_u128(8),
                vec![user("a")],
                vec![],
                Effect::Allow,
            )],
            &Directory::default(),
        )
        .unwrap();
        assert!(out.statements.is_empty() && out.lowerings.is_empty());
    }

    #[test]
    fn an_empty_audience_is_a_lowering_with_todays_text() {
        let c = Uuid::from_u128(9);
        let out = lower_grants(
            &ep(c),
            &[grant(c, vec![], vec![], Effect::Allow)],
            &Directory::default(),
        )
        .unwrap();
        assert_eq!(out.lowerings[0].code, ReasonCode::EmptyAudience);
        assert!(out.lowerings[0]
            .detail
            .starts_with("the grant's principals name no engine principal"));
    }

    // Moved from `egret-perm-compile`'s `grants.rs` (Task 5) with the
    // functions it pins.

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
