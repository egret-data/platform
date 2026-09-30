//! W2 — expression rendering: Kestra's Pebble-based expression language,
//! ported onto minijinja.
//!
//! Three pieces, in the order a render goes through them:
//!
//! - `rewrite` reshapes Kestra's operator syntax (`??`, `???`, ternary
//!   `? :`, `isIn`) into syntax minijinja already understands, or into calls
//!   to functions `env` registers. It has no context and evaluates nothing —
//!   see `rewrite`'s module doc for why a regex cannot do this job.
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

pub mod env;
mod filters;
mod functions;
mod ionfmt;
mod javafmt;
mod javaformat;
mod javare;
mod jqsandbox;
pub mod rewrite;
pub mod secrets;
mod yamlfmt;

// `environment` is deliberately NOT re-exported: it is `pub(crate)` and is
// the unbounded half of the renderer (no output cap, no 128 MiB render
// thread, no rewrite, no `{% raw %}` protection, no context-scope
// sentinel). `render_str` is the supported entry point -- see
// `env::environment`'s doc for the measured table.
pub use env::{
    referenced_roots, render_str, render_str_guarded, GuardedRender, RenderContext, RenderError,
};
/// The refusal every render but a `kestra.*` task's gives `secret()`, also
/// the message of `spec::validate`'s publish-time refusal (W5b fix round 1).
pub(crate) use functions::SECRET_REFUSAL;
pub use rewrite::{rewrite, RewriteError};
