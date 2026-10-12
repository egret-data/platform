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
//! ERROR against the task run; egret stores it on the run (migration 0137)
//! and, since W11, `claim::run_one` also writes it to the attempt's log as
//! the failed-attempt line.
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
//! | a NEGATIVE duration in any spelling or case — `PT-1S`, `pt-1s`, `-PT1S`, `P-1D`, … | FAILED, the same sentence | the same, verbatim; see [`is_negative`], whose first version got the case wrong |
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
//! there. In egret they are not quite: `executor::build_plan` plans
//! `WorkingDirectory` and `AllowFailure` as opaque `TaskKind::Runnable`s, so
//! one of those carrying a `timeout:` is claimable by this worker.
//!
//! **`WaitFor`/`LoopUntil` was the third member of that list and W8 Task 4
//! removed it, exactly as Task 3 removed `Pause` below.** Both spellings reach
//! `models::TaskKind::LoopUntil` now (`LoopUntil.java` registers `WaitFor` as
//! an alias on itself) and `build_plan` gives them a
//! `plan::TaskKind::LoopUntil`, so `claim::persist` writes no `type_id` and the
//! claim never takes one. The residue is per-TASK rather than per-type: a
//! `LoopUntil` whose `checkFrequency:` bound this pure builder cannot read is
//! still planned opaque (`executor::plan::loop_until_gap`) -- and, since
//! no-silent-stall Task 1, refused at validate as
//! `unsupported_loop_until_property`, so no execution of it starts -- and no
//! corpus flow writes one.
//!
//! **`Pause` was the fourth member of that list and the one the divergence was
//! written about, and W8 Task 3 removed it.** It is a `TaskKind::Pause` now:
//! `claim::persist` writes it no `type_id`, the claim's own
//! `WHERE type_id IS NOT NULL` never takes it, and a `Pause`'s `timeout:` is
//! enforced by `advance()` (`executor::flowable::pause`) where the artifact's
//! own validator says it belongs. So the divergence this section names is
//! **narrower** than it was, not gone: the three above are still claimable and
//! still flowables.
//!
//! **This is a PLAN-level list and stays true for both spellings, but only
//! `LoopUntil` is a `models::TaskKind` flowable.** `WaitFor` is Kestra's alias
//! for `LoopUntil` and not egret's: `models::kind_for` has no `"waitfor"` row,
//! so it reaches `models::TaskKind::Leaf`. Nothing above changes --
//! `build_plan` plans it opaque either way, which is all this module reads --
//! and the distinction is recorded because W8 Task 1 fix round 5 found two doc
//! comments in `worker/` that drew the model-level conclusion from the
//! plan-level fact and got it backwards. (`Pause` was named here as the second
//! model flowable; W8 Task 3 gave it a `models::TaskKind::Pause` AND a plan
//! kind, so it left both halves of the sentence at once.)
//!
//! Three of the four baselined entries that declared a `timeout:` were exactly
//! that — `valids/pause-timeout`, `valids/pause-timeout-allow-failure`,
//! `valids/pause-errors-finally-after-execution`. **Since W8 Task 3 they are
//! two of three**: `pause-timeout` is banked, because a `Pause`'s timeout is
//! `advance()`'s and `advance()` now enforces it
//! (`executor::plan::PlanTask::timeout` cites the artifact for the division;
//! `executor::flowable::pause` is where the enforcement lives).
//!
//! **This list named `Subflow` first until W8 Task 1 fix round 1**, and it had
//! been false since W6 Task 3 made a `Subflow` a `TaskKind::Executable` — whose
//! `type_id` is never written, so the claim cannot take one at all. Where the
//! correction came from is worth recording: W8 Task 1 swept every list in this
//! crate enumerating the opaque types, **certified this one as the only correct
//! prose list, and did so from a transcription that had silently dropped
//! `Subflow`**. It was wrong in exactly the way the seven it caught were wrong.
//! The census is **eight lists — seven prose and the derivation they are
//! checked against — and all seven prose lists disagreed.**
//!
//! Nothing reaches that hazard, and the reason has been wrong twice. It first
//! said the claim aborts with `NoHandler` *"before any budget is applied"*,
//! which its own next clause contradicted: the budget is applied **first**.
//! `claim::run_one` calls `cancel::guarded`, which is
//! `timeout::guard(budget, reg.run(task))` — the guard **wraps** the run future
//! rather than gating it — and the instrumented oracle measured the budget
//! applied **4** times, **three of them `Pause`es**. What stopped the hazard
//! was what happened inside that wrapper: no handler is registered for `Pause`,
//! so `Registry::run` refused with
//! [`WorkerError::NoHandler`](super::WorkerError), and because the guard wraps
//! rather than replaces it the refusal still won — the budget expired **0**
//! times.
//!
//! **W8 Task 3 removed the hazard at its source and the 4 is now stale.** A
//! `Pause` gets no `type_id`, so the claim never takes one and this module
//! never sees one; the three `Pause` applications in that measurement cannot
//! happen on this build, leaving `valids/full`'s hour-long `core.log.Log`. The
//! figure is NOT re-instrumented here — it was an application count and nothing
//! re-ran the instrument — so it is corrected as a claim and whoever needs the
//! number must measure it again. `tests/executor.rs`'s
//! `the_baselined_timeout_declarations_are_derived_and_none_can_move_the_ratchet`
//! carries the same correction beside the DECLARATION count it does derive.
//!
//! The written-down hazard did its job: *"whoever implements `Pause` must
//! decide where its `timeout:` is enforced, and must not inherit this one by
//! default."* W8 Task 3 decided `advance()`, which is the artifact's own
//! division.

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
    /// evaluation directly — **except a script or container task**, which
    /// [`defaulted`] turns into [`Budget::Of`] the configured default (W5a).
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

/// Whether `source` is a duration Java **would** parse and this parser refuses
/// *because it is negative*, as opposed to text that is not a duration at all.
///
/// # Fix round 1: the branch this replaces was case-SENSITIVE
///
/// It matched the literal prefixes `PT-`, `P-` and `-P`, while
/// [`parse_duration`] is deliberately case-**in**sensitive — `java.time.Duration.parse`
/// accepts either case and `valids/retry-expo` writes `interval: PT0.25s`. So
/// `pt-1s` fell through to `timeout: "pt-1s" is not an ISO-8601 duration`,
/// contradicting this module's own "verbatim" table. Two of the three prefixes
/// were also reachable by no test at all.
///
/// # What the artifact actually does, measured rather than assumed
///
/// Nine spellings run through
/// `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`,
/// each on a task whose body would otherwise sleep three seconds:
///
/// | `timeout:` | the artifact |
/// |---|---|
/// | `PT-1S`, `pt-1s`, `Pt-1S` | FAILED, `timeout: must be a positive duration` |
/// | `-PT1S`, `-pt1s` | FAILED, same sentence |
/// | `P-1D`, `p-1d` | FAILED, same sentence |
/// | `PT1-S` | FAILED, a raw `JsonParseException` — **not** the positivity sentence |
/// | `--PT1S`, `-` | FAILED, a raw parse error |
/// | `+PT1S` | a real budget: `Timeout after 00:00:01.000` |
/// | `pt1s` | a real budget: `Timeout after 00:00:01.000` |
///
/// So the review that reported this was right about the sentence, and the
/// boundary is sharper than "contains a minus": a `-` is a **sign** exactly
/// where Java's grammar puts one — at the very start, or immediately after the
/// `P` or `T` designator. `PT1-S` has a `-` and is *not* a negative duration.
///
/// This function is that rule rather than a longer list of prefixes: strip the
/// `-`s in sign positions and ask [`parse_duration`], which supplies the
/// case-insensitivity for free because it is the same parser that accepts
/// `PT0.25s`. Every row of the table above is a test below.
fn is_negative(source: &str) -> bool {
    let s = source.trim();
    if !s.contains('-') {
        return false;
    }
    let mut unsigned = String::with_capacity(s.len());
    let mut prev: Option<char> = None;
    for (i, c) in s.chars().enumerate() {
        let sign_position = i == 0 || matches!(prev, Some('P' | 'p' | 'T' | 't'));
        prev = Some(c);
        if c == '-' && sign_position {
            continue;
        }
        unsigned.push(c);
    }
    parse_duration(&unsigned).is_some()
}

/// A script or container task with no `timeout:` runs under
/// `script_default` (W5a spec §6); every other budget, and every other type,
/// is returned unchanged.
///
/// **egret's divergence, not the artifact's**: Kestra runs a script task with
/// no `timeout:` unbounded. See
/// [`DEFAULT_CONTAINER_TIMEOUT`](super::DEFAULT_CONTAINER_TIMEOUT) for why.
/// "Script or container" is [`runs_in_a_container`]: the four script FQCNs,
/// the aliases `egret.container` and `egret.script.python`, and (W5b) every
/// Kestra plugin task, `kestra.*`, which runs in the plugin host's container.
pub(super) fn defaulted(budget: Budget, type_id: &str, script_default: Duration) -> Budget {
    match budget {
        Budget::Absent if runs_in_a_container(type_id) => Budget::Of(script_default),
        other => other,
    }
}

/// The ONE predicate [`defaulted`] reads: a type whose task runs in a
/// container (W5a's script types, W5b's plugin types).
fn runs_in_a_container(type_id: &str) -> bool {
    crate::scripts::kind_of(type_id).is_some() || type_id.starts_with(crate::plugins::PREFIX)
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
        return Budget::Refused(if is_negative(&rendered) {
            NOT_POSITIVE.to_string()
        } else {
            format!("timeout: {rendered:?} is not an ISO-8601 duration")
        });
    };
    if d < MINIMUM {
        return Budget::Refused(NOT_POSITIVE.to_string());
    }
    Budget::Of(d)
}

/// Which of the two ways a guarded handler ended.
///
/// **The outcome alone cannot say.** A budget that expired and a handler that
/// failed on its own both produce `Outcome { state: Failed, .. }`, and the
/// message is the only thing that separates them — so a caller that had to
/// decide whether to kill anything would have to match on a sentence. This
/// enum is what [`super::cancel::guarded`] matches on instead, and it exists
/// because W4 Task 3 gave the expiry a consequence: the artifact calls
/// `task.kill()` there and nowhere else (`AbstractWorkerCallable.callWithTimeout`
/// → `kill(null)` → `WorkerTaskCallable.kill` → `RunnableTask.kill()`), so the
/// expiry has to be distinguishable from every other failure at the type level.
#[derive(Debug, PartialEq)]
pub(super) enum Guarded {
    /// The handler answered inside its budget — with its own success, its own
    /// failure, or no budget at all. Nothing is killed.
    Finished(Outcome),
    /// The budget expired, the future was dropped, and the outcome is the
    /// artifact's own `FAILED` + `Timeout after HH:mm:ss.SSS`.
    Expired(Outcome),
}

impl Guarded {
    /// The outcome either way — what the run commits, which the kill path does
    /// **not** alter (see [`super::cancel`] for the measurement).
    pub(super) fn outcome(self) -> Outcome {
        match self {
            Guarded::Finished(o) | Guarded::Expired(o) => o,
        }
    }
}

/// Run a handler under `budget`.
///
/// **The future is dropped, not signalled.** Kestra interrupts the worker
/// thread (`Timeout.withInterrupt()`, then `kill(null)`); dropping the future
/// is Rust's equivalent and is exactly what
/// [`handlers::sleep`](super::handlers::sleep)'s `cancel` already says a
/// caller should do.
///
/// **Dropping is not the whole of the artifact's answer, and this function is
/// still not where the rest of it lives.** `callWithTimeout` also calls
/// `kill(null)`, which reaches the task's own `RunnableTask.kill()`; egret's
/// counterpart is [`TaskHandler::cancel`](super::TaskHandler) and its caller is
/// [`super::cancel::guarded`], which wraps this function. Keeping the two apart
/// is not tidiness: `guard` is generic over any future and has no registry to
/// dispatch a cancel through, and a version that took one would make every
/// caller of the budget also a caller of the kill path.
///
/// A worker failure (`Err`) is passed through unchanged: the budget decides
/// how long a handler may take, not whether the tick may abort.
pub(super) async fn guard<F>(budget: Option<Duration>, fut: F) -> Result<Guarded>
where
    F: std::future::Future<Output = Result<Outcome>>,
{
    let Some(budget) = budget else {
        return fut.await.map(Guarded::Finished);
    };
    match tokio::time::timeout(budget, fut).await {
        Ok(outcome) => outcome.map(Guarded::Finished),
        // FAILED, not KILLED, and with the artifact's own sentence -- see the
        // module doc for both measurements.
        Err(_elapsed) => Ok(Guarded::Expired(Outcome::failed_with(exceeded(budget)))),
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

    /// **W5b: a plugin task with no `timeout:` runs under the container
    /// default** -- the same predicate as a script task's, not a second rule;
    /// its explicit `timeout:` wins; a native task stays unbounded.
    #[test]
    fn a_plugin_task_without_timeout_runs_under_container_default_timeout() {
        let d = Duration::from_secs(777);
        let q = "kestra.io.kestra.plugin.jdbc.postgresql.Query";
        assert_eq!(defaulted(Budget::Absent, q, d), Budget::Of(d));
        assert_eq!(
            defaulted(
                Budget::Absent,
                "kestra.io.kestra.plugin.jdbc.trino.Query",
                d
            ),
            Budget::Of(d)
        );
        assert_eq!(
            defaulted(Budget::Of(Duration::from_secs(5)), q, d),
            Budget::Of(Duration::from_secs(5))
        );
        assert_eq!(
            defaulted(Budget::Absent, "io.kestra.plugin.scripts.shell.Commands", d),
            Budget::Of(d)
        );
        assert_eq!(
            defaulted(Budget::Absent, "io.kestra.plugin.core.log.Log", d),
            Budget::Absent
        );
        // A bare catalog FQCN is not a plugin task (publish refuses it).
        assert_eq!(
            defaulted(Budget::Absent, "io.kestra.plugin.jdbc.postgresql.Query", d),
            Budget::Absent
        );
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

    /// **Every row of [`is_negative`]'s table, and every spelling reachable.**
    ///
    /// Fix round 1: the branch this covers used to match `PT-`, `P-` and `-P`
    /// case-sensitively, so `pt-1s` reported a parse error where the artifact
    /// reports the positivity sentence — and two of the three prefixes were
    /// reachable by no test, which is the guard-nobody-has-seen-fail shape this
    /// package has produced ten times. Each value below was run through the
    /// pinned image; the expectation is what that image did, not what the
    /// grammar suggests.
    #[test]
    fn a_negative_budget_reports_the_positivity_sentence_in_every_spelling_and_case() {
        for source in [
            "PT-1S", "pt-1s", "Pt-1S", // the sign on the seconds field
            "-PT1S", "-pt1s", // the sign on the whole duration
            "P-1D", "p-1d", // the sign on the days field
        ] {
            assert_eq!(
                resolve(Some(&property(source)), &cx()),
                Budget::Refused(NOT_POSITIVE.to_string()),
                "the artifact answers `{NOT_POSITIVE}` for {source:?}"
            );
        }
    }

    /// The other side of the same boundary, which is what stops
    /// [`is_negative`] from being "contains a minus". A `-` outside a sign
    /// position is not a negative duration — the artifact gives `PT1-S`,
    /// `--PT1S` and `-` a raw parse error, not the positivity sentence.
    #[test]
    fn a_minus_outside_a_sign_position_is_not_a_negative_duration() {
        for source in ["PT1-S", "--PT1S", "-"] {
            assert_eq!(
                resolve(Some(&property(source)), &cx()),
                Budget::Refused(format!("timeout: {source:?} is not an ISO-8601 duration")),
                "the artifact answers a parse error for {source:?}, not the \
                 positivity sentence"
            );
        }
        assert!(
            !is_negative("PT1S") && !is_negative("pt1s"),
            "and a duration with no sign at all is not negative"
        );
    }

    /// A leading `+` is a real budget on the artifact
    /// (`+PT1S` → `Timeout after 00:00:01.000`), and `parse_duration` already
    /// strips it — asserted here because nothing else in this module would
    /// notice if that changed.
    #[test]
    fn an_explicitly_positive_budget_is_a_budget() {
        assert_eq!(
            resolve(Some(&property("+PT1S")), &cx()),
            Budget::Of(Duration::from_secs(1))
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
        assert!(
            matches!(out, Guarded::Finished(_)),
            "a handler with no budget cannot expire, so nothing may be killed \
             for it: {out:?}"
        );
        assert_eq!(out.outcome().state, RunState::Success);
    }

    #[tokio::test]
    async fn a_handler_that_finishes_inside_its_budget_keeps_its_own_answer() {
        let out = guard(Some(Duration::from_secs(30)), async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            Ok(Outcome::failed_with("the handler's own reason"))
        })
        .await
        .expect("no worker failure");
        assert!(
            matches!(out, Guarded::Finished(_)),
            "a handler that FAILED on its own is not an expiry, and reporting it \
             as one would kill a handler that had already stopped: {out:?}"
        );
        let out = out.outcome();
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
        assert!(
            matches!(out, Guarded::Expired(_)),
            "the expiry has to be visible to the caller, because the kill path \
             hangs off it and the outcome alone cannot distinguish it from a \
             handler that failed carrying the same sentence: {out:?}"
        );
        let out = out.outcome();
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
