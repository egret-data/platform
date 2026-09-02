//! W2 — expression rendering: Kestra's Pebble-based expression language,
//! ported onto minijinja.
//!
//! `rewrite` (this module's only export so far) is the first piece: it
//! reshapes Kestra's operator syntax (`??`, `???`, ternary `? :`, `isIn`)
//! into syntax minijinja already understands, or into calls to functions a
//! later task registers. It has no context and evaluates nothing — see
//! `rewrite`'s module doc for why a regex cannot do this job.

pub mod rewrite;

pub use rewrite::{rewrite, RewriteError};
