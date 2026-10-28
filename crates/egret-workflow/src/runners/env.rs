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
//!
//! # W3-BLOCKING: three bounds that are not bounds yet
//!
//! **Read this before the first caller.** These are measured, not suspected,
//! and they are unreachable today for exactly one reason: **nothing in the
//! workspace calls this crate.** W3 is the change that makes them reachable —
//! an execution context is author-controlled data arriving on a tokio worker —
//! so each is a prerequisite of W3, not a deferred nice-to-have. None of them
//! is "low risk"; each is a process abort or a permanent refusal reachable
//! from a flow's own inputs.
//!
//! 1. **[`RenderContext::from_json_object`] recurses on the CALLER's thread.**
//!    It walks author data through `json_to_minijinja` with no depth bound and
//!    aborts around depth 2,600 on a 2 MiB stack — which is what a tokio
//!    worker has. [`RenderContext::from_json_str`] is correctly bounded at 127
//!    (`serde_json`'s own parser limit); the two entry points do not agree.
//!    Note the `serde_json::Value` the caller passes has already been parsed
//!    *somewhere*, so the 127 does not protect this path.
//! 2. **[`put_back_raw_tags`] runs AFTER `render_once`'s size check.** A
//!    `{% for %}` over a raw region therefore amplifies past the cap without
//!    the cap seeing it: measured 985×, a 4,157-byte source returning
//!    4,096,000 bytes in 3.76 s. [`CappedSink`] cannot notice, because during
//!    the render it only ever holds the 36-byte UUID placeholders — the
//!    expansion happens when they are swapped back. The fix is to check the
//!    size again after substitution, not to make the sink cleverer.
//! 3. **`filters::REPLACE_BUDGET` is a thread-local that is never reset on a
//!    caller-owned thread.** [`render_str`] installs a fresh budget on the
//!    render thread it spawns, so it is fine; a caller who renders on its own
//!    thread gets 5,368 renders and then a permanent refusal for the life of
//!    that thread. The doc's escape hatch, `ReplaceBudget::install`, is
//!    `pub(super)` — no caller outside `runners` can reach it, so the
//!    documented remedy is not available to the party who needs it.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::sync::Arc;

use minijinja::value::{Enumerator, Object, ObjectRepr, Rest, Value as MjValue, ValueKind};
use minijinja::{AutoEscape, Environment, Error as MjError, State, UndefinedBehavior};
use regex::Regex;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Value as JsonValue;

use super::files::{FileScope, FileScopeObject, FILE_SCOPE};
use super::filters;
use super::functions;
use super::rewrite::{rewrite, RewriteError};
use crate::storage::kv::value::{too_deep, KvData, KvNumber, Unstorable};

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
pub(crate) const MAX_OUTPUT_BYTES: usize = 1 << 20;

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
///
/// **W9a Task 4: and the files it may read.** [`RenderContext::with_files`]
/// attaches a [`FileScope`]; both render threads then bind it as the
/// [`FILE_SCOPE`] global, and without one the four file functions refuse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderContext {
    bindings: Vec<(String, MjValue)>,
    files: Option<FileScope>,
    /// Values no line a run writes may carry (KI-112): its execution's SECRET
    /// inputs. Not a binding; nothing renders them from here.
    redactions: Vec<String>,
}

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
        match self.bindings.iter_mut().find(|(name, _)| *name == key) {
            Some(slot) => slot.1 = value,
            None => self.bindings.push((key, value)),
        }
        self
    }

    /// The bindings, in order, for callers that need to inspect them.
    pub fn bindings(&self) -> &[(String, MjValue)] {
        &self.bindings
    }

    /// The same bindings, with the files the four file functions read.
    pub fn with_files(mut self, files: FileScope) -> Self {
        self.files = Some(files);
        self
    }

    /// The attached file scope, `None` where the four file functions refuse.
    pub fn files(&self) -> Option<&FileScope> {
        self.files.as_ref()
    }

    /// Adds values the run's log lines and failure message are masked with.
    pub fn redact(&mut self, values: &[String]) -> &mut Self {
        self.redactions.extend(values.iter().cloned());
        self
    }

    /// The values [`Self::redact`] added.
    pub fn redactions(&self) -> &[String] {
        &self.redactions
    }

    /// A render against this context has ended: the file scope releases what
    /// it held for it ([`super::files::RenderFiles::end_render`]).
    pub(crate) fn end_render(&self) {
        if let Some(files) = &self.files {
            files.0.end_render();
        }
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
    /// That recommendation was, for one commit, an instruction to walk into a
    /// bug: `from_json_str` deserialises straight through
    /// [`OrderedJsonVisitor`], and under `serde_json`'s `arbitrary_precision`
    /// — on in every build this crate ships in — that visitor's `visit_map`
    /// built the parser's private number sentinel as a literal one-entry map,
    /// so every float became `{"$serde_json::private::Number":"1.0"}`. This
    /// method survived it only because `json_to_minijinja` matches on
    /// `JsonValue::Number`, which the feature leaves intact. Both paths are
    /// correct now (see `visit_map`); the sentence above is safe to follow
    /// again, and is kept together with what it cost.
    ///
    /// Returns `None` for anything that is not an object: a template's
    /// variables are a name-to-value map and there is no sensible top-level
    /// binding for a bare array or scalar. Callers that must not silently
    /// render against an empty context should treat `None` as an error rather
    /// than falling back to [`RenderContext::new`].
    pub fn from_json_object(value: &JsonValue) -> Option<Self> {
        let object = value.as_object()?;
        Some(Self::from_bindings(
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

    fn from_bindings(bindings: Vec<(String, MjValue)>) -> Self {
        Self {
            bindings,
            files: None,
            redactions: Vec::new(),
        }
    }

    /// The context as one minijinja value: an order-preserving map object.
    pub(crate) fn to_minijinja(&self) -> MjValue {
        MjValue::from_object(OrderedMap::new(self.bindings.clone()))
    }

    /// Binds [`FILE_SCOPE`] when a scope is attached. Called by BOTH render
    /// threads, after anything that could unbind it.
    fn bind_files(&self, env: &mut Environment<'static>) {
        if let Some(files) = &self.files {
            env.add_global(
                FILE_SCOPE,
                MjValue::from_object(FileScopeObject(files.clone())),
            );
        }
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
/// `{"b":1,"a":2,"c":3}` comes back keyed a, b, c —
/// `map_keys_render_in_document_order_not_sorted` pins it.
///
/// Since `serde_json`'s `arbitrary_precision` was declared, that same
/// round-trip also turns each **number** into the parser's private sentinel
/// map, which is a second and independent reason nothing in this crate uses
/// `from_serialize`. The test carries the exact string.
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
    /// Entries are passed through [`last_value_wins`], so an `OrderedMap`
    /// never holds a key twice. That is the crate-wide invariant, enforced in
    /// one place rather than at every construction site.
    pub(super) fn new(entries: Vec<(String, MjValue)>) -> Self {
        OrderedMap {
            entries: last_value_wins(entries),
            index: std::sync::OnceLock::new(),
        }
    }
}

/// `java.util.LinkedHashMap.put`: a repeated key keeps its **first position**
/// and takes its **last value**.
///
/// Measured, both halves, through the harness:
/// `fromJson('{"a":1,"b":2,"a":3}')` is `{"a":3,"b":2}` -- the `3` wins and it
/// is still first. `yaml` and `fromIon` agree, and so does
/// [`RenderContext::insert`], which has said so in its own doc since W2
/// Task 3.
///
/// # What this is fixing
///
/// W2 Task 7's two new readers pushed a duplicate as a **second entry**. The
/// later value was not mis-selected -- it was discarded and replaced by a
/// second copy of the earlier one, which then propagated through `toJson`,
/// `keys`, `length` and every writer:
///
/// | | Kestra | egret, before |
/// |---|---|---|
/// | `fromJson('{"a":1,"a":2}')` | `{"a":2}` | `{"a":1,"a":1}` |
/// | the same, `.a` | `2` | `1` |
/// | the same, through `keys` | `["a"]` | `["a","a"]` |
///
/// `super::ionfmt`'s struct reader, in the **same commit**, got it right and
/// said so in a comment. Putting the rule here rather than at each reader is
/// what makes that impossible to get half-right again -- and it lets
/// `ionfmt` drop its own per-field linear scan, which was quadratic in a
/// struct's width (26 s for an 80 000-field document, against 17 ms for the
/// same width through `fromJson`).
///
/// Returns the input untouched when there is no duplicate, which is every
/// real map: nothing is allocated until one is found.
fn last_value_wins(mut entries: Vec<(String, MjValue)>) -> Vec<(String, MjValue)> {
    // Narrow maps are the overwhelming case and a scan beats hashing there;
    // the threshold is the one [`OrderedMap::index`] already uses.
    let first_seen: Option<std::collections::HashMap<&str, usize>> =
        (entries.len() > ORDERED_MAP_INDEX_THRESHOLD).then(|| {
            let mut seen = std::collections::HashMap::with_capacity(entries.len());
            for (at, (name, _)) in entries.iter().enumerate() {
                seen.entry(name.as_str()).or_insert(at);
            }
            seen
        });
    let duplicated = match &first_seen {
        Some(seen) => seen.len() != entries.len(),
        None => entries
            .iter()
            .enumerate()
            .any(|(at, (name, _))| entries[..at].iter().any(|(other, _)| other == name)),
    };
    if !duplicated {
        return entries;
    }

    let mut out: Vec<(String, MjValue)> = Vec::with_capacity(entries.len());
    let mut at_of: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (name, value) in entries.drain(..) {
        match at_of.get(&name) {
            Some(at) => out[*at].1 = value,
            None => {
                at_of.insert(name.clone(), out.len());
                out.push((name, value));
            }
        }
    }
    out
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
            // `or_insert` rather than `insert`, so this and the linear scan
            // above agree even if a duplicate ever reached here.
            // [`last_value_wins`] means one cannot, and that is the point: the
            // two paths cannot disagree about a key that does not exist.
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

    /// A plain `f64`. **No longer the funnel every non-`i64`/`u64` number
    /// arrives through** — see [`OrderedJsonVisitor::visit_map`].
    ///
    /// # What this used to say, and why it was wrong
    ///
    /// This doc used to record `context-integer-past-u64` as permanent: a
    /// literal wider than 64 bits reached here as an `f64` with its digits
    /// already gone, because `serde_json` **without** `arbitrary_precision`
    /// narrows such a literal inside its own parser, before any `Visitor`
    /// method is called. It then said the fix was that feature, and that
    /// "Cargo features are additive across the graph: turning it on changes
    /// `serde_json::Value`'s number representation for every crate in this
    /// workspace. That is not W2's call to make."
    ///
    /// **The feature was already on.** `starlark` 0.14.2 enables it,
    /// `egret-dataapi` depends on starlark, and `egret-bin` — the shipping
    /// binary — pulls `egret-dataapi` and `egret-workflow` together, so
    /// unification turned it on in every build this crate actually ships in.
    /// The sentence above reasoned about *turning a feature on* and never
    /// asked whether it was **already on**; the answer was one command:
    ///
    /// ```text
    /// $ cargo tree -p egret-bin --format "{p} [{f}]" | grep serde_json
    /// serde_json v1.0.151 [alloc,arbitrary_precision,default,indexmap,preserve_order,raw_value,std]
    /// ```
    ///
    /// [`RenderContext::from_json_object`]'s doc, 250 lines above, had run
    /// exactly that check for `preserve_order` and written "Measured, not
    /// assumed". Two docs in one file, one measured and one reasoned, and
    /// only the reasoned one was wrong. **Check the graph; do not argue about
    /// it.** `crates/egret-workflow/Cargo.toml` now names
    /// `arbitrary_precision` explicitly so this crate's own resolution equals
    /// the shipping one;
    /// `the_arbitrary_precision_sentinel_is_still_spelled_this_way` fails by
    /// name if that declaration is dropped; and `just
    /// test-shipping-resolution` (also a step in `.github/workflows/ci.yml`)
    /// compiles this crate's tests with `-p egret-bin` selected, so the next
    /// such feature cannot drift in silently either.
    ///
    /// # What reaches this method now
    ///
    /// With the feature on, `serde_json`'s parser hands `visit_u64`/
    /// `visit_i64` any literal that fits, and routes **everything else** —
    /// every float, and every integer past 64 bits — through `visit_map`
    /// under its `$serde_json::private::Number` sentinel. So this method is
    /// reached only by a non-JSON `Deserializer` that calls it directly
    /// (`serde_norway`'s, for one). It stays, and stays correct, for those.
    ///
    /// `visit_i128`/`visit_u128` are still deliberately not implemented:
    /// `serde_json` never calls them either way, and the digits now arrive
    /// through the sentinel as text.
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

    /// A JSON object — **or a number wearing an object's clothes.**
    ///
    /// With `serde_json`'s `arbitrary_precision` on (it is; see
    /// [`OrderedJsonVisitor::visit_f64`] and this crate's `Cargo.toml`), the
    /// parser stops narrowing numbers itself. Anything that does not fit
    /// `i64`/`u64` — every float, and every integer past 64 bits — is handed
    /// to `deserialize_any` as a **one-entry map** whose key is the private
    /// sentinel [`ARBITRARY_PRECISION_TOKEN`] and whose value is the literal's
    /// original text. A visitor that faithfully builds that as a map turns
    /// every float in a context into
    /// `{"$serde_json::private::Number":"1.0"}`, which is exactly what this
    /// one did: 13 lib tests and both oracle tests failed in the shipping
    /// resolution while `cargo test -p egret-workflow` was green.
    ///
    /// Recognising the sentinel and parsing the text back is also what
    /// **closes** `context-integer-past-u64`: `18446744073709551616` arrives
    /// here as those exact twenty digits and becomes a `u128`, where before
    /// the feature it had already been narrowed to `1.8446744073709552e19`
    /// inside the parser.
    ///
    /// `serde_json`'s own `Value` visitor does the same thing, so a document
    /// that really does contain that key as author data collapses to a number
    /// on **both** paths ([`RenderContext::from_json_str`] and
    /// [`RenderContext::from_json_object`]) rather than on one of them — the
    /// two entry points agree, which is the property that matters here.
    ///
    /// The recognition is **narrow, and falls back rather than failing**: one
    /// entry, that exact key, a string payload, and a payload that parses as
    /// a number. Anything else is a map, and stays one. The first cut of this
    /// method errored on a second entry instead, and
    /// `the_arbitrary_precision_sentinel_is_still_spelled_this_way` — this
    /// commit's own test — rejected a perfectly ordinary two-key context
    /// because of it. A visitor over author data has no business inventing
    /// errors for shapes the format allows.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut entries: Vec<(String, MjValue)> = Vec::new();
        while let Some((key, OrderedJson(value))) = map.next_entry::<String, OrderedJson>()? {
            entries.push((key, value));
        }

        if let [(key, payload)] = entries.as_slice() {
            if key == ARBITRARY_PRECISION_TOKEN {
                if let Some(number) = payload.as_str().and_then(number_from_literal) {
                    return Ok(OrderedJson(number));
                }
            }
        }

        Ok(OrderedJson(MjValue::from_object(OrderedMap::new(entries))))
    }
}

/// `serde_json`'s `arbitrary_precision` sentinel key.
///
/// Spelled out rather than imported because the constant behind it is
/// `pub(crate) const TOKEN` (`serde_json-1.0.151/src/number.rs:18`) and no
/// public item re-exports it — `serde_json::value::TOKEN` does not exist.
/// `the_arbitrary_precision_sentinel_is_still_spelled_this_way` pins the
/// spelling against a `serde_json` bump by reading the key back off a real
/// serializer rather than by asserting the string against itself.
pub(super) const ARBITRARY_PRECISION_TOKEN: &str = "$serde_json::private::Number";

/// A JSON number literal's **text** back to an exact integer, or `None` if it
/// is not one.
///
/// Widest-exact-first: `i64`, then `u64`, then `i128`, then `u128`. The order
/// matters — `18446744073709551616` is past `u64::MAX` by one and is exact as
/// a `u128`, and failing over to `f64` for it is precisely the loss
/// `context-integer-past-u64` recorded.
///
/// Separate from [`number_from_literal`] because [`super::functions`]'s reader
/// must not hand a **non-finite** `f64` on to a value: `fromJson('1e400')`
/// used to be rejected by `serde_json`'s parser and, with
/// `arbitrary_precision` on, is not — it now reaches this crate as text. See
/// `functions::finite_or_malformed`.
pub(super) fn integer_from_literal(literal: &str) -> Option<MjValue> {
    if let Ok(i) = literal.parse::<i64>() {
        return Some(MjValue::from(i));
    }
    if let Ok(u) = literal.parse::<u64>() {
        return Some(MjValue::from(u));
    }
    if let Ok(i) = literal.parse::<i128>() {
        return Some(MjValue::from(i));
    }
    literal.parse::<u128>().ok().map(MjValue::from)
}

/// [`integer_from_literal`], falling back to `f64`.
///
/// A literal past `u128` still lands on `f64` and still loses digits Jackson
/// keeps. That is a much narrower class than the one this closes, and nothing
/// in `oracle/corpus.json` reaches it.
fn number_from_literal(literal: &str) -> Option<MjValue> {
    integer_from_literal(literal).or_else(|| literal.parse::<f64>().ok().map(MjValue::from))
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
                // The top-level bindings are a `LinkedHashMap` too, and this
                // is the one map in the crate that does not go through
                // `OrderedMap`. `RenderContext::insert` has always collapsed a
                // rebinding in place; the deserialiser did not.
                Ok(RenderContext::from_bindings(last_value_wins(entries)))
            }
        }

        d.deserialize_map(ContextVisitor)
    }
}

impl From<BTreeMap<String, JsonValue>> for RenderContext {
    fn from(map: BTreeMap<String, JsonValue>) -> Self {
        Self::from_bindings(
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
/// binary operator. Since package #5a Task A5 (spec D6) this is reached only
/// when the left operand is NOT a path (a call, a filter, arithmetic): a path
/// compiles to an inline `if` on [`kestra_present`] instead, which is lazy.
/// For what still reaches here, a right operand that *errors* fails the
/// whole render where Kestra would never have reached it -- the recorded
/// divergence `coalesce-non-path-left-missing-right`. See [`rewrite`]'s
/// module doc.
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

/// A path read that never raises -- the left operand of `??`/`???` when it is
/// a path (spec D6; [`rewrite`] compiles `obj.a['b'][0]` there to
/// `kestra_path(obj, 'a', 'b', 0)`).
///
/// Pebble's `GetAttributeExpression` under strict variables throws
/// `AttributeNotFoundException` for a missing attribute or index and
/// `RootAttributeNotFoundException` (a subclass, read with `javap`: "Attempt
/// to get attribute of null object") for a step from `null`; both coalescing
/// operators catch it and answer their right side. Here each of those is
/// `Undefined`, which [`kestra_present`] and [`kestra_defined`] both read as
/// missing. A missing root arrives as `Undefined` already (function
/// arguments are not subject to strict undefined). `.name` and `['name']` are
/// one operation in Pebble, so both are an item lookup here.
fn kestra_path(_state: &State, root: MjValue, steps: Rest<MjValue>) -> MjValue {
    let mut current = root;
    for step in steps.iter() {
        if current.is_undefined() || current.is_none() {
            return MjValue::UNDEFINED;
        }
        current = match current.get_item(step) {
            Ok(next) => next,
            Err(_) => return MjValue::UNDEFINED,
        };
    }
    current
}

/// `??`'s test on a path: present is defined and not null
/// (`NullCoalescingExpression` answers the left side only when it is
/// non-null and did not throw).
fn kestra_present(_state: &State, value: MjValue) -> bool {
    !value.is_undefined() && !value.is_none()
}

/// `???`'s test on a path: defined, null included
/// (`UndefinedCoalescingExpression` answers the left side unless it threw).
fn kestra_defined(_state: &State, value: MjValue) -> bool {
    !value.is_undefined()
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
        // **A `BigDecimal`'s `String.valueOf` is `toString()`, not
        // `toPlainString()`.** `'1E2' | number(type='BIGDECIMAL') | string` is
        // `1E+2` in Kestra; this fell through to `Display`, which is
        // `BigNumber::render`, which is the PLAIN string — so it answered
        // `100`. The two spellings differ whenever the scale is negative or
        // the adjusted exponent is below -6, and `filter-string-bigdecimal`
        // is the entry.
        _ if super::filters::big_number_text(value).is_some() => {
            super::filters::big_number_text(value).expect("checked")
        }
        // Everything else — strings, `Undefined` (see above) — has a `Display`
        // that is either its Java `toString` or, for `Undefined`, the empty
        // string Kestra renders for a null.
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
pub(crate) fn write_json_string(out: &mut String, s: &str) {
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
/// **One digit is never enough for Java.** `Double.toString` (JDK 19+, the
/// Raffaello Giulietti algorithm Kestra's JVM runs) takes the shortest decimal
/// that round-trips, but when that is ONE digit it takes instead the
/// two-digit decimal closest to the double. In the normal range the two agree
/// (`2.0`, `1.0E-5`); at subnormals the rounding interval is wide enough that
/// they do not: the smallest positive double is `5e-324` shortest and
/// `4.9E-324` in Java, twice it `1e-323` and `9.9E-324` (measured with
/// `jshell` on JDK 25, fix wave #5a M9). So a one-digit result is replaced by
/// Rust's correctly rounded two-digit form whenever that round-trips too.
pub(crate) fn write_java_double(out: &mut String, x: f64) {
    if x.is_nan() {
        out.push_str("NaN");
        return;
    }
    if x.is_infinite() {
        out.push_str(if x > 0.0 { "Infinity" } else { "-Infinity" });
        return;
    }

    let mut sci = format!("{x:e}");
    if !sci.contains('.') {
        let two = format!("{x:.1e}");
        // `d.0` is the one-digit form again; only a different second digit
        // is Java's correction.
        if !two.contains(".0e") && two.parse::<f64>() == Ok(x) {
            sci = two;
        }
    }
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
/// Every filter `minijinja` 2.24.0 binds by default under this workspace's
/// feature set.
///
/// Derived from the lock file's `defaults::build_builtin_filters` — `safe`,
/// `escape` and `e` unconditionally, the rest under `builtins`, and **not**
/// `tojson` or `urlencode`, whose features are off. The workspace pins
/// minijinja with `=`, not `^`, precisely so this list cannot go stale under a
/// minor release: see the note beside the dependency.
///
/// It exists so [`the_built_in_surface_matches_the_engines`] can COMPUTE the
/// names egret binds rather than trust the fixture to list them — which is the
/// half of that check a mutant found missing: a filter registered under a name
/// with no row was invisible to it.
#[cfg(test)]
const MINIJINJA_DEFAULT_FILTERS: &[&str] = &[
    "safe",
    "escape",
    "e",
    "lower",
    "upper",
    "title",
    "capitalize",
    "replace",
    "length",
    "count",
    "dictsort",
    "items",
    "reverse",
    "trim",
    "join",
    "split",
    "lines",
    "default",
    "d",
    "round",
    "abs",
    "int",
    "float",
    "attr",
    "first",
    "last",
    "min",
    "max",
    "sort",
    "list",
    "string",
    "bool",
    "batch",
    "slice",
    "sum",
    "indent",
    "select",
    "reject",
    "selectattr",
    "rejectattr",
    "map",
    "groupby",
    "unique",
    "chain",
    "zip",
    "pprint",
    "format",
];

/// Every test `minijinja` 2.24.0 binds by default, minus the six whose names
/// are operators.
///
/// The six (`==`, `!=`, `<`, `<=`, `>`, `>=`) are unreachable from template
/// syntax — `is` parses an identifier — so they are neither removed nor
/// listed; `the_operator_spelled_tests_are_unreachable_rather_than_removed`
/// keeps that a fact rather than a belief.
#[cfg(test)]
const MINIJINJA_DEFAULT_TESTS: &[&str] = &[
    "undefined",
    "defined",
    "none",
    "safe",
    "escaped",
    "boolean",
    "odd",
    "even",
    "divisibleby",
    "number",
    "integer",
    "int",
    "float",
    "string",
    "sequence",
    "iterable",
    "mapping",
    "startingwith",
    "endingwith",
    "lower",
    "upper",
    "sameas",
    "eq",
    "equalto",
    "ne",
    "lt",
    "lessthan",
    "le",
    "gt",
    "greaterthan",
    "ge",
    "in",
    "true",
    "false",
    "filter",
    "test",
];

/// The 27 minijinja filters no Pebble engine has a name for.
///
/// Measured, all 27, against `io.kestra:core:2.0.0-rc13` + `CoreExtension` +
/// `EscaperExtension` + `I18nExtension` — the engine answers
/// `Filter [x] does not exist.` for every one of them, and egret answered
/// before this. `oracle/name-surface.txt` is the fixture and
/// [`the_built_in_surface_matches_the_engines`] asserts it.
///
/// **`escape` is deliberately not here**: Pebble's `EscaperExtension`
/// registers that name too, so removing it would refuse a filter Kestra
/// accepts. Its *semantics* differ (Pebble takes a `strategy` argument); that
/// is a divergence row in the fixture, not a removal.
const MINIJINJA_ONLY_FILTERS: &[&str] = &[
    "safe",
    "e",
    "count",
    "dictsort",
    "items",
    "lines",
    "d",
    "round",
    "int",
    "float",
    "attr",
    "min",
    "max",
    "list",
    "bool",
    "batch",
    "sum",
    "select",
    "reject",
    "selectattr",
    "rejectattr",
    "map",
    "groupby",
    "unique",
    "chain",
    "zip",
    "pprint",
];

/// The three minijinja globals no Pebble engine has a name for.
///
/// minijinja's whole global inventory under this workspace's feature set is
/// `range`, `dict`, `debug`, `namespace` — derived from the lock file's
/// `defaults::build_globals`, not from a list. `range` is Kestra's too and
/// `super::functions` overrides it; the other three do not exist in Pebble
/// (`Function or Macro [dict] does not exist.`, measured).
///
/// `debug` was already removed in Task 3 because it dumps the whole template
/// state; it is here as well now, for the ordinary reason.
const MINIJINJA_ONLY_GLOBALS: &[&str] = &["dict", "debug", "namespace"];

/// The 32 minijinja tests no Pebble engine has a name for.
///
/// The engine carries eight (`defined empty even iterable json map null odd`)
/// and minijinja carries 42. Four names overlap and stay — removing `defined`
/// would refuse `{{ x is defined }}`, which Kestra accepts — and the other 32
/// go.
///
/// **Six of minijinja's 38 non-overlapping test names are not in this list and
/// need no removal: `==`, `!=`, `<`, `<=`, `>`, `>=`.** minijinja's `is`
/// operator parses an *identifier*, so those names are unreachable from
/// template syntax at all — `{{ x is ==(1) }}` is a parse error with them
/// registered and without them. Keeping them out of the list keeps every row
/// in `oracle/name-surface.txt` probeable through `render_str`, which is what
/// makes the fixture an assertion rather than a claim.
///
/// The four that stay were **not** semantically identical: Pebble's
/// `even`/`odd` throw a `ClassCastException` on a non-number where
/// minijinja answered `false`, and Pebble's `iterable` is `false` for a
/// String where minijinja's is `true`. Package #5b's Task B2 (spec D8)
/// overrides all three and registers the other four engine tests
/// (`super::pebble_tests`).
///
/// **`none` stays in this list.** Pebble's null test is spelled `null`, and
/// `{{ x is none }}` is `Test [none] does not exist.` there (measured, B2);
/// the rewriter leaves a test name alone, so `null` is bound under its own
/// name rather than by putting minijinja's `none` back.
const MINIJINJA_ONLY_TESTS: &[&str] = &[
    "undefined",
    "none",
    "safe",
    "escaped",
    "boolean",
    "divisibleby",
    "number",
    "integer",
    "int",
    "float",
    "string",
    "sequence",
    "mapping",
    "startingwith",
    "endingwith",
    "lower",
    "upper",
    "sameas",
    "eq",
    "equalto",
    "ne",
    "lt",
    "lessthan",
    "le",
    "gt",
    "greaterthan",
    "ge",
    "in",
    "true",
    "false",
    "filter",
    "test",
];

/// The global under which [`render_on_this_thread`] publishes the render
/// context's own top-level names.
///
/// **It contains a space, so no template can name it.** `minijinja`'s parser
/// reads an identifier after `{{`, exactly as its `is` operator does, so this
/// is unreachable from template syntax with or without it registered — the
/// same argument that keeps `==`, `!=`, `<`, `<=`, `>`, `>=` out of
/// [`MINIJINJA_ONLY_TESTS`], and it is pinned by a test rather than left as
/// prose (`the_context_scope_global_is_unreachable_from_a_template`).
///
/// # Why a global at all
///
/// `FetchContextFunction` drops a whole **scope** whose key set contains
/// `_context`, and `minijinja` exposes no way to tell a name that came from
/// the render context from one a `{% set %}` or a `{% for %}` bound:
/// `State::known_variables` returns the union with no provenance. Measured,
/// the difference is visible — `{"_context":1,"a":2}` with a
/// `{% set y = 9 %}` in front answers `{"y":9}` on Kestra, not `{}` — so a
/// port that dropped everything would be wrong in the one case the guard
/// exists for. This carries the provenance the API does not.
pub(super) const CONTEXT_SCOPE_NAMES: &str = "egret context scope";

/// The render context's own top-level names, as an **opaque object**.
///
/// # Why not a `Seq`
///
/// [`super::functions::fetch_context`] decides "is this name the
/// environment's?" by comparing `State::lookup`'s answer with the global, and
/// `Value`'s `PartialEq` only falls through to `Object::is_same_object` for
/// two **objects**. A `Seq` compares *structurally*, so a context key whose
/// value happened to equal the sentinel was silently deleted from the answer:
/// measured, `{"egret context scope": ["egret context scope"]}` renders on
/// Kestra and yielded `{}` here.
///
/// An `ObjectRepr::Plain` object cannot be produced by any JSON context, so
/// the identity test is now exact rather than nearly exact.
#[derive(Debug)]
pub(super) struct ContextScope(pub(super) Vec<String>);

impl minijinja::value::Object for ContextScope {}

/// The bare minijinja engine: **the unbounded half of the renderer.**
///
/// `pub(crate)` on purpose, and it used to be `pub` and re-exported at the
/// crate root with no doc comment at all, beside a [`render_str`] carrying
/// about two hundred lines of them. A caller who reached for the one that
/// looked like the primitive got none of what makes the other safe. Measured,
/// on this engine as returned:
///
/// | lost | what happens instead |
/// |---|---|
/// | the output cap | `{{ 'a' * 5000000 }}` returns 5,000,000 bytes; through [`render_str`] it is `RenderError::OutputTooLarge` |
/// | the 128 MiB render thread | the render runs on the caller's stack — a 2 MiB tokio worker, in production |
/// | [`super::rewrite`] | `??` and `isIn` are **syntax errors**, so half of Kestra's operator surface does not parse |
/// | `{% raw %}` fidelity | the raw-region protection [`render_str`] wraps every pass in is simply absent |
/// | [`CONTEXT_SCOPE_NAMES`] | the sentinel global is unbound, so `fetchContext()` silently answers an **empty** scope rather than failing |
///
/// And composing the two `pub` items did not merely lose the bounds, it
/// **aborted the process**: `rewrite()` on one corpus expression yields
/// 40,328 bytes of source, which is inside `rewrite`'s own cap, and feeding
/// that to this engine on a 2 MiB stack overflows it. `rewrite()` alone on
/// the same stack is fine, so the defect belonged to the composition — i.e.
/// to the pair of `pub` functions, which is precisely what no caller could
/// have been expected to know.
///
/// **[`render_str`] is the only supported entry point.** It is the pair, in
/// the right order, with every bound attached. Nothing outside this crate
/// called this function when it was narrowed, so nothing was broken by it.
pub(crate) fn environment() -> Environment<'static> {
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

    // ------------------------------------------------------------------
    // W2 Task 8: the built-in surface, reconciled.
    // ------------------------------------------------------------------
    //
    // **egret binds a name the engine does not, and the name RESOLVES.** That
    // is the trap this package has met four times -- `string` and `indent`
    // (Task 4), `range` (Task 7) and `replace` (Task 8) -- and each time what
    // hid it was that minijinja *answered* instead of erroring. Task 4 found
    // `string` only because one corpus entry happened to disagree; nothing
    // would have found it otherwise.
    //
    // Those four are collisions, where both sides have the name. This is the
    // other half: **62 names minijinja binds that the engine has no
    // counterpart for at all** -- 27 filters, 3 globals and 32 tests -- every
    // one of which egret answered and Kestra refuses with
    // `Filter [x] does not exist.` / `Function or Macro [x] does not exist.` /
    // `Test [x] does not exist.`. Measured, all 62, through the harness (`oracle/name-surface.txt` is the fixture and it is asserted
    // bidirectionally by `the_built_in_surface_matches_the_engines`).
    //
    // Over-acceptance is the one divergence this crate's rules do not allow:
    // a flow that renders here and fails on Kestra is exactly the outcome W2
    // exists to prevent, and it fails *later*, in production, rather than at
    // authoring time. So they go.
    //
    // What is NOT removed, and why:
    //
    // - the four names both sides have (`defined`, `even`, `odd`, `iterable`)
    //   stay -- removing them would refuse a construct Kestra accepts, which
    //   is the opposite error. Three of the four DIVERGE, and the fix round
    //   gave `oracle/name-surface.txt` a `semantics` column so the fixture can
    //   say so: before it, the file's own rule forbade a reason on exactly the
    //   both-bind rows, which is where the `replace` class lives;
    // - `escape` stays and is now PORTED, along with sixteen more Pebble-core
    //   filter names the fix round measured (104 probes, 52 divergent);
    // - `kestra_null_coalesce`, `kestra_undefined_coalesce` and `kestra_is_in`
    //   stay: `super::rewrite` compiles Kestra's `??`, `???` and `isIn`
    //   operators into calls to them, so they are a seam this crate cannot do
    //   without. They are egret-only and the fixture says so. Since W9c Task 4
    //   fix round 1 (2026-10-08) nine more are the same kind of seam:
    //   `super::cmp_rewrite` compiles every comparison operator into a call to
    //   one of `super::compare`'s `kestra_eq` .. `kestra_not_in` (eight since
    //   package #5a's Task A2 compiled a chain left-associatively and removed
    //   `kestra_compare_chain`). Since package #5a's Task A3 five more:
    //   `super::cmp_rewrite`'s arithmetic pass compiles every binary `+`, `-`,
    //   `*`, `/`, `%` into `super::arith`'s `kestra_add` .. `kestra_mod`.
    //
    // `pprint` and `debug` were already removed in Task 3 for a different
    // reason (both dump internal state); they are two of the 62 and are now
    // removed for this reason as well.
    for name in MINIJINJA_ONLY_FILTERS {
        env.remove_filter(name);
    }
    for name in MINIJINJA_ONLY_TESTS {
        env.remove_test(name);
    }
    for name in MINIJINJA_ONLY_GLOBALS {
        env.remove_global(name);
    }

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

    // **`defined` was the first test this crate overrode** (W2b), as one name
    // W2b's own change broke, put back; porting the other seven was left to
    // package #5b's Task B2, which registers them just below.
    //
    // Pebble's `DefinedTest` is "present **and not null**". Measured against
    // the artifact, one probe per kind: `0`, `false`, `""`, `[]` and `{}` are
    // all `true`; a null literal, a context null, an absent root and a missing
    // attribute are all `false`. minijinja's is "not undefined", so a `none`
    // is defined there.
    //
    // Before W2b that difference was invisible: the `null` literal arrived as
    // an `Undefined` and answered `false` -- the right answer for the wrong
    // reason. Translating the literal removed the accident and left the real
    // divergence showing, and a **context** null had been answering `true`
    // since long before this package. `null-literal-is-defined` and
    // `null-from-the-context-is-defined` are the entries; `test-defined` (a
    // present String) agrees either way and could never have separated them.
    //
    // Overriding one name costs one name: `add_test` replaces that binding and
    // nothing else. Package #5b's Task B2 (spec D8) registers the other seven
    // engine tests below; which names egret binds itself, and that `none` is
    // still not one of them, is asserted by
    // `the_engine_tests_are_egrets_and_none_is_not_one`.
    env.add_test("defined", |value: MjValue| {
        value.kind() != ValueKind::Undefined && !value.is_none()
    });
    // Package #5b, Task B2 (spec D8): `null`, `empty`, `map`, `json`, and
    // Pebble's `even`, `odd` and `iterable` over minijinja's defaults.
    super::pebble_tests::register(&mut env);

    env.add_function("kestra_null_coalesce", kestra_null_coalesce);
    env.add_function("kestra_undefined_coalesce", kestra_undefined_coalesce);
    // Package #5a Task A5 (spec D6): a path on the left of `??`/`???`
    // compiles to an inline `if` on these three.
    env.add_function("kestra_path", kestra_path);
    env.add_function("kestra_present", kestra_present);
    env.add_function("kestra_defined", kestra_defined);
    env.add_function("kestra_is_in", kestra_is_in);
    // W9c Task 4 fix round 1: the comparison operators, which
    // [`compile_source`]'s second pass compiles into calls to these.
    super::compare::register(&mut env);
    // Package #5a, Task A3: the arithmetic operators, which
    // [`compile_source`]'s third pass compiles into calls to these.
    super::arith::register(&mut env);
    // Package #5a, Task A6: `contains`, `..` and `and`/`or`'s coercion,
    // which [`rewrite`] compiles into calls to these.
    super::operators::register(&mut env);

    env
}

/// What the engine parses: Kestra's operators reshaped by [`rewrite`], then
/// every comparison compiled to a `super::compare` call
/// (`super::cmp_rewrite`, W9c Task 4 fix round 1), then every binary `+`,
/// `-`, `*`, `/`, `%` compiled to a `super::arith` call (package #5a, Task
/// A3). Every path that hands a
/// template to the parser goes through this one function, so a probe
/// ([`referenced_roots`], [`name_in_code`]) sees what a render runs.
fn compile_source(protected: &str) -> Result<String, RewriteError> {
    let compared = super::compare::compile_comparisons(&rewrite(protected)?)?;
    super::arith::compile_arithmetic(&compared)
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
    match render_str_guarded(template, cx, &[])? {
        GuardedRender::Rendered(text) => Ok(text),
        // Unreachable with an empty forbidden list, and stated rather than
        // `unwrap`ped so a future caller that passes a non-empty one here gets
        // a message instead of a panic.
        GuardedRender::Forbidden(root) => Err(RenderError::Template(format!(
            "internal: render_str refused the root `{root}` with no forbidden list"
        ))),
    }
}

/// Kestra's `renderTyped` (`Set.java:111`, `VariableRenderer.renderTyped`):
/// a `template` that is exactly ONE `{{ … }}` with no other text, not even
/// surrounding spaces (w9c `typed_render.tr_ws`: ` {{ 1 }} ` is the string
/// `" 1 "`), keeps the type its expression evaluates to, as KV data.
/// Anything else is [`render_str`]'s `String` (`x {{ 1 }}`, `{{ 1 }}{{ 2 }}`,
/// a literal `42`). Against w9c `typed_render` (W9c Task 5). An expression
/// the wrapper would change (brackets that do not pair up, `{{ 1) ~ (2 }}`;
/// a top-level comma) is never wrapped: it fails, as Pebble fails it
/// (`rewrite::parenthesizes_safely`; Task 5 review M3).
///
/// # How the type is kept (W9c final review F1)
///
/// The expression is rendered as `{{ (…) | <TYPED_JSON> }}`, a filter
/// registered for this render only. It writes exactly `toJson`'s text, and
/// it also converts the value it was given straight to [`KvData`]
/// ([`typed_data`]), so an exact number keeps its kind and digits: a
/// `BigDecimal` (a decimal `kv()` read, `number(type='BIGDECIMAL')`, a
/// `fromIon` decimal, the result of arithmetic on any of them) is a
/// `KvNumber::Decimal`, a `BigInteger` a `KvNumber::Int`. Through `toJson`'s
/// text alone, `serde_json` and `KvData::from_json`, the decimal
/// `123456789012345678901234567890.5` became the float
/// `1.2345678901234568e29`.
///
/// Every other value converts as that JSON round trip would: an integer is an
/// int at any size, a float a float, a null map entry dropped, a map in its
/// own order (`serde_json`'s `preserve_order`). A flow's own literal keeps
/// `serde_json`'s semantics: `{{ 1.5 }}` is a minijinja float, so a float.
///
/// The render is still the fixpoint [`render_str`] runs. When a later pass
/// changed the filter's text (a string in the value held a template, which
/// the next pass evaluated), the converted value is not what was rendered,
/// and the final text is read back as JSON instead, as before F1: an exact
/// decimal in such a value becomes a float (spec §15).
///
/// The outer `Err` is the render's; the inner one is the value's
/// ([`Unstorable`]: a number over `MAX_NUMBER_DIGITS`, nesting past
/// `storage::ion::MAX_DEPTH`), kept typed so a caller maps the class.
pub fn render_typed(
    template: &str,
    cx: &RenderContext,
) -> Result<Result<KvData, Unstorable>, RenderError> {
    let Some(inner) = super::rewrite::sole_expression(template) else {
        return render_str(template, cx).map(|text| Ok(KvData::String(text)));
    };
    if !super::rewrite::parenthesizes_safely(inner) {
        // Wrapped, the expression would mean something else (`{{ 1) ~ (2 }}`
        // would become `(1) ~ (2)`): the template as written is rendered,
        // so its syntax error is the renderer's own words. Should it render,
        // it is still refused: its type cannot be kept without the wrapper.
        render_str(template, cx)?;
        return Err(RenderError::Template(format!(
            "the expression {template:?} cannot be typed: its brackets do not pair up, or a \
             comma stands outside them"
        )));
    }
    // The FIRST call's text and value. A later pass that calls the filter
    // again is evaluating text an earlier pass produced, never the
    // expression, so it is not recorded.
    type Slot = Arc<std::sync::Mutex<Option<(String, Result<KvData, Unstorable>)>>>;
    let slot: Slot = Arc::default();
    let filled = Arc::clone(&slot);
    let extend = move |env: &mut Environment<'static>| {
        let filled = Arc::clone(&filled);
        env.add_filter(
            TYPED_JSON,
            move |value: MjValue| -> Result<MjValue, MjError> {
                let text = filters::to_json(value.clone())?;
                let mut slot = filled.lock().unwrap_or_else(|e| e.into_inner());
                if slot.is_none() {
                    let written = text.as_str().unwrap_or_default().to_string();
                    *slot = Some((written, typed_data(&value, 0)));
                }
                Ok(text)
            },
        );
    };
    let source = format!("{{{{ ({inner}) | {TYPED_JSON} }}}}");
    let text = match render_guarded_with(&source, cx, &[], &extend, Passes::Fixpoint)? {
        GuardedRender::Rendered(text) => text,
        GuardedRender::Forbidden(root) => {
            return Err(RenderError::Template(format!(
                "internal: render_typed refused the root `{root}` with no forbidden list"
            )))
        }
    };
    let first = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some((written, data)) = first {
        if written == text {
            return Ok(data);
        }
    }
    let json: JsonValue = serde_json::from_str(&text).map_err(|e| {
        RenderError::Template(format!(
            "the expression {template:?} rendered a value egret cannot read back as JSON: {e}"
        ))
    })?;
    Ok(KvData::from_json(&json))
}

/// The filter [`render_typed`] wraps its expression in, registered only on
/// that render's environment. `kestra_` names are egret's own (a template
/// cannot assign one, spec §15).
const TYPED_JSON: &str = "kestra_typed_json";

/// W9c final review F1: `value` as KV data, exactly as `toJson`'s text
/// ([`write_json`], nulls omitted) read back by `serde_json` and
/// `KvData::from_json` would be, except that an exact number (a
/// `BigNumber`) keeps its kind and its digits instead of passing through a
/// JSON number: a `BigDecimal` is `KvNumber::Decimal` (its plain digits), a
/// `BigInteger` `KvNumber::Int`. `depth` is the containers above `value`;
/// a container past `storage::ion::MAX_DEPTH` is refused as
/// `KvData::from_json` refuses it, which also bounds this recursion.
pub(super) fn typed_data(value: &MjValue, depth: usize) -> Result<KvData, Unstorable> {
    // `serde_json`'s text of a number, by `KvData::from_json`'s rule.
    let number = |text: &str| KvNumber::of_json_text(text).map(KvData::Number);
    let container = |depth: usize| {
        if depth >= crate::storage::ion::MAX_DEPTH {
            Err(too_deep())
        } else {
            Ok(())
        }
    };
    Ok(match value.kind() {
        ValueKind::Undefined | ValueKind::None => KvData::Null,
        ValueKind::Bool => KvData::Bool(value.is_true()),
        ValueKind::Number => {
            if value.is_integer() {
                number(&value.to_string())?
            } else {
                match f64::try_from(value.clone()) {
                    Ok(f) if !f.is_finite() => {
                        let mut text = String::new();
                        write_java_double(&mut text, f);
                        KvData::String(text)
                    }
                    Ok(f) => {
                        let mut text = String::new();
                        write_java_double(&mut text, f);
                        number(&text)?
                    }
                    Err(_) => number(&value.to_string())?,
                }
            }
        }
        ValueKind::Seq | ValueKind::Iterable => {
            container(depth)?;
            let mut items = Vec::new();
            if let Ok(iter) = value.try_iter() {
                for item in iter {
                    items.push(typed_data(&item, depth + 1)?);
                }
            }
            KvData::List(items)
        }
        ValueKind::Map => {
            container(depth)?;
            let mut fields: Vec<(String, KvData)> = Vec::new();
            let mut at: std::collections::HashMap<String, usize> = Default::default();
            if let Ok(keys) = value.try_iter() {
                for key in keys {
                    let Ok(item) = value.get_item(&key) else {
                        continue;
                    };
                    if item.is_none() || item.is_undefined() {
                        continue;
                    }
                    let name = key.to_string();
                    let data = typed_data(&item, depth + 1)?;
                    // Two keys with one text: `serde_json`'s map keeps the
                    // first place and the last value.
                    match at.get(&name) {
                        Some(&i) => fields[i].1 = data,
                        None => {
                            at.insert(name.clone(), fields.len());
                            fields.push((name, data));
                        }
                    }
                }
            }
            KvData::Struct(fields)
        }
        _ if filters::big_number_plain(value).is_some() => {
            let plain = filters::big_number_plain(value).expect("checked");
            let exact = match filters::big_number_class(value) {
                Some("java.math.BigInteger") => KvNumber::int(plain),
                _ => KvNumber::decimal(plain),
            };
            match exact {
                Some(n) => {
                    KvNumber::check_exact_digits(plain)?;
                    KvData::Number(n)
                }
                // Not reachable for a `BigNumber` egret built; kept total:
                // the number `toJson` writes, as before F1.
                None => number(&filters::big_number_text(value).expect("checked"))?,
            }
        }
        _ => KvData::String(value.to_string()),
    })
}

/// ONE pass of the renderer, on a copy of the environment that `extend` has
/// changed: Kestra's `VariableRenderer.render` with `recursiveRendering =
/// false`, which is its default (`VariableConfiguration.java:16`), i.e.
/// `renderOnce` followed by `RAW_PATTERN.matcher(x).replaceAll("$2")`.
///
/// W5b Task 4, fix round 1 (user ruling 1A): `kestra.*` plugin-task
/// properties render through this and **not** through [`render_str`]'s
/// fixpoint loop. With the loop, text that a pass *produced* -- render-context
/// data (an input, a webhook body, a prior task's output) or a secret's own
/// value -- was evaluated as a template on the next pass, so an input whose
/// value was `{{ secret('db') }}` resolved a secret the author never named.
/// One pass evaluates only the author's source. The recursion of every other
/// render is unchanged.
///
/// `extend` exists because `secret()` is refused on the shared
/// [`environment`] and resolved only on plugin tasks, so the resolving
/// `secret` is registered **per render** over the refusal. It runs on the
/// render thread, after [`environment`] and before the render context's scope
/// global, so it cannot unbind that global. Everything else [`render_str`]
/// does is kept: the no-`{` short circuit, the render thread, the rewrite and
/// the raw-region protection.
pub(crate) fn render_once_with(
    template: &str,
    cx: &RenderContext,
    extend: &(dyn Fn(&mut Environment<'static>) + Sync),
) -> Result<String, RenderError> {
    match render_guarded_with(template, cx, &[], extend, Passes::One)? {
        GuardedRender::Rendered(text) => Ok(text),
        GuardedRender::Forbidden(root) => Err(RenderError::Template(format!(
            "internal: render_once_with refused the root `{root}` with no forbidden list"
        ))),
    }
}

/// How many passes a render makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Passes {
    /// [`render_once_with`]: `recursiveRendering = false`.
    One,
    /// Every other render: until the output stops changing.
    Fixpoint,
}

/// What [`render_str_guarded`] answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardedRender {
    /// The fixpoint, reached without any pass naming a forbidden root.
    Rendered(String),
    /// Some pass named this root. **No further pass ran**, and no output is
    /// returned.
    Forbidden(String),
}

/// [`render_str`], refusing as soon as **any pass** of the fixpoint loop names
/// one of `forbidden` as a top-level root.
///
/// # Why the check has to live inside this loop
///
/// W6 Task 7 fix round 3, Important 6. Rendering here is a fixpoint, not one
/// pass: `render_once` runs up to [`MAX_RENDERING_AMOUNT`] times and **a pass
/// can emit a template the next pass evaluates**. A caller that asked
/// [`referenced_roots`] about the *source* therefore asked about pass 1 only,
/// and a root reached by indirection was invisible to it:
///
/// ```text
/// variables:
///   tpl: "{{ [trigger] }}"
/// triggers:
///   - inputs:
///       a: "{{ vars.tpl }}"
/// ```
///
/// Pass 1 names `vars`. Pass 2 names `trigger`. Indirection through `vars` is
/// ordinary authoring, not an exotic attack, and the webhook route accepted
/// that input and stored `"[null]"`.
///
/// So the question is asked of **`current` before every pass**, which is the
/// only place that sees every text the engine will evaluate. `forbidden` is
/// empty for [`render_str`], and `["trigger"]` for a webhook's `inputs:`.
///
/// **It is not a security boundary.** Nothing here binds the forbidden name;
/// this decides *acceptance*. See `trigger::webhook::InputContext`.
pub fn render_str_guarded(
    template: &str,
    cx: &RenderContext,
    forbidden: &[&str],
) -> Result<GuardedRender, RenderError> {
    render_guarded_with(template, cx, forbidden, &|_| {}, Passes::Fixpoint)
}

/// [`render_str_guarded`], with [`render_str_with`]'s `extend`.
fn render_guarded_with(
    template: &str,
    cx: &RenderContext,
    forbidden: &[&str],
    extend: &(dyn Fn(&mut Environment<'static>) + Sync),
    passes: Passes,
) -> Result<GuardedRender, RenderError> {
    // `VariableRenderer.java:76-79`: "it's not a Pebble template so we
    // short-circuit rendering". Note the test is `{`, not `{{` -- a source
    // containing a lone `{` still goes through the engine. Done before the
    // thread, because a source with no brace does no work at all.
    //
    // A source with no `{` cannot name a root either, so the short circuit is
    // safe for the guard as well -- asserted by
    // `tests::the_brace_short_circuit_cannot_hide_a_forbidden_root`.
    if !template.contains('{') {
        return Ok(GuardedRender::Rendered(template.to_string()));
    }

    // **On a thread whose stack egret chooses.** See [`RENDER_STACK_BYTES`]
    // for why this is here rather than a fifth per-construct bound.
    // `RenderContext` is `Send + Sync` (minijinja's `Value` is), so this is a
    // scoped borrow and not a copy -- `render_context_crosses_a_thread`
    // pins that, because the day it stops being true this has to become a
    // serialise-and-rebuild and nothing else would say so.
    off_the_runtime(|| {
        std::thread::scope(|scope| {
            let worker = std::thread::Builder::new()
                .name("egret-render".to_string())
                .stack_size(RENDER_STACK_BYTES)
                .spawn_scoped(scope, || {
                    // W9b fix round 1 (I1): nothing the render opened outlives
                    // it -- a drop guard, so a panic below cannot skip it.
                    let _end = EndRender(cx);
                    render_on_this_thread(template, cx, forbidden, extend, passes)
                })
                .map_err(|e| {
                    RenderError::Template(format!("could not start a render thread: {e}"))
                })?;
            match worker.join() {
                Ok(result) => result,
                // A panic below is still a panic -- it is resumed on this thread
                // when the scope ends, so `tests/oracle.rs`'s guard still sees it.
                Err(_) => Err(RenderError::Template("the render panicked".to_string())),
            }
        })
    })
}

/// Ends the render's namespace connection when dropped, so a panic in the
/// render cannot skip it (W9b Task 4 re-review, R1-m1).
struct EndRender<'a>(&'a RenderContext);

impl Drop for EndRender<'_> {
    fn drop(&mut self) {
        self.0.end_render();
    }
}

/// Run `f` on a thread of its own and wait for it, the way a render runs --
/// for a caller that reads a file through a `FileScope` outside a template.
///
/// W9a Task 11: `worker::render` reads a `Loop`'s `values:` file in `settle`,
/// on a tokio worker, and `Snapshot::read_bytes` refuses a runtime thread
/// (`block_on` would panic there). This is the render's own arrangement --
/// [`off_the_runtime`] around a scoped thread -- so the runtime keeps running
/// while the read blocks. `Err` when the thread cannot start or panics.
///
/// **W9b Task 4 re-review, R1-m2.** It takes the `FileScope` the work reads
/// through and ends that scope's render when `f` returns (or panics), as a
/// template render does: a namespace connection a non-template read opens
/// (Task 5's staging, Task 9's bridge) cannot outlive its work.
pub(crate) fn on_a_render_thread<T: Send>(
    files: &FileScope,
    f: impl FnOnce() -> T + Send,
) -> Result<T, String> {
    struct EndScope<'a>(&'a FileScope);
    impl Drop for EndScope<'_> {
        fn drop(&mut self) {
            self.0 .0.end_render();
        }
    }
    off_the_runtime(|| {
        std::thread::scope(|scope| {
            let worker = std::thread::Builder::new()
                .name("egret-render".to_string())
                .stack_size(RENDER_STACK_BYTES)
                .spawn_scoped(scope, || {
                    let _end = EndScope(files);
                    f()
                })
                .map_err(|e| format!("could not start a render thread: {e}"))?;
            worker
                .join()
                .map_err(|_| "the render thread panicked".to_string())
        })
    })
}

/// Run `join`-on-a-render-thread `f` so the runtime keeps running under it.
///
/// **W9a Task 4 fix round 1, C1.** A render can block its own thread on the
/// runtime (`read()`, `storage::scope::Snapshot::read_utf8`), and the thread
/// that spawned the render waits in `join`. On a tokio WORKER that join used
/// to park the worker with its core, so with every worker parked nothing
/// drove the IO and timer drivers and the runtime wedged for good -- measured
/// with one worker and one render, and with 2/2 and 4/4.
/// `block_in_place` hands this worker's core to another thread first.
///
/// Only on the multi-thread runtime: `block_in_place` panics on a
/// current-thread one, which has no other thread to hand anything to --
/// `read()` refuses there instead (`Snapshot::read_utf8`). Off any runtime
/// this is just `f()`.
fn off_the_runtime<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

/// The top-level names a template refers to, as the PARSER sees them.
///
/// W6 Task 7 fix round 1, Critical 2. The webhook route has to tell "this
/// input needs a render root egret does not publish" apart from "this input is
/// broken", and the first version of that decision was a substring scan for
/// `"trigger."` — which `{{ trigger }}` walks straight past. A substring scan
/// is a lexical test applied to an expression language and there are
/// arbitrarily many ways to name a binding.
///
/// This asks the same parser that renders. It runs the identical front half of
/// [`render_once`] — protect the `{% raw %}` regions, then [`rewrite`] — so a
/// template whose roots this reports is a template the renderer would see the
/// same way, and then returns minijinja's `undeclared_variables(false)`: the
/// **top-level** names, so `trigger.body`, `trigger['body']`,
/// `trigger | json` and `{% set t = trigger %}` all report `trigger`, while
/// the literal word in a text run reports nothing.
///
/// `nested = false` on purpose. `true` would also report `body` for
/// `trigger.body`, which is noise for the one question this is asked.
///
/// It is **not** the security boundary — that is the render context simply not
/// binding the name (see `trigger::webhook::InputContext`). This only decides
/// which message a refusal carries.
pub fn referenced_roots(template: &str) -> Result<std::collections::BTreeSet<String>, RenderError> {
    let (protected, _replacers) = replace_raw_tags(template);
    let rewritten = compile_source(&protected)?;
    let env = environment();
    let tmpl = env
        .template_from_str(&rewritten)
        .map_err(|e| RenderError::Template(e.to_string()))?;
    Ok(tmpl.undeclared_variables(false).into_iter().collect())
}

/// Whether `name` occurs as CODE in `template` -- anywhere minijinja's lexer
/// reads it as an identifier, not as template data, a string literal, a
/// comment or a `{% raw %}` body -- after the same raw-tag protection and
/// `rewrite` a render applies (W5b fix round 3).
///
/// **A probe, not a lexer of our own:** each occurrence of `name` that is not
/// an attribute (`x.name`) or part of a longer identifier is followed, one at
/// a time, by `@`, a character minijinja's lexer refuses in code and accepts
/// everywhere else. If that copy no longer parses, the occurrence was code.
/// Unlike [`referenced_roots`] this sees assignment targets
/// (`{% set name = … %}`, `with`, `for`, tuple unpacking) and `{% filter %}`
/// arguments, which `undeclared_variables` does not report. A template that
/// does not rewrite or parse at all answers `true`.
// `let_and_return`: see the binding's comment.
#[allow(clippy::let_and_return)]
pub(crate) fn name_in_code(template: &str, name: &str) -> bool {
    let (protected, _replacers) = replace_raw_tags(template);
    let Ok(rewritten) = compile_source(&protected) else {
        return true;
    };
    let env = environment();
    if env.template_from_str(&rewritten).is_err() {
        return true;
    }
    let Ok(pattern) = regex::Regex::new(&format!(r"(?:^|[^.\w]){}\b", regex::escape(name))) else {
        return true;
    };
    let ends: Vec<usize> = pattern.find_iter(&rewritten).map(|m| m.end()).collect();
    ends.into_iter().any(|end| {
        let probe = format!("{}@{}", &rewritten[..end], &rewritten[end..]);
        // Bound first: the template borrows `probe` and `env`, and must be
        // dropped before either.
        let refused = env.template_from_str(&probe).is_err();
        refused
    })
}

/// The render loop, on the thread [`render_str`] gave it.
fn render_on_this_thread(
    template: &str,
    cx: &RenderContext,
    forbidden: &[&str],
    extend: &(dyn Fn(&mut Environment<'static>) + Sync),
    passes: Passes,
) -> Result<GuardedRender, RenderError> {
    let mut env = environment();
    // Before the scope global, so an `extend` cannot unbind it.
    extend(&mut env);
    let variables = cx.to_minijinja();

    // The render context's own top-level names, for
    // `super::functions::fetch_context`. See [`CONTEXT_SCOPE_NAMES`] for why
    // this is a global rather than something read off `State`.
    env.add_global(
        CONTEXT_SCOPE_NAMES,
        MjValue::from_object(ContextScope(
            cx.bindings().iter().map(|(name, _)| name.clone()).collect(),
        )),
    );
    // W9a Task 4: after `extend`, for the same reason as the scope above.
    cx.bind_files(&mut env);
    let env = env;

    // W2 Task 8 fix round 1: one budget for the whole render, shared by every
    // `replace` call in it. See [`super::filters::MAX_REPLACE_SCAN_BYTES`].
    let _budget = super::filters::ReplaceBudget::install();

    if passes == Passes::One {
        // Nothing is forbidden on this path (`render_once_with` passes
        // `&[]`), so there is no guard to run.
        let once = render_once(&env, template, &variables)?;
        return Ok(GuardedRender::Rendered(strip_raw_markers(&once)));
    }

    let mut current = template.to_string();
    // Inclusive: see [`MAX_RENDERING_AMOUNT`]'s note on Kestra's off-by-one.
    for _ in 0..=MAX_RENDERING_AMOUNT {
        // **Before the pass, not after it.** See [`render_str_guarded`]: a
        // previous pass may have emitted the text this one is about to
        // evaluate, and that text is the only place a root reached by
        // indirection is visible.
        //
        // A `current` that will not parse names nothing; the pass below then
        // fails with the parser's own message, which is the right error for a
        // broken template.
        if !forbidden.is_empty() {
            if let Ok(roots) = referenced_roots(&current) {
                if let Some(hit) = forbidden.iter().find(|f| roots.contains(**f)) {
                    return Ok(GuardedRender::Forbidden((*hit).to_string()));
                }
            }
        }
        let next = render_once(&env, &current, &variables)?;
        if next == current {
            return Ok(GuardedRender::Rendered(strip_raw_markers(&next)));
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
    let rewritten = compile_source(&protected)?;
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

// ---------------------------------------------------------------------------
// W3b Task 6: the same source, with values BOUND instead of interpolated.
// ---------------------------------------------------------------------------

/// The bind marker written in place of every emitted value. Both engines egret
/// speaks to — Trino and StarRocks — spell it `?`, which is why
/// `egret_query_api::Param` has no marker of its own.
pub(crate) const BIND_PLACEHOLDER: &str = "?";

/// The most values one [`render_bound`] will bind.
///
/// Not a performance guard: a `{% for %}` over a task output egret did not
/// write is author-reachable, and a statement with a million markers is a way
/// to spend an engine's parser rather than a query anyone meant. Kestra has no
/// counterpart, exactly as [`MAX_OUTPUT_BYTES`] has none — see that constant.
pub(crate) const MAX_BOUND_VALUES: usize = 10_000;

/// What one `{{ … }}` bound to, as a JSON scalar.
///
/// **Deliberately not `egret_query_api::Param`.** `runners` is the expression
/// layer and touches no I/O (`crate` doc, and the programme spec's crate
/// table); the mapping from a scalar to an engine's bind type belongs to the
/// caller that owns the engine, which is `worker::handlers::sql`. Under this
/// crate's `serde_json` resolution `arbitrary_precision` is **on** (see
/// `Cargo.toml`), so a number crosses this boundary as its exact text and a
/// decimal literal cannot become a lossy `f64` on the way.
pub(crate) type Bound = JsonValue;

/// Renders `template` against `cx` with every emitted value **bound** rather
/// than written, returning the text and the values in the order their markers
/// appear.
///
/// # Why this exists rather than `render_str` plus quoting
///
/// This is `egret-dataapi`'s [`bind_formatter`] idiom
/// (`crates/egret-dataapi/src/render.rs`), applied to *this* crate's engine.
/// minijinja's VM writes to the output in exactly two places: `EmitRaw`, whose
/// argument is a borrowed slice of the **template source**, and `Emit`, which
/// goes through the formatter. Replacing the formatter therefore removes the
/// path from `{{ … }}` to SQL text altogether — an author cannot write an
/// interpolating template even deliberately, because the interpolating code
/// path no longer exists. That is a structural property rather than an
/// escaping convention, and it is the reason a workflow's `sql:` is not the
/// third member of the `quote_string` family.
///
/// One trap in that mechanism, and the reason the `set_formatter` call below is
/// unconditional: `Emit` has a fast path that calls the default escape function
/// directly and skips the formatter, guarded by a flag that only clears when
/// `set_formatter` is called. [`environment`] already calls it (for
/// `json_writer_formatter`), so the flag is clear before this function runs and
/// clear after — but a future `environment` that stopped setting one must not
/// silently turn this into a bypass. **This sentence said that is "what
/// `a_bound_render_never_writes_a_value_into_the_text` is written to catch",
/// and no such test exists** — the identifier occurs once in the tree, here.
/// Found by W8 Task 7's final round, which swept every test name cited in a doc
/// comment across the crate after two dangling citations turned up in one
/// package. The guard is still owed; what it would have to assert is that a
/// bound render with `set_formatter` never called writes no value into the
/// text.
///
/// # One pass, where [`render_str`] loops
///
/// `VariableRenderer` re-renders its own output until it stops changing
/// (`MAX_RENDERING_AMOUNT`), and that loop is exactly the double-evaluation
/// this function exists to remove: a value that rendered to `{{ … }}` would be
/// evaluated as source on the next pass. Binding already stops it — a bound
/// value never reaches the text, so it can never be re-read as a template — and
/// looping on top of that would be either a no-op or a second chance to emit,
/// with the binds of the first pass already recorded. So this renders **once**,
/// and a `sql:` therefore has one fewer evaluation than a `format:` on the same
/// flow. That difference is deliberate and is stated on
/// [`crate::worker::handlers::sql`].
///
/// Everything else [`render_str`] does is kept: the `{% raw %}` protection, the
/// [`rewrite`] of Kestra's operator syntax, the [`MAX_OUTPUT_BYTES`] cap, the
/// [`ReplaceBudget`](super::filters::ReplaceBudget), and the
/// [`RENDER_STACK_BYTES`] thread. Composing `environment` and a render by hand
/// loses all five — see [`environment`]'s own table, and the corpus expression
/// that overflowed a 2 MiB tokio stack.
///
/// # What is refused
///
/// A sequence or a map cannot be one bound value: `IN (?)` takes exactly one,
/// and an author who wrote `{{ inputs.regions }}` meant a list. Those are a
/// [`RenderError::Template`] naming the `{% for %}` spelling that works, which
/// is the same answer `egret-dataapi` gives for the same reason.
pub(crate) fn render_bound(
    template: &str,
    cx: &RenderContext,
) -> Result<(String, Vec<Bound>), RenderError> {
    // NO `!template.contains('{')` short-circuit, unlike [`render_str`]. It
    // would be correct (a source with no brace binds nothing) and it is left
    // out anyway: the value of this function is that ONE code path produces
    // both halves of the answer, and a second path that produces `(text, [])`
    // is a second place for the two to disagree about what "no values" means.
    off_the_runtime(|| {
        std::thread::scope(|scope| {
            let worker = std::thread::Builder::new()
                .name("egret-render-bound".to_string())
                .stack_size(RENDER_STACK_BYTES)
                .spawn_scoped(scope, || {
                    // W9b fix round 1 (I1): as `render_guarded_with`.
                    let _end = EndRender(cx);
                    bind_on_this_thread(template, cx)
                })
                .map_err(|e| {
                    RenderError::Template(format!("could not start a render thread: {e}"))
                })?;
            match worker.join() {
                Ok(result) => result,
                Err(_) => Err(RenderError::Template("the render panicked".to_string())),
            }
        })
    })
}

/// [`render_bound`]'s body, on the thread it was given.
fn bind_on_this_thread(
    template: &str,
    cx: &RenderContext,
) -> Result<(String, Vec<Bound>), RenderError> {
    let binds: Arc<std::sync::Mutex<Vec<Bound>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut env = environment();
    env.add_global(
        CONTEXT_SCOPE_NAMES,
        MjValue::from_object(ContextScope(
            cx.bindings().iter().map(|(name, _)| name.clone()).collect(),
        )),
    );
    // W9a Task 4: `egret.sql`'s `sql:` reads files too -- this thread is the
    // second of the two, and a scope bound on only one of them is the defect
    // `the_file_functions_render_inside_an_egret_sql_statement` exists for.
    cx.bind_files(&mut env);
    // AFTER `environment()`, which sets `json_writer_formatter`. Order is the
    // whole of it: the other way round, Kestra's formatter would win and every
    // value would be written into the SQL.
    let sink = Arc::clone(&binds);
    env.set_formatter(move |out, _state, value| {
        let mut held = sink
            .lock()
            .expect("bind list mutex poisoned by a panic inside the renderer");
        if held.len() >= MAX_BOUND_VALUES {
            return Err(MjError::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("this statement binds more than {MAX_BOUND_VALUES} values"),
            ));
        }
        held.push(to_bound(value)?);
        drop(held);
        out.write_str(BIND_PLACEHOLDER).map_err(MjError::from)
    });
    let env = env;

    let _budget = super::filters::ReplaceBudget::install();
    let variables = cx.to_minijinja();
    let rendered = render_once(&env, template, &variables)?;

    // Taken out of the still-shared list rather than unwrapped out of the `Arc`:
    // the formatter closure holds the other handle and lives as long as `env`,
    // which is still borrowed by the frame above. An `Arc::try_unwrap` here
    // fails at run time for that reason — and it did.
    let values = std::mem::take(
        &mut *binds
            .lock()
            .expect("bind list mutex poisoned by a panic inside the renderer"),
    );
    Ok((strip_raw_markers(&rendered), values))
}

/// The value being emitted, as a JSON scalar an engine can bind.
///
/// Mirrors `egret-dataapi`'s `to_param` and diverges from it in one place, for
/// a reason worth stating: that function has a `Cell` arm, because the Data API
/// wraps every caller-supplied parameter in an **opaque object** to carry a
/// `Date` through the template as a `Date`. The cost of the wrapper is that
/// `{% if mode == 'fast' %}` is silently false for ever, which is why that
/// module needs `check_static`/`check_dynamic` and why five reviews found five
/// bypasses of the byte scanner.
///
/// egret's workflow render context holds ordinary minijinja values — the
/// formatter is the only thing this function changes — so **no comparison is
/// silently false here and there is no lint to bypass.** A workflow's `sql:`
/// gets W2's whole expression language, tests and comparisons included.
fn to_bound(value: &MjValue) -> Result<Bound, MjError> {
    Ok(match value.kind() {
        ValueKind::Undefined | ValueKind::None => JsonValue::Null,
        ValueKind::Bool => JsonValue::Bool(value.is_true()),
        ValueKind::Number => match i64::try_from(value.clone()) {
            Ok(i) => JsonValue::from(i),
            // Exact text rather than a lossy float. `arbitrary_precision` is on
            // in every resolution this crate ships in (`Cargo.toml`), so
            // `from_str` keeps the digits the author wrote.
            Err(_) => serde_json::from_str::<JsonValue>(&value.to_string())
                .unwrap_or_else(|_| JsonValue::String(value.to_string())),
        },
        ValueKind::String => JsonValue::String(value.to_string()),
        _ => {
            return Err(MjError::new(
                minijinja::ErrorKind::InvalidOperation,
                "this value cannot be bound as a single parameter; emit a sequence one \
                 element at a time inside a `{% for %}` loop",
            ))
        }
    })
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

    /// R1-m1 and R1-m2: the render's end is unconditional. A scope whose
    /// namespace lookup panics still has `end_render` called, on a template
    /// render and on `on_a_render_thread`.
    mod the_render_ends_even_when_it_panics {
        use super::*;
        use crate::runners::files::{
            FileMeta, FileScope, FileUnavailable, NsFileArgs, NsFileMeta, RenderFiles,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        #[derive(Default)]
        struct Panicky(AtomicUsize);

        impl RenderFiles for Panicky {
            fn lookup(&self, _: &str) -> Result<FileMeta, FileUnavailable> {
                Err(FileUnavailable::Refused("none".into()))
            }
            fn read_utf8(&self, _: &str) -> Result<String, String> {
                Err("none".into())
            }
            fn read_bytes(&self, _: &str, _: &str) -> Result<Vec<u8>, String> {
                Err("none".into())
            }
            fn task_prefix(&self) -> Option<&str> {
                None
            }
            fn namespace_file(&self, _: &NsFileArgs<'_>) -> Result<NsFileMeta, FileUnavailable> {
                panic!("a lookup that panics");
            }
            fn end_render(&self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        fn scope() -> (Arc<Panicky>, FileScope) {
            let fake = Arc::new(Panicky::default());
            (fake.clone(), FileScope(fake))
        }

        #[test]
        fn a_template_render_that_panics_still_ends() {
            let (fake, files) = scope();
            let cx = RenderContext::new().with_files(files);
            let out = render_str("{{ fileExists('a.txt') }}", &cx);
            assert!(
                matches!(&out, Err(RenderError::Template(m)) if m == "the render panicked"),
                "the lookup's panic is an error, not a hang: {out:?}"
            );
            assert_eq!(
                fake.0.load(Ordering::SeqCst),
                1,
                "render_guarded_with ended it"
            );
        }

        #[test]
        fn a_bound_render_that_panics_still_ends() {
            let (fake, files) = scope();
            let cx = RenderContext::new().with_files(files);
            let out = render_bound("{{ fileExists('a.txt') }}", &cx);
            assert!(
                matches!(&out, Err(RenderError::Template(m)) if m == "the render panicked"),
                "the lookup's panic is an error, not a hang: {out:?}"
            );
            assert_eq!(fake.0.load(Ordering::SeqCst), 1, "render_bound ended it");
        }

        #[test]
        fn on_a_render_thread_ends_the_scope_after_the_work_and_after_a_panic() {
            let (fake, files) = scope();
            assert_eq!(on_a_render_thread(&files, || 7), Ok(7));
            assert_eq!(fake.0.load(Ordering::SeqCst), 1, "ended after the work");
            let out = on_a_render_thread(&files, || -> u8 { panic!("the work panics") });
            assert!(out.is_err());
            assert_eq!(fake.0.load(Ordering::SeqCst), 2, "ended after a panic");
        }
    }

    // -----------------------------------------------------------------
    // W2 Task 8: the name surface
    // -----------------------------------------------------------------

    /// `oracle/name-surface.txt`, asserted **bidirectionally by probing** —
    /// not by reading a registry, which minijinja does not expose for filters
    /// and tests anyway.
    ///
    /// The one thing this closes: a name minijinja binds that Kestra has no
    /// counterpart for **resolves and answers** here, so a flow that renders
    /// in egret fails on Kestra — later, in production, rather than at
    /// authoring time. Nothing else in this crate can see that: the oracle
    /// only renders expressions somebody thought to write down, and nobody
    /// writes `{{ x | groupby(…) }}` into a corpus for an engine that has
    /// never had the filter.
    ///
    /// Verified by deliberate breakage before committing, five ways, each
    /// failing by name: deleting a name from `MINIJINJA_ONLY_FILTERS` (egret
    /// binds what the file says it does not), adding one (egret refuses what
    /// the file says it binds), flipping an `engine` column without a reason,
    /// adding a reason to an agreeing row, and deleting a row.
    /// `defined` is Pebble's — "present **and not null**".
    ///
    /// Pairs `null-literal-is-defined` and `null-from-the-context-is-defined`,
    /// both banked by the override. Every expected value below was measured on
    /// `io.kestra:core:2.0.0-rc13`, one probe per kind.
    ///
    /// # The second half: which tests egret binds itself
    ///
    /// This test used to be the fence for "`defined` is the only test this
    /// crate overrides": `even`, `iterable` and `odd` answered minijinja's way
    /// and `empty`, `json`, `map` and `null` were unknown. Package #5b's Task
    /// B2 (spec D8, 2026-10-29) ported the `is` surface, so the fence now lists
    /// what egret overrides or adds — all eight engine tests answer the
    /// engine's way, three of them where minijinja's default used to — and
    /// asserts that the null literal's other spellings did NOT become test
    /// names along the way: Kestra refuses `is none`, `is NONE` and `is NULL`
    /// (`Test [none] does not exist.`, measured), and so does egret.
    #[test]
    fn the_engine_tests_are_egrets_and_none_is_not_one() {
        // "Present and not null." A falsy-but-present value is DEFINED, which
        // is the half a `!value.is_true()`-shaped mistake would get wrong.
        for (expr, expected) in [
            ("{{ s is defined }}", "true"),
            ("{{ zero is defined }}", "true"),
            ("{{ f is defined }}", "true"),
            ("{{ empty is defined }}", "true"),
            ("{{ lst is defined }}", "true"),
            ("{{ m is defined }}", "true"),
            ("{{ obj.a is defined }}", "true"),
            // The four Pebble calls `false`, and the two on the left are the
            // ones W2b broke and this override fixes.
            ("{{ null is defined }}", "false"),
            ("{{ n is defined }}", "false"),
            ("{{ nothere is defined }}", "false"),
            ("{{ obj.missing is defined }}", "false"),
            // `is not` negates it cleanly, and `{% if %}` sees a real boolean
            // rather than the null that `{% if null %}` chokes on.
            ("{{ null is not defined }}", "true"),
            ("{{ s is not defined }}", "false"),
            ("{% if null is defined %}y{% else %}n{% endif %}", "n"),
            ("{% if s is defined %}y{% else %}n{% endif %}", "y"),
        ] {
            let ctx = json!({
                "s": "x", "zero": 0, "f": false, "empty": "", "lst": [],
                "m": {}, "n": null, "obj": {"a": 1}
            });
            assert_eq!(
                render(expr, ctx).unwrap_or_else(|e| panic!("{expr}: {e}")),
                expected,
                "{expr}"
            );
        }

        // **The overrides, asserted.** The three names minijinja binds by
        // default answer Pebble's way now (each row is one minijinja
        // answered differently), and the four it never had are bound.
        let ctx = json!({"s": "x", "two": 2, "lst": [1], "m": {}, "n": null});
        assert_eq!(render("{{ two is even }}", ctx.clone()).unwrap(), "true");
        assert_eq!(render("{{ two is odd }}", ctx.clone()).unwrap(), "false");
        assert!(
            render("{{ s is even }}", ctx.clone()).is_err(),
            "minijinja's `even` answered false"
        );
        assert!(
            render("{{ s is odd }}", ctx.clone()).is_err(),
            "minijinja's `odd` answered false"
        );
        assert_eq!(render("{{ s is iterable }}", ctx.clone()).unwrap(), "false");
        assert_eq!(render("{{ m is iterable }}", ctx.clone()).unwrap(), "false");
        assert_eq!(
            render("{{ lst is iterable }}", ctx.clone()).unwrap(),
            "true"
        );
        for (expr, expected) in [
            ("{{ n is null }}", "true"),
            ("{{ m is empty }}", "true"),
            ("{{ m is map }}", "true"),
            ("{{ s is json }}", "false"),
        ] {
            assert_eq!(render(expr, ctx.clone()).unwrap(), expected, "{expr}");
        }

        // **The fence.** The null literal's other spellings are not test
        // names, in Kestra or here.
        for spelling in ["none", "NONE", "NULL"] {
            for expr in [
                format!("{{{{ n is {spelling} }}}}"),
                format!("{{{{ n is not {spelling} }}}}"),
            ] {
                let message = render(&expr, ctx.clone())
                    .expect_err(&format!("{expr} must be an unknown test"))
                    .to_string();
                assert!(
                    message.contains("unknown test"),
                    "{expr} must be UNKNOWN, not answered: {message}"
                );
            }
        }
    }

    /// The `{`-less short circuit returns before the guard loop, so it must be
    /// incapable of hiding a forbidden root.
    ///
    /// It is: a source with no `{` cannot name a variable at all. Asserted
    /// rather than reasoned, because the short circuit is a return statement
    /// above every check in the function.
    #[test]
    fn the_brace_short_circuit_cannot_hide_a_forbidden_root() {
        let cx = RenderContext::new();
        assert_eq!(
            render_str_guarded("trigger is a word here", &cx, &["trigger"]).unwrap(),
            GuardedRender::Rendered("trigger is a word here".to_string())
        );
        assert!(referenced_roots("trigger is a word here")
            .unwrap()
            .is_empty());
        // The control: one brace and it goes through the engine, where the
        // guard sees it.
        assert_eq!(
            render_str_guarded("{{ trigger }}", &cx, &["trigger"]).unwrap(),
            GuardedRender::Forbidden("trigger".to_string())
        );
    }

    /// The guard fires on a pass the SOURCE does not name — the whole of fix
    /// round 3's Important 6, at the layer that implements it.
    #[test]
    fn the_guard_sees_every_pass_not_only_the_first() {
        let mut cx = RenderContext::new();
        cx.insert(
            "vars",
            json!({"tpl": "{{ [trigger] }}", "ok": "{{ 1 + 1 }}"}),
        );

        let pass1 = referenced_roots("{{ vars.tpl }}").unwrap();
        assert!(
            pass1.contains("vars") && !pass1.contains("trigger"),
            "pass 1 names `vars` and nothing else: {pass1:?}"
        );

        assert_eq!(
            render_str_guarded("{{ vars.tpl }}", &cx, &["trigger"]).unwrap(),
            GuardedRender::Forbidden("trigger".to_string()),
            "pass 2 names it, and the guard is inside the loop"
        );
        // The control: the same indirection without the root renders to its
        // fixpoint, so the assertion above is not satisfied by a guard that
        // refuses every indirection.
        assert_eq!(
            render_str_guarded("{{ vars.ok }}", &cx, &["trigger"]).unwrap(),
            GuardedRender::Rendered("2".to_string())
        );
    }

    #[test]
    fn the_built_in_surface_matches_the_engines() {
        const FIXTURE: &str = include_str!("../../oracle/name-surface.txt");
        const BASELINE: &str = include_str!("../../oracle/baseline.txt");
        const GOLDEN: &str = include_str!("../../oracle/golden.json");

        let baselined_ids: std::collections::BTreeSet<&str> = BASELINE
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        // The ids, read out of the committed golden file's own `"id"` fields
        // rather than parsed -- this is a name check, not a schema check, and
        // `tests/oracle.rs` is what binds the two files together.
        let golden_ids: std::collections::BTreeSet<&str> = GOLDEN
            .match_indices("\"id\" : \"")
            .filter_map(|(at, m)| {
                let rest = &GOLDEN[at + m.len()..];
                rest.find('"').map(|end| &rest[..end])
            })
            .collect();
        assert!(golden_ids.len() > 1000, "the golden file did not parse");

        let mut counts = std::collections::BTreeMap::new();
        let mut rows = 0usize;
        for line in FIXTURE.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            assert_eq!(
                fields.len(),
                6,
                "every row is `kind name engine egret semantics reason`, tab-separated: {line:?}"
            );
            let (kind, name, engine, egret, semantics, reason) = (
                fields[0], fields[1], fields[2], fields[3], fields[4], fields[5],
            );
            rows += 1;
            if engine == "yes" {
                *counts.entry(kind).or_insert(0usize) += 1;
            }

            // A divergence must be explained, and so must a `no`/`no` row --
            // that row exists only because egret REMOVED a minijinja default,
            // which is a decision and not an absence. The same rule
            // `refusal-sets.txt` carries.
            //
            // **For a BOTH-BIND row the rule is the corpus entry**, which is
            // what makes a semantic divergence on a shared name recordable at
            // all. Before the fix round this file forbade a reason on exactly
            // those rows -- so the `replace` class, which is the class this
            // fixture exists for, was the one thing it could not record.
            if engine == "yes" && egret == "yes" {
                assert!(
                    golden_ids.contains(semantics),
                    "{kind} {name}: `{semantics}` is not a corpus id"
                );
                // **The "a baselined id is preferred" rule, enforced.** The
                // header has said so since fix round 1 and nothing checked it,
                // so swapping a row from its baselined entry to a passing
                // sibling -- which is exactly the hiding the rule forbids --
                // survived. Measured, on the `sort` row.
                let family: Vec<&str> = golden_ids
                    .iter()
                    .copied()
                    .filter(|id| {
                        let stem = format!("{kind}-{}", name.to_lowercase());
                        *id == stem || id.starts_with(&format!("{stem}-"))
                    })
                    .collect();
                let diverging: Vec<&str> = family
                    .iter()
                    .copied()
                    .filter(|id| baselined_ids.contains(id))
                    .collect();
                assert!(
                    diverging.is_empty() || diverging.contains(&semantics),
                    "{kind} {name}: `{semantics}` passes while {diverging:?} \
                     do not -- a row must name a baselined entry when its name has one, \
                     or the divergence is hidden behind a passing sibling"
                );
                let diverges = baselined_ids.contains(semantics);
                assert_eq!(
                    reason == "-",
                    !diverges,
                    "{kind} {name}: `{semantics}` is {} baseline.txt, so the reason column must \
                     {} -- a shared name's divergence is recorded HERE and held by the ratchet",
                    if diverges { "in" } else { "not in" },
                    if diverges {
                        "say what differs"
                    } else {
                        "be `-`"
                    }
                );
            } else {
                assert_eq!(
                    semantics, "-",
                    "{kind} {name}: only a both-bind row names a comparison entry"
                );
                assert_ne!(
                    reason, "-",
                    "{kind} {name}: the two sides differ and the row carries no reason"
                );
            }

            // The probe. A name egret does not bind produces minijinja's
            // `unknown filter` / `unknown function` / `unknown test`; anything
            // else -- a value, or a complaint about arguments -- means it is
            // bound.
            let (template, unknown) = match kind {
                "filter" => (format!("{{{{ 'x' | {name} }}}}"), "unknown filter"),
                "function" => (format!("{{{{ {name}() }}}}"), "unknown function"),
                "test" => (format!("{{{{ 'x' is {name} }}}}"), "unknown test"),
                other => panic!("unknown kind {other:?} in name-surface.txt"),
            };
            let bound = match render_str(&template, &RenderContext::new()) {
                Ok(_) => true,
                Err(e) => !e.to_string().contains(unknown),
            };
            assert_eq!(
                bound,
                egret == "yes",
                "{kind} {name}: `oracle/name-surface.txt` says egret {}bind it, and it does {}",
                if egret == "yes" { "" } else { "does not " },
                if bound { "" } else { "not" },
            );
        }

        // A row cannot be deleted to make a failure go away: these three are
        // `host.Oracle dump`'s own counts for the built engine, plus Pebble's
        // core/escaper/i18n extensions. The total also holds the egret-only
        // rows: 186 -> 185 at package #5a Task A2, which removed the egret-only
        // function `kestra_compare_chain` (a chain compiles left-associatively
        // to the per-operator functions now); 185 -> 190 at package #5a Task
        // A3, which added the egret-only `kestra_add`, `kestra_sub`,
        // `kestra_mul`, `kestra_div` and `kestra_mod` (the arithmetic pass);
        // 190 -> 193 at package #5a Task A5, which added the egret-only
        // `kestra_path`, `kestra_present` and `kestra_defined` (a path on the
        // left of `??`/`???`); 193 -> 196 at package #5a Task A6
        // (2026-10-29), which added the egret-only `kestra_contains`,
        // `kestra_range` and `kestra_bool` (`contains`, `..` and `and`/`or`);
        // the three engine counts below did not move.
        assert_eq!(rows, 196, "row count");
        assert_eq!(counts.get("filter"), Some(&63), "engine filter count");
        assert_eq!(counts.get("function"), Some(&37), "engine function count");
        assert_eq!(counts.get("test"), Some(&8), "engine test count");
    }

    /// The other half of [`the_built_in_surface_matches_the_engines`]: a name
    /// egret binds that has **no row at all** must fail.
    ///
    /// # Why this is a second test and not a stronger assertion in the first
    ///
    /// The first one iterates the fixture's rows and probes egret for each.
    /// That catches a row whose claim is wrong and — since W2 Task 8's mutation
    /// round — a removed minijinja default that came back. It cannot catch an
    /// *addition*: a filter registered under a brand-new name is in neither
    /// the fixture nor the engine, so no row mentions it, the probe never
    /// runs, and `rows == 177` is still satisfied. Measured, before this
    /// existed: registering one left the suite green.
    ///
    /// **The scan is textual, so a doc comment containing a registration call
    /// adds a phantom name.** That is not hypothetical — this comment used to
    /// spell the example out and this test failed on its own prose. Write such
    /// an example without the literal.
    ///
    /// So this computes what egret binds instead of reading it:
    ///
    /// - **functions** exactly, from `Environment::globals()`, which minijinja
    ///   does expose;
    /// - **filters** and **tests** as
    ///   `(minijinja's pinned defaults − the removal list) ∪ (what
    ///   `runners::` registers)`, the second half scanned out of the committed
    ///   source with `include_str!` — the same technique
    ///   `tests/oracle.rs::declared_test_names` uses, and for the same reason:
    ///   minijinja exposes no filter or test registry.
    ///
    /// The scan and the defaults close different holes and both are needed.
    /// The defaults are pinned by an `=` requirement on the dependency, so a
    /// minor release cannot widen them behind this.
    #[test]
    fn no_name_egret_binds_is_missing_from_the_surface_fixture() {
        const FIXTURE: &str = include_str!("../../oracle/name-surface.txt");
        const SOURCES: &[(&str, &str)] = &[
            ("env", include_str!("env.rs")),
            ("filters", include_str!("filters.rs")),
            ("functions", include_str!("functions.rs")),
            ("javaformat", include_str!("javaformat.rs")),
            ("pebble_tests", include_str!("pebble_tests.rs")),
        ];

        let mut rows: std::collections::BTreeSet<(String, String)> = Default::default();
        for line in FIXTURE.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f[3] == "yes" {
                rows.insert((f[0].to_string(), f[1].to_string()));
            }
        }

        // Registered by a string literal somewhere in `src/runners/`.
        let mut bound: std::collections::BTreeSet<(String, String)> = Default::default();
        for (_, source) in SOURCES {
            for (call, kind) in [
                ("add_filter(\"", "filter"),
                ("add_function(\"", "function"),
                ("add_test(\"", "test"),
            ] {
                for (at, _) in source.match_indices(call) {
                    let rest = &source[at + call.len()..];
                    if let Some(end) = rest.find('"') {
                        bound.insert((kind.to_string(), rest[..end].to_string()));
                    }
                }
            }
        }
        // minijinja's own defaults, minus what `environment()` removes.
        for name in MINIJINJA_DEFAULT_FILTERS {
            if !MINIJINJA_ONLY_FILTERS.contains(name) {
                bound.insert(("filter".to_string(), (*name).to_string()));
            }
        }
        for name in MINIJINJA_DEFAULT_TESTS {
            if !MINIJINJA_ONLY_TESTS.contains(name) {
                bound.insert(("test".to_string(), (*name).to_string()));
            }
        }
        // Functions, exactly, from the environment itself.
        let env = environment();
        for (name, _) in env.globals() {
            // The one global no template can name -- it has a space in it.
            // See [`CONTEXT_SCOPE_NAMES`], and
            // `the_context_scope_global_is_unreachable_from_a_template`.
            if name == CONTEXT_SCOPE_NAMES {
                continue;
            }
            bound.insert(("function".to_string(), name.to_string()));
        }

        let missing: Vec<&(String, String)> = bound.difference(&rows).collect();
        assert!(
            missing.is_empty(),
            "egret binds {} name(s) with no row in oracle/name-surface.txt: {missing:?}",
            missing.len()
        );
        let stale: Vec<&(String, String)> = rows.difference(&bound).collect();
        assert!(
            stale.is_empty(),
            "oracle/name-surface.txt says egret binds {} name(s) it does not: {stale:?}",
            stale.len()
        );
    }

    /// [`CONTEXT_SCOPE_NAMES`] cannot be reached from a template, which is why
    /// it is a global with no fixture row.
    ///
    /// The same argument as the six operator-spelled tests, and pinned the
    /// same way rather than left in prose: `minijinja`'s parser reads an
    /// identifier, and this name contains a space.
    #[test]
    fn the_context_scope_global_is_unreachable_from_a_template() {
        assert!(
            CONTEXT_SCOPE_NAMES.contains(' '),
            "the point of this name is that it is not an identifier"
        );
        let message = render_str(
            &format!("{{{{ {CONTEXT_SCOPE_NAMES} }}}}"),
            &RenderContext::new(),
        )
        .expect_err("must not parse");
        assert!(message.to_string().contains("syntax error"), "{message}");
        // And it is not reachable as an ordinary name either.
        assert!(render_str("{{ egret }}", &RenderContext::new()).is_err());
    }

    /// The six operator-spelled test names minijinja registers are unreachable
    /// from template syntax, which is why they are not in
    /// [`MINIJINJA_ONLY_TESTS`] and have no fixture row.
    ///
    /// Without this, "unreachable" is prose, and the next reader who notices
    /// `==` in minijinja's test map has no way to tell a considered omission
    /// from an oversight.
    #[test]
    fn the_operator_spelled_tests_are_unreachable_rather_than_removed() {
        for name in ["==", "!=", "<", "<=", ">", ">="] {
            let message = render_str(
                &format!("{{{{ 'x' is {name}(1) }}}}"),
                &RenderContext::new(),
            )
            .expect_err("must not parse");
            assert!(
                message.to_string().contains("expected identifier"),
                "{name}: {message}"
            );
        }
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
    /// A duplicate key is collapsed by [`last_value_wins`] before either path
    /// sees it, so the map holds ONE `k0`, carrying the LAST value in the
    /// FIRST position. Both halves of the threshold are exercised, because a
    /// test that only used a wide map would pass with the scan deleted.
    #[test]
    fn a_wide_map_answers_exactly_what_the_narrow_one_does() {
        for width in [4usize, ORDERED_MAP_INDEX_THRESHOLD, 5_000] {
            let mut entries: Vec<(String, MjValue)> = (0..width)
                .map(|i| (format!("k{i}"), MjValue::from(i)))
                .collect();
            // The same name twice: `LinkedHashMap.put` keeps the first
            // POSITION and the last VALUE.
            entries.push(("k0".to_string(), MjValue::from("second")));
            let map = MjValue::from_object(OrderedMap::new(entries));

            assert_eq!(
                map.get_item(&MjValue::from("k0")).unwrap(),
                MjValue::from("second"),
                "width {width}: a duplicate key must answer its LAST value"
            );
            assert_eq!(
                map.try_iter().unwrap().next().unwrap(),
                MjValue::from("k0"),
                "width {width}: and keep its FIRST position"
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
            assert_eq!(
                map.try_iter().unwrap().count(),
                width,
                "width {width}: the duplicate is collapsed, not carried"
            );
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
        // `OrderedMap` exists. This is that layer, directly -- and it now
        // shows TWO independent reasons nothing in this crate goes through
        // `Value::from_serialize`:
        //
        // 1. the keys come back a, b, c: minijinja's `ValueMap` is a
        //    `BTreeMap` without its own `preserve_order` feature; and
        // 2. every NUMBER comes back as `serde_json`'s private
        //    `arbitrary_precision` sentinel, because `from_serialize` drives
        //    `Value`'s `Serialize` impl, which emits a number as a one-field
        //    struct -- and minijinja, having no idea what that struct means,
        //    builds it as a map. This crate's own readers (`OrderedJson` and
        //    `functions::BoundedVisitor`) decode the sentinel; `from_serialize`
        //    cannot, and there is no hook to teach it.
        //
        // Pinned exactly as measured. If a future minijinja or serde_json
        // changes either half, this is where it is noticed rather than in a
        // rendered value somewhere downstream.
        assert_eq!(
            serde_json::to_string(&MjValue::from_serialize(
                serde_json::from_str::<JsonValue>(r#"{"b":1,"a":2,"c":3}"#).unwrap()
            ))
            .unwrap(),
            r#"{"a":{"$serde_json::private::Number":"2"},"b":{"$serde_json::private::Number":"1"},"c":{"$serde_json::private::Number":"3"}}"#
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

    /// **The crate asserts the cargo resolution it is written against.**
    ///
    /// Two things, and the first one is the guard the merge gate wanted most
    /// because it holds under *every* command anyone runs — `-p
    /// egret-workflow`, `--workspace`, a downstream binary — rather than only
    /// under a CI step someone remembered to add.
    ///
    /// 1. `serde_json`'s `arbitrary_precision` is ON. It is named in this
    ///    crate's `Cargo.toml` precisely so this holds everywhere; before
    ///    that it was on in `egret-bin` and off under `-p egret-workflow`,
    ///    and the difference silently changed what 15 green tests meant. If
    ///    the declaration is ever removed, this fails **by name** instead of
    ///    as a value mismatch three modules away.
    /// 2. The sentinel is still spelled the way this module matches on it.
    ///    [`ARBITRARY_PRECISION_TOKEN`] cannot be imported — `serde_json`'s
    ///    own constant is `pub(crate) const TOKEN`
    ///    (`serde_json-1.0.151/src/number.rs:18`) and no public item
    ///    re-exports it, so `serde_json::value::TOKEN` does not exist. A
    ///    copied string asserted against itself would prove nothing, so this
    ///    drives a **real serializer** and reads the key back off it.
    ///
    /// # The instrument had to be changed to measure anything
    ///
    /// The first version of this probe was
    /// `serde_json::to_string(&json!({"n": 2.5}))` and it answered `{"n":2.5}`
    /// **with the feature on**: `serde_json`'s own serializer recognises its
    /// own token and writes the number back out. So that probe passes
    /// identically either way and proves nothing at all. The sentinel is only
    /// visible to a **foreign** serializer — which is exactly the situation
    /// the crate is in, and exactly why the bug existed.
    #[test]
    fn the_arbitrary_precision_sentinel_is_still_spelled_this_way() {
        // minijinja is the foreign serializer, and `serde_json::to_string`
        // over the RESULT only ever sees a plain map by then.
        let through_a_foreign_serializer =
            serde_json::to_string(&MjValue::from_serialize(serde_json::json!(2.5)))
                .expect("the minijinja value serialises");
        assert!(
            through_a_foreign_serializer.contains(ARBITRARY_PRECISION_TOKEN),
            "serde_json did not emit the arbitrary_precision sentinel. Either the feature is \
             OFF for this build -- check `serde_json` in crates/egret-workflow/Cargo.toml, and \
             see `OrderedJsonVisitor::visit_f64` for what that costs -- or serde_json has \
             renamed the token and `ARBITRARY_PRECISION_TOKEN` must follow. \
             Got: {through_a_foreign_serializer}"
        );

        // And the decode itself, through the parser rather than a hand-built
        // `MapAccess`: floats keep their value and integers past `u64::MAX`
        // keep their digits.
        assert_eq!(render_ordered("{{ f }}", r#"{"f": 2.5}"#).unwrap(), "2.5");
        assert_eq!(
            render_ordered("{{ v }}", r#"{"v": 18446744073709551616}"#).unwrap(),
            "18446744073709551616"
        );
        // `context-integer-past-u64-classname`: Jackson says `BigInteger`,
        // and it used to say `java.lang.Double` here because the digits were
        // gone before any visitor ran.
        assert_eq!(
            render_ordered("{{ v | className }}", r#"{"v": 18446744073709551616}"#).unwrap(),
            "java.math.BigInteger"
        );
        // A real author key that merely resembles the sentinel is not one:
        // the sentinel is a ONE-entry map, and a second entry is a map.
        assert_eq!(
            render_ordered(
                "{{ m }}",
                r#"{"m": {"$serde_json::private::Number": "1.0", "z": 2}}"#
            )
            .unwrap(),
            r#"{"$serde_json::private::Number":"1.0","z":2}"#
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
        assert_eq!(f(2.0), "2.0");
        assert_eq!(f(100.0), "100.0");
        assert_eq!(f(1e23), "1.0E23");
        assert_eq!(f(1e-5), "1.0E-5");
    }

    /// Fix wave #5a M9: a one-digit shortest decimal becomes Java's closest
    /// two-digit one. Every right-hand side is `Double.toString(
    /// Double.MIN_VALUE * k)` measured with `jshell` on JDK 25.
    #[test]
    fn subnormals_print_javas_two_digits() {
        let f = |x: f64| {
            let mut s = String::new();
            write_java_double(&mut s, x);
            s
        };
        for (k, want) in [
            (1u32, "4.9E-324"),
            (2, "9.9E-324"),
            (3, "1.5E-323"),
            (4, "2.0E-323"),
            (9, "4.4E-323"),
            (10, "4.9E-323"),
            (12, "5.9E-323"),
        ] {
            assert_eq!(f(f64::from_bits(1) * f64::from(k)), want, "MIN_VALUE * {k}");
        }
        assert_eq!(f(-f64::from_bits(1)), "-4.9E-324");
        assert_eq!(f(1e-310), "1.0E-310");
        assert_eq!(f(f64::MIN_POSITIVE), "2.2250738585072014E-308");
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

    /// Fuel bounds instructions, not bytes. Doubling a string twenty times
    /// is a handful of instructions and 8 MiB, so without the sink cap this
    /// allocates that much from one short template. (It was `'a' * 8000000`
    /// until package #5a's Task A3 made `*` Pebble's, which refuses a string
    /// that is not a number.)
    #[test]
    fn an_oversized_render_is_refused_by_the_output_cap() {
        let doubling = "{% set s = s ~ s %}".repeat(20);
        let err = render(
            &format!("{{% set s = 'aaaaaaaa' %}}{doubling}{{{{ s }}}}"),
            json!({}),
        )
        .unwrap_err();
        assert_eq!(
            err,
            RenderError::OutputTooLarge {
                limit: MAX_OUTPUT_BYTES
            }
        );
        // And the budget itself is real: just under it still renders.
        let doubling = "{% set s = s ~ s %}".repeat(10);
        let ok = render(
            &format!("{{% set s = 'a' %}}{doubling}{{{{ s }}}}"),
            json!({}),
        )
        .unwrap();
        assert_eq!(ok.len(), 1024);
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

    /// Package #5a, Task A1: the arithmetic probes Kestra refuses and egret
    /// already refuses. They are `compare: "error-class"` in the oracle corpus
    /// and born matching, so this is what fails if egret ever starts answering
    /// one of them (`oracle/error-class-pairs.txt`). Today the refusal is
    /// minijinja's own; Task A3's arithmetic seam must keep every one of them
    /// failing (`OperatorUtils` throws `Could not perform …` for each). Since
    /// Task A3 every Long/operand row fails with Kestra's own text, which
    /// `runners::arith`'s `an_operation_kestra_refuses_fails_with_kestras_text`
    /// asserts and pairs; since Task A4 the BigDecimal row does too, through
    /// `runners::arith`'s `big_decimal_arithmetic_is_javas`.
    #[test]
    fn arithmetic_kestra_refuses_is_refused_here_too() {
        for (source, ctx, text) in [
            (
                "{{ 7 % 0 }}",
                json!({}),
                "Could not perform greater modulus",
            ),
            (
                "{{ '1.5' | number(type='BIGDECIMAL') / 0 }}",
                json!({}),
                "Could not perform division",
            ),
            (
                "{{ n + 1 }}",
                json!({ "n": null }),
                "Could not perform addition",
            ),
            (
                "{{ 2 * n }}",
                json!({ "n": null }),
                "Could not perform multiplication",
            ),
            ("{{ 2 + [1] }}", json!({}), "Could not perform addition"),
            (
                "{{ m + 1 }}",
                json!({ "m": { "a": 1 } }),
                "Could not perform addition",
            ),
            (
                "{{ m + n }}",
                json!({ "m": { "a": 1 }, "n": { "b": 2 } }),
                "Could not perform addition",
            ),
        ] {
            let got = render(source, ctx);
            assert!(
                matches!(got, Err(RenderError::Template(ref m)) if m.contains(text)),
                "{source} must fail with Kestra's `{text}`; got {got:?}"
            );
        }
    }

    /// Package #5a, Task A1: a missing operand that Kestra evaluates fails here
    /// too. `2 < 1 < missing.x` is `(2 < 1) < missing.x` in Pebble. Task A2's
    /// left-associative chain must keep it failing. (The two `??` rows moved
    /// to [`a_missing_right_side_kestra_reaches_fails_on_it`] with Task A5.)
    #[test]
    fn a_missing_operand_kestra_evaluates_fails_here_too() {
        // Kestra fails on the missing root (`LessThanExpression` evaluates
        // both operands before comparing), not on comparing the boolean
        // `2 < 1` (`Could not perform less than comparison`): fix wave #5a M6.
        let got = render("{{ 2 < 1 < missing.x }}", json!({}));
        assert!(
            matches!(got, Err(RenderError::Template(ref m))
                if m.contains("undefined") && !m.contains("comparison")),
            "must fail on the undefined operand, as Kestra's does; got {got:?}"
        );
    }

    /// Package #5a, Task A5 (spec D6, Q6): `??`/`???` answer through a missing
    /// member or index chain and evaluate the right side only when needed.
    /// The oracle's `coalesce-chain-*` and `coalesce-lazy-*` entries hold the
    /// golden values; this adds the rows they do not reach: a step from a
    /// null under `???` (Pebble's `RootAttributeNotFoundException`, which
    /// `???` catches too), a present `false`/`0`/`''` (present, so not
    /// replaced), the `{% set %}` and call positions, and the non-path left
    /// operand that stays eager (the recorded divergence).
    #[test]
    fn a_path_on_the_left_of_coalesce_reads_through_a_missing_chain() {
        for (source, ctx, expected) in [
            ("{{ obj.a.b ?? 'h' }}", json!({ "obj": {} }), "h"),
            (
                "{{ obj.a.b ??? 'h' }}",
                json!({ "obj": { "a": null } }),
                "h",
            ),
            ("{{ obj.a ??? 'h' }}", json!({ "obj": { "a": null } }), ""),
            (
                "{{ obj.a ?? 'h' }}",
                json!({ "obj": { "a": false } }),
                "false",
            ),
            ("{{ obj.a ?? 'h' }}", json!({ "obj": { "a": 0 } }), "0"),
            ("{{ obj.a ?? 'h' }}", json!({ "obj": { "a": "" } }), ""),
            ("{{ l[1] ?? 'h' }}", json!({ "l": [1, 2] }), "2"),
            ("{{ l[5] ?? 'h' }}", json!({ "l": [1] }), "h"),
            ("{{ s.a ?? 'h' }}", json!({ "s": "text" }), "h"),
            ("{{ x ?? (1 / 0) }}", json!({ "x": 1 }), "1"),
            ("{{ obj.a.b ?? obj.c ?? 'h' }}", json!({ "obj": {} }), "h"),
            (
                "{% set v = obj.a.b ?? 'h' %}{{ v }}",
                json!({ "obj": {} }),
                "h",
            ),
            ("{{ (obj.a.b ?? 'h') ~ '!' }}", json!({ "obj": {} }), "h!"),
            ("{{ obj.a.b ?? 1 + 2 }}", json!({ "obj": {} }), "3"),
        ] {
            let got = render(source, ctx.clone()).unwrap_or_else(|e| panic!("{source}: {e}"));
            assert_eq!(got, expected, "{source} with {ctx}");
        }
        // The recorded divergence (`coalesce-non-path-left-missing-right`):
        // Kestra answers `1`; a call on the left keeps the eager form here.
        assert!(render("{{ fromJson('1') ?? missing.y }}", json!({})).is_err());
    }

    /// Package #5a, Task A5 (spec D6): `??` reaches its right side when the
    /// left is missing, and a right side whose root is missing fails there,
    /// as Kestra's `RootAttributeNotFoundException` for `missing` does
    /// (`coalesce-missing-right-evaluated`,
    /// `coalesce-chain-missing-right-evaluated`). **For Kestra's reason, not
    /// the left side's:** before Task A5, `obj.a.b` raised before the
    /// coalesce ran, so the row failed whatever the right side was. The
    /// second half proves the failure is the right side's: the same
    /// template with `missing.y` present answers it.
    #[test]
    fn a_missing_right_side_kestra_reaches_fails_on_it() {
        for (source, ctx) in [
            ("{{ nothere ?? missing.y }}", json!({})),
            ("{{ obj.a.b ?? missing.y }}", json!({ "obj": {} })),
            ("{{ obj.a.b ??? missing.y }}", json!({ "obj": {} })),
        ] {
            let got = render(source, ctx.clone());
            let err = got.expect_err(source).to_string();
            assert!(
                err.contains("undefined"),
                "{source}: the right side's missing root fails it; got {err}"
            );
            let mut present = ctx;
            present["missing"] = json!({ "y": "r" });
            assert_eq!(render(source, present).unwrap(), "r", "{source}");
        }
    }

    // (`contains_on_a_number_is_refused` was here, Task A1's placeholder; Task
    // A6 ported `contains` and its refusal is asserted with Kestra's text by
    // `runners::operators::tests::contains_on_what_pebble_cannot_search_fails_with_kestras_text`.)

    /// Asserts that every `(source, context)` row fails to render. The four
    /// tests below are package #5b Task B1's pairs for its born-matching
    /// error-class entries (`oracle/error-class-pairs.txt`).
    fn assert_each_fails(rows: &[(&str, JsonValue)]) {
        for (source, ctx) in rows {
            let got = render(source, ctx.clone());
            assert!(
                got.is_err(),
                "{source} must fail, as Kestra's does; got {got:?}"
            );
        }
    }

    /// Package #5b, Task B1 (spec D8): the tests Kestra refuses, tightened to
    /// Kestra's reason by Task B2. A missing root or member under `is null`/
    /// `is empty` fails as an undefined value (Kestra's
    /// `RootAttributeNotFoundException`/`AttributeNotFoundException` under
    /// strict variables; egret raises the VM's own `UndefinedError`), and
    /// `is json` on anything but a string fails with `JsonTest`'s
    /// `(String)` cast (`ClassCastException`) or, for a null, Jackson's
    /// `argument "content" is null`.
    #[test]
    fn a_test_kestra_refuses_is_refused_here_too() {
        for (source, ctx, want) in [
            ("{{ x is null }}", json!({}), "undefined"),
            ("{{ o.x is null }}", json!({ "o": {} }), "undefined"),
            ("{{ x is empty }}", json!({}), "undefined"),
            (
                "{{ x is json }}",
                json!({ "x": { "a": 1 } }),
                "class java.util.LinkedHashMap cannot be cast to class java.lang.String",
            ),
            (
                "{{ x is json }}",
                json!({ "x": [1] }),
                "class java.util.ArrayList cannot be cast to class java.lang.String",
            ),
            (
                "{{ x is json }}",
                json!({ "x": 1 }),
                "class java.lang.Integer cannot be cast to class java.lang.String",
            ),
            (
                "{{ x is json }}",
                json!({ "x": null }),
                "argument \"content\" is null",
            ),
        ] {
            let error = render(source, ctx)
                .expect_err(&format!("{source} must fail, as Kestra's does"))
                .to_string();
            assert!(error.contains(want), "{source}: {error}");
        }
    }

    /// Package #5b, Task B1 (spec D10): `max`/`min` refuse an operand that is
    /// not a number (`invalid operands for mathematical comparison
    /// [GREATER_THAN]`, `[LESS_THAN]` for `min`). **Placeholder:** today they
    /// fail because neither function exists here. Task B3 must keep every row
    /// failing for Kestra's reason, then tighten this to Kestra's text.
    #[test]
    fn max_and_min_refuse_what_pebble_cannot_compare() {
        assert_each_fails(&[
            ("{{ max('a', 'b') }}", json!({})),
            ("{{ min('a', 1) }}", json!({})),
            ("{{ max('5', 1) }}", json!({})),
            ("{{ max(1, true) }}", json!({})),
            ("{{ max([1, 2], 3) }}", json!({})),
        ]);
    }

    /// Package #5b, Task B1 (spec D13): inputs the ten filters refuse in
    /// Kestra: a negative `abbreviate` width, a non-string `abbreviate` input,
    /// a bad base64 string, `merge` on a string, `numberformat` on a string,
    /// and `rsort` of mixed classes (a context list of a Double and two
    /// Integers fails too). **Placeholder:** today they fail because the
    /// filters are unknown here. Task B4 must keep every row failing for
    /// Kestra's reason, then tighten this to Kestra's text.
    #[test]
    fn a_filter_input_kestra_refuses_is_refused_here_too() {
        assert_each_fails(&[
            ("{{ 'abc' | abbreviate(-1) }}", json!({})),
            ("{{ n | abbreviate(3) }}", json!({ "n": 123456 })),
            ("{{ '!!' | base64decode }}", json!({})),
            ("{{ 'a' | merge(['b']) }}", json!({})),
            ("{{ '12' | numberformat }}", json!({})),
            ("{{ l | rsort }}", json!({ "l": [1.5, 3, 2] })),
            ("{{ [1, 'a'] | rsort }}", json!({})),
        ]);
    }

    /// Package #5b, Task B1 (spec D12): a strategy Pebble's `EscapeFilter`
    /// does not register fails (`Unknown escaping strategy [x]`); the names
    /// are case-sensitive, so `JS` is unknown too. **Placeholder:** today
    /// every strategy but `html` is refused with egret's own text. Task B5
    /// must keep these failing when it adds the other four, then tighten
    /// this to Kestra's text.
    #[test]
    fn an_escape_strategy_pebble_does_not_know_is_refused() {
        assert_each_fails(&[
            (
                "{{ s | escape(strategy='html_attr') }}",
                json!({ "s": "a b" }),
            ),
            ("{{ s | escape(strategy='nope') }}", json!({ "s": "a" })),
            ("{{ s | escape(strategy='JS') }}", json!({ "s": "a" })),
        ]);
    }
}
