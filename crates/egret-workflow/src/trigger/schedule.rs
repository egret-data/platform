// kestra: core/src/main/java/io/kestra/plugin/core/trigger/Schedule.java
// kestra: core/src/main/java/io/kestra/core/models/triggers/Backfill.java
// kestra: scheduler/src/main/java/io/kestra/scheduler/TriggerScheduler.java
// cronutils: com.cronutils:cron-utils:9.2.1 (read off the pinned artifact)

//! `core.schedule` — the cron trigger, and the dialect it speaks.
//!
//! # Where every rule in this file came from
//!
//! Nothing here is read off Kestra's documentation and nothing is inherited
//! from a cron library's defaults. Every rule below was measured by **running
//! Kestra's own `Schedule` class out of the pinned oracle artifact**
//! (`kestra/kestra@sha256:28f59bf1…`), on a fixed `SchedulerClock`, and reading
//! what it answered — `docker cp /app/kestra` out of the image (a single fat
//! jar that carries `io.kestra.plugin.core.trigger.Schedule`, `cron-utils`
//! 9.2.1 and everything else), then a probe class compiled into Kestra's own
//! package so it can reach `executionTime()`, `scheduleDates` and
//! `handleMaxDelay`. The image ships a JRE with no `javap` and its classes are
//! **class-file version 69 (Java 25)**, so the host JDK has to be a 25 —
//! Temurin 21 refuses them with `wrong version 69.0, should be 65.0`.
//!
//! The class bytes were also disassembled and compared against the sibling
//! checkout at `96893b589`: for `Schedule` the two **agree**, method for
//! method, including the two constants this file depends on most (the `-1s` in
//! `scheduleDates` and the `+1s` in `eval`). That agreement is a measurement,
//! not an assumption — the image's own labels say `revision=169ca7f2…`, which
//! the checkout does not contain, so the two were not assumed equal.
//!
//! ## 1. The cron dialect — `Schedule.<clinit>`
//!
//! `CRON_DEFINITION_BUILDER` is built field by field in the static
//! initialiser, and it is **not** any of cron-utils' named presets:
//!
//! | field | range | notes |
//! |---|---|---|
//! | minutes | 0–59 | strict range |
//! | hours | 0–23 | strict range |
//! | day-of-month | 1–31 | strict range, **`supportsL`** |
//! | month | 1–12 | strict range |
//! | day-of-week | 0–7 | strict range, Monday = 1, **7 mapped to 0** |
//!
//! plus `withSeconds().withValidRange(0, 59).withStrictRange()` **prepended**
//! for the second parser, which is selected only by `withSeconds: true`.
//!
//! Supported nicknames, exactly these seven and nothing else:
//! `@yearly`, `@annually`, `@monthly`, `@weekly`, `@daily`, `@midnight`,
//! `@hourly`. Measured refusals that matter, because each is a way a
//! more-permissive parser would **fire where Kestra would not**:
//!
//! - `@reboot` → `Nickname @reboot not supported!`
//! - `@DAILY`, `@Daily` → `contains 1 parts but we expect one of [5]`
//!   (nicknames are case-sensitive lowercase)
//! - `?` in either day field → `Invalid expression: ?`
//! - `W` (`15W`, `LW`) → `Invalid chars … W`
//! - `#` (`1#1`) → `Invalid expression: 1#1`
//! - `L` in **day-of-week** (`5L`, `L`) → `Invalid chars … L`
//! - a reversed range (`5-1`, `6-0`, `7-1`, `SUN-SAT`) → `Invalid range!`
//! - `*/0` → `Period 0 not in range [0, 59]`
//! - a **6-field** expression with `withSeconds: false`, and a **5-field** one
//!   with `withSeconds: true` → `contains N parts but we expect one of [M]`
//!
//! Measured acceptances: `L` and `L-n` in day-of-month (`L-3` on a 30-day
//! month is the 27th), `0-0`, `5/10`, `*/7`, month and day names in either
//! case (`MON`, `mon-fri`, `jan-dec`), `1-7` (every day, because 7 maps to 0),
//! `SAT-SUN` (so `SUN` is **7**, not 0, before the mapping), and leading,
//! trailing, repeated or tab whitespace between fields.
//!
//! ## 2. Day-of-month vs day-of-week — OR, and what counts as restricted
//!
//! Measured, not inferred from the Vixie convention:
//!
//! - `0 0 1 * 1` from Mon 2026-06-15 → **2026-06-22**, a Monday that is not
//!   the 1st. Both fields restricted ⇒ **OR**.
//! - `0 0 */2 * 1` → **2026-06-17**, day 17 (in `*/2`) but a Wednesday. So a
//!   **stepped** field counts as restricted.
//! - `0 0 1-31 * 1` → **2026-06-16**, day 16, a Tuesday. So an explicit range
//!   covering everything also counts as restricted.
//! - `0 0 * * 1` → **2026-06-22**; `0 0 1 * *` → **2026-07-01**. A bare `*`
//!   is unrestricted, and the other field alone decides.
//!
//! ## 3. The timezone rule — `Schedule.convertDateTime`
//!
//! ```text
//! if (this.timezone == null) return date;
//! return date.withZoneSameInstant(ZoneId.of(this.timezone));
//! ```
//!
//! `withZoneSameInstant` keeps the instant and changes the calendar the cron
//! is read in, and every call site (`nextEvaluationDate`, `eval`,
//! `scheduleDates`) hands cron-utils an already-converted value — so **the
//! expression is evaluated in the trigger's own zone**. Measured: `0 2 * * *`
//! in `Asia/Tokyo`, from `2026-06-15T12:00Z`, fires at
//! `2026-06-16T02:00+09:00` = `2026-06-15T17:00Z`.
//!
//! DST is *not* smoothed over, in either direction, and both were measured by
//! walking successive executions:
//!
//! - **Spring forward** (`Europe/Paris`, 2026-03-29): `*/30 * * * *` walks
//!   `01:30+01:00` → `03:00+02:00`. The 02:00 and 02:30 local ticks **do not
//!   fire at all** — they are skipped, not shifted. `0 2 * * *` likewise jumps
//!   2026-03-28 → **2026-03-30**.
//! - **Fall back** (`Europe/Paris`, 2026-10-25): `*/30 * * * *` walks
//!   `02:00+02:00`, `02:30+02:00`, `02:00+01:00`, `02:30+01:00` — the repeated
//!   local hour **fires twice**, and the order is by **instant**, not by local
//!   clock reading. That ordering is why `next_after` takes the minimum of a
//!   day's candidate instants rather than the first in local order.
//!
//! `timezone` absent is **UTC** here. Kestra's field carries
//! `@Builder.Default ZoneId.systemDefault().toString()`, which is
//! host-dependent — measured `Asia/Ho_Chi_Minh` on this developer machine —
//! but the pinned image's own `/etc/timezone` is **`Etc/UTC`** and
//! `/etc/localtime` links to `/usr/share/zoneinfo/Etc/UTC`, so UTC is what the
//! artifact that produced the golden actually uses. W1's note left this "to the
//! scheduler to interpret"; this is that interpretation, measured.
//!
//! ## 4. The tick rule — `Schedule.scheduleDates`
//!
//! ```text
//! Optional<ZonedDateTime> next = executionTime.nextExecution(date.minus(Duration.ofSeconds(1)));
//! ```
//!
//! The **minus one second** is load-bearing and is the reason a tick fires on
//! its own instant rather than one period later: measured, `@hourly` evaluated
//! at exactly `12:00` yields `date = 12:00`, and `*/15` at exactly `12:15`
//! yields `12:15` — while `*/15` at `12:07:30` yields `12:15`. Both cron-utils
//! primitives are otherwise **strict**: `nextExecution` is strictly after and
//! `lastExecution` strictly before its argument (`0 2 * * *` from
//! `2026-06-15T02:00Z` gives next `06-16T02:00`, last `06-14T02:00`).
//!
//! A cron that can never match returns **nothing** rather than searching for
//! ever: measured, `0 0 30 2 *` and `0 0 31 4 *` both answer
//! `Optional.empty()` for next *and* last, which reaches `eval` as "no
//! execution, ever".
//!
//! ## 5. The late / missed-tick rule
//!
//! Two separate mechanisms, and conflating them is the mistake this section
//! exists to prevent.
//!
//! - **With no `lateMaximumDelay`, missed ticks are replayed one at a time.**
//!   Measured on `nextEvaluationDate`: last date `2026-06-10T02:00Z`, clock at
//!   `2026-06-15T12:00Z`, `0 2 * * *` → **`2026-06-11T02:00Z`**. The scheduler
//!   catches up tick by tick; it does not jump to now.
//! - **With `lateMaximumDelay`, `handleMaxDelay` walks forward** to the first
//!   tick whose `date + lateMaximumDelay` is not already in the past. Same
//!   fixture plus `PT1H` → **`2026-06-16T02:00Z`**: the five missed days are
//!   dropped. And `*/15` with `PT10M` from `10:00` on a `12:00` clock →
//!   `12:00`, i.e. the first tick that is still within its delay.
//!
//! `handleMaxDelay` is skipped entirely while a backfill is running
//! (`backfill == null` is part of its guard in `nextEvaluationDate`): measured,
//! the same backfill fixture answers `2026-06-01T02:00Z` with and without
//! `PT1H`.
//!
//! ## 6. Backfill window semantics
//!
//! `TriggerState.backfill` fills the window in on creation —
//! `end = end != null ? end : now()`, `currentDate = currentDate != null ?
//! currentDate : start` — and `TriggerState.getBackFillForNextEvaluationDate`
//! ends it:
//!
//! ```text
//! if (backfill != null && !backfill.getPaused()) {
//!     if (localNextEvaluationDate.isAfter(backfill.getEnd())) return null;   // finished
//!     else return backfill.toBuilder().currentDate(localNextEvaluationDate).build();
//! }
//! ```
//!
//! so the backfill advances **`currentDate` to each next evaluation date** and
//! is **cleared the moment that date passes `end`**. `Schedule` reads the same
//! window from the other side: `nextEvaluationDate` computes from
//! `currentDate` and, if the answer `isAfter(end)`, recomputes **from now**;
//! `eval` returns nothing when `paused`, and nothing when `currentDate` is
//! strictly before the tick it computed. All three were measured:
//! `currentDate = 06-01T00:00Z, end = 06-05T00:00Z` → `06-01T02:00Z`;
//! `currentDate = 06-04T02:00Z` → `06-16T02:00Z` (the `06-05T02:00Z` tick is
//! after `end`, so the schedule returns to the live clock);
//! `currentDate = 06-05T02:00Z` → `06-16T02:00Z`.
//!
//! **The corpus contains zero live backfill declarations** — 345 flows, two
//! commented-out lines — so nothing in this section is or can be judged by the
//! execution ratchet. The tests in this file are the whole instrument.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Datelike, Duration, LocalResult, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;

use crate::models::{Schedule, Trigger, TriggerKind};

/// How far forward or back `next_after`/`last_before` will look before
/// answering "never".
///
/// The artifact gives up too — measured, `0 0 30 2 *` answers
/// `Optional.empty()` for both directions — so a bound is the behaviour, not a
/// shortcut around it. Twenty years covers the longest gap a *satisfiable*
/// expression in this dialect can have: `0 0 29 2 *` across a century
/// non-leap year is eight (2096 → 2104). The scan is over **days whose date
/// fields match**, so an unsatisfiable expression costs 7305 integer checks
/// and never builds a single candidate instant.
const SEARCH_DAYS: i64 = 366 * 20;

/// Everything that can be wrong with a `Schedule`'s cron, typed.
///
/// Typed rather than a `String` for the reason W6's brief gives for skips: a
/// caller that cannot tell "this dialect does not have `?`" from "hour 24 is
/// out of range" cannot report either honestly. Each variant below corresponds
/// to a refusal measured on the artifact; see this module's §1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CronError {
    /// `""` — measured: `Empty expression!`
    Empty,
    /// Measured: `Cron expression contains {got} parts but we expect one of [{want}]`.
    WrongFieldCount { got: usize, want: usize },
    /// Measured: `Nickname @reboot not supported!`, and every other `@…` that
    /// is not one of the seven.
    UnsupportedNickname(String),
    /// A character this dialect does not define — `?`, `W`, `#`, or `L`
    /// outside day-of-month.
    UnsupportedSyntax { field: Field, token: String },
    /// Measured: `Value {value} not in range [{min}, {max}]`.
    OutOfRange {
        field: Field,
        value: i64,
        min: u32,
        max: u32,
    },
    /// Measured: `Invalid range! [{from},{to}]` — this dialect has
    /// `withStrictRange()` on every field, so ranges never wrap.
    InvalidRange { field: Field, from: u32, to: u32 },
    /// Measured: `Period 0 not in range [0, 59]`.
    ZeroStep { field: Field },
    /// A token that is none of the above shapes.
    Malformed { field: Field, token: String },
}

impl fmt::Display for CronError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "empty cron expression"),
            Self::WrongFieldCount { got, want } => write!(
                f,
                "cron expression contains {got} parts but we expect one of [{want}]"
            ),
            Self::UnsupportedNickname(n) => write!(f, "nickname {n} not supported"),
            Self::UnsupportedSyntax { field, token } => {
                write!(f, "{field}: unsupported syntax in {token:?}")
            }
            Self::OutOfRange {
                field,
                value,
                min,
                max,
            } => write!(f, "{field}: value {value} not in range [{min}, {max}]"),
            Self::InvalidRange { field, from, to } => {
                write!(f, "{field}: invalid range! [{from},{to}]")
            }
            Self::ZeroStep { field } => write!(f, "{field}: period 0 is not a valid step"),
            Self::Malformed { field, token } => write!(f, "{field}: invalid expression {token:?}"),
        }
    }
}

impl std::error::Error for CronError {}

/// Which cron field a `CronError` is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Second,
    Minute,
    Hour,
    DayOfMonth,
    Month,
    DayOfWeek,
}

impl Field {
    /// `(min, max)` for this field, exactly as `Schedule.<clinit>` declares it.
    fn range(self) -> (u32, u32) {
        match self {
            Self::Second | Self::Minute => (0, 59),
            Self::Hour => (0, 23),
            Self::DayOfMonth => (1, 31),
            Self::Month => (1, 12),
            // 0-7, with 7 mapped to 0 AFTER validation -- which is why `1-7`
            // parses (and means every day) while `7-1` does not.
            Self::DayOfWeek => (0, 7),
        }
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Second => "seconds",
            Self::Minute => "minutes",
            Self::Hour => "hours",
            Self::DayOfMonth => "day of month",
            Self::Month => "month",
            Self::DayOfWeek => "day of week",
        })
    }
}

/// The day-of-month field, which is the only one carrying `supportsL`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DayOfMonth {
    values: BTreeSet<u32>,
    /// Offsets from the last day of the month: `L` is `0`, `L-3` is `3`.
    /// Measured: `0 0 L-3 * *` fires on the 27th of a 30-day month.
    last_offsets: BTreeSet<u32>,
}

/// A parsed cron expression in `Schedule`'s dialect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    seconds: BTreeSet<u32>,
    minutes: BTreeSet<u32>,
    hours: BTreeSet<u32>,
    dom: DayOfMonth,
    months: BTreeSet<u32>,
    /// Already mapped: 7 has become 0, so this set is 0 (Sunday) through 6.
    days_of_week: BTreeSet<u32>,
    /// Whether the day-of-month field was something other than a bare `*`.
    dom_restricted: bool,
    /// Whether the day-of-week field was something other than a bare `*`.
    dow_restricted: bool,
}

impl Cron {
    /// Parses `expr` in `Schedule`'s dialect.
    ///
    /// `with_seconds` selects between the two parsers the artifact builds, and
    /// it is **not** a hint: a 6-field expression with `with_seconds = false`
    /// is refused, and so is a 5-field one with `with_seconds = true`
    /// (measured, §1). A nickname is accepted under either, because the
    /// artifact expands it before counting fields.
    pub fn parse(expr: &str, with_seconds: bool) -> Result<Self, CronError> {
        let trimmed = expr.trim();
        if trimmed.is_empty() {
            return Err(CronError::Empty);
        }

        if let Some(rest) = trimmed.strip_prefix('@') {
            // Nicknames are matched case-sensitively: measured, `@DAILY` is not
            // recognised as a nickname at all and then fails the field count as
            // a one-token expression.
            let expanded = match rest {
                "yearly" | "annually" => "0 0 1 1 *",
                "monthly" => "0 0 1 * *",
                "weekly" => "0 0 * * 0",
                "daily" | "midnight" => "0 0 * * *",
                "hourly" => "0 * * * *",
                _ => return Err(CronError::UnsupportedNickname(trimmed.to_string())),
            };
            // The expansion is always five fields, whatever `with_seconds`
            // says -- measured: `@daily` parses with `withSeconds: true` and
            // still fires at midnight, not at every second of it.
            return Self::parse_fields(expanded, false);
        }

        Self::parse_fields(trimmed, with_seconds)
    }

    fn parse_fields(expr: &str, with_seconds: bool) -> Result<Self, CronError> {
        // Measured: leading, trailing, repeated and tab whitespace are all
        // accepted, so the split is on whitespace runs.
        let parts: Vec<&str> = expr.split_whitespace().collect();
        let want = if with_seconds { 6 } else { 5 };
        if parts.len() != want {
            return Err(CronError::WrongFieldCount {
                got: parts.len(),
                want,
            });
        }

        let (seconds, rest) = if with_seconds {
            (parse_field(parts[0], Field::Second)?.0, &parts[1..])
        } else {
            (BTreeSet::from([0]), &parts[..])
        };

        let (minutes, _) = parse_field(rest[0], Field::Minute)?;
        let (hours, _) = parse_field(rest[1], Field::Hour)?;
        let (dom_values, dom_restricted, last_offsets) = parse_day_of_month(rest[2])?;
        let (months, _) = parse_field(rest[3], Field::Month)?;
        let (dow_raw, dow_restricted) = parse_field(rest[4], Field::DayOfWeek)?;

        // `withIntMapping(7, 0)`: 7 becomes Sunday. Applied AFTER range
        // validation, which is why `1-7` is every day and `7-1` is refused.
        let days_of_week = dow_raw
            .into_iter()
            .map(|d| if d == 7 { 0 } else { d })
            .collect();

        Ok(Self {
            seconds,
            minutes,
            hours,
            dom: DayOfMonth {
                values: dom_values,
                last_offsets,
            },
            months,
            days_of_week,
            dom_restricted,
            dow_restricted,
        })
    }

    /// Does this expression's **date** part select `date`?
    ///
    /// The day-of-month / day-of-week rule, measured in §2: both restricted
    /// means either may match; otherwise the unrestricted one is `*`, whose
    /// value set holds every day, so the plain conjunction is the same answer.
    fn date_matches(&self, date: NaiveDate) -> bool {
        if !self.months.contains(&date.month()) {
            return false;
        }
        let dom_match = self.dom.values.contains(&date.day())
            || self.dom.last_offsets.iter().any(|off| {
                last_day_of_month(date)
                    .checked_sub(*off)
                    .is_some_and(|d| d == date.day())
            });
        // chrono's Sunday = 0 matches this dialect's post-mapping numbering.
        let dow_match = self
            .days_of_week
            .contains(&date.weekday().num_days_from_sunday());

        if self.dom_restricted && self.dow_restricted {
            dom_match || dow_match
        } else {
            dom_match && dow_match
        }
    }

    /// Every instant this expression selects on the local date `day` in `tz`,
    /// in **instant** order.
    ///
    /// Instant order, not local order, is the measured behaviour: across a
    /// fall-back the artifact walks `02:00+02:00`, `02:30+02:00`,
    /// `02:00+01:00`, `02:30+01:00` (§3), which local order would have given
    /// as `02:00+02:00`, `02:00+01:00`, `02:30+02:00`, `02:30+01:00`.
    ///
    /// A local time inside a spring-forward gap contributes **nothing**, and an
    /// ambiguous one contributes **both** of its instants.
    fn candidates_on(&self, tz: Tz, day: NaiveDate) -> Vec<DateTime<Tz>> {
        let mut out = Vec::new();
        for h in &self.hours {
            for m in &self.minutes {
                for s in &self.seconds {
                    let Some(naive) = day.and_hms_opt(*h, *m, *s) else {
                        continue;
                    };
                    match tz.from_local_datetime(&naive) {
                        LocalResult::None => {}
                        LocalResult::Single(dt) => out.push(dt),
                        LocalResult::Ambiguous(a, b) => {
                            out.push(a);
                            out.push(b);
                        }
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// The first tick **strictly after** `after`, or `None` if this expression
    /// selects no instant within [`SEARCH_DAYS`].
    ///
    /// Kestra's `ExecutionTime.nextExecution` is strictly-after (§4), and this
    /// is that.
    pub fn next_after(&self, after: DateTime<Tz>) -> Option<DateTime<Tz>> {
        let tz = after.timezone();
        let mut day = after.naive_local().date();
        for _ in 0..SEARCH_DAYS {
            if self.date_matches(day) {
                if let Some(hit) = self
                    .candidates_on(tz, day)
                    .into_iter()
                    .find(|candidate| *candidate > after)
                {
                    return Some(hit);
                }
            }
            day = day.succ_opt()?;
        }
        None
    }

    /// The last tick **strictly before** `before`, or `None`.
    pub fn last_before(&self, before: DateTime<Tz>) -> Option<DateTime<Tz>> {
        let tz = before.timezone();
        let mut day = before.naive_local().date();
        for _ in 0..SEARCH_DAYS {
            if self.date_matches(day) {
                if let Some(hit) = self
                    .candidates_on(tz, day)
                    .into_iter()
                    .rev()
                    .find(|candidate| *candidate < before)
                {
                    return Some(hit);
                }
            }
            day = day.pred_opt()?;
        }
        None
    }
}

/// The last day of `date`'s month.
fn last_day_of_month(date: NaiveDate) -> u32 {
    let (y, m) = (date.year(), date.month());
    let first_of_next = if m == 12 {
        NaiveDate::from_ymd_opt(y + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(y, m + 1, 1)
    };
    first_of_next
        .and_then(|d| d.pred_opt())
        .map_or(28, |d| d.day())
}

/// Day-of-month is parsed on its own because it is the only field with `L`.
///
/// Returns `(values, restricted, last_offsets)`.
fn parse_day_of_month(token: &str) -> Result<(BTreeSet<u32>, bool, BTreeSet<u32>), CronError> {
    let mut values = BTreeSet::new();
    let mut last_offsets = BTreeSet::new();
    let mut restricted = false;

    for part in token.split(',') {
        let part = part.trim();
        let upper = part.to_ascii_uppercase();
        if upper == "L" {
            restricted = true;
            last_offsets.insert(0);
        } else if let Some(off) = upper.strip_prefix("L-") {
            restricted = true;
            let off: u32 = off.parse().map_err(|_| CronError::Malformed {
                field: Field::DayOfMonth,
                token: part.to_string(),
            })?;
            // `L-31` would name a day before the 1st; the artifact's range is
            // 1..=31 and an offset of 31 can never select a day, so refuse it
            // rather than build a set that silently matches nothing.
            if off > 30 {
                return Err(CronError::OutOfRange {
                    field: Field::DayOfMonth,
                    value: i64::from(off),
                    min: 0,
                    max: 30,
                });
            }
            last_offsets.insert(off);
        } else {
            let (vals, part_restricted) = parse_field(part, Field::DayOfMonth)?;
            restricted |= part_restricted;
            values.extend(vals);
        }
    }

    // A day-of-month of exactly `*` is the only unrestricted spelling, and
    // `parse_field` has already told us so through `part_restricted`.
    if !restricted && values.is_empty() {
        return Err(CronError::Malformed {
            field: Field::DayOfMonth,
            token: token.to_string(),
        });
    }
    Ok((values, restricted, last_offsets))
}

/// Parses one comma-separated cron field.
///
/// Returns `(values, restricted)`, where `restricted` is false only for a bare
/// `*` — measured in §2 as the thing that decides the day-of-month /
/// day-of-week rule, and measured to be **false for `*/2` and for `1-31`**.
fn parse_field(token: &str, field: Field) -> Result<(BTreeSet<u32>, bool), CronError> {
    let (min, max) = field.range();
    let mut values = BTreeSet::new();
    let mut restricted = false;

    for part in token.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(CronError::Malformed {
                field,
                token: token.to_string(),
            });
        }

        // Refuse the characters this dialect does not define, by name, so the
        // error says which one rather than "malformed". These are the four
        // over-acceptances a permissive parser would ship (§1).
        if let Some(bad) = part
            .chars()
            .find(|c| matches!(c, '?' | 'W' | 'w' | '#' | 'L' | 'l'))
        {
            let _ = bad;
            return Err(CronError::UnsupportedSyntax {
                field,
                token: part.to_string(),
            });
        }

        let (spec, step) = match part.split_once('/') {
            Some((spec, step)) => {
                let step: u32 = step.parse().map_err(|_| CronError::Malformed {
                    field,
                    token: part.to_string(),
                })?;
                if step == 0 {
                    return Err(CronError::ZeroStep { field });
                }
                (spec, Some(step))
            }
            None => (part, None),
        };

        let (from, to, is_star) = if spec == "*" {
            (min, max, true)
        } else if let Some((lo, hi)) = spec.split_once('-') {
            let lo = parse_value(lo, field)?;
            let hi = parse_value(hi, field)?;
            // `withStrictRange()` on every field: measured, `5-1`, `6-0`,
            // `7-1` and `SUN-SAT` are all `Invalid range!`.
            if lo > hi {
                return Err(CronError::InvalidRange {
                    field,
                    from: lo,
                    to: hi,
                });
            }
            (lo, hi, false)
        } else {
            let v = parse_value(spec, field)?;
            // `5/10` means 5, 15, 25 … up to the field maximum -- measured.
            // A bare `5` is just 5.
            let hi = if step.is_some() { max } else { v };
            (v, hi, false)
        };

        // A bare `*` is the only spelling that leaves the field unrestricted:
        // `*/2` carries a step and IS restricted (measured, §2).
        if !is_star || step.is_some() {
            restricted = true;
        }

        let step = step.unwrap_or(1);
        let mut v = from;
        while v <= to {
            values.insert(v);
            v += step;
        }
    }

    if values.is_empty() {
        return Err(CronError::Malformed {
            field,
            token: token.to_string(),
        });
    }
    Ok((values, restricted))
}

/// One numeric or named value, range-checked against the field's own bounds.
fn parse_value(token: &str, field: Field) -> Result<u32, CronError> {
    let token = token.trim();
    let (min, max) = field.range();

    let value = if let Ok(n) = token.parse::<i64>() {
        n
    } else {
        // Names, case-insensitively -- measured: `MON`, `mon-fri`, `jan-dec`
        // all parse.
        let upper = token.to_ascii_uppercase();
        let named = match field {
            Field::Month => match upper.as_str() {
                "JAN" => Some(1),
                "FEB" => Some(2),
                "MAR" => Some(3),
                "APR" => Some(4),
                "MAY" => Some(5),
                "JUN" => Some(6),
                "JUL" => Some(7),
                "AUG" => Some(8),
                "SEP" => Some(9),
                "OCT" => Some(10),
                "NOV" => Some(11),
                "DEC" => Some(12),
                _ => None,
            },
            Field::DayOfWeek => match upper.as_str() {
                // SUN is **7**, not 0: measured, `SUN-SAT` fails as
                // `Invalid range! [7,6]` while `SAT-SUN` parses.
                "SUN" => Some(7),
                "MON" => Some(1),
                "TUE" => Some(2),
                "WED" => Some(3),
                "THU" => Some(4),
                "FRI" => Some(5),
                "SAT" => Some(6),
                _ => None,
            },
            _ => None,
        };
        match named {
            Some(n) => n,
            None => {
                return Err(CronError::Malformed {
                    field,
                    token: token.to_string(),
                })
            }
        }
    };

    if value < i64::from(min) || value > i64::from(max) {
        return Err(CronError::OutOfRange {
            field,
            value,
            min,
            max,
        });
    }
    Ok(u32::try_from(value).expect("range-checked above"))
}

/// `Schedule.Output` — the three dates a fired tick carries.
///
/// These are the values Kestra puts in `trigger.date` / `trigger.next` /
/// `trigger.previous`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleDates {
    pub date: DateTime<Utc>,
    pub next: Option<DateTime<Utc>>,
    pub previous: Option<DateTime<Utc>>,
}

/// Why a `Schedule` cannot be scheduled at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    /// The trigger is not a `Schedule`.
    NotASchedule,
    /// `Schedule.cron` is `@NotNull` in Java. W1 modelled it `Option` because
    /// bean validation is not parsing; this is the check that owes.
    MissingCron,
    /// The cron itself.
    Cron(CronError),
    /// `ZoneId.of` accepts a superset of IANA (`GMT+02:00`, `UTC+2`);
    /// `chrono-tz` takes IANA only. Refusing is the safe direction — it
    /// declines to schedule rather than scheduling in the wrong zone — and it
    /// is recorded rather than hidden.
    UnknownTimezone(String),
    /// `Schedule` supports a `when` expression that filters candidate ticks
    /// (`Schedule.findNextDateMatchingConditions`). Rendering it needs a
    /// `RunContext` this seam does not have, and a schedule that ignored it
    /// would **fire where Kestra would not** — the one failure the oracle
    /// cannot catch. So it is refused, loudly.
    UnsupportedWhenCondition(String),
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotASchedule => write!(f, "trigger is not a core.schedule"),
            Self::MissingCron => write!(f, "core.schedule requires a `cron`"),
            Self::Cron(e) => write!(f, "invalid cron: {e}"),
            Self::UnknownTimezone(tz) => write!(f, "unknown timezone {tz:?}"),
            Self::UnsupportedWhenCondition(w) => {
                write!(
                    f,
                    "core.schedule `when` conditions are not supported: {w:?}"
                )
            }
        }
    }
}

impl std::error::Error for ScheduleError {}

/// A `Schedule` resolved far enough to be driven by a clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleTrigger {
    pub trigger_id: String,
    cron: Cron,
    timezone: Tz,
    late_maximum_delay: Option<Duration>,
    disabled: bool,
}

impl ScheduleTrigger {
    /// Resolves a parsed `Trigger` into something schedulable, or says why it
    /// is not.
    pub fn resolve(trigger: &Trigger) -> Result<Self, ScheduleError> {
        let TriggerKind::Schedule(schedule) = &trigger.kind else {
            return Err(ScheduleError::NotASchedule);
        };

        // `hasWhenCondition()` is `getWhen() != null && !"true".equals(getWhen())`
        // -- so both an absent `when` and the literal default `"true"` mean
        // "no condition".
        if let Some(when) = &trigger.when {
            if when.source() != "true" {
                return Err(ScheduleError::UnsupportedWhenCondition(
                    when.source().to_string(),
                ));
            }
        }

        Self::from_schedule(&trigger.id, schedule, trigger.disabled)
    }

    fn from_schedule(
        trigger_id: &str,
        schedule: &Schedule,
        disabled: bool,
    ) -> Result<Self, ScheduleError> {
        let cron = schedule.cron.as_deref().ok_or(ScheduleError::MissingCron)?;
        let cron = Cron::parse(cron, schedule.with_seconds).map_err(ScheduleError::Cron)?;

        let timezone = match schedule.timezone.as_deref() {
            // See §3: measured `Etc/UTC` inside the pinned image, so an absent
            // `timezone` is UTC rather than a host-dependent guess.
            None => Tz::UTC,
            Some(name) => {
                Tz::from_str(name).map_err(|_| ScheduleError::UnknownTimezone(name.to_string()))?
            }
        };

        Ok(Self {
            trigger_id: trigger_id.to_string(),
            cron,
            timezone,
            // `lateMaximumDelay` is a `java.time.Duration` W1 dropped from the
            // model for want of a parser. It is carried here so the rule can be
            // tested and so a future model field has somewhere to land; nothing
            // in the corpus sets it (§5).
            late_maximum_delay: None,
            disabled,
        })
    }

    /// Overrides `lateMaximumDelay`. Separate from `resolve` because the model
    /// does not carry the field yet — see `from_schedule`.
    #[must_use]
    pub fn with_late_maximum_delay(mut self, delay: Option<Duration>) -> Self {
        self.late_maximum_delay = delay;
        self
    }

    pub fn timezone(&self) -> Tz {
        self.timezone
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn late_maximum_delay(&self) -> Option<Duration> {
        self.late_maximum_delay
    }

    /// `Schedule.convertDateTime` — the same instant, read in this trigger's
    /// zone.
    fn in_zone(&self, at: DateTime<Utc>) -> DateTime<Tz> {
        at.with_timezone(&self.timezone)
    }

    /// `Schedule.scheduleDates`.
    ///
    /// The **`date.minus(1s)`** is §4's measurement: a tick evaluated on its
    /// own instant selects itself, not the following one.
    pub fn schedule_dates(&self, date: DateTime<Utc>) -> Option<ScheduleDates> {
        let local = self.in_zone(date);
        let next = self.cron.next_after(local - Duration::seconds(1))?;
        Some(ScheduleDates {
            date: next.with_timezone(&Utc),
            next: self.cron.next_after(next).map(|d| d.with_timezone(&Utc)),
            previous: self.cron.last_before(local).map(|d| d.with_timezone(&Utc)),
        })
    }

    /// `Schedule.handleMaxDelay`.
    ///
    /// Walks forward to the first tick that has not already outrun its delay
    /// (§5). With no `lateMaximumDelay` the input is returned untouched, which
    /// is the artifact's own first branch and the reason a missed tick is
    /// otherwise replayed rather than dropped.
    fn handle_max_delay(
        &self,
        mut output: Option<ScheduleDates>,
        now: DateTime<Utc>,
    ) -> Option<ScheduleDates> {
        let delay = self.late_maximum_delay?;
        let out = output.as_ref()?;
        let _ = out;

        // The artifact bounds this walk at now() +/- 10 years, on the OUTPUT's
        // year rather than on an iteration count.
        loop {
            let current = output.as_ref()?;
            let year = current.date.year();
            if year >= now.year() + 10 || year <= now.year() - 10 {
                return output;
            }
            if current.date + delay < now {
                let next = current.next?;
                output = self.schedule_dates(next);
            } else {
                return output;
            }
        }
    }

    /// `Schedule.nextEvaluationDate(conditionContext, last)` — when this
    /// trigger should next be looked at.
    ///
    /// `last` is the trigger's stored state: its previous evaluation date and
    /// its backfill, if any. The four branches are §5 and §6:
    ///
    /// - a running backfill computes from `currentDate`, and falls back to the
    ///   live clock the moment the answer passes `end`;
    /// - a previous date with no backfill computes from that date, so missed
    ///   ticks are **replayed one at a time**;
    /// - nothing stored computes from `now`;
    /// - `lateMaximumDelay` then skips ahead — **unless a backfill is
    ///   running**, which is the guard measured to keep a backfill's first tick
    ///   at `06-01T02:00Z` with and without `PT1H`.
    pub fn next_evaluation_date(
        &self,
        last_date: Option<DateTime<Utc>>,
        backfill: Option<&Backfill>,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        let mut next: Option<DateTime<Utc>>;
        let mut in_backfill = false;

        if backfill.is_some() || last_date.is_some() {
            let from = if let Some(bf) = backfill {
                in_backfill = true;
                bf.current_date
            } else {
                last_date.expect("checked by the branch condition")
            };

            next = self
                .cron
                .next_after(self.in_zone(from))
                .map(|d| d.with_timezone(&Utc));

            if let (Some(bf), Some(candidate)) = (backfill, next) {
                if candidate > bf.end {
                    next = self
                        .cron
                        .next_after(self.in_zone(now))
                        .map(|d| d.with_timezone(&Utc));
                }
            }
        } else {
            next = self
                .cron
                .next_after(self.in_zone(now))
                .map(|d| d.with_timezone(&Utc));
        }

        if self.late_maximum_delay.is_some() && next.is_some() && !in_backfill {
            let dates = self.handle_max_delay(self.schedule_dates(next?), now)?;
            next = Some(dates.date);
        }

        next
    }

    /// `Schedule.eval` — given the date this trigger was scheduled for, the
    /// execution to create, or nothing and why.
    ///
    /// Four refusals, all measured off `eval`'s own bytecode:
    /// a paused backfill; a cron that selects no instant at all; a backfill
    /// whose `currentDate` is strictly before the tick it computed; and a tick
    /// more than **one second** in the future.
    pub fn eval(
        &self,
        date: DateTime<Utc>,
        backfill: Option<&Backfill>,
        now: DateTime<Utc>,
    ) -> Result<ScheduleDates, SkipReason> {
        let mut current = date;
        if let Some(bf) = backfill {
            if bf.paused {
                return Err(SkipReason::BackfillPaused);
            }
            current = bf.current_date;
        }

        let dates = self
            .schedule_dates(current)
            .ok_or(SkipReason::NoSuchInstant)?;

        // "we have to make sure that 'current-date' is strictly after the next
        // execution date for an execution to be eligible" -- so a backfill
        // whose currentDate has not reached the tick yet waits.
        if backfill.is_some() && current < dates.date {
            return Err(SkipReason::BackfillNotReached);
        }

        // `if (next.compareTo(SchedulerClock.now().plus(Duration.ofSeconds(1))) > 0)`
        // -- strictly greater than now + 1s, so a tick exactly one second ahead
        // still fires.
        if dates.date > now + Duration::seconds(1) {
            return Err(SkipReason::InTheFuture);
        }

        Ok(dates)
    }
}

/// Why a due tick produced no execution.
///
/// Typed for the same reason `CronError` is: "the scheduler declined" and "the
/// scheduler ran and found nothing" are different facts, and a caller that
/// cannot separate them cannot report either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The trigger carries `disabled: true`.
    Disabled,
    /// A backfill is attached and paused.
    BackfillPaused,
    /// The cron selects no instant within the search bound — the artifact's
    /// `Optional.empty()` (§4).
    NoSuchInstant,
    /// A backfill whose `currentDate` has not reached the computed tick.
    BackfillNotReached,
    /// The computed tick is more than a second ahead of the clock.
    InTheFuture,
}

/// `io.kestra.core.models.triggers.Backfill`, reduced to the window.
///
/// `start` is kept although nothing after creation reads it, because it is
/// what `current_date` is seeded from and a window that cannot say where it
/// began cannot be reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backfill {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub current_date: DateTime<Utc>,
    pub paused: bool,
}

impl Backfill {
    /// `TriggerState.backfill(clock, backfill)` — the two defaults it fills in
    /// on creation: an absent `end` becomes **now**, and an absent
    /// `currentDate` becomes **start**.
    pub fn create(start: DateTime<Utc>, end: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Self {
        Self {
            start,
            end: end.unwrap_or(now),
            current_date: start,
            paused: false,
        }
    }
}
