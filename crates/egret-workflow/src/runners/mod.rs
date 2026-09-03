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

pub mod env;
mod filters;
mod functions;
mod ionfmt;
mod javafmt;
mod javare;
mod jqsandbox;
pub mod rewrite;
mod yamlfmt;

pub use env::{environment, render_str, RenderContext, RenderError};
pub use rewrite::{rewrite, RewriteError};
