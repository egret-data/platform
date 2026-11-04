//! Governance SP2b: egret answers Trino's OPA plugin from the lowered
//! permission IR. `build` assembles one profile's evaluator snapshot from the
//! same lowering `crate::trigger::compile_desired` renders for Ranger.

pub mod build;
pub mod cache;
pub mod wire;
