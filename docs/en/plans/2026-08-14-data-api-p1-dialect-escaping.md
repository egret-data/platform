# Data API P1 — Dialect-safe parameter escaping Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

> Spec: `docs/en/specs/2026-08-14-data-api-design.md` §3
> Part: P1 of 5 — see `docs/en/plans/2026-08-14-data-api-decomposition.md`

**Goal:** make SQL string-literal escaping safe on StarRocks at both sites that produce one, closing an injection hole that is open in SQL Lab and chart/dashboard filters today and would be exposed publicly by Data API.

**Architecture:** two sibling helpers — `template::literal` in `egret-query` and `sql_literal` in `egret-bi` — both render a string by doubling `'` only. StarRocks is MySQL-dialect and treats `\` as an escape character, so `\'` breaks out of the literal. Replace both with one `quote_string(engine, value)` in the shared contract crate `egret-query-api`, thread `Engine` through each caller, and prove the fix with unit tests that fail against the old code plus a live StarRocks test.

**Tech Stack:** Rust · sqlx · axum · StarRocks (mysql_async) · Trino.

## Global Constraints

- This plan ships on its own and must merge **before** Data API P2–P5. It is a security fix with independent value: SQL Lab and BI filters are patched by the same change.
- The escaper lives in `egret-query-api`, not `egret-query`. `Engine` already lives there and `egret-bi` already depends on it, so both callers reach one implementation. Two copies are what produced this inconsistency originally — do not create a third.
- Trino behaviour must not change. Trino does not process backslash escapes in standard string literals; doubling backslashes there would silently corrupt legitimate values (Windows paths, regexes).
- Quotes are escaped as `''`, never `\'`. This keeps the backslash and quote passes order-independent — the quote pass emits no backslash, the backslash pass emits no quote.
- Parameters render values only, never identifiers. This plan does not add identifier support.
- Run the full check locally before merge (`just check` includes `cargo fmt`). GitHub CI is out of quota.
- Never run two workspace test suites concurrently.

## File Structure

| File | Responsibility |
|---|---|
| `crates/egret-query-api/src/dialect.rs` (create) | The only place a SQL string literal is quoted, for the whole workspace. One function, keyed on `Engine`. |
| `crates/egret-query-api/src/lib.rs` (modify) | Declare and re-export the `dialect` module. |
| `crates/egret-query/src/execution/template.rs` (modify) | Placeholder scanning + parameter lookup. Delegates literal rendering; no longer knows escaping rules. |
| `crates/egret-query/src/execution/routes.rs:159` (modify) | Pass `db.engine` into `render()`. `db` is already in scope at line 118. |
| `crates/egret-query/tests/connector_starrocks.rs` (modify) | Live proof that the escaped literal is inert on a real StarRocks. |
| `crates/egret-bi/src/chartquery.rs` (modify) | `sql_literal` takes `Engine` and delegates. Both call sites (lines 333, 344) already have `engine` in scope. |

---

### Task 1: Dialect-aware literal escaping

**Files:**
- Create: `crates/egret-query-api/src/dialect.rs`
- Modify: `crates/egret-query-api/src/lib.rs`
- Modify: `crates/egret-query/src/execution/template.rs`
- Modify: `crates/egret-query/src/execution/routes.rs` (line 159)
- Test: `crates/egret-query/src/execution/template.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `egret_query_api::Engine` (variants `Trino`, `StarRocks`; reachable inside `egret-query` as `crate::registry::model::Engine`), `crate::execution::model::QueryParam` (fields `name: String`, `ty: String`, `value: String`).
- Produces:
  - `pub fn egret_query_api::quote_string(engine: Engine, value: &str) -> String` — also used by Task 3.
  - `pub fn template::render(engine: Engine, sql: &str, params: &[QueryParam]) -> Result<String, String>` — **signature change**, `engine` is a new first parameter.

- [ ] **Step 1: Write the failing tests**

Replace the whole `#[cfg(test)] mod tests` block at the bottom of `crates/egret-query/src/execution/template.rs` (currently lines 66-129) with this. Every existing test gains an `Engine` argument; the last two are new and are the point of the task.

```rust
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
        assert!(render(Engine::Trino, "LIMIT {{n}}", &[p("n", "number", "10; DROP")]).is_err());
        assert!(render(Engine::Trino, "AND b = {{b}}", &[p("b", "boolean", "maybe")]).is_err());
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
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test -p egret-query --lib execution::template`

Expected: compile error — `render` takes 2 arguments but 3 were supplied, and `dialect` does not exist. That compile failure IS the red state; do not proceed until you have seen it.

- [ ] **Step 3: Create the dialect module in the contract crate**

Create `crates/egret-query-api/src/dialect.rs`:

```rust
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
```

- [ ] **Step 4: Declare and re-export the module**

In `crates/egret-query-api/src/lib.rs`, add near the top of the file, after the existing `pub use egret_core::{ColumnMeta, TabularResult};` line:

```rust
pub mod dialect;
pub use dialect::quote_string;
```

- [ ] **Step 5: Thread the engine through `render` and `literal`**

In `crates/egret-query/src/execution/template.rs`, change the module doc, the `render` signature, the `literal` call, and the `literal` signature. Replace lines 1-64 with:

```rust
//! SQL Lab query templating: substitute `{{ name }}` placeholders with typed,
//! escaped SQL literals. This is deliberately NOT a general template engine —
//! only simple named-variable references are supported, and every value is
//! rendered as a validated literal (string → quoted + escaped per the target
//! engine's dialect, number / boolean → validated), so a parameter value can
//! never inject SQL.
//!
//! Escaping rules live in [`egret_query_api::dialect`], not here.

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
```

- [ ] **Step 6: Update the one production call site**

In `crates/egret-query/src/execution/routes.rs`, line 159. `db` is already bound at line 118 and carries `engine`. Change:

```rust
    let rendered = crate::execution::template::render(&body.sql, &body.params)
        .map_err(|m| EngineApiError::from(egret_core::Error::InvalidInput(m)))?;
```

to:

```rust
    let rendered = crate::execution::template::render(db.engine, &body.sql, &body.params)
        .map_err(|m| EngineApiError::from(egret_core::Error::InvalidInput(m)))?;
```

- [ ] **Step 7: Run the tests and verify they pass**

Run: `cargo test -p egret-query --lib execution::template`

Expected: PASS, 8 tests.

Then confirm nothing else called `render`:

Run: `cargo build -p egret-query --all-targets`

Expected: builds clean. If another call site appears, pass the `Engine` it has in scope — never default to `Engine::Trino` to make it compile, because that would apply the weaker escaping to StarRocks and silently restore the hole.

- [ ] **Step 8: Verify the test is real by mutating the implementation**

A test that passes with and without the fix is not evidence — that is exactly how this hole survived the first time. Temporarily change `crates/egret-query-api/src/dialect.rs` so `Engine::StarRocks` uses the Trino arm:

```rust
        Engine::StarRocks => format!("'{}'", value.replace('\'', "''")),
```

Run: `cargo test -p egret-query --lib execution::template`

Expected: `starrocks_backslash_cannot_escape_the_closing_quote` FAILS. If it passes, the test is worthless — fix the test before going further.

Then revert the mutation and re-run to confirm green again.

- [ ] **Step 9: Commit**

```bash
git add crates/egret-query-api/src/dialect.rs \
        crates/egret-query-api/src/lib.rs \
        crates/egret-query/src/execution/template.rs \
        crates/egret-query/src/execution/routes.rs
git commit -m "fix(query): escape backslashes in StarRocks string literals

template.rs doubled single quotes but ignored backslash. StarRocks is
MySQL-dialect and treats \\ as an escape character unless
NO_BACKSLASH_ESCAPES is set, so a parameter value containing \\' escaped
the closing quote and broke out of the literal. Trino is unaffected and
must not be given the same treatment, which would corrupt legitimate
values.

Escaping now lives in execution/dialect.rs keyed on Engine, and render()
takes the engine as its first argument."
```

---

### Task 2: Live StarRocks proof

**Files:**
- Modify: `crates/egret-query/tests/connector_starrocks.rs`

**Interfaces:**
- Consumes: `template::render` and `Engine` from Task 1; the file's existing `spec(host, port)` helper and `parse()` self-skip helper; `build_connector`, `Limits`, `CancellationToken`.
- Produces: nothing consumed downstream.

The unit test in Task 1 asserts a string. It cannot prove that StarRocks *parses* that string the way we believe. This test does — it runs the rendered SQL on a real engine and asserts the injected predicate never takes effect.

- [ ] **Step 1: Write the failing test**

Append this verbatim to `crates/egret-query/tests/connector_starrocks.rs`. It uses the file's existing `parse()` self-skip helper and `spec(host, port)` builder, both already defined at the top of that file.

```rust
/// The rendered literal must be inert on a real StarRocks, not merely
/// inert-looking as a Rust string. Injected, `OR 1=1` would make the count 1.
#[tokio::test]
async fn starrocks_rendered_param_cannot_inject() {
    let Some((h, p)) = parse() else {
        eprintln!("skip");
        return;
    };
    let c = build_connector(&spec(&h, p));
    let limits = Limits {
        max_rows: 10,
        timeout: Duration::from_secs(30),
    };

    let sql = template_render_starrocks(
        "SELECT count(*) AS n FROM (SELECT 'a' AS c) t WHERE t.c = {{c}}",
        r"\' OR 1=1 -- ",
    );

    let r = c
        .execute(&sql, &limits, CancellationToken::new())
        .await
        .expect("query must parse and run — a parse error means the literal broke out");

    // 'a' never equals the attacker string, so the only correct answer is 0.
    // A 1 here means `OR 1=1` executed.
    assert_eq!(r.rows.len(), 1, "count() returns exactly one row");
    assert_eq!(
        r.rows[0][0],
        serde_json::json!(0),
        "injected predicate took effect: {sql}"
    );
}

fn template_render_starrocks(sql: &str, value: &str) -> String {
    use egret_query::execution::model::QueryParam;
    use egret_query::execution::template::render;
    use egret_query::registry::model::Engine;
    render(
        Engine::StarRocks,
        sql,
        &[QueryParam {
            name: "c".into(),
            ty: "string".into(),
            value: value.into(),
        }],
    )
    .expect("render must succeed")
}
```

- [ ] **Step 2: Check visibility, and fix it if needed**

`render` and `QueryParam` must be reachable from an integration test. Run:

Run: `cargo test -p egret-query --test connector_starrocks --no-run`

Expected: compiles. If `template` is private, make it `pub` in `crates/egret-query/src/execution/mod.rs` (`pub mod template;`) rather than duplicating the escaping logic into the test — a test that reimplements the thing it tests proves nothing.

- [ ] **Step 3: Run against a real StarRocks and verify it passes**

Start the local stack if it is not already up:

```bash
just up
```

**The env var must be a full URL with a scheme.** `parse()` (`connector_starrocks.rs:23`) runs `url::Url::parse(&base).ok()?`, and a URL scheme may not begin with a digit — so a bare `127.0.0.1:9030` fails to parse, returns `None`, and the test **self-skips silently**. A skipped test prints "skip" and exits 0, which looks exactly like a pass in a summary line.

```bash
EGRET_TEST_STARROCKS_URL=mysql://root@127.0.0.1:9030 \
  cargo test -p egret-query --test connector_starrocks starrocks_rendered_param_cannot_inject -- --nocapture
```

Expected: PASS, with no "skip" in the output. Read the actual output — do not trust the `test result: ok` line alone, because a self-skipped test also reports ok.

- [ ] **Step 4: Verify the test is real by mutating the implementation**

Apply the same mutation as Task 1 Step 8 (in `crates/egret-query-api/src/dialect.rs`, `Engine::StarRocks` rendering like Trino) and re-run Step 3.

Expected: FAILS, and the failure is either the `assert_eq!` reporting `1` instead of `0`, or the `.expect(...)` on a parse error. Either failure proves the test observes the real behaviour. Revert the mutation and re-run to confirm green.

- [ ] **Step 5: Full local check**

```bash
cargo fmt --all --check
cargo clippy -p egret-query -p egret-query-api --all-targets
cargo test -p egret-query -p egret-query-api
```

Expected: fmt clean, clippy clean, suites green. The StarRocks test self-skips without the env var; that is expected in this run.

Do **not** use `just check` here: it runs `cargo test --workspace`, which contradicts this plan's constraint against workspace-wide runs and drags in unrelated suites. The whole-workspace run belongs in the final review, once, not after every task. `egret-config::tests::toml_file_is_merged` is a known pre-existing flake (a `figment::Jail` parallelism race) and is not this plan's to fix.

- [ ] **Step 6: Commit**

```bash
git add crates/egret-query/tests/connector_starrocks.rs crates/egret-query/src/execution/mod.rs
git commit -m "test(query): live StarRocks proof that rendered params cannot inject

The unit test asserts a Rust string; it cannot show how StarRocks parses
that string. This runs the rendered SQL on a real engine and asserts the
injected OR 1=1 never takes effect. Self-skips unless
EGRET_TEST_STARROCKS_URL is set, matching the file's convention."
```

---

### Task 3: Close the same hole in BI filter values

**Files:**
- Modify: `crates/egret-bi/src/chartquery.rs` (`sql_literal` at line 135; call sites at lines 333 and 344)
- Test: `crates/egret-bi/src/chartquery.rs` (inline `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `egret_query_api::quote_string` and `egret_query_api::Engine` from Task 1. `egret-bi` already depends on `egret-query-api`, so no `Cargo.toml` change is needed.
- Produces: `pub(crate) fn sql_literal(engine: Engine, v: &serde_json::Value, type_generic: &str) -> Result<String, BiError>` — **signature change**, `engine` is a new first parameter.

This is the more exposed of the two sites: its own doc comment calls it "the security-critical primitive", and it is reachable with the lowest BI permission (`CH_READ`) through chart and dashboard filter values.

- [ ] **Step 1: Write the failing tests**

In `crates/egret-bi/src/chartquery.rs`, replace the existing `sql_literal_encodes_by_type` test with the version below and add the two new tests after it. Every existing assertion gains an `Engine` argument.

```rust
    #[test]
    fn sql_literal_encodes_by_type() {
        use egret_query_api::Engine;
        let e = Engine::Trino;
        assert_eq!(sql_literal(e, &json!(42), "numeric").unwrap(), "42");
        assert_eq!(sql_literal(e, &json!("42"), "numeric").unwrap(), "42");
        assert!(sql_literal(e, &json!("nope"), "numeric").is_err());
        assert_eq!(sql_literal(e, &json!(true), "boolean").unwrap(), "TRUE");
        assert_eq!(sql_literal(e, &json!("hi"), "string").unwrap(), "'hi'");
        // injection attempt stays safely quoted
        assert_eq!(
            sql_literal(e, &json!("a' OR '1'='1"), "string").unwrap(),
            "'a'' OR ''1''=''1'"
        );
    }

    #[test]
    fn sql_literal_starrocks_escapes_backslash() {
        use egret_query_api::Engine;
        // A dashboard filter value reaches here with only CH_READ. Doubling the
        // quote alone would render '\'' OR 1=1 -- ', where StarRocks reads \' as a
        // literal quote and the next ' closes the string.
        assert_eq!(
            sql_literal(Engine::StarRocks, &json!(r"\' OR 1=1 -- "), "string").unwrap(),
            r"'\\'' OR 1=1 -- '"
        );
    }

    #[test]
    fn sql_literal_trino_leaves_backslash_alone() {
        use egret_query_api::Engine;
        // Trino has no backslash escape in standard literals; doubling it here
        // would corrupt ordinary filter values.
        assert_eq!(
            sql_literal(Engine::Trino, &json!(r"a\b"), "string").unwrap(),
            r"'a\b'"
        );
    }
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `cargo test -p egret-bi --lib chartquery::tests::sql_literal`

Expected: compile error — `sql_literal` takes 2 arguments but 3 were supplied. That compile failure is the red state.

- [ ] **Step 3: Thread `Engine` through `sql_literal`**

In `crates/egret-bi/src/chartquery.rs`, change the signature at line 135 and the fall-through arm that quotes strings. Keep the numeric and boolean arms exactly as they are — they emit no quotes and need no engine.

```rust
/// Encode a filter value as a SQL literal, validated by the target column's generic type.
/// Numerics are validated and emitted unquoted; booleans normalized; everything else is
/// quoted and escaped for `engine`'s dialect. This is the security-critical primitive.
pub(crate) fn sql_literal(
    engine: Engine,
    v: &serde_json::Value,
    type_generic: &str,
) -> Result<String, BiError> {
```

Then change **only line 184**, the last line of the fall-through arm. Leave the surrounding `match` exactly as it is — in particular the `Value::Null` branch, which returns a 400 and must not be lost:

```rust
        _ => {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => {
                    return Err(BiError::BadRequest("filter value is null".into()))
                }
                other => other.to_string(),
            };
            Ok(quote_string(engine, &s))          // was: format!("'{}'", s.replace('\'', "''"))
        }
```

`Engine` is already imported at line 5. Extend that import rather than adding a second one:

```rust
use egret_query_api::{quote_string, Engine};
```

Note the local precedent: `quote_ident(engine, ident)` (`crates/egret-bi/src/schema.rs:186`) already takes the engine and dispatches per dialect. This change makes literals match what identifiers have always done.

**Do not touch `like_literal` (`crates/egret-bi/src/schema.rs:161`).** It looks similar but is a different thing: it escapes a search term for a `LIKE` *pattern* used with `ESCAPE '\'`, so it escapes the wildcards `%` and `_` as well, and it does not wrap the result in quotes — its caller does. Folding it into `quote_string` would break wildcard escaping.

- [ ] **Step 4: Update both call sites**

Lines 333 and 344 already have `engine` in scope — they pass it to `cast_temporal` on the very same line. Change:

```rust
                .map(|v| sql_literal(v, tg).map(|l| cast_temporal(engine, c, l)))
```

to:

```rust
                .map(|v| sql_literal(engine, v, tg).map(|l| cast_temporal(engine, c, l)))
```

and:

```rust
            let lit = cast_temporal(engine, c, sql_literal(&f.value, tg)?);
```

to:

```rust
            let lit = cast_temporal(engine, c, sql_literal(engine, &f.value, tg)?);
```

- [ ] **Step 5: Run the tests and verify they pass**

Run: `cargo test -p egret-bi --lib chartquery`

Expected: PASS. Then check for any call site not covered above:

Run: `cargo build -p egret-bi --all-targets`

Expected: builds clean. As in Task 1, never pass `Engine::Trino` just to make a call site compile.

- [ ] **Step 6: Verify the tests are real by mutating the implementation**

Apply the Task 1 Step 8 mutation again (`Engine::StarRocks` rendering like Trino in `crates/egret-query-api/src/dialect.rs`).

Run: `cargo test -p egret-bi --lib chartquery`

Expected: `sql_literal_starrocks_escapes_backslash` FAILS. Revert and confirm green.

- [ ] **Step 7: Full workspace check**

```bash
just check
cargo test -p egret-query -p egret-bi
```

Expected: fmt clean, clippy clean, both suites green. Run these sequentially, never two workspace suites at once.

- [ ] **Step 8: Commit**

```bash
git add crates/egret-bi/src/chartquery.rs
git commit -m "fix(bi): escape backslashes in StarRocks filter literals

sql_literal had the same backslash-blind escaping as template.rs, and is
the more exposed of the two: it is reachable with CH_READ alone through
chart and dashboard filter values. It now delegates to the shared
quote_string and takes the engine, which both call sites already had in
scope."
```

---

## Done when

- `cargo test -p egret-query -p egret-bi` green, and the StarRocks test green with `EGRET_TEST_STARROCKS_URL` set.
- All three new tests have been observed to FAIL against a mutated implementation (Task 1 Step 8, Task 2 Step 4, Task 3 Step 6). This is the acceptance criterion that matters — green tests alone do not close this out.
- `just check` clean.
- Exactly one implementation of string-literal quoting remains in the workspace. Confirm with:

```bash
rg -n --type rust "\"''\"" crates/
```

Expected: exactly two hits, and no others.
  - `crates/egret-query-api/src/dialect.rs` — the one literal quoter.
  - `crates/egret-bi/src/schema.rs:165` — inside `like_literal`, which is a `LIKE`-pattern escaper (it also escapes `%` and `_`, and does not wrap in quotes). It stays.

A third hit means a copy that will drift, which is how this bug was born.

- Trino behaviour byte-identical to before: `trino_leaves_backslash_alone` and `sql_literal_trino_leaves_backslash_alone` cover the regression on both sites.

## Not in this plan

- Bound parameters at the connector layer (`Connector::execute` takes `sql: &str`). StarRocks could use `mysql_async` binding directly; Trino needs `X-Trino-Prepared-Statement` + `EXECUTE … USING`. That is a `Connector` trait change and its own project. `quote_string` is a single function so it can be replaced in one place.
- Identifier parameters (dynamic column / table names). Spec §3 requires these to be `enum` parameters with an allow-list; that lands with the Data API parameter model in P3.
- `sql_mode = NO_BACKSLASH_ESCAPES` on StarRocks connections. It would be defence in depth, but changing session `sql_mode` alters parsing for every other query on that connection, so it needs its own evaluation rather than a drive-by.
- `starrocks_create_service_user_sql` (`crates/egret-engine-mgmt/src/engines.rs:3229`) has the same gap and its doc comment notes it runs as **root**. It is latent because the username it interpolates is a constant, so it is not a live vulnerability and not a blocker for Data API. It should get its own small PR rather than being smuggled into this one — but it should not be forgotten, because the next person to make that username dynamic inherits a root-level injection.
