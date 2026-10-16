//! View governance B1: a view's **lineage** (the base tables Trino reports it
//! reads) and the classification tags those tables carry to the view.
//!
//! # Where the lineage comes from (spec R3, R5)
//!
//! From the engine, never from parsing the SQL: four sqlparser-based gates in
//! this repository were bypassed through lexer differences. egret runs
//! `EXPLAIN (TYPE IO, FORMAT JSON) <view sql>` on a Trino engine bound to the
//! view's catalog, as the principal [`LINEAGE_PRINCIPAL`], with the view's
//! default catalog and namespace as the session defaults. The plan lists only
//! BASE tables, for nested views and joins alike (live result §1 Q3), so the
//! lineage is table-level: every column of the view is tagged with the union
//! of those tables' clearance-bearing tags, which over-tags by design.
//!
//! # What keeps the principal read-only, and what does not
//!
//! [`explain_only`] is **not** the defence and contains no lexer on purpose.
//! The defences are, first, Trino's statement protocol, which runs exactly one
//! statement per request, so the wrapped text is an `EXPLAIN` or a parse
//! error; and second, the principal itself: on a provisioned Trino engine's
//! download egret grants it `use` and `select` and denies it everything else
//! (`egret_policy::lineage_transform`); no stored policy names it.
//! `explain_only` refuses the one case those two leave open by intent:
//! `EXPLAIN ANALYZE` executes its statement, so a SQL whose first keyword is
//! `EXPLAIN` is refused. A comment in front of it is not looked through; the
//! principal's grants are what bound that case.
//!
//! # An empty lineage is a refusal (spec R6)
//!
//! EXPLAIN reports inputs after optimization: `WHERE false`, `LIMIT 0` and a
//! planning-time-constant predicate all report none (live result §1 Q3), and
//! such a view would otherwise commit untagged and read the column the day
//! the predicate turns true. [`lineage_tables`] therefore never returns an
//! empty set: no input is [`EMPTY_LINEAGE_REASON`], and a plan of any other
//! shape is an error naming the IO plan.
//!
//! # Catalog names
//!
//! The plan names a table by the ENGINE's catalog name. On an engine egret
//! provisions, a catalog is rendered under its `registry.catalogs.name`, which
//! is "the catalog's SQL name on every engine" (migration 0147, D5) — so the
//! two are the same string, and [`view_tag_set`] joins on it. The runner in
//! `egret-query` only uses provisioned engines for this reason.

use std::collections::BTreeSet;

use serde_json::Value;
use uuid::Uuid;

pub use egret_core::engine_principals::LINEAGE_PRINCIPAL;

/// What the lineage client prepends to a view's SQL; the only statement shape
/// it sends as [`LINEAGE_PRINCIPAL`].
pub const EXPLAIN_IO_PREFIX: &str = "EXPLAIN (TYPE IO, FORMAT JSON) ";

/// Spec R6's refusal, verbatim: the reason a view with an empty lineage is
/// refused.
pub const EMPTY_LINEAGE_REASON: &str =
    "the engine reports no input table; egret refuses views it cannot judge";

/// One base table, fully qualified as `(catalog, schema, table)`, with the
/// catalog as the engine (and therefore egret's registry) names it.
pub type LineageTable = (String, String, String);

/// A view, fully qualified the same way as a [`LineageTable`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ViewRef {
    pub catalog: String,
    pub schema: String,
    pub name: String,
}

/// Why a lineage cannot be used to tag a view (spec R6: no empty or untagged
/// path may reach a commit).
#[derive(Debug, thiserror::Error)]
pub enum LineageError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// An empty set of base tables, which only R6's refusal can mean.
    #[error("{EMPTY_LINEAGE_REASON}")]
    Empty,
    /// No lineage was ever recorded for the view.
    #[error("no lineage is recorded for this view; egret refuses views it cannot judge")]
    Missing,
    /// The last computation failed.
    #[error("the view's lineage could not be computed: {0}")]
    Failed(String),
}

/// What is recorded for a view. `Computed` is never empty: [`write_lineage`]
/// refuses an empty set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordedLineage {
    Missing,
    Failed { detail: String },
    Computed(BTreeSet<LineageTable>),
}

impl RecordedLineage {
    /// The tables of a `Computed` lineage, or why there are none to use.
    pub fn tables(&self) -> Result<&BTreeSet<LineageTable>, LineageError> {
        match self {
            RecordedLineage::Computed(t) if !t.is_empty() => Ok(t),
            RecordedLineage::Computed(_) => Err(LineageError::Empty),
            RecordedLineage::Missing => Err(LineageError::Missing),
            RecordedLineage::Failed { detail } => Err(LineageError::Failed(detail.clone())),
        }
    }
}

/// Runs `EXPLAIN (TYPE IO, FORMAT JSON) <sql>` as [`LINEAGE_PRINCIPAL`] with
/// `default_catalog` / `default_schema` as the session defaults, and returns
/// the plan JSON. The production runner is
/// `egret_query::execution::lineage::TrinoLineageRunner`; tests fake it.
///
/// `sql` is the view's SQL, NOT yet wrapped: an implementation wraps it with
/// [`explain_only`], and checks that the answer is the IO-plan shape
/// ([`io_plan_inputs`]) before returning it.
#[async_trait::async_trait]
pub trait ExplainRunner: Send + Sync {
    async fn explain_io(
        &self,
        default_catalog: &str,
        default_schema: &str,
        sql: &str,
    ) -> Result<Value, String>;
}

/// Wrap `sql` as `EXPLAIN (TYPE IO, FORMAT JSON) <sql>`.
///
/// Refuses an empty (or all-whitespace) `sql`, and a `sql` whose first keyword
/// after whitespace is `EXPLAIN`, case-insensitively: `EXPLAIN ANALYZE`
/// executes. No lexer, deliberately — see the module doc for what the real
/// defences are.
pub fn explain_only(sql: &str) -> Result<String, String> {
    let rest = sql.trim_start();
    if rest.is_empty() {
        return Err("the view has no SQL to explain".to_string());
    }
    let keyword: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if keyword.eq_ignore_ascii_case("explain") {
        return Err(
            "a view's SQL may not itself be an EXPLAIN: EXPLAIN ANALYZE executes its statement"
                .to_string(),
        );
    }
    Ok(format!("{EXPLAIN_IO_PREFIX}{sql}"))
}

fn unreadable(what: impl std::fmt::Display) -> String {
    format!("the engine's answer is not the IO plan egret reads: {what}")
}

/// The plan's `inputTableColumnInfos` array, or why the plan is not the IO
/// shape. The runner calls this on every answer before returning it, so a
/// non-plan never reaches [`lineage_tables`] looking like one.
pub fn io_plan_inputs(plan: &Value) -> Result<&Vec<Value>, String> {
    plan.as_object()
        .ok_or_else(|| unreadable("not a JSON object"))?
        .get("inputTableColumnInfos")
        .ok_or_else(|| unreadable("no `inputTableColumnInfos`"))?
        .as_array()
        .ok_or_else(|| unreadable("`inputTableColumnInfos` is not an array"))
}

/// The base tables of a `EXPLAIN (TYPE IO, FORMAT JSON)` plan: each
/// `inputTableColumnInfos[].table = {catalog, schemaTable: {schema, table}}`.
///
/// **Never `Ok(empty)`.** An input list with no entry is spec R6's refusal
/// ([`EMPTY_LINEAGE_REASON`]); any other shape, or any one entry this cannot
/// read, is an error naming the IO plan. Both mean "egret cannot judge this
/// view", and an empty set would mean "this view reads nothing classified".
pub fn lineage_tables(explain_json: &Value) -> Result<BTreeSet<LineageTable>, String> {
    let inputs = io_plan_inputs(explain_json)?;
    if inputs.is_empty() {
        return Err(EMPTY_LINEAGE_REASON.to_string());
    }
    let mut out = BTreeSet::new();
    for (i, info) in inputs.iter().enumerate() {
        let table = info
            .get("table")
            .and_then(Value::as_object)
            .ok_or_else(|| unreadable(format!("input {i} has no `table` object")))?;
        let name = |v: Option<&Value>, field: &str| -> Result<String, String> {
            match v.and_then(Value::as_str) {
                Some(s) if !s.is_empty() => Ok(s.to_string()),
                _ => Err(unreadable(format!(
                    "input {i} has no non-empty string `{field}`"
                ))),
            }
        };
        let catalog = name(table.get("catalog"), "catalog")?;
        let schema_table = table
            .get("schemaTable")
            .and_then(Value::as_object)
            .ok_or_else(|| unreadable(format!("input {i} has no `schemaTable` object")))?;
        let schema = name(schema_table.get("schema"), "schemaTable.schema")?;
        let table_name = name(schema_table.get("table"), "schemaTable.table")?;
        out.insert((catalog, schema, table_name));
    }
    Ok(out)
}

/// Run the EXPLAIN through `runner` and read the plan: the view's lineage, or
/// why egret cannot judge the view (an engine error, an unreadable plan, or
/// R6's empty lineage).
pub async fn compute_lineage(
    runner: &dyn ExplainRunner,
    default_catalog: &str,
    default_schema: &str,
    sql: &str,
) -> Result<BTreeSet<LineageTable>, String> {
    let plan = runner
        .explain_io(default_catalog, default_schema, sql)
        .await?;
    lineage_tables(&plan)
}

/// The union of the **clearance-bearing** tags on any column of `tables`: the
/// tags with a `classification_policies` row that names at least one required
/// role or group (SP4c's gate; both lists empty is an informational tag and
/// carries no clearance).
///
/// A table-level assignment (`column_name = ''`) counts: Ranger's tag enricher
/// applies it to every column of the table. Matching is case-folded on the
/// schema and table, as `egret_policy::resolved_tags_for_resource_via`
/// matches; the catalog joins on `registry.catalogs.name` (module doc).
///
/// A group-only gate is included although the compiler cannot enforce it on an
/// engine (`crate::clearance`'s skip): the view column carries the same
/// classification as its base, which is what Cedar enforces for it.
///
/// **An empty `tables` is [`LineageError::Empty`], never "no tags"**: a caller
/// holding an empty set has no lineage, and "no tags" would commit the view
/// untagged. Use [`recorded_view_tag_set`] to start from what is recorded.
pub async fn view_tag_set(
    conn: &mut sqlx::PgConnection,
    tables: &BTreeSet<LineageTable>,
) -> Result<BTreeSet<Uuid>, LineageError> {
    if tables.is_empty() {
        return Err(LineageError::Empty);
    }
    let (catalogs, (schemas, names)): (Vec<&str>, (Vec<&str>, Vec<&str>)) = tables
        .iter()
        .map(|(c, s, t)| (c.as_str(), (s.as_str(), t.as_str())))
        .unzip();
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT DISTINCT a.tag_id \
           FROM unnest($1::text[], $2::text[], $3::text[]) AS l(catalog, schema_name, table_name) \
           JOIN registry.catalogs c ON c.name = l.catalog \
           JOIN catalog_tag_assignments a \
             ON a.warehouse_id = c.warehouse_id \
            AND lower(a.schema_name) = lower(l.schema_name) \
            AND lower(a.table_name) = lower(l.table_name) \
           JOIN classification_policies cp ON cp.tag_id = a.tag_id \
          WHERE cardinality(cp.required_roles) > 0 OR cardinality(cp.required_groups) > 0",
    )
    .bind(&catalogs)
    .bind(&schemas)
    .bind(&names)
    .fetch_all(&mut *conn)
    .await?;
    Ok(ids.into_iter().collect())
}

/// [`view_tag_set`] over `view`'s RECORDED lineage: an error unless the
/// lineage is `computed` (missing and failed are refusals, not "no tags").
pub async fn recorded_view_tag_set(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
) -> Result<BTreeSet<Uuid>, LineageError> {
    let recorded = recorded_lineage(conn, view).await?;
    let tables = recorded.tables()?.clone();
    view_tag_set(conn, &tables).await
}

/// Record `tables` as `view`'s lineage, replacing whatever was recorded, and
/// mark it `computed`. Runs on the caller's connection, so it joins the
/// caller's transaction (Task 8 writes it beside the view's tag assignments).
///
/// An empty `tables` is refused ([`LineageError::Empty`]) and writes nothing:
/// "computed" always has at least one row.
pub async fn write_lineage(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
    tables: &BTreeSet<LineageTable>,
) -> Result<(), LineageError> {
    if tables.is_empty() {
        return Err(LineageError::Empty);
    }
    delete_lineage_rows(conn, view).await?;
    let (catalogs, (schemas, names)): (Vec<&str>, (Vec<&str>, Vec<&str>)) = tables
        .iter()
        .map(|(c, s, t)| (c.as_str(), (s.as_str(), t.as_str())))
        .unzip();
    sqlx::query(
        "INSERT INTO view_lineage \
           (view_catalog, view_schema, view_name, base_catalog, base_schema, base_table) \
         SELECT $1, $2, $3, l.c, l.s, l.t \
           FROM unnest($4::text[], $5::text[], $6::text[]) AS l(c, s, t)",
    )
    .bind(&view.catalog)
    .bind(&view.schema)
    .bind(&view.name)
    .bind(&catalogs)
    .bind(&schemas)
    .bind(&names)
    .execute(&mut *conn)
    .await?;
    Ok(set_state(conn, view, "computed", None).await?)
}

/// Mark `view`'s lineage `failed` with `detail`, removing any recorded rows: a
/// failed computation leaves no lineage that a sweep could mistake for current.
pub async fn record_lineage_failure(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
    detail: &str,
) -> Result<(), sqlx::Error> {
    delete_lineage_rows(conn, view).await?;
    set_state(conn, view, "failed", Some(detail)).await
}

/// Remove `view`'s lineage rows and state (view dropped, or a held commit
/// compensated).
pub async fn delete_lineage(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
) -> Result<(), sqlx::Error> {
    delete_lineage_rows(conn, view).await?;
    sqlx::query(
        "DELETE FROM view_lineage_state \
          WHERE view_catalog = $1 AND view_schema = $2 AND view_name = $3",
    )
    .bind(&view.catalog)
    .bind(&view.schema)
    .bind(&view.name)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Record `view`'s columns beside its lineage state (migration 0188), or
/// forget them (`None`). Needs a recorded state (computed or failed): with
/// none, nothing is written, because a missing lineage has nothing to tag.
/// An empty slice is stored as `None`: a view always has a column, so an
/// empty list can only mean "not known".
pub async fn set_view_columns(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
    columns: Option<&[String]>,
) -> Result<(), sqlx::Error> {
    let columns = columns.filter(|c| !c.is_empty());
    sqlx::query(
        "UPDATE view_lineage_state SET columns = $4 \
          WHERE view_catalog = $1 AND view_schema = $2 AND view_name = $3",
    )
    .bind(&view.catalog)
    .bind(&view.schema)
    .bind(&view.name)
    .bind(columns)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// The columns recorded for `view` ([`set_view_columns`]), or `None` when
/// none are known.
pub async fn recorded_view_columns(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
) -> Result<Option<Vec<String>>, sqlx::Error> {
    let columns: Option<Option<Vec<String>>> = sqlx::query_scalar(
        "SELECT columns FROM view_lineage_state \
          WHERE view_catalog = $1 AND view_schema = $2 AND view_name = $3",
    )
    .bind(&view.catalog)
    .bind(&view.schema)
    .bind(&view.name)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(columns.flatten())
}

/// What is recorded for `view`: missing, failed (with why), or computed.
pub async fn recorded_lineage(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
) -> Result<RecordedLineage, sqlx::Error> {
    let state: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT state, detail FROM view_lineage_state \
          WHERE view_catalog = $1 AND view_schema = $2 AND view_name = $3",
    )
    .bind(&view.catalog)
    .bind(&view.schema)
    .bind(&view.name)
    .fetch_optional(&mut *conn)
    .await?;
    match state {
        None => return Ok(RecordedLineage::Missing),
        Some((s, detail)) if s != "computed" => {
            return Ok(RecordedLineage::Failed {
                detail: detail.unwrap_or_default(),
            })
        }
        Some(_) => {}
    }
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT base_catalog, base_schema, base_table FROM view_lineage \
          WHERE view_catalog = $1 AND view_schema = $2 AND view_name = $3",
    )
    .bind(&view.catalog)
    .bind(&view.schema)
    .bind(&view.name)
    .fetch_all(&mut *conn)
    .await?;
    Ok(RecordedLineage::Computed(rows.into_iter().collect()))
}

/// [`recorded_lineage`] for many views at once, in two queries, in the order
/// of `views`.
pub async fn recorded_lineages(
    conn: &mut sqlx::PgConnection,
    views: &[ViewRef],
) -> Result<Vec<RecordedLineage>, sqlx::Error> {
    if views.is_empty() {
        return Ok(Vec::new());
    }
    let (c, (s, n)): (Vec<&str>, (Vec<&str>, Vec<&str>)) = views
        .iter()
        .map(|v| (v.catalog.as_str(), (v.schema.as_str(), v.name.as_str())))
        .unzip();
    let states: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT st.view_catalog, st.view_schema, st.view_name, st.state, st.detail \
           FROM view_lineage_state st \
           JOIN unnest($1::text[], $2::text[], $3::text[]) AS v(c, s, n) \
             ON st.view_catalog = v.c AND st.view_schema = v.s AND st.view_name = v.n",
    )
    .bind(&c)
    .bind(&s)
    .bind(&n)
    .fetch_all(&mut *conn)
    .await?;
    let rows: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT l.view_catalog, l.view_schema, l.view_name, \
                l.base_catalog, l.base_schema, l.base_table \
           FROM view_lineage l \
           JOIN unnest($1::text[], $2::text[], $3::text[]) AS v(c, s, n) \
             ON l.view_catalog = v.c AND l.view_schema = v.s AND l.view_name = v.n",
    )
    .bind(&c)
    .bind(&s)
    .bind(&n)
    .fetch_all(&mut *conn)
    .await?;
    let key = |c: String, s: String, n: String| ViewRef {
        catalog: c,
        schema: s,
        name: n,
    };
    let mut by_view: std::collections::HashMap<ViewRef, RecordedLineage> = states
        .into_iter()
        .map(|(c, s, n, state, detail)| {
            let rec = if state == "computed" {
                RecordedLineage::Computed(BTreeSet::new())
            } else {
                RecordedLineage::Failed {
                    detail: detail.unwrap_or_default(),
                }
            };
            (key(c, s, n), rec)
        })
        .collect();
    for (c, s, n, bc, bs, bt) in rows {
        if let Some(RecordedLineage::Computed(tables)) = by_view.get_mut(&key(c, s, n)) {
            tables.insert((bc, bs, bt));
        }
    }
    Ok(views
        .iter()
        .map(|v| by_view.remove(v).unwrap_or(RecordedLineage::Missing))
        .collect())
}

async fn delete_lineage_rows(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM view_lineage \
          WHERE view_catalog = $1 AND view_schema = $2 AND view_name = $3",
    )
    .bind(&view.catalog)
    .bind(&view.schema)
    .bind(&view.name)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn set_state(
    conn: &mut sqlx::PgConnection,
    view: &ViewRef,
    state: &str,
    detail: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO view_lineage_state (view_catalog, view_schema, view_name, state, detail, at) \
         VALUES ($1, $2, $3, $4, $5, now()) \
         ON CONFLICT (view_catalog, view_schema, view_name) \
         DO UPDATE SET state = EXCLUDED.state, detail = EXCLUDED.detail, at = EXCLUDED.at",
    )
    .bind(&view.catalog)
    .bind(&view.schema)
    .bind(&view.name)
    .bind(state)
    .bind(detail)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t(c: &str, s: &str, n: &str) -> LineageTable {
        (c.to_string(), s.to_string(), n.to_string())
    }

    /// Live result §1 Q3, the join of `ice.s.pii` and `ice.s.orders`, verbatim
    /// except that the recorded `"estimate":{…}` elisions are written `{}`.
    fn join_plan() -> Value {
        serde_json::from_str(
            r#"{"inputTableColumnInfos":[
    {"table":{"catalog":"ice","schemaTable":{"schema":"s","table":"orders"}},"constraint":{"none":false,"columnConstraints":[]},"estimate":{}},
    {"table":{"catalog":"ice","schemaTable":{"schema":"s","table":"pii"}},"constraint":{"none":false,"columnConstraints":[]},"estimate":{}}],
   "estimate":{}}"#,
        )
        .expect("recorded join plan")
    }

    /// Live result §1 Q3, the nested view `SELECT id, amount FROM
    /// ice.s.v_nest_inv` (INVOKER over DEFINER over the join): the recorded
    /// inputs, under the recorded top-level keys.
    fn nested_view_plan() -> Value {
        let inputs: Value = serde_json::from_str(
            r#"[{"catalog":"ice","schemaTable":{"schema":"s","table":"orders"}},{"catalog":"ice","schemaTable":{"schema":"s","table":"pii"}}]"#,
        )
        .expect("recorded inputs");
        let infos: Vec<Value> = inputs
            .as_array()
            .unwrap()
            .iter()
            .map(|table| {
                json!({
                    "table": table,
                    "constraint": {"none": false, "columnConstraints": []},
                    "estimate": {}
                })
            })
            .collect();
        json!({ "inputTableColumnInfos": infos, "estimate": {} })
    }

    #[test]
    fn the_join_gives_exactly_its_two_base_tables() {
        assert_eq!(
            lineage_tables(&join_plan()).unwrap(),
            BTreeSet::from([t("ice", "s", "orders"), t("ice", "s", "pii")])
        );
    }

    #[test]
    fn the_nested_view_gives_its_base_tables_and_never_a_view() {
        assert_eq!(
            lineage_tables(&nested_view_plan()).unwrap(),
            BTreeSet::from([t("ice", "s", "orders"), t("ice", "s", "pii")])
        );
    }

    /// Live result §1 Q3, unqualified SQL with session `ice`/`s`.
    #[test]
    fn unqualified_sql_resolves_to_the_session_catalog_and_schema() {
        let plan: Value = json!({
            "inputTableColumnInfos": [{
                "table": {"catalog":"ice","schemaTable":{"schema":"s","table":"pii"}},
                "constraint": {"none": false, "columnConstraints": []},
                "estimate": {}
            }],
            "estimate": {}
        });
        assert_eq!(
            lineage_tables(&plan).unwrap(),
            BTreeSet::from([t("ice", "s", "pii")])
        );
    }

    /// Every way the plan can fail to be the IO shape is an error that says
    /// so, never an empty set: an empty set would read as "reads no
    /// classified table" and the view would commit untagged.
    #[test]
    fn an_unexpected_shape_is_an_error_never_an_empty_set() {
        let cases = [
            json!(null),
            json!([]),
            json!("plan"),
            json!({}),
            json!({"estimate": {}}),
            json!({"inputTableColumnInfos": {}}),
            json!({"inputTableColumnInfos": [{}]}),
            json!({"inputTableColumnInfos": [{"table": "ice.s.pii"}]}),
            json!({"inputTableColumnInfos": [{"table": {"catalog": "ice"}}]}),
            json!({"inputTableColumnInfos": [{"table": {"catalog": "ice", "schemaTable": {"schema": "s"}}}]}),
            json!({"inputTableColumnInfos": [{"table": {"catalog": 1, "schemaTable": {"schema": "s", "table": "t"}}}]}),
            json!({"inputTableColumnInfos": [{"table": {"catalog": "", "schemaTable": {"schema": "s", "table": "t"}}}]}),
            // One good input does not excuse an unreadable one.
            json!({"inputTableColumnInfos": [
                {"table": {"catalog": "ice", "schemaTable": {"schema": "s", "table": "pii"}}},
                {"table": {"catalog": "ice", "schemaTable": {"schema": "s"}}}
            ]}),
        ];
        for plan in cases {
            let err = lineage_tables(&plan).expect_err(&plan.to_string());
            assert!(
                err.contains("IO plan"),
                "the refusal must say the plan was unreadable, for {plan}: {err}"
            );
        }
    }

    /// Spec R6: `WHERE false`, `LIMIT 0` and a planning-time-constant
    /// predicate all report no input. An empty lineage is a refusal.
    #[test]
    fn an_empty_input_list_is_the_r6_refusal() {
        let err = lineage_tables(&json!({"inputTableColumnInfos": [], "estimate": {}}))
            .expect_err("empty");
        assert_eq!(err, EMPTY_LINEAGE_REASON);
    }

    #[test]
    fn explain_only_wraps_the_statement() {
        assert_eq!(
            explain_only("SELECT id, ssn FROM pii").unwrap(),
            "EXPLAIN (TYPE IO, FORMAT JSON) SELECT id, ssn FROM pii"
        );
    }

    #[test]
    fn explain_only_refuses_empty_sql_and_a_nested_explain() {
        for sql in [
            "",
            "   \n\t",
            "explain analyze select 1",
            "EXPLAIN ANALYZE SELECT 1",
            "  Explain select 1",
            "\nEXPLAIN(TYPE IO) select 1",
        ] {
            assert!(explain_only(sql).is_err(), "{sql:?} must be refused");
        }
        // A keyword that merely starts with the letters is not EXPLAIN.
        assert!(explain_only("explainer_view").is_ok());
        assert!(explain_only("SELECT 'explain' AS x").is_ok());
    }

    struct Fake(Value);

    #[async_trait::async_trait]
    impl ExplainRunner for Fake {
        async fn explain_io(&self, _: &str, _: &str, _: &str) -> Result<Value, String> {
            Ok(self.0.clone())
        }
    }

    #[tokio::test]
    async fn a_runner_reporting_no_input_is_refused_with_the_r6_reason() {
        let err = compute_lineage(
            &Fake(json!({"inputTableColumnInfos": [], "estimate": {}})),
            "ice",
            "s",
            "SELECT id FROM pii WHERE false",
        )
        .await
        .expect_err("empty lineage");
        assert_eq!(err, EMPTY_LINEAGE_REASON);
    }

    #[tokio::test]
    async fn a_runner_reporting_the_join_gives_its_tables() {
        let got = compute_lineage(&Fake(join_plan()), "ice", "s", "SELECT 1")
            .await
            .expect("lineage");
        assert_eq!(
            got,
            BTreeSet::from([t("ice", "s", "orders"), t("ice", "s", "pii")])
        );
    }
}
