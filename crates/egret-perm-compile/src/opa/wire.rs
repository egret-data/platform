//! The Trino 483 OPA plugin's wire format: the request body egret receives
//! and the result bodies it answers with.
//!
//! The shapes are pinned to the jar by
//! `crates/egret-perm-eval/tests/fixtures/trino-483-opa-request-shapes.txt`.
//!
//! Parsing fails closed: a missing or mistyped required field, or a resource
//! this module does not recognise, is an `Err`, never a default. Fields this
//! module does not read (`softwareStack`, `identity.groups`, a table's
//! `properties`, `targetResource`, `grantee`, ...) are ignored, because the
//! plugin adds them per operation. `identity.groups` in particular is never
//! read (S7): a user's groups come from egret's own membership, not from what
//! the engine claims.
//!
//! No `Err` text ever contains a value from `context.properties`, so the
//! `egret.secret` cannot leak through an error. Errors name field paths and
//! keys only.

use egret_perm_eval::Resource;
use serde_json::{json, Map, Value};

pub const ENGINE_PROPERTY: &str = "egret.engine";
pub const SECRET_PROPERTY: &str = "egret.secret";

/// The batch operation whose indices address one table's columns.
pub const FILTER_COLUMNS: &str = "FilterColumns";
/// The operation the plugin sends to `/row_filters`.
pub const GET_ROW_FILTERS: &str = "GetRowFilters";
/// The operation the plugin sends to `/column_mask` and `/batch_column_masks`.
pub const GET_COLUMN_MASK: &str = "GetColumnMask";

/// The `egret.secret` property. Its `Debug` prints `<redacted>`, so `{:?}`,
/// `tracing`'s `?req`, a panicking `unwrap` or a failed `assert_eq` on an
/// [`OpaRequest`] never shows the value. Read it only through [`Secret::expose`]
/// at the one place that verifies it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: String) -> Self {
        Secret(value)
    }

    /// The raw value, for the credential check only. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaRequest {
    pub user: String,
    pub engine: Option<String>,
    pub secret: Option<Secret>,
    pub operation: String,
    pub resource: Resource,
    pub filter_resources: Vec<Resource>,
}

/// The table a value route answers for. Every name is non-empty and not `*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueTable {
    pub catalog: String,
    pub schema: String,
    pub table: String,
}

/// The columns a mask route answers for, in request order: `(name, type)`.
/// Index `i` of a mask result refers to `columns[i]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskTargets {
    pub table: ValueTable,
    pub columns: Vec<(String, String)>,
}

/// Parse `{"input": {"context": .., "action": ..}}`.
pub fn parse_request(body: &Value) -> Result<OpaRequest, String> {
    let input = object(body.get("input"), "input")?;
    let context = object(input.get("context"), "input.context")?;
    let identity = object(context.get("identity"), "input.context.identity")?;
    let user = non_empty_string(identity.get("user"), "input.context.identity.user")?;

    let (engine, secret) = match context.get("properties") {
        None | Some(Value::Null) => (None, None),
        Some(Value::Object(p)) => (
            optional_string(
                p.get(ENGINE_PROPERTY),
                "input.context.properties.egret.engine",
            )?,
            optional_string(
                p.get(SECRET_PROPERTY),
                "input.context.properties.egret.secret",
            )?
            .map(Secret::new),
        ),
        Some(_) => return Err("input.context.properties is not an object".into()),
    };

    let action = object(input.get("action"), "input.action")?;
    let operation = non_empty_string(action.get("operation"), "input.action.operation")?;
    let resource = match action.get("resource") {
        None | Some(Value::Null) => Resource::None,
        Some(r) => parse_resource(r, "input.action.resource")?,
    };
    let filter_resources = match action.get("filterResources") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, r)| parse_resource(r, &format!("input.action.filterResources[{i}]")))
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("input.action.filterResources is not an array".into()),
    };

    Ok(OpaRequest {
        user,
        engine,
        secret,
        operation,
        resource,
        filter_resources,
    })
}

/// FilterColumns arrives as ONE table resource with `columns`; expand to one Table per column.
/// Err when `filterResources` holds anything but exactly one table: the index semantics would be ambiguous.
pub fn expand_filter_columns(req: &OpaRequest) -> Result<Vec<Resource>, String> {
    match req.filter_resources.as_slice() {
        [Resource::Table {
            catalog,
            schema,
            table,
            columns,
        }] => Ok(columns
            .iter()
            .map(|c| Resource::Table {
                catalog: catalog.clone(),
                schema: schema.clone(),
                table: table.clone(),
                columns: vec![c.clone()],
            })
            .collect()),
        _ => Err("FilterColumns needs exactly one table in filterResources".into()),
    }
}

/// The length of the list a `/batch` answer's indices address: the expanded
/// columns for FilterColumns, `filter_resources` otherwise. The same `Err` as
/// [`expand_filter_columns`] for a FilterColumns request of another shape.
pub fn batch_len(req: &OpaRequest) -> Result<usize, String> {
    if req.operation == FILTER_COLUMNS {
        expand_filter_columns(req).map(|x| x.len())
    } else {
        Ok(req.filter_resources.len())
    }
}

/// `/row_filters`: the operation must be `GetRowFilters` and the resource one
/// table with real names. Any other shape is an `Err`, which the route answers
/// with 403, never with "no filter".
pub fn row_filter_target(req: &OpaRequest) -> Result<ValueTable, String> {
    expect_operation(req, GET_ROW_FILTERS)?;
    match &req.resource {
        Resource::Table {
            catalog,
            schema,
            table,
            ..
        } => value_table(catalog, schema, table),
        _ => Err("GetRowFilters needs a table resource".into()),
    }
}

/// `/column_mask`: one `column` resource in `resource`.
pub fn column_mask_target(req: &OpaRequest) -> Result<MaskTargets, String> {
    expect_operation(req, GET_COLUMN_MASK)?;
    mask_targets(std::slice::from_ref(&req.resource), "resource")
}

/// `/batch_column_masks`: one `column` resource per column in
/// `filterResources`, all of one table, at least one.
pub fn column_mask_targets(req: &OpaRequest) -> Result<MaskTargets, String> {
    expect_operation(req, GET_COLUMN_MASK)?;
    mask_targets(&req.filter_resources, "filterResources")
}

/// `{"result": bool}`
pub fn allow_result(allowed: bool) -> Value {
    json!({ "result": allowed })
}

/// `{"result": [..]}`. Every index must address one of the request's
/// `request_len` items; one outside it is an `Err` (an evaluator bug the route
/// answers with 500), never an index the plugin would act on.
///
/// `request_len` is the length of the list the indices address. For
/// FilterColumns that is the number of **columns** of its one table (the
/// output of [`expand_filter_columns`]), not `filter_resources.len()` (which
/// is 1). Take it from [`batch_len`] rather than computing it.
pub fn batch_result(indices: &[usize], request_len: usize) -> Result<Value, String> {
    check_indices(indices.iter().copied(), request_len)?;
    Ok(json!({ "result": indices }))
}

/// `{"result": [{"expression": ..}]}`
pub fn row_filters_result(filters: &[String]) -> Value {
    let items: Vec<Value> = filters.iter().map(|e| json!({ "expression": e })).collect();
    json!({ "result": items })
}

/// `{"result": [{"index": i, "viewExpression": {"expression": e}}]}`. Each
/// index must address one of the request's `request_len` columns, and at most
/// once (the plugin builds a map keyed by column and refuses duplicates).
pub fn column_masks_result(masks: &[(usize, String)], request_len: usize) -> Result<Value, String> {
    check_indices(masks.iter().map(|(i, _)| *i), request_len)?;
    let items: Vec<Value> = masks
        .iter()
        .map(|(i, e)| json!({ "index": i, "viewExpression": { "expression": e } }))
        .collect();
    Ok(json!({ "result": items }))
}

/// `{"result": {"expression": e}}` or `{"result": null}`
pub fn column_mask_result(mask: Option<&str>) -> Value {
    match mask {
        Some(e) => json!({ "result": { "expression": e } }),
        None => json!({ "result": null }),
    }
}

fn check_indices(indices: impl Iterator<Item = usize>, request_len: usize) -> Result<(), String> {
    let mut seen = vec![false; request_len];
    for i in indices {
        match seen.get_mut(i) {
            None => {
                return Err(format!(
                    "result index {i} outside a request of {request_len}"
                ))
            }
            Some(true) => return Err(format!("result index {i} repeated")),
            Some(s) => *s = true,
        }
    }
    Ok(())
}

fn expect_operation(req: &OpaRequest, want: &str) -> Result<(), String> {
    if req.operation == want {
        Ok(())
    } else {
        Err(format!("this route answers only {want}"))
    }
}

fn mask_targets(resources: &[Resource], at: &str) -> Result<MaskTargets, String> {
    let mut table: Option<ValueTable> = None;
    let mut columns = Vec::with_capacity(resources.len());
    for r in resources {
        let Resource::Column {
            catalog,
            schema,
            table: t,
            column,
            column_type,
        } = r
        else {
            return Err(format!("GetColumnMask needs column resources in {at}"));
        };
        let this = value_table(catalog, schema, t)?;
        match &table {
            None => table = Some(this),
            Some(first) if *first == this => {}
            Some(_) => return Err(format!("GetColumnMask columns in {at} span several tables")),
        }
        real_name(column, "column")?;
        if column_type.is_empty() {
            return Err("empty column type".into());
        }
        columns.push((column.clone(), column_type.clone()));
    }
    match table {
        Some(table) => Ok(MaskTargets { table, columns }),
        None => Err(format!("GetColumnMask needs at least one column in {at}")),
    }
}

fn value_table(catalog: &str, schema: &str, table: &str) -> Result<ValueTable, String> {
    real_name(catalog, "catalog")?;
    real_name(schema, "schema")?;
    real_name(table, "table")?;
    Ok(ValueTable {
        catalog: catalog.to_string(),
        schema: schema.to_string(),
        table: table.to_string(),
    })
}

/// A value route never answers for "every table" or "every column": an empty
/// or literal `*` name would match only match-any rules and return no
/// filter/mask for a rule naming the object.
fn real_name(v: &str, level: &str) -> Result<(), String> {
    if v.is_empty() || v == "*" {
        Err(format!("empty or `*` {level} name"))
    } else {
        Ok(())
    }
}

fn parse_resource(v: &Value, at: &str) -> Result<Resource, String> {
    let obj = object(Some(v), at)?;
    let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    let [key] = keys.as_slice() else {
        let shown: Vec<String> = keys
            .iter()
            .take(MAX_ECHOED_KEYS)
            .map(|k| echo_key(k))
            .collect();
        let more = keys.len().saturating_sub(MAX_ECHOED_KEYS);
        let tail = if more > 0 {
            format!(",+{more} more")
        } else {
            String::new()
        };
        return Err(format!("unrecognised resource: {}{tail}", shown.join(",")));
    };
    // Only the eight known keys reach the format below, so `inner_at` never
    // carries a caller-chosen key.
    let inner_at = format!("{at}.{key}");
    let body = || object(obj.get(*key), &inner_at);
    let field = |o: &Map<String, Value>, name: &str| -> Result<String, String> {
        string(o.get(name), &format!("{inner_at}.{name}"))
    };
    Ok(match *key {
        "user" => Resource::User(field(body()?, "user")?),
        "systemSessionProperty" => Resource::SystemProperty(field(body()?, "name")?),
        "catalogSessionProperty" => {
            let o = body()?;
            Resource::CatalogSessionProperty {
                catalog: field(o, "catalogName")?,
                property: field(o, "propertyName")?,
            }
        }
        "function" => {
            let o = body()?;
            Resource::Function {
                catalog: field(o, "catalogName")?,
                schema: field(o, "schemaName")?,
                function: field(o, "functionName")?,
            }
        }
        "catalog" => Resource::Catalog(field(body()?, "name")?),
        "schema" => {
            let o = body()?;
            Resource::Schema {
                catalog: field(o, "catalogName")?,
                schema: field(o, "schemaName")?,
            }
        }
        "table" => {
            let o = body()?;
            let columns = match o.get("columns") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(cs)) => cs
                    .iter()
                    .enumerate()
                    .map(|(i, c)| string(Some(c), &format!("{inner_at}.columns[{i}]")))
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(format!("{inner_at}.columns is not an array")),
            };
            Resource::Table {
                catalog: field(o, "catalogName")?,
                schema: field(o, "schemaName")?,
                table: field(o, "tableName")?,
                columns,
            }
        }
        "column" => {
            let o = body()?;
            Resource::Column {
                catalog: field(o, "catalogName")?,
                schema: field(o, "schemaName")?,
                table: field(o, "tableName")?,
                column: field(o, "columnName")?,
                column_type: field(o, "columnType")?,
            }
        }
        other => return Err(format!("unrecognised resource: {}", echo_key(other))),
    })
}

/// How much of a caller-chosen key an error may echo.
const MAX_ECHOED_KEY_CHARS: usize = 64;
const MAX_ECHOED_KEYS: usize = 8;

/// A caller-chosen key for an error text: at most 64 chars, with control
/// characters (and quotes, backslashes) escaped by `{:?}`, so a key can
/// neither flood a log line nor forge a new one.
fn echo_key(k: &str) -> String {
    let cut: String = k.chars().take(MAX_ECHOED_KEY_CHARS).collect();
    if cut.len() < k.len() {
        format!("{cut:?}…")
    } else {
        format!("{cut:?}")
    }
}

fn object<'a>(v: Option<&'a Value>, at: &str) -> Result<&'a Map<String, Value>, String> {
    match v {
        Some(Value::Object(o)) => Ok(o),
        None | Some(Value::Null) => Err(format!("{at} is missing")),
        Some(_) => Err(format!("{at} is not an object")),
    }
}

fn string(v: Option<&Value>, at: &str) -> Result<String, String> {
    match v {
        Some(Value::String(s)) => Ok(s.clone()),
        None | Some(Value::Null) => Err(format!("{at} is missing")),
        Some(_) => Err(format!("{at} is not a string")),
    }
}

fn non_empty_string(v: Option<&Value>, at: &str) -> Result<String, String> {
    let s = string(v, at)?;
    if s.is_empty() {
        return Err(format!("{at} is empty"));
    }
    Ok(s)
}

fn optional_string(v: Option<&Value>, at: &str) -> Result<Option<String>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("{at} is not a string")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(action: serde_json::Value) -> serde_json::Value {
        json!({"input": {"context": {"identity": {"user": "ann", "groups": ["g"]},
            "softwareStack": {"trinoVersion": "483"},
            "properties": {"egret.engine": "e1", "egret.secret": "s3cr3t"}}, "action": action}})
    }

    fn col(table: &str, name: &str) -> serde_json::Value {
        json!({"column": {"catalogName": "lake", "schemaName": "s", "tableName": table,
            "columnName": name, "columnType": "varchar"}})
    }

    fn tbl(table: &str) -> serde_json::Value {
        json!({"table": {"catalogName": "lake", "schemaName": "s", "tableName": table}})
    }

    fn masks(filter_resources: Vec<serde_json::Value>) -> OpaRequest {
        parse_request(&input(
            json!({"operation": "GetColumnMask", "filterResources": filter_resources}),
        ))
        .unwrap()
    }

    #[test]
    fn a_select_parses_to_a_table_with_columns() {
        let r = parse_request(&input(json!({"operation": "SelectFromColumns",
            "resource": {"table": {"catalogName": "lake", "schemaName": "s", "tableName": "t", "columns": ["a", "b"]}}}))).unwrap();
        assert_eq!(r.user, "ann");
        assert_eq!(r.engine.as_deref(), Some("e1"));
        assert_eq!(
            r.resource,
            Resource::Table {
                catalog: "lake".into(),
                schema: "s".into(),
                table: "t".into(),
                columns: vec!["a".into(), "b".into()]
            }
        );
    }

    #[test]
    fn filter_columns_expands_to_one_resource_per_column() {
        let r = parse_request(&input(json!({"operation": "FilterColumns", "filterResources": [
            {"table": {"catalogName": "lake", "schemaName": "s", "tableName": "t", "columns": ["a", "b"]}}]}))).unwrap();
        let x = expand_filter_columns(&r).unwrap();
        assert_eq!(x.len(), 2);
        assert_eq!(
            x[0],
            Resource::Table {
                catalog: "lake".into(),
                schema: "s".into(),
                table: "t".into(),
                columns: vec!["a".into()]
            }
        );
        assert_eq!(
            x[1],
            Resource::Table {
                catalog: "lake".into(),
                schema: "s".into(),
                table: "t".into(),
                columns: vec!["b".into()]
            }
        );
    }

    #[test]
    fn a_column_mask_request_parses_each_column_with_its_type() {
        let r = parse_request(&input(json!({"operation": "GetColumnMask", "filterResources": [
            {"column": {"catalogName": "lake", "schemaName": "s", "tableName": "t", "columnName": "email", "columnType": "varchar"}}]}))).unwrap();
        assert!(
            matches!(&r.filter_resources[0], Resource::Column { column_type, .. } if column_type == "varchar")
        );
    }

    #[test]
    fn a_malformed_body_is_an_error_that_does_not_echo_the_secret() {
        let e = parse_request(&input(
            json!({"operation": "SelectFromColumns", "resource": {"bogus": {}}}),
        ))
        .unwrap_err();
        assert!(!e.contains("s3cr3t"), "an error echoed the secret");
        assert!(parse_request(&json!({"input": {}})).is_err());
    }

    #[test]
    fn results_have_the_plugins_shapes() {
        assert_eq!(allow_result(false), json!({"result": false}));
        assert_eq!(allow_result(true), json!({"result": true}));
        assert_eq!(batch_result(&[0, 2], 3).unwrap(), json!({"result": [0, 2]}));
        assert_eq!(batch_result(&[], 0).unwrap(), json!({"result": []}));
        assert_eq!(
            row_filters_result(&["(a) AND (b)".into()]),
            json!({"result": [{"expression": "(a) AND (b)"}]})
        );
        assert_eq!(row_filters_result(&[]), json!({"result": []}));
        assert_eq!(
            column_masks_result(&[(1, "NULL".into())], 2).unwrap(),
            json!({"result": [{"index": 1, "viewExpression": {"expression": "NULL"}}]})
        );
        assert_eq!(
            column_mask_result(Some("NULL")),
            json!({"result": {"expression": "NULL"}})
        );
        assert_eq!(column_mask_result(None), json!({"result": null}));
    }

    #[test]
    fn filter_columns_over_two_tables_is_refused_not_guessed() {
        let t = |n: &str| json!({"table": {"catalogName": "lake", "schemaName": "s", "tableName": n, "columns": ["a"]}});
        let r = parse_request(&input(
            json!({"operation": "FilterColumns", "filterResources": [t("t1"), t("t2")]}),
        ))
        .unwrap();
        assert!(expand_filter_columns(&r).is_err());
    }

    #[test]
    fn filter_columns_over_no_table_or_a_column_is_refused() {
        let none = parse_request(&input(json!({"operation": "FilterColumns"}))).unwrap();
        assert!(expand_filter_columns(&none).is_err());
        let column = parse_request(&input(
            json!({"operation": "FilterColumns", "filterResources": [col("t", "a")]}),
        ))
        .unwrap();
        assert!(expand_filter_columns(&column).is_err());
    }

    // ---- identity, properties, operation ----

    #[test]
    fn the_user_comes_from_identity_user_and_groups_are_never_read() {
        let body = json!({"input": {"context": {"identity": {"user": "bob", "groups": "not-even-a-list"}},
            "action": {"operation": "ExecuteQuery"}}});
        let r = parse_request(&body).unwrap();
        assert_eq!(r.user, "bob");
        assert_eq!(r.engine, None);
        assert_eq!(r.secret, None);
        let no_groups = json!({"input": {"context": {"identity": {"user": "bob"}},
            "action": {"operation": "ExecuteQuery"}}});
        assert_eq!(parse_request(&no_groups).unwrap(), r);
    }

    #[test]
    fn a_missing_empty_or_mistyped_user_is_an_error() {
        for identity in [
            json!({}),
            json!({"user": ""}),
            json!({"user": 7}),
            json!({"user": null}),
            json!("ann"),
        ] {
            let body = json!({"input": {"context": {"identity": identity}, "action": {"operation": "ExecuteQuery"}}});
            assert!(parse_request(&body).is_err(), "{identity}");
        }
        let no_identity =
            json!({"input": {"context": {}, "action": {"operation": "ExecuteQuery"}}});
        assert!(parse_request(&no_identity).is_err());
    }

    #[test]
    fn the_secret_and_engine_are_read_from_properties() {
        let r = parse_request(&input(json!({"operation": "ExecuteQuery"}))).unwrap();
        assert!(
            r.secret.as_ref().map(Secret::expose) == Some("s3cr3t"),
            "secret mismatch"
        );
        assert_eq!(r.engine.as_deref(), Some("e1"));
        assert_eq!(r.resource, Resource::None);
        assert!(r.filter_resources.is_empty());
    }

    #[test]
    fn a_mistyped_property_is_an_error_that_does_not_echo_the_secret() {
        for props in [
            json!({"egret.engine": 1, "egret.secret": "s3cr3t"}),
            json!({"egret.engine": "e1", "egret.secret": ["s3cr3t"]}),
            json!("s3cr3t"),
        ] {
            let body = json!({"input": {"context": {"identity": {"user": "ann"}, "properties": props},
                "action": {"operation": "ExecuteQuery"}}});
            let e = parse_request(&body).unwrap_err();
            assert!(!e.contains("s3cr3t"), "an error echoed the secret");
        }
    }

    #[test]
    fn the_operation_is_passed_through_exactly() {
        for op in [
            "SelectFromColumns",
            "selectfromcolumns",
            "NoSuchOperation",
            " ExecuteQuery",
        ] {
            let r = parse_request(&input(json!({"operation": op}))).unwrap();
            assert_eq!(r.operation, op);
        }
    }

    #[test]
    fn a_missing_empty_or_mistyped_operation_is_an_error() {
        for action in [
            json!({}),
            json!({"operation": ""}),
            json!({"operation": 3}),
            json!("ExecuteQuery"),
        ] {
            assert!(parse_request(&input(action.clone())).is_err(), "{action}");
        }
        let no_action = json!({"input": {"context": {"identity": {"user": "ann"}}}});
        assert!(parse_request(&no_action).is_err());
        assert!(parse_request(&json!({})).is_err());
        assert!(parse_request(&json!([])).is_err());
    }

    // ---- resources ----

    #[test]
    fn every_resource_kind_maps_by_its_key() {
        let cases = [
            (json!({"user": {"user": "u"}}), Resource::User("u".into())),
            (
                json!({"systemSessionProperty": {"name": "p"}}),
                Resource::SystemProperty("p".into()),
            ),
            (
                json!({"catalogSessionProperty": {"catalogName": "c", "propertyName": "p"}}),
                Resource::CatalogSessionProperty {
                    catalog: "c".into(),
                    property: "p".into(),
                },
            ),
            (
                json!({"function": {"catalogName": "c", "schemaName": "s", "functionName": "f"}}),
                Resource::Function {
                    catalog: "c".into(),
                    schema: "s".into(),
                    function: "f".into(),
                },
            ),
            (
                json!({"catalog": {"name": "c"}}),
                Resource::Catalog("c".into()),
            ),
            (
                json!({"schema": {"catalogName": "c", "schemaName": "s"}}),
                Resource::Schema {
                    catalog: "c".into(),
                    schema: "s".into(),
                },
            ),
            (
                json!({"table": {"catalogName": "c", "schemaName": "s", "tableName": "t", "properties": {"x": 1}}}),
                Resource::Table {
                    catalog: "c".into(),
                    schema: "s".into(),
                    table: "t".into(),
                    columns: vec![],
                },
            ),
            (
                json!({"column": {"catalogName": "c", "schemaName": "s", "tableName": "t", "columnName": "x", "columnType": "bigint"}}),
                Resource::Column {
                    catalog: "c".into(),
                    schema: "s".into(),
                    table: "t".into(),
                    column: "x".into(),
                    column_type: "bigint".into(),
                },
            ),
        ];
        for (resource, want) in cases {
            let r = parse_request(&input(
                json!({"operation": "X", "resource": resource.clone()}),
            ))
            .unwrap();
            assert_eq!(r.resource, want, "{resource}");
            let r = parse_request(&input(
                json!({"operation": "X", "filterResources": [resource.clone()]}),
            ))
            .unwrap();
            assert_eq!(r.filter_resources, vec![want], "{resource}");
        }
    }

    #[test]
    fn a_resource_missing_or_mistyping_a_field_is_an_error() {
        let bad = [
            json!({"user": {}}),
            json!({"systemSessionProperty": {"name": 1}}),
            json!({"catalogSessionProperty": {"catalogName": "c"}}),
            json!({"function": {"catalogName": "c", "schemaName": "s"}}),
            json!({"catalog": {}}),
            json!({"catalog": "c"}),
            json!({"schema": {"schemaName": "s"}}),
            json!({"table": {"catalogName": "c", "schemaName": "s"}}),
            json!({"table": {"catalogName": "c", "schemaName": "s", "tableName": "t", "columns": "a"}}),
            json!({"table": {"catalogName": "c", "schemaName": "s", "tableName": "t", "columns": [1]}}),
            json!({"column": {"catalogName": "c", "schemaName": "s", "tableName": "t", "columnName": "x"}}),
            json!({"column": {"catalogName": "c", "schemaName": "s", "tableName": "t", "columnType": "x"}}),
        ];
        for resource in bad {
            assert!(
                parse_request(&input(
                    json!({"operation": "X", "resource": resource.clone()})
                ))
                .is_err(),
                "{resource}"
            );
            assert!(
                parse_request(&input(
                    json!({"operation": "X", "filterResources": [resource.clone()]})
                ))
                .is_err(),
                "{resource}"
            );
        }
    }

    #[test]
    fn an_unknown_empty_or_two_key_resource_is_an_error() {
        for resource in [
            json!({"bogus": {}}),
            json!({}),
            json!("table"),
            json!([]),
            json!({"table": {"catalogName": "c", "schemaName": "s", "tableName": "t"}, "catalog": {"name": "c"}}),
        ] {
            let e = parse_request(&input(
                json!({"operation": "X", "resource": resource.clone()}),
            ))
            .unwrap_err();
            assert!(!e.contains("s3cr3t"), "an error echoed the secret");
        }
        let e = parse_request(&input(json!({"operation": "X", "resource": {"bogus": {}}})))
            .unwrap_err();
        assert_eq!(e, "unrecognised resource: \"bogus\"");
    }

    #[test]
    fn filter_resources_must_be_an_array_and_one_bad_item_fails_the_request() {
        assert!(parse_request(&input(
            json!({"operation": "X", "filterResources": {"table": {}}})
        ))
        .is_err());
        assert!(parse_request(&input(
            json!({"operation": "X", "filterResources": [col("t", "a"), {"bogus": {}}]})
        ))
        .is_err());
    }

    // ---- value routes (C1: refusal, never an empty answer) ----

    #[test]
    fn a_row_filter_request_names_its_table() {
        let r = parse_request(&input(
            json!({"operation": "GetRowFilters", "resource": tbl("t")}),
        ))
        .unwrap();
        assert_eq!(
            row_filter_target(&r).unwrap(),
            ValueTable {
                catalog: "lake".into(),
                schema: "s".into(),
                table: "t".into()
            }
        );
    }

    #[test]
    fn a_row_filter_request_of_the_wrong_shape_is_refused() {
        for action in [
            json!({"operation": "GetRowFilters"}),
            json!({"operation": "GetRowFilters", "resource": {"schema": {"catalogName": "lake", "schemaName": "s"}}}),
            json!({"operation": "GetRowFilters", "resource": col("t", "a")}),
            json!({"operation": "GetRowFilters", "filterResources": [tbl("t")]}),
            json!({"operation": "GetColumnMask", "resource": tbl("t")}),
            json!({"operation": "SelectFromColumns", "resource": tbl("t")}),
        ] {
            let r = parse_request(&input(action.clone())).unwrap();
            assert!(row_filter_target(&r).is_err(), "{action}");
        }
    }

    #[test]
    fn an_empty_or_star_name_on_a_value_route_is_refused() {
        let named = |c: &str, s: &str, t: &str, x: &str| {
            json!({"column": {"catalogName": c, "schemaName": s,
            "tableName": t, "columnName": x, "columnType": "varchar"}})
        };
        for bad in ["", "*"] {
            for (c, s, t) in [(bad, "s", "t"), ("lake", bad, "t"), ("lake", "s", bad)] {
                let table = json!({"table": {"catalogName": c, "schemaName": s, "tableName": t}});
                let r = parse_request(&input(
                    json!({"operation": "GetRowFilters", "resource": table}),
                ))
                .unwrap();
                assert!(row_filter_target(&r).is_err(), "{c}.{s}.{t}");
                let r = masks(vec![named(c, s, t, "a")]);
                assert!(column_mask_targets(&r).is_err(), "{c}.{s}.{t}");
                let r = parse_request(&input(
                    json!({"operation": "GetColumnMask", "resource": named(c, s, t, "a")}),
                ))
                .unwrap();
                assert!(column_mask_target(&r).is_err(), "{c}.{s}.{t}");
            }
            let r = masks(vec![col("t", "a"), col("t", bad)]);
            assert!(column_mask_targets(&r).is_err(), "column {bad:?}");
            let r = parse_request(&input(
                json!({"operation": "GetColumnMask", "resource": col("t", bad)}),
            ))
            .unwrap();
            assert!(column_mask_target(&r).is_err(), "column {bad:?}");
        }
        let untyped = json!({"column": {"catalogName": "lake", "schemaName": "s", "tableName": "t",
            "columnName": "a", "columnType": ""}});
        assert!(column_mask_targets(&masks(vec![untyped])).is_err());
    }

    #[test]
    fn a_batch_mask_request_lists_its_columns_in_order() {
        let t = column_mask_targets(&masks(vec![col("t", "b"), col("t", "a")])).unwrap();
        assert_eq!(
            t.table,
            ValueTable {
                catalog: "lake".into(),
                schema: "s".into(),
                table: "t".into()
            }
        );
        assert_eq!(
            t.columns,
            vec![
                ("b".into(), "varchar".into()),
                ("a".into(), "varchar".into())
            ]
        );
    }

    #[test]
    fn a_batch_mask_request_of_the_wrong_shape_is_refused() {
        // No columns: refused, never `[]` ("no masks").
        assert!(column_mask_targets(&masks(vec![])).is_err());
        assert!(column_mask_targets(&masks(vec![col("t1", "a"), col("t2", "a")])).is_err());
        assert!(column_mask_targets(&masks(vec![col("t", "a"), tbl("t")])).is_err());
        let single = parse_request(&input(
            json!({"operation": "GetColumnMask", "resource": col("t", "a")}),
        ))
        .unwrap();
        assert!(column_mask_targets(&single).is_err());
        let wrong_op = parse_request(&input(
            json!({"operation": "GetRowFilters", "filterResources": [col("t", "a")]}),
        ))
        .unwrap();
        assert!(column_mask_targets(&wrong_op).is_err());
    }

    #[test]
    fn a_single_mask_request_names_one_column() {
        let r = parse_request(&input(
            json!({"operation": "GetColumnMask", "resource": col("t", "email")}),
        ))
        .unwrap();
        let t = column_mask_target(&r).unwrap();
        assert_eq!(t.columns, vec![("email".into(), "varchar".into())]);
        assert_eq!(t.table.table, "t");
    }

    #[test]
    fn a_single_mask_request_of_the_wrong_shape_is_refused() {
        for action in [
            json!({"operation": "GetColumnMask"}),
            json!({"operation": "GetColumnMask", "resource": tbl("t")}),
            json!({"operation": "GetColumnMask", "filterResources": [col("t", "a")]}),
            json!({"operation": "GetRowFilters", "resource": col("t", "a")}),
        ] {
            let r = parse_request(&input(action.clone())).unwrap();
            assert!(column_mask_target(&r).is_err(), "{action}");
        }
    }

    #[test]
    fn a_result_index_outside_the_request_is_an_error() {
        assert!(batch_result(&[3], 3).is_err());
        assert!(batch_result(&[0], 0).is_err());
        assert!(batch_result(&[2], 3).is_ok());
        assert!(batch_result(&[1, 1], 3).is_err());
        assert!(column_masks_result(&[(2, "NULL".into())], 2).is_err());
        assert!(column_masks_result(&[(1, "NULL".into())], 2).is_ok());
        assert!(column_masks_result(&[(0, "NULL".into()), (0, "x".into())], 2).is_err());
    }

    // ---- fix round 1 ----

    #[test]
    fn debug_of_a_request_never_shows_the_secret() {
        let r = parse_request(&input(json!({"operation": "ExecuteQuery"}))).unwrap();
        let shown = format!("{r:?}");
        assert!(!shown.contains("s3cr3t"), "Debug printed the secret");
        assert!(
            shown.contains("<redacted>"),
            "Debug dropped the secret's presence"
        );
        let pretty = format!("{r:#?}");
        assert!(
            !pretty.contains("s3cr3t"),
            "pretty Debug printed the secret"
        );
        assert!(
            r.secret.as_ref().map(Secret::expose) == Some("s3cr3t"),
            "secret mismatch"
        );
    }

    #[test]
    fn an_echoed_key_is_capped_and_escaped() {
        let huge = format!("evil\nINFO forged line{}", "k".repeat(1 << 20));
        let mut one = serde_json::Map::new();
        one.insert(huge.clone(), json!({}));
        let e = parse_request(&input(json!({"operation": "X", "resource": one}))).unwrap_err();
        assert!(e.len() < 200, "echoed {} bytes", e.len());
        assert!(!e.contains('\n'), "a raw newline reached the error");
        assert!(
            e.contains("evil\\nINFO"),
            "the key prefix is not shown escaped"
        );

        let mut two = serde_json::Map::new();
        two.insert(huge.clone(), json!({}));
        two.insert("table".into(), json!({}));
        let e = parse_request(&input(json!({"operation": "X", "resource": two}))).unwrap_err();
        assert!(e.len() < 300, "echoed {} bytes", e.len());
        assert!(!e.contains('\n'), "a raw newline reached the error");

        let mut many = serde_json::Map::new();
        for i in 0..10_000 {
            many.insert(format!("k{i}"), json!({}));
        }
        let e = parse_request(&input(json!({"operation": "X", "resource": many}))).unwrap_err();
        assert!(e.len() < 300, "echoed {} bytes", e.len());
        assert!(e.ends_with(",+9992 more"), "{e}");
    }

    #[test]
    fn batch_len_counts_filter_columns_by_column() {
        let t = |n: &str| json!({"table": {"catalogName": "lake", "schemaName": "s", "tableName": n, "columns": ["a", "b", "c"]}});
        let fc = parse_request(&input(
            json!({"operation": "FilterColumns", "filterResources": [t("t")]}),
        ))
        .unwrap();
        assert_eq!(batch_len(&fc), Ok(3));
        let two = parse_request(&input(
            json!({"operation": "FilterColumns", "filterResources": [t("t1"), t("t2")]}),
        ))
        .unwrap();
        assert!(batch_len(&two).is_err());
        let ft = parse_request(&input(
            json!({"operation": "FilterTables", "filterResources": [t("t1"), t("t2")]}),
        ))
        .unwrap();
        assert_eq!(batch_len(&ft), Ok(2));
        let none = parse_request(&input(json!({"operation": "FilterCatalogs"}))).unwrap();
        assert_eq!(batch_len(&none), Ok(0));
    }

    #[test]
    fn a_null_resource_is_absent_and_refused_by_the_value_routes() {
        let r = parse_request(&input(
            json!({"operation": "GetRowFilters", "resource": null, "filterResources": null}),
        ))
        .unwrap();
        assert_eq!(r.resource, Resource::None);
        assert!(r.filter_resources.is_empty());
        assert!(row_filter_target(&r).is_err());
        let m = parse_request(&input(
            json!({"operation": "GetColumnMask", "resource": null, "filterResources": null}),
        ))
        .unwrap();
        assert_eq!(m.resource, Resource::None);
        assert!(column_mask_target(&m).is_err());
        assert!(column_mask_targets(&m).is_err());
    }
}
