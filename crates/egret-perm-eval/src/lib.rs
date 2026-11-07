//! Governance SP2b: the OPA decision evaluator over the neutral IR.
//!
//! This crate is pure: no database, async runtime, network or filesystem
//! access. `tests/boundary.rs` enforces it.

pub mod decide;
#[cfg(test)]
mod oracle;
pub mod pattern;
pub mod snapshot;
pub mod trino_ops;
pub mod values;

pub use decide::{Decision, EvalError, ObjectPath, Resource};
pub use snapshot::{ClassifiedColumn, Snapshot, SnapshotError, SnapshotInput};
pub use values::{render_mask, TRINO_MASK_TRANSFORMERS};
