//! The apps-origin proxy (spec §6.2): everything between the browser's iframe and a
//! marimo pod. Pure decisions live in the small modules below and are tested without
//! a network; [`relay`] is the part that moves bytes.

pub mod pathguard;
pub mod policy;
