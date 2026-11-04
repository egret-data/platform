//! The Ranger request(s) the Trino 483 ranger plugin makes for one OPA question.
//!
//! Derived from `egret-perm-eval/tests/fixtures/trino-483-ranger-checks.txt`
//! (extracted from `ranger.jar`, sha256 26dd71b8…), and confirmed against the
//! plugin's own source, `RangerSystemAccessControl.java` at 483, and its
//! bytecode: where the fixture, the source and the plan disagree, the source
//! wins. What the source shows, and the fixture alone does not:
//!
//! - `hasPermission` sends no `resourceMatchingScope`; only
//!   `hasPermissionForFilter` sets `SELF_OR_DESCENDANTS`, and only `filter*`
//!   methods call it. So the `_any` of `ShowSchemas`, `ShowTables`,
//!   `ShowColumns`, `ShowFunctions` and `AccessCatalog` has NO scope.
//! - `checkCanSetSystemSessionProperty` asks `ALTER` on `systemproperty`
//!   and nothing else; `checkCanShowFunctions` asks `_ANY` on the schema and
//!   nothing else. The second token on those two fixture lines
//!   (`ALTER _ANY`, `_ANY EXECUTE`) is the next method's constant leaking
//!   across the extraction (`SetSystemSessionProperty` is followed by
//!   `canAccessCatalog`'s `_ANY`).
//! - `checkCanSelectFromColumns` goes through `RangerTrinoResource.forColumns`,
//!   which for an empty column set returns the TABLE resource: one `select`
//!   ask on `catalog,schema,table`.
//! - `checkCanRead/WriteSystemInformation` ask `READ_SYSINFO` / `WRITE_SYSINFO`
//!   on `sysinfo=*` (`RangerTrinoResource.forSystemInformation`).
//! - `checkCanExecuteQuery` asks `execute` on `queryid=<the real query id>`;
//!   this module uses the wildcard `*`, as OPA's question carries no query id.
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
    /// `Some("SELF_OR_DESCENDANTS")` on the `filter*` checks only.
    pub matching_scope: Option<&'static str>,
}

const SELF_OR_DESCENDANTS: &str = "SELF_OR_DESCENDANTS";

fn kv(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn ask(
    access_type: &str,
    resource: Vec<(String, String)>,
    scope: Option<&'static str>,
) -> RangerAsk {
    RangerAsk {
        access_type: access_type.to_string(),
        resource,
        matching_scope: scope,
    }
}

/// The depth at which the plugin method asks. An operation is only translated
/// for a resource of exactly this depth; any other answers `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Catalog,
    Schema,
    Table,
    /// One ask per column of a table, or the table itself for an empty set
    /// (`forColumns`), or a single column resource.
    Columns,
    Column,
    Function,
    Procedure,
    User,
    SystemProperty,
    CatalogSessionProperty,
    QueryId,
    SysInfo,
}

/// The depth each operation's plugin method names, from the plugin's source
/// (`createTableResource`, `forSchema`, `forCatalog`, `forSchemaFunction`, …).
pub fn shape_of(operation: &str) -> Option<Shape> {
    Some(match operation {
        "AccessCatalog" | "FilterCatalogs" | "ShowSchemas" | "CreateCatalog" | "DropCatalog" => {
            Shape::Catalog
        }
        "FilterSchemas"
        | "ShowFunctions"
        | "ShowTables"
        | "CreateSchema"
        | "DropSchema"
        | "RenameSchema"
        | "SetSchemaAuthorization"
        | "ShowCreateSchema" => Shape::Schema,
        "FilterTables"
        | "ShowColumns"
        | "AddColumn"
        | "AlterColumn"
        | "DropColumn"
        | "RenameColumn"
        | "SetColumnComment"
        | "CreateTable"
        | "DropTable"
        | "RenameTable"
        | "SetTableAuthorization"
        | "SetTableComment"
        | "SetTableProperties"
        | "ShowCreateTable"
        | "ExecuteTableProcedure"
        | "CreateView"
        | "DropView"
        | "RenameView"
        | "SetViewAuthorization"
        | "SetViewComment"
        | "CreateMaterializedView"
        | "DropMaterializedView"
        | "RefreshMaterializedView"
        | "RenameMaterializedView"
        | "SetMaterializedViewProperties"
        | "InsertIntoTable"
        | "UpdateTableColumns"
        | "DeleteFromTable"
        | "TruncateTable"
        | "GetRowFilters" => Shape::Table,
        "SelectFromColumns" | "CreateViewWithSelectFromColumns" | "FilterColumns" => Shape::Columns,
        "GetColumnMask" => Shape::Column,
        "CreateFunction"
        | "DropFunction"
        | "ShowCreateFunction"
        | "FilterFunctions"
        | "ExecuteFunction"
        | "CreateViewWithExecuteFunction" => Shape::Function,
        "ExecuteProcedure" => Shape::Procedure,
        "ImpersonateUser" | "KillQueryOwnedBy" | "ViewQueryOwnedBy" | "FilterViewQueryOwnedBy" => {
            Shape::User
        }
        "SetSystemSessionProperty" => Shape::SystemProperty,
        "SetCatalogSessionProperty" => Shape::CatalogSessionProperty,
        "ExecuteQuery" => Shape::QueryId,
        "ReadSystemInformation" | "WriteSystemInformation" => Shape::SysInfo,
        _ => return None,
    })
}

fn table_of(r: &Resource) -> Option<Vec<(String, String)>> {
    match r {
        Resource::Table {
            catalog,
            schema,
            table,
            ..
        } => Some(kv(&[
            ("catalog", catalog),
            ("schema", schema),
            ("table", table),
        ])),
        _ => None,
    }
}

fn column_of(r: &Resource) -> Option<Vec<(String, String)>> {
    match r {
        Resource::Column {
            catalog,
            schema,
            table,
            column,
            ..
        } => Some(kv(&[
            ("catalog", catalog),
            ("schema", schema),
            ("table", table),
            ("column", column),
        ])),
        _ => None,
    }
}

/// The resource(s) of one question, at exactly the depth `shape` names.
fn resources_for(
    operation: &str,
    shape: Shape,
    resource: &Resource,
) -> Option<Vec<Vec<(String, String)>>> {
    let one = |l: Option<Vec<(String, String)>>| l.map(|l| vec![l]);
    match (shape, resource) {
        (Shape::Catalog, Resource::Catalog(c)) => one(Some(kv(&[("catalog", c)]))),
        (Shape::Schema, Resource::Schema { catalog, schema }) => {
            one(Some(kv(&[("catalog", catalog), ("schema", schema)])))
        }
        (Shape::Table, r @ Resource::Table { .. }) => one(table_of(r)),
        (Shape::Column, r @ Resource::Column { .. }) => one(column_of(r)),
        (Shape::Columns, r @ Resource::Column { .. }) => one(column_of(r)),
        (
            Shape::Columns,
            Resource::Table {
                catalog,
                schema,
                table,
                columns,
            },
        ) => {
            if !columns.is_empty() {
                Some(
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
                )
            } else if operation == "FilterColumns" {
                // `filterColumns` over no columns asks nothing, which would be
                // a vacuous allow; refuse to translate it.
                None
            } else {
                // `forColumns` of an empty set is the table resource.
                one(table_of(resource))
            }
        }
        (
            Shape::Function,
            Resource::Function {
                catalog,
                schema,
                function,
            },
        ) => one(Some(kv(&[
            ("catalog", catalog),
            ("schema", schema),
            ("schemafunction", function),
        ]))),
        (
            Shape::Procedure,
            Resource::Function {
                catalog,
                schema,
                function,
            },
        ) => one(Some(kv(&[
            ("catalog", catalog),
            ("schema", schema),
            ("procedure", function),
        ]))),
        (Shape::User, Resource::User(u)) => one(Some(kv(&[("trinouser", u)]))),
        (Shape::SystemProperty, Resource::SystemProperty(p)) => {
            one(Some(kv(&[("systemproperty", p)])))
        }
        (Shape::CatalogSessionProperty, Resource::CatalogSessionProperty { catalog, property }) => {
            one(Some(kv(&[
                ("catalog", catalog),
                ("sessionproperty", property),
            ])))
        }
        (Shape::QueryId, Resource::None) => one(Some(kv(&[("queryid", "*")]))),
        (Shape::SysInfo, Resource::None) => one(Some(kv(&[("sysinfo", "*")]))),
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

/// The access types the fixture names for `method`, lowercased, as whole tokens.
pub fn fixture_access_types(method: &str) -> Option<Vec<String>> {
    let line = RANGER_CHECKS
        .lines()
        .find(|l| l.starts_with(&format!("{method}:")))?;
    Some(
        line.split_once(':')?
            .1
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect(),
    )
}

fn access_of(operation: &str, class: OpClass) -> Option<String> {
    let a = match class {
        OpClass::Data(DataPrivilege::Read) | OpClass::ValueQuestion => "select",
        OpClass::Data(DataPrivilege::Write) => match operation {
            "InsertIntoTable" | "UpdateTableColumns" => "insert",
            "DeleteFromTable" | "TruncateTable" => "delete",
            _ => return None,
        },
        OpClass::Reach => "_any",
        OpClass::Impersonation => "impersonate",
        OpClass::ExecuteQuery => "execute",
        OpClass::SystemSessionProperty => "alter",
        OpClass::EngineCapability(_) => match operation {
            "SetCatalogSessionProperty" => "alter",
            "FilterFunctions" => "_any",
            "ExecuteProcedure" | "ExecuteFunction" | "CreateViewWithExecuteFunction" => "execute",
            _ => return None,
        },
        // The first access token of the plugin's line (these lines carry one).
        OpClass::AdminOnly => {
            return fixture_access_types(&method_for(operation))?
                .into_iter()
                .next()
        }
    };
    Some(a.to_string())
}

/// All asks must allow for the question to allow (SelectFromColumns: one per
/// column). `None` for an operation the 483 plugin never sends, and for a
/// resource whose depth is not the one the operation's plugin method asks at.
///
/// `GetRowFilters` / `GetColumnMask` are `select` asks too; the caller asks
/// them as policy type 2 (row filter) and 1 (mask), which `RangerAsk` does not
/// carry because the operation name already says which.
///
/// The result is never empty: a question that would ask the plugin nothing
/// answers `None`, never a vacuous "all allow".
pub fn ranger_asks(operation: &str, resource: &Resource) -> Option<Vec<RangerAsk>> {
    let class = classify(operation)?;
    let shape = shape_of(operation)?;
    let access = access_of(operation, class)?;
    // `hasPermissionForFilter` is only called from the `filter*` methods.
    let scope = operation
        .starts_with("Filter")
        .then_some(SELF_OR_DESCENDANTS);
    let asks: Vec<RangerAsk> = resources_for(operation, shape, resource)?
        .into_iter()
        .map(|r| ask(&access, r, scope))
        .collect();
    (!asks.is_empty()).then_some(asks)
}

/// A resource of the depth the operation asks at.
pub fn sample_resource(operation: &str) -> Resource {
    let s = |v: &str| v.to_string();
    match shape_of(operation).unwrap_or_else(|| panic!("{operation} has no shape")) {
        Shape::Catalog => Resource::Catalog(s("lake")),
        Shape::Schema => Resource::Schema {
            catalog: s("lake"),
            schema: s("s"),
        },
        Shape::Table => Resource::Table {
            catalog: s("lake"),
            schema: s("s"),
            table: s("t"),
            columns: vec![],
        },
        Shape::Columns => Resource::Table {
            catalog: s("lake"),
            schema: s("s"),
            table: s("t"),
            columns: vec![s("a"), s("b")],
        },
        Shape::Column => Resource::Column {
            catalog: s("lake"),
            schema: s("s"),
            table: s("t"),
            column: s("a"),
            column_type: s("varchar"),
        },
        Shape::Function | Shape::Procedure => Resource::Function {
            catalog: s("lake"),
            schema: s("s"),
            function: s("f"),
        },
        Shape::User => Resource::User(s("alice")),
        Shape::SystemProperty => Resource::SystemProperty(s("query_max_memory")),
        Shape::CatalogSessionProperty => Resource::CatalogSessionProperty {
            catalog: s("lake"),
            property: s("p"),
        },
        Shape::QueryId | Shape::SysInfo => Resource::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egret_perm_eval::trino_ops::TRINO_483_OPERATIONS;
    use std::collections::BTreeSet;

    fn exp(access: &str, res: &[(&str, &str)], scope: Option<&'static str>) -> RangerAsk {
        RangerAsk {
            access_type: access.into(),
            resource: kv(res),
            matching_scope: scope,
        }
    }

    const LAKE: [(&str, &str); 1] = [("catalog", "lake")];
    const SCHEMA: [(&str, &str); 2] = [("catalog", "lake"), ("schema", "s")];
    const TABLE: [(&str, &str); 3] = [("catalog", "lake"), ("schema", "s"), ("table", "t")];
    const FUNC: [(&str, &str); 3] = [
        ("catalog", "lake"),
        ("schema", "s"),
        ("schemafunction", "f"),
    ];
    const SOD: Option<&str> = Some("SELF_OR_DESCENDANTS");

    /// The access types the plugin really checks, where the fixture line
    /// carries a spurious extra token (see the module docs): method -> token.
    const FIXTURE_ARTEFACTS: &[(&str, &str)] = &[
        ("checkCanSetSystemSessionProperty", "_any"),
        ("checkCanShowFunctions", "execute"),
    ];

    /// The 3 `public boolean` methods outside the fixture's `checkCan|filter|get` set.
    const DIRECT: &[(&str, &str)] = &[
        ("ExecuteFunction", "execute"),
        ("CreateViewWithExecuteFunction", "execute"),
        ("AccessCatalog", "_any"),
    ];

    #[test]
    fn each_ask_uses_an_access_type_the_plugin_checks_and_together_they_cover_all_of_them() {
        for row in TRINO_483_OPERATIONS {
            let r = sample_resource(row.name);
            let asks =
                ranger_asks(row.name, &r).unwrap_or_else(|| panic!("{} untranslated", row.name));
            let used: BTreeSet<String> = asks.iter().map(|a| a.access_type.clone()).collect();
            let method = method_for(row.name);
            let checked: BTreeSet<String> = match DIRECT.iter().find(|(n, _)| *n == row.name) {
                Some((_, a)) => BTreeSet::from([(*a).to_string()]),
                None => {
                    let mut set: BTreeSet<String> = fixture_access_types(&method)
                        .unwrap_or_else(|| panic!("{method} not in fixture"))
                        .into_iter()
                        .collect();
                    for (m, t) in FIXTURE_ARTEFACTS {
                        if *m == method {
                            assert!(set.remove(*t), "{m} no longer carries {t}");
                        }
                    }
                    set
                }
            };
            // Whole tokens, never substrings, never the method name.
            for a in &asks {
                assert!(
                    checked.contains(&a.access_type),
                    "{} asks {} but the plugin checks {checked:?}",
                    row.name,
                    a.access_type
                );
            }
            assert_eq!(
                used, checked,
                "{}: asks must cover what the plugin checks",
                row.name
            );
        }
    }

    #[test]
    fn one_expected_ask_set_per_class_pins_keys_order_and_scope() {
        let cases: Vec<(&str, Resource, Vec<RangerAsk>)> = vec![
            (
                "SelectFromColumns",
                sample_resource("SelectFromColumns"),
                vec![
                    exp(
                        "select",
                        &[
                            ("catalog", "lake"),
                            ("schema", "s"),
                            ("table", "t"),
                            ("column", "a"),
                        ],
                        None,
                    ),
                    exp(
                        "select",
                        &[
                            ("catalog", "lake"),
                            ("schema", "s"),
                            ("table", "t"),
                            ("column", "b"),
                        ],
                        None,
                    ),
                ],
            ),
            (
                "CreateViewWithSelectFromColumns",
                sample_resource("SelectFromColumns"),
                vec![
                    exp(
                        "select",
                        &[
                            ("catalog", "lake"),
                            ("schema", "s"),
                            ("table", "t"),
                            ("column", "a"),
                        ],
                        None,
                    ),
                    exp(
                        "select",
                        &[
                            ("catalog", "lake"),
                            ("schema", "s"),
                            ("table", "t"),
                            ("column", "b"),
                        ],
                        None,
                    ),
                ],
            ),
            (
                "InsertIntoTable",
                sample_resource("InsertIntoTable"),
                vec![exp("insert", &TABLE, None)],
            ),
            (
                "UpdateTableColumns",
                sample_resource("InsertIntoTable"),
                vec![exp("insert", &TABLE, None)],
            ),
            (
                "DeleteFromTable",
                sample_resource("DeleteFromTable"),
                vec![exp("delete", &TABLE, None)],
            ),
            (
                "TruncateTable",
                sample_resource("DeleteFromTable"),
                vec![exp("delete", &TABLE, None)],
            ),
            // check* `_any`: no scope (hasPermission, not hasPermissionForFilter)
            (
                "AccessCatalog",
                sample_resource("AccessCatalog"),
                vec![exp("_any", &LAKE, None)],
            ),
            (
                "ShowSchemas",
                sample_resource("ShowSchemas"),
                vec![exp("_any", &LAKE, None)],
            ),
            (
                "ShowTables",
                sample_resource("ShowTables"),
                vec![exp("_any", &SCHEMA, None)],
            ),
            (
                "ShowFunctions",
                sample_resource("ShowFunctions"),
                vec![exp("_any", &SCHEMA, None)],
            ),
            (
                "ShowColumns",
                sample_resource("ShowColumns"),
                vec![exp("_any", &TABLE, None)],
            ),
            // filter* `_any`: SELF_OR_DESCENDANTS
            (
                "FilterCatalogs",
                sample_resource("FilterCatalogs"),
                vec![exp("_any", &LAKE, SOD)],
            ),
            (
                "FilterSchemas",
                sample_resource("FilterSchemas"),
                vec![exp("_any", &SCHEMA, SOD)],
            ),
            (
                "FilterTables",
                sample_resource("FilterTables"),
                vec![exp("_any", &TABLE, SOD)],
            ),
            (
                "FilterFunctions",
                sample_resource("FilterFunctions"),
                vec![exp("_any", &FUNC, SOD)],
            ),
            (
                "FilterColumns",
                sample_resource("FilterColumns"),
                vec![
                    exp(
                        "_any",
                        &[
                            ("catalog", "lake"),
                            ("schema", "s"),
                            ("table", "t"),
                            ("column", "a"),
                        ],
                        SOD,
                    ),
                    exp(
                        "_any",
                        &[
                            ("catalog", "lake"),
                            ("schema", "s"),
                            ("table", "t"),
                            ("column", "b"),
                        ],
                        SOD,
                    ),
                ],
            ),
            (
                "ImpersonateUser",
                sample_resource("ImpersonateUser"),
                vec![exp("impersonate", &[("trinouser", "alice")], None)],
            ),
            (
                "KillQueryOwnedBy",
                sample_resource("ImpersonateUser"),
                vec![exp("impersonate", &[("trinouser", "alice")], None)],
            ),
            (
                "ViewQueryOwnedBy",
                sample_resource("ImpersonateUser"),
                vec![exp("impersonate", &[("trinouser", "alice")], None)],
            ),
            (
                "FilterViewQueryOwnedBy",
                sample_resource("ImpersonateUser"),
                vec![exp("impersonate", &[("trinouser", "alice")], SOD)],
            ),
            (
                "ExecuteQuery",
                Resource::None,
                vec![exp("execute", &[("queryid", "*")], None)],
            ),
            (
                "SetSystemSessionProperty",
                sample_resource("SetSystemSessionProperty"),
                vec![exp(
                    "alter",
                    &[("systemproperty", "query_max_memory")],
                    None,
                )],
            ),
            (
                "SetCatalogSessionProperty",
                sample_resource("SetCatalogSessionProperty"),
                vec![exp(
                    "alter",
                    &[("catalog", "lake"), ("sessionproperty", "p")],
                    None,
                )],
            ),
            (
                "ExecuteProcedure",
                sample_resource("ExecuteProcedure"),
                vec![exp(
                    "execute",
                    &[("catalog", "lake"), ("schema", "s"), ("procedure", "f")],
                    None,
                )],
            ),
            (
                "ExecuteFunction",
                sample_resource("ExecuteFunction"),
                vec![exp("execute", &FUNC, None)],
            ),
            (
                "CreateViewWithExecuteFunction",
                sample_resource("ExecuteFunction"),
                vec![exp("execute", &FUNC, None)],
            ),
            (
                "ReadSystemInformation",
                Resource::None,
                vec![exp("read_sysinfo", &[("sysinfo", "*")], None)],
            ),
            (
                "WriteSystemInformation",
                Resource::None,
                vec![exp("write_sysinfo", &[("sysinfo", "*")], None)],
            ),
            (
                "GetRowFilters",
                sample_resource("GetRowFilters"),
                vec![exp("select", &TABLE, None)],
            ),
            (
                "GetColumnMask",
                sample_resource("GetColumnMask"),
                vec![exp(
                    "select",
                    &[
                        ("catalog", "lake"),
                        ("schema", "s"),
                        ("table", "t"),
                        ("column", "a"),
                    ],
                    None,
                )],
            ),
        ];
        for (op, r, want) in cases {
            assert_eq!(ranger_asks(op, &r), Some(want), "{op}");
        }
    }

    /// Independent of the fixture: (operation, access type, depth).
    const ADMIN_EXPECTED: &[(&str, &str, &str)] = &[
        ("CreateCatalog", "create", "catalog"),
        ("DropCatalog", "drop", "catalog"),
        ("CreateSchema", "create", "schema"),
        ("DropSchema", "drop", "schema"),
        ("RenameSchema", "alter", "schema"),
        ("SetSchemaAuthorization", "alter", "schema"),
        ("ShowCreateSchema", "show", "schema"),
        ("CreateTable", "create", "table"),
        ("DropTable", "drop", "table"),
        ("RenameTable", "alter", "table"),
        ("SetTableAuthorization", "alter", "table"),
        ("SetTableComment", "alter", "table"),
        ("SetTableProperties", "alter", "table"),
        ("ShowCreateTable", "show", "table"),
        ("AddColumn", "alter", "table"),
        ("AlterColumn", "alter", "table"),
        ("DropColumn", "alter", "table"),
        ("RenameColumn", "alter", "table"),
        ("SetColumnComment", "alter", "table"),
        ("ExecuteTableProcedure", "alter", "table"),
        ("CreateView", "create", "table"),
        ("DropView", "drop", "table"),
        ("RenameView", "alter", "table"),
        ("SetViewAuthorization", "alter", "table"),
        ("SetViewComment", "alter", "table"),
        ("CreateMaterializedView", "create", "table"),
        ("DropMaterializedView", "drop", "table"),
        ("RefreshMaterializedView", "alter", "table"),
        ("RenameMaterializedView", "alter", "table"),
        ("SetMaterializedViewProperties", "alter", "table"),
        ("CreateFunction", "create", "function"),
        ("DropFunction", "drop", "function"),
        ("ShowCreateFunction", "show", "function"),
        ("ReadSystemInformation", "read_sysinfo", "sysinfo"),
        ("WriteSystemInformation", "write_sysinfo", "sysinfo"),
    ];

    #[test]
    fn every_admin_only_operation_matches_an_independent_expected_value() {
        let admin: BTreeSet<&str> = TRINO_483_OPERATIONS
            .iter()
            .filter(|r| matches!(r.class, OpClass::AdminOnly))
            .map(|r| r.name)
            .collect();
        let listed: BTreeSet<&str> = ADMIN_EXPECTED.iter().map(|(n, _, _)| *n).collect();
        assert_eq!(
            admin, listed,
            "the independent table must list every AdminOnly op"
        );
        for (op, access, depth) in ADMIN_EXPECTED {
            let levels: &[(&str, &str)] = match *depth {
                "catalog" => &LAKE,
                "schema" => &SCHEMA,
                "table" => &TABLE,
                "function" => &FUNC,
                "sysinfo" => &[("sysinfo", "*")],
                d => panic!("{d}"),
            };
            assert_eq!(
                ranger_asks(op, &sample_resource(op)),
                Some(vec![exp(access, levels, None)]),
                "{op}"
            );
        }
    }

    #[test]
    fn a_select_of_two_columns_is_two_asks() {
        let r = Resource::Table {
            catalog: "lake".into(),
            schema: "s".into(),
            table: "t".into(),
            columns: vec!["a".into(), "b".into()],
        };
        let asks = ranger_asks("SelectFromColumns", &r).unwrap();
        assert_eq!(asks.len(), 2);
        assert_eq!(
            asks[1].resource,
            kv(&[
                ("catalog", "lake"),
                ("schema", "s"),
                ("table", "t"),
                ("column", "b")
            ])
        );
    }

    /// M2. The plugin's `forColumns` turns an empty column set into the table
    /// resource, so the plugin asks `select` on the table, never nothing.
    #[test]
    fn a_select_of_no_columns_is_one_select_on_the_table_never_zero_asks() {
        let r = Resource::Table {
            catalog: "lake".into(),
            schema: "s".into(),
            table: "t".into(),
            columns: vec![],
        };
        for op in ["SelectFromColumns", "CreateViewWithSelectFromColumns"] {
            assert_eq!(
                ranger_asks(op, &r),
                Some(vec![exp("select", &TABLE, None)]),
                "{op}"
            );
        }
        // `filterColumns` over no columns asks nothing: refuse, not a vacuous allow.
        assert_eq!(ranger_asks("FilterColumns", &r), None);
    }

    #[test]
    fn no_translation_is_ever_an_empty_ask_list() {
        let pool = shape_pool();
        for row in TRINO_483_OPERATIONS {
            for r in pool
                .iter()
                .chain(std::iter::once(&sample_resource(row.name)))
            {
                if let Some(asks) = ranger_asks(row.name, r) {
                    assert!(!asks.is_empty(), "{} on {r:?}", row.name);
                }
            }
        }
    }

    /// M1. Only `filterColumns` splits per column; a table-level `_any` with
    /// columns on the resource stays one ask.
    #[test]
    fn a_table_with_columns_is_one_ask_unless_the_operation_is_per_column() {
        let r = sample_resource("SelectFromColumns"); // a, b
        for op in [
            "ShowColumns",
            "FilterTables",
            "InsertIntoTable",
            "AddColumn",
            "GetRowFilters",
        ] {
            let asks = ranger_asks(op, &r).unwrap();
            assert_eq!(asks.len(), 1, "{op}");
            assert_eq!(asks[0].resource, kv(&TABLE), "{op}");
        }
        assert_eq!(ranger_asks("FilterColumns", &r).unwrap().len(), 2);
    }

    fn shape_pool() -> Vec<Resource> {
        let s = |v: &str| v.to_string();
        vec![
            Resource::None,
            Resource::Catalog(s("lake")),
            Resource::Schema {
                catalog: s("lake"),
                schema: s("s"),
            },
            Resource::Table {
                catalog: s("lake"),
                schema: s("s"),
                table: s("t"),
                columns: vec![],
            },
            Resource::Column {
                catalog: s("lake"),
                schema: s("s"),
                table: s("t"),
                column: s("a"),
                column_type: s("varchar"),
            },
            Resource::User(s("alice")),
            Resource::SystemProperty(s("p")),
            Resource::CatalogSessionProperty {
                catalog: s("lake"),
                property: s("p"),
            },
            Resource::Function {
                catalog: s("lake"),
                schema: s("s"),
                function: s("f"),
            },
        ]
    }

    #[test]
    fn a_resource_of_the_wrong_depth_answers_none() {
        // The named cases of the review.
        let pool = shape_pool();
        let (catalog, schema, table, column) = (&pool[1], &pool[2], &pool[3], &pool[4]);
        assert_eq!(ranger_asks("CreateSchema", table), None);
        assert_eq!(ranger_asks("CreateSchema", catalog), None);
        assert_eq!(ranger_asks("AddColumn", column), None);
        assert_eq!(ranger_asks("AddColumn", schema), None);
        assert_eq!(ranger_asks("ShowTables", table), None);
        assert_eq!(ranger_asks("CreateCatalog", schema), None);
        assert_eq!(ranger_asks("ReadSystemInformation", table), None);
        assert_eq!(ranger_asks("ExecuteQuery", table), None);
        // Every operation accepts exactly the variant its shape names.
        for row in TRINO_483_OPERATIONS {
            let own = sample_resource(row.name);
            let per_column = matches!(shape_of(row.name), Some(Shape::Columns));
            for r in &pool {
                let same = std::mem::discriminant(r) == std::mem::discriminant(&own);
                let also_ok =
                    per_column && matches!(r, Resource::Column { .. } | Resource::Table { .. });
                if !same && !also_ok {
                    assert_eq!(
                        ranger_asks(row.name, r),
                        None,
                        "{} accepted {r:?}",
                        row.name
                    );
                }
            }
        }
    }

    #[test]
    fn an_operation_the_plugin_never_sends_has_no_translation() {
        for r in shape_pool() {
            assert!(ranger_asks("NoSuchOperation", &r).is_none(), "{r:?}");
        }
    }
}
