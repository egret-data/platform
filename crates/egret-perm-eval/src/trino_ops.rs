//! The Trino 483 OPA plugin's operation names and how egret answers each.
//!
//! The name list is pinned to the jar by
//! `tests/fixtures/trino-483-opa-operations.txt`; `why` cites the check the
//! Ranger plugin makes for the same operation
//! (`tests/fixtures/trino-483-ranger-checks.txt`).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataPrivilege {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpClass {
    /// Read (select) of each listed column, or Write (insert/delete) of the table.
    Data(DataPrivilege),
    /// Ranger `_any`: some privilege on the object or below it.
    Reach,
    /// Only the all-access infrastructure statement (`InfraObject::WholeEp`) allows it.
    AdminOnly,
    /// An engine capability from the catalogue, by id (`egret-perm-compile/src/engine_capabilities.rs`).
    EngineCapability(&'static str),
    /// Impersonate, view, kill or filter queries of the resource's user.
    Impersonation,
    ExecuteQuery,
    SystemSessionProperty,
    /// Answered on a value route: `GetRowFilters`, `GetColumnMask`.
    ValueQuestion,
}

#[derive(Debug, Clone, Copy)]
pub struct OpRow {
    pub name: &'static str,
    pub class: OpClass,
    pub why: &'static str,
}

/// Every operation the 483 plugin sends, alphabetically.
pub const TRINO_483_OPERATIONS: &[OpRow] = &[
    OpRow {
        name: "AccessCatalog",
        class: OpClass::Reach,
        why: "Ranger: _ANY, resource or descendants",
    },
    OpRow {
        name: "AddColumn",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "AlterColumn",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "CreateCatalog",
        class: OpClass::AdminOnly,
        why: "Ranger: CREATE, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "CreateFunction",
        class: OpClass::AdminOnly,
        why: "Ranger: CREATE, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "CreateMaterializedView",
        class: OpClass::AdminOnly,
        why: "Ranger: CREATE, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "CreateSchema",
        class: OpClass::AdminOnly,
        why: "Ranger: CREATE, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "CreateTable",
        class: OpClass::AdminOnly,
        why: "Ranger: CREATE, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "CreateView",
        class: OpClass::AdminOnly,
        why: "Ranger: CREATE, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "CreateViewWithExecuteFunction",
        class: OpClass::EngineCapability("trino.execute_function"),
        why: "Ranger: EXECUTE on schemafunction",
    },
    OpRow {
        name: "CreateViewWithSelectFromColumns",
        class: OpClass::Data(DataPrivilege::Read),
        why: "Ranger: SELECT on each column",
    },
    OpRow {
        name: "DeleteFromTable",
        class: OpClass::Data(DataPrivilege::Write),
        why: "Ranger: DELETE on the table",
    },
    OpRow {
        name: "DropCatalog",
        class: OpClass::AdminOnly,
        why: "Ranger: DROP, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "DropColumn",
        class: OpClass::AdminOnly,
        why: "Ranger: DROP, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "DropFunction",
        class: OpClass::AdminOnly,
        why: "Ranger: DROP, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "DropMaterializedView",
        class: OpClass::AdminOnly,
        why: "Ranger: DROP, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "DropSchema",
        class: OpClass::AdminOnly,
        why: "Ranger: DROP, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "DropTable",
        class: OpClass::AdminOnly,
        why: "Ranger: DROP, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "DropView",
        class: OpClass::AdminOnly,
        why: "Ranger: DROP, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "ExecuteFunction",
        class: OpClass::EngineCapability("trino.execute_function"),
        why: "Ranger: EXECUTE on schemafunction",
    },
    OpRow {
        name: "ExecuteProcedure",
        class: OpClass::EngineCapability("trino.execute_procedure"),
        why: "Ranger: EXECUTE on procedure",
    },
    OpRow {
        name: "ExecuteQuery",
        class: OpClass::ExecuteQuery,
        why: "Ranger: EXECUTE on queryid",
    },
    OpRow {
        name: "ExecuteTableProcedure",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "FilterCatalogs",
        class: OpClass::Reach,
        why: "Ranger: _ANY, resource or descendants",
    },
    OpRow {
        name: "FilterColumns",
        class: OpClass::Reach,
        why: "Ranger: _ANY, resource or descendants",
    },
    OpRow {
        name: "FilterFunctions",
        class: OpClass::EngineCapability("trino.execute_function"),
        why: "Ranger: _ANY on the function (filterFunctions); egret is deliberately stricter and requires the execute_function capability, which fails closed",
    },
    OpRow {
        name: "FilterSchemas",
        class: OpClass::Reach,
        why: "Ranger: _ANY, resource or descendants",
    },
    OpRow {
        name: "FilterTables",
        class: OpClass::Reach,
        why: "Ranger: _ANY, resource or descendants",
    },
    OpRow {
        name: "FilterViewQueryOwnedBy",
        class: OpClass::Impersonation,
        why: "Ranger: IMPERSONATE on trinouser",
    },
    OpRow {
        name: "GetColumnMask",
        class: OpClass::ValueQuestion,
        why: "Answered on a value route, not by an access check",
    },
    OpRow {
        name: "GetRowFilters",
        class: OpClass::ValueQuestion,
        why: "Answered on a value route, not by an access check",
    },
    OpRow {
        name: "ImpersonateUser",
        class: OpClass::Impersonation,
        why: "Ranger: IMPERSONATE on trinouser",
    },
    OpRow {
        name: "InsertIntoTable",
        class: OpClass::Data(DataPrivilege::Write),
        why: "Ranger: INSERT on the table",
    },
    OpRow {
        name: "KillQueryOwnedBy",
        class: OpClass::Impersonation,
        why: "Ranger: IMPERSONATE on trinouser",
    },
    OpRow {
        name: "ReadSystemInformation",
        class: OpClass::AdminOnly,
        why: "Ranger: READ_SYSINFO, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "RefreshMaterializedView",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "RenameColumn",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "RenameMaterializedView",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "RenameSchema",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "RenameTable",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "RenameView",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "SelectFromColumns",
        class: OpClass::Data(DataPrivilege::Read),
        why: "Ranger: SELECT on each column",
    },
    OpRow {
        name: "SetCatalogSessionProperty",
        class: OpClass::EngineCapability("trino.set_catalog_session_property"),
        why: "Ranger: ALTER on sessionproperty",
    },
    OpRow {
        name: "SetColumnComment",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "SetMaterializedViewProperties",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "SetSchemaAuthorization",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "SetSystemSessionProperty",
        class: OpClass::SystemSessionProperty,
        why: "Ranger: ALTER _ANY (checkCanSetSystemSessionProperty); egret answers it by the SystemSessionProperty class, not by a resource grant",
    },
    OpRow {
        name: "SetTableAuthorization",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "SetTableComment",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "SetTableProperties",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "SetViewAuthorization",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "SetViewComment",
        class: OpClass::AdminOnly,
        why: "Ranger: ALTER, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "ShowColumns",
        class: OpClass::Reach,
        why: "Ranger: _ANY, resource or descendants",
    },
    OpRow {
        name: "ShowCreateFunction",
        class: OpClass::AdminOnly,
        why: "Ranger: SHOW, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "ShowCreateSchema",
        class: OpClass::AdminOnly,
        why: "Ranger: SHOW, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "ShowCreateTable",
        class: OpClass::AdminOnly,
        why: "Ranger: SHOW, which no grant compiles; only the egret-admin all-access holds it",
    },
    OpRow {
        name: "ShowFunctions",
        class: OpClass::Reach,
        why: "Ranger: _ANY, resource or descendants",
    },
    OpRow {
        name: "ShowSchemas",
        class: OpClass::Reach,
        why: "Ranger: _ANY, resource or descendants",
    },
    OpRow {
        name: "ShowTables",
        class: OpClass::Reach,
        why: "Ranger: _ANY, resource or descendants",
    },
    OpRow {
        name: "TruncateTable",
        class: OpClass::Data(DataPrivilege::Write),
        why: "Ranger: DELETE on the table",
    },
    OpRow {
        name: "UpdateTableColumns",
        class: OpClass::Data(DataPrivilege::Write),
        why: "Ranger: INSERT on the table",
    },
    OpRow {
        name: "ViewQueryOwnedBy",
        class: OpClass::Impersonation,
        why: "Ranger: IMPERSONATE on trinouser",
    },
    OpRow {
        name: "WriteSystemInformation",
        class: OpClass::AdminOnly,
        why: "Ranger: WRITE_SYSINFO, which no grant compiles; only the egret-admin all-access holds it",
    },
];

/// The class of an operation name, or `None` when the 483 plugin never sends it.
pub fn classify(name: &str) -> Option<OpClass> {
    TRINO_483_OPERATIONS
        .iter()
        .find(|r| r.name == name)
        .map(|r| r.class)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/trino-483-opa-operations.txt");

    #[test]
    fn the_table_covers_every_operation_the_483_plugin_sends() {
        let sent: std::collections::BTreeSet<&str> =
            FIXTURE.lines().filter(|l| !l.is_empty()).collect();
        let modelled: std::collections::BTreeSet<&str> =
            TRINO_483_OPERATIONS.iter().map(|r| r.name).collect();
        let missing: Vec<_> = sent.difference(&modelled).collect();
        let extra: Vec<_> = modelled.difference(&sent).collect();
        assert!(
            missing.is_empty(),
            "sent by the 483 plugin, not modelled: {missing:?}"
        );
        assert!(
            extra.is_empty(),
            "modelled, never sent by the 483 plugin: {extra:?}"
        );
        assert_eq!(sent.len(), 63);
    }

    #[test]
    fn each_name_appears_once_and_says_why() {
        let mut seen = std::collections::BTreeSet::new();
        for r in TRINO_483_OPERATIONS {
            assert!(seen.insert(r.name), "{} twice", r.name);
            assert!(!r.why.trim().is_empty(), "{} has no reason", r.name);
        }
    }

    #[test]
    fn an_unknown_operation_is_not_modelled() {
        assert_eq!(classify("GrantTablePrivilege"), None);
        assert_eq!(
            classify("selectfromcolumns"),
            None,
            "names are case-sensitive"
        );
    }

    #[test]
    fn spot_checks_against_the_ranger_plugin() {
        assert_eq!(
            classify("SelectFromColumns"),
            Some(OpClass::Data(DataPrivilege::Read))
        );
        assert_eq!(
            classify("UpdateTableColumns"),
            Some(OpClass::Data(DataPrivilege::Write))
        );
        assert_eq!(
            classify("TruncateTable"),
            Some(OpClass::Data(DataPrivilege::Write))
        );
        assert_eq!(classify("FilterColumns"), Some(OpClass::Reach));
        assert_eq!(classify("CreateTable"), Some(OpClass::AdminOnly));
        assert_eq!(classify("KillQueryOwnedBy"), Some(OpClass::Impersonation));
        assert_eq!(
            classify("ExecuteProcedure"),
            Some(OpClass::EngineCapability("trino.execute_procedure"))
        );
        assert_eq!(classify("GetColumnMask"), Some(OpClass::ValueQuestion));
    }
}
