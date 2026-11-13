//! The withhold family's decisions (governance sub-project 1, Task 7): a
//! classification's CLEARANCE, from `classification_policies` rows to neutral
//! [`Statement`]s with [`Effect::Withhold`]. Which gates a profile lowers,
//! which it skips and why, the ceiling refusal, and how a classification is
//! withheld on an enforcement point that cannot resolve one itself (the
//! capability matrix, M6/M7, M10–M12, M15). The reads are the loader's
//! (`egret-perm-compile`'s `ir_load::load_withhold_inputs`); the rendering of
//! a statement into a target's shape is the target's (`ranger::withhold`,
//! whose module doc carries why a clearance is a DENY on `{public}` with the
//! cleared users enumerated as exceptions, and SP4a D2's amendment).
//!
//! # A tag gated ONLY by `required_groups` compiles to NOTHING, and says so
//!
//! D7 measured that a Ranger item naming a GROUP matches nobody either (no
//! group provider on Trino, no usersync in egret). A group-only gate would
//! therefore compile to a deny whose exceptions except nobody — a control that
//! withholds the column from everybody — or, if the group were put on the deny
//! side, to one that withholds nothing. Neither is the clearance that was
//! authored, so nothing is emitted and the skip is REPORTED (a [`Lowering`],
//! matrix M10) and logged rather than being silent. SP4c Task 5 says the same
//! thing in the UI, where the clearance is set.
//!
//! # Two shapes, because the two engines resolve a tag differently
//!
//! **Trino** has a Ranger TAG service, so a clearance is ONE classification
//! statement (matrix M6: native, given a classification channel), covering
//! however many columns currently carry the tag — O(1), installation-wide, and
//! evaluated before every resource policy.
//!
//! **StarRocks has no tag service** (upstream `starrocks#67458`), so a
//! clearance expands (M7) to **one withhold per tagged column**, the same
//! per-column expansion SP4a's tag MASKS already use on this engine. D0b
//! (`docs/en/plans/2026-09-28-sp4c-d0b-starrocks-result.md`) measured that this
//! works, and three of its findings constrain the shape:
//!
//! - **`select` only, by user ruling (2026-09-28).** `checkColumnAction` has
//!   exactly ONE caller and it passes `PrivilegeType.SELECT`
//!   (`ColumnPrivilege.java:152`), and egret's own `starrocks.json` restricts
//!   the `column` resource to `accessTypeRestrictions: ["select"]`. The
//!   `insert`/`update`/`delete` this compiler withholds on Trino can therefore
//!   NEVER match at column granularity here: emitting them would be decoration
//!   that reads as protection. **So a user without clearance can still
//!   OVERWRITE a column they cannot read** — and the live gate measured that
//!   this is true of **Trino too**, so it is not an asymmetry between the
//!   engines: the uncleared subject INSERTed and DELETEd through a live Trino
//!   clearance, because the deny names a *column* resource while the write check
//!   presents a *table* one. Neither engine withholds a write. Buying the write
//!   half needs a *table*-level deny, which would block every untagged column of
//!   that table too; that is a user ruling, not a compiler decision, and the
//!   ruling so far is "reads only". ([`withheld_privileges`] carries the
//!   matrix rows, M11/M12.)
//! - **No dedup, so the count is exactly N.** A tag-selected ROW FILTER dedups
//!   per table because `rowFilterDef` declares no column resource, so two
//!   tagged columns of one table are the same filter. A withhold DOES carry a
//!   column, so two tagged columns are two different statements and
//!   collapsing them would un-deny one of the columns.
//! - **Priority 0, not an override** — a rendering property, argued in the
//!   target's `clearance_input`.
//!
//! **The expansion ceilings apply, and a clearance now competes for the same
//! budget as a mask.** `ceiling` bounds one clearance
//! ([`CompileError::ClearanceExpansionCeiling`], named after the TAG because a
//! clearance is not a `perm.rules` row), and the profile ceiling bounds the
//! profile's running total — which a StarRocks clearance counts against and a
//! Trino one does not, because the Trino body is filed under the TAG service
//! and the total is scoped to the profile's own `service_id`. A tag that is
//! both masked and gated therefore costs 2N against that total for N columns.
//! Both ceilings **refuse rather than truncate**: a truncated clearance is a
//! control that withholds some of the columns it claims to, which is the
//! failure this programme keeps refusing to ship.
//!
//! A clearance is **not** a view-proof control, on either engine, and this is
//! MEASURED (live case 4, `docs/en/plans/2026-09-28-sp4c-live-result.md`):
//! `ColumnPrivilege` routes a `View` to `checkViewPrivilege` and a materialized
//! view to `checkMaterializedViewAction`, neither of which descends to
//! base-table columns, and a StarRocks view returned every classified value to
//! the uncleared subject. **Trino is worse, not better**: its views are
//! DEFINER-rights, so a view owned by a CLEARED user hands the whole column to
//! an uncleared reader whose direct read is refused in the same minute, and a
//! view owned by an UNCLEARED user blocks everybody including the cleared. The
//! materialized-view half remains source-only — the gate recorded both MVs as
//! BLOCKED (§4c) and declined to call them measured. The ruling is to state the
//! limit, not to block view creation, so it is stated in D0b, in D5's
//! acceptance criterion, and in the UI where a clearance is authored.
//!
//! **Amended by view governance A2 (2026-10-13) for StarRocks.** The
//! per-column withhold above is unchanged and still withholds only the DIRECT
//! read. Each tagged column now also gets a priority-1 `MASK_NULL` clearance
//! mask, which a base-column mask applies through every view kind; this
//! lowering hands the (tag, column, cleared) triples back as
//! [`WithholdLowered::view_guard`], which the recompile folds through the
//! evaluator once the whole desired set is known (`egret-perm-compile`'s
//! `clearance_mask`).

use std::collections::BTreeMap;

use egret_perm::{CompileError, Principal, PrincipalKind, PrincipalSet, TableCount, PUBLIC_GROUP};
use uuid::Uuid;

use crate::audience::{audience_from_expanded, Audience};
use crate::ep::{EpInput, EpKind};
use crate::lower::rules::{CeilingCount, TaggedColumn, TaggedTable};
use crate::matrix::{
    support_for_kind, Capability, LoweringKind, Support, STARROCKS_MASK_MIN_EGRET_IMAGE,
};
use crate::statement::{
    Derivation, Effect, Level, Lowered, Lowering, Path, Privilege, ReasonCode, Source, Statement,
    Target,
};

/// The only catalog StarRocks creates a materialized view in (measured on
/// StarRocks 4.1.6, `docs/en/plans/2026-10-14-starrocks-mv-placement-measurement.md`):
/// the catalog of the materialized-view guard's path, which the EP owns
/// outright (so the path carries no `catalog_id`).
pub const MATERIALIZED_VIEW_CATALOG: &str = "default_catalog";

/// One `classification_policies` row joined to the tag it gates, as the
/// loader read it. A row whose tag no longer exists was dropped by the join —
/// there is no tag left to withhold on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gate {
    /// `tags.id`.
    pub classification_id: Uuid,
    /// `tags.name`, never `tags.key`: an EP resolving the classification
    /// itself matches the tag type egret serves in its tag download, which is
    /// the name (the same resolution a tag-selected rule uses).
    pub classification_name: String,
    pub required_roles: Vec<String>,
    /// The exemption set: the users holding at least one of `required_roles`
    /// UNION the members of every `required_groups` group
    /// (`ClassificationPolicyRepo`'s semantics: `>= 1` role OR `>= 1` group),
    /// sorted and deduplicated so a recompile that changed nothing writes
    /// nothing. A group cannot be named in a Ranger item (M10), so it is
    /// resolved to its members here, like a role.
    pub cleared: Vec<String>,
    /// The tables the classification reaches on the EP, at either level; read
    /// only when [`needs_targets`] says so, else empty.
    pub targets: Vec<GateTarget>,
}

/// One table a gated classification reaches, and what the catalog says it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateTarget {
    pub table: TaggedTable,
    pub shape: TableShape,
}

/// What the catalog answered for a target's table. Looked up only for the
/// tables some gate classifies as a WHOLE table (folded, across every gate),
/// and then used for every target on that table, whichever gate's (C5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableShape {
    /// An active table: its schema, name and current columns, as the catalog
    /// spells them.
    Known {
        schema: String,
        table: String,
        columns: Vec<String>,
    },
    /// A view in the catalog, never a table.
    View,
    /// Not looked up, or not an active table (unknown, or not yet created).
    Unknown,
}

/// One view-guard mask still to fold: a classified column, the users cleared
/// for that classification, and whether the column is spelled as the catalog
/// spells it rather than as an assignment was typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingGuard {
    pub classification_id: Uuid,
    pub column: TaggedColumn,
    pub cleared: Vec<String>,
    pub catalog_spelled: bool,
}

/// What lowering this installation's clearances produced for one EP.
#[derive(Debug, Clone, Default)]
pub struct WithholdLowered {
    pub lowered: Lowered,
    /// Expanding EPs only (D2): the largest tables classified as a whole, for
    /// the profile ceiling's refusal to name (each costs a mask per column).
    pub whole_tables: Vec<TableCount>,
    /// Expanding EPs only (view governance A2): one entry per (gated
    /// classification, tagged column), for the recompile to fold into a
    /// view-guard mask once the whole desired set is known — the fold reads
    /// the EP's other masks, which this lowering cannot see.
    pub view_guard: Vec<PendingGuard>,
}

fn ask(kind: EpKind, channel: bool, cap: &Capability) -> Support {
    support_for_kind(
        kind,
        None,
        Some(STARROCKS_MASK_MIN_EGRET_IMAGE),
        channel,
        cap,
    )
}

/// Whether this EP expands a withhold per column (M7) rather than resolving
/// the classification itself.
fn expands(ep: &EpInput) -> bool {
    ask(
        ep.kind,
        ep.classification_channel,
        &Capability::WithholdByClassification,
    ) == Support::Lowered(LoweringKind::ExpandPerColumn)
}

/// Whether the loader must read the tables carrying `gate`'s classification:
/// the EP expands withholds per column (M7). Every gate the loader hands over
/// names a role or a group, so none is skipped.
pub fn needs_targets(ep: &EpInput) -> bool {
    expands(ep)
}

/// Whether the loader must ask the catalog about the targets' tables: the EP
/// expands withholds and at least one gate exists.
pub fn needs_table_shapes(ep: &EpInput, gates: &[Gate]) -> bool {
    expands(ep) && !gates.is_empty()
}

/// Every privilege a withhold withholds on `kind`: `Read` and `Write`, each
/// kept only where the matrix answers `Native` (M11/M12) — so `Read` and
/// `Write` on Trino, `Read` alone on StarRocks.
///
/// **Trino withholds a write, and the live gate measured that the write
/// half is INERT.** The intent was that a classification withholds the DATA
/// and not only the `SELECT` — an uncleared user who could still `INSERT` into
/// a classified column would be writing into data they are not cleared to
/// see. What actually happens (live case 5,
/// `docs/en/plans/2026-09-28-sp4c-live-result.md`) is that the uncleared
/// subject INSERTed **and** DELETEd through a live clearance: the compiled
/// deny names a *column* resource while Trino's write check presents a
/// *table* one, so the write accesses match nothing. **They are kept, but they
/// are not protection**, and this comment says so rather than arguing for
/// them: leaving the earlier justification standing is how the next reader
/// rebuilds the wrong model of what a clearance does. Buying the write half
/// means a *table*-level deny, which over-blocks every untagged column of that
/// table — a user ruling, not a compiler decision, and the ruling is
/// reads-only.
///
/// **StarRocks: `Read` ALONE, by user ruling (2026-09-28) — and it is NOT an
/// asymmetry with Trino, because neither engine withholds a write.** D0b
/// measured that `checkColumnAction` has exactly ONE caller and it passes
/// `PrivilegeType.SELECT` (`ColumnPrivilege.java:152`), and that egret's own
/// `starrocks.json` restricts the `column` resource to
/// `accessTypeRestrictions: ["select"]`. So a write could never MATCH at
/// column granularity here either. Emitting it anyway would be decoration that
/// reads as protection — a policy listing four withheld accesses while
/// withholding one — which is precisely the failure mode this programme keeps
/// refusing to ship. **The consequence is real on BOTH engines: a user
/// without clearance can still overwrite a column they cannot read.** SP4c
/// Task 5/7 says exactly that in the UI.
pub fn withheld_privileges(kind: EpKind) -> Vec<Privilege> {
    [Privilege::Read, Privilege::Write]
        .into_iter()
        .filter(|p| ask(kind, false, &Capability::WithholdPrivilege(*p)) == Support::Native)
        .collect()
}

/// The users cleared for EVERY classification in `cleared_sets` (one set per
/// gated classification that reaches the EP), in the first set's order, or
/// `None` when there is no set at all. An empty intersection leaves no
/// exception, so the materialized-view guard holds for everyone.
pub fn cleared_for_every(cleared_sets: &[Vec<String>]) -> Option<Vec<String>> {
    let mut cleared: Option<Vec<String>> = None;
    for set in cleared_sets {
        cleared = Some(match cleared.take() {
            None => set.clone(),
            Some(have) => have.into_iter().filter(|u| set.contains(u)).collect(),
        });
    }
    cleared
}

/// The mechanism versions a StarRocks EP's view guard (M13) is folded at:
/// each distinct version of its bound engines, in name order (the loader reads
/// them `ORDER BY name`), first appearance first; or, with no engine bound, the
/// version of egret's default image for the engine type, which is the version
/// `explain` evaluates an unbound profile at.
///
/// `Err` is the skip reason, naming the engine and the remedy. An engine whose
/// version is unknown (`BoundEngine::version` is `Err`, today's
/// `engine_plugin_version` sentence) makes the guard a skip for the whole EP
/// (user ruling 2026-10-13): binding such an engine is not refused, and the
/// per-column withholds are still lowered. The first such engine, in order,
/// names the skip.
///
/// A version the matrix does not lower the guard at (M13 lists 2.8.0 and
/// 2.9.0) is a skip too, with its own text. **That text is unreachable in
/// production**: the loader's versions come from `engine_plugin_version`,
/// which yields only modelled (engine type, version) pairs — for StarRocks
/// exactly M13's two — so it is new text that no compile has ever emitted.
pub fn view_guard_versions(ep: &EpInput) -> Result<Vec<String>, String> {
    let profile = &ep.profile_name;
    let unmodelled = |name: &str, why: &str| {
        format!(
            "view protection (clearance mask) not active for {profile}: engine {name} runs an \
             image egret has not modelled — pin it to a known image (see \
             docs/en/guides/engine-versions.md). {why}"
        )
    };
    let measured = |name: &str, v: &str| {
        if support_for_kind(
            ep.kind,
            Some(v),
            None,
            ep.classification_channel,
            &Capability::WithholdThroughView,
        ) == Support::Lowered(LoweringKind::ViewGuardMask)
        {
            Ok(())
        } else {
            Err(format!(
                "view protection (clearance mask) not active for {profile}: engine {name} runs \
                 plugin version {v}, at which the view guard has not been measured — pin it to \
                 a known image (see docs/en/guides/engine-versions.md)"
            ))
        }
    };
    let mut out: Vec<String> = Vec::new();
    for e in &ep.engines {
        match &e.version {
            Ok(v) => {
                measured(&e.name, v)?;
                if !out.contains(v) {
                    out.push(v.clone());
                }
            }
            Err(why) => return Err(unmodelled(&e.name, why)),
        }
    }
    if out.is_empty() {
        const DEFAULT_IMAGE: &str = "(none bound; egret's default image)";
        match &ep.default_version {
            Ok(v) => {
                measured(DEFAULT_IMAGE, v)?;
                out.push(v.clone());
            }
            Err(why) => return Err(unmodelled(DEFAULT_IMAGE, why)),
        }
    }
    Ok(out)
}

/// Everyone (the `public` group), except the `cleared` users.
fn everyone_except(cleared: &[String]) -> Audience {
    audience_from_expanded(&PrincipalSet {
        include: vec![Principal {
            kind: PrincipalKind::Group,
            r#ref: PUBLIC_GROUP.to_string(),
        }],
        exclude: cleared
            .iter()
            .map(|u| Principal {
                kind: PrincipalKind::User,
                r#ref: u.clone(),
            })
            .collect(),
    })
}

fn source(g: &Gate) -> Source {
    Source::Withhold {
        classification_id: g.classification_id,
        classification_name: g.classification_name.clone(),
    }
}

/// A gate the EP does not lower, with its reason: logged, and reported.
fn skip(ep: &EpInput, g: &Gate, code: ReasonCode, reason: String) -> Lowering {
    tracing::warn!(
        service_id = ep.profile_id,
        tag_id = %g.classification_id,
        reason = %reason,
        "classification clearance skipped during recompile"
    );
    Lowering {
        source: source(g),
        code,
        detail: reason,
    }
}

/// The catalog, schema and table of a whole-table withhold must each be
/// named, exactly as [`TaggedColumn::new`] refuses an empty level
/// (`EmptyScopeLevel`): an empty value is dropped by a target and the level
/// becomes match-any.
fn ensure_table_named(catalog: &str, schema: &str, table: &str) -> Result<(), CompileError> {
    for (level, value) in [("catalog", catalog), ("schema", schema), ("table", table)] {
        if value.is_empty() {
            return Err(CompileError::EmptyScopeLevel { level });
        }
    }
    Ok(())
}

/// The path of a withhold expanded onto a table (`column: None`, the whole
/// table) or one of its columns.
fn expanded_path(catalog: &str, schema: &str, table: &str, column: Option<&str>) -> Target {
    Target::Path(Path {
        catalog_id: None,
        catalog: catalog.to_string(),
        schema: Level::Named(schema.to_string()),
        table: Level::Named(table.to_string()),
        column: column.map_or(Level::Unnamed, |c| Level::Named(c.to_string())),
    })
}

/// A target's table as the catalog spells it, when the catalog knows it.
fn known(t: &GateTarget) -> Option<(&str, &str, &[String])> {
    match &t.shape {
        TableShape::Known {
            schema,
            table,
            columns,
        } => Some((schema.as_str(), table.as_str(), columns.as_slice())),
        TableShape::View | TableShape::Unknown => None,
    }
}

/// The five tables with the most columns, most first, for a ceiling refusal.
fn largest(mut tables: Vec<TableCount>) -> Vec<TableCount> {
    tables.sort_by(|a, b| {
        b.columns
            .cmp(&a.columns)
            .then_with(|| a.table.cmp(&b.table))
    });
    tables.truncate(5);
    tables
}

/// Lower every gate for the enforcement point `ep`.
///
/// On an EP that resolves classifications itself (Trino, M6) a clearance is
/// **installation-wide**, exactly like a tag-selected rule: one
/// [`Target::Classification`] statement per gate, in `gates` order (the
/// loader reads them by `tag_id`), so every gated tag lowers into every Trino
/// profile with a classification channel, and into none without one (a
/// [`ReasonCode::NoClassificationChannel`] lowering).
///
/// On an EP that expands (StarRocks, M7) it is EP-scoped, because the
/// expansion is: only the tables in each gate's `targets` (reachable from the
/// EP) are addressed. A gated tag with no reachable target lowers to nothing
/// there, and that is NOT a lowering — there is no column to withhold, which
/// is the same answer SP4a's tag masks give for the same input. Per gate, in
/// order, per target: a whole-table attachment's [`Derivation::WholeTable`]
/// withhold, or a column attachment's per-column withholds; then, after every
/// gate, the materialized-view guard (M15) excepting the users cleared for
/// every gate that reached the EP ([`cleared_for_every`]).
///
/// A gate naming no role and no group is NO gate — an informational tag
/// (`egret_bi::tags::ClassificationPolicyRepo`'s own semantics, and the Cedar
/// loader's) — and the loader never hands it over. A gate naming groups only
/// is an ordinary gate: the loader resolves the groups into `cleared`.
///
/// `fold` is the EP's identifier fold (`egret_ranger_eval::resource::fold_identifier`
/// on the Ranger target), passed by the caller so the IR does not depend on
/// the evaluator. Lowerings come in the order the old compiler reported them:
/// every gate's group-only or no-channel lowering first, then the expanding
/// EP's per-target lowerings in gate order.
pub fn lower_withholds(
    ep: &EpInput,
    gates: &[Gate],
    fold: fn(&str) -> String,
    ceiling: usize,
    counts: &dyn CeilingCount,
) -> Result<WithholdLowered, CompileError> {
    let mut out = WithholdLowered::default();
    if gates.is_empty() {
        return Ok(out);
    }
    let privileges = withheld_privileges(ep.kind);
    let withhold = Effect::Withhold {
        privileges: privileges.clone(),
    };
    // Expanding EPs: lowered once every gate is known, so a table's catalog
    // shape decides every policy on it, whichever gate's.
    let mut expanding: Vec<&Gate> = Vec::new();

    for g in gates {
        if g.cleared.is_empty() {
            // NOT a lowering, and not a defect: nobody holds the clearing
            // role, so nobody is cleared, and a withhold standing for
            // everyone is the correct compilation of that — the same answer
            // Cedar's `forbid … unless context.roles.containsAny(...)`
            // reaches. Logged because "the column is refused to everybody" is
            // worth being able to find in a log when someone asks why.
            tracing::info!(
                tag_id = %g.classification_id,
                roles = ?g.required_roles,
                "a clearance's required roles have no holders — the compiled \
                 deny withholds the tagged column from everybody, which is what 'nobody is \
                 cleared' means"
            );
        }

        match ask(
            ep.kind,
            ep.classification_channel,
            &Capability::WithholdByClassification,
        ) {
            Support::Native => out.lowered.statements.push(Statement {
                source: source(g),
                audience: everyone_except(&g.cleared),
                target: Target::Classification {
                    name: g.classification_name.clone(),
                },
                effect: withhold.clone(),
                derivation: Derivation::AsDeclared,
            }),
            Support::Missing(ReasonCode::NoClassificationChannel) => {
                out.lowered.lowerings.push(skip(
                    ep,
                    g,
                    ReasonCode::NoClassificationChannel,
                    "this Trino profile has no usable `tagService` (unconfigured, or naming a \
                     profile that does not exist), and a Trino clearance is a TAG policy — \
                     nothing filed here could be served to any plugin"
                        .to_string(),
                ))
            }
            // **SP4c Task 2d — the per-column expansion, on the engine that
            // has no tag service to do it for us**: one withhold per matched
            // column and NO dedup, because a withhold DOES carry a column and
            // collapsing two columns would un-deny one.
            //
            // **D2 (2026-10-25): a whole-table attachment is ONE whole-table
            // withhold** (user decision U1), which needs no column list and
            // also covers columns added later, the hidden `_file`/`_pos` and
            // `___count___` StarRocks checks when every column is masked. It
            // covers the tag's per-column withholds on the same table (C6):
            // same exceptions, wider resource.
            Support::Lowered(LoweringKind::ExpandPerColumn) => expanding.push(g),
            other => {
                return Err(CompileError::Db(format!(
                    "the capability matrix answers {other:?} for a classification withhold on \
                     {:?}, which no withhold lowering handles (tag {:?})",
                    ep.kind, g.classification_name
                )))
            }
        }
    }

    if expands(ep) {
        let mut mv_cleared = Vec::new();
        let mut whole_tables: BTreeMap<String, usize> = BTreeMap::new();
        for g in expanding {
            lower_expanded_gate(
                ep,
                g,
                &withhold,
                fold,
                ceiling,
                counts,
                &mut whole_tables,
                &mut out,
            )?;
            // C7: every gated tag that reaches the EP, at either level,
            // narrows the materialized-view guard.
            if !g.targets.is_empty() {
                mv_cleared.push((g, g.cleared.clone()));
            }
        }
        if let Some(statement) = materialized_view_guard(ep, &mv_cleared) {
            out.lowered.statements.push(statement);
        }
        out.whole_tables = largest(
            whole_tables
                .into_iter()
                .map(|(table, columns)| TableCount { table, columns })
                .collect(),
        );
    }
    Ok(out)
}

/// The expanding half of [`lower_withholds`] for one gate: its withholds, its
/// lowerings and its pending view guards, into `out`.
///
/// **Whole-table attachments (D2).** A table whose shape is
/// [`TableShape::Known`] gets a view-guard mask per current column (C1), and
/// the catalog's spelling of its schema, table and columns for every
/// statement on it, whichever gate's (C5); spelled as the catalog spells it,
/// two spellings of one table can never become two statements.
#[allow(clippy::too_many_arguments)]
fn lower_expanded_gate(
    ep: &EpInput,
    g: &Gate,
    withhold: &Effect,
    fold: fn(&str) -> String,
    ceiling: usize,
    counts: &dyn CeilingCount,
    whole_tables: &mut BTreeMap<String, usize>,
    out: &mut WithholdLowered,
) -> Result<(), CompileError> {
    // C9: a whole-table attachment costs its whole-table withhold and one
    // mask per known column; a column-level one its per-column withholds
    // (column masks stay uncounted per tag, as before D2). A held rename's
    // leaving table or column costs nothing (rename spec C8).
    let mut per_column_denies = 0;
    let mut whole_table_cost = 0;
    let mut tables: Vec<TableCount> = Vec::new();
    for gt in &g.targets {
        let t = &gt.table;
        let counted = |c: Option<&str>| counts.counts(&t.catalog, &t.schema, &t.table, c);
        if t.table_level {
            let (schema, table, columns) =
                known(gt).unwrap_or((t.schema.as_str(), t.table.as_str(), &[][..]));
            if counted(None) {
                whole_table_cost +=
                    1 + columns.iter().filter(|c| counted(Some(c.as_str()))).count();
            }
            let name = format!("{}.{schema}.{table}", t.catalog);
            whole_tables.insert(name.clone(), columns.len());
            tables.push(TableCount {
                table: name,
                columns: columns.len(),
            });
        } else {
            per_column_denies += t.columns.iter().filter(|c| counted(Some(c))).count();
        }
    }
    let count = per_column_denies + whole_table_cost;
    // **Refuse, never truncate.** A truncated clearance withholds SOME of the
    // columns it claims to, and the ones silently dropped are readable by
    // exactly the people the classification exists to keep out. The refusal
    // fails the write that caused it (the recompile propagates it, the
    // caller's `?` rolls back) rather than committing a clearance the UI shows
    // as active.
    if count > ceiling {
        return Err(CompileError::ClearanceExpansionCeiling {
            tag: g.classification_name.clone(),
            count,
            ceiling,
            tables: largest(tables),
            per_column_denies,
        });
    }

    let audience = everyone_except(&g.cleared);
    let statement = |target: Target, derivation: Derivation| Statement {
        source: source(g),
        audience: audience.clone(),
        target,
        effect: withhold.clone(),
        derivation,
    };
    for gt in &g.targets {
        let t = &gt.table;
        let catalog = known(gt);
        let (schema, table) =
            catalog.map_or((t.schema.as_str(), t.table.as_str()), |(s, t, _)| (s, t));
        // (column, spelled as the catalog spells it)
        let mut columns: Vec<(TaggedColumn, bool)> = Vec::new();
        for c in &t.columns {
            let spelled = catalog.and_then(|(_, _, cols)| cols.iter().find(|k| fold(k) == fold(c)));
            columns.push((
                TaggedColumn::new(&t.catalog, schema, table, spelled.unwrap_or(c))?,
                spelled.is_some(),
            ));
        }
        if t.table_level {
            ensure_table_named(&t.catalog, schema, table)?;
            out.lowered.statements.push(statement(
                expanded_path(&t.catalog, schema, table, None),
                Derivation::WholeTable,
            ));
            match &gt.shape {
                TableShape::Known { columns: cols, .. } => {
                    for c in cols {
                        columns.push((TaggedColumn::new(&t.catalog, schema, table, c)?, true));
                    }
                }
                // U4: the withhold needs no column list and is lowered; the
                // masks do. The live spike measured that the deny alone lets
                // a view read real values, so the lowering says so. A view
                // has no columns in the catalog and never will, and nobody
                // measured StarRocks applying a table deny to an Iceberg view
                // (M14), so its lowering says that instead.
                TableShape::View => out.lowered.lowerings.push(skip(
                    ep,
                    g,
                    ReasonCode::ViewNotMeasured,
                    format!(
                        "{}.{schema}.{table} is a view in the catalog, not a table: its \
                         whole-table select deny is compiled on the view's name, which egret \
                         has not measured StarRocks applying to a view, and no NULL masks, \
                         now or later, since egret builds them from a table's columns; \
                         classify the table the view reads to protect what it shows",
                        t.catalog
                    ),
                )),
                TableShape::Unknown => out.lowered.lowerings.push(skip(
                    ep,
                    g,
                    ReasonCode::TableNotInCatalog,
                    format!(
                        "{}.{schema}.{table} is not a table in the catalog (unknown, or not \
                         yet created): its whole-table select deny is compiled, its NULL \
                         masks are not, so a view over it is not protected until the table \
                         exists",
                        t.catalog
                    ),
                )),
            }
        } else {
            for (col, _) in &columns {
                out.lowered.statements.push(statement(
                    expanded_path(&col.catalog, &col.schema, &col.table, Some(&col.column)),
                    Derivation::ExpandedFromClassification,
                ));
            }
        }
        // The withhold holds against a DIRECT read only; a plain view reads
        // straight through it. The column's view-guard mask (A2) is what
        // holds through views.
        for (column, catalog_spelled) in columns {
            out.view_guard.push(PendingGuard {
                classification_id: g.classification_id,
                column,
                cleared: g.cleared.clone(),
                catalog_spelled,
            });
        }
    }
    Ok(())
}

/// A3 (revised 2026-10-14): at most ONE materialized-view guard per expanding
/// EP (matrix M15). Denying `CreateMaterializedView` to everyone keeps an
/// uncleared user from building a view that STORES real values (a mask does
/// not follow stored data).
///
/// **Why EP-wide.** Measured on StarRocks 4.1.6
/// (`docs/en/plans/2026-10-14-starrocks-mv-placement-measurement.md`): a
/// materialized view can only be created in [`MATERIALIZED_VIEW_CATALOG`], and
/// the privilege is checked on the TARGET database `default_catalog.<db>`,
/// never on the base table's catalog or database. The first draft's deny on
/// `{<external catalog>, <database>}` therefore matched no real `CREATE
/// MATERIALIZED VIEW`, and the target database is any one the builder may
/// write to, so the path is the catalog with every database. The first draft's
/// `…mv-{catalog}.{database}` policies are swept by the compiler prefix.
///
/// The exceptions are the users cleared for EVERY classification on the EP
/// ([`cleared_for_every`]), one set per gated tag that reaches a column or
/// (D2, C7) a whole table of the EP. **Cost (accepted by the user):** a user
/// not cleared for every classification on the engine cannot create any
/// materialized view there, including over unclassified data.
///
/// A group-only gate is skipped before it contributes, so it neither creates
/// the guard nor narrows its exceptions (the same skip as the column
/// withhold). An EP with no classification gets no guard. The statement's
/// source is the first gate that reached the EP; the guard has no tag of its
/// own.
fn materialized_view_guard(ep: &EpInput, reached: &[(&Gate, Vec<String>)]) -> Option<Statement> {
    if ask(
        ep.kind,
        ep.classification_channel,
        &Capability::WithholdThroughMaterializedView,
    ) != Support::Lowered(LoweringKind::DenyCreateMaterializedView)
    {
        return None;
    }
    let sets: Vec<Vec<String>> = reached.iter().map(|(_, c)| c.clone()).collect();
    let cleared = cleared_for_every(&sets)?;
    Some(Statement {
        source: source(reached[0].0),
        audience: everyone_except(&cleared),
        target: Target::Path(Path {
            catalog_id: None,
            catalog: MATERIALIZED_VIEW_CATALOG.to_string(),
            schema: Level::Unnamed,
            table: Level::Unnamed,
            column: Level::Unnamed,
        }),
        effect: Effect::Deny {
            privileges: vec![Privilege::CreateMaterializedView],
        },
        derivation: Derivation::MaterializedViewGuard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audience::MemberName;
    use crate::ep::{EpInput, EpKind, Mechanism};
    use crate::lower::rules::{CeilingCount, TaggedTable};
    use crate::statement::{Derivation, Effect, Privilege, ReasonCode, Target};
    use egret_perm::{CompileError, EngineType};
    use uuid::Uuid;

    fn fold(s: &str) -> String {
        s.to_lowercase()
    }
    struct Always;
    impl CeilingCount for Always {
        fn counts(&self, _: &str, _: &str, _: &str, _: Option<&str>) -> bool {
            true
        }
    }
    fn ep(engine: EngineType, channel: bool) -> EpInput {
        EpInput {
            profile_id: 1,
            profile_name: "p".into(),
            kind: EpKind {
                engine,
                mechanism: Mechanism::Ranger,
            },
            engines: Vec::new(),
            default_version: Err("not read".into()),
            default_image_level: None,
            classification_channel: channel,
            catalogs: Default::default(),
        }
    }
    fn trino_ep(channel: bool) -> EpInput {
        ep(EngineType::Trino, channel)
    }
    fn starrocks_ep() -> EpInput {
        ep(EngineType::StarRocks, false)
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
    fn gate(name: &str, roles: &[&str], cleared: Vec<String>) -> Gate {
        Gate {
            classification_id: Uuid::from_u128(9),
            classification_name: name.into(),
            required_roles: roles.iter().map(|r| r.to_string()).collect(),
            cleared,
            targets: Vec::new(),
        }
    }
    fn gate_with(
        name: &str,
        roles: &[&str],
        cleared: Vec<String>,
        targets: Vec<GateTarget>,
    ) -> Gate {
        Gate {
            targets,
            ..gate(name, roles, cleared)
        }
    }

    #[test]
    fn a_groups_only_gate_lowers_like_any_other_excepting_its_resolved_members() {
        // The loader resolves `required_groups` into `cleared`, so a gate with
        // no roles but cleared members is an ordinary withhold.
        let g = gate("pii", &[], vec!["bob".into()]);
        let out = lower_withholds(&trino_ep(true), &[g], fold, 500, &Always).unwrap();
        assert!(out.lowered.lowerings.is_empty());
        assert_eq!(out.lowered.statements.len(), 1);
        assert_eq!(
            out.lowered.statements[0].audience.except[0].name,
            MemberName::User("bob".into())
        );
    }

    #[test]
    fn trino_withholds_read_and_write_on_the_classification() {
        let g = gate("pii", &["analyst"], vec!["ann".into()]);
        let s = &lower_withholds(&trino_ep(true), &[g], fold, 500, &Always)
            .unwrap()
            .lowered
            .statements[0];
        assert_eq!(
            s.effect,
            Effect::Withhold {
                privileges: vec![Privilege::Read, Privilege::Write]
            }
        );
        assert_eq!(s.target, Target::Classification { name: "pii".into() });
        assert_eq!(s.audience.except[0].name, MemberName::User("ann".into()));
    }

    #[test]
    fn starrocks_withholds_read_per_column_whole_table_and_guards_materialized_views() {
        let known = GateTarget {
            table: table("c", "s", "t", true, &[]),
            shape: TableShape::Known {
                schema: "s".into(),
                table: "t".into(),
                columns: vec!["a".into(), "b".into()],
            },
        };
        let g = gate_with("pii", &["analyst"], vec!["ann".into()], vec![known]);
        let out = lower_withholds(&starrocks_ep(), &[g], fold, 500, &Always).unwrap();
        let names: Vec<_> = out
            .lowered
            .statements
            .iter()
            .map(|s| (s.derivation.clone(), s.target.clone()))
            .collect();
        assert_eq!(names[0].0, Derivation::WholeTable);
        assert!(out
            .lowered
            .statements
            .iter()
            .any(|s| s.derivation == Derivation::MaterializedViewGuard));
        assert!(out.lowered.statements.iter().all(|s| match &s.effect {
            Effect::Withhold { privileges } => privileges == &vec![Privilege::Read],
            Effect::Deny { privileges } => privileges == &vec![Privilege::CreateMaterializedView],
            _ => false,
        }));
        assert_eq!(
            out.view_guard.len(),
            2,
            "a NULL mask pending for each known column"
        );
    }

    #[test]
    fn a_view_target_is_not_measured_m14() {
        let v = GateTarget {
            table: table("c", "s", "v", true, &[]),
            shape: TableShape::View,
        };
        let out = lower_withholds(
            &starrocks_ep(),
            &[gate_with("pii", &["r"], vec![], vec![v])],
            fold,
            500,
            &Always,
        )
        .unwrap();
        assert_eq!(out.lowered.lowerings[0].code, ReasonCode::ViewNotMeasured);
        assert!(out.lowered.lowerings[0]
            .detail
            .contains("is a view in the catalog, not a table"));
    }

    fn pending(cleared: &[&str]) -> Vec<String> {
        cleared.iter().map(|s| s.to_string()).collect()
    }

    /// A3 (revised 2026-10-14): the materialized-view guard excepts the users
    /// cleared for every classification on the EP.
    #[test]
    fn the_guard_excepts_the_intersection_of_every_cleared_set() {
        assert_eq!(
            cleared_for_every(&[
                pending(&["ann", "bob"]),
                pending(&["ann", "bob"]),
                pending(&["bob", "cy"]),
            ]),
            Some(pending(&["bob"]))
        );
    }

    #[test]
    fn no_classified_column_means_no_guard() {
        assert_eq!(cleared_for_every(&[]), None);
    }

    #[test]
    fn an_empty_intersection_excepts_nobody() {
        assert_eq!(
            cleared_for_every(&[pending(&["ann"]), pending(&["bob"])]),
            Some(Vec::new())
        );
    }

    #[test]
    fn a_trino_profile_without_a_channel_lowers_to_a_skip() {
        let g = gate("pii", &["analyst"], vec![]);
        let out = lower_withholds(&trino_ep(false), &[g], fold, 500, &Always).unwrap();
        assert!(out.lowered.statements.is_empty());
        assert_eq!(
            out.lowered.lowerings[0].code,
            ReasonCode::NoClassificationChannel
        );
    }

    #[test]
    fn a_column_attachment_withholds_each_column_in_the_catalogs_spelling() {
        let t = GateTarget {
            table: table("c", "s", "t", false, &["EMAIL", "ssn"]),
            shape: TableShape::Known {
                schema: "S".into(),
                table: "T".into(),
                columns: vec!["email".into()],
            },
        };
        let out = lower_withholds(
            &starrocks_ep(),
            &[gate_with("pii", &["r"], vec!["ann".into()], vec![t])],
            fold,
            500,
            &Always,
        )
        .unwrap();
        let paths: Vec<_> = out
            .lowered
            .statements
            .iter()
            .filter(|s| s.derivation == Derivation::ExpandedFromClassification)
            .map(|s| s.target.clone())
            .collect();
        assert_eq!(
            paths,
            vec![
                expanded_path("c", "S", "T", Some("email")),
                expanded_path("c", "S", "T", Some("ssn")),
            ]
        );
        let spelled: Vec<bool> = out.view_guard.iter().map(|g| g.catalog_spelled).collect();
        assert_eq!(spelled, vec![true, false]);
    }

    #[test]
    fn an_unknown_whole_table_withholds_but_says_its_masks_are_missing() {
        let t = GateTarget {
            table: table("c", "s", "t", true, &[]),
            shape: TableShape::Unknown,
        };
        let out = lower_withholds(
            &starrocks_ep(),
            &[gate_with("pii", &["r"], vec![], vec![t])],
            fold,
            500,
            &Always,
        )
        .unwrap();
        assert_eq!(out.lowered.statements[0].derivation, Derivation::WholeTable);
        assert_eq!(out.lowered.lowerings[0].code, ReasonCode::TableNotInCatalog);
        assert!(out.view_guard.is_empty());
    }

    #[test]
    fn a_whole_table_costs_its_withhold_and_a_mask_per_column_and_refuses_over_the_ceiling() {
        let t = GateTarget {
            table: table("c", "s", "t", true, &[]),
            shape: TableShape::Known {
                schema: "s".into(),
                table: "t".into(),
                columns: vec!["a".into(), "b".into()],
            },
        };
        let g = gate_with("pii", &["r"], vec![], vec![t]);
        assert!(
            lower_withholds(&starrocks_ep(), std::slice::from_ref(&g), fold, 3, &Always).is_ok()
        );
        assert!(matches!(
            lower_withholds(&starrocks_ep(), &[g], fold, 2, &Always),
            Err(CompileError::ClearanceExpansionCeiling {
                count: 3,
                ceiling: 2,
                per_column_denies: 0,
                ..
            })
        ));
    }

    #[test]
    fn starrocks_withholds_read_only_and_trino_read_and_write() {
        assert_eq!(
            withheld_privileges(starrocks_ep().kind),
            vec![Privilege::Read]
        );
        assert_eq!(
            withheld_privileges(trino_ep(true).kind),
            vec![Privilege::Read, Privilege::Write]
        );
    }

    fn starrocks_ep_with(engines: Vec<crate::ep::BoundEngine>) -> EpInput {
        EpInput {
            profile_name: "sr".into(),
            engines,
            ..starrocks_ep()
        }
    }
    fn bound(name: &str, version: Result<&str, String>) -> crate::ep::BoundEngine {
        crate::ep::BoundEngine {
            name: name.into(),
            version: version.map(str::to_string),
            egret_image_level: None,
        }
    }

    #[test]
    fn the_view_guard_folds_at_each_distinct_bound_version() {
        let ep = starrocks_ep_with(vec![
            bound("sr-a", Ok("2.8.0")),
            bound("sr-b", Ok("2.9.0")),
            bound("sr-c", Ok("2.8.0")),
        ]);
        assert_eq!(
            view_guard_versions(&ep).unwrap(),
            vec!["2.8.0".to_string(), "2.9.0".to_string()]
        );
    }

    #[test]
    fn an_unmodelled_engine_names_itself_m13() {
        let ep = starrocks_ep_with(vec![bound(
            "sr-x",
            Err("image foo:1 is not a known image".into()),
        )]);
        let why = view_guard_versions(&ep).unwrap_err();
        assert!(why.starts_with(
            "view protection (clearance mask) not active for sr: engine sr-x runs an image \
             egret has not modelled"
        ));
        assert!(why.ends_with(". image foo:1 is not a known image"));
    }

    #[test]
    fn a_known_but_unmeasured_version_is_not_measured() {
        let ep = starrocks_ep_with(vec![bound("sr-y", Ok("2.10.0"))]);
        assert!(view_guard_versions(&ep).unwrap_err().contains("sr-y"));
    }

    #[test]
    fn with_no_engine_bound_the_default_image_decides() {
        let mut ep = starrocks_ep_with(vec![]);
        ep.default_version = Ok("2.8.0".into());
        assert_eq!(view_guard_versions(&ep).unwrap(), vec!["2.8.0".to_string()]);
        ep.default_version = Err("no default".into());
        assert!(view_guard_versions(&ep)
            .unwrap_err()
            .contains("engine (none bound; egret's default image) runs an image"));
    }

    #[test]
    fn the_first_unmodelled_engine_wins_over_later_ones() {
        let ep = starrocks_ep_with(vec![
            bound("sr-a", Ok("2.8.0")),
            bound("sr-b", Err("b".into())),
            bound("sr-c", Err("c".into())),
        ]);
        let why = view_guard_versions(&ep).unwrap_err();
        assert!(
            why.contains("engine sr-b ") && why.ends_with(". b"),
            "{why}"
        );
    }
}
