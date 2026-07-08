//! Engine-provisioning spec types. The definitions now live in the dependency-free
//! `egret-engine-mgmt-api` contract crate; re-exported here so orchestrator's
//! manifest builders keep referring to `crate::spec::*`.

pub use egret_engine_mgmt_api::*;
