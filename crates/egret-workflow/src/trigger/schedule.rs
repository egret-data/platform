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
//! ## 0. Measured, derived, or chosen — and this section exists because the
//! difference was invisible
//!
//! Fix round 1 found three defects that shared one cause: **decisions I made
//! were written in the same measured-sounding voice as the ones I measured**,
//! inside a file whose whole warrant is the heading above. So every claim in
//! this module now has to be one of three things, and the ones that are not
//! measured have to say so.
//!
//! - **Measured** — a running program on the pinned artifact answered it, and
//!   the class and method are named.
//! - **Derived** — it follows from something measured, and the derivation is
//!   written down. A derivation can be wrong, so these are not measurements.
//! - **Chosen** — nobody measured it; it is egret's decision. These are listed
//!   here by name, so the list can be audited rather than discovered.
//!
//! **The chosen decisions in this file:**
//!
//! 1. `SEARCH_DAYS = 366 * 20`. The eight-year worst case is derived; the
//!    number 20 is picked. The artifact's own bound was never measured.
//! 2. `CronError` and `SkipReason` being typed enums at all, and which
//!    refusals map to `UnsupportedSyntax` versus `Malformed`. The artifact's
//!    accept/refuse verdicts are measured; this taxonomy is not its taxonomy.
//! 3. Refusing a timezone `chrono-tz` cannot resolve
//!    (`ScheduleError::UnknownTimezone`) rather than coercing it.
//! 4. Refusing a `when` condition (`UnsupportedWhenCondition`).
//! 5. Refusing a declared `lateMaximumDelay`
//!    (`UnsupportedLateMaximumDelay`) instead of parsing the duration.
//! 6. The progress guard in `handle_max_delay`, which the artifact does not
//!    have — and which no test covers; see its own comment.
//! 7. Returning `None` rather than the standing date when that guard fires.
//! 8. Carrying `ScheduledExecution`'s three instants plus a zone instead of
//!    formatted `trigger.*` strings, because the format was never measured.
//!
//! Everything else in this file is measured or derived, and says which.
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
//! - `@DAILY`, `@Daily`, `@nope`, `@secondly`, `@every` →
//!   `contains 1 parts but we expect one of [5]` — nicknames are
//!   case-sensitive lowercase, and an **unrecognised** `@…` is not a nickname
//!   error at all: it falls through to the field count. `@reboot` is the one
//!   token cron-utils recognises but this definition does not enable, which is
//!   why it alone answers `Nickname @reboot not supported!`
//! - `?` in either day field → `Invalid expression: ?`
//! - `W` (`15W`, `LW`) → `Invalid chars … W`
//! - `#` (`1#1`) → `Invalid expression: 1#1`
//! - `L` in **day-of-week** (`5L`, `L`) → `Invalid chars … L`
//!
//! **`W` and `L` are letters before they are special characters, and fix
//! round 1 exists because this section's first draft did not say so.** The
//! refusal list above is measured and correct, and a parser that turned it
//! into "refuse any token containing `W` or `L`" — which is what this file
//! did — refuses `0 0 * * WED`, `0 0 * JUL *`, `MON,WED,FRI`, `MON-WED` and
//! `JAN-JUL`, every one of which the artifact **accepts** (measured:
//! `0 0 * * WED` → 2026-06-17, `0 0 * JUL *` → 2026-07-01). The general
//! sentence was false even though every case under it was true, because the
//! fixture that produced it held one thing fixed: **none of its names
//! contained a `W` or an `L`.**
//! - a reversed range (`5-1`, `6-0`, `7-1`, `SUN-SAT`) → refused in every
//!   field, but by **three different messages**, because a range is only
//!   range-checked once both of its ends parse: `5-1` and `7-1` are
//!   `Invalid range!` wherever their endpoints parse; `6-0` is
//!   `Value 0 not in range [1, 31]` in day-of-month and `[1, 12]` in month;
//!   `SUN-SAT` is `Invalid chars …` in the **other four** fields — the three
//!   that have no names AND month, whose names are `JAN`–`DEC`, so `SUN` is
//!   not one of them — and `Invalid range! [7,6]` only in day-of-week. (This
//!   bullet said
//!   "→ `Invalid range!`" until fix round 4: the claim was corrected at
//!   `parse_field`'s range arm in round 3 and the copy up here was left
//!   behind, which is the same way copies 3–6 of the "only a bare `*`" claim
//!   survived. No verdict moves — egret refuses all twenty combinations.)
//! - a step outside its **own field's** range → `Period N not in range
//!   [min, max]`: `*/60` in minutes, `*/24` in hours, `*/32` in day-of-month,
//!   `*/13` in month, `*/8` and `5/10` in day-of-week — and `*/0`, measured
//!   in all five fields, including minutes, hours and day-of-week, whose
//!   message prints a range that contains the zero it is refusing
//! - a **6-field** expression with `withSeconds: false`, and a **5-field** one
//!   with `withSeconds: true` → `contains N parts but we expect one of [M]`
//!
//! Measured acceptances: `L`, `L-n` and `nL` in day-of-month (`L-3` on a
//! 30-day month is the 27th; a **sole** `5L` is just `L`, the number
//! range-checked and discarded — but in a comma list it is the reverse, the
//! number survives as a plain day and the last-day meaning is dropped, see
//! `parse_day_of_month`), `0-0`, `5/10` in day-of-month, `*/7` in day-of-week, month and
//! day names in either case (`MON`, `mon-fri`, `jan-dec`), `1-7` (every day,
//! because 7 maps to 0), `SAT-SUN` (so `SUN` is **7**, not 0, before the
//! mapping), and whitespace around and between fields.
//!
//! **Whitespace is two different sets, and neither is Rust's.** Fields are
//! separated by Java regex `\s` — `[ \t\n\x0B\f\r]`, ASCII only — while the
//! ends of the expression are trimmed by `String.trim()`, every char
//! `<= U+0020`. So U+001C…U+001F are trimmed but never split on, and U+0085,
//! U+00A0, U+1680, U+2000, U+2028, U+2029, U+202F, U+205F and U+3000 are
//! neither, i.e. `0 0 * * *\u{a0}` is **refused**. And the nickname switch runs
//! **before** any of that, on the raw string, so `"@daily "` is not a nickname
//! at all. See `normalise`.
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
//! - `0 0 * * 1` → **2026-06-22**; `0 0 1 * *` → **2026-07-01**. `*` is
//!   unrestricted, and the other field alone decides.
//! - `0 0 */1 * 1` → **2026-06-22**: `*/1` is unrestricted too, and it is the
//!   **only** other spelling that is. The rule is cron-utils'
//!   `"*".equals(spec.trim()) && "1".equals(stepToken)` — on the step's
//!   spelling, so `*/01`, `*/001`, `*/0001` and `*/+1` are all **restricted**
//!   (→ 2026-06-16) despite selecting the same set — and it is a property of
//!   the whole field, so `*,*`, `*,*/1`, `*/1,*` and `*/1,*/1` are `And`,
//!   hence restricted too (→ 2026-06-16). See `field_is_unrestricted`.
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
    /// Measured: `Nickname @reboot not supported!`.
    ///
    /// **`@reboot` is the only token that reaches this variant**, and fix
    /// round 2 deleted "and every other `@…` that is not one of the seven",
    /// which was false of both the artifact and the code beneath it.
    /// `@secondly`, `@minutely`, `@every`, `@YEARLY`, `@nope` and a bare `@`
    /// are not recognised as nicknames at all: they fall through to the field
    /// count and answer `contains 1 parts but we expect one of [5]`, so they
    /// arrive as [`CronError::WrongFieldCount`]. `@reboot` is the one spelling
    /// cron-utils recognises and this definition does not enable.
    UnsupportedNickname(String),
    /// A token this dialect does not define.
    ///
    /// `?` and `#` anywhere; `W` in a token that is neither a number nor a
    /// name (`1W`, `15W`, `LW`); `L` in **day-of-week** (`5L`, a bare `L`); a
    /// name used in a field that has none (`0 0 JUL * *`); a long name
    /// (`MONDAY`); and anything else unrecognised, `H` included.
    ///
    /// `5L` is on that list for **day-of-week only** — in a list there too
    /// (`5L,1` and `1,5L` are both `Invalid chars … L`, measured). In
    /// day-of-month the artifact accepts it: as the sole term it fires the
    /// last day of the month, and in a comma list it is the plain day 5. Until
    /// fix round 3 this crate refused it outright, and until fix round 4 it
    /// dropped the list form — see `parse_day_of_month`. The earlier
    /// wording said "`W` and `L` in a token that is neither a number nor a
    /// name (`5L`, …)", which named a field-independent class off day-of-week
    /// examples.
    ///
    /// **The artifact answers these with THREE message classes, not one and
    /// not two**, and the position inside the token is what selects between
    /// them (measured, fix round 3):
    ///
    /// - `Invalid chars in expression! Expression: X Invalid chars: Y` —
    ///   `1W`, `15W`, `LW`, `H`, `JUL`/`WED` in day-of-month, `MONDAY`, `5L`
    ///   in day-of-week, and `?`/`#` **inside a range** (`1-?`, `?-1`, `1-#`,
    ///   `#-1`, and `?/1`);
    /// - `Invalid expression: X` — `?` or `#` as a whole term or a comma term
    ///   (`0 0 ? * 1`, `0 0 *,? * *`, `0 0 * * 1#1`, `0 0 * * FRI#2`,
    ///   `0 0 1,# * *`, `0 0 *#1 * *`);
    /// - `For input string: "X"` — `?` or `#` in the **step** (`*/?`, `*/2#3`,
    ///   `*/1#2`).
    ///
    /// They still share one variant here, because what this crate needs to
    /// distinguish is accept from refuse, not one of Kestra's message strings
    /// from another. That is a CHOICE, and it is listed as one in §0.
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
    /// Measured: `Period {step} not in range [{min}, {max}]`, where the bounds
    /// are the **field's own** — `*/60` in minutes, `*/24` in hours, `*/32` in
    /// day-of-month, `*/13` in month and `*/8` in day-of-week are all refused.
    ///
    /// Zero is refused in all five fields — measured, not inferred from three
    /// of them — including minutes, hours and day-of-week, whose message
    /// prints `[0, …]` while refusing the zero inside it. So the accepted
    /// interval is `1..=max`, not the one the text names. Fix round 3 widened
    /// this from a zero-only check.
    StepOutOfRange { field: Field, step: u32 },
    /// A **structural** fault: an empty comma term, a step that is not a
    /// number, an `L-` with a non-numeric tail, a range missing an end.
    ///
    /// **This variant does NOT mirror a class of the artifact's, and fix
    /// round 2 removed a sentence claiming it did.** Measured, the artifact
    /// splits these five across FOUR classes, two of which are the same
    /// `Invalid chars` that [`CronError::UnsupportedSyntax`] also covers:
    ///
    /// | expression | the artifact's answer |
    /// |---|---|
    /// | `*/x * * * *` | `For input string: "X"` |
    /// | `5/ * * * *` | `Missing steps for expression: 5/` |
    /// | `0 0 L- * *` | `Missing values for range: L-` |
    /// | `0 0 * * ,` | `Invalid field value! Trailing commas not permitted!` |
    /// | `0,,2 * * * *` | **`Invalid chars in expression!`** |
    /// | `0 0 L-x * *` | **`Invalid chars in expression!`** |
    ///
    /// So egret's two variants **cross-cut** Kestra's classes rather than
    /// reproduce them. The measured fact is the refusal; the variant is
    /// egret's, and §0 lists it as chosen.
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
            Self::StepOutOfRange { field, step } => {
                let (min, max) = field.range();
                write!(f, "{field}: period {step} not in range [{min}, {max}]")
            }
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
    ///
    /// **Empty whenever the day-of-month field has more than one comma term**,
    /// because the artifact drops `L` terms from a list — see
    /// `parse_day_of_month`. `L-n` is range-checked to 1..=31 there, so this
    /// set can hold 31, which `date_matches` then never satisfies.
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
    /// Whether the day-of-month field was something other than `*` or `*/1`
    /// — see [`field_is_unrestricted`].
    dom_restricted: bool,
    /// Whether the day-of-week field was something other than `*` or `*/1`.
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
        // `CronParser.parse`, read off the artifact's own bytecode rather than
        // inferred from examples (fix round 3):
        //
        //   String replaced = expression.replaceAll("\\s+", " ").trim();
        //   if (isEmpty(replaced)) throw "Empty expression!";
        //   if (expression.startsWith("@")) { switch (expression) { ... } }   // <- RAW
        //   ... split `replaced` into fields ...
        //
        // Two things follow that this crate had wrong, and both are measured
        // below rather than argued.
        let normalised = normalise(expr);
        if normalised.is_empty() {
            return Err(CronError::Empty);
        }

        // (1) The nickname switch runs on the **raw** expression (`aload_1` at
        // offsets 44 and 72), NOT on the trimmed one, and it is an exact
        // `String.equals`. So a recognised nickname with ANY padding is not a
        // nickname: it falls through to the field count and fails it.
        // Measured -- 15 paddings (`" "`, `"\t"`, `"\n"`, `"\r"`, `"\u{0b}"`,
        // `"\u{0c}"`, `"\u{1c}"`, `"\u{a0}"` and repeats, leading and
        // trailing) on all seven enabled nicknames: **105 rows, all 105
        // refused** with `contains 1 parts but we expect one of [5]`. This
        // crate trimmed first and scheduled **91** of them; the 14 it already
        // refused are the U+001C paddings, which Rust's `trim` leaves alone
        // too.
        //
        // The exact match also subsumes the whitespace guard that used to be
        // here: `@daily extra` simply does not equal `@daily`.
        //
        //   @reboot            -> "Nickname @reboot not supported!"   (recognised, not enabled)
        //   @secondly, @minutely, @every, @YEARLY, @nope, @
        //                      -> "contains 1 parts but we expect one of [5]"
        //                                                             (not recognised at all)
        //   @daily extra       -> "contains 2 parts ..."
        //
        // So an unrecognised `@...` is NOT a nickname error -- it falls through
        // to the field count, and a one-token expression fails it.
        {
            if let Some(rest) = expr.strip_prefix('@') {
                match rest {
                    // The expansion is always five fields, whatever
                    // `with_seconds` says. Fix round 1 asserted that for all
                    // seven nicknames having measured **two**; fix round 2
                    // measured all seven under BOTH parsers (probe 11) and
                    // every one answers identically with `withSeconds` on and
                    // off -- @yearly/@annually 2027-01-01, @monthly
                    // 2026-07-01, @weekly 2026-06-21, @daily/@midnight
                    // 2026-06-16, @hourly 2026-06-15T13:00.
                    "yearly" | "annually" => return Self::parse_fields("0 0 1 1 *", false),
                    "monthly" => return Self::parse_fields("0 0 1 * *", false),
                    "weekly" => return Self::parse_fields("0 0 * * 0", false),
                    "daily" | "midnight" => return Self::parse_fields("0 0 * * *", false),
                    "hourly" => return Self::parse_fields("0 * * * *", false),
                    "reboot" => return Err(CronError::UnsupportedNickname(expr.to_string())),
                    // Everything else falls through, deliberately.
                    _ => {}
                }
            }
        }

        Self::parse_fields(&normalised, with_seconds)
    }

    fn parse_fields(expr: &str, with_seconds: bool) -> Result<Self, CronError> {
        // `expr` has already been through `normalise`, so every separator is a
        // single space and there are none at the ends. Splitting on `' '` is
        // then exactly the artifact's own split, including the cases where it
        // does NOT split -- see `normalise`.
        let parts: Vec<&str> = expr.split(' ').collect();
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
    /// means either may match; otherwise the unrestricted one is `*` or `*/1`
    /// (see [`field_is_unrestricted`]), whose value set holds every day either
    /// way, so the plain conjunction is the same answer.
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

/// `expression.replaceAll("\\s+", " ").trim()`, with Java's definitions of
/// both, because Rust's are a different set **in both directions**.
///
/// Java's regex `\s` is `[ \t\n\x0B\f\r]` -- ASCII only -- while
/// `String.trim()` removes every char `<= U+0020`. Rust's `str::trim` and
/// `split_whitespace` use the Unicode `White_Space` property instead. Measured
/// on the artifact, one character at a time, in three positions each
/// (fix round 3, probe FR3C):
///
/// | chars | splits fields? | trimmed at the ends? | Rust agrees? |
/// |---|---|---|---|
/// | `\t \n \x0B \f \r` and space | yes | yes | yes |
/// | U+001C…U+001F | **no** (`0 0 *\u{1c}* *` is "4 parts") | **yes** | no -- Rust trims neither |
/// | U+0085, U+00A0, U+1680, U+2000, U+2028, U+2029, U+202F, U+205F, U+3000 | **no** | **no** (`0 0 * * *\u{a0}` is `Invalid chars`) | no -- Rust does both |
///
/// Using Rust's set made `"0 0 * * *\u{a0}"` and `"0 0 *\u{a0}* *"` fire where
/// the artifact refuses them: over-acceptance, the direction the oracle cannot
/// catch.
fn normalise(expr: &str) -> String {
    let mut out = String::with_capacity(expr.len());
    let mut in_run = false;
    for c in expr.chars() {
        if is_java_regex_space(c) {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            out.push(c);
            in_run = false;
        }
    }
    // `String.trim()`: every char <= U+0020, which is a SUPERSET of the split
    // set (U+001C..=U+001F are trimmed but never split on).
    out.trim_matches(|c: char| c <= '\u{20}').to_string()
}

/// Java regex `\s` with no `UNICODE_CHARACTER_CLASS` flag: `[ \t\n\x0B\f\r]`.
fn is_java_regex_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r')
}

/// Is this whole field token the one cron-utils turns into `Always` — the
/// thing egret calls **unrestricted**?
///
/// `FieldParser.asteriskOrempty`, read off the bytecode (fix round 3):
///
/// ```text
/// if ("*".equals(spec.trim()) && "1".equals(stepToken)) return noSpecialCharsNorStar(spec);
/// //                             ^^^^^^^^^^^^^^^^^^^^ String.equals on the TOKEN
/// ```
///
/// So the rule is on the step's **spelling**, not its value, and it is a
/// whole-field property, not a per-term one. Measured, in **both** day fields
/// (`Cron.retrieve(f).getExpression()` plus two discriminating fixtures):
///
/// ```text
/// *          Always   <- unrestricted     */01     Every   <- restricted
/// */1        Always   <- unrestricted     */001    Every
/// */2        Every                        */0001   Every
/// */30       Every                        */+1     Every
/// 1-31       Between                      *,*      And     <- restricted
/// 1-31/1     Every                        */1,*/1  And     <- restricted
/// ```
///
/// Fix round 2 read this off one fixture that varied only the step's *value*
/// and wrote `step != 1`, which broke `*/01`, `*/001`, `*/+1` and `*/0001`
/// (they select every day, so they are the same SET as `*`, and cron-utils
/// still does not collapse them). Fix round 3 then had to vary the second
/// thing that fixture held fixed -- the number of comma terms -- and found
/// `*,*`, `*,*/1`, `*/1,*` and `*/1,*/1`, which are `And`, hence restricted,
/// and which every earlier rule got wrong in the same direction.
fn field_is_unrestricted(token: &str) -> bool {
    match token.split_once('/') {
        None => token == "*",
        Some((spec, step)) => spec == "*" && step == "1",
    }
}

/// Day-of-month is parsed on its own because it is the only field with `L`.
///
/// Returns `(values, restricted, last_offsets)`.
fn parse_day_of_month(token: &str) -> Result<(BTreeSet<u32>, bool, BTreeSet<u32>), CronError> {
    let mut values = BTreeSet::new();
    let mut last_offsets = BTreeSet::new();
    // Whole-token property, exactly as in `parse_field`: an `L`, an `L-n`, an
    // `nL` and any comma at all are all `Every`/`On`/`And`, never `Always`.
    let restricted = !field_is_unrestricted(token);

    // How many comma-separated terms the field has. It decides whether an `L`
    // term counts at all -- see the `l_terms_are_dropped` note below.
    let term_count = token.split(',').count();

    for part in token.split(',') {
        // No `part.trim()` -- see `parse_field`.
        let upper = part.to_ascii_uppercase();
        if upper == "L" {
            // MEASURED UPSTREAM DEFECT, reproduced rather than tidied.
            //
            // An `L` term takes effect only when it is the field's SOLE term.
            // In a comma list cron-utils drops every `L`-form term and keeps
            // only the plain ones. Measured on the artifact, walking five
            // executions from 2026-06-01:
            //
            //   `L`          -> 06-30, 07-31, 08-31, 09-30, 10-31
            //   `L,1`, `1,L` -> 07-01, 08-01, 09-01, 10-01, 11-01  (L dropped)
            //   `L,15`,`15,L`-> 06-15, 07-15, 08-15, 09-15, 10-15  (L dropped)
            //   `2,L`        -> 06-02, 07-02, 08-02, 09-02, 10-02  (L dropped)
            //   `L-1,15`     -> 06-15, 07-15, ...                  (L-1 dropped)
            //   `L,L-1`      -> NOTHING AT ALL, in either direction
            //
            // Reproducing it is the fidelity rule ("reproduce observable
            // behaviour, including upstream defects"), and here it is also the
            // safe direction twice over: a union would fire on 06-30 for
            // `L,1`, which is precisely the over-acceptance the oracle cannot
            // catch.
            if term_count == 1 {
                last_offsets.insert(0);
            }
        } else if let Some(off) = upper.strip_prefix("L-") {
            let off: u32 = off.parse().map_err(|_| CronError::Malformed {
                field: Field::DayOfMonth,
                token: part.to_string(),
            })?;
            // MEASURED, and fix round 1 corrected both ends of it. The
            // artifact range-checks the number after `L-` against the
            // day-of-month field's own 1..=31:
            //
            //   `L-0`   -> refused, `Value 0 not in range [1, 31]`
            //   `L-1`   -> 2026-06-29   (June has 30 days)
            //   `L-30`  -> 2026-07-01
            //   `L-31`  -> PARSES, and then never matches anything
            //   `L-32`  -> refused, `Value 32 not in range [1, 31]`
            //   `L-100` -> refused, `Value 100 not in range [1, 31]`
            //
            // The first draft allowed 0 and refused 31, i.e. it was wrong at
            // BOTH ends: it fired on the last day for `L-0` where the artifact
            // refuses the expression outright, and it refused `L-31`, which
            // the artifact accepts as an expression that simply never fires.
            // `L-31` needs no special case here -- `last_day - 31` underflows,
            // `checked_sub` yields `None`, and the day never matches.
            if !(1..=31).contains(&off) {
                return Err(CronError::OutOfRange {
                    field: Field::DayOfMonth,
                    value: i64::from(off),
                    min: 1,
                    max: 31,
                });
            }
            if term_count == 1 {
                last_offsets.insert(off);
            }
        } else if let Some(nth) = upper.strip_suffix('L').filter(|n| !n.is_empty()) {
            // `nL` in DAY-OF-MONTH is accepted, and **which of its two
            // halves survives depends on whether it is the field's sole
            // term**. Measured on the artifact over 34 expressions x 6 clocks
            // (fix round 4; fix round 3 had the sole-term half right and the
            // list half exactly backwards):
            //
            //   SOLE TERM -- the number is range-checked and then DISCARDED,
            //   and the term means the last day:
            //     `5L`, `1L`, `15L`, `31L`, `5l` -> 06-30, 07-31, 08-31
            //     `0L`, `32L`                    -> `Value N not in range [1, 31]`
            //     `L5`, `5LW`, `5L-1`            -> refused
            //
            //   IN A COMMA LIST -- the LAST-DAY meaning is discarded and the
            //   NUMBER survives, as a plain day. This is the opposite of
            //   what a bare `L` or an `L-n` does in a list (those are dropped
            //   whole), and the previous two sentences here said so in the
            //   measured voice while asserting the reverse:
            //     `20L,1`  from 06-15 -> 06-20, 07-01, 07-20   (not 07-01 first)
            //     `5L,1`   from 06-02 -> 06-05, 07-01, 07-05
            //     `20L,25L` from 06-26 -> 07-20, 07-25, 08-20  (never 06-30)
            //     `L,5L`   from 06-15 -> 07-05, 08-05, 09-05   (`L` dropped,
            //                                                   `5L` is day 5)
            //     `31L,1`  from 06-15 -> 07-01, 07-31, 08-01   (31 is a PLAIN
            //               day: June has none, so June is skipped -- the
            //               number is not clamped to the month's last day)
            //     `29L,1`  from 01-20 -> 01-29, 02-01, 03-01   (no 02-29)
            //   The range check still runs on the list form (`0L,1` and
            //   `32L,1` are refused), so it stays above this branch.
            //
            // Reproduced rather than tidied, like the `L`-in-a-list defect
            // above: cron-utils builds an `And` of `On` expressions for a
            // comma field and reads the nominal value off each `On`, ignoring
            // the special character it carries.
            let nth: u32 = nth.parse().map_err(|_| CronError::UnsupportedSyntax {
                field: Field::DayOfMonth,
                token: part.to_string(),
            })?;
            if !(1..=31).contains(&nth) {
                return Err(CronError::OutOfRange {
                    field: Field::DayOfMonth,
                    value: i64::from(nth),
                    min: 1,
                    max: 31,
                });
            }
            if term_count == 1 {
                last_offsets.insert(0);
            } else {
                values.insert(nth);
            }
        } else {
            let (vals, _) = parse_field(part, Field::DayOfMonth)?;
            values.extend(vals);
        }
    }

    // THERE IS NO `!restricted && values.is_empty()` GUARD HERE, AND THAT IS
    // A MEASUREMENT.
    //
    // Fix round 3 kept one and argued that half of it was load-bearing. Both
    // halves were checked in fix round 4: deleting the whole condition leaves
    // 54/54 `trigger::` tests green, so nothing depends on it in either
    // direction -- the conjunction is never evaluated to `true`, because
    // `restricted` is false only for the whole-token spellings `*` and `*/1`
    // and both of those fill `values`. The retention argument was about a
    // DIFFERENT guard, the one you get by dropping `!restricted`, which would
    // indeed refuse a bare `L`.
    //
    // `values` being empty is legitimate and must stay legitimate: `L`, `L-n`
    // and a sole `nL` all put their day in `last_offsets` and nothing in
    // `values`. A guard nobody has seen fail is a guard nobody has seen, so
    // this one is gone rather than kept with a justification for a condition
    // it does not have -- the same disposition as the `?`/`#` fence (F1).
    Ok((values, restricted, last_offsets))
}

/// Parses one comma-separated cron field.
///
/// Returns `(values, restricted)`. `restricted` is the negation of
/// [`field_is_unrestricted`] — **false for exactly two whole-field spellings,
/// `*` and `*/1`** — and it is what decides the day-of-month / day-of-week
/// rule in §2. It is measured **true** for `*/2`, `*/01`, `1-31`, `1-31/1`,
/// `*,*` and `*/1,*/1`.
///
/// (This sentence has been wrong twice. Fix round 2 left it reading
/// "false only for a bare `*` … and false for `*/2` and for `1-31`", which
/// both missed `*/1` and inverted its own measurement. Fix round 3's audit
/// had flagged the row and the round did not act on it, which is why the
/// sweep is now a program — see the report.)
fn parse_field(token: &str, field: Field) -> Result<(BTreeSet<u32>, bool), CronError> {
    let (min, max) = field.range();
    let mut values = BTreeSet::new();
    // Restrictedness is a property of the WHOLE field token, not of its terms
    // -- see `field_is_unrestricted`.
    let restricted = !field_is_unrestricted(token);

    for part in token.split(',') {
        // No `part.trim()`. A field token can hold no character the artifact
        // trims: the separators are exactly `is_java_regex_space`, and every
        // other trimmable char (U+001C..=U+001F) is refused outright inside a
        // field -- measured, `0 0 *\u{1c} * *` and `0 0 *\u{1c}/1 * *` are both
        // `Invalid chars in expression!`. Trimming with Rust's Unicode set
        // additionally ate U+00A0, which the artifact also refuses.
        if part.is_empty() {
            return Err(CronError::Malformed {
                field,
                token: token.to_string(),
            });
        }

        // THERE IS NO `?`/`#` FENCE HERE, AND THAT IS A MEASUREMENT.
        //
        // Fix round 1 put one here and documented it as load-bearing. Fix round
        // 2 mutated it away (`F1-question-hash-fence-removed`) and **no test
        // died**: `parse_value`'s final arm already refuses `?`, `1#1` and
        // `FRI#2` as `UnsupportedSyntax` on the same field, because they are
        // neither a number nor a name. A guard nobody has seen fail is a guard
        // nobody has seen, so the redundant one is gone rather than left with a
        // comment claiming work it was not doing.
        //
        // What must NOT come back is a scan for `W` and `L` here. They are
        // special characters only in day-of-month; in the two fields that have
        // names they are ordinary letters -- `WED` contains a `W`, `JUL`
        // contains an `L`. Scanning for them *before* names resolve refuses
        // `0 0 * * WED`, `0 0 * JUL *`, `MON,WED,FRI`, `MON-WED` and
        // `JAN-JUL`, every one of which the artifact ACCEPTS (`0 0 * * WED`
        // fires 2026-06-17, `0 0 * JUL *` fires 2026-07-01). That was C1.

        let (spec, step) = match part.split_once('/') {
            Some((spec, step)) => {
                let step: u32 = step.parse().map_err(|_| CronError::Malformed {
                    field,
                    token: part.to_string(),
                })?;
                // The step is range-checked against the FIELD'S OWN range,
                // and this crate checked only for zero until fix round 3's
                // sweep put every token the file names to the artifact:
                //
                //   */59 minute  OK      */60 minute  Period 60 not in range [0, 59]
                //   */23 hour    OK      */24 hour    Period 24 not in range [0, 23]
                //   */31 dom     OK      */32 dom     Period 32 not in range [1, 31]
                //   */12 month   OK      */13 month   Period 13 not in range [1, 12]
                //   */7  dow     OK      */8  dow     Period 8 not in range [0, 7]
                //   5/10 dow     Period 10 not in range [0, 7]
                //
                // **22 distinct expressions** across the sweep's corpora,
                // every one ACCEPTED here and fired -- `0 */30 * * *` is a
                // plausible thing to write, and Kestra refuses it. Zero is
                // refused in all five fields (measured in each, including
                // minutes, hours and day-of-week, whose message prints a
                // `[0, ...]` that contains the value it rejects), so the bound
                // is `1 <= step <= max`, not the interval the text prints.
                if step == 0 || step > max {
                    return Err(CronError::StepOutOfRange { field, step });
                }
                (spec, Some(step))
            }
            None => (part, None),
        };

        let (from, to) = if spec == "*" {
            (min, max)
        } else if let Some((lo, hi)) = spec.split_once('-') {
            let lo = parse_value(lo, field)?;
            let hi = parse_value(hi, field)?;
            // `withStrictRange()` on every field: measured, `5-1` and `7-1`
            // answer `Invalid range!` in every field that accepts their
            // endpoints.
            //
            // The class stops there, and fix round 3's sweep is what found the
            // edge: the SAME four expressions reach three different messages,
            // because a range is only range-checked once both ends parse.
            // `6-0` answers `Value 0 not in range [1, 31]` in day-of-month and
            // `[1, 12]` in month -- the zero is out of range before the
            // reversal is looked at -- and `SUN-SAT` answers `Invalid chars in
            // expression!` in the other FOUR fields: the three that have no
            // names and month, whose names are `JAN`-`DEC`, so `SUN` is not
            // one of them either. ("the three fields that have no names" is
            // what round 3 wrote and what the re-review endorsed; fix round 4
            // re-measured all twenty combinations and month is a fourth.)
            // Only in day-of-week are all four `Invalid range!`. egret refuses every
            // one of them, in every field, so no verdict moves; the class
            // claim was still wider than its examples.
            if lo > hi {
                return Err(CronError::InvalidRange {
                    field,
                    from: lo,
                    to: hi,
                });
            }
            (lo, hi)
        } else {
            let v = parse_value(spec, field)?;
            // `5/10` means 5, 15, 25 … up to the field maximum -- measured.
            // A bare `5` is just 5.
            let hi = if step.is_some() { max } else { v };
            (v, hi)
        };

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
    // No `token.trim()` -- see `parse_field`.
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
            // Not a number and not a name. NOW `W` and `L` mean what the
            // dialect says they mean -- a special character it does not define
            // outside day-of-month -- so `1W`, `15W`, and `5L` and a bare `L`
            // **in day-of-week**, land here and are refused as unsupported
            // syntax, while `WED` and `JUL` never reach this arm at all.
            //
            // `5L` is field-scoped as of fix round 3: in DAY-OF-MONTH it never
            // reaches `parse_field` at all, because `parse_day_of_month`
            // handles it -- the artifact accepts it there.
            //
            // A three-letter name used in the WRONG field (`0 0 JUL * *`,
            // `0 0 WED * *`) also lands here, and the artifact refuses it too
            // -- measured, `Invalid chars in expression! Expression: JUL
            // Invalid chars: JUL`. So does a long name (`MONDAY`). They are
            // reported as unsupported syntax rather than `Malformed` because
            // that is the complaint the artifact itself makes about them.
            //
            // One variant, no special case. An earlier draft of this arm
            // split these by whether the token contained a `W` or an `L`,
            // which was a distinction this parser invented; `5L`, `1W`,
            // `JUL`-in-day-of-month, `MONDAY` and `H` in the minute field all
            // answer `Invalid chars in expression!` on the artifact.
            //
            // What the draft's REPLACEMENT sentence then got wrong -- fix
            // round 2 -- was generalising that to "the artifact makes the same
            // complaint about every one of these": `?` and `#` answer
            // `Invalid expression:` or `For input string:` instead, depending
            // on where in the token they sit, and two of `Malformed`'s own
            // cases answer `Invalid chars`. See `CronError::UnsupportedSyntax`
            // for the three-class table and `CronError::Malformed` for the
            // cross-cut.
            None => {
                return Err(CronError::UnsupportedSyntax {
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
    /// `Schedule.lateMaximumDelay` changes which tick fires (see §5) and this
    /// crate's model carries it only as an uninterpreted value, because
    /// reproducing `java.time.Duration.parse` is a measurement this package
    /// has not made. Scheduling anyway would replay a backlog the artifact
    /// throws away -- measured: five days behind, `0 2 * * *` answers
    /// `2026-06-11T02:00Z` without the delay and `2026-06-16T02:00Z` with
    /// `PT1H`. So a `Schedule` that declares it is refused rather than
    /// silently scheduled without it.
    UnsupportedLateMaximumDelay(String),
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
            Self::UnsupportedLateMaximumDelay(d) => {
                write!(f, "core.schedule `lateMaximumDelay` is not supported: {d}")
            }
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

/// `Schedulable.recoverMissedSchedules` (`Schedulable.java:47-51`): which of
/// the ticks missed while a trigger was off a re-enable replays. Read only by
/// the trigger switch route (W7e spec §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverMissed {
    All,
    None,
    Last,
}

impl RecoverMissed {
    /// Absent = `All`. Anything but the strings "ALL", "NONE", "LAST" is
    /// `Err(raw value rendered as JSON)`.
    pub fn from_model(v: Option<&serde_json::Value>) -> Result<Self, String> {
        match v {
            None => Ok(Self::All),
            Some(serde_json::Value::String(s)) if s == "ALL" => Ok(Self::All),
            Some(serde_json::Value::String(s)) if s == "NONE" => Ok(Self::None),
            Some(serde_json::Value::String(s)) if s == "LAST" => Ok(Self::Last),
            Some(other) => Err(other.to_string()),
        }
    }
}

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
        if let Some(delay) = &schedule.late_maximum_delay {
            return Err(ScheduleError::UnsupportedLateMaximumDelay(
                delay.to_string(),
            ));
        }

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
            // Always `None` on this path, and that is now a CONSEQUENCE
            // rather than an omission: `resolve` has already refused any
            // `Schedule` that declares `lateMaximumDelay`, because this crate
            // models the key but not the duration inside it. The only way to
            // set it is `with_late_maximum_delay`, which the tests use to pin
            // the measured rule.
            late_maximum_delay: None,
            disabled,
        })
    }

    /// Sets `lateMaximumDelay` directly, bypassing `resolve`'s refusal.
    ///
    /// **Not reachable from YAML, and since fix round 2 not reachable from a
    /// shipping build at all — by construction, not by convention.**
    ///
    /// It is `#[cfg(test)] pub(crate)`. Narrowing it from `pub` made clippy
    /// report it unused in the lib target, which was the honest signal: every
    /// one of its four call sites is in a `#[cfg(test)]` module. So rather than
    /// silence that with an `allow`, the method now does not exist outside a
    /// test build. That is strictly stronger than `pub(crate)` and is what
    /// "test-only" should always have meant here. The
    /// model now carries the key (so it round-trips and so `resolve` can see
    /// it) but not a parsed duration, so `resolve` refuses any `Schedule` that
    /// declares it rather than scheduling as though it were absent. This
    /// constructor is how the measured rule is still exercised — see
    /// `the_missed_tick_rule_matches_the_artifact`. When someone parses
    /// `java.time.Duration` properly, `from_schedule` should use it and this
    /// can stay as it is.
    ///
    /// **`pub(crate)`, and that is the whole point.** Fix round 1 wrote that
    /// "no YAML can reach it" — true, but while this was `pub` it was
    /// *discipline*, not construction: any caller outside the crate could set
    /// a delay `resolve` refuses, which is exactly the over-acceptance the
    /// refusal exists to close. Unrepresentable beats unforgettable, which is
    /// the same argument that chose refuse-over-parse in the first place.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn with_late_maximum_delay(mut self, delay: Option<Duration>) -> Self {
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

    /// The latest tick `t <= at` in this trigger's zone. `Cron::last_before`
    /// is STRICTLY before, so it is asked about `at + 1ns`.
    pub fn last_tick_at_or_before(&self, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.cron
            .last_before(self.in_zone(at) + Duration::nanoseconds(1))
            .map(|d| d.with_timezone(&Utc))
    }

    /// The ticks `t` with `from <= t <= at`, counted up to `cap`:
    /// `(count, more)` where `more` means "there are more than `cap`".
    pub fn ticks_through(&self, from: DateTime<Utc>, at: DateTime<Utc>, cap: u32) -> (u32, bool) {
        let end = self.in_zone(at);
        let mut n = 0u32;
        let mut cur = self
            .cron
            .next_after(self.in_zone(from) - Duration::nanoseconds(1));
        while let Some(t) = cur {
            if t > end {
                break;
            }
            if n == cap {
                return (n, true);
            }
            n += 1;
            cur = self.cron.next_after(t);
        }
        (n, false)
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

        // The artifact bounds this walk at now() +/- 10 years, on the OUTPUT's
        // year rather than on an iteration count -- and that bound is the ONLY
        // one it has.
        loop {
            let current = output.as_ref()?;
            let year = current.date.year();
            if year >= now.year() + 10 || year <= now.year() - 10 {
                return output;
            }
            if current.date + delay >= now {
                return output;
            }

            let standing = current.date;
            let next = current.next?;
            let advanced = self.schedule_dates(next);

            // PROGRESS GUARD, and it is not in the artifact.
            //
            // Kestra's `handleMaxDelay` is a `while` over the output's YEAR: if
            // the walk ever produced a date that did not move, the year would
            // not move either and the loop would spin for ever. Upstream that
            // cannot happen, because cron-utils' `nextExecution` is strictly
            // after its argument -- so the bound is sound only as a consequence
            // of a property of a different class.
            //
            // This guard makes the termination local instead. It is
            // UNREACHABLE through this crate's public API, because
            // `Cron::next_after` is strict for the same reason; its one
            // demonstration is mutation **M2**, which relaxes `next_after` to
            // `>=` and, before this guard existed, turned the whole test suite
            // into a hang that had to be killed by hand -- a mutation that
            // hangs is indistinguishable from a mutation nothing detects.
            //
            // Refusing (`None`) rather than returning the standing date is the
            // safe direction: a schedule that cannot make progress produces no
            // execution, instead of firing a tick the artifact would have
            // walked past.
            match advanced {
                Some(a) if a.date > standing => output = Some(a),
                _ => return None,
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

#[cfg(test)]
mod tests {
    use super::{
        Cron, CronError, Field, RecoverMissed, ScheduleError, ScheduleTrigger, SkipReason,
    };
    use crate::models::Trigger;
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use chrono_tz::Tz;
    use std::str::FromStr;

    /// The clock every measurement in this file was taken against.
    ///
    /// A constant, not `Utc::now()`: a scheduler is nothing but a clock, and a
    /// test that reads the wall clock is a test that passes at 11:59 and fails
    /// at midnight. `egret-query/tests/connector_trino_pool.rs` once carried
    /// three of those and spent four gate runs on them; it has none now.
    fn probe_clock() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap()
    }

    fn at(iso: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(iso)
            .unwrap_or_else(|e| panic!("bad fixture instant {iso:?}: {e}"))
            .with_timezone(&Utc)
    }

    fn tz(name: &str) -> Tz {
        Tz::from_str(name).unwrap_or_else(|_| panic!("bad fixture zone {name:?}"))
    }

    /// Builds a `ScheduleTrigger` through the **model**, i.e. the same YAML path
    /// a published flow takes, rather than through a private constructor -- so
    /// a model change that stopped feeding the scheduler shows up here.
    fn sched(cron: &str, with_seconds: bool, timezone: Option<&str>) -> ScheduleTrigger {
        let mut yaml = format!("id: t\ntype: core.schedule\ncron: \"{cron}\"\n");
        if with_seconds {
            yaml.push_str("withSeconds: true\n");
        }
        if let Some(zone) = timezone {
            yaml.push_str(&format!("timezone: \"{zone}\"\n"));
        }
        let trigger: Trigger = serde_norway::from_str(&yaml)
            .unwrap_or_else(|e| panic!("fixture yaml did not parse: {e}\n{yaml}"));
        ScheduleTrigger::resolve(&trigger)
            .unwrap_or_else(|e| panic!("fixture did not resolve: {e}\n{yaml}"))
    }

    // ---------------------------------------------------------------------
    // W7e: recoverMissedSchedules + tick helpers
    // ---------------------------------------------------------------------

    #[test]
    fn recover_missed_from_model() {
        use serde_json::json;
        assert_eq!(RecoverMissed::from_model(None), Ok(RecoverMissed::All));
        for (s, want) in [
            ("ALL", RecoverMissed::All),
            ("NONE", RecoverMissed::None),
            ("LAST", RecoverMissed::Last),
        ] {
            assert_eq!(RecoverMissed::from_model(Some(&json!(s))), Ok(want));
        }
        for bad in [json!("all"), json!(true), json!(1), json!("SOME")] {
            assert_eq!(
                RecoverMissed::from_model(Some(&bad)),
                Err(bad.to_string()),
                "{bad}"
            );
        }
    }

    #[test]
    fn last_tick_at_or_before_includes_the_instant() {
        let s = sched("*/15 * * * *", false, None);
        assert_eq!(
            s.last_tick_at_or_before(at("2026-06-15T10:07:00Z")),
            Some(at("2026-06-15T10:00:00Z"))
        );
        assert_eq!(
            s.last_tick_at_or_before(at("2026-06-15T10:15:00Z")),
            Some(at("2026-06-15T10:15:00Z"))
        );
        let z = sched("0 2 * * *", false, Some("Asia/Ho_Chi_Minh"));
        assert_eq!(
            z.last_tick_at_or_before(at("2026-06-15T20:00:00Z")),
            Some(at("2026-06-15T19:00:00Z"))
        );
    }

    #[test]
    fn ticks_through_counts_inclusive_with_cap() {
        let s = sched("*/1 * * * *", false, None);
        let t = |x: &str| at(x);
        assert_eq!(
            s.ticks_through(t("2026-06-15T10:00:00Z"), t("2026-06-15T10:04:30Z"), 100),
            (5, false)
        );
        assert_eq!(
            s.ticks_through(t("2026-06-15T10:05:00Z"), t("2026-06-15T10:00:00Z"), 100),
            (0, false)
        );
        assert_eq!(
            s.ticks_through(t("2026-06-15T10:00:00Z"), t("2026-06-15T10:00:00Z"), 100),
            (1, false)
        );
        assert_eq!(
            s.ticks_through(t("2026-06-15T10:00:00Z"), t("2026-06-15T10:04:30Z"), 3),
            (3, true)
        );
    }

    // ---------------------------------------------------------------------
    // 1. The dialect
    // ---------------------------------------------------------------------

    /// Every row's **accept-or-refuse verdict** is an answer the pinned
    /// artifact actually gave (probes 1, 4, 5, 6, 8 and 10 in
    /// `scratchpad/w6t5/`). That half is measured.
    ///
    /// **The `Want` variant is NOT measured — it is egret's own taxonomy**, and
    /// saying so is the point. Kestra reports most refusals through one
    /// message class (`Invalid chars in expression!`), which is why fix round 1
    /// collapsed `H`, `5L`-in-day-of-week, `MONDAY` and `JUL`-in-day-of-month
    /// into a single
    /// `UnsupportedSyntax`: the earlier split between them was a distinction
    /// this parser invented. `Malformed` is kept for the structural faults the
    /// artifact *does* complain about differently (`Missing steps for
    /// expression`, `For input string`, `Trailing commas not permitted`), so
    /// the two variants correspond to something rather than to nothing.
    ///
    /// The refusals are the half that matters. **Over-acceptance must not
    /// ship**: a cron this parser accepts and Kestra rejects is a trigger that
    /// fires where Kestra would not, and the oracle cannot catch it.
    #[test]
    fn the_dialect_is_the_artifacts_dialect() {
        #[derive(Debug)]
        enum Want {
            Ok,
            Nickname,
            FieldCount(usize, usize),
            Unsupported,
            OutOfRange,
            BadRange,
            StepOutOfRange,
            Empty,
            Malformed,
        }
        use Want::*;

        // (expression, with_seconds, expected)
        let table: &[(&str, bool, Want)] = &[
            // --- accepted, measured ---
            ("0 2 * * *", false, Ok),
            ("*/15 * * * *", false, Ok),
            ("@daily", false, Ok),
            ("@hourly", false, Ok),
            ("@weekly", false, Ok),
            ("@monthly", false, Ok),
            ("@yearly", false, Ok),
            ("@annually", false, Ok),
            ("@midnight", false, Ok),
            ("0 0 * * 7", false, Ok),
            ("0 0 * * 0", false, Ok),
            ("0 0 L * *", false, Ok),
            ("0 0 L-3 * *", false, Ok),
            ("0 0 * * MON", false, Ok),
            ("0 0 * * mon-fri", false, Ok),
            ("0 0 1 JAN *", false, Ok),
            ("0 0 * jan-dec *", false, Ok),
            ("0-59/15 * * * *", false, Ok),
            ("0,30 * * * *", false, Ok),
            ("0 0 1-5 * *", false, Ok),
            ("0 0 * * 1,3,5", false, Ok),
            ("0 0 L * 1", false, Ok),
            ("0-0 * * * *", false, Ok),
            ("0 0 * * SAT-SUN", false, Ok),
            // --- fix round 1, C1: names, which the first draft REFUSED ---
            ("0 0 * * WED", false, Ok),
            ("0 0 * * wed", false, Ok),
            ("0 0 * * Wed", false, Ok),
            ("0 0 * * MON,WED,FRI", false, Ok),
            ("0 0 * * MON-WED", false, Ok),
            ("0 0 * * THU-SAT", false, Ok),
            ("0 0 * JUL *", false, Ok),
            ("0 0 * jul *", false, Ok),
            ("0 0 * JAN-JUL *", false, Ok),
            ("0 0 * JAN,JUL *", false, Ok),
            ("0 0 * NOV *", false, Ok),
            ("0 0 * DEC *", false, Ok),
            ("0 0 * MAR-MAY *", false, Ok),
            // --- fix round 1, I2: both ends of the `L-n` range ---
            ("0 0 L-1 * *", false, Ok),
            ("0 0 L-30 * *", false, Ok),
            ("0 0 L-31 * *", false, Ok),
            ("0 0 L-01 * *", false, Ok),
            ("0 0 L-0 * *", false, OutOfRange),
            ("0 0 L-32 * *", false, OutOfRange),
            ("0 0 L-100 * *", false, OutOfRange),
            // A name in a field that has none, and a long name: both refused.
            ("0 0 JUL * *", false, Unsupported),
            ("0 0 WED * *", false, Unsupported),
            ("0 0 * * MONDAY", false, Unsupported),
            ("0 0 * * 1W", false, Unsupported),
            ("0 0 * * FRI#2", false, Unsupported),
            ("0 0 * * 1-7", false, Ok),
            (" 0 2 * * * ", false, Ok),
            ("0  2 * * *", false, Ok),
            ("0\t2 * * *", false, Ok),
            ("0 0 0 * * *", true, Ok),
            ("*/5 * * * * *", true, Ok),
            // A nickname parses under EITHER parser -- the expansion happens
            // before the field count is checked.
            ("@daily", true, Ok),
            ("@hourly", true, Ok),
            // --- refused, measured ---
            ("@reboot", false, Nickname),
            ("@every 5m", false, FieldCount(2, 5)),
            ("@daily extra", false, FieldCount(2, 5)),
            ("@DAILY", false, FieldCount(1, 5)),
            ("@Daily", false, FieldCount(1, 5)),
            ("@YEARLY", false, FieldCount(1, 5)),
            ("@secondly", false, FieldCount(1, 5)),
            ("@minutely", false, FieldCount(1, 5)),
            ("@every", false, FieldCount(1, 5)),
            ("@nope", false, FieldCount(1, 5)),
            ("@", false, FieldCount(1, 5)),
            ("0 0 * * 8", false, OutOfRange),
            ("0 0 * * 5L", false, Unsupported),
            ("0 0 * * L", false, Unsupported),
            ("0 0 * * 1#1", false, Unsupported),
            ("0 0 ? * 1", false, Unsupported),
            ("0 0 1 * ?", false, Unsupported),
            ("0 0 15W * *", false, Unsupported),
            ("0 0 LW * *", false, Unsupported),
            ("5-1 * * * *", false, BadRange),
            ("0 0 * * 6-0", false, BadRange),
            ("0 0 * * 7-1", false, BadRange),
            ("0 0 * * SUN-SAT", false, BadRange),
            ("0 0 0 * * *", false, FieldCount(6, 5)),
            ("* * * * * *", false, FieldCount(6, 5)),
            ("* * * *", false, FieldCount(4, 5)),
            ("0 0 * * *", true, FieldCount(5, 6)),
            ("0 0 * * 7", true, FieldCount(5, 6)),
            ("60 * * * *", false, OutOfRange),
            ("0 24 * * *", false, OutOfRange),
            ("0 0 32 * *", false, OutOfRange),
            ("0 0 * 13 *", false, OutOfRange),
            ("*/0 * * * *", false, StepOutOfRange),
            ("", false, Empty),
            ("H 0 * * *", false, Unsupported),
            // --- structural faults, all refused (probe 10). NOTE the last
            // two: they answer `Invalid chars`, the SAME class the
            // `Unsupported` rows above get. An earlier header here claimed
            // every row in this group got a different class, while quoting the
            // counter-examples three lines below it. egret's variant is its
            // own; only the refusal is measured. ---
            ("*/x * * * *", false, Malformed), // `For input string: "X"`
            ("5/ * * * *", false, Malformed),  // `Missing steps for expression: 5/`
            ("0,,2 * * * *", false, Malformed), // `Invalid chars ... Expression: `
            ("0 0 L-x * *", false, Malformed), // `Invalid chars ... Expression: X`
            ("0 0 * * ,", false, Malformed),   // `Trailing commas not permitted!`
        ];

        for (expr, with_seconds, want) in table {
            let got = Cron::parse(expr, *with_seconds);
            let label = format!("{expr:?} (withSeconds: {with_seconds})");
            match (want, &got) {
                (Ok, Result::Ok(_)) => {}
                (Nickname, Err(CronError::UnsupportedNickname(_))) => {}
                (FieldCount(g, w), Err(CronError::WrongFieldCount { got: gg, want: ww })) => {
                    assert_eq!((gg, ww), (g, w), "{label}: wrong field-count numbers");
                }
                (Unsupported, Err(CronError::UnsupportedSyntax { .. })) => {}
                (OutOfRange, Err(CronError::OutOfRange { .. })) => {}
                (BadRange, Err(CronError::InvalidRange { .. })) => {}
                (StepOutOfRange, Err(CronError::StepOutOfRange { .. })) => {}
                (Empty, Err(CronError::Empty)) => {}
                (Malformed, Err(CronError::Malformed { .. })) => {}
                _ => panic!("{label}: expected {want:?}, got {got:?}"),
            }
        }
    }

    /// `H 0 * * *` is Jenkins' hashed-minute syntax -- pinned separately so a
    /// parser that started accepting it (and firing at an arbitrary minute)
    /// cannot hide inside the big table's `_ =>` arm.
    ///
    /// It is `UnsupportedSyntax`, not `Malformed`, and fix round 1 changed
    /// that: the artifact makes the *same* complaint about `H` that it makes
    /// about `5L` **in day-of-week** and `MONDAY` -- `Invalid chars in
    /// expression! Expression: H
    /// Invalid chars: H` -- so splitting them apart was a distinction this
    /// parser invented.
    #[test]
    fn jenkins_hashed_syntax_is_not_this_dialect() {
        assert!(matches!(
            Cron::parse("H 0 * * *", false),
            Err(CronError::UnsupportedSyntax {
                field: Field::Minute,
                ..
            })
        ));
    }

    // ---------------------------------------------------------------------
    // 2. next / last
    // ---------------------------------------------------------------------

    /// The artifact's own `nextExecution` / `lastExecution` answers, verbatim
    /// from `probe-out.json` and `probe2-out.json`.
    ///
    /// Both primitives are **strict**: `0 2 * * *` asked from exactly
    /// `02:00` answers the *next* day forward and the *previous* day back.
    #[test]
    fn next_and_last_match_the_artifact() {
        /// (cron, with_seconds, zone, from, expected next, expected last).
        ///
        /// Named because clippy calls the bare tuple complex, and it is right:
        /// six positional fields is exactly where a row gets transposed.
        type Row = (
            &'static str,
            bool,
            &'static str,
            &'static str,
            Option<&'static str>,
            Option<&'static str>,
        );

        let table: &[Row] = &[
            (
                "0 2 * * *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-16T02:00:00Z"),
                Some("2026-06-15T02:00:00Z"),
            ),
            (
                "0 2 * * *",
                false,
                "UTC",
                "2026-06-15T02:00:00Z",
                Some("2026-06-16T02:00:00Z"),
                Some("2026-06-14T02:00:00Z"),
            ),
            (
                "0 2 * * *",
                false,
                "UTC",
                "2026-06-15T01:59:59Z",
                Some("2026-06-15T02:00:00Z"),
                Some("2026-06-14T02:00:00Z"),
            ),
            // The zone decides the calendar the expression is read in.
            (
                "0 2 * * *",
                false,
                "Asia/Tokyo",
                "2026-06-15T12:00:00Z",
                Some("2026-06-15T17:00:00Z"),
                Some("2026-06-14T17:00:00Z"),
            ),
            (
                "0 2 * * *",
                false,
                "America/New_York",
                "2026-06-15T12:00:00Z",
                Some("2026-06-16T06:00:00Z"),
                Some("2026-06-15T06:00:00Z"),
            ),
            (
                "*/15 * * * *",
                false,
                "UTC",
                "2026-06-15T12:07:30Z",
                Some("2026-06-15T12:15:00Z"),
                Some("2026-06-15T12:00:00Z"),
            ),
            (
                "*/15 * * * *",
                false,
                "UTC",
                "2026-06-15T12:15:00Z",
                Some("2026-06-15T12:30:00Z"),
                Some("2026-06-15T12:00:00Z"),
            ),
            (
                "@daily",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-16T00:00:00Z"),
                Some("2026-06-15T00:00:00Z"),
            ),
            (
                "@hourly",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-15T13:00:00Z"),
                Some("2026-06-15T11:00:00Z"),
            ),
            (
                "@weekly",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-21T00:00:00Z"),
                Some("2026-06-14T00:00:00Z"),
            ),
            (
                "@monthly",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-07-01T00:00:00Z"),
                Some("2026-06-01T00:00:00Z"),
            ),
            (
                "@yearly",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2027-01-01T00:00:00Z"),
                Some("2026-01-01T00:00:00Z"),
            ),
            // 0 and 7 are both Sunday.
            (
                "0 0 * * 7",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-21T00:00:00Z"),
                Some("2026-06-14T00:00:00Z"),
            ),
            (
                "0 0 * * 0",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-21T00:00:00Z"),
                Some("2026-06-14T00:00:00Z"),
            ),
            // `L`, in a short month and in a leap February.
            (
                "0 0 L * *",
                false,
                "UTC",
                "2026-02-10T00:00:00Z",
                Some("2026-02-28T00:00:00Z"),
                Some("2026-01-31T00:00:00Z"),
            ),
            (
                "0 0 L * *",
                false,
                "UTC",
                "2028-02-10T00:00:00Z",
                Some("2028-02-29T00:00:00Z"),
                Some("2028-01-31T00:00:00Z"),
            ),
            (
                "0 0 L-3 * *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-27T00:00:00Z"),
                Some("2026-05-28T00:00:00Z"),
            ),
            // A four-year gap, forwards and backwards.
            (
                "0 0 29 2 *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2028-02-29T00:00:00Z"),
                Some("2024-02-29T00:00:00Z"),
            ),
            // Day-of-month OR day-of-week, and what counts as restricted.
            (
                "0 0 1 * 1",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-22T00:00:00Z"),
                Some("2026-06-15T00:00:00Z"),
            ),
            (
                "0 0 */2 * 1",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-17T00:00:00Z"),
                Some("2026-06-15T00:00:00Z"),
            ),
            (
                "0 0 1-31 * 1",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-16T00:00:00Z"),
                Some("2026-06-15T00:00:00Z"),
            ),
            (
                "0 0 * * 1",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-22T00:00:00Z"),
                Some("2026-06-15T00:00:00Z"),
            ),
            (
                "0 0 1 * *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-07-01T00:00:00Z"),
                Some("2026-06-01T00:00:00Z"),
            ),
            (
                "0 0 L * 1",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-22T00:00:00Z"),
                Some("2026-06-15T00:00:00Z"),
            ),
            // Steps with a non-zero base, and a step that does not divide the
            // range.
            (
                "*/7 * * * *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-15T12:07:00Z"),
                Some("2026-06-15T11:56:00Z"),
            ),
            (
                "5/10 * * * *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-15T12:05:00Z"),
                Some("2026-06-15T11:55:00Z"),
            ),
            // Names, and a range whose end is SUN = 7.
            (
                "0 0 * * SUN",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-21T00:00:00Z"),
                Some("2026-06-14T00:00:00Z"),
            ),
            (
                "0 0 * * SAT-SUN",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-20T00:00:00Z"),
                Some("2026-06-14T00:00:00Z"),
            ),
            (
                "0 0 * * 1-5",
                false,
                "UTC",
                "2026-06-13T12:00:00Z",
                Some("2026-06-15T00:00:00Z"),
                Some("2026-06-12T00:00:00Z"),
            ),
            // Seconds precision.
            (
                "*/5 * * * * *",
                true,
                "UTC",
                "2026-06-15T12:00:02Z",
                Some("2026-06-15T12:00:05Z"),
                Some("2026-06-15T12:00:00Z"),
            ),
            // --- fix round 1, C1: the names the first draft could not parse ---
            (
                "0 0 * * WED",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-17T00:00:00Z"),
                Some("2026-06-10T00:00:00Z"),
            ),
            (
                "0 0 * * MON-WED",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-16T00:00:00Z"),
                Some("2026-06-15T00:00:00Z"),
            ),
            (
                "0 0 * * MON,WED,FRI",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-17T00:00:00Z"),
                Some("2026-06-15T00:00:00Z"),
            ),
            (
                "0 0 * JUL *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-07-01T00:00:00Z"),
                Some("2025-07-31T00:00:00Z"),
            ),
            (
                "0 0 * JAN-JUL *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-16T00:00:00Z"),
                Some("2026-06-15T00:00:00Z"),
            ),
            // --- fix round 1, I2: `L-31` parses and then never matches ---
            (
                "0 0 L-31 * *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                None,
                None,
            ),
            (
                "0 0 L-30 * *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-07-01T00:00:00Z"),
                // May has 31 days, so `L-30` is the 1st of May; June has 30,
                // so June has no `L-30` at all. I GUESSED 2026-06-01 here and
                // the artifact said 2026-05-01 -- probe 9 was written because
                // of this line.
                Some("2026-05-01T00:00:00Z"),
            ),
            (
                "0 0 L-1 * *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-29T00:00:00Z"),
                Some("2026-05-30T00:00:00Z"),
            ),
            (
                "0 0 * * THU-SAT",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2026-06-18T00:00:00Z"),
                Some("2026-06-13T00:00:00Z"),
            ),
            (
                "0 0 * MAR-MAY *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                Some("2027-03-01T00:00:00Z"),
                Some("2026-05-31T00:00:00Z"),
            ),
            // An expression that can never match answers nothing, in BOTH
            // directions -- the artifact's `Optional.empty()`.
            (
                "0 0 30 2 *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                None,
                None,
            ),
            (
                "0 0 31 4 *",
                false,
                "UTC",
                "2026-06-15T12:00:00Z",
                None,
                None,
            ),
        ];

        for (expr, with_seconds, zone, from, want_next, want_last) in table {
            let cron = Cron::parse(expr, *with_seconds)
                .unwrap_or_else(|e| panic!("{expr:?} did not parse: {e}"));
            let zone = tz(zone);
            let from_dt = at(from).with_timezone(&zone);
            let label = format!("{expr:?} in {zone} from {from}");

            let got_next = cron.next_after(from_dt).map(|d| d.with_timezone(&Utc));
            assert_eq!(
                got_next,
                want_next.map(at),
                "{label}: next_after disagrees with the artifact"
            );

            let got_last = cron.last_before(from_dt).map(|d| d.with_timezone(&Utc));
            assert_eq!(
                got_last,
                want_last.map(at),
                "{label}: last_before disagrees with the artifact"
            );
        }
    }

    // ---------------------------------------------------------------------
    // 3. DST — the two readings a UTC-only fixture cannot make
    // ---------------------------------------------------------------------

    /// Across a **spring-forward gap** the missing local ticks are skipped
    /// outright, not shifted to the next existing minute.
    ///
    /// `Europe/Paris` skips 02:00–02:59 local on 2026-03-29, so a `*/30`
    /// schedule walks `01:30+01:00` straight to `03:00+02:00`, and a daily
    /// `0 2 * * *` misses the 29th entirely.
    #[test]
    fn a_spring_forward_gap_skips_its_ticks_rather_than_shifting_them() {
        let paris = tz("Europe/Paris");

        let every_30 = Cron::parse("*/30 * * * *", false).unwrap();
        let walk = walk_forward(
            &every_30,
            at("2026-03-29T00:00:00Z").with_timezone(&paris),
            3,
        );
        assert_eq!(
            walk,
            vec![
                at("2026-03-29T00:30:00Z"), // 01:30+01:00
                at("2026-03-29T01:00:00Z"), // 03:00+02:00 -- 02:00 and 02:30 never happen
                at("2026-03-29T01:30:00Z"), // 03:30+02:00
            ],
            "the gap's local ticks must not fire at all"
        );

        let daily = Cron::parse("0 2 * * *", false).unwrap();
        let walk = walk_forward(&daily, at("2026-03-27T01:00:00Z").with_timezone(&paris), 3);
        assert_eq!(
            walk,
            vec![
                at("2026-03-28T01:00:00Z"), // 02:00+01:00
                at("2026-03-30T00:00:00Z"), // 02:00+02:00 -- the 29th is MISSING
                at("2026-03-31T00:00:00Z"),
            ],
            "a daily 02:00 schedule must lose the day whose 02:00 does not exist"
        );
    }

    /// Across a **fall-back** the repeated local hour fires **twice**, and the
    /// order is by instant rather than by local clock reading.
    ///
    /// This is the reading that decides the implementation: local-clock order
    /// would give `02:00+02:00, 02:00+01:00, 02:30+02:00, 02:30+01:00`, and
    /// the artifact gives `02:00+02:00, 02:30+02:00, 02:00+01:00,
    /// 02:30+01:00`. A fixture that never crossed a fall-back hour cannot tell
    /// the two apart.
    #[test]
    fn a_fall_back_hour_fires_twice_in_instant_order() {
        let paris = tz("Europe/Paris");
        let every_30 = Cron::parse("*/30 * * * *", false).unwrap();

        let walk = walk_forward(
            &every_30,
            at("2026-10-24T22:30:00Z").with_timezone(&paris), // 00:30+02:00
            8,
        );
        assert_eq!(
            walk,
            vec![
                at("2026-10-24T23:00:00Z"), // 01:00+02:00
                at("2026-10-24T23:30:00Z"), // 01:30+02:00
                at("2026-10-25T00:00:00Z"), // 02:00+02:00   <- first pass
                at("2026-10-25T00:30:00Z"), // 02:30+02:00
                at("2026-10-25T01:00:00Z"), // 02:00+01:00   <- second pass
                at("2026-10-25T01:30:00Z"), // 02:30+01:00
                at("2026-10-25T02:00:00Z"), // 03:00+01:00
                at("2026-10-25T02:30:00Z"), // 03:30+01:00
            ],
            "the repeated hour must fire twice, ordered by instant"
        );

        // And backwards through the same hour.
        let back = walk_backward(
            &every_30,
            at("2026-10-25T02:00:00Z").with_timezone(&paris),
            4,
        );
        assert_eq!(
            back,
            vec![
                at("2026-10-25T01:30:00Z"),
                at("2026-10-25T01:00:00Z"),
                at("2026-10-25T00:30:00Z"),
                at("2026-10-25T00:00:00Z"),
            ],
            "last_before must walk the repeated hour in reverse instant order"
        );
    }

    fn walk_forward(cron: &Cron, from: DateTime<Tz>, steps: usize) -> Vec<DateTime<Utc>> {
        let mut out = Vec::new();
        let mut cur = from;
        for _ in 0..steps {
            match cron.next_after(cur) {
                Some(next) => {
                    out.push(next.with_timezone(&Utc));
                    cur = next;
                }
                None => break,
            }
        }
        out
    }

    fn walk_backward(cron: &Cron, from: DateTime<Tz>, steps: usize) -> Vec<DateTime<Utc>> {
        let mut out = Vec::new();
        let mut cur = from;
        for _ in 0..steps {
            match cron.last_before(cur) {
                Some(prev) => {
                    out.push(prev.with_timezone(&Utc));
                    cur = prev;
                }
                None => break,
            }
        }
        out
    }

    // ---------------------------------------------------------------------
    // 4. scheduleDates — the minus-one-second
    // ---------------------------------------------------------------------

    /// `Schedule.scheduleDates` asks for the next execution after
    /// `date.minus(1s)`, so a tick evaluated on **its own instant selects
    /// itself** rather than the following one.
    ///
    /// Without the `-1s` every schedule would fire one period late for ever,
    /// and no other test in this file would notice: `next_after` alone is
    /// strictly-after and correct.
    #[test]
    fn schedule_dates_selects_the_tick_it_stands_on() {
        // (cron, from, date, next, previous) -- probe-out.json's sd_* columns.
        let table: &[(&str, &str, &str, &str, &str)] = &[
            (
                "@hourly",
                "2026-06-15T12:00:00Z",
                "2026-06-15T12:00:00Z", // itself, NOT 13:00
                "2026-06-15T13:00:00Z",
                "2026-06-15T11:00:00Z",
            ),
            (
                "*/15 * * * *",
                "2026-06-15T12:15:00Z",
                "2026-06-15T12:15:00Z", // itself
                "2026-06-15T12:30:00Z",
                "2026-06-15T12:00:00Z",
            ),
            (
                "*/15 * * * *",
                "2026-06-15T12:07:30Z",
                "2026-06-15T12:15:00Z", // between ticks: the NEXT one
                "2026-06-15T12:30:00Z",
                "2026-06-15T12:00:00Z",
            ),
            (
                "0 2 * * *",
                "2026-06-15T12:00:00Z",
                "2026-06-16T02:00:00Z",
                "2026-06-17T02:00:00Z",
                "2026-06-15T02:00:00Z",
            ),
        ];

        for (expr, from, date, next, previous) in table {
            let trigger = sched(expr, false, Some("UTC"));
            let got = trigger
                .schedule_dates(at(from))
                .unwrap_or_else(|| panic!("{expr:?} from {from}: no dates at all"));
            assert_eq!(got.date, at(date), "{expr:?} from {from}: date");
            assert_eq!(got.next, Some(at(next)), "{expr:?} from {from}: next");
            assert_eq!(
                got.previous,
                Some(at(previous)),
                "{expr:?} from {from}: previous"
            );
        }
    }

    // ---------------------------------------------------------------------
    // 5. The late / missed-tick rule
    // ---------------------------------------------------------------------

    /// `probe-out.json`'s `next_evaluation_date` block, verbatim.
    ///
    /// The two rows that carry the whole rule are the third and fourth: five
    /// days behind, a schedule with **no** `lateMaximumDelay` answers the next
    /// **missed** tick (`06-11`), and the same schedule **with** `PT1H` answers
    /// the next **live** tick (`06-16`). A test that only had one of them could
    /// not tell "replays the backlog" from "drops it".
    #[test]
    fn the_missed_tick_rule_matches_the_artifact() {
        let now = probe_clock();

        // (last date, late maximum delay, expected)
        let table: &[(Option<&str>, Option<i64>, &str)] = &[
            (None, None, "2026-06-16T02:00:00Z"),
            (Some("2026-06-15T02:00:00Z"), None, "2026-06-16T02:00:00Z"),
            // five days behind, no delay: the NEXT MISSED tick
            (Some("2026-06-10T02:00:00Z"), None, "2026-06-11T02:00:00Z"),
            // five days behind, PT1H: the backlog is dropped
            (
                Some("2026-06-10T02:00:00Z"),
                Some(3600),
                "2026-06-16T02:00:00Z",
            ),
            (
                Some("2026-06-15T02:00:00Z"),
                Some(3600),
                "2026-06-16T02:00:00Z",
            ),
        ];

        for (last, delay, want) in table {
            let trigger = sched("0 2 * * *", false, Some("UTC"))
                .with_late_maximum_delay(delay.map(Duration::seconds));
            let got = trigger.next_evaluation_date(last.map(at), None, now);
            assert_eq!(
                got,
                Some(at(want)),
                "last={last:?} delay={delay:?}: disagrees with the artifact"
            );
        }
    }

    /// `handleMaxDelay` walks forward to the first tick still inside its delay,
    /// measured directly on the private method.
    #[test]
    fn late_maximum_delay_walks_to_the_first_tick_still_in_time() {
        let now = probe_clock();

        // `*/15` with PT10M from 10:00 stops at 12:00 -- the tick whose
        // date + 10m has NOT yet passed 12:00. One tick earlier (11:45) would
        // already be late.
        let trigger = sched("*/15 * * * *", false, Some("UTC"))
            .with_late_maximum_delay(Some(Duration::minutes(10)));
        assert_eq!(
            trigger.next_evaluation_date(Some(at("2026-06-15T10:00:00Z")), None, now),
            Some(at("2026-06-15T12:00:00Z"))
        );

        // And with no delay at all the same fixture replays 10:15 -- the pair
        // that shows the delay is doing the work, not the clock.
        let no_delay = sched("*/15 * * * *", false, Some("UTC"));
        assert_eq!(
            no_delay.next_evaluation_date(Some(at("2026-06-15T10:00:00Z")), None, now),
            Some(at("2026-06-15T10:15:00Z"))
        );
    }

    // ---------------------------------------------------------------------
    // 6. Backfill
    // ---------------------------------------------------------------------

    /// `probe-out.json`'s backfill rows: a running window computes from
    /// `currentDate`, and returns to the **live clock** the moment the answer
    /// passes `end`.
    #[test]
    fn a_backfill_window_ends_by_returning_to_the_live_clock() {
        let now = probe_clock();
        let trigger = sched("0 2 * * *", false, Some("UTC"));

        let window = |current: &str| super::Backfill {
            start: at("2026-06-01T00:00:00Z"),
            end: at("2026-06-05T00:00:00Z"),
            current_date: at(current),
            paused: false,
        };

        assert_eq!(
            trigger.next_evaluation_date(None, Some(&window("2026-06-01T00:00:00Z")), now),
            Some(at("2026-06-01T02:00:00Z")),
            "inside the window, the next tick after currentDate"
        );
        assert_eq!(
            trigger.next_evaluation_date(None, Some(&window("2026-06-04T02:00:00Z")), now),
            Some(at("2026-06-16T02:00:00Z")),
            "the 06-05T02:00 tick is after `end`, so the schedule returns to now"
        );
        assert_eq!(
            trigger.next_evaluation_date(None, Some(&window("2026-06-05T02:00:00Z")), now),
            Some(at("2026-06-16T02:00:00Z")),
            "a currentDate already past `end` is the same answer"
        );
    }

    /// `lateMaximumDelay` is skipped while a backfill is running — the
    /// `backfill == null` half of `nextEvaluationDate`'s guard.
    ///
    /// This is a **two-sided** pair on purpose. The `with` row alone would pass
    /// against an implementation that ignored `lateMaximumDelay` entirely, and
    /// `the_missed_tick_rule_matches_the_artifact` alone would pass against one
    /// that applied it unconditionally. Only the two together pin the guard.
    #[test]
    fn a_running_backfill_ignores_late_maximum_delay() {
        let now = probe_clock();
        let window = super::Backfill {
            start: at("2026-06-01T00:00:00Z"),
            end: at("2026-06-05T00:00:00Z"),
            current_date: at("2026-06-01T00:00:00Z"),
            paused: false,
        };

        let without = sched("0 2 * * *", false, Some("UTC"));
        let with = without
            .clone()
            .with_late_maximum_delay(Some(Duration::hours(1)));

        assert_eq!(
            without.next_evaluation_date(None, Some(&window), now),
            Some(at("2026-06-01T02:00:00Z"))
        );
        assert_eq!(
            with.next_evaluation_date(None, Some(&window), now),
            Some(at("2026-06-01T02:00:00Z")),
            "a running backfill must not have its window skipped by lateMaximumDelay"
        );

        // The same delay DOES bite when no backfill is attached -- otherwise
        // the assertion above would hold for a build that dropped the field.
        assert_eq!(
            with.next_evaluation_date(Some(at("2026-06-01T02:00:00Z")), None, now),
            Some(at("2026-06-16T02:00:00Z")),
            "outside a backfill the same delay must skip the backlog"
        );
    }

    // ---------------------------------------------------------------------
    // 7. eval's four refusals
    // ---------------------------------------------------------------------

    /// Each of `eval`'s refusals, and for each one an adjacent case that
    /// **does** fire — a guard nobody has seen fail is a guard nobody has seen.
    #[test]
    fn eval_refuses_exactly_where_the_artifact_refuses() {
        let now = probe_clock();
        let trigger = sched("0 2 * * *", false, Some("UTC"));

        // Fires: the tick is in the past and there is no backfill.
        assert!(trigger.eval(at("2026-06-15T02:00:00Z"), None, now).is_ok());

        // Refuses: a tick more than one second in the future.
        assert_eq!(
            trigger.eval(at("2026-06-16T02:00:00Z"), None, now),
            Err(SkipReason::InTheFuture)
        );

        // The boundary is `now + 1s`, STRICTLY greater. A minute-granularity
        // cron cannot land on it, so a second-granularity one is used.
        let per_second = sched("* * * * * *", true, Some("UTC"));
        assert!(
            per_second
                .eval(now + Duration::seconds(1), None, now)
                .is_ok(),
            "a tick exactly one second ahead still fires"
        );
        assert_eq!(
            per_second.eval(now + Duration::seconds(2), None, now),
            Err(SkipReason::InTheFuture),
            "two seconds ahead does not"
        );

        // Refuses: a paused backfill.
        let paused = super::Backfill {
            start: at("2026-06-01T00:00:00Z"),
            end: at("2026-06-05T00:00:00Z"),
            current_date: at("2026-06-01T02:00:00Z"),
            paused: true,
        };
        assert_eq!(
            trigger.eval(at("2026-06-01T02:00:00Z"), Some(&paused), now),
            Err(SkipReason::BackfillPaused)
        );
        // ... and the same window unpaused fires.
        let running = super::Backfill {
            paused: false,
            ..paused.clone()
        };
        assert!(trigger
            .eval(at("2026-06-01T02:00:00Z"), Some(&running), now)
            .is_ok());

        // Refuses: a backfill whose currentDate has not reached its tick.
        //
        // This is the guard that makes `attach_backfill`'s immediate
        // recomputation load-bearing: a window left sitting on `start` loses
        // its first date.
        let not_reached = super::Backfill {
            current_date: at("2026-06-01T00:00:00Z"),
            ..running.clone()
        };
        assert_eq!(
            trigger.eval(at("2026-06-01T00:00:00Z"), Some(&not_reached), now),
            Err(SkipReason::BackfillNotReached)
        );

        // Refuses: an expression that selects no instant at all.
        let never = sched("0 0 30 2 *", false, Some("UTC"));
        assert_eq!(
            never.eval(at("2026-06-15T02:00:00Z"), None, now),
            Err(SkipReason::NoSuchInstant)
        );
    }

    // ---------------------------------------------------------------------
    // 8. Refusals at resolve time — over-acceptance must not ship
    // ---------------------------------------------------------------------

    /// A `when` expression filters candidate ticks in Kestra
    /// (`Schedule.findNextDateMatchingConditions`). Rendering it needs a
    /// `RunContext` this seam does not have, so a schedule that carried one and
    /// fired anyway would **fire where Kestra would not** — and the oracle
    /// cannot catch that. It is refused.
    ///
    /// The two-sided half: `when` absent, and the literal default `"true"`,
    /// must both resolve — `hasWhenCondition()` is
    /// `getWhen() != null && !"true".equals(getWhen())`, so a refusal on
    /// `"true"` would refuse every trigger the model gives a default to.
    #[test]
    fn a_when_condition_is_refused_but_the_default_true_is_not() {
        let with_when: Trigger = serde_norway::from_str(
            "id: t\ntype: core.schedule\ncron: \"0 2 * * *\"\nwhen: \"{{ trigger.date | date('E') == 'MON' }}\"\n",
        )
        .unwrap();
        assert!(matches!(
            ScheduleTrigger::resolve(&with_when),
            Err(ScheduleError::UnsupportedWhenCondition(_))
        ));

        let explicit_true: Trigger = serde_norway::from_str(
            "id: t\ntype: core.schedule\ncron: \"0 2 * * *\"\nwhen: \"true\"\n",
        )
        .unwrap();
        assert!(
            ScheduleTrigger::resolve(&explicit_true).is_ok(),
            "the literal default `true` is NOT a condition"
        );

        let absent: Trigger =
            serde_norway::from_str("id: t\ntype: core.schedule\ncron: \"0 2 * * *\"\n").unwrap();
        assert!(ScheduleTrigger::resolve(&absent).is_ok());
    }

    /// Which spellings leave a day field **unrestricted** — in both day
    /// fields, from two clocks, and with one fixture that cannot tell, which
    /// is the point of having four.
    ///
    /// The day-of-month/day-of-week rule is OR only when *both* are
    /// restricted, so OR-ing a spelling against a restricted partner field and
    /// watching whether it contributes is a direct read of its restrictedness.
    ///
    /// **Exactly two spellings are unrestricted, `*` and `*/1`**, and that is
    /// not a reading — it is the complement enumerated over every token this
    /// file names. Fixture 1 alone reports **24** of them as unrestricted;
    /// fixtures 2 and 3 each report **two**. That gap is the whole lesson.
    ///
    /// The rule is cron-utils' `"1".equals(stepToken)` — a `String.equals` on
    /// the step's **spelling** — and it is a property of the whole field.
    /// Fix round 2 wrote it as a property of the step's *value* ("a step of 1
    /// over the whole range is the same set as `*`, and cron-utils collapses
    /// it"), which is a true sentence about sets and a false rule: `*/01`,
    /// `*/001`, `*/0001` and `*/+1` select that same set and are **not**
    /// collapsed. Fix round 3 then varied the one thing the corrected fixture
    /// still held fixed — the number of comma terms — and `*,*`, `*,*/1`,
    /// `*/1,*` and `*/1,*/1` turned out to be `And`, hence restricted.
    ///
    /// **`*/30` and `*/31` are restricted** — and the first fixture below
    /// says 2026-06-22 for them, which is *also* what unrestricted would say,
    /// because `{1, 31}` has no member between 06-15 and the next Monday.
    /// A verdict read off that fixture alone would have been a coin flip
    /// dressed as a measurement, so the second fixture exists to separate them.
    ///
    /// One caveat the re-review raised and this comment owes: egret models
    /// "unrestricted" as the OR-versus-AND switch **only** and keeps the
    /// field's value set either way, while cron-utils' `Always` also widens
    /// the set to every day. The two models agree on every row here because
    /// `*` and `*/1` already hold every day; they would not agree if the flag
    /// were flipped on a narrow set, which is why mutation G3 fails at `*/30`
    /// with a date neither column names.
    #[test]
    fn only_star_and_star_slash_one_leave_a_field_unrestricted() {
        let next = |expr: &str, from: &str| -> String {
            let cron = Cron::parse(expr, false).unwrap_or_else(|e| panic!("{expr:?}: {e}"));
            cron.next_after(at(from).with_timezone(&tz("UTC")))
                .map(|d| d.with_timezone(&Utc).date_naive().to_string())
                .unwrap_or_else(|| "never".into())
        };

        // Fixture 1 — day-of-week = Monday, clock 2026-06-15T12:00Z (a Monday).
        // UNRESTRICTED answers 2026-06-22 (Mondays only); restricted answers
        // earlier, because the day-of-month set contributes a nearer day.
        for (dom, want) in [
            ("*", "2026-06-22"),   // unrestricted
            ("*/1", "2026-06-22"), // unrestricted -- THE FIX
            // Fix round 3: a step that MEANS one but is not SPELLED `1`. The
            // artifact's rule is `"1".equals(stepToken)`, so all four of these
            // are `Every`, hence restricted -- and fix round 2's
            // `step != 1` had them unrestricted.
            ("*/01", "2026-06-16"),
            ("*/001", "2026-06-16"),
            ("*/0001", "2026-06-16"),
            ("*/+1", "2026-06-16"),
            // Fix round 3: a comma list of nothing but unrestricted terms is
            // `And`, never `Always`. Every rule this file has had -- including
            // the one before `*/1` was ever noticed -- got these wrong.
            ("*,*", "2026-06-16"),
            ("*,*/1", "2026-06-16"),
            ("*/1,*", "2026-06-16"),
            ("*/1,*/1", "2026-06-16"),
            ("*,*,*", "2026-06-16"),
            ("*/2", "2026-06-17"),
            ("*/3", "2026-06-16"),
            ("1-31", "2026-06-16"),
            ("1-31/1", "2026-06-16"),
            ("1/1", "2026-06-16"),
            ("1-31/2", "2026-06-17"),
            ("1-30", "2026-06-16"),
            // These two agree with the unrestricted answer here WITHOUT being
            // unrestricted. Kept deliberately, as the negative control for
            // fixture 2.
            ("*/30", "2026-06-22"),
            ("*/31", "2026-06-22"),
        ] {
            let expr = format!("0 0 {dom} * 1");
            assert_eq!(next(&expr, "2026-06-15T12:00:00Z"), want, "{expr}");
        }

        // Fixture 2 — day-of-week = Sunday, clock 2026-06-29T12:00Z. Now the
        // next Sunday (07-05) is LATER than the next member of `{1, 31}`
        // (07-01), so restricted and unrestricted give different answers.
        for (dom, want) in [
            ("*", "2026-07-05"),    // unrestricted
            ("*/1", "2026-07-05"),  // unrestricted -- THE FIX, discriminated twice
            ("*/30", "2026-07-01"), // restricted after all
            ("*/31", "2026-07-01"), // restricted after all
            ("*/2", "2026-07-01"),
            ("1-31", "2026-06-30"),
            // Fix round 3: the four spellings fix round 2 broke, and the five
            // it never varied.
            ("*/01", "2026-06-30"),
            ("*/001", "2026-06-30"),
            ("*/0001", "2026-06-30"),
            ("*/+1", "2026-06-30"),
            ("*,*", "2026-06-30"),
            ("*,*/1", "2026-06-30"),
            ("*/1,*", "2026-06-30"),
            ("*/1,*/1", "2026-06-30"),
            ("*,*,*", "2026-06-30"),
        ] {
            let expr = format!("0 0 {dom} * 0");
            assert_eq!(next(&expr, "2026-06-29T12:00:00Z"), want, "{expr}");
        }

        // Fixture 3 -- the DAY-OF-WEEK field, which no fixture exercised at
        // all before fix round 3, against a restricted day-of-month of `1`.
        // UNRESTRICTED answers 2026-07-01 (the next 1st); restricted answers
        // earlier, because the day-of-week set then contributes.
        for (dow, want) in [
            ("*", "2026-07-01"),   // unrestricted
            ("*/1", "2026-07-01"), // unrestricted
            ("*/01", "2026-06-16"),
            ("*/001", "2026-06-16"),
            ("*/0001", "2026-06-16"),
            ("*/+1", "2026-06-16"),
            ("*/2", "2026-06-16"),
            ("1/1", "2026-06-16"),
            ("*,*", "2026-06-16"),
            ("*,*/1", "2026-06-16"),
            ("*/1,*", "2026-06-16"),
            ("*/1,*/1", "2026-06-16"),
            ("*,*,*", "2026-06-16"),
            // `*/7` is the narrow restricted set -- {0}, Sunday -- so this row
            // is not satisfiable by "restricted happens to mean every day".
            ("*/7", "2026-06-21"),
        ] {
            let expr = format!("0 0 1 * {dow}");
            assert_eq!(next(&expr, "2026-06-15T12:00:00Z"), want, "{expr}");
        }

        // Fixture 4 -- day-of-week again, from the second clock, so that the
        // unrestricted answer (07-01) and the every-day restricted answer
        // (06-30) are a different pair of dates from fixture 3's.
        for (dow, want) in [
            ("*", "2026-07-01"),
            ("*/1", "2026-07-01"),
            ("*/01", "2026-06-30"),
            ("*/+1", "2026-06-30"),
            ("*/2", "2026-06-30"),
            ("*,*", "2026-06-30"),
            ("*/1,*/1", "2026-06-30"),
        ] {
            let expr = format!("0 0 1 * {dow}");
            assert_eq!(next(&expr, "2026-06-29T12:00:00Z"), want, "{expr}");
        }
    }

    /// Padding makes a nickname stop being one — and leaves a five-field
    /// expression alone.
    ///
    /// `CronParser.parse` runs its nickname switch on the **raw** expression
    /// (`aload_1`, exact `String.equals`) and only afterwards splits the
    /// normalised one into fields. This crate trimmed first, so padded
    /// spellings of every enabled nickname were scheduled where Kestra refuses
    /// the flow outright. **15 paddings x 7 nicknames = 105 rows measured on
    /// the artifact, all 105 refused** with `contains 1 parts but we expect
    /// one of [5]`; **91 of them fired here** — the other 14 are the two
    /// U+001C paddings, which Rust's `trim` does not eat either, so those
    /// already agreed by accident.
    ///
    /// The pair is two-sided on purpose: the same paddings on `0 0 * * *` are
    /// **accepted**, so a fix that simply refused whitespace would fail here.
    #[test]
    fn padding_unmakes_a_nickname_but_not_a_five_field_expression() {
        let nicknames = [
            "@yearly",
            "@annually",
            "@monthly",
            "@weekly",
            "@daily",
            "@midnight",
            "@hourly",
        ];
        // Every character the artifact was measured on, in both positions:
        // the six it splits on, one it trims but never splits on, and one it
        // does neither to.
        let pads = [
            " ", "\t", "\n", "\r", "\u{0b}", "\u{0c}", "\u{1c}", "\u{a0}",
        ];

        for nick in nicknames {
            assert!(Cron::parse(nick, false).is_ok(), "{nick} bare");
            for pad in pads {
                for expr in [format!("{nick}{pad}"), format!("{pad}{nick}")] {
                    match Cron::parse(&expr, false) {
                        Err(CronError::WrongFieldCount { got: 1, want: 5 }) => {}
                        other => panic!("{expr:?}: expected 1-part field count, got {other:?}"),
                    }
                }
            }
        }

        // `@reboot` is recognised-but-not-enabled only when it is spelled
        // exactly; padded, it never reaches the switch at all.
        assert!(matches!(
            Cron::parse("@reboot", false),
            Err(CronError::UnsupportedNickname(_))
        ));
        assert!(matches!(
            Cron::parse("@reboot ", false),
            Err(CronError::WrongFieldCount { got: 1, want: 5 })
        ));
        // And the exact match is what makes the old whitespace guard needless.
        assert!(matches!(
            Cron::parse("@daily extra", false),
            Err(CronError::WrongFieldCount { got: 2, want: 5 })
        ));

        // The other side: five-field expressions tolerate the artifact's own
        // whitespace, including U+001C..=U+001F, which `String.trim()` removes
        // and Java's `\s` does not split on.
        let fires = |expr: &str| {
            let cron = Cron::parse(expr, false).unwrap_or_else(|e| panic!("{expr:?}: {e}"));
            cron.next_after(at("2026-06-15T12:00:00Z").with_timezone(&tz("UTC")))
                .map(|d| d.with_timezone(&Utc).date_naive().to_string())
                .unwrap_or_else(|| "never".into())
        };
        for expr in [
            "0 0 * * *",
            " 0 0 * * * ",
            "\t0 0 * * *\t",
            "0 0 * * *\n",
            "0  0  *  *  *",
            "0\t0\t*\t*\t*",
            "\u{1c}0 0 * * *",
            "0 0 * * *\u{1c}",
        ] {
            assert_eq!(fires(expr), "2026-06-16", "{expr:?}");
        }

        // U+00A0 is whitespace to Rust and **not** to the artifact, which is
        // why `normalise` cannot be `str::trim`/`split_whitespace`. Measured:
        // `Invalid chars` at the ends, "4 parts" in the middle.
        assert!(Cron::parse("0 0 * * *\u{a0}", false).is_err());
        assert!(Cron::parse("\u{a0}0 0 * * *", false).is_err());
        assert!(matches!(
            Cron::parse("0 0 *\u{a0}* *", false),
            Err(CronError::WrongFieldCount { got: 4, want: 5 })
        ));

        // `Empty expression!` is decided on the NORMALISED string, so a
        // U+001C-only expression is empty and a U+00A0-only one is one field.
        for blank in ["", " ", "  ", "\t", "\n", "\u{1c}"] {
            assert!(
                matches!(Cron::parse(blank, false), Err(CronError::Empty)),
                "{blank:?}"
            );
        }
        assert!(matches!(
            Cron::parse("\u{a0}", false),
            Err(CronError::WrongFieldCount { got: 1, want: 5 })
        ));
    }

    /// A step is range-checked against its **own field's** range.
    ///
    /// This crate checked only for zero, so `0 */30 * * *` — a plausible thing
    /// to write, and one Kestra refuses — was accepted and scheduled hourly.
    /// Twenty-two such expressions were found by putting every token this file
    /// names to the artifact rather than by reading (fix round 3).
    ///
    /// Each field appears as an accept/refuse **pair** straddling its own
    /// bound, so a rule that refused the wrong constant fails here.
    #[test]
    fn a_step_is_range_checked_against_its_own_fields_range() {
        for (expr, accepted) in [
            ("*/59 * * * *", true),
            ("*/60 * * * *", false),
            ("0 */23 * * *", true),
            ("0 */24 * * *", false),
            ("0 0 */31 * *", true),
            ("0 0 */32 * *", false),
            ("0 0 * */12 *", true),
            ("0 0 * */13 *", false),
            ("0 0 * * */7", true),
            ("0 0 * * */8", false),
            // A step on a value, not on `*`, is checked the same way.
            ("0 0 5/31 * *", true),
            ("0 0 5/32 * *", false),
            ("0 0 * * 1/7", true),
            ("0 0 * * 1/8", false),
            ("0 0 * * 5/10", false),
            // Zero is refused in all five fields, including minutes, hours
            // and day-of-week, whose message prints a range containing it.
            ("*/0 * * * *", false),
            ("0 0 * */0 *", false),
            ("0 0 * * */0", false),
        ] {
            let got = Cron::parse(expr, false);
            assert_eq!(got.is_ok(), accepted, "{expr}: {got:?}");
            if !accepted {
                assert!(
                    matches!(got, Err(CronError::StepOutOfRange { .. })),
                    "{expr}: wrong refusal {got:?}"
                );
            }
        }
    }

    /// `nL` in **day-of-month** means `L` alone and a plain day in a list.
    ///
    /// The file called `5L` unsupported syntax, which is true of day-of-week
    /// and false of day-of-month — the artifact accepts `0 0 5L * *` and fires
    /// it on the last day of the month, the number range-checked to 1..=31 and
    /// then discarded.
    ///
    /// **The two halves are opposites, and the second one is here because the
    /// first version of this test could not tell it from its negation.** In a
    /// comma list the artifact keeps the NUMBER and drops the last-day
    /// meaning. Fix round 3 asserted the reverse and pinned it with
    /// `0 0 5L,1 * *` from 06-15, which answers 2026-07-01 under both rules;
    /// a reviewer implemented the artifact's rule as a mutation and this
    /// suite stayed green. The list rows below name the competing rule each
    /// one refutes — that is the only thing that makes them rows rather than
    /// decoration, and it is the seventh instance in this task of a fixture
    /// answering the question it was built for and staying silent about the
    /// one that was then generalised from it.
    #[test]
    fn an_n_l_day_of_month_term_is_l_alone_and_a_plain_day_in_a_list() {
        let next_from = |expr: &str, from: &str| -> String {
            let cron = Cron::parse(expr, false).unwrap_or_else(|e| panic!("{expr:?}: {e}"));
            cron.next_after(at(from).with_timezone(&tz("UTC")))
                .map(|d| d.with_timezone(&Utc).date_naive().to_string())
                .unwrap_or_else(|| "never".into())
        };
        let next = |expr: &str| next_from(expr, "2026-06-15T12:00:00Z");

        // The number is discarded, so every accepted spelling answers what a
        // bare `L` answers -- which is the control row.
        for expr in [
            "0 0 L * *",
            "0 0 5L * *",
            "0 0 1L * *",
            "0 0 15L * *",
            "0 0 31L * *",
            "0 0 5l * *",
        ] {
            assert_eq!(next(expr), "2026-06-30", "{expr}");
        }
        // It is NOT `L-n`: that one does use the number.
        assert_eq!(next("0 0 L-1 * *"), "2026-06-29");
        // Range-checked before being discarded.
        for expr in ["0 0 0L * *", "0 0 32L * *"] {
            assert!(
                matches!(Cron::parse(expr, false), Err(CronError::OutOfRange { .. })),
                "{expr}"
            );
        }
        // And still refused everywhere the artifact refuses it.
        for expr in ["0 0 L5 * *", "0 0 5LW * *", "0 0 5L-1 * *", "0 0 * * 5L"] {
            assert!(Cron::parse(expr, false).is_err(), "{expr}");
        }
        // IN A LIST THE NUMBER SURVIVES AND THE LAST-DAY MEANING IS DROPPED
        // -- the exact opposite of the sole-term rule above.
        //
        // Fix round 3 wrote "an `nL` term in a list is dropped like every
        // other `L` term" here, in the measured voice, and pinned it with
        // `5L,1` from 06-15 -> 2026-07-01. That row answers 07-01 under BOTH
        // rules, so it discriminated nothing: a reviewer implemented the
        // artifact's rule as a mutation and the whole `trigger::` suite
        // stayed green. Naming the competing rules FIRST and picking a row
        // that kills each is what this block is for -- the six rows below
        // were measured on the pinned artifact (fix round 4), and each is
        // annotated with the rule it refutes and what that rule would answer.
        //
        // "an `nL` in a list is dropped, like `L`"         -> 07-01 / never
        assert_eq!(next("0 0 20L,1 * *"), "2026-06-20");
        // the same rule, from a clock where 07-01 cannot be reached by
        // accident: this is the row `5L,1` could not be
        assert_eq!(
            next_from("0 0 5L,1 * *", "2026-06-02T12:00:00Z"),
            "2026-06-05"
        );
        // "the number AND the last day", and "a list `nL` still means `L`",
        // both of which would answer 2026-06-30 from this clock
        assert_eq!(
            next_from("0 0 20L,25L * *", "2026-06-26T12:00:00Z"),
            "2026-07-20"
        );
        // "every `L`-form in a list survives" -> 2026-06-30; a bare `L` in a
        // list is still dropped whole, so only the 5 remains
        assert_eq!(next("0 0 L,5L * *"), "2026-07-05");
        // "the number, clamped to the month's last day" -> 2026-06-30. It is
        // a PLAIN day: June has no 31st, so June is skipped entirely.
        assert_eq!(next("0 0 31L,1 * *"), "2026-07-01");
        // the sole-term control above ("0 0 5L * *" -> 2026-06-30) is the row
        // that kills "the number always wins", which would answer 2026-07-05.
        //
        // The range check still runs on the list form.
        for expr in ["0 0 0L,1 * *", "0 0 32L,1 * *"] {
            assert!(
                matches!(Cron::parse(expr, false), Err(CronError::OutOfRange { .. })),
                "{expr}"
            );
        }
        // And day-of-week is untouched by all of this, in a list too.
        for expr in ["0 0 * * 5L,1", "0 0 * * 1,5L"] {
            assert!(Cron::parse(expr, false).is_err(), "{expr}");
        }
    }

    /// An `L` term in a comma list is **dropped**, and this reproduces an
    /// upstream defect rather than tidying it away.
    ///
    /// Measured by walking five executions from 2026-06-01 on the artifact
    /// (probe 7). A union — the obvious implementation, and the one the first
    /// round shipped — fires on 06-30 for `L,1`, where the artifact does not:
    /// that is over-acceptance, and the oracle cannot catch it.
    ///
    /// The fixture deliberately varies what the first round held fixed: `L`
    /// alone, `L` first, `L` last, `L-n` in a list, and a list of nothing but
    /// `L` terms.
    #[test]
    fn an_l_term_in_a_comma_list_is_dropped_as_the_artifact_drops_it() {
        let walk_dom = |expr: &str| -> Vec<String> {
            let cron = Cron::parse(expr, false).unwrap_or_else(|e| panic!("{expr:?}: {e}"));
            let zone = tz("UTC");
            let mut out = Vec::new();
            let mut cur = at("2026-06-01T00:00:00Z").with_timezone(&zone);
            for _ in 0..5 {
                match cron.next_after(cur) {
                    Some(next) => {
                        out.push(next.with_timezone(&Utc).date_naive().to_string());
                        cur = next;
                    }
                    None => break,
                }
            }
            out
        };

        // Sole term: `L` means the last day, as it always has.
        assert_eq!(
            walk_dom("0 0 L * *"),
            [
                "2026-06-30",
                "2026-07-31",
                "2026-08-31",
                "2026-09-30",
                "2026-10-31"
            ],
        );

        // In a list, the `L` term vanishes -- in either position.
        for expr in ["0 0 L,1 * *", "0 0 1,L * *"] {
            assert_eq!(
                walk_dom(expr),
                [
                    "2026-07-01",
                    "2026-08-01",
                    "2026-09-01",
                    "2026-10-01",
                    "2026-11-01"
                ],
                "{expr}: the L term must be dropped, not unioned"
            );
        }
        for expr in [
            "0 0 L,15 * *",
            "0 0 15,L * *",
            "0 0 L-1,15 * *",
            "0 0 15,L-1 * *",
        ] {
            assert_eq!(
                walk_dom(expr),
                [
                    "2026-06-15",
                    "2026-07-15",
                    "2026-08-15",
                    "2026-09-15",
                    "2026-10-15"
                ],
                "{expr}: the L term must be dropped, not unioned"
            );
        }
        assert_eq!(
            walk_dom("0 0 2,L * *"),
            [
                "2026-06-02",
                "2026-07-02",
                "2026-08-02",
                "2026-09-02",
                "2026-10-02"
            ],
        );

        // And a list of NOTHING BUT `L` terms selects no day at all.
        assert!(
            walk_dom("0 0 L,L-1 * *").is_empty(),
            "`L,L-1` fires nothing on the artifact, in either direction"
        );
    }

    /// `lateMaximumDelay` changes which tick fires, this crate does not model
    /// the duration, and so a `Schedule` that declares it is **refused**.
    ///
    /// Before fix round 1 the key parsed, resolved, and was silently dropped:
    /// the model has no `deny_unknown_fields`, so a flow asking for the
    /// artifact's skip-the-backlog behaviour got the replay-it behaviour
    /// instead, and lost the key on re-serialisation too.
    #[test]
    fn a_schedule_declaring_late_maximum_delay_is_refused_not_silently_dropped() {
        let yaml = "id: t\ntype: core.schedule\ncron: \"0 2 * * *\"\nlateMaximumDelay: PT1H\n";
        let trigger: Trigger = serde_norway::from_str(yaml).unwrap();

        // It reaches the model rather than being discarded on the way in --
        // which is the half that makes the refusal possible at all.
        match &trigger.kind {
            crate::models::TriggerKind::Schedule(s) => assert!(
                s.late_maximum_delay.is_some(),
                "the key must survive parsing, or nothing downstream can see it"
            ),
            other => panic!("expected a Schedule, got {other:?}"),
        }

        assert!(
            matches!(
                ScheduleTrigger::resolve(&trigger),
                Err(ScheduleError::UnsupportedLateMaximumDelay(_))
            ),
            "a declared lateMaximumDelay must refuse, never schedule without it"
        );

        // Two-sided: the identical flow WITHOUT the key resolves.
        let without = "id: t\ntype: core.schedule\ncron: \"0 2 * * *\"\n";
        let trigger: Trigger = serde_norway::from_str(without).unwrap();
        assert!(ScheduleTrigger::resolve(&trigger).is_ok());
    }

    /// `Schedule.cron` is `@NotNull` in Java; W1 modelled it `Option` because
    /// bean validation is not parsing, and left the check owing. This is it.
    #[test]
    fn a_schedule_without_a_cron_is_refused() {
        let no_cron: Trigger = serde_norway::from_str("id: t\ntype: core.schedule\n").unwrap();
        assert_eq!(
            ScheduleTrigger::resolve(&no_cron),
            Err(ScheduleError::MissingCron)
        );
    }

    /// An absent `timezone` is UTC, because the pinned image's own
    /// `/etc/timezone` is `Etc/UTC` — not because UTC is a convenient default.
    ///
    /// Paired with an explicit zone so the assertion depends on the field: a
    /// build that ignored `timezone` altogether would pass the first half and
    /// fail the second.
    #[test]
    fn an_absent_timezone_is_utc_and_an_explicit_one_is_honoured() {
        assert_eq!(sched("0 2 * * *", false, None).timezone(), Tz::UTC);
        assert_eq!(
            sched("0 2 * * *", false, Some("Asia/Tokyo")).timezone(),
            tz("Asia/Tokyo")
        );

        let now = probe_clock();
        assert_eq!(
            sched("0 2 * * *", false, None).next_evaluation_date(None, None, now),
            Some(at("2026-06-16T02:00:00Z"))
        );
        assert_eq!(
            sched("0 2 * * *", false, Some("Asia/Tokyo")).next_evaluation_date(None, None, now),
            Some(at("2026-06-15T17:00:00Z")),
            "Tokyo's 02:00 is 17:00Z the previous day"
        );
    }

    /// `ZoneId.of` accepts fixed offsets that `chrono-tz` does not. Egret
    /// **refuses** rather than silently coercing to UTC — under-acceptance, in
    /// the safe direction, and recorded rather than hidden.
    #[test]
    fn a_zone_chrono_tz_cannot_resolve_is_refused_not_coerced() {
        for zone in ["GMT+02:00", "UTC+2", "Mars/Olympus"] {
            let yaml =
                format!("id: t\ntype: core.schedule\ncron: \"0 2 * * *\"\ntimezone: \"{zone}\"\n");
            let trigger: Trigger = serde_norway::from_str(&yaml).unwrap();
            assert_eq!(
                ScheduleTrigger::resolve(&trigger),
                Err(ScheduleError::UnknownTimezone(zone.to_string())),
                "{zone} must be refused, never coerced to UTC"
            );
        }
    }

    /// A non-`Schedule` trigger is not silently treated as one.
    #[test]
    fn a_webhook_is_not_a_schedule() {
        let webhook: Trigger =
            serde_norway::from_str("id: t\ntype: core.webhook\nkey: abc\n").unwrap();
        assert_eq!(
            ScheduleTrigger::resolve(&webhook),
            Err(ScheduleError::NotASchedule)
        );
    }
}
