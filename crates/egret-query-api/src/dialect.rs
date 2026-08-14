//! Dialect-aware SQL literal escaping — the only place in the workspace where a
//! string value becomes a SQL literal.
//!
//! Trino does not process backslash escapes inside standard string literals, so
//! doubling `'` is sufficient and doubling `\` would corrupt the value. StarRocks
//! is MySQL-dialect and, unless `NO_BACKSLASH_ESCAPES` is set, treats `\` as an
//! escape character — so a value containing `\'` escapes the quote and breaks out
//! of a literal that only doubled quotes.
//!
//! Quotes are escaped as `''` rather than `\'` on purpose: that keeps the two
//! substitution passes order-independent, because the quote pass emits no
//! backslash and the backslash pass emits no quote.
//!
//! This lives in the contract crate, not in `egret-query`, so that `egret-query`
//! (SQL Lab) and `egret-bi` (chart/dashboard filters) share one implementation.
//! Two copies of this rule are what allowed them to drift apart.

use crate::Engine;

/// Quote and escape `value` as a SQL string literal for `engine`.
pub fn quote_string(engine: Engine, value: &str) -> String {
    match engine {
        Engine::Trino => format!("'{}'", value.replace('\'', "''")),
        Engine::StarRocks => {
            let escaped = value.replace('\\', "\\\\").replace('\'', "''");
            format!("'{escaped}'")
        }
    }
}
