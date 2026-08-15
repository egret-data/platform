//! Rendering an endpoint's SQL template into `(sql, Vec<Param>)`.
//!
//! **This is the injection surface of the whole Data API, and the reason it is
//! not one.** Everything else in the serving chain decides *whether* a request
//! may run; this module decides what SQL it runs, from text the endpoint author
//! wrote and values the caller sent.
//!
//! # Why a template engine at all, and why this one
//!
//! [`minijinja::Environment::set_formatter`] replaces value emission wholesale:
//!
//! ```text
//! Fn(&mut Output, &State, &Value) -> Result<(), Error>
//! ```
//!
//! egret's formatter ([`bind_formatter`]) writes a `?` to the output and pushes
//! the value onto a bind list. There is then **no path from `{{ … }}` to raw SQL
//! text** — an endpoint author cannot write an interpolating template even
//! deliberately, because the interpolating code path no longer exists. That is a
//! structural property, not a convention, and it is why a template engine was
//! chosen over a bespoke `{{name}}` substitution like the one SQL Lab still uses
//! (`egret-query`'s `execution::template`, which quotes-and-interpolates).
//!
//! The VM writes to the output in exactly two places. The raw one is
//! `Instruction::EmitRaw(&'source str)` — a borrowed slice of the *template
//! source*, so no runtime value can reach it. Every value-bearing write is
//! `Instruction::Emit`, which goes through the formatter. One trap in that
//! mechanism, and the reason [`environment`] has the shape it has: `Emit` has a
//! fast path that calls the default escape function directly and skips the
//! formatter. It is guarded by `is_default_formatter()`, a flag that only clears
//! when `set_formatter` is called. **`set_formatter` is therefore called
//! unconditionally at construction — behind a condition it would be a bypass.**
//!
//! # Two safety models live in this codebase at once
//!
//! After P4.5, egret has three SQL-building surfaces and only this one is safe by
//! construction. `egret-bi/src/chartquery.rs` and
//! `egret-query/src/execution/template.rs` both interpolate through
//! `quote_string` — safe by escaper, and both were call sites of the P1
//! backslash vector. Do not read this module as describing them.
//!
//! # The one deliberate hole: [`ident_filter`]
//!
//! Identifiers cannot be bound — `ORDER BY ?` is a syntax error on Trino and on
//! StarRocks — so `{{ p | ident }}` emits text. It emits **the matching entry
//! from the author's own `allowed` list**, never the caller's string. Two equal
//! strings with different provenance; the one written out is always the
//! author's.
//!
//! # What the template can reach
//!
//! Only the values inserted into the context. `ValueRepr` is a closed 13-variant
//! enum with no type object, no class pointer and no parent link; `{{ a.__class__ }}`
//! renders nothing and `{{ a.upper() }}` errors. Python Jinja2's
//! `__class__`/`__globals__`/`__subclasses__` escape class is unreachable **by
//! construction**, not by blocklist — it depends on Python objects carrying a
//! reflective type graph, and a minijinja value built from data carries only
//! data.
//!
//! Keeping it that way is a rule about this file: never call `set_loader`,
//! `path_loader`, `set_unknown_method_callback`, `add_function` or `add_global`,
//! and never insert a callable into the context. An unknown method is retried as
//! a callable stored at that key on the object — inert only while nothing
//! callable is there. `tests/template_render.rs` asserts the loader half of that
//! rule across the whole workspace, because the `loader` cargo feature is a
//! **no-op**: `set_loader` and `path_loader` are compiled in unconditionally and
//! cannot be removed by feature selection.
//!
//! # Known ergonomic limitation, stated because it is silent
//!
//! Every parameter enters the context as a [`Cell`] — an opaque object — because
//! that is what carries a `Date` through the template as a `Date` rather than as
//! a string (see [`Cell`]'s own comment). minijinja compares an object against a
//! string as **not equal**, so `{% if mode == 'fast' %}` would be silently false
//! forever. [`check_static`] rejects every comparison-shaped form it can see and
//! the `value` filter is the supported spelling: `{% if mode | value == 'fast' %}`.
//!
//! "Comparison-shaped" is wider than the operators. `eq`/`equalto`/`ne`/`lt`/
//! `le`/`gt`/`ge`/`in`/`sameas` are registered as **tests** routing to the same
//! `ops::coerce`, so `mode is eq('fast')` is `==` under another spelling; the
//! type tests (`is string`, `is none`) are silently false on a parameter for the
//! same reason; and a comparison is just as silent inside `{% set %}`, inside a
//! for-loop's filter condition and inside a bare `{{ … }}` as inside `{% if %}`.
//! So the lint reads every tag and every expression, `is <test>` is refused
//! unless the test is `defined`/`undefined`, and the only `in` that survives is
//! the one in a `{% for %}` header.
//!
//! A rename does not defeat it: `{% with m = mode %}`, `{% set m = mode %}` and
//! a `{% for r in regions %}` loop variable all bind a parameter's value to a new
//! name, and those names are linted too.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use egret_dataapi_api::{ParamSpec, ParamType};
use egret_query_api::Param;
use minijinja::value::{Object, ObjectRepr, Value};
use minijinja::{Environment, Error as MjError, ErrorKind as MjErrorKind, Output, State};

/// The bind marker written for every emitted value. Both engines spell it `?`.
const PLACEHOLDER: &str = "?";

/// Instruction budget for one render. Exhaustion is a clean `OutOfFuel` error,
/// not a truncated query. `fuel` is not a default feature of minijinja and is
/// named explicitly in the workspace manifest.
const FUEL: u64 = 50_000;

/// Well under minijinja's default of 500. The setter clamps to
/// `min(level, MAX_RECURSION)`, so this can only ever lower the limit.
const RECURSION_LIMIT: usize = 64;

/// Cap on the rendered SQL. minijinja has **no output-size limit of its own** —
/// fuel bounds instructions, not bytes, and `{{ 'a' * 99_999_999 }}` is legal
/// under its 100 MB repeated-string cap. A published statement is a few KB; a
/// megabyte is four orders of magnitude of headroom and still bounded.
const MAX_SQL_BYTES: usize = 1 << 20;

/// Everything [`render`] can refuse, as a value rather than a string, so callers
/// (and the tests) can match the specific rule that fired.
///
/// The two that carry a privilege boundary are [`RenderError::CaptureConstruct`]
/// and [`RenderError::PlaceholderBindMismatch`]: the first is the rule, the
/// second is the invariant behind it.
///
/// **These messages are for an endpoint AUTHOR, not for a caller.** Several of
/// them quote the definition back (a syntax error names the construct and the
/// line), which is the right thing at publish time and context disclosure on a
/// public endpoint. The serving path must map them to a generic failure; only
/// [`RenderError::Ident`], which deliberately names neither the parameter nor the
/// value, is safe to surface as-is.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RenderError {
    #[error(
        "`{{% {0} %}}` may not appear in a published definition. It buffers its body — inside \
         which every `{{{{ … }}}}` has ALREADY become a bind marker — and then emits that whole \
         buffer as one more value, so the literal text `?` is bound as a parameter and the bound \
         values desync from the placeholders. Rewrite the definition without it"
    )]
    CaptureConstruct(&'static str),
    #[error(
        "a `recursive` for-loop may not appear in a published definition: `{{{{ loop(…) }}}}` used \
         as a value captures the loop body, which desyncs the bound values from the placeholders \
         exactly as `{{% filter %}}` does"
    )]
    RecursiveLoop,
    #[error(
        "parameter `{0}` is compared directly against another value. A parameter carries its SQL \
         type through the template as an opaque value, and comparing one to a literal is silently \
         FALSE rather than an error — write `{0} | value` to compare it, e.g. \
         `{{% if {0} | value == 'x' %}}`"
    )]
    BareParameterComparison(String),
    #[error("the definition is not a valid template: {0}")]
    Template(String),
    #[error("the rendered SQL exceeds the {MAX_SQL_BYTES} byte cap")]
    TooLarge,
    #[error("the values bound by this definition exceed the {MAX_BIND_BYTES} byte cap")]
    BindsTooLarge,
    /// The invariant that survives a regression in [`check_static`]'s rules.
    ///
    /// Not merely tidiness: on StarRocks an arity mismatch is raised
    /// client-side as `StmtParamsMismatch` and the connection is then **dead** —
    /// a subsequent `SELECT 1` answers "Connection to the server is closed".
    /// egret opens a fresh `Conn` per query, so today's blast radius is one
    /// query; a future connection pool would inherit a poisoned connection.
    #[error(
        "internal: the rendered SQL carries {placeholders} bind markers but {binds} values were \
         bound. Refusing to send it: a parameter-arity mismatch binds values to the wrong \
         positions, and on StarRocks it kills the connection"
    )]
    PlaceholderBindMismatch { placeholders: usize, binds: usize },
    /// Never names the parameter and never echoes the caller's value: this
    /// reaches a public endpoint, where the error surface must not become a
    /// directory of what an endpoint declares.
    #[error(
        "`ident` may only be applied to an enum parameter, and only to one of its allowed values"
    )]
    Ident,
    #[error("the rendered SQL could not be scanned: {0}")]
    Unscannable(String),
}

/// A caller's values for one request, keyed by declared parameter name.
///
/// Built by the serving path from the request plus the snapshot's `ParamSpec`s
/// (Task 6), and by publish-time validation from representative values (Task 7).
///
/// # Absent is a value, not a gap
///
/// The environment runs with `UndefinedBehavior::Strict`, where an undefined
/// name errors even when merely tested for truth. A **declared but unsupplied**
/// optional parameter must therefore be inserted with [`ParamValues::insert_absent`],
/// which puts `none` in the context: `{% if region %}` is then false and the
/// block is omitted, which is the whole reason an optional parameter needs no
/// default. A name that was never declared stays undefined, so a typo in a
/// template is still an error rather than a silently-empty bind.
#[derive(Debug, Clone, Default)]
pub struct ParamValues {
    entries: BTreeMap<String, Value>,
}

impl ParamValues {
    pub fn new() -> Self {
        Self::default()
    }

    /// A supplied value for a non-enum parameter.
    pub fn insert(&mut self, name: impl Into<String>, value: Param) {
        self.entries
            .insert(name.into(), Value::from_object(Cell::new(value, None)));
    }

    /// A supplied value for an **enum** parameter, carrying the author's
    /// `allowed` list. This is the only way `allowed` enters a render, which is
    /// what makes [`ident_filter`]'s "must be an enum parameter" rule structural
    /// rather than a convention.
    pub fn insert_enum(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
        allowed: Vec<String>,
    ) {
        self.entries.insert(
            name.into(),
            Value::from_object(Cell::new(Param::Str(value.into()), Some(allowed))),
        );
    }

    /// A supplied multi-value parameter. Each element is bound separately, so
    /// `IN ({% for r in regions %}{{ r }}{% if not loop.last %},{% endif %}{% endfor %})`
    /// renders `IN (?, ?, ?)` — the `IN (?)`-binds-one-value problem falls out of
    /// the formatter and needs no construct of its own.
    pub fn insert_list(&mut self, name: impl Into<String>, values: Vec<Param>) {
        let cells: Vec<Value> = values
            .into_iter()
            .map(|p| Value::from_object(Cell::new(p, None)))
            .collect();
        self.entries.insert(name.into(), Value::from(cells));
    }

    /// A declared parameter the caller did not supply. See the type comment:
    /// this is not the same as leaving the name out.
    pub fn insert_absent(&mut self, name: impl Into<String>) {
        self.entries.insert(name.into(), Value::from(()));
    }

    /// Insert `value` under `spec.name`, attaching `allowed` **iff** the spec is
    /// an enum carrying one. Prefer this over [`ParamValues::insert`] /
    /// [`ParamValues::insert_enum`] where a `ParamSpec` is at hand: it is what
    /// keeps `| ident` refusing everything that is not an enum parameter.
    pub fn insert_for_spec(&mut self, spec: &ParamSpec, value: Param) {
        match (&spec.ty, &spec.allowed) {
            (ParamType::Enum, Some(allowed)) => self.entries.insert(
                spec.name.clone(),
                Value::from_object(Cell::new(value, Some(allowed.clone()))),
            ),
            _ => self.entries.insert(
                spec.name.clone(),
                Value::from_object(Cell::new(value, None)),
            ),
        };
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(|k| k.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn context(&self) -> Value {
        Value::from_iter(self.entries.iter().map(|(k, v)| (k.clone(), v.clone())))
    }
}

/// One parameter's value as the template sees it: opaque, with its SQL type
/// still attached.
///
/// # Why a wrapper rather than a plain string or number
///
/// The type has to survive the template. Trino refuses `day >= VARCHAR '2020-01-01'`
/// and a decimal bound as a float stops comparing equal to itself, which is why
/// Tasks 1-3 built [`Param::Date`], [`Param::Timestamp`] and [`Param::Decimal`]
/// at all. A minijinja value has nowhere to carry that: only `String`/`SmallStr`
/// compare with strings, and every type-carrying representation is opaque. So
/// the wrapper is what makes typed binding possible — and the cost is the
/// comparison limitation described in this module's header.
///
/// It is deliberately inert: `ObjectRepr::Plain`, no attributes (the default
/// `get_value` returns `None`, which under Strict makes `{{ a.anything }}` an
/// error), no methods, not callable.
#[derive(Debug)]
struct Cell {
    param: Param,
    /// `Some` only for an enum parameter — see [`ident_filter`].
    allowed: Option<Vec<String>>,
}

impl Cell {
    fn new(param: Param, allowed: Option<Vec<String>>) -> Self {
        Self { param, allowed }
    }

    /// The plain minijinja value behind this cell: what the `value` filter hands
    /// back so a template can compare a parameter against a literal.
    fn plain(&self) -> Value {
        match &self.param {
            Param::Null => Value::from(()),
            Param::Bool(b) => Value::from(*b),
            Param::I64(i) => Value::from(*i),
            Param::F64(f) => Value::from(*f),
            Param::Decimal(s) | Param::Str(s) | Param::Date(s) | Param::Timestamp(s) => {
                Value::from(s.clone())
            }
        }
    }

    fn text(&self) -> Cow<'_, str> {
        match &self.param {
            Param::Null => Cow::Borrowed(""),
            Param::Bool(true) => Cow::Borrowed("true"),
            Param::Bool(false) => Cow::Borrowed("false"),
            Param::I64(i) => Cow::Owned(i.to_string()),
            Param::F64(f) => Cow::Owned(f.to_string()),
            Param::Decimal(s) | Param::Str(s) | Param::Date(s) | Param::Timestamp(s) => {
                Cow::Borrowed(s.as_str())
            }
        }
    }
}

impl Object for Cell {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    /// Jinja truthiness of the underlying value, so `{% if region %}` means
    /// "the caller sent something" the way an author expects.
    fn is_true(self: &Arc<Self>) -> bool {
        match &self.param {
            Param::Null => false,
            Param::Bool(b) => *b,
            Param::I64(i) => *i != 0,
            Param::F64(f) => *f != 0.0,
            Param::Decimal(s) | Param::Str(s) | Param::Date(s) | Param::Timestamp(s) => {
                !s.is_empty()
            }
        }
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text())
    }
}

/// The result of `| ident`: author-owned text that the formatter writes into the
/// SQL **raw**.
///
/// The only way to construct one is [`ident_filter`], and the only string it
/// will ever hold is an entry cloned out of an endpoint author's `allowed` list.
#[derive(Debug)]
struct Ident {
    text: String,
}

impl Object for Ident {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// `{{ p | ident }}` — the one construct that may put parameter-derived text
/// into the SQL, and the reason it is safe: **it emits the author's string, not
/// the caller's.**
///
/// `p` must be an enum parameter carrying a non-empty `allowed` list, and the
/// caller's value must match one of its entries. The entry — the author's own
/// `String`, cloned out of the endpoint definition — is what gets written. The
/// caller's bytes are compared and then dropped.
///
/// Anything else is [`RenderError::Ident`], which names neither the parameter
/// nor the value.
fn ident_filter(value: &Value) -> Result<Value, MjError> {
    let cell = value
        .downcast_object_ref::<Cell>()
        .ok_or_else(|| MjError::new(MjErrorKind::InvalidOperation, IDENT_MSG))?;
    let allowed = cell
        .allowed
        .as_ref()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| MjError::new(MjErrorKind::InvalidOperation, IDENT_MSG))?;
    let supplied = match &cell.param {
        Param::Str(s) => s.as_str(),
        _ => return Err(MjError::new(MjErrorKind::InvalidOperation, IDENT_MSG)),
    };
    // `==` on the caller's bytes, then the AUTHOR's `String` is what is emitted.
    // Cloning `entry` rather than `supplied` is the whole security property of
    // this function; a mutation that emits `supplied` is caught by
    // `ident_emits_the_authors_string_not_the_callers`.
    let entry = allowed
        .iter()
        .find(|entry| entry.as_str() == supplied)
        .ok_or_else(|| MjError::new(MjErrorKind::InvalidOperation, IDENT_MSG))?;
    Ok(Value::from_object(Ident {
        text: entry.clone(),
    }))
}

const IDENT_MSG: &str =
    "`ident` may only be applied to an enum parameter, and only to one of its allowed values";

/// `{{ p | value }}` — the plain scalar behind a parameter, for comparisons.
///
/// Emitting one still binds (the formatter has no other mode), but the bind is
/// untyped: `{{ from | value }}` binds a `Date` as text. Compare with it; do not
/// emit with it.
fn value_filter(value: &Value) -> Value {
    match value.downcast_object_ref::<Cell>() {
        Some(cell) => cell.plain(),
        None => value.clone(),
    }
}

/// The values bound so far, and how many bytes they hold.
///
/// The byte count is why this is a struct: [`MAX_SQL_BYTES`] caps the SQL, but
/// the bind list is the other half of a render's memory and nothing bounded it.
/// `{{ 'a' * 20000000 }}` produces an 8-byte statement and 20 MB of bound value
/// in a few milliseconds — author-privileged rather than a caller vector, but it
/// runs at PUBLISH time too, where an author's mistake would take the process
/// with it.
#[derive(Default)]
struct Binds {
    list: Vec<Param>,
    bytes: usize,
}

/// Cap on the total size of the bound values. Request-sized by construction: a
/// caller's parameters travel in a query string or a small JSON body.
const MAX_BIND_BYTES: usize = 1 << 20;

impl Binds {
    fn push(&mut self, param: Param) -> Result<(), MjError> {
        self.bytes += match &param {
            Param::Decimal(s) | Param::Str(s) | Param::Date(s) | Param::Timestamp(s) => s.len(),
            _ => std::mem::size_of::<Param>(),
        };
        if self.bytes > MAX_BIND_BYTES {
            return Err(MjError::new(MjErrorKind::InvalidOperation, BINDS_TOO_LARGE));
        }
        self.list.push(param);
        Ok(())
    }
}

const BINDS_TOO_LARGE: &str = "the values bound by this definition exceed the size cap";

/// Writes `?` and binds. The only thing a `{{ … }}` can do.
///
/// The bind list is captured by the closure rather than carried in the template
/// context, so no context key exists for a template to collide with or read.
fn bind_formatter(
    binds: Arc<Mutex<Binds>>,
) -> impl Fn(&mut Output, &State, &Value) -> Result<(), MjError> + Send + Sync + 'static {
    move |out, _state, value| {
        // The one raw write in this module. See `Ident`: the text is the
        // author's, taken from the endpoint definition's `allowed` list.
        if let Some(ident) = value.downcast_object_ref::<Ident>() {
            return out.write_str(&ident.text).map_err(MjError::from);
        }
        let param = to_param(value)?;
        binds
            .lock()
            .expect("bind list mutex poisoned by a panic inside the renderer")
            .push(param)?;
        out.write_str(PLACEHOLDER).map_err(MjError::from)
    }
}

/// The value being emitted, as something the engine can bind.
///
/// A [`Cell`] keeps the type Tasks 1-3 gave it. Anything else is a value the
/// template computed — a concatenation, a captured buffer, a literal — and binds
/// by its minijinja kind. Sequences and maps are refused: emitting a whole list
/// as one bound value is never what an author meant, and `IN (?)` would take
/// exactly one of them.
fn to_param(value: &Value) -> Result<Param, MjError> {
    if let Some(cell) = value.downcast_object_ref::<Cell>() {
        return Ok(cell.param.clone());
    }
    use minijinja::value::ValueKind as K;
    Ok(match value.kind() {
        K::Undefined | K::None => Param::Null,
        K::Bool => Param::Bool(value.is_true()),
        K::Number => match i64::try_from(value.clone()) {
            Ok(i) => Param::I64(i),
            // Exact text rather than a lossy float: a decimal literal in a
            // template must not become `1234567.875` on its way to the engine.
            Err(_) => match f64::try_from(value.clone()) {
                Ok(f) => Param::F64(f),
                Err(_) => Param::Decimal(value.to_string()),
            },
        },
        K::String => Param::Str(value.to_string()),
        _ => {
            return Err(MjError::new(
                MjErrorKind::InvalidOperation,
                "this value cannot be bound as a single parameter; emit a sequence one element \
                 at a time inside a `{% for %}` loop",
            ))
        }
    })
}

/// The hardened environment. **Read [`render`]'s comment before changing the
/// order or removing a line.**
fn environment(binds: Arc<Mutex<Binds>>) -> Environment<'static> {
    let mut env = Environment::new();
    // `debug()` is `format!("{state:#?}")`. A probe against 2.24.0 recovered the
    // entire template context plus an inventory of every registered filter, test
    // and global. On a public endpoint that is context disclosure, and this is
    // the highest-value single call in this function.
    env.remove_global("debug");
    env.remove_global("range");
    env.remove_global("dict");
    env.remove_global("namespace");
    // The second disclosure vector: the `Debug` repr of a value.
    env.remove_filter("pprint");
    // A mistyped name is an error rather than a silently-empty bind. Note the
    // consequence for optional parameters, spelled out on [`ParamValues`].
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    env.set_fuel(Some(FUEL));
    env.set_recursion_limit(RECURSION_LIMIT);
    env.add_filter("ident", ident_filter);
    env.add_filter("value", value_filter);
    // UNCONDITIONAL, and last. `Instruction::Emit` has a fast path that calls the
    // default escape function and never reaches the formatter; it is guarded by
    // `is_default_formatter()`, which only clears when `set_formatter` is called.
    // Put this behind ANY condition and that condition becomes a bypass.
    env.set_formatter(bind_formatter(binds));
    env
}

/// Render `sql` against `ctx`, producing the statement to send and the values to
/// bind to it, in the order their markers appear.
///
/// ```text
/// WHERE 1=1 {% if region %} AND region = {{ region }} {% endif %}
///   region=EU  ->  ("WHERE 1=1  AND region = ?", [Str("EU")])
///   absent     ->  ("WHERE 1=1 ", [])
/// ```
///
/// Three things happen, in this order, and each is load-bearing:
///
/// 1. [`check_static`] refuses the constructs that would desync the bind list,
///    and the comparison form that would be silently false.
/// 2. The template renders under [`environment`] into a size-capped sink.
/// 3. **The invariant**: the number of bind markers standing in SQL structure
///    equals the number of values bound. A regression in step 1's rules is
///    caught here rather than by review.
pub fn render(sql: &str, ctx: &ParamValues) -> Result<(String, Vec<Param>), RenderError> {
    check_static(sql, ctx)?;

    let binds = Arc::new(Mutex::new(Binds::default()));
    let mut env = environment(binds.clone());
    env.add_template_owned("endpoint", sql.to_owned())
        .map_err(mj_err)?;
    let template = env.get_template("endpoint").map_err(mj_err)?;

    let mut sink = CappedSink::default();
    let outcome = template.render_captured_to(ctx.context(), &mut sink);
    if sink.overflowed {
        return Err(RenderError::TooLarge);
    }
    outcome.map_err(mj_err)?;

    let rendered = String::from_utf8(sink.buf).expect("minijinja writes UTF-8");
    // Taken, not unwrapped: the environment (and so the formatter's clone of the
    // `Arc`) is still alive here because `template` borrows it.
    let binds = std::mem::take(
        &mut binds
            .lock()
            .expect("bind list mutex poisoned by a panic inside the renderer")
            .list,
    );

    let placeholders = structural_placeholders(&rendered)?;
    if placeholders != binds.len() {
        return Err(RenderError::PlaceholderBindMismatch {
            placeholders,
            binds: binds.len(),
        });
    }
    Ok((rendered, binds))
}

/// Bind markers that stand in SQL structure — a `?` inside a string literal or a
/// comment is text to the engine and is not counted.
///
/// Counting the markers in the **rendered output** rather than counting the
/// formatter's own writes is the point: a `{% filter %}` body is buffered, so its
/// markers never reach the output while its values do reach the bind list.
/// Counting what the formatter wrote would make the invariant trivially true and
/// blind to exactly the case it exists for.
fn structural_placeholders(rendered: &str) -> Result<usize, RenderError> {
    // Both escaping readings, as everywhere else in this crate: which regions
    // count as quoted is the one thing Trino and StarRocks disagree about, and
    // the larger count is the safe one to compare against.
    let mut max = 0;
    for backslash_escapes in [true, false] {
        let stripped = crate::lifecycle::strip_comments_and_strings(rendered, backslash_escapes)
            .map_err(|e| RenderError::Unscannable(e.to_string()))?;
        max = max.max(stripped.matches(PLACEHOLDER).count());
    }
    Ok(max)
}

/// The rules that must hold before a template is allowed to render at all.
///
/// Exposed for publish-time validation (Task 7), and called by [`render`] itself
/// so a definition that slipped past publish still cannot serve.
pub fn check_static(sql: &str, ctx: &ParamValues) -> Result<(), RenderError> {
    // Names that are not parameters but hold one: `{% with m = mode %}` and a
    // loop variable both carry a `Cell`, so comparing THEM is silently false in
    // exactly the same way. Accumulated in source order, which is the order a
    // template can use them in.
    let mut aliases: Vec<String> = Vec::new();
    for block in blocks(sql) {
        let body = block.body();
        if let Block::Tag(_) = block {
            let keyword = body
                .split(|c: char| c.is_whitespace() || c == '(')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            // `{% filter %}` and `{% set x %}…{% endset %}` buffer their body and
            // re-emit it through the formatter, binding the literal text `?` as a
            // parameter: measured, `{% filter upper %}{{ a }}{% endfilter %}` gives
            // ONE placeholder and TWO binds, and `{% call %}` gave three. No cargo
            // feature removes those two — they parse unconditionally — so rejecting
            // them here is the only lever.
            //
            // `macro`/`call`/`include`/`extends`/`import`/`from`/`block` do not parse
            // at all with `macros` and `multi_template` off, and are listed anyway:
            // cargo features are ADDITIVE ACROSS THE WORKSPACE (the workspace is on
            // resolver 2), so one member adding minijinja with default features
            // would turn them back on here, silently, without touching this crate.
            let forbidden = match keyword.as_str() {
                "filter" => Some("filter"),
                "endfilter" => Some("endfilter"),
                "endset" => Some("endset"),
                "macro" | "endmacro" => Some("macro"),
                "call" | "endcall" => Some("call"),
                "include" => Some("include"),
                "extends" => Some("extends"),
                "import" | "from" => Some("import"),
                "block" | "endblock" => Some("block"),
                _ => None,
            };
            if let Some(name) = forbidden {
                return Err(RenderError::CaptureConstruct(name));
            }
            // A `recursive` for-loop is the third capture path: `{{ loop(x) }}` used
            // as a value (not as the whole emit) captures the loop body and re-emits
            // it, desyncing exactly as `{% filter %}` does. Not named in the spec;
            // found by reading the VM.
            if keyword == "for" && body.split_whitespace().any(|w| w == "recursive") {
                return Err(RenderError::RecursiveLoop);
            }
        }
        // EVERY block, not only `{% if %}`: a comparison against a bare parameter
        // is meaningless wherever it is written, and it was silently taking the
        // wrong branch from `{% set f = mode == 'fast' %}`, from a for-loop's
        // filter condition, and from a bare `{{ 'FAST' if mode == 'fast' else … }}`.
        if let Some(name) = bare_comparison(body, ctx, &aliases, matches!(block, Block::Tag(_))) {
            return Err(RenderError::BareParameterComparison(name));
        }
        if let Block::Tag(_) = block {
            collect_aliases(body, ctx, &mut aliases);
        }
    }
    Ok(())
}

/// Records the names a `{% set %}`, `{% with %}` or `{% for %}` binds to a value
/// that came from a parameter.
///
/// Without this, one rename defeats the comparison rule:
/// `{% with m = mode %}{% if m == 'fast' %}` is the same silent falsehood one
/// level of indirection away, and so is `{% for r in regions %}{% if r == 'EU' %}`
/// — a loop variable holds a [`Cell`] too.
fn collect_aliases(body: &str, ctx: &ParamValues, aliases: &mut Vec<String>) {
    let toks = tokens(body);
    let Some(keyword) = toks.first().filter(|t| t.kind == Tok::Word) else {
        return;
    };
    match keyword.text {
        "set" | "with" => {
            // `<name> = <expr>`: alias `<name>` when `<expr>` mentions a
            // parameter or an alias.
            for (i, tok) in toks.iter().enumerate() {
                if tok.kind != Tok::Other || tok.text != "=" {
                    continue;
                }
                let Some(lhs) = toks[..i].iter().rev().find(|t| t.kind == Tok::Word) else {
                    continue;
                };
                if toks[i + 1..]
                    .iter()
                    .any(|t| t.kind == Tok::Word && is_param(t.text, ctx, aliases))
                {
                    aliases.push(lhs.text.to_string());
                }
            }
        }
        "for" => {
            let Some(at) = toks
                .iter()
                .position(|t| t.kind == Tok::Word && t.text == "in")
            else {
                return;
            };
            if toks[at + 1..]
                .iter()
                .any(|t| t.kind == Tok::Word && is_param(t.text, ctx, aliases))
            {
                for t in &toks[1..at] {
                    if t.kind == Tok::Word {
                        aliases.push(t.text.to_string());
                    }
                }
            }
        }
        _ => {}
    }
}

/// A `{% … %}` tag or a `{{ … }}` expression, body only.
enum Block<'a> {
    Tag(&'a str),
    Expr(&'a str),
}

impl<'a> Block<'a> {
    fn body(&self) -> &'a str {
        match self {
            Block::Tag(b) | Block::Expr(b) => b,
        }
    }
}

/// Every tag and expression in the template, in order.
///
/// A scanner, not a parser — minijinja does the parsing — but it has to agree
/// with the lexer on three things it originally did not:
///
/// * **Whitespace markers.** `Whitespace::from_byte` maps `-` to Trim and `+` to
///   Preserve, and `find_start_marker` skips both. `{%+ filter upper %}` is a
///   perfectly ordinary filter block, and a scanner that trims only `-` reads its
///   keyword as `"+"` and matches nothing. Three of the four rules here were
///   backstopped by the placeholder==bind invariant; the comparison rule was not,
///   and `{%+ if mode == 'fast' %}` was a publishable silently-false template.
/// * **Comments.** `{# … #}` is skipped by the lexer, so `{# {% #}` contains no
///   tag — while a comment-blind scan pairs that `{%` with the NEXT `%}` and
///   desyncs for the rest of the template, stepping over a real `{% filter %}`.
/// * **String literals.** Inside a block the lexer tokenises strings, so `%}` and
///   `}}` inside one do not close it.
///
/// It still over-approximates — a `{% filter %}` written inside `{% raw %}` is
/// inert but rejected anyway — and over-approximating is the safe direction for a
/// rejection rule.
fn blocks(sql: &str) -> Vec<Block<'_>> {
    let b = sql.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] != b'{' {
            i += 1;
            continue;
        }
        let (close, is_tag) = match b[i + 1] {
            b'%' => ("%}", true),
            b'{' => ("}}", false),
            b'#' => {
                // A comment: skip it whole, so nothing inside is read as a tag.
                match sql[i + 2..].find("#}") {
                    Some(at) => i += 2 + at + 2,
                    None => break,
                }
                continue;
            }
            _ => {
                i += 1;
                continue;
            }
        };
        let body_start = i + 2;
        match find_block_end(sql, body_start, close) {
            Some(at) => {
                let body = trim_markers(&sql[body_start..at]);
                out.push(if is_tag {
                    Block::Tag(body)
                } else {
                    Block::Expr(body)
                });
                i = at + 2;
            }
            // Unterminated: minijinja refuses it as a syntax error, and there is
            // nothing further to scan.
            None => break,
        }
    }
    out
}

/// The offset of `close`, skipping over string literals the way the lexer does.
///
/// Compared as BYTES throughout. `sql[i..]` panics the moment `i` lands inside a
/// multi-byte character, and a definition carrying one — a `'café'` literal, a
/// Vietnamese comment — is ordinary SQL, not an edge case.
fn find_block_end(sql: &str, from: usize, close: &str) -> Option<usize> {
    let b = sql.as_bytes();
    let close = close.as_bytes();
    let mut i = from;
    while i < b.len() {
        match b[i] {
            q @ (b'\'' | b'"') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    // minijinja's lexer honours `\` escapes inside a string.
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            _ if b[i..].starts_with(close) => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Both whitespace-control markers, and the trailing one too.
fn trim_markers(body: &str) -> &str {
    body.trim_start_matches(['-', '+'])
        .trim_end_matches(['-', '+'])
        .trim()
}

/// The parameter compared directly against something, if any.
///
/// A parameter is an opaque value (see [`Cell`]), and minijinja compares an
/// object against a string as NOT EQUAL with no error, so every form recognised
/// here is silently false rather than wrong-and-loud. `| value` is the supported
/// spelling and passes, because the identifier this looks at is the one
/// **immediately** adjacent to the operator: in `region | value == 'EU'` that is
/// `value`, not `region`.
///
/// `in_tag` says whether a `for` header's `in` may appear — that one `in` is a
/// loop header, and every other `in` is the containment operator, which has the
/// same weakness as `==` and is refused.
fn bare_comparison(
    body: &str,
    ctx: &ParamValues,
    aliases: &[String],
    in_tag: bool,
) -> Option<String> {
    let toks = tokens(body);
    let is_for = in_tag && toks.first().is_some_and(|t| t.text == "for");
    let mut for_header_in_seen = false;
    for (i, tok) in toks.iter().enumerate() {
        match tok.kind {
            Tok::Op => {
                if let Some(name) = operand_param(&toks, i, ctx, aliases) {
                    return Some(name);
                }
            }
            Tok::Word if tok.text == "in" => {
                if is_for && !for_header_in_seen {
                    for_header_in_seen = true;
                    continue;
                }
                if let Some(name) = operand_param(&toks, i, ctx, aliases) {
                    return Some(name);
                }
            }
            // `eq`/`equalto`/`ne`/`lt`/`le`/`gt`/`ge`/`in`/`sameas` are registered
            // as TESTS routing to the same `ops::coerce`, so `mode is eq('fast')`
            // is `==` under a spelling no operator scan can see. The type tests
            // (`is string`, `is none`, `is number`) are silently false on a
            // parameter for the same reason. Only `defined`/`undefined` mean on a
            // parameter what an author expects, so they are the allowlist.
            Tok::Word if tok.text == "is" => {
                let subject = toks[..i].iter().rev().find(|t| t.kind == Tok::Word);
                let test = toks[i + 1..]
                    .iter()
                    .find(|t| t.kind == Tok::Word && t.text != "not");
                let benign = test.is_some_and(|t| matches!(t.text, "defined" | "undefined"));
                if benign {
                    continue;
                }
                let subject = subject
                    .map(|t| t.text)
                    .filter(|n| is_param(n, ctx, aliases));
                if let Some(name) = subject {
                    return Some(name.to_string());
                }
                // `'fast' is eq(mode)` — the parameter is the ARGUMENT.
                for t in &toks[i + 1..] {
                    if t.kind == Tok::Word && is_param(t.text, ctx, aliases) {
                        return Some(t.text.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// The parameter standing on either side of the operator at `i`, skipping a
/// `not` (`mode not in xs`).
fn operand_param(
    toks: &[Token<'_>],
    i: usize,
    ctx: &ParamValues,
    aliases: &[String],
) -> Option<String> {
    let mut before = i;
    while before > 0 && toks[before - 1].text == "not" {
        before -= 1;
    }
    for side in [before.checked_sub(1), Some(i + 1)].into_iter().flatten() {
        if let Some(t) = toks.get(side) {
            if t.kind == Tok::Word && is_param(t.text, ctx, aliases) {
                return Some(t.text.to_string());
            }
        }
    }
    None
}

fn is_param(name: &str, ctx: &ParamValues, aliases: &[String]) -> bool {
    ctx.names().any(|n| n == name) || aliases.iter().any(|a| a == name)
}

#[derive(PartialEq, Clone, Copy)]
enum Tok {
    Word,
    Op,
    Other,
}

struct Token<'a> {
    text: &'a str,
    kind: Tok,
}

/// Identifiers, comparison operators and everything else, with string literals
/// dropped so `'a == b'` is text rather than a comparison.
fn tokens(body: &str) -> Vec<Token<'_>> {
    let b = body.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'\'' || c == b'"' {
            i += 1;
            while i < b.len() && b[i] != c {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
        } else if c.is_ascii_alphanumeric() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Token {
                text: &body[start..i],
                kind: Tok::Word,
            });
        } else {
            let len = match (c, b.get(i + 1)) {
                (b'=', Some(b'=')) | (b'!', Some(b'=')) => 2,
                (b'<', Some(b'=')) | (b'>', Some(b'=')) => 2,
                (b'<', _) | (b'>', _) => 1,
                _ => 0,
            };
            if len > 0 {
                out.push(Token {
                    text: &body[i..i + len],
                    kind: Tok::Op,
                });
                i += len;
            } else {
                // A non-ASCII byte lands here too, and `&body[i..i + 1]` would
                // panic on one. Every operator and every parameter name is ASCII
                // (P3 refuses a placeholder name that is not), so a multi-byte
                // character can only ever be an opaque separator — which is
                // exactly what an empty `Other` token is.
                out.push(Token {
                    text: if c.is_ascii() { &body[i..i + 1] } else { "" },
                    kind: Tok::Other,
                });
                i += 1;
            }
        }
    }
    out
}

/// A sink that stops accepting bytes past [`MAX_SQL_BYTES`].
///
/// minijinja has no output-size limit and fuel does not provide one, so this is
/// the only bound on how much text a render can produce.
#[derive(Default)]
struct CappedSink {
    buf: Vec<u8>,
    overflowed: bool,
}

impl std::io::Write for CappedSink {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.overflowed || self.buf.len() + data.len() > MAX_SQL_BYTES {
            self.overflowed = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "rendered SQL exceeds the size cap",
            ));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// minijinja's error, flattened with its source chain.
///
/// The chain matters: a filter's own message (`ident` refusing a value) is the
/// `source`, and the top-level display is only "invalid operation".
fn mj_err(e: MjError) -> RenderError {
    if chain_contains(&e, IDENT_MSG) {
        return RenderError::Ident;
    }
    if chain_contains(&e, BINDS_TOO_LARGE) {
        return RenderError::BindsTooLarge;
    }
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = std::error::Error::source(s);
    }
    RenderError::Template(msg)
}

fn chain_contains(e: &MjError, needle: &str) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if err.to_string().contains(needle) {
            return true;
        }
        cur = std::error::Error::source(err);
    }
    false
}
