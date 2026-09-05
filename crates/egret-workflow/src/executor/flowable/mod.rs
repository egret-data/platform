//! One module per flowable, because each is an independent semantic unit with
//! its own corpus slice — and because a single `executor.rs` is how
//! `dataapi/src/lifecycle.rs` reached 12,091 lines.
//!
//! Task 3 ships `sequential`; Tasks 4-8 add `parallel`, `dag`, `branch` and
//! `loop_` beside it.

pub(crate) mod sequential;
