//! Pure chart query builder: QuerySpec → SQL. No I/O. Filter values are the only
//! untrusted input and pass through `sql_literal`; identifiers/expressions are
//! dataset-sourced and trusted.

use egret_database::model::Engine;
use serde::Deserialize;

use crate::dataset::{DatasetColumn, Metric, SourceSpec};
use crate::error::BiError;
use crate::expr::source_ref_for;
use crate::schema::quote_ident;

/// Hard ceiling on rows a chart query may request.
pub const MAX_CHART_ROWS: i64 = 10_000;

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
pub struct Filter {
    pub col: String,
    pub op: FilterOp,
    #[serde(default)]
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OrderBy {
    pub field: String,
    #[serde(default)]
    pub desc: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
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
        // extraction variants — implemented in Task 4
        _ => extract_expr(engine, inner, unit),
    }
}

fn extract_expr(_engine: Engine, inner: &str, _unit: TemporalUnit) -> String {
    inner.to_string()
}

fn find_column<'a>(columns: &'a [DatasetColumn], name: &str) -> Option<&'a DatasetColumn> {
    columns.iter().find(|c| c.column_name == name)
}
fn find_metric<'a>(metrics: &'a [Metric], name: &str) -> Option<&'a Metric> {
    metrics.iter().find(|m| m.metric_name == name)
}

/// Render one column filter into a WHERE predicate. Metric filters are rejected (→ HAVING,
/// deferred to 3c-3.2). The operator comes from an enum, so no operator injection is possible.
fn render_filter(engine: Engine, columns: &[DatasetColumn], f: &Filter) -> Result<String, BiError> {
    let c = find_column(columns, &f.col)
        .ok_or_else(|| BiError::BadRequest(format!("unknown filter column: {}", f.col)))?;
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
                .map(|v| sql_literal(v, tg))
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
            let lit = sql_literal(&f.value, tg)?;
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

/// Resolve an ORDER BY field (a metric or a column name) to its SQL expression.
fn orderby_sql(
    engine: Engine,
    columns: &[DatasetColumn],
    metrics: &[Metric],
    field: &str,
) -> Result<String, BiError> {
    if let Some(m) = find_metric(metrics, field) {
        return Ok(m.expression.clone());
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

    if aggregate {
        if !spec.rollup_groups.is_empty() && spec.dimensions.is_empty() {
            return Err(BiError::BadRequest(
                "rollup requires at least one dimension".into(),
            ));
        }
        // Resolve every dimension to its expression once; remember each name's position.
        let mut dim_exprs: Vec<String> = Vec::new();
        let mut dim_index: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for (i, name) in spec.dimensions.iter().enumerate() {
            let c = find_column(columns, name)
                .ok_or_else(|| BiError::BadRequest(format!("unknown dimension: {name}")))?;
            let e = column_sql(engine, c);
            select.push(format!("{e} AS {}", quote_ident(engine, name)));
            dim_index.insert(name.as_str(), i);
            dim_exprs.push(e);
        }
        for name in &spec.metrics {
            let m = find_metric(metrics, name)
                .ok_or_else(|| BiError::BadRequest(format!("unknown metric: {name}")))?;
            select.push(format!("{} AS {}", m.expression, quote_ident(engine, name)));
        }
        let has_rollup = !spec.rollup_groups.is_empty();
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
        push_orderby(&mut sql, engine, columns, metrics, &spec.orderby)?;
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
        push_orderby(&mut sql, engine, columns, metrics, &spec.orderby)?;
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
    orderby: &[OrderBy],
) -> Result<(), BiError> {
    if orderby.is_empty() {
        return Ok(());
    }
    let parts = orderby
        .iter()
        .map(|o| {
            let e = orderby_sql(engine, columns, metrics, &o.field)?;
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
        assert!(Year.is_extract() == false);
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
        // (works on both Trino and StarRocks).
        assert!(
            sql.contains(
                "GROUP BY GROUPING SETS ((\"region\", \"seg\"), (\"region\"), (\"seg\"), ())"
            ),
            "{sql}"
        );
        assert!(sql.contains("GROUPING(\"region\") AS __grp_0"), "{sql}");
        assert!(sql.contains("GROUPING(\"seg\") AS __grp_1"), "{sql}");
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
                "GROUP BY GROUPING SETS ((\"region\", \"nation\", \"seg\"), (\"region\", \"nation\"), (\"region\", \"seg\"), (\"region\"), (\"seg\"), ())"
            ),
            "{sql}"
        );
        assert!(sql.contains("GROUPING(\"region\") AS __grp_0"), "{sql}");
        assert!(sql.contains("GROUPING(\"nation\") AS __grp_1"), "{sql}");
        assert!(sql.contains("GROUPING(\"seg\") AS __grp_2"), "{sql}");
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
}
