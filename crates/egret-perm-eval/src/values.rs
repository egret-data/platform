//! Row filters and column masks over a [`Snapshot`] (plan D2 "Row filters"
//! and "Masks"), and the Ranger Trino plugin's mask rendering (plan D1).
//!
//! The evaluator never filters or masks less than the Ranger artifact
//! compiled from the same IR would: every applying filter is ANDed, and two
//! distinct masks on one column are an error rather than a choice.

use std::collections::{BTreeMap, BTreeSet};

use egret_perm_ir::statement::{Effect, Level, Statement, Target};

use crate::decide::{applies, level_covers, path_on_table, EvalError};
use crate::snapshot::Snapshot;

/// `dataMaskDef.maskTypes[].transformer` of the Trino service-def
/// (`crates/egret-policy/src/service-defs/trino.json`), for every type that
/// declares one. `tests/values.rs` pins it to that file.
pub const TRINO_MASK_TRANSFORMERS: &[(&str, &str)] = &[
    (
        "MASK",
        "cast(regexp_replace(regexp_replace(regexp_replace({col},'([A-Z])', 'X'),'([a-z])','x'),'([0-9])','0') as {type})",
    ),
    (
        "MASK_SHOW_LAST_4",
        "cast(regexp_replace({col}, '(.*)(.{4}$)', x -> regexp_replace(x[1], '.', 'X') || x[2]) as {type})",
    ),
    (
        "MASK_SHOW_FIRST_4",
        "cast(regexp_replace({col}, '(^.{4})(.*)', x -> x[1] || regexp_replace(x[2], '.', 'X')) as {type})",
    ),
    (
        "MASK_HASH",
        "cast(to_hex(sha256(to_utf8({col}))) as {type})",
    ),
    ("MASK_DATE_SHOW_YEAR", "date_trunc('year', {col})"),
];

/// The schema every Trino catalog carries for its own metadata. A
/// catalog-scoped path mask (schema level unnamed) does not reach it: the
/// Ranger compile renders that mask's schema level as
/// `{values: ["information_schema"], isExcludes: true}`
/// (`egret-perm-compile` `ranger::grants::scope_to_resources`, PR #837),
/// because Trino asks for column masks on its relations like any table and a
/// mask there would hash what `SHOW TABLES`, `SHOW COLUMNS` and `DESCRIBE`
/// return. The OPA plugin asks the same `GetColumnMask` questions, so the
/// evaluator excludes it alike.
const INFORMATION_SCHEMA: &str = "information_schema";

const MASK_NULL: &str = "MASK_NULL";
const MASK_NONE: &str = "MASK_NONE";
const CUSTOM: &str = "CUSTOM";

/// The Ranger Trino plugin's `getColumnMask` rendering:
/// - `MASK_NONE` is no mask;
/// - `MASK_NULL` is `NULL`;
/// - `CUSTOM` is the masked value, or `NULL` when there is none;
/// - every other type is the service-def transformer.
///
/// The plugin then replaces `{col}` with the column name and `{type}` with the
/// type's display name in whichever expression it chose, CUSTOM's included.
///
/// Two places are stricter than the plugin, never looser: a blank CUSTOM value
/// renders `NULL` (the plugin would mask nothing), and a type with no
/// transformer renders `NULL` (the plugin finds no transformer and masks
/// nothing; the lowering refuses undeclared types before this is reached).
pub fn render_mask(
    mask_type: &str,
    custom: Option<&str>,
    column: &str,
    column_type: &str,
) -> Option<String> {
    let expr = match mask_type {
        MASK_NONE => return None,
        MASK_NULL => "NULL",
        CUSTOM => custom.filter(|c| !c.trim().is_empty()).unwrap_or("NULL"),
        other => TRINO_MASK_TRANSFORMERS
            .iter()
            .find(|(n, _)| *n == other)
            .map_or("NULL", |(_, t)| *t),
    };
    Some(expr.replace("{col}", column).replace("{type}", column_type))
}

/// One applying mask before rendering: `(mask_type, custom expression)`.
type MaskChoice = (String, Option<String>);

impl Snapshot {
    /// The AND of every distinct applying row filter on the table, in
    /// statement order; empty when none. Identical expressions are merged
    /// first (the agreeing ties `ambiguous_arbitration` admits).
    pub fn row_filters(
        &self,
        user: &str,
        catalog: &str,
        schema: &str,
        table: &str,
    ) -> Result<Vec<String>, EvalError> {
        let classes = self.table_classes(catalog, schema, table);
        let mut seen: Vec<&str> = Vec::new();
        for i in self.candidates(catalog, Some(schema), Some(table), &classes) {
            let s = &self.statements[i];
            let Effect::RowFilter { expression } = &s.effect else {
                continue;
            };
            // An empty filterExpr is Ranger's "no filtering" (the exempt idiom).
            if expression.is_empty() || !applies(&s.audience, user) {
                continue;
            }
            // A row-filter policy has no column level, and an unnamed level
            // renders `*` (`scope_to_resources`, `PolicyTarget::RowFilter`). A
            // classification reaches the table only through the table's own tags
            // (KI-287: column-only tables get their own path statements).
            let hit = match &s.target {
                Target::Path(p) => path_on_table(p, catalog, schema, table),
                Target::Classification { name } => classes.contains(&name.to_lowercase()),
                Target::EngineObject { .. } | Target::Infra(_) => false,
            };
            if hit && !seen.contains(&expression.as_str()) {
                seen.push(expression);
            }
        }
        if seen.is_empty() {
            return Ok(Vec::new());
        }
        let anded = seen
            .iter()
            .map(|e| format!("({e})"))
            .collect::<Vec<_>>()
            .join(" AND ");
        Ok(vec![anded])
    }

    /// Column name → SQL expression, only for masked columns. `columns` are
    /// (name, Trino type display name).
    pub fn column_masks(
        &self,
        user: &str,
        catalog: &str,
        schema: &str,
        table: &str,
        columns: &[(String, String)],
    ) -> Result<BTreeMap<String, String>, EvalError> {
        let mut out = BTreeMap::new();
        for (column, column_type) in columns {
            if let Some((mask_type, custom)) =
                self.column_mask_choice(user, catalog, schema, table, column)?
            {
                if let Some(expr) = render_mask(&mask_type, custom.as_deref(), column, column_type)
                {
                    out.insert(column.clone(), expr);
                }
            }
        }
        Ok(out)
    }

    /// The applying mask before rendering, for explain and the differential:
    /// (mask_type, custom expression). A `WithholdMask` is `MASK_NULL`;
    /// identical masks are merged; two distinct ones are `TwoMasks`.
    pub fn column_mask_choice(
        &self,
        user: &str,
        catalog: &str,
        schema: &str,
        table: &str,
        column: &str,
    ) -> Result<Option<(String, Option<String>)>, EvalError> {
        let mut classes = self.table_classes(catalog, schema, table);
        classes.extend(self.column_classes(catalog, schema, table, column));
        let mut chosen: Option<MaskChoice> = None;
        for i in self.candidates(catalog, Some(schema), Some(table), &classes) {
            let s = &self.statements[i];
            let Some(choice) = mask_of(s) else {
                continue;
            };
            if !applies(&s.audience, user) {
                continue;
            }
            // An unnamed column covers every column of the tables named. A
            // rule mask whose schema is unnamed (catalog-scoped) skips
            // `information_schema`, as the Ranger artifact does.
            let hit = match &s.target {
                Target::Path(p) => {
                    path_on_table(p, catalog, schema, table)
                        && level_covers(&p.column, Some(column))
                        && !(matches!(s.effect, Effect::Mask { .. })
                            && p.schema == Level::Unnamed
                            && schema.eq_ignore_ascii_case(INFORMATION_SCHEMA))
                }
                Target::Classification { name } => classes.contains(&name.to_lowercase()),
                Target::EngineObject { .. } | Target::Infra(_) => false,
            };
            if !hit {
                continue;
            }
            match &chosen {
                None => chosen = Some(choice),
                Some(c) if *c == choice => {}
                Some(_) => {
                    return Err(EvalError::TwoMasks {
                        user: user.to_string(),
                        column: column.to_string(),
                    })
                }
            }
        }
        Ok(chosen)
    }

    /// The classifications on the table itself, lowercased.
    fn table_classes(&self, catalog: &str, schema: &str, table: &str) -> BTreeSet<String> {
        self.index
            .classes
            .get(&table_key(catalog, schema, table))
            .map(|tc| tc.table.clone())
            .unwrap_or_default()
    }

    /// The classifications on one column, lowercased.
    fn column_classes(
        &self,
        catalog: &str,
        schema: &str,
        table: &str,
        column: &str,
    ) -> BTreeSet<String> {
        self.index
            .classes
            .get(&table_key(catalog, schema, table))
            .and_then(|tc| tc.columns.get(&column.to_lowercase()).cloned())
            .unwrap_or_default()
    }
}

fn table_key(catalog: &str, schema: &str, table: &str) -> (String, String, String) {
    (
        catalog.to_lowercase(),
        schema.to_lowercase(),
        table.to_lowercase(),
    )
}

/// The mask a statement carries: its own, or `MASK_NULL` for a withhold's
/// view guard.
fn mask_of(s: &Statement) -> Option<MaskChoice> {
    match &s.effect {
        Effect::Mask {
            mask_type,
            expression,
        } => Some((mask_type.clone(), expression.clone())),
        Effect::WithholdMask => Some((MASK_NULL.to_string(), None)),
        _ => None,
    }
}
