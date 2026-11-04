//! The target-neutral IR between the `perm` store and every compiler target
//! (governance sub-project 1).
//!
//! Pure: no database and no I/O. It knows enforcement points by kind
//! (`engine × mechanism`, [`ep`]), what each kind can enforce
//! ([`matrix`], the capability matrix), and what follows from that for a
//! rule's enforcement ([`enforcement`]). Target-specific rendering lives in
//! the target's own crate.
//!
//! **The D4 boundary.** Store to IR holds the decisions: scope, skips,
//! refusals and tag expansion are taken in [`lower`] on data the loader
//! already read. IR to target renders shape only. To keep it that way this
//! crate has no sqlx in its own manifest, does no I/O, depends on neither
//! `egret-policy`, `egret-policy-api` nor `egret-ranger-eval`, and has no
//! Ranger word in a public type name (`tests/boundary.rs`). The Ranger target
//! is `egret-perm-compile`'s `ranger` module; the reads are its `ir_load`.

pub mod audience;
pub mod enforcement;
pub mod ep;
pub mod lower;
pub mod matrix;
pub mod statement;
