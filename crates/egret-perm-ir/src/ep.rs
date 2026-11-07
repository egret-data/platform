//! What the IR knows about the enforcement point it lowers for.

use std::collections::BTreeMap;

use egret_perm::EngineType;
use uuid::Uuid;

/// How an engine is asked: the profile's `policy.authz_services.mechanism`
/// (migration 0213). `Opa` exists for Trino only (governance SP2b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mechanism {
    Ranger,
    Opa,
}

/// `engine × mechanism`; the version is per bound engine (several share one profile).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EpKind {
    pub engine: EngineType,
    pub mechanism: Mechanism,
}

/// One engine bound to the profile and the mechanism version it runs, or why
/// the version is unknown (the `Err` text is today's `engine_plugin_version` error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundEngine {
    pub name: String,
    pub version: Result<String, String>,
}

/// Everything about one enforcement point (today: one profile) the lowering needs.
#[derive(Debug, Clone)]
pub struct EpInput {
    pub profile_id: i64,
    pub profile_name: String,
    pub kind: EpKind,
    /// Bound engines, by name; empty when none is bound.
    pub engines: Vec<BoundEngine>,
    /// The version egret's default image runs, used when `engines` is empty.
    pub default_version: Result<String, String>,
    /// The EP can resolve classifications itself (today: a Trino profile whose
    /// `tagService` names an existing service).
    pub classification_channel: bool,
    /// Catalog scope: registry catalog id → engine-facing name.
    pub catalogs: BTreeMap<Uuid, String>,
}
