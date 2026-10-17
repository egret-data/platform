//! DG frontend redesign #1 (Governance Data): the shape both engine listers
//! answer in, and the two rules they share — which schemas are hidden and how
//! an engine's table type maps to a kind.
//!
//! The Trino lister lives in `egret-query`, the StarRocks one in
//! `egret-engine-mgmt`, and the route that consumes them in
//! `egret-controlplane`, which depends on neither; `egret-bin` maps a
//! [`CatalogLevel`] onto the route's own types. This leaf crate is the one place
//! both listers can reach, so the rules are spelled once.
//!
//! Measured on Trino 483 and StarRocks 4.1.6
//! (`docs/en/plans/2026-10-16-dg-frontend-data-live.md` §1.1, §1.4).

use crate::Engine;

/// The kind of a table-like object, as an engine reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListedKind {
    Table,
    View,
    MaterializedView,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedRelation {
    pub name: String,
    pub kind: ListedKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedColumn {
    pub name: String,
    /// The type as the engine spells it.
    pub r#type: String,
}

/// One level of a catalog, as an engine listed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogLevel {
    /// The catalog's schemas, sorted, without [`is_hidden_schema`] ones.
    Schemas(Vec<String>),
    /// A schema's tables, views and materialized views, sorted by name.
    Relations(Vec<ListedRelation>),
    /// A table's or a view's columns, in the engine's order.
    Columns(Vec<ListedColumn>),
}

/// Whether `schema` is left out of a catalog's schema listing on `engine`:
/// `information_schema` on both engines, and on Trino also `system`, the
/// schema Trino's Iceberg connector adds (measured J8). Connector schemas such
/// as Postgres's `pg_catalog` are kept: they are the source's own.
///
/// The lister does not know the catalog's connector, so on Trino a source
/// schema really named `system` is hidden too.
pub fn is_hidden_schema(engine: Engine, schema: &str) -> bool {
    match schema {
        "information_schema" => true,
        "system" => engine == Engine::Trino,
        _ => false,
    }
}

/// The kind of a relation out of the type an engine reports for it: Trino's
/// `information_schema.tables.table_type`, StarRocks's `SHOW FULL TABLES`
/// `Table_type`. An unknown type is an error rather than a guess.
///
/// Measured: `BASE TABLE` and `VIEW` only. A remote view or materialized view
/// on a JDBC catalog is reported as `BASE TABLE` by both engines, so it lists
/// as a table. A materialized-view type is honoured only when an engine names
/// it explicitly (unmeasured: neither engine was seen to).
pub fn relation_kind(table_type: &str) -> Result<ListedKind, String> {
    match table_type.to_ascii_uppercase().as_str() {
        "BASE TABLE" => Ok(ListedKind::Table),
        "VIEW" | "SYSTEM VIEW" => Ok(ListedKind::View),
        "MATERIALIZED VIEW" | "MATERIALIZED_VIEW" => Ok(ListedKind::MaterializedView),
        _ => Err(format!(
            "the engine reported an unknown table type {table_type:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn information_schema_is_hidden_on_both_engines() {
        for engine in [Engine::Trino, Engine::StarRocks] {
            assert!(is_hidden_schema(engine, "information_schema"), "{engine:?}");
        }
    }

    #[test]
    fn system_is_hidden_on_trino_only() {
        assert!(is_hidden_schema(Engine::Trino, "system"));
        assert!(!is_hidden_schema(Engine::StarRocks, "system"));
    }

    #[test]
    fn connector_and_user_schemas_are_kept() {
        for engine in [Engine::Trino, Engine::StarRocks] {
            for s in [
                "pg_catalog",
                "public",
                "sales",
                "Information_Schema",
                "information_schema2",
            ] {
                assert!(!is_hidden_schema(engine, s), "{engine:?} {s}");
            }
        }
    }

    #[test]
    fn the_measured_types_map_to_table_and_view() {
        assert_eq!(relation_kind("BASE TABLE"), Ok(ListedKind::Table));
        assert_eq!(relation_kind("VIEW"), Ok(ListedKind::View));
    }

    #[test]
    fn an_explicit_materialized_view_type_is_honoured() {
        assert_eq!(
            relation_kind("MATERIALIZED VIEW"),
            Ok(ListedKind::MaterializedView)
        );
        assert_eq!(
            relation_kind("MATERIALIZED_VIEW"),
            Ok(ListedKind::MaterializedView)
        );
    }

    #[test]
    fn a_system_view_is_a_view() {
        assert_eq!(relation_kind("SYSTEM VIEW"), Ok(ListedKind::View));
    }

    #[test]
    fn the_type_is_read_case_insensitively() {
        assert_eq!(relation_kind("base table"), Ok(ListedKind::Table));
        assert_eq!(relation_kind("View"), Ok(ListedKind::View));
    }

    #[test]
    fn an_unknown_type_is_an_error_naming_it() {
        for t in ["", "TABLE", "EXTERNAL TABLE"] {
            let err = relation_kind(t).expect_err(t);
            assert!(err.contains(&format!("{t:?}")), "{err}");
        }
    }
}
