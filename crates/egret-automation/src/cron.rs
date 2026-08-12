//! Crontab parsing and next-run computation.
//!
//! Split out from the repo because it is pure: no database, no clock of its own
//! (the caller passes `now`), so the interesting cases — an invalid expression,
//! an unknown timezone, a DST boundary — are unit-testable without a fixture.

use chrono::{DateTime, TimeZone, Utc};
use croner::Cron;
use std::str::FromStr;

/// Why a schedule's timing could not be interpreted. Carried as a message
/// because it is shown to the user in the editor, next to the field.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CronError {
    #[error("invalid crontab: {0}")]
    InvalidCrontab(String),
    #[error("unknown timezone: {0}")]
    UnknownTimezone(String),
    #[error("crontab has no next occurrence")]
    NoOccurrence,
}

/// Parse `crontab` in `timezone` and return the first firing strictly after
/// `after`, in UTC.
///
/// The expression is evaluated **in the schedule's own zone**, then converted,
/// which is the whole reason `timezone` is stored beside it: "every day at
/// 09:00 Asia/Ho_Chi_Minh" must stay 09:00 local across a DST change in any
/// other zone, and storing a UTC-normalised time would freeze it to whichever
/// offset applied on the day it was saved.
pub fn next_run_after(
    crontab: &str,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<DateTime<Utc>, CronError> {
    let tz = chrono_tz::Tz::from_str(timezone)
        .map_err(|_| CronError::UnknownTimezone(timezone.to_string()))?;
    let cron = parse(crontab)?;

    let local_after = tz.from_utc_datetime(&after.naive_utc());
    let next = cron
        .find_next_occurrence(&local_after, false)
        .map_err(|_| CronError::NoOccurrence)?;
    Ok(next.with_timezone(&Utc))
}

/// Validate `crontab` alone, for the write path's 400. Separate from
/// [`next_run_after`] so a caller can reject a bad expression without also
/// having to have a valid timezone in hand.
pub fn validate(crontab: &str, timezone: &str) -> Result<(), CronError> {
    chrono_tz::Tz::from_str(timezone)
        .map_err(|_| CronError::UnknownTimezone(timezone.to_string()))?;
    parse(crontab).map(|_| ())
}

fn parse(crontab: &str) -> Result<Cron, CronError> {
    Cron::new(crontab)
        .parse()
        .map_err(|e| CronError::InvalidCrontab(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn a_five_field_crontab_is_what_users_type_and_must_parse() {
        // The trap this pins: the `cron` crate wants a leading SECONDS field,
        // so it reads "0 9 * * *" as second=0 minute=9 hour=* — a schedule that
        // fires 60x a day instead of once, with no error to notice.
        assert!(validate("0 9 * * *", "UTC").is_ok());
        let next = next_run_after("0 9 * * *", "UTC", utc("2026-03-01T10:00:00Z")).unwrap();
        assert_eq!(next, utc("2026-03-02T09:00:00Z"), "must be 09:00 tomorrow");
    }

    #[test]
    fn the_expression_is_evaluated_in_the_schedules_own_timezone() {
        // 09:00 in Ho Chi Minh (UTC+7, no DST) is 02:00 UTC.
        let next =
            next_run_after("0 9 * * *", "Asia/Ho_Chi_Minh", utc("2026-03-01T10:00:00Z")).unwrap();
        assert_eq!(next, utc("2026-03-02T02:00:00Z"));
    }

    #[test]
    fn a_dst_shift_keeps_the_local_hour_not_the_utc_hour() {
        // US DST began 2026-03-08. A 09:00 New York schedule is 14:00 UTC
        // before the shift and 13:00 UTC after it. A design that normalised to
        // UTC at save time would keep sending at the wrong local hour for half
        // the year — which is the bug users report as "my report moved".
        let before =
            next_run_after("0 9 * * *", "America/New_York", utc("2026-03-06T20:00:00Z")).unwrap();
        assert_eq!(before, utc("2026-03-07T14:00:00Z"));
        let after =
            next_run_after("0 9 * * *", "America/New_York", utc("2026-03-09T20:00:00Z")).unwrap();
        assert_eq!(after, utc("2026-03-10T13:00:00Z"));
    }

    #[test]
    fn an_unparseable_crontab_is_an_error_not_a_default() {
        // Defaulting a bad expression to "hourly" would be the worst outcome:
        // the user gets mail on a schedule they never asked for.
        assert!(matches!(
            validate("not a cron", "UTC"),
            Err(CronError::InvalidCrontab(_))
        ));
        assert!(matches!(
            validate("* * *", "UTC"),
            Err(CronError::InvalidCrontab(_))
        ));
    }

    #[test]
    fn an_unknown_timezone_is_rejected() {
        assert_eq!(
            validate("0 9 * * *", "Mars/Olympus"),
            Err(CronError::UnknownTimezone("Mars/Olympus".into()))
        );
    }

    #[test]
    fn next_run_is_strictly_after_the_given_instant() {
        // Otherwise a schedule that just ran at exactly 09:00:00 would compute
        // 09:00:00 again as its next run and spin on the same tick.
        let exactly_now = utc("2026-03-02T09:00:00Z");
        let next = next_run_after("0 9 * * *", "UTC", exactly_now).unwrap();
        assert!(next > exactly_now, "got {next}, expected strictly later");
        assert_eq!(next, utc("2026-03-03T09:00:00Z"));
    }

    #[test]
    fn the_migrations_default_timezone_resolves() {
        // Migration 0110 defaults `timezone` to 'UTC'; if chrono-tz could not
        // resolve that name, every schedule created without an explicit zone
        // would fail to compute a next run.
        assert!(chrono_tz::Tz::from_str("UTC").is_ok());
        assert!(validate("0 9 * * *", "UTC").is_ok());
    }
}
