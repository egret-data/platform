//! Numbers in `csv`/`text` reports, written the way the chart table shows them
//! (KI-169).
//!
//! The engine hands numbers over as JSON numbers whose text is the engine's own
//! (`arbitrary_precision` is on in the server build), so a `double` arrived in
//! the mail as `1.0356810234900002e+9` — float noise in scientific notation.
//! The table (`frontend/src/components/chart/format.ts`, `formatNumber` with no
//! column format) shows the same cell as `1,035,681,023.49`: at most three
//! decimals, trailing zeros dropped, digits grouped.
//!
//! - **Text** follows the table exactly.
//! - **CSV** rounds the same way but does NOT group: a CSV is read by a
//!   spreadsheet, and a `,`-grouped number is text to every locale whose
//!   decimal separator is not `.`.
//!
//! A column's own format settings (decimals, currency, prefix…) are not applied:
//! that is the chart's display layer, and only the defaults are mirrored here.

use egret_core::tabular::ColumnMeta;
use serde_json::Value;

/// How a report writes a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberStyle {
    /// Rounded, not grouped: `1035681023.49`.
    Csv,
    /// Rounded and grouped, as the table shows it: `1,035,681,023.49`.
    Text,
}

/// The table's default: at most three decimals (`maximumFractionDigits: 3`).
const MAX_DECIMALS: usize = 3;

/// The table's `isNumericType` (`format-types.ts`): a substring match on the
/// engine's type name, so a `decimal(12,2)` that the engine sends as a string
/// is formatted like a `double`.
fn is_numeric_type(ty: &str) -> bool {
    let t = ty.to_ascii_lowercase();
    [
        "int", "double", "decimal", "real", "numeric", "float", "bigint",
    ]
    .iter()
    .any(|k| t.contains(k))
}

/// Every cell of `rows`, numbers formatted for `style`; everything else as is.
pub fn format_rows(
    columns: &[ColumnMeta],
    rows: &[Vec<Value>],
    style: NumberStyle,
) -> Vec<Vec<Value>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, cell)| {
                    let numeric_column = columns.get(i).is_some_and(|c| is_numeric_type(&c.ty));
                    match format_number(cell, numeric_column, style) {
                        Some(text) => Value::String(text),
                        None => cell.clone(),
                    }
                })
                .collect()
        })
        .collect()
}

/// `Some(text)` when `cell` is a number (or a numeric string in a numeric
/// column, as `formatValue` treats one); `None` leaves the cell alone.
fn format_number(cell: &Value, numeric_column: bool, style: NumberStyle) -> Option<String> {
    let text = match cell {
        Value::Number(n) => n.to_string(),
        Value::String(s) if numeric_column && !s.trim().is_empty() => s.trim().to_string(),
        _ => return None,
    };
    let plain = plain_decimal(&text)?;
    Some(match style {
        NumberStyle::Csv => plain,
        NumberStyle::Text => group_thousands(&plain),
    })
}

/// `text` as a plain decimal rounded to `MAX_DECIMALS`, trailing zeros dropped.
/// An integer keeps every digit — a `bigint` id is not a float and must not be
/// rounded through one. `None` when `text` is not a finite number.
fn plain_decimal(text: &str) -> Option<String> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
        let trimmed = digits.trim_start_matches('0');
        let int = if trimmed.is_empty() { "0" } else { trimmed };
        let neg = text.starts_with('-') && int != "0";
        return Some(format!("{}{int}", if neg { "-" } else { "" }));
    }
    let f: f64 = text.parse().ok()?;
    if !f.is_finite() {
        return None;
    }
    let mut out = format!("{f:.MAX_DECIMALS$}");
    if out.contains('.') {
        out.truncate(out.trim_end_matches('0').trim_end_matches('.').len());
    }
    if out == "-0" {
        out = "0".into();
    }
    Some(out)
}

/// `1035681023.49` → `1,035,681,023.49`, as `Intl.NumberFormat("en")` groups.
fn group_thousands(plain: &str) -> String {
    let (sign, rest) = match plain.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", plain),
    };
    let (int, frac) = match rest.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (rest, None),
    };
    let mut grouped = String::with_capacity(int.len() + int.len() / 3);
    for (i, ch) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn col(name: &str, ty: &str) -> ColumnMeta {
        ColumnMeta {
            name: name.into(),
            ty: ty.into(),
        }
    }

    /// The number exactly as the engine's text spelled it — what the server
    /// build holds — rather than as `json!` would re-spell an `f64`.
    fn engine_number(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn a_double_is_written_as_the_table_shows_it() {
        // The KI-169 rows: `revenue` came out as float noise.
        let columns = [col("orderstatus", "varchar"), col("revenue", "double")];
        let rows = vec![
            vec![json!("F"), engine_number("1.0356810234900002e+9")],
            vec![json!("P"), engine_number("6.333947531999999e+7")],
        ];
        let text = format_rows(&columns, &rows, NumberStyle::Text);
        assert_eq!(text[0], vec![json!("F"), json!("1,035,681,023.49")]);
        assert_eq!(text[1], vec![json!("P"), json!("63,339,475.32")]);

        let csv = format_rows(&columns, &rows, NumberStyle::Csv);
        assert_eq!(csv[0], vec![json!("F"), json!("1035681023.49")]);
        assert_eq!(csv[1], vec![json!("P"), json!("63339475.32")]);
    }

    #[test]
    fn rounding_keeps_at_most_three_decimals_and_drops_trailing_zeros() {
        for (input, want) in [
            ("0.1234567", "0.123"),
            ("2.5", "2.5"),
            ("2.0", "2"),
            ("-0.0001", "0"),
            ("-1234.5678", "-1,234.568"),
            ("1e21", "1,000,000,000,000,000,000,000"),
        ] {
            let got = format_number(&engine_number(input), true, NumberStyle::Text);
            assert_eq!(got.as_deref(), Some(want), "{input}");
        }
    }

    #[test]
    fn an_integer_keeps_every_digit() {
        // A bigint beyond 2^53 must not be rounded through an f64.
        let big = engine_number("9007199254740993");
        assert_eq!(
            format_number(&big, true, NumberStyle::Csv).as_deref(),
            Some("9007199254740993")
        );
        assert_eq!(
            format_number(&big, true, NumberStyle::Text).as_deref(),
            Some("9,007,199,254,740,993")
        );
        assert_eq!(
            format_number(&json!(-42), true, NumberStyle::Text).as_deref(),
            Some("-42")
        );
    }

    #[test]
    fn a_decimal_sent_as_a_string_is_formatted_only_in_a_numeric_column() {
        let columns = [col("amount", "decimal(12,2)"), col("code", "varchar")];
        let rows = vec![vec![json!("1234567.50"), json!("00123")]];
        let out = format_rows(&columns, &rows, NumberStyle::Text);
        assert_eq!(out[0], vec![json!("1,234,567.5"), json!("00123")]);
    }

    #[test]
    fn non_numbers_pass_through() {
        let columns = [col("a", "varchar"), col("b", "boolean"), col("c", "double")];
        let rows = vec![vec![json!("x"), json!(true), Value::Null]];
        assert_eq!(format_rows(&columns, &rows, NumberStyle::Csv), rows);
    }
}
