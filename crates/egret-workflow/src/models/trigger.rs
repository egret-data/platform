// kestra: core/src/main/java/io/kestra/core/models/triggers/AbstractTrigger.java
// kestra: core/src/main/java/io/kestra/plugin/core/trigger/Schedule.java
// kestra: core/src/main/java/io/kestra/plugin/core/trigger/AbstractWebhookTrigger.java
// kestra: core/src/main/java/io/kestra/plugin/core/trigger/Webhook.java
// kestra: core/src/main/java/io/kestra/plugin/core/trigger/Flow.java

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::models::flow::{deserialize_labels, Label};
use crate::models::wire::{optional, required, with_default};
use crate::models::{Property, State};

/// A trigger node on a flow. Mirrors `Task`'s dispatch shape exactly: shared
/// fields on `Trigger` itself, plus `kind: TriggerKind` for the fields
/// specific to whichever trigger type this is.
///
/// Ported from `AbstractTrigger.java`, curated the same way Task 2 curated
/// `Subflow`: kept to what a trigger's *scheduling and gating* behaviour
/// needs, dropped what's infra/EE/logging plumbing unrelated to the model.
/// Dropped fields and why:
/// - `version` -- plugin versioning metadata, not modelled anywhere in this
///   crate.
/// - `workerSelector` -- worker-routing (tags + fallback), same as `Flow`'s
///   dropped field of the same name; no type in this crate models it.
/// - `policyRefs` -- Enterprise Edition governance-policy ids, same as
///   `Flow`'s dropped field of the same name.
/// - `logLevel` -- `org.slf4j.event.Level`, a logging concern with no
///   equivalent type in this crate.
/// - `assets` -- `AssetsDeclaration`, asset-lineage tracking, out of scope
///   for W1 (`TaskRun`'s own `assetEmits` is dropped for the same reason).
///
/// `Serialize` is hand-written, the exact inverse of `Deserialize`, for the
/// same reason `Task`'s is: a `#[serde(flatten)] kind: TriggerKind` derive
/// would nest a `Leaf`'s fields under a `"spec"` key instead of at the top
/// level, breaking round-trip.
#[derive(Debug, Clone, PartialEq)]
pub struct Trigger {
    pub id: String,
    pub r#type: String,
    pub description: Option<String>,
    // `@Builder.Default @NotNull private String when = "true";` -- `Option`
    // for the same `@NotNull`-is-bean-validation reason every other
    // `@NotNull` `Property<T>` field in this crate is `Option`-wrapped (see
    // `Parallel::concurrent` in `task.rs`); `default_when()` supplies
    // Kestra's own default of `"true"` when the key is absent, and an
    // explicit `when: null` still parses to `None` rather than erroring.
    pub when: Option<Property<String>>,
    pub disabled: bool,
    pub labels: Vec<Label>,
    /// **Parsed, serialised, and read by nothing** — W8 Task 2 fix round 2's
    /// type-directed sweep found it: this is the only state-typed field in the
    /// crate with no consumer. `trigger::flow::run` filters on
    /// [`states`](Self::states) and never on this, so a flow declaring
    /// `stopAfter:` round-trips it faithfully and is not stopped by it. Kestra
    /// disables the trigger once the source execution reaches one of these
    /// states. Recorded rather than implemented: no corpus flow declares it,
    /// so nothing here could judge an implementation.
    pub stop_after: Vec<State>,
    pub log_to_file: bool,
    pub fail_on_trigger_error: bool,
    pub allow_concurrent: bool,
    pub kind: TriggerKind,
}

/// The trigger types this crate's scheduler must see into, versus the opaque
/// leaf it must not -- same split `TaskKind` makes for tasks. Only
/// `Schedule`, `Webhook`, and `Flow` (this checkout's three
/// `io.kestra.plugin.core.trigger` types with a runnable `eval()`/`evaluate()`
/// beyond `McpToolTrigger`, which is MCP-specific and out of scope) are typed;
/// everything else -- third-party trigger plugins, `ScheduleOnDates` -- is an
/// opaque `Leaf`.
#[derive(Debug, Clone, PartialEq)]
pub enum TriggerKind {
    Schedule(Schedule),
    Webhook(Webhook),
    Flow(FlowTrigger),
    Leaf { spec: serde_json::Value },
}

/// `io.kestra.plugin.core.trigger.Schedule`. Cron-scheduled trigger.
///
/// `cron` and `withSeconds` are plain (non-dynamic) `@PluginProperty` fields
/// in Java -- not rendered, so plain Rust types, not `Property<T>`.
/// `timezone` defaults to `ZoneId.systemDefault().toString()` in Java, which
/// is JVM-environment-dependent (usually but not guaranteed UTC in a
/// container) -- rather than hard-coding a guess, this is `None` when absent
/// and left to the scheduler (W3+) to interpret "no timezone" as UTC, the
/// same call Kestra's own `Schedule::convertDateTime` makes when
/// `this.timezone == null`.
///
/// Dropped: the `executionTime`/`cachedCron` runtime caches (transient in
/// Java, not wire fields at all). `lateMaximumDelay` is captured verbatim (see
/// the field). `recoverMissedSchedules` is captured verbatim too; only the
/// trigger switch route reads it (W7e spec §3.3) -- no scheduler path does.
///
/// **`inputs` was dropped too, and the reason written down for it was wrong.**
/// W1 recorded it as "the deprecated `inputs` map"; measured at the pinned
/// checkout `96893b589`, `Schedule.java:223` is a plain
/// `private Map<String, Object> inputs;` with **no** `@Deprecated` annotation
/// anywhere in the file. W3a Task 8 restored the field because
/// `FlowValidator.findMissingInputsForScheduleTriggers` reads it
/// (`FlowValidator.java:284-312`) to decide whether a scheduled flow can
/// actually start, and a dropped field would have made that rule refuse every
/// scheduled flow that supplies its own inputs -- an over-refusal manufactured
/// out of an incorrect note. See `spec::validate`'s `schedule_missing_inputs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    // `Schedule.java:186-202`: `@NotNull private String cron;`, no Java
    // default. Fix round 1 finding 2: this was originally left required
    // (`String`, not `Option`), which was an oversight -- the same
    // `@NotNull`-is-bean-validation-not-parsing principle applied everywhere
    // else in this task (`Concurrency::limit`, `SlaCommon::behavior`, ...)
    // applies here too. Task 4 owes the not-null check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    #[serde(default)]
    pub with_seconds: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    // `Schedule.java:223`: `private Map<String, Object> inputs;` -- the values
    // a scheduled execution is started with. Only the KEY SET is read in this
    // crate today (`spec::validate`'s `schedule_missing_inputs` asks which
    // flow inputs the schedule supplies), so the values stay opaque
    // `serde_json::Value`s rather than being coerced against the input's
    // declared type; that coercion is W6's, at the point the schedule fires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<BTreeMap<String, serde_json::Value>>,
    // W6 Task 5 fix round 1: `lateMaximumDelay` is CAPTURED but deliberately
    // NOT interpreted.
    //
    // W1's comment above says the field was dropped "a `Duration`, no parser
    // for that type yet". Dropping it did two things nobody had measured:
    // `serde` has no `deny_unknown_fields` here, so a flow declaring
    // `lateMaximumDelay: PT1H` PARSED, RESOLVED, and then scheduled as though
    // the key were absent -- replaying a backlog the artifact throws away,
    // which is over-acceptance in the direction the oracle cannot catch -- and
    // the key was silently lost on re-serialisation too.
    //
    // It is `serde_json::Value`, not a `Duration`, because reproducing
    // `java.time.Duration.parse` (`PT1H30M`, `P1DT2H`, fractional seconds,
    // negatives) is a measurement this package has not made, and guessing it
    // is the class of unmeasured decision fix round 1 exists to remove.
    // Capturing it verbatim keeps the round trip honest and lets
    // `trigger::ScheduleTrigger::resolve` REFUSE the schedule, exactly as it
    // already refuses a `when` it cannot render.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_maximum_delay: Option<serde_json::Value>,
    // `Schedule.java`: `recoverMissedSchedules` (`ALL` | `NONE` | `LAST`).
    // Captured verbatim; interpreted by `trigger::RecoverMissed::from_model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recover_missed_schedules: Option<serde_json::Value>,
}

/// `io.kestra.plugin.core.trigger.Webhook` + its
/// `AbstractWebhookTrigger` base.
///
/// `key` is `@PluginProperty(dynamic = true)` in Java (a plain `String`
/// field, not literally typed `Property<String>`) -- modelled as
/// `Property<String>` anyway on the same basis `Loop::values` in `task.rs`
/// already established: what matters is whether Kestra renders the field
/// dynamically, not its literal Java type. `responseCode` IS literally
/// `Property<Integer>` in Java.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Webhook {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Property<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub fetch_type: FetchType,
    #[serde(default)]
    pub wait: bool,
    #[serde(default)]
    pub return_outputs: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_code: Option<Property<i32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FetchType {
    None,
    #[default]
    Fetch,
    Store,
}

/// `io.kestra.plugin.core.trigger.Flow` -- Kestra's *"Flow trigger"*, which
/// fires based on other flows' executions. Named `FlowTrigger` here (not
/// `Flow`) for the same reason `Output` was renamed `FlowOutput`: the obvious
/// name collides with this crate's own `Flow` (the flow model), and the
/// brief's own `TriggerKind::Flow` variant name is what carries the wire
/// identity, not this struct's name.
///
/// `states` defaults, in Java, to `terminatedTypes() + [PAUSED]` --
/// reproduced field-for-field as `default_flow_trigger_states()` rather than
/// hard-coding a shorter equivalent list, so a change to `State::is_terminated`
/// (W0) cannot silently desync this default.
///
/// `window` is modelled since W13b Task 2 as [`Window`]; before that it was
/// dropped, and because this struct has no `deny_unknown_fields` a declared
/// window silently vanished from the parsed flow. The
/// `dependsOnAsMultipleCondition()`/`DependencyCondition` machinery is still
/// not here: it is evaluation logic (`trigger::window` and
/// `trigger::depends_on`, W13b Task 3), not model. Which window shapes egret
/// accepts is decided by `trigger::window::window_refusals`, which
/// `spec::validate` reports at save and `TimeWindow::from_model` enforces at
/// run time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowTrigger {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inputs: Option<HashMap<String, serde_json::Value>>,
    #[serde(default = "default_flow_trigger_states")]
    pub states: Vec<State>,
    #[serde(default)]
    pub depends_on: Vec<Dependency>,
    #[serde(default)]
    pub mode: FlowTriggerMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_satisfied: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
}

/// `io.kestra.core.models.triggers.Window` -- the `dependsOn` evaluation
/// window, **as the pinned artifact declares it**, which is not the checkout's.
///
/// W13b Task 1 measured the image's class (`oracle-exec/w13b-windows.json`,
/// finding `window_fields`): it has exactly seven properties -- `offset`,
/// `lookback`, `deadline`, `timezone`, `from`, `to`, `every` -- and refuses
/// anything else with 422 `Unrecognized field "fireOnce" (class
/// io.kestra.core.models.triggers.Window), not marked as ignorable (7 known
/// properties ...)`. The sibling checkout (`96893b589`) differs both ways: it
/// has a `fireOnce` and no `timezone`. The measurement wins, so `timezone` is
/// here and `fireOnce` is refused.
///
/// **`deny_unknown_fields`, and that is deliberate, unlike [`FlowTrigger`].**
/// `FlowTrigger` and [`Schedule`] tolerate unknown keys because W1 dropped
/// fields of theirs that real flows carry (see `late_maximum_delay` above for
/// what that cost). `Window` drops nothing: the artifact's class has exactly
/// these seven, and Jackson refuses an eighth. Refusing it at the model means
/// no stored draft, and therefore no `flow_listeners` row
/// (`scheduler::sync` builds those from the draft's source whenever it
/// PARSES, validation errors or not), can ever carry a window field egret
/// does not implement -- nothing downstream has to remember to refuse it.
///
/// Every value is kept as the author wrote it (`String`), not as a parsed
/// `Duration`/`LocalTime`/`Tz`, for the same reason `late_maximum_delay` is a
/// raw value: the round trip stays byte-honest, and parsing is one place --
/// `trigger::duration` -- called by the validator at save and by the window
/// computation at run time, so the two cannot disagree about what a value
/// means. Which COMBINATIONS are accepted is `trigger::window::window_refusals`'s job.
///
/// Kind, per spec §5.1 and `Window.toTimeWindow()`: `deadline` gives
/// `DAILY_TIME_DEADLINE`; `from` + `to` gives `DAILY_TIME_WINDOW`; `lookback`
/// gives `SLIDING_WINDOW`; anything else gives `DURATION_WINDOW` with `every`
/// (default one day) and `offset`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Window {
    /// `LocalTime`, `HH:mm` or `HH:mm:ss`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    /// `LocalTime`, `HH:mm` or `HH:mm:ss`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `LocalTime`, `HH:mm` or `HH:mm:ss`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// ISO-8601 `Duration`; see `trigger::duration::parse_duration`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// ISO-8601 `Duration`; see `trigger::duration::parse_duration`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<String>,
    /// ISO-8601 `Duration`; see `trigger::duration::parse_duration`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lookback: Option<String>,
    /// An IANA zone (`chrono_tz::Tz`). Absent means UTC -- chosen (user
    /// decision D2): egret has no server zone, and UTC is what the artifact
    /// aligned on with none set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// `Flow.Dependency` (inner class).
///
/// Names another flow by `(namespace, flowId)` -- structurally identical to
/// `task.rs`'s `Subflow`, and resolved through the exact same seam: see
/// [`Self::flow_ref`] below, and `publish::resolve_trigger_dependencies`
/// (fix round, whole-branch review finding 2) for the resolution walk that
/// fills it in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub states: Option<Vec<State>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<HashMap<String, serde_json::Value>>,
    // `@Builder.Default private Property<String> when = Property.ofValue("true");`
    // -- literally `Property<String>` in Java this time, with a real default,
    // same `Option<Property<T>>` + `default_when()` treatment as `Trigger::when`.
    #[serde(default = "default_when")]
    pub when: Option<Property<String>>,

    // Mirrors `task.rs`'s `Subflow::flow_ref` field-for-field, including WHY
    // it is a third field rather than an overwrite of `namespace`/`flow_id`:
    // those are Kestra's own identity pair, plausibly still needed by an
    // error message or an editor after publish, so the resolved id sits
    // beside them rather than replacing either. Filled in by
    // `publish::resolve_trigger_dependencies`, which since debt §5.1
    // (2026-10-19) looks `namespace`/`flow_id` up in the PUBLISHING
    // workspace only (`FlowRepo::find_in_workspace`): `Some` when that
    // workspace holds the flow, `None` (and no warning, as Kestra) when it
    // does not. Publish always overwrites it, so a value an author wrote
    // into a draft never reaches a snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_ref: Option<uuid::Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FlowTriggerMode {
    #[default]
    All,
    Any,
    AtLeast,
}

fn default_when() -> Option<Property<String>> {
    Some(Property::from_source("true".to_string()))
}

/// `State.Type.terminatedTypes()` (`SUCCESS, WARNING, FAILED, KILLED,
/// CANCELLED, RETRIED, SKIPPED, RESUBMITTED`, in `State.Type`'s declaration
/// order) with `PAUSED` appended, matching
/// `ListUtils.concat(State.Type.terminatedTypes(), List.of(PAUSED))` exactly.
fn default_flow_trigger_states() -> Vec<State> {
    vec![
        State::Success,
        State::Warning,
        State::Failed,
        State::Killed,
        State::Cancelled,
        State::Retried,
        State::Skipped,
        State::Resubmitted,
        State::Paused,
    ]
}

impl<'de> Deserialize<'de> for Trigger {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;

        let mut map = serde_json::Map::deserialize(d)?;

        // `required`/`optional`/`with_default` (see `wire.rs`) always try to
        // deserialize a *present* value as the target type -- a caller-
        // supplied `disabled: "true"` (a quoted string) is a real type error
        // here, not a silent fall-through to `false`. Only a genuinely
        // *absent* key takes `missing_field`/`None`/the given default. See
        // the task report, Fix round 1 finding 1.
        let id: String = required(&mut map, "id")?;
        let r#type: String = required(&mut map, "type")?;
        let description: Option<String> = optional(&mut map, "description")?;
        let when: Option<Property<String>> = match map.remove("when") {
            Some(v) => serde_json::from_value(v).map_err(D::Error::custom)?,
            None => default_when(),
        };
        let disabled: bool = with_default(&mut map, "disabled", false)?;
        // `deserialize_labels` takes a `Deserializer` directly (it dispatches
        // on the JSON shape itself: list, map, or null) rather than a target
        // type via `serde_json::from_value::<T>`, so it does not fit the
        // `required`/`optional`/`with_default` shape above -- but it already
        // errors on any shape that is neither a list, a map, nor null (see
        // its own `other => Err(...)` arm in `flow.rs`), so a wrong-typed
        // `labels` was never silently swallowed to begin with.
        let labels = match map.remove("labels") {
            Some(v) => deserialize_labels(v).map_err(D::Error::custom)?,
            None => Vec::new(),
        };
        let stop_after: Vec<State> = with_default(&mut map, "stopAfter", Vec::new())?;
        let log_to_file: bool = with_default(&mut map, "logToFile", false)?;
        let fail_on_trigger_error: bool = with_default(&mut map, "failOnTriggerError", false)?;
        let allow_concurrent: bool = with_default(&mut map, "allowConcurrent", false)?;

        let rest = serde_json::Value::Object(map);
        let kind = kind_for(&r#type, rest).map_err(D::Error::custom)?;

        Ok(Trigger {
            id,
            r#type,
            description,
            when,
            disabled,
            labels,
            stop_after,
            log_to_file,
            fail_on_trigger_error,
            allow_concurrent,
            kind,
        })
    }
}

impl Serialize for Trigger {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;

        let kind_value = match &self.kind {
            TriggerKind::Schedule(v) => serde_json::to_value(v),
            TriggerKind::Webhook(v) => serde_json::to_value(v),
            TriggerKind::Flow(v) => serde_json::to_value(v),
            TriggerKind::Leaf { spec } => Ok(spec.clone()),
        }
        .map_err(S::Error::custom)?;

        let mut map = match kind_value {
            serde_json::Value::Object(map) => map,
            other => {
                return Err(S::Error::custom(format!(
                    "trigger kind serialised to a non-object JSON value: {other}"
                )))
            }
        };

        map.insert("id".to_string(), serde_json::Value::String(self.id.clone()));
        map.insert(
            "type".to_string(),
            serde_json::Value::String(self.r#type.clone()),
        );
        if let Some(description) = &self.description {
            map.insert(
                "description".to_string(),
                serde_json::Value::String(description.clone()),
            );
        }
        if let Some(when) = &self.when {
            map.insert(
                "when".to_string(),
                serde_json::to_value(when).map_err(S::Error::custom)?,
            );
        }
        map.insert(
            "disabled".to_string(),
            serde_json::Value::Bool(self.disabled),
        );
        if !self.labels.is_empty() {
            map.insert(
                "labels".to_string(),
                serde_json::to_value(&self.labels).map_err(S::Error::custom)?,
            );
        }
        if !self.stop_after.is_empty() {
            map.insert(
                "stopAfter".to_string(),
                serde_json::to_value(&self.stop_after).map_err(S::Error::custom)?,
            );
        }
        map.insert(
            "logToFile".to_string(),
            serde_json::Value::Bool(self.log_to_file),
        );
        map.insert(
            "failOnTriggerError".to_string(),
            serde_json::Value::Bool(self.fail_on_trigger_error),
        );
        map.insert(
            "allowConcurrent".to_string(),
            serde_json::Value::Bool(self.allow_concurrent),
        );

        serde_json::Value::Object(map).serialize(serializer)
    }
}

/// Dispatch a trigger's remaining fields on its `type`, mirroring `Task`'s
/// `kind_for` -- both the FQCN and the `core.` short alias resolve to the
/// same three typed variants.
fn kind_for(r#type: &str, rest: serde_json::Value) -> Result<TriggerKind, serde_json::Error> {
    let canonical = r#type
        .strip_prefix("io.kestra.plugin.core.trigger.")
        .map(str::to_ascii_lowercase)
        .or_else(|| r#type.strip_prefix("core.").map(str::to_ascii_lowercase));

    Ok(match canonical.as_deref() {
        Some("schedule") => TriggerKind::Schedule(serde_json::from_value(rest)?),
        Some("webhook") => TriggerKind::Webhook(serde_json::from_value(rest)?),
        Some("flow") => TriggerKind::Flow(serde_json::from_value(rest)?),
        _ => TriggerKind::Leaf { spec: rest },
    })
}

#[cfg(test)]
mod tests {
    use super::{FetchType, Trigger, TriggerKind};
    use crate::models::State;

    #[test]
    fn recover_missed_schedules_round_trips() {
        let base = "id: n\ntype: core.schedule\ncron: \"0 2 * * *\"\n";
        let t: Trigger =
            serde_norway::from_str(&format!("{base}recoverMissedSchedules: LAST\n")).unwrap();
        let TriggerKind::Schedule(s) = &t.kind else {
            panic!("{t:?}")
        };
        assert_eq!(s.recover_missed_schedules, Some(serde_json::json!("LAST")));
        let out = serde_json::to_value(&t).unwrap();
        assert_eq!(out["recoverMissedSchedules"], "LAST");
        let t: Trigger = serde_norway::from_str(base).unwrap();
        let out = serde_json::to_value(&t).unwrap();
        assert!(out.get("recoverMissedSchedules").is_none(), "{out}");
    }

    /// The reference flow's own trigger, exercised directly rather than only
    /// through `Flow`.
    #[test]
    fn schedule_is_typed_under_both_spellings() {
        for spelling in ["io.kestra.plugin.core.trigger.Schedule", "core.schedule"] {
            let yaml = format!("id: nightly\ntype: {spelling}\ncron: \"0 2 * * *\"\n");
            let t: Trigger = serde_norway::from_str(&yaml).unwrap();
            match &t.kind {
                TriggerKind::Schedule(s) => {
                    assert_eq!(s.cron.as_deref(), Some("0 2 * * *"), "{spelling}")
                }
                other => panic!("{spelling}: expected Schedule, got {other:?}"),
            }
            assert_eq!(
                t.when.as_ref().unwrap().source(),
                "true",
                "when must default to Kestra's own \"true\""
            );
        }
    }

    /// Fix round 1 finding 2: `Schedule::cron` is `@NotNull` with no Java
    /// default, so -- same `@NotNull`-is-bean-validation-not-parsing
    /// principle as `Concurrency::limit` -- a `Schedule` with no `cron` at
    /// all, or an explicit `cron: null`, must still parse. Task 4 owns
    /// rejecting it.
    #[test]
    fn schedule_without_cron_still_parses() {
        let yaml = "id: nightly\ntype: core.schedule\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TriggerKind::Schedule(s) => assert!(s.cron.is_none()),
            other => panic!("expected Schedule, got {other:?}"),
        }

        let yaml_null = "id: nightly\ntype: core.schedule\ncron: null\n";
        let t2: Trigger = serde_norway::from_str(yaml_null).unwrap();
        match &t2.kind {
            TriggerKind::Schedule(s) => assert!(s.cron.is_none()),
            other => panic!("expected Schedule, got {other:?}"),
        }
    }

    #[test]
    fn webhook_carries_its_own_key_and_fetch_type() {
        let yaml = "id: hook\ntype: io.kestra.plugin.core.trigger.Webhook\nkey: abc123\nfetchType: STORE\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TriggerKind::Webhook(w) => {
                assert_eq!(w.key.as_ref().unwrap().source(), "abc123");
                assert_eq!(w.fetch_type, FetchType::Store);
            }
            other => panic!("expected Webhook, got {other:?}"),
        }
    }

    #[test]
    fn flow_trigger_states_default_to_terminated_plus_paused() {
        let yaml = "id: chain\ntype: io.kestra.plugin.core.trigger.Flow\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TriggerKind::Flow(f) => {
                assert!(f.states.contains(&State::Success));
                assert!(f.states.contains(&State::Paused));
                assert!(!f.states.contains(&State::Running));
            }
            other => panic!("expected Flow, got {other:?}"),
        }
    }

    /// Anything not registered (third-party plugin triggers, `ScheduleOnDates`)
    /// becomes an opaque `Leaf`, same as an unknown `Task` type.
    #[test]
    fn an_unknown_trigger_type_becomes_an_opaque_leaf() {
        let yaml =
            "id: custom\ntype: io.kestra.plugin.aws.sqs.Trigger\nqueueUrl: https://example\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();
        match &t.kind {
            TriggerKind::Leaf { spec } => assert_eq!(spec["queueUrl"], "https://example"),
            other => panic!("expected Leaf, got {other:?}"),
        }
    }

    #[test]
    fn disabled_and_stop_after_parse() {
        let yaml = "id: nightly\ntype: core.schedule\ncron: \"@daily\"\ndisabled: true\nstopAfter:\n  - FAILED\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();
        assert!(t.disabled);
        assert_eq!(t.stop_after, vec![State::Failed]);
    }

    /// Fix round 1 finding 6: the previous version of this test never
    /// actually round-tripped anything -- it only deserialized once. This
    /// one serializes and re-parses, and checks the actual wire keys the
    /// hand-written `Serialize` produces.
    #[test]
    fn disabled_and_stop_after_actually_round_trip() {
        let yaml = "id: nightly\ntype: core.schedule\ncron: \"@daily\"\ndisabled: true\nstopAfter:\n  - FAILED\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();

        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["disabled"], true);
        assert_eq!(json["stopAfter"], serde_json::json!(["FAILED"]));

        let round: Trigger = serde_json::from_value(json).unwrap();
        assert_eq!(round, t);
    }

    // -- Fix round 1, finding 4: the hand-written `Serialize for Trigger`
    // (mirroring `Task`'s, ~70 lines) had no test at all. `Task`'s own
    // equivalent bug -- a `Serialize` that was not the inverse of its
    // `Deserialize`, nesting a leaf's fields under a `"spec"` key -- is
    // exactly the class of bug an untested hand-written serializer hides.
    // These two tests are `Task`'s
    // `leaf_round_trips_without_growing_a_spec_wrapper` /
    // `nested_sequential_leaf_round_trips_without_growing_a_spec_wrapper`,
    // adapted to `Trigger`.

    /// A typed variant (`Schedule`): parse -> serialise -> parse -> serialise
    /// must be byte-identical, and every `Trigger`-level field (`when`,
    /// `disabled`, ...) must survive alongside the kind's own fields.
    #[test]
    fn schedule_round_trips_byte_identically() {
        let yaml = "id: nightly\ntype: io.kestra.plugin.core.trigger.Schedule\ncron: \"0 2 * * *\"\ndisabled: true\n";
        let t1: Trigger = serde_norway::from_str(yaml).unwrap();
        let once = serde_json::to_string(&t1).unwrap();

        let as_value: serde_json::Value = serde_json::from_str(&once).unwrap();
        assert_eq!(as_value["cron"], "0 2 * * *");
        assert_eq!(as_value["disabled"], true);
        assert_eq!(as_value["id"], "nightly");
        assert_eq!(
            as_value["when"], "true",
            "when must default to Kestra's own \"true\" and survive round-trip"
        );

        let t2: Trigger = serde_json::from_str(&once).unwrap();
        let twice = serde_json::to_string(&t2).unwrap();
        assert_eq!(
            once, twice,
            "parse -> serialise -> parse -> serialise must be byte-identical"
        );
        assert_eq!(t1, t2);
    }

    /// A `Leaf`: its own fields must sit at the TOP level (beside `id`/
    /// `type`/`disabled`), not nested under a `"spec"` key -- the same
    /// property `Task`'s hand-written `Serialize` protects, for the same
    /// reason (`#[serde(untagged)]`'s derived `Serialize` would write a
    /// struct variant as its field map, `{"spec": {...}}`, growing a deeper
    /// `spec.spec.spec...` nest on every re-parse).
    #[test]
    fn leaf_trigger_round_trips_without_growing_a_spec_wrapper() {
        let yaml = "id: custom\ntype: io.kestra.plugin.aws.sqs.Trigger\nqueueUrl: https://example\nqueueName: orders\n";
        let t1: Trigger = serde_norway::from_str(yaml).unwrap();
        let once = serde_json::to_string(&t1).unwrap();

        let as_value: serde_json::Value = serde_json::from_str(&once).unwrap();
        assert_eq!(as_value["queueUrl"], "https://example");
        assert_eq!(as_value["queueName"], "orders");
        assert!(
            as_value.get("spec").is_none(),
            "a leaf's fields must sit at the top level, not nested under spec: {once}"
        );
        // `id`/`type` are `Trigger`'s own fields and must not be duplicated
        // into the leaf's own field set on the way back out.
        assert_eq!(as_value["id"], "custom");
        assert_eq!(as_value["type"], "io.kestra.plugin.aws.sqs.Trigger");

        let t2: Trigger = serde_json::from_str(&once).unwrap();
        let twice = serde_json::to_string(&t2).unwrap();
        assert_eq!(
            once, twice,
            "parse -> serialise -> parse -> serialise must be byte-identical"
        );
        assert_eq!(t1, t2);
    }

    // -- Fix round 1, finding 1 (CRITICAL) -- same bug the reviewer proved
    // live here originally: `disabled: "true"` (a quoted string) parsed
    // successfully with `disabled == false`. See the equivalent block of
    // tests in `task.rs`.

    #[test]
    fn disabled_with_a_string_value_is_a_parse_error_not_a_silent_false() {
        let yaml = "id: t\ntype: core.schedule\ncron: \"@daily\"\ndisabled: \"true\"\n";
        let err = serde_norway::from_str::<Trigger>(yaml).unwrap_err();
        assert!(
            err.to_string().to_lowercase().contains("bool")
                || err.to_string().to_lowercase().contains("invalid type"),
            "expected a type error, got: {err}"
        );
    }

    #[test]
    fn disabled_with_a_number_value_is_a_parse_error() {
        let yaml = "id: t\ntype: core.schedule\ncron: \"@daily\"\ndisabled: 1\n";
        assert!(serde_norway::from_str::<Trigger>(yaml).is_err());
    }

    #[test]
    fn description_with_a_non_string_value_is_a_parse_error() {
        let yaml = "id: t\ntype: core.schedule\ncron: \"@daily\"\ndescription: [not, a, string]\n";
        assert!(serde_norway::from_str::<Trigger>(yaml).is_err());
    }

    #[test]
    fn wrong_typed_id_is_a_type_error_not_a_misleading_missing_field() {
        let yaml = "id: 5\ntype: core.schedule\ncron: \"@daily\"\n";
        let err = serde_norway::from_str::<Trigger>(yaml).unwrap_err();
        assert!(
            !err.to_string().to_lowercase().contains("missing field"),
            "a wrong-typed `id` must not report as a missing field: {err}"
        );
    }

    /// The other three booleans the reviewer named directly:
    /// `logToFile`/`failOnTriggerError`/`allowConcurrent`.
    #[test]
    fn every_other_trigger_boolean_rejects_a_wrong_typed_value() {
        for key in ["logToFile", "failOnTriggerError", "allowConcurrent"] {
            let yaml = format!("id: t\ntype: core.schedule\ncron: \"@daily\"\n{key}: \"true\"\n");
            assert!(
                serde_norway::from_str::<Trigger>(&yaml).is_err(),
                "{key}: \"true\" must be a parse error, not a silent false"
            );
        }
    }

    /// Defaults still apply when keys are genuinely absent.
    #[test]
    fn trigger_booleans_still_default_to_false_when_absent() {
        let yaml = "id: t\ntype: core.schedule\ncron: \"@daily\"\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();
        assert!(!t.disabled);
        assert!(!t.log_to_file);
        assert!(!t.fail_on_trigger_error);
        assert!(!t.allow_concurrent);
    }

    // -- Fix round 2: an explicit `null` must parse exactly like an absent
    // key (`AbstractTrigger.java:60`'s `disabled` is a primitive `boolean`
    // with `@Builder.Default`; Jackson's default
    // `FAIL_ON_NULL_FOR_PRIMITIVES = false` coerces an explicit `null` there
    // to the type default rather than erroring) -- while a WRONG-TYPED,
    // non-null value must still be rejected. Round 1 fixed the wrong-type
    // case and broke the null case as a side effect; both need their own
    // test or one regresses the other again.

    #[test]
    fn explicit_null_description_parses_like_an_absent_one() {
        let yaml = "id: t\ntype: core.schedule\ncron: \"@daily\"\ndescription: null\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();
        assert!(t.description.is_none());
    }

    #[test]
    fn explicit_null_disabled_parses_like_an_absent_one() {
        let yaml = "id: t\ntype: core.schedule\ncron: \"@daily\"\ndisabled: null\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();
        assert!(!t.disabled);
    }

    /// Companion to the two tests above: `null` must parse, but a
    /// wrong-typed non-null value must still be rejected -- the exact
    /// distinction this round exists to draw.
    #[test]
    fn wrong_typed_non_null_disabled_is_still_rejected() {
        assert!(serde_norway::from_str::<Trigger>(
            "id: t\ntype: core.schedule\ncron: \"@daily\"\ndisabled: \"true\"\n"
        )
        .is_err());
        assert!(serde_norway::from_str::<Trigger>(
            "id: t\ntype: core.schedule\ncron: \"@daily\"\ndisabled: 1\n"
        )
        .is_err());
    }

    // ---------- W13b Task 2: `window` ----------

    /// `window` survives a parse -> serialise -> parse round trip, all seven
    /// fields of it. Before W13b it was not modelled, and `FlowTrigger` has no
    /// `deny_unknown_fields`, so the key was silently DROPPED: the flow parsed,
    /// and the window vanished from the plan and from any re-serialisation.
    ///
    /// Written against the wire (JSON) rather than the Rust field so that it
    /// compiled, and failed, on the commit before `Window` existed.
    #[test]
    fn a_flow_triggers_window_round_trips_every_field() {
        let yaml = "id: t\ntype: io.kestra.plugin.core.trigger.Flow\n\
                    dependsOn:\n  - namespace: a.b\n    flowId: p1\n\
                    window:\n  every: PT6H\n  offset: PT1H\n  timezone: Asia/Kolkata\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(
            json["window"],
            serde_json::json!({"every": "PT6H", "offset": "PT1H", "timezone": "Asia/Kolkata"}),
            "the window must be carried, and absent fields must stay absent: {json}"
        );

        // The other four, in the shapes that set them (they are mutually
        // exclusive with the three above, so they cannot share one window).
        for (yaml_window, expected) in [
            (
                "window:\n  deadline: \"09:00:00\"\n",
                serde_json::json!({"deadline": "09:00:00"}),
            ),
            (
                "window:\n  from: \"10:00:00\"\n  to: \"12:00:00\"\n",
                serde_json::json!({"from": "10:00:00", "to": "12:00:00"}),
            ),
            (
                "window:\n  lookback: PT2M\n",
                serde_json::json!({"lookback": "PT2M"}),
            ),
        ] {
            let yaml = format!(
                "id: t\ntype: io.kestra.plugin.core.trigger.Flow\n\
                 dependsOn:\n  - namespace: a.b\n    flowId: p1\n{yaml_window}"
            );
            let t: Trigger = serde_norway::from_str(&yaml).unwrap();
            let json = serde_json::to_value(&t).unwrap();
            assert_eq!(json["window"], expected, "{yaml_window}");

            // And back: the re-serialised form parses to the same trigger.
            let again: Trigger = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(again, t, "{yaml_window}");
        }
    }

    /// An UNQUOTED `deadline: 09:00:00` reaches the model as the string the
    /// author wrote. YAML 1.1 would read it as a base-60 integer (32400);
    /// serde_norway does not, and this pins that, because a number here would
    /// be a parse error the author could not see the reason for.
    #[test]
    fn an_unquoted_time_of_day_is_a_string() {
        let yaml = "id: t\ntype: io.kestra.plugin.core.trigger.Flow\n\
                    dependsOn:\n  - namespace: a.b\n    flowId: p1\n\
                    window:\n  from: 09:00:00\n  to: 17:30\n";
        let t: Trigger = serde_norway::from_str(yaml).unwrap();
        let TriggerKind::Flow(f) = &t.kind else {
            panic!("expected Flow, got {:?}", t.kind)
        };
        let w = f.window.as_ref().expect("window");
        assert_eq!(w.from.as_deref(), Some("09:00:00"));
        assert_eq!(w.to.as_deref(), Some("17:30"));
    }
}
