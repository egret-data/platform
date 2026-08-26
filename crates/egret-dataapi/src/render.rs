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
//! **The rule is enforced twice, and only one of the two is a scanner.** At
//! publish, [`check_dynamic`] measures it from BEHAVIOUR — it renders the
//! definition with the opaque values and again with plain ones, and an
//! inspection takes a different branch whatever its spelling. Read that
//! function's comment for why: five separate reviews found five bypasses of the
//! byte scanner, every one of them this module disagreeing with minijinja's
//! lexer about the same bytes, and the sixth spelling exists but is not written
//! down yet. The scanner below is the fast first line and is precise where it
//! fires; the gate is what the rule actually rests on.
//!
//! "Comparison-shaped" is wider than the operators. `eq`/`equalto`/`ne`/`lt`/
//! `le`/`gt`/`ge`/`in`/`sameas` are registered as **tests** routing to the same
//! `ops::coerce`, so `mode is eq('fast')` is `==` under another spelling; the
//! type tests (`is string`, `is none`) are silently false on a parameter for the
//! same reason; and a comparison is just as silent inside `{% set %}`, inside a
//! for-loop's filter condition and inside a bare `{{ … }}` as inside `{% if %}`.
//! So the lint reads every tag and every expression, `is <test>` is refused
//! unless the test is `defined`/`undefined`, the only `in` that survives is the
//! one in a `{% for %}` header, and the operand it reads is the whole region
//! around the operator — `(mode)`, `regions[0]` and `regions | first` are the
//! same silence as a bare `mode`.
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
    #[error(
        "a `{{% raw %}}` block is never closed. minijinja refuses it as a syntax error, and the \
         scanner that enforces every rule above cannot tell live template text from raw text \
         past that point — so it is refused here rather than scanned on a guess"
    )]
    UnterminatedRaw,
    /// The behavioural gate. Deliberately does not name a construct: it is
    /// detected by rendering the same definition twice and watching the SQL
    /// change, so there is no token to point at.
    #[error(
        "this definition INSPECTS a parameter rather than using it as a value — a comparison, a \
         test, a subscript or an index whose result changes depending on how the value is \
         represented. A parameter carries its SQL type as an opaque value, so `p == 'x'` and \
         everything shaped like it is silently FALSE. Write `p | value` to compare. Detected by \
         rendering with and without the wrapper and watching the SQL diverge, so the spelling is \
         not what triggers it"
    )]
    ParameterInspected,
    #[error("the rendered SQL exceeds the {MAX_SQL_BYTES} byte cap")]
    TooLarge,
    /// The one thing `| ident` can put into a statement that a caller cannot:
    /// AUTHOR text, unscanned. See [`check_rendered_statement`].
    #[error(
        "the rendered statement carries a statement separator `;` outside a trailing position. \
         The definition itself cannot contain one — publish refuses it by the same byte test — \
         so it arrived through `| ident`, which writes an `allowed` entry into the SQL as text. \
         On StarRocks the serving path sends this over COM_QUERY, which executes every statement \
         in one submission, so a second statement here is arbitrary DDL"
    )]
    RenderedSeparator,
    #[error(
        "the rendered statement carries `#`, which StarRocks ACCEPTS AND IGNORES as a comment \
         while Trino has no such token. The definition cannot contain one, so it arrived through \
         `| ident` — the same path, the same refusal"
    )]
    RenderedHash,
    /// A sequence or map reached the formatter as one value.
    ///
    /// Its own variant rather than a [`RenderError::Template`] carrying a
    /// message, because the serving path has to tell this ONE caller-triggered
    /// render failure from every author-fault one — a repeated query parameter
    /// resolves to a list — and matching on the text of a message is the check
    /// that keeps compiling after someone rewords it.
    #[error(
        "this value cannot be bound as a single parameter; emit a sequence one element at a time \
         inside a `{{% for %}}` loop"
    )]
    SequenceBind,
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

    /// The same values as plain scalars — what the behavioural gate in
    /// [`check_publish`] renders against. Private: the serving path must never
    /// render this way, because a `Date` bound from a plain value binds as text.
    fn plainified(&self, keep_opaque: &[String]) -> ParamValues {
        ParamValues {
            entries: self
                .entries
                .iter()
                .map(|(k, v)| {
                    let v = if keep_opaque.iter().any(|n| n == k) {
                        v.clone()
                    } else {
                        plainify(v)
                    };
                    (k.clone(), v)
                })
                .collect(),
        }
    }

    /// The names of the enum parameters, in order.
    fn enum_names(&self) -> impl Iterator<Item = String> + '_ {
        self.entries.iter().filter_map(|(k, v)| {
            v.downcast_object_ref::<Cell>()
                .and_then(|c| c.allowed.as_ref())
                .map(|_| k.clone())
        })
    }

    /// Candidate values for probing `name`: an enum parameter's own `allowed`
    /// entries, or the definition's literals — plus, for a boolean parameter,
    /// its two closed values appended to those literals.
    fn candidates(&self, name: &str, literals: &[Param]) -> Vec<Param> {
        let cell = self
            .entries
            .get(name)
            .and_then(|v| v.downcast_object_ref::<Cell>());
        match cell.and_then(|c| c.allowed.clone()) {
            // Capped only by the shared probe budget — see
            // [`MAX_ENUM_CANDIDATES`], which is that budget. `allowed` has no
            // length limit anywhere in the model, so the cap still has to
            // exist; what it must not do is stop short of the budget, because
            // an allowed value publish never probes is a branch it never
            // describes.
            Some(list) => list
                .into_iter()
                .take(MAX_ENUM_CANDIDATES)
                .map(Param::Str)
                .collect(),
            None => {
                // A boolean's value space is closed and has exactly two members,
                // so both are appended unconditionally rather than hoping the
                // definition mentions `true`/`false` as literals.
                // `{% if detail %}, name {% endif %}` contains no literal at
                // all — without this, `detail` is only ever probed at its own
                // (usually `true`) value, and the `false` branch is never
                // rendered. This must be an APPEND, not a replacement: a
                // boolean can also be inspected against a string literal in a
                // spelling `check_static` cannot see (`(detail) == 'slow'`),
                // and the literal harvest is check_dynamic's only route to
                // that value. An early return that skipped `literals` would
                // silently drop that detection for every boolean parameter.
                let mut candidates = literals.to_vec();
                if matches!(cell.map(|c| &c.param), Some(Param::Bool(_))) {
                    candidates.push(Param::Bool(true));
                    candidates.push(Param::Bool(false));
                }
                candidates
            }
        }
    }

    /// Replaces one value, keeping its SHAPE — enum-ness, and list-ness.
    ///
    /// Shape matters more than it looks: substituting a scalar for a list
    /// parameter makes `regions[0]` and `regions | first` fail to render, and a
    /// probe that fails to render is inconclusive rather than a finding — so the
    /// gate silently stopped covering exactly the two spellings it was built
    /// for. Measured, with the byte lint disabled: `{% if regions[0] == 'EU' %}`
    /// came back clean. Probe-only.
    fn probe_replace(&mut self, name: &str, value: Param) {
        let Some(existing) = self.entries.get(name) else {
            return;
        };
        if existing.downcast_object_ref::<Cell>().is_none() && existing.try_iter().is_ok() {
            self.insert_list(name.to_string(), vec![value]);
            return;
        }
        let allowed = existing
            .downcast_object_ref::<Cell>()
            .and_then(|c| c.allowed.clone());
        self.entries.insert(
            name.to_string(),
            Value::from_object(Cell::new(value, allowed)),
        );
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

/// One parameter value as a plain scalar, for [`check_publish`]'s second render.
fn plainify(value: &Value) -> Value {
    if let Some(cell) = value.downcast_object_ref::<Cell>() {
        return cell.plain();
    }
    // **`none` and `undefined` ITERATE AS EMPTY in minijinja**
    // (`Value::try_iter`, `ValueRepr::None => ValueIterImpl::Empty`), so without
    // this line the branch below turns a declared-but-absent parameter into an
    // empty LIST. A list cannot be bound as one value, so the plain render fails
    // where the opaque one succeeded, and [`check_dynamic`] reports
    // [`RenderError::ParameterInspected`] against a definition that inspects
    // nothing — a divergence manufactured entirely by this function.
    //
    // Unreachable until Task 7: nothing called [`check_publish`] with an absent
    // parameter, and absence is the case an OPTIONAL parameter is in on most
    // calls. Absent is already as plain as a value gets.
    if value.is_none() || value.is_undefined() {
        return value.clone();
    }
    match value.try_iter() {
        Ok(items) => Value::from(items.map(|v| plainify(&v)).collect::<Vec<_>>()),
        Err(_) => value.clone(),
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
    /// Charges `bytes` against the budget before the value is materialised.
    fn reserve(&mut self, bytes: usize) -> Result<(), MjError> {
        self.bytes = self.bytes.saturating_add(bytes);
        if self.bytes > MAX_BIND_BYTES {
            return Err(MjError::new(MjErrorKind::InvalidOperation, BINDS_TOO_LARGE));
        }
        Ok(())
    }

    fn push(&mut self, param: Param) {
        self.list.push(param);
    }
}

/// What binding `value` will cost, read without cloning it.
fn prospective_bytes(value: &Value) -> usize {
    if let Some(cell) = value.downcast_object_ref::<Cell>() {
        return match &cell.param {
            Param::Decimal(s) | Param::Str(s) | Param::Date(s) | Param::Timestamp(s) => s.len(),
            _ => std::mem::size_of::<Param>(),
        };
    }
    match value.as_str() {
        Some(s) => s.len(),
        None => std::mem::size_of::<Param>(),
    }
}

const BINDS_TOO_LARGE: &str = "the values bound by this definition exceed the size cap";

const SEQUENCE_BIND_MSG: &str = "this value cannot be bound as a single parameter; emit a \
                                 sequence one element at a time inside a `{% for %}` loop";

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
        let mut held = binds
            .lock()
            .expect("bind list mutex poisoned by a panic inside the renderer");
        // Measured BEFORE `to_param`, which clones. `{{ 'a' * 99_999_999 }}` is
        // already 99 MB inside minijinja by the time we see it; cloning it first
        // and checking afterwards doubles exactly the allocation the cap exists
        // to prevent. What minijinja itself allocated is bounded by its own
        // 100 MB repeated-string cap and is not ours to refuse.
        held.reserve(prospective_bytes(value))?;
        let param = to_param(value)?;
        held.push(param);
        drop(held);
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
                SEQUENCE_BIND_MSG,
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
    render_inner(sql, ctx)
}

/// The render itself: the hardened environment, the capped sink and the
/// invariant, with none of [`check_static`]'s source rules.
///
/// Separate so [`check_dynamic`] can probe with it. Re-running the scanner inside
/// every probe render would make the gate short-circuit on the scanner's own
/// verdict — which is the coupling the gate exists to remove — and the rules it
/// skips are ones [`check_publish`] has already applied once, backstopped in here
/// by the placeholder==bind invariant.
fn render_inner(sql: &str, ctx: &ParamValues) -> Result<(String, Vec<Param>), RenderError> {
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

    let readings = scannable_readings(&rendered)?;
    // BOTH readings, not the larger of the two. The invariant exists for
    // StarRocks arity, and StarRocks is the backslash-escaping reading — taking
    // the max let the reading that matters disagree as long as the other one
    // agreed. Requiring both is strictly stronger and rejects nothing
    // legitimate, since a definition whose two readings disagree about where a
    // literal ends is refused at publish anyway.
    for stripped in &readings {
        let placeholders = stripped.matches(PLACEHOLDER).count();
        if placeholders != binds.len() {
            return Err(RenderError::PlaceholderBindMismatch {
                placeholders,
                binds: binds.len(),
            });
        }
    }
    check_rendered_statement(&rendered, &readings)?;
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
fn scannable_readings(rendered: &str) -> Result<Vec<String>, RenderError> {
    [true, false]
        .into_iter()
        .map(|backslash_escapes| {
            crate::lifecycle::strip_comments_and_strings(rendered, backslash_escapes)
                .map_err(|e| RenderError::Unscannable(e.to_string()))
        })
        .collect()
}

/// **The rendered statement gets the same byte tests the definition got.**
///
/// # Why the definition's scan is not enough
///
/// `check_single_select` refuses a non-trailing `;` and `reject_unscannable_bytes`
/// refuses `#`, both on the RAW definition — and until this phase that was the
/// whole story, because the statement sent to an engine WAS the definition. It is
/// not any more. `| ident` writes an entry from the author's `allowed` list into
/// the statement as text, and nothing scanned that entry:
/// `allowed = ["id", "id; DROP TABLE t"]` published green, rendered
/// `SELECT id FROM t ORDER BY id; DROP TABLE t` and bound **zero** values — so
/// the placeholder==bind invariant was satisfied, and with no parameters the
/// serving path calls `execute`, which on StarRocks is COM_QUERY and runs every
/// statement in one submission. Author-to-arbitrary-DDL, past every rule.
///
/// On Trino the same payload is a query failure rather than an escalation: its
/// REST client submits one statement per request and the trailing text is a
/// parse error. The refusal is unconditional anyway — which engine is behind an
/// endpoint is not something this module knows, and "safe on the engine we
/// happen to have" is how the first version of this reasoning went wrong.
///
/// # Where it runs
///
/// Inside [`render`], so publish (through `check_rendered`) and every request
/// get the same test. The readings are the ones the invariant already computed.
fn check_rendered_statement(rendered: &str, readings: &[String]) -> Result<(), RenderError> {
    // `#` on the raw text and `;` on the stripped text, mirroring exactly what
    // the definition-side rules do — `reject_unscannable_bytes` is a raw scan and
    // `check_single_select` strips first.
    if rendered.contains('#') {
        return Err(RenderError::RenderedHash);
    }
    for stripped in readings {
        if stripped.trim_end().trim_end_matches(';').contains(';') {
            return Err(RenderError::RenderedSeparator);
        }
    }
    Ok(())
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
    for block in blocks(sql)? {
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
            // Tokens, not `split_whitespace`: `{% for x in (regions)recursive %}`
            // needs no space before the modifier.
            if keyword == "for"
                && tokens(body)
                    .iter()
                    .any(|t| t.kind == Tok::Word && t.text == "recursive")
            {
                return Err(RenderError::RecursiveLoop);
            }
        }
        // EVERY block, not only `{% if %}`: a comparison against a bare parameter
        // is meaningless wherever it is written, and it was silently taking the
        // wrong branch from `{% set f = mode == 'fast' %}`, from a for-loop's
        // filter condition, and from a bare `{{ 'FAST' if mode == 'fast' else … }}`.
        // BEFORE the lint, not after: `{% for r in regions if r == 'EU' %}`
        // compares the loop variable in the very tag that binds it, and a
        // collect-afterwards pass has no name for `r` yet when it reads that
        // condition.
        if let Block::Tag(_) = block {
            collect_aliases(body, ctx, &mut aliases);
        }
        if let Some(name) = bare_comparison(body, ctx, &aliases, matches!(block, Block::Tag(_))) {
            return Err(RenderError::BareParameterComparison(name));
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
            // Segment on top-level commas FIRST. `{% with a = 1, b = mode %}` is
            // several assignments — `parse_with_block` accepts a comma-separated
            // list — and taking the first `=` aliased `a` and stopped, which
            // reopened the rename bypass this function exists to close. But
            // `{% set m, n = mode, other %}` is ONE assignment to a tuple, with
            // commas on both sides. A segment with no `=` of its own is what
            // tells them apart.
            let segments = split_top_level(&toks);
            let multi = segments.len() > 1
                && segments
                    .iter()
                    .all(|seg| seg.iter().any(|t| t.kind == Tok::Other && t.text == "="));
            let assignments: Vec<&[Token<'_>]> = if multi { segments } else { vec![&toks[..]] };
            for seg in assignments {
                let Some(at) = seg
                    .iter()
                    .position(|t| t.kind == Tok::Other && t.text == "=")
                else {
                    continue;
                };
                let (targets, rhs) = (&seg[..at], &seg[at + 1..]);
                // `{% set m = mode | value %}` is the author doing exactly what
                // the error message asks; aliasing `m` there refuses the
                // comparison that follows. Filter position only, and only for
                // this segment — an early return used to discard every other
                // target in the tag.
                if param_in_region(rhs, ctx, aliases).is_none() {
                    continue;
                }
                for t in targets {
                    if t.kind == Tok::Word && t.text != "set" && t.text != "with" {
                        aliases.push(t.text.to_string());
                    }
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
            // The header ends at the loop's own `if` filter, so a condition
            // mentioning a parameter does not make the iterable look like one.
            let end = toks[at + 1..]
                .iter()
                .position(|t| t.kind == Tok::Word && t.text == "if")
                .map_or(toks.len(), |p| at + 1 + p);
            if toks[at + 1..end]
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

/// **The publish-time gate: everything [`check_static`] refuses, plus a
/// BEHAVIOURAL test for the rule no source scan has been able to hold.**
///
/// # Why this exists rather than a sixth round of spellings
///
/// Five separate reviews found five bypasses of the comparison rule — `{%+`,
/// `{# {% #}`, `{% raw %}`, `(mode)`, `regions[0] / | first` — and every one was
/// the same defect: **this module's scanner disagreeing with minijinja's lexer
/// about the same bytes.** A sixth spelling exists; nobody has written it down
/// yet. The scanner is fixed for the five, and it is still a scanner.
///
/// So the real gate is measured from behaviour instead. A parameter reaches the
/// template as an opaque [`Cell`] (so its SQL type survives), and minijinja
/// compares an object against a string as NOT EQUAL — that is the whole defect.
/// Render the same definition twice, once with the opaque values serving will
/// use and once with the same values as plain scalars: a construct that merely
/// BINDS a parameter renders identically both ways, and a construct that
/// INSPECTS one — `==`, `is eq`, `in`, a subscript, whatever its spelling —
/// takes a different branch and the SQL diverges. There is no token to evade.
///
/// It is the same instrument choice that makes the placeholder==bind invariant
/// work, and for the same reason: it reads the rendered output rather than the
/// source. That invariant is the one gate nothing has bypassed.
///
/// # What it costs, and the one thing it needs to be sensitive
///
/// A comparison only diverges when the plain render takes the OTHER branch,
/// which needs a value the comparison is true for. Representative values alone
/// would therefore miss `{% if mode == 'fast' %}` whenever the representative
/// value is not `fast` — i.e. nearly always. So the candidate values are taken
/// from the definition's own literals: `'fast'` is right there in the template
/// that compares against it.
///
/// **The property that makes this worth building**: harvesting a literal wrongly
/// costs SENSITIVITY, never soundness. A missed literal is a comparison this pass
/// does not happen to trigger; a missed spelling in the scanner was a rule that
/// did not run at all. The failure modes are not the same shape.
///
/// # It has no production caller yet
///
/// Task 7 wires this into publish. Until it does, **this gate delivers nothing
/// at runtime** — the serving path runs [`check_static`] and the invariant, and
/// nothing else. When it is wired, the published snapshot should record that it
/// passed, so a version frozen before the gate existed can be refused at serve
/// time rather than relying on an operator remembering to re-publish it.
///
/// # Deliberately not called by [`render`]
///
/// It renders the definition many times. That is right at publish, where it
/// happens once per version, and wrong per request. The serving path keeps
/// [`check_static`] and the invariant.
pub fn check_publish(sql: &str, ctx: &ParamValues) -> Result<(), RenderError> {
    check_static(sql, ctx)?;
    check_dynamic(sql, ctx)
}

/// Every name this template reads out of its context, as **minijinja's own
/// parser** reports it.
///
/// # This is the reconciliation of the two scanners
///
/// P3's `lifecycle::placeholders` treats the whole inside of a `{{ … }}` as a
/// parameter NAME. That was true of the renderer it was written against — one
/// that only ever substituted `{{name}}` — and it stopped being true the moment
/// P4.5 made the definition a real template. `{{ sort | ident }}` became an
/// undeclared placeholder called `"sort | ident"`, a loop variable `{{ r }}`
/// became an undeclared placeholder called `"r"`, and **spec §4.3's own
/// canonical example could not be published**.
///
/// The fix is not a better scanner. `undeclared_variables` walks the AST
/// minijinja itself compiled, so a loop target, a `{% set %}` target and a
/// `{% with %}` binding are already excluded, filter arguments and `{% if %}`
/// conditions are already included, and there is no spelling to keep in sync:
/// this module's whole history is the byte scanner disagreeing with the lexer,
/// and the answer that has held every time is to ask the lexer.
///
/// Not a replacement for `PlaceholderInsideQuotedText`, which is a different
/// question — *where in the SQL* a reference stands, which minijinja has no
/// opinion about — and stays a byte scan over both escaping readings.
///
/// The template is parsed under [`environment`], not a bare one, so the answer
/// is the same parse the render will do. A definition that does not parse is
/// reported as [`RenderError::Template`] rather than silently yielding an empty
/// set, which is what `undeclared_variables` does with a parse error on its own.
pub fn context_names(sql: &str) -> Result<Vec<String>, RenderError> {
    let binds = Arc::new(Mutex::new(Binds::default()));
    let mut env = environment(binds);
    env.add_template_owned("endpoint", sql.to_owned())
        .map_err(mj_err)?;
    let template = env.get_template("endpoint").map_err(mj_err)?;
    // `false`: a nested lookup is reported as its ROOT name, which is the only
    // half a `ParamSpec` can ever declare.
    let mut names: Vec<String> = template.undeclared_variables(false).into_iter().collect();
    // `undeclared_variables` returns a `HashSet`; publish errors must not depend
    // on hash order.
    names.sort();
    Ok(names)
}

/// The declared parameters SOME statement of an endpoint iterates over in a
/// `{% for %}` header — the union across `sqls`.
///
/// **The union is the whole point, and it is why this takes a slice.** One
/// request's parameters are shared by every statement an endpoint declares
/// (spec §4.2) and every statement renders against ONE `ParamValues`, so
/// whether a parameter is a list is a property of the ENDPOINT, not of the
/// statement being looked at. A signature taking a single `&str` can express
/// "just this one statement", and both sides of the endpoint — the publish-time
/// context builder and the serve-time resolver — used to ask it that way on the
/// publish side and this way on the serving side. They agree only at N = 1;
/// where they disagree the endpoint publishes green and answers 500 on every
/// call. There is now one function, and it cannot be asked about one statement
/// of several.
///
/// Order is the order first seen, so a caller that reports the set reports it
/// deterministically.
pub fn iterated_params_union(sqls: &[&str], declared: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for sql in sqls {
        for name in iterated_params(sql, declared) {
            if !out.iter().any(|o| o == &name) {
                out.push(name);
            }
        }
    }
    out
}

/// The declared parameters ONE definition iterates over in a `{% for %}`
/// header.
///
/// `pub(crate)` and, outside its own tests, called only by
/// [`iterated_params_union`]: an endpoint's shape decision is the union, and a
/// caller reaching for this one would be asking the question that used to
/// disagree with serving. Everything with an endpoint in hand goes through the
/// union.
///
/// Publish-time only, and it exists because `ParamSpec` has no multiplicity
/// field: whether a parameter arrives as one value or as a list is decided by
/// the REQUEST (a repeated query parameter becomes a list — see
/// `serving::params`), so representative values have to guess the shape the
/// definition expects. A parameter named as a loop's iterable gets a
/// one-element list; everything else gets a scalar.
///
/// A byte scan, with the same over-approximating habits as [`blocks`] — and the
/// direction it fails in is the safe one: a misclassified parameter makes the
/// representative render FAIL, which refuses the publish. It cannot make a
/// definition publish that would not otherwise.
pub(crate) fn iterated_params(sql: &str, declared: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Ok(bs) = blocks(sql) else {
        return out;
    };
    for block in bs {
        let Block::Tag(body) = block else { continue };
        let toks = tokens(body);
        if toks.first().map(|t| t.text) != Some("for") {
            continue;
        }
        let Some(at) = toks
            .iter()
            .position(|t| t.kind == Tok::Word && t.text == "in")
        else {
            continue;
        };
        // The header ends at the loop's own `if` filter — a condition naming a
        // parameter does not make that parameter the iterable.
        let end = toks[at + 1..]
            .iter()
            .position(|t| t.kind == Tok::Word && t.text == "if")
            .map_or(toks.len(), |p| at + 1 + p);
        for t in &toks[at + 1..end] {
            if t.kind == Tok::Word
                && declared.iter().any(|d| d == t.text)
                && !out.iter().any(|o| o == t.text)
            {
                out.push(t.text.to_string());
            }
        }
    }
    out
}

/// The behavioural half of [`check_publish`], on its own.
///
/// Public so it can be exercised — and mutated — without [`check_static`]
/// answering first. A gate whose tests can only reach it through the scanner is
/// a gate nobody has actually measured.
pub fn check_dynamic(sql: &str, ctx: &ParamValues) -> Result<(), RenderError> {
    for probe in probe_assignments(sql, ctx) {
        let Ok((opaque_sql, opaque_binds)) = render_inner(sql, &probe) else {
            // This assignment does not render; publish will refuse it through
            // its own render of the representative values, with the real error.
            continue;
        };
        let plain = match plain_render(sql, &probe) {
            PlainRender::Rendered(sql, binds) => (sql, binds),
            // The opaque render SUCCEEDED and the plain one did not: the two
            // have diverged, which is the finding, not an inconclusive probe.
            // `{% if <spelling> %}{{ regions }}{% endif %}` is the shape — a
            // branch only the plain values take, containing something that
            // cannot render. Discarding it threw away a positive.
            PlainRender::Failed => return Err(RenderError::ParameterInspected),
            PlainRender::Inconclusive => continue,
        };
        let (plain_sql, plain_binds) = plain;
        // Bind TEXT, not the bind list itself. The two renders legitimately
        // disagree about a value's TYPE — that is the entire reason parameters
        // are opaque, so `Date("2020-02-29")` opposite `Str("2020-02-29")` is the
        // gate working, not a finding. What must not differ is the text, and it
        // is the half that carries `{{ 'FAST' if (mode) == 'fast' else 'SLOW' }}`
        // — an inspection whose SQL shape and placeholder count never move.
        if opaque_sql != plain_sql || bind_text(&opaque_binds) != plain_binds {
            return Err(RenderError::ParameterInspected);
        }
    }
    Ok(())
}

/// Every DISTINCT statement this definition can render to, across the probe
/// assignments.
///
/// Deduplicated **by the rendered SQL string**, which is what makes publish
/// affordable: the probe budget is sixty-four assignments ([`MAX_PROBES`]), but
/// a definition with no `{% if %}` renders all sixty-four to the same
/// statement and costs one describe — exactly what it cost before this
/// existed. Cost tracks the number of structurally distinct queries, not the
/// number of parameters.
///
/// Assignments that fail to render are skipped, not propagated — the same way
/// [`check_dynamic`] skips them: publish refuses an unrenderable definition
/// through its own render of the representative values, and that render
/// reports the real error. This function has nothing more useful to say about
/// a probe that does not render.
pub fn render_permutations(sql: &str, ctx: &ParamValues) -> Vec<(String, Vec<Param>)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for probe in probe_assignments(sql, ctx) {
        let Ok((rendered, binds)) = render_inner(sql, &probe) else {
            continue;
        };
        if seen.insert(rendered.clone()) {
            out.push((rendered, binds));
        }
    }
    out
}

/// How many probe renders one definition may cost.
const MAX_PROBES: usize = 64;
/// How many of a definition's own literals are used as candidate values.
const MAX_PROBE_LITERALS: usize = 16;
/// How many of an enum parameter's allowed values are probed.
///
/// # Why this is the same number as [`MAX_PROBES`], and was once an eighth of it
///
/// It was `8` while the only consumer was [`check_dynamic`], where missing a
/// candidate costs SENSITIVITY — a definition that inspects a parameter goes
/// undetected — and the comment at the `take` reasoned that "one 64-value enum
/// would otherwise consume the entire probe budget".
///
/// It now decides a PUBLISHED CONTRACT: an allowed value that is never probed
/// is a branch never described, so its columns are missing from the frozen
/// union and the call that sends that value is a permanent `503 schema_drift`.
/// Measured through the real publish route, one enum of 64 allowed values whose
/// last-but-one value turns a column on: **1 describe at `8`** (the branch is
/// invisible, publish is green, and `?p=v62` can only ever 503) and **2
/// describes at `64`** (the union covers it). Wall clock for that publish moved
/// 123 ms → 139 ms; four such enums, 144 ms → 179 ms; thirty-two of them,
/// 1.24 s → 1.21 s (unchanged — that case is bounded by [`MAX_PROBES`], not by
/// this).
///
/// The cost the old cap was protecting is already bounded twice over, and by
/// limits that do not degrade silently: [`MAX_PROBES`] caps the ASSIGNMENTS at
/// 64 however deep any one candidate list is, and `lifecycle`'s
/// `MAX_DESCRIBED_BRANCHES` caps the DESCRIBES at 64 with a loud publish
/// refusal. A third, lower cap bought nothing either of those did not already
/// buy, and it cost an undescribed branch.
///
/// **Residual**: [`MAX_PROBES`] counts the base context itself, so a single
/// parameter reaches at most 63 candidates, and every parameter shares the
/// budget round-robin. An enum with more allowed values than the budget can
/// reach — or several enums sharing it — still leaves branches undescribed.
/// That is the pre-existing probe-budget starvation, recorded in the spec; the
/// serving check fails closed, so it is a loud 503 rather than a wrong 200.
const MAX_ENUM_CANDIDATES: usize = MAX_PROBES;

/// The value assignments the gate renders: the endpoint's own representative
/// values, then each parameter set to each literal the definition mentions (or,
/// for an enum parameter, to each of its allowed values — an enum cannot hold
/// anything else, and substituting something else would only break `| ident`).
fn probe_assignments(sql: &str, ctx: &ParamValues) -> Vec<ParamValues> {
    let mut out = vec![ctx.clone()];
    let literals = literals(sql);
    let per_name: Vec<(String, Vec<Param>)> = ctx
        .names()
        .map(str::to_owned)
        .map(|n| {
            let candidates = ctx.candidates(&n, &literals);
            (n, candidates)
        })
        .collect();
    // ROUND-ROBIN, one candidate per parameter per pass. Parameter-major
    // exhausts the budget on whichever parameter sorts first — `ctx.names()` is
    // alphabetical — and every later parameter is then never probed at all,
    // silently. A cap that quietly stops testing is the failure shape this gate
    // exists to remove, so it must at least degrade evenly.
    let deepest = per_name.iter().map(|(_, c)| c.len()).max().unwrap_or(0);
    for round in 0..deepest {
        for (name, candidates) in &per_name {
            let Some(candidate) = candidates.get(round) else {
                continue;
            };
            if out.len() >= MAX_PROBES {
                return out;
            }
            let mut probe = ctx.clone();
            probe.probe_replace(name, candidate.clone());
            out.push(probe);
        }
    }
    out
}

/// Every string and integer literal in the definition — those inside a tag or an
/// expression first, because those are the ones a template compares against,
/// then the rest of the text until the cap.
///
/// **Harvesting from the whole source, not only from blocks, is deliberate.**
/// Measured: with raw-awareness removed from [`blocks`], the gate ALSO stopped
/// catching `{% raw %}{#{% endraw %}{% if mode == 'fast' %}` — not because the
/// gate is syntactic, but because its candidate values were coming through the
/// same scanner, so a desync starved it of the literal `'fast'`. The gate's
/// soundness never depended on the scanner; its SENSITIVITY did, and this is what
/// cuts that last thread.
fn literals(sql: &str) -> Vec<Param> {
    let mut out: Vec<Param> = Vec::new();
    let bodies: Vec<&str> = blocks(sql)
        .map(|bs| bs.iter().map(|b| b.body()).collect())
        .unwrap_or_default();
    // Block bodies first, and only they contribute numbers. The SQL around them
    // is full of digits that are not comparison operands — `WHERE 1=1`,
    // `LIMIT 100`, `col2` — and a 16-entry budget spent on those is a budget not
    // spent on the literal a comparison actually names.
    for body in &bodies {
        harvest(body, true, &mut out);
    }
    harvest(sql, false, &mut out);
    out
}

/// Every string literal in `text`, and its integers too when `numbers` is set.
fn harvest(text: &str, numbers: bool, out: &mut Vec<Param>) {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() && out.len() < MAX_PROBE_LITERALS {
        let c = b[i];
        if c == b'\'' || c == b'"' {
            let start = i + 1;
            i += 1;
            while i < b.len() && b[i] != c {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            push_unique(out, Param::Str(text[start..i.min(text.len())].to_string()));
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            // Not a number if it is part of an identifier (`col2`, `x1`), and
            // not one at all outside a block body.
            let inside_word =
                start > 0 && (b[start - 1].is_ascii_alphanumeric() || b[start - 1] == b'_');
            if numbers && !inside_word {
                if let Ok(n) = text[start..i].parse::<i64>() {
                    push_unique(out, Param::I64(n));
                }
            }
        } else {
            i += 1;
        }
    }
}

fn push_unique(out: &mut Vec<Param>, param: Param) {
    if !out.contains(&param) {
        out.push(param);
    }
}

/// Renders `sql` with every parameter as a plain scalar rather than an opaque
/// value, returning the SQL and the number of values bound.
///
/// `| ident` refuses anything but an enum parameter — by construction, which is
/// the point of it — so a definition that uses it cannot have its enum
/// parameters plainified. That case is retried with them left opaque, and if it
/// still cannot render, this assignment is inconclusive rather than a finding.
fn plain_render(sql: &str, ctx: &ParamValues) -> PlainRender {
    // Everything plain, then one enum at a time kept opaque, then all of them.
    //
    // Per-parameter, because `| ident` needs ONE enum to stay opaque and keeping
    // every enum opaque blinds the gate to all the others — and spec §4.3's
    // canonical endpoint is exactly that shape: an `ident` sort column beside
    // ordinary parameters. `enums` is capped so a definition with many of them
    // cannot turn one probe into an unbounded number of renders.
    let mut attempts: Vec<Vec<String>> = vec![Vec::new()];
    let enums: Vec<String> = ctx.enum_names().take(MAX_ENUM_RETRIES).collect();
    attempts.extend(enums.iter().map(|e| vec![e.clone()]));
    if enums.len() > 1 {
        attempts.push(enums.clone());
    }
    for keep_opaque in attempts {
        match render_inner(sql, &ctx.plainified(&keep_opaque)) {
            Ok((sql, binds)) => return PlainRender::Rendered(sql, bind_text(&binds)),
            // `| ident` refuses anything but an enum parameter, by construction:
            // this attempt plainified the one it is applied to.
            Err(RenderError::Ident) => continue,
            Err(_) => return PlainRender::Failed,
        }
    }
    PlainRender::Inconclusive
}

/// How many enum parameters one probe will retry individually.
const MAX_ENUM_RETRIES: usize = 8;

enum PlainRender {
    Rendered(String, Vec<String>),
    /// Rendered opaque, refused plain — a divergence.
    Failed,
    /// `| ident` could not be satisfied by any attempt; nothing is learned.
    Inconclusive,
}

/// The token list split on commas at bracket depth zero.
fn split_top_level<'t, 'a>(toks: &'t [Token<'a>]) -> Vec<&'t [Token<'a>]> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, t) in toks.iter().enumerate() {
        if t.kind == Tok::Other {
            match t.text {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth -= 1,
                "," if depth == 0 => {
                    out.push(&toks[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
    }
    out.push(&toks[start..]);
    out
}

/// Each bound value as text, so two renders can be compared without their types.
fn bind_text(binds: &[Param]) -> Vec<String> {
    binds
        .iter()
        .map(|p| match p {
            Param::Null => String::new(),
            Param::Bool(b) => b.to_string(),
            Param::I64(i) => i.to_string(),
            Param::F64(f) => f.to_string(),
            Param::Decimal(s) | Param::Str(s) | Param::Date(s) | Param::Timestamp(s) => s.clone(),
        })
        .collect()
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
fn blocks(sql: &str) -> Result<Vec<Block<'_>>, RenderError> {
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
        let Some(at) = find_block_end(sql, body_start, close) else {
            // Unterminated: minijinja refuses it as a syntax error, and there is
            // nothing further to scan.
            break;
        };
        let body = trim_markers(&sql[body_start..at]);
        if is_tag && body.split_whitespace().next() == Some("raw") {
            // `{% raw %}` makes the lexer emit everything up to `{% endraw %}` as
            // template DATA — no tag, no comment, no expression inside it exists.
            // A scanner that does not know this reads `{% raw %}{#{% endraw %}` as
            // an unterminated comment, gives up, and every rule below is silently
            // off for the rest of the definition.
            i = find_endraw(sql, at + 2).ok_or(RenderError::UnterminatedRaw)?;
            continue;
        }
        out.push(if is_tag {
            Block::Tag(body)
        } else {
            Block::Expr(body)
        });
        i = at + 2;
    }
    Ok(out)
}

/// The offset just past the `{% endraw %}` that closes a raw block, by the
/// lexer's own rule: scan forward for `{%`, and accept it only if what follows is
/// optional whitespace-control, optional whitespace, `endraw`, optional
/// whitespace-control and `%}` (`skip_basic_tag(.., "endraw", .., true)`).
fn find_endraw(sql: &str, from: usize) -> Option<usize> {
    let b = sql.as_bytes();
    let mut i = from;
    while let Some(at) = b[i..].windows(2).position(|w| w == b"{%") {
        let after = i + at + 2;
        let rest = sql[after..].trim_start_matches(['-', '+']).trim_start();
        if let Some(tail) = rest.strip_prefix("endraw") {
            let tail = tail.trim_start().trim_start_matches(['-', '+']);
            if let Some(end) = tail.strip_prefix("%}") {
                return Some(sql.len() - end.len());
            }
        }
        i = after;
    }
    None
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
                let test = toks[i + 1..]
                    .iter()
                    .find(|t| t.kind == Tok::Word && t.text != "not");
                let benign = test.is_some_and(|t| matches!(t.text, "defined" | "undefined"));
                if benign {
                    continue;
                }
                // The whole subject region, not the nearest word: `regions |
                // first is string` is the same silence as `regions is string`.
                if let Some(name) = param_in_region(region_before(&toks, i), ctx, aliases) {
                    return Some(name);
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

/// The parameter an operand of the operator at `i` resolves to, if any.
///
/// Reads the whole operand REGION rather than the one token beside the operator,
/// because `(mode)`, `regions[0]` and `regions | first` are the same silence as a
/// bare `mode` and all three escape an adjacency check. The region ends at the
/// nearest thing that cannot be part of one operand: `,`, `and`, `or`, `if`,
/// `else`, or another operator.
///
/// [`PLAINISING_FILTERS`] is the one relaxation, and it is a usability list, not
/// a security boundary — `{{ regions | length > 0 }}` compares two integers and
/// is perfectly correct, so refusing it would be a false positive. A wrong entry
/// there costs a false positive or a miss that [`check_publish`] catches; it
/// cannot let caller text into SQL.
fn operand_param(
    toks: &[Token<'_>],
    i: usize,
    ctx: &ParamValues,
    aliases: &[String],
) -> Option<String> {
    [region_before(toks, i), region_after(toks, i)]
        .into_iter()
        .find_map(|region| param_in_region(region, ctx, aliases))
}

/// The parameter an operand region resolves to, unless a filter has already
/// turned it into a plain value.
///
/// # The position check is the whole point
///
/// A [`PLAINISING_FILTERS`] word counts **only in filter position** — directly
/// after a `|`. Matching the list against every word in the region regardless of
/// position was a hole with no evasion in it at all: a parameter *named*
/// `format`, `count`, `value`, `length`, `sum`, `title`, `string`, `int`,
/// `float`, `bool`, `join`, `trim`, `round` or `replace` matched its own name,
/// and `{% if format == 'csv' %}` — the plainest spelling there is — passed. Any
/// author could pick that name; nothing anywhere validates parameter names
/// against this list, and nothing should have to.
fn param_in_region(region: &[Token<'_>], ctx: &ParamValues, aliases: &[String]) -> Option<String> {
    let plainised = region.iter().enumerate().any(|(i, t)| {
        t.kind == Tok::Word
            && PLAINISING_FILTERS.contains(&t.text)
            && i > 0
            && region[i - 1].kind == Tok::Other
            && region[i - 1].text == "|"
    });
    if plainised {
        return None;
    }
    region
        .iter()
        .find(|t| t.kind == Tok::Word && is_param(t.text, ctx, aliases))
        .map(|t| t.text.to_string())
}

/// Filters whose result is a plain minijinja value, so a comparison after one is
/// meaningful. `value` is the documented escape hatch; the rest are here so
/// ordinary template arithmetic is not refused. Note what is NOT here: `first`,
/// `last`, `min`, `max`, `default` and `sort` all hand back an ELEMENT, which is
/// still a parameter value.
const PLAINISING_FILTERS: &[&str] = &[
    "value",
    "length",
    "count",
    "sum",
    "int",
    "float",
    "string",
    "join",
    "lower",
    "upper",
    "trim",
    "abs",
    "round",
    "capitalize",
    "title",
    "replace",
    "format",
    "bool",
    "indent",
];

/// Tokens making up the operand to the left of `i`.
fn region_before<'t, 'a>(toks: &'t [Token<'a>], i: usize) -> &'t [Token<'a>] {
    let mut start = i;
    while start > 0 && !ends_operand(&toks[start - 1]) {
        start -= 1;
    }
    &toks[start..i]
}

/// Tokens making up the operand to the right of `i`.
fn region_after<'t, 'a>(toks: &'t [Token<'a>], i: usize) -> &'t [Token<'a>] {
    let mut end = i + 1;
    while end < toks.len() && !ends_operand(&toks[end]) {
        end += 1;
    }
    &toks[i + 1..end]
}

fn ends_operand(tok: &Token<'_>) -> bool {
    match tok.kind {
        Tok::Op => true,
        // `=` is an assignment or a kwarg separator, never part of the operand
        // beside it (`==` lexes as one `Op`, so this cannot be a comparison).
        Tok::Other => tok.text == "," || tok.text == "=",
        Tok::Word => matches!(
            tok.text,
            "and" | "or" | "if" | "else" | "elif" | "in" | "is"
        ),
    }
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
    if chain_contains(&e, SEQUENCE_BIND_MSG) {
        return RenderError::SequenceBind;
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

#[cfg(test)]
mod tests {
    use super::*;
    use egret_dataapi_api::ParamLocation;

    #[test]
    fn a_boolean_parameter_is_probed_with_both_values() {
        let spec = ParamSpec {
            name: "detail".into(),
            ty: ParamType::Boolean,
            location: ParamLocation::Query,
            required: false,
            default: None,
            allowed: None,
            minimum: None,
            maximum: None,
            min_length: None,
            max_length: None,
            pattern: None,
            not_before: None,
            not_after: None,
            description: String::new(),
        };
        let mut ctx = ParamValues::new();
        ctx.insert_for_spec(&spec, Param::Bool(true));

        // A definition with no literals in it at all — the shape that hid the defect.
        let cands = ctx.candidates("detail", &[]);
        assert!(
            cands.contains(&Param::Bool(false)),
            "a boolean whose definition has no literals was never probed with false, \
             so the branch it guards was never described"
        );
        assert!(cands.contains(&Param::Bool(true)));
    }

    /// Regression for a fix-round-1 defect: an early `return` for booleans in
    /// `candidates` REPLACED the literal candidates instead of adding to them,
    /// so a boolean inspected against a *string* literal — a spelling
    /// `check_static` cannot see — silently stopped being caught by the
    /// behavioural gate. `candidates` must append `[true, false]` to the
    /// definition's literals, not substitute for them, or this regresses again.
    #[test]
    fn a_boolean_inspected_against_a_string_literal_is_still_caught() {
        let mut ctx = ParamValues::new();
        ctx.insert("detail", Param::Bool(true));

        let err = check_dynamic("{{ ',name' if (detail) == 'slow' else '' }}", &ctx)
            .expect_err("a boolean compared against a string literal must be caught");
        assert!(
            matches!(err, RenderError::ParameterInspected),
            "expected ParameterInspected, got {err:?}"
        );
    }

    #[test]
    fn a_definition_with_no_branch_renders_to_exactly_one_statement() {
        // `region` is bound, not interpolated, so it can never change the
        // rendered TEXT — that part of the claim is free. The literals
        // `'EU'`/`'US'`/`'APAC'` are what make the probe count itself
        // non-trivial: `region` is neither boolean nor enum, so `candidates`
        // hands it every literal the definition mentions, and `probe_assignments`
        // renders one probe per candidate plus the representative context. A
        // definition with only one probe to begin with would let this test pass
        // even with the `HashSet` guard deleted, which is why the count is
        // asserted below rather than assumed.
        let sql =
            "SELECT id FROM t WHERE r = {{ region }} AND a = 'EU' AND b = 'US' AND c = 'APAC'";
        let mut ctx = ParamValues::new();
        ctx.insert("region", Param::Str("EU".into()));
        let probes = probe_assignments(sql, &ctx);
        assert!(
            probes.len() > 1,
            "test is vacuous unless there is more than one probe to deduplicate; \
             probe_assignments returned {}",
            probes.len()
        );
        assert_eq!(
            render_permutations(sql, &ctx).len(),
            1,
            "deduplication must collapse every probe of a non-branching definition \
             to one statement, or publish silently pays for a describe per probe"
        );
    }

    #[test]
    fn a_branching_definition_renders_to_one_statement_per_branch() {
        let sql = "SELECT id {% if detail %}, name {% endif %}FROM t";
        let mut ctx = ParamValues::new();
        ctx.insert("detail", Param::Bool(true));
        let got = render_permutations(sql, &ctx);
        let rendered: Vec<&String> = got.iter().map(|p| &p.0).collect();
        assert_eq!(got.len(), 2, "got: {rendered:?}");
    }
}
