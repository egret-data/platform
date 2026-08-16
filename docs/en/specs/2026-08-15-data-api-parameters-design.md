# Data API P4.5 — Parameters: bound values and template rendering

Amends `2026-08-14-data-api-design.md` §3, §6.2 step 7, §6.3 and §10. Where the two
disagree, this document governs and the earlier one is corrected rather than left standing.

## 1. Why this exists

Spec §6.2 lists ten steps in the serving chain. **Step 7 — rendering declared `params` into
the SQL — was never implemented.** P3 validates the shape of a parameter, P4 puts it in the
cache key, and nothing substitutes it. Measured on the shipped code: `?region=EU` and
`?region=US` return identical rows, because no caller input reaches the engine at all.

So the Data API today serves static `SELECT`s. The headline promise of the programme — a
parameterised query published as a REST endpoint — is the one part not delivered.

Step 7 is also the **only** place caller input reaches SQL. It is therefore the whole
feature's injection surface, and how it is built decides whether that surface exists at all.

## 2. Decisions, and what changed from the original spec

**Values are bound, not interpolated.** §10 recorded bound parameters as deferred "for cost,
not because interpolation was judged adequate", and listed every escaping defect this feature
has had as a consequence of that deferral. The cost was measured (§3) and is affordable.

**Rendering is `minijinja` with a custom formatter, not a bespoke template syntax and not a
MyBatis port.** No Rust library implements MyBatis at runtime: `rbatis` describes itself as
"Compile Time Async Dynamic SQL ORM" and `rbatis-codegen` exposes only `rb_html`/`rb_py`,
both producing `TokenStream`; the `mybatis` crate's last release is 2022-09-23 and its XML
mapping is unimplemented. `minijinja` renders at runtime from a `&str`, and — decisively —
`Environment::set_formatter` intercepts **every** value emission, so binding is not a
convention the template author must follow but the only thing the engine can do.

**§10 is corrected in three places.** It said the fix would be "subtractive", deleting the
dialect escaper: not true — Trino still needs it (§3.1), and `egret-bi` still uses it. It
said a general template engine would be "a new attack surface… known risk area for Superset":
true of Python Jinja2, not of minijinja (§4.4). It deferred conditional rendering as a
separate, later, minimal construct: unnecessary — conditionals arrive free with the renderer,
and the ordering rule it stated ("bound parameters first") is satisfied within this one phase
because the formatter *is* the binding.

## 3. Engine mechanics

The two engines do not do the same thing, and the spec says so rather than calling both
"binding".

### 3.1 Trino — parser-level separation, egret renders the literals

Measured against Trino 481 over HTTPS with a Keycloak JWT:

```
POST /v1/statement
  Authorization: Bearer …
  X-Trino-User: tester
  X-Trino-Prepared-Statement: p1=SELECT%20%3F%20AS%20s%2C%20%3F%20AS%20n
  body: EXECUTE p1 USING 'hello', 42
→ 200, columns=[('s','varchar(5)'),('n','integer')], rows=[["hello",42]]
```

Removing the header gives `Prepared statement not found: p1`, so the header is what carries
the statement. Measured properties:

- Coexists with the bearer token and `X-Trino-User` egret already sends. Impersonating a
  different user is refused by Trino (`Access Denied: User tester cannot impersonate alice`),
  which is the existing behaviour and is unaffected.
- The header is required **only on the POST**; the `nextUri` GET loop does not need it.
  Verified on a 200 000-row, 10-hop fetch: header on the POST alone, all rows correct.
- Header size is not a practical constraint: 120 024 bytes of SQL returned 200.

**The `USING` list is SQL text that egret renders.** Trino's HTTP protocol has no wire-level
parameter channel. The parser boundary is real and strong — measured:

| `USING` payload | Result |
|---|---|
| `'a'; DROP TABLE zz` | `SYNTAX_ERROR` — the endpoint takes one statement |
| `'' UNION ALL SELECT …` | `SYNTAX_ERROR` |
| `'a' \|\| (SELECT count(*) FROM …)` | rejected — `Unsupported expression (SubqueryExpression)` |
| `'a' \|\| 'b'` | **`"ab"`** — accepted |
| `current_user` | **`"tester"`** |
| `upper('ab')` | **`"AB"`** |

So a defect in the literal renderer is **arbitrary-scalar-expression** injection, not
arbitrary SQL. That is a large reduction and it is not zero. `quote_string` therefore
survives on the Trino path and keeps its tests. Backslash is not an escape character in
Trino (`USING 'a\\b'` → `a\\b`, 4 chars), so the P1 backslash vector does not apply here.

Typed values round-trip exactly: `'x'`, `9007199254740993`, `DATE '2020-02-29'`,
`TIMESTAMP '2020-02-29 13:14:15.123'`, `DECIMAL '123.456'`, `true`, `NULL` come back as
`varchar(1), bigint, date, timestamp, decimal(6,3), boolean, varchar`.

### 3.2 StarRocks — true wire-level binding

`conn.prep()` then `exec_iter()`, binary protocol, length-prefixed. Measured on StarRocks
4.1.3: `prep` returns parameter count and full output-column metadata; hostile values return
verbatim as data and the canary database survived:

```
E-backslash  bound "a\' OR 1=1 -- "  ->  Bytes "a\' OR 1=1 -- "  length 14
F. p45_bind still present after hostile binds: true
```

This closes the P1 backslash vector by construction on the engine where it was proven
exploitable.

Measured constraints, each of which is a publish-time validation rule:

- `?` is rejected in `LIMIT`/`OFFSET` (`using parameter(?) as limit or offset not supported`).
- `?` is rejected as an identifier (`No viable statement for input '….?'`).
- `IN (?)` binds exactly **one** value. A list parameter must render N placeholders (§4.3).
- StarRocks reports every parameter as `MYSQL_TYPE_VAR_STRING`; there is no parameter type
  inference to rely on.
- Prepared statements work **after** `EXECUTE AS <user> WITH NO REVERT`, so the impersonate
  path is unaffected.
- **Not measured:** prepared statements over the JWT/`mysql_clear_password` bearer path; the
  dev StarRocks has no `authentication_jwt` configured. Nothing suggests the auth plugin
  affects `COM_STMT_PREPARE`. Treat as unverified until it is run.

**Cost:** one extra round trip (`COM_STMT_PREPARE`) per query — measured 5.59 ms/op → 9.88
ms/op locally for connect+execute. Irrelevant for analytics queries, visible on the Data
API's cheap cache-miss path.

### 3.3 The decode regression that must be fixed in the same change

Switching StarRocks to `exec()` changes the wire representation. Running egret's own
`row_to_json`/`bytes_to_json` unmodified over both protocols:

| column | text `query()` | binary `exec()` |
|---|---|---|
| INT / BIGINT / BOOLEAN | `Bytes` | `Int` |
| DOUBLE | `Bytes` | `Double` |
| LARGEINT / DECIMAL / VARCHAR | `Bytes` | `Bytes` |
| **DATE / DATETIME** | **`Bytes`** | **`Value::Date(..)`** |

`row_to_json` has no `V::Date` arm, so DATE and DATETIME fall through to
`Value::String(format!("{:?}", other))` and serialize as the literal `Date("'2020-02-29'")`.
Measured, not hypothesised.

**Every existing unit test on `bytes_to_json` still passes, because `bytes_to_json` is no
longer the function being called.** This is why §9's acceptance criterion is written the way
it is.

The by-column-type coercion stays correct and becomes partly redundant: integers and doubles
arrive pre-typed, so those arms stop firing, while the `"007"` VARCHAR-preservation property
holds because VARCHAR/DECIMAL/LARGEINT remain `Bytes` on both protocols.

### 3.4 The seam

A new method on `Connector`, defaulting to **refuse**:

```rust
async fn execute_with_params(
    &self, sql: &str, params: &[Param], limits: &Limits, cancel: CancellationToken,
) -> Result<TabularResult, EngineError> {
    if params.is_empty() { self.execute(sql, limits, cancel).await }
    else { Err(EngineError::Engine("connector does not support bound parameters".into())) }
}
```

Not a `params` argument added to `execute`. A defaulted argument forces every implementor —
including three test fakes and any future connector — to decide, and the cheapest wrong
decision is to ignore it and interpolate. A separate method whose default is an error fails
closed. This codebase already lost a SEV-0 to a defaulted inherited behaviour in `describe`.

The same shape for `QueryExecPort::execute_as_with_params`, defaulted to delegate when
`params.is_empty()`. This matters more than it looks: `Connector` has 5 production call
sites, but `QueryExecPort` has **10 implementations, 9 of them hand-written test fakes** —
changing its signature is roughly 9× the edit, and each fake is a place where a wrong default
silently makes a test vacuous. Delete the defaults later, when the fakes are migrated
deliberately.

`describe` needs the same treatment, and not for symmetry: on StarRocks the text path given
SQL containing `?` returns **zero columns and no error**, so publish would freeze an empty
schema instead of failing. Both engines can describe a parameterised query without values —
Trino via `DESCRIBE OUTPUT p1` (and `DESCRIBE INPUT p1` for the parameter list), StarRocks
via `prep()` on the `LIMIT 0` wrapper.

**Neither engine infers a parameter's type.** A bare `?` in the projection types as `unknown`
on Trino and `VAR_STRING` on StarRocks. A parameter appearing in the select list must
therefore be wrapped in a `CAST`; publish rejects it otherwise, rather than freezing a wrong
type into the snapshot.

## 4. Rendering

### 4.1 The formatter is the binding

`Environment::set_formatter` replaces value emission wholesale:

```rust
pub fn set_formatter<F>(&mut self, f: F)
where F: Fn(&mut Output<'_>, &State<'_, '_>, &Value) -> Result<(), Error> + 'static + Sync + Send
```

egret's formatter writes `?` to the output and pushes the value onto the bind list. There is
no path from `{{ … }}` to raw SQL text, so an endpoint author cannot write an interpolating
template even deliberately.

Verified by probe against minijinja 2.24.0, with the formatter emitting `?`:

| Construct | SQL | Binds |
|---|---|---|
| `SELECT {{ a }}` | `SELECT ?` | `["EVIL' OR 1=1--"]` |
| `{{ a\|safe }}` | `SELECT ?` | 1 — `safe` marks, it does not bypass |
| `{{ a\|escape }}` | `SELECT ?` | 1 |
| `{{ 'pre' ~ a ~ 'post' }}` | `SELECT ?` | 1 — the whole concatenation is one value |
| `{% for %}` / `{% if %}` bodies | `SELECT ?,?,` | one per value |
| `{% raw %}{{ a }}{% endraw %}` | `SELECT {{ a }}` | 0 — literal template text |

The structural argument behind the probe: the VM writes to output in exactly two places, and
the raw one is typed `EmitRaw(&'source str)` — a borrowed slice of the template source. No
runtime `Value` can reach it. Every value-bearing emit is `Instruction::Emit`, generated at
three codegen sites: `{{ expr }}`, `{% filter %}` end, and `{% call %}`.

One trap in that mechanism: `Emit` has a fast path that calls the default escape function
directly, skipping the formatter. It is guarded by `is_default_formatter()`, and
`set_formatter` clears that flag. **The fast path is dead only once `set_formatter` has been
called** — so it is called unconditionally at environment construction, never behind a
condition.

### 4.2 The capture constructs desync the bind list — reject them at save time

`{% filter %}`, `{% set x %}…{% endset %}`, `{% macro %}` and `{% call %}` buffer their body
— during which inner `{{ }}` has already become `?` — and then emit that whole buffer as one
more value through the formatter. The literal string `"?"` is bound as a parameter:

```
{% filter upper %}{{ a }}{% endfilter %}
   SQL   : "?"                      one placeholder
   BINDS : ["EVIL' OR 1=1--", "?"]  two values
```

`{% call %}` measured worse: one placeholder, three binds.

This is not injection — no caller text reaches the SQL — but the bind list desyncs from the
placeholder count, which means values bound to the wrong positions or a driver arity error.
On StarRocks an arity mismatch **kills the connection**: `StmtParamsMismatch` is raised
client-side and a subsequent `SELECT 1` returns "Connection to the server is closed". egret
opens a fresh `Conn` per query so today's blast radius is one query, but a future connection
pool would inherit a poisoned connection.

**S-2 — there is a THIRD capture path, and it is not a tag.** A `{% for … recursive %}` loop
re-entered with `{{ loop(x) }}` used **as a value** rather than as the whole emit captures the
loop body and re-emits it, desyncing exactly as `{% filter %}` does. It was not named in the
original text and was found by reading the VM, not by review. So the capture set is four
things, not three: `{% filter %}`, `{% set x %}…{% endset %}`, `{% macro %}`/`{% call %}`, and
the recursive for-loop — and the last one **is rejected at publish** like the others
(`RenderError::RecursiveLoop`), detected on the `for` tag's tokens rather than by
`split_whitespace`, because `{% for x in (regions)recursive %}` needs no space before the
modifier.

**S-2 — `{% raw %}` is a scanner-desync vector.** Everything inside `{% raw %}…{% endraw %}`
is literal text to the lexer but looks like live template syntax to a byte scanner. Any rule
implemented as a byte scan over the template source therefore reads constructs the renderer
will never execute (over-rejection, the safe direction) and can be walked out of step with
what actually runs. This is the general reason §4.2's guarantee is stated as a **property of
the rendered output** — placeholder count equals bind count — rather than as a list of
forbidden spellings, and the reason the comparison gate in `check_publish` is behavioural
(render twice, once with typed opaque values and once with the same values plainised; any
divergence in SQL text, placeholder count or bind text is a bare comparison, whatever its
spelling). Five separate reviews during implementation each found a different byte-scanner
evasion; the measured spellings were `{%+` (the whitespace-PRESERVE marker), `{# {% #}`,
`(mode)`, `regions[0]`, `| first`, `is eq`, `{% raw %}` and a loop-header alias. **Every one
of them was the scanner disagreeing with the lexer**, which is why the answer was to stop
asking the scanner and start asking the lexer (see S-3 in §4.3). The scanner survives only as
serving-path enforcement, never as the thing the guarantee rests on.

`{% macro %}` and `{% call %}` disappear with the `macros` feature **when it is in fact
absent — see S-1 in §4.5, which is why they are rejected at parse level too rather than
assumed gone**. `{% filter %}` and `{% set x %}…{% endset %}` are parsed unconditionally and
no feature removes them, so publish-time validation is the only lever: a template containing
either is rejected.

Backing that with a property rather than a rule: every stored template is rendered at publish
time with representative values, and **the placeholder count must equal the bind count**. A
regression in the rejection rule is then caught by the invariant rather than by review.

### 4.3 What an endpoint looks like

```sql
SELECT region, SUM(amount) AS total
FROM sales
WHERE 1=1
  {% if region %} AND region = {{ region }} {% endif %}
  {% if from %}   AND day >= {{ from }}     {% endif %}
  {% if regions %} AND region IN (
      {% for r in regions %}{{ r }}{% if not loop.last %},{% endif %}{% endfor %}
  ) {% endif %}
GROUP BY region
ORDER BY {{ sort | ident }}
```

`region=EU` renders `AND region = ?` with one bound value. `regions=[EU,US,APAC]` renders
`IN (?, ?, ?)` with three. The `IN`-list problem — where `IN (?)` binds exactly one value —
falls out of the formatter for free; it needs no `foreach` construct of its own.

**S-3 — CLOSED.** An earlier revision of this document warned that this example would not
publish, because P3's placeholder scanner treated the whole inside of `{{ }}` as a parameter
*name*: `{{ sort | ident }}` came back as `UndeclaredPlaceholder("sort | ident")` and the loop
variable `{{ r }}` as `UndeclaredPlaceholder("r")`. That was fixed by asking the **lexer**
rather than by writing a better scanner — minijinja's `Template::undeclared_variables` answers
"which names does this template read", and it already excludes loop targets and
`{% set %}`/`{% with %}` bindings and already includes names read only by a tag, which the
byte scan could not see at all. The byte scan still answers the different question "where in
the SQL does a reference stand", now keyed on the raw occurrence text rather than a parsed
name, so `PlaceholderInsideQuotedText` is unchanged in substance. **The example above now
publishes verbatim, and there is a test that says so.** Confirmed live on 2026-08-16 against
StarRocks: `?regions=EU&regions=US` returned two grouped rows, a *single* `?regions=EU`
returned one (a single value resolves to a one-element list, not a scalar — see §5's
arity-vs-shape rule), and `?sort=` outside `allowed` was a 400.

### 4.4 Identifier positions

Identifiers cannot be bound: `ORDER BY ?` is a syntax error on both engines. One named,
narrow escape hatch exists — the `ident` filter — and it is the only way to emit text derived
from a parameter.

`{{ p | ident }}` requires that `p`'s `ParamSpec` is `ParamType::Enum` with an `allowed`
list, matches the caller's value against that list, and emits **the matching entry from
`allowed`** — the endpoint author's own string, never the caller's. Two equal strings with
different provenance; the one written out is always the author's. A value not in `allowed` is
a 400. `ident` on a non-enum parameter is rejected at publish.

### 4.5 Hardened environment

```toml
minijinja = { version = "2.24", default-features = false,
              features = ["builtins", "serde", "deserialization",
                          "std_collections", "debug", "fuel"] }
```

`macros` and `multi_template` are dropped; `fuel` is added because it is **not** default-on.
`json`, `urlencode`, `custom_syntax`, `loop_controls` and `unstable_machinery` stay off.
Leaving `custom_syntax` off also means no template author can change the `{{ }}` delimiters.

**S-1 — the feature list is a REDUCTION, not a BOUNDARY, and an earlier revision of this
section presented it as a guarantee.** Cargo's `resolver = "2"` unifies features across the
workspace: the moment any other member depends on `minijinja` with default features, `macros`
and `multi_template` are compiled back in *here*, with no change to this manifest entry and no
warning. So `{% macro %}` and `{% call %}` cannot be assumed absent from what the parser will
accept. What actually holds is **parse-level rejection at publish** (§4.2) plus the
placeholder-equals-bind invariant — both of which are properties of egret's own code and
cannot be re-enabled by a third party's dependency edit. Read the feature list as removing
attack surface when it happens to be removed, never as the reason a construct is safe.

At construction, in this order:

1. `remove_global("debug")` — the highest-value single call. `debug()` is
   `format!("{state:#?}")`; a probe recovered the entire template context plus an inventory of
   every registered filter, test and global. On a public endpoint that is context disclosure.
2. `remove_global("range")`, `remove_global("dict")`, `remove_global("namespace")`.
3. `remove_filter("pprint")` — the second disclosure vector (`Debug` repr of a value).
4. `set_undefined_behavior(UndefinedBehavior::Strict)` — a mistyped name becomes an error
   rather than silently binding an empty value.
5. `set_fuel(Some(…))` — the project's own render fuzzer uses 50 000. Exhaustion is a clean
   error (`OutOfFuel`), not truncation.
6. `set_recursion_limit(…)` — well under the 500 default; the setter clamps to `min(level,
   MAX_RECURSION)` so it can only be lowered.
7. `set_formatter(…)` — the binding formatter, unconditionally (§4.1).

Never call `set_loader`, `set_unknown_method_callback`, `add_function` or `add_global`; use
`add_template_owned` only. **The `loader` cargo feature is a no-op** — `set_loader` and
`path_loader` are compiled in unconditionally and cannot be removed, so a CI grep asserting
they are never called is part of this work, not a follow-up.

Render into a size-capped sink. minijinja has **no output-size limit**; fuel bounds
instructions, not bytes, and `'a' * 99_999_999` is legal under the 100 MB repeated-string cap.

**S-4 — this section originally also said "and under a wall-clock timeout". The render has
none, deliberately.** `render` is synchronous, so a wall clock around it needs
`spawn_blocking`, and expiry there does not stop the work — it detaches a runaway thread that
keeps burning a core with nothing left holding it. The render's bound is therefore **fuel +
the 1 MiB sink + a 1 MiB bind cap**, all three of which stop the work rather than stop waiting
for it. The wall clock lives one layer out, on the engine call: `port_impl::run_bounded` —
shared by `execute_as` and `execute_as_with_params`, which is why it cannot be present on one
path and missing on the other — wraps the connector future in
`tokio::time::timeout(limits.timeout, …)`, and expiry drops the future, which drops the
connection. Neither connector reads `limits.timeout` itself.

### 4.6 What the template can reach

Only the values inserted into the context. Attribute access is meaningful only for
`ValueRepr::Object` and dispatches to that object's own `get_value_by_str`; every other
variant returns `Undefined`. `ValueRepr` is a closed 13-variant enum with no type object, no
class pointer and no parent link. Default `Object::call_method` returns `UnknownMethod`, and
plain maps and strings have no methods.

Probed: `{{ a.__class__ }}` renders empty, `{{ a.upper() }}` errors with "unknown method".
**Python Jinja2's `__class__`/`__globals__`/`__subclasses__` escape class is unreachable by
construction, not by blocklist** — it depends on Python objects carrying a reflective type
graph, and a minijinja `Value` built from serde data carries only data.

One fallback to know: an unknown method is retried as a callable stored at that key on the
object. With scalar data nothing is callable, so it is inert — it becomes live only if a
callable is ever inserted into the context, which the rule in §4.5 forbids.

## 5. Parameter model and validation

`ParamSpec` is unchanged: `name`, `ty` (`String|Number|Boolean|Date|Timestamp|Enum`),
`location`, `required`, `default`, `allowed`, `description`.

**At publish time**, in addition to P3's existing checks (duplicate name, undeclared
placeholder, unreferenced parameter, `PlaceholderInsideQuotedText`):

- The template parses under the hardened environment.
- It contains no `{% filter %}` and no `{% set x %}…{% endset %}` (§4.2).
- Rendered with representative values, **placeholder count equals bind count** (§4.2).
- `| ident` is applied only to an `Enum` parameter carrying `allowed` (§4.4).
- No parameter appears in `LIMIT`/`OFFSET` (§3.2).
- A parameter in the projection is wrapped in a `CAST` (§3.4).
- `describe` runs through the parameter-aware path and returns a non-empty column list;
  an empty list is a publish error, never a frozen empty schema (§3.4).
- **`location: body` is refused.** Nothing on the serving surface parses a request body, so
  a body parameter is never supplied. Required, that is a permanent 400 — loud. Optional, it
  resolves as absent, its `{% if %}` block is omitted, and the endpoint answers **200 with
  unfiltered rows, forever**, with nothing in the response to say a predicate went missing.
- **`default` is coerced by the same function the serving path uses**, and must satisfy its
  own `ty`. A `date` parameter defaulted to `"yesterday"` otherwise makes every call that
  omits it a 400 `param_invalid` — blaming the caller for the author's mistake, and recording
  it in `access_log` under a code that points at nothing.
- **Every optional parameter with no `default` is rendered ABSENT, on its own.** Absence is
  what most calls carry, and it is a second contract rather than a variation on the first: an
  absent parameter is `none`, so `{{ sort | ident }}` outside an `{% if %}` renders fine with
  a value and is a 500 without one. A definition that does not render with the parameter
  missing does not publish.

The rules that need a rendered statement are enforced by rendering it, not by scanning the
template: `check_publish` (§4.2's behavioural gate) runs at publish and only at publish —
it renders up to sixty-four times, which is right once per version and absurd per request.
The serving path keeps the source rules and the placeholder==bind invariant.

**At serve time**, before rendering:

- Parameter names are **percent-decoded before matching** against `ParamSpec.name`. Today
  they are matched raw, so `?d%61y=` is dropped and shares a cache entry with a request that
  has no `day` at all — inert while nothing renders, wrong rows the moment this phase lands.
  The landmine is recorded in `serving/router.rs` and this is its disarming.
- Parameters the client sends that are not declared are ignored, preserving P4 behaviour and
  not breaking clients that append tracking parameters.
- Values are type-checked per `ParamSpec.ty`. `Date`/`Timestamp` parse to real temporal
  values and bind as typed parameters, not strings.
- A missing required parameter is a 400; a value failing its type is a 400.
- A **repeated** query parameter becomes a LIST, one bind per value — `ParamSpec` has no
  multiplicity field, so multiplicity comes from the request. A repeated `Enum` is a 400: an
  enum is a single choice and the only type that can reach an identifier position.
- **Arity is the caller's; SHAPE is the definition's.** A parameter the definition iterates
  (`{% for r in regions %}`) is a list even when one value is supplied, because a scalar
  there is not a one-element loop — it is "plain is not iterable", a 500 on the ordinary
  request `?regions=EU`. Publish infers the same set from the same function, so the shape a
  definition is validated against is the shape it is served.
- **S-5 — `+` decodes to a SPACE in both names and values**, the
  `application/x-www-form-urlencoded` reading that `axum` and mainstream clients produce. This
  silently changes a `String` value rather than failing: `?token=a+b` binds `"a b"`. Anything
  whose alphabet contains `+` — standard base64, a `C++`-shaped literal, a signed number — is
  corrupted unless the client sends `%2B`. **Measured live on 2026-08-16**, against rows
  holding both spellings: `?region=EU+X` and `?region=EU%20X` each returned the `EU X` row,
  and only `?region=EU%2BX` returned the `EU+X` row. This is the documented behaviour, not a
  defect to be fixed on the server: changing it would break the `+`-means-space reading every
  HTML form and HTTP client library already produces. **Client-facing endpoint documentation
  must say it**, because nothing in the response distinguishes "you got the row you asked for"
  from "you asked for a different value than you typed".

**Errors never name the parameter.** The name is the endpoint author's, and naming it turns
the error surface into a directory of which parameters an endpoint declares. This restates
§7 of the base spec, now that those codes are reachable.

An **optional parameter with no value simply omits its `{% if %}` block**, which is why the
publish-time rule requiring `required`-or-`default` — considered during design — is not
adopted: conditionals make it unnecessary, and it would have forbidden the most common REST
shape, an optional filter.

## 6. Cache key

§6.3's two rules survive unchanged: every field length-prefixed, principal last. The key
covers `endpoint_id ‖ version ‖ declared params ‖ principal`, and the **absence** of a
parameter is representable and distinct from its presence — a call with `region` and a call
without must not share an entry, because they now execute different SQL.

## 7. Compatibility

An endpoint published before this phase that declares parameters starts rendering them.
Publish-time validation is where every new rule lives, so an old snapshot bypasses all of
them — a `location: body` parameter that silently drops a filter, a `default` that makes
every defaulted call a 400, a `| ident` that is a 500 whenever the caller omits its value.

This was originally written as an instruction — "every endpoint with parameters must be
re-published after P4.5" — and an instruction is not an enforcement. **The snapshot records
which revision of the gate it passed** (`EndpointSnapshot::param_gate`, stamped by publish
and nothing else), and a version that declares parameters and carries less than the current
`PARAM_GATE` is refused at serve time with a 503 `param_gate_stale` — before it is keyed,
cached or rendered. A pre-P4.5 snapshot deserialises to `0` and so is refused; one re-publish
clears it. A version with no parameters renders nothing and is unaffected.

The same mechanism carries the *next* migration: when a future revision of the gate closes a
rule this one misses, bumping `PARAM_GATE` refuses every snapshot the older gate blessed
until it has been through the new one. That is the reason the field is an integer and not a
boolean — "some gate ran" is precisely the claim that was not good enough this time.

## 8. What stays out of scope

- **Writes.** Unchanged from §10.
- **A MyBatis-shaped tag set.** `{% if %}`/`{% for %}` cover it; `<set>` is for writes,
  `<bind>` is evaluation, and `<where>`/`<trim>` are ergonomics over `WHERE 1=1`.
- **Migrating the other two interpolating surfaces.** There are **three** SQL-building
  surfaces after this phase, not two. `egret-bi/src/chartquery.rs::sql_literal` interpolates
  chart and dashboard filter values — the surface where the P1 backslash vector was actually
  reachable with `chart:read`. `egret-query/src/execution/template.rs` interpolates SQL Lab
  parameters; its own doc says it is "deliberately NOT a general template engine". Both are
  `quote_string` call sites and both were call sites of the P1 bug. Migrating them is the
  larger prize; BI alone touches 19 charts and a large test surface, and mixing it in here
  would force one review to hold injection analysis and BI regression analysis at once.
  **Until they land, egret carries two safety models side by side — Data API safe by
  construction, the other two safe by escaper — and that must be stated at the top of each
  interpolating module, not only here: a reader of `chartquery.rs` has no reason to open this
  document.**

  Note also a live documentation trap: `egret-dataapi/src/lifecycle.rs` says at `:551`,
  `:592` and `:629` that P3's validation matches "what `template::render` will do at serve
  time". **Nothing on the Data API serving path ever called it** — that renderer is SQL Lab's.
  P3's rules are correct and `PlaceholderInsideQuotedText` is what makes binding possible at
  all; only the comments' account of live behaviour is wrong. This phase fixes them.

## 9. Acceptance

**The criterion that matters most.** For **each** engine, a round-trip test that binds a
date, a decimal, a zero-padded string (`"007"`), a `NULL`, a backslash and a single quote,
and asserts the **exact** JSON returned. Run against the live engine. **Stub the
implementation once and confirm the assertion actually fails.**

The date regression (§3.3) corrupts data while every existing unit test stays green, because
the function those tests cover is no longer called. A test never observed failing proves
nothing.

Further:

- Trino's hostile-value test asserts the result is **data**, not that there was no error: a
  well-formed injected scalar expression parses fine and never surfaces as a syntax error.
- A template with `{% filter %}` is rejected at publish, and the placeholder-equals-bind
  invariant holds for every stored template.
- `describe` on parameterised SQL returns real columns on both engines, and an empty column
  list fails publish.
- A CI check asserts `set_loader` and `path_loader` are never called (§4.5).

**Live check, per §9 of the base spec.** Publish an endpoint with a parameter, call it with
two different values, and **confirm the results actually differ** — that is precisely what
does not work today and the reason this phase exists. Then call it with the parameter absent
and confirm the `{% if %}` block was omitted rather than bound as `NULL`.

**PERFORMED 2026-08-16 against StarRocks** (dev, `localhost:9030`), full transcript in
`.superpowers/sdd/2026-08-15-data-api-p45-parameters/task-8-report.md`. Summary: `?region=EU`
and `?region=US` returned disjoint row sets; `region` absent returned every row, so the
`{% if %}` block was omitted rather than bound as `NULL` (which would have returned zero);
`?d=2020-02-29` matched the leap day, so the date bound as a date; `?regions=EU&regions=US`
rendered `IN (?, ?)` and a single `?regions=EU` rendered `IN (?)` rather than 500;
`describe_as_with_params` returned five typed columns from the publish call site on a
template full of `{% if %}` tags; and a snapshot with `param_gate` stripped was refused
**503 `param_gate_stale`**. Hostile values (`EU' OR '1'='1`, a trailing `\`) came back as
**data** — zero rows, HTTP 200 — never as an error and never as extra rows.

**ALSO PERFORMED 2026-08-16 against Trino** (Trino 481, a coordinator built to the shape egret
itself provisions — see below). The same sequence, same results: `?region=EU` → ids `{1,2}` and
`?region=US` → ids `{3,4}`; `region` absent → all five rows; `?d=2020-02-29` → the two leap-day
rows; the `IN`-list at 2, 1 and 3 values with §4.3's canonical example published verbatim; and
`describe_as_with_params` returning five typed columns from the publish call site through
Trino's `DESCRIBE OUTPUT`.

Two things this run added that the StarRocks one could not:

1. **The rendered `EXECUTE … USING` text was read back off the coordinator**
   (`system.runtime.queries`), so §3.1's claim is observed rather than inferred. Trino received
   `EXECUTE p1 USING 'EU', DATE '2020-02-29'` — a *typed* date literal — and, for the payload
   `a' || 'b`, `EXECUTE p1 USING 'a'' || ''b', 'a'' || ''b'`. **The quote is doubled.** Broken
   escaping renders `'a' || 'b'`, which Trino evaluates to `"ab"` with a 200 and no error; the
   value came back as the literal 8-byte string instead. `x' || current_user || 'y` likewise
   round-tripped as 24 bytes rather than leaking the engine principal, and a trailing backslash
   survived as one byte — Trino does not treat it as an escape, so it must *not* be doubled
   here, the opposite of the StarRocks rule.
2. **Task 4's spelling divergence was re-measured and normalization shown to be load-bearing at
   publish.** On this coordinator raw `DESCRIBE OUTPUT` returns `decimal(10,2)` while the
   execute response returns `decimal(10, 2)`. Two endpoints over the identical projection — one
   with a parameter, one without, i.e. the two different describe paths — both froze
   `decimal(10, 2)` into the snapshot. `varchar(20)` was identical either way.

**Environment note, because it is a live trap and not a code defect.** The dev docker
coordinator (`deploy/trino/etc/config.properties`) trusts *Keycloak's* JWKS with
`required-audience=trino`. egret's only engine-token issuer is `MachineIdpEngineMinter`, which
signs `aud: "engine"` with egret's own key; no `ConnOptions` field carries a static bearer, and
the non-delegated and impersonate arms of `delegation_for` send no bearer at all. **Nothing
egret can mint is acceptable to that coordinator**, so it can never serve a Data API request.
That file is stale relative to what egret itself provisions — the `demo-trino-coordinator`
ConfigMap in the kind cluster reads `key-file=<egret>/.well-known/jwks.json` with
`required-audience=engine`, which is the configuration this check used. Relatedly,
`POST /databases/{id}/test` builds its connection with `bearer_token: None`
(`registry/tester.rs`), so it reports `{"ok":false,"detail":"forbidden"}` against *any* JWT-only
Trino even when the published endpoint serves correctly — do not read that as a broken
connection.

## 10. Residual risk, stated rather than implied

- **Trino's `USING` list is rendered text.** A defect there evaluates arbitrary scalar
  expressions (`current_user`, `now()`). It cannot open a second statement or a subquery —
  measured — but it is not nothing, and it will not appear as a syntax error.
- **minijinja has no security policy.** Zero advisories today, verified across RustSec, GHSA,
  NVD and OSV with control queries. But there is no `SECURITY.md` and no disclosure channel,
  and process-aborting bugs reachable from template text have shipped as ordinary changelog
  entries — 2.21.0 fixed a stack overflow from repeated sequence concatenation that core-dumped
  after ~1500 concatenations. Pin the version, watch the changelog, and do not assume an
  abort cannot take a process down.
- **No output-size limit exists in the crate.** §4.5's capped sink and timeout are the only
  defence.
- **`path_loader` cannot be compiled out.** Filesystem reach is one accidental call away,
  permanently; the CI grep is the control.
- **minijinja 3.0 moves the defaults** — the alpha already drops `serde` and `deserialization`
  from the default feature list. Re-run the surface audit before upgrading rather than
  assuming it carried over.
- **The trust boundary is unchanged by all of this.** Template authors write raw SQL by
  design; `EmitRaw` is that channel and always was. Every hardening above protects against
  the **API client**, whose values provably cannot escape the formatter. Who may author and
  publish an endpoint is doing the real security work, and that lives in egret's
  authorization, not in minijinja.
