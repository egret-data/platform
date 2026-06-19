//! Validate a metric/calculated-column expression by probing the engine, and infer its type.

use std::time::Duration;

use egret_database::model::Engine;
use egret_engine::connector::{build_connector, ConnSpec};

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

/// Build the `FROM <source>` reference for the probe.
fn source_ref(spec: &ConnSpec, source: &SourceSpec) -> String {
    match source {
        SourceSpec::Physical {
            catalog,
            schema,
            table,
        } => match spec.engine {
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
        source_ref(spec, source)
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
}
