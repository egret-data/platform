//! One Cedar policy template per role. The formatters now live in the shared
//! `egret-cedar` crate (EO2); re-exported here so the PDP's call sites are
//! unchanged and the Cedar text has exactly one source.
pub use egret_cedar::text::{classification_forbid, deny_template_for_role, template_for_role};
