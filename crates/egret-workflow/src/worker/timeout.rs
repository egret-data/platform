//! The task-level `timeout:` — the **worker's**, not `advance()`'s.
//!
//! # Where this was, and what it was replaced with
//!
//! `handlers::sql`'s [`DEFAULT_TIMEOUT`](super::handlers::sql::DEFAULT_TIMEOUT)
//! was a 300-second constant with a comment saying why it was a constant: a
//! task's `timeout:` is taken out of a leaf's `spec` by `models::Task`'s
//! deserializer before any handler sees it, and wiring the two together "means
//! carrying it on `TaskInput`, which is W4's business". This module is that
//! wiring. The constant is still there and is now the **fallback** for a task
//! that declares no `timeout:`, not a ceiling over one that does.
//!
//! # Everything below is measured against the pinned artifact
//!
//! `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`,
//! read two ways — `javap` over the classes inside `/app/kestra`, and a flow
//! actually executed through a standalone server — because the image and the
//! sibling checkout are different builds and this programme has twice drawn a
//! confident conclusion from the wrong one.
//!
//! **Who enforces it.** `WorkerTaskCallable.doCall` reads
//! `workerTask.getTask().getTimeout()`, renders it
//! (`runContext.render(property).as(Duration.class)`) and passes it to
//! `AbstractWorkerCallable.callWithTimeout`. A `null` timeout runs the task
//! with no budget at all; a non-null one is wrapped in a Failsafe
//! `Timeout.builder(d).withInterrupt()`.
//!
//! **What happens at the boundary.** On `TimeoutExceededException`,
//! `callWithTimeout` bumps a metric, calls `kill(null)` — which leaves
//! `killedState` **null**, deliberately — clears the interrupt flag, and hands
//! `new TimeoutExceededException(duration)` to `exceptionHandler`. That method
//! returns `killedState` when it is set and `State.Type.FAILED` when it is
//! not, so a timed-out run is **FAILED and not KILLED**. Confirmed live: a
//! `Sleep` of `PT10S` under `timeout: PT1S` ended
//! `['CREATED','SUBMITTED','RUNNING','FAILED']` in 1.04 s, its execution
//! FAILED, and its successor never started.
//!
//! **The message.** `TimeoutExceededException(Duration)` is
//! `"Timeout after " + DurationFormatUtils.formatDurationHMS(d.toMillis())`,
//! and the bundled commons-lang3 formats that as `HH:mm:ss.SSS` — **hours are
//! not wrapped at 24**. Probed inside the image rather than derived:
//! `PT1S → "Timeout after 00:00:01.000"`, `PT1.5S → "…00:00:01.500"`,
//! `PT2M → "…00:02:00.000"`, `PT30H → "…30:00:00.000"`. Kestra logs it at
//! ERROR against the task run; egret has no workflow-log table
//! ([`LogSink`](super::handlers::LogSink) says so), so it is stored on the run
//! instead — migration 0137.
//!
//! **A `timeout:` on a flowable is ignored**, which is Kestra's own validator
//! speaking (`FlowService` warns "cannot use the 'timeout' property as it's
//! only relevant for runnable tasks"). Measured: a `Sequential` carrying
//! `timeout: PT1S` around a 4-second `Sleep` ran to SUCCESS in 4.2 s. See
//! *The one divergence that is structural* below for where egret does not
//! automatically inherit that.
//!
//! **A timed-out task is an ordinary failure to everything downstream.** With
//! `retry: {constant, PT1S, maxAttempts: 2}` the same flow recorded two
//! attempts and a `RETRYING` between them. Nothing here has to arrange that:
//! the budget produces an `Outcome` whose state is `Failed`, and `advance()`
//! does the rest.
//!
//! # The three refusals, and which wording is egret's own
//!
//! | the author wrote | the artifact does | this module does |
//! |---|---|---|
//! | `timeout: PT0S`, or anything under a millisecond | FAILED, `timeout: must be a positive duration` | the same, verbatim ([`NOT_POSITIVE`]) |
//! | `timeout: "{{ nope }}"` | FAILED, ``Unable to find `nope` used in the expression `{{ nope }}` at line 1`` | FAILED, `timeout: undefined value (in <string>:1)` |
//! | `timeout: not-a-duration` | FAILED, a raw `com.fasterxml.jackson.core.JsonParseException` | FAILED, `timeout: "not-a-duration" is not an ISO-8601 duration` |
//!
//! The second's wording is **not** this module's divergence to fix and is not
//! papered over: egret's renderer does not name the missing variable at all
//! (`undefined value (in <string>:1)`), which is a W2 property that every
//! handler shares — `handlers::ret`'s `an_unrenderable_format_fails_the_run…`
//! runs into the same sentence. What this module owns is that the failure is
//! reported **as a render failure against `timeout`** and not as a bad
//! duration, which is what
//! `a_budget_that_does_not_render_is_refused_as_a_render_failure` pins in both
//! directions.
//!
//! The third is a **deliberate divergence** and the reason is
//! [`handlers::sleep`](super::handlers::sleep)'s: a Jackson parse error names
//! a Java class and a byte offset, and reproducing it would be reproducing an
//! implementation detail rather than a behaviour. The state is the same, the
//! wording is egret's, and the shape matches the sentence `Sleep` already
//! gives for the same mistake on `duration:`.
//!
//! None of the three is refused at publish, on either side: the artifact
//! published all three flows with a 200 and failed them at run time, so egret
//! failing them at run time is the same answer and not a gap.
//!
//! # The one divergence that is structural, and it is stated rather than guarded
//!
//! Kestra's worker never sees a flowable, so "the worker enforces `timeout:`"
//! and "`timeout:` is only relevant for runnable tasks" are the same sentence
//! there. In egret they are not quite: `executor::build_plan` plans `Subflow`,
//! `WorkingDirectory`, `WaitFor`, `AllowFailure` and **`Pause`** as opaque
//! `TaskKind::Runnable`s, so a `Pause` carrying a `timeout:` is claimable by
//! this worker. Three of the four baselined entries that declare a `timeout:`
//! are exactly that — `valids/pause-timeout`,
//! `valids/pause-timeout-allow-failure`,
//! `valids/pause-errors-finally-after-execution` — and a `Pause`'s timeout is
//! `advance()`'s, not this module's (`executor::plan::PlanTask::timeout` cites
//! the artifact for it).
//!
//! Today nothing reaches that hazard: no handler is registered for `Pause`, so
//! the claim aborts with [`WorkerError::NoHandler`](super::WorkerError) before
//! any budget is applied, and the budget wraps `Registry::run` rather than
//! replacing it, so the refusal still wins. **A special case here would be a
//! guard nobody could ever see fail**, which is the trap this package has
//! caught ten times, so the hazard is written down instead: whoever implements
//! `Pause` must decide where its `timeout:` is enforced, and must not inherit
//! this one by default.

use std::time::Duration;

use egret_core::Result;

use super::Outcome;
use crate::executor::parse_duration;
use crate::models::Property;
use crate::runners::{render_str, RenderContext};

/// The shortest budget the artifact accepts: `@DurationMin(millis = 1)`.
///
/// Measured at the boundary in both directions rather than read off an
/// annotation: `timeout: PT0.0005S` failed with [`NOT_POSITIVE`], and
/// `timeout: PT0.001S` was accepted and fired, logging
/// `Timeout after 00:00:00.001`.
pub const MINIMUM: Duration = Duration::from_millis(1);

/// The artifact's own wording for a non-positive budget, kept verbatim so an
/// author reading an egret failure and a Kestra one sees the same sentence.
///
/// Note the noun: `Sleep` says `duration: must be a positive duration` for the
/// same mistake on its own property, and this is the same constraint reported
/// against `timeout`.
pub const NOT_POSITIVE: &str = "timeout: must be a positive duration";

/// Kestra's `TimeoutExceededException` message, reproduced exactly.
///
/// `"Timeout after " + DurationFormatUtils.formatDurationHMS(millis)`, whose
/// pattern in the bundled commons-lang3 is `HH:mm:ss.SSS`. Two properties of
/// that formatter are easy to get wrong and are pinned by
/// `the_timeout_message_is_the_artifacts_own` in this module's tests:
///
/// * the hour field is **not** modular — `PT30H` formats as `30:00:00.000`,
///   not `06:00:00.000`;
/// * sub-millisecond precision is truncated, because Java formats
///   `Duration::toMillis()`.
pub fn exceeded(budget: Duration) -> String {
    let ms = budget.as_millis();
    format!(
        "Timeout after {:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        (ms / 60_000) % 60,
        (ms / 1_000) % 60,
        ms % 1_000
    )
}

/// What a run's `timeout:` resolved to, before the handler is called.
///
/// Three answers rather than a `Result<Option<Duration>, String>` because the
/// caller does three different things with them, and a shape that made
/// "absent" and "refused" both the empty case is how a refusal turns into an
/// unbounded run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Budget {
    /// The task declares no `timeout:`. The handler runs unbounded — the
    /// artifact's `null` branch in `callWithTimeout`, which calls the
    /// evaluation directly.
    Absent,
    /// The author's budget, rendered and parsed.
    Of(Duration),
    /// The `timeout:` itself is wrong, and the run **fails without being
    /// started**, carrying this message. The artifact does the same: all three
    /// bad spellings produce a FAILED task run whose only attempt never ran
    /// the task body.
    Refused(String),
}

impl Budget {
    /// The budget to enforce, and `None` for both of the other two — which is
    /// safe only because [`Budget::Refused`] never reaches a handler at all.
    pub(super) fn duration(&self) -> Option<Duration> {
        match self {
            Budget::Of(d) => Some(*d),
            _ => None,
        }
    }
}

/// Resolve one task's `timeout:` against the context its handler will render
/// against.
///
/// **Rendered per run, not at parse time**, which is the artifact's own order:
/// `WorkerTaskCallable.doCall` renders the property inside the worker, on the
/// same `RunContext` the task body gets, so `timeout: "{{ inputs.budget }}"`
/// is a supported thing to write and is resolved with everything else this
/// execution knows.
pub(super) fn resolve(timeout: Option<&Property<Duration>>, cx: &RenderContext) -> Budget {
    let Some(property) = timeout else {
        return Budget::Absent;
    };
    // `Property::<Duration>::render` cannot be used: `std::time::Duration`'s
    // serde form is `{secs, nanos}`, not an ISO-8601 string. This is the same
    // split `handlers::sleep` documents — render as text, then parse with
    // `executor::parse_duration`, which is Java's `Duration.parse` over the
    // subset a flow can write.
    let rendered = match render_str(property.source(), cx) {
        Ok(text) => text,
        Err(e) => return Budget::Refused(format!("timeout: {e}")),
    };
    let Some(d) = parse_duration(&rendered) else {
        // `parse_duration` refuses a negative duration outright, where Java
        // parses it and the constraint then rejects it; the observable answer
        // is the same sentence, so both spellings report it. Identical to
        // `handlers::sleep`'s split for `duration:`.
        return Budget::Refused(
            if rendered.starts_with("PT-")
                || rendered.starts_with("P-")
                || rendered.starts_with("-P")
            {
                NOT_POSITIVE.to_string()
            } else {
                format!("timeout: {rendered:?} is not an ISO-8601 duration")
            },
        );
    };
    if d < MINIMUM {
        return Budget::Refused(NOT_POSITIVE.to_string());
    }
    Budget::Of(d)
}

/// Run a handler under `budget`.
///
/// **The future is dropped, not signalled.** Kestra interrupts the worker
/// thread (`Timeout.withInterrupt()`, then `kill(null)`); dropping the future
/// is Rust's equivalent and is exactly what
/// [`handlers::sleep`](super::handlers::sleep)'s `cancel` already says a
/// caller should do. It is **not** [`TaskHandler::cancel`](super::TaskHandler),
/// which still has no caller — W4 Task 3 owns the kill path, and calling
/// `cancel` from here would be that task's work done in the wrong place and
/// without its test.
///
/// A worker failure (`Err`) is passed through unchanged: the budget decides
/// how long a handler may take, not whether the tick may abort.
pub(super) async fn guard<F>(budget: Option<Duration>, fut: F) -> Result<Outcome>
where
    F: std::future::Future<Output = Result<Outcome>>,
{
    let Some(budget) = budget else {
        return fut.await;
    };
    match tokio::time::timeout(budget, fut).await {
        Ok(outcome) => outcome,
        // FAILED, not KILLED, and with the artifact's own sentence -- see the
        // module doc for both measurements.
        Err(_elapsed) => Ok(Outcome::failed_with(exceeded(budget))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::RunState;

    fn cx() -> RenderContext {
        RenderContext::new()
    }

    fn property(source: &str) -> Property<Duration> {
        Property::from_source(source.to_string())
    }

    /// The six pairs probed inside the pinned image, by reflection over
    /// `TimeoutExceededException(Duration)` — not derived from a format string
    /// read off a class file, because the pattern changed between commons-lang3
    /// versions (`H:mm:ss.SSS` in older ones) and the bundled version is the
    /// only one that decides.
    #[test]
    fn the_timeout_message_is_the_artifacts_own() {
        for (millis, expected) in [
            (1_000, "Timeout after 00:00:01.000"),
            (1_500, "Timeout after 00:00:01.500"),
            (120_000, "Timeout after 00:02:00.000"),
            (0, "Timeout after 00:00:00.000"),
            (108_000_000, "Timeout after 30:00:00.000"),
            (250, "Timeout after 00:00:00.250"),
        ] {
            assert_eq!(exceeded(Duration::from_millis(millis)), expected);
        }
    }

    /// Sub-millisecond precision is truncated, because Java formats
    /// `Duration.toMillis()` and not the nanoseconds.
    #[test]
    fn the_message_truncates_below_a_millisecond() {
        assert_eq!(
            exceeded(Duration::from_micros(1_999)),
            "Timeout after 00:00:00.001"
        );
    }

    #[test]
    fn an_absent_timeout_is_no_budget_at_all() {
        assert_eq!(resolve(None, &cx()), Budget::Absent);
        assert_eq!(Budget::Absent.duration(), None);
    }

    #[test]
    fn a_literal_budget_is_parsed_as_java_parses_it() {
        assert_eq!(
            resolve(Some(&property("PT1S")), &cx()),
            Budget::Of(Duration::from_secs(1))
        );
        // Lower case, which `executor::parse_duration` accepts because
        // `java.time.Duration.parse` does.
        assert_eq!(
            resolve(Some(&property("pt0.25s")), &cx()),
            Budget::Of(Duration::from_millis(250))
        );
        assert_eq!(
            resolve(Some(&property("PT60M")), &cx()),
            Budget::Of(Duration::from_secs(3_600))
        );
    }

    /// The boundary, both sides of it, as measured on the artifact.
    #[test]
    fn a_budget_under_a_millisecond_is_refused_and_one_millisecond_is_not() {
        assert_eq!(
            resolve(Some(&property("PT0.0005S")), &cx()),
            Budget::Refused(NOT_POSITIVE.to_string())
        );
        assert_eq!(
            resolve(Some(&property("PT0S")), &cx()),
            Budget::Refused(NOT_POSITIVE.to_string())
        );
        assert_eq!(
            resolve(Some(&property("PT0.001S")), &cx()),
            Budget::Of(Duration::from_millis(1))
        );
    }

    #[test]
    fn a_negative_budget_reports_the_positivity_sentence_rather_than_a_parse_error() {
        assert_eq!(
            resolve(Some(&property("PT-1S")), &cx()),
            Budget::Refused(NOT_POSITIVE.to_string())
        );
    }

    #[test]
    fn an_unparseable_budget_names_what_was_written() {
        assert_eq!(
            resolve(Some(&property("not-a-duration")), &cx()),
            Budget::Refused("timeout: \"not-a-duration\" is not an ISO-8601 duration".to_string())
        );
    }

    /// A `timeout:` is rendered per run, so a missing variable is the
    /// **render's** error and not a duration complaint. The artifact says
    /// ``Unable to find `nope` used in the expression `{{ nope }}` at line 1``
    /// and fails the run; egret's renderer says `undefined value (in
    /// <string>:1)` and does not name the variable, which is W2's wording and
    /// is measured here rather than wished for — the first draft of this test
    /// asserted `message.contains("nope")` and was wrong about egret, not
    /// about Kestra.
    ///
    /// **Both directions.** A parseable budget must NOT take this path, or
    /// "everything is a render failure" would satisfy the first half.
    #[test]
    fn a_budget_that_does_not_render_is_refused_as_a_render_failure() {
        let Budget::Refused(message) = resolve(Some(&property("{{ nope }}")), &cx()) else {
            panic!("a template that cannot render must refuse the run");
        };
        assert_eq!(
            message, "timeout: undefined value (in <string>:1)",
            "the refusal is reported against `timeout` and carries the \
             renderer's own words"
        );
        assert!(
            !message.contains("ISO-8601"),
            "a render failure must not be reported as a bad duration: {message}"
        );
        assert_eq!(
            resolve(Some(&property("{{ 'PT1S' }}")), &cx()),
            Budget::Of(Duration::from_secs(1)),
            "and an expression that DOES render is a budget, not a refusal"
        );
    }

    #[tokio::test]
    async fn no_budget_lets_a_handler_run_as_long_as_it_likes() {
        let out = guard(None, async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            Ok(Outcome::success())
        })
        .await
        .expect("no worker failure");
        assert_eq!(out.state, RunState::Success);
    }

    #[tokio::test]
    async fn a_handler_that_finishes_inside_its_budget_keeps_its_own_answer() {
        let out = guard(Some(Duration::from_secs(30)), async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok(Outcome::failed_with("the handler's own reason"))
        })
        .await
        .expect("no worker failure");
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(out.message.as_deref(), Some("the handler's own reason"));
    }

    #[tokio::test]
    async fn a_handler_that_outruns_its_budget_is_dropped_and_the_run_is_failed() {
        let ran_to_completion = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = ran_to_completion.clone();
        let out = guard(Some(Duration::from_millis(50)), async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Outcome::success())
        })
        .await
        .expect("a timeout is a task failure, not a worker failure");
        assert_eq!(
            out.state,
            RunState::Failed,
            "FAILED and not KILLED: `callWithTimeout` calls `kill(null)` so that \
             `exceptionHandler` falls through to State.Type.FAILED"
        );
        assert_eq!(out.message.as_deref(), Some("Timeout after 00:00:00.050"));
        assert!(
            !ran_to_completion.load(std::sync::atomic::Ordering::SeqCst),
            "the handler must be DROPPED at the budget, not relabelled after it \
             finished -- a wrapper that awaited the future and then rewrote the \
             outcome would pass every other assertion here"
        );
    }

    /// A worker failure is not a task failure, and the budget must not turn one
    /// into the other: `Registry::run`'s `Err` aborts the tick and leaves the
    /// execution where it was.
    #[tokio::test]
    async fn a_worker_failure_inside_the_budget_is_still_a_worker_failure() {
        let out = guard(Some(Duration::from_secs(30)), async {
            Err(egret_core::Error::Internal(anyhow::anyhow!(
                "the seam broke"
            )))
        })
        .await;
        assert!(
            out.is_err(),
            "an Err must pass through the budget unchanged"
        );
    }
}
