//! Pure chart query builder: QuerySpec → SQL. No I/O. Filter values are the only
//! untrusted input and pass through `sql_literal`; identifiers/expressions are
//! dataset-sourced and trusted.

use egret_query_api::Engine;
use serde::Deserialize;

use crate::dataset::{DatasetColumn, Metric, SourceSpec};
use crate::error::BiError;
use crate::expr::source_ref_for;
use crate::schema::quote_ident;

/// Hard ceiling on rows a chart query may request.
pub const MAX_CHART_ROWS: i64 = 10_000;

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct QuerySpec {
    #[serde(default)]
    pub metrics: Vec<String>,
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub orderby: Vec<OrderBy>,
    pub row_limit: i64,
    #[serde(default)]
    pub rollup_groups: Vec<Vec<String>>,
    /// dimension column name -> temporal unit token (snake_case, e.g. "month", "day_of_week").
    /// Empty (default) = no truncation = legacy behavior.
    #[serde(default)]
    pub temporal_units: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct Filter {
    pub col: String,
    pub op: FilterOp,
    /// Comparison operand. Scalar for most operators; a JSON array for `in`/`not_in`;
    /// ignored for `is_null`/`is_not_null`. Validated against the column's generic type.
    #[serde(default)]
    #[schema(value_type = Object)]
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct OrderBy {
    pub field: String,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Like,
    NotLike,
    In,
    NotIn,
    IsNull,
    IsNotNull,
}

/// A Metabase-style temporal grouping unit. Truncation variants bucket to a date/timestamp;
/// extraction variants (`*Of*`) return an integer field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemporalUnit {
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
    MinuteOfHour,
    HourOfDay,
    DayOfWeek,
    DayOfMonth,
    DayOfYear,
    WeekOfYear,
    MonthOfYear,
    QuarterOfYear,
}

impl TemporalUnit {
    pub fn parse(token: &str) -> Option<Self> {
        use TemporalUnit::*;
        Some(match token {
            "minute" => Minute,
            "hour" => Hour,
            "day" => Day,
            "week" => Week,
            "month" => Month,
            "quarter" => Quarter,
            "year" => Year,
            "minute_of_hour" => MinuteOfHour,
            "hour_of_day" => HourOfDay,
            "day_of_week" => DayOfWeek,
            "day_of_month" => DayOfMonth,
            "day_of_year" => DayOfYear,
            "week_of_year" => WeekOfYear,
            "month_of_year" => MonthOfYear,
            "quarter_of_year" => QuarterOfYear,
            _ => return None,
        })
    }

    pub fn is_extract(self) -> bool {
        use TemporalUnit::*;
        matches!(
            self,
            MinuteOfHour
                | HourOfDay
                | DayOfWeek
                | DayOfMonth
                | DayOfYear
                | WeekOfYear
                | MonthOfYear
                | QuarterOfYear
        )
    }
}

/// Encode a filter value as a SQL literal, validated by the target column's generic type.
/// Numerics are validated and emitted unquoted; booleans normalized; everything else is
/// single-quoted with `'` doubled. This is the security-critical primitive.
pub(crate) fn sql_literal(v: &serde_json::Value, type_generic: &str) -> Result<String, BiError> {
    match type_generic {
        "numeric" => {
            if let Some(n) = v.as_i64() {
                Ok(n.to_string())
            } else if let Some(n) = v.as_u64() {
                Ok(n.to_string())
            } else if let Some(n) = v.as_f64() {
                if n.is_finite() {
                    Ok(n.to_string())
                } else {
                    Err(BiError::BadRequest(format!("not a finite number: {n}")))
                }
            } else if let Some(s) = v.as_str() {
                s.parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .map(|n| n.to_string())
                    .ok_or_else(|| BiError::BadRequest(format!("not a number: {s}")))
            } else {
                Err(BiError::BadRequest(
                    "expected a numeric filter value".into(),
                ))
            }
        }
        "boolean" => {
            let b = match v {
                serde_json::Value::Bool(b) => *b,
                serde_json::Value::String(s) => match s.to_ascii_lowercase().as_str() {
                    "true" | "1" => true,
                    "false" | "0" => false,
                    _ => return Err(BiError::BadRequest(format!("not a boolean: {s}"))),
                },
                _ => {
                    return Err(BiError::BadRequest(
                        "expected a boolean filter value".into(),
                    ))
                }
            };
            Ok(if b { "TRUE".into() } else { "FALSE".into() })
        }
        _ => {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => {
                    return Err(BiError::BadRequest("filter value is null".into()))
                }
                other => other.to_string(),
            };
            Ok(format!("'{}'", s.replace('\'', "''")))
        }
    }
}

/// Resolve a column to its SQL expression: a calculated column uses its stored expression;
/// a physical column uses its quoted identifier.
fn column_sql(engine: Engine, c: &DatasetColumn) -> String {
    match &c.expression {
        Some(expr) => expr.clone(),
        None => quote_ident(engine, &c.column_name),
    }
}

/// Wrap an already-resolved column expression in its temporal grouping. `inner` is trusted
/// (dataset-sourced); the unit string is a fixed literal, never user text.
fn temporal_expr(engine: Engine, inner: &str, unit: TemporalUnit) -> String {
    use TemporalUnit::*;
    let trunc = |u: &str| format!("date_trunc('{u}', {inner})");
    match unit {
        Minute => trunc("minute"),
        Hour => trunc("hour"),
        Day => trunc("day"),
        Week => trunc("week"),
        Month => trunc("month"),
        Quarter => trunc("quarter"),
        Year => trunc("year"),
        // extraction variants (hour-of-day, day-of-week, …) delegate to extract_expr
        _ => extract_expr(engine, inner, unit),
    }
}

/// Extraction units differ by engine; results are normalized so both engines return the
/// same integers (ISO week, Monday=1 weekday), matching Trino and Metabase.
fn extract_expr(engine: Engine, inner: &str, unit: TemporalUnit) -> String {
    use Engine::*;
    use TemporalUnit::*;
    match (engine, unit) {
        (_, MinuteOfHour) => format!("minute({inner})"),
        (_, HourOfDay) => format!("hour({inner})"),
        (_, MonthOfYear) => format!("month({inner})"),
        (_, QuarterOfYear) => format!("quarter({inner})"),

        (Trino, DayOfWeek) => format!("day_of_week({inner})"),
        (StarRocks, DayOfWeek) => format!("(weekday({inner}) + 1)"),

        (Trino, DayOfMonth) => format!("day_of_month({inner})"),
        (StarRocks, DayOfMonth) => format!("dayofmonth({inner})"),

        (Trino, DayOfYear) => format!("day_of_year({inner})"),
        (StarRocks, DayOfYear) => format!("dayofyear({inner})"),

        (Trino, WeekOfYear) => format!("week_of_year({inner})"),
        (StarRocks, WeekOfYear) => format!("week({inner}, 3)"),

        // Truncation variants are handled by temporal_expr and never dispatched here.
        (_, Minute | Hour | Day | Week | Month | Quarter | Year) => {
            unreachable!("truncation units must be handled by temporal_expr, not extract_expr")
        }
    }
}

fn find_column<'a>(columns: &'a [DatasetColumn], name: &str) -> Option<&'a DatasetColumn> {
    columns.iter().find(|c| c.column_name == name)
}

/// Resolve a dimension name to SQL, applying its temporal unit (from `spec.temporal_units`) if set.
/// A unit on a non-temporal column, or an unknown unit token, is a BadRequest.
fn dim_expr(
    engine: Engine,
    columns: &[DatasetColumn],
    spec: &QuerySpec,
    name: &str,
) -> Result<String, BiError> {
    let c = find_column(columns, name)
        .ok_or_else(|| BiError::BadRequest(format!("unknown dimension: {name}")))?;
    let base = column_sql(engine, c);
    match spec.temporal_units.get(name) {
        None => Ok(base),
        Some(tok) => {
            if !c.is_temporal {
                return Err(BiError::BadRequest(format!(
                    "temporal unit on non-temporal column: {name}"
                )));
            }
            let unit = TemporalUnit::parse(tok)
                .ok_or_else(|| BiError::BadRequest(format!("unknown temporal unit: {tok}")))?;
            Ok(temporal_expr(engine, &base, unit))
        }
    }
}
fn find_metric<'a>(metrics: &'a [Metric], name: &str) -> Option<&'a Metric> {
    metrics.iter().find(|m| m.metric_name == name)
}

/// Give a temporal literal a type, so the engine will compare it against a date column.
///
/// `sql_literal` emits a bare quoted string for anything that is not numeric or boolean, and no
/// engine will implicitly compare `date = varchar`:
///
/// ```text
/// Cannot apply operator: date = varchar(10)
/// ```
///
/// Every drill on a time dimension produced exactly that — clicking a cell in a pivot, a bar or a
/// line chart with a date on the axis returned a 502, on every chart type. The literal is already
/// quoted and escaped by `sql_literal`; this only wraps it.
///
/// The type name differs by engine: StarRocks calls a timestamp `DATETIME`, Trino calls it
/// `TIMESTAMP`. `CAST` (rather than Trino's `DATE '…'` literal syntax) is what both accept.
fn cast_temporal(engine: Engine, c: &DatasetColumn, lit: String) -> String {
    if !c.is_temporal || !lit.starts_with('\'') {
        return lit;
    }
    // `data_type` is the engine's own name for the column; only a plain date has no time part.
    let date_only = c.data_type.trim().to_ascii_lowercase().starts_with("date")
        && !c.data_type.to_ascii_lowercase().contains("time");
    let ty = if date_only {
        "DATE"
    } else if matches!(engine, Engine::StarRocks) {
        "DATETIME"
    } else {
        "TIMESTAMP"
    };
    format!("CAST({lit} AS {ty})")
}

/// Render one column filter into a WHERE predicate. Metric filters are rejected (→ HAVING,
/// deferred to 3c-3.2). The operator comes from an enum, so no operator injection is possible.
fn render_filter(engine: Engine, columns: &[DatasetColumn], f: &Filter) -> Result<String, BiError> {
    let c = find_column(columns, &f.col)
        .ok_or_else(|| BiError::BadRequest(format!("unknown filter column: {}", f.col)))?;
    // Filters compare the raw column, never the temporal bucket — a date range must match the
    // real timestamp, not the truncated group key.
    let lhs = column_sql(engine, c);
    let tg = c.type_generic.as_str();
    let pred = match f.op {
        FilterOp::IsNull => format!("{lhs} IS NULL"),
        FilterOp::IsNotNull => format!("{lhs} IS NOT NULL"),
        FilterOp::In | FilterOp::NotIn => {
            let arr = f
                .value
                .as_array()
                .ok_or_else(|| BiError::BadRequest("IN filter needs an array value".into()))?;
            if arr.is_empty() {
                return Err(BiError::BadRequest("IN filter value is empty".into()));
            }
            let items = arr
                .iter()
                .map(|v| sql_literal(v, tg).map(|l| cast_temporal(engine, c, l)))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ");
            let kw = if matches!(f.op, FilterOp::In) {
                "IN"
            } else {
                "NOT IN"
            };
            format!("{lhs} {kw} ({items})")
        }
        _ => {
            let lit = cast_temporal(engine, c, sql_literal(&f.value, tg)?);
            let op = match f.op {
                FilterOp::Eq => "=",
                FilterOp::Ne => "<>",
                FilterOp::Gt => ">",
                FilterOp::Ge => ">=",
                FilterOp::Lt => "<",
                FilterOp::Le => "<=",
                FilterOp::Like => "LIKE",
                FilterOp::NotLike => "NOT LIKE",
                _ => unreachable!("handled above"),
            };
            format!("{lhs} {op} {lit}")
        }
    };
    Ok(pred)
}

/// Resolve an ORDER BY field to SQL: a metric expression, a grouped dimension (with its temporal
/// unit), or a bare column. Order matters — a dimension with a unit must match its GROUP BY expr.
fn orderby_sql(
    engine: Engine,
    columns: &[DatasetColumn],
    metrics: &[Metric],
    spec: &QuerySpec,
    field: &str,
) -> Result<String, BiError> {
    if let Some(m) = find_metric(metrics, field) {
        return Ok(m.expression.clone());
    }
    if spec.dimensions.iter().any(|d| d == field) {
        return dim_expr(engine, columns, spec, field);
    }
    if let Some(c) = find_column(columns, field) {
        return Ok(column_sql(engine, c));
    }
    Err(BiError::BadRequest(format!(
        "unknown order-by field: {field}"
    )))
}

/// Build the SQL for a chart query. Aggregate mode when `metrics` is non-empty; otherwise
/// raw-records mode over `columns`. GROUP BY / SELECT use resolved expressions, never aliases.
pub fn build(
    engine: Engine,
    source: &SourceSpec,
    columns: &[DatasetColumn],
    metrics: &[Metric],
    spec: &QuerySpec,
) -> Result<String, BiError> {
    let from = source_ref_for(engine, source);
    let aggregate = !spec.metrics.is_empty();
    let mut select: Vec<String> = Vec::new();

    // A temporal unit must target a column the query actually selects (a dimension in aggregate
    // mode, or a listed column in raw-records mode); otherwise it would be silently ignored.
    for key in spec.temporal_units.keys() {
        let targeted =
            spec.dimensions.iter().any(|d| d == key) || spec.columns.iter().any(|c| c == key);
        if !targeted {
            return Err(BiError::BadRequest(format!(
                "temporal unit for a column that is not selected: {key}"
            )));
        }
    }

    if aggregate {
        if !spec.rollup_groups.is_empty() && spec.dimensions.is_empty() {
            return Err(BiError::BadRequest(
                "rollup requires at least one dimension".into(),
            ));
        }
        // When rolling up, a dimension outside every group would be selected without appearing in
        // any grouping set — not valid SQL, and the engine's complaint ("must be an aggregate
        // expression or appear in GROUP BY") names an internal `__dim_N` the caller never wrote.
        // Reject it here, where the message can name the caller's own dimension. The pivot builder
        // always covers every dimension, so this is a guard on the public API, not on the UI.
        if !spec.rollup_groups.is_empty() {
            for name in &spec.dimensions {
                if !spec.rollup_groups.iter().any(|g| g.contains(name)) {
                    return Err(BiError::BadRequest(format!(
                        "dimension is not in any rollup group: {name}"
                    )));
                }
            }
        }
        // Resolve every dimension to its expression once; remember each name's position.
        let mut dim_exprs: Vec<String> = Vec::new();
        let mut dim_index: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for (i, name) in spec.dimensions.iter().enumerate() {
            let e = dim_expr(engine, columns, spec, name)?;
            dim_index.insert(name.as_str(), i);
            dim_exprs.push(e);
        }
        let has_rollup = !spec.rollup_groups.is_empty();

        // A rollup query mentions each dimension THREE times: in the SELECT list, in
        // `GROUPING(...)`, and inside `GROUP BY GROUPING SETS (...)`. That is fine while a
        // dimension is a bare column, and breaks the moment it is an expression — a temporal unit
        // makes it `date_trunc('month', "order_date")`, and `GROUPING()` accepts a COLUMN
        // REFERENCE, not a function call. Trino reads `date_trunc` as the start of a qualified
        // name, hits `(`, and fails to parse:
        //
        //     line 2:164: mismatched input '('. Expecting: ')', ',', '.'
        //
        // Every pivot with a time bucket died here, and the studio defaults a date column's unit
        // to Month, so it was the first thing a user hit rather than an edge case.
        //
        // So when rolling up, each dimension expression is materialised once in a derived table
        // under a reserved name, and everything downstream refers to that plain identifier. This
        // also keeps expressions out of `GROUPING SETS`, which StarRocks parses more strictly than
        // Trino does — the same reason this file already emits GROUPING SETS instead of ROLLUP().
        //
        // `SELECT *` carries the raw columns through, so metric expressions (`sum(totalprice)`)
        // and the WHERE clause still resolve against real columns — a filter must match the real
        // timestamp, never the truncated group key (see `render_filter`). The `__dim_` prefix
        // follows the existing `__grp_` convention: reserved, and not reachable as a user column.
        let from = if has_rollup {
            let projections: Vec<String> = dim_exprs
                .iter()
                .enumerate()
                // {i} is a usize counter, never user data — safe unquoted.
                .map(|(i, e)| format!("{e} AS __dim_{i}"))
                .collect();
            let inner = format!("(SELECT *, {} FROM {from}) AS __b", projections.join(", "));
            dim_exprs = (0..dim_exprs.len()).map(|i| format!("__dim_{i}")).collect();
            inner
        } else {
            from
        };

        for (i, name) in spec.dimensions.iter().enumerate() {
            select.push(format!("{} AS {}", dim_exprs[i], quote_ident(engine, name)));
        }
        for name in &spec.metrics {
            let m = find_metric(metrics, name)
                .ok_or_else(|| BiError::BadRequest(format!("unknown metric: {name}")))?;
            select.push(format!("{} AS {}", m.expression, quote_ident(engine, name)));
        }
        if has_rollup {
            for (i, e) in dim_exprs.iter().enumerate() {
                // {i} is a usize counter, never user data — safe unquoted.
                select.push(format!("GROUPING({e}) AS __grp_{i}"));
            }
        }
        let mut sql = format!("SELECT {}\nFROM {from}", select.join(", "));
        push_where(&mut sql, engine, columns, &spec.filters)?;
        if has_rollup {
            // Each group contributes its ROLLUP levels (full prefix down to empty); the
            // cross-product across groups is every grouping level. We emit explicit
            // GROUPING SETS rather than `ROLLUP(a), ROLLUP(b)` because the latter is
            // Trino-only — StarRocks parses `ROLLUP(col)` as a function call and errors.
            // GROUPING SETS is accepted by both. Rollup names must be a subset of
            // `dimensions` (so each has a GROUPING marker).
            let mut group_levels: Vec<Vec<Vec<String>>> = Vec::new();
            for group in &spec.rollup_groups {
                let mut exprs: Vec<String> = Vec::new();
                for name in group {
                    let &i = dim_index.get(name.as_str()).ok_or_else(|| {
                        BiError::BadRequest(format!("rollup dimension not in dimensions: {name}"))
                    })?;
                    exprs.push(dim_exprs[i].clone());
                }
                if exprs.is_empty() {
                    continue;
                }
                // ROLLUP(a, b) ⇒ levels (a, b), (a), (): every prefix, longest first.
                let levels: Vec<Vec<String>> = (0..=exprs.len())
                    .rev()
                    .map(|k| exprs[..k].to_vec())
                    .collect();
                group_levels.push(levels);
            }
            if group_levels.is_empty() {
                return Err(BiError::BadRequest(
                    "rollup_groups contains no resolvable dimensions".into(),
                ));
            }
            // Cartesian product of each group's levels → the full set of grouping sets.
            let mut sets: Vec<Vec<String>> = vec![Vec::new()];
            for levels in &group_levels {
                let mut next: Vec<Vec<String>> = Vec::new();
                for prefix in &sets {
                    for level in levels {
                        let mut combined = prefix.clone();
                        combined.extend(level.iter().cloned());
                        next.push(combined);
                    }
                }
                sets = next;
            }
            let sets_sql: Vec<String> =
                sets.iter().map(|s| format!("({})", s.join(", "))).collect();
            sql.push_str(&format!(
                "\nGROUP BY GROUPING SETS ({})",
                sets_sql.join(", ")
            ));
        } else if !dim_exprs.is_empty() {
            sql.push_str(&format!("\nGROUP BY {}", dim_exprs.join(", ")));
        }
        push_orderby(&mut sql, engine, columns, metrics, spec)?;
        push_limit(&mut sql, spec.row_limit);
        Ok(sql)
    } else {
        if spec.columns.is_empty() {
            return Err(BiError::BadRequest(
                "a table needs at least one metric or column".into(),
            ));
        }
        for name in &spec.columns {
            let c = find_column(columns, name)
                .ok_or_else(|| BiError::BadRequest(format!("unknown column: {name}")))?;
            select.push(format!(
                "{} AS {}",
                column_sql(engine, c),
                quote_ident(engine, name)
            ));
        }
        let mut sql = format!("SELECT {}\nFROM {from}", select.join(", "));
        push_where(&mut sql, engine, columns, &spec.filters)?;
        push_orderby(&mut sql, engine, columns, metrics, spec)?;
        push_limit(&mut sql, spec.row_limit);
        Ok(sql)
    }
}

fn push_where(
    sql: &mut String,
    engine: Engine,
    columns: &[DatasetColumn],
    filters: &[Filter],
) -> Result<(), BiError> {
    if filters.is_empty() {
        return Ok(());
    }
    let preds = filters
        .iter()
        .map(|f| render_filter(engine, columns, f))
        .collect::<Result<Vec<_>, _>>()?;
    sql.push_str(&format!("\nWHERE {}", preds.join(" AND ")));
    Ok(())
}

fn push_orderby(
    sql: &mut String,
    engine: Engine,
    columns: &[DatasetColumn],
    metrics: &[Metric],
    spec: &QuerySpec,
) -> Result<(), BiError> {
    if spec.orderby.is_empty() {
        return Ok(());
    }
    let parts = spec
        .orderby
        .iter()
        .map(|o| {
            let e = orderby_sql(engine, columns, metrics, spec, &o.field)?;
            Ok(format!("{e} {}", if o.desc { "DESC" } else { "ASC" }))
        })
        .collect::<Result<Vec<_>, BiError>>()?;
    sql.push_str(&format!("\nORDER BY {}", parts.join(", ")));
    Ok(())
}

fn push_limit(sql: &mut String, row_limit: i64) {
    let n = row_limit.clamp(1, MAX_CHART_ROWS);
    sql.push_str(&format!("\nLIMIT {n}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn temporal_unit_parse_and_classify() {
        use TemporalUnit::*;
        assert_eq!(TemporalUnit::parse("month"), Some(Month));
        assert_eq!(TemporalUnit::parse("day_of_week"), Some(DayOfWeek));
        assert_eq!(TemporalUnit::parse("week_of_year"), Some(WeekOfYear));
        assert_eq!(TemporalUnit::parse("nonsense"), None);
        assert!(!Month.is_extract());
        assert!(!Year.is_extract());
        assert!(DayOfWeek.is_extract());
        assert!(HourOfDay.is_extract());
    }

    #[test]
    fn sql_literal_encodes_by_type() {
        assert_eq!(sql_literal(&json!(42), "numeric").unwrap(), "42");
        assert_eq!(sql_literal(&json!("42"), "numeric").unwrap(), "42");
        assert!(sql_literal(&json!("nope"), "numeric").is_err());
        assert_eq!(sql_literal(&json!(true), "boolean").unwrap(), "TRUE");
        assert_eq!(sql_literal(&json!("hi"), "string").unwrap(), "'hi'");
        // injection attempt stays safely quoted
        assert_eq!(
            sql_literal(&json!("a' OR '1'='1"), "string").unwrap(),
            "'a'' OR ''1''=''1'"
        );
    }

    #[test]
    fn sql_literal_rejects_non_finite() {
        assert!(sql_literal(&json!("inf"), "numeric").is_err());
        assert!(sql_literal(&json!("nan"), "numeric").is_err());
        assert!(sql_literal(&json!("infinity"), "numeric").is_err());
        // finite still works
        assert_eq!(sql_literal(&json!("3.5"), "numeric").unwrap(), "3.5");
    }

    fn col(name: &str, tg: &str, expr: Option<&str>) -> DatasetColumn {
        DatasetColumn {
            column_name: name.into(),
            data_type: "x".into(),
            type_generic: tg.into(),
            is_temporal: false,
            ordinal: 0,
            expression: expr.map(|e| e.into()),
            verbose_name: String::new(),
            description: String::new(),
            groupby: true,
            filterable: true,
            format: String::new(),
            semantic_type: String::new(),
        }
    }
    fn metric(name: &str, expr: &str) -> Metric {
        Metric {
            id: uuid::Uuid::nil(),
            metric_name: name.into(),
            expression: expr.into(),
            data_type: "double".into(),
            type_generic: "numeric".into(),
            verbose_name: String::new(),
            description: String::new(),
            ordinal: 0,
            format: String::new(),
            certified: false,
            certified_by: String::new(),
            certification_details: String::new(),
        }
    }
    fn phys() -> SourceSpec {
        SourceSpec::Physical {
            catalog: Some("c".into()),
            schema: "s".into(),
            table: "t".into(),
        }
    }

    #[test]
    fn build_aggregate_groups_by_dimension_expression() {
        let cols = vec![
            col("region", "string", None),
            col("priced", "numeric", Some("price/100")),
        ];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into(), "priced".into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![],
            temporal_units: std::collections::HashMap::new(),
        };
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        assert!(sql.contains("\"region\" AS \"region\""), "{sql}");
        assert!(sql.contains("price/100 AS \"priced\""), "{sql}");
        assert!(sql.contains("SUM(price) AS \"revenue\""), "{sql}");
        assert!(sql.contains("GROUP BY \"region\", price/100"), "{sql}");
        assert!(sql.contains("LIMIT 100"), "{sql}");
    }

    #[test]
    fn build_rollup_adds_grouping_markers() {
        let cols = vec![col("region", "string", None), col("seg", "string", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into(), "seg".into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![vec!["region".into()], vec!["seg".into()]],
            temporal_units: std::collections::HashMap::new(),
        };
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        // ROLLUP(region) × ROLLUP(seg) expanded to portable GROUPING SETS
        // (works on both Trino and StarRocks). Dimensions are materialised in a derived table
        // first, so every one of these positions is a bare identifier — see the comment on the
        // rollup branch for why an expression here is a parse error.
        assert!(
            sql.contains("GROUP BY GROUPING SETS ((__dim_0, __dim_1), (__dim_0), (__dim_1), ())"),
            "{sql}"
        );
        assert!(sql.contains("GROUPING(__dim_0) AS __grp_0"), "{sql}");
        assert!(sql.contains("GROUPING(__dim_1) AS __grp_1"), "{sql}");
        // The output still speaks the user's column names.
        assert!(sql.contains("__dim_0 AS \"region\""), "{sql}");
        assert!(
            sql.contains("(SELECT *, \"region\" AS __dim_0, \"seg\" AS __dim_1 FROM"),
            "{sql}"
        );
    }

    /// A pivot: two rollup groups, one of them bucketed by month.
    ///
    /// This is the shape that could not run at all. `rollup_groups` and `temporal_units` each had
    /// tests; their INTERSECTION had none — all four rollup tests passed an empty units map — and
    /// the bug lived in exactly that empty cell. The tests below fill the row.
    fn pivot_spec(unit: Option<&str>) -> QuerySpec {
        let mut tu = std::collections::HashMap::new();
        if let Some(u) = unit {
            tu.insert("created".to_string(), u.to_string());
        }
        QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into(), "created".into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![vec!["region".into()], vec!["created".into()]],
            temporal_units: tu,
        }
    }

    fn temporal_cols() -> Vec<DatasetColumn> {
        let mut c = col("created", "datetime", None);
        c.is_temporal = true;
        vec![col("region", "string", None), c]
    }

    /// The invariant the whole fix exists to hold: whatever a dimension's expression is, the three
    /// places a rollup query mentions it must be plain identifiers. `GROUPING(` followed by
    /// anything that opens another paren is the parse error this shipped with.
    fn assert_no_expression_in_grouping(sql: &str) {
        assert!(
            !sql.contains("GROUPING(date_trunc") && !sql.contains("GROUPING(EXTRACT"),
            "GROUPING() must take a column reference, never an expression:\n{sql}"
        );
        for (i, _) in sql.match_indices("GROUPING(") {
            let rest = &sql[i + "GROUPING(".len()..];
            let arg: String = rest.chars().take_while(|c| *c != ')').collect();
            assert!(
                !arg.contains('('),
                "GROUPING() argument is an expression, not a column: {arg}\n{sql}"
            );
        }
    }

    #[test]
    fn build_rollup_with_truncation_unit_groups_on_a_plain_identifier() {
        for engine in [Engine::Trino, Engine::StarRocks] {
            let sql = build(
                engine,
                &phys(),
                &temporal_cols(),
                &[metric("revenue", "SUM(price)")],
                &pivot_spec(Some("month")),
            )
            .unwrap();
            assert_no_expression_in_grouping(&sql);
            // the bucket is computed once, in the derived table, and named
            assert!(sql.contains("AS __dim_1"), "{sql}");
            assert!(sql.contains("date_trunc('month'"), "{sql}");
            assert!(sql.contains("GROUPING(__dim_1) AS __grp_1"), "{sql}");
            // and the result still speaks the user's column name (quoting differs per engine)
            assert!(
                sql.contains("__dim_1 AS ") && sql.contains("created"),
                "{sql}"
            );
        }
    }

    #[test]
    fn build_rollup_with_extraction_unit_groups_on_a_plain_identifier() {
        // `day_of_week` is an extraction, not a truncation — a different expression shape, and it
        // failed the same way, which is what showed the bug was about expressions in general.
        for engine in [Engine::Trino, Engine::StarRocks] {
            let sql = build(
                engine,
                &phys(),
                &temporal_cols(),
                &[metric("revenue", "SUM(price)")],
                &pivot_spec(Some("day_of_week")),
            )
            .unwrap();
            assert_no_expression_in_grouping(&sql);
            assert!(sql.contains("GROUPING(__dim_1) AS __grp_1"), "{sql}");
        }
    }

    #[test]
    fn build_rollup_without_a_unit_is_unaffected() {
        let sql = build(
            Engine::Trino,
            &phys(),
            &temporal_cols(),
            &[metric("revenue", "SUM(price)")],
            &pivot_spec(None),
        )
        .unwrap();
        assert_no_expression_in_grouping(&sql);
        assert!(sql.contains("\"created\" AS __dim_1"), "{sql}");
    }

    #[test]
    fn build_without_rollup_is_not_wrapped_at_all() {
        // The non-rollup path is used by every other chart type. It must stay byte-identical, so
        // this fix cannot regress them.
        let mut tu = std::collections::HashMap::new();
        tu.insert("created".to_string(), "month".to_string());
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["created".into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![],
            temporal_units: tu,
        };
        let sql = build(
            Engine::Trino,
            &phys(),
            &temporal_cols(),
            &[metric("revenue", "SUM(price)")],
            &spec,
        )
        .unwrap();
        assert!(!sql.contains("__dim_"), "{sql}");
        assert!(!sql.contains("__b"), "{sql}");
        assert!(sql.contains("GROUP BY date_trunc('month'"), "{sql}");
    }

    /// Every drill on a time dimension sent `col = '2026-01-01'` and every engine refused it with
    /// "Cannot apply operator: date = varchar(10)" — a 502 from clicking any cell on any chart
    /// with a date on the axis. Live-verified before the fix, on a plain filter as well as a
    /// pivot drill, so this was never pivot-specific.
    #[test]
    fn temporal_filter_literals_are_typed() {
        let mut date_col = col("d", "datetime", None);
        date_col.is_temporal = true;
        date_col.data_type = "date".into();
        let mut ts_col = col("ts", "datetime", None);
        ts_col.is_temporal = true;
        ts_col.data_type = "timestamp(3)".into();

        let spec = |c: &str| QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec![],
            columns: vec![],
            filters: vec![Filter {
                col: c.into(),
                op: FilterOp::Eq,
                value: serde_json::json!("2026-01-01"),
            }],
            orderby: vec![],
            row_limit: 10,
            rollup_groups: vec![],
            temporal_units: std::collections::HashMap::new(),
        };
        let cols = vec![date_col, ts_col];
        let metrics = [metric("revenue", "SUM(price)")];

        let d = build(Engine::Trino, &phys(), &cols, &metrics, &spec("d")).unwrap();
        assert!(d.contains("CAST('2026-01-01' AS DATE)"), "{d}");

        // A timestamp is DATETIME on StarRocks and TIMESTAMP on Trino.
        let t_trino = build(Engine::Trino, &phys(), &cols, &metrics, &spec("ts")).unwrap();
        assert!(
            t_trino.contains("CAST('2026-01-01' AS TIMESTAMP)"),
            "{t_trino}"
        );
        let t_sr = build(Engine::StarRocks, &phys(), &cols, &metrics, &spec("ts")).unwrap();
        assert!(t_sr.contains("CAST('2026-01-01' AS DATETIME)"), "{t_sr}");
    }

    #[test]
    fn non_temporal_filters_are_left_alone() {
        // The cast must not leak onto ordinary string columns.
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec![],
            columns: vec![],
            filters: vec![Filter {
                col: "region".into(),
                op: FilterOp::Eq,
                value: serde_json::json!("EU"),
            }],
            orderby: vec![],
            row_limit: 10,
            rollup_groups: vec![],
            temporal_units: std::collections::HashMap::new(),
        };
        let sql = build(
            Engine::Trino,
            &phys(),
            &[col("region", "string", None)],
            &[metric("revenue", "SUM(price)")],
            &spec,
        )
        .unwrap();
        assert!(sql.contains("\"region\" = 'EU'"), "{sql}");
        assert!(!sql.contains("CAST("), "{sql}");
    }

    #[test]
    fn build_rejects_a_dimension_outside_every_rollup_group() {
        // Live-verified as invalid regardless of temporal units — the engine returns
        // "'__dim_1' must be an aggregate expression or appear in GROUP BY", naming an internal
        // alias. Caught here instead, naming the caller's dimension.
        let mut spec = pivot_spec(None);
        spec.rollup_groups = vec![vec!["region".into()]];
        let err = build(
            Engine::Trino,
            &phys(),
            &temporal_cols(),
            &[metric("revenue", "SUM(price)")],
            &spec,
        )
        .unwrap_err();
        assert!(
            format!("{err:?}").contains("not in any rollup group"),
            "{err:?}"
        );
    }

    #[test]
    fn build_rollup_with_a_filter_keeps_it_on_the_raw_column() {
        // A filter must match the real timestamp, never the truncated group key. The derived table
        // carries the raw columns through with `*`, so the predicate still resolves against them.
        let mut spec = pivot_spec(Some("month"));
        spec.filters = vec![Filter {
            col: "region".into(),
            op: FilterOp::Eq,
            value: serde_json::json!("EU"),
        }];
        let sql = build(
            Engine::Trino,
            &phys(),
            &temporal_cols(),
            &[metric("revenue", "SUM(price)")],
            &spec,
        )
        .unwrap();
        assert!(sql.contains("WHERE \"region\" = 'EU'"), "{sql}");
        assert_no_expression_in_grouping(&sql);
    }

    #[test]
    fn build_raw_records_selects_columns_no_group_by() {
        let cols = vec![col("id", "numeric", None), col("name", "string", None)];
        let spec = QuerySpec {
            metrics: vec![],
            dimensions: vec![],
            columns: vec!["id".into(), "name".into()],
            filters: vec![],
            orderby: vec![OrderBy {
                field: "id".into(),
                desc: true,
            }],
            row_limit: 50,
            rollup_groups: vec![],
            temporal_units: std::collections::HashMap::new(),
        };
        let sql = build(Engine::Trino, &phys(), &cols, &[], &spec).unwrap();
        assert!(sql.contains("\"id\" AS \"id\""), "{sql}");
        assert!(!sql.contains("GROUP BY"), "{sql}");
        assert!(sql.contains("ORDER BY \"id\" DESC"), "{sql}");
    }

    #[test]
    fn build_filters_use_where_and_escape() {
        let cols = vec![col("region", "string", None), col("qty", "numeric", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into()],
            columns: vec![],
            filters: vec![
                Filter {
                    col: "region".into(),
                    op: FilterOp::Eq,
                    value: serde_json::json!("EU"),
                },
                Filter {
                    col: "qty".into(),
                    op: FilterOp::Gt,
                    value: serde_json::json!(10),
                },
                Filter {
                    col: "region".into(),
                    op: FilterOp::In,
                    value: serde_json::json!(["A", "B"]),
                },
                Filter {
                    col: "qty".into(),
                    op: FilterOp::IsNotNull,
                    value: serde_json::Value::Null,
                },
            ],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![],
            temporal_units: std::collections::HashMap::new(),
        };
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        assert!(sql.contains("\"region\" = 'EU'"), "{sql}");
        assert!(sql.contains("\"qty\" > 10"), "{sql}");
        assert!(sql.contains("\"region\" IN ('A', 'B')"), "{sql}");
        assert!(sql.contains("\"qty\" IS NOT NULL"), "{sql}");
    }

    #[test]
    fn not_like_filter_renders_not_like() {
        let cols = vec![col("region", "string", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into()],
            columns: vec![],
            filters: vec![Filter {
                col: "region".into(),
                op: FilterOp::NotLike,
                value: serde_json::json!("%spam%"),
            }],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![],
            temporal_units: std::collections::HashMap::new(),
        };
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        assert!(sql.contains("\"region\" NOT LIKE '%spam%'"), "{sql}");
    }

    #[test]
    fn build_rejects_unknown_names() {
        let cols = vec![col("region", "string", None)];
        let spec = QuerySpec {
            metrics: vec!["nope".into()],
            dimensions: vec![],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 10,
            rollup_groups: vec![],
            temporal_units: std::collections::HashMap::new(),
        };
        assert!(build(Engine::Trino, &phys(), &cols, &[], &spec).is_err());
    }

    #[test]
    fn build_aggregate_starrocks_uses_backticks() {
        let cols = vec![col("region", "string", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let src = SourceSpec::Physical {
            catalog: None,
            schema: "s".into(),
            table: "t".into(),
        };
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![],
            temporal_units: std::collections::HashMap::new(),
        };
        let sql = build(Engine::StarRocks, &src, &cols, &metrics, &spec).unwrap();
        assert!(sql.contains("`region` AS `region`"), "{sql}");
        assert!(sql.contains("GROUP BY `region`"), "{sql}");
        assert!(sql.contains("FROM `s`.`t`"), "{sql}");
    }

    #[test]
    fn build_two_rollups_for_pivot() {
        let cols = vec![
            col("region", "string", None),
            col("nation", "string", None),
            col("seg", "string", None),
        ];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into(), "nation".into(), "seg".into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![vec!["region".into(), "nation".into()], vec!["seg".into()]],
            temporal_units: std::collections::HashMap::new(),
        };
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        // ROLLUP(region, nation) × ROLLUP(seg) → portable GROUPING SETS.
        assert!(
            sql.contains(
                "GROUP BY GROUPING SETS ((__dim_0, __dim_1, __dim_2), (__dim_0, __dim_1), (__dim_0, __dim_2), (__dim_0), (__dim_2), ())"
            ),
            "{sql}"
        );
        assert!(sql.contains("GROUPING(__dim_0) AS __grp_0"), "{sql}");
        assert!(sql.contains("GROUPING(__dim_1) AS __grp_1"), "{sql}");
        assert!(sql.contains("GROUPING(__dim_2) AS __grp_2"), "{sql}");
    }

    #[test]
    fn build_rejects_rollup_name_not_in_dimensions() {
        let cols = vec![col("region", "string", None), col("seg", "string", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into()], // seg is NOT a dimension
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 10,
            rollup_groups: vec![vec!["region".into()], vec!["seg".into()]],
            temporal_units: std::collections::HashMap::new(),
        };
        assert!(build(Engine::Trino, &phys(), &cols, &metrics, &spec).is_err());
    }

    #[test]
    fn build_rejects_all_empty_rollup_groups() {
        let cols = vec![col("region", "string", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 10,
            rollup_groups: vec![vec![], vec![]],
            temporal_units: std::collections::HashMap::new(),
        };
        assert!(build(Engine::Trino, &phys(), &cols, &metrics, &spec).is_err());
    }

    #[test]
    fn temporal_expr_truncation_both_engines() {
        use TemporalUnit::*;
        assert_eq!(
            temporal_expr(Engine::Trino, "\"created_at\"", Month),
            "date_trunc('month', \"created_at\")"
        );
        assert_eq!(
            temporal_expr(Engine::StarRocks, "`created_at`", Quarter),
            "date_trunc('quarter', `created_at`)"
        );
        assert_eq!(
            temporal_expr(Engine::Trino, "x", Year),
            "date_trunc('year', x)"
        );
    }

    #[test]
    fn temporal_expr_extraction_normalized() {
        use TemporalUnit::*;
        // identical function on both engines
        assert_eq!(temporal_expr(Engine::Trino, "x", MonthOfYear), "month(x)");
        assert_eq!(
            temporal_expr(Engine::StarRocks, "x", MonthOfYear),
            "month(x)"
        );
        assert_eq!(temporal_expr(Engine::Trino, "x", HourOfDay), "hour(x)");
        // day_of_week: Trino native vs StarRocks normalized to Mon=1
        assert_eq!(
            temporal_expr(Engine::Trino, "x", DayOfWeek),
            "day_of_week(x)"
        );
        assert_eq!(
            temporal_expr(Engine::StarRocks, "x", DayOfWeek),
            "(weekday(x) + 1)"
        );
        // week_of_year: Trino native vs StarRocks ISO mode 3
        assert_eq!(
            temporal_expr(Engine::Trino, "x", WeekOfYear),
            "week_of_year(x)"
        );
        assert_eq!(
            temporal_expr(Engine::StarRocks, "x", WeekOfYear),
            "week(x, 3)"
        );
        // day_of_month / day_of_year naming differs
        assert_eq!(
            temporal_expr(Engine::Trino, "x", DayOfMonth),
            "day_of_month(x)"
        );
        assert_eq!(
            temporal_expr(Engine::StarRocks, "x", DayOfMonth),
            "dayofmonth(x)"
        );
        assert_eq!(
            temporal_expr(Engine::Trino, "x", DayOfYear),
            "day_of_year(x)"
        );
        assert_eq!(
            temporal_expr(Engine::StarRocks, "x", DayOfYear),
            "dayofyear(x)"
        );
    }

    #[test]
    fn empty_temporal_units_leaves_sql_unchanged() {
        let cols = vec![col("region", "string", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec!["region".into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![],
            temporal_units: std::collections::HashMap::new(),
        };
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        assert!(sql.contains("GROUP BY \"region\""), "{sql}");
        assert!(!sql.contains("date_trunc"), "{sql}");
    }

    fn tcol(name: &str) -> DatasetColumn {
        DatasetColumn {
            column_name: name.into(),
            data_type: "timestamp".into(),
            type_generic: "temporal".into(),
            is_temporal: true,
            ordinal: 0,
            expression: None,
            verbose_name: String::new(),
            description: String::new(),
            groupby: true,
            filterable: true,
            format: String::new(),
            semantic_type: String::new(),
        }
    }

    fn spec_with_unit(dim: &str, unit: &str) -> QuerySpec {
        let mut tu = std::collections::HashMap::new();
        tu.insert(dim.to_string(), unit.to_string());
        QuerySpec {
            metrics: vec!["revenue".into()],
            dimensions: vec![dim.into()],
            columns: vec![],
            filters: vec![],
            orderby: vec![],
            row_limit: 100,
            rollup_groups: vec![],
            temporal_units: tu,
        }
    }

    #[test]
    fn dimension_truncation_select_and_group_match() {
        let cols = vec![tcol("created_at")];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = spec_with_unit("created_at", "month");
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        assert!(
            sql.contains("date_trunc('month', \"created_at\") AS \"created_at\""),
            "{sql}"
        );
        assert!(
            sql.contains("GROUP BY date_trunc('month', \"created_at\")"),
            "{sql}"
        );
    }

    #[test]
    fn dimension_unit_on_non_temporal_is_rejected() {
        let cols = vec![col("region", "string", None)];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = spec_with_unit("region", "month");
        let err = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap_err();
        assert!(matches!(err, BiError::BadRequest(_)));
    }

    #[test]
    fn dimension_unknown_unit_is_rejected() {
        let cols = vec![tcol("created_at")];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let spec = spec_with_unit("created_at", "fortnight");
        let err = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap_err();
        assert!(matches!(err, BiError::BadRequest(_)));
    }

    #[test]
    fn temporal_unit_for_unknown_dimension_is_rejected() {
        let cols = vec![tcol("created_at")];
        let metrics = vec![metric("revenue", "SUM(price)")];
        // unit keyed to a column that is NOT in dimensions (e.g. a typo)
        let mut spec = spec_with_unit("created_at", "month");
        spec.dimensions = vec![]; // created_at is no longer a dimension
        let err = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap_err();
        assert!(matches!(err, BiError::BadRequest(_)));
    }

    #[test]
    fn orderby_on_grouped_temporal_dim_wraps_identically() {
        let cols = vec![tcol("created_at")];
        let metrics = vec![metric("revenue", "SUM(price)")];
        let mut spec = spec_with_unit("created_at", "month");
        spec.orderby = vec![OrderBy {
            field: "created_at".into(),
            desc: false,
        }];
        let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
        assert!(
            sql.contains("GROUP BY date_trunc('month', \"created_at\")"),
            "{sql}"
        );
        assert!(
            sql.contains("ORDER BY date_trunc('month', \"created_at\") ASC"),
            "{sql}"
        );
    }
}
