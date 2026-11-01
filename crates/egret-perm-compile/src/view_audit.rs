//! View governance B5: what the DEFINER audit decides from a view's recorded
//! lineage. The audit itself is the control-plane route
//! `GET /classifications/{id}/view-exposure` (`trinoViews`).
//!
//! A DEFINER view runs as its owner, so two behaviours surprise operators even
//! with B1's tags in place:
//! - an uncleared owner blocks every reader, cleared readers included (`vd_u`);
//! - base masks and row filters apply as the owner, not the reader (`wd_c`).
//!
//! So a view is listed when its lineage reaches a table of the classification
//! ([`tables_carry_tag`]), or a table a mask or row-filter rule selects
//! ([`tables_have_rule`]). For a DEFINER view, [`owner_cleared`] says whether
//! the owner is cleared for every classification the lineage reaches.
//!
//! Every function takes the lineage as recorded
//! ([`crate::lineage::RecordedLineage::Computed`]); a missing or failed
//! lineage is the caller's to report, never "reads nothing".

use std::collections::{BTreeMap, BTreeSet};

use egret_perm::{CompileError, Selector};
use uuid::Uuid;

use crate::lineage::LineageTable;

/// Split `tables` into the three parallel arrays the queries unnest.
fn columns(tables: &BTreeSet<LineageTable>) -> (Vec<&str>, Vec<&str>, Vec<&str>) {
    let mut c = Vec::new();
    let mut s = Vec::new();
    let mut t = Vec::new();
    for (catalog, schema, table) in tables {
        c.push(catalog.as_str());
        s.push(schema.as_str());
        t.push(table.as_str());
    }
    (c, s, t)
}

/// Whether any of `tables` carries `tag_id` on a column or on the table
/// itself. Matched as [`crate::lineage::view_tag_set`] matches: the catalog
/// by registry name, the schema and table case-folded. No clearance gate is
/// required: the page is the classification's, gated or not.
pub async fn tables_carry_tag(
    conn: &mut sqlx::PgConnection,
    tables: &BTreeSet<LineageTable>,
    tag_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let (c, s, t) = columns(tables);
    sqlx::query_scalar(
        "SELECT EXISTS ( \
           SELECT 1 \
             FROM unnest($1::text[], $2::text[], $3::text[]) AS l(catalog, schema_name, table_name) \
             JOIN registry.catalogs rc ON rc.name = l.catalog \
             JOIN catalog_tag_assignments a \
               ON a.warehouse_id = rc.warehouse_id \
              AND lower(a.schema_name) = lower(l.schema_name) \
              AND lower(a.table_name) = lower(l.table_name) \
            WHERE a.tag_id = $4)",
    )
    .bind(&c)
    .bind(&s)
    .bind(&t)
    .bind(tag_id)
    .fetch_one(&mut *conn)
    .await
}

/// Every mask and row-filter rule (`perm.rules`), read once per report and
/// matched against each view's lineage by [`tables_have_rule`].
#[derive(Debug, Clone, Default)]
pub struct RuleIndex {
    /// Path rules as `(registry catalog name, schema, table)`; `None` is an
    /// absent level. A rule whose catalog is gone is dropped.
    paths: Vec<(String, Option<String>, Option<String>)>,
    /// The `tags.key` of every tag rule.
    tag_keys: Vec<String>,
}

/// Read every rule, and the names of the catalogs path rules name, in two
/// queries.
pub async fn load_rule_index(conn: &mut sqlx::PgConnection) -> Result<RuleIndex, CompileError> {
    let rules = egret_perm::store::list_rules_via(conn)
        .await
        .map_err(CompileError::from_perm)?;
    let ids: Vec<Uuid> = rules
        .iter()
        .filter_map(|r| match &r.selector {
            Selector::Path(scope) => Some(scope.catalog_id),
            Selector::Tag { .. } => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let names: BTreeMap<Uuid, String> = if ids.is_empty() {
        BTreeMap::new()
    } else {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, name FROM registry.catalogs WHERE id = ANY($1)",
        )
        .bind(&ids)
        .fetch_all(&mut *conn)
        .await
        .map_err(CompileError::from_db)?
        .into_iter()
        .collect()
    };
    let mut index = RuleIndex::default();
    for rule in rules {
        match rule.selector {
            Selector::Path(scope) => {
                if let Some(catalog) = names.get(&scope.catalog_id) {
                    index
                        .paths
                        .push((catalog.clone(), scope.schema, scope.table));
                }
            }
            Selector::Tag { key } => index.tag_keys.push(key),
        }
    }
    Ok(index)
}

/// Whether a mask or row-filter rule (`perm.rules`, as `index` read them)
/// selects any of `tables`.
///
/// - **A path rule** selects a table when its catalog is the table's and its
///   schema and table levels are absent or match by
///   [`egret_perm::eval::resource_level_matches`] — the engine's
///   `ignoreCase: true` fold, so `NGƯỜI.Bảng` selects `người.bảng` (see
///   [`path_rule_selects`]). A column-level rule selects its table: the mask
///   applies to a column the view may read.
/// - **A tag rule** selects a table that carries the tag on a column or on
///   the table.
///
/// Policies written by hand in a `manual` profile are not `perm.rules` rows
/// and are not seen here.
pub async fn tables_have_rule(
    conn: &mut sqlx::PgConnection,
    index: &RuleIndex,
    tables: &BTreeSet<LineageTable>,
) -> Result<bool, CompileError> {
    if path_rule_selects(index, tables) {
        return Ok(true);
    }
    let tag_keys: Vec<&str> = index.tag_keys.iter().map(String::as_str).collect();
    if tag_keys.is_empty() {
        return Ok(false);
    }
    let (c, s, t) = columns(tables);
    sqlx::query_scalar(
        "SELECT EXISTS ( \
           SELECT 1 \
             FROM unnest($1::text[], $2::text[], $3::text[]) AS l(catalog, schema_name, table_name) \
             JOIN registry.catalogs rc ON rc.name = l.catalog \
             JOIN catalog_tag_assignments a \
               ON a.warehouse_id = rc.warehouse_id \
              AND lower(a.schema_name) = lower(l.schema_name) \
              AND lower(a.table_name) = lower(l.table_name) \
             JOIN tags tg ON tg.id = a.tag_id \
            WHERE tg.key = ANY($4))",
    )
    .bind(&c)
    .bind(&s)
    .bind(&t)
    .bind(&tag_keys)
    .fetch_one(&mut *conn)
    .await
    .map_err(CompileError::from_db)
}

/// The path half of [`tables_have_rule`], pure so it can be tested without a
/// database: whether any path rule in `index` selects any of `tables`.
///
/// The schema and table levels go through
/// [`egret_perm::eval::resource_level_matches`], the same fold the model
/// oracle and the engine apply. It replaced a `to_ascii_lowercase` fold here
/// (design R9), which left `Ư`/`Ờ` unfolded, so a rule on `NGƯỜI.Bảng` missed
/// a view reading `người.bảng` and the audit failed open in reporting: the
/// view went unlisted. The catalog compares exactly: both sides are the
/// registry name, not a user-typed one.
fn path_rule_selects(index: &RuleIndex, tables: &BTreeSet<LineageTable>) -> bool {
    let level = |rule: &Option<String>, value: &str| match rule {
        None => true,
        Some(p) => egret_perm::eval::resource_level_matches(p, value),
    };
    index.paths.iter().any(|(catalog, schema, table)| {
        tables
            .iter()
            .any(|(c, s, t)| c == catalog && level(schema, s) && level(table, t))
    })
}

/// Whether `owner` (a `users.username`, as Trino names the DEFINER) is cleared
/// for every gated classification in `tags`, the clearance-bearing tag set of
/// the view's lineage ([`crate::lineage::view_tag_set`]).
///
/// Cleared for one gate means holding at least one of its `required_roles`,
/// read through [`crate::members::role_usernames`], the same membership the
/// clearance compiles into its deny exceptions — with
/// [`MissingUsername::Omit`](crate::members::MissingUsername::Omit), because
/// this is served on GET routes and must not write (2026-10-18 design R5). A
/// holder with no username yet is left out, which is exact here: `owner` is a
/// username, so such a holder can never be it. An empty `tags` is
/// `Some(true)`: nothing reached needs a clearance.
///
/// `None` when a reached gate names only `required_groups`: no engine egret
/// provisions can see a group, so the clearance compiles to nothing there,
/// and whether the owner holds it cannot be told from roles.
pub async fn owner_cleared(
    conn: &mut sqlx::PgConnection,
    owner: &str,
    tags: &BTreeSet<Uuid>,
) -> Result<Option<bool>, CompileError> {
    if tags.is_empty() {
        return Ok(Some(true));
    }
    let ids: Vec<Uuid> = tags.iter().copied().collect();
    let gates: Vec<(Uuid, Vec<String>)> = sqlx::query_as(
        "SELECT tag_id, required_roles FROM classification_policies \
          WHERE tag_id = ANY($1) \
            AND (cardinality(required_roles) > 0 OR cardinality(required_groups) > 0)",
    )
    .bind(&ids)
    .fetch_all(&mut *conn)
    .await
    .map_err(CompileError::from_db)?;
    if gates.iter().any(|(_, roles)| roles.is_empty()) {
        return Ok(None);
    }
    let wanted: Vec<String> = gates
        .iter()
        .flat_map(|(_, roles)| roles.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let members =
        crate::members::role_usernames(conn, &wanted, crate::members::MissingUsername::Omit)
            .await?;
    Ok(Some(gates.iter().all(|(_, roles)| {
        roles.iter().any(|r| {
            members
                .get(r)
                .is_some_and(|names| names.iter().any(|n| n == owner))
        })
    })))
}

#[cfg(test)]
mod path_rule_selects_tests {
    use super::*;

    fn index(schema: &str, table: &str) -> RuleIndex {
        RuleIndex {
            paths: vec![("lake".into(), Some(schema.into()), Some(table.into()))],
            tag_keys: vec![],
        }
    }

    fn tables(schema: &str, table: &str) -> BTreeSet<LineageTable> {
        BTreeSet::from([("lake".into(), schema.into(), table.into())])
    }

    #[test]
    fn a_rule_selects_a_non_ascii_table_named_in_another_case() {
        // `to_ascii_lowercase` leaves `Ư`, `Ờ` and `Ả` alone and misses this.
        assert!(path_rule_selects(
            &index("NGƯỜI", "Bảng"),
            &tables("người", "bảng")
        ));
    }

    #[test]
    fn a_rule_does_not_select_a_different_table_or_catalog() {
        assert!(!path_rule_selects(
            &index("người", "bảng"),
            &tables("người", "bàn")
        ));
        let other_catalog = BTreeSet::from([("other".into(), "người".into(), "bảng".into())]);
        assert!(!path_rule_selects(&index("người", "bảng"), &other_catalog));
    }
}
