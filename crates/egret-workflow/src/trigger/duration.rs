// kestra: core/src/main/java/io/kestra/core/models/triggers/Window.java
// jdk: java.time.Duration#parse, java.time.format.DateTimeFormatter#ISO_LOCAL_TIME

//! The two `java.time` value types a flow trigger's `window` carries, parsed
//! as a **subset**: `Duration` (`every`, `offset`, `lookback`) and `LocalTime`
//! (`deadline`, `from`, `to`).
//!
//! Pure: no clock, no I/O. It is called twice -- by `spec::validate` at save,
//! so a value this module refuses never publishes, and by the window
//! computation at run time -- so the two can never disagree about what a
//! string means.
//!
//! # `Duration`: what is accepted, and why the rest is refused
//!
//! `java.time.Duration.parse` accepts `[-+]PnDTnHnMn.nS`, case-insensitively,
//! with a sign on every component. egret accepts the part the corpus and W13b
//! Task 1's measurements use -- `P`, then optional whole days, then an
//! optional `T` with optional whole hours, minutes and seconds, in that order,
//! upper case -- and **refuses everything else rather than guessing**
//! (over-acceptance is the hazard; spec §5.1):
//!
//! | input | answer | why |
//! |---|---|---|
//! | `PT10M`, `PT6H`, `P1D`, `PT1H30M`, `P1DT2H`, `PT90S` | accepted | the subset |
//! | `P1M`, `P1Y` (and `P1W`) | refused | months and years have no fixed length; `java.time.Duration` refuses them too, and weeks as well |
//! | `PT1.5S` | refused | fractional seconds: Java accepts them, nothing measured uses them |
//! | `-PT1H`, `PT-1H`, `+PT1H` | refused | a window cannot run backwards; Java accepts signs |
//! | `pt1h` | refused | Java is case-insensitive; the subset is not -- chosen |
//! | `P`, `PT`, `P1DT`, `PT1M1H`, `1H` | refused | not a duration at all |
//!
//! # The upper bound: [`MAX_WINDOW_DURATION`], 366 days
//!
//! A duration that merely fits `TimeDelta` is not one a window can use:
//! `every: P100000000D` added to `now` either overflows `DateTime` (a panic on
//! `+`) or lands past Postgres `timestamptz`'s upper bound (an `INSERT`
//! failure at run time, long after the flow saved). So a value longer than
//! [`MAX_WINDOW_DURATION`] is refused here, at save. See the constant for why
//! 366 days.
//!
//! Zero (`PT0S`) parses: whether a zero is meaningful depends on the field,
//! and that is the validator's call, not this parser's.
//!
//! # `LocalTime`
//!
//! `HH:mm` and `HH:mm:ss`, two digits each. W13b Task 1 only ever sent
//! `HH:mm:ss` (`"10:00:00"`, `"05:00:00"`, ...); `HH:mm` is accepted because
//! `ISO_LOCAL_TIME`, which Jackson's `LocalTimeDeserializer` uses, makes the
//! seconds optional -- derived from the JDK, not measured. Fractional seconds,
//! which `ISO_LOCAL_TIME` also allows, are refused: unmeasured, and a window
//! bound at sub-second precision is not something any flow here writes.

use chrono::{NaiveTime, TimeDelta};

/// Why a string is not a `Duration` egret accepts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DurationError {
    #[error(
        "`{0}`: months and years have no fixed length, so they cannot size a window; \
         write the length in days instead (`P30D`, not `P1M`)"
    )]
    NoFixedLength(String),
    #[error("`{0}`: weeks are not accepted by `java.time.Duration`; write days (`P7D`)")]
    Weeks(String),
    #[error(
        "`{0}`: fractional values are not supported; write a smaller unit (`PT90S`, not `PT1.5M`)"
    )]
    Fractional(String),
    #[error("`{0}`: a signed duration is not supported; a window cannot run backwards")]
    Signed(String),
    #[error("`{0}`: too large to represent")]
    Overflow(String),
    #[error(
        "`{0}` is longer than the longest window egret accepts, 366 days (`P366D`); \
         a window longer than a year is not supported"
    )]
    TooLong(String),
    #[error(
        "`{0}` is not an ISO-8601 duration of the form `P[nD][T[nH][nM][nS]]` \
         (for example `PT10M`, `PT6H`, `P1D`, `P1DT2H`)"
    )]
    Malformed(String),
}

/// Parse the ISO-8601 `Duration` subset described in the module doc.
pub fn parse_duration(s: &str) -> Result<TimeDelta, DurationError> {
    let malformed = || DurationError::Malformed(s.to_string());

    if s.starts_with('-') || s.starts_with('+') || s.contains("T-") || s.contains("T+") {
        return Err(DurationError::Signed(s.to_string()));
    }
    let Some(body) = s.strip_prefix('P') else {
        return Err(malformed());
    };
    if body.contains('-') || body.contains('+') {
        return Err(DurationError::Signed(s.to_string()));
    }
    if body.contains('.') || body.contains(',') {
        return Err(DurationError::Fractional(s.to_string()));
    }

    let (date_part, time_part) = match body.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (body, None),
    };

    // The date part: nothing, or `nD`. `Y`/`M` here are calendar units, and
    // they are named in the refusal rather than reported as malformed.
    let date = components_of(date_part).ok_or_else(malformed)?;
    match date.iter().find(|(_, u)| matches!(u, 'Y' | 'M' | 'W')) {
        Some((_, 'W')) => return Err(DurationError::Weeks(s.to_string())),
        Some(_) => return Err(DurationError::NoFixedLength(s.to_string())),
        None => {}
    }
    let mut components = 0usize;
    let mut total_secs = accumulate(date_part, &[('D', 86_400)], 0, &mut components, s)?;

    if let Some(time_part) = time_part {
        if time_part.is_empty() {
            return Err(malformed());
        }
        let time_units: &[(char, i64)] = &[('H', 3_600), ('M', 60), ('S', 1)];
        let before = components;
        total_secs = accumulate(time_part, time_units, total_secs, &mut components, s)?;
        if components == before {
            return Err(malformed());
        }
    }

    if components == 0 {
        return Err(malformed());
    }
    let d =
        TimeDelta::try_seconds(total_secs).ok_or_else(|| DurationError::Overflow(s.to_string()))?;
    if d > MAX_WINDOW_DURATION {
        return Err(DurationError::TooLong(s.to_string()));
    }
    Ok(d)
}

/// The longest `every`, `offset` or `lookback` egret accepts: **366 days**,
/// the longest calendar year. **Chosen**, and the reasons are these:
///
/// - **What a window bound can mean.** A window is the span in which every
///   `dependsOn` condition must be met. A `DURATION_WINDOW` of `every` >= 1
///   day starts at today's midnight in its zone (the measured rule, D3), so a
///   bound is `today + offset + every`; a `SLIDING_WINDOW` ends at the first
///   matching event + `lookback`. A year covers every calendar-shaped intent
///   ("within the year", a yearly `P365D`/`P366D` job), and a longer one can
///   no longer be told apart from "no window at all".
/// - **Kestra.** The measurements only ever used `every` up to `P1D`
///   (`w13b-windows.json`); `java.time.Duration` itself goes to ~292 billion
///   years and Kestra adds it to `ZonedDateTime` unchecked, so Kestra supplies
///   no bound to copy -- only the evidence that real windows are small.
/// - **Arithmetic.** At 366 days, `now + offset + every` (two maxima) stays
///   ~2 years from `now`: nowhere near `DateTime<Utc>`'s year 262143 or
///   `timestamptz`'s 294276 AD, so no bound computed from accepted values
///   can overflow either. The run-time computation (W13b Task 3) should still
///   use checked arithmetic: this cap is a save-time rule, and a row written
///   before it existed is not re-validated.
pub const MAX_WINDOW_DURATION: TimeDelta = TimeDelta::days(366);

/// Split `1D` / `1H30M` into `(digits, unit)` pairs, or `None` if the text is
/// not a run of `<digits><upper-case letter>` pairs.
fn components_of(part: &str) -> Option<Vec<(&str, char)>> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, c) in part.char_indices() {
        if c.is_ascii_digit() {
            continue;
        }
        if !c.is_ascii_uppercase() || i == start {
            return None;
        }
        out.push((&part[start..i], c));
        start = i + c.len_utf8();
    }
    if start != part.len() {
        return None; // trailing digits with no unit
    }
    Some(out)
}

/// Add `part`'s components to `total`, requiring each unit to be one of
/// `units`, in `units`' order, at most once.
fn accumulate(
    part: &str,
    units: &[(char, i64)],
    mut total: i64,
    components: &mut usize,
    whole: &str,
) -> Result<i64, DurationError> {
    let malformed = || DurationError::Malformed(whole.to_string());
    let overflow = || DurationError::Overflow(whole.to_string());
    let mut next_unit = 0usize;
    for (digits, unit) in components_of(part).ok_or_else(malformed)? {
        let pos = units[next_unit..]
            .iter()
            .position(|(u, _)| *u == unit)
            .ok_or_else(malformed)?;
        let (_, secs_per) = units[next_unit + pos];
        next_unit += pos + 1;
        let n: i64 = digits.parse().map_err(|_| overflow())?;
        total = n
            .checked_mul(secs_per)
            .and_then(|v| total.checked_add(v))
            .ok_or_else(overflow)?;
        *components += 1;
    }
    Ok(total)
}

/// Why a string is not a `LocalTime` egret accepts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not a time of day of the form `HH:mm` or `HH:mm:ss` (for example `09:00` or `17:30:00`)")]
pub struct LocalTimeError(pub String);

/// Parse `HH:mm` or `HH:mm:ss`, two digits per field.
pub fn parse_local_time(s: &str) -> Result<NaiveTime, LocalTimeError> {
    let err = || LocalTimeError(s.to_string());
    let fields: Vec<&str> = s.split(':').collect();
    if !(2..=3).contains(&fields.len())
        || fields
            .iter()
            .any(|f| f.len() != 2 || !f.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(err());
    }
    let n = |i: usize| fields.get(i).map_or(Ok(0), |f| f.parse::<u32>());
    let (h, m, sec) = (
        n(0).map_err(|_| err())?,
        n(1).map_err(|_| err())?,
        n(2).map_err(|_| err())?,
    );
    NaiveTime::from_hms_opt(h, m, sec).ok_or_else(err)
}

#[cfg(test)]
mod tests {
    use super::{parse_duration, parse_local_time, DurationError};
    use chrono::{NaiveTime, TimeDelta};

    #[test]
    fn the_subset_parses_to_the_right_length() {
        for (s, secs) in [
            ("PT10M", 600),
            ("PT6H", 6 * 3600),
            ("P1D", 86_400),
            ("PT1H30M", 5400),
            ("P1DT2H", 86_400 + 7200),
            ("PT90S", 90),
            ("PT45S", 45),
            ("PT90M", 5400),
            ("P1DT1H1M1S", 86_400 + 3661),
            ("PT0S", 0),
        ] {
            assert_eq!(
                parse_duration(s),
                Ok(TimeDelta::try_seconds(secs).unwrap()),
                "{s}"
            );
        }
    }

    #[test]
    fn months_and_years_are_refused_because_they_have_no_fixed_length() {
        for s in ["P1M", "P1Y", "P1Y2M", "P1MT1H"] {
            let e = parse_duration(s).unwrap_err();
            assert_eq!(e, DurationError::NoFixedLength(s.to_string()), "{s}");
            assert!(
                e.to_string().contains("no fixed length"),
                "the error must say why: {e}"
            );
        }
        // `M` AFTER the `T` is minutes, not months.
        assert_eq!(
            parse_duration("PT1M"),
            Ok(TimeDelta::try_minutes(1).unwrap())
        );
        assert!(matches!(
            parse_duration("P1W"),
            Err(DurationError::Weeks(_))
        ));
    }

    #[test]
    fn fractions_signs_and_garbage_are_refused() {
        for s in ["PT1.5S", "PT0,5S", "PT1.5M"] {
            assert!(
                matches!(parse_duration(s), Err(DurationError::Fractional(_))),
                "{s}"
            );
        }
        for s in ["-PT1H", "+PT1H", "PT-1H", "P-1D", "PT+1H"] {
            assert!(
                matches!(parse_duration(s), Err(DurationError::Signed(_))),
                "{s}"
            );
        }
        for s in [
            "", "P", "PT", "P1DT", "1H", "PT1H1H", "PT1M1H", "PT1S1M", "P1H", "PTH", "PT1", "P1",
            "pt1h", "PT1h", "garbage", "PT1HX", "PT 1H", "P1DT2D",
        ] {
            assert!(
                matches!(parse_duration(s), Err(DurationError::Malformed(_))),
                "{s:?} -> {:?}",
                parse_duration(s)
            );
        }
    }

    #[test]
    fn a_duration_too_large_to_represent_is_refused_not_wrapped() {
        for s in [
            "PT99999999999999999999S",
            "P9223372036854775807D",
            "P106751991167301D",
            // Fits an i64 of seconds, but not `TimeDelta` (i64 milliseconds).
            "P200000000000D",
        ] {
            assert!(
                matches!(parse_duration(s), Err(DurationError::Overflow(_))),
                "{s} -> {:?}",
                parse_duration(s)
            );
        }
    }

    /// The cap is inclusive: `P366D` (and the same length spelled in hours)
    /// is accepted, one second more is refused as `TooLong`, not `Overflow`.
    #[test]
    fn a_duration_longer_than_366_days_is_refused_at_the_cap_plus_one() {
        assert_eq!(parse_duration("P366D"), Ok(super::MAX_WINDOW_DURATION));
        assert_eq!(parse_duration("PT8784H"), Ok(super::MAX_WINDOW_DURATION));
        for s in ["P366DT1S", "PT8784H0M1S", "P367D", "P100000000D"] {
            let e = parse_duration(s).unwrap_err();
            assert_eq!(e, DurationError::TooLong(s.to_string()), "{s}");
            assert!(e.to_string().contains("366 days"), "{e}");
        }
    }

    #[test]
    fn local_times_parse_with_and_without_seconds() {
        assert_eq!(
            parse_local_time("09:00"),
            Ok(NaiveTime::from_hms_opt(9, 0, 0).unwrap())
        );
        assert_eq!(
            parse_local_time("17:30:15"),
            Ok(NaiveTime::from_hms_opt(17, 30, 15).unwrap())
        );
        assert_eq!(
            parse_local_time("00:00:00"),
            Ok(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
        );
        for s in [
            "",
            "9:00",
            "09",
            "24:00",
            "23:60",
            "23:59:60",
            "09:00:00.5",
            "09:00:00:00",
            "ab:cd",
            "09-00",
            " 09:00",
            "+9:00",
        ] {
            assert!(parse_local_time(s).is_err(), "{s:?}");
        }
    }
}
