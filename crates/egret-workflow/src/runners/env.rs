// kestra: core/src/main/java/io/kestra/core/runners/VariableRenderer.java
// kestra: core/src/main/java/io/kestra/core/runners/PebbleEngineFactory.java

//! The render layer: the minijinja environment, the two coalescing functions
//! [`rewrite`] targets, and the `VariableRenderer` loop around them.
//!
//! [`rewrite`] reshapes syntax and evaluates nothing. This module is the other
//! half: it builds the engine that evaluates the reshaped source, and wraps it
//! in the same pre/post-processing `VariableRenderer.render(String, Map)` does.
//!
//! # The pipeline, and why it is in this order
//!
//! [`render_str`] mirrors `VariableRenderer.render(Object, Map, boolean,
//! boolean)` step for step:
//!
//! 1. **Short-circuit** — a source with no `{` at all is not a template; it is
//!    returned unchanged (`VariableRenderer.java:76-79`). This is the identity
//!    that makes W5's plugin-host pre-rendering coherent, and it is pinned by
//!    `property.rs`'s `a_source_without_an_expression_renders_to_itself`.
//! 2. **Recursive re-render** — `renderOnce` in a loop, stopping the moment
//!    output stops changing, capped at [`MAX_RENDERING_AMOUNT`]
//!    (`VariableRenderer.java:170-181`). A render whose *output* is itself a
//!    template renders again.
//! 3. **Strip the raw markers** — `RAW_PATTERN.matcher(x).replaceAll("$2")`
//!    (`VariableRenderer.java:85-87`), which is what turns
//!    `{% raw %}{{ v }}{% endraw %}` into the literal text `{{ v }}` rather
//!    than leaving the markers in the output.
//!
//! and each `renderOnce` (`VariableRenderer.java:93-140`) is:
//!
//! 1. **Protect raw regions** — every `{% raw %}…{% endraw %}` region is
//!    swapped for a UUID placeholder *before* the engine ever sees it, and
//!    swapped back after. See [`RAW_PATTERN`].
//! 2. **Rewrite** — egret's own step, standing exactly where Pebble's parse
//!    stands in Kestra. `??`, `???`, ternary and `isIn` become syntax
//!    minijinja can parse (see [`rewrite`]).
//! 3. **Render** through [`environment`].
//!
//! # `{% raw %}` is Kestra's, not Pebble's
//!
//! `raw` is **not** a Pebble tag — real Pebble 4.1.2 throws
//! `ParserException: Unexpected tag name "raw"`, and Kestra registers no
//! custom tags (`Extension.getTokenParsers()` returns `null`). Kestra supports
//! it one layer *above* Pebble, in `VariableRenderer`, by substituting raw
//! regions out of the string and putting them back afterwards. That is why the
//! handling lives here and not in [`rewrite`], and why it is a regex rather
//! than a tag: the port copies Kestra's structure.
//!
//! minijinja *does* have a real `{% raw %}` tag, so leaving raw regions in
//! place would mostly work by accident — but not identically, and
//! `oracle/corpus.json`'s `raw-multiline` entry is the case where the
//! difference is visible (see [`RAW_PATTERN`]).
//!
//! # Strictness
//!
//! [`environment`] uses [`minijinja::UndefinedBehavior::Strict`], the closest
//! match to `PebbleEngineFactory`'s `.strictVariables(true)`: a bare missing
//! variable is an error, not a silent empty. Measured against minijinja 2.24,
//! all three settings, with the same context:
//!
//! | source | `Strict` | `Chainable` | `Lenient` | Kestra |
//! |---|---|---|---|---|
//! | `{{ nothere }}` | **error** | `""` | `""` | **error** |
//! | `{{ obj.missing }}` | **error** | `""` | `""` | **error** |
//! | `{{ obj.missing ?? 'x' }}` | `x` | `x` | `x` | `x` |
//! | `{{ obj.missing.deeper ?? 'x' }}` | **error** | `x` | error | **`x`** |
//!
//! `Strict` is right on three rows out of four and `Chainable` is right on
//! one; the last row is a **known, recorded divergence** — Kestra's `??`
//! catches `AttributeNotFoundException` from *anywhere* in the left operand,
//! and minijinja under `Strict` raises before the function is reached.
//! `oracle/corpus.json`'s `coalesce-nested-absent` pins it. Do not "fix" it by
//! moving to `Chainable`: that trades one divergence for two, and silently
//! empties every typo'd variable in every flow.

use std::collections::BTreeMap;
use std::io;

use minijinja::value::{Value as MjValue, ValueKind};
use minijinja::{
    AutoEscape, Environment, Error as MjError, ErrorKind as MjErrorKind, State, UndefinedBehavior,
};
use regex::Regex;
use serde_json::Value as JsonValue;

use super::rewrite::{rewrite, RewriteError};

/// `VariableRenderer.MAX_RENDERING_AMOUNT`. A render whose output is itself a
/// template renders again; this bounds that.
pub const MAX_RENDERING_AMOUNT: usize = 100;

/// Instruction budget for one render pass. Exhaustion is a clean minijinja
/// `OutOfFuel` error, not a hang: `fuel` is not one of minijinja's default
/// features and is named explicitly in the workspace manifest.
///
/// **What it protects against.** Flow source is author-supplied and rendering
/// happens on a server thread. Without fuel, `{% for i in range(100000000) %}`
/// -- legal minijinja, and `range` is a real Kestra function -- is an
/// unbounded busy loop holding a worker.
///
/// **What a legitimate expression costs.** Measured by bisecting the fuel
/// needed for each of these to succeed at all (minijinja 2.24, this
/// environment):
///
/// | source | fuel |
/// |---|---:|
/// | `{{ a }}` | 3 |
/// | `{{ kestra_null_coalesce(a, 'x') }}` | 5 |
/// | `{{ 'yes' if flag else 'no' }}` | 6 |
/// | `{% if a %}yes{% else %}no{% endif %}` | 5 |
/// | `{{ items \| join(',') \| upper }}` | 6 |
/// | `{% for i in items %}{{ i }}{% endfor %}` (10 items) | 53 |
/// | `{% for i in range(1000) %}{{ i }}{% endfor %}` | 5,004 |
///
/// So a `Property`'s typical expression costs single-digit fuel, and the
/// budget here buys roughly a 10,000-iteration loop. That is four orders of
/// magnitude of headroom over what a real flow's property needs, and still
/// bounded. Same number [`egret-dataapi`'s SQL renderer picked][dataapi] for
/// the same reason, so the two seams do not need separate reasoning.
///
/// [dataapi]: ../../../egret_dataapi/render/index.html
const FUEL: u64 = 50_000;

/// Well under minijinja's own default of 500. The setter clamps to
/// `min(level, MAX_RECURSION)`, so this can only ever lower the limit. Note
/// this is minijinja's *template* recursion, which is a different budget from
/// [`MAX_RENDERING_AMOUNT`] (Kestra's re-render loop) and from
/// `rewrite`'s `MAX_NESTING_DEPTH` (the rewriter's own parse recursion).
const RECURSION_LIMIT: usize = 64;

/// Cap on one render pass's output.
///
/// **Fuel is not a substitute and does not bound this.** Fuel counts
/// instructions; `{{ 'a' * 999_999 }}` is a handful of instructions and a
/// megabyte of output, and minijinja has no output-size limit of its own. The
/// re-render loop makes it worse than the usual case rather than better: each
/// of up to [`MAX_RENDERING_AMOUNT`] passes renders the *previous pass's
/// output*, so a template that grows its own output compounds.
///
/// 1 MiB, the same number [`egret-dataapi`'s SQL renderer uses][dataapi] and
/// for the same reason: a real `Property` is at most a few KB (a command line,
/// a URI, a small JSON body), so this is three orders of magnitude of headroom
/// and still bounded. Enforced by rendering into a sized sink rather than by
/// checking afterwards, so the memory is never allocated in the first place.
///
/// [dataapi]: ../../../egret_dataapi/render/index.html
const MAX_OUTPUT_BYTES: usize = 1 << 20;

/// `VariableRenderer.RAW_PATTERN`, copied character for character from
/// `core/src/main/java/io/kestra/core/runners/VariableRenderer.java:23`:
///
/// ```text
/// Pattern.compile("(\\{%-*\\s*raw\\s*-*%}(.*?)\\{%-*\\s*endraw\\s*-*%})")
/// ```
///
/// Three properties of it are load-bearing, and the obvious implementation
/// misses all three (Kestra's own `PebbleVariableRendererTest.raw` asserts
/// each):
///
/// - it accepts whitespace-control markers and uneven spacing --
///   `{% raw %}`, `{%raw%}` and `{%-  raw%}…{%endraw -%}` all match;
/// - it is **non-greedy**, so several raw regions in one template each end at
///   their own `endraw` rather than the last one;
/// - group 2 is the *body*, which is what `render` keeps when it strips the
///   markers.
///
/// A fourth property is accidental but faithfully reproduced: Java's `.` does
/// not match a newline and neither does Rust's, and the pattern sets no
/// DOTALL flag -- so a raw region **spanning lines is not protected at all**
/// in Kestra, and Pebble then chokes on the unknown `raw` tag. Pinned by
/// `oracle/corpus.json`'s `raw-multiline`, whose golden value is that parse
/// error. This is exactly the case where minijinja's own (real, multiline)
/// `{% raw %}` tag makes egret *more* capable than the engine it ports.
fn raw_pattern() -> &'static Regex {
    static RAW_PATTERN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RAW_PATTERN.get_or_init(|| {
        Regex::new(r"(\{%-*\s*raw\s*-*%\}(.*?)\{%-*\s*endraw\s*-*%\})")
            .expect("RAW_PATTERN is a compile-time constant")
    })
}

/// Everything the render layer can refuse.
///
/// Kept as a value rather than a string so callers can tell a *rewrite*
/// failure (the source is not Kestra syntax this port understands) from a
/// *template* failure (minijinja rejected or refused it) from a *conversion*
/// failure (the rendered text is not a `T`). The oracle test flattens all
/// three to their `Display`, which is the only shape a differential
/// comparison can use.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RenderError {
    /// The rewriter could not make sense of the source. See [`RewriteError`].
    #[error("{0}")]
    Rewrite(#[from] RewriteError),
    /// minijinja rejected the rewritten source, or refused it at render time
    /// (an undefined variable under `Strict`, an unknown filter, out of fuel).
    #[error("{0}")]
    Template(String),
    /// The re-render loop did not converge. Same wording as
    /// `VariableRenderer`'s `IllegalVariableEvaluationException`.
    #[error("Too many rendering attempts")]
    TooManyRenderingAttempts,
    /// One render pass produced more than [`MAX_OUTPUT_BYTES`]. Kestra has no
    /// such limit; this is egret refusing to let author-supplied source
    /// allocate without bound, in the same spirit as `rewrite`'s nesting
    /// depth cap.
    #[error("rendered output exceeded {limit} bytes")]
    OutputTooLarge {
        /// [`MAX_OUTPUT_BYTES`], named in the message so an author sees the
        /// actual budget rather than being told only that they missed it.
        limit: usize,
    },
    /// The text rendered fine but is not a `T`. Carries the rendered text,
    /// because the author's mistake is almost always visible in it.
    #[error("rendered value {rendered:?} is not a valid {target}: {detail}")]
    Convert {
        /// `std::any::type_name::<T>()` of the `Property`'s target.
        target: &'static str,
        /// The text that was rendered, before conversion.
        rendered: String,
        /// serde's own complaint.
        detail: String,
    },
}

/// The variables an expression renders against.
///
/// A thin wrapper over `BTreeMap<String, serde_json::Value>` rather than a
/// bare alias, so the render seam has one type to name and W3 has one place to
/// build `{{ inputs.* }}` / `{{ outputs.* }}` / `{{ trigger.* }}` into.
/// `BTreeMap`, not `HashMap`: iteration order is deterministic, which matters
/// the moment an error message or a debug dump names the keys.
///
/// W2 does not build an execution context — that is W3's job (see the crate
/// doc). This type is the shape W3 will fill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderContext(BTreeMap<String, JsonValue>);

impl RenderContext {
    /// An empty context. A template that references any variable will fail
    /// against it, by design — see this module's strictness note.
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds one top-level name.
    pub fn insert(&mut self, key: impl Into<String>, value: JsonValue) -> &mut Self {
        self.0.insert(key.into(), value);
        self
    }

    /// The bindings, for callers that need to inspect or extend them.
    pub fn variables(&self) -> &BTreeMap<String, JsonValue> {
        &self.0
    }

    /// Builds a context from a JSON object.
    ///
    /// Returns `None` for anything that is not an object: a template's
    /// variables are a name-to-value map and there is no sensible top-level
    /// binding for a bare array or scalar. Callers that must not silently
    /// render against an empty context should treat `None` as an error rather
    /// than falling back to [`RenderContext::new`].
    pub fn from_json_object(value: &JsonValue) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self(
            object.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        ))
    }

    fn to_minijinja(&self) -> MjValue {
        MjValue::from_serialize(&self.0)
    }
}

impl From<BTreeMap<String, JsonValue>> for RenderContext {
    fn from(map: BTreeMap<String, JsonValue>) -> Self {
        Self(map)
    }
}

/// `NullCoalescingExpression` — the `??` the rewriter emits calls to.
///
/// Kestra
/// (`core/src/main/java/io/kestra/core/runners/pebble/expression/NullCoalescingExpression.java`):
/// evaluate the left side; if it throws `AttributeNotFoundException` treat it
/// as `null`; if the result is `null`, return the right side; otherwise return
/// the left. **Null and missing are treated the same.**
///
/// Here, "missing" arrives as minijinja's `undefined` — function arguments are
/// not subject to `UndefinedBehavior::Strict` (measured: `{{ nothere }}`
/// errors, `{{ f(nothere) }}` hands the function an `Undefined`), which is
/// precisely what makes this expressible as a function at all.
///
/// Both operands are evaluated before this is called; Kestra's is a lazy
/// binary operator. That is a real difference, and it is unfixable from a
/// rewrite that only reshapes text — a wrong-but-cheap right operand costs a
/// little work here where Kestra would skip it, and a right operand that
/// *errors* fails the whole render where Kestra would never have reached it.
/// See [`rewrite`]'s module doc.
fn kestra_null_coalesce(_state: &State, left: MjValue, right: MjValue) -> MjValue {
    if left.is_undefined() || left.is_none() {
        right
    } else {
        left
    }
}

/// `UndefinedCoalescingExpression` — the `???` the rewriter emits calls to.
///
/// **Not the same operator as `??`.** Kestra
/// (`.../expression/UndefinedCoalescingExpression.java`) returns the right
/// side *only* when evaluating the left throws `AttributeNotFoundException`.
/// A left side that is present but `null` is returned as-is.
///
/// `oracle/golden.json` is the proof, not the source reading:
/// `undef-coalesce-null` (`{{ missing ??? 'fallback' }}` with
/// `missing: null`) is `""`, while `coalesce-null` — the same context, `??` —
/// is `"fallback"`.
fn kestra_undefined_coalesce(_state: &State, left: MjValue, right: MjValue) -> MjValue {
    if left.is_undefined() {
        right
    } else {
        left
    }
}

/// `InExpression` — the `isIn` the rewriter emits calls to.
///
/// **Not minijinja's `in`.** Task 2 mapped `x isIn y` to `(x in y)`
/// provisionally and was told the measurement belonged to Task 3; the
/// measurement says the mapping is wrong in three separate ways.
/// `InExpression.evaluate`
/// (`core/src/main/java/io/kestra/core/runners/pebble/expression/InExpression.java`):
///
/// - a `null` on **either** side is `false`, before anything else;
/// - a `Collection`/`Iterable` right-hand side is compared by
///   **`Object::toString()` on both sides** — `1 isIn ['1','2']` is `true`;
/// - a `Map` right-hand side matches a key **or a value** —
///   `'v' isIn {'k':'v'}` is `true`, where minijinja's `in` checks keys only;
/// - **any other** right-hand type is `false` — `'b' isIn 'abc'` is `false`,
///   where minijinja's `in` does substring containment and says `true`.
///
/// All four rows are corpus entries (`isin`, `isin-number-in-strings`,
/// `isin-map-value`, `isin-string-rhs`). The first agreed with minijinja's
/// `in` by luck — same types on both sides, list right-hand — which is why the
/// other three had to be added before this could be settled.
///
/// Stringifying with `MjValue::to_string` is the deliberate counterpart of
/// Java's `toString()`: a minijinja string Displays as its own contents (no
/// quotes), a number as its digits, a bool as `true`/`false` through this
/// module's formatter rules. It is what makes the number-versus-string row
/// work.
fn kestra_is_in(_state: &State, left: MjValue, right: MjValue) -> Result<MjValue, MjError> {
    if left.is_undefined() || left.is_none() || right.is_undefined() || right.is_none() {
        return Ok(MjValue::from(false));
    }

    let needle = left.to_string();

    match right.kind() {
        ValueKind::Seq | ValueKind::Iterable => {
            let mut items = right.try_iter()?;
            Ok(MjValue::from(items.any(|item| item.to_string() == needle)))
        }
        ValueKind::Map => {
            for key in right.try_iter()? {
                if key.to_string() == needle {
                    return Ok(MjValue::from(true));
                }
                if let Ok(value) = right.get_item(&key) {
                    if value.to_string() == needle {
                        return Ok(MjValue::from(true));
                    }
                }
            }
            Ok(MjValue::from(false))
        }
        // `InExpression`'s `default -> return false`. A string right-hand side
        // lands here, which is the row minijinja's `in` gets backwards.
        _ => Ok(MjValue::from(false)),
    }
}

/// `JsonWriter.write(Object)` — Kestra's rendering of a value into output
/// text, which is a different job from what any filter does and therefore
/// belongs on the environment.
///
/// Read off `core/src/main/java/io/kestra/core/runners/pebble/JsonWriter.java`
/// (lines 63-87), which is exactly four cases:
///
/// | value | Kestra | minijinja's default | this formatter |
/// |---|---|---|---|
/// | `null` | writes nothing (early `return`) | `None` | `""` |
/// | `Map`/`Collection`/array | `MAPPER.writeValueAsString(o)` — compact JSON | `{"a": 1}` / `[1, 2]` (spaces) | compact JSON |
/// | `Boolean` | `toString()` → `true`/`false` | `True`/`False` | `true`/`false` |
/// | anything else | appended as-is | as-is | as-is |
///
/// Three of the four differ from minijinja's default, and none of the three is
/// something a filter or function task could fix — `{{ list }}` names no
/// filter at all. `serde_json::to_string` on a `minijinja::Value` is the
/// counterpart of Jackson's compact writer: no spaces after `:` or `,`, which
/// is what `filter-keys`'s golden `["a","b"]` and `function-fetchcontext`'s
/// `{"x":1}` are.
fn json_writer_formatter(
    out: &mut minijinja::Output<'_>,
    state: &State<'_, '_>,
    value: &MjValue,
) -> Result<(), MjError> {
    if value.is_none() {
        return Ok(());
    }

    match value.kind() {
        ValueKind::Bool => {
            out.write_str(if value.is_true() { "true" } else { "false" })?;
            Ok(())
        }
        ValueKind::Seq | ValueKind::Map => {
            let json = serde_json::to_string(value).map_err(|e| {
                MjError::new(
                    MjErrorKind::InvalidOperation,
                    format!("value cannot be rendered as JSON: {e}"),
                )
            })?;
            out.write_str(&json)?;
            Ok(())
        }
        _ => minijinja::escape_formatter(out, state, value),
    }
}

/// The render environment: strictness, limits, and the three operator
/// functions [`rewrite`] emits calls to.
///
/// Read the module doc before changing a line of this; every setting here is
/// either a `PebbleEngineFactory` setting being reproduced or a bound on
/// author-supplied input.
pub fn environment() -> Environment<'static> {
    let mut env = Environment::new();

    // `PebbleEngineFactory.newPebbleEngineBuilder().strictVariables(true)`.
    // See the module doc's table for what the three minijinja settings do and
    // why this is the closest of them.
    env.set_undefined_behavior(UndefinedBehavior::Strict);

    // `.autoEscaping(false)`. minijinja's default callback escapes by file
    // extension; a `Property` is a literal template with no name, so this is
    // belt-and-braces rather than a live bug -- but a future caller that
    // registers a named template must not silently start HTML-escaping a
    // task's arguments.
    env.set_auto_escape_callback(|_| AutoEscape::None);

    env.set_fuel(Some(FUEL));
    env.set_recursion_limit(RECURSION_LIMIT);

    // Two minijinja built-ins that have no Pebble counterpart and both dump
    // internal state: `debug` is `format!("{state:#?}")`, which recovers the
    // whole template context plus an inventory of every registered filter,
    // test and global; `pprint` is the `Debug` repr of a value. A flow author
    // is not an anonymous caller, but a rendered `Property` can end up in a
    // log, an output or a downstream task's arguments, and neither of these
    // exists in the language being ported -- removing them costs nothing.
    //
    // The rest of minijinja's global inventory (`range`, `dict`, `namespace`)
    // is deliberately left alone here: reconciling it against Kestra's 34
    // registered functions is Task 7/8's job, and `range` in particular is a
    // real Kestra function (`BoundedRangeFunction`) with different semantics
    // from minijinja's, not something to silently keep or silently drop now.
    env.remove_global("debug");
    env.remove_filter("pprint");

    // The port of `JsonWriter.write(Object)` -- null, booleans and containers.
    // See [`json_writer_formatter`] for the four-row table; three of the four
    // differ from minijinja's default, and none of them is reachable from a
    // filter or function, because `{{ list }}` names neither.
    env.set_formatter(json_writer_formatter);

    env.add_function("kestra_null_coalesce", kestra_null_coalesce);
    env.add_function("kestra_undefined_coalesce", kestra_undefined_coalesce);
    env.add_function("kestra_is_in", kestra_is_in);

    env
}

/// Renders a template string against `cx`, mirroring
/// `VariableRenderer.render(String, Map)`.
///
/// This is the whole render layer as one function: the no-`{` short-circuit,
/// the re-render loop, the raw-region protection, the rewrite, and the strip
/// of the raw markers. [`crate::models::Property::render`] is this plus a
/// conversion.
pub fn render_str(template: &str, cx: &RenderContext) -> Result<String, RenderError> {
    // `VariableRenderer.java:76-79`: "it's not a Pebble template so we
    // short-circuit rendering". Note the test is `{`, not `{{` -- a source
    // containing a lone `{` still goes through the engine.
    if !template.contains('{') {
        return Ok(template.to_string());
    }

    let env = environment();
    let variables = cx.to_minijinja();

    let mut current = template.to_string();
    for _ in 0..MAX_RENDERING_AMOUNT {
        let next = render_once(&env, &current, &variables)?;
        if next == current {
            return Ok(strip_raw_markers(&next));
        }
        current = next;
    }
    Err(RenderError::TooManyRenderingAttempts)
}

/// One pass: protect raw regions, rewrite, render, put the raw regions back.
///
/// `VariableRenderer.renderOnce`, with egret's [`rewrite`] standing where
/// Pebble's parse stands. The raw regions go back **with their markers**, so
/// the next pass of the loop protects them again; only [`render_str`]'s final
/// [`strip_raw_markers`] takes the markers off.
fn render_once(
    env: &Environment<'static>,
    source: &str,
    variables: &MjValue,
) -> Result<String, RenderError> {
    let (protected, replacers) = replace_raw_tags(source);
    let rewritten = rewrite(&protected)?;
    let template = env
        .template_from_str(&rewritten)
        .map_err(|e| RenderError::Template(e.to_string()))?;
    let mut sink = CappedSink::new(MAX_OUTPUT_BYTES);
    let render_result = template.render_captured_to(variables, &mut sink);
    // The sink's own flag is checked FIRST and unconditionally. minijinja
    // wraps a writer failure in its own error, and deciding "was this the cap
    // or a real template error?" by inspecting that error's text would be a
    // guess; the sink knows.
    if sink.overflowed {
        return Err(RenderError::OutputTooLarge {
            limit: MAX_OUTPUT_BYTES,
        });
    }
    render_result.map_err(|e| RenderError::Template(e.to_string()))?;

    let rendered = String::from_utf8(sink.buf).map_err(|e| RenderError::Template(e.to_string()))?;
    Ok(put_back_raw_tags(&replacers, &rendered))
}

/// An [`io::Write`] that refuses past a byte budget instead of growing.
///
/// The point is that the bytes are never allocated: checking the length of a
/// finished `String` would mean having built the oversized `String` first,
/// which is the allocation the cap exists to prevent.
struct CappedSink {
    buf: Vec<u8>,
    limit: usize,
    /// Set once the budget is exceeded. Read by [`render_once`] rather than
    /// inferred from minijinja's wrapped error.
    overflowed: bool,
}

impl CappedSink {
    fn new(limit: usize) -> Self {
        Self {
            buf: Vec::new(),
            limit,
            overflowed: false,
        }
    }
}

impl io::Write for CappedSink {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.limit {
            self.overflowed = true;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "rendered output exceeded the size cap",
            ));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// `VariableRenderer.replaceRawTags`: swap each raw region for a fresh UUID
/// and remember what it was.
///
/// The placeholder must be something the engine will carry through untouched
/// and something an author cannot collide with; Kestra uses a random UUID per
/// region and so does this. A *fixed* placeholder would let a template that
/// contains the placeholder text steal another region's body.
fn replace_raw_tags(source: &str) -> (String, Vec<(String, String)>) {
    let mut replacers: Vec<(String, String)> = Vec::new();
    let protected = raw_pattern().replace_all(source, |caps: &regex::Captures<'_>| {
        let placeholder = uuid::Uuid::new_v4().to_string();
        replacers.push((placeholder.clone(), caps[1].to_string()));
        placeholder
    });
    (protected.into_owned(), replacers)
}

/// `VariableRenderer.putBackRawTags`: substitute each placeholder back for the
/// region it stood in for, markers and all.
fn put_back_raw_tags(replacers: &[(String, String)], rendered: &str) -> String {
    let mut out = rendered.to_string();
    for (placeholder, original) in replacers {
        out = out.replace(placeholder, original);
    }
    out
}

/// `RAW_PATTERN.matcher(x).replaceAll("$2")` (`VariableRenderer.java:85-87`):
/// drop the `{% raw %}`/`{% endraw %}` markers and keep the body.
fn strip_raw_markers(rendered: &str) -> String {
    raw_pattern().replace_all(rendered, "${2}").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cx(value: JsonValue) -> RenderContext {
        RenderContext::from_json_object(&value).expect("test context must be a JSON object")
    }

    fn render(template: &str, ctx: JsonValue) -> Result<String, RenderError> {
        render_str(template, &cx(ctx))
    }

    /// `??` falls back for BOTH a present-but-null and an absent variable;
    /// `???` falls back only for the absent one. `oracle/golden.json`'s
    /// `coalesce-null` / `undef-coalesce-null` pair is the measured proof that
    /// collapsing the two is wrong, and this is that pair as a unit test so a
    /// regression does not have to wait for the oracle to be read.
    #[test]
    fn the_two_coalescing_operators_differ_on_a_present_null() {
        assert_eq!(
            render("{{ missing ?? 'fallback' }}", json!({ "missing": null })).unwrap(),
            "fallback"
        );
        assert_eq!(
            render("{{ missing ??? 'fallback' }}", json!({ "missing": null })).unwrap(),
            ""
        );
        // Absent is the one case both agree on.
        assert_eq!(
            render("{{ nothere ?? 'fallback' }}", json!({})).unwrap(),
            "fallback"
        );
        assert_eq!(
            render("{{ nothere ??? 'fallback' }}", json!({})).unwrap(),
            "fallback"
        );
    }

    /// Stub check: with `kestra_null_coalesce` returning its LEFT operand
    /// unconditionally (the implementation a reader might assume), the first
    /// assertion above renders `""` and fails. With `kestra_undefined_coalesce`
    /// collapsed into `kestra_null_coalesce`, the second renders `"fallback"`
    /// and fails. Both were run before this test was committed.
    #[test]
    fn a_missing_variable_is_an_error_not_an_empty_string() {
        let err = render("{{ nothere }}", json!({})).unwrap_err();
        assert!(
            matches!(err, RenderError::Template(ref m) if m.contains("undefined")),
            "unexpected error: {err}"
        );
    }

    /// Pebble writes nothing for a null, and `VariableRenderer` turns a null
    /// result into `""`. minijinja's own rendering of `none` is the text
    /// `None`, so this is the formatter doing its job.
    #[test]
    fn a_null_renders_as_the_empty_string() {
        assert_eq!(render("{{ n }}", json!({ "n": null })).unwrap(), "");
        assert_eq!(render("[{{ n }}]", json!({ "n": null })).unwrap(), "[]");
    }

    /// The other three rows of `JsonWriter.write(Object)`. Every one of them
    /// differs from minijinja's default, and none is reachable from a filter
    /// or function -- `{{ list }}` names neither -- which is why they live on
    /// the environment.
    ///
    /// The container cases pin the ABSENCE of spaces specifically: minijinja
    /// renders `[1, 2]` and `{"a": 1}`, Jackson's compact writer (and so every
    /// container-valued golden in `oracle/golden.json`, e.g. `filter-keys`'s
    /// `["a","b"]`) has no space after `,` or `:`.
    #[test]
    fn booleans_and_containers_render_the_way_jsonwriter_does() {
        assert_eq!(render("{{ t }}", json!({ "t": true })).unwrap(), "true");
        assert_eq!(render("{{ f }}", json!({ "f": false })).unwrap(), "false");
        // Not via a variable: an expression that PRODUCES a boolean.
        assert_eq!(render("{{ 2 > 10 }}", json!({})).unwrap(), "false");

        assert_eq!(
            render("{{ list }}", json!({ "list": [1, 2] })).unwrap(),
            "[1,2]"
        );
        assert_eq!(
            render("{{ obj }}", json!({ "obj": { "a": 1, "b": ["x"] } })).unwrap(),
            r#"{"a":1,"b":["x"]}"#
        );
        // A scalar is still written as-is, not JSON-quoted.
        assert_eq!(render("{{ s }}", json!({ "s": "ada" })).unwrap(), "ada");
        assert_eq!(render("{{ n }}", json!({ "n": 42 })).unwrap(), "42");
    }

    /// `isIn` is `InExpression`, not minijinja's `in`. All four rows are also
    /// corpus entries; they are here as well because the first one agrees with
    /// minijinja's `in` by luck, and a reader who checks only that one would
    /// conclude the mapping was fine.
    #[test]
    fn is_in_is_kestras_in_expression_not_minijinjas() {
        // Same types, list right-hand: the case that agreed by luck.
        assert_eq!(
            render("{{ 'b' isIn ['a','b'] }}", json!({})).unwrap(),
            "true"
        );
        // Stringified on both sides: minijinja's `in` says false here.
        assert_eq!(render("{{ 1 isIn ['1','2'] }}", json!({})).unwrap(), "true");
        // A map matches on VALUES as well as keys; minijinja checks keys only.
        let m = json!({ "m": { "k": "v" } });
        assert_eq!(render("{{ 'v' isIn m }}", m.clone()).unwrap(), "true");
        assert_eq!(render("{{ 'k' isIn m }}", m).unwrap(), "true");
        // Any other right-hand type is false; minijinja does substring.
        assert_eq!(render("{{ 'b' isIn 'abc' }}", json!({})).unwrap(), "false");
        // Either side null is false, before anything else.
        assert_eq!(
            render("{{ n isIn ['a'] }}", json!({ "n": null })).unwrap(),
            "false"
        );
        assert_eq!(
            render("{{ 'a' isIn n }}", json!({ "n": null })).unwrap(),
            "false"
        );
    }

    /// Fuel bounds instructions, not bytes. `'a' * n` is a handful of
    /// instructions and an arbitrary number of bytes, so without the sink cap
    /// this allocates ~8 MB from one short expression.
    #[test]
    fn an_oversized_render_is_refused_by_the_output_cap() {
        let err = render("{{ 'a' * 8000000 }}", json!({})).unwrap_err();
        assert_eq!(
            err,
            RenderError::OutputTooLarge {
                limit: MAX_OUTPUT_BYTES
            }
        );
        // And the budget itself is real: just under it still renders.
        let ok = render("{{ 'a' * 1000 }}", json!({})).unwrap();
        assert_eq!(ok.len(), 1000);
    }

    /// All three spellings `PebbleVariableRendererTest.raw` asserts, plus the
    /// non-greedy multi-region case. Each body is `{{ var }}` with `var`
    /// bound, so an implementation that renders instead of preserving emits
    /// `V` and fails visibly.
    #[test]
    fn raw_regions_are_preserved_in_every_spelling_kestra_accepts() {
        let ctx = json!({ "var": "V" });
        for source in [
            "See some code {% raw %}{{ var }}{% endraw %}",
            "See some code {%raw%}{{ var }}{%endraw%}",
            "See some code {%-  raw%}{{ var }}{%endraw -%}",
        ] {
            assert_eq!(
                render(source, ctx.clone()).unwrap(),
                "See some code {{ var }}",
                "source: {source}"
            );
        }

        // Non-greedy: each region ends at its OWN endraw, and an expression
        // outside them still renders.
        assert_eq!(
            render(
                "{% raw %}{{ var }}{% endraw %} {{ var }} {% raw %}{{ var }}{% endraw %}",
                ctx,
            )
            .unwrap(),
            "{{ var }} V {{ var }}",
        );
    }

    /// Kestra's `RAW_PATTERN` sets no DOTALL flag, so a raw region spanning
    /// lines is **not protected**, and Pebble then fails on the unknown `raw`
    /// tag (`oracle/corpus.json`'s `raw-multiline`, whose golden value is that
    /// parse error). egret does not reproduce the failure, and this test pins
    /// what it does instead rather than asserting a parity that is not there.
    ///
    /// Two things happen once the region is unprotected, and the second is the
    /// less obvious one:
    ///
    /// - minijinja has a real, multiline `{% raw %}` tag of its own, so the
    ///   markers are consumed and the body survives the first pass as the
    ///   literal text `\n{{ var }}\n`;
    /// - that output differs from the input, so [`render_str`]'s re-render
    ///   loop renders it AGAIN — and this time there is no `raw` left to
    ///   protect it, so `{{ var }}` resolves.
    ///
    /// The result is `"\nV"`, which is neither Kestra's error nor the
    /// preserved literal a reader would expect from "minijinja supports raw".
    #[test]
    fn a_multiline_raw_region_is_not_protected_by_the_pattern() {
        let source = "{% raw %}\n{{ var }}\n{% endraw %}";
        let (protected, replacers) = replace_raw_tags(source);
        assert!(replacers.is_empty(), "the pattern must not match a newline");
        assert_eq!(protected, source);
        assert_eq!(render(source, json!({ "var": "V" })).unwrap(), "\nV");
    }

    /// The re-render loop: an output that is itself a template renders again,
    /// exactly as `VariableRenderer.renderRecursively` does.
    #[test]
    fn output_that_is_itself_a_template_renders_again() {
        assert_eq!(
            render(
                "{{ tmpl }}",
                json!({ "tmpl": "{{ inner }}", "inner": "deep" })
            )
            .unwrap(),
            "deep"
        );
    }

    /// A template that never converges is refused rather than looping.
    #[test]
    fn a_non_converging_render_is_refused() {
        // Renders to itself-plus-more on every pass.
        let ctx = json!({ "a": "{{ a }}x" });
        let err = render_str("{{ a }}", &cx(ctx)).unwrap_err();
        assert_eq!(err, RenderError::TooManyRenderingAttempts);
    }

    /// Author-supplied source is bounded. Without fuel this hangs.
    #[test]
    fn a_runaway_loop_runs_out_of_fuel() {
        // Nested loops rather than one huge `range`: minijinja caps a single
        // range's length itself, which would refuse this for a different
        // reason and leave the fuel setting untested.
        let err = render(
            "{% for a in range(1000) %}{% for b in range(1000) %}x{% endfor %}{% endfor %}",
            json!({}),
        )
        .unwrap_err();
        assert!(
            matches!(err, RenderError::Template(ref m) if m.contains("fuel")),
            "unexpected error: {err}"
        );
    }

    /// The two globals that dump internal state are gone. `debug` in
    /// particular prints the whole context.
    #[test]
    fn the_state_dumping_globals_are_removed() {
        for source in ["{{ debug() }}", "{{ secret | pprint }}"] {
            let err = render(source, json!({ "secret": "s3cr3t" })).unwrap_err();
            assert!(
                matches!(err, RenderError::Template(_)),
                "{source} should not render: {err:?}"
            );
        }
    }

    /// `macros` and `multi_template` are off in the workspace's minijinja
    /// features, so these do not parse at all. Pinned here because a future
    /// "just add the feature" would silently give flow authors template
    /// inheritance and file inclusion that Kestra does not have.
    #[test]
    fn macro_and_include_tags_do_not_parse() {
        for source in [
            "{% macro m() %}x{% endmacro %}",
            "{% include 'other' %}",
            "{% extends 'other' %}",
        ] {
            assert!(
                render(source, json!({})).is_err(),
                "{source} must not parse"
            );
        }
    }

    /// A non-object context has no top-level bindings to make, and silently
    /// rendering against an empty one would turn a caller's bug into a
    /// "variable is undefined" a page away from its cause.
    #[test]
    fn a_non_object_context_is_rejected_rather_than_emptied() {
        assert!(RenderContext::from_json_object(&json!([1, 2])).is_none());
        assert!(RenderContext::from_json_object(&json!("s")).is_none());
        assert!(RenderContext::from_json_object(&json!(null)).is_none());
        assert_eq!(
            RenderContext::from_json_object(&json!({ "a": 1 }))
                .unwrap()
                .variables()
                .len(),
            1
        );
    }
}
