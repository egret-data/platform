// kestra: core/src/main/java/io/kestra/plugin/core/trigger/ScheduleOnDates.java

//! `ScheduleOnDates` -- a Schedulable trigger over an explicit date list
//! (package 11e, Task 1).
//!
//! Measured at the pinned checkout (`ScheduleOnDates.java`, Task 0 report §1):
//!
//! - `nextDate` **sorts** the list, converts each date with
//!   `withZoneSameInstant` (an instant-preserving zone shift) and truncates
//!   the answer to whole seconds. This module truncates every date up front,
//!   so a sub-second date is the same tick as its whole second.
//! - The **fire** predicate is inclusive (`date.isEqual(last) ||
//!   date.isAfter(last)`) and the **init/advance** predicate is strict-after
//!   (`date.isAfter(dt)`). egret's `ScheduleTrigger::schedule_dates` already
//!   asks its tick source for the first tick strictly after `date - 1s`, which
//!   for whole-second dates is "at or after `date`": inclusive at fire. The
//!   advance asks strictly after the fired tick. So one strict-after
//!   [`next_after`] serves both, exactly as a cron does.
//!
//! # Deliberate divergence: an exhausted list parks off the queue
//!
//! When no date is left Kestra returns `now + 1 year`, keeping the trigger on
//! the queue. egret instead stores `next_evaluation_date = NULL` with a
//! `never_fires` reason -- the same as a cron with no future tick -- so the row
//! leaves `workflow_triggers_due`. Editing the dates re-initialises it
//! (`spec_key` changes).

use chrono::{DateTime, DurationRound, TimeDelta, Utc};
use chrono_tz::Tz;

/// Parse the authored `dates` (RFC 3339, as `ZonedDateTime` renders), truncate
/// each to whole seconds and sort ascending. The error names the offending
/// entry.
pub fn parse_dates(raw: &[String]) -> Result<Vec<DateTime<Utc>>, String> {
    let mut dates = raw
        .iter()
        .map(|s| {
            let d = DateTime::parse_from_rfc3339(s.trim())
                .map_err(|e| format!("date {s:?} is not an RFC 3339 timestamp: {e}"))?;
            d.with_timezone(&Utc)
                .duration_trunc(TimeDelta::seconds(1))
                .map_err(|e| format!("date {s:?} cannot be truncated: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    dates.sort();
    Ok(dates)
}

/// The first date **strictly after** `after`, shifted into `tz`, or `None`
/// when the list is exhausted. `dates` need not be sorted.
pub fn next_after(dates: &[DateTime<Utc>], tz: Tz, after: DateTime<Tz>) -> Option<DateTime<Tz>> {
    let after = after.with_timezone(&Utc);
    dates
        .iter()
        .filter(|d| **d > after)
        .min()
        .map(|d| d.with_timezone(&tz))
}

/// The last date strictly before `before`.
pub fn last_before(dates: &[DateTime<Utc>], tz: Tz, before: DateTime<Tz>) -> Option<DateTime<Tz>> {
    let before = before.with_timezone(&Utc);
    dates
        .iter()
        .filter(|d| **d < before)
        .max()
        .map(|d| d.with_timezone(&tz))
}

/// The dates `t` with `after < t <= until`, counted up to `cap`:
/// `(count, more)`, `more` meaning "there are more than `cap`".
pub fn count_through(
    dates: &[DateTime<Utc>],
    after: DateTime<Tz>,
    until: DateTime<Tz>,
    cap: u32,
) -> (u32, bool) {
    let (after, until) = (after.with_timezone(&Utc), until.with_timezone(&Utc));
    let n = dates.iter().filter(|d| **d > after && **d <= until).count();
    let cap = cap as usize;
    (n.min(cap) as u32, n > cap)
}

/// `dates|timezone` -- what decides WHEN, so an edit re-initialises the row.
/// The dates are the sorted, truncated instants, so reordering is not an edit.
pub(crate) fn spec_key_of(dates: &[DateTime<Utc>], timezone: Option<&str>) -> String {
    let list: Vec<String> = dates.iter().map(DateTime::to_rfc3339).collect();
    format!("dates:{}|{}", list.join(","), timezone.unwrap_or(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn dates(raw: &[&str]) -> Vec<DateTime<Utc>> {
        parse_dates(&raw.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    fn utc(s: &str) -> DateTime<Tz> {
        dt(s).with_timezone(&Tz::UTC)
    }

    #[test]
    fn next_after_returns_the_first_date_strictly_after() {
        let d = dates(&[
            "2026-02-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
            "2026-03-01T00:00:00Z",
        ]);
        assert_eq!(
            next_after(&d, Tz::UTC, utc("2026-01-01T00:00:00Z")),
            Some(utc("2026-02-01T00:00:00Z"))
        );
        assert_eq!(next_after(&d, Tz::UTC, utc("2026-03-01T00:00:00Z")), None);
    }

    /// An unsorted list the caller did not sort still answers the earliest.
    #[test]
    fn next_after_does_not_depend_on_the_order() {
        let unsorted = vec![dt("2026-03-01T00:00:00Z"), dt("2026-02-01T00:00:00Z")];
        assert_eq!(
            next_after(&unsorted, Tz::UTC, utc("2026-01-01T00:00:00Z")),
            Some(utc("2026-02-01T00:00:00Z"))
        );
    }

    #[test]
    fn parse_sorts_and_truncates_to_whole_seconds() {
        let d = dates(&["2026-02-01T00:00:00.900Z", "2026-01-01T00:00:00.100Z"]);
        assert_eq!(
            d,
            vec![dt("2026-01-01T00:00:00Z"), dt("2026-02-01T00:00:00Z")]
        );
    }

    /// `withZoneSameInstant`: the zone changes the rendering, not the instant.
    #[test]
    fn the_timezone_shifts_the_zone_not_the_instant() {
        let d = dates(&["2026-01-01T00:00:00+07:00"]);
        let tz: Tz = "Asia/Ho_Chi_Minh".parse().unwrap();
        let got = next_after(&d, tz, utc("2025-12-01T00:00:00Z")).unwrap();
        assert_eq!(got.with_timezone(&Utc), dt("2025-12-31T17:00:00Z"));
        assert_eq!(got.timezone(), tz);
    }

    #[test]
    fn a_malformed_date_is_named() {
        let err = parse_dates(&["not-a-date".to_string()]).unwrap_err();
        assert!(err.contains("not-a-date"), "{err}");
    }

    #[test]
    fn count_and_last_agree_with_the_list() {
        let d = dates(&[
            "2026-01-01T00:00:00Z",
            "2026-02-01T00:00:00Z",
            "2026-03-01T00:00:00Z",
        ]);
        let (a, b) = (utc("2026-01-01T00:00:00Z"), utc("2026-03-01T00:00:00Z"));
        assert_eq!(count_through(&d, a, b, 10), (2, false));
        assert_eq!(count_through(&d, a, b, 1), (1, true));
        assert_eq!(
            last_before(&d, Tz::UTC, b),
            Some(utc("2026-02-01T00:00:00Z"))
        );
    }

    #[test]
    fn the_spec_key_ignores_order_and_changes_with_the_dates() {
        let two = dates(&["2026-01-01T00:00:00Z", "2026-02-01T00:00:00Z"]);
        let swapped = dates(&["2026-02-01T00:00:00Z", "2026-01-01T00:00:00Z"]);
        let one = dates(&["2026-01-01T00:00:00Z"]);
        assert_eq!(spec_key_of(&two, None), spec_key_of(&swapped, None));
        assert_ne!(spec_key_of(&two, None), spec_key_of(&one, None));
        assert_ne!(spec_key_of(&two, None), spec_key_of(&two, Some("UTC")));
    }
}
