//! The Ranger request(s) the Trino 483 ranger plugin makes for one OPA question.
//!
//! Derived from `egret-perm-eval/tests/fixtures/trino-483-ranger-checks.txt`,
//! the plugin's own source: where the fixture and the table in the plan
//! disagree, the fixture wins.
#![allow(dead_code)] // the differential (Task 18) is the other consumer

use egret_perm_eval::trino_ops::{classify, DataPrivilege, OpClass};
use egret_perm_eval::Resource;

const RANGER_CHECKS: &str =
    include_str!("../../../egret-perm-eval/tests/fixtures/trino-483-ranger-checks.txt");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangerAsk {
    /// "select", "_any", "impersonate", …
    pub access_type: String,
    /// Ordered (key, value), Trino service-def keys.
    pub resource: Vec<(String, String)>,
    /// `Some("SELF_OR_DESCENDANTS")` for `_any`.
    pub matching_scope: Option<&'static str>,
}

fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn ask(access_type: &str, resource: Vec<(String, String)>) -> RangerAsk {
    RangerAsk {
        access_type: access_type.to_string(),
        resource,
        matching_scope: None,
    }
}

/// The levels a resource names, in service-def order (`function_key` is
/// `schemafunction` for functions, `procedure` for procedures).
fn levels(resource: &Resource, function_key: &str) -> Vec<(String, String)> {
    match resource {
        Resource::None => vec![],
        Resource::Catalog(c) => kv(&[("catalog", c)]),
        Resource::Schema { catalog, schema } => kv(&[("catalog", catalog), ("schema", schema)]),
        Resource::Table {
            catalog,
            schema,
            table,
            ..
        } => kv(&[("catalog", catalog), ("schema", schema), ("table", table)]),
        Resource::Column {
            catalog,
            schema,
            table,
            column,
            ..
        } => kv(&[
            ("catalog", catalog),
            ("schema", schema),
            ("table", table),
            ("column", column),
        ]),
        Resource::User(u) => kv(&[("trinouser", u)]),
        Resource::SystemProperty(p) => kv(&[("systemproperty", p)]),
        Resource::CatalogSessionProperty { catalog, property } => {
            kv(&[("catalog", catalog), ("sessionproperty", property)])
        }
        Resource::Function {
            catalog,
            schema,
            function,
        } => kv(&[
            ("catalog", catalog),
            ("schema", schema),
            (function_key, function),
        ]),
    }
}

/// Every column of a table as its own four-level resource; a column resource
/// as itself. `None` for any other shape.
fn per_column(resource: &Resource) -> Option<Vec<Vec<(String, String)>>> {
    match resource {
        Resource::Table {
            catalog,
            schema,
            table,
            columns,
        } => Some(
            columns
                .iter()
                .map(|c| {
                    kv(&[
                        ("catalog", catalog),
                        ("schema", schema),
                        ("table", table),
                        ("column", c),
                    ])
                })
                .collect(),
        ),
        Resource::Column { .. } => Some(vec![levels(resource, "")]),
        _ => None,
    }
}

/// The table-level resource of a table or column resource.
fn table_levels(resource: &Resource) -> Option<Vec<(String, String)>> {
    match resource {
        Resource::Table { .. } | Resource::Column { .. } => {
            let mut l = levels(resource, "");
            l.truncate(3);
            Some(l)
        }
        _ => None,
    }
}

/// The method of the plugin that decides `name`.
pub fn method_for(name: &str) -> String {
    match name {
        "KillQueryOwnedBy" | "ViewQueryOwnedBy" | "ExecuteQuery" => format!("checkCan{name}"),
        "FilterViewQueryOwnedBy" => "filterViewQueryOwnedBy".into(),
        "ExecuteFunction" => "canExecuteFunction".into(),
        "CreateViewWithExecuteFunction" => "canCreateViewWithExecuteFunction".into(),
        "AccessCatalog" => "canAccessCatalog".into(),
        _ => {
            if let Some(rest) = name.strip_prefix("Filter") {
                format!("filter{rest}")
            } else if let Some(rest) = name.strip_prefix("Get") {
                format!("get{rest}")
            } else {
                format!("checkCan{name}")
            }
        }
    }
}

/// The first access type the fixture names for `name`, lowercased.
fn fixture_access_type(name: &str) -> Option<String> {
    let method = method_for(name);
    let line = RANGER_CHECKS
        .lines()
        .find(|l| l.starts_with(&format!("{method}:")))?;
    line.split_once(':')?
        .1
        .split_whitespace()
        .next()
        .map(str::to_ascii_lowercase)
}

/// All asks must allow for the question to allow (SelectFromColumns: one per
/// column). `None` for an operation the 483 plugin never sends, an operation
/// the fixture lacks, or a resource of the wrong shape for the operation.
///
/// `GetRowFilters` / `GetColumnMask` are `select` asks too; the caller asks
/// them as policy type 2 (row filter) and 1 (mask), which `RangerAsk` does not
/// carry because the operation name already says which.
pub fn ranger_asks(operation: &str, resource: &Resource) -> Option<Vec<RangerAsk>> {
    let class = classify(operation)?;
    // `Filter*` items are `_any` asks, except the impersonation-class one.
    if operation.starts_with("Filter") && !matches!(class, OpClass::Impersonation) {
        return reach_asks(resource, "schemafunction");
    }
    match class {
        OpClass::Data(DataPrivilege::Read) => Some(
            per_column(resource)?
                .into_iter()
                .map(|r| ask("select", r))
                .collect(),
        ),
        OpClass::Data(DataPrivilege::Write) => {
            let access = match operation {
                "InsertIntoTable" | "UpdateTableColumns" => "insert",
                "DeleteFromTable" | "TruncateTable" => "delete",
                _ => return None,
            };
            Some(vec![ask(access, table_levels(resource)?)])
        }
        OpClass::Reach => reach_asks(resource, "schemafunction"),
        OpClass::Impersonation => match resource {
            Resource::User(_) => Some(vec![ask("impersonate", levels(resource, ""))]),
            _ => None,
        },
        OpClass::ExecuteQuery => Some(vec![ask("execute", kv(&[("queryid", "*")]))]),
        OpClass::SystemSessionProperty => match resource {
            Resource::SystemProperty(_) => Some(vec![ask("alter", levels(resource, ""))]),
            _ => None,
        },
        OpClass::EngineCapability(_) => match (operation, resource) {
            ("SetCatalogSessionProperty", Resource::CatalogSessionProperty { .. }) => {
                Some(vec![ask("alter", levels(resource, ""))])
            }
            ("ExecuteProcedure", Resource::Function { .. }) => {
                Some(vec![ask("execute", levels(resource, "procedure"))])
            }
            ("ExecuteFunction" | "CreateViewWithExecuteFunction", Resource::Function { .. }) => {
                Some(vec![ask("execute", levels(resource, "schemafunction"))])
            }
            _ => None,
        },
        OpClass::AdminOnly => {
            let access = fixture_access_type(operation)?;
            Some(vec![ask(&access, levels(resource, "schemafunction"))])
        }
        OpClass::ValueQuestion => match operation {
            "GetRowFilters" => Some(vec![ask("select", table_levels(resource)?)]),
            "GetColumnMask" => match resource {
                Resource::Column { .. } => Some(vec![ask("select", levels(resource, ""))]),
                _ => None,
            },
            _ => None,
        },
    }
}

/// `_any` on the object's levels, matched against the object or anything below
/// it; a table with columns is one ask per column (the plugin's `filterColumns`).
fn reach_asks(resource: &Resource, function_key: &str) -> Option<Vec<RangerAsk>> {
    let any = |r: Vec<(String, String)>| RangerAsk {
        access_type: "_any".into(),
        resource: r,
        matching_scope: Some("SELF_OR_DESCENDANTS"),
    };
    match resource {
        Resource::Catalog(_)
        | Resource::Schema { .. }
        | Resource::Column { .. }
        | Resource::Function { .. } => Some(vec![any(levels(resource, function_key))]),
        Resource::Table { columns, .. } if columns.is_empty() => {
            Some(vec![any(levels(resource, function_key))])
        }
        Resource::Table { .. } => Some(per_column(resource)?.into_iter().map(any).collect()),
        _ => None,
    }
}

/// A resource of the right shape for an operation of this class.
pub fn sample_resource(class: OpClass) -> Resource {
    let s = |v: &str| v.to_string();
    match class {
        OpClass::Data(DataPrivilege::Read) => Resource::Table {
            catalog: s("lake"),
            schema: s("s"),
            table: s("t"),
            columns: vec![s("a"), s("b")],
        },
        OpClass::Data(DataPrivilege::Write) | OpClass::Reach | OpClass::AdminOnly => {
            Resource::Table {
                catalog: s("lake"),
                schema: s("s"),
                table: s("t"),
                columns: vec![],
            }
        }
        OpClass::EngineCapability("trino.set_catalog_session_property") => {
            Resource::CatalogSessionProperty {
                catalog: s("lake"),
                property: s("p"),
            }
        }
        OpClass::EngineCapability(_) => Resource::Function {
            catalog: s("lake"),
            schema: s("s"),
            function: s("f"),
        },
        OpClass::Impersonation => Resource::User(s("alice")),
        OpClass::ExecuteQuery => Resource::None,
        OpClass::SystemSessionProperty => Resource::SystemProperty(s("query_max_memory")),
        OpClass::ValueQuestion => Resource::Column {
            catalog: s("lake"),
            schema: s("s"),
            table: s("t"),
            column: s("a"),
            column_type: s("varchar"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operation_in_the_fixture_has_a_translation_whose_access_type_the_fixture_names() {
        let checks =
            include_str!("../../../egret-perm-eval/tests/fixtures/trino-483-ranger-checks.txt");
        for row in egret_perm_eval::trino_ops::TRINO_483_OPERATIONS {
            let r = sample_resource(row.class);
            let asks =
                ranger_asks(row.name, &r).unwrap_or_else(|| panic!("{} untranslated", row.name));
            // The three `public boolean` methods are not in the fixture's
            // `checkCan|filter|get` set; assert their access type directly.
            let direct = match row.name {
                "ExecuteFunction" | "CreateViewWithExecuteFunction" => Some("execute"),
                "AccessCatalog" => Some("_any"),
                _ => None,
            };
            if let Some(expected) = direct {
                assert!(!asks.is_empty(), "{} has no ask", row.name);
                for a in &asks {
                    assert_eq!(a.access_type, expected, "{}", row.name);
                }
                continue;
            }
            let method = method_for(row.name);
            let line = checks
                .lines()
                .find(|l| l.starts_with(&format!("{method}:")))
                .unwrap_or_else(|| panic!("{method} not in fixture"));
            assert!(!asks.is_empty(), "{} has no ask", row.name);
            for a in &asks {
                assert!(
                    line.to_ascii_lowercase()
                        .contains(&a.access_type.to_ascii_lowercase()),
                    "{} asks {} but the plugin's {line}",
                    row.name,
                    a.access_type
                );
            }
        }
    }

    #[test]
    fn a_select_of_two_columns_is_two_asks() {
        let r = egret_perm_eval::Resource::Table {
            catalog: "lake".into(),
            schema: "s".into(),
            table: "t".into(),
            columns: vec!["a".into(), "b".into()],
        };
        let asks = ranger_asks("SelectFromColumns", &r).unwrap();
        assert_eq!(asks.len(), 2);
        assert_eq!(
            asks[1].resource,
            vec![
                ("catalog".into(), "lake".into()),
                ("schema".into(), "s".into()),
                ("table".into(), "t".into()),
                ("column".into(), "b".into())
            ]
        );
    }

    #[test]
    fn a_reach_ask_matches_the_object_or_anything_below_it() {
        let r = egret_perm_eval::Resource::Schema {
            catalog: "lake".into(),
            schema: "s".into(),
        };
        let asks = ranger_asks("ShowTables", &r).unwrap();
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0].access_type, "_any");
        assert_eq!(asks[0].matching_scope, Some("SELF_OR_DESCENDANTS"));
    }

    #[test]
    fn an_operation_the_plugin_never_sends_has_no_translation() {
        assert!(ranger_asks("NoSuchOperation", &egret_perm_eval::Resource::None).is_none());
    }
}
