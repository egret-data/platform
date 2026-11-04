//! Governance SP2b: the OPA decision evaluator over the neutral IR.
//!
//! This crate is pure: no database, async runtime, network or filesystem
//! access. `tests/boundary.rs` enforces it.

pub mod decide;
pub mod pattern;
pub mod snapshot;
pub mod trino_ops;

pub use decide::{Decision, EvalError, ObjectPath, Resource};
pub use snapshot::{ClassifiedColumn, Snapshot, SnapshotError, SnapshotInput};
