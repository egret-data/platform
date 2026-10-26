//! Turning YAML source into a validated `Flow`: located parse errors, the
//! task-type registry, and the three validation layers.
//!
//! `registry` stays a module path (`spec::registry::known_type`), not
//! flattened, because it is meant to be read like a table, not called like a
//! single function alongside `parse`/`validate`.

pub(crate) mod keys;
mod parse;
pub(crate) mod path;
mod properties;
pub mod registry;
pub mod schema;
mod unrunnable;
pub(crate) mod validate;

pub use parse::{parse, ParseError};
pub use properties::property_warnings;
pub use unrunnable::{describe, unrunnable};
pub use validate::{validate, validate_for_run, Finding, ValidationReport};
