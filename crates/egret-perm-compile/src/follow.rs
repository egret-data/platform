//! What a table or column rename carries of the permission model (spec
//! `2026-10-27-grants-follow-table-rename-design.md`, §5.2): which grants and
//! path rules follow the object, which restrictive patterns would let it
//! escape, and the writes that copy, move or undo them on a caller's
//! transaction.
//!
//! Coordinate-based, so the table hook and a later view hold use it alike
//! (D19). Selection folds case as the engines do; a level is a Ranger glob
//! ([`egret_perm::eval::resource_level_matches`]).
//!
//! | row | follows (D3) | matched the source but not the destination (D4) |
//! |---|---|---|
//! | deny grant, path mask, path row filter | copied before the commit; the original stays (G2, D1) | refused (G4), unless the new name is covered (G5) |
//! | allow grant | moved in place after the commit (G2, D2) | left and reported |
//!
//! A destination namespace of more than one level cannot be named by a perm
//! row (D14): a following restriction refuses there, a following allow is
//! left.

use egret_perm::eval::resource_level_matches;
use egret_perm::{Effect, Grant, GrantOrigin, Rule, Scope, Selector};
use uuid::Uuid;

/// One side of a move: a table, or one of its columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coord {
    pub schema: String,
    pub table: String,
    pub column: Option<String>,
}

impl Coord {
    /// `schema.table` or `schema.table.column`, a multi-level namespace's
    /// levels joined with `.`.
    pub fn label(&self) -> String {
        let schema = self.schema.replace('\u{1f}', ".");
        match &self.column {
            Some(c) => format!("{schema}.{}.{c}", self.table),
            None => format!("{schema}.{}", self.table),
        }
    }

    fn is_multi_level(&self) -> bool {
        self.schema.contains('\u{1f}')
    }
}

/// One coordinate move in the catalog `catalog_id` (the registry catalog over
/// the warehouse). `from` and `to` both name a column, or neither does. A
/// namespace of several levels is spelled with lakekeeper's `\u{1f}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub catalog_id: Uuid,
    pub from: Coord,
    pub to: Coord,
}

impl Move {
    fn is_column(&self) -> bool {
        self.from.column.is_some()
    }
}

/// A grant or a path rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermRow {
    Grant(Grant),
    Rule(Rule),
}

impl PermRow {
    /// The row's path scope (a rule here is always path-selected).
    pub fn scope(&self) -> &Scope {
        match self {
            PermRow::Grant(g) => &g.scope,
            PermRow::Rule(r) => match &r.selector {
                Selector::Path(s) => s,
                Selector::Tag { .. } => unreachable!("only path rules are selected"),
            },
        }
    }

    /// `deny grant 7c1e… (sales.ord*)`, `mask rule pii-email (sales.*)`.
    pub fn describe(&self) -> String {
        let s = self.scope();
        let scope: Vec<&str> = [&s.schema, &s.table, &s.column]
            .into_iter()
            .map_while(|l| l.as_deref())
            .collect();
        let scope = if scope.is_empty() {
            "the whole catalog".to_string()
        } else {
            scope.join(".")
        };
        match self {
            PermRow::Grant(g) => format!("{} grant {} ({scope})", g.effect.as_str(), g.id),
            PermRow::Rule(r) => format!("{} rule {} ({scope})", rule_kind_word(r), r.name),
        }
    }
}

/// `mask` or `row-filter`.
pub fn rule_kind_word(r: &Rule) -> &'static str {
    match r.kind {
        egret_perm::RuleKind::Mask { .. } => "mask",
        egret_perm::RuleKind::RowFilter { .. } => "row-filter",
    }
}

/// A restrictive row the move would let the object escape, and what the new
/// name lacks (spec §5.9; empty until [`cover_via`] fills it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub row: PermRow,
    pub missing: Vec<String>,
}

/// What one move carries (spec §5.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PermFollow {
    /// Following deny grants: copied to the destination before the commit.
    pub copy_grants: Vec<Grant>,
    /// Following path rules: copied to the destination before the commit.
    pub copy_rules: Vec<Rule>,
    /// Following allow grants: moved to the destination after the commit.
    pub move_grants: Vec<Grant>,
    /// Restrictive rows that matched the source and would not reach the
    /// destination (D4, D14).
    pub refused: Vec<Refused>,
    /// Allow grants that matched the source and do not reach the destination
    /// (D4, D14): left as they are, reported.
    pub left: Vec<Grant>,
}

impl PermFollow {
    /// Nothing to copy, move, refuse or report.
    pub fn is_empty(&self) -> bool {
        self.copy_grants.is_empty()
            && self.copy_rules.is_empty()
            && self.move_grants.is_empty()
            && self.refused.is_empty()
            && self.left.is_empty()
    }

    /// Something is copied before the commit (the hold must wait).
    pub fn copies(&self) -> bool {
        !self.copy_grants.is_empty() || !self.copy_rules.is_empty()
    }
}

fn fold(s: &str) -> String {
    s.to_lowercase()
}

/// No `*` or `?` (the 0177 test).
pub fn is_literal(level: &str) -> bool {
    !level.contains(['*', '?'])
}

/// A named literal level equal to `value`, folded.
fn literal_eq(level: &Option<String>, value: &str) -> bool {
    level
        .as_deref()
        .is_some_and(|l| is_literal(l) && fold(l) == fold(value))
}

/// Does `scope` follow the move (D3)? Its schema and table levels (and, for a
/// column move, its column level) are literal and fold equal to the source.
fn follows(scope: &Scope, mv: &Move) -> bool {
    literal_eq(&scope.schema, &mv.from.schema)
        && literal_eq(&scope.table, &mv.from.table)
        && mv
            .from
            .column
            .as_deref()
            .is_none_or(|c| literal_eq(&scope.column, c))
}

/// Does `scope` reach `at` (D4)? Every named level matches, an unnamed one
/// matches anything. A table coordinate is matched on its schema and table;
/// a column coordinate on its column too. No named schema level reaches a
/// namespace of several levels (no perm row can name one).
pub fn reaches(scope: &Scope, at: &Coord) -> bool {
    let level =
        |l: &Option<String>, v: &str| l.as_deref().is_none_or(|l| resource_level_matches(l, v));
    if scope.schema.is_some() && at.is_multi_level() {
        return false;
    }
    level(&scope.schema, &at.schema)
        && level(&scope.table, &at.table)
        && at.column.as_deref().is_none_or(|c| level(&scope.column, c))
}

/// The scope a following row gets on the destination: the renamed levels
/// take the request's destination spelling, every other level keeps its own
/// (#757 C18).
pub fn moved_scope(scope: &Scope, mv: &Move) -> Scope {
    let pick = |own: &Option<String>, from: &str, to: &str| match own {
        Some(o) if fold(from) == fold(to) => Some(o.clone()),
        _ => Some(to.to_string()),
    };
    Scope {
        catalog_id: scope.catalog_id,
        schema: pick(&scope.schema, &mv.from.schema, &mv.to.schema),
        table: pick(&scope.table, &mv.from.table, &mv.to.table),
        column: match (&mv.from.column, &mv.to.column) {
            (Some(from), Some(to)) => pick(&scope.column, from, to),
            _ => scope.column.clone(),
        },
    }
}

/// Sort the explicit grants and path rules of `mv.catalog_id` for one move
/// (spec §5.2, D3, D4, D14). Pure: `grants` and `rules` are every row the
/// caller read.
pub fn plan(grants: &[Grant], rules: &[Rule], mv: &Move) -> PermFollow {
    let mut out = PermFollow::default();
    let into_multi_level = mv.to.is_multi_level();
    let restrictive = |out: &mut PermFollow, row: PermRow, follows: bool| {
        let escapes = {
            let scope = row.scope();
            reaches(scope, &mv.from) && !reaches(scope, &mv.to)
        };
        if follows && !into_multi_level {
            match row {
                PermRow::Grant(g) => out.copy_grants.push(g),
                PermRow::Rule(r) => out.copy_rules.push(r),
            }
        } else if follows || escapes {
            out.refused.push(Refused {
                row,
                missing: Vec::new(),
            });
        }
    };
    for g in grants {
        if g.scope.catalog_id != mv.catalog_id || g.origin != GrantOrigin::Explicit {
            continue;
        }
        if mv.is_column() && g.scope.column.is_none() {
            // A table-level row is not about a column: it reaches both names.
            continue;
        }
        let f = follows(&g.scope, mv);
        match g.effect {
            Effect::Deny => restrictive(&mut out, PermRow::Grant(g.clone()), f),
            Effect::Allow => {
                if f && !into_multi_level {
                    out.move_grants.push(g.clone());
                } else if f || (reaches(&g.scope, &mv.from) && !reaches(&g.scope, &mv.to)) {
                    out.left.push(g.clone());
                }
            }
        }
    }
    for r in rules {
        let Selector::Path(scope) = &r.selector else {
            continue;
        };
        if scope.catalog_id != mv.catalog_id {
            continue;
        }
        if mv.is_column() && scope.column.is_none() {
            continue;
        }
        let f = follows(scope, mv);
        restrictive(&mut out, PermRow::Rule(r.clone()), f);
    }
    out
}

/// [`plan`] over the catalog's rows as stored, on `conn`.
///
/// # Errors
/// The store's error.
pub async fn plan_via(
    conn: &mut sqlx::PgConnection,
    mv: &Move,
) -> Result<PermFollow, egret_perm::PermError> {
    let grants = grants_of_catalog_via(conn, mv.catalog_id).await?;
    let rules = path_rules_of_catalog_via(conn, mv.catalog_id).await?;
    Ok(plan(&grants, &rules, mv))
}

/// Every explicit grant of `catalog_id`.
async fn grants_of_catalog_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<Vec<Grant>, egret_perm::PermError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM perm.grants WHERE catalog_id = $1 AND origin = 'explicit' ORDER BY id",
    )
    .bind(catalog_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(g) = egret_perm::get_grant_via(conn, id).await? {
            out.push(g);
        }
    }
    Ok(out)
}

/// Every path rule of `catalog_id`.
async fn path_rules_of_catalog_via(
    conn: &mut sqlx::PgConnection,
    catalog_id: Uuid,
) -> Result<Vec<Rule>, egret_perm::PermError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM perm.rules WHERE selector_kind = 'path' AND catalog_id = $1 ORDER BY id",
    )
    .bind(catalog_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(r) = egret_perm::get_rule_via(conn, id).await? {
            out.push(r);
        }
    }
    Ok(out)
}
