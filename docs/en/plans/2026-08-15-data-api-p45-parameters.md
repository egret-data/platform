# Data API P4.5 — Parameters Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make spec §6.2 step 7 real — caller-supplied parameters reach the engine as **bound values**, and endpoint SQL is rendered by a hardened `minijinja` environment whose formatter makes binding the only thing a template can do.

**Architecture:** A new parameter channel is added to `Connector` and `QueryExecPort` as **defaulted methods that refuse**, so no existing implementor silently keeps interpolating. StarRocks binds at the wire (`prep`/`exec_iter`); Trino uses `X-Trino-Prepared-Statement` + `EXECUTE … USING`, where egret renders the literals. `egret-dataapi` renders the template, producing `(sql, Vec<Param>)` in one traversal.

**Tech Stack:** Rust, `mysql_async` 0.37 (StarRocks), Trino HTTP `/v1/statement`, `minijinja` 2.24, sqlx/Postgres for snapshots.

**Spec:** `docs/en/specs/2026-08-15-data-api-parameters-design.md` (+ vi mirror). Read it before Task 1; every engine claim in it was measured, and the section numbers below refer to it.

## Global Constraints

- **Every engine fact in the spec was measured. Do not "improve" on one from reasoning — re-measure it.** Three engine claims made from code-reading during this programme were wrong.
- **A test must be observed failing before it counts.** For each task, stub the implementation once and confirm the assertion goes red. §9's headline criterion exists because a data-corrupting regression leaves every existing unit test green.
- **One traversal produces both the placeholders and the values.** An arity mismatch kills the StarRocks connection (`StmtParamsMismatch`, then "Connection to the server is closed"), so two counts that could disagree must not exist.
- **Errors never name a parameter.** Base spec §7: the name is the endpoint author's, and naming it turns the error surface into a directory of an endpoint's parameters.
- **`minijinja` is pinned at `2.24` with `default-features = false`.** 3.0's alpha already moves the default feature list; do not upgrade in this plan.
- Never call `set_loader`, `set_unknown_method_callback`, `add_function`, or `add_global` on the `Environment`.
- Throwaway `DATABASE_URL` for tests, never the dev `egret` database. Never run two workspace test suites concurrently.
- Commit style: Conventional Commits. Do not push or open a PR; the human merges.

---

### Task 1: The `Param` type and the refusing seam

**Files:**
- Modify: `crates/egret-query-api/src/lib.rs`
- Modify: `crates/egret-query/src/execution/connector/mod.rs`
- Test: `crates/egret-query/tests/connector_params_seam.rs` (create)

**Interfaces:**
- Produces: `egret_query_api::Param`, `Connector::execute_with_params`, `Connector::describe_with_params`. Tasks 2, 3 and 4 implement these; Task 6 calls them through the port.

- [ ] **Step 1: Add the `Param` type to the seam crate**

`Param` lives in `egret-query-api` beside `Limits`, because `egret-dataapi` must construct one without depending on `egret-query`.

```rust
/// A caller-supplied value bound into a query. Never rendered into SQL text by
/// the caller of a connector — the connector decides how its engine carries it.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    /// Exact decimal, kept as text so no float rounding is introduced.
    Decimal(String),
    Str(String),
    /// `YYYY-MM-DD`
    Date(String),
    /// `YYYY-MM-DD HH:MM:SS[.fff]`
    Timestamp(String),
}
```

- [ ] **Step 2: Write the failing seam test**

```rust
// crates/egret-query/tests/connector_params_seam.rs
use egret_core::tabular::TabularResult;
use egret_query::execution::connector::Connector;
use egret_query_api::{Limits, Param};
use tokio_util::sync::CancellationToken;

struct Legacy;

#[async_trait::async_trait]
impl Connector for Legacy {
    async fn execute(&self, _sql: &str, _l: &Limits, _c: CancellationToken)
        -> Result<TabularResult, egret_query::execution::EngineError> {
        Ok(TabularResult::default())
    }
}

#[tokio::test]
async fn a_connector_that_has_not_opted_in_refuses_params_instead_of_ignoring_them() {
    let err = Legacy
        .execute_with_params("SELECT ?", &[Param::I64(1)], &Limits::default(), CancellationToken::new())
        .await
        .expect_err("a connector with no parameter support must refuse, not silently drop the values");
    assert!(err.to_string().contains("does not support bound parameters"), "got: {err}");
}

#[tokio::test]
async fn no_params_still_delegates_to_the_plain_path() {
    Legacy
        .execute_with_params("SELECT 1", &[], &Limits::default(), CancellationToken::new())
        .await
        .expect("an empty parameter list must behave exactly like execute()");
}
```

- [ ] **Step 3: Run it and watch it fail to compile**

Run: `cargo test -p egret-query --test connector_params_seam`
Expected: FAIL — `no method named execute_with_params`.

- [ ] **Step 4: Add the defaulted methods**

In `connector/mod.rs`, on `trait Connector`:

```rust
    /// Run `sql` with `params` bound by the engine's own parameter mechanism.
    ///
    /// The default REFUSES rather than interpolating. That asymmetry is the point:
    /// a defaulted `params` argument on `execute` would force every implementor —
    /// including test fakes and any future connector — to make a decision, and the
    /// cheapest wrong decision is to ignore the argument and build a string. This
    /// codebase already lost a SEV-0 to a defaulted inherited behaviour in `describe`.
    async fn execute_with_params(
        &self,
        sql: &str,
        params: &[Param],
        limits: &Limits,
        cancel: CancellationToken,
    ) -> Result<TabularResult, EngineError> {
        if params.is_empty() {
            self.execute(sql, limits, cancel).await
        } else {
            Err(EngineError::Engine(
                "this connector does not support bound parameters".into(),
            ))
        }
    }

    /// Typed columns of a parameterised `SELECT`, without supplying values.
    ///
    /// Not symmetry: on StarRocks the text path given SQL containing `?` returns
    /// ZERO COLUMNS AND NO ERROR (measured), so publish would freeze an empty
    /// schema instead of failing.
    async fn describe_with_params(
        &self,
        sql: &str,
        param_count: usize,
        timeout: Duration,
    ) -> Result<Vec<ColumnMeta>, EngineError> {
        if param_count == 0 {
            self.describe(sql, timeout).await
        } else {
            Err(EngineError::Engine(
                "this connector does not support describing a parameterised query".into(),
            ))
        }
    }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p egret-query --test connector_params_seam`
Expected: PASS, 2 tests.

- [ ] **Step 6: Mutation — prove the refusal is load-bearing**

Change the `else` arm to `self.execute(sql, limits, cancel).await` (i.e. ignore the params).
Run the test. Expected: `a_connector_that_has_not_opted_in_refuses_params_instead_of_ignoring_them` FAILS.
Revert.

- [ ] **Step 7: Commit**

```bash
git add crates/egret-query-api/src/lib.rs crates/egret-query/src/execution/connector/mod.rs crates/egret-query/tests/connector_params_seam.rs
git commit -m "feat(query): a parameter channel whose default is to refuse"
```

---

### Task 2: StarRocks binding, and the DATE decode arm

**Files:**
- Modify: `crates/egret-query/src/execution/connector/starrocks.rs`
- Test: `crates/egret-query/tests/connector_starrocks_params.rs` (create)

**Interfaces:**
- Consumes: `Param` from Task 1.
- Produces: a working `execute_with_params` on `StarRocksConnector`.

**Live engine required.** StarRocks is on `localhost:9030` via the `egret-dev-starrocks-1` container. The existing `connector_starrocks.rs` gates on `EGRET_TEST_STARROCKS_URL` and **self-skips with `eprintln!("skip")` when it is unset — reporting `ok` while asserting nothing.** Follow that idiom for consistency, but understand what it costs: this task's round-trip test is the plan's headline acceptance criterion, and a self-skip makes it worthless without announcing itself. In your report, state the env var you set and the test's **duration** — a skipped engine test finishes in microseconds, and that is the only signal distinguishing it from a passing one.

- [ ] **Step 1: Write the round-trip test, live, with the six values**

This is §9's headline criterion. `"007"` catches a string being coerced to a number; the backslash and quote are the P1 vector; `NULL` and the decimal catch type collapse; the date is the regression this task must not ship.

```rust
#[tokio::test]
async fn bound_values_round_trip_exactly_on_starrocks() {
    let c = starrocks_or_skip().await;
    let out = c.execute_with_params(
        "SELECT CAST(? AS DATE) d, CAST(? AS DECIMAL(10,4)) m, ? s, ? n, ? bs, ? q",
        &[
            Param::Date("2020-02-29".into()),
            Param::Decimal("12345.6789".into()),
            Param::Str("007".into()),
            Param::Null,
            Param::Str(r"a\' OR 1=1 -- ".into()),
            Param::Str("it's".into()),
        ],
        &Limits::default(),
        CancellationToken::new(),
    ).await.expect("bind");

    assert_eq!(
        serde_json::to_string(&out.rows[0]).unwrap(),
        r#"["2020-02-29","12345.6789","007",null,"a\\' OR 1=1 -- ","it's"]"#,
        "exact JSON: a DATE arriving as Value::Date must not serialize as its Debug repr"
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p egret-query --test connector_starrocks_params`
Expected: FAIL — `does not support bound parameters` (the Task 1 default).

- [ ] **Step 3: Implement `execute_with_params` via prepared statements**

Use `conn.prep(sql)` then `conn.exec_iter(stmt, values)`, mapping `Param` to `mysql_async::Value`. `Decimal`, `Date` and `Timestamp` go over as `Bytes` of their text form — StarRocks reports every parameter as `MYSQL_TYPE_VAR_STRING` and does no inference, and the `CAST` in the SQL is what gives them their type.

- [ ] **Step 4: Add the `V::Date` arm to `row_to_json`**

`starrocks.rs:317`'s match has arms for `Int`, `UInt`, `Float`, `Double`, `Bytes`, and then
`Some(other) => serde_json::Value::String(format!("{:?}", other))`. On the binary protocol
DATE and DATETIME arrive as `V::Date(..)`, hit that fallthrough, and serialize as the literal
string `Date("'2020-02-29'")` — measured.

```rust
            // Binary protocol only: `query()` returns dates as Bytes, `exec()` returns
            // V::Date. Without this arm they fall through to the Debug-repr catch-all
            // below and reach the frontend as `Date("'2020-02-29'")`. Every test on
            // `bytes_to_json` stays green when that happens, because on this path
            // `bytes_to_json` is no longer the function being called.
            Some(V::Date(y, mo, d, h, mi, s, us)) => serde_json::Value::String(
                if h == 0 && mi == 0 && s == 0 && us == 0 {
                    format!("{y:04}-{mo:02}-{d:02}")
                } else if us == 0 {
                    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
                } else {
                    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{us:06}")
                },
            ),
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p egret-query --test connector_starrocks_params`
Expected: PASS.

- [ ] **Step 6: Mutation — delete the `V::Date` arm and watch it go red**

Expected failure text contains `Date("'2020-02-29'")`. This is the mutation that matters most in the whole plan: it is the one that a green suite cannot see. Revert.

- [ ] **Step 7: Add the hostile-value and canary test**

Bind `"'; DROP DATABASE <canary>; --"` and assert (a) it returns as data of the exact length, and (b) the canary database still exists afterwards.

- [ ] **Step 8: Commit**

```bash
git commit -m "feat(query): bind parameters on StarRocks, and decode binary-protocol dates"
```

---

### Task 3: Trino prepared statements

**Files:**
- Modify: `crates/egret-query/src/execution/connector/trino.rs`
- Test: `crates/egret-query/tests/connector_trino_params.rs` (create)

**Interfaces:**
- Consumes: `Param` from Task 1.

**Read spec §3.1 first.** This is **not** wire-level binding: the `USING` list is SQL text egret renders. The parser boundary blocks a second statement and blocks subqueries — measured — but a defect here evaluates arbitrary scalar expressions.

- [ ] **Step 1: Write the literal renderer with its own unit tests (no engine needed)**

Strings go through `egret_query_api::quote_string(Engine::Trino, s)`. `Date`/`Timestamp`/`Decimal` render as typed literals (`DATE '…'`, `TIMESTAMP '…'`, `DECIMAL '…'`) so the value keeps its type. `Null` renders `NULL`.

- [ ] **Step 2: Send the statement in the header, values in the body**

```rust
// The statement travels in a header; the body is `EXECUTE <name> USING <literals>`.
// Measured on Trino 481: the header is required only on the POST — the nextUri GET
// loop does not need it (verified across a 200k-row, 10-hop fetch) — and it coexists
// with the bearer token and X-Trino-User this connector already sends. A 120KB
// statement in the header returned 200, so size is not a practical constraint.
req = req.header("X-Trino-Prepared-Statement", format!("{name}={}", urlencode(sql)));
let body = format!("EXECUTE {name} USING {}", rendered.join(", "));
```

- [ ] **Step 3: Write the live hostile-value test**

Gate on the same env var the existing Trino tests use.

```rust
// Assert the RESULT IS DATA, never merely that there was no error: a well-formed
// injected scalar expression parses fine and never surfaces as a syntax error.
// Measured: `USING current_user` returns the username, `USING 'a' || 'b'` returns "ab".
assert_eq!(row[0], serde_json::json!("'; DROP TABLE zz --"));
```

- [ ] **Step 4: Run, then mutate**

Remove the quote-doubling from the string renderer and confirm the hostile test fails. Revert.

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(query): parameterised execution on Trino via prepared statements"
```

---

### Task 4: `describe_with_params` on both engines

**Files:**
- Modify: `crates/egret-query/src/execution/connector/{trino,starrocks}.rs`
- Test: extend both param test files

**Interfaces:**
- Consumes: Task 1's `describe_with_params`.
- Produces: the publish-time typing path Task 7 depends on.

- [ ] **Step 1: Write the failing tests**

For each engine: describe `SELECT ? AS v, c_int FROM t` (through the existing `LIMIT 0` wrapper) and assert a **non-empty** column list. Add a test asserting that the plain `describe` on the same SQL is *not* what gets called — on StarRocks it returns zero columns and no error, which is the silent failure this exists to prevent.

- [ ] **Step 2: Implement**

Trino: `DESCRIBE OUTPUT <name>` with the prepared-statement header. StarRocks: `conn.prep()` and read `columns()` off the statement.

- [ ] **Step 3: Record the typing constraint in a test**

Neither engine infers a parameter's type: a bare `?` in the projection types as `unknown` (Trino) / `VAR_STRING` (StarRocks). Assert that, so the constraint is pinned rather than folklore — Task 7's `CAST` rule depends on it.

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(query): describe a parameterised query without supplying values"
```

---

### Task 5: The hardened minijinja environment and the binding formatter

**Files:**
- Create: `crates/egret-dataapi/src/render.rs` — a **new** module, named `render` not `template` to avoid colliding with the existing one described below
- Modify: `crates/egret-dataapi/Cargo.toml`
- Test: `crates/egret-dataapi/tests/template_render.rs` (create)

**Read this before writing anything.** `template::render` exists, but it is
`crates/egret-query/src/execution/template.rs` and it belongs to **SQL Lab** — its only
caller is `execution/routes.rs:128`. It interpolates via `quote_string` and its own doc says
"deliberately NOT a general template engine". The Data API **never calls it**, which is
exactly why step 7 does not exist.

The trap: `egret-dataapi/src/lifecycle.rs` comments at `:551`, `:592` and `:629` say P3's
validation matches "what `template::render` will do at serve time". **Nothing on the Data API
serving path ever did.** P3's rules were written against a renderer that was never wired in.
The rules themselves are still right — `PlaceholderInsideQuotedText` in particular is what
makes binding possible — but do not read those comments as a description of live behaviour,
and fix them as you go.

**Interfaces:**
- Produces: `render(sql: &str, ctx: &ParamValues) -> Result<(String, Vec<Param>), RenderError>` — used by Task 6 at serve time and Task 7 at publish time.

- [ ] **Step 1: Add the dependency, exactly**

```toml
minijinja = { version = "2.24", default-features = false,
              features = ["builtins", "serde", "deserialization",
                          "std_collections", "debug", "fuel"] }
```

`macros` and `multi_template` are dropped; `fuel` is added because it is **not** default-on. Do not add `json`, `urlencode`, `custom_syntax`, `loop_controls`, or `unstable_machinery`.

- [ ] **Step 2: Write the failing formatter test — the whole task in one assertion**

```rust
#[test]
fn every_value_emission_becomes_a_placeholder_and_no_construct_bypasses_it() {
    for (tpl, want_sql, want_binds) in [
        ("SELECT {{ a }}",                 "SELECT ?", 1),
        ("SELECT {{ a|safe }}",            "SELECT ?", 1), // `safe` marks; it does not bypass
        ("SELECT {{ 'pre' ~ a ~ 'post' }}","SELECT ?", 1), // the whole concat is ONE value
        ("SELECT {% raw %}{{ a }}{% endraw %}", "SELECT {{ a }}", 0), // literal template text
    ] {
        let (sql, binds) = render(tpl, &ctx_with_a("EVIL' OR 1=1--")).unwrap();
        assert_eq!(sql, want_sql);
        assert_eq!(binds.len(), want_binds);
        assert!(!sql.contains("EVIL"), "no caller text may reach the SQL");
    }
}
```

- [ ] **Step 3: Build the environment, in this order**

```rust
let mut env = Environment::new();
// `debug()` is `format!("{state:#?}")` — a probe recovered the entire template
// context plus an inventory of every registered filter, test and global. On a
// public endpoint that is context disclosure. This is the highest-value call here.
env.remove_global("debug");
env.remove_global("range");
env.remove_global("dict");
env.remove_global("namespace");
env.remove_filter("pprint");                 // second disclosure vector: Debug repr
env.set_undefined_behavior(UndefinedBehavior::Strict);
env.set_fuel(Some(50_000));                  // OutOfFuel is a clean error, not truncation
env.set_recursion_limit(64);                 // setter clamps to min(level, MAX_RECURSION)
// UNCONDITIONAL. `Emit` has a fast path that calls the default escape function and
// skips the formatter; it is guarded by `is_default_formatter()`, which only clears
// when set_formatter is called. Behind a condition, this is a bypass.
env.set_formatter(bind_formatter);
```

- [ ] **Step 4: Write the formatter**

Writes `?` to the output, pushes the value onto the bind list carried in the render state.

- [ ] **Step 5: Reject the capture constructs, with an invariant behind the rule**

```rust
/// `{% filter %}` and `{% set x %}…{% endset %}` buffer their body — inner `{{ }}`
/// has ALREADY become `?` — then emit that whole buffer as one more value through
/// the formatter, so the literal string "?" is bound as a parameter. Measured:
/// `{% filter upper %}{{ a }}{% endfilter %}` yields one placeholder and TWO binds;
/// `{% call %}` yielded three. The bind list desyncs from the placeholder count,
/// which on StarRocks KILLS THE CONNECTION (StmtParamsMismatch, then "Connection to
/// the server is closed"). `{% macro %}`/`{% call %}` are gone with the `macros`
/// feature; these two are parsed unconditionally, so rejection is the only lever.
```

Then the invariant that survives a regression in the rule: **placeholder count == bind count**, asserted by `render` itself and covered by a test over every construct.

- [ ] **Step 6: Implement the `ident` filter**

`{{ p | ident }}` requires `p` to be an `Enum` parameter with `allowed`, matches the caller's value against the list, and emits **the matching entry from `allowed`** — the author's own string, never the caller's. Anything else is an error.

- [ ] **Step 7: Run all tests, then mutate three ways**

(a) Make `set_formatter` conditional → the fast-path bypass test must fail.
(b) Allow `{% filter %}` → the placeholder==bind invariant test must fail.
(c) Make `ident` emit the caller's string → a test binding an `allowed`-adjacent value must fail.

- [ ] **Step 8: Add the CI grep**

`set_loader` and `path_loader` are compiled in unconditionally — the `loader` cargo feature is a **no-op** — so filesystem reach is one accidental call away, permanently. Add a check (test or CI step) asserting neither is called anywhere in the workspace.

- [ ] **Step 9: Commit**

```bash
git commit -m "feat(dataapi): render endpoint SQL with a formatter that can only bind"
```

---

### Task 6: Wire step 7 into the serving path

**Files:**
- Modify: `crates/egret-query-api/src/port.rs`, `crates/egret-query/src/execution/port_impl.rs`
- Modify: `crates/egret-dataapi/src/serving/router.rs`
- Test: `crates/egret-dataapi/tests/serving_params.rs` (create)

**Interfaces:**
- Consumes: Task 5's `render`, Task 1's channel.
- Produces: step 7 in the chain, between authorization and execution.

- [ ] **Step 1: Add `QueryExecPort::execute_as_with_params`, defaulted**

Defaulted to delegate to `execute_as` when `params.is_empty()`. This matters more than it looks: `QueryExecPort` has **10 implementations, 9 of them hand-written test fakes**. A defaulted method keeps all nine compiling; a changed signature is ~9× the edit, and each fake is a place a wrong default silently makes a test vacuous.

- [ ] **Step 2: Percent-decode parameter names before matching**

`declared_query_params` currently matches `p.name == k` against the **raw** key, so `?d%61y=` is dropped and shares a cache entry with a request that has no `day`. `router.rs` records this as a landmine for step 7; this step disarms it. Write the test first: two requests, `?day=x` and `?d%61y=x`, must produce the same rendered SQL and the same bind list.

- [ ] **Step 3: Validate values by `ParamSpec.ty`, and return 400s that name nothing**

`param_missing` and `param_invalid`, neither naming the parameter (base spec §7). Test the body text, not just the status.

- [ ] **Step 4: Keep the cache key honest**

A call with `region` and a call without now execute **different SQL**, so they must not share an entry. §6.3's two rules stand: every field length-prefixed, principal last. Test: warm the cache with `?region=EU`, then call with no `region`, and assert the second is a miss with different rows.

- [ ] **Step 5: Run, then mutate**

Remove the percent-decode → the Step 2 test must fail. Remove the absent-parameter distinction from the cache key → the Step 4 test must fail.

- [ ] **Step 6: Commit**

```bash
git commit -m "feat(dataapi): step 7 — render and bind caller parameters"
```

---

### Task 7: Publish-time validation

**Files:**
- Modify: `crates/egret-dataapi/src/lifecycle.rs`
- Test: `crates/egret-dataapi/tests/lifecycle_params.rs` (create)

**Interfaces:**
- Consumes: Task 5's `render`, Task 4's `describe_with_params`.

- [ ] **Step 1: Write one failing test per rule**

Each rule from spec §5, one test each, each asserting the publish is **rejected** with its own error variant:

- template does not parse under the hardened environment
- contains `{% filter %}` or `{% set x %}…{% endset %}`
- placeholder count != bind count when rendered with representative values
- `| ident` applied to a non-`Enum` parameter, or one without `allowed`
- a parameter inside `LIMIT`/`OFFSET`
- a parameter in the projection without a `CAST`
- `describe_with_params` returns an **empty** column list

- [ ] **Step 2: Implement, extending P3's `check_params` rather than replacing it**

P3 already rejects duplicate names, undeclared placeholders, unreferenced parameters, and `PlaceholderInsideQuotedText`. That last one is what makes binding possible at all — a placeholder that survives it is outside quoted text, which is exactly where a bind marker can go. Do not weaken it.

- [ ] **Step 3: Run, then mutate each rule individually**

Seven rules, seven mutations, each dying independently. A rule that does not die alone is pinned by a different guard than its name claims — that happened twice in P4.

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(dataapi): publish-time validation for parameterised endpoints"
```

---

### Task 8: Documentation and the live check

**Files:**
- Modify: `docs/en/specs/2026-08-14-data-api-design.md` and the vi mirror (§6.2 step 7 is no longer absent)
- Modify: `crates/egret-bi/src/chartquery.rs` (a comment only)

- [ ] **Step 1: Mark step 7 present**

§6.2's step 7 row and every "until step 7 lands" comment in `serving/router.rs` are now stale. A comment that outlives its truth tells the next reader not to look.

- [ ] **Step 2: State the safety models where a reader of any of them would see it**

There are **three** SQL-building surfaces after this phase, not two: the Data API binds;
`egret-bi/src/chartquery.rs::sql_literal` interpolates (chart and dashboard filter values);
and `egret-query/src/execution/template.rs` interpolates (SQL Lab). The latter two are both
`quote_string` call sites, and both were call sites of the P1 backslash bug.

Put a note at the top of **both** interpolating modules saying it is now the older of two
models in this codebase, that the Data API binds, and that migrating them is the larger
remaining prize. Do not write the note only in the spec — a reader of `chartquery.rs` has no
reason to open it.

- [ ] **Step 3: The live check (spec §9)**

Not a unit test. Against a running stack:

1. Publish an endpoint with an optional `region` parameter.
2. Call it with `?region=EU` and with `?region=US`; **confirm the rows actually differ.** This is precisely what does not work today.
3. Call it with `region` absent; confirm the `{% if %}` block was **omitted**, not bound as `NULL` — a `WHERE region = NULL` returns zero rows and would look like "no data" rather than a bug.
4. Bind a date and confirm it renders as `2020-02-29`, not `Date("'2020-02-29'")`.
5. Record the exact commands and raw output in the PR body.

- [ ] **Step 4: Commit**

```bash
git commit -m "docs(dataapi): step 7 is implemented; name the two safety models"
```

---

## Self-review

**Spec coverage.** §3.1 → Task 3; §3.2 → Task 2; §3.3 → Task 2 step 4; §3.4 → Tasks 1 and 4; §4.1 → Task 5 steps 3-4; §4.2 → Task 5 step 5; §4.4 → Task 5 step 6; §4.5 → Task 5 steps 1, 3, 8; §5 publish rules → Task 7; §5 serve rules → Task 6; §6 → Task 6 step 4; §7 → Task 8 step 1; §8 → Task 8 step 2; §9 → the mutation step of every task plus Task 8 step 3.

**Type consistency.** `Param` is defined once (Task 1) and consumed unchanged by Tasks 2, 3, 5, 6. `TabularResult`/`ColumnMeta` come from `egret_core::tabular`; `Limits` from `egret_query_api`; `EngineError::Engine(String)` is the refusal variant. `row_to_json(row, col_types)` is at `starrocks.rs:317` and its catch-all is `Some(other) => Value::String(format!("{:?}", other))` — verified in the tree, not assumed.

**Known gap, deliberately left to the implementer.** Task 5 step 1 says to check whether `egret-dataapi/src/template.rs` already exists — spec §5 refers to `template::render`, and P3 shipped a `template.rs` with the `{{param}}` scanner. Whether this extends that module or replaces it is a judgement the implementer should make with the file open, and the brief says so rather than guessing here.
