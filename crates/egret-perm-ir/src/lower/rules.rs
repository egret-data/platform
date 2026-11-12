//! The rule family's decisions (governance sub-project 1, Task 6): masks and
//! row filters, from `perm.rules` rows to neutral [`Statement`]s. Which rules
//! a profile lowers, which it skips and why, every refusal, and how a
//! classification selector is lowered on an enforcement point that cannot
//! resolve one itself (the capability matrix, M1–M5 and M19). The rendering
//! of a statement into a target's shape lives in that target
//! (`egret-perm-compile`'s `ranger::rules`).
//!
//! **The asymmetry this lowering exists for (D5).** Trino's Ranger plugin
//! resolves tags itself, so a tag-selected rule lowers to ONE classification
//! statement on Trino (matrix M1/M2: native, given a classification channel),
//! covering however many columns currently carry the tag. StarRocks' plugin
//! has no tag service (upstream `starrocks#67458`), so the same rule lowers to
//! ONE statement PER matching column on StarRocks (M4/M5), enumerated at
//! compile time from [`RuleInputs::tagged`] — a list this lowering does not
//! resolve itself (no registry/tag I/O here; the loader has already looked up
//! which columns carry the rule's tag). That enumeration is stale the moment a
//! new column gets the tag; a later task recompiles on tag writes.
//!
//! **The ceiling is a refusal, never a truncation.** Above `ceiling`,
//! [`lower_rule`] returns `CompileError::ExpansionCeiling` naming the rule and
//! the count, on an expanded path only — Trino's tag policy is O(1) regardless
//! of how many columns carry the tag, so the ceiling does not apply there.
//!
//! **A rule's `exempt` set (D2: "how an individual escapes a mask — not a
//! competing grant") lowers to an audience of everyone except the exempt
//! principals**, which a target realizes as the documented Apache Ranger
//! idiom for "everyone, except these people" on a masking/row-filter policy: a
//! policy item scoped to the special `public` group applies to every
//! principal, and a more specific user/group item ahead of it is evaluated
//! first and wins for whoever it names
//! ([Row-level filtering and column-masking using Apache Ranger policies in
//! Apache Hive](https://cwiki.apache.org/confluence/pages/viewpage.action?pageId=65868896) —
//! Use Case #2 uses `group=public` for "applies to all users", Use Case #4
//! inserts a user-specific item with an empty filter to override it for one
//! user; the same page is cited in the design spec for this feature). The
//! exempt item uses `MASK_NONE` (masking) or an empty `filterExpr` (row
//! filter) — `MASK_NONE` is a declared type in every service-def this
//! compiler targets. `exempt.exclude` and a `Role` principal in `exempt`
//! cannot be expressed this way (see `CompileError::UnexpressibleExemption`)
//! and refuse rather than silently mis-compile.

use std::collections::BTreeMap;

use egret_perm::{CompileError, PrincipalKind, Rule, RuleKind, Selector, PUBLIC_GROUP};
use uuid::Uuid;

use crate::audience::{
    audience, audience_from_expanded, Audience, Directory, Member, MemberName, Via,
};
use crate::ep::{EpInput, EpKind};
use crate::lower::grants::{ensure_scope_levels_are_named, validate_user_ref};
use crate::matrix::{
    support_for_kind, Capability, LoweringKind, RuleEffectKind, Support,
    STARROCKS_MASK_MIN_EGRET_IMAGE,
};
use crate::statement::{
    Derivation, Effect, Level, LowerResult, Lowered, Lowering, Path, ReasonCode, Source, Statement,
    Target,
};

/// One column carrying the tag a rule selects on, resolved by the CALLER —
/// this lowering does no I/O and cannot itself know which columns carry
/// which `tags.key` (D6). Every field is the engine-facing NAME, not an id,
/// since a target stamps these values directly into a policy's resources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedColumn {
    pub catalog: String,
    pub schema: String,
    pub table: String,
    pub column: String,
}

impl TaggedColumn {
    /// A column with every level named. An empty level is refused
    /// ([`CompileError::EmptyScopeLevel`]): a Ranger target's
    /// `resources_object` keeps a `""` value and Ranger drops it, so the level
    /// would become match-any (spec E8). A table-level attachment's
    /// `column_name` is `''`, which is why the per-column expansion builds its
    /// columns through here.
    pub fn new(
        catalog: &str,
        schema: &str,
        table: &str,
        column: &str,
    ) -> Result<Self, CompileError> {
        for (level, value) in [
            ("catalog", catalog),
            ("schema", schema),
            ("table", table),
            ("column", column),
        ] {
            if value.is_empty() {
                return Err(CompileError::EmptyScopeLevel { level });
            }
        }
        Ok(Self {
            catalog: catalog.to_string(),
            schema: schema.to_string(),
            table: table.to_string(),
            column: column.to_string(),
        })
    }
}

/// One table a classification reaches on the enforcement point, at either
/// attachment level (spec §5.1), as the loader grouped it: the names are
/// engine-facing, and `columns` holds one entry per folded column name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaggedTable {
    pub catalog: String,
    pub schema: String,
    pub table: String,
    /// The classification is attached to the whole table (`column_name = ''`).
    pub table_level: bool,
    /// The columns the classification is attached to.
    pub columns: Vec<String>,
}

/// What the loader read for one rule. `classification_name` is the tag's
/// `tags.name` — never `tags.key`, which is stable but not what an engine's
/// tag plugin matches on (D6); `None` for a tag selector is refused
/// (`MissingTagName`), never a silent fallback to the key. `tagged` is read
/// only when [`needs_tagged_tables`] says the matrix expands the rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleInputs {
    pub classification_name: Option<String>,
    pub tagged: Vec<TaggedTable>,
}

/// Whether a statement on `catalog.schema.table` (and `column`, when it names
/// one) counts toward an expansion ceiling. A held rename's leaving
/// coordinates (rename spec C8) do not: their statements are still emitted.
pub trait CeilingCount {
    fn counts(&self, catalog: &str, schema: &str, table: &str, column: Option<&str>) -> bool;
}

/// The part of compiling a rule that needs **no service and no engine** —
/// the rule-side twin of [`crate::lower::grants::validate_grant_shape`]; read that
/// function's doc for why a write-time check is needed at all and how "service
/// independent" is drawn.
///
/// In: a path selector naming a level as the EMPTY string
/// (`EmptyScopeLevel` — SP4d debt Task 12; see
/// [`crate::lower::grants::ensure_scope_levels_are_named`] for the measurement and for
/// why this is the rule side of a GRANT-side escalation rather than the same
/// severity: an empty level makes a mask or row filter cover every sibling at
/// that level, which withholds more than the author wrote instead of granting
/// more, and `ensure_row_filter_scope_is_a_table` below would otherwise accept
/// `schema = ''` as a named schema),
/// a `CUSTOM` mask expression that does not parse (`InvalidCustomMask`),
/// and an exempt set a masking/row-filter item cannot express — a non-empty
/// `exempt.exclude`, a `Role`, the literal `public` group
/// (`UnexpressibleExemption`), or an unexpanded principal kind. Out:
/// `UnsupportedMaskType` (asks the capability matrix for the target engine),
/// `MissingTagName`/`MissingTagService` (read the profile), `ExpansionCeiling`
/// (counts that profile's tagged columns).
///
/// Like its grant-side twin this calls the same private helpers [`lower_rule`]
/// calls — `ensure_custom_mask_parses` and `exempt_audience` — rather than
/// restating their rules.
pub fn validate_rule_shape(r: &Rule) -> Result<(), CompileError> {
    if let RuleKind::Mask {
        mask_type,
        value_expr,
    } = &r.kind
    {
        if mask_type == "CUSTOM" {
            ensure_custom_mask_parses(&r.name, value_expr.as_deref().unwrap_or_default())?;
        }
    }
    if let Selector::Path(scope) = &r.selector {
        // ⚠ `crate::lower::grants::ensure_a_grant_wildcard_level_has_nothing_named_beneath_it`
        // is deliberately NOT called here — product decision 2026-10-10, recorded
        // in `migrations/0179`'s closing section. A match-any level above a named
        // one widens AUTHORITY on a grant (fails open: the measured escalation)
        // and widens RESTRICTION on a rule (fails closed), and both plugins
        // enforce the rule form, so `('sales','*','email')` as a mask is a
        // control a steward writes on purpose. `''` is still refused on both
        // tables by the call below, because `''` is an absence rather than an
        // authored value — the same line `0178` draws.
        ensure_scope_levels_are_named(scope)?;
    }
    ensure_row_filter_scope_is_a_table(r)?;
    exempt_audience(r, &audience_from_expanded(&r.exempt))?;
    Ok(())
}

/// A path-selected ROW FILTER must name exactly catalog + schema + table
/// (whole-branch review, Critical 1).
///
/// **Service-independent, so it belongs here.** `rowFilterDef.resources`
/// declares the same three mandatory levels and no `column` on BOTH engines
/// this compiler targets, so a scope that names a column — or that leaves a
/// gap above the table — is unrepresentable for every profile alike; nothing
/// about which service is being compiled for can change the answer, which is
/// the line `validate_grant_shape`'s doc draws for what may live at the
/// authoring boundary. (The three levels are pinned against the Ranger
/// target's `ROW_FILTER_LEVELS` by a compile-time assertion in
/// `egret-perm-compile`'s `ranger::rules`.)
///
/// **Why a refusal and not a best-effort render.** `scope_to_resources` now
/// renders such a scope into something the def CAN match (the column dropped,
/// a gap wildcarded), so nothing silently fails to filter any more — but what
/// it renders is not what the author asked for. A row filter scoped to
/// `orders.customer_email` would filter every row of `orders` for everyone,
/// which reads to the author as "my column-scoped filter worked" and is a
/// different control from the one they wrote. Refusing says so at the one
/// moment the author can still fix it. The `Tag` selector needs no equivalent
/// check: a tag row filter is table-granular by construction on both engines
/// (Trino's tag policy carries no path at all, and the per-table expansion
/// stops at `table`), so there is no scope for an author to get wrong.
///
/// ⚠ **`Some(_)` in the match below means "the level is PRESENT", not "the level
/// NAMES something"**, and the two shapes that exploited that are now settled
/// differently from each other.
///
/// `(Some(""), Some("orders"), None)` and `(Some("*"), Some("orders"), None)`
/// both land on the `(Some(_), Some(_), None)` arm and are accepted here as "a
/// table", while in enforcement terms both are a table with NO schema.
///
/// * `''` never reaches this function: [`validate_rule_shape`] runs
///   [`crate::lower::grants::ensure_scope_levels_are_named`] (Task 12) first, and
///   `0178` refuses the value in the table as well.
/// * `'*'` **does** reach it now, and is **accepted on purpose** — product
///   decision 2026-10-10 (`migrations/0179`'s closing section). A row filter on
///   `('*','orders')` is "filter `orders` in every schema", which withholds more
///   rather than fewer: it fails CLOSED, both plugins enforce it, and the author
///   typed the `*`. Task 14's rule-side refusal, which used to keep this shape
///   away from here, has been removed.
///
/// So this function's job is unchanged — a row-filter scope must name catalog +
/// schema + table — and it is no longer relying on an upstream guard to keep a
/// match-any schema out of its `Some(_)` arm.
fn ensure_row_filter_scope_is_a_table(r: &Rule) -> Result<(), CompileError> {
    let (RuleKind::RowFilter { .. }, Selector::Path(scope)) = (&r.kind, &r.selector) else {
        return Ok(());
    };
    let reason = match (&scope.schema, &scope.table, &scope.column) {
        (_, _, Some(c)) => Some(format!(
            "it is scoped to column {c:?}, and a row filter has no column to be scoped to — \
             the engines' rowFilterDef declares catalog/schema/table and nothing below. Mask \
             the column instead, or scope the row filter to its table"
        )),
        (None, _, None) | (_, None, None) => Some(
            "it must name a catalog, a schema and a table — every level the engines' \
             rowFilterDef declares is mandatory, so a scope that stops short of the table \
             cannot be matched as written"
                .to_string(),
        ),
        (Some(_), Some(_), None) => None,
    };
    match reason {
        Some(reason) => Err(CompileError::UnfilterableRowFilterScope {
            rule: r.name.clone(),
            reason,
        }),
        None => Ok(()),
    }
}

/// Refuse a mask type the matrix does not answer `Native` for on `kind` (M19:
/// the rows are the types each engine's service-def declares under
/// `dataMaskDef.maskTypes`, pinned to the file by `egret-perm-compile`'s
/// `ranger::rules::tests::matrix_mask_types_equal_the_service_def`). A name the
/// plugin does not know is silently ignored: the policy compiles, deploys, and
/// masks nothing, so it is refused here.
fn ensure_mask_type_declared(
    rule_name: &str,
    mask_type: &str,
    kind: EpKind,
    image_level: Option<u32>,
) -> Result<(), CompileError> {
    match support_for_kind(
        kind,
        None,
        image_level,
        false,
        &Capability::MaskType(mask_type.to_string()),
    ) {
        Support::Native => Ok(()),
        _ => Err(CompileError::UnsupportedMaskType {
            rule: rule_name.to_string(),
            mask_type: mask_type.to_string(),
            engine: kind.engine,
        }),
    }
}

/// The smallest check that still catches the obvious broken case: balanced
/// parentheses and the `{col}` substitution token present. Ranger never
/// parses this string itself; it is spliced into the engine's SQL at query
/// time, so a string that fails even this check is refused here rather than
/// failing every query that touches the masked column. A real SQL parser is
/// SP5's — this task does not start one.
fn ensure_custom_mask_parses(rule_name: &str, expr: &str) -> Result<(), CompileError> {
    let mut depth: i32 = 0;
    for c in expr.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return Err(invalid_custom_mask(rule_name, expr));
                }
            }
            _ => {}
        }
    }
    if depth != 0 || !expr.contains("{col}") {
        return Err(invalid_custom_mask(rule_name, expr));
    }
    Ok(())
}

fn invalid_custom_mask(rule_name: &str, expr: &str) -> CompileError {
    CompileError::InvalidCustomMask {
        rule: rule_name.to_string(),
        expr: expr.to_string(),
    }
}

/// A rule's audience: everyone (the `public` group), except its exempt
/// principals. A non-`public` exempt `Group` has already been expanded into
/// its members' usernames by `egret-perm-compile`'s
/// `members::expand_enumerated_principals` on every recompile (J-F3 Task 1b),
/// so on that path no group reaches `except`; one may still arrive from the
/// grammar differential, which compiles hand-written bodies directly. A
/// masking/row-filter item carries neither a `roles` bucket (unlike an access
/// item) nor an exception/subtract block the way access-policy items do — so a
/// `Role` principal or a non-empty `exempt.exclude` cannot be expressed and
/// refuses via `CompileError::UnexpressibleExemption`, and so does an exempt
/// `Group` principal literally named `"public"`: its exempt item would carry
/// `MASK_NONE`/an empty filter for "everyone", exempting every principal from
/// the rule — the one fail-open this design otherwise avoids by refusing
/// everything it cannot express correctly. `WorkspaceMembers` refuses for the
/// same not-yet-expanded reason a grant refuses it
/// (`CompileError::UnexpandedPrincipalKind`).
///
/// `exempt` is the rule's exempt set as an audience: expanded by
/// [`lower_rules`], taken as already expanded by every other caller. Each
/// member is checked as the principal it stands for.
fn exempt_audience(rule: &Rule, exempt: &Audience) -> Result<Audience, CompileError> {
    if !exempt.except.is_empty() {
        return Err(CompileError::UnexpressibleExemption {
            rule: rule.name.clone(),
            reason: "exempt.exclude is non-empty, but a masking/row-filter item has no \
                     exception block to subtract from (unlike an access-policy item)"
                .to_string(),
        });
    }
    for m in &exempt.include {
        let p = m.name.to_principal();
        match p.kind {
            PrincipalKind::User => {
                validate_user_ref(&p.r#ref)?;
            }
            PrincipalKind::Group => {
                if p.r#ref == PUBLIC_GROUP {
                    return Err(CompileError::UnexpressibleExemption {
                        rule: rule.name.clone(),
                        reason: format!(
                            "an exempt group literally named {PUBLIC_GROUP:?} would exempt \
                             everyone — that name is what this compiler's own \"applies to \
                             everyone\" item already uses"
                        ),
                    });
                }
            }
            PrincipalKind::Role => {
                return Err(CompileError::UnexpressibleExemption {
                    rule: rule.name.clone(),
                    reason: "a Role principal has no roles bucket on a masking/row-filter item"
                        .to_string(),
                });
            }
            PrincipalKind::WorkspaceMembers => {
                return Err(CompileError::UnexpandedPrincipalKind { kind: p.kind });
            }
        }
    }
    Ok(Audience {
        include: vec![Member {
            name: MemberName::Everyone,
            via: Via::Direct,
        }],
        except: exempt.include.clone(),
    })
}

fn effect_kind(kind: &RuleKind) -> RuleEffectKind {
    match kind {
        RuleKind::Mask { .. } => RuleEffectKind::Mask,
        RuleKind::RowFilter { .. } => RuleEffectKind::RowFilter,
    }
}

fn rule_effect(kind: &RuleKind) -> Effect {
    match kind {
        RuleKind::Mask {
            mask_type,
            value_expr,
        } => Effect::Mask {
            mask_type: mask_type.clone(),
            expression: value_expr.clone(),
        },
        RuleKind::RowFilter { filter_expr } => Effect::RowFilter {
            expression: filter_expr.clone(),
        },
    }
}

/// Whether the loader must read the tables carrying `r`'s classification:
/// the matrix lowers the selector by expansion on this EP (M4/M5), or the rule
/// is a row filter and the EP expands its column-only tables (M3). A path
/// selector never needs them.
pub fn needs_tagged_tables(ep: &EpInput, r: &Rule) -> bool {
    if !matches!(r.selector, Selector::Tag { .. }) {
        return false;
    }
    let effect = effect_kind(&r.kind);
    let lowered = |cap: &Capability| {
        matches!(
            support_for_kind(
                ep.kind,
                None,
                ep.effective_image_level(),
                ep.classification_channel,
                cap,
            ),
            Support::Lowered(_)
        )
    };
    lowered(&Capability::ClassificationSelector(effect))
        || (effect == RuleEffectKind::RowFilter
            && lowered(&Capability::ClassificationRowFilterOnColumnAttachment))
}

/// Refuse with `ExpansionCeiling` when more than `ceiling` statements count.
fn ensure_within_ceiling(r: &Rule, count: usize, ceiling: usize) -> Result<(), CompileError> {
    if count > ceiling {
        return Err(CompileError::ExpansionCeiling {
            rule: r.name.clone(),
            count,
            ceiling,
        });
    }
    Ok(())
}

/// Lower one `Rule` for an enforcement point of `ep_kind`.
///
/// - A classification selector the EP resolves natively (Trino with a
///   classification channel, M1/M2) lowers to exactly one
///   [`Target::Classification`] statement, covering whatever the EP currently
///   resolves the classification to (D5). A ROW FILTER also lowers M3 (KI-287,
///   below).
/// - A classification selector the matrix lowers by expansion (StarRocks,
///   M4/M5) lowers a MASK to one statement per tagged column in
///   `inputs.tagged` (column-level attachments only: a mask rule on a
///   whole-table classification is refused at write), and a ROW FILTER to one
///   statement per table, at either attachment level (D2, C8). Above `ceiling`
///   (columns for a mask, tables for a row filter) it refuses with
///   `CompileError::ExpansionCeiling`.
/// - A classification selector on an EP with no channel (M1/M2 missing) is
///   `CompileError::MissingTagService`.
/// - A path selector lowers to exactly one statement on either engine, at
///   `r`'s own scope with `catalog` as the catalog name (`inputs`/`ceiling`
///   are unused on this path).
///
/// Refuses in the order the old compiler did, so the first error is the same
/// one: an empty path level ([`ensure_scope_levels_are_named`]), the mask type
/// (M19), a `CUSTOM` mask expression, then per selector: `MissingTagName`
/// before the channel, and the exempt set (`exempt_audience`) at the point
/// the first statement is built — after the ceiling on an expanded path, and
/// not at all when the expansion is empty.
///
/// # The SCOPE-SHAPE refusal that runs here as well as at the authoring boundary
///
/// A `Path` selector's scope goes through
/// [`ensure_scope_levels_are_named`] (SP4d debt Task 12), exactly as a grant's
/// does: `''` is an absence, not an authored value, and a `pub` entry point
/// should not emit an artifact its own validator refuses.
///
/// ⚠ **It did NOT run here until Task 14, and the reason is worth keeping because
/// it was half right.** Task 12 left the rule compiler unguarded on a severity
/// argument: an unnamed level makes a mask or row filter cover every sibling at
/// that level, so a wrong rule artifact withholds MORE than the author wrote
/// (fails closed) while a wrong grant artifact ALLOWS more (fails open). That
/// argument is sound **about severity** and not sound **about a `pub` entry
/// point**, so the `''` guard runs here.
///
/// ⚠ **The match-any guard is GONE from the rule path entirely** — product
/// decision 2026-10-10, recorded in `migrations/0179`'s closing section. Task 14
/// had added `ensure_a_wildcard_level_has_nothing_named_beneath_it` at both
/// rule-side positions on the reasoning above; the ruling is that the severity
/// argument it set aside is the whole point on THIS table. A match-any level
/// above a named one widens AUTHORITY on a grant and RESTRICTION on a rule, both
/// plugins enforce the rule form (every `dataMaskDef` level is
/// `"wildCard": true` on `ranger-plugins-common` 2.5.0 and 2.8.0), and
/// `('sales','*','email')` is a mask a steward writes on purpose. It is not a
/// wrong artifact; it is the one the author asked for. The predicate is now
/// named [`crate::lower::grants::ensure_a_grant_wildcard_level_has_nothing_named_beneath_it`]
/// for the table it binds to, and nothing on the rule path calls it.
///
/// ⚠ **The trade-off that ruling accepts**, because this is a place a reader
/// meets it: a broad mask can win Ranger's evaluation order and apply a WEAKER
/// mask than a narrow mask on the same table would — two mask policies on one
/// column at the same `policyPriority` are arbitrated with `>=` and the
/// tie-break is not expressed in the artifact. That is the already-open
/// ambiguous-priority debt (`egret_policy::ambiguous_arbitration` refuses the
/// differing pair at the authoring boundary), NOT a new hole, and it is
/// deliberately not addressed here.
///
/// **`ensure_row_filter_scope_is_a_table` deliberately stays at the authoring
/// boundary only, and that is NOT the same omission.** Its argument is not about
/// severity. The shapes it refuses — a row-filter scope naming a column, or
/// stopping above the table — are shapes `0154` still permits in `perm.rules`, so
/// rows written before that guard existed are STORABLE today; and the Ranger
/// target's `scope_to_resources` `RowFilter` arm exists precisely to make such
/// a row compile to something the plugin can still match instead of to a
/// silent no-op (read that arm's comment). Moving the refusal into the
/// lowering would turn those stored rows uncompilable, which is a behaviour
/// change rather than a consistency fix. The `''` guard above has no such
/// population: since `0178` nothing stored can carry an empty level. (`0179`
/// constrains `perm.grants` only, so a match-any level above a named one IS
/// storable in `perm.rules` — by decision, and `scope_to_resources` renders it.)
#[allow(clippy::too_many_arguments)]
pub fn lower_rule(
    r: &Rule,
    exempt: &Audience,
    ep_kind: EpKind,
    image_level: Option<u32>,
    classification_channel: bool,
    catalog: &str,
    inputs: &RuleInputs,
    ceiling: usize,
    counts: &dyn CeilingCount,
) -> Result<Vec<Statement>, CompileError> {
    // Validated up front, before any engine/selector branching, so a rule
    // that cannot compile on ANY path is refused the same way regardless of
    // which one is taken.
    //
    // SP4d debt Task 14, step 5 (carried from Task 12's concern 3): the `''`
    // refusal runs HERE too, not only in `validate_rule_shape` — see this
    // function's doc for why the fail-closed severity argument that left it out
    // does not answer "a `pub` entry point emits an artifact its own validator
    // refuses", and for why `ensure_row_filter_scope_is_a_table` is a different
    // case rather than the same omission.
    //
    // ⚠ The match-any guard is NOT called here, and that is the ruling rather
    // than an omission (product decision 2026-10-10, `migrations/0179`'s closing
    // section): on a rule the shape fails CLOSED and both plugins enforce it.
    if let Selector::Path(scope) = &r.selector {
        ensure_scope_levels_are_named(scope)?;
    }
    if let RuleKind::Mask {
        mask_type,
        value_expr,
    } = &r.kind
    {
        ensure_mask_type_declared(&r.name, mask_type, ep_kind, image_level)?;
        if mask_type == "CUSTOM" {
            ensure_custom_mask_parses(&r.name, value_expr.as_deref().unwrap_or_default())?;
        }
    }

    let statement = |audience: &Audience, target: Target, derivation: Derivation| Statement {
        source: Source::Rule {
            rule_id: r.id,
            rule_name: r.name.clone(),
        },
        audience: audience.clone(),
        target,
        effect: rule_effect(&r.kind),
        derivation,
    };
    let expanded_path = |catalog: &str, schema: &str, table: &str, column: Option<&str>| {
        Target::Path(Path {
            catalog_id: None,
            catalog: catalog.to_string(),
            schema: Level::Named(schema.to_string()),
            table: Level::Named(table.to_string()),
            column: column.map_or(Level::Unnamed, |c| Level::Named(c.to_string())),
        })
    };

    if let Selector::Path(scope) = &r.selector {
        let level = |v: &Option<String>| v.clone().map_or(Level::Unnamed, Level::Named);
        let audience = exempt_audience(r, exempt)?;
        return Ok(vec![statement(
            &audience,
            Target::Path(Path {
                catalog_id: Some(scope.catalog_id),
                catalog: catalog.to_string(),
                schema: level(&scope.schema),
                table: level(&scope.table),
                column: level(&scope.column),
            }),
            Derivation::AsDeclared,
        )]);
    }

    // A tag selector. The name is checked before the channel and on every
    // EP: on an expanding EP the statements never carry the classification
    // itself (each is addressed by its own path), but a caller that hasn't
    // resolved the name has not done the resolution step D6 requires either,
    // and `inputs.tagged` being pre-filtered to the RIGHT tag is exactly the
    // invariant the name stands for.
    let Some(name) = &inputs.classification_name else {
        return Err(CompileError::MissingTagName {
            rule: r.name.clone(),
        });
    };
    let effect = effect_kind(&r.kind);
    match support_for_kind(
        ep_kind,
        None,
        image_level,
        classification_channel,
        &Capability::ClassificationSelector(effect),
    ) {
        Support::Native => {
            let audience = exempt_audience(r, exempt)?;
            let mut out = vec![statement(
                &audience,
                Target::Classification { name: name.clone() },
                Derivation::AsDeclared,
            )];
            // KI-287 (owner decision: a row filter by classification is
            // table-level; matrix M3). Trino's plugin applies a tag row filter
            // only to a table carrying the tag itself: a column attachment
            // reaches the table request as a descendant and filters nothing
            // (E14). So a table carrying the classification on columns only
            // gets its own path row filter, named and shaped as the per-table
            // expansion below names and shapes one. A table carrying it on
            // the table stays the classification statement's alone. A mask
            // needs none: Trino masks a tagged column through the tag policy.
            if effect == RuleEffectKind::RowFilter
                && support_for_kind(
                    ep_kind,
                    None,
                    image_level,
                    classification_channel,
                    &Capability::ClassificationRowFilterOnColumnAttachment,
                ) == Support::Lowered(LoweringKind::ExpandColumnOnlyTablesPerTable)
            {
                let column_only: Vec<&TaggedTable> =
                    inputs.tagged.iter().filter(|t| !t.table_level).collect();
                let count = column_only
                    .iter()
                    .filter(|t| counts.counts(&t.catalog, &t.schema, &t.table, None))
                    .count();
                ensure_within_ceiling(r, count, ceiling)?;
                for t in column_only {
                    out.push(statement(
                        &audience,
                        expanded_path(&t.catalog, &t.schema, &t.table, None),
                        Derivation::ExpandedFromClassification,
                    ));
                }
            }
            Ok(out)
        }
        Support::Missing(ReasonCode::NoClassificationChannel) => {
            Err(CompileError::MissingTagService {
                rule: r.name.clone(),
            })
        }
        // **A mask expands per COLUMN; a row filter expands per TABLE**
        // (whole-branch review, Critical 1, third shape). A row-filter policy
        // has no column resource to carry — `rowFilterDef` does not declare
        // one — so two tagged columns of the same table are one and the same
        // row filter, and so is a whole-table attachment of the tag (D2, C8).
        //
        // The ceiling counts what is emitted: tables for a row filter,
        // columns for a mask.
        Support::Lowered(LoweringKind::ExpandPerTable) => {
            let count = inputs
                .tagged
                .iter()
                .filter(|t| counts.counts(&t.catalog, &t.schema, &t.table, None))
                .count();
            ensure_within_ceiling(r, count, ceiling)?;
            let mut out = Vec::new();
            if !inputs.tagged.is_empty() {
                let audience = exempt_audience(r, exempt)?;
                for t in &inputs.tagged {
                    out.push(statement(
                        &audience,
                        expanded_path(&t.catalog, &t.schema, &t.table, None),
                        Derivation::ExpandedFromClassification,
                    ));
                }
            }
            Ok(out)
        }
        Support::Lowered(LoweringKind::ExpandPerColumn) => {
            let mut columns = Vec::new();
            for t in &inputs.tagged {
                for c in &t.columns {
                    columns.push(TaggedColumn::new(&t.catalog, &t.schema, &t.table, c)?);
                }
            }
            let count = columns
                .iter()
                .filter(|c| counts.counts(&c.catalog, &c.schema, &c.table, Some(&c.column)))
                .count();
            ensure_within_ceiling(r, count, ceiling)?;
            let mut out = Vec::new();
            if !columns.is_empty() {
                let audience = exempt_audience(r, exempt)?;
                for c in &columns {
                    out.push(statement(
                        &audience,
                        expanded_path(&c.catalog, &c.schema, &c.table, Some(&c.column)),
                        Derivation::ExpandedFromClassification,
                    ));
                }
            }
            Ok(out)
        }
        other => Err(CompileError::Db(format!(
            "the capability matrix answers {other:?} for a classification {effect:?} on \
             {ep_kind:?}, which no rule lowering handles (rule {:?})",
            r.name
        ))),
    }
}

/// The capability whose image gate (`matrix::Entry::min_egret_image`) decides
/// whether `r` can be enforced on a StarRocks EP: its mask type, or the
/// classification selector of a tag rule. `None` for a rule the gate never
/// touches (a path row filter).
fn image_gated_capability(r: &Rule) -> Option<Capability> {
    match (&r.kind, &r.selector) {
        (RuleKind::Mask { mask_type, .. }, _) => Some(Capability::MaskType(mask_type.clone())),
        (RuleKind::RowFilter { .. }, Selector::Tag { .. }) => Some(
            Capability::ClassificationSelector(RuleEffectKind::RowFilter),
        ),
        (RuleKind::RowFilter { .. }, Selector::Path(_)) => None,
    }
}

/// Whether the image gate, and nothing else, is what makes `cap` unanswerable
/// on this EP: the matrix answers it at the gate's minimum level but not at
/// `image_level`. A mask type the engine never declares is a different refusal
/// (`UnsupportedMaskType`) and is NOT this.
fn denied_by_image_level(kind: EpKind, image_level: Option<u32>, cap: &Capability) -> bool {
    let ask = |level| support_for_kind(kind, None, level, true, cap);
    matches!(ask(image_level), Support::Missing(_))
        && !matches!(
            ask(Some(STARROCKS_MASK_MIN_EGRET_IMAGE)),
            Support::Missing(_)
        )
}

/// Lower `rules` for the enforcement point `ep`, in rule order. Each rule's
/// exempt set is expanded against `dir` ([`audience`]), every rule's, before
/// anything else about it is looked at.
///
/// A path-selected rule whose catalog is not in `ep.catalogs` is out of scope
/// and silently absent. A rule with a `read_skips` entry (the loader could not
/// resolve its classification) is that lowering and nothing else. A rule whose
/// exempt set reached here unexpanded, or a classification rule on an EP with
/// no classification channel, is a [`Lowering`] carrying the exact text the
/// recompile has always reported. Every other error aborts the lowering.
///
/// Lowerings keep rule order whatever produced them: the read skips are
/// interleaved, not appended first, because the old driver reported each
/// rule's skip as it reached that rule.
pub fn lower_rules(
    ep: &EpInput,
    rules: &[Rule],
    dir: &Directory,
    inputs: &BTreeMap<Uuid, RuleInputs>,
    mut read_skips: BTreeMap<Uuid, Lowering>,
    ceiling: usize,
    counts: &dyn CeilingCount,
) -> LowerResult {
    let none = RuleInputs::default();
    let image_level = ep.effective_image_level();
    let mut out = Lowered::default();
    for r in rules {
        let exempt = audience(&r.exempt, dir);
        if let Some(l) = read_skips.remove(&r.id) {
            out.lowerings.push(l);
            continue;
        }
        let catalog = match &r.selector {
            Selector::Path(scope) => match ep.catalogs.get(&scope.catalog_id) {
                Some(name) => name.as_str(),
                None => continue, // not reachable by this service — out of scope
            },
            Selector::Tag { .. } => "",
        };
        // The image gate (SP3a): a StarRocks mask / row filter on an image
        // below the level that enforces it through every protocol is not
        // lowered. Skipped, not aborted, like a tag rule with no tag service:
        // one engine on an old image must not stop every other profile's
        // compile, and the skip names the remedy and reaches
        // `CompileSummary::skipped`.
        if image_gated_capability(r)
            .is_some_and(|cap| denied_by_image_level(ep.kind, image_level, &cap))
        {
            let have = image_level.map_or("not an egret fork image".to_string(), |n| {
                format!("egret.{n}")
            });
            out.lowerings.push(Lowering {
                source: Source::Rule {
                    rule_id: r.id,
                    rule_name: r.name.clone(),
                },
                code: ReasonCode::NotMeasured,
                detail: format!(
                    "rule {:?} is not enforced: a StarRocks engine enforces masks and row filters \
                     only from egret.{STARROCKS_MASK_MIN_EGRET_IMAGE} (this EP's least capable \
                     engine is {have}); upgrade the engine to the egret-starrocks image",
                    r.name
                ),
            });
            continue;
        }
        let rule_inputs = inputs.get(&r.id).unwrap_or(&none);
        let (code, reason) = match lower_rule(
            r,
            &exempt,
            ep.kind,
            image_level,
            ep.classification_channel,
            catalog,
            rule_inputs,
            ceiling,
            counts,
        ) {
            Ok(statements) => {
                out.statements.extend(statements);
                continue;
            }
            Err(CompileError::UnexpandedPrincipalKind { kind }) => (
                ReasonCode::UnexpandedPrincipal,
                format!(
                    "principal kind {kind:?} reached the compiler unexpanded — a bug in \
                     `crate::members::expand_enumerated_principals`, not in the rule"
                ),
            ),
            // A Trino profile with no `tagService` in its config cannot host a
            // tag policy at all — the Ranger tag plugin it would be resolved by
            // is not wired to this profile. That is a property of the PROFILE,
            // not a defect in the rule, so it is recorded as a skip (and warned
            // about) exactly like the dangling-tag-key case rather than
            // aborting the service's recompile.
            //
            // **The discriminator for skip-vs-refuse, stated once here because it
            // is the question every future variant will have to answer: would the
            // skipped item have been ENFORCED had it compiled?**
            //
            // - A deny grant: yes. Skipping it silently drops an enforced denial,
            //   which fails OPEN, so the compiler refuses instead.
            // - A tag rule on a profile with no `tagService`: no. Ranger's
            //   `download_tags` (`egret-policy/src/ranger/api/download.rs`) reads
            //   the tag service strictly from `config_jsonb`, with no fallback, so
            //   this profile serves no tag policies to anyone whatever we do here.
            //   Aborting would have left it unmasked AND stopped every OTHER
            //   profile from getting the mask. Skipping is strictly more
            //   protective, not merely more convenient.
            //
            // **Why this matters as of SP4a Task 9 and not before.** Task 7's
            // only caller recompiled the handful of profiles bound to ONE
            // warehouse. Task 9's rule-mutation path recompiles every
            // compiler-targeted profile in the installation, because a Trino tag
            // rule is catalog-unscoped (see `affected_services_for_rule`). Under
            // an abort, one misconfigured Trino profile anywhere would make
            // every tag-rule create and delete fail with a 500 — a profile
            // nobody involved in the write has heard of blocking the write. Each
            // skip still reaches the caller in `CompileSummary::skipped` and the
            // logs, so it is visible rather than silent.
            Err(CompileError::MissingTagService { rule }) => (
                ReasonCode::NoClassificationChannel,
                format!(
                    "profile has no usable `tagService` (unconfigured, or naming a profile that \
                     does not exist), so tag rule {rule:?} cannot compile here"
                ),
            ),
            Err(other) => return Err(other),
        };
        tracing::warn!(
            service_id = ep.profile_id,
            rule_id = %r.id,
            reason = %reason,
            "rule skipped during recompile"
        );
        out.lowerings.push(Lowering {
            source: Source::Rule {
                rule_id: r.id,
                rule_name: r.name.clone(),
            },
            code,
            detail: reason,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audience::MemberName;
    use crate::ep::{BoundEngine, EpInput, EpKind, Mechanism};
    use crate::statement::{Derivation, Level, Target};
    use egret_perm::{CompileError, EngineType, PrincipalSet, Rule, RuleKind, Selector};
    use uuid::Uuid;

    fn trino() -> EpKind {
        EpKind {
            engine: EngineType::Trino,
            mechanism: Mechanism::Ranger,
        }
    }
    fn starrocks() -> EpKind {
        EpKind {
            engine: EngineType::StarRocks,
            mechanism: Mechanism::Ranger,
        }
    }
    fn tag_rule(key: &str, kind: RuleKind) -> Rule {
        Rule {
            id: Uuid::from_u128(7),
            name: "r".into(),
            kind,
            selector: Selector::Tag { key: key.into() },
            exempt: PrincipalSet::default(),
        }
    }
    fn tag_mask_rule(key: &str, mask_type: &str) -> Rule {
        tag_rule(
            key,
            RuleKind::Mask {
                mask_type: mask_type.into(),
                value_expr: None,
            },
        )
    }
    fn tag_row_filter_rule(key: &str, filter: &str) -> Rule {
        tag_rule(
            key,
            RuleKind::RowFilter {
                filter_expr: filter.into(),
            },
        )
    }
    fn table(c: &str, s: &str, t: &str, table_level: bool, columns: &[&str]) -> TaggedTable {
        TaggedTable {
            catalog: c.into(),
            schema: s.into(),
            table: t.into(),
            table_level,
            columns: columns.iter().map(|c| c.to_string()).collect(),
        }
    }
    fn inputs(name: &str, tagged: Vec<TaggedTable>) -> RuleInputs {
        RuleInputs {
            classification_name: Some(name.into()),
            tagged,
        }
    }
    struct Always;
    impl CeilingCount for Always {
        fn counts(&self, _: &str, _: &str, _: &str, _: Option<&str>) -> bool {
            true
        }
    }

    #[test]
    fn trino_classification_mask_is_one_native_statement() {
        let r = tag_mask_rule("pii", "MASK_HASH");
        let s = lower_rule(
            &r,
            &audience_from_expanded(&r.exempt),
            trino(),
            Some(STARROCKS_MASK_MIN_EGRET_IMAGE),
            true,
            "",
            &inputs("PII", vec![]),
            500,
            &Always,
        )
        .unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].target, Target::Classification { name: "PII".into() });
        assert_eq!(s[0].derivation, Derivation::AsDeclared);
        assert_eq!(s[0].audience.include[0].name, MemberName::Everyone);
    }

    #[test]
    fn trino_classification_row_filter_adds_column_only_tables_ki_287() {
        let r = tag_row_filter_rule("pii", "region = 'us'");
        let tagged = vec![
            table("c", "s", "orders", false, &["email"]),
            table("c", "s", "customers", true, &[]),
        ];
        let s = lower_rule(
            &r,
            &audience_from_expanded(&r.exempt),
            trino(),
            Some(STARROCKS_MASK_MIN_EGRET_IMAGE),
            true,
            "",
            &inputs("PII", tagged),
            500,
            &Always,
        )
        .unwrap();
        assert_eq!(
            s.len(),
            2,
            "the tag statement plus orders (column-only); customers stays the tag's"
        );
        assert_eq!(s[1].derivation, Derivation::ExpandedFromClassification);
        assert!(
            matches!(&s[1].target, Target::Path(p) if p.table == Level::Named("orders".into()) && p.column == Level::Unnamed)
        );
    }

    #[test]
    fn starrocks_classification_mask_expands_per_column_and_refuses_over_the_ceiling() {
        let r = tag_mask_rule("pii", "MASK_NULL");
        let tagged = vec![table("c", "s", "t", false, &["a", "b", "c"])];
        assert_eq!(
            lower_rule(
                &r,
                &audience_from_expanded(&r.exempt),
                starrocks(),
                Some(STARROCKS_MASK_MIN_EGRET_IMAGE),
                false,
                "",
                &inputs("PII", tagged.clone()),
                3,
                &Always
            )
            .unwrap()
            .len(),
            3
        );
        assert!(matches!(
            lower_rule(
                &r,
                &audience_from_expanded(&r.exempt),
                starrocks(),
                Some(STARROCKS_MASK_MIN_EGRET_IMAGE),
                false,
                "",
                &inputs("PII", tagged),
                2,
                &Always
            ),
            Err(CompileError::ExpansionCeiling {
                count: 3,
                ceiling: 2,
                ..
            })
        ));
    }

    #[test]
    fn a_trino_classification_rule_without_a_channel_is_missing_tag_service() {
        let r = tag_mask_rule("pii", "MASK_NULL");
        assert!(matches!(
            lower_rule(
                &r,
                &audience_from_expanded(&r.exempt),
                trino(),
                Some(STARROCKS_MASK_MIN_EGRET_IMAGE),
                false,
                "",
                &inputs("PII", vec![]),
                500,
                &Always
            ),
            Err(CompileError::MissingTagService { .. })
        ));
    }

    #[test]
    fn a_held_rename_does_not_count_toward_the_ceiling() {
        struct Leaving;
        impl CeilingCount for Leaving {
            fn counts(&self, _: &str, _: &str, t: &str, _: Option<&str>) -> bool {
                t != "old"
            }
        }
        let r = tag_mask_rule("pii", "MASK_NULL");
        let tagged = vec![
            table("c", "s", "old", false, &["a", "b"]),
            table("c", "s", "new", false, &["a"]),
        ];
        assert_eq!(
            lower_rule(
                &r,
                &audience_from_expanded(&r.exempt),
                starrocks(),
                Some(STARROCKS_MASK_MIN_EGRET_IMAGE),
                false,
                "",
                &inputs("PII", tagged),
                1,
                &Leaving
            )
            .unwrap()
            .len(),
            3
        );
    }

    fn starrocks_ep(levels: &[Option<u32>]) -> EpInput {
        EpInput {
            profile_id: 1,
            profile_name: "p".into(),
            kind: starrocks(),
            engines: levels
                .iter()
                .enumerate()
                .map(|(i, l)| BoundEngine {
                    name: format!("e{i}"),
                    version: Ok("2.8.0".into()),
                    egret_image_level: *l,
                })
                .collect(),
            default_version: Ok("2.8.0".into()),
            default_image_level: Some(4),
            classification_channel: false,
            catalogs: BTreeMap::new(),
        }
    }

    #[test]
    fn effective_image_level_is_the_most_restrictive_and_unknown_wins() {
        assert_eq!(starrocks_ep(&[]).effective_image_level(), Some(4));
        assert_eq!(starrocks_ep(&[Some(4)]).effective_image_level(), Some(4));
        assert_eq!(
            starrocks_ep(&[Some(4), Some(2)]).effective_image_level(),
            Some(2)
        );
        assert_eq!(starrocks_ep(&[Some(4), None]).effective_image_level(), None);
        assert_eq!(starrocks_ep(&[None, Some(4)]).effective_image_level(), None);
    }

    /// The gate is live in lowering: below egret.3 (or unknown) a StarRocks
    /// mask / row-filter rule is not lowered (`lower_rule` refuses it,
    /// `lower_rules` reports it as a skip), at egret.3/4 the same rule lowers by
    /// the normal expansion.
    #[test]
    fn starrocks_mask_and_row_filter_below_image_level_are_not_lowered() {
        let mask = tag_mask_rule("pii", "MASK_NULL");
        let filter = tag_row_filter_rule("pii", "region = 'us'");
        let tagged = vec![table("c", "s", "t", false, &["a", "b"])];
        let lower = |r: &Rule, ep: &EpInput| {
            lower_rule(
                r,
                &audience_from_expanded(&r.exempt),
                ep.kind,
                ep.effective_image_level(),
                false,
                "",
                &inputs("PII", tagged.clone()),
                500,
                &Always,
            )
        };
        let lower_all = |ep: &EpInput| {
            let rules = [mask.clone(), filter.clone()];
            let by_rule: BTreeMap<Uuid, RuleInputs> = rules
                .iter()
                .map(|r| (r.id, inputs("PII", tagged.clone())))
                .collect();
            lower_rules(
                ep,
                &rules,
                &Directory::default(),
                &by_rule,
                BTreeMap::new(),
                500,
                &Always,
            )
            .unwrap()
        };
        for below in [Some(2), None] {
            let ep = starrocks_ep(&[Some(4), below]);
            assert!(
                matches!(
                    lower(&mask, &ep),
                    Err(CompileError::UnsupportedMaskType { .. })
                ),
                "mask at {below:?}"
            );
            assert!(lower(&filter, &ep).is_err(), "row filter at {below:?}");
            assert!(
                !needs_tagged_tables(&ep, &mask) && !needs_tagged_tables(&ep, &filter),
                "a denied rule needs no tagged tables at {below:?}"
            );
            let all = lower_all(&ep);
            assert!(all.statements.is_empty(), "nothing lowers at {below:?}");
            assert_eq!(all.lowerings.len(), 2, "both rules are reported skipped");
            assert!(all
                .lowerings
                .iter()
                .all(|l| l.code == ReasonCode::NotMeasured && l.detail.contains("egret.3")));
        }
        for at in [3, 4] {
            let ep = starrocks_ep(&[Some(at)]);
            assert_eq!(lower(&mask, &ep).unwrap().len(), 2, "mask at egret.{at}");
            assert_eq!(
                lower(&filter, &ep).unwrap().len(),
                1,
                "filter at egret.{at}"
            );
            assert!(needs_tagged_tables(&ep, &mask));
            let all = lower_all(&ep);
            assert_eq!(all.statements.len(), 3, "unchanged at egret.{at}");
            assert!(all.lowerings.is_empty());
        }
    }
}
