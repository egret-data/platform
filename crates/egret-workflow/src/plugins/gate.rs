//! The ONE governance gate for Kestra plugin tasks (W5b spec §5).
//!
//! [`check`] is pure and is the only place that decides whether a plugin
//! task may run. It has two call sites, as W5a's script gate does: at publish
//! (`publish::check_plugin_tasks`, step 3c; a refusal is a named 400) and at
//! run after rendering, before anything is created (the plugin handler,
//! Task 6).
//!
//! # The rules, in order
//!
//! 1. the workspace feature `workflow_kestra_plugins` must be ON (OFF when
//!    the workspace has no row for it);
//! 2. `workflow_containers` must be ON too: a plugin task runs in W5a's
//!    container runner, so a workspace may run scripts without plugins, never
//!    plugins without scripts;
//! 3. a container runner must be configured (`workflow.container_runner` is
//!    not `none`): with none, nothing would run the task;
//! 4. the FQCN (without `kestra.`) must match the `kestra` entry's
//!    `allowed-types` with W5a's [`glob_match`]. **No `kestra` entry allows
//!    every catalog type** (the catalog was reviewed as a PR); an entry with
//!    an empty list allows none. A FQCN holds no `/`, so `*` spans package
//!    segments.
//!
//! The host image is `workflow.plugin_host.image` and is **not** matched
//! against `allowed-images`: the author cannot choose it.

use std::fmt;

use egret_config::PluginConfigurations;

use super::{CONFIG_TYPE, FEATURE_KEY};
use crate::scripts::gate::glob_match;
use crate::scripts::RunnerKind;

/// What [`check`] decides on.
#[derive(Debug, Clone, Copy)]
pub struct GateInput<'a> {
    /// The workspace has `workflow_kestra_plugins` ON.
    pub feature_enabled: bool,
    /// The workspace has `workflow_containers` ON.
    pub containers_enabled: bool,
    pub plugin_configurations: &'a PluginConfigurations,
    /// The configured container runner; `None` for `container_runner = none`.
    pub runner: Option<RunnerKind>,
    /// The task's class, without [`super::PREFIX`].
    pub fqcn: &'a str,
}

/// Why the gate refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateRefusal {
    /// `workflow_kestra_plugins` is OFF.
    FeatureDisabled,
    /// `workflow_containers` is OFF.
    ContainersDisabled,
    /// No container runner is configured.
    NoRunner,
    /// The FQCN matched none of the `kestra` entry's `allowed-types`.
    TypeNotAllowed { fqcn: String },
}

impl GateRefusal {
    /// A machine-checkable code, as `spec::Finding::code` carries.
    pub fn code(&self) -> &'static str {
        match self {
            GateRefusal::FeatureDisabled => "workflow_kestra_plugins_disabled",
            GateRefusal::ContainersDisabled => "workflow_containers_disabled",
            GateRefusal::NoRunner => "plugin_runner_not_configured",
            GateRefusal::TypeNotAllowed { .. } => "plugin_type_not_allowed",
        }
    }
}

impl fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateRefusal::FeatureDisabled => write!(
                f,
                "the '{FEATURE_KEY}' feature is disabled for this workspace; a platform admin \
                 must enable it before a Kestra plugin task can run"
            ),
            GateRefusal::ContainersDisabled => write!(
                f,
                "the '{}' feature is disabled for this workspace; Kestra plugin tasks run in \
                 containers, so a platform admin must enable it (as well as '{FEATURE_KEY}') \
                 before a plugin task can run",
                crate::scripts::FEATURE_KEY
            ),
            GateRefusal::NoRunner => write!(
                f,
                "no container runner is configured on this egret (workflow.container_runner \
                 is none), so nothing would run a Kestra plugin task"
            ),
            GateRefusal::TypeNotAllowed { fqcn } => write!(
                f,
                "plugin type '{fqcn}' is not in the 'allowed-types' of the '{CONFIG_TYPE}' entry \
                 of workflow.plugin_configurations"
            ),
        }
    }
}

impl std::error::Error for GateRefusal {}

/// **The gate.** Pure; see the module doc for the rules and the two callers.
pub fn check(input: &GateInput<'_>) -> Result<(), GateRefusal> {
    if !input.feature_enabled {
        return Err(GateRefusal::FeatureDisabled);
    }
    if !input.containers_enabled {
        return Err(GateRefusal::ContainersDisabled);
    }
    if input.runner.is_none() {
        return Err(GateRefusal::NoRunner);
    }
    match input.plugin_configurations.allowed_types() {
        None => Ok(()),
        Some(patterns) if patterns.iter().any(|p| glob_match(p, input.fqcn)) => Ok(()),
        Some(_) => Err(GateRefusal::TypeNotAllowed {
            fqcn: input.fqcn.to_string(),
        }),
    }
}
