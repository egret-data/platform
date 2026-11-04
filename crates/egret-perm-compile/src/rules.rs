//! `compile_rule`: turns one `perm.rules` row (plus its exempt set) into the
//! Ranger masking/row-filter JSON bodies for one engine. Since governance
//! sub-project 1 Task 6 it is two calls: `egret_perm_ir::lower::rules::lower_rule`
//! makes every decision (the D5 asymmetry, the expansion ceiling, every
//! refusal; read that module's doc), and `crate::ranger::rules::render` renders
//! each statement into a Ranger body (read that module's doc for the
//! `serviceType` stamp and where each body is stored). What stays here is the
//! loader-side grouping of a tag's assignments ([`group_by_table`]) and the
//! public entry point's inputs.
//!
//! **`ResolvedNames` (added in the round-1 review fix, Criticals 1 and 2).**
//! The first version of `compile_rule` took no resolved name at all: a tag
//! selector stamped `Selector::Tag`'s `key` straight into the tag policy, and
//! a path selector stamped `scope.catalog_id`'s raw UUID as the catalog
//! resource. Both deploy cleanly, are listed by Ranger, and match no
//! resource — egret's own tag download pushes `tags.name` (a separate,
//! mutable column from `tags.key`;
//! `egret-policy/src/store.rs:245` → `egret-policy-api/src/dto.rs:184`;
//! `migrations/0080_tags.sql`), and every engine identifies a catalog by
//! name, never by id. Spec D6 ("resolves key → name and id → name at compile
//! time") named both halves; the original signature carried neither. The
//! compiler still does no I/O — `ResolvedNames` is caller-resolved, and a
//! tag selector with `tag_name: None` refuses rather than falling back to
//! the key.

use egret_perm::{CompileError, EngineType, Rule};
use egret_perm_ir::ep::{EpKind, Mechanism};
use egret_perm_ir::lower::rules::{lower_rule, RuleInputs, TaggedTable};
use serde_json::Value;

use crate::table_columns::Leaving;

pub use egret_perm_ir::lower::rules::TaggedColumn;

/// One table a tag reaches on a StarRocks profile, at either level (spec
/// §5.1). Ranger matches every StarRocks level case-insensitively, so the
/// assignments are grouped under the FOLDED `(catalog, schema, table)`
/// ([`egret_ranger_eval::resource::fold_identifier`]) and each name keeps the
/// first spelling in the expansion's order: a table tagged in one spelling
/// compiles exactly as before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableTarget {
    pub catalog: String,
    /// The egret warehouse (`catalog.warehouses.id`) the table lives in.
    pub warehouse_id: uuid::Uuid,
    pub schema: String,
    pub table: String,
    /// The tag is attached to the whole table (`column_name = ''`).
    pub table_level: bool,
    /// The columns the tag is attached to, one per folded name.
    pub columns: Vec<String>,
}

impl TableTarget {
    /// The table as the IR lowering reads it (the warehouse is the loader's).
    pub(crate) fn to_tagged(&self) -> TaggedTable {
        TaggedTable {
            catalog: self.catalog.clone(),
            schema: self.schema.clone(),
            table: self.table.clone(),
            table_level: self.table_level,
            columns: self.columns.clone(),
        }
    }
}

/// Group a tag's assignments (both levels, as
/// `egret_policy::tag_assignments_for_service_via` returns them) into one
/// [`TableTarget`] per folded table, in first-seen order.
pub fn group_by_table(
    rows: impl IntoIterator<Item = egret_policy::ServiceTagAssignmentRow>,
) -> Vec<TableTarget> {
    use egret_ranger_eval::resource::fold_identifier as fold;
    let mut out: Vec<TableTarget> = Vec::new();
    let mut index: std::collections::HashMap<(String, String, String), usize> =
        std::collections::HashMap::new();
    for row in rows {
        let key = (
            fold(&row.catalog),
            fold(&row.schema_name),
            fold(&row.table_name),
        );
        let i = *index.entry(key).or_insert_with(|| {
            out.push(TableTarget {
                catalog: row.catalog.clone(),
                warehouse_id: row.warehouse_id,
                schema: row.schema_name.clone(),
                table: row.table_name.clone(),
                table_level: false,
                columns: Vec::new(),
            });
            out.len() - 1
        });
        let target = &mut out[i];
        if row.column_name.is_empty() {
            target.table_level = true;
        } else if !target
            .columns
            .iter()
            .any(|c| fold(c) == fold(&row.column_name))
        {
            target.columns.push(row.column_name);
        }
    }
    out
}

/// The engine-facing names `compile_rule` cannot resolve itself (no registry
/// or tag I/O in this compiler). `catalog_name` is used only for a `Path`
/// selector (`compile_grant`'s equivalent parameter); `tag_name` is used
/// only for a `Tag` selector, and MUST be the tag's `tags.name` — never
/// `tags.key`, which is stable but not what the Ranger tag plugin matches on
/// (D6). A `Tag`-selector rule with `tag_name: None` is a `CompileError`
/// (`MissingTagName`), never a silent fallback to the key.
pub struct ResolvedNames<'a> {
    pub catalog_name: &'a str,
    pub tag_name: Option<&'a str>,
}

/// Compile one `Rule` into the Ranger masking/row-filter JSON bodies for
/// engine `e`: [`lower_rule`] with the classification channel present exactly
/// when `tag_service` is, and every statement it returns rendered by
/// `crate::ranger::rules::render`. Every refusal is `lower_rule`'s (read its
/// doc for what each selector lowers to and in which order the refusals run);
/// `MissingTagService` is still an `Err` here, and only the recompile driver
/// turns it into a skip.
///
/// - A tag selector on Trino compiles to exactly one tag policy (D5) —
///   `tag_service` and `resolved.tag_name` must both be `Some` — plus, for a
///   row filter, one resource row filter per table in `tagged` carrying the
///   tag on columns only (KI-287).
/// - A tag selector on StarRocks compiles a MASK to one policy per tagged
///   column in `tagged` and a ROW FILTER to one policy per table in `tagged`
///   (D2, C8), refused above `ceiling` with `CompileError::ExpansionCeiling`.
/// - A path selector compiles to exactly one policy on either engine, at
///   `r`'s own scope with `resolved.catalog_name` as the catalog resource.
///
/// `service` is the target Ranger service name stamped onto the body (for a
/// tag policy, `tagify` overwrites it with `tag_service` regardless of what
/// is passed here).
pub fn compile_rule(
    r: &Rule,
    e: EngineType,
    service: &str,
    tag_service: Option<&str>,
    resolved: &ResolvedNames,
    tagged: &[TableTarget],
    ceiling: usize,
) -> Result<Vec<Value>, CompileError> {
    let inputs = RuleInputs {
        classification_name: resolved.tag_name.map(str::to_string),
        tagged: tagged.iter().map(TableTarget::to_tagged).collect(),
    };
    let kind = EpKind {
        engine: e,
        mechanism: Mechanism::Ranger,
    };
    lower_rule(
        r,
        &egret_perm_ir::audience::audience_from_expanded(&r.exempt),
        kind,
        tag_service.is_some(),
        resolved.catalog_name,
        &inputs,
        ceiling,
        &Leaving::default(),
    )?
    .iter()
    .map(|s| crate::ranger::rules::render(s, service, tag_service, e))
    .collect()
}
