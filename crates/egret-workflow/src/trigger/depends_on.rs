// kestra: core/src/main/java/io/kestra/plugin/core/trigger/Flow.java
// kestra: core/src/main/java/io/kestra/core/models/triggers/multipleflows/MultipleCondition.java
// kestra: core/src/main/java/io/kestra/core/models/triggers/multipleflows/MultipleConditionWindow.java
// kestra: executor/src/main/java/io/kestra/executor/FlowTriggerService.java
// kestra: core/src/main/java/io/kestra/core/utils/Hashing.java

//! `dependsOn` evaluation, pure: which dependency an execution meets
//! ([`dependency_matches`]), how a window's results accumulate ([`merge`]),
//! and whether the mode is met ([`satisfied`]).
//!
//! # The condition key is a content hash, not a position
//!
//! `Flow.DependsOnMultipleCondition.getConditions()` (Flow.java:470-478 at
//! `96893b589`) keys each dependency by `hash(dependency)` (Flow.java:497-505):
//!
//! ```text
//! Hashing.hashToString(namespace, "_", flowId, when.toString(),
//!                      states' names sorted and joined by ",",
//!                      labels sorted by key as "k:v" joined by ",")
//! ```
//!
//! `hashToString(String...)` is `StringUtils.join` (no separator, a `null` is
//! `""`) fed to Guava's `murmur3_128` (seed 0), printed as the lowercase hex
//! of its 16 bytes. **Measured:** [`condition_key`] reproduces the three keys
//! the artifact stored for `p1`/`p2`/`p3` (`w13b-windows.json`,
//! `method.results_keys`), and the pinned image's own bytecode for `hash`
//! (`javap -c` of `Flow$DependsOnMultipleCondition` in
//! `kestra/kestra:v2.0.0-rc13`, commit `fde6740`) is the checkout's, term for
//! term.
//!
//! **The measurement corrects the reading in one place: an undeclared `when`
//! hashes as `null`, not as `"true"`.** `p1`'s stored key is
//! `murmur3("w13b_p1SUCCESS")`, not `murmur3("w13b_p1trueSUCCESS")`: the
//! `@Builder.Default when = Property.ofValue("true")` only applies through
//! the builder, and Jackson builds a YAML-declared `Dependency` through its
//! all-args constructor, so an absent `when` is `null` (and
//! `DependencyCondition.test`'s `render(null)` ... `orElse("true")` is what
//! makes it truthy). `models::Dependency` fills an absent `when` with
//! `"true"`, so [`condition_key`] hashes a `"true"` source as that null.
//! **Chosen, and it is a divergence at exactly one input:** an EXPLICIT
//! `when: "true"`, which Kestra hashes with `"true"` and egret's model cannot
//! tell from an absent one. The key differs; which executions match does not.
//!
//! Consequences of the key, each derived from the hash's inputs:
//!
//! - reordering `dependsOn` keeps every stored result attached to its
//!   dependency; editing one dependency orphans only its own key, and
//!   [`satisfied`] counts only keys that are CURRENT conditions
//!   (`MultipleCondition.test` iterates `getConditions().keySet()`), so an
//!   orphan never counts;
//! - two dependencies with the same content have the same key, and
//!   `Collectors.toMap` then throws `IllegalStateException` outside any
//!   `try` in `processMultipleConditionWindow`: Kestra never evaluates such a
//!   trigger. egret refuses it at save instead (`dependency_duplicate`,
//!   chosen) — and because the concatenation has no separator between
//!   `flowId` and `when`, and `"_"` can occur in both halves of the pair,
//!   two DIFFERENT dependencies can collide too; they are refused the same way.
//!
//! # One refusal list for save and run time
//!
//! Drafts save despite validation errors, and `scheduler::sync` gives any
//! parseable draft a listener row, so everything `spec::validate` refuses
//! about a `dependsOn` trigger must also stop the processor.
//! [`depends_on_refusals`] is what `spec::validate` reports; its parts are
//! what the run-time functions call: [`super::window::window_refusals`] via
//! `TimeWindow::from_model`, [`mode_refusal`] via [`satisfied`], and the
//! duplicate check via [`condition_keys`].

use std::collections::BTreeMap;
use std::fmt;

use crate::executor::is_truthy;
use crate::models::{Dependency, FlowTrigger, FlowTriggerMode, Label, State};
use crate::spec::validate::{is_valid_flow_id, is_valid_namespace};

use super::flow::WhenVerdict;
use super::window::window_refusals;

/// The execution a dependency is tested against — the fields of Kestra's
/// `ConditionContext.getExecution()` that `DependencyCondition.test` reads.
// No `Eq`: `models::Label` derives `PartialEq` only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DepSource<'a> {
    pub namespace: &'a str,
    pub flow_id: &'a str,
    /// `execution.getState().getCurrent()`.
    pub state: State,
    pub labels: &'a [Label],
}

/// `Flow.DependsOnMultipleCondition.hash(dependency)`. See the module doc.
pub fn condition_key(dep: &Dependency) -> String {
    // `states.stream().map(Enum::name).sorted()`.
    let mut states: Vec<String> = dep.states.iter().flatten().map(state_name).collect();
    states.sort();
    // `labels.entrySet().stream().sorted(comparingByKey()).map(k + ":" + v)`.
    // Keys are unique, so sorting the pairs sorts by key.
    let mut labels: Vec<(&str, String)> = dep
        .labels
        .iter()
        .flatten()
        .map(|(k, v)| (k.as_str(), java_string(v)))
        .collect();
    labels.sort();
    let labels: Vec<String> = labels.iter().map(|(k, v)| format!("{k}:{v}")).collect();

    // `StringUtils.join(values)`: no separator, `null` is "".
    // `when.toString()` is the Property's expression, its source -- except
    // that an undeclared `when` is NULL on the artifact, not the builder's
    // "true" (see the module doc), and `models::Dependency` supplies "true"
    // for an absent one, so "true" hashes as the null it stands for.
    let when = dep
        .when
        .as_ref()
        .map(|w| w.source())
        .filter(|w| *w != "true")
        .unwrap_or("");
    let joined = [
        dep.namespace.as_deref().unwrap_or(""),
        "_",
        dep.flow_id.as_deref().unwrap_or(""),
        when,
        &states.join(","),
        &labels.join(","),
    ]
    .concat();

    // Guava `murmur3_128()` (x64, seed 0); `HashCode.toString()` is the hex
    // of its bytes, which are h1 then h2, each little-endian: the crate's
    // `(h2 << 64) | h1` as `u128::to_le_bytes`.
    let hash = murmur3::murmur3_x64_128(&mut joined.as_bytes(), 0)
        .expect("reading from an in-memory byte slice cannot fail");
    hash.to_le_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `Enum.name()` of a `State.Type`: its wire spelling.
fn state_name(state: &State) -> String {
    match serde_json::to_value(state) {
        Ok(serde_json::Value::String(name)) => name,
        other => unreachable!("State serialises as a string, got {other:?}"),
    }
}

/// `String.valueOf(Object)` for a YAML-parsed label value, as Jackson would
/// have typed it: a string is itself; an integer its digits (`Integer`/`Long`
/// /`BigInteger`); a float `Double.toString` (`1.5`, `2.0`, `1.0E10`); a
/// boolean `true`/`false`; `null` `"null"`; a list `[a, b]`; a map `{k=v}`.
///
/// **Chosen, not measured:** Task 1 declared no dependency labels. Strings
/// are what labels hold in practice; the other arms follow the JDK's
/// `toString`s and are what the hash and the label match both read, so the
/// two cannot disagree with each other.
fn java_string(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            let text = n.to_string();
            if !text.contains(['.', 'e', 'E']) {
                return text;
            }
            match n.as_f64() {
                Some(f) if f.is_finite() && (f == 0.0 || (1e-3..1e7).contains(&f.abs())) => {
                    let plain = format!("{f}");
                    if plain.contains('.') {
                        plain
                    } else {
                        format!("{plain}.0")
                    }
                }
                Some(f) if f.is_finite() => {
                    let sci = format!("{f:e}");
                    let (mantissa, exponent) = sci.split_once('e').unwrap_or((&sci, "0"));
                    if mantissa.contains('.') {
                        format!("{mantissa}E{exponent}")
                    } else {
                        format!("{mantissa}.0E{exponent}")
                    }
                }
                _ => text,
            }
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(java_string).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{k}={}", java_string(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Two dependencies with the same [`condition_key`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateCondition {
    pub first: usize,
    pub second: usize,
}

impl fmt::Display for DuplicateCondition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "dependsOn[{}] has the same condition key as dependsOn[{}]",
            self.second, self.first
        )
    }
}

impl std::error::Error for DuplicateCondition {}

/// Every dependency's key, in declaration order, or the first collision.
pub fn condition_keys(deps: &[Dependency]) -> Result<Vec<String>, DuplicateCondition> {
    let keys: Vec<String> = deps.iter().map(condition_key).collect();
    match duplicates(&keys).into_iter().next() {
        Some(dup) => Err(dup),
        None => Ok(keys),
    }
}

/// Every index whose key an earlier dependency already has.
fn duplicates(keys: &[String]) -> Vec<DuplicateCondition> {
    keys.iter()
        .enumerate()
        .filter_map(|(second, key)| {
            keys[..second]
                .iter()
                .position(|k| k == key)
                .map(|first| DuplicateCondition { first, second })
        })
        .collect()
}

/// `Flow.DependencyCondition.test`, with `when` rendered by the caller.
///
/// In Kestra's order, each an early `false`: `namespace` then `flowId`,
/// `states` (a `null` list accepts any state), `labels` (each declared `(key, String.valueOf(value))` must be on
/// the execution), then `when`. Not modelled: the `SIMULATED_EXECUTION`
/// short-circuit between `flowId` and `states` — egret has no simulated
/// executions.
///
/// **A missing `namespace` or `flowId` matches NOTHING, which is a
/// divergence, chosen.** Kestra's `dependency.namespace != null && ...`
/// leaves a null one unconstrained. egret refuses that shape at save
/// (`dependency_namespace_required` / `dependency_flow_id_required`, via
/// [`depends_on_refusals`]), but a draft saves anyway and still gets a
/// listener row, so this function fails closed rather than letting a
/// half-written `- namespace: company` fire on every execution in the
/// namespace. The gate Task 4 applies ([`depends_on_refusals`] empty) makes
/// this unreachable in production; failing closed here as well means the
/// two would have to be broken together.
///
/// `when` follows `trigger::flow::evaluate`'s three-way rule: `Unrenderable`
/// is `false` (`ConditionService.isValid` catches the render exception), a
/// rendered string is `TruthUtils.isTruthy`, and `NotDeclared` is accepted
/// only when the dependency declares no `when` (`render(null)` ...
/// `orElse("true")`); a declared `when` the caller did not render is refused
/// rather than obeyed. **Callers always render `dep.when` when it is
/// `Some`:** `models::Dependency` fills an absent `when` with `"true"`, so
/// `NotDeclared` is only right for an explicit `when: null`, and passing it
/// otherwise makes the dependency silently never match.
pub fn dependency_matches(dep: &Dependency, source: &DepSource<'_>, when: &WhenVerdict) -> bool {
    // Fail closed on a missing field: see above.
    if dep.namespace.as_deref() != Some(source.namespace) {
        return false;
    }
    if dep.flow_id.as_deref() != Some(source.flow_id) {
        return false;
    }
    if dep
        .states
        .as_ref()
        .is_some_and(|states| !states.contains(&source.state))
    {
        return false;
    }
    if let Some(labels) = &dep.labels {
        let on_execution = |key: &str, value: String| {
            source
                .labels
                .iter()
                .any(|l| l.key == key && l.value == value)
        };
        if !labels.iter().all(|(k, v)| on_execution(k, java_string(v))) {
            return false;
        }
    }
    match when {
        WhenVerdict::Unrenderable => false,
        WhenVerdict::NotDeclared => dep.when.is_none(),
        WhenVerdict::Rendered(rendered) => is_truthy(rendered),
    }
}

/// `MultipleConditionWindow.with(newResult)`: the stored results plus every
/// key `now_true` holds as `true`. Only `true` is ever written, and nothing
/// already stored is removed or overwritten with `false` — a met condition
/// stays met until the window is deleted.
pub fn merge(
    results: &BTreeMap<String, bool>,
    now_true: &BTreeMap<String, bool>,
) -> BTreeMap<String, bool> {
    let mut merged = results.clone();
    for (key, met) in now_true {
        if *met {
            merged.insert(key.clone(), true);
        }
    }
    merged
}

/// `MultipleCondition.test`: is the mode met by `results`?
///
/// **Takes the current condition KEYS rather than the brief's
/// `n_conditions`, and that is Kestra's rule:** `test` counts `true` results
/// only for keys in `getConditions().keySet()`, so a result left behind by
/// an edited dependency never counts. `n` is `condition_keys.len()`.
///
/// Refused shapes never fire: `AT_LEAST` with `minSatisfied` absent, below 1
/// or above `n` ([`mode_refusal`], the same function `spec::validate`
/// reports), and no conditions at all (the window-free arm's business).
pub fn satisfied(
    mode: FlowTriggerMode,
    min: Option<i32>,
    condition_keys: &[String],
    results: &BTreeMap<String, bool>,
) -> bool {
    let n = condition_keys.len();
    if n == 0 || mode_refusal(mode, min, n).is_some() {
        return false;
    }
    let met = condition_keys
        .iter()
        .filter(|k| results.get(k.as_str()) == Some(&true))
        .count();
    match mode {
        FlowTriggerMode::All => met == n,
        FlowTriggerMode::Any => met > 0,
        // `mode_refusal` passed, so `min` is `Some(1..=n)`.
        FlowTriggerMode::AtLeast => min.is_some_and(|m| met >= m as usize),
    }
}

/// Why a `(mode, minSatisfied)` pair is refused, for `n` conditions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeRefusal {
    pub code: &'static str,
    pub message: String,
}

/// The `minSatisfied` refusals — `@Positive` (Flow.java:298) under any mode,
/// `FlowTriggerValidator`'s "must be set when mode is AT_LEAST", and egret's
/// own `AT_LEAST` above `n` (chosen: Kestra saves it and then never fires).
pub fn mode_refusal(mode: FlowTriggerMode, min: Option<i32>, n: usize) -> Option<ModeRefusal> {
    let (code, message) = match (mode, min) {
        (_, Some(m)) if m < 1 => (
            "min_satisfied_not_positive",
            format!("minSatisfied must be greater than 0, got {m}"),
        ),
        (FlowTriggerMode::AtLeast, None) => (
            "min_satisfied_required",
            "`minSatisfied` must be set when mode is AT_LEAST".to_string(),
        ),
        (FlowTriggerMode::AtLeast, Some(m)) if m as usize > n => (
            "min_satisfied_exceeds_conditions",
            format!(
                "minSatisfied is {m} but dependsOn declares only {n} condition(s), so the \
                 trigger could never fire"
            ),
        ),
        _ => return None,
    };
    Some(ModeRefusal { code, message })
}

/// Where a [`TriggerRefusal`] points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalAt {
    /// `window`, or `window.<field>` when the field is not `""`.
    Window(&'static str),
    MinSatisfied,
    /// `dependsOn[i]`.
    Dependency(usize),
    /// `dependsOn[i].<field>`.
    DependencyField(usize, &'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerRefusal {
    pub code: &'static str,
    pub at: RefusalAt,
    pub message: String,
}

/// Every refusal of a flow trigger's `dependsOn` machinery — each
/// dependency's `namespace`/`flowId`, `minSatisfied`, `window`, duplicate
/// conditions — in the order `spec::validate` reports them. See the module
/// doc, *One refusal list*.
///
/// **This is the run-time gate as well:** a listener whose trigger has any
/// refusal here must be skipped (with a named reason), never evaluated.
/// `spec::validate` reports nothing about a flow trigger that is not in this
/// list (`tests::save_and_run_time_refuse_the_same_shapes` asserts it).
pub fn depends_on_refusals(ft: &FlowTrigger) -> Vec<TriggerRefusal> {
    let mut out = Vec::new();
    // W1's `(namespace, flowId)` checks (whole-branch review finding 2): the
    // SAME grammar `Subflow` is held to: Kestra's own namespace and flow id
    // patterns. (Introduced when publish encoded the pair into one
    // `"{namespace}/{flowId}"` string; since debt §5.1, 2026-10-19, it looks
    // the pair up as two columns.) Moved here
    // from `spec::validate` unchanged (W13b Task 3 fix round 1) so a draft
    // with a half-written dependency is refused at run time too.
    for (i, dep) in ft.depends_on.iter().enumerate() {
        match &dep.namespace {
            None => out.push(TriggerRefusal {
                code: "dependency_namespace_required",
                at: RefusalAt::DependencyField(i, "namespace"),
                message: format!("dependsOn[{i}] is missing a required namespace"),
            }),
            Some(ns) if !is_valid_namespace(ns) => out.push(TriggerRefusal {
                code: "dependency_namespace_invalid",
                at: RefusalAt::DependencyField(i, "namespace"),
                message: format!(
                    "dependsOn[{i}] namespace '{ns}' must match ^[a-z0-9][a-z0-9._-]* and be \
                     1-150 characters long -- Kestra's own namespace pattern, the same grammar \
                     `namespace:` itself uses"
                ),
            }),
            Some(_) => {}
        }
        match &dep.flow_id {
            None => out.push(TriggerRefusal {
                code: "dependency_flow_id_required",
                at: RefusalAt::DependencyField(i, "flowId"),
                message: format!("dependsOn[{i}] is missing a required flowId"),
            }),
            Some(fid) if !is_valid_flow_id(fid) => out.push(TriggerRefusal {
                code: "dependency_flow_id_invalid",
                at: RefusalAt::DependencyField(i, "flowId"),
                message: format!(
                    "dependsOn[{i}] flowId '{fid}' must match ^[a-zA-Z0-9][a-zA-Z0-9._-]* and \
                     be 1-100 characters long -- Kestra's own flow id pattern"
                ),
            }),
            Some(_) => {}
        }
    }
    if let Some(r) = mode_refusal(ft.mode, ft.min_satisfied, ft.depends_on.len()) {
        out.push(TriggerRefusal {
            code: r.code,
            at: RefusalAt::MinSatisfied,
            message: r.message,
        });
    }
    if let Some(window) = &ft.window {
        // Kestra reads a window only through `dependsOnAsMultipleCondition()`,
        // so on the window-free arm it is inert; egret refuses rather than
        // silently ignoring a declaration.
        if ft.depends_on.is_empty() {
            out.push(TriggerRefusal {
                code: "window_without_depends_on",
                at: RefusalAt::Window(""),
                message: "window: a window is only read by a trigger with `dependsOn`; this \
                          trigger has none, so the window would be silently ignored"
                    .to_string(),
            });
        }
        out.extend(window_refusals(window).into_iter().map(|r| TriggerRefusal {
            code: r.code,
            at: RefusalAt::Window(r.field),
            message: format!("window: {}", r.message),
        }));
    }
    let keys: Vec<String> = ft.depends_on.iter().map(condition_key).collect();
    out.extend(duplicates(&keys).into_iter().map(|d| TriggerRefusal {
        code: "dependency_duplicate",
        at: RefusalAt::Dependency(d.second),
        message: format!(
            "{d}; Kestra keys a dependency's results by a hash of its content, and two \
             dependencies with one key cannot be told apart (Kestra fails to evaluate the \
             trigger at all)"
        ),
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Flow, TriggerKind};
    use crate::trigger::window::TimeWindow;
    use crate::trigger::{FlowTriggerError, WindowFreeFlowTrigger};
    use std::collections::BTreeSet;

    const FIXTURE: &str = include_str!("../../oracle-exec/w13b-windows.json");

    fn fixture() -> serde_json::Value {
        serde_json::from_str(FIXTURE).unwrap()
    }

    fn flow_trigger_of(flow_yaml: &str) -> FlowTrigger {
        let flow: Flow =
            serde_norway::from_str(flow_yaml).unwrap_or_else(|e| panic!("{flow_yaml}\n{e}"));
        match &flow.triggers[0].kind {
            TriggerKind::Flow(ft) => ft.clone(),
            other => panic!("not a flow trigger: {other:?}"),
        }
    }

    /// The listener YAML the artifact ran for `scenario`.
    fn listener_yaml(fx: &serde_json::Value, scenario: &str) -> String {
        for records in fx["raw_records"].as_object().unwrap().values() {
            for r in records.as_array().unwrap() {
                if r["scenario"] == scenario {
                    if let Some(y) = r["flow_yaml"].as_str() {
                        return y.to_string();
                    }
                }
            }
        }
        panic!("no flow_yaml for {scenario}")
    }

    fn dep(yaml: &str) -> Dependency {
        serde_norway::from_str(yaml).unwrap_or_else(|e| panic!("{yaml}: {e}"))
    }

    fn source<'a>(flow_id: &'a str, state: State, labels: &'a [Label]) -> DepSource<'a> {
        DepSource {
            namespace: "w13b",
            flow_id,
            state,
            labels,
        }
    }

    fn map(entries: &[(&str, bool)]) -> BTreeMap<String, bool> {
        entries.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn keys(ks: &[&str]) -> Vec<String> {
        ks.iter().map(|k| k.to_string()).collect()
    }

    // ------------------------------------------------------------------
    // The condition key
    // ------------------------------------------------------------------

    /// **Measured.** The artifact stored `results` under these three keys for
    /// `q2b_atl`'s `p1`, `p2`, `p3` (fixture `method.results_keys`).
    #[test]
    fn condition_key_reproduces_the_artifacts_stored_keys() {
        let fx = fixture();
        let ft = flow_trigger_of(&listener_yaml(&fx, "q2_atleast2_nowindow"));
        let measured: BTreeMap<String, String> = fx["method"]["results_keys"]
            .as_str()
            .map(|s| {
                let json = &s[s.find('{').unwrap()..];
                serde_json::from_str(json).unwrap()
            })
            .unwrap();
        assert_eq!(measured.len(), 3);
        let computed: BTreeMap<String, String> = ft
            .depends_on
            .iter()
            .map(|d| (condition_key(d), d.flow_id.clone().unwrap()))
            .collect();
        assert_eq!(computed, measured);
    }

    #[test]
    fn condition_key_is_content_not_position() {
        let a = dep("namespace: n\nflowId: a\nstates: [SUCCESS, FAILED]\nlabels: {x: 1, a: b}");
        let reordered =
            dep("namespace: n\nflowId: a\nstates: [FAILED, SUCCESS]\nlabels: {a: b, x: 1}");
        assert_eq!(condition_key(&a), condition_key(&reordered));

        // An absent `when` and `when: "true"` are one key (the model cannot
        // tell them apart; see the module doc).
        assert_eq!(
            condition_key(&dep("namespace: n\nflowId: a")),
            condition_key(&dep("namespace: n\nflowId: a\nwhen: \"true\""))
        );

        // Each hashed field moves the key.
        for other in [
            "namespace: m\nflowId: a\nstates: [SUCCESS, FAILED]\nlabels: {x: 1, a: b}",
            "namespace: n\nflowId: b\nstates: [SUCCESS, FAILED]\nlabels: {x: 1, a: b}",
            "namespace: n\nflowId: a\nstates: [SUCCESS]\nlabels: {x: 1, a: b}",
            "namespace: n\nflowId: a\nstates: [SUCCESS, FAILED]\nlabels: {x: 2, a: b}",
            "namespace: n\nflowId: a\nstates: [SUCCESS, FAILED]\nlabels: {x: 1, a: b}\nwhen: \"{{ x }}\"",
        ] {
            assert_ne!(condition_key(&a), condition_key(&dep(other)), "{other}");
        }

        // Position does not: swapping two dependencies swaps their keys.
        let p = dep("namespace: n\nflowId: p");
        let q = dep("namespace: n\nflowId: q");
        assert_eq!(
            condition_keys(&[p.clone(), q.clone()]).unwrap(),
            condition_keys(&[q, p])
                .unwrap()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_same_condition_twice_is_a_duplicate_and_so_is_a_separator_collision() {
        let p = dep("namespace: n\nflowId: p");
        assert_eq!(
            condition_keys(&[p.clone(), dep("namespace: n\nflowId: q"), p]),
            Err(DuplicateCondition {
                first: 0,
                second: 2
            })
        );
        // `"a" + "_" + "b_c"` == `"a_b" + "_" + "c"`.
        assert_eq!(
            condition_keys(&[
                dep("namespace: a\nflowId: b_c"),
                dep("namespace: a_b\nflowId: c")
            ]),
            Err(DuplicateCondition {
                first: 0,
                second: 1
            })
        );
    }

    // ------------------------------------------------------------------
    // dependency_matches
    // ------------------------------------------------------------------

    #[test]
    fn a_dependency_matches_on_namespace_flow_state_labels_and_when() {
        let d = dep("namespace: w13b\nflowId: p1\nstates: [SUCCESS]");
        let yes = WhenVerdict::Rendered("true".into());
        assert!(dependency_matches(
            &d,
            &source("p1", State::Success, &[]),
            &yes
        ));
        assert!(!dependency_matches(
            &d,
            &source("p2", State::Success, &[]),
            &yes
        ));
        assert!(!dependency_matches(
            &d,
            &source("p1", State::Failed, &[]),
            &yes
        ));
        let other_ns = DepSource {
            namespace: "elsewhere",
            ..source("p1", State::Success, &[])
        };
        assert!(!dependency_matches(&d, &other_ns, &yes));

        // `states` absent: any state (`dependency.states != null && ...`).
        let any_state = dep("namespace: w13b\nflowId: p1");
        assert!(dependency_matches(
            &any_state,
            &source("p1", State::Killed, &[]),
            &yes
        ));
        // namespace/flowId absent: matches NOTHING (fail closed; refused at
        // save as `dependency_*_required`). Each source below matches every
        // other field of its dependency, so only the missing one refuses.
        let no_namespace = dep("flowId: p1\nstates: [FAILED]");
        assert!(!dependency_matches(
            &no_namespace,
            &source("p1", State::Failed, &[]),
            &yes
        ));
        let no_flow_id = dep("namespace: w13b\nstates: [FAILED]");
        assert!(!dependency_matches(
            &no_flow_id,
            &source("p1", State::Failed, &[]),
            &yes
        ));
        let only_state = dep("states: [FAILED]");
        assert!(!dependency_matches(
            &only_state,
            &source("zz", State::Failed, &[]),
            &yes
        ));
    }

    #[test]
    fn declared_labels_must_all_be_on_the_execution_as_strings() {
        let d = dep("namespace: w13b\nflowId: p1\nlabels: {team: data, tier: 1}");
        let yes = WhenVerdict::Rendered("true".into());
        let label = |k: &str, v: &str| Label {
            key: k.into(),
            value: v.into(),
        };
        let both = [label("team", "data"), label("tier", "1"), label("x", "y")];
        assert!(dependency_matches(
            &d,
            &source("p1", State::Success, &both),
            &yes
        ));
        let one = [label("team", "data")];
        assert!(!dependency_matches(
            &d,
            &source("p1", State::Success, &one),
            &yes
        ));
        let wrong = [label("team", "data"), label("tier", "2")];
        assert!(!dependency_matches(
            &d,
            &source("p1", State::Success, &wrong),
            &yes
        ));
    }

    #[test]
    fn when_is_truthy_tested_and_a_caller_that_skips_it_is_refused() {
        let d = dep("namespace: w13b\nflowId: p1\nwhen: \"{{ x }}\"");
        let s = source("p1", State::Success, &[]);
        assert!(dependency_matches(
            &d,
            &s,
            &WhenVerdict::Rendered("yes".into())
        ));
        for falsy in ["false", "0", "-0", "", "  false "] {
            assert!(
                !dependency_matches(&d, &s, &WhenVerdict::Rendered(falsy.into())),
                "{falsy:?}"
            );
        }
        // `ConditionService.isValid` catches the render exception: false.
        assert!(!dependency_matches(&d, &s, &WhenVerdict::Unrenderable));
        // A declared `when` the caller did not render: refused, not obeyed.
        assert!(!dependency_matches(&d, &s, &WhenVerdict::NotDeclared));
        // `when: null` declares none: `render(null)...orElse("true")`.
        let none = dep("namespace: w13b\nflowId: p1\nwhen: null");
        assert!(dependency_matches(&none, &s, &WhenVerdict::NotDeclared));
        // `when` is only rendered for a structural match; a mismatch is false
        // whatever the verdict.
        assert!(!dependency_matches(
            &d,
            &source("p2", State::Success, &[]),
            &WhenVerdict::Rendered("true".into())
        ));
    }

    // ------------------------------------------------------------------
    // merge
    // ------------------------------------------------------------------

    #[test]
    fn merge_writes_only_true_and_never_unsets() {
        let stored = map(&[("a", true)]);
        // `a` false now: still true (sticky). `b` false: not written.
        assert_eq!(
            merge(&stored, &map(&[("a", false), ("b", false)])),
            map(&[("a", true)])
        );
        assert_eq!(
            merge(&stored, &map(&[("a", false), ("b", true)])),
            map(&[("a", true), ("b", true)])
        );
        assert_eq!(merge(&BTreeMap::new(), &BTreeMap::new()), BTreeMap::new());
    }

    // ------------------------------------------------------------------
    // satisfied
    // ------------------------------------------------------------------

    #[test]
    fn modes_all_any_and_at_least() {
        use FlowTriggerMode::*;
        let ks = keys(&["a", "b", "c"]);
        let none = BTreeMap::new();
        let one = map(&[("a", true)]);
        let two = map(&[("a", true), ("c", true)]);
        let all = map(&[("a", true), ("b", true), ("c", true)]);

        assert!(!satisfied(All, None, &ks, &none));
        assert!(!satisfied(All, None, &ks, &two));
        assert!(satisfied(All, None, &ks, &all));

        assert!(!satisfied(Any, None, &ks, &none));
        assert!(satisfied(Any, None, &ks, &one));

        assert!(!satisfied(AtLeast, Some(2), &ks, &one));
        assert!(satisfied(AtLeast, Some(2), &ks, &two));
        assert!(satisfied(AtLeast, Some(2), &ks, &all));

        // A stored `false` is not a met condition.
        assert!(!satisfied(Any, None, &ks, &map(&[("a", false)])));
    }

    #[test]
    fn a_result_under_a_key_that_is_no_longer_a_condition_does_not_count() {
        use FlowTriggerMode::*;
        let ks = keys(&["a", "b"]);
        let stale = map(&[("a", true), ("gone", true)]);
        assert!(!satisfied(All, None, &ks, &stale));
        assert!(!satisfied(AtLeast, Some(2), &ks, &stale));
        assert!(!satisfied(Any, None, &ks, &map(&[("gone", true)])));
    }

    /// Refused at save (Task 2) and never satisfied at run time: `AT_LEAST`
    /// above `n`, with none, or below 1; and no conditions at all.
    #[test]
    fn refused_modes_are_never_satisfied() {
        use FlowTriggerMode::*;
        let ks = keys(&["a", "b"]);
        let all = map(&[("a", true), ("b", true)]);
        for min in [Some(3), None, Some(0), Some(-1)] {
            assert!(mode_refusal(AtLeast, min, 2).is_some(), "{min:?}");
            assert!(!satisfied(AtLeast, min, &ks, &all), "{min:?}");
        }
        assert!(mode_refusal(AtLeast, Some(2), 2).is_none());
        // Below 1 is refused under every mode, so ALL/ANY never fire either.
        for mode in [All, Any] {
            assert!(!satisfied(mode, Some(0), &ks, &all), "{mode:?}");
            assert!(
                satisfied(mode, Some(3), &ks, &all),
                "{mode:?}: an oversized value is inert"
            );
        }
        for mode in [All, Any, AtLeast] {
            assert!(
                !satisfied(mode, Some(1), &[], &all),
                "{mode:?} with no conditions"
            );
        }
    }

    /// **The measured mode sequences, replayed.** The six Task 1 scenarios
    /// whose window never expired (so D1's lifecycle divergence cannot
    /// reach them): each event is matched against the listener's own
    /// `dependsOn`, merged, tested, and the window deleted on fire (D1: under
    /// every mode). Asserted per step: fired or not, and the met set.
    #[test]
    fn the_measured_mode_sequences_replay() {
        let fx = fixture();
        let scenarios = [
            "q1_all_nowindow",
            "q1_all_window_every_PT1H_fireOnce_default",
            "q2_any_nowindow",
            "q2_atleast2_nowindow",
            "q2_any_window_every_PT1H_fireOnce_default",
            "q2_atleast2_window_every_PT1H_fireOnce_default",
        ];
        let mut steps_checked = 0;
        let mut fires = 0;
        for scenario in scenarios {
            let ft = flow_trigger_of(&listener_yaml(&fx, scenario));
            let ks = condition_keys(&ft.depends_on).unwrap();
            let name_of: BTreeMap<&str, &str> = ks
                .iter()
                .map(String::as_str)
                .zip(ft.depends_on.iter().map(|d| d.flow_id.as_deref().unwrap()))
                .collect();
            let seq = fx["sequences"]
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["scenario"] == scenario)
                .unwrap();

            let mut results: BTreeMap<String, bool> = BTreeMap::new();
            for step in seq["steps"].as_array().unwrap() {
                let Some(producer) = step["producer"].as_str() else {
                    continue;
                };
                let s = source(producer, State::Success, &[]);
                let now_true: BTreeMap<String, bool> = ft
                    .depends_on
                    .iter()
                    .zip(&ks)
                    .map(|(d, k)| {
                        (
                            k.clone(),
                            dependency_matches(d, &s, &WhenVerdict::Rendered("true".into())),
                        )
                    })
                    .collect();
                let fired = if now_true.values().any(|v| *v) {
                    results = merge(&results, &now_true);
                    let f = satisfied(ft.mode, ft.min_satisfied, &ks, &results);
                    if f {
                        results.clear();
                    }
                    f
                } else {
                    false // D1: an unrelated execution opens nothing.
                };
                let what = format!("{scenario} / {}", step["step"]);
                assert_eq!(fired, step["fired"].as_u64().unwrap() == 1, "{what}: fired");
                if let Some(met) = step["window_after"]["met"].as_array() {
                    let expected: BTreeSet<&str> =
                        met.iter().map(|m| m.as_str().unwrap()).collect();
                    let actual: BTreeSet<&str> = results
                        .iter()
                        .filter(|(_, v)| **v)
                        .map(|(k, _)| name_of[k.as_str()])
                        .collect();
                    assert_eq!(actual, expected, "{what}: met");
                }
                steps_checked += 1;
                fires += usize::from(fired);
            }
        }
        assert_eq!((steps_checked, fires), (25, 14));
    }

    // ------------------------------------------------------------------
    // Save and run time agree
    // ------------------------------------------------------------------

    /// **The drift check Task 2's review asked for.** For every shape
    /// `spec::validate` refuses about a `dependsOn` trigger, the run-time
    /// functions refuse it too, with the same code: `TimeWindow::from_model`
    /// for window codes, `mode_refusal` (hence `satisfied`) for
    /// `minSatisfied` codes, `condition_keys` for duplicates, and -- for a
    /// trigger with no `dependsOn` -- `WindowFreeFlowTrigger::resolve` ALONE,
    /// because that is the only gate the window-free arm has (final review
    /// I1: `window` and the `mode`/`minSatisfied` shapes). And for the
    /// accepted shapes, none of them refuses.
    #[test]
    fn save_and_run_time_refuse_the_same_shapes() {
        const DEPS2: &str = "    dependsOn:\n      - {namespace: a.b, flowId: p0}\n      - {namespace: a.b, flowId: p1}\n";
        let cases: &[(&str, &str)] = &[
            // (dependsOn + body, the one code expected, "" for none)
            (
                "    window:\n      deadline: \"09:00\"\n      from: \"08:00\"\n",
                "window_conflicting_fields",
            ),
            (
                "    window:\n      every: PT1H\n      lookback: PT2M\n",
                "window_conflicting_fields",
            ),
            (
                "    window:\n      lookback: PT2M\n      offset: PT1M\n",
                "window_conflicting_fields",
            ),
            (
                "    window:\n      from: \"08:00\"\n      to: \"09:00\"\n      offset: PT1M\n",
                "window_conflicting_fields",
            ),
            (
                "    window:\n      from: \"08:00\"\n",
                "window_daily_bound_missing",
            ),
            (
                "    window:\n      offset: PT6H\n",
                "window_offset_without_every",
            ),
            ("    window:\n      every: P1M\n", "window_duration_invalid"),
            (
                "    window:\n      every: P366DT1S\n",
                "window_duration_invalid",
            ),
            (
                "    window:\n      every: PT0S\n",
                "window_duration_not_positive",
            ),
            (
                "    window:\n      deadline: \"9am\"\n",
                "window_time_invalid",
            ),
            (
                "    window:\n      from: \"12:00\"\n      to: \"10:00\"\n",
                "window_daily_range_empty",
            ),
            (
                "    window:\n      deadline: \"00:00\"\n",
                "window_daily_range_empty",
            ),
            (
                "    window:\n      every: PT1H\n      timezone: Mars/Olympus_Mons\n",
                "window_timezone_invalid",
            ),
            (
                "    mode: AT_LEAST\n    minSatisfied: 3\n",
                "min_satisfied_exceeds_conditions",
            ),
            ("    mode: AT_LEAST\n", "min_satisfied_required"),
            (
                "    mode: ANY\n    minSatisfied: 0\n",
                "min_satisfied_not_positive",
            ),
            ("    window:\n      every: PT1H\n", ""),
            (
                "    window:\n      lookback: PT2M\n      timezone: Europe/Paris\n",
                "",
            ),
            ("    window:\n      deadline: \"05:00\"\n", ""),
            ("    mode: AT_LEAST\n    minSatisfied: 2\n", ""),
            ("", ""),
        ];
        let flow = |deps: &str, body: &str| {
            format!(
                "id: f\nnamespace: a.b\ntasks:\n  - id: t\n    type: core.log\n\
                 triggers:\n  - id: listen\n    type: io.kestra.plugin.core.trigger.Flow\n{deps}{body}"
            )
        };
        // EVERY error `spec::validate` reports about the trigger, by path --
        // not a list of codes, which is how the first version of this test
        // could not see `dependency_*` (fix round 1).
        let about_the_trigger = |path: &str| path.starts_with("triggers.listen");
        let run_time =
            |ft: &FlowTrigger, trigger: &crate::models::Trigger| -> BTreeSet<&'static str> {
                let mut codes = BTreeSet::new();
                // No `dependsOn`: the processor's window-free arm, whose ONLY
                // gate is `WindowFreeFlowTrigger::resolve` (and `sync`'s, the
                // same call). Measure that path, not a stand-in for it: the
                // final review's I1 was a stand-in (`mode_refusal` called
                // directly) agreeing while `resolve` did not.
                if ft.depends_on.is_empty() {
                    match WindowFreeFlowTrigger::resolve(trigger) {
                        Ok(_) => {}
                        Err(FlowTriggerError::Refused(c)) => codes.extend(c),
                        Err(e) => panic!("unexpected window-free refusal: {e:?}"),
                    }
                    return codes;
                }
                if let Err(e) = TimeWindow::from_model(ft.window.as_ref()) {
                    codes.extend(e.0.iter().map(|r| r.code));
                }
                if let Some(r) = mode_refusal(ft.mode, ft.min_satisfied, ft.depends_on.len()) {
                    codes.insert(r.code);
                }
                if condition_keys(&ft.depends_on).is_err() {
                    codes.insert("dependency_duplicate");
                }
                // A missing `namespace`/`flowId` fails closed in the matcher:
                // offer a source that matches every OTHER field.
                let yes = WhenVerdict::Rendered("true".into());
                for d in &ft.depends_on {
                    let s = DepSource {
                        namespace: d.namespace.as_deref().unwrap_or("a.b"),
                        flow_id: d.flow_id.as_deref().unwrap_or("p0"),
                        state: State::Success,
                        labels: &[],
                    };
                    let matches = dependency_matches(d, &s, &yes);
                    if d.namespace.is_none() && !matches {
                        codes.insert("dependency_namespace_required");
                    }
                    if d.flow_id.is_none() && !matches {
                        codes.insert("dependency_flow_id_required");
                    }
                }
                codes
            };
        // Gate-only: a grammar-invalid name is refused by the gate
        // (`depends_on_refusals`); no real source execution can carry one, so
        // the matcher has nothing to refuse.
        const GATE_ONLY: [&str; 2] = ["dependency_namespace_invalid", "dependency_flow_id_invalid"];

        let mut all: Vec<(String, &str)> = cases
            .iter()
            .map(|(body, code)| (flow(DEPS2, body), *code))
            .collect();
        // The shapes that are not window-local.
        for (deps, code) in [
            (
                "    dependsOn:\n      - {flowId: p0}\n      - {namespace: a.b, flowId: p1}\n",
                "dependency_namespace_required",
            ),
            (
                "    dependsOn:\n      - {namespace: a.b}\n      - {namespace: a.b, flowId: p1}\n",
                "dependency_flow_id_required",
            ),
            (
                "    dependsOn:\n      - {namespace: \"A/B\", flowId: p0}\n",
                "dependency_namespace_invalid",
            ),
            (
                "    dependsOn:\n      - {namespace: a.b, flowId: \"p/0\"}\n",
                "dependency_flow_id_invalid",
            ),
        ] {
            all.push((flow(deps, ""), code));
        }
        all.push((
            flow("", "    window:\n      every: PT1H\n"),
            "window_without_depends_on",
        ));
        // The window-free arm's `mode`/`minSatisfied` shapes (final review
        // I1): save refuses them with no `dependsOn` too, so `resolve` must.
        for (body, code) in [
            ("    mode: AT_LEAST\n", "min_satisfied_required"),
            (
                "    mode: AT_LEAST\n    minSatisfied: 1\n",
                "min_satisfied_exceeds_conditions",
            ),
            ("    minSatisfied: 0\n", "min_satisfied_not_positive"),
            ("    mode: ANY\n", ""),
            ("    mode: ALL\n    minSatisfied: 1\n", ""),
            ("", ""),
        ] {
            all.push((flow("", body), code));
        }
        all.push((
            flow(
                "    dependsOn:\n      - {namespace: a.b, flowId: p0}\n      - {namespace: a.b, flowId: p0}\n",
                "",
            ),
            "dependency_duplicate",
        ));

        let mut refused = 0;
        for (src, code) in &all {
            let parsed = crate::spec::parse(src).unwrap_or_else(|e| panic!("{src}\n{e:?}"));
            let saved: BTreeSet<&'static str> = crate::spec::validate(&parsed)
                .errors
                .iter()
                .filter(|f| about_the_trigger(&f.path))
                .map(|f| f.code)
                .collect();
            let trigger = &parsed.triggers[0];
            let TriggerKind::Flow(ft) = &trigger.kind else {
                unreachable!()
            };
            // The gate Task 4 applies covers everything saved says.
            let gate: BTreeSet<&'static str> =
                depends_on_refusals(ft).iter().map(|r| r.code).collect();
            assert_eq!(saved, gate, "save and the run-time gate disagree on\n{src}");
            // And each run-time function refuses its own shapes by itself.
            let at_run_time = run_time(ft, trigger);
            let expected_run_time: BTreeSet<&'static str> = saved
                .iter()
                .copied()
                .filter(|c| !GATE_ONLY.contains(c))
                .collect();
            assert_eq!(
                expected_run_time, at_run_time,
                "save and the run-time functions disagree on\n{src}"
            );
            let expected: BTreeSet<&str> = if code.is_empty() {
                BTreeSet::new()
            } else {
                refused += 1;
                [*code].into()
            };
            assert_eq!(saved, expected, "{src}");
        }
        assert_eq!(refused, 25);
    }

    /// W13b Task 5: the corpus's twenty `dependsOn` declarations, put through
    /// the run-time gate the processor applies (`depends_on_refusals`, then
    /// `TimeWindow::from_model`, then `condition_keys`) and through
    /// `spec::validate`. Eighteen pass both and would open a window; one flow
    /// is refused at save, and one does not parse. Compared here by a running
    /// program, like `trigger::flow`'s twenty-to-one split, so the coverage
    /// statement's count (`oracle-exec/README.md`, *W13b*) cannot drift.
    ///
    /// None of the twenty declares a `window:`, so every one that resolves
    /// takes the default one-day `DURATION_WINDOW`: the four window kinds are
    /// judged by Task 1's measurements and this crate's own tests, never by
    /// the corpus.
    #[test]
    fn the_corpus_depends_on_declarations_resolve_eighteen_of_twenty() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("oracle-exec/corpus");
        assert!(dir.is_dir(), "corpus not found at {}", dir.display());
        let mut files = Vec::new();
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("read corpus dir") {
                let p = e.expect("dir entry").path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "yaml" || x == "yml") {
                    files.push(p);
                }
            }
        }
        files.sort();
        assert!(files.len() > 300, "expected the 351-flow corpus");

        let mut resolved = Vec::new();
        let mut refused_at_save = Vec::new();
        let mut unparseable = Vec::new();
        let mut window_free = 0usize;
        let mut with_window = 0usize;
        for path in &files {
            let text = std::fs::read_to_string(path).expect("read flow");
            if !text.contains("io.kestra.plugin.core.trigger.Flow") {
                continue;
            }
            let flow: Flow = match serde_norway::from_str(&text) {
                Ok(flow) => flow,
                Err(e) => {
                    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
                    unparseable.push((stem, e.to_string()));
                    continue;
                }
            };
            let report = crate::spec::validate::validate(&flow);
            for t in &flow.triggers {
                let TriggerKind::Flow(ft) = &t.kind else {
                    continue;
                };
                if ft.depends_on.is_empty() {
                    window_free += 1;
                    continue;
                }
                with_window += usize::from(ft.window.is_some());
                let name = format!("{}/{}", flow.id, t.id);
                let codes: Vec<&str> = depends_on_refusals(ft).iter().map(|r| r.code).collect();
                let path_prefix = format!("triggers.{}.", t.id);
                let at_save: Vec<&str> = report
                    .errors
                    .iter()
                    .filter(|f| f.path.starts_with(&path_prefix))
                    .map(|f| f.code)
                    .collect();
                // Save and run time refuse the same shapes (the module doc,
                // *One refusal list*), declaration by declaration.
                assert_eq!(at_save, codes, "{name}: save vs run-time gate");
                let passes = codes.is_empty()
                    && TimeWindow::from_model(ft.window.as_ref()).is_ok()
                    && condition_keys(&ft.depends_on).is_ok();
                if passes {
                    resolved.push(name);
                } else {
                    refused_at_save.push((name, codes));
                }
            }
        }

        assert_eq!(window_free, 1, "the one window-free declaration");
        assert_eq!(with_window, 0, "no corpus declaration names a `window:`");
        assert_eq!(
            refused_at_save,
            vec![(
                "trigger-flow-listener-namespace-condition/listen-flow".to_string(),
                vec![
                    "dependency_flow_id_required",
                    "dependency_namespace_required",
                    "dependency_flow_id_required",
                ],
            )],
            "W1's namespace-only over-refusal, `executor::PUBLISHED_BUT_REFUSED`"
        );
        assert_eq!(unparseable.len(), 1, "{unparseable:?}");
        assert_eq!(unparseable[0].0, "flow-trigger-fire-once-true-flow-listen");
        assert!(unparseable[0].1.contains("unknown field `fireOnce`"));
        assert_eq!(
            resolved,
            vec![
                "flow-trigger-any-mode-flow-listen/flow",
                "flow-trigger-at-least-mode-flow-listen/flow",
                "flow-trigger-depends-on-flow-listen/flow",
                "flow-trigger-invalid-inputs-flow-listen/flow",
                "flow-trigger-mixed-conditions-flow-listen/on_completion",
                "flow-trigger-mixed-conditions-flow-listen/on_failure",
                "flow-trigger-multiple-conditions-flow-listen/on_completion",
                "flow-trigger-multiple-conditions-flow-listen/on_failure",
                "flow-trigger-multiple-depends-on-flow-listen/flow1",
                "flow-trigger-multiple-depends-on-flow-listen/flow2",
                "flow-trigger-paused-listen/flow",
                "flow-trigger-stable-condition-id-flow-listen/flow",
                "trigger-flow-listener-invalid/listen-flow-invalid",
                "trigger-flow-listener-no-inputs/listen-flow",
                "trigger-flow-listener-no-inputs/listen-flow-failed",
                "trigger-flow-listener-with-concurrency-limit/flow",
                "trigger-flow-listener-with-pause/listen-flow",
                "trigger-flow-listener/listen-flow",
            ]
        );
        // 18 + 1 + 1 = 20 `dependsOn` declarations; + 1 window-free = 21.
        assert_eq!(
            resolved.len() + refused_at_save.len() + unparseable.len(),
            20
        );
    }
}
