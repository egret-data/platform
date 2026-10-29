//! W2 — expression rendering: Kestra's Pebble-based expression language,
//! ported onto minijinja.
//!
//! Three pieces, in the order a render goes through them:
//!
//! - `rewrite` reshapes Kestra's operator syntax (`??`, `???`, ternary
//!   `? :`, `isIn`) into syntax minijinja already understands, or into calls
//!   to functions `env` registers. It has no context and evaluates nothing —
//!   see `rewrite`'s module doc for why a regex cannot do this job.
//! - `cmp_rewrite` is the rewrite's second pass (W9c Task 4 fix round 1): it
//!   reads `rewrite`'s output with minijinja's own grammar and turns every
//!   comparison into a call to `compare`, whose functions compare a
//!   `BigDecimal`/`BigInteger` exactly and every other pair as minijinja
//!   does. Its third pass compiles `+ - * / %` into calls to `arith`.
//! - `operators` is Pebble's `contains`, `..` and the boolean coercion of
//!   `and`/`or` (package #5a Task A6), which `rewrite` compiles into calls.
//! - `filters` is all 35 of Kestra's filters, registered onto that engine.
//!   Two of its names (`indent`, `string`) override minijinja built-ins
//!   rather than adding to them.
//! - `functions` is the 19 of Kestra's 34 functions that reach no service.
//!   `range` is a third minijinja built-in it overrides — a global this time,
//!   not a filter, and inclusive where minijinja's is exclusive.
//! - `javafmt` is Java's `DateTimeFormatter` pattern language, which the six
//!   date/time filters in `filters` are written against. It is its own module
//!   because no Rust crate provides that pattern language and because it is
//!   the only part of this crate that is about `java.time` rather than about
//!   Pebble.
//! - `javaformat` is `java.util.Formatter`'s `%` grammar, which the `format`
//!   filter is written against. W2 Task 8's fix round 1 wrapped minijinja's
//!   own `format` on the premise that the two agreed; its round 2 measured
//!   five silently different values, two of them in the specifiers the premise
//!   named, and ported the grammar instead.
//! - `decimalformat` is `java.text.DecimalFormat` under the `en` symbols,
//!   which Pebble core's `numberformat` filter is written against (package
//!   #5b Task B4).
//! - `ionfmt`, `yamlfmt` and `javare` are the same shape of thing for W2
//!   Task 6's filters: Amazon Ion text as `JacksonMapper.ofIon()` writes it,
//!   YAML as Jackson's `YAMLGenerator` over SnakeYAML's emitter writes it,
//!   and `java.util.regex` translated onto the `regex` crate. Each is a
//!   foreign format with no Rust equivalent that answers the same way, and
//!   `javare`'s doc is required reading before touching a pattern.
//! - `env` is the engine that evaluates the reshaped source, wrapped in the
//!   same pre/post-processing `VariableRenderer` does (the `{% raw %}`
//!   protection, the re-render loop). `Property<T>::render` is `env`'s
//!   `render_str` plus a conversion.
//! - `secrets` is W5b's `secret()`: a two-pass render over `env`'s
//!   `render_str_with` that resolves it for `kestra.*` plugin tasks only. The
//!   shared environment keeps refusing it (see `secrets`' module doc).

mod arith;
mod cmp_rewrite;
mod compare;
mod decimalformat;
pub mod env;
pub mod files;
mod filters;
mod functions;
mod ionfmt;
mod javafmt;
mod javaformat;
mod javare;
mod jqsandbox;
mod nested;
mod operators;
mod pebble_tests;
pub mod rewrite;
pub mod secrets;
mod yamlfmt;

// `environment` is deliberately NOT re-exported: it is `pub(crate)` and is
// the unbounded half of the renderer (no output cap, no 128 MiB render
// thread, no rewrite, no `{% raw %}` protection, no context-scope
// sentinel). `render_str` is the supported entry point -- see
// `env::environment`'s doc for the measured table.
pub use env::{
    referenced_roots, render_str, render_str_guarded, render_typed, GuardedRender, RenderContext,
    RenderError,
};
pub use files::{FileMeta, FileScope, FileUnavailable, RenderFiles};
/// W9c final review F1: `storage::kv::value::coerce` writes a decimal under
/// `kvType: STRING` as Java's `BigDecimal.toString()`.
pub(crate) use filters::big_decimal_to_string;
#[cfg(test)]
pub(crate) use filters::jq_each_within as filters_jq_each_within;
/// W9a: `storage::uri` builds Kestra's URIs from these two (`Slugify.of`, and
/// the base62 of `IdUtils.from`).
pub(crate) use filters::slugify_str;
/// Package #4 Task C4: `http.SseRequest`'s `concatJqExpression`, run with the
/// `jq` filter's sandbox.
pub(crate) use filters::{jq_each, JqScalar};
pub(crate) use functions::base62;
/// The refusal every render but a `kestra.*` task's gives `secret()`, also
/// the message of `spec::validate`'s publish-time refusal (W5b fix round 1).
pub(crate) use functions::SECRET_REFUSAL;
pub(crate) use ionfmt::{
    write_ion_element, write_ion_float, write_ion_row, write_ion_string,
    write_symbol as write_ion_symbol,
};
/// W9a Task 12: `core.storage.Split`'s `regexPattern` is a `java.util.regex`
/// pattern, compiled by the same translation the `regex*` filters use.
pub(crate) use javare::{compile as compile_java_regex, JavaRegex};
pub use rewrite::{rewrite, RewriteError};
