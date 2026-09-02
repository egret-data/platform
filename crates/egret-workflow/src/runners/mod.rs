//! W2 — expression rendering: Kestra's Pebble-based expression language,
//! ported onto minijinja.
//!
//! Three pieces, in the order a render goes through them:
//!
//! - `rewrite` reshapes Kestra's operator syntax (`??`, `???`, ternary
//!   `? :`, `isIn`) into syntax minijinja already understands, or into calls
//!   to functions `env` registers. It has no context and evaluates nothing —
//!   see `rewrite`'s module doc for why a regex cannot do this job.
//! - `filters` is the 28 filters of W2 Tasks 4 and 5, registered onto that
//!   engine. Two of its names (`indent`, `string`) override minijinja
//!   built-ins rather than adding to them.
//! - `javafmt` is Java's `DateTimeFormatter` pattern language, which the six
//!   date/time filters in `filters` are written against. It is its own module
//!   because no Rust crate provides that pattern language and because it is
//!   the only part of this crate that is about `java.time` rather than about
//!   Pebble.
//! - `env` is the engine that evaluates the reshaped source, wrapped in the
//!   same pre/post-processing `VariableRenderer` does (the `{% raw %}`
//!   protection, the re-render loop). `Property<T>::render` is `env`'s
//!   `render_str` plus a conversion.

pub mod env;
mod filters;
mod javafmt;
pub mod rewrite;

pub use env::{environment, render_str, RenderContext, RenderError};
pub use rewrite::{rewrite, RewriteError};
