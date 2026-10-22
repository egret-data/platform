//! Validate a metric/calculated-column expression by probing the engine, and infer its type.

use std::time::Duration;

use egret_query::execution::connector::{build_connector, ConnSpec};
use egret_query_api::Engine;

use crate::dataset::SourceSpec;
use crate::error::BiError;
use crate::schema::{quote_mysql, quote_trino};
use crate::typegen::generic_type;

/// The inferred result type of a validated expression.
#[derive(Debug, Clone)]
pub struct DescribedExpr {
    pub data_type: String,
    pub type_generic: String,
    pub is_temporal: bool,
}

/// Reject empty or multi-statement expressions. A `;` outside string literals would let the
/// expression smuggle a second statement into the probe; one statement only.
fn guard_single_statement(expr: &str) -> Result<(), BiError> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err(BiError::BadRequest("expression is empty".into()));
    }
    let mut in_str = false; // inside a '...' string literal
    let mut in_id = false; // inside a "..." quoted identifier
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_id => {
                if in_str && chars.peek() == Some(&'\'') {
                    chars.next(); // escaped '' inside a string literal
                } else {
                    in_str = !in_str;
                }
            }
            '"' if !in_str => {
                if in_id && chars.peek() == Some(&'"') {
                    chars.next(); // escaped "" inside a quoted identifier
                } else {
                    in_id = !in_id;
                }
            }
            ';' if !in_str && !in_id => {
                return Err(BiError::BadRequest(
                    "expression must be a single statement".into(),
                ));
            }
            _ => {}
        }
    }
    if in_str || in_id {
        return Err(BiError::BadRequest(
            "unterminated quoted string or identifier in expression".into(),
        ));
    }
    Ok(())
}

/// Build the `FROM <source>` reference for `engine`.
pub(crate) fn source_ref_for(engine: Engine, source: &SourceSpec) -> String {
    match source {
        SourceSpec::Physical {
            catalog,
            schema,
            table,
        } => match engine {
            Engine::Trino => match catalog.as_deref() {
                Some(cat) if !cat.is_empty() => format!(
                    "{}.{}.{}",
                    quote_trino(cat),
                    quote_trino(schema),
                    quote_trino(table)
                ),
                // No catalog: qualify with schema.table (a quoted empty catalog is invalid SQL).
                _ => format!("{}.{}", quote_trino(schema), quote_trino(table)),
            },
            Engine::StarRocks => format!("{}.{}", quote_mysql(schema), quote_mysql(table)),
        },
        SourceSpec::Virtual { sql } => format!("(\n{sql}\n) AS _egret_src"),
    }
}

/// Validate `expr` against `source` on `spec`'s engine and return its inferred type.
/// Reuses `Connector::describe`, which wraps the probe in `SELECT * FROM (...) LIMIT 0`.
pub async fn validate_expression(
    spec: &ConnSpec,
    source: &SourceSpec,
    expr: &str,
) -> Result<DescribedExpr, BiError> {
    guard_single_statement(expr)?;
    let probe = format!(
        "SELECT {expr} AS _egret_expr FROM {}",
        source_ref_for(spec.engine, source)
    );
    let cols = build_connector(spec)
        .describe(&probe, Duration::from_secs(15))
        .await
        .map_err(BiError::from)?;
    let data_type = cols
        .into_iter()
        .next()
        .map(|c| c.ty)
        .ok_or_else(|| BiError::Engine("expression produced no column".into()))?;
    let g = generic_type(spec.engine, &data_type);
    Ok(DescribedExpr {
        data_type,
        type_generic: g.as_str().to_string(),
        is_temporal: g.is_temporal(),
    })
}

/// The probe that tells an aggregate from a row-level expression: beside `count(*)` and with no
/// GROUP BY, every engine refuses a select item that is neither an aggregate nor a constant
/// (`'totalprice' must be an aggregate expression or appear in GROUP BY clause`). That is exactly
/// the condition a chart puts a metric in, so it fails here instead of inside the first chart.
fn aggregate_probe(engine: Engine, source: &SourceSpec, expr: &str) -> String {
    format!(
        "SELECT count(*) AS _egret_n, {expr} AS _egret_expr FROM {}",
        source_ref_for(engine, source)
    )
}

/// [`validate_expression`] for a METRIC: the expression must also be an aggregate. The plain
/// probe runs first so a typo is reported as the engine's own error; only an expression that is
/// valid on its own and fails in an aggregate query gets the aggregate message.
pub async fn validate_metric_expression(
    spec: &ConnSpec,
    source: &SourceSpec,
    expr: &str,
) -> Result<DescribedExpr, BiError> {
    let described = validate_expression(spec, source, expr).await?;
    build_connector(spec)
        .describe(
            &aggregate_probe(spec.engine, source, expr),
            Duration::from_secs(15),
        )
        .await
        .map_err(|e| {
            BiError::BadRequest(format!(
                "metric expression must be an aggregate, such as SUM(column) or COUNT(*): {e}"
            ))
        })?;
    Ok(described)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_accepts_normal_expressions() {
        for e in [
            "SUM(x)",
            "price * qty",
            "EXTRACT(year FROM ts)",
            "concat(a, ';', b)",
        ] {
            assert!(guard_single_statement(e).is_ok(), "{e}");
        }
    }

    #[test]
    fn guard_rejects_multi_statement_and_empty() {
        assert!(guard_single_statement("").is_err());
        assert!(guard_single_statement("   ").is_err());
        assert!(guard_single_statement("1; DROP TABLE t").is_err());
        assert!(guard_single_statement("concat(a, ';', b)").is_ok());
        // `;` inside a double-quoted identifier is allowed; unterminated quotes rejected.
        assert!(guard_single_statement("\"col;name\" + 1").is_ok());
        assert!(guard_single_statement("'abc").is_err());
        assert!(guard_single_statement("\"abc").is_err());
    }

    #[test]
    fn the_aggregate_probe_puts_the_expression_beside_an_aggregate_without_group_by() {
        let src = SourceSpec::Physical {
            catalog: Some("tpch".into()),
            schema: "tiny".into(),
            table: "orders".into(),
        };
        let sql = aggregate_probe(Engine::Trino, &src, "totalprice");
        assert_eq!(
            sql,
            "SELECT count(*) AS _egret_n, totalprice AS _egret_expr FROM \"tpch\".\"tiny\".\"orders\""
        );
        assert!(!sql.to_ascii_lowercase().contains("group by"));
    }
}
