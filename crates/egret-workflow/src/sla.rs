//! `sla:` — Kestra's flow-level service-level agreements, **one of the two
//! kinds evaluated and the other refused by name**.
//!
//! kestra: `core/src/main/java/io/kestra/core/models/flows/sla/` (`SLA`,
//! `ExecutionChangedSLA`, `ExecutionMonitoringSLA`, `Violation`,
//! `types/{ExecutionAssertionSLA,MaxDurationSLA}`),
//! `executor/src/main/java/io/kestra/executor/SLAService.java`, and
//! `ExecutorService.{handleExecutionChangedSLA,processViolation}`.
//!
//! W1 ported `sla:` as a data shape ([`crate::models::Sla`]) and wrote that
//! *"evaluating an SLA against a running execution is W3/W8"*. This is that,
//! for exactly one of the two subtypes.
//!
//! # The two subtypes are two different mechanisms in the artifact, not two
//! cases of one
//!
//! That is the fact the rest of this module rests on, and it is read off the
//! `implements` clause rather than off the names:
//!
//! | subtype | interface | who evaluates it | when |
//! |---|---|---|---|
//! | `EXECUTION_ASSERTION` | `ExecutionChangedSLA` | `ExecutorService.handleExecutionChangedSLA`, on the executor's own message path | **every execution state change**, ahead of `process` (`ExecutionEventMessageHandler.java:244`) |
//! | `MAX_DURATION` | `ExecutionMonitoringSLA` | `DefaultExecutor.executionSLAMonitorLoop`, a **separate scheduled loop** (`DefaultExecutor.java:347`) | when a stored `SLAMonitor.deadline` expires against `Instant.now()` |
//!
//! `SLAService.evaluateExecutionChangedSLA` filters on
//! `ExecutionChangedSLA.class::isInstance`, so the message path **cannot** see
//! a `MAX_DURATION` at all; and the monitor loop reads one `SLA` by id and
//! never evaluates an assertion. Two constructs under one YAML key, and only
//! the first of them is a function of the execution's own state.
//!
//! # `MAX_DURATION` is refused by name, and the refusal is a MEASUREMENT
//!
//! **Five golden entries are held by a `MAX_DURATION` and not one of them can
//! be banked by any implementation whatsoever** — including a correct one — for
//! a reason that is a property of `oracle-exec/driver.rs` rather than of egret:
//!
//! * `valids/sla-max-duration-fail` (`PT0.5S` over a `Sleep: PT2S`),
//! * `valids/sla-subflow` (`PT1S` over `PT10S`),
//! * `valids/flow-concurrency-sla-fail-child` (`PT3S` over `PT10M`),
//! * `valids/sla-parent-flow` and `valids/flow-concurrency-sla-fail-parent`,
//!   which declare no `sla:` themselves and are held by the one in the child
//!   they call.
//!
//! `MaxDurationSLA.evaluate` is
//! `Duration.between(execution.getState().getStartDate(), Instant.now())
//! .compareTo(duration) > 0`, so it needs an **interval during which the
//! execution is non-terminal and longer than the bound**. The driver's registry
//! is built with `worker::handlers::sleep::Sleep::capped(Duration::ZERO)`,
//! which validates the duration exactly as the product handler does and then
//! waits `min(duration, ZERO)`; `Sleep::run` is synchronous inside one
//! `worker::tick` and writes **no** `TaskRun::not_before`. So the only task any
//! of these five flows has reaches its terminal inside the tick that started
//! it, and that interval does not exist.
//!
//! **The clock does move, and the refusal has to survive a reader who knows
//! that.** `driver::pass_time` rewinds `executions.created_at` by
//! `shortfall + 1` and calls `worker::resume`, so a deadline really does
//! elapse — **but only a deadline that was registered**. Its query is
//! `WHERE r.not_before_ms IS NOT NULL AND r.not_before_ms > (now - created_at)`,
//! and a capped `Sleep` registers none, so for these five flows `pass_time`
//! returns `Ok(false)` the first time it is asked and the driver's loop breaks.
//! The window is not *entered late* and it is not *missed*: there is nothing in
//! this harness that can advance an execution's clock while one of its runs is
//! in flight, because no run of these flows is ever in flight across a step.
//! `tests/executor.rs::the_max_duration_window_does_not_exist_in_this_harness`
//! is that sentence as a running program, with a control that reports a
//! non-zero count for a flow which *does* register a deadline.
//!
//! This is `oracle-exec/README.md`'s **fifth** blindness mechanism — *the
//! harness compresses time* — and its effect is a third one within that
//! mechanism, distinct from the other two and written down there rather than
//! merged with them. So no `MAX_DURATION` evaluator is written here: an
//! implementation nothing can score is the shape this programme has already
//! paid for three times, and a monitor loop keyed on wall-clock time is
//! something [`crate::executor::advance`] — pure over `(plan, state, now)`,
//! no I/O and no clock — cannot host in any case.
//!
//! # What is NOT ported from `processViolation`, named rather than implied
//!
//! * **`violation.labels()`.** `processViolation` appends the violated SLA's
//!   labels to the execution's. `workflow.executions` has no labels column and
//!   nothing in the golden compares labels, so a `behavior: NONE` violation —
//!   `valids/sla-execution-condition`'s second SLA is exactly that — is a
//!   complete no-op here. It is reachable only when it is the FIRST violation,
//!   which for that flow it never is.
//! * **`markAs`'s task-run half.** Kestra also pushes the new state onto
//!   `execution.findLastNotTerminated()` and emits an `ExecutionKilledExecution`
//!   with `isOnKillCascade`. Both golden entries this module banks violate at
//!   `CREATED` with **zero** task runs (`"runs": []`), so nothing in the
//!   corpus can see either half, and the kill queue is a subsystem egret does
//!   not have. An execution that cancels with a run still going therefore
//!   leaves that run where it was — recorded as a divergence rather than
//!   guessed at.

use crate::executor::RunState;
use crate::models::{Flow, Label, Property, Sla, SlaBehavior};

/// `io.kestra.core.models.flows.sla.Violation` — `(slaId, behavior, labels,
/// reason)`.
///
/// The `reason` is carried even though nothing compares it: it is the string
/// `SLAService` logs, and a violation that could not say why it fired would be
/// unusable from a product log.
///
/// `PartialEq` and not `Eq`: `models::Label` is `PartialEq` only, and widening
/// a W1 model to satisfy a derive in W8 would be this crate's tail wagging its
/// own dog.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub sla_id: String,
    pub behavior: Option<SlaBehavior>,
    pub labels: Vec<Label>,
    pub reason: String,
}

/// **The first `ExecutionChangedSLA` violation this flow has, or none.**
///
/// `SLAService.evaluateExecutionChangedSLA` collects every violation and
/// `ExecutorService.handleExecutionChangedSLA` then takes
/// `violations.getFirst()` — *"For now, we only consider the first violation to
/// be capable of updating the execution. Other violations would only be
/// logged."* Since nothing here logs, collecting the rest would be building a
/// list to throw away, and **short-circuiting is observably the same thing**:
/// the later ones cannot affect the execution.
///
/// `render` is the caller's renderer for one `Property<String>`, returning
/// `None` for a render that will not resolve. That is Kestra's own handling and
/// not a shortcut: `evaluateExecutionChangedSLA` wraps each `sla.evaluate` in a
/// `try/catch` that logs and yields `Optional.empty()`, so **an assertion that
/// will not render is skipped rather than failing the execution** — the
/// opposite of what a `runIf:` does with the same accident
/// (`render::render_run_if` fails the run). The two are different Java and are
/// deliberately different here.
///
/// A `MAX_DURATION` is skipped, which is the `ExecutionChangedSLA.class
/// ::isInstance` filter and not a gap — see this module's doc.
pub fn first_execution_changed_violation(
    flow: &Flow,
    render: &mut dyn FnMut(&Property<String>) -> Option<String>,
) -> Option<Violation> {
    flow.sla.iter().find_map(|sla| match sla {
        // `ExecutionMonitoringSLA`, and the message path never sees one.
        Sla::MaxDuration { .. } => None,
        Sla::ExecutionAssertion { common, assert_ } => {
            // `@NotNull @NotEmpty` in Java. `spec::validate` has no rule for an
            // absent `assert:` (only `behavior:` and `MAX_DURATION`'s
            // `duration:` are checked), so this is reachable from a source that
            // was never published -- which the oracle's driver is. Kestra would
            // render a null and NPE; skipping is the answer that cannot
            // over-accept.
            let assertion = assert_.as_ref()?;
            let rendered = render(assertion)?;
            // `TruthUtils.isTruthy`, which is the SAME predicate
            // `If`/`Switch`/`runIf:` use -- `executor::is_truthy`, measured
            // against the running artifact over 38 rows including the eleven
            // where Java's `String.trim()` and Rust's `str::trim()` disagree.
            // Calling it rather than copying it is what keeps one Java method
            // from having two answers in this crate.
            match crate::executor::is_truthy(&rendered) {
                true => None,
                false => Some(Violation {
                    sla_id: common.id.clone(),
                    behavior: common.behavior,
                    labels: common.labels.clone(),
                    // `ExecutionAssertionSLA.evaluate`'s own wording, over the
                    // UNRENDERED source, which is what Java interpolates.
                    reason: format!("assertion is false: {}.", assertion.source()),
                }),
            }
        }
    })
}

/// **The state `ExecutorService.processViolation` puts the execution into**, or
/// `None` for a violation that changes no state.
///
/// `FAIL -> markAs(FAILED)`, `CANCEL -> markAs(CANCELLED)`, `NONE -> the
/// execution unchanged`. `None` is returned for `NONE` *and* for a violation
/// whose `behavior:` is absent: Kestra's field is `@NotNull` bean validation,
/// `spec::validate` refuses the flow with `sla_behavior_required`, and the Java
/// would `switch` on a null and throw — which is not a state this executor can
/// express and not one a published flow can reach.
pub fn state_for(behavior: Option<SlaBehavior>) -> Option<RunState> {
    match behavior? {
        SlaBehavior::Fail => Some(RunState::Failed),
        SlaBehavior::Cancel => Some(RunState::Cancelled),
        SlaBehavior::None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::SlaCommon;

    fn assertion(id: &str, behavior: Option<SlaBehavior>, source: &str) -> Sla {
        Sla::ExecutionAssertion {
            common: SlaCommon {
                id: id.to_string(),
                behavior,
                labels: Vec::new(),
            },
            assert_: Some(Property::from_source(source.to_string())),
        }
    }

    fn max_duration(id: &str) -> Sla {
        Sla::MaxDuration {
            common: SlaCommon {
                id: id.to_string(),
                behavior: Some(SlaBehavior::Fail),
                labels: Vec::new(),
            },
            duration: Some("PT0.001S".to_string()),
        }
    }

    fn flow_with(sla: Vec<Sla>) -> Flow {
        let mut f: Flow = crate::spec::parse(
            "id: f\nnamespace: n\ntasks:\n  - id: t\n    type: io.kestra.plugin.core.log.Log\n    message: x\n",
        )
        .expect("the fixture flow must parse");
        f.sla = sla;
        f
    }

    /// Render the source verbatim — the identity a literal `Property` has
    /// anyway (`Property::render`'s "a source with no `{` renders to itself").
    fn verbatim(p: &Property<String>) -> Option<String> {
        Some(p.source().to_string())
    }

    /// **The whole of `processViolation`'s state half, both directions.**
    ///
    /// One-sided would pass on a function that answered `None` to everything,
    /// which is the shape of the two rows that matter here.
    #[test]
    fn each_behavior_maps_to_the_state_kestra_marks_the_execution_as() {
        assert_eq!(
            state_for(Some(SlaBehavior::Fail)),
            Some(RunState::Failed),
            "`case FAIL -> markAs(FAILED)`"
        );
        assert_eq!(
            state_for(Some(SlaBehavior::Cancel)),
            Some(RunState::Cancelled),
            "`case CANCEL -> markAs(CANCELLED)`, which is the whole reason \
             `RunState::Cancelled` exists"
        );
        assert_eq!(
            state_for(Some(SlaBehavior::None)),
            None,
            "`case NONE -> executor.getExecution()` -- unchanged. Only the \
             labels move, and this crate has nowhere to put them"
        );
        assert_eq!(
            state_for(None),
            None,
            "an absent `behavior:` -- `spec::validate` refuses it at publish; \
             here it must not invent a terminal"
        );
    }

    /// **A falsy assertion violates and a truthy one does not**, with the
    /// truthy row as the control that stops an always-violating implementation
    /// passing.
    #[test]
    fn a_falsy_assertion_is_the_violation_and_a_truthy_one_is_not() {
        let violated = first_execution_changed_violation(
            &flow_with(vec![assertion("a", Some(SlaBehavior::Cancel), "false")]),
            &mut verbatim,
        )
        .expect("`false` is falsy and must violate");
        assert_eq!(violated.sla_id, "a");
        assert_eq!(violated.behavior, Some(SlaBehavior::Cancel));
        assert_eq!(
            violated.reason, "assertion is false: false.",
            "`ExecutionAssertionSLA.evaluate`'s own sentence, over the \
             unrendered source"
        );
        assert_eq!(
            first_execution_changed_violation(
                &flow_with(vec![assertion("a", Some(SlaBehavior::Cancel), "true")]),
                &mut verbatim,
            ),
            None
        );
    }

    /// **`TruthUtils`'s four exact strings, not a boolean parse** — the rows
    /// that separate the two readings.
    ///
    /// `"abc"` and `"null"` are TRUTHY on the artifact, so an implementation
    /// that parsed the rendered text as a boolean and treated a parse failure
    /// as false would violate on both. The padded row pins that there is a trim
    /// at all.
    #[test]
    fn truthiness_is_kestras_and_not_a_boolean_parse() {
        for truthy in ["true", "abc", "null", "1", "00", "FALSE", " true "] {
            assert_eq!(
                first_execution_changed_violation(
                    &flow_with(vec![assertion("a", Some(SlaBehavior::Cancel), truthy)]),
                    &mut verbatim,
                ),
                None,
                "{truthy:?} is truthy to `TruthUtils.isTruthy` and must not violate"
            );
        }
        for falsy in ["false", "0", "-0", "", "  false  "] {
            assert!(
                first_execution_changed_violation(
                    &flow_with(vec![assertion("a", Some(SlaBehavior::Cancel), falsy)]),
                    &mut verbatim,
                )
                .is_some(),
                "{falsy:?} is falsy to `TruthUtils.isTruthy` and must violate"
            );
        }
    }

    /// **A `MAX_DURATION` is not evaluated here, and the control is an
    /// assertion in the same list that IS.**
    ///
    /// A zero alone cannot tell "the filter works" from "nothing is ever
    /// evaluated": the second row supplies a violation through the same call.
    #[test]
    fn a_max_duration_is_filtered_out_and_an_assertion_beside_it_is_not() {
        assert_eq!(
            first_execution_changed_violation(&flow_with(vec![max_duration("d")]), &mut verbatim),
            None,
            "`ExecutionChangedSLA.class::isInstance` excludes it; it is the \
             SLA MONITOR's, and this harness has no window for it -- see the \
             module doc"
        );
        let both = first_execution_changed_violation(
            &flow_with(vec![
                max_duration("d"),
                assertion("a", Some(SlaBehavior::Fail), "false"),
            ]),
            &mut verbatim,
        )
        .expect("the assertion behind the MAX_DURATION must still be reached");
        assert_eq!(
            both.sla_id, "a",
            "the filter must SKIP a `MAX_DURATION` rather than stop at it"
        );
    }

    /// **Only the first violation is returned** — `violations.getFirst()`.
    ///
    /// Both SLAs below violate, and they carry different behaviours, so a
    /// reader that took the last one would produce a different terminal.
    #[test]
    fn the_first_violation_wins_and_the_second_is_not_consulted() {
        let v = first_execution_changed_violation(
            &flow_with(vec![
                assertion("first", Some(SlaBehavior::Cancel), "false"),
                assertion("second", Some(SlaBehavior::Fail), "false"),
            ]),
            &mut verbatim,
        )
        .expect("both violate");
        assert_eq!(
            (v.sla_id.as_str(), v.behavior),
            ("first", Some(SlaBehavior::Cancel))
        );
        assert_eq!(state_for(v.behavior), Some(RunState::Cancelled));

        // …and declaration order is what decides it, not the behaviour.
        let swapped = first_execution_changed_violation(
            &flow_with(vec![
                assertion("second", Some(SlaBehavior::Fail), "false"),
                assertion("first", Some(SlaBehavior::Cancel), "false"),
            ]),
            &mut verbatim,
        )
        .expect("both violate");
        assert_eq!(swapped.sla_id, "second");
        assert_eq!(state_for(swapped.behavior), Some(RunState::Failed));
    }

    /// **A violating SLA that is not first does not reach the execution, and a
    /// NON-violating first one does not hide it.**
    ///
    /// `valids/sla-execution-condition`'s exact shape: two assertions, the
    /// first `CANCEL` and the second `NONE` with labels. Driven with the input
    /// the harvest posted, the first violates; driven with the alternative the
    /// Kestra test posts for the LABEL case, the first does not and the second
    /// does — and the answer is then `None`, which is why that alternative
    /// leaves no trace the oracle could compare.
    #[test]
    fn a_none_behaviour_violation_changes_no_state_even_when_it_is_first() {
        let v = first_execution_changed_violation(
            &flow_with(vec![
                assertion("cancel", Some(SlaBehavior::Cancel), "true"),
                assertion("label", Some(SlaBehavior::None), "false"),
            ]),
            &mut verbatim,
        )
        .expect("the second violates");
        assert_eq!(v.sla_id, "label");
        assert_eq!(
            state_for(v.behavior),
            None,
            "`behavior: NONE` only adds labels, and this crate has nowhere to \
             put them -- the execution runs to completion"
        );
    }

    /// **A render that will not resolve SKIPS the SLA**, which is
    /// `evaluateExecutionChangedSLA`'s `try/catch`, and is the OPPOSITE of what
    /// the same accident does to a `runIf:`.
    ///
    /// The control is the second SLA: it is reached, so "skipped" is not
    /// "aborted".
    #[test]
    fn an_unrenderable_assertion_is_skipped_rather_than_violated() {
        let mut render = |p: &Property<String>| match p.source() {
            "{{ nosuch }}" => None,
            other => Some(other.to_string()),
        };
        assert_eq!(
            first_execution_changed_violation(
                &flow_with(vec![assertion(
                    "bad",
                    Some(SlaBehavior::Fail),
                    "{{ nosuch }}"
                )]),
                &mut render,
            ),
            None,
            "Kestra logs the error and yields `Optional.empty()`; it does NOT \
             fail the execution"
        );
        let after = first_execution_changed_violation(
            &flow_with(vec![
                assertion("bad", Some(SlaBehavior::Fail), "{{ nosuch }}"),
                assertion("good", Some(SlaBehavior::Cancel), "false"),
            ]),
            &mut render,
        )
        .expect("the SLA after an unrenderable one must still be evaluated");
        assert_eq!(after.sla_id, "good");
    }

    /// A flow with no `sla:` at all asks the renderer nothing —
    /// `handleExecutionChangedSLA`'s `ListUtils.isEmpty` short-circuit, and the
    /// reason `worker::claim::settle` can call this on all 294 corpus entries.
    #[test]
    fn a_flow_with_no_sla_never_renders_anything() {
        let mut calls = 0usize;
        let mut count = |p: &Property<String>| {
            calls += 1;
            Some(p.source().to_string())
        };
        assert_eq!(
            first_execution_changed_violation(&flow_with(Vec::new()), &mut count),
            None
        );
        assert_eq!(calls, 0);
        // The control: one assertion and the renderer IS asked.
        let mut calls = 0usize;
        let mut count = |p: &Property<String>| {
            calls += 1;
            Some(p.source().to_string())
        };
        assert_eq!(
            first_execution_changed_violation(
                &flow_with(vec![assertion("a", Some(SlaBehavior::Cancel), "true")]),
                &mut count
            ),
            None
        );
        assert_eq!(calls, 1, "a zero above must be falsifiable");
    }
}
