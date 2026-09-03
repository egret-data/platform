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
use std::fmt;
use std::io;
use std::sync::Arc;

use minijinja::value::{Enumerator, Object, ObjectRepr, Value as MjValue, ValueKind};
use minijinja::{AutoEscape, Environment, Error as MjError, State, UndefinedBehavior};
use regex::Regex;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use super::filters;
use super::functions;
use super::rewrite::{rewrite, RewriteError};

/// `VariableRenderer.MAX_RENDERING_AMOUNT`. A render whose output is itself a
/// template renders again; this bounds that.
///
/// Note the arithmetic, because it is easy to be one out: Kestra's
/// `renderRecursively(int renderingCount, ...)` throws when
/// `renderingCount > MAX_RENDERING_AMOUNT` and is first called with `0`, so
/// `renderOnce` runs at counts `0..=100` — **101 passes**, not 100. The loop
/// below is `0..=MAX_RENDERING_AMOUNT` for that reason.
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
/// and still bounded.
///
/// **What it does and does not bound.** It bounds *accumulated* output: the
/// render stops the moment the total crosses the budget, which is what the
/// re-render loop needs, since each pass renders the previous pass's output.
/// It does **not** bound a single value's own allocation — `{{ 'a' * 8000000 }}`
/// builds the whole 8 MB string inside minijinja's `s.repeat(n)` before the
/// sink ever sees a byte. Bounding that would need a limit inside the engine,
/// which minijinja does not offer.
///
/// [dataapi]: ../../../egret_dataapi/render/index.html
///
/// **`runners::filters` derives its own pre-allocation bound from this.**
/// `indent`/`nindent` can compute the size of their result before building it,
/// so they refuse above this budget instead of allocating and then failing —
/// see `runners::filters::apply_indent`. That is deliberately a bound Kestra
/// does not have, for the same reason `rewrite::MAX_NESTING_DEPTH` is.
pub(super) const MAX_OUTPUT_BYTES: usize = 1 << 20;

/// The stack every render gets, whoever called it.
///
/// # Why the render owns a thread
///
/// **The hazard here is not per value type; it is per thread.** Every
/// recursive descent over author-supplied input in this crate runs on
/// whatever stack the caller happened to have — in production a 2 MiB tokio
/// worker — and a recursive descent that overflows is `SIGABRT`, which
/// `catch_unwind` does not see and no test harness can report. Four rounds of
/// this package bounded one descent each, always the axis of the last
/// exploit: bracket nesting in the jq source, then jaq's parser, then the
/// depth of a jq value, then a regex character class. The list was never the
/// problem; the scope was.
///
/// Running the whole render on a thread whose stack egret chooses covers the
/// list at once, and makes every per-construct cap derivable from ONE number
/// instead of from a guess about the caller. `oracle/README.md` carries the
/// table of descents and what bounds each.
///
/// **The number is derived from the deepest per-construct cap, measured.**
/// The most expensive descent is `javare`'s class parser at about 2 KiB a
/// level (bisected; see [`super::javare::MAX_CLASS_DEPTH`] for the table),
/// and Java's own limit -- which egret must reach to be faithful -- is 10 000
/// levels. 10 000 x 2 KiB = 20 MB, so 128 MiB carries a 6.4x margin. A tokio
/// worker's 2 MiB holds 1 046 of those levels, which is why this exists.
///
/// It is reserved address space, committed lazily: a render that nests
/// nothing touches a few KiB of it, and the reservation lasts as long as the
/// render, not as long as the process.
pub const RENDER_STACK_BYTES: usize = 128 * 1024 * 1024;

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
/// A `Vec<(String, minijinja::Value)>` rather than a map, because **the order
/// is part of the contract**: Kestra's variables are `LinkedHashMap`s and
/// Jackson writes them in insertion order (`oracle/corpus.json`'s
/// `map-key-order`), and every map type available here — `BTreeMap`,
/// `serde_json::Map` without `preserve_order`, minijinja's own `ValueMap` —
/// sorts. See [`OrderedMap`], which is the same decision one level down.
///
/// A named type rather than a bare alias so the render seam has one thing to
/// name, and so W3 has one place to build `{{ inputs.* }}` / `{{ outputs.* }}`
/// / `{{ trigger.* }}` into.
///
/// W2 does not build an execution context — that is W3's job (see the crate
/// doc). This type is the shape W3 will fill.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderContext(Vec<(String, MjValue)>);

impl RenderContext {
    /// An empty context. A template that references any variable will fail
    /// against it, by design — see this module's strictness note.
    pub fn new() -> Self {
        Self::default()
    }

    /// Binds one top-level name, at the end of the current order.
    ///
    /// Rebinding an existing name replaces its value **in place**, keeping its
    /// original position, which is what a `LinkedHashMap.put` does.
    pub fn insert(&mut self, key: impl Into<String>, value: JsonValue) -> &mut Self {
        let key = key.into();
        let value = json_to_minijinja(&value);
        match self.0.iter_mut().find(|(name, _)| *name == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
        self
    }

    /// The bindings, in order, for callers that need to inspect them.
    pub fn bindings(&self) -> &[(String, MjValue)] {
        &self.0
    }

    /// Builds a context from a JSON object.
    ///
    /// **Key order here is incidental, not guaranteed.** `serde_json::Map`
    /// is an `IndexMap` only while `serde_json`'s `preserve_order` feature is
    /// on, and in this workspace it is on **transitively** — `utoipa`
    /// (workspace `Cargo.toml:61`) and `cedar-policy-core` both enable it, so
    /// cargo's feature unification turns it on for everyone. Measured, not
    /// assumed: `serde_json::from_str::<Value>(r#"{"b":1,"a":2}"#)`
    /// round-trips as `{"b":1,"a":2}` in this tree.
    ///
    /// That is a dependency's decision, not this crate's, and it can change
    /// without anything here changing. Use [`RenderContext::from_json_str`]
    /// when order matters — which it does against Kestra, whose maps are
    /// `LinkedHashMap`s (`oracle/corpus.json`'s `map-key-order`).
    ///
    /// Returns `None` for anything that is not an object: a template's
    /// variables are a name-to-value map and there is no sensible top-level
    /// binding for a bare array or scalar. Callers that must not silently
    /// render against an empty context should treat `None` as an error rather
    /// than falling back to [`RenderContext::new`].
    pub fn from_json_object(value: &JsonValue) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self(
            object
                .iter()
                .map(|(k, v)| (k.clone(), json_to_minijinja(v)))
                .collect(),
        ))
    }

    /// Builds a context from JSON **text**, preserving the order the keys
    /// appear in — at every level, not just the top.
    ///
    /// This is the order-preserving entry point. serde's map visitor sees
    /// entries in document order, so parsing straight into this type keeps
    /// what `serde_json::Value` would already have thrown away.
    pub fn from_json_str(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The context as one minijinja value: an order-preserving map object.
    fn to_minijinja(&self) -> MjValue {
        MjValue::from_object(OrderedMap::new(self.0.clone()))
    }
}

/// Renders the bindings as JSON, in order, **including** nulls.
///
/// Diagnostics only (the oracle's mismatch report). Deliberately not the same
/// as [`write_json`]'s `omit_nulls` mode: a debug line that hides the null a
/// reader is trying to explain is worse than no debug line.
impl fmt::Display for RenderContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        write_json(&mut out, &self.to_minijinja(), false);
        f.write_str(&out)
    }
}

/// A minijinja map that iterates in **insertion** order.
///
/// Needed because **minijinja's own `ValueMap` is a `BTreeMap`** unless its
/// `preserve_order` feature is on, and it is not (the workspace names six
/// minijinja features and that is not one of them). Measured directly rather
/// than inferred: `Value::from_serialize` on a `serde_json::Value` holding
/// `{"b":1,"a":2,"c":3}` renders `{"a":2,"b":1,"c":3}`.
///
/// Note it is *only* minijinja that sorts here. `serde_json`'s own
/// `preserve_order` is already on in this workspace, transitively via
/// `utoipa` and `cedar-policy-core` — the same round-trip through
/// `serde_json::Value` alone keeps `{"b":1,"a":2,"c":3}`. Turning
/// minijinja's on would be a workspace-level feature change affecting
/// `egret-dataapi`'s SQL renderer too, so the order-preserving map is local
/// to this crate instead and **no workspace-level feature list was touched**.
///
/// Linear lookup is deliberate: a render context is a handful of top-level
/// names and small nested objects, and a `Vec` is what keeps the order.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct OrderedMap {
    entries: Vec<(String, MjValue)>,
    /// Built on the first lookup, and only for a map wide enough to need it.
    ///
    /// **Every value writer in this crate walks a map as "enumerate the keys,
    /// then `get_item` each one"**, which is minijinja's `Object` API and not
    /// something a writer can avoid. With a linear `find` behind `get_value`
    /// that is O(n^2) per render, and it is reachable from a plain context
    /// object: measured on the render path, `{{ v }}` over a context map took
    /// 8 ms at 1 000 keys, **745 ms at 10 000** and **7 370 ms at 40 000**,
    /// and `yaml(yaml=...)` over a 100 000-key document took **43 seconds**.
    ///
    /// Found by W2 Task 7 attacking its own `fromJson`/`fromIon`/`yaml`
    /// readers, which are the first things in the crate that let a flow author
    /// build a map that wide from a **filter argument** rather than from a
    /// context. The cost was already there for a context value; the readers
    /// widened the reach.
    ///
    /// 32 is a threshold, not a bound: below it a linear scan over a `Vec` is
    /// cheaper than hashing the key, and every map a real flow renders is far
    /// below it.
    index: std::sync::OnceLock<std::collections::HashMap<String, usize>>,
}

/// Below this many entries, `get_value` scans. See [`OrderedMap::index`].
const ORDERED_MAP_INDEX_THRESHOLD: usize = 32;

impl OrderedMap {
    pub(super) fn new(entries: Vec<(String, MjValue)>) -> Self {
        OrderedMap {
            entries,
            index: std::sync::OnceLock::new(),
        }
    }
}

impl Object for OrderedMap {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Map
    }

    fn get_value(self: &Arc<Self>, key: &MjValue) -> Option<MjValue> {
        let key = key.as_str()?;
        if self.entries.len() <= ORDERED_MAP_INDEX_THRESHOLD {
            return self
                .entries
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone());
        }
        let index = self.index.get_or_init(|| {
            // The FIRST occurrence wins, matching the linear `find` above --
            // a duplicate key would otherwise answer differently depending on
            // how wide the map happened to be.
            let mut index = std::collections::HashMap::with_capacity(self.entries.len());
            for (at, (name, _)) in self.entries.iter().enumerate() {
                index.entry(name.clone()).or_insert(at);
            }
            index
        });
        index.get(key).map(|at| self.entries[*at].1.clone())
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Values(
            self.entries
                .iter()
                .map(|(name, _)| MjValue::from(name.clone()))
                .collect(),
        )
    }

    fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
        Some(self.entries.len())
    }
}

/// `serde_json::Value` -> `minijinja::Value`, mapping objects onto
/// [`OrderedMap`] so everything downstream sees one map representation.
///
/// The order is whatever the `serde_json::Value` has, which for an object is
/// sorted — see [`RenderContext::from_json_object`]. This exists so the
/// convenience constructors keep working, not because it is the good path.
fn json_to_minijinja(value: &JsonValue) -> MjValue {
    match value {
        JsonValue::Null => MjValue::from(()),
        JsonValue::Bool(b) => MjValue::from(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                MjValue::from(i)
            } else if let Some(u) = n.as_u64() {
                MjValue::from(u)
            } else {
                MjValue::from(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        JsonValue::String(s) => MjValue::from(s.clone()),
        JsonValue::Array(items) => {
            MjValue::from(items.iter().map(json_to_minijinja).collect::<Vec<_>>())
        }
        JsonValue::Object(map) => MjValue::from_object(OrderedMap::new(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_minijinja(v)))
                .collect(),
        )),
    }
}

/// Deserialises straight into ordered `minijinja::Value`s, never through
/// `serde_json::Value`.
///
/// serde hands a map visitor its entries in document order; going through
/// `serde_json::Value` first would sort them before this code ever ran. That
/// is the whole reason this visitor exists rather than a `From` impl.
pub(super) struct OrderedJson(pub(super) MjValue);

impl<'de> Deserialize<'de> for OrderedJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(OrderedJsonVisitor)
    }
}

struct OrderedJsonVisitor;

impl<'de> Visitor<'de> for OrderedJsonVisitor {
    type Value = OrderedJson;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(OrderedJson(MjValue::from(())))
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.visit_unit()
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
        Ok(OrderedJson(MjValue::from(v)))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(OrderedJson(MjValue::from(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(OrderedJson(MjValue::from(v)))
    }

    /// **A context integer wider than 64 bits arrives here as a FLOAT, and
    /// its digits are already gone.**
    ///
    /// `{"v": 18446744073709551616}` renders `1.8446744073709552E19` where
    /// Kestra renders the exact digits — through `{{ v }}`, `toJson`, `yaml`,
    /// `toIon` and `jq` — and `className` says `java.lang.Double` where
    /// Jackson says `java.math.BigInteger`. Jackson reads such a literal as a
    /// `BigInteger`; `serde_json` without `arbitrary_precision` converts it to
    /// an `f64` **inside its own parser**, before any `Visitor` method is
    /// called, so `visit_i128`/`visit_u128` do not help and are deliberately
    /// not implemented here — a visitor that is never called, documented as a
    /// fix, is worse than none.
    ///
    /// The fix is `serde_json`'s `arbitrary_precision`, and Cargo features
    /// are additive across the graph: turning it on changes
    /// `serde_json::Value`'s number representation for **every** crate in
    /// this workspace. That is not W2's call to make.
    ///
    /// Recorded rather than hidden: `context-integer-past-u64` and its
    /// siblings are the entries, baselined with this reason.
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Ok(OrderedJson(MjValue::from(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(OrderedJson(MjValue::from(v.to_string())))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::new();
        while let Some(OrderedJson(item)) = seq.next_element()? {
            items.push(item);
        }
        Ok(OrderedJson(MjValue::from(items)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entries: Vec<(String, MjValue)> = Vec::new();
        while let Some((key, OrderedJson(value))) = map.next_entry::<String, OrderedJson>()? {
            entries.push((key, value));
        }
        Ok(OrderedJson(MjValue::from_object(OrderedMap::new(entries))))
    }
}

impl<'de> Deserialize<'de> for RenderContext {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ContextVisitor;

        impl<'de> Visitor<'de> for ContextVisitor {
            type Value = RenderContext;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object of template variables")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some((key, OrderedJson(value))) =
                    map.next_entry::<String, OrderedJson>()?
                {
                    entries.push((key, value));
                }
                Ok(RenderContext(entries))
            }
        }

        d.deserialize_map(ContextVisitor)
    }
}

impl From<BTreeMap<String, JsonValue>> for RenderContext {
    fn from(map: BTreeMap<String, JsonValue>) -> Self {
        Self(
            map.iter()
                .map(|(k, v)| (k.clone(), json_to_minijinja(v)))
                .collect(),
        )
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
/// Two divergences are known, measured, and **deliberately left alone,
/// because egret is the better of the two**: Kestra calls `Object::toString()`
/// on every element with no null check, so `{{ 'a' isIn l }}` with
/// `l = [null, "a"]` and `{{ 'v' isIn m }}` with `m = {"k": null}` both throw
/// a `NullPointerException` in real Pebble, where egret answers `true` and
/// `false`. Do not "fix" this into a panic to match: a port that reproduces
/// an upstream NPE has turned a working answer into a crash.
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
    // `InExpression`'s `leftValue == null || rightValue == null` guard, and
    // ONLY that. It is about a Java `null`, which Pebble keeps distinct from
    // an absent variable: `{{ nothere isIn ['a'] }}` throws
    // `RootAttributeNotFoundException` under `strictVariables(true)` rather
    // than returning `false` (`oracle/corpus.json`'s `isin-undefined-left`,
    // `isin-undefined-right`, `isin-missing-attr-left`).
    //
    // **Do not add `is_undefined()` here.** minijinja does not apply
    // `UndefinedBehavior::Strict` to function ARGUMENTS (see this module's
    // strictness note), so moving `isIn` from an operator to a call means an
    // undefined operand arrives here instead of raising. Swallowing it would
    // make a typo'd variable inside an `isIn` silently decide a branch, which
    // is strictly worse than the three divergences this function was written
    // to fix. Returning the undefined unchanged is what makes it raise at the
    // point of use, exactly as it would have as a bare operator.
    if left.is_undefined() {
        return Ok(left);
    }
    if right.is_undefined() {
        return Ok(right);
    }
    if left.is_none() || right.is_none() {
        return Ok(MjValue::from(false));
    }

    // `kestra_to_string`, NOT `MjValue::to_string`. See that function: the
    // `Display` impl is not routed through `env.set_formatter`, so using it
    // here compares `"True"` against Kestra's `"true"` and silently flips
    // the answer.
    let needle = kestra_to_string(&left);

    match right.kind() {
        ValueKind::Seq | ValueKind::Iterable => {
            let mut items = right.try_iter()?;
            Ok(MjValue::from(
                items.any(|item| kestra_to_string(&item) == needle),
            ))
        }
        ValueKind::Map => {
            for key in right.try_iter()? {
                if kestra_to_string(&key) == needle {
                    return Ok(MjValue::from(true));
                }
                if let Ok(value) = right.get_item(&key) {
                    if kestra_to_string(&value) == needle {
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

/// Java's `Object::toString()` for the values that reach an expression —
/// **not** minijinja's `Display`.
///
/// `Display for Value` does not go through [`environment`]'s formatter, so
/// everything [`json_writer_formatter`] fixes is undone by any code that
/// reaches for `.to_string()`. Measured on minijinja 2.24:
///
/// | value | `Value::to_string()` | Java `toString()` |
/// |---|---|---|
/// | `true` | `True` | **`true`** |
/// | `1e100` | `1000000…0.0` (101 digits) | **`1.0E100`** |
///
/// Not academic: `InExpression` compares **stringified** operands, so the
/// wrong spelling silently flips the answer. No corpus entry caught it until
/// `isin-bool-lowercase`, `isin-bool-uppercase`, `isin-float-scientific`,
/// `isin-float-plain` and `isin-int` were added for exactly this. Anything
/// that stringifies a value for comparison against Kestra's own must come
/// through here.
///
/// **The `None` arm writes Java's `"null"`, not minijinja's `"None"`** —
/// Python's spelling, which appears nowhere in Kestra's vocabulary and which
/// `escapeChar(type='shell')` would once have interpolated into a command
/// line. `kestra_is_in` cannot reach it (its first guard answers `false` on a
/// null operand, which is what `isin-null-vs-none` pins), but the container
/// arms above do, through `AbstractCollection.toString`'s `String.valueOf`.
///
/// **`Undefined` is NOT that arm, and the distinction is load-bearing.** A
/// first cut of this fix folded the two together and broke five filters that
/// had been correct — see the `None` arm's comment. `Object::toString()` on a
/// null reference throws; `"null"` is the container element rule only.
///
/// Total over the kinds a Kestra expression can produce, which is narrower
/// than total over `ValueKind`; see the fallthrough's comment for the residue.
pub(super) fn kestra_to_string(value: &MjValue) -> String {
    match value.kind() {
        ValueKind::Bool => (if value.is_true() { "true" } else { "false" }).to_string(),
        ValueKind::Number if !value.is_integer() => match f64::try_from(value.clone()) {
            Ok(f) => {
                let mut buf = String::new();
                write_java_double(&mut buf, f);
                buf
            }
            Err(_) => value.to_string(),
        },
        // `AbstractCollection.toString`: square brackets, `", "` between
        // elements -- WITH the space, unlike [`write_json`], which is the
        // JSON writer and has none. `{{ list }}` is `[1,2]` and
        // `{{ list | string }}` is `[1, 2]`, from the same list, because they
        // go through different Java methods. `filter-string-list` pins it.
        ValueKind::Seq | ValueKind::Iterable => match value.try_iter() {
            Ok(items) => {
                let rendered: Vec<String> = items.map(|item| kestra_to_string(&item)).collect();
                format!("[{}]", rendered.join(", "))
            }
            Err(_) => value.to_string(),
        },
        // `AbstractMap.toString`: `{k=v, k2=v2}` -- an `=` and no quotes,
        // which is nothing like minijinja's `{"a": 1}`. `filter-string-map`.
        ValueKind::Map => match value.try_iter() {
            Ok(keys) => {
                let rendered: Vec<String> = keys
                    .map(|key| {
                        let item = value.get_item(&key).unwrap_or_default();
                        format!("{}={}", kestra_to_string(&key), kestra_to_string(&item))
                    })
                    .collect();
                format!("{{{}}}", rendered.join(", "))
            }
            Err(_) => value.to_string(),
        },
        // `String.valueOf(null)` is the four characters `null`, and that is an
        // ELEMENT rule -- `AbstractCollection.toString` / `AbstractMap.
        // toString` are the callers that reach it. `Object::toString()` on a
        // null reference throws and never returns `"null"`, so this arm is
        // about containers, not about a top-level value.
        //
        // **`Undefined` is deliberately NOT folded in here.** It was, briefly,
        // and that broke five filters that had been right: Pebble's `null` is
        // a real literal but minijinja has no such name, so `{{ null | … }}`
        // arrives as an undefined, and Kestra renders it as nothing. Undefined
        // falls through to `Display`, which writes the empty string -- which is
        // what Kestra does. `runners::filters::is_java_null` is where an
        // undefined is recognised as Pebble's null, at the filter's own
        // `input == null` branch, which is the level Kestra makes the decision
        // at too.
        ValueKind::None => "null".to_string(),
        // Everything else — strings, `Undefined` (see above), and the `Plain`
        // objects `runners::filters` builds for Java's `BigDecimal`/
        // `BigInteger` — has a `Display` that is either its Java `toString` or,
        // for `Undefined`, the empty string Kestra renders for a null.
        //
        // `ValueKind::Bytes` and `ValueKind::Invalid` also land here and are
        // the residue: nothing in this crate constructs either, no corpus
        // entry reaches them, and their `Display` is minijinja's own. This
        // function is total over the kinds a Kestra expression can produce,
        // which is a narrower claim than total over `ValueKind`.
        _ => value.to_string(),
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
///
/// **This formatter is not the only place a value becomes text.** `Display for
/// Value` bypasses it entirely, so anything that stringifies for comparison
/// (`kestra_is_in`) must use [`kestra_to_string`] instead — see that function
/// for the two spellings that flip an answer.
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
        ValueKind::Seq | ValueKind::Map | ValueKind::Iterable => {
            let mut json = String::new();
            write_json(&mut json, value, true);
            out.write_str(&json)?;
            Ok(())
        }
        // A non-integer number goes through `StringBuilder.append(double)` in
        // `JsonWriter.writeSpecialized(double)`, i.e. `Double.toString`.
        // minijinja's own float rendering is not that -- see
        // [`write_java_double`].
        ValueKind::Number if !value.is_integer() => {
            let mut buf = String::new();
            match f64::try_from(value.clone()) {
                Ok(f) => write_java_double(&mut buf, f),
                // A number that is neither an integer nor an f64 has no
                // Java counterpart to imitate; render it as minijinja would
                // rather than guess.
                Err(_) => buf.push_str(&value.to_string()),
            }
            out.write_str(&buf)?;
            Ok(())
        }
        _ => minijinja::escape_formatter(out, state, value),
    }
}

/// Writes `value` as JSON the way Jackson's `JacksonMapper.ofJson()` does.
///
/// Three things differ from `serde_json::to_string`, each measured against a
/// corpus entry rather than assumed:
///
/// - **key order** is the map's own (insertion) order, not sorted --
///   `map-key-order`: `{"b":1,"a":2,"c":3}`, not `{"a":2,"b":1,"c":3}`;
/// - **null map entries are omitted, recursively**, because the mapper is
///   configured `JsonInclude.Include.NON_NULL` -- `map-nested-null`:
///   `{"a":null,"b":{"c":null,"d":1},"e":[null,1]}` renders
///   `{"b":{"d":1},"e":[null,1]}`. Note the array element is **kept**:
///   `NON_NULL` applies to object properties, not to sequence elements, and
///   that entry exists to pin exactly that asymmetry;
/// - **floats** use `Double.toString` -- see [`write_java_double`].
///
/// `omit_nulls` is `false` only for [`RenderContext`]'s `Display`, which is
/// diagnostics and must not hide the null a reader is trying to explain.
pub(super) fn write_json(out: &mut String, value: &MjValue, omit_nulls: bool) {
    match value.kind() {
        ValueKind::Undefined | ValueKind::None => out.push_str("null"),
        ValueKind::Bool => out.push_str(if value.is_true() { "true" } else { "false" }),
        ValueKind::Number => {
            if value.is_integer() {
                out.push_str(&value.to_string());
            } else {
                match f64::try_from(value.clone()) {
                    // JSON has no infinity and no NaN, so Jackson writes them
                    // as QUOTED strings -- `[Infinity]` is `["Infinity"]`.
                    // Reachable only through `jq` (a JSON context cannot carry
                    // one), which is why no golden moved when this was added.
                    Ok(f) if !f.is_finite() => {
                        let mut text = String::new();
                        write_java_double(&mut text, f);
                        write_json_string(out, &text);
                    }
                    Ok(f) => write_java_double(out, f),
                    Err(_) => out.push_str(&value.to_string()),
                }
            }
        }
        ValueKind::Seq | ValueKind::Iterable => {
            out.push('[');
            let mut first = true;
            if let Ok(items) = value.try_iter() {
                for item in items {
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    write_json(out, &item, omit_nulls);
                }
            }
            out.push(']');
        }
        ValueKind::Map => {
            out.push('{');
            let mut first = true;
            if let Ok(keys) = value.try_iter() {
                for key in keys {
                    let Ok(item) = value.get_item(&key) else {
                        continue;
                    };
                    if omit_nulls && (item.is_none() || item.is_undefined()) {
                        continue;
                    }
                    if !first {
                        out.push(',');
                    }
                    first = false;
                    write_json_string(out, &key.to_string());
                    out.push(':');
                    write_json(out, &item, omit_nulls);
                }
            }
            out.push('}');
        }
        // A `BigDecimal`/`BigInteger` is a NUMBER, and it is `ValueKind::Plain`
        // -- so without this arm it fell into the catch-all below and was
        // quoted as a string. `{{ [x] }}` on a `number(type='BIGDECIMAL')` was
        // `["1.50"]` here and is `[1.50]` in Kestra, and no filter was
        // involved. See `super::filters::big_number_text`.
        _ if super::filters::big_number_text(value).is_some() => {
            out.push_str(&super::filters::big_number_text(value).expect("checked"));
        }
        // Strings, and anything else that has no JSON shape of its own.
        _ => write_json_string(out, &value.to_string()),
    }
}

/// A JSON string literal. Delegated to `serde_json` so the escape set is the
/// library's and not a hand-rolled one; Jackson's default escaping is the
/// same (quote, backslash, control characters, and non-ASCII left as UTF-8).
///
/// **One byte-level difference had to be fixed on top of it**: Jackson writes
/// the four hex digits of a `\uXXXX` escape in UPPER case and `serde_json`
/// writes them in lower case, so `"a\u000Bb"` in Kestra was `"a\u000bb"`
/// here. It is only reachable for the control characters whose code point
/// has a hex letter in it — `0x0B`, `0x0E`, `0x0F` and `0x1A`-`0x1F`, since
/// `\b \t \n \f \r` have named escapes — which is why no existing golden
/// moved when this was fixed and why only W2 Task 6's `toJson` sweep found
/// it. Verified: the mismatch count is unchanged either side of this fix.
fn write_json_string(out: &mut String, s: &str) {
    match serde_json::to_string(s) {
        Ok(quoted) => out.push_str(&uppercase_unicode_escapes(&quoted)),
        // `to_string` on a `&str` cannot fail; this arm keeps the writer
        // total rather than panicking in a formatter.
        Err(_) => {
            out.push('"');
            out.push('"');
        }
    }
}

/// Uppercases the hex digits of every `\uXXXX` escape in an already-quoted
/// JSON string, which is the one thing `serde_json`'s escaping spells
/// differently from Jackson's.
fn uppercase_unicode_escapes(quoted: &str) -> String {
    if !quoted.contains("\\u") {
        return quoted.to_string();
    }
    let bytes: Vec<char> = quoted.chars().collect();
    let mut out = String::with_capacity(quoted.len());
    let mut i = 0;
    while i < bytes.len() {
        // A `\\` consumes both characters, so the `u` of a literal `\\u` in
        // the source is never read as the start of an escape.
        if bytes[i] == '\\' && i + 1 < bytes.len() {
            if bytes[i + 1] == 'u' && i + 5 < bytes.len() {
                out.push('\\');
                out.push('u');
                for c in &bytes[i + 2..i + 6] {
                    out.push(c.to_ascii_uppercase());
                }
                i += 6;
                continue;
            }
            out.push(bytes[i]);
            out.push(bytes[i + 1]);
            i += 2;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// Java's `Double.toString`, which is what `JsonWriter` reaches through
/// `StringBuilder.append(double)`.
///
/// Two formats, chosen by magnitude, and neither is Rust's `{}` or `{:?}`:
///
/// - `1e-3 <= |d| < 1e7` -> plain decimal with **at least one** fractional
///   digit: `1.0`, `0.1`, `0.001`, `9999999.0`, `-1.5`;
/// - otherwise -> scientific, mantissa in `[1,10)` with at least one
///   fractional digit, `E`, and a bare exponent: `1.0E100`, `1.0E-7`,
///   `1.2345678E7`.
///
/// Every one of those is a value in `oracle/corpus.json`'s `float-list`,
/// whose golden was produced by real Kestra; none of them is a guess about
/// what Java does.
///
/// Rust's `{:e}` supplies the shortest round-tripping digits and the base-10
/// exponent, and for every value in the normal range that is the same digit
/// sequence Java picks, so this only has to re-lay-out what it gives.
///
/// **The digit sequences diverge at subnormals.** `5e-324` (the smallest
/// positive double) is `4.9E-324` in Java and `5.0E-324` here: both round-trip
/// to the same bit pattern, but Java's shortest-decimal algorithm and Rust's
/// break the tie differently down there. No corpus entry reaches it and no
/// plausible flow does; recorded so the claim above is not read as
/// unconditional.
pub(super) fn write_java_double(out: &mut String, x: f64) {
    if x.is_nan() {
        out.push_str("NaN");
        return;
    }
    if x.is_infinite() {
        out.push_str(if x > 0.0 { "Infinity" } else { "-Infinity" });
        return;
    }

    let sci = format!("{x:e}");
    let (mantissa, exponent) = match sci.split_once('e') {
        Some(parts) => parts,
        None => {
            out.push_str(&sci);
            return;
        }
    };
    let exponent: i32 = exponent.parse().unwrap_or(0);

    let (sign, mantissa) = match mantissa.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();

    out.push_str(sign);

    if (-3..7).contains(&exponent) {
        if exponent < 0 {
            out.push_str("0.");
            for _ in 0..(-exponent - 1) {
                out.push('0');
            }
            out.push_str(&digits);
        } else {
            let point = exponent as usize + 1;
            if digits.len() <= point {
                out.push_str(&digits);
                for _ in 0..(point - digits.len()) {
                    out.push('0');
                }
                out.push_str(".0");
            } else {
                out.push_str(&digits[..point]);
                out.push('.');
                out.push_str(&digits[point..]);
            }
        }
    } else {
        out.push_str(&digits[..1]);
        out.push('.');
        if digits.len() > 1 {
            out.push_str(&digits[1..]);
        } else {
            out.push('0');
        }
        out.push('E');
        out.push_str(&exponent.to_string());
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

    // `PebbleEngineFactory.newPebbleEngineBuilder().newLineTrimming(false)`.
    // minijinja's default is the OPPOSITE: it drops one trailing newline from
    // every template source.
    //
    // That default is not merely cosmetic here, because [`render_str`] feeds
    // each pass's OUTPUT back in as the next pass's SOURCE. A value whose
    // last character is a newline therefore loses it on the pass after the
    // one that produced it -- silently, in a loop nothing else in this file
    // is about. W2 Task 6's `yaml` filter is the first thing in this crate
    // whose output ends in a newline, and `filter-yaml` is the entry that
    // found it; every earlier filter's value happened not to.
    env.set_keep_trailing_newline(true);

    // Two minijinja built-ins that have no Pebble counterpart and both dump
    // internal state: `debug` is `format!("{state:#?}")`, which recovers the
    // whole template context plus an inventory of every registered filter,
    // test and global; `pprint` is the `Debug` repr of a value. A flow author
    // is not an anonymous caller, but a rendered `Property` can end up in a
    // log, an output or a downstream task's arguments, and neither of these
    // exists in the language being ported -- removing them costs nothing.
    //
    // The rest of minijinja's global inventory is `range`, `dict` and
    // `namespace`. **`range` is Kestra's too**, and W2 Task 7 overrides it in
    // `super::functions` -- inclusive where minijinja's is exclusive, with
    // named arguments and a 1 000 000-element cap. `dict` and `namespace`
    // stay: neither exists in Pebble, but neither leaks anything either, and
    // reconciling the remaining built-in surface is Task 8's.
    env.remove_global("debug");
    env.remove_filter("pprint");

    // The port of `JsonWriter.write(Object)` -- null, booleans and containers.
    // See [`json_writer_formatter`] for the four-row table; three of the four
    // differ from minijinja's default, and none of them is reachable from a
    // filter or function, because `{{ list }}` names neither.
    env.set_formatter(json_writer_formatter);

    // W2 Task 4's 22 string/collection filters. **Two of them, `indent` and
    // `string`, OVERRIDE minijinja built-ins of the same name** -- those
    // names resolve to minijinja's own answer rather than erroring as
    // unknown, so nothing but the oracle would have noticed. See
    // `super::filters`' module doc for the table of what each one answered
    // before.
    filters::register(&mut env);

    // W2 Task 7's 19 pure functions. **`range` OVERRIDES a minijinja global**
    // of the same name, which resolves and answers minijinja's way rather
    // than erroring as unknown -- the same trap `indent` and `string` are,
    // one layer up. See `super::functions`' module doc.
    functions::register(&mut env);

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
///
/// # A note on cost
///
/// This builds a fresh [`environment`] on every call — registering the
/// functions, removing the globals, setting the formatter — which is wasted
/// work when a caller renders many properties against one context. It is
/// deliberately not optimised here: W3 is what introduces bulk rendering (a
/// task's whole property set at once), and that is where a shared, reused
/// `Environment` belongs, with a clear owner for its lifetime.
pub fn render_str(template: &str, cx: &RenderContext) -> Result<String, RenderError> {
    // `VariableRenderer.java:76-79`: "it's not a Pebble template so we
    // short-circuit rendering". Note the test is `{`, not `{{` -- a source
    // containing a lone `{` still goes through the engine. Done before the
    // thread, because a source with no brace does no work at all.
    if !template.contains('{') {
        return Ok(template.to_string());
    }

    // **On a thread whose stack egret chooses.** See [`RENDER_STACK_BYTES`]
    // for why this is here rather than a fifth per-construct bound.
    // `RenderContext` is `Send + Sync` (minijinja's `Value` is), so this is a
    // scoped borrow and not a copy -- `render_context_crosses_a_thread`
    // pins that, because the day it stops being true this has to become a
    // serialise-and-rebuild and nothing else would say so.
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("egret-render".to_string())
            .stack_size(RENDER_STACK_BYTES)
            .spawn_scoped(scope, || render_on_this_thread(template, cx))
            .map_err(|e| RenderError::Template(format!("could not start a render thread: {e}")))?;
        match worker.join() {
            Ok(result) => result,
            // A panic below is still a panic -- it is resumed on this thread
            // when the scope ends, so `tests/oracle.rs`'s guard still sees it.
            Err(_) => Err(RenderError::Template("the render panicked".to_string())),
        }
    })
}

/// The render loop, on the thread [`render_str`] gave it.
fn render_on_this_thread(template: &str, cx: &RenderContext) -> Result<String, RenderError> {
    let env = environment();
    let variables = cx.to_minijinja();

    let mut current = template.to_string();
    // Inclusive: see [`MAX_RENDERING_AMOUNT`]'s note on Kestra's off-by-one.
    for _ in 0..=MAX_RENDERING_AMOUNT {
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
/// This bounds *accumulated* output across a whole render, and stops at the
/// first write that would cross the budget rather than building a finished
/// oversized `String` and measuring it afterwards. It does not bound what a
/// single expression allocates on its way here — see [`MAX_OUTPUT_BYTES`].
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
///
/// This runs **after** [`render_once`]'s size check, so the string it returns
/// can be larger than [`MAX_OUTPUT_BYTES`] — by the difference between a
/// 36-byte UUID and the raw region it stands for. That is bounded by the
/// source template's own length, not by anything an expression can generate,
/// so it is not a resource-exhaustion path; but the cap's name promises a
/// bound the return value of this function does not strictly satisfy, and
/// that is worth knowing rather than discovering.
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

    /// A map past [`ORDERED_MAP_INDEX_THRESHOLD`] answers the same questions
    /// the linear scan does, including the one the two arms could disagree
    /// about.
    ///
    /// The index exists because every value writer walks a map as "enumerate
    /// the keys, then `get_item` each one", which made a render quadratic in
    /// the map's width -- 7 370 ms for a 40 000-key context object, 43 s for a
    /// 100 000-key `yaml()` argument, both measured on the render path.
    ///
    /// The risk the index introduces is a DUPLICATE key: a `HashMap` built by
    /// insertion would answer the last occurrence where `Vec::find` answers
    /// the first. `or_insert` is what keeps them the same, and this is what
    /// says so. Both halves of the threshold are exercised, because a test
    /// that only used a wide map would pass with the scan deleted.
    #[test]
    fn a_wide_map_answers_exactly_what_the_narrow_one_does() {
        for width in [4usize, ORDERED_MAP_INDEX_THRESHOLD, 5_000] {
            let mut entries: Vec<(String, MjValue)> = (0..width)
                .map(|i| (format!("k{i}"), MjValue::from(i)))
                .collect();
            // The same name twice: the FIRST wins on both paths.
            entries.push(("k0".to_string(), MjValue::from("second")));
            let map = MjValue::from_object(OrderedMap::new(entries));

            assert_eq!(
                map.get_item(&MjValue::from("k0")).unwrap(),
                MjValue::from(0usize),
                "width {width}: a duplicate key must answer its FIRST occurrence"
            );
            assert_eq!(
                map.get_item(&MjValue::from(format!("k{}", width - 1)))
                    .unwrap(),
                MjValue::from(width - 1),
                "width {width}"
            );
            assert!(map
                .get_item(&MjValue::from("absent"))
                .unwrap()
                .is_undefined());
            assert_eq!(map.try_iter().unwrap().count(), width + 1, "width {width}");
        }
    }

    /// `??` falls back for BOTH a present-but-null and an absent variable;
    /// `???` falls back only for the absent one. `oracle/golden.json`'s
    /// `coalesce-null` / `undef-coalesce-null` pair is the measured proof that
    /// collapsing the two is wrong, and this is that pair as a unit test so a
    /// regression does not have to wait for the oracle to be read.
    ///
    /// Mutation-checked, both directions, before this was committed: with
    /// `kestra_null_coalesce` returning its LEFT operand unconditionally (the
    /// implementation a reader might assume), the first assertion renders
    /// `""` and this fails; with `kestra_undefined_coalesce` collapsed into
    /// `kestra_null_coalesce`, the second renders `"fallback"` and this
    /// fails.
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

    /// Renders from JSON **text**, so key order survives -- the only entry
    /// point that can, and the one `tests/oracle.rs` uses.
    fn render_ordered(template: &str, json: &str) -> Result<String, RenderError> {
        let cx = RenderContext::from_json_str(json).expect("test context must be a JSON object");
        render_str(template, &cx)
    }

    /// Kestra's variables are `LinkedHashMap`s and Jackson writes them in
    /// insertion order; both `serde_json::Map` and minijinja's own `ValueMap`
    /// are `BTreeMap`s by default, so without [`OrderedMap`] this renders the
    /// keys sorted. `oracle/corpus.json`'s `map-key-order` is the golden.
    ///
    /// The context is parsed from TEXT on purpose: routing it through a
    /// `serde_json::Value` first would sort the keys before this code ran, and
    /// the test would pass against a broken implementation.
    #[test]
    fn map_keys_render_in_document_order_not_sorted() {
        assert_eq!(
            render_ordered("{{ m }}", r#"{"m": {"b": 1, "a": 2, "c": 3}}"#).unwrap(),
            r#"{"b":1,"a":2,"c":3}"#
        );
        // Order survives being handed to the engine and coming back, which is
        // not free: minijinja round-trips a context through `Serialize`.
        assert_eq!(
            render_ordered(
                "{% for k in m %}{{ k }}{% endfor %}",
                r#"{"m": {"b": 1, "a": 2}}"#
            )
            .unwrap(),
            "ba"
        );
        // The `serde_json::Value` path happens to agree TODAY, because
        // `serde_json`'s `preserve_order` is on transitively (utoipa,
        // cedar-policy-core). That is a dependency's decision and could
        // change under us, which is why the oracle uses `from_json_str`.
        // Pinned as an observation, not as a guarantee.
        assert_eq!(
            render("{{ m }}", json!({ "m": { "b": 1, "a": 2, "c": 3 } })).unwrap(),
            r#"{"b":1,"a":2,"c":3}"#
        );
        // What genuinely sorts is minijinja's own ValueMap, which is why
        // `OrderedMap` exists. This is that layer, directly.
        assert_eq!(
            serde_json::to_string(&MjValue::from_serialize(
                serde_json::from_str::<JsonValue>(r#"{"b":1,"a":2,"c":3}"#).unwrap()
            ))
            .unwrap(),
            r#"{"a":2,"b":1,"c":3}"#
        );
    }

    /// Jackson is configured `JsonInclude.Include.NON_NULL`, so a null map
    /// entry is omitted -- at every level. A null **array element** is kept,
    /// because `NON_NULL` is about object properties. `map-nested-null` exists
    /// to pin exactly that asymmetry.
    #[test]
    fn null_map_entries_are_omitted_recursively_but_null_array_elements_are_not() {
        assert_eq!(
            render_ordered(
                "{{ m }}",
                r#"{"m": {"a": null, "b": {"c": null, "d": 1}, "e": [null, 1]}}"#
            )
            .unwrap(),
            r#"{"b":{"d":1},"e":[null,1]}"#
        );
        assert_eq!(
            render_ordered("{{ m }}", r#"{"m": {"a": null}}"#).unwrap(),
            "{}"
        );
    }

    /// `JsonWriter` reaches `Double.toString` through
    /// `StringBuilder.append(double)`, which is neither Rust's `{}` nor its
    /// `{:?}`. Every value here is in `oracle/corpus.json`'s `float-list`, so
    /// every expected string below was produced by real Kestra.
    #[test]
    fn floats_render_the_way_java_double_tostring_does() {
        assert_eq!(
            render_ordered(
                "{{ fs }}",
                "{\"fs\": [1.0, 0.1, 2.5, 1e100, 1e-7, 12345678.0, 0.001, 0.0001, 9999999.0, -1.5]}"
            )
            .unwrap(),
            "[1.0,0.1,2.5,1.0E100,1.0E-7,1.2345678E7,0.001,1.0E-4,9999999.0,-1.5]"
        );
        // Also at the top level, not only inside a container.
        assert_eq!(render_ordered("{{ f }}", r#"{"f": 1.0}"#).unwrap(), "1.0");
        assert_eq!(
            render_ordered("{{ f }}", r#"{"f": 1e100}"#).unwrap(),
            "1.0E100"
        );
        // Integers are not floats and must not grow a `.0`.
        assert_eq!(
            render_ordered("{{ ns }}", r#"{"ns": [0, 1, -1, 1000000]}"#).unwrap(),
            "[0,1,-1,1000000]"
        );
    }

    /// `write_java_double`'s two branches and their boundary, directly.
    /// `1e-3 <= |d| < 1e7` is plain decimal; outside it is scientific.
    #[test]
    fn java_double_formatting_switches_at_the_documented_boundaries() {
        let f = |x: f64| {
            let mut s = String::new();
            write_java_double(&mut s, x);
            s
        };
        assert_eq!(f(0.0), "0.0");
        assert_eq!(f(-0.0), "-0.0");
        assert_eq!(f(0.001), "0.001");
        assert_eq!(f(0.0009), "9.0E-4");
        assert_eq!(f(9999999.0), "9999999.0");
        assert_eq!(f(10000000.0), "1.0E7");
        assert_eq!(f(1000000.0), "1000000.0");
        assert_eq!(f(f64::NAN), "NaN");
        assert_eq!(f(f64::INFINITY), "Infinity");
        assert_eq!(f(f64::NEG_INFINITY), "-Infinity");
    }

    /// **C2.** `isIn` became a function call, and minijinja does not apply
    /// `UndefinedBehavior::Strict` to function arguments -- so without care an
    /// undefined operand reaches `kestra_is_in` and gets answered `false`
    /// instead of raising. That would let a typo'd variable silently decide a
    /// branch, which is worse than the divergences the function fixed.
    ///
    /// All three rows are corpus entries whose golden is a Pebble
    /// `RootAttributeNotFoundException` / `AttributeNotFoundException`.
    #[test]
    fn an_undefined_is_in_operand_raises_rather_than_answering_false() {
        for source in [
            "{{ nothere isIn ['a'] }}",
            "{{ 'a' isIn nothere }}",
            "{{ obj.missing isIn ['a'] }}",
        ] {
            let err = render(source, json!({ "obj": { "a": 1 } })).unwrap_err();
            assert!(
                matches!(err, RenderError::Template(ref m) if m.contains("undefined")),
                "{source} must raise, got: {err:?}"
            );
        }
        // A bound null is a different state and IS `false` -- that is
        // `InExpression`'s own `leftValue == null` guard.
        assert_eq!(
            render("{{ n isIn ['a'] }}", json!({ "n": null })).unwrap(),
            "false"
        );
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
        // **The assertion that makes the null guard falsifiable.** The two
        // `['a']` cases above pass with the guard DELETED -- whatever a
        // `none` stringifies to, it is not `"a"` -- so they were not the test
        // they looked like. This needle IS what a `none` stringifies to:
        // `Display for Value` gives `"None"`, capital N, measured (NOT
        // `"none"`, which would leave this test just as unfalsifiable as the
        // ones above). Kestra answers `false` here via `InExpression`'s null
        // guard; unguarded egret answers `true`.
        assert_eq!(
            render("{{ n isIn ['None'] }}", json!({ "n": null })).unwrap(),
            "false"
        );

        // Operands are stringified Kestra's way, not minijinja's. Without
        // `kestra_to_string` a bool compares as `"True"` and a large float as
        // its 101-digit plain form, and every one of these flips.
        assert_eq!(
            render("{{ b isIn ['true'] }}", json!({ "b": true })).unwrap(),
            "true"
        );
        assert_eq!(
            render("{{ b isIn ['True'] }}", json!({ "b": true })).unwrap(),
            "false"
        );
        assert_eq!(
            render_ordered("{{ f isIn ['1.0E100'] }}", r#"{"f": 1e100}"#).unwrap(),
            "true"
        );
        assert_eq!(
            render("{{ i isIn ['42'] }}", json!({ "i": 42 })).unwrap(),
            "true"
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
    /// The result is `"\nV\n"`, which is neither Kestra's error nor the
    /// preserved literal a reader would expect from "minijinja supports raw".
    ///
    /// **The trailing newline is part of the value and used not to be.** It
    /// was eaten by minijinja's default `keep_trailing_newline(false)` acting
    /// on the SECOND pass of the re-render loop, where the first pass's output
    /// is the template source. Pebble is configured `newLineTrimming(false)`
    /// and this environment now matches it; W2 Task 6's `yaml` filter is what
    /// made that visible, since it is the first filter whose value ends in a
    /// newline.
    /// **The render runs on a thread egret spawns, so the context must be
    /// able to cross one.**
    ///
    /// `render_str` borrows the caller's `RenderContext` into a scoped thread
    /// with `env::RENDER_STACK_BYTES` of stack. That compiles only while
    /// `RenderContext` is `Send + Sync`, and it is a property of the TYPE, not
    /// of any call: adding an `Rc` field anywhere inside it — jaq's own value
    /// is `Rc`-based, so it is one careless `pub` away — breaks the whole
    /// render path, and this says so at the type rather than at the use.
    #[test]
    fn the_render_context_can_cross_a_thread() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<RenderContext>();
        assert_sync::<RenderContext>();
        assert_send::<MjValue>();
        assert_sync::<MjValue>();
    }

    /// **The render's own recursion depth, which belongs to no filter.**
    ///
    /// `{{ v | lower | lower | ... }}` x 1 600 is minijinja's evaluator
    /// recursing over its own AST: no filter in this crate is involved, no
    /// per-construct cap sees it, and on a 2 MiB tokio worker it SIGABRTed
    /// the process. It is the case that proves the fix had to be at the
    /// render's entry point rather than at a filter -- the hazard is
    /// per-thread, not per-value-type.
    ///
    /// Kestra answers `a`, measured through the harness.
    #[test]
    fn a_render_nested_past_a_worker_stack_still_answers() {
        let mut cx = RenderContext::default();
        cx.insert("v", serde_json::json!("A"));
        let template = format!("{{{{ v{} }}}}", " | lower".repeat(1_600));
        assert_eq!(render_str(&template, &cx).expect("renders"), "a");
    }

    /// The stack the render thread asks for is a number this crate DERIVES,
    /// and two of its consumers point back at it. If it is lowered, the
    /// class-nesting test's process dies rather than failing politely, so the
    /// number is stated here where a reader will see it change.
    #[test]
    fn the_render_stack_is_the_size_the_deepest_cap_needs() {
        assert_eq!(RENDER_STACK_BYTES, 128 * 1024 * 1024);
        // 10 000 class levels x ~2 KiB measured = ~20 MB, so the margin is
        // better than sixfold. A change to either side must move both.
        const {
            assert!(
                RENDER_STACK_BYTES >= crate::runners::javare::MAX_CLASS_DEPTH * 2 * 1024 * 6,
                "the render stack no longer covers the deepest per-construct cap"
            )
        };
    }

    #[test]
    fn a_multiline_raw_region_is_not_protected_by_the_pattern() {
        let source = "{% raw %}\n{{ var }}\n{% endraw %}";
        let (protected, replacers) = replace_raw_tags(source);
        assert!(replacers.is_empty(), "the pattern must not match a newline");
        assert_eq!(protected, source);
        assert_eq!(render(source, json!({ "var": "V" })).unwrap(), "\nV\n");
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
    ///
    /// **The `range` here is Kestra's, not minijinja's.** This test used to
    /// read `range(1000)` and passed on minijinja's own positional global;
    /// W2 Task 7 overrode that name with `BoundedRangeFunction`, whose
    /// arguments are `start`/`end`, and the test then failed with an
    /// argument error rather than exercising fuel at all. It is the same trap
    /// `indent` and `string` were, seen from the other side: the override was
    /// deliberate and the caller was not.
    #[test]
    fn a_runaway_loop_runs_out_of_fuel() {
        // Nested loops rather than one huge range: Kestra caps a single
        // range at 1 000 000 elements, which would refuse this for a
        // different reason and leave the fuel setting untested.
        let err = render(
            "{% for a in range(start=1, end=1000) %}\
             {% for b in range(start=1, end=1000) %}x{% endfor %}{% endfor %}",
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
                .bindings()
                .len(),
            1
        );
    }
}
