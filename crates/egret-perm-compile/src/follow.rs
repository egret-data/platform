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
    /// The row follows, but into a namespace of several levels (D14).
    pub multi_level: bool,
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
    /// Restrictive pattern rows that would refuse, but whose restriction the
    /// new name already carries (G5). They change nothing; the hold still
    /// runs, so the cover cannot be deleted while it does (D10).
    pub covered: Vec<PermRow>,
}

impl PermFollow {
    /// Nothing to copy, move, refuse or report.
    pub fn is_empty(&self) -> bool {
        self.copy_grants.is_empty()
            && self.copy_rules.is_empty()
            && self.move_grants.is_empty()
            && self.refused.is_empty()
            && self.left.is_empty()
            && self.covered.is_empty()
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
                multi_level: follows,
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
    // G5: a pattern row whose restriction the new name already carries, or
    // will carry through this move's own copies (D21), does not refuse.
    let pending = out.clone();
    for mut r in std::mem::take(&mut out.refused) {
        if r.multi_level {
            out.refused.push(r);
            continue;
        }
        match cover(&r.row, grants, rules, mv, &pending) {
            Cover::Full => out.covered.push(r.row),
            Cover::Partial { missing } => {
                r.missing = missing;
                out.refused.push(r);
            }
        }
    }
    out
}

// ── a covered destination (G5, spec §5.9, D21) ─────────────────────────────

/// Whether the destination already carries a refused pattern row's
/// restriction (spec §5.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cover {
    Full,
    /// What the new name lacks, one sentence per gap.
    Partial {
        missing: Vec<String>,
    },
}

fn principal_label(p: &egret_perm::Principal) -> String {
    format!("{}:{}", p.kind.as_str(), p.r#ref)
}

fn same_principal(a: &egret_perm::Principal, b: &egret_perm::Principal) -> bool {
    a.kind == b.kind && a.r#ref == b.r#ref
}

/// `a ⊆ b`, principals compared as stored.
fn subset(a: &[egret_perm::Principal], b: &[egret_perm::Principal]) -> bool {
    a.iter().all(|x| b.iter().any(|y| same_principal(x, y)))
}

/// Does a candidate's column level reach what the refused row restricts?
/// For a column move the destination coordinate carries the new column and
/// [`reaches`] decides; for a table move a table-level refused row needs a
/// table-level candidate (no column, or `*`), and a column-level one a
/// candidate naming the same column (folded) or none.
fn column_covers(candidate: &Scope, refused: &Scope, mv: &Move) -> bool {
    if mv.is_column() {
        return true;
    }
    match candidate.column.as_deref() {
        None | Some("*") => true,
        Some(c) => refused
            .column
            .as_deref()
            .is_some_and(|r| r != "*" && fold(r) == fold(c)),
    }
}

/// G5 for one refused row: the candidates are the catalog's deny grants and
/// path rules that reach the destination, plus the copies `pending` is about
/// to insert (D21). Tag rules and hand-written policies never count.
pub fn cover(
    refused: &PermRow,
    grants: &[Grant],
    rules: &[Rule],
    mv: &Move,
    pending: &PermFollow,
) -> Cover {
    let to = mv.to.label();
    let copies_g: Vec<Grant> = pending
        .copy_grants
        .iter()
        .map(|g| Grant {
            scope: moved_scope(&g.scope, mv),
            ..g.clone()
        })
        .collect();
    let copies_r: Vec<Rule> = pending
        .copy_rules
        .iter()
        .filter_map(|r| match &r.selector {
            Selector::Path(s) => Some(Rule {
                selector: Selector::Path(moved_scope(s, mv)),
                ..r.clone()
            }),
            Selector::Tag { .. } => None,
        })
        .collect();
    let reaches_to = |s: &Scope| s.catalog_id == mv.catalog_id && reaches(s, &mv.to);
    match refused {
        PermRow::Grant(r) => {
            let candidates: Vec<&Grant> = grants
                .iter()
                .chain(&copies_g)
                .filter(|c| {
                    c.effect == Effect::Deny
                        && c.origin == GrantOrigin::Explicit
                        && reaches_to(&c.scope)
                        && column_covers(&c.scope, &r.scope, mv)
                        && subset(&c.principals.exclude, &r.principals.exclude)
                })
                .collect();
            let mut missing = Vec::new();
            for a in &r.actions {
                let lacking: Vec<String> = r
                    .principals
                    .include
                    .iter()
                    .filter(|p| {
                        !candidates.iter().any(|c| {
                            c.actions.contains(a)
                                && c.principals.include.iter().any(|q| same_principal(p, q))
                        })
                    })
                    .map(principal_label)
                    .collect();
                if !lacking.is_empty() {
                    missing.push(format!(
                        "{to} lacks a deny of {} for {}",
                        a.as_str(),
                        lacking.join(", ")
                    ));
                }
            }
            if missing.is_empty() {
                Cover::Full
            } else {
                Cover::Partial { missing }
            }
        }
        PermRow::Rule(r) => {
            let Selector::Path(rs) = &r.selector else {
                return Cover::Partial {
                    missing: vec![format!("{to} is not covered")],
                };
            };
            let candidates: Vec<&Rule> = rules
                .iter()
                .chain(&copies_r)
                .filter(|c| match &c.selector {
                    Selector::Path(cs) => reaches_to(cs),
                    Selector::Tag { .. } => false,
                })
                .collect();
            let exemptions_fit = |c: &Rule| {
                subset(&c.exempt.include, &r.exempt.include)
                    && subset(&r.exempt.exclude, &c.exempt.exclude)
            };
            match &r.kind {
                egret_perm::RuleKind::Mask {
                    mask_type,
                    value_expr,
                } => {
                    let column = rs.column.clone().unwrap_or_default();
                    let same_mask: Vec<&&Rule> = candidates
                        .iter()
                        .filter(|c| {
                            let Selector::Path(cs) = &c.selector else {
                                return false;
                            };
                            let same_column = mv.is_column()
                                || cs
                                    .column
                                    .as_deref()
                                    .is_some_and(|cc| fold(cc) == fold(&column));
                            same_column
                                && matches!(&c.kind, egret_perm::RuleKind::Mask {
                                    mask_type: t, value_expr: e
                                } if t == mask_type && e == value_expr)
                        })
                        .collect();
                    if same_mask.iter().any(|c| exemptions_fit(c)) {
                        return Cover::Full;
                    }
                    let column = mv.to.column.clone().unwrap_or(column);
                    let expr = value_expr.as_deref().map_or_else(
                        || "no expression".to_string(),
                        |e| format!("expression {e}"),
                    );
                    let missing = if same_mask.is_empty() {
                        format!("{to} has no mask on {column} with type {mask_type} and {expr}")
                    } else {
                        format!("the mask on {column} of {to} exempts more principals")
                    };
                    Cover::Partial {
                        missing: vec![missing],
                    }
                }
                egret_perm::RuleKind::RowFilter { filter_expr } => {
                    let same_filter: Vec<&&Rule> = candidates
                        .iter()
                        .filter(|c| {
                            matches!(&c.kind, egret_perm::RuleKind::RowFilter { filter_expr: e }
                                if e == filter_expr)
                        })
                        .collect();
                    if same_filter.iter().any(|c| exemptions_fit(c)) {
                        return Cover::Full;
                    }
                    let missing = if same_filter.is_empty() {
                        format!("{to} has no row filter \"{filter_expr}\"")
                    } else {
                        format!("the row filter \"{filter_expr}\" on {to} exempts more principals")
                    };
                    Cover::Partial {
                        missing: vec![missing],
                    }
                }
            }
        }
    }
}

/// [`cover`] over the catalog's rows as stored, on `conn`.
///
/// # Errors
/// The store's error.
pub async fn cover_via(
    conn: &mut sqlx::PgConnection,
    refused: &PermRow,
    mv: &Move,
    pending: &PermFollow,
) -> Result<Cover, egret_perm::PermError> {
    let grants = grants_of_catalog_via(conn, mv.catalog_id).await?;
    let rules = path_rules_of_catalog_via(conn, mv.catalog_id).await?;
    Ok(cover(refused, &grants, &rules, mv, pending))
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

// ── hand-written policies (D5, G3) ─────────────────────────────────────────

/// One hand-written policy, as a refusal or a report names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handwritten {
    pub id: i64,
    pub profile: String,
    pub name: String,
}

impl Handwritten {
    /// `profile/name (id)`.
    pub fn label(&self) -> String {
        format!("{}/{} ({})", self.profile, self.name, self.id)
    }
}

/// What D5 found for one move.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandwrittenCheck {
    /// Restrictive (a mask, a row filter, a deny, or an exclusion list on the
    /// renamed level): the rename is refused.
    pub refuse: Vec<Handwritten>,
    /// A plain literal allow: reported.
    pub report: Vec<Handwritten>,
}

/// One resource level of a Ranger policy: `(values, isExcludes)`.
fn resource<'a>(body: &'a serde_json::Value, level: &str) -> Option<(Vec<&'a str>, bool)> {
    let r = body.get("resources")?.get(level)?;
    let values = r
        .get("values")?
        .as_array()?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let excludes = r
        .get("isExcludes")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    Some((values, excludes))
}

/// Does a level (glob values, honouring `isExcludes`) reach `value`?
fn level_reaches((values, excludes): &(Vec<&str>, bool), value: &str) -> bool {
    values.iter().any(|v| resource_level_matches(v, value)) != *excludes
}

/// Does a level name `value` literally, in either polarity?
fn names_literally((values, _): &(Vec<&str>, bool), value: &str) -> bool {
    values
        .iter()
        .any(|v| is_literal(v) && fold(v) == fold(value))
}

fn non_empty_list(body: &serde_json::Value, key: &str) -> bool {
    body.get(key)
        .and_then(serde_json::Value::as_array)
        .is_some_and(|a| !a.is_empty())
}

/// Classify one hand-written policy body for `mv` (D5): `None` when it does
/// not name the source literally, else `Some(true)` when it is restrictive.
pub fn handwritten_names(body: &serde_json::Value, catalog_name: &str, mv: &Move) -> Option<bool> {
    let catalog = resource(body, "catalog")?;
    let schema = resource(body, "schema").or_else(|| resource(body, "database"))?;
    let table = resource(body, "table")?;
    if !level_reaches(&catalog, catalog_name) || !level_reaches(&schema, &mv.from.schema) {
        return None;
    }
    // The renamed level and its own `isExcludes`.
    let renamed = match &mv.from.column {
        None => {
            if !names_literally(&table, &mv.from.table) {
                return None;
            }
            table
        }
        Some(c) => {
            if !level_reaches(&table, &mv.from.table) {
                return None;
            }
            let column = resource(body, "column")?;
            if !names_literally(&column, c) {
                return None;
            }
            column
        }
    };
    let policy_type = body
        .get("policyType")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    Some(
        policy_type == 1
            || policy_type == 2
            || non_empty_list(body, "denyPolicyItems")
            || non_empty_list(body, "denyExceptions")
            || renamed.1,
    )
}

/// D5 over the enabled operator policies of `services` (the profiles bound to
/// the warehouse), `catalog_name` the registry catalog's name.
///
/// # Errors
/// The database's error.
pub async fn handwritten_via(
    conn: &mut sqlx::PgConnection,
    services: &[i64],
    catalog_name: &str,
    mv: &Move,
) -> Result<HandwrittenCheck, sqlx::Error> {
    let rows: Vec<(i64, String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT p.id, s.name, p.name, p.policy_jsonb FROM policy.authz_policies p \
           JOIN policy.authz_services s ON s.id = p.service_id \
          WHERE p.service_id = ANY($1) AND p.enabled AND p.managed_by = 'operator' \
          ORDER BY s.name, p.name, p.id",
    )
    .bind(services)
    .fetch_all(&mut *conn)
    .await?;
    let mut out = HandwrittenCheck::default();
    for (id, profile, name, body) in rows {
        let Some(restrictive) = handwritten_names(&body, catalog_name, mv) else {
            continue;
        };
        let h = Handwritten { id, profile, name };
        if restrictive {
            out.refuse.push(h);
        } else {
            out.report.push(h);
        }
    }
    Ok(out)
}

// ── the writes (D1, D2, D13) ───────────────────────────────────────────────

/// The rows one move's copy inserted: `(source id, copy id)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Copies {
    pub grants: Vec<(Uuid, Uuid)>,
    pub rules: Vec<(Uuid, Uuid)>,
}

impl Copies {
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty() && self.rules.is_empty()
    }
}

fn folded_scope(s: &Scope) -> Scope {
    let f = |l: &Option<String>| l.as_deref().map(fold);
    Scope {
        catalog_id: s.catalog_id,
        schema: f(&s.schema),
        table: f(&s.table),
        column: f(&s.column),
    }
}

/// D13's equivalence key of a grant: its canonical body (as a migration
/// hashes it) with the id cleared and the scope folded.
fn grant_key(g: &Grant) -> String {
    let mut g = g.clone();
    g.id = Uuid::nil();
    g.scope = folded_scope(&g.scope);
    crate::migrate::grant_hash(&g)
}

/// D13's equivalence key of a path rule: as [`grant_key`], the name cleared.
fn rule_key(r: &Rule) -> String {
    let mut r = r.clone();
    r.id = Uuid::nil();
    r.name = String::new();
    if let Selector::Path(s) = &r.selector {
        r.selector = Selector::Path(folded_scope(s));
    }
    crate::migrate::rule_hash(&r)
}

/// `<name> (<schema>.<table>)`, or `.<column>` for a column move.
fn copy_name(r: &Rule, mv: &Move) -> String {
    format!("{} ({})", r.name, mv.to.label())
}

/// Copy `plan`'s restrictive rows to the destination on `tx` (D1): each a new
/// row with the source's body and principals or exemptions, the scope
/// re-pointed ([`moved_scope`]). A copy whose equivalent already sits on the
/// destination is skipped (D13). A rule copy is named
/// `"<name> (<destination>)"`, with ` #2`, ` #3` … on a name collision.
///
/// # Errors
/// The store's error; a database CHECK refusing a destination name.
pub async fn copy_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    plan: &PermFollow,
    mv: &Move,
) -> Result<Copies, egret_perm::PermError> {
    let mut out = Copies::default();
    if !plan.copies() {
        return Ok(out);
    }
    let existing_grants: Vec<String> = grants_of_catalog_via(tx, mv.catalog_id)
        .await?
        .iter()
        .map(grant_key)
        .collect();
    let existing_rules: Vec<String> = path_rules_of_catalog_via(tx, mv.catalog_id)
        .await?
        .iter()
        .map(rule_key)
        .collect();
    for g in &plan.copy_grants {
        let copy = Grant {
            id: Uuid::nil(),
            scope: moved_scope(&g.scope, mv),
            ..g.clone()
        };
        if existing_grants.contains(&grant_key(&copy)) {
            continue;
        }
        let id = egret_perm::create_grant_in_tx(tx, &copy).await?;
        out.grants.push((g.id, id));
    }
    for r in &plan.copy_rules {
        let Selector::Path(scope) = &r.selector else {
            continue;
        };
        let mut copy = Rule {
            id: Uuid::nil(),
            name: copy_name(r, mv),
            selector: Selector::Path(moved_scope(scope, mv)),
            ..r.clone()
        };
        if existing_rules.contains(&rule_key(&copy)) {
            continue;
        }
        let base = copy.name.clone();
        let mut n = 1;
        let id = loop {
            let taken: bool =
                sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM perm.rules WHERE name = $1)")
                    .bind(&copy.name)
                    .fetch_one(&mut **tx)
                    .await?;
            if !taken {
                // A savepoint: a concurrent writer taking the name between
                // the check and the insert retries with the next suffix.
                let mut sp = sqlx::Connection::begin(&mut **tx).await?;
                match egret_perm::create_rule_in_tx(&mut sp, &copy).await {
                    Ok(id) => {
                        sp.commit().await?;
                        break id;
                    }
                    Err(egret_perm::PermError::Db(e))
                        if e.as_database_error()
                            .and_then(|d| d.code())
                            .is_some_and(|c| c == "23505") =>
                    {
                        sp.rollback().await?;
                    }
                    Err(e) => return Err(e),
                }
            }
            n += 1;
            copy.name = format!("{base} #{n}");
        };
        out.rules.push((r.id, id));
    }
    Ok(out)
}

/// Undo a copy (compensation): delete exactly the rows it inserted.
///
/// # Errors
/// The store's error.
pub async fn delete_copies_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    copies: &Copies,
) -> Result<(), egret_perm::PermError> {
    for (_, id) in &copies.grants {
        egret_perm::delete_grant_in_tx(tx, *id).await?;
    }
    for (_, id) in &copies.rules {
        egret_perm::delete_rule_in_tx(tx, *id).await?;
    }
    Ok(())
}

/// Re-point the allow grants `ids` to the destination in place, keeping each
/// id (D2), after re-reading it under the caller's locks: a row that no
/// longer follows the move is left alone. Returns `(before, after)` of each
/// moved grant.
///
/// # Errors
/// The store's error.
pub async fn move_allows_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ids: &[Uuid],
    mv: &Move,
) -> Result<Vec<(Grant, Grant)>, egret_perm::PermError> {
    let mut out = Vec::new();
    for &id in ids {
        let Some(before) = egret_perm::get_grant_via(tx, id).await? else {
            continue;
        };
        if before.effect != Effect::Allow
            || before.origin != GrantOrigin::Explicit
            || before.scope.catalog_id != mv.catalog_id
            || !follows(&before.scope, mv)
        {
            continue;
        }
        let after = Grant {
            scope: moved_scope(&before.scope, mv),
            ..before.clone()
        };
        egret_perm::update_grant_in_tx(tx, id, &after).await?;
        out.push((before, after));
    }
    Ok(out)
}

// ── the refusal (spec §8) ──────────────────────────────────────────────────

/// How many refused rows a message lists.
const LISTED: usize = 10;

/// The 409's text for one move's refusals (spec §8), without the
/// "egret refused the table change: " prefix. `None` when nothing refuses.
pub fn refusal_message(
    refused: &[Refused],
    handwritten: &[Handwritten],
    mv: &Move,
) -> Option<String> {
    let (from, to) = (mv.from.label(), mv.to.label());
    let mut parts = Vec::new();
    let patterns: Vec<&Refused> = refused.iter().filter(|r| !r.multi_level).collect();
    if !patterns.is_empty() {
        let n = patterns.len();
        let rows: Vec<String> = patterns
            .iter()
            .take(LISTED)
            .map(|r| {
                if r.missing.is_empty() {
                    r.row.describe()
                } else {
                    format!("{} — {}", r.row.describe(), r.missing.join("; "))
                }
            })
            .collect();
        let more = if n > LISTED {
            format!("; and {} more", n - LISTED)
        } else {
            String::new()
        };
        let (count, verb) = if n == 1 {
            ("1 restriction covers".to_string(), "it")
        } else {
            (format!("{n} restrictions cover"), "these restrictions")
        };
        parts.push(format!(
            "{count} {from} through a name pattern and would not cover {to}: {}{more}. egret does \
             not rewrite patterns: add {verb} for the new name, change the patterns, or choose a \
             name they cover",
            rows.join("; ")
        ));
    }
    let multi: Vec<&Refused> = refused.iter().filter(|r| r.multi_level).collect();
    if !multi.is_empty() {
        let rows: Vec<String> = multi
            .iter()
            .take(LISTED)
            .map(|r| r.row.describe())
            .collect();
        parts.push(format!(
            "{from} carries restrictions that cannot follow it into {to}, whose namespace has more \
             than one level, which no grant or rule can name: {}; use a single-level schema",
            rows.join("; ")
        ));
    }
    if !handwritten.is_empty() {
        let names: Vec<String> = handwritten.iter().map(Handwritten::label).collect();
        parts.push(format!(
            "hand-written Ranger policies name {from} with a deny, mask, row filter or exclusion \
             list: {}; egret does not rewrite hand-written policies: edit or migrate them first",
            names.join(", ")
        ));
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!("{}. Nothing was renamed.", parts.join(". ")))
}

// ── event summaries (D9) ───────────────────────────────────────────────────

/// A grant's governance summary (`{catalog, schema, table, column, effect,
/// actions, include, exclude}`) with its `id`, as the feed records grants.
///
/// # Errors
/// The database's error, as text.
pub async fn grant_summary_via(
    conn: &mut sqlx::PgConnection,
    g: &Grant,
) -> Result<serde_json::Value, String> {
    let mut v = crate::governance_events::grant_summary(conn, g)
        .await
        .map_err(|e| e.to_string())?;
    v["id"] = serde_json::json!(g.id);
    Ok(v)
}

/// A rule's governance summary (`{name, kind, selector, function, …}`) with
/// its `id`.
///
/// # Errors
/// The database's error, as text.
pub async fn rule_summary_via(
    conn: &mut sqlx::PgConnection,
    r: &Rule,
) -> Result<serde_json::Value, String> {
    let mut v = crate::governance_events::rule_summary(conn, r)
        .await
        .map_err(|e| e.to_string())?;
    v["id"] = serde_json::json!(r.id);
    Ok(v)
}
