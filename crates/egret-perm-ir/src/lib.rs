//! The target-neutral IR between the `perm` store and every compiler target
//! (governance sub-project 1).
//!
//! Pure: no database and no I/O. It knows enforcement points by kind
//! (`engine × mechanism`, [`ep`]), what each kind can enforce
//! ([`matrix`], the capability matrix), and what follows from that for a
//! rule's enforcement ([`enforcement`]). Target-specific rendering lives in
//! the target's own crate.

pub mod audience;
pub mod enforcement;
pub mod ep;
pub mod lower;
pub mod matrix;
pub mod statement;
