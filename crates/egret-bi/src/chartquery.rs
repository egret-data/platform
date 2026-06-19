//! Pure chart query builder: QuerySpec → SQL. No I/O. Filter values are the only
//! untrusted input and pass through `sql_literal`; identifiers/expressions are
//! dataset-sourced and trusted.

// Types and helpers here are scaffolding used by chartquery::build in later tasks.
#![allow(dead_code)]

use serde::Deserialize;

use crate::error::BiError;

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
    pub rollup: bool,
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
    In,
    NotIn,
    IsNull,
    IsNotNull,
}

/// Encode a filter value as a SQL literal, validated by the target column's generic type.
/// Numerics are validated and emitted unquoted; booleans normalized; everything else is
/// single-quoted with `'` doubled. This is the security-critical primitive.
pub(crate) fn sql_literal(v: &serde_json::Value, type_generic: &str) -> Result<String, BiError> {
    match type_generic {
        "numeric" => {
            if let Some(n) = v.as_f64() {
                Ok(n.to_string())
            } else if let Some(s) = v.as_str() {
                s.parse::<f64>()
                    .map(|n| n.to_string())
                    .map_err(|_| BiError::BadRequest(format!("not a number: {s}")))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
