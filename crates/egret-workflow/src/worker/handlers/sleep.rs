//! `core.flow.Sleep` — a `RunnableTask` despite the module it lives in, and
//! the one place the checkout and the running artifact disagree.
//!
//! # `PT0S` is refused, and it is refused at RUN time
//!
//! W3b's plan carried the claim that a `Sleep`'s duration must be positive,
//! attributed to the W3a spike, and W3b Task 1 could not confirm it: the
//! checkout's `Sleep.java:52-53` is `@NotNull private Property<Duration>
//! duration;` with no positivity constraint, and `TimeUnit.MILLISECONDS
//! .sleep(0)` does not throw. The plan then guessed that the refusal "came
//! from somewhere else in the publish path". **Both readings are wrong, and
//! the artifact settles it.**
//!
//! Measured against `kestra/kestra:v2.0.0-rc13`:
//!
//! | duration | `POST /flows/validate` | `POST /flows` | execution |
//! |---|---|---|---|
//! | `PT0S` | 200 | 200 | **FAILED**, `duration: must be a positive duration` |
//! | `PT-1S` | 200 | 200 | **FAILED**, same message |
//! | `PT0.000999S` | 200 | 200 | **FAILED**, same message |
//! | `PT0.001S` | 200 | 200 | SUCCESS |
//! | `PT0.002S`, `PT1S` | 200 | 200 | SUCCESS |
//! | *absent* | 200 with `constraints` | **422** `tasks[s].duration: must not be null` | — |
//!
//! So the publish path refuses only the **absent** duration (`@NotNull`); the
//! positivity rule is a **run-time** refusal. The task's own log carries the
//! stack: `jakarta.validation.ConstraintViolationException: duration: must be
//! a positive duration` at `DefaultRunContext.validate(DefaultRunContext.java
//! :395)` ← `RunContextProperty.validate(:44)` ← `RunContextProperty.as(:78)`
//! ← `Sleep.run(Sleep.java:58)`.
//!
//! And the reason Task 1 could not see it in the source is that **the artifact
//! is not the checkout**. `javap -p -v` on `io/kestra/plugin/core/flow/
//! Sleep.class`, extracted from the running image, shows the field as
//!
//! ```text
//! RuntimeVisibleTypeAnnotations:
//!   0: FIELD, location=[TYPE_ARGUMENT(0)]
//!     org.hibernate.validator.constraints.time.DurationMin(
//!       millis=1l
//!       message="must be a positive duration")
//! ```
//!
//! — a **type-use** annotation on `Property<@DurationMin Duration>`, which the
//! checkout at `96893b589a` (2026-08-17) does not carry at all; no commit in
//! that checkout's history ever touched `DurationMin` in `Sleep.java`. The
//! image tagged `v2.0.0-rc13` reports
//! `org.opencontainers.image.revision=169ca7f25a1b3aeeaaab1206a89b2446a1335a46`
//! and `org.opencontainers.image.version=nightly`, built 2026-09-01, and that
//! revision is not an object in the checkout. **The pin is a mutable tag over
//! a nightly**, which is a fact about every measurement this programme has
//! taken from the artifact, not only this one.
//!
//! `millis = 1` is pinned behaviourally as well as from the bytecode, by the
//! `PT0.000999S`/`PT0.001S` pair above.

use std::time::Duration;

use serde::Deserialize;

use super::super::{Outcome, TaskHandler, TaskInput};
use crate::executor::parse_duration;
use crate::models::Property;

/// `io.kestra.plugin.core.flow.Sleep`.
pub const TYPE_ID: &str = "io.kestra.plugin.core.flow.Sleep";

/// `@DurationMin(millis = 1)` on the artifact — see the module doc.
const MINIMUM: Duration = Duration::from_millis(1);

/// The artifact's own wording, kept verbatim so a user reading an egret
/// failure and a Kestra one sees the same sentence.
const NOT_POSITIVE: &str = "duration: must be a positive duration";

/// `Sleep.java:52-53` — `@NotNull private Property<Duration> duration;`.
///
/// Read as a `Property<String>` rather than a `Property<Duration>` because
/// `std::time::Duration`'s serde form is `{secs, nanos}`, not an ISO-8601
/// string; the ISO parse is [`parse_duration`], which is Java's
/// `Duration.parse` over the subset a flow can write.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Spec {
    duration: Option<Property<String>>,
}

/// Sleep for the rendered duration.
///
/// # The cap, and the one caller that uses it
///
/// [`Sleep::capped`] validates the duration exactly as [`Sleep`] does — the
/// `@DurationMin(millis = 1)` refusal above included — and then waits
/// `min(duration, cap)`. It exists for `oracle-exec/driver.rs`, which runs the
/// whole corpus through this handler and would otherwise wait `PT10M` for
/// `valids/flow-concurrency-sla-fail-child` and `PT30S` three more times.
///
/// **Nothing the oracle compares can see the difference**, and that is a
/// measurement rather than a hope: `oracle-exec/normalise.py` drops every clock
/// from the golden, and egret's own happens-before endpoints are the logical
/// `executions.seq` (migration 0136), not wall-clock instants. A capped sleep
/// therefore changes no state, no history and no ordered pair. It is stated in
/// `oracle-exec/README.md` as a property of the driver, not hidden in it.
#[derive(Debug, Default)]
pub struct Sleep {
    cap: Option<Duration>,
}

impl Sleep {
    /// The product handler: it waits for as long as the author asked.
    pub fn new() -> Self {
        Self { cap: None }
    }

    /// A `Sleep` that validates identically and waits at most `cap`.
    pub fn capped(cap: Duration) -> Self {
        Self { cap: Some(cap) }
    }
}

#[async_trait::async_trait]
impl TaskHandler for Sleep {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome> {
        let spec: Spec = match serde_json::from_value(task.spec.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::failed_with(format!("{TYPE_ID}: {e}"))),
        };
        // Kestra refuses this at PUBLISH (`@NotNull`, measured as a 422), and
        // egret's publish carries no such rule, so the run refuses it instead.
        let Some(duration) = spec.duration else {
            return Ok(Outcome::failed_with("duration: must not be null"));
        };
        let rendered = match duration.render(&task.render) {
            Ok(v) => v,
            Err(e) => return Ok(Outcome::failed_with(e.to_string())),
        };

        // `parse_duration` refuses a negative duration outright (there is no
        // negative `std::time::Duration`), where Java parses it and the
        // constraint then rejects it. The observable answer is the same —
        // measured, `PT-1S` is FAILED with the positivity message — so the two
        // paths report the same sentence rather than pretending to a
        // distinction nothing can see.
        let Some(d) = parse_duration(&rendered) else {
            return Ok(Outcome::failed_with(
                if rendered.starts_with("PT-")
                    || rendered.starts_with("P-")
                    || rendered.starts_with("-P")
                {
                    NOT_POSITIVE.to_string()
                } else {
                    format!("duration: {rendered:?} is not an ISO-8601 duration")
                },
            ));
        };
        if d < MINIMUM {
            return Ok(Outcome::failed_with(NOT_POSITIVE));
        }

        tokio::time::sleep(self.cap.map_or(d, |c| d.min(c))).await;
        // `return null` (`Sleep.java:62`) — a `VoidOutput` task.
        Ok(Outcome::success())
    }

    /// Nothing to cancel **through this method**: the sleep is a future, and
    /// the caller has already dropped it by the time this is called.
    ///
    /// W3b wrote *"W4's pod-backed handlers are where `cancel` has work to
    /// do"*, and W4 Task 3 can now say which half of that came true. The kill
    /// path exists and calls this ([`cancel`](super::super::cancel)); the
    /// handler that has work to do is [`sql`](super::sql), and it **refuses**
    /// rather than pretending — dropping a future does not stop a statement on
    /// a remote engine. For this handler the drop really is the whole of the
    /// stopping, so `Ok(())` is a measurement and not a shrug.
    async fn cancel(&self, _task: &TaskInput) -> egret_core::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::input;
    use super::*;
    use crate::executor::RunState;

    /// The shape: a `Sleep` succeeds and produces **nothing**
    /// (`Sleep.java:47`, `:62`).
    #[tokio::test]
    async fn a_sleep_succeeds_and_produces_no_output() {
        let out = Sleep::new()
            .run(&input(TYPE_ID, serde_json::json!({"duration": "PT0.001S"})))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success);
        assert_eq!(
            out.outputs, None,
            "`Sleep` returns Java null and never instantiates `VoidOutput`"
        );
        assert_eq!(out.message, None);
    }

    /// It really waits. Asserted on the *lower* bound only — an upper bound
    /// would be a timing assertion wearing a correctness costume, and the
    /// oracle's own normaliser drops durations for exactly that reason.
    #[tokio::test]
    async fn a_sleep_actually_waits() {
        let t0 = std::time::Instant::now();
        Sleep::new()
            .run(&input(TYPE_ID, serde_json::json!({"duration": "PT0.05S"})))
            .await
            .unwrap();
        assert!(
            t0.elapsed() >= Duration::from_millis(50),
            "a Sleep that returned immediately would make every `overlapping` \
             ordered pair in the oracle unreachable; elapsed {:?}",
            t0.elapsed()
        );
    }

    /// **The measurement this task was asked for.** `PT0S` is refused, and it
    /// is refused *here* — at run time, by `@DurationMin(millis = 1)` — not by
    /// publish. `PT0.000999S` and `PT-1S` go the same way; `PT0.001S` does
    /// not.
    #[tokio::test]
    async fn a_duration_under_one_millisecond_fails_the_run() {
        for refused in ["PT0S", "PT0.000999S", "PT-1S", "PT0.0005S"] {
            let out = Sleep::new()
                .run(&input(TYPE_ID, serde_json::json!({"duration": refused})))
                .await
                .unwrap();
            assert_eq!(
                out.state,
                RunState::Failed,
                "{refused} is below @DurationMin(millis = 1)"
            );
            assert_eq!(
                out.message.as_deref(),
                Some(NOT_POSITIVE),
                "the artifact's own wording, for {refused}"
            );
        }

        // The control, and it is what makes the four above mean anything: one
        // millisecond is the boundary and it PASSES.
        let out = Sleep::new()
            .run(&input(TYPE_ID, serde_json::json!({"duration": "PT0.001S"})))
            .await
            .unwrap();
        assert_eq!(
            out.state,
            RunState::Success,
            "1ms is the boundary and it is allowed"
        );
    }

    /// Kestra refuses this at publish (422); egret's publish does not, so the
    /// run refuses it. Failing is the safe direction — a `Sleep` with no
    /// duration that succeeded would be an over-acceptance.
    #[tokio::test]
    async fn a_missing_duration_fails_the_run() {
        let out = Sleep::new()
            .run(&input(TYPE_ID, serde_json::json!({})))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(out.message.as_deref(), Some("duration: must not be null"));
    }

    /// Measured: `duration: nonsense` FAILS the run on the artifact (a Jackson
    /// parse error). The state matches; the message is egret's own, and says
    /// so.
    #[tokio::test]
    async fn an_unparseable_duration_fails_the_run() {
        let out = Sleep::new()
            .run(&input(TYPE_ID, serde_json::json!({"duration": "nonsense"})))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message.as_deref().unwrap_or("").contains("ISO-8601"),
            "got {:?}",
            out.message
        );
    }
}
