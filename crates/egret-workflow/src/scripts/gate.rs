//! The ONE governance gate for script tasks (W5a spec §4).
//!
//! [`check`] is pure and is the only place that decides whether a script task
//! may run an image. It is called at two sites, and the W13b lesson (a draft
//! saves despite validation errors and still runs) is why there are two:
//!
//! * **at publish** (`publish::publish_gated`), for a literal image; a
//!   refusal is a named 400;
//! * **at run**, after rendering, by `worker::handlers::script::Script`. A
//!   refusal there is a FAILED task with the reason, and no runner is called,
//!   so no pod or container is created.
//!
//! An image written as a `{{ }}` expression cannot be matched before it is
//! rendered, so publish calls [`check_unrendered`] for it: the same function
//! up to, and not including, the image match. Everything that does not depend
//! on the image (the feature, the runner's allowlist existing and being
//! non-empty) is therefore refused at publish either way.
//!
//! # The rules, and why they are stricter than Kestra
//!
//! Kestra OSS has neither a per-tenant switch nor an image allowlist. egret
//! has both, by the user's decision of 2026-09-28:
//!
//! 1. the workspace feature `workflow_containers` must be ON (it is OFF when
//!    the workspace has no row for it);
//! 2. `plugin_configurations` must hold an entry for the runner's FQCN, and
//!    its `allowed-images` must be non-empty: **no entry, or an empty list,
//!    allows no image**;
//! 3. the image, normalised by [`normalise_image_ref`], must match one of the
//!    glob patterns ([`glob_match`]).

use std::fmt;

use egret_config::PluginConfigurations;

use super::{RunnerKind, FEATURE_KEY};

/// What [`check`] decides on.
#[derive(Debug, Clone, Copy)]
pub struct GateInput<'a> {
    /// Whether the workspace has `workflow_containers` ON.
    pub feature_enabled: bool,
    /// The image reference, rendered (at run) or literal (at publish).
    pub image: &'a str,
    pub runner: RunnerKind,
    pub plugin_configurations: &'a PluginConfigurations,
}

/// Why the gate refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateRefusal {
    /// The workspace has `workflow_containers` OFF (or no row for it).
    FeatureDisabled,
    /// `plugin_configurations` has no entry for the runner's FQCN.
    NoPluginConfiguration { runner: &'static str },
    /// The runner's entry has an empty `allowed-images`.
    EmptyAllowlist { runner: &'static str },
    /// The image matched none of the runner's patterns.
    ImageNotAllowed {
        image: String,
        matched_as: String,
        runner: &'static str,
    },
}

impl GateRefusal {
    /// A machine-checkable code, as `spec::Finding::code` carries.
    pub fn code(&self) -> &'static str {
        match self {
            GateRefusal::FeatureDisabled => "workflow_containers_disabled",
            GateRefusal::NoPluginConfiguration { .. } => "script_runner_not_configured",
            GateRefusal::EmptyAllowlist { .. } => "script_allowlist_empty",
            GateRefusal::ImageNotAllowed { .. } => "script_image_not_allowed",
        }
    }
}

impl fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateRefusal::FeatureDisabled => write!(
                f,
                "the '{FEATURE_KEY}' feature is disabled for this workspace; a platform admin \
                 must enable it before a script task can run"
            ),
            GateRefusal::NoPluginConfiguration { runner } => write!(
                f,
                "no plugin configuration for task runner '{runner}', so no image is allowed \
                 on it; a platform admin must add its 'allowed-images' to \
                 workflow.plugin_configurations"
            ),
            GateRefusal::EmptyAllowlist { runner } => write!(
                f,
                "the 'allowed-images' list for task runner '{runner}' is empty, so no image \
                 is allowed on it"
            ),
            GateRefusal::ImageNotAllowed {
                image,
                matched_as,
                runner,
            } => write!(
                f,
                "image '{image}' (matched as '{matched_as}') is not in the 'allowed-images' \
                 of task runner '{runner}'"
            ),
        }
    }
}

impl std::error::Error for GateRefusal {}

/// **The gate.** Pure; see the module doc for the rules and the two callers.
pub fn check(input: &GateInput) -> Result<(), GateRefusal> {
    let allowed = allowlist(
        input.feature_enabled,
        input.runner,
        input.plugin_configurations,
    )?;
    let matched_as = normalise_image_ref(input.image);
    if allowed.iter().any(|p| glob_match(p, &matched_as)) {
        Ok(())
    } else {
        Err(GateRefusal::ImageNotAllowed {
            image: input.image.to_string(),
            matched_as,
            runner: input.runner.fqcn(),
        })
    }
}

/// [`check`] for an image not yet rendered (a `{{ }}` expression at publish):
/// every rule that does not depend on the image, by the same code. The image
/// itself is checked by [`check`] at run time, after rendering.
pub fn check_unrendered(
    feature_enabled: bool,
    runner: RunnerKind,
    plugin_configurations: &PluginConfigurations,
) -> Result<(), GateRefusal> {
    allowlist(feature_enabled, runner, plugin_configurations).map(|_| ())
}

/// Rules 1 and 2: the non-empty pattern list the image must match.
fn allowlist(
    feature_enabled: bool,
    runner: RunnerKind,
    plugin_configurations: &PluginConfigurations,
) -> Result<Vec<&str>, GateRefusal> {
    if !feature_enabled {
        return Err(GateRefusal::FeatureDisabled);
    }
    let runner = runner.fqcn();
    let allowed = plugin_configurations
        .allowed_images(runner)
        .ok_or(GateRefusal::NoPluginConfiguration { runner })?;
    if allowed.is_empty() {
        return Err(GateRefusal::EmptyAllowlist { runner });
    }
    Ok(allowed)
}

/// True if `image` is a `{{ }}`/`{% %}` expression that must be rendered
/// before it can be gated.
pub fn is_unrendered(image: &str) -> bool {
    image.contains("{{") || image.contains("{%")
}

/// The reference the allowlist is matched against (W5a spec §4, Task 1's
/// `image_reference_normalisation`), exactly:
///
/// * nothing is added or stripped: no `docker.io/`, no `library/`;
/// * a digest reference (`name@sha256:…`) is matched as written;
/// * otherwise the tag is the part after a `:` that follows the last `/`
///   (so the port in `host:5000/img` is not a tag), and a reference with no
///   tag gets `:latest` (measured: `containerImage: python` pulled
///   `python:latest`).
pub fn normalise_image_ref(image: &str) -> String {
    if image.contains('@') {
        return image.to_string();
    }
    let name = image.rsplit('/').next().unwrap_or(image);
    if name.contains(':') {
        image.to_string()
    } else {
        format!("{image}:latest")
    }
}

/// Glob match over the whole of `text`, **within path segments**:
///
/// * `*` matches any run of characters (including none) **except `/`**;
/// * `?` matches exactly one character **other than `/`**;
/// * anything else matches itself. There is no `**`: two stars are two `*`,
///   and neither crosses a `/`.
///
/// Why `/` is the boundary: an image reference names a registry host only
/// through its first `/`-separated segment (`host[:port]/repo…`). If a star
/// could cross `/`, a tag glob such as `python:3*` would also match
/// `python:3000/x/img`, a reference to the registry host `python` on port
/// 3000, and `python*` would match `python.attacker.io/x`. With the boundary,
/// the segments before the last `/` (the host included) can only be matched
/// by literal text or by a star that stays inside one segment, and a pattern
/// with no `/` can only match a reference with no `/`, i.e. a Docker Hub
/// official image. To allow everything under a registry, write one star per
/// level: `registry.company.com/*` (one level) or
/// `registry.company.com/*/*` (two).
///
/// Patterns are not normalised: write `ubuntu:latest` or `ubuntu:*`, not
/// `ubuntu`, since the image side always carries a tag or a digest.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    // memo[pi][ti]: does p[pi..] match t[ti..]? Filled back to front.
    let mut memo = vec![vec![false; t.len() + 1]; p.len() + 1];
    memo[p.len()][t.len()] = true;
    for pi in (0..p.len()).rev() {
        for ti in (0..=t.len()).rev() {
            memo[pi][ti] = match p[pi] {
                // Match nothing, or one non-`/` character and stay on `*`.
                '*' => memo[pi + 1][ti] || (ti < t.len() && t[ti] != '/' && memo[pi][ti + 1]),
                '?' => ti < t.len() && t[ti] != '/' && memo[pi + 1][ti + 1],
                c => ti < t.len() && t[ti] == c && memo[pi + 1][ti + 1],
            };
        }
    }
    memo[0][0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_basics() {
        assert!(glob_match("python:*", "python:3.13-slim"));
        assert!(glob_match("*", "anything:at-all"));
        assert!(!glob_match("*", "any/thing"));
        assert!(!glob_match("?", "/"));
        assert!(glob_match("reg.io/*/*", "reg.io/team/img:1"));
        assert!(!glob_match("reg.io/**", "reg.io/team/img:1"), "no `**`");
        assert!(glob_match("python:3.1?-slim", "python:3.13-slim"));
        assert!(!glob_match("python:*", "pythonx:1"));
        assert!(!glob_match("python", "python:latest"));
        assert!(glob_match(
            "registry.local:5000/team/*",
            "registry.local:5000/team/img:1.2"
        ));
        assert!(!glob_match("a*b", "a"));
        assert!(glob_match("a*b*c", "a-b-b-c"));
        assert!(!glob_match("", "x"));
        assert!(glob_match("", ""));
    }
}
