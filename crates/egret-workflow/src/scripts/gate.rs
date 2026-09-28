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
//! 3. the image must be a well-formed reference ([`image_ref_problem`]): it
//!    starts with an ASCII letter or digit (or `[`, an IPv6 host) and holds
//!    only the characters an image reference can hold -- so no leading `-`,
//!    no whitespace, no control character, not empty. **This is a security rule, not tidiness** (final
//!    review I1): the Docker runner passes the image as one argv slot of
//!    `docker create`, which parses flags up to the first positional
//!    argument, so `--privileged` as an image was a FLAG -- and it matched
//!    `*`. The runner also puts `--` before the image (defence in depth); the
//!    Kubernetes runner writes the image into a struct field, where a
//!    flag-shaped string is only an unpullable name;
//! 4. the image, normalised by [`normalise_image_ref`], must match one of the
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
    /// The image is not a well-formed image reference ([`image_ref_problem`]).
    MalformedImage {
        image: String,
        problem: &'static str,
    },
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
            GateRefusal::MalformedImage { .. } => "script_image_malformed",
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
            GateRefusal::MalformedImage { image, problem } => write!(
                f,
                "image {image:?} is not a well-formed image reference: {problem}"
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
    if let Some(problem) = image_ref_problem(input.image) {
        return Err(GateRefusal::MalformedImage {
            image: input.image.to_string(),
            problem,
        });
    }
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

/// Why `image` is not a well-formed image reference, or `None` if it is.
///
/// The shape of the distribution reference grammar
/// (`distribution/reference`), not all of it: a reference starts with a
/// domain or path component, both of which start with a letter or digit (or
/// a `[` opening an IPv6 host), and
/// every character in it is one of `[A-Za-z0-9._:/@+-]` (plus `[`/`]` around
/// an IPv6 registry host). Anything else -- a leading `-` (a `docker create`
/// FLAG, final review I1), whitespace, a control character, the empty string
/// -- cannot name an image, so it is refused rather than matched: a pattern
/// such as `*` must never be what admits it.
pub fn image_ref_problem(image: &str) -> Option<&'static str> {
    let Some(first) = image.chars().next() else {
        return Some("it is empty");
    };
    if first == '-' {
        return Some("it starts with '-', which a container CLI would read as a flag");
    }
    // `[` opens an IPv6 registry host (`[::1]:5000/img`).
    if !(first.is_ascii_alphanumeric() || first == '[') {
        return Some("it must start with an ASCII letter or digit");
    }
    if image.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Some("it contains whitespace or a control character");
    }
    if !image
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._:/@+-[]".contains(c))
    {
        return Some("it contains a character no image reference holds");
    }
    None
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
    let mut unbounded = u64::MAX;
    glob_match_within(pattern, text, &mut unbounded).unwrap_or(false)
}

/// [`glob_match`] under a **step budget** shared across calls (W5a Task 5 fix
/// round 3): `None` once `budget` is spent, so a caller matching
/// author-controlled patterns against an author-sized listing has a hard
/// bound on the work, independent of the input. Allocation-free.
///
/// Because neither `*` nor `?` matches `/`, a `/` in the pattern must meet a
/// `/` in the text: the two are split on `/` and matched segment by segment
/// (the same number of segments, or no match). Each segment is the classic
/// greedy wildcard match with backtracking to the last `*` -- O(|p|·|t|)
/// steps at worst, each step one unit of `budget`. `?` consumes one whole
/// character (UTF-8 aware); literals compare bytes.
pub fn glob_match_within(pattern: &str, text: &str, budget: &mut u64) -> Option<bool> {
    let mut ps = pattern.split('/');
    let mut ts = text.split('/');
    loop {
        match (ps.next(), ts.next()) {
            (None, None) => return Some(true),
            (Some(p), Some(t)) => {
                if !segment_match(p.as_bytes(), t.as_bytes(), budget)? {
                    return Some(false);
                }
            }
            _ => return Some(false),
        }
    }
}

/// The width of the UTF-8 character starting at `b[i]`.
fn char_len(b: &[u8], i: usize) -> usize {
    match b[i] {
        x if x < 0x80 => 1,
        x if x >= 0xF0 => 4,
        x if x >= 0xE0 => 3,
        _ => 2,
    }
    .min(b.len() - i)
}

fn segment_match(p: &[u8], t: &[u8], budget: &mut u64) -> Option<bool> {
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ti < t.len() {
        *budget = budget.checked_sub(1)?;
        if pi < p.len() && p[pi] == b'?' {
            pi += 1;
            ti += char_len(t, ti);
        } else if pi < p.len() && p[pi] == b'*' {
            star = Some(pi);
            pi += 1;
            mark = ti;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += char_len(t, mark);
            ti = mark;
        } else {
            return Some(false);
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    Some(pi == p.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first (DP) matcher, kept as the reference the allocation-free one
    /// must agree with.
    fn reference(pattern: &str, text: &str) -> bool {
        let p: Vec<char> = pattern.chars().collect();
        let t: Vec<char> = text.chars().collect();
        let mut memo = vec![vec![false; t.len() + 1]; p.len() + 1];
        memo[p.len()][t.len()] = true;
        for pi in (0..p.len()).rev() {
            for ti in (0..=t.len()).rev() {
                memo[pi][ti] = match p[pi] {
                    '*' => memo[pi + 1][ti] || (ti < t.len() && t[ti] != '/' && memo[pi][ti + 1]),
                    '?' => ti < t.len() && t[ti] != '/' && memo[pi + 1][ti + 1],
                    c => ti < t.len() && t[ti] == c && memo[pi + 1][ti + 1],
                };
            }
        }
        memo[0][0]
    }

    /// **The allocation-free matcher agrees with the DP one** on every
    /// pattern/text pair over a small alphabet (`a`, `b`, `/`, `é` and the
    /// two wildcards), up to length 4 -- exhaustive, not sampled.
    #[test]
    fn the_greedy_matcher_agrees_with_the_reference_exhaustively() {
        fn all(alphabet: &[&str], max: usize) -> Vec<String> {
            let mut out = vec![String::new()];
            let mut frontier = vec![String::new()];
            for _ in 0..max {
                let mut next = Vec::new();
                for s in &frontier {
                    for a in alphabet {
                        next.push(format!("{s}{a}"));
                    }
                }
                out.extend(next.iter().cloned());
                frontier = next;
            }
            out
        }
        let patterns = all(&["a", "b", "/", "é", "*", "?"], 4);
        let texts = all(&["a", "b", "/", "é"], 4);
        let mut checked = 0u64;
        for p in &patterns {
            for t in &texts {
                assert_eq!(glob_match(p, t), reference(p, t), "{p:?} vs {t:?}");
                checked += 1;
            }
        }
        assert!(checked > 500_000, "{checked}");
    }

    /// The budget bounds the work and says so (`None`), rather than
    /// answering wrongly.
    #[test]
    fn the_step_budget_is_a_hard_bound() {
        let mut b = 3u64;
        assert_eq!(glob_match_within("*z", "aaaaaaaa", &mut b), None);
        let mut b = 1_000u64;
        assert_eq!(glob_match_within("*a", "bbba", &mut b), Some(true));
        assert!(b < 1_000);
    }

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
