//! The trigger loop's I/O half (W12): `workflow.triggers`, and the code that
//! reads and writes it.
//!
//! [`trigger`](crate::trigger) stays pure -- its module doc promises that
//! nothing in that tree reaches a database, and its tests rely on it -- so the
//! database lives here, and this module is `trigger`'s only I/O caller. Every
//! "now" in this module comes from Postgres's `now()` in the current
//! transaction, handed to `trigger` as a [`FixedClock`](crate::trigger::FixedClock).
//!
//! See `docs/en/specs/2026-09-24-workflow-w12-trigger-loop-design.md` §3.

mod sync;

pub use sync::sync;
