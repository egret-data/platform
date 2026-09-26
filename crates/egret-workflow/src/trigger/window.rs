// kestra: core/src/main/java/io/kestra/core/models/triggers/TimeWindow.java
// kestra: core/src/main/java/io/kestra/core/models/triggers/multipleflows/MultipleConditionStateStore.java
// kestra: core/src/main/java/io/kestra/core/models/triggers/multipleflows/MultipleConditionWindow.java
// kestra: core/src/main/java/io/kestra/core/validations/validator/WindowValidator.java

//! The `dependsOn` window's bounds — `MultipleConditionStateStore.create` and
//! `MultipleConditionWindow.isValid`, pure over `(window, now, zone)`.
//!
//! # Where the rules come from
//!
//! W13b Task 1 measured the pinned artifact (`oracle-exec/w13b-windows.json`,
//! `bounds_rule`), and the user's decisions D1–D3 bind what egret does with
//! the measurements:
//!
//! - **`DURATION_WINDOW`** (D3): take `now` in the zone with the nanos
//!   dropped; if `every` >= 1 day set the hour to 0; if >= 1 hour set the
//!   minute to 0; if >= 1 minute set the second to 0 and floor the minute to a
//!   multiple of `every`'s whole minutes; `start` = that + `offset`; `end` =
//!   `start` + `every` − 1 ms. Measured for `every` in {P1D, PT6H, PT90M,
//!   PT1H, PT2M, PT45S}; every other `every` follows the same rule, **chosen
//!   from the Java** and consistent with the measured cases.
//! - **`SLIDING_WINDOW`** (D1): `[now truncated to the second, + lookback]`,
//!   fixed once opened.
//! - **`DAILY_TIME_WINDOW`** / **`DAILY_TIME_DEADLINE`**: from the zone's
//!   midnight of `now`'s day, plus `from`/`to`/`deadline`.
//! - **Two nows.** Bounds use `now` truncated to the second; [`is_valid`]
//!   uses the caller's untruncated `now` (egret: Postgres `now()`, in
//!   microseconds), strict at both ends. So the event that opens a sliding or
//!   sub-minute window is itself counted (`grid_every_PT45S`,
//!   `q5_sliding_lookback_PT2M`).
//!
//! # DST — chosen from the Java, never measured
//!
//! Task 1's container was UTC and no case crossed a transition, so how the
//! bounds behave over a DST change is **derived from `java.time`**, not
//! observed:
//!
//! - every `withHour`/`withMinute`/`withSecond` and `truncatedTo(DAYS)` is a
//!   LOCAL edit re-resolved by `ZonedDateTime.ofLocal(ldt, zone,
//!   preferredOffset)`: in an overlap the current offset is kept if valid,
//!   else the earlier one; in a gap the local time moves forward by the gap
//!   ([`resolve_local`]);
//! - every `plus`/`minus` of a `Duration`, `plusMinutes` and `plusSeconds` is
//!   INSTANT arithmetic. So a `P1D` window is always exactly 24 h long (on a
//!   23-hour day it ends an hour into the next local day), and a daily window
//!   `from: 10:00` on a spring-forward day opens at local 11:00, because it is
//!   `midnight.plusSeconds(36000)`, not a wall-clock 10:00.
//!
//! # Checked arithmetic
//!
//! [`TimeWindow::bounds`] returns a `Result` and never panics. The 366-day
//! cap in `duration::parse_duration` applies wherever a window is PARSED —
//! at save and in [`TimeWindow::from_model`] — but the enum's variants are
//! public and a stored row is not re-validated by anything but `from_model`,
//! so the arithmetic itself cannot rely on the cap.
//!
//! # One refusal list for save and run time
//!
//! Drafts save despite validation errors, and `scheduler::sync` gives any
//! parseable draft a listener row. So a window shape refused at save would
//! still reach the processor through a draft. [`window_refusals`] is the one
//! list: `spec::validate` reports it, and [`TimeWindow::from_model`] refuses
//! whenever it is non-empty, so the two cannot drift.

use std::collections::HashMap;
use std::fmt;

use chrono::{
    DateTime, LocalResult, NaiveDateTime, NaiveTime, Offset, TimeDelta, TimeZone, Timelike, Utc,
};
use chrono_tz::Tz;

use super::duration::{parse_duration, parse_local_time};
use crate::models::Window;

/// `TimeWindow.window`'s default, `Duration.ofDays(1)`: no `window:` at all,
/// or a window with no `every`, is a one-day `DURATION_WINDOW` (measured,
/// finding `no_window_default`).
pub const DEFAULT_EVERY: TimeDelta = TimeDelta::days(1);

/// `TimeWindow.Type` with its parameters, parsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeWindow {
    /// `DURATION_WINDOW`: `every` (`TimeWindow.window`) and `offset`
    /// (`TimeWindow.windowAdvance`).
    Duration { every: TimeDelta, offset: TimeDelta },
    /// `SLIDING_WINDOW`: `lookback` (`TimeWindow.window`).
    Sliding { lookback: TimeDelta },
    /// `DAILY_TIME_WINDOW`: `from`/`to` (`startTime`/`endTime`).
    DailyWindow { from: NaiveTime, to: NaiveTime },
    /// `DAILY_TIME_DEADLINE`: `deadline`.
    DailyDeadline { deadline: NaiveTime },
}

/// A window plus the zone its bounds are computed in.
///
/// **A change from the brief's `from_model -> Result<TimeWindow, _>`, and
/// chosen:** the zone is `window.timezone` (D2), so parsing it anywhere but
/// next to the rest of the window would give the processor a second place to
/// forget it and pass UTC. `bounds(now, tz)` on [`TimeWindow`] stays the
/// primitive; [`Self::bounds`] is the call that cannot get the zone wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedWindow {
    pub window: TimeWindow,
    /// `window.timezone`, or UTC when absent (D2: egret has no server zone,
    /// and UTC is what the artifact aligned on with none set).
    pub tz: Tz,
}

impl ResolvedWindow {
    pub fn bounds(&self, now: DateTime<Utc>) -> Result<Bounds, BoundsError> {
        self.window.bounds(now, self.tz)
    }
}

/// `[start, end]` of one window, in UTC. `start < end` always holds for a
/// value [`TimeWindow::bounds`] returns.
pub type Bounds = (DateTime<Utc>, DateTime<Utc>);

/// Why [`TimeWindow::bounds`] produced no window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BoundsError {
    /// A date computation left chrono's representable range. Only reachable
    /// through a `TimeWindow` built directly (the variants are public) or a
    /// `now` at the edge of time: `from_model` caps every duration at 366
    /// days.
    #[error("the window's bounds are outside the representable date range")]
    OutOfRange,
    /// The bounds came out with `start >= end`: a window that no instant is
    /// strictly inside (for example an `every` of one millisecond, which
    /// `from_model` cannot produce). The window row requires `start < end`.
    #[error("the window's bounds are empty: start {start} is not before end {end}")]
    Empty {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
}

/// One reason a declared `window` is refused. `field` is the window field it
/// is about, or `""` for the window as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRefusal {
    pub code: &'static str,
    pub field: &'static str,
    pub message: String,
}

/// Why [`TimeWindow::from_model`] refused a window: every refusal
/// [`window_refusals`] reports, in its order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowError(pub Vec<WindowRefusal>);

impl fmt::Display for WindowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "window refused:")?;
        for r in &self.0 {
            write!(f, " [{}] {};", r.code, r.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for WindowError {}

impl TimeWindow {
    /// `Window.toTimeWindow()` with egret's refusals in front.
    ///
    /// `None` (no `window:`) is `Duration { every: 1 day, offset: 0 }` in
    /// UTC. A window [`window_refusals`] finds anything wrong with is
    /// refused, whole — never approximated.
    pub fn from_model(w: Option<&Window>) -> Result<ResolvedWindow, WindowError> {
        let Some(w) = w else {
            return Ok(ResolvedWindow {
                window: TimeWindow::Duration {
                    every: DEFAULT_EVERY,
                    offset: TimeDelta::zero(),
                },
                tz: chrono_tz::UTC,
            });
        };
        let refusals = window_refusals(w);
        if !refusals.is_empty() {
            return Err(WindowError(refusals));
        }

        // Every value below parsed cleanly inside `window_refusals`. A
        // re-parse failure would be a bug there, and is refused, not
        // unwrapped.
        let tz = match &w.timezone {
            None => chrono_tz::UTC,
            Some(zone) => zone.parse::<Tz>().map_err(|_| reparse("timezone"))?,
        };
        // `Window.toTimeWindow()`'s order, which `window_refusals` has made
        // unambiguous: at most one kind's fields are present.
        let window = if let Some(deadline) = time_of("deadline", &w.deadline)? {
            TimeWindow::DailyDeadline { deadline }
        } else if let (Some(from), Some(to)) = (time_of("from", &w.from)?, time_of("to", &w.to)?) {
            TimeWindow::DailyWindow { from, to }
        } else if let Some(lookback) = duration_of("lookback", &w.lookback)? {
            TimeWindow::Sliding { lookback }
        } else {
            TimeWindow::Duration {
                every: duration_of("every", &w.every)?.unwrap_or(DEFAULT_EVERY),
                offset: duration_of("offset", &w.offset)?.unwrap_or_else(TimeDelta::zero),
            }
        };
        Ok(ResolvedWindow { window, tz })
    }

    /// `MultipleConditionStateStore.create`'s `(start, end)`, for an event at
    /// `now`, computed in `tz`.
    pub fn bounds(&self, now: DateTime<Utc>, tz: Tz) -> Result<Bounds, BoundsError> {
        use BoundsError::OutOfRange;
        let add = |t: DateTime<Tz>, d: TimeDelta| t.checked_add_signed(d).ok_or(OutOfRange);

        // `ZonedDateTime.now().withNano(0)`, in the window's zone.
        let now = now.with_nanosecond(0).ok_or(OutOfRange)?.with_timezone(&tz);

        let (start, end) = match *self {
            TimeWindow::Duration { every, offset } => {
                // `toDays() > 0` / `toHours() > 0` / `toMinutes() > 0`, each a
                // truncating division, exactly as `java.time.Duration`.
                let mut t = now;
                if every.num_days() > 0 {
                    t = with_local(t, |l| l.with_hour(0))?;
                }
                if every.num_hours() > 0 {
                    t = with_local(t, |l| l.with_minute(0))?;
                }
                let every_minutes = every.num_minutes();
                if every_minutes > 0 {
                    // `now.getMinute()` is read before the chain reassigns
                    // `now`: `withSecond(0).withMinute(0).plusMinutes(w *
                    // (minute / w))`.
                    let minute = i64::from(t.minute());
                    t = with_local(t, |l| l.with_second(0))?;
                    t = with_local(t, |l| l.with_minute(0))?;
                    let floor = (minute / every_minutes)
                        .checked_mul(every_minutes)
                        .and_then(TimeDelta::try_minutes)
                        .ok_or(OutOfRange)?;
                    t = add(t, floor)?;
                }
                let start = truncate_to_millis(add(t, offset)?)?;
                let end =
                    truncate_to_millis(add(add(start, every)?, -TimeDelta::milliseconds(1))?)?;
                (start, end)
            }
            TimeWindow::Sliding { lookback } => (now, add(now, lookback)?),
            TimeWindow::DailyWindow { from, to } => {
                let midnight = local_midnight(now)?;
                (
                    add(midnight, second_of_day(from))?,
                    add(midnight, second_of_day(to))?,
                )
            }
            TimeWindow::DailyDeadline { deadline } => {
                let midnight = local_midnight(now)?;
                (midnight, add(midnight, second_of_day(deadline))?)
            }
        };

        let (start, end) = (start.with_timezone(&Utc), end.with_timezone(&Utc));
        if start < end {
            Ok((start, end))
        } else {
            Err(BoundsError::Empty { start, end })
        }
    }

    /// `MultipleConditionWindow.isValid`: `now.isAfter(start) &&
    /// now.isBefore(end)`, strict at both ends, over the UNTRUNCATED `now`.
    pub fn is_valid(start: DateTime<Utc>, end: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        start < now && now < end
    }
}

fn reparse(field: &'static str) -> WindowError {
    WindowError(vec![WindowRefusal {
        code: "window_internal",
        field,
        message: format!("{field} passed the window checks and then failed to parse"),
    }])
}

fn duration_of(field: &'static str, v: &Option<String>) -> Result<Option<TimeDelta>, WindowError> {
    v.as_deref()
        .map(|s| parse_duration(s).map_err(|_| reparse(field)))
        .transpose()
}

fn time_of(field: &'static str, v: &Option<String>) -> Result<Option<NaiveTime>, WindowError> {
    v.as_deref()
        .map(|s| parse_local_time(s).map_err(|_| reparse(field)))
        .transpose()
}

/// `LocalTime.toSecondOfDay()`, as a `Duration` to add on the instant line.
fn second_of_day(t: NaiveTime) -> TimeDelta {
    TimeDelta::seconds(i64::from(t.num_seconds_from_midnight()))
}

/// `ZonedDateTime.truncatedTo(ChronoUnit.DAYS)`: a local edit, re-resolved.
fn local_midnight(t: DateTime<Tz>) -> Result<DateTime<Tz>, BoundsError> {
    with_local(t, |l| Some(l.date().and_time(NaiveTime::MIN)))
}

/// `truncatedTo(ChronoUnit.MILLIS)`. The nanosecond field is never
/// negative, so this floors, as Java does. Done on the UTC instant, not
/// through `DateTime<Tz>::with_nanosecond`: chrono re-resolves that one
/// locally and gives up (`None`) inside a DST overlap. Offsets are whole
/// seconds, so the sub-second part is the same either way.
fn truncate_to_millis(t: DateTime<Tz>) -> Result<DateTime<Tz>, BoundsError> {
    let utc = t.with_timezone(&Utc);
    let ns = utc.nanosecond();
    utc.with_nanosecond(ns - ns % 1_000_000)
        .map(|u| u.with_timezone(&t.timezone()))
        .ok_or(BoundsError::OutOfRange)
}

/// A `ZonedDateTime.withXxx` / `truncatedTo`: edit the LOCAL date-time and
/// re-resolve it with the current offset preferred.
fn with_local(
    t: DateTime<Tz>,
    edit: impl FnOnce(NaiveDateTime) -> Option<NaiveDateTime>,
) -> Result<DateTime<Tz>, BoundsError> {
    let local = edit(t.naive_local()).ok_or(BoundsError::OutOfRange)?;
    resolve_local(t.timezone(), local, t.offset().fix())
}

/// `ZonedDateTime.ofLocal(localDateTime, zone, preferredOffset)`.
///
/// - one valid offset: it;
/// - an overlap: `preferred` if it is one of the two, else the EARLIER
///   offset (`validOffsets.get(0)`, the one before the transition — chrono's
///   `earliest` instant);
/// - a gap: the local time moved later by the gap's length, with the offset
///   after it — which is the same instant as reading `local` with the offset
///   BEFORE the gap. That offset is the zone's at an instant 15 hours before
///   `local`-as-UTC: for any offset in [-12, +14] that instant precedes the
///   transition, and no zone has two transitions that close together.
fn resolve_local(
    tz: Tz,
    local: NaiveDateTime,
    preferred: chrono::FixedOffset,
) -> Result<DateTime<Tz>, BoundsError> {
    match tz.from_local_datetime(&local) {
        LocalResult::Single(t) => Ok(t),
        LocalResult::Ambiguous(earlier, later) => Ok(if later.offset().fix() == preferred {
            later
        } else {
            earlier
        }),
        LocalResult::None => {
            let probe = local
                .checked_sub_signed(TimeDelta::hours(15))
                .ok_or(BoundsError::OutOfRange)?;
            let before = tz.offset_from_utc_datetime(&probe).fix();
            let instant = local
                .checked_sub_signed(TimeDelta::seconds(i64::from(before.local_minus_utc())))
                .ok_or(BoundsError::OutOfRange)?;
            Ok(tz.from_utc_datetime(&instant))
        }
    }
}

/// Every reason a declared window is refused — the ONE list, called by
/// `spec::validate` at save and by [`TimeWindow::from_model`] at run time.
///
/// **Kind and exclusivity follow Kestra's `WindowValidator`**, in its order
/// and with its messages: `deadline` first, then `from` + `to`, then `from`
/// or `to` alone, then `lookback`. The artifact refused `every` + `lookback`
/// with exactly *Window of type `SLIDING_WINDOW` cannot have an every
/// duration.* (measured, `w13b-windows.json` `save_validation`).
///
/// **egret refuses more than Kestra does, and each addition is chosen:**
///
/// - `offset` without `every`. Kestra accepts it (a one-day window, shifted);
///   Task 1 never measured it, so it is refused rather than approximated;
/// - a zero `every` or `lookback`, and a daily window that is empty
///   (`from` >= `to`, or `deadline` at midnight). Kestra saves these and then
///   never fires them; egret's window row requires `start < end`
///   (migration 0158), so they could not even be stored;
/// - a duration over 366 days (`duration::MAX_WINDOW_DURATION`).
///
/// A window on a trigger with no `dependsOn` is refused too, but that is a
/// fact about the trigger, not the window: see
/// `depends_on::depends_on_refusals` and
/// `flow::FlowTriggerError::Refused`.
///
/// Every duration, time and zone present is parsed with the functions
/// [`TimeWindow::from_model`] uses (`trigger::duration`), so a value that
/// passes here is one the processor can read.
pub fn window_refusals(w: &Window) -> Vec<WindowRefusal> {
    let mut out = Vec::new();
    let mut refuse = |code: &'static str, field: &'static str, message: String| {
        out.push(WindowRefusal {
            code,
            field,
            message,
        });
    };

    // `WindowValidator.isValid`, arm for arm.
    let set = |v: &Option<String>| v.is_some();
    let conflicts: [(&'static str, bool, &str); 6] = [
        ("deadline", set(&w.deadline), "a deadline"),
        ("from", set(&w.from), "a from time"),
        ("to", set(&w.to), "a to time"),
        ("every", set(&w.every), "an every duration"),
        ("offset", set(&w.offset), "an offset"),
        ("lookback", set(&w.lookback), "a lookback duration"),
    ];
    let (kind, forbidden): (Option<&str>, &[&str]) = if set(&w.deadline) {
        (
            Some("DAILY_TIME_DEADLINE"),
            &["every", "offset", "from", "to", "lookback"],
        )
    } else if set(&w.from) && set(&w.to) {
        (Some("DAILY_TIME_WINDOW"), &["every", "offset", "lookback"])
    } else if set(&w.from) || set(&w.to) {
        let missing = if set(&w.from) { "to" } else { "from" };
        refuse(
            "window_daily_bound_missing",
            missing,
            format!("Window of type `DAILY_TIME_WINDOW` must have a {missing} time."),
        );
        (None, &[])
    } else if set(&w.lookback) {
        (Some("SLIDING_WINDOW"), &["offset", "every"])
    } else {
        (None, &[])
    };
    if let Some(kind) = kind {
        for (field, present, noun) in conflicts {
            if present && forbidden.contains(&field) {
                refuse(
                    "window_conflicting_fields",
                    field,
                    format!("Window of type `{kind}` cannot have {noun}."),
                );
            }
        }
    }

    // DURATION_WINDOW: `offset` alone is refused (chosen; see above).
    if set(&w.offset) && !set(&w.every) && kind.is_none() {
        refuse(
            "window_offset_without_every",
            "offset",
            "`offset` without `every` is not supported; write `every` as well (`every: P1D` \
             is the default it would shift)"
                .to_string(),
        );
    }

    // Values: each one present must parse, whatever else is wrong.
    for (field, value, positive) in [
        ("every", &w.every, true),
        ("offset", &w.offset, false),
        ("lookback", &w.lookback, true),
    ] {
        let Some(value) = value else { continue };
        match parse_duration(value) {
            Err(e) => refuse("window_duration_invalid", field, format!("{field}: {e}")),
            Ok(d) if positive && d <= TimeDelta::zero() => refuse(
                "window_duration_not_positive",
                field,
                format!("{field} must be longer than zero, got `{value}`"),
            ),
            Ok(_) => {}
        }
    }
    let mut times = HashMap::new();
    for (field, value) in [("deadline", &w.deadline), ("from", &w.from), ("to", &w.to)] {
        let Some(value) = value else { continue };
        match parse_local_time(value) {
            Err(e) => refuse("window_time_invalid", field, format!("{field}: {e}")),
            Ok(t) => {
                times.insert(field, t);
            }
        }
    }
    if let (Some(from), Some(to)) = (times.get("from"), times.get("to")) {
        if from >= to {
            refuse(
                "window_daily_range_empty",
                "to",
                format!(
                    "`from` ({from}) must be earlier than `to` ({to}); a daily window that \
                     wraps past midnight or has no length is not supported"
                ),
            );
        }
    }
    if times.get("deadline") == Some(&NaiveTime::MIN) {
        refuse(
            "window_daily_range_empty",
            "deadline",
            "a deadline at midnight gives a window with no length".to_string(),
        );
    }

    if let Some(zone) = &w.timezone {
        if zone.parse::<Tz>().is_err() {
            refuse(
                "window_timezone_invalid",
                "timezone",
                format!("timezone `{zone}` is not an IANA time zone (for example `Europe/Paris`)"),
            );
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn at(iso: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(iso)
            .unwrap_or_else(|e| panic!("bad instant {iso:?}: {e}"))
            .with_timezone(&Utc)
    }

    fn window(yaml: &str) -> Window {
        serde_norway::from_str(yaml).unwrap_or_else(|e| panic!("{yaml}: {e}"))
    }

    fn resolved(yaml: &str) -> ResolvedWindow {
        TimeWindow::from_model(Some(&window(yaml))).unwrap_or_else(|e| panic!("{yaml}: {e}"))
    }

    fn tz(name: &str) -> Tz {
        name.parse().unwrap()
    }

    // ------------------------------------------------------------------
    // The fixture: every bounds case Task 1 measured
    // ------------------------------------------------------------------

    const FIXTURE: &str = include_str!("../../oracle-exec/w13b-windows.json");

    /// **Every `bounds_cases` entry of `w13b-windows.json`, asserted against
    /// its `egret_expected`** (not against a Kestra lifecycle sequence, which
    /// D1 diverges from). For the one case egret opens no window for
    /// (`q1_all_nowindow`, an unrelated execution), the observed bounds are
    /// still what egret computes for a MATCHING event at that `now` — the
    /// case's own note says so — so those are asserted instead.
    #[test]
    fn every_measured_bounds_case_matches_egret_expected() {
        let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        let cases = fixture["bounds_cases"].as_array().unwrap();
        assert_eq!(cases.len(), 26, "the fixture's case count moved");

        let mut opened = 0;
        let mut counted = 0;
        let mut not_counted = 0;
        for case in cases {
            let name = case["scenario"].as_str().unwrap();
            let declared = &case["window"];
            let w: Window =
                serde_json::from_value(declared.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
            // An empty `window` object in the fixture is "no `window:`".
            let model = if declared.as_object().unwrap().is_empty() {
                assert_eq!(
                    TimeWindow::from_model(Some(&w)),
                    TimeWindow::from_model(None),
                    "{name}: `window: {{}}` and no window must be the same"
                );
                None
            } else {
                Some(&w)
            };
            let r = TimeWindow::from_model(model).unwrap_or_else(|e| panic!("{name}: {e}"));

            let kind = match r.window {
                TimeWindow::Duration { .. } => "DURATION_WINDOW",
                TimeWindow::Sliding { .. } => "SLIDING_WINDOW",
                TimeWindow::DailyWindow { .. } => "DAILY_TIME_WINDOW",
                TimeWindow::DailyDeadline { .. } => "DAILY_TIME_DEADLINE",
            };
            assert_eq!(kind, case["kind"].as_str().unwrap(), "{name}: kind");

            let now = at(case["now"].as_str().unwrap());
            let (start, end) = r.bounds(now).unwrap_or_else(|e| panic!("{name}: {e}"));
            let expected = &case["egret_expected"];

            if expected["opens_window"].as_bool().unwrap() {
                opened += 1;
                assert_eq!(
                    start,
                    at(expected["start"].as_str().unwrap()),
                    "{name}: start"
                );
                assert_eq!(end, at(expected["end"].as_str().unwrap()), "{name}: end");
                let is_counted = expected["event_counted"].as_bool().unwrap();
                assert_eq!(
                    TimeWindow::is_valid(start, end, now),
                    is_counted,
                    "{name}: event_counted"
                );
                if is_counted {
                    counted += 1;
                } else {
                    not_counted += 1;
                }
            } else {
                assert_eq!(
                    start,
                    at(case["observed_start"].as_str().unwrap()),
                    "{name}"
                );
                assert_eq!(end, at(case["observed_end"].as_str().unwrap()), "{name}");
                assert_eq!(
                    TimeWindow::is_valid(start, end, now),
                    case["now_in_window"].as_bool().unwrap(),
                    "{name}: now_in_window"
                );
            }
        }
        // Numbers that can disagree, beside the loop that produced them: a
        // loop that asserted nothing would leave these at zero.
        assert_eq!((opened, counted, not_counted), (25, 21, 4));
    }

    // ------------------------------------------------------------------
    // is_valid: strict at both ends
    // ------------------------------------------------------------------

    #[test]
    fn is_valid_is_strict_at_start_and_at_end() {
        let start = at("2026-09-26T04:00:00Z");
        let end = at("2026-09-26T04:59:59.999Z");
        assert!(!TimeWindow::is_valid(start, end, start), "exactly at start");
        assert!(!TimeWindow::is_valid(start, end, end), "exactly at end");
        let micro = TimeDelta::microseconds(1);
        assert!(TimeWindow::is_valid(start, end, start + micro));
        assert!(TimeWindow::is_valid(start, end, end - micro));
        assert!(!TimeWindow::is_valid(start, end, start - micro));
        assert!(!TimeWindow::is_valid(start, end, end + micro));
    }

    /// The two nows: bounds from `now` truncated to the second, validity from
    /// the untruncated one — so the event that opens a sliding window counts,
    /// and one landing on a whole second does not.
    #[test]
    fn the_opening_event_of_a_sliding_window_counts_unless_it_is_on_the_second() {
        let r = resolved("lookback: PT2M");
        let now = at("2026-09-26T05:04:21.721936Z");
        let (start, end) = r.bounds(now).unwrap();
        assert_eq!(start, at("2026-09-26T05:04:21Z"));
        assert_eq!(end, at("2026-09-26T05:06:21Z"));
        assert!(TimeWindow::is_valid(start, end, now));

        let on_the_second = at("2026-09-26T05:04:21Z");
        let (start, end) = r.bounds(on_the_second).unwrap();
        assert!(!TimeWindow::is_valid(start, end, on_the_second));
    }

    // ------------------------------------------------------------------
    // Shapes Task 1 did not measure (D3: chosen from the Java)
    // ------------------------------------------------------------------

    #[test]
    fn unmeasured_durations_follow_the_same_rule() {
        for (yaml, now, start, end) in [
            // PT7M: the floor restarts at :00 each hour, so :56 then :00.
            (
                "every: PT7M",
                "2026-09-26T04:59:10.5Z",
                "2026-09-26T04:56:00Z",
                "2026-09-26T05:02:59.999Z",
            ),
            // PT90S: whole minutes = 1, so the minute is kept.
            (
                "every: PT90S",
                "2026-09-26T04:13:40Z",
                "2026-09-26T04:13:00Z",
                "2026-09-26T04:14:29.999Z",
            ),
            // P2D: the day is not floored to an even day, only the hour zeroed.
            (
                "every: P2D",
                "2026-09-27T13:00:00Z",
                "2026-09-27T00:00:00Z",
                "2026-09-28T23:59:59.999Z",
            ),
            // P1DT2H: >= 1 day, so hour then minute zeroed.
            (
                "every: P1DT2H",
                "2026-09-26T13:45:00Z",
                "2026-09-26T00:00:00Z",
                "2026-09-27T01:59:59.999Z",
            ),
        ] {
            let (s, e) = resolved(yaml).bounds(at(now)).unwrap();
            assert_eq!((s, e), (at(start), at(end)), "{yaml} at {now}");
        }
    }

    // ------------------------------------------------------------------
    // DST (chosen from the Java; see the module doc)
    // ------------------------------------------------------------------

    /// America/New_York springs forward on 2026-03-08 (02:00 EST -> 03:00
    /// EDT), a 23-hour day. `withHour(0)` lands on 00:00 EST, and the window
    /// is `Duration.ofDays(1)` = 24 h of INSTANT time, so it ends at 00:59:59.999
    /// EDT the next day — not at the next local midnight, which a wall-clock
    /// implementation would give (`2026-03-09T03:59:59.999Z`).
    #[test]
    fn a_day_window_across_spring_forward_is_24_hours_of_instant_time() {
        let ny = tz("America/New_York");
        let (s, e) = TimeWindow::Duration {
            every: DEFAULT_EVERY,
            offset: TimeDelta::zero(),
        }
        .bounds(at("2026-03-08T15:34:56Z"), ny)
        .unwrap();
        assert_eq!(s, at("2026-03-08T05:00:00Z"));
        assert_eq!(e, at("2026-03-09T04:59:59.999Z"));
    }

    /// America/New_York falls back on 2026-11-01: local 01:00-01:59 happens
    /// twice. `withMinute(0)` keeps the CURRENT offset when it is valid
    /// (`ZonedDateTime.ofLocal`'s preferred offset), so the two 01:30s align
    /// to two different hours.
    #[test]
    fn an_hour_window_in_the_fall_back_overlap_keeps_the_current_offset() {
        let ny = tz("America/New_York");
        let hour = TimeWindow::Duration {
            every: TimeDelta::hours(1),
            offset: TimeDelta::zero(),
        };
        // 01:30 EDT (first pass).
        let (s, e) = hour.bounds(at("2026-11-01T05:30:00Z"), ny).unwrap();
        assert_eq!(
            (s, e),
            (at("2026-11-01T05:00:00Z"), at("2026-11-01T05:59:59.999Z"))
        );
        // 01:30 EST (second pass).
        let (s, e) = hour.bounds(at("2026-11-01T06:30:00Z"), ny).unwrap();
        assert_eq!(
            (s, e),
            (at("2026-11-01T06:00:00Z"), at("2026-11-01T06:59:59.999Z"))
        );
    }

    /// America/Santiago springs forward AT MIDNIGHT on 2026-09-06 (00:00 -04
    /// -> 01:00 -03), so `withHour(0)` lands in the gap and Java moves it
    /// forward by the gap's length: 01:00 -03.
    #[test]
    fn a_day_window_whose_midnight_is_in_a_gap_starts_after_the_gap() {
        let santiago = tz("America/Santiago");
        let gap = NaiveDate::from_ymd_opt(2026, 9, 6)
            .unwrap()
            .and_hms_opt(0, 30, 0)
            .unwrap();
        assert!(
            matches!(santiago.from_local_datetime(&gap), LocalResult::None),
            "precondition: 00:30 on 2026-09-06 must not exist in America/Santiago"
        );
        let (s, e) = TimeWindow::Duration {
            every: DEFAULT_EVERY,
            offset: TimeDelta::zero(),
        }
        .bounds(at("2026-09-06T15:00:00Z"), santiago)
        .unwrap();
        assert_eq!(s, at("2026-09-06T04:00:00Z"));
        assert_eq!(e, at("2026-09-07T03:59:59.999Z"));
    }

    /// `midnight.plusSeconds(from.toSecondOfDay())` is instant arithmetic, so
    /// on a 23-hour day `from: 10:00` opens at local 11:00. Chosen from the
    /// Java, not measured.
    #[test]
    fn a_daily_window_on_a_spring_forward_day_adds_seconds_to_midnight() {
        let (s, e) = TimeWindow::DailyWindow {
            from: NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
            to: NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
        }
        .bounds(at("2026-03-08T15:30:00Z"), tz("America/New_York"))
        .unwrap();
        assert_eq!(
            (s, e),
            (at("2026-03-08T15:00:00Z"), at("2026-03-08T17:00:00Z"))
        );
    }

    // ------------------------------------------------------------------
    // Checked arithmetic
    // ------------------------------------------------------------------

    #[test]
    fn bounds_never_panic_on_overflow() {
        let utc = chrono_tz::UTC;
        let edge = DateTime::<Utc>::MAX_UTC - TimeDelta::days(1);
        for w in [
            TimeWindow::Duration {
                every: TimeDelta::days(400),
                offset: TimeDelta::zero(),
            },
            TimeWindow::Duration {
                every: TimeDelta::hours(1),
                offset: TimeDelta::days(400),
            },
            TimeWindow::Sliding {
                lookback: TimeDelta::days(400),
            },
            TimeWindow::Sliding {
                lookback: TimeDelta::MAX,
            },
            TimeWindow::Duration {
                every: TimeDelta::MAX,
                offset: TimeDelta::zero(),
            },
        ] {
            assert_eq!(w.bounds(edge, utc), Err(BoundsError::OutOfRange), "{w:?}");
        }
        // And at an ordinary now, TimeDelta::MAX still refuses rather than
        // panicking.
        let now = at("2026-09-26T04:00:00Z");
        assert_eq!(
            TimeWindow::Sliding {
                lookback: TimeDelta::MAX
            }
            .bounds(now, utc),
            Err(BoundsError::OutOfRange)
        );
    }

    #[test]
    fn a_window_with_no_room_inside_is_empty_not_returned() {
        let now = at("2026-09-26T04:00:00Z");
        let r = TimeWindow::Duration {
            every: TimeDelta::milliseconds(1),
            offset: TimeDelta::zero(),
        }
        .bounds(now, chrono_tz::UTC);
        assert!(matches!(r, Err(BoundsError::Empty { .. })), "{r:?}");
    }

    // ------------------------------------------------------------------
    // from_model
    // ------------------------------------------------------------------

    #[test]
    fn from_model_maps_each_kind_and_defaults_to_one_utc_day() {
        let none = TimeWindow::from_model(None).unwrap();
        assert_eq!(
            none,
            ResolvedWindow {
                window: TimeWindow::Duration {
                    every: TimeDelta::days(1),
                    offset: TimeDelta::zero()
                },
                tz: chrono_tz::UTC,
            }
        );
        assert_eq!(
            resolved("every: PT2M\noffset: PT1M\ntimezone: Asia/Kolkata"),
            ResolvedWindow {
                window: TimeWindow::Duration {
                    every: TimeDelta::minutes(2),
                    offset: TimeDelta::minutes(1)
                },
                tz: tz("Asia/Kolkata"),
            }
        );
        assert_eq!(
            resolved("timezone: Asia/Kolkata").window,
            TimeWindow::Duration {
                every: TimeDelta::days(1),
                offset: TimeDelta::zero()
            }
        );
        assert_eq!(
            resolved("lookback: PT2M").window,
            TimeWindow::Sliding {
                lookback: TimeDelta::minutes(2)
            }
        );
        assert_eq!(
            resolved("from: \"03:00\"\nto: \"05:00:00\"").window,
            TimeWindow::DailyWindow {
                from: NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
                to: NaiveTime::from_hms_opt(5, 0, 0).unwrap(),
            }
        );
        assert_eq!(
            resolved("deadline: \"05:00\"").window,
            TimeWindow::DailyDeadline {
                deadline: NaiveTime::from_hms_opt(5, 0, 0).unwrap()
            }
        );
    }

    /// The shapes `spec::validate` refuses, each refused by `from_model` with
    /// the same code — the run-time half of the drift check (the save-time
    /// half, which compares against `validate` itself, is
    /// `depends_on::tests::save_and_run_time_refuse_the_same_shapes`).
    #[test]
    fn from_model_refuses_every_refused_shape() {
        for (yaml, code) in [
            (
                "deadline: \"09:00\"\nfrom: \"08:00\"",
                "window_conflicting_fields",
            ),
            (
                "deadline: \"09:00\"\nevery: PT1H",
                "window_conflicting_fields",
            ),
            (
                "from: \"08:00\"\nto: \"09:00\"\nevery: PT1H",
                "window_conflicting_fields",
            ),
            ("every: PT1H\nlookback: PT2M", "window_conflicting_fields"),
            ("lookback: PT2M\noffset: PT1M", "window_conflicting_fields"),
            ("from: \"08:00\"", "window_daily_bound_missing"),
            ("to: \"08:00\"", "window_daily_bound_missing"),
            ("offset: PT6H", "window_offset_without_every"),
            ("every: P1M", "window_duration_invalid"),
            ("every: P366DT1S", "window_duration_invalid"),
            ("lookback: P367D", "window_duration_invalid"),
            ("every: PT1H\noffset: P366DT1S", "window_duration_invalid"),
            ("every: PT0S", "window_duration_not_positive"),
            ("lookback: P0D", "window_duration_not_positive"),
            ("deadline: \"25:00\"", "window_time_invalid"),
            ("from: \"12:00\"\nto: \"10:00\"", "window_daily_range_empty"),
            ("deadline: \"00:00\"", "window_daily_range_empty"),
            (
                "every: PT1H\ntimezone: Mars/Olympus_Mons",
                "window_timezone_invalid",
            ),
        ] {
            let err = TimeWindow::from_model(Some(&window(yaml)))
                .expect_err(&format!("{yaml} must be refused"));
            assert!(
                err.0.iter().any(|r| r.code == code),
                "{yaml}: expected {code}, got {err}"
            );
        }
    }
}
