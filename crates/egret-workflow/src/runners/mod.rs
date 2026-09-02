//! W2 — expression rendering: Kestra's Pebble-based expression language,
//! ported onto minijinja.
//!
//! Two pieces, in the order a render goes through them:
//!
//! - `rewrite` reshapes Kestra's operator syntax (`??`, `???`, ternary
//!   `? :`, `isIn`) into syntax minijinja already understands, or into calls
//!   to functions `env` registers. It has no context and evaluates nothing —
//!   see `rewrite`'s module doc for why a regex cannot do this job.
//! - `env` is the engine that evaluates the reshaped source, wrapped in the
//!   same pre/post-processing `VariableRenderer` does (the `{% raw %}`
//!   protection, the re-render loop). `Property<T>::render` is `env`'s
//!   `render_str` plus a conversion.

pub mod env;
pub mod rewrite;

pub use env::{environment, render_str, RenderContext, RenderError};
pub use rewrite::{rewrite, RewriteError};
