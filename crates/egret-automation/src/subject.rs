//! `{{token}}` substitution for a notification's subject line.
//!
//! "Daily orders" arriving thirty times with the same subject collapses into
//! one mail thread and nobody can find yesterday's. A subject wants the run's
//! date in it, so this renders a small fixed set of placeholders.
//!
//! `{{name}}` style is the house convention — SQL Lab parameters, chart link
//! templates and dashboard URL templates all use it, so a user who has met one
//! of those already knows this.
//!
//! Two rules make it safe to put user text in a mail header:
//!
//! - The set is CLOSED and every value is generated here. Nothing the user
//!   writes is interpolated, so a subject cannot carry a `\r\n` and inject a
//!   header (`lettre` would reject it, but the run would fail at send time —
//!   long after the person who typed it could see why).
//! - An unknown token is rejected at SAVE time by [`validate`], never at send
//!   time. A typo like `{{dat}}` should fail while its author is looking at
//!   the form; failing during delivery would lose the notification itself.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

/// The placeholders a subject may use, each with the line the editor shows.
///
/// Kept as data rather than a `match` with a parallel list, so the editor's
/// help text and the renderer cannot drift apart.
pub const TOKENS: &[(&str, &str)] = &[
    ("date", "the run's date, 2026-08-13"),
    ("time", "the run's time, 09:00"),
    ("datetime", "date and time, 2026-08-13 09:00"),
    ("day", "day of the month, 13"),
    ("month", "month number, 08"),
    ("year", "year, 2026"),
    ("weekday", "day name, Thursday"),
    ("name", "the schedule's name"),
];

/// Substitute the known tokens in `template`.
///
/// `at` is rendered in `tz` — the schedule's own timezone, not UTC. A report
/// that runs 09:00 in Asia/Ho_Chi_Minh is "today's" report to the people
/// reading it, and UTC would date a third of the year's editions wrongly.
///
/// An unrecognised token is left VERBATIM rather than blanked: by the time a
/// subject is being rendered the mail is already worth sending, and a visible
/// `{{dat}}` tells the recipient exactly what to fix. [`validate`] is what
/// stops it ever getting this far.
pub fn render(template: &str, name: &str, at: DateTime<Utc>, tz: &str) -> String {
    let local = at.with_timezone(&tz.parse::<Tz>().unwrap_or(chrono_tz::UTC));
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            // An unterminated `{{` is literal text, not a broken token.
            out.push_str(&rest[start..]);
            return out;
        };
        let key = after[..end].trim();
        match key {
            "date" => out.push_str(&local.format("%Y-%m-%d").to_string()),
            "time" => out.push_str(&local.format("%H:%M").to_string()),
            "datetime" => out.push_str(&local.format("%Y-%m-%d %H:%M").to_string()),
            "day" => out.push_str(&local.format("%d").to_string()),
            "month" => out.push_str(&local.format("%m").to_string()),
            "year" => out.push_str(&local.format("%Y").to_string()),
            "weekday" => out.push_str(&local.format("%A").to_string()),
            "name" => out.push_str(name),
            _ => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

/// Reject a subject that uses a token this renderer does not know.
///
/// Called from the route's validation, so the message names the offending
/// token while its author is still on the form.
pub fn validate(template: &str) -> Result<(), String> {
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Ok(());
        };
        let key = after[..end].trim();
        if !TOKENS.iter().any(|(t, _)| *t == key) {
            let known: Vec<&str> = TOKENS.iter().map(|(t, _)| *t).collect();
            return Err(format!(
                "unknown subject placeholder {{{{{key}}}}} — try one of: {}",
                known.join(", ")
            ));
        }
        rest = &after[end + 2..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> DateTime<Utc> {
        // 2026-08-13T02:30Z — deliberately late enough in UTC that Ho Chi Minh
        // (+07) is already on the NEXT day's morning, so a timezone bug cannot
        // hide behind a same-day answer.
        "2026-08-13T02:30:00Z".parse().unwrap()
    }

    #[test]
    fn renders_the_date_in_the_schedules_own_timezone() {
        let out = render("[egret] orders {{date}} {{time}}", "orders", at(), "UTC");
        assert_eq!(out, "[egret] orders 2026-08-13 02:30");

        let out = render(
            "[egret] orders {{date}} {{time}}",
            "orders",
            at(),
            "Asia/Ho_Chi_Minh",
        );
        assert_eq!(out, "[egret] orders 2026-08-13 09:30");
    }

    /// The case the timezone actually decides: 21:00 UTC is the following day
    /// in Ho Chi Minh, so a UTC-rendered subject would date it a day early.
    #[test]
    fn a_late_run_is_dated_by_the_local_day_not_utc() {
        let evening: DateTime<Utc> = "2026-08-13T21:00:00Z".parse().unwrap();
        assert_eq!(
            render("{{date}} {{weekday}}", "n", evening, "Asia/Ho_Chi_Minh"),
            "2026-08-14 Friday"
        );
        assert_eq!(
            render("{{date}} {{weekday}}", "n", evening, "UTC"),
            "2026-08-13 Thursday"
        );
    }

    #[test]
    fn substitutes_the_name_and_the_parts() {
        assert_eq!(
            render(
                "{{name}}: {{year}}-{{month}}-{{day}}",
                "Q3 pack",
                at(),
                "UTC"
            ),
            "Q3 pack: 2026-08-13"
        );
    }

    #[test]
    fn tolerates_spaces_inside_the_braces() {
        assert_eq!(render("{{ date }}", "n", at(), "UTC"), "2026-08-13");
    }

    /// An unknown token survives to the subject rather than blanking it: the
    /// recipient can see what to fix, and the mail still arrives.
    #[test]
    fn leaves_an_unknown_token_verbatim() {
        assert_eq!(
            render("a {{dat}} b {{date}}", "n", at(), "UTC"),
            "a {{dat}} b 2026-08-13"
        );
    }

    #[test]
    fn an_unterminated_brace_is_literal_text() {
        assert_eq!(render("cost {{ 100", "n", at(), "UTC"), "cost {{ 100");
    }

    /// An unknown token is a SAVE-time error — the only place its author is
    /// looking. The message names the token and lists the alternatives.
    #[test]
    fn validate_names_the_offending_token() {
        assert!(validate("[egret] {{date}} {{name}}").is_ok());
        let err = validate("[egret] {{dat}}").unwrap_err();
        assert!(err.contains("{{dat}}"), "{err}");
        assert!(err.contains("date"), "{err}");
    }

    /// A bad timezone must not lose the notification. It falls back to UTC,
    /// and the row cannot hold one anyway — `timezone` is validated on write.
    #[test]
    fn an_unparseable_timezone_falls_back_to_utc() {
        assert_eq!(render("{{time}}", "n", at(), "Mars/Olympus"), "02:30");
    }
}
