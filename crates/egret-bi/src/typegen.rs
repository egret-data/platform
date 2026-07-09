//! Pure classification of an engine column type into a generic class.
//! This is the single source of truth charts rely on to tell dimensions,
//! measures, and time columns apart.

use egret_query::registry::model::Engine;

/// The generic class of a column, independent of engine-specific type spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericType {
    String,
    Numeric,
    Temporal,
    Boolean,
}

impl GenericType {
    pub fn as_str(self) -> &'static str {
        match self {
            GenericType::String => "string",
            GenericType::Numeric => "numeric",
            GenericType::Temporal => "temporal",
            GenericType::Boolean => "boolean",
        }
    }

    /// Whether this column is a time column (drives time-series chart axes).
    pub fn is_temporal(self) -> bool {
        matches!(self, GenericType::Temporal)
    }
}

/// Classify a raw engine type string (e.g. "varchar(20)", "decimal(10,2)",
/// "timestamp with time zone") into a `GenericType`. Matching is
/// case-insensitive on the leading keyword; precision/length suffixes are
/// ignored. Unknown types fall back to `String` (safe — usable as a dimension).
pub fn generic_type(engine: Engine, raw_type: &str) -> GenericType {
    let head = type_head(raw_type);
    match engine {
        Engine::Trino => classify_trino(&head),
        Engine::StarRocks => classify_starrocks(&head),
    }
}

/// Lowercased type minus any "(...)" precision/length suffix (e.g.
/// "VARCHAR(20)" -> "varchar", "timestamp(3) with time zone" -> "timestamp with
/// time zone"). The per-engine classifiers then key off the leading keyword.
fn type_head(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    let no_paren = lowered.split('(').next().unwrap_or("").trim();
    no_paren.to_string()
}

fn classify_trino(head: &str) -> GenericType {
    let first = head.split_whitespace().next().unwrap_or("");
    match first {
        "tinyint" | "smallint" | "integer" | "int" | "bigint" | "real" | "double" | "decimal" => {
            GenericType::Numeric
        }
        "date" | "time" | "timestamp" => GenericType::Temporal,
        "boolean" => GenericType::Boolean,
        _ => GenericType::String,
    }
}

// Known physical-vs-virtual vocabulary asymmetry: the MySQL wire protocol used
// for virtual datasets cannot distinguish StarRocks `BOOLEAN` from `TINYINT`
// (both arrive as MYSQL_TYPE_TINY), and `LARGEINT` has no protocol type. So a
// `BOOLEAN` column classifies as Boolean via DESCRIBE (physical) but Numeric via
// describe() (virtual); a `LARGEINT` is Numeric physical but falls to String
// virtual. This is protocol-inherent, not a coding bug; documented as a v1 edge.
fn classify_starrocks(head: &str) -> GenericType {
    let first = head.split_whitespace().next().unwrap_or("");
    match first {
        "tinyint" | "smallint" | "int" | "bigint" | "largeint" | "float" | "double" | "decimal"
        | "decimalv2" | "decimalv3" => GenericType::Numeric,
        "date" | "datetime" | "time" => GenericType::Temporal,
        "boolean" => GenericType::Boolean,
        _ => GenericType::String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trino_numeric_variants() {
        for t in [
            "bigint",
            "INTEGER",
            "double",
            "decimal(10,2)",
            "real",
            "tinyint",
        ] {
            assert_eq!(generic_type(Engine::Trino, t), GenericType::Numeric, "{t}");
        }
    }

    #[test]
    fn trino_temporal_including_tz() {
        assert_eq!(generic_type(Engine::Trino, "date"), GenericType::Temporal);
        assert_eq!(
            generic_type(Engine::Trino, "timestamp(3) with time zone"),
            GenericType::Temporal
        );
        assert!(generic_type(Engine::Trino, "timestamp").is_temporal());
    }

    #[test]
    fn trino_string_and_boolean() {
        assert_eq!(
            generic_type(Engine::Trino, "varchar(255)"),
            GenericType::String
        );
        assert_eq!(generic_type(Engine::Trino, "json"), GenericType::String);
        assert_eq!(generic_type(Engine::Trino, "boolean"), GenericType::Boolean);
    }

    #[test]
    fn starrocks_variants() {
        assert_eq!(generic_type(Engine::StarRocks, "int"), GenericType::Numeric);
        assert_eq!(
            generic_type(Engine::StarRocks, "largeint"),
            GenericType::Numeric
        );
        assert_eq!(
            generic_type(Engine::StarRocks, "decimalv2(10,2)"),
            GenericType::Numeric
        );
        assert_eq!(
            generic_type(Engine::StarRocks, "decimalv3"),
            GenericType::Numeric
        );
        assert_eq!(
            generic_type(Engine::StarRocks, "DATETIME"),
            GenericType::Temporal
        );
        assert_eq!(
            generic_type(Engine::StarRocks, "varchar(20)"),
            GenericType::String
        );
        assert_eq!(
            generic_type(Engine::StarRocks, "boolean"),
            GenericType::Boolean
        );
    }

    #[test]
    fn unknown_falls_back_to_string() {
        assert_eq!(
            generic_type(Engine::Trino, "hyperloglog"),
            GenericType::String
        );
        assert_eq!(
            generic_type(Engine::StarRocks, "bitmap"),
            GenericType::String
        );
        assert_eq!(generic_type(Engine::Trino, ""), GenericType::String);
    }

    #[test]
    fn as_str_values() {
        assert_eq!(GenericType::Numeric.as_str(), "numeric");
        assert_eq!(GenericType::Temporal.as_str(), "temporal");
    }
}
