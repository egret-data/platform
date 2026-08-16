//! SQL Lab query templating: substitute `{{ name }}` placeholders with typed,
//! escaped SQL literals. This is deliberately NOT a general template engine —
//! only simple named-variable references are supported, and every value is
//! rendered as a validated literal (string → quoted + escaped per the target
//! engine's dialect, number / boolean → validated), so a parameter value can
//! never inject SQL.
//!
//! Escaping rules live in [`egret_query_api::dialect`], not here.
//!
//! # Safety model: this module INTERPOLATES, and that is now the older of two
//!
//! egret has three SQL-building surfaces and they do not share a safety model.
//!
//! * **The Data API BINDS** (`egret-dataapi/src/render.rs`). Caller values never
//!   enter the statement text at all: the renderer's formatter can only emit a
//!   `?`, and the value travels beside the SQL as a driver bind (StarRocks) or
//!   an `EXECUTE … USING` item (Trino). A value there cannot break out of its
//!   own position because it was never in a position to break out of. Shipped
//!   and live-checked in P4.5 — see
//!   `docs/en/specs/2026-08-15-data-api-parameters-design.md`.
//! * **This module interpolates**, escaping through
//!   [`egret_query_api::quote_string`]. Correctness depends on that escaper
//!   being right for the target dialect and on every value reaching it.
//! * **`egret-bi/src/chartquery.rs::sql_literal` interpolates** the same way,
//!   for chart and dashboard filter values.
//!
//! **This and `chartquery.rs` were both call sites of the P1 backslash bug**
//! (`\` unescaped while `'` was doubled, so `\'` broke out of the literal on
//! StarRocks; Trino unaffected). The escaper was fixed; the *model* was not, and
//! a model that has already failed once is worth naming where a reader of it
//! will see it.
//!
//! Note also that this renderer is **SQL Lab's and only SQL Lab's**. Nothing on
//! the Data API serving path has ever called it — comments in
//! `egret-dataapi/src/lifecycle.rs` used to say P3's validation matched "what
//! `template::render` will do at serve time", which was never true and is now
//! corrected there.
//!
//! Migrating both interpolating surfaces to binding is the larger remaining
//! prize and was deliberately left out of P4.5 (spec §8).

use super::model::QueryParam;
use crate::registry::model::Engine;
use egret_query_api::quote_string;

/// Render `{{ name }}` placeholders in `sql` using `params`, escaping string
/// values for `engine`'s dialect. Returns an error naming the first unknown
/// parameter or malformed placeholder. If `sql` has no placeholders it is
/// returned unchanged.
pub fn render(engine: Engine, sql: &str, params: &[QueryParam]) -> Result<String, String> {
    if !sql.contains("{{") {
        return Ok(sql.to_string());
    }
    let mut out = String::with_capacity(sql.len());
    let mut i = 0;
    while i < sql.len() {
        if i + 1 < sql.len() && &sql[i..i + 2] == "{{" {
            let end = sql[i + 2..]
                .find("}}")
                .ok_or_else(|| "unterminated '{{' in query template".to_string())?;
            let raw = &sql[i + 2..i + 2 + end];
            let name = raw.trim();
            if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(format!("invalid parameter placeholder: {{{{{raw}}}}}"));
            }
            let param = params
                .iter()
                .find(|p| p.name == name)
                .ok_or_else(|| format!("missing value for parameter: {name}"))?;
            out.push_str(&literal(engine, param)?);
            i += 2 + end + 2;
        } else {
            // Advance one UTF-8 char (indices stay on char boundaries).
            let ch_len = sql[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            out.push_str(&sql[i..i + ch_len]);
            i += ch_len;
        }
    }
    Ok(out)
}

/// Render a single parameter as a safe SQL literal for `engine`.
fn literal(engine: Engine, p: &QueryParam) -> Result<String, String> {
    match p.ty.as_str() {
        "number" => {
            let v = p.value.trim();
            v.parse::<f64>()
                .map_err(|_| format!("parameter '{}' is not a number: {}", p.name, p.value))?;
            Ok(v.to_string())
        }
        "boolean" => match p.value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok("true".to_string()),
            "false" | "0" => Ok("false".to_string()),
            _ => Err(format!(
                "parameter '{}' is not a boolean: {}",
                p.name, p.value
            )),
        },
        // Default (`string` or anything unrecognized): quote + escape per dialect.
        _ => Ok(quote_string(engine, &p.value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::model::Engine;

    fn p(name: &str, ty: &str, value: &str) -> QueryParam {
        QueryParam {
            name: name.into(),
            ty: ty.into(),
            value: value.into(),
        }
    }

    #[test]
    fn no_placeholders_is_identity() {
        assert_eq!(render(Engine::Trino, "SELECT 1", &[]).unwrap(), "SELECT 1");
    }

    #[test]
    fn string_param_is_quoted_and_escaped() {
        let sql = "SELECT * FROM t WHERE c = {{ c }}";
        assert_eq!(
            render(Engine::Trino, sql, &[p("c", "string", "US")]).unwrap(),
            "SELECT * FROM t WHERE c = 'US'"
        );
    }

    #[test]
    fn string_param_neutralizes_injection() {
        // The classic break-out attempt stays inside a single quoted literal.
        let out = render(
            Engine::Trino,
            "WHERE c = {{c}}",
            &[p("c", "string", "x' OR '1'='1")],
        )
        .unwrap();
        assert_eq!(out, "WHERE c = 'x'' OR ''1''=''1'");
    }

    #[test]
    fn number_and_boolean_are_validated() {
        assert_eq!(
            render(Engine::Trino, "LIMIT {{n}}", &[p("n", "number", "10")]).unwrap(),
            "LIMIT 10"
        );
        assert_eq!(
            render(Engine::Trino, "AND b = {{b}}", &[p("b", "boolean", "TRUE")]).unwrap(),
            "AND b = true"
        );
        assert!(render(
            Engine::Trino,
            "LIMIT {{n}}",
            &[p("n", "number", "10; DROP")]
        )
        .is_err());
        assert!(render(
            Engine::Trino,
            "AND b = {{b}}",
            &[p("b", "boolean", "maybe")]
        )
        .is_err());
    }

    #[test]
    fn missing_and_malformed_params_error() {
        assert!(render(Engine::Trino, "WHERE c = {{c}}", &[]).is_err());
        assert!(render(Engine::Trino, "WHERE c = {{ a.b }}", &[]).is_err());
        assert!(render(Engine::Trino, "WHERE c = {{ c ", &[]).is_err());
    }

    #[test]
    fn multiple_and_repeated_params() {
        let out = render(
            Engine::Trino,
            "SELECT {{a}}, {{b}}, {{a}}",
            &[p("a", "number", "1"), p("b", "string", "x")],
        )
        .unwrap();
        assert_eq!(out, "SELECT 1, 'x', 1");
    }

    // --- new: the hole this task closes ---

    #[test]
    fn starrocks_backslash_cannot_escape_the_closing_quote() {
        // StarRocks is MySQL-dialect: `\` is an escape character unless
        // NO_BACKSLASH_ESCAPES is set. Doubling only `'` would render this as
        //   WHERE c = '\'' OR 1=1 -- '
        // where StarRocks reads `\'` as a literal quote, the `'` after it CLOSES
        // the string, and `OR 1=1` becomes live SQL. Doubling the backslash too
        // keeps the whole value inert.
        let out = render(
            Engine::StarRocks,
            "WHERE c = {{c}}",
            &[p("c", "string", r"\' OR 1=1 -- ")],
        )
        .unwrap();
        assert_eq!(out, r"WHERE c = '\\'' OR 1=1 -- '");
    }

    #[test]
    fn trino_leaves_backslash_alone() {
        // Trino has no backslash escape in standard string literals. Applying the
        // StarRocks rule here would silently corrupt ordinary values — a Windows
        // path or a regex would gain a second slash.
        let out = render(
            Engine::Trino,
            "WHERE c = {{c}}",
            &[p("c", "string", r"a\b")],
        )
        .unwrap();
        assert_eq!(out, r"WHERE c = 'a\b'");
    }
}
